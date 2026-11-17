//! Tests of the screen logic: for each element the row of the table in re/menu-flow. The handlers run individually
//! via [`dispatch`] with synthetic messages and in operation via `MenuApp` with the corpus dialogs (they need
//! `NML_GAME_DIR`, otherwise they skip themselves silently).

use std::cell::RefCell;
use std::rc::Rc;

use nml_formats::campaign::{CampaignEntry, CampaignTable, EntryKind};
use nml_formats::engine_ini::EngineConfig;
use nml_formats::profiles::ProfilesFile;

use super::*;
use crate::app::tests::Answer;
use crate::app::tests::{Corpus, Driver, S1280, close, corpus, open};
use crate::app::{MenuApp, MenuOptions};
use crate::dialog::tests::{Fixture, fixture};
use crate::input::UiInput;
use crate::msg::{MenuLevelEffect, Param, Requery, code};
use crate::profiles::{Profiles, name_field, name_text};
use crate::tree::NodeKind;

const ESCAPE: u8 = 0x1B;
const RETURN: u8 = 0x0D;

/// The logic alone: settings, queue, effects, state and data ([`EnvParts`]), texts from the fixture.
struct Logic {
    fixture: Fixture,
    parts: EnvParts,
}

impl std::ops::Deref for Logic {
    type Target = EnvParts;
    fn deref(&self) -> &EnvParts {
        &self.parts
    }
}

impl std::ops::DerefMut for Logic {
    fn deref_mut(&mut self) -> &mut EnvParts {
        &mut self.parts
    }
}

impl Logic {
    fn new() -> Logic {
        Logic {
            fixture: fixture(),
            parts: EnvParts::default(),
        }
    }

    /// Message `code` from `dialog`/`element` to the dispatcher chain; return value and answered message.
    fn send(&mut self, dialog: &str, element: &str, code: u32) -> (i32, ElementMsg) {
        self.deliver(ElementMsg::new(dialog, element, code))
    }

    /// Like [`Self::send`] with the parameter `int` (code 1 of a slider, code 4 of a combobox).
    fn send_int(&mut self, dialog: &str, element: &str, code: u32, v: i32) -> (i32, ElementMsg) {
        let mut msg = ElementMsg::new(dialog, element, code);
        msg.params.insert("int".into(), Param::Int(v));
        self.deliver(msg)
    }

    fn deliver(&mut self, mut msg: ElementMsg) -> (i32, ElementMsg) {
        let r = self.parts.deliver(&self.fixture.texts, &mut msg);
        (r, msg)
    }

    /// Answer `int` to a query (return value, value).
    fn query(&mut self, dialog: &str, element: &str, code: u32) -> (i32, Option<i32>) {
        let (r, msg) = self.send(dialog, element, code);
        (
            r,
            int_of(&msg, "int").or_else(|| int_of(&msg, "resolution")),
        )
    }

    /// Entries `array` of a combobox (code 2).
    fn items(&mut self, dialog: &str, element: &str) -> Vec<String> {
        let (r, msg) = self.send(dialog, element, code::QUERY_ITEMS);
        assert_eq!(r, 1, "{dialog}/{element}");
        match msg.params.get("array") {
            Some(Param::Array(items)) => items.clone(),
            other => panic!("{dialog}/{element}: {other:?}"),
        }
    }

    /// Texts for the numbers like `FUN_0055FFE0`.
    fn texts(&self, ids: &[i32]) -> Vec<String> {
        let texts = &self.fixture.texts;
        ids.iter()
            .map(|&id| {
                texts.resolve(id);
                texts.display(id)
            })
            .collect()
    }

    fn click(&mut self, dialog: &str, element: &str) -> i32 {
        self.send(dialog, element, code::TRIGGERED).0
    }

    /// Fetch the queued commands.
    fn take_queue(&mut self) -> Vec<AppCmd> {
        let cmds = self.queue.iter().cloned().collect();
        self.queue = AppQueue::default();
        cmds
    }
}

/// `MenuApp` over the corpus with the real screen logic (the dummy only records and passes on).
fn live(start_menu: bool, game_spy_installed: bool) -> Option<Driver<Corpus>> {
    let opts = MenuOptions {
        start_menu,
        game_spy_installed,
        ..MenuOptions::default()
    };
    live_with(EngineConfig::default(), opts, Box::new(dispatch))
}

/// Like [`live`] with own settings (without intro), options of the host and logic.
fn live_with(config: EngineConfig, opts: MenuOptions, logic: Answer) -> Option<Driver<Corpus>> {
    let c = corpus()?;
    let config = EngineConfig {
        is_intro: false,
        ..config
    };
    let app = MenuApp::new(&c.vfs, &c.texts, config, opts).expect("MenuApp");
    Some(Driver::with_app(c, app, logic))
}

fn int_of(msg: &ElementMsg, name: &str) -> Option<i32> {
    match msg.params.get(name) {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Effects and queue at one point in time.
type Snapshot = Rc<RefCell<Option<(Vec<UiEffect>, Vec<AppCmd>)>>>;

fn click_sfx() -> UiEffect {
    UiEffect::PlaySfx {
        event: "SFX_KLICK".into(),
        flags: 0x14,
        category: 1,
    }
}

#[test]
fn start_routes_by_button_name() {
    // `0x004D1826`...`0x004D1855`: every button except `quit`, `singleplayermenu`, `gamespy` closes `start` and
    // opens the dialog with its name - also an unknown one.
    let mut l = Logic::new();
    for name in ["options", "lan1", "credits", "nowhere"] {
        assert_eq!(l.click("start", name), 1);
        assert_eq!(
            l.take_queue(),
            [close("start", true), open(name, true)],
            "{name}"
        );
    }
    assert!(l.effects.is_empty());
    // Other codes are not answered by the handler; other dialogs go to no handler.
    assert_eq!(l.send("start", "options", code::QUERY_STATUS).0, -1);
    assert_eq!(l.click("lan1", "options"), -1);
    assert!(l.take_queue().is_empty());

    // Live: click on OPTIONS, START and LOGO fly out, OPTIONS in.
    let Some(mut d) = live(true, false) else {
        return;
    };
    d.idle();
    d.app.take_effects();
    d.click("start", "options");
    d.frame();
    assert_eq!(d.triggered("options"), 1);
    assert_eq!(d.app.take_effects(), [click_sfx()]);
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["options"]);
    assert!(d.find("START").is_none() && d.find("LOGO").is_none());
    assert!(d.find("OPTIONS").is_some());
}

#[test]
fn quit_quits_without_question() {
    // O-30 (main menu): `PostQuitMessage(0)` at once, no command, no confirmation prompt.
    let mut l = Logic::new();
    assert_eq!(l.click("start", "quit"), 1);
    assert_eq!(l.effects, [UiEffect::Quit]);
    assert!(l.take_queue().is_empty());

    let Some(mut d) = live(true, false) else {
        return;
    };
    d.idle();
    d.app.take_effects();
    d.click("start", "quit");
    d.frame();
    assert_eq!(d.app.take_effects(), [click_sfx(), UiEffect::Quit]);
    assert!(d.app.queue().is_empty());
    assert!(d.find("INFOFRAME").is_none());
}

#[test]
fn gamespy_is_disabled_without_arcade() {
    // `0x004D1895`...`0x004D18E1`: code 5 at `gamespy` -> `status 0x20000000` without GameSpy Arcade, otherwise 0.
    let mut l = Logic::new();
    let (r, msg) = l.send("start", "gamespy", code::QUERY_STATUS);
    assert_eq!(
        (r, msg.params.get("status")),
        (1, Some(&Param::Int(0x2000_0000)))
    );
    l.game_spy_installed = true;
    let (_, msg) = l.send("start", "gamespy", code::QUERY_STATUS);
    assert_eq!(msg.params.get("status"), Some(&Param::Int(0)));

    let Some(mut d) = live(true, false) else {
        return;
    };
    d.idle();
    let enabled = |d: &Driver<Corpus>| {
        let report = d.app.report(S1280, d.now);
        report
            .elements
            .iter()
            .find(|e| e.name == "gamespy")
            .unwrap()
            .enabled
    };
    assert!(!enabled(&d));
    // The locked button gets no mouse slot: no click sound, no code 8, no confirmation prompt.
    d.app.take_effects();
    d.click("start", "gamespy");
    d.idle();
    assert_eq!(d.triggered("gamespy"), 0);
    assert!(d.app.take_effects().is_empty());
    assert!(d.find("INFOFRAME").is_none());

    // With GameSpy Arcade: confirmation prompt 15117, OK -> `{0x36}` (reported as Unhandled).
    let Some(mut d) = live(true, true) else {
        return;
    };
    d.idle();
    assert!(enabled(&d));
    d.click("start", "gamespy");
    d.idle();
    assert!(d.find("INFOFRAME").is_some());
    assert_eq!(d.app.screen_state().info_text, Some(15117));
    assert_eq!(d.app.screen_state().info_ok, Some(AppCmd::GameSpy));
    assert_eq!(d.app.screen_state().info_cancel, None);
    d.app.take_effects();
    d.click("infoframe", "ok");
    d.idle();
    assert!(d.find("INFOFRAME").is_none());
    let effects = d.app.take_effects();
    assert_eq!(
        effects.last(),
        Some(&UiEffect::Unhandled(AppCmd::GameSpy)),
        "{effects:?}"
    );
}

#[test]
fn singleplayer_opens_selectprofile_without_profile() {
    // `0x004D1764`...`0x004D17B9`: without an active profile (`m_CurProfileID < 0`) `selectprofile`, `NetName` stays.
    let mut l = Logic::new();
    assert_eq!(l.click("start", "singleplayermenu"), 1);
    assert_eq!(
        l.take_queue(),
        [close("start", true), open("selectprofile", true)]
    );
    assert!(l.effects.is_empty());
}

#[test]
fn options_switches_pages_and_ignores_the_open_one() {
    // O-103: "frame opened" clears the flag; a subpage button closes the open page (also the empty one) and opens
    // its own; every message of the open subpage is not handled.
    let mut l = Logic::new();
    l.state.options_page = "old".into();
    assert_eq!(l.send("options", "options", code::QUERY_STATUS).0, -1);
    assert_eq!(l.state.options_page, "");
    assert_eq!(l.click("options", "videooptions"), 1);
    assert_eq!(
        l.take_queue(),
        [close("", true), open("videooptions", true)]
    );
    assert_eq!(l.state.options_page, "videooptions");
    assert_eq!(l.click("options", "videooptions"), -1, "open page");
    assert_eq!(l.send("options", "videooptions", code::QUERY_STATUS).0, -1);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.click("options", "soundoptions"), 1);
    assert_eq!(
        l.take_queue(),
        [close("videooptions", true), open("soundoptions", true)]
    );
    // Other codes of the remaining buttons: not handled.
    assert_eq!(l.send("options", "videooptions", code::QUERY_STATUS).0, -1);
    assert!(l.take_queue().is_empty() && l.effects.is_empty());

    // Live: page open, the same button once more without effect (but with a click sound), then the next page.
    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open("options", true));
    d.idle();
    d.click("options", "videooptions");
    d.idle();
    assert!(d.find("VIDEOOPTIONS").is_some());
    d.app.take_effects();
    d.click("options", "videooptions");
    d.frame();
    assert_eq!(d.triggered("videooptions"), 2);
    assert_eq!(d.app.take_effects(), [click_sfx()]);
    assert!(d.app.queue().is_empty() && !d.app.menu().busy());
    d.click("options", "soundoptions");
    d.idle();
    assert!(d.find("VIDEOOPTIONS").is_none());
    assert_eq!(d.app.menu().open_names(), ["options", "soundoptions"]);
}

