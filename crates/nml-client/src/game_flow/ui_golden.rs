//! Golden checksums `UI_GOLDEN`: the whole circle without graphics via `MenuApp` + [`GameFlow`] + `FakeSession` with a
//! real simulation (`real_levels`) on the corpus dialogs, with the fixed program clock of the harness from
//! `game_flow/tests.rs` (16 ms per frame). Main menu -> create a profile -> own game on [`TWO_PLAYERS`] with a computer
//! -> 200 steps -> quicksave, 100 steps, quickload -> victory via `sim_result` -> end screen -> accounting with all
//! tabs -> main menu -> campaign 1, video `SPA-1` (fake [`fake_opener`]) with automatic start of `kamp1-1` -> victory
//! -> accounting -> "Continue" starts the next level.
//!
//! Per section an FNV-1a-64 checksum over the states at its end ([`Snapshot`]): names of the dialog stack, length of
//! the queue, game menu state, checksum of the simulation (if a match is running) and the rows of the accounting with
//! game time (if it is open). Corpus test (`NML_GAME_DIR`, skipped silently without it); the values hold for the
//! German edition with patch (reference), the English one only checks "the same twice".

use std::cell::RefCell;
use std::rc::Rc;

use nml_control::{Command, UiElementData, UiStateData};
use nml_rda::testutil::{GameVersion, game_version};
use nml_ui::app::MenuOptions;
use nml_ui::input::{MouseButton, UiInput};
use nml_ui::saves::{SaveDir, SaveFiles};

use super::net_tests::{
    CHANGE, Lan, NET_MENU, in_accounting, in_main_menu, lan_match, leave_accounting, message,
    random_values, start_shown,
};
use super::tests::{KAMP1_1, Run, S1024, TWO_PLAYERS};
use super::*;
use crate::game::sim_control::execute_sim;
use crate::testutil::{FakeVideoLog, fake_opener};
use crate::ui_control::state_data;
use nml_net::MapId;

/// Measured (German edition with patch, debug = release, the same twice); measured again later (only the
/// sections with a running match, via the simulation checksum). Per section of the circle FNV-1a 64 over the states at
/// its end: dialog stack (names as text), length of the queue, game menu state (`Engine+0x2CCC`), checksum of the
/// simulation (if a match is running) and accounting (rows and game time, if `accounting` is open); `accounting_tabs`
/// over all four tab states. If a value changes, find the cause (UI, flow or simulation - the 19 simulation sequences
/// show whether it is the simulation).
/// Changed: script commands, AI start, victory and defeat from scripts; block sequence before/after in re/scripts
/// 17.4. Only the campaign sections `campaign_video` and `campaign_level_won` changed (counter-check: without the
/// command path, the AI actions 0/1/37/340 and the direct halt 174 of the scripts and with `serde(skip)` on the skip
/// flag of the container the old values `0x3B22E98F4F4032C8` and `0xC4552DE31DF60B5A`).
/// Changed: presentation state, mirror, profile variables (serialization) and effect of the presentation actions. All
/// sections with a running match are new (via the simulation checksum); `profile`, `accounting_tabs` and `main_menu`
/// stay. Counter-check: with `serde(skip)` on the six new fields of `MissionState` and without the game speed switch
/// of the cinemascope exactly the values before this change (the campaign sections do not reach the cinemascope
/// from step 2, the game speed has no effect here).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26). The six sections with
/// a running match are new (via the simulation checksum); `profile`, `accounting_tabs` and `main_menu` stay.
/// Counter-check: with `serde(skip)` on the new field exactly the values before this change (debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and `time_limit`,
/// FORMAT_VERSION 27); behavior the same. The sections with a running match are measured again (`create_game`,
/// `match_200_steps`, `save_load`, `victory`, `campaign_video`, `campaign_level_won`, via the simulation checksum);
/// `profile`, `accounting_tabs` and `main_menu` stay. Counter-check: with `serde(skip)` on the two new fields exactly
/// the values before this change.
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same). The same six sections
/// with a running match are measured again (via the simulation checksum); `profile`, `accounting_tabs` and `main_menu`
/// stay. Counter-check: with `serde(skip)` on `SimState::network` exactly the values before this change.
/// Added: the tenth section `net_lobby_start` ([`net_lobby_start`]) with two machines in the process (measured German
/// edition with patch, debug = release, the same twice); the nine old values are unchanged.
/// Added: sections eleven and twelve, `net_change_random` ([`net_change_random`]: map change via `LAN31` to a random
/// map with seed text "4711", start, 200 ticks) and `net_match_end` ([`net_match_end`]: network game menu, diplomacy,
/// `QUITGAME`, defeat and victory through the accounting to the main menu); measured German edition with patch, debug
/// = release, the same twice. Every step changes the hashed state ([`LanTrace`]). The ten old values are unchanged.
/// Added: the thirteenth section `menu_level` ([`menu_level_section`]): `IsMenuLevel 1`, menu level behind `START`,
/// submenu with halt, `OPTIONS`, profile and campaign 1 via the menu path (the start from the submenu ends it while the
/// menu level is halted), game end, restart with tick 0. The [`Snapshot`] takes visibility, ticks and checksum of the
/// menu level only while it runs. Measured German edition with patch, debug = release, the same twice. The twelve old
/// sections run with `IsMenuLevel 0` (`testutil::test_config`) and are unchanged.
/// Added: the fourteenth section `net_load_saved` ([`net_load_saved`], behind `net_match_end`): network match, saving
/// via `SAVEGAME2` with a typed name, `QUITGAME` of the host to the main menu, loading in `LAN3`, join to `LAN4`,
/// `START`, 200 ticks from the tick of the savegame with equal checksums. Measured German edition with patch, debug =
/// release, the same twice. [`hash_lan`] takes the slots of a loaded lobby only if there are any; the thirteen old
/// values are unchanged.
/// Added: the fifteenth section `replay` ([`replay_section`], behind `menu_level`): match with `SAVEREPLAY`, 200 ticks
/// with walk commands, `QUITGAME` to the main menu, `LOADAUTOPLAY` -> `START`, F1 without effect, 200 ticks of the
/// replay with the checksum of the match at tick 200. The [`Snapshot`] takes `ui_state.game.replay` only while a
/// recording or replay runs. Measured German edition with patch, debug = release, the same twice; the fourteen old
/// values are unchanged.
/// Changed (intended): `QUITGAME` of the host is now, as for the client, its own defeat instead of the end for all, and
/// `END_*` of the host hands the match over as long as a reachable client is there. The end part of `net_load_saved`
/// therefore no longer reaches "message on both"; it now runs like this: Bert `QUITGAME` -> defeat -> `END_DEFEAT` ->
/// accounting, Anna `END_VICTORY` -> end for all (no active client anymore) -> message -> accounting. Only
/// `net_load_saved` is measured again (German edition with patch, debug = release, the same twice); the fourteen other
/// values are unchanged.
/// Added: the sixteenth section `net_host_migration` ([`net_host_migration`], behind `replay`): three machines via
/// `MemTransport` from the menu, start, Anna (host) gives up and leaves the end screen, handover to Bert, 200 ticks on
/// Bert and Cara with equal checksums. Measured German edition with patch, debug = release, the same twice; the fifteen
/// other values are unchanged.
const UI_GOLDEN: [(&str, u64); 16] = [
    ("profile", 0xB4A04C43CD99F1AA),
    ("create_game", 0x1BD2E55D8426EFAC),
    ("match_200_steps", 0xF29AC29C452E2345),
    ("save_load", 0xE3009674588339BE),
    ("victory", 0xFC3F4BD818DF4C6C),
    ("accounting_tabs", 0x8922E71BCB64B0D5),
    ("main_menu", 0xB9A5AAA395D36BA8),
    ("campaign_video", 0x6AD79EEAFA195FBB),
    ("campaign_level_won", 0xD71A1D9056258B44),
    ("net_lobby_start", 0xB57192C28EDD4531),
    ("net_change_random", 0xBFAD861C11AC4D23),
    ("net_match_end", 0x8B2DA8D6A7C24F16),
    ("net_load_saved", 0x029233175ADAE7DC),
    ("menu_level", 0x29F789ECEC8CF1DA),
    ("replay", 0x4EF40E9395F25B65),
    ("net_host_migration", 0x0F847567C79E3540),
];

