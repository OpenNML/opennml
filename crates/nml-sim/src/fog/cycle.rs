//! Fog cycle `GameWorld_UpdateFogOfWarStep` (`0x00486CD0`, re/vision 3): per engine tick at most
//! `B = max(1, objects of all slots / 19)` stamps of player objects, players 0...7 in order, then each player's
//! vision sources; after player 7 the background grids are copied into the foreground grids (state -1), followed
//! by an idle tick (-1 -> -2) and, in state -2, the next cycle immediately. Terrain alpha, fog memories and the
//! minimap of these states are the host's business. Named view rectangles ([`stamp_area_views`]) and hidden slots
//! exist only with scripts: for a hidden slot the cycle reads, instead of the slot table, only its special list
//! `SimState::inactive_lists[p]` (`L[p]`, cursor `Fog::inactive_pos`, [`crate::world::hide`]).

use crate::filter::matches_class;
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::sim::Sim;

use super::{CLEAR_UP, EXPLORE, SEE, stamp_side};

/// Divisor of the budget (`/ 0x13`).
const BUDGET_DIVISOR: i32 = 19;

/// One step of the cycle (at the end of each engine tick after [`super::tick_vision_sources`]).
pub fn fog_step(sim: &mut Sim) {
    match sim.state.fog.processed {
        -1 => {
            // Idle tick (minimap, host).
            sim.state.fog.processed = -2;
            return;
        }
        // The host renews the terrain alpha (it checks the fog memories after every step anyway, D-290);
        // then the new cycle follows immediately.
        -2 => sim.state.fog.processed = 0,
        _ => {}
    }
    let mut cursor = sim.state.fog.enum_pos as usize;
    let mut done = 0i32;
    let mut finished = false;
    if sim.state.fog.processed == 0 {
        let all = i32::try_from(sim.state.object_count()).unwrap_or(i32::MAX);
        let fog = &mut sim.state.fog;
        fog.per_tick = (all / BUDGET_DIVISOR).max(1);
        fog.player = 0;
        cursor = 0;
        // `+0x2064 = L[0]` head (`0x00486E18`).
        fog.inactive_pos = 0;
        fog.seen_back.clear();
        fog.cleared_back.clear();
    }
    if sim.state.fog.per_tick > 0 {
        loop {
            let p = sim.state.fog.player;
            // A player >= 8 (end of cycle, also from a savegame) yields no object.
            // The slot flag is re-read for each object (`0x00486E40`): if it changes in the middle of a player,
            // the other path applies from then on.
            let next = if usize::from(p) >= PLAYER_SLOTS {
                None
            } else if sim.state.slot_running[usize::from(p)] {
                next_object(sim, p, &mut cursor)
            } else {
                next_inactive(sim, p)
            };
            match next {
                Some(id) => {
                    if stamp_object(sim, id, p) {
                        done += 1;
                    }
                }
                None => {
                    if usize::from(p) < PLAYER_SLOTS {
                        stamp_sources(sim, p);
                        stamp_area_views(&mut sim.state.fog, p);
                    }
                    let fog = &mut sim.state.fog;
                    fog.player = fog.player.saturating_add(1);
                    if usize::from(fog.player) >= PLAYER_SLOTS {
                        fog.cleared.copy_from(&fog.cleared_back);
                        fog.seen.copy_from(&fog.seen_back);
                        fog.processed = -1;
                        // The original leaves `+0x2064` as it is at the end of the cycle; it is then the null
                        // pointer (list read to the end) or, with a 160 hanging in the middle of slot 7 (A7), stays
                        // dangling and is read again only at the next cycle start, which resets it. The
                        // reimplementation equivalently sets it to 0 right away.
                        fog.inactive_pos = 0;
                        finished = true;
                        break;
                    }
                    cursor = 0;
                    // `+0x2064 = L[p + 1]` head (`0x0048737C`).
                    fog.inactive_pos = 0;
                }
            }
            if done >= sim.state.fog.per_tick {
                break;
            }
        }
    }
    if !finished {
        sim.state.fog.processed = sim.state.fog.processed.wrapping_add(done);
    }
    sim.state.fog.enum_pos = cursor as u32;
}

