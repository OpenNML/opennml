//! Module 10 `CPropertyShootProjectile` (re/combat-ranged 2, 3): arrow, rifle, cannon
//! via timers to the animation markers. One shot per order `0x10A {target, zone, side}`; the
//! completion message `Finish` sends `ATTACK_DONE 0x511` to the attack module. The module does not turn
//! the shooter and checks no range. The broadside of the warships (`SHIP`: aiming
//! `0x10A`, firing `0xD0A`) is in [`super::broadside`]; siege engines (`0x2E`) do not exist in the
//! shipped data.

use std::collections::BTreeSet;

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{send, send_fault, send_success};
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 0x0A;
pub(super) const GROUP: u8 = 0x80;
pub(super) const GROUP_KEEP: u8 = 0x81;

/// Flags (VALUEID 110001, `MID_PROP_SHOOT_*` without low byte).
pub mod flag {
    pub const ARROW: u32 = 0x100;
    pub const CANON: u32 = 0x200;
    pub const GUN: u32 = 0x400;
    pub const RELOAD: u32 = 0x1_0000;
    pub const SHIP: u32 = 0x2_0000;
    pub const FIREARROW: u32 = 0x4_0000;
    pub const HOOK: u32 = 0x8_0000;
    pub const CANSHOOTFIREARROW: u32 = 0x20_0000;
}

/// `CPropertyShootProjectile` (`0xF4` bytes); in the savegame like `0x00527B90`. `+0xC4` (target point
/// of the siege engine) is omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shoot {
    /// `m_MainObject` (`+0x70`, 110007): mesh names.
    pub main_object: String,
    /// `m_TargetHID` (`+0x74`).
    pub target: ObjectId,
    /// `m_Hitpoints` (`+0x78`): damage per shot (arrow, rifle).
    pub hitpoints: i32,
    /// `m_TZoneID` (`+0x7C`).
    pub tzone: i32,
    /// `m_BulletsCur`/`m_BulletsCount` (`+0x80`/`+0x84`).
    pub bullets_cur: i32,
    pub bullets_count: i32,
    /// `m_ShootDelay` (`+0x88`).
    pub shoot_delay: i32,
    /// `m_CanShoot`, `m_SendDone`, `m_IsGunPresent` (`+0x8C…+0x8E`).
    pub can_shoot: bool,
    pub send_done: bool,
    pub is_gun_present: bool,
    /// `m_ShootPosition` (`+0x90`): helper node of the muzzle.
    pub shoot_position: String,
    /// `m_IsBusy` (`+0x94`).
    pub is_busy: bool,
    /// `m_IsSteuerboard` (`+0x95`): side of the broadside, 1 = starboard (re/ships 3.6).
    pub is_steuerboard: bool,
    /// `m_CannonCounter` (`+0x98`): after aiming cannon count + 1 (re/ships 1.3).
    pub cannon_counter: i32,
    /// `m_GunSelect` (`+0x9C`): 1 if the target is a game animal (module `0x15`).
    pub gun_select: i32,
    /// `m_PresentAnim`, `m_ShootAnim`, `m_ReloadAnim` (110003, 110002, 110004).
    pub present_anim: Vec<i32>,
    pub shoot_anim: Vec<i32>,
    pub reload_anim: Vec<i32>,
    /// `m_ProjectileVelocity`/`m_ProjectileSlopeAngle` (110008/110009 `<< 12`, never used).
    pub velocity: i32,
    pub slope: i32,
    /// Resolved meshes from `m_MainObject` (`+0xE4`): `None` = name without a mesh.
    pub models: Vec<Option<String>>,
}

impl Default for Shoot {
    fn default() -> Self {
        Self {
            main_object: String::new(),
            target: ObjectId::NONE,
            hitpoints: 0,
            tzone: -1,
            bullets_cur: 0,
            bullets_count: 0,
            shoot_delay: 6000,
            can_shoot: true,
            send_done: false,
            is_gun_present: false,
            shoot_position: String::new(),
            is_busy: false,
            is_steuerboard: false,
            cannon_counter: 0,
            gun_select: 0,
            present_anim: Vec::new(),
            shoot_anim: Vec::new(),
            reload_anim: Vec::new(),
            velocity: 0,
            slope: 0,
            models: Vec::new(),
        }
    }
}

pub(crate) fn shoot<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Shoot> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Shoot(s) => Some(s),
        _ => None,
    }
}

pub(crate) fn shoot_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Shoot> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Shoot(s) => Some(s),
        _ => None,
    }
}

/// `Module_HasFlags` (`0x004FD280`): without low byte.
pub(crate) fn has(sim: &Sim, id: ObjectId, f: u32) -> bool {
    sim.module(id, INDEX).is_some_and(|m| m.base.has_flags(f))
}

/// `PropertyShootProjectile_ResolveMainObject` (`0x005257C0`, re/combat-projectiles 15.6): split at
/// spaces and line breaks; each name lowercased against the mesh names of the
/// shooter; names before the last one also go unresolved (`None`) into the list, the last only resolved.
pub fn resolve_models(text: &str, names: Option<&BTreeSet<String>>) -> Vec<Option<String>> {
    let resolve = |t: &str| {
        let t = t.to_lowercase();
        names.and_then(|n| n.contains(&t).then_some(t))
    };
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find([' ', '\n']) {
        out.push(resolve(&rest[..i]));
        rest = &rest[i + 1..];
    }
    if let Some(m) = resolve(rest) {
        out.push(Some(m));
    }
    out
}

/// Init (`0x005247A0`): flags 110001, then ReadParams.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    read_params(sim, id, desc);
}

/// `PropertyShootProjectile_ReadParams` (`0x00525D00`, re/combat-ranged 2.3).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let int = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let flags = int(110001) as u32;
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let main = text(110007);
    let models = resolve_models(&main, sim.rules.mesh_names.get(&guid));
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    let ModuleKind::Shoot(s) = &mut m.kind else {
        return;
    };
    s.main_object = main;
    s.models = models;
    s.shoot_anim = super::stand::parse_int_list(&text(110002), &s.shoot_anim);
    let has = |f: u32| flags & f != 0;
    if has(flag::CANON) {
        s.shoot_position = "Muendung01".into();
    } else if has(flag::ARROW) {
        s.shoot_position = "schussposition01".into();
    } else if has(flag::GUN) {
        s.shoot_position = "gun01".into();
    } else if has(flag::SHIP) {
        s.present_anim = super::stand::parse_int_list(&text(110003), &s.present_anim);
        s.reload_anim = super::stand::parse_int_list(&text(110004), &s.reload_anim);
    }
    s.shoot_delay = 6000;
    s.can_shoot = true;
    s.is_busy = false;
    s.is_gun_present = false;
    s.velocity = int(110008).wrapping_shl(12);
    s.slope = int(110009).wrapping_shl(12);
    if has(flag::RELOAD) {
        s.bullets_count = int(110005).max(1);
        s.bullets_cur = 0;
        s.present_anim = super::stand::parse_int_list(&text(110003), &s.present_anim);
        s.reload_anim = super::stand::parse_int_list(&text(110004), &s.reload_anim);
        s.shoot_delay = 2000;
        if has(flag::ARROW) {
            s.bullets_cur = s.bullets_count;
        }
    }
}

/// `PropertyShootProjectile_GetAnim(kind, selection)` (`0x005248B0`): riders/boats ask their riding
/// or boat module (`0x50D`/`0x50E {kind, selection}`, re/riding 2.3); otherwise 1 shoot, 2 present,
/// 3 reload animation `list[selection mod n]` (empty list: -1 instead of division by 0, exception A7).
/// The notation `{0x50D, 2, kind, selection}` in re/combat-ranged 2.3 includes the argument count 2.
pub(crate) fn get_anim(sim: &mut Sim, id: ObjectId, kind: i32, sel: i32) -> i32 {
    let st = sim.object(id).map_or(0, |o| o.status);
    if st & status::ISMOUNTED != 0 {
        return send(sim, id, &Message::new(0x50D, &[kind, sel]));
    }
    if st & status::ISMOUNTEDONBOAT != 0 {
        return send(sim, id, &Message::new(0x50E, &[kind, sel]));
    }
    let Some(s) = shoot(sim, id) else { return 0 };
    let list = match kind {
        1 => &s.shoot_anim,
        2 => &s.present_anim,
        3 => &s.reload_anim,
        _ => return 0,
    };
    super::stand::list_modulo(list, sel)
}

/// `GameObject_GetAnimLabelTime(anim, mark)`: `.rdm` marker, missing -> 0.
pub(crate) fn label(sim: &Sim, id: ObjectId, anim: i32, mark: i32) -> i32 {
    super::gather::anim_label_time(sim, id, anim, mark)
}

