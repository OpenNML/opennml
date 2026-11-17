//! Fog of war (re/vision): the grids "explored", "seen" and "cleared" with one bit per player per tile, the cycle
//! `GameWorld_UpdateFogOfWarStep` (`0x00486CD0`), the vision sources (`GameWorld+0x4A8`) and the queries.
//! The simulation needs the fog only in the hit filter (`MM_CHECKFOG`) and in the build command; the host reads
//! everything else.

pub mod cycle;
pub mod grid;
pub mod memory;
pub mod query;
pub mod sources;

use serde::{Deserialize, Serialize};

pub use cycle::fog_step;
pub use grid::{ByteGrid, MASK_SIDE, circle_bit, stamp, stamp_side};
pub use memory::{MemorySnapshot, memory_matches};
pub use query::FogView;
pub use sources::{VisionSource, set_vision_source, tick_vision_sources};

use crate::state::SimState;

/// Stamp kind "explored" (flag 1 of the vision sources; the stamp is 2 tiles larger per side).
pub const EXPLORE: u8 = 1;
/// Stamp kind "seen" (flag 2).
pub const SEE: u8 = 2;
/// Stamp kind "cleared" (flag 4).
pub const CLEAR_UP: u8 = 4;
/// Category bit `CANCLEARUP` (`MID_PROP_GLOBAL_CANCLEARUP` `0x10FF`, `0x0046CA20`).
pub const CANCLEARUP: u32 = 0x1000;

/// Named view rectangle of the scripts (`m_AreaView`, re/vision 3.4): rectangle of the area `name` in tiles
/// (`[x0, z0, x1, z1]`, as the area stood when created), slot 0...7 and whether it also stamps "cleared". Areas
/// can extend beyond the map: no rectangle test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaView {
    pub name: String,
    pub rect: [i32; 4],
    pub player: u8,
    pub clear: bool,
}

/// Fog state (`GameWorld+0x1FD8...+0x205C`, sources `+0x4A8`). Unlike the original, all grids, the cycle and the
/// sources belong to the savegame so that loading in the middle of the cycle continues bit-identically
/// (re/vision 12.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fog {
    /// `m_FOGDiscovered` (`+0x1FD8`): only ORed.
    pub explored: ByteGrid,
    /// Seen, foreground (`+0x1FE8`, reader).
    pub seen: ByteGrid,
    /// Cleared, foreground (`+0x1FF8`).
    pub cleared: ByteGrid,
    /// Seen, background (`+0x2008`, written in the cycle).
    pub seen_back: ByteGrid,
    /// Cleared, background (`+0x2018`).
    pub cleared_back: ByteGrid,
    /// `m_FogHandlesProcessed` (`+0x2050`): 0 new cycle, > 0 running, -1 idle tick, -2 alpha.
    pub processed: i32,
    /// `m_FogCurPlayerID` (`+0x2054`): 0...8.
    pub player: u8,
    /// `m_FogHandlesPerTick` (`+0x2058`).
    pub per_tick: i32,
    /// `m_FogHandleEnumPos` (`+0x205C`): index in the slot table of the player.
    pub enum_pos: u32,
    /// Vision sources in insertion order.
    pub sources: Vec<VisionSource>,
    /// Named view rectangles (`m_AreaView`) in insertion order.
    pub area_views: Vec<AreaView>,
    /// `m_FogTempRunningHandlesPosition`: cursor in `SimState::inactive_lists[player]` when the cycle runs
    /// through a hidden slot (index instead of node): index of the next entry, `== L.len()` is the end of the
    /// list. Cycle start, player change and cycle end set 0 ([`crate::world::hide`]).
    pub inactive_pos: u32,
}

impl Fog {
    /// `GameWorld_Init` (`0x0048CC93`...): grids empty, cycle 0.
    pub fn new(width: i32, height: i32) -> Self {
        let g = || ByteGrid::new(width, height);
        Self {
            explored: g(),
            seen: g(),
            cleared: g(),
            seen_back: g(),
            cleared_back: g(),
            ..Self::default()
        }
    }

    /// Stamp around the tile `(kx, kz)` with side `s` (re/vision 3.2/4.3): "explored" at
    /// `(kx − s/2 − 2, kz − s/2 − 2)` with side `s + 4`, "seen" and "cleared" (background) at
    /// `(kx − s/2, kz − s/2)` with side `s`, depending on `kinds`.
    pub fn stamp_sight(&mut self, (kx, kz): (i32, i32), s: i32, bit: u8, kinds: u8) {
        let (x0, z0) = (kx.wrapping_sub(s / 2), kz.wrapping_sub(s / 2));
        let big = s.saturating_add(4);
        if kinds & EXPLORE != 0 {
            stamp(
                &mut self.explored,
                x0.wrapping_sub(2),
                z0.wrapping_sub(2),
                big,
                big,
                bit,
            );
        }
        if kinds & SEE != 0 {
            stamp(&mut self.seen_back, x0, z0, s, s, bit);
        }
        if kinds & CLEAR_UP != 0 {
            stamp(&mut self.cleared_back, x0, z0, s, s, bit);
        }
    }

