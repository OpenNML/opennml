//! Projectiles (re/combat-projectiles): arrow (`ArrowProjectile`) and cannonball
//! (`CannonBall`) as simulation state - remaining flight time, shooter, target, damage snapshot,
//! splash center. The host computes trajectory, trail and smoke.
//!
//! The world effects dynamite, explosion and storm are appended to the same list (same tick order,
//! re/spells 3, re/special-attacks 1.5). The headhunter's dynamite throw
//! ([`ProjectileKind::Throw`], [`launch_throw`] and the impact) belongs here as well.

use serde::{Deserialize, Serialize};

use nml_formats::targets::MatchFilter;

use crate::collision::{
    edge_distance_to_object, edge_distance_to_point, find_nearest_matching, query_radius,
};
use crate::combat::hit_zone::{attach_blood, vector3_length, world_pos};
use crate::fx::{Fx, FxVec3, fixed_div};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::building::destroy::IMPACT_GROUND;
use crate::sim::Sim;
use crate::state::SimEvent;

/// `ParticleID` 1 (arrow) or 3 (ball) of the original savegame; the world effects are appended at
/// the end (the savegame stores the variant index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectileKind {
    Arrow,
    CannonBall,
    /// Dynamite charge of the demolition expert (`ParticleID` 4, `DynamiteEffect` `0x004F04D0`).
    Dynamite,
    /// World effect explosion of spell kind 1 (VTable `0x00620D50`, re/spells 1.3).
    Explosion,
    /// World effect storm of spell kind 2 (VTable `0x00620DC8`, re/spells 1.4).
    Storm,
    /// Dynamite throw of the headhunter (`ArrowProjectile` with flag `0x200`,
    /// re/headhunter-robbing 3.5, re/combat-projectiles 2.2): `flags` `0x201`, thrower in `shooter`, no
    /// target, `damage` = impact message `{0xD22 | 0xE22, [0]}` to the thrower, `start` world position
    /// of the headhunter, `target_pos` hit zone 0 of the target at the throw, `zone`/`aim` 0.
    Throw,
}

/// A flying projectile in tick and damage order (`EffectManager`, AddTail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projectile {
    /// ID for the host (start/end of a trajectory), consecutive per match.
    pub id: u32,
    pub kind: ProjectileKind,
    /// `m_Info`: arrow `0x21` (`| 0x40` fire arrow), ball `0x1C`, dynamite 4 (`ParticleID`),
    /// explosion and storm the slot of the caster, throw `0x201`.
    pub flags: u32,
    /// `m_Time`: remaining flight time in ms.
    pub remaining_ms: i32,
    /// `m_TotalTime`: flight time at launch (host).
    pub flight_ms: i32,
    /// Shooter (`DamageMsg+8`).
    pub shooter: ObjectId,
    /// Target (`+0x88` or `+0x80`).
    pub target: ObjectId,
    /// Damage message `0x17F` (12 arguments) as a snapshot of the shooter at launch;
    /// the position (`args[6..9]`) is not set until the hit. Throw: impact message to the thrower.
    pub damage: Message,
    /// Ball: splash center = target position at launch (`+0xF4`) in fixed point; arrow and world effects:
    /// unused (0).
    pub aim: FxVec3,
    /// Target point of the trajectory (world): arrow the hit zone, ball the impact `+0x98`.
    pub target_pos: [f32; 3],
    /// Start of the trajectory (world): world position of the shooter; the host computes the muzzle.
    pub start: [f32; 3],
    /// Hit zone (`+0x8C`, presentation only); dynamite: charge animation (370004/370006, host);
    /// storm: pulse counter (start 1); explosion and throw: 0.
    pub zone: i32,
}

/// Projectile list (`GameWorld+0x3E0` -> `m_ParticleList`) and the next ID.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Projectiles {
    pub next_id: u32,
    pub list: Vec<Projectile>,
}

/// Arrow speed (`Arrow+0x94` = `0x3F333333`) in world units per ms.
pub const ARROW_SPEED: f32 = 0.7;
/// Ball speed (`Ball+0x88` = 1.0).
pub const BALL_SPEED: f32 = 1.0;
/// Splash radius `Fixed(3) / 2` = 1.5 tiles.
pub const SPLASH_RADIUS: i32 = 6144;
/// Fallback target: search 2 tiles around the shooter, edge to edge below `Ball+0x84` = 1 tile.
const FALLBACK_SEARCH: i32 = 2 * 4096;
const FALLBACK_EDGE: i32 = 4096;
/// Type GUID of the sound of an orphaned impact (`0x455` = warship, C-112).
const ORPHAN_GUID: u32 = 1109;

/// Dynamite charge: ParticleID 4 as the flag word of the entry (`DynamiteEffect_Construct` `0x004F04D0`,
/// re/special-attacks 1.5).
const DYNAMITE_FLAGS: u32 = 4;
/// Dynamite throw: arrow ID 1 with impact message `0x200` (re/headhunter-robbing 3.5).
pub const THROW_FLAGS: u32 = 0x201;
/// Explosion: delay and percent of max HP (`0x004F0D50`, `0x004F1020`, re/spells 1.5).
pub const EXPLOSION_MS: i32 = 500;
pub const EXPLOSION_PERCENT: i32 = -75;
/// Storm: duration and percent per pulse (`0x004F42C0`, `0x004F43B0`).
pub const STORM_MS: i32 = 7000;
pub const STORM_PERCENT: i32 = -2;
/// Radius of explosion and storm: `Fixed_FromInt(5)` or `5 · DAT_006722EC` = 5 tiles,
/// center distance.
pub const WORLD_EFFECT_RADIUS: i32 = 5 * 4096;

const EXPLOSION_MEDICINE_MAN: &str = "data/effects/explosion_medicineman.pef";
const EXPLOSION_DYNAMITE: &str = "data/effects/explosion_dynamite.pef";
/// Blood at the target (arrow and rifle hits, sniping; `0x00620F44`). The host chooses the blood
/// switch `0x006732BC` and thereby `no_blood.pef` (cheat `bloodmode`, [`SimEvent::BloodToggle`], O-392).
pub const BLOOD: &str = "data/effects/blood.pef";
const SPARKS: &str = "data/effects/funken.pef";
/// Model of the dynamite throw (`0x00624650`, re/headhunter-robbing 1.4); helper node on the headhunter
/// `0x00624678`.
pub const THROW_MODEL: &str = "data/r3d/misc/Tools/dynamit/dynamit.rdo";
const THROW_NODE: &str = "dynamitposition";
const IMPACT_WATER: &str = "data/effects/einschlag_wasser.pef";

/// Flight time (`ArrowProjectile_Construct` `0x004EE481`, `Projectile_Construct` `0x004EFF22`,
/// re/combat-projectiles 8): `floor(|from − to| / v)` in the `f32` sequence of `Vector3_Length`.
pub fn flight_time_ms(from: [f32; 3], to: [f32; 3], speed: f32) -> i32 {
    let len = vector3_length([from[0] - to[0], from[1] - to[1], from[2] - to[2]]);
    (len / speed).floor() as i32
}

/// Launch of an arrow (`LaunchArrow` `0x00525F90` -> `ArrowProjectile_Construct`).
pub struct ArrowLaunch {
    pub shooter: ObjectId,
    pub target: ObjectId,
    /// `DamageMsg_InitFromAttacker` with amount `−m_Hitpoints`.
    pub damage: Message,
    pub start: [f32; 3],
    /// Hit zone of the target (`GameObject_GetHitZoneWorldPos`).
    pub target_pos: [f32; 3],
    pub zone: i32,
    /// Fire arrow (`FIREARROW` on the shoot module).
    pub fire: bool,
    /// First resolved mesh from 110007.
    pub model: String,
    /// Helper node of the muzzle for the host: `schussposition01`; empty for the quartered shooter
    /// (`start` is already the shot position of the building, re/combat-ranged 3.2).
    pub node: String,
}

/// Launch of a cannonball (`FireCannon` `0x005264C0` -> `Projectile_Construct`).
pub struct BallLaunch {
    pub shooter: ObjectId,
    pub target: ObjectId,
    /// Amount `−attribute 20`.
    pub damage: Message,
    pub start: [f32; 3],
    /// Impact `+0x98`: hit zone, lowered to ground/water for walk targets.
    pub target_pos: [f32; 3],
    /// Helper node of the muzzle for the host: cannon `Muendung01`, broadside `steuercannonNN` or
    /// `backcannonNN` (re/ships 3.6).
    pub node: String,
    /// Additional yaw of the muzzle smoke (`Projectile` `+0xB8`): cannon 0, broadside `+π/2`
    /// (port) or `−π/2` (starboard).
    pub yaw_extra: f32,
}

