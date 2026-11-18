//! Module 22 `CPropertyHunting` (vtable `0x006248E8`, re/gathering A,
//! re/animals 4): hunting on the gatherer base.
//!
//! `0x216 {animal}` (command hunt/gut) or `0x216 {}` (search): living animal -> register with the
//! dying module (callback `0x616`), attack it itself (`0x111`), state 102; dead animal ->
//! state 100. In state 100 the tick sends it to the carcass (FINDTO argc 7, success `0x116`), there per
//! step 1 meat (`0x115`, time formula with rate 31), full -> delivery to a food store, then
//! back. No failed-attempt gift (W-63).

use opennml_formats::property::ModuleDesc;
use opennml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_mut};
use crate::module::events::update_events;
use crate::module::stand::{list_modulo, parse_int_list};
use crate::module::{ModuleKind, abort as abort_module};
use crate::object::status::{ISMOUNTED, ISMOUNTEDONBOAT};
use crate::sim::Sim;

use super::{
    capacity, goto_delivery, load, own_load, position, set_animation, set_carried_load, status,
    walk_layer,
};

/// Module index.
pub const INDEX: usize = 22;
/// Food rate (attribute 31, current value; < 1 -> 1, W-65).
pub const RATE: u32 = 31;
/// Object state bit of the hunt (`+0x90` = `0x200`).
pub const STATUS_BIT: u32 = 0x200;
/// Radius of the FINDTO message to the carcass (`DAT_0067296C`).
pub const CARCASS_RADIUS: i32 = 2048;
/// Word 0xC of the search: hard radius + `DAT_00672E54`.
const SEARCH_EXTRA: i32 = 0x400;

/// Work states (`m_Status`) of the hunt.
pub mod state {
    /// Target dead: to the carcass in the next tick.
    pub const GOTO_CARCASS: i32 = 100;
    /// New search in the next tick (`HuntOrGut(None)`).
    pub const SEARCH: i32 = 101;
    /// Hunting (attack running, waiting for `0x616`); the tick does nothing (W-66).
    pub const HUNTING: i32 = 102;
}

/// State of `CPropertyHunting`, saved like `PropertyHunting_SerializeFields`
/// (`0x0051B200`) after the base fields. `m_DeliverAnim` (230004) is in [`super::Gather`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunting {
    pub gather: super::Gather,
    /// `m_WalkAnim` (`+0x9C`, 230002): walk animation with meat.
    pub walk_anim: i32,
    /// `m_StandAnim` (`+0xA0`, 230003): stand animations with meat.
    pub stand_anim: Vec<i32>,
    /// `m_ExtractAnim` (`+0xAC`, 230001): gutting animation, set per step.
    pub extract_anim: i32,
    /// `m_TargetHID` (`+0xB0`): animal or carcass; constructor 0.
    pub target_hid: ObjectId,
    /// `m_lastTimeLeak` (`+0xB4`): time remainder of the gutting steps.
    pub last_time_leak: i32,
}

impl Default for Hunting {
    /// `PropertyHunting_Construct` (`0x0051A860`): state 0, `m_TargetHID` 0, leak 0.
    fn default() -> Self {
        Self {
            gather: super::Gather::default(),
            walk_anim: 0,
            stand_anim: Vec::new(),
            extract_anim: 0,
            target_hid: ObjectId(0),
            last_time_leak: 0,
        }
    }
}

pub(crate) fn hunting<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Hunting> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Hunting(h) => Some(h),
        _ => None,
    }
}

fn hunting_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Hunting> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Hunting(h) => Some(h),
        _ => None,
    }
}

fn status_of(sim: &Sim, id: ObjectId) -> i32 {
    super::state(sim, id, INDEX)
}

fn set_status_of(sim: &mut Sim, id: ObjectId, s: i32) {
    super::set_state(sim, id, INDEX, s);
}

fn target(sim: &Sim, id: ObjectId) -> ObjectId {
    hunting(sim, id).map_or(ObjectId(0), |h| h.target_hid)
}

fn set_target(sim: &mut Sim, id: ObjectId, t: ObjectId) {
    if let Some(h) = hunting_mut(sim, id) {
        h.target_hid = t;
    }
}

