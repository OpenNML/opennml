//! Tests of `ui_click` on `GAMEUI` and `ui_state.game.hud.toolbar` through the flow: `GameFlow` with a real
//! `GameSession` (corpus and GPU, otherwise silently skipped) on the two-player map. The clicks go through
//! [`GameFlow::gameui_click`] as in `opennml-game`; the menu host supplies only the game menu.

use opennml_formats::gifcon::button_kind as kinds;
use opennml_sim::command::CommandKind;
use opennml_sim::id::ObjectId;
use opennml_sim::object::GameObject;
use opennml_ui::app::MenuOptions;
use opennml_ui::hud::portrait::{VALUE_ORDER, value_text};
use opennml_ui::hud::toolbar::button::{KIND_PRODUCTION, KIND_SELECTION};

use super::tests::{S1024, request};
use super::*;
use crate::game::hud::SimGameText;
use crate::game::toolbar_host::portrait_model;
use crate::menu_host::MenuHost;
use crate::testutil::FakeAudio;
use crate::testutil::test_config;
use crate::ui_layer::UiLayer;

/// A running match with flow, menu host and session at 1024 x 768.
pub(super) struct Match {
    pub(super) vfs: Vfs,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub(super) host: MenuHost,
    pub(super) flow: GameFlow,
    pub(super) session: Option<GameSession>,
    view: ViewOptions,
    controls: Controls,
    files: FsSaveFiles,
    pub(super) now: u64,
    _tmp: tempfile::TempDir,
}

