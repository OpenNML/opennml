//! Tests of the network session without a menu: session name, map checksum, the host's lobby rule and the slot
//! assignment of the rows. The flows with a menu are in `game_flow/net_tests.rs`.

use nml_formats::config::LevelPlayer;
use nml_formats::localisation::Localisation;
use nml_net::{Announcement, LobbyBook, SessionList, TurnConfig};
use nml_sim::rules::Rules;
use nml_sim::setup::SlotKind;
use nml_ui::draft::{DraftSettings, LevelInfoData, MatchRequest};

use super::*;
use crate::testutil::{install, utf16};

/// Texts: 15789 "Session", colors and "Computer" for the computer names.
fn texts() -> Texts {
    let table = "15150=Yellow
15151=Blue
15152=Green
15195=Computer
15789=Session
";
    let (_dir, vfs) = install(&[("data/txt/t.txt", utf16(table))]);
    let loc = Localisation {
        text_files: vec!["data/txt/t.txt".into()],
        ..Localisation::default()
    };
    Texts::load(&vfs, &loc)
}

// QUIRK E-300: deviation
#[test]
fn session_name_defaults_to_session_dash_player() {
    // Quirk E-300 (`FUN_004EB190`): empty -> "<15789> - <player>", written back to `NetSession`.
    let texts = texts();
    let mut cfg = EngineConfig::default();
    assert_eq!(session_name(&mut cfg, &texts, "Anna"), "Session - Anna");
    assert_eq!(name_text(&cfg.net_session), "Session - Anna");
    // Only spaces count as empty.
    cfg.net_session = name_field("   ");
    assert_eq!(session_name(&mut cfg, &texts, "Bert"), "Session - Bert");
    // A custom name stays, without `[`/`]` and shortened to 39 characters.
    cfg.net_session = name_field("Round [COLOR FF0000]");
    assert_eq!(session_name(&mut cfg, &texts, "Anna"), "Round COLOR FF0000");
    let long = "x".repeat(60);
    let mut cfg = EngineConfig::default();
    let name = session_name(&mut cfg, &texts, &long);
    assert_eq!(name.chars().count(), 39);
    assert!(name.starts_with("Session - xxx"));
    assert_eq!(name_text(&cfg.net_session), name);
}

/// Announcement of a fixed map `path` with the checksum `checksum` (one free slot).
fn announcement(path: &str, checksum: u64, hello: &Hello) -> Announcement {
    Announcement {
        instance: 7,
        nonce: 0,
        versions: hello.clone(),
        tcp_port: 2302,
        free: 1,
        total: 1,
        session: SessionInfo {
            map: MapId::Fixed {
                path: path.into(),
                checksum,
            },
            ..SessionInfo::plain("Round", path, 2, TurnConfig::default())
        },
    }
}

fn test_hello() -> Hello {
    Hello {
        listen_port: None,
        net_version: NET_VERSION,
        protocol: 1,
        format: 1,
        edition: "test".into(),
        name: "Anna".into(),
        build: "test".into(),
        seat: None,
    }
}

#[test]
fn map_checksum_hides_a_changed_map() {
    // FNV-1a 64 over `.lvl` and `.wld`; the list shows a game only with the same checksum of the local map. A
    // different `.wld` (same `.lvl`) changes the checksum, a missing file yields none.
    let lvl = "data/levels/multiplayer/[2players]-test.lvl";
    let wld = "data/levels/multiplayer/[2players]-test.wld";
    let (_a, host) = install(&[(lvl, b"LEVEL".to_vec()), (wld, b"WORLD".to_vec())]);
    let (_b, same) = install(&[(lvl, b"LEVEL".to_vec()), (wld, b"WORLD".to_vec())]);
    let (_c, changed) = install(&[(lvl, b"LEVEL".to_vec()), (wld, b"WORLD2".to_vec())]);
    let (_d, missing) = install(&[(lvl, b"LEVEL".to_vec())]);
    let sum = map_checksum(&host, lvl).expect("checksum");
    assert_eq!(map_checksum(&same, lvl), Some(sum));
    assert_ne!(map_checksum(&changed, lvl), Some(sum));
    assert_eq!(map_checksum(&missing, lvl), None);
    let hello = test_hello();
    let from: SocketAddr = "10.0.0.1:2303".parse().unwrap();
    for (vfs, visible) in [(&same, true), (&changed, false), (&missing, false)] {
        let mut list = SessionList::new();
        list.update(0, from, announcement(lvl, sum, &hello), &hello, |map| {
            matches!(map, MapId::Fixed { path, checksum } if map_checksum(vfs, path) == Some(*checksum))
        });
        assert_eq!(
            list.games().len(),
            usize::from(visible),
            "visible: {visible}"
        );
    }
}

