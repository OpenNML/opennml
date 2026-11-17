//! Tests of the healing module (re/healing).

use super::*;
use crate::message::{Envelope, Message, MessageFilter};
use crate::messaging::{post_command, post_message, send};
use crate::module::ability_testkit as tk;
use crate::module::walk::find_to_msg;
use crate::object::status;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::world::{ObjectDesc, create_object};

/// Type with only module 29 (flags `0x3400`, setup 7, teardown `7`, dying 2, standing `0`, heal animation 7,
/// 288 HP per minute) and the HEF value `hef`.
fn hef_type(guid: u32, hef: &str) -> String {
    format!(
        "GUID {guid}\nNAME TEST_Heiler\nVALUEID 100 6 1 33554432\nPROPID 29\n\
         VALUEID 300001 1 1 7\nVALUEID 300002 6 1 13312\nVALUEID 300003 6 1 7\n\
         VALUEID 300004 6 1 2\nVALUEID 300005 1 1 0\nVALUEID 300006 6 1 7\n\
         VALUEID 300007 6 1 288\nVALUEID 300008 7 1 {hef}\n"
    )
}

#[test]
fn read_params_zeroes_mana_and_builds_the_filter() {
    let rules = tk::rules_from(&(tk::types() + &hef_type(9001, "0")));
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Pilgrim: mana full after `InitAttributes`, `ReadParams` sets it to 0 (C-241).
    let pilgrim = tk::spawn(&mut sim, tk::PILGRIM, 0, (10, 10));
    let attrs = &sim.object(pilgrim).unwrap().attributes;
    assert_eq!(attrs.current_fraction(15), 0);
    assert_eq!(attrs.regen_rate(15), 1, "rate 1 from InitAttributes stays");
    let h = healing(&sim, pilgrim).expect("module 29");
    assert_eq!((h.establish_anim, h.die_anim, h.heal_anim), (7, 2, 7));
    assert_eq!(h.remove_anim, [7]);
    assert_eq!(h.stand_anim, [0]);
    assert_eq!(h.healing_points, 288);
    assert_eq!(h.hef, "data/effects/healing_circle.hef");
    assert!(!h.ready);
    assert_eq!((h.sick, h.new_target), (ObjectId(0), ObjectId(0)));
    assert_eq!(sim.module(pilgrim, INDEX).unwrap().base.flags, 0x3400);
    let f = match_filter(0x3400);
    assert_eq!((f.mode, f.prop_id), (0x80_0114, 0));
    // Shipyard: the GUID list is in `remove_anim`.
    let yard = tk::spawn(&mut sim, tk::SHIPYARD, 0, (20, 10));
    let h = healing(&sim, yard).unwrap();
    assert_eq!(h.remove_anim, [1108, 1109, 1117]);
    assert_eq!(h.hef, "data/effects/healing_circle.hef");
    assert_eq!(sim.module(yard, INDEX).unwrap().base.flags, 0x1700);
    // 300008 = "0" -> empty HEF name.
    let none = tk::spawn(&mut sim, 9001, 0, (30, 10));
    assert_eq!(healing(&sim, none).unwrap().hef, "");
}

#[test]
fn the_filter_follows_the_flag_word() {
    let expect = |mode: u32, prop_id: u32, match_bits: u32| MatchFilter {
        mode,
        prop_id,
        match_bits,
        ..MatchFilter::constructed()
    };
    // Shipped data: healer `0x3400`, monastery `0x1500`, shipyard `0x1700` (MACHINES and PTTOPT).
    for flags in [0x3400, 0x1500, 0x1700] {
        assert_eq!(match_filter(flags), expect(0x80_0114, 0, 0), "{flags:#x}");
    }
    // Only single healing: no machines (`0x4000`), walk bit `0x20000`.
    assert_eq!(match_filter(flags::SINGLE), expect(0x80_4114, 0, 0x2_0000));
    // Only machines: `0x2`.
    assert_eq!(
        match_filter(flags::MACHINES),
        expect(0x80_0116, 0, 0x2_0000)
    );
    // Buildings: property 3.
    assert_eq!(
        match_filter(flags::BUILDING),
        expect(0x80_4114, 3, 0x2_0000)
    );
    // PTTOPT and BUILDING: chain with the building filter (any building, OR).
    let chained = match_filter(flags::PTTOPT | flags::BUILDING);
    assert_eq!(
        MatchFilter {
            next: None,
            ..chained.clone()
        },
        expect(0x2080_4114, 0, 0x2_0000)
    );
    assert_eq!(chained.next.as_deref(), Some(&expect(0x4004, 3, 0x2_0000)));
    let chained = match_filter(flags::PTTOPT | flags::BUILDING | flags::MACHINES);
    assert_eq!(chained.mode, 0x2080_0114);
    assert_eq!(chained.next.as_deref(), Some(&expect(4, 3, 0)));
    // `NOHEALINGANIM` and `ONLYGUIDS` do not change the filter.
    assert_eq!(
        match_filter(flags::NOHEALINGANIM | flags::ONLYGUIDS | flags::SINGLE),
        match_filter(flags::SINGLE)
    );
}

// --- Healing trance, tick and hooks (re/healing 2.1, 3.1-3.5, 4, 8 no. 7-10, 9.4) ---

/// Attack and walk module (out of the tick list in the trance).
const ATTACK: usize = 17;
const WALK: usize = 0;

/// Building with module 3, optionally quarters 23, and module 29 like the monastery (`0x1500`, setup 15,
/// standing `0`, 180 HP per minute) with the HEF value `hef`.
fn house_type(guid: u32, hef: &str, quarter: bool) -> String {
    let quarter = if quarter {
        "PROPID 23\nVALUEID 240001 6 1 0\n"
    } else {
        ""
    };
    format!(
        "GUID {guid}\nNAME TEST_Heilhaus\nVALUEID 100 6 1 16777216\nPROPID 3\nVALUEID 40003 6 1 0\n\
         {quarter}PROPID 29\nVALUEID 300001 1 1 0\nVALUEID 300002 6 1 5376\nVALUEID 300003 6 1 15\n\
         VALUEID 300004 6 1 0\nVALUEID 300005 1 1 0\nVALUEID 300006 6 1 0\n\
         VALUEID 300007 6 1 180\nVALUEID 300008 7 1 {hef}\n"
    )
}