/// `Module_PostDelayedMessage(d, msg, once, group)`.
pub(crate) fn post(sim: &mut Sim, id: ObjectId, delay: i32, msg: Message, group: u8) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.post_delayed_message(delay, msg, false, group);
    }
}

fn clear_group(sim: &mut Sim, id: ObjectId, group: u8) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(group);
    }
}

/// `GameObject_SetAnimation(anim, 128.0, 1, …)`: with restart.
pub(crate) fn set_animation(sim: &mut Sim, id: ObjectId, anim: i32) {
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, true, false, false);
    }
}

/// Mount animation when aiming (re/combat-ranged 3.1/3.3): only `ISMOUNTED` (not in a boat); the
/// shooter asks its **own** riding module `0x50D {10}` (list `140011`), the mount applies
/// the answer at speed 128 without restart.
fn set_mount_animation(sim: &mut Sim, id: ObjectId) {
    if sim
        .object(id)
        .is_none_or(|o| o.status & status::ISMOUNTED == 0)
    {
        return;
    }
    let Some(t) = super::walk::mount_of(sim, id) else {
        return;
    };
    let a = send(sim, id, &Message::new(0x50D, &[10]));
    if let Some(m) = sim.object_mut(t) {
        m.set_animation(a, 0, false, false, false);
    }
}

/// `PropertyShootProjectile_Finish` (`0x00524890`): not busy, success message.
pub(crate) fn finish(sim: &mut Sim, id: ObjectId) {
    if let Some(s) = shoot_mut(sim, id) {
        s.is_busy = false;
    }
    send_success(sim, id, INDEX);
}

/// Vision source at the shooter for the **owner of the target** (re/combat-ranged 4, C-95): size `size`,
/// 3000 ms, explored + seen; key = ID of the shooter.
pub(crate) fn vision(sim: &mut Sim, id: ObjectId, size: i32, target: ObjectId) {
    crate::fog::set_vision_source(
        &mut sim.state.fog,
        crate::fog::VisionSource {
            x: -1,
            z_or_hid: id.as_i32(),
            size,
            player: i32::from(target.slot()),
            remaining_ms: 3000,
            flags: crate::fog::EXPLORE | crate::fog::SEE,
        },
    );
}

/// `PropertyShootProjectile_SetMeshesVisible` (`0x00525A50`).
fn set_meshes_visible(sim: &mut Sim, id: ObjectId, visible: bool) {
    let names = shoot(sim, id)
        .map(|s| s.main_object.clone())
        .unwrap_or_default();
    let starts = sim.object(id).map_or(0, |o| o.anim.starts);
    sim.state.events.push(SimEvent::MeshVisibility {
        object: id,
        names,
        visible,
        anim_starts: starts,
    });
}

/// `PropertyShootProjectile_AimArrow` (`0x00527440`, re/combat-ranged 3.1).
fn aim_arrow(sim: &mut Sim, id: ObjectId) {
    if has(sim, id, flag::HOOK)
        && let Some(o) = sim.object_mut(id)
    {
        o.render_bits |= 0x400; // Node `geschoss01` (re/combat-host 1.3), never set in the shipped data
    }
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    // `GetObjectById(target, 0)`: dying targets count too, it does not end until LaunchArrow (re/combat-ranged 3.1).
    if sim.object(s.target).is_none() {
        finish(sim, id);
        return;
    }
    if !s.can_shoot {
        if let Some(s) = shoot_mut(sim, id) {
            s.send_done = true;
        }
        return;
    }
    if has(sim, id, flag::RELOAD) && s.bullets_cur >= s.bullets_count {
        start_reload(sim, id, Message::new(0x140A, &[s.target.as_i32(), s.tzone]));
        return;
    }
    if let Some(s) = shoot_mut(sim, id) {
        s.is_busy = false; // `HasModule(0x2E)` - siege engine, never in the shipped data
    }
    let anim = get_anim(sim, id, 1, 0);
    set_animation(sim, id, anim);
    set_mount_animation(sim, id);
    clear_group(sim, id, GROUP_KEEP);
    let l = |sim: &Sim, m| label(sim, id, anim, m);
    let t3 = l(sim, 3);
    if t3 != 0 {
        post(sim, id, t3.max(100), Message::new(0x40A, &[]), GROUP); // C-80: without marker 3 no arrow
    }
    post(sim, id, l(sim, 2).max(100), Message::new(0x50A, &[]), GROUP);
    let t5 = l(sim, 5);
    if t5 != 0 {
        post(sim, id, t5.max(100), Message::new(0x110A, &[]), GROUP);
    }
    let end = l(sim, 1);
    post(sim, id, end.max(100), Message::new(0x60A, &[]), GROUP);
    post(sim, id, end.max(100), Message::new(0x160A, &[]), GROUP_KEEP);
    let hp = sim.attribute(id, 20).wrapping_mul(end) / 2000;
    if let Some(s) = shoot_mut(sim, id) {
        s.hitpoints = hp;
    }
    post(
        sim,
        id,
        end.wrapping_sub(300).max(100),
        Message::new(0x120A, &[]),
        GROUP,
    );
    vision(sim, id, 2, s.target);
}

/// `PropertyShootProjectile_StartReload(msg)` (`0x00524990`): reload animation, `msg` after marker 1
/// (without minimum). Only `ARROW | RELOAD` (never in the shipped data).
fn start_reload(sim: &mut Sim, id: ObjectId, msg: Message) {
    let a = shoot(sim, id).map_or(-1, |s| s.reload_anim.first().copied().unwrap_or(0));
    let d = label(sim, id, a, 1);
    post(sim, id, d, msg, GROUP);
    set_animation(sim, id, a);
}

/// `PropertyShootProjectile_LaunchArrow` (`0x00525F90`, re/combat-ranged 3.2).
///
/// QUIRK C-88: deviation (A7)
fn launch_arrow(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    vision(sim, id, 4, s.target);
    let alive = sim.state.object(s.target, true).is_some();
    if !alive {
        finish(sim, id); // without a siege engine no ground shot
        return;
    }
    let target_pos =
        crate::combat::hit_zone::hit_zone_world_pos(sim, s.target, s.tzone).unwrap_or([0.0; 3]);
    // Quartered: start at the shoot position of the building toward the hit zone (without effect).
    let quartered = quartered_start(sim, id, target_pos, None);
    let start = quartered
        .or_else(|| crate::combat::hit_zone::world_pos(sim, id))
        .unwrap_or([0.0; 3]);
    if has(sim, id, flag::RELOAD)
        && let Some(x) = shoot_mut(sim, id)
    {
        x.bullets_cur = x.bullets_cur.wrapping_add(1);
    }
    if has(sim, id, flag::HOOK)
        && let Some(o) = sim.object_mut(id)
    {
        o.render_bits &= !0x400;
    }
    // C-96: fire arrow automatic at every shot (bit 13 of the player variant mask).
    if has(sim, id, flag::CANSHOOTFIREARROW)
        && crate::variants::player_variant_mask(sim, id) & 0x2000 != 0
    {
        let building = crate::filter::matches_class(sim, s.target, 0x1_00FF)
            || sim.object(s.target).is_some_and(|o| o.has_module(3, false));
        super::set_match_bits(sim, id, 0x4_000A, building);
        super::set_match_bits(sim, id, 0x4_0011, building);
    }
    let fire = has(sim, id, flag::FIREARROW);
    if s.models.is_empty() {
        // C-88: the original jumps into a CRT error routine; here no arrow (exception A7).
        log::warn!(
            "Shoot module {id:?}: no projectile models in \"{}\" - no arrow",
            s.main_object
        );
    } else {
        let damage = crate::combat::damage::damage_message(sim, id, s.hitpoints.wrapping_neg());
        crate::combat::projectile::launch_arrow(
            sim,
            crate::combat::projectile::ArrowLaunch {
                shooter: id,
                target: s.target,
                damage,
                start,
                target_pos,
                zone: s.tzone,
                fire,
                model: s.models[0].clone().unwrap_or_default(),
                node: if quartered.is_some() {
                    String::new()
                } else {
                    "schussposition01".into()
                },
            },
        );
        let guid = sim.object(id).map_or(0, |o| o.guid);
        sim.state.events.push(SimEvent::Sound3d {
            object: id,
            action: 5,
            flags: 0x23,
            pos: start,
            guid,
        });
    }
    set_meshes_visible(sim, id, false);
    if let Some(x) = shoot_mut(sim, id) {
        x.can_shoot = false;
    }
}

