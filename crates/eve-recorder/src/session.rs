//! Session orchestration: client discovery, warmup barrier, thread
//! fan-out, the shared stop flag wired to Ctrl-C, and the manifest.
//!
//! All streams warm up in parallel (UIRoot scan, encoder probe, WGC +
//! ffmpeg init, hook install); the shared clock — the session `t = 0` —
//! is created only after every stream reports ready, then released to
//! all of them at once. Recording therefore starts aligned, with no
//! stream's first record buried in another stream's initialization.
//!
//! Shutdown is an ordered drain — hooks first, then observers and
//! captures (each capture thread finalizes its ffmpeg mp4 before
//! exiting) — and every writer flushes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use serde_json::json;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use eve_memory::discovery::{GameClient, discover_clients};

use crate::capture::{self, VideoSize, VideoStats};
use crate::clock::{SessionClock, SharedClock};
use crate::error::{RecorderError, Result};
use crate::input::{ClientRef, InputRecorder, InputStats};
use crate::observe::{self, ObserveStats};

/// Session-wide stop signal; the console handler flips it on Ctrl-C (and
/// on console close/logoff, where the process then has ~5 s before the
/// OS kills it — the ordered drain is designed to fit comfortably).
static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    /// PIDs to record; empty = every discovered client.
    pub pids: Vec<u32>,
    /// Output directory (created if missing).
    pub out_dir: PathBuf,
    /// Observation polling interval (500 ms = 2 fps).
    pub interval_ms: u64,
    /// Minimum spacing of mouse_move lines (16 ≈ 60 fps; 0 = all).
    pub move_every_ms: u64,
    /// Video frame rate.
    pub video_fps: u32,
    /// Explicit encoder backend name; `None` = auto (probe + throughput
    /// gate, hardware first).
    pub encoder: Option<String>,
    /// Target video resolution budget.
    pub video_size: VideoSize,
    /// Disable the video stream entirely.
    pub no_video: bool,
    /// Stop automatically after this many seconds.
    pub duration_sec: Option<u64>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            pids: Vec::new(),
            out_dir: PathBuf::from("."),
            interval_ms: 500,
            move_every_ms: 16,
            video_fps: 15,
            encoder: None,
            video_size: VideoSize::P720,
            no_video: false,
            duration_sec: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientSummary {
    pub pid: u32,
    pub flavor: String,
    pub character_name: Option<String>,
    pub window_title: String,
    pub observe: ObserveStats,
    pub video: Option<VideoStats>,
}

#[derive(Debug)]
pub struct SessionSummary {
    pub out_dir: PathBuf,
    pub started_at_unix_ms: u128,
    pub ended_at_unix_ms: u128,
    pub input: InputStats,
    pub clients: Vec<ClientSummary>,
}

unsafe extern "system" fn console_ctrl_handler(_ctrl_type: u32) -> windows::core::BOOL {
    STOP.store(true, Ordering::Relaxed);
    windows::core::BOOL(1)
}

