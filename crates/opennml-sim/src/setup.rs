//! Match setup: players from the `.lvl`, lobby assignment like
//! `GameEngine_SetupPlayersFromLobby` (`0x00432C00`), diplomacy from the teams, the objects of the
//! `.wld` (`World_PlaceObject` `0x00440720`) and the start objects at the start points
//! (`World_PlaceStartObjects` `0x0043E970`).

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::ki::KiSetup;
use crate::message::Message;
use crate::messaging::send;
use crate::path::region::SECTOR;
use crate::random_map::{RandomMapRules, RandomMapSpec};
use crate::rng::CrtRand;
use crate::rules::{RuleChecksums, Rules};
use crate::script::ScriptSetup;
use crate::sim::Sim;
use crate::state::{Control, SimState};
use crate::world::{ObjectDesc, create_object, create_object_named, desc_flags};
use opennml_formats::wld::WorldObject;

/// Start point (`OBJ_GUID 4200`): location of the start objects of the player `OBJ_COLOR`.
pub const START_POINT: u32 = 4200;
/// Game type of the menu level behind the main menu (`FUN_004C9140` `0x004C9140`).
pub const MENU_LEVEL_MODE: i32 = -2;
/// Ambient effects of the `.wld` (surf, fish, eagles): no game objects.
pub const AMBIENT: [u32; 5] = [4104, 4105, 4106, 3411, 3408];
/// Player flag of the game type 2 start objects.
pub const PLAYER_FLAG: u32 = 4300;
/// Flags of the world objects of a random map (`FUN_005474B0` -> `World_PlaceObject` `0x00440720`):
/// `READY | COLOR | PLAYER | ROT_Y | POSITION | GUID` = `0x8D3` - the slot flag also for owner 8.
pub const RANDOM_OBJECT_FLAGS: u32 = desc_flags::READY
    | desc_flags::COLOR
    | desc_flags::PLAYER
    | desc_flags::ROT_Y
    | desc_flags::POSITION
    | desc_flags::GUID;
/// AI difficulty of the campaign menu (0 to 2); equal to `opennml_control::CAMPAIGN_DIFFICULTY_RANGE`, with which
/// the control channel and viewer check the setup.
pub const DIFFICULTY_RANGE: RangeInclusive<i32> = 0..=2;
/// Format version of the start package, savegame and replay; increases when the behavior of the
/// simulation changes (old replays are then rejected). 2: path and occupancy grid;
/// 4: economy (account, counter "in progress", extra tick list, resource and
/// living space setting); 5: fog of war (grid, cycle, vision sources, "map explored");
/// 6: combat (damage factors, behavior modes, combat points, rank); 7: ranged combat (building
/// map, projectiles, hit zones, modules 10/19-22); 8: groups and formations;
/// 9: riding, swimming, diving (modules 13/14, 15, 24); 10 = ships, train, multipart objects
/// (meeting point, modules 11/33/51, last track, event bits); 11 = upgrades and arming
/// (type table, queue, counter-upgrades, time of day, temporary upgrades, modules
/// 30/38/44); 12 = tents, trade (modules 35, 42, 43); 13 = gates and enemy watch (area
/// queries, gate state, building guard); 14 = stealth, digging in, healing (modules
/// 25/28/29, message filter checked); 15 = ghosts, banishing, animal evocation, auras
/// (modules 26/27/31/32, influence of module 44); 16 = spells and special attacks (module 38
/// target point/corpse, modules 36/37/49, world effects dynamite/explosion/storm); 17 = headhunter
/// and robbing (world field `headhunters`, modules 34/48, throw); 18 = AI state
/// (`SimState::ki`, `KiSetup` in the start package, catalog in the rules); 19 = AI world state
/// (`GameObject::ai_marks`, `SimState::attack_reports`, `SimState::campaign_difficulty`); 20 =
/// AI commands and AI control commands (`CommandKind` `Die`, `OccupancyBorder`, `AiPlayer*`,
/// `AiObject*`, `AddResource`, `KiStart`, `KiPause`, `KiStep`); 21 = script state
/// (`SimState::script`, `SimState::names`, evocation counter, `ScriptSetup` in the start package); 22 =
/// match end and statistics (rule check, command `SetPlayerResult`, timer list,
/// `SimState::accounting`, `MatchSetup::local_name`/`time_limit`); 23 = world fields of the scripts
/// (kill counters, build button lock, fade-out state, vision rectangles, mission objectives,
/// area overrides, level container with ID 1); 24 = new commands (0x42/0x43), skip dialog, destruction
/// helper, skip flag; 25 = presentation state, selection mirror, button, profile variables, commands
/// ScriptSelection/ScriptButton; 26 = idle workers (`SimState::idle_workers`); 27 = `SimState::flag_leader`,
/// `time_limit`; 28 = map description in the start package (`StartPackage::random_map`); 29 = network flag
/// `SimState::network`, `MatchSetup::network`.
pub const FORMAT_VERSION: u32 = 29;

/// Default of `TimeLimit` in minutes (`Engine.ini`, `EngineConfig`; re/singleplayer-menus part 2, 1.2).
pub const DEFAULT_TIME_LIMIT: i32 = 20;

/// Largest time limit in minutes whose `· 60 · 1000` milliseconds still fit in 32 bits
/// (`i32::MAX / 60_000`); upper limit of the setup in the viewer. `new_match` computes wrapped, a larger
/// value only results in a different time limit.
pub const MAX_TIME_LIMIT: i32 = 35_791;

/// ID of the timer of the time limit (`FUN_004453D0` `0x004453D0`, game type 4; re/ingame-menus 7).
pub const TIME_LIMIT_COUNTDOWN: i32 = 0;

fn default_time_limit() -> i32 {
    DEFAULT_TIME_LIMIT
}

/// Kind of a lobby slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotKind {
    /// Closed.
    Closed,
    Human,
    /// AI (control kind 2); `MatchSetup::ki.enabled` starts the opponent AI.
    Ai,
}

/// An entry of the lobby (`NetSession_GetLobbyPlayerInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyEntry {
    pub kind: SlotKind,
    pub name: String,
    /// Nation 0-5 (0 WID, 1 PRI, 2 SPA, 3 SID, 4 ENG, 5 PAT).
    pub nation: i32,
    pub team: i32,
    pub color: i32,
    pub handicap: i32,
}

/// Match setup (part of the start package).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSetup {
    /// Game type (`GameEngine+0x108`): -1 campaign, 0 normal, 1 kill hero, 2 domination (hold
    /// flags), 3 King of the Hill, 4 and 5 Railroad 1 and 2 (railway); 6 is locked. Victory and defeat
    /// according to the rules of the game type
    /// ([`crate::game_end::check_victory_defeat`]); the campaign has none.
    pub mode: i32,
    /// Start index of the random sequence (the original starts at 0).
    pub seed: u32,
    /// Lobby in order; only for multiplayer maps (mode >= 0).
    pub lobby: Vec<LobbyEntry>,
    /// Setting "resources" (`GameEngine+0x4EC`): 0 from the map (`.lvl`), 1 low, 2 normal,
    /// 3 high (`resourcenconfig.ini`); multiplayer only.
    pub resource_setting: i32,
    /// Setting "living space" 0-5 -> limit 75, 100, 125, 150, 175, 200 (default 2 = 125); multiplayer
    /// only, the campaign always has 125.
    pub living_space: i32,
    /// Game setting "map explored" (`MapExplored`, `GameEngine+0x114`; default off as in
    /// `Engine.ini`): fills "explored" completely at the start (`GameWorld_Init` `0x0048D2DC`).
    #[serde(default)]
    pub map_explored: bool,
    /// AI settings; `enabled` starts the AI in [`new_match`].
    #[serde(default)]
    pub ki: KiSetup,
    /// Script setup; `enabled` starts the level script in
    /// [`new_match`].
    #[serde(default)]
    pub script: ScriptSetup,
    /// Setting "time limit" in minutes (`TimeLimit`, session `NS+0x204` -> `GameEngine+0x4E8 =
    /// TimeLimit * 60` in `GameEngine_ApplyGameSettings` `0x00431160`): in game type 4 the
    /// timer 0 runs over `TimeLimit · 60 · 1000` ms ([`new_match`]).
    #[serde(default = "default_time_limit")]
    pub time_limit: i32,
    /// Name of the local player (profile name, `EngineConfig+0x13C` as in `FUN_00403F50`) for accounting
    /// and savegame; `None` without a profile. As in the original it only takes effect in the
    /// campaign (mode < 0) on the first human slot; in free play the lobby name applies.
    #[serde(default)]
    pub local_name: Option<String>,
    /// Network match: sets [`SimState::network`] (locks, no chat cheats).
    #[serde(default)]
    pub network: bool,
}

impl Default for MatchSetup {
    /// Campaign (mode -1) without lobby, resources from the map, living space 125, AI and script off,
    /// time limit [`DEFAULT_TIME_LIMIT`], without names - like [`MatchSetup::default_for`] for a
    /// campaign map.
    fn default() -> Self {
        Self {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: KiSetup::default(),
            script: ScriptSetup::default(),
            time_limit: DEFAULT_TIME_LIMIT,
            local_name: None,
            network: false,
        }
    }
}

