//! Tests of the network start without GPU: setup, port, address, build ID, the command-line path over `MemTransport`
//! (played time, no real waiting) and over TCP on `127.0.0.1`, the start distribution in row order and `net_status`
//! from all sources.

use std::time::{Duration, Instant};

use opennml_client::game::testworld;
use opennml_formats::config::LevelPlayer;
use opennml_net::{
    Announcement, MemNet, MemTransport, NET_VERSION, SystemTransport, UNKNOWN_BUILD,
};

use super::*;

/// Test rules as a multiplayer map with `n` `.lvl` slots 0 ... n - 1 (`level_path` with `/multiplayer/`).
fn rules(n: u8) -> Rules {
    let mut r = testworld::rules();
    r.level_path = "data/levels/multiplayer/test.lvl".into();
    for slot in 0..n {
        r.level.players.push(LevelPlayer {
            slot,
            name: format!("Lvl {slot}"),
            nation: 2,
            kind: 0,
            resources: vec![(0, 1000)],
        });
    }
    r
}

fn settings() -> NetSettings {
    NetSettings {
        resources: 3,
        unit_limit: 4,
        time_limit: 45,
        map_explored: true,
    }
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("Human {i}")).collect()
}

fn hello(name: &str) -> Hello {
    Hello {
        listen_port: None,
        net_version: NET_VERSION,
        protocol: 1,
        format: 1,
        edition: "test".into(),
        name: name.into(),
        build: "test".into(),
        seat: None,
    }
}

fn paths(dir: &Path, i: usize) -> NetPaths {
    let dir = dir.join(format!("machine{i}"));
    NetPaths {
        saves: dir.join("savegame"),
        netlog: dir.join("netlog"),
    }
}

/// Plan of the host: `players` humans and `ai` AI on a map with `places` slots, raster `raster`.
fn plan(places: u8, players: u8, ai: u8, raster: TurnConfig) -> HostPlan {
    HostPlan {
        rules: rules(places),
        settings: settings(),
        players,
        ai,
        seed: 77,
        session: "Session - Anna".into(),
        game_speed: 4,
        raster,
        map_checksum: 0x1234,
        words: opennml_net::LocalWords::default(),
    }
}

/// `NML_BUILD_ID` from `build.rs` is set - 12 hex characters of the commit, with changes `-` and 8 hex characters,
/// without Git "unbekannt"; the literal in `build.rs` is `opennml_net::UNKNOWN_BUILD`.
#[test]
fn build_id_is_set() {
    let id = build_id();
    assert!(!id.is_empty());
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    let valid = id == UNKNOWN_BUILD
        || match id.split_once('-') {
            None => hex(id, 12),
            Some((commit, diff)) => hex(commit, 12) && hex(diff, 8),
        };
    assert!(valid, "Build ID \"{id}\"");
    let script = include_str!("../../build.rs");
    assert!(
        script.contains(&format!("const UNKNOWN_BUILD: &str = \"{UNKNOWN_BUILD}\";")),
        "build.rs uses the same literal as opennml_net::UNKNOWN_BUILD"
    );
}

/// On 4 slots with N 2 and K 1 -> human, human, AI, closed; network flag, seed and the values of the settings of the
/// host.
// QUIRK E-262: deviation
#[test]
fn host_setup_fills_humans_then_ai_then_closed() {
    let setup = host_setup(&rules(4), &settings(), 2, 1, 0xDEAD_BEEF, &names(2)).unwrap();
    let kinds: Vec<SlotKind> = setup.lobby.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SlotKind::Human,
            SlotKind::Human,
            SlotKind::Ai,
            SlotKind::Closed
        ]
    );
    assert_eq!(setup.lobby[0].name, "Human 0");
    assert_eq!(setup.lobby[1].name, "Human 1");
    assert_eq!(
        setup.lobby[2].name, "Lvl 2",
        "AI keeps the name of the .lvl"
    );
    assert_eq!(setup.mode, 0);
    assert!(setup.network);
    assert_eq!(setup.seed, 0xDEAD_BEEF);
    assert!(setup.ki.enabled, "with an AI slot");
    assert_eq!(
        (
            setup.resource_setting,
            setup.living_space,
            setup.time_limit,
            setup.map_explored
        ),
        (3, 4, 45, true)
    );
    let without_ai = host_setup(&rules(4), &settings(), 2, 0, 1, &names(2)).unwrap();
    assert!(!without_ai.ki.enabled);
}

/// More humans and AI than slots, wrong number of names, not a multiplayer map: error messages.
#[test]
fn host_setup_rejects_too_many_players() {
    let err = host_setup(&rules(4), &settings(), 5, 0, 0, &names(5)).unwrap_err();
    assert!(err.contains("4 slots"), "{err}");
    let err = host_setup(&rules(4), &settings(), 3, 2, 0, &names(3)).unwrap_err();
    assert!(err.contains("too few"), "{err}");
    let err = host_setup(&rules(4), &settings(), 2, 0, 0, &names(1)).unwrap_err();
    assert!(err.contains("names"), "{err}");
    let mut campaign = rules(4);
    campaign.level_path = "data/levels/campaign/one.lvl".into();
    let err = host_setup(&campaign, &settings(), 2, 0, 0, &names(2)).unwrap_err();
    assert!(err.contains("not a multiplayer map"), "{err}");
}

