//! Game UI in the client: connection between simulation, messages and drawing.
//! - [`SimGameText`]: source of the game value tags of the text layout.
//! - [`Hud`]: window tree according to the `Engine.ini` switches, message bar, action stack and game time clock;
//!   the events of the simulation become `MessageBar_ShowMessage` calls like at the callers of the original
//!   (re/game-ui N.7.2). In addition the status bar with its values ([`Hud::status_values`]), drawing in the order
//!   of the original ([`Hud::draw`]) and the report for the control channel ([`Hud::state`]).
//! - The command bar ([`Hud::toolbar`], only with `IsToolBar`), the tooltip window and the tooltip areas; the mouse
//!   goes through the window tree via [`Hud::input`], drawing follows the order of the tree ([`Hud::draw_windows`]
//!   with [`BarFrame`]).
//! - The cinemascope area ([`Hud::cinemascope`]) with the UI messages of the script presentation ([`Hud::present`]),
//!   drawn in the `Cinemascope` node of the tree after the command bar and before the tooltip (re/campaign-ui D6.4).
//! - The minimap ([`Hud::minimap`]) in the `Minimap` node with the data of the host ([`MinimapFrame`]); event `0x1D`
//!   hides and shows it.
//! - The pings of the minimap ([`Hud::pings`]) next to the action stack, one path for all sources ([`Hud::ping`],
//!   [`Hud::script_ping`]).
//! - Mouse events to the minimap before the bar, also without a bar ([`Hud::input`]).
//! - The corner buttons of the minimap ([`Hud::minimap_buttons`]) as children of its node: mouse by slot 3 of the
//!   map, drawing after the camera frame, setup at `{0x19}`, `{0x1D}`, `{0x22}`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use nml_control::{GAME_UI_DIALOG, UiHudData, UiHudMessage, UiHudWindow, UiStatusBarData};
use nml_formats::engine_ini::EngineConfig;
use nml_formats::messages::Messages;
use nml_formats::statusbar::{STATUSBAR_PATH, parse_statusbar};
use nml_formats::text::decode_cp1252;
use nml_sim::economy::message::flags;
use nml_sim::id::ObjectId;
use nml_sim::state::SimEvent;
use nml_ui::assets::Texts;
use nml_ui::coords::Screen;
use nml_ui::dialog::UiContext;
use nml_ui::draw::DrawCmd;
use nml_ui::hud::cinemascope::{CinemascopeData, CinemascopeView};
use nml_ui::hud::message_bar::{
    ActionStack, BarEffect, GameClock, MessageBar, NO_TILE, ShowResult,
};
use nml_ui::hud::portrait::{PortraitModel, TooltipAreas, draw_counter_row, draw_portrait};
use nml_ui::hud::screen_text::ScreenTexts;
use nml_ui::hud::status_bar::{StatusBar, StatusValues};
use nml_ui::hud::tile_grids::TileGrids;
use nml_ui::hud::toolbar::button::ButtonOut;
use nml_ui::hud::toolbar::data::ToolbarData;
use nml_ui::hud::toolbar::minimap_buttons::MinimapButtons;
use nml_ui::hud::toolbar::{Toolbar, ToolbarModel, ToolbarOut};
use nml_ui::hud::tooltip::TooltipWindow;
use nml_ui::hud::tree::{GameWindows, HudSwitches, WindowId, WindowKind, mouse_point};
use nml_ui::hud::{HudEvent, HudEventKind};
use nml_ui::text::{GameTags, GameText, TextContext, TradeCost, resolve_game_tags};

mod control;

use crate::game::input_gate::InputGate;
use crate::game::minimap::pings::{SCRIPT_PING_REPEATS, SIGNAL_PING_REPEATS};
use crate::game::minimap::{Minimap, MinimapClick, MinimapFrame, MinimapPings};
use crate::game::notices::{Notices, SFX_SIGNAL, SoundRequest};
use crate::game::sim_host::SimHost;
use crate::game::toolbar_host::ToolbarHost;
use crate::game_flow::player_display_name;

/// Message "Tribute" (`GameWorld_CmdSendTribute` `0x00478840`: `0xE6`).
pub const TRIBUTE_MESSAGE: i32 = 230;
/// Substitute message "Unit is under attack" (`GameObject_ReportUnderAttack` `0x0046CB40`).
pub const UNIT_ATTACKED: i32 = 100;
/// Substitute message "Building is under attack".
pub const BUILDING_ATTACKED: i32 = 101;
/// Flag word of the attack message: priority 1, **not** queued (`0x01000000`).
pub const UNDER_ATTACK_FLAGS: u32 = flags::priority(1);

/// Message "Game speed was increased." (`GameEngine_CmdSetGameSpeed` `0x0042EF30`, text 17260).
pub const SPEED_RAISED_MESSAGE: i32 = 9;
/// Message "Game speed was slowed down." (`0x0042EF30`, text 17261).
pub const SPEED_LOWERED_MESSAGE: i32 = 10;
/// Flag word of the speed messages 9 and 10 (`0x0042EF30`): priority 3, queued (`0x03010000`).
pub const SPEED_MESSAGE_FLAGS: u32 = flags::priority(3) | flags::QUEUE;

/// Settings of the game UI from the `Engine.ini`: path from the host into the session via
/// [`crate::game::options::ViewOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudOptions {
    /// `TimePerChar` (`EngineConfig+0x50`): display duration of the messages per character in ms.
    pub time_per_char: i32,
    /// `IsStatusBar`, `IsMap`, `IsToolBar`: which windows the tree creates (`0x004491F0`).
    pub switches: HudSwitches,
    /// `EnhancedTooltips` (`EngineConfig+0x38` != 0): extended tooltip text `GUID + 10000`
    /// (`nml_ui::hud::tooltip::TooltipWindow::draw`).
    pub enhanced_tooltips: bool,
}

impl HudOptions {
    /// From the loaded `Engine.ini`.
    pub fn from_config(config: &EngineConfig) -> Self {
        Self {
            time_per_char: config.time_per_char,
            switches: HudSwitches::from_config(config),
            enhanced_tooltips: config.enhanced_tooltips != 0,
        }
    }
}

impl Default for HudOptions {
    /// Default of the `Engine.ini` (55 ms, all windows on).
    fn default() -> Self {
        Self::from_config(&EngineConfig::default())
    }
}

/// What drawing the command bar needs per frame besides the bar itself: the snapshot of the match, portrait and
/// counter-upgrades, the game values of the tooltip (with selection and formation) and the key layout for `[KEYID]`.
/// Built when drawing from the match (read-only).
pub struct BarFrame<'a> {
    pub model: &'a ToolbarModel<'a>,
    pub portrait: &'a PortraitModel,
    pub game: &'a dyn GameText,
    pub keys: &'a TextContext,
}

/// Game UI of a match: window tree, message bar with game time clock and action stack, status bar; command bar,
/// tooltip window and tooltip areas.
#[derive(Debug, Clone)]
pub struct Hud {
    pub messages: MessageBar,
    /// Ping locations for button/key 570 (O-219; `GameSession::minimap_button`).
    pub actions: ActionStack,
    /// Pings of the minimap (`GameWorld+0x22F8`): next to the stack because every addition changes both
    /// ([`Hud::ping`]); advanced only in frames with a drawn map (`GameSession::sync_minimap`).
    pub pings: MinimapPings,
    /// Status bar (`0x00452720`): only with `IsStatusBar` and a read `statusbar.ini`
    /// ([`Hud::load_status_bar`]); `Hud::new` leaves it empty.
    pub status: Option<StatusBar>,
    /// Window tree according to the switches (`0x004491F0`).
    pub windows: GameWindows,
    /// State of the minimap: terrain, fog buffer, visibility `+0x4D`, mode and the position of the last frame. The
    /// host creates it with the level (`GameSession::new`); `Hud::new` leaves it empty. It does not hang on the window
    /// tree: [`Hud::resize`] rebuilds the tree, the map stays. It is drawn only with a node (`IsMap`) and visible
    /// ([`Hud::minimap_shown`]).
    pub minimap: Option<Minimap>,
    /// Corner buttons of the minimap (`0x0044A040`): children of the `Minimap` node, only with `IsMap`. Created with
    /// the tree (constructor `0x0044A710`), anew at `{0x19, 0}`, `{0x1D, 1}`, `{0x22}`, at a new screen size and after
    /// loading/restart; gone at `{0x19, 1}` and `{0x1D, 0}`. Their clicks go to slot 3 of the bar ([`Hud::input`]).
    pub minimap_buttons: MinimapButtons,
    /// Game type of the match (`Engine+0x108`) for building the corner buttons: 580 only at `≥ 0`. Default -1 like
    /// `GameSession::game_type`; the host sets it via [`Hud::set_game_type`].
    pub game_type: i32,
    /// Command bar (`0x00456630`): only with `IsToolBar` ([`Hud::load`]); `Hud::new` leaves it empty. The host mirrors
    /// the visibility `+0x1C` (event `0x11`) into the node of the tree ([`Hud::sync_toolbar_visibility`]).
    pub toolbar: Option<Toolbar>,
    /// Tooltip window (`0x004592E0`): the request of the frame, set in the frame step.
    pub tooltip: TooltipWindow,
    /// Tooltip areas of portrait, values and counter-upgrades (owner `Hud`). As in the original they arise **while
    /// drawing** (`0x00456B70`, `0x00455C00`), because their width depends on the text layout of the values; drawing
    /// stays `&self`, hence the cell. The frame step reads the areas of the last drawing (`0x00453B50`); when hidden
    /// it clears them.
    pub areas: RefCell<TooltipAreas>,
    /// Box of the last drawn tooltip (for the control channel); `None` without a tooltip.
    pub tooltip_box: Cell<Option<[i32; 4]>>,
    /// `EnhancedTooltips` (`EngineConfig+0x38`): extended tooltip text; default from [`HudOptions`], `nml-game`
    /// updates it from the settings each frame.
    pub enhanced_tooltips: bool,
    /// Texture cache of the original by file name (`0x00445F40`, O-280): the status bar enters its icon sheet, the
    /// command bar reads from it. Building the game UI (`0x004491F0`) loads status bar, minimap, command bar in that
    /// order; [`Hud::load`] likewise.
    pub tile_grids: TileGrids,
    /// Cinemascope area (`0x004483D0`): bars, portrait and text slots; messages via [`Hud::present`], fading on the
    /// ticks of `GameSession::tick_presentation`.
    pub cinemascope: CinemascopeView,
    /// Data of the area (`cinemascope.ini`, `portraits.ini`, `portraitframes.ini`), read with [`Hud::load`];
    /// `Hud::new` leaves it empty (no bars).
    pub cinemascope_data: CinemascopeData,
    /// Screen texts of the message bar (`MessageBar+0xC`): `n5` queues (`GameSession::apply_presentation_event`),
    /// [`Hud::tick`] advances them with the game time of the bar, drawn with the world (`GameSession::combat_cmds`,
    /// re/campaign-ui D6.12).
    pub screen_texts: ScreenTexts,
    /// Accumulated game time of the bar in ms (sum of `dt` from [`Hud::tick`], integer like `+0x44` of the entries):
    /// the clock of the floating numbers (`FUN_004925D0`). It stands during pause and game menu and runs with the game
    /// speed; only differences count, which is why it keeps running across loading and restart (not in the savegame;
    /// the numbers themselves are cleared by `Notices::clear`).
    pub bar_ms: i64,
    switches: HudSwitches,
    screen: Screen,
    clock: GameClock,
}

impl Hud {
    /// Bar from `messages.ini` and `texte.deu` (the data of [`Notices::load`]) and the window tree for `screen`
    /// according to the switches, without a status bar (only [`Hud::load_status_bar`] reads it).
    pub fn new(
        messages: &Messages,
        texts: &BTreeMap<i32, String>,
        options: &HudOptions,
        screen: Screen,
    ) -> Self {
        // The minimap constructor loads `map_icons.tga` via the texture cache (O-280).
        let mut tile_grids = TileGrids::new();
        let minimap_buttons = MinimapButtons::new(&mut tile_grids);
        let mut hud = Self {
            messages: MessageBar::new(messages, texts, options.time_per_char),
            actions: ActionStack::default(),
            pings: MinimapPings::default(),
            status: None,
            windows: GameWindows::new(screen, options.switches, 0),
            minimap: None,
            minimap_buttons,
            game_type: -1,
            toolbar: None,
            tooltip: TooltipWindow::new(),
            areas: RefCell::new(TooltipAreas::new()),
            tooltip_box: Cell::new(None),
            enhanced_tooltips: options.enhanced_tooltips,
            tile_grids,
            cinemascope: CinemascopeView::new(),
            cinemascope_data: CinemascopeData::default(),
            screen_texts: ScreenTexts::new(),
            bar_ms: 0,
            switches: options.switches,
            screen,
            clock: GameClock::default(),
        };
        hud.rebuild_minimap_buttons();
        hud
    }

