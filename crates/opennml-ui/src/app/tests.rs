//! Tests of the dialog stack, the command queue and `MenuApp`. Corpus dialogs (`start`, `logo`, `options`,
//! `videooptions`, `msgbox`, `credits`, `gameendvictory`) need `NML_GAME_DIR` and silently skip themselves otherwise;
//! ComboBox cases run with test dialogs on the fixture.

use std::cell::RefCell;
use std::rc::Rc;

use opennml_formats::dlg::upper_word;
use opennml_rda::testutil::corpus_game_dir;

use super::*;
use crate::assets::{UiAssets, load_localisation};
use crate::dialog::tests::{Fixture, fixture};
use crate::draw::{Blend, Filter};
use crate::fonts::FontBook;
use crate::input::MouseButton;
use crate::msg::{MenuLevelEffect, Param, code};
use crate::video::{INTRO_VIDEOS, VIDEO_TEXTURE, video_rect};

pub(crate) const S1280: Screen = Screen {
    width: 1280,
    height: 1024,
};

/// Frame rate of the tests.
pub(crate) const DT: u64 = 16;

/// Installation with fonts for 1280 pixels width.
pub(crate) struct Corpus {
    pub(crate) vfs: Vfs,
    pub(crate) assets: UiAssets,
    pub(crate) fonts: FontBook,
    pub(crate) texts: Texts,
}

pub(crate) trait World {
    fn ctx(&mut self, screen: Screen) -> UiContext<'_>;
    fn vfs(&self) -> &Vfs;
    fn texts(&self) -> &Texts;
}

impl World for Corpus {
    fn ctx(&mut self, screen: Screen) -> UiContext<'_> {
        UiContext {
            vfs: &self.vfs,
            assets: &mut self.assets,
            fonts: &self.fonts,
            texts: &self.texts,
            screen,
        }
    }
    fn vfs(&self) -> &Vfs {
        &self.vfs
    }
    fn texts(&self) -> &Texts {
        &self.texts
    }
}

impl World for Fixture {
    fn ctx(&mut self, screen: Screen) -> UiContext<'_> {
        Fixture::ctx(self, screen)
    }
    fn vfs(&self) -> &Vfs {
        &self.vfs
    }
    fn texts(&self) -> &Texts {
        &self.texts
    }
}

pub(crate) fn corpus() -> Option<Corpus> {
    let vfs = Vfs::open_game_dir(corpus_game_dir()?).expect("installation cannot be opened");
    let loc = load_localisation(&vfs).expect("localisation loads");
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(&vfs, &loc, 1280, &mut assets, None);
    let texts = Texts::load(&vfs, &loc);
    Some(Corpus {
        vfs,
        assets,
        fonts,
        texts,
    })
}

/// Recorded messages (dialog, element, code).
pub(crate) type Log = Rc<RefCell<Vec<(String, String, u32)>>>;

/// `MenuApp` with world, clock and a listener dummy that records every message and (if set) lets `answer` answer it.
pub(crate) struct Driver<W: World> {
    pub(crate) world: W,
    pub(crate) app: MenuApp,
    pub(crate) now: u64,
    pub(crate) log: Log,
}

pub(crate) type Answer = Box<dyn FnMut(&mut ElementMsg, &mut ScreenEnv) -> i32>;

impl<W: World> Driver<W> {
    pub(crate) fn with_app(world: W, app: MenuApp, mut answer: Answer) -> Driver<W> {
        let mut app = app;
        let log: Log = Rc::default();
        let l = log.clone();
        app.logic = Some(Box::new(move |msg, env| {
            l.borrow_mut()
                .push((msg.dialog.clone(), msg.element.clone(), msg.code));
            answer(msg, env)
        }));
        Driver {
            world,
            app,
            now: 0,
            log,
        }
    }

    /// One frame at the current time; afterwards the clock advances by [`DT`].
    pub(crate) fn frame(&mut self) -> Vec<DrawCmd> {
        let mut ctx = self.world.ctx(S1280);
        let out = self.app.frame(&mut ctx, self.now);
        self.now += DT;
        out
    }

    /// Frames until the clock has reached `until`.
    pub(crate) fn run_until(&mut self, until: u64) {
        while self.now <= until {
            self.frame();
        }
    }

    pub(crate) fn push(&mut self, cmd: AppCmd) {
        self.app.queue.push(cmd);
    }

    /// Left click on an element in the next frame (movement, press, release).
    pub(crate) fn click(&mut self, dialog: &str, element: &str) {
        let (x, y) = self
            .app
            .element_center(dialog, element, S1280)
            .unwrap_or_else(|e| panic!("{e}"));
        let now = self.now;
        self.app.input(UiInput::Move { x, y }, S1280, now);
        self.app.input(UiInput::Down(MouseButton::Left), S1280, now);
        self.app.input(UiInput::Up(MouseButton::Left), S1280, now);
    }

    pub(crate) fn find(&self, key: &str) -> Option<NodeId> {
        self.app.tree.find(key)
    }

    pub(crate) fn triggered(&self, element: &str) -> usize {
        self.log
            .borrow()
            .iter()
            .filter(|m| m.1 == element && m.2 == code::TRIGGERED)
            .count()
    }

    /// Key `vk` (held alone, character = `vk`) in the next frame.
    pub(crate) fn key(&mut self, vk: u8) {
        let now = self.now;
        self.app.input(
            UiInput::Key {
                held: vec![vk],
                ch: Some(u16::from(vk)),
            },
            S1280,
            now,
        );
    }

    /// Frames until the stack rests and the queue is empty (at most 2 s), and one more so that a dialog built in the
    /// last command is drawn too.
    pub(crate) fn idle(&mut self) {
        let until = self.now + 2000;
        self.frame();
        while self.app.menu.busy() || !self.app.queue.is_empty() || self.app.queue.waiting() {
            assert!(self.now < until, "UI does not come to rest");
            self.frame();
        }
        self.frame();
    }

    /// Query like `FUN_004BCD70` from a logic.
    pub(crate) fn ask(&mut self, text_id: i32, ok: Option<AppCmd>, cancel: Option<AppCmd>) {
        let app = &mut self.app;
        screens::ask(&mut app.screens, &mut app.queue, text_id, ok, cancel);
    }

    /// Message like `FUN_004BCF10` from a logic.
    pub(crate) fn tell(&mut self, text_id: i32, ok: Option<AppCmd>) {
        let app = &mut self.app;
        screens::tell(&mut app.screens, &mut app.queue, text_id, ok);
    }
}

pub(crate) fn corpus_driver(start_menu: bool, answer: Answer) -> Option<Driver<Corpus>> {
    let c = corpus()?;
    // Test scaffold: `IsMenuLevel 0` like the reference images (with 1, `START` builds the menu level version).
    let config = EngineConfig {
        is_intro: false,
        is_menu_level: false,
        ..EngineConfig::default()
    };
    let opts = MenuOptions {
        start_menu,
        ..MenuOptions::default()
    };
    let app = MenuApp::new(c.vfs(), c.texts(), config, opts).expect("MenuApp");
    Some(Driver::with_app(c, app, answer))
}

pub(crate) fn no_answer() -> Answer {
    Box::new(|_, _| -1)
}

pub(crate) fn open(name: &str, wait: bool) -> AppCmd {
    AppCmd::Open {
        name: name.into(),
        wait,
    }
}

pub(crate) fn close(name: &str, wait: bool) -> AppCmd {
    AppCmd::Close {
        name: name.into(),
        wait,
    }
}

fn frame_state(d: &Driver<impl World>, id: NodeId) -> &crate::widgets::frame::FrameState {
    match &d.app.tree.node(id).kind {
        NodeKind::Frame(s) => s,
        _ => panic!("no frame"),
    }
}

fn root_children(d: &Driver<impl World>) -> Vec<NodeId> {
    let t = &d.app.tree;
    t.node(t.dialog_root()).children.clone()
}

fn desktop_children(d: &Driver<impl World>) -> Vec<NodeId> {
    let t = &d.app.tree;
    t.node(t.desktop()).children.clone()
}

const PAT: &str = "DATA/MENÜ/BACKGROUND_1024X1024_PAT.TGA";

// QUIRK O-90: kept
#[test]
fn start_opens_logo_above_it() {
    // 12.1.1, 12.1.6: `open("start")` opens `logo` with flag 0; LOGO is built after START, so it lies on top.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    assert!(d.app.tree.find("START").is_none());
    let first = d.frame();
    assert!(first.is_empty(), "commands run after drawing");
    let start = d.find("START").expect("START built");
    let logo = d.find("LOGO").expect("LOGO built");
    assert_eq!(root_children(&d), [start, logo]);
    assert_eq!(d.app.menu.open_names(), ["logo", "start"]);
    assert!(d.app.tree.node(d.app.tree.dialog_root()).visible);
    assert!(d.app.menu.busy());
    assert!(frame_state(&d, start).moving && frame_state(&d, logo).moving);
    // `BACKGROUND` in `start.dlg` sends `0x20` when building (`0x004DDEF7`); it runs in the next frame as
    // `MenuLevel(Hide)` to the host.
    assert_eq!(
        d.app.queue.iter().cloned().collect::<Vec<_>>(),
        [AppCmd::HideLevelBackground]
    );
    d.frame();
    let hidden: Vec<UiEffect> = d
        .app
        .take_effects()
        .into_iter()
        .filter(|e| matches!(e, UiEffect::MenuLevel(_)))
        .collect();
    assert_eq!(hidden, [UiEffect::MenuLevel(MenuLevelEffect::Hide)]);
    d.run_until(400);
    assert!(!d.app.menu.busy());
    let report = d.app.report(S1280, d.now);
    let names: Vec<&str> = report.dialogs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["start", "logo"]);
    assert!(report.dialogs.iter().all(|r| !r.animating && !r.modal));
    // Closing START also closes LOGO; after the fly-out and the frame event the root is empty and
    // invisible (name table empty).
    d.push(close("start", true));
    d.frame();
    assert!(d.app.menu.open_names().is_empty());
    d.run_until(800);
    assert!(root_children(&d).is_empty());
    assert!(!d.app.tree.node(d.app.tree.dialog_root()).visible);
    assert_eq!(d.app.tree.name_count(), 0);
}

