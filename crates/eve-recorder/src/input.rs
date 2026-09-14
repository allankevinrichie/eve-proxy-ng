//! Human-input recording: global low-level hooks (keyboard + mouse),
//! event-to-client attribution, and a ~60 fps mouse-move aggregator.
//!
//! Architecture:
//! - A pump thread installs `WH_KEYBOARD_LL` / `WH_MOUSE_LL` and runs a
//!   `GetMessage` loop (LL hooks are called back on the installing
//!   thread). The callback resolves ownership **at event time** — mouse
//!   via `WindowFromPoint`, keyboard via the foreground window, both
//!   mapped pid→recorded-client — converts screen to client-area
//!   coordinates, and `try_send`s into a bounded channel. All of this is
//!   a few microseconds per event, well inside the system hook budget;
//!   nothing ever blocks.
//! - Attribution at event time (rather than when the consumer dequeues)
//!   matters: queued events would otherwise be attributed against the
//!   window layout of the future. EVE has no global hotkeys — a keypress
//!   belongs to whatever client window is foreground at that instant;
//!   a mouse event belongs to the client whose window is under the
//!   cursor. While a button is held, motion stays attributed to the
//!   client where the press started (drag continuity) even outside its
//!   window.
//! - A consumer thread coalesces mouse moves to the configured spacing
//!   (default one line per ~16 ms bucket, keeping the newest position)
//!   and appends to `input.jsonl`.
//! - Injected input (`LLMHF_INJECTED` / `LLKHF_INJECTED`, e.g. SendInput)
//!   is **recorded, not filtered**: every line carries an `injected:
//!   true/false` flag, so a session mixing human play and executor
//!   actions stays a faithful log of what the client actually received
//!   while remaining separable for analysis. Modifier state counts
//!   injected keys too (they set the system-visible modifier state).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_UP, LLMHF_INJECTED, MSLLHOOKSTRUCT, MSG,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WindowFromPoint,
};

use crate::clock::SharedClock;
use crate::jsonl::JsonlWriter;

/// A client the recorder attributes input to.
#[derive(Clone)]
pub struct ClientRef {
    pub pid: u32,
    /// Raw HWND of the main window, used for client-area mapping.
    pub hwnd_raw: isize,
}

// ---------------------------------------------------------------------------
// Raw events (hook → consumer), already attributed at event time
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Button {
    Left,
    Right,
    Middle,
    X,
}

