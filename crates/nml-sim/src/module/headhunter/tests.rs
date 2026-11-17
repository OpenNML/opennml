//! Tests of the headhunter (re/headhunter-robbing): state, `Init` and destructor of module 34,
//! world field `headhunters` (1.2-1.4, 10.1), search, approach, way home, tick, melee kill and
//! dynamite throw.

use crate::combat::hit_zone::{hit_zone_world_pos, vector3_length, world_pos};
use crate::combat::projectile::ProjectileKind;
use crate::command::Command;
use crate::command::headhunter::{COLOR_GREEN, SFX_SUCCESS, TEXT_SUCCESS};
use crate::fog::SEE;
use crate::fog::sources::VisionSource;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::module::ability_testkit as tk;
use crate::module::building::{building, get_exit_pos};
use crate::module::quarter;
use crate::object::status;
use crate::sim::Sim;
use crate::state::{SimEvent, TextArg};
use crate::world::{change_owner, queue_destroy, set_grid_position};

use super::{FAR, GROUP, INDEX, NEAR, VISION_SIZE, art_of, headhunter, headhunter_mut, msg};

/// The sight source of the headhunter `id` (object-bound, re/headhunter-robbing 1.2).
fn bound_source(id: ObjectId) -> VisionSource {
    VisionSource {
        x: -1,
        z_or_hid: id.as_i32(),
        size: VISION_SIZE,
        player: -1,
        remaining_ms: -1,
        flags: SEE,
    }
}

/// All sight sources bound to `id`.
fn sources_of(sim: &Sim, id: ObjectId) -> Vec<VisionSource> {
    sim.state
        .fog
        .sources
        .iter()
        .filter(|s| s.x == -1 && s.z_or_hid == id.as_i32())
        .cloned()
        .collect()
}

// QUIRK C-310: kept
#[test]
fn init_registers_the_hunter_and_its_vision() {
    // C-310 (simulation part): the headhunter on slot 8 reads the price from the value set of slot
    // 8 (here `default.dat`, config 12 = 300), the animations 350002/350003, registers itself in
    // `GameWorld+0x4DC[kind]` and is a sight source for everyone (size 8, unlimited, only
    // "seen", `PropertyHeadHunter_Init` `0x00517C20`).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let id = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    assert_eq!(id.slot(), 8);
    let h = headhunter(&sim, id).expect("module 34");
    assert_eq!((h.cur_reward, h.customer_cost), (300, 300));
    assert_eq!((h.kill_anim_near, h.kill_anim_far), (1, 5));
    assert_eq!(
        (h.target_guid, h.target_hid, h.target_player, h.customer),
        (0, ObjectId::NONE, 0, -1)
    );
    assert_eq!((h.state, h.last_target), (0, ObjectId::NONE));
    // 350001 is not read (re/headhunter-robbing 1.1).
    assert_eq!(sim.module(id, INDEX).unwrap().base.flags, 0);
    assert_eq!(sim.state.headhunters, [Some(id), None, None]);
    assert_eq!(sources_of(&sim, id), [bound_source(id)]);
    // The other kinds register at their slot (kind from the GUID).
    let pri = tk::spawn(&mut sim, tk::HUNTER_PRI, 8, (14, 10));
    let sid = tk::spawn(&mut sim, tk::HUNTER_SID, 8, (18, 10));
    assert_eq!(sim.state.headhunters, [Some(id), Some(pri), Some(sid)]);
    let h = headhunter(&sim, pri).unwrap();
    assert_eq!((h.kill_anim_near, h.kill_anim_far), (4, 3));
    assert_eq!(
        [art_of(4099), art_of(4100), art_of(4102), art_of(4103)],
        [None, Some(0), Some(2), None]
    );
}

// QUIRK C-310: kept
#[test]
fn a_foreign_type_with_module_34_sees_but_is_not_registered() {
    // `SetVisionSource` sits behind the branch "type 0x1004-6 / no valid headhunter", so it
    // applies to every type with module 34; only the table entry depends on the kind. Without a
    // value set (no config entry) price and customer price stay at the constructor value 500.
    let types = format!(
        "{}GUID 4200\nNAME Fremdjaeger\nVALUEID 100 6 1 33554432\n\
         PROPID 34\nVALUEID 350001 6 1 0\nVALUEID 350002 1 1 2\nVALUEID 350003 1 1 6\n",
        tk::hunter_types()
    );
    let rules = tk::hunter_rules_from(&types);
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let id = tk::spawn(&mut sim, 4200, 8, (10, 10));
    let h = headhunter(&sim, id).expect("module 34");
    assert_eq!((h.cur_reward, h.customer_cost), (500, 500));
    assert_eq!((h.kill_anim_near, h.kill_anim_far), (2, 6));
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert_eq!(sources_of(&sim, id), [bound_source(id)]);
    // The destructor removes the source here too.
    queue_destroy(&mut sim, id);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.object(id).is_none());
    assert!(sources_of(&sim, id).is_empty());
}

// QUIRK C-310: kept
#[test]
fn destroying_the_hunter_clears_table_and_vision() {
    // Destructor `0x00517B80`: `FUN_0047B5B0(kind)` (entry empty), `0x0047E0F0(-1, hid)`.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let id = tk::spawn(&mut sim, tk::HUNTER_SID, 8, (10, 10));
    assert_eq!(sim.state.headhunters, [None, None, Some(id)]);
    queue_destroy(&mut sim, id);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.object(id).is_none());
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert!(sources_of(&sim, id).is_empty());
}

/// A distant timer keeps a hand-made headhunter from going home: without movement, lock
/// and timer its tick sends it home, without a building it vanishes immediately (C-324, C-316).
fn keep_busy(sim: &mut Sim, id: ObjectId) {
    let base = &mut sim.module_mut(id, INDEX).unwrap().base;
    base.post_delayed_message(600_000, Message::new(msg::THINK, &[]), false, GROUP);
}

#[test]
fn a_dead_hunter_stays_registered_until_destroyed() {
    // `FUN_00477100` reads the entry with `GetObjectById(hid, false)` - also the dead (1.4). The
    // headhunter on slot 8 dies without a new ID (`queue_owner_change` to the same slot).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let id = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    keep_busy(&mut sim, id);
    send(&mut sim, id, &Message::new(0x27F, &[-110]));
    // The death happens in the object tick (hit points <= 0, `tick.rs`).
    tk::run_ticks(&mut sim, 1);
    let dead = |sim: &Sim| {
        sim.state
            .object(id, false)
            .is_some_and(|o| o.status & status::DIED != 0)
    };
    assert!(dead(&sim), "dead");
    tk::run_ticks(&mut sim, 3);
    assert!(dead(&sim), "same ID");
    assert_eq!(sim.state.headhunters, [Some(id), None, None]);
    assert_eq!(sources_of(&sim, id), [bound_source(id)]);
    queue_destroy(&mut sim, id);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert!(sources_of(&sim, id).is_empty());
}

#[test]
fn a_second_hunter_overwrites_and_its_destructor_clears() {
    // re/headhunter-robbing 1.2: `Init` overwrites the entry of the kind, the destructor clears it
    // **unconditionally** (`FUN_0047B5B0`) - even if it already belongs to the second headhunter.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let first = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    let second = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (14, 10));
    keep_busy(&mut sim, second);
    assert_eq!(sim.state.headhunters, [Some(second), None, None]);
    queue_destroy(&mut sim, first);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.object(second).is_some());
    assert_eq!(sim.state.headhunters, [None; 3]);
    // Only the source of the first one goes.
    assert!(sources_of(&sim, first).is_empty());
    assert_eq!(sources_of(&sim, second), [bound_source(second)]);
}

// QUIRK C-310: kept
#[test]
fn the_building_spawns_units_and_the_hunter_with_0x2d03() {
    // `0x2D03 {guid, slot}` (`0x0050BA00` -> `!PropertyBuilding_SpawnProducedUnit`, 2.3):
    // 0 = created, 1 = failed. C-310: the headhunter on slot 8 in the color of the HQ.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = tk::spawn(&mut sim, tk::HQ_SPA, 1, (10, 10));
    let spawn = |sim: &mut Sim, guid: u32, slot: i32| {
        send(sim, hq, &Message::new(0x2D03, &[guid as i32, slot]))
    };
    let count = |sim: &Sim| sim.state.objects().count();
    let before = count(&sim);
    assert_eq!(spawn(&mut sim, tk::COLONIST, 0), 0);
    let colonist = sim
        .state
        .objects()
        .find(|o| o.guid == tk::COLONIST)
        .map(|o| o.id)
        .expect("colonist");
    assert_eq!(colonist.slot(), 0);
    assert_eq!(spawn(&mut sim, tk::HUNTER_SPA, 8), 0);
    let hh = sim.state.headhunters[0].expect("headhunter registered");
    let o = sim.object(hh).unwrap();
    assert_eq!((hh.slot(), o.guid), (8, tk::HUNTER_SPA));
    assert_eq!(o.color, sim.object(hq).unwrap().color);
    assert_eq!(o.color, 1);
    // Exit `[0, 0, 300]` of the test world: valid for the headhunter, outside the footprint; the
    // headhunter stands there, on the rally point (without a rally point = exit).
    let (ok, exit) = get_exit_pos(&mut sim, hq, Some(hh));
    assert!(ok, "valid exit");
    let o = sim.object(hh).unwrap();
    assert_eq!((o.grid.x, o.grid.z), (exit.x, exit.z));
    let b = building(&sim, hq).unwrap();
    let (tx, tz) = (exit.x.0 >> 12, exit.z.0 >> 12);
    let inside = (b.rect[0]..=b.rect[2]).contains(&tx) && (b.rect[1]..=b.rect[3]).contains(&tz);
    assert!(!inside, "exit {tx}/{tz} in the footprint {:?}", b.rect);
    assert_eq!((b.meeting_point.x, b.meeting_point.z), (exit.x, exit.z));
    // Unknown GUID: nothing created, returns 1.
    assert_eq!(spawn(&mut sim, 4099, 8), 1);
    assert_eq!(count(&sim), before + 2);
}

