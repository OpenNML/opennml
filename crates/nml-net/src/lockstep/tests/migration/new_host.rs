//! New host: `HostCore::from_client` collects the `Rejoin`s, completes the fringe and keeps bundling.

use super::*;

/// Rig with `n` listening, reachable clients in the running match (slot = peer).
fn rig_candidates(n: usize) -> Rig {
    let mut rig = Rig::build(TurnConfig::default(), &vec![0; n]);
    for (i, c) in rig.clients.iter_mut().enumerate() {
        let peer = i as u16 + 1;
        *c = ClientCore::new(hello_listening(&format!("Client {peer}"), 5000 + peer));
    }
    rig.frames_until(|r| r.clients.iter().all(|c| c.peer().is_some()));
    rig.start();
    rig.frames_until(|r| r.clients.iter().all(|c| c.phase() == &Phase::Loading));
    for c in &mut rig.clients {
        c.loaded(None);
    }
    rig.host.loaded_self();
    for peer in 1..=n as u8 {
        rig.host
            .set_reachable(peer, Some(addr(5000 + u16::from(peer))));
    }
    rig.frames_until(|r| r.all_in(&Phase::Running));
    rig
}

/// Crash of the host in the rig: all connections closed, then all clients report `HostGone`.
fn crash_rig(rig: &mut Rig) {
    for link in &rig.client_links {
        link.cut();
    }
    rig.run_for(500);
    for i in 1..rig.machines.len() {
        assert_eq!(host_gone(rig, i).len(), 1, "machine {i}");
    }
}

/// Client 1 (index 0) becomes the new host (`results`, `only`). The rig keeps it and the clients `keep` (indices into
/// `rig.clients`), each with a new connection to the new host; the old host and all others drop out.
fn take_over(rig: Rig, keep: &[usize], results: [bool; 8], only: Option<&[u8]>) -> Rig {
    let seats = Rig::info(&rig.slots, rig.host.config(), 0).seats;
    let mut clients: Vec<Option<ClientCore>> = rig.clients.into_iter().map(Some).collect();
    let mut machines: Vec<Option<Machine>> = rig.machines.into_iter().map(Some).collect();
    let mut events: Vec<Option<Vec<CoreEvent>>> = rig.events.into_iter().map(Some).collect();
    let me = clients[0].take().expect("new host");
    let host = HostCore::from_client(
        Box::new(me),
        &seats,
        results,
        Box::new(AcceptAll),
        rig.now,
        only,
    )
    .map_err(|(_, e)| e)
    .expect("new host");
    let mut kept = Vec::new();
    let mut host_links = Vec::new();
    let mut client_links = Vec::new();
    let mut slots = vec![rig.slots[1]];
    let mut new_machines = vec![machines[1].take().expect("machine")];
    let mut new_events = vec![events[1].take().expect("events")];
    for &i in keep {
        let c = clients[i].take().expect("Client");
        let (h, l) = rig.net.pair(0);
        host_links.push((c.peer().expect("Peer"), h));
        client_links.push(l);
        kept.push(c);
        slots.push(rig.slots[i + 1]);
        new_machines.push(machines[i + 1].take().expect("machine"));
        new_events.push(events[i + 1].take().expect("events"));
    }
    Rig {
        net: rig.net,
        host,
        clients: kept,
        host_links,
        client_links,
        machines: new_machines,
        events: new_events,
        slots,
        now: rig.now,
        max_lead: 0,
        states_sent: 0,
        close_after_bye: false,
        closed: Vec::new(),
    }
}

/// First turn whose bundle at the new host carries the command `c`.
fn bundle_with(host: &HostCore, c: &Command) -> Option<u32> {
    (0..10_000).find(|&t| host.bundle(t).is_some_and(|b| b.commands.contains(c)))
}

/// `lost` from `MigrationDone` among the events `events`.
fn done(events: &[CoreEvent]) -> Option<Vec<u8>> {
    events.iter().find_map(|e| match e {
        CoreEvent::MigrationDone { lost } => Some(lost.clone()),
        _ => None,
    })
}

