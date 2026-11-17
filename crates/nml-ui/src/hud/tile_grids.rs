//! Tile grids of the image sheets by file name (quirk O-280).
//!
//! The original keeps all image sheets of the game UI in one cache (`0x0066FF24`): `FUN_00445F40(name, tw, th)`
//! first searches with `FUN_00445C20` by **name** (`_mbscmp`, case matters) and returns a hit unchanged. Only a new
//! entry takes over name and tile size (`FUN_00402DF0` writes `+0x14`/`+0x18` and always succeeds, even if the file
//! is missing; loading happens only when drawing). Drawing uses the tile size of the entry (`FUN_004020E0` reads
//! `+0x14`/`+0x18`). Whoever loads a file first thus fixes its grid for everyone.
//!
//! The game UI is built in `0x004491F0`: status bar (`0x00452720`, call `0x004492BC`), minimap, then the command bar
//! (`0x00456630`, call `0x00449342`). The status bar loads `data/icons/resourcen_icons.tga` when reading `ICONFILE`
//! with `ICONSIZE 32 32` (`0x0045229A`); the 24 x 24 of the bar (`0x00456A5D`) are lost that way. Without a status
//! bar (`IsStatusBar 0`) the grid of the bar applies.
//!
//! The reimplementation only models the grid: the textures themselves are still managed by [`crate::assets::UiAssets`]
//! by path.

use std::collections::BTreeMap;

/// Grid per file name in load order: the first entry wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileGrids {
    grids: BTreeMap<String, [i32; 2]>,
}

impl TileGrids {
    /// Empty cache (before the game UI is built).
    pub fn new() -> Self {
        Self::default()
    }

    /// `FUN_00445F40(name, tw, th)`: if `name` is already loaded, the grid of the first load; otherwise `tile` is
    /// registered and returned. The name is compared exactly (`_mbscmp`).
    pub fn register(&mut self, name: &str, tile: [i32; 2]) -> [i32; 2] {
        *self.grids.entry(name.to_string()).or_insert(tile)
    }

    /// Grid of `name`, if already loaded.
    pub fn get(&self, name: &str) -> Option<[i32; 2]> {
        self.grids.get(name).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_loader_fixes_the_grid_by_exact_name() {
        let mut g = TileGrids::new();
        assert_eq!(g.get("data/icons/resourcen_icons.tga"), None);
        assert_eq!(
            g.register("data/icons/resourcen_icons.tga", [32, 32]),
            [32, 32]
        );
        assert_eq!(
            g.register("data/icons/resourcen_icons.tga", [24, 24]),
            [32, 32]
        );
        assert_eq!(g.get("data/icons/resourcen_icons.tga"), Some([32, 32]));
        // `_mbscmp` distinguishes case: a different entry.
        assert_eq!(
            g.register("data/icons/Resourcen_icons.tga", [24, 24]),
            [24, 24]
        );
    }
}