    /// Re-create the corner buttons (`0x0044A040`) according to screen size, map mode (`GameWorld+0x22D4`, 0 without a
    /// minimap) and game type; without a `Minimap` node (`IsMap` off) there are none.
    pub fn rebuild_minimap_buttons(&mut self) {
        if self.windows.find(WindowKind::Minimap).is_none() {
            self.minimap_buttons.remove();
            return;
        }
        let mode = self.minimap.as_ref().map_or(0, |m| m.mode);
        let Screen { width, height } = self.screen;
        self.minimap_buttons
            .build(width, height, mode, self.game_type);
    }

    /// Game type of the match (`Engine+0x108`); if the corner buttons exist, they are re-created with it. The
    /// original knows the game type already when building the UI, the reimplementation learns it only from the flow.
    pub fn set_game_type(&mut self, game_type: i32) {
        if self.game_type != game_type {
            self.game_type = game_type;
            if self.minimap_buttons.is_built() {
                self.rebuild_minimap_buttons();
            }
        }
    }

    /// The game UI when building the session (`0x004491F0`): message bar from the data of `notices` with
    /// `TimePerChar`, status bar from `statusbar.ini` (only with `IsStatusBar`), window tree according to the
    /// switches, the command bar and last the data of the cinemascope area (its node is the last child of the root,
    /// re/campaign-ui D6.1). `messages.ini` and `texte.deu` are already in [`Notices`] and are not read a second time.
    pub fn load(
        vfs: &nml_rda::Vfs,
        notices: &Notices,
        options: &HudOptions,
        screen: Screen,
    ) -> Self {
        let mut hud = Self::new(&notices.messages, &notices.texts, options, screen);
        hud.load_status_bar(vfs, options);
        hud.load_toolbar(vfs, options);
        hud.cinemascope_data = CinemascopeData::load(vfs);
        hud
    }

    /// Create the command bar if `IsToolBar` is set (`0x00456630`): data from `toolbar.cfg` and `icons.ini`
    /// ([`ToolbarData::load`]); without the switch no bar. The sheets take the grid from [`Hud::tile_grids`]: what
    /// the status bar loaded earlier keeps its grid (O-280).
    pub fn load_toolbar(&mut self, vfs: &nml_rda::Vfs, options: &HudOptions) {
        self.toolbar = options
            .switches
            .tool_bar
            .then(|| Toolbar::new(ToolbarData::load(vfs, &mut self.tile_grids), self.screen));
    }

    /// Node of the command bar in the tree.
    pub fn toolbar_window(&self) -> Option<WindowId> {
        self.windows.find(WindowKind::Toolbar)
    }

    /// Event `0x11` mirrored: the bar keeps `+0x1C` itself ([`Toolbar::shown`]); the node in the tree follows it (two
    /// sources of truth, the host keeps them equal).
    pub fn sync_toolbar_visibility(&mut self) {
        if let (Some(t), Some(id)) = (&self.toolbar, self.toolbar_window()) {
            self.windows.set_visible(id, t.shown);
        }
    }

    /// Raw mouse `raw` at `at` (UI pixels) through the window tree from layer 4 (`0x004528F0`,
    /// [`GameWindows::dispatch_with_toolbar`]): the minimap via [`Minimap::mouse`] with the window rectangle, the
    /// query `gate` and "selection not empty" from `model` (its effects go to `clicks`), the bar via
    /// [`Toolbar::handle`], the other nodes take nothing. `true` if a window takes the input; then it does not belong
    /// to the world (O-222). Without a bar only the minimap takes it, without a minimap (no level or `IsMap` off) only
    /// the bar.
    pub fn input(
        &mut self,
        raw: HudEventKind,
        at: [i32; 2],
        model: &ToolbarModel,
        gate: InputGate,
        out: &mut Vec<ToolbarOut>,
        clicks: &mut Vec<MinimapClick>,
    ) -> bool {
        let ev = HudEvent::with_args(raw, &at);
        let selected = !model.selection.is_empty();
        self.minimap_buttons.mouse = Some(at);
        let mut button_outs = Vec::new();
        let Self {
            windows,
            toolbar,
            minimap,
            minimap_buttons,
            ..
        } = self;
        let windows = &*windows;
        let map_rect = windows.find(WindowKind::Minimap).map(|id| windows.rect(id));
        let taken = windows.dispatch_with_toolbar(
            4,
            &ev,
            &mut |e| match (minimap.as_mut(), map_rect, mouse_point(e)) {
                (Some(m), Some(rect), Some(at)) => {
                    let (taken, click) = m.mouse(e.kind, at, rect, gate, selected);
                    clicks.extend(click);
                    taken
                }
                _ => false,
            },
            &mut |e| {
                mouse_point(e).is_some_and(|at| minimap_buttons.mouse(e.kind, at, &mut button_outs))
            },
            &mut |slot, e| {
                toolbar
                    .as_mut()
                    .is_some_and(|t| t.handle(slot, e, model, out))
            },
            &mut |_, _, _| false,
        );
        // The corner buttons: click sound by themselves (`0x0044F860`), the event 9/`0x26`/`0x27` to slot 3 of the bar
        // (`0x00458510`) - only a visible bar takes it (O-292); without a bar taken without effect. `0x28` is not taken
        // by the bar (O-223).
        for o in button_outs {
            match o {
                ButtonOut::Sound(sound) => out.push(ToolbarOut::Sound(sound)),
                ButtonOut::Event { msg, flags, ev, .. } => {
                    if let Some(t) = self.toolbar.as_mut() {
                        let e = HudEvent::with_args(ev.kind(), &[msg, flags as i32]);
                        t.event(&e, model, out);
                    }
                }
            }
        }
        self.sync_toolbar_visibility();
        taken
    }

    /// Create the status bar if `IsStatusBar` is set (`0x00452720`, reader `0x00451F80`): the configuration comes
    /// from `data/gifcon/statusbar.ini`. If the file is not readable, there is a warning and no bar; with the switch
    /// off nothing is read and an existing bar is removed. The window tree follows the switches and the height
    /// `STATUSSIZE` (0 without a bar). The icon sheet comes with its grid into [`Hud::tile_grids`] (O-280).
    pub fn load_status_bar(&mut self, vfs: &nml_rda::Vfs, options: &HudOptions) {
        self.status = None;
        self.switches = options.switches;
        if options.switches.status_bar {
            match vfs.read(STATUSBAR_PATH) {
                Ok(bytes) => {
                    let mut config = parse_statusbar(&decode_cp1252(&bytes));
                    // `ICONFILE` loads the sheet with `ICONSIZE` (`0x0045229A`); being first, the status bar thereby
                    // fixes the grid of the file (O-280).
                    config.icon_size = self
                        .tile_grids
                        .register(&config.icon_file, config.icon_size);
                    self.status = Some(StatusBar::new(config));
                }
                Err(e) => log::warn!("Status bar: {STATUSBAR_PATH} not read: {e:#}"),
            }
        }
        self.rebuild_windows();
    }

    /// New screen size: rectangles of the tree anew (fixed pixels, O-34). The original builds the tree only at the
    /// start of the match; the reimplementation follows a changed window size, the visibility per window stays.
    pub fn resize(&mut self, screen: Screen) {
        if self.screen != screen {
            self.screen = screen;
            self.rebuild_windows();
            if let Some(t) = &mut self.toolbar {
                t.resize(screen);
            }
            // The corner buttons follow the positions (fixed pixels from the right and bottom edge).
            if self.minimap_buttons.is_built() {
                self.rebuild_minimap_buttons();
            }
        }
    }