/// Delivers the messages `out` of the new host to the `clients`; `Migrated` goes to `rehome` as in the driver.
fn deliver_out(out: Vec<(Peer, NetMessage)>, clients: &mut [ClientCore]) {
    for (to, msg) in out {
        let c = clients
            .iter_mut()
            .find(|c| c.peer() == Some(to))
            .unwrap_or_else(|| panic!("receiver {to} unknown"));
        match msg {
            NetMessage::Migrated(m) => c.rehome(*m).expect("rehome"),
            msg => c.handle(0, HOST_PEER, LinkEvent::Message(msg)),
        }
    }
}

/// Messages back and forth between the new host and the `clients` until nobody sends anything more.
fn exchange(host: &mut HostCore, clients: &mut [ClientCore]) {
    for _ in 0..100 {
        let out = host.outbox();
        let mut quiet = out.is_empty();
        deliver_out(out, clients);
        for c in clients.iter_mut() {
            let peer = c.peer().expect("Peer");
            for (to, msg) in c.outbox() {
                assert_eq!(to, HOST_PEER);
                quiet = false;
                host.handle(0, peer, LinkEvent::Message(msg));
            }
        }
        if quiet {
            return;
        }
    }
    panic!("exchange does not end");
}

/// `Migrated` to `peer` in the outbox `out`.
fn migrated_to(out: &[(Peer, NetMessage)], peer: Peer) -> Option<Migrated> {
    out.iter().find_map(|(to, m)| match m {
        NetMessage::Migrated(m) if *to == peer => Some((**m).clone()),
        _ => None,
    })
}

/// Bundles 0 ... `last` agree at the new host and at every client (the same sequence).
fn assert_same_bundles(host: &HostCore, clients: &[ClientCore], last: u32) {
    for t in 0..=last {
        let reference = host
            .bundle(t)
            .unwrap_or_else(|| panic!("host without bundle {t}"));
        for c in clients {
            assert_eq!(
                c.bundle(t),
                Some(reference),
                "Peer {:?}, bundle {t}",
                c.peer()
            );
        }
    }
}

/// The new host does not bundle until all expected `Rejoin`s are there; then `Migrated` to everyone and, in bundle
/// `K + 1`, the defeat of the old host's slot. Afterward everyone executes the same sequence.
#[test]
fn new_host_waits_for_expected_rejoins() {
    let mut rig = rig_candidates(3);
    rig.run_until(30);
    crash_rig(&mut rig);
    let own_next = rig.clients[0].rejoin().expect("Rejoin").next_bundle;
    let mut rig = take_over(rig, &[1, 2], [false; 8], None);
    assert!(rig.host.migrating());
    assert_eq!(rig.host.phase(), &Phase::Running);

    // The first one arrives: no bundle yet, no `Migrated`.
    let r2 = rig.clients[0].rejoin().expect("Rejoin");
    let next2 = r2.next_bundle;
    assert_eq!(rig.host.on_rejoin(rig.now, r2), Ok(2));
    assert!(rig.host.migrating());
    assert_eq!(rig.host.rejoined(), [2]);
    let steps = rig.steps();
    rig.run_for(2_000);
    assert!(rig.host.migrating());
    assert!(
        rig.steps().iter().zip(&steps).all(|(a, b)| a - b < 12),
        "without a bundle everyone stops after at most one turn: {steps:?} -> {:?}",
        rig.steps()
    );
    assert!(done(&rig.events[0]).is_none());

    // The second one arrives: collecting is done, the match continues.
    let r3 = rig.clients[1].rejoin().expect("Rejoin");
    let next3 = r3.next_bundle;
    assert_eq!(rig.host.on_rejoin(rig.now, r3), Ok(3));
    assert!(!rig.host.migrating());
    assert_eq!(rig.host.rejoined(), [2, 3]);
    let goal = rig.steps().iter().max().copied().unwrap_or(0) + 40;
    rig.run_until(goal);
    assert_eq!(done(&rig.events[0]), Some(vec![0]));
    let k1 = own_next.max(next2).max(next3);
    assert_eq!(
        bundle_with(&rig.host, &loss(0)),
        Some(k1),
        "defeat in K + 1"
    );
    rig.assert_same_execution();
    for i in 0..3 {
        assert_eq!(rig.executed_count(i, &loss(0)), 1, "machine {i}");
        assert!(failures(&rig, i).is_empty(), "machine {i}");
    }
    assert!(rig.has_event(1, |e| e == &CoreEvent::Rehomed));
    assert!(rig.has_event(2, |e| e == &CoreEvent::Rehomed));

    // If the new host itself closes a turn during collecting, no bundle results yet.
    let mut clients = running_clients(2, candidates(2));
    for c in clients.iter_mut() {
        for k in 0..2 {
            play(c, &bundle(k, Vec::new()));
        }
    }
    clients[1].close_round(2, 2);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    host.close_round(2, 2);
    assert!(host.bundle(2).is_none(), "no bundle while collecting");
    assert!(host.outbox().is_empty());
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.bundle(2).expect("bundle K + 1").commands, [loss(0)]);
}