fn row(kind: i32, team: i32, color: i32) -> LobbyRow {
    LobbyRow {
        kind,
        name: String::new(),
        nation: 0,
        team,
        handicap: 0,
        color,
        ready: false,
        peer: None,
    }
}

#[test]
fn client_lobby_rules_follow_team_color_check() {
    // The host's rule is `FUN_004BA710` ([`team_color_check`]) with row n = key n - 1.
    let rules = ClientLobbyRules;
    let rows = [row(999, 0, 0), row(999, 1, 1), row(0, 2, 2)];
    // Different color, different team: allowed.
    assert_eq!(
        rules.check(&rows, 1, &row(999, 1, 3), 0),
        RuleVerdict::Accept
    );
    // Color of another occupied slot: rejected.
    assert_eq!(
        rules.check(&rows, 1, &row(999, 1, 0), 0),
        RuleVerdict::Reject
    );
    // All in the same team: rejected.
    assert_eq!(
        rules.check(&rows, 1, &row(999, 0, 1), 0),
        RuleVerdict::Reject
    );
    // Game type 4: same team as row 0 -> swap, row 0 gets the old team 1.
    assert_eq!(
        rules.check(&rows, 1, &row(999, 0, 1), 4),
        RuleVerdict::Swap { row: 0, team: 1 }
    );
}

/// Rules with `.lvl` players on these slots.
fn rules(slots: &[u8]) -> Rules {
    let mut r = Rules::synthetic("", 16);
    r.level.players = slots
        .iter()
        .map(|&slot| LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: 0,
            kind: 0,
            resources: Vec::new(),
        })
        .collect();
    r
}

#[test]
fn start_infos_follow_rows_with_an_open_row() {
    // Row 1 open between host (row 0) and client (row 2), row 3 a computer. `Game_StartLevel` removes the open row,
    // the remaining ones move up to the active slots: host slot 0, client slot 1, computer slot 2 - not
    // "slot = row". Slot choice, occupancy and setup come from the same calculation.
    let texts = texts();
    let rules = rules(&[0, 1, 2, 3]);
    let session = SessionInfo::plain("Round", "multiplayer/test.lvl", 4, TurnConfig::default());
    let mut book = LobbyBook::open(session, "Anna").unwrap();
    assert_eq!(book.join(1, "Guest"), Ok(1));
    assert_eq!(book.join(2, "Bert"), Ok(2));
    assert_eq!(book.leave(1), Some(1));
    let mut state = book.state().clone();
    state.rows[3].kind = 2;
    state.rows[3].color = 3;
    state.rows[3].team = 3;
    let slot = |key: i32, r: &LobbyRow| SlotDraft {
        key,
        name: r.name.clone(),
        kind: r.kind,
        nation: r.nation,
        team: r.team,
        handicap: r.handicap,
        color: r.color,
    };
    // The start order as after `{0x0D}`: open slots removed.
    let req = MatchRequest {
        level: "multiplayer/test.lvl".into(),
        game_type: 0,
        slots: state
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind != 0)
            .map(|(i, r)| slot(i as i32 - 1, r))
            .collect(),
        settings: DraftSettings {
            time_limit: 20,
            ..DraftSettings::default()
        },
        info: LevelInfoData::default(),
    };
    let slots = row_slots(&req, &rules, &texts).unwrap();
    assert_eq!(
        slots,
        [Some(0), None, Some(1), Some(2), None, None, None, None]
    );
    let setup = to_net_setup(&req, &rules, &texts, 4711).unwrap();
    assert!(setup.network);
    assert_eq!(setup.seed, 4711);
    let kinds: Vec<SlotKind> = setup.lobby.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [SlotKind::Human, SlotKind::Human, SlotKind::Ai]);
    let infos = start_infos(&state, &slots, &active_slots(&rules), &setup, &rules, 9).unwrap();
    let rows: Vec<(u8, u8)> = infos.iter().map(|(r, i)| (*r, i.your_slot)).collect();
    assert_eq!(rows, [(0, 0), (2, 1)], "host slot 0, Bert slot 1");
    let info = &infos[1].1;
    assert_eq!(
        (info.seats[0].peer, info.seats[0].name.as_str()),
        (Some(0), "Anna")
    );
    assert_eq!(
        (info.seats[1].peer, info.seats[1].name.as_str()),
        (Some(2), "Bert")
    );
    // The computer is named as in the setup (here without a name: key 2 is above the limit of O-192).
    assert_eq!(info.seats[2].peer, None);
    assert_eq!(info.seats[2].name, setup.lobby[2].name);
    assert_eq!(info.seats[3].peer, None);
    assert_eq!((info.spacing, info.lead, info.game_speed), (8, 4, 9));
    assert_eq!(info.session, "Round");
    assert_eq!(info.package.setup, setup);
}

