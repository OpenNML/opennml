//! Combined test of the host migration: inputs from outside that must never crash the core. One row per check; later
//! additions append their rows. Expected is "no panic, no hang", not a specific text.

use super::*;
use crate::msg::{HANDED_OVER, HOST_LEFT, MIGRATION_FAILED, Migrated, Rejoin};
use crate::wire::MAX_FRAME;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A client that has already loaded and waits for `Go`.
fn loading_client() -> ClientCore {
    let mut rig = Rig::build(TurnConfig::default(), &[0]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    let mut client = rig.clients.remove(0);
    client.loaded(None);
    client
}

fn successor(peer: u8) -> Successor {
    Successor {
        peer,
        slot: peer,
        addr: None,
    }
}

/// Sends the waiting client a foreign `Go` and checks that the match runs and the list is empty.
fn go_with(list: Vec<Successor>) {
    let mut client = loading_client();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Go { successors: list }),
    );
    assert_eq!(client.phase(), &Phase::Running);
    assert!(client.successors().is_empty());
}

#[test]
fn oversized_successor_list_is_dropped() {
    go_with((1..=9).map(successor).collect());
}

// ---------------------------------------------------------------------------------------------------------------------
// Client core: keeping, `HostGone`, `Rejoin`, `Migrated`, `Succession`.

/// Entry of the successor list with an address (candidate); slot = peer.
fn reachable(peer: u8) -> Successor {
    Successor {
        peer,
        slot: peer,
        addr: Some(addr(5000 + u16::from(peer))),
    }
}

/// A running client (peer 1, slot 1) with the successor list `list`; bundles come by hand. Outbox and events are
/// collected.
fn running_client(list: Vec<Successor>) -> ClientCore {
    let mut client = loading_client();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Go { successors: list }),
    );
    assert_eq!(client.phase(), &Phase::Running);
    client.events();
    client.outbox();
    client
}

/// Bundle `turn` with a distinguishable command of the host and the control entries `control`.
fn bundle(turn: u32, control: Vec<NetControl>) -> TurnBundle {
    TurnBundle {
        turn,
        commands: vec![cmd(0, turn as i32)],
        control,
    }
}

fn deliver(client: &mut ClientCore, b: TurnBundle) {
    client.handle(0, HOST_PEER, LinkEvent::Message(NetMessage::Bundle(b)));
}

/// The connection to the host closes.
fn lose_host(client: &mut ClientCore) {
    client.handle(0, HOST_PEER, LinkEvent::Closed("Connection gone".into()));
}

/// Turns of the contributions in the outbox.
fn turns(out: &[(Peer, NetMessage)]) -> Vec<u32> {
    out.iter()
        .filter_map(|(_, m)| match m {
            NetMessage::Turn(c) => Some(c.turn),
            _ => None,
        })
        .collect()
}

fn turns_of(contributions: &[TurnContribution]) -> Vec<u32> {
    contributions.iter().map(|c| c.turn).collect()
}

/// `Migrated` of the new host (peer 2) with the bundles `bundles`, without a halt.
fn migrated(bundles: Vec<TurnBundle>) -> Migrated {
    Migrated {
        host: 2,
        bundles,
        paused_by: None,
        successors: vec![reachable(1)],
        desyncs: 0,
    }
}

/// `HostGone` events of machine `i`: (reason, planned).
fn host_gone(rig: &Rig, i: usize) -> Vec<(String, bool)> {
    rig.events[i]
        .iter()
        .filter_map(|e| match e {
            CoreEvent::HostGone { reason, planned } => Some((reason.clone(), *planned)),
            _ => None,
        })
        .collect()
}

/// Rig with two listening, reachable clients in the running match.
fn rig_running_with_candidates() -> Rig {
    let mut rig = rig_loaded_with_listeners();
    rig.host.set_reachable(1, Some(addr(5001)));
    rig.host.set_reachable(2, Some(addr(5002)));
    rig.frames_until(|r| r.all_in(&Phase::Running));
    rig
}

