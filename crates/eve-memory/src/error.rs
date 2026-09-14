//! Error type shared across the perception layer.

/// Errors produced by the perception layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no EVE Online client process found")]
    NoClientProcess,

    #[error("process {pid} not found or already exited")]
    ProcessNotFound { pid: u32 },

    #[error("failed to open process {pid}: {message}")]
    OpenProcessFailed { pid: u32, message: String },

    #[error("failed to enumerate memory regions of process {pid}: {message}")]
    RegionEnumerationFailed { pid: u32, message: String },

    #[error("memory read failed at {address:#x}: {message}")]
    ReadFailed { address: u64, message: String },

    #[error("short read at {address:#x}: requested {requested} bytes, got {received}")]
    ShortRead {
        address: u64,
        requested: usize,
        received: usize,
    },

    #[error("invalid python object at {address:#x}: {message}")]
    InvalidPyObject { address: u64, message: String },

    #[error("UI root discovery failed: {message}")]
    UiRootDiscoveryFailed { message: String },

    #[error("window not found for process {pid}")]
    WindowNotFound { pid: u32 },

    #[error("window operation failed: {message}")]
    WindowOperationFailed { message: String },

    #[error("screen capture failed: {message}")]
    CaptureFailed { message: String },

    #[error("sample archive is malformed: {message}")]
    MalformedSample { message: String },

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub(crate) fn read_failed(address: u64, message: impl Into<String>) -> Self {
        Error::ReadFailed {
            address,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
