//! Start request of the menu UI -> setup of the simulation: [`to_match_setup`] converts a [`MatchRequest`] (command
//! `0x0D`, `opennml_ui::draft`) into a [`MatchSetup`], as `Game_StartLevel` reads it from the session with
//! `GameEngine_ApplyGameSettings` (`0x00431160`) and `GameEngine_SetupPlayersFromLobby` (`0x00432C00`);
//! [`campaign_setup`] sets up a campaign level (command `0x0C`); [`random_map_spec`] turns the menu values of a random
//! map into the map description. See re/singleplayer-menus part 1, 6.2 and part 2, 4.3 and 5; re/object-lifecycle 5;
//! re/random-map Z.2.

use opennml_formats::config::lower_german;
use opennml_sim::ki::KiSetup;
use opennml_sim::random_map::RandomMapSpec;
use opennml_sim::rules::Rules;
use opennml_sim::script::ScriptSetup;
use opennml_sim::setup::{LobbyEntry, MatchSetup, SlotKind};
use opennml_ui::assets::Texts;
use opennml_ui::draft::{HUMAN, MatchRequest, SlotDraft, text_of};

/// Game type "Destroy/defend railroad": slot = team.
const RAILROAD: i32 = 4;
/// Text 15195 "Computer" (`0x3B5B`) and the first color 15150 "Yellow" (`0x3B2E`) of the computer names.
const COMPUTER_TEXT: i32 = 15195;
const COLOR_TEXT: i32 = 15150;

/// Start request -> setup:
///
/// - Mode = game type (`NS+0x1F4` -> `Engine+0x108`), seed 0 (the original starts the random sequence at 0),
///   resources = `Resourcen` (`NS+0x200`), living space = `UnitLimit` (`NS+0x1EC`, index 0 ... 5 = 75 ... 200),
///   `MapExplored` (`NS+0x1F0`), time limit = `TimeLimit` (`NS+0x204`, minutes); the AI starts if at least one
///   computer slot is occupied.
/// - Computer names ([`name_computers`]): `SetupPlayersFromLobby` (`0x00432C00`, beginning) names slots of type 1...4
///   "Computer (color)" - but only the keys -1 ... n - 2 with n = min(slot count, active `.lvl` slots) (O-192).
/// - Lobby in key order (`for li in -1..=6` in `SetupPlayersFromLobby`): type 999 -> human, 1...4 -> AI (control
///   kind 2), otherwise closed; handicap = slot value 0 ... 5 (`Player_SetHandicap`).
/// - Game type 4 (K-277): each entry lands on the slot of its team (`GetPlayer(team)`), a later one overwrites an
///   earlier one. `opennml-sim` puts the lobby in order on the active `.lvl` slots; the conversion therefore sorts them
///   by slot and fills active slots without an entry with `Closed` entries that leave the values of the player
///   unchanged (name and nation from the `.lvl`, team 0, color = slot, handicap 0 as in `Player_Construct`). The
///   conversion rejects a team without a `.lvl` slot.
///
/// Errors (empty map path - draft without a map, O-188 -, team without a slot in game type 4) are returned as an
/// error message; the values themselves are not checked, `new_match` tolerates any. For a random map
/// ([`random_map_spec`]) the map path does not apply (`Game_StartLevel` does not read it for `LevelType 1`), so an
/// empty one is not an error.
pub fn to_match_setup(
    req: &MatchRequest,
    rules: &Rules,
    texts: &Texts,
) -> Result<MatchSetup, String> {
    if req.level.is_empty() && random_map_spec(req).is_none() {
        return Err("Match cannot be started: no map selected (empty map path)".to_string());
    }
    let lobby: Vec<LobbyEntry> = lobby_order(req, rules, texts)?
        .into_iter()
        .map(|(_, entry)| entry)
        .collect();
    let setup = MatchSetup {
        mode: req.game_type,
        seed: 0,
        resource_setting: req.settings.resources,
        living_space: req.settings.unit_limit,
        map_explored: req.settings.map_explored,
        ki: KiSetup {
            enabled: lobby.iter().any(|e| e.kind == SlotKind::Ai),
            ..KiSetup::default()
        },
        // Level scripts are not enabled here (only the viewer covers them).
        script: ScriptSetup::default(),
        lobby,
        // `TimeLimit` in minutes (`NS+0x204`); `GameEngine_ApplyGameSettings` converts it to seconds.
        time_limit: req.settings.time_limit,
        // The profile name is entered separately (`FUN_00403F50`).
        ..MatchSetup::default()
    };
    Ok(setup)
}

