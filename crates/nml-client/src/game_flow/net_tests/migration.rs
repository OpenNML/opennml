//! Host migration in the flow: the host gives up in the game menu like a client and stays host until it leaves the end
//! screen; `END_*` and the program end hand the match over to a client (without a reachable client: end for all as
//! before), the old host goes into the accounting without a message. The new host gets the network role of the UI
//! (game speed slider, game speed keys), pause and chat run over it (deviation from quirk E-346). Three machines from
//! the menu via `MemTransport` ([`Lan`]); without `NML_GAME_DIR` the tests skip themselves silently.

use super::*;
use nml_ui::screens::NetRoleView;

/// Anna (host), Bert and Cara from the menu on a map with at least three slots; machine `i` listens as client only
/// with `listens(i)`. Frames until after `Go` and at least `steps` ticks on all.
fn three_match(listens: impl Fn(usize) -> bool, steps: u64) -> Option<Lan> {
    let mut n = Lan::corpus_listening(&["Anna", "Bert", "Cara"], listens, |_, _| {})?;
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
    n.frames_until("START visible", start_shown);
    n.runs[0].click("lan2", "start");
    n.frames_until("all after Go", |l| {
        (0..3).all(|i| l.phase(i) == NetPhase::Match && l.runs[i].view.steps() >= steps)
    });
    Some(n)
}

/// Driver of machine `i` in the match.
fn driver(n: &Lan, i: usize) -> &NetDriver {
    n.runs[i].view.net().expect("driver in the match")
}

/// Network role of the UI of machine `i` (`GameInfo::net`).
fn role(n: &Lan, i: usize) -> Option<NetRoleView> {
    n.runs[i].host.app.game().and_then(|g| g.net)
}

/// How often machine `i` received message 17200 + `slot` ("... was eliminated!").
fn defeat_messages(n: &Lan, i: usize, slot: u8) -> usize {
    n.runs[i].view.game.as_ref().map_or(0, |g| {
        g.player_messages
            .iter()
            .filter(|&&(_, id)| id == DEFEAT_MESSAGE + i32::from(slot))
            .count()
    })
}

/// Frames until machines `a` and `b` are at least `steps` ticks past their current state; their checksums
/// (`(tick, checksum)`) must be equal in the common range.
fn run_on_equal(n: &mut Lan, a: usize, b: usize, steps: u64) {
    let target = n.runs[a].view.steps().max(n.runs[b].view.steps()) + steps;
    let mut seqs = [Vec::new(), Vec::new()];
    n.frames_until("both compute on", |l| {
        l.runs[a].view.steps() >= target && l.runs[b].view.steps() >= target
    });
    // At most two ticks per frame, the history of the simulation keeps 64: the last values suffice for the comparison.
    collect_lan(&mut seqs[0], &n.runs[a]);
    collect_lan(&mut seqs[1], &n.runs[b]);
    let common: Vec<(u64, u64)> = seqs[0]
        .iter()
        .copied()
        .filter(|x| seqs[1].iter().any(|y| y.0 == x.0))
        .collect();
    assert!(common.len() >= 20, "common range: {common:?}");
    for (s, h) in &common {
        let other = seqs[1].iter().find(|y| y.0 == *s).unwrap().1;
        assert_eq!(*h, other, "tick {s}: machines {a} and {b} equal");
    }
}

/// "End current game" of the host is, as for the client, its own defeat through the turn (`SetPlayerResult` of the
/// own slot; re/multiplayer M.10.4): end screen `GAMEEND_DEFEAT` on the host, message 17200 + p on the clients, no
/// `MSGBOX` on anyone. The host stays host until it leaves the end screen; the match goes on.
// QUIRK E-268: deviation
#[test]
fn host_surrender_shows_defeat_and_stays_host() {
    let Some(mut n) = three_match(|_| true, 10) else {
        return;
    };
    let slot = n.surrender_to_the_end_screen(0);
    for i in 0..3 {
        assert_eq!(
            n.runs[i].view.sim().unwrap().state.players[usize::from(slot)].result,
            1,
            "machine {i}: defeat of the host"
        );
    }
    let at = n.runs[1].view.steps();
    n.frames_until("the clients compute on", |l| {
        (1..3).all(|i| l.runs[i].view.steps() >= at + 100)
    });
    let host = driver(&n, 0);
    assert!(host.is_host(), "the host stays host");
    assert!(!host.ending() && !host.finished(), "no handover, no end");
    assert!(
        n.dialogs(0).iter().any(|d| d == "gameend_defeat"),
        "{:?}",
        n.dialogs(0)
    );
    assert_eq!(role(&n, 0), Some(NetRoleView::Host));
    for i in 0..3 {
        assert_eq!(message(&n.runs[i]), None, "machine {i}: no message");
        assert!(n.runs[i].flow.running(), "machine {i}");
    }
    for i in 1..3 {
        assert_eq!(defeat_messages(&n, i, slot), 1, "machine {i}");
        assert!(!driver(&n, i).is_host(), "machine {i}");
    }
}