impl Match {
    pub(super) fn start() -> Option<Match> {
        let dir = opennml_rda::testutil::corpus_game_dir()?;
        let Ok((device, queue, _)) =
            opennml_render::headless_device(opennml_render::BackendChoice::Auto)
        else {
            eprintln!("no GPU, test skipped");
            return None;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let files = FsSaveFiles {
            dir: tmp.path().join("savegame"),
        };
        let layer = UiLayer::new(&vfs, 1024, None).unwrap();
        let config = EngineConfig {
            is_intro: false,
            ..test_config(&vfs)
        };
        let audio = FakeAudio::default();
        let host =
            MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
        let flow = GameFlow::new(&host.layer.texts);
        let mut m = Match {
            vfs,
            device,
            queue,
            host,
            flow,
            session: None,
            view: ViewOptions::default(),
            controls: Controls::default(),
            files,
            now: 0,
            _tmp: tmp,
        };
        let effect = UiEffect::StartMatch(request(0));
        let env = FlowEnv {
            vfs: &m.vfs,
            texts: &m.host.layer.texts,
            files: &m.files,
        };
        let mut view = LiveSession {
            device: m.device.clone(),
            queue: m.queue.clone(),
            format: opennml_render::CAPTURE_FORMAT,
            vfs: &m.vfs,
            session: &mut m.session,
            view: &mut m.view,
            controls: &mut m.controls,
            size: (320, 200),
            audio: None,
        };
        assert!(
            m.flow
                .apply(&effect, &mut m.host.app, &mut view, &env, &mut |_, _| {})
        );
        let session = m.session.as_mut().unwrap();
        session.resize(1024, 768);
        Some(m)
    }

    /// `f` with flow, menu and the session as a `SessionView`.
    pub(super) fn with_view<R>(
        &mut self,
        f: impl FnOnce(&mut GameFlow, &mut MenuApp, &mut LiveSession) -> R,
    ) -> R {
        let mut view = LiveSession {
            device: self.device.clone(),
            queue: self.queue.clone(),
            format: opennml_render::CAPTURE_FORMAT,
            vfs: &self.vfs,
            session: &mut self.session,
            view: &mut self.view,
            controls: &mut self.controls,
            size: (1024, 768),
            audio: None,
        };
        f(&mut self.flow, &mut self.host.app, &mut view)
    }

    pub(super) fn session(&self) -> &GameSession {
        self.session.as_ref().unwrap()
    }

    /// One frame of 16 ms program time: step of the match including the frame step of the bar, then drawn (the
    /// tooltip areas and the box arise during drawing).
    pub(super) fn frame(&mut self) {
        self.now += 16;
        let now = self.now;
        // Like `with_view`, plus the texts for the requests of the scripts (`GameFlow::frame`).
        let mut view = LiveSession {
            device: self.device.clone(),
            queue: self.queue.clone(),
            format: opennml_render::CAPTURE_FORMAT,
            vfs: &self.vfs,
            session: &mut self.session,
            view: &mut self.view,
            controls: &mut self.controls,
            size: (1024, 768),
            audio: None,
        };
        self.flow
            .frame(now, &mut self.host.app, &mut view, &self.host.layer.texts);
        self.draw();
    }

    fn draw(&mut self) {
        let mut layer = UiLayer::new(&self.vfs, 1024, None).unwrap();
        let mut ctx = layer.context(&self.vfs, S1024);
        let texts = &self.host.layer.texts;
        self.session().hud_cmds(&mut ctx, texts);
    }

    /// Select the first own object that `pick` allows (like clicking it); two frames build the bar.
    fn select_own(&mut self, pick: impl Fn(&SimHost, &GameObject) -> bool) -> ObjectId {
        let session = self.session.as_mut().unwrap();
        let sim = session.sim().unwrap();
        let local = usize::from(sim.active_player);
        let id = sim.state.tables[local]
            .objects
            .iter()
            .flatten()
            .find(|o| pick(sim, o))
            .map(|o| o.id)
            .expect("matching own object");
        session.selection.ids = vec![id];
        session.selection_changed(&self.vfs, None);
        self.frame();
        self.frame();
        id
    }

    /// Report of the command bar like `ui_state.game.hud.toolbar`.
    fn report(&self) -> opennml_control::UiToolbarData {
        self.session()
            .hud_state(&self.host.layer.texts)
            .expect("report")
            .toolbar
            .expect("command bar")
    }

    fn click(&mut self, element: &str) -> Result<(i32, i32), String> {
        let now = self.now;
        self.with_view(|flow, app, view| {
            flow.gameui_click(element, MouseButton::Left, S1024, now, app, view)
        })
    }

    /// A clickable button of the bar: element name and `BUTTONMSG`.
    fn clickable(
        &self,
        pick: impl Fn(&opennml_ui::hud::toolbar::button::Button) -> bool,
    ) -> (String, i32) {
        let toolbar = self.session().hud.toolbar.as_ref().unwrap();
        let b = toolbar
            .buttons()
            .find(|b| b.clickable && pick(b))
            .expect("clickable button");
        let name = self
            .report()
            .buttons
            .into_iter()
            .find(|r| r.rect == b.spec.rect)
            .expect("button in the report")
            .element;
        (name, b.spec.msg)
    }
}

/// Own object with a bar menu.
fn has_menu(sim: &SimHost, o: &GameObject) -> bool {
    sim.rules
        .toolbar
        .menus
        .get(&(o.guid as i32))
        .is_some_and(|menu| !menu.is_empty())
}

/// Every name from the report is a target of `ui_click`: the click hits the center of the rectangle, the button
/// triggers the click path (`last_button`). `guid:<msg>` means the same normal button as `normal:<i>`; `toolbar` is
/// free bar area `(630, H - 86)` and triggers nothing; `statusbar` lies in the status bar.
#[test]
fn ui_click_reaches_toolbar_elements_by_name() {
    let Some(mut m) = Match::start() else {
        return;
    };
    m.select_own(has_menu);
    let report = m.report();
    assert!(!report.buttons.is_empty());
    let mut names: Vec<_> = report.buttons.iter().map(|b| b.element.clone()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), report.buttons.len(), "names unique");
    // The center of every button hits its rectangle; the answer is the point.
    for b in &report.buttons {
        let (x, y) = m.session().gameui_center(&b.element).unwrap();
        let [l, t, r, bo] = b.rect;
        assert!(
            (f64::from(l)..f64::from(r)).contains(&x) && (f64::from(t)..f64::from(bo)).contains(&y),
            "{}: ({x}, {y}) in {:?}",
            b.element,
            b.rect
        );
    }
    // Click on a normal button: `normal:<i>` and `guid:<msg>` mean the same.
    let (name, msg) = m.clickable(|b| b.spec.object.is_none() && b.spec.kind() != 0);
    let by_guid = format!("guid:{msg}");
    let first = report
        .buttons
        .iter()
        .find(|b| b.element.starts_with("normal:") && b.msg == msg)
        .expect("normal button with this message");
    assert_eq!(
        m.session().gameui_center(&by_guid),
        m.session().gameui_center(&first.element)
    );
    assert_eq!(m.session().last_button, None);
    let (x, y) = m.click(&name).unwrap();
    let [l, t, r, bo] = m
        .report()
        .buttons
        .iter()
        .find(|b| b.element == name)
        .unwrap()
        .rect;
    assert!((l..r).contains(&x) && (t..bo).contains(&y), "({x}, {y})");
    assert_eq!(
        m.session().last_button,
        Some(msg),
        "click path of the button"
    );
    assert_eq!(m.report().last_button, Some(msg));
    // Free bar area and status bar: point as defined, no effect on the last button.
    m.frame();
    assert_eq!(m.click("toolbar"), Ok((630, 768 - 86)));
    assert_eq!(m.session().last_button, Some(msg));
    let status = m
        .session()
        .hud
        .windows
        .find(opennml_ui::hud::tree::WindowKind::StatusBar)
        .map(|id| m.session().hud.windows.rect(id))
        .expect("status bar");
    let (x, y) = m.click("statusbar").unwrap();
    assert!((status[0]..status[2]).contains(&x) && (status[1]..status[3]).contains(&y));
}