/// Network match from the lobby: [`to_match_setup`] with the network flag and the seed `seed` of the host (deviating
/// from E-262: the program draws it from the clock, tests use a fixed one). The slots of the rows come from
/// [`row_slots`], computed the same way.
pub fn to_net_setup(
    req: &MatchRequest,
    rules: &Rules,
    texts: &Texts,
    seed: u32,
) -> Result<MatchSetup, String> {
    let mut setup = to_match_setup(req, rules, texts)?;
    setup.network = true;
    setup.seed = seed;
    Ok(setup)
}

/// Slot in the simulation per lobby row 0 ... 7: from the same order as the lobby of the setup ([`lobby_order`]).
/// `GameEngine_SetupPlayersFromLobby` puts the `i`-th entry on the `i`-th active `.lvl` slot; `Game_StartLevel` has
/// removed the open rows beforehand (`FUN_004E9800`), so the remaining ones move up (compaction), and in game type 4
/// every entry is on the slot of its team (a later entry in the same team displaces the earlier one, which then has
/// no slot). Row = key + 1; `None`: no slot (open, displaced or more entries than active slots).
pub fn row_slots(
    req: &MatchRequest,
    rules: &Rules,
    texts: &Texts,
) -> Result<[Option<u8>; 8], String> {
    let active = active_slots(rules);
    let mut rows = [None; 8];
    for (i, (key, _)) in lobby_order(req, rules, texts)?.into_iter().enumerate() {
        let Some(row) = key
            .and_then(|k| usize::try_from(k + 1).ok())
            .filter(|&r| r < rows.len())
        else {
            continue;
        };
        rows[row] = active.get(i).copied();
    }
    Ok(rows)
}

/// The lobby of the setup in slot order, each entry with the key of the slot in the draft (`None` for the fillers of
/// game type 4): the shared calculation of [`to_match_setup`] and [`row_slots`]. Slots ordered by key, computers
/// named ([`name_computers`]), in game type 4 by team ([`railroad_lobby`]).
fn lobby_order(
    req: &MatchRequest,
    rules: &Rules,
    texts: &Texts,
) -> Result<Vec<(Option<i32>, LobbyEntry)>, String> {
    let mut slots = req.slots.clone();
    slots.sort_by_key(|s| s.key);
    name_computers(&mut slots, active_slots(rules).len(), texts);
    if req.game_type == RAILROAD {
        railroad_lobby(&slots, rules)
    } else {
        Ok(slots
            .iter()
            .map(|s| (Some(s.key), lobby_entry(s)))
            .collect())
    }
}

/// Setup of a campaign level (command `0x0C`, `FUN_004C8EC0(path, -1)`, re/singleplayer-menus part 1, 6.2):
///
/// - Game type -1, no lobby: the slots come from the level header (`FUN_004BDDA0`), in `opennml-sim` directly from the
///   `.lvl` (`setup_players` for mode < 0); "Map explored" off (`+0x1F0 = 0`), seed 0. Living space is fixed at 125
///   in the campaign (`GameEngine_ApplyGameSettings` `0x00431160`, branch game type < 0), `UnitLimit` (`+0x1EC = 1`)
///   has no effect.
/// - `0x0C` does not reset `Resourcen`/`TimeLimit` (O-142); both stay in the draft but have no effect in the
///   campaign: only the lobby setup (`GameEngine_ApplyResourceConfigStartResources`) and the random map
///   (`GameEngine_ApplyResourceConfigToObject`, called by `World_PlaceObject` only with the flag set;
///   `World_ParseNextBlock` `0x004433B9` passes 0) read the resource setting, only game type 4 reads the time limit.
///   The setup therefore does not read the draft and takes the defaults.
/// - Difficulty `difficulty` (`GameApp+0x19B4` -> `Engine+0x5F8`, `0x004311C2`).
/// - Level script and AI run (`ScriptSetup { enabled, time_per_char }` with `TimePerChar` from `Engine.ini`,
///   `ki.enabled`) as in the scene of `SCRIPT_GOLDEN`; a script that fails the load check is switched off by the
///   start (`sim_host::script_fallback`). A level ends through the script actions 4/13 (command path
///   `script::command::player_result`), through giving up or `sim_result`.
/// - `local_name`: profile name `NetName` (`EngineConfig+0x13C`) for the first human slot (`Game_SelectLocalPlayer`
///   `0x004BB3B0` -> `FUN_00403F50`, only for game type < 0).
/// - `profile_vars`: the profile variables (`m_VarMap`) of the active profile for 383. Only names that are already
///   lowercase in the German sense (`lower_german(name) == name`) are taken over: 383 writes the name it looks up in
///   lowercase and compares case-sensitively (`FUN_0040C780` -> `_mbscmp`, E-252); other entries (only by hand in
///   `profiles.ini`) are never seen by the original. If a name appears twice in the file, the later one applies.
///
/// The values are not checked, `new_match` tolerates any.
pub fn campaign_setup(
    difficulty: i32,
    time_per_char: i32,
    local_name: Option<String>,
    profile_vars: &[(String, i32)],
) -> MatchSetup {
    level_setup(-1, difficulty, time_per_char, local_name, profile_vars)
}