/// Start package: everything from which a match arises bit-identically anew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPackage {
    pub format: u32,
    /// Normalized path of the `.lvl`; for a random map `randommap:<seed text>` (no file).
    pub level: String,
    /// Checksums of the rules ([`Rules::checksums`]).
    pub rules: RuleChecksums,
    pub setup: MatchSetup,
    /// Map description of a random map: loading regenerates the map from it,
    /// with the game mode of the savegame (deviating from D-348). `None` for fixed maps. `serde(default)` only as
    /// documentation: the savegame is postcard, the format version catches older savegames.
    #[serde(default)]
    pub random_map: Option<RandomMapSpec>,
}

impl MatchSetup {
    /// Default of a map as in the original menu: multiplayer maps (`multiplayer/`) in mode 0
    /// with one human player on every `.lvl` slot with nation, color and own team
    /// ("all against all"); other maps as campaign (mode -1) with the players of the `.lvl`.
    pub fn default_for(rules: &Rules) -> Self {
        if !rules.level_path.contains("/multiplayer/") {
            return Self::default();
        }
        let lobby = rules
            .level
            .players
            .iter()
            .map(|p| LobbyEntry {
                kind: SlotKind::Human,
                name: p.name.clone(),
                nation: p.nation,
                team: i32::from(p.slot),
                color: i32::from(p.slot),
                handicap: 0,
            })
            .collect();
        Self {
            mode: 0,
            lobby,
            ..Self::default()
        }
    }
}

/// AI kind of the lobby -> handicap (E-40): 1 -> 5, 2 -> 2, 3 -> 0, 4 -> 0, otherwise `None`. The kind
/// lives only as a handicap in the lobby entry, as in the original.
pub fn ki_kind_handicap(kind: u8) -> Option<i32> {
    match kind {
        1 => Some(5),
        2 => Some(2),
        3 | 4 => Some(0),
        _ => None,
    }
}

/// New match: players, diplomacy, all objects of the `.wld` and the start objects. On a
/// random map (`rules.random_map`, `GameEngine_CreateRandomMapLevel` `0x004402F0` after the
/// generator) the CRT sequence continues at the final state of the generator and the
/// world objects arise like `FUN_005474B0`.
pub fn new_match(rules: &Rules, setup: &MatchSetup) -> Result<SimState, String> {
    if let Some(random) = &rules.random_map {
        check_random_map_mode(random, setup)?;
    }
    // The regions of the pathfinding run in 16 x 16 sectors (16, 64, 128, 256 tiles in the shipped data);
    // other sizes ran beyond the fields in the original (exception A7).
    let (w, h) = (rules.map.dim_x, rules.map.dim_z);
    if w <= 0 || h <= 0 || w % SECTOR != 0 || h % SECTOR != 0 {
        return Err(format!(
            "map size {w} x {h} is not supported: the pathfinding needs multiples of {SECTOR} tiles"
        ));
    }
    let mut state = SimState::new(setup.mode, setup.seed);
    // On the random map `rand()` continues after the generator without a new `srand` (D-342);
    // the first draws after that make the trees of the world objects (measured on the original).
    if let Some(random) = &rules.random_map {
        state.crt = CrtRand::new(random.crt_start);
    }
    // `0x4E39` reads the difficulty in the tick (`GameEngine+0x5F8`); the simulation does not
    // know the start package at run time.
    state.campaign_difficulty = setup.ki.campaign_difficulty;
    state.network = setup.network;
    state.init_map(rules);
    if setup.map_explored {
        state.fog.explored.fill(0xFF);
    }
    let mut sim = Sim::new(&mut state, rules);
    apply_game_settings(&mut sim, setup);
    setup_players(&mut sim, setup)?;
    set_local_accounting_name(&mut sim, setup);
    apply_player_handicaps(&mut sim);
    place_level_objects(&mut sim, setup);
    // At the end of the `.wld`: regions of all layers, without clearing the dirty rectangle.
    sim.state.path.build_regions(u32::MAX);
    // Idle workers (`0x0047ECA0`, in the original at the start of `GameWorld_ApplyPlayerHandicaps`
    // `0x00484B70` from `GameEngine_InitLevelView`, i.e. after the world build and before the AI start;
    // here the factors run before the objects, hence not until here). It does not run when loading.
    crate::idle_workers::init(&mut sim);
    // AI start like `FUN_0043D560` after the world build; events of the start expire
    // with those of the build.
    if setup.ki.enabled {
        crate::ki::start::start_match(&mut sim);
    }
    // Timer of the time limit after the AI start like `GameEngine_InitLevelView` (`0x004458B0`, only
    // when restarting a level).
    start_time_limit(&mut sim, setup);
    // Script start after the AI start; without `script.enabled` the default state remains.
    crate::script::start_match(&mut sim, &setup.script, setup.seed)?;
    state.events.clear();
    Ok(state)
}

/// Random maps exist only for the game modes 0 ... 3 (`GTYPE` index = game type); the match must have
/// the game mode of the map description from which the generator built the map.
fn check_random_map_mode(random: &RandomMapRules, setup: &MatchSetup) -> Result<(), String> {
    let game_type = random.spec.game_type;
    if (0..=3).contains(&setup.mode) && usize::try_from(setup.mode).ok() == Some(game_type) {
        return Ok(());
    }
    Err(format!(
        "random map: game mode {} does not match the map description (game mode {game_type}; \
        random maps exist only for the game modes 0 to 3)",
        setup.mode
    ))
}

/// `FUN_004453D0` (`0x004453D0`): in game type 4 (Railroad 1) timer 0 with `GameEngine+0x4E8 · 1000`
/// ms, i.e. `TimeLimit · 60 · 1000` (wrapped to 32 bits like the original); via `FUN_00445350` (remove the old
/// timer with the same ID, append the new one at the end, kind 1 = remaining time top left, position `{0, 0x28}`).
/// Other game types create none at the start (re/ingame-menus 7).
fn start_time_limit(sim: &mut Sim, setup: &MatchSetup) {
    if setup.mode == 4 {
        let ms = sim.state.time_limit.wrapping_mul(1000);
        sim.state.countdowns.insert(TIME_LIMIT_COUNTDOWN, ms);
    }
}

/// Living space limit and time limit from the game settings (`GameEngine_ApplyGameSettings`
/// `0x00431160`): campaign (mode < 0) 125, otherwise according to setting 0-5; `m_TimeLimit`
/// (`Engine+0x4E8`) = `TimeLimit · 60` seconds in every game type (wrapped to 32 bits).
fn apply_game_settings(sim: &mut Sim, setup: &MatchSetup) {
    sim.state.time_limit = setup.time_limit.wrapping_mul(60);
    let e = &mut sim.state.economy;
    e.max_living_space = if setup.mode < 0 {
        crate::economy::DEFAULT_MAX_LIVING_SPACE
    } else {
        usize::try_from(setup.living_space)
            .ok()
            .and_then(|i| crate::economy::LIVING_SPACE_STEPS.get(i))
            .copied()
            .unwrap_or(e.max_living_space)
    };
}

/// `GameEngine_ApplyResourceConfigStartResources` (`0x00431280`, only from the lobby setup):
/// with setting 1-3 every start resource record of `resourcenconfig.ini` sets its resource in
/// file order for **all** players, also empty slots
/// (`Player_SetResource`, population and living space untouched). The original runs over nine
/// player objects; the ninth has no counterpart in the simulation.
///
/// The original checks `([GameEngine+0x2A0]+0x160C ≠ 0 || setting > 0) && 0 ≤ setting < 4`
/// (`Info.LevelType ≠ 0`): so on a random map (`random_map`) stage 0 also applies
/// (`values[0]`, W-6), on fixed maps not until stage 1.
fn apply_resource_config_start_resources(sim: &mut Sim, setting: i32, random_map: bool) {
    if !(0..=3).contains(&setting) || (setting == 0 && !random_map) {
        return;
    }
    let rules = sim.rules;
    for p in &mut sim.state.players {
        for r in rules.resource_config.start_records() {
            if let Ok(index) = usize::try_from(r.config_guid) {
                p.set_resource(index, r.values[setting as usize]);
            }
        }
    }
}

/// `GameWorld_ApplyPlayerHandicaps` (`0x00484B70`): factor `(10 − handicap) · 10` percent per slot
/// 0-7 for resources as well as damage dealt and suffered; at the end the time-of-day upgrades of the
/// time of day of the level (`level_resources[40]`, re/upgrades 3.8).
fn apply_player_handicaps(sim: &mut Sim) {
    for slot in 0..PLAYER_SLOTS {
        let h = sim.state.players[slot].handicap;
        let f = 10i32.wrapping_sub(h).wrapping_mul(10);
        sim.state.economy.set_resource_factor(slot as i32, f);
        sim.state.combat.attack_factor[slot] = f;
        sim.state.combat.hit_factor[slot] = f;
    }
    sim.state.daytime = -1;
    let t = sim.rules.level_daytime();
    crate::upgrades::apply_daytime_upgrades(sim, t);
}