/// Two `ui_click`s before the same frame have the same clock: the second click is a double-click (original: 9, 9,
/// `0x27`, for MAKE 1 + 1 + 2 = 4 orders, O-224). If a wait above `DoubleClickTimeout` lies between them, they are two
/// single clicks.
#[test]
fn two_ui_clicks_in_one_frame_are_a_double_click() {
    let Some(mut m) = Match::start() else {
        return;
    };
    let building = m.select_own(|sim, o| {
        sim.rules
            .toolbar
            .menus
            .get(&(o.guid as i32))
            .is_some_and(|menu| menu.iter().any(|b| b.kind() == kinds::MAKE))
    });
    let (name, _) = m.clickable(|b| b.spec.kind() == kinds::MAKE);
    let queued = |m: &Match| {
        m.session()
            .sim()
            .unwrap()
            .pending_commands()
            .iter()
            .filter(|c| matches!(c.kind, CommandKind::EnqueueProduction { building: b, .. } if b == building))
            .count()
    };
    assert_eq!(queued(&m), 0);
    m.click(&name).unwrap();
    m.click(&name).unwrap();
    assert_eq!(queued(&m), 4, "double-click: 1 + 1 + 2");
    // Deliver the commands at the tick boundary, then two clicks with a wait between them: two single clicks.
    m.session.as_mut().unwrap().sim_mut().unwrap().step_once();
    assert_eq!(queued(&m), 0);
    m.now += 10_000;
    m.click(&name).unwrap();
    m.now += 10_000;
    m.click(&name).unwrap();
    assert_eq!(queued(&m), 2, "two single clicks");
}

/// Unknown elements, missing match and open game menu.
#[test]
fn unknown_gameui_element_is_rejected_in_german() {
    // Without a match: no session, no center.
    let Some(mut m) = Match::start() else {
        return;
    };
    for element in [
        "banana",
        "normal:99",
        "normal:x",
        "queue:-1",
        "value:0",
        "value:abc",
        "guid:9999999",
        "portrait:1",
        "selection:30",
        "counterupgrade:0",
        ":",
    ] {
        assert_eq!(
            m.click(element),
            Err(format!("element {element} does not exist in GAMEUI")),
            "{element}"
        );
    }
    // Game menu open: the click is blocked as by hand (answer as usual, no effect).
    m.select_own(has_menu);
    let (name, _) = m.clickable(|b| b.spec.object.is_none());
    // Hidden bar (event `0x11`): its buttons cannot be clicked.
    m.session
        .as_mut()
        .unwrap()
        .hud
        .toolbar
        .as_mut()
        .unwrap()
        .shown = false;
    assert_eq!(m.click(&name), Err("command bar is hidden".to_string()));
    m.session
        .as_mut()
        .unwrap()
        .hud
        .toolbar
        .as_mut()
        .unwrap()
        .shown = true;
    m.host.app.open_game_menu();
    assert_ne!(m.host.app.game_menu_state(), 0);
    assert!(m.click(&name).is_ok());
    assert_eq!(m.session().last_button, None, "game menu blocks the click");
    // Without a running match: "match is not running", also for names that would exist.
    let mut idle = GameFlow::default();
    let mut fake = crate::testutil::FakeSession::default();
    let result = idle.gameui_click(
        "normal:0",
        MouseButton::Left,
        S1024,
        0,
        &mut m.host.app,
        &mut fake,
    );
    assert_eq!(result, Err("match is not running".to_string()));
}