#[test]
fn options_ok_writes_settings_before_the_menu_commands() {
    // O-103, `0x004D3809`...`0x004D38A8`: write `Engine.ini` at once, then `{3, page}`, `{4,"options"}`,
    // `{5,"start"}`; flag afterwards `"ok"`.
    let mut l = Logic::new();
    l.click("options", "gameplayoptions");
    l.take_queue();
    assert_eq!(l.click("options", "ok"), 1);
    assert_eq!(l.effects, [UiEffect::WriteSettings]);
    assert_eq!(
        l.take_queue(),
        [
            close("gameplayoptions", true),
            close("options", false),
            open("start", true)
        ]
    );
    assert_eq!(l.state.options_page, "ok");

    // Live: state of effects and queue right after handling code 8 at `ok`.
    let Some(c) = corpus() else {
        return;
    };
    let snapshot: Snapshot = Rc::default();
    let s = snapshot.clone();
    let config = EngineConfig {
        is_intro: false,
        ..EngineConfig::default()
    };
    let app = MenuApp::new(&c.vfs, &c.texts, config, MenuOptions::default()).unwrap();
    let mut d = Driver::with_app(
        c,
        app,
        Box::new(move |m, env| {
            let r = dispatch(m, env);
            if (m.dialog.as_str(), m.element.as_str(), m.code) == ("options", "ok", code::TRIGGERED)
            {
                *s.borrow_mut() = Some((env.effects.clone(), env.queue.iter().cloned().collect()));
            }
            r
        }),
    );
    d.push(open("options", true));
    d.idle();
    d.click("options", "videooptions");
    d.idle();
    d.app.take_effects();
    d.click("options", "ok");
    d.frame();
    let (effects, queue) = snapshot.borrow_mut().take().expect("OK triggered");
    assert_eq!(effects, [click_sfx(), UiEffect::WriteSettings]);
    assert_eq!(
        queue,
        [
            close("videooptions", true),
            close("options", false),
            open("start", true)
        ]
    );
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["logo", "start"]);
    assert!(d.find("OPTIONS").is_none() && d.find("VIDEOOPTIONS").is_none());
    assert_eq!(d.app.screen_state().options_page, "ok");

    // OK without an open subpage: `{3,""}` finds nothing and closes nothing.
    d.click("start", "options");
    d.idle();
    assert_eq!(d.app.screen_state().options_page, "");
    d.click("options", "ok");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["logo", "start"]);
}

#[test]
fn escape_does_nothing_in_options() {
    // O-103: `OK` has no `CHECKESCAPE` (like the subpage buttons and `VIDEOOPTIONS`): ESC triggers nothing.
    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open("options", true));
    d.idle();
    d.click("options", "videooptions");
    d.idle();
    d.app.take_effects();
    d.log.borrow_mut().clear();
    d.key(ESCAPE);
    d.idle();
    assert!(
        !d.log.borrow().iter().any(|m| m.2 == code::TRIGGERED),
        "{:?}",
        d.log.borrow()
    );
    assert!(d.app.take_effects().is_empty());
    assert_eq!(d.app.menu().open_names(), ["options", "videooptions"]);
}

#[test]
fn credits_ok_returns_to_start() {
    // `0x004DB920`...`0x004DB990`: `{3,"credits"}`, `{5,"start"}`; in operation also via ESC (`CHECKESCAPE`).
    let mut l = Logic::new();
    assert_eq!(l.click("credits", "ok"), 1);
    assert_eq!(
        l.take_queue(),
        [close("credits", true), open("start", true)]
    );
    assert_eq!(l.click("credits", "scrolltext"), -1);

    let Some(mut d) = live(false, false) else {
        return;
    };
    d.push(open("credits", true));
    d.idle();
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.triggered("ok"), 1);
    assert_eq!(d.app.menu().open_names(), ["logo", "start"]);
    assert!(d.find("CREDITS").is_none());
}

