/// Error while parsing a game data file.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("line {line}: {message}")]
    Syntax { line: usize, message: String },
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("image could not be decoded: {0}")]
    Image(#[from] image::ImageError),
    #[error("file ends prematurely at offset {offset:#x} ({what})")]
    Truncated { offset: usize, what: String },
    #[error("{remaining} bytes of unexpected data from offset {offset:#x}")]
    TrailingData { offset: usize, remaining: usize },
}

pub(crate) fn syntax(line: usize, message: &str) -> FormatError {
    FormatError::Syntax {
        line,
        message: message.to_string(),
    }
}