/// `ui_state.game.hud.toolbar`: buttons with names, portrait with values, tooltip after moving onto a button and the
/// last button after the click. The elements `portrait`, `value:<i>` lead to the areas of the last drawing.
#[test]
fn ui_state_reports_buttons_portrait_tooltip_and_last_button() {
    let Some(mut m) = Match::start() else {
        return;
    };
    let id = m.select_own(has_menu);
    let report = m.report();
    let toolbar = m.session().hud.toolbar.as_ref().unwrap();
    assert_eq!(report.shown_menu, toolbar.shown_menu());
    assert_eq!(report.selected, id.0 as i32);
    assert_eq!(report.buttons.len(), toolbar.buttons().count());
    assert_eq!(report.last_button, None, "before the first click");
    assert_eq!(report.cursor, None);
    assert_eq!(report.tooltip, None, "mouse nowhere yet");
    // Portrait: type GUID of the selected object, subtitle from the text table, drawn values.
    let guid = m
        .session()
        .sim()
        .unwrap()
        .state
        .object(id, false)
        .unwrap()
        .guid as i32;
    let portrait = report.portrait.expect("portrait");
    assert_eq!(portrait.guid, guid);
    assert_eq!(portrait.subtitle, m.host.layer.texts.display(guid));
    let areas = m.session().hud.areas.borrow().clone();
    assert_eq!(
        portrait.values.len() + 1,
        areas.portrait_areas().len(),
        "one value row per area behind the portrait"
    );
    // The values are those of the portrait model in the sequence of `VALUE_ORDER`, as text like drawn.
    let model = portrait_model(m.session().sim().unwrap(), toolbar);
    let object = model.object.as_ref().expect("portrait object");
    let expected: Vec<(u32, String)> = object
        .values
        .iter()
        .filter(|v| VALUE_ORDER.iter().any(|k| k.attribute == v.attribute))
        .map(|v| (v.attribute, value_text(v)))
        .take(toolbar.data.portrait.len() - 2)
        .collect();
    assert!(!expected.is_empty(), "the object has values");
    assert_eq!(portrait.values, expected);
    // The tooltip GUID of the value rows is `icon + 20` of the drawn order.
    let value_guids: Vec<i32> = areas.portrait_areas()[1..].iter().map(|a| a.1).collect();
    let expected_guids: Vec<i32> = portrait
        .values
        .iter()
        .map(|(attribute, _)| {
            VALUE_ORDER
                .iter()
                .find(|k| k.attribute == *attribute)
                .unwrap()
                .tooltip_guid()
        })
        .collect();
    assert_eq!(value_guids, expected_guids);
    let (x, y) = m.session().gameui_center("portrait").unwrap();
    let [l, t, r, b] = areas.portrait_areas()[0].0;
    assert!((f64::from(l)..f64::from(r)).contains(&x) && (f64::from(t)..f64::from(b)).contains(&y));
    for i in 0..portrait.values.len() {
        m.session().gameui_center(&format!("value:{i}")).unwrap();
    }
    assert!(
        m.session()
            .gameui_center(&format!("value:{}", portrait.values.len()))
            .is_err()
    );
    // Mouse onto a button, one frame: tooltip with GUID, kind, text and box.
    let (name, msg) = m.clickable(|b| b.spec.object.is_none() && b.spec.kind() != 0);
    m.click(&name).unwrap();
    m.frame();
    let report = m.report();
    assert_eq!(report.last_button, Some(msg));
    assert_eq!(
        report.hover,
        m.session().hud.toolbar.as_ref().unwrap().hover
    );
    let button = report.buttons.iter().find(|b| b.element == name).unwrap();
    assert_eq!(button.msg, msg);
    assert_eq!(button.state, 1, "mouse over the button");
    // The kind is the word of the `toolbar.ini` row of this BUTTONMSG in the menu of the object.
    let row = m
        .session()
        .sim()
        .unwrap()
        .rules
        .toolbar
        .menus
        .get(&guid)
        .and_then(|menu| menu.iter().find(|r| r.msg == msg))
        .expect("row in the menu");
    assert_eq!(button.kind, row.flags);
    assert_eq!(button.progress, 100, "normal button without veil");
    let tip = report.tooltip.expect("tooltip over the button");
    assert_eq!((tip.guid, tip.kind), (msg, row.flags & kinds::KIND_MASK));
    // The text comes from the same inputs as the drawing (selection, formation, key layout) ...
    let session = m.session();
    let texts = &m.host.layer.texts;
    let game = SimGameText::new(session.sim().unwrap(), texts)
        .with_selection(&session.selection.ids)
        .with_formation(session.toolbar_host.bar.formation());
    let keys = opennml_ui::screens::text_context(&session.key_map, texts, None);
    let drawn = session
        .hud
        .tooltip
        .text(&game, &keys, session.hud.enhanced_tooltips)
        .expect("text");
    assert_eq!(tip.text, drawn);
    // ... and begins with the name of the GUID, `[KEYID n]` replaced by the key; the extra line follows as the second.
    let raw = texts.display(msg);
    let title = raw.split(" [KEYID").next().unwrap();
    let first = tip.text.lines().next().unwrap();
    assert!(first.starts_with(title), "{first:?} / {raw:?}");
    assert!(!tip.text.contains("[KEYID"), "{:?}", tip.text);
    assert!(tip.text.lines().count() >= 2, "{:?}", tip.text);
    // Box: left at the screen edge, bottom edge `H - 200`, exactly the size of the drawn layout.
    let layer = UiLayer::new(&m.vfs, 1024, None).unwrap();
    let layout = session
        .hud
        .tooltip
        .layout(&layer.fonts, &game, &keys, session.hud.enhanced_tooltips)
        .expect("layout");
    assert_eq!(
        tip.rect,
        [0, 768 - 200 - layout.height, layout.width, 768 - 200]
    );
    assert_eq!(session.hud.tooltip_box.get(), Some(tip.rect));
    // Mouse away (free bar area): the button falls to state 0, the tooltip disappears.
    m.click("toolbar").unwrap();
    m.frame();
    let report = m.report();
    let button = report.buttons.iter().find(|b| b.element == name).unwrap();
    assert_eq!(button.state, 0);
    assert_eq!(report.tooltip, None);
    assert_eq!(
        report.last_button,
        Some(msg),
        "free area does not change it"
    );
    // The report round-trips as a line (Eq holds).
    let hud = m.session().hud_state(&m.host.layer.texts).unwrap();
    let line = serde_json::to_value(&hud).unwrap();
    assert_eq!(serde_json::from_value::<UiHudData>(line).unwrap(), hud);
}