#[test]
fn one_app_command_per_frame() {
    let mut d = Driver::with_app(
        fixture(),
        MenuApp::from_library(
            DialogLibrary::default(),
            EngineConfig::default(),
            MenuOptions::default(),
        ),
        no_answer(),
    );
    // `0x1F` only acts with a match.
    d.app.set_game(Some(GameInfo::default()));
    for c in [
        AppCmd::ReloadLevel,
        AppCmd::HideLevelBackground,
        AppCmd::GameSpy,
    ] {
        d.push(c);
    }
    d.push(AppCmd::Quit);
    let mut per_frame = Vec::new();
    for _ in 0..5 {
        d.frame();
        per_frame.push(d.app.take_effects());
    }
    assert_eq!(
        per_frame,
        [
            vec![UiEffect::ReloadLevel],
            vec![UiEffect::MenuLevel(MenuLevelEffect::Hide)],
            vec![UiEffect::Unhandled(AppCmd::GameSpy)],
            vec![UiEffect::Quit],
            vec![],
        ]
    );
    // The report holds the effects with the time of their frame.
    let report = d.app.report(S1280, d.now);
    assert_eq!(report.effects[0], (0, UiEffect::ReloadLevel));
    assert_eq!(report.effects[3], (3 * DT, UiEffect::Quit));
}

#[test]
fn triggers_are_refused_while_transitions_run() {
    // 12.1.11, 3.5: during the fly-in the button asks `0x12F` before the sound - no code 8 and no `SFX_KLICK`.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    d.frame();
    // Frames until OPTIONS can be hit, but the stack is still working.
    while d.app.element_center("start", "options", S1280).is_err() {
        d.frame();
        assert!(d.now < 300, "OPTIONS never visible during the fly-in");
    }
    assert!(d.app.menu.busy());
    d.app.take_effects();
    d.click("start", "options");
    d.frame();
    assert_eq!(d.triggered("options"), 0);
    let effects = d.app.take_effects();
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, UiEffect::PlaySfx { event, .. } if event == "SFX_KLICK")),
        "{effects:?}"
    );
    // After the transition the same click triggers, with a click sound.
    d.run_until(500);
    assert!(!d.app.menu.busy());
    d.app.take_effects();
    d.click("start", "options");
    d.frame();
    assert_eq!(d.triggered("options"), 1);
    assert_eq!(
        d.app.take_effects(),
        [UiEffect::PlaySfx {
            event: "SFX_KLICK".into(),
            flags: 0x14,
            category: 1
        }]
    );
}

// QUIRK O-21: kept
#[test]
fn double_click_on_a_button_during_the_flight_triggers_nothing() {
    // Two clicks within `DoubleClickTimeout` (12.8, O-106) on `OPTIONS` during the fly-in - both ask `0x12F` before
    // the sound and are refused (O-21): no code 8, no `SFX_KLICK`, no subdialog.
    let Some(mut d) = corpus_driver(true, Box::new(screens::dispatch)) else {
        return;
    };
    d.frame();
    while d.app.element_center("start", "options", S1280).is_err() {
        d.frame();
        assert!(d.now < 300, "OPTIONS never visible during the fly-in");
    }
    assert!(d.app.menu.busy());
    d.app.take_effects();
    d.click("start", "options");
    d.click("start", "options");
    assert!(
        d.app
            .mouse_in
            .iter()
            .any(|(i, _)| *i == UiInput::DoubleClick(MouseButton::Left)),
        "double click detected"
    );
    d.frame();
    assert!(d.app.menu.busy(), "still in the fly-in");
    let codes: Vec<u32> = d
        .log
        .borrow()
        .iter()
        .filter(|m| m.1 == "options")
        .map(|m| m.2)
        .collect();
    assert!(
        !codes.contains(&code::TRIGGERED) && !codes.contains(&code::DOUBLE_CLICK),
        "{codes:?}"
    );
    let effects = d.app.take_effects();
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, UiEffect::PlaySfx { event, .. } if event == "SFX_KLICK")),
        "{effects:?}"
    );
    d.idle();
    assert_eq!(d.triggered("options"), 0);
    assert_eq!(d.app.menu.open_names(), ["logo", "start"]);
    assert!(d.find("OPTIONS").is_none());
}

// QUIRK O-97: kept
#[test]
fn close_then_open_waits_with_flag_1() {
    // Flag 1: `Open options` waits until START **and** LOGO are done; it is built in the `0x136` of LOGO
    // during the pass, ticked as the last child only in the next frame.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    d.run_until(500);
    let (start, logo) = (d.find("START").unwrap(), d.find("LOGO").unwrap());
    let begin = d.now;
    d.push(close("start", true));
    d.push(open("options", true));
    d.frame();
    d.frame();
    assert!(d.find("OPTIONS").is_none(), "enqueued");
    assert!(d.app.menu.busy());
    let mut built_at = None;
    while built_at.is_none() {
        let t = d.now;
        d.frame();
        if d.find("OPTIONS").is_some() {
            built_at = Some(t);
        } else {
            assert!(t < begin + 400, "OPTIONS never built");
        }
    }
    let t = built_at.unwrap();
    assert!(t >= begin + 300 && t < begin + 300 + DT, "built at {t}");
    let options = d.find("OPTIONS").unwrap();
    // START and LOGO are marked for deletion, still in the tree and drawn; OPTIONS hangs behind them, unticked.
    assert_eq!(root_children(&d), [start, logo, options]);
    let root = d.app.tree.dialog_root();
    assert_eq!(d.app.tree.node(root).pending_deletes, [start, logo]);
    assert!(d.app.tree.node(options).visit.is_none());
    assert!(!frame_state(&d, options).moving || frame_state(&d, options).rem == 300);
    d.frame();
    assert_eq!(root_children(&d), [options]);
    assert!(!d.app.tree.contains(start) && !d.app.tree.contains(logo));
    assert_eq!(d.app.menu.open_names(), ["options"]);
}

#[test]
fn parallel_open_with_flag_0() {
    // Flag 0 starts immediately in parallel - unless jobs are already waiting, then queue at the end.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    d.frame();
    // The second frame executes the `0x20` that the construction of START sent.
    d.frame();
    assert_eq!(d.app.queue.len(), 0);
    assert!(d.app.menu.busy());
    d.push(open("msgbox", false));
    d.push(open("options", true));
    d.push(open("credits", false));
    d.frame();
    assert!(d.find("MSGBOX").is_some(), "flag 0 immediately");
    d.frame();
    assert!(d.find("OPTIONS").is_none(), "flag 1 waits");
    d.frame();
    assert!(d.find("CREDITS").is_none(), "flag 0 behind a waiting one");
    d.run_until(400);
    let (options, credits) = (d.find("OPTIONS").unwrap(), d.find("CREDITS").unwrap());
    assert_eq!(
        root_children(&d)[2..],
        [options, credits],
        "built in job order"
    );
}

// QUIRK O-97: kept
#[test]
fn closing_deletes_and_reopening_rebuilds() {
    // O-97: closing deletes the dialog; the ComboBox selection comes from the logic after reopening.
    let answer: Answer = Box::new(|m, _| {
        if m.element != "resolution" {
            return -1;
        }
        match m.code {
            code::QUERY_ITEMS => {
                let items = ["800 x 600", "1024 x 768", "1280 x 1024"].map(String::from);
                m.params
                    .insert("array".into(), Param::Array(items.to_vec()));
                m.params
                    .insert("indexarray".into(), Param::IndexArray(vec![0, 1, 2]));
                0
            }
            code::QUERY_VALUE => {
                m.params.insert("int".into(), Param::Int(1));
                0
            }
            _ => -1,
        }
    });
    let Some(mut d) = corpus_driver(false, answer) else {
        return;
    };
    d.app.open_by_name("videooptions").unwrap();
    d.run_until(400);
    let combo = d.find("VIDEOOPTIONSRESOLUTION").expect("ComboBox");
    let selection = |d: &Driver<Corpus>, id| match &d.app.tree.node(id).kind {
        NodeKind::Button(s) => s.selection,
        _ => unreachable!(),
    };
    assert_eq!(selection(&d, combo), 1);
    // A selection that the logic does not know (only in the tree).
    if let NodeKind::Button(s) = &mut d.app.tree.node_mut(combo).kind {
        s.selection = 2;
    }
    d.push(close("videooptions", false));
    d.run_until(800);
    assert!(!d.app.tree.contains(combo));
    assert!(d.find("VIDEOOPTIONS").is_none());
    d.push(open("videooptions", true));
    d.run_until(1200);
    let again = d.find("VIDEOOPTIONSRESOLUTION").expect("rebuilt");
    assert_ne!(again, combo);
    assert_eq!(
        selection(&d, again),
        1,
        "from the logic, not from the old tree"
    );
}

