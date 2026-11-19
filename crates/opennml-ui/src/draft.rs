//! Match draft of the single-player menu as own data (without `opennml-sim`; the conversion into the setup of the
//! simulation is done by `opennml-client`). It holds the map description [`LevelInfoData`], which the savegame container
//! (`opennml_client::game_save::HostPart`) carries for the loading screen, as well as the map catalog ([`MapCatalog`]),
//! the selected map ([`SelectedLevel`]), the slots ([`SlotDraft`]) and the settings of the match
//! ([`DraftSettings`]) in the draft ([`MatchDraft`]).
//!
//! Basis: re/singleplayer-menus part 2, 1.1, 1.2, 2 and 4.3.

use opennml_formats::mp_levels::{
    MapListEntry, RandomMapTypes, load_map_list, parse_random_map_types,
};
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;
use serde::{Deserialize, Serialize};

use crate::assets::Texts;
use crate::own_texts;
use crate::text::CurLevelText;

/// `randommaptypes.ini` (`FUN_004CC0D0`).
const RANDOM_MAP_TYPES_PATH: &str = "data/gifcon/randommaptypes.ini";
/// Name of the map slots from the `.lvl` (`L"EMPTY"`, `0x0061ECE8`).
pub const EMPTY_SLOT_NAME: &str = "EMPTY";
/// Type "Human" of the own slot (`FUN_004BF1F0`, `0x004BF37E`-`0x004BF3C8`).
pub const HUMAN: i32 = 999;
/// First text ID of the size classes "Small/Medium/Large" (`FUN_004BFAE0`: `15450 + class`, `0x3C5A`).
pub const SIZE_TEXT: i32 = 15450;
/// First text ID of "n players" (`FUN_004BFAE0`: `15458 + player count`, `0x3C62`).
pub const PLAYERS_TEXT: i32 = 15458;
/// First text ID of the game modes (`FUN_004CBB10`: game type = `GNAME` text - 15540, `0x3CB4`).
pub const GAME_TYPE_TEXT: i32 = 15540;

/// Description of the selected map (`LevelInfo` of the session, `GameApp+0xD650`): name, `.lvl` path,
/// size class (0/1/2 from `TERRAIN_DIM_X` 64/128/256), number of slots and `LevelType`. For a random map
/// (`LevelType 1`, `FUN_004BF1F0` `0x004BF241`-`0x004BF2D2`) `name` is the seed text, `size_class` the size,
/// `players` = `RandomPlayers + 2`, plus map type, landscape and time of day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelInfoData {
    pub name: String,
    pub path: String,
    pub size_class: i32,
    pub players: i32,
    pub level_type: i32,
    /// `Info+0x1620`: `RandomTerrain` (index in `MSCRIPT`/`MNAME`); 0 for a fixed map.
    pub random_terrain: i32,
    /// `Info+0x1624`: `RandomLandscape` (index in `LTYPE`/`LNAME`); 0 for a fixed map.
    pub random_landscape: i32,
    /// `Info+0x1628`: `RandomDaytime` (index in `DTYPE`/`DNAME`); 0 for a fixed map.
    pub random_daytime: i32,
}

/// Own `rand()` state of the menu for the empty seed (O-140): MSVC sequence
/// (`state = state * 214013 + 2531011`, result `(state >> 16) & 0x7FFF`), start 1 at program start, not
/// saved. The original draws from the global `rand()` (`FUN_004BF1F0` `0x004BF261`), whose state depends on everything
/// that has drawn since program start; the reimplementation keeps the menu draw separate from it (a deviation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedRand(pub u32);

impl Default for SeedRand {
    /// Start value 1 (`srand` never called).
    fn default() -> Self {
        SeedRand(1)
    }
}

impl SeedRand {
    /// Next value 0 ... 32767 (`rand`).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7FFF) as i32
    }
}