#[test]
fn infoframe_enter_is_ok_escape_is_cancel() {
    // `FUN_004BCD70`: remember text and commands, `{5,"infoframe"}`; `textinfo` code 7 returns the text; `ok`
    // (`CHECKENTER`) or `cancel` (`CHECKESCAPE`) close and queue their command, both copies are dropped.
    let mut l = Logic::new();
    ask(
        &mut l.parts.state,
        &mut l.parts.queue,
        15096,
        Some(AppCmd::ResetKeys),
        Some(AppCmd::ReloadLevel),
    );
    assert_eq!(l.take_queue(), [open("infoframe", true)]);
    let (r, msg) = l.send("infoframe", "textinfo", code::QUERY_TEXT);
    assert_eq!(r, 1);
    let expected = l.fixture.texts.display(15096);
    assert_eq!(msg.params.get("text"), Some(&Param::Text(expected)));
    assert_eq!(l.click("infoframe", "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [close("infoframe", true), AppCmd::ReloadLevel]
    );
    assert_eq!(
        (l.state.info_ok.clone(), l.state.info_cancel.clone()),
        (None, None)
    );
    // A second OK without a new confirmation prompt only closes.
    assert_eq!(l.click("infoframe", "ok"), 1);
    assert_eq!(l.take_queue(), [close("infoframe", true)]);

    let Some(mut d) = live(false, false) else {
        return;
    };
    let text = d.world.texts.display(15096);
    // Commands that the reimplementation does not execute yet (`0x32` and `0x1F` are executed).
    for (key, cmd) in [
        (RETURN, AppCmd::GameSpy),
        (ESCAPE, AppCmd::HideLevelBackground),
    ] {
        d.ask(
            15096,
            Some(AppCmd::GameSpy),
            Some(AppCmd::HideLevelBackground),
        );
        d.idle();
        let report = d.app.report(S1280, d.now);
        let label = report
            .elements
            .iter()
            .find(|e| e.dialog == "infoframe" && e.name == "textinfo")
            .expect("label textinfo");
        assert_eq!(label.text.as_deref(), Some(text.as_str()));
        assert!(
            report
                .dialogs
                .iter()
                .any(|r| r.name == "infoframe" && r.modal)
        );
        d.app.take_effects();
        d.key(key);
        d.idle();
        assert!(d.find("INFOFRAME").is_none(), "{key:#x}");
        let want = match cmd {
            AppCmd::HideLevelBackground => UiEffect::MenuLevel(MenuLevelEffect::Hide),
            other => UiEffect::Unhandled(other),
        };
        assert_eq!(d.app.take_effects(), [want], "{key:#x}");
    }
}

#[test]
fn msgbox_takes_a_free_text_first() {
    // A message with free text (network match: name of the savegame) opens `MSGBOX` like `FUN_004BCF10`; `textinfo`
    // code 7 returns the free text before a text ID. `tell`/`ask` with a text ID clear it again.
    let mut l = Logic::new();
    let text = "The host has ended the match. It was saved as \"netz_2026_11_06_1807\".";
    tell_text(
        &mut l.parts.state,
        &mut l.parts.queue,
        text.into(),
        Some(AppCmd::EndMatch),
    );
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert_eq!(l.state.info_cancel, None);
    let (r, msg) = l.send("msgbox", "textinfo", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(msg.params.get("text"), Some(&Param::Text(text.into())));
    assert_eq!(l.click("msgbox", "ok"), 1);
    assert_eq!(l.take_queue(), [close("msgbox", true), AppCmd::EndMatch]);
    assert_eq!(l.state.info_free, None, "forgotten after \"OK\"");
    // A message with a text ID afterwards shows its text again, likewise a confirmation prompt.
    tell(&mut l.parts.state, &mut l.parts.queue, 15748, None);
    assert_eq!(l.state.info_free, None);
    let (_, msg) = l.send("msgbox", "textinfo", code::QUERY_TEXT);
    let expected = l.fixture.texts.display(15748);
    assert_eq!(msg.params.get("text"), Some(&Param::Text(expected)));
    tell_text(&mut l.parts.state, &mut l.parts.queue, text.into(), None);
    ask(&mut l.parts.state, &mut l.parts.queue, 15096, None, None);
    assert_eq!(l.state.info_free, None);
}

#[test]
fn msgbox_has_no_escape() {
    // O-103: `MSGBOX` has only `OK` with `CHECKENTER`; ESC has no effect, ENTER closes and queues the command.
    let mut l = Logic::new();
    l.state.info_cancel = Some(AppCmd::Quit);
    tell(
        &mut l.parts.state,
        &mut l.parts.queue,
        15748,
        Some(AppCmd::ReloadLevel),
    );
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert_eq!(l.state.info_cancel, None, "message without cancel command");

    let Some(mut d) = live(false, false) else {
        return;
    };
    d.tell(15748, Some(AppCmd::HideLevelBackground));
    d.idle();
    let msgbox = d.find("MSGBOX").expect("MSGBOX open");
    d.app.take_effects();
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.find("MSGBOX"), Some(msgbox));
    assert_eq!(d.triggered("ok"), 0);
    assert!(d.app.take_effects().is_empty());
    d.key(RETURN);
    d.idle();
    assert!(d.find("MSGBOX").is_none());
    assert_eq!(
        d.app.take_effects(),
        [UiEffect::MenuLevel(MenuLevelEffect::Hide)]
    );
}

// Video, sound and game options (re/menu-flow, `VIDEOOPTIONS`, `SOUNDOPTIONS`/`...2`/`...3`,
// `GAMEPLAYOPTIONS`/`...2`).

fn mode(width: i32, height: i32, bits: i32) -> DisplayMode {
    DisplayMode {
        width,
        height,
        bits,
    }
}

/// Raw modes as from the monitor: 1920 x 1080 (width) and 24 bits drop out, 1024 x 768 x 32 comes twice.
fn raw_modes() -> Vec<DisplayMode> {
    vec![
        mode(800, 600, 32),
        mode(1024, 768, 16),
        mode(1920, 1080, 32),
        mode(1024, 768, 32),
        mode(1280, 1024, 32),
        mode(1024, 768, 32),
        mode(1600, 1200, 24),
        mode(1280, 960, 16),
    ]
}

/// What [`filter_display_modes`] makes of [`raw_modes`].
fn filtered_modes() -> Vec<DisplayMode> {
    vec![
        mode(800, 600, 32),
        mode(1024, 768, 16),
        mode(1024, 768, 32),
        mode(1280, 1024, 32),
        mode(1280, 960, 16),
    ]
}

/// Texts of the filtered modes (`"%d x %d x %d"`).
const MODE_TEXTS: [&str; 5] = [
    "800 x 600 x 32",
    "1024 x 768 x 16",
    "1024 x 768 x 32",
    "1280 x 1024 x 32",
    "1280 x 960 x 16",
];

/// Report on an element of an open dialog.
fn element(d: &Driver<Corpus>, dialog: &str, name: &str) -> crate::app::ReportElement {
    d.app
        .report(S1280, d.now)
        .elements
        .into_iter()
        .find(|e| e.dialog == dialog && e.name == name)
        .unwrap_or_else(|| panic!("{dialog}/{name} is missing"))
}

/// Value and step count of a slider (key = dialog + element name in uppercase).
fn slider(d: &Driver<Corpus>, key: &str) -> (i32, i32) {
    let id = d.find(key).unwrap_or_else(|| panic!("{key} is missing"));
    match &d.app.tree().node(id).kind {
        NodeKind::Slider(s) => (s.value, s.resolution),
        _ => panic!("{key} is not a slider"),
    }
}

/// Opens `name` (command 5) and waits until the UI is at rest.
fn show(d: &mut Driver<Corpus>, name: &str) {
    d.push(open(name, true));
    d.idle();
}

fn volume(category: u8, value: i32) -> UiEffect {
    UiEffect::SetVolume { category, value }
}

fn sfx_test() -> UiEffect {
    UiEffect::PlaySfx {
        event: "SFX_TEST".into(),
        flags: 0x10,
        category: 1,
    }
}

fn speech_test() -> UiEffect {
    UiEffect::PlaySpeech {
        path: "data/sprache/testtext.ogg".into(),
    }
}

#[test]
fn video_changes_only_the_working_copy_until_accept() {
    // O-98: code 5 of the frame copies resolution, qualities and gamma; comboboxes change only the copy, without
    // `ACCEPT` they are lost on the next opening - gamma not (takes effect at once).
    let mut l = Logic::new();
    l.modes = raw_modes();
    assert_eq!(
        l.send("videooptions", "videooptions", code::QUERY_STATUS).0,
        -1
    );
    let opened = VideoCopy {
        mode: mode(1024, 768, 16),
        texture: 0,
        terrain: 0,
        particles: 2,
        shadow: 2,
        gamma: 1.0,
    };
    assert_eq!(l.state.video_copy, Some(opened));
    let before = l.config.clone();
    for (element, index) in [
        ("resolution", 3),
        ("texturequality", 2),
        ("terrainquality", 1),
        ("particles", 0),
        ("shadow", 3),
    ] {
        assert_eq!(
            l.send_int("videooptions", element, code::ITEM_SELECTED, index)
                .0,
            1,
            "{element}"
        );
    }
    let changed = VideoCopy {
        mode: mode(1280, 1024, 32),
        texture: 2,
        terrain: 1,
        particles: 2,
        shadow: 0,
        gamma: 1.0,
    };
    assert_eq!(l.state.video_copy, Some(changed));
    assert_eq!(l.config, before);
    assert!(l.effects.is_empty() && l.take_queue().is_empty());
    // An index outside the list changes nothing (`0x004CEA6B`...`0x004CEA72`), return value 0.
    for index in [-1, 5] {
        let (r, _) = l.send_int("videooptions", "resolution", code::ITEM_SELECTED, index);
        assert_eq!(r, 0, "{index}");
    }
    assert_eq!(l.state.video_copy, Some(changed));
    // Gamma takes effect at once on copy, setting and display.
    l.send_int("videooptions", "gamma", code::VALUE_CHANGED, 60);
    let g = slider_to_gamma(60);
    assert_eq!(l.config.gamma, g);
    assert_eq!(l.effects, [UiEffect::SetGamma(g)]);
    // Leave the page without `ACCEPT` and open it again: the copy comes anew from the settings.
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    assert_eq!(l.state.video_copy, Some(VideoCopy { gamma: g, ..opened }));

    // Live: the copy is there before the queries of the comboboxes (frame before the children).
    let config = EngineConfig {
        screen_x_size: 1280,
        screen_y_size: 1024,
        screen_depth: 32,
        ..EngineConfig::default()
    };
    let opts = MenuOptions {
        display_modes: raw_modes(),
        ..MenuOptions::default()
    };
    let Some(mut d) = live_with(config, opts, Box::new(dispatch)) else {
        return;
    };
    show(&mut d, "videooptions");
    let copy = d.app.screen_state().video_copy.expect("working copy");
    assert_eq!(copy.mode, mode(1280, 1024, 32));
    let resolution = element(&d, "videooptions", "resolution");
    assert_eq!(resolution.items, MODE_TEXTS);
    assert_eq!(resolution.selection, Some(3));
    assert_eq!(element(&d, "videooptions", "particles").selection, Some(0));
    assert_eq!(element(&d, "videooptions", "shadow").selection, Some(1));
    let texture = element(&d, "videooptions", "texturequality");
    assert_eq!(texture.selection, Some(0));
    assert_eq!(slider(&d, "VIDEOOPTIONSGAMMA"), (25, 100));
}

#[test]
fn accept_writes_mode_and_quality_and_queues_reload() {
    // `0x004CE7E6`...`0x004CE8F8`: gamma from the copy; `SetDisplayMode` only on a changed resolution; `{0x1F}` only
    // on a changed quality (all four are written). Return value -1 (jump to `0x004CE8FD`).
    let mut l = Logic::new();
    l.modes = raw_modes();
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    assert_eq!(l.click("videooptions", "accept"), -1);
    assert!(l.effects.is_empty() && l.take_queue().is_empty());

    l.send_int("videooptions", "resolution", code::ITEM_SELECTED, 3);
    l.click("videooptions", "accept");
    assert_eq!(l.effects, [UiEffect::SetDisplayMode(mode(1280, 1024, 32))]);
    assert!(l.take_queue().is_empty());
    let c = &l.config;
    let screen = (c.screen_x_size, c.screen_y_size, c.screen_depth);
    assert_eq!(screen, (1280, 1024, 32));
    l.effects.clear();

    l.send_int("videooptions", "shadow", code::ITEM_SELECTED, 3);
    l.click("videooptions", "accept");
    assert!(l.effects.is_empty(), "{:?}", l.effects);
    assert_eq!(l.take_queue(), [AppCmd::ReloadLevel]);
    let c = &l.config;
    let quality = (
        c.shadow_quality,
        c.particles_quality,
        c.texture_quality,
        c.terrain_quality,
    );
    assert_eq!(quality, (0, 2, 0, 0));
    // `ACCEPT` writes gamma from the copy, without effect on the display.
    l.state.video_copy.as_mut().unwrap().gamma = 2.0;
    l.click("videooptions", "accept");
    assert_eq!(l.config.gamma, 2.0);
    assert!(l.effects.is_empty() && l.take_queue().is_empty());

    // Live: object quality changed in the copy, `ACCEPT` -> click sound; `0x1F` has no effect in the main
    // menu without a match (`GameApp+0xD648`).
    let opts = MenuOptions {
        display_modes: raw_modes(),
        ..MenuOptions::default()
    };
    let logic: Answer = Box::new(|m, env| {
        let r = dispatch(m, env);
        if (m.element.as_str(), m.code) == ("videooptions", code::QUERY_STATUS) {
            env.state.video_copy.as_mut().unwrap().texture = 2;
        }
        r
    });
    let Some(mut d) = live_with(EngineConfig::default(), opts, logic) else {
        return;
    };
    show(&mut d, "videooptions");
    d.app.take_effects();
    d.click("videooptions", "accept");
    d.idle();
    assert_eq!(d.app.take_effects(), [click_sfx()]);
    assert_eq!(d.app.config().texture_quality, 2);
    assert_eq!(d.app.config().screen_x_size, 1024);
}

#[test]
fn gamma_applies_at_once_without_lower_bound() {
    // O-99: `int = min(ftol((G - 0.5) * 50), 99)` without a lower limit; code 1 `G = (f32)v * 0.02 + 0.5` at once.
    let mut l = Logic::new();
    l.config.gamma = 0.1;
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    let value = l.query("videooptions", "gamma", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(-20)));
    let steps = l.query("videooptions", "gamma", code::QUERY_RESOLUTION);
    assert_eq!(steps, (1, Some(100)));
    let (r, _) = l.send_int("videooptions", "gamma", code::VALUE_CHANGED, 50);
    assert_eq!(r, 1);
    assert_eq!(l.config.gamma.to_bits(), 1.5f32.to_bits());
    assert_eq!(l.state.video_copy.unwrap().gamma, 1.5);
    assert_eq!(l.effects, [UiEffect::SetGamma(1.5)]);
    assert_eq!(gamma_to_slider(1.5), 50);
    assert_eq!(gamma_to_slider(9.0), 99, "upper limit");
    // `__ftol2` (`0x005D2A58`): `FISTP qword` toward 0, the low dword; NaN, +-inf and |x| >= 2^63 give 0.
    assert_eq!(video::ftol(f32::NAN), 0);
    assert_eq!(video::ftol(f32::INFINITY), 0);
    assert_eq!(video::ftol(f32::NEG_INFINITY), 0);
    assert_eq!(video::ftol(-1.0e19), 0, "|x| >= 2^63");
    assert_eq!(
        video::ftol(-1.0e12),
        727_384_064,
        "low dword of -999,999,995,904"
    );
    assert_eq!(
        video::ftol(3.0e9),
        -1_294_967_296,
        "2^31 <= x < 2^63: low 32 bits"
    );
    assert_eq!((video::ftol(-2.7), video::ftol(2.7)), (-2, 2), "toward 0");
    assert_eq!(gamma_to_slider(f32::NAN), 0, "ftol(NaN) = 0");
    // (-10^12 - 0.5) * 50 = -50,000,000,188,416 -> low dword 2,009,071,616 -> upper limit 99.
    assert_eq!(gamma_to_slider(-1.0e12), 99, "low dword, then min(..., 99)");

    // Round trip slider -> gamma -> slider in f32 (24-bit x87 like `0x004CE40A`...`0x004CE420` and
    // `0x004CE449`...`0x004CE45B`): 33 values come back one too small, e.g. 1 -> 0.52 - eps -> 0.
    const ONE_LESS: [i32; 33] = [
        1, 4, 7, 8, 10, 14, 15, 20, 21, 23, 27, 28, 29, 30, 40, 41, 42, 43, 46, 53, 54, 59, 60, 67,
        73, 80, 81, 82, 83, 84, 85, 86, 92,
    ];
    let round_trip: Vec<(i32, i32)> = (0..100)
        .map(|v| (v, gamma_to_slider(slider_to_gamma(v))))
        .filter(|&(v, back)| v != back)
        .collect();
    assert_eq!(round_trip, ONE_LESS.map(|v| (v, v - 1)));
    assert_eq!(slider_to_gamma(1).to_bits(), 0x3F05_1EB8);

    // Live: the slider is at -20, without a panic when drawing.
    let config = EngineConfig {
        gamma: 0.1,
        ..EngineConfig::default()
    };
    let Some(mut d) = live_with(config, MenuOptions::default(), Box::new(dispatch)) else {
        return;
    };
    show(&mut d, "videooptions");
    assert_eq!(slider(&d, "VIDEOOPTIONSGAMMA"), (-20, 100));
    assert_eq!(element(&d, "videooptions", "gamma").value, Some(-20));
}