/// Players from the `.lvl` (`PlayerSetup_ParseLvl`), in multiplayer then the lobby. Both paths create
/// the records of the statistics log (`FUN_004BA5D0`, re/accounting 3.4): from the `.lvl` only in the
/// campaign (`GameEngine+0x108 ≤ −1`) with color = team = slot (O-173) and the nation from
/// `PLAYER_PLAYER` (`0x00486BE6`, `[ESP+0x2C]` = nation), from the lobby with its values.
///
/// Lobby overflow (`0x00432E09 … 0x00432E55`, D-339, for all maps): the entries
/// go in order onto the active slots; if the search reaches slot 8, it starts at 0 with
/// a new round - surplus entries thus overwrite assigned slots (the last one wins).
/// After more than 8 rounds (no active slot) the original returns without start resources and diplomacy
/// (`0x00432F94`). Fixed maps do not reach the overflow (their lobby has at most as many
/// entries as the `.lvl` has slots). If an entry overwrites the slot of a human, the
/// original would have no local player anymore (hypothesis: memory error): the reimplementation aborts
/// with a message.
fn setup_players(sim: &mut Sim, setup: &MatchSetup) -> Result<(), String> {
    for lp in &sim.rules.level.players {
        if setup.mode < 0 {
            let slot = i32::from(lp.slot);
            sim.state
                .accounting
                .init_record(slot, &lp.name, slot, lp.nation, slot);
        }
        let p = &mut sim.state.players[usize::from(lp.slot)];
        p.slot_active = true;
        p.name = lp.name.clone();
        p.control = match lp.kind {
            0 => Control::Human,
            1 => Control::Ai,
            _ => Control::Passive,
        };
        p.nation = lp.nation;
        p.result = 0;
        for &(index, value) in &lp.resources {
            p.set_resource(index, value);
        }
    }
    if setup.mode < 0 {
        return Ok(());
    }
    let random_map = sim.rules.random_map.is_some();
    let players = &mut sim.state.players;
    for p in players.iter_mut() {
        p.control = Control::Empty;
        p.result = 3;
    }
    // Lobby in order onto the active slots, with overflow to the front (`EBX` slot, `EBP` round).
    let mut slot = 0usize;
    let mut rounds = 0u32;
    let mut human = [false; PLAYER_SLOTS];
    for entry in &setup.lobby {
        loop {
            if slot >= PLAYER_SLOTS {
                slot = 0;
                rounds += 1;
                if rounds > 8 {
                    return Ok(());
                }
            }
            if players[slot].slot_active {
                break;
            }
            slot += 1;
        }
        if human[slot] {
            let map = if random_map { "random map" } else { "map" };
            return Err(format!("{map}: too few start slots, the own slot was lost"));
        }
        human[slot] = entry.kind == SlotKind::Human;
        let p = &mut players[slot];
        p.nation = entry.nation;
        (p.control, p.result) = match entry.kind {
            SlotKind::Closed => (Control::Empty, 3),
            SlotKind::Ai => (Control::Ai, 0),
            SlotKind::Human => (Control::Human, 0),
        };
        p.color = entry.color;
        p.team = entry.team;
        p.handicap = entry.handicap;
        p.name = entry.name.clone();
        // `GameEngine_SetupPlayersFromLobby` (`0x00432DC6`/`0x00432EFD`) creates a record for every
        // lobby entry. Closed entries do not exist there in the original (`Game_StartLevel`
        // removes open slots beforehand, `FUN_004E9800`); in the reimplementation they are the fillers
        // of the railroad lobby, which leave the slot unchanged - they get no record.
        if entry.kind != SlotKind::Closed {
            sim.state.accounting.init_record(
                slot as i32,
                &entry.name,
                entry.color,
                entry.nation,
                entry.team,
            );
        }
        slot += 1;
    }
    apply_resource_config_start_resources(sim, setup.resource_setting, random_map);
    let players = &mut sim.state.players;
    // Equal teams are friends - also with unoccupied slots (quirk).
    for a in 0..PLAYER_SLOTS {
        if !players[a].slot_active {
            continue;
        }
        for b in 0..PLAYER_SLOTS {
            if a != b && players[b].team == players[a].team {
                let d = &mut sim.state.diplomacy;
                d.set_friend(a as i32, b as i32, true);
                d.set_enemy(a as i32, b as i32, false);
                d.set_vision(a as i32, b as i32, true);
            }
        }
    }
    Ok(())
}

/// `Game_SelectLocalPlayer` (`0x004BB3B0`) outside of network play: with game type < 0 (campaign)
/// `FUN_00403F50(GameEngine_FindNthHumanPlayerSlot(0), profile name)` - the first active slot with
/// control kind human gets the profile name in the statistics log ([`MatchSetup::local_name`],
/// `EngineConfig+0x13C`). In free play the lobby entry carries the name.
fn set_local_accounting_name(sim: &mut Sim, setup: &MatchSetup) {
    let Some(name) = setup.local_name.as_deref() else {
        return;
    };
    if setup.mode >= 0 {
        return;
    }
    if let Some(slot) = sim
        .state
        .players
        .iter()
        .position(|p| p.slot_active && p.control == Control::Human)
    {
        sim.state.accounting.set_local_name(slot as i32, name);
    }
}

/// Display color of a slot (`GameWorld_GetPlayerColor` `0x004789E0`): players 0-7 their
/// lobby color, otherwise 8.
pub fn player_color(sim: &Sim, slot: i32) -> i32 {
    usize::try_from(slot)
        .ok()
        .and_then(|s| sim.state.players.get(s))
        .map_or(8, |p| p.color)
}

/// `[OBJECT]` blocks of the `.wld` in file order (`World_ParseNextBlock` and
/// `World_PlaceObject`): start points set the start objects of occupied slots, ambient effects
/// are presentation, everything else is created. The `[CONFIG]` of the object applies until the next
/// object (and beyond, quirk). Random maps: [`place_random_map_objects`].
fn place_level_objects(sim: &mut Sim, setup: &MatchSetup) {
    if sim.rules.random_map.is_some() {
        place_random_map_objects(sim, setup.resource_setting);
        return;
    }
    let rules = sim.rules;
    for o in &rules.world_objects {
        sim.state.object_config = o.config.clone();
        let player = o.color.unwrap_or(8);
        let mut desc = ObjectDesc {
            flags: desc_flags::PLACED
                | desc_flags::GUID
                | desc_flags::POSITION
                | desc_flags::ROT_Y
                | desc_flags::COLOR,
            guid: o.guid,
            player,
            position: o.position,
            scale: o.scale,
            rotation: o.rotation,
            color: player_color(sim, player),
        };
        if (0..8).contains(&player) {
            desc.flags |= desc_flags::PLAYER;
        }
        // The player of this slot (`GetPlayer(OBJ_COLOR)`), if there is one.
        let slot_player = usize::try_from(player)
            .ok()
            .and_then(|s| sim.state.players.get(s));
        // `OBJ_COLOR` (`0x004431C2`): in game type 4 (Railroad 1) an object of unoccupied
        // slots is omitted (`Player_GetResultState ≠ 0`; empty and closed have result 3),
        // start points included. Game type 5 and all others place it.
        if sim.state.mode == 4 && slot_player.is_some_and(|p| p.result != 0) {
            continue;
        }
        if o.guid == START_POINT {
            if slot_player.is_some_and(|p| p.control != Control::Empty) {
                place_start_objects(sim, player as u8, o.position);
            }
            continue;
        }
        if AMBIENT.contains(&o.guid) {
            continue;
        }
        // `OBJ_NAME` untrimmed (E-147); `wld.rs` writes it in lowercase with `to_lowercase`, the
        // registration once more with `lower_german` - no difference in the shipped data.
        create_object_named(sim, &mut desc, o.name.as_deref());
    }
}

/// World objects of a random map in placement order (`FUN_005474B0` -> `World_PlaceObject`
/// `0x00440720` with flag argument 1): for **every** object first
/// [`apply_resource_config_to_object`] with the stage "resources" (also 0, W-6; `0x00440751`), then
/// `TOWN_CENTER` 4200 -> start objects of the slot when control != empty (`0x00440777`, origin with
/// y = 0), the ambient effects ([`AMBIENT`], including `FISH` 3411) are presentation, everything else
/// arises with [`RANDOM_OBJECT_FLAGS`]. The fisher baskets draw nothing from `crt` (W-61).
fn place_random_map_objects(sim: &mut Sim, stage: i32) {
    let rules = sim.rules;
    for o in &rules.world_objects {
        apply_resource_config_to_object(sim, o.guid, stage);
        let mut desc = random_object_desc(sim, o);
        if o.guid == START_POINT {
            let occupied = usize::try_from(desc.player)
                .ok()
                .and_then(|s| sim.state.players.get(s))
                .is_some_and(|p| p.control != Control::Empty);
            if occupied {
                place_start_objects(sim, desc.player as u8, desc.position);
            }
            continue;
        }
        if AMBIENT.contains(&o.guid) {
            continue;
        }
        create_object(sim, &mut desc);
    }
}

/// Creation description of a random map object (`FUN_005474B0`): [`RANDOM_OBJECT_FLAGS`],
/// owner from the scene (without: 8), position `[x, 0.0, z]` exactly - the height is set by
/// `GameObject_SnapHeightToTerrain` when snapping, its ray starts above the old height -,
/// rotation of the scene (only Y takes effect), color `GameWorld_GetPlayerColor(slot)`.
fn random_object_desc(sim: &Sim, o: &WorldObject) -> ObjectDesc {
    let player = o.color.unwrap_or(8);
    ObjectDesc {
        flags: RANDOM_OBJECT_FLAGS,
        guid: o.guid,
        player,
        position: [o.position[0], 0.0, o.position[2]],
        scale: [1.0; 3],
        rotation: o.rotation,
        color: player_color(sim, player),
    }
}