fn next_id(sim: &mut Sim) -> u32 {
    let id = sim.state.projectiles.next_id;
    sim.state.projectiles.next_id = id.wrapping_add(1);
    id
}

/// `ArrowProjectile_Construct` (`0x004EE2E0`) + `GameWorld_AddEffect(e, 0)` (append at the end):
/// flight time between the **object positions** of shooter and target (if one is missing: start or
/// target point).
pub fn launch_arrow(sim: &mut Sim, l: ArrowLaunch) -> u32 {
    let s = world_pos(sim, l.shooter).unwrap_or(l.start);
    let z = world_pos(sim, l.target).unwrap_or(l.target_pos);
    let t = flight_time_ms(s, z, ARROW_SPEED);
    let id = next_id(sim);
    let shooter_guid = sim.object(l.shooter).map_or(0, |o| o.guid);
    sim.state.projectiles.list.push(Projectile {
        id,
        kind: ProjectileKind::Arrow,
        flags: 0x21 | if l.fire { 0x40 } else { 0 },
        remaining_ms: t,
        flight_ms: t,
        shooter: l.shooter,
        target: l.target,
        damage: l.damage,
        aim: FxVec3::default(),
        target_pos: l.target_pos,
        start: l.start,
        zone: l.zone,
    });
    sim.state.events.push(SimEvent::ProjectileLaunched {
        id,
        kind: ProjectileKind::Arrow,
        fire: l.fire,
        shooter: l.shooter,
        shooter_guid,
        model: l.model,
        node: l.node,
        target: l.target,
        zone: l.zone,
        start: l.start,
        target_pos: l.target_pos,
        flight_ms: t,
        yaw_extra: 0.0,
    });
    id
}

/// `Projectile_Construct` (`0x004EFDE0`): target = object position of a unit, otherwise (building, pattern,
/// tower, target gone) the target point; this position is also the splash center (`+0xF4`).
pub fn launch_ball(sim: &mut Sim, l: BallLaunch) -> u32 {
    let s = world_pos(sim, l.shooter).unwrap_or(l.start);
    let structure = sim.object(l.target).is_none_or(|o| {
        o.has_module(3, false) || o.has_module(0x29, false) || o.has_module(0x32, false)
    });
    let z = if structure {
        l.target_pos
    } else {
        world_pos(sim, l.target).unwrap_or(l.target_pos)
    };
    let t = flight_time_ms(s, z, BALL_SPEED);
    let cell = sim.rules.map.cell;
    // `GridPos_FromWorldFloat`: exact for object positions.
    let aim = FxVec3::new(
        Fx(Fx::from_f32(z[0]).0 / cell[0]),
        Fx(Fx::from_f32(z[1]).0 / cell[1]),
        Fx(Fx::from_f32(z[2]).0 / cell[2]),
    );
    let id = next_id(sim);
    let shooter_guid = sim.object(l.shooter).map_or(0, |o| o.guid);
    sim.state.projectiles.list.push(Projectile {
        id,
        kind: ProjectileKind::CannonBall,
        flags: 0x1C,
        remaining_ms: t,
        flight_ms: t,
        shooter: l.shooter,
        target: l.target,
        damage: l.damage,
        aim,
        target_pos: l.target_pos,
        start: l.start,
        zone: -1,
    });
    sim.state.events.push(SimEvent::ProjectileLaunched {
        id,
        kind: ProjectileKind::CannonBall,
        fire: false,
        shooter: l.shooter,
        shooter_guid,
        model: String::new(),
        node: l.node,
        target: l.target,
        zone: -1,
        start: l.start,
        target_pos: l.target_pos,
        flight_ms: t,
        yaw_extra: l.yaw_extra,
    });
    id
}

/// Dynamite throw of the headhunter (`0x00518C60`/`0x005188C0` -> `ArrowProjectile_Construct` with
/// flags `0x201`, re/headhunter-robbing 3.5): entry with the field assignment - start = world position
/// of the headhunter, target point `target_pos` (hit zone 0 of the target at the throw), no target (no
/// pursuit), flight time `floor(|start − target point| / 0.7)` as with the arrow
/// (re/combat-projectiles 1.3), impact message `{impact, [0]}` (`0xD22`/`0xE22`) to the thrower.
/// Appended at the end of the list; event for the host (model `dynamit.rdo` from `dynamitposition`).
pub fn launch_throw(sim: &mut Sim, hunter: ObjectId, target_pos: [f32; 3], impact: u32) -> u32 {
    // The thrower calls this itself, its position always exists.
    let start = world_pos(sim, hunter).unwrap_or(target_pos);
    let t = flight_time_ms(start, target_pos, ARROW_SPEED);
    let id = next_id(sim);
    let shooter_guid = sim.object(hunter).map_or(0, |o| o.guid);
    sim.state.projectiles.list.push(Projectile {
        id,
        kind: ProjectileKind::Throw,
        flags: THROW_FLAGS,
        remaining_ms: t,
        flight_ms: t,
        shooter: hunter,
        target: ObjectId::NONE,
        damage: Message::new(impact, &[0]),
        aim: FxVec3::default(),
        target_pos,
        start,
        zone: 0,
    });
    sim.state.events.push(SimEvent::ProjectileLaunched {
        id,
        kind: ProjectileKind::Throw,
        fire: false,
        shooter: hunter,
        shooter_guid,
        model: THROW_MODEL.into(),
        node: THROW_NODE.into(),
        target: ObjectId::NONE,
        zone: 0,
        start,
        target_pos,
        flight_ms: t,
        yaw_extra: 0.0,
    });
    id
}

/// Explosion (`0x004F0D50`, re/spells 1.3): `+0x0C` remainder 500 ms, `+0x1C` slot from the ID of the
/// caster, `+0x20` caster; presentation effect `explosion_medicineman.pef` at the position (height
/// as passed, D-237). Appended at the end of the projectile list (`GameWorld_AddEffect`, AddTail).
pub fn add_explosion(sim: &mut Sim, caster: ObjectId, pos: [f32; 3]) {
    push_world_effect(
        sim,
        ProjectileKind::Explosion,
        caster,
        EXPLOSION_MS,
        Message::new(0x27F, &[EXPLOSION_PERCENT, caster.as_i32()]),
        pos,
        0,
    );
    effect_at(sim, EXPLOSION_MEDICINE_MAN, pos, 1.0);
}

/// Storm (`0x004F42C0`, re/spells 1.4): remainder 7000 ms, pulse counter 1, slot and ID of the caster;
/// radius 5 tiles as a constant. No event: the host derives the presentation effect
/// `storm_medicineman.pef` from the list.
pub fn add_storm(sim: &mut Sim, caster: ObjectId, pos: [f32; 3]) {
    push_world_effect(
        sim,
        ProjectileKind::Storm,
        caster,
        STORM_MS,
        Message::new(0x27F, &[STORM_PERCENT, caster.as_i32()]),
        pos,
        1,
    );
}

/// Dynamite charge (`DynamiteEffect_Construct` `0x004F04D0`, re/special-attacks 1.5): target (or
/// `NONE`), fuse time 10,000 ms, position, charge animation (host) and the damage message with
/// ID and GUID of the demolition expert at the time of laying (`m_OwnerHID`, `m_OwnerGUID`). No event:
/// the host derives model and animation from the list.
pub fn add_dynamite(sim: &mut Sim, bomber: ObjectId, target: ObjectId, pos: [f32; 3], anim: i32) {
    use crate::module::special::bomberman::{DAMAGE, FUSE_MS};
    let guid = sim.object(bomber).map_or(0, |o| o.guid);
    let damage = crate::combat::damage::explicit_damage_message(DAMAGE, bomber, guid);
    let id = next_id(sim);
    sim.state.projectiles.list.push(Projectile {
        id,
        kind: ProjectileKind::Dynamite,
        flags: DYNAMITE_FLAGS,
        remaining_ms: FUSE_MS,
        flight_ms: FUSE_MS,
        shooter: bomber,
        target,
        damage,
        aim: FxVec3::default(),
        target_pos: pos,
        start: pos,
        zone: anim,
    });
}

/// Explosion and storm: entry with the field assignment, slot `flags` =
/// `(hid >> 11) & 0x1F` of the caster at creation (re/spells 4).
fn push_world_effect(
    sim: &mut Sim,
    kind: ProjectileKind,
    caster: ObjectId,
    time_ms: i32,
    damage: Message,
    pos: [f32; 3],
    zone: i32,
) {
    let id = next_id(sim);
    sim.state.projectiles.list.push(Projectile {
        id,
        kind,
        flags: u32::from(caster.slot()),
        remaining_ms: time_ms,
        flight_ms: time_ms,
        shooter: caster,
        target: ObjectId::NONE,
        damage,
        aim: FxVec3::default(),
        target_pos: pos,
        start: pos,
        zone,
    });
}

