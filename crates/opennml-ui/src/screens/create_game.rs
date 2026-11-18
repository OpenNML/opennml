//! Custom game: `CREATESINGLEPLAYERGAME` (`FUN_004D4B60`), its network branch `LAN3` and the map change `LAN31`
//! (re/multiplayer-menus L.2, L.16), the map selection `LOADMULTIPLAYERMAP` (`FUN_004D7E90`) and the random map
//! `RANDOMMAP` (`FUN_004D7760`, enabled; it used to be locked). See re/singleplayer-menus part 2, 1 to 3.
//!
//! `LAN3` differs from the single-player dialog (re/multiplayer-menus L.2): the frame does not clamp `LevelType` 2 to
//! 1 (only `createsingleplayergame` does that), the buttons are named `CREATE`/`EXIT`, `LEVELTYPE` additionally
//! offers "Load game" (value 2), `CREATE` hosts (`{8}`). `LAN3` does not choose the turn raster: the own selection
//! field `TURNRASTER` found no room in `LAN3` without covering the switches and was dropped; the step comes from
//! `NetTurnRaster` or the command line. `CREATE` with a random map hosts; with a savegame (`{0x12}`) only a log
//! entry, the frame stays.
//!
//! `LAN31`: code 5 of the frame sets the settings and the selected map to the session (the lobby draft,
//! [`load_session`]; deviating from E-281), the map list preselects the map of the session. As in `LAN3` the elements
//! write only into the settings and `Info` - never into the draft, which the host overwrites with every new lobby
//! state. `LEVELTYPE` offers only map and random map (deviating from E-308). `CHANGEGAME` checks the rows (15402) and
//! hands the change to the network session, which closes `LAN31` only on success ([`close_change_game`]);
//! `EXITCHANGE` closes without a change. With a random map `CHANGEGAME` fills `Info` like [`random_slots`], but
//! leaves the rows standing ([`random_info`]).
//!
//! The side frames hang on the index `EngineConfig.LevelType` (field `0x004D4BB4`...`0x004D4BE9`): 0
//! `loadmultiplayermap`, 1 `randommap`, 2 `loadsavednetworkgame`. The handler queues the "query again"
//! ([`super::requery`]); `MenuApp` delivers the list after the message. A requery with code 2 to a ComboBox stays
//! without effect as in the original (`FUN_0045E8A0` takes only code 5 of `0x12E`).
//!
//! Remainder of the former lock of the random map (no effect): `CREATE2` always answers code 5 with status 0
//! (visible, free), and `leveltype`/`gametype` code 4 additionally query it again; the original has no code-5 branch
//! there, the element stays visible and free as in the `.dlg` - so the same. The game mode 2 (Domination) is enabled:
//! map list `NORMAL`, `TIMELIMIT` free, info row 11 (O-134).

use super::lan2::open_lobby;
use super::{MenuData, ScreenEnv, close, open, requery, tell};
use crate::draft::{HUMAN, MapCatalog, SIZE_TEXT, SlotDraft, preview_path, rows_fit, text_of};
use crate::msg::{AppCmd, ElementMsg, LobbyAction, Param, UiEffect, code};
use crate::profiles::{name_field, name_text};
use crate::saves::SaveDir;

/// Status "locked" (`DISABLE`).
const DISABLE: i32 = 0x2000_0000;
/// Side frame per `LevelType` (`0x004D4BB4`...`0x004D4BE9`).
const SIDE_FRAMES: [&str; 3] = ["loadmultiplayermap", "randommap", "loadsavednetworkgame"];
/// Size of `SESSIONNAME` (`size 30`, `0x004D5A11`).
const SESSION_SIZE: i32 = 30;
/// Size of `SEED` (`size 0x1E`, `0x004D77D0`...`0x004D7836`).
const SEED_SIZE: i32 = 30;
/// Highest value of `RandomPlayers` per size (`mapsize` code 4, `0x004D78BF`...`0x004D78F8`): small 2, medium 4,
/// large 8 players.
const MAX_RANDOM_PLAYERS: [i32; 3] = [0, 2, 6];
/// Size of `TIMELIMIT` (`size 4`, `0x004D59B1`...`0x004D5A26`).
const TIME_LIMIT_SIZE: i32 = 4;
/// `LEVELTYPE`: 15520 "Select predefined map", 15521 "Create random map", 15522 "Load game".
const LEVEL_TYPE_TEXTS: [i32; 3] = [15520, 15521, 15522];
/// `UNITLIMIT`: 15531...15534 "75/100/125/150 units" (15530 "50" omitted, O-127).
const UNIT_LIMIT_TEXT: i32 = 15531;
/// `RESOURCEN`: 15550 "take over from map", 15551 "low", 15552 "normal", 15553 "high".
const RESOURCES_TEXT: i32 = 15550;
/// Entries of `UNITLIMIT` and `RESOURCEN` (loops up to 4, `0x004D57C0`, `0x004D58C0`).
const FOUR: i32 = 4;
/// Elements that `leveltype` code 4 queries again with code 5 (`0x004D5B3E`...`0x004D5EDA`; `gametype` twice), plus
/// `create2` (remainder of the former lock, without effect).
const LEVEL_TYPE_REQUERIES: [&str; 9] = [
    "savereplay",
    "lockteams",
    "mapexplored",
    "gametype",
    "unitlimit",
    "resourcen",
    "timelimit",
    "gametype",
    "create2",
];
/// First text ID of the player counts in `RANDOMMAP` (`0x004D7AD0`: `15460 + i`, 1/3/7 entries per size).
const RANDOM_PLAYERS_TEXT: i32 = 15460;
/// Network dialog "Create multiplayer game" (`lan3.dlg`, re/multiplayer-menus L.2).
const NET_DIALOG: &str = "lan3";
/// Network dialog "Change game" (`lan31.dlg`, re/multiplayer-menus L.2).
const NET_CHANGE_DIALOG: &str = "lan31";
/// Text 15402 "Attention! You have selected a map that does not provide enough players. ..." (`LAN31 CHANGEGAME`,
/// `PUSH 0x3C2A`, re/multiplayer-menus L.16 no. 3): message without a follow-up command, `LAN31` stays open.
pub const CHANGE_GAME_REFUSED: i32 = 15402;

