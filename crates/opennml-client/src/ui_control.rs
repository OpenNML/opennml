//! Commands of the menu UI on the control channel (`ui_state`, `ui_click`, `ui_key`, `ui_open` and `ui_video`) on a
//! [`MenuApp`]. Viewer (`--dialog` in the window) and `opennml-game` call [`execute_ui`] in the window thread (`MenuApp` is
//! not `Send`); without a UI they answer with [`NO_UI`]. GPU-free.
//!
//! The commands act only through the input of the next frame or the queue of the UI, like mouse, keyboard and command
//! 5 - the simulation stays untouched. Description: `docs/viewer-control.md`, section "UI (`ui_*`)".
//!
//! Menu dialogs only: `opennml-game` intercepts `ui_click` on `GAMEUI` (command bar of the match) beforehand
//! ([`crate::game_flow::GameFlow::gameui_click`]); [`execute_ui`] stays menu-only.

use opennml_control::{
    Command, MouseButtonName, UiAccountingRow, UiDialogData, UiEffectData, UiEffectKind,
    UiElementData, UiSlotData, UiStateData, UiVideoData,
};
use opennml_ui::app::MenuApp;
use opennml_ui::coords::Screen;
use opennml_ui::input::{MouseButton, UiInput};
use opennml_ui::keys::{parse_chord, vk_name};
use opennml_ui::msg::{MenuLevelEffect, UiEffect};
use opennml_ui::screens::DiplomacyAction;
use serde_json::{Value, json};

/// Error message when no UI is running (viewer without `--dialog` or without a window).
pub const NO_UI: &str = "no UI open";

/// Is `command` a UI command ([`execute_ui`])?
pub fn is_ui_command(command: &Command) -> bool {
    matches!(
        command,
        Command::UiState
            | Command::UiClick { .. }
            | Command::UiKey { .. }
            | Command::UiOpen { .. }
            | Command::UiVideo { .. }
    )
}

/// Executes a UI command on `app` (screen `screen`, UI clock `now`):
/// - `ui_state` -> [`UiStateData`] ([`state_data`]);
/// - `ui_click` -> center of the element (`MenuApp::element_center`: unknown or not open dialog, unknown, hidden, not
///   drawn or out-of-screen element -> error message), then `Move`, `Down`, `Up` into the input of the next frame -
///   exactly like the mouse by hand: locked elements do nothing, during the flight-in triggering is refused (O-21), two
///   clicks within `DoubleClickTimeout` are a double-click (`MenuApp::input`, re/menu-system 12.8). Response
///   `{"x", "y"}`;
/// - `ui_key` -> [`chord_codes`], a key event with the keys and the character. Response `{"vk"}`;
/// - `ui_open` -> `MenuApp::open_by_name` (unknown -> message with the known names). Response `null`;
/// - `ui_video` -> `MenuApp::play_video_by_name` (probe video without a campaign: enqueues `CloseAll`, `WaitDialogs`,
///   `PlayVideo`, `WaitVideo`, `AfterProbeVideo` and the reopening of the open menus, without open menus only the
///   middle three; booking and `0x28` go only through `VIDEOSTART`;
///   invalid name or busy UI - transition, waiting commands, video -> error message as the error response).
///   Response `null`.
pub fn execute_ui(
    app: &mut MenuApp,
    command: &Command,
    screen: Screen,
    now: u64,
) -> Result<Value, String> {
    match command {
        Command::UiState => serde_json::to_value(state_data(app, screen, now))
            .map_err(|e| format!("state not serializable: {e}")),
        Command::UiClick {
            dialog,
            element,
            button,
        } => {
            let (x, y) = app.element_center(dialog, element, screen)?;
            let b = match button.unwrap_or(MouseButtonName::Left) {
                MouseButtonName::Left => MouseButton::Left,
                MouseButtonName::Right => MouseButton::Right,
                // `Command::validate` already rejects this (O-331).
                MouseButtonName::Middle => return Err(opennml_control::UI_CLICK_MIDDLE.to_string()),
            };
            app.input(UiInput::Move { x, y }, screen, now);
            app.input(UiInput::Down(b), screen, now);
            app.input(UiInput::Up(b), screen, now);
            Ok(json!({"x": x, "y": y}))
        }
        Command::UiKey { chord, char } => {
            let held = chord_codes(chord)?;
            app.input(
                UiInput::Key {
                    held: held.clone(),
                    ch: *char,
                },
                screen,
                now,
            );
            Ok(json!({ "vk": held }))
        }
        Command::UiOpen { dialog } => {
            app.open_by_name(dialog)?;
            Ok(Value::Null)
        }
        Command::UiVideo { name } => {
            app.play_video_by_name(name)?;
            Ok(Value::Null)
        }
        _ => {
            Err("not a UI command (expected: ui_state, ui_click, ui_key, ui_open, ui_video)".into())
        }
    }
}

