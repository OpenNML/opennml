//! Golden checksums `RANDOM_MAP_GOLDEN` for random maps: match start via `build_level` ->
//! `Rules::load_random` -> `new_match`, steps, saving/loading via `build_level`.
//! Applies only to the reference edition (German with patch); otherwise, or without an installation, skipped.

use opennml_randommap::{RandomMapSpec, build_level, load_types};
use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_sim::rules::{Fnv64, Rules};
use opennml_sim::save::{SaveGame, checksum};
use opennml_sim::setup::{LobbyEntry, MatchSetup, SlotKind, new_match, start_package};
use opennml_sim::state::SimState;
use opennml_sim::tick::step;

/// Sections: checksum after `new_match` (`start`), sequence of the first 200 steps (`steps_200`), checksum of the
/// state loaded from the savegame at step 200 (`save_load`), sequence of steps 1 to 400 of the continued loaded
/// match (`steps_400`) and checksum after `new_match` of the second map (`koth_start`).
/// Sequences: FNV-1a 64 over the checksums per step in little endian. Map 1: seed `ZKB`, continental, 128-tile,
/// 4 players, meadow, day, normal, lobby human/AI/AI, AI on. Map 2: islands, 64-tile, 2 players, winter, night,
/// King of the Hill. Measured, the same in debug and release.
/// The field `SimState::network` does not change these values (counter-check with `serde(skip)`).
const RANDOM_MAP_GOLDEN: [(&str, u64); 5] = [
    ("start", 0x04ca_2277_11ac_6768),
    ("steps_200", 0xa24d_b678_b4ea_6423),
    ("save_load", 0xa87d_a8c8_bb13_0e87),
    ("steps_400", 0x81c9_9577_3c3e_cc85),
    ("koth_start", 0xa900_6b2f_2553_daa8),
];

struct Fixture {
    vfs: Vfs,
    spec1: RandomMapSpec,
    spec2: RandomMapSpec,
}

fn open() -> Option<Fixture> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    if game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) != GameVersion::German {
        eprintln!(
            "English edition - RANDOM_MAP_GOLDEN applies only to the reference edition, skipped"
        );
        return None;
    }
    let types = load_types(&vfs).unwrap();
    let terrain = |name: &str| {
        types
            .map_scripts
            .iter()
            .position(|s| s.to_lowercase().ends_with(&format!("/{name}.rnd")))
            .unwrap()
    };
    let landscape = |n: &str| types.landscape_types.iter().position(|l| l == n).unwrap();
    let daytime = |n: &str| types.daytime_types.iter().position(|l| l == n).unwrap();
    let game_type = |n: &str| types.game_types.iter().position(|g| g.gtype == n).unwrap();
    let spec1 = RandomMapSpec {
        seed_text: "ZKB".into(),
        size: 1,
        players: 4,
        terrain: terrain("continental"),
        landscape: landscape("MEADOW"),
        daytime: daytime("DAY"),
        game_type: game_type("NORMAL"),
    };
    let spec2 = RandomMapSpec {
        seed_text: "ZKB".into(),
        size: 0,
        players: 2,
        terrain: terrain("islands"),
        landscape: landscape("WINTER"),
        daytime: daytime("NIGHT"),
        game_type: game_type("KING_OF_THE_HILL"),
    };
    Some(Fixture { vfs, spec1, spec2 })
}

fn entry(kind: SlotKind, name: &str, team: i32) -> LobbyEntry {
    LobbyEntry {
        kind,
        name: name.into(),
        nation: 0,
        team,
        color: team,
        handicap: 0,
    }
}

fn setup_for(spec: &RandomMapSpec, lobby: Vec<LobbyEntry>) -> MatchSetup {
    let mut setup = MatchSetup {
        mode: spec.game_type as i32,
        lobby,
        ..MatchSetup::default()
    };
    setup.ki.enabled = true;
    setup
}

fn rules_for(vfs: &Vfs, spec: &RandomMapSpec) -> Rules {
    let level = build_level(vfs, spec).unwrap();
    Rules::load_random(vfs, &level.scene, level.rules).unwrap()
}

fn fold(sums: &[u64]) -> u64 {
    let mut h = Fnv64::default();
    for c in sums {
        h.write(&c.to_le_bytes());
    }
    h.0
}

/// Runs the sequence: returns the five sections.
fn measure(f: &Fixture) -> [(&'static str, u64); 5] {
    let lobby = vec![
        entry(SlotKind::Human, "Mensch", 0),
        entry(SlotKind::Ai, "KI", 1),
        entry(SlotKind::Ai, "KI", 2),
    ];
    let setup = setup_for(&f.spec1, lobby);
    let rules = rules_for(&f.vfs, &f.spec1);
    let mut state: SimState = new_match(&rules, &setup).unwrap();
    let start = checksum(&state);
    let mut sums = Vec::new();
    for _ in 0..200 {
        let _ = step(&mut state, &rules, &[]);
        sums.push(checksum(&state));
    }
    let steps_200 = fold(&sums);
    // Save at 200, load like the load path: regenerate the map from the map description.
    let bytes = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    }
    .to_bytes();
    let save = SaveGame::from_bytes(&bytes).unwrap();
    let spec = save.start.random_map.clone().expect("map description");
    assert_eq!(spec, f.spec1);
    let loaded_rules = rules_for(&f.vfs, &spec);
    save.check(&loaded_rules).unwrap();
    let mut loaded = save.state;
    let save_load = checksum(&loaded);
    assert_eq!(save_load, sums[199], "loaded state differs");
    for _ in 200..400 {
        let _ = step(&mut loaded, &loaded_rules, &[]);
        let _ = step(&mut state, &rules, &[]);
        let c = checksum(&loaded);
        assert_eq!(c, checksum(&state), "loaded match differs");
        sums.push(c);
    }
    let steps_400 = fold(&sums);
    let rules2 = rules_for(&f.vfs, &f.spec2);
    let setup2 = setup_for(
        &f.spec2,
        vec![
            entry(SlotKind::Human, "Mensch", 0),
            entry(SlotKind::Ai, "KI", 1),
        ],
    );
    let koth_start = checksum(&new_match(&rules2, &setup2).unwrap());
    [
        ("start", start),
        ("steps_200", steps_200),
        ("save_load", save_load),
        ("steps_400", steps_400),
        ("koth_start", koth_start),
    ]
}

#[test]
fn random_map_golden() {
    let Some(f) = open() else { return };
    let got = measure(&f);
    for (n, v) in got {
        eprintln!("RANDOM_MAP_GOLDEN {n} {v:#018x}");
    }
    assert_eq!(got, RANDOM_MAP_GOLDEN, "measured: {got:#x?}");
}

#[test]
fn golden_twice_equal() {
    let Some(f) = open() else { return };
    assert_eq!(measure(&f), measure(&f));
}