/// The game speed of the start is at most 20 (protocol rule); 255 from the settings would halt the start. A smaller
/// game speed stays.
// QUIRK O-153: deviation
#[test]
fn start_infos_clamp_the_speed_to_the_net_limit() {
    let texts = texts();
    let rules = rules(&[0, 1]);
    let session = SessionInfo::plain("Round", "multiplayer/test.lvl", 2, TurnConfig::default());
    let mut book = LobbyBook::open(session, "Anna").unwrap();
    assert_eq!(book.join(1, "Bert"), Ok(1));
    let state = book.state().clone();
    let slot = |key: i32, r: &LobbyRow| SlotDraft {
        key,
        name: r.name.clone(),
        kind: r.kind,
        nation: r.nation,
        team: r.team,
        handicap: r.handicap,
        color: r.color,
    };
    let req = MatchRequest {
        level: "multiplayer/test.lvl".into(),
        game_type: 0,
        slots: state
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind != 0)
            .map(|(i, r)| slot(i as i32 - 1, r))
            .collect(),
        settings: DraftSettings {
            time_limit: 20,
            ..DraftSettings::default()
        },
        info: LevelInfoData::default(),
    };
    let slots = row_slots(&req, &rules, &texts).unwrap();
    let setup = to_net_setup(&req, &rules, &texts, 1).unwrap();
    let active = active_slots(&rules);
    for (given, want) in [(255, 20), (21, 20), (20, 20), (9, 9)] {
        let infos = start_infos(&state, &slots, &active, &setup, &rules, given).unwrap();
        assert!(!infos.is_empty());
        for (_, info) in &infos {
            assert_eq!(info.game_speed, want, "game speed {given}");
        }
    }
}

/// A `Rejected` in the lobby (not produced there by the core, only before the `Welcome`) does not end the lobby
/// silently but like "host gone": once `{0x2F}` (message 15212 with `{0x2D}`), then nothing more.
#[test]
fn rejected_in_the_lobby_counts_as_host_gone() {
    let (dir, vfs) = crate::testutil::world();
    let layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
    let opts = nml_ui::app::MenuOptions {
        start_menu: false,
        ..nml_ui::app::MenuOptions::default()
    };
    let config = crate::testutil::test_config(&vfs);
    let mut app = MenuApp::new(&vfs, &layer.texts, config, opts).unwrap();
    let net = nml_net::MemNet::new();
    let identity = NetIdentity {
        hello: test_hello(),
        port: 2302,
        seed: Box::new(|| 1),
        listen: true,
    };
    let mut session = NetSession::new(
        Box::new(nml_net::MemTransport::new(&net, 0)),
        identity,
        crate::testutil::net_paths(dir.path(), 0),
    );
    let rejected = || CoreEvent::Rejected {
        reason: "rejected".into(),
    };
    assert!(session.lobby_events(vec![rejected()], &mut app));
    let queued: Vec<&AppCmd> = app.queue().iter().collect();
    assert_eq!(queued, [&AppCmd::HostGone]);
    // Only once.
    assert!(session.lobby_events(vec![rejected(), CoreEvent::HostLeft], &mut app));
    assert_eq!(app.queue().len(), 1);
}