/// An expected client that does not come loses after the window `W` (bundle `K + 1`), and the new host plays on
/// alone.
#[test]
fn absent_client_loses_after_the_window() {
    let mut rig = rig_candidates(2);
    rig.run_until(20);
    crash_rig(&mut rig);
    let start = rig.now;
    let mut rig = take_over(rig, &[], [false; 8], None);
    rig.run_for(REJOIN_WINDOW_MS - 100);
    assert!(rig.host.migrating(), "before the deadline");
    rig.pump_at(start + REJOIN_WINDOW_MS);
    assert!(!rig.host.migrating(), "after the deadline");
    assert_eq!(done(&rig.events[0]), Some(vec![0, 2]));
    let goal = rig.machines[0].steps + 40;
    rig.run_until(goal);
    let k1 = bundle_with(&rig.host, &loss(0)).expect("defeat of the old host");
    assert_eq!(bundle_with(&rig.host, &loss(2)), Some(k1));
    assert_eq!(
        rig.host.bundle(k1).expect("bundle").commands,
        [loss(0), loss(2)]
    );
    assert_eq!(rig.executed_count(0, &loss(2)), 1);
}

/// A slot with a result is not expected (costs no waiting time); if it comes anyway while the host is waiting, it is
/// accepted. After collecting it comes too late.
#[test]
fn defeated_client_gone_costs_no_wait() {
    let mut defeated = [false; 8];
    defeated[2] = true;
    // Only the defeated one in the list: no waiting.
    let mut clients = running_clients(2, candidates(2));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, defeated);
    assert!(!host.migrating(), "nothing to expect");
    assert_eq!(done(&host.events()), Some(vec![0]));
    assert_eq!(host.on_rejoin(0, r2), Err(MIGRATION_FAILED.to_string()));
    let b2 = host
        .bundle(2)
        .expect("bundle K + 1 from its own contribution");
    assert_eq!(b2.commands, [loss(0)], "no dropout command for slot 2");

    // Defeated and one expected: the defeated one arrives first and is accepted, the expected one ends collecting.
    let mut clients = running_clients(3, candidates(3));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, defeated);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert!(host.migrating(), "slot 3 still missing");
    assert_eq!(host.on_rejoin(0, r3), Ok(3));
    assert!(!host.migrating());
    let out = host.outbox();
    assert!(migrated_to(&out, 2).is_some());
    assert!(migrated_to(&out, 3).is_some());
    assert_eq!(done(&host.events()), Some(vec![0]));
}

/// The new host is one bundle behind a client; it takes the bundle from that client's `Rejoin` (control entries as on
/// reception), its own open contribution for it is dropped, and everyone executes the same sequence.
#[test]
fn fringe_new_host_behind_takes_the_bundle() {
    let mut clients = running_clients(2, candidates(2));
    for c in clients.iter_mut() {
        for k in 0..3 {
            play(c, &bundle(k, Vec::new()));
        }
        c.close_round(3, 3);
    }
    // Bundle 3 reached only client 2; it is already closing turn 4.
    let b3 = TurnBundle {
        turn: 3,
        commands: vec![cmd(0, 3), cmd(2, 30)],
        control: vec![NetControl::Speed(4)],
    };
    deliver(&mut clients[1], b3.clone());
    clients[1].close_round(4, 4);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    assert_eq!((r2.next_bundle, turns_of(&r2.open)), (4, vec![4]));
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    assert!(host.bundle(3).is_none());
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert!(!host.migrating());
    assert_eq!(host.bundle(3), Some(&b3));
    let events = host.events();
    assert!(events.contains(&CoreEvent::Control {
        turn: 3,
        control: NetControl::Speed(4),
    }));
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").bundles, []);
    deliver_out(out, &mut clients);
    // Bundle 4 = K + 1: contribution of client 2 from the `Rejoin`, plus its own.
    host.submit(cmd(1, 40));
    host.close_round(4, 4);
    exchange(&mut host, &mut clients);
    let b4 = host.bundle(4).expect("bundle 4");
    assert_eq!(b4.commands, [cmd(1, 40), loss(0)]);
    assert_same_bundles(&host, &clients, 4);
}

