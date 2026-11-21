//! Savegames on original maps from a private corpus: every `*.nmlgame` below `NML_SAVE_CORPUS` (recursively) loads with
//! the rules of its map from the original installation (`NML_GAME_DIR`), continues for 100 steps and loads again
//! after saving; at the end the test reports size and load time per file. Skipped without one of the two variables.
//!
//! `write_corpus_saves` (ignored) writes the savegames of this version to `$NML_SAVE_CORPUS/save-<SAVE_FORMAT>/`: a
//! campaign map and a large random map with eight AI slots, 12 000 steps each.

use std::path::{Path, PathBuf};
use std::time::Instant;

use opennml_client::game::map::{MapLoad, MapSource, load_rules, map_rules};
use opennml_client::game::options::DEFAULT_GAME_SPEED;
use opennml_client::game_save::{CameraSave, GameSaveFile, HostPart, SAVE_FORMAT};
use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;
use opennml_sim::random_map::RandomMapSpec;
use opennml_sim::rules::Rules;
use opennml_sim::save::SaveGame;
use opennml_sim::setup::{
    LobbyEntry, MatchSetup, SlotKind, ki_kind_handicap, new_match, start_package,
};
use opennml_world::camera::{CameraSetup, GameCamera};
use opennml_world::level::LevelScene;

/// Campaign map of the corpus.
const CAMPAIGN_LEVEL: &str = "data/levels/kampagne3/kamp3-14/kamp3-14.lvl";
/// Steps of a written match.
const STEPS: usize = 12_000;
/// Steps after loading.
const CONTINUE_STEPS: usize = 100;

/// Original installation and corpus folder, or `None` with a line on stderr.
fn open() -> Option<(Vfs, PathBuf)> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found (NML_GAME_DIR) - savegame corpus test skipped");
        return None;
    };
    let Some(corpus) = std::env::var_os("NML_SAVE_CORPUS").map(PathBuf::from) else {
        eprintln!("NML_SAVE_CORPUS not set - savegame corpus test skipped");
        return None;
    };
    Some((Vfs::open_game_dir(&dir).unwrap(), corpus))
}

/// All `*.nmlgame` below `dir`, sorted.
fn savegames(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut open = vec![dir.to_path_buf()];
    while let Some(dir) = open.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                open.push(path);
            } else if path.extension().is_some_and(|e| e == "nmlgame") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Measured load of a file.
