/// Errors shared by the core and the drivers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Talking to the device failed, e.g. a USB write error.
    #[error("transport error: {0}")]
    Transport(String),
    /// The device is gone (unplugged or reset).
    #[error("device disconnected")]
    Disconnected,
    /// An image could not be decoded, scaled or encoded.
    #[error("image error: {0}")]
    Image(String),
    /// The display does not offer this operation.
    #[error("{0} is not supported by this display")]
    Unsupported(&'static str),
    /// A caller passed a value the operation cannot use.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The display was closed and accepts no more work.
    #[error("display is closed")]
    Closed,
}

impl Error {
    /// Whether the device can no longer be used and has to be opened again.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::Disconnected)
    }
}

/// Result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
