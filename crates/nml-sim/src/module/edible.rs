//! Module 21 `CPropertyEdibleAnimal` (re/gathering A): the carcass of a hunted animal.
//! Meat = attribute 24 (full at init); `0x215` (from the dying module, `max(dying animation, 1500)`
//! after death) makes the carcass ready (render bits `0x42`, selectable) and starts the rotting
//! (1 meat every `m_RotTime` = 2000 ms, continues after it is emptied, too, W-64).

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::sim::Sim;

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 0x15;
/// Meat (config 24).
const MEAT: u32 = 24;
/// Module flag "has meat" (`0x215` = bit `0x200` | module `0x15`).
pub const HAS_MEAT: u32 = 0x200;
/// Module flag "rotten" (`ISROT`, `0x115`).
pub const IS_ROT: u32 = 0x100;
const GROUP: u8 = 0x80;

/// `CPropertyEdibleAnimal`; the savegame holds `m_RotTime` (`0x0050FDF0`).
/// The constructor `0x0050FBC0` sets only the base (`rot_time` 0), `Init` sets 2000.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edible {
    /// `m_RotTime` (`+0x70`): fixed 2000 ms (VALUEID 220001 is never read).
    pub rot_time: i32,
}

fn edible<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Edible> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Edible(e) => Some(e),
        _ => None,
    }
}

/// `PropertyEdibleAnimal_Init` (`0x0050FBF0`): `m_RotTime = 2000`, flag "has meat", meat full.
pub fn init(sim: &mut Sim, id: ObjectId, _desc: &ModuleDesc) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags |= HAS_MEAT;
        m.kind = ModuleKind::Edible(Box::new(Edible { rot_time: 2000 }));
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_tracks_current(MEAT, true);
        o.attributes.fill_current(MEAT);
    }
}

/// Scale `rest / max` (rendering). Exception A7: with `max <= 0` do not divide (the original would give
/// `inf`/`NaN`, which would end up in the object state) - scale 0.0. Because of `current_value > 0` this is practically
/// unreachable (only a negative fraction with a negative maximum), hence a separate building block with a test.
fn meat_scale(rest: i32, max: i32) -> f32 {
    if max > 0 {
        rest as f32 / max as f32
    } else {
        0.0
    }
}

/// `PropertyEdibleAnimal_TakeMeat` (`0x0050FC30`): `args[0]` wanted -> taken, `args[1]` remainder;
/// scale `rest / max` (rendering, `f32`); empty -> `0x210` to itself (decay), no longer
/// selectable, "has meat" off.
pub fn take_meat(sim: &mut Sim, id: ObjectId, msg: &mut Message) {
    let Some((cur, max, p)) = sim.object(id).map(|o| {
        let p = sim.type_upgrade_percent(o.owner(), o.guid, MEAT);
        (
            o.attributes.current_value(MEAT, p),
            o.attributes.max_value(MEAT, p),
            p,
        )
    }) else {
        return;
    };
    if cur <= 0 {
        msg.set_arg(0, 0);
        msg.set_arg(1, 0);
        return;
    }
    let taken = msg.arg(0).min(cur);
    let mut rest = cur.wrapping_sub(taken);
    msg.set_arg(0, taken);
    msg.set_arg(1, rest);
    if let Some(o) = sim.object_mut(id) {
        o.progress_scale = meat_scale(rest, max);
    }
    if rest <= 0 {
        send(sim, id, &Message::new(0x210, &[]));
        if let Some(o) = sim.object_mut(id) {
            o.selectable = false;
        }
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.flags &= !HAS_MEAT;
        }
        rest = 0;
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_current_value(MEAT, rest, p);
    }
}

/// `PropertyEdibleAnimal_Query` (`0x0050FD20`).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        0x115 => {
            take_meat(sim, id, msg);
            0
        }
        0x215 => {
            let rot = edible(sim, id).map_or(2000, |e| e.rot_time);
            if let Some(o) = sim.object_mut(id) {
                o.render_bits = 0x42;
                o.progress_scale = 1.0;
                o.selectable = true;
            }
            if let Some(m) = sim.module_mut(id, INDEX) {
                m.base.flags |= IS_ROT;
                m.base
                    .post_delayed_message(rot, Message::new(0x115, &[1, 0]), true, GROUP);
            }
            0
        }
        _ => -1,
    }
}

