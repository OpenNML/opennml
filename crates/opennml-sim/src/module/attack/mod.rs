//! Module 17 `PROP_ATTACK` (re/combat-attack): state machine without timers, driven
//! by synchronous feedback messages (walk `0x211`/`0x311`, melee/ranged combat `0x511`/`0x411`), with
//! stances (`0x611`), stubbornness, retaliation (`0x911`), target search ([`scan`]) and combat points.

pub mod scan;

use opennml_formats::property::ModuleDesc;
use opennml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::filter::matches;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_fault, send_mut};
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::sim::Sim;

use super::ModuleKind;

pub const INDEX: usize = 0x11;
/// Busy mask of the search and the retaliation (re/combat-targeting 1.2).
pub const BUSY: u32 = 0xF9EE_06EC;

/// Permission by how the owner moves (on foot, mounted, in a boat) (`AttackAbility`, `0x00500550`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackAbility {
    pub foot: bool,
    pub mounted: bool,
    pub boat: bool,
}

impl AttackAbility {
    /// `ISMOUNTED` -> mounted, otherwise `ISMOUNTEDONBOAT` -> boat, otherwise on foot.
    pub fn allowed(&self, st: u32) -> bool {
        if st & status::ISMOUNTED != 0 {
            self.mounted
        } else if st & status::ISMOUNTEDONBOAT != 0 {
            self.boat
        } else {
            self.foot
        }
    }
}

/// `PropertyAttack` (`0xC8` bytes); fields in savegame order (`0x005028F0`). The fault message
/// taken over with `0x111` together with the target is in the module base (C-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attack {
    /// `m_TargetHID` (`+0x70`).
    pub target: ObjectId,
    /// `m_AttackMode` (`+0x74`): behavior mode 0...4.
    pub mode: i32,
    /// `m_Aggressiveness` (`+0x7C`): 1 no combat, 2 retaliation, 3 escalatable, 4 target search.
    pub aggressiveness: i32,
    /// `m_Stubbornness[3]` (`+0x84`): stance, command, movement.
    pub stubbornness: [i32; 3],
    /// `+0x78`, `+0x80`, `+0x90`: restore values (`0x1111`, `RestoreAggressiveness`).
    pub restore_mode: i32,
    pub restore_aggressiveness: i32,
    pub restore_stubbornness: i32,
    /// `+0xC0`, `+0xBC`, `+0xC4`: backup when quartering (`0x611` with backup, `0x1611`).
    pub backup_mode: i32,
    pub backup_aggressiveness: i32,
    pub backup_stubbornness: i32,
    /// `m_SearchCounter` (`+0x94`).
    pub search_counter: i32,
    /// `m_AutomaticTarget` (`+0x98`): false only for a commanded attack.
    pub automatic: bool,
    /// `m_Status` (`+0x9C`): 0 calm, 1 approach, 2 swing/shot running.
    pub status: i32,
    /// `m_Near`/`m_Far` (`+0xA0`/`+0xA8`).
    pub near: AttackAbility,
    pub far: AttackAbility,
    /// `m_ShootTolerance` (`+0xB4`): 180002 only with `HASSHOOTTOLERANCE`, otherwise -1.
    pub shoot_tolerance: i32,
    /// `m_AttackAnimals` (`+0xB8`): the last attacker had an animal module (C-34).
    pub attack_animals: bool,
}

impl Default for Attack {
    /// Constructor `0x00500870` without the part that needs object and player ([`init`]).
    fn default() -> Self {
        Self {
            target: ObjectId::NONE,
            mode: 0,
            aggressiveness: 0,
            stubbornness: [4, 0, 0],
            restore_mode: 0,
            restore_aggressiveness: 0,
            restore_stubbornness: 0,
            backup_mode: 0,
            backup_aggressiveness: 0,
            backup_stubbornness: 0,
            search_counter: 0,
            automatic: true,
            status: 0,
            near: AttackAbility::default(),
            far: AttackAbility::default(),
            shoot_tolerance: 0,
            attack_animals: false,
        }
    }
}

pub(crate) fn attack<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Attack> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Attack(a) => Some(a),
        _ => None,
    }
}

pub(crate) fn attack_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Attack> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Attack(a) => Some(a),
        _ => None,
    }
}

/// Attack mode message for slot `slot`: AI players (control kind 2) map mode 3 to
/// `{4, 3, 2, 0}` and mode 2 to `{4, 3, 1, 0}` (`FUN_004E5610`), otherwise `FUN_004E5580`.
pub fn attack_mode_for(sim: &Sim, slot: u8, mode: i32) -> Message {
    let ai = sim
        .state
        .players
        .get(usize::from(slot))
        .is_some_and(|p| p.control == crate::state::Control::Ai);
    match (ai, mode) {
        (true, 3) => super::quarter::ai_attack_mode_msg(4, 2),
        (true, 2) => super::quarter::ai_attack_mode_msg(4, 1),
        _ => super::quarter::attack_mode_msg(mode, false),
    }
}

/// `PropertyAttack_ReadParams` (`0x00500CC0`).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = desc.value_int(180001).unwrap_or(0) as u32;
    let tol = if flags & 0x8_0000 != 0 {
        desc.value_int(180002).unwrap_or(0) as i32
    } else {
        -1
    };
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Attack(a) = &mut m.kind {
        a.shoot_tolerance = tol;
        a.near = AttackAbility {
            foot: flags & 0x100 != 0,
            mounted: flags & 0x200 != 0,
            boat: flags & 0x400 != 0,
        };
        a.far = AttackAbility {
            foot: flags & 0x800 != 0,
            mounted: flags & 0x1000 != 0,
            boat: flags & 0x2000 != 0,
        };
    }
}