/// Name of the profile in the circle.
const PROFILE: &str = "Anna";
/// Second level of campaign 1 (after [`KAMP1_1`], the first after the video `SPA-1`).
const KAMP1_2: &str = "data/levels/kampagne1/kamp1-2/kamp1-2.lvl";

/// FNV-1a 64 step by step.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
        self.bytes(&[0]);
    }
}

/// State of the circle at one point in time (content of the checksum).
#[derive(Debug)]
struct Snapshot {
    dialogs: Vec<String>,
    queued: usize,
    menu_state: u8,
    sim: Option<u64>,
    accounting: Option<(Vec<nml_control::UiAccountingRow>, Option<String>)>,
    /// Menu level (visible, ticks, checksum); only `Some` while it runs - the old sections have none and stay the
    /// same this way.
    menu_level: Option<(bool, u64, String)>,
    /// Recording or replay (`ui_state.game.replay`); only `Some` while one runs - the old sections do not record and
    /// stay the same this way.
    replay: Option<nml_control::UiReplayData>,
}

impl Snapshot {
    fn of(r: &Run) -> Self {
        let state = state_data(&r.host.app, S1024, r.now);
        Self {
            dialogs: state.dialogs.into_iter().map(|d| d.name).collect(),
            queued: state.queued,
            menu_state: r.host.app.game_menu_state(),
            sim: r.flow.running().then(|| r.view.checksum()),
            accounting: state.accounting.map(|rows| (rows, state.play_time)),
            menu_level: r
                .flow
                .menu_level_data(r.view.sim())
                .map(|d| (d.shown, d.steps, d.checksum)),
            replay: r
                .flow
                .game_data(
                    &r.host.app,
                    r.view.sim(),
                    r.view.level(),
                    None,
                    None,
                    None,
                    None,
                    Vec::new(),
                )
                .and_then(|d| d.replay),
        }
    }

    fn hash_into(&self, h: &mut Fnv) {
        h.bytes(&(self.dialogs.len() as u64).to_le_bytes());
        for name in &self.dialogs {
            h.text(name);
        }
        h.bytes(&(self.queued as u64).to_le_bytes());
        h.bytes(&[self.menu_state]);
        match self.sim {
            Some(sum) => {
                h.bytes(&[1]);
                h.bytes(&sum.to_le_bytes());
            }
            None => h.bytes(&[0]),
        }
        match &self.accounting {
            Some((rows, time)) => {
                h.bytes(&[1]);
                h.bytes(&(rows.len() as u64).to_le_bytes());
                for row in rows {
                    h.text(&row.name);
                    h.bytes(&row.color.to_le_bytes());
                    for value in &row.values {
                        h.text(value);
                    }
                }
                h.text(time.as_deref().unwrap_or_default());
            }
            None => h.bytes(&[0]),
        }
        // Added only with a menu level, so that the sections without it keep their values.
        if let Some((shown, steps, checksum)) = &self.menu_level {
            h.bytes(&[2, u8::from(*shown)]);
            h.bytes(&steps.to_le_bytes());
            h.text(checksum);
        }
        // Likewise only with a recording or replay.
        if let Some(replay) = &self.replay {
            let mode = match replay.mode {
                nml_control::UiReplayMode::Record => 0,
                nml_control::UiReplayMode::Play => 1,
            };
            h.bytes(&[3, mode, u8::from(replay.ended)]);
            h.text(&replay.file);
            h.bytes(&replay.step.to_le_bytes());
        }
    }
}

/// The circle: host with profiles from the circle, section checksums.
struct Circle {
    r: Run,
    sections: Vec<(&'static str, u64)>,
    hash: Fnv,
}

impl Circle {
    fn new() -> Option<Self> {
        Self::configured(|_| {})
    }

    /// Like [`Self::new`], with the settings changed by `adjust` (section `menu_level`: `IsMenuLevel 1`).
    fn configured(adjust: impl FnOnce(&mut EngineConfig)) -> Option<Self> {
        let mut r = Run::corpus_configured(
            MenuOptions {
                start_menu: true,
                ..MenuOptions::default()
            },
            adjust,
        )?;
        r.view.real_levels = true;
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        r.host.set_video_opener(fake_opener(&log, false, None));
        Some(Self {
            r,
            sections: Vec::new(),
            hash: Fnv::new(),
        })
    }

    /// State now into the checksum of the running section.
    fn snap(&mut self) {
        let snapshot = Snapshot::of(&self.r);
        if std::env::var_os("NML_UI_GOLDEN_TRACE").is_some() {
            eprintln!("  {snapshot:?}");
        }
        snapshot.hash_into(&mut self.hash);
    }

    /// Close the section with the current state.
    fn section(&mut self, name: &'static str) {
        self.snap();
        let hash = std::mem::replace(&mut self.hash, Fnv::new());
        if std::env::var_os("NML_UI_GOLDEN_TRACE").is_some() {
            eprintln!("{name}: 0x{:016X}", hash.0);
        }
        self.sections.push((name, hash.0));
    }

    fn state(&self) -> UiStateData {
        state_data(&self.r.host.app, S1024, self.r.now)
    }

    fn dialogs(&self) -> Vec<String> {
        let mut names = self.r.dialogs();
        names.sort();
        names
    }

    fn element(&self, dialog: &str, name: &str) -> UiElementData {
        let state = self.state();
        state
            .elements
            .iter()
            .find(|e| e.dialog == dialog && e.name == name)
            .cloned()
            .unwrap_or_else(|| {
                let open: Vec<&str> = state.dialogs.iter().map(|d| d.name.as_str()).collect();
                panic!("{dialog}/{name} is missing; open: {open:?}")
            })
    }

    /// `ui_click` and wait until the UI is at rest.
    fn go(&mut self, dialog: &str, element: &str) {
        self.r.click(dialog, element);
        self.r.idle();
    }

