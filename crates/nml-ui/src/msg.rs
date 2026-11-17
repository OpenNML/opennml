//! Messages from the widgets to the dialog logic, application commands and effects to the outside
//! (re/menu-system 3.3, 12.1, 12.5; re/menu-flow, section "Commands").
//!
//! The widgets "pull" their data: they send an [`ElementMsg`] to the [`UiListener`], which reads parameters
//! **and writes** (return channel). What the UI causes on the outside (sound, display, settings) it returns as
//! [`UiEffect`]; it plays nothing itself.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use nml_formats::config::lower_german;

use crate::draft::MatchRequest;
use crate::screens::DiplomacyAction;

/// Codes of the element message `0x12E` (`FUN_00464970`, table in re/menu-system 3.3).
pub mod code {
    /// ComboBox, Slider, RadioButton, ListBox: query the current value (`int`).
    pub const QUERY_VALUE: u32 = 0;
    /// Slider, RadioButton: value changed (`int`).
    pub const VALUE_CHANGED: u32 = 1;
    /// ComboBox, ListBox: query the entries (`array`, `indexarray`).
    pub const QUERY_ITEMS: u32 = 2;
    /// Free the entries.
    pub const FREE_ITEMS: u32 = 3;
    /// ListBox: entry selected (`int`).
    pub const ITEM_SELECTED: u32 = 4;
    /// Query the status (`status` <- bits `HIDE`/`DISABLE`).
    pub const QUERY_STATUS: u32 = 5;
    /// Slider: query the number of steps (`resolution`).
    pub const QUERY_RESOLUTION: u32 = 6;
    /// Query the text (`text`; EditField also `size`, `inputbuffer`).
    pub const QUERY_TEXT: u32 = 7;
    /// Button triggered (click, ESC, ENTER).
    pub const TRIGGERED: u32 = 8;
    /// Timer: next query interval (`timeout` in ms).
    pub const QUERY_TIMEOUT: u32 = 9;
    /// EditField: key (`key` or `keydef`); a response != -1 discards the character.
    pub const EDIT_KEY: u32 = 0xB;
    /// EditField: text changed (`text`).
    pub const EDIT_CHANGED: u32 = 0xC;
    /// ListBox: double-click.
    pub const DOUBLE_CLICK: u32 = 0xD;
}

/// Text buffer of the logic that the widgets share through a reference: in the original a pointer into the memory of
/// the application (e.g. `GameApp+0x9A0`) that Label (`FUN_00462490`, `+0x98 = *text`) and EditField with
/// `inputbuffer` (`FUN_0045F5D0`, `+0xC0 = *text`) keep and read on every draw. What the logic writes into it later
/// they show without a new query; what the EditField writes, the logic sees. Content is UTF-16 like `wchar_t`.
/// Comparison and `Debug` go by the content; `clone` shares the buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedText(Rc<RefCell<Vec<u16>>>);

impl SharedText {
    pub fn new(text: &str) -> SharedText {
        let shared = SharedText::default();
        shared.set(text);
        shared
    }

    /// Content as text (up to the first null character, like `wcslen`).
    pub fn get(&self) -> String {
        String::from_utf16_lossy(&self.units())
    }

    /// Replace the content (`wcscpy`).
    pub fn set(&self, text: &str) {
        self.set_units(text.encode_utf16().take_while(|&u| u != 0).collect());
    }

    /// Content as UTF-16 units.
    pub fn units(&self) -> Vec<u16> {
        self.0.borrow().clone()
    }

    pub fn set_units(&self, units: Vec<u16>) {
        *self.0.borrow_mut() = units;
    }
}

/// Value of a parameter of the element message (name table of the message object, vtable `0x0061A920`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    Int(i32),
    Text(String),
    /// Text as a reference to a buffer of the logic ([`SharedText`]); widgets without pointer semantics copy it.
    Shared(SharedText),
    /// `array`: entries of a list.
    Array(Vec<String>),
    /// `indexarray`: values for the entries.
    IndexArray(Vec<i32>),
}

/// Element message `{0x12E, 1, &object}` (`FUN_00464970`): dialog and element name in **lowercase**
/// (`CString_MakeLowerGerman`), code and parameters. The dialog logic compares the names exactly and in lowercase
/// (dispatcher `0x004DC280`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementMsg {
    pub dialog: String,
    pub element: String,
    pub code: u32,
    pub params: BTreeMap<String, Param>,
}

