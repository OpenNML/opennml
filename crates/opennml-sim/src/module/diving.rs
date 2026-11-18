//! Module 24 `CPropertyDiving` (re/swim-dive): dive state, dive animations as the answer
//! to `0x318`/`0x418`, the remembered walk layer, `Dive`/`GoUp` (`0x118`/`0x218`) and the dive tick
//! (`0x0050E080`, re/swim-dive 3.2). The opacity 0.6/1.0 while diving is host (rendering).

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::path;
use crate::sim::Sim;
use crate::world::region_id_at;

use super::ModuleKind;
use super::stand::{list_modulo, parse_int_list};

pub const INDEX: usize = 0x18;
/// Deepest dive depth (fixed y, `m_DiveCount` runs from 0 to -1024).
pub const DIVE_DEPTH: i32 = 1024;
/// Dive layer: `PathPlanner_LayerFromMoveMask(0x24)`.
pub const DIVE_LAYER: i32 = 8;

/// `CPropertyDiving` (`0x90` bytes); order of the fields as in the savegame of the original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diving {
    /// `m_DiveWalkAnim` (`+0x70`, 250001).
    pub walk_anim: i32,
    /// `m_DiveStandAnim` (`+0x74`, 250002).
    pub stand_anim: Vec<i32>,
    /// `m_DiveCount` (`+0x80`): current dive depth, -1024 ... 0.
    pub count: i32,
    /// `m_OrigPPHandleNxN` (`+0x84`): walk layer at init (`0x1400`).
    pub orig_layer: i32,
    /// `m_DivePPHandleNxN` (`+0x88`): dive layer.
    pub dive_layer: i32,
    /// `m_Status` (`+0x8C`): 0 up, 1 down, 2 sinking, 3 rising.
    pub status: i32,
}

impl Default for Diving {
    /// Constructor `0x0050DDE0`.
    fn default() -> Self {
        Self {
            walk_anim: 0,
            stand_anim: Vec::new(),
            count: 0,
            orig_layer: 0,
            dive_layer: DIVE_LAYER,
            status: 0,
        }
    }
}

pub fn diving<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Diving> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Diving(d) => Some(d),
        _ => None,
    }
}

pub fn diving_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Diving> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Diving(d) => Some(d),
        _ => None,
    }
}

/// `PropertyDiving_Init` (`0x0050E170`): 250001, 250002 and the walk layer (`0x1400`; walk is
/// module 0 and initialized earlier, the modules are created in ascending order).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let orig_layer = send(sim, id, &Message::new(0x1400, &[]));
    let Some(d) = diving_mut(sim, id) else {
        return;
    };
    let text = desc.values.get(&250_002).map_or("", String::as_str);
    d.walk_anim = desc.value_int(250_001).unwrap_or(0) as i32;
    d.stand_anim = parse_int_list(text, &d.stand_anim);
    d.orig_layer = orig_layer;
}

/// `PropertyDiving_Query` (`0x0050E020`, re/swim-dive 2.2): `0x318` walking, `0x418 {sel}` standing
/// (empty list -1, A7), `0x118` diving and `0x218` surfacing (both return 0), otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let Some(d) = diving(sim, id) else {
        return -1;
    };
    match msg.code {
        0x118 => {
            dive(sim, id);
            0
        }
        0x218 => {
            go_up(sim, id);
            0
        }
        0x318 => d.walk_anim,
        0x418 => list_modulo(&d.stand_anim, msg.arg(0)),
        _ => -1,
    }
}

/// `PropertyDiving_Dive` (`0x0050DEC0`, re/swim-dive 3.2): only with air (stamina, attribute 28,
/// at least 1), not in a boat and only if the own tile in the dive layer is free (open
/// water, 3 x 3). Neither riding, quarters, swimming, death nor an already
/// running dive is checked: a second `0x118` sets state 2 and the layer again (K-235). The
/// opacity 0.6 and `NotifyUiIfSelected` are host.
pub fn dive(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 28);
    if o.attributes.current_value(28, p) <= 0 || o.status & status::ISMOUNTEDONBOAT != 0 {
        return;
    }
    let Some(layer) = diving(sim, id).map(|d| d.dive_layer) else {
        return;
    };
    // Region at the own position (-1/-1); the function shifts the fixed-point value itself.
    if region_id_at(sim, id, layer, -1, -1) == path::BLOCKED {
        return;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::DIVING, true);
    }
    if let Some(d) = diving_mut(sim, id) {
        d.status = 2;
    }
    send(sim, id, &Message::new(0xF00, &[layer]));
}