/// `--host` without a value = `NetPort`, 0 = 2302, a value is used.
// QUIRK E-254: deviation
#[test]
fn host_port_follows_net_port_and_2302() {
    assert_eq!(host_port(None, 0), 2302);
    assert_eq!(host_port(None, 23020), 23020);
    assert_eq!(host_port(None, -5), 2302);
    assert_eq!(host_port(None, 70_000), 2302);
    assert_eq!(host_port(Some(0), 23020), 2302);
    assert_eq!(host_port(Some(4711), 23020), 4711);
}

// QUIRK E-254: deviation
#[test]
fn join_addr_defaults_to_the_game_port() {
    assert_eq!(
        parse_join_addr("127.0.0.1:23020").unwrap(),
        "127.0.0.1:23020".parse().unwrap()
    );
    assert_eq!(
        parse_join_addr("192.168.1.5").unwrap(),
        "192.168.1.5:2302".parse().unwrap()
    );
    assert_eq!(
        parse_join_addr("[::1]").unwrap(),
        "[::1]:2302".parse().unwrap()
    );
    assert_eq!(parse_join_addr("localhost:4711").unwrap().port(), 4711);
    assert!(parse_join_addr("1.2.3.4:99999").is_err());
}

/// One pass of all `launches` at the played time `t`.
fn step(net: &MemNet, t: u64, launches: &mut [&mut NetLaunch]) {
    net.advance_to(t);
    for launch in launches.iter_mut() {
        launch.pump(t);
    }
}

/// Without a host, `--join` gives up after [`JOIN_TIMEOUT_MS`] = 10 s (deviates from E-288) - not with a program
/// end, but with a log entry and a free main menu; `net_status` reports `ended` with the reason. Until then the join
/// waits in the event loop (phase `connecting`, input locked). Played time over `MemTransport`: the test does not
/// wait for real.
// QUIRK E-288: deviation
#[test]
fn join_without_host_returns_to_the_main_menu() {
    assert_eq!(JOIN_TIMEOUT_MS, 10_000);
    let started = Instant::now();
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    // A machine without a listener.
    let ghost = SocketAddr::new(MemTransport::new(&net, 5).ip(), 2302);
    let mut client = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        ghost,
        hello("Guest"),
        paths(dir.path(), 1),
        true,
    );
    let mut gave_up = None;
    for t in (0..=12_000).step_by(100) {
        step(&net, t, &mut [&mut client]);
        if client.failure().is_some() {
            gave_up = Some(t);
            break;
        }
        assert!(client.waiting(), "waiting at {t} ms");
        let status = status_data(&client.source().unwrap(), None);
        assert_eq!(
            (status.role, status.phase),
            (Some(NetRoleName::Client), NetPhaseName::Connecting)
        );
        assert_eq!(status.port, Some(2302));
        let guard = net_guard(
            CliState {
                waiting: client.waiting(),
                handed: client.handed(),
            },
            None,
            Some(NetPhase::Idle),
        );
        assert!(guard.locked, "input locked until the start");
    }
    let t = gave_up.expect("gave up");
    assert!((10_000..10_200).contains(&t), "after 10 s: {t} ms");
    let text = client.failure().unwrap().to_string();
    assert!(text.contains("No host reachable at"), "{text}");
    assert!(text.contains(&ghost.to_string()), "{text}");
    // Main menu with free input, the network no longer locked.
    assert!(!client.waiting());
    assert!(client.driver().is_none());
    let guard = net_guard(
        CliState {
            waiting: client.waiting(),
            handed: client.handed(),
        },
        None,
        Some(NetPhase::Idle),
    );
    assert_eq!(guard, NetGuard::default());
    let status = status_data(&client.source().unwrap(), None);
    assert_eq!(status.phase, NetPhaseName::Ended);
    assert_eq!(status.failure.as_deref(), Some(text.as_str()));
    assert!(!status.started);
    // Further passes change nothing.
    step(&net, 13_000, &mut [&mut client]);
    assert_eq!(client.failure(), Some(text.as_str()));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "without real waiting: {:?}",
        started.elapsed()
    );
}

