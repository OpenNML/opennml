//! Scripts on the campaign map `kamp1-1`: `SCRIPT_GOLDEN` (golden scene) with saving/loading in the
//! middle of the flow and in the middle of the intro, and the key figure `SCRIPT_BLOCKS`.

use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use nml_sim::rules::{Fnv64, Rules};
use nml_sim::save::{SaveGame, checksum};
use nml_sim::script::ScriptTrace;
use nml_sim::setup::{MatchSetup, new_match, start_package};
use nml_sim::state::{SimEvent, SimState};
use nml_sim::tick::step;
use nml_world::level::load_level;

/// Map of the golden scene.
const LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
/// Start index of the random sequence of the golden scene ("NML2").
const SEED: u32 = 0x4E4D_4C32;
/// Steps of `SCRIPT_GOLDEN`: five game minutes.
const STEPS: usize = 6000;
/// Save point in the middle of the scene.
const SAVE_AT: usize = STEPS / 2;
/// Save point in the middle of the intro (measured): cinemascope is running (steps 2 to 1,600) and
/// line 20008 is spoken (steps 1,098 to 1,229).
const SAVE_IN_INTRO: usize = 1100;
/// Dialog that the level script of `kamp1-1` starts in step 1.
const INTRO_DIALOG: &str = "data/levels/kampagne1/kamp1-1/dialogszene intro.abl";

/// Checksum over the 6000 checksums of the golden scene (FNV-1a 64 over the checksums in little
/// endian, like `KI_GOLDEN`): the same in debug and release. Measured (reference edition only).
/// Changed: world actions and conditions, level container ID 1; block sequence before/after in
/// re/scripts 16.4.
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: the two changes above combined: the value with world actions and conditions plus serialization of
/// `accounting` and `countdowns` (counter-check: with `serde(skip)` on both, the value with world actions and
/// conditions).
/// Changed: script commands, AI start, victory and defeat from scripts; block sequence
/// before/after in re/scripts 17.4 (counter-check: without the command path, the AI actions
/// 0/1/37/340 and the direct halt 174 of the scripts and with `serde(skip)` on the skip flag
/// `Container::skipped`, the preceding values; the flag alone changes `SCRIPT_GOLDEN`, not
/// `SCRIPT_BLOCKS`).
/// Changed: presentation state, mirror, profile variables (serialization) and effect of the
/// presentation actions. Counter-check: with `serde(skip)` on the six new fields of
/// `MissionState`, the preceding value `0xc662_e8c1_4832_17e1`.
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26);
/// behavior the same (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and
/// `time_limit`, FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all
/// values unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same;
/// counter-check with `serde(skip)`: all values unchanged).
const SCRIPT_GOLDEN: u64 = 0xe26c_c575_e48e_2e30;

/// Key figure: FNV-1a 64 over all `ScriptTrace::Fired { container, block }` of the scene in the
/// reported sequence. 16 bytes per message, little endian: `Step` (u64, 1-based like the count of
/// the checksums, i.e. the step in which it is reported), `Container` (i32, -1 = dialog) and
/// `Block` (u32, index in the file). Measured (reference edition only).
/// Changed: world actions and conditions, level container ID 1; block sequence before/after in
/// re/scripts 16.4.
/// Changed: script commands, AI start, victory and defeat from scripts; block sequence
/// before/after in re/scripts 17.4.
/// Measured again, unchanged: the presentation actions do not change the block sequence (the same
/// 81 messages).
const SCRIPT_BLOCKS: u64 = 0x6ff9_5abc_ffe5_ba7f;

