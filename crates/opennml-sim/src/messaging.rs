//! Delivering messages (re/sim-core 3): synchronously (`GameObject_SendMessage` `0x0046E800`), via the object's FIFO
//! (`GameObject_PostMessage` `0x0046B760`, `GameObject_PostCommandMessage` `0x0046B7E0`) and the object's own
//! messages `0x7F` (`GameObject_HandleOwnMessage` `0x0046D840`).

use crate::id::ObjectId;
use crate::message::{Envelope, Message, OBJECT_MODULE, Posted};
use crate::module;
use crate::object::MODULE_SLOTS;
use crate::sim::Sim;
use crate::world;

/// `GameObject_SendMessage` without looking at the reply in the arguments: like [`send_mut`] with a copy of the
/// message.
pub fn send(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let mut copy = msg.clone();
    send_mut(sim, id, &mut copy)
}

/// `GameObject_SendMessage`: `0x7F` to the object itself, otherwise to the module in the low byte (`Query`). If the
/// object or module is missing or the module is switched off -> -1. As in the original, queries also reply in the
/// arguments (`msg[2..]` = `args[0..]`), which the sender reads afterwards.
pub fn send_mut(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    // An object that does not (or no longer) exist accepts no message, not even `0x7F` (targets after resolving are
    // silently discarded, re/module-runtime 2).
    if sim.object(id).is_none() {
        return -1;
    }
    let m = usize::from(msg.module());
    if msg.module() == OBJECT_MODULE {
        return handle_own_message(sim, id, msg);
    }
    match sim.object(id) {
        Some(o) if m < MODULE_SLOTS && o.has_module(m, false) => module::query(sim, id, m, msg),
        _ => -1,
    }
}

/// `GameObject_PostMessage(msg, abort)`: check the filter, then append to the FIFO.
pub fn post_message(sim: &mut Sim, id: ObjectId, msg: Message, abort: bool) -> bool {
    let Some(o) = sim.object_mut(id) else {
        return false;
    };
    if !o.filter.is_allowed(msg.code) {
        return false;
    }
    o.fifo.push_back(Posted::Message { msg, abort });
    true
}

/// `GameObject_PostCommandMessage(envelope, abort)`.
pub fn post_command(sim: &mut Sim, id: ObjectId, env: Envelope, abort: bool) -> bool {
    let Some(o) = sim.object_mut(id) else {
        return false;
    };
    if !o.filter.is_allowed(env.msg.code) {
        return false;
    }
    o.fifo.push_back(Posted::Command { env, abort });
    true
}

/// `GameObject_DispatchPostedCommand` (`0x00473690`): `0x7F` to the object (return value 1), otherwise `OnMessage` of
/// the module (without checking `m_Enabled`); if the module is missing -> 0.
pub fn dispatch_command(sim: &mut Sim, id: ObjectId, env: &Envelope) -> i32 {
    if env.msg.module() == OBJECT_MODULE {
        // If the object is missing, the message is discarded (as with `send`).
        if sim.object(id).is_none() {
            return -1;
        }
        handle_own_message(sim, id, &env.msg);
        return 1;
    }
    let m = usize::from(env.msg.module());
    if sim.module(id, m).is_none() {
        return 0;
    }
    i32::from(module::on_message(sim, id, m, env))
}

/// `GameObject_AbortAllModules` (`0x004662A0`): all 60 slots, including switched-off ones.
pub fn abort_all_modules(sim: &mut Sim, id: ObjectId) {
    for n in 0..MODULE_SLOTS {
        if sim.module(id, n).is_some() {
            module::abort(sim, id, n);
        }
    }
}

/// `GameObject_IsAnyModuleBusy` (`0x00469E20`): modules of the tick list.
pub fn is_any_module_busy(sim: &Sim, id: ObjectId) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    o.tick_list
        .iter()
        .any(|&n| module::is_busy(sim, id, usize::from(n)))
}