impl Button {
    fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
            Button::X => "x",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum RawEv {
    Move { x: i32, y: i32, client_x: Option<i64>, client_y: Option<i64> },
    Button {
        down: bool,
        button: Button,
        x: i32,
        y: i32,
        client_x: Option<i64>,
        client_y: Option<i64>,
    },
    Wheel { delta: i32, x: i32, y: i32, client_x: Option<i64>, client_y: Option<i64> },
    Key { down: bool, vk: u32, scancode: u32 },
}

struct TimedRawEvent {
    t: Instant,
    /// Owning recorded client, resolved when the event occurred.
    pid: u32,
    ev: RawEv,
    /// Event was synthesized (SendInput & friends) rather than physical —
    /// recorded faithfully, flagged for the analyzer to separate human
    /// input from executor actions.
    injected: bool,
}

// ---------------------------------------------------------------------------
// Hook-side globals (callbacks run on the pump thread)
// ---------------------------------------------------------------------------

static EVENT_TX: OnceLock<Mutex<Option<SyncSender<TimedRawEvent>>>> = OnceLock::new();
/// pid → client map for at-event-time attribution. Written once at
/// session start; callbacks only read it.
static CLIENTS: OnceLock<Arc<HashMap<u32, ClientRef>>> = OnceLock::new();
/// Client whose window received the current button press; 0 = none.
/// Keeps drags attributed to their origin client across window bounds.
static DRAG_PID: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

const MOD_SHIFT: u8 = 1 << 0;
const MOD_CTRL: u8 = 1 << 1;
const MOD_ALT: u8 = 1 << 2;
const MOD_WIN: u8 = 1 << 3;
static MODS: AtomicU8 = AtomicU8::new(0);

fn send_raw(t: Instant, pid: u32, ev: RawEv, injected: bool) {
    let Some(cell) = EVENT_TX.get() else { return };
    // The slot mutex is only ever held at session start/stop; in steady
    // state this try_lock always succeeds.
    if let Ok(guard) = cell.try_lock() {
        if let Some(tx) = guard.as_ref() {
            if tx.try_send(TimedRawEvent { t, pid, ev, injected }).is_err() {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
    } else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Recorded client under the screen point, with client-area coordinates.
fn resolve_point(x: i32, y: i32) -> Option<(u32, Option<i64>, Option<i64>)> {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let client = CLIENTS.get()?.get(&pid)?;
        let (cx, cy) = client_coords(client, x, y);
        Some((pid, cx, cy))
    }
}

/// Recorded client currently in the foreground (EVE has no global
/// hotkeys: a keypress goes to the foreground client or nowhere).
fn resolve_foreground() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        CLIENTS.get()?.contains_key(&pid).then_some(pid)
    }
}

/// Screen point → that client's client-area coordinates.
fn client_coords(client: &ClientRef, x: i32, y: i32) -> (Option<i64>, Option<i64>) {
    let handle = eve_memory::WindowHandle::from_raw(client.hwnd_raw);
    match handle.and_then(|h| h.client_rect_screen().ok()) {
        Some(rect) => (
            Some(x as i64 - rect.x as i64),
            Some(y as i64 - rect.y as i64),
        ),
        None => (None, None),
    }
}

/// Attribute a cursor event: the client under the cursor, or — mid-drag —
/// the client where the press started (motion outside its window stays
/// attributed to it until release).
fn resolve_cursor_event(x: i32, y: i32) -> Option<(u32, Option<i64>, Option<i64>)> {
    if let Some(hit) = resolve_point(x, y) {
        return Some(hit);
    }
    let drag = DRAG_PID.load(Ordering::Relaxed);
    if drag != 0 {
        let client = CLIENTS.get()?.get(&drag)?;
        let (cx, cy) = client_coords(client, x, y);
        return Some((drag, cx, cy));
    }
    None
}

fn update_mods(vk: u32, down: bool) {
    let bit = match vk {
        0x10 | 0xA0 | 0xA1 => MOD_SHIFT, // SHIFT / LSHIFT / RSHIFT
        0x11 | 0xA2 | 0xA3 => MOD_CTRL,
        0x12 | 0xA4 | 0xA5 => MOD_ALT,
        0x5B | 0x5C => MOD_WIN,
        _ => return,
    };
    if down {
        MODS.fetch_or(bit, Ordering::Relaxed);
    } else {
        MODS.fetch_and(!bit, Ordering::Relaxed);
    }
}

fn mods_names() -> Vec<&'static str> {
    let m = MODS.load(Ordering::Relaxed);
    let mut out = Vec::new();
    if m & MOD_CTRL != 0 {
        out.push("ctrl");
    }
    if m & MOD_SHIFT != 0 {
        out.push("shift");
    }
    if m & MOD_ALT != 0 {
        out.push("alt");
    }
    if m & MOD_WIN != 0 {
        out.push("win");
    }
    out
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        // Injected events are recorded too, flagged — the recording must
        // be a faithful log of what the client actually received.
        let injected = info.flags & LLMHF_INJECTED != 0;
        let x = info.pt.x;
        let y = info.pt.y;
        let t = Instant::now();
        match wparam.0 as u32 {
            WM_MOUSEMOVE => {
                if let Some((pid, cx, cy)) = resolve_cursor_event(x, y) {
                    send_raw(t, pid, RawEv::Move { x, y, client_x: cx, client_y: cy }, injected);
                }
            }
            WM_LBUTTONDOWN => button_event(t, true, Button::Left, x, y, wparam, injected),
            WM_LBUTTONUP => button_event(t, false, Button::Left, x, y, wparam, injected),
            WM_RBUTTONDOWN => button_event(t, true, Button::Right, x, y, wparam, injected),
            WM_RBUTTONUP => button_event(t, false, Button::Right, x, y, wparam, injected),
            WM_MBUTTONDOWN => button_event(t, true, Button::Middle, x, y, wparam, injected),
            WM_MBUTTONUP => button_event(t, false, Button::Middle, x, y, wparam, injected),
            WM_XBUTTONDOWN => button_event(t, true, Button::X, x, y, wparam, injected),
            WM_XBUTTONUP => button_event(t, false, Button::X, x, y, wparam, injected),
            WM_MOUSEWHEEL => {
                if let Some((pid, cx, cy)) = resolve_cursor_event(x, y) {
                    send_raw(
                        t,
                        pid,
                        RawEv::Wheel {
                            // High word of the wheel delta (signed,
                            // ±120 per notch).
                            delta: (info.mouseData >> 16) as u16 as i16 as i32,
                            x,
                            y,
                            client_x: cx,
                            client_y: cy,
                        },
                        injected,
                    );
                }
            }
            _ => {}
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

fn button_event(
    t: Instant,
    down: bool,
    button: Button,
    x: i32,
    y: i32,
    _wparam: WPARAM,
    injected: bool,
) {
    let hit = resolve_point(x, y);
    let attributed = if down {
        // A press starts (or fails) a drag on the client under the cursor.
        hit.inspect(|(pid, _, _)| DRAG_PID.store(*pid, Ordering::Relaxed))
    } else {
        // A release ends the drag; attribute to the drag owner when the
        // cursor has left the window.
        hit.or_else(|| {
            let drag = DRAG_PID.load(Ordering::Relaxed);
            if drag == 0 {
                return None;
            }
            CLIENTS.get().and_then(|c| c.get(&drag)).map(|client| {
                let (cx, cy) = client_coords(client, x, y);
                (drag, cx, cy)
            })
        })
        .inspect(|_| DRAG_PID.store(0, Ordering::Relaxed))
    };
    if let Some((pid, cx, cy)) = attributed {
        send_raw(t, pid, RawEv::Button { down, button, x, y, client_x: cx, client_y: cy }, injected);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = (info.flags & LLKHF_INJECTED).0 != 0;
        let down = (info.flags & LLKHF_UP).0 == 0;
        // Track modifiers for every key, attributed or not, so an
        // alt-tab's key-up never leaves a stale modifier behind; injected
        // keys also count — they set the system-visible modifier state.
        update_mods(info.vkCode, down);
        if let Some(pid) = resolve_foreground() {
            send_raw(
                Instant::now(),
                pid,
                RawEv::Key { down, vk: info.vkCode, scancode: info.scanCode },
                injected,
            );
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

// ---------------------------------------------------------------------------
// Consumer: aggregation + jsonl
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct InputStats {
    pub events_written: u64,
    pub events_dropped: u64,
}

/// Human-readable name for common virtual-key codes.
pub fn vk_name(vk: u32) -> String {
    match vk {
        0x08 => "backspace".into(),
        0x09 => "tab".into(),
        0x0D => "enter".into(),
        0x10 => "shift".into(),
        0x11 => "ctrl".into(),
        0x12 => "alt".into(),
        0x13 => "pause".into(),
        0x14 => "capslock".into(),
        0x1B => "esc".into(),
        0x20 => "space".into(),
        0x21 => "pageup".into(),
        0x22 => "pagedown".into(),
        0x23 => "end".into(),
        0x24 => "home".into(),
        0x25 => "left".into(),
        0x26 => "up".into(),
        0x27 => "right".into(),
        0x28 => "down".into(),
        0x2C => "printscreen".into(),
        0x2D => "insert".into(),
        0x2E => "delete".into(),
        0x30..=0x39 => format!("{}", (b'0' + (vk - 0x30) as u8) as char),
        0x41..=0x5A => format!("{}", (b'A' + (vk - 0x41) as u8) as char),
        0x5B => "lwin".into(),
        0x5C => "rwin".into(),
        0x60..=0x69 => format!("numpad{}", vk - 0x60),
        0x6A => "multiply".into(),
        0x6B => "add".into(),
        0x6D => "subtract".into(),
        0x6E => "decimal".into(),
        0x6F => "divide".into(),
        0x70..=0x87 => format!("f{}", vk - 0x6F),
        0xA0 => "lshift".into(),
        0xA1 => "rshift".into(),
        0xA2 => "lctrl".into(),
        0xA3 => "rctrl".into(),
        0xA4 => "lalt".into(),
        0xA5 => "ralt".into(),
        _ => format!("vk_{vk:02X}"),
    }
}

/// Newest move of the currently open ~16 ms bucket, coordinates already
/// resolved at event time.
struct PendingMove {
    t: Instant,
    x: i32,
    y: i32,
    client_x: Option<i64>,
    client_y: Option<i64>,
    pid: u32,
    injected: bool,
}

struct Aggregator {
    writer: JsonlWriter,
    clock: SharedClock,
    /// Minimum spacing between emitted mouse_move lines (60 fps default).
    move_every: Duration,
    pending_move: Option<PendingMove>,
    last_move_emit: Option<Instant>,
    written: u64,
}

impl Aggregator {
    fn new(
        writer: JsonlWriter,
        clock: SharedClock,
        clients: Vec<ClientRef>,
        move_every_ms: u64,
    ) -> Self {
        // Kept for interface symmetry; attribution happens hook-side.
        let _ = clients;
        Self {
            writer,
            clock,
            move_every: Duration::from_millis(move_every_ms),
            pending_move: None,
            last_move_emit: None,
            written: 0,
        }
    }

    fn emit_line(&mut self, pid: u32, mut value: Value) {
        let obj = value.as_object_mut().expect("input lines are objects");
        obj.insert("t_ms".into(), json!(self.clock.now_rel_ms()));
        obj.insert("pid".into(), json!(pid));
        if self.writer.write(&value).is_ok() {
            self.written += 1;
        }
    }

    fn handle(&mut self, ev: TimedRawEvent) {
        let t_ms = self.clock.rel_of(ev.t);
        let injected = ev.injected;
        match ev.ev {
            RawEv::Move { x, y, client_x, client_y } => {
                if self.move_every.is_zero() {
                    self.write_move(t_ms, ev.pid, x, y, client_x, client_y, injected);
                    self.last_move_emit = Some(ev.t);
                    return;
                }
                // Bucket boundary anchored at the last emitted bucket:
                // flush the previous bucket's newest position when this
                // event opens a new one.
                let opens_new_bucket = self
                    .last_move_emit
                    .is_none_or(|last| ev.t.saturating_duration_since(last) >= self.move_every);
                if opens_new_bucket {
                    self.flush_pending_move();
                    self.last_move_emit = Some(ev.t);
                }
                self.pending_move = Some(PendingMove {
                    t: ev.t,
                    x,
                    y,
                    client_x,
                    client_y,
                    pid: ev.pid,
                    injected,
                });
            }
            RawEv::Button { down, button, x, y, client_x, client_y } => {
                self.flush_pending_move();
                let line = json!({
                    "kind": if down { "mouse_down" } else { "mouse_up" },
                    "button": button.name(),
                    "x": x, "y": y,
                    "client_x": client_x, "client_y": client_y,
                    "mods": mods_names(),
                    "injected": injected,
                    "t_ms": t_ms,
                });
                self.emit_line(ev.pid, line);
            }
            RawEv::Wheel { delta, x, y, client_x, client_y } => {
                self.flush_pending_move();
                let line = json!({
                    "kind": "mouse_wheel",
                    "delta": delta,
                    "x": x, "y": y,
                    "client_x": client_x, "client_y": client_y,
                    "mods": mods_names(),
                    "injected": injected,
                    "t_ms": t_ms,
                });
                self.emit_line(ev.pid, line);
            }
            RawEv::Key { down, vk, scancode } => {
                let line = json!({
                    "kind": if down { "key_down" } else { "key_up" },
                    "vk": vk,
                    "vk_name": vk_name(vk),
                    "scancode": scancode,
                    "mods": mods_names(),
                    "injected": injected,
                    "t_ms": t_ms,
                });
                self.emit_line(ev.pid, line);
            }
        }
    }

    fn write_move(
        &mut self,
        t_ms: u64,
        pid: u32,
        x: i32,
        y: i32,
        client_x: Option<i64>,
        client_y: Option<i64>,
        injected: bool,
    ) {
        let line = json!({
            "kind": "mouse_move",
            "x": x, "y": y,
            "client_x": client_x, "client_y": client_y,
            "mods": mods_names(),
            "injected": injected,
            "t_ms": t_ms,
        });
        self.emit_line(pid, line);
    }

    fn flush_pending_move(&mut self) {
        let Some(p) = self.pending_move.take() else { return };
        self.write_move(
            self.clock.rel_of(p.t),
            p.pid,
            p.x,
            p.y,
            p.client_x,
            p.client_y,
            p.injected,
        );
    }

    /// When an unflushed move must go out (so trailing motion is not lost
    /// when the mouse stops moving).
    fn pending_deadline(&self) -> Option<Instant> {
        self.pending_move
            .as_ref()
            .map(|p| p.t + self.move_every)
            .or_else(|| self.last_move_emit.map(|l| l + self.move_every))
    }

    fn stats(&self) -> InputStats {
        InputStats {
            events_written: self.written,
            events_dropped: DROPPED.load(Ordering::Relaxed),
        }
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.writer.finish()
    }
}

// ---------------------------------------------------------------------------
// Public recorder control
// ---------------------------------------------------------------------------

pub struct InputRecorder {
    pump_tid: u32,
    pump_thread: JoinHandle<()>,
    consumer_thread: JoinHandle<InputStats>,
}

impl InputRecorder {
    /// Install hooks, report ready, then block until the session gate
    /// sends the shared clock — events before the gate are not recorded
    /// (the hook→channel slot is only armed once the clock exists).
    /// `move_every_ms` of 0 disables move throttling.
    pub fn spawn(
        clients: Vec<ClientRef>,
        move_every_ms: u64,
        path: PathBuf,
        clock_rx: Receiver<SharedClock>,
        ready_tx: mpsc::Sender<&'static str>,
    ) -> std::io::Result<Self> {
        let writer = JsonlWriter::create(&path)?;
        let _ = CLIENTS.set(Arc::new(
            clients.iter().cloned().map(|c| (c.pid, c)).collect::<HashMap<_, _>>(),
        ));
        DRAG_PID.store(0, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel::<TimedRawEvent>(8192);
        MODS.store(0, Ordering::Relaxed);

        let (tid_tx, tid_rx) = mpsc::channel::<(u32, bool)>();
        let pump_thread = std::thread::Builder::new()
            .name("input-hook-pump".into())
            .spawn(move || {
                unsafe {
                    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0);
                    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
                    let ok = keyboard.is_ok() && mouse.is_ok();
                    let _ = tid_tx.send((
                        windows::Win32::System::Threading::GetCurrentThreadId(),
                        ok,
                    ));
                    if ok {
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            // The pump exists solely to service the hooks.
                        }
                        let _ = UnhookWindowsHookEx(keyboard.unwrap());
                        let _ = UnhookWindowsHookEx(mouse.unwrap());
                    }
                }
                // Clear the shared slot so callbacks stop referencing the
                // channel (the arming sender lives only in the slot, so
                // clearing it also closes the consumer's channel).
                if let Some(cell) = EVENT_TX.get() {
                    if let Ok(mut guard) = cell.try_lock() {
                        *guard = None;
                    }
                }
            })
            .expect("spawn hook pump");

        let (tid, hooks_ok) = tid_rx.recv().unwrap_or((0, false));
        if !hooks_ok {
            let _ = pump_thread.join();
            let _ = ready_tx.send("input-failed");
            return Err(std::io::Error::other("SetWindowsHookExW failed (LL hooks)"));
        }

        let _ = ready_tx.send("input");
        // The gate wait lives inside the consumer thread so this spawn
        // returns immediately (the session keeps spawning other streams).
        let consumer_thread = std::thread::Builder::new()
            .name("input-aggregator".into())
            .spawn(move || {
                let clock = match clock_rx.recv() {
                    Ok(c) => c,
                    Err(_) => return InputStats::default(),
                };
                // Arm the hook→channel slot only now: pre-gate events are
                // not part of the session timeline.
                {
                    let tx_slot = EVENT_TX.get_or_init(|| Mutex::new(None));
                    let mut guard = tx_slot.lock().unwrap();
                    if guard.is_some() {
                        tracing::error!("an input recorder is already active");
                        return InputStats::default();
                    }
                    *guard = Some(tx);
                }
                run_consumer(rx, clock, clients, move_every_ms, writer)
            })
            .expect("spawn input consumer");

        Ok(Self { pump_tid: tid, pump_thread, consumer_thread })
    }

    /// Signal the pump to unhook; the consumer drains and exits once the
    /// channel closes.
    pub fn stop(&self) {
        if self.pump_tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.pump_tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn join(self) -> InputStats {
        let _ = self.pump_thread.join();
        self.consumer_thread.join().unwrap_or_default()
    }
}

fn run_consumer(
    rx: Receiver<TimedRawEvent>,
    clock: SharedClock,
    clients: Vec<ClientRef>,
    move_every_ms: u64,
    writer: JsonlWriter,
) -> InputStats {
    let mut agg = Aggregator::new(writer, clock, clients, move_every_ms);
    loop {
        let timeout = agg
            .pending_deadline()
            .map(|d| d.saturating_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_millis(50));
        match rx.recv_timeout(timeout) {
            Ok(ev) => agg.handle(ev),
            Err(RecvTimeoutError::Timeout) => {
                if let Some(deadline) = agg.pending_deadline() {
                    if Instant::now() >= deadline {
                        agg.flush_pending_move();
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    agg.flush_pending_move();
    let stats = agg.stats();
    let _ = agg.finish();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SessionClock;
    use std::sync::Arc;

    fn temp_jsonl(tag: &str) -> (JsonlWriter, PathBuf) {
        let dir = std::env::temp_dir().join(format!("eve-rec-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("input.jsonl");
        (JsonlWriter::create(&path).unwrap(), path)
    }

    fn clock() -> SharedClock {
        Arc::new(SessionClock::new())
    }

    /// Moves within one 16 ms bucket coalesce to one line keeping the
    /// newest position; the next bucket flushes it. Each line carries the
    /// `injected` flag of its newest event.
    #[test]
    fn moves_coalesce_per_bucket() {
        let (writer, path) = temp_jsonl("bucket");
        let mut agg = Aggregator::new(writer, clock(), vec![], 16);
        let t0 = Instant::now();
        agg.handle(TimedRawEvent {
            t: t0,
            pid: 1,
            ev: RawEv::Move { x: 10, y: 10, client_x: Some(1), client_y: Some(2) },
            injected: false,
        });
        agg.handle(TimedRawEvent {
            t: t0 + Duration::from_millis(5),
            pid: 1,
            ev: RawEv::Move { x: 20, y: 20, client_x: Some(11), client_y: Some(12) },
            injected: true,
        });
        agg.handle(TimedRawEvent {
            t: t0 + Duration::from_millis(21),
            pid: 1,
            ev: RawEv::Move { x: 30, y: 30, client_x: Some(21), client_y: Some(22) },
            injected: false,
        });
        agg.flush_pending_move();
        agg.finish().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(lines.len(), 2, "two buckets → two move lines");
        assert_eq!(lines[0]["x"], 20, "first bucket keeps the newest position");
        assert_eq!(lines[0]["client_x"], 11);
        assert_eq!(lines[1]["x"], 30);
        // The flag follows the newest (kept) event of each bucket.
        assert_eq!(lines[0]["injected"], true);
        assert_eq!(lines[1]["injected"], false);
        // Lines are ordered oldest-first.
        assert!(lines[0]["t_ms"].as_u64() <= lines[1]["t_ms"].as_u64());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// Discrete events (button/wheel/key) flush the pending move first so
    /// click position is never lost to throttling.
    #[test]
    fn click_flushes_pending_move() {
        let (writer, path) = temp_jsonl("click");
        let mut agg = Aggregator::new(writer, clock(), vec![], 16);
        let t0 = Instant::now();
        agg.handle(TimedRawEvent {
            t: t0,
            pid: 7,
            ev: RawEv::Move { x: 5, y: 5, client_x: Some(0), client_y: Some(0) },
            injected: false,
        });
        agg.handle(TimedRawEvent {
            t: t0 + Duration::from_millis(3),
            pid: 7,
            ev: RawEv::Button {
                down: true,
                button: Button::Left,
                x: 6,
                y: 6,
                client_x: Some(1),
                client_y: Some(1),
            },
            injected: false,
        });
        agg.finish().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["kind"], "mouse_move");
        assert_eq!(lines[1]["kind"], "mouse_down");
        assert_eq!(lines[1]["pid"], 7);
        assert_eq!(lines[1]["vk_name"], serde_json::Value::Null);
        assert_eq!(lines[1]["injected"], false);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn vk_names_cover_common_keys() {
        assert_eq!(vk_name(0x41), "A");
        assert_eq!(vk_name(0x74), "f5");
        assert_eq!(vk_name(0x0D), "enter");
        assert_eq!(vk_name(0xFF), "vk_FF");
    }
}