/// State of the UI as a response (`MenuApp::report`, lossless image); `game` stays empty, `opennml-game` sets it from the
/// flow of the match. The accounting (`accounting`, `play_time`) comes from the UI (`MenuData::accounting`, stored at
/// `0x0E` when the match has already ended) and appears in the report only while the frame `accounting` is open - the
/// table itself stays until the next accounting.
pub fn state_data(app: &MenuApp, screen: Screen, now: u64) -> UiStateData {
    let report = app.report(screen, now);
    let accounting = report
        .dialogs
        .iter()
        .any(|d| d.name == ACCOUNTING)
        .then_some(app.data().accounting.as_ref())
        .flatten();
    UiStateData {
        now_ms: now,
        screen: [screen.width, screen.height],
        dialogs: report
            .dialogs
            .into_iter()
            .map(|d| UiDialogData {
                name: d.name,
                rect: d.rect,
                animating: d.animating,
                modal: d.modal,
            })
            .collect(),
        focus: report.focus,
        hover: report.hover,
        elements: report
            .elements
            .into_iter()
            .map(|e| UiElementData {
                dialog: e.dialog,
                name: e.name,
                kind: e.kind.to_string(),
                rect: e.rect,
                visible: e.visible,
                enabled: e.enabled,
                hidden: e.hidden,
                checked: e.checked,
                value: e.value,
                selection: e.selection,
                items: e.items,
                text: e.text,
            })
            .collect(),
        images: report.images,
        busy: report.busy,
        queued: report.queued,
        effects: report
            .effects
            .into_iter()
            .map(|(at_ms, effect)| UiEffectData {
                at_ms,
                effect: effect_kind(effect),
            })
            .collect(),
        video: report.video.map(|v| UiVideoData {
            path: v.path,
            frame: v.frame,
            frames: v.frames,
            abortable: v.abortable,
        }),
        pointer_hidden: report.pointer_hidden,
        // Only the host knows the match (`GameFlow::game_data`); the UI alone has none.
        game: None,
        menu_level: None,
        accounting: accounting.map(|view| {
            view.rows
                .iter()
                .map(|(name, color, values)| UiAccountingRow {
                    name: name.clone(),
                    color: *color,
                    values: values.clone(),
                })
                .collect()
        }),
        play_time: accounting.map(|view| view.time_text.clone()),
        pointer_model: None,
    }
}

/// Frame of the accounting (`ACCOUNTING`, re/accounting 5).
const ACCOUNTING: &str = "accounting";

fn effect_kind(effect: UiEffect) -> UiEffectKind {
    match effect {
        UiEffect::PlaySfx {
            event,
            flags,
            category,
        } => UiEffectKind::PlaySfx {
            event,
            flags,
            category,
        },
        UiEffect::PlaySpeech { path } => UiEffectKind::PlaySpeech { path },
        UiEffect::MenuMusic { path, looping } => UiEffectKind::MenuMusic { path, looping },
        UiEffect::RestartMusic => UiEffectKind::RestartMusic,
        UiEffect::SetSoundDriver(index) => UiEffectKind::SetSoundDriver { index },
        UiEffect::SetVolume { category, value } => UiEffectKind::SetVolume { category, value },
        UiEffect::SetGamma(gamma) => UiEffectKind::SetGamma { gamma },
        UiEffect::SetHardwareCursor(on) => UiEffectKind::SetHardwareCursor { on },
        UiEffect::SetDisplayMode(m) => UiEffectKind::SetDisplayMode {
            width: m.width,
            height: m.height,
            bits: m.bits,
        },
        UiEffect::WriteSettings => UiEffectKind::WriteSettings,
        UiEffect::WriteProfiles => UiEffectKind::WriteProfiles,
        UiEffect::DeleteProfileSaves { guid } => UiEffectKind::DeleteProfileSaves { guid },
        UiEffect::Quit => UiEffectKind::Quit,
        // Random map: the map name of the match `randommap:<seed>` instead of the path of the last selected fixed map,
        // which `Info` keeps with `LevelType 1`.
        UiEffect::StartMatch(req) => UiEffectKind::StartMatch {
            level: crate::match_request::random_map_spec(&req)
                .map_or(req.level, |spec| opennml_randommap::random_map_label(&spec)),
            game_type: req.game_type,
            slots: req
                .slots
                .iter()
                .map(|s| UiSlotData {
                    kind: s.kind,
                    nation: s.nation,
                    team: s.team,
                    color: s.color,
                    handicap: s.handicap,
                })
                .collect(),
        },
        UiEffect::ResumeGame { game_speed } => UiEffectKind::ResumeGame { game_speed },
        UiEffect::ReloadLevel => UiEffectKind::ReloadLevel,
        UiEffect::LoadGame { name } => UiEffectKind::LoadGame { name },
        UiEffect::SaveGame { name } => UiEffectKind::SaveGame { name },
        UiEffect::DeleteSave { name, .. } => UiEffectKind::DeleteSave { name },
        UiEffect::PlayReplay { name } => UiEffectKind::PlayReplay { name },
        UiEffect::GameEnd { victory } => UiEffectKind::GameEnd { victory },
        UiEffect::EndMatch => UiEffectKind::EndMatch,
        UiEffect::Surrender { slot } => UiEffectKind::Surrender { slot },
        UiEffect::StartCampaignLevel { level, difficulty } => {
            UiEffectKind::StartCampaignLevel { level, difficulty }
        }
        UiEffect::BookCampaign { won } => UiEffectKind::BookCampaign { won },
        UiEffect::Diplomacy(action) => diplomacy_kind(action),
        UiEffect::SendChat { window, text } => UiEffectKind::SendChat { window, text },
        UiEffect::MenuLevel(MenuLevelEffect::Load { path }) => {
            UiEffectKind::MenuLevel { load: Some(path) }
        }
        UiEffect::MenuLevel(MenuLevelEffect::Hide) => UiEffectKind::MenuLevel { load: None },
        UiEffect::Unhandled(cmd) => UiEffectKind::Unhandled {
            code: cmd.code(),
            command: format!("{cmd:?}"),
        },
        UiEffect::Net(cmd) => UiEffectKind::Net {
            code: cmd.code(),
            command: format!("{cmd:?}"),
        },
        UiEffect::Search(on) => UiEffectKind::Search { on },
        UiEffect::Lobby(action) => UiEffectKind::Lobby {
            action: format!("{action:?}"),
        },
        UiEffect::PlayVideo { path, abortable } => UiEffectKind::PlayVideo { path, abortable },
        UiEffect::StopVideo { path } => UiEffectKind::StopVideo { path },
    }
}