/// Healer with stand (list `1`) and the trance stand list `4 6`.
const STANDER: u32 = 9002;
/// Healing house with quarters and HEF name or without.
const HOUSE: u32 = 9003;
const HOUSE_WITHOUT_HEF: u32 = 9004;
/// Healing building like the monastery without HEF name (300008 = `"0"`).
const MONASTERY_WITHOUT_HEF: u32 = 9005;

/// Test world rules with the additional types of this file; the buildings have the hull, construction time and
/// hit points of the monastery and two quarters places (config 23).
fn rules() -> Rules {
    let stander = format!(
        "GUID {STANDER}\nNAME TEST_Steher\nVALUEID 100 6 1 33554432\nPROPID 2\nVALUEID 30001 1 1 1\n\
         PROPID 29\nVALUEID 300001 1 1 7\nVALUEID 300002 6 1 13312\nVALUEID 300003 6 1 7\n\
         VALUEID 300004 6 1 2\nVALUEID 300005 1 1 4 6\nVALUEID 300006 6 1 7\n\
         VALUEID 300007 6 1 288\nVALUEID 300008 7 1 0\n"
    );
    let types = [
        tk::types(),
        stander,
        house_type(HOUSE, "data/effects/healing_circle.hef", true),
        house_type(HOUSE_WITHOUT_HEF, "0", true),
        house_type(MONASTERY_WITHOUT_HEF, "0", false),
    ]
    .concat();
    let mut r = tk::rules_from(&types);
    let shape = r.building_shapes[&tk::MONASTERY].clone();
    let model = r.model_boxes[&tk::MONASTERY];
    for guid in [HOUSE, HOUSE_WITHOUT_HEF, MONASTERY_WITHOUT_HEF] {
        r.defaults
            .insert(guid, [(13, 50), (14, 3000), (18, 40), (23, 2)].into());
        r.building_shapes.insert(guid, shape.clone());
        r.model_boxes.insert(guid, model);
    }
    r
}

fn say(sim: &mut Sim, id: ObjectId, code: u32) -> i32 {
    send(sim, id, &Message::new(code, &[]))
}

fn st(sim: &Sim, id: ObjectId) -> u32 {
    sim.object(id).unwrap().status
}

fn filter(sim: &Sim, id: ObjectId) -> MessageFilter {
    sim.object(id).unwrap().filter.clone()
}

fn ready(sim: &Sim, id: ObjectId) -> bool {
    healing(sim, id).unwrap().ready
}

fn wanted_anim(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).unwrap().anim.wanted
}

/// Module `index` switched on (after the queued tick list changes have been applied).
fn on(sim: &Sim, id: ObjectId, index: usize) -> bool {
    sim.object(id).unwrap().has_module(index, false)
}

/// Pending timers `code` of the healing module (list and `m_TimersWS`).
fn timers(sim: &Sim, id: ObjectId, code: u32) -> usize {
    let b = &sim.module(id, INDEX).unwrap().base;
    b.events
        .iter()
        .chain(b.timers_ws.iter())
        .filter(|e| e.msg.code == code)
        .count()
}

fn trance_filter() -> MessageFilter {
    MessageFilter {
        mode: 0,
        codes: vec![msg::END_REQUEST],
    }
}

fn closed_filter() -> MessageFilter {
    MessageFilter {
        mode: 0,
        codes: Vec::new(),
    }
}

/// Walk order as a posted command (like `0x04`).
fn walk_order() -> Envelope {
    Envelope::bare(find_to_msg(
        30 << 12,
        20 << 12,
        0,
        crate::motion::NO_DIRECTION,
        0,
        false,
        0x201,
    ))
}

/// Read the savegame and check it against the rules (like the viewer when loading).
fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

/// Construction site (not preplaced) of slot 0 at the center of the tile `tile`.
fn site(sim: &mut Sim, guid: u32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, 0, [tk::world(tile.0), 0.0, tk::world(tile.1)]);
    create_object(sim, &mut desc).expect("construction site created")
}

/// Complete a construction site with one build step (`0x203`, speed far above the remaining work).
fn finish(sim: &mut Sim, id: ObjectId) {
    let mut m = Message::new(crate::module::building::msg::BUILD_TICK, &[1_000_000, 0]);
    crate::messaging::send_mut(sim, id, &mut m);
    assert!(crate::filter::matches_class(sim, id, 0x403), "finished");
}

/// Trance of a healer of type `guid` (setup/teardown animation, duration in ticks, re/healing 9.4).
fn trance_timeline(guid: u32, establish: i32, remove: i32, setup: usize, teardown: usize) {
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, guid, 0, (20, 20));
    tk::run_ticks(&mut sim, 1);
    assert!(on(&sim, h, WALK) && on(&sim, h, ATTACK));
    // `0x11D` before tick N: immediately `HEAL`, lock bit, setup animation and filter `{0, [0x21D]}`.
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    assert_eq!(st(&sim, h) & status::HEAL, status::HEAL);
    assert_eq!(st(&sim, h) & status::ANIM_LOCKED, status::ANIM_LOCKED);
    assert_eq!(wanted_anim(&sim, h), establish);
    assert_eq!(filter(&sim, h), trance_filter());
    assert!(!ready(&sim, h));
    tk::run_ticks(&mut sim, 1); // N
    assert!(
        !on(&sim, h, WALK) && !on(&sim, h, ATTACK),
        "{guid}: walk/attack off"
    );
    assert_eq!(sim.object(h).unwrap().anim.current, establish);
    // `0x31D` after marker 1: N + floor(t/100) + 1.
    tk::run_ticks(&mut sim, setup - 1); // N + 1 ... N + setup - 1
    assert!(!ready(&sim, h), "{guid}: not ready before N + {setup}");
    assert_ne!(st(&sim, h) & status::ANIM_LOCKED, 0);
    tk::run_ticks(&mut sim, 1); // N + setup
    assert!(ready(&sim, h), "{guid}: ready in N + {setup}");
    assert_eq!(st(&sim, h) & status::ANIM_LOCKED, 0);
    assert_eq!(st(&sim, h) & status::HEAL, status::HEAL);
    assert_eq!(filter(&sim, h), trance_filter());
    assert!(!on(&sim, h, WALK) && !on(&sim, h, ATTACK));
    tk::run_ticks(&mut sim, 5);
    // `0x21D` before tick M: no longer ready, teardown animation, filter empty in mode 0 (K-53).
    assert_eq!(say(&mut sim, h, msg::END_REQUEST), -1);
    assert!(!ready(&sim, h));
    assert_eq!(wanted_anim(&sim, h), remove);
    assert_eq!(filter(&sim, h), closed_filter());
    assert_eq!(
        st(&sim, h) & (status::HEAL | status::ANIM_LOCKED),
        status::HEAL
    );
    tk::run_ticks(&mut sim, teardown); // M ... M + teardown - 1
    assert_eq!(
        st(&sim, h) & status::HEAL,
        status::HEAL,
        "{guid}: teardown running"
    );
    assert_eq!(filter(&sim, h), closed_filter());
    tk::run_ticks(&mut sim, 1); // M + teardown
    assert_eq!(st(&sim, h) & (status::HEAL | status::ANIM_LOCKED), 0);
    assert_eq!(filter(&sim, h), MessageFilter::default());
    assert!(
        on(&sim, h, WALK) && on(&sim, h, ATTACK),
        "{guid}: modules back on"
    );
    assert_eq!(healing(&sim, h).unwrap().sick, ObjectId(0));
    assert!(!ready(&sim, h));
    assert_loads(sim.state, &rules);
}

