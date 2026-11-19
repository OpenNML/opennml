//! Tests of the network session without a menu: session name, map checksum, the host's lobby rule and the slot
//! assignment of the rows. The flows with a menu are in `game_flow/net_tests.rs`.

use opennml_formats::config::LevelPlayer;
use opennml_formats::localisation::Localisation;
use opennml_net::{Announcement, LobbyBook, SessionList, TurnConfig};
use opennml_sim::rules::Rules;
use opennml_sim::setup::SlotKind;
use opennml_ui::draft::{DraftSettings, LevelInfoData, MatchRequest};

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

/// The map checksum covers the bytes of `.lvl` and `.wld`, in order: swapped lines change it. The `.hlm` is not part of
/// it (the rules checksum of the data check covers it).
// QUIRK E-353: deviation
#[test]
fn map_checksum_covers_lvl_and_wld_bytes() {
    let lvl = "data/levels/multiplayer/[2players]-test.lvl";
    let wld = "data/levels/multiplayer/[2players]-test.wld";
    let sum = |level: &[u8], world: &[u8]| {
        let (_dir, vfs) = install(&[(lvl, level.to_vec()), (wld, world.to_vec())]);
        map_checksum(&vfs, lvl).expect("checksum")
    };
    let base = sum(
        b"line one
line two
",
        b"world one
world two
",
    );
    assert_ne!(
        base,
        sum(
            b"line two
line one
",
            b"world one
world two
"
        )
    );
    assert_ne!(
        base,
        sum(
            b"line one
line two
",
            b"world two
world one
"
        )
    );

    // A changed `.hlm` leaves the map checksum, but the rules checksum differs.
    let Some(vfs) = corpus_vfs() else {
        return;
    };
    let req = request(FIXED_MAP, LevelInfoData::default());
    let (rules, entries) = net_rules_recorded(&vfs, &req);
    let rules = rules.unwrap();
    let hlm = entries
        .iter()
        .find(|e| e.path.ends_with(".hlm") && e.hash.is_some())
        .expect("a .hlm of the map")
        .path
        .clone();
    let mut bytes = vfs.read(&hlm).unwrap();
    *bytes.last_mut().expect("bytes") ^= 1;
    let dir = tempfile::tempdir().unwrap();
    let disk = dir.path().join("changed.hlm");
    std::fs::write(&disk, &bytes).unwrap();
    let other = opennml_rda::testutil::with_loose_file(&vfs, &hlm, &disk);
    assert_eq!(
        map_checksum(&other, FIXED_MAP),
        map_checksum(&vfs, FIXED_MAP)
    );
    assert_ne!(
        net_rules(&other, &req).map(|r| r.checksum),
        Ok(rules.checksum)
    );
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
        data: DataState::Equal,
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
    let opts = opennml_ui::app::MenuOptions {
        start_menu: false,
        ..opennml_ui::app::MenuOptions::default()
    };
    let config = crate::testutil::test_config(&vfs);
    let mut app = MenuApp::new(&vfs, &layer.texts, config, opts).unwrap();
    let net = opennml_net::MemNet::new();
    let identity = NetIdentity {
        hello: test_hello(),
        port: 2302,
        seed: Box::new(|| 1),
        listen: true,
    };
    let mut session = NetSession::new(
        Box::new(opennml_net::MemTransport::new(&net, 0)),
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
    let text = opennml_ui::draft::level_infos(&draft, None, &texts);
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
    // The slot check of `LAN31` (`opennml_ui::draft::rows_fit` over the slots of the draft) and that of the core
    // (`opennml_net::rows_fit` over the lobby rows) judge the same - for every occupancy of up to eight rows with open,
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
                        data: DataState::Equal,
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
                    opennml_ui::draft::rows_fit(&slots, i32::from(players)),
                    opennml_net::rows_fit(&rows, players),
                    "{rows:?}, {players} players"
                );
                cases += 1;
            }
        }
    }
    assert!(cases > 90_000);
}

/// Fixed multiplayer map of the corpus tests.
const FIXED_MAP: &str = "data/levels/multiplayer/[2players]-tintebariver.lvl";

fn request(level: &str, info: LevelInfoData) -> MatchRequest {
    MatchRequest {
        level: level.into(),
        game_type: 0,
        slots: Vec::new(),
        settings: DraftSettings::default(),
        info,
    }
}

fn random_request() -> MatchRequest {
    request(
        "",
        LevelInfoData {
            name: "77".into(),
            players: 2,
            level_type: 1,
            ..LevelInfoData::default()
        },
    )
}

