//! Permanent effects of ghost evocation, the ghost, banishing, animal evocation and the
//! auras (re/ghosts-banish 7, re/evoke-aura 7): presentation only, derived from the
//! module state of the simulation, following the pattern of [`crate::game::heal_effects::HealEffects`].
//!
//! Deviation from the original: the original attaches the effects on start messages and removes
//! them again on end messages or on abort (own effect IDs in the module). The reimplementation
//! keeps no effect keys in the state; the host derives what is to be
//! seen from the state every frame - that survives loading and reconnecting:
//!
//! - **Evoker hand and staff** `Geisterbeschwoerer_Hand.pef`, `..._Stab.pef`, both at the node
//!   `geisterhand01` (D-236; `m_StabDummyName` `geisterbeschwoerer01` stays unused), as long as
//!   an object with module 26 carries `EVOKING` (`0x41A` `0x00511340`/`0x005113DA`). **Hypothesis**:
//!   the original ends them only on abort; the reimplementation shows them until `0x31A`.
//! - **Animal evoker beam** `Tierbeschwoerer.pef` at the node `zauber01`, as long as an object with
//!   module 32 carries `EVOKING` (re/evoke-aura 7, init `0x00510770`); the target is the spawn position at
//!   height 0, set once on creation (`0x005103B0`/`0x005106A0`, D-277); every position at the staff
//!   measures the flight time again.
//! - **Suction beam** `Geist_angriff.pef` at the victim (+50, with object rotation), target the node
//!   `saugen01` of the ghost, as long as `Ghost.target` is set (`0x31B` `0x00515C00`, end `0x71B`
//!   `0x00515A60`; re/world-display W.4.2). The simulation sends `0x71B` when the victim dies
//!   or vanishes (via the attack end); until then the
//!   beam stays at the victim's last position. A new victim without a `0x71B` in between does not
//!   re-hang it (D-279).
//! - **Banishing spell** `radius_bannen.hef` as a handle effect and `bannspruch.pef` at every node
//!   `<320005>01`, `<320005>02`, ... up to the first missing one (`0x11F` `0x00502F40`), as long as
//!   module 31 has a timer `0x41F` (`Banish_RemoveEffects` `0x00503370` on `0x41F`, the
//!   abort deletes the group `0x80`).
//! - **Aura ring** 450003 of the type (`wolf.hef`, `baer.hef`, `wildkatze.hef`) as a handle effect,
//!   as long as [`influence::ring_shown`] holds (`GameObject_AddHandleEffect` on `0x22C`, removed on
//!   `0x32C`, slot 5 and in the destructor).
//!
//! Both beams are effects with a target (D-242): the particles fly to the target point ([`EffectTarget`],
//! re/world-display W.4.1). If a beam ends during the run, the sync only stops it
//! (`AttachedEffect_Stop`), and it runs out with its particles; [`AbilityEffects::reset`]
//! removes it immediately. The reimplementation does not reproduce the tick path
//! `AttachedEffect_UpdateTransform` (`0x004F37A0`) (D-278).
//!
//! Like the healing circle, the handle effects have the radius `floor(E/4) * SIZE_X` with `E` = sight of the
//! carrier (re/effects, section "HEF radius indicators"), not the banishing radius. Unlike the
//! healing circle, every viewer sees aura and banishing (no owner filter, A6). If the
//! effect world loses effects on a complete rebuild (change of `ParticlesQuality`) or with its
//! carrier, the next sync creates them again;
//! [`AbilityEffects::reset`] clears everything on loading and restart.

use std::collections::BTreeMap;

use glam::Vec3;
use opennml_effects::attached::{Anchor, EffectTarget, UNLIMITED_MS};
use opennml_effects::handle::HandleEffect;
use opennml_effects::world::EffectWorld;
use opennml_formats::hef::parse_hef;
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_sim::fx::{Fx, FxVec3};
use opennml_sim::id::ObjectId;
use opennml_sim::module::ghosts::{banish, evoke, ghost};
use opennml_sim::module::{ModuleKind, evoke_animal, exec_temp, influence};
use opennml_sim::object::{GameObject, status};
use opennml_sim::rules::Rules;
use opennml_sim::state::SimState;
use opennml_sim::world::grid_to_world;
use opennml_world::assets::ModelCache;

use crate::game::heal_effects::sight;
use crate::game::sim_host::SimHost;

/// `m_HandEffectName` of the ghost evoker (`+0x94`, re/ghosts-banish 1.2).
pub const HAND_EFFECT: &str = "data/effects/Geisterbeschwoerer_Hand.pef";
/// `m_StabEffectName` (`+0x98`).
pub const STAFF_EFFECT: &str = "data/effects/Geisterbeschwoerer_Stab.pef";
/// `m_HandDummyName` (`+0x9C`): node of **both** evoker effects (D-236).
pub const HAND_NODE: &str = "geisterhand01";
/// Beam of animal evocation (`+0x88`, re/evoke-aura 1.2).
pub const BEAM_EFFECT: &str = "data/effects/Tierbeschwoerer.pef";
/// `m_DummyName` of animal evocation (`+0x8C`).
pub const BEAM_NODE: &str = "zauber01";
/// `m_SuckEffectName` of the ghost (`+0x90`, re/ghosts-banish 1.4).
pub const SUCK_EFFECT: &str = "data/effects/Geist_angriff.pef";
/// `m_SuckEffectDummy` (`+0x94`).
pub const SUCK_NODE: &str = "saugen01";
/// Influence effect of module 44 (type value 450003).
const RING_VALUE: u32 = 450_003;

/// An effect that an object is to show.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Show {
    /// Game effect `path` at the node `node`.
    Node { path: String, node: String },
    /// Game effect `path` at every node `<prefix>01`, `<prefix>02`, ... up to the first missing one.
    Nodes { path: String, prefix: String },
    /// Handle effect `hef` with the radius `floor(sight/4) * SIZE_X`.
    Handle { hef: String, sight: i32 },
    /// Animal evoker beam [`BEAM_EFFECT`] at the node [`BEAM_NODE`] to the spawn position `spawn`
    /// (`EvokeAnimal::spawn`, fixed point 20.12 in tiles, raw: `Show` is the key of a
    /// `BTreeMap`). A new spawn position (casting again) is a new beam.
    Beam { spawn: [i32; 3] },
    /// Suction beam [`SUCK_EFFECT`] from the victim to the node [`SUCK_NODE`] of the ghost; without the victim in the
    /// key, so that a change of target without an end does not re-hang it (D-279).
    Suck,
}