/// `EffectManager_Tick` (`0x004F2260`): only in the phase-0 step, after objects and the additional list;
/// per projectile remaining time -100, the arrow hits at `< 1`, the ball at `≤ 0` (C-102). Order =
/// creation order. Arrows falling/stuck (states 2/4) are handled only by the host (C-110).
///
/// World effects (re/spells 3.8, re/special-attacks 3.3) in the same loop: explosion
/// (`0x004F1020`) takes effect at remainder `≤ 0`, dynamite (`0x004F07A0`) at `< 1` - both are then
/// removed; the storm (`0x004F43B0`) does not end until remainder `< 0` (remainder 0 still runs) and pulses
/// every second effect tick. As in the original, a removed entry does not advance the index; a pulse
/// reads its fields from the list before `query_radius`/`send` - `send` can only extend the list at the
/// end. Counting: an entry from object tick N (group 0 or 1) has its effect tick k in object tick
/// N+k-1 - explosion takes effect in N+4, storm pulses in N+1, N+3, ..., N+69 (gone in N+70), dynamite
/// in N+99. The headhunter's dynamite throw arrives like the arrow at remainder `≤ 0`
/// ([`throw_arrive`]).
pub fn tick(sim: &mut Sim) {
    let mut i = 0;
    while i < sim.state.projectiles.list.len() {
        let p = &mut sim.state.projectiles.list[i];
        p.remaining_ms = p.remaining_ms.wrapping_sub(100);
        if p.kind == ProjectileKind::Storm {
            if p.remaining_ms < 0 {
                sim.state.projectiles.list.remove(i);
                continue;
            }
            p.zone = p.zone.wrapping_sub(1);
            if p.zone >= 0 {
                i += 1;
                continue;
            }
            p.zone = 1;
            let (pos, slot, msg) = (p.target_pos, p.flags, p.damage.clone());
            i += 1;
            storm_pulse(sim, pos, slot, &msg);
            continue;
        }
        if p.remaining_ms > 0 {
            i += 1;
            continue;
        }
        let p = sim.state.projectiles.list.remove(i);
        match p.kind {
            ProjectileKind::Arrow => arrive(sim, p),
            ProjectileKind::CannonBall => impact(sim, p),
            ProjectileKind::Dynamite => detonate(sim, p),
            ProjectileKind::Explosion => explode(sim, p),
            // Handled above (stays in the list at remainder 0).
            ProjectileKind::Storm => {}
            ProjectileKind::Throw => throw_arrive(sim, p),
        }
    }
}

/// Enemy objects with module `prop_id` within 5 tiles (center distance) of `pos`, from the point of view
/// of slot `slot` (`MatchFilter` `0x414` = ENEMYLIST | ANDALL | CHECKPROPERTY, no fog). Center
/// `GridPos_FromWorldFloat` (`0x0044B2C0`).
fn world_effect_hits(sim: &mut Sim, pos: [f32; 3], slot: u32, prop_id: u32) -> Vec<ObjectId> {
    let g = crate::world::world_to_grid(pos, sim.rules.map.cell);
    let filter = MatchFilter {
        mode: 0x414,
        prop_id,
        ..MatchFilter::constructed()
    };
    query_radius(
        sim,
        (g.x.0, g.z.0),
        &filter,
        WORLD_EFFECT_RADIUS,
        slot as i32,
        false,
    )
}

/// `Explosion_Tick` (`0x004F1020`) at remainder `≤ 0` -> `0x004F0E30`: all enemy units
/// (walk module) <= 5 tiles around the remembered point (C-301): impact x0.5 at the object position and
/// `{0x27F, [−75, caster]}` (`d = trunc(Max · −75 / 100)`; experience and retaliation only with an
/// existing caster, messaging `add_hitpoints_percent`).
fn explode(sim: &mut Sim, p: Projectile) {
    for o in world_effect_hits(sim, p.target_pos, p.flags, 0) {
        if let Some(pos) = world_pos(sim, o) {
            effect_at(sim, IMPACT_GROUND, pos, 0.5);
        }
        send(sim, o, &p.damage);
    }
}

/// Pulse of the storm (`0x004F43B0`, `0x004F449A`): `{0x27F, [−2, caster]}` to all enemy
/// buildings (module 3) <= 5 tiles (C-302); no effect per hit.
fn storm_pulse(sim: &mut Sim, pos: [f32; 3], slot: u32, msg: &Message) {
    for o in world_effect_hits(sim, pos, slot, 3) {
        send(sim, o, msg);
    }
}

/// `DynamiteEffect_Tick` (`0x004F07A0`) at remainder `< 1` -> `0x004F06A0`: explosion effect at the
/// charge; if the target exists (even dead), `0x17F` with -4000 and the ID of the demolition expert
/// (if he is missing, without the attacker calculation, C-329). Only that one target (C-330).
fn detonate(sim: &mut Sim, p: Projectile) {
    effect_at(sim, EXPLOSION_DYNAMITE, p.target_pos, 1.0);
    if sim.object(p.target).is_some() {
        send(sim, p.target, &p.damage);
    }
}

/// `ArrowProjectile_OnArrive` (`0x004ED990`) with flag `0x200` (re/combat-projectiles 2.2,
/// re/headhunter-robbing 3.5): thrower present (`GetObjectById(…, false)`, even dead) -> the
/// impact message `0xD22`/`0xE22` synchronously to him; otherwise nothing. No damage from the
/// projectile itself.
fn throw_arrive(sim: &mut Sim, p: Projectile) {
    let present = sim.object(p.shooter).is_some();
    sim.state.events.push(SimEvent::ProjectileEnded {
        id: p.id,
        hit: present,
        pos: p.target_pos,
    });
    if present {
        send(sim, p.shooter, &p.damage);
    }
}

/// Hit position into the damage message (`Fixed_FromFloat(·)/128`, `args[6..9]` = x, z, y).
fn with_pos(mut m: Message, pos: [f32; 3]) -> Message {
    let f = |v: f32| Fx::from_f32(v).0 / 128;
    m.set_arg(6, f(pos[0]));
    m.set_arg(7, f(pos[2]));
    m.set_arg(8, f(pos[1]));
    m
}

fn effect_at(sim: &mut Sim, path: &str, pos: [f32; 3], scale: f32) {
    sim.state.events.push(SimEvent::EffectAt {
        path: path.into(),
        pos,
        yaw: 0.0,
        scale,
        start_ms: 0,
    });
}

fn sound(sim: &mut Sim, object: ObjectId, action: i32, pos: [f32; 3], guid: u32) {
    sim.state.events.push(SimEvent::Sound3d {
        object,
        action,
        flags: 0x42,
        pos,
        guid,
    });
}

/// `ArrowProjectile_OnArrive` (`0x004ED990`) and `ApplyDamage` (`0x004ED7D0`): shooter and target
/// present (even dying, re/combat-projectiles 15.6) -> effect (sparks at a building/large vehicle,
/// otherwise blood at the target at the zone in the direction of the shooter via [`attach_blood`]; the
/// counter turns, C-91) and `0x17F` synchronously; otherwise the arrow falls without damage (C-100,
/// C-101). Position = target point at launch (deviation from the original, C-108).
fn arrive(sim: &mut Sim, p: Projectile) {
    let pos = p.target_pos;
    let present = sim.object(p.shooter).is_some() && sim.object(p.target).is_some();
    sim.state.events.push(SimEvent::ProjectileEnded {
        id: p.id,
        hit: present,
        pos,
    });
    if !present {
        return;
    }
    let hard = sim
        .object(p.target)
        .is_some_and(|o| !o.has_module(0, false))
        || crate::filter::matches_class(sim, p.target, 0x2_0000);
    if hard {
        effect_at(sim, SPARKS, pos, 1.0);
    } else {
        let from = world_pos(sim, p.shooter).unwrap_or(p.start);
        attach_blood(sim, p.target, from);
    }
    let msg = with_pos(p.damage, pos);
    send(sim, p.target, &msg);
}

/// `Terrain_IsBelowWaterLevel` (`0x0042D730`): presentation only (effect/sound).
fn is_water(sim: &Sim, p: [f32; 3]) -> bool {
    crate::height::terrain_height(sim.rules, p[0], p[1], p[2]) < sim.rules.map.water_level
}