#[test]
fn display_modes_are_filtered_like_the_original() {
    // O-98, `0x004D1475`...`0x004D158B`: width in {800, 1024, 1280, 1400, 1600}, 16/32 bits, first one per (W, H,
    // bits), order of the enumeration.
    assert_eq!(filter_display_modes(&raw_modes()), filtered_modes());
    let few = [mode(1400, 1050, 16), mode(640, 480, 16), mode(1600, 900, 8)];
    assert_eq!(filter_display_modes(&few), [mode(1400, 1050, 16)]);
    assert!(filter_display_modes(&[]).is_empty());
    // The logic filters the raw modes of the host: text `"%d x %d x %d"`, code 0 = first equal entry.
    let mut l = Logic::new();
    l.modes = raw_modes();
    l.config.screen_depth = 32;
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    assert_eq!(l.items("videooptions", "resolution"), MODE_TEXTS);
    let value = l.query("videooptions", "resolution", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(2)));
    assert_eq!(l.send("videooptions", "resolution", code::FREE_ITEMS).0, 1);
}

#[test]
fn resolution_without_match_gives_no_answer() {
    // O-98, `0x004CEAC8`...`0x004CEB3A`: without an equal entry no answer and return value -1.
    let mut l = Logic::new();
    l.modes = raw_modes();
    l.config.screen_x_size = 1920;
    l.config.screen_y_size = 1080;
    l.config.screen_depth = 32;
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    let value = l.query("videooptions", "resolution", code::QUERY_VALUE);
    assert_eq!(value, (-1, None));

    let config = EngineConfig {
        screen_x_size: 1920,
        screen_y_size: 1080,
        screen_depth: 32,
        ..EngineConfig::default()
    };
    let opts = MenuOptions {
        display_modes: raw_modes(),
        ..MenuOptions::default()
    };
    let Some(mut d) = live_with(config, opts, Box::new(dispatch)) else {
        return;
    };
    show(&mut d, "videooptions");
    let resolution = element(&d, "videooptions", "resolution");
    assert_eq!(resolution.items, MODE_TEXTS);
    assert_eq!(resolution.selection, Some(-1));
}

#[test]
fn particles_and_shadow_lists_run_backwards() {
    // `particles`: texts 15043, 15042, 15041, index = 2 - value; `shadow`: 15054 ... 15051, index = 3 - value;
    // `texturequality` 15010 ... 15012 and `terrainquality` 15014 ... 15016 forward.
    let mut l = Logic::new();
    l.config.particles_quality = 0;
    l.config.shadow_quality = 1;
    l.config.texture_quality = 2;
    l.config.terrain_quality = 1;
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    for (element, ids, value) in [
        ("particles", vec![15043, 15042, 15041], 2),
        ("shadow", vec![15054, 15053, 15052, 15051], 2),
        ("texturequality", vec![15010, 15011, 15012], 2),
        ("terrainquality", vec![15014, 15015, 15016], 1),
    ] {
        let expected = l.texts(&ids);
        assert_eq!(l.items("videooptions", element), expected, "{element}");
        let answer = l.query("videooptions", element, code::QUERY_VALUE);
        assert_eq!(answer, (1, Some(value)), "{element}");
    }
    l.send_int("videooptions", "particles", code::ITEM_SELECTED, 1);
    l.send_int("videooptions", "shadow", code::ITEM_SELECTED, 0);
    let copy = l.state.video_copy.unwrap();
    assert_eq!((copy.particles, copy.shadow), (1, 3));
}

#[test]
fn volumes_are_1000_steps_one_to_one() {
    // `FUN_004D1930`: `resolution 1000` overrides the 20 of the `.dlg`; value = volume of the category 1:1
    // (0 = music, 1 = sound effects, 2 = speech), code 1 sets it at once.
    let mut l = Logic::new();
    l.state.speech_playing = true;
    for (element, category, value) in [
        ("soundvolume", 1u8, 599),
        ("musicvolume", 0, 349),
        ("speachvolume", 2, 499),
    ] {
        let steps = l.query("soundoptions", element, code::QUERY_RESOLUTION);
        assert_eq!(steps, (1, Some(1000)), "{element}");
        let answer = l.query("soundoptions", element, code::QUERY_VALUE);
        assert_eq!(answer, (1, Some(value)), "{element}");
        l.effects.clear();
        let (r, _) = l.send_int("soundoptions2", element, code::VALUE_CHANGED, 777);
        assert_eq!(r, 1, "{element}");
        assert_eq!(l.config.sound.volumes[usize::from(category)], 777);
        assert_eq!(l.effects.first(), Some(&volume(category, 777)), "{element}");
    }
    assert!(l.take_queue().is_empty());

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "soundoptions");
    assert_eq!(slider(&d, "SOUNDOPTIONSSOUNDVOLUME"), (599, 1000));
    assert_eq!(slider(&d, "SOUNDOPTIONSMUSICVOLUME"), (349, 1000));
    assert_eq!(slider(&d, "SOUNDOPTIONSSPEACHVOLUME"), (499, 1000));
}

#[test]
fn sfx_test_on_every_report() {
    // O-100: the slider reports code 1 on every call, `soundvolume` plays `SFX_TEST` on every message - also
    // with the same value.
    let mut l = Logic::new();
    for _ in 0..2 {
        l.send_int("soundoptions", "soundvolume", code::VALUE_CHANGED, 300);
    }
    let expected = [volume(1, 300), sfx_test(), volume(1, 300), sfx_test()];
    assert_eq!(l.effects, expected);

    // Live: two clicks on the same point of the slider.
    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "soundoptions");
    d.app.take_effects();
    for _ in 0..2 {
        d.click("soundoptions", "soundvolume");
        d.frame();
    }
    let effects = d.app.take_effects();
    let tests = effects.iter().filter(|e| **e == sfx_test()).count();
    assert_eq!(tests, 2, "{effects:?}");
    let set: Vec<&UiEffect> = effects
        .iter()
        .filter(|e| matches!(e, UiEffect::SetVolume { .. }))
        .collect();
    assert_eq!(set.len(), 2);
    assert_eq!(set[0], set[1]);
}

#[test]
fn speech_test_waits_for_the_previous() {
    // `0x004D1C31`...`0x004D1C8A`: test sentence only when the previous one has faded out; the host reports it via
    // `ScreenState.speech_playing`, the logic sets it itself at the start.
    let mut l = Logic::new();
    l.send_int("soundoptions", "speachvolume", code::VALUE_CHANGED, 400);
    assert_eq!(l.effects, [volume(2, 400), speech_test()]);
    assert!(l.state.speech_playing);
    l.effects.clear();
    l.send_int("soundoptions", "speachvolume", code::VALUE_CHANGED, 410);
    assert_eq!(l.effects, [volume(2, 410)]);
    l.state.speech_playing = false;
    l.effects.clear();
    l.send_int("soundoptions", "speachvolume", code::VALUE_CHANGED, 420);
    assert_eq!(l.effects, [volume(2, 420), speech_test()]);

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "soundoptions");
    d.app.take_effects();
    d.app.set_speech_playing(true);
    d.click("soundoptions", "speachvolume");
    d.frame();
    let effects = d.app.take_effects();
    assert!(!effects.contains(&speech_test()), "{effects:?}");
    let speech_volume = |e: &UiEffect| matches!(e, UiEffect::SetVolume { category: 2, .. });
    assert!(effects.iter().any(speech_volume), "{effects:?}");
    d.app.set_speech_playing(false);
    d.click("soundoptions", "speachvolume");
    d.frame();
    let effects = d.app.take_effects();
    let probes = effects.iter().filter(|e| **e == speech_test()).count();
    assert_eq!(probes, 1, "{effects:?}");
    assert!(d.app.speech_playing());
}