    /// Screen for which the tree is built.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    fn rebuild_windows(&mut self) {
        let size = self.status.as_ref().map_or(0, |bar| bar.config.size);
        let mut windows = GameWindows::new(self.screen, self.switches, size);
        for id in windows.ids() {
            if let Some(old) = self.windows.find(windows.kind(id)) {
                windows.set_visible(id, self.windows.visible(old));
            }
        }
        let had_map = self.windows.find(WindowKind::Minimap).is_some();
        self.windows = windows;
        // The corner buttons hang on the node of the minimap: if it falls away (`IsMap` off) or appears, they do too.
        if had_map != self.windows.find(WindowKind::Minimap).is_some() {
            self.rebuild_minimap_buttons();
        }
    }

    /// Values of the status bar for the active slot (`0x00451B40`): resources 0-4 via `Player_GetResource` (living
    /// space limited as in the original) and the display name ([`player_display_name`]; empty outside 0-7).
    pub fn status_values(sim: &SimHost, texts: &Texts) -> StatusValues {
        let slot = usize::from(sim.active_player);
        StatusValues {
            resources: std::array::from_fn(|i| sim.state.resource(slot, i)),
            name: player_display_name(sim, texts, i32::from(sim.active_player)),
        }
    }

    /// Messages in the world pass (`0x00438F2A` -> `0x00493B80`): per kind the head ([`MessageBar::draw`]).
    pub fn draw_world_pass(
        &self,
        ctx: &UiContext<'_>,
        game: &dyn GameText,
        out: &mut Vec<DrawCmd>,
    ) {
        self.messages.draw(ctx, game, out);
    }

    /// The game windows in the draw order of the tree (`0x004529A0`, [`GameWindows::draw_order`]), read-only after the
    /// frame step:
    /// - status bar;
    /// - command bar like `0x004574B0`: background, portrait with values (`0x00456B70`), counter-upgrades
    ///   (`0x00455C00`) (both rebuild the tooltip areas in the process ([`Hud::areas`]); the area check `0x00453B50`
    ///   belongs to the frame step), then the buttons as children. The target point decal of the same drawer lies in
    ///   the world pass (`GameSession::mark_ground`);
    /// - the cinemascope area (`0x004484F0`: bars, edge tiles, portraits, text slots; re/campaign-ui D6.4), game
    ///   values of the text slots from `game`;
    /// - last the tooltip window (layer 1), above the bars.
    ///
    /// The minimap draws its node, on layer 2 after the status bar and before the command bar, only with `minimap`
    /// (the data of the host for this frame) and visible ([`crate::game::minimap::draw::draw`]).
    ///
    /// Without `bar` (no snapshot of the match): no command bar and no tooltip.
    pub fn draw_windows(
        &self,
        ctx: &mut UiContext<'_>,
        game: &dyn GameText,
        values: &StatusValues,
        bar: Option<&BarFrame>,
        minimap: Option<&MinimapFrame>,
        out: &mut Vec<DrawCmd>,
    ) {
        for id in self.windows.draw_order() {
            match self.windows.kind(id) {
                WindowKind::StatusBar => {
                    if let Some(status) = &self.status {
                        status.draw(ctx, values, out);
                    }
                }
                WindowKind::Toolbar => {
                    if let (Some(t), Some(f)) = (&self.toolbar, bar) {
                        t.draw_background(ctx, out);
                        if t.shown {
                            let mut areas = self.areas.borrow_mut();
                            draw_portrait(
                                ctx, &t.data, t.selected, t.hover, f.portrait, &mut areas, out,
                            );
                            draw_counter_row(
                                ctx,
                                &t.data,
                                &f.portrait.counter_upgrades,
                                &mut areas,
                                out,
                            );
                        }
                        t.draw_children(ctx, f.model, out);
                    }
                }
                WindowKind::Minimap => {
                    if let (Some(m), Some(f)) = (&self.minimap, minimap)
                        && m.visible()
                    {
                        crate::game::minimap::draw::draw(m, f, ctx, out);
                    }
                    // The corner buttons are children of the node (layer 1): after the map including the camera frame.
                    self.minimap_buttons.draw(ctx, out);
                }
                WindowKind::Cinemascope => {
                    let tags = GameTags::new(game, 0);
                    self.cinemascope
                        .draw(&self.cinemascope_data, ctx, &tags, out);
                }
                WindowKind::Tooltip => {
                    if let Some(f) = bar {
                        let rect =
                            self.tooltip
                                .draw(ctx, f.game, f.keys, self.enhanced_tooltips, out);
                        self.tooltip_box.set(rect);
                    }
                }
                _ => {}
            }
        }
    }

    /// One frame of the game UI as in the original: first the messages in the world pass, then the windows.
    pub fn draw(
        &self,
        ctx: &mut UiContext<'_>,
        game: &dyn GameText,
        values: &StatusValues,
        bar: Option<&BarFrame>,
        minimap: Option<&MinimapFrame>,
        out: &mut Vec<DrawCmd>,
    ) {
        self.draw_world_pass(ctx, game, out);
        self.draw_windows(ctx, game, values, bar, minimap, out);
    }

    /// Is the minimap drawn in this frame if the host supplies it? Node in the tree (`IsMap`) and visible (`+0x4D`);
    /// the host additionally checks cinemascope ([`crate::game::input_gate::InputGate::minimap_visible`]).
    pub fn minimap_shown(&self) -> bool {
        self.windows.find(WindowKind::Minimap).is_some()
            && self.minimap.as_ref().is_some_and(Minimap::visible)
    }

    /// Report for `ui_state.game.hud`: windows in creation order; per kind with messages in file order the head with
    /// remaining times (text with game values inserted) and the MSGIDs behind it; action stack; status bar texts.
    /// Read-only.
    pub fn state(&self, game: &dyn GameText, values: &StatusValues) -> UiHudData {
        let windows = self
            .windows
            .ids()
            .map(|id| UiHudWindow {
                kind: self.windows.kind(id).name().to_string(),
                rect: self.windows.rect(id),
                layer: self.windows.layer(id),
                visible: self.windows.visible(id),
            })
            .collect();
        let messages = self
            .messages
            .tags()
            .iter()
            .enumerate()
            .filter_map(|(slot, tag)| {
                let queue = self.messages.queue(slot);
                let head = queue.front()?;
                let tags = GameTags::new(game, head.players);
                Some(UiHudMessage {
                    tag: tag.name.clone(),
                    id: head.id,
                    text: resolve_game_tags(&head.text, &tags),
                    flags: head.flags,
                    display_ms: head.display_ms,
                    fade_ms: head.fade_ms,
                    queued: queue.iter().skip(1).map(|m| m.id).collect(),
                })
            })
            .collect();
        let status = self.status.as_ref().map(|bar| {
            let texts = bar.texts(values);
            UiStatusBarData {
                shown: bar.shown,
                red: bar.shows_overfull(values),
                texts: texts.into_iter().map(|(_, text, _)| text).collect(),
            }
        });
        UiHudData {
            dialog: GAME_UI_DIALOG.to_string(),
            windows,
            messages,
            action_stack: self.actions.tiles().iter().map(|&(x, z)| [x, z]).collect(),
            status,
            // The bar needs portrait and keys of the session: `GameSession::hud_state` sets it via
            // [`Hud::toolbar_report`].
            toolbar: None,
        }
    }

    /// Events of a frame for the local player `active` (re/game-ui N.7.2):
    /// - `PlayerMessage` to `active` or to everyone -> as reported;
    /// - `UnderAttack` of an object of `active` -> type GUID with `0x01000000`, on return value 0 ("not exist") 101
    ///   (building) or 100;
    /// - `TributeSent` -> 230 without location with `0x10000 | recipient << 8 | giver`, to everyone;
    /// - `CounterUpgradeStarted` -> GUID at the location of the source with `slot | 0x10000`, to everyone;
    /// - `Signal` (`0x5C`) -> with the bit of the local slot in the mask `SFX_SIGNAL` and a ping with one repetition
    ///   (location also onto the stack).
    ///
    /// Sounds go to `out`, pings onto the action stack.
    pub fn handle(&mut self, events: &[SimEvent], active: u8, out: &mut Vec<SoundRequest>) {
        let mut effects = Vec::new();
        for event in events {
            match *event {
                SimEvent::PlayerMessage {
                    to,
                    id,
                    tile,
                    flags,
                } if to.is_none_or(|p| p == active) => {
                    self.messages.show(id, tile, flags, &mut effects);
                }
                // `GameObject_ReportUnderAttack` `0x0046CB40` (re/combat-host 4.1), only at the owner.
                SimEvent::UnderAttack {
                    owner,
                    guid,
                    building,
                    tile,
                    ..
                } if owner == active => {
                    let shown = i32::try_from(guid).map_or(ShowResult::NotExist, |id| {
                        self.messages
                            .show(id, tile, UNDER_ATTACK_FLAGS, &mut effects)
                    });
                    if shown == ShowResult::NotExist {
                        let id = if building {
                            BUILDING_ATTACKED
                        } else {
                            UNIT_ATTACKED
                        };
                        self.messages
                            .show(id, tile, UNDER_ATTACK_FLAGS, &mut effects);
                    }
                }
                // `GameWorld_CmdSendTribute` `0x00478840`: `((recipient | 0x100) << 8) | giver`, without checking the
                // local player. Byte 1 lies on the sound bits (O-216).
                SimEvent::TributeSent { giver, recipient } => {
                    let merker = flags::players(u32::from(giver), u32::from(recipient));
                    self.messages
                        .show(TRIBUTE_MESSAGE, NO_TILE, merker, &mut effects);
                }
                // `GameWorld_CmdStartCounterUpgrade` `0x004848F0` (re/temp-upgrades 3.5): everyone sees it.
                SimEvent::CounterUpgradeStarted { guid, player, tile } => {
                    if let Ok(id) = i32::try_from(guid) {
                        let merker = u32::from(player) | flags::QUEUE;
                        self.messages.show(id, tile, merker, &mut effects);
                    }
                }
                // Signal `0x5C` (`0x0048D4D6…0x0048D539`, O-321): only if the bit of the local slot is in the mask,
                // `SFX_SIGNAL` 2D (`PUSH 0, 1, 0x14`), then `GameWorld_Ping` (`0x00480C00`) with the color of the
                // slot field and **one** repetition. If a ping is already running at the tile, only sound and stack
                // remain (O-41).
                SimEvent::Signal {
                    color, mask, tile, ..
                } if mask
                    .checked_shr(u32::from(active))
                    .is_some_and(|m| m & 1 != 0) =>
                {
                    out.push(SoundRequest::Event2d {
                        name: SFX_SIGNAL.to_string(),
                    });
                    self.ping(tile, color_argb(color), SIGNAL_PING_REPEATS);
                }
                _ => {}
            }
        }
        self.apply(effects, out);
    }

    /// A message from the host instead of from a `SimEvent` (`MessageBar_ShowMessage(id, location, flags)`, here for
    /// the speed messages 9/10 of the speed keys): like [`Hud::handle`] with sound into `out` and ping onto the action
    /// stack. Returns the return value of the bar (0 "not exist" to 3 "ignored").
    pub fn post(
        &mut self,
        id: i32,
        tile: (i32, i32),
        flags: u32,
        out: &mut Vec<SoundRequest>,
    ) -> ShowResult {
        let mut effects = Vec::new();
        let shown = self.messages.show(id, tile, flags, &mut effects);
        self.apply(effects, out);
        shown
    }

    /// A frame of `dt_ms` ms at `game_speed`: game time of the clock, then as in `FUN_00494A00` first the screen texts
    /// (`FUN_004925D0`, re/campaign-ui D6.12) and after them the tick of the bar (`0x00494570`). Call only while the
    /// match is running (during pause and game menu the bar and the screen texts stand still, O-213, `0x0043D1DB`).
    pub fn tick(&mut self, dt_ms: f32, game_speed: u8, out: &mut Vec<SoundRequest>) {
        let dt = self.clock.tick(dt_ms, game_speed);
        self.bar_ms += i64::from(dt);
        self.screen_texts.tick(dt);
        let mut effects = Vec::new();
        self.messages.tick(dt, &mut effects);
        self.apply(effects, out);
    }

    /// After loading or restart: bar, screen texts, action stack and clock remainder empty (not in the savegame,
    /// re/game-ui N.7.4); the command bar in the initial state of the constructor (no buttons, no cursor, visible), no
    /// tooltip, no areas (none of it is in the savegame). Status bar visible and cinemascope area empty
    /// (`GameSession::restore_presentation` rebuilds both from the state).
    pub fn clear(&mut self) {
        self.messages.clear();
        self.screen_texts.clear();
        self.actions.clear();
        self.pings.clear();
        self.clock.reset();
        if let Some(t) = &mut self.toolbar {
            *t = Toolbar::new(t.data.clone(), self.screen);
        }
        self.sync_toolbar_visibility();
        self.tooltip.clear();
        self.areas.borrow_mut().clear();
        self.tooltip_box.set(None);
        if let Some(status) = &mut self.status {
            status.shown = true;
        }
        self.cinemascope = CinemascopeView::new();
        // The minimap begins visible (`+0x4D = 1` of the constructor); a loaded cinemascope hides it via the lock of
        // the host. Terrain and fog buffer stay (the fog catches up with the immediate setup of loading).
        if let Some(m) = &mut self.minimap {
            m.set_visible(true);
        }
        // The corner buttons as when building the window (`0x0044A710` -> `0x0044A040`).
        self.rebuild_minimap_buttons();
    }

    /// The UI messages of a presentation event (`toolbar_host::presentation_events`) to their recipients, in send
    /// order:
    /// - `0x1B`, `0x1E` ... `0x21` immediately to the cinemascope area (`0x00447110`);
    /// - `0x1C` immediately to the status bar (`0x004518E0`; it hides itself in cinemascope);
    /// - `0x11`, `0x19` to the command bar via `toolbar` (with the next frame, in sequence with `{8}`);
    /// - `0x1D` to the minimap: `+0x4D = (w ≠ 0)` ([`Minimap::set_visible`]), the corner buttons anew or gone;
    ///   `0x19` also to the corner buttons, `0x22` only to them.
    pub fn present(&mut self, events: Vec<HudEvent>, toolbar: &mut ToolbarHost) {
        for ev in events {
            self.route(ev, toolbar, false);
        }
    }

    /// Like [`Hud::present`]; with `now`, shown portraits and text slots stand there fully at once
    /// ([`CinemascopeView::show_now`], restoring after loading).
    pub(crate) fn route(&mut self, ev: HudEvent, toolbar: &mut ToolbarHost, now: bool) {
        match ev.kind {
            HudEventKind::Cinemascope
            | HudEventKind::PortraitShow
            | HudEventKind::PortraitHide
            | HudEventKind::TextShow
            | HudEventKind::TextHide => {
                if now {
                    self.cinemascope.show_now(&ev, &self.cinemascope_data);
                } else {
                    self.cinemascope.message(&ev, &self.cinemascope_data);
                }
            }
            HudEventKind::StatusBarOnOff => {
                if let Some(status) = &mut self.status {
                    status.event(&ev);
                }
            }
            // `{0x1D, w}` (`0x0044A5C0`, slot 2 of the minimap): `+0x4D = (w ≠ 0)`, then the corner buttons anew
            // (`0x0044A040`) or gone (`FUN_00449F60`). Without a minimap (no level) `+0x4D` has no effect.
            HudEventKind::MinimapOnOff => {
                let on = ev.args.first().copied().unwrap_or(0) != 0;
                if let Some(m) = &mut self.minimap {
                    m.set_visible(on);
                }
                if on {
                    self.rebuild_minimap_buttons();
                } else {
                    self.minimap_buttons.remove();
                }
            }
            // `{0x22}` (slot 3 `0x0044A655`): the corner buttons anew, e.g. after the mode switch.
            HudEventKind::MinimapButtons => self.rebuild_minimap_buttons(),
            // `{0x19, w}` also reaches the minimap (`0x0044A59C`): w != 0 removes the corner buttons, w = 0 rebuilds
            // them (also with a hidden map); then on to the bar.
            HudEventKind::Lock => {
                if ev.args.first().copied().unwrap_or(0) != 0 {
                    self.minimap_buttons.remove();
                } else {
                    self.rebuild_minimap_buttons();
                }
                toolbar.host_event(ev);
            }
            _ => toolbar.host_event(ev),
        }
    }

    /// Effects of the bar: sound -> [`SoundRequest::Message`]; ping -> [`Hud::ping`] (location onto the action stack,
    /// then the minimap ping with the color of the kind and 5 or 1 repetitions respectively).
    fn apply(&mut self, effects: Vec<BarEffect>, out: &mut Vec<SoundRequest>) {
        for effect in effects {
            match effect {
                BarEffect::Sound(guid) => out.push(SoundRequest::Message(guid)),
                BarEffect::Ping {
                    tile,
                    color,
                    repeats,
                } => self.ping(tile, color, repeats),
            }
        }
    }

    /// `GameWorld_Ping(x, z, color, repeats)` (`0x00480C00`, [`MinimapPings::add`]): location in front onto the action
    /// stack, then the ping, unless one is already running at the same tile. One path for all sources: messages (5),
    /// script action 7 (10), signal (1).
    pub fn ping(&mut self, tile: (i32, i32), color: u32, repeats: i32) {
        self.pings.add(tile, color, repeats, &mut self.actions);
    }

    /// Script action 7 (`FUN_0043EBE0`): first `SFX_SIGNAL` as a 2D event (`PlayEvent2D(name, 0x14, 1, 0)`, for every
    /// viewer, without a player filter), then the ping at `tile` with the color `0x006189D0[player]` (raw; a `player`
    /// outside the table gives light gray, there is no load check, E-244) and a fixed [`SCRIPT_PING_REPEATS`]
    /// repetitions (`0x00422E6E PUSH 0xA`).
    pub fn script_ping(&mut self, player: i32, tile: (i32, i32)) -> SoundRequest {
        self.ping(tile, color_argb(player), SCRIPT_PING_REPEATS);
        SoundRequest::Event2d {
            name: SFX_SIGNAL.to_string(),
        }
    }
}