    /// Click at `(x, y)` like a window message (list and combo box rows have no name).
    fn click_at(&mut self, x: i32, y: i32) {
        let app = &mut self.r.host.app;
        app.input(UiInput::Move { x, y }, S1024, self.r.now);
        app.input(UiInput::Down(MouseButton::Left), S1024, self.r.now);
        app.input(UiInput::Up(MouseButton::Left), S1024, self.r.now);
        self.r.idle();
    }

    /// Type text (uppercase letters as key name, the character in cp1252), one frame per character.
    fn typ(&mut self, text: &str) {
        for c in text.chars() {
            self.r.ui(Command::UiKey {
                chord: c.to_ascii_uppercase().to_string(),
                char: Some(c as u16),
            });
            self.r.frame();
        }
        self.r.idle();
    }

    /// Select row `row` of a ListBox (row height 22 px).
    fn list_row(&mut self, dialog: &str, list: &str, row: usize) {
        let e = self.element(dialog, list);
        let [x0, y0, x1, _] = e.rect;
        self.click_at((x0 + x1) / 2, y0 + 11 + 22 * row as i32);
        assert_eq!(
            self.element(dialog, list).selection,
            Some(row as i32),
            "{dialog}/{list}: row {row} of {:?}",
            e.items
        );
    }

    /// Select entry `text` of a combo box: open it, then the row (height 21 px, offset 6).
    fn combo(&mut self, dialog: &str, combo: &str, text: &str) {
        self.go(dialog, combo);
        let e = self.element(dialog, combo);
        let k = e
            .items
            .iter()
            .position(|i| i == text)
            .unwrap_or_else(|| panic!("{text} is missing in {:?}", e.items)) as i32;
        let [x0, y0, x1, y1] = e.rect;
        self.click_at((x0 + x1) / 2, (y0 + y1) / 2 + 6 + 21 * k);
        assert_eq!(self.element(dialog, combo).selection, Some(k));
    }

    /// Frames until the simulation has at least `steps` steps.
    fn run_to(&mut self, steps: u64) {
        let until = self.r.now + 60_000;
        while self.r.view.steps() < steps {
            assert!(
                self.r.now < until,
                "simulation does not reach {steps} steps"
            );
            self.r.frame();
        }
    }

    /// Frames until the game menu state is `state`.
    fn until_state(&mut self, state: u8) {
        let until = self.r.now + 10_000;
        while self.r.host.app.game_menu_state() != state {
            assert!(self.r.now < until, "state {state} does not arrive");
            self.r.frame();
        }
        self.r.idle();
    }

    /// `sim_result` victory for the local player via the control channel, then until the end screen.
    fn win(&mut self) {
        let slot = self.r.host.app.game().expect("match").local_slot;
        execute_sim(
            self.r.view.sim_mut(),
            &Command::SimResult {
                player: slot,
                result: 2,
            },
        )
        .unwrap();
        self.until_state(6);
        assert_eq!(self.r.dialogs(), ["gameend_victory"]);
    }