fn set_status_flag(sim: &mut Sim, id: ObjectId, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(STATUS_BIT, on);
    }
}

fn mounted(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & (ISMOUNTED | ISMOUNTEDONBOAT) != 0)
}

/// `PropertyHunting_Init` (`0x0051B0A0`): 230001 gutting, 230003 stand list, 230002 walk,
/// 230004 delivery; no flags.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let (extract, walk, deliver, stand) =
        (value(230001), value(230002), value(230004), text(230003));
    if let Some(h) = hunting_mut(sim, id) {
        h.extract_anim = extract;
        h.stand_anim = parse_int_list(&stand, &h.stand_anim);
        h.walk_anim = walk;
        h.gather.deliver_anim = deliver;
    }
}

/// `PropertyHunting_Query` (`0x0051B110`); other codes via the base query (1/-1).
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        // `IntList_GetModulo`: empty list -> -1 (A7).
        0x416 => hunting(sim, id).map_or(-1, |h| list_modulo(&h.stand_anim, m.arg(0))),
        0x116 => {
            let m = m.clone();
            extract_step(sim, id, &m)
        }
        0x216 => {
            let m = m.clone();
            hunt_or_gut(sim, id, Some(&m))
        }
        0x316 => hunting(sim, id).map_or(-1, |h| h.walk_anim),
        0x616 => {
            let m = m.clone();
            on_prey_died(sim, id, &m)
        }
        0x716 => 0,
        _ => super::base_query(sim, id, INDEX, m),
    }
}

/// `PropertyHunting_Tick` (`0x0051AEE0`): hunt bit and mounted/in boat -> abort; then the
/// timers; state 100 -> to the carcass, 101 -> search, 102 -> nothing, otherwise the base tick.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let s = sim.object(id).map_or(0, |o| o.status);
    if s & STATUS_BIT != 0 && s & (ISMOUNTED | ISMOUNTEDONBOAT) != 0 {
        abort_module(sim, id, INDEX);
    }
    update_events(sim, id, INDEX, dt);
    match status_of(sim, id) {
        state::GOTO_CARCASS => goto_carcass(sim, id),
        state::SEARCH => {
            hunt_or_gut(sim, id, None);
        }
        state::HUNTING => {}
        _ => super::tick_base(sim, id, INDEX),
    }
}

/// `PropertyHunting_Abort` (`0x0051A960`): unregister from the target (`0x410 {self}`, also when dying),
/// in state 102 end the attack (`0x411`), state 0, hunt bit off, clear group `0x80`,
/// then the base stop. **Quirk (W-194):** the state is already 0 at the base stop - a
/// docking place at the store (state 2) is not released.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    let t = target(sim, id);
    if sim.object(t).is_some() {
        send(sim, t, &Message::new(0x410, &[id.as_i32()]));
    }
    if status_of(sim, id) == state::HUNTING {
        send(sim, id, &Message::new(0x411, &[]));
    }
    set_status_of(sim, id, status::IDLE);
    set_status_flag(sim, id, false);
    super::clear_events(sim, id, INDEX);
    super::stop(sim, id, INDEX)
}

/// `PropertyHunting_HuntOrGut` (`0x0051AC20`, `0x216`): with exactly one argument the animal (also
/// dying), otherwise the search over the whole map ([`search_prey`], C-181). Nothing found: with a
/// load (of any kind) to the store, otherwise abort (return 0). Found: hunt bit,
/// `m_TargetHID`; dead (`DIED`) -> state 100; alive -> register with the dying module
/// (`{0x310, [self]}`, success `{0x616, [0]}` to itself), attack it itself (`0x111 {animal, 0}`, without
/// feedback messages), state 102 (return 1).
pub fn hunt_or_gut(sim: &mut Sim, id: ObjectId, msg: Option<&Message>) -> i32 {
    let found = match msg.filter(|m| m.args.len() == 1) {
        Some(m) => {
            let t = ObjectId::from_i32(m.arg(0));
            sim.object(t).is_some().then_some(t)
        }
        None => search_prey(sim, id),
    };
    let Some(t) = found else {
        let amount = sim.object(id).map_or(0, |o| o.load_amount);
        if amount == 0 {
            abort_module(sim, id, INDEX);
        } else {
            goto_delivery(sim, id, INDEX, ObjectId::NONE);
        }
        return 0;
    };
    set_status_flag(sim, id, true);
    set_target(sim, id, t);
    let dead = sim
        .object(t)
        .is_some_and(|o| o.status & crate::object::status::DIED != 0);
    if dead {
        set_status_of(sim, id, state::GOTO_CARCASS);
        return 1;
    }
    let register = Envelope {
        msg: Message::new(0x310, &[id.as_i32()]),
        success: Some(Message::new(0x616, &[0])),
        fault: None,
        success_target: id,
        fault_target: ObjectId(0),
    };
    dispatch_command(sim, t, &register);
    dispatch_command(
        sim,
        id,
        &Envelope::bare(Message::new(0x111, &[t.as_i32(), 0])),
    );
    set_status_of(sim, id, state::HUNTING);
    1
}