/// The command-line host creates the lobby from the setup (one row per human, itself immediately ready), the clients
/// set themselves ready after the first lobby state, and the host starts without a click via `start_from_lobby` as
/// soon as all `N` humans are there and ready - not earlier. Slots in row order, raster of the host; a late machine
/// finds no host any more and returns to the main menu.
// QUIRK E-298: deviation
#[test]
fn cli_host_starts_when_full() {
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let host_transport = MemTransport::new(&net, 5);
    let addr = SocketAddr::new(host_transport.ip(), 2302);
    let raster = TurnConfig::new(2, 1).unwrap();
    let mut host = NetLaunch::host(
        Box::new(host_transport),
        2302,
        hello("Anna"),
        plan(4, 3, 1, raster),
        paths(dir.path(), 0),
    )
    .unwrap();
    assert_eq!(host.port(), 2302);
    // State before the first join: lobby with three rows, row 0 ready.
    let status = status_data(&host.source().unwrap(), None);
    assert_eq!(
        (status.role, status.phase),
        (Some(NetRoleName::Host), NetPhaseName::Gathering)
    );
    assert_eq!((status.spacing, status.lead), (Some(2), Some(1)));
    assert_eq!(status.build, build_id());
    assert_eq!(status.players, Some(3));
    let lobby = status.lobby.expect("lobby state");
    assert_eq!(lobby.session, "Session - Anna");
    assert_eq!(lobby.map, "data/levels/multiplayer/test.lvl");
    assert_eq!(lobby.stage, "lan_fast");
    assert_eq!(lobby.own_row, Some(0));
    assert_eq!(lobby.rows.len(), 3, "one row per human, not the AI");
    assert_eq!(
        (
            lobby.rows[0].kind,
            lobby.rows[0].name.as_str(),
            lobby.rows[0].ready
        ),
        (KIND_HUMAN, "Anna", true)
    );
    assert_eq!((lobby.rows[1].kind, lobby.rows[1].peer), (0, None));
    let mut bert = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        addr,
        hello("Bert"),
        paths(dir.path(), 1),
        true,
    );
    let mut cleo = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        addr,
        hello("Cleo"),
        paths(dir.path(), 2),
        true,
    );
    // Only Bert: he becomes ready, but the host does not start.
    let mut t = 0;
    while t < 2_000 {
        step(&net, t, &mut [&mut host, &mut bert]);
        t += 10;
    }
    let core = host.driver().and_then(NetDriver::host_core).unwrap();
    assert_eq!(core.phase(), &Phase::Gathering, "not all there yet");
    let row = &core.lobby().rows[1];
    assert_eq!(
        (row.kind, row.name.as_str(), row.ready),
        (KIND_HUMAN, "Bert", true)
    );
    assert!(bert.driver().unwrap().start_info().is_none());
    let status = status_data(&bert.source().unwrap(), None);
    assert_eq!(status.phase, NetPhaseName::Gathering);
    assert_eq!(status.lobby.as_ref().and_then(|l| l.own_row), Some(1));
    // Cleo added: start without a click.
    while t < 10_000
        && !(bert.driver().is_some_and(|d| d.start_info().is_some())
            && cleo.driver().is_some_and(|d| d.start_info().is_some()))
    {
        step(&net, t, &mut [&mut host, &mut bert, &mut cleo]);
        t += 10;
    }
    let infos: Vec<StartInfo> = [&host, &bert, &cleo]
        .iter()
        .map(|l| l.driver().unwrap().start_info().cloned().expect("start"))
        .collect();
    let slots: Vec<u8> = infos.iter().map(|i| i.your_slot).collect();
    assert_eq!(slots, vec![0, 1, 2], "slots in row order");
    for info in &infos {
        assert_eq!((info.spacing, info.lead), (2, 1), "raster of the host");
        assert_eq!(info.package, infos[0].package);
        assert_eq!(info.package.setup.seed, 77);
        assert_eq!(info.session, "Session - Anna");
        assert_eq!(info.seats[0].name, "Anna");
        assert_eq!(info.seats[1].name, "Bert");
        assert_eq!(info.seats[2].name, "Cleo");
        assert_eq!(
            info.seats[3],
            Seat {
                peer: None,
                name: "Lvl 3".into()
            }
        );
    }
    let status = status_data(&host.source().unwrap(), None);
    assert_eq!(status.phase, NetPhaseName::Loading);
    assert!(status.lobby.is_none(), "no lobby state after the start");
    // A late machine: the host no longer listens -> back to the main menu after the deadline.
    let mut dora = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        addr,
        hello("Dora"),
        paths(dir.path(), 3),
        true,
    );
    let until = t + 11_000;
    while t < until && dora.failure().is_none() {
        step(&net, t, &mut [&mut host, &mut dora]);
        t += 100;
    }
    assert!(dora.failure().is_some_and(|f| f.contains("No host")));
    // Each driver goes to loading exactly once.
    assert!(host.take_ready().is_some());
    assert!(host.take_ready().is_none());
    assert!(host.handed() && !host.waiting());
    assert!(
        host.source().is_none(),
        "afterwards the match supplies the state"
    );
}