fn corpus_vfs() -> Option<Vfs> {
    let dir = opennml_rda::testutil::corpus_game_dir()?;
    Some(Vfs::open_game_dir(dir).expect("corpus installation"))
}

fn has(entries: &[opennml_rda::ReadEntry], path: &str) -> bool {
    entries.iter().any(|e| e.path == path && e.hash.is_some())
}

#[test]
fn recorded_inputs_cover_the_rules_files() {
    let Some(vfs) = corpus_vfs() else {
        return;
    };
    let fixed = request(FIXED_MAP, LevelInfoData::default());
    let (rules, entries) = net_rules_recorded(&vfs, &fixed);
    let rules = rules.expect("rules of the fixed map");
    assert_eq!(rules.checksum, net_rules(&vfs, &fixed).unwrap().checksum);
    for path in [
        "data/gifcon/bonus.ini",
        "data/levels/multiplayer/[2players]-tintebariver.lvl",
    ] {
        assert!(has(&entries, path), "{path}");
    }
    for ext in [".wld", ".hlm"] {
        assert!(
            entries
                .iter()
                .any(|e| e.path.ends_with(ext) && e.hash.is_some()),
            "{ext}"
        );
    }
    assert!(
        entries
            .iter()
            .any(|e| e.path.ends_with("nml.db") && e.hash.is_some())
    );
    assert!(
        entries.windows(2).all(|w| w[0].path < w[1].path),
        "ascending, once"
    );

    let random = random_request();
    let (rules, entries) = net_rules_recorded(&vfs, &random);
    let rules = rules.expect("rules of the random map");
    assert_eq!(rules.checksum, net_rules(&vfs, &random).unwrap().checksum);
    assert!(has(&entries, "data/gifcon/randommaptypes.ini"));
    assert!(
        entries
            .iter()
            .any(|e| e.path.ends_with(".rnd") && e.hash.is_some())
    );
}

#[test]
fn changed_loose_file_changes_its_entry_and_the_checksum() {
    let Some(vfs) = corpus_vfs() else {
        return;
    };
    let req = request(FIXED_MAP, LevelInfoData::default());
    let (rules, before) = net_rules_recorded(&vfs, &req);
    let rules = rules.unwrap();
    let mut bytes = vfs.read("data/gifcon/bonus.ini").unwrap();
    // A digit of a value, so that the file still parses.
    let at = bytes.iter().rposition(u8::is_ascii_digit).expect("a digit");
    bytes[at] = if bytes[at] == b'9' {
        b'8'
    } else {
        bytes[at] + 1
    };
    let dir = tempfile::tempdir().unwrap();
    let disk = dir.path().join("bonus.ini");
    std::fs::write(&disk, &bytes).unwrap();
    let other = opennml_rda::testutil::with_loose_file(&vfs, "data/gifcon/bonus.ini", &disk);
    let (changed, after) = net_rules_recorded(&other, &req);
    assert_ne!(changed.unwrap().checksum, rules.checksum);
    let diff: Vec<_> = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a.path.as_str())
        .collect();
    assert_eq!(before.len(), after.len());
    assert_eq!(diff, ["data/gifcon/bonus.ini"]);
}

