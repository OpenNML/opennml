//! Module 15 `CPropertySwimming` (re/swim-dive): swim animations as the response to
//! `0x10F`/`0x20F`/`0x30F`. The swim tick (`0x0052D9C0`, re/swim-dive 3.1) sets and clears `SWIMMING`.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::height::terrain_height_fx;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{grid_to_world, world_to_grid};

use super::ModuleKind;
use super::stand::{list_modulo, parse_int_list};

pub const INDEX: usize = 0x0F;

/// `CPropertySwimming` (`0x84` bytes): only the configuration values, no run state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swim {
    /// `m_SwimWalkAnim` (`+0x7C`, 160002).
    pub walk_anim: i32,
    /// `m_SwimStandAnim` (`+0x70`, 160003).
    pub stand_anim: Vec<i32>,
    /// `m_SwimDieAnim` (`+0x80`, 160004).
    pub die_anim: i32,
}

pub fn swim<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Swim> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Swim(s) => Some(s),
        _ => None,
    }
}

/// `PropertySwimming_Init` (`0x0052DB80`): 160002 to 160004; 160001 is not read, the module flags
/// stay 0.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let Some(ModuleKind::Swim(s)) = sim.module_mut(id, INDEX).map(|m| &mut m.kind) else {
        return;
    };
    let text = desc.values.get(&160_003).map_or("", String::as_str);
    s.walk_anim = desc.value_int(160_002).unwrap_or(0) as i32;
    s.stand_anim = parse_int_list(text, &s.stand_anim);
    s.die_anim = desc.value_int(160_004).unwrap_or(0) as i32;
}

/// `PropertySwimming_Query` (`0x0052D950`, re/swim-dive 2.1): `0x10F` walking, `0x20F {sel}` standing
/// (empty list -1, exception A7), `0x30F` dying, otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let Some(s) = swim(sim, id) else {
        return -1;
    };
    match msg.code {
        0x10F => s.walk_anim,
        0x20F => list_modulo(&s.stand_anim, msg.arg(0)),
        0x30F => s.die_anim,
        _ => -1,
    }
}