/// `FUN_004B98E0(GameType)`: random maps only for `0 <= GameType < 4` (without the railway game modes).
fn random_map_allowed(game_type: i32) -> bool {
    (0..4).contains(&game_type)
}

/// Side frame for `LevelType` (only 0...2).
fn side_frame(level_type: i32) -> Option<&'static str> {
    usize::try_from(level_type)
        .ok()
        .and_then(|i| SIDE_FRAMES.get(i).copied())
}

/// Command 6 `{6, name}` (open without waiting).
fn open_now(name: &str) -> AppCmd {
    AppCmd::Open {
        name: name.to_string(),
        wait: false,
    }
}

fn int(msg: &ElementMsg) -> Option<i32> {
    match msg.params.get("int") {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}

fn put(msg: &mut ElementMsg, name: &str, value: Param) {
    msg.params.insert(name.into(), value);
}

/// List of text IDs (`FUN_0055FFE0` per entry).
fn texts(env: &ScreenEnv, ids: impl IntoIterator<Item = i32>) -> Param {
    Param::Array(ids.into_iter().map(|id| text_of(env.texts, id)).collect())
}

/// Status of the switches and lists (`savereplay`, `lockteams`, `mapexplored`, `gametype`, `unitlimit`, `resourcen`):
/// locked only when loading a network savegame (`LevelType == 2`).
fn status_unless_saved_game(msg: &mut ElementMsg, env: &ScreenEnv) -> i32 {
    let status = if env.config.level_type == 2 {
        DISABLE
    } else {
        0
    };
    put(msg, "status", Param::Int(status));
    1
}

/// Close `LAN31` and open the lobby again (`EXITCHANGE` `0x004D51A7`...`0x004D5202`; after a successful `CHANGEGAME`
/// the network session queues the same): `{3,"lan31"}`, `{4, side frame}` (only `LevelType` 0 ... 2), then the lobby
/// ([`open_lobby`]).
pub fn close_change_game(level_type: i32) -> Vec<AppCmd> {
    let mut cmds = vec![close(NET_CHANGE_DIALOG, true)];
    if let Some(frame) = side_frame(level_type) {
        cmds.push(close(frame, false));
    }
    cmds.extend(open_lobby());
    cmds
}

/// `FUN_004D4B60` for `CREATESINGLEPLAYERGAME`, `LAN3` and `LAN31`.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let dialog = msg.dialog.clone();
    let change = dialog == NET_CHANGE_DIALOG;
    let net = dialog == NET_DIALOG || change;
    match (msg.element.as_str(), msg.code) {
        // `0x004D4E02`...`0x004D4EA1`: `FUN_004BFC40(1)` (savegame directory `savegame`, `SaveDir::Main`, O-150),
        // `LevelType` above 1 -> 1; negative -> return value 0 without side frame; otherwise `{6, side frame}`,
        // return value 1, no `status`.
        //
        // `LAN3` (`0x004D4C10`...`0x004D4C94`): `FUN_004BFC40(2)` (directory `savegame/multiplayer`,
        // `SaveDir::Multiplayer`, O-150), no clamping; `LevelType` 0 ... 2 -> `{6, side frame}`, otherwise return
        // value 0.
        //
        // `LAN31`: first settings and map from the session ([`load_session`]); without "Load game" the
        // reimplementation clamps `LevelType` 2 to 0.
        (own, code::QUERY_STATUS) if own == dialog => {
            // Savegame directory: `LAN3` network folder, `CREATESINGLEPLAYERGAME` main folder; `LAN31` does not
            // change it.
            if net && !change {
                env.state.save_dir = SaveDir::Multiplayer;
            } else if !net {
                env.state.save_dir = SaveDir::Main;
            }
            if change {
                load_session(env);
                if env.config.level_type > 1 {
                    env.config.level_type = 0;
                }
            }
            if !net && env.config.level_type > 1 {
                env.config.level_type = 1;
            }
            match side_frame(env.config.level_type) {
                Some(frame) => {
                    env.queue.push(open_now(frame));
                    1
                }
                None => 0,
            }
        }
        // `0x004D4EA6`...`0x004D4EE6`: pointer to `NetSession`, `size 30`, `inputbuffer 1` (O-132).
        ("sessionname", code::QUERY_TEXT) => {
            let units = crate::profiles::name_units(&env.config.net_session).to_vec();
            env.state.net_session.set_units(units);
            put(msg, "text", Param::Shared(env.state.net_session.clone()));
            put(msg, "size", Param::Int(SESSION_SIZE));
            put(msg, "inputbuffer", Param::Int(1));
            1
        }
        ("leveltype", code::QUERY_VALUE) => {
            put(msg, "int", Param::Int(env.config.level_type));
            1
        }
        ("leveltype", code::QUERY_ITEMS) => level_type_items(msg, env),
        ("leveltype", code::ITEM_SELECTED) => {
            let Some(new) = int(msg) else {
                return 0;
            };
            // `0x004D5B3E`...`0x004D5EDA`: on a change `{3, frame[old]}`, `LevelType = new`, `{5, frame[new]}`.
            let old = env.config.level_type;
            if new != old {
                if let Some(frame) = side_frame(old) {
                    env.queue.push(close(frame, true));
                }
                env.config.level_type = new;
                if let Some(frame) = side_frame(new) {
                    env.queue.push(open(frame));
                }
            }
            for element in LEVEL_TYPE_REQUERIES {
                // `LAN3 CREATE` has no code-5 branch (remainder of the former lock only in single player).
                if !(net && element == "create2") {
                    requery(env, &dialog, element, code::QUERY_STATUS);
                }
            }
            1
        }
        // `0x004D5320`...`0x004D54E5`: switches onto `SaveReplay`, `LockTeams`, `MapExplored`.
        ("savereplay" | "lockteams" | "mapexplored", c) => {
            let field = match msg.element.as_str() {
                "savereplay" => &mut env.config.save_replay,
                "lockteams" => &mut env.config.lock_teams,
                _ => &mut env.config.map_explored,
            };
            match c {
                code::QUERY_VALUE => {
                    let v = *field;
                    put(msg, "int", Param::Int(v));
                    1
                }
                code::VALUE_CHANGED => {
                    if let Some(v) = int(msg) {
                        *field = v;
                    }
                    1
                }
                code::QUERY_STATUS => status_unless_saved_game(msg, env),
                _ => -1,
            }
        }
        ("gametype", code::QUERY_VALUE) => {
            put(msg, "int", Param::Int(env.config.game_type));
            1
        }
        // `0x004D5514`, `FUN_004B98D0`: the `GNAME` texts of `randommaptypes.ini`, without `indexarray`.
        ("gametype", code::QUERY_ITEMS) => {
            let ids: Vec<i32> = env
                .data
                .maps
                .types
                .game_types
                .iter()
                .map(|g| g.name_id)
                .collect();
            let items = texts(env, ids);
            put(msg, "array", items);
            1
        }
        ("gametype", code::ITEM_SELECTED) => game_type_selected(msg, env, &dialog),
        ("gametype", code::QUERY_STATUS) => status_unless_saved_game(msg, env),
        // `0x004D576D`...`0x004D5945`: `UnitLimit` with 15531...15534 (O-127), `Resourcen` with 15550...15553.
        ("unitlimit" | "resourcen", c) => {
            let unit = msg.element == "unitlimit";
            match c {
                code::QUERY_VALUE => {
                    let v = if unit {
                        env.config.unit_limit
                    } else {
                        env.config.resourcen
                    };
                    put(msg, "int", Param::Int(v));
                    1
                }
                code::QUERY_ITEMS => {
                    let first = if unit {
                        UNIT_LIMIT_TEXT
                    } else {
                        RESOURCES_TEXT
                    };
                    let items = texts(env, (0..FOUR).map(|i| first + i));
                    put(msg, "array", items);
                    1
                }
                code::ITEM_SELECTED => {
                    if let Some(v) = int(msg) {
                        if unit {
                            env.config.unit_limit = v;
                        } else {
                            env.config.resourcen = v;
                        }
                    }
                    1
                }
                code::QUERY_STATUS => status_unless_saved_game(msg, env),
                _ => -1,
            }
        }
        ("timelimit", c) => time_limit(msg, env, c),
        ("create2", code::TRIGGERED) if !net => create2(env, &dialog),
        // Remainder of the former lock (without effect): always visible and free, also with a random map.
        ("create2", code::QUERY_STATUS) if !net => {
            put(msg, "status", Param::Int(0));
            1
        }
        // `0x004D529E`...`0x004D5316`: `{3, dialog}`, `{4, side frame}`, `{5,"singleplayermenu"}`; `LAN3 EXIT`
        // (`0x004D521D`...`0x004D5295`) likewise with `{5,"lan1"}`.
        ("exit2", code::TRIGGERED) if !net => exit(env, &dialog, "singleplayermenu"),
        ("exit", code::TRIGGERED) if net && !change => exit(env, &dialog, "lan1"),
        ("create", code::TRIGGERED) if net && !change => create_net(env, &dialog),
        ("changegame", code::TRIGGERED) if change => change_game(env),
        // `0x004D51A7`...`0x004D5202`: frame closed, lobby open - nothing else (the session stays).
        ("exitchange", code::TRIGGERED) if change => {
            for cmd in close_change_game(env.config.level_type) {
                env.queue.push(cmd);
            }
            1
        }
        _ => -1,
    }
}

