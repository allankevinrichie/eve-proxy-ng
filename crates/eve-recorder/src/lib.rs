//! eve-recorder: synchronized session recording for EVE Online clients.
//!
//! Three timestamp-aligned streams per session (see `record_session`):
//! - **observation** — semantic [`UiSnapshot`] polling with frame-to-frame
//!   section deltas (`clients/<pid>.jsonl`);
//! - **input** — human keyboard/mouse events from global LL hooks, mouse
//!   moves coalesced to ~60 fps (`input.jsonl`); injected (synthesized)
//!   input is recorded too, flagged per event;
//! - **video** — per-client window capture at a constant frame rate,
//!   encoded to HEVC by an ffmpeg subprocess (`clients/<pid>.mp4`).
//!
//! All streams share one [`SessionClock`]; the manifest records anchors
//! (video first-frame time, epoch start) so events, observations and
//! video frames map onto the same timeline.

pub mod capture;
pub mod clock;
pub mod delta;
pub mod error;
pub mod input;
pub mod jsonl;
pub mod observe;
pub mod session;
pub mod video_encoder;
pub mod watermark;

pub use capture::{VideoSize, VideoStats};
pub use clock::{SessionClock, SharedClock};
pub use error::{RecorderError, Result};
pub use input::{InputStats, InputRecorder};
pub use observe::ObserveStats;
pub use session::{ClientSummary, RecorderConfig, SessionSummary, record_session};