/// Goods per trade in the price tags (`Market_ConvertWoodGold(±100)` in `FUN_00560FF0`, GUID 490-493).
const TRADE_AMOUNT: i32 = 100;

/// Game values for the game tags of the text layout (`FUN_00560FF0`, re/game-ui N.9) from a running match.
/// Read-only: names via [`player_display_name`], prices from the economy, counters and timers from the state; in
/// addition the values of the extra tags of the tooltip (re/game-ui N.8, N.14.12): texts from `texts`, costs from the
/// value set, prerequisites from `Rules.toolbar`, mana of the first selected object from `selection`, formation and
/// headhunter price.
pub struct SimGameText<'a> {
    pub sim: &'a SimHost,
    pub texts: &'a Texts,
    /// Selection in selection order (`Selection_GetFirst` for the mana line of `[RESOURCES]`, O-243).
    pub selection: &'a [ObjectId],
    /// Selected formation of the world (`GameWorld+0x20B8`, `FUN_00477950`); -1 = unknown (`[FORMATION]` stays
    /// empty). The formation lives in the host (viewer `UnitBar`, in the client the bar host), not in the simulation.
    pub formation: i32,
}

impl<'a> SimGameText<'a> {
    /// Game values without selection and without formation (message bar, status bar).
    pub fn new(sim: &'a SimHost, texts: &'a Texts) -> Self {
        Self {
            sim,
            texts,
            selection: &[],
            formation: -1,
        }
    }

    /// The same game values with the selection (tooltip of the command bar).
    pub fn with_selection(self, selection: &'a [ObjectId]) -> Self {
        Self { selection, ..self }
    }

    /// The same game values with the selected formation.
    pub fn with_formation(self, formation: i32) -> Self {
        Self { formation, ..self }
    }
}

/// Player color as ARGB like the table `0x006189D0` (`[CR;COLOR %X]` of the headhunter tooltips), from the color
/// number `Player_GetColor`; numbers outside the table light gray.
fn color_argb(color: i32) -> u32 {
    nml_world::markers::marker_argb(color)
}

impl GameText for SimGameText<'_> {
    fn player_name(&self, slot: i32) -> String {
        player_display_name(self.sim, self.texts, slot)
    }

    /// `Market_ConvertWoodGold` `0x00477970` / `Market_ConvertFoodGold` `0x004779C0` with the global prices
    /// (`GameWorld+0x20BC`/`+0x20C0`): `+100` = price of 100 goods in gold, `-100` = goods for 100 gold.
    fn trade_cost(&self, tag: TradeCost) -> i32 {
        let economy = &self.sim.state.economy;
        let (price, n) = match tag {
            TradeCost::WoodGold => (economy.wood_price, TRADE_AMOUNT),
            TradeCost::FoodGold => (economy.food_price, TRADE_AMOUNT),
            TradeCost::GoldWood => (economy.wood_price, -TRADE_AMOUNT),
            TradeCost::GoldFood => (economy.food_price, -TRADE_AMOUNT),
        };
        nml_sim::economy::market_convert(price, n)
    }

    /// `Player_GetGuidCount` `0x0047DD90` (slot outside 0-7 -> 0, groups resolved).
    fn guid_count(&self, slot: i32, guid: i32) -> i32 {
        let sim = self.sim;
        nml_sim::economy::guid_count_in(&sim.state, &sim.rules, slot, guid as u32)
    }

    /// `FUN_0047E7F0` (slot outside 0-7 -> 0, groups resolved).
    fn kill_count(&self, slot: i32, guid: i32) -> i32 {
        let sim = self.sim;
        nml_sim::world::kill_count_in(&sim.state, &sim.rules, slot, guid as u32)
    }

    /// `FUN_00432B60(n)`: entry of the task (action 200 writes 1): if it is missing, `None`; value != 0
    /// `Some(true)`, value 0 `Some(false)`.
    fn task_status(&self, n: i32) -> Option<bool> {
        self.sim
            .state
            .script
            .mission
            .task_status
            .get(&n)
            .map(|&value| value != 0)
    }

    /// `FUN_00431580(n)`: remaining time of the timer in whole seconds (if it is missing, 0).
    fn countdown_seconds(&self, n: i32) -> i32 {
        nml_sim::game_end::countdown_seconds_in(&self.sim.state, n)
    }

    /// Text table via `Texts::get` (not `display`: that returns the last remembered missing-text number, O-4); the
    /// fallback rule "missing -> number" is done by `nml-ui` (`text_or_number`, `FUN_0055FFE0`).
    fn text(&self, id: i32) -> Option<String> {
        self.texts.get(id).map(str::to_string)
    }

    /// `GameEngine_GetLocalPlayer` -> `Player_GetSlotIndex`: the active slot of the host.
    fn local_slot(&self) -> i32 {
        i32::from(self.sim.active_player)
    }

    /// `GameWorld_GetObjectById(hid, 0)` -> `+0x54` (also dying objects).
    fn object_type(&self, hid: i32) -> Option<i32> {
        self.sim
            .state
            .object(ObjectId(hid as u32), false)
            .map(|o| o.guid as i32)
    }

    /// `[RESOURCES]` (`0x0043DEB0`): `Player_GetResource` 0, 1, 2 and the free living space R4 - R3 (R4 capped at the
    /// limit of the match like `Player_GetResource`); `None` without a player (`GameEngine_GetPlayer` = 0).
    fn available(&self, slot: i32) -> Option<[i32; 4]> {
        let state = &self.sim.state;
        let s = player_slot(state, slot)?;
        let r = |i| state.resource(s, i);
        Some([r(0), r(1), r(2), r(4).wrapping_sub(r(3))])
    }

    /// `GameWorld_GetTypeValueSet(slot, guid)` with `ValueSet_GetInt` 12 (gold), 11 (wood), 10 (food), 22 (living
    /// space) and 15 (mana); missing values 0, without a value set `None`.
    fn costs(&self, slot: i32, guid: i32) -> Option<[i32; 5]> {
        let set = self.sim.rules.type_value_set(slot, guid as u32)?;
        Some(TOOLTIP_COST_CONFIG.map(|cfg| set.get(&cfg).copied().unwrap_or(0)))
    }

    /// `Selection_GetFirst` -> `AttributeSet_GetCurrentValue(15)` with the upgrade percentages of the owner; without a
    /// selection (or without the object) `None`.
    fn first_selected_mana(&self) -> Option<i32> {
        let state = &self.sim.state;
        let o = state.object(*self.selection.first()?, false)?;
        let p = nml_sim::upgrades::type_upgrade_percent(state, o.owner(), o.guid, MANA);
        Some(o.attributes.current_value(MANA, p))
    }

    /// `GameWorld_LookupToolbarEntry` (`0x0047E440`): `REQUIRED_GUID` of the first button with this `BUTTONMSG`.
    fn requirements(&self, guid: i32) -> Option<[i32; 5]> {
        self.sim.rules.toolbar.lookup(guid).map(|b| b.required)
    }

    /// `[HEADHUNTERREWARD]` (re/headhunter-robbing 2.5, re/game-ui N.14.12): price `FUN_00438850`, then target
    /// `FUN_0042F7B0` and customer `FUN_0042F8B0` (the last insertion stands in front).
    /// - Price `FUN_0047DEE0`: for 4100 ... 4102 with an existing headhunter its `0x222` price
    ///   ([`nml_sim::command::headhunter::hire_price_in`]), otherwise config 12 of the value set at slot 8, without a
    ///   value set 500; red (`[COLOR FFFF0000]…[DEFAULTCOLOR]`) if the gold of the local player is smaller.
    /// - Target and customer only with an existing headhunter: answer to `0x222` (target player `[3]`, target GUID
    ///   `[4]`, customer `[5]`; with the module switched off the words stay 0 as in the original), player != -1 and
    ///   present -> `[CR;COLOR %X]` with the color of the player and the text of the target GUID (`FUN_0055FFE0`,
    ///   missing as a number) or the name of the customer (`Player+0xC`, here like `[PLAYERNAME]` via
    ///   [`player_display_name`]).
    fn headhunter_reward(&self, guid: i32) -> String {
        use nml_sim::module::headhunter as hunter;
        let state = &self.sim.state;
        let rules = &self.sim.rules;
        let art = hunter::GUIDS.iter().position(|&g| g as i32 == guid);
        let price = match art {
            Some(art) => nml_sim::command::headhunter::hire_price_in(state, rules, art),
            None => rules
                .type_value_set(8, guid as u32)
                .map_or(hunter::DEFAULT_PRICE, |set| {
                    set.get(&12).copied().unwrap_or(0)
                }),
        };
        let gold = state.resource(usize::from(self.sim.active_player), 0);
        let mut s = if gold < price {
            format!("[COLOR FFFF0000][ICON INFOICONS 0]{price}[DEFAULTCOLOR]")
        } else {
            format!("[ICON INFOICONS 0]{price}")
        };
        let Some(id) = art.and_then(|art| hunter::current(state, art)) else {
            return s;
        };
        // Answer to `0x222`: without an enabled module nobody answers, the words stay 0.
        let (target_player, target_guid, customer) = state
            .object(id, false)
            .and_then(|o| o.module(hunter::INDEX))
            .filter(|m| m.base.enabled)
            .and_then(|m| match &m.kind {
                nml_sim::module::ModuleKind::HeadHunter(h) => {
                    Some((h.target_player, h.target_guid, h.customer))
                }
                _ => None,
            })
            .unwrap_or((0, 0, 0));
        let color =
            |slot: i32| player_slot(state, slot).map(|s| color_argb(state.players[s].color));
        if target_player != -1
            && let Some(c) = color(target_player)
        {
            let name = nml_ui::text::text_or_number(self, target_guid);
            s.push_str(&format!("[CR;COLOR {c:X}]{name}"));
        }
        if customer != -1
            && let Some(c) = color(customer)
        {
            let name = player_display_name(self.sim, self.texts, customer);
            s.push_str(&format!("[CR;COLOR {c:X}]{name}"));
        }
        s
    }

    /// `FUN_00477950`: the selected formation of the world ([`SimGameText::formation`]).
    fn formation(&self) -> i32 {
        self.formation
    }
}

/// Mana (attribute 15).
const MANA: u32 = 15;

/// Values of the costs in the tooltip (`0x0043DEB0`): gold 12, wood 11, food 10, living space 22, mana 15 (the
/// simulation keeps a 0 instead of mana in `COST_CONFIG`).
const TOOLTIP_COST_CONFIG: [u32; 5] = [12, 11, 10, 22, MANA];

