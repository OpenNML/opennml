//! Persistent world effects of the spells and special attacks (re/spells 7, re/special-attacks 7):
//! storm, dynamite charge and smith effect, pure presentation derived from the simulation state
//! after the pattern of [`crate::game::ability_effects::AbilityEffects`].
//!
//! Deviation from the original: the simulation reports only one-shot effects as an event; what lives
//! long, the host derives from the state each frame - this survives loading and reconnecting and
//! needs no effect keys in the savegame:
//!
//! - **Storm** (`Storm_Construct` `0x004F42C0`): `storm_medicineman.pef` at `target_pos`, as long
//!   as the entry [`ProjectileKind::Storm`] is in the projectile list (7000 ms). Height =
//!   terrain ([`ground_lift`], D-237).
//! - **Dynamite charge** (`DynamiteEffect` `0x004F04D0`): the charge model 370002 of the
//!   demolition expert type (type GUID from `damage.arg(2)`, the charge outlives its demolition
//!   expert) with the charge animation (`zone`, 370004/370006) and the time since it was laid.
//!   The host keeps a [`ChargeTrack`] per charge (D-244, D-281, D-282): offset and yaw offset to
//!   the target when laid (`0x004F04D0`), following each frame (`0x004F0480`), drawing
//!   `T(position) * RotY(rotation)` without a fog check (`0x004F0620`), digits 9 ... 0 at the
//!   node `dynamite01` (`DynamiteEffect_Tick` `0x004F07A0`) and the explosion at the position of
//!   the last frame (`0x004F06A0`).
//! - **Smith effect** (`SpecialMove`, `m_EffectID`): `schmied_rundumschlag.pef` (mode 0) or
//!   `schmied_gebäude.pef` (mode 1) at the node `schmiedhammer01`, as long as `busy` is set
//!   (re/special-attacks 3.6, 3.7).
//!
//! In addition the height rule of the event effects ([`ground_lift`]): explosion, storm and
//! scouting sit at the terrain height in the viewer (D-237).

use std::collections::BTreeMap;

use glam::{Affine3A, Vec3};
use opennml_effects::attached::{Anchor, UNLIMITED_MS};
use opennml_effects::output::{DrawCommand, EffectFrame, ModelDraw};
use opennml_effects::particles::Placement;
use opennml_effects::world::EffectWorld;
use opennml_rda::{Vfs, normalize_path};
use opennml_sim::combat::projectile::ProjectileKind;
use opennml_sim::id::ObjectId;
use opennml_sim::module::ModuleKind;
use opennml_sim::module::special::{bomberman, special_move};
use opennml_sim::rules::Rules;
use opennml_sim::state::SimState;
use opennml_world::anim::AnimationPlayer;
use opennml_world::assets::ModelCache;
use opennml_world::math::object_world_matrix;

use crate::game::ability_effects::attach;
use crate::game::notices::Notices;
use crate::game::sim_host::SimHost;

/// Presentation effect of the explosion (`0x004F0D50`, re/spells 1.3); the simulation reports it
/// as `EffectAt` with height 0.
pub const EXPLOSION_EFFECT: &str = "data/effects/explosion_medicineman.pef";
/// Presentation effect of the storm (`0x004F42C0`, re/spells 1.4).
pub const STORM_EFFECT: &str = "data/effects/storm_medicineman.pef";
/// Presentation effect of scouting (`0x37`, re/spells 3.6); the simulation reports it as `EffectAt`
/// with height 0, the original sets the terrain height.
pub const SCOUT_EFFECT: &str = "data/effects/aufklären.pef";
/// Effect of the whirlwind (`0x625D6C`, re/special-attacks 3.6).
pub const WHIRL_EFFECT: &str = "data/effects/schmied_rundumschlag.pef";
/// Effect of "Damage building" (`0x625DB8`, re/special-attacks 3.7).
pub const HAMMER_EFFECT: &str = "data/effects/schmied_gebäude.pef";
/// Node of both smith effects (`0x625D5C`).
pub const HAMMER_NODE: &str = "schmiedhammer01";
/// `m_DynamiteObject` (370002): model of the charge at the type of the demolition expert.
const CHARGE_MODEL_VALUE: u32 = 370_002;
/// Explosion of the dynamite charge (`FUN_004F06A0`, suggested name `DynamiteEffect_Explode`). The
/// simulation reports it as `EffectAt` at the laying position (`p.target_pos`); its constant is
/// private there, compared after [`normalize_path`].
pub const DYNAMITE_EXPLOSION: &str = "data/effects/explosion_dynamite.pef";
/// Helper node of the digits in the charge model (`DynamiteEffect_Tick` `0x004F07A0`).
pub const CHARGE_NODE: &str = "dynamite01";
/// Color of the digits (`FUN_00493030(s, 0xC8C8C8, ...)` from `0x004F07A0`).
pub const DIGIT_COLOR: u32 = 0xC8_C8C8;
/// Start value of `m_LastCount` (`10000 / 1000`, `0x004F04D0`).
const DIGIT_START: i32 = 10;

/// Presentation track of a dynamite charge (fields `+0x18` ... `+0x38` of the `DynamiteEffect`,
/// re/special-attacks 1.5), per projectile ID in the host; not in the savegame.
///
/// Created in the step path (after the `advance_by` in which the charge appears in the projectile
/// list), not at the first frame: the original measures in the constructor `0x004F04D0` in the
/// tick of laying; the host measures after the `advance_by` of laying (the target has moved at
/// most two ticks). It is followed after every `advance_by` and before every frame
/// ([`WorldEffects::sync_charges`]). Charges found after loading or restarting
/// ([`WorldEffects::reset`]) stay at the laying position without offset and rotation
/// (`follows = false`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChargeTrack {
    /// Target (`m_TargetHID`, `NONE` without a target).
    pub target: ObjectId,
    /// `m_Offset`: laying position - target position, in world axes (not rotated along, D-282).
    pub offset: Vec3,
    /// `m_YRotOffset`: yaw of the demolition expert - yaw of the target (radians).
    pub yaw_offset: f32,
    /// Position of the last frame (`+0x24`, place of the explosion).
    pub pos: Vec3,
    /// Rotation of the last frame (`+0x34`).
    pub yaw: f32,
    /// Laying position `p.target_pos`: assigns the explosion event to the track.
    pub laid: [f32; 3],
    /// `m_LastCount`: digit shown last.
    pub last_digit: i32,
    /// Follows the target (created with a target, not found after loading).
    pub follows: bool,
}