/// `GameWorld_NextFogObjectOfPlayer` (`0x00476EE0`): from the cursor, the next object of the slot table `p`
/// (skipping gaps); the cursor is then behind it.
fn next_object(sim: &Sim, p: u8, cursor: &mut usize) -> Option<ObjectId> {
    let table = &sim.state.tables[usize::from(p)];
    while let Some(slot) = table.objects.get(*cursor) {
        *cursor += 1;
        if let Some(o) = slot {
            return Some(o.id);
        }
    }
    None
}

/// Special list of a hidden slot (`0x00486E5D...0x00486E98`): from `inactive_pos` the next entry with an existing
/// object (dead IDs skipped, without budget); the cursor is then behind it. `inactive_pos == L.len()` is the end
/// of the list.
fn next_inactive(sim: &mut Sim, p: u8) -> Option<ObjectId> {
    let list = &sim.state.inactive_lists[usize::from(p)];
    let mut pos = sim.state.fog.inactive_pos as usize;
    let mut found = None;
    while let Some(&id) = list.get(pos) {
        pos += 1;
        if sim.state.object(id, false).is_some() {
            found = Some(id);
            break;
        }
    }
    sim.state.fog.inactive_pos = pos as u32;
    found
}

/// Stamp of an object of player `p` at `pos >> 12` (position at the time of the stamp): buildings only when
/// finished (`HasModule(3, 0)` -> `MatchesClass(0x403)`), side from attribute 18 (current). `true` = stamped
/// (counts toward the budget).
fn stamp_object(sim: &mut Sim, id: ObjectId, p: u8) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    if o.has_module(3, false) && !matches_class(sim, id, 0x403) {
        return false;
    }
    let Some(s) = stamp_side(sim.attribute(id, 18)) else {
        return false;
    };
    let Some(o) = sim.object(id) else {
        return false;
    };
    let tile = (o.grid.x.to_int(), o.grid.z.to_int());
    let kinds = if o.clear_up_sight {
        EXPLORE | SEE | CLEAR_UP
    } else {
        EXPLORE | SEE
    };
    sim.state.fog.stamp_sight(tile, s, 1 << p, kinds);
    true
}

/// Vision sources of player `p` and of all (-1) in list order (re/vision 4.3); a source bound to a vanished
/// object is skipped, not deleted.
fn stamp_sources(sim: &mut Sim, p: u8) {
    for i in 0..sim.state.fog.sources.len() {
        let e = sim.state.fog.sources[i].clone();
        if e.player != i32::from(p) && e.player != -1 {
            continue;
        }
        let tile = if e.x == -1 {
            match sim.object(ObjectId::from_i32(e.z_or_hid)) {
                Some(o) => o.tile,
                None => continue,
            }
        } else {
            (e.x, e.z_or_hid)
        };
        if let Some(s) = stamp_side(e.size) {
            sim.state.fog.stamp_sight(tile, s, 1 << p, e.flags);
        }
    }
}