#[test]
fn railroad_rows_take_the_slot_of_their_team() {
    // With game type 4: the row stands on the slot of its team (`GetPlayer(team)`, quirk K-277).
    let texts = texts();
    let rules = rules(&[0, 1, 2]);
    let human = |key: i32, team: i32| SlotDraft {
        key,
        name: format!("S{key}"),
        kind: 999,
        nation: 0,
        team,
        handicap: 0,
        color: key + 1,
    };
    let req = MatchRequest {
        level: "multiplayer/test.lvl".into(),
        game_type: 4,
        slots: vec![human(-1, 2), human(0, 0)],
        settings: DraftSettings {
            game_type: 4,
            time_limit: 20,
            ..DraftSettings::default()
        },
        info: LevelInfoData::default(),
    };
    let slots = row_slots(&req, &rules, &texts).unwrap();
    assert_eq!(&slots[..3], &[Some(2), Some(0), None]);
}

// ---------------------------------------------------------------------------------------------------------------
// Session and rows from the draft, map check. The flows with a menu (map change, client without a map) are in
// `game_flow/net_tests.rs`.

/// Small installation with a fixed map `multiplayer/a.lvl` including `.wld`.
fn map_install() -> (tempfile::TempDir, Vfs) {
    install(&[
        ("multiplayer/a.lvl", b"[PLAYER]\n".to_vec()),
        ("multiplayer/a.wld", b"WORLD".to_vec()),
    ])
}

/// A fixed map only with the same checksum over `.lvl` and `.wld` (deviation from quirk E-322); never without one of
/// the files; a random map always.
// QUIRK E-322: deviation
#[test]
fn map_available_follows_the_checksum() {
    let (_dir, vfs) = map_install();
    let checksum = map_checksum(&vfs, "multiplayer/a.lvl").expect("map readable");
    let fixed = |path: &str, checksum: u64| MapId::Fixed {
        path: path.into(),
        checksum,
    };
    assert!(map_available(&vfs, &fixed("multiplayer/a.lvl", checksum)));
    assert!(!map_available(
        &vfs,
        &fixed("multiplayer/a.lvl", checksum ^ 1)
    ));
    assert!(!map_available(&vfs, &fixed("multiplayer/b.lvl", checksum)));
    let (_only, lvl_only) = install(&[("multiplayer/a.lvl", b"[PLAYER]\n".to_vec())]);
    assert!(!map_available(
        &lvl_only,
        &fixed("multiplayer/a.lvl", checksum)
    ));
    let random = MapId::Random {
        seed_text: "77".into(),
        size: 1,
        players: 3,
        terrain: 0,
        landscape: 0,
        daytime: 0,
    };
    assert!(map_available(&lvl_only, &random));
}

/// Draft on `multiplayer/a.lvl` with three slots.
fn draft_a() -> MatchDraft {
    MatchDraft {
        map: Some(0),
        info: LevelInfoData {
            name: "Map A".into(),
            path: "multiplayer/a.lvl".into(),
            size_class: 1,
            players: 3,
            level_type: 0,
            ..LevelInfoData::default()
        },
        slots: Vec::new(),
        settings: DraftSettings {
            game_type: 2,
            unit_limit: 3,
            resources: 1,
            time_limit: 30,
            lock_teams: true,
            map_explored: false,
            save_replay: true,
            session: "any".into(),
        },
    }
}