    /// Match is running and the UI is at rest (after loading two rounds, like `nml-game`).
    fn settle_match(&mut self) {
        assert!(self.r.flow.running(), "match running");
        self.r.flow.begin_round();
        self.r.flow.begin_round();
        self.r.idle();
    }
}

/// The whole circle; checksum per section.
fn circle() -> Option<Vec<(&'static str, u64)>> {
    let mut c = Circle::new()?;
    let r = &mut c;

    // profile: main menu -> SINGLEPLAYERMENU without a profile -> SELECTPROFILE, type the name, CREATE, OK.
    r.r.idle();
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
    r.go("start", "singleplayermenu");
    assert_eq!(r.dialogs(), ["selectprofile"]);
    r.go("selectprofile", "profilename");
    r.typ(PROFILE);
    r.go("selectprofile", "create");
    assert_eq!(r.element("selectprofile", "profilelist").items, [PROFILE]);
    r.go("selectprofile", "ok");
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    r.section("profile");

    // create_game: own game, game mode 0, map TWO_PLAYERS, slot 1 "Computer (Normal)", START2.
    r.go("singleplayermenu", "createsingleplayergame");
    assert_eq!(
        r.dialogs(),
        ["createsingleplayergame", "loadmultiplayermap"]
    );
    let levels = r.element("loadmultiplayermap", "levels");
    let row = levels
        .items
        .iter()
        .position(|i| i.to_lowercase().contains("tinte"))
        .expect("map Tintebariver");
    r.list_row("loadmultiplayermap", "levels", row);
    r.go("createsingleplayergame", "create2");
    assert_eq!(r.dialogs(), ["levelinfo", "singleplayerstart"]);
    r.combo("singleplayerstart", "name1", "Computer (Normal)");
    r.go("singleplayerstart", "start2");
    r.settle_match();
    assert!(r.r.view.level().eq_ignore_ascii_case(TWO_PLAYERS));
    r.section("create_game");

    // match_200_steps.
    r.run_to(200);
    r.section("match_200_steps");

    // save_load: F1 (quicksave1), 100 steps, F4 -> INGAMELOADGAME, row quicksave1, LOAD -> state as when saving.
    r.r.key("F1");
    r.r.idle();
    let saved = (r.r.view.steps(), r.r.view.checksum());
    let guid = active_guid(&r.r.host.app);
    assert!(r.r.files.exists(SaveDir::Main, guid, "quicksave1"));
    r.snap();
    let target = saved.0 + 100;
    r.run_to(target);
    r.snap();
    r.r.key("F4");
    r.r.idle();
    assert_eq!(r.dialogs(), ["ingameloadgame"]);
    let list = r.element("ingameloadgame", "dir");
    let row = list
        .items
        .iter()
        .position(|i| i.ends_with("quicksave1"))
        .unwrap_or_else(|| panic!("quicksave1 is missing in {:?}", list.items));
    r.list_row("ingameloadgame", "dir", row);
    r.go("ingameloadgame", "load");
    r.settle_match();
    assert_eq!((r.r.view.steps(), r.r.view.checksum()), saved);
    r.section("save_load");

    // victory: sim_result victory -> GAMEEND_VICTORY (state 6).
    r.r.until_step();
    r.win();
    r.section("victory");

    // accounting_tabs: END_VICTORY -> ACCOUNTING with the tab "Military", then all three tabs.
    r.go("gameend_victory", "end_victory");
    assert_eq!(r.dialogs(), ["account1", "accounting"]);
    r.snap();
    for tab in ["account0", "account2", "account1"] {
        r.go("accounting", tab);
        assert!(r.dialogs().contains(&tab.to_string()), "{:?}", r.dialogs());
        r.snap();
    }
    r.section("accounting_tabs");

    // main_menu: MAINMENU -> main menu.
    r.go("accounting", "mainmenu");
    assert!(
        r.dialogs().contains(&"start".to_string()),
        "{:?}",
        r.dialogs()
    );
    r.section("main_menu");

    // campaign_video: SINGLEPLAYERMENU -> SELECTCAMPAIGN -> campaign 1 (entry 1 = video SPA-1) -> VIDEOSTART: booking,
    // video, `kamp1-1` becomes free and starts without a dialog (`0x28`, automatic continue).
    r.go("start", "singleplayermenu");
    r.go("singleplayermenu", "selectcampaign");
    assert_eq!(r.dialogs(), ["kampagne0", "selectcampaign"]);
    r.go("selectcampaign", "kampagne1");
    assert_eq!(
        r.dialogs(),
        ["campaignvideo", "kampagne1", "selectcampaign"]
    );
    r.go("campaignvideo", "videostart");
    r.settle_match();
    assert!(r.r.view.level().eq_ignore_ascii_case(KAMP1_1));
    assert_eq!(r.r.host.app.game().map(|g| g.game_type), Some(-1));
    r.section("campaign_video");

    // campaign_level_won: sim_result victory -> booking -> END_VICTORY -> ACCOUNTING -> "Continue" starts the next
    // level.
    r.r.until_step();
    r.win();
    r.go("gameend_victory", "end_victory");
    r.snap();
    r.go("accounting", "quit");
    r.settle_match();
    assert!(r.dialogs().is_empty(), "{:?}", r.dialogs());
    assert!(r.r.view.level().eq_ignore_ascii_case(KAMP1_2));
    r.section("campaign_level_won");

    c.sections.push(("net_lobby_start", net_lobby_start()?));
    c.sections.push(("net_change_random", net_change_random()?));
    c.sections.push(("net_match_end", net_match_end()?));
    c.sections.push(("net_load_saved", net_load_saved()?));
    c.sections.push(("menu_level", menu_level_section()?));
    c.sections.push(("replay", replay_section()?));
    c.sections
        .push(("net_host_migration", net_host_migration()?));
    Some(c.sections)
}

/// State of both machines of the network lobby into the checksum: dialog stack, lobby state (postcard), chat lines of
/// the lobby and, once the match runs, the simulation checksum.
fn snap_lan(n: &Lan, h: &mut Fnv) {
    hash_lan(n, h, true);
}

/// Like [`snap_lan`]; with `sim = false` without the simulation checksum (menu state only: dialogs, lobby state, chat).
fn hash_lan(n: &Lan, h: &mut Fnv, sim: bool) {
    for (i, run) in n.runs.iter().enumerate() {
        let state = state_data(&run.host.app, S1024, run.now);
        h.bytes(&(state.dialogs.len() as u64).to_le_bytes());
        for d in &state.dialogs {
            h.text(&d.name);
        }
        match n.session(i).status().lobby {
            Some(lobby) => {
                h.bytes(&[1]);
                // The fields from before loaded lobbies existed as a tuple in their order (the same bytes as the
                // struct then), the slots of a loaded lobby only if there are any: this way the old sections stay
                // the same.
                let old = (
                    lobby.revision,
                    &lobby.session,
                    &lobby.rows,
                    lobby.max_rtt_ms,
                );
                h.bytes(&postcard::to_allocvec(&old).expect("lobby state"));
                if !lobby.seats.is_empty() {
                    h.bytes(&postcard::to_allocvec(&lobby.seats).expect("slots"));
                }
            }
            None => h.bytes(&[0]),
        }
        let chat = run
            .host
            .app
            .data()
            .net
            .as_ref()
            .and_then(|v| v.lobby.as_ref())
            .map(|l| l.chat.clone())
            .unwrap_or_default();
        h.bytes(&(chat.len() as u64).to_le_bytes());
        for line in &chat {
            h.text(line);
        }
        if !sim {
            continue;
        }
        match run.flow.running().then(|| run.view.checksum()) {
            Some(sum) => {
                h.bytes(&[1]);
                h.bytes(&sum.to_le_bytes());
            }
            None => h.bytes(&[0]),
        }
    }
}

/// Section `net_lobby_start`: host "Anna" and client "Bert" via `MemTransport` (fixed clock 16 ms per frame, fixed
/// seed): `LAN1` -> `LAN3` with the first multiplayer map of the list -> `LAN2`, Bert joins, both chat, Bert changes
/// his nation, both ready, `START`, 200 ticks. Checksum over dialog stack, lobby states and chat lines of both after
/// every step and finally over the checksums of both simulations at tick 200.
fn net_lobby_start() -> Option<u64> {
    use nml_ui::msg::LobbyAction;
    let mut n = Lan::corpus(&["Anna", "Bert"])?;
    let mut h = Fnv::new();
    n.host(0, None);
    snap_lan(&n, &mut h);
    n.join(1);
    snap_lan(&n, &mut h);
    n.wish(0, LobbyAction::Chat("Hallo Bert".into()));
    n.wish(1, LobbyAction::Chat("Hallo Anna".into()));
    let chat_lines = |n: &Lan, i: usize| {
        n.runs[i]
            .host
            .app
            .data()
            .net
            .as_ref()
            .and_then(|v| v.lobby.as_ref())
            .map_or(0, |l| l.chat.len())
    };
    n.frames_until("chat on both", |n| {
        chat_lines(n, 0) == 2 && chat_lines(n, 1) == 2
    });
    n.settle();
    snap_lan(&n, &mut h);
    let row = n.lobby(1).rows[1].clone();
    let nation = if row.nation == 1 { 2 } else { 1 };
    n.wish(
        1,
        LobbyAction::Row {
            row: 1,
            nation,
            team: row.team,
            handicap: row.handicap,
            color: row.color,
        },
    );
    n.frames_until("nation on the host", |n| {
        n.lobby(0).rows[1].nation == nation
    });
    n.settle();
    snap_lan(&n, &mut h);
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", |n| {
        let run = &n.runs[0];
        state_data(&run.host.app, S1024, run.now)
            .elements
            .iter()
            .any(|e| e.dialog == "lan2" && e.name == "start" && e.visible && !e.hidden)
    });
    snap_lan(&n, &mut h);
    n.runs[0].click("lan2", "start");
    n.frames_until("200 ticks", |n| {
        n.runs.iter().all(|r| r.view.steps() >= 200)
    });
    snap_lan(&n, &mut h);
    let sums: Vec<u64> = n
        .runs
        .iter()
        .map(|r| {
            r.view
                .sim()
                .and_then(|s| s.hash_at(200))
                .expect("checksum at tick 200")
        })
        .collect();
    assert_eq!(sums[0], sums[1], "both machines compute equally");
    for sum in sums {
        h.bytes(&sum.to_le_bytes());
    }
    Some(h.0)
}

/// Checksum of a network section step by step via [`snap_lan`]. Every step must change the hashed state, otherwise it
/// would be meaningless in the checksum: a menu step the menu state without simulation (dialogs, lobby state, chat), a
/// tick step the whole state.
struct LanTrace {
    h: Fnv,
    menu: Option<u64>,
    full: Option<u64>,
}

impl LanTrace {
    fn new() -> Self {
        Self {
            h: Fnv::new(),
            menu: None,
            full: None,
        }
    }

    fn digest(n: &Lan, sim: bool) -> u64 {
        let mut h = Fnv::new();
        hash_lan(n, &mut h, sim);
        h.0
    }

    /// Menu step: dialog stack, lobby state or chat of a machine have changed.
    fn menu(&mut self, n: &Lan, step: &str) {
        let menu = Self::digest(n, false);
        assert_ne!(self.menu, Some(menu), "{step}: menu state unchanged");
        self.record(n, step);
    }