/// Search from `HuntOrGut(None)`: filter `0x6000036` (`RETURN_DIED | CHECKFOG | 0x20 | ANDALL |
/// CHECKPROPERTY | CHECKHANDLETYPE`), module `0x15` with "has meat" (`0x215`), fog and reference =
/// hunter, distance `0 ... R + 0x400` with cell search (word 0x11 = -1), line of sight for class
/// `0x100011`; only with a valid walk layer (0...8); whole map, edge distance with the hard radius.
fn search_prey(sim: &mut Sim, id: ObjectId) -> Option<ObjectId> {
    let (radius, pos) = (sim.object(id)?.hard_radius, position(sim, id));
    let slot = i32::from(id.slot());
    let mut f = MatchFilter {
        mode: 0x600_0036,
        mode2: 0,
        match_bits: 0x215,
        prop_id: 0x15,
        fog_player: slot,
        ref_object: id.as_i32(),
        range_max: radius.wrapping_add(SEARCH_EXTRA),
        range_min: 0,
        ..MatchFilter::constructed()
    };
    f.line_of_sight = matches_class(sim, id, 0x10_0011);
    let layer = walk_layer(sim, id);
    if !(0..9).contains(&layer) {
        return None;
    }
    f.layer = layer;
    find_nearest_matching(sim, pos, &f, -1, slot, radius)
}

/// `PropertyHunting_OnPreyDied` (`0x0051AA00`, `0x616 {animal}` from the dying module): end the attack and
/// shooting; animal present and not mounted/in boat -> target, state 100, otherwise 101.
fn on_prey_died(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    send(sim, id, &Message::new(0x411, &[]));
    send(sim, id, &Message::new(0xF0A, &[]));
    let prey = ObjectId::from_i32(msg.arg(0));
    if sim.object(prey).is_some() && !mounted(sim, id) {
        set_target(sim, id, prey);
        set_status_of(sim, id, state::GOTO_CARCASS);
    } else {
        set_status_of(sim, id, state::SEARCH);
    }
    0
}