/// `Module_SendSuccessMessage` (`0x004FD0E0`): discard the fault message, empty the success message and send it
/// synchronously to the target (-1 = own object).
pub fn send_success(sim: &mut Sim, id: ObjectId, index: usize) {
    let Some(m) = sim.module_mut(id, index) else {
        return;
    };
    m.base.fault = None;
    let (msg, target) = (m.base.success.take(), m.base.success_target);
    if let Some(msg) = msg {
        let to = if target.is_none() { id } else { target };
        send(sim, to, &msg);
    }
}

/// `Module_SendFaultMessage` (`0x004FD080`), mirrored.
pub fn send_fault(sim: &mut Sim, id: ObjectId, index: usize) {
    let Some(m) = sim.module_mut(id, index) else {
        return;
    };
    m.base.success = None;
    let (msg, target) = (m.base.fault.take(), m.base.fault_target);
    if let Some(msg) = msg {
        let to = if target.is_none() { id } else { target };
        send(sim, to, &msg);
    }
}

/// `GameObject_HandleOwnMessage`: the object's own messages.
pub fn handle_own_message(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    match msg.code {
        // Damage (`combat::damage`).
        0x17F => crate::combat::damage::on_damage(sim, id, msg),
        0x27F => add_hitpoints_percent(sim, id, msg),
        // Set an event bit for scripts (`0x0046DE70`): ID and bit from the message, always on, without checking the
        // argument count; the only sender is `RaiseIdleEvent` (`0x97F`, bit 1).
        0x37F => {
            world::set_object_event_bit(
                sim,
                ObjectId::from_i32(msg.arg(0)),
                msg.arg(1) as u32,
                true,
            );
            0
        }
        // Queue a change of owner; with three arguments additionally the stored message.
        0x47F if msg.args.len() == 1 || msg.args.len() == 3 => {
            let slot = msg.arg(0);
            if let Some(o) = sim.object_mut(id) {
                let three = msg.args.len() == 3;
                o.owner_change_target = ObjectId::from_i32(if three { msg.arg(1) } else { 0 });
                o.owner_change_msg = if three { msg.arg(2) } else { 0 };
            }
            if (0..=10).contains(&slot) {
                world::queue_owner_change(sim, id, slot as u8);
            }
            0
        }
        // Resources raw (fixed point) to the owner, in pairs; quirk: every other `argc` of `0x47F` also ends up
        // here.
        0x47F | 0x57F => {
            let slot = usize::from(id.slot());
            if let Some(p) = sim.state.players.get_mut(slot) {
                for pair in msg.args.chunks_exact(2) {
                    if let Some(r) = usize::try_from(pair[0])
                        .ok()
                        .and_then(|i| p.resources_fx.get_mut(i))
                    {
                        *r = r.wrapping_add(pair[1]);
                    }
                }
            }
            0
        }
        // Transfer a fraction of an attribute to another object, then the temporary upgrades `arg2` onto itself and
        // `arg3` onto the target (`0x0046DF40...0x0046E00A`, K-64, re/evoke-aura 13; no sender in the program). If the
        // target is missing, the whole branch is skipped.
        0x67F => {
            let (target, attr) = (ObjectId::from_i32(msg.arg(0)), msg.arg(1) as u32);
            if sim.object(target).is_none() {
                return 0;
            }
            let own = sim
                .object(id)
                .map_or(0, |o| o.attributes.current_fraction(attr));
            let free = sim
                .object(target)
                .map_or(0, |o| 100_000 - o.attributes.current_fraction(attr));
            let t = own.min(free);
            if let Some(o) = sim.object_mut(id) {
                o.attributes.add_fraction(attr, -t);
            }
            if let Some(o) = sim.object_mut(target) {
                o.attributes.add_fraction(attr, t);
            }
            // Without checking the argument count: a missing argument is 0, so no record (A7: the original reads
            // behind the message).
            crate::upgrades::temp::apply(sim, id, msg.arg(2) as u32);
            crate::upgrades::temp::apply(sim, target, msg.arg(3) as u32);
            0
        }
        // Continue the nearest work: queries for lumbering, harvesting, gold (control kind 2: nothing).
        0x77F => {
            resume_nearest_work(sim, id);
            0
        }
        // Report an idle worker (`GameWorld_RegisterIdleWorker` `0x0047EDA0`): the object itself; the original does
        // not read the arguments.
        0x87F => {
            crate::idle_workers::register(sim, id);
            0
        }
        // Event bit 1 and report as idle.
        0x97F => {
            send(sim, id, &Message::new(0x37F, &[id.as_i32(), 1]));
            send(sim, id, &Message::new(0x87F, &[id.as_i32()]));
            0
        }
        // Set carrier.
        0xA7F => {
            if let Some(o) = sim.object_mut(id) {
                o.carrier = ObjectId::from_i32(msg.arg(0));
            }
            0
        }
        // Extra energy of the source into hit points.
        0xB7F => {
            let source = ObjectId::from_i32(msg.arg(0));
            if sim.object(source).is_none() {
                return 0;
            }
            // `v` with the percent of the source (`sim.attribute`), the receiver's hit points with its own
            // (re/upgrades 9).
            let v = sim.attribute(source, 15);
            let ps = sim
                .object(source)
                .map_or(0, |o| sim.type_upgrade_percent(o.owner(), o.guid, 15));
            let pr = sim
                .object(id)
                .map_or(0, |o| sim.type_upgrade_percent(o.owner(), o.guid, 14));
            if let Some(o) = sim.object_mut(source) {
                o.attributes.set_current_value(15, 0, ps);
            }
            if let Some(o) = sim.object_mut(id) {
                o.attributes.add_to_current_value(14, v, pr);
            }
            0
        }
        _ => -1,
    }
}