/// `PropertyShootProjectile_AimCannon` (`0x00524AA0`, re/combat-ranged 3.5): waits without
/// `CanShoot` (SendDone); otherwise clear group `0x80`, shoot animation `110002[0]`, `CANNONANIM` at
/// marker 1, shot at marker 3 (at least 100 ms each; without marker 3 after 100 ms, C-81),
/// vision source 4. Siege engine (`0x2E`) omitted.
fn aim_cannon(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    if !s.can_shoot {
        if let Some(x) = shoot_mut(sim, id) {
            x.send_done = true;
        }
        return;
    }
    clear_group(sim, id, GROUP);
    let a = s.shoot_anim.first().copied().unwrap_or(0);
    set_animation(sim, id, a);
    post(
        sim,
        id,
        label(sim, id, a, 1).max(100),
        Message::new(0x80A, &[]),
        GROUP,
    );
    post(
        sim,
        id,
        label(sim, id, a, 3).max(100),
        Message::new(0x130A, &[]),
        GROUP,
    );
    vision(sim, id, 4, s.target);
}

/// `PropertyShootProjectile_FireCannon` (`0x005264C0`, re/combat-ranged 3.5): target alive (otherwise
/// Finish); target point = hit zone `m_TZoneID` (for walk targets lowered to `max(terrain, water)`);
/// sound 1 (`0x22`); ball with damage `−attribute 20` (current, re/combat-ranged 0);
/// `CanShoot = 0`, `CANNONDELAY` after `ShootDelay` in group `0x81` (survives every abort, C-86).
fn fire_cannon(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    if sim.state.object(s.target, true).is_none() {
        finish(sim, id);
        return;
    }
    // Quartered: start at the shoot position of the building toward the target position, `kanone_rauch.pef` at the
    // building.
    let aim = crate::combat::hit_zone::world_pos(sim, s.target).unwrap_or([0.0; 3]);
    let quartered = quartered_start(sim, id, aim, Some(CANNON_SMOKE));
    let start = quartered
        .or_else(|| crate::combat::hit_zone::world_pos(sim, id))
        .unwrap_or([0.0; 3]);
    let mut tp =
        crate::combat::hit_zone::hit_zone_world_pos(sim, s.target, s.tzone).unwrap_or(start);
    let guid = sim.object(id).map_or(0, |o| o.guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 1,
        flags: 0x22,
        pos: start,
        guid,
    });
    if sim.object(s.target).is_some_and(|o| o.has_module(0, false)) {
        let ground = crate::height::terrain_height(sim.rules, tp[0], tp[1], tp[2]);
        tp[1] = ground.max(sim.rules.map.water_level);
    }
    let amount = sim.attribute(id, 20).wrapping_neg();
    let damage = crate::combat::damage::damage_message(sim, id, amount);
    crate::combat::projectile::launch_ball(
        sim,
        crate::combat::projectile::BallLaunch {
            shooter: id,
            target: s.target,
            damage,
            start,
            target_pos: tp,
            node: if quartered.is_some() {
                String::new()
            } else {
                "Muendung01".into()
            },
            yaw_extra: 0.0,
        },
    );
    if let Some(x) = shoot_mut(sim, id) {
        x.can_shoot = false;
    }
    post(sim, id, s.shoot_delay, Message::new(0x70A, &[]), GROUP_KEEP);
}

const MUZZLE_FLASH: &str = "data/effects/normaler_schuss.pef";
const CANNON_SMOKE: &str = "data/effects/kanone_rauch.pef";

/// `PropertyShootProjectile_QuarteredShootPos(target)` (`0x00525500`, re/combat-ranged 3.2): without a
/// container (`GameObject_GetQuarteringContainer` `0x00466BB0`) the world position of the shooter; otherwise among
/// the helper nodes `schussposition01`, `02`, ... (until one is missing) of the container the one with the smallest
/// distance to `target − shooter position` (start 1e9, strictly smaller: on a tie the first), rotated
/// by the yaw of the container (`+0x158`), plus container position. Without a node (0, 0, 0) remains, i.e. the
/// container position.
///
/// The target direction is **not** rotated into model space (O-283): the choice applies to the
/// unrotated tower. Distances as in the original in `f32` (`Vector3_Length` under 24-bit x87, like
/// [`crate::combat::hit_zone::pick_hit_zone_towards`]); nodes in the rest pose of the building shape
/// (like [`crate::footprint::helper_node_map_position`]).
fn quartered_shoot_pos(sim: &Sim, id: ObjectId, target_pos: [f32; 3]) -> [f32; 3] {
    use crate::combat::hit_zone::{vector3_length, world_pos};
    let own = world_pos(sim, id).unwrap_or([0.0; 3]);
    let Some(c) = crate::collision::quartering_container(sim, id) else {
        return own;
    };
    let rel = [
        target_pos[0] - own[0],
        target_pos[1] - own[1],
        target_pos[2] - own[2],
    ];
    let (guid, yaw) = sim.object(c).map_or((0, 0.0), |o| (o.guid, o.rotation[1]));
    let shape = sim.rules.building_shapes.get(&guid);
    let mut best = [0.0f32; 3];
    let mut min = 1e9f32;
    for i in 1.. {
        let Some(node) = shape.and_then(|s| s.helper_node(&format!("schussposition{i:02}"))) else {
            break;
        };
        let d = vector3_length([node[0] - rel[0], node[1] - rel[1], node[2] - rel[2]]);
        if d < min {
            min = d;
            best = node;
        }
    }
    let m = crate::footprint::world_matrix([0.0; 3], [0.0, yaw, 0.0], [1.0; 3]);
    let r = crate::footprint::transform_coord(best, &m);
    let w = world_pos(sim, c).unwrap_or([0.0; 3]);
    [r[0] + w[0], r[1] + w[1], r[2] + w[2]]
}

/// Start point of a quartered shooter ([`quartered_shoot_pos`] to `aim`) and, with a container,
/// the effect `path` as own effect of the container at this point, rotated to `aim` (FireGun
/// `0x005269E0…0x00526BB3`, FireCannon `0x00526563…0x0052660B`). `None` if the shooter is not
/// quartered.
fn quartered_start(
    sim: &mut Sim,
    id: ObjectId,
    aim: [f32; 3],
    path: Option<&str>,
) -> Option<[f32; 3]> {
    if sim
        .object(id)
        .is_none_or(|o| o.status & status::QUARTERED == 0)
    {
        return None;
    }
    let start = quartered_shoot_pos(sim, id, aim);
    if let (Some(path), Some(c)) = (path, crate::collision::quartering_container(sim, id)) {
        sim.state.events.push(SimEvent::EffectOwnedAt {
            object: c,
            path: path.into(),
            pos: start,
            aim,
        });
    }
    Some(start)
}

/// `PropertyShootProjectile_AimGun` (`0x00524D10`, re/combat-ranged 3.3): with a magazine, present
/// on the first shot (`GUNSTART` after marker 1 of the present animation, **without** the 100 minimum) and
/// compute the damage per shot from the whole cycle duration (C-92); empty magazine -> only reload
/// animation (C-90). Otherwise damage `attack · S1 / 2000` and at once `StartGunShot`.
fn aim_gun(sim: &mut Sim, id: ObjectId) {
    set_mount_animation(sim, id);
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    let reload = has(sim, id, flag::RELOAD);
    if !s.is_gun_present && reload {
        let sel = i32::from(
            sim.state
                .object(s.target, true)
                .is_some_and(|t| t.has_module(0x15, false)),
        );
        if let Some(x) = shoot_mut(sim, id) {
            x.gun_select = sel;
        }
        if s.bullets_cur >= s.bullets_count {
            let r = get_anim(sim, id, 3, sel);
            set_animation(sim, id, r);
            return;
        }
        let p = get_anim(sim, id, 2, sel);
        set_animation(sim, id, p);
        post(
            sim,
            id,
            label(sim, id, p, 1),
            Message::new(0x90A, &[]),
            GROUP,
        );
        let sa = get_anim(sim, id, 1, sel);
        let r = get_anim(sim, id, 3, sel);
        let t = label(sim, id, p, 1)
            .wrapping_add(label(sim, id, sa, 1).wrapping_mul(s.bullets_count.wrapping_sub(1)))
            .wrapping_add(label(sim, id, sa, 4))
            .wrapping_add(label(sim, id, r, 1));
        let d = sim.attribute(id, 20).wrapping_mul(t) / 2000;
        if let Some(x) = shoot_mut(sim, id) {
            x.hitpoints = if x.bullets_count > 0 {
                d / x.bullets_count
            } else {
                d
            };
            x.is_gun_present = true;
        }
        return;
    }
    let sa = get_anim(sim, id, 1, s.gun_select);
    let hp = sim.attribute(id, 20).wrapping_mul(label(sim, id, sa, 1)) / 2000;
    if let Some(x) = shoot_mut(sim, id) {
        x.hitpoints = hp;
    }
    start_gun_shot(sim, id);
}