/// A client two bundles behind gets both in the `Migrated`, in sequence; afterward the same sequence.
#[test]
fn fringe_client_two_behind_is_filled() {
    let mut clients = running_clients(2, candidates(2));
    for k in 0..5 {
        play(&mut clients[0], &bundle(k, Vec::new()));
    }
    clients[0].close_round(5, 5);
    for k in 0..3 {
        play(&mut clients[1], &bundle(k, Vec::new()));
    }
    clients[1].close_round(3, 3);
    clients[1].close_round(4, 4);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    let out = host.outbox();
    let m = migrated_to(&out, 2).expect("Migrated");
    assert_eq!(
        m.bundles,
        [bundle(3, Vec::new()), bundle(4, Vec::new())],
        "the two missing ones in sequence"
    );
    deliver_out(out, &mut clients);
    clients[0].close_round(5, 5);
    exchange(&mut host, &mut clients);
    assert_eq!(host.bundle(5).expect("bundle K + 1").commands, [loss(0)]);
    assert_same_bundles(&host, &clients, 5);
}

/// Open contributions `> K` become ordinary contributions, `≤ K` (already in the bundle) are dropped - no command is
/// missing, none comes twice.
#[test]
fn open_turns_become_contributions_and_duplicates_are_dropped() {
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        for k in 0..2 {
            play(c, &bundle(k, Vec::new()));
        }
    }
    clients[0].close_round(2, 2);
    clients[1].submit(cmd(2, 22));
    clients[1].close_round(2, 2);
    clients[2].close_round(2, 2);
    // Bundle 2 (with the command of slot 2) reached only client 3; it is already closing turn 3.
    let b2 = TurnBundle {
        turn: 2,
        commands: vec![cmd(2, 22)],
        control: Vec::new(),
    };
    deliver(&mut clients[2], b2.clone());
    clients[2].submit(cmd(3, 33));
    clients[2].close_round(3, 3);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    assert_eq!(turns_of(&r2.open), [2]);
    assert_eq!(turns_of(&r3.open), [3]);
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.on_rejoin(0, r3), Ok(3));
    let out = host.outbox();
    assert_eq!(
        migrated_to(&out, 2).expect("Migrated").bundles,
        std::slice::from_ref(&b2)
    );
    assert_eq!(migrated_to(&out, 3).expect("Migrated").bundles, []);
    deliver_out(out, &mut clients);
    assert_eq!(host.bundle(2), Some(&b2));
    // Turn 3: client 2 closes after the `Migrated`, and the host itself; client 3 came with the `Rejoin`.
    clients[0].submit(cmd(2, 23));
    clients[0].close_round(3, 3);
    host.close_round(3, 3);
    exchange(&mut host, &mut clients);
    assert_eq!(
        host.bundle(3).expect("bundle 3").commands,
        [cmd(2, 23), cmd(3, 33), loss(0)]
    );
    let all: Vec<&Command> = (0..=3)
        .flat_map(|t| host.bundle(t).expect("bundle").commands.iter())
        .collect();
    assert_eq!(all.iter().filter(|c| ***c == cmd(2, 22)).count(), 1);
    assert_eq!(all.iter().filter(|c| ***c == cmd(3, 33)).count(), 1);
    assert_same_bundles(&host, &clients, 3);
}