/// A player slot (node data of the list `NS+0x724`, 0x6C bytes; re/singleplayer-menus part 2, 4.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotDraft {
    /// `+0x54`: -1 = own slot, otherwise block number - 1.
    pub key: i32,
    /// `+0x00`: Name (`wchar_t[40]`).
    pub name: String,
    /// `+0x50`: 0 open, 1-4 computer easy/normal/hard/manual, 999 human.
    pub kind: i32,
    /// `+0x58`: nation 0-5 (index of `PLAYER_PLAYER` in the `NTYPE` list, not found -1).
    pub nation: i32,
    /// `+0x5C`: team (from the map: block number).
    pub team: i32,
    /// `+0x60`: handicap 0-5.
    pub handicap: i32,
    /// `+0x64`: color 0-8 (`PLAYER_COLOR`).
    pub color: i32,
}

/// A map of the catalog: entry of the map list with display name, size class, slots and preview image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCatalogEntry {
    pub entry: MapListEntry,
    /// `NAME`-Text (`FUN_004E5840`).
    pub name: String,
    /// `TERRAIN_DIM_X` of the `.wld` 64/128/256 -> 0/1/2, otherwise 0 ([`size_class`]).
    pub size_class: i32,
    /// One slot per `[PLAYER]` block of the `.lvl` (`FUN_004BDDA0` with slots): name "EMPTY", type 0, key =
    /// block - 1, team = block, nation from `PLAYER_PLAYER`, color `PLAYER_COLOR`, handicap 0.
    pub slots: Vec<SlotDraft>,
    /// `.lvl` path without the last three characters + `tga` ([`preview_path`]).
    pub preview: String,
    /// The `.lvl` was present (`GameFile_Open` in `FUN_004BDDA0`); without it the player count and slots stay
    /// unchanged.
    pub level_read: bool,
    /// The `.wld` was present; without it reading fails **after** the player count and slots, the size stays
    /// unchanged ([`SelectedLevel::select`], `create2`).
    pub world_read: bool,
}

impl MapCatalogEntry {
    /// Player count (`Info+0x15C`, number of `[PLAYER]` blocks).
    pub fn players(&self) -> i32 {
        i32::try_from(self.slots.len()).unwrap_or(i32::MAX)
    }
}

/// Map catalog (a deviation from the original): the multiplayer maps (`levels%04d.ini`) with slots, size and
/// preview, read once at startup (the original reads the `.lvl` and `.wld` at every choice, `FUN_004BFAE0`),
/// and `randommaptypes.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapCatalog {
    pub maps: Vec<MapCatalogEntry>,
    pub types: RandomMapTypes,
}

impl MapCatalog {
    /// Reads the map list and `randommaptypes.ini` from the VFS. Missing files yield empty lists with a warning.
    ///
    /// The reimplementation reads the `.lvl` line by line like `FUN_004BDDA0` ([`read_slots`]), and of the `.wld` only
    /// the header up to `TERRAIN_DIM_X` ([`size_class`]).
    pub fn load(vfs: &Vfs, texts: &Texts) -> MapCatalog {
        let types = match vfs.read(RANDOM_MAP_TYPES_PATH) {
            Ok(bytes) => parse_random_map_types(&decode_cp1252(&bytes)),
            Err(e) => {
                log::warn!("\"{RANDOM_MAP_TYPES_PATH}\" not readable: {e}; no game modes");
                RandomMapTypes::default()
            }
        };
        let maps = load_map_list(|path| vfs.read(path).ok())
            .into_iter()
            .map(|entry| catalog_entry(vfs, texts, &types, entry))
            .collect();
        MapCatalog { maps, types }
    }

    /// `FUN_004BDC50`/`FUN_004E5880`: the maps whose `TYPE` equals exactly the `GTYPE` word of the game mode
    /// `game_type` (`_mbscmp`), in list order (indices into [`Self::maps`]). An invalid game mode has the empty word
    /// (`FUN_004CC9D0`) and matches only maps without `TYPE`.
    pub fn filtered(&self, game_type: i32) -> Vec<usize> {
        let gtype = self.gtype(game_type);
        self.maps
            .iter()
            .enumerate()
            .filter(|(_, m)| m.entry.kind == gtype)
            .map(|(i, _)| i)
            .collect()
    }

    /// `FUN_004CC9D0`: `GTYPE` word of the game mode; invalid index -> empty.
    pub fn gtype(&self, game_type: i32) -> &str {
        self.game_type(game_type).map_or("", |g| g.gtype.as_str())
    }