    /// Tick step: the simulation has run on (the menu state may stay the same).
    fn ticks(&mut self, n: &Lan, step: &str) {
        let full = Self::digest(n, true);
        assert_ne!(self.full, Some(full), "{step}: state unchanged");
        self.record(n, step);
    }

    fn record(&mut self, n: &Lan, step: &str) {
        self.menu = Some(Self::digest(n, false));
        self.full = Some(Self::digest(n, true));
        snap_lan(n, &mut self.h);
        if std::env::var_os("NML_UI_GOLDEN_TRACE").is_some() {
            eprintln!("  {step}: {:?} | {:?}", n.dialogs(0), n.dialogs(1));
        }
    }
}

/// Section `net_change_random`: host "Anna" and client "Bert" via `MemTransport` (fixed clock, fixed seed) in the lobby
/// on the fixed map "Tintebariver" -> Anna `CHANGEGAME` -> `LAN31` -> random map (seed text "4711" from the settings,
/// small, two players) -> `CHANGEGAME` -> Bert sees the new session -> both ready -> `START` -> 200 ticks.
/// [`snap_lan`] after every step, finally the checksums of both simulations at tick 200 (equal).
fn net_change_random() -> Option<u64> {
    let mut n = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            random_values(cfg, "4711");
        }
    })?;
    let mut t = LanTrace::new();
    n.host(0, Some("tinte"));
    assert!(matches!(n.lobby(0).session.map, MapId::Fixed { .. }));
    t.menu(&n, "lobby of the host");
    n.join(1);
    t.menu(&n, "Bert joins");
    n.open_change();
    t.menu(&n, "LAN31");
    n.combo_row(0, CHANGE, "leveltype", 1);
    assert!(
        n.dialogs(0).iter().any(|d| d == "randommap"),
        "{:?}",
        n.dialogs(0)
    );
    t.menu(&n, "random map");
    n.change_game();
    let session = n.lobby(0).session;
    assert_eq!(
        session.map,
        MapId::Random {
            seed_text: "4711".into(),
            size: 0,
            players: 2,
            terrain: 0,
            landscape: 0,
            daytime: 0,
        }
    );
    // Bert already has the new lobby state when the host's UI is at rest after the click (10 ms delay, the frames
    // until rest suffice): one step for both, otherwise "Bert sees the new session" would have nothing of its own.
    n.frames_until("state on the client", |l| l.lobby(1).session == session);
    n.settle();
    assert!(n.in_lobby(1), "{:?}", n.dialogs(1));
    assert_eq!(n.runs[1].host.app.data().draft.info.level_type, 1);
    t.menu(&n, "CHANGEGAME, Bert sees the new session");
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    t.menu(&n, "both ready");
    n.runs[0].click("lan2", "start");
    n.frames_until("200 ticks", |n| {
        n.runs.iter().all(|r| r.view.steps() >= 200)
    });
    for run in &n.runs {
        assert!(run.view.sim().expect("match").rules.random_map.is_some());
    }
    t.ticks(&n, "200 ticks");
    let sums: Vec<u64> = n
        .runs
        .iter()
        .map(|r| {
            r.view
                .sim()
                .and_then(|s| s.hash_at(200))
                .expect("checksum at tick 200")
        })
        .collect();
    assert_eq!(sums[0], sums[1], "both machines compute equally");
    for sum in sums {
        t.h.bytes(&sum.to_le_bytes());
    }
    Some(t.h.0)
}

/// Section `net_match_end`: host "Anna" and client "Bert" on the fixed map "Tintebariver", `START`, 100 ticks; Bert ESC
/// -> `INGAMEOPTIONS2` -> `RETURN2`, opens and closes `DIPLOMATIE`, ESC -> `QUITGAME` -> "OK" -> defeat (through the
/// turn; Anna wins in the same tick) -> `END_DEFEAT` -> `ACCOUNTING`; Anna `END_VICTORY` -> message about the end by
/// the host -> "OK" -> `ACCOUNTING` -> `MAINMENU` -> `START`. [`snap_lan`] after every step (dialog stack of both).
fn net_match_end() -> Option<u64> {
    let mut n = Lan::corpus(&["Anna", "Bert"])?;
    let mut t = LanTrace::new();
    n.host(0, Some("tinte"));
    t.menu(&n, "lobby of the host");
    n.join(1);
    t.menu(&n, "Bert joins");
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    t.menu(&n, "both ready");
    lan_match(&mut n, 100);
    t.ticks(&n, "100 ticks");
    n.escape(1);
    t.menu(&n, "ESC");
    n.click(1, NET_MENU, "return2");
    assert_eq!(n.runs[1].state(), 0);
    assert!(!n.dialogs(1).iter().any(|d| d == NET_MENU));
    t.menu(&n, "RETURN2");
    n.toggle_diplomacy(1);
    assert_eq!(n.runs[1].state(), DIPLOMACY_MENU_STATE);
    assert!(n.dialogs(1).iter().any(|d| d == "diplomatie"));
    t.menu(&n, "diplomacy open");
    n.click(1, "diplomatie", "back");
    assert_eq!(n.runs[1].state(), 0);
    assert!(!n.dialogs(1).iter().any(|d| d == "diplomatie"));
    t.menu(&n, "diplomacy closed");
    n.escape(1);
    t.menu(&n, "ESC before QUITGAME");
    n.click(1, NET_MENU, "quitgame");
    assert_eq!(n.runs[1].host.app.screen_state().info_text, Some(15780));
    t.menu(&n, "QUITGAME");
    n.runs[1].click("infoframe", "ok");
    n.frames_until("confirmation closed", |l| {
        !l.runs[1].dialogs().iter().any(|d| d == "infoframe")
    });
    assert_eq!(n.runs[1].state(), 2, "waiting time after QUITGAME");
    let bert = usize::from(n.runs[1].host.app.game().expect("match").local_slot);
    assert_eq!(
        n.runs[1].view.sim().expect("match").state.players[bert].result,
        0,
        "the defeat goes through the turn"
    );
    assert!(
        !n.dialogs(1)
            .iter()
            .any(|d| d == NET_MENU || d == "infoframe"),
        "{:?}",
        n.dialogs(1)
    );
    t.menu(&n, "OK");
    // Bert is eliminated at the execution tick on both machines; with that Anna wins in the same tick (no opponents
    // left, re/multiplayer-menus L.8): both end screens open together, one step for both.
    n.frames_until("defeat on Bert, victory on Anna", |l| {
        l.runs[1].dialogs().iter().any(|d| d == "gameend_defeat")
            && l.runs[0].dialogs().iter().any(|d| d == "gameend_victory")
    });
    n.settle();
    assert_eq!((n.runs[0].state(), n.runs[1].state()), (6, 7));
    t.menu(&n, "defeat and victory");
    n.click(1, "gameend_defeat", "end_defeat");
    n.frames_until("accounting of Bert", |l| in_accounting(&l.runs[1]));
    n.settle();
    t.menu(&n, "END_DEFEAT");
    // The host with running turns ends in an orderly way (`net.end()`): first the message "The host has ended the
    // match ...", its "OK" (`0x0E`) opens the accounting.
    n.click(0, "gameend_victory", "end_victory");
    n.frames_until("message on Anna", |l| message(&l.runs[0]).is_some());
    n.settle();
    assert!(!n.dialogs(0).iter().any(|d| d == "accounting"));
    t.menu(&n, "END_VICTORY, message");
    n.click(0, "msgbox", "ok");
    n.frames_until("accounting of Anna", |l| in_accounting(&l.runs[0]));
    n.settle();
    t.menu(&n, "OK, accounting of Anna");
    leave_accounting(&mut n.runs[0]);
    n.frames_until("main menu of Anna", |l| in_main_menu(&l.runs[0]));
    n.settle();
    t.menu(&n, "MAINMENU");
    Some(t.h.0)
}

