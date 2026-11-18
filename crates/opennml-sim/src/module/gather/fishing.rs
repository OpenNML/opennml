//! Module 55 `CPropertyFishing` (vtable `0x00623D20`, re/gathering): fishing.
//!
//! Flow like gold mining: `0x137 {basket}` -> by FINDTO to the fishing basket, state 400 and per
//! step (`60000 / rate` ms with leak, rate = attribute 31) one unit of food directly from the basket
//! (`0x206`); the first immediately on arrival, "full" in the step after the last unit, then
//! without exit animation to the harbor. No basket search, no `0x77F` finder. **Quirk:** the
//! state bit is `0x40` (MINING) like in gold mining.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::events::update_events;
use crate::module::resource;
use crate::module::stand::{list_modulo, parse_int_list};
use crate::module::{ModuleKind, abort as abort_module};
use crate::sim::Sim;

use super::{
    FISHING, Gather, gather, gather_mut, list_at, load, own_load, set_animation, set_carried_load,
    status,
};

/// Module index.
pub const INDEX: usize = 55;
/// Food rate (attribute 31, units per minute; maximum with upgrades).
pub const RATE: u32 = 31;
/// Object state bit of fishing (**quirk:** `0x40` MINING, `+0x90`).
pub const FISHING_BIT: u32 = 0x40;
/// Object state bit "dying": the basket accepts no more work.
const DYING: u32 = 0x4_0000;
/// Radius argument of the FINDTO message (`2·DAT_00672868`); has no effect with three arguments.
const UNUSED_RADIUS: i32 = 2 * 0x1000;

/// Messages of fishing.
pub mod msg {
    /// `{basket}`: fish.
    pub const START: u32 = 0x137;
    /// Walk: destination reached.
    pub const DEST_REACHED: u32 = 0x237;
    /// Walk: aborted.
    pub const ABORT: u32 = 0x337;
    /// `{sel}` -> stand animation with fish (no sender, only in the handler).
    pub const GET_STAND_ANIM: u32 = 0x437;
    /// -> walk animation with fish (no sender, only in the handler).
    pub const GET_WALK_ANIM: u32 = 0x537;
    /// `{harbor}`: deliver.
    pub const DELIVER: u32 = 0x637;
    /// Timer: catch step.
    pub const FISH_STEP: u32 = 0x737;
    /// Timer: delivery finished.
    pub const DELIVERED: u32 = 0x837;
}

/// Work state (`m_Status`) of fishing.
pub mod state {
    /// Fishes at the basket.
    pub const FISHING: i32 = 400;
}

/// State of `CPropertyFishing` (`PropertyFishing_Construct` `0x005149E0`), saved like
/// `PropertyFishing_SerializeFields` (`0x00515000`) after the base fields; plus the unused
/// list 80004 (`+0xC0`, not saved in the original - saved here as well, without effect). The
/// flags 80001 (`m_MatchInt`) are in the module base; `m_DeliverAnim` stays -1.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fishing {
    pub gather: Gather,
    /// `m_FishingWalkAnim` (`+0x9C`, 80002).
    pub walk_anim: Vec<i32>,
    /// `m_FishingStandAnim` (`+0xA8`, 80003).
    pub stand_anim: Vec<i32>,
    /// `m_FishingAnim` (`+0xB4`, 80005): `[0]` set at every step.
    pub fishing_anim: Vec<i32>,
    /// List without a name (`+0xC0`, 80004): read, never used.
    pub unused_anim: Vec<i32>,
    /// `m_AnimCount` (`+0xCC`): empty steps since the last full load.
    pub anim_count: i32,
    /// `m_lastTimeLeak` (`+0xD0`): time remainder of the catch steps, never reset.
    pub last_time_leak: i32,
}

pub(crate) fn fishing<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Fishing> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Fishing(f) => Some(f),
        _ => None,
    }
}

pub(crate) fn fishing_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Fishing> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Fishing(f) => Some(f),
        _ => None,
    }
}

