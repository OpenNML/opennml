//! Menu level behind the main menu (re/menu-level): the session of `menü2.lvl` in game type -2. It shares the
//! [`SessionView`] with the match (the original has one engine); `meta` and [`GameFlow::menu_level`] exclude each
//! other, `MenuApp` never sees the menu level as a match.
//!
//! - **Start** (`0x21`, `FUN_004C9140`, re/menu-level ML.2): without a menu level and without a match, loading through
//!   `Game_StartLevel` with the loading screen of the map or `default.ldg` (O-439), **without** step 2 (menus, desktop
//!   images and title music stay); if it is already running, it only becomes visible. If loading fails, the menu stays
//!   without a menu level; the next build of `START`/`OPTIONS` tries again.
//! - **Stop** (`0x20`, `FUN_004BA670`, re/menu-level ML.4): invisible and stopped, the session stays (O-437). On resume
//!   the simulation catches up on nothing (at most 2 ticks in the first frame, `GameEngine_Update` `0x00445650`); the
//!   script camera gets the whole pause time in the first frame (O-447, [`SessionView::hold`]).
//! - **End** ([`GameFlow::end_menu_level`], `FUN_004BAF40`): before every level start and every load.
//! - **Input** (re/menu-level ML.6, ML.8, ML.15 no. 5): Only the screenshot reaches the menu level
//!   ([`GameFlow::menu_level_key`]), no key, no click, no camera (`InputGate::menu_level`, O-441, O-442).
//!   A result of the local slot stops it, without an end screen (O-443).

use opennml_control::{UiMenuLevelData, hex_checksum};
use opennml_rda::normalize_path;
use opennml_sim::game_end::{DEFEAT, VICTORY};
use opennml_sim::setup::MENU_LEVEL_MODE;
use opennml_ui::app::{LevelWorld, MenuApp};
use opennml_ui::keys::{chord_codes, chord_matches};
use opennml_ui::loading::ldg_path_for;
use opennml_ui::msg::MenuLevelEffect;
use opennml_ui::profiles::name_text;

use super::{FlowEnv, GameFlow, SessionView, config_speed};
use crate::game::sim_host::SimHost;
use crate::game_session::LoadPhase;
use crate::match_request::menu_level_setup;

/// The running menu level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuLevelRun {
    /// Path of the `.lvl` (normalized, `data/levels/menülevel/menü2.lvl`).
    pub(crate) level: String,
    /// Bit `0x1` of `GameApp+0xD5E0`: visible and running.
    pub(crate) shown: bool,
    /// A video (`0x2B`/`0x2C`) is running: invisible and stopped, afterwards the old state.
    pub(crate) video: bool,
    /// Just loaded, the session has had no frame yet: the host counts the loading time toward the menus (the menus tick
    /// on real time as in the original, `FUN_004615A0` with `GetTickCount`; the fly-in of `START` ends during
    /// loading, re/menu-level ML.15 no. 1) and gives the first frame of the session at once at the clock after loading
    /// (frame time 0).
    fresh: bool,
    /// The local slot has a result (O-443): stopped until the end of the menu level.
    result: bool,
}

impl GameFlow {
    /// The menu level: path of the `.lvl` and whether it is visible; `None` without a menu level.
    pub fn menu_level(&self) -> Option<(&str, bool)> {
        self.menu_level
            .as_ref()
            .map(|run| (run.level.as_str(), run.shown))
    }

    /// Report of the menu level for `ui_state.menu_level`: path, whether the world is visible
    /// ([`Self::world_shown`]: visible and no menu video), ticks and checksum of the simulation `sim` (which the
    /// host takes from the shared session). `None` without a menu level or without a simulation. Read-only.
    pub fn menu_level_data(&self, sim: Option<&SimHost>) -> Option<UiMenuLevelData> {
        let run = self.menu_level.as_ref()?;
        let sim = sim?;
        Some(UiMenuLevelData {
            level: run.level.clone(),
            shown: self.world_shown(),
            steps: sim.state.steps,
            checksum: hex_checksum(sim.checksum()),
        })
    }

