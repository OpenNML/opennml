//! The tick (re/sim-core 1): one call of [`step`] is an engine tick of 50 ms game time
//! (`GameEngine_GameTick` `0x00441B70`). One of the two object groups ticks with 100 ms in turn
//! (`GameWorld_Tick` `0x0048E710`), so every object gets its turn every 100 ms; destruction and
//! change of owner run every second tick (phase 0).

use crate::attributes::{ATTRIBUTE_COUNT, FIRST_ATTRIBUTE};
use crate::command::{Command, execute, process_queue};
use crate::id::ObjectId;
use crate::message::{Message, Posted};
use crate::messaging::{abort_all_modules, dispatch_command, is_any_module_busy, send};
use crate::module;
use crate::object::{MODULE_SLOTS, status};
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::world::{process_destroy_queue, process_owner_changes, queue_destroy};

/// Length of an engine tick in ms of game time.
pub const STEP_MS: u32 = 50;
/// Tick length of an object (`GameObject_Tick(100)`).
pub const OBJECT_TICK_MS: i32 = 100;

/// One engine tick: first the commands (at the tick boundary), then the world tick, then the AI tick
/// (`GameEngine_GameTick` calls `KIManager_TickAllPlayers` after `GameWorld_Tick`), then the scripts.
/// Return value: the events of the tick (not part of the state).
pub fn step(state: &mut SimState, rules: &Rules, commands: &[Command]) -> Vec<SimEvent> {
    step_with(state, rules, commands, StepOptions::default())
}

/// Switches for tests and the dry run of the scripts; they live in [`Sim`], not in the state.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepOptions {
    /// Every condition combination of the scripts counts as true (the conditions still run).
    pub all_conditions_true: bool,
}

/// [`step`] with test switches. Sequence (`GameEngine_Update` `0x00445650`, re/scripts 4): commands ->
/// world tick -> timer list -> AI only without a dialog (E-139) -> victory/defeat -> level scripts only
/// without a dialog -> dialog script -> speech time.
#[doc(hidden)]
pub fn step_with(
    state: &mut SimState,
    rules: &Rules,
    commands: &[Command],
    options: StepOptions,
) -> Vec<SimEvent> {
    let mut sim = Sim::new(state, rules);
    sim.all_conditions_true = options.all_conditions_true;
    for c in commands {
        execute(&mut sim, c, true);
    }
    world_tick(&mut sim);
    // Timer list directly after the world tick (`GameEngine_TickTimedActions(50)` `0x004409E0` in
    // `GameEngine_GameTick` `0x00441B70`), also during a dialog.
    crate::game_end::tick_countdowns(&mut sim);
    if !crate::script::dialog_running(sim.state) {
        crate::ki::run::tick(&mut sim);
    }
    // Victory and defeat (`GameEngine_CheckVictoryDefeat` `0x00431300` from `GameEngine_Update`
    // `0x00445650`), also during a dialog.
    crate::game_end::check_victory_defeat(&mut sim);
    if !crate::script::dialog_running(sim.state) {
        crate::script::tick_level(&mut sim);
    }
    crate::script::tick_dialog(&mut sim);
    crate::script::tick_speech(&mut sim);
    // A loaded savegame may carry every tick: wrapped as in the release build.
    sim.state.steps = sim.state.steps.wrapping_add(1);
    std::mem::take(&mut sim.state.events)
}