/// The pause of the old host is lifted by the new host, as is that of one who did not return.
#[test]
fn pause_of_the_old_host_is_lifted() {
    let mut clients = running_clients(2, candidates(2));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Halt { by: 0 }]));
        assert_eq!(c.phase(), &Phase::Halted(HaltReason::Pause { by: 0 }));
    }
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    assert_eq!(host.phase(), &Phase::Halted(HaltReason::Pause { by: 0 }));
    host.events();
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.phase(), &Phase::Running);
    assert!(host.events().contains(&CoreEvent::Resumed));
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").paused_by, None);
    deliver_out(out, &mut clients);
    assert_eq!(clients[0].phase(), &Phase::Running);

    // Pause by slot 3, which does not return: lifted after the deadline.
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Halt { by: 3 }]));
    }
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    host.tick(REJOIN_WINDOW_MS);
    assert_eq!(host.phase(), &Phase::Running);
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").paused_by, None);
}

/// The pause of a third party who returns stays; only that party can lift it.
#[test]
fn pause_of_a_third_stays_and_only_the_pauser_resumes() {
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Halt { by: 2 }]));
    }
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.on_rejoin(0, r3), Ok(3));
    let paused = Phase::Halted(HaltReason::Pause { by: 2 });
    assert_eq!(host.phase(), &paused);
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").paused_by, Some(2));
    assert_eq!(migrated_to(&out, 3).expect("Migrated").paused_by, Some(2));
    deliver_out(out, &mut clients);
    assert!(clients.iter().all(|c| c.phase() == &paused));
    // Neither the host nor slot 3 lifts it.
    host.resume();
    host.handle(0, 3, LinkEvent::Message(NetMessage::Resume));
    assert_eq!(host.phase(), &paused);
    // Slot 2 lifts it: everyone runs.
    clients[0].resume();
    exchange(&mut host, &mut clients);
    assert_eq!(host.phase(), &Phase::Running);
    assert!(clients.iter().all(|c| c.phase() == &Phase::Running));
}

/// The `Resume` of the pausing player reached only some; `Migrated::paused_by` aligns everyone with the new host.
#[test]
fn resume_seen_by_some_is_aligned_by_migrated() {
    let resume =
        |c: &mut ClientCore| c.handle(0, HOST_PEER, LinkEvent::Message(NetMessage::Resume));
    // The new host got it, client 3 did not: client 3 continues afterward.
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Halt { by: 2 }]));
    }
    resume(&mut clients[0]);
    resume(&mut clients[1]);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    host.on_rejoin(0, r2).expect("Rejoin 2");
    host.on_rejoin(0, r3).expect("Rejoin 3");
    assert_eq!(host.phase(), &Phase::Running);
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 3).expect("Migrated").paused_by, None);
    deliver_out(out, &mut clients);
    assert_eq!(clients[1].phase(), &Phase::Running);
    assert!(clients[1].events().contains(&CoreEvent::Resumed));

    // Only client 3 got it: afterward it is in the pause of slot 2 again until slot 2 lifts it.
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Halt { by: 2 }]));
    }
    resume(&mut clients[2]);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    host.on_rejoin(0, r2).expect("Rejoin 2");
    host.on_rejoin(0, r3).expect("Rejoin 3");
    let paused = Phase::Halted(HaltReason::Pause { by: 2 });
    assert_eq!(host.phase(), &paused);
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 3).expect("Migrated").paused_by, Some(2));
    deliver_out(out, &mut clients);
    assert_eq!(clients[1].phase(), &paused);
    clients[0].resume();
    exchange(&mut host, &mut clients);
    assert_eq!(host.phase(), &Phase::Running);
    assert!(clients.iter().all(|c| c.phase() == &Phase::Running));
}