/// `PropertyDiving_GoUp` (`0x0050DF90`): only with `DIVING`. Always restores the layer from init,
/// even across a combat swim ban of walk (whose flag stays, K-237).
pub fn go_up(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object_mut(id) else { return };
    if o.status & status::DIVING == 0 {
        return;
    }
    o.set_status_flags(status::DIVING, false);
    let Some(d) = diving_mut(sim, id) else { return };
    d.status = 3;
    let layer = d.orig_layer;
    send(sim, id, &Message::new(0xF00, &[layer]));
}

/// Tick (`0x0050E080`, re/swim-dive 3.2; `dt` unused): while diving every tick costs 10 scale
/// units of air (no drowning: below 1 only `GoUp`); state 2/3 moves the depth by
/// `DIVE_DEPTH / 5` (division toward zero: 204) with clamping, and `height_offset` gets the
/// depth every tick, also in state 0 and 1 (K-77, K-238).
pub fn tick(sim: &mut Sim, id: ObjectId, _dt: i32) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 28);
    let Some(o) = sim.object_mut(id) else { return };
    if o.status & status::DIVING != 0 {
        o.attributes.add_scaled_to_current(28, -10, 10, p);
        if o.attributes.current_value(28, p) < 1 {
            go_up(sim, id);
        }
    }
    let Some(d) = diving_mut(sim, id) else { return };
    match d.status {
        2 => {
            d.count = d.count.wrapping_add(DIVE_DEPTH / -5);
            if d.count < -DIVE_DEPTH {
                d.status = 1;
                d.count = -DIVE_DEPTH;
            }
        }
        3 => {
            d.count = d.count.wrapping_add(DIVE_DEPTH / 5);
            if d.count > 0 {
                d.count = 0;
                d.status = 0;
            }
        }
        _ => {}
    }
    let count = d.count;
    if let Some(o) = sim.object_mut(id) {
        o.height_offset = count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::riding::testkit::*;
    use crate::module::walk;
    use crate::object::status;
    use crate::tick::object_tick;

    /// Diver with a full air supply on the tile `tile` of the lake (ground -80).
    fn full_diver_at(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
        let d = spawn(sim, DIVER, 0, tile);
        sim.object_mut(d)
            .unwrap()
            .attributes
            .set_current_fraction(28, 100_000);
        d
    }

    fn full_diver(sim: &mut Sim) -> ObjectId {
        full_diver_at(sim, (30, 30))
    }

    fn tick1(sim: &mut Sim, id: ObjectId) {
        object_tick(sim, id, 100);
    }

    fn post(sim: &mut Sim, id: ObjectId, code: u32) {
        let msg = Message::new(code, &[]);
        assert!(crate::messaging::post_message(sim, id, msg, true));
    }

    fn is_diving(sim: &Sim, id: ObjectId) -> bool {
        sim.object(id).unwrap().status & status::DIVING != 0
    }

    /// (state, depth, `height_offset`, walk layer).
    fn view(sim: &Sim, id: ObjectId) -> (i32, i32, i32, usize) {
        let d = diving(sim, id).unwrap();
        (
            d.status,
            d.count,
            sim.object(id).unwrap().height_offset,
            walk::walk(sim, id).unwrap().layer,
        )
    }

    /// Simulation height `grid.y` after `snap_height_to_terrain`; in the lake with ONWATER the water level.
    fn surface_height(sim: &mut Sim, id: ObjectId) -> i32 {
        crate::world::snap_height_to_terrain(sim, id);
        sim.object(id).unwrap().grid.y.0
    }

    fn air(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.raw_fraction(28)
    }

    // QUIRK K-77, K-238: kept
    #[test]
    fn diving_descends_in_six_ticks_and_surfaces_in_six() {
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        assert_eq!(view(&sim, d), (0, 0, 0, 1));
        // K-238: The simulation height stays at the water level (clamp in `snap_height_to_terrain`),
        // no matter how deep `height_offset` is.
        let surface_y = surface_height(&mut sim, d);
        post(&mut sim, d, 0x118);
        // Command in tick T: first deduction and first depth step in the same tick.
        for (n, depth) in [-204, -408, -612, -816, -1020].into_iter().enumerate() {
            tick1(&mut sim, d);
            assert_eq!(surface_height(&mut sim, d), surface_y, "tick T+{n}");
            assert_eq!(view(&sim, d), (2, depth, depth, 8), "tick T+{n}");
            assert!(is_diving(&sim, d));
        }
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d), (1, -1024, -1024, 8));
        assert_eq!(surface_height(&mut sim, d), surface_y);
        // At the bottom the depth stays and is written again every tick.
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d), (1, -1024, -1024, 8));
        assert!(is_diving(&sim, d));
        // Surfacing in tick U.
        post(&mut sim, d, 0x218);
        for (n, depth) in [-820, -616, -412, -208, -4].into_iter().enumerate() {
            tick1(&mut sim, d);
            assert_eq!(view(&sim, d), (3, depth, depth, 1), "tick U+{n}");
            assert!(!is_diving(&sim, d));
        }
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d), (0, 0, 0, 1));
    }

    fn onwater(sim: &Sim, id: ObjectId) -> bool {
        sim.module(id, crate::module::stand::INDEX)
            .unwrap()
            .base
            .flags
            & crate::module::stand::ONWATER
            != 0
    }

    fn swimming(sim: &Sim, id: ObjectId) -> bool {
        sim.object(id).unwrap().status & status::SWIMMING != 0
    }

    /// Diver in shallow water: ground -40 (fixed -1280) is above the swim limit W - 1800, a
    /// swimmer would have walked here long ago (K-271); layer 8 is free anyway.
    // QUIRK K-271, K-272: kept
    #[test]
    fn layer_8_has_no_depth_limit_and_the_height_stays_at_the_surface_after_surfacing() {
        let rules = lake_rules(-40.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        // As after swimming in: SWIMMING and stand ONWATER (height = water level).
        sim.object_mut(d)
            .unwrap()
            .set_status_flags(status::SWIMMING, true);
        send(&mut sim, d, &Message::new(0x302, &[]));
        let surface_y = surface_height(&mut sim, d);
        assert_eq!(surface_y, 0, "ONWATER clamps to the water level");
        // K-271: diving succeeds in shallow water, layer 8 without a depth limit.
        post(&mut sim, d, 0x118);
        for _ in 0..8 {
            tick1(&mut sim, d);
        }
        assert!(is_diving(&sim, d));
        assert_eq!(view(&sim, d), (1, -1024, -1024, 8));
        post(&mut sim, d, 0x218);
        for _ in 0..10 {
            tick1(&mut sim, d);
        }
        // Surfaced: swim limit not reached -> SWIMMING and ONWATER off.
        assert!(!is_diving(&sim, d));
        assert_eq!(view(&sim, d), (0, 0, 0, 1));
        assert!(!swimming(&sim, d) && !onwater(&sim, d));
        // K-272: Standing, nobody resets the height: it stays at the water level, 40 units
        // above the ground, even after further ticks.
        for _ in 0..5 {
            tick1(&mut sim, d);
        }
        assert_eq!(sim.object(d).unwrap().grid.y.0, surface_y);
        // Only a snap (the walk step calls it) sets the ground.
        assert_eq!(surface_height(&mut sim, d), -1280);
    }

    // QUIRK K-272: kept
    #[test]
    fn a_diver_without_swimming_keeps_onwater_and_the_surface_height_after_surfacing() {
        // K-234 + K-272: If the unit dove without SWIMMING, ONWATER stays after surfacing;
        // the height stays at the water level, even after a snap.
        let rules = lake_rules(-40.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        send(&mut sim, d, &Message::new(0x402, &[]));
        assert!(!swimming(&sim, d) && !onwater(&sim, d));
        // Shallow and without SWIMMING: diving succeeds (K-271).
        post(&mut sim, d, 0x118);
        for _ in 0..8 {
            tick1(&mut sim, d);
        }
        assert!(is_diving(&sim, d));
        assert!(!swimming(&sim, d) && onwater(&sim, d));
        assert_eq!(surface_height(&mut sim, d), 0);
        post(&mut sim, d, 0x218);
        for _ in 0..10 {
            tick1(&mut sim, d);
        }
        assert!(!is_diving(&sim, d));
        assert!(!swimming(&sim, d) && onwater(&sim, d));
        assert_eq!(sim.object(d).unwrap().grid.y.0, 0);
        assert_eq!(surface_height(&mut sim, d), 0);
    }

    // QUIRK K-236: kept
    #[test]
    fn a_fresh_diver_surfaces_in_the_same_tick() {
        // K-236, re/swim-dive 9.3: Stamina starts at 0 (fraction 28: tick 1 -> 80, tick 2 -> 160, current 1).
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = spawn(&mut sim, DIVER, 0, (30, 30));
        for (fraction, current) in [(80, 0), (160, 1)] {
            tick1(&mut sim, d);
            let a = &sim.object(d).unwrap().attributes;
            assert_eq!(
                (a.raw_fraction(28), a.current_value(28, 0)),
                (fraction, current)
            );
        }
        // Tick 3: deduction -200 -> -40, current 0 < 1 -> surfacing in the same tick, then +80.
        post(&mut sim, d, 0x118);
        tick1(&mut sim, d);
        assert!(!is_diving(&sim, d));
        assert_eq!(view(&sim, d), (0, 0, 0, 1));
        assert_eq!(air(&sim, d), 40);
    }

    // QUIRK K-77, K-236: kept
    #[test]
    fn a_full_diver_surfaces_after_832_ticks() {
        // re/swim-dive 9.4: max 500, full supply -> automatic surfacing in the 832nd dive tick; no drowning.
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        let hp = sim.object(d).unwrap().attributes.current_value(14, 0);
        post(&mut sim, d, 0x118);
        let mut n = 0;
        loop {
            tick1(&mut sim, d);
            n += 1;
            if !is_diving(&sim, d) {
                break;
            }
            assert!(n < 900, "never surfaces");
        }
        assert_eq!(n, 832);
        // Fraction 80 when surfacing (re/swim-dive 9.4, below 50000 / 500 = 100), then the
        // regeneration of the tick (+80).
        assert_eq!(air(&sim, d), 160);
        assert_eq!(sim.object(d).unwrap().attributes.current_value(14, 0), hp);
        assert_eq!(diving(&sim, d).unwrap().status, 3);
    }

    // QUIRK K-235, K-239: kept
    #[test]
    fn dive_is_refused_without_air_in_a_boat_or_outside_open_water() {
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let dive_at = |sim: &mut Sim, d: ObjectId| {
            assert_eq!(send(sim, d, &Message::new(0x118, &[])), 0);
            (is_diving(sim, d), view(sim, d).0, view(sim, d).3)
        };
        // K-239: without stamina (max 0) nothing.
        // The fraction is at 100,000 and regenerates for a few ticks: only the maximum 0 prevents
        // diving (`current_value` would be 0 anyway for any max 0).
        let no_air = spawn(&mut sim, NO_AIR_DIVER, 0, (30, 30));
        sim.object_mut(no_air)
            .unwrap()
            .attributes
            .set_current_fraction(28, 100_000);
        for _ in 0..3 {
            tick1(&mut sim, no_air);
        }
        assert_eq!(
            sim.object(no_air).unwrap().attributes.max_value(28, 0),
            0,
            "max 0 from the type"
        );
        assert_eq!(dive_at(&mut sim, no_air), (false, 0, 1));
        // The same setup with DIVER (max 500) dives.
        let air = full_diver_at(&mut sim, (31, 31));
        for _ in 0..3 {
            tick1(&mut sim, air);
        }
        assert_eq!(dive_at(&mut sim, air), (true, 2, 8));
        // Land in the 3 x 3 window (tile 19 is land): layer 8 region blocked.
        let near_land = full_diver_at(&mut sim, (20, 30));
        assert_eq!(dive_at(&mut sim, near_land), (false, 0, 1));
        // Control: tile 21 has only water in the window, as does the center of the lake.
        let inside = full_diver_at(&mut sim, (21, 30));
        assert_eq!(dive_at(&mut sim, inside), (true, 2, 8));
        let middle = full_diver(&mut sim);
        assert_eq!(dive_at(&mut sim, middle), (true, 2, 8));
        // In a boat (0x8000): nothing.
        let boater = full_diver_at(&mut sim, (26, 26));
        let canoe = spawn(&mut sim, CANOE, 0, (34, 34));
        link(&mut sim, boater, canoe);
        assert_eq!(dive_at(&mut sim, boater), (false, 0, 1));
        // Map edge (position beyond the map): nothing.
        let edge = full_diver(&mut sim);
        sim.object_mut(edge).unwrap().grid.x = crate::fx::Fx(64 << 12);
        assert_eq!(dive_at(&mut sim, edge), (false, 0, 1));
    }

    // QUIRK K-235: kept
    #[test]
    fn diving_twice_restarts_the_descent() {
        // K-235: A second 0x118 in state 1 sets state 2 and the layer again. The
        // posted 0x118 would run before the module ticks and the tick would reset state 2 to 1
        // immediately: send synchronously.
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        send(&mut sim, d, &Message::new(0x118, &[]));
        for _ in 0..6 {
            tick1(&mut sim, d);
        }
        assert_eq!(view(&sim, d), (1, -1024, -1024, 8));
        walk::walk_mut(&mut sim, d).unwrap().layer = 3;
        send(&mut sim, d, &Message::new(0x118, &[]));
        assert_eq!((diving(&sim, d).unwrap().status, view(&sim, d).3), (2, 8));
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d), (1, -1024, -1024, 8));
    }

    // QUIRK K-237: kept
    #[test]
    fn surfacing_restores_the_init_layer_over_the_combat_swim_ban() {
        // K-237: GoUp restores the init layer, the combat swim ban stays set.
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        send(&mut sim, d, &Message::new(0x118, &[]));
        {
            let w = walk::walk_mut(&mut sim, d).unwrap();
            w.swim_disallowed_for_combat = true;
            w.layer = 5;
        }
        send(&mut sim, d, &Message::new(0x218, &[]));
        let w = walk::walk(&sim, d).unwrap();
        assert_eq!((w.layer, w.swim_disallowed_for_combat), (1, true));
        assert_eq!(diving(&sim, d).unwrap().status, 3);
        // Six ticks of surfacing bring the state from 3 to 0; after that 0x218 without DIVING
        // has no effect (state stays 0, the layer stays, no DIVING).
        for _ in 0..6 {
            tick1(&mut sim, d);
        }
        assert_eq!((view(&sim, d), is_diving(&sim, d)), ((0, 0, 0, 1), false));
        walk::walk_mut(&mut sim, d).unwrap().layer = 5;
        send(&mut sim, d, &Message::new(0x218, &[]));
        assert_eq!(walk::walk(&sim, d).unwrap().layer, 5);
        assert_eq!(diving(&sim, d).unwrap().status, 0);
        assert!(!is_diving(&sim, d));
    }

    // QUIRK C-220: kept
    #[test]
    fn a_dead_diver_stays_submerged_after_two_ticks() {
        // C-220 (re/swim-dive 13.1): Swim and Diving tick after death only in the death tick T and
        // in T+1; after that DIVING, depth and layer stay frozen.
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = full_diver(&mut sim);
        post(&mut sim, d, 0x118);
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d).1, -204);
        let before = air(&sim, d);
        sim.object_mut(d)
            .unwrap()
            .attributes
            .set_current_fraction(14, 0);
        tick1(&mut sim, d);
        assert_ne!(sim.object(d).unwrap().status & status::DIED, 0);
        assert_eq!(view(&sim, d), (2, -408, -408, 8));
        let after_t = air(&sim, d);
        assert!(after_t < before, "air deduction in the death tick");
        tick1(&mut sim, d);
        assert_eq!(view(&sim, d), (2, -612, -612, 8));
        assert!(air(&sim, d) < after_t, "air deduction in T+1");
        let frozen = air(&sim, d);
        for _ in 0..20 {
            tick1(&mut sim, d);
            assert_eq!(view(&sim, d), (2, -612, -612, 8));
            assert!(is_diving(&sim, d));
        }
        // No more Diving tick: only the regeneration takes effect, the supply does not drop further.
        assert!(air(&sim, d) >= frozen);
    }

    // QUIRK K-151: kept
    #[test]
    fn diving_answers_its_animations_and_remembers_the_walk_layer() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = spawn(&mut sim, DIVER, 0, (12, 10));
        assert_eq!(send(&mut sim, d, &Message::new(0x318, &[])), 7);
        assert_eq!(send(&mut sim, d, &Message::new(0x418, &[0])), 8);
        assert_eq!(send(&mut sim, d, &Message::new(0x518, &[])), -1);
        let x = diving(&sim, d).unwrap();
        assert_eq!((x.orig_layer, x.dive_layer), (1, DIVE_LAYER));
        assert_eq!((x.status, x.count), (0, 0));
        // Walk layer from 0x1400 (land + water = 1); the module flags stay 0.
        assert_eq!(send(&mut sim, d, &Message::new(0x1400, &[])), 1);
        assert_eq!(sim.module(d, INDEX).unwrap().base.flags, 0);
    }
}