// QUIRK C-309: kept
#[test]
fn the_price_query_answers_in_the_arguments() {
    // `0x222` (inline in `0x00519210`, 2.1): `[2..6] = price, target player, target GUID, customer`.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hh = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    {
        let h = headhunter_mut(&mut sim, hh).unwrap();
        (h.cur_reward, h.target_player, h.target_guid, h.customer) = (450, 1, 2615, 0);
    }
    let mut q = Message::new(msg::PRICE, &[0; 4]);
    assert_eq!(send_mut(&mut sim, hh, &mut q), 0);
    assert_eq!(q.args, [450, 1, 2615, 0]);
    // `0x322` in state 0 searches (returns 1; here without target and without building: immediately
    // `0x522`), unknown -1.
    assert_eq!(send(&mut sim, hh, &Message::new(msg::THINK, &[])), 1);
    assert_eq!(sim.state.destroy_queue, [hh]);
    assert_eq!(send(&mut sim, hh, &Message::new(0x1022, &[])), -1);
}

// QUIRK C-309: kept
#[test]
fn the_hire_message_refunds_and_restarts_the_search() {
    // `0x122` (`0x00517FB0`, 3.2): refund to the old customer, then customer, paid price,
    // target player, price +50, target GUID, state 0, clear group 0x80, `0x322` after 1000 ms.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    sim.state.players[1].set_resource(0, 0);
    sim.state.players[0].set_resource(0, 0);
    let hh = tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (10, 10));
    {
        let h = headhunter_mut(&mut sim, hh).unwrap();
        (h.customer, h.customer_cost, h.cur_reward, h.state) = (1, 300, 350, 2);
    }
    let base = &mut sim.module_mut(hh, INDEX).unwrap().base;
    base.post_delayed_message(500, Message::new(msg::KILL, &[0]), false, GROUP);
    assert_eq!(
        send(&mut sim, hh, &Message::new(msg::HIRE, &[1, 2614, 0])),
        0
    );
    assert_eq!(
        (sim.state.resource(0, 0), sim.state.resource(1, 0)),
        (0, 300)
    );
    let h = headhunter(&sim, hh).unwrap();
    assert_eq!(
        (
            h.customer,
            h.customer_cost,
            h.cur_reward,
            h.target_player,
            h.target_guid,
            h.state
        ),
        (0, 350, 400, 1, 2614, 0)
    );
    let base = &sim.module(hh, INDEX).unwrap().base;
    assert!(!base.has_pending_timer(msg::KILL), "group 0x80 cleared");
    assert!(base.has_pending_timer(msg::THINK));
    // Without a customer (-1) no refund.
    headhunter_mut(&mut sim, hh).unwrap().customer = -1;
    send(&mut sim, hh, &Message::new(msg::HIRE, &[1, 2614, 0]));
    assert_eq!(
        (sim.state.resource(0, 0), sim.state.resource(1, 0)),
        (0, 300)
    );
}

// --- Search, approach, way home and tick (re/headhunter-robbing 3.3, 3.6, 3.10) ---

/// Customer A (slot 0, SPA) and target player B (slot 1, SID).
const A: u8 = 0;
const B: i32 = 1;
/// Steps from the first headhunter tick N1 to the step that delivers `0x322`:
/// `0x122` creates `0x322` (1000 ms) in N1 before `update_events`, due in object tick N1 + 11
/// (re/headhunter-robbing 9.2 "+10 ticks" = ten full ticks in between, timer rule floor(1000/100) + 1); one
/// object tick = two steps.
const THINK_STEPS: usize = 22;

/// The HQ 1000 of A on `tile`; A has 1000 gold.
fn hq_a(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let hq = tk::spawn(sim, tk::HQ_SPA, i32::from(A), tile);
    sim.state.players[usize::from(A)].set_resource(0, 1000);
    hq
}

/// `n` engine steps without commands; returns: their events.
fn steps(sim: &mut Sim, n: usize) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..n {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

/// A hires the SPA headhunter on `guid` of B via command `0x2C`: command in step T = 1,
/// then up to and including the first headhunter tick N1 (the step in which `0x122` reaches the
/// headhunter; it depends on the tick group the new headhunter joins). Returns: headhunter and N1
/// (counted from the command step = 1).
fn hire_by_command(sim: &mut Sim, hq: ObjectId, guid: u32) -> (ObjectId, usize) {
    hire_art(sim, hq, 0, guid)
}

/// Like [`hire_by_command`] with the headhunter of kind `art` (0 SPA, 1 PRI, 2 SID).
fn hire_art(sim: &mut Sim, hq: ObjectId, art: i32, guid: u32) -> (ObjectId, usize) {
    let cmd = Command {
        player: A,
        kind: tk::hire_command(art, B, guid as i32, hq),
    };
    crate::tick::step(sim.state, sim.rules, &[cmd]);
    let hh = sim.state.headhunters[art as usize].expect("created in the command step");
    let mut n1 = 1;
    while headhunter(sim, hh).unwrap().customer != i32::from(A) {
        steps(sim, 1);
        n1 += 1;
        assert!(n1 <= 2, "no tick of the headhunter");
    }
    (hh, n1)
}

/// A timer `code` of the headhunter is queued.
fn pending(sim: &Sim, hh: ObjectId, code: u32) -> bool {
    sim.module(hh, INDEX).unwrap().base.has_pending_timer(code)
}

/// Success and fault feedback of the walk (module 0) as codes.
fn replies(sim: &Sim, hh: ObjectId) -> (Option<u32>, Option<u32>) {
    let b = &sim.module(hh, 0).unwrap().base;
    let code = |m: &Option<Message>| m.as_ref().map(|m| m.code);
    (code(&b.success), code(&b.fault))
}

/// `(state, target_hid, last_target)` of the headhunter.
fn hunt_state(sim: &Sim, hh: ObjectId) -> (i32, ObjectId, ObjectId) {
    let h = headhunter(sim, hh).unwrap();
    (h.state, h.target_hid, h.last_target)
}

/// Place the headhunter at the center of the tile `tile` (test setup: far from or near certain buildings).
fn put(sim: &mut Sim, id: ObjectId, tile: (i32, i32)) {
    let mut p = sim.object(id).unwrap().grid;
    p.x.0 = (tile.0 << 12) + 0x800;
    p.z.0 = (tile.1 << 12) + 0x800;
    set_grid_position(sim, id, p, true);
}

fn moving(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).unwrap().status & status::MOVE != 0
}

fn walk_of<'s>(sim: &'s Sim, id: ObjectId) -> &'s crate::module::walk::Walk {
    crate::module::walk::walk(sim, id).expect("walk")
}

// QUIRK C-312, C-324: kept
#[test]
fn the_hunter_searches_one_second_after_hiring() {
    // 9.2: measured T = N1 = step 1 (command); `0x322` in object tick
    // N1 + 11 = step N1 + 22. Search (`0x00518490`) -> FINDTO_UNIT `{target, 0, 4096, 0, -1, 1, 0,
    // 0}` with `0x822`/`0x922` to itself, state 1 (unit, 3.3).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 30));
    let (hh, n1) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    // Measured: N1 = T = step 1 - in this world (HQ and sheriff) the new headhunter joins the
    // tick group that still ticks in the command step (in a world with five objects: step 2).
    assert_eq!(n1, 1, "N1 measured");
    steps(&mut sim, THINK_STEPS - 1);
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
    assert!(pending(&sim, hh, msg::THINK), "step N1 + 21: still queued");
    steps(&mut sim, 1);
    assert!(!pending(&sim, hh, msg::THINK), "step N1 + 22: delivered");
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
    let w = walk_of(&sim, hh);
    assert_eq!(
        (w.quarry, w.min_attack_radius, w.max_attack_radius),
        (sheriff, 0, NEAR)
    );
    assert_eq!(
        replies(&sim, hh),
        (Some(msg::WALK_OK), Some(msg::WALK_FAULT))
    );
    let b = &sim.module(hh, 0).unwrap().base;
    assert_eq!((b.success_target, b.fault_target), (hh, hh));
    assert!(moving(&sim, hh));
    // On the way (movement, no timers) the tick does not send it home (3.10).
    tk::run_ticks(&mut sim, 5);
    assert!(sim.module(hh, INDEX).unwrap().base.events.is_empty());
    assert_eq!(
        replies(&sim, hh),
        (Some(msg::WALK_OK), Some(msg::WALK_FAULT))
    );
}