/// `{3, dialog}`, `{4, side frame}` (only `LevelType` 0 ... 2), `{5, back}`.
fn exit(env: &mut ScreenEnv, dialog: &str, back: &str) -> i32 {
    env.queue.push(close(dialog, true));
    if let Some(frame) = side_frame(env.config.level_type) {
        env.queue.push(close(frame, false));
    }
    env.queue.push(open(back));
    1
}

/// `LAN3 CREATE` code 8 (`0x004D4F05`...`0x004D4FBC`, re/multiplayer-menus L.2): `{3, dialog}`, `{4, side frame}`,
/// `FUN_004BF1F0(0)` (slots from the map, own slot with `NetName`), then `{8}` (hosting). The original calls
/// `FUN_004B9700` (settings into the session) only in command `8`; the reimplementation calls it here, so that
/// `draft.info` and `draft.settings` are ready for the session data. Random map (`LevelType` 1): the slots via
/// [`random_slots`] (empty seed -> new text for every `CREATE`, E-313), otherwise the same. Savegame (`LevelType` 2,
/// re/multiplayer-saves S.2.1): `{3, dialog}`, `{4,"loadsavednetworkgame"}`, `{0x12}` - without slots and settings
/// from the menu (they come from the savegame, `FUN_004C80D0`). Other values: only a log entry.
fn create_net(env: &mut ScreenEnv, dialog: &str) -> i32 {
    let level_type = env.config.level_type;
    if !(0..=2).contains(&level_type) {
        log::info!("LAN3 CREATE with LevelType {level_type}: no effect");
        return 1;
    }
    env.queue.push(close(dialog, true));
    if let Some(frame) = side_frame(level_type) {
        env.queue.push(close(frame, false));
    }
    if level_type == 2 {
        env.queue.push(AppCmd::HostSavedGame);
        return 1;
    }
    build_slots(env);
    copy_settings(env);
    env.queue.push(AppCmd::Host);
    1
}