/// Production and multi-selection on the real shipped data: the running production is called `queue:0`, waiting ones
/// by the index of the kind bits (`queue:1` ..., like the cancel command), `progress` is the progress of the running
/// order in percent; the multi-selection `selection:<i>` with the HID as `msg`.
#[test]
fn production_queue_and_multi_selection_are_named_by_index() {
    let Some(mut m) = Match::start() else {
        return;
    };
    m.select_own(|sim, o| {
        sim.rules
            .toolbar
            .menus
            .get(&(o.guid as i32))
            .is_some_and(|menu| menu.iter().any(|b| b.kind() == kinds::MAKE))
    });
    let (name, msg) = m.clickable(|b| b.spec.kind() == kinds::MAKE);
    // Double-click: four orders (O-224), the first runs, three wait.
    m.click(&name).unwrap();
    m.click(&name).unwrap();
    let mut frames = 0;
    let running = loop {
        m.frame();
        frames += 1;
        let report = m.report();
        let queue: Vec<_> = report
            .buttons
            .iter()
            .filter(|b| b.element.starts_with("queue:"))
            .collect();
        if queue.len() == 4 && queue[0].progress > 0 {
            break report;
        }
        assert!(
            frames < 2_000,
            "production not running: {:?}",
            report.buttons
        );
    };
    let queue: Vec<_> = running
        .buttons
        .iter()
        .filter(|b| b.element.starts_with("queue:"))
        .collect();
    for (i, b) in queue.iter().enumerate() {
        assert_eq!(b.element, format!("queue:{i}"));
        assert_eq!(
            b.kind,
            KIND_PRODUCTION | i as u32,
            "kind bits carry the index"
        );
        assert_eq!(
            b.msg, msg,
            "GUID of the order = BUTTONMSG of the MAKE button"
        );
    }
    // Running order: progress of the button in whole percent, real intermediate value; waiting ones without veil.
    let toolbar = m.session().hud.toolbar.as_ref().unwrap();
    let first = toolbar.set.production[0].progress();
    assert_eq!(queue[0].progress, (first * 100.0).round() as i32);
    assert!(
        (1..100).contains(&queue[0].progress),
        "{}",
        queue[0].progress
    );
    assert!(queue[1..].iter().all(|b| b.progress == 100));
    // The names lead to the center of the buttons.
    for b in &queue {
        let (x, y) = m.session().gameui_center(&b.element).unwrap();
        let [l, t, r, bo] = b.rect;
        assert!(
            (f64::from(l)..f64::from(r)).contains(&x) && (f64::from(t)..f64::from(bo)).contains(&y)
        );
    }
    // The tooltip of the production: GUID of the order, kind `0x6000000`.
    m.click("queue:0").unwrap();
    m.frame();
    let tip = m.report().tooltip.expect("tooltip of the production");
    assert_eq!((tip.guid, tip.kind), (msg, KIND_PRODUCTION));

    // Multi-selection: two own objects.
    let session = m.session.as_mut().unwrap();
    let sim = session.sim().unwrap();
    let local = usize::from(sim.active_player);
    let ids: Vec<ObjectId> = sim.state.tables[local]
        .objects
        .iter()
        .flatten()
        .map(|o| o.id)
        .take(3)
        .collect();
    assert_eq!(ids.len(), 3);
    session.selection.ids = ids.clone();
    session.selection_changed(&m.vfs, None);
    m.frame();
    m.frame();
    let report = m.report();
    assert_eq!(report.selected, -1);
    let selection: Vec<_> = report
        .buttons
        .iter()
        .filter(|b| b.element.starts_with("selection:"))
        .map(|b| (b.element.clone(), b.msg, b.kind))
        .collect();
    let want: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            (
                format!("selection:{i}"),
                id.0 as i32,
                KIND_SELECTION | i as u32,
            )
        })
        .collect();
    assert_eq!(selection, want);
    // The portrait of the group: text 500, no values.
    let portrait = report.portrait.expect("portrait of the group");
    assert_eq!(portrait.guid, 500);
    assert_eq!(portrait.subtitle, m.host.layer.texts.display(500));
    assert!(portrait.values.is_empty());
    // A click on `selection:1` triggers the click path (selection button: object to the front, `last_button` = HID).
    m.click("selection:1").unwrap();
    assert_eq!(m.session().last_button, Some(ids[1].0 as i32));
}

/// Submenu (button without kind) in the menu of `o` with UPGRADE buttons; in the shipped data the research 600 of the
/// headquarters.
fn upgrade_menu(sim: &SimHost, o: &GameObject) -> Option<i32> {
    let menus = &sim.rules.toolbar.menus;
    menus.get(&(o.guid as i32)).and_then(|menu| {
        menu.iter().map(|b| b.msg).find(|sub| {
            menus
                .get(sub)
                .is_some_and(|s| s.iter().any(|b| b.kind() == kinds::UPGRADE))
        })
    })
}

