//! Recorder error type.

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("no running EVE clients found")]
    NoClients,
    #[error("client pid {0} not found among running clients")]
    UnknownPid(u32),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("memory read error: {0}")]
    Memory(#[from] eve_memory::Error),
    #[error("ffmpeg was not found on PATH — install ffmpeg or pass --no-video")]
    FfmpegMissing,
    #[error(
        "no usable HEVC encoder (tried hevc_nvenc, hevc_amf, hevc_qsv, libx265); \
         pass --encoder to select one explicitly"
    )]
    NoEncoder,
    #[error("a recording stream died during warmup before the session started")]
    WarmupFailed,
    #[error("video capture failed: {0}")]
    Capture(String),
}

pub type Result<T> = std::result::Result<T, RecorderError>;