/// In the command-line path the client has loaded (driver in loading, `Loaded` out), then the host aborts before `Go`.
/// The match driver reports the failure, the input becomes free (`match_state`), and the driver ends with a message as
/// soon as the match computes it (the `NetFinish` and the way to the main menu are checked by
/// `net_driver::tests::host_leaving_before_go_finishes_the_loaded_match` and
/// `game_flow::net_tests::host_abort_after_loading_before_go_returns_to_the_main_menu`; the flow is the same
/// `GameFlow` for both paths).
#[test]
fn cli_host_abort_before_go_frees_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let host_transport = MemTransport::new(&net, 5);
    let addr = SocketAddr::new(host_transport.ip(), 2302);
    let mut host = NetLaunch::host(
        Box::new(host_transport),
        2302,
        hello("Anna"),
        plan(2, 2, 0, TurnConfig::default()),
        paths(dir.path(), 0),
    )
    .unwrap();
    let mut bert = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        addr,
        hello("Bert"),
        paths(dir.path(), 1),
        true,
    );
    let mut t = 0;
    while t < 10_000 && bert.driver().is_none_or(|d| d.start_info().is_none()) {
        step(&net, t, &mut [&mut host, &mut bert]);
        t += 10;
    }
    // The client loads (the driver goes to the match) and reports `Loaded`; the host itself has not loaded yet.
    let mut driver = bert.take_ready().expect("start at the client");
    driver.loaded(None);
    let guard = |d: &NetDriver| {
        net_guard(
            CliState {
                waiting: bert.waiting(),
                handed: bert.handed(),
            },
            match_state(Some(d)),
            Some(NetPhase::Idle),
        )
    };
    assert!(guard(&driver).locked, "locked until Go");
    host.driver_mut()
        .expect("driver of the host")
        .leave("Check run: host aborts");
    let until = t + 2_000;
    while t < until && driver.failure().is_none() {
        step(&net, t, &mut [&mut host]);
        driver.pump(t);
        t += 10;
    }
    let text = driver.failure().expect("abort at the client").to_string();
    assert!(text.contains("The host has ended the connection"), "{text}");
    assert!(!driver.started());
    assert_eq!(match_state(Some(&driver)), Some(true));
    assert!(
        !guard(&driver).locked,
        "input free for the OK of the message"
    );
    assert_eq!(match_state(None), None);
}

/// Slots in row order - host on the first active slot, clients afterwards; AI without a machine with the lobby name,
/// closed ones empty. Each machine gets its slot, all the same package with seed and network flag.
// QUIRK E-262: deviation
#[test]
fn start_infos_follow_join_order() {
    let plan = plan(4, 3, 1, TurnConfig::default());
    let peers = vec![
        (0, "Anna".to_string()),
        (2, "Bert".to_string()),
        (5, "Cleo".to_string()),
    ];
    let infos = start_infos(&plan, &peers).unwrap();
    let slots: Vec<(Peer, u8)> = infos.iter().map(|(p, i)| (*p, i.your_slot)).collect();
    assert_eq!(slots, vec![(0, 0), (2, 1), (5, 2)]);
    let seats = &infos[0].1.seats;
    assert_eq!(
        seats[1],
        Seat {
            peer: Some(2),
            name: "Bert".into()
        }
    );
    assert_eq!(
        seats[3],
        Seat {
            peer: None,
            name: "Lvl 3".into()
        }
    );
    assert_eq!(
        seats[4],
        Seat {
            peer: None,
            name: String::new()
        }
    );
    for (_, info) in &infos {
        assert_eq!(info.package, infos[0].1.package);
        assert_eq!(info.seats, *seats);
        assert!(info.package.setup.network);
        assert_eq!(info.package.rules, plan.rules.checksums);
        assert_eq!((info.spacing, info.lead), (8, 4));
        assert_eq!(
            (info.game_speed, info.session.as_str()),
            (4, "Session - Anna")
        );
    }
    assert!(start_infos(&plan, &peers[..2]).is_err(), "too few names");
}

/// Run over TCP on loopback (`SystemTransport` with `loopback`): the host listens on a free port, the client connects
/// in the event loop; as soon as it is there and ready, the host distributes the start. Both then have their start
/// information and wait in `Loading` - no tick before `Go`.
// QUIRK E-254, E-298: deviation
#[test]
fn launch_gathers_and_starts_over_tcp() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = NetLaunch::host(
        Box::new(SystemTransport::new(true)),
        0,
        hello("Host"),
        plan(2, 2, 0, TurnConfig::from_turn_steps(3).unwrap()),
        paths(dir.path(), 0),
    )
    .unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", host.port()).parse().unwrap();
    let mut client = NetLaunch::client(
        Box::new(SystemTransport::new(true)),
        addr,
        hello("Guest\u{7}"),
        paths(dir.path(), 1),
        true,
    );
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let now = started.elapsed().as_millis() as u64;
        host.pump(now);
        client.pump(now);
        if client.driver().is_some_and(|d| d.start_info().is_some()) {
            break;
        }
        assert!(client.failure().is_none(), "{:?}", client.failure());
        std::thread::sleep(Duration::from_millis(5));
    }
    let info = client
        .driver()
        .unwrap()
        .start_info()
        .cloned()
        .expect("start at the client");
    assert_eq!(info.your_slot, 1);
    assert_eq!((info.spacing, info.lead), (12, 6), "raster of the host");
    assert_eq!(
        info.seats[1],
        Seat {
            peer: Some(1),
            name: "Guest".into()
        },
        "name cleaned"
    );
    let own = host
        .driver()
        .unwrap()
        .start_info()
        .cloned()
        .expect("start at the host");
    assert_eq!(own.your_slot, 0);
    assert_eq!(own.package, info.package);
    let status = status_data(&host.source().unwrap(), None);
    assert_eq!(
        (status.role, status.phase),
        (Some(NetRoleName::Host), NetPhaseName::Loading)
    );
    assert_eq!(status.port, Some(addr.port()));
    assert_eq!(status.players, Some(2));
    assert_eq!(status.peers.len(), 2);
    assert_eq!(status.seats.len(), 8);
    assert!(!status.started);
    assert!(host.take_ready().is_some());
    assert!(host.take_ready().is_none());
    assert!(client.take_ready().is_some());
}