impl ElementMsg {
    /// New message without parameters; the names are lowercased like `CString_MakeLowerGerman`.
    pub fn new(dialog: &str, element: &str, code: u32) -> ElementMsg {
        ElementMsg {
            dialog: lower_german(dialog),
            element: lower_german(element),
            code,
            params: BTreeMap::new(),
        }
    }
}

/// Listener of the widgets (`MenuSystem` and `MenuListener` on the desktop, `Widget_Notify` `FUN_00459F70`).
pub trait UiListener {
    /// Element message `0x12E`; -1 = no listener has answered. `fx` is the same effect list that the widgets write
    /// into (`UiEnv::fx`): effects of the logic thus stand in the order of the original between those of the
    /// widgets.
    fn element(&mut self, msg: &mut ElementMsg, fx: &mut Vec<UiEffect>) -> i32;
    /// "May it be triggered?" (`0x12F`): 0 = yes, -1 = no listener (counts as no, re/menu-system 12.6).
    fn may_trigger(&mut self) -> i32;
    /// Enqueue an application command (`FUN_004BCB40`, `AddTail` at `GameApp+0xD6B0`) - the way of the `.dlg` reader
    /// during construction (`0x20`/`0x21`, `0x004DDEF7`), so that its commands stand in the same queue, in order of
    /// creation, before those of the logic. Without an application (static drawing, tests) the command is dropped.
    fn push_command(&mut self, _cmd: AppCmd) {}
    /// The targeted requeries that the logic has demanded since the last call ([`Requery`]), in order; `MenuApp`
    /// works them off after the message returns. Without an application there are none.
    fn take_requery(&mut self) -> Vec<Requery> {
        Vec::new()
    }
}

/// Targeted requery "requery X" (`FUN_00464970` builds the message `{0x12E, code}` for frame and element,
/// `FUN_004DD600` looks the element up in the name table and calls its `OnMessage`, slot 19 `+0x4C`;
/// re/singleplayer-menus part 1, section "Introduction"). Names in lowercase like in the messages; code 5 requeries
/// the status ([`crate::tree::Broadcast::Requery`]), code 2 the entries ([`crate::tree::Broadcast::RequeryItems`]).
///
/// Reimplementation: the original calls the element in the middle of the handler; `MenuApp` works off the list after
/// each `MenuApp::step` (the tree is borrowed during the message). A difference would only be visible if the handler
/// afterwards did something that depends on the response. Most of them requery as the last thing. Exception:
/// `gametype` code 4 in `CREATESINGLEPLAYERGAME` queries `leveltype` code 2 in the middle of the flow and
/// afterwards evaluates `LevelType`; but a ComboBox answers only code 5 of `0x12E` (`FUN_0045E8A0`), so the requery
/// changes nothing that the rest of the handler reads (O-129, O-187).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requery {
    pub dialog: String,
    pub element: String,
    pub code: u32,
}

impl Requery {
    pub fn new(dialog: &str, element: &str, code: u32) -> Requery {
        Requery {
            dialog: lower_german(dialog),
            element: lower_german(element),
            code,
        }
    }
}

/// Listener that never answers (widgets without dialog logic, static drawing, tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoListener;

impl UiListener for NoListener {
    fn element(&mut self, _msg: &mut ElementMsg, _fx: &mut Vec<UiEffect>) -> i32 {
        -1
    }

    fn may_trigger(&mut self) -> i32 {
        -1
    }
}

