//! Orderly handover: `HostCore::hand_over` appends the own dropout command and `Handover { peers }` to the next
//! bundle, then `Bye { HANDED_OVER }`; the clients switch as planned and collect only under `peers`.

use super::*;

/// Rig with `n` listening, reachable clients in the running match (slot = peer), at tick 5.
fn rig_reachable(n: usize) -> Rig {
    rig_probed(&vec![true; n])
}

/// Like [`rig_reachable`]; client `i` (peer `i + 1`) counts as reachable after the probe only with `reachable[i]`.
fn rig_probed(reachable: &[bool]) -> Rig {
    let n = reachable.len();
    let mut rig = Rig::build(TurnConfig::default(), &vec![20; n]);
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
        let at = reachable[usize::from(peer) - 1].then(|| addr(5000 + u16::from(peer)));
        rig.host.set_reachable(peer, at);
    }
    rig.frames_until(|r| r.all_in(&Phase::Running));
    rig.run_until(5);
    rig
}

/// Turn and `peers` of the `Handover` that machine `i` reported.
fn handover_of(rig: &Rig, i: usize) -> Option<(u32, Vec<u8>)> {
    rig.controls(i).into_iter().find_map(|(t, c)| match c {
        NetControl::Handover { peers } => Some((t, peers)),
        _ => None,
    })
}

/// The next bundle carries the dropout command of the host slot and `Handover` with all active clients by peer;
/// afterwards the host is `Ended` and sends `Bye { HANDED_OVER }` to each. A running pause is lifted by the handover
/// beforehand (like `end`), a second `hand_over` and an `end` afterwards have no effect.
#[test]
fn hand_over_puts_loss_and_handover_in_the_next_bundle() {
    for paused in [false, true] {
        let mut rig = rig_reachable(2);
        if paused {
            rig.clients[0].wish(Wish::Pause);
            rig.run_for(3_000);
            assert_eq!(
                rig.host.phase(),
                &Phase::Halted(HaltReason::Pause { by: 1 })
            );
        }
        assert!(rig.host.hand_over(), "accepted");
        assert!(!rig.host.hand_over(), "second handover has no effect");
        assert!(!rig.host.end(), "no end after the handover");
        if paused {
            assert_eq!(rig.host.phase(), &Phase::Running);
            rig.pump_at(rig.now);
            assert_eq!(rig.count_events(0, is_resumed), 1);
        }
        rig.run_for(3_000);
        let (turn, peers) = handover_of(&rig, 0).expect("handover in the bundle");
        assert_eq!(peers, vec![1, 2], "Pause {paused}");
        let b = rig.host.bundle(turn).expect("handover bundle").clone();
        assert_eq!(
            b.commands.iter().filter(|c| **c == loss(0)).count(),
            1,
            "{b:?}"
        );
        assert_eq!(rig.host.phase(), &Phase::Ended);
        for i in 1..3 {
            assert_eq!(
                handover_of(&rig, i),
                Some((turn, vec![1, 2])),
                "machine {i}"
            );
            assert_eq!(rig.clients[i - 1].bundle(turn), Some(&b), "machine {i}");
            assert_eq!(rig.clients[i - 1].handover(), Some(&[1u8, 2][..]));
        }
        assert!(!rig.host.end());
        assert!(!rig.host.hand_over());
    }
}

/// After the handover bundle comes `Bye { HANDED_OVER }` to each client, right behind it (as at the end).
#[test]
fn hand_over_sends_the_bundle_then_bye() {
    let mut rig = rig_reachable(1);
    assert!(rig.host.hand_over());
    let mut tail = Vec::new();
    for _ in 0..MAX_FRAMES {
        rig.now += FRAME_MS;
        rig.net.advance_to(rig.now);
        // Like `Rig::pump`, but the outbox of the host is recorded too.
        for (peer, link) in &mut rig.host_links {
            for ev in link.poll() {
                rig.host.handle(rig.now, *peer, ev);
            }
        }
        for (i, link) in rig.client_links.iter_mut().enumerate() {
            for ev in link.poll() {
                rig.clients[i].handle(rig.now, HOST_PEER, ev);
            }
        }
        for i in 0..rig.machines.len() {
            rig.step_machine(i, u64::MAX);
        }
        let out = rig.host.outbox();
        for (to, msg) in &out {
            let (_, link) = rig.host_links.iter_mut().find(|(p, _)| p == to).unwrap();
            link.send(msg.clone());
        }
        for (to, msg) in rig.clients[0].outbox() {
            assert_eq!(to, HOST_PEER);
            rig.client_links[0].send(msg);
        }
        rig.events[0].extend(rig.host.events());
        rig.events[1].extend(rig.clients[0].events());
        if rig.host.phase() == &Phase::Ended {
            tail = out;
            break;
        }
    }
    assert_eq!(rig.host.phase(), &Phase::Ended);
    assert!(
        matches!(
            &tail[..],
            [(1, NetMessage::Bundle(b)), (1, NetMessage::Bye { reason })]
                if reason == HANDED_OVER
                    && b.control.iter().any(|c| matches!(c, NetControl::Handover { .. }))
        ),
        "{tail:?}"
    );
}