#[test]
fn return2_closes_soundoptions2() {
    // O-100, `0x004D1F41`...`0x004D1FB6`: `RETURN` and `RETURN2` queue `{3,"soundoptions2"}`,
    // `{4,"gameplayoptions2"}` and the remembered return command (free afterwards); return value 0. `RETURN2` is in
    // `SOUNDOPTIONS3` and still closes `soundoptions2`.
    let mut l = Logic::new();
    let back = [
        close("soundoptions2", true),
        close("gameplayoptions2", false),
    ];
    assert_eq!(l.click("soundoptions3", "return2"), 0);
    assert_eq!(l.take_queue(), back);
    l.state.return_cmd = Some(open("ingameoptions", true));
    assert_eq!(l.click("soundoptions2", "return"), 0);
    let mut expected = back.to_vec();
    expected.push(open("ingameoptions", true));
    assert_eq!(l.take_queue(), expected);
    assert_eq!(l.state.return_cmd, None);
    assert_eq!(l.send("soundoptions3", "return2", code::QUERY_STATUS).0, -1);

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "soundoptions2");
    show(&mut d, "soundoptions3");
    assert_eq!(
        d.app.menu().open_names(),
        ["soundoptions2", "soundoptions3"]
    );
    d.click("soundoptions3", "return2");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["soundoptions3"]);
}

#[test]
fn sounddriver_lists_the_host_drivers() {
    // Code 2 from `MenuOptions.sound_drivers`, code 0 = `m_DriverID`, code 4 with a different index sets it,
    // creates the device anew (`CreateInterface`/`InitDevice`) and restarts the menu music (`0x161`).
    let drivers = vec!["Primary sound driver".to_string(), "Second".to_string()];
    let mut l = Logic::new();
    l.sound_drivers = drivers.clone();
    assert_eq!(l.items("soundoptions", "sounddriver"), drivers);
    let value = l.query("soundoptions", "sounddriver", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(0)));
    let (r, _) = l.send_int("soundoptions", "sounddriver", code::ITEM_SELECTED, 0);
    assert_eq!(r, 1);
    assert!(l.effects.is_empty());
    let (r, _) = l.send_int("soundoptions", "sounddriver", code::ITEM_SELECTED, 1);
    assert_eq!(r, 1);
    assert_eq!(l.config.sound.driver_id, 1);
    assert_eq!(
        l.effects,
        [UiEffect::SetSoundDriver(1), UiEffect::RestartMusic]
    );
    assert_eq!(l.send("soundoptions", "sounddriver", code::FREE_ITEMS).0, 1);

    let opts = MenuOptions {
        sound_drivers: drivers.clone(),
        ..MenuOptions::default()
    };
    let Some(mut d) = live_with(EngineConfig::default(), opts, Box::new(dispatch)) else {
        return;
    };
    show(&mut d, "soundoptions");
    let combo = element(&d, "soundoptions", "sounddriver");
    assert_eq!((combo.items, combo.selection), (drivers, Some(0)));
}

#[test]
fn three_d_sound_restarts_the_music() {
    // `0x004D1ADF`...`0x004D1EB5`: `m_IsHardwareOn` = (v != 0), then broadcast `0x161` (menu music anew); the restart
    // of the device is omitted by the reimplementation.
    let mut l = Logic::new();
    let value = l.query("soundoptions", "3dsound", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(0)));
    let (r, _) = l.send_int("soundoptions", "3dsound", code::VALUE_CHANGED, 1);
    assert_eq!(r, 1);
    assert!(l.config.sound.is_hardware_on);
    assert_eq!(l.effects, [UiEffect::RestartMusic]);
    // The remaining switches: `unitresponse` raw in `EngineConfig`, `revertchannels` (v != 0) without reinitialization.
    l.effects.clear();
    l.send_int("soundoptions", "unitresponse", code::VALUE_CHANGED, 0);
    l.send_int("soundoptions", "revertchannels", code::VALUE_CHANGED, 5);
    assert_eq!(l.config.unit_response, 0);
    assert!(l.config.sound.revert_channels);
    let value = l.query("soundoptions", "revertchannels", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(1)));
    assert!(l.effects.is_empty());

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "soundoptions");
    d.app.take_effects();
    d.click("soundoptions", "3dsound");
    d.frame();
    assert_eq!(d.app.take_effects(), [UiEffect::RestartMusic]);
    assert!(d.app.config().sound.is_hardware_on);
}

#[test]
fn gameplay_sliders_have_20_steps() {
    // O-101, `FUN_004CEB80`: code 6 `resolution 20` (the `.dlg` says 10), value = field 1:1.
    let mut l = Logic::new();
    for (element, value) in [
        ("mousescroll", 10),
        ("keyboardscroll", 10),
        ("gamespeed", 14),
        ("mousespeed", 10),
    ] {
        let steps = l.query("gameplayoptions", element, code::QUERY_RESOLUTION);
        assert_eq!(steps, (1, Some(20)), "{element}");
        let answer = l.query("gameplayoptions2", element, code::QUERY_VALUE);
        assert_eq!(answer, (1, Some(value)), "{element}");
        let (r, _) = l.send_int("gameplayoptions", element, code::VALUE_CHANGED, 3);
        assert_eq!(r, 1, "{element}");
    }
    let c = &l.config;
    let sliders = (
        c.mouse_scroll,
        c.keyboard_scroll,
        c.game_speed,
        c.mouse_speed,
    );
    assert_eq!(sliders, (3, 3, 3, 3));
    // Switches: raw; steps do not query them. `gamespeed` code 5 (`0x004CF274[5]` -> `0x004CEE7B`): without a
    // network game no `status`, return value 1 (`JZ 0x004CF215`); the other sliders do not answer code 5.
    l.send_int("gameplayoptions", "zoom", code::VALUE_CHANGED, 7);
    assert_eq!(l.config.enable_zoom, 7);
    let zoom = l.query("gameplayoptions", "zoom", code::QUERY_VALUE);
    assert_eq!(zoom, (1, Some(7)));
    assert_eq!(
        l.send("gameplayoptions", "zoom", code::QUERY_RESOLUTION).0,
        -1
    );
    for dialog in ["gameplayoptions", "gameplayoptions2"] {
        let (r, msg) = l.send(dialog, "gamespeed", code::QUERY_STATUS);
        assert_eq!(r, 1, "{dialog}");
        assert!(!msg.params.contains_key("status"), "{dialog}: no status");
    }
    assert_eq!(
        l.send("gameplayoptions", "mousespeed", code::QUERY_STATUS)
            .0,
        -1
    );
    // `occlusion`: texts 15081 ... 15083, index = value.
    let expected = l.texts(&[15081, 15082, 15083]);
    assert_eq!(l.items("gameplayoptions", "occlusion"), expected);
    l.send_int("gameplayoptions", "occlusion", code::ITEM_SELECTED, 2);
    assert_eq!(l.config.show_occluded, 2);
    assert!(l.effects.is_empty() && l.take_queue().is_empty());

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "gameplayoptions");
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSMOUSESCROLL"), (10, 20));
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSKEYBOARDSCROLL"), (10, 20));
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSGAMESPEED"), (14, 20));
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSMOUSESPEED"), (10, 20));
}

#[test]
fn gamespeed_hidden_for_net_clients() {
    // `0x004CEEB5`...`0x004CEEC2` (re/multiplayer-menus L.7.1, E-315): code 5 of `GAMESPEED` hides the slider in a
    // network session if the machine is not the host (`status 0x10000000`, return value 1); the host and, without a
    // network, everyone sees it (no `status`). The other sliders stay at -1.
    const HIDE: i32 = 0x1000_0000;
    let game = |net| GameInfo {
        game_type: 0,
        local_slot: 1,
        ui_locked: false,
        net,
        playback: false,
    };
    let mut l = Logic::new();
    for (g, status) in [
        (None, None),
        (Some(game(None)), None),
        (Some(game(Some(NetRoleView::Host))), None),
        (Some(game(Some(NetRoleView::Client))), Some(HIDE)),
    ] {
        l.data.game = g;
        let (r, msg) = l.send("gameplayoptions2", "gamespeed", code::QUERY_STATUS);
        assert_eq!((r, int_of(&msg, "status")), (1, status), "{g:?}");
        assert_eq!(
            l.send("gameplayoptions2", "mousespeed", code::QUERY_STATUS)
                .0,
            -1
        );
    }

    let Some(mut d) = live(false, false) else {
        return;
    };
    for (net, shown) in [
        (None, true),
        (Some(NetRoleView::Host), true),
        (Some(NetRoleView::Client), false),
    ] {
        d.app.set_game(Some(game(net)));
        show(&mut d, "gameplayoptions2");
        let e = element(&d, "gameplayoptions2", "gamespeed");
        assert_eq!(e.hidden, !shown, "{net:?}: HIDE");
        d.push(close("gameplayoptions2", true));
        d.idle();
    }
}

#[test]
fn menulevel_changes_only_the_setting() {
    // O-101: `MENULEVEL` sets only `IsMenuLevel` (v != 0), no command `0x20`/`0x21`, no effect.
    let mut l = Logic::new();
    let value = l.query("gameplayoptions", "menulevel", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(1)));
    let (r, _) = l.send_int("gameplayoptions", "menulevel", code::VALUE_CHANGED, 0);
    assert_eq!(r, 1);
    assert!(!l.config.is_menu_level);
    l.send_int("gameplayoptions", "menulevel", code::VALUE_CHANGED, 5);
    assert!(l.config.is_menu_level);
    // Code 1 without `int`: return value 0, setting stays (`0x004CF1A1` -> `0x004CF1A3`).
    let (r, _) = l.send("gameplayoptions", "menulevel", code::VALUE_CHANGED);
    assert_eq!(r, 0);
    assert!(l.config.is_menu_level);
    assert!(l.effects.is_empty() && l.take_queue().is_empty());

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "gameplayoptions");
    d.app.take_effects();
    d.click("gameplayoptions", "menulevel");
    d.idle();
    assert!(!d.app.config().is_menu_level);
    assert!(d.app.take_effects().is_empty());
    assert_eq!(d.app.menu().open_names(), ["gameplayoptions"]);
}

