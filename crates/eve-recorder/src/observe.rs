//! Per-client observation pipeline, two-stage and frame-parallel:
//!
//! ```text
//! read thread:   read_tree ──▶ read_tree ──▶ …   (paced by `interval`)
//!                   │             │
//!                   ▼ (bounded channel, capacity 2)
//! parse thread:  parse+diff+write ──› parse+diff+write ──› …
//! ```
//!
//! The frame period becomes `max(read, parse)` instead of their sum:
//! the read thread continuously re-reads the full tree (one
//! point-in-time snapshot per frame — dynamic values are never carried
//! across frames, so UI updates cannot be missed), while the parser
//! consumes the previous frame's owned `UiNode` snapshot concurrently.
//! The bounded channel provides back-pressure: if parsing ever becomes
//! the slower stage, the read thread blocks on send and the pace stays
//! at the slower stage's rate instead of building a queue.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;

use eve_memory::UiNode;
use eve_memory::UiReader;
use eve_memory::discovery::GameClient;

use crate::clock::SharedClock;
use crate::delta::SnapshotDiffer;
use crate::jsonl::JsonlWriter;

#[derive(Debug, Clone)]
pub struct ObserveStats {
    pub frames: u64,
    pub failed_reads: u64,
    /// Reads slower than the target interval (frame rate below target).
    pub slow_reads: u64,
    pub stopped_reason: String,
}

/// Consecutive read failures before we declare the client gone. UIRoot
/// relocations recover after one `invalidate_root()` + re-search, so this
/// only trips on a real exit/crash.
const MAX_CONSECUTIVE_FAILURES: u32 = 8;

pub fn spawn_observer(
    stop: Arc<AtomicBool>,
    client: GameClient,
    interval: Duration,
    path: PathBuf,
    clock_rx: Receiver<SharedClock>,
    ready_tx: Sender<&'static str>,
) -> JoinHandle<ObserveStats> {
    std::thread::Builder::new()
        .name(format!("observe-{}", client.pid))
        .spawn(move || run(stop, client, interval, path, clock_rx, ready_tx))
        .expect("spawn observer")
}

/// One frame handed from the read stage to the parse stage.
struct FrameDelivery {
    t_ms: u64,
    read_ms: u64,
    tree: UiNode,
}

fn run(
    stop: Arc<AtomicBool>,
    client: GameClient,
    interval: Duration,
    path: PathBuf,
    clock_rx: Receiver<SharedClock>,
    ready_tx: Sender<&'static str>,
) -> ObserveStats {
    let mut stats =
        ObserveStats { frames: 0, failed_reads: 0, slow_reads: 0, stopped_reason: String::new() };
    let mut writer = match JsonlWriter::create(&path) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(pid = client.pid, error = %e, "cannot create observation file");
            stats.stopped_reason = format!("open_error: {e}");
            let _ = ready_tx.send("observe-failed");
            return stats;
        }
    };
    let mut reader = match UiReader::live(client.pid) {
        Ok(r) => r,
        Err(e) => {
            stats.stopped_reason = format!("open_error: {e}");
            let _ = ready_tx.send("observe-failed");
            return stats;
        }
    };
    // Warmup: locate the UIRoot now (the expensive scan) so the first
    // post-gate frame is a fast hot read.
    tracing::info!(pid = client.pid, "observing warmup: locating UIRoot (cold scan)…");
    match reader.find_ui_root() {
        Ok(_) => tracing::info!(pid = client.pid, "UIRoot located during warmup"),
        Err(e) => {
            stats.stopped_reason = format!("warmup_error: {e}");
            let _ = ready_tx.send("observe-failed");
            return stats;
        }
    }
    let _ = ready_tx.send("observe");
    let clock = match clock_rx.recv() {
        Ok(c) => c,
        Err(_) => {
            stats.stopped_reason = "gate_closed".into();
            return stats;
        }
    };

    // ---- Stage 1: paced full-tree reads (owns the reader). ----
    let (tx, rx) = mpsc::sync_channel::<FrameDelivery>(2);
    let pid = client.pid;
    let window = client.window;
    let read_clock = Arc::clone(&clock);
    let read_handle: JoinHandle<(u64, String)> = std::thread::Builder::new()
        .name(format!("observe-read-{}", client.pid))
        .spawn(move || {
            let mut failures: u32 = 0;
            let mut failed_reads: u64 = 0;
            let reason: String;
            loop {
                if stop.load(Ordering::Relaxed) {
                    reason = "stopped".into();
                    break;
                }
                let frame_start = Instant::now();
                let t_ms = read_clock.now_rel_ms();
                match reader.read_tree() {
                    Ok(tree) => {
                        failures = 0;
                        let read_ms = frame_start.elapsed().as_millis() as u64;
                        if tx
                            .send(FrameDelivery { t_ms, read_ms, tree })
                            .is_err()
                        {
                            // Parser stage went away.
                            reason = "parser_gone".into();
                            break;
                        }
                    }
                    Err(e) => {
                        failed_reads += 1;
                        failures += 1;
                        tracing::warn!(pid, error = %e, "tree read failed");
                        // The UIRoot moves on scene changes; re-search next round.
                        reader.invalidate_root();
                        let window_gone = window.is_some_and(|w| !w.is_valid());
                        if failures >= MAX_CONSECUTIVE_FAILURES || window_gone {
                            reason =
                                if window_gone { "process_exit" } else { "read_failures" }.into();
                            break;
                        }
                    }
                }
                let elapsed = frame_start.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
            }
            (failed_reads, reason)
        })
        .expect("spawn observe read stage");

    // ---- Stage 2: parse + delta + write (this thread, owns the writer). ----
    let mut differ = SnapshotDiffer::new();
    let mut warned_slow = false;
    let mut read_slow = 0u64;
    for delivery in rx.into_iter() {
        let FrameDelivery { t_ms, read_ms, tree } = delivery;
        let parse_start = Instant::now();
        let (snap, timing) = eve_semantics::parse_ui_tree_timed(&tree, client.flavor);
        let parse_ms = parse_start.elapsed().as_millis() as u64;
        if read_ms > interval.as_millis() as u64 {
            read_slow += 1;
            if !warned_slow {
                tracing::warn!(
                    pid,
                    read_ms,
                    interval_ms = interval.as_millis() as u64,
                    "observation read slower than target interval; frame rate will lag"
                );
                warned_slow = true;
            }
        }
        let mut line = differ.next_line(&snap);
        let obj = line.as_object_mut().expect("differ lines are objects");
        obj.insert("t_ms".into(), json!(t_ms));
        obj.insert("read_ms".into(), json!(read_ms));
        obj.insert("parse_ms".into(), json!(parse_ms));
        obj.insert(
            "parse_detail".into(),
            json!({
                "regioned_us": timing.regioned_us,
                "interaction_us": timing.interaction_us,
                "extractors_us": timing.extractors_us,
            }),
        );
        if writer.write(&line).is_ok() {
            stats.frames += 1;
        }
    }

    let (failed_reads, reason) = read_handle.join().unwrap_or((0, "read_thread_panicked".into()));
    stats.failed_reads = failed_reads;
    stats.slow_reads = read_slow;
    stats.stopped_reason = reason;
    let stopped = json!({
        "t_ms": clock.now_rel_ms(),
        "kind": "observer_stopped",
        "reason": stats.stopped_reason,
    });
    let _ = writer.write(&stopped);
    let _ = writer.finish();
    if stats.slow_reads > 0 {
        tracing::warn!(
            pid,
            slow = stats.slow_reads,
            frames = stats.frames,
            "observation had reads slower than the target interval"
        );
    }
    stats
}