    /// Is the world drawn? During a running match and with a visible menu level without video (bit `0x1`,
    /// re/menu-level ML.4). The host passes this on to the UI only for the menu level ([`LevelWorld`]).
    pub fn world_shown(&self) -> bool {
        self.meta.is_some()
            || self
                .menu_level
                .as_ref()
                .is_some_and(|run| run.shown && !run.video)
    }

    /// Did the loading of the menu level just run? Then the host counts the loading time toward the menus (see
    /// [`MenuLevelRun`]): in the original the menu frames (DLG `FRAME`) keep ticking on
    /// `GetTickCount` (`FUN_004615A0`), `START` is at its target after the loading screen. Applies until the next
    /// [`Self::frame`].
    pub fn load_time_counts_for_menus(&self) -> bool {
        self.menu_level.as_ref().is_some_and(|run| run.fresh)
    }

    /// `0x21` or `0x20` from building a dialog (`MenuLevelEffect`).
    pub(crate) fn apply_menu_level(
        &mut self,
        effect: &MenuLevelEffect,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        if self.meta.is_some() {
            // In the shipped data unreachable: `START`/`OPTIONS` are not built during a match.
            log::info!("Menu level {effect:?} has no effect during a running match");
            return;
        }
        match effect {
            MenuLevelEffect::Load { path } => match self.menu_level.as_mut() {
                // `FUN_004C9140` step 5: engine present -> only `D5E0 |= 1`.
                Some(run) => run.shown = true,
                None => self.start_menu_level(path, app, view, env, present),
            },
            MenuLevelEffect::Hide => {
                if let Some(run) = self.menu_level.as_mut() {
                    run.shown = false;
                }
            }
        }
        self.sync_level_world(app);
    }

    /// `FUN_004C9140` -> `Game_StartLevel` at game type -2 (re/menu-level ML.2, ML.3): loading screen per
    /// [`ldg_path_for`] (`menü2.ldg` missing -> `default.ldg`), setup [`menu_level_setup`] (name `NetName`, profile
    /// variables of the active profile), speed `GameSpeed`. No `close_menus`, no stop of the menu music
    /// ([`SessionView::start_menu_level`]).
    fn start_menu_level(
        &mut self,
        path: &str,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let level = normalize_path(path);
        let ldg = ldg_path_for(&level, MENU_LEVEL_MODE, false, |p| env.vfs.exists(p));
        let speed = config_speed(app.config());
        view.apply_settings(app.config());
        let time_per_char = app.config().time_per_char;
        let local_name = name_text(&app.config().net_name);
        let vars = app.profiles().current_vars().to_vec();
        let result = self.load(&ldg, present, |progress| {
            view.start_menu_level(
                env.vfs,
                &level,
                &mut |_| {
                    Ok(menu_level_setup(
                        time_per_char,
                        Some(local_name.clone()),
                        &vars,
                    ))
                },
                speed,
                progress,
            )
        });
        match result {
            Ok(()) => {
                view.set_match_info(MENU_LEVEL_MODE, false);
                log::info!("Menu level started: {level}");
                self.menu_level = Some(MenuLevelRun {
                    level,
                    shown: true,
                    video: false,
                    fresh: true,
                    result: false,
                });
            }
            Err(e) => log::warn!("Menu level \"{level}\" not started: {e:#}"),
        }
    }