/// `CannonBall_Impact` (`0x004EF140`, re/combat-projectiles 3.4).
fn impact(sim: &mut Sim, p: Projectile) {
    let pos = p.target_pos;
    sim.state.events.push(SimEvent::ProjectileEnded {
        id: p.id,
        hit: true,
        pos,
    });
    let aim_world = crate::world::grid_to_world(p.aim, sim.rules.map.cell);
    let Some(shooter_guid) = sim.object(p.shooter).map(|o| o.guid) else {
        // Shooter gone: impact without damage, sound with type 1109 (C-112).
        if is_water(sim, aim_world) {
            effect_at(sim, IMPACT_WATER, pos, 2.0);
            sound(sim, ObjectId::NONE, 4, pos, ORPHAN_GUID);
        } else {
            effect_at(sim, IMPACT_GROUND, pos, 1.5);
            sound(sim, ObjectId::NONE, 3, pos, ORPHAN_GUID);
        }
        return;
    };
    let msg = with_pos(p.damage, pos);
    let hard = sim.object(p.target).is_some_and(|o| {
        o.has_module(3, false) || o.has_module(0x29, false) || o.has_module(0x32, false)
    }) || (sim.object(p.target).is_some()
        && crate::filter::matches_class(sim, p.target, 0x2_0000));
    if hard {
        effect_at(sim, SPARKS, pos, 1.0);
        send(sim, p.target, &msg);
        sound(sim, p.shooter, 2, pos, shooter_guid);
        return;
    }
    // Target is a unit or gone -> splash around the target position at launch (C-105).
    let c = (p.aim.x.0, p.aim.z.0);
    let player = i32::from(p.shooter.slot());
    let walkers = MatchFilter {
        mode: 0x214, // NOMANSLIST | ANDALL | CHECKPROPERTY
        prop_id: 0,
        ..MatchFilter::constructed()
    };
    let filter = MatchFilter {
        mode: 0x2000_0414, // ORNEXT | ENEMYLIST | ANDALL | CHECKPROPERTY
        prop_id: 0,
        next: Some(Box::new(walkers)),
        ..MatchFilter::constructed()
    };
    let hits = query_radius(sim, c, &filter, SPLASH_RADIUS, player, false);
    if hits.is_empty() {
        // Fallback target around the **shooter** (C-106): enemy building or neutral animal.
        let mut f2 = filter.clone();
        f2.prop_id = 3;
        let s = sim
            .object(p.shooter)
            .map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
        let Some(e) = find_nearest_matching(sim, s, &f2, FALLBACK_SEARCH, player, -1) else {
            return; // C-107: no effect, no sound
        };
        if edge_distance_to_object(sim, p.shooter, e) >= FALLBACK_EDGE {
            return;
        }
        effect_at(sim, IMPACT_GROUND, pos, 1.0);
        send(sim, e, &msg);
        sound(sim, p.shooter, 3, pos, shooter_guid);
        return;
    }
    if is_water(sim, aim_world) {
        effect_at(sim, IMPACT_WATER, pos, 2.0);
        sound(sim, p.shooter, 4, pos, shooter_guid);
    } else {
        effect_at(sim, IMPACT_GROUND, pos, 1.5);
        sound(sim, p.shooter, 3, pos, shooter_guid);
        // Crater `FootprintEffect_Construct(…, 0, 0x430)` at the same position (`0x004EF7C2`, D-283):
        // only here, drawn by the host.
        sim.state.events.push(SimEvent::Crater { pos });
    }
    let amount = msg.arg(0);
    for u in hits {
        let d = edge_distance_to_point(sim, u, c);
        if let Some(up) = world_pos(sim, u) {
            effect_at(sim, IMPACT_GROUND, up, 0.5);
        }
        let q = fixed_div(d.min(SPLASH_RADIUS), SPLASH_RADIUS);
        let mut m = msg.clone();
        m.set_arg(0, amount.wrapping_mul(4096 - q) / 4096);
        send(sim, u, &m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::*;
    use crate::sim::Sim;
    use crate::state::SimEvent;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// World position from a fixed-point grid (`p / 32`, exact).
    fn w(g: [i32; 3]) -> [f32; 3] {
        g.map(|v| v as f32 / 32.0)
    }

    /// Finished object at the fixed-point grid position `(x, z)` (height 0).
    fn at(sim: &mut Sim, guid: u32, slot: i32, x: i32, z: i32) -> ObjectId {
        let mut d = ObjectDesc::new(guid, slot, w([x, 0, z]));
        d.flags |= desc_flags::PLACED;
        let id = create_object(sim, &mut d).unwrap();
        let g = sim.object(id).unwrap().grid;
        assert_eq!((g.x.0, g.z.0), (x, z), "position unchanged");
        id
    }

    fn hp(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.current_value(14, 0)
    }

    const T: i32 = (10 << 12) + 2048;

    #[test]
    fn flight_times_follow_the_f32_sequence_and_floor() {
        // re/combat-projectiles 8 and 9: check vectors f32 2319 (f64 would be 2318) and 1384 respectively.
        assert_eq!(
            flight_time_ms([0.0; 3], w([25809, -2427, -45015]), ARROW_SPEED),
            2319
        );
        assert_eq!(
            flight_time_ms([0.0; 3], w([44193, -1167, 2654]), BALL_SPEED),
            1384
        );
        let tiles = |n: i32| [n as f32 * 128.0, 0.0, 0.0];
        assert_eq!(flight_time_ms([0.0; 3], tiles(1), ARROW_SPEED), 182);
        assert_eq!(flight_time_ms([0.0; 3], tiles(4), ARROW_SPEED), 731);
        assert_eq!(flight_time_ms([0.0; 3], tiles(7), ARROW_SPEED), 1280);
        assert_eq!(
            flight_time_ms([0.0; 3], [384.0, 0.0, 512.0], ARROW_SPEED),
            914
        );
        assert_eq!(
            flight_time_ms([0.0; 3], w([19000, 2048, 7777]), ARROW_SPEED),
            921
        );
        assert_eq!(flight_time_ms([0.0; 3], tiles(2), BALL_SPEED), 256);
        assert_eq!(flight_time_ms([0.0; 3], tiles(14), BALL_SPEED), 1792);
        assert_eq!(
            flight_time_ms([0.0; 3], w([18261, 1500, 16310]), BALL_SPEED),
            766
        );
    }

    fn arrow(sim: &mut Sim, a: ObjectId, t: ObjectId, amount: i32) -> u32 {
        let damage = crate::combat::damage::damage_message(sim, a, amount);
        let start = crate::combat::hit_zone::world_pos(sim, a).unwrap();
        let target_pos = crate::combat::hit_zone::world_pos(sim, t).unwrap();
        launch_arrow(
            sim,
            ArrowLaunch {
                shooter: a,
                target: t,
                damage,
                start,
                target_pos,
                zone: 0,
                fire: false,
                model: "pfeil".into(),
                node: "schussposition01".into(),
            },
        )
    }

    // QUIRK C-102: kept
    #[test]
    fn an_arrow_over_four_tiles_hits_in_the_eighth_effect_tick() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, ARCHER, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T);
        sim.state.events.clear();
        let id = arrow(&mut sim, a, t, -12);
        let launched = sim.state.events.iter().find_map(|e| match e {
            SimEvent::ProjectileLaunched {
                id,
                flight_ms,
                model,
                node,
                ..
            } => Some((*id, *flight_ms, model.clone(), node.clone())),
            _ => None,
        });
        assert_eq!(
            launched,
            Some((id, 731, "pfeil".to_string(), "schussposition01".to_string()))
        );
        for k in 1..=7 {
            tick(&mut sim);
            assert_eq!(hp(&sim, t), 160, "no hit yet in effect tick {k}");
        }
        sim.state.events.clear();
        tick(&mut sim);
        assert_eq!(
            hp(&sim, t),
            148,
            "-12 in the 8th effect tick (ceil(731/100))"
        );
        assert!(sim.state.projectiles.list.is_empty());
        assert!(sim.state.events.contains(&SimEvent::ProjectileEnded {
            id,
            hit: true,
            pos: crate::combat::hit_zone::world_pos(&sim, t).unwrap()
        }));
        assert!(sim.state.events.iter().any(|e| matches!(e, SimEvent::EffectAttached { object, path, .. } if *object == t && path.ends_with("blood.pef"))));
    }

    // QUIRK C-100, C-101: kept
    #[test]
    fn an_arrow_needs_shooter_and_target_until_the_owner_change() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, ARCHER, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 2 * 4096, T);
        // Shooter dies in flight but still exists (`GetObjectById(id, 0)`): hit.
        arrow(&mut sim, a, t, -12);
        sim.object_mut(a).unwrap().status |= crate::object::status::DIED;
        for _ in 0..4 {
            tick(&mut sim);
        }
        assert_eq!(hp(&sim, t), 148);
        // After the change of owner to slot 8 the old ID is gone: no damage, end without a hit.
        let id = arrow(&mut sim, a, t, -12);
        crate::world::change_owner(&mut sim, a, 8);
        sim.state.events.clear();
        for _ in 0..4 {
            tick(&mut sim);
        }
        assert_eq!(hp(&sim, t), 148);
        assert!(
            sim.state.events.iter().any(
                |e| matches!(e, SimEvent::ProjectileEnded { id: i, hit: false, .. } if *i == id)
            )
        );
    }

    // QUIRK C-101: kept
    #[test]
    fn a_target_quartered_in_flight_is_still_hit() {
        // C-101: arrows hit as long as shooter and target exist - also a target quartered in flight.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, ARCHER, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 2 * 4096, T);
        arrow(&mut sim, a, t, -12);
        sim.object_mut(t).unwrap().status |= crate::object::status::QUARTERED;
        for _ in 0..4 {
            tick(&mut sim);
        }
        assert_eq!(hp(&sim, t), 148);
        assert!(sim.state.projectiles.list.is_empty());
    }

    fn ball(sim: &mut Sim, a: ObjectId, t: ObjectId, target_pos: [f32; 3]) -> u32 {
        let damage = crate::combat::damage::damage_message(sim, a, -70);
        let start = crate::combat::hit_zone::world_pos(sim, a).unwrap();
        launch_ball(
            sim,
            BallLaunch {
                shooter: a,
                target: t,
                damage,
                start,
                target_pos,
                node: "Muendung01".into(),
                yaw_extra: 0.0,
            },
        )
    }

    // QUIRK C-104: kept
    #[test]
    fn splash_falls_off_with_the_edge_distance() {
        // re/combat-projectiles 9.3: amount -70, radius 512: -70/-52/-29/-5 at 0/0.5/1/1.5 tiles.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, (2 << 12) + 2048, (2 << 12) + 2048);
        let units: Vec<_> = [0, 2048, 4096, 6144]
            .map(|dx| at(&mut sim, WARRIOR, 1, T + dx, T + 3 * 4096))
            .to_vec();
        let target = units[0];
        let aim = crate::combat::hit_zone::world_pos(&sim, target).unwrap();
        ball(&mut sim, a, target, aim);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        let got: Vec<i32> = units.iter().map(|&u| hp(&sim, u)).collect();
        assert_eq!(got, vec![90, 108, 131, 155]);
        let impacts = sim.state.events.iter().filter(|e| matches!(e, SimEvent::EffectAt { path, scale, .. } if path.ends_with("einschlag_boden.pef") && *scale == 0.5)).count();
        assert_eq!(impacts, 4, "one impact x0.5 per unit");
        assert!(sim.state.events.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d {
                action: 3,
                flags: 0x42,
                ..
            }
        )));
    }

    #[test]
    fn the_launch_event_carries_the_muzzle_node_and_the_smoke_turn() {
        // Cannon: `Muendung01`, no rotation; broadside (re/ships 3.6): node and +-pi/2 from `BallLaunch`.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        ball(&mut sim, a, t, aim);
        let damage = crate::combat::damage::damage_message(&mut sim, a, -70);
        let start = crate::combat::hit_zone::world_pos(&sim, a).unwrap();
        launch_ball(
            &mut sim,
            BallLaunch {
                shooter: a,
                target: t,
                damage,
                start,
                target_pos: aim,
                node: "backcannon03".into(),
                yaw_extra: std::f32::consts::FRAC_PI_2,
            },
        );
        let seen: Vec<(String, f32)> = sim
            .state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::ProjectileLaunched {
                    node, yaw_extra, ..
                } => Some((node.clone(), *yaw_extra)),
                _ => None,
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("Muendung01".to_string(), 0.0),
                ("backcannon03".to_string(), std::f32::consts::FRAC_PI_2)
            ]
        );
    }

    // QUIRK C-105: kept
    #[test]
    fn a_ball_without_target_splashes_at_the_old_aim() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, (2 << 12) + 2048, (2 << 12) + 2048);
        let target = at(&mut sim, WARRIOR, 1, T, T);
        let other = at(&mut sim, WARRIOR, 1, T + 2048, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, target).unwrap();
        ball(&mut sim, a, target, aim);
        crate::world::destroy_object(&mut sim, target);
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert_eq!(
            hp(&sim, other),
            108,
            "splash around the target position at launch (C-105)"
        );
    }

    // QUIRK C-106, C-107: kept
    #[test]
    fn an_empty_splash_hits_a_hostile_building_beside_the_shooter_or_nothing() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, T, T);
        let nobody = at(&mut sim, WARRIOR, 1, T + 10 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, nobody).unwrap();
        // Target gone, no building at the shooter: no damage, no effect, no sound (C-107).
        ball(&mut sim, a, nobody, aim);
        crate::world::destroy_object(&mut sim, nobody);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::EffectAt { .. } | SimEvent::Sound3d { .. }))
        );
        // Enemy HQ one tile next to the shooter: full damage without falloff (C-106). The shooter
        // is a saber fighter, so `ATTACKBONUS_UNIT_BUILDING` (-20 %, re/combat-damage 7.2 case 7) applies;
        // the +150 % `CANNON_BUILDING` exists only for attackers with the cannon flag (net SPA/ENG cannon x2.0,
        // SID x2.5, PAT x1.0; C-123, C-106). Only checked: the HQ takes damage.
        let hq = spawn(&mut sim, HQ, 1, (11, 10));
        let before = sim.object(hq).unwrap().attributes.raw_fraction(14);
        let far = at(&mut sim, WARRIOR, 1, T + 10 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, far).unwrap();
        ball(&mut sim, a, far, aim);
        crate::world::destroy_object(&mut sim, far);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert!(sim.object(hq).unwrap().attributes.raw_fraction(14) < before);
        assert!(
            sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::EffectAt { scale, .. } if *scale == 1.0))
        );
    }

    // QUIRK C-100, C-112: kept
    // QUIRK C-89: deviation
    #[test]
    fn hard_targets_take_a_direct_hit_and_orphans_sound_like_the_warship() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, T, T);
        let hq = spawn(&mut sim, HQ, 1, (14, 10));
        let near = at(&mut sim, WARRIOR, 1, (14 << 12) + 2048, (12 << 12) + 2048);
        let before = sim.object(hq).unwrap().attributes.raw_fraction(14);
        let aim = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        ball(&mut sim, a, hq, aim);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert!(sim.object(hq).unwrap().attributes.raw_fraction(14) < before);
        assert_eq!(hp(&sim, near), 160, "direct hit without splash");
        assert!(
            sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::Sound3d { action: 2, .. }))
        );
        // Shooter gone: impact with sound and type 1109, no damage (C-112).
        let aim = crate::combat::hit_zone::world_pos(&sim, near).unwrap();
        ball(&mut sim, a, near, aim);
        crate::world::destroy_object(&mut sim, a);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert_eq!(hp(&sim, near), 160);
        assert!(sim.state.events.iter().any(|e| matches!(
            e,
            SimEvent::Sound3d {
                guid: 1109,
                action: 3,
                ..
            }
        )));
    }

    // QUIRK C-104: kept
    #[test]
    fn splash_at_the_map_edge_counts_only_inside() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, T, T);
        let corner = at(&mut sim, WARRIOR, 1, 2048, 2048);
        // Center distance 6200 > 6144 (C-104: inclusion by center distance).
        let outside = at(&mut sim, WARRIOR, 1, 2048 + 6200, 2048);
        let aim = crate::combat::hit_zone::world_pos(&sim, corner).unwrap();
        ball(&mut sim, a, corner, aim);
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert_eq!(hp(&sim, corner), 90);
        assert_eq!(hp(&sim, outside), 160, "just outside the radius: no damage");
    }

    /// Positions of the cannon craters among the events.
    fn craters(events: &[SimEvent]) -> Vec<[f32; 3]> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::Crater { pos } => Some(*pos),
                _ => None,
            })
            .collect()
    }

    /// Ball from `a` at `t` at its position, until it has landed; returns the events.
    fn fire_and_land(sim: &mut Sim, a: ObjectId, t: ObjectId, aim: [f32; 3]) -> Vec<SimEvent> {
        ball(sim, a, t, aim);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(sim);
        }
        std::mem::take(&mut sim.state.events)
    }

    // QUIRK D-283: kept
    #[test]
    fn cannon_crater_only_on_the_splash_land_branch() {
        // D-283, `CannonBall_Impact` `0x004EF7C2`: crater `0x430` only in the splash branch on land, at the
        // position of the ground impact x1.5 (`target_pos`), after the sound.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = at(&mut sim, SABRE, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        let events = fire_and_land(&mut sim, a, t, aim);
        assert_eq!(craters(&events), vec![aim], "splash on land: one crater");
        let ground = events
            .iter()
            .position(|e| matches!(e, SimEvent::EffectAt { path, scale, pos, .. } if path == IMPACT_GROUND && *scale == 1.5 && *pos == aim))
            .expect("ground impact x1.5");
        let sound = events
            .iter()
            .position(|e| matches!(e, SimEvent::Sound3d { action: 3, .. }))
            .expect("sound 3");
        let crater = events
            .iter()
            .position(|e| matches!(e, SimEvent::Crater { .. }))
            .unwrap();
        assert!(ground < sound && sound < crater, "order as at 0x004EF7C2");
        // Fallback target (C-106): target gone, enemy HQ next to the shooter - impact x1.0, no crater.
        let hq = spawn(&mut sim, HQ, 1, (11, 10));
        let far = at(&mut sim, WARRIOR, 1, T + 10 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, far).unwrap();
        ball(&mut sim, a, far, aim);
        crate::world::destroy_object(&mut sim, far);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert!(
            sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::EffectAt { scale, .. } if *scale == 1.0)),
            "fallback target hit"
        );
        assert!(
            craters(&sim.state.events).is_empty(),
            "fallback target: no crater"
        );
        // Building target (hard): sparks, no crater.
        let aim = crate::combat::hit_zone::world_pos(&sim, hq).unwrap();
        let events = fire_and_land(&mut sim, a, hq, aim);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SimEvent::EffectAt { path, .. } if path == SPARKS))
        );
        assert!(craters(&events).is_empty(), "building target: no crater");
        // Shooter gone (C-112): ground impact x1.5 with sound, but no crater.
        let near = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T + 4096);
        let aim = crate::combat::hit_zone::world_pos(&sim, near).unwrap();
        ball(&mut sim, a, near, aim);
        crate::world::destroy_object(&mut sim, a);
        sim.state.events.clear();
        while !sim.state.projectiles.list.is_empty() {
            tick(&mut sim);
        }
        assert!(sim.state.events.iter().any(|e| matches!(e, SimEvent::EffectAt { path, scale, .. } if path == IMPACT_GROUND && *scale == 1.5)));
        assert!(
            craters(&sim.state.events).is_empty(),
            "without shooter: no crater"
        );
        // Water: splash with a hit, but water impact and no crater.
        let mut wet = crate::combat::testkit::rules();
        wet.map.water_level = 1.0e6;
        let mut state = crate::combat::testkit::state(&wet);
        let mut sim = Sim::new(&mut state, &wet);
        let a = at(&mut sim, SABRE, 0, T, T);
        let t = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T);
        let aim = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
        let events = fire_and_land(&mut sim, a, t, aim);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SimEvent::EffectAt { path, .. } if path == IMPACT_WATER))
        );
        assert!(craters(&events).is_empty(), "water: no crater");
    }

    // QUIRK D-248, D-283: kept
    #[test]
    fn destruction_events_leave_the_checksum_alone() {
        // Real flow: cannonball with splash on land and destruction of an HQ, then 120 engine ticks
        // (past the collapse after 4.5 s). The events `BuildingDestroyed`/`Crater` do not change
        // the checksum (before/after `events.clear()`), and a run with evaluated (kept)
        // events ends in the same state as one with discarded ones.
        let run = |keep: bool| -> (u64, ObjectId, Vec<SimEvent>) {
            let rules = rules();
            let mut state = state(&rules);
            let (hq, seen) = {
                let mut sim = Sim::new(&mut state, &rules);
                let a = at(&mut sim, SABRE, 0, T, T);
                let t = at(&mut sim, WARRIOR, 1, T + 4 * 4096, T);
                let hq = spawn(&mut sim, HQ, 1, (14, 20));
                let aim = crate::combat::hit_zone::world_pos(&sim, t).unwrap();
                ball(&mut sim, a, t, aim);
                sim.state.events.clear();
                crate::module::building::destroy::destroy(&mut sim, hq);
                while !sim.state.projectiles.list.is_empty() {
                    tick(&mut sim);
                }
                let seen = sim.state.events.clone();
                let before = crate::save::checksum(sim.state);
                let without: Vec<SimEvent> = seen
                    .iter()
                    .filter(|e| {
                        !matches!(
                            e,
                            SimEvent::BuildingDestroyed { .. } | SimEvent::Crater { .. }
                        )
                    })
                    .cloned()
                    .collect();
                sim.state.events = without;
                assert_eq!(
                    crate::save::checksum(sim.state),
                    before,
                    "without the new events"
                );
                sim.state.events.clear();
                assert_eq!(
                    crate::save::checksum(sim.state),
                    before,
                    "after events.clear()"
                );
                if keep {
                    sim.state.events = seen.clone();
                }
                (hq, seen)
            };
            for _ in 0..120 {
                crate::tick::step(&mut state, &rules, &[]);
            }
            (crate::save::checksum(&state), hq, seen)
        };
        let (kept, hq, events) = run(true);
        let (cleared, _, _) = run(false);
        assert_eq!(kept, cleared);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, SimEvent::BuildingDestroyed { object } if *object == hq))
                .count(),
            1
        );
        assert_eq!(craters(&events).len(), 1);
    }
}