/// Section `net_host_migration`: "Anna" (host), "Bert" and "Cara" via `MemTransport` (fixed clock, fixed seed) on a map
/// with at least three slots, all ready, `START`, 100 ticks -> Anna gives up (end screen, the match goes on), 100 ticks
/// -> `END_DEFEAT` hands the match over, Anna goes into the accounting -> Bert is host -> 200 ticks on Bert and Cara.
/// [`LanTrace`] after every step; the checksum does not depend on addresses or ports ([`hash_lan`]), finally the
/// checksums of Bert and Cara at the same tick (equal).
fn net_host_migration() -> Option<u64> {
    use crate::game::net_driver::NetDriver;
    let is_host = |n: &Lan, i: usize| n.runs[i].view.net().is_some_and(NetDriver::is_host);
    let mut n = Lan::corpus_listening(&["Anna", "Bert", "Cara"], |_| true, |_, _| {})?;
    let mut t = LanTrace::new();
    n.host_by(0, |m| m.players() >= 3);
    t.menu(&n, "lobby of the host");
    n.join(1);
    t.menu(&n, "Bert joins");
    n.join(2);
    t.menu(&n, "Cara joins");
    n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
    n.frames_until("START visible", start_shown);
    t.menu(&n, "all ready");
    n.runs[0].click("lan2", "start");
    n.frames_until("all after Go", |l| {
        (0..3).all(|i| l.phase(i) == NetPhase::Match && l.runs[i].view.steps() >= 100)
    });
    t.ticks(&n, "100 ticks");
    n.surrender_to_the_end_screen(0);
    t.menu(&n, "Anna gives up");
    let at = n.runs[1].view.steps();
    n.frames_until("the others compute on", |l| {
        (1..3).all(|i| l.runs[i].view.steps() >= at + 100)
    });
    assert!(is_host(&n, 0), "Anna stays host until she leaves");
    t.ticks(&n, "100 ticks after giving up");
    n.click(0, "gameend_defeat", "end_defeat");
    n.frames_until("accounting of Anna", |l| in_accounting(&l.runs[0]));
    n.frames_until("Bert is host", |l| is_host(l, 1));
    n.settle();
    assert!(!is_host(&n, 2));
    t.menu(&n, "handover");
    let at = n.runs[1].view.steps().max(n.runs[2].view.steps());
    n.frames_until("200 ticks on the others", |l| {
        (1..3).all(|i| l.runs[i].view.steps() >= at + 200)
    });
    t.ticks(&n, "200 ticks after the handover");
    let sums: Vec<u64> = (1..3)
        .map(|i| {
            n.runs[i]
                .view
                .sim()
                .and_then(|s| s.hash_at(at + 200))
                .expect("checksum at the tick")
        })
        .collect();
    assert_eq!(sums[0], sums[1], "Bert and Cara compute equally");
    for sum in sums {
        t.h.bytes(&sum.to_le_bytes());
    }
    Some(t.h.0)
}

/// Name of the network savegame in the section [`net_load_saved`].
const NET_SAVE: &str = "netz1";

/// Type text into the input field with the focus of machine `i` (uppercase letters as key name, the character in
/// cp1252), one frame per character, then until the UI is at rest. The click that gives the field the focus must have
/// run a frame before (`Lan::click` waits for rest): a frame processes first the keyboard, then the mouse
/// (`MenuApp::frame`, re/menu-system 12.1), characters in the same frame as the click would miss the field.
fn type_text(n: &mut Lan, i: usize, text: &str) {
    for c in text.chars() {
        n.runs[i].ui(Command::UiKey {
            chord: c.to_ascii_uppercase().to_string(),
            char: Some(c as u16),
        });
        n.frame();
    }
    n.settle();
}

