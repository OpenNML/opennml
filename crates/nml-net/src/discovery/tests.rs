use super::*;
use crate::link::LinkEvent;
use crate::link::mem::MemNet;
use crate::lobby::SessionSettings;
use crate::msg::NetMessage;
use crate::transport::{MemTransport, SystemTransport, Transport};
use crate::turn::TurnConfig;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::time::{Duration, Instant};

fn hello(build: &str) -> Hello {
    Hello {
        listen_port: None,
        net_version: crate::wire::NET_VERSION,
        protocol: 43,
        format: 29,
        edition: "de".into(),
        name: "Anna".into(),
        build: build.into(),
        seat: None,
    }
}

fn fixed(path: &str) -> MapId {
    MapId::Fixed {
        path: path.into(),
        checksum: 0x1234_5678_9abc_def0,
    }
}

fn session(name: &str, map: MapId) -> SessionInfo {
    let mut s = SessionInfo::plain(name, "maps/x.lvl", 4, TurnConfig::default());
    s.map = map;
    s.map_name = "Test map".into();
    s.settings = SessionSettings {
        game_type: 1,
        unit_limit: 2,
        resources: 3,
        time_limit: 4,
        lock_teams: true,
        map_explored: false,
        save_replay: true,
    };
    s
}

fn ann(instance: u64, free: u8) -> Announcement {
    Announcement {
        instance,
        nonce: 77,
        versions: hello("test"),
        tcp_port: 2302,
        free,
        total: 3,
        session: session("Session - Anna", fixed("maps/x.lvl")),
    }
}

fn from(last: u8) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, last)), 50_000)
}

fn all_maps(_: &MapId) -> bool {
    true
}

/// Free port on 127.0.0.1 (UDP); never the fixed discovery port 2303.
fn free_udp_port() -> u16 {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.local_addr().unwrap().port()
}

/// Free port on 127.0.0.1 (TCP).
fn free_tcp_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn loopback_transport() -> SystemTransport {
    SystemTransport {
        loopback: true,
        discovery_port: free_udp_port(),
    }
}

#[test]
fn datagrams_round_trip() {
    let q = Datagram::Query { nonce: 0xDEAD_BEEF };
    let bytes = encode_datagram(&q).unwrap();
    assert_eq!(&bytes[..4], &DISCOVERY_MAGIC);
    assert_eq!(decode_datagram(&bytes).unwrap(), q);

    let mut a = ann(0x0102_0304_0506_0708, 2);
    a.session.map = MapId::Random {
        seed_text: "Seed".into(),
        size: 1,
        players: 4,
        terrain: 2,
        landscape: 3,
        daytime: 0,
    };
    let d = Datagram::Announce(Box::new(a));
    let bytes = encode_datagram(&d).unwrap();
    assert!(bytes.len() <= MAX_DATAGRAM);
    assert_eq!(decode_datagram(&bytes).unwrap(), d);
}

#[test]
fn bad_datagrams_are_ignored() {
    let good = encode_datagram(&Datagram::Announce(Box::new(ann(1, 1)))).unwrap();
    // Wrong magic.
    let mut b = good.clone();
    b[0] = b'X';
    assert!(matches!(decode_datagram(&b), Err(WireError::BadMagic)));
    // Wrong version.
    let mut b = good.clone();
    b[4] = b[4].wrapping_add(1);
    assert!(matches!(
        decode_datagram(&b),
        Err(WireError::Version { .. })
    ));
    // Truncated: every shorter length is an error, no panic.
    for n in 0..good.len() {
        assert!(decode_datagram(&good[..n]).is_err(), "length {n}");
    }
    // One byte too many after the body.
    let mut b = good.clone();
    b.push(0);
    assert!(decode_datagram(&b).is_err());
    // Oversized: 1,401 bytes, even with a valid header.
    let mut big = vec![0u8; MAX_DATAGRAM + 1];
    big[..4].copy_from_slice(&DISCOVERY_MAGIC);
    big[4..6].copy_from_slice(&crate::wire::NET_VERSION.to_le_bytes());
    assert!(decode_datagram(&big).is_err());
    // Garbage in the body with a matching length.
    let mut junk = Vec::new();
    junk.extend_from_slice(&DISCOVERY_MAGIC);
    junk.extend_from_slice(&crate::wire::NET_VERSION.to_le_bytes());
    junk.extend_from_slice(&5u16.to_le_bytes());
    junk.extend_from_slice(&[0xFF; 5]);
    assert!(decode_datagram(&junk).is_err());

    // Texts from the network are sanitized: player name to 32, session and map name to 39, without brackets.
    let mut a = ann(2, 1);
    a.versions.name = format!("[b]{}", "x".repeat(50));
    a.session.name = format!("[i]Session\u{0007}{}", "y".repeat(60));
    a.session.map_name = "[Map]".into();
    let bytes = encode_datagram(&Datagram::Announce(Box::new(a))).unwrap();
    let Datagram::Announce(got) = decode_datagram(&bytes).unwrap() else {
        panic!("announcement expected");
    };
    assert_eq!(got.versions.name, format!("b{}", "x".repeat(31)));
    assert_eq!(got.session.name.chars().count(), 39);
    assert!(got.session.name.starts_with("iSession y"));
    assert_eq!(got.session.map_name, "Map");
}