/// World matrix of the charge `T(pos) * RotY(yaw)` (`0x004F0620`) in the rotation direction of
/// the objects.
fn charge_matrix(pos: Vec3, yaw: f32) -> Affine3A {
    object_world_matrix(pos, Vec3::new(0.0, yaw, 0.0), Vec3::ONE)
}

/// Event effects at terrain height (D-237): the original put explosion and storm at height 0 (the
/// viewer deviates), scouting at the terrain height of the host (the viewer is the same as the
/// original). Full event paths; compared after [`normalize_path`].
pub const GROUND_EFFECTS: [&str; 3] = [EXPLOSION_EFFECT, STORM_EFFECT, SCOUT_EFFECT];

/// Position of an event effect `path` in the viewer: the three [`GROUND_EFFECTS`] get the terrain
/// height `height(x, z)` as y, every other path stays unchanged.
pub fn ground_lift(path: &str, pos: [f32; 3], height: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    let path = normalize_path(path);
    if GROUND_EFFECTS.iter().any(|g| normalize_path(g) == path) {
        [pos[0], height(pos[0], pos[2]), pos[2]]
    } else {
        pos
    }
}

/// Key of a derived effect. IDs of projectiles and objects are separate number spaces and can
/// coincide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Projectile(u32),
    Object(u32),
}

/// What the host should show.
#[derive(Debug, Clone, PartialEq)]
pub enum Show {
    /// Storm at `pos` (height of the simulation, the sync lifts it onto the terrain).
    Storm { pos: [f32; 3] },
    /// Charge model `model` (normalized VFS path), laid at `pos`, animation `animation` since
    /// `elapsed_ms`. Not an effect of the effect world: drawn by [`charge_draws`] at the position of
    /// its [`ChargeTrack`].
    Charge {
        model: String,
        animation: i32,
        elapsed_ms: i32,
        pos: [f32; 3],
    },
    /// Game effect `path` at the node `node` of the object.
    Smith {
        path: &'static str,
        node: &'static str,
    },
}

/// A derived effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub key: Key,
    pub show: Show,
}

/// Created effect of a [`Wanted`].
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    show: Show,
    /// Key of the game effect; `None`: file not loadable (no new attempt).
    effect: Option<u32>,
    /// Position at which it was created (storm: with terrain height).
    at: [f32; 3],
}

/// Persistent world effects of a map.
#[derive(Debug, Default)]
pub struct WorldEffects {
    shown: BTreeMap<Key, Shown>,
    /// Tracks of the dynamite charges per projectile ID.
    charges: BTreeMap<u32, ChargeTrack>,
    /// After [`WorldEffects::reset`]: the next sync creates charges found without offset.
    after_reset: bool,
}

impl WorldEffects {
    /// Remove all effects (loading, restart); the tracks of the charges too, the charges found are
    /// silently created at the laying position by the next [`WorldEffects::track_charges`].
    pub fn reset(&mut self, effects: &mut EffectWorld) {
        for shown in std::mem::take(&mut self.shown).into_values() {
            if let Some(k) = shown.effect {
                effects.remove_game_effect(k);
            }
        }
        self.charges.clear();
        self.after_reset = true;
    }

    /// Tracks of the dynamite charges per projectile ID.
    pub fn charges(&self) -> &BTreeMap<u32, ChargeTrack> {
        &self.charges
    }

    /// Sync of the tracks after a logic step (`MapView::apply_sim_events`, after the events) and
    /// before every frame: tracks of vanished charges are removed, new charges get their track
    /// like `DynamiteEffect_Construct` (`0x004F04D0`): with an existing target
    /// `offset = laying position - target position` and `yaw_offset = yaw(demolition expert) -
    /// yaw(target)`, otherwise fixed at the laying position with the yaw of the demolition expert
    /// (0 if missing). The yaw of the demolition expert is the simulation value `rotation[1]`
    /// (`GO+0x158` when igniting; the presentation already turns it toward the way back in the tick
    /// of laying), position and yaw of the target come from [`SimHost::display`] as when
    /// following. After [`WorldEffects::reset`] (loading, restart) without offset and rotation,
    /// `m_LastCount` = the digit of this state (it was in the savegame of the original). Reads only
    /// the state.
    pub fn track_charges(&mut self, sim: &SimHost) {
        let list = &sim.state.projectiles.list;
        self.charges
            .retain(|id, _| list.iter().any(|p| p.id == *id));
        for p in list {
            if p.kind != ProjectileKind::Dynamite || self.charges.contains_key(&p.id) {
                continue;
            }
            let laid = Vec3::from(p.target_pos);
            let track = if self.after_reset {
                ChargeTrack {
                    target: p.target,
                    offset: Vec3::ZERO,
                    yaw_offset: 0.0,
                    pos: laid,
                    yaw: 0.0,
                    laid: p.target_pos,
                    last_digit: digit(p.remaining_ms).min(DIGIT_START),
                    follows: false,
                }
            } else {
                let yaw = sim
                    .state
                    .object(p.shooter, false)
                    .map_or(0.0, |o| o.rotation[1]);
                let target = (!p.target.is_none())
                    .then(|| sim.state.object(p.target, false))
                    .flatten()
                    .map(|t| sim.display(t));
                let (offset, yaw_offset, follows) = match target {
                    Some((at, rotation)) => (laid - at, yaw - rotation.y, true),
                    None => (Vec3::ZERO, 0.0, false),
                };
                ChargeTrack {
                    target: p.target,
                    offset,
                    yaw_offset,
                    pos: laid,
                    yaw,
                    laid: p.target_pos,
                    last_digit: DIGIT_START,
                    follows,
                }
            };
            self.charges.insert(p.id, track);
        }
        self.after_reset = false;
    }