/// Section `net_load_saved`: host "Anna" and client "Bert" via `MemTransport` (fixed clock, fixed seed, a savegame
/// folder of its own per machine) on the fixed map "Tintebariver": lobby, `START`, 150 ticks; Anna saves via
/// `SAVEGAME2` under "netz1" (name typed); Bert gives up with `QUITGAME` -> `END_DEFEAT` -> accounting, Anna
/// `END_VICTORY` -> end for all -> message -> accounting; main menu on both. Anna `LAN1` -> `LAN3` -> "Load game" ->
/// "netz1" -> `CREATE` -> `LAN4`; Bert `LAN1` -> `JOIN` -> `LAN4`; Anna `START`, 200 ticks from the tick of the
/// savegame (origin). [`snap_lan`] after every step, finally the checksums of both simulations at tick origin + 200
/// (equal).
fn net_load_saved() -> Option<u64> {
    let mut n = Lan::corpus(&["Anna", "Bert"])?;
    let mut t = LanTrace::new();
    n.host(0, Some("tinte"));
    t.menu(&n, "lobby of the host");
    n.join(1);
    t.menu(&n, "Bert joins");
    n.all_ready(&[(0, 0), (1, 1)]);
    n.frames_until("START visible", start_shown);
    t.menu(&n, "both ready");
    lan_match(&mut n, 150);
    t.ticks(&n, "150 ticks");

    // Saving via `SAVEGAME2` (`0x004D735E`): network folder, name typed in `FILENAME` (name buffer `GameApp+0x177C`),
    // `SAVE`; written after the execution tick on both.
    n.escape(0);
    t.menu(&n, "ESC");
    n.click(0, NET_MENU, "savegame2");
    assert!(
        n.dialogs(0).iter().any(|d| d == "savegame"),
        "{:?}",
        n.dialogs(0)
    );
    t.menu(&n, "SAVEGAME2");
    n.click(0, "savegame", "filename");
    type_text(&mut n, 0, NET_SAVE);
    assert_eq!(n.runs[0].host.app.screen_state().save_name.get(), NET_SAVE);
    n.click(0, "savegame", "save");
    n.frames_until("both have saved", |l| {
        (0..2).all(|i| l.paths(i).net_save(NET_SAVE).exists())
    });
    n.settle();
    t.menu(&n, "netz1 saved");
    let origin = GameSaveFile::from_bytes(&std::fs::read(n.paths(0).net_save(NET_SAVE)).unwrap())
        .unwrap()
        .sim
        .state
        .steps;
    assert!(origin > 150, "saved after tick 150: {origin}");

    // `QUITGAME` is the own defeat on host and client. Bert gives up (through the turn; Anna wins in the same tick)
    // and leaves his end screen (`END_DEFEAT` -> accounting, `Bye`). After that Anna is alone: her `END_VICTORY` has
    // nobody to hand over to and ends the match for all (`net.end()`): message, "OK", accounting; both "Main menu".
    n.escape(1);
    t.menu(&n, "ESC before QUITGAME");
    n.click(1, NET_MENU, "quitgame");
    assert_eq!(n.runs[1].host.app.screen_state().info_text, Some(15780));
    t.menu(&n, "QUITGAME");
    n.runs[1].click("infoframe", "ok");
    n.frames_until("defeat on Bert, victory on Anna", |l| {
        l.runs[1].dialogs().iter().any(|d| d == "gameend_defeat")
            && l.runs[0].dialogs().iter().any(|d| d == "gameend_victory")
    });
    n.settle();
    t.menu(&n, "defeat and victory");
    n.click(1, "gameend_defeat", "end_defeat");
    n.frames_until("accounting of Bert", |l| in_accounting(&l.runs[1]));
    n.settle();
    t.menu(&n, "END_DEFEAT");
    n.click(0, "gameend_victory", "end_victory");
    n.frames_until("message on Anna", |l| message(&l.runs[0]).is_some());
    n.settle();
    t.menu(&n, "end for all");
    n.click(0, "msgbox", "ok");
    n.frames_until("accounting on both", |l| l.runs.iter().all(in_accounting));
    n.settle();
    t.menu(&n, "accounting");
    for run in &mut n.runs {
        leave_accounting(run);
    }
    n.frames_until("main menu on both", |l| l.runs.iter().all(in_main_menu));
    n.settle();
    t.menu(&n, "main menu");

    // Anna loads: main menu `LAN1` (`{3,"start"}`, `{5,"lan1"}`) -> `LAN3`, "Load game" (`LevelType` 2), "netz1",
    // `CREATE` (`{0x12}`) -> `LAN4`.
    n.click(0, "start", "lan1");
    t.menu(&n, "Anna LAN1");
    n.click(0, "lan1", "create");
    t.menu(&n, "Anna LAN3");
    let combo = lan_element(&n, 0, "lan3", "leveltype");
    n.combo_row(0, "lan3", "leveltype", combo.items.len() - 1);
    assert_eq!(n.runs[0].host.app.config().level_type, 2);
    t.menu(&n, "load game");
    let list = lan_element(&n, 0, "loadsavednetworkgame", "dir");
    let row = list
        .items
        .iter()
        .position(|i| i.ends_with(&format!(") {NET_SAVE}")))
        .unwrap_or_else(|| panic!("{NET_SAVE} is missing in {:?}", list.items));
    n.list_row(0, "loadsavednetworkgame", "dir", row);
    n.click(0, "lan3", "create");
    assert!(
        n.dialogs(0).iter().any(|d| d == "lan4"),
        "{:?}",
        n.dialogs(0)
    );
    assert!(n.lobby(0).is_loaded());
    t.menu(&n, "netz1, CREATE");

    // Bert: main menu `LAN1`, the game appears (only with his own file), `JOIN` -> `LAN4`.
    n.click(1, "start", "lan1");
    t.menu(&n, "Bert LAN1");
    n.frames_until("game in the list", |l| {
        !lan_element(l, 1, "lan1", "opengames").items.is_empty()
    });
    n.list_row(1, "lan1", "opengames", 0);
    n.click(1, "lan1", "join");
    n.frames_until("LAN4 on Bert", |l| {
        l.phase(1) == NetPhase::Lobby && l.dialogs(1).iter().any(|d| d == "lan4")
    });
    n.settle();
    t.menu(&n, "Bert JOIN");

    // `START` in `LAN4`: both continue their own file, turn 0 at the tick of the savegame; 200 ticks.
    n.start_loaded();
    for run in &n.runs {
        assert_eq!(run.view.net().map(NetDriver::origin), Some(origin));
    }
    n.frames_until("200 ticks from the savegame", |l| {
        l.runs.iter().all(|r| r.view.steps() >= origin + 200)
    });
    t.ticks(&n, "200 ticks");
    let sums: Vec<u64> = n
        .runs
        .iter()
        .map(|r| {
            r.view
                .sim()
                .and_then(|s| s.hash_at(origin + 200))
                .expect("checksum at origin + 200")
        })
        .collect();
    assert_eq!(sums[0], sums[1], "both machines compute equally");
    for sum in sums {
        t.h.bytes(&sum.to_le_bytes());
    }
    Some(t.h.0)
}

/// Element `dialog`/`name` of machine `i` (must exist).
fn lan_element(n: &Lan, i: usize, dialog: &str, name: &str) -> UiElementData {
    let run = &n.runs[i];
    state_data(&run.host.app, S1024, run.now)
        .elements
        .into_iter()
        .find(|e| e.dialog == dialog && e.name == name)
        .unwrap_or_else(|| panic!("{dialog}/{name} is missing on machine {i}"))
}

/// Section `menu_level`: `IsMenuLevel 1`, real maps. Program start with `START` (loading screen `default.ldg`,
/// menu level runs), 200 frames, `SINGLEPLAYERMENU` (100 frames, ticks stand still), back, 100 frames, `OPTIONS` ->
/// `GAMEPLAYOPTIONS` -> back, start campaign 1-1 (the menu level ends), 50 ticks, game end `0x0E`, accounting ->
/// `MAINMENU` -> `START` (restart, tick 0), 50 frames; [`Circle::snap`] after every step.
fn menu_level_section() -> Option<u64> {
    let mut c = Circle::configured(|config| config.is_menu_level = true)?;
    let r = &mut c;
    let frames = |r: &mut Circle, n: usize| {
        for _ in 0..n {
            r.r.frame();
        }
    };
    r.r.idle();
    assert!(r.r.flow.menu_level().is_some_and(|(_, shown)| shown));
    r.snap();
    frames(r, 200);
    assert!(r.r.view.steps() > 0, "ticks run");
    r.snap();
    r.go("start", "singleplayermenu");
    assert_eq!(r.dialogs(), ["selectprofile"]);
    assert!(r.r.flow.menu_level().is_some_and(|(_, shown)| !shown));
    let paused = r.r.view.steps();
    frames(r, 100);
    assert_eq!(r.r.view.steps(), paused, "ticks stand still in the submenu");
    r.snap();
    r.go("selectprofile", "cancel");
    assert!(r.dialogs().contains(&"start".to_string()));
    assert!(r.r.flow.menu_level().is_some_and(|(_, shown)| shown));
    r.snap();
    frames(r, 100);
    assert!(r.r.view.steps() > paused, "ticks run again");
    r.snap();
    r.go("start", "options");
    r.snap();
    r.go("options", "gameplayoptions");
    assert!(r.dialogs().contains(&"gameplayoptions".to_string()));
    r.snap();
    r.go("options", "ok");
    assert!(r.dialogs().contains(&"start".to_string()));
    r.snap();
    // Real menu path: create a profile, campaign 1, video `SPA-1`; `VIDEOSTART` starts `kamp1-1` with the menu level
    // halted (submenu displaces `START`), the start ends it (`FUN_004BAF40`), the match runs with game type -1.
    r.go("start", "singleplayermenu");
    assert_eq!(r.dialogs(), ["selectprofile"]);
    r.go("selectprofile", "profilename");
    r.typ(PROFILE);
    r.go("selectprofile", "create");
    r.go("selectprofile", "ok");
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    r.snap();
    r.go("singleplayermenu", "selectcampaign");
    assert_eq!(r.dialogs(), ["kampagne0", "selectcampaign"]);
    r.go("selectcampaign", "kampagne1");
    assert_eq!(
        r.dialogs(),
        ["campaignvideo", "kampagne1", "selectcampaign"]
    );
    assert!(r.r.flow.menu_level().is_some_and(|(_, shown)| !shown));
    r.snap();
    r.go("campaignvideo", "videostart");
    r.settle_match();
    assert!(r.r.flow.menu_level().is_none());
    assert!(r.r.view.level().eq_ignore_ascii_case(KAMP1_1));
    assert_eq!(r.r.host.app.game().map(|g| g.game_type), Some(-1));
    r.snap();
    let at = r.r.view.steps();
    r.run_to(at + 50);
    r.snap();
    // Game end `0x0E` -> accounting -> MAINMENU -> START rebuilds the menu level.
    r.r.apply(nml_ui::msg::UiEffect::EndMatch);
    r.r.idle();
    assert!(!r.r.flow.running());
    assert!(r.r.flow.menu_level().is_none());
    r.snap();
    r.r.click("accounting", "mainmenu");
    let mut restarted = None;
    for _ in 0..200 {
        r.r.frame();
        if r.r.flow.menu_level().is_some() {
            restarted = Some(r.r.view.steps());
            break;
        }
    }
    assert_eq!(restarted, Some(0), "new session, tick 0");
    r.r.idle();
    r.snap();
    frames(r, 50);
    assert!(r.r.view.steps() > 0);
    Some({
        r.snap();
        std::mem::replace(&mut r.hash, Fnv::new()).0
    })
}