#[test]
fn the_trance_follows_its_four_messages() {
    // Time markers: 2014 setup = teardown 7 (3999 ms) -> +40; 2303 setup 3 / teardown 6
    // (4500 ms) -> +46. Timers from `send` before tick N fall in N + floor(d/100) + 1.
    trance_timeline(tk::PILGRIM, 7, 7, 40, 40);
    trance_timeline(tk::MEDIC, 3, 6, 46, 46);
}

#[test]
fn begin_is_ignored_while_locked_or_in_trance() {
    // C-254: `0x11D` aborts with `0x800000` (banishing or setup) and with `HEAL`.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, true);
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    assert_eq!(st(&sim, h) & status::HEAL, 0, "locked: no trance");
    assert_eq!(filter(&sim, h), MessageFilter::default());
    assert_eq!(timers(&sim, h, msg::READY), 0);
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, false);
    // First `0x11D`; a second one in the same tick (lock bit and `HEAL`) and one after the setup
    // (only `HEAL`) have no effect.
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    assert_eq!(timers(&sim, h, msg::READY), 1);
    tk::run_ticks(&mut sim, 41); // N ... N + 40
    assert!(ready(&sim, h));
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    assert!(ready(&sim, h), "without a new setup");
    assert_eq!(timers(&sim, h, msg::READY), 0);
    assert_eq!(st(&sim, h) & status::ANIM_LOCKED, 0);
    assert_eq!(filter(&sim, h), trance_filter());
}

#[test]
fn orders_are_lost_only_until_the_end_of_the_trance() {
    // K-53, C-247: in the trance only `0x21D`, in the teardown nothing at all; discarded, not queued.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    tk::run_ticks(&mut sim, 41); // N ... N + 40
    assert!(ready(&sim, h));
    assert!(
        !post_command(&mut sim, h, walk_order(), true),
        "walking discarded"
    );
    assert!(!post_message(
        &mut sim,
        h,
        Message::new(msg::BEGIN, &[]),
        true
    ));
    assert!(!post_message(
        &mut sim,
        h,
        Message::new(msg::NEW_TARGET, &[1]),
        true
    ));
    assert!(sim.object(h).unwrap().fifo.is_empty(), "nothing queued");
    // Command `0x35`: `0x21D` posted (with abort), effective on processing in tick M.
    assert!(post_message(
        &mut sim,
        h,
        Message::new(msg::END_REQUEST, &[]),
        true
    ));
    tk::run_ticks(&mut sim, 1); // M
    assert!(!ready(&sim, h));
    assert_eq!(filter(&sim, h), closed_filter());
    // Saving and loading in the teardown phase: filter, `HEAL` and the timer `0x41D` stay.
    let mut loaded = crate::save::testkit::save_and_load(sim.state, &rules);
    assert_loads(&loaded, &rules);
    let mut sim = Sim::new(&mut loaded, &rules);
    assert_eq!(filter(&sim, h), closed_filter());
    assert_eq!(timers(&sim, h, msg::END), 1);
    assert!(!post_message(
        &mut sim,
        h,
        Message::new(msg::END_REQUEST, &[]),
        true
    ));
    assert!(!post_message(
        &mut sim,
        h,
        Message::new(msg::BEGIN, &[]),
        true
    ));
    assert!(!post_command(&mut sim, h, walk_order(), true));
    // `0x41D` from the FIFO processing in M falls in M + 40.
    tk::run_ticks(&mut sim, 39); // M + 1 ... M + 39
    assert_eq!(st(&sim, h) & status::HEAL, status::HEAL);
    assert!(!post_command(&mut sim, h, walk_order(), true));
    tk::run_ticks(&mut sim, 1); // M + 40
    assert_eq!(st(&sim, h) & status::HEAL, 0);
    assert!(
        post_command(&mut sim, h, walk_order(), true),
        "accepted again"
    );
    assert!(post_message(
        &mut sim,
        h,
        Message::new(msg::BEGIN, &[]),
        true
    ));
}

#[test]
fn stop_during_the_setup_animation_is_lost() {
    // C-248: `0x21D` with `0x800000` set (setup) is ignored; the unit becomes ready in N + 40
    // anyway and stays in the trance.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
    tk::run_ticks(&mut sim, 10); // N ... N + 9
    assert_eq!(say(&mut sim, h, msg::END_REQUEST), -1);
    // Also posted (command `0x35`): the filter lets it through, the query discards it.
    assert!(post_message(
        &mut sim,
        h,
        Message::new(msg::END_REQUEST, &[]),
        true
    ));
    tk::run_ticks(&mut sim, 1); // N + 10
    assert_eq!(filter(&sim, h), trance_filter());
    assert_eq!(timers(&sim, h, msg::END), 0);
    assert_eq!(wanted_anim(&sim, h), 7);
    tk::run_ticks(&mut sim, 29); // N + 11 ... N + 39
    assert!(!ready(&sim, h));
    tk::run_ticks(&mut sim, 1); // N + 40
    assert!(ready(&sim, h));
    tk::run_ticks(&mut sim, 60);
    assert!(ready(&sim, h), "still ready");
    assert_eq!(st(&sim, h) & status::HEAL, status::HEAL);
    assert_eq!(filter(&sim, h), trance_filter());
    assert!(!on(&sim, h, WALK));
}