    /// One frame of the menu level (from [`Self::frame`] without a match): visible -> `GameApp_Tick` with
    /// `GameEngine_Update` ([`SessionView::advance`] with `running`); otherwise stopped ([`SessionView::hold`]: no
    /// ticks, the pause time accumulates for the script camera, O-447). Before that the settings and game type -2
    /// without a game menu go to the session. Requests of the scripts (quicksave 508, profile variable 382) go to the
    /// log: `menü2.abl` makes none, and the menu level cannot be saved. Number of ticks.
    pub(crate) fn menu_level_frame(
        &mut self,
        now: u64,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
    ) -> u32 {
        let Some(run) = self.menu_level.as_mut() else {
            return 0;
        };
        run.fresh = false;
        // Every menu video ([`MenuApp::video`]) hides the menu level and stops it (a video sets bit `0x10`,
        // `FUN_004BA680` draws only without it). `0x2B`/`0x2C` themselves are unreachable in the menu; `shown` stays
        // untouched, the old state comes back by itself after the video. The title music is not affected.
        let video = app.video().is_some();
        let video_changed = run.video != video;
        run.video = video;
        view.apply_settings(app.config());
        view.set_match_info(MENU_LEVEL_MODE, false);
        // O-443: A result of the local slot (`GameEngine_SetPlayerResult` `0x00430CE0`, script `0x56`, no trigger in
        // the shipped data) stops the menu level. The original then opens `GAMEEND_*` over the main menu; that would
        // need a match in `MenuApp`, the reimplementation only logs (deviation, unreachable).
        if !run.result && local_result(view) {
            run.result = true;
            log::warn!("Menu level: result of the local slot, stopped (no end screen)");
        }
        let running = run.shown && !run.video && !run.result && self.loading.is_none();
        let steps = if running {
            view.advance(now, true)
        } else {
            view.hold(now);
            0
        };
        for request in view.take_host_requests() {
            log::info!("Menu level: request of the scripts {request:?} has no effect");
        }
        if video_changed {
            self.sync_level_world(app);
        }
        steps
    }

    /// A key the menu did not take, with a menu level and no match (key path `FUN_004C6050`, re/game-ui D5.3). Only
    /// the screenshot `KD_HARDCOPY` reaches the menu level (reference run: the Print key works, the image shows menu
    /// and world), assigned per event as in the match (O-326) and not under the lock:
    ///
    /// - ESC sets no flag (`FUN_0042E150` opens the game menu not until game type -1, re/menu-level ML.6).
    /// - Bindings from `bindings2.ini` (groups, Home, chat, pause) and lines from `keys.ini` (speed, F1/F4, `SHIFT D`,
    ///   bar shortcuts, `selectnextguid`) do not reach it ([`InputGate::bindings`], [`InputGate::keys_ini`]): the speed
    ///   keys (`FUN_0042F050`, without a mode check) and `SHIFT D` have no effect in the original (re/menu-level ML.15
    ///   no. 5), so the stages of the key path are not reached in the state "main menu with world"; without clicks
    ///   into the world the selection for the group keys would stay empty anyway (O-442). So Home does not output the
    ///   message "No game available." (`FUN_00420230`) either.
    ///
    /// Without a menu level the key lapses (in the plain menu the screenshot is missing). `true` if the key asks for a
    /// screenshot.
    ///
    /// [`InputGate::bindings`]: crate::game::input_gate::InputGate::bindings
    /// [`InputGate::keys_ini`]: crate::game::input_gate::InputGate::keys_ini
    pub(crate) fn menu_level_key(&mut self, name: &str, view: &dyn SessionView) -> bool {
        if self.menu_level.is_none() || !view.input_gate().hardcopy() {
            return false;
        }
        self.hardcopy = chord_matches(self.hotkeys.keydef("KD_HARDCOPY"), &chord_codes(name));
        self.hardcopy
    }

    /// End of the menu level (`FUN_004BAF40`: delete the engine, `D5E0 = 6`): session closed, state gone. Nothing
    /// without a menu level (can be called repeatedly; called before every start and every load).
    pub(crate) fn end_menu_level(&mut self, app: &mut MenuApp, view: &mut dyn SessionView) {
        if self.menu_level.take().is_some() {
            view.end();
            log::info!("Menu level ended");
        }
        self.sync_level_world(app);
    }

    /// State of the world to the UI (`levelbackground.bkg` only over a visible world).
    fn sync_level_world(&self, app: &mut MenuApp) {
        app.set_level_world(LevelWorld {
            present: self.menu_level.is_some() || self.meta.is_some(),
            shown: self.world_shown(),
        });
    }
}

/// Has the local slot "lost" or "won"? Local is the active player of the session; the menu level has no match in
/// `MenuApp`.
fn local_result(view: &dyn SessionView) -> bool {
    view.sim().is_some_and(|sim| {
        sim.state
            .players
            .get(usize::from(sim.active_player))
            .is_some_and(|p| p.result == DEFEAT || p.result == VICTORY)
    })
}
