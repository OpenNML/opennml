//! The original's pause with two bits (re/game-ui D5.5): `Engine+0x24E4` stops the simulation ([`PauseState::sim`]: no
//! world tick, no AI, no level scripts, no message bar), `Engine+0x24E5` is the **visible** pause
//! ([`PauseState::shown`]: PAUSE text, build buttons without effect O-231, no focus pause O-154).
//!
//! In single player the pause is purely local: no command, no recording, not in the savegame (loading starts without
//! pause). The network echo `0x10000015`, which sets only the visible bit on the receiver,
//! belongs to network play (O-316).

/// The engine's two pause bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseState {
    /// Simulation pause `Engine+0x24E4`.
    pub sim: bool,
    /// Visible pause `Engine+0x24E5`.
    pub shown: bool,
}

/// Game type of the menu level (`Engine+0x108`); neither pause nor `togglepause` work there.
const MENU_LEVEL: i32 = -2;

impl PauseState {
    /// `togglepause` (`0x0042D780`, O-316): only with game type > -2 **and** equal bits; then flips both.
    /// `true` if something changed.
    pub fn toggle(&mut self, game_type: i32) -> bool {
        if game_type <= MENU_LEVEL || self.sim != self.shown {
            return false;
        }
        let on = !self.sim;
        self.sim = on;
        self.shown = on;
        true
    }

    /// `GameEngine_Pause` (`0x0042D5E0`): both bits on, only with game type > -2.
    pub fn pause(&mut self, game_type: i32) {
        if game_type > MENU_LEVEL {
            self.sim = true;
            self.shown = true;
        }
    }

    /// `GameEngine_Resume` (`0x0042D600`): both bits off, unconditionally.
    pub fn resume(&mut self) {
        self.sim = false;
        self.shown = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O-316: `togglepause` flips both bits only when they are equal and not in the menu level; a network echo (only
    /// the visible bit) blocks it.
    #[test]
    fn toggle_pause_needs_equal_bits() {
        let mut p = PauseState::default();
        assert!(p.toggle(-1), "campaign: on");
        assert_eq!(
            p,
            PauseState {
                sim: true,
                shown: true
            }
        );
        assert!(p.toggle(0), "multiplayer map: off again");
        assert_eq!(p, PauseState::default());
        // Menu level: nothing.
        assert!(!p.toggle(-2));
        assert!(!p.toggle(-3));
        assert_eq!(p, PauseState::default());
        // Only the visible bit (network echo `0x0042F480`): `togglepause` blocks in both directions.
        let mut echo = PauseState {
            sim: false,
            shown: true,
        };
        assert!(!echo.toggle(-1));
        assert_eq!(
            echo,
            PauseState {
                sim: false,
                shown: true
            }
        );
        let mut half = PauseState {
            sim: true,
            shown: false,
        };
        assert!(!half.toggle(-1));
        // `GameEngine_Pause` sets both (not in the menu level), `GameEngine_Resume` clears both unconditionally.
        let mut q = PauseState::default();
        q.pause(-2);
        assert_eq!(q, PauseState::default());
        q.pause(-1);
        assert_eq!(
            q,
            PauseState {
                sim: true,
                shown: true
            }
        );
        echo.resume();
        assert_eq!(echo, PauseState::default());
    }
}