fn status_of(sim: &Sim, id: ObjectId) -> i32 {
    super::state(sim, id, INDEX)
}

fn set_status_of(sim: &mut Sim, id: ObjectId, s: i32) {
    super::set_state(sim, id, INDEX, s);
}

fn resource_of(sim: &Sim, id: ObjectId) -> ObjectId {
    gather(sim, id, INDEX).map_or(ObjectId::NONE, |g| g.resource_hid)
}

fn set_status_flags(sim: &mut Sim, id: ObjectId, bits: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(bits, on);
    }
}

/// `PropertyFishing_Init` (`0x00514F30`): flags 80001 (`m_MatchInt`), walk 80002, stand 80003
/// and catch animations 80005, the unused list 80004 (all lists).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let flags = crate::module::init_flags(INDEX, desc);
    let (walk, stand, fish, unused) = (text(80002), text(80003), text(80005), text(80004));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(f) = fishing_mut(sim, id) {
        f.walk_anim = parse_int_list(&walk, &f.walk_anim);
        f.stand_anim = parse_int_list(&stand, &f.stand_anim);
        f.fishing_anim = parse_int_list(&fish, &f.fishing_anim);
        f.unused_anim = parse_int_list(&unused, &f.unused_anim);
    }
}

/// `PropertyFishing_Query` (`0x00514ED0`, slot 7); all other codes go via the
/// base query to [`on_message`].
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        // `IntList_GetModulo`: empty list (division by 0 in the original) -> -1 (exception A7).
        msg::GET_STAND_ANIM => {
            fishing(sim, id).map_or(-1, |f| list_modulo(&f.stand_anim, m.arg(0)))
        }
        msg::GET_WALK_ANIM => fishing(sim, id).map_or(-1, |f| list_at(&f.walk_anim, 0)),
        msg::FISH_STEP => {
            fish_step(sim, id);
            1
        }
        _ => super::base_query(sim, id, INDEX, m),
    }
}

/// `PropertyFishing_OnMessage` (`0x00514E90`, slot 12): `0x137` starts, everything else the base
/// (`0x237` destination reached, `0x337` abort, `0x637` deliver, `0x837` delivery finished).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    if env.msg.code == msg::START {
        on_start_fishing(sim, id, env);
        return true;
    }
    super::on_command(sim, id, INDEX, env)
}

/// `PropertyFishing_Tick` (`0x00514B40`, slot 13): except while fishing (state 400) the
/// base tick, then the timers.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    if status_of(sim, id) != state::FISHING {
        super::tick_base(sim, id, INDEX);
    }
    update_events(sim, id, INDEX, dt);
}

/// `PropertyFishing_Abort` (`0x00514B70`, slot 11): only the base abort.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    super::stop(sim, id, INDEX)
}

/// `PropertyFishing_OnDestReached` (`0x00514EB0`, slot 18): in state 1 arrived at the basket ->
/// first catch step, otherwise the base (state 2 -> deliver).
pub fn on_dest_reached(sim: &mut Sim, id: ObjectId, _env: &Envelope) -> bool {
    if status_of(sim, id) == status::TO_SOURCE {
        fish_step(sim, id);
        return true;
    }
    super::base_on_dest_reached(sim, id, INDEX)
}

/// `PropertyFishing_OnStartFishing` (`0x00514CC0`, command `0x137 {basket}`): take over the feedback
/// messages, `m_ResourceHID`, bit `0x40`, then `GotoBaskets`.
fn on_start_fishing(sim: &mut Sim, id: ObjectId, env: &Envelope) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
        m.base.set_success(env.success.as_ref(), env.success_target);
    }
    if let Some(g) = gather_mut(sim, id, INDEX) {
        g.resource_hid = ObjectId::from_i32(env.msg.arg(0));
    }
    set_status_flags(sim, id, FISHING_BIT, true);
    goto_baskets(sim, id);
}