    /// `FUN_004CBB10`: game type of the game mode = `GNAME` text ID - 15540; invalid index -> 0.
    pub fn game_kind(&self, game_type: i32) -> i32 {
        self.game_type(game_type)
            .map_or(0, |g| g.name_id.wrapping_sub(GAME_TYPE_TEXT))
    }

    /// Inverse of [`Self::game_kind`] (opening `LAN31`): the first list index of the game mode with
    /// the game type `kind`; none -> `None`.
    pub fn game_type_of_kind(&self, kind: i32) -> Option<i32> {
        self.types
            .game_types
            .iter()
            .position(|g| g.name_id.wrapping_sub(GAME_TYPE_TEXT) == kind)
            .and_then(|i| i32::try_from(i).ok())
    }

    fn game_type(&self, game_type: i32) -> Option<&opennml_formats::mp_levels::GameTypeDesc> {
        usize::try_from(game_type)
            .ok()
            .and_then(|i| self.types.game_types.get(i))
    }
}

/// Text like `FUN_0055FFE0` (miss: the most recently formatted number, O-4).
pub fn text_of(texts: &Texts, id: i32) -> String {
    texts.resolve(id);
    texts.display(id)
}

fn catalog_entry(
    vfs: &Vfs,
    texts: &Texts,
    types: &RandomMapTypes,
    entry: MapListEntry,
) -> MapCatalogEntry {
    let name = text_of(texts, entry.name_id);
    let slots = match vfs.read(&entry.path) {
        Ok(bytes) => Some(read_slots(&decode_cp1252(&bytes), types)),
        Err(e) => {
            log::warn!("Map \"{}\" is missing: {e}", entry.path);
            None
        }
    };
    let world = world_path(&entry.path);
    let size = match vfs.read(&world) {
        Ok(bytes) => Some(size_class(&bytes)),
        Err(e) => {
            log::warn!("World \"{world}\" is missing: {e}");
            None
        }
    };
    MapCatalogEntry {
        preview: preview_path(&entry.path),
        level_read: slots.is_some(),
        world_read: size.is_some(),
        slots: slots.unwrap_or_default(),
        size_class: size.unwrap_or(0),
        name,
        entry,
    }
}

/// Slots from the `.lvl` like `FUN_004BDDA0` (first loop, `0x004BDF46`-`0x004BE391`): line by line (without the line
/// ending); a line that is exactly `[PLAYER]` starts a slot whose lines extend to a line that is exactly `}` (indented
/// `}` of nested blocks do not count). In it, after `TrimLeft`: `PLAYER_PLAYER ` -> `Mid(15)` without the last
/// character (the quotation marks) as the nation ([`nation_index`]), `PLAYER_COLOR ` -> `atol` from character 13;
/// `PLAYER_TYPE ` is read and discarded (O-135). Name "EMPTY", type 0, handicap 0, key = block - 1, team = block.
/// Nation and color are stack values in the original that a block without the line inherits from the previous one; the
/// first block without it would have random values - the reimplementation takes nation -1 and color 0.
fn read_slots(text: &str, types: &RandomMapTypes) -> Vec<SlotDraft> {
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let mut slots = Vec::new();
    let (mut nation, mut color) = (-1, 0);
    let mut block = 0i32;
    while let Some(line) = lines.next() {
        if line != "[PLAYER]" {
            continue;
        }
        for line in lines.by_ref() {
            if line == "}" {
                break;
            }
            let t = line.trim_start();
            if t.starts_with("PLAYER_PLAYER ") {
                let mut value: Vec<char> = t.chars().skip(15).collect();
                value.pop();
                nation = nation_index(types, &value.into_iter().collect::<String>());
            } else if t.starts_with("PLAYER_COLOR ") {
                color = atol(t.get(13..).unwrap_or("").as_bytes());
            }
        }
        slots.push(SlotDraft {
            key: block - 1,
            name: EMPTY_SLOT_NAME.to_string(),
            kind: 0,
            nation,
            team: block,
            handicap: 0,
            color,
        });
        block += 1;
    }
    slots
}

/// `FUN_004CBC70`: index of the nation in the `NTYPE` list (exact comparison), otherwise -1.
fn nation_index(types: &RandomMapTypes, nation: &str) -> i32 {
    types
        .nations
        .iter()
        .position(|n| n.ntype == nation)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}