/// `PropertyHunting_GotoCarcass` (`0x0051AAA0`, slot 19): without a target state 101; otherwise state 1
/// and walk FINDTO `{x, z, 2048, x, z, 0, 0}` (argc 7) with success `{0x116, [target]}` and fault
/// `{0x216}` (new search).
pub fn goto_carcass(sim: &mut Sim, id: ObjectId) {
    let t = target(sim, id);
    let Some((x, z)) = sim.object(t).map(|o| (o.grid.x.0, o.grid.z.0)) else {
        set_status_of(sim, id, state::SEARCH);
        return;
    };
    set_status_of(sim, id, status::TO_SOURCE);
    let env = Envelope {
        msg: Message::new(0x200, &[x, z, CARCASS_RADIUS, x, z, 0, 0]),
        success: Some(Message::new(0x116, &[t.as_i32()])),
        fault: Some(Message::new(0x216, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyHunting_ExtractStep` (`0x0051AF50`, `0x116 {carcass}`): mounted/in boat -> abort;
/// foreign load counts 0; carcass present and space free (`max(0, capacity - load) >= 1`) ->
/// `{0x115, [1, 0]}` to the carcass; result > 0 -> load, next step (rate 31), gutting
/// animation. No space -> to the store (-1). Carcass gone or empty -> `HuntOrGut({0x216})` (search).
/// The state stays 1 (state of the path).
fn extract_step(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    if mounted(sim, id) {
        abort_module(sim, id, INDEX);
        return 0;
    }
    let amount = own_load(sim, id, load::FOOD);
    let free = capacity(sim, id).wrapping_sub(amount).max(0);
    let c = ObjectId::from_i32(msg.arg(0));
    if sim.object(c).is_some() {
        if free < 1 {
            goto_delivery(sim, id, INDEX, ObjectId::NONE);
            return 0;
        }
        let mut q = Message::new(0x115, &[1, 0]);
        send_mut(sim, c, &mut q);
        let got = q.arg(0);
        if got > 0 {
            set_carried_load(sim, id, got.wrapping_add(amount), load::FOOD);
            let leak = hunting(sim, id).map_or(0, |h| h.last_time_leak);
            let leak = super::schedule_next_unit(
                sim,
                id,
                INDEX,
                RATE,
                leak,
                Message::new(0x116, &[c.as_i32()]),
            );
            if let Some(h) = hunting_mut(sim, id) {
                h.last_time_leak = leak;
            }
            let anim = hunting(sim, id).map_or(0, |h| h.extract_anim);
            set_animation(sim, id, anim);
            return 0;
        }
    }
    hunt_or_gut(sim, id, Some(&Message::new(0x216, &[])));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::command::testkit::STORE;
    use crate::object::status as st;
    use crate::rules::Rules;
    use crate::state::{SimEvent, SimState};
    use crate::tick::step;

    fn h_of(state: &mut SimState, rules: &Rules, h: ObjectId) -> Hunting {
        let sim = Sim::new(state, rules);
        hunting(&sim, h).unwrap().clone()
    }

    fn meat(state: &SimState, c: ObjectId) -> i32 {
        // An empty carcass decays (`0x210`) and disappears: then 0.
        state
            .object(c, false)
            .map_or(0, |o| o.attributes.current_value(24, 0))
    }

    /// Kill the goat and run until the carcass is ready (`0x215`, render bits `0x42`).
    fn carcass(state: &mut SimState, rules: &Rules, g: ObjectId) {
        {
            let mut sim = Sim::new(state, rules);
            sim.object_mut(g)
                .unwrap()
                .attributes
                .set_current_value(14, 0, 0);
        }
        for _ in 0..40 {
            step(state, rules, &[]);
            if state.object(g, false).unwrap().render_bits == 0x42 {
                return;
            }
        }
        panic!("carcass not ready");
    }

    /// Hunter on the tile of a ready carcass, warehouse nearby. On the tile itself
    /// (not next to it) he arrives only 6 ticks after readiness: the path planner needs several ticks even
    /// in place, a neighboring tile cost 14 ticks and thus one more decay
    /// (re/animals 6.5 sets the arrival <= 11 ticks after readiness).
    fn gutting_scene(rules: &Rules) -> (SimState, ObjectId, ObjectId) {
        let mut state = state(rules);
        let (h, g) = {
            let mut sim = Sim::new(&mut state, rules);
            let g = spawn(&mut sim, GOAT, 8, (10, 10));
            let h = spawn(&mut sim, HUNTER, 0, (10, 10));
            spawn(&mut sim, STORE, 0, (16, 10));
            (h, g)
        };
        carcass(&mut state, rules, g);
        state.events.clear();
        (state, h, g)
    }

    /// Ticks in which the hunter's load changes (with load and remaining meat of the carcass
    /// `c`), and the credits to the hunter (any kind: delivered empty is kind -1).
    fn run(
        state: &mut SimState,
        rules: &Rules,
        h: ObjectId,
        c: ObjectId,
        n: usize,
    ) -> (Vec<(u32, i32, i32)>, Vec<i32>) {
        let (mut loads, mut credits, mut last) = (Vec::new(), Vec::new(), 0);
        for _ in 0..n {
            for e in step(state, rules, &[]) {
                if let SimEvent::Credited { object, amount, .. } = e
                    && object == h
                {
                    credits.push(amount);
                }
            }
            let l = state.object(h, false).map_or(0, |o| o.load_amount);
            if l != last {
                loads.push((state.tick, l, meat(state, c)));
                last = l;
            }
        }
        (loads, credits)
    }

    #[test]
    fn init_reads_the_four_values_and_the_query_answers_the_animations() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HUNTER, 0, (10, 10));
        let x = hunting(&sim, h).unwrap().clone();
        assert_eq!(
            (x.extract_anim, x.walk_anim, x.gather.deliver_anim),
            (12, 13, -1)
        );
        assert_eq!(x.stand_anim, vec![14, 15]);
        assert_eq!(
            (x.target_hid, x.last_time_leak, x.gather.status),
            (ObjectId(0), 0, 0)
        );
        assert_eq!(send(&mut sim, h, &Message::new(0x316, &[])), 13);
        assert_eq!(send(&mut sim, h, &Message::new(0x416, &[3])), 15);
        assert_eq!(send(&mut sim, h, &Message::new(0x716, &[])), 0);
        assert_eq!(send(&mut sim, h, &Message::new(0x1F16, &[])), -1);
    }

    // QUIRK W-66, C-180: kept
    #[test]
    fn hunting_a_living_goat_registers_attacks_and_hangs_in_102() {
        // re/animals 4.1 no. 2; W-66/C-180: if the attack ends without a death, the hunter stays in 102.
        let rules = rules();
        let mut state = state(&rules);
        let (h, g) = {
            let mut sim = Sim::new(&mut state, &rules);
            let g = spawn(&mut sim, GOAT, 8, (16, 10));
            let h = spawn(&mut sim, HUNTER, 0, (10, 10));
            assert_eq!(send(&mut sim, h, &Message::new(0x216, &[g.as_i32()])), 1);
            let x = hunting(&sim, h).unwrap().clone();
            assert_eq!((x.gather.status, x.target_hid), (state::HUNTING, g));
            assert_ne!(sim.object(h).unwrap().status & STATUS_BIT, 0);
            let registered = match &sim.module(g, 0x10).unwrap().kind {
                ModuleKind::Dying(d) => d.listeners.iter().any(|(k, m)| *k == h && m.code == 0x616),
                _ => false,
            };
            assert!(registered, "registered with the dying module");
            // End the attack from outside: no feedback to the hunt.
            send(&mut sim, h, &Message::new(0x411, &[]));
            (h, g)
        };
        for _ in 0..50 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.object(g, false).unwrap().status & st::DIED == 0);
        assert_eq!(h_of(&mut state, &rules, h).gather.status, state::HUNTING);
        assert_ne!(state.object(h, false).unwrap().status & STATUS_BIT, 0);
    }

    #[test]
    fn a_prey_killed_by_another_still_calls_the_hunter() {
        // re/animals 4.2 no. 2: callback 0x616 -> attack off, status 100, then to the carcass.
        let rules = rules();
        let mut state = state(&rules);
        let (h, g) = {
            let mut sim = Sim::new(&mut state, &rules);
            let g = spawn(&mut sim, GOAT, 8, (16, 10));
            let h = spawn(&mut sim, HUNTER, 0, (10, 10));
            send(&mut sim, h, &Message::new(0x216, &[g.as_i32()]));
            sim.object_mut(g)
                .unwrap()
                .attributes
                .set_current_value(14, 0, 0);
            (h, g)
        };
        let mut seen = Vec::new();
        for _ in 0..5 {
            step(&mut state, &rules, &[]);
            seen.push(h_of(&mut state, &rules, h).gather.status);
        }
        assert!(seen.contains(&status::TO_SOURCE), "states {seen:?}");
        assert_eq!(h_of(&mut state, &rules, h).target_hid, g);
        let sim = Sim::new(&mut state, &rules);
        let (x, z) = (
            sim.object(g).unwrap().grid.x.0,
            sim.object(g).unwrap().grid.z.0,
        );
        assert_eq!(
            crate::module::walk::walk(&sim, h).and_then(|w| w.find_to.clone()),
            Some(Message::new(0x200, &[x, z, CARCASS_RADIUS, x, z, 0, 0]))
        );
    }

    // QUIRK W-63: kept
    #[test]
    fn gutting_takes_one_meat_every_11_ticks_and_delivers_20() {
        // re/gathering A.6, re/animals 6.5 (arrival <= 11 ticks after readiness -> remainder 20; C-183).
        let rules = rules();
        let (mut state, h, g) = gutting_scene(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            assert_eq!(send(&mut sim, h, &Message::new(0x216, &[g.as_i32()])), 1);
            assert_eq!(
                hunting(&sim, h).unwrap().gather.status,
                state::GOTO_CARCASS,
                "target dead -> 100"
            );
        }
        // `run` counts engine ticks (2 per object tick): 20 withdrawals = 209 ticks, then the way to the store.
        let (loads, credits) = run(&mut state, &rules, h, g, 800);
        let ticks: Vec<u32> = loads.iter().take(20).map(|l| l.0).collect();
        assert_eq!(loads[19].1, 20, "loads {loads:?}");
        assert!(
            ticks.windows(2).all(|w| w[1] - w[0] == 11),
            "intervals {ticks:?}"
        );
        // Remainder at "full": 50 - 20 - 10 decays (W-64/C-183 run in parallel).
        assert_eq!(loads[19].2, 20, "remainder when full");
        assert_eq!(credits.first(), Some(&20));
        // After the delivery (load 0) it goes back to the carcass.
        assert_eq!(loads[20].1, 0);
    }

    // QUIRK W-65: kept
    #[test]
    fn a_trapper_with_rate_0_takes_one_meat_per_601_ticks() {
        // W-65: rate 0 -> max(1) -> 60000 ms -> 601 ticks.
        let mut rules = rules();
        rules.defaults.get_mut(&HUNTER).unwrap().insert(31, 0);
        let (mut state, h, g) = gutting_scene(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            send(&mut sim, h, &Message::new(0x216, &[g.as_i32()]));
        }
        // The carcass is empty after 50 decays (~ 1000 ticks): only two withdrawals measurable.
        let (loads, _) = run(&mut state, &rules, h, g, 1300);
        assert_eq!(loads.len(), 2, "loads {loads:?}");
        assert_eq!(loads[1].0 - loads[0].0, 601);
    }

    // QUIRK W-67, C-184: kept
    #[test]
    fn a_hunter_with_capacity_0_shuttles_with_empty_hands() {
        // C-184/W-67: capacity 0 -> free < 1 -> empty to the store, credit 0, back; shuttling across ticks,
        // no loop within a tick.
        let mut rules = rules();
        rules.defaults.get_mut(&HUNTER).unwrap().insert(17, 0);
        let (mut state, h, g) = gutting_scene(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            send(&mut sim, h, &Message::new(0x216, &[g.as_i32()]));
        }
        let before = state.resource(0, 2);
        let (loads, credits) = run(&mut state, &rules, h, g, 1200);
        assert!(loads.is_empty(), "never loaded: {loads:?}");
        assert!(
            credits.len() >= 2 && credits.iter().all(|&c| c == 0),
            "credits {credits:?}"
        );
        assert_eq!(state.resource(0, 2), before);
        assert!(meat(&state, g) > 0, "only the decay takes meat");
    }

    // QUIRK C-181: kept
    #[test]
    fn the_automatic_search_finds_living_goats_or_gives_up() {
        // C-181: search over the whole map, also living animals; nothing found -> with a load to the
        // store, otherwise abort.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HUNTER, 0, (10, 10));
        let near = spawn(&mut sim, GOAT, 8, (14, 10));
        spawn(&mut sim, GOAT, 8, (30, 10));
        assert_eq!(send(&mut sim, h, &Message::new(0x216, &[])), 1);
        let x = hunting(&sim, h).unwrap().clone();
        assert_eq!((x.gather.status, x.target_hid), (state::HUNTING, near));

        // Second world without animals (`state` is the variable here, hence the full path).
        let mut empty = crate::combat::testkit::state(&rules);
        let mut sim = Sim::new(&mut empty, &rules);
        let h = spawn(&mut sim, HUNTER, 0, (10, 10));
        spawn(&mut sim, STORE, 0, (16, 10));
        assert_eq!(send(&mut sim, h, &Message::new(0x216, &[])), 0);
        assert_eq!(hunting(&sim, h).unwrap().gather.status, status::IDLE);
        set_carried_load(&mut sim, h, 5, load::FOOD);
        send(&mut sim, h, &Message::new(0x216, &[]));
        assert_eq!(hunting(&sim, h).unwrap().gather.status, status::TO_DELIVERY);
    }

    #[test]
    fn a_hunter_dying_while_gutting_leaves_the_carcass_to_rot() {
        // The dying module aborts the hunt (0x410 to the carcass), no more withdrawal.
        let rules = rules();
        let (mut state, h, g) = gutting_scene(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            send(&mut sim, h, &Message::new(0x216, &[g.as_i32()]));
        }
        let (loads, _) = run(&mut state, &rules, h, g, 80);
        assert!(!loads.is_empty());
        {
            let mut sim = Sim::new(&mut state, &rules);
            sim.object_mut(h)
                .unwrap()
                .attributes
                .set_current_value(14, 0, 0);
        }
        let mut new = h;
        for _ in 0..5 {
            for e in step(&mut state, &rules, &[]) {
                if let SimEvent::OwnerChanged { old, new: n } = e
                    && old == h
                {
                    new = n;
                }
            }
        }
        let x = h_of(&mut state, &rules, new);
        assert_eq!(x.gather.status, status::IDLE);
        let pending = state.object(new, false).unwrap().module(INDEX).unwrap();
        assert!(
            !pending
                .base
                .events
                .iter()
                .chain(&pending.base.timers_ws)
                .any(|e| e.msg.code == 0x116)
        );
        let m0 = meat(&state, g);
        for _ in 0..25 {
            step(&mut state, &rules, &[]);
        }
        assert!(m0 - meat(&state, g) <= 2, "only decay");
    }

    // QUIRK W-194: kept
    #[test]
    fn aborting_on_the_way_to_the_store_keeps_the_dock_claimed() {
        // W-194: `PropertyHunting_Abort` sets state 0 before the base stop; the stop no longer sees
        // "to the store" (2) and does not send `0xD03` - the slot stays claimed with the ID
        // of the hunter. (Otherwise the base releases in state 2, see `lumber.rs`.)
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, HUNTER, 0, (10, 10));
        let store = spawn(&mut sim, STORE, 0, (16, 10));
        set_carried_load(&mut sim, h, 5, load::FOOD);
        send(&mut sim, store, &Message::new(0xC03, &[0, h.as_i32()]));
        let claimed = |sim: &Sim| {
            crate::module::building::building(sim, store)
                .unwrap()
                .parking_lots
                .iter()
                .any(|l| l.hid == h)
        };
        assert!(claimed(&sim), "slot claimed before the abort");
        {
            let x = hunting_mut(&mut sim, h).unwrap();
            x.gather.status = status::TO_DELIVERY;
            x.gather.hq_hid = store;
        }
        assert!(crate::module::abort(&mut sim, h, INDEX));
        assert_eq!(status_of(&sim, h), status::IDLE);
        assert!(
            claimed(&sim),
            "slot stays claimed with the hunter's ID (W-194)"
        );
        assert_eq!(sim.object(h).unwrap().status & STATUS_BIT, 0);
    }

    #[test]
    fn a_hunter_shoots_a_goat_and_brings_the_meat_home() {
        // Overall flow re/animals 4.1: arrows until death, carcass after T+18, gutting, delivery.
        let rules = rules();
        let mut state = state(&rules);
        let (h, g) = {
            let mut sim = Sim::new(&mut state, &rules);
            let g = spawn(&mut sim, GOAT, 8, (15, 10));
            let h = spawn(&mut sim, HUNTER, 0, (10, 10));
            spawn(&mut sim, STORE, 0, (10, 16));
            send(&mut sim, h, &Message::new(0x216, &[g.as_i32()]));
            (h, g)
        };
        let (_, credits) = run(&mut state, &rules, h, g, 1500);
        // The carcass may already be removed after decay.
        assert!(
            state
                .object(g, false)
                .is_none_or(|o| o.status & st::DIED != 0),
            "goat dead"
        );
        assert_eq!(credits.first(), Some(&20), "credits {credits:?}");
    }
}