/// Application command of the queue `[MenuSystem+0x10]` (jump table `0x004CA66C`, re/menu-flow, section
/// "Commands"). One command runs per frame after drawing (re/menu-system 12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCmd {
    /// 3 (`wait`) / 4: close the dialog (`FUN_004E2E80`).
    Close { name: String, wait: bool },
    /// 5 (`wait`) / 6: open the dialog (`FUN_004E3010`).
    Open { name: String, wait: bool },
    /// `0x15`: wait until the dialog root has no more children (`FUN_004DCE80`, corrected in 12.1.10).
    WaitDialogs,
    /// `0x1A`: wait until the video has finished.
    WaitVideo,
    /// `0x0D`: start a free match (`0x004CA274`...`0x004CA28A`: `Game_StartLevel` without parameters; all start
    /// values are in the session, re/singleplayer-menus part 2, 5). `MenuApp` hands [`UiEffect::StartMatch`] to the
    /// host.
    StartMatch,
    /// `0x30`: wait (ms).
    WaitMs(u32),
    /// `0x1B`: logo/intro video (0, 1 or 2) ([`crate::video::INTRO_VIDEOS`]).
    Video(u8),
    /// `0x1D`: video by path (campaign video, `ui_video`); pointer off.
    PlayVideo(String),
    /// `0x24`: after the intro back to the display mode, pointer on.
    AfterIntro,
    /// `0x28`: after the campaign video (`0x004CA4F0`...`0x004CA643`, re/singleplayer-menus part 1, 6.1): pointer on,
    /// game type of the session -1, then automatically continue into a level (clear the queue, `{0x0C}`) or clear
    /// the queue, restart the menu music and open `selectcampaign`.
    AfterVideo,
    /// After the probe video of `ui_video` (`MenuApp::play_video_by_name`): only the part of `0x28` that does not
    /// belong to the campaign - pointer on, restart the menu music; without campaign logic, without clearing the
    /// queue (afterwards the menus that were open before open again). Not a command of the original; reports
    /// itself with the code of `0x28`.
    AfterProbeVideo,
    /// `0x0F`: close the game menu (`FUN_0042E290`, re/ingame-menus 1.2); `RETURN` of the game menu.
    CloseGameMenu,
    /// `0x13` (`0x004CA328`, re/multiplayer-menus L.9): close the network game menu (`FUN_0042E5D0`: only at state 2:
    /// state 0 and `{4,"INGAMEOPTIONS2"}`; the reimplementation does not send the game speed command `0x6E`,
    /// E-265); without an engine `FUN_004E2E80("ingameoptions2", 0)`. `RETURN2` of `INGAMEOPTIONS2`; `MenuApp`
    /// executes it itself.
    CloseNetGameMenu,
    /// `0x1F`: reload the current level (only with a match, otherwise no effect).
    ReloadLevel,
    /// `0x20`: 3D menu background off.
    HideLevelBackground,
    /// `0x21`: load the path as menu level.
    LoadMenuLevel(String),
    /// `0x25`: quit the program.
    Quit,
    /// `0x27`: give up the match `{0x27, 2, slot, 1}` - in single player the defeat of the slot immediately
    /// (message `0x50000056` -> `GameEngine_SetPlayerResult(slot, 1)`, re/ingame-menus 4); `MenuApp` hands
    /// [`UiEffect::Surrender`] to the host.
    Surrender { slot: u8 },
    /// `0x0E`: end of game (`0x004C9F77`...`0x004CA03B`, re/accounting 6): end the network, remember the result,
    /// build the accounting, game time text, end the game, open `accounting`. `MenuApp` hands [`UiEffect::EndMatch`]
    /// to the host; it builds the table and the time text, ends the match and calls `MenuApp::set_accounting`.
    EndMatch,
    /// `0x0C`: start a campaign level (`0x004CA253`, `FUN_004C8EC0(path, -1)`, re/singleplayer-menus part 1, 6.2):
    /// path of the current entry, game type of the session -1; `MenuApp` hands [`UiEffect::StartCampaignLevel`] with
    /// the difficulty `GameApp+0x19B4` to the host.
    StartCampaignLevel,
    /// `0x1C`: book the current campaign entry as won and write `profiles.ini` (`0x004CA443`, `FUN_004C6B10(1)`,
    /// re/singleplayer-menus part 1, 5): before a campaign video (`VIDEOSTART`, `ACCOUNTING` "Continue").
    /// `MenuApp::book_campaign(true)`.
    BookCampaign,
    /// `0x29`: delete the profile with this index, reopen `selectprofile` (`0x004C9D73`...`0x004C9E14`,
    /// re/singleplayer-menus part 1, 6.3).
    DeleteProfile(i32),
    /// `0x32`: reset the key layout to the defaults.
    ResetKeys,
    /// `0x34`: close all menus (`FUN_004E2C10`).
    CloseAll,
    /// `0x36`: set the GameSpy flag, close the window.
    GameSpy,
    /// `0x37`: leave the game menu without a return command (`FUN_0042F6B0`, way back from `INGAMELOADGAME`,
    /// re/ingame-menus 1.2).
    ResumeAfterLoadDialog,
    /// `0x0A`: end the running game and load the savegame `GameApp+0x177C` (`0x004CA224`, `FUN_004C8DE0`,
    /// re/ingame-menus 4); `MenuApp` hands [`UiEffect::LoadGame`] to the host.
    LoadGame,
    /// `0x26`: submit the savegame (`Game_SubmitSaveGameCommand(0)` `0x004BEEA0`): state 0, message
    /// `0x50000054` -> `GameEngine_CmdSaveGame` (`0x0042DF50`): `{6,"SAVINGMESSAGE"}`, `{0x0B}` (re/ingame-menus 4.1).
    SubmitSave,
    /// `0x0B`: write the savegame (`FUN_004C7D50(0)`), then close `savingmessage`; `MenuApp` hands
    /// [`UiEffect::SaveGame`] to the host.
    WriteSave,
    /// `0x2A`: delete the savegame `GameApp+0x177C` (`FUN_004BF9A0`), then close the frame with this name and
    /// reopen it (`0x004C9D4B`...`0x004C9D69`).
    DeleteSave(String),
    /// `7` (`0x004CA0FA`, re/multiplayer-menus L.9): join the chosen session (`LAN1 JOIN`/double-click); on
    /// success clear the chat and open the lobby, otherwise silently `{5,"lan1"}`. `MenuApp` hands
    /// [`UiEffect::Net`] to the host.
    Join,
    /// `8` (`0x004CA1B5`): host the match from the session (`LAN3 CREATE`); on success clear the chat and open the
    /// lobby, otherwise `{5,"lan1"}` ([`UiEffect::Net`]).
    Host,
    /// `9` (`0x004CA214`): network closed, slot list empty (`LAN1 CANCEL`, lobby `EXIT`, `0x2D`, `0x2F`;
    /// [`UiEffect::Net`]).
    CloseNet,
    /// `0x2D` (`0x004C9B84`): lobby closed, `{9}`, `{5,"start"}` - "OK" of message 15212 after a removal or after the
    /// host has left ([`UiEffect::Net`]).
    LeaveLobby,
    /// `0x2F` (`0x004C9AFF`, `HOST_MIGRATE`): without a match `{9}` and message 15212 with `{0x2D}`; the host
    /// migration with a match is omitted ([`UiEffect::Net`]).
    HostGone,
    /// `0x12` (`FUN_004C80D0`, re/multiplayer-saves S.2.2): load the network savegame from the name buffer
    /// (`GameApp+0x177C`) and host from it (`LAN3 CREATE` with `LevelType` 2); on success open `LAN4`, otherwise a
    /// message and `{5,"lan3"}` ([`UiEffect::Net`]).
    HostSavedGame,
    /// `0x14` (`0x004CA36C`, S.4.2): start the loaded match (`LAN4 START`; [`UiEffect::Net`]).
    StartSavedGame,
    /// `0x19` (`FUN_004C9000`, `LOADAUTOPLAY START`): play the chosen recording (`GameApp+0x177C`); `MenuApp` hands
    /// [`UiEffect::PlayReplay`] to the host.
    PlayReplay,
}