/// `.wld` for the `.lvl` (`FUN_004BDDA0`: path without the last four characters + `".wld"`).
fn world_path(level: &str) -> String {
    let chars: Vec<char> = level.chars().collect();
    let keep = chars.len().saturating_sub(4);
    chars[..keep].iter().collect::<String>() + ".wld"
}

/// Preview image (`FUN_004C6FB0`): path without the last three characters + `"tga"`, so `[n players]-name.tga`.
pub fn preview_path(level: &str) -> String {
    let chars: Vec<char> = level.chars().collect();
    let keep = chars.len().saturating_sub(3);
    chars[..keep].iter().collect::<String>() + "tga"
}

/// Size class from the header of the `.wld` (`FUN_004BDDA0`, second loop): the first line that begins with
/// `"TERRAIN_DIM_X "` after `TrimLeft`, `atol` of the rest; 64 -> 0, 128 -> 1, 256 -> 2, every other value and a
/// missing entry -> 0. The original reads the whole file and takes the **last** match; every `.wld` of the installation
/// has exactly one (in the `[TERRAIN]` block at the start, corpus test), the reimplementation stops at the first.
pub fn size_class(wld: &[u8]) -> i32 {
    const KEY: &[u8] = b"TERRAIN_DIM_X ";
    for line in wld.split(|&b| b == b'\n') {
        let start = line
            .iter()
            .position(|&b| !b.is_ascii_whitespace())
            .unwrap_or(line.len());
        if let Some(rest) = line[start..].strip_prefix(KEY) {
            return match atol(rest) {
                128 => 1,
                256 => 2,
                _ => 0,
            };
        }
    }
    0
}

/// `atol`: whitespace, sign, digits (32-bit wrapping); without digits 0.
pub(crate) fn atol(bytes: &[u8]) -> i32 {
    let start = bytes
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let (negative, digits) = match &bytes[start..] {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0i32, |v, &d| {
            v.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// The selected map in the application (before `CREATE2`): `GameApp+0xD654` (level path, here the catalog index),
/// the map description `GameApp+0x14B8` (`Info`) and the three text pointers `GameApp+0xD734`/`+0xD738`/`+0xD73C`
/// for `[CURLEVELNAME]`, `[CURLEVELSIZE]` and `[CURLEVELPLAYERS]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectedLevel {
    /// Index in [`MapCatalog::maps`] (`GameApp+0xD654`).
    pub map: Option<usize>,
    /// `Info` (`GameApp+0x14B8`).
    pub info: LevelInfoData,
    /// Texts of the placeholders; `None` as long as no map was selected (pointer 0, the replacer inserts nothing).
    pub text: Option<CurLevelText>,
}

impl SelectedLevel {
    /// `FUN_004BFAE0(i, Info)`: select entry `i` of the list filtered by `game_type`. Outside the list ->
    /// false without effect. Otherwise set the level path and name text (`+0xD734`), read the map (`FUN_004BDDA0`,
    /// slots unchanged: `.lvl` path to `Info+0x50`, `Info.LevelType = 0`, with a read `.lvl` the player count, with a
    /// read `.wld` the size); if one of the files is missing -> false, the name in `Info` and the texts of size and
    /// players stay. Otherwise `Info.Name`, `+0xD738` = text `15450 + size`, `+0xD73C` = text `15458 + player count` ->
    /// true.
    pub fn select(&mut self, maps: &MapCatalog, game_type: i32, i: i32, texts: &Texts) -> bool {
        let filtered = maps.filtered(game_type);
        let Some(&index) = usize::try_from(i).ok().and_then(|i| filtered.get(i)) else {
            return false;
        };
        let m = &maps.maps[index];
        self.map = Some(index);
        let text = self.text.get_or_insert_with(CurLevelText::default);
        text.name = m.name.clone();
        self.info.path = m.entry.path.clone();
        self.info.level_type = 0;
        if !m.level_read {
            return false;
        }
        let players = m.players();
        self.info.players = players;
        if !m.world_read {
            return false;
        }
        self.info.size_class = m.size_class;
        self.info.name = m.name.clone();
        text.size = text_of(texts, SIZE_TEXT.wrapping_add(m.size_class));
        text.players = text_of(texts, PLAYERS_TEXT.wrapping_add(players));
        true
    }
}

/// Settings of the match in the session (`FUN_004B9700` on `CREATE2`; re/singleplayer-menus part 2, 1.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftSettings {
    /// `NS+0x1F4`: **game type** = `FUN_004CBB10(GameType)` (`GNAME` text - 15540), not the list index.
    pub game_type: i32,
    /// `NS+0x1EC`: `UnitLimit` (index 0 = 75 units).
    pub unit_limit: i32,
    /// `NS+0x200`: `Resourcen` (0 from map, 1 low, 2 normal, 3 high).
    pub resources: i32,
    /// `NS+0x204`: `TimeLimit` in minutes.
    pub time_limit: i32,
    /// `NS+0x1F8`: `LockTeams != 0`.
    pub lock_teams: bool,
    /// `NS+0x1F0`: `MapExplored != 0`.
    pub map_explored: bool,
    /// `NS+0x1FC`: `SaveReplay != 0`.
    pub save_replay: bool,
    /// `NS+0x08`: `NetSession`.
    pub session: String,
}

/// Match draft = network session `GameApp+0xD650` (also in single player the carrier of the settings and slots).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchDraft {
    /// Map selected at the last `CREATE2` (index in [`MapCatalog::maps`]).
    pub map: Option<usize>,
    /// `NS+0x78`: copy of `Info`.
    pub info: LevelInfoData,
    /// Slot list `NS+0x724` in creation order.
    pub slots: Vec<SlotDraft>,
    pub settings: DraftSettings,
}