// QUIRK O-96: kept
#[test]
fn reopening_an_open_dialog_restarts_the_flight() {
    // O-96: no rebuild, no `OnAttach`; fly-in from EXTENT and fade-in from alpha 0 anew, name twice.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    d.run_until(1000);
    let start = d.find("START").unwrap();
    assert_eq!(d.app.tree.abs_rect(start)[0], 40);
    let report = d.app.report(S1280, d.now);
    assert_eq!(report.images, [(PAT.to_string(), 255)]);
    d.app.take_effects();
    d.log.borrow_mut().clear();
    d.push(open("start", true));
    let t = d.now;
    d.frame();
    assert_eq!(d.find("START"), Some(start), "no rebuild");
    assert!(d.log.borrow().is_empty(), "no OnAttach (no queries)");
    assert!(
        !d.app
            .take_effects()
            .iter()
            .any(|e| matches!(e, UiEffect::MenuMusic { .. })),
        "no music"
    );
    assert_eq!(d.app.menu.open_names(), ["logo", "start", "logo", "start"]);
    assert!(frame_state(&d, start).moving);
    assert_eq!(d.app.report(S1280, t).images, [(PAT.to_string(), 0)]);
    // First step in the next frame: the frame jumps close to EXTENT (-440).
    d.frame();
    let x = d.app.tree.abs_rect(start)[0];
    assert!((-440..-400).contains(&x), "x = {x}");
}

/// Test dialog with a ComboBox (list can be expanded) and a button `CANCEL` with `CHECKESCAPE`.
fn combo_dialog(name: &str, modal: bool) -> String {
    let flags = if modal { "MENUFRAME_MODAL" } else { "" };
    format!(
        "FRAME {name}\n FLAGS {flags} /FLAGS\n EXTENT 100 100 600 500\n\
         COMBOBOX RES FLAGS MENUCOMBOBOX_TEXT /FLAGS BUTTONEXTENT 50 10 250 50 \
         LISTBOXEXTENT 50 60 250 200 CURSELECTION -1 /COMBOBOX\n\
         BUTTON CANCEL FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 300 10 400 50 /BUTTON\n\
         /FRAME\n"
    )
}

/// Logic of the test dialogs: three entries; `CANCEL` closes its own dialog (flag 1).
fn combo_logic() -> Answer {
    Box::new(|m, env| match (m.element.as_str(), m.code) {
        ("res", code::QUERY_ITEMS) => {
            let items = ["A", "B", "C"].map(String::from).to_vec();
            m.params.insert("array".into(), Param::Array(items));
            0
        }
        ("cancel", code::TRIGGERED) => {
            env.queue.push(close(&m.dialog, true));
            1
        }
        _ => -1,
    })
}

fn combo_driver() -> Driver<Fixture> {
    let modal = combo_dialog("MODAL", true);
    let plain = combo_dialog("PLAIN", false);
    let library = DialogLibrary::from_sources(&[("modal", &modal), ("plain", &plain)]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    Driver::with_app(fixture(), app, combo_logic())
}

/// Opens the dialog, expands the ComboBox and returns (popup, blocker, list).
fn open_combo_list(d: &mut Driver<Fixture>, name: &str) -> (NodeId, NodeId, NodeId) {
    d.app.open_by_name(name).unwrap();
    d.frame();
    d.frame();
    d.click(name, "res");
    d.frame();
    let combo = d.find(&format!("{}RES", upper_word(name))).unwrap();
    let NodeKind::Button(s) = &d.app.tree.node(combo).kind else {
        unreachable!()
    };
    assert!(s.list_open, "list open");
    let popup = s.popup.unwrap();
    let NodeKind::ComboPopup(p) = &d.app.tree.node(popup).kind else {
        unreachable!()
    };
    (popup, p.blocker.unwrap(), p.list_box.unwrap())
}

// QUIRK O-109: deviation
#[test]
fn closing_with_an_open_combo_list_removes_popup_and_blocker() {
    // Popup, list and blocker fall with the dialog in the same deletion pass - non-modal (the frame falls while
    // descending, the blocker afterwards out of the list of the desktop) and modal (the blocker of the popup
    // lands in the list that is just being processed; lost in the original, O-109).
    for name in ["plain", "modal"] {
        let mut d = combo_driver();
        let (popup, blocker, list) = open_combo_list(&mut d, name);
        let frame = d.find(name).unwrap();
        d.push(close(name, false));
        // Without MOVE, `0x136` comes synchronously: marked in the command, deleted in the next frame event.
        d.frame();
        assert!(d.app.tree.contains(frame), "{name}: only marked at first");
        d.frame();
        for id in [frame, popup, blocker, list] {
            assert!(!d.app.tree.contains(id), "{name}: {id:?} orphaned");
        }
        assert_eq!(desktop_children(&d), [d.app.tree.dialog_root()], "{name}");
        assert!(root_children(&d).is_empty(), "{name}");
        assert!(
            d.app
                .tree
                .node(d.app.tree.desktop())
                .pending_deletes
                .is_empty()
        );
        // A click afterwards hits nothing any more (no swallowing blocker) and does not panic.
        let now = d.now;
        d.app.input(UiInput::Down(MouseButton::Left), S1280, now);
        d.app.input(UiInput::Up(MouseButton::Left), S1280, now);
        d.frame();
        assert!(!d.app.menu.busy());
    }
}

// QUIRK O-109: deviation
#[test]
fn escape_with_an_open_combo_list_triggers_checkescape() {
    // The popup does not take ESC (slot 19 = base `0x0045A380`), the blocker does not lock the keyboard -
    // ESC triggers `CANCEL` as without a list; the logic closes, nothing stays orphaned.
    for name in ["plain", "modal"] {
        let mut d = combo_driver();
        let (popup, blocker, list) = open_combo_list(&mut d, name);
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: vec![0x1B],
                ch: Some(0x1B),
            },
            S1280,
            now,
        );
        d.frame();
        assert_eq!(d.triggered("cancel"), 1, "{name}");
        assert!(
            d.app.tree.contains(popup),
            "{name}: list stays open until closing"
        );
        d.frame();
        d.frame();
        for id in [popup, blocker, list] {
            assert!(!d.app.tree.contains(id), "{name}: {id:?} orphaned");
        }
        assert_eq!(desktop_children(&d), [d.app.tree.dialog_root()], "{name}");
        assert!(d.find(name).is_none());
    }
}

#[test]
fn wait_dialogs_ends_when_the_root_is_empty() {
    // `0x15` waits until the dialog root has no children (12.1.10); the blocker of the modal MSGBOX does not count.
    let Some(mut d) = corpus_driver(true, no_answer()) else {
        return;
    };
    d.push(open("msgbox", false));
    d.run_until(500);
    assert!(d.find("MSGBOX").is_some());
    d.push(close("start", false));
    d.push(AppCmd::WaitDialogs);
    d.push(open("options", true));
    d.frame();
    d.frame();
    assert!(d.app.queue.waiting());
    while !root_children(&d).is_empty() {
        assert!(d.find("OPTIONS").is_none());
        d.frame();
        assert!(d.now < 1200, "START/LOGO never deleted");
    }
    // In the frame in which the root became empty, only the waiting state ends; the command comes in the next one.
    assert!(!d.app.queue.waiting());
    assert!(d.find("OPTIONS").is_none());
    d.frame();
    assert!(d.find("OPTIONS").is_some());
    assert!(d.find("MSGBOX").is_some());
}

#[test]
fn music_effect_on_attach_with_loop_flag() {
    // `MenuFrame.OnAttach` `0x00461CA1`: `0x160 {path as read, !DONOTLOOPMUSIC}`; the supplier of the
    // music length runs before the construction, the credits roll with this length.
    let Some(c) = corpus() else {
        return;
    };
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let a = asked.clone();
    let opts = MenuOptions {
        music_length: Some(Box::new(move |path| {
            a.borrow_mut().push(path.to_string());
            (path == "DATA/MUSIC/CREDITS.OGG").then_some(300_000)
        })),
        ..MenuOptions::default()
    };
    let app = MenuApp::new(&c.vfs, &c.texts, EngineConfig::default(), opts).unwrap();
    let mut d = Driver::with_app(c, app, no_answer());
    let music = |d: &mut Driver<Corpus>| -> Vec<UiEffect> {
        d.app
            .take_effects()
            .into_iter()
            .filter(|e| matches!(e, UiEffect::MenuMusic { .. }))
            .collect()
    };
    let cases = [
        ("start", "DATA/MUSIC/TITLE.OGG", true),
        ("credits", "DATA/MUSIC/CREDITS.OGG", true),
        ("gameend_victory", "DATA/MUSIC/WIN.OGG", false),
    ];
    for (name, path, looping) in cases {
        d.app.open_by_name(name).unwrap();
        let until = d.now + 400;
        d.run_until(until);
        assert_eq!(
            music(&mut d),
            [UiEffect::MenuMusic {
                path: path.into(),
                looping
            }],
            "{name}"
        );
    }
    assert_eq!(
        *asked.borrow(),
        [
            "DATA/MUSIC/TITLE.OGG",
            "DATA/MUSIC/CREDITS.OGG",
            "DATA/MUSIC/WIN.OGG"
        ]
    );
    let label = d.find("CREDITSSCROLLTEXT").unwrap();
    let NodeKind::Label(s) = &d.app.tree.node(label).kind else {
        unreachable!()
    };
    let roll = s.credits.as_ref().unwrap();
    assert_eq!(roll.speed, roll.end / 300_000.0);
}

/// Short test video: three frames at 25/s (end after 120 ms), 356 rows.
const SHORT: VideoInfo = VideoInfo {
    width: 640,
    height: 356,
    frames: 3,
    fps_num: 25,
    fps_den: 1,
};

/// Long test video: 145 frames (5,800 ms).
const LONG: VideoInfo = VideoInfo {
    frames: 145,
    ..SHORT
};

const START_DLG: &str = "FRAME START\n FLAGS  /FLAGS\n EXTENT 100 100 600 500\n/FRAME\n";