/// Tick: only `UpdateEvents` (the dying module leaves module `0x15` in the tick, C-150).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;

    #[test]
    fn take_meat_answers_taken_and_rest_and_shrinks_the_corpse() {
        // re/gathering A.6: TakeMeat(150, 150, 1) -> {1, 149}, scale 149/150.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WOLF, 8, (10, 10));
        sim.object_mut(w)
            .unwrap()
            .attributes
            .set_modifier(24, 100, false, 9); // 50 + 100 = 150
        sim.object_mut(w).unwrap().attributes.fill_current(24);
        let mut m = Message::new(0x115, &[1, 0]);
        assert_eq!(crate::messaging::send_mut(&mut sim, w, &mut m), 0);
        assert_eq!((m.arg(0), m.arg(1)), (1, 149));
        assert_eq!(sim.object(w).unwrap().progress_scale, 149.0 / 150.0);
        // More wanted than there is: everything, remainder 0 -> decay `0x210`, no longer selectable, HASMEAT off.
        let mut m = Message::new(0x115, &[500, 0]);
        crate::messaging::send_mut(&mut sim, w, &mut m);
        assert_eq!((m.arg(0), m.arg(1)), (149, 0));
        assert!(!sim.object(w).unwrap().selectable);
        assert_eq!(sim.module(w, INDEX).unwrap().base.flags & HAS_MEAT, 0);
        // Empty: {0, 0}, no effect.
        let mut m = Message::new(0x115, &[1, 0]);
        crate::messaging::send_mut(&mut sim, w, &mut m);
        assert_eq!((m.arg(0), m.arg(1)), (0, 0));
    }

    // QUIRK W-64, C-183: kept
    #[test]
    fn a_dead_wolf_becomes_a_corpse_at_t_plus_18_and_rots_every_20_ticks() {
        // re/combat-death 2.4: 0x215 in T+18 (dying animation 1000 -> max(1000, 1500)); meat -1 in
        // +21, then every 20 (W-64); render bits 0x42, scale 1, selectable (re/animals 4.1 no. 5).
        let rules = rules();
        let mut state = state(&rules);
        let w = {
            let mut sim = Sim::new(&mut state, &rules);
            let w = spawn(&mut sim, WOLF, 8, (10, 10));
            sim.object_mut(w)
                .unwrap()
                .attributes
                .set_current_value(14, 0, 0);
            w
        };
        let (mut died, mut ready, mut rots) = (None, None, Vec::new());
        let mut last = 50;
        for _ in 0..2 * 70 {
            crate::tick::step(&mut state, &rules, &[]);
            let o = state.object(w, false).unwrap();
            let k = state.tick;
            if died.is_none() && o.status & crate::object::status::DIED != 0 {
                died = Some(k);
            }
            if ready.is_none() && o.render_bits == 0x42 {
                ready = Some(k);
                assert_eq!(o.progress_scale, 1.0);
                assert!(o.selectable);
            }
            let meat = o.attributes.current_value(24, 0);
            if meat != last {
                rots.push(k);
                last = meat;
            }
        }
        let (t, r) = (died.unwrap(), ready.unwrap());
        assert_eq!(r - t, 18);
        assert_eq!(rots.iter().map(|k| k - r).collect::<Vec<_>>(), vec![21, 41]);
    }

    #[test]
    fn the_meat_scale_does_not_divide_by_a_zero_or_negative_maximum() {
        // A7: no `inf`/`NaN` in the object state.
        assert_eq!(meat_scale(149, 150), 149.0 / 150.0);
        assert_eq!(meat_scale(10, 0), 0.0);
        assert_eq!(meat_scale(0, 0), 0.0);
        assert_eq!(meat_scale(10, -10), 0.0);
    }

    // QUIRK C-211: kept
    #[test]
    fn a_hunter_can_take_meat_before_the_corpse_is_ready_and_0x215_resets_the_scale() {
        // Quirk C-211: `TakeMeat` does not check ISROT; `0x215` then sets the scale
        // back to 1.0 (the meat taken stays gone).
        let rules = rules();
        let mut state = state(&rules);
        let w = {
            let mut sim = Sim::new(&mut state, &rules);
            let w = spawn(&mut sim, WOLF, 8, (10, 10));
            sim.object_mut(w)
                .unwrap()
                .attributes
                .set_current_value(14, 0, 0);
            w
        };
        while state.object(w, false).unwrap().status & crate::object::status::DIED == 0 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_ne!(
            state.object(w, false).unwrap().render_bits,
            0x42,
            "no carcass yet"
        );
        let mut sim = Sim::new(&mut state, &rules);
        let mut m = Message::new(0x115, &[10, 0]);
        assert_eq!(crate::messaging::send_mut(&mut sim, w, &mut m), 0);
        assert_eq!((m.arg(0), m.arg(1)), (10, 40));
        assert_eq!(sim.object(w).unwrap().progress_scale, 40.0 / 50.0);
        while state.object(w, false).unwrap().render_bits != 0x42 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let o = state.object(w, false).unwrap();
        assert_eq!(o.progress_scale, 1.0);
        assert_eq!(o.attributes.current_value(24, 0), 40);
    }
}