/// Constructor `0x00500870` (stance of the owner slot, search phase `HID % 1500`) and
/// `PropertyAttack_Init` `0x005001D0` (parameters, rank 0 for player slots 0-7).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let slot = id.slot();
    let mode = sim.state.combat.behavior_mode(slot);
    let msg = attack_mode_for(sim, slot, mode);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.kind = ModuleKind::Attack(Box::new(Attack {
            mode: msg.arg(1),
            aggressiveness: msg.arg(0),
            stubbornness: [msg.arg(2), 0, 0],
            search_counter: id.as_i32() % 1500,
            ..Attack::default()
        }));
    }
    read_params(sim, id, desc);
    if slot < 8 {
        crate::combat::rank::set_rank_reset_experience(sim, id, 0);
    }
}

/// `CanFightNear` (`0x00500810`): checks only the permission, not module 18.
pub fn can_fight_near(sim: &Sim, id: ObjectId) -> bool {
    let st = sim.object(id).map_or(0, |o| o.status);
    attack(sim, id).is_some_and(|a| a.near.allowed(st))
}

/// `CanFightFar` (`0x00500830`).
pub fn can_fight_far(sim: &Sim, id: ObjectId) -> bool {
    let st = sim.object(id).map_or(0, |o| o.status);
    attack(sim, id).is_some_and(|a| a.far.allowed(st))
}

/// `PropertyAttack_HandleMessage` (`0x005021C0`, slot 7; re/combat-attack 2.1). The dead answer only
/// to `0x411`; unknown codes go to [`on_message`] without feedback messages (return value -1).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && msg.code != 0x411 {
        return -1;
    }
    let Some(a) = attack(sim, id).copied() else {
        return -1;
    };
    match msg.code {
        0x211 => {
            on_target_reached(sim, id, msg.arg(0));
            0
        }
        0x611 => {
            apply_attack_mode_msg(sim, id, msg);
            0
        }
        0x711 => a.mode,
        0x811 => {
            let flags = sim.module(id, INDEX).map_or(0, |m| m.base.flags);
            let st = sim.object(id).map_or(0, |o| o.status);
            crate::combat::attack_art(flags, st) as i32
        }
        0x911 => {
            on_msg_attacked_by(sim, id, ObjectId::from_i32(msg.arg(0)));
            0
        }
        0xA11 => {
            if let Some(s) = attack_mut(sim, id) {
                if msg.args.is_empty() {
                    s.stubbornness[2] = 0;
                } else {
                    set_stubbornness(s, msg.arg(0), msg.arg(1), msg.arg(2) == 1);
                }
            }
            0
        }
        0xB11 => i32::from(can_fight_near(sim, id)),
        0xC11 => {
            msg.set_arg(0, a.target.as_i32());
            a.status
        }
        0xD11 => max_reach(sim, id),
        0xE11 => {
            restore_aggressiveness(sim, id);
            1
        }
        0xF11 => get_stubbornness(sim, id),
        0x1011 => a.aggressiveness,
        // `FUN_00500450`: no effect, nobody writes the restore fields (C-18).
        0x1111 => {
            if a.restore_stubbornness != 0
                && let Some(s) = attack_mut(sim, id)
            {
                s.mode = s.restore_mode;
                s.aggressiveness = s.restore_aggressiveness;
                s.stubbornness = [s.restore_stubbornness, 0, 0];
            }
            i32::from(a.restore_stubbornness != 0)
        }
        // `FUN_00500650 {sender, x}`: neighbor alarm; `x` has an animal module -> animals attack.
        0x1211 => {
            let Some(animal) = sim
                .object(ObjectId::from_i32(msg.arg(1)))
                .map(|o| o.has_module(0x14, false))
            else {
                return 0;
            };
            if let Some(s) = attack_mut(sim, id) {
                if s.aggressiveness == 3 {
                    s.restore_aggressiveness = 3;
                    s.aggressiveness = 4;
                }
                if animal {
                    s.attack_animals = true;
                }
            }
            1
        }
        0x1311 => i32::from(can_fight_far(sim, id)),
        // `FUN_005006B0`: compares the argument count instead of `args[0]` (C-11).
        0x1411 => {
            let argc = msg.args.len() as i32;
            i32::from(argc >= 0 && argc == a.target.as_i32() && a.stubbornness[1] > 3)
        }
        // Basis of `MM2_IS_ATTACKING`: the target regardless of the status.
        0x1511 => a.target.as_i32(),
        0x1611 => {
            if let Some(s) = attack_mut(sim, id) {
                s.stubbornness = [s.backup_stubbornness, 0, 0];
                s.mode = s.backup_mode;
                s.aggressiveness = s.backup_aggressiveness;
            }
            0
        }
        _ => {
            on_message(sim, id, &Envelope::bare(msg.clone()));
            -1 // `0x0044A810`
        }
    }
}

/// `FUN_005005E0` (`0xD11`): first `0xB13 {-1, -1}` to ranged combat if far is allowed; then, if
/// near is allowed, **the same** message once more: the original writes `0xA12` into the
/// argument count instead of the ID (`0x0050062F`), so it asks ranged combat again (C-204).
/// Return value the maximum of `args[1]` (-1 without a response).
fn max_reach(sim: &mut Sim, id: ObjectId) -> i32 {
    let mut m = Message::new(0xB13, &[-1, -1]);
    let mut best = -1;
    if can_fight_far(sim, id) {
        send_mut(sim, id, &mut m);
        best = m.arg(1);
    }
    if can_fight_near(sim, id) {
        send_mut(sim, id, &mut m);
        best = best.max(m.arg(1));
    }
    best
}