/// An effect at the object `id` (several per object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub id: ObjectId,
    pub show: Show,
}

/// Created effects of a [`Wanted`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    /// Keys of the game effects (empty: no node or file not loadable, counts as done).
    Game(Vec<u32>),
    /// Key of the handle effect; `None`: file not loadable (no new attempt).
    Handle(Option<u32>),
}

impl Shown {
    /// All effects are still alive (or are done as not loadable).
    fn alive(&self, effects: &EffectWorld) -> bool {
        match self {
            Shown::Game(keys) => keys.iter().all(|&k| effects.game_effect(k).is_some()),
            Shown::Handle(key) => key.is_none_or(|k| effects.handle(k).is_some()),
        }
    }

    /// Stop game effects (`AttachedEffect_Stop`): no new particles, the effect lives until
    /// the last particle (end of a beam, `0x00515A60` or `0x220`/abort).
    fn stop(&self, effects: &mut EffectWorld) {
        match self {
            Shown::Game(keys) => {
                for &k in keys {
                    effects.stop_game_effect(k);
                }
            }
            Shown::Handle(_) => self.remove(effects),
        }
    }

    /// Remove effects (`KillOwnedEffect` or `GameObject_RemoveHandleEffect`).
    fn remove(&self, effects: &mut EffectWorld) {
        match self {
            Shown::Game(keys) => {
                for &k in keys {
                    effects.remove_game_effect(k);
                }
            }
            Shown::Handle(key) => {
                if let Some(k) = key {
                    effects.remove_handle(*k);
                }
            }
        }
    }
}

/// Permanent ability effects of a map.
#[derive(Debug, Default)]
pub struct AbilityEffects {
    /// Created effects per (ID, effect).
    shown: BTreeMap<(u32, Show), Shown>,
}

impl AbilityEffects {
    /// Remove all effects (loading, restart).
    pub fn reset(&mut self, effects: &mut EffectWorld) {
        for shown in std::mem::take(&mut self.shown).into_values() {
            shown.remove(effects);
        }
    }

    /// The running beams for the control channel (`world_display.beams`): per
    /// suction beam ([`SUCK_EFFECT`], owner the ghost) and animal evoker beam ([`BEAM_EFFECT`],
    /// owner the evoker) the ID, the path and the key of the game effect, sorted by
    /// (ID, effect). Stopped beams (ended, they only run out) are missing.
    pub fn beams(&self) -> Vec<(ObjectId, &'static str, u32)> {
        self.shown
            .iter()
            .filter_map(|((id, show), shown)| {
                let path = match show {
                    Show::Beam { .. } => BEAM_EFFECT,
                    Show::Suck => SUCK_EFFECT,
                    Show::Node { .. } | Show::Nodes { .. } | Show::Handle { .. } => return None,
                };
                match shown {
                    Shown::Game(keys) => Some(keys.iter().map(move |&k| (ObjectId(*id), path, k))),
                    Shown::Handle(_) => None,
                }
            })
            .flatten()
            .collect()
    }

    /// The effects that the objects are to show according to their module state, in table order
    /// and per object in the order hand, staff, beam, suction beam, banishing circle, banishing spell,
    /// ring; the same for every viewer. Reads only the state.
    pub fn wanted(state: &SimState, rules: &Rules) -> Vec<Wanted> {
        let mut out = Vec::new();
        for o in state.objects() {
            let mut push = |show| out.push(Wanted { id: o.id, show });
            let kind = |index| o.module(index).map(|m| &m.kind);
            let evoking = o.status & status::EVOKING != 0;
            if evoking && matches!(kind(evoke::INDEX), Some(ModuleKind::EvokeGhost(_))) {
                push(node(HAND_EFFECT, HAND_NODE));
                push(node(STAFF_EFFECT, HAND_NODE));
            }
            if evoking && let Some(ModuleKind::EvokeAnimal(e)) = kind(evoke_animal::INDEX) {
                push(Show::Beam { spawn: e.spawn });
            }
            // Suction target set (`0x31B` to `0x71B`); whether the victim still exists is decided only
            // when building.
            if let Some(ModuleKind::Ghost(g)) = kind(ghost::INDEX)
                && g.target != ObjectId::NONE
            {
                push(Show::Suck);
            }
            if let Some(m) = o.module(banish::INDEX)
                && let ModuleKind::Banish(b) = &m.kind
                && m.base.has_pending_timer(banish::msg::EFFECTS_OFF)
            {
                push(Show::Handle {
                    hef: banish::RADIUS_EFFECT.into(),
                    sight: sight(state, o),
                });
                push(Show::Nodes {
                    path: banish::SPELL_EFFECT.into(),
                    prefix: b.dummy.clone(),
                });
            }
            if let Some(m) = o.module(exec_temp::INDEX)
                && influence::ring_shown(m)
                && let Some(hef) = ring_hef(rules, o)
            {
                push(Show::Handle {
                    hef,
                    sight: sight(state, o),
                });
            }
        }
        out
    }