impl MatchDraft {
    /// `FUN_004E8CE0`: number of slots.
    pub fn slot_count(&self) -> i32 {
        i32::try_from(self.slots.len()).unwrap_or(i32::MAX)
    }

    /// `NetSession_GetLobbyPlayerInfo` (`0x004E8B90`): the slot with this key.
    pub fn slot(&self, key: i32) -> Option<&SlotDraft> {
        self.slots.iter().find(|s| s.key == key)
    }

    /// `FUN_004E9750`: replace the slot with the key of `slot`, append it if it is missing.
    pub fn set_slot(&mut self, slot: SlotDraft) {
        match self.slots.iter_mut().find(|s| s.key == slot.key) {
            Some(s) => *s = slot,
            None => self.slots.push(slot),
        }
    }

    /// `FUN_004E8D20`: type of the slot, -1 without a slot.
    pub fn kind_of(&self, key: i32) -> i32 {
        self.slot(key).map_or(-1, |s| s.kind)
    }

    /// `FUN_004E9800` (from `Game_StartLevel`, `0x004C85CB`): remove all open slots (type 0).
    pub fn remove_open_slots(&mut self) {
        self.slots.retain(|s| s.kind != 0);
    }
}

/// Do the lobby rows fit a map with `players` players? No occupied row (type != 0: human or computer) with
/// row `>= players`; the slots carry key = row - 1. The core checks the same rule (`opennml_net::rows_fit`); a test in
/// `opennml-client` keeps both equal. The original compares the number of **all** slots with the player count
/// (E-307, `0x004D50E1`-`0x004D50EF`).
pub fn rows_fit(slots: &[SlotDraft], players: i32) -> bool {
    !slots
        .iter()
        .any(|s| s.kind != 0 && s.key.wrapping_add(1) >= players)
}

/// State of the network session for the network menus, set by the host per frame (`MenuApp::set_net`, like
/// diplomacy); `None` without a network session. Own types of the UI (no dependency on `opennml-net`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetMenuView {
    /// Found open games in the order of the first response (`LAN1 OPENGAMES`, re/multiplayer-menus L.1.2).
    pub games: Vec<FoundGameView>,
    /// The lobby, once this machine has joined one (host after `{8}`, client after `{7}`).
    pub lobby: Option<LobbyView>,
}

/// A found game of the list `LAN1`: session ID (replacement for the instance GUID) and session name, the only thing
/// the row shows (E-300).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundGameView {
    pub instance: u64,
    pub name: String,
}

