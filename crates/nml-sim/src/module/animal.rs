//! Module 20 `CPropertyAnimal` (re/animals 1): aggressive animals switch to target search after 100 ms (attacking is
//! done by the attack module), shy ones flee while standing from player objects with an attack module, roamers walk
//! around their home (two draws of the table sequence per attempt). All three behaviors only on slot 8.
//! Appearing/disappearing (`0x414`/`0x114`), the home via script (`0x914`) and "is being evoked" (`0x314`,
//! re/evoke-aura 3.2a) are there.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{region_id_at, wrap_angle};

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 0x14;
const GROUP: u8 = 0x80;
/// Slot of nature/animals.
const NEUTRAL: u8 = 8;
/// Effects of appearing/disappearing per `m_Type` (0 wolf, 1 bear, 2 cat; rendering only).
const APPEAR: [&str; 3] = [
    "data/effects/Wolf_erscheinen.pef",
    "data/effects/Baer_erscheinen.pef",
    "data/effects/Katze_erscheinen.pef",
];
const DISAPPEAR: [&str; 3] = [
    "data/effects/Wolf_verschwinden.pef",
    "data/effects/Baer_verschwinden.pef",
    "data/effects/Katze_verschwinden.pef",
];

/// `CPropertyAnimal` (`0xC8` bytes); in the savegame like `0x004FF3B0` (the effect names follow from `kind`). The
/// result list of the flee check (`+0xAC`) lives only in the call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Animal {
    /// `m_Type` (210002): index of the evocation effects.
    pub kind: i32,
    /// `m_Alpha`/`m_AlphaSpeed`: transparency (rendering), fixed `f32` addition sequence.
    pub alpha: f32,
    pub alpha_speed: f32,
    pub aggressive: bool,
    pub shy: bool,
    pub escaping: bool,
    pub roaming: bool,
    pub roamer: bool,
    pub underwater: bool,
    /// `m_Home` (x, z, height) in fixed point.
    pub home: (i32, i32, i32),
}

impl Default for Animal {
    fn default() -> Self {
        Self {
            kind: 0,
            alpha: 1.0,
            alpha_speed: 0.0125,
            aggressive: false,
            shy: false,
            escaping: false,
            roaming: false,
            roamer: false,
            underwater: false,
            home: (0, 0, 0),
        }
    }
}

pub(crate) fn animal<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Animal> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Animal(a) => Some(a),
        _ => None,
    }
}

pub(crate) fn animal_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Animal> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Animal(a) => Some(a),
        _ => None,
    }
}

fn post(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, GROUP);
    }
}

fn neutral_roamer(sim: &Sim, id: ObjectId) -> bool {
    id.slot() == NEUTRAL && animal(sim, id).is_some_and(|a| a.roamer)
}

/// `PropertyAnimal_Init` (`0x004FEEF0`, re/animals 1.3); `slot = min(owner, 11)`. The masks are the MID constants
/// (C-170). `0x611` is created first, then `0x2100` (they fire in reverse).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, slot: u8) {
    let flags = desc.value_int(210001).unwrap_or(0) as u32;
    let kind = desc.value_int(210002).unwrap_or(0) as i32;
    let neutral = slot == NEUTRAL;
    let aggressive = flags & 0x114 != 0 && neutral && sim.attribute(id, 19) >= 1;
    let shy = flags & 0x1014 != 0 && neutral;
    let roamer = flags & 0x2014 != 0 && neutral;
    let home = sim
        .object(id)
        .map_or((0, 0, 0), |o| (o.grid.x.0, o.grid.z.0, o.grid.y.0));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        m.kind = ModuleKind::Animal(Box::new(Animal {
            kind,
            aggressive,
            shy,
            roamer,
            underwater: flags & 0x414 != 0,
            home,
            ..Animal::default()
        }));
    }
    if aggressive {
        post(sim, id, 100, super::quarter::attack_mode_msg(3, false));
        post(sim, id, 100, super::quarter::walk_behavior_msg(3, false));
    }
    if shy {
        post(sim, id, 700, Message::new(0x614, &[]));
    }
    if neutral && roamer {
        post(sim, id, 5000, Message::new(0x714, &[]));
    }
}