#[test]
fn oversized_announcement_is_not_sent() {
    let mut a = ann(3, 1);
    a.session.name = "S".repeat(10_000);
    assert_eq!(
        encode_datagram(&Datagram::Announce(Box::new(a.clone()))),
        None
    );
    // With the name limited to 39 characters it fits.
    a.session.name = crate::msg::clean_session_name(&a.session.name);
    assert_eq!(a.session.name.chars().count(), 39);
    let bytes = encode_datagram(&Datagram::Announce(Box::new(a))).unwrap();
    assert!(bytes.len() <= MAX_DATAGRAM);
}

#[test]
fn list_filters_like_the_original() {
    let own = hello("test");
    let mut list = SessionList::new();
    // Visible: free slot, same versions, map present.
    list.update(0, from(1), ann(1, 1), &own, all_maps);
    assert_eq!(list.games().len(), 1);
    // Full.
    list.update(0, from(2), ann(2, 0), &own, all_maps);
    // Different network version, protocol, format, game edition.
    let mut a = ann(3, 1);
    a.versions.net_version += 1;
    list.update(0, from(3), a, &own, all_maps);
    let mut a = ann(4, 1);
    a.versions.protocol += 1;
    list.update(0, from(4), a, &own, all_maps);
    let mut a = ann(5, 1);
    a.versions.format += 1;
    list.update(0, from(5), a, &own, all_maps);
    let mut a = ann(6, 1);
    a.versions.edition = "en".into();
    list.update(0, from(6), a, &own, all_maps);
    // Map missing locally (or different checksum).
    list.update(0, from(7), ann(7, 1), &own, |_| false);
    assert_eq!(
        list.games().iter().map(|g| g.instance).collect::<Vec<_>>(),
        vec![1]
    );
    // Random map: visible without a map check.
    let mut a = ann(8, 2);
    a.session.map = MapId::Random {
        seed_text: "x".into(),
        size: 0,
        players: 2,
        terrain: 0,
        landscape: 0,
        daytime: 0,
    };
    list.update(0, from(8), a, &own, |_| {
        panic!("random map without a map check")
    });
    // Order of the first response; address = sender IP + TCP port of the announcement; display = session name.
    let games = list.games();
    assert_eq!(games.iter().map(|g| g.instance).collect::<Vec<_>>(), [1, 8]);
    assert_eq!(games[0].addr, SocketAddr::new(from(1).ip(), 2302));
    assert_eq!(games[0].name, "Session - Anna");
    assert_eq!(list.find(8).unwrap().addr.ip(), from(8).ip());
    assert!(list.find(2).is_none());
}

/// A session from a network savegame is visible only if `map_ok` confirms the savegame (locally readable, same
/// checksum); a name that must not be a file name stays hidden without asking.
#[test]
fn saved_session_needs_map_ok() {
    let own = hello("test");
    let saved = MapId::Saved {
        name: "Netgame".into(),
        checksum: 42,
    };
    let with = |instance: u64, map: &MapId| {
        let mut a = ann(instance, 1);
        a.session.map = map.clone();
        a
    };
    let mut list = SessionList::new();
    list.update(0, from(1), with(1, &saved), &own, |m| m == &saved);
    list.update(0, from(2), with(2, &saved), &own, |_| false);
    let bad = MapId::Saved {
        name: "../Netgame".into(),
        checksum: 42,
    };
    list.update(0, from(3), with(3, &bad), &own, |_| {
        panic!("invalid name without checking the savegame")
    });
    assert_eq!(
        list.games().iter().map(|g| g.instance).collect::<Vec<_>>(),
        [1]
    );
    let reason = hide_reason(&with(2, &saved), &own, |_| false).expect("hidden");
    assert_eq!(reason, "Savegame \"Netgame\" is missing here or differs");
    assert!(hide_reason(&with(3, &bad), &own, all_maps).is_some());
}