/// `PropertyShootProjectile_StartGunShot` (`0x00524C00`, `GUNSTART`): vision source 4, shoot animation,
/// `GUNSHOOTING` at marker 3; without a magazine `GUNRELOADED` at marker 1.
fn start_gun_shot(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    vision(sim, id, 4, s.target);
    let sa = get_anim(sim, id, 1, s.gun_select);
    set_animation(sim, id, sa);
    post(
        sim,
        id,
        label(sim, id, sa, 3).max(100),
        Message::new(0xC0A, &[]),
        GROUP,
    );
    if !has(sim, id, flag::RELOAD) {
        post(
            sim,
            id,
            label(sim, id, sa, 1).max(100),
            Message::new(0xB0A, &[]),
            GROUP,
        );
    }
}

/// `PropertyShootProjectile_FireGun` (`0x00526930`, `GUNSHOOTING`): instant hit without range, sight or
/// line-of-sight check (C-93). Muzzle flash at the node (host), sound 1; hit zone toward the
/// shooter (the counter turns, C-91); building/`0x20000`/`0x10000` -> sparks and sound 2, otherwise blood without
/// sound (C-115 comes from the sound table). With a magazine: next shot after the rest of the
/// shoot animation or reload.
fn fire_gun(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    if sim.state.object(s.target, true).is_none() {
        finish(sim, id);
        return;
    }
    // Quartered: start at the shoot position of the building toward the target position, muzzle flash at the building.
    let aim = crate::combat::hit_zone::world_pos(sim, s.target).unwrap_or([0.0; 3]);
    let start = match quartered_start(sim, id, aim, Some(MUZZLE_FLASH)) {
        Some(start) => start,
        None => {
            sim.state.events.push(SimEvent::EffectAttached {
                object: id,
                path: MUZZLE_FLASH.into(),
                node: s.shoot_position.clone(),
                offset: [0.0; 3],
            });
            crate::combat::hit_zone::world_pos(sim, id).unwrap_or([0.0; 3])
        }
    };
    let guid = sim.object(id).map_or(0, |o| o.guid);
    sim.state.events.push(SimEvent::Sound3d {
        object: id,
        action: 1,
        flags: 0x23,
        pos: start,
        guid,
    });
    let tz = crate::combat::hit_zone::world_pos(sim, s.target).unwrap_or(start);
    let hit =
        crate::combat::hit_zone::pick_hit_zone_towards(sim, s.target, start).map_or(tz, |z| z.1);
    let hard = sim.object(s.target).is_some_and(|o| o.has_module(3, false))
        || crate::filter::matches_class(sim, s.target, 0x2_0000)
        || crate::filter::matches_class(sim, s.target, 0x1_0000);
    if hard {
        sim.state.events.push(SimEvent::EffectAt {
            path: "data/effects/funken.pef".into(),
            pos: hit,
            yaw: 0.0,
            scale: 1.0,
            start_ms: 0,
        });
        sim.state.events.push(SimEvent::Sound3d {
            object: id,
            action: 2,
            flags: 0x23,
            pos: hit,
            guid,
        });
    } else {
        sim.state.events.push(SimEvent::EffectAttached {
            object: s.target,
            path: crate::combat::projectile::BLOOD.into(),
            node: String::new(),
            offset: [hit[0] - tz[0], hit[1] - tz[1], hit[2] - tz[2]],
        });
    }
    let msg = crate::combat::damage::damage_message(sim, id, s.hitpoints.wrapping_neg());
    send(sim, s.target, &msg);
    if !has(sim, id, flag::RELOAD) {
        return;
    }
    let cur = shoot_mut(sim, id).map_or(0, |x| {
        x.bullets_cur = x.bullets_cur.wrapping_add(1);
        x.bullets_cur
    });
    let sa = get_anim(sim, id, 1, s.gun_select);
    clear_group(sim, id, 0xFF);
    if cur < s.bullets_count {
        let elapsed = sim.module(id, INDEX).map_or(0, |m| m.base.cur_prop_time);
        let d = label(sim, id, sa, 1).max(100).wrapping_sub(elapsed);
        post(sim, id, d, Message::new(0xA0A, &[]), GROUP);
    } else {
        let mut t4 = label(sim, id, sa, 4);
        if t4 <= 0 {
            t4 = label(sim, id, sa, 1);
        }
        let d = t4.wrapping_sub(label(sim, id, sa, 3));
        let ra = get_anim(sim, id, 3, s.gun_select);
        let r = label(sim, id, ra, 1).max(100);
        post(sim, id, d, Message::new(0xA0A, &[]), GROUP);
        post(
            sim,
            id,
            d.wrapping_add(r),
            Message::new(0xB0A, &[]),
            GROUP_KEEP,
        );
        if let Some(x) = shoot_mut(sim, id) {
            x.is_gun_present = false;
        }
    }
}

/// `GUNDELAY` (`0xA0A`): magazine not empty -> Finish; otherwise with `RELOAD` the reload animation.
fn gun_delay(sim: &mut Sim, id: ObjectId) {
    let Some(s) = shoot(sim, id).cloned() else {
        return;
    };
    if s.bullets_cur < s.bullets_count {
        finish(sim, id);
    } else if has(sim, id, flag::RELOAD) {
        let r = get_anim(sim, id, 3, s.gun_select);
        set_animation(sim, id, r);
    }
}

/// `GUNRELOADED` (`0xB0A`): not busy, magazine full, presented, Finish.
fn gun_reloaded(sim: &mut Sim, id: ObjectId) {
    if let Some(x) = shoot_mut(sim, id) {
        x.is_busy = false;
        x.bullets_cur = 0;
        x.is_gun_present = true;
    }
    finish(sim, id);
}

/// `PropertyShootProjectile_HandleMessage` (`0x005277F0`, re/combat-ranged 2.5) with the broadside
/// (`0x10A` at `SHIP`, `0xD0A`) from [`super::broadside`]; the dead answer -1 except to `0xF0A`.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && msg.code != 0xF0A {
        return -1;
    }
    if shoot(sim, id).is_none() {
        return -1;
    }
    match msg.code {
        0x10A | 0x20A | 0x140A => {
            if let Some(s) = shoot_mut(sim, id) {
                if msg.code == 0x140A {
                    s.bullets_cur = 0;
                }
                s.target = ObjectId::from_i32(msg.arg(0));
                s.tzone = if msg.args.len() >= 2 { msg.arg(1) } else { -1 };
            }
            if has(sim, id, flag::ARROW) {
                aim_arrow(sim, id);
            } else if has(sim, id, flag::CANON) {
                aim_cannon(sim, id);
            } else if has(sim, id, flag::GUN) {
                aim_gun(sim, id);
            } else if has(sim, id, flag::SHIP) {
                super::broadside::aim_ship(sim, id, msg);
            }
            0
        }
        0xD0A => super::broadside::fire_ship_broadside(sim, id, msg.arg(0)),
        0x130A => {
            fire_cannon(sim, id);
            1
        }
        0x90A => {
            start_gun_shot(sim, id);
            0
        }
        0xA0A => {
            gun_delay(sim, id);
            0
        }
        0xB0A => {
            gun_reloaded(sim, id);
            0
        }
        0xC0A => {
            fire_gun(sim, id);
            0
        }
        0x40A => {
            launch_arrow(sim, id);
            0
        }
        0x50A => {
            set_meshes_visible(sim, id, true);
            0
        }
        0x110A => {
            set_meshes_visible(sim, id, false);
            0
        }
        0x60A => {
            finish(sim, id);
            0
        }
        0x70A | 0xE0A => {
            if let Some(s) = shoot_mut(sim, id) {
                s.can_shoot = true;
            }
            finish(sim, id);
            0
        }
        0x80A | 0x120A => {
            if let Some(s) = shoot_mut(sim, id) {
                s.is_busy = false;
            }
            0
        }
        0xF0A => i32::from(abort(sim, id)),
        0x100A => i32::from(has(sim, id, flag::CANON)),
        0x150A => shoot(sim, id).map_or(-1, |s| {
            if msg.arg(0) == s.target.as_i32() {
                s.hitpoints
            } else {
                -1
            }
        }),
        0x160A => {
            let was = shoot_mut(sim, id).is_some_and(|s| {
                s.can_shoot = true;
                std::mem::take(&mut s.send_done)
            });
            if was {
                finish(sim, id);
            }
            1
        }
        _ => -1,
    }
}