/// Section `replay`: a circle of its own with a fresh savegame folder and the test clock (name of the recording
/// fixed). Create a profile, `CREATESINGLEPLAYERGAME` with the switch `savereplay` (click), map Tintebariver, `START2`;
/// 200 ticks with walk commands (the right-click path `move_units`, [`walk`]); `QUITGAME` -> defeat -> accounting ->
/// main menu; `SINGLEPLAYERMENU` -> `LOADAUTOPLAY` -> row 0 -> `START`; F1 (no effect, no quicksave); 200 ticks.
/// [`Circle::snap`] after every step (with `ui_state.game.replay`), finally the checksum of the replay at tick 200 - it
/// equals that of the match before `QUITGAME` (giving up runs immediately and replaces the checksum of its tick).
fn replay_section() -> Option<u64> {
    use super::replay_tests::{NAME, walk};
    use nml_control::UiReplayMode;
    let mut c = Circle::new()?;
    let r = &mut c;
    let replay_mode = |r: &Circle| Snapshot::of(&r.r).replay.map(|d| d.mode);
    r.r.idle();
    r.go("start", "singleplayermenu");
    assert_eq!(r.dialogs(), ["selectprofile"]);
    r.go("selectprofile", "profilename");
    r.typ(PROFILE);
    r.go("selectprofile", "create");
    r.go("selectprofile", "ok");
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    r.snap();
    // Own game with `SAVEREPLAY`.
    r.go("singleplayermenu", "createsingleplayergame");
    let levels = r.element("loadmultiplayermap", "levels");
    let row = levels
        .items
        .iter()
        .position(|i| i.to_lowercase().contains("tinte"))
        .expect("map Tintebariver");
    r.list_row("loadmultiplayermap", "levels", row);
    assert_eq!(r.r.host.app.config().save_replay, 0);
    r.go("createsingleplayergame", "savereplay");
    assert_eq!(r.r.host.app.config().save_replay, 1, "switch savereplay");
    r.snap();
    r.go("createsingleplayergame", "create2");
    assert_eq!(r.dialogs(), ["levelinfo", "singleplayerstart"]);
    r.combo("singleplayerstart", "name1", "Computer (Normal)");
    r.go("singleplayerstart", "start2");
    r.settle_match();
    assert_eq!(replay_mode(r), Some(UiReplayMode::Record), "recording runs");
    r.snap();
    // 200 ticks with walk commands.
    r.r.until_step();
    walk(&mut r.r, 400.0);
    r.run_to(70);
    r.snap();
    walk(&mut r.r, -300.0);
    r.run_to(140);
    r.snap();
    walk(&mut r.r, 200.0);
    r.run_to(200);
    let played = r.r.view.sim().unwrap().hash_at(200).expect("tick 200");
    r.snap();
    // `QUITGAME` -> confirmation -> defeat -> accounting -> main menu; the recording ends with the match.
    r.r.key("ESCAPE");
    r.r.until_step();
    r.r.idle();
    assert_eq!(r.r.host.app.game_menu_state(), 1, "game menu open");
    r.snap();
    r.go("ingameoptions3", "quitgame");
    r.r.key("RETURN");
    r.r.idle();
    assert_eq!(r.dialogs(), ["gameend_defeat"]);
    r.snap();
    r.go("gameend_defeat", "end_defeat");
    r.snap();
    r.go("accounting", "mainmenu");
    assert!(!r.r.flow.running(), "match ended");
    assert!(r.r.flow.recorder().is_none(), "recording closed");
    r.snap();
    // `LOADAUTOPLAY` -> row 0 -> `START`.
    r.go("start", "singleplayermenu");
    assert_eq!(r.dialogs(), ["singleplayermenu"]);
    r.go("singleplayermenu", "loadautoplay");
    assert!(
        r.dialogs().contains(&"loadautoplay".to_string()),
        "{:?}",
        r.dialogs()
    );
    r.snap();
    r.list_row("loadautoplay", "dir", 0);
    assert!(r.element("loadautoplay", "dir").items[0].ends_with(NAME));
    r.snap();
    r.go("loadautoplay", "start");
    r.settle_match();
    assert!(r.r.flow.playing_back(), "replay runs");
    assert_eq!(replay_mode(r), Some(UiReplayMode::Play));
    r.snap();
    // F1 stays without effect in the replay.
    r.r.key("F1");
    r.r.idle();
    let guid = active_guid(&r.r.host.app);
    assert!(!r.r.files.exists(SaveDir::Main, guid, "quicksave1"));
    r.snap();
    r.run_to(200);
    r.snap();
    let replayed = r.r.view.sim().unwrap().hash_at(200).expect("tick 200");
    assert_eq!(replayed, played, "replay equal to the match at tick 200");
    Some({
        r.hash.bytes(&replayed.to_le_bytes());
        std::mem::replace(&mut r.hash, Fnv::new()).0
    })
}

#[test]
fn ui_golden() {
    let Some(first) = circle() else {
        return;
    };
    let second = circle().unwrap();
    assert_eq!(first, second, "twice the same");
    let dir = nml_rda::testutil::corpus_game_dir().unwrap();
    let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
    if game_version(&vfs) == Ok(GameVersion::German) && first != UI_GOLDEN {
        let table: String = first
            .iter()
            .map(|(name, hash)| format!("    ({name:?}, 0x{hash:016X}),\n"))
            .collect();
        panic!("UI_GOLDEN differs; measured:\n{table}");
    }
}