/// The lobby `LAN2` from the point of view of this machine. The rows themselves are in [`MatchDraft::slots`]
/// (key = row - 1); what `SlotDraft` does not carry is here per row 0 ... 7.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyView {
    /// This machine is the host.
    pub is_host: bool,
    /// The own row (host 0).
    pub own_row: u8,
    /// Ready per row.
    pub ready: [bool; 8],
    /// Row with a machine (human connected) per row.
    pub peers: [bool; 8],
    /// The chat lines, fully laid out, oldest first.
    pub chat: Vec<String>,
    /// Stage of the turn raster 0 ... 2 (`NetTurnRaster`, own texts 99 101 ... 99 103).
    pub stage: i32,
    /// Largest measured round trip in ms (host only, once measured).
    pub max_rtt_ms: Option<u32>,
    /// Row whose game data the data check has not found equal to the host's (yet): its `READY` is locked
    /// (re/multiplayer-menus L.18).
    pub data_locked: [bool; 8],
    /// Host: the data check holds `START` back (no own checksum yet or a row not equal).
    pub start_locked: bool,
}

/// Start order of the command `0x0D` (re/singleplayer-menus part 2, 5): everything that `Game_StartLevel` reads from
/// the session - `.lvl` path, game type (`NS+0x1F4`, already converted from the game mode at `CREATE2`,
/// `FUN_004CBB10`), the slots without the open ones (`FUN_004E9800`) in list order, the settings and the map
/// description.
/// The conversion into the setup of the simulation is done by `opennml-client` (`match_request::to_match_setup`).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRequest {
    pub level: String,
    pub game_type: i32,
    pub slots: Vec<SlotDraft>,
    pub settings: DraftSettings,
    pub info: LevelInfoData,
}

impl MatchRequest {
    /// Order from the draft; the draft must have lost the open slots already
    /// ([`MatchDraft::remove_open_slots`]).
    pub fn from_draft(draft: &MatchDraft) -> MatchRequest {
        MatchRequest {
            level: draft.info.path.clone(),
            game_type: draft.settings.game_type,
            slots: draft.slots.clone(),
            settings: draft.settings.clone(),
            info: draft.info.clone(),
        }
    }
}

/// First text IDs of the values in `LEVELINFO` (`FUN_004B9C90`).
const LEVELINFO_UNIT_LIMIT: i32 = 15531;
const LEVELINFO_RESOURCES: i32 = 15550;
/// Map type, landscape and time of day of a random map (`FUN_004B9C90`, rows 4-6).
const LEVELINFO_TERRAIN: i32 = 15470;
const LEVELINFO_LANDSCAPE: i32 = 15490;
const LEVELINFO_DAYTIME: i32 = 15500;