/// `0x27F {pct}` or `{pct, attacker}` (`GameObject_HandleOwnMessage` `0x0046D840`): `d = Max(14) · pct / 100` (`i32`,
/// truncated; negative = damage), `AddToCurrentValue(14, d)`; then synchronously to the existing modules (including
/// switched-off ones) building `0x1403 {}` and walk `0x1100 {}` (the receivers of modules `0x29`/`0x32` do not
/// exist). With two arguments and an existing attacker, the attacker gets `GameObject_AddExperience(−d)` if the
/// target is a unit or an occupied building (`0x217`) and in an enemy relation (`HasEnemyRelation`; slot 8 is an
/// enemy of everyone only as an attacker; against neutrals as a victim there is no experience, C-125), then
/// `NotifyAttackedBy(attacker)`. At the end always `GameObject_ReportUnderAttack` (event; the local player's UI shows
/// it). Returns `d`.
fn add_hitpoints_percent(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    let Some(o) = sim.object(id) else { return 0 };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
    let d = o.attributes.max_value(14, p).wrapping_mul(msg.arg(0)) / 100;
    if let Some(o) = sim.object_mut(id) {
        o.attributes.add_to_current_value(14, d, p);
    }
    for (module, code) in [(3, 0x1403), (0, 0x1100)] {
        if sim.module(id, module).is_some() {
            send(sim, id, &Message::new(code, &[]));
        }
    }
    let attacker = ObjectId::from_i32(msg.arg(1));
    if msg.args.len() == 2 && sim.object(attacker).is_some() {
        let unit_or_occupied = sim.object(id).is_some_and(|o| o.has_module(0, false))
            || (sim.object(id).is_some_and(|o| o.has_module(3, false))
                && crate::filter::matches_class(sim, id, 0x217));
        if unit_or_occupied
            && sim
                .state
                .diplomacy
                .is_enemy(i32::from(attacker.slot()), i32::from(id.slot()))
        {
            crate::combat::rank::add_experience(sim, attacker, d.wrapping_neg());
        }
        notify_attacked_by(sim, id, attacker);
    }
    crate::combat::damage::report_under_attack(sim, id);
    d
}