/// Setup of the menu level behind the main menu (command `0x21`, `FUN_004C9140` `0x004C9140`): like
/// [`campaign_setup`], but game type -2 ([`opennml_sim::setup::MENU_LEVEL_MODE`]). In the original the difficulty remains
/// a leftover of the previous start and `menü2.abl` starts no AI modules; the reimplementation takes 0. No lobby,
/// `map_explored` off, no entry in the network session or lobby (deviating from O-436); the time limit only applies
/// to game type 4. `local_name` is `NetName` (`Game_SelectLocalPlayer` for game type < 0).
pub fn menu_level_setup(
    time_per_char: i32,
    local_name: Option<String>,
    profile_vars: &[(String, i32)],
) -> MatchSetup {
    level_setup(
        opennml_sim::setup::MENU_LEVEL_MODE,
        0,
        time_per_char,
        local_name,
        profile_vars,
    )
}

/// Shared part of [`campaign_setup`] and [`menu_level_setup`]: level with fixed slots from the `.lvl`.
fn level_setup(
    mode: i32,
    difficulty: i32,
    time_per_char: i32,
    local_name: Option<String>,
    profile_vars: &[(String, i32)],
) -> MatchSetup {
    MatchSetup {
        mode,
        seed: 0,
        lobby: Vec::new(),
        map_explored: false,
        ki: KiSetup {
            enabled: true,
            campaign_difficulty: difficulty,
        },
        script: ScriptSetup {
            enabled: true,
            time_per_char,
            profile_vars: profile_vars
                .iter()
                .filter(|(name, _)| lower_german(name) == *name)
                .cloned()
                .collect(),
        },
        local_name,
        ..MatchSetup::default()
    }
}

/// Map description of a random map from the start request (`Game_StartLevel` `0x004C877C`, branch `LevelType 1` ->
/// `Game_StartRandomMapFromSeedText` `0x004BF3E0` with `Info`): only for `Info.LevelType == 1`. Seed text =
/// `Info.Name` (`CREATE2` has already replaced an empty seed, O-140), size, player count (`RandomPlayers + 2`), map
/// type, landscape and time of day from `Info`; game mode = game type of the session (`NS+0x1F4`). The game type
/// (`GNAME` text - 15540, `FUN_004CBB10`) serves as an index into `GTYPE` - in the shipped data both lists are in
/// the same order (corpus test `random_map_request_becomes_a_spec`). A negative value (only from a changed
/// `Engine.ini`) becomes an invalid index, which the generator rejects with an error message. On the network `Info`
/// comes from the session (`MapId::Random`, player count = `SessionInfo.players`); the host reads the rules with it
/// (`net_session::net_rules`).
pub fn random_map_spec(req: &MatchRequest) -> Option<RandomMapSpec> {
    let info = &req.info;
    if info.level_type != 1 {
        return None;
    }
    let index = |v: i32| usize::try_from(v).unwrap_or(usize::MAX);
    Some(RandomMapSpec {
        seed_text: info.name.clone(),
        size: info.size_class,
        players: info.players,
        terrain: index(info.random_terrain),
        landscape: index(info.random_landscape),
        daytime: index(info.random_daytime),
        game_type: index(req.game_type),
    })
}

/// Active `.lvl` slots in ascending order (`Player_IsSlotActive`).
pub fn active_slots(rules: &Rules) -> Vec<u8> {
    let mut active: Vec<u8> = rules.level.players.iter().map(|p| p.slot).collect();
    active.sort_unstable();
    active.dedup();
    active
}