/// The defeated host leaves its end screen (`END_DEFEAT`, `0x0E`). It hands the match over ([`NetDriver::hand_over`]),
/// computes up to the execution tick of the handover bundle and goes into the accounting without a message
/// (`NetFinish::HandedOver`), whose "Main menu" leads to the main menu. Bert becomes host (network role of the UI
/// host), Cara stays client; both compute on equally, no message, the defeat of the old host arrives exactly once.
// QUIRK E-268: deviation
#[test]
fn host_leaving_the_end_screen_hands_over() {
    let Some(mut n) = three_match(|_| true, 10) else {
        return;
    };
    let slot = n.surrender_to_the_end_screen(0);
    n.runs[0].click("gameend_defeat", "end_defeat");
    let mut handing = false;
    for _ in 0..MAX_LAN_FRAMES {
        if in_accounting(&n.runs[0]) {
            break;
        }
        assert_eq!(message(&n.runs[0]), None, "no message at the old host");
        handing |= n.runs[0].view.net().is_some_and(NetDriver::ending);
        n.frame();
    }
    assert!(handing, "handover ran");
    assert!(in_accounting(&n.runs[0]), "{:?}", n.dialogs(0));
    let rows = &n.runs[0].host.app.data().accounting.as_ref().unwrap().rows;
    assert!(!rows.is_empty());
    n.frames_until("Bert is host", |l| driver(l, 1).is_host());
    n.settle();
    assert_eq!(role(&n, 1), Some(NetRoleView::Host));
    assert_eq!(role(&n, 2), Some(NetRoleView::Client));
    assert!(!driver(&n, 2).is_host());
    run_on_equal(&mut n, 1, 2, 200);
    for i in 1..3 {
        assert_eq!(message(&n.runs[i]), None, "machine {i}: no message");
        assert!(n.runs[i].flow.running(), "machine {i}");
        assert_eq!(
            defeat_messages(&n, i, slot),
            1,
            "machine {i}: 17200 + p once"
        );
    }
    leave_accounting(&mut n.runs[0]);
    n.frames_until("main menu of the old host", |l| in_main_menu(&l.runs[0]));
    assert_eq!(n.phase(0), NetPhase::Idle);
}

/// The host closes the program in the running match (`nml-game` `Game::net_closing` ->
/// [`NetDriver::close_program`]).
/// (a) With reachable clients it hands the match over and waits while game time runs until the handover bundle is
/// executed - within the closing deadline; it goes into the accounting without a message, Bert becomes host, Anna
/// loses. (b) Without listening clients the match ends for all as before (message "The host has ended the match ..."
/// on the clients), also within the deadline.
#[test]
fn host_closing_the_program_hands_over_within_the_close_time() {
    for listening in [true, false] {
        let Some(mut n) = three_match(|_| listening, 10) else {
            return;
        };
        let speed = n.runs[0].view.sim().unwrap().game_speed;
        let start = n.runs[0].now;
        let close_ms = driver(&n, 0).close_ms(speed);
        let close = |n: &mut Lan| {
            let now = n.runs[0].now;
            n.runs[0]
                .view
                .net_mut()
                .is_some_and(|net| net.close_program(speed, now))
        };
        assert!(close(&mut n), "listening {listening}: the program waits");
        assert!(driver(&n, 0).ending());
        while close(&mut n) {
            assert!(
                n.runs[0].now < start + close_ms,
                "listening {listening}: deadline {close_ms} ms exceeded"
            );
            n.frame();
        }
        assert!(
            n.runs[0].now < start + close_ms,
            "listening {listening}: within the deadline"
        );
        if listening {
            assert!(n.runs[0].view.net().is_none(), "match closed");
            assert!(n.runs[0].host.app.data().accounting.is_some());
            n.frames_until("accounting of the old host", |l| in_accounting(&l.runs[0]));
            n.frames_until("Bert is host", |l| driver(l, 1).is_host());
            n.settle();
            assert_eq!(role(&n, 1), Some(NetRoleView::Host));
            for i in 1..3 {
                let sim = n.runs[i].view.sim().unwrap();
                assert_eq!(sim.state.players[0].result, 1, "machine {i}");
                assert_eq!(message(&n.runs[i]), None, "machine {i}");
            }
        } else {
            assert!(driver(&n, 0).finished(), "end bundle executed");
            n.frames_until("message on the clients", |l| {
                (1..3).all(|i| message(&l.runs[i]).is_some())
            });
            let name = NET_CLOCK.net_end_name();
            let expected = format!("The host has ended the match. It was saved as \"{name}\".");
            for i in 1..3 {
                assert_eq!(message(&n.runs[i]).as_deref(), Some(expected.as_str()));
            }
        }
    }
}