/// On the real shipped data: a cancelled upgrade appears again at once among the normal buttons, as a waiting
/// (`queue:1`) and as a running order (`queue:0`). Original: `0x00507960` reports `{0x10, 0}`, the silent rebuild
/// checks the hiding of the one-time buttons anew (`0x004557C0`).
// QUIRK O-395: kept
#[test]
fn cancelled_upgrade_returns_to_the_toolbar() {
    let Some(mut m) = Match::start() else {
        return;
    };
    let makes = |sim: &SimHost, o: &GameObject| {
        sim.rules
            .toolbar
            .menus
            .get(&(o.guid as i32))
            .is_some_and(|menu| menu.iter().any(|b| b.kind() == kinds::MAKE))
    };
    let hq = m.select_own(|sim, o| makes(sim, o) && upgrade_menu(sim, o).is_some());
    let sub = {
        let sim = m.session().sim().unwrap();
        upgrade_menu(sim, sim.state.object(hq, false).unwrap()).unwrap()
    };
    let (make, _) = m.clickable(|b| b.spec.kind() == kinds::MAKE);
    let (sub_button, _) = m.clickable(|b| b.spec.msg == sub);
    // Distance between two clicks so that no double-click arises (O-224, O-225).
    let pause = |m: &mut Match| {
        for _ in 0..40 {
            m.frame();
        }
    };
    // First a MAKE order (it runs), then open the submenu with the upgrades.
    m.click(&make).unwrap();
    pause(&mut m);
    m.click(&sub_button).unwrap();
    pause(&mut m);
    let (upgrade, msg) = m.clickable(|b| b.spec.kind() == kinds::UPGRADE);
    let shown = |m: &Match| {
        m.report()
            .buttons
            .iter()
            .any(|b| b.element.starts_with("normal:") && b.msg == msg)
    };
    let queued = |m: &Match| {
        m.report()
            .buttons
            .iter()
            .filter(|b| b.element.starts_with("queue:"))
            .map(|b| b.msg)
            .collect::<Vec<_>>()
    };
    assert!(shown(&m), "upgrade button {msg} at the start");

    // Frames until the cancel has arrived in the simulation step of a frame (not every frame computes one).
    let until = |m: &mut Match, done: &dyn Fn(&Match) -> bool| {
        for _ in 0..20 {
            m.frame();
            if done(m) {
                return;
            }
        }
        panic!("cancel did not arrive");
    };

    // Waiting: the upgrade behind the MAKE order.
    m.click(&upgrade).unwrap();
    pause(&mut m);
    let queue = queued(&m);
    assert_eq!(
        queue.len(),
        2,
        "running MAKE and waiting upgrade: {queue:?}"
    );
    assert_eq!(queue[1], msg);
    assert!(!shown(&m), "queued: button hidden");
    m.click("queue:1").unwrap();
    until(&mut m, &|m| queued(m).len() == 1);
    assert!(
        shown(&m),
        "cancelled while waiting: button back in the same frame"
    );
    pause(&mut m);

    // Running: cancel the MAKE order, then queue the upgrade and cancel it.
    m.click("queue:0").unwrap();
    pause(&mut m);
    assert!(queued(&m).is_empty());
    m.click(&upgrade).unwrap();
    pause(&mut m);
    assert_eq!(queued(&m), [msg]);
    assert!(!shown(&m), "running: button hidden");
    m.click("queue:0").unwrap();
    until(&mut m, &|m| queued(m).is_empty());
    assert!(
        shown(&m),
        "cancelled while running: button back in the same frame"
    );
}

/// Also in the pause a cancelled upgrade appears again in the next frame. The cancel runs there at once
/// (`SimHost::execute_now`, O-318), without a tick; the pause frame gives the bar the events of the immediate command
/// (`SimHost::take_immediate_events`). Original: `0x00507960` calls `NotifyUiIfSelected(0)` synchronously, the bar is
/// operable in the pause (re/game-ui N.4). After resuming the button stays.
// QUIRK O-395: kept
#[test]
fn cancelled_upgrade_returns_to_the_toolbar_in_the_pause() {
    let Some(mut m) = Match::start() else {
        return;
    };
    let hq = m.select_own(|sim, o| upgrade_menu(sim, o).is_some());
    let sub = {
        let sim = m.session().sim().unwrap();
        upgrade_menu(sim, sim.state.object(hq, false).unwrap()).unwrap()
    };
    let (sub_button, _) = m.clickable(|b| b.spec.msg == sub);
    // Distance between two clicks so that no double-click arises (O-224, O-225).
    let pause = |m: &mut Match| {
        for _ in 0..40 {
            m.frame();
        }
    };
    m.click(&sub_button).unwrap();
    pause(&mut m);
    let (upgrade, msg) = m.clickable(|b| b.spec.kind() == kinds::UPGRADE);
    let shown = |m: &Match| {
        m.report()
            .buttons
            .iter()
            .any(|b| b.element.starts_with("normal:") && b.msg == msg)
    };
    let queued = |m: &Match| {
        m.report()
            .buttons
            .iter()
            .filter(|b| b.element.starts_with("queue:"))
            .count()
    };
    m.click(&upgrade).unwrap();
    pause(&mut m);
    assert_eq!(queued(&m), 1, "upgrade runs");
    assert!(!shown(&m), "running: button hidden");

    // Pause on: no more ticks, commands take effect at once.
    assert!(m.session.as_mut().unwrap().toggle_pause());
    let steps = m.session().steps();
    m.frame();
    m.click("queue:0").unwrap();
    m.frame();
    assert_eq!(m.session().steps(), steps, "no tick in the pause");
    assert_eq!(queued(&m), 0, "cancelled at once");
    assert!(
        shown(&m),
        "cancelled in the pause: button back in the next frame"
    );

    // Resume: the tick delivers the event once more (second silent rebuild), the button stays.
    assert!(m.session.as_mut().unwrap().toggle_pause());
    pause(&mut m);
    assert!(m.session().steps() > steps, "runs again");
    assert!(shown(&m), "after resuming");
}

