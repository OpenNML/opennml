//! State of the world display for the control channel (re/world-display): rally flag, ground marks, tread marks,
//! water rings, silhouettes, beams and dynamite charges as
//! [`nml_control::WorldDisplayState`] (viewer `state.world_display`, `nml-game` `ui_state.game.world`).
//!
//! Read-only: the parts come from the effect world and the host's display lists
//! ([`crate::game::map::MapView::world_display`]); the simulation and its checksum stay untouched.

use std::collections::BTreeMap;

use glam::Vec3;
use nml_control::{BeamState, ChargeState, MarkCount, RallyFlagState, WorldDisplayState};
use nml_effects::world::EffectWorld;
use nml_render::scene::RENDER_STEALTH;
use nml_sim::id::ObjectId;

use crate::game::rally_flag::RallyFlag;
use crate::game::world_effects::ChargeTrack;

/// Counter for the control channel (saturating; the lists of the effect world are small).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// State of the world display from the parts of the host:
/// - `rally`: the flag of the last frame ([`crate::game::map::MapView::set_rally_flag`]) with its
///   drawn position (with a rally target on top of the target); animation time is the clock of the effect
///   world ([`EffectWorld::now_ms`], D-261).
/// - Marks, tread points and water rings from the effect world ([`EffectWorld::mark_counts`],
///   [`EffectWorld::tread_points`], [`EffectWorld::surf_ring_count`]).
/// - `stealth`: IDs with the silhouette bit ([`stealth_ids`]).
/// - `beams`: (owner, path, key) of the running beams
///   ([`crate::game::ability_effects::AbilityEffects::beams`]); position, target, flight time and particles from
///   the game effect. A key without an effect (rebuild, not yet recreated) is missing.
/// - `charges`: the tracks of the dynamite charges per projectile ID
///   ([`crate::game::world_effects::WorldEffects::charges`]).
///
/// Only reads; the simulation does not come into play.
pub fn world_display_state(
    effects: &EffectWorld,
    rally: Option<(&RallyFlag, Vec3)>,
    stealth: Vec<u32>,
    beams: &[(ObjectId, &str, u32)],
    charges: &BTreeMap<u32, ChargeTrack>,
) -> WorldDisplayState {
    WorldDisplayState {
        rally_flag: rally.map(|(flag, position)| RallyFlagState {
            building: flag.building.0,
            position: position.to_array(),
            color: flag.color,
            anim_ms: effects.now_ms() as f32,
        }),
        marks: effects
            .mark_counts()
            .into_iter()
            .map(|m| MarkCount {
                texture: m.texture,
                count: count(m.count),
                visible: count(m.visible),
            })
            .collect(),
        tread_points: count(effects.tread_points()),
        surf_rings: count(effects.surf_ring_count()),
        stealth,
        beams: beams
            .iter()
            .filter_map(|&(owner, path, key)| {
                let e = &effects.game_effect(key)?.effect;
                Some(BeamState {
                    owner: owner.0,
                    path: path.to_string(),
                    position: e.placement().position.to_array(),
                    target: e.target().map(<[f32; 3]>::from),
                    flight_s: e.flight_s(),
                    particles: count(e.particle_count()),
                })
            })
            .collect(),
        charges: charges
            .iter()
            .map(|(&projectile, t)| ChargeState {
                projectile,
                position: t.pos.to_array(),
                yaw: t.yaw,
                last_digit: t.last_digit,
                follows: t.follows,
            })
            .collect(),
    }
}