/// Message "The host has ended the match ..." for the network savegame of the fixed local time.
fn host_ended_message() -> String {
    let name = NET_CLOCK.net_end_name();
    format!("The host has ended the match. It was saved as \"{name}\".")
}

/// Bert gives up and stays connected on his end screen; Anna wins as a result. If she leaves her victory screen
/// (`END_VICTORY`), nobody is playing anymore: no silent handover to the defeated one, but the end for all - message
/// "The host has ended the match ..." and network savegame on both; "OK" leads Anna through the accounting.
// QUIRK E-268: deviation
#[test]
fn host_victory_with_a_defeated_client_ends_for_all() {
    let Some((mut n, _)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 10);
    n.surrender_to_the_end_screen(1);
    n.frames_until("victory screen of Anna", |l| {
        l.runs[0].dialogs().iter().any(|d| d == "gameend_victory")
    });
    n.settle();
    n.runs[0].click("gameend_victory", "end_victory");
    n.frames_until("message on both", |l| {
        (0..2).all(|i| message(&l.runs[i]).is_some())
    });
    let name = NET_CLOCK.net_end_name();
    for i in 0..2 {
        assert_eq!(
            message(&n.runs[i]),
            Some(host_ended_message()),
            "machine {i}"
        );
        assert!(n.paths(i).net_save(&name).exists(), "machine {i}");
    }
    assert!(!driver(&n, 1).is_host(), "Bert does not become host");
    n.settle();
    n.runs[0].click("msgbox", "ok");
    n.frames_until("accounting of Anna", |l| in_accounting(&l.runs[0]));
}

/// Cara does not listen. Anna leaves the match - (a) via her end screen after giving up, (b) with the program end
/// ([`NetDriver::close_program`]); the handover is accepted, but Bert, the only reachable client, crashes before the
/// bundle is formed. The bundle carries the end for all: network savegame at Anna and Cara, message "The host has
/// ended the match ..." at Cara, in (a) also at Anna with "OK" -> accounting; in (b) the program waits until the end
/// bundle is executed.
// QUIRK E-268: deviation
#[test]
fn handover_falls_back_to_end_when_the_candidate_crashes() {
    for closing in [false, true] {
        let Some(mut n) = three_match(|i| i != 2, 10) else {
            return;
        };
        let speed = n.runs[0].view.sim().unwrap().game_speed;
        let close = |n: &mut Lan| {
            let now = n.runs[0].now;
            n.runs[0]
                .view
                .net_mut()
                .is_some_and(|net| net.close_program(speed, now))
        };
        if closing {
            assert!(close(&mut n), "the program waits");
        } else {
            n.surrender_to_the_end_screen(0);
            n.runs[0].click("gameend_defeat", "end_defeat");
            n.frames_until("handover requested", |l| driver(l, 0).ending());
        }
        assert!(
            driver(&n, 0).handing_over(),
            "program end {closing}: handover accepted"
        );
        n.net.cut_host(n.ips[1]);
        n.runs[1].view.end();
        if closing {
            let start = n.runs[0].now;
            let close_ms = driver(&n, 0).close_ms(speed);
            while close(&mut n) {
                assert!(n.runs[0].now < start + close_ms, "deadline {close_ms} ms");
                n.frame();
            }
            assert!(driver(&n, 0).finished(), "end bundle executed");
        } else {
            n.frames_until("message on Anna", |l| message(&l.runs[0]).is_some());
            assert_eq!(message(&n.runs[0]), Some(host_ended_message()));
        }
        assert!(!driver(&n, 0).handing_over(), "program end {closing}");
        n.frames_until("message on Cara", |l| message(&l.runs[2]).is_some());
        assert_eq!(message(&n.runs[2]), Some(host_ended_message()));
        let name = NET_CLOCK.net_end_name();
        for i in [0, 2] {
            assert!(
                n.paths(i).net_save(&name).exists(),
                "program end {closing}, machine {i}"
            );
        }
        assert!(!driver(&n, 2).is_host());
        if !closing {
            n.settle();
            n.runs[0].click("msgbox", "ok");
            n.frames_until("accounting of Anna", |l| in_accounting(&l.runs[0]));
        }
    }
}