#[test]
fn recorded_inputs_are_deterministic() {
    let Some(vfs) = corpus_vfs() else {
        return;
    };
    for req in [
        request(FIXED_MAP, LevelInfoData::default()),
        random_request(),
    ] {
        let (_, first) = net_rules_recorded(&vfs, &req);
        let (_, second) = net_rules_recorded(&vfs, &req);
        assert!(!first.is_empty());
        assert_eq!(first, second);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Game data check of the session (re/multiplayer-menus L.18): host and client compute the rules of the session in the
// rules worker; the host publishes its checksum, the client reports once it is there.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use opennml_ui::draft::MapCatalogEntry;

/// Rules worker that computes only on [`Self::release`] and returns every result, also of older jobs - like threads
/// that keep running after a newer job.
#[derive(Clone, Default)]
struct ManualWorker {
    jobs: Arc<Mutex<VecDeque<RulesJob>>>,
    done: Arc<Mutex<VecDeque<RulesDone>>>,
}

impl ManualWorker {
    /// Session revisions of the waiting jobs, oldest first.
    fn waiting(&self) -> Vec<u32> {
        let jobs = self.jobs.lock().unwrap();
        jobs.iter().map(|j| j.session_revision).collect()
    }

    /// Computes the oldest waiting job; its result comes with the next poll.
    fn release(&self) {
        let job = self
            .jobs
            .lock()
            .unwrap()
            .pop_front()
            .expect("a waiting job");
        let done = job.run();
        self.done.lock().unwrap().push_back(done);
    }
}

impl RulesWorker for ManualWorker {
    fn submit(&mut self, job: RulesJob) {
        self.jobs.lock().unwrap().push_back(job);
    }

    fn poll(&mut self) -> Option<RulesDone> {
        self.done.lock().unwrap().pop_front()
    }
}

/// A machine of [`Pair`]: menu without frames, network session, installation.
struct Machine {
    app: MenuApp,
    session: NetSession,
    vfs: Vfs,
}

/// Host Anna (machine 0) and client Bert (machine 1) on the corpus over a `MemNet`, without the menu flow: the
/// sessions are pumped directly, application commands stay in the queue.
struct Pair {
    net: opennml_net::MemNet,
    machines: Vec<Machine>,
    texts: Texts,
    now: u64,
    _dir: tempfile::TempDir,
}

impl Pair {
    /// Both machines on the corpus; the host's rules come from `host_worker`, the client's from an [`InlineWorker`].
    fn corpus(host_worker: Box<dyn RulesWorker>) -> Option<Self> {
        let vfs = corpus_vfs()?;
        let layer = crate::ui_layer::UiLayer::new(&vfs, 1024, None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let net = opennml_net::MemNet::new();
        let mut workers: Vec<Box<dyn RulesWorker>> =
            vec![host_worker, Box::new(InlineWorker::default())];
        let mut machines = Vec::new();
        for (i, name) in ["Anna", "Bert"].into_iter().enumerate() {
            let opts = opennml_ui::app::MenuOptions {
                start_menu: false,
                ..opennml_ui::app::MenuOptions::default()
            };
            let config = crate::testutil::test_config(&vfs);
            let mut app = MenuApp::new(&vfs, &layer.texts, config, opts).unwrap();
            let units: Vec<u16> = name.encode_utf16().collect();
            app.set_net_name(&units);
            let identity = NetIdentity {
                hello: Hello {
                    name: name.into(),
                    ..test_hello()
                },
                port: 2302,
                seed: Box::new(|| 1),
                listen: true,
            };
            let transport = opennml_net::MemTransport::new(&net, 10 * i as u64);
            let mut session = NetSession::new(
                Box::new(transport),
                identity,
                crate::testutil::net_paths(dir.path(), i),
            );
            session.set_rules_worker(workers.remove(0));
            machines.push(Machine {
                app,
                session,
                vfs: vfs.clone(),
            });
        }
        Some(Self {
            net,
            machines,
            texts: layer.texts,
            now: 0,
            _dir: dir,
        })
    }

    fn frame(&mut self) {
        self.now += 16;
        self.net.advance_to(self.now);
        for m in &mut self.machines {
            m.session.pump(self.now, &mut m.app, &m.vfs, &self.texts);
        }
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// Installation of machine `i`: the game file `game_path` changed by `edit` as a loose file of its own (shadows
    /// the archive entry); the other machine keeps its own. Call before the rules of the session are computed.
    fn change_file(&mut self, i: usize, game_path: &str, edit: impl FnOnce(&mut Vec<u8>)) {
        let m = &mut self.machines[i];
        let mut bytes = m.vfs.read(game_path).expect("game file");
        edit(&mut bytes);
        let disk = self._dir.path().join(format!("install-{i}"));
        std::fs::create_dir_all(&disk).expect("folder");
        let disk = disk.join(game_path.rsplit('/').next().expect("file name"));
        std::fs::write(&disk, &bytes).expect("loose file");
        m.vfs = opennml_rda::testutil::with_loose_file(&m.vfs, game_path, &disk);
    }

    /// Free text of the last message of machine `i` and the number of `MSGBOX` openings in its queue (the menu of
    /// [`Pair`] runs no frames: every message stays queued and counts as open).
    fn messages(&self, i: usize) -> (Option<String>, usize) {
        let app = &self.machines[i].app;
        let opened = app
            .queue()
            .iter()
            .filter(|c| matches!(c, AppCmd::Open { name, .. } if name == "msgbox"))
            .count();
        (app.screen_state().info_free.clone(), opened)
    }

    fn frames_until(&mut self, what: &str, cond: impl Fn(&Self) -> bool) {
        for _ in 0..2_000 {
            if cond(self) {
                return;
            }
            self.frame();
        }
        panic!("{what}: not reached");
    }

    fn host_core(&self) -> &HostCore {
        self.machines[0]
            .session
            .driver()
            .and_then(NetDriver::host_core)
            .expect("core of the host")
    }

    fn host_core_mut(&mut self) -> &mut HostCore {
        self.machines[0]
            .session
            .driver_mut()
            .and_then(NetDriver::host_core_mut)
            .expect("core of the host")
    }

    /// Lobby state of the host's core.
    fn host_lobby(&self) -> LobbyState {
        self.host_core().lobby().clone()
    }

    /// Verdict of the client's row on the host.
    fn client_data(&self) -> DataState {
        self.host_lobby().rows[1].data.clone()
    }

    /// Fixed 2-player maps of the catalog with `.lvl` and `.wld`, the small "Tintebariver" first.
    fn two_player_maps(&self) -> Vec<MapCatalogEntry> {
        let mut maps: Vec<MapCatalogEntry> = self.machines[0]
            .app
            .data()
            .maps
            .maps
            .iter()
            .filter(|m| m.level_read && m.world_read && m.players() == 2)
            .cloned()
            .collect();
        maps.sort_by_key(|m| !m.entry.path.to_lowercase().contains("tinte"));
        maps
    }

    /// The host creates the session on the fixed map `map` (`{8}` with the draft of `LAN3`).
    fn host(&mut self, map: &MapCatalogEntry) {
        let info = LevelInfoData {
            name: map.name.clone(),
            path: map.entry.path.clone(),
            size_class: map.size_class,
            players: map.players(),
            level_type: 0,
            ..LevelInfoData::default()
        };
        let m = &mut self.machines[0];
        m.app
            .set_lobby_draft(Vec::new(), info, DraftSettings::default());
        assert!(
            m.session
                .on_app_cmd(&AppCmd::Host, &mut m.app, &m.vfs, &self.texts)
        );
        assert!(m.session.is_host_lobby(), "hosting failed");
    }

    /// The client finds the game and joins, up to its row in the host's lobby.
    fn join(&mut self) {
        self.machines[1].session.search(true);
        self.frames_until("game in the list", |p| {
            !p.machines[1].session.list.games().is_empty()
        });
        let m = &mut self.machines[1];
        let game = m.session.list.games()[0].clone();
        m.session.join_game(&game, &mut m.app);
        self.frames_until("client in the lobby", |p| {
            p.machines[1].session.phase() == NetPhase::Lobby
                && p.host_lobby().rows[1].peer.is_some()
        });
    }

    /// Both ready (the host for its own row, the client by its wish); the core takes the client's only with `Equal`.
    fn ready(&mut self) {
        for (i, row) in [(0usize, 0u8), (1, 1)] {
            let m = &mut self.machines[i];
            let ready = LobbyAction::Ready { row, ready: true };
            m.session.on_lobby(&ready, &mut m.app, &m.vfs, &self.texts);
        }
        self.frames(20);
    }

    /// Start request of the host from its lobby state.
    fn host_request(&self) -> MatchRequest {
        request_from_lobby(&self.host_lobby(), &self.machines[0].app.data().maps)
    }

    /// The host switches the session to the fixed map `map` in its core; returns the new session revision.
    fn switch(&mut self, map: &MapCatalogEntry) -> u32 {
        let mut s = self.host_lobby().session;
        let path = map.entry.path.clone();
        s.map = MapId::Fixed {
            checksum: map_checksum(&self.machines[0].vfs, &path).expect("map checksum"),
            path,
        };
        s.map_name = map.name.clone();
        self.host_core_mut().set_session(s).expect("map change");
        self.frames(5);
        self.host_lobby().session_revision
    }
}

/// After `CREATE` the host computes the rules of the session and publishes their checksum with the session revision.
#[test]
fn host_publishes_its_checksum_after_create() {
    let Some(mut p) = Pair::corpus(Box::new(InlineWorker::default())) else {
        return;
    };
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    assert_eq!(
        p.host_lobby().rules_checksum,
        None,
        "before the first frame"
    );
    p.frames(2);
    let lobby = p.host_lobby();
    let rules = net_rules(&p.machines[0].vfs, &p.host_request()).unwrap();
    assert_eq!(lobby.rules_checksum, Some(rules.checksum));
    assert_eq!(lobby.session_revision, 0);
}

/// While the host computes, `START` stays locked and the client cannot become ready; with the host's checksum the
/// client reports, its row becomes `Equal`, and the start condition holds.
#[test]
fn start_waits_for_the_host_checksum() {
    let worker = ManualWorker::default();
    let Some(mut p) = Pair::corpus(Box::new(worker.clone())) else {
        return;
    };
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    p.join();
    p.ready();
    assert_eq!(worker.waiting(), [0]);
    assert_eq!(p.host_lobby().rules_checksum, None);
    assert_eq!(p.client_data(), DataState::Pending);
    assert!(
        !p.host_lobby().rows[1].ready,
        "Ready without Equal discarded"
    );
    assert!(!p.host_core().can_start());
    worker.release();
    p.frames_until("verdict of the client", |p| {
        p.client_data() == DataState::Equal
    });
    assert!(p.host_lobby().rules_checksum.is_some());
    assert!(!p.host_core().can_start(), "the client is not ready yet");
    p.ready();
    assert!(p.host_core().can_start());
}

/// The start takes the rules the host kept from the check: no file is read again.
#[test]
fn start_uses_the_kept_rules() {
    let Some(mut p) = Pair::corpus(Box::new(InlineWorker::default())) else {
        return;
    };
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    p.join();
    p.frames_until("verdict of the client", |p| {
        p.client_data() == DataState::Equal
    });
    p.ready();
    assert!(p.host_core().can_start());
    let checksum = p.host_lobby().rules_checksum.expect("checksum of the host");
    let (recording, log) = p.machines[0].vfs.recording();
    let maps = p.machines[0].app.data().maps.clone();
    let cfg = p.machines[0].app.config().clone();
    let driver = p.machines[0]
        .session
        .start_host_match(&maps, &cfg, &recording, &p.texts)
        .expect("start");
    assert!(log.take().is_empty(), "no rules computed at the start");
    let info = driver.start_info().expect("own start");
    assert_eq!(info.package.rules_checksum, checksum);
}

/// The client computes its rules at once, but sends its report only when the host's checksum for the same session
/// revision is in the state (an earlier report would be discarded by the core).
#[test]
fn client_waits_for_the_host_checksum() {
    let worker = ManualWorker::default();
    let Some(mut p) = Pair::corpus(Box::new(worker.clone())) else {
        return;
    };
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    p.join();
    p.frames(20);
    let held = p.machines[1]
        .session
        .held_report
        .as_ref()
        .map(|h| h.session_revision);
    assert_eq!(held, Some(p.host_lobby().session_revision), "result held");
    assert_eq!(p.client_data(), DataState::Pending);
    worker.release();
    p.frames_until("verdict of the client", |p| {
        p.client_data() == DataState::Equal
    });
    assert!(p.machines[1].session.held_report.is_none());
}

/// A second map change while the host still computes the first: the result for the first change is discarded, the
/// checksum and the verdict come only from the computation for the current session revision.
#[test]
fn second_change_discards_the_running_check() {
    let worker = ManualWorker::default();
    let Some(mut p) = Pair::corpus(Box::new(worker.clone())) else {
        return;
    };
    let maps = p.two_player_maps();
    assert!(maps.len() >= 3, "three 2-player maps");
    p.host(&maps[0]);
    p.join();
    worker.release();
    p.frames_until("first verdict", |p| p.client_data() == DataState::Equal);
    p.ready();
    assert!(p.host_core().can_start());
    let checksum = p.host_lobby().rules_checksum;
    let first = p.switch(&maps[1]);
    let second = p.switch(&maps[2]);
    assert_eq!(worker.waiting(), [first, second]);
    worker.release();
    p.frames(20);
    assert_eq!(
        p.host_lobby().rules_checksum,
        None,
        "result of the first change discarded"
    );
    assert_eq!(p.client_data(), DataState::Pending);
    assert!(!p.host_core().can_start());
    worker.release();
    p.frames_until("verdict for the second change", |p| {
        p.client_data() == DataState::Equal
    });
    let lobby = p.host_lobby();
    assert_eq!(lobby.session_revision, second);
    assert_ne!(lobby.rules_checksum, checksum, "checksum of the new map");
    let rules = net_rules(&p.machines[0].vfs, &p.host_request()).unwrap();
    assert_eq!(lobby.rules_checksum, Some(rules.checksum));
    assert!(p.host_core().can_start(), "ready flags stay (E-302)");
}

// --- Messages of the game data check (re/multiplayer-menus L.18) -----------------------------------------------------

use opennml_ui::own_texts::{
    DATA_DIFFERS, FILE_UNKNOWN, FILES_MORE, MAP_MISSING, ROW_DIFFERS, ROW_MAP_MISSING, fill,
};

/// Game file of the rules of every map (bonus values of the objects).
const BONUS_INI: &str = "data/gifcon/bonus.ini";

/// Changes the last digit of the file (a digit of a value, so that the file still parses).
fn bump_last_digit(bytes: &mut [u8]) {
    let at = bytes.iter().rposition(u8::is_ascii_digit).expect("a digit");
    bytes[at] = if bytes[at] == b'9' {
        b'8'
    } else {
        bytes[at] + 1
    };
}

fn differs(files: &[&str], more: u16) -> DataState {
    DataState::Differs {
        files: files.iter().map(|f| f.to_string()).collect(),
        more,
    }
}

/// Host Anna and client Bert on the small 2-player map, Bert with a changed `bonus.ini`; up to the verdict of Bert's
/// row.
fn bonus_pair() -> Option<Pair> {
    let mut p = Pair::corpus(Box::new(InlineWorker::default()))?;
    p.change_file(1, BONUS_INI, |b| bump_last_digit(b));
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    p.join();
    p.frames_until("verdict of the client", |p| {
        p.client_data() != DataState::Pending
    });
    assert_eq!(p.client_data(), differs(&[BONUS_INI], 0));
    p.frames(20);
    Some(p)
}

/// More lobby states of the same session revision: the host toggles its Ready flag a few times.
fn more_lobby_states(p: &mut Pair) {
    let revision = p.host_lobby().revision;
    let session = p.host_lobby().session_revision;
    for ready in [true, false, true] {
        let m = &mut p.machines[0];
        let wish = LobbyAction::Ready { row: 0, ready };
        m.session.on_lobby(&wish, &mut m.app, &m.vfs, &p.texts);
        p.frames(10);
    }
    let lobby = p.host_lobby();
    assert!(lobby.revision > revision, "new lobby states");
    assert_eq!(lobby.session_revision, session, "same session revision");
}

/// The differing client sees its files once per session revision ("OK" without a follow-up command, the lobby below
/// it), also after further lobby states; a new session revision with the same verdict brings one more.
#[test]
fn client_sees_its_differing_files_once() {
    let Some(mut p) = bonus_pair() else {
        return;
    };
    let expected = fill(&p.texts.display(DATA_DIFFERS), &[BONUS_INI]);
    assert_eq!(p.messages(1), (Some(expected.clone()), 1));
    assert!(p.machines[1].app.screen_state().info_ok.is_none());
    more_lobby_states(&mut p);
    assert_eq!(p.messages(1), (Some(expected), 1));
    assert!(p.machines[1].session.notices.waiting.is_empty());
    // A map change to another map with the same `bonus.ini` difference: one message for the new revision, waiting
    // behind the open one.
    let next = p.two_player_maps()[1].clone();
    p.switch(&next);
    p.frames_until("verdict for the new map", |p| {
        p.client_data() != DataState::Pending
    });
    p.frames(10);
    let waiting: Vec<DataState> = p.machines[1]
        .session
        .notices
        .waiting
        .iter()
        .map(|n| n.data.clone())
        .collect();
    assert_eq!(waiting, [differs(&[BONUS_INI], 0)]);
    assert_eq!(p.messages(1).1, 1, "waits for the OK");
}

/// The host sees the name of the row and its files once per session revision.
#[test]
fn host_sees_the_row_name_and_files_once() {
    let Some(mut p) = bonus_pair() else {
        return;
    };
    let expected = fill(&p.texts.display(ROW_DIFFERS), &["Bert", BONUS_INI]);
    assert_eq!(p.messages(0), (Some(expected.clone()), 1));
    assert!(p.machines[0].app.screen_state().info_ok.is_none());
    more_lobby_states(&mut p);
    assert_eq!(p.messages(0), (Some(expected), 1));
    assert!(p.machines[0].session.notices.waiting.is_empty());
    assert!(p.machines[0].session.is_host_lobby(), "the lobby stays");
}

/// The fixed map of the session is missing on the client: 99 105 there (own row), 99 112 with the name on the host.
#[test]
fn missing_map_shows_99_105_and_99_112() {
    let Some(mut p) = Pair::corpus(Box::new(InlineWorker::default())) else {
        return;
    };
    let map = p.two_player_maps()[0].clone();
    p.host(&map);
    p.join();
    p.frames_until("first verdict", |p| p.client_data() == DataState::Equal);
    p.frames(10);
    assert_eq!(p.messages(0), (None, 0), "no message with Equal");
    assert_eq!(p.messages(1), (None, 0), "no message with Equal");
    let mut s = p.host_lobby().session;
    s.map = MapId::Fixed {
        path: "multiplayer/gibtesnicht.lvl".into(),
        checksum: 1,
    };
    p.host_core_mut().set_session(s).expect("map change");
    p.frames_until("verdict Missing", |p| p.client_data() == DataState::Missing);
    p.frames(10);
    assert_eq!(p.messages(1), (Some(p.texts.display(MAP_MISSING)), 1));
    let host = fill(&p.texts.display(ROW_MAP_MISSING), &["Bert"]);
    assert_eq!(p.messages(0), (Some(host), 1));
}

/// Up to three paths with `, `, then " and %d more" with the number of the others - from `more` and from paths beyond
/// three in the list itself; `Pending` and `Equal` have no message.
#[test]
fn five_files_show_three_and_two_more() {
    let texts = texts();
    let three = differs(&["a.ini", "b.ini", "c.ini"], 2);
    let expected = fill(
        &texts.display(ROW_DIFFERS),
        &[
            "Bert",
            &format!(
                "a.ini, b.ini, c.ini{}",
                fill(&texts.display(FILES_MORE), &["2"])
            ),
        ],
    );
    assert_eq!(
        expected,
        "[CENTER]Die Spieldaten von Bert weichen ab: a.ini, b.ini, c.ini und 2 weitere. Die Partie kann so nicht \
         beginnen."
    );
    assert_eq!(
        data_message(&three, Some("Bert"), &texts),
        Some(expected.clone())
    );
    let five = differs(&["a.ini", "b.ini", "c.ini", "d.ini", "e.ini"], 0);
    assert_eq!(data_message(&five, Some("Bert"), &texts), Some(expected));
    let own = data_message(
        &differs(&["a.ini", "b.ini", "c.ini", "d.ini"], 1),
        None,
        &texts,
    );
    assert_eq!(
        own.as_deref(),
        Some(
            "[CENTER]Ihre Spieldaten für diese Karte weichen vom Host ab: a.ini, b.ini, c.ini und 2 weitere. Sie \
             können erst mitspielen, wenn die Dateien übereinstimmen."
        )
    );
    for state in [DataState::Pending, DataState::Equal] {
        assert_eq!(data_message(&state, Some("Bert"), &texts), None);
        assert_eq!(data_message(&state, None, &texts), None);
    }
}

/// `Differs` without a known file (e.g. a read recording beyond the limits of the wire): "file unknown".
#[test]
fn unknown_file_text_for_an_empty_list() {
    let texts = texts();
    let unknown = texts.display(FILE_UNKNOWN);
    assert_eq!(unknown, "Datei unbekannt");
    let empty = differs(&[], 0);
    assert_eq!(
        data_message(&empty, None, &texts),
        Some(fill(&texts.display(DATA_DIFFERS), &[&unknown]))
    );
    assert_eq!(
        data_message(&empty, Some("Bert"), &texts),
        Some(fill(&texts.display(ROW_DIFFERS), &["Bert", &unknown]))
    );
}

/// Paths come from a foreign read recording: every character outside printable ASCII becomes `?` (MSGBOX, O-61,
/// E-296), the name keeps the characters up to U+00FF; no panic.
#[test]
fn paths_are_ascii_in_the_message() {
    let texts = texts();
    let state = differs(
        &[
            "data/b\u{f6}nus.ini",
            "data/\u{7}x\ty.ini",
            "data/\u{65e5}\u{672c}.ini",
        ],
        0,
    );
    let text = data_message(&state, Some("Zo\u{eb} \u{674e}"), &texts).expect("message");
    assert_eq!(
        text,
        fill(
            &texts.display(ROW_DIFFERS),
            &["Zo\u{eb} ?", "data/b?nus.ini, data/?x?y.ini, data/??.ini"]
        )
    );
    assert!(text.chars().all(|c| u32::from(c) <= 0xFF), "{text}");
    // A long hostile path: shown as given (the wire limits the length), still without a panic.
    let long = format!("{}\u{1F600}", "x".repeat(300));
    let text = data_message(&differs(&[&long], u16::MAX), None, &texts).expect("message");
    assert!(text.contains(&format!("{}?", "x".repeat(300))), "{text}");
    assert!(text.contains("65535"), "{text}");
}

/// Lobby row of the tests of the message queue: a human on `peer` named `name` with the verdict `data`.
fn data_row(peer: u8, name: &str, data: DataState) -> LobbyRow {
    LobbyRow {
        kind: KIND_HUMAN,
        name: name.into(),
        peer: Some(peer),
        data,
        ..row(KIND_HUMAN, i32::from(peer), i32::from(peer))
    }
}

/// Lobby state with host Anna (row 0) and the rows `rows` behind it, for the session revision `session_revision`.
fn data_lobby(session_revision: u32, rows: Vec<LobbyRow>) -> LobbyState {
    let mut all = vec![data_row(HOST_PEER, "Anna", DataState::Equal)];
    all.extend(rows);
    LobbyState {
        revision: session_revision,
        session_revision,
        rules_checksum: Some(1),
        session: SessionInfo::plain("Round", "x.lvl", 3, TurnConfig::default()),
        rows: all,
        max_rtt_ms: None,
        seats: Vec::new(),
    }
}

/// Name and verdict of the next message to show (the box is closed).
fn next_shown(notices: &mut DataNotices) -> Option<(Option<String>, DataState)> {
    notices.next(false).map(|n| (n.name, n.data))
}

/// A client sees only the verdict of its own row, without a name; the host sees every row with a machine.
#[test]
fn other_clients_see_nothing() {
    let bert = differs(&[BONUS_INI], 0);
    let lobby = data_lobby(
        0,
        vec![
            data_row(1, "Bert", bert.clone()),
            data_row(2, "Carl", DataState::Equal),
        ],
    );
    let mut carl = DataNotices::default();
    carl.update(&lobby, Some(2), false);
    assert_eq!(next_shown(&mut carl), None);
    let mut bert_notices = DataNotices::default();
    bert_notices.update(&lobby, Some(1), false);
    assert_eq!(next_shown(&mut bert_notices), Some((None, bert.clone())));
    let mut host = DataNotices::default();
    host.update(&lobby, Some(0), true);
    assert_eq!(next_shown(&mut host), Some((Some("Bert".into()), bert)));
    assert_eq!(next_shown(&mut host), None);
}

/// Two differing rows on the host: one message after the other, each after the "OK" of the one before; a waiting
/// message whose row has another verdict in the meantime or is gone is dropped; no "OK" gets lost, and every row and
/// session revision has its message only once.
#[test]
fn two_differing_rows_show_two_messages_in_turn() {
    let bert = differs(&[BONUS_INI], 0);
    let carl = DataState::Missing;
    let both = |session: u32, b: &DataState, c: &DataState| {
        data_lobby(
            session,
            vec![
                data_row(1, "Bert", b.clone()),
                data_row(2, "Carl", c.clone()),
            ],
        )
    };
    let mut host = DataNotices::default();
    host.update(&both(0, &bert, &carl), Some(0), true);
    assert_eq!(
        next_shown(&mut host),
        Some((Some("Bert".into()), bert.clone()))
    );
    // Bert's message is open: Carl's waits, also over further lobby states.
    assert!(host.next(true).is_none());
    let mut later = both(0, &bert, &carl);
    later.revision = 5;
    host.update(&later, Some(0), true);
    assert!(host.next(true).is_none());
    // "OK": Carl's comes, then nothing more for this session revision.
    assert_eq!(
        next_shown(&mut host),
        Some((Some("Carl".into()), carl.clone()))
    );
    host.update(&later, Some(0), true);
    assert_eq!(next_shown(&mut host), None);
    // New session revision: Bert's comes; Carl's row becomes `Equal` before the "OK" - his message is dropped.
    host.update(&both(1, &bert, &carl), Some(0), true);
    assert_eq!(
        next_shown(&mut host),
        Some((Some("Bert".into()), bert.clone()))
    );
    host.update(&both(1, &bert, &DataState::Equal), Some(0), true);
    assert_eq!(next_shown(&mut host), None);
    // Next revision: Carl leaves (open row) before the "OK" of Bert's message - his message is dropped.
    host.update(&both(2, &bert, &carl), Some(0), true);
    assert_eq!(
        next_shown(&mut host),
        Some((Some("Bert".into()), bert.clone()))
    );
    let mut gone = both(2, &bert, &carl);
    gone.rows[2] = row(0, 2, 2);
    host.update(&gone, Some(0), true);
    assert_eq!(next_shown(&mut host), None);
    // A changed verdict of the same row and revision (another file) is the same row and revision: no second message.
    host.update(&both(3, &bert, &DataState::Equal), Some(0), true);
    assert_eq!(next_shown(&mut host), Some((Some("Bert".into()), bert)));
    host.update(
        &both(3, &differs(&["x.ini"], 0), &DataState::Equal),
        Some(0),
        true,
    );
    assert_eq!(next_shown(&mut host), None);
}