#[test]
fn hardware_cursor_applies_at_once() {
    // `0x004CF0F8`...`0x004CF133`: `HardwareCursor` = (v != 0), `Cursor_SetHardwareCursorVisible` at once.
    let mut l = Logic::new();
    let value = l.query("gameplayoptions", "hardwaremaus", code::QUERY_VALUE);
    assert_eq!(value, (1, Some(1)));
    l.send_int("gameplayoptions", "hardwaremaus", code::VALUE_CHANGED, 0);
    assert!(!l.config.hardware_cursor);
    assert_eq!(l.effects, [UiEffect::SetHardwareCursor(false)]);
    // Code 1 without `int`: return value 0 without `Cursor_SetHardwareCursorVisible` (`0x004CF10B` -> `0x004CF1A3`).
    l.effects.clear();
    let (r, _) = l.send("gameplayoptions", "hardwaremaus", code::VALUE_CHANGED);
    assert_eq!(r, 0);
    assert!(!l.config.hardware_cursor);
    assert!(l.effects.is_empty(), "{:?}", l.effects);

    let Some(mut d) = live(false, false) else {
        return;
    };
    show(&mut d, "gameplayoptions");
    d.app.take_effects();
    d.click("gameplayoptions", "hardwaremaus");
    d.frame();
    assert_eq!(d.app.take_effects(), [UiEffect::SetHardwareCursor(false)]);
    assert!(!d.app.config().hardware_cursor);
}

#[test]
fn out_of_range_settings_do_not_panic() {
    // Values outside the menu ranges are taken over unchecked by the original; sliders, comboboxes and
    // drawing must not abort.
    let mut config = EngineConfig {
        game_speed: 99,
        screen_x_size: 0,
        show_occluded: 7,
        gamma: 0.1,
        particles_quality: i32::MIN,
        shadow_quality: 99,
        texture_quality: -5,
        mouse_speed: i32::MAX,
        ..EngineConfig::default()
    };
    config.sound.volumes = [5000, -3, i32::MIN];
    let mut l = Logic::new();
    l.config = config.clone();
    l.modes = raw_modes();
    l.send("videooptions", "videooptions", code::QUERY_STATUS);
    let particles = l.query("videooptions", "particles", code::QUERY_VALUE);
    assert_eq!(particles, (1, Some(2i32.wrapping_sub(i32::MIN))));
    let shadow = l.query("videooptions", "shadow", code::QUERY_VALUE);
    assert_eq!(shadow, (1, Some(-96)));
    l.send_int("videooptions", "particles", code::ITEM_SELECTED, i32::MIN);
    l.send_int("videooptions", "gamma", code::VALUE_CHANGED, i32::MAX);
    l.click("videooptions", "accept");

    let opts = MenuOptions {
        display_modes: raw_modes(),
        ..MenuOptions::default()
    };
    let Some(mut d) = live_with(config, opts, Box::new(dispatch)) else {
        return;
    };
    show(&mut d, "videooptions");
    let resolution = element(&d, "videooptions", "resolution");
    assert_eq!(resolution.selection, Some(-1));
    d.click("videooptions", "accept");
    d.idle();
    show(&mut d, "gameplayoptions");
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSGAMESPEED"), (99, 20));
    assert_eq!(slider(&d, "GAMEPLAYOPTIONSMOUSESPEED"), (i32::MAX, 20));
    let occlusion = element(&d, "gameplayoptions", "occlusion");
    assert_eq!(occlusion.selection, Some(7));
    show(&mut d, "soundoptions");
    assert_eq!(slider(&d, "SOUNDOPTIONSMUSICVOLUME"), (5000, 1000));
    assert_eq!(slider(&d, "SOUNDOPTIONSSOUNDVOLUME"), (-3, 1000));
    d.click("soundoptions", "soundvolume");
    d.idle();
}

// Profiles, `SELECTPROFILE`, `SINGLEPLAYERMENU`, `LOADAUTOPLAY` (re/singleplayer-menus part 1, tables
// 1.1 and 1.3, 6.3).

/// Status `HIDE`.
const HIDE: i32 = 0x1000_0000;

/// Profiles with these names (GUID 0, 1, ...), `current` active.
fn profiles_with(names: &[&str], current: i32) -> ProfilesFile {
    let mut p = Profiles::default();
    for name in names {
        p.create(&mut name_field(name), &CampaignTable::default())
            .unwrap();
    }
    p.select(current);
    p.file
}

/// Small campaign table: per campaign one heading and `n` levels.
fn small_campaigns(n: usize) -> CampaignTable {
    let entry = |kind| CampaignEntry {
        kind,
        name_id: 1,
        text_id: 1,
        stop: false,
        path: String::new(),
    };
    let campaign = || {
        std::iter::once(entry(EntryKind::Headline))
            .chain((0..n).map(|_| entry(EntryKind::Level)))
            .collect()
    };
    CampaignTable {
        campaigns: [campaign(), campaign(), campaign(), campaign()],
    }
}

fn rq(element: &str, code: u32) -> Requery {
    Requery::new("selectprofile", element, code)
}

impl Logic {
    fn with_profiles(names: &[&str], current: i32) -> Logic {
        let mut l = Logic::new();
        l.data.profiles = Profiles::new(profiles_with(names, current));
        l
    }

    fn names(&self) -> Vec<String> {
        self.data
            .profiles
            .file
            .profiles
            .iter()
            .map(|p| name_text(&p.name))
            .collect()
    }

    fn net_name(&self) -> String {
        name_text(&self.config.net_name)
    }

    fn status(&mut self, dialog: &str, element: &str) -> (i32, Option<i32>) {
        let (r, msg) = self.send(dialog, element, code::QUERY_STATUS);
        (r, int_of(&msg, "status"))
    }
}

/// `MenuApp` over the corpus with these profiles and `NetName`, without a start sequence.
fn live_profiles(names: &[&str], current: i32, net_name: &str) -> Option<Driver<Corpus>> {
    let config = EngineConfig {
        net_name: name_field(net_name),
        ..EngineConfig::default()
    };
    let opts = MenuOptions {
        profiles: profiles_with(names, current),
        ..MenuOptions::default()
    };
    live_with(config, opts, Box::new(dispatch))
}

/// Types `text` into the focused field (one key with a character per character).
fn type_text(d: &mut Driver<Corpus>, text: &str) {
    for c in text.chars() {
        let vk = c.to_ascii_uppercase() as u8;
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: vec![vk],
                ch: Some(c as u16),
            },
            S1280,
            now,
        );
    }
    d.frame();
}

fn list_names(d: &Driver<Corpus>) -> (Vec<String>, Option<i32>) {
    let e = element(d, "selectprofile", "profilelist");
    (e.items, e.selection)
}

/// Only the effects of the profile management (without click sounds and the `0x20` of the backgrounds).
fn profile_effects(effects: Vec<UiEffect>) -> Vec<UiEffect> {
    effects
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                UiEffect::WriteProfiles | UiEffect::DeleteProfileSaves { .. }
            )
        })
        .collect()
}

#[test]
fn start_with_profile_opens_singleplayer_menu_and_copies_name() {
    // `0x004D1764`...`0x004D17B9`: with an active profile `NetName` = profile name (`FUN_0040C060`),
    // `{5,"singleplayermenu"}`.
    let mut l = Logic::with_profiles(&["Anna", "Bert"], 1);
    l.config.net_name = name_field("Zed");
    assert_eq!(l.click("start", "singleplayermenu"), 1);
    assert_eq!(
        l.take_queue(),
        [close("start", true), open("singleplayermenu", true)]
    );
    assert_eq!(l.net_name(), "Bert");
    assert_eq!(l.state.net_name.get(), "Bert");
    assert!(l.effects.is_empty());
    // Without a profile `NetName` stays.
    let mut l = Logic::new();
    l.config.net_name = name_field("Zed");
    l.click("start", "singleplayermenu");
    assert_eq!(l.net_name(), "Zed");

    // Live: the field `PROFILENAME` (locked from the file) shows the name.
    let Some(mut d) = live_profiles(&["Anna", "Bert"], 1, "Zed") else {
        return;
    };
    show(&mut d, "start");
    d.click("start", "singleplayermenu");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["singleplayermenu"]);
    let field = element(&d, "singleplayermenu", "profilename");
    assert_eq!(field.text.as_deref(), Some("Bert"));
    assert!(!field.enabled, "DISABLE from the file");
    assert_eq!(name_text(&d.app.config().net_name), "Bert");
}

#[test]
fn create_profile_unlocks_entries_0_and_1() {
    // `create` code 8 (`0x004DB4AE`...`0x004DB634`): `FUN_004C8430`, write `profiles.ini`, Sel = new index, query
    // again `profilelist` 2, `ok` 5, `delete` 5.
    let mut l = Logic::new();
    l.config.net_name = name_field("Anna");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.names(), ["Anna"]);
    let levels: Vec<(i32, i32)> = l.data.profiles.file.profiles[0]
        .levels
        .iter()
        .map(|r| (r.campaign, r.level))
        .collect();
    assert_eq!(
        levels,
        [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (2, 0),
            (2, 1),
            (3, 0),
            (3, 1)
        ]
    );
    assert_eq!(l.data.profiles.file.current, -1, "created, not active");
    assert_eq!(l.state.profile_selection, 0);
    assert_eq!(l.effects, [UiEffect::WriteProfiles]);
    assert_eq!(
        l.requery,
        [rq("profilelist", 2), rq("ok", 5), rq("delete", 5)]
    );
    assert!(l.take_queue().is_empty());

    // Live: type a name, create - the list shows the new profile marked, OK and Delete appear.
    let Some(mut d) = live_profiles(&[], -1, "") else {
        return;
    };
    show(&mut d, "selectprofile");
    assert!(element(&d, "selectprofile", "ok").hidden);
    assert!(element(&d, "selectprofile", "delete").hidden);
    d.click("selectprofile", "profilename");
    d.frame();
    type_text(&mut d, "Anna");
    assert_eq!(name_text(&d.app.config().net_name), "Anna", "Code 0xC");
    d.app.take_effects();
    d.click("selectprofile", "create");
    d.idle();
    assert_eq!(list_names(&d), (vec!["Anna".to_string()], Some(0)));
    assert!(!element(&d, "selectprofile", "ok").hidden);
    assert!(!element(&d, "selectprofile", "delete").hidden);
    assert_eq!(
        profile_effects(d.app.take_effects()),
        [UiEffect::WriteProfiles]
    );
    assert_eq!(d.app.profiles().file.profiles.len(), 1);
}