/// `session_from_draft` takes name and raster from outside, map (path, checksum), map name, number of players and
/// settings from the draft; only fixed maps with a path, 2 ... 8 players, a readable map.
#[test]
fn session_from_draft_takes_map_settings_and_raster() {
    let (_dir, vfs) = map_install();
    let raster = TurnConfig::new(4, 2).unwrap();
    let draft = draft_a();
    let s = session_from_draft(&draft, "Round".into(), &vfs, raster).expect("session");
    assert_eq!(s.name, "Round");
    assert_eq!(
        s.map,
        MapId::Fixed {
            path: "multiplayer/a.lvl".into(),
            checksum: map_checksum(&vfs, "multiplayer/a.lvl").unwrap(),
        }
    );
    assert_eq!(s.map_name, "Map A");
    assert_eq!((s.players, s.spacing, s.lead), (3, 4, 2));
    assert_eq!(
        s.settings,
        SessionSettings {
            game_type: 2,
            unit_limit: 3,
            resources: 1,
            time_limit: 30,
            lock_teams: true,
            map_explored: false,
            save_replay: true,
        }
    );
    let refused = |change: fn(&mut MatchDraft)| {
        let mut d = draft_a();
        change(&mut d);
        session_from_draft(&d, "Round".into(), &vfs, raster).is_err()
    };
    assert!(
        refused(|d| d.info.level_type = 2),
        "level type 2 (savegame) is refused"
    );
    assert!(refused(|d| d.info.path.clear()));
    assert!(refused(|d| d.info.players = 1));
    assert!(refused(|d| d.info.players = 9));
    assert!(refused(|d| d.info.path = "multiplayer/b.lvl".into()));
}

/// Draft of a random map with seed text `seed` (three players; the path is that of the last chosen fixed map and does
/// not count).
fn random_draft(seed: &str) -> MatchDraft {
    let mut d = draft_a();
    d.info = LevelInfoData {
        name: seed.into(),
        path: "multiplayer/gibtesnicht.lvl".into(),
        size_class: 1,
        players: 3,
        level_type: 1,
        random_terrain: 2,
        random_landscape: 1,
        random_daytime: 3,
    };
    d
}

/// `session_from_draft` with `LevelType` 1 makes a `MapId::Random` of `Info` (seed text, size, number of players =
/// `SessionInfo.players`, map type, landscape, time of day), map name = seed text; no file is read (a path without a
/// map is the same too), settings and raster as with a fixed map.
// QUIRK E-313: kept
#[test]
fn random_session_carries_the_seed_text() {
    let (_dir, vfs) = map_install();
    let raster = TurnConfig::new(4, 2).unwrap();
    let s = session_from_draft(&random_draft("18467"), "Round".into(), &vfs, raster)
        .expect("session of the random map");
    assert_eq!(
        s.map,
        MapId::Random {
            seed_text: "18467".into(),
            size: 1,
            players: 3,
            terrain: 2,
            landscape: 1,
            daytime: 3,
        }
    );
    assert_eq!(s.map_name, "18467");
    assert_eq!((s.players, s.spacing, s.lead), (3, 4, 2));
    assert_eq!(s.settings.game_type, 2);
    // An empty seed text (only via a hand-built draft) and numbers of players outside 2 ... 8 are not.
    assert!(session_from_draft(&random_draft(""), "R".into(), &vfs, raster).is_err());
    let mut d = random_draft("1");
    d.info.players = 9;
    assert!(session_from_draft(&d, "R".into(), &vfs, raster).is_err());
    // The rows follow the number of players as with `LAN3 CREATE` (`random_slots`: open, team and color = row).
    let rows = rows_from_draft(&random_draft("1"), s.players);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .enumerate()
            .all(|(i, r)| r.kind == 0 && r.team == i as i32 && r.color == i as i32)
    );
}