// QUIRK C-312: kept
#[test]
fn the_hunter_takes_the_next_object_of_the_guid() {
    // C-312: the search takes the **nearest** object with target GUID and target owner (MYLIST
    // against the target player); a nearer 2615 of A does not count. After a change of owner (new
    // ID at slot 2) the next search no longer finds the old target.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    tk::spawn(&mut sim, tk::UNDERTAKER, i32::from(A), (12, 16));
    let near = tk::spawn(&mut sim, tk::UNDERTAKER, B, (24, 16));
    let far = tk::spawn(&mut sim, tk::UNDERTAKER, B, (50, 50));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::UNDERTAKER);
    steps(&mut sim, THINK_STEPS);
    assert_eq!(hunt_state(&sim, hh), (1, near, near));
    let moved = change_owner(&mut sim, near, 2).expect("change of owner");
    assert_ne!(moved, near);
    // New search as after `retry` (state 0, `0x322`).
    headhunter_mut(&mut sim, hh).unwrap().state = 0;
    assert_eq!(send(&mut sim, hh, &Message::new(msg::THINK, &[])), 1);
    assert_eq!(hunt_state(&sim, hh), (1, far, far));
    assert_eq!(walk_of(&sim, hh).quarry, far);
}

// QUIRK C-312: kept
#[test]
fn the_search_ignores_distance_and_fog() {
    // C-312: whole map (radius -1), filter without `MM_CHECKFOG`: the target at the other end of
    // the map in the fog of the customer is found.
    let rules = tk::hunter_rules();
    let mut state = tk::state_fogged(&rules);
    for (slot, nation) in [(0, 2), (1, 3)] {
        state.players[slot].nation = nation;
    }
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (4, 4));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (60, 60));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    steps(&mut sim, THINK_STEPS);
    let fog = &sim.state.fog;
    assert_eq!(fog.seen.get(60, 60).unwrap() & 1, 0, "not seen by A");
    assert_eq!(
        fog.explored.get(60, 60).unwrap() & 1,
        0,
        "not explored by A"
    );
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
    assert_eq!(walk_of(&sim, hh).quarry, sheriff);
}

// QUIRK C-312: kept
#[test]
fn a_quartered_target_makes_the_building_the_target() {
    // C-312: the sheriff is in the HQ 2500 of B via `quarter::accept_unit` (modules 3 + 23). The
    // search finds him (`MM2_DONOTIGNOREQUARTERED`), notes him in `target_hid`, but takes
    // the container (`GameObject_GetQuarteringContainer`) as target: distance 4 * 4096,
    // state 2.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let hq_b = tk::spawn(&mut sim, tk::HQ_SID, B, (40, 40));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 45));
    assert_eq!(quarter::accept_unit(&mut sim, hq_b, sheriff), 0);
    assert_ne!(sim.object(sheriff).unwrap().status & status::QUARTERED, 0);
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    steps(&mut sim, THINK_STEPS);
    assert_eq!(hunt_state(&sim, hh), (2, sheriff, hq_b));
    let w = walk_of(&sim, hh);
    assert_eq!((w.quarry, w.max_attack_radius), (hq_b, FAR));
    assert_eq!(
        replies(&sim, hh),
        (Some(msg::WALK_OK), Some(msg::WALK_FAULT))
    );
}

// QUIRK C-316: kept
#[test]
fn no_target_sends_the_hunter_home() {
    // C-316: without an object of the GUID at the target player -> FINDTO `{building, -1, 1}` to
    // the nearest building of **any** owner (OR filter only with module 3; here the monastery on
    // slot 8, nearer than the own HQ and the HQ of B; the filter does not check "unfinished"
    // either, re/headhunter-robbing 3.6), success and fault `0x522`. Arrival -> `0x522` -> `QueueObjectDestroy`,
    // destroyed in the phase-0 step without dying; table entry empty, no gold back.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    tk::spawn(&mut sim, tk::HQ_SID, B, (30, 40));
    let monastery = tk::spawn(&mut sim, tk::MONASTERY, 8, (50, 12));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 30));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    assert_eq!(sim.state.resource(0, 0), 700);
    put(&mut sim, hh, (44, 12));
    queue_destroy(&mut sim, sheriff);
    steps(&mut sim, 2);
    assert!(sim.object(sheriff).is_none());
    steps(&mut sim, THINK_STEPS - 2);
    assert!(!pending(&sim, hh, msg::THINK));
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
    assert_eq!(walk_of(&sim, hh).dest, monastery);
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
    assert!(moving(&sim, hh));
    // Until arrival: way home. The arrival sends `0x522` (feedback consumed) and
    // queues the destruction; it follows in the next phase-0 step.
    let mut n = 0;
    let mut events = loop {
        let ev = steps(&mut sim, 1);
        n += 1;
        let Some(o) = sim.object(hh) else { break ev };
        assert_eq!(o.status & status::DIED, 0, "without dying");
        if replies(&sim, hh) == (None, None) {
            assert!(sim.state.destroy_queue.contains(&hh), "arrived");
            assert_eq!(sim.state.phase, 1);
            break ev;
        }
        assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
        assert!(n < 400, "not arrived");
    };
    if sim.object(hh).is_some() {
        events = steps(&mut sim, 1);
        assert!(sim.object(hh).is_none());
    }
    assert_eq!(sim.state.phase, 0, "destruction in the phase-0 step");
    assert!(events.contains(&SimEvent::Destroyed { id: hh }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::TextMessage { .. })),
        "no message"
    );
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert_eq!(sim.state.resource(0, 0), 700, "no refund");
}

// QUIRK C-316: kept
#[test]
fn without_buildings_the_hunter_vanishes_at_once() {
    // C-316: hire, then remove HQ and target (phase 0) before `0x322`. Without a
    // building: debug text "vanishing directly", `0x522` to itself without FINDTO; destroyed in the
    // next phase-0 step (measured: N1 = step 1, the headhunter ticks in the steps of
    // phase 1; `0x322` in step N1 + 22 queues, step N1 + 23 destroys).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 30));
    let (hh, n1) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    assert_eq!(n1, 1, "N1 measured");
    queue_destroy(&mut sim, hq);
    queue_destroy(&mut sim, sheriff);
    steps(&mut sim, 2);
    assert!(sim.object(hq).is_none() && sim.object(sheriff).is_none());
    steps(&mut sim, THINK_STEPS - 3);
    assert!(pending(&sim, hh, msg::THINK));
    assert!(!moving(&sim, hh));
    assert_eq!(replies(&sim, hh), (None, None));
    steps(&mut sim, 1);
    assert!(!pending(&sim, hh, msg::THINK));
    assert_eq!(sim.state.phase, 1);
    // Queued twice: after `0x322` (in `update_events`) the tick check finds the headhunter
    // without movement and timers and calls the way home once more (3.10) - likewise in the original
    // (`QueueObjectDestroy` without a duplicate check, the second resolving fails).
    assert_eq!(sim.state.destroy_queue, [hh, hh], "immediately 0x522");
    assert!(!moving(&sim, hh), "no FINDTO");
    assert_eq!(replies(&sim, hh), (None, None));
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
    let events = steps(&mut sim, 1);
    assert!(
        sim.object(hh).is_none(),
        "destroyed in the next phase-0 step"
    );
    assert_eq!(sim.state.phase, 0);
    assert!(events.contains(&SimEvent::Destroyed { id: hh }));
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert_eq!(sim.state.resource(0, 0), 700, "no refund");
}

// QUIRK C-324: kept
#[test]
fn an_idle_hunter_goes_home() {
    // C-324 (3.10): after `update_events` without `MOVE`, without `ANIM_LOCKED` and without
    // timers (`m_Events` empty) -> way home. Timers removed via `clear_event_group(0x80)`
    // (takes effect in the next `update_events`).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    put(&mut sim, hh, (20, 10));
    // With a timer: stands and stays.
    tk::run_ticks(&mut sim, 1);
    assert!(!moving(&sim, hh));
    assert_eq!(replies(&sim, hh), (None, None));
    // Lock without a timer: stays.
    sim.module_mut(hh, INDEX)
        .unwrap()
        .base
        .clear_event_group(GROUP);
    sim.object_mut(hh)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, true);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.module(hh, INDEX).unwrap().base.events.is_empty());
    assert_eq!(replies(&sim, hh), (None, None));
    // Without a lock: next tick way home to the HQ.
    sim.object_mut(hh)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, false);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
    assert_eq!(walk_of(&sim, hh).dest, hq);
    assert!(moving(&sim, hh));
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
}