/// `[CURLEVELINFOS]` = `FUN_004B9C90(1)` (`0x004B9C90`, re/singleplayer-menus part 2, 4.4): 13 rows from the
/// **session** (`NS+0x78`, here the draft), each as symbol 11 "[COLOR FF88FF88]" + label + symbol 2
/// "[DEFAULTCOLOR]" + value + symbol 6 "[CR]" (`extrasymbols.txt`; the original queries the three symbols first).
/// "Valid" = `NS.LevelType (+0x1CC) != -1`; otherwise the values and the switch and time-limit rows are omitted.
///
/// Rows: name (15650, for a random map 15659 "Random value"), size (15651, `15450 + size`), players (15652,
/// `15458 + player count`), only for a random map map type/landscape/time of day (15671/15670/15672), unit limit
/// (15653, `15531 + UnitLimit`), the switches SaveReplay/LockTeams/MapExplored as "[DEFAULTCOLOR]" + 15656/15657/
/// 15658 + "[CR]" (only if set), time limit (15655, `"%d %s"` with 15674 "minutes", only for game type 2...4),
/// resources (15654, `15550 + Resourcen`), game mode (15673, `15540 + game type`).
///
/// Random map: the values of rows 4-6 are `15470 + terrain`, `15490 + landscape`,
/// `15500 + time of day` (record `+0x168/+0x16C/+0x170`).
///
/// Network session (own rows of the reimplementation): with `net` the function appends the row
/// "Turn raster" (99 100) with the stage (99 101 ... 99 103), built like the rows of the original (label with
/// ": "), and - once measured, only for the host ([`LevelNetInfo::max_rtt_ms`]) - the round trip (99 104 "Round trip:
/// %d ms") as a row in the base color like the switch rows.
pub fn level_infos(draft: &MatchDraft, net: Option<&LevelNetInfo>, texts: &Texts) -> String {
    let cr = text_of(texts, 6);
    let green = text_of(texts, 11);
    let default = text_of(texts, 2);
    let info = &draft.info;
    let s = &draft.settings;
    let valid = info.level_type != -1;
    let random = valid && info.level_type == 1;
    let mut out = String::new();
    // One row; the original queries the value after the label (order of the misses, O-4).
    let line = |out: &mut String, label: i32, value: &dyn Fn() -> String| {
        out.push_str(&green);
        out.push_str(&text_of(texts, label));
        out.push_str(&default);
        if valid {
            out.push_str(&value());
        }
        out.push_str(&cr);
    };
    let text = |id: i32| text_of(texts, id);
    line(&mut out, if random { 15659 } else { 15650 }, &|| {
        info.name.clone()
    });
    line(&mut out, 15651, &|| {
        text(SIZE_TEXT.wrapping_add(info.size_class))
    });
    line(&mut out, 15652, &|| {
        text(PLAYERS_TEXT.wrapping_add(info.players))
    });
    if random {
        for (label, first, value) in [
            (15671, LEVELINFO_TERRAIN, info.random_terrain),
            (15670, LEVELINFO_LANDSCAPE, info.random_landscape),
            (15672, LEVELINFO_DAYTIME, info.random_daytime),
        ] {
            line(&mut out, label, &|| text(first.wrapping_add(value)));
        }
    }
    line(&mut out, 15653, &|| {
        text(LEVELINFO_UNIT_LIMIT.wrapping_add(s.unit_limit))
    });
    if valid {
        for (on, label) in [
            (s.save_replay, 15656),
            (s.lock_teams, 15657),
            (s.map_explored, 15658),
        ] {
            if on {
                out.push_str(&default);
                out.push_str(&text(label));
                out.push_str(&cr);
            }
        }
        if (2..5).contains(&s.game_type) {
            // `swprintf(GameApp+0x19A0, L"%d %s", TimeLimit, text 15674)`.
            line(&mut out, 15655, &|| {
                format!("{} {}", s.time_limit, text(15674))
            });
        }
    }
    line(&mut out, 15654, &|| {
        text(LEVELINFO_RESOURCES.wrapping_add(s.resources))
    });
    line(&mut out, 15673, &|| {
        text(GAME_TYPE_TEXT.wrapping_add(s.game_type))
    });
    if let Some(net) = net {
        out.push_str(&green);
        out.push_str(&text(own_texts::TURN_RASTER));
        out.push_str(": ");
        out.push_str(&default);
        out.push_str(&text(own_texts::RASTER_STAGE.wrapping_add(net.stage)));
        out.push_str(&cr);
        if let Some(rtt) = net.max_rtt_ms {
            out.push_str(&default);
            out.push_str(&text(own_texts::ROUND_TRIP).replacen("%d", &rtt.to_string(), 1));
            out.push_str(&cr);
        }
    }
    out
}

/// What `LEVELINFO` shows from the lobby ([`level_infos`]): the stage of the turn raster (0 ... 2) for everyone, the
/// largest round trip only for the host. At the same time the key under which `MenuApp` recomputes `[CURLEVELINFOS]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelNetInfo {
    pub stage: i32,
    pub max_rtt_ms: Option<u32>,
}

impl LevelNetInfo {
    /// From the lobby view: stage always, round trip only for the host.
    pub fn from_lobby(lobby: &LobbyView) -> LevelNetInfo {
        LevelNetInfo {
            stage: lobby.stage,
            max_rtt_ms: if lobby.is_host {
                lobby.max_rtt_ms
            } else {
                None
            },
        }
    }

    /// From the network state of the menus: only in a lobby.
    pub fn from_net(net: Option<&NetMenuView>) -> Option<LevelNetInfo> {
        net.and_then(|n| n.lobby.as_ref()).map(Self::from_lobby)
    }
}

#[cfg(test)]
pub(crate) mod tests;
