use super::*;

fn session(players: u8) -> SessionInfo {
    SessionInfo {
        name: "Session - Anna".to_string(),
        map: MapId::Fixed {
            path: "maps/x.nml".to_string(),
            checksum: 7,
        },
        map_name: "Test map".to_string(),
        settings: SessionSettings {
            game_type: 0,
            unit_limit: 0,
            resources: 0,
            time_limit: 0,
            lock_teams: false,
            map_explored: false,
            save_replay: false,
        },
        players,
        spacing: 8,
        lead: 4,
    }
}

fn row(i: i32, kind: i32) -> LobbyRow {
    LobbyRow {
        kind,
        name: format!("Z{i}"),
        nation: i,
        team: i,
        handicap: 0,
        color: i,
        ready: false,
        peer: None,
    }
}

/// Lobby with 4 rows: host, then computer (Normal), open, open.
fn book() -> LobbyBook {
    let rows = vec![row(0, 999), row(1, 2), row(2, 0), row(3, 0)];
    LobbyBook::new(session(4), rows, "Anna".to_string()).unwrap()
}

struct Deny;
impl LobbyRules for Deny {
    fn check(&self, _: &[LobbyRow], _: u8, _: &LobbyRow, _: i32) -> RuleVerdict {
        RuleVerdict::Reject
    }
}

struct SwapWith(u8, i32);
impl LobbyRules for SwapWith {
    fn check(&self, _: &[LobbyRow], _: u8, _: &LobbyRow, _: i32) -> RuleVerdict {
        RuleVerdict::Swap {
            row: self.0,
            team: self.1,
        }
    }
}

fn wish_row(nation: i32, team: i32, handicap: i32, color: i32) -> LobbyWish {
    LobbyWish::Row {
        nation,
        team,
        handicap,
        color,
    }
}

#[test]
fn new_checks_session_and_rows() {
    let rows = vec![row(0, 999), row(1, 0)];
    assert!(LobbyBook::new(session(2), rows.clone(), "A".into()).is_ok());
    assert!(LobbyBook::new(session(3), rows.clone(), "A".into()).is_err());
    assert!(LobbyBook::new(session(1), vec![row(0, 999)], "A".into()).is_err());
    let mut bad = session(2);
    bad.spacing = 4;
    bad.lead = 4;
    assert!(LobbyBook::new(bad, rows, "A".into()).is_err());
    let b = book();
    let host = &b.state().rows[0];
    assert_eq!((host.kind, host.peer, host.ready), (999, Some(0), false));
}

#[test]
fn join_takes_the_first_free_index() {
    let mut b = book();
    // Row 1 is a computer (Normal, handicap/Ready after the type choice).
    let mut rows = b.state().rows.clone();
    rows[1].handicap = 2;
    rows[1].ready = true;
    b = LobbyBook::new(session(4), rows, "Anna".into()).unwrap();
    assert_eq!(b.join(5, "Bernd").unwrap(), 1);
    let r = &b.state().rows[1];
    assert_eq!(
        (r.kind, r.handicap, r.ready, r.peer),
        (999, 0, false, Some(5))
    );
    assert_eq!(r.name, "Bernd");
    assert_eq!((r.nation, r.team, r.color), (1, 1, 1));
    assert_eq!(b.join(6, "Cora").unwrap(), 2);
    assert_eq!(b.state().rows[2].kind, 999);
}

#[test]
fn full_lobby_rejects() {
    let mut b = book();
    for p in 1..=3 {
        b.join(p, "X").unwrap();
    }
    let rev = b.state().revision;
    let err = b.join(9, "Too late").unwrap_err();
    assert!(err.contains("full"), "{err}");
    assert_eq!(b.state().revision, rev);
    assert!(b.join(2, "Duplicate").is_err());
}