// QUIRK C-317: kept
#[test]
fn think_resets_a_busy_state() {
    // C-317 (search part): `0x322` with state 1 (or 2) -> `retry` (`0x00517F60`): state 0 and
    // new `0x322` after 2000 ms (group 0x80), returns 1. A walk success in state 0 fizzles
    // (returns 0), a walk fault in state 0 calls `retry`.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    sim.module_mut(hh, INDEX)
        .unwrap()
        .base
        .clear_event_group(GROUP);
    headhunter_mut(&mut sim, hh).unwrap().state = 1;
    assert_eq!(send(&mut sim, hh, &Message::new(msg::THINK, &[])), 1);
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
    assert!(pending(&sim, hh, msg::THINK));
    // Walk success in state 0: nothing.
    assert_eq!(send(&mut sim, hh, &Message::new(msg::WALK_OK, &[])), 0);
    assert_eq!(hunt_state(&sim, hh), (0, ObjectId::NONE, ObjectId::NONE));
    // Newly created is only the one timer from `retry` (the old `0x322` in `m_Events` falls
    // with the queued group clearing in the next `update_events`).
    let timers = |sim: &Sim| sim.module(hh, INDEX).unwrap().base.timers_ws.len();
    assert_eq!(timers(&sim), 1);
    // `0x322` from `retry` created between two ticks: the next tick takes it over first
    // (step D), from the following one it counts; due in the 22nd tick (1 + floor(2000/100) + 1) - measured.
    tk::run_ticks(&mut sim, 21);
    assert!(pending(&sim, hh, msg::THINK));
    assert_eq!(hunt_state(&sim, hh).0, 0);
    tk::run_ticks(&mut sim, 1);
    assert!(!pending(&sim, hh, msg::THINK));
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
    // State 2 resets likewise; walk fault in state 0 -> `retry` (returns 0).
    headhunter_mut(&mut sim, hh).unwrap().state = 2;
    assert_eq!(send(&mut sim, hh, &Message::new(msg::THINK, &[])), 1);
    assert_eq!(hunt_state(&sim, hh).0, 0);
    sim.module_mut(hh, INDEX)
        .unwrap()
        .base
        .clear_event_group(GROUP);
    assert_eq!(send(&mut sim, hh, &Message::new(msg::WALK_FAULT, &[])), 0);
    assert_eq!(hunt_state(&sim, hh).0, 0);
    assert_eq!(timers(&sim), 1);
    assert!(pending(&sim, hh, msg::THINK));
}

// QUIRK C-324: kept
#[test]
fn rehiring_on_the_way_home_cancels_the_vanish() {
    // Hypothesis (3.6): rehiring on the way home -> `0x122` with refund to the
    // customer (>= 0), `0x322` after 1 s; the new FINDTO_UNIT silently replaces the feedback `0x522`
    // (`take_replies`), the headhunter does not vanish. The HQ is so far away that the arrival would be
    // after N1' + 11.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (5, 5));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (58, 8));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    assert_eq!(sim.state.resource(0, 0), 700);
    // Way home via the empty tick (C-324), far from all buildings.
    put(&mut sim, hh, (55, 58));
    sim.module_mut(hh, INDEX)
        .unwrap()
        .base
        .clear_event_group(GROUP);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
    assert_eq!(walk_of(&sim, hh).dest, hq);
    // A rehires (same target): pays 350, gets 300 back.
    let cmd = Command {
        player: A,
        kind: tk::hire_command(0, B, tk::SID_SHERIFF as i32, hq),
    };
    crate::tick::step(sim.state, sim.rules, &[cmd]);
    // First headhunter tick after the command (measured).
    let mut n1 = 1;
    while headhunter(&sim, hh).unwrap().cur_reward != 400 {
        steps(&mut sim, 1);
        n1 += 1;
        assert!(n1 <= 2, "no tick of the headhunter");
    }
    assert_eq!(sim.state.resource(0, 0), 650, "-350 + 300 refund");
    steps(&mut sim, THINK_STEPS - 1);
    assert!(pending(&sim, hh, msg::THINK));
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
    steps(&mut sim, 1);
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
    assert_eq!(
        replies(&sim, hh),
        (Some(msg::WALK_OK), Some(msg::WALK_FAULT))
    );
    tk::run_ticks(&mut sim, 40);
    assert!(sim.object(hh).is_some(), "not vanished");
    assert_eq!(sim.state.headhunters[0], Some(hh));
    // After a success the customer is -1 (melee kill, 3.4; set here): no refund.
    headhunter_mut(&mut sim, hh).unwrap().customer = -1;
    tk::hire(&mut sim, A, 0, B, tk::SID_SHERIFF as i32, hq);
    assert_eq!(sim.state.resource(0, 0), 250, "-400 without refund");
    assert_eq!(headhunter(&sim, hh).unwrap().customer, i32::from(A));
}

// --- Melee kill, success and dynamite throw (re/headhunter-robbing 3.4, 3.5, 9.2) ---

/// The lock `0x800000` of the headhunter is set.
fn locked(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| o.status & status::ANIM_LOCKED != 0)
}

/// Alive (present and not `DIED`; a corpse changes its ID in the phase-0 step).
fn alive(sim: &Sim, id: ObjectId) -> bool {
    sim.state.object(id, true).is_some()
}

fn hp(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).unwrap().attributes.current_value(14, 0)
}

/// Queued `0x322` of the headhunter (newly created and running).
fn thinks(sim: &Sim, hh: ObjectId) -> usize {
    let b = &sim.module(hh, INDEX).unwrap().base;
    b.timers_ws
        .iter()
        .chain(&b.events)
        .filter(|e| e.msg.code == msg::THINK)
        .count()
}

/// The throws in the projectile list.
fn throws(sim: &Sim) -> Vec<crate::combat::projectile::Projectile> {
    sim.state
        .projectiles
        .list
        .iter()
        .filter(|p| p.kind == ProjectileKind::Throw)
        .cloned()
        .collect()
}

/// Single steps until `cond` holds (at most `max`). Returns: count and events.
fn step_until(sim: &mut Sim, max: usize, cond: impl Fn(&Sim) -> bool) -> (usize, Vec<SimEvent>) {
    let mut events = Vec::new();
    for n in 1..=max {
        events.extend(steps(sim, 1));
        if cond(sim) {
            return (n, events);
        }
    }
    panic!("condition not reached after {max} steps");
}

/// Like [`step_until`], then until the end of the object tick (phase-0 step with the projectile tick):
/// the following [`tk::run_ticks`] then count whole object ticks N + 1, N + 2, ...
fn tick_until(sim: &mut Sim, max: usize, cond: impl Fn(&Sim) -> bool) -> Vec<SimEvent> {
    let (_, mut events) = step_until(sim, max, cond);
    if sim.state.phase != 0 {
        events.extend(steps(sim, 1));
    }
    events
}

/// The success message 10011 and the sound `SFX_HEADHUNTERSUCCESS` (3.4, D-238).
fn success_events(events: &[SimEvent]) -> Vec<SimEvent> {
    events
        .iter()
        .filter(|e| {
            matches!(e, SimEvent::Sound2d { name } if name == SFX_SUCCESS)
                || matches!(e, SimEvent::TextMessage { text, .. } if *text == TEXT_SUCCESS)
        })
        .cloned()
        .collect()
}

/// Explosion effects `explosion_pulverhaus.pef` at their position.
fn explosions(events: &[SimEvent]) -> Vec<[f32; 3]> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::EffectAt { path, pos, .. }
                if path == "data/effects/explosion_pulverhaus.pef" =>
            {
                Some(*pos)
            }
            _ => None,
        })
        .collect()
}