    /// Sync after a frame: create effects for each [`Wanted`] that has none yet (or
    /// lost them on rebuild), remove the others - only stop the beams ([`Show::Beam`],
    /// [`Show::Suck`]), they run out. `nodes` names the nodes `<prefix>01...` of the
    /// object model up to the first missing one, `carrier` the index of the object in the
    /// object list of the map, `cell` the tile size (`SIZE_X`) of the handle effects. A carrier not yet
    /// drawn (for the suction beam also a missing victim) is tried
    /// again in the next frame. Reads only the state.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        sim: &SimHost,
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
        nodes: &dyn Fn(ObjectId, &str) -> Vec<String>,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
        cell: f32,
    ) {
        let wanted = Self::wanted(&sim.state, &sim.rules);
        // End (`0x31A`/abort, `0x220`, `0x71B`, `0x41F`, ring off, object gone).
        self.shown.retain(|(id, show), shown| {
            if wanted.iter().any(|w| w.id.0 == *id && w.show == *show) {
                return true;
            }
            match show {
                Show::Beam { .. } | Show::Suck => shown.stop(effects),
                Show::Node { .. } | Show::Nodes { .. } | Show::Handle { .. } => {
                    shown.remove(effects)
                }
            }
            false
        });
        for w in wanted {
            let key = (w.id.0, w.show);
            if let Some(shown) = self.shown.get(&key) {
                if shown.alive(effects) {
                    continue;
                }
                // A part was lost with the rebuild: remove the rest, create anew.
                shown.remove(effects);
                self.shown.remove(&key);
            }
            let Some(at) = carrier(w.id) else {
                continue;
            };
            let shown = match &key.1 {
                Show::Node { path, node } => Shown::Game(
                    attach(effects, models, vfs, path, node, at)
                        .into_iter()
                        .collect(),
                ),
                Show::Nodes { path, prefix } => Shown::Game(
                    nodes(w.id, prefix)
                        .iter()
                        .filter_map(|n| attach(effects, models, vfs, path, n, at))
                        .collect(),
                ),
                Show::Handle { hef, sight } => {
                    let moving = sim
                        .state
                        .object(w.id, false)
                        .is_some_and(|o| o.has_module(opennml_sim::module::walk::INDEX, true));
                    Shown::Handle(add_handle(effects, vfs, hef, *sight, at, moving, cell))
                }
                Show::Beam { spawn } => Shown::Game(
                    attach_beam(effects, models, vfs, at, *spawn, sim.rules.map.cell)
                        .into_iter()
                        .collect(),
                ),
                Show::Suck => {
                    let Some(victim) = suck_victim(&sim.state, w.id).and_then(carrier) else {
                        continue;
                    };
                    Shown::Game(
                        attach_suck(effects, models, vfs, victim, at)
                            .into_iter()
                            .collect(),
                    )
                }
            };
            self.shown.insert(key, shown);
        }
    }
}

/// Suction target of the ghost `ghost` (`Ghost.target`), if set.
fn suck_victim(state: &SimState, ghost: ObjectId) -> Option<ObjectId> {
    let o = state.object(ghost, false)?;
    match &o.module(ghost::INDEX)?.kind {
        ModuleKind::Ghost(g) if g.target != ObjectId::NONE => Some(g.target),
        _ => None,
    }
}

/// Animal evoker beam like `0x005103B0`: `Tierbeschwoerer.pef` at the node `zauber01` of the
/// evoker `at`, target the spawn position `spawn` (fixed point 20.12) as the world point
/// `(spawn * SIZE) * 2^-12` with height 0 (`0x005106A0`, `grid_to_world`, D-277), set before the
/// first position (`AttachedEffect_SetTarget` `0x004F3660`). The measurement against the map origin is
/// only an intermediate value: the first position at the staff measures again (`Effect_SetPosition` `0x0057C4E0`)
/// before a particle is created.
fn attach_beam(
    effects: &mut EffectWorld,
    models: &mut ModelCache,
    vfs: &Vfs,
    at: usize,
    spawn: [i32; 3],
    cell: [i32; 3],
) -> Option<u32> {
    let key = attach(effects, models, vfs, BEAM_EFFECT, BEAM_NODE, at)?;
    let w = grid_to_world(FxVec3::new(Fx(spawn[0]), Fx(spawn[1]), Fx(spawn[2])), cell);
    let z = Vec3::new(w[0], 0.0, w[2]);
    effects.set_game_effect_target(key, EffectTarget::Point(z), None);
    Some(key)
}

/// Suction beam like `0x00515C00`: `Geist_angriff.pef` at the victim `victim` (+50 with object rotation,
/// `AttachToObject`), target the node `saugen01` of the ghost `ghost`, "follow"
/// (`AttachedEffect_SetTargetNode` `0x004F3DE0`); [`EffectWorld::step`] measures the flight time every frame.
fn attach_suck(
    effects: &mut EffectWorld,
    models: &mut ModelCache,
    vfs: &Vfs,
    victim: usize,
    ghost: usize,
) -> Option<u32> {
    let key = effects.add_game_effect(
        vfs,
        models,
        SUCK_EFFECT,
        Anchor::Object {
            offset: Vec3::new(0.0, 50.0, 0.0),
            with_rotation: true,
        },
        Some(victim),
        1.0,
        0,
        UNLIMITED_MS,
    );
    let Some(key) = key else {
        log::warn!("effect {SUCK_EFFECT} not loadable");
        return None;
    };
    let target = EffectTarget::Node {
        node: SUCK_NODE.into(),
        follow: true,
    };
    effects.set_game_effect_target(key, target, Some(ghost));
    Some(key)
}

fn node(path: &str, node: &str) -> Show {
    Show::Node {
        path: path.into(),
        node: node.into(),
    }
}

/// Influence effect 450003 of the type (`rules.types[guid].module(44).values[450003]`); `None` if
/// the value is missing or empty.
fn ring_hef(rules: &Rules, o: &GameObject) -> Option<String> {
    let hef = rules
        .types
        .get(&o.guid)?
        .module(exec_temp::INDEX as u32)?
        .values
        .get(&RING_VALUE)?
        .trim();
    (!hef.is_empty()).then(|| hef.to_string())
}

/// Game effect `path` at the node `node` of the carrier `at` (duration `0x7FFFFFFF`).
pub(crate) fn attach(
    effects: &mut EffectWorld,
    models: &mut ModelCache,
    vfs: &Vfs,
    path: &str,
    node: &str,
    at: usize,
) -> Option<u32> {
    let key = effects.add_game_effect(
        vfs,
        models,
        path,
        Anchor::Node(node.into()),
        Some(at),
        1.0,
        0,
        UNLIMITED_MS,
    );
    if key.is_none() {
        log::warn!("effect {path} not loadable");
    }
    key
}