/// `StopRoaming` (`0x004FE000`): only if a roaming order is running: flag off, then walk abort `{0xC00}` (its fault
/// message `0x814` already sees the flag off).
fn stop_roaming(sim: &mut Sim, id: ObjectId) {
    let was = animal_mut(sim, id).is_some_and(|a| std::mem::take(&mut a.roaming));
    if was {
        send(sim, id, &Message::new(0xC00, &[]));
    }
}

/// Target around `centre` at `r` under `ang` with +/-0.25 degree evasion steps (up to 30 attempts, angle not re-folded,
/// C-171): (x, z, hit, attempt counter k after the loop). After 30 failed attempts `k == 31`.
fn search_point(
    sim: &mut Sim,
    id: ObjectId,
    centre: (i32, i32),
    r: i32,
    mut ang: i32,
) -> (i32, i32, bool, i32) {
    let own = region_id_at(sim, id, -1, -1, -1);
    let (mut step, mut k, mut ok) = (0x400, 1, false);
    let (mut x, mut z);
    loop {
        x = centre.0.wrapping_add(fixed_mul(r, cos_deg(ang)));
        z = centre.1.wrapping_add(fixed_mul(r, sin_deg(ang)));
        if x >= 0 && z >= 0 {
            ok = region_id_at(sim, id, -1, x, z) == own;
        }
        ang = ang.wrapping_add(k * step);
        step = -step;
        k += 1;
        if ok || k > 30 {
            break;
        }
    }
    (x, z, ok, k)
}

