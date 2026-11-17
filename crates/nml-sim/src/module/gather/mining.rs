//! Module 7 `CPropertyGoldMining` (vtable `0x006241C8`, re/gathering): gold mining.
//!
//! Flow: `0x107 {mine}` -> by FINDTO to the edge of the mine (docking place), there state 300 and
//! per step (`60000 / rate` ms with leak, rate = attribute 33) one unit via `0x303` from the
//! mine; the **first** unit comes immediately on arrival, "full" is recognized only in the 21st step.
//! Then the exit animation `80007[1]` and after its time marker 1 `0x607 {-1}` to itself
//! -> to the gold store. The gold digger does not disappear into the mine.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::{edge_distance_to_object, find_nearest_matching, quartering_container};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_message, send};
use crate::module::building;
use crate::module::events::update_events;
use crate::module::stand::{list_modulo, parse_int_list};
use crate::module::{ModuleKind, abort as abort_module};
use crate::object::status as object_status;
use crate::sim::Sim;

use super::{
    Gather, MINING, gather, gather_mut, list_at, load, own_load, position, post_delayed,
    set_animation, set_carried_load, status,
};

/// Module index.
pub const INDEX: usize = 7;
/// Gold rate (attribute 33, units per minute; maximum with upgrades).
pub const RATE: u32 = 33;
/// Object state bit "mines gold" (`MINING`, `+0x90`).
pub const MINING_BIT: u32 = 0x40;
/// Object state bit "dying": the mine accepts no more work.
const DYING: u32 = 0x4_0000;
/// One tile in fixed point (`DAT_006728E8`).
const TILE: i32 = 0x1000;

/// Messages of gold mining.
pub mod msg {
    /// `{mine}`: mine gold (`PMSG_MINING`).
    pub const START: u32 = 0x107;
    /// Walk: destination reached.
    pub const DEST_REACHED: u32 = 0x207;
    /// Walk: aborted.
    pub const ABORT: u32 = 0x307;
    /// `{sel}` -> stand animation with gold.
    pub const GET_STAND_ANIM: u32 = 0x407;
    /// -> walk animation with gold.
    pub const GET_WALK_ANIM: u32 = 0x507;
    /// `{store}`: deliver (also timer after the exit animation, `{-1}`).
    pub const DELIVER: u32 = 0x607;
    /// Timer: mining step.
    pub const MINE_STEP: u32 = 0x707;
    /// Timer: delivery finished.
    pub const DELIVERED: u32 = 0x807;
    /// `{-1, 0}` -> `{distance, mine}` or `{d, mine}` -> start work (`0x77F`).
    pub const FIND_MINE: u32 = 0x907;
}

/// Work state (`m_Status`) of gold mining.
pub mod state {
    /// Mines at the mine (also during the exit animation).
    pub const MINING: i32 = 300;
}

/// State of `CPropertyGoldMining` (`PropertyGoldMining_Construct` `0x00516510`), saved like
/// `PropertyGoldMining_SerializeFields` (`0x00516E10`) after the base fields. The flags 80004
/// (`m_MatchInt`) are in the module base; the original does not read 80002/80003.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mining {
    pub gather: Gather,
    /// `m_MiningWalkAnim` (`+0x9C`, 80001): walk animation with gold.
    pub walk_anim: i32,
    /// `m_MiningStandAnim` (`+0xA0`, 80005): stand animations with gold.
    pub stand_anim: Vec<i32>,
    /// `m_MiningAnim` (`+0xAC`, 80007): `[0]` mining (set at every step), `[1]` exit.
    pub mining_anim: Vec<i32>,
    /// `m_AnimCount` (`+0xB8`): empty steps since the last full load (survives an abort
    /// and a new command).
    pub anim_count: i32,
    /// `m_lastTimeLeak` (`+0xBC`): time remainder of the mining steps, never reset.
    pub last_time_leak: i32,
}