/// Fixed form of the diplomacy effect on the control channel (`docs/viewer-control.md`): `action` in lowercase with an
/// underscore, plus `slot` and `value` where they apply. A bit is 1 (on) or 0 (off).
fn diplomacy_kind(action: DiplomacyAction) -> UiEffectKind {
    let (name, slot, value) = match action {
        DiplomacyAction::Status { slot, mode } => ("status", Some(slot), Some(mode)),
        DiplomacyAction::View { slot, on } => ("view", Some(slot), Some(i32::from(on))),
        DiplomacyAction::Chat { slot, on } => ("chat", Some(slot), Some(i32::from(on))),
        DiplomacyAction::Tribute { slot, on } => ("tribute", Some(slot), Some(i32::from(on))),
        DiplomacyAction::ChatBy { relation } => ("chat_by", None, Some(relation)),
        DiplomacyAction::SendTribute { resource } => ("send_tribute", None, Some(resource)),
        DiplomacyAction::Close => ("close", None, None),
    };
    UiEffectKind::Diplomacy {
        action: name.to_string(),
        slot,
        value,
    }
}

/// Key names that can be read back: `parse_chord(name)` hits exactly its own key. This drops the `0xNN` names of
/// unnamed keys (`parse_chord` writes them as `0XNN` in uppercase and never finds them, O-107).
pub fn chord_key_names() -> Vec<&'static str> {
    (1..=255u8)
        .filter(|&vk| parse_chord(vk_name(vk)) == [vk])
        .map(vk_name)
        .collect()
}