    /// Following each frame (`0x004F0480`): `pos = target position + offset` (unrotated, D-282),
    /// `yaw = yaw(target) + yaw_offset`. If the target is gone (sunk, destroyed, change of owner
    /// with a new ID), the last values stay. Reads only the state.
    pub fn follow_charges(&mut self, sim: &SimHost) {
        for track in self.charges.values_mut().filter(|t| t.follows) {
            let Some(t) = sim.state.object(track.target, false) else {
                continue;
            };
            let (at, rotation) = sim.display(t);
            track.pos = at + track.offset;
            track.yaw = rotation.y + track.yaw_offset;
        }
    }

    /// Sync the tracks and follow ([`WorldEffects::track_charges`], then
    /// [`WorldEffects::follow_charges`]): after every `advance_by` (behind the events, so that the
    /// explosion still finds the position before the disappearance), before every frame and right
    /// after [`WorldEffects::reset`]. So in the viewer's step mode (`step N` without a frame) every
    /// step counts like a frame. Reads only the state.
    pub fn sync_charges(&mut self, sim: &SimHost) {
        self.track_charges(sim);
        self.follow_charges(sim);
    }

    /// Position of the dynamite explosion `path` at `pos` (`0x004F06A0`: at the charge position of
    /// the last frame): the first track with laying position `pos` (bit-identical) whose charge is
    /// no longer in the projectile list. `None` for other paths and without a matching track (then
    /// `pos` applies).
    pub fn explosion_at(&self, path: &str, pos: [f32; 3], state: &SimState) -> Option<Vec3> {
        if normalize_path(path) != normalize_path(DYNAMITE_EXPLOSION) {
            return None;
        }
        let bits = |v: [f32; 3]| v.map(f32::to_bits);
        self.charges
            .iter()
            .find(|(id, t)| {
                bits(t.laid) == bits(pos) && !state.projectiles.list.iter().any(|p| p.id == **id)
            })
            .map(|(_, t)| t.pos)
    }

    /// Digits of the charges after a logic step (`DynamiteEffect_Tick` `0x004F07A0`, behind
    /// `Notices::handle`, game time `bar_ms` of the message bar): `s = (remaining_ms + 100) / 1000`
    /// (the original compares before subtracting 100); if `s` falls below `last_digit`, it becomes
    /// the new digit, and at the node `dynamite01` of the animated charge (charge animation since
    /// laying), rotated and moved like drawing, the floating number `s` in [`DIGIT_COLOR`]
    /// appears - only on ground the active player sees (`0x00493030`). Without model or node no
    /// number. Reads only the state.
    pub fn charge_digits(
        &mut self,
        sim: &SimHost,
        models: &mut ModelCache,
        vfs: &Vfs,
        notices: &mut Notices,
        bar_ms: i64,
    ) {
        for p in &sim.state.projectiles.list {
            let Some(track) = self.charges.get_mut(&p.id) else {
                continue;
            };
            let s = digit(p.remaining_ms);
            if s >= track.last_digit {
                continue;
            }
            track.last_digit = s;
            let Some(asset) = charge_model(&sim.rules, p.damage.arg(2) as u32)
                .and_then(|model| models.get(vfs, &model))
            else {
                continue;
            };
            let elapsed_ms = (p.flight_ms - p.remaining_ms) as f32;
            let animation = usize::try_from(p.zone).unwrap_or(0);
            let Some(node) = AnimationPlayer::new(Some(animation), 0.0).helper_node(
                &asset,
                elapsed_ms,
                CHARGE_NODE,
            ) else {
                continue;
            };
            let at = charge_matrix(track.pos, track.yaw).transform_point3(node);
            let seen = sim.fog.view(&sim.state).world_pos_visible(
                &sim.rules.map,
                at.to_array(),
                i32::from(sim.active_player),
                false,
            );
            notices.push_float(at, s.to_string(), DIGIT_COLOR, bar_ms, seen);
        }
    }

    /// The effects the state should show: first the projectile list in its order (storm, charge),
    /// then the busy smiths in object order. The same for every viewer (the charge is drawn by
    /// [`charge_draws`] without a fog check, D-281). Reads only the state.
    pub fn wanted(state: &SimState, rules: &Rules) -> Vec<Wanted> {
        let mut out = Vec::new();
        for p in &state.projectiles.list {
            let show = match p.kind {
                ProjectileKind::Storm => Show::Storm { pos: p.target_pos },
                ProjectileKind::Dynamite => {
                    let Some(model) = charge_model(rules, p.damage.arg(2) as u32) else {
                        continue;
                    };
                    Show::Charge {
                        model,
                        animation: p.zone,
                        elapsed_ms: p.flight_ms - p.remaining_ms,
                        pos: p.target_pos,
                    }
                }
                _ => continue,
            };
            out.push(Wanted {
                key: Key::Projectile(p.id),
                show,
            });
        }
        for o in state.objects() {
            let Some(ModuleKind::SpecialMove(s)) = o.module(special_move::INDEX).map(|m| &m.kind)
            else {
                continue;
            };
            if !s.busy {
                continue;
            }
            let path = if s.mode == 1 {
                HAMMER_EFFECT
            } else {
                WHIRL_EFFECT
            };
            out.push(Wanted {
                key: Key::Object(o.id.0),
                show: Show::Smith {
                    path,
                    node: HAMMER_NODE,
                },
            });
        }
        out
    }