/// Without an active client with an address, outside the match and after `end`, `hand_over` refuses; `end` then
/// applies as it does today.
#[test]
fn hand_over_without_reachable_client_is_refused() {
    // Clients without a listening port: not reachable.
    let mut rig = Rig::running(&[20]);
    rig.run_until(5);
    assert!(!rig.host.hand_over());
    assert!(rig.host.end(), "the end remains possible");
    rig.run_for(3_000);
    assert!(handover_of(&rig, 0).is_none());
    assert!(ended_turn(&rig.events[0]).is_some());

    // The only reachable client has dropped out.
    let mut rig = rig_reachable(1);
    rig.client_links[0].cut();
    rig.run_for(500);
    assert_eq!(rig.count_events(0, is_lost), 1);
    assert!(!rig.host.hand_over());

    // Before `Go`.
    let mut rig = rig_loaded_with_listeners();
    assert!(!rig.host.hand_over());

    // After `end`.
    let mut rig = rig_reachable(1);
    assert!(rig.host.end());
    assert!(!rig.host.hand_over());
}

/// Results of the slots `slots` (all others without), for [`HostCore::set_results`].
fn results_of(slots: &[u8]) -> [bool; 8] {
    std::array::from_fn(|s| slots.contains(&(s as u8)))
}

/// The host hands over only if an active client is reachable and an active client without a result is still playing;
/// both independently of each other. A defeated reachable client may become host for an unreachable client that is
/// still playing. If all clients have a result, `hand_over` refuses and `end` applies as it does today; the host's own
/// result does not count.
#[test]
fn hand_over_needs_a_client_still_playing() {
    // All clients defeated.
    let mut rig = rig_reachable(2);
    rig.host.set_results(results_of(&[1, 2]));
    assert!(!rig.host.hand_over(), "nobody is playing any more");
    assert!(rig.host.end(), "the end remains possible");

    // The host is defeated, two clients are playing.
    let mut rig = rig_reachable(2);
    rig.host.set_results(results_of(&[0]));
    assert!(rig.host.hand_over());

    // Only the defeated client 1 is reachable; the unreachable client 2 is still playing.
    let mut rig = rig_probed(&[true, false]);
    rig.host.set_results(results_of(&[1]));
    assert!(rig.host.hand_over(), "client 1 takes over for client 2");
    rig.run_for(3_000);
    let (_, peers) = handover_of(&rig, 0).expect("handover in the bundle");
    assert_eq!(peers, vec![1, 2]);

    // Likewise, but client 2 is defeated too.
    let mut rig = rig_probed(&[true, false]);
    rig.host.set_results(results_of(&[1, 2]));
    assert!(!rig.host.hand_over());
}

/// If the accepted handover is no longer possible when the bundle is formed (the last reachable client dropped out, or
/// the last player still playing has a result in the meantime), the bundle carries `End` instead of `Handover` - end
/// for all as with [`HostCore::end`] - and no dropout command for the host slot. `Bye` with the reason of the end
/// follows.
#[test]
fn hand_over_falls_back_to_end_when_nobody_can_take_over() {
    for case in ["client gone", "client defeated"] {
        let mut rig = rig_reachable(1);
        assert!(rig.host.hand_over(), "{case}");
        match case {
            "client gone" => rig.client_links[0].cut(),
            _ => rig.host.set_results(results_of(&[1])),
        }
        rig.run_for(3_000);
        assert!(handover_of(&rig, 0).is_none(), "{case}: no handover");
        let turn = ended_turn(&rig.events[0]).expect("end in the bundle");
        let b = rig.host.bundle(turn).expect("end bundle");
        assert!(
            b.control
                .iter()
                .any(|c| matches!(c, NetControl::End { .. })),
            "{case}: {b:?}"
        );
        assert!(!b.commands.contains(&loss(0)), "{case}: {b:?}");
        assert_eq!(rig.host.phase(), &Phase::Ended, "{case}");
        assert!(!rig.host.end() && !rig.host.hand_over(), "{case}");
        if case == "client defeated" {
            assert_eq!(
                ended_turn(&rig.events[1]),
                Some(turn),
                "{case}: client ends"
            );
            assert!(
                !host_gone(&rig, 1).iter().any(|(_, planned)| *planned),
                "{case}: no planned switch"
            );
        }
    }
}