impl AppCmd {
    /// Command code of the original.
    pub fn code(&self) -> u32 {
        match self {
            AppCmd::Close { wait: true, .. } => 3,
            AppCmd::Close { wait: false, .. } => 4,
            AppCmd::Open { wait: true, .. } => 5,
            AppCmd::Open { wait: false, .. } => 6,
            AppCmd::WaitDialogs => 0x15,
            AppCmd::WaitVideo => 0x1A,
            AppCmd::StartMatch => 0x0D,
            AppCmd::WaitMs(_) => 0x30,
            AppCmd::Video(_) => 0x1B,
            AppCmd::PlayVideo(_) => 0x1D,
            AppCmd::AfterIntro => 0x24,
            AppCmd::AfterVideo | AppCmd::AfterProbeVideo => 0x28,
            AppCmd::CloseGameMenu => 0x0F,
            AppCmd::CloseNetGameMenu => 0x13,
            AppCmd::ReloadLevel => 0x1F,
            AppCmd::HideLevelBackground => 0x20,
            AppCmd::LoadMenuLevel(_) => 0x21,
            AppCmd::Quit => 0x25,
            AppCmd::Surrender { .. } => 0x27,
            AppCmd::EndMatch => 0x0E,
            AppCmd::StartCampaignLevel => 0x0C,
            AppCmd::BookCampaign => 0x1C,
            AppCmd::DeleteProfile(_) => 0x29,
            AppCmd::ResetKeys => 0x32,
            AppCmd::CloseAll => 0x34,
            AppCmd::GameSpy => 0x36,
            AppCmd::ResumeAfterLoadDialog => 0x37,
            AppCmd::LoadGame => 0x0A,
            AppCmd::SubmitSave => 0x26,
            AppCmd::WriteSave => 0x0B,
            AppCmd::DeleteSave(_) => 0x2A,
            AppCmd::Join => 7,
            AppCmd::Host => 8,
            AppCmd::CloseNet => 9,
            AppCmd::LeaveLobby => 0x2D,
            AppCmd::HostGone => 0x2F,
            AppCmd::HostSavedGame => 0x12,
            AppCmd::StartSavedGame => 0x14,
            AppCmd::PlayReplay => 0x19,
        }
    }
}