struct Loaded {
    path: PathBuf,
    bytes: usize,
    objects: usize,
    /// `GameSaveFile::from_bytes` (header, unpacking, decoding, structure checks).
    read_ms: f64,
    /// Rules of the map (`map_rules`, from the original installation; for a random map from the stored map).
    rules_ms: f64,
    /// `GameSaveFile::prepare`.
    prepare_ms: f64,
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Loads `path` like the game (file, rules of its map, `prepare`), continues the match for [`CONTINUE_STEPS`] steps,
/// saves it and loads the new file again with the same rules.
fn load_and_continue(vfs: &Vfs, path: &Path) -> Loaded {
    let name = path.display();
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let t = Instant::now();
    let mut file = GameSaveFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let read_ms = ms(t);
    let t = Instant::now();
    let source = MapSource::of_save(&file);
    let rules = map_rules(vfs, &source, "DEU").unwrap_or_else(|e| panic!("{name}: {e:#}"));
    let rules_ms = ms(t);
    let t = Instant::now();
    file.prepare(&rules)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let prepare_ms = ms(t);
    let GameSaveFile {
        sim,
        host,
        random_map,
        ..
    } = file;
    let mut state = sim.state;
    let tick = state.tick;
    for _ in 0..CONTINUE_STEPS {
        opennml_sim::tick::step(&mut state, &rules, &[]);
    }
    assert_ne!(state.tick, tick, "{name}: the match continues");
    let objects = state.objects().count();
    // Saving the continued match on this installation gives a file that loads again here. `prepare` took over the
    // rules checksums of this installation and the other edition has the same logic part, so the file loads there
    // too.
    let sim = SaveGame { state, ..sim };
    let saved = GameSaveFile::new(sim, host, random_map).to_bytes();
    let mut again =
        GameSaveFile::from_bytes(&saved).unwrap_or_else(|e| panic!("{name} saved again: {e}"));
    again
        .prepare(&rules)
        .unwrap_or_else(|e| panic!("{name} saved again: {e}"));
    Loaded {
        path: path.to_path_buf(),
        bytes: bytes.len(),
        objects,
        read_ms,
        rules_ms,
        prepare_ms,
    }
}

fn report(loaded: &[Loaded]) {
    println!("| File | Bytes | Objects | Read ms | Rules ms | Prepare ms |");
    println!("|---|---|---|---|---|---|");
    for l in loaded {
        println!(
            "| {} | {} | {} | {:.1} | {:.1} | {:.2} |",
            l.path.display(),
            l.bytes,
            l.objects,
            l.read_ms,
            l.rules_ms,
            l.prepare_ms
        );
    }
}

#[test]
fn corpus_savegames_load_and_continue() {
    let Some((vfs, corpus)) = open() else {
        return;
    };
    let files = savegames(&corpus);
    if files.is_empty() {
        eprintln!("no savegames below {}", corpus.display());
    }
    let loaded: Vec<Loaded> = files.iter().map(|p| load_and_continue(&vfs, p)).collect();
    report(&loaded);
}

/// Host part with default values: camera at the start pose of the map, map description, no control groups, no
/// command log.
fn default_host_part(scene: &LevelScene, label: &str, players: usize, game_type: i32) -> HostPart {
    let camera = GameCamera::new(CameraSetup::default(), &scene.terrain);
    HostPart {
        camera: CameraSave::of(&camera),
        level_info: opennml_ui::draft::LevelInfoData {
            name: label.into(),
            path: label.into(),
            players: players as i32,
            ..opennml_ui::draft::LevelInfoData::default()
        },
        game_type,
        local_name: "Player".into(),
        game_speed: DEFAULT_GAME_SPEED,
        cinemascope_speed: None,
        script_camera: Default::default(),
        control_groups: opennml_client::game::control_groups::empty_save(),
        next_guid_index: opennml_client::game::control_groups::empty_next_guid_index(),
        idle_worker_index: [0; 8],
        masks: opennml_client::game::diplomacy::Masks::initial(),
        lock_teams: false,
        fog_show_mode: 0,
        net: None,
        log: None,
        save_replay: false,
    }
}

/// Savegame of a new match of `rules` and `setup` after [`STEPS`] steps without commands.
fn play(rules: &Rules, setup: &MatchSetup) -> SaveGame {
    let mut state = new_match(rules, setup).unwrap();
    for _ in 0..STEPS {
        opennml_sim::tick::step(&mut state, rules, &[]);
    }
    SaveGame {
        start: start_package(rules, setup),
        state,
        commands: Vec::new(),
    }
}

/// Writes `file` to `path` (through a temporary file, so the loading test never reads half a file).
fn write(file: &GameSaveFile, path: &Path) {
    let temp = path.with_extension("nmlgame.part");
    std::fs::write(&temp, file.to_bytes()).unwrap_or_else(|e| panic!("{}: {e}", temp.display()));
    std::fs::rename(&temp, path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// Writes the corpus savegames of this version and loads them again: the campaign map [`CAMPAIGN_LEVEL`] (defaults of
/// the level, AI and script on, difficulty 1) and a large random map (256 x 256) with eight AI slots (kind 3, nations
/// in turn), each after [`STEPS`] steps with the default host part; the random map with its generated map.
#[test]
#[ignore = "writes savegames into NML_SAVE_CORPUS (12 000 steps per match, run in release)"]
fn write_corpus_saves() {
    let Some((vfs, corpus)) = open() else {
        return;
    };
    let out = corpus.join(format!("save-{SAVE_FORMAT}"));
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));

    let level = MapLoad::read(&vfs, CAMPAIGN_LEVEL).unwrap();
    let rules = load_rules(&vfs, CAMPAIGN_LEVEL, "DEU").unwrap();
    let mut setup = MatchSetup::default_for(&rules);
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    setup.script.enabled = true;
    let host = default_host_part(
        &level.scene,
        CAMPAIGN_LEVEL,
        rules.level.players.len(),
        setup.mode,
    );
    let campaign = out.join("kamp3-14.nmlgame");
    write(
        &GameSaveFile::new(play(&rules, &setup), host, None),
        &campaign,
    );

    let spec = RandomMapSpec {
        seed_text: "LARGE8".into(),
        size: 2,
        players: 8,
        terrain: 0,
        landscape: 0,
        daytime: 0,
        game_type: 0,
    };
    let source = MapSource::Random(spec);
    let mut load = MapLoad::open(&vfs, &source).unwrap();
    let random = load.random.take().expect("rules input of the random map");
    let rules = Rules::load_random(&vfs, &load.scene, random).unwrap();
    assert_eq!(rules.map.dim_x, 256, "large random map");
    let lobby = rules
        .level
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| LobbyEntry {
            kind: SlotKind::Ai,
            name: String::new(),
            nation: (i % 6) as i32,
            team: i32::from(p.slot),
            color: i32::from(p.slot),
            handicap: ki_kind_handicap(3).unwrap(),
        })
        .collect();
    let mut setup = MatchSetup {
        mode: 0,
        seed: 0x4E4D_4C31,
        lobby,
        ..MatchSetup::default()
    };
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    let label = source.label();
    let host = default_host_part(&load.scene, &label, rules.level.players.len(), setup.mode);
    let random = out.join("random-large-8ai.nmlgame");
    let file = GameSaveFile::new(play(&rules, &setup), host, load.random_map);
    write(&file, &random);

    let loaded: Vec<Loaded> = [campaign, random]
        .iter()
        .map(|p| load_and_continue(&vfs, p))
        .collect();
    report(&loaded);
}