/// Carcass of the pilgrim on slot 8 (`StartDying` changes the owner, C-142).
fn corpse(sim: &Sim) -> ObjectId {
    sim.state
        .objects()
        .find(|o| o.guid == tk::PILGRIM && o.owner() == 8 && o.status & status::DIED != 0)
        .map(|o| o.id)
        .expect("carcass")
}

#[test]
fn a_healer_dying_in_trance_gets_the_die_anim_and_stops() {
    // re/healing 4 (dying): `0x61D` returns `m_DieAnim` (2, not the 3 of the dying module) and clears
    // `m_ReadyToHeal`; `HEAL` and the filter stay. Death occurs directly before the tick of the
    // timer: the healing module is still in the tick list in this tick (C-150), but the
    // timer does not fire because the healing tick ends at `DIED`.
    enum Phase {
        Setup,
        Ready,
        Teardown,
    }
    for phase in [Phase::Setup, Phase::Ready, Phase::Teardown] {
        let rules = rules();
        let mut state = tk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let h = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
        assert_eq!(say(&mut sim, h, msg::BEGIN), -1);
        let (expect, pending) = match phase {
            Phase::Setup => {
                // `0x31D` falls in N + 40: death before it.
                tk::run_ticks(&mut sim, 40); // N ... N + 39
                assert_eq!(timers(&sim, h, msg::READY), 1);
                (trance_filter(), Some(msg::READY))
            }
            Phase::Ready => {
                tk::run_ticks(&mut sim, 41); // N ... N + 40
                assert!(ready(&sim, h));
                (trance_filter(), None)
            }
            Phase::Teardown => {
                tk::run_ticks(&mut sim, 41);
                // `0x41D` falls in M + 40: death before it.
                assert_eq!(say(&mut sim, h, msg::END_REQUEST), -1);
                tk::run_ticks(&mut sim, 40); // M ... M + 39
                assert_eq!(timers(&sim, h, msg::END), 1);
                (closed_filter(), Some(msg::END))
            }
        };
        assert_eq!(say(&mut sim, h, 0x110), 0);
        assert_eq!(wanted_anim(&sim, h), 2, "dying animation from module 29");
        assert!(!ready(&sim, h));
        tk::run_ticks(&mut sim, 60);
        let c = corpse(&sim);
        assert_eq!(st(&sim, c) & status::HEAL, status::HEAL, "HEAL stays");
        assert_eq!(filter(&sim, c), expect);
        assert!(!healing(&sim, c).unwrap().ready, "never ready");
        assert_eq!(sim.object(c).unwrap().anim.current, 2);
        if let Some(code) = pending {
            assert_eq!(timers(&sim, c, code), 1, "{code:#X} never fired");
        }
        assert_loads(sim.state, &rules);
    }
}

#[test]
fn the_stand_module_asks_for_the_trance_idle() {
    // W-152: with `HEAL` module 29 answers (`0x51D {sel}` -> `m_StandAnim[sel % n]`) without
    // falling back to the stand list, also with -1 (negative index, A7).
    use crate::module::stand::select_idle_anim;
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, STANDER, 0, (20, 20));
    assert_eq!(
        select_idle_anim(&mut sim, h, 3),
        1,
        "without HEAL: stand list"
    );
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::HEAL, true);
    assert_eq!(select_idle_anim(&mut sim, h, 3), 6);
    assert_eq!(select_idle_anim(&mut sim, h, 4), 4);
    assert_eq!(select_idle_anim(&mut sim, h, -1), -1, "no fallback");
    assert_eq!(send(&mut sim, h, &Message::new(msg::STAND, &[5])), 6);
    // Single-element list (pilgrim `0`): -1 % 1 = 0.
    let p = tk::spawn(&mut sim, tk::PILGRIM, 0, (24, 20));
    sim.object_mut(p)
        .unwrap()
        .set_status_flags(status::HEAL, true);
    assert_eq!(select_idle_anim(&mut sim, p, -1), 0);
}

#[test]
fn a_quartered_healer_switches_the_building() {
    // re/healing 4 (quarters), C-250: no shipped building has module 23 and 29 at the same time. Entry ->
    // `0x71D {1}` (ready), exit -> `0x71D {0}` (not ready); a finished healing building with a
    // HEF name becomes ready again by itself in the next tick, one without stays off.
    use crate::module::quarter::{accept_unit, leave_container};
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    for (k, (guid, comes_back)) in [(HOUSE, true), (HOUSE_WITHOUT_HEF, false)]
        .into_iter()
        .enumerate()
    {
        let x = 10 + 20 * k as i32;
        let house = tk::spawn(&mut sim, guid, 0, (x, 20));
        let healer = tk::spawn(&mut sim, tk::PILGRIM, 0, (x, 26));
        assert!(
            !ready(&sim, house),
            "{guid}: not ready before the first tick"
        );
        assert_eq!(accept_unit(&mut sim, house, healer), 0);
        assert!(ready(&sim, house), "{guid}: entry");
        assert_ne!(st(&sim, healer) & status::QUARTERED, 0);
        assert!(leave_container(&mut sim, healer));
        assert!(!ready(&sim, house), "{guid}: exit");
        assert_eq!(
            send(&mut sim, house, &Message::new(msg::CONTAINER, &[1])),
            1
        );
        assert!(ready(&sim, house));
        assert_eq!(
            send(&mut sim, house, &Message::new(msg::CONTAINER, &[7])),
            1
        );
        assert!(!ready(&sim, house), "{guid}: every argument != 1");
        tk::run_ticks(&mut sim, 1);
        assert_eq!(ready(&sim, house), comes_back, "{guid}: next tick");
        assert!(!ready(&sim, healer), "the healer itself stays untouched");
    }
}

#[test]
fn buildings_become_ready_after_completion_with_a_hef() {
    // C-249, re/healing 3.1/9.4: finished in F -> ready in the first healing tick after it; never without a HEF name.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let monastery = site(&mut sim, tk::MONASTERY, (20, 20));
    let plain = site(&mut sim, MONASTERY_WITHOUT_HEF, (30, 20));
    tk::run_ticks(&mut sim, 5);
    assert!(!ready(&sim, monastery), "construction site");
    finish(&mut sim, monastery);
    finish(&mut sim, plain);
    assert!(!ready(&sim, monastery));
    tk::run_ticks(&mut sim, 1);
    assert!(ready(&sim, monastery), "first tick after completion");
    tk::run_ticks(&mut sim, 100);
    assert!(ready(&sim, monastery), "stays ready");
    assert!(!ready(&sim, plain), "never without a HEF name");
    // Created finished: ready in the first tick.
    let yard = tk::spawn(&mut sim, tk::SHIPYARD, 0, (40, 20));
    assert!(!ready(&sim, yard));
    tk::run_ticks(&mut sim, 1);
    assert!(ready(&sim, yard));
    // Units never become ready through the tick.
    let p = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 40));
    tk::run_ticks(&mut sim, 10);
    assert!(!ready(&sim, p));
}