/// Display mode (width, height in pixels, color depth in bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: i32,
    pub height: i32,
    pub bits: i32,
}

/// Effect of the UI on the outside; the host executes it.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEffect {
    /// `SoundManager_PlayEvent2D` (e.g. `SFX_KLICK`, flags `0x14`, category 1).
    PlaySfx {
        event: String,
        flags: u32,
        category: u8,
    },
    /// Speech sample as a file.
    PlaySpeech {
        path: String,
    },
    /// Menu music (`0x160`); `looping` = without `MENUFRAME_DONOTLOOPMUSIC`.
    MenuMusic {
        path: String,
        looping: bool,
    },
    /// Restart the menu music (`0x161`).
    RestartMusic,
    /// Recreate the sound device with the driver `m_DriverID` (`SoundManager_CreateInterface(-1)`,
    /// `SoundManager_InitDevice(index, -1)`; `sounddriver` code 4 with a different index, before the `0x161`).
    SetSoundDriver(i32),
    /// Volume of a category (0 music, 1 sounds, 2 speech).
    SetVolume {
        category: u8,
        value: i32,
    },
    SetGamma(f32),
    SetHardwareCursor(bool),
    SetDisplayMode(DisplayMode),
    /// Write the settings file.
    WriteSettings,
    /// Write `profiles.ini` (`FUN_004C6A90`); the host reads the content from [`crate::app::MenuApp::profiles`].
    WriteProfiles,
    /// Delete the savegames of a deleted profile: `savegame/*.<GUID %03X>` (`FUN_004C6D70`, only `savegame/`,
    /// not the subfolders; O-29).
    DeleteProfileSaves {
        guid: i32,
    },
    Quit,
    /// Open and play a video (`FUN_0045D560`); the host reports the opened video with
    /// [`crate::app::MenuApp::video_started`]. `abortable` = menu root `+0xBC`.
    PlayVideo {
        path: String,
        abortable: bool,
    },
    /// End the video (`FUN_0045CA70`: close Bink, fade out the voice track within 1000 ms) - at the end, on abort by
    /// key and before a new video.
    StopVideo {
        path: String,
    },
    /// Start a match (command `0x0D`, `Game_StartLevel`): map, game type, occupied slots and settings from the
    /// session.
    StartMatch(MatchRequest),
    /// The game continues (game menu closed, state `Engine+0x2CCC` = 0): `game_speed` is, at `0x0F`, the value from
    /// `EngineConfig.GameSpeed`, which the original writes directly into `Engine+0x2D30` (without command `0x6E`,
    /// O-153), at `0x37` `None` (the speed stays). The host takes it over.
    ResumeGame {
        game_speed: Option<u8>,
    },
    /// End the running match and restart the same level (command `0x1F`, `FUN_004C8EC0`), without booking in the
    /// campaign; only with a match.
    ReloadLevel,
    /// Load the savegame `name` of the active profile (command `0x0A`, path `savegame/<name>.<GUID %03X>`); the host
    /// checks the file before it ends the running match (a deviation from the original).
    LoadGame {
        name: String,
    },
    /// Write the savegame `name` of the active profile (command `0x0B`); the UI has already closed `SAVINGMESSAGE`
    /// (the error path is a deviation from the original).
    SaveGame {
        name: String,
    },
    /// Delete the savegame `name` of the active profile (command `0x2A`, `DeleteFileA`); the dialog reopens afterwards.
    DeleteSave {
        name: String,
        /// Directory in which to delete (`GameApp+0x1984`): `savegame/` or `savegame/multiplayer/`.
        dir: crate::saves::SaveDir,
    },
    /// Play the recording `name` (full file name in `savegame/autoplay`) (command `0x19`); the host runs the
    /// replay.
    PlayReplay {
        name: String,
    },
    /// The end screen is queued (`GameEngine_SetPlayerResult` `0x00430CE0` step 4 for the local player,
    /// `MenuApp::show_game_end`): report only, the host does nothing.
    GameEnd {
        victory: bool,
    },
    /// Command `0x0E`: the host builds the accounting from the simulation (`FUN_00403FA0`) and the game time text,
    /// ends the match (`FUN_004BAF40`) and calls `MenuApp::set_accounting` (re/accounting 6).
    EndMatch,
    /// Command `0x27`: slot `slot` gives up - in single player the host executes the message
    /// `{0x50000056, 2, slot, 1}` **immediately** (defeat, re/ingame-menus 4).
    Surrender {
        slot: u8,
    },
    /// Command `0x0C`: start the campaign level `level` (path from `campaign.ini`) with the difficulty `difficulty`
    /// (`GameApp+0x19B4`, 0 to 2) - game type -1, setup `nml_client::match_request::campaign_setup`
    /// (re/singleplayer-menus part 1, 6.2).
    StartCampaignLevel {
        level: String,
        difficulty: i32,
    },
    /// The current campaign entry is booked (`FUN_004C6B10(won)`: command `0x1C` with `won = true` or the end of
    /// the game in the campaign, `MenuApp::book_campaign`): report only, writing `profiles.ini` follows as
    /// [`UiEffect::WriteProfiles`].
    BookCampaign {
        won: bool,
    },
    /// A change from the diplomacy dialog `DIPLOMATIE` (`FUN_004D0750`): status, vision and tribute as a command via
    /// the host (in single player the match stands still while the dialog is open, so they take effect immediately;
    /// in the network the dialog halts nothing and the commands go through the turn, re/multiplayer-menus L.7.2,
    /// E-320), chat and tribute mask locally in the host; `Close` is report only.
    Diplomacy(DiplomacyAction),
    /// Send from the chat window `window` 1-3 (`FUN_004B9940`): the host builds the command `0x5D` with the
    /// recipients of the window (1 everyone, 2 the friends of the sender including the sender, 3 the chat mask). Only
    /// with text.
    SendChat {
        window: u8,
        text: String,
    },
    /// Search in the LAN on (`true`, code 5 of `LAN1`: `FUN_004E8500`) or off (`false`, `FUN_004E8510` at `JOIN`,
    /// double-click, `CREATE`, `CANCEL`; re/multiplayer-menus L.1.1). The host (the network session) then queries
    /// every second and delivers the list through `MenuApp::set_net`.
    Search(bool),
    /// A request from the lobby `LAN2` or `CHATFRAME`: the host (the network session) translates it into a request
    /// to the network host or a change by the network host.
    Lobby(LobbyAction),
    /// Application command of the network session (`7`, `8`, `9`, `0x2D`, `0x2F`; re/multiplayer-menus L.9):
    /// connecting, hosting and closing are done by the game host.
    Net(AppCmd),
    /// `0x20`/`0x21`: the menu level behind the menu; the host (`GameFlow`) executes it.
    MenuLevel(MenuLevelEffect),
    /// Command that the reimplementation does not (yet) execute; the host logs it.
    Unhandled(AppCmd),
}