#[test]
fn create_with_rd_prefix_unlocks_everything_and_strips_prefix() {
    // O-27: `[RD]` is dropped (the field shows the shortened name at once), `FUN_0040C380` unlocks everything.
    let mut l = Logic::new();
    l.data.campaigns = small_campaigns(2);
    l.config.net_name = name_field("[RD]Max");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.names(), ["Max"]);
    assert_eq!(l.net_name(), "Max");
    assert_eq!(l.state.net_name.get(), "Max");
    assert_eq!(l.data.profiles.file.profiles[0].levels.len(), 12, "4 x 3");
    // The prefix is also dropped on failure.
    l.config.net_name = name_field("[RD]Max");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.names(), ["Max"]);
    assert_eq!(l.net_name(), "Max");
    assert_eq!(l.state.info_text, Some(15748));

    // In operation with the campaign table of the installation: every entry free.
    let Some(mut d) = live_profiles(&[], -1, "") else {
        return;
    };
    let total: usize = d.app.data().campaigns.campaigns.iter().map(Vec::len).sum();
    assert!(total > 8, "campaign.ini loaded");
    show(&mut d, "selectprofile");
    d.click("selectprofile", "profilename");
    d.frame();
    type_text(&mut d, "[RD]Max");
    d.click("selectprofile", "create");
    d.idle();
    assert_eq!(
        element(&d, "selectprofile", "profilename").text.as_deref(),
        Some("Max")
    );
    let p = &d.app.profiles().file.profiles[0];
    assert_eq!(name_text(&p.name), "Max");
    assert_eq!(p.levels.len(), total);
}

#[test]
fn create_empty_name_does_nothing() {
    // O-123: `wcslen(NetName) < 1` -> return value -1, no message, no command, no writing.
    let mut l = Logic::new();
    assert_eq!(l.click("selectprofile", "create"), -1);
    assert!(l.names().is_empty());
    assert!(l.effects.is_empty() && l.requery.is_empty());
    assert!(l.take_queue().is_empty());
    // A name made of spaces is allowed.
    l.config.net_name = name_field(" ");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.names(), [" "]);
}

#[test]
fn create_duplicate_shows_15748() {
    // O-123: name exists (`_wcscmp`) -> message 15748, return value 1, nothing else; different case -> created.
    let mut l = Logic::with_profiles(&["Anna"], 0);
    l.config.net_name = name_field("Anna");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.take_queue(), [open("msgbox", true)]);
    assert_eq!(l.state.info_text, Some(15748));
    assert_eq!(l.state.info_ok, None);
    assert!(l.effects.is_empty() && l.requery.is_empty());
    assert_eq!(l.names(), ["Anna"]);
    l.config.net_name = name_field("anna");
    assert_eq!(l.click("selectprofile", "create"), 1);
    assert_eq!(l.names(), ["Anna", "anna"]);
    assert_eq!(l.effects, [UiEffect::WriteProfiles]);
}

#[test]
fn ok_and_delete_hidden_without_selection() {
    // `profilename` code 5: Sel = cur (return value -1); `ok`/`delete` code 5: `HIDE` without a selection;
    // `profilelist` code 0/2/4.
    let mut l = Logic::new();
    assert_eq!(
        l.send("selectprofile", "profilename", code::QUERY_STATUS).0,
        -1
    );
    assert_eq!(l.state.profile_selection, -1);
    assert_eq!(l.status("selectprofile", "ok"), (1, Some(HIDE)));
    assert_eq!(l.status("selectprofile", "delete"), (1, Some(HIDE)));
    assert_eq!(
        l.query("selectprofile", "profilelist", code::QUERY_VALUE),
        (1, Some(-1))
    );
    assert!(l.items("selectprofile", "profilelist").is_empty());

    let mut l = Logic::with_profiles(&["Anna", "Bert"], 1);
    l.send("selectprofile", "profilename", code::QUERY_STATUS);
    assert_eq!(l.state.profile_selection, 1);
    assert_eq!(l.status("selectprofile", "ok"), (1, Some(0)));
    assert_eq!(l.status("selectprofile", "delete"), (1, Some(0)));
    assert_eq!(l.items("selectprofile", "profilelist"), ["Anna", "Bert"]);
    assert_eq!(
        l.send_int("selectprofile", "profilelist", code::ITEM_SELECTED, 0)
            .0,
        1
    );
    assert_eq!(l.state.profile_selection, 0);
    assert_eq!(l.requery, [rq("ok", 5), rq("delete", 5)]);
    assert_eq!(
        l.query("selectprofile", "profilelist", code::QUERY_VALUE),
        (1, Some(0))
    );
    // Code 4 without `int`: 0, nothing changed; double-click, free and the frame: not handled.
    assert_eq!(
        l.send("selectprofile", "profilelist", code::ITEM_SELECTED)
            .0,
        0
    );
    assert_eq!(l.state.profile_selection, 0);
    for c in [code::DOUBLE_CLICK, code::FREE_ITEMS] {
        assert_eq!(l.send("selectprofile", "profilelist", c).0, -1);
    }
    assert_eq!(
        l.send("selectprofile", "selectprofile", code::QUERY_STATUS)
            .0,
        -1
    );
    // No character filter (O-125): code 0xB stays unanswered, every character from 0x20 gets into the field.
    assert_eq!(l.send("selectprofile", "profilename", code::EDIT_KEY).0, -1);

    // In operation with an active profile: both visible, the list marks the active one.
    let Some(mut d) = live_profiles(&["Anna", "Bert"], 1, "Bert") else {
        return;
    };
    show(&mut d, "selectprofile");
    assert_eq!(
        list_names(&d),
        (vec!["Anna".to_string(), "Bert".to_string()], Some(1))
    );
    assert!(!element(&d, "selectprofile", "ok").hidden);
    assert!(!element(&d, "selectprofile", "delete").hidden);
}

#[test]
fn ok_activates_the_selection_and_writes() {
    // `ok` code 8: `FUN_004C83F0(Sel)` (active, name to `NetName`, write), `{3}`, `{5,"singleplayermenu"}`.
    let mut l = Logic::with_profiles(&["Anna", "Bert"], 0);
    l.state.profile_selection = 1;
    assert_eq!(l.click("selectprofile", "ok"), 1);
    assert_eq!(l.data.profiles.file.current, 1);
    assert_eq!(l.net_name(), "Bert");
    assert_eq!(l.state.net_name.get(), "Bert");
    assert_eq!(l.effects, [UiEffect::WriteProfiles]);
    assert_eq!(
        l.take_queue(),
        [close("selectprofile", true), open("singleplayermenu", true)]
    );
}

#[test]
fn delete_asks_15747_and_reopens() {
    // `delete` code 8: confirmation prompt 15747 with OK `{0x29, Sel}` (Sel at the click), without a cancel command.
    let mut l = Logic::with_profiles(&["Anna", "Bert"], 0);
    l.state.profile_selection = 1;
    assert_eq!(l.click("selectprofile", "delete"), 1);
    assert_eq!(l.take_queue(), [open("infoframe", true)]);
    assert_eq!(l.state.info_text, Some(15747));
    assert_eq!(l.state.info_ok, Some(AppCmd::DeleteProfile(1)));
    assert_eq!(l.state.info_cancel, None);
    assert_eq!(l.names(), ["Anna", "Bert"], "only the command deletes");

    // Live: OK of the confirmation prompt -> `0x29`: profile gone, savegames and file, `selectprofile` anew
    // (Sel = cur).
    let Some(mut d) = live_profiles(&["Anna", "Bert"], 0, "Anna") else {
        return;
    };
    show(&mut d, "selectprofile");
    d.click("selectprofile", "delete");
    d.idle();
    assert!(d.find("INFOFRAME").is_some());
    d.app.take_effects();
    d.click("infoframe", "ok");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["selectprofile"]);
    assert_eq!(list_names(&d), (vec!["Bert".to_string()], Some(0)));
    assert_eq!(
        profile_effects(d.app.take_effects()),
        [
            UiEffect::DeleteProfileSaves { guid: 0 },
            UiEffect::WriteProfiles
        ]
    );
    // O-122: `NetName` stays the name of the deleted profile.
    assert_eq!(name_text(&d.app.config().net_name), "Anna");
    assert_eq!(d.app.profiles().current_name(), "Bert");
}

#[test]
fn deleting_an_earlier_profile_keeps_the_active_one() {
    // Deviation (O-121): deleting before the active one adjusts the index; in the original index 2 would stay
    // active, invalid after deleting.
    let Some(mut d) = live_profiles(&["Anna", "Bert", "Cleo"], 2, "Cleo") else {
        return;
    };
    d.push(AppCmd::DeleteProfile(0));
    d.idle();
    let p = d.app.profiles();
    assert_eq!((p.file.current, p.current_name().as_str()), (1, "Cleo"));
    assert_eq!(p.current_guid(), Some(2));
    assert_eq!(d.app.menu().open_names(), ["selectprofile"]);
    assert_eq!(
        list_names(&d),
        (vec!["Bert".to_string(), "Cleo".to_string()], Some(1))
    );
    // Invalid index: no savegames, only writing; `selectprofile` anew.
    d.app.take_effects();
    d.push(AppCmd::DeleteProfile(7));
    d.idle();
    assert_eq!(
        profile_effects(d.app.take_effects()),
        [UiEffect::WriteProfiles]
    );
    assert_eq!(d.app.profiles().file.profiles.len(), 2);
}