#[test]
fn new_target_is_stored_unfiltered() {
    // C-251: `0x81D {target}` accepts any ID (also enemies, without a trance); the effect follows in the
    // single-healing tests.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    let enemy = tk::spawn(&mut sim, tk::COLONIST, 1, (40, 40));
    let to = |t: i32| Message::new(msg::NEW_TARGET, &[t]);
    assert_eq!(send(&mut sim, h, &to(enemy.as_i32())), 1);
    assert_eq!(healing(&sim, h).unwrap().new_target, enemy);
    assert_eq!(send(&mut sim, h, &to(12_345)), 1);
    assert_eq!(healing(&sim, h).unwrap().new_target, ObjectId(12_345));
    assert_eq!(st(&sim, h) & status::HEAL, 0);
    // Unknown codes -1; `0x61D` also outside of dying.
    assert_eq!(say(&mut sim, h, 0x91D), -1);
    assert_eq!(say(&mut sim, h, msg::DIE), 2);
    assert_loads(sim.state, &rules);
}

// --- Area healing (re/healing 3.6, 4, 9.1, C-242, C-246) ---

/// Hit points (attribute 14) and the infection flag of the dying module (`ISINFECTED`, private in
/// `dying.rs`).
const HP: u32 = 14;
const INFECTED: u32 = 0x1000;
/// Radius of the monasteries and shipyards (sight 40): `3 * (40 << 10) / 4`.
const AREA_RADIUS: i32 = 30_720;

fn fraction(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).unwrap().attributes.raw_fraction(HP)
}

fn wound(sim: &mut Sim, id: ObjectId, f: i32) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_fraction(HP, f);
}

fn grid(sim: &Sim, id: ObjectId) -> (i32, i32) {
    let o = sim.object(id).unwrap();
    (o.grid.x.0, o.grid.z.0)
}

/// One object tick (two `step`s) with its events.
fn tick_events(sim: &mut Sim) -> Vec<SimEvent> {
    (0..2)
        .flat_map(|_| crate::tick::step(sim.state, sim.rules, &[]))
        .collect()
}

/// The `Healed` messages as `(healer, target, amount)`.
fn healed(events: &[SimEvent]) -> Vec<(ObjectId, ObjectId, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Healed {
                healer,
                target,
                points,
            } => Some((*healer, *target, *points)),
            _ => None,
        })
        .collect()
}

fn infect(sim: &mut Sim, id: ObjectId) {
    send(sim, id, &Message::new(0x910, &[1]));
    assert!(infected(sim, id));
}

fn infected(sim: &Sim, id: ObjectId) -> bool {
    let dying = crate::module::dying::INDEX;
    sim.module(id, dying).unwrap().base.flags & INFECTED != 0
}

#[test]
fn a_monastery_heals_by_the_minute_table() {
    // re/healing 9.1: monastery 1015 (H = 180), saber fighter 1107 (190) at 50 % within 7 tiles ->
    // delta = 180 * 100,000 / (190 * 600) = 157 per tick, full after 319 ticks.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = tk::spawn(&mut sim, tk::MONASTERY, 0, (20, 20));
    let s = tk::spawn(&mut sim, tk::SABRE, 0, (27, 20));
    let d = crate::collision::fixed_pos_distance(grid(&sim, m), grid(&sim, s));
    assert_eq!(d, 7 << 12, "7 tiles");
    wound(&mut sim, s, 50_000);
    // First tick: the finished monastery becomes ready, but heals only in the next.
    let ev = tick_events(&mut sim);
    assert!(ready(&sim, m));
    assert!(healed(&ev).is_empty());
    assert_eq!(fraction(&sim, s), 50_000);
    for k in 1..=318 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(m, s, 180)], "tick {k}");
        assert_eq!(fraction(&sim, s), 50_000 + 157 * k, "tick {k}");
    }
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(m, s, 180)]);
    assert!(
        sim.object(s).unwrap().attributes.is_full(HP),
        "full after 319"
    );
    assert_eq!(fraction(&sim, s), 100_000);
    // Full: no longer found, no message.
    let ev = tick_events(&mut sim);
    assert!(healed(&ev).is_empty());
    assert_loads(sim.state, &rules);
}

#[test]
fn a_shipyard_heals_its_own_ships() {
    // re/healing 9.1: shipyard 1008 (H = 600), SPA_Transportschiff 1108 (1400) -> 71 per tick.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let yard = tk::spawn(&mut sim, tk::SHIPYARD, 0, (20, 20));
    let ship = tk::spawn(&mut sim, tk::SHIP, 0, (25, 20));
    wound(&mut sim, ship, 50_000);
    tk::run_ticks(&mut sim, 1);
    assert!(ready(&sim, yard));
    for k in 1..=5 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(yard, ship, 600)]);
        assert_eq!(fraction(&sim, ship), 50_000 + 71 * k);
    }
}

#[test]
fn a_soldier_first_in_the_query_stops_the_shipyard() {
    // C-242: the injured soldier is in the bucket of the shipyard (tiles 16-23), the ship
    // in the first ring; the soldier comes first, its GUID is missing from `1108 1109 1117` -> the whole
    // loop ends: neither healing nor `0x910` for him or the ship.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let yard = tk::spawn(&mut sim, tk::SHIPYARD, 0, (20, 20));
    let soldier = tk::spawn(&mut sim, tk::SABRE, 0, (22, 20));
    let ship = tk::spawn(&mut sim, tk::SHIP, 0, (26, 20));
    wound(&mut sim, soldier, 50_000);
    wound(&mut sim, ship, 50_000);
    // Ships are `NOTHUMAN` and never infected; the break before `0x910` is shown by the soldier.
    infect(&mut sim, soldier);
    let pos = grid(&sim, yard);
    let hits =
        crate::collision::query_radius(&mut sim, pos, &match_filter(0x1700), AREA_RADIUS, 0, false);
    assert_eq!(hits, [soldier, ship], "order of discovery");
    tk::run_ticks(&mut sim, 1); // ready
    for _ in 0..5 {
        let ev = tick_events(&mut sim);
        assert!(healed(&ev).is_empty());
    }
    assert_eq!(fraction(&sim, soldier), 50_000);
    assert_eq!(fraction(&sim, ship), 50_000);
    assert!(infected(&sim, soldier), "no 0x910");
    // If the soldier is full, the filter no longer finds him: the ship is healed.
    wound(&mut sim, soldier, 100_000);
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(yard, ship, 600)]);
    assert_eq!(fraction(&sim, ship), 50_071);
    assert!(infected(&sim, soldier), "full: not found");
}