pub(crate) fn mining<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Mining> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Mining(m) => Some(m),
        _ => None,
    }
}

pub(crate) fn mining_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Mining> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Mining(m) => Some(m),
        _ => None,
    }
}

fn status_of(sim: &Sim, id: ObjectId) -> i32 {
    super::state(sim, id, INDEX)
}

fn set_status_of(sim: &mut Sim, id: ObjectId, s: i32) {
    super::set_state(sim, id, INDEX, s);
}

fn resource(sim: &Sim, id: ObjectId) -> ObjectId {
    gather(sim, id, INDEX).map_or(ObjectId::NONE, |g| g.resource_hid)
}

fn set_status_flags(sim: &mut Sim, id: ObjectId, bits: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bits, on);
    }
}

/// `PropertyGoldMining_Init` (`0x00516CC0`): flags 80004 (`m_MatchInt`), walk animation 80001,
/// stand animations 80005 (list), delivery animation 80006, mining animations 80007 (list).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let flags = crate::module::init_flags(INDEX, desc);
    let (walk, deliver) = (value(80001), value(80006));
    let (stand, anims) = (text(80005), text(80007));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(m) = mining_mut(sim, id) {
        m.walk_anim = walk;
        m.stand_anim = parse_int_list(&stand, &m.stand_anim);
        m.gather.deliver_anim = deliver;
        m.mining_anim = parse_int_list(&anims, &m.mining_anim);
    }
}

/// `PropertyGoldMining_Query` (`0x00516D40`, slot 7); all other codes go through the
/// base query to [`on_message`].
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        // `IntList_GetModulo`: empty list (division by 0 in the original) -> -1 (A7).
        msg::GET_STAND_ANIM => mining(sim, id).map_or(-1, |g| list_modulo(&g.stand_anim, m.arg(0))),
        msg::GET_WALK_ANIM => mining(sim, id).map_or(-1, |g| g.walk_anim),
        msg::MINE_STEP => {
            mine_step(sim, id);
            1
        }
        msg::FIND_MINE => find_mine(sim, id, m),
        _ => super::base_query(sim, id, INDEX, m),
    }
}

/// `PropertyGoldMining_OnMessage` (`0x00516880`, slot 12): `0x107` starts, everything else the
/// base (`0x207` destination reached, `0x307` abort, `0x607` deliver, `0x807` delivery finished).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code == msg::START {
        on_start_mining(sim, id, env);
        return true;
    }
    super::on_command(sim, id, INDEX, env)
}

/// `PropertyGoldMining_Tick` (`0x00516680`, slot 13): except while mining (state 300) the
/// base tick, then the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    if status_of(sim, id) != state::MINING {
        super::tick_base(sim, id, INDEX);
    }
    update_events(sim, id, INDEX, dt);
}

/// `PropertyGoldMining_Abort` (`0x00516630`, slot 11): if state bit `0x100` is set and the
/// mine is the container, the bit is cleared (dead remainder - gold mining never sets `0x100`), then
/// the base abort. `m_AnimCount` and `m_lastTimeLeak` stay (quirk).
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    clear_quartered_at_source(sim, id, resource(sim, id));
    super::stop(sim, id, INDEX)
}

/// Clear state bit `0x100` if the source is the container (`GetQuarteringContainer`).
pub(crate) fn clear_quartered_at_source(sim: &mut Sim, id: ObjectId, source: ObjectId) {
    if quartering_container(sim, id).is_some_and(|c| c == source) {
        set_status_flags(sim, id, object_status::QUARTERED, false);
    }
}

/// `PropertyGoldMining_OnDestReached` (`0x00516AB0`, slot 18): in state 1 arrived at the mine
/// -> first mining step, otherwise the base (state 2 -> deliver).
pub fn on_dest_reached(sim: &mut Sim, id: ObjectId, _env: &Envelope) -> bool {
    if status_of(sim, id) == status::TO_SOURCE {
        mine_step(sim, id);
        return true;
    }
    super::base_on_dest_reached(sim, id, INDEX)
}