/// `GameEngine_ApplyResourceConfigToObject` (`0x004349A0`, only on random maps): at stage 0 ... 3
/// clear the object configuration (`GameEngine+0x2B4`) (`0x004349BB`) and, if `resourcenconfig.ini`
/// has a record for the GUID (`GameEngine+0x2C90`), set `[config_guid] = values[stage]`
/// (`0x004349E3 … 0x004349F1`). Other stages leave the configuration in place (unreachable
/// from the menu).
fn apply_resource_config_to_object(sim: &mut Sim, guid: u32, stage: i32) {
    let Ok(index) = usize::try_from(stage) else {
        return;
    };
    if index >= 4 {
        return;
    }
    sim.state.object_config.clear();
    let record = i32::try_from(guid)
        .ok()
        .and_then(|g| sim.rules.resource_config.object_record(g));
    if let Some(r) = record {
        sim.state
            .object_config
            .insert(r.config_guid as u32, r.values[index]);
    }
}

/// `World_PlaceStartObjects`: list [game type][nation] from `defaultobjects.ini`; position = start point +
/// (dx * SX, 0, dz * SZ) without rotation, buildings snap with the hull box corner to the tile, the
/// flag 4300 looks for the first free tile ([`find_free_flag_tile`]);
/// created finished (`0x8C3`), buildings get `0x3303` (lock margin, effective via `building::query`).
fn place_start_objects(sim: &mut Sim, slot: u8, origin: [f32; 3]) {
    let rules = sim.rules;
    let nation = sim.state.players[usize::from(slot)].nation;
    if !(0..6).contains(&nation) || !(0..7).contains(&sim.state.mode) {
        return;
    }
    let Some(list) = rules.default_objects.start.get(&(sim.state.mode, nation)) else {
        return;
    };
    let cell = rules.map.cell;
    for entry in list {
        let mut v = [
            origin[0] + cell[0].wrapping_mul(entry.dx) as f32,
            origin[1],
            origin[2] + cell[2].wrapping_mul(entry.dz) as f32,
        ];
        // `GameEngine_SnapStartObjectPosition` (`0x00434A00`): the flag 4300 goes only to
        // `FindFreeFlagTile`, all others to the hull box snapping (D-297).
        if entry.guid == PLAYER_FLAG {
            find_free_flag_tile(sim.state, rules, &mut v);
        } else {
            snap_start_object(rules, entry.guid, &mut v);
        }
        let mut desc = ObjectDesc {
            flags: desc_flags::START_OBJECT,
            guid: entry.guid,
            player: i32::from(slot),
            position: v,
            scale: [1.0; 3],
            rotation: [0.0; 3],
            color: player_color(sim, i32::from(slot)),
        };
        if let Some(id) = create_object(sim, &mut desc)
            && entry.guid != PLAYER_FLAG
        {
            send(sim, id, &Message::new(0x3303, &[1]));
        }
    }
}

/// `World_PlaceSupplyObjects` (`0x0043EAE0`, caller `Train_UnloadSupply` `0x00440910`;
/// re/object-lifecycle 4): supply list 0 of the nation of slot `slot` (`defaultobjects.ini`
/// `SUPPLY 0`), only for nations 0 to 5 (otherwise `false`). Every entry arises finished (`0x8C3`) on
/// slot `slot` at `origin + (SX · dx, 0, SZ · dz)` - the sum is computed in `f32` -,
/// **without snapping and without `0x3303`** (K-35; unlike `place_start_objects`). The original
/// has no return value; `true` here means "the list is processed" (also an empty one).
pub fn place_supply_objects(sim: &mut Sim, slot: u8, origin: [f32; 3]) -> bool {
    let rules = sim.rules;
    let Some(nation) = sim.state.players.get(usize::from(slot)).map(|p| p.nation) else {
        return false;
    };
    if !(0..6).contains(&nation) {
        return false;
    }
    let cell = rules.map.cell;
    let color = player_color(sim, i32::from(slot));
    for entry in rules
        .default_objects
        .supply
        .get(&(0, nation))
        .into_iter()
        .flatten()
    {
        let mut desc = ObjectDesc {
            flags: desc_flags::START_OBJECT,
            guid: entry.guid,
            player: i32::from(slot),
            position: [
                cell[0].wrapping_mul(entry.dx) as f32 + origin[0],
                origin[1],
                cell[2].wrapping_mul(entry.dz) as f32 + origin[2],
            ],
            scale: [1.0; 3],
            rotation: [0.0; 3],
            color,
        };
        create_object(sim, &mut desc);
    }
    true
}

/// `GameEngine_SnapStartObjectPosition` (`0x00434A00`) with `GameWorld_SnapBoxMinCornerToGrid`
/// (`0x0047A930`): the lower corner of the hull box (center - half extent) goes to a
/// tile boundary. z stays in the x87 register but rounds at 24-bit precision per step like `f32`
/// (like [`crate::placement::snap_box_min_corner_to_grid`]).
pub fn snap_start_object(rules: &Rules, guid: u32, v: &mut [f32; 3]) {
    let Some(b) = rules.model_boxes.get(&guid) else {
        return;
    };
    let (sx, sz) = (rules.map.cell[0] as f32, rules.map.cell[2] as f32);
    let min_x = b.center[0] - b.half[0];
    let min_z = b.center[2] - b.half[2];
    let ax = min_x + v[0];
    let az = min_z + v[2];
    let qx: f32 = ax / sx;
    let qz: f32 = az / sz;
    let fr_x: f32 = qx - qx.floor();
    let fr_z: f32 = qz - qz.floor();
    v[0] -= sx * fr_x;
    v[2] -= fr_z * sz;
}

/// `GameWorld_FindFreeFlagTile` (`0x004797B0`, caller `GameEngine_SnapStartObjectPosition`
/// `0x00434A00` only for the flag 4300; re/object-lifecycle 4, re/domination F.2/F.13, K-33, D-297):
/// If the tile `(tx, tz) = (trunc(x / SX), trunc(z / SZ))` of the target position in the occupancy grid
/// (`GameWorld+0x1FC0`) is "occupied", the function searches in rings `k = 1 … 19` (`lo = −k`,
/// `hi = k`) for the first free tile: pass A for `i = lo … hi` the tiles `(tx + i, tz + lo)`,
/// then `(tx + i, tz + hi)`; pass B for `j = lo … hi` `(tx + lo, tz + j)`, then
/// `(tx + hi, tz + j)`. Tiles outside the map count as free (grid `test`). Hit
/// `(a, b)`: `x = (tx + a) · SX + SX / 2`, `z = (tz + b) · SZ + SZ / 2` (integer, `/ 2` toward zero,
/// then `float`); y stays. Without a hit the counters after the last round apply,
/// `(a, b) = (19, 20)` (pass B runs up to `j = hi + 1`), the flag ends up there. If the target tile
/// is free, the position stays unchanged (no tile center).
pub fn find_free_flag_tile(state: &SimState, rules: &Rules, v: &mut [f32; 3]) {
    let (sx, sz) = (rules.map.cell[0], rules.map.cell[2]);
    // `FILD SX; FDIVR v.x; CALL __ftol`: quotient at 24-bit mantissa, truncated.
    let tx = (v[0] / sx as f32) as i32;
    let tz = (v[2] / sz as f32) as i32;
    let grid = &state.occupancy;
    if !grid.test(tx, tz) {
        return;
    }
    let free = |a: i32, b: i32| !grid.test(tx.wrapping_add(a), tz.wrapping_add(b));
    let mut found = None;
    'rings: for k in 1..=19 {
        let (lo, hi) = (-k, k);
        for i in lo..=hi {
            if free(i, lo) {
                found = Some((i, lo));
                break 'rings;
            }
            if free(i, hi) {
                found = Some((i, hi));
                break 'rings;
            }
        }
        for j in lo..=hi {
            if free(lo, j) {
                found = Some((lo, j));
                break 'rings;
            }
            if free(hi, j) {
                found = Some((hi, j));
                break 'rings;
            }
        }
    }
    // Without a hit: counters after ring 19, pass B (`a` = hi, `j` = hi + 1).
    let (a, b) = found.unwrap_or((19, 20));
    v[0] = (tx.wrapping_add(a).wrapping_mul(sx).wrapping_add(sx / 2)) as f32;
    v[2] = (tz.wrapping_add(b).wrapping_mul(sz).wrapping_add(sz / 2)) as f32;
}

/// Start package of a match.
pub fn start_package(rules: &Rules, setup: &MatchSetup) -> StartPackage {
    StartPackage {
        format: FORMAT_VERSION,
        level: rules.level_path.clone(),
        rules: rules.checksums,
        setup: setup.clone(),
        random_map: rules.random_map.as_ref().map(|r| r.spec.clone()),
    }
}