#[test]
fn area_healing_takes_own_walk_objects_only() {
    // C-246: only own (`MYLIST`, no allies), non-quartered walk objects, without swimming
    // exception; center distance <= 30,720. `Fixed_Hypot` (`0x00401200`) loses the lower two bits via
    // `Fixed_Div`: on the axis 30,721 ... 30,723 count as 30,720, only
    // 30,724 is outside.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = &mut sim.state.diplomacy;
    for (p, q) in [(0, 2), (2, 0)] {
        d.friend[p] |= 1 << q;
        d.enemy[p] &= !(1 << q);
    }
    assert!(sim.state.diplomacy.is_friend(0, 2) && !sim.state.diplomacy.is_enemy(0, 2));
    let m = tk::spawn(&mut sim, tk::MONASTERY, 0, (20, 20));
    let ally = tk::spawn(&mut sim, tk::SABRE, 2, (22, 20));
    let enemy = tk::spawn(&mut sim, tk::SABRE, 1, (20, 22));
    let post = tk::spawn(&mut sim, tk::ALARM_POST, 0, (18, 18));
    let guest = tk::spawn(&mut sim, tk::SABRE, 0, (18, 21));
    let swimmer = tk::spawn(&mut sim, tk::SABRE, 0, (22, 22));
    let at_edge = tk::spawn(&mut sim, tk::SABRE, 0, (28, 20));
    let beyond = tk::spawn(&mut sim, tk::SABRE, 0, (13, 20));
    tk::run_ticks(&mut sim, 1);
    assert!(ready(&sim, m));
    assert_eq!(
        crate::module::quarter::accept_unit(&mut sim, post, guest),
        0
    );
    assert_ne!(st(&sim, guest) & status::QUARTERED, 0);
    sim.object_mut(swimmer)
        .unwrap()
        .set_status_flags(status::SWIMMING, true);
    let dist = |x: i32| crate::collision::fixed_pos_distance((0, 0), (x, 0));
    assert_eq!(
        [dist(30_720), dist(30_721), dist(30_723), dist(30_724)],
        [30_720, 30_720, 30_720, 30_724]
    );
    // On the radius (rounded) or the first fixed-point unit behind it, in the same bucket as
    // before.
    let (mx, mz) = grid(&sim, m);
    for (id, x) in [
        (at_edge, mx + AREA_RADIUS + 3),
        (beyond, mx - AREA_RADIUS - 4),
    ] {
        let o = sim.object_mut(id).unwrap();
        o.grid.x = crate::fx::Fx(x);
        o.grid.z = crate::fx::Fx(mz);
    }
    let all = [ally, enemy, post, guest, swimmer, at_edge, beyond];
    for id in all {
        wound(&mut sim, id, 50_000);
    }
    // Called directly: the position of the two moved soldiers stays untouched.
    sim.state.events.clear();
    heal::heal_area(&mut sim, m);
    let ev = std::mem::take(&mut sim.state.events);
    assert_eq!(healed(&ev), [(m, swimmer, 180), (m, at_edge, 180)]);
    assert_eq!(fraction(&sim, swimmer), 50_157, "swimmer");
    assert_eq!(fraction(&sim, at_edge), 50_157, "distance 30,723");
    for id in [ally, enemy, post, guest, beyond] {
        assert_eq!(fraction(&sim, id), 50_000, "{id:?} not healed");
    }
}

#[test]
fn area_healing_cures_the_infection_of_healed_targets() {
    // re/healing 3.6: `0x910 {0}` goes to every hit (cure plague); full objects are no
    // hits (`ENERGYLOST` in the healing filter), their infection stays.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = tk::spawn(&mut sim, tk::MONASTERY, 0, (20, 20));
    let sick = tk::spawn(&mut sim, tk::SABRE, 0, (23, 20));
    let full = tk::spawn(&mut sim, tk::SABRE, 0, (20, 23));
    wound(&mut sim, sick, 50_000);
    infect(&mut sim, sick);
    infect(&mut sim, full);
    tk::run_ticks(&mut sim, 1); // ready, no healing yet
    assert!(ready(&sim, m));
    assert!(infected(&sim, sick));
    tk::run_ticks(&mut sim, 1);
    assert!(!infected(&sim, sick), "infection off");
    assert_eq!(fraction(&sim, sick), 50_157);
    assert!(infected(&sim, full), "full: not found");
    assert!(sim.object(full).unwrap().attributes.is_full(HP));
}

// --- Single healing (re/healing 3.7, 8 no. 3-5 and 11, 9.2, 9.3; C-243 to C-245, C-251) ---

/// Mana (attribute 15, extra energy) and sight (18).
const MANA: u32 = 15;
const SIGHT: u32 = 18;

fn mana(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).unwrap().attributes.raw_fraction(MANA)
}

fn set_mana(sim: &mut Sim, id: ObjectId, f: i32) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_fraction(MANA, f);
}

fn sick(sim: &Sim, id: ObjectId) -> ObjectId {
    healing(sim, id).unwrap().sick
}

fn new_target(sim: &Sim, id: ObjectId) -> ObjectId {
    healing(sim, id).unwrap().new_target
}

/// Range of single healing `r = sight << 8` (re/healing 3.7).
fn reach(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, SIGHT) << 8
}

/// Send a healer with full mana (created healers have 0) into the trance and tick until it is
/// ready; the first healing tick is the next.
fn entrance(sim: &mut Sim, h: ObjectId) {
    set_mana(sim, h, 100_000);
    assert_eq!(say(sim, h, msg::BEGIN), -1);
    for _ in 0..50 {
        if ready(sim, h) {
            assert_eq!(mana(sim, h), 100_000);
            return;
        }
        tk::run_ticks(sim, 1);
    }
    panic!("healer not ready");
}