/// Handle effect `hef` at the carrier `at` with the radius `floor(sight/4) * cell` (re/effects,
/// section "HEF radius indicators").
fn add_handle(
    effects: &mut EffectWorld,
    vfs: &Vfs,
    hef: &str,
    sight: i32,
    at: usize,
    moving: bool,
    cell: f32,
) -> Option<u32> {
    let path = normalize_path(hef);
    match vfs.read(&path) {
        Ok(bytes) => {
            let records = parse_hef(&decode_cp1252(&bytes));
            let radius = (sight / 4) as f32 * cell;
            Some(effects.add_handle(at, moving, HandleEffect::new(&path, records, radius)))
        }
        Err(e) => {
            log::warn!("Handle effect {path} not loadable: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testworld::{ANIMAL_EVOKER, ARCHER, GHOST, GHOST_EVOKER, WOLF, host, spawn};
    use glam::Vec3;
    use opennml_effects::attached::EffectTarget;
    use opennml_effects::world::{ObjectFrame, ObjectView};
    use opennml_rda::testutil::{TestFile, build_rda};
    use opennml_sim::message::Message;
    use opennml_sim::messaging::send;
    use opennml_sim::module::spell::MANA;
    use opennml_sim::save::checksum;
    use opennml_sim::sim::Sim;
    use opennml_sim::world::grid_to_world;
    use opennml_world::camera::{game_projection, look_at_lh};
    use opennml_world::math::{game_rotation_x, game_rotation_y, object_world_matrix};
    use opennml_world::visibility::Frustum;

    /// Time mode, one effect per 100 ms around the carrier.
    const HEF: &str = "PARTICLEMOVE\r\nARCRANGE 0 360\r\nDURATION 1000\r\nTIMEOUT 100\r\n\
                       EFFECT data/effects/ring.pef\r\nEFFECTDURATION 500\r\n/PARTICLEMOVE\r\n";
    /// Loop with 10 particles per s; `MOVEMENTTIME 1`, otherwise the target flight becomes NaN.
    const PEF: &str = "EFFECT\r\n{\r\n\tMOVEMENTTIME 1.000000\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
    const WOLF_HEF: &str = "data/effects/wolf.hef";
    const BEAR_HEF: &str = "data/effects/baer.hef";

    /// Archive with all effects of this module.
    fn archive() -> (tempfile::TempDir, Vfs) {
        let pefs = [
            HAND_EFFECT,
            STAFF_EFFECT,
            BEAM_EFFECT,
            SUCK_EFFECT,
            banish::SPELL_EFFECT,
            "data/effects/ring.pef",
        ];
        let hefs = [banish::RADIUS_EFFECT, WOLF_HEF, BEAR_HEF];
        let files: Vec<TestFile> = pefs
            .iter()
            .map(|p| (p, PEF))
            .chain(hefs.iter().map(|p| (p, HEF)))
            .map(|(path, data)| TestFile {
                path,
                data: data.as_bytes(),
                compress: false,
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    /// Node `saugen01` in object space (every model of the test world).
    const SUCK_LOCAL: Vec3 = Vec3::new(0.0, 80.0, 10.0);
    /// Node `zauber01` in object space.
    const STAFF_LOCAL: Vec3 = Vec3::new(10.0, 120.0, 0.0);

    /// Objects of the test world for [`EffectWorld::step`]: index = ID (like the carrier in
    /// [`Fixture::sync`]), position from the grid, rotation from the state; a missing object
    /// stands unseen at the origin.
    struct View<'a> {
        host: &'a SimHost,
    }

    impl ObjectView for View<'_> {
        fn frame(&self, object: usize) -> ObjectFrame {
            match self.host.state.object(ObjectId(object as u32), false) {
                Some(o) => ObjectFrame {
                    position: Vec3::from(grid_to_world(o.grid, self.host.rules.map.cell)),
                    rotation: Vec3::from(o.rotation),
                    visible: true,
                },
                None => ObjectFrame {
                    position: Vec3::ZERO,
                    rotation: Vec3::ZERO,
                    visible: false,
                },
            }
        }

        fn node(&self, _object: usize, name: &str) -> Option<Vec3> {
            match name {
                SUCK_NODE => Some(SUCK_LOCAL),
                BEAM_NODE => Some(STAFF_LOCAL),
                _ => None,
            }
        }
    }

    fn frustum() -> Frustum {
        let view = look_at_lh(Vec3::new(0.0, 500.0, -500.0), Vec3::ZERO, Vec3::Y);
        Frustum::from_view_proj(game_projection(0.8, 1.0, 700.0) * view)
    }

    /// Position of the suction beam above the victim: `attach_to_object(t, (0, 50, 0), with rotation)`
    /// (re/world-display W.4.2), calculated like `AttachedEffect::placement`.
    fn above_victim(frame: ObjectFrame) -> Vec3 {
        frame.position
            + game_rotation_y(frame.rotation.y)
                * (game_rotation_x(frame.rotation.x) * Vec3::new(0.0, 50.0, 0.0))
    }

    /// Node `local` of the object in world position (like `EffectWorld::step`).
    fn node_in_world(frame: ObjectFrame, local: Vec3) -> Vec3 {
        object_world_matrix(frame.position, frame.rotation, Vec3::ONE).transform_point3(local)
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        ability: AbilityEffects,
        effects: EffectWorld,
        models: ModelCache,
    }

    impl Fixture {
        fn new() -> Self {
            let (dir, vfs) = archive();
            Self {
                _dir: dir,
                vfs,
                host: host(),
                ability: AbilityEffects::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
            }
        }

        /// One frame: sync, checksum unchanged. Every model has the nodes
        /// `<prefix>01` and `<prefix>02`; the index of an object is its ID, a
        /// missing one has none (like `object_of` of the map).
        fn sync(&mut self) {
            let sum = checksum(&self.host.state);
            let state = &self.host.state;
            self.ability.sync(
                &self.host,
                &mut self.effects,
                &mut self.models,
                &self.vfs,
                &|_, prefix| (1..=2).map(|i| format!("{prefix}{i:02}")).collect(),
                &|id| state.object(id, false).map(|_| id.0 as usize),
                128.0,
            );
            assert_eq!(
                checksum(&self.host.state),
                sum,
                "presentation changes nothing"
            );
        }

        /// One logic step of the effect world of `ms` game time (checksum unchanged).
        fn step(&mut self, ms: u32) {
            let sum = checksum(&self.host.state);
            let view = View { host: &self.host };
            self.effects
                .step(ms, &frustum(), &view, &self.vfs, &mut self.models);
            assert_eq!(checksum(&self.host.state), sum, "effects change nothing");
        }

        /// Position and rotation of the object `id`, as the effect world sees them.
        fn frame(&self, id: ObjectId) -> ObjectFrame {
            View { host: &self.host }.frame(id.0 as usize)
        }

        /// Key of the only game effect for (`id`, `show`).
        fn key(&self, id: ObjectId, show: &Show) -> Option<u32> {
            match self.ability.shown.get(&(id.0, show.clone())) {
                Some(Shown::Game(keys)) => {
                    assert_eq!(keys.len(), 1);
                    Some(keys[0])
                }
                Some(other) => panic!("not a game effect: {other:?}"),
                None => None,
            }
        }

        /// Frames of 20 ms each until the effect `key` is gone; returns: number of frames.
        fn run_out(&mut self, key: u32) -> usize {
            let mut frames = 0;
            while self.effects.game_effect(key).is_some() {
                self.step(20);
                frames += 1;
                assert!(frames < 10_000, "runs out");
            }
            frames
        }

        /// Wanted effects of the object `id` (checksum unchanged).
        fn wanted(&self, id: ObjectId) -> Vec<Show> {
            let sum = checksum(&self.host.state);
            let out = AbilityEffects::wanted(&self.host.state, &self.host.rules)
                .into_iter()
                .filter(|w| w.id == id)
                .map(|w| w.show)
                .collect();
            assert_eq!(checksum(&self.host.state), sum, "query changes nothing");
            out
        }

        /// Living game effects at the object `id`: (node, path).
        fn attached(&self, id: ObjectId) -> Vec<(String, String)> {
            self.ability
                .shown
                .iter()
                .filter(|((o, _), _)| *o == id.0)
                .flat_map(|((_, show), shown)| {
                    let path = match show {
                        Show::Node { path, .. } | Show::Nodes { path, .. } => path.as_str(),
                        Show::Beam { .. } => BEAM_EFFECT,
                        Show::Suck => SUCK_EFFECT,
                        Show::Handle { .. } => return Vec::new(),
                    };
                    match shown {
                        Shown::Game(keys) => keys
                            .iter()
                            .filter_map(|&k| self.effects.game_effect(k))
                            .map(|a| (a.node().unwrap_or("").to_string(), path.to_string()))
                            .collect::<Vec<_>>(),
                        Shown::Handle(_) => Vec::new(),
                    }
                })
                .collect()
        }

        /// Living handle effects at the object `id`: (path, radius).
        fn handles(&self, id: ObjectId) -> Vec<(String, f32)> {
            self.ability
                .shown
                .iter()
                .filter(|((o, _), _)| *o == id.0)
                .filter_map(|(_, shown)| match shown {
                    Shown::Handle(Some(k)) => self.effects.handle(*k),
                    _ => None,
                })
                .map(|h| (h.path.clone(), h.radius))
                .collect()
        }

        fn status(&mut self, id: ObjectId, bits: u32, on: bool) {
            self.host
                .state
                .object_mut(id, false)
                .unwrap()
                .set_status_flags(bits, on);
        }

        fn send(&mut self, id: ObjectId, code: u32, args: &[i32]) -> i32 {
            let rules = self.host.rules.clone();
            let mut sim = Sim::new(&mut self.host.state, &rules);
            send(&mut sim, id, &Message::new(code, args))
        }

        fn run_ticks(&mut self, n: usize) {
            for _ in 0..2 * n {
                opennml_sim::tick::step(&mut self.host.state, &self.host.rules, &[]);
                self.sync();
            }
        }

        fn set_target(&mut self, g: ObjectId, target: ObjectId) {
            let o = self.host.state.object_mut(g, false).unwrap();
            let Some(ModuleKind::Ghost(gh)) = o.module_mut(ghost::INDEX).map(|m| &mut m.kind)
            else {
                panic!("not a ghost");
            };
            gh.target = target;
        }

        fn set_spawn(&mut self, id: ObjectId, spawn: [i32; 3]) {
            let o = self.host.state.object_mut(id, false).unwrap();
            let Some(ModuleKind::EvokeAnimal(e)) =
                o.module_mut(evoke_animal::INDEX).map(|m| &mut m.kind)
            else {
                panic!("not an animal evoker");
            };
            e.spawn = spawn;
        }

        fn pending(&self, id: ObjectId, index: usize, code: u32) -> bool {
            self.host
                .state
                .object(id, false)
                .and_then(|o| o.module(index))
                .is_some_and(|m| m.base.has_pending_timer(code))
        }
    }

    fn pair(node: &str, path: &str) -> (String, String) {
        (node.to_string(), path.to_string())
    }

    // QUIRK D-236, D-242: kept
    #[test]
    fn effects_follow_the_module_state() {
        let mut f = Fixture::new();
        let evoker = spawn(&mut f.host, GHOST_EVOKER, 0, (20, 20));
        let animals = spawn(&mut f.host, ANIMAL_EVOKER, 1, (30, 20));
        let g = spawn(&mut f.host, GHOST, 0, (20, 26));
        let victim = spawn(&mut f.host, ARCHER, 1, (22, 26));
        f.sync();
        assert!(f.ability.shown.is_empty(), "rest: nothing");
        assert_eq!(f.effects.counts().world, 0);

        // Evoking on (`EVOKING`): hand and staff at `geisterhand01` (D-236), the beam of
        // animal evocation at `zauber01` with the spawn position as target - also for the foreign
        // evoker.
        f.status(evoker, status::EVOKING, true);
        f.status(animals, status::EVOKING, true);
        assert_eq!(
            f.wanted(evoker),
            [node(HAND_EFFECT, HAND_NODE), node(STAFF_EFFECT, HAND_NODE)]
        );
        assert_eq!(f.wanted(animals), [Show::Beam { spawn: [0; 3] }]);
        f.sync();
        assert_eq!(
            f.attached(evoker),
            [pair(HAND_NODE, HAND_EFFECT), pair(HAND_NODE, STAFF_EFFECT)]
        );
        assert_eq!(f.attached(animals), [pair(BEAM_NODE, BEAM_EFFECT)]);
        assert_eq!(f.effects.counts().world, 3);
        // Again every frame: no duplicates.
        f.sync();
        assert_eq!(f.effects.counts().world, 3);
        // Complete rebuild of the effect world: lost and created anew.
        f.effects.clear();
        f.sync();
        assert_eq!(f.effects.counts().world, 3);
        // Evoking off (`0x31A`, abort): hand and staff gone, the beam stopped
        // (`AttachedEffect_Stop`); it lives until the last particle (here without particles: until
        // the next game tick).
        f.status(evoker, status::EVOKING, false);
        f.status(animals, status::EVOKING, false);
        f.sync();
        assert!(f.wanted(evoker).is_empty() && f.wanted(animals).is_empty());
        assert!(f.ability.shown.is_empty());
        assert_eq!(f.effects.counts().world, 1);
        f.step(100);
        assert_eq!(f.effects.counts().world, 0);

        // Suction target set: beam at the victim (+50) with the node `saugen01` of the ghost as
        // target; `NONE` (`0x71B`): stopped, gone with the last particle.
        f.set_target(g, victim);
        assert_eq!(f.wanted(g), [Show::Suck]);
        f.sync();
        assert_eq!(f.attached(g), [pair("", SUCK_EFFECT)]);
        f.set_target(g, ObjectId::NONE);
        f.sync();
        assert!(f.wanted(g).is_empty());
        assert!(f.ability.shown.is_empty());
        assert_eq!(f.effects.counts().world, 1);
        f.step(100);
        assert_eq!(f.effects.counts().world, 0);
        // The beam applies as long as `Ghost.target` is set: also for a dead victim and after
        // the change of owner to slot 8 (new ID); the simulation sends `0x71B` via the attack end.
        f.set_target(g, victim);
        f.send(victim, 0x110, &[]);
        assert_ne!(
            f.host.state.object(victim, false).unwrap().status & status::DIED,
            0
        );
        assert_eq!(f.wanted(g), [Show::Suck]);
        f.sync();
        assert_eq!(f.effects.counts().world, 1);
        for _ in 0..10 {
            if f.host.state.object(victim, false).is_none() {
                break;
            }
            f.run_ticks(1);
        }
        assert!(f.host.state.object(victim, false).is_none(), "new ID");
        f.sync();
        assert_eq!(f.wanted(g), [Show::Suck]);
        assert_eq!(f.effects.counts().world, 1);
        f.set_target(g, ObjectId::NONE);
        f.sync();
        f.step(100);
        assert_eq!(f.effects.counts().world, 0);

        // Banishing: with the timer `0x41F` the banishing circle (radius floor(52/4) * 128) and `bannspruch.pef`
        // at `geisterhand01`, `geisterhand02`; without it nothing.
        f.host
            .state
            .object_mut(evoker, false)
            .unwrap()
            .attributes
            .set_current_fraction(MANA, 100_000);
        assert!(!f.pending(evoker, banish::INDEX, banish::msg::EFFECTS_OFF));
        assert!(f.wanted(evoker).is_empty());
        f.send(evoker, banish::msg::START, &[547]);
        assert!(f.pending(evoker, banish::INDEX, banish::msg::EFFECTS_OFF));
        assert_eq!(
            f.wanted(evoker),
            [
                Show::Handle {
                    hef: banish::RADIUS_EFFECT.into(),
                    sight: 52,
                },
                Show::Nodes {
                    path: banish::SPELL_EFFECT.into(),
                    prefix: "geisterhand".into(),
                },
            ]
        );
        f.sync();
        assert_eq!(
            f.handles(evoker),
            [(banish::RADIUS_EFFECT.to_string(), 13.0 * 128.0)]
        );
        assert_eq!(
            f.attached(evoker),
            [
                pair("geisterhand01", banish::SPELL_EFFECT),
                pair("geisterhand02", banish::SPELL_EFFECT)
            ]
        );
        // Complete rebuild: circle and spell anew.
        f.effects.clear();
        f.sync();
        assert_eq!(f.handles(evoker).len(), 1);
        assert_eq!(f.attached(evoker).len(), 2);
        // `0x41F` (N+44; `send` before the first tick acts like a command in tick N = 1):
        // gone.
        let mut ticks = 0;
        while f.pending(evoker, banish::INDEX, banish::msg::EFFECTS_OFF) {
            assert!(!f.handles(evoker).is_empty());
            f.run_ticks(1);
            ticks += 1;
            assert!(ticks <= 50, "0x41F arrives");
        }
        assert_eq!(ticks, 45, "0x41F in N+44");
        assert!(f.wanted(evoker).is_empty());
        assert!(f.handles(evoker).is_empty() && f.attached(evoker).is_empty());
        assert!(f.ability.shown.is_empty());

        // Aura: before the first aura tick (only the timer `0x42C`), running, gone after death.
        let wolf = spawn(&mut f.host, WOLF, 0, (40, 40));
        assert!(f.wanted(wolf).is_empty(), "without influence no ring");
        f.send(wolf, 0x22C, &[0]);
        assert!(f.pending(wolf, exec_temp::INDEX, 0x42C));
        let ring = Show::Handle {
            hef: WOLF_HEF.into(),
            sight: 20,
        };
        assert_eq!(f.wanted(wolf), std::slice::from_ref(&ring));
        f.sync();
        assert_eq!(f.handles(wolf), [(WOLF_HEF.to_string(), 5.0 * 128.0)]);
        f.run_ticks(10);
        assert_eq!(f.wanted(wolf), [ring]);
        assert_eq!(f.handles(wolf).len(), 1);
        f.send(wolf, 0x110, &[]);
        for _ in 0..10 {
            if f.host.state.object(wolf, false).is_none() {
                break;
            }
            f.run_ticks(1);
        }
        assert!(f.host.state.object(wolf, false).is_none(), "new ID");
        f.sync();
        assert!(
            AbilityEffects::wanted(&f.host.state, &f.host.rules).is_empty(),
            "the carcass shows no ring"
        );
        assert!(f.ability.shown.is_empty());

        // `reset` clears what is running.
        f.status(evoker, status::EVOKING, true);
        f.sync();
        assert_eq!(f.effects.counts().world, 2);
        f.ability.reset(&mut f.effects);
        assert!(f.ability.shown.is_empty());
        assert_eq!(f.effects.counts().world, 0);
    }

    /// Ghost at (20, 26), victim at (22, 26), suction target set (`0x31B`), one sync.
    fn sucking() -> (Fixture, ObjectId, ObjectId) {
        let mut f = Fixture::new();
        let g = spawn(&mut f.host, GHOST, 0, (20, 26));
        let victim = spawn(&mut f.host, ARCHER, 1, (22, 26));
        f.set_target(g, victim);
        f.sync();
        (f, g, victim)
    }

    // QUIRK D-242: kept
    #[test]
    fn suck_beam_flies_from_the_victim_to_the_ghost_node() {
        let (mut f, g, victim) = sucking();
        let key = f.key(g, &Show::Suck).expect("suction beam");
        let a = f.effects.game_effect(key).unwrap();
        // `0x00515C00`: carrier = victim + 50 with object rotation, target = node `saugen01` of the
        // ghost, "follow" (re/world-display W.4.2, D-242).
        assert_eq!(
            a.anchor,
            Anchor::Object {
                offset: Vec3::new(0.0, 50.0, 0.0),
                with_rotation: true,
            }
        );
        assert_eq!(
            a.target,
            Some(EffectTarget::Node {
                node: SUCK_NODE.into(),
                follow: true,
            })
        );
        assert_eq!(a.effect.target(), None, "node target not until the frame");
        // First frame in the render list: target before position; `Effect_SetPosition` (`0x0057C4E0`) measures
        // again immediately with the new position, even before the first particle:
        // `T = |saugen01 - (victim + 50)| / 128` (D-280 not effective in the original).
        f.step(100);
        let z = node_in_world(f.frame(g), SUCK_LOCAL);
        let start = above_victim(f.frame(victim));
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.target(), Some(z));
        assert_eq!(e.flight_s(), (z - start).length() / 128.0);
        assert!(e.flight_s() < 3.0, "short flight: {}", e.flight_s());
        assert_eq!(e.placement().position, start);
        assert_eq!(e.placement().yaw, f.frame(g).rotation.y, "yaw of the ghost");
        // Every further frame likewise.
        f.step(20);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.flight_s(), (z - start).length() / 128.0);
        assert!(e.particle_count() > 0, "particles fly");
        // The ghost walks: the target follows it every frame.
        f.host.state.object_mut(g, false).unwrap().grid.x.0 += 4096;
        f.step(20);
        let z2 = node_in_world(f.frame(g), SUCK_LOCAL);
        assert_ne!(z2, z);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.target(), Some(z2));
        assert_eq!(e.flight_s(), (z2 - start).length() / 128.0);
    }

    // QUIRK D-279: kept
    #[test]
    fn suck_beam_is_not_rehung_on_a_new_target() {
        let (mut f, g, victim) = sucking();
        let other = spawn(&mut f.host, ARCHER, 1, (24, 30));
        let key = f.key(g, &Show::Suck).unwrap();
        f.step(100);
        // New target without a `0x71B` in between (counter-attack, target search): the same beam stays at the
        // old victim (`0x00515C8C`, D-279).
        f.set_target(g, other);
        f.sync();
        f.step(20);
        assert_eq!(f.key(g, &Show::Suck), Some(key));
        assert_eq!(f.effects.counts().world, 1);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.placement().position, above_victim(f.frame(victim)));
        // Only after the end a new beam at the new victim; the old one runs out.
        f.set_target(g, ObjectId::NONE);
        f.sync();
        f.set_target(g, other);
        f.sync();
        let new = f.key(g, &Show::Suck).unwrap();
        assert_ne!(new, key);
        assert_eq!(f.effects.counts().world, 2);
        f.step(100);
        let e = &f.effects.game_effect(new).unwrap().effect;
        assert_eq!(e.placement().position, above_victim(f.frame(other)));
    }

    // QUIRK D-242: kept
    #[test]
    fn suck_beam_runs_out_after_the_end() {
        let (mut f, g, _victim) = sucking();
        let key = f.key(g, &Show::Suck).unwrap();
        f.step(100);
        // Even the particle of the first frame flies measured against the position above the victim
        // (D-280 not effective in the original): the flight time stays the same from the start.
        let first = f.effects.game_effect(key).unwrap().effect.flight_s();
        for _ in 0..10 {
            f.step(20);
        }
        let flight = f.effects.game_effect(key).unwrap().effect.flight_s();
        assert_eq!(first, flight);
        assert!(f.effects.game_effect(key).unwrap().effect.particle_count() > 0);
        // `0x71B` (`0x00515A60`): `Ghost.target = NONE` -> `Stop`; no new particles, the
        // effect lives until the last particle is dead.
        f.send(g, ghost::msg::STOP_SUCK, &[]);
        assert!(f.wanted(g).is_empty());
        f.sync();
        assert!(f.ability.shown.is_empty());
        let a = f.effects.game_effect(key).expect("runs out");
        assert!(!a.effect.is_active());
        assert!(a.effect.particle_count() > 0);
        let frames = f.run_out(key);
        // Run-out ~ flight time (in the original ~ 1.5 s): the most recently started particle (at most
        // one emission interval of 0.1 s before the end) lives its flight time, the game tick
        // deletes the effect afterward (<= 100 ms).
        let left = frames as f32 * 0.02;
        assert!(
            (flight - 0.15..=flight + 0.15).contains(&left),
            "{frames} frames at T = {flight}"
        );
        assert!(left < 3.0, "no long run-out: {left} s");
        assert_eq!(f.effects.counts().world, 0);
    }

    /// The running beams for the control channel - suction beam at the ghost, beam at the
    /// animal evoker, each with the key of the game effect; hand and staff do not count. Stopped
    /// (running out) beams are missing, even if their effect is still alive.
    #[test]
    fn beams_list_the_running_suck_and_evoker_beams() {
        let mut f = Fixture::new();
        let evoker = spawn(&mut f.host, GHOST_EVOKER, 0, (20, 20));
        let g = spawn(&mut f.host, GHOST, 0, (20, 26));
        let animals = spawn(&mut f.host, ANIMAL_EVOKER, 1, (30, 20));
        let victim = spawn(&mut f.host, ARCHER, 1, (22, 26));
        f.sync();
        assert!(f.ability.beams().is_empty());
        f.status(evoker, status::EVOKING, true);
        f.status(animals, status::EVOKING, true);
        f.set_target(g, victim);
        f.sync();
        let suck = f.key(g, &Show::Suck).unwrap();
        let beam = f.key(animals, &Show::Beam { spawn: [0; 3] }).unwrap();
        // Sorted by the raw ID (the foreign evoker comes first here).
        let mut expected = vec![(g, SUCK_EFFECT, suck), (animals, BEAM_EFFECT, beam)];
        expected.sort_by_key(|b| b.0.0);
        assert_eq!(f.ability.beams(), expected);
        f.status(animals, status::EVOKING, false);
        f.set_target(g, ObjectId::NONE);
        f.sync();
        assert!(f.effects.game_effect(suck).is_some(), "still running out");
        assert!(f.ability.beams().is_empty());
    }

    // QUIRK D-242, D-277: kept
    // QUIRK D-278: deviation
    #[test]
    fn evoker_beam_measures_its_flight_against_the_staff() {
        let mut f = Fixture::new();
        let animals = spawn(&mut f.host, ANIMAL_EVOKER, 0, (30, 20));
        // Spawn position tile (32, 21) next to the evoker, in fixed point 20.12 (`m_SpawnPos`,
        // `0x0050FF10`).
        let spawn_at = [32 << 12, 0, 21 << 12];
        f.set_spawn(animals, spawn_at);
        f.status(animals, status::EVOKING, true);
        assert_eq!(f.wanted(animals), [Show::Beam { spawn: spawn_at }]);
        f.sync();
        let key = f.key(animals, &Show::Beam { spawn: spawn_at }).unwrap();
        let a = f.effects.game_effect(key).unwrap();
        assert_eq!(a.anchor, Anchor::Node(BEAM_NODE.into()));
        // `0x005106A0`: target `(spawn * SIZE) * 2^-12` with height 0 (D-277), set on creation.
        // It is measured immediately against the position (0, 0, 0), but that is only an intermediate value: no
        // particle has been created yet.
        let z = Vec3::new(4096.0, 0.0, 2688.0);
        assert_eq!(a.target, Some(EffectTarget::Point(z)));
        assert_eq!(a.effect.target(), Some(z));
        assert_eq!(a.effect.flight_s(), z.length() / 128.0);
        assert_eq!(a.effect.particle_count(), 0);
        // First frame: the position at the staff node measures again (`Effect_SetPosition` `0x0057C4E0`) before
        // a particle is created; no long flight (D-277: flight-time part not effective in the original).
        f.step(100);
        let staff = node_in_world(f.frame(animals), STAFF_LOCAL);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.placement().position, staff);
        assert_eq!(e.target(), Some(z), "fixed point, height 0");
        assert_eq!(e.flight_s(), (z - staff).length() / 128.0);
        assert!(e.flight_s() < 4.0, "short flight: {}", e.flight_s());
        assert!(e.particle_count() > 0);
        f.step(20);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.flight_s(), (z - staff).length() / 128.0);
        // New spawn position (casting again after abort): old beam stopped, new one created.
        let again = [33 << 12, 0, 21 << 12];
        f.set_spawn(animals, again);
        f.sync();
        let new = f.key(animals, &Show::Beam { spawn: again }).unwrap();
        assert_ne!(new, key);
        assert!(!f.effects.game_effect(key).unwrap().effect.is_active());
        // End (`0x220`/abort): stopped, not removed.
        f.status(animals, status::EVOKING, false);
        f.sync();
        assert!(f.ability.shown.is_empty());
        assert!(!f.effects.game_effect(new).unwrap().effect.is_active());
    }

    // QUIRK D-242, D-279: kept
    #[test]
    fn suck_beam_is_rebuilt_after_load() {
        let (mut f, g, victim) = sucking();
        f.step(100);
        // Loading/restart: `reset` removes immediately (no running out), the next sync rebuilds
        // from the state (state of the loaded `Ghost.target`).
        f.ability.reset(&mut f.effects);
        assert_eq!(f.effects.counts().world, 0);
        f.effects = EffectWorld::new();
        f.ability = AbilityEffects::default();
        f.sync();
        let key = f.key(g, &Show::Suck).expect("rebuilt");
        f.step(100);
        let e = &f.effects.game_effect(key).unwrap().effect;
        assert_eq!(e.placement().position, above_victim(f.frame(victim)));
        assert_eq!(e.target(), Some(node_in_world(f.frame(g), SUCK_LOCAL)));
        // Complete rebuild of the effect world: likewise.
        f.effects.clear();
        f.sync();
        assert!(f.key(g, &Show::Suck).is_some_and(|k| k != key));
        assert_eq!(f.effects.counts().world, 1);
        // If the victim is missing during the build, no beam is created; tried again every frame.
        f.ability.reset(&mut f.effects);
        f.set_target(g, ObjectId(9_999));
        f.sync();
        assert_eq!(f.wanted(g), [Show::Suck]);
        assert!(f.ability.shown.is_empty());
        assert_eq!(f.effects.counts().world, 0);
        f.set_target(g, victim);
        f.sync();
        assert!(f.key(g, &Show::Suck).is_some());
    }

    // QUIRK D-242: kept
    #[test]
    fn beams_leave_the_checksum_alone() {
        let (mut f, _g, _victim) = sucking();
        let animals = spawn(&mut f.host, ANIMAL_EVOKER, 0, (30, 20));
        f.set_spawn(animals, [60 << 12, 0, 80 << 12]);
        f.status(animals, status::EVOKING, true);
        let sum = checksum(&f.host.state);
        // Sync, frames, rebuild: `sync` and `step` check the checksum on each call.
        for _ in 0..3 {
            f.sync();
            f.step(100);
            f.step(20);
        }
        f.ability.reset(&mut f.effects);
        f.sync();
        f.step(100);
        assert_eq!(checksum(&f.host.state), sum);
        assert_eq!(f.effects.counts().world, 2);
    }

    #[test]
    fn the_ring_uses_the_type_hef() {
        let mut f = Fixture::new();
        // Foreign wolf: every viewer sees the ring (no owner filter).
        let wolf = spawn(&mut f.host, WOLF, 1, (40, 40));
        f.send(wolf, 0x22C, &[0]);
        f.sync();
        assert_eq!(f.handles(wolf), [(WOLF_HEF.to_string(), 5.0 * 128.0)]);
        let old = match f.ability.shown.values().next() {
            Some(Shown::Handle(Some(k))) => *k,
            other => panic!("not a ring: {other:?}"),
        };
        // A different 450003 of the type: the ring follows it.
        let set = |f: &mut Fixture, value: Option<&str>| {
            let values = &mut f
                .host
                .rules
                .types
                .get_mut(&WOLF)
                .unwrap()
                .modules
                .iter_mut()
                .find(|m| m.index == exec_temp::INDEX as u32)
                .unwrap()
                .values;
            match value {
                Some(v) => values.insert(RING_VALUE, v.into()),
                None => values.remove(&RING_VALUE),
            };
        };
        set(&mut f, Some(BEAR_HEF));
        f.sync();
        assert_eq!(f.handles(wolf), [(BEAR_HEF.to_string(), 5.0 * 128.0)]);
        assert!(f.effects.handle(old).is_none(), "old ring removed");
        // Without 450003 (or empty) no ring, although the influence is running.
        set(&mut f, Some(" "));
        assert!(f.wanted(wolf).is_empty());
        set(&mut f, None);
        assert!(f.wanted(wolf).is_empty());
        f.sync();
        assert!(f.ability.shown.is_empty());
        assert!(f.pending(wolf, exec_temp::INDEX, 0x42C));
    }
}