/// Deviation from quirk E-269: After the host's crash Bert becomes full host. The UI gets his network role
/// (`GameInfo::net`, [`MenuApp::set_net_role`]): `GAMEPLAYOPTIONS2` shows him the game speed slider `GAMESPEED`, Cara
/// not. His game speed key requests `Speed(v)` through his core ([`NetDriver::change_game_speed`]), the game speed
/// changes on both at the same tick with message 9; Cara's key has no effect.
// QUIRK E-269: deviation
#[test]
fn new_host_gets_the_speed_slider_and_keys() {
    let Some(mut n) = three_match(|_| true, 10) else {
        return;
    };
    assert_eq!(role(&n, 1), Some(NetRoleView::Client));
    n.net.cut_host(n.ips[0]);
    n.runs[0].view.end();
    n.frames_until("Bert is host", |l| driver(l, 1).is_host());
    n.frame();
    assert_eq!(role(&n, 1), Some(NetRoleView::Host));
    assert_eq!(role(&n, 2), Some(NetRoleView::Client));
    for i in [1, 2] {
        n.runs[i].host.app.push_command(AppCmd::Open {
            name: "gameplayoptions2".into(),
            wait: true,
        });
    }
    n.settle();
    let slider = |n: &Lan, i: usize| {
        element(n, i, "gameplayoptions2", "gamespeed").expect("slider GAMESPEED")
    };
    assert!(!slider(&n, 1).hidden, "Bert sees the slider");
    assert!(slider(&n, 2).hidden, "Cara does not");
    let speed = n.runs[1].view.sim().unwrap().game_speed;
    assert_eq!(
        n.runs[2].view.change_game_speed(1),
        None,
        "Cara: has no effect"
    );
    assert_eq!(n.runs[1].view.change_game_speed(1), Some(speed + 1));
    n.frames_until("game speed on both", |l| {
        (1..3).all(|i| l.runs[i].view.sim().unwrap().game_speed == speed + 1)
    });
    let changes: Vec<Vec<(u64, i32)>> = (1..3)
        .map(|i| n.runs[i].view.game.as_ref().unwrap().speed_messages.clone())
        .collect();
    assert_eq!(changes[0].len(), 1, "{changes:?}");
    assert_eq!(changes[0][0].1, SPEED_RAISED_MESSAGE);
    assert_eq!(changes[0], changes[1], "same tick on both");
}

/// Deviation from quirk E-346: After the migration pause and chat run over the new host instead of into the void.
/// Cara's pause halts both (network pause visible), lifting it lets both compute on; a chat from Cara to all and one
/// from Bert arrive on both at the same tick.
// QUIRK E-346: deviation
#[test]
fn pause_and_chat_run_over_the_new_host() {
    let Some(mut n) = three_match(|_| true, 10) else {
        return;
    };
    n.net.cut_host(n.ips[0]);
    n.runs[0].view.end();
    n.frames_until("Bert is host", |l| driver(l, 1).is_host());
    n.frames(10);
    assert!(n.runs[2].view.toggle_pause(), "pause request");
    n.frames_until("pause on both", |l| {
        (1..3).all(|i| driver(l, i).pause_shown())
    });
    let stand: Vec<u64> = (1..3).map(|i| n.runs[i].view.steps()).collect();
    n.frames(60);
    let now: Vec<u64> = (1..3).map(|i| n.runs[i].view.steps()).collect();
    assert_eq!(now, stand, "both stand still");
    assert!(n.runs[2].view.toggle_pause(), "Cara lifts her pause");
    n.frames_until("both compute on", |l| {
        (1..3).all(|i| l.runs[i].view.steps() > stand[i - 1] + 20)
    });
    let chats = |n: &Lan, i: usize, from: u8| -> Vec<u64> {
        n.runs[i]
            .view
            .sim()
            .unwrap()
            .log
            .iter()
            .filter(|c| {
                matches!(c.command.kind, CommandKind::Chat { player, .. } if player == i32::from(from))
            })
            .map(|c| c.step)
            .collect()
    };
    let slots: Vec<u8> = (1..3)
        .map(|i| n.runs[i].host.app.game().unwrap().local_slot)
        .collect();
    for (k, i) in [2usize, 1].into_iter().enumerate() {
        let from = slots[i - 1];
        n.runs[i].apply(UiEffect::SendChat {
            window: 1,
            text: format!("Hello {k}"),
        });
        n.frames_until("chat on both", |l| {
            (1..3).all(|j| chats(l, j, from).len() == 1)
        });
        assert_eq!(chats(&n, 1, from), chats(&n, 2, from), "chat of {from}");
    }
    run_on_equal(&mut n, 1, 2, 60);
}