/// After `Go` with a candidate in the list, the client reports the host's dropout as `HostGone` (phase stays); further
/// silence, `Closed` and `Bye` of the old connection trigger nothing more. Only `abandon` turns that into today's abort
/// (`Failed` with [`HOST_LOST`]).
#[test]
fn host_loss_after_go_is_host_gone_with_a_candidate() {
    let mut rig = rig_running_with_candidates();
    rig.run_until(7);
    for link in &rig.client_links {
        link.cut();
    }
    rig.run_for(500);
    for i in 1..3 {
        let gone = host_gone(&rig, i);
        assert_eq!(gone.len(), 1, "machine {i}: {gone:?}");
        assert!(!gone[0].1, "machine {i}: unplanned");
        assert!(failures(&rig, i).is_empty(), "machine {i}");
        assert!(rig.phase(i).in_match(), "machine {i}: {:?}", rig.phase(i));
    }
    // The old connection keeps reporting silence, a second `Closed` and a `Bye`: no second dropout.
    rig.run_for(TIMEOUT_MS + 1_000);
    assert_eq!(host_gone(&rig, 1).len(), 1);
    assert!(failures(&rig, 1).is_empty());
    let now = rig.now;
    let client = &mut rig.clients[0];
    client.silence(now, HOST_PEER, 2 * TIMEOUT_MS);
    client.handle(now, HOST_PEER, LinkEvent::Closed("once more".into()));
    client.handle(
        now,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Bye {
            reason: "late".into(),
        }),
    );
    assert!(client.events().is_empty());
    assert!(client.phase().in_match());
    // Giving up: today's abort.
    client.abandon();
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);
    assert_eq!(client.phase(), &Phase::Ended);
    client.abandon();
    assert!(client.events().is_empty(), "second abandon has no effect");
}

/// Without a candidate (no entry with an address) it stays with the abort as before, even after 30 s of silence.
#[test]
fn host_loss_without_candidate_still_fails() {
    // `hello()` reports no listening port: all entries without an address.
    let mut rig = Rig::running(&[20, 40]);
    rig.run_until(7);
    for link in &rig.client_links {
        link.cut();
    }
    rig.run_for(500);
    for i in 1..3 {
        assert_eq!(
            failures(&rig, i),
            vec![HOST_LOST.to_string()],
            "machine {i}"
        );
        assert!(host_gone(&rig, i).is_empty(), "machine {i}");
        assert_eq!(rig.phase(i), &Phase::Ended);
    }
    // Silence with a list without addresses.
    let mut client = running_client(vec![successor(1), successor(2)]);
    client.silence(0, HOST_PEER, TIMEOUT_MS);
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);
}

/// Text 15212 path (E-268 part "Lobby" adopted): before `Go` nothing changes, even if the clients are listening.
// QUIRK E-268: deviation
#[test]
fn host_loss_in_lobby_is_unchanged() {
    // Lobby after the `Welcome`: connection closed -> abort.
    let mut rig = Rig::build(TurnConfig::default(), &[0]);
    rig.clients[0] = ClientCore::new(hello_listening("Client 1", 5001));
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    let mut client = rig.clients.remove(0);
    client.events();
    lose_host(&mut client);
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);
    assert_eq!(client.phase(), &Phase::Ended);

    // The host leaves the lobby: `HostLeft` as before.
    let mut rig = Rig::build(TurnConfig::default(), &[0]);
    rig.clients[0] = ClientCore::new(hello_listening("Client 1", 5001));
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.host.leave(HOST_LEFT.into());
    rig.frames_until(|r| r.clients[0].phase() == &Phase::Ended);
    assert!(rig.has_event(1, |e| e == &CoreEvent::HostLeft));
    assert!(host_gone(&rig, 1).is_empty());

    // Loaded, `Go` still missing (probe results pending): abort as before.
    let mut rig = rig_loaded_with_listeners();
    for link in &rig.client_links {
        link.cut();
    }
    rig.run_for(500);
    for i in 1..3 {
        assert_eq!(
            failures(&rig, i),
            vec![HOST_LOST.to_string()],
            "machine {i}"
        );
        assert!(host_gone(&rig, i).is_empty(), "machine {i}");
    }
}

