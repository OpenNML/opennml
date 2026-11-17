//! Menu level behind the menu, UI side (re/menu-level ML.4, ML.8): which state the host reports to the UI. With it the
//! UI draws `levelbackground.bkg` (`FUN_0045CAF0`); the host itself runs the world.

use super::MenuApp;

/// State of the world behind the menu level (`GameApp+0xD5E0`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelWorld {
    /// An engine is present. The host reports the state only for the menu level; during a match it stays at the
    /// value from the end of the menu level (no consequences, ML.16).
    pub present: bool,
    /// Bit `0x1`: the world is drawn (`FUN_004BA680() == 0`).
    pub shown: bool,
}

impl MenuApp {
    /// The host reports the state of the world; from the next frame on, the desktop draws `levelbackground.bkg`
    /// only with `shown` and without video.
    pub fn set_level_world(&mut self, world: LevelWorld) {
        self.level_world = world;
    }

    pub fn level_world(&self) -> LevelWorld {
        self.level_world
    }
}
