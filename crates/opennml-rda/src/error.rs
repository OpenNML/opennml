use std::path::PathBuf;

/// Error while reading RDA archives.
#[derive(Debug, thiserror::Error)]
pub enum RdaError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}: not an RDA archive (header \"Resource File V1.0\" missing)")]
    InvalidHeader(PathBuf),
    #[error("{path}: directory with {count} entries does not fit in the file")]
    Truncated { path: PathBuf, count: u32 },
    #[error("{archive}: entry {entry} lies outside the file")]
    EntryOutOfBounds { archive: PathBuf, entry: String },
    #[error("{entry}: unpacked {actual} bytes, expected {expected}")]
    SizeMismatch {
        entry: String,
        expected: u32,
        actual: usize,
    },
    #[error("{entry}: zlib error: {source}")]
    Decompress {
        entry: String,
        #[source]
        source: std::io::Error,
    },
    #[error("file not found: {0}")]
    NotFound(String),
    #[error(
        "no RDA archives found in {0} (expected NML<n>.RDA without gaps, starting at NML0.RDA)"
    )]
    NoArchives(PathBuf),
    #[error("{}", crate::gamedir::not_found_message(.tried, *.explicit))]
    GameDirNotFound {
        tried: Vec<crate::gamedir::Tried>,
        explicit: bool,
    },
}