/// After a `Handover` in the bundle the dropout is planned, and the election is only among its `peers`; if there is
/// no candidate there, it stays with the abort.
#[test]
fn handover_marks_the_host_loss_as_planned() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    assert_eq!(client.handover(), None);
    deliver(
        &mut client,
        bundle(0, vec![NetControl::Handover { peers: vec![2] }]),
    );
    assert_eq!(client.handover(), Some(&[2u8][..]));
    client.events();
    client.handle(
        0,
        HOST_PEER,
        LinkEvent::Message(NetMessage::Bye {
            reason: HANDED_OVER.into(),
        }),
    );
    assert_eq!(
        client.events(),
        vec![CoreEvent::HostGone {
            reason: HANDED_OVER.into(),
            planned: true,
        }]
    );

    // `peers` without a candidate (peer 2 not reachable): no switch.
    let mut client = running_client(vec![reachable(1), successor(2)]);
    deliver(
        &mut client,
        bundle(0, vec![NetControl::Handover { peers: vec![2] }]),
    );
    client.events();
    lose_host(&mut client);
    assert_eq!(client.events(), vec![CoreEvent::Failed(HOST_LOST.into())]);
}

/// The client keeps the bundles and its open contribution; `Rejoin` carries them with `next_close`, `next_bundle` and
/// the desync count.
#[test]
fn rejoin_carries_kept_bundles_and_open_turns() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    let err = client.rejoin().expect_err("no Rejoin before the dropout");
    assert!(!err.is_empty());
    for k in 0..3 {
        client.submit(cmd(1, 10 + k as i32));
        client.close_round(k, 100 + u64::from(k));
        deliver(&mut client, bundle(k, Vec::new()));
    }
    client.submit(cmd(1, 13));
    client.close_round(3, 103);
    lose_host(&mut client);
    assert!(matches!(
        &client.events()[..],
        [CoreEvent::HostGone { planned: false, .. }]
    ));
    let r = client.rejoin().expect("Rejoin");
    assert_eq!(
        r,
        Rejoin {
            peer: 1,
            slot: 1,
            next_close: 4,
            next_bundle: 3,
            bundles: (0..3).map(|k| bundle(k, Vec::new())).collect(),
            open: vec![TurnContribution {
                turn: 3,
                hash: 103,
                commands: vec![cmd(1, 13)],
                wishes: Vec::new(),
            }],
            desyncs: 0,
        }
    );
}

/// `into_parts` gives the new host bundles, open contributions and the rest of the client's state.
#[test]
fn into_parts_hands_over_kept_bundles_and_open_turns() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    for k in 0..3 {
        client.close_round(k, u64::from(k));
        deliver(&mut client, bundle(k, Vec::new()));
    }
    client.close_round(3, 3);
    client.submit(cmd(1, 50));
    lose_host(&mut client);
    let parts = client.into_parts();
    assert_eq!(parts.bundles.keys().copied().collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(parts.bundles[&2], bundle(2, Vec::new()));
    assert_eq!(turns_of(&parts.sent), [3]);
    assert_eq!((parts.next_close, parts.next_bundle), (4, 3));
    assert_eq!((parts.peer, parts.slot), (Some(1), Some(1)));
    assert_eq!(parts.inputs.commands, [cmd(1, 50)]);
    assert_eq!(parts.successors, [reachable(1), reachable(2)]);
    assert_eq!(parts.phase, Phase::Running);
    assert_eq!(parts.cfg, TurnConfig::default());
    assert_eq!(parts.hello.name, "Client 1");
    assert!(parts.lobby.is_some());
    assert!(!parts.resync);
    assert_eq!(parts.pause_after, None);
    assert_eq!(parts.desyncs, 0);
    assert_eq!(parts.handover, None);
}