#[test]
fn foreign_build_is_hidden() {
    let own = hello("0123456789ab");
    let mut list = SessionList::new();
    let mut a = ann(1, 1);
    a.versions.build = "ba9876543210-0badf00d".into();
    list.update(0, from(1), a, &own, all_maps);
    assert!(list.games().is_empty());
    // Same ID: visible.
    let mut a = ann(2, 1);
    a.versions.build = "0123456789ab".into();
    list.update(0, from(2), a, &own, all_maps);
    assert_eq!(list.games().len(), 1);
    // Two "unknown" match (with a warning in the log), "unknown" against a real ID does not.
    let unknown = hello(crate::msg::UNKNOWN_BUILD);
    let mut list = SessionList::new();
    let mut a = ann(3, 1);
    a.versions.build = crate::msg::UNKNOWN_BUILD.into();
    list.update(0, from(3), a.clone(), &unknown, all_maps);
    assert_eq!(list.games().len(), 1);
    let mut list = SessionList::new();
    list.update(0, from(3), a, &own, all_maps);
    assert!(list.games().is_empty());
}

#[test]
fn entries_expire_after_3s() {
    let own = hello("test");
    let mut list = SessionList::new();
    list.update(1_000, from(1), ann(1, 1), &own, all_maps);
    list.update(1_500, from(2), ann(2, 1), &own, all_maps);
    assert_eq!(list.find(1).unwrap().expires_ms, 1_000 + ENTRY_TTL_MS);
    list.expire(3_999);
    assert_eq!(list.games().len(), 2);
    list.expire(4_000);
    assert_eq!(
        list.games().iter().map(|g| g.instance).collect::<Vec<_>>(),
        [2]
    );
    // Every response extends it.
    list.update(4_400, from(2), ann(2, 1), &own, all_maps);
    list.expire(4_500);
    assert_eq!(list.games().len(), 1);
    list.expire(7_399);
    assert_eq!(list.games().len(), 1);
    list.expire(7_400);
    assert!(list.games().is_empty());
}

#[test]
fn known_entry_is_rechecked() {
    let own = hello("test");
    let mut list = SessionList::new();
    list.update(0, from(1), ann(1, 1), &own, all_maps);
    list.update(0, from(2), ann(2, 1), &own, all_maps);
    // New response with a changed name: entry replaced, position stays.
    let mut a = ann(1, 2);
    a.session.name = "New".into();
    list.update(100, from(1), a, &own, all_maps);
    assert_eq!(list.games()[0].name, "New");
    assert_eq!(list.games()[0].announcement.free, 2);
    // E-301 deviation: the second response with a full session removes the known entry.
    list.update(200, from(1), ann(1, 0), &own, all_maps);
    assert_eq!(
        list.games().iter().map(|g| g.instance).collect::<Vec<_>>(),
        [2]
    );
    // Likewise a changed map that is missing locally.
    list.update(300, from(2), ann(2, 1), &own, |_| false);
    assert!(list.games().is_empty());
}

#[test]
fn loopback_query_finds_the_host() {
    let mut t = loopback_transport();
    let listener = t.listen(0).unwrap();
    let tcp_port = listener.port();
    let mut beacon = t.beacon().unwrap();
    let mut seeker = t.seeker().unwrap();
    seeker.query(4711);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    let mut queries = 0;
    while got.is_empty() {
        assert!(Instant::now() < deadline, "no announcement via 127.0.0.1");
        for (to, nonce) in beacon.poll_queries() {
            assert_eq!(nonce, 4711);
            queries += 1;
            let mut a = ann(42, 1);
            a.nonce = nonce;
            a.tcp_port = tcp_port;
            beacon.answer(to, &a);
        }
        got.extend(seeker.poll());
        std::thread::sleep(Duration::from_millis(5));
    }
    // Wait a little longer: it stays at one query and one announcement.
    std::thread::sleep(Duration::from_millis(100));
    queries += beacon.poll_queries().len();
    got.extend(seeker.poll());
    assert_eq!((queries, got.len()), (1, 1), "{got:?}");
    let (sender, a) = got.pop().unwrap();
    assert_eq!(a.nonce, 4711);
    let mut list = SessionList::new();
    list.update(0, sender, a, &hello("test"), all_maps);
    assert_eq!(
        list.find(42).unwrap().addr,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), tcp_port)
    );
}