    /// Sync after a frame: create effects for every [`Wanted`] that has none yet (or lost it when
    /// rebuilding), remove the others. `carrier` returns the index of the object in the object list
    /// of the map (smith), `height` the terrain height. A carrier not yet drawn is tried again in
    /// the next frame. Reads only the state.
    pub fn sync(
        &mut self,
        sim: &SimHost,
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
        height: &dyn Fn(f32, f32) -> f32,
    ) {
        // The charge has no effect of the effect world.
        let wanted: Vec<Wanted> = Self::wanted(&sim.state, &sim.rules)
            .into_iter()
            .filter(|w| !matches!(w.show, Show::Charge { .. }))
            .collect();
        // End (storm expired, smith finished or object gone).
        self.shown.retain(|key, shown| {
            if wanted.iter().any(|w| w.key == *key && w.show == shown.show) {
                return true;
            }
            if let Some(k) = shown.effect {
                effects.remove_game_effect(k);
            }
            false
        });
        for w in wanted {
            if let Some(shown) = self.shown.get(&w.key) {
                if shown
                    .effect
                    .is_none_or(|k| effects.game_effect(k).is_some())
                {
                    continue;
                }
                // The rebuild of the effect world lost it: create it again.
                self.shown.remove(&w.key);
            }
            let (effect, at) = match (&w.key, &w.show) {
                (_, Show::Storm { pos }) => {
                    let at = ground_lift(STORM_EFFECT, *pos, height);
                    let placement = Placement {
                        position: Vec3::from(at),
                        yaw: 0.0,
                    };
                    let key = effects.add_game_effect(
                        vfs,
                        models,
                        STORM_EFFECT,
                        Anchor::Fixed(placement),
                        None,
                        1.0,
                        0,
                        UNLIMITED_MS,
                    );
                    if key.is_none() {
                        log::warn!("effect {STORM_EFFECT} not loadable");
                    }
                    (key, at)
                }
                (Key::Object(id), Show::Smith { path, node }) => {
                    let Some(at) = carrier(ObjectId(*id)) else {
                        continue;
                    };
                    (attach(effects, models, vfs, path, node, at), [0.0; 3])
                }
                _ => continue,
            };
            self.shown.insert(
                w.key,
                Shown {
                    show: w.show,
                    effect,
                    at,
                },
            );
        }
    }
}

/// Model of the dynamite charge (370002 of the demolition expert type `guid`) as a normalized VFS
/// path; `None` for an unknown type or an empty value.
fn charge_model(rules: &Rules, guid: u32) -> Option<String> {
    let value = rules
        .types
        .get(&guid)?
        .module(bomberman::INDEX as u32)?
        .values
        .get(&CHARGE_MODEL_VALUE)?
        .trim();
    (!value.is_empty()).then(|| normalize_path(value))
}

/// Digit of the charge for the remaining time after the tick (`0x004F07A0` divides the remaining
/// time before subtracting 100 by 1000).
fn digit(remaining_ms: i32) -> i32 {
    remaining_ms.saturating_add(100) / 1000
}