/// Keys of a combination in press order like `FUN_0049C050` (`parse_chord`: trimmed, uppercased, split at spaces;
/// "left" hits `LEFT`, re/menu-system 12.7), each key once (like the table of held keys). A word without a hit
/// (code 0) is an error message with the allowed names ([`chord_key_names`]).
pub fn chord_codes(chord: &str) -> Result<Vec<u8>, String> {
    let codes = parse_chord(chord);
    if codes.contains(&0) {
        let unknown: Vec<&str> = chord
            .split_whitespace()
            .filter(|w| parse_chord(w) == [0])
            .collect();
        let what = if unknown.is_empty() {
            format!("\"{chord}\"")
        } else {
            format!("\"{}\"", unknown.join("\", \""))
        };
        return Err(format!(
            "unknown key name {what} in \"{chord}\" (allowed, separated by spaces: {})",
            chord_key_names().join(", ")
        ));
    }
    let mut held = Vec::with_capacity(codes.len());
    for vk in codes {
        if !held.contains(&vk) {
            held.push(vk);
        }
    }
    Ok(held)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{InputEvent, MouseButton as HostButton};
    use crate::menu_host::MenuHost;
    use crate::testutil::test_config;
    use crate::testutil::{
        FakeAudio, FakeVideoLog, LOCALISATION, fake_opener, fnt, install, tga, utf16,
        world as host_world,
    };
    use crate::ui_layer::UiLayer;
    use opennml_formats::engine_ini::EngineConfig;
    use opennml_rda::Vfs;
    use opennml_ui::app::MenuOptions;
    use std::cell::RefCell;
    use std::rc::Rc;

    const S1024: Screen = Screen {
        width: 1024,
        height: 768,
    };

    /// UI with clock and screen 1024 x 768; frames of 16 ms.
    struct Ui {
        vfs: Vfs,
        layer: UiLayer,
        app: MenuApp,
        now: u64,
        _dir: Option<tempfile::TempDir>,
    }

    impl Ui {
        fn new(vfs: Vfs, dir: Option<tempfile::TempDir>, start_menu: bool) -> Ui {
            let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
            let config = EngineConfig {
                is_intro: false,
                ..test_config(&vfs)
            };
            let opts = MenuOptions {
                start_menu,
                ..MenuOptions::default()
            };
            let app = MenuApp::new(&vfs, &layer.texts, config, opts).unwrap();
            Ui {
                vfs,
                layer,
                app,
                now: 0,
                _dir: dir,
            }
        }

        /// Installation from `NML_GAME_DIR`; without the variable `None` (the test silently skips itself).
        fn corpus(start_menu: bool) -> Option<Ui> {
            let dir = opennml_rda::testutil::corpus_game_dir()?;
            Some(Ui::new(Vfs::open_game_dir(&dir).unwrap(), None, start_menu))
        }

        fn run(&mut self, command: Command) -> Result<Value, String> {
            command.validate()?;
            execute_ui(&mut self.app, &command, S1024, self.now)
        }

        fn frame(&mut self) {
            let mut ctx = self.layer.context(&self.vfs, S1024);
            self.app.frame(&mut ctx, self.now);
            self.now += 16;
        }

        /// Frames until stack and queue are at rest (at most 3 s), and one more.
        fn idle(&mut self) {
            let until = self.now + 3000;
            self.frame();
            while self.app.menu().busy()
                || !self.app.queue().is_empty()
                || self.app.queue().waiting()
            {
                assert!(self.now < until, "UI does not come to rest");
                self.frame();
            }
            self.frame();
        }

        fn dialogs(&self) -> Vec<String> {
            state_data(&self.app, S1024, self.now)
                .dialogs
                .into_iter()
                .map(|d| d.name)
                .collect()
        }
    }

    /// Menu host on the installation from `NML_GAME_DIR` like [`Ui::new`] (main menu, without introduction), sound
    /// fake, settings in memory only; frames of 16 ms through `MenuHost::frame_cmds` (with `HeldVk::begin_round`).
    struct HostUi {
        vfs: Vfs,
        host: MenuHost,
        now: u64,
    }

    impl HostUi {
        fn corpus() -> Option<HostUi> {
            let dir = opennml_rda::testutil::corpus_game_dir()?;
            let vfs = Vfs::open_game_dir(&dir).unwrap();
            let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
            let config = EngineConfig {
                is_intro: false,
                ..test_config(&vfs)
            };
            let opts = MenuOptions {
                start_menu: true,
                ..MenuOptions::default()
            };
            let host =
                MenuHost::new(&vfs, layer, config, opts, &FakeAudio::default(), None).unwrap();
            Some(HostUi { vfs, host, now: 0 })
        }

        fn run(&mut self, command: Command) -> Result<Value, String> {
            command.validate()?;
            execute_ui(&mut self.host.app, &command, S1024, self.now)
        }

        fn frame(&mut self) {
            self.host.frame_cmds(&self.vfs, S1024, self.now);
            self.now += 16;
        }

        /// Frames until stack and queue are at rest (at most 3 s), and one more.
        fn idle(&mut self) {
            let until = self.now + 3000;
            self.frame();
            let busy = |h: &HostUi| {
                let app = &h.host.app;
                app.menu().busy() || !app.queue().is_empty() || app.queue().waiting()
            };
            while busy(self) {
                assert!(self.now < until, "UI does not come to rest");
                self.frame();
            }
            self.frame();
        }
    }

    /// Number of `SFX_KLICK` in the effect log of the report.
    fn klicks(app: &MenuApp, now: u64) -> usize {
        state_data(app, S1024, now)
            .effects
            .iter()
            .filter(|e| matches!(&e.effect, UiEffectKind::PlaySfx { event, .. } if event == "SFX_KLICK"))
            .count()
    }

    fn click(dialog: &str, element: &str) -> Command {
        Command::UiClick {
            dialog: dialog.into(),
            element: element.into(),
            button: None,
        }
    }

    fn key(chord: &str) -> Command {
        Command::UiKey {
            chord: chord.into(),
            char: None,
        }
    }

    /// Small installation with dialog `T`: `OK`, hidden `HID`, `FAR` to the right outside the grid.
    fn world() -> Ui {
        let button = |name: &str, flags: &str, extent: &str| {
            format!(
                "\tBUTTON {name}\n\t\tFLAGS MENUBUTTON_TEXT {flags} /FLAGS\n\t\tTEXT 1\n\t\tEXTENT {extent}\n\t/BUTTON\n"
            )
        };
        let dlg = format!(
            "FRAME T\n\tFLAGS MENUFRAME_NOFRAME /FLAGS\n\tEXTENT 0 0 1280 1024\n{}{}{}/FRAME\n",
            button("OK", "", "100 100 300 200"),
            button("HID", "HIDE", "100 300 300 400"),
            button("FAR", "", "1400 100 1500 200"),
        );
        let (dir, vfs) = install(&[
            (
                "data/txt/localisation.deu",
                LOCALISATION.as_bytes().to_vec(),
            ),
            ("data/txt/t.txt", utf16("1=OK\r\n")),
            ("data/fonts/Test_21_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_21_0_0_0_0.tga", tga(256, 256)),
            (
                "data/menü/dialogs.map",
                b"T data/men\xFC/t.dlg\r\n".to_vec(),
            ),
            ("data/menü/t.dlg", dlg.into_bytes()),
        ]);
        let mut ui = Ui::new(vfs, Some(dir), false);
        ui.run(Command::UiOpen { dialog: "t".into() }).unwrap();
        ui.idle();
        ui
    }

    #[test]
    fn ui_click_on_unknown_names_is_a_german_error() {
        let mut ui = world();
        assert_eq!(ui.dialogs(), ["t"]);
        let err = |ui: &mut Ui, d: &str, e: &str| ui.run(click(d, e)).unwrap_err();
        assert_eq!(
            err(&mut ui, "options", "ok"),
            "dialog \"options\" is not open (open: t)"
        );
        let unknown = err(&mut ui, "t", "nothing");
        assert!(
            unknown.starts_with("element \"nothing\" does not exist in dialog \"t\" (known: "),
            "{unknown}"
        );
        assert!(unknown.contains("ok"), "{unknown}");
        assert_eq!(err(&mut ui, "t", "hid"), "element \"t/hid\" is hidden");
        let far = err(&mut ui, "t", "far");
        assert!(
            far == "element \"t/far\" is outside the screen"
                || far == "element \"t/far\" was not drawn in the last frame",
            "{far}"
        );
        // No error left an input behind: the next frame does nothing.
        ui.frame();
        let state = state_data(&ui.app, S1024, ui.now);
        assert!(state.effects.is_empty(), "{:?}", state.effects);
        assert_eq!(state.hover, None);
        // Case does not matter; the click on `OK` hits (center in screen pixels).
        let at = ui.run(click("T", "OK")).unwrap();
        assert_eq!(at, json!({"x": 160, "y": 112}));
    }

    #[test]
    fn ui_state_reports_the_running_video() {
        // `ui_video t` in the host with a fake video (three frames of 40 ms) above the open dialog `test`: modeled on
        // `videostart` (`FUN_004DA8A0` `0x004DAA14`...) `CloseAll`, `WaitDialogs`, `PlayVideo`, `WaitVideo`,
        // `AfterProbeVideo`, afterwards `test` opens again.
        let (_dir, vfs) = host_world();
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let config = EngineConfig {
            is_intro: false,
            ..test_config(&vfs)
        };
        let mut audio = FakeAudio::default();
        let mut host =
            MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        host.set_video_opener(fake_opener(&log, false, None));
        let mut step = |host: &mut MenuHost, now: u64| {
            host.frame_cmds(&vfs, S1024, now);
            host.apply_effects(&mut audio, &vfs);
            state_data(&host.app, S1024, now)
        };
        let names = |state: &UiStateData| -> Vec<String> {
            state.dialogs.iter().map(|d| d.name.clone()).collect()
        };

        // Before the command: `test` open, no video, pointer on.
        value_of(
            &mut host,
            Command::UiOpen {
                dialog: "test".into(),
            },
            0,
        )
        .unwrap();
        let mut now = 0;
        let mut before = step(&mut host, now);
        while before.busy || before.queued > 0 {
            now += 16;
            assert!(now < 500, "test does not open");
            before = step(&mut host, now);
        }
        assert_eq!(names(&before), ["test"]);
        assert_eq!((before.video.clone(), before.pointer_hidden), (None, false));
        let value = value_of(&mut host, Command::UiVideo { name: "t".into() }, now);
        assert_eq!(value, Ok(Value::Null));
        assert_eq!(host.app.queue().len(), 6);

        // First the menus close, then the video; as long as it runs, no dialog is open.
        let mut state = before;
        while state.video.is_none() {
            now += 16;
            assert!(now < 1000, "video does not start");
            state = step(&mut host, now);
        }
        let started = host.app.video().unwrap().started_ms;
        while now < started + 50 {
            now += 16;
            state = step(&mut host, now);
            assert!(state.dialogs.is_empty(), "{now} ms: {:?}", names(&state));
        }
        assert_eq!(
            state.video,
            Some(UiVideoData {
                path: "data/videos/t.bik".into(),
                frame: 1,
                frames: 3,
                abortable: true,
            })
        );
        assert!(state.pointer_hidden, "pointer off during the video");
        assert!(
            state.effects.iter().any(|e| e.effect
                == UiEffectKind::PlayVideo {
                    path: "data/videos/t.bik".into(),
                    abortable: true
                }),
            "{:?}",
            state.effects
        );
        // As JSON: `video` with all four fields, `pointer_hidden` and effect `play_video`.
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json["video"],
            json!({"path": "data/videos/t.bik", "frame": 1, "frames": 3, "abortable": true})
        );
        assert_eq!(json["pointer_hidden"], json!(true));
        // A second `ui_video` during the video: error response, nothing enqueued.
        let queued = host.app.queue().len();
        let busy = value_of(&mut host, Command::UiVideo { name: "t".into() }, now);
        assert!(busy.as_ref().is_err_and(|e| e.contains("busy")), "{busy:?}");
        assert_eq!(host.app.queue().len(), queued);

        // End after 3 * 40 ms: `stop_video`, no video any more; `AfterProbeVideo` switches the pointer on again, `test`
        // is open again.
        while !host.app.queue().is_empty() || host.app.queue().waiting() || state.busy {
            now += 16;
            assert!(now < started + 1000, "sequence does not end");
            state = step(&mut host, now);
        }
        assert_eq!((state.video.clone(), state.pointer_hidden), (None, false));
        assert_eq!(names(&state), ["test"]);
        assert!(
            state.effects.iter().any(|e| e.effect
                == UiEffectKind::StopVideo {
                    path: "data/videos/t.bik".into()
                }),
            "{:?}",
            state.effects
        );
        // Invalid names: error message, nothing enqueued.
        let bad = value_of(&mut host, Command::UiVideo { name: "a/b".into() }, now);
        assert!(bad.is_err(), "{bad:?}");
        assert_eq!(host.app.queue().len(), 0);
    }

    /// `execute_ui` on the host (after `Command::validate`).
    fn value_of(host: &mut MenuHost, command: Command, now: u64) -> Result<Value, String> {
        command.validate()?;
        execute_ui(&mut host.app, &command, S1024, now)
    }

    #[test]
    fn start_match_reports_level_game_type_and_slots() {
        // `UiEffect::StartMatch` (command `0x0D`) appears on the control channel as `start_match` with the occupied
        // slots.
        use opennml_ui::draft::{MatchRequest, SlotDraft};
        let slot = SlotDraft {
            key: -1,
            name: "Anna".into(),
            kind: 999,
            nation: 2,
            team: 1,
            handicap: 3,
            color: 4,
        };
        let req = MatchRequest {
            level: "data/levels/multiplayer/x.lvl".into(),
            game_type: 3,
            slots: vec![slot],
            settings: Default::default(),
            info: Default::default(),
        };
        assert_eq!(
            effect_kind(UiEffect::StartMatch(req)),
            UiEffectKind::StartMatch {
                level: "data/levels/multiplayer/x.lvl".into(),
                game_type: 3,
                slots: vec![UiSlotData {
                    kind: 999,
                    nation: 2,
                    team: 1,
                    color: 4,
                    handicap: 3,
                }],
            }
        );
        // With a random map (`LevelType 1`) `start_match` reports the map name of the match `randommap:<seed>` instead
        // of the leftover path of the last fixed map.
        let req = MatchRequest {
            level: "data/levels/multiplayer/x.lvl".into(),
            game_type: 0,
            slots: Vec::new(),
            settings: Default::default(),
            info: opennml_ui::draft::LevelInfoData {
                name: "41".into(),
                players: 2,
                level_type: 1,
                ..Default::default()
            },
        };
        assert_eq!(
            effect_kind(UiEffect::StartMatch(req)),
            UiEffectKind::StartMatch {
                level: "randommap:41".into(),
                game_type: 0,
                slots: Vec::new(),
            }
        );
    }

    #[test]
    fn resume_and_reload_reach_the_control_channel() {
        // `0x0F`/`0x37` -> `resume_game` (with or without speed), `0x1F` -> `reload_level`.
        assert_eq!(
            effect_kind(UiEffect::ResumeGame {
                game_speed: Some(14)
            }),
            UiEffectKind::ResumeGame {
                game_speed: Some(14)
            }
        );
        assert_eq!(
            effect_kind(UiEffect::ResumeGame { game_speed: None }),
            UiEffectKind::ResumeGame { game_speed: None }
        );
        assert_eq!(
            effect_kind(UiEffect::ReloadLevel),
            UiEffectKind::ReloadLevel
        );
    }

    #[test]
    fn menu_level_effects_reach_the_control_channel() {
        // `0x21` -> `menu_level` with a path, `0x20` -> `menu_level` without.
        use opennml_ui::msg::MenuLevelEffect;
        let path = "data/levels/menülevel/menü2.lvl".to_string();
        assert_eq!(
            effect_kind(UiEffect::MenuLevel(MenuLevelEffect::Load {
                path: path.clone()
            })),
            UiEffectKind::MenuLevel { load: Some(path) }
        );
        assert_eq!(
            effect_kind(UiEffect::MenuLevel(MenuLevelEffect::Hide)),
            UiEffectKind::MenuLevel { load: None }
        );
    }

    #[test]
    fn save_load_effects_reach_the_control_channel() {
        // `0x0A` -> `load_game`, `0x0B` -> `save_game`, `0x2A` -> `delete_save`, each with the name.
        let name = || "quicksave1".to_string();
        assert_eq!(
            effect_kind(UiEffect::LoadGame { name: name() }),
            UiEffectKind::LoadGame { name: name() }
        );
        assert_eq!(
            effect_kind(UiEffect::SaveGame { name: name() }),
            UiEffectKind::SaveGame { name: name() }
        );
        assert_eq!(
            effect_kind(UiEffect::DeleteSave {
                name: name(),
                dir: opennml_ui::saves::SaveDir::Main
            }),
            UiEffectKind::DeleteSave { name: name() }
        );
        // `0x19` -> `play_replay` with the full file name of the recording.
        assert_eq!(
            effect_kind(UiEffect::PlayReplay {
                name: "261108-1435".to_string()
            }),
            UiEffectKind::PlayReplay {
                name: "261108-1435".to_string()
            }
        );
    }

    /// The effect of the diplomacy dialog has a fixed form on the control channel (not the debug representation of the
    /// action): `action` in lowercase, `slot` and `value` where they apply; bits as 1/0.
    #[test]
    fn diplomacy_effects_have_a_fixed_form_on_the_control_channel() {
        let kind = |action| effect_kind(UiEffect::Diplomacy(action));
        let form = |action: &str, slot, value| UiEffectKind::Diplomacy {
            action: action.into(),
            slot,
            value,
        };
        assert_eq!(
            kind(DiplomacyAction::Status { slot: 3, mode: 1 }),
            form("status", Some(3), Some(1))
        );
        assert_eq!(
            kind(DiplomacyAction::View { slot: 2, on: true }),
            form("view", Some(2), Some(1))
        );
        assert_eq!(
            kind(DiplomacyAction::Chat { slot: 7, on: false }),
            form("chat", Some(7), Some(0))
        );
        assert_eq!(
            kind(DiplomacyAction::Tribute { slot: 0, on: true }),
            form("tribute", Some(0), Some(1))
        );
        assert_eq!(
            kind(DiplomacyAction::ChatBy { relation: -1 }),
            form("chat_by", None, Some(-1))
        );
        assert_eq!(
            kind(DiplomacyAction::SendTribute { resource: 2 }),
            form("send_tribute", None, Some(2))
        );
        assert_eq!(kind(DiplomacyAction::Close), form("close", None, None));
    }

    #[test]
    fn game_end_effects_reach_the_control_channel() {
        // End screen -> `game_end`, `0x0E` -> `accounting`, `0x27` -> `surrender`;
        assert_eq!(
            effect_kind(UiEffect::GameEnd { victory: false }),
            UiEffectKind::GameEnd { victory: false }
        );
        assert_eq!(effect_kind(UiEffect::EndMatch), UiEffectKind::EndMatch);
        assert_eq!(
            effect_kind(UiEffect::Surrender { slot: 3 }),
            UiEffectKind::Surrender { slot: 3 }
        );
        // `0x0C` -> `start_campaign_level`.
        let level = || "data/levels/kampagne1/kamp1-1/kamp1-1.lvl".to_string();
        assert_eq!(
            effect_kind(UiEffect::StartCampaignLevel {
                level: level(),
                difficulty: 2
            }),
            UiEffectKind::StartCampaignLevel {
                level: level(),
                difficulty: 2
            }
        );
        // `0x1C` or game end in the campaign -> `book_campaign`.
        assert_eq!(
            effect_kind(UiEffect::BookCampaign { won: true }),
            UiEffectKind::BookCampaign { won: true }
        );
    }

    #[test]
    fn ui_open_unknown_lists_known_names() {
        let mut ui = world();
        let e = ui
            .run(Command::UiOpen {
                dialog: "nonexistent".into(),
            })
            .unwrap_err();
        assert_eq!(e, "unknown dialog \"nonexistent\" (known: T)");
        assert_eq!(ui.app.queue().len(), 0, "nothing enqueued");
        assert_eq!(
            ui.run(Command::UiOpen { dialog: "T".into() }),
            Ok(Value::Null)
        );
        assert_eq!(ui.app.queue().len(), 1);
        // Not a UI command: error instead of a silent effect.
        assert!(ui.run(Command::State).is_err());
        assert!(is_ui_command(&Command::UiState) && !is_ui_command(&Command::State));
    }

    // QUIRK O-107: kept
    #[test]
    fn ui_key_names_follow_parse_chord_and_never_offer_hex() {
        assert_eq!(chord_codes("escape"), Ok(vec![0x1B]));
        assert_eq!(chord_codes(" Control  h "), Ok(vec![0x11, 0x48]));
        assert_eq!(chord_codes("SHIFT SHIFT"), Ok(vec![0x10]), "each key once");
        let names = chord_key_names();
        assert!(names.contains(&"ESCAPE") && names.contains(&"LEFT"));
        assert!(names.iter().all(|n| !n.starts_with("0x")), "O-107");
        // `0xNN` cannot be read back and is therefore unknown (O-107).
        for bad in ["ESCAP", "0x1B", "CONTROL HH"] {
            let e = chord_codes(bad).unwrap_err();
            assert!(e.starts_with("unknown key name \""), "{e}");
            assert!(e.contains("allowed, separated by spaces: "), "{e}");
            assert!(
                e.contains("ESCAPE") && !e.contains("0x00") && !e.contains("0xFF"),
                "{e}"
            );
        }
        assert!(chord_codes("CONTROL HH").unwrap_err().contains("\"HH\""));
    }

    #[test]
    fn ui_key_escape_hits_checkescape() {
        let Some(mut ui) = Ui::corpus(false) else {
            return;
        };
        // `credits`: `OK` has `CHECKESCAPE` and leads back to the main menu (`0x004DB920`, O-15).
        ui.run(Command::UiOpen {
            dialog: "credits".into(),
        })
        .unwrap();
        ui.idle();
        assert_eq!(ui.dialogs(), ["credits"]);
        assert_eq!(ui.run(key("escape")), Ok(json!({"vk": [27]})));
        ui.idle();
        assert_eq!(ui.app.menu().open_names(), ["logo", "start"]);
        let state = state_data(&ui.app, S1024, ui.now);
        assert!(
            !state
                .effects
                .iter()
                .any(|e| matches!(&e.effect, UiEffectKind::PlaySfx { .. })),
            "ESC triggers without a click sound: {:?}",
            state.effects
        );
        assert!(
            ui.run(key("ESCAP"))
                .unwrap_err()
                .starts_with("unknown key name \"ESCAP\"")
        );
    }

    #[test]
    fn ui_click_behaves_like_a_real_click() {
        let (Some(mut a), Some(mut b)) = (HostUi::corpus(), HostUi::corpus()) else {
            return;
        };
        a.idle();
        b.idle();
        assert_eq!(a.now, b.now);
        // A through the control channel, B by hand with the mouse through the host: window coordinates with a fraction
        // (`floor`), mouse button in the table of held keys, round start in the frame (O-116).
        let at = a.run(click("start", "options")).unwrap();
        let (x, y) = b
            .host
            .app
            .element_center("start", "options", S1024)
            .unwrap();
        assert_eq!(at, json!({"x": x, "y": y}));
        let now = b.now;
        for ev in [
            InputEvent::CursorMoved {
                x: f64::from(x) + 0.75,
                y: f64::from(y) + 0.25,
            },
            InputEvent::Button {
                button: HostButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: HostButton::Left,
                pressed: false,
            },
        ] {
            b.host.on_input(&ev, S1024, now);
        }
        // Frame by frame the same effects and the same state until rest.
        let mut effects = Vec::new();
        for _ in 0..200 {
            a.frame();
            b.frame();
            let (ea, eb) = (a.host.app.take_effects(), b.host.app.take_effects());
            assert_eq!(ea, eb, "effects at {} ms", a.now);
            effects.extend(ea);
            assert_eq!(
                a.host.app.report(S1024, a.now),
                b.host.app.report(S1024, b.now),
                "state at {} ms",
                a.now
            );
        }
        assert_eq!(a.host.app.menu().open_names(), ["options"]);
        assert_eq!(klicks(&a.host.app, a.now), 1);
        assert!(effects.contains(&UiEffect::PlaySfx {
            event: "SFX_KLICK".into(),
            flags: 0x14,
            category: 1
        }));

        // Locked `GAMESPY` (without GameSpy Arcade, `0x004D1895`): a center, but the click does nothing.
        let Some(mut c) = HostUi::corpus() else {
            return;
        };
        c.idle();
        let before = c.host.app.menu().open_names().to_vec();
        let gamespy = state_data(&c.host.app, S1024, c.now)
            .elements
            .into_iter()
            .find(|e| e.dialog == "start" && e.name == "gamespy")
            .expect("start/gamespy");
        assert!(!gamespy.enabled && gamespy.visible);
        c.host.app.take_effects();
        c.run(click("start", "gamespy")).unwrap();
        c.idle();
        assert_eq!(
            c.host.app.take_effects(),
            Vec::new(),
            "no click sound, no logic"
        );
        assert_eq!(c.host.app.menu().open_names(), before);
        assert_eq!(c.host.app.queue().len(), 0);
    }

    // QUIRK O-21: kept
    #[test]
    fn ui_click_during_a_flight_does_nothing() {
        // During the flight-in the button asks `0x12F` before the sound (`MenuButton_OnLeftClick` `0x0045EFD0`,
        // re/menu-system 12.1.11, O-21) - `ui_click` hits but triggers neither code 8 nor the click sound.
        let Some(mut ui) = HostUi::corpus() else {
            return;
        };
        ui.frame();
        while ui
            .host
            .app
            .element_center("start", "options", S1024)
            .is_err()
        {
            ui.frame();
            assert!(ui.now < 300, "OPTIONS never visible during the flight-in");
        }
        let state = state_data(&ui.host.app, S1024, ui.now);
        assert!(state.busy, "stack is still working");
        assert!(
            state
                .dialogs
                .iter()
                .any(|d| d.name == "start" && d.animating)
        );
        ui.host.app.take_effects();
        assert!(ui.run(click("start", "options")).is_ok());
        ui.frame();
        assert_eq!(ui.host.app.queue().len(), 0, "no order from code 8");
        ui.idle();
        assert_eq!(ui.host.app.take_effects(), Vec::new(), "no click sound");
        assert_eq!(klicks(&ui.host.app, ui.now), 0);
        assert_eq!(ui.host.app.menu().open_names(), ["logo", "start"]);
    }

    #[test]
    fn ui_state_lists_dialogs_elements_and_effects() {
        let Some(mut ui) = Ui::corpus(true) else {
            return;
        };
        ui.idle();
        assert_eq!(ui.dialogs(), ["start", "logo"]);
        ui.run(click("start", "options")).unwrap();
        ui.idle();
        let value = ui.run(Command::UiState).unwrap();
        let state: UiStateData = serde_json::from_value(value).unwrap();
        assert_eq!(state.now_ms, ui.now);
        assert_eq!(state.screen, [1024, 768]);
        // `LOGO` closes with `START` (`FUN_004E2E80`, logo rule), `OPTIONS` remains.
        let names: Vec<&str> = state.dialogs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["options"]);
        assert!(!state.dialogs[0].animating && !state.busy);
        assert_eq!(state.queued, 0);
        assert!(
            state.elements.iter().any(|e| e.dialog == "options"
                && e.name == "ok"
                && e.kind == "button"
                && e.enabled),
            "{:?}",
            state.elements
        );
        assert!(state.elements.iter().all(|e| e.dialog == "options"));
        let klicks = state
            .effects
            .iter()
            .filter(|e| {
                matches!(&e.effect, UiEffectKind::PlaySfx { event, flags: 0x14, category: 1 } if event == "SFX_KLICK")
            })
            .count();
        assert_eq!(klicks, 1, "{:?}", state.effects);
    }
}