/// A contribution that the client closes between `rejoin` and `rehome` goes out only after the `Migrated`, and exactly
/// once; what was open before the `Rejoin` travels only in it.
// QUIRK E-348: deviation
#[test]
fn contribution_closed_during_migration_reaches_the_new_host() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(&mut client, bundle(0, Vec::new()));
    client.close_round(1, 1);
    assert_eq!(turns(&client.outbox()), [0, 1]);
    lose_host(&mut client);
    client.events();
    // After the dropout, before the `Rejoin`: closed, but not in the outbox.
    client.close_round(2, 2);
    assert!(turns(&client.outbox()).is_empty());
    let r = client.rejoin().expect("Rejoin");
    assert_eq!(turns_of(&r.open), [1, 2]);
    // After the `Rejoin`: held back.
    client.submit(cmd(1, 33));
    client.close_round(3, 3);
    assert!(turns(&client.outbox()).is_empty());
    // The new host still had bundle 1 (edge): contribution 1 is done, 2 came with the `Rejoin`, 3 goes out now.
    client
        .rehome(migrated(vec![bundle(1, Vec::new())]))
        .expect("rehome");
    let out = client.outbox();
    assert_eq!(turns(&out), [3]);
    assert!(matches!(
        &out[..],
        [(HOST_PEER, NetMessage::Turn(c))] if c.commands == [cmd(1, 33)]
    ));
    // Afterwards ordinary again: contribution immediately in the outbox, nothing twice.
    deliver(&mut client, bundle(2, Vec::new()));
    client.close_round(4, 4);
    assert_eq!(turns(&client.outbox()), [4]);
    assert!(
        client
            .events()
            .iter()
            .all(|e| !matches!(e, CoreEvent::Failed(_)))
    );
}

/// `rehome` inserts the missing bundles in sequence (control entries as in `on_bundle`), takes over the list and the
/// desync count and reports `Rehomed`; afterwards the bundles of the new host continue as usual.
// QUIRK E-348: deviation
#[test]
fn rehome_fills_missing_bundles_in_order() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(&mut client, bundle(0, Vec::new()));
    client.close_round(1, 1);
    lose_host(&mut client);
    client.events();
    assert_eq!(client.rejoin().expect("Rejoin").next_bundle, 1);
    let b1 = bundle(1, vec![NetControl::Speed(7)]);
    let b2 = bundle(2, Vec::new());
    client
        .rehome(Migrated {
            host: 2,
            bundles: vec![b1.clone(), b2.clone()],
            paused_by: None,
            successors: vec![reachable(1)],
            desyncs: 1,
        })
        .expect("rehome");
    assert_eq!(client.bundle(1), Some(&b1));
    assert_eq!(client.bundle(2), Some(&b2));
    assert_eq!(
        client.events(),
        vec![
            CoreEvent::Control {
                turn: 1,
                control: NetControl::Speed(7),
            },
            CoreEvent::Rehomed,
        ]
    );
    assert_eq!(client.desyncs(), 1);
    assert_eq!(client.successors(), [reachable(1)]);
    assert_eq!(client.phase(), &Phase::Running);
    // Second `rehome` without a host migration: rejected.
    assert!(client.rehome(migrated(Vec::new())).is_err());
    // Bundle 3 from the new host fits.
    deliver(&mut client, bundle(3, Vec::new()));
    assert!(client.bundle(3).is_some());
    assert!(
        client
            .events()
            .iter()
            .all(|e| !matches!(e, CoreEvent::Failed(_)))
    );
}