/// The client's draft from a lobby state with a random map ([`lobby_draft`]) shows "Random value:" (15659) with the
/// seed text in `LEVELINFO` (quirk E-313), plus map type, landscape and time of day (15671/15670/15672).
// QUIRK E-313: kept
#[test]
fn levelinfo_shows_the_random_seed_in_the_lobby() {
    let table = "15659=Random value:
15650=Name:
15670=Landscape type:
15671=Map type:
15672=Time of day:
15472=Island
15491=Desert
15503=Night
";
    let (_dir, vfs) = install(&[("data/txt/t.txt", utf16(table))]);
    let loc = Localisation {
        text_files: vec!["data/txt/t.txt".into()],
        ..Localisation::default()
    };
    let texts = Texts::load(&vfs, &loc);
    let raster = TurnConfig::new(4, 2).unwrap();
    let session =
        session_from_draft(&random_draft("18467"), "Round".into(), &vfs, raster).expect("session");
    let rows = rows_from_draft(&random_draft("18467"), session.players);
    let book = LobbyBook::new(session, rows, "Anna".into()).expect("book");
    let (slots, info, settings) = lobby_draft(book.state(), &MapCatalog::default());
    assert_eq!((info.level_type, info.name.as_str()), (1, "18467"));
    let draft = MatchDraft {
        map: None,
        info,
        slots,
        settings,
    };
    let text = nml_ui::draft::level_infos(&draft, None, &texts);
    assert!(text.contains("Random value:"), "{text}");
    assert!(text.contains("18467"), "{text}");
    assert!(!text.contains("Name:"), "{text}");
    for value in [
        "Map type:",
        "Island",
        "Landscape type:",
        "Desert",
        "Time of day:",
        "Night",
    ] {
        assert!(text.contains(value), "{value}: {text}");
    }
}

/// `rows_from_draft` cleans only computer names (type 1 ... 4); open rows and the own slot keep their name, rows
/// without a slot are open with team and color = row, invalid types open.
#[test]
fn rows_from_draft_cleans_computer_names() {
    let slot = |key: i32, kind: i32, name: &str| SlotDraft {
        key,
        name: name.into(),
        kind,
        nation: key + 2,
        team: 0,
        handicap: if kind == 1 { 5 } else { 0 },
        color: key + 1,
    };
    let mut draft = draft_a();
    draft.slots = vec![
        slot(-1, 999, "Anna"),
        slot(0, 1, "[KI] Hans"),
        slot(1, 0, "[EMPTY]"),
        slot(2, 7, "Strange"),
    ];
    let rows = rows_from_draft(&draft, 5);
    let view: Vec<(i32, &str, i32, i32)> = rows
        .iter()
        .map(|r| (r.kind, r.name.as_str(), r.team, r.color))
        .collect();
    assert_eq!(
        view,
        vec![
            (0, "Anna", 0, 0),
            (1, "KI Hans", 0, 1),
            (0, "[EMPTY]", 0, 2),
            (0, "Strange", 0, 3),
            (0, "", 4, 4),
        ]
    );
    assert!(rows.iter().all(|r| !r.ready && r.peer.is_none()));
    assert_eq!(rows[1].handicap, 5);
    assert_eq!(rows_from_draft(&draft, 2).len(), 2);
}

// QUIRK E-307: deviation
#[test]
fn rows_fit_matches_in_ui_and_core() {
    // The slot check of `LAN31` (`nml_ui::draft::rows_fit` over the slots of the draft) and that of the core
    // (`nml_net::rows_fit` over the lobby rows) judge the same - for every occupancy of up to eight rows with open,
    // computer and human and every number of players 0 ... 9 (occupied = type != 0).
    const KINDS: [i32; 3] = [0, 2, 999];
    let mut cases = 0;
    for len in 0..=8u32 {
        for code in 0..3u32.pow(len) {
            let rows: Vec<LobbyRow> = (0..len)
                .map(|i| {
                    let kind = KINDS[((code / 3u32.pow(i)) % 3) as usize];
                    let row = i32::try_from(i).unwrap();
                    LobbyRow {
                        kind,
                        name: String::new(),
                        nation: 0,
                        team: row,
                        handicap: 0,
                        color: row,
                        ready: false,
                        peer: None,
                    }
                })
                .collect();
            let slots: Vec<SlotDraft> = rows
                .iter()
                .enumerate()
                .map(|(i, r)| slot_of(i, r))
                .collect();
            for players in 0..=9u8 {
                assert_eq!(
                    nml_ui::draft::rows_fit(&slots, i32::from(players)),
                    nml_net::rows_fit(&rows, players),
                    "{rows:?}, {players} players"
                );
                cases += 1;
            }
        }
    }
    assert!(cases > 90_000);
}
