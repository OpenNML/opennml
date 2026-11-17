//! Commands of the menu UI `ui_state`, `ui_click`, `ui_key`, `ui_open` (protocol 24), `ui_video` (protocol 25) and
//! `ui_present`: validation of the parameters and the response of `ui_state` as a mirror of
//! `nml_ui::app::UiReport`. Without a dependency on `nml-ui`; the commands are executed in `nml_client::ui_control`
//! (viewer with `--dialog` in a window and `nml-game`). Description: `docs/viewer-control.md`, section "UI (`ui_*`)".

use serde::{Deserialize, Serialize};

use crate::protocol::{Command, MouseButtonName};

/// Validates the UI commands without a UI: names not empty. Only execution checks the key names (`parse_chord`
/// capitalizes by itself, "left" matches `LEFT`, re/menu-system 12.7), likewise dialog and element names.
/// `ui_click` with the middle button (quirk O-331): the UI knows only left and right.
pub const UI_CLICK_MIDDLE: &str = "ui_click with the middle button does not exist (allowed: left or right; the middle button via mouse_button)";

pub(crate) fn validate(command: &Command) -> Result<(), String> {
    match command {
        Command::UiClick {
            dialog,
            element,
            button,
        } => {
            not_empty("dialog", dialog, NAME_HINT)?;
            not_empty("element", element, NAME_HINT)?;
            if *button == Some(MouseButtonName::Middle) {
                return Err(UI_CLICK_MIDDLE.into());
            }
            Ok(())
        }
        Command::UiKey { chord, .. } if chord.trim().is_empty() => Err(
            "empty key combination (expected: key names of the original such as ESCAPE, RETURN or \"CONTROL H\")"
                .into(),
        ),
        Command::UiOpen { dialog } => not_empty("dialog", dialog, NAME_HINT),
        Command::UiVideo { name } => not_empty("name", name, VIDEO_HINT),
        Command::UiPresent { event } => validate_present(event),
        _ => Ok(()),
    }
}

/// `ui_present`: `event` is an object with exactly one key (variant with fields) or a string (unit variant), as in
/// the JSON form of `Presentation`. Only execution checks which variant and which fields (serde error as
/// "ui_present: ...").
fn validate_present(event: &serde_json::Value) -> Result<(), String> {
    match event {
        serde_json::Value::String(name) if !name.trim().is_empty() => Ok(()),
        serde_json::Value::Object(map) if map.len() == 1 => Ok(()),
        _ => Err(PRESENT_HINT.into()),
    }
}

/// Message for an unsuitable form of `event`.
const PRESENT_HINT: &str = "event must be an object with exactly one key or a string, e.g. {\"Portrait\": {\"slot\": 1, \"portrait\": 1000, \"frame\": 101}} or \"CameraBackup\"";

/// Hint of the "empty" message for dialog and element names.
const NAME_HINT: &str = "a name as in ui_state";
/// Hint of the "empty" message for video names (they are not in `ui_state`).
const VIDEO_HINT: &str = "a video name like eng, without .bik";

fn not_empty(field: &str, value: &str, hint: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} is empty (expected: {hint})"));
    }
    Ok(())
}

/// Result of `ui_state` (and of `state`/`step` in `nml-game`): state of the UI at time `now_ms` on the screen
/// `screen`. Names lowercase as in the messages, rectangles `[x0, y0, x1, y1]` in screen pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiStateData {
    /// Clock of the UI (ms).
    pub now_ms: u64,
    /// Screen `[width, height]` in pixels.
    pub screen: [i32; 2],
    /// Open frames in drawing order (non-modal, then modal).
    pub dialogs: Vec<UiDialogData>,
    /// Innermost widget with keyboard focus `[dialog, element]`.
    pub focus: Option<(String, String)>,
    /// Innermost widget with "mouse over" `[dialog, element]`.
    pub hover: Option<(String, String)>,
    /// Named elements of the open dialogs.
    pub elements: Vec<UiElementData>,
    /// Images of the desktop `[path, opacity]` in drawing order.
    pub images: Vec<(String, u8)>,
    /// The dialog stack has pending orders (triggering is refused, O-21).
    pub busy: bool,
    /// Waiting application commands.
    pub queued: usize,
    /// The last 32 effects with the time of their frame.
    pub effects: Vec<UiEffectData>,
    /// Running video (protocol 25); absent when none is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<UiVideoData>,
    /// The mouse pointer is off (start sequence, campaign video; protocol 25).
    #[serde(default)]
    pub pointer_hidden: bool,
    /// Running match in `nml-game`; absent without a match and in the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<UiGameData>,
    /// Menu level behind the main menu in `nml-game`; absent as long as none is running (and in the viewer). `game`
    /// remains reserved for the match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_level: Option<UiMenuLevelData>,
    /// Rows of the accounting as long as the frame `accounting` is open (protocol 29; from
    /// `nml_ui::screens::MenuData::accounting`, the match has already ended after `0x0E`); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounting: Option<Vec<UiAccountingRow>>,
    /// Game time text of the open accounting ("Game time: 0:05" as hours:minutes, O-164; protocol 29); absent like
    /// [`Self::accounting`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_time: Option<String>,
    /// State of the 3D mouse pointer: ID, running animation, tint, phase of the loop and whether the software pointer
    /// (`HardwareCursor 0`) draws it; also in the main menu without a match. Absent in the viewer and without a loaded
    /// `mouse.rdo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_model: Option<UiPointerModelData>,
}

/// 3D mouse pointer in the report of `nml-game` (`nml_client::pointer_model::PointerModelData` plus setting):
/// read-only, does not commit the restart of the loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiPointerModelData {
    /// Pointer ID 0-11 (`SetPointer`).
    pub id: i32,
    /// Running animation of the model.
    pub animation: i32,
    /// Tint `[r, g, b, a]` (build cursor green/red, O-464).
    pub tint: [f32; 4],
    /// Phase of the loop in ms; absent as long as a restart is pending (next drawn frame).
    pub phase_ms: Option<f32>,
    /// `HardwareCursor 0`: the model is drawn; otherwise the system draws the pointer (O-462).
    pub software: bool,
}

/// A row of the accounting (protocol 29, mirror of a row of `nml_ui::screens::AccountingView`): an active player in
/// slot order with name, color index and the twelve values as text as in the table (`FUN_00403FA0`, re/accounting 4.1;
/// columns 0-3 technology, 4-7 military, 8-11 economy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiAccountingRow {
    pub name: String,
    /// Color index of the record (`+0x408`, without range check; from 8 gray).
    pub color: i32,
    pub values: [String; 12],
}

/// The menu level in the report of `nml-game` (`nml_client::game_flow::GameFlow::menu_level_data`): the session of
/// `menü2.lvl` in game type -2, which runs behind the main menu or stands stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiMenuLevelData {
    /// `.lvl` path of the menu level (normalized).
    pub level: String,
    /// Visible and running (bit `0x1` of `GameApp+0xD5E0`); false in submenus with their own `BACKGROUND` and under a
    /// video.
    pub shown: bool,
    /// Engine ticks of the simulation since the start.
    pub steps: u64,
    /// Checksum of the simulation as hex text with 16 digits (like [`UiGameData::checksum`]).
    pub checksum: String,
}

/// The running match in the report of `nml-game` (`nml_client::game_flow::GameFlow::game_data`). Without `Eq`: the
/// state of the world rendering ([`Self::world`]) and the rotation angle of the minimap are floating-point numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiGameData {
    /// `.lvl` path of the map.
    pub level: String,
    /// Game type of the original (`GameInfo::game_type`).
    pub game_type: i32,
    /// Game menu state (`Engine+0x2CCC`): 0 game running, != 0 a game menu halts.
    pub menu_state: u8,
    /// Engine ticks of the simulation since the start.
    pub steps: u64,
    /// Checksum of the simulation as hex text with 16 digits (like `SimHashResult::checksum`).
    pub checksum: String,
    /// Loading phase in percent (0, 40, 70, 100); set only during loading. Loading blocks the host, so it is never
    /// visible over the control channel - [`Self::load_phases`] exists for that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loading: Option<u8>,
    /// History of the last load (percent per drawn phase).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_phases: Vec<u8>,
    /// Result of the local player (`Player+0x358`, protocol 29): 0 playing, 1 lost, 2 won.
    #[serde(default)]
    pub result: i32,
    /// Game UI of the match: window tree, message bar, action stack, status bar. Absent in older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hud: Option<UiHudData>,
    /// Input of the match: pause, game speed, control groups, pointer, keyboard context and locks. Absent in older
    /// lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<UiInputData>,
    /// Presentation of the campaign UI: bars, portrait and text slots, fade, pointer, clear color. Absent in older
    /// lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<UiPresentationData>,
    /// State of the world rendering of the map (protocol 38): flag, marks, wheel tracks, water rings, silhouettes,
    /// beams, charges. `nml-game` fills it after `GameFlow::game_data`; absent without a map and in older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<crate::state::WorldDisplayState>,
    /// Flags of the game types Domination and King of the Hill (protocol 39): the same structure as
    /// `state.sim.flags`; set in game type 2/3 or with a living player flag, otherwise the field is absent. Absent in
    /// older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<crate::sim::SimFlagsInfo>,
    /// Minimap at the bottom right: visibility, mode, position, pings, number of idle workers, fog counter. Absent
    /// without a minimap node (`IsMap`) and in older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimap: Option<UiMinimapData>,
    /// The diplomacy dialog `DIPLOMATIE` is open (game menu state 10). Absent in older lines (`false`).
    #[serde(default)]
    pub diplomacy_open: bool,
    /// The active chat lines (screen texts of kind 10) as "name: text" in list order, not the waiting ones; empty
    /// without chat and in older lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chat_lines: Vec<String>,
    /// Recording or replay of the match (protocol 47): kind, file, tick and end. Absent without recording and replay
    /// and in older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<UiReplayData>,
}