/// Without a reachable active client (`end()` as before): Anna plays with Bert and a computer; Bert gives up and
/// leaves the match. Anna is alone with the AI; she gives up and leaves her end screen - that is the end for all
/// (there is nobody to hand over to): network savegame, message "The host has ended the match ...", "OK" ->
/// accounting -> main menu.
#[test]
fn host_alone_with_ai_still_ends_for_all() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert"]) else {
        return;
    };
    let m = n.catalog_map(0, |m| m.players() >= 3);
    let mut slots = m.slots.clone();
    for s in &mut slots {
        match s.key {
            -1 => {
                s.kind = 999;
                s.name = "Anna".into();
            }
            1 => {
                s.kind = 2;
                s.handicap = 2;
                s.name = "KI Hans".into();
            }
            _ => {}
        }
    }
    n.runs[0]
        .host
        .app
        .set_lobby_draft(slots, info_of(&m), DraftSettings::default());
    n.runs[0].apply(UiEffect::Net(AppCmd::Host));
    n.settle();
    assert_eq!(n.phase(0), NetPhase::Lobby);
    n.join(1);
    // The host sets computer row 2 ready (re/multiplayer-menus L.3.4).
    n.all_ready(&[(0, 0), (1, 1), (0, 2)]);
    lan_match(&mut n, 10);
    n.surrender_to_the_end_screen(1);
    n.runs[1].click("gameend_defeat", "end_defeat");
    n.frames_until("accounting of Bert", |l| in_accounting(&l.runs[1]));
    n.settle();
    // Bert sent `Bye` with the accounting; the host sees it within a few frames.
    n.frames(30);
    n.surrender_to_the_end_screen(0);
    n.runs[0].click("gameend_defeat", "end_defeat");
    n.frames_until("message on Anna", |l| message(&l.runs[0]).is_some());
    let name = NET_CLOCK.net_end_name();
    assert_eq!(
        message(&n.runs[0]),
        Some(format!(
            "The host has ended the match. It was saved as \"{name}\"."
        ))
    );
    assert!(n.paths(0).net_save(&name).exists());
    n.settle();
    n.runs[0].click("msgbox", "ok");
    n.frames_until("accounting of Anna", |l| in_accounting(&l.runs[0]));
    n.settle();
    leave_accounting(&mut n.runs[0]);
    n.frames_until("main menu of Anna", |l| in_main_menu(&l.runs[0]));
}

/// If the host drops out in the lobby (connection cut, no `Bye`; quirk E-268, part "Lobby", as in the
/// original), nothing migrates: both clients show 15212 exactly once, "OK" leads to the main menu, nobody has a
/// driver.
// QUIRK E-268: deviation
#[test]
fn lobby_host_loss_still_shows_15212() {
    let Some(mut n) = Lan::corpus(&["Anna", "Bert", "Cara"]) else {
        return;
    };
    n.host_by(0, |m| m.players() >= 3);
    n.join(1);
    n.join(2);
    n.net.cut_host(n.ips[0]);
    n.frames_until("message on both", |l| {
        message_id(&l.runs[1]).is_some() && message_id(&l.runs[2]).is_some()
    });
    n.settle();
    n.frames(120);
    for i in [1, 2] {
        assert_eq!(message_id(&n.runs[i]), Some(KICKED_TEXT), "Client {i}");
        assert_eq!(
            n.runs[i]
                .dialogs()
                .iter()
                .filter(|d| *d == "msgbox")
                .count(),
            1,
            "Client {i}"
        );
        assert!(session_closed(&n, i), "Client {i}");
        assert!(n.runs[i].view.net().is_none(), "Client {i}: no driver");
        n.click(i, "msgbox", "ok");
        assert!(at_start(&n, i), "Client {i}: {:?}", n.dialogs(i));
    }
}