/// Rules of the map; `None` without original installation or in the English edition (the
/// checksums apply only to the reference edition, German with patch).
fn rules_for(level: &str) -> Option<Rules> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    if version != GameVersion::German {
        eprintln!("English edition - SCRIPT_GOLDEN applies only to the reference edition, skipped");
        return None;
    }
    let scene = load_level(&vfs, level).unwrap();
    Some(Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{level}: {e:#}")))
}

/// Golden scene: `kamp1-1`, seed [`SEED`], AI on, difficulty 1, script on, 55 ms per character.
fn golden_setup(rules: &Rules) -> MatchSetup {
    let mut setup = MatchSetup::default_for(rules);
    setup.seed = SEED;
    setup.ki.enabled = true;
    setup.ki.campaign_difficulty = 1;
    setup.script.enabled = true;
    setup.script.time_per_char = 55;
    setup
}

/// `STEPS` steps from `new_match(setup)` without commands; after every step the checksum and
/// `on_step(step, state, events)`. After every step in `save_at` the game is saved and loaded with
/// check.
fn play(
    rules: &Rules,
    setup: &MatchSetup,
    save_at: &[usize],
    mut on_step: impl FnMut(u64, &SimState, &[SimEvent]),
) -> (Vec<u64>, SimState) {
    let mut state = new_match(rules, setup).unwrap();
    let mut sums = Vec::with_capacity(STEPS);
    for n in 1..=STEPS {
        let events = step(&mut state, rules, &[]);
        sums.push(checksum(&state));
        on_step(n as u64, &state, &events);
        if save_at.contains(&n) {
            let save = SaveGame {
                start: start_package(rules, setup),
                state: state.clone(),
                commands: Vec::new(),
            };
            let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
            loaded.check(rules).unwrap();
            assert_eq!(checksum(&loaded.state), checksum(&state));
            state = loaded.state;
        }
    }
    (sums, state)
}

/// Checksum of a sequence like for the other golden checksums (FNV-1a 64 over the checksums in
/// little endian).
fn fold(sums: &[u64]) -> u64 {
    let mut h = Fnv64::default();
    for c in sums {
        h.write(&c.to_le_bytes());
    }
    h.0
}

/// The key figure [`SCRIPT_BLOCKS`] (encoding there) and the number of messages.
#[derive(Default)]
struct Blocks {
    hash: Fnv64,
    fired: u64,
    in_dialog: u64,
    first_dialog_step: Option<u64>,
}

impl Blocks {
    fn feed(&mut self, step: u64, events: &[SimEvent]) {
        for e in events {
            let SimEvent::Script(ScriptTrace::Fired { container, block }) = e else {
                continue;
            };
            self.hash.write(&step.to_le_bytes());
            self.hash.write(&container.to_le_bytes());
            self.hash.write(&block.to_le_bytes());
            self.fired += 1;
            if *container < 0 {
                self.in_dialog += 1;
                self.first_dialog_step.get_or_insert(step);
            }
        }
    }
}

mod checksums {
    use super::*;

    // QUIRK E-140: deviation
    #[test]
    fn script_golden() {
        let Some(rules) = rules_for(LEVEL) else {
            return;
        };
        let setup = golden_setup(&rules);
        // Counter-checks before the golden.
        let state = new_match(&rules, &setup).unwrap();
        let level = state.script.level.as_ref().expect("level container");
        assert_eq!(level.id, nml_sim::script::LEVEL_CONTAINER_ID);
        assert!(state.ki.engines.iter().any(Option::is_some), "no AI engine");
        let mut blocks = Blocks::default();
        let mut dialog_started = Vec::new();
        let (sums, end) = play(&rules, &setup, &[], |n, _, events| {
            blocks.feed(n, events);
            for e in events {
                if let SimEvent::Script(ScriptTrace::DialogStarted { path }) = e {
                    dialog_started.push((n, path.clone()));
                }
            }
        });
        eprintln!(
            "Dialog starts: {dialog_started:?}; Fired {} (dialog {}, first in dialog: {:?}); \
             steps {}",
            blocks.fired, blocks.in_dialog, blocks.first_dialog_step, end.steps
        );
        assert_eq!(
            dialog_started.first(),
            Some(&(1, INTRO_DIALOG.to_string())),
            "Dialog \"dialogszene intro\" starts in step 1"
        );
        assert!(blocks.in_dialog >= 1, "no block ran in the dialog");
        let h = fold(&sums);
        eprintln!(
            "SCRIPT_GOLDEN {h:#018x}, SCRIPT_BLOCKS {:#018x}",
            blocks.hash.0
        );
        assert_eq!(h, SCRIPT_GOLDEN, "{h:#018x}");
        assert_eq!(blocks.hash.0, SCRIPT_BLOCKS, "{:#018x}", blocks.hash.0);
    }

    /// After step 3000 or in the middle of the intro (cinemascope on, running line) saved and loaded
    /// with check: the same sequence and the same block messages as without.
    #[test]
    fn script_golden_survives_saving_and_loading() {
        let Some(rules) = rules_for(LEVEL) else {
            return;
        };
        let setup = golden_setup(&rules);
        for save_at in [SAVE_IN_INTRO, SAVE_AT] {
            let mut blocks = Blocks::default();
            let (saved, _) = play(&rules, &setup, &[save_at], |n, state, events| {
                blocks.feed(n, events);
                if n == SAVE_IN_INTRO as u64 {
                    let mission = &state.script.mission;
                    assert!(mission.cinemascope, "cinemascope in step {n}");
                    assert!(mission.speech.line.is_some(), "no running line in step {n}");
                }
            });
            assert_eq!(fold(&saved), SCRIPT_GOLDEN, "saved after {save_at}");
            assert_eq!(blocks.hash.0, SCRIPT_BLOCKS, "saved after {save_at}");
        }
    }
}