/// Recording or replay in the report of `nml-game` (`ui_state.game.replay`, from
/// `nml_client::game_flow::GameFlow::game_data`). Read-only: recording and replay do not touch the state of the
/// simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiReplayData {
    /// Recording (`record`, `SAVEREPLAY`) or replay (`play`, `LOADAUTOPLAY`).
    pub mode: UiReplayMode,
    /// File name in `savegame/autoplay` (without folder), e.g. `261108-1435`.
    pub file: String,
    /// Recording: tick of the simulation at the last update of the file (it extends up to there). Replay: tick of
    /// the next command of the file; after the end of the file the current tick of the simulation.
    pub step: u64,
    /// Recording: it has ended (eight hours, write error, network sync). Replay: all commands of the file have
    /// been executed (end of file).
    pub ended: bool,
}

/// Kind of [`UiReplayData`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiReplayMode {
    /// The match is being recorded.
    Record,
    /// The match is a replay.
    Play,
}

/// Minimap in the report of `nml-game` (`nml_client::game_session::GameSession::minimap_state`;
/// `ui_state.game.minimap`). Read-only: the state lives in the host, not in the simulation, and does not go into the
/// savegame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiMinimapData {
    /// The map is visible: node in the tree, switch `+0x4D` on and no cinemascope. When hidden, the node still takes
    /// the mouse (O-291).
    pub visible: bool,
    /// Map mode (`GameWorld+0x22D4`): 0 all, 1 economy, 2 military.
    pub mode: u8,
    /// Center of the diamond `[W - 89, H - 89]` in UI pixels (position of the last frame; before the first frame from
    /// the image size).
    pub center: [i32; 2],
    /// Rotation angle theta in radians (`atan2(eye.z - target.z, eye.x - target.x) + pi/2`, position of the last
    /// frame; 0 before the first frame).
    pub theta: f32,
    /// The pings of the map in list order as `(tile x, tile z, remaining repetitions)`.
    pub pings: Vec<(i32, i32, i32)>,
    /// Length of the list of idle workers of the active slot (buttons 576/577).
    pub idle_workers: usize,
    /// Number of buffer swaps of the fog image since the start of the match: each finished pass over all tile rows
    /// counts one (instant rebuilds do not).
    pub fog_swaps: u32,
}

/// Input of the match in the report of `nml-game` (`nml_client::game_session::GameSession::input_data`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiInputData {
    /// Pause bit of the match (`Engine+0x24E4`): the world stands still (O-317).
    pub paused: bool,
    /// Visible pause (`Engine+0x24E5`): the PAUSE lettering.
    pub shown_paused: bool,
    /// Game speed of the simulation (`SimHost::game_speed`, 0 ... 20).
    pub game_speed: u8,
    /// Setting `GameSpeed` of the UI (`EngineConfig+0x34`; the speed keys write it).
    pub config_speed: i32,
    /// The 10 control groups of the active slot, per group the object IDs in selection order.
    pub groups: Vec<Vec<u32>>,
    /// Last chosen group 0 ... 9, -1 none (`Player+0x354`).
    pub last_group: i8,
    /// ID of the mouse pointer that the host draws (`Cursor3D+0x38`; 7 normal, always 7 in menus and dialogs).
    pub pointer: i32,
    /// Variant of the pointer (`Cursor3D+0x2C`): 0 normal, 1 red, 2 green.
    pub pointer_variant: u8,
    /// The pointer is on (not `pointer_hidden` of the UI).
    pub pointer_visible: bool,
    /// Keyboard context for `keys.ini`: `[type, nation + 1, selection GUID]` (re/game-ui D5.3, O-303).
    pub key_context: [i32; 3],
    /// Effective input locks (`InputGate`) in fixed order: `ui_locked`, `cinemascope`, `dialog_running`,
    /// `script_camera`, `campaign`, `menu_open`, `menu_level` (game type -2; practically never in the report of the
    /// match because `game` is absent for the menu level); empty without a lock.
    pub gate: Vec<String>,
}

/// Presentation of the campaign UI in the report of `nml-game` (`nml_client::game_flow::GameFlow::game_data`;
/// `ui_state.game.presentation`). Read-only: the values come from the UI, not from the simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPresentationData {
    /// Height of the cinemascope bars `[top, bottom]` in screen pixels (truncating like the play area, top and bottom
    /// can differ by one pixel); `[0, 0]` without bars.
    pub bars: [i32; 2],
    /// Occupied portrait slots in slot order. The portraits report even without bars (state bit 1 of the slots), but
    /// are drawn only with bars != 0 (O-354).
    pub portraits: Vec<UiPortraitSlot>,
    /// Occupied text slots in slot order (speech and hint lines).
    pub texts: Vec<UiTextSlot>,
    /// Opacity of the fade 0...255 (0 no fade, 255 black).
    pub fade_alpha: u8,
    /// The pointer is drawn (opposite of `ui_state.pointer_hidden`).
    pub pointer_visible: bool,
    /// Clear color of the frame (RGB 0...255; the fade tints it, `(c * 255)` rounded).
    pub clear_color: [u8; 3],
    /// Open info windows (168/213) from head to end as `(text ID, [left, top, right, bottom])` in the 1280 x 1024
    /// grid; only measured windows (measured at the first drawing). Absent in older lines.
    #[serde(default)]
    pub info_frames: Vec<(i32, [i32; 4])>,
    /// Text of the script countdown (`m:ss`), only with a registered container; absent in older lines.
    #[serde(default)]
    pub countdown: Option<String>,
    /// Remaining times of the timer list (`m_CountDownList`, kind bit 1) as `m:ss` in ID order, drawn at the top left
    /// (leader timers of game types 2/3, time limit of game type 4); empty without timer or simulation; absent in
    /// older lines.
    #[serde(default)]
    pub timers: Vec<String>,
    /// Text ID of the screen text `n5` (kind 2) as long as it stands; absent in older lines.
    #[serde(default)]
    pub screen_text: Option<i32>,
}

/// An occupied portrait slot of the campaign UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPortraitSlot {
    /// Slot (0...3).
    pub slot: u8,
    /// Portrait number of the table `portraits.ini`.
    pub portrait: i32,
    /// Frame number of the portrait.
    pub frame: i32,
    /// Opacity 0...255.
    pub alpha: u8,
}

/// An occupied text slot of the campaign UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextSlot {
    /// Text slot 0...3 (speech lines); a hint occupies `slot + 2` (see docs/viewer-control.md).
    pub slot: u8,
    /// Text number.
    pub text: i32,
    /// Opacity 0...255.
    pub alpha: u8,
}

/// Dialog name of the game UI in the report (target of `ui_click`).
pub const GAME_UI_DIALOG: &str = "GAMEUI";

/// Game UI in the report of `nml-game` (`nml_client::game::hud::Hud::state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiHudData {
    /// Always [`GAME_UI_DIALOG`].
    pub dialog: String,
    /// Windows of the tree in creation order (base, root, status bar, minimap, command bar, cinemascope, tooltip;
    /// only those created according to the `Engine.ini` switches).
    pub windows: Vec<UiHudWindow>,
    /// Message bar: per kind with messages in file order the head with the queue behind it.
    pub messages: Vec<UiHudMessage>,
    /// Action stack: ping locations `[x, z]` as tiles, newest first (at most 5, O-219).
    pub action_stack: Vec<[i32; 2]>,
    /// Status bar; absent without `IsStatusBar` or without a readable `statusbar.ini`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<UiStatusBarData>,
    /// Command bar; absent without `IsToolBar` and in older lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolbar: Option<UiToolbarData>,
}

