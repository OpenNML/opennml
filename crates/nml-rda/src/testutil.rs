//! Test helpers: synthetic RDA archives and the location of the original installation.

use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::Vfs;
use crate::archive::{ENTRY_LEN, FLAG_ZLIB, HEADER_LEN, MAGIC, NAME_LEN};

/// A file for [`build_rda`].
pub struct TestFile<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
    pub compress: bool,
}

/// Builds the bytes of a valid "Resource File V1.0" archive.
pub fn build_rda(files: &[TestFile<'_>]) -> Vec<u8> {
    let mut out = vec![b' '; HEADER_LEN];
    out[..MAGIC.len()].copy_from_slice(MAGIC);
    out[MAGIC.len()] = 0;
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());

    let mut data_offset = HEADER_LEN + 4 + files.len() * ENTRY_LEN;
    let mut blobs = Vec::new();
    for file in files {
        let stored = if file.compress {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(file.data).unwrap();
            enc.finish().unwrap()
        } else {
            file.data.to_vec()
        };
        let (name, _, unmappable) = encoding_rs::WINDOWS_1252.encode(file.path);
        assert!(
            !unmappable && name.len() < NAME_LEN,
            "invalid test path {}",
            file.path
        );
        let mut field = vec![b' '; NAME_LEN];
        field[..name.len()].copy_from_slice(&name);
        field[name.len()] = 0;
        out.extend_from_slice(&field);
        let flags = if file.compress { FLAG_ZLIB } else { 0 };
        for value in [
            data_offset as u32,
            stored.len() as u32,
            file.data.len() as u32,
            flags,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        data_offset += stored.len();
        blobs.push(stored);
    }
    for blob in blobs {
        out.extend_from_slice(&blob);
    }
    out
}

/// Original installation for corpus tests: `NML_GAME_DIR` or `<repo>/../original/No Man's Land`.
pub fn corpus_game_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("NML_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../original/No Man's Land")
        });
    dir.is_dir().then_some(dir)
}

pub use crate::version::GameVersion;

/// Language edition of the original installation for corpus tests (see [`crate::detect_version`]).
pub fn game_version(vfs: &Vfs) -> Result<GameVersion, String> {
    crate::version::detect_version(vfs)
}

/// Creates an artificial installation: `NML0.RDA` through `NML{archives-1}.RDA`, each with one small
/// file; with `german`, `NML0.RDA` additionally contains the hero voice line by which the German
/// edition is recognized.
pub fn write_fake_install(dir: &Path, archives: usize, german: bool) {
    std::fs::create_dir_all(dir).unwrap();
    for i in 0..archives {
        let name = format!("data/fake{i}.txt");
        let mut files = vec![TestFile {
            path: &name,
            data: b"x",
            compress: false,
        }];
        if german && i == 0 {
            files.push(TestFile {
                path: "data/sprache/helden/wid_umak_select_06.wav",
                data: b"x",
                compress: false,
            });
        }
        std::fs::write(dir.join(format!("NML{i}.RDA")), build_rda(&files)).unwrap();
    }
}