/// `SetupPlayersFromLobby` (`0x00432C00`, first loop): n = min(`FUN_004E8CE0` slot count, `FUN_0042DA50` active
/// slots); for the keys -1 ... n - 2 whose slot has type 1...4, `swprintf("%s (%s)", text 15195, text 15150 +
/// color)` (the color is queried first) and back into the session. Because `Game_StartLevel` removes the open slots
/// beforehand (`FUN_004E9800`), the keys are then no longer contiguous: a computer with key >= n - 1 keeps its name
/// "EMPTY" (O-192).
fn name_computers(slots: &mut [SlotDraft], active: usize, texts: &Texts) {
    let n = i32::try_from(slots.len().min(active)).unwrap_or(i32::MAX);
    for s in slots.iter_mut() {
        if (-1..n - 1).contains(&s.key) && (1..5).contains(&s.kind) {
            let color = text_of(texts, COLOR_TEXT.wrapping_add(s.color));
            let computer = text_of(texts, COMPUTER_TEXT);
            s.name = format!("{computer} ({color})");
        }
    }
}

/// A slot as a lobby entry (kind after `SetupPlayersFromLobby`: 999 human, 1...4 AI, otherwise closed).
fn lobby_entry(s: &SlotDraft) -> LobbyEntry {
    LobbyEntry {
        kind: match s.kind {
            HUMAN => SlotKind::Human,
            1..=4 => SlotKind::Ai,
            _ => SlotKind::Closed,
        },
        name: s.name.clone(),
        nation: s.nation,
        team: s.team,
        color: s.color,
        handicap: s.handicap,
    }
}