/// The phase after `rehome` follows `paused_by` of the new host; a running resync is dropped.
#[test]
fn rehome_takes_the_halt_of_the_new_host() {
    // The client was in the pause of slot 0 (old host); the new host lifts it.
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(&mut client, bundle(0, vec![NetControl::Halt { by: 0 }]));
    assert_eq!(client.phase(), &Phase::Halted(HaltReason::Pause { by: 0 }));
    lose_host(&mut client);
    client.events();
    client.rejoin().expect("Rejoin");
    client.rehome(migrated(Vec::new())).expect("rehome");
    assert_eq!(client.phase(), &Phase::Running);
    assert_eq!(
        client.events(),
        vec![CoreEvent::Rehomed, CoreEvent::Resumed]
    );

    // Running, the new host is in the pause of slot 2: the client halts.
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    lose_host(&mut client);
    client.events();
    client.rejoin().expect("Rejoin");
    client
        .rehome(Migrated {
            paused_by: Some(2),
            ..migrated(Vec::new())
        })
        .expect("rehome");
    assert_eq!(client.phase(), &Phase::Halted(HaltReason::Pause { by: 2 }));
    assert_eq!(client.events(), vec![CoreEvent::Rehomed]);

    // In the middle of a resync: it is dropped, the match runs.
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(
        &mut client,
        bundle(0, vec![NetControl::Resync { slots: vec![1] }]),
    );
    assert_eq!(client.phase(), &Phase::Halted(HaltReason::Resync));
    lose_host(&mut client);
    client.events();
    client.rejoin().expect("Rejoin");
    client.rehome(migrated(Vec::new())).expect("rehome");
    assert_eq!(client.phase(), &Phase::Running);
    assert_eq!(
        client.events(),
        vec![CoreEvent::Rehomed, CoreEvent::Resumed]
    );
    assert!(!client.into_parts().resync);
}

/// If a bundle is missing in `Migrated` (gap) or it does not start at the next one, `rehome` refuses and changes
/// nothing; the host migration stays open.
#[test]
fn rehome_with_a_gap_is_refused() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(&mut client, bundle(0, Vec::new()));
    lose_host(&mut client);
    client.events();
    client.rejoin().expect("Rejoin");
    for bundles in [
        vec![bundle(2, Vec::new())],
        vec![bundle(1, Vec::new()), bundle(3, Vec::new())],
        vec![bundle(0, Vec::new()), bundle(1, Vec::new())],
    ] {
        let turns: Vec<u32> = bundles.iter().map(|b| b.turn).collect();
        let err = client.rehome(migrated(bundles)).expect_err("gap");
        assert!(!err.is_empty(), "{turns:?}");
        assert!(client.bundle(1).is_none(), "{turns:?}");
        assert!(client.events().is_empty(), "{turns:?}");
    }
    client
        .rehome(migrated(vec![bundle(1, Vec::new()), bundle(2, Vec::new())]))
        .expect("without a gap");
    assert!(client.bundle(2).is_some());
}

/// A `Rejoin` that would be larger than [`MAX_FRAME`] when encoded is not built by the client; the driver then
/// gives up.
#[test]
fn oversized_rejoin_is_refused() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.wish(Wish::Save("x".repeat(MAX_FRAME)));
    client.close_round(0, 0);
    lose_host(&mut client);
    client.events();
    let err = client.rejoin().expect_err("too large");
    assert!(err.contains("too long"), "{err}");
}

/// The client counts the desyncs from the bundles: each `Resync` and each `Stop` counts one.
#[test]
fn client_counts_desyncs_from_bundles() {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    assert_eq!(client.desyncs(), 0);
    deliver(
        &mut client,
        bundle(0, vec![NetControl::Resync { slots: vec![0] }]),
    );
    assert_eq!(client.desyncs(), 1);
    client.handle(0, HOST_PEER, LinkEvent::Message(NetMessage::Resume));
    deliver(&mut client, bundle(1, vec![NetControl::Speed(3)]));
    assert_eq!(client.desyncs(), 1);
    deliver(
        &mut client,
        bundle(2, vec![NetControl::Resync { slots: vec![1] }]),
    );
    client.handle(0, HOST_PEER, LinkEvent::Message(NetMessage::Resume));
    assert_eq!(client.desyncs(), 2);
    deliver(
        &mut client,
        bundle(
            3,
            vec![NetControl::Stop {
                name: "checksumerror_1970_01_01_0000".into(),
            }],
        ),
    );
    assert_eq!(client.desyncs(), 3);
    assert_eq!(client.phase(), &Phase::Stopped);
}