/// First object of a type on a slot (tests, tools).
pub fn find_object(state: &SimState, slot: u8, guid: u32) -> Option<ObjectId> {
    state.tables[usize::from(slot)]
        .objects
        .iter()
        .flatten()
        .find(|o| o.guid == guid)
        .map(|o| o.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::ModelBox;
    use opennml_formats::config::LevelPlayer;
    use opennml_formats::gifcon::PlaceObject;
    use opennml_formats::wld::WorldObject;
    use std::collections::BTreeMap;

    const TYPES: &str = "GUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\nGUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nGUID 3602\nNAME Baum\nVALUEID 100 6 1 256\nPROPID 4\n";

    // QUIRK E-40: kept
    #[test]
    fn ki_kind_maps_to_handicap() {
        let handicaps: Vec<_> = (0..=5).map(ki_kind_handicap).collect();
        assert_eq!(handicaps, [None, Some(5), Some(2), Some(0), Some(0), None]);
        assert_eq!(ki_kind_handicap(u8::MAX), None);
    }

    fn world_object(guid: u32, color: i32, position: [f32; 3]) -> WorldObject {
        WorldObject {
            guid,
            position,
            scale: [1.0; 3],
            rotation: [0.0; 3],
            color: Some(color),
            name: None,
            config: BTreeMap::new(),
        }
    }

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        r.level_path = "data/levels/multiplayer/test.lvl".into();
        for slot in [0, 2] {
            r.level.players.push(LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation: 2,
                kind: 0,
                resources: vec![(0, 700), (1, 800)],
            });
        }
        r.default_objects.start.insert(
            (0, 2),
            vec![
                PlaceObject {
                    guid: 1000,
                    dx: 0,
                    dz: 0,
                },
                PlaceObject {
                    guid: 1100,
                    dx: 5,
                    dz: 0,
                },
            ],
        );
        r.model_boxes.insert(
            1000,
            ModelBox {
                center: [10.0, 0.0, -20.0],
                half: [200.0, 50.0, 150.0],
            },
        );
        r.world_objects = vec![
            world_object(3602, 8, [100.0, 0.0, 100.0]),
            world_object(START_POINT, 0, [1000.0, 0.0, 1000.0]),
            world_object(START_POINT, 1, [3000.0, 0.0, 1000.0]),
            world_object(START_POINT, 2, [2000.0, 0.0, 3000.0]),
            world_object(4104, 8, [50.0, 0.0, 50.0]),
        ];
        r
    }

    /// `MatchSetup::network` ends up in `SimState::network`.
    #[test]
    fn network_flag_reaches_the_state() {
        let rules = rules();
        let mut setup = MatchSetup::default_for(&rules);
        assert!(!new_match(&rules, &setup).unwrap().network);
        setup.network = true;
        assert!(new_match(&rules, &setup).unwrap().network);
    }

    #[test]
    fn multiplayer_setup_fills_lobby_slots_and_places_start_objects() {
        let rules = rules();
        let setup = MatchSetup::default_for(&rules);
        assert_eq!((setup.mode, setup.lobby.len()), (0, 2));
        let state = new_match(&rules, &setup).unwrap();
        // Slot 1 has no [PLAYER]: empty, no start objects.
        assert_eq!(state.players[1].control, Control::Empty);
        assert_eq!(state.players[0].control, Control::Human);
        assert_eq!(state.resource(0, 0), 700);
        assert_eq!(state.tables[0].count, 2);
        assert_eq!(state.tables[1].count, 0);
        assert_eq!(state.tables[2].count, 2);
        assert_eq!(state.tables[9].count, 1);
        // Own teams: all against all; team 0 (slot 0) is a friend of the empty slots (quirk).
        assert!(state.diplomacy.is_enemy(0, 2) && !state.diplomacy.is_friend(0, 2));
        assert!(state.diplomacy.is_friend(0, 1) && state.diplomacy.is_friend(0, 5));
        assert!(!state.diplomacy.is_friend(2, 5));
        // HQ: lower hull box corner (-190, -170) + (1000, 1000) on the tile boundary.
        let hq = find_object(&state, 0, 1000).unwrap();
        let hq = state.object(hq, false).unwrap();
        assert_eq!(hq.world_pos[0] - 190.0, 768.0);
        assert_eq!(hq.world_pos[2] - 170.0, 768.0);
        // Colonist 5 tiles further in x, without snapping.
        let unit = state
            .object(find_object(&state, 0, 1100).unwrap(), false)
            .unwrap();
        assert_eq!(unit.world_pos, [1640.0, 0.0, 1000.0]);
        assert!(state.events.is_empty());
    }

    #[test]
    fn time_limit_is_minutes_times_60_in_every_mode() {
        // `GameEngine_ApplyGameSettings` (`0x00431160`): `m_TimeLimit = TimeLimit · 60`, without a
        // condition on the game type, also in the campaign; wrapped to 32 bits. The function
        // is called directly.
        let rules = rules();
        for mode in [-1, 0, 2, 4] {
            for (minutes, seconds) in [
                (7, 420),
                (20, 1200),
                (0, 0),
                (-1, -60),
                (i32::MAX, i32::MAX.wrapping_mul(60)),
            ] {
                let setup = MatchSetup {
                    mode,
                    time_limit: minutes,
                    ..MatchSetup::default()
                };
                let mut state = SimState::new(mode, 0);
                let mut sim = Sim::new(&mut state, &rules);
                apply_game_settings(&mut sim, &setup);
                assert_eq!(sim.state.time_limit, seconds, "mode {mode}, {minutes} min");
            }
        }
    }

    // QUIRK W-7: kept
    #[test]
    fn resource_and_living_space_settings_apply_in_multiplayer_only() {
        let mut rules = rules();
        rules.resource_config = opennml_formats::gifcon::parse_resource_config(
            "CONFIG_GUID 0\nCONFIG_VALUE0 2000\nCONFIG_VALUE1 500\nCONFIG_VALUE2 1500\nCONFIG_VALUE3 3000\n/CONFIG_GUID\nCONFIG_GUID 2\nCONFIG_VALUE2 1400\n/CONFIG_GUID\nGUID 3500\nCONFIG_GUID 26\nCONFIG_VALUE2 5000\n/GUID\n",
        );
        let mut setup = MatchSetup::default_for(&rules);
        assert_eq!((setup.resource_setting, setup.living_space), (0, 2));
        // Stage 0: resources of the .lvl, living space 125.
        let state = new_match(&rules, &setup).unwrap();
        assert_eq!((state.resource(0, 0), state.resource(0, 1)), (700, 800));
        assert_eq!(state.economy.max_living_space, 125);
        // Stage 2 "normal": gold and food from resourcenconfig.ini for all players, also the
        // empty slot 1; wood stays. Living space 4 -> 175; handicap 3 -> resource factor 70 %.
        setup.resource_setting = 2;
        setup.living_space = 4;
        setup.lobby[0].handicap = 3;
        let state = new_match(&rules, &setup).unwrap();
        for slot in [0, 1, 2] {
            assert_eq!(
                (state.resource(slot, 0), state.resource(slot, 2)),
                (1500, 1400),
                "slot {slot}"
            );
        }
        assert_eq!((state.resource(0, 1), state.resource(1, 1)), (800, 30000));
        assert_eq!(state.economy.max_living_space, 175);
        assert_eq!(state.economy.resource_factor(0), 70);
        assert_eq!(state.economy.resource_factor(2), 100);
        // Handicap affects attack and hit equally (C-121); slot 8 stays 100.
        assert_eq!(
            (state.combat.attack_factor[0], state.combat.hit_factor[0]),
            (70, 70)
        );
        assert_eq!(
            (state.combat.attack_factor[2], state.combat.hit_factor[2]),
            (100, 100)
        );
        assert_eq!(
            (state.combat.attack_factor[8], state.combat.hit_factor[8]),
            (100, 100)
        );
        // Campaign: neither resource nor living space setting (125).
        let campaign = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 3,
            living_space: 0,
            map_explored: false,
            ki: KiSetup::default(),
            script: ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let state = new_match(&rules, &campaign).unwrap();
        assert_eq!(
            (state.resource(0, 0), state.economy.max_living_space),
            (700, 125)
        );
    }

    #[test]
    fn campaign_keeps_the_level_players_without_start_objects() {
        let rules = rules();
        let campaign = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: KiSetup::default(),
            script: ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let state = new_match(&rules, &campaign).unwrap();
        // Campaign: players of the .lvl stay, no start objects (mode -1).
        assert_eq!(state.players[2].control, Control::Human);
        assert_eq!(state.tables[0].count, 0);
    }

    // QUIRK E-41: deviation
    #[test]
    fn new_match_copies_the_campaign_difficulty() {
        // `0x4E39` reads the difficulty in the tick; the simulation does not know the start package.
        use crate::state::ATTACK_REPORT_BUCKETS;
        let rules = rules();
        let mut setup = MatchSetup::default_for(&rules);
        assert_eq!(setup.ki, KiSetup::default());
        assert_eq!((setup.ki.enabled, setup.ki.campaign_difficulty), (false, 1));
        for v in [0, 1, 2] {
            setup.ki.campaign_difficulty = v;
            let state = new_match(&rules, &setup).unwrap();
            assert_eq!(state.campaign_difficulty, v);
            assert!(
                state
                    .attack_reports
                    .iter()
                    .all(|m| m.is_empty() && m.bucket_count() == ATTACK_REPORT_BUCKETS)
            );
        }
    }

    // QUIRK K-35: kept
    #[test]
    fn railroad_game_types_are_accepted() {
        let rules = rules();
        let mut setup = MatchSetup::default_for(&rules);
        // Game types 4 and 5 "Railroad" start like the others; their placement applies.
        for mode in [4, 5] {
            setup.mode = mode;
            assert!(new_match(&rules, &setup).is_ok(), "game type {mode}");
        }
    }

    /// Game type -2 (menu level) starts without a lobby.
    // QUIRK O-446: kept
    #[test]
    fn menu_level_mode_starts_without_lobby() {
        let rules = rules();
        let mut setup = MatchSetup::default_for(&rules);
        setup.mode = MENU_LEVEL_MODE;
        setup.lobby.clear();
        assert!(
            new_match(&rules, &setup).is_ok(),
            "-2 starts without a lobby"
        );
    }

    /// Game type 2 (domination) and lobby color 8 ("White") are free.
    // QUIRK D-298: kept
    #[test]
    fn domination_and_white_are_accepted() {
        let rules = rules();
        let mut setup = MatchSetup::default_for(&rules);
        setup.mode = 2;
        assert!(new_match(&rules, &setup).is_ok());
        setup.mode = 0;
        setup.lobby[0].color = 8;
        let state = new_match(&rules, &setup).unwrap();
        assert_eq!(state.players[0].color, 8);
    }

    // QUIRK K-35, K-277: kept
    #[test]
    fn mode_4_skips_objects_of_empty_slots() {
        // Colonists on slots 0 (occupied), 1 (no [PLAYER]: empty) and 2 (occupied), plus a
        // neutral tree (slot 8, not affected).
        let mut rules = rules();
        rules.world_objects = vec![
            world_object(1100, 0, [500.0, 0.0, 500.0]),
            world_object(1100, 1, [700.0, 0.0, 500.0]),
            world_object(1100, 2, [900.0, 0.0, 500.0]),
            world_object(3602, 8, [100.0, 0.0, 100.0]),
        ];
        let counts = |mode: i32| {
            let mut setup = MatchSetup::default_for(&rules);
            setup.mode = mode;
            let state = new_match(&rules, &setup).unwrap();
            assert_eq!(state.players[1].result, 3, "slot 1 empty");
            [0, 1, 2, 9].map(|t| state.tables[t].count)
        };
        // Railroad 1: occupied slots only (`Player_GetResultState ≠ 0` skips, 0x004431C2).
        assert_eq!(counts(4), [1, 0, 1, 1]);
        // The other game types (also Railroad 2) place everything.
        assert_eq!(counts(5), [1, 1, 1, 1]);
        assert_eq!(counts(0), [1, 1, 1, 1]);
    }

    // --- Snapping the start flag (`GameWorld_FindFreeFlagTile` 0x004797B0) ---

    // Without `BLOCKFFBRASTER` (0x1000): `PropertyNatur_Init` otherwise snaps the finished flag to the tile center.
    const FLAG_TYPE: &str = "GUID 4300