/// `GameWorld_Tick(100)`. Before the first half of a 100 ms tick, the budgets of the B* planners are
/// reset and the combat points of the elapsed double tick are reported. In the phase 0 step, after the
/// group the upgrade queue runs, then the extra list, then the expiry of the counter-upgrades; at the
/// end of every engine tick the vision sources and the fog step run (re/vision 3.1). Projectiles tick
/// in the phase 0 step after the group, the extra list and the counter-upgrades, then the list of idle
/// workers; other effects run in the rendering.
fn world_tick(sim: &mut Sim) {
    if sim.state.phase == 0 {
        for p in &mut sim.state.planners {
            p.reset_tick_budget();
        }
        // Combat points to the host, sum back to 0 (re/combat-host 5).
        let points = std::mem::take(&mut sim.state.combat.attack_points);
        sim.state.events.push(SimEvent::AttackPoints { points });
        sim.state.phase = 2;
        sim.state.tick = sim.state.tick.wrapping_add(1);
        // Unit against unit (collision manager on, `GameWorld+0x314`), before all object ticks.
        crate::collision::records::tick(sim);
    }
    sim.state.phase -= 1;
    let group = if sim.state.phase == 1 { 0 } else { 1 };
    // The list grows if an object is created during the tick and appended at the end; it then still
    // gets its turn in the same pass (as with the MFC list of the original).
    let mut i = 0;
    while let Some(&id) = sim.state.tick_groups[group].get(i) {
        object_tick(sim, id, OBJECT_TICK_MS);
        i += 1;
    }
    // Special lists of hidden slots directly after group 0 (`0x0048E845`).
    if sim.state.phase == 1 {
        crate::world::hide::tick_inactive_lists(sim);
    }
    if sim.state.phase == 0 {
        // World queue `m_UpgradeStack` (`0x0048E8C1`, re/upgrades 3.5) before the extra list.
        crate::upgrades::process_upgrade_queue(sim);
        tick_extra_list(sim);
        // Counter-upgrades (`0x004127F0`, revocation `0x004812F0`; re/temp-upgrades 3.6): after the
        // extra list, before `EffectManager_Tick`.
        crate::upgrades::counter::tick(sim);
        // `EffectManager_Tick` (`0x004F2260`): projectiles after the group and the extra list
        // (re/combat-projectiles 5).
        crate::combat::projectile::tick(sim);
        // Idle workers (`0x0048E99A` -> `0x0047EB20`): after the objects, before the destruction
        // list - the dead are still in the table and fail at the filter (`DIED`).
        crate::idle_workers::refresh(sim);
        process_destroy_queue(sim);
        process_owner_changes(sim);
    }
    // At the end of every engine tick, independent of the phase (`GameWorld_Tick` `0x0048E710`):
    // run time of the vision sources (`0x0047DF80(50)`).
    crate::fog::tick_vision_sources(&mut sim.state.fog, STEP_MS as i32);
    // One step of the fog cycle (`GameWorld_UpdateFogOfWarStep` `0x00486CD0`).
    crate::fog::fog_step(sim);
}

/// Extra tick list `GameWorld+0x1F70` (`GameWorld_Tick` `0x0048E931`...): only in the phase 0 call,
/// after both tick groups and before the destruction and change-of-owner queues. For each entry
/// `GetObjectById(id, 0)`: present -> `GameObject_Tick(100)` (the whole object tick, as for the
/// groups), otherwise the entry is removed. Like the MFC list, the pass remembers the following entry
/// **before** the tick: if an object removes itself in its own tick (fallen tree), the pass continues
/// with the remembered one; an entry appended during the tick still gets its turn, unless the ticked
/// one was the last. If the remembered following entry itself disappears (a dangling pointer in the
/// original, exception A7), the pass ends.
fn tick_extra_list(sim: &mut Sim) {
    let mut i = 0;
    while let Some(&id) = sim.state.extra_tick.get(i) {
        let next = sim.state.extra_tick.get(i + 1).copied();
        if sim.object(id).is_some() {
            object_tick(sim, id, OBJECT_TICK_MS);
        } else if let Some(p) = sim.state.extra_tick.iter().position(|&x| x == id) {
            sim.state.extra_tick.remove(p);
        }
        let Some(next) = next else { return };
        match sim.state.extra_tick.iter().position(|&x| x == next) {
            Some(p) => i = p,
            None => return,
        }
    }
}