/// Without a network session all network commands are an error "no network session"; with a session but without a
/// match, `net_status` and `net_debug drop` answer, the commands on the state do not, `net_save` not until after `Go`.
#[test]
fn net_status_without_session_is_an_error() {
    let commands = [
        Command::NetStatus,
        Command::NetPartHashes,
        Command::NetSave { name: "x".into() },
        Command::NetDebug {
            action: NetDebugAction::Corrupt,
        },
        Command::NetDebug {
            action: NetDebugAction::Drop,
        },
        Command::NetDebug {
            action: NetDebugAction::Mute,
        },
        Command::NetDebug {
            action: NetDebugAction::Unmute,
        },
    ];
    for command in &commands {
        assert_eq!(
            run_net(None, None, None, command),
            Err(NO_NET.to_string()),
            "{command:?}"
        );
    }
    let idle = NetSessionStatus {
        phase: NetPhase::Idle,
        is_host: false,
        games: vec![],
        lobby: None,
        own_row: None,
        rtt_ms: vec![],
        port: None,
        chat: vec![],
    };
    assert!(
        session_source(idle, None).is_none(),
        "network session of the menu path without search and lobby: no network session"
    );
    // Command-line client while connecting: state without a driver.
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let ghost = SocketAddr::new(MemTransport::new(&net, 5).ip(), 2302);
    let mut connecting = NetLaunch::client(
        Box::new(MemTransport::new(&net, 5)),
        ghost,
        hello("Guest"),
        paths(dir.path(), 2),
        true,
    );
    connecting.pump(0);
    let status = status_data(&connecting.source().unwrap(), None);
    let value = run_net(Some(status.clone()), None, None, &Command::NetStatus).unwrap();
    assert_eq!(value["role"], "client");
    assert_eq!(value["phase"], "connecting");
    let dropped = run_net(
        Some(status.clone()),
        None,
        None,
        &Command::NetDebug {
            action: NetDebugAction::Drop,
        },
    )
    .unwrap();
    assert_eq!(dropped["links"], 0);
    let muted = run_net(
        Some(status),
        None,
        None,
        &Command::NetDebug {
            action: NetDebugAction::Mute,
        },
    )
    .unwrap();
    assert_eq!(muted, json!({"action": "mute", "links": 0}));
    // Driver in the sign-on.
    let (to_host, _host_side) = net.pair(5);
    let mut driver = NetDriver::client(
        ClientCore::new(hello("Guest")),
        Box::new(to_host),
        paths(dir.path(), 1),
    );
    let snapshot = Some(status_data(
        &driver_source(
            &driver,
            LaunchInfo {
                port: Some(2302),
                players: None,
            },
        ),
        None,
    ));
    let value = run_net(
        snapshot.clone(),
        Some(&mut driver),
        None,
        &Command::NetStatus,
    )
    .unwrap();
    assert_eq!(value["role"], "client");
    assert_eq!(value["phase"], "gathering");
    assert_eq!(value["port"], 2302);
    assert_eq!(value["waiting"], "starting");
    assert_eq!(value["steps"], Value::Null);
    assert_eq!(value["build"], build_id());
    assert_eq!(
        run_net(
            snapshot.clone(),
            Some(&mut driver),
            None,
            &Command::NetPartHashes
        ),
        Err("no match".to_string())
    );
    let err = run_net(
        snapshot.clone(),
        Some(&mut driver),
        None,
        &Command::NetSave { name: "x".into() },
    )
    .unwrap_err();
    assert!(err.contains("running match"), "{err}");
    let muted = run_net(
        snapshot.clone(),
        Some(&mut driver),
        None,
        &Command::NetDebug {
            action: NetDebugAction::Mute,
        },
    )
    .unwrap();
    assert_eq!(muted, json!({"action": "mute", "links": 1}));
    let unmuted = run_net(
        snapshot.clone(),
        Some(&mut driver),
        None,
        &Command::NetDebug {
            action: NetDebugAction::Unmute,
        },
    )
    .unwrap();
    assert_eq!(unmuted["action"], "unmute");
    let dropped = run_net(
        snapshot,
        Some(&mut driver),
        None,
        &Command::NetDebug {
            action: NetDebugAction::Drop,
        },
    )
    .unwrap();
    assert_eq!(dropped["links"], 1);
    assert_eq!(
        driver.client_core().map(|c| c.phase().clone()),
        Some(Phase::Gathering)
    );
}