/// Steps of the election up to and including `GiveUp`.
fn steps(s: &mut Succession) -> Vec<SuccessionStep> {
    let mut steps = Vec::new();
    loop {
        let step = s.next();
        let end = step == SuccessionStep::GiveUp;
        steps.push(step);
        if end || steps.len() > 20 {
            return steps;
        }
    }
}

fn dial(peer: u8) -> SuccessionStep {
    SuccessionStep::Dial {
        peer,
        addr: addr(5000 + u16::from(peer)),
    }
}

/// Candidates in order of peer, entries without an address skipped.
#[test]
fn succession_dials_in_peer_order_and_skips_unreachable() {
    let list = [reachable(3), reachable(1), successor(2), successor(4)];
    let mut s = Succession::new(&list, 4, None);
    assert_eq!(steps(&mut s), [dial(1), dial(3), SuccessionStep::GiveUp]);
}

/// At its own entry the client becomes host itself.
#[test]
fn succession_becomes_host_at_its_own_entry() {
    let list = [reachable(1), reachable(2), reachable(3)];
    let mut s = Succession::new(&list, 1, None);
    assert_eq!(s.next(), SuccessionStep::BecomeHost);
    let mut s = Succession::new(&list, 2, None);
    assert_eq!(s.next(), dial(1));
    assert_eq!(s.next(), SuccessionStep::BecomeHost);
}

/// After a handover only the peers from `Handover`.
#[test]
fn succession_after_handover_only_dials_listed_peers() {
    let list = [reachable(1), reachable(2), reachable(3)];
    let mut s = Succession::new(&list, 3, Some(&[3, 2]));
    assert_eq!(
        steps(&mut s),
        [dial(2), SuccessionStep::BecomeHost, SuccessionStep::GiveUp]
    );
}

/// List at the end -> give up (abort savegame), also repeatedly.
#[test]
fn succession_gives_up_at_the_end() {
    let mut s = Succession::new(&[], 1, None);
    assert_eq!(s.next(), SuccessionStep::GiveUp);
    assert_eq!(s.next(), SuccessionStep::GiveUp);
    let mut s = Succession::new(&[reachable(2)], 1, None);
    assert_eq!(steps(&mut s), [dial(2), SuccessionStep::GiveUp]);
    assert_eq!(s.next(), SuccessionStep::GiveUp);
}

/// If a candidate refuses with [`crate::msg::MIGRATION_REFUSED`] (it still hears the host), the client gives up
/// instead of trying the next one - otherwise it could become a second host itself.
// QUIRK E-292: kept
#[test]
fn succession_gives_up_when_refused() {
    let list = [reachable(1), reachable(2), reachable(3)];
    let mut s = Succession::new(&list, 3, None);
    assert_eq!(s.next(), dial(1));
    s.refused();
    assert_eq!(s.next(), SuccessionStep::GiveUp);
    assert_eq!(s.next(), SuccessionStep::GiveUp);
}

// ---------------------------------------------------------------------------------------------------------------------
// Shared helpers for the new host (tests in `new_host`).

/// `n` running clients (peers 1 ... n, slot = peer) with the successor list `list`; bundles come by hand, outbox and
/// events are collected.
fn running_clients(n: usize, list: Vec<Successor>) -> Vec<ClientCore> {
    let mut rig = Rig::build(TurnConfig::default(), &vec![0; n]);
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    let mut clients = std::mem::take(&mut rig.clients);
    for c in &mut clients {
        c.loaded(None);
        c.handle(
            0,
            HOST_PEER,
            LinkEvent::Message(NetMessage::Go {
                successors: list.clone(),
            }),
        );
        assert_eq!(c.phase(), &Phase::Running);
        c.events();
        c.outbox();
    }
    clients
}