// QUIRK C-314, C-315: kept
#[test]
fn the_near_kill_follows_its_timeline() {
    // 9.2, C-314, C-315: walk success in state 1 in object tick N -> `m_KillAnimNear` (lock on),
    // `0x422 {target}` at time marker 1 (group 0x80): SPA animation 1, 1800 ms -> N + 19; PRI
    // animation 4, 1400 ms -> N + 15 (timer rule floor(t/100) + 1). Then `0x27F {-110, headhunter}`,
    // message 10011 and sound to all, price hard 500, customer/target player/target -1, way home (state
    // stays 1); no reward.
    for (art, guid, anim, kill) in [(0, tk::HUNTER_SPA, 1, 19), (1, tk::HUNTER_PRI, 4, 15)] {
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = hq_a(&mut sim, (10, 10));
        let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (16, 16));
        let (hh, _) = hire_art(&mut sim, hq, art, tk::SID_SHERIFF);
        assert_eq!(sim.object(hh).unwrap().guid, guid);
        let gold = sim.state.resource(0, 0);
        steps(&mut sim, THINK_STEPS);
        assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
        // N: the step of the headhunter tick with the walk success.
        tick_until(&mut sim, 400, |s| locked(s, hh));
        assert_eq!(sim.object(hh).unwrap().anim.wanted, anim, "kind {art}");
        assert!(pending(&sim, hh, msg::KILL));
        let full = hp(&sim, sheriff);
        for k in 1..kill {
            tk::run_ticks(&mut sim, 1);
            assert!(locked(&sim, hh), "N + {k}: locked");
            assert_eq!(hp(&sim, sheriff), full, "N + {k}: unharmed");
        }
        let events = tk::run_ticks_collect(&mut sim, 1);
        assert!(!locked(&sim, hh), "N + {kill}: lock off");
        assert!(!pending(&sim, hh, msg::KILL));
        assert!(
            sim.object(sheriff)
                .is_none_or(|o| o.attributes.current_value(14, 0) <= 0),
            "N + {kill}: -110 %"
        );
        assert_eq!(
            success_events(&events),
            [
                SimEvent::TextMessage {
                    to: None,
                    text: TEXT_SUCCESS,
                    args: vec![TextArg::TypeName(guid)],
                    color: COLOR_GREEN,
                    duration_ms: 3000,
                },
                SimEvent::Sound2d {
                    name: SFX_SUCCESS.into()
                },
            ]
        );
        let h = headhunter(&sim, hh).unwrap();
        assert_eq!(
            (
                h.cur_reward,
                h.customer,
                h.target_player,
                h.target_guid,
                h.target_hid,
                h.state
            ),
            (500, -1, -1, -1, ObjectId::NONE, 1)
        );
        assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
        assert_eq!(walk_of(&sim, hh).dest, hq, "way home");
        tk::run_ticks(&mut sim, 1);
        assert!(!alive(&sim, sheriff), "sheriff dead");
        assert_eq!(sim.state.resource(0, 0), gold, "no reward");
    }
}

// QUIRK E-189: kept
#[test]
fn headhunter_success_counts_for_the_customer() {
    // `FUN_00519000` `0x0051901C`: first the kill counter of the customer with the target GUID. The
    // victim then dies via `0x27F {-110, headhunter}`; the headhunter is on slot 8, its slot is
    // discarded on dying - no double counting.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (16, 16));
    let (hh, _) = hire_art(&mut sim, hq, 0, tk::SID_SHERIFF);
    assert_eq!(hh.slot(), crate::id::NEUTRAL_SLOT);
    steps(&mut sim, THINK_STEPS);
    tick_until(&mut sim, 400, |s| locked(s, hh));
    assert_eq!(
        crate::world::kill_count(&sim, i32::from(A), tk::SID_SHERIFF),
        0
    );
    tick_until(&mut sim, 40, |s| !locked(s, hh));
    assert_eq!(headhunter(&sim, hh).unwrap().customer, -1, "success");
    assert_eq!(
        crate::world::kill_count(&sim, i32::from(A), tk::SID_SHERIFF),
        1
    );
    tk::run_ticks(&mut sim, 2);
    assert!(!alive(&sim, sheriff), "sheriff dead");
    assert_eq!(
        crate::world::kill_count(&sim, i32::from(A), tk::SID_SHERIFF),
        1
    );
    let counted: usize = sim.state.kill_counts.iter().map(|c| c.len()).sum();
    assert_eq!(counted, 1, "only the customer counts");
}

// QUIRK C-315, C-319: kept
#[test]
fn a_swimming_hunter_kills_at_once() {
    // C-319: if the headhunter is swimming at the walk success, `0x00517CE0` calls the query with
    // `0x422` immediately - without animation, without lock, without timer. `SWIMMING` by hand, in the same
    // call (on land the swim tick clears the bit every tick).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 40));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    sim.module_mut(hh, INDEX)
        .unwrap()
        .base
        .clear_event_group(GROUP);
    tk::run_ticks(&mut sim, 1);
    {
        let h = headhunter_mut(&mut sim, hh).unwrap();
        (h.state, h.last_target) = (1, sheriff);
    }
    let o = sim.object_mut(hh).unwrap();
    o.set_status_flags(status::SWIMMING, true);
    let wanted = o.anim.wanted;
    let full = hp(&sim, sheriff);
    assert_eq!(send(&mut sim, hh, &Message::new(msg::WALK_OK, &[])), 1);
    assert!(
        hp(&sim, sheriff) < full && hp(&sim, sheriff) <= 0,
        "-110 % immediately"
    );
    assert_eq!(sim.object(sheriff).unwrap().last_attacker, hh);
    assert!(!locked(&sim, hh), "no lock");
    assert_eq!(sim.object(hh).unwrap().anim.wanted, wanted, "no animation");
    assert!(!pending(&sim, hh, msg::KILL), "no timer");
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(success_events(&events).len(), 2, "success");
    let h = headhunter(&sim, hh).unwrap();
    assert_eq!((h.cur_reward, h.customer, h.state), (500, -1, 1));
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
}

/// The building scene: HQ 1000 of A at (10, 10), HQ 2500 of B at (26, 10) with the
/// quartered sheriff, a second sheriff of B free at (50, 50). A hires the SPA headhunter on
/// 2614; after the search it walks to the HQ of B (state 2, 4 tiles). Returns: headhunter, HQ of
/// B, quartered and free sheriff; the world stands at the end of object tick N of the
/// walk success.
struct BuildingScene {
    hh: ObjectId,
    hq_b: ObjectId,
    quartered: ObjectId,
    free: ObjectId,
}

fn building_scene(sim: &mut Sim) -> BuildingScene {
    let hq = hq_a(sim, (10, 10));
    let hq_b = tk::spawn(sim, tk::HQ_SID, B, (26, 10));
    let quartered = tk::spawn(sim, tk::SID_SHERIFF, B, (26, 14));
    assert_eq!(quarter::accept_unit(sim, hq_b, quartered), 0);
    let free = tk::spawn(sim, tk::SID_SHERIFF, B, (50, 50));
    let (hh, _) = hire_by_command(sim, hq, tk::SID_SHERIFF);
    steps(sim, THINK_STEPS);
    assert_eq!(hunt_state(sim, hh), (2, quartered, hq_b));
    tick_until(sim, 400, |s| locked(s, hh));
    BuildingScene {
        hh,
        hq_b,
        quartered,
        free,
    }
}