/// Announcement of a host with session `name`.
fn announcement(name: &str) -> Announcement {
    Announcement {
        instance: 0xDEAD_BEEF,
        nonce: 0,
        versions: hello("Anna"),
        tcp_port: 2302,
        free: 1,
        total: 1,
        session: SessionInfo::plain(
            name,
            "data/levels/multiplayer/m.lvl",
            2,
            TurnConfig::default(),
        ),
    }
}

/// `net_status` from the state of the network session of the menu path - search without a role with the found
/// games, lobby with state, raster, round-trip times and chat, connecting as a client.
#[test]
fn net_status_reports_search_and_lobby() {
    let addr: SocketAddr = "127.0.0.1:2302".parse().unwrap();
    let searching = NetSessionStatus {
        phase: NetPhase::Searching,
        is_host: false,
        games: vec![FoundGame {
            instance: 0xDEAD_BEEF,
            addr,
            name: "Session - Anna".into(),
            announcement: announcement("Session - Anna"),
            expires_ms: 3_000,
        }],
        lobby: None,
        own_row: None,
        rtt_ms: vec![],
        port: None,
        chat: vec![],
    };
    let status = status_data(&session_source(searching.clone(), None).unwrap(), None);
    assert_eq!((status.role, status.phase), (None, NetPhaseName::Searching));
    assert_eq!((status.spacing, status.lead), (None, None));
    assert!(status.lobby.is_none());
    assert_eq!(status.found.len(), 1);
    assert_eq!(status.found[0].instance, "00000000deadbeef");
    assert_eq!(status.found[0].name, "Session - Anna");
    assert_eq!(status.found[0].addr, "127.0.0.1:2302");
    let connecting = NetSessionStatus {
        phase: NetPhase::Connecting,
        ..searching.clone()
    };
    let status = status_data(&session_source(connecting, None).unwrap(), None);
    assert_eq!(
        (status.role, status.phase),
        (Some(NetRoleName::Client), NetPhaseName::Connecting)
    );
    assert!(status.found.is_empty(), "games only during the search");
    let book = LobbyBook::open(
        SessionInfo::plain(
            "Session - Anna",
            "data/levels/multiplayer/m.lvl",
            2,
            RasterStage::Lan.config(),
        ),
        "Anna",
    )
    .unwrap();
    let lobby = NetSessionStatus {
        phase: NetPhase::Lobby,
        is_host: true,
        games: vec![],
        lobby: Some(book.state().clone()),
        own_row: Some(0),
        rtt_ms: vec![(1, 15)],
        port: Some(2302),
        chat: vec!["Anna: Hello".into()],
    };
    let status = status_data(&session_source(lobby, None).unwrap(), None);
    assert_eq!(
        (status.role, status.phase),
        (Some(NetRoleName::Host), NetPhaseName::Lobby)
    );
    assert_eq!((status.spacing, status.lead), (Some(4), Some(2)));
    assert_eq!(status.port, Some(2302));
    assert_eq!(status.rtt_ms, vec![NetRttData { peer: 1, ms: 15 }]);
    let lobby = status.lobby.expect("lobby state");
    assert_eq!(lobby.stage, "lan");
    assert_eq!(lobby.map, "data/levels/multiplayer/m.lvl");
    assert_eq!(lobby.own_row, Some(0));
    assert_eq!(lobby.rows.len(), 2);
    assert_eq!(lobby.rows[0].peer, Some(0));
    assert_eq!(lobby.chat, vec!["Anna: Hello".to_string()]);
    assert_eq!(stage_name(8, 4), "original");
    assert_eq!(stage_name(2, 1), "lan_fast");
    assert_eq!(stage_name(12, 6), "12/6");
}