/// A running resync is dropped; a pause from the same bundle applies afterward if the pauser is still there.
#[test]
fn resync_in_progress_is_dropped() {
    // Resync alone: the match runs.
    let mut clients = running_clients(2, candidates(2));
    for c in clients.iter_mut() {
        play(c, &bundle(0, vec![NetControl::Resync { slots: vec![2] }]));
        assert_eq!(c.phase(), &Phase::Halted(HaltReason::Resync));
    }
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    host.on_rejoin(0, r2).expect("Rejoin");
    assert_eq!(host.phase(), &Phase::Running);
    let events = host.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::SendState { .. })),
        "{events:?}"
    );
    assert!(events.contains(&CoreEvent::Resumed));
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").paused_by, None);
    // A state reported later has no effect.
    host.send_state(vec![1, 2, 3]);
    assert!(host.large_outbox().is_empty());

    // Resync and pause by slot 3 in the same bundle: afterward the match is in this pause.
    let mut clients = running_clients(3, candidates(3));
    for c in clients.iter_mut() {
        play(
            c,
            &bundle(
                0,
                vec![
                    NetControl::Resync { slots: vec![2] },
                    NetControl::Halt { by: 3 },
                ],
            ),
        );
    }
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    host.on_rejoin(0, r2).expect("Rejoin 2");
    host.on_rejoin(0, r3).expect("Rejoin 3");
    assert_eq!(host.phase(), &Phase::Halted(HaltReason::Pause { by: 3 }));
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").paused_by, Some(3));
}

/// The desync count survives the host migration (also from a bundle that only one client had); the next desync is
/// the third and stops the match.
#[test]
fn desync_count_survives_migration() {
    let resync = |k| bundle(k, vec![NetControl::Resync { slots: vec![2] }]);
    let resume =
        |c: &mut ClientCore| c.handle(0, HOST_PEER, LinkEvent::Message(NetMessage::Resume));
    let mut clients = running_clients(2, candidates(2));
    for c in clients.iter_mut() {
        play(c, &resync(0));
        resume(c);
        c.close_round(1, 1);
    }
    // Only client 2 saw the second resync.
    deliver(&mut clients[1], resync(1));
    resume(&mut clients[1]);
    clients[1].close_round(2, 2);
    assert_eq!((clients[0].desyncs(), clients[1].desyncs()), (1, 2));
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    assert_eq!(host.desyncs(), 1);
    host.on_rejoin(0, r2).expect("Rejoin");
    assert_eq!(host.desyncs(), 2);
    assert_eq!(host.phase(), &Phase::Running, "the resync is dropped");
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").desyncs, 2);
    deliver_out(out, &mut clients);
    assert_eq!(clients[0].desyncs(), 2);
    // Turn 2 without deviation, turn 3 deviates: desync 3 -> stop.
    host.close_round(2, 2);
    exchange(&mut host, &mut clients);
    assert!(host.bundle(2).expect("bundle 2").control.is_empty());
    clients[0].close_round(3, 99);
    host.close_round(3, 3);
    exchange(&mut host, &mut clients);
    assert!(matches!(
        &host.bundle(3).expect("bundle 3").control[..],
        [NetControl::Stop { .. }]
    ));
    assert_eq!(host.desyncs(), 3);
    assert_eq!(host.phase(), &Phase::Stopped);
    assert_eq!(clients[0].phase(), &Phase::Stopped);
}

/// E-268, "grace period" part, deviated: Even the first turn after the host migration compares the checksum, including
/// that of a contribution from the `Rejoin`.
#[test]
fn deviation_after_migration_is_caught_in_the_first_round() {
    let mut clients = running_clients(2, candidates(2));
    play_until(&mut clients, 3);
    crash(&mut clients);
    let mut r2 = clients[1].rejoin().expect("Rejoin");
    r2.open[0].hash = 999;
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    host.on_rejoin(0, r2).expect("Rejoin");
    let b3 = host
        .bundle(3)
        .expect("bundle K + 1 from both open contributions");
    assert_eq!(b3.commands, [loss(0)]);
    assert_eq!(b3.control, [NetControl::Resync { slots: vec![2] }]);
    assert_eq!(host.desyncs(), 1);
}