/// World effects explosion, storm and dynamite. A module of its own because `tests` glob-imports
/// `combat::testkit::*`.
#[cfg(test)]
mod world_effect_tests {
    use std::cell::Cell;

    use super::*;
    use crate::combat::rank::set_rank_reset_experience;
    use crate::module::ability_testkit as ak;
    use crate::rules::Rules;
    use crate::save::{SaveGame, checksum};
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// World position of the tile center `(x, z)` at height 0.
    fn tile(x: i32, z: i32) -> [f32; 3] {
        [ak::world(x), 0.0, ak::world(z)]
    }

    /// Grid center of a tile (fixed point).
    fn grid(t: i32) -> i32 {
        (t << 12) + 2048
    }

    /// Finished object at the fixed-point grid position `(x, z)` (height 0; `p / 32` is exact).
    fn at(sim: &mut Sim, guid: u32, slot: i32, x: i32, z: i32) -> ObjectId {
        let mut d = ObjectDesc::new(guid, slot, [x as f32 / 32.0, 0.0, z as f32 / 32.0]);
        d.flags |= desc_flags::PLACED;
        let id = create_object(sim, &mut d).unwrap();
        let g = sim.object(id).unwrap().grid;
        assert_eq!((g.x.0, g.z.0), (x, z), "position unchanged");
        id
    }

