//! Helpers for the tests of the crate: synthetic TGA files and a test VFS (pattern like
//! `opennml-world/src/assets.rs`). The text layout, drawing and atlas generation tests use them as well.

use std::cell::RefCell;
use std::ops::Deref;

use opennml_rda::Vfs;
use opennml_rda::testutil::{TestFile, build_rda};

use crate::saves::{LocalStamp, SaveDir, SaveEntry, SaveFiles};

/// TGA type 2, 32 bit, **rows stored from bottom to top** (origin bit 0): this is how `decode_tga_game` reads the
/// file as an image with the first pixel at the top left, when `pixel(x, y)` returns the color `[r, g, b, a]` of the
/// pixel (x, y) from the top left. (`decode_tga_game` ignores the origin bit: file row `r` becomes image row
/// `H - 1 - r`, `docs/formats/tga.md`.)
pub(crate) fn tga_bottom_up(
    width: u16,
    height: u16,
    pixel: impl Fn(u32, u32) -> [u8; 4],
) -> Vec<u8> {
    let mut tga = vec![0u8; 18];
    tga[2] = 2;
    tga[12..14].copy_from_slice(&width.to_le_bytes());
    tga[14..16].copy_from_slice(&height.to_le_bytes());
    tga[16] = 32;
    for file_row in 0..u32::from(height) {
        let y = u32::from(height) - 1 - file_row;
        for x in 0..u32::from(width) {
            let [r, g, b, a] = pixel(x, y);
            tga.extend_from_slice(&[b, g, r, a]);
        }
    }
    tga
}

/// Test VFS: an archive `NML0.RDA` in a throwaway directory that lives as long as the VFS.
pub(crate) struct TestVfs {
    vfs: Vfs,
    _dir: tempfile::TempDir,
}

impl Deref for TestVfs {
    type Target = Vfs;
    fn deref(&self) -> &Vfs {
        &self.vfs
    }
}

/// Builds a [`TestVfs`] from path/content pairs.
pub(crate) fn test_vfs(files: &[(&str, Vec<u8>)]) -> TestVfs {
    let dir = tempfile::tempdir().unwrap();
    let entries: Vec<TestFile> = files
        .iter()
        .map(|(path, data)| TestFile {
            path,
            data: data.as_slice(),
            compress: true,
        })
        .collect();
    std::fs::write(dir.path().join("NML0.RDA"), build_rda(&entries)).unwrap();
    let vfs = Vfs::open_game_dir(dir.path()).unwrap();
    TestVfs { vfs, _dir: dir }
}

/// `.fnt` text in the game's format: for each code 0...255 a line `code width height u0 u1 v0 v1` in the 16 x 16 grid;
/// `width_of(c)` and `height_of(c)` give width and height.
pub(crate) fn fnt_text(width_of: impl Fn(u32) -> i32, height_of: impl Fn(u32) -> i32) -> String {
    let mut out = String::from("FONTFILE V1.0\r\n\r\n[CHARINFOS]\r\n");
    for c in 0..256u32 {
        let (u, v) = ((c % 16) as f32 / 16.0, (c / 16) as f32 / 16.0);
        out.push_str(&format!(
            "{c} {} {} {:.6} {:.6} {:.6} {:.6}\r\n",
            width_of(c),
            height_of(c),
            u,
            u + 1.0 / 16.0,
            v,
            v + 1.0 / 16.0
        ));
    }
    out
}

/// Savegames for tests ([`SaveFiles`]): fixed entries per profile GUID in list order (newest first, as the host
/// delivers them) and a fixed clock. The entries can be changed (`remove`) to simulate the host's deleting.
pub(crate) struct FakeSaveFiles {
    pub(crate) entries: RefCell<Vec<(i32, SaveEntry)>>,
    /// The files of the network folder (without GUID, newest first).
    pub(crate) net: RefCell<Vec<SaveEntry>>,
    /// The files of the recording folder, in the order the host delivers them (sorted by name).
    pub(crate) autoplay: RefCell<Vec<SaveEntry>>,
    pub(crate) now: LocalStamp,
}

impl FakeSaveFiles {
    /// Entries `(guid, name, modification time)` and the clock `now`.
    pub(crate) fn new(entries: &[(i32, &str, LocalStamp)], now: LocalStamp) -> FakeSaveFiles {
        FakeSaveFiles {
            entries: RefCell::new(
                entries
                    .iter()
                    .map(|&(guid, name, modified)| {
                        (
                            guid,
                            SaveEntry {
                                name: name.to_string(),
                                modified,
                            },
                        )
                    })
                    .collect(),
            ),
            net: RefCell::new(Vec::new()),
            autoplay: RefCell::new(Vec::new()),
            now,
        }
    }

    /// Plus the recordings `(file name, modification time)` in list order.
    pub(crate) fn with_autoplay(self, entries: &[(&str, LocalStamp)]) -> FakeSaveFiles {
        *self.autoplay.borrow_mut() = entries
            .iter()
            .map(|&(name, modified)| SaveEntry {
                name: name.to_string(),
                modified,
            })
            .collect();
        self
    }

    /// Plus the network savegames `(name, modification time)` in list order.
    pub(crate) fn with_net(self, entries: &[(&str, LocalStamp)]) -> FakeSaveFiles {
        *self.net.borrow_mut() = entries
            .iter()
            .map(|&(name, modified)| SaveEntry {
                name: name.to_string(),
                modified,
            })
            .collect();
        self
    }

    /// Remove the savegame `name` of the profile `guid` (like `DeleteFileA` in the host).
    pub(crate) fn remove(&self, guid: i32, name: &str) {
        self.entries
            .borrow_mut()
            .retain(|(g, e)| !(*g == guid && e.name == name));
    }
}

impl SaveFiles for FakeSaveFiles {
    fn list(&self, dir: SaveDir, guid: i32) -> Vec<SaveEntry> {
        if dir == SaveDir::Multiplayer {
            return self.net.borrow().clone();
        }
        if dir == SaveDir::Autoplay {
            return self.autoplay.borrow().clone();
        }
        self.entries
            .borrow()
            .iter()
            .filter(|(g, _)| *g == guid)
            .map(|(_, e)| e.clone())
            .collect()
    }

    fn exists(&self, dir: SaveDir, guid: i32, name: &str) -> bool {
        if dir == SaveDir::Multiplayer {
            return self.net.borrow().iter().any(|e| e.name == name);
        }
        if dir == SaveDir::Autoplay {
            return self.autoplay.borrow().iter().any(|e| e.name == name);
        }
        self.entries
            .borrow()
            .iter()
            .any(|(g, e)| *g == guid && e.name == name)
    }

    fn now_local(&self) -> LocalStamp {
        self.now
    }
}

/// Local time `y-m-d h:min`.
pub(crate) fn stamp(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> LocalStamp {
    LocalStamp {
        year,
        month,
        day,
        hour,
        minute,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_up_tga_reads_back_with_the_first_pixel_at_the_top_left() {
        let tga = tga_bottom_up(2, 2, |x, y| [x as u8 * 100, y as u8 * 100, 7, 255]);
        let img = opennml_formats::tga::decode_tga_game(&tga).unwrap();
        assert_eq!(img.get_pixel(1, 0).0, [100, 0, 7, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 100, 7, 255]);
    }

    #[test]
    fn test_vfs_serves_its_files() {
        let vfs = test_vfs(&[("data/a.txt", b"hello".to_vec())]);
        assert_eq!(vfs.read(r"DATA\A.TXT").unwrap(), b"hello");
    }
}