/// Without clients the new host plays on alone as a network match (pause and game speed work as on the host).
#[test]
fn lone_new_host_plays_on() {
    let mut clients = running_clients(1, candidates(1));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let mut host = promote(clients.remove(0), 1, [false; 8]);
    assert!(!host.migrating());
    assert!(host.rejoined().is_empty());
    assert_eq!(done(&host.events()), Some(vec![0]));
    assert!(host.successors().is_empty());
    assert_eq!(host.bundle(2).expect("bundle K + 1").commands, [loss(0)]);
    host.wish(Wish::Pause);
    host.wish(Wish::Speed(7));
    host.close_round(3, 3);
    assert_eq!(
        host.bundle(3).expect("bundle 3").control,
        [NetControl::Speed(7), NetControl::Halt { by: 1 }]
    );
    assert_eq!(host.phase(), &Phase::Halted(HaltReason::Pause { by: 1 }));
    host.resume();
    assert_eq!(host.phase(), &Phase::Running);
    host.close_round(4, 4);
    assert!(host.bundle(4).is_some());
    assert!(host.outbox().is_empty());
}

/// A `Rejoin` that does not fit the list (unknown or duplicate peer, wrong slot) or arrives after collecting is
/// rejected (the driver sends `Reject` with the reason); after collecting with [`MIGRATION_FAILED`], so that the
/// latecomer gives up instead of looking for a host of its own.
#[test]
fn foreign_or_late_rejoin_is_rejected() {
    let mut clients = running_clients(3, candidates(3));
    play_until(&mut clients, 1);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r3 = clients[2].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 3, [false; 8]);
    for r in [
        Rejoin {
            peer: 9,
            ..r2.clone()
        },
        Rejoin {
            slot: 3,
            ..r2.clone()
        },
    ] {
        let err = host.on_rejoin(0, r).expect_err("does not fit");
        assert!(!err.is_empty());
    }
    assert_eq!(host.on_rejoin(0, r2.clone()), Ok(2));
    assert!(host.on_rejoin(0, r2.clone()).is_err(), "duplicate");
    assert!(host.migrating());
    assert_eq!(host.rejoined(), [2]);
    // Too late: after the deadline (even without a prior `tick`) and after collecting.
    assert_eq!(
        host.on_rejoin(REJOIN_WINDOW_MS, r3.clone()),
        Err(MIGRATION_FAILED.to_string())
    );
    assert!(!host.migrating());
    assert_eq!(done(&host.events()), Some(vec![0, 3]));
    assert_eq!(host.on_rejoin(0, r3), Err(MIGRATION_FAILED.to_string()));
    assert_eq!(host.rejoined(), [2]);
}

/// `Migrated::successors` is the old list without the new host and without those who did not return (entries without
/// an address stay); the new host is peer 0, the others keep their number.
#[test]
fn migrated_lists_successors_without_the_new_host() {
    let list = vec![reachable(1), reachable(2), reachable(3), successor(4)];
    let mut clients = running_clients(4, list);
    play_until(&mut clients, 1);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let r4 = clients[3].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 4, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.on_rejoin(0, r4), Ok(4));
    host.tick(REJOIN_WINDOW_MS);
    let expected = vec![reachable(2), successor(4)];
    let out = host.outbox();
    assert_eq!(migrated_to(&out, 2).expect("Migrated").successors, expected);
    assert_eq!(migrated_to(&out, 4).expect("Migrated").successors, expected);
    assert_eq!(migrated_to(&out, 2).expect("Migrated").host, 1);
    assert!(migrated_to(&out, 3).is_none());
    assert_eq!(host.successors(), expected);
    assert_eq!(done(&host.events()), Some(vec![0, 3]));
    assert_eq!(host.slot_of(HOST_PEER), Some(1));
    assert_eq!(host.slot_of(2), Some(2));
    let names: Vec<String> = host.peers().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["Client 1", "Client 2", "Client 3", "Client 4"]);
}