// QUIRK C-312, C-314, C-317: kept
#[test]
fn a_building_gets_dynamite() {
    // 9.2, 3.5, C-314, C-317: walk success in state 2 (object tick N) -> `m_KillAnimFar` 5, lock;
    // `0xB22` at time marker 2 (760 ms -> N + 8): throw with impact `0xE22`, flight time
    // `floor(d / 0.7)` from the world position of the headhunter to hit zone 0 of the HQ. Impact in
    // object tick W + ceil(t/100) - 1 (W = N + 8, `t` from the entry): explosion at the HQ,
    // `0x27F {-110}`, `retry`. `0xC22` at time marker 1 (1720 ms -> N + 18): lock off, `retry`.
    // Two `0x322` (impact + 22, N + 39): the first searches, the second resets (C-317).
    // No success, price unchanged.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = building_scene(&mut sim);
    let hh = s.hh;
    assert_eq!(sim.object(hh).unwrap().anim.wanted, 5);
    assert!(pending(&sim, hh, msg::THROW_BUILDING));
    assert!(pending(&sim, hh, msg::THROW_END));
    let full = hp(&sim, s.hq_b);
    tk::run_ticks(&mut sim, 7);
    assert!(throws(&sim).is_empty(), "N + 7: no throw yet");
    let events = tk::run_ticks_collect(&mut sim, 1);
    // W = N + 8.
    let [p] = throws(&sim).try_into().expect("one throw");
    let start = world_pos(&sim, hh).unwrap();
    let aim = hit_zone_world_pos(&sim, s.hq_b, 0).unwrap();
    assert_eq!((p.shooter, p.target, p.flags), (hh, ObjectId::NONE, 0x201));
    assert_eq!(p.damage, Message::new(msg::IMPACT_BUILDING, &[0]));
    assert_eq!((p.start, p.target_pos), (start, aim));
    let d = vector3_length([aim[0] - start[0], aim[1] - start[1], aim[2] - start[2]]);
    assert_eq!(p.flight_ms, (d / 0.7).floor() as i32);
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::ProjectileLaunched { kind: ProjectileKind::Throw, fire: false, shooter, model, node, target, .. }
            if *shooter == hh
                && model == "data/r3d/misc/Tools/dynamit/dynamit.rdo"
                && node == "dynamitposition"
                && target.is_none()
    )));
    let starts = sim.object(hh).unwrap().anim.starts;
    assert!(events.contains(&SimEvent::MeshVisibility {
        object: hh,
        names: "cylinder01".into(),
        visible: false,
        anim_starts: starts,
    }));
    // Impact in object tick W + ceil(t/100) - 1.
    let k = (p.flight_ms + 99) / 100;
    let impact = 8 + k - 1;
    // Measured: the approach ends at d ~ 4.7 tiles, t = 855 ms -> 9 effect ticks, impact N + 16.
    assert_eq!((p.flight_ms, impact), (855, 16), "measured");
    assert!(
        impact < 18,
        "impact before the animation end (N + {impact})"
    );
    tk::run_ticks(&mut sim, (impact - 1 - 8) as usize);
    assert_eq!(throws(&sim).len(), 1, "N + {}: still flying", impact - 1);
    assert_eq!(hp(&sim, s.hq_b), full);
    let events = tk::run_ticks_collect(&mut sim, 1);
    assert!(throws(&sim).is_empty(), "N + {impact}: impact");
    assert_eq!(explosions(&events), [world_pos(&sim, s.hq_b).unwrap()]);
    assert!(events.contains(&SimEvent::ProjectileEnded {
        id: p.id,
        hit: true,
        pos: aim,
    }));
    assert!(hp(&sim, s.hq_b) <= 0, "-110 %");
    assert_eq!(hunt_state(&sim, hh).0, 0, "retry from 0xE22");
    assert_eq!(thinks(&sim, hh), 1);
    assert!(locked(&sim, hh));
    tk::run_ticks(&mut sim, (17 - impact) as usize);
    assert!(locked(&sim, hh), "N + 17: locked");
    tk::run_ticks(&mut sim, 1);
    assert!(!locked(&sim, hh), "N + 18: 0xC22");
    assert!(!pending(&sim, hh, msg::THROW_END));
    assert_eq!(thinks(&sim, hh), 2, "two 0x322 (C-317)");
    // First `0x322` in impact + 22 (created from the projectile tick after `update_events`).
    let first = impact + 22;
    tk::run_ticks(&mut sim, (first - 1 - 18) as usize);
    assert_eq!(thinks(&sim, hh), 2, "N + {}: both queued", first - 1);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(thinks(&sim, hh), 1, "N + {first}: first search");
    // The HQ is destroyed, the sheriff free: the search takes the next object of the GUID, now as a
    // unit (state 1).
    assert!(!alive(&sim, s.hq_b), "HQ destroyed");
    let o = sim.object(s.quartered).expect("sheriff alive");
    assert_eq!(o.status & status::QUARTERED, 0, "no longer quartered");
    assert_eq!(hunt_state(&sim, hh), (1, s.quartered, s.quartered));
    assert_eq!(walk_of(&sim, hh).quarry, s.quartered);
    // Second `0x322` in N + 39 (`retry` from `0xC22` in `update_events`, N + 18 + 21): state 1 ->
    // `retry` (C-317), state 0, third `0x322` 2 s later.
    tk::run_ticks(&mut sim, (38 - first) as usize);
    assert_eq!(hunt_state(&sim, hh).0, 1, "N + 38");
    tk::run_ticks(&mut sim, 1);
    assert_eq!(hunt_state(&sim, hh).0, 0, "N + 39: reset");
    assert_eq!(thinks(&sim, hh), 1);
    assert_ne!(hunt_state(&sim, hh).1, s.free, "the nearer one");
    let h = headhunter(&sim, hh).unwrap();
    assert_eq!(
        (h.cur_reward, h.customer),
        (350, i32::from(A)),
        "no success"
    );
}

// QUIRK C-313, C-314: kept
#[test]
fn an_unreachable_target_gets_dynamite_from_afar() {
    // C-313: the sheriff stands on an island (inner area of `wall_ring`). The
    // FINDTO_UNIT fails (`0x922` in state 1) -> throw animation, `0xA22` at time marker 2 from
    // any distance, impact `0xD22` kills (-110 %). No success (C-314): no
    // message, price and customer stay; `0xD22` does not retry, only `0xC22` (N + 18 -> `0x322`
    // in N + 39) searches on.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    tk::wall_ring(&mut state, (30, 10), 6, 7);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (30, 10));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    steps(&mut sim, THINK_STEPS);
    assert_eq!(hunt_state(&sim, hh).2, sheriff);
    let mut events = tick_until(&mut sim, 400, |s| locked(s, hh));
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff), "state 1");
    assert_eq!(sim.object(hh).unwrap().anim.wanted, 5, "m_KillAnimFar");
    assert!(pending(&sim, hh, msg::THROW_UNIT));
    tk::run_ticks(&mut sim, 7);
    assert!(throws(&sim).is_empty());
    tk::run_ticks(&mut sim, 1);
    let [p] = throws(&sim).try_into().expect("one throw in N + 8");
    assert_eq!(p.damage, Message::new(msg::IMPACT_UNIT, &[0]));
    let tiles = |v: f32| v / 128.0;
    let d = vector3_length([
        p.target_pos[0] - p.start[0],
        p.target_pos[1] - p.start[1],
        p.target_pos[2] - p.start[2],
    ]);
    assert!(tiles(d) > 7.0, "from a distance: {} tiles", tiles(d));
    let impact = 8 + (p.flight_ms + 99) / 100 - 1;
    let full = hp(&sim, sheriff);
    tk::run_ticks(&mut sim, (impact - 1 - 8) as usize);
    assert_eq!(hp(&sim, sheriff), full, "N + {}: unharmed", impact - 1);
    events.extend(tk::run_ticks_collect(&mut sim, 1));
    assert!(throws(&sim).is_empty(), "N + {impact}: impact");
    assert!(hp(&sim, sheriff) <= 0, "-110 %");
    assert_eq!(sim.object(sheriff).unwrap().last_attacker, hh);
    tk::run_ticks(&mut sim, 1);
    assert!(!alive(&sim, sheriff), "dead");
    events.extend(tk::run_ticks_collect(&mut sim, 30));
    assert!(success_events(&events).is_empty(), "no success");
    let h = headhunter(&sim, hh).unwrap();
    assert_eq!((h.cur_reward, h.customer), (350, i32::from(A)));
    // Measured: the approach ends at the ring, throw over about 8 tiles, t = 1462 ms -> N + 22.
    assert_eq!((p.flight_ms, impact), (1462, 22), "measured");
}

// QUIRK C-317: kept
#[test]
fn a_vanished_target_is_retried() {
    // Hypothesis (11): the target dies during the approach through third parties. The walk aborts
    // (`update_seek_target`: dead target -> `0x922`) in state 1 -> throw animation and lock start
    // up; `0xA22` no longer finds the old ID after the change of owner of the corpse -> no throw,
    // `retry`; `0xC22` calls `retry` once more. Two `0x322` (N + 8 + 21, N + 18 + 21), the first
    // searches the next object of the GUID.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (30, 12));
    let other = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    steps(&mut sim, THINK_STEPS);
    assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
    tk::run_ticks(&mut sim, 3);
    assert!(moving(&sim, hh), "in the approach");
    send(&mut sim, sheriff, &Message::new(0x27F, &[-110]));
    tick_until(&mut sim, 20, |s| locked(s, hh));
    assert!(!alive(&sim, sheriff));
    assert_eq!(
        hunt_state(&sim, hh),
        (1, sheriff, sheriff),
        "0x922 in state 1"
    );
    assert_eq!(sim.object(hh).unwrap().anim.wanted, 5);
    tk::run_ticks(&mut sim, 8);
    assert!(sim.object(sheriff).is_none(), "old ID gone");
    assert!(throws(&sim).is_empty(), "no throw into the void");
    assert_eq!(hunt_state(&sim, hh).0, 0, "N + 8: retry");
    assert_eq!(thinks(&sim, hh), 1);
    tk::run_ticks(&mut sim, 10);
    assert!(!locked(&sim, hh), "N + 18: lock off");
    assert_eq!(thinks(&sim, hh), 2);
    tk::run_ticks(&mut sim, 10);
    assert_eq!(hunt_state(&sim, hh).0, 0, "N + 28");
    tk::run_ticks(&mut sim, 1);
    assert_eq!(hunt_state(&sim, hh), (1, other, other), "N + 29: search");
    assert_eq!(thinks(&sim, hh), 1);
    tk::run_ticks(&mut sim, 10);
    assert_eq!(hunt_state(&sim, hh).0, 0, "N + 39: reset");
}

