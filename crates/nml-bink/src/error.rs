//! Errors of the Bink decoder.

/// Error while reading and decoding a Bink file.
#[derive(Debug, thiserror::Error)]
pub enum BinkError {
    /// Reading or seeking in the file failed.
    #[error("cannot read Bink file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is corrupted or not a Bink file.
    #[error("invalid Bink file: {0}")]
    Format(String),
    /// The file is valid, but not supported by the decoder (e.g. a different revision).
    #[error("Bink file not supported: {0}")]
    Unsupported(String),
}

/// Result with [`BinkError`].
pub type Result<T> = std::result::Result<T, BinkError>;