/// Test dialog with a button `CANCEL` with `CHECKESCAPE` (without `CHECKENTER`).
const ESCAPE_DLG: &str = "FRAME TEST\n FLAGS  /FLAGS\n EXTENT 100 100 600 500\n\
     BUTTON CANCEL FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 300 10 400 50 /BUTTON\n\
     /FRAME\n";

/// `MenuApp` on the fixture with test dialogs; `EngineConfig::default()` has `IsIntro`.
fn video_driver(sources: &[(&str, &str)], start_menu: bool) -> Driver<Fixture> {
    let library = DialogLibrary::from_sources(sources);
    let opts = MenuOptions {
        start_menu,
        ..MenuOptions::default()
    };
    let app = MenuApp::from_library(library, EngineConfig::default(), opts);
    Driver::with_app(fixture(), app, no_answer())
}

/// One frame with a host substitute (instead of `opennml-client::MenuHost`): after the frame it answers every
/// `PlayVideo` whose file `open` knows with `video_started` at the time of the frame (frame 0 under
/// [`VIDEO_TEXTURE`]). Returns drawing commands and effects of the frame together with those of `video_started`.
fn host_frame<W: World>(
    d: &mut Driver<W>,
    open: impl Fn(&str) -> Option<VideoInfo>,
) -> (Vec<DrawCmd>, Vec<UiEffect>) {
    let now = d.now;
    let out = d.frame();
    let mut fx = d.app.take_effects();
    for e in fx.clone() {
        if let UiEffect::PlayVideo { path, .. } = e
            && let Some(info) = open(&path)
        {
            let tex = d
                .world
                .ctx(S1280)
                .assets
                .insert_image(VIDEO_TEXTURE, image::RgbaImage::new(4, 4));
            d.app.video_started(&path, info, tex, now);
            fx.extend(d.app.take_effects());
        }
    }
    (out, fx)
}

fn always(info: VideoInfo) -> impl Fn(&str) -> Option<VideoInfo> {
    move |_| Some(info)
}

/// Quad of a video with 356 rows at 1280 x 1024 (deviation).
fn video_quad(texture: TextureId) -> DrawCmd {
    DrawCmd::Quad {
        texture: Some(texture),
        dst: video_rect(S1280, 356),
        uv: video_uv(video_rect(S1280, 356)),
        color: 0xFFFF_FFFF,
        filter: Filter::Linear,
        blend: Blend::Alpha,
    }
}

fn play(path: &str) -> UiEffect {
    UiEffect::PlayVideo {
        path: path.into(),
        abortable: true,
    }
}

fn stop(path: &str) -> UiEffect {
    UiEffect::StopVideo { path: path.into() }
}

/// `0x1D`, `0x1A` and the end of the probe video directly (without the closing of the menus by
/// [`MenuApp::play_video_by_name`]): for tests with an open dialog during the video. `AfterProbeVideo` instead of
/// `0x28` so that no campaign branch touches the open dialogs.
fn push_video_commands(d: &mut Driver<Fixture>, path: &str) {
    d.push(AppCmd::PlayVideo(path.into()));
    d.push(AppCmd::WaitVideo);
    d.push(AppCmd::AfterProbeVideo);
}

/// Logo of the main menu (`open("start")` opens it too, 12.1.1).
const LOGO_DLG: &str = "FRAME LOGO
 FLAGS  /FLAGS
 EXTENT 0 0 100 50
/FRAME
";

/// Main menu `start` (with `logo`) and above it the test dialog with `CHECKESCAPE`, all transitions done.
fn stacked_menus() -> Driver<Fixture> {
    let mut d = video_driver(
        &[
            ("start", START_DLG),
            ("logo", LOGO_DLG),
            ("test", ESCAPE_DLG),
        ],
        false,
    );
    d.app.open_by_name("start").unwrap();
    d.app.open_by_name("test").unwrap();
    for _ in 0..4 {
        d.frame();
    }
    assert!(
        ["START", "LOGO", "TEST"]
            .iter()
            .all(|k| d.find(k).is_some())
    );
    assert!(!d.app.menu.busy() && d.app.queue.is_empty());
    assert_eq!(d.app.menu.open_names(), ["logo", "start", "test"]);
    d
}

fn has_video_quad(d: &Driver<Fixture>, out: &[DrawCmd]) -> bool {
    out.iter().any(|c| {
        matches!(c, DrawCmd::Quad { texture: Some(t), .. } if d.world.assets.path(*t) == VIDEO_TEXTURE)
    })
}

#[test]
fn start_sequence_plays_three_videos_then_start() {
    // Program start with `IsIntro` (`0x004C96C1`...`0x004C976B`): pointer off, `0x1B(0)`, `0x1A`, `0x1B(1)`, `0x1A`,
    // `0x1B(2)`, `0x1A`, `0x24`, then `start`. `0x1A` waits until the menu root has no video any more; the end
    // costs one frame, the next command comes in the following one. A video ends in the first frame from 120 ms on.
    let mut d = video_driver(&[("start", START_DLG)], true);
    assert!(d.app.config().is_intro);
    assert!(d.app.pointer_hidden(), "pointer off before the first frame");
    let mut fx_log = Vec::new();
    let mut quads = 0;
    while d.find("START").is_none() {
        assert!(d.now < 2_000, "startup sequence hangs");
        let now = d.now;
        let before = d.app.video().cloned();
        let (out, fx) = host_frame(&mut d, always(SHORT));
        // If a video runs in this frame, the desktop draws it instead of its images.
        match before.filter(|v| v.frame_at(now).is_some()) {
            Some(v) => {
                assert_eq!(out.first(), Some(&video_quad(v.texture)), "{now} ms");
                quads += 1;
            }
            None => assert!(!has_video_quad(&d, &out), "{now} ms"),
        }
        fx_log.extend(fx.into_iter().map(|e| (now, e)));
        assert_eq!(d.app.pointer_hidden(), now < 432, "pointer at {now} ms");
    }
    let [cdv, related, intro] = INTRO_VIDEOS;
    assert_eq!(
        fx_log,
        [
            (0, play(cdv)),
            (128, stop(cdv)),
            (144, play(related)),
            (272, stop(related)),
            (288, play(intro)),
            (416, stop(intro)),
        ]
    );
    assert_eq!(
        quads,
        3 * 7,
        "per video the frames 16 ... 112 ms after the start"
    );
    assert_eq!(d.now, 464, "start built in the frame at 448 ms");
    assert!(d.app.video().is_none());

    // Without `video_started` (the host opened nothing) the sequence runs through without a pause: three frames each.
    let mut d = video_driver(&[("start", START_DLG)], true);
    let mut played = Vec::new();
    for i in 0..11 {
        let (out, fx) = host_frame(&mut d, |_| None);
        assert!(!has_video_quad(&d, &out), "frame {i}");
        played.extend(fx.into_iter().map(|e| (i, e)));
        assert_eq!(d.app.pointer_hidden(), i < 9, "frame {i}");
        assert_eq!(d.find("START").is_some(), i >= 10, "frame {i}");
    }
    assert_eq!(
        played,
        [(0, play(cdv)), (3, play(related)), (6, play(intro))]
    );
}

/// Quirk E-295: The network start via the command line skips the startup sequence like a direct start
/// (`MenuOptions::skip_intro`), also with `IsIntro`: no video, the pointer stays on, `start` in the first frame. The
/// setting itself stays unchanged (it is not remembered as `IsIntro 0`).
// QUIRK E-295: kept
#[test]
fn skip_intro_opens_start_without_videos() {
    let library = DialogLibrary::from_sources(&[("start", START_DLG)]);
    let opts = MenuOptions {
        start_menu: true,
        skip_intro: true,
        ..MenuOptions::default()
    };
    let app = MenuApp::from_library(library, EngineConfig::default(), opts);
    let mut d = Driver::with_app(fixture(), app, no_answer());
    assert!(d.app.config().is_intro, "IsIntro stays set");
    assert!(!d.app.pointer_hidden(), "pointer on");
    let (out, fx) = host_frame(&mut d, always(SHORT));
    assert!(!has_video_quad(&d, &out));
    assert!(
        !fx.iter().any(|e| matches!(e, UiEffect::PlayVideo { .. })),
        "no startup video: {fx:?}"
    );
    assert!(d.find("START").is_some(), "start in the first frame");
    assert!(d.app.video().is_none());
}

#[test]
fn video_index_above_two_is_ignored() {
    // `0x1B(n)` knows only 0...2; another value has no effect (warning).
    let mut d = video_driver(&[], false);
    d.push(AppCmd::Video(3));
    d.push(AppCmd::Video(255));
    for _ in 0..3 {
        let (_, fx) = host_frame(&mut d, always(SHORT));
        assert!(fx.is_empty(), "{fx:?}");
    }
    assert!(d.app.video().is_none());
    assert!(d.app.queue.is_empty());
}