NAME Spieler_Fahne
VALUEID 100 6 1 1024
PROPID 5
VALUEID 60001 6 1 1
VALUEID 60002 6 1 1792
";

    /// State with an empty occupancy grid `dim × dim` and tile size 128 (`Rules::synthetic`).
    fn flag_world(dim: u32) -> (Rules, SimState) {
        let rules = Rules::synthetic("", dim);
        let mut state = SimState::new(2, 0);
        state.occupancy = crate::occupancy::OccupancyGrid::new(dim as i32, dim as i32);
        (rules, state)
    }

    // QUIRK D-297: kept
    #[test]
    fn free_flag_tile_keeps_the_position() {
        // Target tile free: no tile center, no change (not to y either).
        let (rules, state) = flag_world(32);
        let mut v = [7.0 * 128.0 + 10.5, 3.25, 15.0 * 128.0 + 99.75];
        let before = v;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, before);
    }

    // QUIRK D-297: kept
    #[test]
    fn occupied_flag_tile_takes_the_first_ring_tile() {
        let (rules, mut state) = flag_world(32);
        let start = [7.0 * 128.0 + 10.0, 3.25, 15.0 * 128.0 + 3.0];
        state.occupancy.set(7, 15, true);
        // Ring 1, pass A: first `(i, lo)` for i = -1: tile (6, 14); y stays.
        let mut v = start;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [6.0 * 128.0 + 64.0, 3.25, 14.0 * 128.0 + 64.0]);
        // (6, 14) occupied: then `(i, hi)` of the same i: tile (6, 16).
        state.occupancy.set(6, 14, true);
        let mut v = start;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [6.0 * 128.0 + 64.0, 3.25, 16.0 * 128.0 + 64.0]);
        // Whole rows lo and hi of ring 1 occupied: pass B, `(lo, j)` for j = -1 (occupied),
        // then (6, 16) occupied again; j = 0: tile (6, 15).
        for i in 6..=8 {
            state.occupancy.set(i, 14, true);
            state.occupancy.set(i, 16, true);
        }
        let mut v = start;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [6.0 * 128.0 + 64.0, 3.25, 15.0 * 128.0 + 64.0]);
        // (6, 15) occupied: `(hi, j)` with j = -1 and 1 are occupied, j = 0: tile (8, 15).
        state.occupancy.set(6, 15, true);
        let mut v = start;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [8.0 * 128.0 + 64.0, 3.25, 15.0 * 128.0 + 64.0]);
        // Ring 1 full: ring 2 begins in pass A at `(i, lo)` with i = -2: tile (5, 13).
        state.occupancy.set(8, 15, true);
        let mut v = start;
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [5.0 * 128.0 + 64.0, 3.25, 13.0 * 128.0 + 64.0]);
    }

    // QUIRK K-33: kept
    #[test]
    fn flag_tile_outside_the_map_counts_as_free() {
        // Tile (0, 0) occupied, ring 1 begins at (-1, -1) outside: free (K-33); the position then lies
        // outside the map, `(tx + a) · SX + SX / 2` with a negative part.
        let (rules, mut state) = flag_world(32);
        state.occupancy.set(0, 0, true);
        let mut v = [20.0, 1.0, 30.0];
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [-64.0, 1.0, -64.0]);
        // At the right edge: (31, 5) occupied; (30, 4), (30, 6) occupied, the rows lo and hi in ring 1
        // are occupied inside, `(hi, j)` lies outside with x = 32 - but `(lo, j)` with j = -1
        // comes first and is occupied, j = 0: (30, 5) occupied, then (32, 5) outside.
        let (rules, mut state) = flag_world(32);
        for x in 30..32 {
            for z in 4..=6 {
                state.occupancy.set(x, z, true);
            }
        }
        let mut v = [31.0 * 128.0 + 1.0, 0.0, 5.0 * 128.0 + 1.0];
        find_free_flag_tile(&state, &rules, &mut v);
        // Pass A: i = -1: (30, 4), (30, 6) occupied; i = 0: (31, 4), (31, 6) occupied; i = 1:
        // (32, 4) outside = free.
        assert_eq!(v, [32.0 * 128.0 + 64.0, 0.0, 4.0 * 128.0 + 64.0]);
        // Negative target tile (position in front of the map): truncation toward zero, tile (0, 0) -> free.
        let (rules, state) = flag_world(32);
        let mut v = [-5.0, 0.0, -5.0];
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [-5.0, 0.0, -5.0]);
    }

    // QUIRK K-33: kept
    #[test]
    fn no_free_tile_keeps_the_last_counters() {
        // Everything occupied (K-33): after ring 19 the counters stand at (a, b) =
        // (19, 20), the position is the center of the tile (tx + 19, tz + 20).
        let (rules, mut state) = flag_world(64);
        state.occupancy.cells.fill(1);
        let mut v = [30.0 * 128.0 + 5.0, 2.0, 30.0 * 128.0 + 6.0];
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [49.0 * 128.0 + 64.0, 2.0, 50.0 * 128.0 + 64.0]);
        // If only tile (49, 49) is unoccupied - ring 19, i = 19, `(i, hi)` -, the search hits it.
        state.occupancy.set(30 + 19, 30 + 19, false);
        let mut v = [30.0 * 128.0 + 5.0, 2.0, 30.0 * 128.0 + 6.0];
        find_free_flag_tile(&state, &rules, &mut v);
        assert_eq!(v, [49.0 * 128.0 + 64.0, 2.0, 49.0 * 128.0 + 64.0]);
    }

    // QUIRK D-297: kept
    #[test]
    fn start_flag_is_snapped_by_the_ring_search_not_the_box() {
        // `place_start_objects`: the flag 4300 goes to `find_free_flag_tile`; free target tile ->
        // position unchanged, occupied -> center of the ring tile. A hull box of type 4300 would
        // not apply.
        let mut rules = Rules::synthetic(FLAG_TYPE, 32);
        rules.default_objects.start.insert(
            (2, 2),
            vec![PlaceObject {
                guid: PLAYER_FLAG,
                dx: 3,
                dz: 0,
            }],
        );
        rules.model_boxes.insert(
            PLAYER_FLAG,
            ModelBox {
                center: [10.0, 0.0, -20.0],
                half: [200.0, 50.0, 150.0],
            },
        );
        let origin = [5.0 * 128.0 + 20.0, 0.0, 9.0 * 128.0 + 30.0];
        let flag_pos = |occupied: &[(i32, i32)]| {
            let mut state = SimState::new(2, 0);
            state.occupancy = crate::occupancy::OccupancyGrid::new(32, 32);
            for &(x, z) in occupied {
                state.occupancy.set(x, z, true);
            }
            state.players[0].nation = 2;
            let mut sim = Sim::new(&mut state, &rules);
            place_start_objects(&mut sim, 0, origin);
            let id = find_object(&state, 8, PLAYER_FLAG).expect("flag on slot 8");
            let o = state.object(id, false).unwrap();
            [o.world_pos[0], o.world_pos[2]]
        };
        // Target tile (8, 9) free: position = start point + 3 * 128 in x, without snapping.
        assert_eq!(flag_pos(&[]), [origin[0] + 384.0, origin[2]]);
        // Occupied: center of (7, 8).
        assert_eq!(
            flag_pos(&[(8, 9)]),
            [7.0 * 128.0 + 64.0, 8.0 * 128.0 + 64.0]
        );
    }

    /// Corpus (K-33, D-297): the start flags of the game type 2 lists on
    /// `[2players]-battlebeach`. The flag stands 3 tiles right of the start point (`PLACEOBJECT 4300 3 0`),
    /// the main building sets its lock margin beforehand. If the target tile lies in it (nations 2 to 5 with
    /// a wider main building), the flag goes to the first free ring tile, otherwise it stays.
    // QUIRK D-297: kept
    #[test]
    fn corpus_battle_beach_start_flags() {
        use opennml_rda::Vfs;
        use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
        use opennml_world::level::load_level;
        let Some(dir) = corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let version = game_version(&vfs).unwrap();
        let scene = load_level(&vfs, "data/levels/multiplayer/[2players]-battlebeach.lvl").unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let cell = rules.map.cell;
        let tile_of = |x: f32, z: f32| ((x / cell[0] as f32) as i32, (z / cell[2] as f32) as i32);
        let starts: Vec<_> = rules
            .world_objects
            .iter()
            .filter(|o| o.guid == START_POINT)
            .map(|o| o.position)
            .collect();
        assert_eq!(starts.len(), 2);
        // Position of the flag tiles per nation (slot 0, slot 1) after snapping.
        let mut found = Vec::new();
        for nation in 0..6 {
            let list = rules.default_objects.start.get(&(2, nation)).unwrap();
            assert_eq!(list.iter().filter(|e| e.guid == PLAYER_FLAG).count(), 1);
            let mut setup = MatchSetup::default_for(&rules);
            setup.mode = 2;
            for e in &mut setup.lobby {
                e.nation = nation;
            }
            // Build like `new_match` in mode 2.
            let build = |rules: &Rules| {
                let mut state = SimState::new(2, 0);
                state.init_map(rules);
                let mut sim = Sim::new(&mut state, rules);
                apply_game_settings(&mut sim, &setup);
                setup_players(&mut sim, &setup).unwrap();
                apply_player_handicaps(&mut sim);
                place_level_objects(&mut sim, &setup);
                state
            };
            let state = build(&rules);
            // The same list without a flag: the occupancy grid that `FindFreeFlagTile` found.
            let mut without_flag = rules.clone();
            without_flag
                .default_objects
                .start
                .get_mut(&(2, nation))
                .unwrap()
                .retain(|e| e.guid != PLAYER_FLAG);
            let before = build(&without_flag);
            let mut tiles: Vec<_> = state
                .objects()
                .filter(|o| o.guid == PLAYER_FLAG)
                .map(|o| tile_of(o.world_pos[0], o.world_pos[2]))
                .collect();
            tiles.sort_unstable();
            assert_eq!(tiles.len(), 2, "nation {nation}");
            let mut wanted: Vec<_> = starts
                .iter()
                .map(|p| {
                    let e = list.iter().find(|e| e.guid == PLAYER_FLAG).unwrap();
                    tile_of(
                        p[0] + cell[0].wrapping_mul(e.dx) as f32,
                        p[2] + cell[2].wrapping_mul(e.dz) as f32,
                    )
                })
                .collect();
            wanted.sort_unstable();
            for (tile, wanted) in tiles.iter().zip(&wanted) {
                // Target tile free: the flag stays; occupied: a free tile of the ring.
                let occupied = before.occupancy.test(wanted.0, wanted.1);
                assert_eq!(*tile != *wanted, occupied, "nation {nation}: {wanted:?}");
                assert!(!before.occupancy.test(tile.0, tile.1), "nation {nation}");
                assert!(
                    (tile.0 - wanted.0).abs() <= 19 && (tile.1 - wanted.1).abs() <= 19,
                    "nation {nation}"
                );
            }
            found.push(tiles);
        }
        // Measured on the German edition with patch: nations 0 and 1 keep the target tile
        // (14, 14) and (52, 51); nations 2, 4 and 5 move to (+1, -1), nation 3 to (+2, -2).
        // The English edition was not measured (no installation), only the properties
        // above apply there.
        if version == GameVersion::German {
            let unchanged = vec![(14, 14), (52, 51)];
            let ring1 = vec![(15, 13), (53, 50)];
            let ring2 = vec![(16, 12), (54, 49)];
            assert_eq!(
                found,
                [
                    unchanged.clone(),
                    unchanged,
                    ring1.clone(),
                    ring2,
                    ring1.clone(),
                    ring1
                ]
            );
        }
    }

    // QUIRK E-147: kept
    #[test]
    fn wld_names_are_registered_untrimmed() {
        // `OBJ_NAME` comes from the `.wld` already in lowercase; spaces stay (E-147).
        let mut rules = rules();
        let mut named = world_object(1100, 0, [500.0, 0.0, 500.0]);
        named.name = Some("handelshaus ".into());
        let mut empty = world_object(1100, 2, [700.0, 0.0, 500.0]);
        empty.name = Some(String::new());
        rules.world_objects = vec![named, empty, world_object(1100, 2, [900.0, 0.0, 500.0])];
        let state = new_match(&rules, &MatchSetup::default_for(&rules)).unwrap();
        let id = find_object(&state, 0, 1100).unwrap();
        assert_eq!(
            state.names.get("handelshaus "),
            Some(&*id.as_i32().to_string())
        );
        assert_eq!(state.names.get("handelshaus"), None);
        assert_eq!(state.names.len(), 1);
    }

    #[test]
    fn mode_4_skips_nothing_without_a_slot_color() {
        // No `OBJ_COLOR`: slot 8 (no player), placed in mode 4 as well.
        let mut rules = rules();
        let mut o = world_object(1100, 0, [500.0, 0.0, 500.0]);
        o.color = None;
        rules.world_objects = vec![o];
        let mut setup = MatchSetup::default_for(&rules);
        setup.mode = 4;
        let state = new_match(&rules, &setup).unwrap();
        assert_eq!(state.tables[8].count, 1);
    }

    #[test]
    fn maps_with_sides_not_divisible_by_16_are_rejected() {
        let odd = Rules::synthetic("", 20);
        let campaign = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: KiSetup::default(),
            script: ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let err = new_match(&odd, &campaign).unwrap_err();
        assert!(err.contains("20 x 20") && err.contains("16"), "{err}");
        assert!(new_match(&Rules::synthetic("", 16), &campaign).is_ok());
    }

    // --- Random map: match start ---

    /// Rules like [`rules`] as a random map: exactly `slots` active (players without names, nation 0, without
    /// resources), game mode `game_type`, CRT start value 77.
    fn random_rules(slots: &[u8], game_type: usize) -> Rules {
        use crate::random_map::{RandomMapRules, RandomMapSpec};
        let mut r = rules();
        r.level_path = "randommap:T".into();
        r.level.players = slots
            .iter()
            .map(|&slot| LevelPlayer {
                slot,
                name: String::new(),
                nation: 0,
                kind: 0,
                resources: Vec::new(),
            })
            .collect();
        let mut active = [false; 8];
        for &s in slots {
            active[usize::from(s)] = true;
        }
        r.random_map = Some(RandomMapRules {
            spec: RandomMapSpec {
                seed_text: "T".into(),
                size: 0,
                players: slots.len() as i32,
                terrain: 0,
                landscape: 0,
                daytime: 0,
                game_type,
            },
            slots: active,
            island: false,
            crt_start: 77,
            digest: 0,
        });
        r
    }

    fn lobby_entry(kind: SlotKind, name: &str, team: i32) -> LobbyEntry {
        LobbyEntry {
            kind,
            name: name.into(),
            nation: 2,
            team,
            color: team,
            handicap: 0,
        }
    }

    fn free_play(mode: i32, lobby: Vec<LobbyEntry>) -> MatchSetup {
        MatchSetup {
            mode,
            lobby,
            ..MatchSetup::default()
        }
    }

    #[test]
    fn random_map_mode_must_match_the_spec() {
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        let rules = random_rules(&[0, 2], 1);
        assert!(new_match(&rules, &free_play(1, lobby.clone())).is_ok());
        for mode in [0, 2, 3] {
            let err = new_match(&rules, &free_play(mode, lobby.clone())).unwrap_err();
            assert!(
                err.starts_with("random map: game mode") && err.contains("map description"),
                "{err}"
            );
        }
        // Game modes above 3 do not exist on random maps (GTYPE 0 ... 3).
        let rules = random_rules(&[0, 2], 4);
        let err = new_match(&rules, &free_play(4, lobby)).unwrap_err();
        assert!(err.starts_with("random map: game mode 4"), "{err}");
    }

    // QUIRK D-339: kept
    #[test]
    fn lobby_wraps_and_overwrites_front_slots() {
        // `GameEngine_SetupPlayersFromLobby` `0x00432E09`: slots 0 and 2 active, three entries - AI "A"
        // -> 0, AI "B" -> 2, human "C" starts at the front again and overwrites slot 0 (D-339). Applies to all
        // maps; fixed maps reach it only with more entries than `.lvl` slots.
        let lobby = vec![
            lobby_entry(SlotKind::Ai, "A", 1),
            lobby_entry(SlotKind::Ai, "B", 2),
            lobby_entry(SlotKind::Human, "C", 3),
        ];
        for rules in [random_rules(&[0, 2], 0), rules()] {
            let state = new_match(&rules, &free_play(0, lobby.clone())).unwrap();
            let p = &state.players;
            assert_eq!(
                (p[0].control, p[0].name.as_str(), p[0].team),
                (Control::Human, "C", 3)
            );
            assert_eq!(
                (p[2].control, p[2].name.as_str(), p[2].team),
                (Control::Ai, "B", 2)
            );
            assert_eq!(p[1].control, Control::Empty);
            // Start objects on both occupied slots, none for the overwritten entry "A".
            assert_eq!((state.tables[0].count, state.tables[2].count), (2, 2));
        }
    }

    // QUIRK D-339: kept
    #[test]
    fn human_losing_the_slot_aborts_the_start() {
        // Human "A" -> 0, AI "B" -> 2, AI "C" overwrites slot 0: the original would have no local
        // player anymore (D-339, hypothesis: memory error); the reimplementation aborts with a message.
        let lobby = vec![
            lobby_entry(SlotKind::Human, "A", 0),
            lobby_entry(SlotKind::Ai, "B", 1),
            lobby_entry(SlotKind::Ai, "C", 2),
        ];
        let rules = random_rules(&[0, 2], 0);
        assert_eq!(
            new_match(&rules, &free_play(0, lobby.clone())).unwrap_err(),
            "random map: too few start slots, the own slot was lost"
        );
        // With a third active slot it is enough.
        let rules = random_rules(&[0, 1, 2], 0);
        let state = new_match(&rules, &free_play(0, lobby)).unwrap();
        assert_eq!(state.players[0].control, Control::Human);
        assert_eq!(state.players[1].name, "B");
    }

    // QUIRK D-339: kept
    #[test]
    fn no_active_slot_returns_without_start_resources() {
        // No active slot: after more than 8 rounds the original returns without start resources
        // and diplomacy (`0x00432E14` -> `0x00432F94`).
        let config = "CONFIG_GUID 0\nCONFIG_VALUE0 2000\nCONFIG_VALUE1 500\nCONFIG_VALUE2 1500\nCONFIG_VALUE3 3000\n/CONFIG_GUID\n";
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        let mut rules = random_rules(&[], 0);
        rules.resource_config = opennml_formats::gifcon::parse_resource_config(config);
        let setup = MatchSetup {
            resource_setting: 2,
            ..free_play(0, lobby.clone())
        };
        let state = new_match(&rules, &setup).unwrap();
        let fresh = SimState::new(0, 0);
        assert_eq!(state.resource(0, 0), fresh.resource(0, 0));
        assert!(state.players.iter().all(|p| p.control == Control::Empty));
        // Counter-check with an active slot: start resources of stage 2.
        let mut rules = random_rules(&[0], 0);
        rules.resource_config = opennml_formats::gifcon::parse_resource_config(config);
        let state = new_match(&rules, &setup).unwrap();
        assert_eq!(state.resource(0, 0), 1500);
    }

    // QUIRK W-6: kept
    #[test]
    fn random_map_start_resources_and_deposits_use_level_zero() {
        // W-6: on random maps stage 0 (`values[0]`) also applies to the start resources of all slots,
        // and every object gets an empty configuration beforehand with `[config_guid] = values[stage]`
        // (`GameEngine_ApplyResourceConfigToObject` `0x004349A0`).
        let mut rules = random_rules(&[0, 2], 0);
        rules.resource_config = opennml_formats::gifcon::parse_resource_config(
            "CONFIG_GUID 0\nCONFIG_VALUE0 2000\nCONFIG_VALUE1 500\nCONFIG_VALUE2 1500\nCONFIG_VALUE3 3000\n/CONFIG_GUID\nGUID 3500\nCONFIG_GUID 26\nCONFIG_VALUE0 2000\nCONFIG_VALUE1 3000\nCONFIG_VALUE2 5000\nCONFIG_VALUE3 8000\n/GUID\n",
        );
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        rules
            .world_objects
            .push(world_object(3500, 8, [600.0, 0.0, 600.0]));
        for (stage, gold, deposit) in [(0, 2000, 2000), (2, 1500, 5000), (3, 3000, 8000)] {
            let setup = MatchSetup {
                resource_setting: stage,
                ..free_play(0, lobby.clone())
            };
            let state = new_match(&rules, &setup).unwrap();
            for slot in [0, 1, 2, 7] {
                assert_eq!(state.resource(slot, 0), gold, "stage {stage}, slot {slot}");
            }
            // The last object (3500, without a type in these rules) leaves its configuration behind.
            assert_eq!(
                state.object_config,
                BTreeMap::from([(26, deposit)]),
                "stage {stage}"
            );
        }
        // A start point clears the configuration (no record for 4200): the start objects
        // arise without the value of the occurrence before.
        rules.world_objects = vec![
            world_object(3500, 8, [600.0, 0.0, 600.0]),
            world_object(START_POINT, 0, [1000.0, 0.0, 1000.0]),
        ];
        let state = new_match(&rules, &free_play(0, lobby.clone())).unwrap();
        assert!(state.object_config.is_empty());
        assert_eq!(state.tables[0].count, 2);
        // Fixed map: stage 0 leaves the resources of the `.lvl` (unchanged).
        let mut fixed = self::rules();
        fixed.resource_config = rules.resource_config.clone();
        let state = new_match(&fixed, &MatchSetup::default_for(&fixed)).unwrap();
        assert_eq!(state.resource(0, 0), 700);
    }

    #[test]
    fn neutral_owner_objects_have_player_8() {
        // Flags `0x8D3` set the slot flag also for owner 8 - neutral stays slot 8;
        // an owner 9/10 without a category ends up on slot 0 because of the flag (`create_object`).
        let mut rules = random_rules(&[0, 2], 0);
        rules.world_objects = vec![
            world_object(1100, 8, [500.0, 0.0, 500.0]),
            world_object(1100, 9, [700.0, 0.0, 500.0]),
        ];
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        let state = new_match(&rules, &free_play(0, lobby)).unwrap();
        assert_eq!(
            [0, 8, 9].map(|t| state.tables[t].count),
            [1, 1, 0],
            "random map"
        );
        // Fixed map without the slot flag for 8 ... 10: owner 9 stays 9.
        let mut fixed = self::rules();
        fixed.world_objects = rules.world_objects.clone();
        let state = new_match(&fixed, &MatchSetup::default_for(&fixed)).unwrap();
        assert_eq!([0, 8, 9].map(|t| state.tables[t].count), [0, 1, 1]);
    }

    #[test]
    fn random_objects_use_flags_0x8d3() {
        // `FUN_005474B0`: `READY | COLOR | PLAYER | ROT_Y | POSITION | GUID`, position with y = 0 exactly (the
        // height is only read afterwards from the terrain by `GameObject_SnapHeightToTerrain`), color of the slot.
        assert_eq!(RANDOM_OBJECT_FLAGS, 0x8D3);
        let rules = random_rules(&[0, 2], 0);
        let mut o = world_object(1100, 2, [500.0, 123.5, 600.0]);
        o.rotation = [0.25, 0.5, 0.75];
        o.scale = [2.0; 3];
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        state.players[2].color = 5;
        let sim = Sim::new(&mut state, &rules);
        let d = random_object_desc(&sim, &o);
        assert_eq!(d.flags, 0x8D3);
        assert_eq!((d.guid, d.player, d.color), (1100, 2, 5));
        assert_eq!(
            d.position.map(f32::to_bits),
            [500.0f32, 0.0, 600.0].map(f32::to_bits)
        );
        assert_eq!(d.rotation, [0.25, 0.5, 0.75]);
        // Without a color neutral (8).
        o.color = None;
        assert_eq!(random_object_desc(&sim, &o).player, 8);
        // In the world only the rotation about the vertical axis, scale 1.
        let mut rules = random_rules(&[0, 2], 0);
        let mut o = world_object(1100, 2, [500.0, 123.5, 600.0]);
        o.rotation = [0.25, 0.5, 0.75];
        o.scale = [2.0; 3];
        rules.world_objects = vec![o];
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        let state = new_match(&rules, &free_play(0, lobby)).unwrap();
        let obj = state
            .object(find_object(&state, 2, 1100).unwrap(), false)
            .unwrap();
        assert_eq!(obj.rotation, [0.0, 0.5, 0.0]);
        assert_eq!(obj.scale, [1.0; 3]);
        assert_eq!(obj.world_pos[1], 0.0, "flat terrain");
    }

    // QUIRK D-342: kept
    #[test]
    fn random_map_crt_starts_at_the_generator_end() {
        // D-342: `SimState.crt` starts at the final state of the generator; the one tree of the
        // rules draws one value.
        let rules = random_rules(&[0, 2], 0);
        let lobby = vec![lobby_entry(SlotKind::Human, "A", 0)];
        let state = new_match(&rules, &free_play(0, lobby)).unwrap();
        assert_eq!(state.tables[9].count, 1);
        let mut expected = crate::rng::CrtRand::new(77);
        expected.rand();
        assert_eq!(state.crt, expected);
        // Fixed map: start 1 as before.
        let fixed = self::rules();
        let state = new_match(&fixed, &MatchSetup::default_for(&fixed)).unwrap();
        let mut expected = crate::rng::CrtRand::new(1);
        expected.rand();
        assert_eq!(state.crt, expected);
    }
}