/// `leveltype` code 2 (`0x004D5F0C`...`0x004D6067`): 15520 (value 0); random map (value 1) only for game modes 0...3,
/// otherwise `LevelType` 1 -> 0 (only the variable, no frame change); 15522 (value 2) in the original outside of
/// `createsingleplayergame` (network), in the reimplementation only in `LAN3` (deviating from E-308: not in `LAN31`),
/// otherwise `LevelType` 2 -> 0.
fn level_type_items(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let mut values = vec![0];
    if random_map_allowed(env.config.game_type) {
        values.push(1);
    } else if env.config.level_type == 1 {
        env.config.level_type = 0;
    }
    if msg.dialog == NET_DIALOG {
        values.push(2);
    } else if env.config.level_type == 2 {
        env.config.level_type = 0;
    }
    let items = texts(
        env,
        values
            .iter()
            .map(|&v| LEVEL_TYPE_TEXTS[usize::try_from(v).unwrap_or(0)]),
    );
    put(msg, "array", items);
    put(msg, "indexarray", Param::IndexArray(values));
    1
}

/// `gametype` code 4 (`0x004D5579`...`0x004D5768`): `GameType = int`; `leveltype` code 2 again (without effect on a
/// ComboBox, `FUN_0045E8A0`); `LevelType == 0` -> `{3,"loadmultiplayermap"}`, `{5,"loadmultiplayermap"}` (list
/// refiltered, entry 0 selected, O-128); `LevelType == 1` without random map -> `LevelType = 0`, `{3,"randommap"}`,
/// `{5,"loadmultiplayermap"}`, `leveltype` code 5 again; then `timelimit` code 5 again (and `create2`,
/// reimplementation).
///
/// Correction O-129: the requery of `leveltype` code 2 does not reach the handler, `LevelType` stays 1 - so the
/// branch "close random map" takes effect, `RANDOMMAP` does not lie under `LOADMULTIPLAYERMAP`.
fn game_type_selected(msg: &ElementMsg, env: &mut ScreenEnv, dialog: &str) -> i32 {
    let Some(game_type) = int(msg) else {
        return 0;
    };
    env.config.game_type = game_type;
    requery(env, dialog, "leveltype", code::QUERY_ITEMS);
    match env.config.level_type {
        0 => {
            env.queue.push(close(SIDE_FRAMES[0], true));
            env.queue.push(open(SIDE_FRAMES[0]));
        }
        1 if !random_map_allowed(game_type) => {
            env.config.level_type = 0;
            env.queue.push(close(SIDE_FRAMES[1], true));
            env.queue.push(open(SIDE_FRAMES[0]));
            requery(env, dialog, "leveltype", code::QUERY_STATUS);
        }
        _ => {}
    }
    requery(env, dialog, "timelimit", code::QUERY_STATUS);
    if dialog != NET_DIALOG && dialog != NET_CHANGE_DIALOG {
        requery(env, dialog, "create2", code::QUERY_STATUS);
    }
    1
}