#[test]
fn leave_opens_the_row() {
    let mut b = book();
    b.join(5, "Bernd").unwrap();
    b.wish(5, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    b.wish(5, &wish_row(1, 0, 3, 1), &AcceptAll).unwrap();
    assert_eq!(b.leave(5), Some(1));
    let r = &b.state().rows[1];
    assert_eq!(
        (r.kind, r.ready, r.handicap, r.team, r.peer),
        (0, false, 0, 1, None)
    );
    assert_eq!(b.leave(5), None);
    assert_eq!(b.leave(0), None);
}

#[test]
fn foreign_row_wish_is_dropped() {
    let mut b = book();
    b.join(1, "A").unwrap();
    b.join(2, "B").unwrap();
    let before = b.state().clone();
    // The sender counts, not a row field: peer 1 only ever changes row 1.
    b.wish(1, &wish_row(3, 1, 0, 6), &AcceptAll).unwrap();
    assert_eq!(b.state().rows[2], before.rows[2]);
    assert_eq!(b.state().rows[1].color, 6);
    // Unknown machine: error, nothing changes.
    let snap = b.state().clone();
    assert!(b.wish(9, &LobbyWish::Ready(true), &AcceptAll).is_err());
    assert_eq!(*b.state(), snap);
}

#[test]
fn client_cannot_change_kind_or_name() {
    let mut b = book();
    b.join(1, "A").unwrap();
    let snap = b.state().clone();
    b.wish(1, &LobbyWish::Chat("hello".into()), &AcceptAll)
        .unwrap();
    assert_eq!(*b.state(), snap);
    // There is no wish for type or name; Row changes neither.
    b.wish(1, &wish_row(2, 1, 1, 7), &AcceptAll).unwrap();
    let r = &b.state().rows[1];
    assert_eq!((r.kind, r.name.as_str()), (999, "A"));
    // The host does not change the row of another human either.
    assert!(
        b.edit(
            &HostEdit::Row {
                row: 1,
                nation: 0,
                team: 1,
                handicap: 0,
                color: 1
            },
            &AcceptAll
        )
        .is_err()
    );
    assert!(
        b.edit(
            &HostEdit::Ready {
                row: 1,
                ready: true
            },
            &AcceptAll
        )
        .is_err()
    );
    assert!(
        b.edit(&HostEdit::Kind { row: 1, kind: 2 }, &AcceptAll)
            .is_err()
    );
}

#[test]
fn host_edits_open_and_computer_rows() {
    let mut b = book();
    assert_eq!(
        b.edit(&HostEdit::Kind { row: 2, kind: 3 }, &AcceptAll),
        Ok(EditOutcome::Changed)
    );
    assert_eq!(b.state().rows[2].kind, 3);
    assert_eq!(
        b.edit(
            &HostEdit::Row {
                row: 2,
                nation: 5,
                team: 2,
                handicap: 0,
                color: 7
            },
            &AcceptAll
        ),
        Ok(EditOutcome::Changed)
    );
    assert_eq!((b.state().rows[2].nation, b.state().rows[2].color), (5, 7));
    // Host row: nation/team/color/Ready yes, type never.
    b.edit(
        &HostEdit::Ready {
            row: 0,
            ready: true,
        },
        &AcceptAll,
    )
    .unwrap();
    assert!(b.state().rows[0].ready);
    b.edit(
        &HostEdit::Row {
            row: 0,
            nation: 2,
            team: 0,
            handicap: 0,
            color: 4,
        },
        &AcceptAll,
    )
    .unwrap();
    assert_eq!(b.state().rows[0].color, 4);
    assert!(
        b.edit(&HostEdit::Kind { row: 0, kind: 0 }, &AcceptAll)
            .is_err()
    );
    assert!(
        b.edit(&HostEdit::Kind { row: 9, kind: 0 }, &AcceptAll)
            .is_err()
    );
    assert!(
        b.edit(&HostEdit::Kind { row: 2, kind: 999 }, &AcceptAll)
            .is_err()
    );
}

#[test]
fn kick_returns_the_peer() {
    let mut b = book();
    b.join(4, "A").unwrap();
    b.wish(4, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    assert_eq!(
        b.edit(&HostEdit::Kind { row: 1, kind: 0 }, &AcceptAll),
        Ok(EditOutcome::Kicked { peer: 4 })
    );
    let r = &b.state().rows[1];
    assert_eq!((r.kind, r.ready, r.team, r.peer), (0, false, 1, None));
    // Humans only to Open.
    b.join(4, "A").unwrap();
    assert!(
        b.edit(&HostEdit::Kind { row: 1, kind: 2 }, &AcceptAll)
            .is_err()
    );
}

#[test]
fn computer_kind_sets_ready_and_handicap() {
    let mut b = book();
    for (kind, handicap) in [(1, 5), (2, 2), (3, 0), (4, 0)] {
        b.edit(&HostEdit::Kind { row: 2, kind }, &AcceptAll)
            .unwrap();
        let r = &b.state().rows[2];
        assert_eq!((r.kind, r.handicap, r.ready), (kind, handicap, true));
    }
    // Team of the first other computer (row 1, team 1).
    assert_eq!(b.state().rows[2].team, 1);
    // Open resets.
    b.edit(&HostEdit::Kind { row: 2, kind: 0 }, &AcceptAll)
        .unwrap();
    let r = &b.state().rows[2];
    assert_eq!((r.handicap, r.ready, r.team), (0, false, 2));
    // Game type 4: no team join.
    let mut s = session(4);
    s.settings.game_type = 4;
    let rows = vec![row(0, 999), row(1, 2), row(2, 0), row(3, 0)];
    let mut b = LobbyBook::new(s, rows, "Anna".into()).unwrap();
    b.edit(&HostEdit::Kind { row: 2, kind: 3 }, &AcceptAll)
        .unwrap();
    assert_eq!(b.state().rows[2].team, 2);
}

#[test]
fn handicap_locked_for_easy_to_hard() {
    let mut b = book();
    for kind in 1..=3 {
        b.edit(&HostEdit::Kind { row: 2, kind }, &AcceptAll)
            .unwrap();
        let h = b.state().rows[2].handicap;
        let r = HostEdit::Row {
            row: 2,
            nation: 0,
            team: 2,
            handicap: (h + 1) % 6,
            color: 2,
        };
        assert!(b.edit(&r, &AcceptAll).is_err(), "type {kind}");
    }
    // Manual (4): free.
    b.edit(&HostEdit::Kind { row: 2, kind: 4 }, &AcceptAll)
        .unwrap();
    let r = HostEdit::Row {
        row: 2,
        nation: 0,
        team: 2,
        handicap: 3,
        color: 2,
    };
    assert!(b.edit(&r, &AcceptAll).is_ok());
    assert_eq!(b.state().rows[2].handicap, 3);
}

#[test]
fn rules_reject_keeps_the_host_state() {
    let mut b = book();
    b.join(1, "A").unwrap();
    let snap = b.state().clone();
    assert!(b.wish(1, &wish_row(0, 3, 0, 5), &Deny).is_err());
    assert_eq!(*b.state(), snap);
    let host = HostEdit::Row {
        row: 0,
        nation: 0,
        team: 3,
        handicap: 0,
        color: 5,
    };
    assert!(b.edit(&host, &Deny).is_err());
    assert_eq!(*b.state(), snap);
}

#[test]
fn rules_swap_sets_the_other_row() {
    let mut b = book();
    b.join(1, "A").unwrap();
    b.wish(1, &wish_row(1, 3, 0, 1), &SwapWith(2, 1)).unwrap();
    assert_eq!(b.state().rows[1].team, 3);
    assert_eq!(b.state().rows[2].team, 1);
    // Invalid row in the swap: rejected, nothing changed.
    let snap = b.state().clone();
    assert!(b.wish(1, &wish_row(1, 0, 0, 1), &SwapWith(9, 1)).is_err());
    assert_eq!(*b.state(), snap);
}

#[test]
fn ready_is_kept_on_race_team_color() {
    let mut b = book();
    b.join(1, "A").unwrap();
    b.wish(1, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    b.wish(1, &wish_row(4, 0, 2, 6), &AcceptAll).unwrap();
    let r = &b.state().rows[1];
    assert_eq!((r.nation, r.team, r.handicap, r.color), (4, 0, 2, 6));
    assert!(r.ready);
}

#[test]
fn start_needs_all_ready_and_two_rows() {
    let mut b = book();
    // Only the host and the (not ready) computer.
    assert!(!b.can_start());
    b.edit(
        &HostEdit::Ready {
            row: 1,
            ready: true,
        },
        &AcceptAll,
    )
    .unwrap();
    assert!(!b.can_start(), "host without checkmark");
    b.edit(
        &HostEdit::Ready {
            row: 0,
            ready: true,
        },
        &AcceptAll,
    )
    .unwrap();
    assert!(b.can_start());
    // A human without a checkmark blocks.
    b.join(3, "A").unwrap();
    assert!(!b.can_start());
    b.wish(3, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    assert!(b.can_start());
    // Only the host row occupied: too few.
    let rows = vec![row(0, 999), row(1, 0)];
    let mut one = LobbyBook::new(session(2), rows, "A".into()).unwrap();
    one.edit(
        &HostEdit::Ready {
            row: 0,
            ready: true,
        },
        &AcceptAll,
    )
    .unwrap();
    assert!(!one.can_start());
    // Map name empty.
    let mut s = b.state().session.clone();
    s.map_name = String::new();
    b.set_session(s).unwrap();
    assert!(!b.can_start());
}

/// A random map whose seed consists only of spaces or brackets has an empty map name after cleaning; `START` still
/// applies (the original starts with such a seed, the UI shows `START`). A fixed map without a name still does not
/// start.
#[test]
fn random_map_with_a_blank_seed_can_start() {
    for seed in [" ", "[ ]"] {
        let mut b = book();
        for row in [0, 1] {
            b.edit(&HostEdit::Ready { row, ready: true }, &AcceptAll)
                .unwrap();
        }
        let mut s = b.state().session.clone();
        s.map = MapId::Random {
            seed_text: seed.to_string(),
            size: 0,
            players: 4,
            terrain: 0,
            landscape: 0,
            daytime: 0,
        };
        s.map_name = seed.to_string();
        b.set_session(s).unwrap();
        assert_eq!(
            b.state().session.map_name,
            "",
            "Seed {seed:?}: empty after cleaning"
        );
        assert!(b.can_start(), "Seed {seed:?}");
    }
}

#[test]
fn revision_counts_every_change() {
    let mut b = book();
    let mut rev = b.state().revision;
    let mut step = |b: &LobbyBook, expect_bump: bool| {
        let now = b.state().revision;
        assert_eq!(now != rev, expect_bump, "revision {rev} -> {now}");
        if expect_bump {
            assert_eq!(now, rev + 1);
        }
        rev = now;
    };
    b.join(1, "A").unwrap();
    step(&b, true);
    b.wish(1, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    step(&b, true);
    b.wish(1, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    step(&b, false);
    b.wish(1, &LobbyWish::Chat("x".into()), &AcceptAll).unwrap();
    step(&b, false);
    b.wish(1, &wish_row(3, 1, 0, 1), &AcceptAll).unwrap();
    step(&b, true);
    b.edit(&HostEdit::Kind { row: 2, kind: 1 }, &AcceptAll)
        .unwrap();
    step(&b, true);
    b.edit(&HostEdit::Kind { row: 1, kind: 0 }, &AcceptAll)
        .unwrap();
    step(&b, true);
    b.join(2, "B").unwrap();
    step(&b, true);
    b.leave(2);
    step(&b, true);
    b.set_rtt(Some(40));
    step(&b, true);
    b.set_rtt(Some(40));
    step(&b, false);
    b.set_rtt(None);
    step(&b, true);
    let mut s = b.state().session.clone();
    b.set_session(s.clone()).unwrap();
    step(&b, false);
    s.map_name = "Other".into();
    b.set_session(s).unwrap();
    step(&b, true);
}

#[test]
fn set_session_resizes_rows_and_cleans_names() {
    let mut b = book();
    b.join(1, "A").unwrap();
    let mut s = session(6);
    s.name = "[Hello] ".to_string() + &"x".repeat(60);
    b.set_session(s.clone()).unwrap();
    assert_eq!(b.state().rows.len(), 6);
    assert_eq!(b.state().rows[5].kind, 0);
    assert!(!b.state().session.name.contains('['));
    assert_eq!(b.state().session.name.chars().count(), 39);
    // Shrinking across a machine does not work.
    s.players = 2;
    assert!(b.set_session(s.clone()).is_ok(), "row 1 stays");
    b.join(2, "B").unwrap_err();
    let mut bad = s;
    bad.lead = 9;
    assert!(b.set_session(bad).is_err());
}

/// E-307 deviation: A computer on a row that drops out prevents the switch to a map with fewer players; state and
/// `revision` stay. If the row is open, it drops out.
#[test]
fn set_session_refuses_dropped_rows_with_computers() {
    let mut b = book();
    b.edit(&HostEdit::Kind { row: 3, kind: 1 }, &AcceptAll)
        .unwrap();
    let before = b.state().clone();
    let mut s = session(3);
    s.map_name = "Smaller".into();
    assert!(b.set_session(s.clone()).is_err());
    assert_eq!(b.state(), &before, "state and revision the same");
    b.edit(&HostEdit::Kind { row: 3, kind: 0 }, &AcceptAll)
        .unwrap();
    let revision = b.state().revision;
    assert_eq!(b.set_session(s), Ok(()));
    assert_eq!(b.state().rows.len(), 3);
    assert_eq!(b.state().session.map_name, "Smaller");
    assert_eq!(b.state().revision, revision + 1);
}

/// Teams `≥ n` of the remaining rows are set to the row number, smaller ones stay.
#[test]
fn set_session_clamps_teams() {
    let mut b = book();
    b.edit(
        &HostEdit::Row {
            row: 1,
            nation: 1,
            team: 3,
            handicap: 0,
            color: 1,
        },
        &AcceptAll,
    )
    .unwrap();
    b.edit(
        &HostEdit::Row {
            row: 2,
            nation: 2,
            team: 0,
            handicap: 0,
            color: 2,
        },
        &AcceptAll,
    )
    .unwrap();
    assert_eq!(b.state().rows[1].team, 3);
    b.set_session(session(3)).unwrap();
    let teams: Vec<i32> = b.state().rows.iter().map(|r| r.team).collect();
    assert_eq!(teams, vec![0, 1, 0]);
}

/// E-302: Ready flags stay when the map is switched.
#[test]
fn set_session_keeps_ready_flags() {
    let mut b = book();
    b.join(1, "Bert").unwrap();
    b.wish(1, &LobbyWish::Ready(true), &AcceptAll).unwrap();
    b.edit(
        &HostEdit::Ready {
            row: 0,
            ready: true,
        },
        &AcceptAll,
    )
    .unwrap();
    let mut s = session(6);
    s.map = MapId::Fixed {
        path: "maps/y.nml".into(),
        checksum: 9,
    };
    b.set_session(s).unwrap();
    let ready: Vec<bool> = b.state().rows.iter().map(|r| r.ready).collect();
    assert_eq!(ready, vec![true, true, false, false, false, false]);
    b.set_session(session(2)).unwrap();
    let ready: Vec<bool> = b.state().rows.iter().map(|r| r.ready).collect();
    assert_eq!(ready, vec![true, true]);
}

/// [`rows_fit`] judges like [`LobbyBook::set_session`] (host and core use the same rule).
#[test]
fn rows_fit_matches_set_session() {
    let kind = |b: &mut LobbyBook, row: u8, kind: i32| {
        b.edit(&HostEdit::Kind { row, kind }, &AcceptAll).unwrap();
    };
    let mut cases = Vec::new();
    // Host, computer, open, open.
    cases.push(book());
    // Row 3 computer.
    let mut b = book();
    kind(&mut b, 3, 4);
    cases.push(b);
    // Host, open, human, open.
    let mut b = book();
    b.join(1, "A").unwrap();
    b.join(2, "B").unwrap();
    b.leave(1);
    cases.push(b);
    // Eight rows: only row 7 computer, or rows 1 ... 4 computers.
    let mut b = LobbyBook::open(session(8), "Anna").unwrap();
    kind(&mut b, 7, 1);
    cases.push(b);
    let mut b = LobbyBook::open(session(8), "Anna").unwrap();
    for row in 1..5 {
        kind(&mut b, row, 2);
    }
    cases.push(b);
    for (i, b) in cases.iter().enumerate() {
        for n in 2..=8u8 {
            let fit = rows_fit(&b.state().rows, n);
            let ok = b.clone().set_session(session(n)).is_ok();
            assert_eq!(fit, ok, "case {i}, {n} players");
        }
    }
    assert!(rows_fit(&cases[2].state().rows, 3));
    assert!(!rows_fit(&cases[2].state().rows, 2));
    assert!(!rows_fit(&cases[3].state().rows, 7));
    assert!(rows_fit(&cases[4].state().rows, 5));
    assert!(!rows_fit(&cases[4].state().rows, 4));
}

// ---------------------------------------------------------------------------------------------------------------
// Loaded lobby
// ---------------------------------------------------------------------------------------------------------------

/// Session of a network savegame with `players` rows.
fn saved_session(players: u8) -> SessionInfo {
    let mut s = session(players);
    s.map = MapId::Saved {
        name: "Netgame 1".to_string(),
        checksum: 0xABCD,
    };
    s.map_name = "Netgame 1".to_string();
    s
}

/// Rows from a savegame with the slots `seats`: nation = slot + 10, team = slot mod 2, color = slot, handicap 1.
/// Type, name, Ready and machine are deliberately set "wrongly": [`LobbyBook::loaded`] sets them itself.
fn saved_rows(seats: &[u8]) -> Vec<LobbyRow> {
    seats
        .iter()
        .map(|&s| LobbyRow {
            kind: KIND_HUMAN,
            name: format!("Save {s}"),
            nation: i32::from(s) + 10,
            team: i32::from(s) % 2,
            handicap: 1,
            color: i32::from(s),
            ready: true,
            peer: Some(s),
        })
        .collect()
}

/// Loaded lobby with the slots 1, 3, 4; the host plays slot 3 (row 1).
fn loaded_book() -> LobbyBook {
    LobbyBook::loaded(
        saved_session(3),
        saved_rows(&[1, 3, 4]),
        vec![1, 3, 4],
        3,
        "Anna",
    )
    .unwrap()
}

/// (Type, machine, Ready, name) of a row.
fn seat_of(b: &LobbyBook, row: usize) -> (i32, Option<u8>, bool, String) {
    let r = &b.state().rows[row];
    (r.kind, r.peer, r.ready, r.name.clone())
}

/// The host stands in the row of its slot (E-325 deviation), the other rows are open without machine and name; nation,
/// team, color and handicap come from the savegame, nobody is ready.
#[test]
fn loaded_book_puts_the_host_on_his_seat() {
    let b = loaded_book();
    let s = b.state();
    assert!(s.is_loaded());
    assert_eq!(s.seats, vec![1, 3, 4]);
    assert_eq!(b.row_of(0), Some(1));
    assert_eq!(seat_of(&b, 1), (KIND_HUMAN, Some(0), false, "Anna".into()));
    for row in [0, 2] {
        assert_eq!(
            seat_of(&b, row),
            (0, None, false, String::new()),
            "row {row}"
        );
    }
    for (r, seat) in s.rows.iter().zip([1, 3, 4]) {
        assert_eq!(
            (r.nation, r.team, r.color, r.handicap),
            (seat + 10, seat % 2, seat, 1),
            "slot {seat}"
        );
    }
    // The host's name is cleaned; a new lobby is not loaded.
    let b = LobbyBook::loaded(
        saved_session(2),
        saved_rows(&[0, 7]),
        vec![0, 7],
        7,
        " [b]Anna ",
    )
    .unwrap();
    assert_eq!(seat_of(&b, 1), (KIND_HUMAN, Some(0), false, "bAnna".into()));
    assert!(!book().state().is_loaded());
    assert!(book().state().seats.is_empty());
}

/// `LobbyBook::loaded` checks map, lengths, player count and slots.
#[test]
fn loaded_book_rejects_bad_seats() {
    let make = |players: u8, rows: &[u8], seats: &[u8], host: u8| {
        LobbyBook::loaded(
            saved_session(players),
            saved_rows(rows),
            seats.to_vec(),
            host,
            "Anna",
        )
    };
    assert!(make(3, &[1, 3, 4], &[1, 3, 4], 3).is_ok());
    assert!(make(3, &[3, 1, 4], &[3, 1, 4], 3).is_err(), "unsorted");
    assert!(make(3, &[1, 3, 3], &[1, 3, 3], 3).is_err(), "duplicate");
    assert!(make(3, &[1, 3, 8], &[1, 3, 8], 3).is_err(), "slot > 7");
    assert!(
        make(3, &[1, 3, 4], &[1, 3, 4], 2).is_err(),
        "host not among them"
    );
    assert!(make(3, &[1, 3], &[1, 3, 4], 3).is_err(), "fewer rows");
    assert!(make(3, &[1, 3, 4], &[1, 3], 3).is_err(), "fewer slots");
    assert!(make(2, &[1, 3, 4], &[1, 3, 4], 3).is_err(), "player count");
    assert!(make(1, &[3], &[3], 3).is_err(), "one row");
    // No savegame map, or a name that must not be a file name.
    let rows = saved_rows(&[1, 3]);
    assert!(LobbyBook::loaded(session(2), rows.clone(), vec![1, 3], 3, "Anna").is_err());
    let mut s = saved_session(2);
    s.map = MapId::Saved {
        name: "../Netgame".into(),
        checksum: 1,
    };
    assert!(LobbyBook::loaded(s, rows, vec![1, 3], 3, "Anna").is_err());
    // A savegame as a map works only through `loaded`, not through a new lobby or a map switch.
    assert!(LobbyBook::open(saved_session(3), "Anna").is_err());
    assert!(book().set_session(saved_session(4)).is_err());
}

/// A machine joins in the slot it reports: row of this slot, type human, cleaned name, not ready; nation, team, color
/// and handicap stay those of the savegame.
#[test]
fn join_seat_takes_the_row_of_the_seat() {
    let mut b = loaded_book();
    let revision = b.state().revision;
    assert_eq!(b.join_seat(7, "[Bert]", 4), Ok(2));
    assert_eq!(seat_of(&b, 2), (KIND_HUMAN, Some(7), false, "Bert".into()));
    let r = &b.state().rows[2];
    assert_eq!((r.nation, r.team, r.color, r.handicap), (14, 0, 4, 1));
    assert_eq!(b.state().revision, revision + 1);
    assert_eq!(b.join_seat(2, "Cora", 1), Ok(0));
    assert_eq!(b.row_of(2), Some(0));
    assert_eq!(b.row_of(0), Some(1), "the host stays");
}

/// Slot without a row (no human or dropped out), occupied slot (also the host's), duplicate machine and a new lobby
/// are rejected; the state stays.
#[test]
fn join_seat_refuses_unknown_and_taken_seats() {
    let mut b = loaded_book();
    b.join_seat(7, "Bert", 4).unwrap();
    let before = b.state().clone();
    assert_eq!(
        b.join_seat(8, "Cora", 2),
        Err("Slot 3 has no player left in this savegame.".to_string())
    );
    assert_eq!(
        b.join_seat(8, "Cora", 4),
        Err("Slot 5 is already occupied.".to_string())
    );
    assert_eq!(
        b.join_seat(8, "Cora", 3),
        Err("Slot 4 is already occupied.".to_string()),
        "slot of the host"
    );
    assert!(b.join_seat(7, "Bert", 1).is_err(), "already in the lobby");
    assert!(b.join_seat(0, "Host", 1).is_err(), "the host itself");
    assert_eq!(b.state(), &before);
    let mut fresh = book();
    let before = fresh.state().clone();
    assert!(fresh.join_seat(1, "Bert", 1).is_err(), "new lobby");
    assert_eq!(fresh.state(), &before);
}

/// E-275 deviation: Without a slot report nobody joins a loaded lobby.
#[test]
fn plain_join_is_refused_in_a_loaded_lobby() {
    let mut b = loaded_book();
    let before = b.state().clone();
    assert_eq!(
        b.join(7, "Bert"),
        Err("This match needs the savegame on this machine.".to_string())
    );
    assert_eq!(b.state(), &before);
}

/// When a client leaves, its row becomes open again without name and machine; the values of the savegame stay (not
/// team = row), the slot is free again. Row 0 can leave too; the host does not leave through `leave`.
#[test]
fn leaving_reopens_the_loaded_row() {
    let mut b = loaded_book();
    b.join_seat(7, "Bert", 1).unwrap();
    let revision = b.state().revision;
    assert_eq!(b.leave(7), Some(0));
    assert_eq!(seat_of(&b, 0), (0, None, false, String::new()));
    let r = &b.state().rows[0];
    assert_eq!((r.nation, r.team, r.color, r.handicap), (11, 1, 1, 1));
    assert_eq!(b.state().revision, revision + 1);
    assert_eq!(b.leave(7), None);
    assert_eq!(b.leave(0), None, "the host");
    assert_eq!(b.row_of(0), Some(1));
    assert_eq!(b.join_seat(8, "Cora", 1), Ok(0), "row free again");
}

/// Ready, wishes, chat, host changes and map switches do not exist in the loaded lobby.
#[test]
fn loaded_lobby_ignores_wishes_and_edits() {
    let mut b = loaded_book();
    b.join_seat(7, "Bert", 1).unwrap();
    let before = b.state().clone();
    for w in [
        LobbyWish::Ready(true),
        wish_row(1, 0, 0, 2),
        LobbyWish::Chat("Hello".into()),
    ] {
        assert!(b.wish(7, &w, &AcceptAll).is_err(), "Client: {w:?}");
        assert!(b.wish(0, &w, &AcceptAll).is_err(), "Host: {w:?}");
    }
    for e in [
        HostEdit::Kind { row: 2, kind: 1 },
        HostEdit::Kind { row: 0, kind: 0 },
        HostEdit::Row {
            row: 2,
            nation: 1,
            team: 0,
            handicap: 0,
            color: 2,
        },
        HostEdit::Ready {
            row: 1,
            ready: true,
        },
    ] {
        assert!(b.edit(&e, &AcceptAll).is_err(), "{e:?}");
    }
    assert!(b.set_session(saved_session(3)).is_err());
    assert!(b.set_session(session(3)).is_err());
    assert_eq!(b.state(), &before);
}

/// `START` in the loaded lobby as soon as every row has a machine (without Ready).
#[test]
fn loaded_can_start_needs_every_row_taken() {
    let mut b = loaded_book();
    assert!(!b.can_start(), "only the host");
    b.join_seat(7, "Bert", 1).unwrap();
    assert!(!b.can_start(), "row 2 open");
    b.join_seat(8, "Cora", 4).unwrap();
    assert!(b.can_start(), "all rows occupied");
    b.leave(8);
    assert!(!b.can_start(), "row 2 open again");
}