/// Finished object at the world position `(x, z)` (fixed point = world * 32).
fn spawn_at(sim: &mut Sim, guid: u32, slot: i32, x: f32, z: f32) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [x, 0.0, z]);
    desc.flags |= crate::world::desc_flags::PLACED;
    create_object(sim, &mut desc).expect("object created")
}

#[test]
fn a_field_doctor_heals_by_the_end_spurt_formula() {
    // re/healing 9.2 (C-243): medic 2303 (H = 288, mana max 950), female farmer 2302 (max 100).
    // `v = min(max(min(H, (Max - Current) * 60), 60), min(H, Mana * 60))`; target and mana with scale 600,
    // the mana regeneration (+10) runs after the module tick.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let medic = tk::spawn(&mut sim, tk::MEDIC, 0, (20, 20));
    let farmer = tk::spawn(&mut sim, tk::FARMER, 0, (22, 20));
    entrance(&mut sim, medic);
    infect(&mut sim, farmer);
    // (fraction before, delta target, delta mana): current 50 -> v = 288; current 98 -> v = 120; fraction 99,600
    // (current rounded 100, need 0) -> v = 60 (minimum amount).
    for (hp, gain, cost) in [(50_000, 480, 50), (98_000, 200, 21), (99_600, 100, 10)] {
        wound(&mut sim, farmer, hp);
        let before = mana(&sim, medic);
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(medic, farmer, 288)], "fraction {hp}");
        assert_eq!(fraction(&sim, farmer), hp + gain, "fraction {hp}");
        assert_eq!(mana(&sim, medic), before - cost + 10, "fraction {hp}");
        assert_eq!(sick(&sim, medic), farmer);
        assert_eq!(wanted_anim(&sim, medic), 7, "healing animation");
    }
    assert!(!infected(&sim, farmer), "0x910 {{0}} to the target");
    // Full: given up, no target any more -> stand animation `stand_anim[0]` (4), no mana.
    wound(&mut sim, farmer, 100_000);
    let before = mana(&sim, medic);
    let ev = tick_events(&mut sim);
    assert!(healed(&ev).is_empty());
    assert_eq!(sick(&sim, medic), ObjectId(0));
    assert_eq!(wanted_anim(&sim, medic), 4);
    assert_eq!(mana(&sim, medic), before + 10);
    assert_loads(sim.state, &rules);
}

#[test]
fn the_healer_heals_himself_first() {
    // C-245: the healing filter (MYLIST) contains the healer itself with edge distance 0; the female farmer
    // next to it has edge distance > 0. Pilgrim 2014 (1000, 500): need 30,000 -> v = 288,
    // fraction +48, mana -50.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let pilgrim = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    let farmer = tk::spawn(&mut sim, tk::FARMER, 0, (21, 20));
    entrance(&mut sim, pilgrim);
    let p = grid(&sim, pilgrim);
    assert_eq!(
        crate::collision::edge_distance_to_point(&mut sim, pilgrim, p),
        0
    );
    assert!(crate::collision::edge_distance_to_point(&mut sim, farmer, p) > 0);
    wound(&mut sim, pilgrim, 50_000);
    wound(&mut sim, farmer, 50_000);
    for k in 1..=3 {
        let before = mana(&sim, pilgrim);
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(pilgrim, pilgrim, 288)], "tick {k}");
        assert_eq!(fraction(&sim, pilgrim), 50_000 + 48 * k);
        assert_eq!(mana(&sim, pilgrim), before - 50 + 10);
        assert_eq!(sick(&sim, pilgrim), pilgrim);
        assert_eq!(fraction(&sim, farmer), 50_000, "the female farmer waits");
    }
}

#[test]
fn single_healing_searches_own_walk_objects_only() {
    // C-246 for single healing: the same filter as for the area (`MYLIST`, walk, not full).
    // An injured ally and an injured enemy stand closer to the medic than the own
    // female farmer; only the female farmer is found and healed.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = &mut sim.state.diplomacy;
    for (p, q) in [(0, 2), (2, 0)] {
        d.friend[p] |= 1 << q;
        d.enemy[p] &= !(1 << q);
    }
    let medic = tk::spawn(&mut sim, tk::MEDIC, 0, (20, 20));
    let ally = tk::spawn(&mut sim, tk::FARMER, 2, (21, 20));
    let enemy = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 21));
    let farmer = tk::spawn(&mut sim, tk::FARMER, 0, (22, 20));
    entrance(&mut sim, medic);
    for id in [ally, enemy, farmer] {
        wound(&mut sim, id, 50_000);
    }
    let m = grid(&sim, medic);
    let dist = |sim: &mut Sim, id| crate::collision::edge_distance_to_point(sim, id, m);
    assert!(dist(&mut sim, ally) < dist(&mut sim, farmer));
    assert!(dist(&mut sim, enemy) < dist(&mut sim, farmer));
    for k in 1..=3 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(medic, farmer, 288)], "tick {k}");
        assert_eq!(sick(&sim, medic), farmer);
    }
    assert_eq!(fraction(&sim, ally), 50_000);
    assert_eq!(fraction(&sim, enemy), 50_000);
}

#[test]
fn a_target_in_the_band_is_dropped_and_found_again() {
    // C-244: keeping only up to edge distance r (= 10,240 for the medic), the search reaches up to
    // r + r_self (512). The female farmer with edge distance 10,496 is given up every tick and found again
    // immediately: healing runs uninterrupted, `sick` stays the same ID. A
    // second one with edge distance 10,760 is outside the search.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let medic = tk::spawn(&mut sim, tk::MEDIC, 0, (20, 20));
    let c = tk::world(20);
    // Center distance 11,008 or 11,272 (divisible by 4, `Fixed_Hypot` exact, K-143).
    let band = spawn_at(&mut sim, tk::FARMER, 0, c + 344.0, c);
    let outside = spawn_at(&mut sim, tk::FARMER, 0, c - 352.25, c);
    entrance(&mut sim, medic);
    let r = reach(&sim, medic);
    assert_eq!(r, 10_240);
    let m = grid(&sim, medic);
    let edge = |sim: &mut Sim, id| crate::collision::edge_distance_to_point(sim, id, m);
    assert_eq!(edge(&mut sim, band), 10_496);
    assert_eq!(edge(&mut sim, outside), 10_760);
    assert!(r < 10_496 && 10_496 <= r + 512 && r + 512 < 10_760);
    wound(&mut sim, band, 50_000);
    wound(&mut sim, outside, 50_000);
    for k in 1..=5 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(medic, band, 288)], "tick {k}");
        assert_eq!(fraction(&sim, band), 50_000 + 480 * k);
        assert_eq!(sick(&sim, medic), band);
    }
    // Full: given up; the search does not find the second one.
    wound(&mut sim, band, 100_000);
    let ev = tick_events(&mut sim);
    assert!(healed(&ev).is_empty());
    assert_eq!(sick(&sim, medic), ObjectId(0));
    assert_eq!(fraction(&sim, outside), 50_000);
}