/// Successor list with the peers 1 ... `n`, all reachable.
fn candidates(n: u8) -> Vec<Successor> {
    (1..=n).map(reachable).collect()
}

/// Occupancy of the old host (slot 0) and of the clients 1 ... `n` (slot = peer), names as in the rig.
fn seats(n: u8) -> [Seat; 8] {
    let slots: Vec<u8> = (0..=n).collect();
    Rig::info(&slots, TurnConfig::default(), 0).seats
}

/// The client plays turn `b.turn`: close the contribution (checksum = turn), then receive the bundle.
fn play(c: &mut ClientCore, b: &TurnBundle) {
    c.close_round(b.turn, u64::from(b.turn));
    deliver(c, b.clone());
}

/// Each client plays the turns `0..n` with `bundle(k, [])` and then closes turn `n`.
fn play_until(clients: &mut [ClientCore], n: u32) {
    for c in clients.iter_mut() {
        for k in 0..n {
            play(c, &bundle(k, Vec::new()));
        }
        c.close_round(n, u64::from(n));
    }
}

/// The host drops out for all `clients` (connection closed): each reports `HostGone`.
fn crash(clients: &mut [ClientCore]) {
    for c in clients.iter_mut() {
        lose_host(c);
        let events = c.events();
        assert!(
            matches!(events.last(), Some(CoreEvent::HostGone { .. })),
            "{events:?}"
        );
        c.outbox();
    }
}

/// Client `c` becomes the new host (occupancy with `n` clients, results `results`, time 0).
fn promote(c: ClientCore, n: u8, results: [bool; 8]) -> HostCore {
    HostCore::from_client(
        Box::new(c),
        &seats(n),
        results,
        Box::new(AcceptAll),
        0,
        None,
    )
    .map_err(|(_, e)| e)
    .expect("new host")
}

/// New host (peer 1) from two running clients after turn 2, peer 2 is still waiting: `r` is the honest `Rejoin` of
/// peer 2, `f` falsifies it, then it goes to `on_rejoin`.
fn rejoin_with(f: fn(&mut Rejoin)) -> (HostCore, Result<Peer, String>) {
    let mut clients = running_clients(2, candidates(2));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let mut r = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    f(&mut r);
    let result = host.on_rejoin(0, r);
    (host, result)
}

/// Like [`rejoin_with`]; the falsified `Rejoin` must be rejected, and the host keeps waiting.
fn rejoin_refused(f: fn(&mut Rejoin)) {
    let (host, result) = rejoin_with(f);
    assert!(result.is_err(), "{result:?}");
    assert!(host.migrating());
}

/// A client in the host migration (bundle 0 present, `Rejoin` built) receives the foreign `m`; the result does not
/// matter, only that there is no panic.
fn rehome_with(m: Migrated) -> ClientCore {
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    client.close_round(0, 0);
    deliver(&mut client, bundle(0, Vec::new()));
    lose_host(&mut client);
    client.rejoin().expect("Rejoin");
    let _ = client.rehome(m);
    client
}