/// `ui_state.game.minimap`, `diplomacy_open` and `chat_lines`. The map reports visibility, mode, center of the diamond
/// `(W - 89, H - 89)`, rotation angle, its pings (tile, repetitions), the length of the list of idle workers of the
/// local slot and the number of fog swaps; the dialog `DIPLOMATIE` (state 10) and the active chat lines come from the
/// flow.
#[test]
fn ui_state_reports_minimap() {
    use opennml_ui::hud::screen_text::CHAT_MS;
    use opennml_ui::hud::toolbar::minimap_buttons::MSG_MODE_ECONOMY;

    let Some(mut m) = Match::start() else {
        return;
    };
    m.frame();
    m.frame();
    let report = m.session().minimap_state().expect("minimap in the report");
    assert!(report.visible);
    assert_eq!(report.mode, 0);
    assert_eq!(report.center, [1024 - 89, 768 - 89]);
    assert!(report.theta.is_finite());
    assert!(report.pings.is_empty());
    let sim = m.session().sim().unwrap();
    assert_eq!(
        report.idle_workers,
        sim.state.idle_workers.list(sim.active_player).len()
    );
    assert_eq!(
        report.fog_swaps,
        m.session().hud.minimap.as_ref().unwrap().fog().swaps()
    );

    // Mode (key 574 -> economy map), pings and hidden map.
    let vfs = &m.vfs;
    let session = m.session.as_mut().unwrap();
    session.minimap_button(MSG_MODE_ECONOMY, false, vfs, None);
    session.hud.ping((70, 101), 0xFF00_00FF, 10);
    session.hud.ping((3, 4), 0xFF00_FF00, 1);
    // A second message at the same tile creates no second ping (O-41).
    session.hud.ping((70, 101), 0xFFFF_0000, 3);
    let report = m.session().minimap_state().unwrap();
    assert_eq!(report.mode, 1);
    assert_eq!(report.pings, [(70, 101, 10), (3, 4, 1)]);
    fn map_mut(m: &mut Match) -> &mut crate::game::minimap::Minimap {
        m.session.as_mut().unwrap().hud.minimap.as_mut().unwrap()
    }
    map_mut(&mut m).set_visible(false);
    assert!(!m.session().minimap_state().unwrap().visible);
    map_mut(&mut m).set_visible(true);

    // Fog swap: a whole pass over all tile rows counts one; the number only grows.
    let before = m.session().minimap_state().unwrap().fog_swaps;
    for _ in 0..1500 {
        if m.session().minimap_state().unwrap().fog_swaps > before {
            break;
        }
        m.frame();
    }
    assert!(m.session().minimap_state().unwrap().fog_swaps > before);

    // Chat lines: only the active ones, as "Name: text"; the second one waits.
    let session = m.session.as_mut().unwrap();
    assert!(session.chat_lines().is_empty());
    session
        .hud
        .screen_texts
        .push_chat("Anna: Hello".into(), 0x00FF_FFFF, CHAT_MS, 768);
    session
        .hud
        .screen_texts
        .push_chat("Ben: waiting".into(), 0x00FF_FFFF, CHAT_MS, 768);
    assert_eq!(m.session().chat_lines(), ["Anna: Hello"]);

    // `game_data` brings the map, dialog state and chat into the report of `ui_state`.
    let game_data = |m: &Match| {
        m.flow
            .game_data(
                &m.host.app,
                m.session().sim(),
                m.session().level(),
                None,
                None,
                None,
                m.session().minimap_state(),
                m.session().chat_lines(),
            )
            .expect("report of the match")
    };
    let data = game_data(&m);
    assert!(!data.diplomacy_open);
    assert_eq!(data.minimap.as_ref().map(|x| x.mode), Some(1));
    // The flag query is the same function as `state.sim.flags` of the viewer (campaign map: no flags).
    let sim_state = &m.session().sim().expect("Simulation").state;
    assert_eq!(data.flags, crate::game::sim_host::flags_info(sim_state));
    assert_eq!(data.chat_lines, ["Anna: Hello"]);
    assert!(m.host.app.toggle_diplomacy(true), "dialog opens");
    let data = game_data(&m);
    assert_eq!(data.menu_state, 10);
    assert!(data.diplomacy_open);
    // Without a minimap node the part is missing.
    m.session.as_mut().unwrap().hud.minimap = None;
    assert_eq!(m.session().minimap_state(), None);
}