/// Named view rectangles for player `p` (`0x00487126...0x00487346`, re/vision 3.4), after the vision sources.
/// The rectangle variable starts at `(0, 0, 0, 0)` for each player and is overwritten only by entries with slot
/// `p`; but **every** entry is stamped - so a foreign one stamps the last taken rectangle with **its** clear-up
/// flag (E-163). Corners are swapped (in the variable), both bounds inclusive, bit `p` in "explored", "seen"
/// (background) and, with `clear`, in "cleared" (background); the original skips tiles outside the map per tile.
fn stamp_area_views(fog: &mut super::Fog, p: u8) {
    let bit = 1u8 << p;
    let mut rect = [0i32; 4];
    for i in 0..fog.area_views.len() {
        let view = &fog.area_views[i];
        if view.player == p {
            rect = view.rect;
        }
        let clear = view.clear;
        if rect[0] > rect[2] {
            rect.swap(0, 2);
        }
        if rect[1] > rect[3] {
            rect.swap(1, 3);
        }
        let (width, height) = (fog.explored.width, fog.explored.height);
        let (xs, xe) = (rect[0].max(0), rect[2].min(width - 1));
        let (zs, ze) = (rect[1].max(0), rect[3].min(height - 1));
        for z in zs..=ze {
            for x in xs..=xe {
                fog.explored.or(x, z, bit);
                fog.seen_back.or(x, z, bit);
                if clear {
                    fog.cleared_back.or(x, z, bit);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::fog::{CLEAR_UP, EXPLORE, SEE, VisionSource, set_vision_source};
    use crate::id::ObjectId;
    use crate::state::SimState;
    use crate::tick::step;

    fn spawn(
        state: &mut SimState,
        rules: &crate::rules::Rules,
        guid: u32,
        slot: i32,
        tile: (i32, i32),
    ) -> ObjectId {
        kit::spawn(&mut Sim::new(state, rules), guid, slot, tile)
    }

    #[test]
    fn a_unit_with_sight_32_sees_224_tiles_and_explores_345() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        spawn(&mut state, &rules, kit::COLONIST, 0, (32, 32));
        // One object: budget 1 = S. Stamped in the 1st tick, not copied until the 2nd (budget used up,
        // correction to re/vision 3.3).
        step(&mut state, &rules, &[]);
        assert_eq!((state.fog.processed, state.fog.seen.count(1)), (1, 0));
        assert_eq!(
            (state.fog.explored.count(1), state.fog.seen_back.count(1)),
            (345, 224)
        );
        step(&mut state, &rules, &[]);
        assert_eq!((state.fog.processed, state.fog.seen.count(1)), (-1, 224));
        assert!(state.fog.seen.any_in_rect([24, 24, 24, 40], 1));
        assert!(!state.fog.seen.any_in_rect([0, 0, 63, 23], 0xFF));
        assert_eq!(state.fog.cleared.count(0xFF), 0);
    }

    #[test]
    fn the_cycle_takes_floor_s_over_b_plus_one_stamp_steps_and_one_idle_step() {
        let rules = kit::rules();
        let cases = [
            (1, [-1, -2, -1, -2, -1, -2]),
            (2, [2, -1, -2, 2, -1, -2]),
            (3, [2, -1, -2, 2, -1, -2]),
        ];
        for (units, expected) in cases {
            let mut state = kit::fogged_state(&rules);
            for i in 0..units {
                spawn(&mut state, &rules, kit::COLONIST, 0, (10 + 2 * i, 10));
            }
            for i in 0..40 - units {
                spawn(&mut state, &rules, kit::TREE, 9, (i, 50));
            }
            assert_eq!(state.object_count(), 40, "budget 40 / 19 = 2");
            let seq: Vec<i32> = (0..6)
                .map(|_| {
                    step(&mut state, &rules, &[]);
                    state.fog.processed
                })
                .collect();
            assert_eq!(seq, expected, "{units} units");
        }
    }

    #[test]
    fn buildings_see_only_when_finished() {
        let mut rules = kit::rules();
        rules.defaults.get_mut(&kit::HOUSE).unwrap().insert(18, 16);
        let mut state = kit::fogged_state(&rules);
        kit::site(&mut Sim::new(&mut state, &rules), kit::HOUSE, 0, (10, 10));
        spawn(&mut state, &rules, kit::HOUSE, 1, (40, 40));
        for _ in 0..4 {
            step(&mut state, &rules, &[]);
        }
        assert_eq!(
            state.fog.seen.count(1),
            0,
            "the construction site sees nothing"
        );
        assert_eq!(state.fog.seen.count(2), 64);
    }

    #[test]
    fn clear_up_sight_stamps_the_cleared_grid_and_sources_stamp_their_players() {
        let types = kit::TYPES.replace("VALUEID 10003 6 1 264448", "VALUEID 10003 6 1 788736");
        let rules = kit::rules_from(&types);
        let mut state = kit::fogged_state(&rules);
        spawn(&mut state, &rules, kit::COLONIST, 2, (20, 20));
        let fog = &mut state.fog;
        let src = |x, z, player, flags| VisionSource {
            x,
            z_or_hid: z,
            size: 8,
            player,
            remaining_ms: -1,
            flags,
        };
        set_vision_source(fog, src(50, 50, -1, EXPLORE | SEE));
        set_vision_source(fog, src(5, 50, 3, SEE | CLEAR_UP));
        // Bound to an object that does not exist: skipped, not deleted.
        set_vision_source(fog, src(-1, ObjectId::new(7, 0, 1).as_i32(), -1, 3));
        // Player 8 does not exist (slot 8 is not a player slot): the source stamps nothing.
        set_vision_source(fog, src(30, 30, 8, EXPLORE | SEE));
        for _ in 0..2 {
            step(&mut state, &rules, &[]);
        }
        let f = &state.fog;
        assert_eq!(
            (f.cleared.count(1 << 2), f.seen.count(1 << 2)),
            (224, 224 + 16)
        );
        assert_eq!((f.seen.count(1 << 3), f.cleared.count(1 << 3)), (32, 16));
        assert_eq!((f.explored.count(1 << 3), f.seen.count(0x80)), (64, 16));
        assert_eq!(f.sources.len(), 4);
        assert_eq!(
            (f.seen.get(30, 30), f.explored.get(30, 30)),
            (Some(0), Some(0))
        );
    }

    #[test]
    fn explored_stays_when_the_unit_is_gone_and_seen_follows_the_next_copy() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        let unit = spawn(&mut state, &rules, kit::COLONIST, 0, (32, 32));
        for _ in 0..2 {
            step(&mut state, &rules, &[]);
        }
        assert_eq!(state.fog.seen.count(1), 224);
        crate::world::queue_destroy(&mut Sim::new(&mut state, &rules), unit);
        for _ in 0..8 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.object(unit, false).is_none());
        assert_eq!(
            (state.fog.seen.count(1), state.fog.explored.count(1)),
            (0, 345)
        );
    }

    /// A cycle up to the copying of the background grids (state -1).
    fn run_cycle(sim: &mut Sim) {
        loop {
            fog_step(sim);
            if sim.state.fog.processed == -1 {
                break;
            }
        }
    }

    fn view(name: &str, rect: [i32; 4], player: u8, clear: bool) -> crate::fog::AreaView {
        crate::fog::AreaView {
            name: name.to_owned(),
            rect,
            player,
            clear,
        }
    }

    // QUIRK E-163: kept
    #[test]
    fn foreign_area_view_stamps_last_taken_rect_with_its_own_clear_flag() {
        // re/vision 3.4: the rectangle variable is overwritten only for a matching player, but every entry is
        // stamped - with the clear-up flag of the **current** entry, also the foreign one.
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        state.fog.area_views = vec![
            view("a", [10, 10, 11, 11], 1, false),
            view("b", [20, 20, 20, 20], 0, true),
            view("c", [30, 30, 30, 30], 1, true),
        ];
        run_cycle(&mut Sim::new(&mut state, &rules));
        let f = &state.fog;
        // Player 0: a is foreign and comes before the first own entry - rectangle (0, 0, 0, 0), without
        // clear-up; b own (20, 20) with; c foreign stamps (20, 20) once more.
        // Player 1: a own (10...11), b foreign stamps a's rectangle with **b's** clear-up, c own (30, 30).
        // Players 2...7 have no own entry: a without, b and c with clear-up on tile (0, 0).
        for (grid, name) in [(&f.explored, "explored"), (&f.seen, "seen")] {
            assert_eq!(grid.get(0, 0), Some(0xFD), "{name}: all except player 1");
            assert_eq!(grid.get(20, 20), Some(0b01), "{name}");
            assert_eq!(grid.get(30, 30), Some(0b10), "{name}");
            for (x, z) in [(10, 10), (11, 10), (10, 11), (11, 11)] {
                assert_eq!(grid.get(x, z), Some(0b10), "{name} {x},{z}");
            }
            assert_eq!(grid.get(40, 40), Some(0), "{name}");
        }
        assert_eq!(f.cleared.get(0, 0), Some(0xFC), "set only by b/c");
        assert_eq!(f.cleared.get(20, 20), Some(0b01));
        assert_eq!(f.cleared.get(30, 30), Some(0b10));
        for (x, z) in [(10, 10), (11, 10), (10, 11), (11, 11)] {
            assert_eq!(
                f.cleared.get(x, z),
                Some(0b10),
                "a's rectangle, cleared by the flag of the foreign b ({x},{z})"
            );
        }
        assert_eq!(f.explored.count(0xFF), 1 + 1 + 1 + 4);
    }

    // QUIRK E-163: kept
    #[test]
    fn area_views_stamp_inclusive_with_swapped_corners_and_clip_to_the_map() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        state.fog.area_views = vec![
            view("swapped", [5, 4, 3, 2], 3, false),
            view("edge", [62, 60, 70, 70], 3, true),
            view("outside", [-9, -9, -5, -5], 3, true),
        ];
        run_cycle(&mut Sim::new(&mut state, &rules));
        let f = &state.fog;
        // Corner swap: x 3...5, z 2...4, both bounds inclusive.
        assert_eq!(f.explored.count(0b1000), 9 + 2 * 4);
        assert_eq!(f.seen.get(3, 2), Some(0b1000));
        assert_eq!(f.seen.get(5, 4), Some(0b1000));
        assert_eq!(f.seen.get(6, 4), Some(0));
        assert_eq!(f.seen.get(63, 63), Some(0b1000));
        // The rectangles extend beyond the map: only tiles in the grid, no crash; "cleared" only with the flag.
        assert_eq!(f.cleared.count(0b1000), 2 * 4);
        assert_eq!(f.cleared.get(3, 2), Some(0));
    }

    // QUIRK E-163: kept
    #[test]
    fn area_views_of_a_hidden_slot_are_stamped_independently_of_the_slot_flag() {
        // `0x00486FC2...0x00487346` stamp independently of the slot flag.
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        state.slot_running[2] = false;
        state.fog.area_views = vec![view("a", [7, 7, 7, 7], 2, true)];
        run_cycle(&mut Sim::new(&mut state, &rules));
        assert_eq!(state.fog.explored.get(7, 7), Some(0b100));
        assert_eq!(state.fog.cleared.get(7, 7), Some(0b100));
    }

    #[test]
    fn border_units_huge_sight_and_far_sources_do_not_panic() {
        let mut rules = kit::rules();
        rules
            .defaults
            .get_mut(&kit::BOAT)
            .unwrap()
            .insert(18, 100_000);
        let mut state = kit::fogged_state(&rules);
        spawn(&mut state, &rules, kit::COLONIST, 0, (0, 0));
        spawn(&mut state, &rules, kit::COLONIST, 7, (63, 63));
        spawn(&mut state, &rules, kit::BOAT, 1, (30, 30));
        for (x, z) in [(i32::MAX, i32::MIN), (i32::MIN, 5), (70, -9)] {
            set_vision_source(
                &mut state.fog,
                VisionSource {
                    x,
                    z_or_hid: z,
                    size: i32::MAX,
                    player: -1,
                    remaining_ms: -1,
                    flags: 7,
                },
            );
        }
        for _ in 0..6 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.fog.seen.get(0, 0).unwrap() & 1 != 0);
        assert!(state.fog.seen.get(63, 63).unwrap() & 0x80 != 0);
        // Quirk: huge sight: side 256, but the corner at kx - s/2 - the stamp lies next to the map.
        assert_eq!(state.fog.explored.count(2), 0);
    }
}