/// `SetStubbornness(v, i, only_raise)` (`0x005004A0`); indexes outside 0...2 have no effect.
fn set_stubbornness(a: &mut Attack, v: i32, i: i32, only_raise: bool) {
    if let Some(s) = usize::try_from(i)
        .ok()
        .and_then(|i| a.stubbornness.get_mut(i))
        && (!only_raise || *s < v)
    {
        *s = v;
    }
}

/// `GetStubbornness` (`0x005004D0`): maximum; below 4 and walk `m_InObstacle != 0` (without walk -1)
/// -> 4 (C-21).
pub fn get_stubbornness(sim: &mut Sim, id: ObjectId) -> i32 {
    let s = attack(sim, id).map_or(4, |a| {
        a.stubbornness[0]
            .max(0)
            .max(a.stubbornness[1])
            .max(a.stubbornness[2])
    });
    if s < 4 && send(sim, id, &Message::new(0x2400, &[])) != 0 {
        return 4;
    }
    s
}

/// `RestoreAggressiveness` (`0x00500530`).
pub(crate) fn restore_aggressiveness(sim: &mut Sim, id: ObjectId) {
    if let Some(a) = attack_mut(sim, id)
        && a.restore_aggressiveness != 0
    {
        a.aggressiveness = a.restore_aggressiveness;
        a.restore_aggressiveness = 0;
    }
}

/// `FIGHTING` on the object and, for riders, on the mount (`+0x2CC`).
fn set_fighting(sim: &mut Sim, id: ObjectId, on: bool) {
    let Some(o) = sim.object_mut(id) else { return };
    o.set_status_flags(status::FIGHTING, on);
    let (mounted, mount) = (o.status & status::ISMOUNTED != 0, o.target);
    if mounted && let Some(m) = sim.object_mut(mount) {
        m.set_status_flags(status::FIGHTING, on);
    }
}

/// `PropertyAttack_ResetStatus` (`0x005002C0`).
fn reset_status(sim: &mut Sim, id: ObjectId) {
    if let Some(a) = attack_mut(sim, id) {
        a.status = 0;
    }
    set_fighting(sim, id, false);
}

/// `PropertyAttack_EndAttack` (`0x00500310`): calm, command part of stubbornness 0,
/// "automatic", no target; ranged combat `0xC13` (ghost only) and melee `0xB12` (the swing lapses).
/// The search counter stays (C-12).
pub fn end_attack(sim: &mut Sim, id: ObjectId) {
    reset_status(sim, id);
    if let Some(a) = attack_mut(sim, id) {
        a.stubbornness[1] = 0;
        a.automatic = true;
        a.target = ObjectId::NONE;
    }
    send(sim, id, &Message::new(0xC13, &[]));
    send(sim, id, &Message::new(0xB12, &[]));
    if let Some(a) = attack_mut(sim, id) {
        a.attack_animals = false;
    }
}

/// `PropertyAttack_Abort` (`0x00500BC0`, slot 11): end and the stored fault message, also
/// that of a long since normally ended attack (C-1).
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    end_attack(sim, id);
    send_fault(sim, id, INDEX);
    log::debug!("Attack {id:?}: abort");
    true
}

fn pos(sim: &Sim, id: ObjectId) -> (i32, i32) {
    sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0))
}

/// `PropertyAttack_StartAttack(flags, force_melee)` (`0x00500FF0`, re/combat-attack 5.1): choose near/far
/// and give walk the approach (`0x400` FINDTO_UNIT, building in melee `0x200`) with success
/// `0x211 {far}` and fault `0x311`; status 1, `FIGHTING` off. On a failure the target stays
/// (the callers decide).
///
/// QUIRK C-4, C-5: kept
pub fn start_attack(sim: &mut Sim, id: ObjectId, flags: i32, force_melee: bool) -> bool {
    if sim
        .object(id)
        .is_some_and(|o| o.status & status::DIVING != 0)
    {
        return false;
    }
    let Some(a) = attack(sim, id).copied() else {
        return false;
    };
    // `t` stays the unit, what is passed on is the HID of its building (C-5).
    let t = a.target;
    if sim.state.object(t, true).is_none() || send(sim, t, &Message::new(0x4303, &[])) == 1 {
        return false;
    }
    let mut target_hid = t;
    if let Some(c) = crate::collision::quartering_container(sim, t) {
        let prio = scan::priority_filters(sim, id);
        if scan::find_match_index(sim, Some(c), &prio, i32::from(id.slot())) < 0 {
            return false;
        }
        if let Some(s) = attack_mut(sim, id) {
            s.target = c;
        }
        target_hid = c;
    }
    let big_ship = send(sim, t, &Message::new(0x3700, &[])) == 1;
    let mut range = Message::new(0xB13, &[-1, -1]);
    let mut melee = true;
    if !force_melee && can_fight_far(sim, id) {
        if can_fight_near(sim, id) && !big_ship {
            send_mut(sim, id, &mut range);
            // Center distance, walk afterwards measures the edge distance (C-6).
            let d = crate::collision::fixed_pos_distance(pos(sim, t), pos(sim, id));
            melee = range.arg(0) > d;
        } else {
            melee = false;
        }
    } else if !can_fight_near(sim, id) {
        log::debug!("Attack {id:?}: Can't attack in melee or ranged!");
        return false;
    }
    let mut dir_offset = 0;
    if melee {
        if big_ship {
            return false;
        }
        // The original sets only the ID of the already used `0xB13` message (`0x0050118E`); module
        // 18 always overwrites both arguments (`0x0051437B`), and in the shipped data every type with
        // melee permission has module 18 - the old arguments are thus never visible.
        range = Message::new(0xA12, &[-1, -1]);
    } else if send(sim, id, &Message::new(0x513, &[])) == 1 {
        dir_offset = 90 << 12; // own ship: broadside
    }
    send_mut(sim, id, &mut range);
    let mut wf = flags | 0x40;
    if a.automatic {
        wf |= 0x02 | 0x20;
    }
    if sim.object(id).is_some_and(|o| o.has_module(0x14, false)) {
        wf |= 0x100;
    }
    let building = sim.object(t).is_some_and(|o| o.has_module(3, false));
    let msg = if melee && building {
        Message::new(0x200, &[target_hid.as_i32(), a.shoot_tolerance, wf])
    } else {
        Message::new(
            0x400,
            &[
                target_hid.as_i32(),
                range.arg(0),
                range.arg(1),
                dir_offset,
                a.shoot_tolerance,
                wf,
                0,
                0,
            ],
        )
    };
    let env = Envelope {
        msg,
        success: Some(Message::new(0x211, &[i32::from(!melee)])),
        fault: Some(Message::new(0x311, &[])),
        success_target: id,
        fault_target: id,
    };
    if let Some(s) = attack_mut(sim, id) {
        s.status = 1;
    }
    set_fighting(sim, id, false);
    if dispatch_command(sim, id, &env) != 0 {
        return true;
    }
    if !melee && can_fight_near(sim, id) {
        // C-4: ranged combat again; ends because walk `0x311` has already deleted the target.
        return start_attack(sim, id, wf, force_melee);
    }
    if let Some(s) = attack_mut(sim, id) {
        s.status = 0;
    }
    false
}