/// `PropertyFishing_GotoBaskets` (`0x00514B80`, slot 19): without a basket abort; otherwise state 1,
/// bit `0x40` and walk FINDTO `{basket, 8192, 1}` (with three arguments the radius argument
/// has no effect) with `0x237`/`0x337`. The boat travels with the flags of its walk module.
pub fn goto_baskets(sim: &mut Sim, id: ObjectId) {
    let basket = resource_of(sim, id);
    if sim.object(basket).is_none() {
        abort_module(sim, id, INDEX);
        return;
    }
    set_status_of(sim, id, status::TO_SOURCE);
    set_status_flags(sim, id, FISHING_BIT, true);
    let env = Envelope {
        msg: Message::new(0x200, &[basket.as_i32(), UNUSED_RADIUS, 1]),
        success: Some(Message::new(FISHING.success, &[])),
        fault: Some(Message::new(FISHING.fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyFishing_FishStep` (`0x00514D20`, arrival and `0x737`), like the mining step of
/// gold mining: **quirk:** state 400 before the check of the basket; basket gone or dying ->
/// abort; foreign load counts as 0; room free -> `0x206 {1}` directly to the basket; nothing ->
/// `m_AnimCount + 1`, at the `capacity + 1`-th empty step a unit is given as a gift and "full";
/// otherwise next step (rate 31) and the catch animation `80005[0]` at every step. Full ->
/// `m_AnimCount = 0` and immediately to the harbor (slot 20 with -1, no exit animation).
fn fish_step(sim: &mut Sim, id: ObjectId) {
    if status_of(sim, id) != state::FISHING {
        set_status_of(sim, id, state::FISHING);
    }
    let basket = resource_of(sim, id);
    let Some(b) = sim.object(basket) else {
        abort_module(sim, id, INDEX);
        return;
    };
    if b.status & DYING != 0 {
        abort_module(sim, id, INDEX);
        return;
    }
    let amount = own_load(sim, id, load::FOOD);
    let cap = super::capacity(sim, id);
    if cap.wrapping_sub(amount) <= 0 {
        full(sim, id);
        return;
    }
    let got = send(sim, basket, &Message::new(resource::msg::TAKE_FOOD, &[1]));
    if got > 0 {
        set_carried_load(sim, id, amount.wrapping_add(got), load::FOOD);
    } else {
        let count = fishing_mut(sim, id).map_or(0, |f| {
            f.anim_count = f.anim_count.wrapping_add(1);
            f.anim_count
        });
        if count > cap {
            set_carried_load(sim, id, amount.wrapping_add(1), load::FOOD);
            full(sim, id);
            return;
        }
    }
    let leak = fishing(sim, id).map_or(0, |f| f.last_time_leak);
    let leak = super::schedule_next_unit(
        sim,
        id,
        INDEX,
        RATE,
        leak,
        Message::new(msg::FISH_STEP, &[]),
    );
    let anim = fishing_mut(sim, id).map_or(0, |f| {
        f.last_time_leak = leak;
        list_at(&f.fishing_anim, 0)
    });
    set_animation(sim, id, anim);
}

/// "Full": `m_AnimCount = 0`, to the harbor.
fn full(sim: &mut Sim, id: ObjectId) {
    if let Some(f) = fishing_mut(sim, id) {
        f.anim_count = 0;
    }
    super::goto_delivery(sim, id, INDEX, ObjectId::NONE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::gather::tests::{
        BASKET, BOAT, ENG_BOAT, HARBOR, econ_rules, spawn, spawn_deposit, state as new_state,
    };
    use crate::module::tick as module_tick;
    use crate::state::SimEvent;

    fn load_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
        sim.object(id)
            .map_or((0, 0), |o| (o.load_amount, o.load_kind))
    }

    /// Boat `guid` at the basket (amount `amount`) with load `start`; arrival in tick T0, then until the
    /// end of fishing. Return value: withdrawal ticks, end, load.
    fn fish(guid: u32, amount: i32, start: i32, harbor: bool) -> (Vec<i32>, i32, (i32, i32)) {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let boat = spawn(&mut sim, guid, 0, (10, 10));
        let basket = spawn_deposit(&mut sim, BASKET, amount, (14, 10));
        if harbor {
            spawn(&mut sim, HARBOR, 0, (30, 10));
        }
        if start > 0 {
            set_carried_load(&mut sim, boat, start, load::FOOD);
        }
        assert_eq!(
            send(
                &mut sim,
                boat,
                &Message::new(msg::START, &[basket.as_i32()])
            ),
            1
        );
        assert_eq!(status_of(&sim, boat), status::TO_SOURCE);
        assert_eq!(sim.object(boat).unwrap().status & 0x40, 0x40);
        let w = crate::module::walk::walk(&sim, boat).unwrap();
        assert_eq!(
            w.find_to,
            Some(Message::new(0x200, &[basket.as_i32(), 8192, 1]))
        );
        // Arrival in tick T0.
        sim.state.events.clear();
        assert_eq!(
            send(&mut sim, boat, &Message::new(msg::DEST_REACHED, &[])),
            1
        );
        assert!(
            sim.state
                .events
                .contains(&SimEvent::Revealed { object: basket })
        );
        module_tick(&mut sim, boat, INDEX, 100);
        let mut units = Vec::new();
        if load_of(&sim, boat).0 > start {
            units.push(0);
        }
        let mut last = load_of(&sim, boat).0;
        for t in 1..=20_000 {
            module_tick(&mut sim, boat, INDEX, 100);
            let now = load_of(&sim, boat).0;
            if now != last {
                units.push(t);
                last = now;
            }
            if status_of(&sim, boat) != state::FISHING {
                return (units, t, load_of(&sim, boat));
            }
        }
        panic!("fishing does not end");
    }

    // QUIRK W-23: kept
    #[test]
    fn a_spanish_boat_fishes_at_once_every_eight_or_nine_ticks_up_to_300() {
        let (units, done, load) = fish(BOAT, 2000, 296, false);
        // Four units (tick 0, 8, 17, 25), "full" in the step after (tick 34): without a harbor
        // abort with a full load.
        assert_eq!(units, [0, 8, 17, 25]);
        assert_eq!(done, 34);
        assert_eq!(load, (300, load::FOOD));
    }

    #[test]
    fn an_english_boat_fishes_every_21_ticks_and_heads_for_the_harbor() {
        let (units, done, load) = fish(ENG_BOAT, 2000, 497, true);
        assert_eq!(units, [0, 21, 42]);
        assert_eq!(done, 63);
        assert_eq!(load, (500, load::FOOD));
    }

    // QUIRK W-44: kept
    #[test]
    fn an_empty_basket_gives_one_fish_after_capacity_plus_one_misses() {
        let (units, done, load) = fish(ENG_BOAT, 0, 0, false);
        // 501 empty steps 21 ticks apart, in the last one the gift and "full".
        assert_eq!(units, [500 * 21]);
        assert_eq!(done, 500 * 21);
        assert_eq!(load, (1, load::FOOD));
    }

    // QUIRK W-54: kept
    #[test]
    fn fishing_reads_80001_as_flags_and_answers_its_animation_queries() {
        let rules = econ_rules();
        let mut state = new_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let boat = spawn(&mut sim, BOAT, 0, (10, 10));
        let f = fishing(&sim, boat).unwrap();
        assert_eq!(
            (&f.walk_anim, &f.stand_anim, &f.fishing_anim, &f.unused_anim),
            (&vec![2, 3], &vec![4, 5], &vec![6, 7], &vec![9])
        );
        assert_eq!(f.gather.deliver_anim, -1);
        assert_eq!(
            send(&mut sim, boat, &Message::new(msg::GET_STAND_ANIM, &[3])),
            5
        );
        assert_eq!(
            send(&mut sim, boat, &Message::new(msg::GET_WALK_ANIM, &[])),
            2
        );
        // Without a basket: abort.
        assert_eq!(send(&mut sim, boat, &Message::new(msg::START, &[77])), 1);
        assert_eq!(status_of(&sim, boat), status::IDLE);
    }
}