type Case = (&'static str, fn());

#[test]
fn foreign_migration_inputs_never_panic() {
    let table: Vec<Case> = vec![
        ("Go with more than 8 entries", || {
            go_with((1..=9).map(successor).collect())
        }),
        ("Go with a duplicate peer", || {
            go_with(vec![successor(1), successor(2), successor(1)])
        }),
        ("Migrated with a gap", || {
            rehome_with(migrated(vec![bundle(2, Vec::new())]));
        }),
        ("Migrated with more than 2 * kept_bundles bundles", || {
            let n = 2 * TurnConfig::default().kept_bundles() + 1;
            rehome_with(migrated((1..=n).map(|k| bundle(k, Vec::new())).collect()));
        }),
        ("Migrated with an oversized successor list", || {
            let client = rehome_with(Migrated {
                successors: (1..=9).map(reachable).collect(),
                ..migrated(Vec::new())
            });
            assert!(client.successors().is_empty());
        }),
        // New host: `Rejoin` from outside.
        ("Rejoin from an unknown peer", || {
            rejoin_refused(|r| r.peer = 9)
        }),
        ("Rejoin from the new host's own peer", || {
            rejoin_refused(|r| r.peer = 1)
        }),
        ("Rejoin from the peer of the old host", || {
            rejoin_refused(|r| r.peer = HOST_PEER)
        }),
        ("Rejoin with a wrong slot", || {
            rejoin_refused(|r| r.slot = 3)
        }),
        ("Rejoin twice", || {
            let mut clients = running_clients(3, candidates(3));
            play_until(&mut clients, 1);
            crash(&mut clients);
            let r = clients[1].rejoin().expect("Rejoin");
            let mut host = promote(clients.remove(0), 3, [false; 8]);
            assert_eq!(host.on_rejoin(0, r.clone()), Ok(2));
            assert!(host.on_rejoin(0, r).is_err());
            assert!(host.migrating());
        }),
        ("Rejoin after the deadline", || {
            let mut clients = running_clients(3, candidates(3));
            play_until(&mut clients, 1);
            crash(&mut clients);
            let r = clients[1].rejoin().expect("Rejoin");
            let mut host = promote(clients.remove(0), 3, [false; 8]);
            assert_eq!(
                host.on_rejoin(REJOIN_WINDOW_MS, r),
                Err(MIGRATION_FAILED.to_string())
            );
            assert!(!host.migrating());
        }),
        ("Rejoin with more than 2 * kept_bundles bundles", || {
            rejoin_refused(|r| {
                let n = 2 * TurnConfig::default().kept_bundles() + 1;
                r.bundles = (0..n).map(|k| bundle(k, Vec::new())).collect();
                r.next_bundle = n;
            })
        }),
        (
            "Rejoin with more than 2 * kept_bundles open contributions",
            || {
                rejoin_refused(|r| {
                    let n = 2 * TurnConfig::default().kept_bundles() + 1;
                    r.open = (0..n)
                        .map(|k| TurnContribution {
                            turn: 2 + k,
                            hash: 0,
                            commands: Vec::new(),
                            wishes: Vec::new(),
                        })
                        .collect();
                })
            },
        ),
        ("Rejoin with bundles out of order", || {
            rejoin_refused(|r| r.bundles.swap(0, 1))
        }),
        ("Rejoin with a gap between bundles and next_bundle", || {
            rejoin_refused(|r| r.next_bundle += 1)
        }),
        ("Rejoin with bundle u32::MAX", || {
            rejoin_refused(|r| {
                r.bundles = vec![bundle(u32::MAX, Vec::new())];
                r.next_bundle = 0;
            })
        }),
        ("Rejoin with next_bundle u32::MAX without bundles", || {
            // No hang when completing: the gap up to u32::MAX is missing everywhere, the host migration aborts.
            let (host, result) = rejoin_with(|r| {
                r.bundles.clear();
                r.next_bundle = u32::MAX;
                r.next_close = u32::MAX;
            });
            assert_eq!(result, Ok(2));
            assert_eq!(host.phase(), &Phase::Ended);
        }),
        ("Rejoin with too many commands per contribution", || {
            rejoin_refused(|r| {
                r.open[0].commands = (0..=MAX_COMMANDS_PER_TURN as i32)
                    .map(|t| cmd(2, t))
                    .collect();
            })
        }),
        (
            "Rejoin with too many command bytes per contribution",
            || {
                rejoin_refused(|r| {
                    let units = (0..30_000).map(|i| nml_sim::id::ObjectId(0x1000_0000 + i));
                    r.open[0].commands = vec![Command {
                        player: 2,
                        kind: CommandKind::Halt {
                            units: units.collect(),
                        },
                    }];
                })
            },
        ),
    ];
    let mut failed = Vec::new();
    for (name, run) in table {
        if catch_unwind(AssertUnwindSafe(run)).is_err() {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "panic at: {failed:?}");
}

mod handover;
mod new_host;