/// `GameObject_Tick(dt)` (`0x004736E0`).
pub fn object_tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    #[cfg(test)]
    if let Some(hook) = test_hook::ON_TICK.with(|h| *h.borrow()) {
        hook(sim, id);
    }
    let Some(o) = sim.object(id) else { return };
    // 1. Transported, carrier gone -> destroy (the tick continues).
    if o.status & status::DIED == 0
        && o.status & status::QUARTERED != 0
        && sim.object(o.carrier).is_none()
    {
        queue_destroy(sim, id);
    }
    // 2. Message FIFO or command queue.
    process_fifo_or_queue(sim, id);
    // 3. Animation lock or module ticks.
    let lock = sim.object(id).map_or(0, |o| o.anim_lock_ms);
    if lock > 0 {
        let rest = lock.wrapping_sub(dt);
        if let Some(o) = sim.object_mut(id) {
            o.anim_lock_ms = rest.max(0);
            if rest <= 0 {
                o.set_status_flags(status::ANIM_LOCKED, false);
            }
        }
        // Expiry of the lock (rest < 1): event bit 3 for the scripts (`0x00473899`, E-186). Releasing
        // it via command `0x43` sets `+0x174 = 0` directly, without the bit.
        if rest <= 0 {
            crate::world::set_object_event_bit(sim, id, 3, true);
        }
    } else if lock == 0 {
        tick_modules(sim, id, dt);
    }
    // Frame update of the modules (in the original per drawn frame).
    crate::module::frame_update(sim, id);
    let Some(o) = sim.object_mut(id) else { return };
    // 4. Hand the desired animation to the rendering.
    o.transfer_animation();
    // 5. Check death (every tick until DIED is set).
    check_death(sim, id);
    // 6. Temporary upgrades (`GameObject_TickTempUpgrades` `0x00472600`, K-59).
    crate::upgrades::temp::tick(sim, id);
    // 7. Regeneration (tick length unused); upgrade percentages of the owner per attribute
    //    (`GameWorld_GetTypeUpgradePercent`, re/upgrades 9), computed before the mutating access.
    let Some(o) = sim.object(id) else { return };
    let (slot, guid) = (o.owner(), o.guid);
    let percents: [i32; ATTRIBUTE_COUNT] =
        core::array::from_fn(|i| sim.type_upgrade_percent(slot, guid, FIRST_ATTRIBUTE + i as u32));
    let Some(o) = sim.object_mut(id) else { return };
    o.attributes
        .tick_regeneration(|a| percents[(a - FIRST_ATTRIBUTE) as usize]);
    // 8. Transported: take over the position of the carrier (`SetGridPosition` with own height).
    let (st, carrier) = (o.status, o.carrier);
    if st & status::QUARTERED != 0
        && let Some(grid) = sim.object(carrier).map(|c| c.grid)
    {
        crate::world::set_grid_position(sim, id, grid, true);
    }
    let target_alive = {
        let t = sim.object(id).map_or(ObjectId::NONE, |o| o.target);
        !t.is_none() && t.0 != 0 && sim.object(t).is_some()
    };
    let Some(o) = sim.object_mut(id) else { return };
    // 10. Ticks since the last hit.
    if !o.last_attacker.is_none() && o.ticks_since_hit < i32::MAX {
        o.ticks_since_hit += 1;
    }
    // 11. Clear connection flags without a target.
    for f in [0x4000, 0x2000, 0x8000] {
        if o.status & f != 0 && !target_alive {
            o.set_status_flags(f, false);
        }
    }
}

/// Step 2 of `GameObject_Tick`: if the FIFO was empty, at most one entry of the command queue
/// (only when quiet, with a quiet target); otherwise as many FIFO entries as no module reports
/// "busy". Posted `0x7F` messages are never executed (quirk).
fn process_fifo_or_queue(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    if o.fifo.is_empty() {
        let has_commands = !o.commands.is_empty();
        let quiet = o.status & status::BUSY_MASK == 0;
        let target = o.target;
        let target_quiet = target.is_none()
            || target.0 == 0
            || sim
                .object(target)
                .is_none_or(|t| t.status & status::BUSY_MASK == 0);
        if has_commands && quiet && target_quiet && !is_any_module_busy(sim, id) {
            process_queue(sim, id);
        }
        if let Some(o) = sim.object_mut(id) {
            o.set_status_flags(0x800, false);
        }
        return;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(0x800, false);
    }
    loop {
        if sim.object(id).is_none_or(|o| o.fifo.is_empty()) || is_any_module_busy(sim, id) {
            return;
        }
        let Some(entry) = sim.object_mut(id).and_then(|o| o.fifo.pop_front()) else {
            return;
        };
        if entry.aborts() {
            abort_all_modules(sim, id);
        }
        let m = (entry.code() & 0xFF) as usize;
        let present = m < MODULE_SLOTS && sim.object(id).is_some_and(|o| o.has_module(m, false));
        if !present {
            continue;
        }
        match entry {
            Posted::Message { msg, .. } => {
                send(sim, id, &msg);
            }
            Posted::Command { env, .. } => {
                dispatch_command(sim, id, &env);
            }
        }
    }
}