/// `timelimit` (`0x004D5977`...`0x004D5AE2`).
fn time_limit(msg: &mut ElementMsg, env: &mut ScreenEnv, c: u32) -> i32 {
    match c {
        // Locked for the savegame, for deathmatch/heroes (`GameType < 2`) and for the race (5).
        code::QUERY_STATUS => {
            let gt = env.config.game_type;
            let locked = env.config.level_type == 2 || gt < 2 || gt == 5;
            put(msg, "status", Param::Int(if locked { DISABLE } else { 0 }));
            1
        }
        // Clamp `TimeLimit` to 1...999 (written back), `swprintf(buffer, "%d")`, pointer, `size 4`.
        code::QUERY_TEXT => {
            let limit = env.config.time_limit.clamp(1, 999);
            env.config.time_limit = limit;
            env.state.time_limit_text.set(&limit.to_string());
            put(
                msg,
                "text",
                Param::Shared(env.state.time_limit_text.clone()),
            );
            put(msg, "size", Param::Int(TIME_LIMIT_SIZE));
            put(msg, "inputbuffer", Param::Int(1));
            1
        }
        // Without `key` -1; `'0'...'9'` -> -1 (insert); any other character -> 1, also ESC (O-131).
        code::EDIT_KEY => match msg.params.get("key") {
            Some(Param::Int(key)) if (0x30..0x3A).contains(key) => -1,
            Some(Param::Int(_)) => 1,
            _ => -1,
        },
        // `swscanf(text, "%d", &TimeLimit)` without clamping upward (O-130); afterwards `<= 0` -> 1; return value -1.
        code::EDIT_CHANGED => {
            let text = match msg.params.get("text") {
                Some(Param::Text(t)) => Some(t.clone()),
                Some(Param::Shared(t)) => Some(t.get()),
                _ => None,
            };
            if let Some(text) = text {
                if let Some(v) = scan_int(&text) {
                    env.config.time_limit = v;
                }
                if env.config.time_limit < 1 {
                    env.config.time_limit = 1;
                }
            }
            -1
        }
        _ => -1,
    }
}