/// If the host slot already has a result, the handover bundle carries no dropout command for it.
#[test]
fn defeated_host_hands_over_without_a_second_loss() {
    let mut rig = rig_reachable(1);
    let mut defeated = [false; 8];
    defeated[0] = true;
    rig.host.set_results(defeated);
    assert!(rig.host.hand_over());
    rig.run_for(3_000);
    let (turn, peers) = handover_of(&rig, 0).expect("handover in the bundle");
    assert_eq!(peers, vec![1]);
    let b = rig.host.bundle(turn).expect("handover bundle");
    assert!(!b.commands.contains(&loss(0)), "{b:?}");
}

/// After the `Handover`, the host's `Bye { HANDED_OVER }` is a planned dropout (`HostGone { planned: true }`), as is
/// a connection closed afterwards.
#[test]
fn bye_after_handover_is_a_planned_host_gone() {
    let mut rig = rig_reachable(2);
    assert!(rig.host.hand_over());
    rig.run_for(3_000);
    for i in 1..3 {
        assert_eq!(
            host_gone(&rig, i),
            vec![(HANDED_OVER.to_string(), true)],
            "machine {i}"
        );
        assert!(!rig.has_event(i, is_failed), "machine {i}");
        assert!(rig.clients[i - 1].phase().in_match(), "machine {i}");
    }

    // `Closed` instead of `Bye` after the `Handover`.
    let mut client = running_client(vec![reachable(1), reachable(2)]);
    deliver(
        &mut client,
        bundle(0, vec![NetControl::Handover { peers: vec![1, 2] }]),
    );
    client.events();
    lose_host(&mut client);
    assert_eq!(
        client.events(),
        vec![CoreEvent::HostGone {
            reason: "Connection gone".into(),
            planned: true,
        }]
    );
}

/// After the handover the new host collects only under `peers` (`only`). If all come, collecting is done at once,
/// without the deadline `W`, even if the successor list still has one who left earlier. The old host is in `lost`,
/// but gets no second dropout command in bundle `K + 1`.
#[test]
fn handover_migration_finishes_without_waiting() {
    let mut rig = rig_reachable(3);
    // Peer 3 leaves beforehand: it stays in the successor list from `Go` but is no longer active.
    rig.clients[2].leave("Player has left the match".into());
    rig.run_for(1_000);
    assert_eq!(rig.count_events(0, is_lost), 1);
    assert!(rig.host.hand_over());
    rig.run_for(3_000);
    let (turn, peers) = handover_of(&rig, 0).expect("handover in the bundle");
    assert_eq!(peers, vec![1, 2]);
    for i in 1..3 {
        assert_eq!(host_gone(&rig, i), vec![(HANDED_OVER.to_string(), true)]);
    }
    assert_eq!(rig.clients[0].successors().len(), 3, "list from `Go`");

    let seats = Rig::info(&rig.slots, rig.host.config(), 0).seats;
    let now = rig.now;
    let mut clients = std::mem::take(&mut rig.clients);
    let r2 = clients[1].rejoin().expect("Rejoin");
    let me = clients.remove(0);
    let only = me.handover().expect("handover").to_vec();
    let mut host = HostCore::from_client(
        Box::new(me),
        &seats,
        [false; 8],
        Box::new(AcceptAll),
        now,
        Some(&only),
    )
    .map_err(|(_, e)| e)
    .expect("new host");
    assert!(host.migrating(), "waits for peer 2");
    assert_eq!(host.on_rejoin(now, r2), Ok(2));
    assert!(
        !host.migrating(),
        "all from `peers` present: done without the deadline"
    );
    let lost: Vec<Vec<u8>> = host
        .events()
        .into_iter()
        .filter_map(|e| match e {
            CoreEvent::MigrationDone { lost } => Some(lost),
            _ => None,
        })
        .collect();
    assert_eq!(lost, vec![vec![0]]);
    assert!(migrated_to_2(&host.outbox()));
    // No defeat of the old host after the handover bundle.
    for t in turn + 1..turn + 20 {
        if let Some(b) = host.bundle(t) {
            assert!(!b.commands.contains(&loss(0)), "bundle {t}: {b:?}");
        }
    }
}

/// The outbox contains the `Migrated` to peer 2.
fn migrated_to_2(out: &[(Peer, NetMessage)]) -> bool {
    out.iter()
        .any(|(to, m)| *to == 2 && matches!(m, NetMessage::Migrated(_)))
}