/// Module 0 first (only if enabled), then the tick list (without module 0, without checking
/// `m_Enabled`), then take over pending activations.
fn tick_modules(sim: &mut Sim, id: ObjectId, dt: i32) {
    if sim.object(id).is_some_and(|o| o.has_module(0, false)) {
        module::tick(sim, id, 0, dt);
    }
    let list = sim
        .object(id)
        .map_or_else(Vec::new, |o| o.tick_list.clone());
    for n in list {
        if n != 0 {
            module::tick(sim, id, usize::from(n), dt);
        }
    }
    if let Some(o) = sim.object_mut(id)
        && !o.pending_activation.is_empty()
    {
        o.apply_module_activation_changes();
    }
}

/// `GameObject_CheckDeath` (`0x0046F700`): hit points below 1 with maximum != 0 -> building
/// `0x3703`, otherwise `PMSG_DIE` (`0x110`); every tick until `DIED` is set.
fn check_death(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    if o.status & status::DIED != 0 {
        return;
    }
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
    let hp = o.attributes.current_value(14, p);
    let max = o.attributes.max_value(14, p);
    if hp < 1 && max != 0 {
        let code = if o.module(3).is_some() { 0x3703 } else { 0x110 };
        send(sim, id, &Message::new(code, &[]));
    }
}