/// Request to the menu level (re/menu-level ML.4, ML.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuLevelEffect {
    /// `0x21` (`FUN_004C9140`): start the menu level or make it visible; `path` as the `.dlg` provides it
    /// (uppercase, `data/levels/menülevel/menü2.lvl`).
    Load { path: String },
    /// `0x20` (`FUN_004BA670`): menu level invisible and stopped (the session stays).
    Hide,
}

/// Request from the lobby: row `row` 0 ... 7 (0 = host, key = row - 1), values like [`crate::draft::SlotDraft`]. The
/// client may only change its own row; the host checks that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobbyAction {
    /// Nation, team, handicap and color of a row.
    Row {
        row: u8,
        nation: i32,
        team: i32,
        handicap: i32,
        color: i32,
    },
    /// Type of a row (`NAMEn`, host only): 0 open (for a human = removal), 1 ... 4 computer.
    Kind { row: u8, kind: i32 },
    /// Ready flag of a row (`READYn`).
    Ready { row: u8, ready: bool },
    /// Send a chat line (`CHATFRAME`).
    Chat(String),
    /// `START` of the host.
    Start,
    /// `CHANGEGAME` of the host in `LAN31`: map and settings of the draft become the new session; the host (the
    /// network session) builds it, the core checks the rows and distributes the state.
    ChangeGame,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_messages_carry_lowercase_names() {
        let msg = ElementMsg::new("SOUNDOPTIONS", "MUSIKÄ0", code::VALUE_CHANGED);
        assert_eq!(msg.dialog, "soundoptions");
        assert_eq!(msg.element, "musikä0");
        assert_eq!(msg.code, 1);
        assert!(msg.params.is_empty());
        let mut none = NoListener;
        let mut msg = msg;
        assert_eq!(none.element(&mut msg, &mut Vec::new()), -1);
        assert_eq!(none.may_trigger(), -1);
    }

    // QUIRK E-315: kept
    #[test]
    fn app_commands_carry_the_original_codes() {
        let name = || "start".to_string();
        let cases = [
            (
                AppCmd::Close {
                    name: name(),
                    wait: true,
                },
                3,
            ),
            (
                AppCmd::Close {
                    name: name(),
                    wait: false,
                },
                4,
            ),
            (
                AppCmd::Open {
                    name: name(),
                    wait: true,
                },
                5,
            ),
            (
                AppCmd::Open {
                    name: name(),
                    wait: false,
                },
                6,
            ),
            (AppCmd::WaitDialogs, 0x15),
            (AppCmd::WaitVideo, 0x1A),
            (AppCmd::StartMatch, 0x0D),
            (AppCmd::WaitMs(1000), 0x30),
            (AppCmd::Video(2), 0x1B),
            (AppCmd::PlayVideo(name()), 0x1D),
            (AppCmd::AfterIntro, 0x24),
            (AppCmd::AfterVideo, 0x28),
            (AppCmd::AfterProbeVideo, 0x28),
            (AppCmd::CloseGameMenu, 0x0F),
            (AppCmd::CloseNetGameMenu, 0x13),
            (AppCmd::ReloadLevel, 0x1F),
            (AppCmd::HideLevelBackground, 0x20),
            (AppCmd::LoadMenuLevel(name()), 0x21),
            (AppCmd::Quit, 0x25),
            (AppCmd::Surrender { slot: 3 }, 0x27),
            (AppCmd::EndMatch, 0x0E),
            (AppCmd::StartCampaignLevel, 0x0C),
            (AppCmd::BookCampaign, 0x1C),
            (AppCmd::DeleteProfile(2), 0x29),
            (AppCmd::ResetKeys, 0x32),
            (AppCmd::CloseAll, 0x34),
            (AppCmd::GameSpy, 0x36),
            (AppCmd::ResumeAfterLoadDialog, 0x37),
            (AppCmd::LoadGame, 0x0A),
            (AppCmd::SubmitSave, 0x26),
            (AppCmd::WriteSave, 0x0B),
            (AppCmd::DeleteSave(name()), 0x2A),
            (AppCmd::Join, 7),
            (AppCmd::Host, 8),
            (AppCmd::CloseNet, 9),
            (AppCmd::LeaveLobby, 0x2D),
            (AppCmd::HostGone, 0x2F),
            (AppCmd::HostSavedGame, 0x12),
            (AppCmd::StartSavedGame, 0x14),
            (AppCmd::PlayReplay, 0x19),
        ];
        for (cmd, code) in cases {
            assert_eq!(cmd.code(), code, "{cmd:?}");
        }
    }
}