/// `PropertyAttack_OnTargetReached` (`0x00500A60`, re/combat-attack 5.2): order a swing (`0x112`) or
/// shot (`0xA13`) with success `0x511` and fault `0x411`; the result does not count
/// (C-2). When swimming first abort, then continue (C-10; the target is gone then -> end).
fn on_target_reached(sim: &mut Sim, id: ObjectId, far: i32) {
    if sim
        .object(id)
        .is_some_and(|o| o.status & status::SWIMMING != 0)
    {
        abort(sim, id);
    }
    let target = attack(sim, id).map_or(ObjectId::NONE, |a| a.target);
    if sim.state.object(target, true).is_none() {
        end_attack(sim, id);
        return;
    }
    let (st, mount) = sim
        .object(id)
        .map_or((0, ObjectId::NONE), |o| (o.status, o.target));
    let mount_moving = sim
        .object(mount)
        .is_some_and(|m| m.status & status::MOVE != 0);
    if mount_moving || st & status::MOVE != 0 {
        send(sim, id, &Message::new(0xC00, &[1]));
    }
    let code = if far == 1 { 0xA13 } else { 0x112 };
    set_fighting(sim, id, true);
    if let Some(s) = attack_mut(sim, id) {
        s.status = 2;
    }
    let env = Envelope {
        msg: Message::new(code, &[target.as_i32()]),
        success: Some(Message::new(0x511, &[])),
        fault: Some(Message::new(0x411, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// `PropertyAttack_ApplyAttackModeMsg` (`0x00501670`, message `0x611 {aggr, mode, stubb0,
/// backup}`): mode 0 aborts a running attack, mode 1/2 restarts a commanded one
/// (then "automatic", C-7) and aborts an automatic one; 3/4 let it run.
fn apply_attack_mode_msg(sim: &mut Sim, id: ObjectId, msg: &Message) {
    let backup = msg.arg(3) == 1;
    if !backup {
        restore_aggressiveness(sim, id);
    }
    let Some(a) = attack_mut(sim, id) else { return };
    if backup {
        a.backup_aggressiveness = a.aggressiveness;
        a.backup_mode = a.mode;
        a.backup_stubbornness = a.stubbornness[0];
    }
    a.aggressiveness = msg.arg(0);
    a.stubbornness[0] = msg.arg(2);
    // A change of mode otherwise only notifies the UI (selection).
    a.mode = msg.arg(1);
    let (target, automatic) = (a.target, a.automatic);
    if target.is_none() {
        return;
    }
    match msg.arg(1) {
        0 => {
            abort(sim, id);
        }
        1 | 2 => {
            if automatic {
                abort(sim, id);
                return;
            }
            end_attack(sim, id);
            if let Some(a) = attack_mut(sim, id) {
                a.target = target;
            }
            if !start_attack(sim, id, 0, false)
                && let Some(a) = attack_mut(sim, id)
            {
                a.target = ObjectId::NONE;
            }
        }
        _ => {}
    }
}

/// Free for search and retaliation: none of the bits of the busy mask.
fn is_free(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_some_and(|o| o.status & BUSY == 0)
}

/// `PropertyAttack_OnAttackedBy` (`0x00501E70`, re/combat-attack 6.2, re/combat-targeting 1.10):
/// aggressiveness 2 strikes back at once when calm (without enemy and stubbornness check, C-9);
/// otherwise an enemy without a fighting target becomes the target (even without an attack, C-8),
/// attacked only with aggressiveness 4 and free.
pub fn on_attacked_by(sim: &mut Sim, id: ObjectId, a: ObjectId) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    let (st, mount) = (o.status, o.target);
    if st & status::CARRIING != 0 {
        return true;
    }
    let Some(attacker_animal) = sim.state.object(a, true).map(|o| o.has_module(0x14, false)) else {
        return false;
    };
    let Some(s) = attack(sim, id).copied() else {
        return false;
    };
    if a == s.target {
        return false;
    }
    let mounted = st & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0;
    if s.aggressiveness == 2 {
        let mover = if mounted && sim.object(mount).is_some() {
            mount
        } else {
            id
        };
        let moving = sim
            .object(mover)
            .is_some_and(|m| m.status & status::MOVE != 0);
        if s.status == 0 && !moving {
            if let Some(x) = attack_mut(sim, id) {
                x.restore_aggressiveness = x.aggressiveness;
                x.target = a;
                x.automatic = true;
                if x.stubbornness[1] == 3 {
                    x.stubbornness[1] = 0;
                }
            }
            start_attack(sim, id, 0, false);
        }
        return true;
    }
    if let Some(x) = attack_mut(sim, id) {
        x.attack_animals = attacker_animal; // C-34
        if x.stubbornness[1] == 3 {
            x.stubbornness[1] = 0;
        }
    }
    if s.status == 2 || get_stubbornness(sim, id) > 3 {
        return false;
    }
    let owner = i32::from(id.slot());
    let mut f = scan::enemy_filter(sim, id);
    f.mode &= !0x200_0000; // without fog
    if !matches(sim, a, &f, owner) {
        return false;
    }
    // Static filter `0x00672620`: the current target has an attack module -> stay.
    let fighter = MatchFilter {
        mode: 0x14,
        mode2: 4,
        prop_id: 0x11,
        ..MatchFilter::constructed()
    };
    if sim.state.object(s.target, true).is_some() && matches(sim, s.target, &fighter, owner) {
        return false;
    }
    if let Some(x) = attack_mut(sim, id) {
        x.target = a;
        x.automatic = true;
    }
    let mount_free = sim.object(mount).is_none_or(|m| m.status & BUSY == 0);
    if s.aggressiveness == 4 && is_free(sim, id) && (!mounted || mount_free) {
        return start_attack(sim, id, 0, false);
    }
    false
}

/// `0x911 {attacker}` (`0x005023E4`): if the retaliation fails and has changed the target,
/// the old target and "automatic" come back and the old target is approached again (silent at -1).
fn on_msg_attacked_by(sim: &mut Sim, id: ObjectId, a: ObjectId) {
    let Some(old) = attack(sim, id).copied() else {
        return;
    };
    if !on_attacked_by(sim, id, a) && attack(sim, id).is_some_and(|s| s.target != old.target) {
        if let Some(s) = attack_mut(sim, id) {
            s.automatic = old.automatic;
            s.target = old.target;
        }
        start_attack(sim, id, 0, false);
    }
}

/// `PropertyAttack_OnCmdAttack` (`0x005018C0`, message `0x111 {target, flags}` or with
/// ground position `{target, flags, px, py, pz}`): if the target is gone and a position is there, the
/// unit walks there (FINDTO without feedback messages, flags 1). If the start fails, "no
/// target, automatic" applies again.
fn on_cmd_attack(sim: &mut Sim, id: ObjectId, msg: &Message) -> bool {
    let t = ObjectId::from_i32(msg.arg(0));
    if sim.state.object(t, true).is_none() {
        if msg.args.len() != 2 {
            // Tile size 128 (`IDIV`); the direction `DAT_006724C8` is assumed to be NONE (hypothesis).
            let (x, z) = (msg.arg(2) / 128, msg.arg(4) / 128);
            send(
                sim,
                id,
                &Message::new(0x200, &[x, z, -1, NO_DIRECTION, -1, 1]),
            );
            return true;
        }
        return false;
    }
    if let Some(s) = attack_mut(sim, id) {
        s.target = t;
        s.automatic = false;
    }
    if start_attack(sim, id, msg.arg(1), false) {
        return true;
    }
    if let Some(s) = attack_mut(sim, id) {
        s.target = ObjectId::NONE;
        s.automatic = true;
    }
    false
}

/// `PropertyAttack_OnMessage` (`0x005020F0`, slot 12; re/combat-attack 2.2). After every swing
/// (`0x511`): retaliation against the last attacker (never ages), target search, restart on the same
/// target, otherwise end. `0x311` ends without a new attempt.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && env.msg.code != 0x411 {
        return false;
    }
    match env.msg.code {
        0x111 => {
            if let Some(m) = sim.module_mut(id, INDEX) {
                m.base.set_success(env.success.as_ref(), env.success_target);
                m.base.set_fault(env.fault.as_ref(), env.fault_target);
            }
            on_cmd_attack(sim, id, &env.msg);
            true
        }
        0x311 => {
            end_attack(sim, id);
            true
        }
        0x411 => abort(sim, id),
        0x511 => {
            reset_status(sim, id);
            let la = sim.object(id).map_or(ObjectId::NONE, |o| o.last_attacker);
            if !la.is_none() && on_attacked_by(sim, id, la) {
                return true;
            }
            if scan::scan_for_target(sim, id) {
                return true;
            }
            if !start_attack(sim, id, 0x1000, false) {
                end_attack(sim, id);
            }
            true
        }
        _ => true,
    }
}

/// `PropertyAttack_Update` (`0x00500200`, slot 13): reduce stubbornness, search counter (remainder
/// discarded, continues in status 2, C-12), combat points per target owner from status 1 (C-160).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let Some(a) = attack(sim, id).copied() else {
        return;
    };
    let (st, mount) = sim
        .object(id)
        .map_or((0, ObjectId::NONE), |o| (o.status, o.target));
    let target_gone = sim.state.object(a.target, true).is_none();
    let mount_moving = sim
        .object(mount)
        .is_some_and(|m| m.status & status::MOVE != 0);
    let counter = a.search_counter.wrapping_add(dt);
    let Some(s) = attack_mut(sim, id) else { return };
    if s.stubbornness[1] > 0 && target_gone {
        s.stubbornness[1] = 0;
    }
    if s.stubbornness[2] > 0 && st & status::MOVE == 0 && !mount_moving {
        s.stubbornness[2] = 0;
    }
    s.search_counter = counter;
    if s.status != 2 && counter > 1499 {
        s.search_counter = 0;
        scan::scan_for_target(sim, id);
    }
    if let Some(s) = attack(sim, id).copied()
        && s.status != 0
    {
        add_attack_points(sim, id, s.target);
    }
}