/// Draw commands of the dynamite charges (`0x004F0620`): the model as a whole `T(pos) * RotY(yaw)`
/// from the track (without a track silently at the laying position), with the charge animation and
/// its run time (speed 128). **Without a fog check** (D-281): every player sees the charge model,
/// even in fog.
pub fn charge_draws(
    wanted: &[Wanted],
    tracks: &BTreeMap<u32, ChargeTrack>,
    frame: &mut EffectFrame,
) {
    for w in wanted {
        let Show::Charge {
            model,
            animation,
            elapsed_ms,
            pos,
        } = &w.show
        else {
            continue;
        };
        let track = match w.key {
            Key::Projectile(id) => tracks.get(&id),
            Key::Object(_) => None,
        };
        let (pos, yaw) = track.map_or((Vec3::from(*pos), 0.0), |t| (t.pos, t.yaw));
        frame.commands.push(DrawCommand::Model(ModelDraw {
            model: model.clone(),
            world: charge_matrix(pos, yaw),
            animation: Some((usize::try_from(*animation).unwrap_or(0), *elapsed_ms as f32)),
            tint: None,
            alpha: None,
            blend: None,
            cull: true,
            fog: true,
            mesh: None,
            player_color: None,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testworld::{CHIEF_TENT, PATRIOT, SMITH, host, spawn};
    use opennml_formats::rdo::RdoLayout;
    use opennml_formats::testutil::{sample_rdo, write_rdo};
    use opennml_rda::testutil::{TestFile, build_rda};
    use opennml_sim::combat::projectile::{add_dynamite, add_explosion, add_storm};
    use opennml_sim::save::checksum;
    use opennml_sim::sim::Sim;
    use opennml_sim::state::SimEvent;
    use opennml_world::math::object_world_matrix;

    const PEF: &str = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
    const CHARGE_PATH: &str = "data/r3d/misc/tools/dynamitbuendel/dynamitbuendel.rdo";

    /// Archive with the three persistent effects and the charge model (sample model with the
    /// helper node `dynamite01` at `(0, 120, 0)`).
    fn archive() -> (tempfile::TempDir, Vfs) {
        let mut files: Vec<TestFile> = [STORM_EFFECT, WHIRL_EFFECT, HAMMER_EFFECT]
            .iter()
            .map(|path| TestFile {
                path,
                data: PEF.as_bytes(),
                compress: false,
            })
            .collect();
        let mut rdo = sample_rdo(RdoLayout::Standard);
        for m in &mut rdo.meshes {
            if m.name == "zauber01" {
                m.name = CHARGE_NODE.into();
            }
        }
        let rdo = write_rdo(&rdo);
        files.push(TestFile {
            path: CHARGE_PATH,
            data: &rdo,
            compress: false,
        });
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        world: WorldEffects,
        effects: EffectWorld,
        models: ModelCache,
        notices: Notices,
        /// Clock of the presentation (ms), +100 per logic step.
        now_ms: f64,
    }

    impl Fixture {
        fn new() -> Self {
            let (dir, vfs) = archive();
            Self {
                _dir: dir,
                vfs,
                host: host(),
                world: WorldEffects::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
                notices: Notices::default(),
                now_ms: 0.0,
            }
        }

        /// One logic step as in the host (`GameSession::advance_by`): simulation, explosion position
        /// of the charges (`MapView::apply_sim_events`), create and clear tracks, then the digits
        /// behind `Notices::handle`. Returns the positions of the dynamite explosions of this step.
        /// The checksum depends only on the simulation.
        fn step(&mut self) -> Vec<Vec3> {
            let events = opennml_sim::tick::step(&mut self.host.state, &self.host.rules, &[]);
            self.now_ms += 100.0;
            let sum = checksum(&self.host.state);
            let mut blasts = Vec::new();
            for e in &events {
                let SimEvent::EffectAt { path, pos, .. } = e else {
                    continue;
                };
                if normalize_path(path) != normalize_path(DYNAMITE_EXPLOSION) {
                    continue;
                }
                let at = self
                    .world
                    .explosion_at(path, *pos, &self.host.state)
                    .unwrap_or(Vec3::from(*pos));
                blasts.push(at);
            }
            self.world.sync_charges(&self.host);
            self.world.charge_digits(
                &self.host,
                &mut self.models,
                &self.vfs,
                &mut self.notices,
                self.now_ms as i64,
            );
            assert_eq!(
                checksum(&self.host.state),
                sum,
                "presentation changes nothing"
            );
            blasts
        }

        /// One frame: follow the tracks, draw the charges.
        fn frame(&mut self) -> Vec<DrawCommand> {
            let sum = checksum(&self.host.state);
            self.world.sync_charges(&self.host);
            let mut frame = EffectFrame::default();
            charge_draws(&self.wanted(), self.world.charges(), &mut frame);
            assert_eq!(
                checksum(&self.host.state),
                sum,
                "presentation changes nothing"
            );
            frame.commands
        }

        /// The one charge in the frame.
        fn charge_world(&mut self) -> Affine3A {
            let commands = self.frame();
            let [DrawCommand::Model(m)] = commands.as_slice() else {
                panic!("exactly one model: {commands:?}");
            };
            m.world
        }

        /// The demolition expert lays a charge at `pos` (yaw `yaw`) on `target`; then a sync as
        /// after the step of laying.
        fn lay(&mut self, bomber: ObjectId, target: ObjectId, pos: [f32; 3], yaw: f32) -> u32 {
            self.host.state.object_mut(bomber, false).unwrap().rotation[1] = yaw;
            self.sim(|sim| add_dynamite(sim, bomber, target, pos, 3));
            self.world.track_charges(&self.host);
            self.host.state.projectiles.list.last().unwrap().id
        }

        /// Target at `pos` with yaw `yaw` (presentation = state, no walk module).
        fn place(&mut self, id: ObjectId, pos: [f32; 3], yaw: f32) {
            let o = self.host.state.object_mut(id, false).unwrap();
            o.world_pos = pos;
            o.rotation[1] = yaw;
        }

        /// Loading or restart as `MapView::reset_sim_view`: effects and tracks new, the charges
        /// found at once silently at the laying position, no floating numbers.
        fn load(&mut self) {
            self.world.reset(&mut self.effects);
            self.world.sync_charges(&self.host);
            self.notices.clear();
        }

        /// Digits of the floating numbers so far.
        fn digits(&self) -> Vec<String> {
            self.notices.floats.iter().map(|f| f.text.clone()).collect()
        }

        /// One frame: sync with terrain height 5; the checksum stays the same.
        fn sync(&mut self) {
            let sum = checksum(&self.host.state);
            self.world.sync(
                &self.host,
                &mut self.effects,
                &mut self.models,
                &self.vfs,
                &|id| Some(id.0 as usize),
                &|_, _| 5.0,
            );
            assert_eq!(
                checksum(&self.host.state),
                sum,
                "presentation changes nothing"
            );
        }

        fn wanted(&self) -> Vec<Wanted> {
            let sum = checksum(&self.host.state);
            let out = WorldEffects::wanted(&self.host.state, &self.host.rules);
            assert_eq!(checksum(&self.host.state), sum, "query changes nothing");
            out
        }

        fn sim<T>(&mut self, f: impl FnOnce(&mut Sim) -> T) -> T {
            let rules = self.host.rules.clone();
            f(&mut Sim::new(&mut self.host.state, &rules))
        }

        fn smith(&mut self, id: ObjectId, busy: bool, mode: i32) {
            let o = self.host.state.object_mut(id, false).unwrap();
            let Some(ModuleKind::SpecialMove(s)) =
                o.module_mut(special_move::INDEX).map(|m| &mut m.kind)
            else {
                panic!("no smith");
            };
            (s.busy, s.mode) = (busy, mode);
        }

        fn drop_projectiles(&mut self) {
            self.host.state.projectiles.list.clear();
        }
    }

    #[test]
    fn world_effects_follow_the_state() {
        let mut f = Fixture::new();
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let victim = spawn(&mut f.host, SMITH, 1, (30, 20));
        let smith = spawn(&mut f.host, SMITH, 0, (40, 40));
        f.sync();
        assert!(f.wanted().is_empty(), "calm: nothing");
        assert_eq!(f.effects.counts().world, 0);

        // Storm: there as long as the entry lives; the position carries the terrain height (D-237).
        f.sim(|sim| add_storm(sim, patriot, [640.0, 0.0, 1280.0]));
        let storm_id = f.host.state.projectiles.list[0].id;
        assert_eq!(
            f.wanted(),
            [Wanted {
                key: Key::Projectile(storm_id),
                show: Show::Storm {
                    pos: [640.0, 0.0, 1280.0]
                },
            }]
        );
        f.sync();
        assert_eq!(f.effects.counts().world, 1);
        let shown = &f.world.shown[&Key::Projectile(storm_id)];
        assert_eq!(shown.at, [640.0, 5.0, 1280.0], "at terrain height");
        // Every frame again: no duplicates; complete rebuild of the effect world: created anew.
        f.sync();
        assert_eq!(f.effects.counts().world, 1);
        f.effects.clear();
        f.sync();
        assert_eq!(f.effects.counts().world, 1);

        // Charge with a target and without: one entry each, but no effect of the effect world.
        f.sim(|sim| {
            add_dynamite(sim, patriot, victim, [3000.0, 20.0, 2000.0], 2);
            add_dynamite(sim, patriot, ObjectId::NONE, [100.0, 0.0, 200.0], 3);
        });
        let wanted = f.wanted();
        let charges: Vec<_> = wanted
            .iter()
            .filter_map(|w| match &w.show {
                Show::Charge { pos, animation, .. } => Some((*pos, *animation)),
                _ => None,
            })
            .collect();
        assert_eq!(
            charges,
            [([3000.0, 20.0, 2000.0], 2), ([100.0, 0.0, 200.0], 3)]
        );
        f.sync();
        assert_eq!(
            f.effects.counts().world,
            1,
            "the charge is not an effect of the effect world"
        );

        // Smith: busy in the whirlwind, in "Damage building", free.
        f.smith(smith, true, 0);
        assert!(f.wanted().contains(&Wanted {
            key: Key::Object(smith.0),
            show: Show::Smith {
                path: WHIRL_EFFECT,
                node: HAMMER_NODE
            },
        }));
        f.sync();
        assert_eq!(f.effects.counts().world, 2);
        f.smith(smith, true, 1);
        assert!(f.wanted().contains(&Wanted {
            key: Key::Object(smith.0),
            show: Show::Smith {
                path: HAMMER_EFFECT,
                node: HAMMER_NODE
            },
        }));
        f.sync();
        assert_eq!(f.effects.counts().world, 2, "one effect replaces the other");
        f.smith(smith, false, 1);
        assert!(
            f.wanted()
                .iter()
                .all(|w| !matches!(w.show, Show::Smith { .. }))
        );
        f.sync();
        assert_eq!(f.effects.counts().world, 1);

        // Storm expired: gone. IDs of projectile and object do not collide.
        f.drop_projectiles();
        f.sync();
        assert!(f.world.shown.is_empty());
        assert_eq!(f.effects.counts().world, 0);

        // `reset` clears what is running.
        f.smith(smith, true, 0);
        f.sim(|sim| add_storm(sim, patriot, [640.0, 0.0, 1280.0]));
        f.sync();
        assert_eq!(f.effects.counts().world, 2);
        f.world.reset(&mut f.effects);
        assert!(f.world.shown.is_empty());
        assert_eq!(f.effects.counts().world, 0);
    }

    #[test]
    fn projectile_and_object_keys_do_not_collide() {
        assert_ne!(Key::Projectile(7), Key::Object(7));
        let mut f = Fixture::new();
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let smith = spawn(&mut f.host, SMITH, 0, (40, 40));
        // Projectile with the ID of the smith.
        f.sim(|sim| add_storm(sim, patriot, [640.0, 0.0, 1280.0]));
        f.host.state.projectiles.list[0].id = smith.0;
        f.smith(smith, true, 0);
        f.sync();
        assert_eq!(f.effects.counts().world, 2);
        assert_eq!(f.world.shown.len(), 2);
    }

    // QUIRK D-244: kept
    #[test]
    fn the_charge_uses_the_bomber_model() {
        let mut f = Fixture::new();
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        f.lay(patriot, ObjectId::NONE, [3000.0, 20.0, 2000.0], 0.0);
        let draw = |f: &mut Fixture| f.frame();

        // Model 370002 of the type from `damage.arg(2)`, the animation from `zone`, run time 0.
        let commands = draw(&mut f);
        let [DrawCommand::Model(m)] = commands.as_slice() else {
            panic!("exactly one model: {commands:?}");
        };
        assert_eq!(m.model, CHARGE_PATH);
        assert_eq!(m.animation, Some((3, 0.0)));
        assert_eq!(
            m.world,
            Affine3A::from_translation(Vec3::new(3000.0, 20.0, 2000.0))
        );
        assert!(m.mesh.is_none() && m.cull && m.fog);

        // The run time follows the state (flight time - remainder), the savegame stays the same.
        f.host.state.projectiles.list[0].remaining_ms -= 2500;
        let commands = draw(&mut f);
        let [DrawCommand::Model(m)] = commands.as_slice() else {
            panic!("exactly one model");
        };
        assert_eq!(m.animation, Some((3, 2500.0)));

        // The charge outlives the demolition expert: the type is in `damage.arg(2)`.
        f.sim(|sim| opennml_sim::world::queue_destroy(sim, patriot));
        for _ in 0..4 {
            opennml_sim::tick::step(&mut f.host.state, &f.host.rules, &[]);
        }
        assert!(
            f.host.state.object(patriot, false).is_none(),
            "demolition expert gone"
        );
        assert_eq!(draw(&mut f).len(), 1);

        // Type without a charge model (value missing): no entry.
        f.host
            .rules
            .types
            .get_mut(&PATRIOT)
            .unwrap()
            .modules
            .iter_mut()
            .for_each(|m| {
                m.values.remove(&CHARGE_MODEL_VALUE);
            });
        assert!(draw(&mut f).is_empty());
    }

    /// Digits 9, 8, ..., 0 (D-244, `DynamiteEffect_Tick` `0x004F07A0`) as a gray floating number,
    /// one each as soon as `(remaining_ms + 100) / 1000` falls below the last one (start 10): at
    /// `remaining_ms` 9,800, 8,800, ..., 800. Position = node `dynamite01` rotated by the charge
    /// rotation. Then the explosion at the charge.
    // QUIRK D-244: kept
    #[test]
    fn digits_count_nine_to_zero() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let laid = [3000.0, 20.0, 2000.0];
        let id = f.lay(patriot, ObjectId::NONE, laid, 0.0);
        f.frame();
        let mut shown = Vec::new();
        let mut blasts = Vec::new();
        while f.host.state.projectiles.list.iter().any(|p| p.id == id) {
            let before = f.notices.floats.len();
            blasts.extend(f.step());
            let Some(p) = f.host.state.projectiles.list.iter().find(|p| p.id == id) else {
                break;
            };
            if f.notices.floats.len() > before {
                assert_eq!(f.notices.floats.len(), before + 1, "one digit per step");
                let n = f.notices.floats.last().unwrap();
                shown.push((n.text.clone(), p.remaining_ms));
                assert_eq!(n.color, DIGIT_COLOR);
                assert_eq!(n.start_bar_ms, f.now_ms as i64, "game time of the bar");
                // Node (0, 120, 0) above the unrotated charge.
                assert_eq!(n.position, Vec3::from(laid) + Vec3::Y * 120.0);
            }
            f.frame();
        }
        let expected: Vec<(String, i32)> = (0..10)
            .rev()
            .map(|d| (d.to_string(), 1000 * d + 800))
            .collect();
        assert_eq!(shown, expected);
        assert_eq!(f.world.charges().len(), 0, "track gone with the charge");
        assert_eq!(blasts, [Vec3::from(laid)], "explosion at the charge");
        // Two seconds each (`FLOAT_MS`), then gone.
        f.notices
            .expire(f.now_ms as i64 + crate::game::notices::FLOAT_MS);
        assert!(f.notices.floats.is_empty());
    }

    /// D-282 (`0x004F0480`, `0x004F06A0`): the charge on the moving, turning ship sits at ship
    /// position + fixed, unrotated world offset and turns by ship yaw + yaw offset; the yaw of the
    /// demolition expert is the simulation value when laying. The explosion appears at the
    /// followed position of the last frame.
    // QUIRK D-244, D-282: kept
    #[test]
    fn charge_follows_a_moving_ship_with_an_unrotated_offset() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        // A target without a walk module stands for the ship: its presentation is the state.
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        let t0 = [3100.0, 10.0, 1950.0];
        f.place(ship, t0, 0.25);
        let laid = [3000.0, 20.0, 2000.0];
        f.lay(patriot, ship, laid, 0.75);
        let offset = Vec3::from(laid) - Vec3::from(t0);
        let yaw_offset = 0.75f32 - 0.25;

        // When laying: position and yaw of the demolition expert.
        assert_eq!(
            f.charge_world(),
            object_world_matrix(Vec3::from(laid), Vec3::new(0.0, 0.75, 0.0), Vec3::ONE)
        );
        // The ship moves and turns: offset fixed in world axes, rotation along.
        let mut last = Vec3::from(laid);
        for k in 1..=40 {
            let t = [3100.0 + 7.0 * k as f32, 10.0, 1950.0 - 3.0 * k as f32];
            let yaw = 0.25 + 0.05 * k as f32;
            f.place(ship, t, yaw);
            let blasts = f.step();
            assert!(blasts.is_empty());
            last = Vec3::from(t) + offset;
            assert_eq!(
                f.charge_world(),
                object_world_matrix(last, Vec3::new(0.0, yaw + yaw_offset, 0.0), Vec3::ONE),
                "frame {k}"
            );
        }
        assert_ne!(last, Vec3::from(laid));
        // Until the explosion: at the position of the last frame, not at the laying position.
        let mut blasts = Vec::new();
        while blasts.is_empty() {
            blasts = f.step();
            f.frame();
        }
        assert_eq!(blasts, [last]);
        assert!(f.world.charges().is_empty());
    }

    /// D-281 (`0x004F0620`, `0x00493030`): everyone sees the charge model, even in fog; the digits
    /// only on ground the local player sees.
    // QUIRK D-281: kept
    #[test]
    fn charge_is_drawn_in_fog_but_digits_are_not() {
        let mut f = Fixture::new();
        // A foreign demolition expert: the active player 0 has no sight of the charge.
        assert_eq!(f.host.active_player, 0);
        let patriot = spawn(&mut f.host, PATRIOT, 1, (20, 20));
        f.host.state.fog =
            opennml_sim::fog::Fog::new(f.host.rules.map.dim_x, f.host.rules.map.dim_z);
        f.lay(patriot, ObjectId::NONE, [3000.0, 20.0, 2000.0], 0.0);
        while f.host.state.projectiles.list[0].remaining_ms > 5000 {
            f.step();
            assert_eq!(f.frame().len(), 1, "drawn in fog");
        }
        assert!(f.digits().is_empty(), "no digit in fog");
        // Seen ground: the next digit appears (the hidden ones count down anyway).
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        while f.digits().is_empty() {
            f.step();
        }
        assert_eq!(f.digits(), ["4"], "the 5 fell in fog");
        assert_eq!(f.host.state.projectiles.list[0].remaining_ms, 4800);
    }

    /// The target vanishes during the fuse time (ship sunk, building taken over, new ID). The
    /// charge stays at its last position, keeps counting, the explosion appears there; no jump to
    /// the origin.
    // QUIRK D-282: kept
    #[test]
    fn charge_stays_where_its_target_vanished() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        f.place(ship, [3100.0, 10.0, 1950.0], 0.5);
        let laid = [3000.0, 20.0, 2000.0];
        f.lay(patriot, ship, laid, 1.0);
        f.place(ship, [3200.0, 10.0, 1900.0], 0.9);
        f.step();
        let held = f.charge_world();
        let expected_pos =
            Vec3::new(3200.0, 10.0, 1900.0) + (Vec3::from(laid) - Vec3::new(3100.0, 10.0, 1950.0));
        assert_eq!(Vec3::from(held.translation), expected_pos);
        // The target goes.
        f.sim(|sim| opennml_sim::world::queue_destroy(sim, ship));
        let mut blasts = Vec::new();
        let mut digits = f.digits().len();
        while f.host.state.object(ship, false).is_some() && blasts.is_empty() {
            blasts.extend(f.step());
        }
        assert!(blasts.is_empty(), "target gone before the ignition");
        for _ in 0..30 {
            blasts.extend(f.step());
            assert_eq!(f.charge_world(), held, "frozen");
        }
        assert!(f.digits().len() > digits, "keeps counting");
        digits = f.digits().len();
        while blasts.is_empty() {
            blasts.extend(f.step());
            f.frame();
        }
        assert!(f.digits().len() > digits);
        assert_eq!(blasts, [expected_pos]);
    }

    /// D-282: after loading (or restart, `reset`) a charge found sits silently at the laying
    /// position without offset and rotation, the explosion there; the digits keep counting from
    /// the next boundary (`m_LastCount` was in the savegame).
    // QUIRK D-282: kept
    #[test]
    fn loaded_charge_sits_still_at_the_laying_point() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        f.place(ship, [3100.0, 10.0, 1950.0], 0.5);
        let laid = [3000.0, 20.0, 2000.0];
        f.lay(patriot, ship, laid, 1.0);
        for _ in 0..30 {
            f.step();
            f.frame();
        }
        // Loading: presentation new, no floating numbers.
        let loaded_digit = (f.host.state.projectiles.list[0].remaining_ms + 100) / 1000;
        assert!(loaded_digit < 10);
        f.load();
        let laid_world = object_world_matrix(Vec3::from(laid), Vec3::ZERO, Vec3::ONE);
        assert_eq!(f.charge_world(), laid_world, "first frame after loading");
        let track = f.world.charges().values().next().unwrap().clone();
        assert!(!track.follows);
        assert_eq!((track.offset, track.yaw_offset), (Vec3::ZERO, 0.0));
        // The target moves: the charge does not.
        let mut blasts = Vec::new();
        for k in 1..=10 {
            f.place(
                ship,
                [3100.0 + 10.0 * k as f32, 10.0, 1950.0],
                0.5 + 0.1 * k as f32,
            );
            blasts.extend(f.step());
            assert_eq!(f.charge_world(), laid_world);
        }
        // The digit at loading does not come again; then one per boundary, down to 0.
        while blasts.is_empty() {
            blasts.extend(f.step());
            f.frame();
        }
        let expected: Vec<String> = (0..loaded_digit).rev().map(|d| d.to_string()).collect();
        assert_eq!(f.digits(), expected);
        assert_eq!(blasts, [Vec3::from(laid)]);
    }

    /// Tracks, following, drawing and digits read only the state.
    // QUIRK D-282: kept
    #[test]
    fn charges_leave_the_checksum_alone() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        f.lay(patriot, ship, [3000.0, 20.0, 2000.0], 1.0);
        let mut reference = f.host.state.clone();
        loop {
            let sum = checksum(&f.host.state);
            assert_eq!(checksum(&reference), sum);
            opennml_sim::tick::step(&mut reference, &f.host.rules, &[]);
            // `step` and `frame` check the checksum around the presentation.
            let blasts = f.step();
            f.frame();
            if !blasts.is_empty() {
                break;
            }
        }
        assert_eq!(checksum(&reference), checksum(&f.host.state));
        assert!(!f.digits().is_empty());
    }

    // QUIRK D-237: deviation
    #[test]
    fn ground_effects_sit_on_the_terrain() {
        let height = |x: f32, z: f32| x * 0.5 + z * 0.25;
        let pos = [64.0, 0.0, 128.0];
        let lifted = [64.0, 64.0, 128.0];
        // The three paths of the events, also with another spelling of the path.
        for path in [
            "data/effects/explosion_medicineman.pef",
            "data/effects/storm_medicineman.pef",
            "data/effects/aufklären.pef",
            "Data\\Effects\\Aufklären.pef",
        ] {
            assert_eq!(ground_lift(path, pos, height), lifted, "{path}");
        }
        // Every other path stays unchanged (impact, dynamite, blood).
        for path in [
            "data/effects/einschlag_boden.pef",
            "data/effects/explosion_dynamite.pef",
            "data/effects/schmied_panzerung.pef",
        ] {
            assert_eq!(
                ground_lift(path, [64.0, 9.0, 128.0], height),
                [64.0, 9.0, 128.0]
            );
        }
        assert_eq!(GROUND_EFFECTS.len(), 3);
    }

    /// The paths the simulation reports are those of the list (otherwise the viewer never lifts).
    #[test]
    fn the_sim_events_use_the_ground_effect_paths() {
        let mut f = Fixture::new();
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        f.host.state.events.clear();
        f.sim(|sim| add_explosion(sim, patriot, [64.0, 0.0, 64.0]));
        let paths: Vec<_> = f
            .host
            .state
            .events
            .iter()
            .filter_map(|e| match e {
                opennml_sim::state::SimEvent::EffectAt { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(paths, [EXPLOSION_EFFECT]);
        assert_eq!(
            ground_lift(&paths[0], [64.0, 0.0, 64.0], |_, _| 3.0)[1],
            3.0
        );
    }

    /// In the viewer's step mode (`step N`, no frame until afterwards) the charge follows the target
    /// after every logic step; digit and explosion are at the position of the last step, not at
    /// that of the last drawn frame.
    // QUIRK D-282: kept
    #[test]
    fn charge_follows_its_target_through_steps_without_frames() {
        let mut f = Fixture::new();
        opennml_sim::fog::reveal_map(&mut f.host.state, -1);
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        let t0 = Vec3::new(3100.0, 10.0, 1950.0);
        f.place(ship, t0.to_array(), 0.25);
        let laid = [3000.0, 20.0, 2000.0];
        let id = f.lay(patriot, ship, laid, 0.75);
        f.frame();
        let offset = Vec3::from(laid) - t0;
        // No more frame until the explosion; the target moves on each step.
        let mut k = 0;
        let mut last = Vec3::from(laid);
        let mut blasts = Vec::new();
        while blasts.is_empty() {
            k += 1;
            let t = Vec3::new(3100.0 + 5.0 * k as f32, 10.0, 1950.0 - 2.0 * k as f32);
            f.place(ship, t.to_array(), 0.25 + 0.01 * k as f32);
            let digits = f.notices.floats.len();
            blasts = f.step();
            if !blasts.is_empty() {
                break;
            }
            let track = &f.world.charges()[&id];
            assert_eq!(track.pos, t + offset, "step {k}");
            last = track.pos;
            // A new digit stands above the position of this step (node 120 above the charge,
            // rotated by the yaw: height stays, x/z equal to the charge).
            if f.notices.floats.len() > digits {
                let n = f.notices.floats.last().unwrap();
                assert_eq!(n.position.y, last.y + 120.0);
                assert!(
                    (n.position.x - last.x).abs() < 1e-3 && (n.position.z - last.z).abs() < 1e-3
                );
            }
        }
        assert!(k > 40, "many steps without a frame");
        assert_ne!(last, Vec3::from(laid));
        // The explosion takes the position before the disappearance (last step), not that of the
        // explosion step.
        assert_eq!(blasts, [last]);
        assert!(f.world.charges().is_empty());
    }

    /// The flag `after_reset` is used up at once after loading; a charge laid afterwards follows
    /// its target.
    // QUIRK D-282: kept
    #[test]
    fn charge_laid_right_after_a_load_follows_its_target() {
        let mut f = Fixture::new();
        let patriot = spawn(&mut f.host, PATRIOT, 0, (20, 20));
        let ship = spawn(&mut f.host, CHIEF_TENT, 1, (24, 15));
        f.place(ship, [3100.0, 10.0, 1950.0], 0.5);
        f.load();
        let id = f.lay(patriot, ship, [3000.0, 20.0, 2000.0], 1.0);
        let track = &f.world.charges()[&id];
        assert!(track.follows, "laid after loading: follows");
        assert_eq!(track.yaw_offset, 0.5);
        // A charge already there at loading stays still.
        f.load();
        assert!(!f.world.charges()[&id].follows);
    }
}
