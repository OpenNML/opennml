//! Reading a single RDA archive in the "Resource File V1.0" format.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;

use crate::RdaError;

pub(crate) const MAGIC: &[u8] = b"Resource File V1.0";
pub(crate) const HEADER_LEN: usize = 256;
pub(crate) const NAME_LEN: usize = 256;
pub(crate) const ENTRY_LEN: usize = NAME_LEN + 16;
pub(crate) const FLAG_ZLIB: u32 = 1;

/// A directory entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path as stored in the archive (decoded from cp1252, `/` as separator).
    pub path: String,
    pub offset: u32,
    pub stored_size: u32,
    pub size: u32,
    pub flags: u32,
}

impl Entry {
    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_ZLIB != 0
    }
}

/// An opened RDA archive. Data is not read until [`Archive::read`] is called.
#[derive(Debug)]
pub struct Archive {
    path: PathBuf,
    entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RdaError> {
        let path = path.as_ref().to_path_buf();
        let io_err = |e: io::Error| RdaError::Io {
            path: path.clone(),
            source: e,
        };
        let mut file = File::open(&path).map_err(io_err)?;
        let file_len = file.metadata().map_err(io_err)?.len();

        let mut header = [0u8; HEADER_LEN + 4];
        if file_len < header.len() as u64 {
            return Err(RdaError::InvalidHeader(path.clone()));
        }
        file.read_exact(&mut header).map_err(io_err)?;
        if !header.starts_with(MAGIC) {
            return Err(RdaError::InvalidHeader(path.clone()));
        }
        let count = u32::from_le_bytes(header[HEADER_LEN..].try_into().unwrap());

        let dir_len = count as u64 * ENTRY_LEN as u64;
        if header.len() as u64 + dir_len > file_len {
            return Err(RdaError::Truncated {
                path: path.clone(),
                count,
            });
        }
        let mut dir = vec![0u8; dir_len as usize];
        file.read_exact(&mut dir).map_err(io_err)?;

        let mut entries = Vec::with_capacity(count as usize);
        for raw in dir.chunks_exact(ENTRY_LEN) {
            let entry = parse_entry(raw);
            if entry.offset as u64 + entry.stored_size as u64 > file_len {
                return Err(RdaError::EntryOutOfBounds {
                    archive: path.clone(),
                    entry: entry.path,
                });
            }
            entries.push(entry);
        }
        Ok(Self { path, entries })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, RdaError> {
        let io_err = |e: io::Error| RdaError::Io {
            path: self.path.clone(),
            source: e,
        };
        let mut file = File::open(&self.path).map_err(io_err)?;
        file.seek(SeekFrom::Start(entry.offset as u64))
            .map_err(io_err)?;
        let mut stored = vec![0u8; entry.stored_size as usize];
        file.read_exact(&mut stored).map_err(io_err)?;

        let data = if entry.is_compressed() {
            let mut out = Vec::with_capacity(entry.size as usize);
            ZlibDecoder::new(&stored[..])
                .read_to_end(&mut out)
                .map_err(|e| RdaError::Decompress {
                    entry: entry.path.clone(),
                    source: e,
                })?;
            out
        } else {
            stored
        };
        if data.len() != entry.size as usize {
            return Err(RdaError::SizeMismatch {
                entry: entry.path.clone(),
                expected: entry.size,
                actual: data.len(),
            });
        }
        Ok(data)
    }
}

/// Splits a 272-byte directory entry into its fields.
fn parse_entry(raw: &[u8]) -> Entry {
    let name = &raw[..NAME_LEN];
    let end = name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    let (path, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&name[..end]);
    let field = |i: usize| {
        let at = NAME_LEN + i * 4;
        u32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
    };
    Entry {
        path: path.into_owned(),
        offset: field(0),
        stored_size: field(1),
        size: field(2),
        flags: field(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TestFile, build_rda};

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NML0.RDA");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn sample() -> Vec<u8> {
        build_rda(&[
            TestFile {
                path: "data/config/gebäude.cfg",
                data: b"hello hello hello",
                compress: true,
            },
            TestFile {
                path: "data/menü/a.dlg",
                data: b"FRAME",
                compress: false,
            },
        ])
    }

    #[test]
    fn lists_entries_with_cp1252_paths() {
        let (_dir, path) = write_temp(&sample());
        let archive = Archive::open(&path).unwrap();
        let paths: Vec<_> = archive.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["data/config/gebäude.cfg", "data/menü/a.dlg"]);
        assert!(archive.entries()[0].is_compressed());
        assert!(!archive.entries()[1].is_compressed());
        assert_eq!(archive.entries()[0].size, 17);
    }

    #[test]
    fn reads_compressed_and_stored_data() {
        let (_dir, path) = write_temp(&sample());
        let archive = Archive::open(&path).unwrap();
        assert_eq!(
            archive.read(&archive.entries()[0]).unwrap(),
            b"hello hello hello"
        );
        assert_eq!(archive.read(&archive.entries()[1]).unwrap(), b"FRAME");
    }

    #[test]
    fn rejects_file_without_magic() {
        let (_dir, path) = write_temp(&[b'x'; 300]);
        assert!(matches!(
            Archive::open(&path),
            Err(RdaError::InvalidHeader(_))
        ));
    }

    #[test]
    fn rejects_file_shorter_than_header() {
        let (_dir, path) = write_temp(b"Resource File V1.0");
        assert!(matches!(
            Archive::open(&path),
            Err(RdaError::InvalidHeader(_))
        ));
    }

    #[test]
    fn rejects_directory_larger_than_file() {
        let mut bytes = build_rda(&[]);
        bytes[256..260].copy_from_slice(&1000u32.to_le_bytes());
        let (_dir, path) = write_temp(&bytes);
        assert!(matches!(
            Archive::open(&path),
            Err(RdaError::Truncated { count: 1000, .. })
        ));
    }

    #[test]
    fn rejects_entry_pointing_past_end() {
        let mut bytes = sample();
        let offset_field = 260 + NAME_LEN;
        bytes[offset_field..offset_field + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let (_dir, path) = write_temp(&bytes);
        assert!(matches!(
            Archive::open(&path),
            Err(RdaError::EntryOutOfBounds { .. })
        ));
    }

    #[test]
    fn detects_size_mismatch_after_unpacking() {
        let mut bytes = sample();
        let size_field = 260 + NAME_LEN + 8;
        bytes[size_field..size_field + 4].copy_from_slice(&999u32.to_le_bytes());
        let (_dir, path) = write_temp(&bytes);
        let archive = Archive::open(&path).unwrap();
        let err = archive.read(&archive.entries()[0]).unwrap_err();
        assert!(matches!(
            err,
            RdaError::SizeMismatch {
                expected: 999,
                actual: 17,
                ..
            }
        ));
    }
}