/// `net_status` of a loaded lobby (host on slot 3, row 1) reports the savegame name (`lobby.saved`, also as map name)
/// and per row the player slot (`rows[].seat`); a new lobby reports both as `None`.
#[test]
fn net_status_shows_the_loaded_lobby() {
    let mut session = SessionInfo::plain("Session - Anna", "netz1", 3, RasterStage::Lan.config());
    session.map = MapId::Saved {
        name: "netz1".into(),
        checksum: 0xABCD,
    };
    let rows = [1, 3, 4]
        .map(|seat: u8| LobbyRow {
            kind: 0,
            name: String::new(),
            nation: 1,
            team: i32::from(seat % 2),
            handicap: 0,
            color: i32::from(seat),
            ready: false,
            peer: None,
            data: DataState::Equal,
        })
        .to_vec();
    let book = LobbyBook::loaded(session, rows, vec![1, 3, 4], 3, "Anna").unwrap();
    let lobby = NetSessionStatus {
        phase: NetPhase::Lobby,
        is_host: true,
        games: vec![],
        lobby: Some(book.state().clone()),
        own_row: Some(1),
        rtt_ms: vec![],
        port: Some(2302),
        chat: vec![],
    };
    let status = status_data(&session_source(lobby.clone(), None).unwrap(), None);
    let data = status.lobby.expect("lobby state");
    assert_eq!(data.saved.as_deref(), Some("netz1"));
    assert_eq!(data.map, "netz1");
    assert_eq!(data.own_row, Some(1));
    let rows: Vec<(u8, Option<u8>, Option<u8>, i32)> = data
        .rows
        .iter()
        .map(|r| (r.row, r.seat, r.peer, r.kind))
        .collect();
    assert_eq!(
        rows,
        [
            (0, Some(1), None, 0),
            (1, Some(3), Some(0), KIND_HUMAN),
            (2, Some(4), None, 0)
        ]
    );
    let value = serde_json::to_value(&data).unwrap();
    assert_eq!(value["saved"], "netz1");
    assert_eq!(value["rows"][2]["seat"], 4);

    // New lobby: no savegame, no slots.
    let fresh = LobbyBook::open(
        SessionInfo::plain(
            "Session - Anna",
            "data/levels/multiplayer/m.lvl",
            2,
            RasterStage::Lan.config(),
        ),
        "Anna",
    )
    .unwrap();
    let status = status_data(
        &session_source(
            NetSessionStatus {
                lobby: Some(fresh.state().clone()),
                own_row: Some(0),
                ..lobby
            },
            None,
        )
        .unwrap(),
        None,
    );
    let data = status.lobby.expect("lobby state");
    assert_eq!(data.saved, None);
    assert!(data.rows.iter().all(|r| r.seat.is_none()));
    let value = serde_json::to_value(&data).unwrap();
    assert_eq!(value["saved"], Value::Null);
}

/// `net_status` reports the game data check of the lobby: session revision, the host's rules checksum as hex text
/// (`null` while it computes) and the verdict per row in all four forms.
#[test]
fn net_status_shows_the_data_check() {
    let book = LobbyBook::open(
        SessionInfo::plain(
            "Session - Anna",
            "data/levels/multiplayer/m.lvl",
            4,
            RasterStage::Lan.config(),
        ),
        "Anna",
    )
    .unwrap();
    let mut state = book.state().clone();
    state.session_revision = 3;
    state.rules_checksum = Some(0x00AB_CDEF_0123_4567);
    let verdicts = [
        DataState::Equal,
        DataState::Pending,
        DataState::Missing,
        DataState::Differs {
            files: vec!["data/bonus.ini".into()],
            more: 2,
        },
    ];
    for (row, data) in state.rows.iter_mut().zip(verdicts) {
        row.data = data;
    }
    let source = |state: &LobbyState| NetSessionStatus {
        phase: NetPhase::Lobby,
        is_host: true,
        games: vec![],
        lobby: Some(state.clone()),
        own_row: Some(0),
        rtt_ms: vec![],
        port: Some(2302),
        chat: vec![],
    };
    let status = status_data(&session_source(source(&state), None).unwrap(), None);
    let value = serde_json::to_value(status.lobby.expect("lobby state")).unwrap();
    assert_eq!(value["session_revision"], 3);
    assert_eq!(value["rules_checksum"], "00abcdef01234567");
    let data: Vec<Value> = value["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["data"].clone())
        .collect();
    assert_eq!(
        data,
        [
            json!("equal"),
            json!("pending"),
            json!("missing"),
            json!({"differs": {"files": ["data/bonus.ini"], "more": 2}})
        ]
    );
    state.rules_checksum = None;
    let status = status_data(&session_source(source(&state), None).unwrap(), None);
    assert_eq!(status.lobby.unwrap().rules_checksum, None);
}

/// `--join` listens on a free port once connected (not the game port) and reports it in the `Hello`; the host
/// probes it, and the successor list in `Go` carries the address of the client. With `listen: false`
/// (`--net-no-listen`) the client reports no port and stands without an address in the list.
#[test]
fn cli_join_listens_on_a_free_port() {
    for listen in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let net = MemNet::new();
        let host_transport = MemTransport::new(&net, 5);
        let addr = SocketAddr::new(host_transport.ip(), 2302);
        let client_transport = MemTransport::new(&net, 5);
        let client_ip = client_transport.ip();
        let mut host = NetLaunch::host(
            Box::new(host_transport),
            2302,
            hello("Anna"),
            plan(2, 2, 0, TurnConfig::new(2, 1).unwrap()),
            paths(dir.path(), 0),
        )
        .unwrap();
        let mut bert = NetLaunch::client(
            Box::new(client_transport),
            addr,
            hello("Bert"),
            paths(dir.path(), 1),
            listen,
        );
        let mut t = 0;
        while t < 10_000
            && !(host.driver().is_some_and(|d| d.start_info().is_some())
                && bert.driver().is_some_and(|d| d.start_info().is_some()))
        {
            step(&net, t, &mut [&mut host, &mut bert]);
            t += 10;
        }
        let core = host.driver().and_then(NetDriver::host_core).unwrap();
        let port = core.listen_port(1);
        assert_eq!(port.is_some(), listen, "listen port in the Hello");
        assert!(
            port.is_none_or(|p| p != 0 && p != 2302),
            "free port: {port:?}"
        );
        // Both have loaded: `Go` with the list.
        host.driver_mut().unwrap().loaded(None);
        bert.driver_mut().unwrap().loaded(None);
        let list = |l: &NetLaunch| {
            l.driver()
                .and_then(NetDriver::client_core)
                .map(|c| c.successors().to_vec())
                .unwrap_or_default()
        };
        while t < 20_000 && list(&bert).is_empty() {
            step(&net, t, &mut [&mut host, &mut bert]);
            t += 10;
        }
        let list = list(&bert);
        assert_eq!(list.len(), 1, "listen = {listen}");
        assert_eq!(list[0].peer, 1);
        let expected = port.map(|p| SocketAddr::new(client_ip, p));
        assert_eq!(list[0].addr, expected, "listen = {listen}");
    }
}