/// `Combat_AddAttackMusicPoints` (`0x00478E30`): max HP of the attacker to the owner of the target
/// (from the ID, even if the target is gone); owners from 8 do not count.
fn add_attack_points(sim: &mut Sim, id: ObjectId, target: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, 14);
    let hp = o.attributes.max_value(14, p);
    if let Some(v) = sim
        .state
        .combat
        .attack_points
        .get_mut(usize::from(target.slot()))
    {
        *v = v.wrapping_add(hp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::module::test_hook;
    use crate::state::{Control, SimEvent};

    /// Range `0xA12` for module 18 as an idle module; has no effect (module 18 answers
    /// itself, the hook only applies to idle modules).
    fn near_range(_: &mut Sim, _: ObjectId, index: usize, msg: &mut Message) -> i32 {
        if index == 0x12 && msg.code == 0xA12 {
            msg.set_arg(0, 0);
            msg.set_arg(1, 2048);
            return 0;
        }
        -1
    }

    fn snapshot(sim: &Sim, id: ObjectId) -> Attack {
        *attack(sim, id).expect("attack module")
    }

    /// Two fighters center to center 3072 (edge distance 2048), the attacker looks toward the target.
    fn pair(sim: &mut Sim, a: u32, t: u32) -> (ObjectId, ObjectId) {
        pair_at(sim, a, t, 3072)
    }

    /// Like [`pair`] with center distance `d` (fixed point).
    fn pair_at(sim: &mut Sim, a: u32, t: u32, d: i32) -> (ObjectId, ObjectId) {
        let x = (5 << 12) + 0x800;
        let world = |fx: i32| fx as f32 / 32.0;
        let mk = |sim: &mut Sim, guid, slot, fx| {
            crate::world::create_object(
                sim,
                &mut crate::world::ObjectDesc::new(guid, slot, [world(fx), 0.0, world(x)]),
            )
            .unwrap()
        };
        let att = mk(sim, a, 0, x);
        let tgt = mk(sim, t, 1, x + d);
        sim.object_mut(att).unwrap().direction = crate::fx::Fx(0);
        (att, tgt)
    }

    // QUIRK C-17: kept
    #[test]
    fn init_takes_the_player_behaviour_the_ai_mapping_and_rank_zero() {
        let rules = rules();
        let mut state = state(&rules);
        state.players[1].control = Control::Ai;
        state.combat.behavior_mode[2] = 0;
        let mut sim = Sim::new(&mut state, &rules);
        let h = spawn(&mut sim, SABRE, 0, (4, 4));
        let a = snapshot(&sim, h);
        assert_eq!(
            (a.aggressiveness, a.mode, a.stubbornness),
            (2, 2, [1, 0, 0])
        );
        assert_eq!((a.target, a.automatic, a.status), (ObjectId::NONE, true, 0));
        assert_eq!(a.search_counter, h.as_i32() % 1500);
        assert_eq!(
            (a.near, a.far, a.shoot_tolerance),
            (
                AttackAbility {
                    foot: true,
                    mounted: false,
                    boat: false
                },
                AttackAbility::default(),
                -1
            )
        );
        assert_eq!(
            sim.object(h).unwrap().attributes.max_value(30, 0),
            100,
            "rank 0 set"
        );
        // AI, mode 2 -> {4, 3, 1}; mode 3 -> {4, 3, 2}.
        let k = spawn(&mut sim, SABRE, 1, (8, 8));
        let s = snapshot(&sim, k);
        assert_eq!(
            (s.aggressiveness, s.mode, s.stubbornness),
            (4, 3, [1, 0, 0])
        );
        sim.state.combat.behavior_mode[1] = 3;
        let k3 = spawn(&mut sim, SABRE, 1, (10, 10));
        assert_eq!(snapshot(&sim, k3).stubbornness, [2, 0, 0]);
        // Human, mode 0 "CEASE FIRE" -> {1, 0, 4}.
        let z = spawn(&mut sim, SABRE, 2, (12, 12));
        let s = snapshot(&sim, z);
        assert_eq!((s.aggressiveness, s.stubbornness[0]), (1, 4));
    }

    // QUIRK C-11, C-204: kept
    #[test]
    fn queries_answer_like_the_original() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (4, 4));
        let q = |sim: &mut Sim, code, args: &[i32]| send(sim, w, &Message::new(code, args));
        assert_eq!(q(&mut sim, 0x711, &[]), 2);
        assert_eq!(
            q(&mut sim, 0x811, &[]) as u32,
            opennml_formats::bonus::ART_SWORD_PEEK
        );
        assert_eq!((q(&mut sim, 0xB11, &[]), q(&mut sim, 0x1311, &[])), (1, 0));
        assert_eq!(q(&mut sim, 0x1011, &[]), 2);
        assert_eq!(q(&mut sim, 0x1511, &[]), -1);
        // 0xD11 asks for melee once more 0xB13 instead of 0xA12 (C-204): without a ranged combat module -1.
        test_hook::with(near_range, || {
            assert_eq!(q(&mut sim, 0xD11, &[]), -1);
        });
        // Quirk: 0x1411 compares the argument count with the target HID -> 0.
        assert_eq!(q(&mut sim, 0x1411, &[w.as_i32()]), 0);
        // 0xA11 {v, i, only raise}: [1] = 4; {} clears [2].
        q(&mut sim, 0xA11, &[4, 1, 1]);
        q(&mut sim, 0xA11, &[2, 2, 0]);
        assert_eq!(snapshot(&sim, w).stubbornness, [1, 4, 2]);
        q(&mut sim, 0xA11, &[]);
        assert_eq!(snapshot(&sim, w).stubbornness, [1, 4, 0]);
        assert_eq!(q(&mut sim, 0xF11, &[]), 4);
        // 0x1211: neighbor alarm raises 3 to 4 (restore 3).
        attack_mut(&mut sim, w).unwrap().aggressiveness = 3;
        assert_eq!(q(&mut sim, 0x1211, &[0, w.as_i32()]), 1);
        let a = snapshot(&sim, w);
        assert_eq!((a.aggressiveness, a.restore_aggressiveness), (4, 3));
        assert_eq!(q(&mut sim, 0xE11, &[]), 1);
        assert_eq!(snapshot(&sim, w).aggressiveness, 3);
        // 0xC11: target in args[0], return value status.
        let mut c = Message::new(0xC11, &[0]);
        assert_eq!(send_mut(&mut sim, w, &mut c), 0);
        assert_eq!(c.args, [-1]);
        // Dead: everything -1 except 0x411.
        sim.object_mut(w).unwrap().status |= crate::object::status::DIED;
        assert_eq!(q(&mut sim, 0x711, &[]), -1);
    }

    // QUIRK C-2: kept
    #[test]
    fn a_commanded_attack_reaches_the_target_and_restarts_after_each_swing() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, t) = pair(&mut sim, SABRE, WARRIOR);
        test_hook::with(near_range, || {
            // 0x111 via the query path (like the garrison): OnMessage runs, return value -1.
            assert_eq!(
                send(&mut sim, a, &Message::new(0x111, &[t.as_i32(), 0])),
                -1
            );
            let s = snapshot(&sim, a);
            assert_eq!(
                (s.target, s.automatic, s.status),
                (t, false, 2),
                "in range -> swing at once"
            );
            assert!(sim.object(a).unwrap().status & crate::object::status::FIGHTING != 0);
            // ATTACK_DONE: status 0, retaliation/search (stance 2: none), restart on the same target.
            send(&mut sim, a, &Message::new(0x511, &[]));
            let s = snapshot(&sim, a);
            assert_eq!((s.status, s.target), (2, t));
            // Target gone -> restart fails -> end: target -1, automatic, FIGHTING off.
            sim.object_mut(t).unwrap().status |= crate::object::status::DIED;
            send(&mut sim, a, &Message::new(0x511, &[]));
            let s = snapshot(&sim, a);
            assert_eq!(
                (s.target, s.automatic, s.status, s.stubbornness[1]),
                (ObjectId::NONE, true, 0, 0)
            );
            assert!(sim.object(a).unwrap().status & crate::object::status::FIGHTING == 0);
        });
    }

    #[test]
    fn stubbornness_follows_commands_moves_and_vanished_targets() {
        // re/combat-attack 11: stance 3, attack command -> [1, 4, 0].
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, t) = pair(&mut sim, SABRE, WARRIOR);
        send(
            &mut sim,
            a,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        test_hook::with(near_range, || {
            send(&mut sim, a, &Message::new(0x111, &[t.as_i32(), 0]));
        });
        assert_eq!(snapshot(&sim, a).stubbornness, [1, 4, 0]);
        // Target dead -> update sets [1] = 0.
        sim.object_mut(t).unwrap().status |= crate::object::status::DIED;
        super::tick(&mut sim, a, 100);
        assert_eq!(snapshot(&sim, a).stubbornness[1], 0);
    }

    // QUIRK C-21: kept
    #[test]
    fn a_unit_stuck_in_an_obstacle_counts_as_deaf() {
        // C-21: stubbornness below 4 and walk `m_InObstacle` -> 4.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, SABRE, 0, (4, 4));
        assert_eq!(get_stubbornness(&mut sim, w), 1);
        crate::module::walk::walk_mut(&mut sim, w)
            .unwrap()
            .in_obstacle = true;
        assert_eq!(get_stubbornness(&mut sim, w), 4);
    }

    // QUIRK C-12: kept
    #[test]
    fn the_search_counter_drops_the_rest_and_runs_on_while_fighting() {
        // C-12: in status 2 no trigger, the counter keeps growing; otherwise = 0 (remainder discarded).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, SABRE, 0, (4, 4));
        let s = attack_mut(&mut sim, w).unwrap();
        s.search_counter = 1450;
        s.status = 2;
        super::tick(&mut sim, w, 100);
        assert_eq!(snapshot(&sim, w).search_counter, 1550);
        attack_mut(&mut sim, w).unwrap().status = 0;
        super::tick(&mut sim, w, 100);
        assert_eq!(snapshot(&sim, w).search_counter, 0);
        // The end of the attack does not reset it.
        attack_mut(&mut sim, w).unwrap().search_counter = 700;
        end_attack(&mut sim, w);
        assert_eq!(snapshot(&sim, w).search_counter, 700);
    }

    // QUIRK C-8, C-9: kept
    #[test]
    fn defensive_units_strike_back_at_once_and_others_only_remember() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (v, att) = pair(&mut sim, SABRE, WARRIOR);
        // Stance 2 (default), calm: strike back at once (C-9).
        test_hook::with(near_range, || {
            crate::messaging::notify_attacked_by(&mut sim, v, att);
        });
        let s = snapshot(&sim, v);
        assert_eq!((s.target, s.automatic), (att, true));
        assert!(s.status > 0);
        // Aggressiveness 3 (or 1): target remembered, no attack; the 0x911 branch restores the old
        // target (NONE) and restarts it - fails silently (C-8).
        let (v3, att3) = pair(&mut sim, SABRE, WARRIOR);
        attack_mut(&mut sim, v3).unwrap().aggressiveness = 3;
        test_hook::with(near_range, || {
            crate::messaging::notify_attacked_by(&mut sim, v3, att3);
        });
        let s = snapshot(&sim, v3);
        assert_eq!((s.target, s.status), (ObjectId::NONE, 0));
    }

    // QUIRK C-7: kept
    #[test]
    fn stance_changes_abort_or_restart_a_running_attack() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, t) = pair(&mut sim, SABRE, WARRIOR);
        test_hook::with(near_range, || {
            send(&mut sim, a, &Message::new(0x111, &[t.as_i32(), 0]));
            // Mode 2 with a commanded attack: restart, afterwards "automatic" (C-7).
            send(
                &mut sim,
                a,
                &crate::module::quarter::attack_mode_msg(2, false),
            );
            let s = snapshot(&sim, a);
            assert_eq!((s.target, s.automatic), (t, true));
            // Mode 0: abort (end + stored fault message).
            send(
                &mut sim,
                a,
                &crate::module::quarter::attack_mode_msg(0, false),
            );
            assert_eq!(snapshot(&sim, a).target, ObjectId::NONE);
        });
    }

    // QUIRK C-160: kept
    #[test]
    fn attack_points_sum_max_hp_per_target_owner_every_double_tick() {
        let rules = rules();
        let mut state = state(&rules);
        {
            let mut sim = Sim::new(&mut state, &rules);
            let t = spawn(&mut sim, WARRIOR, 1, (20, 20));
            let n = spawn(&mut sim, WARRIOR, 8, (25, 25));
            for (i, target) in [t, t, t, n].into_iter().enumerate() {
                let a = spawn(&mut sim, SABRE, 0, (4 + 2 * i as i32, 4));
                let s = attack_mut(&mut sim, a).unwrap();
                s.status = 1; // approach counts too (C-160)
                s.target = target;
            }
        }
        let mut points = Vec::new();
        for _ in 0..6 {
            for e in crate::tick::step(&mut state, &rules, &[]) {
                if let SimEvent::AttackPoints { points: p } = e {
                    points.push(p);
                }
            }
        }
        assert_eq!(points.len(), 3, "one event per double tick");
        assert_eq!(points[0], [0; 8], "before the first object ticks");
        assert_eq!(points[1][1], 3 * 190, "three saber fighters against slot 1");
        assert!(
            points
                .iter()
                .all(|p| p.iter().sum::<i32>() == p[1] || p.iter().sum::<i32>() == 0),
            "slot 8 discarded"
        );
    }

    thread_local! {
        static SEEN: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Observer: codes of all queries to the modules of the object.
    fn record(_: &mut Sim, _: ObjectId, _: usize, msg: &Message) {
        SEEN.with(|s| s.borrow_mut().push(msg.code));
    }

    fn seen(code: u32) -> usize {
        SEEN.with(|s| s.borrow().iter().filter(|&&c| c == code).count())
    }

    // QUIRK C-6: kept
    #[test]
    fn near_or_far_is_chosen_by_the_centre_distance() {
        // re/combat-attack 11: melee (0xA12) as long as `Fixed_Hypot` < far minimum distance 4096
        // (C-6). `Fixed_Hypot(4093...4095, 0)` = 4096 (`Fixed_Div` with 10 bits), so 4092 is the last near one.
        let rules = rules();
        for (d, near) in [(4092, true), (4093, false), (4096, false)] {
            let mut state = state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let (a, t) = pair_at(&mut sim, ENG_INFANTRY, WARRIOR, d);
            SEEN.with(|s| s.borrow_mut().clear());
            test_hook::observe(record, || {
                send(&mut sim, a, &Message::new(0x111, &[t.as_i32(), 0]));
            });
            // Choice with 0xB13, then the range of the chosen module for walk.
            let expected = if near { (1, 1) } else { (0, 2) };
            assert_eq!((seen(0xA12), seen(0xB13)), expected, "distance {d}");
        }
    }

    // QUIRK C-1: kept
    #[test]
    fn the_stored_fault_goes_out_only_on_the_next_abort() {
        // C-1: no success message; the fault message of the command stays after a normal end.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, t) = pair(&mut sim, SABRE, WARRIOR);
        let env = Envelope {
            msg: Message::new(0x111, &[t.as_i32(), 0]),
            success: Some(Message::new(0x1C12, &[])),
            fault: Some(Message::new(0x1D12, &[])),
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        SEEN.with(|s| s.borrow_mut().clear());
        test_hook::observe(record, || {
            test_hook::with(near_range, || {
                assert_eq!(dispatch_command(&mut sim, a, &env), 1);
                sim.object_mut(t).unwrap().status |= status::DIED;
                send(&mut sim, a, &Message::new(0x511, &[]));
            });
            assert_eq!(snapshot(&sim, a).target, ObjectId::NONE, "ended normally");
            assert_eq!((seen(0x1C12), seen(0x1D12)), (0, 0));
            crate::messaging::abort_all_modules(&mut sim, a);
        });
        assert_eq!((seen(0x1C12), seen(0x1D12)), (0, 1));
    }

    // QUIRK C-10: kept
    #[test]
    fn a_swimming_unit_aborts_when_it_reaches_its_target() {
        // C-10: abort (end + fault message) instead of swing.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, t) = pair(&mut sim, SABRE, WARRIOR);
        let s = attack_mut(&mut sim, a).unwrap();
        s.target = t;
        s.status = 1;
        sim.object_mut(a).unwrap().status |= status::SWIMMING;
        send(&mut sim, a, &Message::new(0x211, &[0]));
        let s = snapshot(&sim, a);
        assert_eq!((s.target, s.status), (ObjectId::NONE, 0));
        assert!(sim.object(a).unwrap().status & status::FIGHTING == 0);
    }
}