#[test]
fn a_fresh_healer_settles_at_a_tenth_hitpoint_per_tick() {
    // re/healing 9.3, C-241: the created pilgrim has mana 0, regeneration +10 per tick after the
    // module tick. 9.3 presupposes the ready trance from creation on (the setup takes 40 ticks),
    // therefore `ready` is set directly here. Until `current(15) = round(950 * B / 100,000)` reaches 1
    // (B = 60), the healer pays nothing and stands; after that `pay = 60`, `v = 60`: target
    // +10 fraction, mana -10 + 10 -> equilibrium at 60.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let healer = tk::spawn(&mut sim, tk::PILGRIM, 0, (20, 20));
    let target = tk::spawn(&mut sim, tk::PILGRIM, 0, (21, 20));
    wound(&mut sim, target, 50_000);
    assert_eq!(mana(&sim, healer), 0);
    healing_mut(&mut sim, healer).unwrap().ready = true;
    for k in 1..=6 {
        let ev = tick_events(&mut sim);
        assert!(healed(&ev).is_empty(), "tick {k}");
        assert_eq!(mana(&sim, healer), 10 * k, "tick {k}");
        assert_eq!(fraction(&sim, target), 50_000);
        assert_eq!(sick(&sim, healer), target, "target found, but no mana");
    }
    assert_eq!(sim.attribute(healer, MANA), 1);
    for k in 1..=20 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(healer, target, 288)], "healing tick {k}");
        assert_eq!(fraction(&sim, target), 50_000 + 10 * k);
        assert_eq!(mana(&sim, healer), 60);
    }
}

#[test]
fn a_requested_target_out_of_range_blocks_healing() {
    // C-251: `m_NewTarget` out of range (center distance > r) blocks every healing; the
    // walk order `0x400` hits a switched-off walk in the trance. In range, the requested target is
    // taken over without a filter (here an enemy) and `m_NewTarget` is cleared.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let medic = tk::spawn(&mut sim, tk::MEDIC, 0, (20, 20));
    let near = tk::spawn(&mut sim, tk::FARMER, 0, (21, 20));
    let far = tk::spawn(&mut sim, tk::FARMER, 0, (26, 20));
    let enemy = tk::spawn(&mut sim, tk::COLONIST, 1, (20, 22));
    entrance(&mut sim, medic);
    let m = grid(&sim, medic);
    let r = reach(&sim, medic);
    assert!(crate::collision::fixed_pos_distance(m, grid(&sim, far)) > r);
    assert!(crate::collision::fixed_pos_distance(m, grid(&sim, enemy)) <= r);
    for id in [near, far, enemy] {
        wound(&mut sim, id, 50_000);
    }
    let request = |t: ObjectId| Message::new(msg::NEW_TARGET, &[t.as_i32()]);
    assert_eq!(send(&mut sim, medic, &request(far)), 1);
    for k in 1..=5 {
        let ev = tick_events(&mut sim);
        assert!(healed(&ev).is_empty(), "tick {k}");
        assert_eq!(new_target(&sim, medic), far);
        assert_eq!(sick(&sim, medic), ObjectId(0));
    }
    assert!(!on(&sim, medic, WALK));
    assert_eq!(grid(&sim, medic), m, "no walking");
    for id in [near, far, enemy] {
        assert_eq!(fraction(&sim, id), 50_000);
    }
    // In range: taken over without a filter (enemy on slot 1), kept afterwards.
    assert_eq!(send(&mut sim, medic, &request(enemy)), 1);
    for k in 1..=2 {
        let ev = tick_events(&mut sim);
        assert_eq!(healed(&ev), [(medic, enemy, 288)], "tick {k}");
        // Colonist (max 160): 288 * 100,000 / (160 * 600) = 300.
        assert_eq!(fraction(&sim, enemy), 50_000 + 300 * k);
        assert_eq!(new_target(&sim, medic), ObjectId(0));
        assert_eq!(sick(&sim, medic), enemy);
    }
    assert_eq!(fraction(&sim, near), 50_000);
}

#[test]
fn a_dying_target_is_given_up() {
    // `GameWorld_GetObjectById(id, 0)` (`0x00475300`) also returns the dead; the carcass changes
    // to slot 8, though, and with it the ID (C-142), `m_SickPersonHID` points to nothing and the search
    // finds the next injured unit. A removed target likewise.
    let rules = rules();
    let mut state = tk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let medic = tk::spawn(&mut sim, tk::MEDIC, 0, (20, 20));
    let first = tk::spawn(&mut sim, tk::FARMER, 0, (21, 20));
    let second = tk::spawn(&mut sim, tk::FARMER, 0, (22, 20));
    let third = tk::spawn(&mut sim, tk::FARMER, 0, (19, 22));
    entrance(&mut sim, medic);
    for id in [first, second, third] {
        wound(&mut sim, id, 50_000);
    }
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(medic, first, 288)]);
    assert_eq!(say(&mut sim, first, 0x110), 0);
    assert_ne!(st(&sim, first) & status::DIED, 0);
    // In the first tick after that the dying unit still stands under its ID and is healed further;
    // after that the carcass has the new ID on slot 8.
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(medic, first, 288)], "dying one");
    assert!(sim.object(first).is_none(), "ID changed");
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(medic, second, 288)]);
    assert_eq!(sick(&sim, medic), second);
    crate::world::destroy_object(&mut sim, second);
    let ev = tick_events(&mut sim);
    assert_eq!(healed(&ev), [(medic, third, 288)]);
    assert_eq!(sick(&sim, medic), third);
    assert_loads(sim.state, &rules);
}
