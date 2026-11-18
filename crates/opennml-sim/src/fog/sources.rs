//! Vision sources (`GameWorld+0x4A8`, re/vision 4): revealed circles with a duration, at a tile
//! or at an object, for one player or all.

use serde::{Deserialize, Serialize};

/// Entry of the vision source list (`[2]`...`[7]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionSource {
    /// Tile x; -1 = bound to the object `z_or_hid` (`[2]`).
    pub x: i32,
    /// Tile z or ID of the object (`[3]`).
    pub z_or_hid: i32,
    /// Sight in quarter tiles (`[4]`).
    pub size: i32,
    /// Player; -1 = all (`[5]`).
    pub player: i32,
    /// Remaining time in ms; -1 = unlimited (`[6]`).
    pub remaining_ms: i32,
    /// Stamp kinds [`super::EXPLORE`] | [`super::SEE`] | [`super::CLEAR_UP`] (`[7]`).
    pub flags: u8,
}

/// `GameWorld_SetVisionSource(x, z, size, player, duration_ms, clear_up, flags)` (`0x0047E010`); the
/// argument `clear_up` (-> flag 4) is folded into `flags` by the caller. If there is an entry with
/// `x == −1 && z_or_hid == z`, it is overwritten entirely, otherwise appended at the end. **Quirk:** the search
/// checks `x == −1` only on the existing entry: a tile source whose z happens to equal the ID of a bound source
/// overwrites it.
pub fn set_vision_source(fog: &mut super::Fog, source: VisionSource) {
    match fog
        .sources
        .iter_mut()
        .find(|s| s.x == -1 && s.z_or_hid == source.z_or_hid)
    {
        Some(s) => *s = source,
        None => fog.sources.push(source),
    }
}

/// `GameWorld_RemoveVisionSourceAt(x, z)` (`0x0047E0F0`, re/vision 4): removes the **first**
/// entry with `x == x && z_or_hid == z_or_hid` (object-bound: `x = −1`, `z_or_hid` = ID);
/// without an entry nothing. Caller is the destructor of the headhunter (re/headhunter-robbing 1.2).
pub fn remove_vision_source(fog: &mut super::Fog, x: i32, z_or_hid: i32) {
    if let Some(p) = fog
        .sources
        .iter()
        .position(|s| s.x == x && s.z_or_hid == z_or_hid)
    {
        fog.sources.remove(p);
    }
}

/// `0x0047DF80(dt)` (before every fog step, `dt` = 50): every source with remaining time != -1 loses
/// `dt`; below 0 it is removed (10,000 ms last 201 engine ticks).
pub fn tick_vision_sources(fog: &mut super::Fog, dt: i32) {
    fog.sources.retain_mut(|s| {
        if s.remaining_ms == -1 {
            return true;
        }
        let rest = s.remaining_ms.wrapping_sub(dt);
        s.remaining_ms = rest;
        rest >= 0
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::fog::{EXPLORE, Fog, SEE};
    use crate::sim::Sim;

    fn at(x: i32, z: i32, player: i32, ms: i32) -> VisionSource {
        VisionSource {
            x,
            z_or_hid: z,
            size: 8,
            player,
            remaining_ms: ms,
            flags: EXPLORE | SEE,
        }
    }

    #[test]
    fn bound_sources_are_overwritten_by_their_hid_and_others_appended() {
        let mut fog = Fog::new(16, 16);
        set_vision_source(&mut fog, at(-1, 7, -1, -1));
        set_vision_source(&mut fog, at(3, 5, 0, -1));
        set_vision_source(&mut fog, at(3, 5, 0, -1));
        assert_eq!(fog.sources.len(), 3, "tile sources are never overwritten");
        set_vision_source(&mut fog, at(-1, 7, 2, 300));
        assert_eq!((fog.sources.len(), fog.sources[0].player), (3, 2));
        // Quirk: a tile source with z = 7 overwrites the one bound to object 7.
        let tile = VisionSource {
            size: 4,
            flags: EXPLORE,
            ..at(2, 7, 1, 500)
        };
        set_vision_source(&mut fog, tile.clone());
        assert_eq!((fog.sources.len(), &fog.sources[0]), (3, &tile));
    }

    #[test]
    fn a_bound_vision_source_can_be_removed() {
        let mut fog = Fog::new(16, 16);
        set_vision_source(&mut fog, at(-1, 7, -1, -1));
        set_vision_source(&mut fog, at(-1, 9, 0, -1));
        // `set_vision_source` appends both of two equal tile sources.
        set_vision_source(&mut fog, at(3, 5, 0, -1));
        set_vision_source(&mut fog, at(3, 5, 1, 500));
        remove_vision_source(&mut fog, -1, 7);
        assert_eq!(
            fog.sources,
            [at(-1, 9, 0, -1), at(3, 5, 0, -1), at(3, 5, 1, 500)]
        );
        // Second removal without an entry: nothing.
        remove_vision_source(&mut fog, -1, 7);
        assert_eq!(fog.sources.len(), 3);
        // Of two equal tile sources only the first goes.
        remove_vision_source(&mut fog, 3, 5);
        assert_eq!(fog.sources, [at(-1, 9, 0, -1), at(3, 5, 1, 500)]);
    }

    #[test]
    fn timed_sources_live_201_steps_and_unbounded_ones_stay() {
        let mut fog = Fog::new(16, 16);
        set_vision_source(&mut fog, at(1, 1, 0, 10_000));
        set_vision_source(&mut fog, at(-1, 99, -1, -1));
        for _ in 0..200 {
            tick_vision_sources(&mut fog, 50);
        }
        let rest: Vec<i32> = fog.sources.iter().map(|s| s.remaining_ms).collect();
        assert_eq!(rest, [0, -1]);
        tick_vision_sources(&mut fog, 50);
        assert_eq!(fog.sources, [at(-1, 99, -1, -1)]);
    }

    #[test]
    fn mercenaries_vision_flags_and_destroyed_buildings_set_sources() {
        const EXTRA: &str = concat!(
            "GUID 2720\nNAME Söldner\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
            "GUID 2726\nNAME Kein_Söldner\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
            "GUID 4300\nNAME Spielerfahne\nVALUEID 100 6 1 0\nPROPID 5\nVALUEID 60002 6 1 512\n",
        );
        let mut rules = kit::rules_from(&format!("{}{EXTRA}", kit::TYPES));
        rules
            .defaults
            .insert(4300, [(18, 40)].into_iter().collect());
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let merc = kit::spawn(&mut sim, 2720, 0, (5, 5));
        kit::spawn(&mut sim, 2726, 0, (6, 5));
        kit::spawn(&mut sim, 4300, 8, (12, 9));
        let house = kit::spawn(&mut sim, kit::HOUSE, 1, (30, 30));
        let tile = sim.object(house).unwrap().tile;
        crate::module::building::destroy::destroy(&mut sim, house);
        let expected = [
            VisionSource {
                z_or_hid: merc.as_i32(),
                ..at(-1, 0, -1, -1)
            },
            VisionSource {
                size: 40,
                ..at(12, 9, -1, -1)
            },
            at(tile.0, tile.1, 1, 10_000),
        ];
        assert!(
            sim.state.fog.sources.ends_with(&expected),
            "{:?}",
            sim.state.fog.sources
        );
        // The remaining time runs down per tick (50 ms per engine tick).
        crate::tick::step(&mut state, &rules, &[]);
        assert_eq!(state.fog.sources.last().unwrap().remaining_ms, 9_950);
    }
}