// QUIRK O-68: kept
#[test]
fn any_key_aborts_including_escape_and_return() {
    // O-68: without a receiver every key goes as event `0xF` to the menu root, whose `0x0045CAB0` does not check the
    // code - also ESC and RETURN (no button takes the broadcast) and Shift alone (the host sends a key event without
    // a character for it, `MenuHost::on_key`). The video ends in the frame of the key, the sequence continues.
    let keys: [(&str, Vec<u8>, Option<u16>); 4] = [
        ("A", vec![0x41], Some(0x41)),
        ("ESCAPE", vec![0x1B], Some(0x1B)),
        ("RETURN", vec![0x0D], Some(0x0D)),
        ("SHIFT", vec![0x10], None),
    ];
    for (name, held, ch) in keys {
        let mut d = video_driver(&[], false);
        // A key in the frame of the command has no effect yet: opening happens only after the frame.
        d.app.play_video_by_name("t").unwrap();
        d.key(0x41);
        let (_, fx) = host_frame(&mut d, always(LONG));
        assert_eq!(fx, [play("data/videos/t.bik")], "{name}");
        host_frame(&mut d, always(LONG));
        host_frame(&mut d, always(LONG));
        assert_eq!(d.app.video_frame(d.now), Some(1), "{name}");
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: held.clone(),
                ch,
            },
            S1280,
            now,
        );
        let (out, fx) = host_frame(&mut d, always(LONG));
        assert_eq!(fx, [stop("data/videos/t.bik")], "{name}");
        assert!(!has_video_quad(&d, &out), "{name}: aborted before drawing");
        assert!(d.app.video().is_none());
        // The waiting ended in the same frame, the end of the probe video (`AfterProbeVideo`) comes in the next one.
        let (_, fx) = host_frame(&mut d, always(LONG));
        assert_eq!(fx, [UiEffect::RestartMusic], "{name}");
        assert!(!d.app.pointer_hidden());
        assert!(d.app.queue.is_empty() && !d.app.queue.waiting());
    }
    // Key in the last frame: exactly one `StopVideo`.
    let mut d = video_driver(&[], false);
    d.app.play_video_by_name("t").unwrap();
    let mut stops = 0;
    while d.now <= 300 {
        if d.now == 112 {
            assert_eq!(d.app.video_frame(112), Some(2), "last frame");
            d.key(0x41);
        }
        let (_, fx) = host_frame(&mut d, always(SHORT));
        stops += fx
            .iter()
            .filter(|e| matches!(e, UiEffect::StopVideo { .. }))
            .count();
        if d.now == 128 {
            assert!(d.app.video().is_none(), "abort in the frame at 112 ms");
        }
    }
    assert_eq!(stops, 1);
}

// QUIRK O-68: kept
#[test]
fn mouse_never_aborts_a_video() {
    // The click slots of the menu root return 0 (`0x004527C0`): movement, clicks, double clicks and wheel over the
    // whole video do not abort; it ends only in the first frame from 3 * 40 ms on.
    let mut d = video_driver(&[], false);
    d.app.play_video_by_name("t").unwrap();
    let mut stops = Vec::new();
    let mut i = 0;
    while d.now <= 300 {
        let now = d.now;
        let (x, y) = ((i * 97) % 1280, (i * 61) % 1024);
        for input in [
            UiInput::Move { x, y },
            UiInput::Down(MouseButton::Left),
            UiInput::Up(MouseButton::Left),
            UiInput::Down(MouseButton::Right),
            UiInput::Up(MouseButton::Right),
            UiInput::WheelUp,
            UiInput::WheelDown,
        ] {
            d.app.input(input, S1280, now);
        }
        let (_, fx) = host_frame(&mut d, always(SHORT));
        for e in fx {
            if matches!(e, UiEffect::StopVideo { .. }) {
                stops.push(now);
            }
        }
        if (16..120).contains(&now) {
            assert!(d.app.video().is_some(), "{now} ms");
        }
        i += 1;
    }
    assert_eq!(stops, [128]);
}

// QUIRK O-68: kept
#[test]
fn escape_is_taken_by_a_checkescape_button_first() {
    // re/video, section "Abort": ESC first goes through the tree as broadcast `0x17E`; if a button with
    // `CHECKESCAPE` takes it, the video continues. RETURN is taken by nobody here (no `CHECKENTER`): it aborts. Open
    // dialogs draw over the video. `ui_video` closes the menus beforehand (`videostart`), so the test enqueues
    // `0x1D` directly.
    let mut d = video_driver(&[("test", ESCAPE_DLG)], false);
    d.app.open_by_name("test").unwrap();
    d.frame();
    d.frame();
    assert!(d.find("TEST").is_some() && !d.app.menu.busy());
    push_video_commands(&mut d, "data/videos/t.bik");
    host_frame(&mut d, always(LONG));
    host_frame(&mut d, always(LONG));
    let tex = d.app.video().expect("video runs").texture;
    let (out, _) = host_frame(&mut d, always(LONG));
    assert_eq!(out.first(), Some(&video_quad(tex)));
    assert!(out.len() > 1, "the dialog draws over the video");
    d.key(0x1B);
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(d.triggered("cancel"), 1);
    assert!(fx.iter().all(|e| !matches!(e, UiEffect::StopVideo { .. })));
    assert!(d.app.video().is_some(), "ESC taken by the button");
    d.key(0x0D);
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [stop("data/videos/t.bik")]);
    assert_eq!(d.triggered("cancel"), 1);
}

// QUIRK O-68: kept
#[test]
fn focused_edit_field_takes_the_key_and_the_video_runs_on() {
    // The video start sets only the focus of the menu root itself (`0x0045D6A6` -> `FUN_00459400`, without clearing the
    // rest of the tree): an EditField focused beforehand keeps it, takes the key (focus chain `0x0045B523`) and
    // the video continues. The reimplementation leaves the focus unchanged - equivalent.
    const EDIT_DLG: &str = "FRAME TEST\n FLAGS  /FLAGS\n EXTENT 100 100 600 500\n\
         EDITFIELD E EXTENT 100 100 300 160 /EDITFIELD\n/FRAME\n";
    let mut d = video_driver(&[("test", EDIT_DLG)], false);
    d.app.open_by_name("test").unwrap();
    d.frame();
    d.frame();
    let edit = d.find("TESTE").expect("EditField built");
    assert!(d.app.tree.node(edit).focused, "OnAttach sets the focus");
    push_video_commands(&mut d, "data/videos/t.bik");
    host_frame(&mut d, always(LONG));
    host_frame(&mut d, always(LONG));
    d.key(0x41);
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert!(fx.iter().all(|e| !matches!(e, UiEffect::StopVideo { .. })));
    assert!(d.app.video().is_some(), "the EditField took the key");
    assert!(
        d.log
            .borrow()
            .iter()
            .any(|m| m.1 == "e" && m.2 == code::EDIT_KEY),
        "code 0xB to the EditField"
    );
}

#[test]
fn starting_a_video_stops_the_running_one() {
    // `FUN_0045D560` first stops a running video (`0x0045D584`); the new one starts at frame 0, the
    // queue does not hang.
    let mut d = video_driver(&[], false);
    d.push(AppCmd::PlayVideo("data/videos/a.bik".into()));
    d.push(AppCmd::PlayVideo("data/videos/b.bik".into()));
    d.push(AppCmd::WaitVideo);
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [play("data/videos/a.bik")]);
    let t1 = d.now;
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [stop("data/videos/a.bik"), play("data/videos/b.bik")]);
    let v = d.app.video().unwrap().clone();
    assert_eq!((v.path.as_str(), v.started_ms), ("data/videos/b.bik", t1));
    assert_eq!(d.app.video_frame(t1), Some(0));
    // If the host reports a video while one is running, the running one ends likewise.
    let now = d.now;
    d.app
        .video_started("data/videos/c.bik", LONG, v.texture, now);
    assert_eq!(d.app.take_effects(), [stop("data/videos/b.bik")]);
    assert_eq!(d.app.video_frame(now), Some(0));
    assert_eq!(
        d.app.report(S1280, now).effects.last(),
        Some(&(now, stop("data/videos/b.bik")))
    );
    host_frame(&mut d, always(LONG));
    assert!(d.app.queue.waiting(), "`0x1A` waits for c");
    d.key(0x41);
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [stop("data/videos/c.bik")]);
    host_frame(&mut d, always(LONG));
    assert!(d.app.queue.is_empty() && !d.app.queue.waiting());
}

#[test]
fn host_can_end_a_video_like_a_key() {
    // An undecodable frame ends the video via the host like an abort by key; the
    // sequence (`0x1A`, `AfterProbeVideo`) continues.
    let mut d = video_driver(&[], false);
    d.app.end_video(0);
    assert!(d.app.take_effects().is_empty(), "nothing without a video");
    d.app.play_video_by_name("t").unwrap();
    let (_, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [play("data/videos/t.bik")]);
    host_frame(&mut d, always(LONG));
    let now = d.now;
    d.app.end_video(now);
    assert!(d.app.video().is_none());
    assert_eq!(d.app.take_effects(), [stop("data/videos/t.bik")]);
    assert_eq!(
        d.app.report(S1280, now).effects.last(),
        Some(&(now, stop("data/videos/t.bik")))
    );
    let mut later = Vec::new();
    for _ in 0..3 {
        later.extend(host_frame(&mut d, always(LONG)).1);
    }
    assert_eq!(later, [UiEffect::RestartMusic]);
    assert!(d.app.queue.is_empty() && !d.app.queue.waiting());
}

#[test]
fn probe_video_restores_pointer_and_music() {
    // `0x1D(path)` switches the pointer off (`FUN_004CD6A0(0)`) and plays; afterwards like `0x28` without
    // campaign (`AfterProbeVideo`): pointer on, menu music anew (`MenuMusic_Restart`). No mode switch. Without
    // open menus, the closing and reopening of `ui_video` is omitted.
    let mut d = video_driver(&[], false);
    assert!(
        !d.app.pointer_hidden(),
        "without a startup sequence the pointer stays on"
    );
    d.app.play_video_by_name("eng").unwrap();
    let path = "data/videos/eng.bik";
    assert_eq!(
        d.app.queue.iter().cloned().collect::<Vec<_>>(),
        [
            AppCmd::PlayVideo(path.into()),
            AppCmd::WaitVideo,
            AppCmd::AfterProbeVideo
        ]
    );
    let mut log = Vec::new();
    while d.now <= 300 {
        let now = d.now;
        let (_, fx) = host_frame(&mut d, always(SHORT));
        log.extend(fx.into_iter().map(|e| (now, e)));
        assert_eq!(d.app.pointer_hidden(), now < 144, "pointer at {now} ms");
        if now == 48 {
            let report = d.app.report(S1280, now);
            assert_eq!(
                report.video,
                Some(ReportVideo {
                    path: path.into(),
                    frame: 1,
                    frames: 3,
                    abortable: true
                })
            );
            assert!(report.pointer_hidden);
        }
    }
    assert_eq!(
        log,
        [
            (0, play(path)),
            (128, stop(path)),
            (144, UiEffect::RestartMusic)
        ]
    );
    assert_eq!(d.app.report(S1280, d.now).video, None);
}