/// Listener that reports a port on which nobody ever listens (test helper for a dead address).
struct DeadListener(u16);

impl opennml_net::Listener for DeadListener {
    fn accept_pending(&mut self) -> Vec<Box<dyn opennml_net::Link>> {
        Vec::new()
    }
    fn port(&self) -> u16 {
        self.0
    }
}

/// `MemTransport` whose `listen(0)` reports a dead port.
struct DeadPortTransport(MemTransport);

impl opennml_net::Transport for DeadPortTransport {
    fn listen(&mut self, _port: u16) -> std::io::Result<Box<dyn opennml_net::Listener>> {
        Ok(Box::new(DeadListener(45_678)))
    }
    fn connect(&mut self, addr: SocketAddr, timeout_ms: u64) -> Box<dyn opennml_net::Connecting> {
        self.0.connect(addr, timeout_ms)
    }
    fn beacon(&mut self) -> std::io::Result<Box<dyn opennml_net::Beacon>> {
        self.0.beacon()
    }
    fn seeker(&mut self) -> std::io::Result<Box<dyn opennml_net::Seeker>> {
        self.0.seeker()
    }
    fn fork(&self) -> Box<dyn opennml_net::Transport> {
        Box::new(DeadPortTransport(self.0.clone()))
    }
}

/// If a client reports a port on which nobody listens, the probe does not fail until the deadline
/// `SUCCESSOR_CONNECT_MS`; `Go` then comes anyway (shortly after the deadline, not before) with `addr: None`.
#[test]
fn go_after_a_dead_port_comes_after_the_probe_deadline_without_address() {
    let dir = tempfile::tempdir().unwrap();
    let net = MemNet::new();
    let host_transport = MemTransport::new(&net, 5);
    let addr = SocketAddr::new(host_transport.ip(), 2302);
    let mut host = NetLaunch::host(
        Box::new(host_transport),
        2302,
        hello("Anna"),
        plan(2, 2, 0, TurnConfig::new(2, 1).unwrap()),
        paths(dir.path(), 0),
    )
    .unwrap();
    let mut bert = NetLaunch::client(
        Box::new(DeadPortTransport(MemTransport::new(&net, 5))),
        addr,
        hello("Bert"),
        paths(dir.path(), 1),
        true,
    );
    let mut t = 0;
    while t < 10_000
        && !(host.driver().is_some_and(|d| d.start_info().is_some())
            && bert.driver().is_some_and(|d| d.start_info().is_some()))
    {
        step(&net, t, &mut [&mut host, &mut bert]);
        t += 10;
    }
    assert_eq!(
        host.driver()
            .and_then(NetDriver::host_core)
            .unwrap()
            .listen_port(1),
        Some(45_678)
    );
    host.driver_mut().unwrap().loaded(None);
    bert.driver_mut().unwrap().loaded(None);
    let list = |l: &NetLaunch| {
        l.driver()
            .and_then(NetDriver::client_core)
            .map(|c| c.successors().to_vec())
            .unwrap_or_default()
    };
    let loaded_at = t;
    while t < 20_000 && list(&bert).is_empty() {
        step(&net, t, &mut [&mut host, &mut bert]);
        t += 10;
    }
    let list = list(&bert);
    assert_eq!(list.len(), 1, "Go came");
    assert_eq!((list[0].peer, list[0].addr), (1, None));
    assert!(
        t >= opennml_net::SUCCESSOR_CONNECT_MS,
        "Go not before the deadline, at {t} ms (loaded at {loaded_at} ms)"
    );
    assert!(
        t < opennml_net::SUCCESSOR_CONNECT_MS + 1_500,
        "Go at {t} ms"
    );
}
