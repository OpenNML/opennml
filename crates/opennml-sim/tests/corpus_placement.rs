//! Build place check on real maps: the snapping of the check is idempotent (exception A8).

use std::collections::BTreeMap;

use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;
use opennml_sim::placement::{
    building_flags, cursor_placement, flags, same_pose, snapped_command_pose,
};
use opennml_sim::rules::Rules;
use opennml_sim::setup::{MatchSetup, new_match};
use opennml_sim::sim::Sim;
use opennml_world::level::load_level;

/// A8 accepts a build command only if the check leaves position and rotation bit-identical
/// starting from the command position (`snapped_command_pose`). The UI sends the position of the
/// build cursor (`cursor_placement`); for field, gold mine, tower and gate the check snaps
/// further (deposit or wall piece). Every valid cursor position must therefore pass the command
/// check - otherwise A8 would silently reject real commands, e.g. after a change to the search
/// radii, to `snap_box_min_corner_to_grid` or to `find_nearest_matching`.
///
/// Sample, so that the test needs only seconds even in the debug build: three maps with deposits
/// and walls of own players; ground points in the half-tile grid (with an offset so that they do
/// not fall on tile boundaries) around each deposit (±6 tiles, field and gold mine) or each wall
/// piece of a player (±2 tiles, tower and gate). One rotation suffices: all four kinds carry the
/// rotation lock in the shipped data, the cursor sets the rotation to 0. The full run (all 55 maps, both
/// rotations, 3.01 million valid positions) likewise found no deviation.
#[test]
fn cursor_snaps_are_idempotent_on_sample_maps() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let levels = [
        "data/levels/kampagne1/kamp1-2/kamp1-2.lvl",
        "data/levels/tutorial0-2/kamp0-2.lvl",
        "data/levels/multiplayer/[2players]-crossrail.lvl",
    ];
    let kinds_of_interest = [flags::FIELD, flags::GOLD_MINE, flags::TOWER, flags::GATE];
    let mut valid: BTreeMap<u32, u64> = BTreeMap::new();
    for lvl in levels {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let setup = MatchSetup::default_for(&rules);
        let mut state = new_match(&rules, &setup).unwrap_or_else(|e| panic!("{lvl}: {e}"));
        // Types per kind (the low byte does not count, as in the check).
        let mut kinds: Vec<(u32, u32)> = Vec::new();
        for &guid in rules.types.keys() {
            if let Some(f) = building_flags(&rules, guid) {
                for kind in kinds_of_interest {
                    if f & 0xFFFF_FF00 & kind != 0 {
                        kinds.push((guid, kind));
                    }
                }
            }
        }
        // Anchors: nature objects (module 6) for field and gold mine, wall pieces of the players for
        // tower and gate.
        let anchors: Vec<(i32, [f32; 3], bool)> = state
            .objects()
            .filter_map(|o| {
                let wall = o.owner() < 8
                    && building_flags(&rules, o.guid).is_some_and(|f| f & flags::WALL_SEGMENT != 0);
                let deposit = o.has_module(6, false);
                (wall || deposit).then_some((i32::from(o.owner()), o.world_pos, deposit))
            })
            .collect();
        let half = [
            rules.map.cell[0] as f32 * 0.5,
            rules.map.cell[2] as f32 * 0.5,
        ];
        let mut sim = Sim::new(&mut state, &rules);
        for &(owner, at, deposit) in &anchors {
            for &(guid, kind) in &kinds {
                if (kind == flags::FIELD || kind == flags::GOLD_MINE) != deposit {
                    continue;
                }
                let (player, span) = if deposit { (0, 12) } else { (owner, 4) };
                for dx in -span..=span {
                    for dz in -span..=span {
                        let ground = [
                            at[0] + dx as f32 * half[0] + 13.0,
                            at[1],
                            at[2] + dz as f32 * half[1] + 7.0,
                        ];
                        let Some(c) = cursor_placement(&mut sim, guid, ground, 0.0, player, None)
                        else {
                            continue;
                        };
                        if !c.valid {
                            continue;
                        }
                        *valid.entry(kind).or_default() += 1;
                        let again =
                            snapped_command_pose(&mut sim, guid, c.pos, c.rot, player, None);
                        assert!(
                            again.is_some_and(|p| same_pose(p, (c.pos, c.rot))),
                            "{lvl}: type {guid} cursor position {:?} -> command check {again:?}",
                            (c.pos, c.rot)
                        );
                    }
                }
            }
        }
    }
    // The sample covers all four kinds.
    for kind in kinds_of_interest {
        assert!(
            valid.get(&kind).is_some_and(|&n| n > 100),
            "too few valid positions for {kind:#x}: {valid:?}"
        );
    }
}