    /// Load check: every grid has sides >= 0 (`any_in_rect` and `stamp` compute with `side − 1` or
    /// `side − corner`) and as many cells as its sides (the accesses compute the index from the width and height
    /// of the grid). The cycle tolerates the other fields.
    pub fn check(&self) -> Result<(), String> {
        let grids = [
            ("explored", &self.explored),
            ("seen", &self.seen),
            ("cleared", &self.cleared),
            ("seen (background)", &self.seen_back),
            ("cleared (background)", &self.cleared_back),
        ];
        for (name, g) in grids {
            if g.width < 0 || g.height < 0 || g.cells.len() != g.width as usize * g.height as usize
            {
                return Err(format!(
                    "grid \"{name}\" with {} cells at {} x {}",
                    g.cells.len(),
                    g.width,
                    g.height
                ));
            }
        }
        Ok(())
    }
}

/// Reveals the map for `player` (-1 = all) - for tests only (test world
/// `command::testkit::state`, viewer tests): immediately in "explored" and "seen" and permanently via
/// a vision source of size 512 in the middle of the map (circle with a radius of 128 tiles).
#[doc(hidden)]
pub fn reveal_map(state: &mut SimState, player: i32) {
    let (w, h) = (state.fog.explored.width, state.fog.explored.height);
    set_vision_source(
        &mut state.fog,
        VisionSource {
            x: w / 2,
            z_or_hid: h / 2,
            size: 512,
            player,
            remaining_ms: -1,
            flags: EXPLORE | SEE,
        },
    );
    let bits = if player < 0 {
        0xFF
    } else {
        1u8 << player.clamp(0, 7)
    };
    let fog = &mut state.fog;
    for c in fog
        .explored
        .cells
        .iter_mut()
        .chain(fog.seen.cells.iter_mut())
    {
        *c |= bits;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::rules::Rules;
    use crate::setup::{MatchSetup, new_match};
    use crate::sim::Sim;

    fn setup(map_explored: bool) -> MatchSetup {
        MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored,
            ki: crate::ki::KiSetup::default(),
            script: crate::script::ScriptSetup::default(),
            ..MatchSetup::default()
        }
    }

    #[test]
    fn fog_grids_come_with_the_map_and_map_explored_fills_explored() {
        let rules = Rules::synthetic("", 16);
        let state = new_match(&rules, &setup(false)).unwrap();
        let f = &state.fog;
        assert!(f.check().is_ok());
        assert_eq!((f.explored.width, f.explored.height), (16, 16));
        assert_eq!(
            (f.explored.count(0xFF), f.seen.count(0xFF), f.processed),
            (0, 0, 0)
        );
        let state = new_match(&rules, &setup(true)).unwrap();
        assert_eq!(state.fog.explored.count(0x80), 256);
        assert_eq!(state.fog.seen.count(0xFF), 0);
    }

    #[test]
    fn clear_up_sight_comes_from_the_category_and_the_walk_flag() {
        // Colonist with walk flag `UNITCLEARINGUP` (0x80000 added to 0x40900), house with `CANCLEARUP`.
        let types = kit::TYPES
            .replace("VALUEID 10003 6 1 264448", "VALUEID 10003 6 1 788736")
            .replace(
                "NAME SPA_Wohnhaus\nVALUEID 100 6 1 0",
                "NAME SPA_Wohnhaus\nVALUEID 100 6 1 4096",
            );
        let rules = kit::rules_from(&types);
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let colonist = kit::spawn(&mut sim, kit::COLONIST, 0, (5, 5));
        let house = kit::spawn(&mut sim, kit::HOUSE, 0, (10, 10));
        let store = kit::spawn(&mut sim, kit::STORE, 0, (20, 10));
        let flag = |id| sim.object(id).unwrap().clear_up_sight;
        assert_eq!(
            [flag(colonist), flag(house), flag(store)],
            [true, true, false]
        );
        let plain = kit::rules();
        let mut state = kit::state(&plain);
        let mut sim = Sim::new(&mut state, &plain);
        let colonist = kit::spawn(&mut sim, kit::COLONIST, 0, (5, 5));
        assert!(!sim.object(colonist).unwrap().clear_up_sight);
    }

    #[test]
    fn stamp_sight_stamps_the_kinds_around_the_tile() {
        let mut f = Fog::new(200, 200);
        f.stamp_sight((100, 100), 17, 1 << 3, EXPLORE | SEE);
        assert_eq!(
            (
                f.explored.count(8),
                f.seen_back.count(8),
                f.cleared_back.count(8)
            ),
            (345, 224, 0)
        );
        f.stamp_sight((100, 100), 17, 1, CLEAR_UP);
        assert_eq!((f.cleared_back.count(1), f.seen.count(0xFF)), (224, 0));
        // Extreme values: no panic.
        f.stamp_sight((i32::MAX, i32::MIN), i32::MAX, 1, 7);
    }
}