/// `GameEngine_GetPlayer(slot)` != 0: slot 0 ... 7 with a player entry.
fn player_slot(state: &nml_sim::state::SimState, slot: i32) -> Option<usize> {
    usize::try_from(slot)
        .ok()
        .filter(|&s| s < nml_sim::id::PLAYER_SLOTS && s < state.players.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::sim_host::plain_testkit::host;

    /// Screen of the tests.
    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    /// Empty text table (no corpus needed): `Texts::load` with an empty `Localisation` over an empty file system.
    fn empty_texts() -> Texts {
        let vfs = nml_rda::Vfs::from_archives(Vec::new());
        Texts::load(&vfs, &nml_formats::localisation::Localisation::default())
    }

    #[test]
    fn trade_costs_follow_the_market_prices_with_sign() {
        // `Market_ConvertWoodGold`/`…FoodGold`: +100 = price of 100 goods in gold, -100 = goods for 100 gold.
        let mut host = host();
        host.state.economy.wood_price = 1024; // 0.25 gold per wood (fixed point 20.12)
        host.state.economy.food_price = 1352;
        let texts = empty_texts();
        let values = SimGameText::new(&host, &texts);
        assert_eq!(values.trade_cost(TradeCost::WoodGold), 25);
        assert_eq!(values.trade_cost(TradeCost::FoodGold), 33);
        assert_eq!(
            values.trade_cost(TradeCost::GoldWood),
            nml_sim::economy::market_convert(1024, -100)
        );
        assert_eq!(
            values.trade_cost(TradeCost::GoldFood),
            nml_sim::economy::market_convert(1352, -100)
        );
        assert_eq!(values.trade_cost(TradeCost::GoldWood), 400);
        assert!(values.trade_cost(TradeCost::GoldFood) > 100);
    }

    #[test]
    fn task_status_distinguishes_zero_nonzero_and_missing() {
        // `FUN_00432B60`: if the entry is missing -> unknown, value != 0 -> done, value 0 -> open.
        let mut host = host();
        let mission = &mut host.state.script.mission;
        mission.task_status.insert(1, 1);
        mission.task_status.insert(2, 0);
        let texts = empty_texts();
        let values = SimGameText::new(&host, &texts);
        assert_eq!(values.task_status(1), Some(true));
        assert_eq!(values.task_status(2), Some(false));
        assert_eq!(values.task_status(3), None);
    }

    #[test]
    fn counters_countdowns_and_names_read_the_state() {
        let mut host = host();
        host.state.guid_counts[0].insert(1100, 5);
        host.state.kill_counts[1].insert(1100, 3);
        host.state.countdowns.insert(7, 12_345);
        host.state.players[0].name = "Anna".into();
        let texts = empty_texts();
        let values = SimGameText::new(&host, &texts);
        assert_eq!(values.guid_count(0, 1100), 5);
        assert_eq!(values.guid_count(8, 1100), 0, "slot outside 0-7");
        assert_eq!(values.kill_count(1, 1100), 3);
        assert_eq!(values.kill_count(-1, 1100), 0);
        assert_eq!(values.countdown_seconds(7), 12);
        assert_eq!(values.countdown_seconds(8), 0);
        // Names via the trait: slot 0 raw (free play/without text ID), outside 0-7 empty.
        assert_eq!(values.player_name(0), "Anna");
        for slot in [-1, 8, 255] {
            assert_eq!(values.player_name(slot), "", "slot {slot}");
        }
    }

    /// `statusbar.ini` as in the shipped data (resources 0 and 3, name).
    const STATUS_INI: &str = "STATUSSIZE 26\r\nSTATUSCOLOR 2 22 35\r\nSTATUSFONTCOLOR 255 255 255\r\n\
ICONSIZE 32 32\r\nICONFILE data/icons/resourcen_icons.tga\r\n\
RESOURCE 0 1 1 33 5\r\nRESOURCE 3 360 1 393 5\r\nPLAYERNAME 480 1 480 5\r\n";

    /// Archive with only `statusbar.ini` ([`STATUS_INI`]).
    fn status_archive() -> (tempfile::TempDir, nml_rda::Vfs) {
        crate::testutil::install(&[(STATUSBAR_PATH, STATUS_INI.as_bytes().to_vec())])
    }

    fn new_hud(options: &HudOptions) -> Hud {
        Hud::new(
            &parse_messages(MESSAGES),
            &empty_texts_map(),
            options,
            SCREEN,
        )
    }

    fn empty_texts_map() -> BTreeMap<i32, String> {
        BTreeMap::new()
    }

    #[test]
    fn status_bar_exists_only_with_the_switch_and_the_file() {
        // `IsStatusBar 0`: read nothing, no bar; with switch and file: bar, visible.
        let (_dir, vfs) = status_archive();
        let mut options = HudOptions::default();
        let mut hud = new_hud(&options);
        assert!(hud.status.is_none(), "Hud::new reads nothing");
        hud.load_status_bar(&vfs, &options);
        let bar = hud.status.as_ref().expect("bar");
        assert!(bar.shown);
        assert_eq!(bar.config.size, 26);
        assert_eq!(bar.config.entries.len(), 3);
        options.switches.status_bar = false;
        hud.load_status_bar(&vfs, &options);
        assert!(hud.status.is_none(), "switch off removes the bar");
        // File missing: warning, no bar.
        options.switches.status_bar = true;
        let empty = nml_rda::Vfs::from_archives(Vec::new());
        hud.load_status_bar(&empty, &options);
        assert!(hud.status.is_none());
    }

    #[test]
    fn status_values_read_the_active_slot() {
        // Resources 0-4 of the active slot (fixed point 20.12 -> integer), name of the slot raw in free play.
        let mut host = host();
        host.state.players[0].resources_fx = [100 << 12, -5 << 12, 500_000 << 12, 7 << 12, 9 << 12];
        host.state.players[0].name = "Anna".into();
        host.state.economy.max_living_space = 200;
        let texts = empty_texts();
        host.active_player = 0;
        let v = Hud::status_values(&host, &texts);
        assert_eq!(v.resources, [100, -5, 500_000, 7, 9]);
        assert_eq!(v.name, "Anna");
        // Living space is capped at the limit of the match like `Player_GetResource`.
        host.state.economy.max_living_space = 5;
        assert_eq!(Hud::status_values(&host, &texts).resources[4], 5);
        // Another active slot reads its own values.
        host.state.players[1].resources_fx[0] = 42 << 12;
        host.active_player = 1;
        assert_eq!(Hud::status_values(&host, &texts).resources[0], 42);
    }

    use nml_formats::messages::parse_messages;
    use nml_sim::id::ObjectId;

    /// Display kind `MILITARY` with the attack messages 100/101 (`SOUNDONCE 872`) and the type message 1000
    /// (`SOUNDONCE 873`) as in `messages.ini`; plus `ECONOMY` with 0 (`SOUND 877`) and 4 (`PING`).
    const MESSAGES: &str = "MSGTAG ECONOMY
POSITION 0 400
COLOR E000FF00
/MSGTAG
MSGTAG MILITARY
POSITION 0 500
COLOR E0FF0000
/MSGTAG
MSGID 0
MSGTYPE ECONOMY
SOUND 877
TEXT 17000
/MSGID
MSGID 4
MSGTYPE ECONOMY
PING
TEXT 17004
/MSGID
MSGID 100
MSGTYPE MILITARY
TEXT 17100
SOUNDONCE 872
/MSGID
MSGID 101
MSGTYPE MILITARY
TEXT 17101
SOUNDONCE 872
/MSGID
MSGID 1000
MSGTYPE MILITARY
TEXT 17110
SOUNDONCE 873
/MSGID
MSGID 1001
TEXT 17110
/MSGID
";

    fn synthetic_hud() -> Hud {
        let texts: BTreeMap<i32, String> = [
            (17000, "WIR HABEN NICHT GENUG NAHRUNG!"),
            (17004, "FELD LEER"),
            (17100, "Einheit"),
            (17101, "Gebäude"),
            (17110, "Typ"),
        ]
        .into_iter()
        .map(|(id, t)| (id, t.to_string()))
        .collect();
        Hud::new(
            &parse_messages(MESSAGES),
            &texts,
            &HudOptions::default(),
            SCREEN,
        )
    }

    fn message(to: Option<u8>, id: i32, tile: (i32, i32)) -> SimEvent {
        SimEvent::PlayerMessage {
            to,
            id,
            tile,
            flags: flags::DEPOSIT,
        }
    }

    fn attack(owner: u8, guid: u32, building: bool) -> SimEvent {
        SimEvent::UnderAttack {
            object: ObjectId(1),
            owner,
            guid,
            building,
            tile: (1, 1),
        }
    }

    /// MSGIDs of the heads per kind.
    fn heads(hud: &Hud) -> Vec<i32> {
        hud.messages.heads().map(|(_, m)| m.id).collect()
    }

    #[test]
    fn messages_reach_the_active_player_or_everyone() {
        // To slot 1: not at the active slot 0; to slot 0 and to everyone (`to: None`): yes.
        let mut hud = synthetic_hud();
        let mut sounds = Vec::new();
        hud.handle(&[message(Some(1), 0, NO_TILE)], 0, &mut sounds);
        assert!(heads(&hud).is_empty() && sounds.is_empty());
        hud.handle(&[message(None, 0, NO_TILE)], 0, &mut sounds);
        assert_eq!(heads(&hud), [0]);
        assert_eq!(sounds, [SoundRequest::Message(877)]);
        hud.handle(&[message(Some(0), 0, (2, 2))], 0, &mut sounds);
        assert_eq!(hud.messages.queue(0).len(), 2, "queued behind the head");
        // The same `to: None` appears at every active slot.
        for active in 0..8 {
            let mut hud = synthetic_hud();
            hud.handle(&[message(None, 4, NO_TILE)], active, &mut Vec::new());
            assert_eq!(heads(&hud), [4], "slot {active}");
        }
    }

    #[test]
    fn three_hits_in_a_row_show_one_attack_line_with_one_sound() {
        let mut hud = synthetic_hud();
        let hit = attack(0, 1107, false);
        let mut sounds = Vec::new();
        hud.handle(&[hit.clone(), hit.clone(), hit.clone()], 0, &mut sounds);
        // Without 0x10000: as long as the kind is occupied, further hits drop out together with the sound (O-42).
        assert_eq!(sounds, [SoundRequest::Message(872)]);
        assert_eq!(hud.messages.queue(1).len(), 1);
        // After expiry (unit: 7 * 55 ms + 1500 ms game time, speed 10 = 1:1) a line again, but before 15 s without
        // sound (SOUNDONCE lock per kind).
        for _ in 0..3 {
            hud.tick(1000.0, 10, &mut sounds);
        }
        assert!(hud.messages.queue(1).is_empty());
        hud.handle(std::slice::from_ref(&hit), 0, &mut sounds);
        assert_eq!(hud.messages.queue(1).len(), 1);
        assert_eq!(sounds.len(), 1);
        // Foreign owner: nothing.
        let mut hud = synthetic_hud();
        hud.handle(&[attack(1, 1107, false)], 0, &mut sounds);
        assert!(heads(&hud).is_empty());
    }

    #[test]
    fn under_attack_falls_back_to_100_and_101() {
        // Type GUID first (1000 has a def), otherwise - also with a def without MSGTYPE (1001) - 101 for buildings or
        // 100 for units; the substitute message comes only on return value 0, not on 3 (occupied).
        for (guid, building, id, sound) in [
            (1000, true, 1000, 873),
            (1001, true, 101, 872),
            (4711, true, 101, 872),
            (1107, false, 100, 872),
            (u32::MAX, false, 100, 872),
        ] {
            let mut hud = synthetic_hud();
            let mut sounds = Vec::new();
            hud.handle(&[attack(0, guid, building)], 0, &mut sounds);
            assert_eq!(heads(&hud), [id], "GUID {guid}");
            assert_eq!(sounds, [SoundRequest::Message(sound)], "GUID {guid}");
            let head = &hud.messages.queue(1)[0];
            assert_eq!((head.flags & 0xFFFF_0000, head.tile), (0x0100_0000, (1, 1)));
            // Occupied: return value 3, no substitute message.
            hud.handle(&[attack(0, 4711, true)], 0, &mut sounds);
            assert_eq!(hud.messages.queue(1).len(), 1);
            assert_eq!(sounds.len(), 1);
        }
    }

    #[test]
    fn pings_put_their_tile_on_the_action_stack() {
        // Message 4 (PING) with location: location onto the action stack (O-219), without location not; only on
        // appearing.
        let mut hud = synthetic_hud();
        let mut sounds = Vec::new();
        hud.handle(&[message(Some(0), 4, (3, 4))], 0, &mut sounds);
        hud.handle(&[message(Some(0), 4, (5, 6))], 0, &mut sounds);
        hud.handle(&[message(Some(0), 4, NO_TILE)], 0, &mut sounds);
        let stack: Vec<(i32, i32)> = hud.actions.tiles().iter().copied().collect();
        assert_eq!(stack, [(3, 4)], "queued ones ping only on appearing");
        assert!(sounds.is_empty());
        // The same path sets the minimap ping with 5 repetitions.
        let pings: Vec<_> = hud
            .pings
            .list()
            .iter()
            .map(|p| (p.tile, p.repeats))
            .collect();
        assert_eq!(pings, [((3, 4), 5)]);
        hud.clear();
        assert!(hud.actions.tiles().is_empty() && heads(&hud).is_empty());
        assert!(hud.pings.list().is_empty(), "pings not in the savegame");
    }

    /// Script action 7: `SFX_SIGNAL` 2D, ping with `0x006189D0[player]` and 10 repetitions, the location in front on
    /// the stack; a second one at the same tile only puts down the location (O-41).
    #[test]
    fn script_ping_plays_the_signal_and_pings_ten_times() {
        let mut hud = synthetic_hud();
        let sound = hud.script_ping(9, (7, 8));
        assert_eq!(
            sound,
            SoundRequest::Event2d {
                name: "SFX_SIGNAL".into()
            }
        );
        hud.script_ping(2, (7, 8));
        let pings: Vec<_> = hud
            .pings
            .list()
            .iter()
            .map(|p| (p.tile, p.color, p.repeats))
            .collect();
        assert_eq!(pings, [((7, 8), nml_world::markers::marker_argb(9), 10)]);
        let stack: Vec<(i32, i32)> = hud.actions.tiles().iter().copied().collect();
        assert_eq!(stack, [(7, 8), (7, 8)]);
        // E-244 (no load check): players outside the table (-1, 20) give light gray (slot 8), without a panic.
        let mut hud = synthetic_hud();
        hud.script_ping(20, (1, 1));
        hud.script_ping(-1, (2, 2));
        let colors: Vec<u32> = hud.pings.list().iter().map(|p| p.color).collect();
        assert_eq!(colors, [nml_world::markers::marker_argb(8); 2]);
    }

    /// The event of the signal `0x5C` (O-321) for the slot `player` with the color number `color`.
    fn signal(player: i32, color: i32, mask: u32, tile: (i32, i32)) -> SimEvent {
        SimEvent::Signal {
            player,
            color,
            mask,
            tile,
        }
    }

    /// (`0x0048D4D6`): bit of the local slot in the mask -> `SFX_SIGNAL` 2D and a ping with one repetition in the
    /// color of the slot field, the location in front on the stack.
    #[test]
    fn signal_for_me_plays_the_sound_and_pings_once() {
        let mut hud = synthetic_hud();
        let mut out = Vec::new();
        hud.handle(&[signal(3, 3, 0b101, (70, 101))], 2, &mut out);
        assert_eq!(
            out,
            [SoundRequest::Event2d {
                name: "SFX_SIGNAL".into()
            }]
        );
        let pings: Vec<_> = hud
            .pings
            .list()
            .iter()
            .map(|p| (p.tile, p.color, p.repeats))
            .collect();
        assert_eq!(pings, [((70, 101), nml_world::markers::marker_argb(3), 1)]);
        assert_eq!(hud.actions.tiles().front(), Some(&(70, 101)));
    }

    /// Without the bit of the local slot in the mask nothing happens (no stack either); a slot beyond the mask width
    /// is not one of the recipients either.
    #[test]
    fn signal_not_for_me_does_nothing() {
        let mut hud = synthetic_hud();
        let mut out = Vec::new();
        hud.handle(&[signal(3, 3, 0b101, (70, 101))], 1, &mut out);
        hud.handle(&[signal(3, 3, u32::MAX, (70, 101))], 32, &mut out);
        hud.handle(&[signal(3, 3, 0, (70, 101))], 0, &mut out);
        assert!(out.is_empty());
        assert!(hud.pings.list().is_empty() && hud.actions.tiles().is_empty());
    }

    /// If a ping is already running at the tile, the second signal brings only sound and stack (O-41); the color
    /// stays that of the first.
    #[test]
    fn second_signal_during_ring_only_sound_and_stack() {
        let mut hud = synthetic_hud();
        let mut out = Vec::new();
        hud.handle(&[signal(1, 1, 1, (5, 6))], 0, &mut out);
        hud.handle(&[signal(2, 2, 1, (5, 6))], 0, &mut out);
        assert_eq!(out.len(), 2, "the sound both times");
        let pings: Vec<_> = hud.pings.list().iter().map(|p| (p.tile, p.color)).collect();
        assert_eq!(pings, [((5, 6), nml_world::markers::marker_argb(1))]);
        let stack: Vec<_> = hud.actions.tiles().iter().copied().collect();
        assert_eq!(stack, [(5, 6), (5, 6)]);
    }

    #[test]
    fn counter_upgrades_and_tributes_reach_everyone_with_their_players() {
        let mut hud = Hud::new(
            &parse_messages(concat!(
                "MSGTAG OTHER\nCOLOR E00000FF\n/MSGTAG\n",
                "MSGID 3370\nMSGTYPE OTHER\nSOUND 876\nTEXT 17250\n/MSGID\n",
                "MSGID 230\nMSGTYPE OTHER\nTEXT 17240\n/MSGID\n",
            )),
            &[
                (
                    17250,
                    "[PLAYERNAMEPLID1] hat die Pest gestartet.".to_string(),
                ),
                (
                    17240,
                    "[PLAYERNAMEPLID1] zahlt [PLAYERNAMEPLID2] Tribut!".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &HudOptions::default(),
            SCREEN,
        );
        let mut sounds = Vec::new();
        hud.handle(
            &[
                SimEvent::CounterUpgradeStarted {
                    guid: 3370,
                    player: 1,
                    tile: (3, 4),
                },
                SimEvent::TributeSent {
                    giver: 2,
                    recipient: 3,
                },
            ],
            0,
            &mut sounds,
        );
        let queue: Vec<(i32, u32, (i32, i32), u16)> = hud
            .messages
            .queue(0)
            .iter()
            .map(|m| (m.id, m.flags, m.tile, m.players))
            .collect();
        // 3370: `slot | 0x10000` together with the SOUND of the def; 230: `0x10000 | 3 << 8 | 2`, without location,
        // queued.
        assert_eq!(
            queue,
            [
                (3370, 0x1_0401, (3, 4), 0x0001),
                (230, 0x1_0302, NO_TILE, 0x0302)
            ]
        );
        assert_eq!(sounds, [SoundRequest::Message(876)]);
        let mut host = crate::game::sim_host::plain_testkit::host();
        for (slot, name) in [(1, "Anna"), (2, "Bert"), (3, "Cora")] {
            host.state.players[slot].name = name.into();
        }
        let texts = empty_texts();
        let values = SimGameText::new(&host, &texts);
        assert_eq!(
            hud.messages.lines(&values)[0].0,
            "Anna hat die Pest gestartet."
        );
        // 41 characters = 2255 ms display; then remainder immediately 0 (230 waits), in the fifth tick 230 appears.
        sounds.clear();
        for _ in 0..5 {
            hud.tick(1000.0, 10, &mut sounds);
        }
        assert_eq!(hud.messages.lines(&values)[0].0, "Bert zahlt Cora Tribut!");
        assert!(sounds.is_empty(), "230 without sound for recipient 3");
    }

    #[test]
    fn game_speed_scales_the_bar() {
        // "WIR HABEN NICHT GENUG NAHRUNG!" = 30 characters: 1650 ms display + 1500 ms fade-out game time. Frames of
        // 20 ms, game time per frame `20 · (speed + 10) / 20`: speed 10 -> 20 ms (83 frames display until < 1, 74
        // frames fade-out until remainder <= 20, one to remove), speed 0 -> 10 ms (165 + 149 + 1), speed 19 -> 29 ms
        // (57 + 51 + 1).
        for (speed, expected) in [(10u8, 158usize), (0, 315), (19, 109)] {
            let mut hud = synthetic_hud();
            hud.handle(&[message(Some(0), 0, NO_TILE)], 0, &mut Vec::new());
            let mut frames = 0;
            while !hud.messages.queue(0).is_empty() {
                hud.tick(20.0, speed, &mut Vec::new());
                frames += 1;
                assert!(frames < 10_000);
            }
            assert_eq!(frames, expected, "speed {speed}");
        }
    }

    /// Original installation (silently skipped without it): messages, texts and the text table of the text layout.
    fn corpus() -> Option<(Hud, Texts)> {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return None;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let notices = crate::game::notices::Notices::load(&vfs);
        let hud = Hud::load(&vfs, &notices, &HudOptions::default(), SCREEN);
        let loc = nml_ui::assets::load_localisation(&vfs).unwrap();
        Some((hud, Texts::load(&vfs, &loc)))
    }

    #[test]
    fn tribute_to_slot_4_plays_the_inherited_sound() {
        // O-216: byte 1 of the flag word lies on 0x400 (SOUND) ... 0x1000 (SOUNDONCE): recipients 4-7 set SOUND, and
        // 230 plays the sound field 925 inherited from block 228; recipients 0-3 stay silent.
        let Some((mut hud, _)) = corpus() else {
            return;
        };
        for recipient in 0..8u8 {
            hud.clear();
            let mut sounds = Vec::new();
            let tribute = SimEvent::TributeSent {
                giver: if recipient == 0 { 1 } else { 0 },
                recipient,
            };
            hud.handle(&[tribute], 2, &mut sounds);
            let expected: &[SoundRequest] = if recipient >= 4 {
                &[SoundRequest::Message(925)]
            } else {
                &[]
            };
            assert_eq!(sounds, expected, "recipient {recipient}");
            assert_eq!(heads(&hud), [TRIBUTE_MESSAGE], "recipient {recipient}");
        }
    }

    #[test]
    fn installation_messages_have_their_texts() {
        // Every message with a kind has its text in `texte.deu` (the fallback to the number, O-241, never applies in
        // the shipped data).
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let notices = crate::game::notices::Notices::load(&vfs);
        let missing: Vec<i32> = notices
            .messages
            .defs
            .iter()
            .filter(|(_, d)| notices.messages.tag_of(d).is_some())
            .filter(|(_, d)| !notices.texts.contains_key(&d.text))
            .map(|(&id, _)| id)
            .collect();
        assert!(missing.is_empty(), "without text: {missing:?}");
    }

    #[test]
    fn defeat_message_names_the_slot() {
        // O-209: 200 + p (`GameEngine_SetPlayerResult` to everyone except p, flag `0x04010000`) names slot p via
        // `[PLAYERNAME n]`.
        let Some((mut hud, texts)) = corpus() else {
            return;
        };
        let mut host = crate::game::sim_host::plain_testkit::host();
        host.state.players[3].name = "Bob".into();
        let event = SimEvent::PlayerMessage {
            to: Some(0),
            id: 203,
            tile: NO_TILE,
            flags: flags::GAME_END,
        };
        let mut sounds = Vec::new();
        hud.handle(&[event], 0, &mut sounds);
        let raw = &hud.messages.heads().next().expect("message 203").1.text;
        assert!(raw.contains("[PLAYERNAME 3]"), "{raw}");
        let values = SimGameText::new(&host, &texts);
        let line = &hud.messages.lines(&values)[0].0;
        assert!(line.contains("Bob") && !line.contains('['), "{line}");
    }

    #[test]
    fn the_original_counter_upgrade_messages_name_the_player() {
        let Some((mut hud, texts)) = corpus() else {
            return;
        };
        let mut host = crate::game::sim_host::plain_testkit::host();
        host.state.players[2].name = "Cora".into();
        let values = SimGameText::new(&host, &texts);
        for guid in 3370..=3377u32 {
            hud.clear();
            hud.handle(
                &[SimEvent::CounterUpgradeStarted {
                    guid,
                    player: 2,
                    tile: (0, 0),
                }],
                0,
                &mut Vec::new(),
            );
            let line = &hud.messages.lines(&values)[0].0;
            assert!(line.starts_with("Cora ") && !line.contains('['), "{line}");
        }
    }

    // --- Integration ---------------------------------------------------------------------------------------------

    /// Installation with a language file, the fonts `InfoMessages` and `StatusFont` (every character 10 wide) and
    /// `statusbar.ini` ([`STATUS_INI`]).
    fn ui_install() -> (tempfile::TempDir, nml_rda::Vfs) {
        use crate::testutil::{fnt, install, tga, utf16};
        let loc = "TextDatei data\\txt\\t.txt\n\
                   Font InfoMessages\nFontSize 15\nTextureFont\nFontName Test\n\
                   Font StatusFont\nFontSize 15\nTextureFont\nFontName Test\n";
        install(&[
            ("data/txt/localisation.deu", loc.as_bytes().to_vec()),
            ("data/txt/t.txt", utf16("1=AB\r\n")),
            ("data/fonts/Test_15_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_15_0_0_0_0.tga", tga(256, 256)),
            (STATUSBAR_PATH, STATUS_INI.as_bytes().to_vec()),
        ])
    }

    #[test]
    fn status_bar_first_gives_the_toolbar_its_32_grid() {
        // O-280: the status bar (`0x004492BC`) loads `resourcen_icons.tga` before the command bar (`0x00449342`) with
        // `ICONSIZE 32 32`; the texture cache searches by name, the 24 x 24 of the bar are lost. Without a status bar
        // (`IsStatusBar 0`) the grid of the bar applies.
        let (_dir, vfs) = ui_install();
        let notices = crate::game::notices::Notices::load(&vfs);
        let grid = |options: &HudOptions| {
            let hud = Hud::load(&vfs, &notices, options, SCREEN);
            hud.toolbar.expect("bar").data.resource_icons.tile
        };
        assert_eq!(grid(&HudOptions::default()), [32, 32]);
        let off = EngineConfig {
            is_status_bar: 0,
            ..EngineConfig::default()
        };
        assert_eq!(grid(&HudOptions::from_config(&off)), [24, 24]);
    }

    /// Hud from [`MESSAGES`] with the texts of [`synthetic_hud`], the switches `options` and the status bar from
    /// `vfs`.
    fn loaded_hud(vfs: &nml_rda::Vfs, options: &HudOptions) -> Hud {
        let mut hud = synthetic_hud();
        hud.load_status_bar(vfs, options);
        hud
    }

    #[test]
    fn hud_draws_messages_before_windows() {
        // As in the original: the messages in the world pass (`0x00438F2A`), then the windows of the tree
        // (`0x004529A0`); the status bar begins with its rectangle without a texture.
        let (_dir, vfs) = ui_install();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
        let options = HudOptions::default();
        let mut hud = loaded_hud(&vfs, &options);
        hud.handle(&[message(Some(0), 0, NO_TILE)], 0, &mut Vec::new());
        let host = host();
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        let mut ctx = layer.context(&vfs, SCREEN);
        let mut all = Vec::new();
        hud.draw(&mut ctx, &game, &values, None, None, &mut all);
        let mut messages = Vec::new();
        hud.draw_world_pass(&ctx, &game, &mut messages);
        let mut windows = Vec::new();
        hud.draw_windows(&mut ctx, &game, &values, None, None, &mut windows);
        assert!(!messages.is_empty() && !windows.is_empty());
        assert_eq!(all, [messages.clone(), windows.clone()].concat());
        let DrawCmd::Quad { texture, dst, .. } = windows[0] else {
            unreachable!()
        };
        assert_eq!((texture, dst), (None, [0, 0, 1024, 26]), "status bar");
        assert!(
            messages
                .iter()
                .all(|c| matches!(c, DrawCmd::Quad { texture, .. } if texture.is_some())),
            "before the bar only characters of the message"
        );
    }

    /// Draw order of the tree (re/game-ui N.14.2): status bar, command bar (background, portrait, counter-upgrades,
    /// buttons), **last** the tooltip window (layer 1); the box lies on the left with lower edge `H − 200`. A hidden
    /// bar is missing from the sequence, the tooltip stays.
    #[test]
    fn tooltip_draws_last() {
        use nml_formats::toolbar_cfg::ToolbarIcon;
        use nml_ui::hud::toolbar::button::TooltipRequest;
        use nml_ui::hud::toolbar::data::Sheet;
        let (_dir, vfs) = ui_install();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
        let mut hud = loaded_hud(&vfs, &HudOptions::default());
        let mut data = ToolbarData::from_texts("", "");
        data.sheets = vec![Sheet::new("data/fonts/Test_15_0_0_0_0.tga", [128, 128])];
        data.background = vec![ToolbarIcon {
            pos: [0, 0],
            sheet: 0,
            tile: 0,
        }];
        hud.toolbar = Some(Toolbar::new(data, SCREEN));
        let host = host();
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        let model = ToolbarModel::new(&host.rules.toolbar);
        let portrait = PortraitModel::default();
        let keys = TextContext::default();
        let bar = BarFrame {
            model: &model,
            portrait: &portrait,
            game: &game,
            keys: &keys,
        };
        let draw = |hud: &Hud, layer: &mut crate::ui_layer::UiLayer| {
            let mut out = Vec::new();
            hud.draw_windows(
                &mut layer.context(&vfs, SCREEN),
                &game,
                &values,
                Some(&bar),
                None,
                &mut out,
            );
            out
        };
        let plain = draw(&hud, &mut layer);
        let toolbar_top = SCREEN.height - 176;
        assert!(
            plain
                .iter()
                .any(|c| matches!(c, DrawCmd::Quad { dst, .. } if dst[1] == toolbar_top)),
            "background of the bar drawn"
        );
        assert_eq!(hud.tooltip_box.get(), None);
        hud.tooltip.set(TooltipRequest {
            guid: 20,
            kind: 0x1700_0000,
            value: 1.0,
            required: [0; 5],
        });
        let with = draw(&hud, &mut layer);
        assert!(with.len() > plain.len());
        assert_eq!(
            with[..plain.len()],
            plain[..],
            "everything before the tooltip unchanged"
        );
        let rect = hud.tooltip_box.get().expect("tooltip box");
        assert_eq!((rect[0], rect[3]), (0, SCREEN.height - 200));
        let DrawCmd::Quad { texture, dst, .. } = with[plain.len()] else {
            unreachable!()
        };
        assert_eq!(
            (texture, dst),
            (None, rect),
            "the box follows first after all windows"
        );
        // Hidden bar (event `0x11`): its node is missing from the draw order, the tooltip comes anyway.
        hud.toolbar.as_mut().unwrap().shown = false;
        hud.sync_toolbar_visibility();
        let hidden = draw(&hud, &mut layer);
        assert!(
            !hidden
                .iter()
                .any(|c| matches!(c, DrawCmd::Quad { dst, .. } if dst[1] == toolbar_top))
        );
        assert!(hud.tooltip_box.get().is_some());
    }

    /// (re/campaign-ui D6.4): the cinemascope area is the last child of the root (`0x004491F0`); the tree draws it
    /// after status bar and command bar and before the tooltip (layer 1, above the bars).
    #[test]
    fn hud_draws_cinemascope_after_the_toolbar_and_before_the_tooltip() {
        use nml_formats::toolbar_cfg::ToolbarIcon;
        use nml_ui::hud::cinemascope::{BAR_COLOR, CinemascopeData};
        use nml_ui::hud::toolbar::button::TooltipRequest;
        use nml_ui::hud::toolbar::data::Sheet;
        let (_dir, vfs) = ui_install();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
        let mut hud = loaded_hud(&vfs, &HudOptions::default());
        let mut data = ToolbarData::from_texts("", "");
        data.sheets = vec![Sheet::new("data/fonts/Test_15_0_0_0_0.tga", [128, 128])];
        data.background = vec![ToolbarIcon {
            pos: [0, 0],
            sheet: 0,
            tile: 0,
        }];
        hud.toolbar = Some(Toolbar::new(data, SCREEN));
        hud.cinemascope_data = CinemascopeData::from_texts("BARSIZE 170\n", "", "");
        hud.cinemascope.message(
            &HudEvent::with_args(HudEventKind::Cinemascope, &[1]),
            &hud.cinemascope_data,
        );
        hud.tooltip.set(TooltipRequest {
            guid: 20,
            kind: 0x1700_0000,
            value: 1.0,
            required: [0; 5],
        });
        let host = host();
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        let model = ToolbarModel::new(&host.rules.toolbar);
        let portrait = PortraitModel::default();
        let keys = TextContext::default();
        let bar = BarFrame {
            model: &model,
            portrait: &portrait,
            game: &game,
            keys: &keys,
        };
        let mut out = Vec::new();
        hud.draw_windows(
            &mut layer.context(&vfs, SCREEN),
            &game,
            &values,
            Some(&bar),
            None,
            &mut out,
        );
        let find = |pred: &dyn Fn(&DrawCmd) -> bool| out.iter().position(pred);
        let status = find(&|c| matches!(c, DrawCmd::Quad { dst, .. } if *dst == [0, 0, 1024, 26]));
        let toolbar_top = SCREEN.height - 176;
        let toolbar = out
            .iter()
            .rposition(|c| matches!(c, DrawCmd::Quad { dst, .. } if dst[1] == toolbar_top));
        // Upper bar (0, 0)-(1024, py(170)) = 127 rows at 768.
        let bars = find(&|c| {
            matches!(c, DrawCmd::Quad { texture: None, dst, color, .. }
                if *dst == [0, 0, 1024, 127] && *color == BAR_COLOR)
        });
        let rect = hud.tooltip_box.get().expect("tooltip box");
        let tooltip =
            find(&|c| matches!(c, DrawCmd::Quad { texture: None, dst, .. } if *dst == rect));
        let (Some(status), Some(toolbar), Some(bars), Some(tooltip)) =
            (status, toolbar, bars, tooltip)
        else {
            panic!("everything drawn: {status:?} {toolbar:?} {bars:?} {tooltip:?}");
        };
        assert!(
            status < toolbar && toolbar < bars && bars < tooltip,
            "status bar {status}, bar {toolbar}, bars {bars}, tooltip {tooltip}"
        );
    }

    #[test]
    fn hud_respects_engine_ini_switches() {
        // `IsStatusBar`, `IsMap`, `IsToolBar` from the `Engine.ini` (`0x004491F0`): only enabled windows, the status
        // bar only with the switch (then from `statusbar.ini`, height `STATUSSIZE`), cinemascope always.
        let (_dir, vfs) = ui_install();
        let kinds = |hud: &Hud| -> Vec<&str> {
            hud.windows
                .ids()
                .map(|id| hud.windows.kind(id).name())
                .collect()
        };
        let off = EngineConfig {
            is_status_bar: 0,
            is_map: 0,
            is_tool_bar: 0,
            ..EngineConfig::default()
        };
        let hud = loaded_hud(&vfs, &HudOptions::from_config(&off));
        assert_eq!(kinds(&hud), ["base", "root", "cinemascope"]);
        // `EnhancedTooltips` (default 1) for the tooltip window.
        assert!(HudOptions::default().enhanced_tooltips);
        let plain = EngineConfig {
            enhanced_tooltips: 0,
            ..EngineConfig::default()
        };
        assert!(!HudOptions::from_config(&plain).enhanced_tooltips);
        assert!(hud.status.is_none());
        let host = host();
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
        let mut out = Vec::new();
        hud.draw_windows(
            &mut layer.context(&vfs, SCREEN),
            &game,
            &values,
            None,
            None,
            &mut out,
        );
        assert!(
            out.is_empty(),
            "without a status bar no window with content"
        );

        let status_only = EngineConfig {
            is_status_bar: 1,
            ..off
        };
        let mut hud = loaded_hud(&vfs, &HudOptions::from_config(&status_only));
        assert_eq!(kinds(&hud), ["base", "root", "status_bar", "cinemascope"]);
        let bar = hud.windows.find(WindowKind::StatusBar).unwrap();
        assert_eq!(hud.windows.rect(bar), [0, 0, 1024, 26]);
        hud.draw_windows(
            &mut layer.context(&vfs, SCREEN),
            &game,
            &values,
            None,
            None,
            &mut out,
        );
        assert!(!out.is_empty());

        // New size: rectangles anew, visibility stays.
        hud.windows.set_visible(bar, false);
        hud.resize(Screen {
            width: 800,
            height: 600,
        });
        let bar = hud.windows.find(WindowKind::StatusBar).unwrap();
        assert_eq!(hud.windows.rect(bar), [0, 0, 800, 26]);
        assert!(!hud.windows.visible(bar));

        let all = loaded_hud(&vfs, &HudOptions::from_config(&EngineConfig::default()));
        assert_eq!(
            kinds(&all),
            [
                "base",
                "root",
                "status_bar",
                "minimap",
                "toolbar",
                "cinemascope",
                "tooltip"
            ]
        );
    }

    /// Draws the game windows with the data of the minimap for `screen` (start camera, map 128) and returns the
    /// commands with the layout of the frame.
    fn draw_with_minimap(
        hud: &Hud,
        vfs: &nml_rda::Vfs,
        screen: Screen,
    ) -> (Vec<DrawCmd>, crate::game::minimap::MinimapLayout) {
        use crate::game::minimap::draw::{self, tests::start_camera};
        let host = host();
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        let mut layer = crate::ui_layer::UiLayer::new(vfs, 1280, None).unwrap();
        let mut ctx = layer.context(vfs, screen);
        let size = draw::screen_size(screen);
        let layout = draw::layout(
            size,
            &start_camera(glam::Vec3::new(8192.0, 0.0, 8192.0)),
            (128, 128),
        );
        let frame = crate::game::minimap::MinimapFrame {
            layout,
            dots: Vec::new(),
            pings: Vec::new(),
            camera: [[0.0; 2]; 4],
            texture: hud.minimap.as_ref().unwrap().texture(ctx.assets),
        };
        let mut out = Vec::new();
        hud.draw_windows(&mut ctx, &game, &values, None, Some(&frame), &mut out);
        // Only the commands with the fog texture of the minimap.
        let fog = ctx.assets.find(draw::FOG_TEXTURE);
        out.retain(|c| matches!(c, DrawCmd::Quad4 { texture, .. } if *texture == fog));
        (out, layout)
    }

    /// New window size - 1024 x 768, 800 x 600, 1280 x 1024. Node and diamond keep their fixed pixel layout
    /// (`(W − 176, H − 176, W, H)`, center `(W − 89, H − 89)`); terrain and fog buffer are kept, the map is not rebuilt
    /// from the level.
    #[test]
    fn minimap_survives_resize_and_follows_m1() {
        use crate::game::minimap::draw::tests::{minimap, ui_install};
        let (_dir, vfs) = ui_install();
        let mut hud = synthetic_hud();
        let mut map = minimap(128);
        // Immediate setup with "explored": the fog differs from the unfogged terrain.
        map.sync(0, 1, 0, |_, _| 1);
        let front = map.fog().front().clone();
        assert_ne!(&front, &map.terrain().image);
        hud.minimap = Some(map);
        for (width, height) in [(1024, 768), (800, 600), (1280, 1024), (1024, 768)] {
            let screen = Screen { width, height };
            hud.resize(screen);
            let node = hud.windows.find(WindowKind::Minimap).expect("node");
            assert_eq!(
                hud.windows.rect(node),
                [width - 176, height - 176, width, height]
            );
            assert!(hud.minimap_shown());
            let m = hud.minimap.as_ref().unwrap();
            assert_eq!(m.fog().front(), &front, "{width} x {height}: fog kept");
            assert_eq!(m.terrain().dim_x, 128);
            let (cmds, layout) = draw_with_minimap(&hud, &vfs, screen);
            assert_eq!(cmds.len(), 1, "one terrain quad");
            let DrawCmd::Quad4 { corners, .. } = cmds[0] else {
                unreachable!()
            };
            assert_eq!(corners, layout.corners());
            let mid = [
                corners.iter().map(|c| c[0]).sum::<f32>() / 4.0,
                corners.iter().map(|c| c[1]).sum::<f32>() / 4.0,
            ];
            crate::game::minimap::draw::tests::close(
                mid,
                [(width - 89) as f32, (height - 89) as f32],
                "center of the diamond",
            );
            assert_eq!(m.last_layout(), Some(layout));
        }
    }

    /// The corner buttons come as children of the minimap after the camera frame, per button base tile 15 and icon
    /// from `map_icons.tga` (30 x 30); the icon at (left + 2, top - 3) in the color of the constructor (normal
    /// `B4B4B4`). `{0x1D, 0}` removes them together with the map.
    #[test]
    fn corner_buttons_draw_after_the_camera_frame() {
        use crate::game::minimap::draw::tests::{minimap, ui_install};
        use nml_ui::hud::toolbar::minimap_buttons::MAP_ICONS_PATH;
        let (_dir, vfs) = ui_install();
        let mut hud = synthetic_hud();
        hud.minimap = Some(minimap(128));
        hud.rebuild_minimap_buttons();
        let draw = |hud: &Hud| {
            use crate::game::minimap::draw::{self, tests::start_camera};
            let host = host();
            let texts = empty_texts();
            let game = SimGameText::new(&host, &texts);
            let values = Hud::status_values(&host, &texts);
            let mut layer = crate::ui_layer::UiLayer::new(&vfs, 1280, None).unwrap();
            let mut ctx = layer.context(&vfs, SCREEN);
            let layout = draw::layout(
                draw::screen_size(SCREEN),
                &start_camera(glam::Vec3::new(8192.0, 0.0, 8192.0)),
                (128, 128),
            );
            let frame = MinimapFrame {
                layout,
                dots: Vec::new(),
                pings: Vec::new(),
                camera: [[0.0; 2]; 4],
                texture: hud.minimap.as_ref().unwrap().texture(ctx.assets),
            };
            let mut out = Vec::new();
            hud.draw_windows(&mut ctx, &game, &values, None, Some(&frame), &mut out);
            let icons = ctx.assets.find(MAP_ICONS_PATH);
            (out, icons)
        };
        let (out, icons) = draw(&hud);
        let icons = icons.expect("map_icons.tga loaded");
        let first_icon = out
            .iter()
            .position(|c| matches!(c, DrawCmd::Quad { texture: Some(t), .. } if *t == icons))
            .expect("corner buttons drawn");
        let last_line = out
            .iter()
            .rposition(|c| matches!(c, DrawCmd::Quad4 { texture: None, .. }))
            .expect("camera frame");
        assert!(last_line < first_icon, "buttons after the camera frame");
        let quads: Vec<_> = out[first_icon..]
            .iter()
            .filter_map(|c| match c {
                DrawCmd::Quad {
                    texture: Some(t),
                    dst,
                    color,
                    ..
                } if *t == icons => Some((*dst, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(
            quads.len(),
            10,
            "five buttons (campaign) per base tile and icon"
        );
        // Mode button (W - 177, H - 173): white base tile at (847, 595), icon at (849, 592).
        assert_eq!(quads[0], ([847, 595, 877, 625], 0xFFFF_FFFF));
        assert_eq!(quads[1], ([849, 592, 879, 622], 0xFFB4_B4B4));
        let mut toolbar = ToolbarHost::new();
        hud.present(
            vec![HudEvent::with_args(HudEventKind::MinimapOnOff, &[0])],
            &mut toolbar,
        );
        let (out, _) = draw(&hud);
        assert!(
            !out.iter()
                .any(|c| matches!(c, DrawCmd::Quad { texture: Some(t), .. } if *t == icons))
        );
    }

    /// Without a node (`IsMap 0`) and hidden (`{0x1D, 0}`, cinemascope) the minimap draws nothing, even if the host
    /// supplies data; `{0x1D, 1}` shows it again. Cinemascope itself additionally blocks it in the host
    /// (`GameSession::minimap_frame`, test `the_session_draws_the_minimap_except_in_cinemascope`).
    #[test]
    fn minimap_not_drawn_in_cinemascope_or_without_ismap() {
        use crate::game::minimap::draw::tests::{minimap, ui_install};
        let (_dir, vfs) = ui_install();
        let mut hud = synthetic_hud();
        hud.minimap = Some(minimap(128));
        assert_eq!(draw_with_minimap(&hud, &vfs, SCREEN).0.len(), 1);
        let mut toolbar = ToolbarHost::new();
        hud.present(
            vec![HudEvent::with_args(HudEventKind::MinimapOnOff, &[0])],
            &mut toolbar,
        );
        assert!(!hud.minimap_shown());
        assert!(draw_with_minimap(&hud, &vfs, SCREEN).0.is_empty());
        // The node stays visible (it takes the mouse even when hidden, O-291).
        let node = hud.windows.find(WindowKind::Minimap).unwrap();
        assert!(hud.windows.visible(node));
        hud.present(
            vec![HudEvent::with_args(HudEventKind::MinimapOnOff, &[1])],
            &mut toolbar,
        );
        assert!(hud.minimap_shown());
        assert_eq!(draw_with_minimap(&hud, &vfs, SCREEN).0.len(), 1);
        // Loading and restart: visible like the constructor.
        hud.present(
            vec![HudEvent::with_args(HudEventKind::MinimapOnOff, &[0])],
            &mut toolbar,
        );
        hud.clear();
        assert!(hud.minimap_shown());

        // `IsMap 0`: no node, no image.
        let off = EngineConfig {
            is_map: 0,
            ..EngineConfig::default()
        };
        let mut hud = Hud::new(
            &parse_messages(MESSAGES),
            &BTreeMap::new(),
            &HudOptions::from_config(&off),
            SCREEN,
        );
        hud.minimap = Some(minimap(128));
        assert!(hud.windows.find(WindowKind::Minimap).is_none());
        assert!(!hud.minimap_shown());
        assert!(draw_with_minimap(&hud, &vfs, SCREEN).0.is_empty());
    }

    #[test]
    fn ui_state_reports_heads_with_remaining_times() {
        // `ui_state.game.hud`: per kind with messages the head with remaining times in game time and the MSGIDs behind
        // it, action stack, status bar texts (red at population > living space) and the windows.
        let (_dir, vfs) = ui_install();
        let mut hud = loaded_hud(&vfs, &HudOptions::default());
        let mut host = host();
        host.active_player = 0;
        host.state.players[0].name = "Anna".into();
        host.state.players[0].resources_fx = [100 << 12, 0, 0, 11 << 12, 10 << 12];
        host.state.economy.max_living_space = 200;
        let texts = empty_texts();
        let game = SimGameText::new(&host, &texts);
        let values = Hud::status_values(&host, &texts);
        hud.handle(
            &[
                message(Some(0), 0, NO_TILE),
                message(Some(0), 4, (3, 4)),
                attack(0, 1107, false),
            ],
            0,
            &mut Vec::new(),
        );
        // 1000 ms game time (speed 10): "WIR HABEN NICHT GENUG NAHRUNG!" 30 * 55 = 1650 -> 650; "Einheit" 7 * 55 = 385
        // -> -615 (the duration runs below 0, the fade-out begins in the next tick).
        hud.tick(1000.0, 10, &mut Vec::new());
        let state = hud.state(&game, &values);
        assert_eq!(state.dialog, "GAMEUI");
        let heads: Vec<_> = state
            .messages
            .iter()
            .map(|m| {
                (
                    m.tag.as_str(),
                    m.id,
                    m.display_ms,
                    m.fade_ms,
                    m.queued.clone(),
                )
            })
            .collect();
        assert_eq!(
            heads,
            [
                ("ECONOMY", 0, 650, 1500, vec![4]),
                ("MILITARY", 100, -615, 1500, vec![])
            ]
        );
        assert_eq!(state.messages[0].text, "WIR HABEN NICHT GENUG NAHRUNG!");
        assert_eq!(state.messages[0].flags, hud.messages.queue(0)[0].flags);
        assert!(state.action_stack.is_empty(), "4 pings only on appearing");
        let status = state.status.as_ref().unwrap();
        assert_eq!(status.texts, ["100", "11/10", "Anna"]);
        assert!(status.shown && status.red);
        assert_eq!(state.windows.len(), 7);
        assert_eq!(
            (state.windows[2].kind.as_str(), state.windows[2].rect),
            ("status_bar", [0, 0, 1024, 26])
        );
        assert_eq!(
            state.windows.iter().map(|w| w.layer).collect::<Vec<_>>(),
            [4, 3, 2, 2, 2, 2, 1]
        );
        // Further: 0 expires (display, then remainder immediately 0 because 4 waits), 4 appears and pings.
        for _ in 0..3 {
            hud.tick(1000.0, 10, &mut Vec::new());
        }
        let state = hud.state(&game, &values);
        assert_eq!(
            (state.messages[0].id, state.messages[0].display_ms),
            (4, 9 * 55)
        );
        assert!(state.messages[0].queued.is_empty());
        assert_eq!(state.action_stack, [[3, 4]]);
        // Second ping: it queues behind the running message 4 and pings on appearing. The report lists the most
        // recent location first (O-219: `push` in front).
        hud.handle(&[message(Some(0), 4, (7, 8))], 0, &mut Vec::new());
        for _ in 0..10 {
            if hud.state(&game, &values).action_stack.len() == 2 {
                break;
            }
            hud.tick(1000.0, 10, &mut Vec::new());
        }
        assert_eq!(
            hud.state(&game, &values).action_stack,
            [[7, 8], [3, 4]],
            "most recent first"
        );
        // Without a status bar `status` is missing.
        let off = HudOptions {
            switches: HudSwitches {
                status_bar: false,
                ..HudSwitches::default()
            },
            ..HudOptions::default()
        };
        hud.load_status_bar(&vfs, &off);
        assert_eq!(hud.state(&game, &values).status, None);
    }
}