/// Game type 4 (K-277): lobby by slot, slot = team; active slots without an entry before the last occupied one get
/// a `Closed` filler with the unchanged values of the player.
fn railroad_lobby(
    slots: &[SlotDraft],
    rules: &Rules,
) -> Result<Vec<(Option<i32>, LobbyEntry)>, String> {
    let active = active_slots(rules);
    let mut placed: Vec<Option<(Option<i32>, LobbyEntry)>> = vec![None; active.len()];
    for s in slots {
        let Some(i) = u8::try_from(s.team)
            .ok()
            .and_then(|team| active.iter().position(|&a| a == team))
        else {
            return Err(format!(
                "Match cannot be started: team {} of \"{}\" has no slot on the map in game type 4",
                s.team, s.name
            ));
        };
        placed[i] = Some((Some(s.key), lobby_entry(s)));
    }
    let last = placed
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |i| i + 1);
    Ok(placed
        .into_iter()
        .zip(&active)
        .take(last)
        .map(|(entry, &slot)| {
            entry.unwrap_or_else(|| {
                let lp = rules.level.players.iter().rev().find(|p| p.slot == slot);
                let filler = LobbyEntry {
                    kind: SlotKind::Closed,
                    name: lp.map_or_else(String::new, |p| p.name.clone()),
                    nation: lp.map_or(0, |p| p.nation),
                    team: 0,
                    color: i32::from(slot),
                    handicap: 0,
                };
                (None, filler)
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use opennml_formats::config::LevelPlayer;
    use opennml_formats::localisation::Localisation;
    use opennml_ui::draft::{DraftSettings, LevelInfoData, SlotDraft};

    use super::*;
    use crate::testutil::{install, utf16};

    /// Texts of the computer names: colors 15150... and 15195 "Computer".
    fn texts() -> Texts {
        let table = "15150=Yellow
15151=Blue
15152=Green
15153=Red
15155=Pink
15158=White
15195=Computer
";
        let (_dir, vfs) = install(&[("data/txt/t.txt", utf16(table))]);
        let loc = Localisation {
            text_files: vec!["data/txt/t.txt".into()],
            ..Localisation::default()
        };
        Texts::load(&vfs, &loc)
    }

    /// `to_match_setup` with the test texts.
    fn convert(req: &MatchRequest, rules: &Rules) -> Result<MatchSetup, String> {
        to_match_setup(req, rules, &texts())
    }

    /// Rules with `.lvl` players on these slots (name "P<slot>", nation = slot % 6).
    fn rules(slots: &[u8]) -> Rules {
        let mut r = Rules::synthetic("", 16);
        r.level.players = slots
            .iter()
            .map(|&slot| LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation: i32::from(slot % 6),
                kind: 0,
                resources: Vec::new(),
            })
            .collect();
        r
    }

    fn slot(key: i32, kind: i32, team: i32, color: i32, handicap: i32) -> SlotDraft {
        SlotDraft {
            key,
            name: format!("S{key}"),
            kind,
            nation: 1,
            team,
            handicap,
            color,
        }
    }

    fn request(game_type: i32, slots: Vec<SlotDraft>) -> MatchRequest {
        MatchRequest {
            level: "data/levels/multiplayer/[4Players]-Map.lvl".into(),
            game_type,
            slots,
            settings: DraftSettings {
                game_type,
                unit_limit: 3,
                resources: 2,
                time_limit: 35,
                map_explored: true,
                ..DraftSettings::default()
            },
            info: LevelInfoData::default(),
        }
    }

    fn entry(
        kind: SlotKind,
        name: &str,
        nation: i32,
        team: i32,
        color: i32,
        handicap: i32,
    ) -> LobbyEntry {
        LobbyEntry {
            kind,
            name: name.into(),
            nation,
            team,
            color,
            handicap,
        }
    }

    // QUIRK O-133: kept
    #[test]
    fn match_request_converts_to_match_setup() {
        // Mode = game type, lobby in key order (999 -> human, 1...4 -> AI, handicap = slot value),
        // resources = `Resourcen`, living space = `UnitLimit`, `MapExplored`, time limit = `TimeLimit` (minutes), AI on
        // with an AI slot, seed 0.
        let req = request(
            3,
            vec![
                slot(1, 2, 1, 5, 2),
                slot(-1, 999, 0, 0, 0),
                slot(0, 1, 1, 3, 5),
            ],
        );
        let setup = convert(&req, &rules(&[0, 1, 2])).unwrap();
        assert_eq!(setup.mode, 3);
        assert_eq!(setup.seed, 0);
        assert_eq!((setup.resource_setting, setup.living_space), (2, 3));
        assert!(setup.map_explored);
        assert_eq!(setup.time_limit, 35);
        assert_eq!(setup.local_name, None);
        assert!(setup.ki.enabled);
        assert_eq!(setup.ki.campaign_difficulty, 1, "default");
        assert_eq!(
            setup.lobby,
            [
                entry(SlotKind::Human, "S-1", 1, 0, 0, 0),
                entry(SlotKind::Ai, "Computer (Red)", 1, 1, 3, 5),
                entry(SlotKind::Ai, "Computer (Pink)", 1, 1, 5, 2),
            ]
        );
        // Without computers the AI stays off.
        let req = request(0, vec![slot(-1, 999, 0, 0, 0), slot(0, 999, 1, 1, 0)]);
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        assert!(!setup.ki.enabled);
        assert_eq!(setup.lobby.len(), 2);
    }

    // QUIRK K-277: kept
    #[test]
    fn railroad_puts_each_entry_on_the_slot_of_its_team() {
        // K-277: game type 4 puts each entry on the slot of its team (`GetPlayer(team)`); `opennml-sim` occupies the active
        // slots in order, hence `Closed` fillers (with the values of the `.lvl` or of the player) for active slots
        // without an entry before it.
        let req = request(4, vec![slot(-1, 999, 2, 0, 0), slot(0, 1, 0, 1, 5)]);
        let setup = convert(&req, &rules(&[0, 1, 2, 3])).unwrap();
        assert_eq!(
            setup.lobby,
            [
                entry(SlotKind::Ai, "Computer (Blue)", 1, 0, 1, 5),
                entry(SlotKind::Closed, "P1", 1, 0, 1, 0),
                entry(SlotKind::Human, "S-1", 1, 2, 0, 0),
            ]
        );
        // Active slots with a gap (0, 2, 5): team 5 is the third active slot.
        let req = request(4, vec![slot(-1, 999, 5, 0, 0), slot(0, 1, 0, 1, 0)]);
        let setup = convert(&req, &rules(&[0, 2, 5])).unwrap();
        assert_eq!(
            setup.lobby.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [SlotKind::Ai, SlotKind::Closed, SlotKind::Human]
        );
        assert_eq!(setup.lobby[1].color, 2, "filler: color = slot");
        // Two entries in the same team: the later one (in key order) overwrites.
        let req = request(4, vec![slot(0, 1, 1, 1, 0), slot(-1, 999, 1, 0, 0)]);
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        assert_eq!(
            setup.lobby,
            [
                entry(SlotKind::Closed, "P0", 0, 0, 0, 0),
                entry(SlotKind::Ai, "Computer (Blue)", 1, 1, 1, 0),
            ]
        );
        // Team on a slot without a `.lvl` player -> error message.
        let req = request(4, vec![slot(-1, 999, 3, 0, 0)]);
        let err = convert(&req, &rules(&[0, 1])).unwrap_err();
        assert!(err.contains("team 3"), "{err}");
        // Other game types keep the order.
        let req = request(5, vec![slot(-1, 999, 3, 0, 0), slot(0, 1, 0, 1, 0)]);
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        assert_eq!(setup.lobby[0].kind, SlotKind::Human);
    }

    // QUIRK O-192: kept
    #[test]
    fn computers_are_named_only_below_the_slot_bound() {
        // `SetupPlayersFromLobby` names type 1...4 "Computer (color)" only for the keys -1 ... n - 2,
        // n = min(slot count, active slots). After removing the open slots (key 0 open), the computer with key 1
        // keeps its name (O-192).
        let req = request(0, vec![slot(-1, 999, 0, 0, 0), slot(1, 3, 2, 2, 0)]);
        let setup = convert(&req, &rules(&[0, 1, 2])).unwrap();
        assert_eq!(setup.lobby[1].name, "S1");
        // Without a gap: named (color 2 = 15152).
        let req = request(0, vec![slot(-1, 999, 0, 0, 0), slot(0, 3, 2, 2, 0)]);
        let setup = convert(&req, &rules(&[0, 1, 2])).unwrap();
        assert_eq!(setup.lobby[1].name, "Computer (Green)");
        // Fewer active slots than entries: n = active slots.
        let req = request(
            0,
            vec![
                slot(-1, 4, 0, 0, 0),
                slot(0, 1, 1, 1, 0),
                slot(1, 2, 2, 2, 0),
            ],
        );
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        let names: Vec<&str> = setup.lobby.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Computer (Yellow)", "Computer (Blue)", "S1"]);
        // A missing text like `FUN_0055FFE0`: the last missing number (O-4), here color 7.
        let req = request(0, vec![slot(-1, 999, 0, 0, 0), slot(0, 1, 1, 7, 0)]);
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        assert_eq!(setup.lobby[1].name, "Computer (15157)");
    }

    // QUIRK O-134: kept
    #[test]
    fn computer_named_white() {
        // O-134: color 8 is called 15158 "White", the computer name "Computer (White)" (15195 and 15158).
        let req = request(2, vec![slot(-1, 999, 0, 0, 0), slot(0, 2, 1, 8, 0)]);
        let setup = convert(&req, &rules(&[0, 1])).unwrap();
        assert_eq!(setup.lobby[1].name, "Computer (White)");
        assert_eq!(setup.lobby[1].color, 8);
        assert_eq!(setup.mode, 2);
    }

    // QUIRK K-277, O-192: kept
    #[test]
    fn railroad_slots_follow_teams_in_the_simulation() {
        // Corpus: railroad map in game type 4, the human (row 0) in team 1, the computer in team 0 -> in the game
        // player 1 is the human and player 0 the AI (like `GetPlayer(team)` in the original).
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let lvl = "data/levels/multiplayer/[2players]-railroadrage.lvl";
        let scene = opennml_world::level::load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
        let mut req = request(4, vec![slot(-1, 999, 1, 0, 0), slot(0, 2, 0, 1, 2)]);
        req.level = lvl.into();
        let loc = opennml_ui::assets::load_localisation(&vfs, "DEU").unwrap();
        let texts = Texts::load(&vfs, &loc);
        let setup = to_match_setup(&req, &rules, &texts).unwrap();
        let state = opennml_sim::setup::new_match(&rules, &setup).unwrap();
        // Computer name "Computer (color)" from text 15195 and 15150 + color (per edition).
        let name = match opennml_rda::testutil::game_version(&vfs).unwrap() {
            opennml_rda::testutil::GameVersion::German => "Computer (Blau)".to_string(),
            opennml_rda::testutil::GameVersion::English => {
                format!("{} ({})", text_of(&texts, 15195), text_of(&texts, 15151))
            }
        };
        assert_eq!(state.players[0].name, name);
        use opennml_sim::state::Control;
        assert_eq!(state.players[0].control, Control::Ai);
        assert_eq!(state.players[1].control, Control::Human);
        assert_eq!(
            (state.players[1].name.as_str(), state.players[1].team),
            ("S-1", 1)
        );
        assert_eq!((state.players[0].handicap, state.players[0].color), (2, 1));
    }

    #[test]
    fn human_lobby_entry_carries_the_net_name() {
        // In free play the accounting shows the lobby name (`Game_SelectLocalPlayer` `0x004BB3B0` sets the profile
        // name only for game type < 0). `FUN_004BF1F0` names the own slot (key -1) with `NetName` (`CREATE2`, test
        // `screens::tests::create_game` in `opennml-ui`); the lobby entry takes over the name of the slot unchanged, the
        // simulation creates the record with it.
        let mut req = request(0, vec![slot(-1, 999, 0, 0, 0), slot(0, 2, 1, 1, 0)]);
        req.slots[0].name = "Anna".into();
        let rules = rules(&[0, 1]);
        let setup = convert(&req, &rules).unwrap();
        assert_eq!(setup.lobby[0].kind, SlotKind::Human);
        assert_eq!(setup.lobby[0].name, "Anna");
        assert_eq!(
            setup.local_name, None,
            "only the campaign sets the profile name"
        );
        let state = opennml_sim::setup::new_match(&rules, &setup).unwrap();
        let table = opennml_sim::accounting::accounting_table(&state, "Computer");
        assert_eq!(table[0].name, "Anna");
    }

    /// The menu level is set up like the campaign, only with game type -2 (`FUN_004C9140` `0x004C9140`), difficulty 0
    /// as a leftover and `NetName` as the local name.
    // QUIRK O-446: kept
    #[test]
    fn menu_level_setup_is_campaign_like() {
        let vars = vec![("seen".to_owned(), 3)];
        let setup = menu_level_setup(80, Some("Anna".into()), &vars);
        assert_eq!(setup.mode, opennml_sim::setup::MENU_LEVEL_MODE);
        assert!(setup.lobby.is_empty());
        assert!(setup.ki.enabled);
        assert_eq!(setup.ki.campaign_difficulty, 0);
        assert!(setup.script.enabled);
        assert_eq!(setup.script.time_per_char, 80);
        assert_eq!(setup.script.profile_vars.get("seen"), Some(&3));
        assert_eq!(setup.local_name.as_deref(), Some("Anna"));
        assert!(!setup.map_explored);
        assert_eq!(setup.seed, 0);
        // Like the campaign except for the game type.
        let campaign = campaign_setup(0, 80, Some("Anna".into()), &vars);
        assert_eq!(
            MatchSetup {
                mode: campaign.mode,
                ..setup.clone()
            },
            campaign
        );
    }

    // QUIRK O-142: kept
    #[test]
    fn campaign_setup_uses_mode_minus_1_and_difficulty() {
        // `0x0C` (re/singleplayer-menus part 1, 6.2): game type -1, no lobby (the slots come from the `.lvl`), "map
        // explored" off (`+0x1F0 = 0`), seed 0; the game setup reads the difficulty `GameApp+0x19B4`
        // (`GameEngine_ApplyGameSettings` `0x004311C2`). Level script and AI on, speech time `TimePerChar`; profile
        // name `NetName` for the first human slot.
        let setup = campaign_setup(0, 80, Some("Anna".into()), &[]);
        assert_eq!(setup.mode, -1);
        assert_eq!(setup.seed, 0);
        assert!(setup.lobby.is_empty());
        assert!(!setup.map_explored);
        assert!(setup.ki.enabled);
        assert_eq!(setup.ki.campaign_difficulty, 0);
        assert_eq!(
            setup.script,
            opennml_sim::script::ScriptSetup {
                enabled: true,
                time_per_char: 80,
                profile_vars: Default::default(),
            }
        );
        assert_eq!(setup.local_name.as_deref(), Some("Anna"));
        // The setup does not read `Resourcen`/`TimeLimit` from the draft (O-142, no effect in the campaign).
        let default = MatchSetup::default();
        assert_eq!(
            (setup.resource_setting, setup.time_limit, setup.living_space),
            (
                default.resource_setting,
                default.time_limit,
                default.living_space
            )
        );
        assert_eq!(campaign_setup(2, 55, None, &[]).ki.campaign_difficulty, 2);

        // Corpus: `kamp1-1` with a profile name - game type -1, AI and script run, the accounting carries the name.
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let lvl = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
        let scene = opennml_world::level::load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene, "DEU").unwrap();
        let setup = campaign_setup(1, 55, Some("Anna".into()), &[]);
        let state = opennml_sim::setup::new_match(&rules, &setup).unwrap();
        assert_eq!(state.mode, -1);
        assert_eq!(state.campaign_difficulty, 1);
        assert!(state.script.level.is_some(), "level script runs");
        let table = opennml_sim::accounting::accounting_table(&state, "Computer");
        assert!(table.iter().any(|row| row.name == "Anna"), "{table:?}");
    }

    /// The profile variables of the profile go into the setup; the level script reads them through the mirror
    /// (`MissionState::profile_vars`). As in the original (E-252), 383 only sees names that are already lowercase in
    /// the German sense in the file: `FUN_0040C780` writes the name it looks up in lowercase and compares in `m_VarMap`
    /// with `_mbscmp` (case matters); "Gold" and "ÄRGER" (written into the file only by hand) stay invisible. If the
    /// same name appears twice in the file, the later one applies (`CMap::operator[]` when loading).
    // QUIRK E-252: kept
    // QUIRK E-166: deviation
    #[test]
    fn campaign_setup_takes_profile_vars() {
        let vars = vec![
            ("Gold".to_string(), 7),
            ("ÄRGER".to_string(), -2),
            ("gold".to_string(), 9),
            ("bonus".to_string(), 1),
            ("bonus".to_string(), 4),
        ];
        let setup = campaign_setup(1, 55, None, &vars);
        let got: Vec<(&str, i32)> = setup
            .script
            .profile_vars
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        assert_eq!(got, [("bonus", 4), ("gold", 9)]);
        // Without profile variables the map stays empty.
        assert!(
            campaign_setup(1, 55, None, &[])
                .script
                .profile_vars
                .is_empty()
        );
    }

    // QUIRK O-134: kept
    #[test]
    fn errors_are_german_messages() {
        // Draft without a map (O-188): empty path -> error message instead of a crash.
        let mut req = request(0, vec![slot(-1, 999, 0, 0, 0)]);
        req.level.clear();
        let err = convert(&req, &rules(&[0])).unwrap_err();
        assert!(err.contains("no map"), "{err}");
        // Game type 2 (domination) and color 8 "White" are free.
        for (game_type, color) in [(2, 0), (0, 8)] {
            let req = request(game_type, vec![slot(-1, 999, 0, color, 0)]);
            assert!(convert(&req, &rules(&[0])).is_ok(), "{game_type}/{color}");
        }
    }

    /// Start request of a random map as after `CREATE2` (`FUN_004BF1F0` at `LevelType 1`): seed in `Info.name`,
    /// player count = `RandomPlayers + 2`, menu values in `Info`; the path is that of the last chosen fixed map
    /// (none here).
    fn random_request() -> MatchRequest {
        let mut req = request(
            3,
            vec![
                slot(-1, 999, 0, 0, 0),
                slot(0, 2, 1, 1, 0),
                slot(1, 0, 2, 2, 0),
            ],
        );
        req.level.clear();
        req.info = LevelInfoData {
            name: "Seed".into(),
            path: String::new(),
            size_class: 1,
            players: 4,
            level_type: 1,
            random_terrain: 2,
            random_landscape: 1,
            random_daytime: 0,
        };
        req
    }

    // QUIRK O-430: kept
    #[test]
    fn random_map_request_becomes_a_spec() {
        // The menu values are in `Info` (seed text, size, player count, map type, landscape, time of day), the game
        // mode is the game type of the session (`NS+0x1F4`); it serves as an index into `GTYPE`.
        let req = random_request();
        assert_eq!(
            random_map_spec(&req),
            Some(RandomMapSpec {
                seed_text: "Seed".into(),
                size: 1,
                players: 4,
                terrain: 2,
                landscape: 1,
                daytime: 0,
                game_type: 3,
            })
        );
        // Fixed map (`LevelType 0`): no description.
        assert_eq!(random_map_spec(&request(0, Vec::new())), None);
        // Negative values (only from a changed `Engine.ini`) become an invalid index, which the generator rejects
        // with an error message (`script_and_defines`).
        let mut bad = random_request();
        bad.info.random_terrain = -1;
        bad.game_type = -1;
        let spec = random_map_spec(&bad).unwrap();
        assert_eq!((spec.terrain, spec.game_type), (usize::MAX, usize::MAX));
        // `to_match_setup` accepts the empty path for a random map; mode = game type.
        let setup = convert(&req, &rules(&[0, 1, 2, 3])).unwrap();
        assert_eq!(setup.mode, 3);
        assert_eq!(setup.lobby.len(), 3);
        // In the shipped data the `GTYPE` index equals the game type `GNAME` text - 15540 (`FUN_004CBB10`).
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let types = opennml_randommap::load_types(&vfs).unwrap();
        assert_eq!(types.game_types.len(), 6);
        for (i, g) in types.game_types.iter().enumerate() {
            assert_eq!(
                usize::try_from(g.name_id - opennml_ui::draft::GAME_TYPE_TEXT).ok(),
                Some(i),
                "{}",
                g.gtype
            );
        }
    }
}