/// `PropertySwimming_Tick` (`0x0052D9C0`, re/swim-dive 3.1; `dt` unused): swimming is a status bit
/// with hysteresis on the terrain height at the grid point (in when `ground < W - 2300`, out when
/// `ground > W - 1800`, both strict). The ground comes from the round trip grid -> world -> grid
/// (y too) and the terrain ray. Boat (`ISMOUNTEDONBOAT`) and quarters clear `SWIMMING` but send
/// `0x302` instead of `0x402` (K-233); while diving, `0x302` goes out every tick and `SWIMMING`
/// stays as it is (K-234). No `DIED` test: the tick list handles the end (re/swim-dive 13.1).
/// Then sound 10 when `SWIMMING` and not `DIVING`.
pub fn tick(sim: &mut Sim, id: ObjectId, _dt: i32) {
    let rules = sim.rules;
    let cell = rules.map.cell;
    let water = rules.tile_terrain.water;
    let Some(o) = sim.object(id) else { return };
    let g = world_to_grid(grid_to_world(o.grid, cell), cell);
    let ground = terrain_height_fx(rules, grid_to_world(g, cell));
    let s = o.status;
    let set_swimming = |sim: &mut Sim, on: bool| {
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(status::SWIMMING, on);
        }
    };
    if s & (status::ISMOUNTEDONBOAT | status::QUARTERED) != 0 {
        if s & status::SWIMMING != 0 {
            set_swimming(sim, false);
            // K-233: ONWATER stays (0x302 instead of 0x402).
            send(sim, id, &Message::new(0x302, &[]));
        }
    } else if s & status::DIVING != 0 {
        // K-234: ONWATER on every tick, SWIMMING unchanged.
        send(sim, id, &Message::new(0x302, &[]));
    } else if ground < water.wrapping_sub(2300) && s & status::SWIMMING == 0 {
        // End the camouflage: masking `0x0051D800` (module 25), response 1; pending `0x319`/`0x419`
        // stay (K-302).
        send(sim, id, &Message::new(0x619, &[]));
        set_swimming(sim, true);
        send(sim, id, &Message::new(0x302, &[]));
    } else if ground > water.wrapping_sub(1800) && s & status::SWIMMING != 0 {
        set_swimming(sim, false);
        send(sim, id, &Message::new(0x402, &[]));
    }
    // `NotifyUiIfSelected(0)` belongs to the host (the viewer reads the status).
    let Some(o) = sim.object(id) else { return };
    if o.status & status::SWIMMING != 0 && o.status & status::DIVING == 0 {
        let (object, pos, guid) = (id, o.world_pos, o.guid);
        // `SoundManager_PlayObjectSound3D(10, 0x11, ..)`, every tick.
        sim.state.events.push(SimEvent::Sound3d {
            object,
            action: 10,
            flags: 0x11,
            pos,
            guid,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Fx, FxVec3};
    use crate::messaging::send;
    use crate::module::riding::testkit::*;
    use crate::module::stand;

    // QUIRK K-151: kept
    #[test]
    fn swim_answers_its_animations() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (12, 10));
        assert_eq!(send(&mut sim, w, &Message::new(0x10F, &[])), 3);
        assert_eq!(send(&mut sim, w, &Message::new(0x20F, &[0])), 4);
        assert_eq!(send(&mut sim, w, &Message::new(0x30F, &[])), 5);
        assert_eq!(send(&mut sim, w, &Message::new(0x40F, &[])), -1);
        // No flags (160001 is not read), no run state.
        assert_eq!(sim.module(w, INDEX).unwrap().base.flags, 0);
        assert_eq!(swim(&sim, w).unwrap().stand_anim, vec![4]);
    }

    /// Unit in the lake: `ONWATER` off (the stand initialization already sets it in deep water),
    /// `SWIMMING` as given.
    fn lake_unit(sim: &mut Sim, guid: u32, tile: (i32, i32), swimming: bool) -> ObjectId {
        let u = spawn(sim, guid, 0, tile);
        send(sim, u, &Message::new(0x402, &[]));
        sim.object_mut(u)
            .unwrap()
            .set_status_flags(status::SWIMMING, swimming);
        u
    }

    fn swimming(sim: &Sim, id: ObjectId) -> bool {
        sim.object(id).unwrap().status & status::SWIMMING != 0
    }

    fn onwater(sim: &Sim, id: ObjectId) -> bool {
        sim.module(id, stand::INDEX).unwrap().base.flags & stand::ONWATER != 0
    }

    fn sounds(sim: &Sim) -> Vec<(i32, u32)> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::Sound3d { action, flags, .. } => Some((*action, *flags)),
                _ => None,
            })
            .collect()
    }

    /// One tick on a lake floor of height `floor` (water level 0, cell 128): result
    /// (`SWIMMING`, `ONWATER`) after the tick.
    fn one_tick(floor: f32, swimming_before: bool) -> (bool, bool) {
        let rules = lake_rules(floor);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = lake_unit(&mut sim, WARRIOR, (30, 30), swimming_before);
        assert!(!onwater(&sim, w));
        tick(&mut sim, w, 0);
        (swimming(&sim, w), onwater(&sim, w))
    }

    /// A unit with `MASKED` swims in (ground Fixed -2301): ranger with module 25
    /// (ability test world) or warrior without. Result: `MASKED` after the tick.
    fn masked_after_swimming_in(with_masking: bool) -> bool {
        use crate::module::ability_testkit as tk;
        let floor = -71.906_25;
        let rules = if with_masking {
            tk::lake_rules(floor)
        } else {
            lake_rules(floor)
        };
        let mut state = if with_masking {
            tk::state(&rules)
        } else {
            state(&rules)
        };
        let mut sim = Sim::new(&mut state, &rules);
        let w = if with_masking {
            tk::spawn(&mut sim, tk::RANGER, 0, (30, 30))
        } else {
            spawn(&mut sim, WARRIOR, 0, (30, 30))
        };
        sim.object_mut(w)
            .unwrap()
            .set_status_flags(status::MASKED, true);
        tick(&mut sim, w, 0);
        assert!(swimming(&sim, w));
        sim.object(w).unwrap().status & status::MASKED != 0
    }

    #[test]
    fn swimming_switches_with_strict_hysteresis() {
        // re/swim-dive 9 (vector 1): at cell_y 128 and water level 0: ground -71.875 (Fixed -2300) -> no SWIMMING.
        assert_eq!(one_tick(-71.875, false), (false, false));
        // One step deeper (-71.90625, Fixed -2301) -> SWIMMING on, stand ONWATER on.
        assert_eq!(one_tick(-71.906_25, false), (true, true));
        // Here `0x619` goes to masking (module 25, `0x0051D800`, response 1): a camouflaged unit
        // with module 25 is visible afterwards; without module 25 (response -1) a set MASKED stays.
        assert!(!masked_after_swimming_in(true));
        assert!(masked_after_swimming_in(false));
        // Back (SWIMMING set before, ONWATER off): ground -62.5 (-2000) stays ...
        assert_eq!(one_tick(-62.5, true), (true, false));
        // ... -56.25 (-1800) stays (strict) ...
        assert_eq!(one_tick(-56.25, true), (true, false));
        // ... -56.0 (-1792) -> off.
        assert_eq!(one_tick(-56.0, true), (false, false));
    }

    #[test]
    fn swimming_off_sends_0x402_and_clears_onwater() {
        let rules = lake_rules(-56.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = lake_unit(&mut sim, WARRIOR, (30, 30), true);
        send(&mut sim, w, &Message::new(0x302, &[]));
        assert!(onwater(&sim, w));
        tick(&mut sim, w, 0);
        assert_eq!((swimming(&sim, w), onwater(&sim, w)), (false, false));
    }

    // QUIRK K-233: kept
    #[test]
    fn boat_and_quarter_clear_swimming_but_keep_onwater() {
        // K-233: swimmer with 0x8000 (link to the canoe) -> SWIMMING off, 0x302 (ONWATER stays on).
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = lake_unit(&mut sim, WARRIOR, (30, 30), true);
        let canoe = spawn(&mut sim, CANOE, 0, (34, 34));
        link(&mut sim, w, canoe);
        assert_ne!(sim.object(w).unwrap().status & status::ISMOUNTEDONBOAT, 0);
        tick(&mut sim, w, 0);
        assert_eq!((swimming(&sim, w), onwater(&sim, w)), (false, true));
        // Without SWIMMING in the boat: no message, ONWATER stays off.
        send(&mut sim, w, &Message::new(0x402, &[]));
        tick(&mut sim, w, 0);
        assert_eq!((swimming(&sim, w), onwater(&sim, w)), (false, false));
        // Likewise in quarters (0x100).
        let q = lake_unit(&mut sim, WARRIOR, (26, 26), true);
        sim.object_mut(q)
            .unwrap()
            .set_status_flags(status::QUARTERED, true);
        tick(&mut sim, q, 0);
        assert_eq!((swimming(&sim, q), onwater(&sim, q)), (false, true));
    }

    // QUIRK K-234, K-238: kept
    #[test]
    fn a_diver_keeps_its_swim_bit_and_gets_onwater_every_tick() {
        // K-234: status DIVING (by hand) in shallow water (ground -30) -> SWIMMING unchanged,
        // ONWATER on, in both initial states and every tick.
        for swimming_before in [false, true] {
            let rules = lake_rules(-30.0);
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let d = lake_unit(&mut sim, DIVER, (30, 30), swimming_before);
            sim.object_mut(d)
                .unwrap()
                .set_status_flags(status::DIVING, true);
            tick(&mut sim, d, 0);
            assert_eq!(swimming(&sim, d), swimming_before);
            assert!(onwater(&sim, d));
            send(&mut sim, d, &Message::new(0x402, &[]));
            tick(&mut sim, d, 0);
            assert!(onwater(&sim, d));
        }
        // Deep water: a diver does not become a swimmer.
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let d = lake_unit(&mut sim, DIVER, (30, 30), false);
        sim.object_mut(d)
            .unwrap()
            .set_status_flags(status::DIVING, true);
        tick(&mut sim, d, 0);
        assert!(!swimming(&sim, d));
    }

    #[test]
    fn a_swimmer_plays_sound_10_every_tick() {
        let rules = lake_rules(-80.0);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = lake_unit(&mut sim, WARRIOR, (30, 30), false);
        sim.state.events.clear();
        // First tick: switch to swimmer, sound in the same tick; afterwards every tick.
        tick(&mut sim, w, 0);
        assert_eq!(sounds(&sim), vec![(10, 0x11)]);
        tick(&mut sim, w, 0);
        assert_eq!(sounds(&sim), vec![(10, 0x11), (10, 0x11)]);
        let (pos, guid) = {
            let o = sim.object(w).unwrap();
            (o.world_pos, o.guid)
        };
        assert!(sim.state.events.iter().all(|e| matches!(e,
            SimEvent::Sound3d { object, pos: p, guid: g, .. } if *object == w && *p == pos && *g == guid)));
        // Diving: none.
        sim.state.events.clear();
        sim.object_mut(w)
            .unwrap()
            .set_status_flags(status::DIVING, true);
        tick(&mut sim, w, 0);
        assert!(sounds(&sim).is_empty());
        // Not a swimmer (land): none.
        let land = spawn(&mut sim, WARRIOR, 0, (5, 5));
        sim.state.events.clear();
        tick(&mut sim, land, 0);
        assert!(!swimming(&sim, land));
        assert!(sounds(&sim).is_empty());
    }

    #[test]
    fn world_to_grid_inverts_grid_to_world_like_the_original() {
        let cell = [128; 3];
        // Round trip (x, y, z) = (4096*5 + 7, -2301, 4096*3 + 2049) at cell 128: exact.
        let grid = FxVec3::new(Fx(4096 * 5 + 7), Fx(-2301), Fx(4096 * 3 + 2049));
        let world = grid_to_world(grid, cell);
        assert_eq!(world_to_grid(world, cell), grid);
        // Bit-identical to the previous inline calculation from `apply_placement`: `ftol(p*4096) / cell`,
        // division toward zero (also for negative y).
        for p in [
            [100.3, -5.7, 77.7],
            [0.01, -0.01, 4095.99],
            [-3.2, 12.5, -0.6],
        ] {
            let expect = FxVec3::new(
                Fx(Fx::from_f32(p[0]).0 / cell[0]),
                Fx(Fx::from_f32(p[1]).0 / cell[1]),
                Fx(Fx::from_f32(p[2]).0 / cell[2]),
            );
            assert_eq!(world_to_grid(p, cell), expect);
        }
        // A different cell per axis.
        assert_eq!(
            world_to_grid([64.0, 64.0, 64.0], [64, 128, 256]),
            FxVec3::new(Fx(4096), Fx(2048), Fx(1024))
        );
    }
}