/// Command bar in the report of `nml-game` (`nml_client::game::hud::Hud::toolbar_report`): the shown menu, the
/// buttons with the element names of `ui_click` on [`GAME_UI_DIALOG`], portrait, tooltip, counter-upgrades, cursor and
/// the last button. Rectangles `[x0, y0, x1, y1]` in screen pixels, state of the last frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiToolbarData {
    /// `Toolbar+0x5CC`: GUID of the shown menu (0 after a multi-selection is extended).
    pub shown_menu: i32,
    /// `Toolbar+0x524`: HID of the single selection, -1 for a multi-selection, 0 without selection.
    pub selected: i32,
    /// `Toolbar+0x48`: HID of the object under the mouse, 0 none.
    pub hover: u32,
    /// All buttons in child order (normal slots, production, multi-selection, stance, formation, heroes,
    /// headhunters).
    pub buttons: Vec<UiToolbarButton>,
    /// Portrait with values as drawn; absent when the bar shows nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portrait: Option<UiPortraitData>,
    /// Tooltip of the frame; absent without a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<UiTooltipData>,
    /// Running counter-upgrades in list order: GUID and remaining time as `m:ss`.
    pub counter_upgrades: Vec<(i32, String)>,
    /// Open cursor: `build`, `tent`, `wall`, `target` or `defense`; absent without a cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// `GameEngine+0x4E0`: `BUTTONMSG` of the last pressed button, for selection, hero and occupant buttons the HID of
    /// the object (O-246, O-269); absent before the first click.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_button: Option<i32>,
}

/// A button of the command bar in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiToolbarButton {
    /// Element name for `ui_click` on [`GAME_UI_DIALOG`]: `normal:<slot>`, `queue:<i>` (0 = running order),
    /// `selection:<i>`, `stance:<i>`, `formation:<i>`, `hero:<slot>` or `headhunter:<slot>`. Normal buttons are also
    /// reachable as `guid:<msg>`.
    pub element: String,
    /// `BUTTONMSG` (for selection and hero buttons the HID, for production the GUID of the order).
    pub msg: i32,
    /// Full kind word from `toolbar.ini` (top byte the kind, below it flag or the index for selection and
    /// queue).
    pub kind: u32,
    /// State `+0x90`: 0 normal, 1 mouse over, 2 pressed, 3 locked.
    pub state: u8,
    /// Rectangle in screen pixels.
    pub rect: [i32; 4],
    /// Progress `+0x9C` in percent (100 = without veil).
    pub progress: i32,
}

/// Portrait of the bar in the report: type GUID (for group 500), subtitle and the drawn values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPortraitData {
    pub guid: i32,
    /// Subtitle (type name or "Group"), as drawn.
    pub subtitle: String,
    /// Value lines in drawing order: attribute and text; color tags (`[COLOR ...]`) remain as text.
    pub values: Vec<(u32, String)>,
}

/// Tooltip in the report (window `tooltip`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTooltipData {
    /// GUID of the request (`BUTTONMSG`, HID or tooltip GUID of a region).
    pub guid: i32,
    /// Kind of the request (`0x17000000` for regions of the bar, otherwise the button kind).
    pub kind: u32,
    /// Text with inserted game values and keys, sections separated by line breaks.
    pub text: String,
    /// Box of the last drawn tooltip; `[0, 0, 0, 0]` as long as it has not been drawn yet.
    pub rect: [i32; 4],
}

/// A window of the game UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiHudWindow {
    /// Kind: `base`, `root`, `status_bar`, `minimap`, `toolbar`, `cinemascope`, `tooltip`.
    pub kind: String,
    /// Rectangle `[x0, y0, x1, y1]` in screen pixels.
    pub rect: [i32; 4],
    /// Layer (4 base ... 1 tooltip, O-248).
    pub layer: u8,
    /// Visible (`+0x1C`).
    pub visible: bool,
}

/// Head of a message kind of the message bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiHudMessage {
    /// Name of the kind (`MSGTAG`).
    pub tag: String,
    /// `MSGID` of the head.
    pub id: i32,
    /// Text of the head with inserted game values (tags of the text set remain).
    pub text: String,
    /// Flag word (priority, `0x10000`, player bytes, sound and ping bits).
    pub flags: u32,
    /// Remaining display time in ms of game time.
    pub display_ms: i32,
    /// Remaining fade-out time in ms of game time (1500 on appearance).
    pub fade_ms: i32,
    /// `MSGID`s of the queued messages behind the head, in order.
    pub queued: Vec<i32>,
}

/// Status bar in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiStatusBarData {
    /// Bar drawn (event `0x1C`).
    pub shown: bool,
    /// Texts of the entries in file order (resources, "Population/Living space", name).
    pub texts: Vec<String>,
    /// The population is shown in red (population > living space).
    pub red: bool,
}

/// Running video in the report (mirror of `nml_ui::app::ReportVideo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiVideoData {
    /// Path of the `.bik` as in the original (`data/videos/<name>.bik`).
    pub path: String,
    /// Frame at the time of the report (from 0).
    pub frame: u32,
    pub frames: u32,
    /// Any key aborts (O-68).
    pub abortable: bool,
}

/// An open dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiDialogData {
    pub name: String,
    pub rect: [i32; 4],
    /// Fly-in or fly-out is running.
    pub animating: bool,
    pub modal: bool,
}

/// A named element of an open dialog; fields without meaning for the element kind are absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiElementData {
    pub dialog: String,
    pub name: String,
    /// `button`, `combobox`, `listbox`, `editfield`, `slider`, `radiobutton` or `label`.
    pub kind: String,
    pub rect: [i32; 4],
    /// Drawn in the last frame.
    pub visible: bool,
    pub enabled: bool,
    /// Flag `HIDE` (from the file or from the logic).
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// Text of the logic or of the edit field (the report does not resolve texts from text IDs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// An effect of the UI with the time of its frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEffectData {
    pub at_ms: u64,
    #[serde(flatten)]
    pub effect: UiEffectKind,
}

/// Mirror of `nml_ui::msg::UiEffect` (field `kind`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiEffectKind {
    /// `SoundManager_PlayEvent2D`, e.g. `SFX_KLICK` with flags `0x14`, category 1.
    PlaySfx {
        event: String,
        flags: u32,
        category: u8,
    },
    PlaySpeech {
        path: String,
    },
    MenuMusic {
        path: String,
        looping: bool,
    },
    RestartMusic,
    /// Recreate the sound device with driver `index` (`sounddriver`).
    SetSoundDriver {
        index: i32,
    },
    /// Start video (`0x1D`, protocol 25): path of the `.bik`, `abortable` = any key aborts.
    PlayVideo {
        path: String,
        abortable: bool,
    },
    /// Video ended (end, abort or replacement by a new one, protocol 25).
    StopVideo {
        path: String,
    },
    /// Category 0 music, 1 sound effects, 2 speech.
    SetVolume {
        category: u8,
        value: i32,
    },
    SetGamma {
        gamma: f32,
    },
    SetHardwareCursor {
        on: bool,
    },
    SetDisplayMode {
        width: i32,
        height: i32,
        bits: i32,
    },
    WriteSettings,
    /// Write `profiles.ini` (field `kind` = `write_profiles`).
    WriteProfiles,
    /// Delete the savegames `savegame/*.<GUID %03X>` of a deleted profile.
    DeleteProfileSaves {
        guid: i32,
    },
    Quit,
    /// Start match (command `0x0D`): `.lvl` path, game type of the original and the occupied slots in list
    /// order.
    StartMatch {
        level: String,
        game_type: i32,
        slots: Vec<UiSlotData>,
    },
    /// The game continues (game menu closed): `game_speed` from `EngineConfig.GameSpeed` at `RETURN` (command
    /// `0x0F`), no value on the way back from `INGAMELOADGAME` (`0x37`).
    ResumeGame {
        game_speed: Option<u8>,
    },
    /// End the running match and restart the same level (command `0x1F`).
    ReloadLevel,
    /// Load the savegame of the active profile (command `0x0A`): name without the extension `.<GUID>`.
    LoadGame {
        name: String,
    },
    /// Write the savegame of the active profile (command `0x0B`).
    SaveGame {
        name: String,
    },
    /// Delete the savegame of the active profile (command `0x2A`).
    DeleteSave {
        name: String,
    },
    /// Play a recording (command `0x19`): full file name in `savegame/autoplay`.
    PlayReplay {
        name: String,
    },
    /// End screen of the local player queued (`victory` victory, otherwise defeat).
    GameEnd {
        victory: bool,
    },
    /// End of the match (command `0x0E`): the host builds the accounting, ends the match and opens `accounting`
    /// (field `kind` = `accounting`).
    #[serde(rename = "accounting")]
    EndMatch,
    /// Slot `slot` surrenders (command `0x27`): defeat, executed immediately.
    Surrender {
        slot: u8,
    },
    /// Start campaign level `level` with difficulty `difficulty` (0 ... 2) (command `0x0C`).
    StartCampaignLevel {
        level: String,
        difficulty: i32,
    },
    /// Current campaign entry booked (`FUN_004C6B10`: command `0x1C` or end of the match in the campaign); `won`
    /// won, otherwise one more attempt. `write_profiles` follows.
    BookCampaign {
        won: bool,
    },
    /// Change from the diplomacy dialog (fixed form): `action` is one of `status` (`slot`, `value` = status 0 friend,
    /// 1 enemy, 2 neutral), `view`, `chat`, `tribute` (each `slot` and `value` 1 on, 0 off), `chat_by` (`value` =
    /// relation -1 all, 0 friend, 1 enemy, 2 neutral), `send_tribute` (`value` = resource 0 gold, 1 wood, 2 food) or
    /// `close`. `slot` and `value` are absent where they do not apply.
    Diplomacy {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<i32>,
    },
    /// Send from the chat window `window` 1-3: the host sends `0x5D` with `text`.
    SendChat {
        window: u8,
        text: String,
    },
    /// `0x21` (`load = Some(path)`) or `0x20` (`None`): start/resume or stop the menu level.
    MenuLevel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        load: Option<String>,
    },
    /// Application command that the reimplementation does not (yet) execute: code of the original and description.
    Unhandled {
        code: u32,
        command: String,
    },
    /// Application command of the network session (`7`, `8`, `9`, `0x2D`, `0x2F`): code of the original and
    /// description.
    Net {
        code: u32,
        command: String,
    },
    /// LAN search on or off (`LAN1`).
    Search {
        on: bool,
    },
    /// Request from the lobby (`LAN2`/`CHATFRAME`), as description.
    Lobby {
        action: String,
    },
}