/// Run a full recording session; returns when stopped (Ctrl-C), the
/// duration elapses, or every client exits.
pub fn record_session(config: RecorderConfig) -> Result<SessionSummary> {
    // Physical pixels everywhere: hook coordinates, WGC frames and the
    // UI tree regions all agree.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if SetConsoleCtrlHandler(Some(console_ctrl_handler), true).is_err() {
            tracing::warn!("SetConsoleCtrlHandler failed; Ctrl-C will hard-kill the session");
        }
    }
    STOP.store(false, Ordering::Relaxed);

    let all = discover_clients()?;
    let clients: Vec<GameClient> = if config.pids.is_empty() {
        all
    } else {
        config
            .pids
            .iter()
            .map(|want| {
                all.iter()
                    .find(|c| c.pid == *want)
                    .cloned()
                    .ok_or(RecorderError::UnknownPid(*want))
            })
            .collect::<Result<Vec<_>>>()?
    };
    if clients.is_empty() {
        return Err(RecorderError::NoClients);
    }

    std::fs::create_dir_all(config.out_dir.join("clients"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = channel::<&'static str>();
    let mut gates: Vec<Sender<SharedClock>> = Vec::new();

    // --- Spawn all streams; each warms up, reports ready, waits on its gate.
    let refs: Vec<ClientRef> = clients
        .iter()
        .map(|c| ClientRef { pid: c.pid, hwnd_raw: c.window_handle_raw().unwrap_or(0) })
        .collect();
    let (input_clock_tx, input_clock_rx) = channel();
    gates.push(input_clock_tx);
    tracing::info!("warming up: input hooks…");
    let input_recorder =
        InputRecorder::spawn(refs, config.move_every_ms, config.out_dir.join("input.jsonl"), input_clock_rx, ready_tx.clone())?;

    let mut observers = Vec::new();
    for client in &clients {
        let (tx, rx) = channel();
        gates.push(tx);
        tracing::info!(pid = client.pid, "warming up: observation (UIRoot scan)…");
        observers.push((
            client.pid,
            observe::spawn_observer(
                Arc::clone(&stop),
                client.clone(),
                Duration::from_millis(config.interval_ms.max(5)),
                config.out_dir.join("clients").join(format!("{}.jsonl", client.pid)),
                rx,
                ready_tx.clone(),
            ),
        ));
    }

    let mut captures = Vec::new();
    if !config.no_video {
        for client in &clients {
            let (tx, rx) = channel();
            gates.push(tx);
            tracing::info!(pid = client.pid, "warming up: video capture (WGC + in-process encoder)…");
            captures.push((
                client.pid,
                capture::spawn_capture(
                    Arc::clone(&stop),
                    client.clone(),
                    config.video_fps,
                    config.video_size,
                    config.encoder.clone(),
                    config.out_dir.join("clients").join(format!("{}.mp4", client.pid)),
                    rx,
                    ready_tx.clone(),
                )?,
            ));
        }
    }

    // --- Barrier: wait for every stream's warmup, then release one clock.
    let expected = gates.len();
    for i in 0..expected {
        match ready_rx.recv() {
            Ok(name) => tracing::info!("[{}/{}] {name} ready", i + 1, expected),
            Err(_) => {
                tracing::error!("a stream died during warmup");
                return Err(RecorderError::WarmupFailed);
            }
        }
    }
    let clock: SharedClock = Arc::new(SessionClock::new());
    for gate in gates {
        let _ = gate.send(Arc::clone(&clock));
    }
    tracing::info!("all streams ready — recording starts now (t=0)");

    write_manifest(&config, &clock, &clients, &[], None)?;

    // Main wait: Ctrl-C flag or duration deadline.
    let deadline = config
        .duration_sec
        .map(|s| Instant::now() + Duration::from_secs(s));
    loop {
        if STOP.load(Ordering::Relaxed) {
            tracing::info!("stopping: Ctrl-C received");
            break;
        }
        if let Some(d) = deadline {
            if Instant::now() >= d {
                tracing::info!("stopping: duration reached");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    stop.store(true, Ordering::Relaxed);

    // --- Ordered drain ---
    // 1. Unhook input first so no further human events arrive.
    input_recorder.stop();
    let input_stats = input_recorder.join();
    // 2./3. Observers and captures see the flag; each flushes/finalizes
    //    (capture threads wait for their ffmpeg to write the mp4 trailer).
    let mut observe_by_pid = Vec::new();
    for (pid, handle) in observers {
        let stats = handle.join().expect("observer panicked");
        observe_by_pid.push((pid, stats));
    }
    let mut video_by_pid = Vec::new();
    for (pid, handle) in captures {
        let stats = handle.join().expect("capture panicked");
        video_by_pid.push((pid, stats));
    }

    let mut clients_out = Vec::new();
    for client in &clients {
        let observe = observe_by_pid
            .iter()
            .find(|(p, _)| *p == client.pid)
            .map(|(_, s)| s.clone())
            .unwrap_or(ObserveStats {
                frames: 0,
                failed_reads: 0,
                slow_reads: 0,
                stopped_reason: "missing".into(),
            });
        let video = video_by_pid
            .iter()
            .find(|(p, _)| *p == client.pid)
            .map(|(_, s)| s.clone());
        clients_out.push(ClientSummary {
            pid: client.pid,
            flavor: client.flavor.display_name().to_string(),
            character_name: client.character_name.clone(),
            window_title: client.window_title.clone(),
            observe,
            video,
        });
    }

    let ended_rel = clock.now_rel_ms();
    write_manifest(&config, &clock, &clients, &clients_out, Some(&input_stats))?;

    Ok(SessionSummary {
        out_dir: config.out_dir,
        started_at_unix_ms: clock.started_at_unix_ms(),
        ended_at_unix_ms: clock.to_unix_ms(ended_rel),
        input: input_stats,
        clients: clients_out,
    })
}

fn write_manifest(
    config: &RecorderConfig,
    clock: &SessionClock,
    clients: &[GameClient],
    final_stats: &[ClientSummary],
    input: Option<&InputStats>,
) -> Result<()> {
    let clients_json: Vec<_> = clients
        .iter()
        .map(|c| {
            let summary = final_stats.iter().find(|s| s.pid == c.pid);
            let video = summary.and_then(|s| s.video.as_ref());
            json!({
                "pid": c.pid,
                "flavor": c.flavor.display_name(),
                "character_name": c.character_name,
                "window_title": c.window_title,
                "observe_file": format!("clients/{}.jsonl", c.pid),
                "observe_frames": summary.map(|s| s.observe.frames).unwrap_or(0),
                "observe_failed_reads": summary.map(|s| s.observe.failed_reads).unwrap_or(0),
                "observe_slow_reads": summary.map(|s| s.observe.slow_reads).unwrap_or(0),
                "observe_stopped_reason": summary
                    .map(|s| s.observe.stopped_reason.clone())
                    .unwrap_or_else(|| "recording".into()),
                "video_file": (!config.no_video).then(|| format!("clients/{}.mp4", c.pid)),
                "video_encoder": video.map(|v| v.encoder.clone()),
                "video_size": video.map(|v| v.video_size),
                "video_native_width": video.map(|v| v.native_width),
                "video_native_height": video.map(|v| v.native_height),
                "video_scale": video.map(|v| v.scale),
                "video_encoder_error": video.and_then(|v| v.encoder_error.clone()),
                "video_anchor_t_ms": video.and_then(|v| v.anchor_t_ms),
                "video_fps": video.map(|v| v.fps),
                "video_width": video.map(|v| v.width),
                "video_height": video.map(|v| v.height),
                "video_frames": video.map(|v| v.frames_written),
                "video_dropped_frames": video.map(|v| v.frames_dropped),
                "video_client_offset": video.map(|v| v.client_offset),
                "video_stopped_reason": video.map(|v| v.stopped_reason.clone()),
            })
        })
        .collect();
    let manifest = json!({
        "format_version": 2,
        "started_at_unix_ms": clock.started_at_unix_ms(),
        "ended_at_unix_ms": (!final_stats.is_empty()).then(|| clock.to_unix_ms(clock.now_rel_ms())),
        "params": {
            "interval_ms": config.interval_ms,
            "move_every_ms": config.move_every_ms,
            "video_fps": config.video_fps,
            "no_video": config.no_video,
            "encoder": config.encoder.clone(),
        },
        "clients": clients_json,
        "input_events": input.map(|i| i.events_written),
        "input_dropped": input.map(|i| i.events_dropped),
    });
    std::fs::write(
        config.out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}