/// Test hook: sees every object tick before its first step (per test thread).
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;

    use super::{ObjectId, Sim};

    /// `(simulation, object)`.
    pub type Hook = fn(&mut Sim, ObjectId);

    thread_local! {
        pub static ON_TICK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Hooks `hook` in for the duration of `f`.
    pub fn with<R>(hook: Hook, f: impl FnOnce() -> R) -> R {
        ON_TICK.with(|h| *h.borrow_mut() = Some(hook));
        let r = f();
        ON_TICK.with(|h| *h.borrow_mut() = None);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandKind, QueuedCommand, enqueue};
    use crate::message::Envelope;
    use crate::messaging::{post_command, post_message};
    use crate::world::{ObjectDesc, create_object, queue_owner_change};

    const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 4201\nNAME Fahne\nVALUEID 100 6 1 0\nPROPID 5\nVALUEID 60002 6 1 256\n";

    fn setup() -> (Rules, SimState) {
        (Rules::synthetic(TYPES, 16), SimState::new(0, 0))
    }

    #[test]
    fn steps_alternate_the_groups_and_count_100_ms_ticks() {
        let (rules, mut state) = setup();
        let ids: Vec<ObjectId> = {
            let mut sim = Sim::new(&mut state, &rules);
            (0..3)
                .map(|i| {
                    create_object(
                        &mut sim,
                        &mut ObjectDesc::new(4201, 0, [10.0 + 100.0 * i as f32, 0.0, 10.0]),
                    )
                    .unwrap()
                })
                .collect()
        };
        assert_eq!(state.tick_groups[0], [ids[0], ids[2]]);
        assert_eq!(state.tick_groups[1], [ids[1]]);
        // Nature ANIM: the desired animation does not arrive until the object's own tick.
        step(&mut state, &rules, &[]);
        assert_eq!((state.tick, state.phase, state.steps), (1, 1, 1));
        let current = |s: &SimState, id| s.object(id, false).unwrap().anim.current;
        assert_eq!((current(&state, ids[0]), current(&state, ids[1])), (0, -1));
        step(&mut state, &rules, &[]);
        assert_eq!((state.tick, state.phase), (1, 0));
        assert_eq!(current(&state, ids[1]), 0);
        step(&mut state, &rules, &[]);
        assert_eq!(state.tick, 2);
    }

    #[test]
    fn the_extra_list_ticks_only_in_phase_0_and_drops_missing_objects() {
        let types = format!(
            "{TYPES}GUID 3700\nNAME Stein\nVALUEID 100 6 1 512\nPROPID 5\nVALUEID 60002 6 1 256\n"
        );
        let rules = Rules::synthetic(&types, 16);
        let mut state = SimState::new(0, 0);
        let (a, b, c, gone) = {
            let mut sim = Sim::new(&mut state, &rules);
            let mut stone = |x: f32| {
                create_object(&mut sim, &mut ObjectDesc::new(3700, 0, [x, 0.0, 10.0])).unwrap()
            };
            let (a, b, c, gone) = (stone(10.0), stone(300.0), stone(600.0), stone(900.0));
            crate::world::destroy_object(&mut sim, gone);
            for id in [a, gone, b, a] {
                crate::world::add_extra_tick(&mut sim, id);
            }
            (a, b, c, gone)
        };
        // Nature on slot 10 is in no tick group.
        assert_eq!(a.slot(), 10);
        assert_eq!(state.extra_tick, [a, gone, b]);
        let current = |s: &SimState, id| s.object(id, false).unwrap().anim.current;
        step(&mut state, &rules, &[]);
        assert_eq!((current(&state, a), current(&state, b)), (-1, -1));
        step(&mut state, &rules, &[]);
        assert_eq!(state.phase, 0);
        assert_eq!((current(&state, a), current(&state, b)), (0, 0));
        assert_eq!(current(&state, c), -1);
        assert_eq!(state.extra_tick, [a, b]);
        let mut sim = Sim::new(&mut state, &rules);
        crate::world::remove_extra_tick(&mut sim, a);
        crate::world::remove_extra_tick(&mut sim, a);
        assert_eq!(sim.state.extra_tick, [b]);
    }

    thread_local! {
        /// Ticked objects in order.
        static TICKED: std::cell::RefCell<Vec<ObjectId>> = const { std::cell::RefCell::new(Vec::new()) };
        /// For each entry once in the tick of `.0`: remove itself (`.1`), append `.2`.
        static PLAN: std::cell::RefCell<Vec<(ObjectId, bool, ObjectId)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_and_apply_plan(sim: &mut Sim, id: ObjectId) {
        TICKED.with(|t| t.borrow_mut().push(id));
        let action = PLAN.with(|p| {
            let mut p = p.borrow_mut();
            let i = p.iter().position(|e| e.0 == id)?;
            Some(p.remove(i))
        });
        if let Some((_, remove_self, append)) = action {
            if remove_self {
                crate::world::remove_extra_tick(sim, id);
            }
            crate::world::add_extra_tick(sim, append);
        }
    }

    #[test]
    fn the_extra_list_continues_after_self_removal_and_reaches_appended_entries() {
        // As `GameWorld_Tick` `0x0048E93B...0x0048E974`: the following node is remembered before the
        // tick (`MOV EDI,[EDI]`), afterwards `TEST EDI,EDI; JNZ`.
        let types = format!(
            "{TYPES}GUID 3700
NAME Stein
VALUEID 100 6 1 512
PROPID 5
VALUEID 60002 6 1 256
"
        );
        let rules = Rules::synthetic(&types, 16);
        let mut state = SimState::new(0, 0);
        let (a, b, c) = {
            let mut sim = Sim::new(&mut state, &rules);
            let mut stone = |x: f32| {
                create_object(&mut sim, &mut ObjectDesc::new(3700, 0, [x, 0.0, 10.0])).unwrap()
            };
            (stone(10.0), stone(300.0), stone(600.0))
        };
        // Nature (slot 10) is in no tick group: every tick comes from the extra list.
        let ticked = |state: &mut SimState| {
            TICKED.with(|t| t.borrow_mut().clear());
            test_hook::with(record_and_apply_plan, || {
                step(state, &rules, &[]);
                step(state, &rules, &[]);
            });
            assert_eq!(state.phase, 0);
            TICKED.with(|t| t.borrow().clone())
        };
        // a removes itself in its own tick and appends c: b (remembered) and c still get their turn.
        {
            let mut sim = Sim::new(&mut state, &rules);
            crate::world::add_extra_tick(&mut sim, a);
            crate::world::add_extra_tick(&mut sim, b);
        }
        PLAN.with(|p| *p.borrow_mut() = vec![(a, true, c)]);
        assert_eq!(ticked(&mut state), [a, b, c]);
        assert_eq!(state.extra_tick, [b, c]);
        // The last entry appends: no remembered following node, the new one not until the next tick.
        state.extra_tick = vec![a];
        PLAN.with(|p| *p.borrow_mut() = vec![(a, false, c)]);
        assert_eq!(ticked(&mut state), [a]);
        assert_eq!(state.extra_tick, [a, c]);
        assert_eq!(ticked(&mut state), [a, c]);
    }

    #[test]
    fn destruction_and_owner_changes_wait_for_phase_0() {
        let (rules, mut state) = setup();
        let unit = {
            let mut sim = Sim::new(&mut state, &rules);
            let u =
                create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
            queue_owner_change(&mut sim, u, 2);
            u
        };
        let events = step(&mut state, &rules, &[]);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SimEvent::OwnerChanged { .. }))
        );
        assert!(state.object(unit, false).is_some());
        let events = step(&mut state, &rules, &[]);
        assert!(matches!(events[0], SimEvent::OwnerChanged { old, .. } if old == unit));
        assert_eq!(state.tables[2].count, 1);
    }

    #[test]
    fn the_fifo_runs_until_empty_and_the_queue_only_when_the_fifo_was_empty() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let b = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [20.0, 0.0, 10.0])).unwrap();
        // Messages to Stand (query 0x302/0x402), one posted 0x7F (never executed) and a command
        // envelope 0x302 (goes to OnMessage, which Stand does not evaluate).
        post_message(&mut sim, a, Message::new(0x302, &[]), false);
        post_message(&mut sim, a, Message::new(0xA7F, &[b.as_i32()]), false);
        post_message(&mut sim, a, Message::new(0x402, &[]), false);
        post_command(&mut sim, a, Envelope::bare(Message::new(0x302, &[])), false);
        // Cyclic halt in the queue.
        let halt = |cyclic| QueuedCommand {
            cyclic,
            command: Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![a] },
            },
        };
        enqueue(&mut sim, a, halt(true));
        enqueue(&mut sim, a, halt(false));
        object_tick(&mut sim, a, 100);
        let o = sim.object(a).unwrap();
        assert!(o.fifo.is_empty() && o.carrier.is_none());
        assert_eq!(o.module(2).unwrap().base.flags & 0x100, 0);
        // The queue was locked in this tick (FIFO not empty).
        assert_eq!(o.commands.len(), 2);
        assert_eq!(o.command_cursor, 0);
        object_tick(&mut sim, a, 100);
        // The cyclic entry stays, cursor 1; then the one-time entry (removed), cursor stays 1.
        assert_eq!(sim.object(a).unwrap().command_cursor, 1);
        object_tick(&mut sim, a, 100);
        let o = sim.object(a).unwrap();
        assert_eq!((o.commands.len(), o.command_cursor), (1, 1));
        object_tick(&mut sim, a, 100);
        assert_eq!(sim.object(a).unwrap().command_cursor, 1);
        // 25 entries at most.
        for _ in 0..30 {
            enqueue(&mut sim, a, halt(false));
        }
        assert_eq!(sim.object(a).unwrap().commands.len(), 25);
        // A direct halt empties the queue.
        execute(&mut sim, &halt(false).command, true);
        assert!(sim.object(a).unwrap().commands.is_empty());
    }

    #[test]
    fn commands_check_their_sender() {
        let (rules, mut state) = setup();
        let diplomacy = |player, p, q, mode| Command {
            player,
            kind: CommandKind::Diplomacy { p, q, mode },
        };
        step(
            &mut state,
            &rules,
            &[diplomacy(0, 0, 1, 0), diplomacy(1, 0, 2, 0)],
        );
        assert!(state.diplomacy.is_friend(0, 1) && !state.diplomacy.is_enemy(0, 1));
        assert!(!state.diplomacy.is_friend(0, 2));
        let vision = Command {
            player: 3,
            kind: CommandKind::Queue {
                cyclic: false,
                inner: Box::new(CommandKind::SharedVision {
                    p: 3,
                    q: 4,
                    on: true,
                }),
            },
        };
        step(&mut state, &rules, &[vision, diplomacy(9, 0, 1, 1)]);
        assert!(state.diplomacy.shares_vision(3, 4) && state.diplomacy.is_friend(0, 1));
    }

    #[test]
    fn a_direct_halt_from_another_player_leaves_the_queue_alone() {
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        // Unit of slot 0 with a queued command and an entry at the read cursor.
        let unit =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let entry = QueuedCommand {
            cyclic: true,
            command: Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![unit] },
            },
        };
        enqueue(&mut sim, unit, entry.clone());
        enqueue(&mut sim, unit, entry);
        sim.object_mut(unit).unwrap().command_cursor = 1;
        let before = sim.object(unit).unwrap().commands.clone();
        // Slot 1 commands the unit of slot 0: the command is rejected, queue and cursor
        // stay.
        let foreign = Command {
            player: 1,
            kind: CommandKind::Halt { units: vec![unit] },
        };
        execute(&mut sim, &foreign, true);
        let o = sim.object(unit).unwrap();
        assert_eq!(o.commands, before);
        assert_eq!(o.command_cursor, 1);
    }

    // QUIRK C-102: kept
    #[test]
    fn projectiles_tick_once_per_double_tick_after_the_objects() {
        use crate::combat::testkit as ck;
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            let a = ck::spawn(&mut sim, ck::ARCHER, 0, (5, 5));
            let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (9, 5));
            let damage = crate::combat::damage::damage_message(&mut sim, a, -1);
            let start = crate::combat::hit_zone::world_pos(&sim, a).unwrap();
            let target_pos = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
            crate::combat::projectile::launch_arrow(
                &mut sim,
                crate::combat::projectile::ArrowLaunch {
                    shooter: a,
                    target: t,
                    damage,
                    start,
                    target_pos,
                    zone: 0,
                    fire: false,
                    model: String::new(),
                    node: "schussposition01".into(),
                },
            );
        }
        let r0 = state.projectiles.list[0].remaining_ms;
        let mut seen = Vec::new();
        for _ in 0..4 {
            step(&mut state, &rules, &[]);
            seen.push((state.phase, state.projectiles.list[0].remaining_ms));
        }
        // Per double tick once -100, in the step that reaches phase 0.
        assert_eq!(
            seen,
            vec![(1, r0), (0, r0 - 100), (1, r0 - 100), (0, r0 - 200)]
        );
    }

    // QUIRK E-186: kept
    #[test]
    fn anim_lock_expiry_sets_bit_3_and_stop_does_not() {
        // `GameObject_Tick` `0x00473899`: when the animation lock `+0x174` expires (rest < 1), status
        // `0x800000` off and event bit 3 on. The test sets the lock by hand instead of via command `0x42`;
        // releasing it like `0x43` (`+0x174 = 0` directly) sets no bit.
        let (rules, mut state) = setup();
        let mut sim = Sim::new(&mut state, &rules);
        let a = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [10.0, 0.0, 10.0])).unwrap();
        let b = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [90.0, 0.0, 10.0])).unwrap();
        for id in [a, b] {
            let o = sim.object_mut(id).unwrap();
            o.anim_lock_ms = 1000;
            o.set_status_flags(status::ANIM_LOCKED, true);
        }
        for _ in 0..9 {
            object_tick(&mut sim, a, 100);
        }
        assert_eq!(sim.object(a).unwrap().anim_lock_ms, 100);
        assert_eq!(crate::world::object_event_bits(&sim, a), 0);
        object_tick(&mut sim, a, 100);
        let o = sim.object(a).unwrap();
        assert_eq!((o.anim_lock_ms, o.status & status::ANIM_LOCKED), (0, 0));
        assert_eq!(crate::world::object_event_bits(&sim, a), 1 << 3);
        // Release by hand (like `CmdStopAnimationLock`): no bit.
        let o = sim.object_mut(b).unwrap();
        o.anim_lock_ms = 0;
        o.set_status_flags(status::ANIM_LOCKED, false);
        object_tick(&mut sim, b, 100);
        assert!(!sim.state.object_event_bits.contains_key(&b));
    }
}