/// `PropertyShootProjectile_OnCommand` (`0x00524830`, slot 12): dead -> only `0xF0A`; feedback messages
/// only with `0x10A`; then the query, accepted for return value != -1.
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let dead = sim.object(id).is_none_or(|o| o.status & status::DIED != 0);
    if dead && env.msg.code != 0xF0A {
        return false;
    }
    if env.msg.code == 0x10A
        && let Some(m) = sim.module_mut(id, INDEX)
    {
        m.base.set_success(env.success.as_ref(), env.success_target);
        m.base.set_fault(env.fault.as_ref(), env.fault_target);
    }
    let mut m = env.msg.clone();
    query(sim, id, &mut m) != -1
}

/// `PropertyShootProjectile_Abort` (`0x005247D0`): gun off, not busy; arrow/gun
/// clear group `0x80` (cannon and ship none, C-86); fault message.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(s) = shoot_mut(sim, id) {
        s.is_gun_present = false;
        s.is_busy = false;
    }
    if has(sim, id, flag::ARROW | flag::GUN) {
        clear_group(sim, id, GROUP);
    }
    send_fault(sim, id, INDEX);
    true
}

/// Tick `0x005119A0`: only `UpdateEvents`.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::state::SimEvent;

    /// Archer (slot 0) and warrior (slot 1) 4 tiles apart, facing each other, attack
    /// like command `0x11`.
    fn duel(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
    ) -> (ObjectId, ObjectId) {
        let mut sim = Sim::new(state, rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let t = spawn(&mut sim, WARRIOR, 1, (9, 5));
        sim.object_mut(a).unwrap().direction = crate::fx::Fx(0);
        sim.object_mut(t).unwrap().direction = crate::fx::Fx(180 << 12);
        // The warrior does not defend himself (stance 0), so the flow of the bow stays undisturbed.
        send(
            &mut sim,
            t,
            &crate::module::quarter::attack_mode_msg(0, false),
        );
        crate::messaging::post_message(&mut sim, a, Message::new(0x111, &[t.as_i32(), 0]), true);
        (a, t)
    }

    /// Per step: (tick, events); up to `steps` steps.
    fn run(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
        steps: usize,
    ) -> Vec<(u32, SimEvent)> {
        let mut out = Vec::new();
        for _ in 0..steps {
            for e in crate::tick::step(state, rules, &[]) {
                out.push((state.tick, e));
            }
        }
        out
    }

    #[test]
    fn an_archer_looses_at_marker_3_and_hits_eight_effect_ticks_later() {
        // re/combat-ranged 10: shot tick S+8, arrow visible again S+9, end S+24; damage
        // 11 * 2333 / 2000 = 12 after ceil(731/100) = 8 effect ticks (hit S+15).
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules);
        // S = tick of aiming: the attack module jumps to status 2.
        let mut s = None;
        let mut events = Vec::new();
        for _ in 0..2 * 45 {
            for e in crate::tick::step(&mut state, &rules, &[]) {
                events.push((state.tick, e));
            }
            let sim = Sim::new(&mut state, &rules);
            if s.is_none() && crate::module::attack::attack(&sim, a).is_some_and(|x| x.status == 2)
            {
                s = Some(sim.state.tick);
            }
        }
        let s = s.expect("aims");
        let at = |f: &dyn Fn(&SimEvent) -> bool| -> Vec<u32> {
            events
                .iter()
                .filter(|(_, e)| f(e))
                .map(|(k, _)| *k - s)
                .collect()
        };
        assert_eq!(
            at(&|e| matches!(e, SimEvent::ProjectileLaunched { .. })),
            vec![8, 32]
        );
        assert_eq!(
            at(&|e| matches!(
                e,
                SimEvent::Sound3d {
                    action: 5,
                    flags: 0x23,
                    ..
                }
            )),
            vec![8, 32]
        );
        assert_eq!(
            at(&|e| matches!(e, SimEvent::MeshVisibility { visible: true, .. })),
            vec![9, 33],
            "marker 2: arrow back in the hand"
        );
        assert_eq!(
            at(&|e| matches!(e, SimEvent::ProjectileEnded { hit: true, .. })),
            vec![15, 39]
        );
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            sim.object(t).unwrap().attributes.current_value(14, 0),
            160 - 24
        );
    }

    // QUIRK C-95: kept
    #[test]
    fn aiming_and_loosing_set_vision_sources_for_the_target_owner() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, _t) = duel(&mut state, &rules);
        let source = |state: &crate::state::SimState| {
            state
                .fog
                .sources
                .iter()
                .find(|s| s.x == -1 && s.z_or_hid == a.as_i32())
                .map(|s| (s.size, s.player, s.flags))
        };
        let mut sizes = Vec::new();
        for _ in 0..2 * 10 {
            crate::tick::step(&mut state, &rules, &[]);
            if let Some(v) = source(&state)
                && sizes.last() != Some(&v)
            {
                sizes.push(v);
            }
        }
        let flags = crate::fog::EXPLORE | crate::fog::SEE;
        assert_eq!(
            sizes,
            vec![(2, 1, flags), (4, 1, flags)],
            "aiming 2, shot 4, player of the target (C-95)"
        );
    }

    #[test]
    fn the_target_dying_before_marker_3_fires_no_arrow() {
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules);
        for _ in 0..2 * 3 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        Sim::new(&mut state, &rules).object_mut(t).unwrap().status |= crate::object::status::DIED;
        let events = run(&mut state, &rules, 2 * 10);
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. }))
        );
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            crate::module::attack::attack(&sim, a).unwrap().target,
            ObjectId::NONE
        );
    }

    #[test]
    fn a_dying_target_still_lets_the_bow_aim_but_launch_ends_without_an_arrow() {
        // re/combat-ranged 3.1: AimArrow looks for the target with `GetObjectById(target, 0)` - a dying
        // target (0x40000, still in the object table) counts; LaunchArrow (without dying ones) ends without an arrow.
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = spawn(&mut sim, ARCHER, 0, (5, 5));
            let t = spawn(&mut sim, WARRIOR, 1, (9, 5));
            sim.object_mut(t).unwrap().status |= crate::object::status::DIED;
            let env = Envelope {
                msg: Message::new(0x10A, &[t.as_i32(), 0, -1]),
                success: Some(Message::new(0x7FF, &[])),
                fault: None,
                success_target: ObjectId::NONE,
                fault_target: ObjectId::NONE,
            };
            assert!(on_message(&mut sim, a, &env));
            (a, t)
        };
        let source = |state: &crate::state::SimState| {
            state
                .fog
                .sources
                .iter()
                .find(|s| s.x == -1 && s.z_or_hid == a.as_i32())
                .map(|s| (s.size, s.player))
        };
        assert_eq!(
            source(&state),
            Some((2, 1)),
            "AimArrow started: vision source 2"
        );
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(shoot(&sim, a).unwrap().target, t);
        assert!(
            sim.module(a, INDEX).is_some_and(|m| m
                .base
                .timers_ws
                .iter()
                .any(|e| e.msg.code == 0x40A)),
            "ARROWSTART (marker 3) is scheduled"
        );
        let events = run(&mut state, &rules, 2 * 10);
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. })),
            "LaunchArrow ends without an arrow for a dead target"
        );
    }

    // QUIRK C-3, C-87: kept
    #[test]
    fn an_order_while_the_bow_is_not_ready_ends_at_canshoot_without_a_shot() {
        // C-87: abort in the middle of the shot clears 0x60A (group 0x80), 0x160A (0x81) stays; a new
        // aim order before 0x160A waits (SendDone) and ends at 0x160A via Finish without a shot.
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = duel(&mut state, &rules);
        let events = run(&mut state, &rules, 2 * 10);
        assert_eq!(
            events
                .iter()
                .filter(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. }))
                .count(),
            1
        );
        let mut sim = Sim::new(&mut state, &rules);
        crate::module::abort(&mut sim, a, INDEX);
        assert!(!shoot(&sim, a).unwrap().can_shoot);
        let env = Envelope {
            msg: Message::new(0x10A, &[t.as_i32(), 0, -1]),
            success: Some(Message::new(0x7FF, &[])),
            fault: None,
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        assert!(on_message(&mut sim, a, &env));
        assert!(shoot(&sim, a).unwrap().send_done);
        let events = run(&mut state, &rules, 2 * 20);
        let sim = Sim::new(&mut state, &rules);
        assert!(!shoot(&sim, a).unwrap().send_done, "ended at 0x160A");
        assert!(shoot(&sim, a).unwrap().can_shoot);
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. })),
            "the waiting order does not shoot"
        );
    }

    // QUIRK C-96: kept
    #[test]
    fn fire_arrows_are_chosen_per_shot_with_variant_bit_13() {
        // C-96: fire arrow with a building target and bit 13; FIREARROW at the shoot module, FIRE at the attack module.
        let mut rules = rules();
        rules
            .variant_rules
            .push(opennml_formats::gifcon::VariantRule {
                bit: 13,
                terms: vec![1],
            });
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let hq = spawn(&mut sim, HQ, 1, (9, 5));
        let w = spawn(&mut sim, WARRIOR, 1, (5, 9));
        let shot = |sim: &mut Sim, t: ObjectId| {
            if let Some(s) = shoot_mut(sim, a) {
                s.target = t;
                s.tzone = -1;
                s.can_shoot = true;
            }
            sim.state.events.clear();
            send(sim, a, &Message::new(0x40A, &[]));
            sim.state.events.iter().find_map(|e| match e {
                SimEvent::ProjectileLaunched { fire, .. } => Some(*fire),
                _ => None,
            })
        };
        assert_eq!(shot(&mut sim, hq), Some(true));
        assert!(sim.module(a, INDEX).unwrap().base.flags & flag::FIREARROW != 0);
        assert!(
            sim.module(a, 0x11).unwrap().base.flags & 0x4_0000 != 0,
            "FIRE at the attack module"
        );
        assert_eq!(shot(&mut sim, w), Some(false));
        assert!(sim.module(a, 0x11).unwrap().base.flags & 0x4_0000 == 0);
    }

    // QUIRK C-80: kept
    #[test]
    fn an_archer_without_marker_3_looses_no_arrow() {
        // C-80: without time marker 3 `aim_arrow` does not schedule the shot `0x40A` - the order runs
        // to the end (marker 1), but no projectile arises.
        let mut rules = rules();
        rules.set_anim_labels(ARCHER, 5, &[(1, 2333), (2, 833)]);
        let mut state = state(&rules);
        let (a, _t) = duel(&mut state, &rules);
        let events = run(&mut state, &rules, 2 * 45);
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. })),
            "no arrow without marker 3"
        );
        let sim = Sim::new(&mut state, &rules);
        assert!(
            shoot(&sim, a).unwrap().can_shoot,
            "the end (0x60A/0x160A) still runs"
        );
    }

    #[test]
    fn ranged_damage_wraps_like_the_original() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let t = spawn(&mut sim, WARRIOR, 1, (9, 5));
        sim.object_mut(a)
            .unwrap()
            .attributes
            .set_modifier(20, i32::MAX, false, 5);
        // Attribute 20 = base value 11 + i32::MAX = -2,147,483,638 (overflow, not tracked -> maximum).
        assert_eq!(sim.attribute(a, 20), -2_147_483_638);
        send(&mut sim, a, &Message::new(0x10A, &[t.as_i32(), 0, -1]));
        // By hand: -2,147,483,638 * 2333 mod 2^32 = 2^31 + 23,330 (2333 odd, so 2^31 * 2333 is congruent to 2^31),
        // as i32 -2,147,460,318; divided by 2000 (rounded toward 0) = -1,073,730.
        assert_eq!(shoot(&sim, a).unwrap().hitpoints, -1_073_730);
    }

    #[test]
    fn queries_and_busy_follow_the_flags() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ARCHER, 0, (5, 5));
        let s = shoot(&sim, a).unwrap().clone();
        assert_eq!(s.shoot_position, "schussposition01");
        assert_eq!(
            (s.shoot_delay, s.can_shoot, s.bullets_count),
            (6000, true, 0)
        );
        assert_eq!(s.models, vec![Some("pfeil".to_string())]);
        assert_eq!(
            send(&mut sim, a, &Message::new(0x100A, &[])),
            0,
            "no cannon"
        );
        assert_eq!(send(&mut sim, a, &Message::new(0x30A, &[])), -1);
        assert!(!crate::module::is_busy(&sim, a, INDEX));
        shoot_mut(&mut sim, a).unwrap().is_busy = true;
        assert!(crate::module::is_busy(&sim, a, INDEX));
        assert_eq!(send(&mut sim, a, &Message::new(0x120A, &[])), 0);
        assert!(!crate::module::is_busy(&sim, a, INDEX));
        // Dead: only 0xF0A.
        sim.object_mut(a).unwrap().status |= crate::object::status::DIED;
        assert_eq!(send(&mut sim, a, &Message::new(0x100A, &[])), -1);
    }

    #[test]
    fn main_object_names_resolve_against_the_shooters_meshes() {
        let names: std::collections::BTreeSet<String> =
            ["pfeil", "box01"].map(String::from).into_iter().collect();
        // Before the last name an unresolved (0) one also goes into the list, the last only resolved.
        assert_eq!(
            resolve_models("pfeil", Some(&names)),
            vec![Some("pfeil".to_string())]
        );
        assert_eq!(
            resolve_models("Pfeil", Some(&names)),
            vec![Some("pfeil".to_string())]
        );
        assert_eq!(
            resolve_models("speer box01", Some(&names)),
            vec![None, Some("box01".to_string())]
        );
        assert_eq!(
            resolve_models("box01 speer", Some(&names)),
            vec![Some("box01".to_string())]
        );
        assert!(resolve_models("0", Some(&names)).is_empty());
        assert!(resolve_models("pfeil", None).is_empty());
    }

    /// ENG infantryman (slot 0) against warrior (slot 1, does not defend himself) 4 tiles away.
    fn rifle_duel(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
    ) -> (ObjectId, ObjectId) {
        let mut sim = Sim::new(state, rules);
        let a = spawn(&mut sim, ENG_INFANTRY, 0, (5, 5));
        let t = spawn(&mut sim, WARRIOR, 1, (9, 5));
        sim.object_mut(a).unwrap().direction = crate::fx::Fx(0);
        send(
            &mut sim,
            t,
            &crate::module::quarter::attack_mode_msg(0, false),
        );
        crate::messaging::post_message(&mut sim, a, Message::new(0x111, &[t.as_i32(), 0]), true);
        (a, t)
    }

    // QUIRK C-92: kept
    #[test]
    fn a_rifle_presents_once_fires_59_at_s_plus_16_and_reloads_until_s_plus_50() {
        // re/combat-ranged 10: GUNSTART 666 -> S+7, GUNSHOOTING 866 -> S+16 (59 = 24*4931/2000),
        // GUNDELAY S+28, GUNRELOADED S+50; afterwards without presenting 24*3265/2000 = 39 in S+59 (C-92).
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = rifle_duel(&mut state, &rules);
        let mut s = None;
        let mut hits = Vec::new();
        let mut sounds = Vec::new();
        let mut last = 160;
        for _ in 0..2 * 70 {
            let events = crate::tick::step(&mut state, &rules, &[]);
            let sim = Sim::new(&mut state, &rules);
            if s.is_none() && crate::module::attack::attack(&sim, a).is_some_and(|x| x.status == 2)
            {
                s = Some(sim.state.tick);
            }
            for e in events {
                if let SimEvent::Sound3d {
                    action: 1,
                    flags: 0x23,
                    ..
                } = e
                {
                    sounds.push(sim.state.tick);
                }
            }
            let hp = sim.object(t).unwrap().attributes.current_value(14, 0);
            if hp != last {
                hits.push((sim.state.tick, last - hp));
                last = hp;
            }
        }
        let s = s.expect("presents");
        let rel: Vec<_> = hits.iter().map(|&(k, d)| (k - s, d)).collect();
        assert_eq!(rel, vec![(16, 59), (59, 39)]);
        assert_eq!(
            sounds.iter().map(|k| k - s).collect::<Vec<_>>(),
            vec![16, 59]
        );
        let sim = Sim::new(&mut state, &rules);
        // The second shot (S+59) empties the magazine (1 >= 1): gun off, GUNRELOADED not until after the window.
        assert!(
            !shoot(&sim, a).unwrap().is_gun_present,
            "lowered after the second shot"
        );
    }

    // QUIRK C-91, C-93: kept
    #[test]
    fn a_gun_hits_at_once_without_range_or_line_of_sight_checks() {
        // C-93: the hit falls at once at marker 3 (S+16), even if a house stands in between after the
        // target choice. The house arrives in the step of presenting (target chosen, gun presented),
        // demonstrably before the first hit.
        let rules = rules();
        let mut state = state(&rules);
        let (a, t) = rifle_duel(&mut state, &rules);
        let mut placed = false;
        for _ in 0..2 * 40 {
            crate::tick::step(&mut state, &rules, &[]);
            let mut sim = Sim::new(&mut state, &rules);
            if shoot(&sim, a).unwrap().is_gun_present {
                assert_eq!(
                    sim.object(t).unwrap().attributes.current_value(14, 0),
                    160,
                    "no hit yet at the house construction"
                );
                spawn(&mut sim, crate::command::testkit::HOUSE, 2, (7, 5));
                placed = true;
                break;
            }
        }
        assert!(placed, "the gun presents");
        for _ in 0..2 * 20 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            sim.object(t).unwrap().attributes.current_value(14, 0),
            160 - 59,
            "hit despite a house in the line of fire"
        );
    }

    // QUIRK C-90: kept
    #[test]
    fn a_gun_without_bullets_only_plays_the_reload_animation() {
        // C-90: empty magazine and not presented -> only the reload animation, no timer.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ENG_INFANTRY, 0, (5, 5));
        let t = spawn(&mut sim, WARRIOR, 1, (9, 5));
        shoot_mut(&mut sim, a).unwrap().bullets_cur = 1;
        send(&mut sim, a, &Message::new(0x10A, &[t.as_i32(), 0, -1]));
        assert_eq!(sim.object(a).unwrap().anim.wanted, 8);
        let m = sim.module(a, INDEX).unwrap();
        assert!(m.base.timers_ws.is_empty() && m.base.events.is_empty());
        assert!(!shoot(&sim, a).unwrap().is_gun_present);
        assert_eq!(sim.object(t).unwrap().attributes.current_value(14, 0), 160);
    }

    // QUIRK C-115: kept
    #[test]
    fn gun_hits_on_buildings_spark_and_on_units_bleed() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, ENG_INFANTRY, 0, (5, 5));
        let w = spawn(&mut sim, WARRIOR, 1, (9, 5));
        let hq = spawn(&mut sim, HQ, 1, (5, 10));
        for (t, spark) in [(w, false), (hq, true)] {
            if let Some(s) = shoot_mut(&mut sim, a) {
                s.target = t;
                s.hitpoints = 1;
            }
            sim.state.events.clear();
            send(&mut sim, a, &Message::new(0xC0A, &[]));
            let ev = &sim.state.events;
            assert!(ev.iter().any(|e| matches!(e, SimEvent::EffectAttached { object, node, .. } if *object == a && node == "gun01")));
            assert_eq!(
                ev.iter().any(
                    |e| matches!(e, SimEvent::EffectAt { path, .. } if path.ends_with("funken.pef"))
                ),
                spark
            );
            assert_eq!(
                ev.iter()
                    .any(|e| matches!(e, SimEvent::Sound3d { action: 2, .. })),
                spark
            );
            assert_eq!(
                ev.iter().any(|e| matches!(e, SimEvent::EffectAttached { path, .. } if path.ends_with("blood.pef"))),
                !spark
            );
        }
    }

    /// Cannon (slot 0) at (5, 5) facing +x against `target` (slot 1, stance 0).
    fn cannon_at(sim: &mut Sim, guid: u32) -> ObjectId {
        let a = spawn(sim, guid, 0, (5, 5));
        sim.object_mut(a).unwrap().direction = crate::fx::Fx(0);
        a
    }

    fn order(sim: &mut Sim, a: ObjectId, t: ObjectId) {
        crate::messaging::post_message(sim, a, Message::new(0x111, &[t.as_i32(), 0]), true);
    }

    /// Tick of aiming and (tick, event) up to `ticks` ticks.
    fn watch(
        state: &mut crate::state::SimState,
        rules: &crate::rules::Rules,
        a: ObjectId,
        ticks: usize,
    ) -> (u32, Vec<(u32, SimEvent)>) {
        let mut s = None;
        let mut out = Vec::new();
        for _ in 0..2 * ticks {
            let events = crate::tick::step(state, rules, &[]);
            let k = state.tick;
            let sim = Sim::new(state, rules);
            if s.is_none() && crate::module::attack::attack(&sim, a).is_some_and(|x| x.status == 2)
            {
                s = Some(k);
            }
            out.extend(events.into_iter().map(|e| (k, e)));
        }
        (s.expect("aims"), out)
    }

    #[test]
    fn a_cannon_fires_at_marker_3_and_reloads_for_6_s() {
        // re/combat-ranged 10: ball S+8 with 70 per ball, CANNONANIM S+54, CANNONDELAY S+69 ->
        // next shot S+77. 6 tiles = 768 WU -> flight time 768, hit after 8 effect ticks (S+15).
        let rules = rules();
        let mut state = state(&rules);
        let (a, group) = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = cannon_at(&mut sim, SPA_CANNON);
            let x = (11 << 12) + 2048;
            let mk = |sim: &mut Sim, dx: i32| {
                let mut d = crate::world::ObjectDesc::new(
                    WARRIOR,
                    1,
                    [
                        (x + dx) as f32 / 32.0,
                        0.0,
                        ((5 << 12) + 2048) as f32 / 32.0,
                    ],
                );
                d.flags |= crate::world::desc_flags::PLACED;
                let u = crate::world::create_object(sim, &mut d).unwrap();
                send(sim, u, &crate::module::quarter::attack_mode_msg(0, false));
                u
            };
            let group = [mk(&mut sim, 0), mk(&mut sim, 2048)];
            order(&mut sim, a, group[0]);
            (a, group)
        };
        // Second start S+77, impact S+84: window 90 ticks.
        let (s, events) = watch(&mut state, &rules, a, 90);
        let launches: Vec<(u32, i32)> = events
            .iter()
            .filter_map(|(k, e)| match e {
                SimEvent::ProjectileLaunched { flight_ms, .. } => Some((k - s, *flight_ms)),
                _ => None,
            })
            .collect();
        assert_eq!(launches, vec![(8, 768), (77, 768)]);
        assert!(events.iter().any(|(k, e)| matches!(
            e,
            SimEvent::Sound3d {
                action: 1,
                flags: 0x22,
                ..
            }
        ) && *k == s + 8));
        let ended: Vec<u32> = events
            .iter()
            .filter(|(_, e)| matches!(e, SimEvent::ProjectileEnded { .. }))
            .map(|(k, _)| k - s)
            .collect();
        assert_eq!(ended[0], 15);
        let sim = Sim::new(&mut state, &rules);
        let hp = |u| sim.object(u).unwrap().attributes.current_value(14, 0);
        // Two balls (the warriors stand still): -70, -70 -> fraction 12,500 -> 20; -52, -52 -> 35,000 -> 56.
        assert_eq!(hp(group[0]), 20);
        assert_eq!(hp(group[1]), 56);
    }

    // QUIRK C-91, C-123: kept
    #[test]
    fn a_cannon_against_a_building_flies_to_the_chosen_hit_zone() {
        // Zones +/-100 in x/z at the HQ (6 tiles away, rotation 0): from the cannon (-100, 0, 0) is
        // nearest (668 WU), then (0, 0, +/-100) (774); h = 2, counter 0 -> zone 1; flight time 668.
        let mut rules = rules();
        rules.hit_zones.insert(
            HQ,
            vec![
                [100.0, 0.0, 0.0],
                [-100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0],
                [0.0, 0.0, -100.0],
            ],
        );
        let mut state = state(&rules);
        let (a, hq) = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = cannon_at(&mut sim, SPA_CANNON);
            let hq = spawn(&mut sim, HQ, 1, (11, 5));
            sim.object_mut(hq).unwrap().rotation[1] = 0.0;
            order(&mut sim, a, hq);
            (a, hq)
        };
        let before = Sim::new(&mut state, &rules)
            .object(hq)
            .unwrap()
            .attributes
            .raw_fraction(14);
        let (s, events) = watch(&mut state, &rules, a, 20);
        let first = events.iter().find_map(|(k, e)| match e {
            SimEvent::ProjectileLaunched {
                flight_ms,
                target_pos,
                ..
            } => Some((k - s, *flight_ms, *target_pos)),
            _ => None,
        });
        let hqw = (11.0 * 128.0 + 64.0, 5.0 * 128.0 + 64.0);
        assert_eq!(first, Some((8, 668, [hqw.0 - 100.0, 0.0, hqw.1])));
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(sim.object(hq).unwrap().hit_zone_counter, 1);
        // Direct hit: 70 * 2.5 (CANNON_BUILDING) * 0.8 (no NOTHUMAN, C-123) = 140.
        let after = sim.object(hq).unwrap().attributes.raw_fraction(14);
        // 140 * 100,000 / 4000 (ball damage 140, building HP 4000).
        assert_eq!(before - after, 3500);
    }

    // QUIRK C-81: kept
    #[test]
    fn a_pat_cannon_without_marker_3_fires_after_100_ms() {
        // C-81: max(marker 3 = 0, 100) -> S+2.
        let rules = rules();
        let mut state = state(&rules);
        let a = {
            let mut sim = Sim::new(&mut state, &rules);
            let a = cannon_at(&mut sim, PAT_CANNON);
            let t = spawn(&mut sim, WARRIOR, 1, (11, 5));
            send(
                &mut sim,
                t,
                &crate::module::quarter::attack_mode_msg(0, false),
            );
            order(&mut sim, a, t);
            a
        };
        let (s, events) = watch(&mut state, &rules, a, 10);
        let k = events
            .iter()
            .find(|(_, e)| matches!(e, SimEvent::ProjectileLaunched { .. }))
            .map(|(k, _)| k - s);
        assert_eq!(k, Some(2));
    }

    // QUIRK C-86: kept
    #[test]
    fn an_aborted_cannon_still_fires_its_running_shot() {
        // C-86: abort clears no timers for the cannon.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = cannon_at(&mut sim, SPA_CANNON);
        let t = spawn(&mut sim, WARRIOR, 1, (11, 5));
        send(&mut sim, a, &Message::new(0x10A, &[t.as_i32(), 0, -1]));
        crate::module::abort(&mut sim, a, INDEX);
        sim.state.events.clear();
        for _ in 0..9 {
            crate::module::tick(&mut sim, a, INDEX, 100);
        }
        assert!(
            sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::ProjectileLaunched { .. }))
        );
        assert!(!shoot(&sim, a).unwrap().can_shoot);
    }

    /// Shoot positions of the tower substitute (HQ): one each on the four sides at height 500.
    const SHOOT_NODES: [[f32; 3]; 4] = [
        [0.0, 500.0, -60.0],
        [-60.0, 500.0, 0.0],
        [60.0, 500.0, 0.0],
        [0.0, 500.0, 60.0],
    ];

    fn rules_with_shoot_nodes() -> crate::rules::Rules {
        let mut r = rules();
        let shape = r.building_shapes.get_mut(&HQ).unwrap();
        for (i, v) in SHOOT_NODES.iter().enumerate() {
            shape
                .helper_nodes
                .insert(format!("schussposition{:02}", i + 1), *v);
        }
        r
    }

    /// HQ (slot 0) at (5, 5) with yaw `yaw`, the shooter `guid` quartered in it; target
    /// `WARRIOR` (slot 1, stance 0) at `tile`. Returns: shooter, HQ, target.
    fn quartered_shooter(
        sim: &mut Sim,
        guid: u32,
        yaw: f32,
        tile: (i32, i32),
    ) -> (ObjectId, ObjectId, ObjectId) {
        let hq = spawn(sim, HQ, 0, (5, 5));
        sim.object_mut(hq).unwrap().rotation[1] = yaw;
        let a = spawn(sim, guid, 0, (3, 5));
        crate::module::quarter::enter_container(sim, a, hq);
        let t = spawn(sim, WARRIOR, 1, tile);
        send(sim, t, &crate::module::quarter::attack_mode_msg(0, false));
        (a, hq, t)
    }

    fn offset(p: [f32; 3], d: [f32; 3]) -> [f32; 3] {
        [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
    }

    // QUIRK O-283: kept
    #[test]
    fn the_quartered_shoot_position_is_the_building_node_nearest_the_target() {
        // 0x00525500: distance node - (target - shooter), the smallest wins, rotated by the yaw of the
        // building, plus building position.
        let rules = rules_with_shoot_nodes();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, _) = quartered_shooter(&mut sim, ENG_INFANTRY, 0.0, (13, 5));
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let s = |sim: &Sim, d: [f32; 3]| quartered_shoot_pos(sim, a, offset(w, d));
        assert_eq!(s(&sim, [1000.0, 0.0, 0.0]), offset(w, SHOOT_NODES[2]));
        assert_eq!(s(&sim, [-1000.0, 0.0, 0.0]), offset(w, SHOOT_NODES[1]));
        assert_eq!(s(&sim, [0.0, 0.0, -1000.0]), offset(w, SHOOT_NODES[0]));
        assert_eq!(s(&sim, [0.0, 0.0, 1000.0]), offset(w, SHOOT_NODES[3]));
        // Tie (nodes 03 and 04 equally far, strictly smaller): the one found first.
        assert_eq!(s(&sim, [1000.0, 0.0, 1000.0]), offset(w, SHOOT_NODES[2]));
    }

    // QUIRK O-283: kept
    #[test]
    fn the_node_choice_ignores_the_building_yaw_but_the_node_turns_with_it() {
        // O-283: the choice is computed in model space with the unrotated target direction; the rotation is not
        // applied until the node has been chosen. Yaw pi/2, target in +x -> node 03 (60, 500, 0) -> (~0, 500, 60).
        let rules = rules_with_shoot_nodes();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, _) =
            quartered_shooter(&mut sim, ENG_INFANTRY, std::f32::consts::FRAC_PI_2, (13, 5));
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let s = quartered_shoot_pos(&sim, a, offset(w, [1000.0, 0.0, 0.0]));
        let d = [s[0] - w[0], s[1] - w[1], s[2] - w[2]];
        assert!(d[0].abs() < 1e-3, "{d:?}");
        assert_eq!(d[1], 500.0);
        assert!((d[2] - 60.0).abs() < 1e-3, "{d:?}");
    }

    #[test]
    fn without_nodes_or_container_the_shoot_position_falls_back() {
        // Without nodes: minimum distance 1e9 stays, node (0, 0, 0) -> building position. Without a container
        // (0x00466BB0 returns none): world position of the shooter.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, t) = quartered_shooter(&mut sim, ENG_INFANTRY, 0.0, (13, 5));
        let tp = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        assert_eq!(quartered_shoot_pos(&sim, a, tp), w);
        sim.object_mut(a).unwrap().carrier = ObjectId::NONE;
        sim.object_mut(a).unwrap().grid = sim.object(t).unwrap().grid;
        assert_eq!(quartered_shoot_pos(&sim, a, tp), tp);
    }

    #[test]
    fn a_quartered_gun_fires_from_the_building_node_with_the_flash_on_the_building() {
        // FireGun 0x00526930: start = shoot position, muzzle flash at the container (position start,
        // rotated toward the target), sound 1 at the start; no effect at the node of the shooter.
        let rules = rules_with_shoot_nodes();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, t) = quartered_shooter(&mut sim, ENG_INFANTRY, 0.0, (13, 5));
        if let Some(s) = shoot_mut(&mut sim, a) {
            s.target = t;
            s.hitpoints = 1;
        }
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let tp = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        let start = offset(w, SHOOT_NODES[2]);
        sim.state.events.clear();
        send(&mut sim, a, &Message::new(0xC0A, &[]));
        let ev = &sim.state.events;
        assert!(ev.contains(&SimEvent::EffectOwnedAt {
            object: hq,
            path: MUZZLE_FLASH.into(),
            pos: start,
            aim: tp,
        }));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SimEvent::EffectAttached { object, .. } if *object == a))
        );
        assert!(ev.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d { action: 1, pos, .. } if *pos == start
        )));
        assert!(sim.object(t).unwrap().attributes.current_value(14, 0) < 160);
    }

    #[test]
    fn a_quartered_archer_looses_from_the_building_node() {
        // LaunchArrow 0x00525F90: start = shoot position toward the hit zone; no node for the host.
        let rules = rules_with_shoot_nodes();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, t) = quartered_shooter(&mut sim, ARCHER, 0.0, (13, 5));
        if let Some(s) = shoot_mut(&mut sim, a) {
            s.target = t;
            s.hitpoints = 1;
        }
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let start = offset(w, SHOOT_NODES[2]);
        sim.state.events.clear();
        send(&mut sim, a, &Message::new(0x40A, &[]));
        let ev = &sim.state.events;
        assert!(ev.iter().any(|e| matches!(
            e,
            SimEvent::ProjectileLaunched { start: s, node, .. } if *s == start && node.is_empty()
        )));
        assert!(ev.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d { action: 5, pos, .. } if *pos == start
        )));
        assert_eq!(sim.state.projectiles.list[0].start, start);
    }

    #[test]
    fn a_quartered_cannon_fires_from_the_building_node_with_the_smoke_on_the_building() {
        // FireCannon 0x005264C0: start = shoot position toward the target position, `kanone_rauch.pef` at the
        // container, sound 1 and ball from the start, no node for the host.
        let rules = rules_with_shoot_nodes();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a, hq, t) = quartered_shooter(&mut sim, SPA_CANNON, 0.0, (13, 5));
        if let Some(s) = shoot_mut(&mut sim, a) {
            s.target = t;
        }
        let w = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let tp = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        let start = offset(w, SHOOT_NODES[2]);
        sim.state.events.clear();
        send(&mut sim, a, &Message::new(0x130A, &[]));
        let ev = &sim.state.events;
        assert!(ev.contains(&SimEvent::EffectOwnedAt {
            object: hq,
            path: "data/effects/kanone_rauch.pef".into(),
            pos: start,
            aim: tp,
        }));
        assert!(ev.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d { action: 1, flags: 0x22, pos, .. } if *pos == start
        )));
        assert!(ev.iter().any(|e| matches!(
            e,
            SimEvent::ProjectileLaunched { start: s, node, .. } if *s == start && node.is_empty()
        )));
    }
}