/// IDs of the renderer entries `(simulation ID, render bits)` that currently carry the silhouette bit
/// [`RENDER_STEALTH`] (`0x80`) - foreign camouflaged units that are walking (re/world-display W.2.1, D-263; when
/// standing, the host passes on bits 0, `sim_host::stealth_bits`). Sorted, without duplicates; entries without a
/// simulation object do not count.
pub fn stealth_ids(entries: impl IntoIterator<Item = (Option<u32>, u32)>) -> Vec<u32> {
    let mut ids: Vec<u32> = entries
        .into_iter()
        .filter(|&(_, bits)| bits & RENDER_STEALTH != 0)
        .filter_map(|(id, _)| id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ability_effects::{
        AbilityEffects, BEAM_EFFECT, BEAM_NODE, SUCK_EFFECT, SUCK_NODE,
    };
    use crate::game::rally_flag::{flag_color, rally_flag};
    use crate::game::sim_host::{SimHost, stealth_bits};
    use crate::game::testworld::{ANIMAL_EVOKER, ARCHER, CHIEF_TENT, GHOST, PATRIOT, host, spawn};
    use crate::game::world_effects::WorldEffects;
    use nml_effects::attached::EffectTarget;
    use nml_effects::marks::{Footprint, SurfRing};
    use nml_effects::world::{ObjectFrame, ObjectView};
    use nml_rda::Vfs;
    use nml_rda::testutil::{TestFile, build_rda};
    use nml_render::scene::RENDER_MODEL;
    use nml_sim::combat::projectile::add_dynamite;
    use nml_sim::module::ghosts::ghost;
    use nml_sim::module::{ModuleKind, evoke_animal};
    use nml_sim::object::status;
    use nml_sim::save::checksum;
    use nml_sim::sim::Sim;
    use nml_sim::world::grid_to_world;
    use nml_world::assets::ModelCache;
    use nml_world::camera::{game_projection, look_at_lh};
    use nml_world::visibility::Frustum;

    /// Loop with 10 particles per s; `MOVEMENTTIME 1` (otherwise the target flight becomes NaN).
    const PEF: &str = "EFFECT\r\n{\r\n\tMOVEMENTTIME 1.000000\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
    /// Node `saugen01` or `zauber01` in object space (every model of the test world).
    const SUCK_LOCAL: Vec3 = Vec3::new(0.0, 80.0, 10.0);
    const STAFF_LOCAL: Vec3 = Vec3::new(10.0, 120.0, 0.0);

    fn archive() -> (tempfile::TempDir, Vfs) {
        let files: Vec<TestFile> = [SUCK_EFFECT, BEAM_EFFECT]
            .iter()
            .map(|path| TestFile {
                path,
                data: PEF.as_bytes(),
                compress: false,
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    /// Objects of the test world for the effect world: index = ID, position from the grid.
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

    /// A test world with all kinds reachable without a GPU: ghost with a suction target, evoking animal
    /// evoker, two dynamite charges (with and without a target), footprints and craters, one tread mark, two
    /// water rings; plus a chief's tent as the flag building.
    struct World {
        _dir: tempfile::TempDir,
        host: SimHost,
        effects: EffectWorld,
        ability: AbilityEffects,
        world: WorldEffects,
        ghost: ObjectId,
        evoker: ObjectId,
        victim: ObjectId,
        tent: ObjectId,
    }

    fn world() -> World {
        let (dir, vfs) = archive();
        let mut host = host();
        let g = spawn(&mut host, GHOST, 0, (20, 26));
        let evoker = spawn(&mut host, ANIMAL_EVOKER, 0, (30, 20));
        let victim = spawn(&mut host, ARCHER, 1, (22, 26));
        let patriot = spawn(&mut host, PATRIOT, 0, (24, 26));
        let tent = spawn(&mut host, CHIEF_TENT, 0, (40, 40));
        {
            let o = host.state.object_mut(g, false).unwrap();
            let Some(ModuleKind::Ghost(gh)) = o.module_mut(ghost::INDEX).map(|m| &mut m.kind)
            else {
                panic!("no ghost");
            };
            gh.target = victim;
        }
        {
            let o = host.state.object_mut(evoker, false).unwrap();
            o.set_status_flags(status::EVOKING, true);
            let Some(ModuleKind::EvokeAnimal(e)) =
                o.module_mut(evoke_animal::INDEX).map(|m| &mut m.kind)
            else {
                panic!("no animal evoker");
            };
            e.spawn = [60 << 12, 0, 80 << 12];
        }
        let rules = host.rules.clone();
        {
            let mut sim = Sim::new(&mut host.state, &rules);
            add_dynamite(&mut sim, patriot, victim, [3000.0, 20.0, 2000.0], 2);
            add_dynamite(&mut sim, patriot, ObjectId::NONE, [100.0, 0.0, 200.0], 3);
        }
        let mut world = WorldEffects::default();
        world.sync_charges(&host);

        let mut effects = EffectWorld::new();
        let mut models = ModelCache::new();
        let mut ability = AbilityEffects::default();
        let state = &host.state;
        ability.sync(
            &host,
            &mut effects,
            &mut models,
            &vfs,
            &|_, _| Vec::new(),
            &|id| state.object(id, false).map(|_| id.0 as usize),
            128.0,
        );
        // A pair and a single step (type 1, listed after the game tick), a crater after that
        // (not yet listed).
        effects.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2));
        effects.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 1, 1, 2));
        let tread = effects.add_tread(2);
        let now = effects.now_ms();
        let t = effects.tread_mut(tread).unwrap();
        for z in [0.0, 50.0, 100.0] {
            t.add_point(Vec3::new(0.0, 0.0, z), now);
        }
        for x in [10.0, 20.0] {
            effects.add_surf_ring(SurfRing::new(
                Vec3::new(x, -3.0, 20.0),
                0.0,
                Vec3::ZERO,
                10.0,
            ));
        }
        effects.step(500, &frustum(), &View { host: &host }, &vfs, &mut models);
        effects.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 0x430, 2));
        World {
            _dir: dir,
            host,
            effects,
            ability,
            world,
            ghost: g,
            evoker,
            victim,
            tent,
        }
    }

    #[test]
    fn world_display_reports_flag_marks_beams_and_charges() {
        let w = world();
        // Silhouettes: render bits per `stealth_bits` - only the walking foreign camouflaged unit carries
        // `0x80`; entries without a simulation object do not count.
        let (walking, standing, own) = (ObjectId(0x0005_0001), ObjectId(0x0003_0001), w.ghost);
        let stealth = stealth_ids([
            (Some(standing.0), stealth_bits(RENDER_STEALTH, 0)),
            (Some(walking.0), stealth_bits(RENDER_STEALTH, status::MOVE)),
            (Some(own.0), RENDER_MODEL),
            (None, RENDER_STEALTH),
        ]);
        assert_eq!(stealth, [walking.0]);

        // Flag with position from the last frame (with a rally target on top of the target).
        let flag = RallyFlag {
            building: w.tent,
            position: Vec3::new(640.0, 0.0, 1280.0),
            color: flag_color(2),
            target: Some(w.victim),
        };
        let drawn = Vec3::new(2880.0, 96.0, 3392.0);
        let beams = w.ability.beams();
        let state = world_display_state(
            &w.effects,
            Some((&flag, drawn)),
            stealth.clone(),
            &beams,
            w.world.charges(),
        );

        let f = state.rally_flag.as_ref().expect("flag");
        assert_eq!(
            (f.building, f.position, f.color, f.anim_ms),
            (w.tent.0, drawn.to_array(), flag_color(2), 500.0)
        );
        assert_eq!(
            state.marks,
            [
                MarkCount {
                    texture: Footprint::texture(1).into(),
                    count: 2,
                    visible: 2
                },
                MarkCount {
                    texture: Footprint::texture(0x430).into(),
                    count: 1,
                    visible: 0
                },
            ]
        );
        assert_eq!((state.tread_points, state.surf_rings), (3, 2));
        // The tread mark of the setup runs exactly along z: counted, but never drawn (D-286).
        assert!(
            w.effects
                .ground_marks(&|_, _| 0.0, &|_| 7)
                .treads
                .is_empty()
        );
        assert_eq!(state.stealth, stealth);

        // Beams: suction beam of the ghost (target of the nodes `saugen01`, measured in the game tick) and
        // animal evoker beam (target is the spawn position with height 0, D-277; flight time measured per
        // position), with position, flight time and particles of the effect world.
        assert_eq!(beams.len(), 2);
        assert_eq!(state.beams.len(), 2);
        for (b, &(owner, path, key)) in state.beams.iter().zip(&beams) {
            let a = w.effects.game_effect(key).unwrap();
            assert_eq!(
                (b.owner, b.path.as_str(), b.position),
                (owner.0, path, a.effect.placement().position.to_array())
            );
            assert_eq!(b.target, a.effect.target().map(<[f32; 3]>::from));
            assert_eq!(b.flight_s, a.effect.flight_s());
            assert_eq!(b.particles as usize, a.effect.particle_count());
            assert!(b.particles > 0, "{b:?}");
        }
        let suck = state.beams.iter().find(|b| b.owner == w.ghost.0).unwrap();
        assert_eq!(suck.path, SUCK_EFFECT);
        assert!(suck.target.is_some(), "target set");
        let evoked = state.beams.iter().find(|b| b.owner == w.evoker.0).unwrap();
        assert_eq!(evoked.path, BEAM_EFFECT);
        assert_eq!(evoked.target, Some([7680.0, 0.0, 10240.0]));
        let key = beams.iter().find(|b| b.0 == w.evoker).unwrap().2;
        assert_eq!(
            w.effects.game_effect(key).unwrap().target,
            Some(EffectTarget::Point(Vec3::new(7680.0, 0.0, 10240.0)))
        );

        // Charges per projectile ID: with a target it follows, without a target it does not.
        let tracks: Vec<_> = w.world.charges().iter().collect();
        assert_eq!(tracks.len(), 2);
        assert_eq!(state.charges.len(), 2);
        for (c, (id, t)) in state.charges.iter().zip(&tracks) {
            assert_eq!(
                (c.projectile, c.position, c.yaw, c.last_digit, c.follows),
                (**id, t.pos.to_array(), t.yaw, t.last_digit, t.follows)
            );
        }
        assert_eq!(
            state.charges.iter().map(|c| c.follows).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(state.charges[1].position, [100.0, 0.0, 200.0]);

        // Without flag, beams and charges: empty.
        let empty = world_display_state(
            &EffectWorld::new(),
            None,
            Vec::new(),
            &[],
            &Default::default(),
        );
        assert_eq!(empty, WorldDisplayState::default());
        // A beam whose effect is gone is missing.
        let gone = world_display_state(
            &w.effects,
            None,
            Vec::new(),
            &[(w.ghost, SUCK_EFFECT, u32::MAX)],
            &Default::default(),
        );
        assert!(gone.beams.is_empty());
    }

    #[test]
    fn world_display_leaves_the_checksum_alone() {
        let w = world();
        let sum = checksum(&w.host.state);
        let selection = [w.tent];
        let flag = rally_flag(&w.host.state, &w.host.rules, &selection, 0);
        let collect = || {
            world_display_state(
                &w.effects,
                flag.as_ref().map(|f| (f, f.position)),
                stealth_ids([(Some(w.ghost.0), RENDER_STEALTH)]),
                &w.ability.beams(),
                w.world.charges(),
            )
        };
        let first = collect();
        assert_eq!(checksum(&w.host.state), sum, "the query changes nothing");
        assert_eq!(collect(), first, "repeatable");
        assert_eq!(checksum(&w.host.state), sum);
        assert_eq!(first.beams.len(), 2);
    }
}