/// `PropertyGoldMining_OnStartMining` (`0x005166B0`, command `0x107 {mine}`): take over the
/// feedback messages, `m_ResourceHID`, bit `0x40`, then `GotoMine`. The mine is not checked - any
/// object is possible as a "mine" (the command check lies with the command).
fn on_start_mining(sim: &mut Sim, id: ObjectId, env: &Envelope) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    let mine = ObjectId::from_i32(env.msg.arg(0));
    if let Some(g) = gather_mut(sim, id, INDEX) {
        g.resource_hid = mine;
    }
    set_status_flags(sim, id, MINING_BIT, true);
    goto_mine(sim, id);
}

/// `PropertyGoldMining_GotoMine` (`0x00516700`, slot 19): without a mine abort ("no mine"); otherwise
/// state 1, bit `0x40` and walk FINDTO `{mine, -1, 1}` (docking place at the building) with
/// `0x207`/`0x307`.
pub fn goto_mine(sim: &mut Sim, id: ObjectId) {
    let mine = resource(sim, id);
    if sim.object(mine).is_none() {
        log::debug!("Gold digger {id:?}: no mine");
        abort_module(sim, id, INDEX);
        return;
    }
    set_status_of(sim, id, status::TO_SOURCE);
    set_status_flags(sim, id, MINING_BIT, true);
    let env = Envelope {
        msg: Message::new(0x200, &[mine.as_i32(), -1, 1]),
        success: Some(Message::new(MINING.success, &[])),
        fault: Some(Message::new(MINING.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyGoldMining_MineStep` (`0x005168D0`, arrival and `0x707`). **Quirk:** state 300
/// is set **before** the check of the mine. Mine gone or dying -> abort. Foreign load
/// counts as 0. Slot free: `0x303 {1}` to the mine; gold -> load it; nothing -> `m_AnimCount + 1`
/// and at the `capacity + 1`-th empty step **one** unit is given for free and immediately "full"
/// (failed-attempt gift, quirk). Then schedule the next step (rate 33) and set the
/// mining animation `80007[0]` - at every step, also after a failed attempt. No space
/// left -> [`full`].
fn mine_step(sim: &mut Sim, id: ObjectId) {
    if status_of(sim, id) != state::MINING {
        set_status_of(sim, id, state::MINING);
    }
    let mine = resource(sim, id);
    let Some(m) = sim.object(mine) else {
        abort_module(sim, id, INDEX);
        return;
    };
    if m.status & DYING != 0 {
        abort_module(sim, id, INDEX);
        return;
    }
    let amount = own_load(sim, id, load::GOLD);
    let cap = super::capacity(sim, id);
    if cap.wrapping_sub(amount) <= 0 {
        full(sim, id);
        return;
    }
    let got = send(sim, mine, &Message::new(building::msg::GET_GOLD, &[1]));
    if got > 0 {
        set_carried_load(sim, id, amount.wrapping_add(got), load::GOLD);
    } else {
        let count = mining_mut(sim, id).map_or(0, |m| {
            m.anim_count = m.anim_count.wrapping_add(1);
            m.anim_count
        });
        if count > cap {
            set_carried_load(sim, id, amount.wrapping_add(1), load::GOLD);
            full(sim, id);
            return;
        }
    }
    let leak = mining(sim, id).map_or(0, |m| m.last_time_leak);
    let leak = super::schedule_next_unit(
        sim,
        id,
        INDEX,
        RATE,
        leak,
        Message::new(msg::MINE_STEP, &[]),
    );
    let anim = mining_mut(sim, id).map_or(0, |m| {
        m.last_time_leak = leak;
        list_at(&m.mining_anim, 0)
    });
    set_animation(sim, id, anim);
}

/// "Full" (`0x00516A0A`): `m_AnimCount = 0`; with at least two mining animations the state stays
/// 300, the exit animation `80007[1]` starts and after its time marker 1
/// (`GameObject_GetAnimLabelTime`) `0x607 {-1}` arrives to itself (-> `OnDeliverCommand` -> to the
/// gold store); otherwise immediately to the gold store (slot 20 with -1).
fn full(sim: &mut Sim, id: ObjectId) {
    let anims = mining_mut(sim, id).map_or_else(Vec::new, |m| {
        m.anim_count = 0;
        m.mining_anim.clone()
    });
    if anims.len() > 1 {
        set_status_of(sim, id, state::MINING);
        let exit = list_at(&anims, 1);
        set_animation(sim, id, exit);
        let t = super::anim_label_time(sim, id, exit, 1);
        post_delayed(sim, id, INDEX, t, Message::new(msg::DELIVER, &[-1]));
    } else {
        super::goto_delivery(sim, id, INDEX, ObjectId::NONE);
    }
}

/// `PropertyGoldMining_FindMine` (`0x00516AD0`, query `0x907` from `GameObject_ResumeNearestWork`
/// `0x77F`). Second call (`args[0] != -1`): post `0x107 {args[1]}` with abort (1 =
/// queued). First: nearest own mine (module 3 with `GOLDMINE`, also unfinished) within
/// **three tiles** (center distance) and any own finished gold store on the map
/// (edge distance; the result is discarded) -> `args = {edge distance mine->gold digger, mine}`, 1;
/// otherwise 0.
fn find_mine(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    if m.arg(0) != -1 {
        return i32::from(post_message(
            sim,
            id,
            Message::new(msg::START, &[m.arg(1)]),
            true,
        ));
    }
    let mut f = MatchFilter {
        mode: 0x112,
        match_bits: building::flags::GOLDMINE,
        ..MatchFilter::constructed()
    };
    let (pos, owner) = (position(sim, id), i32::from(id.slot()));
    let Some(mine) = find_nearest_matching(sim, pos, &f, 3 * TILE, owner, -1) else {
        return 0;
    };
    f.mode = 0x150;
    f.match_bits = MINING.deliver_class;
    let hard = sim.object(id).map_or(0, |o| o.hard_radius);
    if find_nearest_matching(sim, pos, &f, -1, owner, hard).is_none() {
        return 0;
    }
    let d = edge_distance_to_object(sim, mine, id);
    m.set_arg(0, d);
    m.set_arg(1, mine.as_i32());
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Posted;
    use crate::messaging::send_mut;
    use crate::module::gather::tests::{
        COLONIST, GOLD_DEPOSIT, GOLD_STORE, MINE, econ_rules, run_until, spawn, spawn_deposit,
        state as new_state,
    };
    use crate::module::resource;
    use crate::module::tick as module_tick;
    use crate::rules::Rules;

    fn load_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
        sim.object(id)
            .map_or((0, 0), |o| (o.load_amount, o.load_kind))
    }

    /// Arrival at the mine in tick T0 (walk reports `0x207` before the tick of gold mining).
    fn arrive(sim: &mut Sim, unit: ObjectId, mine: ObjectId) {
        if let Some(m) = mining_mut(sim, unit) {
            m.gather.resource_hid = mine;
            m.gather.status = status::TO_SOURCE;
        }
        set_status_flags(sim, unit, MINING_BIT, true);
        assert_eq!(send(sim, unit, &Message::new(msg::DEST_REACHED, &[])), 1);
        module_tick(sim, unit, INDEX, 100);
    }

    /// Ticks after T0 with a new load (T0 included if the arrival loaded), the tick of the
    /// exit animation ("full") and the tick in which the mining ends.
    fn mine_until_done(sim: &mut Sim, unit: ObjectId, max: i32) -> (Vec<i32>, Option<i32>, i32) {
        let mut units = Vec::new();
        if load_of(sim, unit).0 > 0 {
            units.push(0);
        }
        let mut last = load_of(sim, unit).0;
        let exit = mining(sim, unit).and_then(|m| m.mining_anim.get(1).copied());
        let mut full_at = None;
        for t in 1..=max {
            module_tick(sim, unit, INDEX, 100);
            let now = load_of(sim, unit).0;
            if now != last {
                units.push(t);
                last = now;
            }
            if full_at.is_none() && exit == Some(sim.object(unit).unwrap().anim.wanted) {
                full_at = Some(t);
            }
            if status_of(sim, unit) != state::MINING {
                return (units, full_at, t);
            }
        }
        panic!("mining does not end");
    }

    /// Gold rate (attribute 33) of the colonist.
    fn setup(rules: &mut Rules, rate: i32) {
        rules
            .defaults
            .get_mut(&COLONIST)
            .unwrap()
            .insert(RATE, rate);
    }

    #[test]
    fn a_fast_colonist_mines_at_once_alternates_eight_and_nine_ticks_and_leaves_after_the_label() {
        let mut rules = econ_rules();
        // Exit animation 17 with time marker 1 after 1500 ms (model datum, assumed here).
        rules.set_anim_labels(COLONIST, 17, &[(1, 1500)]);
        setup(&mut rules, 80);
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let mine = spawn(&mut sim, MINE, 0, (14, 10));
        let dep = spawn_deposit(&mut sim, GOLD_DEPOSIT, 5000, (14, 10));
        arrive(&mut sim, unit, mine);
        assert_eq!(status_of(&sim, unit), state::MINING);
        // The first unit comes immediately, the mining animation 16 at every step.
        assert_eq!(load_of(&sim, unit), (1, load::GOLD));
        assert_eq!(sim.object(unit).unwrap().anim.wanted, 16);
        let (units, full_at, done) = mine_until_done(&mut sim, unit, 400);
        assert_eq!(
            units,
            [
                0, 8, 17, 25, 34, 42, 51, 59, 68, 76, 85, 93, 102, 110, 119, 127, 136, 144, 153,
                161
            ]
        );
        assert_eq!(full_at, Some(170));
        // `0x607 {-1}` after 1500 ms: without a gold store abort with a full load.
        assert_eq!(done, 186);
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(load_of(&sim, unit), (20, load::GOLD));
        assert_eq!(sim.attribute(dep, resource::GOLD_AMOUNT), 4980);
        let m = mining(&sim, unit).unwrap();
        assert_eq!((m.anim_count, m.last_time_leak), (0, 0));
    }

    /// Mining at rate 60 from T0 to the end (deposit with amount `deposit` under the mine or
    /// none, initial load `load`); returns: withdrawal ticks, end, load, message flag of the
    /// mine, `m_AnimCount`.
    fn mine_at_rate_60(
        deposit: Option<i32>,
        load: i32,
    ) -> (Vec<i32>, i32, (i32, i32), Option<bool>, i32) {
        let mut rules = econ_rules();
        setup(&mut rules, 60);
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let mine = spawn(&mut sim, MINE, 0, (14, 10));
        if let Some(n) = deposit {
            spawn_deposit(&mut sim, GOLD_DEPOSIT, n, (14, 10));
        }
        if load > 0 {
            set_carried_load(&mut sim, unit, load, load::GOLD);
        }
        arrive(&mut sim, unit, mine);
        let (units, _, done) = mine_until_done(&mut sim, unit, 1000);
        let announced = building::building(&sim, mine).map(|b| b.no_resources_announced);
        let count = mining(&sim, unit).unwrap().anim_count;
        (units, done, load_of(&sim, unit), announced, count)
    }

    // QUIRK W-44: kept
    #[test]
    fn an_empty_deposit_gives_one_unit_after_twenty_one_empty_steps() {
        let (units, done, load, announced, count) = mine_at_rate_60(Some(0), 0);
        // 21 steps in ticks 0, 11, ..., 220; only in the 21st load 1 and immediately "full" (without
        // exit time: `0x607` in the next tick).
        assert_eq!(units, [220]);
        assert_eq!(done, 221);
        assert_eq!(load, (1, load::GOLD));
        assert_eq!(announced, Some(true));
        assert_eq!(count, 0);
    }

    // QUIRK W-45: kept
    #[test]
    fn a_mine_without_a_deposit_gives_one_unit_per_step_forever() {
        let (units, done, load, announced, _) = mine_at_rate_60(None, 0);
        assert_eq!(units, (0..20).map(|k| 11 * k).collect::<Vec<_>>());
        // "Full" in the 21st step (tick 220), then `0x607`.
        assert_eq!(done, 221);
        assert_eq!(load, (20, load::GOLD));
        assert_eq!(announced, Some(false));
    }

    #[test]
    fn a_deposit_with_seven_left_gives_seven_units_and_one_present_after_twenty_one_misses() {
        let (units, done, load, _, _) = mine_at_rate_60(Some(7), 0);
        let mut want: Vec<i32> = (0..7).map(|k| 11 * k).collect();
        want.push(297);
        assert_eq!(units, want);
        assert_eq!(done, 298);
        assert_eq!(load, (8, load::GOLD));
    }

    #[test]
    fn the_miss_counter_survives_an_abort_and_a_full_load_skips_the_mine() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let mine = spawn(&mut sim, MINE, 0, (14, 10));
        spawn_deposit(&mut sim, GOLD_DEPOSIT, 0, (14, 10));
        arrive(&mut sim, unit, mine);
        assert_eq!(mining(&sim, unit).unwrap().anim_count, 1);
        assert!(crate::module::abort(&mut sim, unit, INDEX));
        let m = mining(&sim, unit).unwrap();
        assert_eq!((m.anim_count, m.gather.status), (1, status::IDLE));
        assert_eq!(sim.object(unit).unwrap().status & MINING_BIT, 0);
        // Full load on arrival: no `0x303`, immediately "full" - with exit animation.
        set_carried_load(&mut sim, unit, 20, load::GOLD);
        arrive(&mut sim, unit, mine);
        let m = mining(&sim, unit).unwrap();
        assert_eq!((m.anim_count, m.gather.status), (0, state::MINING));
        assert_eq!(sim.object(unit).unwrap().anim.wanted, 17);
        // The timer `0x607 {-1}` (without a time marker: delay 0) fires only in tick T1.
        let base = &sim.module(unit, INDEX).unwrap().base;
        assert_eq!(base.timers_ws.len() + base.events.len(), 1);
        module_tick(&mut sim, unit, INDEX, 100);
        assert_eq!(status_of(&sim, unit), status::IDLE);
    }

    #[test]
    fn the_step_sets_state_300_before_it_checks_the_mine() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        let fault = Message::new(0xA7F, &[55]);
        sim.module_mut(unit, INDEX)
            .unwrap()
            .base
            .set_fault(Some(&fault), ObjectId::NONE);
        if let Some(m) = mining_mut(&mut sim, unit) {
            m.gather.resource_hid = ObjectId(0x7_0001);
        }
        assert_eq!(send(&mut sim, unit, &Message::new(msg::MINE_STEP, &[])), 1);
        // No mine: abort (state 0, fault message out).
        assert_eq!(status_of(&sim, unit), status::IDLE);
        assert_eq!(sim.object(unit).unwrap().carrier, ObjectId(55));
    }

    #[test]
    fn loaded_miners_answer_the_stand_and_walk_animation_queries() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_STAND_ANIM, &[3])),
            7
        );
        assert_eq!(
            send(&mut sim, unit, &Message::new(msg::GET_WALK_ANIM, &[])),
            6
        );
        // The stand idle asks with gold `0x407` (answer from 80005), without a load the list.
        assert_eq!(crate::module::stand::select_idle_anim(&mut sim, unit, 3), 0);
        set_carried_load(&mut sim, unit, 3, load::GOLD);
        assert_eq!(crate::module::stand::select_idle_anim(&mut sim, unit, 3), 7);
        let m = mining(&sim, unit).unwrap();
        assert_eq!(
            (m.walk_anim, &m.stand_anim, &m.mining_anim),
            (6, &vec![7], &vec![16, 17])
        );
        assert_eq!(m.gather.deliver_anim, -1);
        assert_eq!(sim.module(unit, INDEX).unwrap().base.flags, 0);
        assert_eq!(send(&mut sim, unit, &Message::new(0xC07, &[])), -1);
    }

    #[test]
    fn find_mine_needs_a_mine_within_three_tiles_and_any_finished_gold_store() {
        // (mine tile, gold store present) -> answer.
        for (tile, store, expect) in [
            ((13, 10), true, true),
            ((13, 10), false, false),
            ((14, 10), true, false),
        ] {
            let rules = econ_rules();
            let mut state = new_state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 10));
            let mine = spawn(&mut sim, MINE, 0, tile);
            // Foreign mines and gold stores do not count.
            spawn(&mut sim, MINE, 1, (11, 11));
            spawn(&mut sim, GOLD_STORE, 1, (30, 30));
            if store {
                spawn(&mut sim, GOLD_STORE, 0, (40, 40));
            }
            let mut q = Message::new(msg::FIND_MINE, &[-1, 0]);
            let r = send_mut(&mut sim, unit, &mut q);
            if expect {
                let d = edge_distance_to_object(&mut sim, mine, unit);
                assert!(d > 0);
                assert_eq!((r, q.args.clone()), (1, vec![d, mine.as_i32()]));
            } else {
                assert_eq!((r, q.args.clone()), (0, vec![-1, 0]), "{tile:?} {store}");
            }
            // Second call: start queued with abort.
            let mut q = Message::new(msg::FIND_MINE, &[5, mine.as_i32()]);
            assert_eq!(send_mut(&mut sim, unit, &mut q), 1);
            let fifo: Vec<Posted> = sim.object(unit).unwrap().fifo.iter().cloned().collect();
            assert_eq!(
                fifo,
                vec![Posted::Message {
                    msg: Message::new(msg::START, &[mine.as_i32()]),
                    abort: true
                }]
            );
        }
    }

    #[test]
    fn a_gold_digger_mines_delivers_to_the_gold_store_and_returns() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let (unit, mine, dep) = {
            let mut sim = Sim::new(&mut state, &rules);
            let unit = spawn(&mut sim, COLONIST, 0, (10, 16));
            let mine = spawn(&mut sim, MINE, 0, (16, 16));
            let dep = spawn_deposit(&mut sim, GOLD_DEPOSIT, 5000, (16, 16));
            spawn(&mut sim, GOLD_STORE, 0, (4, 16));
            post_message(
                &mut sim,
                unit,
                Message::new(msg::START, &[mine.as_i32()]),
                true,
            );
            (unit, mine, dep)
        };
        let gold0 = state.players[0].resource_raw(0);
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == state::MINING
        });
        run_until(&mut state, &rules, 2000, |sim| {
            status_of(sim, unit) == status::TO_DELIVERY
        });
        {
            let sim = Sim::new(&mut state, &rules);
            assert_eq!(load_of(&sim, unit), (20, load::GOLD));
            assert_eq!(sim.attribute(dep, resource::GOLD_AMOUNT), 4980);
        }
        run_until(&mut state, &rules, 2000, |sim| {
            sim.state.players[0].resource_raw(0) != gold0
        });
        assert_eq!(state.players[0].resource_raw(0), gold0 + (20 << 12));
        // Back to the mine.
        let sim = Sim::new(&mut state, &rules);
        let g = gather(&sim, unit, INDEX).unwrap();
        assert_eq!((g.status, g.resource_hid), (status::TO_SOURCE, mine));
        assert_eq!(load_of(&sim, unit), (0, load::NONE));
    }
}