    fn hp(sim: &Sim, id: ObjectId) -> i32 {
        let o = sim.object(id).unwrap();
        let pct = sim.type_upgrade_percent(o.owner(), o.guid, 14);
        o.attributes.current_value(14, pct)
    }

    fn rank_and_fraction(sim: &Sim, id: ObjectId) -> (u8, i32) {
        let o = sim.object(id).unwrap();
        (o.rank, o.attributes.current_fraction(30))
    }

    fn effects<'a>(events: &'a [SimEvent], file: &str) -> Vec<(&'a [f32; 3], f32)> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::EffectAt {
                    path, pos, scale, ..
                } if path.ends_with(file) => Some((pos, *scale)),
                _ => None,
            })
            .collect()
    }

    fn under_attack(events: &[SimEvent]) -> Vec<ObjectId> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::UnderAttack { object, .. } => Some(*object),
                _ => None,
            })
            .collect()
    }

    // QUIRK C-301: kept
    #[test]
    fn the_explosion_hits_enemy_units_only() {
        // re/spells 9.1, C-301: -75 % max on all enemy units <= 5 tiles (center distance).
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let caster = ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (5, 5));
        let mut hit: Vec<(ObjectId, i32)> = [
            (ak::FARMER, (31, 30), 25),
            (ak::MEDIC, (29, 30), 50),
            (ak::MEDICINE_MAN, (30, 32), 200),
            (ak::PILGRIM, (30, 28), 250),
            // Exactly 5 tiles: still hit.
            (ak::FARMER, (35, 30), 25),
        ]
        .into_iter()
        .map(|(guid, t, after)| (ak::spawn(&mut sim, guid, 1, t), after))
        .collect();
        // `Fixed_Hypot` via `Fixed_Div` discards the lower 2 bits (K-143): 5 tiles + 1
        // grid point measures 5 tiles, still hit (measured).
        let plus_one = at(&mut sim, ak::FARMER, 1, grid(30), grid(30) + 5 * 4096 + 1);
        hit.push((plus_one, 25));
        // The smallest measurable distance over 5 tiles (+4): not hit.
        let beyond = at(&mut sim, ak::FARMER, 1, grid(30) - 5 * 4096 - 4, grid(30));
        let own = ak::spawn(&mut sim, ak::FARMER, 0, (31, 31));
        let animal = ak::spawn(&mut sim, ak::BATTLE_WOLF, 8, (29, 29));
        let monastery = ak::spawn(&mut sim, ak::MONASTERY, 1, (33, 33));
        let untouched = [
            (beyond, 100),
            (own, 100),
            (animal, 200),
            (monastery, 10_000),
        ];
        for &(id, max) in &untouched {
            assert_eq!(hp(&sim, id), max);
        }
        sim.state.events.clear();
        add_explosion(&mut sim, caster, tile(30, 30));
        assert_eq!(
            effects(&sim.state.events, "explosion_medicineman.pef"),
            vec![(&tile(30, 30), 1.0)]
        );
        for k in 1..=4 {
            tick(&mut sim);
            assert_eq!(hp(&sim, hit[0].0), 100, "no damage yet in effect tick {k}");
        }
        sim.state.events.clear();
        tick(&mut sim);
        for &(id, after) in &hit {
            assert_eq!(hp(&sim, id), after, "{id:?}");
        }
        for &(id, max) in &untouched {
            assert_eq!(hp(&sim, id), max, "{id:?} untouched");
        }
        let impacts = effects(&sim.state.events, "einschlag_boden.pef");
        let want: Vec<[f32; 3]> = hit
            .iter()
            .map(|&(id, _)| world_pos(&sim, id).unwrap())
            .collect();
        assert_eq!(impacts.len(), hit.len(), "one impact per hit");
        for (pos, scale) in impacts {
            assert_eq!(scale, 0.5);
            assert!(want.contains(pos), "impact at a unit that was hit");
        }
        assert!(sim.state.projectiles.list.is_empty());
    }

    thread_local! {
        /// Caster and position at which the test hook creates an explosion in the caster's tick.
        static ARMED: Cell<Option<(ObjectId, [f32; 3])>> = const { Cell::new(None) };
        /// Number of the step (from 1) in which the hook created it.
        static CREATED_IN: Cell<u64> = const { Cell::new(0) };
    }

    fn explode_in_own_tick(sim: &mut Sim, id: ObjectId) {
        if let Some((caster, pos)) = ARMED.get()
            && caster == id
        {
            ARMED.set(None);
            add_explosion(sim, id, pos);
            CREATED_IN.set(sim.state.steps + 1);
        }
    }

    /// How the explosion is created: in the object tick of a caster of group 0 or 1, or from
    /// the test after an odd step (group 0 has run, the effects have not yet).
    #[derive(Clone, Copy, Debug)]
    enum Creation {
        Group(usize),
        AfterOddStep,
    }

    /// Steps from the creation step (or the step before it) to the step with the damage.
    fn explosion_delay(creation: Creation) -> u64 {
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let casters = [
            ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (5, 5)),
            ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (8, 5)),
        ];
        for (g, c) in casters.iter().enumerate() {
            assert_eq!(sim.state.tick_groups[g], [*c], "caster {g} in group {g}");
        }
        let target = ak::spawn(&mut sim, ak::FARMER, 1, (50, 50));
        ak::run_ticks(&mut sim, 2);
        assert_eq!(sim.state.phase, 0);
        match creation {
            Creation::Group(g) => ARMED.set(Some((casters[g], tile(50, 50)))),
            Creation::AfterOddStep => {
                crate::tick::step(sim.state, sim.rules, &[]);
                add_explosion(&mut sim, casters[0], tile(50, 50));
                CREATED_IN.set(sim.state.steps);
            }
        }
        for _ in 0..30 {
            crate::tick::test_hook::with(explode_in_own_tick, || {
                crate::tick::step(sim.state, sim.rules, &[]);
            });
            if hp(&sim, target) < 100 {
                assert_eq!(hp(&sim, target), 25);
                return sim.state.steps - CREATED_IN.get();
            }
        }
        panic!("{creation:?}: no damage");
    }

    // QUIRK C-301: kept
    #[test]
    fn the_explosion_waits_five_effect_ticks() {
        // Effect tick k = object tick N+k-1 (the effect tick runs in the second step of the
        // object tick after group 1); damage in the fifth. Measured: group 0 nine steps after the
        // creation step, group 1 eight; from the test after an odd step like group 0.
        const GROUP_0: u64 = 9;
        const GROUP_1: u64 = 8;
        assert_eq!(explosion_delay(Creation::Group(0)), GROUP_0);
        assert_eq!(explosion_delay(Creation::Group(1)), GROUP_1);
        assert_eq!(explosion_delay(Creation::AfterOddStep), GROUP_0);
    }

    /// [`ak::spell_rules`] with monastery max `max` (differing maxima per world).
    fn storm_rules(max: i32) -> Rules {
        let mut r = ak::spell_rules();
        r.defaults.get_mut(&ak::MONASTERY).unwrap().insert(14, max);
        r
    }

    // QUIRK C-302: kept
    #[test]
    fn the_storm_hits_enemy_buildings_in_35_pulses() {
        // re/spells 9.2, C-302: pulses at -2 % in effect ticks 2, 4, ..., 70; entry gone in the 71st.
        for (max, pulse) in [(1000, 20), (150, 3), (3000, 60)] {
            let rules = storm_rules(max);
            let mut state = ak::state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let caster = ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (5, 5));
            let enemy = ak::spawn(&mut sim, ak::MONASTERY, 1, (33, 30));
            let own = ak::spawn(&mut sim, ak::MONASTERY, 0, (27, 30));
            let unit = ak::spawn(&mut sim, ak::FARMER, 1, (30, 31));
            assert_eq!(hp(&sim, enemy), max);
            sim.state.events.clear();
            add_storm(&mut sim, caster, tile(30, 30));
            assert!(sim.state.events.is_empty(), "no event on creation");
            for k in 1..=70 {
                tick(&mut sim);
                assert_eq!(
                    hp(&sim, enemy),
                    max - pulse * (k / 2),
                    "max {max}, effect tick {k}"
                );
                assert_eq!(sim.state.projectiles.list.len(), 1, "effect tick {k}");
            }
            assert_eq!(sim.state.projectiles.list[0].remaining_ms, 0);
            tick(&mut sim);
            assert!(
                sim.state.projectiles.list.is_empty(),
                "gone in the 71st effect tick"
            );
            assert_eq!(hp(&sim, enemy), max - 35 * pulse);
            assert_eq!(hp(&sim, own), max, "own building untouched");
            assert_eq!(hp(&sim, unit), 100, "unit untouched");
        }
    }

    /// Spell world with demolition expert (slot 0, rank 0 with Max(30) = 100) and two enemy
    /// monasteries (HP 10,000).
    struct DynamiteWorld {
        bomber: ObjectId,
        target: ObjectId,
        neighbour: ObjectId,
    }

    fn dynamite_world(sim: &mut Sim) -> DynamiteWorld {
        let bomber = ak::spawn(sim, ak::PATRIOT, 0, (10, 10));
        set_rank_reset_experience(sim, bomber, 0);
        DynamiteWorld {
            bomber,
            target: ak::spawn(sim, ak::MONASTERY, 1, (14, 10)),
            neighbour: ak::spawn(sim, ak::MONASTERY, 1, (14, 16)),
        }
    }

    /// Charge at `target` at the position of the demolition expert; 99 effect ticks without, the 100th
    /// with explosion. Returns: events of the 100th effect tick.
    fn charge_and_wait(sim: &mut Sim, bomber: ObjectId, target: ObjectId) -> Vec<SimEvent> {
        let pos = world_pos(sim, bomber).unwrap();
        add_dynamite(sim, bomber, target, pos, 2);
        let before = sim.object(target).map(|_| hp(sim, target));
        for k in 1..=99 {
            tick(sim);
            assert_eq!(
                sim.object(target).map(|_| hp(sim, target)),
                before,
                "effect tick {k}"
            );
        }
        sim.state.events.clear();
        tick(sim);
        assert!(sim.state.projectiles.list.is_empty());
        let events = std::mem::take(&mut sim.state.events);
        assert_eq!(
            effects(&events, "explosion_dynamite.pef"),
            vec![(&pos, 1.0)]
        );
        events
    }

    // QUIRK C-329, C-330: kept
    #[test]
    fn dynamite_hits_its_target_once() {
        // re/special-attacks 9 T1/T2, C-329, C-330: -4000 via `0x17F` with the living demolition expert
        // -> building penalty -20 % = -3200; only the target.
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = dynamite_world(&mut sim);
        let events = charge_and_wait(&mut sim, w.bomber, w.target);
        assert_eq!(hp(&sim, w.target), 6800);
        assert_eq!(hp(&sim, w.neighbour), 10_000, "no area damage");
        assert_eq!(under_attack(&events), [w.target]);
        assert_eq!(
            rank_and_fraction(&sim, w.bomber),
            (0, 0),
            "unoccupied: no experience"
        );
        // T2: occupied building = alarm post with a quartered colonist. Experience
        // -d/1000 = 3200; `add_scaled_to_current(30, 3200, 10)` at Max(30) = 100 gives the fraction
        // 320,000 >= 100,000 -> full -> rank 1, fraction 0 (the excess is forfeited; measured).
        let post = ak::spawn(&mut sim, ak::ALARM_POST, 1, (20, 10));
        let guest = ak::spawn(&mut sim, ak::COLONIST, 1, (20, 13));
        assert_eq!(
            crate::module::quarter::accept_unit(&mut sim, post, guest),
            0
        );
        assert!(
            crate::filter::matches_class(&mut sim, post, 0x217),
            "occupied"
        );
        charge_and_wait(&mut sim, w.bomber, post);
        assert_eq!(
            rank_and_fraction(&sim, w.bomber),
            (1, 0),
            "+3200 experience"
        );
    }

    // QUIRK C-329: kept
    #[test]
    fn a_charge_without_its_bomber_hits_harder() {
        // re/special-attacks 9 T3, C-329: without the demolition expert no attacker calculation -> -4000.
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = dynamite_world(&mut sim);
        let pos = world_pos(&sim, w.bomber).unwrap();
        add_dynamite(&mut sim, w.bomber, w.target, pos, 2);
        for _ in 0..50 {
            tick(&mut sim);
        }
        crate::world::destroy_object(&mut sim, w.bomber);
        for _ in 0..50 {
            tick(&mut sim);
        }
        assert!(sim.state.projectiles.list.is_empty());
        assert_eq!(hp(&sim, w.target), 6000);
        assert_eq!(hp(&sim, w.neighbour), 10_000);
    }

    // QUIRK C-330: kept
    #[test]
    fn the_dynamite_spares_a_vanished_target() {
        // re/special-attacks 3.3: explosion effect always, damage only to an existing target.
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = dynamite_world(&mut sim);
        let pos = world_pos(&sim, w.bomber).unwrap();
        add_dynamite(&mut sim, w.bomber, w.target, pos, 2);
        crate::world::destroy_object(&mut sim, w.target);
        sim.state.events.clear();
        for _ in 0..100 {
            tick(&mut sim);
        }
        assert!(sim.state.projectiles.list.is_empty());
        assert_eq!(
            effects(&sim.state.events, "explosion_dynamite.pef").len(),
            1
        );
        assert!(under_attack(&sim.state.events).is_empty());
        assert_eq!(hp(&sim, w.neighbour), 10_000);
        // Charge without target (ship too far, C-328).
        let events = charge_and_wait(&mut sim, w.bomber, ObjectId::NONE);
        assert!(under_attack(&events).is_empty());
        assert_eq!(hp(&sim, w.neighbour), 10_000);
        assert_eq!(rank_and_fraction(&sim, w.bomber), (0, 0));
    }

    // QUIRK C-301, C-302: kept
    #[test]
    fn the_effect_keeps_the_casters_slot() {
        // re/spells 4: explosion and storm hit the enemies of the slot at the time of
        // creation (`flags`), even if the caster changes owner afterwards or disappears.
        type Vanish = fn(&mut Sim, ObjectId);
        let vanish: [(&str, Vanish); 2] = [
            ("change of owner", |sim, id| {
                crate::world::change_owner(sim, id, 1).unwrap();
            }),
            ("destroyed", |sim, id| crate::world::destroy_object(sim, id)),
        ];
        for (what, vanish) in vanish {
            let rules = ak::spell_rules();
            let mut state = ak::state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let caster = ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (5, 5));
            let enemy = ak::spawn(&mut sim, ak::FARMER, 1, (20, 21));
            let own = ak::spawn(&mut sim, ak::FARMER, 0, (21, 20));
            let enemy_building = ak::spawn(&mut sim, ak::MONASTERY, 1, (40, 43));
            let own_building = ak::spawn(&mut sim, ak::MONASTERY, 0, (40, 37));
            add_explosion(&mut sim, caster, tile(20, 20));
            add_storm(&mut sim, caster, tile(40, 40));
            vanish(&mut sim, caster);
            assert!(sim.object(caster).is_none(), "{what}: ID stale");
            assert!(sim.state.projectiles.list.iter().all(|p| p.flags == 0));
            while !sim.state.projectiles.list.is_empty() {
                tick(&mut sim);
            }
            assert_eq!(hp(&sim, enemy), 25, "{what}");
            assert_eq!(hp(&sim, own), 100, "{what}");
            assert_eq!(hp(&sim, enemy_building), 3000, "{what}");
            assert_eq!(hp(&sim, own_building), 10_000, "{what}");
        }
    }

    /// Arrow and explosion with the same due tick: arrow over (2, 1) tiles
    /// (sqrt(5) * 128 / 0.7 -> 408 ms) and explosion (500 ms) both fall in the 5th effect tick. Returns:
    /// order of the `UnderAttack`, arrow target, explosion target.
    fn damage_order(arrow_first: bool) -> (Vec<ObjectId>, ObjectId, ObjectId) {
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let archer = ak::spawn(&mut sim, ak::ARCHER, 0, (10, 10));
        let shot = ak::spawn(&mut sim, ak::FARMER, 1, (12, 11));
        let caster = ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (10, 30));
        let blasted = ak::spawn(&mut sim, ak::FARMER, 1, (40, 40));
        let launch = |sim: &mut Sim| {
            let damage = crate::combat::damage::damage_message(sim, archer, -12);
            let start = world_pos(sim, archer).unwrap();
            let target_pos = world_pos(sim, shot).unwrap();
            launch_arrow(
                sim,
                ArrowLaunch {
                    shooter: archer,
                    target: shot,
                    damage,
                    start,
                    target_pos,
                    zone: 0,
                    fire: false,
                    model: "pfeil".into(),
                    node: "schussposition01".into(),
                },
            )
        };
        if arrow_first {
            launch(&mut sim);
            add_explosion(&mut sim, caster, tile(40, 40));
        } else {
            add_explosion(&mut sim, caster, tile(40, 40));
            launch(&mut sim);
        }
        let arrow = usize::from(!arrow_first);
        assert_eq!(sim.state.projectiles.list[arrow].remaining_ms, 408);
        let events = ak::run_ticks_collect(&mut sim, 5);
        assert!(sim.state.projectiles.list.is_empty());
        (under_attack(&events), shot, blasted)
    }

    // QUIRK C-301: kept
    #[test]
    fn world_effects_tick_in_creation_order() {
        let (order, shot, blasted) = damage_order(true);
        assert_eq!(order, [shot, blasted], "arrow created first");
        let (order, shot, blasted) = damage_order(false);
        assert_eq!(order, [blasted, shot], "explosion created first");
    }

    #[test]
    fn world_effects_survive_saving_and_loading() {
        // Dynamite, 45 object ticks, then explosion and storm, one step, save; then 120 steps
        // against the continuous sequence.
        let rules = ak::spell_rules();
        let mut state = ak::state(&rules);
        let (charged, stormed, blasted) = {
            let mut sim = Sim::new(&mut state, &rules);
            let bomber = ak::spawn(&mut sim, ak::PATRIOT, 0, (10, 10));
            let charged = ak::spawn(&mut sim, ak::MONASTERY, 1, (14, 10));
            let caster = ak::spawn(&mut sim, ak::MEDICINE_MAN, 0, (10, 40));
            let blasted = ak::spawn(&mut sim, ak::FARMER, 1, (40, 40));
            let stormed = ak::spawn(&mut sim, ak::MONASTERY, 1, (40, 20));
            let pos = world_pos(&sim, bomber).unwrap();
            add_dynamite(&mut sim, bomber, charged, pos, 2);
            ak::run_ticks(&mut sim, 45);
            add_explosion(&mut sim, caster, tile(40, 40));
            add_storm(&mut sim, caster, tile(40, 20));
            crate::tick::step(sim.state, sim.rules, &[]);
            (charged, stormed, blasted)
        };
        let kinds: Vec<_> = state
            .projectiles
            .list
            .iter()
            .map(|p| (p.kind, p.remaining_ms))
            .collect();
        assert_eq!(
            kinds,
            [
                (ProjectileKind::Dynamite, 5500),
                (ProjectileKind::Explosion, 500),
                (ProjectileKind::Storm, 7000)
            ]
        );
        let save = SaveGame::from_bytes(&crate::save::testkit::save_bytes(&state, &rules)).unwrap();
        save.check(&rules).unwrap();
        let mut loaded: SimState = save.state;
        assert_eq!(checksum(&loaded), checksum(&state));
        for i in 1..=120 {
            crate::tick::step(&mut state, &rules, &[]);
            crate::tick::step(&mut loaded, &rules, &[]);
            assert_eq!(
                checksum(&loaded),
                checksum(&state),
                "step {i} after loading"
            );
        }
        let sim = Sim::new(&mut loaded, &rules);
        assert_eq!(hp(&sim, blasted), 25, "explosion after loading");
        assert_eq!(hp(&sim, charged), 6800, "dynamite after loading");
        assert_eq!(
            hp(&sim, stormed),
            10_000 - 30 * 200,
            "30 storm pulses after loading"
        );
    }
}