/// If a bundle `≤ K` is missing everywhere, the new host aborts the host migration: `Reject` with
/// [`MIGRATION_FAILED`] to every returnee (its [`Succession`] gives up), `MigrationFailed`, phase `Ended`.
#[test]
fn missing_bundle_everywhere_aborts() {
    let mut clients = running_clients(2, candidates(2));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    host.events();
    // Peer 2 reports bundle 3, nobody has bundle 2.
    let r2 = Rejoin {
        peer: 2,
        slot: 2,
        next_close: 5,
        next_bundle: 4,
        bundles: vec![bundle(3, Vec::new())],
        open: Vec::new(),
        desyncs: 0,
    };
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert_eq!(host.phase(), &Phase::Ended);
    assert!(!host.migrating());
    assert_eq!(
        host.outbox(),
        [(
            2,
            NetMessage::Reject {
                reason: MIGRATION_FAILED.into(),
            }
        )]
    );
    let events = host.events();
    assert!(
        matches!(&events[..], [CoreEvent::MigrationFailed { reason }] if !reason.is_empty()),
        "{events:?}"
    );
    assert!(host.bundle(2).is_none());
    // Whoever comes afterward is turned down with the same reason.
    let late = clients[0].rejoin().expect("Rejoin");
    assert_eq!(host.on_rejoin(0, late), Err(MIGRATION_FAILED.to_string()));
    // The one turned down gives up.
    let mut s = Succession::new(&candidates(3), 3, None);
    assert_eq!(s.next(), dial(1));
    s.rejected(MIGRATION_FAILED);
    assert_eq!(s.next(), SuccessionStep::GiveUp);
    let mut s = Succession::new(&candidates(3), 3, None);
    assert_eq!(s.next(), dial(1));
    s.rejected("something else");
    assert_eq!(s.next(), dial(2));
}

/// After an orderly handover (`only`) or if a bundle `≤ K` already carries the result of the old host, no second
/// dropout command comes.
#[test]
fn old_host_loss_is_not_repeated() {
    // Orderly handover.
    let mut clients = running_clients(1, candidates(1));
    play_until(&mut clients, 2);
    crash(&mut clients);
    let mut host = HostCore::from_client(
        Box::new(clients.remove(0)),
        &seats(1),
        [false; 8],
        Box::new(AcceptAll),
        0,
        Some(&[1u8][..]),
    )
    .map_err(|(_, e)| e)
    .expect("new host");
    assert_eq!(done(&host.events()), Some(vec![0]));
    assert_eq!(host.bundle(2).expect("bundle 2").commands, []);

    // The last bundle already carried the defeat of the old host.
    let mut clients = running_clients(1, candidates(1));
    for c in clients.iter_mut() {
        play(c, &bundle(0, Vec::new()));
        play(
            c,
            &TurnBundle {
                turn: 1,
                commands: vec![loss(0)],
                control: Vec::new(),
            },
        );
        c.close_round(2, 2);
    }
    crash(&mut clients);
    let host = promote(clients.remove(0), 1, [false; 8]);
    assert_eq!(host.bundle(2).expect("bundle 2").commands, []);
}

/// If the `Migrated` to a client would be longer than [`MAX_FRAME`], the client counts as not having come:
/// `Reject` with [`MIGRATION_FAILED`], it loses.
#[test]
fn oversized_migrated_counts_as_not_come() {
    let big = |k| {
        bundle(
            k,
            vec![NetControl::Save {
                name: "x".repeat(MAX_FRAME * 2 / 3),
            }],
        )
    };
    let mut clients = running_clients(2, candidates(2));
    for k in 0..2 {
        play(&mut clients[0], &big(k));
    }
    clients[0].close_round(2, 2);
    clients[1].close_round(0, 0);
    crash(&mut clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let mut host = promote(clients.remove(0), 2, [false; 8]);
    assert_eq!(host.on_rejoin(0, r2), Ok(2));
    assert!(!host.migrating());
    assert_eq!(
        host.outbox(),
        [(
            2,
            NetMessage::Reject {
                reason: MIGRATION_FAILED.into(),
            }
        )]
    );
    assert_eq!(done(&host.events()), Some(vec![0, 2]));
    assert!(host.rejoined().is_empty());
    assert_eq!(
        host.bundle(2).expect("bundle 2").commands,
        [loss(0), loss(2)]
    );
}

/// Without host migration (or without a slot), `from_client` returns the client unchanged.
#[test]
fn from_client_without_migration_returns_the_client() {
    let clients = running_clients(1, candidates(1));
    let c = clients.into_iter().next().expect("Client");
    let (back, err) = HostCore::from_client(
        Box::new(c),
        &seats(1),
        [false; 8],
        Box::new(AcceptAll),
        0,
        None,
    )
    .err()
    .expect("no dropout");
    assert!(!err.is_empty());
    assert_eq!(back.phase(), &Phase::Running);
    assert_eq!(back.peer(), Some(1));
}