fn find_to(sim: &mut Sim, id: ObjectId, code: u32, args: &[i32], reply: u32) {
    let env = Envelope {
        msg: Message::new(code, args),
        success: Some(Message::new(reply, &[])),
        fault: Some(Message::new(reply, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyAnimal_PickRoamTarget` (`0x004FE090`, re/animals 1.6): only when standing (status 0), otherwise only the
/// next timer. Two draws of the table sequence, distance 1...6 tiles, angle 0...359 degrees around the home; a hit only
/// in the 30th attempt does not count, without a target the chain ends (C-172).
fn pick_roam_target(sim: &mut Sim, id: ObjectId) {
    if sim.object(id).is_some_and(|o| o.status != 0) {
        if neutral_roamer(sim, id) {
            post(sim, id, 5000, Message::new(0x714, &[]));
        }
        return;
    }
    let r1 = sim.state.rng.rand();
    let dist = (6 - r1 % 6) << 12;
    let r2 = sim.state.rng.rand();
    let ang = wrap_angle((r2 % 360) << 12);
    let home = match animal_mut(sim, id) {
        Some(a) => {
            a.roaming = true;
            (a.home.0, a.home.1)
        }
        None => return,
    };
    let (x, z, _, k) = search_point(sim, id, home, dist, ang);
    if k < 31 {
        find_to(sim, id, 0x200, &[x, z, 0x1000, NO_DIRECTION, -1, 1], 0x814);
    }
}

/// `PropertyAnimal_CheckFlee` (`0x004FE450`, re/animals 1.7): only when standing; player objects with an attack module
/// in the radius `sight · 512` (center, without fog); flight from the centroid by the same radius (they run only with
/// walk `RUN`); without checking the attempt count (C-173).
fn check_flee(sim: &mut Sim, id: ObjectId) {
    let Some((gx, gz, st)) = sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0, o.status)) else {
        return;
    };
    if st != 0 {
        return;
    }
    let filter = MatchFilter {
        mode: 0x414, // ANDALL | CHECKPROPERTY | ENEMYLIST
        prop_id: 0x11,
        ..MatchFilter::constructed()
    };
    let r = sim.attribute(id, 18).wrapping_shl(12) >> 3;
    let list =
        crate::collision::query_radius(sim, (gx, gz), &filter, r, i32::from(id.slot()), false);
    if list.is_empty() {
        return;
    }
    if let Some(a) = animal_mut(sim, id) {
        a.escaping = true;
    }
    stop_roaming(sim, id);
    let (mut sx, mut sz) = (0i32, 0i32);
    for &o in &list {
        if let Some(p) = sim.object(o) {
            sx = sx.wrapping_add(p.grid.x.0);
            sz = sz.wrapping_add(p.grid.z.0);
        }
    }
    let n = list.len() as i32;
    let (cx, cz) = (sx / n, sz / n);
    let ang = if (cx, cz) != (gx, gz) {
        wrap_angle(atan2_deg(gz.wrapping_sub(cz), gx.wrapping_sub(cx)))
    } else {
        0
    };
    let (x, z, _, _) = search_point(sim, id, (gx, gz), r, ang);
    find_to(sim, id, 0x100, &[x, z, -1, NO_DIRECTION, -1, 1], 0x514);
}

/// "Is being evoked" `0x314` (block `0x004FEC12`, re/evoke-aura 3.2a, from the appearance of module 32): module flag
/// `0x200` (`IS_EVOKED`), meat module `0x15` off via the pending list (`GameObject_QueueModuleTickChange(0x15, 0)`,
/// `0x004FEC23` -> `0x00467030`: applying it takes it out of the tick and switches it off - the carcass thus becomes an
/// ordinary corpse, C-146), neither aggressive nor shy, end roaming, clear timer group `0x80` and start the influence
/// (`{0x22C, 0}` to module 44).
fn evoked(sim: &mut Sim, id: ObjectId) {
    super::set_match_bits(sim, id, 0x214, true);
    if let Some(o) = sim.object_mut(id) {
        o.queue_module_tick_change(super::edible::INDEX, false);
    }
    if let Some(a) = animal_mut(sim, id) {
        a.aggressive = false;
        a.shy = false;
    }
    stop_roaming(sim, id);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    send(sim, id, &Message::new(0x22C, &[0]));
}

/// `PropertyAnimal_Query` (`0x004FEB80`, re/animals 1.4); return value 1, otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let Some(a) = animal(sim, id).cloned() else {
        return -1;
    };
    match msg.code {
        0x114 => {
            post(sim, id, 1000, Message::new(0x214, &[]));
            if let Some(x) = animal_mut(sim, id) {
                x.alpha = 1.0;
                x.alpha_speed = -0.0125;
            }
            if let Some(o) = sim.object_mut(id) {
                o.selectable = false;
            }
            if let Some(p) = usize::try_from(a.kind).ok().and_then(|k| DISAPPEAR.get(k)) {
                sim.state.events.push(SimEvent::EffectOnObject {
                    object: id,
                    path: (*p).into(),
                });
            }
        }
        0x214 => crate::world::queue_destroy(sim, id),
        0x314 => evoked(sim, id),
        0x414 => {
            if let Some(x) = animal_mut(sim, id) {
                x.alpha = 0.0;
                x.alpha_speed = 0.0125;
            }
            if let Some(p) = usize::try_from(a.kind).ok().and_then(|k| APPEAR.get(k)) {
                sim.state.events.push(SimEvent::EffectOnObject {
                    object: id,
                    path: (*p).into(),
                });
            }
        }
        0x514 => {
            let pos = sim
                .object(id)
                .map_or(a.home, |o| (o.grid.x.0, o.grid.z.0, o.grid.y.0));
            if let Some(x) = animal_mut(sim, id) {
                x.escaping = false;
                x.home = pos;
            }
            post(sim, id, 700, Message::new(0x614, &[]));
            if neutral_roamer(sim, id) {
                post(sim, id, 5000, Message::new(0x714, &[]));
            }
        }
        0x614 => {
            check_flee(sim, id);
            if !animal(sim, id).is_some_and(|x| x.escaping) {
                post(sim, id, 700, Message::new(0x614, &[]));
            }
        }
        0x714 => {
            if !a.escaping {
                pick_roam_target(sim, id);
            }
        }
        0x814 => {
            if a.roaming {
                if let Some(x) = animal_mut(sim, id) {
                    x.roaming = false;
                }
                if neutral_roamer(sim, id) {
                    post(sim, id, 5000, Message::new(0x714, &[]));
                }
            }
        }
        0x914 => {
            if let Some(x) = animal_mut(sim, id) {
                x.home = (msg.arg(0), msg.arg(2), msg.arg(1));
            }
        }
        0xA14 => return i32::from(a.underwater),
        _ => return -1,
    }
    1
}

/// `PropertyAnimal_Tick` (`0x004FDF90`): advance alpha and clamp it (rendering), then the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    if let Some(a) = animal_mut(sim, id) {
        a.alpha = (a.alpha + a.alpha_speed).clamp(0.0, 1.0);
    }
    update_events(sim, id, INDEX, dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    fn at(sim: &mut Sim, guid: u32, slot: i32, x: i32, z: i32) -> ObjectId {
        let mut d = ObjectDesc::new(guid, slot, [x as f32 / 32.0, 0.0, z as f32 / 32.0]);
        d.flags |= desc_flags::PLACED;
        create_object(sim, &mut d).unwrap()
    }

    fn find_to(sim: &Sim, id: ObjectId) -> Option<Message> {
        crate::module::walk::walk(sim, id).and_then(|w| w.find_to.clone())
    }

    #[test]
    fn init_derives_the_flags_only_on_slot_8_and_posts_the_timers() {
        // re/animals 6.1.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WOLF, 0, (10, 10));
        assert_eq!(w.slot(), 8, "category 0x400 -> slot 8");
        let a = animal(&sim, w).unwrap().clone();
        assert!(a.aggressive && !a.shy && a.roamer && !a.underwater);
        assert_eq!((a.alpha, a.alpha_speed), (1.0, 0.0125));
        let codes: Vec<(u32, i32)> = sim
            .module(w, INDEX)
            .unwrap()
            .base
            .timers_ws
            .iter()
            .map(|e| (e.msg.code, e.b))
            .collect();
        // Created: 0x611, 0x2100 (100 each), 0x714 (5000); the most recently created is at the front.
        assert_eq!(codes, vec![(0x714, 5000), (0x2100, 100), (0x611, 100)]);
        let g = spawn(&mut sim, GOAT, 0, (12, 10));
        let a = animal(&sim, g).unwrap().clone();
        assert!(!a.aggressive && a.shy && a.roamer);
        let s = spawn(&mut sim, SHARK, 0, (14, 10));
        assert_eq!(
            send(&mut sim, s, &Message::new(0xA14, &[])),
            1,
            "underwater (C-36)"
        );
        assert_eq!(send(&mut sim, w, &Message::new(0xA14, &[])), 0);
    }

    #[test]
    fn roaming_draws_two_table_values_and_walks_around_home() {
        // re/animals 6.2: first draws 18467, 6334 -> 1 tile at -146 degrees -> (-3395, -2290).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = at(&mut sim, GOAT, 8, 40960, 40960);
        let before = sim.state.rng.index();
        send(&mut sim, g, &Message::new(0x714, &[]));
        assert_eq!(
            sim.state.rng.index(),
            before + 2,
            "two draws of the table sequence"
        );
        // The target lies just inside the target radius (hypot ~ 4095 < 4096): Walk reports success immediately
        // (`0x814`), the flag is off again and the next `0x714` is due in 5000 ms.
        assert!(!animal(&sim, g).unwrap().roaming);
        let timers: Vec<(u32, i32)> = sim
            .module(g, INDEX)
            .unwrap()
            .base
            .timers_ws
            .iter()
            .map(|e| (e.msg.code, e.b))
            .collect();
        assert_eq!(
            timers.iter().filter(|t| **t == (0x714, 5000)).count(),
            2,
            "init timer plus new one: {timers:?}"
        );
        assert_eq!(
            find_to(&sim, g),
            Some(Message::new(
                0x200,
                &[
                    40960 - 3395,
                    40960 - 2290,
                    0x1000,
                    crate::motion::NO_DIRECTION,
                    -1,
                    1
                ]
            ))
        );
        // While the animal is moving (status != 0): no draw, only the next 0x714.
        sim.object_mut(g).unwrap().status |= crate::object::status::MOVE;
        let before = sim.state.rng.index();
        let count_714 = |sim: &Sim| {
            sim.module(g, INDEX)
                .unwrap()
                .base
                .timers_ws
                .iter()
                .filter(|e| (e.msg.code, e.b) == (0x714, 5000))
                .count()
        };
        let timers_before = count_714(&sim);
        send(&mut sim, g, &Message::new(0x714, &[]));
        assert_eq!(sim.state.rng.index(), before);
        assert_eq!(count_714(&sim), timers_before + 1, "newly posted 0x714");
        // Second roaming step (re/animals 6.2 row 2: 26500, 19169 -> 2 tiles at 89 degrees): the target lies outside
        // the target radius, the flag stays and Walk really runs.
        sim.object_mut(g).unwrap().status &= !crate::object::status::MOVE;
        let before = sim.state.rng.index();
        send(&mut sim, g, &Message::new(0x714, &[]));
        assert_eq!(
            sim.state.rng.index(),
            before + 2,
            "two further draws of the table sequence"
        );
        assert!(animal(&sim, g).unwrap().roaming, "roaming order running");
        assert_eq!(
            find_to(&sim, g),
            Some(Message::new(
                0x200,
                &[
                    40960 + 142,
                    40960 + 8190,
                    0x1000,
                    crate::motion::NO_DIRECTION,
                    -1,
                    1
                ]
            ))
        );
        assert_ne!(
            sim.object(g).unwrap().status & crate::object::status::MOVE,
            0,
            "walk running"
        );
    }

    #[test]
    fn a_shy_goat_flees_four_tiles_away_from_the_centroid() {
        // re/animals 6.3.
        let rules = rules();
        for (threats, want) in [
            (vec![(49152, 40960)], (24576, 40960)),
            (vec![(49152, 40960), (40960, 49152)], (29376, 29376)),
        ] {
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let g = at(&mut sim, GOAT, 8, 40960, 40960);
            for (x, z) in threats {
                at(&mut sim, ENG_INFANTRY, 0, x, z);
            }
            send(&mut sim, g, &Message::new(0x614, &[]));
            assert!(animal(&sim, g).unwrap().escaping);
            assert_eq!(
                find_to(&sim, g),
                Some(Message::new(
                    0x100,
                    &[want.0, want.1, -1, crate::motion::NO_DIRECTION, -1, 1]
                ))
            );
        }
    }

    #[test]
    fn workers_and_moving_goats_cause_no_flight_and_flight_ends_with_a_new_home() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = at(&mut sim, GOAT, 8, 40960, 40960);
        // Colonist without attack module: no flight (C-174).
        at(&mut sim, crate::command::testkit::COLONIST, 0, 45056, 40960);
        send(&mut sim, g, &Message::new(0x614, &[]));
        assert!(!animal(&sim, g).unwrap().escaping);
        // Walking goat (status MOVE) with a threat next to it: no flight (`0x614` checks only when standing).
        sim.object_mut(g).unwrap().status |= crate::object::status::MOVE;
        at(&mut sim, ENG_INFANTRY, 0, 45056, 40960);
        send(&mut sim, g, &Message::new(0x614, &[]));
        assert!(!animal(&sim, g).unwrap().escaping);
        sim.object_mut(g).unwrap().status &= !crate::object::status::MOVE;
        // Flight ended (0x514): home = current position, 0x614 and 0x714 new.
        sim.module_mut(g, INDEX).unwrap().base.timers_ws.clear();
        sim.object_mut(g).unwrap().grid.x = crate::fx::Fx(50000);
        send(&mut sim, g, &Message::new(0x514, &[]));
        assert_eq!(animal(&sim, g).unwrap().home.0, 50000);
        let codes: Vec<u32> = sim
            .module(g, INDEX)
            .unwrap()
            .base
            .timers_ws
            .iter()
            .map(|e| e.msg.code)
            .collect();
        assert!(codes.contains(&0x614) && codes.contains(&0x714));
    }

    #[test]
    fn fleeing_after_30_failures_sends_the_last_point() {
        // C-173: all 30 attempts lie outside the map (negative position) -> the last point goes out.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = at(&mut sim, GOAT, 8, 2048, 2048);
        at(&mut sim, ENG_INFANTRY, 0, 2048 + 8192, 2048);
        send(&mut sim, g, &Message::new(0x614, &[]));
        let m = find_to(&sim, g).unwrap();
        assert_eq!(m.code, 0x100);
        assert!(m.arg(0) < 0, "flee point outside: {:?}", m.args);
    }

    #[test]
    fn a_roam_without_a_valid_cell_sends_nothing_and_ends_the_chain() {
        // C-172: without a valid cell (all 30 attempts invalid) no order and no new 0x714.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = at(&mut sim, GOAT, 8, 2048, 2048);
        // Set the home in the map corner outside: all attempts negative.
        if let Some(a) = animal_mut(&mut sim, g) {
            a.home = (-50_000, -50_000, 0);
        }
        // The timers from init (0x614, 0x714) are still in `m_TimersWS`.
        sim.module_mut(g, INDEX).unwrap().base.timers_ws.clear();
        send(&mut sim, g, &Message::new(0x714, &[]));
        assert!(animal(&sim, g).unwrap().roaming, "stays set");
        assert!(find_to(&sim, g).is_none());
        let codes: Vec<u32> = sim
            .module(g, INDEX)
            .unwrap()
            .base
            .timers_ws
            .iter()
            .map(|e| e.msg.code)
            .collect();
        assert!(!codes.contains(&0x714), "chain dead");
    }

    #[test]
    fn an_aggressive_wolf_attacks_a_settler_who_fights_back() {
        // re/animals 3.2: after 100 ms stance 4; target search only human player walkers; the unit retaliates (C-34,
        // m_AttackAnimals).
        let rules = rules();
        let mut state = state(&rules);
        let (w, s) = {
            let mut sim = Sim::new(&mut state, &rules);
            let w = spawn(&mut sim, WOLF, 8, (10, 10));
            let s = spawn(&mut sim, SABRE, 0, (13, 10));
            (w, s)
        };
        for _ in 0..2 * 120 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let sim = Sim::new(&mut state, &rules);
        let hp = |id| {
            sim.state
                .object(id, false)
                .map_or(0, |o| o.attributes.current_value(14, 0))
        };
        assert!(hp(s) < 190, "the wolf attacks");
        assert!(hp(w) < 80, "the saber fighter retaliates");
        assert_eq!(
            crate::module::attack::attack(&sim, w).map(|a| a.aggressiveness),
            Some(4)
        );
    }

    #[test]
    fn the_alpha_follows_the_fixed_addition_and_appear_disappear_turn_it() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOAT, 8, (10, 10));
        send(&mut sim, g, &Message::new(0x114, &[]));
        assert_eq!(animal(&sim, g).unwrap().alpha_speed, -0.0125);
        assert!(!sim.object(g).unwrap().selectable);
        for _ in 0..100 {
            tick(&mut sim, g, 100);
        }
        assert_eq!(animal(&sim, g).unwrap().alpha, 0.0, "clamped to 0");
        send(&mut sim, g, &Message::new(0x414, &[]));
        tick(&mut sim, g, 100);
        assert_eq!(animal(&sim, g).unwrap().alpha, 0.0125);
        // 0x914 (script home): {x, height, z}.
        send(&mut sim, g, &Message::new(0x914, &[1, 2, 3]));
        assert_eq!(animal(&sim, g).unwrap().home, (1, 3, 2));
    }
}