// QUIRK C-318: kept
#[test]
fn a_takeover_mid_throw_keeps_the_flying_dynamite() {
    // C-318 (3.2): B takes over the headhunter in the object tick after the throw (N + 9) with
    // another target (governor of A). `0x122` clears group `0x80` (`0xC22` gone), the lock
    // stays; the flying dynamite hits before the new search (N1' + 11) and hits
    // the old `last_target` (HQ of B, -110 %). The lock only falls with the next
    // animation end (measured: `0x422` of the melee kill on the new target).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let governor = tk::spawn(&mut sim, tk::GOVERNOR, i32::from(A), (20, 18));
    let s = building_scene(&mut sim);
    let hh = s.hh;
    sim.state.players[1].set_resource(0, 1000);
    tk::run_ticks(&mut sim, 8);
    let [p] = throws(&sim).try_into().expect("throw in N + 8");
    let impact = 8 + (p.flight_ms + 99) / 100 - 1;
    assert_eq!(impact, 16, "measured as in a_building_gets_dynamite");
    // N + 9: B hires (command at the start of the step, `0x122` in the headhunter tick of the same object tick).
    let cmd = Command {
        player: 1,
        kind: tk::hire_command(0, i32::from(A), tk::GOVERNOR as i32, s.hq_b),
    };
    crate::tick::step(sim.state, sim.rules, &[cmd]);
    steps(&mut sim, 1);
    let h = headhunter(&sim, hh).unwrap();
    assert_eq!(
        (h.customer, h.target_player, h.target_guid, h.state),
        (1, i32::from(A), tk::GOVERNOR as i32, 0),
        "N + 9: taken over"
    );
    assert_eq!(sim.state.resource(1, 0), 650, "B pays 350");
    assert_eq!(sim.state.resource(0, 0), 1000, "A: 700 + 300 back");
    assert!(!pending(&sim, hh, msg::THROW_END), "group 0x80 cleared");
    assert!(locked(&sim, hh), "lock stays");
    assert_eq!(throws(&sim).len(), 1, "dynamite keeps flying");
    assert_eq!(hunt_state(&sim, hh).2, s.hq_b);
    let full = hp(&sim, s.hq_b);
    tk::run_ticks(&mut sim, (impact - 1 - 9) as usize);
    assert_eq!(hp(&sim, s.hq_b), full);
    let events = tk::run_ticks_collect(&mut sim, 1);
    assert!(throws(&sim).is_empty(), "N + {impact}: impact");
    assert!(impact < 20, "before the new search");
    assert_eq!(explosions(&events).len(), 1);
    assert!(hp(&sim, s.hq_b) <= 0, "old target -110 %");
    assert!(locked(&sim, hh));
    // Until N + 20 (`0x322` of the takeover): locked, state 0; then search for the governor.
    tk::run_ticks(&mut sim, (19 - impact) as usize);
    assert!(locked(&sim, hh), "N + 19: locked");
    assert_eq!(hunt_state(&sim, hh).0, 0);
    tk::run_ticks(&mut sim, 1);
    assert_eq!(
        hunt_state(&sim, hh),
        (1, governor, governor),
        "N + 20: search"
    );
    assert!(locked(&sim, hh), "lock without animation");
    let mut tick = 20;
    let mut events = Vec::new();
    while locked(&sim, hh) {
        events = tk::run_ticks_collect(&mut sim, 1);
        tick += 1;
        assert!(tick < 200, "lock stays");
    }
    // Measured: the lock falls in N + 84 with the `0x422` of the melee kill on the governor (in between
    // the double `0x322` from `0xE22` and the approach, C-317); success for B.
    assert_eq!(tick, 84, "measured");
    assert!(hp(&sim, governor) <= 0, "governor -110 %");
    assert_eq!(success_events(&events).len(), 2);
    assert_eq!(headhunter(&sim, hh).unwrap().customer, -1);
}

#[test]
fn a_throw_without_its_hunter_does_nothing() {
    // re/combat-projectiles 2.2: if the thrower is missing at the impact, the entry ends without a
    // message (`ProjectileEnded { hit: false }`): no explosion, no `0x27F` at the target. The headhunter
    // is destroyed one object tick before the impact (projectiles tick before the
    // destroy list).
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let s = building_scene(&mut sim);
    tk::run_ticks(&mut sim, 8);
    let [p] = throws(&sim).try_into().expect("throw in N + 8");
    let impact = 8 + (p.flight_ms + 99) / 100 - 1;
    assert_eq!(impact, 16, "measured as in a_building_gets_dynamite");
    let full = hp(&sim, s.hq_b);
    queue_destroy(&mut sim, s.hh);
    tk::run_ticks(&mut sim, 1);
    assert!(sim.object(s.hh).is_none(), "N + 9: headhunter gone");
    assert_eq!(sim.state.headhunters, [None; 3]);
    let events = tk::run_ticks_collect(&mut sim, (impact - 9) as usize);
    assert!(throws(&sim).is_empty(), "N + {impact}: entry removed");
    assert!(events.contains(&SimEvent::ProjectileEnded {
        id: p.id,
        hit: false,
        pos: p.target_pos,
    }));
    assert!(explosions(&events).is_empty());
    assert_eq!(hp(&sim, s.hq_b), full, "no 0x27F");
    assert!(sim.object(s.hq_b).unwrap().last_attacker.is_none());
}

#[test]
fn throws_survive_saving_and_loading() {
    // Savegame in the middle of the flight (N + 10): same checksum sequence over 60 steps, the impact
    // also hits after loading.
    use crate::save::{SaveGame, checksum};
    use crate::state::SimState;
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let hq_b = {
        let mut sim = Sim::new(&mut state, &rules);
        let s = building_scene(&mut sim);
        tk::run_ticks(&mut sim, 10);
        assert_eq!(throws(&sim).len(), 1, "in flight");
        s.hq_b
    };
    let save = SaveGame::from_bytes(&crate::save::testkit::save_bytes(&state, &rules)).unwrap();
    save.check(&rules).unwrap();
    let mut loaded: SimState = save.state;
    assert_eq!(checksum(&loaded), checksum(&state));
    for i in 1..=60 {
        crate::tick::step(&mut state, &rules, &[]);
        crate::tick::step(&mut loaded, &rules, &[]);
        assert_eq!(
            checksum(&loaded),
            checksum(&state),
            "step {i} after loading"
        );
    }
    let sim = Sim::new(&mut loaded, &rules);
    assert!(sim.state.projectiles.list.is_empty());
    assert!(!alive(&sim, hq_b), "impact after loading");
}

#[test]
fn a_hunter_killed_on_the_way_locks_its_corpse() {
    // Measurement for re/headhunter-robbing 11: the headhunter dies during the approach (state 1)
    // from `0x27F {-110}`. In the death tick `AbortAllModules` aborts the walk; `PropertyWalk_Abort`
    // sends the fault `0x922` to the still enabled module 34 -> throw animation as with an
    // unreachable target (C-313): lock `0x800000`, `0xA22`/`0xC22` queued; the dying animation
    // replaces the throw animation right after. In the tick after death module 34 ticks once more
    // (C-150): the lock blocks the tick condition, no way home for the corpse - with or without a
    // building. After that the module is off, the timers stay around (no throw), the corpse stays
    // locked and registered; hiring is then free and lapses (C-338).
    for with_building in [true, false] {
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = hq_a(&mut sim, (10, 10));
        let sheriff = tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
        let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
        steps(&mut sim, THINK_STEPS);
        assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
        if !with_building {
            queue_destroy(&mut sim, hq);
        }
        tk::run_ticks(&mut sim, 3);
        assert_eq!(sim.object(hq).is_some(), with_building);
        assert!(moving(&sim, hh), "in the approach");
        send(&mut sim, hh, &Message::new(0x27F, &[-110]));
        let dead = |s: &Sim| s.object(hh).is_some_and(|o| o.status & status::DIED != 0);
        tick_until(&mut sim, 4, dead);
        let o = sim.object(hh).unwrap();
        assert!(!moving(&sim, hh));
        assert!(locked(&sim, hh), "lock from the throw");
        assert_eq!(o.anim.wanted, 4, "dying animation replaces the throw");
        assert_eq!(
            replies(&sim, hh),
            (None, None),
            "feedback messages consumed"
        );
        assert!(pending(&sim, hh, msg::THROW_UNIT) && pending(&sim, hh, msg::THROW_END));
        assert_eq!(hunt_state(&sim, hh), (1, sheriff, sheriff));
        // The tick after death (C-150): module 34 still in the tick list, then off.
        assert!(o.tick_list.contains(&(INDEX as u8)));
        tk::run_ticks(&mut sim, 1);
        let o = sim.object(hh).unwrap();
        assert!(
            !o.tick_list.contains(&(INDEX as u8)),
            "module 34 out of the tick"
        );
        assert!(!sim.module(hh, INDEX).unwrap().base.enabled);
        assert_eq!(
            replies(&sim, hh),
            (None, None),
            "no way home (case {with_building})"
        );
        assert!(sim.state.destroy_queue.is_empty(), "not vanished");
        // The timers no longer run: no throw, the lock stays.
        tk::run_ticks(&mut sim, 50);
        assert!(throws(&sim).is_empty(), "no throw");
        assert!(locked(&sim, hh));
        assert!(pending(&sim, hh, msg::THROW_UNIT));
        assert!(sim.object(hh).is_some());
        assert_eq!(sim.state.headhunters, [Some(hh), None, None]);
        assert!(alive(&sim, sheriff));
        if with_building {
            // C-338: hiring the dead headhunter is free and lapses.
            let gold = sim.state.resource(0, 0);
            tk::hire(&mut sim, A, 0, B, tk::SID_SHERIFF as i32, hq);
            assert_eq!(sim.state.resource(0, 0), gold, "free");
            assert_eq!(sim.state.headhunters, [Some(hh), None, None]);
            assert_eq!(headhunter(&sim, hh).unwrap().cur_reward, 350, "no takeover");
        }
    }
}