#[test]
fn ui_video_closes_the_menus_and_reopens_them_afterwards() {
    // Modeled on `videostart` (`FUN_004DA8A0` `0x004DAA14`...`0x004DAB18`): the open menus closed (code 4), `0x15`,
    // `0x1D`, `0x1A`, then `AfterProbeVideo` (the part of `0x28` without a campaign) and the menus open again
    // (code 5; in the original `selectcampaign`). During the video no dialog is open and the desktop draws only
    // the video; afterwards the menus are back in the same order (`logo` opens `start` itself), the pointer
    // is on and the menu music anew.
    let mut d = stacked_menus();
    d.app.play_video_by_name("eng").unwrap();
    let path = "data/videos/eng.bik";
    assert_eq!(
        d.app.queue.iter().cloned().collect::<Vec<_>>(),
        [
            AppCmd::CloseAll,
            AppCmd::WaitDialogs,
            AppCmd::PlayVideo(path.into()),
            AppCmd::WaitVideo,
            AppCmd::AfterProbeVideo,
            open("start", true),
            open("test", true),
        ]
    );
    let mut fx_log = Vec::new();
    let mut shown = 0;
    while d.now <= 600 {
        let now = d.now;
        let before = d.app.video().cloned();
        let (out, fx) = host_frame(&mut d, always(SHORT));
        fx_log.extend(fx);
        if d.app.video().is_some() {
            let root = d.app.tree.dialog_root();
            assert!(d.app.tree.node(root).children.is_empty(), "{now} ms");
            assert!(d.app.menu.open_names().is_empty(), "{now} ms");
            assert!(d.app.pointer_hidden(), "{now} ms");
        }
        if let Some(v) = before.filter(|v| v.frame_at(now).is_some()) {
            assert_eq!(out, [video_quad(v.texture)], "only the video at {now} ms");
            shown += 1;
        }
    }
    assert_eq!(shown, 7, "frames 16 ... 112 ms after the start");
    assert_eq!(fx_log, [play(path), stop(path), UiEffect::RestartMusic]);
    assert_eq!(d.app.menu.open_names(), ["logo", "start", "test"]);
    assert!(
        ["START", "LOGO", "TEST"]
            .iter()
            .all(|k| d.find(k).is_some())
    );
    assert!(!d.app.pointer_hidden());
    assert!(d.app.queue.is_empty() && !d.app.queue.waiting());
}

#[test]
fn ui_video_from_stacked_menus_aborts_by_key() {
    // ESC aborts: the button with `CHECKESCAPE` is closed together with its dialog and does not take the broadcast.
    // Afterwards the menus come back as after the end.
    let mut d = stacked_menus();
    d.app.play_video_by_name("t").unwrap();
    while d.app.video_frame(d.now) != Some(1) {
        assert!(d.now < 300, "video does not start");
        host_frame(&mut d, always(LONG));
    }
    d.key(0x1B);
    let (out, fx) = host_frame(&mut d, always(LONG));
    assert_eq!(fx, [stop("data/videos/t.bik")]);
    assert!(!has_video_quad(&d, &out));
    assert_eq!(d.triggered("cancel"), 0);
    let mut later = Vec::new();
    for _ in 0..6 {
        later.extend(host_frame(&mut d, always(LONG)).1);
    }
    assert_eq!(later, [UiEffect::RestartMusic]);
    assert_eq!(d.app.menu.open_names(), ["logo", "start", "test"]);
    assert!(d.find("TEST").is_some() && !d.app.pointer_hidden());
    assert!(d.app.queue.is_empty());
}

/// Main menu with fly-in (`MOVE`, 300 ms) as in the shipped data.
const FLYING_START: &str = "FRAME START\n FLAGS MENUFRAME_MOVE /FLAGS\n EXTENT -500 100 0 500\n\
     ZIELPOSITION 100 100\n DURATION 300\n/FRAME\n";

/// `ui_video` must be rejected and enqueues nothing.
fn refuse_video(d: &mut Driver<Fixture>) {
    let queued = d.app.queue.len();
    let err = d.app.play_video_by_name("eng").unwrap_err();
    assert!(err.contains("busy"), "{} ms: {err}", d.now);
    assert_eq!(d.app.queue.len(), queued, "{} ms: nothing enqueued", d.now);
}

/// The UI is at rest: no transition, no commands, no waiting, no video.
fn ui_idle(d: &Driver<Fixture>) -> bool {
    !d.app.menu.busy()
        && d.app.queue.is_empty()
        && !d.app.queue.waiting()
        && d.app.video().is_none()
}

#[test]
fn ui_video_is_refused_while_the_menus_are_busy() {
    // `videostart` is a button in the original and triggers only with menus at rest (`0x12F`, O-21). `ui_video`
    // therefore rejects as long as the startup sequence runs, a menu flies in, commands wait or a video runs -
    // otherwise `CloseAll` could skip a still waiting dialog and `WaitDialogs` would never end.
    let mut d = video_driver(&[("start", FLYING_START)], true);
    while d.find("START").is_none() {
        assert!(d.now < 2_000, "startup sequence hangs");
        refuse_video(&mut d);
        host_frame(&mut d, always(SHORT));
    }
    assert!(d.app.menu.busy(), "start flies in");
    while !ui_idle(&d) {
        assert!(d.now < 3_000, "fly-in does not end");
        refuse_video(&mut d);
        host_frame(&mut d, always(SHORT));
    }
    d.app.play_video_by_name("eng").unwrap();
    // First `start` flies out, then the video runs; a second `ui_video` is rejected.
    let mut fx_log = Vec::new();
    while d.app.video().is_none() {
        assert!(d.now < 4_000, "video does not start");
        fx_log.extend(host_frame(&mut d, always(SHORT)).1);
    }
    assert!(d.find("START").is_none() && d.app.menu.open_names().is_empty());
    refuse_video(&mut d);
    // Afterwards `start` flies in again, and the UI rests without a hang.
    while !ui_idle(&d) {
        assert!(d.now < 6_000, "queue hangs");
        fx_log.extend(host_frame(&mut d, always(SHORT)).1);
    }
    let path = "data/videos/eng.bik";
    assert_eq!(fx_log, [play(path), stop(path), UiEffect::RestartMusic]);
    assert!(d.find("START").is_some());
    assert_eq!(d.app.menu.open_names(), ["start"]);
    assert!(!d.app.pointer_hidden());
    d.app.play_video_by_name("eng").unwrap();
}

#[test]
fn play_video_by_name_rejects_paths() {
    let mut d = video_driver(&[], false);
    for name in ["../x", "", "a/b", "a\\b", "x.bik", "a b", "ä"] {
        let err = d.app.play_video_by_name(name).unwrap_err();
        assert!(err.contains("video"), "{name}: {err}");
    }
    assert!(d.app.queue.is_empty());
    d.app.play_video_by_name("Spa-2_x").unwrap();
    assert_eq!(
        d.app.queue.iter().next(),
        Some(&AppCmd::PlayVideo("data/videos/Spa-2_x.bik".into()))
    );
}

#[test]
fn modal_dialog_lies_above_all_non_modal() {
    // 12.1.6: the blocker of the modal dialog hangs on the desktop behind the dialog root - above everything that is
    // opened at the root afterwards.
    let Some(mut d) = corpus_driver(false, no_answer()) else {
        return;
    };
    d.push(open("msgbox", true));
    d.push(open("start", true));
    d.run_until(500);
    let msgbox = d.find("MSGBOX").unwrap();
    let blocker = d.app.tree.node(msgbox).parent.unwrap();
    assert_eq!(desktop_children(&d), [d.app.tree.dialog_root(), blocker]);
    let report = d.app.report(S1280, d.now);
    let order: Vec<(&str, bool)> = report
        .dialogs
        .iter()
        .map(|r| (r.name.as_str(), r.modal))
        .collect();
    assert_eq!(order, [("start", false), ("logo", false), ("msgbox", true)]);
    // A click on START hits the blocker: no code 8.
    d.click("start", "options");
    d.frame();
    assert_eq!(d.triggered("options"), 0);
    d.click("msgbox", "ok");
    d.frame();
    assert_eq!(d.triggered("ok"), 1);
}