/// `GameObject_NotifyAttackedBy(attacker)` (`0x00466ED0`): last attacker `+0x1B8`, ticks since the hit `+0x1BC = 0`,
/// `0x911 {attacker}` to the attack module (retaliation), then the attack report for AI slots ([`report_attack`]).
/// Before that, first of all event bit 7 for the scripts (`0x00466EE1`), on every call and without a relation check
/// (also own and allied fire, E-188); a missing object gets nothing (`this` in the original).
pub fn notify_attacked_by(sim: &mut Sim, id: ObjectId, attacker: ObjectId) {
    if sim.object(id).is_none() {
        return;
    }
    world::set_object_event_bit(sim, id, 7, true);
    let Some(o) = sim.object_mut(id) else { return };
    o.last_attacker = attacker;
    o.ticks_since_hit = 0;
    send(sim, id, &Message::new(0x911, &[attacker.as_i32()]));
    report_attack(sim, id);
}

/// AI alarm `FUN_00482260` from `NotifyAttackedBy` (re/ki-queries 5.11.2, E-61): if the victim is on a slot 0-7 with
/// control kind AI, enter `{ID -> tile}` into the slot's report table (`SetAt`: new at the front of the bucket,
/// present -> overwrite the tile, the position stays). As in the original, the tile is read freshly from the object
/// **after** `0x911`. Attacker and relation play no role (also own and allied fire).
pub fn report_attack(sim: &mut Sim, victim: ObjectId) {
    let slot = usize::from(victim.slot());
    let ai = sim
        .state
        .players
        .get(slot)
        .is_some_and(|p| p.control == crate::state::Control::Ai);
    if !ai {
        return;
    }
    let Some((x, z)) = sim.object(victim).map(|o| o.tile) else {
        return;
    };
    if let Some(map) = sim.state.attack_reports.get_mut(slot) {
        map.insert(victim.as_i32(), crate::state::ReportTile { x, z });
    }
}