#[test]
fn cancel_without_profile_returns_to_start() {
    // `cancel` code 8: `{3}`; without an active profile `{5,"start"}`, otherwise `{5,"singleplayermenu"}`.
    let mut l = Logic::with_profiles(&["Anna"], -1);
    assert_eq!(l.click("selectprofile", "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [close("selectprofile", true), open("start", true)]
    );
    let mut l = Logic::with_profiles(&["Anna"], 0);
    assert_eq!(l.click("selectprofile", "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [close("selectprofile", true), open("singleplayermenu", true)]
    );
    assert!(l.effects.is_empty());
}

#[test]
fn singleplayer_menu_buttons_follow_menu_flow() {
    // `FUN_004DA150`: six buttons, each `{3, frame}` and `{5, target}`; no code 5 branch (O-125).
    let mut l = Logic::new();
    let buttons = [
        ("selectprofile", "selectprofile"),
        ("selectcampaign", "selectcampaign"),
        ("loadsavedgame", "loadsavedgame2"),
        ("createsingleplayergame", "createsingleplayergame"),
        ("loadautoplay", "loadautoplay"),
        ("cancel", "start"),
    ];
    for (button, target) in buttons {
        assert_eq!(l.click("singleplayermenu", button), 1, "{button}");
        assert_eq!(
            l.take_queue(),
            [close("singleplayermenu", true), open(target, true)],
            "{button}"
        );
        assert_eq!(l.status("singleplayermenu", button).0, -1, "{button}");
    }
    assert_eq!(l.click("singleplayermenu", "nirgends"), -1);
    // `profilename` code 7: pointer to `NetName`, `size 30`, `inputbuffer 1`.
    l.config.net_name = name_field("Anna");
    let (r, msg) = l.send("singleplayermenu", "profilename", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.net_name.clone()))
    );
    assert_eq!(l.state.net_name.get(), "Anna");
    assert_eq!(int_of(&msg, "size"), Some(30));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    assert_eq!(l.status("singleplayermenu", "profilename").0, -1);
    assert!(l.effects.is_empty());

    // Live: all buttons free, `CANCEL` back to the main menu.
    let Some(mut d) = live_profiles(&["Anna"], 0, "Anna") else {
        return;
    };
    show(&mut d, "singleplayermenu");
    for (button, _) in buttons {
        let b = element(&d, "singleplayermenu", button);
        assert!(b.enabled && !b.hidden, "{button}");
    }
    d.click("singleplayermenu", "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["logo", "start"]);
}

/// The logic with recordings in the order the host delivers (ascending by name, the fake delivers them pre-sorted).
fn autoplay_logic() -> Logic {
    use crate::testutil::{FakeSaveFiles, stamp};
    let mut l = Logic::new();
    let files = FakeSaveFiles::new(&[], stamp(2026, 11, 8, 15, 0)).with_autoplay(&[
        ("261108-0900", stamp(2026, 11, 8, 9, 0)),
        ("261108-1435", stamp(2026, 11, 8, 14, 35)),
        ("x.rec", stamp(2026, 11, 7, 20, 5)),
    ]);
    l.data.saves = crate::saves::Saves(Rc::new(files));
    l
}

#[test]
fn loadautoplay_lists_the_autoplay_folder_by_name() {
    // `DIR` code 2 sets the directory `Autoplay` (`FUN_004BFC40(3)`) and lists it with the full file name in the
    // order of the host (`display_name`); no preselection, flag 0.
    let mut l = autoplay_logic();
    assert_eq!(l.query("loadautoplay", "dir", code::QUERY_VALUE), (1, None));
    assert_eq!(
        l.items("loadautoplay", "dir"),
        [
            "(26/11/08 9:00) 261108-0900",
            "(26/11/08 14:35) 261108-1435",
            "(26/11/07 20:05) x.rec"
        ]
    );
    assert_eq!(l.state.save_dir, crate::saves::SaveDir::Autoplay);
    assert_eq!(
        l.state.save_entries,
        ["261108-0900", "261108-1435", "x.rec"]
    );
    assert!(!l.state.autoplay_selected);
    assert_eq!(l.send("loadautoplay", "dir", code::FREE_ITEMS).0, 1);
}

#[test]
fn loadautoplay_start_needs_a_selection() {
    let mut l = autoplay_logic();
    l.items("loadautoplay", "dir");
    assert_eq!(l.status("loadautoplay", "start"), (1, Some(HIDE)));
    assert_eq!(l.status("loadautoplay", "deletefile"), (1, Some(HIDE)));
    // ENTER also hits the hidden `START` (`CHECKENTER`, O-186): without a choice it has no effect in the
    // reimplementation.
    l.click("loadautoplay", "start");
    assert!(l.take_queue().is_empty());
    assert!(l.effects.is_empty());
    let (r, _) = l.send_int("loadautoplay", "dir", code::ITEM_SELECTED, 1);
    assert_eq!(r, 1);
    assert_eq!(l.state.save_name.get(), "261108-1435");
    assert_eq!(l.status("loadautoplay", "start"), (1, Some(0)));
    assert_eq!(l.status("loadautoplay", "deletefile"), (1, Some(0)));
    // New list: the flag falls back.
    l.items("loadautoplay", "dir");
    assert_eq!(l.status("loadautoplay", "start"), (1, Some(HIDE)));
}

#[test]
fn loadautoplay_start_queues_close_wait_and_0x19() {
    let mut l = autoplay_logic();
    l.items("loadautoplay", "dir");
    l.send_int("loadautoplay", "dir", code::ITEM_SELECTED, 2);
    assert_eq!(l.click("loadautoplay", "start"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("loadautoplay", true),
            AppCmd::WaitDialogs,
            AppCmd::PlayReplay
        ]
    );
}

#[test]
fn loadautoplay_delete_asks_15174_and_deletes_the_full_name() {
    let mut l = autoplay_logic();
    l.items("loadautoplay", "dir");
    l.send_int("loadautoplay", "dir", code::ITEM_SELECTED, 2);
    assert_eq!(l.click("loadautoplay", "deletefile"), 1);
    assert_eq!(l.take_queue(), [open("infoframe", true)]);
    assert_eq!(l.state.info_text, Some(15174));
    assert_eq!(
        l.state.info_ok,
        Some(AppCmd::DeleteSave("loadautoplay".into()))
    );
    assert_eq!(l.state.info_cancel, None);
}

#[test]
fn loadautoplay_delete_effect_carries_dir_and_reopens_the_frame() {
    // `0x2A` deletes the full name in the recordings folder and opens the frame anew.
    use crate::testutil::{FakeSaveFiles, stamp};
    // Enough rows so that the middle of the list hits one.
    let names: Vec<String> = (0..60).map(|i| format!("r{i:02}.rec")).collect();
    let entries: Vec<(&str, _)> = names
        .iter()
        .map(|n| (n.as_str(), stamp(2026, 11, 7, 20, 5)))
        .collect();
    let files = Rc::new(FakeSaveFiles::new(&[], stamp(2026, 11, 8, 15, 0)).with_autoplay(&entries));
    let opts = MenuOptions {
        profiles: profiles_with(&["Anna"], 0),
        saves: Some(files.clone() as Rc<dyn crate::saves::SaveFiles>),
        ..MenuOptions::default()
    };
    let Some(mut d) = live_with(EngineConfig::default(), opts, Box::new(dispatch)) else {
        return;
    };
    d.push(open("loadautoplay", true));
    d.idle();
    d.click("loadautoplay", "dir");
    d.idle();
    let row = element(&d, "loadautoplay", "dir").selection.unwrap();
    assert!(row >= 0, "a click selects a row");
    assert!(!element(&d, "loadautoplay", "deletefile").hidden);
    d.click("loadautoplay", "deletefile");
    d.idle();
    d.app.take_effects();
    d.click("infoframe", "ok");
    let until = d.now + 2000;
    let deletes = loop {
        assert!(d.now < until, "no delete effect");
        d.frame();
        let found: Vec<UiEffect> = d
            .app
            .take_effects()
            .into_iter()
            .filter(|e| matches!(e, UiEffect::DeleteSave { .. }))
            .collect();
        if !found.is_empty() {
            break found;
        }
    };
    assert_eq!(
        deletes,
        [UiEffect::DeleteSave {
            name: names[row as usize].clone(),
            dir: crate::saves::SaveDir::Autoplay
        }]
    );
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["loadautoplay"]);
}

#[test]
fn loadautoplay_cancel_returns_to_the_main_folder() {
    // `CANCEL`: `FUN_004BFC40(1)`, `{3, frame}`, `{5,"singleplayermenu"}`.
    let mut l = autoplay_logic();
    l.items("loadautoplay", "dir");
    assert_eq!(l.click("loadautoplay", "cancel"), 1);
    assert_eq!(l.state.save_dir, crate::saves::SaveDir::Main);
    assert_eq!(
        l.take_queue(),
        [close("loadautoplay", true), open("singleplayermenu", true)]
    );
}

#[test]
fn loadautoplay_opens_from_the_singleplayer_menu() {
    let Some(mut d) = live_profiles(&["Anna"], 0, "Anna") else {
        return;
    };
    show(&mut d, "singleplayermenu");
    d.click("singleplayermenu", "loadautoplay");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["loadautoplay"]);
    assert!(element(&d, "loadautoplay", "start").hidden);
    assert!(element(&d, "loadautoplay", "deletefile").hidden);
    assert!(element(&d, "loadautoplay", "dir").items.is_empty());
    // ENTER hits the hidden `START` (`CHECKENTER`, O-186): without a choice nothing; ESC = `CANCEL`.
    d.key(RETURN);
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["loadautoplay"]);
    assert_eq!(d.triggered("start"), 1);
    d.key(ESCAPE);
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["singleplayermenu"]);
}

#[test]
fn net_name_survives_cancel() {
    // O-122: typing in `PROFILENAME` writes `NetName` directly; `CANCEL` does not undo that, the single player menu
    // shows the typed name, the profile keeps its old name.
    let Some(mut d) = live_profiles(&["Anna"], 0, "Anna") else {
        return;
    };
    show(&mut d, "selectprofile");
    assert_eq!(
        element(&d, "selectprofile", "profilename").text.as_deref(),
        Some("Anna")
    );
    d.click("selectprofile", "profilename");
    d.frame();
    type_text(&mut d, "X");
    d.click("selectprofile", "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["singleplayermenu"]);
    assert_eq!(name_text(&d.app.config().net_name), "AnnaX");
    assert_eq!(
        element(&d, "singleplayermenu", "profilename")
            .text
            .as_deref(),
        Some("AnnaX")
    );
    assert_eq!(d.app.profiles().current_name(), "Anna");
}

mod campaign;
mod chat;
mod create_game;
mod diplomacy;
mod game_end;
mod ingame;
mod lan1;
mod lan2;
mod lan31;
mod lan4;
mod player_slots;
mod save_load;