#[test]
fn input_detects_double_clicks_and_ignores_host_frame_events() {
    let mut app = MenuApp::from_library(
        DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    app.input(UiInput::Frame, S1280, 0);
    app.input(UiInput::Up(MouseButton::Left), S1280, 0);
    app.input(UiInput::Up(MouseButton::Left), S1280, 199);
    app.input(UiInput::Up(MouseButton::Left), S1280, 399);
    let got: Vec<UiInput> = app.mouse_in.iter().map(|(i, _)| i.clone()).collect();
    assert_eq!(
        got,
        [
            UiInput::Up(MouseButton::Left),
            UiInput::Up(MouseButton::Left),
            UiInput::DoubleClick(MouseButton::Left),
            UiInput::Up(MouseButton::Left),
        ]
    );
}

#[test]
fn names_and_centers_report_german_errors() {
    let mut d = combo_driver();
    let err = d.app.open_by_name("nowhere").unwrap_err();
    assert!(
        err.contains("unknown dialog \"nowhere\"") && err.contains("MODAL"),
        "{err}"
    );
    d.app.open_by_name("plain").unwrap();
    d.frame();
    d.frame();
    let err = d.app.element_center("modal", "res", S1280).unwrap_err();
    assert!(
        err.contains("is not open") && err.contains("plain"),
        "{err}"
    );
    let err = d.app.element_center("plain", "x", S1280).unwrap_err();
    assert!(
        err.contains("does not exist") && err.contains("res, cancel"),
        "{err}"
    );
    assert_eq!(
        d.app.element_center("plain", "cancel", S1280),
        Ok((465, 160))
    );
    let half = Screen {
        width: 640,
        height: 512,
    };
    let r = to_screen_rect(half, [415, 140, 515, 180]);
    assert_eq!(
        d.app.element_center("plain", "cancel", half),
        Ok(((r[0] + r[2]) / 2, (r[1] + r[3]) / 2))
    );
    // Report: elements with kind, position and state.
    let report = d.app.report(S1280, d.now);
    let res = report.elements.iter().find(|e| e.name == "res").unwrap();
    assert_eq!((res.kind, res.dialog.as_str()), ("combobox", "plain"));
    assert_eq!(res.items, ["A", "B", "C"]);
    assert!(res.visible && res.enabled && !res.hidden);
    assert!(!report.busy);
    assert_eq!(report.queued, 0);
}

/// Test dialog with `MUSIC` and `BACKGROUND` without a flag (command `0x20` when reading, path discarded) and a button.
const MIXED_DIALOG: &str = "FRAME MIX\n FLAGS MENUFRAME_MUSIC /FLAGS\n EXTENT 100 100 600 500\n\
     BACKGROUND data/x.tga\n MUSIC data/music/x.ogg\n\
     BUTTON GO FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 10 200 50 /BUTTON\n/FRAME\n";

#[test]
fn effects_and_commands_keep_the_original_order() {
    // One effect list for widgets and logic, one queue for logic and `.dlg` reader. When building: `0x20` of the file
    // (when reading `BACKGROUND`) before the command of the query in `OnAttach`, the effect of the query
    // (`0x00461C9C`) before the music (`0x160`, `0x00461CA1`); on a click `SFX_KLICK` before the effect of code 8.
    let library = DialogLibrary::from_sources(&[("mix", MIXED_DIALOG)]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    let answer: Answer = Box::new(|m, env| match (m.element.as_str(), m.code) {
        ("mix", code::QUERY_STATUS) => {
            env.effects.push(UiEffect::SetGamma(2.0));
            env.queue.push(AppCmd::ReloadLevel);
            -1
        }
        ("go", code::TRIGGERED) => {
            env.effects.push(UiEffect::WriteSettings);
            env.queue.push(AppCmd::GameSpy);
            1
        }
        _ => -1,
    });
    let mut d = Driver::with_app(fixture(), app, answer);
    d.push(open("mix", false));
    d.frame();
    assert_eq!(
        d.app.take_effects(),
        [
            UiEffect::SetGamma(2.0),
            UiEffect::MenuMusic {
                path: "DATA/MUSIC/X.OGG".into(),
                looping: true
            }
        ]
    );
    let queued: Vec<AppCmd> = d.app.queue.iter().cloned().collect();
    assert_eq!(queued, [AppCmd::HideLevelBackground, AppCmd::ReloadLevel]);
    d.frame();
    d.frame();
    d.app.take_effects();
    d.click("mix", "go");
    d.frame();
    assert_eq!(
        d.app.take_effects(),
        [
            UiEffect::PlaySfx {
                event: "SFX_KLICK".into(),
                flags: 0x14,
                category: 1
            },
            UiEffect::WriteSettings,
            UiEffect::Unhandled(AppCmd::GameSpy)
        ]
    );
}

/// Test dialog with fly-in (`MOVE`, 300 ms) and a button.
const FLYING_DIALOG: &str = "FRAME FLY\n FLAGS MENUFRAME_MOVE /FLAGS\n EXTENT -500 100 0 500\n\
     ZIELPOSITION 100 100\n DURATION 300\n\
     BUTTON GO FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 10 200 50 /BUTTON\n/FRAME\n";

/// Test dialog without fly-in (transition synchronous) with a button.
const STILL_DIALOG: &str = "FRAME STILL\n EXTENT 700 100 1100 500\n\
     BUTTON GO FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 10 200 50 /BUTTON\n/FRAME\n";

// QUIRK O-21: kept
#[test]
fn escape_during_the_closing_flight_is_refused() {
    // ESC triggers the button with `CHECKESCAPE` (`FUN_0045E0C0`), the logic closes the dialog with a
    // fly-out; a second ESC during the fly-out finds no release (`0x12F` != 0, 12.1.11): no second
    // code 8, no second job. (In the shipped data the credits do not fly out, hence a test dialog.)
    const FLY_BACK: &str = "FRAME FLY
 FLAGS MENUFRAME_MOVE /FLAGS
 EXTENT -500 100 0 500
         ZIELPOSITION 100 100
 DURATION 300
         BUTTON BACK FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 10 10 200 50 /BUTTON
/FRAME
";
    let library = DialogLibrary::from_sources(&[("fly", FLY_BACK)]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    let logic: Answer = Box::new(|m, env| match (m.element.as_str(), m.code) {
        ("back", code::TRIGGERED) => {
            env.queue.push(close(&m.dialog, true));
            1
        }
        _ => -1,
    });
    let mut d = Driver::with_app(fixture(), app, logic);
    d.push(open("fly", false));
    d.run_until(400);
    let fly = d.find("FLY").unwrap();
    assert!(!d.app.menu.busy());
    d.key(0x1B);
    d.frame();
    assert_eq!(d.triggered("back"), 1);
    d.frame();
    assert!(frame_state(&d, fly).moving, "flies out");
    assert!(d.app.menu.busy());
    d.key(0x1B);
    d.frame();
    assert_eq!(
        d.triggered("back"),
        1,
        "second ESC during the fly-out refused"
    );
    assert!(d.app.queue.is_empty(), "no second job");
    d.run_until(d.now + 400);
    assert!(!d.app.tree.contains(fly));
    assert!(d.app.menu.open_names().is_empty());
    assert!(!d.app.menu.busy());
    assert_eq!(d.triggered("back"), 1);
}

// QUIRK O-110: kept
#[test]
fn close_then_parallel_open_of_the_same_dialog_hangs_the_stack() {
    // O-110: `Open X` with flag 0 while `Close X` runs -> running `[Close X, Open X]`. The `0x136` of X
    // (`FUN_004E2820`, `0x004E28CC`/`0x004E294C`) deletes X together with its names at the Close entry; the Open entry
    // finds X no longer afterwards (`FUN_004E24F0(name, 0)`) and stays stuck - `0x12F` refuses every triggering until a
    // newly built X is finished.
    let library = DialogLibrary::from_sources(&[("fly", FLYING_DIALOG), ("still", STILL_DIALOG)]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    let mut d = Driver::with_app(fixture(), app, no_answer());
    d.push(open("fly", false));
    d.run_until(400);
    let first = d.find("FLY").unwrap();
    assert!(!d.app.menu.busy());
    d.push(close("fly", false));
    d.frame();
    assert!(frame_state(&d, first).moving, "flies out");
    d.push(open("fly", false));
    d.frame();
    assert!(d.app.menu.busy());
    d.run_until(d.now + 400);
    // The dialog that flew in again is deleted, the Open entry hangs.
    assert!(!d.app.tree.contains(first));
    assert!(d.find("FLY").is_none());
    assert!(d.app.menu.busy());
    assert_eq!(d.app.menu.open_names(), ["fly"]);
    // Another dialog (flag 0, without fly-in) opens, but its button does not trigger.
    d.push(open("still", false));
    d.frame();
    d.frame();
    assert!(d.find("STILL").is_some());
    d.click("still", "go");
    d.frame();
    assert_eq!(d.triggered("go"), 0, "triggering refused");
    // Only a newly built FLY takes the hanging entry along after its fly-in.
    d.push(open("fly", false));
    d.run_until(d.now + 400);
    assert!(d.find("FLY").is_some_and(|id| id != first));
    assert!(!d.app.menu.busy());
    d.click("still", "go");
    d.frame();
    assert_eq!(d.triggered("go"), 1);
}

/// Dialog for the targeted requery: button `GO` (triggers), button `B` (status from the logic), list `L`.
const REQUERY_DLG: &str = "FRAME RQ\n FLAGS MENUFRAME_NOFRAME /FLAGS\n EXTENT 0 0 1280 1024\n\
     BUTTON GO FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 10 110 50 /BUTTON\n\
     BUTTON B FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 60 110 100 /BUTTON\n\
     LISTBOX L EXTENT 10 110 300 300 /LISTBOX\n/FRAME\n";

/// Driver over [`REQUERY_DLG`]: `GO` requests `{rq, b, 5}`, `{rq, missing, 5}`, `{rq, l, 2}` and sets the
/// counter `n`; `B` is hidden with odd `n`, `L` has `n` entries. With `chain` the status query
/// of `B` in turn requests `{rq, l, 2}`.
fn requery_driver(chain: bool) -> Driver<Fixture> {
    let library = DialogLibrary::from_sources(&[("rq", REQUERY_DLG)]);
    let app = MenuApp::from_library(library, EngineConfig::default(), MenuOptions::default());
    let n = Rc::new(RefCell::new(0));
    Driver::with_app(
        fixture(),
        app,
        Box::new(move |m, env| match (m.element.as_str(), m.code) {
            ("go", code::TRIGGERED) => {
                *n.borrow_mut() += 1;
                let list = if chain {
                    vec!["b"]
                } else {
                    vec!["b", "missing", "l"]
                };
                for (element, c) in list.into_iter().zip([5, 5, 2]) {
                    env.requery.push(Requery::new("RQ", element, c));
                }
                1
            }
            ("b", code::QUERY_STATUS) => {
                let hide = if *n.borrow() % 2 == 1 { 0x1000_0000 } else { 0 };
                m.params.insert("status".into(), Param::Int(hide));
                if chain && *n.borrow() > 0 {
                    env.requery.push(Requery::new("rq", "l", 2));
                }
                1
            }
            ("l", code::QUERY_ITEMS) => {
                let items = (0..*n.borrow()).map(|i| format!("E{i}")).collect();
                m.params.insert("array".into(), Param::Array(items));
                1
            }
            _ => -1,
        }),
    )
}

fn hidden(d: &Driver<Fixture>, key: &str) -> bool {
    d.app.tree.node(d.find(key).unwrap()).flags & common_flags::HIDE != 0
}

fn list_items(d: &Driver<Fixture>) -> Vec<String> {
    match &d.app.tree.node(d.find("RQL").unwrap()).kind {
        NodeKind::ListBox(s) => s.items.clone(),
        _ => unreachable!(),
    }
}

#[test]
fn requery_asks_the_named_elements_after_the_handler() {
    // "Requery X" (FUN_00464970 + FUN_004DD600): after the return of the handler in order, nodes via the
    // name table; a missing element is ignored.
    let mut d = requery_driver(false);
    d.app.open_by_name("rq").unwrap();
    d.idle();
    assert!(!hidden(&d, "RQB"));
    assert!(list_items(&d).is_empty());
    d.log.borrow_mut().clear();
    d.click("rq", "go");
    d.frame();
    assert!(hidden(&d, "RQB"), "status queried again");
    assert_eq!(list_items(&d), ["E0"], "list refilled");
    let asked: Vec<(String, u32)> = d
        .log
        .borrow()
        .iter()
        .map(|(_, e, c)| (e.clone(), *c))
        .filter(|(e, _)| e != "go")
        .collect();
    // B: code 5 (afterwards 7 of the requery of the button), then L with the full query.
    let first_l = asked.iter().position(|(e, _)| e == "l").unwrap();
    assert!(asked[..first_l].iter().all(|(e, _)| e == "b"));
    assert!(asked[..first_l].contains(&("b".to_string(), code::QUERY_STATUS)));
    assert!(asked[first_l..].contains(&("l".to_string(), code::QUERY_ITEMS)));
    assert!(d.app.requery.is_empty());
    // Second click: visible again, two entries.
    d.click("rq", "go");
    d.frame();
    assert!(!hidden(&d, "RQB"));
    assert_eq!(list_items(&d), ["E0", "E1"]);
}

#[test]
fn requery_from_a_requery_runs_in_the_same_step() {
    let mut d = requery_driver(true);
    d.app.open_by_name("rq").unwrap();
    d.idle();
    d.click("rq", "go");
    d.frame();
    assert!(hidden(&d, "RQB"));
    assert_eq!(list_items(&d), ["E0"], "requery from the response of B");
    assert!(d.app.requery.is_empty());
}

// Menu level version of the dialogs, commands `0x20`/`0x21` as effects, `levelbackground.bkg`.

const LEVEL: &str = "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL";

fn menu_level_effects(d: &mut Driver<impl World>) -> Vec<MenuLevelEffect> {
    d.app
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            UiEffect::MenuLevel(m) => Some(m),
            _ => None,
        })
        .collect()
}

#[test]
fn menu_builds_the_variant_of_the_build_time() {
    // O-438: the switch applies only to the next construction; open dialogs stay as they were built.
    let Some(mut d) = corpus_driver(false, no_answer()) else {
        return;
    };
    assert!(!d.app.config.is_menu_level);
    d.push(open("options", true));
    d.idle();
    assert_eq!(menu_level_effects(&mut d), [MenuLevelEffect::Hide]);
    d.app.config.is_menu_level = true;
    d.idle();
    assert_eq!(menu_level_effects(&mut d), [], "open dialogs stay");
    d.push(close("options", true));
    d.push(open("start", true));
    d.idle();
    assert_eq!(
        menu_level_effects(&mut d),
        [MenuLevelEffect::Load { path: LEVEL.into() }],
        "START rebuilt: version with IsMenuLevel, 0x21 instead of 0x20"
    );
    // Back to 0: the next construction of OPTIONS sends 0x20 again.
    d.app.config.is_menu_level = false;
    d.push(close("start", true));
    d.push(open("options", true));
    d.idle();
    assert_eq!(menu_level_effects(&mut d), [MenuLevelEffect::Hide]);
}

#[test]
fn menu_level_commands_become_effects() {
    let mut d = Driver::with_app(
        fixture(),
        MenuApp::from_library(
            DialogLibrary::default(),
            EngineConfig::default(),
            MenuOptions::default(),
        ),
        no_answer(),
    );
    // The path arrives as the reader supplies it (uppercase).
    d.push(AppCmd::LoadMenuLevel(LEVEL.into()));
    d.push(AppCmd::HideLevelBackground);
    d.frame();
    assert_eq!(
        d.app.take_effects(),
        [UiEffect::MenuLevel(MenuLevelEffect::Load {
            path: LEVEL.into()
        })]
    );
    d.frame();
    assert_eq!(
        d.app.take_effects(),
        [UiEffect::MenuLevel(MenuLevelEffect::Hide)]
    );
}

#[test]
fn level_world_is_reported_and_applied_per_frame() {
    let mut d = Driver::with_app(
        fixture(),
        MenuApp::from_library(
            DialogLibrary::default(),
            EngineConfig::default(),
            MenuOptions::default(),
        ),
        no_answer(),
    );
    assert_eq!(d.app.level_world(), LevelWorld::default());
    let w = LevelWorld {
        present: true,
        shown: true,
    };
    d.app.set_level_world(w);
    assert_eq!(d.app.level_world(), w);
    d.frame();
    assert!(d.app.tree.world_shown);
    d.app.set_level_world(LevelWorld {
        present: true,
        shown: false,
    });
    d.frame();
    assert!(!d.app.tree.world_shown);
}

#[test]
fn level_background_bkg_only_over_a_shown_world() {
    // `FUN_0045CAF0` -> `FUN_004BA680`: the `.bkg` of the desktop only with bit `0x1` set and without video.
    let Some(mut d) = corpus_driver(false, no_answer()) else {
        return;
    };
    d.app.config.is_menu_level = true;
    d.push(open("start", true));
    d.idle();
    let mut want = Vec::new();
    {
        let ctx = d.world.ctx(S1280);
        let bkg = crate::bkg_paint::load_bkg(ctx.vfs, "data/menü/levelbackground.bkg", ctx.assets)
            .expect("levelbackground.bkg");
        let screen = to_screen_rect(S1280, crate::tree::FULL);
        crate::bkg_paint::paint_bkg(
            &bkg,
            crate::tree::FULL,
            screen,
            S1280,
            ctx.assets,
            &mut want,
        );
    }
    assert!(!want.is_empty(), "the .bkg draws tiles");
    // World invisible (default): no tiles.
    let hidden = d.frame();
    assert!(!hidden.starts_with(&want));
    // World visible: the tiles come before the images of the desktop, the rest stays the same.
    d.app.set_level_world(LevelWorld {
        present: true,
        shown: true,
    });
    let shown = d.frame();
    assert_eq!(shown[..want.len()], want[..]);
    assert_eq!(shown[want.len()..], hidden[..]);
    // With video no tiles, also with a visible world.
    let tex = d
        .world
        .ctx(S1280)
        .assets
        .insert_image(VIDEO_TEXTURE, image::RgbaImage::new(4, 4));
    let now = d.now;
    d.app.video_started("data/videos/a.bik", LONG, tex, now);
    d.app.take_effects();
    let with_video = d.frame();
    d.app.set_level_world(LevelWorld::default());
    let video_hidden = d.frame();
    assert_eq!(with_video, video_hidden);
}

#[test]
fn level_background_bkg_cleared_by_a_frame_without_the_flag() {
    // `FUN_0045A0A0` (called from `OnAttach` `0x00461D17`...): a frame without `MENUFRAME_LEVELBACKGROUND` (`0x80`)
    // and without `MENUFRAME_DONOTCLEARBACKGROUND` (so `credits.dlg`: `0x440`; most menus carry `0x100000`) frees the
    // sets of the desktop - the tiles of `levelbackground.bkg` disappear, also with a visible world; a frame with
    // `0x80` loads them anew.
    let Some(mut d) = corpus_driver(false, no_answer()) else {
        return;
    };
    d.app.config.is_menu_level = true;
    d.push(open("start", true));
    d.idle();
    d.app.set_level_world(LevelWorld {
        present: true,
        shown: true,
    });
    let with_bkg = d.frame();
    d.push(open("credits", true));
    d.idle();
    let cleared = d.frame();
    let ctx = d.world.ctx(S1280);
    let bkg = crate::bkg_paint::load_bkg(ctx.vfs, "data/menü/levelbackground.bkg", ctx.assets)
        .expect("levelbackground.bkg");
    let mut want = Vec::new();
    crate::bkg_paint::paint_bkg(
        &bkg,
        crate::tree::FULL,
        to_screen_rect(S1280, crate::tree::FULL),
        S1280,
        ctx.assets,
        &mut want,
    );
    assert!(with_bkg.starts_with(&want), "START: tiles present");
    assert!(
        !cleared.starts_with(&want),
        "frame without 0x80 and DONOTCLEARBACKGROUND: tiles gone"
    );
}