/// `swscanf(..., L"%d")`: whitespace, sign, at least one digit (32 bits, wrapping); otherwise `None`.
fn scan_int(text: &str) -> Option<i32> {
    let rest = text.trim_start_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let (negative, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits: Vec<i32> = rest
        .chars()
        .map_while(|c| c.to_digit(10))
        .map(|d| d as i32)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let v = digits
        .iter()
        .fold(0i32, |v, &d| v.wrapping_mul(10).wrapping_add(d));
    Some(if negative { v.wrapping_neg() } else { v })
}

/// `create2` code 8 (`0x004D4FC5`...`0x004D50A5`): `{3, dialog}`, `{4, side frame}`, `FUN_004BF1F0(0)` (slots from
/// the map or the random map), then `FUN_004BFC40(1)` (savegame directory; no counterpart, O-150) and `FUN_004B9700`
/// (settings into the session), `{5,"singleplayerstart"}`, `{5,"levelinfo"}`. For `LevelType == 2` the original
/// queues `{0x0A}` (load savegame) - unreachable in single player (code 5 of the frame and `leveltype` code 2 set 2
/// to 1 or 0).
fn create2(env: &mut ScreenEnv, dialog: &str) -> i32 {
    env.queue.push(close(dialog, true));
    if let Some(frame) = side_frame(env.config.level_type) {
        env.queue.push(close(frame, false));
    }
    build_slots(env);
    if env.config.level_type == 2 {
        log::warn!(
            "CREATE2 with LevelType 2 (network savegame): not provided for in single player"
        );
        return 1;
    }
    copy_settings(env);
    env.queue.push(open("singleplayerstart"));
    env.queue.push(open("levelinfo"));
    1
}

/// `FUN_004BF1F0(0)`: delete all slots, `Info.LevelType = LevelType`; for `LevelType 0` read the map with slots
/// (`FUN_004BDDA0(level path, Info, 1)`, from the catalog: name "EMPTY", type 0 - `PLAYER_TYPE` discarded, O-135);
/// for `LevelType 1` the random map ([`random_slots`]); `LevelType 2` -> return at once. At the end the own slot (key
/// -1): name `NetName`, type 999, handicap `EngineConfig.Handicap`; nation, color and team stay those of the map. If
/// the slot is missing (map without `[PLAYER]`), the original appends a record from an uninitialized buffer; the
/// reimplementation appends it with nation, team and color 0.
fn build_slots(env: &mut ScreenEnv) {
    let data = &mut *env.data;
    data.draft.slots.clear();
    let level_type = env.config.level_type;
    data.level.info.level_type = level_type;
    match level_type {
        0 => read_fixed_map(data, true),
        1 => random_slots(env),
        2 => return,
        // Other values read no map.
        _ => {}
    }
    let data = &mut *env.data;
    let name = name_text(&env.config.net_name);
    let handicap = env.config.handicap;
    match data.draft.slots.iter_mut().find(|s| s.key == -1) {
        Some(own) => {
            own.name = name;
            own.kind = HUMAN;
            own.handicap = handicap;
        }
        None => data.draft.slots.push(SlotDraft {
            key: -1,
            name,
            kind: HUMAN,
            handicap,
            ..SlotDraft::default()
        }),
    }
}

/// `FUN_004BDDA0(level path, Info, slots)` from the catalog: read the map again - path, with `.lvl` the player count
/// and (only with `slots`) the slots (created per block before the `.wld` is opened), with `.wld` the size.
fn read_fixed_map(data: &mut MenuData, slots: bool) {
    if let Some(m) = data.level.map.and_then(|i| data.maps.maps.get(i)) {
        data.level.info.path = m.entry.path.clone();
        if m.level_read {
            data.level.info.players = m.players();
            if slots {
                data.draft.slots = m.slots.clone();
            }
        }
        if m.world_read {
            data.level.info.size_class = m.size_class;
        }
    }
}

/// `LAN31 CHANGEGAME` code 8 (`0x004D50C4`...`0x004D518C`, re/multiplayer-menus L.2, L.16 no. 3): `FUN_004BF1F0(1)` -
/// `Info` anew from the selected map, the slots (= lobby rows) stay -, then the slot check: if a row that falls away
/// is occupied ([`rows_fit`]; the original compares the number of all slots, deviating from E-307), message 15402
/// without a follow-up command, the frame stays, nothing changes. Otherwise `FUN_004B9700` (map, `Info` and settings
/// into the draft, [`copy_settings`]) and the change as [`LobbyAction::ChangeGame`] to the network session: it
/// distributes the session (instead of broadcast `0x10000014`) and closes `LAN31` only on success
/// ([`close_change_game`]); if the core rejects it (a join in between), it shows 15402 and `LAN31` stays. Random map
/// (`LevelType 1`): `Info` like [`random_slots`] ([`random_info`]: empty seed -> new text for every `CHANGEGAME`,
/// E-313; players `RandomPlayers + 2`), the rows stay; the check follows after the draw as in the original. Other
/// values (not selectable in `LAN31`): only a log entry.
fn change_game(env: &mut ScreenEnv) -> i32 {
    let level_type = env.config.level_type;
    if !(0..=1).contains(&level_type) {
        log::info!("LAN31 CHANGEGAME with LevelType {level_type}: no effect");
        return 1;
    }
    env.data.level.info.level_type = level_type;
    if level_type == 0 {
        read_fixed_map(env.data, false);
    } else {
        random_info(env);
    }
    let data = &*env.data;
    if !rows_fit(&data.draft.slots, data.level.info.players) {
        tell(env.state, env.queue, CHANGE_GAME_REFUSED, None);
        return 1;
    }
    copy_settings(env);
    env.effects.push(UiEffect::Lobby(LobbyAction::ChangeGame));
    1
}

/// `LAN31` opens on the session (deviating from E-281 - the original leaves the settings standing and sets `Info` to
/// the first entry via the map list): map kind, game mode (list index for the game type,
/// [`MapCatalog::game_type_of_kind`]; without a match it stays), unit limit, resources, time limit, the three
/// switches and `NetSession` from the draft (= session, from the lobby state of the host); for a random map size,
/// `RandomPlayers` = players - 2, map type, landscape and time of day (`RandomMapSeed` stays, E-313). The selected
/// map (`Info`, catalog index) becomes that of the session.
fn load_session(env: &mut ScreenEnv) {
    let data = &mut *env.data;
    let config = &mut *env.config;
    let info = &data.draft.info;
    let s = &data.draft.settings;
    let random = info.level_type == 1;
    config.level_type = i32::from(random);
    if let Some(game_type) = data.maps.game_type_of_kind(s.game_type) {
        config.game_type = game_type;
    }
    config.unit_limit = s.unit_limit;
    config.resourcen = s.resources;
    config.time_limit = s.time_limit;
    config.lock_teams = i32::from(s.lock_teams);
    config.map_explored = i32::from(s.map_explored);
    config.save_replay = i32::from(s.save_replay);
    config.net_session = name_field(&s.session);
    if random {
        config.random_map_size = info.size_class;
        config.random_players = info.players.wrapping_sub(2);
        config.random_terrain = info.random_terrain;
        config.random_landscape = info.random_landscape;
        config.random_daytime = info.random_daytime;
    } else {
        data.level.map = data
            .maps
            .maps
            .iter()
            .position(|m| m.entry.path.eq_ignore_ascii_case(&info.path));
    }
    data.level.info = info.clone();
}

/// Preselection of the map list: in `LAN31` (there is a lobby) the row of the session map in the list filtered by
/// `game_type`; if it is missing there (another game mode) or in `LAN3`, entry 0 (O-128).
fn preselected_row(data: &MenuData, game_type: i32) -> i32 {
    let in_lobby = data.net.as_ref().is_some_and(|n| n.lobby.is_some());
    if !in_lobby || data.draft.info.level_type != 0 {
        return 0;
    }
    let path = &data.draft.info.path;
    data.maps
        .filtered(game_type)
        .into_iter()
        .position(|i| data.maps.maps[i].entry.path.eq_ignore_ascii_case(path))
        .and_then(|row| i32::try_from(row).ok())
        .unwrap_or(0)
}

/// `FUN_004BF1F0` for `LevelType 1` (`0x004BF241`...`0x004BF33C`): `Info.Name` = `RandomMapSeed`; empty (`wcslen ==
/// 0`, a seed of spaces counts as set) -> `swprintf(L"%d", rand())` from the menu state ([`crate::draft::SeedRand`],
/// O-140), **only** in `Info`, `SEED` stays empty. `Info.Size` = `RandomMapSize`, players = `RandomPlayers + 2`, map
/// type, landscape, time of day; `Info.Path` stays (the path of the last selected fixed map). For each slot `i = 0
/// ... n-1` one record with name `""`, type 0, key `i - 1`, nation 0, team `i`, handicap 0, color `i`
/// (`FUN_004E9750`); the own one (key -1, team and color 0) is set by the caller.
fn random_slots(env: &mut ScreenEnv) {
    random_info(env);
    let data = &mut *env.data;
    for i in 0..data.level.info.players {
        data.draft.set_slot(SlotDraft {
            key: i - 1,
            name: String::new(),
            kind: 0,
            nation: 0,
            team: i,
            handicap: 0,
            color: i,
        });
    }
}

/// The `Info` part of [`random_slots`] (seed or drawn text, size, players `RandomPlayers + 2`, map type, landscape,
/// time of day) without the slots: so also `LAN31 CHANGEGAME` (`FUN_004BF1F0(1)`), whose slots are the lobby rows.
fn random_info(env: &mut ScreenEnv) {
    let config = &*env.config;
    let data = &mut *env.data;
    let info = &mut data.level.info;
    let seed = name_text(&config.random_map_seed);
    info.name = if seed.is_empty() {
        data.seed_rand.next().to_string()
    } else {
        seed
    };
    info.size_class = config.random_map_size;
    info.random_daytime = config.random_daytime;
    info.players = config.random_players.wrapping_add(2);
    info.random_landscape = config.random_landscape;
    info.random_terrain = config.random_terrain;
}

/// `FUN_004B9700`: `NS+0x78 <- Info`, player count, `NetSession`, `LockTeams`, `MapExplored`, `SaveReplay`, game type
/// (`FUN_004CBB10(GameType)`), `UnitLimit`, `Resourcen`, `TimeLimit`. `GameSpeed`, `CheckSumEnabled` and the version
/// number 909 are also in the session of the original; the reimplementation reads them from the settings at start.
fn copy_settings(env: &mut ScreenEnv) {
    let config = &*env.config;
    let data = &mut *env.data;
    data.draft.map = data.level.map;
    data.draft.info = data.level.info.clone();
    let s = &mut data.draft.settings;
    s.session = name_text(&config.net_session);
    s.lock_teams = config.lock_teams != 0;
    s.map_explored = config.map_explored != 0;
    s.save_replay = config.save_replay != 0;
    s.game_type = data.maps.game_kind(config.game_type);
    s.unit_limit = config.unit_limit;
    s.resources = config.resourcen;
    s.time_limit = config.time_limit;
}

/// `FUN_004D7E90` for `LOADMULTIPLAYERMAP`.
pub(super) fn handle_map(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let dialog = msg.dialog.clone();
    let game_type = env.config.game_type;
    match (msg.element.as_str(), msg.code) {
        // `FUN_004BDD40`: names of the maps with the `TYPE` of the game mode, without `indexarray`.
        ("levels", code::QUERY_ITEMS) => {
            let maps: &MapCatalog = &env.data.maps;
            let names = maps
                .filtered(game_type)
                .into_iter()
                .map(|i| maps.maps[i].name.clone())
                .collect();
            put(msg, "array", Param::Array(names));
            1
        }
        // Always entry 0 (O-128), then `FUN_004BFAE0(0)`; if that fails, return value 0. In `LAN31` the session map
        // ([`preselected_row`]).
        ("levels", code::QUERY_VALUE) => {
            let row = preselected_row(env.data, game_type);
            put(msg, "int", Param::Int(row));
            let data = &mut *env.data;
            i32::from(data.level.select(&data.maps, game_type, row, env.texts))
        }
        // `FUN_004BFAE0(int)`; if it succeeds, `mappreview` code 5 again.
        ("levels", code::ITEM_SELECTED) => {
            let Some(i) = int(msg) else {
                return 0;
            };
            let data = &mut *env.data;
            if !data.level.select(&data.maps, game_type, i, env.texts) {
                return 0;
            }
            requery(env, &dialog, "mappreview", code::QUERY_STATUS);
            1
        }
        // `FUN_004C6FB0`: `.lvl` path from `Info+0x50` without the last three characters + `tga`; no `status`.
        ("mappreview", code::QUERY_STATUS) => {
            let tga = preview_path(&env.data.level.info.path);
            put(msg, "tga", Param::Text(tga));
            1
        }
        _ => -1,
    }
}

/// `FUN_004D7760` for `RANDOMMAP` (enabled; the earlier lock is lifted). No element has a code-5 or code-`0xC` branch
/// (-1): `SEED` writes `RandomMapSeed` via the shared buffer ([`super::ScreenState::random_map_seed`], mirror before
/// the handler), the ComboBoxes via code 4. `mapsize` clamps `RandomPlayers` and queries `players` again with code 5;
/// `players`, `terrain`, `landscape` and `daytime` write without a check. `cancel` (network remainder
/// `{4,"randommap"}`, `{6,"lan3"}`) stays unanswered. `opennml-client` starts the game
/// (`Game_StartRandomMapFromSeedText`) from `Info` ([`build_slots`]).
pub(super) fn handle_random(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let element = msg.element.clone();
    let element = element.as_str();
    if !matches!(
        element,
        "seed" | "mapsize" | "players" | "terrain" | "landscape" | "daytime"
    ) {
        return -1;
    }
    if msg.code == code::ITEM_SELECTED && element != "seed" {
        let dialog = msg.dialog.clone();
        let Some(v) = int(msg) else {
            return 0;
        };
        return random_item_selected(env, &dialog, element, v);
    }
    let config = &*env.config;
    match msg.code {
        // `0x004D77D0`...`0x004D7836`: pointer to `RandomMapSeed`, `size 0x1E`, `inputbuffer 1`.
        code::QUERY_TEXT if element == "seed" => {
            let units = crate::profiles::name_units(&config.random_map_seed).to_vec();
            env.state.random_map_seed.set_units(units);
            put(
                msg,
                "text",
                Param::Shared(env.state.random_map_seed.clone()),
            );
            put(msg, "size", Param::Int(SEED_SIZE));
            put(msg, "inputbuffer", Param::Int(1));
            1
        }
        code::QUERY_VALUE if element != "seed" => {
            let v = match element {
                "mapsize" => config.random_map_size,
                "players" => config.random_players,
                "terrain" => config.random_terrain,
                "landscape" => config.random_landscape,
                _ => config.random_daytime,
            };
            put(msg, "int", Param::Int(v));
            1
        }
        code::QUERY_ITEMS if element != "seed" => {
            let types = &env.data.maps.types;
            let ids: Vec<i32> = match element {
                // 15450...15452 (`0x004D7990`).
                "mapsize" => (0..3).map(|i| SIZE_TEXT + i).collect(),
                // 1/3/7 entries per size from 15460 (`0x004D7A49`...`0x004D7AF0`).
                "players" => {
                    let n = match config.random_map_size {
                        0 => 1,
                        1 => 3,
                        2 => 7,
                        _ => 0,
                    };
                    (0..n).map(|i| RANDOM_PLAYERS_TEXT + i).collect()
                }
                // `FUN_004B98A0`/`B0`/`C0`: `MNAME`, `LNAME`, `DNAME`.
                "terrain" => types.maps.clone(),
                "landscape" => types.landscapes.clone(),
                _ => types.daytimes.clone(),
            };
            let items = texts(env, ids);
            put(msg, "array", items);
            1
        }
        _ => -1,
    }
}

/// Code 4 of the ComboBoxes of `RANDOMMAP`: `mapsize` (`0x004D78B3`...`0x004D7950`) sets `RandomMapSize`, clamps
/// `RandomPlayers` to at most {0, 2, 6}[size] (never raising it; a size outside 0...2 clamps against a stack value in
/// the original, the reimplementation not at all - reachable only with a changed `Engine.ini`) and sends code 5 to
/// `players` (the ComboBox queries list and value again); `players` (`0x004D7A39`), `terrain` (`0x004D7B72`),
/// `landscape` (`0x004D7C31`) and `daytime` (`0x004D7CF8`) write their field without a check.
fn random_item_selected(env: &mut ScreenEnv, dialog: &str, element: &str, v: i32) -> i32 {
    let config = &mut *env.config;
    match element {
        "mapsize" => {
            config.random_map_size = v;
            if let Some(&max) = usize::try_from(v)
                .ok()
                .and_then(|i| MAX_RANDOM_PLAYERS.get(i))
            {
                config.random_players = config.random_players.min(max);
            }
            requery(env, dialog, "players", code::QUERY_STATUS);
        }
        "players" => config.random_players = v,
        "terrain" => config.random_terrain = v,
        "landscape" => config.random_landscape = v,
        _ => config.random_daytime = v,
    }
    1
}
