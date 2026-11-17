//! Corpus: a rifleman in the tower fires from the shoot position of the tower (`FireGun`
//! `0x00526930` -> `QuarteredShootPos` `0x00525500`, re/combat-ranged 3.4): muzzle flash as the
//! tower's own effect at the node `schussposition0N` nearest to the target, sound 1 there.

use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_sim::command::{Command, CommandKind};
use nml_sim::rules::Rules;
use nml_sim::setup::{MatchSetup, new_match};
use nml_sim::sim::Sim;
use nml_sim::state::{SimEvent, SimState};
use nml_sim::tick::step;
use nml_sim::world::{ObjectDesc, create_object, desc_flags};
use nml_world::level::load_level;

const LEVEL: &str = "data/levels/multiplayer/[4players]-exile.lvl";
/// Spanish tower (SID) and infantryman (rifle with magazine).
const TOWER: u32 = 2510;
const GUNNER: u32 = 2607;
const FLASH: &str = "data/effects/normaler_schuss.pef";

fn corpus() -> Option<Rules> {
    let dir = corpus_game_dir()?;
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    Some(Rules::load(&vfs, &scene).unwrap())
}

fn world_pos(state: &SimState, rules: &Rules, id: nml_sim::id::ObjectId) -> [f32; 3] {
    let o = state.object(id, false).unwrap();
    nml_sim::world::grid_to_world(o.grid, rules.map.cell)
}

#[test]
fn a_quartered_gunner_fires_from_the_tower_node_nearest_the_target() {
    let Some(rules) = corpus() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).unwrap();
    let hq = state
        .objects()
        .find(|o| o.guid == 2500 && o.owner() == 0)
        .map(|o| o.world_pos)
        .unwrap();
    // Free field east of the own HQ; the opponent stands 8 tiles in +x in front of the tower.
    let base = [hq[0] + 12.0 * 128.0, hq[1], hq[2] + 6.0 * 128.0];
    let mut make = |guid, slot, p: [f32; 3]| {
        let mut sim = Sim::new(&mut state, &rules);
        let mut d = ObjectDesc::new(guid, slot, p);
        d.flags |= desc_flags::START_OBJECT;
        create_object(&mut sim, &mut d).expect("created")
    };
    let tower = make(TOWER, 0, base);
    let gunner = make(GUNNER, 0, [base[0] - 3.0 * 128.0, base[1], base[2]]);
    let enemy = make(GUNNER, 1, [base[0] + 8.0 * 128.0, base[1], base[2]]);
    assert_eq!(state.object(tower, false).unwrap().rotation[1], 0.0);
    // Unrotated tower, target in +x: the node with the largest x (all nodes equally high).
    let shape = &rules.building_shapes[&TOWER];
    let node = (1..)
        .map_while(|i| shape.helper_node(&format!("schussposition{i:02}")))
        .max_by(|a, b| a[0].total_cmp(&b[0]))
        .expect("shoot positions of the tower");
    let t = world_pos(&state, &rules, tower);
    let expected = [t[0] + node[0], t[1] + node[1], t[2] + node[2]];
    assert!(
        node[1] > 400.0,
        "shoot position at the top of the tower: {node:?}"
    );

    let hp = |s: &SimState| {
        s.object(enemy, false)
            .unwrap()
            .attributes
            .current_value(14, 0)
    };
    let hp0 = hp(&state);
    let mut cmds = vec![Command {
        player: 0,
        kind: CommandKind::Quarter {
            container: tower,
            units: vec![gunner],
        },
    }];
    let mut flashes = Vec::new();
    let mut shots = Vec::new();
    for _ in 0..1200 {
        let events = step(&mut state, &rules, &cmds);
        cmds.clear();
        for e in events {
            match e {
                SimEvent::EffectOwnedAt {
                    object, path, pos, ..
                } if path == FLASH => flashes.push((object, pos)),
                SimEvent::EffectAttached { object, path, .. } if object == gunner => {
                    panic!("effect {path} at the node of the quartered shooter")
                }
                SimEvent::Sound3d {
                    object,
                    action,
                    pos,
                    ..
                } if object == gunner && action == 1 => shots.push(pos),
                _ => {}
            }
        }
        if flashes.len() >= 2 {
            break;
        }
    }
    let g = state.object(gunner, false).unwrap();
    assert_ne!(
        g.status & nml_sim::object::status::QUARTERED,
        0,
        "quartered"
    );
    assert!(!flashes.is_empty(), "the shooter in the tower fires");
    for (object, pos) in &flashes {
        assert_eq!(*object, tower, "muzzle flash at the tower");
        assert_eq!(*pos, expected, "at the shoot position toward the target");
    }
    assert!(
        shots.iter().all(|p| *p == expected),
        "sound 1 at the shoot position"
    );
    assert!(hp(&state) < hp0, "the opponent takes damage");
}