#[test]
fn busy_discovery_port_keeps_the_lobby() {
    let mut t = loopback_transport();
    let _first = t.beacon().unwrap();
    // Second host on the same machine: the discovery port is occupied ...
    assert!(t.beacon().is_err());
    // ... listening on TCP still works (the lobby opens without an announcement).
    let listener = t.listen(0).unwrap();
    assert_ne!(listener.port(), 0);
}

#[test]
fn connect_does_not_block() {
    let mut t = loopback_transport();
    // Nobody is listening: `poll` returns `None` at once, `Err` after the deadline.
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), free_tcp_port());
    let started = Instant::now();
    let mut c = t.connect(addr, 300);
    assert!(c.poll().is_none());
    assert!(started.elapsed() < Duration::from_millis(100));
    let result = loop {
        if let Some(r) = c.poll() {
            break r;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "hangs");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(result.is_err());
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(c.poll().is_none(), "the result arrives exactly once");

    // To its own `listen`: `Ok`, both sides carry messages.
    let mut listener = t.listen(0).unwrap();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listener.port());
    let mut c = t.connect(addr, 2_000);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = loop {
        if let Some(r) = c.poll() {
            break r.unwrap();
        }
        assert!(Instant::now() < deadline, "no connection");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut host = loop {
        if let Some(link) = listener.accept_pending().pop() {
            break link;
        }
        assert!(Instant::now() < deadline, "not accepted");
        std::thread::sleep(Duration::from_millis(5));
    };
    client.send(NetMessage::Resume);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if host.poll() == vec![LinkEvent::Message(NetMessage::Resume)] {
            break;
        }
        assert!(Instant::now() < deadline, "message did not arrive");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn mem_transport_broadcasts() {
    let net = MemNet::new();
    let mut host_a = MemTransport::new(&net, 10);
    let mut host_b = MemTransport::new(&net, 10);
    let mut client = MemTransport::new(&net, 10);
    assert_ne!(host_a.ip(), host_b.ip());
    let mut beacon_a = host_a.beacon().unwrap();
    let mut beacon_b = host_b.beacon().unwrap();
    assert!(
        host_a.beacon().is_err(),
        "discovery port only once per machine"
    );
    let mut listener_a = host_a.listen(0).unwrap();
    let mut seeker = client.seeker().unwrap();

    // Broadcast to all beacons of the same `MemNet`, after the delay.
    seeker.query(9);
    net.advance_to(9);
    assert!(beacon_a.poll_queries().is_empty());
    net.advance_to(10);
    let qa = beacon_a.poll_queries();
    let qb = beacon_b.poll_queries();
    assert_eq!(qa.len(), 1);
    assert_eq!(qb.len(), 1);
    assert_eq!(qa[0].0.ip(), client.ip());
    assert_eq!(qa[0].1, 9);
    let mut a = ann(1, 1);
    a.tcp_port = listener_a.port();
    beacon_a.answer(qa[0].0, &a);
    beacon_b.answer(qb[0].0, &ann(2, 1));
    net.advance_to(20);
    let got = seeker.poll();
    assert_eq!(
        got.iter()
            .map(|(f, a)| (f.ip(), a.instance))
            .collect::<Vec<_>>(),
        [(host_a.ip(), 1), (host_b.ip(), 2)]
    );
    assert!(seeker.poll().is_empty());

    // Connect via the found address.
    let mut list = SessionList::new();
    for (f, a) in got {
        list.update(20, f, a, &hello("test"), all_maps);
    }
    let addr = list.find(1).unwrap().addr;
    let mut c = client.connect(addr, 1_000);
    assert!(c.poll().is_none(), "not until after the delay");
    net.advance_to(30);
    let mut link = c.poll().unwrap().unwrap();
    assert!(c.poll().is_none());
    let mut host = listener_a.accept_pending().pop().unwrap();
    link.send(NetMessage::Resume);
    net.advance_to(40);
    assert_eq!(host.poll(), vec![LinkEvent::Message(NetMessage::Resume)]);

    // Without a listener: `None` until the deadline, then `Err`.
    let nowhere = SocketAddr::new(host_b.ip(), 2302);
    let mut c = client.connect(nowhere, 500);
    net.advance_to(539);
    assert!(c.poll().is_none());
    net.advance_to(540);
    assert!(c.poll().unwrap().is_err());

    // Without a beacon no response; a dropped beacon frees the port.
    drop(beacon_b);
    seeker.query(10);
    net.advance_to(600);
    assert_eq!(beacon_a.poll_queries().len(), 1);
    assert!(host_b.beacon().is_ok());
}