/// An occupied player slot of [`UiEffectKind::StartMatch`]: type (1...4 computer, 999 human), nation, team, color,
/// handicap 0...5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSlotData {
    pub kind: i32,
    pub nation: i32,
    pub team: i32,
    pub color: i32,
    pub handicap: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MouseButtonName, Request};
    use crate::sim::hex_checksum;
    use serde_json::json;

    fn parse(line: serde_json::Value) -> Result<Command, serde_json::Error> {
        serde_json::from_value::<Request>(line).map(|r| r.command)
    }

    #[test]
    fn ui_video_round_trips_and_validates() {
        let line = json!({"id": 3, "cmd": "ui_video", "name": "eng"});
        let request: Request = serde_json::from_value(line.clone()).unwrap();
        assert_eq!(request.command, Command::UiVideo { name: "eng".into() });
        assert_eq!(serde_json::to_value(&request).unwrap(), line);
        assert_eq!(request.command.validate(), Ok(()));
        // Required field missing -> error when reading; empty name -> error message.
        assert!(parse(json!({"id": 1, "cmd": "ui_video"})).is_err());
        let empty = Command::UiVideo { name: " ".into() };
        let message = empty.validate().unwrap_err();
        assert!(message.contains("name is empty"), "{message}");
        // The hint names video names, not the names from `ui_state`.
        assert!(
            message.contains("a video name like eng, without .bik"),
            "{message}"
        );
        assert!(!message.contains("ui_state"), "{message}");
        // Dialog and element names keep their hint.
        let open = Command::UiOpen { dialog: "".into() };
        assert!(
            open.validate()
                .unwrap_err()
                .contains("a name as in ui_state")
        );
    }

    /// `ui_present` goes out and back as a line; `event` is an object with exactly one key or a string, everything
    /// else an error message.
    #[test]
    fn ui_present_round_trips_and_validates() {
        let line = json!({"id": 4, "cmd": "ui_present",
                          "event": {"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}});
        let request: Request = serde_json::from_value(line.clone()).unwrap();
        assert_eq!(
            request.command,
            Command::UiPresent {
                event: json!({"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}})
            }
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), line);
        assert_eq!(request.command.validate(), Ok(()));
        // Unit variant as a string.
        let unit = parse(json!({"id": 5, "cmd": "ui_present", "event": "CameraBackup"})).unwrap();
        assert_eq!(unit.validate(), Ok(()));
        // Required field missing -> error when reading.
        assert!(parse(json!({"id": 1, "cmd": "ui_present"})).is_err());
        // Wrong forms: number, empty string, empty object, object with two keys, list.
        for bad in [
            json!(3),
            json!(" "),
            json!({}),
            json!({"Fade": {"into": true, "ms": 1}, "CameraBackup": null}),
            json!([1]),
        ] {
            let message = Command::UiPresent { event: bad.clone() }
                .validate()
                .unwrap_err();
            assert!(
                message.contains("event must be an object"),
                "{bad}: {message}"
            );
        }
    }

    #[test]
    fn ui_commands_round_trip_and_validate() {
        // Round trip including defaults (`button` and `char` are missing in the line).
        let cases = [
            (json!({"id": 7, "cmd": "ui_state"}), Command::UiState),
            (
                json!({"id": 7, "cmd": "ui_click", "dialog": "start", "element": "options"}),
                Command::UiClick {
                    dialog: "start".into(),
                    element: "options".into(),
                    button: None,
                },
            ),
            (
                json!({"id": 7, "cmd": "ui_click", "dialog": "start", "element": "options", "button": "right"}),
                Command::UiClick {
                    dialog: "start".into(),
                    element: "options".into(),
                    button: Some(MouseButtonName::Right),
                },
            ),
            (
                json!({"id": 7, "cmd": "ui_key", "chord": "CONTROL H"}),
                Command::UiKey {
                    chord: "CONTROL H".into(),
                    char: None,
                },
            ),
            (
                json!({"id": 7, "cmd": "ui_key", "chord": "a", "char": 228}),
                Command::UiKey {
                    chord: "a".into(),
                    char: Some(228),
                },
            ),
            (
                json!({"id": 7, "cmd": "ui_open", "dialog": "credits"}),
                Command::UiOpen {
                    dialog: "credits".into(),
                },
            ),
            (
                json!({"id": 7, "cmd": "ui_video", "name": "eng"}),
                Command::UiVideo { name: "eng".into() },
            ),
        ];
        for (line, expected) in cases {
            let request: Request = serde_json::from_value(line.clone()).unwrap();
            assert_eq!(request.command, expected);
            assert_eq!(serde_json::to_value(&request).unwrap(), line);
            assert_eq!(expected.validate(), Ok(()), "{line}");
        }

        // `button` only `left`/`right`: an unknown button fails when reading, `middle` (a button of `mouse_button`
        // since quirk O-331) when validating; required fields missing -> error when reading.
        assert!(
            parse(
                json!({"id": 1, "cmd": "ui_click", "dialog": "a", "element": "b", "button": "x1"})
            )
            .is_err()
        );
        let middle: Request = serde_json::from_value(
            json!({"id": 1, "cmd": "ui_click", "dialog": "a", "element": "b", "button": "middle"}),
        )
        .unwrap();
        assert!(
            middle
                .command
                .validate()
                .unwrap_err()
                .contains("middle button")
        );
        assert!(parse(json!({"id": 1, "cmd": "ui_click", "dialog": "a"})).is_err());
        assert!(parse(json!({"id": 1, "cmd": "ui_key"})).is_err());
        assert!(parse(json!({"id": 1, "cmd": "ui_open"})).is_err());

        // Empty names are error messages; lowercase is allowed (re/menu-system 12.7).
        let click = |d: &str, e: &str| Command::UiClick {
            dialog: d.into(),
            element: e.into(),
            button: None,
        };
        assert!(
            click(" ", "ok")
                .validate()
                .unwrap_err()
                .contains("dialog is empty")
        );
        assert!(
            click("options", "")
                .validate()
                .unwrap_err()
                .contains("element is empty")
        );
        let key = |c: &str| Command::UiKey {
            chord: c.into(),
            char: None,
        };
        assert!(
            key("  ")
                .validate()
                .unwrap_err()
                .contains("empty key combination")
        );
        assert_eq!(key("left").validate(), Ok(()));
        assert_eq!(key("Escape").validate(), Ok(()));
        let open = Command::UiOpen { dialog: "".into() };
        assert!(open.validate().unwrap_err().contains("dialog is empty"));
        // Quirk O-331: the middle button exists only for `mouse_button`.
        let middle = Command::UiClick {
            dialog: "GAMEUI".into(),
            element: "minimap".into(),
            button: Some(MouseButtonName::Middle),
        };
        assert!(middle.validate().unwrap_err().contains("middle button"));
    }

    /// `menu_level` in the report, with and without the field; a line without the field (older than this version)
    /// can be read.
    #[test]
    fn ui_state_reports_the_menu_level() {
        let level = UiMenuLevelData {
            level: "data/levels/menülevel/menü2.lvl".into(),
            shown: true,
            steps: 7,
            checksum: hex_checksum(0xAB),
        };
        let json = |menu_level: Option<UiMenuLevelData>| {
            serde_json::to_value(UiStateData {
                now_ms: 0,
                screen: [1024, 768],
                dialogs: Vec::new(),
                focus: None,
                hover: None,
                elements: Vec::new(),
                images: Vec::new(),
                busy: false,
                queued: 0,
                effects: Vec::new(),
                video: None,
                pointer_hidden: false,
                game: None,
                menu_level,
                accounting: None,
                play_time: None,
                pointer_model: None,
            })
            .unwrap()
        };
        let with = json(Some(level.clone()));
        assert_eq!(
            with["menu_level"],
            json!({"level": "data/levels/menülevel/menü2.lvl", "shown": true, "steps": 7,
                   "checksum": "00000000000000ab"})
        );
        let back: UiStateData = serde_json::from_value(with).unwrap();
        assert_eq!(back.menu_level, Some(level));
        let without = json(None);
        assert!(without.get("menu_level").is_none(), "{without}");
        let back: UiStateData = serde_json::from_value(without).unwrap();
        assert_eq!(back.menu_level, None);
    }

    /// `pointer_model` in the report, with and without the field; a line without the field (older than this version)
    /// can be read.
    #[test]
    fn ui_state_reports_the_pointer_model() {
        let model = UiPointerModelData {
            id: 6,
            animation: 7,
            tint: [0.0, 1.0, 0.0, 1.0],
            phase_ms: Some(120.5),
            software: true,
        };
        let json = |pointer_model: Option<UiPointerModelData>| {
            serde_json::to_value(UiStateData {
                now_ms: 0,
                screen: [1024, 768],
                dialogs: Vec::new(),
                focus: None,
                hover: None,
                elements: Vec::new(),
                images: Vec::new(),
                busy: false,
                queued: 0,
                effects: Vec::new(),
                video: None,
                pointer_hidden: false,
                game: None,
                menu_level: None,
                accounting: None,
                play_time: None,
                pointer_model,
            })
            .unwrap()
        };
        let with = json(Some(model.clone()));
        assert_eq!(
            with["pointer_model"],
            json!({"id": 6, "animation": 7, "tint": [0.0, 1.0, 0.0, 1.0], "phase_ms": 120.5,
                   "software": true})
        );
        let back: UiStateData = serde_json::from_value(with).unwrap();
        assert_eq!(back.pointer_model, Some(model));
        let without = json(None);
        assert!(without.get("pointer_model").is_none(), "{without}");
        let back: UiStateData = serde_json::from_value(without).unwrap();
        assert_eq!(back.pointer_model, None);
    }

    #[test]
    fn ui_state_data_round_trips() {
        let data = UiStateData {
            now_ms: 1008,
            screen: [1024, 768],
            dialogs: vec![UiDialogData {
                name: "options".into(),
                rect: [0, 0, 1024, 768],
                animating: false,
                modal: false,
            }],
            focus: None,
            hover: Some(("options".into(), "ok".into())),
            elements: vec![UiElementData {
                dialog: "options".into(),
                name: "ok".into(),
                kind: "button".into(),
                rect: [10, 20, 30, 40],
                visible: true,
                enabled: true,
                hidden: false,
                checked: None,
                value: None,
                selection: None,
                items: Vec::new(),
                text: None,
            }],
            images: vec![("data/menü/bkg.tga".into(), 255)],
            busy: false,
            queued: 0,
            video: Some(UiVideoData {
                path: "data/videos/eng.bik".into(),
                frame: 3,
                frames: 100,
                abortable: true,
            }),
            pointer_hidden: true,
            game: Some(UiGameData {
                level: "data/levels/multiplayer/[2Players]-BattleBeach.lvl".into(),
                game_type: 4,
                menu_state: 1,
                steps: 640,
                checksum: hex_checksum(0xDEAD_BEEF_0000_0001),
                loading: None,
                load_phases: vec![0, 40, 70, 100],
                result: 2,
                hud: None,
                input: None,
                presentation: None,
                world: None,
                flags: None,
                minimap: None,
                diplomacy_open: false,
                chat_lines: Vec::new(),
                replay: None,
            }),
            menu_level: Some(UiMenuLevelData {
                level: "data/levels/menülevel/menü2.lvl".into(),
                shown: false,
                steps: 1200,
                checksum: hex_checksum(0x0123_4567_89AB_CDEF),
            }),
            accounting: Some(vec![UiAccountingRow {
                name: "Anna".into(),
                color: 3,
                values: std::array::from_fn(|i| (i * 7).to_string()),
            }]),
            play_time: Some("[CENTER]Game time: 0:05".into()),
            pointer_model: None,
            effects: vec![
                UiEffectData {
                    at_ms: 992,
                    effect: UiEffectKind::PlaySfx {
                        event: "SFX_KLICK".into(),
                        flags: 0x14,
                        category: 1,
                    },
                },
                UiEffectData {
                    at_ms: 16,
                    effect: UiEffectKind::Unhandled {
                        code: 0x20,
                        command: "HideLevelBackground".into(),
                    },
                },
                UiEffectData {
                    at_ms: 32,
                    effect: UiEffectKind::RestartMusic,
                },
                UiEffectData {
                    at_ms: 48,
                    effect: UiEffectKind::PlayVideo {
                        path: "data/videos/eng.bik".into(),
                        abortable: true,
                    },
                },
                UiEffectData {
                    at_ms: 64,
                    effect: UiEffectKind::StopVideo {
                        path: "data/videos/eng.bik".into(),
                    },
                },
                UiEffectData {
                    at_ms: 80,
                    effect: UiEffectKind::DeleteProfileSaves { guid: 10 },
                },
                UiEffectData {
                    at_ms: 80,
                    effect: UiEffectKind::WriteProfiles,
                },
                UiEffectData {
                    at_ms: 96,
                    effect: UiEffectKind::StartMatch {
                        level: "data/levels/multiplayer/[2Players]-BattleBeach.lvl".into(),
                        game_type: 4,
                        slots: vec![UiSlotData {
                            kind: 999,
                            nation: 2,
                            team: 1,
                            color: 3,
                            handicap: 0,
                        }],
                    },
                },
                UiEffectData {
                    at_ms: 112,
                    effect: UiEffectKind::ResumeGame {
                        game_speed: Some(14),
                    },
                },
                UiEffectData {
                    at_ms: 128,
                    effect: UiEffectKind::ResumeGame { game_speed: None },
                },
                UiEffectData {
                    at_ms: 144,
                    effect: UiEffectKind::ReloadLevel,
                },
                UiEffectData {
                    at_ms: 160,
                    effect: UiEffectKind::LoadGame { name: "new".into() },
                },
                UiEffectData {
                    at_ms: 176,
                    effect: UiEffectKind::SaveGame {
                        name: "quicksave1".into(),
                    },
                },
                UiEffectData {
                    at_ms: 192,
                    effect: UiEffectKind::DeleteSave { name: "old".into() },
                },
                UiEffectData {
                    at_ms: 208,
                    effect: UiEffectKind::MenuLevel {
                        load: Some("data/levels/menülevel/menü2.lvl".into()),
                    },
                },
                UiEffectData {
                    at_ms: 224,
                    effect: UiEffectKind::MenuLevel { load: None },
                },
                UiEffectData {
                    at_ms: 240,
                    effect: UiEffectKind::SetSoundDriver { index: 1 },
                },
            ],
        };
        let value = serde_json::to_value(&data).unwrap();
        assert_eq!(
            value["effects"][0],
            json!({"at_ms": 992, "kind": "play_sfx", "event": "SFX_KLICK", "flags": 20, "category": 1})
        );
        assert_eq!(
            value["effects"][2],
            json!({"at_ms": 32, "kind": "restart_music"})
        );
        assert_eq!(
            value["effects"][3],
            json!({"at_ms": 48, "kind": "play_video", "path": "data/videos/eng.bik", "abortable": true})
        );
        assert_eq!(
            value["effects"][4],
            json!({"at_ms": 64, "kind": "stop_video", "path": "data/videos/eng.bik"})
        );
        assert_eq!(
            value["effects"][5],
            json!({"at_ms": 80, "kind": "delete_profile_saves", "guid": 10})
        );
        assert_eq!(
            value["effects"][6],
            json!({"at_ms": 80, "kind": "write_profiles"})
        );
        assert_eq!(
            value["effects"][7],
            json!({"at_ms": 96, "kind": "start_match",
                   "level": "data/levels/multiplayer/[2Players]-BattleBeach.lvl", "game_type": 4,
                   "slots": [{"kind": 999, "nation": 2, "team": 1, "color": 3, "handicap": 0}]})
        );
        assert_eq!(
            value["effects"][8],
            json!({"at_ms": 112, "kind": "resume_game", "game_speed": 14})
        );
        assert_eq!(
            value["effects"][9],
            json!({"at_ms": 128, "kind": "resume_game", "game_speed": null})
        );
        assert_eq!(
            value["effects"][10],
            json!({"at_ms": 144, "kind": "reload_level"})
        );
        assert_eq!(
            value["effects"][11],
            json!({"at_ms": 160, "kind": "load_game", "name": "new"})
        );
        assert_eq!(
            value["effects"][12],
            json!({"at_ms": 176, "kind": "save_game", "name": "quicksave1"})
        );
        assert_eq!(
            value["effects"][13],
            json!({"at_ms": 192, "kind": "delete_save", "name": "old"})
        );
        // `0x21` with path, `0x20` without (the field is absent).
        assert_eq!(
            value["effects"][14],
            json!({"at_ms": 208, "kind": "menu_level", "load": "data/levels/menülevel/menü2.lvl"})
        );
        assert_eq!(
            value["effects"][15],
            json!({"at_ms": 224, "kind": "menu_level"})
        );
        // Switching the device in the sound options.
        assert_eq!(
            value["effects"][16],
            json!({"at_ms": 240, "kind": "set_sound_driver", "index": 1})
        );
        assert_eq!(
            value["menu_level"],
            json!({"level": "data/levels/menülevel/menü2.lvl", "shown": false, "steps": 1200,
                   "checksum": "0123456789abcdef"})
        );
        assert_eq!(
            value["video"],
            json!({"path": "data/videos/eng.bik", "frame": 3, "frames": 100, "abortable": true})
        );
        assert_eq!(value["pointer_hidden"], json!(true));
        assert_eq!(
            value["game"],
            json!({"level": "data/levels/multiplayer/[2Players]-BattleBeach.lvl", "game_type": 4,
                   "menu_state": 1, "steps": 640, "checksum": "deadbeef00000001",
                   "load_phases": [0, 40, 70, 100], "result": 2, "diplomacy_open": false}),
            "loading is absent outside of loading, `diplomacy_open` is always present, `minimap` and `chat_lines` are absent when empty"
        );
        // Protocol 29: accounting with rows and game time.
        assert_eq!(
            value["accounting"],
            json!([{"name": "Anna", "color": 3,
                    "values": ["0", "7", "14", "21", "28", "35", "42", "49", "56", "63", "70", "77"]}])
        );
        assert_eq!(value["play_time"], json!("[CENTER]Game time: 0:05"));
        assert_eq!(value["hover"], json!(["options", "ok"]));
        assert_eq!(
            value["elements"][0],
            json!({"dialog": "options", "name": "ok", "kind": "button", "rect": [10, 20, 30, 40],
                   "visible": true, "enabled": true, "hidden": false}),
            "fields without meaning are absent"
        );
        let back: UiStateData = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, data);

        // Without a match `game` is absent entirely (viewer, main menu), without an open accounting `accounting` and
        // `play_time`; older lines without the fields stay readable.
        let mut without = data.clone();
        without.game = None;
        without.menu_level = None;
        without.accounting = None;
        without.play_time = None;
        let value = serde_json::to_value(&without).unwrap();
        for field in ["game", "menu_level", "accounting", "play_time"] {
            assert!(value.get(field).is_none(), "{field}: {value}");
        }
        let back = serde_json::from_value::<UiStateData>(value).unwrap();
        assert_eq!(
            (back.game, back.accounting, back.play_time),
            (None, None, None)
        );
        assert_eq!(
            back.menu_level, None,
            "old lines without the field can be read"
        );
        // A report of protocol 27 without `result`: the player is playing (0).
        let mut old = serde_json::to_value(data.game.clone().unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("result");
        assert_eq!(serde_json::from_value::<UiGameData>(old).unwrap().result, 0);

        // During loading: phase set, with an empty course `load_phases` is absent.
        let loading = UiGameData {
            loading: Some(40),
            load_phases: Vec::new(),
            ..data.game.clone().unwrap()
        };
        let value = serde_json::to_value(&loading).unwrap();
        assert_eq!(value["loading"], json!(40));
        assert!(value.get("load_phases").is_none(), "{value}");
        assert_eq!(
            serde_json::from_value::<UiGameData>(value).unwrap(),
            loading
        );
    }

    /// `ui_state.game.hud` goes out and back as a line; without a status bar `status` is absent, older lines without
    /// `hud` stay readable.
    #[test]
    fn hud_data_round_trips() {
        let hud = UiHudData {
            dialog: GAME_UI_DIALOG.into(),
            windows: vec![UiHudWindow {
                kind: "status_bar".into(),
                rect: [0, 0, 1024, 26],
                layer: 2,
                visible: true,
            }],
            messages: vec![UiHudMessage {
                tag: "ECONOMY".into(),
                id: 0,
                text: "WE DO NOT HAVE ENOUGH FOOD!".into(),
                flags: 0x0301_0400,
                display_ms: 650,
                fade_ms: 1500,
                queued: vec![4, 7],
            }],
            action_stack: vec![[3, 4]],
            status: Some(UiStatusBarData {
                shown: true,
                texts: vec!["100".into(), "11/10".into(), "Anna".into()],
                red: true,
            }),
            toolbar: None,
        };
        let value = serde_json::to_value(&hud).unwrap();
        assert_eq!(
            value,
            json!({"dialog": "GAMEUI",
                   "windows": [{"kind": "status_bar", "rect": [0, 0, 1024, 26], "layer": 2, "visible": true}],
                   "messages": [{"tag": "ECONOMY", "id": 0, "text": "WE DO NOT HAVE ENOUGH FOOD!",
                                 "flags": 0x0301_0400, "display_ms": 650, "fade_ms": 1500, "queued": [4, 7]}],
                   "action_stack": [[3, 4]],
                   "status": {"shown": true, "texts": ["100", "11/10", "Anna"], "red": true}})
        );
        assert_eq!(serde_json::from_value::<UiHudData>(value).unwrap(), hud);
        let without = UiHudData {
            status: None,
            ..hud.clone()
        };
        let value = serde_json::to_value(&without).unwrap();
        assert!(value.get("status").is_none(), "{value}");
        assert_eq!(serde_json::from_value::<UiHudData>(value).unwrap(), without);
        // In the report of the match: `game.hud`; without a UI the field is absent, old lines read as `None`.
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 0,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: Some(hud.clone()),
            input: None,
            presentation: None,
            world: None,
            flags: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(value["hud"]["dialog"], "GAMEUI");
        assert_eq!(serde_json::from_value::<UiGameData>(value).unwrap(), game);
        let mut old = serde_json::to_value(UiGameData { hud: None, ..game }).unwrap();
        assert!(old.get("hud").is_none());
        old.as_object_mut().unwrap().remove("hud");
        assert_eq!(serde_json::from_value::<UiGameData>(old).unwrap().hud, None);
    }

    /// `ui_state.game.flags` goes out and back as a line; without flags the field is absent, older lines read as
    /// `None`.
    #[test]
    fn game_flags_round_trip() {
        let flags = crate::sim::SimFlagsInfo {
            flags: vec![crate::sim::SimFlagInfo {
                id: 0x0009_4801,
                tile: [5, 6],
                color: 9,
                holder: 8,
                check_ms: 1000,
            }],
            counts: [0, 0, 0, 0, 0, 0, 0, 0, 1],
            leader: 8,
            time_limit_s: 1200,
            remaining_ms: Some(60_000),
        };
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 2,
            menu_state: 0,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: None,
            presentation: None,
            world: None,
            flags: Some(flags),
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(value["flags"]["remaining_ms"], 60_000);
        assert_eq!(value["flags"]["flags"][0]["holder"], 8);
        assert_eq!(serde_json::from_value::<UiGameData>(value).unwrap(), game);
        let bare = UiGameData {
            flags: None,
            ..game
        };
        let line = serde_json::to_value(&bare).unwrap();
        assert!(line.get("flags").is_none(), "{line}");
        assert_eq!(
            serde_json::from_value::<UiGameData>(line).unwrap().flags,
            None
        );
    }

    /// `ui_state.game.input` goes out and back as a line; without an input part the field is absent, older lines read
    /// as `None`.
    #[test]
    fn input_data_round_trips() {
        let input = UiInputData {
            paused: true,
            shown_paused: true,
            game_speed: 13,
            config_speed: 13,
            groups: vec![vec![65537, 65538], Vec::new()],
            last_group: 0,
            pointer: 9,
            pointer_variant: 2,
            pointer_visible: false,
            key_context: [2, 1, 1400],
            gate: vec!["cinemascope".into(), "campaign".into()],
        };
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(
            value,
            json!({"paused": true, "shown_paused": true, "game_speed": 13, "config_speed": 13,
                   "groups": [[65537, 65538], []], "last_group": 0, "pointer": 9, "pointer_variant": 2,
                   "pointer_visible": false, "key_context": [2, 1, 1400], "gate": ["cinemascope", "campaign"]})
        );
        assert_eq!(serde_json::from_value::<UiInputData>(value).unwrap(), input);
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 0,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: Some(input),
            presentation: None,
            world: None,
            flags: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(value["input"]["last_group"], json!(0));
        assert_eq!(serde_json::from_value::<UiGameData>(value).unwrap(), game);
        let old = serde_json::to_value(UiGameData {
            input: None,
            ..game
        })
        .unwrap();
        assert!(old.get("input").is_none(), "{old}");
        assert_eq!(
            serde_json::from_value::<UiGameData>(old).unwrap().input,
            None
        );
    }

    /// `ui_state.game.presentation` goes out and back as a line; older lines without the field stay readable, without
    /// presentation the field is absent.
    #[test]
    fn presentation_data_round_trips() {
        let presentation = UiPresentationData {
            bars: [61, 61],
            portraits: vec![UiPortraitSlot {
                slot: 1,
                portrait: 1000,
                frame: 101,
                alpha: 255,
            }],
            texts: vec![UiTextSlot {
                slot: 0,
                text: 7,
                alpha: 128,
            }],
            fade_alpha: 64,
            pointer_visible: true,
            clear_color: [0, 0, 0],
            info_frames: vec![(20950, [10, 20, 310, 60])],
            countdown: Some("1:05".into()),
            timers: vec!["0:59".into()],
            screen_text: Some(7),
        };
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 0,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: None,
            presentation: Some(presentation.clone()),
            world: None,
            flags: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(
            value["presentation"],
            json!({"bars": [61, 61],
                   "portraits": [{"slot": 1, "portrait": 1000, "frame": 101, "alpha": 255}],
                   "texts": [{"slot": 0, "text": 7, "alpha": 128}],
                   "fade_alpha": 64, "pointer_visible": true, "clear_color": [0, 0, 0],
                   "info_frames": [[20950, [10, 20, 310, 60]]], "countdown": "1:05", "timers": ["0:59"],
                   "screen_text": 7})
        );
        assert_eq!(
            serde_json::from_value::<UiGameData>(value.clone()).unwrap(),
            game
        );
        // Lines without info windows, countdown, timers and screen text stay readable.
        let mut old = value.clone();
        for key in ["info_frames", "countdown", "timers", "screen_text"] {
            old["presentation"].as_object_mut().unwrap().remove(key);
        }
        let read = serde_json::from_value::<UiGameData>(old).unwrap();
        let bare = read.presentation.unwrap();
        assert_eq!(
            (
                bare.info_frames,
                bare.countdown,
                bare.timers,
                bare.screen_text
            ),
            (Vec::new(), None, Vec::new(), None)
        );
        let none = UiGameData {
            presentation: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            ..game
        };
        let value = serde_json::to_value(&none).unwrap();
        assert!(value.get("presentation").is_none(), "{value}");
        assert_eq!(serde_json::from_value::<UiGameData>(value).unwrap(), none);
    }

    /// `ui_state.game.world` (state of the world rendering) goes out and back as a line; without a map the field is
    /// absent, older lines without `world` read as `None`.
    #[test]
    fn world_display_data_round_trips() {
        let world = crate::state::StateSnapshot::example()
            .world_display
            .unwrap();
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 0,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: None,
            presentation: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            world: Some(world.clone()),
            flags: None,
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(value["world"]["rally_flag"]["building"], json!(0x0001_0005));
        assert_eq!(value["world"]["surf_rings"], json!(2));
        assert_eq!(
            serde_json::from_value::<UiGameData>(value.clone()).unwrap(),
            game
        );
        let none = UiGameData {
            world: None,
            flags: None,
            ..game.clone()
        };
        let line = serde_json::to_value(&none).unwrap();
        assert!(line.get("world").is_none(), "{line}");
        assert_eq!(serde_json::from_value::<UiGameData>(line).unwrap(), none);
        // A line from an older version (without `world`).
        let mut old = value;
        old.as_object_mut().unwrap().remove("world");
        assert_eq!(
            serde_json::from_value::<UiGameData>(old).unwrap().world,
            None
        );
    }

    /// Minimap, diplomacy dialog and chat lines go out and back as a line (protocol round trip); empty parts are
    /// absent, older lines without the fields read as `None`/`false`/empty.
    #[test]
    fn minimap_diplomacy_and_chat_round_trip() {
        let minimap = UiMinimapData {
            visible: true,
            mode: 2,
            center: [935, 679],
            theta: 1.5,
            pings: vec![(70, 101, 10), (3, 4, 1)],
            idle_workers: 2,
            fog_swaps: 5,
        };
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 10,
            steps: 1,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: None,
            presentation: None,
            world: None,
            flags: None,
            minimap: Some(minimap.clone()),
            diplomacy_open: true,
            chat_lines: vec!["Anna: Hello".into()],
            replay: None,
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(
            value["minimap"],
            json!({"visible": true, "mode": 2, "center": [935, 679], "theta": 1.5,
                   "pings": [[70, 101, 10], [3, 4, 1]], "idle_workers": 2, "fog_swaps": 5})
        );
        assert_eq!(value["diplomacy_open"], true);
        assert_eq!(value["chat_lines"], json!(["Anna: Hello"]));
        assert_eq!(
            serde_json::from_value::<UiGameData>(value.clone()).unwrap(),
            game
        );
        // Older lines without the three fields stay readable.
        let mut old = value;
        for key in ["minimap", "diplomacy_open", "chat_lines"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let read = serde_json::from_value::<UiGameData>(old).unwrap();
        assert_eq!(
            (read.minimap, read.diplomacy_open, read.chat_lines),
            (None, false, Vec::new())
        );
        // Without map, dialog and chat, `minimap` and `chat_lines` are absent from the line.
        let bare = UiGameData {
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            ..game
        };
        let value = serde_json::to_value(&bare).unwrap();
        assert!(value.get("minimap").is_none(), "{value}");
        assert!(value.get("chat_lines").is_none(), "{value}");
        assert_eq!(value["diplomacy_open"], false);
        assert_eq!(serde_json::from_value::<UiGameData>(value).unwrap(), bare);
    }

    /// Recording and replay (`ui_state.game.replay`) go out and back as a line; the kind is lowercase (`record`,
    /// `play`), without recording and replay the field is absent, older lines without it read as `None`.
    #[test]
    fn ui_replay_round_trips() {
        let game = UiGameData {
            level: "x.lvl".into(),
            game_type: 0,
            menu_state: 0,
            steps: 120,
            checksum: hex_checksum(1),
            loading: None,
            load_phases: Vec::new(),
            result: 0,
            hud: None,
            input: None,
            presentation: None,
            world: None,
            flags: None,
            minimap: None,
            diplomacy_open: false,
            chat_lines: Vec::new(),
            replay: Some(UiReplayData {
                mode: UiReplayMode::Record,
                file: "261108-1435".into(),
                step: 120,
                ended: false,
            }),
        };
        let value = serde_json::to_value(&game).unwrap();
        assert_eq!(
            value["replay"],
            json!({"mode": "record", "file": "261108-1435", "step": 120, "ended": false})
        );
        assert_eq!(
            serde_json::from_value::<UiGameData>(value.clone()).unwrap(),
            game
        );
        let play = UiReplayData {
            mode: UiReplayMode::Play,
            file: "261108-1435".into(),
            step: 300,
            ended: true,
        };
        let line = serde_json::to_value(&play).unwrap();
        assert_eq!(
            line,
            json!({"mode": "play", "file": "261108-1435", "step": 300, "ended": true})
        );
        assert_eq!(serde_json::from_value::<UiReplayData>(line).unwrap(), play);
        assert!(
            serde_json::from_value::<UiReplayData>(
                json!({"mode": "Play", "file": "x", "step": 0, "ended": false})
            )
            .is_err()
        );
        // Without recording and replay the field is absent; a line from an older version reads as `None`.
        let bare = UiGameData {
            replay: None,
            ..game
        };
        let mut old = serde_json::to_value(&bare).unwrap();
        assert!(old.get("replay").is_none(), "{old}");
        old.as_object_mut().unwrap().remove("replay");
        assert_eq!(serde_json::from_value::<UiGameData>(old).unwrap(), bare);
    }

    /// The effect `diplomacy` has a fixed form (`action`, plus `slot` and `value` where they apply).
    #[test]
    fn diplomacy_effect_has_a_fixed_form() {
        let cases = [
            (
                json!({"at_ms": 4, "kind": "diplomacy", "action": "status", "slot": 3, "value": 0}),
                UiEffectKind::Diplomacy {
                    action: "status".into(),
                    slot: Some(3),
                    value: Some(0),
                },
            ),
            (
                json!({"at_ms": 4, "kind": "diplomacy", "action": "chat_by", "value": 1}),
                UiEffectKind::Diplomacy {
                    action: "chat_by".into(),
                    slot: None,
                    value: Some(1),
                },
            ),
            (
                json!({"at_ms": 4, "kind": "diplomacy", "action": "close"}),
                UiEffectKind::Diplomacy {
                    action: "close".into(),
                    slot: None,
                    value: None,
                },
            ),
        ];
        for (line, effect) in cases {
            let expected = UiEffectData { at_ms: 4, effect };
            assert_eq!(
                serde_json::from_value::<UiEffectData>(line.clone()).unwrap(),
                expected
            );
            assert_eq!(serde_json::to_value(&expected).unwrap(), line);
        }
    }

    /// `ui_state.game.hud.toolbar` goes out and back as a line; empty parts (`portrait`, `tooltip`, `cursor`,
    /// `last_button`) are absent, and without `toolbar` the line of the game UI stays as it was before the field
    /// existed.
    #[test]
    fn toolbar_data_round_trips() {
        let full = UiToolbarData {
            shown_menu: 1300,
            selected: 65537,
            hover: 0,
            buttons: vec![
                UiToolbarButton {
                    element: "normal:3".into(),
                    msg: 534,
                    kind: 0x0200_0000,
                    state: 1,
                    rect: [10, 600, 60, 650],
                    progress: 100,
                },
                UiToolbarButton {
                    element: "queue:0".into(),
                    msg: 1410,
                    kind: 0x0600_0000,
                    state: 0,
                    rect: [70, 600, 120, 650],
                    progress: 42,
                },
            ],
            portrait: Some(UiPortraitData {
                guid: 1300,
                subtitle: "Headquarters".into(),
                values: vec![(14, "100/100".into()), (28, "5[COLOR FF00FF00]+1".into())],
            }),
            tooltip: Some(UiTooltipData {
                guid: 534,
                kind: 0x0200_0000,
                text: "Colonist
Cost"
                    .into(),
                rect: [0, 400, 120, 568],
            }),
            counter_upgrades: vec![(602, "1:05".into())],
            cursor: Some("build".into()),
            last_button: Some(534),
        };
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(
            value,
            json!({"shown_menu": 1300, "selected": 65537, "hover": 0,
                   "buttons": [
                       {"element": "normal:3", "msg": 534, "kind": 0x0200_0000, "state": 1,
                        "rect": [10, 600, 60, 650], "progress": 100},
                       {"element": "queue:0", "msg": 1410, "kind": 0x0600_0000, "state": 0,
                        "rect": [70, 600, 120, 650], "progress": 42}],
                   "portrait": {"guid": 1300, "subtitle": "Headquarters",
                                "values": [[14, "100/100"], [28, "5[COLOR FF00FF00]+1"]]},
                   "tooltip": {"guid": 534, "kind": 0x0200_0000, "text": "Colonist
Cost",
                               "rect": [0, 400, 120, 568]},
                   "counter_upgrades": [[602, "1:05"]],
                   "cursor": "build", "last_button": 534})
        );
        assert_eq!(
            serde_json::from_value::<UiToolbarData>(value).unwrap(),
            full
        );
        let empty = UiToolbarData {
            buttons: Vec::new(),
            portrait: None,
            tooltip: None,
            counter_upgrades: Vec::new(),
            cursor: None,
            last_button: None,
            ..full.clone()
        };
        let value = serde_json::to_value(&empty).unwrap();
        for key in ["portrait", "tooltip", "cursor", "last_button"] {
            assert!(value.get(key).is_none(), "{key}: {value}");
        }
        assert_eq!(
            serde_json::from_value::<UiToolbarData>(value).unwrap(),
            empty
        );
        // In the game UI: without `toolbar` the field is absent (older lines stay readable).
        let hud = UiHudData {
            dialog: GAME_UI_DIALOG.into(),
            windows: Vec::new(),
            messages: Vec::new(),
            action_stack: Vec::new(),
            status: None,
            toolbar: Some(full),
        };
        let value = serde_json::to_value(&hud).unwrap();
        assert_eq!(value["toolbar"]["buttons"][0]["element"], "normal:3");
        assert_eq!(serde_json::from_value::<UiHudData>(value).unwrap(), hud);
        let plain = UiHudData {
            toolbar: None,
            ..hud
        };
        let value = serde_json::to_value(&plain).unwrap();
        assert!(value.get("toolbar").is_none(), "{value}");
        assert_eq!(serde_json::from_value::<UiHudData>(value).unwrap(), plain);
    }

    /// The game effects go out and back as a line (field `kind` in lowercase with underscore).
    #[test]
    fn new_effects_round_trip() {
        let cases = [
            (
                json!({"at_ms": 96, "kind": "start_match",
                       "level": "data/levels/multiplayer/[2Players]-BattleBeach.lvl", "game_type": 4,
                       "slots": [{"kind": 999, "nation": 2, "team": 1, "color": 3, "handicap": 0},
                                 {"kind": 2, "nation": 1, "team": 2, "color": 4, "handicap": 5}]}),
                UiEffectKind::StartMatch {
                    level: "data/levels/multiplayer/[2Players]-BattleBeach.lvl".into(),
                    game_type: 4,
                    slots: vec![
                        UiSlotData {
                            kind: 999,
                            nation: 2,
                            team: 1,
                            color: 3,
                            handicap: 0,
                        },
                        UiSlotData {
                            kind: 2,
                            nation: 1,
                            team: 2,
                            color: 4,
                            handicap: 5,
                        },
                    ],
                },
            ),
            (
                json!({"at_ms": 96, "kind": "load_game", "name": "new"}),
                UiEffectKind::LoadGame { name: "new".into() },
            ),
            (
                json!({"at_ms": 96, "kind": "save_game", "name": "quicksave1"}),
                UiEffectKind::SaveGame {
                    name: "quicksave1".into(),
                },
            ),
            (
                json!({"at_ms": 96, "kind": "delete_save", "name": "old"}),
                UiEffectKind::DeleteSave { name: "old".into() },
            ),
            (
                json!({"at_ms": 96, "kind": "play_replay", "name": "261108-1435"}),
                UiEffectKind::PlayReplay {
                    name: "261108-1435".into(),
                },
            ),
            (
                json!({"at_ms": 96, "kind": "reload_level"}),
                UiEffectKind::ReloadLevel,
            ),
            (
                json!({"at_ms": 96, "kind": "resume_game", "game_speed": 14}),
                UiEffectKind::ResumeGame {
                    game_speed: Some(14),
                },
            ),
            (
                json!({"at_ms": 96, "kind": "resume_game", "game_speed": null}),
                UiEffectKind::ResumeGame { game_speed: None },
            ),
            (
                json!({"at_ms": 96, "kind": "write_profiles"}),
                UiEffectKind::WriteProfiles,
            ),
            (
                json!({"at_ms": 96, "kind": "game_end", "victory": true}),
                UiEffectKind::GameEnd { victory: true },
            ),
            (
                json!({"at_ms": 96, "kind": "accounting"}),
                UiEffectKind::EndMatch,
            ),
            (
                json!({"at_ms": 96, "kind": "surrender", "slot": 2}),
                UiEffectKind::Surrender { slot: 2 },
            ),
            (
                json!({"at_ms": 96, "kind": "start_campaign_level", "level": "data/levels/k.lvl", "difficulty": 0}),
                UiEffectKind::StartCampaignLevel {
                    level: "data/levels/k.lvl".into(),
                    difficulty: 0,
                },
            ),
            (
                json!({"at_ms": 96, "kind": "book_campaign", "won": true}),
                UiEffectKind::BookCampaign { won: true },
            ),
            (
                json!({"at_ms": 96, "kind": "delete_profile_saves", "guid": 10}),
                UiEffectKind::DeleteProfileSaves { guid: 10 },
            ),
        ];
        for (line, kind) in cases {
            let expected = UiEffectData {
                at_ms: 96,
                effect: kind,
            };
            assert_eq!(
                serde_json::from_value::<UiEffectData>(line.clone()).unwrap(),
                expected,
                "{line}"
            );
            assert_eq!(serde_json::to_value(&expected).unwrap(), line);
        }
    }
}