/// `GameObject_ResumeNearestWork` (`0x00467E40`, `0x77F`): control kind 2 (AI) -> nothing. Otherwise the queries wood
/// `0xA08`, harvest `0x90C` and gold `0x907`, each with `{−1, 0}`; the gatherer modules fill `{distance, target}` into
/// the arguments on the first call (if the work is missing, `args[0] = −1` remains). What is compared is `args[0]`
/// (signed, not the return value): the smallest value != -1 wins, in case of a tie the earlier query. The winner
/// message goes out again **with the filled arguments** (`args[0] ≠ −1` starts the work at the gatherer). Return
/// value as in the original: `true` if a work was continued.
pub fn resume_nearest_work(sim: &mut Sim, id: ObjectId) -> bool {
    let ai = sim
        .state
        .players
        .get(usize::from(id.slot()))
        .is_some_and(|p| p.control == crate::state::Control::Ai);
    if ai {
        return false;
    }
    let mut queries = [0xA08, 0x90C, 0x907].map(|code| Message::new(code, &[-1, 0]));
    for q in &mut queries {
        send_mut(sim, id, q);
    }
    // Like the original's comparison chain: first wood against harvest, then against gold.
    let mut best = 0;
    let mut value = queries[0].arg(0);
    for (i, q) in queries.iter().enumerate().skip(1) {
        let v = q.arg(0);
        if value == -1 || (v < value && v != -1) {
            best = i;
            value = v;
        }
    }
    if value == -1 {
        return false;
    }
    let mut winner = queries[best].clone();
    send_mut(sim, id, &mut winner);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, destroy_object, process_owner_changes};

    fn setup() -> (Rules, SimState) {
        let rules = Rules::synthetic(
            "GUID 1100\nNAME K\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\n",
            16,
        );
        (rules, SimState::new(0, 0))
    }

    #[test]
    fn messages_route_to_modules_or_the_object_and_missing_ones_answer_minus_one() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        // Inert modules and missing modules: -1; unknown object message: -1.
        assert_eq!(send(&mut sim, id, &Message::new(0x102, &[])), -1);
        assert_eq!(send(&mut sim, id, &Message::new(0x106, &[])), -1);
        assert_eq!(send(&mut sim, id, &Message::new(0xC7F, &[])), -1);
        assert_eq!(
            send(&mut sim, ObjectId::NONE, &Message::new(0x102, &[])),
            -1
        );
        // Carrier and resources (raw, in pairs; invalid indices ignored).
        send(&mut sim, id, &Message::new(0xA7F, &[0x10800]));
        assert_eq!(sim.object(id).unwrap().carrier, ObjectId(0x10800));
        let gold = sim.state.players[0].resources_fx[0];
        send(&mut sim, id, &Message::new(0x57F, &[0, 1000, 9, 5, 1]));
        assert_eq!(sim.state.players[0].resources_fx[0], gold + 1000);
        // 0x47F with two arguments falls into the resources branch.
        send(&mut sim, id, &Message::new(0x47F, &[0, 96]));
        assert_eq!(sim.state.players[0].resources_fx[0], gold + 1096);
        assert!(sim.state.owner_changes.is_empty());
        send(&mut sim, id, &Message::new(0x47F, &[4]));
        process_owner_changes(&mut sim);
        assert_eq!(sim.state.tables[4].count, 1);
    }

    #[test]
    fn posting_respects_the_filter_and_queues_in_order() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        assert!(post_message(&mut sim, id, Message::new(0x900, &[]), false));
        assert!(post_command(
            &mut sim,
            id,
            Envelope::bare(Message::new(0x200, &[])),
            true
        ));
        sim.object_mut(id).unwrap().filter.codes.push(0x900);
        assert!(!post_message(&mut sim, id, Message::new(0x900, &[]), false));
        let o = sim.object(id).unwrap();
        assert_eq!(o.fifo.len(), 2);
        assert!(!o.fifo[0].aborts() && o.fifo[1].aborts());
        assert_eq!(
            dispatch_command(&mut sim, id, &Envelope::bare(Message::new(0x206, &[]))),
            0
        );
        // The stand module does not evaluate `OnMessage` (base: 1).
        assert_eq!(
            dispatch_command(&mut sim, id, &Envelope::bare(Message::new(0x202, &[]))),
            1
        );
        assert!(!is_any_module_busy(&sim, id));
    }

    #[test]
    fn replies_go_synchronously_to_their_target() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let b = create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [20.0, 0.0, 10.0])).unwrap();
        let reply = Message::new(0xA7F, &[a.as_i32()]);
        sim.module_mut(a, 2)
            .unwrap()
            .base
            .set_success(Some(&reply), b);
        sim.module_mut(a, 2)
            .unwrap()
            .base
            .set_fault(Some(&Message::new(0xA7F, &[7])), ObjectId::NONE);
        send_success(&mut sim, a, 2);
        // Success to b, fault discarded, both fields empty.
        assert_eq!(sim.object(b).unwrap().carrier, a);
        let base = &sim.module(a, 2).unwrap().base;
        assert!(base.success.is_none() && base.fault.is_none());
        assert!(sim.object(a).unwrap().carrier.is_none());
    }

    /// Answer of a gatherer query: `(code, distance, target, return value)`.
    type Answer = (u32, i32, i32, i32);
    /// Answers and log of the sent queries.
    type Work = (Vec<Answer>, Vec<Message>);

    thread_local! {
        static WORK: std::cell::RefCell<Work> =
            const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
    }

    /// Gatherer query like `PropertyLumbering_FindTree` (`0xA08`): the first call (`args[0] = −1`) fills
    /// `{distance, target}`, the second starts the work; everything is logged.
    fn gatherer(_: &mut Sim, _: ObjectId, _: usize, msg: &mut Message) -> i32 {
        WORK.with(|w| {
            let mut w = w.borrow_mut();
            w.1.push(msg.clone());
            if msg.arg(0) != -1 {
                return 1;
            }
            match w.0.iter().find(|a| a.0 == msg.code) {
                Some(&(_, d, t, r)) => {
                    msg.set_arg(0, d);
                    msg.set_arg(1, t);
                    r
                }
                None => 0,
            }
        })
    }

    fn resume_with(answers: Vec<Answer>, ai: bool) -> (bool, Vec<Message>) {
        let rules = Rules::synthetic(
            "GUID 1100\nNAME K\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nPROPID 7\nPROPID 8\nPROPID 12\n",
            16,
        );
        let mut state = SimState::new(0, 0);
        if ai {
            state.players[0].control = crate::state::Control::Ai;
        }
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        // The gatherer modules have behavior; here they answer through the hook.
        for index in [7, 8, 12] {
            sim.module_mut(id, index).unwrap().kind = crate::module::ModuleKind::Inert;
        }
        WORK.with(|w| *w.borrow_mut() = (answers, Vec::new()));
        let done = crate::module::test_hook::with(gatherer, || resume_nearest_work(&mut sim, id));
        (done, WORK.with(|w| w.borrow().1.clone()))
    }

    #[test]
    fn resuming_work_compares_the_filled_distances_and_resends_the_filled_message() {
        // Wood 5000 (return value 1), harvest 3000, gold 3000 (return value 7): `args[0]` is compared, tie -> the
        // earlier query (harvest), which goes out again with its answers.
        let (done, log) = resume_with(
            vec![
                (0xA08, 5000, 11, 1),
                (0x90C, 3000, 22, 1),
                (0x907, 3000, 33, 7),
            ],
            false,
        );
        assert!(done);
        let codes: Vec<u32> = log.iter().map(|m| m.code).collect();
        assert_eq!(codes, [0xA08, 0x90C, 0x907, 0x90C]);
        assert!(log[..3].iter().all(|m| m.args == [-1, 0]));
        assert_eq!(log[3].args, [3000, 22]);
        // Wood missing (-1): gold with the smaller distance wins against harvest.
        let (_, log) = resume_with(vec![(0x90C, 900, 22, 1), (0x907, 800, 33, 1)], false);
        assert_eq!((log[3].code, log[3].args.clone()), (0x907, vec![800, 33]));
        // No work: nothing sent again.
        let (done, log) = resume_with(Vec::new(), false);
        assert!(!done && log.len() == 3);
        // Control kind AI: no query at all.
        let (done, log) = resume_with(vec![(0xA08, 1, 11, 1)], true);
        assert!(!done && log.is_empty());
    }

    #[test]
    fn messages_to_stale_objects_are_dropped() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        destroy_object(&mut sim, id);
        // The ID points to slot 0, but the object is gone: nothing may take effect.
        let gold = sim.state.players[0].resources_fx[0];
        assert_eq!(send(&mut sim, id, &Message::new(0x57F, &[0, 1000])), -1);
        assert_eq!(send(&mut sim, id, &Message::new(0x47F, &[4])), -1);
        assert_eq!(
            dispatch_command(
                &mut sim,
                id,
                &Envelope::bare(Message::new(0x57F, &[0, 1000]))
            ),
            -1
        );
        assert_eq!(sim.state.players[0].resources_fx[0], gold);
        assert!(sim.state.owner_changes.is_empty());
    }

    // --- Attack reports (re/ki-queries 5.11, E-61) ---

    /// Tile center as world position (128 units per tile).
    fn at(x: i32, z: i32) -> [f32; 3] {
        [(x * 128 + 64) as f32, 0.0, (z * 128 + 64) as f32]
    }

    /// Slot 1 AI, slot 0 human.
    fn ai_state(state: &mut SimState) {
        state.players[0].control = crate::state::Control::Human;
        state.players[1].control = crate::state::Control::Ai;
    }

    fn spawn_at(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        create_object(sim, &mut ObjectDesc::new(guid, slot, at(tile.0, tile.1))).unwrap()
    }

    fn report(sim: &Sim, slot: usize, id: ObjectId) -> Option<crate::state::ReportTile> {
        sim.state.attack_reports[slot].get(id.as_i32()).copied()
    }

    // QUIRK E-61: kept
    #[test]
    fn attack_reports_fill_only_for_ai_victims() {
        use crate::state::ReportTile;
        let (rules, mut state) = setup();
        ai_state(&mut state);
        let mut sim = Sim::new(&mut state, &rules);
        let ai = spawn_at(&mut sim, 1100, 1, (3, 5));
        let ai_friend = spawn_at(&mut sim, 1100, 1, (7, 2));
        let human = spawn_at(&mut sim, 1100, 0, (9, 9));
        let neutral = spawn_at(&mut sim, 1100, 8, (4, 4));
        let gone = spawn_at(&mut sim, 1100, 1, (6, 6));
        destroy_object(&mut sim, gone);
        assert!(sim.object(gone).is_none());
        assert_eq!(sim.object(ai).unwrap().tile, (3, 5));
        // Victim on the AI slot: report with its tile, key = its ID.
        notify_attacked_by(&mut sim, ai, human);
        assert_eq!(report(&sim, 1, ai), Some(ReportTile { x: 3, z: 5 }));
        // Victim on the human slot, on slot 8 and a removed victim: no report.
        notify_attacked_by(&mut sim, human, ai);
        notify_attacked_by(&mut sim, neutral, ai);
        notify_attacked_by(&mut sim, gone, human);
        assert!(sim.state.attack_reports[0].is_empty());
        assert_eq!(sim.state.attack_reports[1].len(), 1);
        assert!(
            sim.state
                .attack_reports
                .iter()
                .enumerate()
                .all(|(p, m)| p == 1 || m.is_empty())
        );
        // E-61: own fire (attacker of the same slot) also reports.
        notify_attacked_by(&mut sim, ai_friend, ai);
        assert_eq!(report(&sim, 1, ai_friend), Some(ReportTile { x: 7, z: 2 }));
        assert_eq!(sim.state.attack_reports[1].len(), 2);
        // The fields of the hit are as before.
        let o = sim.object(ai_friend).unwrap();
        assert_eq!((o.last_attacker, o.ticks_since_hit), (ai, 0));
    }

    // QUIRK E-61: kept
    #[test]
    fn attack_reports_keep_one_entry_per_victim_in_mfc_order() {
        use crate::mfc::MfcMap;
        use crate::state::{ATTACK_REPORT_BUCKETS, ReportTile};
        let (rules, mut state) = setup();
        ai_state(&mut state);
        let mut sim = Sim::new(&mut state, &rules);
        let attacker = spawn_at(&mut sim, 1100, 0, (0, 0));
        // Buckets from the created IDs (`(hid >> 4) % 17`), not guessed.
        let ids: Vec<ObjectId> = (0..32)
            .map(|i| spawn_at(&mut sim, 1100, 1, (i % 16, 1 + i / 16)))
            .collect();
        let probe = MfcMap::<ReportTile>::new(ATTACK_REPORT_BUCKETS);
        let in_bucket = |b: usize| -> Vec<ObjectId> {
            ids.iter()
                .copied()
                .filter(|id| probe.bucket_of(id.as_i32()) == b)
                .collect()
        };
        let (b3, b1) = (in_bucket(3), in_bucket(1));
        assert!(
            b3.len() >= 2 && !b1.is_empty(),
            "bucket 3: {b3:?}, bucket 1: {b1:?}"
        );
        let (first, second, third) = (b3[0], b1[0], b3[1]);
        for v in [first, second, third] {
            notify_attacked_by(&mut sim, v, attacker);
        }
        let order = |sim: &Sim| -> Vec<i32> {
            sim.state.attack_reports[1].iter().map(|(k, _)| k).collect()
        };
        // Bucket 1 first, in bucket 3 the newer entry before the older one.
        let expected = vec![second.as_i32(), third.as_i32(), first.as_i32()];
        assert_eq!(order(&sim), expected);
        // Second hit on the first victim after walking: new tile, position unchanged.
        sim.object_mut(first).unwrap().tile = (12, 13);
        notify_attacked_by(&mut sim, first, attacker);
        assert_eq!(report(&sim, 1, first), Some(ReportTile { x: 12, z: 13 }));
        assert_eq!(order(&sim), expected);
        assert_eq!(
            sim.state.attack_reports[1].check(ATTACK_REPORT_BUCKETS),
            Ok(())
        );
    }

    // QUIRK E-61: kept
    #[test]
    fn destroyed_container_reports_every_occupant() {
        use crate::module::quarter;
        use crate::state::ReportTile;
        // Container without a building module (wagon with quarters, capacity 5): the occupants stay in `occupants` and
        // learn the last attacker via `notify_attacked_by`.
        let mut rules = Rules::synthetic(
            concat!(
                "GUID 1100\nNAME K\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\n",
                "GUID 1300\nNAME Wagen\nVALUEID 100 6 1 0\nPROPID 23\n",
            ),
            16,
        );
        rules
            .defaults
            .insert(1300, [(23, 5), (14, 100)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        ai_state(&mut state);
        let mut sim = Sim::new(&mut state, &rules);
        let wagon = spawn_at(&mut sim, 1300, 1, (8, 8));
        let units = [
            spawn_at(&mut sim, 1100, 1, (3, 3)),
            spawn_at(&mut sim, 1100, 1, (4, 3)),
        ];
        for &u in &units {
            assert_eq!(quarter::accept_unit(&mut sim, wagon, u), 0);
        }
        let attacker = spawn_at(&mut sim, 1100, 0, (1, 1));
        sim.object_mut(wagon).unwrap().last_attacker = attacker;
        let tiles: Vec<(i32, i32)> = units.iter().map(|&u| sim.object(u).unwrap().tile).collect();
        assert_eq!(
            send(
                &mut sim,
                wagon,
                &Message::new(quarter::msg::CONTAINER_DESTROYED, &[])
            ),
            1
        );
        for (&u, &(x, z)) in units.iter().zip(&tiles) {
            assert_eq!(report(&sim, 1, u), Some(ReportTile { x, z }), "{u:?}");
        }
        assert_eq!(sim.state.attack_reports[1].len(), 2);
        assert_eq!(report(&sim, 1, wagon), None);
    }

    // QUIRK E-148: kept
    #[test]
    fn msg_37f_sets_the_event_bit() {
        // `HandleOwnMessage` `0x37F {id, bit}` (`0x0046DE70`): bit on the ID from the message, always on; the only
        // sender is `RaiseIdleEvent` from `0x97F` with its own ID and bit 1.
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let b = create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [20.0, 0.0, 10.0])).unwrap();
        assert_eq!(world::object_event_bits(&sim, a), 0);
        assert_eq!(send(&mut sim, a, &Message::new(0x97F, &[])), 0);
        assert_eq!(world::object_event_bits(&sim, a), 1 << 1);
        // Foreign ID and other bit from the arguments; a set bit stays set.
        assert_eq!(send(&mut sim, a, &Message::new(0x37F, &[b.as_i32(), 7])), 0);
        assert_eq!(world::object_event_bits(&sim, b), 1 << 7);
        send(&mut sim, a, &Message::new(0x37F, &[a.as_i32(), 1]));
        assert_eq!(world::object_event_bits(&sim, a), 1 << 1);
    }

    // QUIRK E-188: kept
    #[test]
    fn notify_attacked_by_sets_bit_7_without_relation_check() {
        // `GameObject_NotifyAttackedBy` (`0x00466EE1`): bit 7 first, without a relation check.
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let victim =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let ally =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [20.0, 0.0, 10.0])).unwrap();
        for (p, q) in [(0, 1), (1, 0)] {
            sim.state.diplomacy.set_friend(p, q, true);
            sim.state.diplomacy.set_enemy(p, q, false);
        }
        assert!(!sim.state.diplomacy.is_enemy(1, 0) && !sim.state.diplomacy.is_enemy(0, 1));
        notify_attacked_by(&mut sim, victim, ally);
        assert_eq!(world::object_event_bits(&sim, victim), 1 << 7);
        assert_eq!(sim.object(victim).unwrap().last_attacker, ally);
        // Via the percentage change `0x27F {−1 %, attacker}` with an allied attacker likewise.
        world::set_object_event_bit(&mut sim, victim, 7, false);
        send(&mut sim, victim, &Message::new(0x27F, &[-1, ally.as_i32()]));
        assert_eq!(world::object_event_bits(&sim, victim), 1 << 7);
        // A missing object gets no bit (like `this` in the original).
        notify_attacked_by(&mut sim, ObjectId::new(9, 0, 1), ally);
        assert_eq!(world::object_event_bits(&sim, ObjectId::new(9, 0, 1)), 0);
        assert!(
            !sim.state
                .object_event_bits
                .contains_key(&ObjectId::new(9, 0, 1))
        );
    }
}