/// `ui_click` on `GAMEUI` reaches the minimap and its corner buttons (`minimap`, `minimap_mode`, `minimap_idle`,
/// `minimap_signal`, `minimap_idle_all`, `minimap_menu`, `minimap_diplomacy`; the names apply before the bar, also
/// without a command bar). The center of every button hits its rectangle, `minimap` the center of the diamond (camera
/// jump to the map center), the mode button switches the mode, 579 and 580 ask the flow for the game menu or the
/// diplomacy dialog.
#[test]
fn gameui_click_minimap_elements() {
    use opennml_ui::hud::toolbar::minimap_buttons::{
        MSG_DIPLOMACY_BUTTON, MSG_GAME_MENU, MSG_IDLE_ALL, MSG_IDLE_ONE, MSG_SIGNAL, mode_of,
    };

    let Some(mut m) = Match::start() else {
        return;
    };
    m.frame();
    m.frame();
    // The center of every corner button lies in the rectangle of the button.
    type Pick = fn(i32) -> bool;
    let rect_of = |m: &Match, pick: Pick| {
        m.session()
            .hud
            .minimap_buttons
            .buttons
            .iter()
            .find(|b| pick(b.spec.msg))
            .map(|b| b.spec.rect)
            .expect("corner button")
    };
    let cases: [(&str, Pick); 6] = [
        ("minimap_mode", |msg| mode_of(msg).is_some()),
        ("minimap_idle", |msg| msg == MSG_IDLE_ONE),
        ("minimap_signal", |msg| msg == MSG_SIGNAL),
        ("minimap_idle_all", |msg| msg == MSG_IDLE_ALL),
        ("minimap_menu", |msg| msg == MSG_GAME_MENU),
        ("minimap_diplomacy", |msg| msg == MSG_DIPLOMACY_BUTTON),
    ];
    for (name, pick) in cases {
        let (x, y) = m.session().gameui_center(name).unwrap();
        let [l, t, r, b] = rect_of(&m, pick);
        assert!(
            (f64::from(l)..f64::from(r)).contains(&x) && (f64::from(t)..f64::from(b)).contains(&y),
            "{name}: ({x}, {y}) in {:?}",
            [l, t, r, b]
        );
    }
    // Without a command bar the names of the minimap still apply; bar names do not.
    let toolbar = m.session.as_mut().unwrap().hud.toolbar.take();
    assert!(m.session().gameui_center("minimap_signal").is_ok());
    assert!(m.session().gameui_center("minimap").is_ok());
    assert_eq!(
        m.session().gameui_center("normal:0"),
        Err("element normal:0 does not exist in GAMEUI".to_string())
    );
    m.session.as_mut().unwrap().hud.toolbar = toolbar;
    assert_eq!(
        m.click("minimap_unknown"),
        Err("element minimap_unknown does not exist in GAMEUI".to_string())
    );

    // `minimap`: center of the diamond; the click jumps with the camera to the map center.
    let (cx, cy) = m.click("minimap").unwrap();
    assert_eq!((cx, cy), (1024 - 89, 768 - 89));
    m.frame();
    let session = m.session();
    let sim = session.sim().unwrap();
    let cell = sim.rules.map.cell;
    let (dim_x, dim_z) = {
        let terrain = session.hud.minimap.as_ref().unwrap().terrain();
        (terrain.dim_x as f32, terrain.dim_z as f32)
    };
    let target = session.map.camera.target;
    assert!(
        (target.x - cell[0] as f32 * dim_x / 2.0).abs() < 2.0 * cell[0] as f32
            && (target.z - cell[2] as f32 * dim_z / 2.0).abs() < 2.0 * cell[2] as f32,
        "camera at the map center: {target:?}"
    );

    // Mode button: 0 -> 1 (the button carried 574), the corner buttons rebuild.
    assert_eq!(m.session().hud.minimap.as_ref().unwrap().mode, 0);
    m.click("minimap_mode").unwrap();
    assert_eq!(m.session().hud.minimap.as_ref().unwrap().mode, 1);
    m.frame();
    assert_eq!(m.session().minimap_state().map(|s| s.mode), Some(1));
    // Signal (571): a command at the next tick boundary.
    m.click("minimap_signal").unwrap();
    assert!(
        m.session()
            .sim()
            .unwrap()
            .pending_commands()
            .iter()
            .any(|c| matches!(c.kind, CommandKind::Signal { .. })),
        "signal goes out as a command"
    );
    // Diplomacy (580) and game menu (579) go as requests to the flow, which executes them at once
    // (`GameFlow::input` fetches them after every input): state 10 or 1. A second click without time in between does
    // not toggle back: with an open menu `GameFlow::input` lets neither press nor release through.
    m.now += 10_000;
    m.click("minimap_diplomacy").unwrap();
    assert_eq!(m.host.app.game_menu_state(), 10);
    m.click("minimap_diplomacy").unwrap();
    assert_eq!(
        m.host.app.game_menu_state(),
        10,
        "the second click hits the open menu"
    );
    assert!(m.host.app.toggle_diplomacy(false), "Back closes");
    assert_eq!(m.host.app.game_menu_state(), 0);
    m.now += 10_000;
    m.click("minimap_menu").unwrap();
    assert_eq!(m.host.app.game_menu_state(), 1);
    m.click("minimap_menu").unwrap();
    assert_eq!(m.host.app.game_menu_state(), 1, "579 also only once");
}

/// The names of the minimap work with a hidden command bar (`{0x11, 1}`, the buttons take the click but have no
/// effect, O-292); the button 580 does not exist in the campaign ("does not exist in GAMEUI"), the other names
/// stay.
#[test]
fn minimap_elements_with_hidden_toolbar_and_in_the_campaign() {
    let Some(mut m) = Match::start() else {
        return;
    };
    m.frame();
    m.frame();
    m.session
        .as_mut()
        .unwrap()
        .hud
        .toolbar
        .as_mut()
        .unwrap()
        .shown = false;
    for name in [
        "minimap",
        "minimap_mode",
        "minimap_idle",
        "minimap_signal",
        "minimap_idle_all",
        "minimap_menu",
        "minimap_diplomacy",
    ] {
        assert!(
            m.session().gameui_center(name).is_ok(),
            "{name} with a hidden bar"
        );
    }
    // Bar names then do not work.
    assert_eq!(
        m.session().gameui_center("toolbar"),
        Err("command bar is hidden".to_string())
    );
    m.session
        .as_mut()
        .unwrap()
        .hud
        .toolbar
        .as_mut()
        .unwrap()
        .shown = true;
    // Campaign (game type -1): without button 580.
    m.session.as_mut().unwrap().hud.set_game_type(-1);
    assert_eq!(
        m.session().gameui_center("minimap_diplomacy"),
        Err("element minimap_diplomacy does not exist in GAMEUI".to_string())
    );
    assert!(m.session().gameui_center("minimap_menu").is_ok());
}