// --- Death on the way home, locked corpse in the savegame ---

#[test]
fn a_hunter_killed_on_the_way_home_vanishes() {
    // Measurement for re/headhunter-robbing 11: after the success the headhunter walks home (FINDTO with `0x522` as
    // success and fault, 3.6). If it dies on the way, `AbortAllModules` aborts the walk in the death tick;
    // `PropertyWalk_Abort` sends the fault `0x522` to the still enabled module 34 -> `vanish`
    // (destroy list). In the next phase-0 step the corpse is destroyed, the destructor clears
    // table and sight source - C-338 does not apply: the next hire creates a new headhunter at
    // the config 12 price.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    tk::spawn(&mut sim, tk::SID_SHERIFF, B, (16, 16));
    let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    // Measured: the headhunter ticks in group 0 (phase-1 step); so a step boundary lies between
    // death and destruction. In group 1 both fall into the same phase-0 step.
    assert!(sim.state.tick_groups[0].contains(&hh));
    steps(&mut sim, THINK_STEPS);
    tick_until(&mut sim, 400, |s| locked(s, hh));
    // Melee kill in N + 19 (SPA, `the_near_kill_follows_its_timeline`), then way home.
    tk::run_ticks(&mut sim, 19);
    assert_eq!(headhunter(&sim, hh).unwrap().customer, -1, "success");
    assert_eq!(replies(&sim, hh), (Some(msg::VANISH), Some(msg::VANISH)));
    tk::run_ticks(&mut sim, 3);
    assert!(moving(&sim, hh), "on the way home");
    send(&mut sim, hh, &Message::new(0x27F, &[-110]));
    let dead = |s: &Sim| s.object(hh).is_some_and(|o| o.status & status::DIED != 0);
    step_until(&mut sim, 4, dead);
    // Death step: walk aborted, `0x522` -> destroy list; still registered.
    assert!(!moving(&sim, hh));
    assert_eq!(
        replies(&sim, hh),
        (None, None),
        "feedback messages consumed"
    );
    assert!(
        sim.module(hh, INDEX).unwrap().base.enabled,
        "module 34 still on"
    );
    assert_eq!(sim.state.destroy_queue, [hh], "0x522 -> vanish");
    assert_eq!(sim.state.headhunters, [Some(hh), None, None]);
    // Next phase-0 step: destroyed.
    assert_eq!(sim.state.phase, 1, "death in the phase-1 step");
    steps(&mut sim, 1);
    assert_eq!(sim.state.phase, 0);
    assert!(sim.state.object(hh, false).is_none(), "corpse destroyed");
    assert_eq!(sim.state.headhunters, [None; 3], "table empty");
    assert!(sources_of(&sim, hh).is_empty());
    // New hire (on a second sheriff): new headhunter at the config 12 price 300 (not
    // free as with C-338).
    tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
    assert_eq!(crate::command::headhunter::hire_price(&sim, 0), 300);
    let gold = sim.state.resource(0, 0);
    tk::hire(&mut sim, A, 0, B, tk::SID_SHERIFF as i32, hq);
    assert_eq!(sim.state.resource(0, 0), gold - 300, "config 12");
    let fresh = sim.state.headhunters[0].expect("new headhunter");
    assert_ne!(fresh, hh);
    let h = headhunter(&sim, fresh).unwrap();
    assert_eq!((h.customer, h.customer_cost, h.cur_reward), (0, 300, 350));
}

#[test]
fn a_locked_hunter_corpse_survives_saving_and_loading() {
    // The headhunter corpse killed in the approach (`a_hunter_killed_on_the_way_locks_its_corpse`:
    // `ANIM_LOCKED`, module 34 off, `0xA22`/`0xC22` lie dormant) is a reachable state. The
    // savegame loads (check against the rules) and the checksum sequence stays the same over 400
    // steps; the corpse stays locked and registered.
    use crate::save::{SaveGame, checksum};
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let hh = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = hq_a(&mut sim, (10, 10));
        tk::spawn(&mut sim, tk::SID_SHERIFF, B, (50, 50));
        let (hh, _) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
        steps(&mut sim, THINK_STEPS);
        tk::run_ticks(&mut sim, 3);
        assert!(moving(&sim, hh), "in the approach");
        send(&mut sim, hh, &Message::new(0x27F, &[-110]));
        let dead = |s: &Sim| s.object(hh).is_some_and(|o| o.status & status::DIED != 0);
        tick_until(&mut sim, 4, dead);
        tk::run_ticks(&mut sim, 10);
        assert!(locked(&sim, hh));
        assert!(!sim.module(hh, INDEX).unwrap().base.enabled);
        assert!(pending(&sim, hh, msg::THROW_UNIT) && pending(&sim, hh, msg::THROW_END));
        hh
    };
    let save = SaveGame::from_bytes(&crate::save::testkit::save_bytes(&state, &rules))
        .expect("savegame readable");
    save.check(&rules).expect("savegame valid");
    let mut loaded = save.state;
    assert_eq!(checksum(&loaded), checksum(&state));
    for i in 1..=400 {
        crate::tick::step(&mut state, &rules, &[]);
        crate::tick::step(&mut loaded, &rules, &[]);
        assert_eq!(
            checksum(&loaded),
            checksum(&state),
            "step {i} after loading"
        );
    }
    let sim = Sim::new(&mut loaded, &rules);
    assert!(locked(&sim, hh), "corpse locked");
    assert!(pending(&sim, hh, msg::THROW_UNIT) && pending(&sim, hh, msg::THROW_END));
    assert_eq!(sim.state.headhunters, [Some(hh), None, None]);
}

// --- Hiring in the vanish step (C-340) ---

// QUIRK C-340: kept
#[test]
fn hiring_in_the_vanish_step_pays_and_loses_the_order() {
    // C-340 (Ghidra read): `GameWorld_CmdHireHeadhunter` `0x0048A8F0` only queries
    // `GetObjectById(+0x4DC[art], 0)` via `FUN_00477100`, without checking "queued for
    // destruction". `QueueObjectDestroy` `0x0047C7D0` is only `AddTail`; the list is processed at the end of
    // phase 0 in `GameWorld_Tick` `0x0048E710`, only then does the destructor
    // `0x00517B80` run (table empty). A hire in this window pays the price (`0x222` ->
    // `cur_reward`), `0x122` is lost with the destruction; afterwards there is no headhunter, the
    // next price is config 12 again. The window exists only in tick group 0 (headhunter ticks in the
    // phase-1 step, the destruction follows only in the next step); in group 1 the same
    // phase-0 step cleans up before a command arrives.
    let rules = tk::hunter_rules();
    let mut state = tk::hunter_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = hq_a(&mut sim, (10, 10));
    tk::spawn(&mut sim, tk::SID_SHERIFF, B, (40, 30));
    let (hh, n1) = hire_by_command(&mut sim, hq, tk::SID_SHERIFF);
    assert_eq!(n1, 1, "N1 measured");
    assert_eq!(sim.state.resource(0, 0), 700, "300 paid");
    // Measured: group 0; after step N1 the world is in the phase-1 state, the next
    // step (commands first, then phase 0 with the destroy list at the end) is the window.
    assert!(sim.state.tick_groups[0].contains(&hh));
    assert_eq!(sim.state.phase, 1);
    // `0x522` (way home without a building or sent directly): destruction queued, the headhunter stays in
    // the table.
    send(&mut sim, hh, &Message::new(msg::VANISH, &[]));
    assert_eq!(sim.state.destroy_queue, [hh]);
    assert_eq!(sim.state.headhunters[0], Some(hh));
    assert_eq!(crate::command::headhunter::hire_price(&sim, 0), 350);
    // Hire in the window: price 350 (`cur_reward`) is paid, message and sound arrive.
    let cmd = Command {
        player: A,
        kind: tk::hire_command(0, B, tk::SID_SHERIFF as i32, hq),
    };
    let events = crate::tick::step(sim.state, sim.rules, &[cmd]);
    assert_eq!(sim.state.resource(0, 0), 350, "350 paid");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::TextMessage { to: None, .. })),
        "message 10010 anyway"
    );
    // Destroyed in the same step: table empty, the `0x122` is lost, no headhunter, no
    // refund gold, no panic.
    assert_eq!(sim.state.phase, 0);
    assert!(events.contains(&SimEvent::Destroyed { id: hh }));
    assert!(sim.object(hh).is_none());
    assert_eq!(sim.state.headhunters, [None; 3]);
    assert!(sources_of(&sim, hh).is_empty());
    steps(&mut sim, 2 * THINK_STEPS);
    assert_eq!(sim.state.headhunters, [None; 3], "no new headhunter");
    assert!(
        sim.state.objects().all(|o| o.guid != tk::HUNTER_SPA),
        "no headhunter object: the building creates no new one as long as the old one still stands"
    );
    assert_eq!(sim.state.resource(0, 0), 350, "no refund");
    // The next price is config 12 again.
    assert_eq!(crate::command::headhunter::hire_price(&sim, 0), 300);
}
