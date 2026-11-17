//! Loading network savegames: the host loads a savegame, map filter and join with the own file, waiting room `LAN4`
//! with start and exit. Same helpers as the other network flows ([`Lan`]); every machine has its own savegame folder.
//! Without `NML_GAME_DIR` the tests skip themselves silently.

use super::*;

// ---------------------------------------------------------------------------------------------------------------
// Network savegame loading on the host (re/multiplayer-saves S.2.2; deviation from quirks E-327, E-330): `LAN3` with
// `LevelType` 2 -> `{0x12}` reads the name from the name buffer, checks the savegame ([`read_net_save`]) and hosts a
// loaded lobby (a row per human slot in the game) that `LAN4` shows; errors with message 99 106 ... 99 108 via `LAN3`.

use crate::game_save::sim_checksum;
use crate::net_session::read_net_save;
use nml_net::{Announcement, HOST_PEER, KIND_HUMAN};
use nml_sim::state::Control;
use nml_ui::own_texts::{SAVE_NOT_LOADED, SAVE_ONE_PLAYER, SAVE_SLOT_OUT};
use std::sync::OnceLock;

/// Name of the network savegames of the loading tests.
const SAVED: &str = "netz";

/// The three network savegames (Anna, Bert, Carl) of a real network match via `MemTransport` on a map with at least
/// three slots, saved with `Wish::Save` (played once per test run); `None` without the corpus.
fn played_net_saves() -> Option<&'static [Vec<u8>]> {
    static SAVES: OnceLock<Option<Vec<Vec<u8>>>> = OnceLock::new();
    SAVES
        .get_or_init(|| {
            let mut n = Lan::corpus(&["Anna", "Bert", "Carl"])?;
            n.host_by(0, |m| m.players() >= 3);
            n.join(1);
            n.join(2);
            n.all_ready(&[(0, 0), (1, 1), (2, 2)]);
            n.frames_until("START visible", start_shown);
            n.runs[0].click("lan2", "start");
            n.frames_until("all compute", |l| {
                l.runs.iter().all(|r| r.view.steps() >= 20)
            });
            n.runs[0].apply(UiEffect::SaveGame { name: SAVED.into() });
            n.frames_until("all have saved", |l| {
                (0..3).all(|i| l.paths(i).net_save(SAVED).exists())
            });
            Some(
                (0..3)
                    .map(|i| std::fs::read(n.paths(i).net_save(SAVED)).expect("network savegame"))
                    .collect(),
            )
        })
        .as_deref()
}

/// A machine "Anna" with `LevelType` 2 in the settings and `file` as `savegame/multiplayer/netz` (without a file:
/// empty folder).
fn loading_host(file: Option<&[u8]>) -> Option<Lan> {
    let n = Lan::corpus_with(&["Anna"], |_, cfg| cfg.level_type = 2)?;
    if let Some(bytes) = file {
        let path = n.paths(0).net_save(SAVED);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    Some(n)
}

/// Savegame `i` of the played match, changed by `change`.
fn changed_save(i: usize, change: impl FnOnce(&mut GameSaveFile)) -> Option<Vec<u8>> {
    let mut file = GameSaveFile::from_bytes(&played_net_saves()?[i]).unwrap();
    change(&mut file);
    Some(file.to_bytes())
}

/// Player slots with humans in the game of the savegame, in slot order.
fn humans_in_game(file: &GameSaveFile) -> Vec<u8> {
    (0u8..)
        .zip(&file.sim.state.players)
        .filter(|(_, p)| p.control == Control::Human && p.result == 0)
        .map(|(slot, _)| slot)
        .collect()
}

impl Lan {
    /// Machine `i` opens the frame `LOADSAVEDNETWORKGAME` via `LAN1` -> `LAN3` (`LevelType` 2 from the settings) and
    /// clicks `CREATE` (`{0x12}`).
    fn host_saved(&mut self, i: usize) {
        assert_eq!(self.runs[i].host.app.config().level_type, 2);
        self.open_lan1(i);
        self.click(i, "lan1", "create");
        let d = self.dialogs(i);
        assert!(
            d.iter().any(|x| x == "lan3") && d.iter().any(|x| x == "loadsavednetworkgame"),
            "{d:?}"
        );
        self.click(i, "lan3", "create");
    }

    /// The host failed with message `text`: no session, `LAN3` open again with the frame, no `LAN4`.
    fn assert_refused(&self, i: usize, text: i32) {
        assert_eq!(message_id(&self.runs[i]), Some(text));
        assert_eq!(self.phase(i), NetPhase::Idle);
        let status = self.session(i).status();
        assert!(
            status.lobby.is_none() && status.port.is_none(),
            "no session"
        );
        let d = self.dialogs(i);
        assert!(
            d.iter().any(|x| x == "lan3") && d.iter().any(|x| x == "loadsavednetworkgame"),
            "{d:?}"
        );
        assert!(!d.iter().any(|x| x == "lan4"), "{d:?}");
    }
}

/// Announcements a seeker gets on `n.net` (request, then a few frames).
fn announcements(n: &mut Lan) -> Vec<Announcement> {
    let mut seeker = MemTransport::new(&n.net, 0).seeker().expect("seeker");
    seeker.query(1);
    let mut found = Vec::new();
    for _ in 0..20 {
        n.frame();
        found.extend(seeker.poll().into_iter().map(|(_, a)| a));
    }
    found
}

/// The host loads the savegame of another slot (Bert's file; deviation from quirk E-327): a row per human slot in the
/// game in slot order with nation, team, color and handicap from the savegame, the host ("Anna", type 999) in the row
/// of its slot (`local_slot`), the others open; the announcement carries `MapId::Saved` with the checksum of the
/// simulation part, map name = savegame name, number of players = number of rows, raster from `NetTurnRaster`; `LAN4`
/// is open, `LAN3` and the lobby `LAN2` are not.
#[test]
fn host_loads_a_net_save_into_lan4() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let file = GameSaveFile::from_bytes(&saves[1]).unwrap();
    let seats = humans_in_game(&file);
    assert_eq!(seats.len(), 3, "three humans in the game");
    let own = file.host.net.as_ref().unwrap().local_slot;
    let host_row = seats.iter().position(|&s| s == own).unwrap();
    assert_eq!(host_row, 1, "Bert's slot is the second human slot");
    let mut n = loading_host(Some(&saves[1])).unwrap();
    n.host_saved(0);
    assert_eq!(n.phase(0), NetPhase::Lobby);
    let d = n.dialogs(0);
    assert!(d.iter().any(|x| x == "lan4"), "{d:?}");
    assert!(
        !d.iter()
            .any(|x| x == "lan3" || x == "lan2" || x == "msgbox"),
        "{d:?}"
    );
    let status = n.session(0).status();
    assert!(status.is_host);
    assert_eq!(status.own_row, Some(host_row as u8));
    let lobby = status.lobby.unwrap();
    assert_eq!(lobby.seats, seats);
    assert_eq!(lobby.rows.len(), 3);
    for (row, (r, &slot)) in lobby.rows.iter().zip(&seats).enumerate() {
        let p = &file.sim.state.players[usize::from(slot)];
        assert_eq!(
            (r.nation, r.team, r.color, r.handicap),
            (p.nation, p.team, p.color, p.handicap),
            "row {row}"
        );
        assert!(!r.ready);
        if row == host_row {
            assert_eq!(
                (r.kind, r.name.as_str(), r.peer),
                (KIND_HUMAN, "Anna", Some(HOST_PEER))
            );
        } else {
            assert_eq!(
                (r.kind, r.name.as_str(), r.peer),
                (0, "", None),
                "row {row}"
            );
        }
    }
    let session = &lobby.session;
    assert_eq!(
        session.map,
        MapId::Saved {
            name: SAVED.into(),
            checksum: sim_checksum(&file.sim),
        }
    );
    assert_eq!(session.map_name, SAVED);
    assert_eq!(session.name, "Session - Anna", "name as when hosting");
    assert_eq!(session.players, 3);
    let raster = RasterStage::from_index(n.runs[0].host.app.config().net_turn_raster).config();
    assert_eq!((session.spacing, session.lead), raster.raw());
    // Announcement on the LAN: free rows counted over all rows, not "host = row 0".
    let found = announcements(&mut n);
    let a = found.first().expect("announcement");
    assert_eq!(a.session, *session);
    assert_eq!((a.free, a.total), (2, 2));
    // The same savegame via `read_net_save`: slots and checksum as in the session.
    let check = read_net_save(&n.paths(0).net_save(SAVED), &n.runs[0].vfs).unwrap();
    assert_eq!(check.seats, seats);
    assert_eq!(check.checksum, sim_checksum(&file.sim));
}

/// An eliminated human slot (result != 0) gets no row (deviation from quirk E-327).
#[test]
fn defeated_human_slots_get_no_row() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let file = GameSaveFile::from_bytes(&saves[0]).unwrap();
    let all = humans_in_game(&file);
    let bytes = changed_save(0, |f| f.sim.state.players[usize::from(all[2])].result = 1).unwrap();
    let mut n = loading_host(Some(&bytes)).unwrap();
    n.host_saved(0);
    assert_eq!(n.phase(0), NetPhase::Lobby);
    let lobby = n.lobby(0);
    assert_eq!(lobby.seats, all[..2]);
    assert_eq!(lobby.session.players, 2);
    assert_eq!(n.session(0).status().own_row, Some(0));
    assert!(n.dialogs(0).iter().any(|x| x == "lan4"));
}

/// A damaged savegame (checksum of the container) shows 99 106 via `LAN3` (deviation from quirk E-330).
#[test]
fn damaged_save_shows_99106_and_reopens_lan3() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut bytes = saves[0].clone();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    let mut n = loading_host(Some(&bytes)).unwrap();
    n.host_saved(0);
    n.assert_refused(0, SAVE_NOT_LOADED);
}

/// A single-player savegame (without network part and network flag) in the network folder shows 99 106.
#[test]
fn single_player_save_is_refused_with_99106() {
    let Some(bytes) = changed_save(0, |f| {
        f.host.net = None;
        f.sim.start.setup.network = false;
        f.sim.state.network = false;
    }) else {
        return;
    };
    let mut n = loading_host(Some(&bytes)).unwrap();
    n.host_saved(0);
    n.assert_refused(0, SAVE_NOT_LOADED);
}

/// If only one human is left in the game, the host does not host (message 99 107).
#[test]
fn one_remaining_human_shows_99107() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let all = humans_in_game(&GameSaveFile::from_bytes(&saves[0]).unwrap());
    let bytes = changed_save(0, |f| {
        for &slot in &all[1..] {
            f.sim.state.players[usize::from(slot)].result = 1;
        }
    })
    .unwrap();
    let mut n = loading_host(Some(&bytes)).unwrap();
    n.host_saved(0);
    n.assert_refused(0, SAVE_ONE_PLAYER);
}

/// If the host's own slot is eliminated, it does not host (message 99 108), even if two other humans still play.
#[test]
fn own_slot_out_shows_99108() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let file = GameSaveFile::from_bytes(&saves[0]).unwrap();
    let own = file.host.net.as_ref().unwrap().local_slot;
    let bytes = changed_save(0, |f| f.sim.state.players[usize::from(own)].result = 1).unwrap();
    let mut n = loading_host(Some(&bytes)).unwrap();
    n.host_saved(0);
    n.assert_refused(0, SAVE_SLOT_OUT);
}

/// re/multiplayer-saves S.2.2 step 2 (deviation from quirk E-330): Without a savegame the name buffer is empty,
/// `{0x12}` opens `LAN3` again (with the page frame `loadsavednetworkgame`) instead of the original's dead end -
/// without a message, no session, no port, no `LAN4`.
#[test]
fn empty_name_reopens_lan3() {
    let Some(mut n) = loading_host(None) else {
        return;
    };
    n.host_saved(0);
    assert_eq!(n.runs[0].host.app.screen_state().save_name.get(), "");
    assert_eq!(n.phase(0), NetPhase::Idle);
    assert!(n.session(0).status().port.is_none());
    let d = n.dialogs(0);
    assert!(d.iter().any(|x| x == "lan3"), "{d:?}");
    assert!(d.iter().any(|x| x == "loadsavednetworkgame"), "{d:?}");
    assert!(!d.iter().any(|x| x == "lan4" || x == "msgbox"), "{d:?}");
}

// ---------------------------------------------------------------------------------------------------------------
// Map filter and join with the own file (deviation from quirks E-275, E-325): `LAN1` shows a loaded session only with
// the same own network savegame (checksum of the simulation part), `JOIN` reports the own slot (`Hello::seat`), the
// host checks it; the first lobby state opens `LAN4`. Every machine has its own savegame folder.

use crate::net_session::read_net_head;

/// Frames in which every search gets at least two responses (query every second).
const TWO_QUERIES: u32 = 150;

/// Host "Anna" (`LevelType` 2) and clients; `files[i]` is the savegame `netz` of machine `i` (`None`: none). Anna
/// hosts her savegame and is in `LAN4`.
fn saved_lan(names: &[&str], files: &[Option<&[u8]>]) -> Option<Lan> {
    saved_lan_listening(names, files, true)
}

/// Like [`saved_lan`]; `listen`: the clients listen. Without listeners the successor list has no candidate, and a
/// dropout of the host stays the abort.
fn saved_lan_listening(names: &[&str], files: &[Option<&[u8]>], listen: bool) -> Option<Lan> {
    let mut n = Lan::corpus_listening(
        names,
        |_| listen,
        |i, cfg| {
            if i == 0 {
                cfg.level_type = 2;
            }
        },
    )?;
    for (i, file) in files.iter().enumerate() {
        if let Some(bytes) = file {
            n.write_save(i, bytes);
        }
    }
    n.host_saved(0);
    assert!(
        n.dialogs(0).iter().any(|x| x == "lan4"),
        "{:?}",
        n.dialogs(0)
    );
    Some(n)
}

/// Own slot (`local_slot`) in the savegame `bytes`.
fn local_slot(bytes: &[u8]) -> u8 {
    let file = GameSaveFile::from_bytes(bytes).unwrap();
    file.host.net.unwrap().local_slot
}

/// Row of the slot `slot` in the loaded lobby.
fn row_of_slot(lobby: &nml_net::LobbyState, slot: u8) -> usize {
    lobby
        .seats
        .iter()
        .position(|&s| s == slot)
        .expect("slot with row")
}

/// Savegame `i` of the played match with a different simulation part (one tick more, same length).
fn other_state(i: usize) -> Option<Vec<u8>> {
    changed_save(i, |f| f.sim.state.steps += 1)
}

impl Lan {
    /// Writes `bytes` as `savegame/multiplayer/netz` of machine `i`.
    fn write_save(&self, i: usize, bytes: &[u8]) {
        let path = self.paths(i).net_save(SAVED);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }

    /// Overwrites the savegame of machine `i` with `bytes` and sets the modification time two seconds ahead (like a
    /// later save under the same name).
    fn rewrite_save(&self, i: usize, bytes: &[u8]) {
        let path = self.paths(i).net_save(SAVED);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        self.write_save(i, bytes);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(before + std::time::Duration::from_secs(2))
            .unwrap();
    }

    /// Visible games in the search of machine `i`.
    fn found(&self, i: usize) -> usize {
        self.session(i).status().games.len()
    }

    /// Machine `i` opens `LAN1`, waits for the game and selects it (without `JOIN`).
    fn pick_game(&mut self, i: usize) {
        self.open_lan1(i);
        self.frames_until("game in the list", |l| {
            element(l, i, "lan1", "opengames").is_some_and(|e| !e.items.is_empty())
        });
        self.list_row(i, "lan1", "opengames", 0);
    }

    /// Like [`Self::click_join`], then until `LAN4`.
    fn join_saved(&mut self, i: usize) {
        self.click_join(i);
        self.frames_until("LAN4 of the client", |l| {
            l.phase(i) == NetPhase::Lobby && l.dialogs(i).iter().any(|x| x == "lan4")
        });
        self.settle();
    }

    /// The join of machine `i` ended silently in `LAN1`: no message, no lobby, the session is closed.
    fn assert_silently_in_lan1(&self, i: usize) {
        let d = self.dialogs(i);
        assert!(d.contains(&"lan1".to_string()), "{d:?}");
        assert!(
            !d.iter()
                .any(|x| x == "msgbox" || x == "lan2" || x == "lan4"),
            "{d:?}"
        );
        assert!(session_closed(self, i));
    }
}

/// Only whoever has the same network savegame sees the loaded session (Bert: his own file of the same match). Without
/// a file (Cara), with a different state of the same name (Dora) or a single-player savegame of the same name (Emil)
/// it stays silently hidden.
#[test]
fn saved_session_is_listed_only_with_the_same_file() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let other = other_state(1).unwrap();
    let single = changed_save(1, |f| {
        f.host.net = None;
        f.sim.start.setup.network = false;
        f.sim.state.network = false;
    })
    .unwrap();
    let mut n = saved_lan(
        &["Anna", "Bert", "Cara", "Dora", "Emil"],
        &[
            Some(&saves[0]),
            Some(&saves[1]),
            None,
            Some(&other),
            Some(&single),
        ],
    )
    .unwrap();
    for i in 1..5 {
        n.open_lan1(i);
    }
    n.frames_until("Bert sees the session", |l| l.found(1) == 1);
    n.frames(TWO_QUERIES);
    assert_eq!(n.found(1), 1);
    for i in 2..5 {
        assert_eq!(n.found(i), 0, "machine {i}");
    }
    // The light header of the files: checksum and own slot, without rules.
    let file = GameSaveFile::from_bytes(&saves[0]).unwrap();
    let head = |i: usize| read_net_head(&n.paths(i).net_save(SAVED));
    assert_eq!(
        head(1),
        Ok((sim_checksum(&file.sim), local_slot(&saves[1])))
    );
    assert!(head(2).is_err());
    assert_ne!(head(3).unwrap().0, sim_checksum(&file.sim));
    assert!(head(4).is_err());
}

/// Bert and Carl join with their own files (deviation from quirks E-275, E-325); each is in the row of his slot
/// ("Occupied" with his name), sees the host's state and has `LAN4` open (not `LAN2`).
#[test]
fn client_joins_into_lan4_on_its_own_row() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&saves[0]), Some(&saves[1]), Some(&saves[2])],
    )
    .unwrap();
    n.join_saved(1);
    n.join_saved(2);
    n.frames(10);
    let host = n.lobby(0);
    for (i, name) in [(1, "Bert"), (2, "Carl")] {
        let row = row_of_slot(&host, local_slot(&saves[i]));
        let r = &host.rows[row];
        assert_eq!((r.kind, r.name.as_str()), (KIND_HUMAN, name), "row {row}");
        assert!(r.peer.is_some_and(|p| p != HOST_PEER));
        assert_eq!(n.session(i).status().own_row, Some(row as u8));
        assert_eq!(n.lobby(i), host, "state of the host at {name}");
        let d = n.dialogs(i);
        assert!(d.iter().any(|x| x == "lan4"), "{d:?}");
        assert!(
            !d.iter()
                .any(|x| x == "lan1" || x == "lan2" || x == "chatframe" || x == "msgbox"),
            "{d:?}"
        );
    }
    let own = row_of_slot(&host, local_slot(&saves[0]));
    assert_eq!(host.rows[own].name, "Anna");
    assert_eq!(host.rows[own].peer, Some(HOST_PEER));
}

/// Bert has a copy of the host's file (same slot): he sees the session, `JOIN` is rejected, he stays silently in
/// `LAN1`; the host's rows stay.
#[test]
fn client_with_the_hosts_slot_is_refused() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(&["Anna", "Bert"], &[Some(&saves[0]), Some(&saves[0])]).unwrap();
    let rows = n.lobby(0).rows;
    n.pick_game(1);
    n.runs[1].click("lan1", "join");
    n.frames_until("join running", |l| l.phase(1) == NetPhase::Connecting);
    n.frames_until("join decided", |l| l.phase(1) != NetPhase::Connecting);
    n.settle();
    n.assert_silently_in_lan1(1);
    assert_eq!(n.lobby(0).rows, rows);
    assert_eq!(message_id(&n.runs[0]), None);
}

/// Bert and Cara have copies of the same file (Bert's slot) and click `JOIN` at the same time; Bert (shorter
/// connection) gets the row, Cara is rejected and stays silently in `LAN1`.
#[test]
fn two_clients_with_the_same_slot_only_first_joins() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Cara"],
        &[Some(&saves[0]), Some(&saves[1]), Some(&saves[1])],
    )
    .unwrap();
    n.pick_game(1);
    n.pick_game(2);
    n.runs[1].click("lan1", "join");
    n.runs[2].click("lan1", "join");
    n.frames_until("both joins decided", |l| {
        l.phase(1) == NetPhase::Lobby && l.phase(2) != NetPhase::Connecting
    });
    n.settle();
    assert!(
        n.dialogs(1).iter().any(|x| x == "lan4"),
        "{:?}",
        n.dialogs(1)
    );
    n.assert_silently_in_lan1(2);
    let host = n.lobby(0);
    let row = row_of_slot(&host, local_slot(&saves[1]));
    assert_eq!(host.rows[row].name, "Bert");
    assert_eq!(n.session(1).status().own_row, Some(row as u8));
    assert_eq!(
        host.rows.iter().filter(|r| r.peer.is_some()).count(),
        2,
        "host and Bert"
    );
}

/// Bert's file changes after the list showed the session (new save under the same name); `JOIN` reports the new
/// checksum, the host rejects (before the deadline), Bert stays silently in `LAN1`.
#[test]
fn changed_file_after_listing_is_refused_by_the_host() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(&["Anna", "Bert"], &[Some(&saves[0]), Some(&saves[1])]).unwrap();
    let rows = n.lobby(0).rows;
    n.pick_game(1);
    n.rewrite_save(1, &other_state(1).unwrap());
    n.runs[1].click("lan1", "join");
    n.frames_until("join running", |l| l.phase(1) == NetPhase::Connecting);
    let mut frames = 0;
    while n.phase(1) == NetPhase::Connecting {
        n.frame();
        frames += 1;
        assert!(frames < 100, "rejected before the deadline");
    }
    n.settle();
    n.assert_silently_in_lan1(1);
    assert_eq!(n.lobby(0).rows, rows);
}

/// The cache follows length and modification time: Bert's file is overwritten with a different state of the same
/// length -> the session disappears; the right file again -> it reappears.
#[test]
fn listing_rereads_a_rewritten_file() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let other = other_state(1).unwrap();
    assert_eq!(other.len(), saves[1].len(), "same length");
    let mut n = saved_lan(&["Anna", "Bert"], &[Some(&saves[0]), Some(&saves[1])]).unwrap();
    n.open_lan1(1);
    n.frames_until("session visible", |l| l.found(1) == 1);
    n.rewrite_save(1, &other);
    n.frames_until("session hidden", |l| l.found(1) == 0);
    n.rewrite_save(1, &saves[1]);
    n.frames_until("session visible again", |l| l.found(1) == 1);
}

/// Bert is rejected when joining a savegame (the host's slot); his next join to a new lobby (Cara, fixed map) carries
/// no slot claim anymore and succeeds.
#[test]
fn join_after_a_saved_session_sends_no_claim() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Cara"],
        &[Some(&saves[0]), Some(&saves[0])],
    )
    .unwrap();
    n.click_join(1);
    n.frames_until("join decided", |l| l.phase(1) != NetPhase::Connecting);
    n.settle();
    n.assert_silently_in_lan1(1);
    n.runs[0].host.app.push_command(AppCmd::CloseNet);
    n.settle();
    // Anna's entry expires in Bert's list (3 s without a response), then Cara's lobby is the only game.
    n.frames_until("Anna's session expired", |l| l.found(1) == 0);
    n.host(2, Some("tinte"));
    n.join(1);
    let lobby = n.lobby(2);
    assert_eq!(lobby.rows[1].name, "Bert");
    assert!(lobby.seats.is_empty(), "new lobby");
}

/// After a rejected join to a savegame Bert hosts a new lobby; his sign-on (in the announcement) carries no slot
/// claim.
#[test]
fn hosting_after_a_saved_join_announces_no_claim() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(&["Anna", "Bert"], &[Some(&saves[0]), Some(&saves[0])]).unwrap();
    n.click_join(1);
    n.frames_until("join decided", |l| l.phase(1) != NetPhase::Connecting);
    n.settle();
    n.assert_silently_in_lan1(1);
    n.runs[0].host.app.push_command(AppCmd::CloseNet);
    n.settle();
    n.host(1, Some("tinte"));
    let found = announcements(&mut n);
    let a = found
        .iter()
        .find(|a| a.session.name == "Session - Bert")
        .expect("announcement of Bert");
    assert_eq!(a.versions.seat, None);
}

// ---------------------------------------------------------------------------------------------------------------
// Waiting room `LAN4` (deviation from quirk E-326): ESC leaves `LAN4` like `EXIT` of the lobby (`{0x2D}`: `LAN4`
// closed, `{9}`, `{5,"start"}`), `{0x14}` of the host requires every row occupied.

use nml_ui::screens::lan4::close_lan4;

impl Lan {
    /// `START` in `LAN4` of machine `i` is visible (code 5 anew every second).
    fn lan4_start_shown(&self, i: usize) -> bool {
        element(self, i, "lan4", "start").is_some_and(|e| e.visible && !e.hidden)
    }

    /// Machine `i` is in the main menu without `LAN4`, without a message and without a session.
    fn assert_left_lan4(&self, i: usize) {
        let d = self.dialogs(i);
        assert!(at_start(self, i) && !d.iter().any(|x| x == "lan4"), "{d:?}");
        assert!(session_closed(self, i));
        assert_eq!(self.phase(i), NetPhase::Idle);
    }
}

/// Anna hosts her savegame, Bert and Carl join with their files; all in `LAN4`, `START` visible on the host.
fn full_lan4() -> Option<(Lan, &'static [Vec<u8>])> {
    full_lan4_listening(true)
}

/// Like [`full_lan4`]; `listen` as for [`saved_lan_listening`].
fn full_lan4_listening(listen: bool) -> Option<(Lan, &'static [Vec<u8>])> {
    let saves = played_net_saves()?;
    let mut n = saved_lan_listening(
        &["Anna", "Bert", "Carl"],
        &[Some(&saves[0]), Some(&saves[1]), Some(&saves[2])],
        listen,
    )?;
    n.join_saved(1);
    n.join_saved(2);
    n.frames_until("START visible", |l| l.lan4_start_shown(0));
    Some((n, saves))
}

/// Carl presses ESC in `LAN4` (deviation from quirk E-326): he is in the main menu without a message, his session is
/// closed. On the host his row becomes "Open" again (nation, team, color, handicap stay) and `START` disappears; host
/// and Bert stay in `LAN4`.
#[test]
fn escape_in_lan4_takes_the_client_to_the_main_menu() {
    let Some((mut n, saves)) = full_lan4() else {
        return;
    };
    let row = row_of_slot(&n.lobby(0), local_slot(&saves[2]));
    let before = n.lobby(0).rows[row].clone();
    n.runs[2].key("ESCAPE");
    n.settle();
    n.assert_left_lan4(2);
    assert_eq!(message_id(&n.runs[2]), None);
    n.frames_until("row open", |l| l.lobby(0).rows[row].peer.is_none());
    let r = &n.lobby(0).rows[row];
    assert_eq!((r.kind, r.name.as_str()), (0, ""));
    assert_eq!(
        (r.nation, r.team, r.color, r.handicap),
        (before.nation, before.team, before.color, before.handicap)
    );
    n.frames_until("START hidden", |l| !l.lan4_start_shown(0));
    for i in [0, 1] {
        assert!(n.dialogs(i).iter().any(|x| x == "lan4"), "machine {i}");
        assert_eq!(n.phase(i), NetPhase::Lobby, "machine {i}");
    }
    n.frames_until("state at Bert", |l| l.lobby(1) == l.lobby(0));
}

/// Anna (host) presses ESC in `LAN4`: she is in the main menu, `Bye { HOST_LEFT }` goes to all; Bert and Carl see
/// 15212 exactly once. ESC over the message has no effect (`MSGBOX` does not take ESC, quirk O-103); "OK" closes `LAN4`
/// (the session is already reset by then) and leads to the main menu.
#[test]
fn host_escape_closes_the_session_and_clients_see_15212_once() {
    let Some((mut n, _)) = full_lan4() else {
        return;
    };
    n.runs[0].key("ESCAPE");
    n.settle();
    n.assert_left_lan4(0);
    n.frames_until("message on both", |l| {
        message_id(&l.runs[1]).is_some() && message_id(&l.runs[2]).is_some()
    });
    n.settle();
    n.frames(120);
    let messages = |run: &Run| run.dialogs().iter().filter(|d| *d == "msgbox").count();
    for i in [1, 2] {
        assert_eq!(message_id(&n.runs[i]), Some(KICKED_TEXT), "Client {i}");
        assert_eq!(messages(&n.runs[i]), 1, "Client {i}");
        assert!(n.runs[i].host.app.queue().is_empty(), "Client {i}");
        assert!(session_closed(&n, i), "Client {i}");
        assert!(
            n.dialogs(i).iter().any(|x| x == "lan4"),
            "under the message"
        );
        n.runs[i].key("ESCAPE");
        n.settle();
        assert_eq!(message_id(&n.runs[i]), Some(KICKED_TEXT), "Client {i}");
        assert_eq!(messages(&n.runs[i]), 1, "Client {i}");
        n.click(i, "msgbox", "ok");
        n.assert_left_lan4(i);
    }
}

/// `{0x14}` on the host without every row occupied does nothing (`START` is hidden; deviation from quirk E-328); on
/// the client never anything. With every row occupied: `LAN4` closed, `{0x15}`, `{0x0D}` like `START` in `LAN2`.
#[test]
fn start_saved_game_needs_every_row() {
    let Some(saves) = played_net_saves() else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&saves[0]), Some(&saves[1]), Some(&saves[2])],
    )
    .unwrap();
    n.join_saved(1);
    n.frames(TWO_QUERIES);
    assert!(!n.lan4_start_shown(0), "Carl's row is open");
    n.runs[0].apply(UiEffect::Net(AppCmd::StartSavedGame));
    assert!(n.runs[0].host.app.queue().is_empty());
    n.settle();
    assert!(n.dialogs(0).iter().any(|x| x == "lan4"));
    assert_eq!(n.phase(0), NetPhase::Lobby);
    n.join_saved(2);
    n.frames_until("START visible", |l| l.lan4_start_shown(0));
    n.runs[1].apply(UiEffect::Net(AppCmd::StartSavedGame));
    assert!(n.runs[1].host.app.queue().is_empty(), "Client");
    assert_eq!(n.phase(1), NetPhase::Lobby);
    n.runs[0].apply(UiEffect::Net(AppCmd::StartSavedGame));
    let queued: Vec<AppCmd> = n.runs[0].host.app.queue().iter().cloned().collect();
    assert_eq!(
        queued,
        [close_lan4(), AppCmd::WaitDialogs, AppCmd::StartMatch]
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Start of a loaded network match (deviation from quirks E-275 (loading path), E-324, E-328): `START` in `LAN4`
// continues the match on every machine from its own file (checksum, tick, start package, own slot), with the load
// screen of savegame loading, the host's game speed, start lock and `Go`; turn 0 begins at the tick of the savegame
// (origin). Dropout, giving up, abort by the host and resync as in every network match.

use crate::game_save::CameraSave;

/// Tick of the savegame `bytes` (origin of the loaded match).
fn steps_of(bytes: &[u8]) -> u64 {
    GameSaveFile::from_bytes(bytes).unwrap().sim.state.steps
}

impl Lan {
    /// Machine `i` is in the network match after `Go`.
    fn after_go(&self, i: usize) -> bool {
        self.phase(i) == NetPhase::Match && self.runs[i].view.net().is_some_and(NetDriver::started)
    }

    /// `START` in `LAN4` of the host; frames until every machine is in the match after `Go` (also `UI_GOLDEN`).
    pub(in crate::game_flow) fn start_loaded(&mut self) {
        self.frames_until("START visible", |l| l.lan4_start_shown(0));
        self.click(0, "lan4", "start");
        self.frames_until("all after Go", |l| (0..l.runs.len()).all(|i| l.after_go(i)));
    }

    /// Frames until every machine has computed at least tick `steps`; the checksums `(tick, checksum)` per machine.
    fn hashes_until(&mut self, steps: u64) -> Vec<Vec<(u64, u64)>> {
        let mut seqs = vec![Vec::new(); self.runs.len()];
        for _ in 0..MAX_LAN_FRAMES {
            for (i, run) in self.runs.iter().enumerate() {
                collect_lan(&mut seqs[i], run);
            }
            if self.runs.iter().all(|r| r.view.steps() >= steps) {
                return seqs;
            }
            self.frame();
        }
        panic!("tick {steps} not reached");
    }

    /// End of the match by the host for all ([`end_for_all`]; the host's giving up is its own defeat): "OK" of the
    /// message on every machine, accounting, "Main menu".
    pub(in crate::game_flow) fn host_ends_to_the_main_menu(&mut self) {
        end_for_all(&mut self.runs[0]);
        self.frames_until("message on all", |l| {
            l.runs.iter().all(|r| message(r).is_some())
        });
        self.settle();
        for run in &mut self.runs {
            run.click("msgbox", "ok");
        }
        self.frames_until("accounting on all", |l| l.runs.iter().all(in_accounting));
        self.settle();
        for run in &mut self.runs {
            leave_accounting(run);
        }
        self.frames_until("main menu on all", |l| l.runs.iter().all(in_main_menu));
    }
}

/// Sequence `seq` from `from` to `to` (both inclusive).
fn window(seq: &[(u64, u64)], from: u64, to: u64) -> Vec<(u64, u64)> {
    seq.iter()
        .copied()
        .filter(|&(s, _)| (from..=to).contains(&s))
        .collect()
}

/// The loaded match of the three machines (Anna, Bert, Carl with their files) after `Go`.
fn loaded_match() -> Option<(Lan, &'static [Vec<u8>])> {
    loaded_match_listening(true)
}

/// Like [`loaded_match`]; `listen` as for [`saved_lan_listening`].
fn loaded_match_listening(listen: bool) -> Option<(Lan, &'static [Vec<u8>])> {
    let (mut n, saves) = full_lan4_listening(listen)?;
    n.start_loaded();
    Some((n, saves))
}

/// Anna and Bert play 300 ticks, Anna saves via `SAVEGAME2`, ends, both go to the main menu. Anna loads the savegame
/// in `LAN3` (`CREATE`), Bert joins, `START`: both continue their own file and compute 400 ticks with equal checksums;
/// the sequence begins at the tick of the savegame, turn 0 there (deviation from quirk E-275, loading path).
#[test]
fn saved_game_resumes_with_equal_hashes() {
    let Some((mut n, _)) = ready_pair() else {
        return;
    };
    lan_match(&mut n, 300);
    // Saving via `SAVEGAME2` (`0x004D735E`): network folder, name in the name buffer (`GameApp+0x177C`, which
    // `FILENAME`
    // shows), `SAVE`; written after the execution tick.
    n.escape(0);
    n.click(0, NET_MENU, "savegame2");
    assert!(
        n.dialogs(0).iter().any(|x| x == "savegame"),
        "{:?}",
        n.dialogs(0)
    );
    n.runs[0].host.app.screen_state().save_name.set(SAVED);
    n.click(0, "savegame", "save");
    n.frames_until("both have saved", |l| {
        (0..2).all(|i| l.paths(i).net_save(SAVED).exists())
    });
    let saved = [0, 1].map(|i| std::fs::read(n.paths(i).net_save(SAVED)).unwrap());
    let origin = steps_of(&saved[0]);
    assert!(origin > 300, "saved after tick 300: {origin}");
    n.host_ends_to_the_main_menu();
    // Anna: main menu `LAN1` (`{3,"start"}`, `{5,"lan1"}`) -> `LAN3`, "Load game", choose the savegame, `CREATE`.
    n.settle();
    n.click(0, "start", "lan1");
    assert!(
        n.dialogs(0).iter().any(|x| x == "lan1"),
        "{:?}",
        n.dialogs(0)
    );
    n.click(0, "lan1", "create");
    let combo = element(&n, 0, "lan3", "leveltype").expect("LEVELTYPE");
    n.combo_row(0, "lan3", "leveltype", combo.items.len() - 1);
    assert_eq!(n.runs[0].host.app.config().level_type, 2);
    let list = element(&n, 0, "loadsavednetworkgame", "dir").expect("list");
    let row = list
        .items
        .iter()
        .position(|t| t.ends_with(&format!(") {SAVED}")))
        .unwrap_or_else(|| panic!("{SAVED} in {:?}", list.items));
    n.list_row(0, "loadsavednetworkgame", "dir", row);
    n.click(0, "lan3", "create");
    assert!(
        n.dialogs(0).iter().any(|x| x == "lan4"),
        "{:?}",
        n.dialogs(0)
    );
    n.join_saved(1);
    n.start_loaded();
    for i in 0..2 {
        let net = n.runs[i].view.net().unwrap();
        assert_eq!(
            net.origin(),
            origin,
            "machine {i}: origin at the tick of the savegame"
        );
    }
    let seqs = n.hashes_until(origin + 400);
    let host = window(&seqs[0], origin, origin + 400);
    assert_eq!(
        host.len(),
        401,
        "sequence of the host without gaps from the savegame"
    );
    assert_eq!(
        host[0].0, origin,
        "the sequence begins at the tick of the savegame"
    );
    assert_eq!(
        window(&seqs[1], origin, origin + 400),
        host,
        "equal checksums from the savegame"
    );
    for (i, bytes) in saved.iter().enumerate() {
        assert_eq!(n.phase(i), NetPhase::Match);
        assert!(n.runs[i].flow.running(), "machine {i}");
        assert_eq!(
            n.runs[i].view.sim().unwrap().active_player,
            local_slot(bytes),
            "machine {i}: own slot from the own file"
        );
    }
}

/// Every machine continues camera and masks of its own file (the camera is not set to the start of the slot); the
/// slot is the own one.
#[test]
fn each_machine_keeps_its_own_camera_and_masks() {
    let camera = |i: usize| CameraSave {
        from: [1000.0 + 100.0 * i as f32, 2500.0, 400.0],
        at: [1200.0 + 50.0 * i as f32, 100.0, 1300.0],
        fov: 0.6 + 0.1 * i as f32,
    };
    let masks = |i: usize| crate::game::diplomacy::Masks {
        chat: [0x10 * (i as u32 + 1); 8],
        tribute: [0x0F - i as u32; 8],
    };
    let Some(files) = (0..3)
        .map(|i| {
            changed_save(i, |f| {
                f.host.camera = camera(i);
                f.host.masks = masks(i);
            })
        })
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&files[0]), Some(&files[1]), Some(&files[2])],
    )
    .unwrap();
    n.join_saved(1);
    n.join_saved(2);
    n.start_loaded();
    n.frames(20);
    for (i, file) in files.iter().enumerate() {
        let run = &n.runs[i];
        assert_eq!(run.view.camera(), camera(i), "machine {i}: camera");
        assert_eq!(run.view.masks, masks(i), "machine {i}: masks");
        assert_eq!(
            run.host.app.game().map(|g| g.local_slot),
            Some(local_slot(file)),
            "machine {i}"
        );
    }
}

/// After loading, every machine waits at the tick of the savegame for `Go` as long as one is still loading (Carl
/// halts; start lock, deviation from quirk E-328); nobody computes until then.
#[test]
fn loaded_start_waits_for_go() {
    let Some((mut n, saves)) = full_lan4() else {
        return;
    };
    let origin = steps_of(&saves[0]);
    n.frames_until("START visible", |l| l.lan4_start_shown(0));
    n.runs[0].click("lan4", "start");
    // From `START` Carl does not compute for 60 frames (his clock runs along): he does not get the start until
    // afterwards, no `Loaded` from him, no `Go`; host and Bert load.
    for _ in 0..60 {
        n.net.advance_to(n.runs[0].now);
        n.runs[0].frame();
        n.runs[1].frame();
        n.runs[2].now += 16;
    }
    assert!(
        n.runs[0].flow.running() && n.runs[1].flow.running(),
        "host and Bert have loaded"
    );
    assert!(!n.runs[2].flow.running(), "Carl is not loading yet");
    for i in 0..2 {
        let run = &n.runs[i];
        assert_eq!(
            run.view.steps(),
            origin,
            "machine {i} stands at the tick of the savegame"
        );
        assert!(!run.view.net().unwrap().started(), "machine {i}: no Go");
        assert_eq!(n.phase(i), NetPhase::Loading, "machine {i}");
    }
    n.frames_until("all after Go", |l| (0..3).all(|i| l.after_go(i)));
    n.frames_until("all compute", |l| {
        l.runs.iter().all(|r| r.view.steps() > origin + 10)
    });
}

/// The start of a loaded network match shows the loading screen of savegame loading on every machine
/// (`ldg_path_for(..., true, ...)`, without an own `.ldg` `default.ldg`), not `multiplayer.ldg` of the network start.
#[test]
fn loaded_start_uses_the_load_screen_of_saved_games() {
    let Some((n, saves)) = loaded_match() else {
        return;
    };
    for (i, _) in saves.iter().enumerate() {
        // Fixed map `[3players]-canonballcanyon.lvl` without an own `.ldg`: fallback `default.ldg` (hard-coded, not
        // computed with `saved_ldg`).
        let expected = "data/gifcon/default.ldg";
        assert_ne!(expected, MULTIPLAYER_LDG);
        let shown = &n.runs[i].shown;
        assert!(
            !shown.is_empty() && shown.iter().all(|(ldg, _)| *ldg == expected),
            "machine {i}: {shown:?}"
        );
        assert_eq!(
            shown.iter().map(|(_, p)| *p).collect::<Vec<_>>(),
            [0, 40, 70, 100],
            "machine {i}: phases"
        );
    }
}

/// The game speed comes from the host's file, not from the own host part.
#[test]
fn loaded_game_takes_the_hosts_speed() {
    let speeds = [7, 15, 3];
    let Some(files) = (0..3)
        .map(|i| changed_save(i, |f| f.host.game_speed = speeds[i]))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&files[0]), Some(&files[1]), Some(&files[2])],
    )
    .unwrap();
    n.join_saved(1);
    n.join_saved(2);
    n.start_loaded();
    for i in 0..3 {
        assert_eq!(
            n.runs[i].view.sim().unwrap().game_speed,
            7,
            "machine {i}: game speed of the host"
        );
    }
}

/// If the host's file carries a game speed above the protocol rule (30), all machines start with 20.
#[test]
fn loaded_game_clamps_the_hosts_speed_to_the_net_limit() {
    let Some(files) = (0..3)
        .map(|i| changed_save(i, |f| f.host.game_speed = 30))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let mut n = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&files[0]), Some(&files[1]), Some(&files[2])],
    )
    .unwrap();
    n.join_saved(1);
    n.join_saved(2);
    n.start_loaded();
    for i in 0..3 {
        assert_eq!(
            n.runs[i].view.sim().unwrap().game_speed,
            20,
            "machine {i}: game speed limited to 20"
        );
    }
}

/// If a client drops out after loading (Carl crashes, without `Bye`; deviation from quirk E-324), his slot loses as in
/// every network match: result 1 on host and Bert, message 17200 + p on the host; the others compute on.
#[test]
fn lost_client_after_load_loses_its_slot() {
    let Some((mut n, saves)) = loaded_match() else {
        return;
    };
    let slot = local_slot(&saves[2]);
    n.frames(10);
    n.runs[2].view.end();
    n.frames_until("slot lost", |l| {
        (0..2).all(|i| l.runs[i].view.sim().unwrap().state.players[usize::from(slot)].result == 1)
    });
    let shown = &n.runs[0].view.game.as_ref().unwrap().player_messages;
    assert!(
        shown
            .iter()
            .any(|&(_, id)| id == DEFEAT_MESSAGE + i32::from(slot)),
        "message 17200 + {slot} on the host: {shown:?}"
    );
    let at = n.runs[0].view.steps();
    n.frames_until("host and Bert compute on", |l| {
        (0..2).all(|i| l.runs[i].view.steps() >= at + 40)
    });
    for i in 0..2 {
        assert_eq!(message(&n.runs[i]), None, "machine {i}");
        assert!(n.runs[i].flow.running(), "machine {i}");
    }
}

/// If the host drops out after loading, each client writes its network savegame `netz_YYYY_MM_DD_hhmm` with the same
/// simulation part at the tick where it stands (after the origin) and shows the abort message. The clients do not
/// listen: without candidates no host migration.
#[test]
fn host_loss_after_load_ends_with_a_net_save() {
    let Some((mut n, saves)) = loaded_match_listening(false) else {
        return;
    };
    let origin = steps_of(&saves[0]);
    n.frames_until("all compute", |l| {
        l.runs.iter().all(|r| r.view.steps() >= origin + 20)
    });
    n.runs[0].view.end();
    n.frames_until("message on both clients", |l| {
        l.runs[1..].iter().all(|r| message(r).is_some())
    });
    let name = NET_CLOCK.net_end_name();
    let files: Vec<GameSaveFile> = (1..3)
        .map(|i| {
            GameSaveFile::from_bytes(&std::fs::read(n.paths(i).net_save(&name)).unwrap()).unwrap()
        })
        .collect();
    for (file, i) in files.iter().zip(1..3) {
        let text = message(&n.runs[i]).unwrap();
        assert!(text.contains(&name), "machine {i}: {text}");
        assert_eq!(file.sim.state.steps, n.runs[i].view.steps(), "machine {i}");
        assert!(file.sim.state.steps > origin, "machine {i}");
        assert_eq!(file.sim.to_bytes(), files[0].sim.to_bytes(), "machine {i}");
        assert_eq!(
            file.host.net.as_ref().unwrap().local_slot,
            local_slot(&saves[i])
        );
    }
}

/// Giving up after loading goes through the turn: Bert's end screen shows the defeat, on the host his slot is
/// eliminated (message 17200 + p); host and Carl compute on.
#[test]
fn surrender_after_load_works() {
    let Some((mut n, saves)) = loaded_match() else {
        return;
    };
    n.frames(10);
    let slot = n.surrender_to_the_end_screen(1);
    assert_eq!(slot, local_slot(&saves[1]));
    n.frames_until("result on the host", |l| {
        l.runs[0].view.sim().unwrap().state.players[usize::from(slot)].result == 1
    });
    let shown = &n.runs[0].view.game.as_ref().unwrap().player_messages;
    assert!(
        shown
            .iter()
            .any(|&(_, id)| id == DEFEAT_MESSAGE + i32::from(slot)),
        "message 17200 + {slot} on the host: {shown:?}"
    );
    let at = n.runs[0].view.steps();
    n.frames_until("host and Carl compute on", |l| {
        [0, 2].iter().all(|&i| l.runs[i].view.steps() >= at + 40)
    });
    assert_eq!(
        n.runs[2].view.sim().unwrap().state.players[usize::from(slot)].result,
        1
    );
}

/// All three are in the main menu without a match after a failed start (a message confirmed first with "OK"), the
/// sessions are closed.
fn assert_all_back_in_the_main_menu(n: &mut Lan) {
    n.frames_until("message or main menu on all", |l| {
        l.runs
            .iter()
            .all(|r| message(r).is_some() || in_main_menu(r))
    });
    n.settle();
    for run in &mut n.runs {
        if message(run).is_some() {
            run.click("msgbox", "ok");
        }
    }
    n.frames_until("main menu on all", |l| l.runs.iter().all(in_main_menu));
    for i in 0..n.runs.len() {
        assert_eq!(n.phase(i), NetPhase::Idle, "machine {i}");
        assert!(session_closed(n, i), "machine {i}");
        assert!(!n.runs[i].flow.running(), "machine {i}: no match");
        assert_eq!(n.runs[i].host.app.data().accounting, None, "machine {i}");
    }
}

/// Bert's file changes after joining (new save under the same name). On `START` his loading fails with
/// `Loaded { error }`, the start aborts for all, all land in the main menu.
#[test]
fn changed_file_before_start_aborts_the_start() {
    let Some((mut n, _)) = full_lan4() else {
        return;
    };
    n.rewrite_save(1, &other_state(1).unwrap());
    n.frames(TWO_QUERIES);
    assert!(
        n.dialogs(1).iter().any(|x| x == "lan4"),
        "Bert stays in LAN4"
    );
    n.click(0, "lan4", "start");
    n.frames_until("host aborts", |l| message(&l.runs[0]).is_some());
    let text = message(&n.runs[0]).unwrap();
    assert!(text.contains("differs from the host's"), "{text}");
    assert!(!n.runs[1].flow.running(), "Bert has not loaded");
    assert_all_back_in_the_main_menu(&mut n);
}

/// The host's file changes after hosting (new save under the same name). The checksum of the session then no longer
/// holds for the newly read file: the start is rejected on the host, `LAN4` opens again, the clients stay in `LAN4`,
/// nothing loads.
#[test]
fn changed_host_file_before_start_reopens_lan4() {
    let Some((mut n, _)) = full_lan4() else {
        return;
    };
    n.rewrite_save(0, &other_state(0).unwrap());
    n.frames(TWO_QUERIES);
    n.click(0, "lan4", "start");
    n.settle();
    n.frames(70);
    let d = n.dialogs(0);
    assert!(d.iter().any(|x| x == "lan4"), "{d:?}");
    assert!(!d.iter().any(|x| x == "lan2" || x == "levelinfo"), "{d:?}");
    assert_eq!(n.phase(0), NetPhase::Lobby);
    for i in 0..3 {
        assert!(!n.runs[i].flow.running(), "machine {i}: no match");
        assert!(n.runs[i].shown.is_empty(), "machine {i}: no loading");
    }
    for i in 1..3 {
        assert!(
            n.dialogs(i).iter().any(|x| x == "lan4"),
            "machine {i} stays in LAN4"
        );
        assert_eq!(n.phase(i), NetPhase::Lobby, "machine {i}");
    }
}

/// Carl leaves `LAN4` while loading after `START` (host has loaded, Carl not yet): the start aborts for all, no match
/// hangs, all land in the main menu.
#[test]
fn client_gone_while_loading_a_saved_game_aborts_for_all() {
    let Some((mut n, _)) = full_lan4() else {
        return;
    };
    n.runs[0].click("lan4", "start");
    n.frames_until("host has loaded", |l| l.runs[0].flow.running());
    assert_eq!(
        n.phase(2),
        NetPhase::Lobby,
        "Carl does not have the start yet"
    );
    n.runs[2].apply(UiEffect::Net(AppCmd::LeaveLobby));
    n.frames_until("host aborts", |l| message(&l.runs[0]).is_some());
    let text = message(&n.runs[0]).unwrap();
    assert!(text.contains("dropped out while loading"), "{text}");
    assert_all_back_in_the_main_menu(&mut n);
}

/// The loaded match is saved again (the origin is not on the raster of tick 0) and loaded again; all compute equally
/// from the new savegame (second generation).
#[test]
fn a_reloaded_save_can_be_saved_and_loaded_again() {
    const AGAIN: &str = "again";
    let Some((mut n, saves)) = loaded_match() else {
        return;
    };
    let origin = steps_of(&saves[0]);
    let raster = n.runs[0].view.net().unwrap().raster();
    let (t, v) = (raster.spacing(), raster.lead());
    n.frames_until("all compute", |l| {
        l.runs.iter().all(|r| r.view.steps() >= origin + 20)
    });
    n.runs[0].apply(UiEffect::SaveGame { name: AGAIN.into() });
    n.frames_until("all have saved", |l| {
        (0..3).all(|i| l.paths(i).net_save(AGAIN).exists())
    });
    let again: Vec<Vec<u8>> = (0..3)
        .map(|i| std::fs::read(n.paths(i).net_save(AGAIN)).unwrap())
        .collect();
    let second = steps_of(&again[0]);
    assert_eq!(
        (second - origin) % t,
        v + 1,
        "after e(k) of the shifted raster"
    );
    assert_ne!(second % t, (v + 1) % t, "not on the raster of tick 0");
    for (i, bytes) in again.iter().enumerate() {
        let file = GameSaveFile::from_bytes(bytes).unwrap();
        assert_eq!(file.sim.state.steps, second, "machine {i}");
        assert_eq!(file.host.net.unwrap().local_slot, local_slot(&saves[i]));
    }
    // Second generation: the same three with the new files as "netz".
    let mut m = saved_lan(
        &["Anna", "Bert", "Carl"],
        &[Some(&again[0]), Some(&again[1]), Some(&again[2])],
    )
    .unwrap();
    m.join_saved(1);
    m.join_saved(2);
    m.start_loaded();
    assert_eq!(m.runs[0].view.net().unwrap().origin(), second);
    let seqs = m.hashes_until(second + 100);
    let host = window(&seqs[0], second, second + 100);
    assert_eq!(host.len(), 101);
    for (i, seq) in seqs.iter().enumerate().skip(1) {
        assert_eq!(window(seq, second, second + 100), host, "machine {i}");
    }
}

/// If Carl deviates in the loaded match (`debug_corrupt`), he takes over the host's state on resync; afterwards all
/// compute equally. The replay stays complete after loading (the log is in the savegame and the resync takes over the
/// host's).
#[test]
fn resync_after_load_keeps_equal_hashes() {
    let Some((mut n, saves)) = loaded_match() else {
        return;
    };
    let origin = steps_of(&saves[0]);
    n.frames_until("all compute", |l| {
        l.runs.iter().all(|r| r.view.steps() >= origin + 20)
    });
    assert_eq!(n.runs[2].view.sim_mut().unwrap().debug_corrupt(), "daytime");
    let seqs = n.hashes_until(origin + 200);
    let host = window(&seqs[0], origin + 100, origin + 200);
    assert_eq!(host.len(), 101);
    for (i, seq) in seqs.iter().enumerate().skip(1) {
        assert_eq!(window(seq, origin + 100, origin + 200), host, "machine {i}");
    }
    let net = n.runs[0].view.net().unwrap();
    assert_eq!(net.status().desyncs, Some(1));
    assert_eq!(
        n.runs[2].view.game.as_ref().unwrap().rebuilds,
        1,
        "Carl took over"
    );
    let replay = n.runs[0].view.sim().unwrap().replay().expect("host");
    for i in 1..3 {
        assert_eq!(
            n.runs[i].view.sim().unwrap().replay().unwrap(),
            replay,
            "machine {i}"
        );
    }
}

/// Every network savegame carries the command log from the start of the match, the same on all machines; after
/// loading the replay is complete and begins with this log.
#[test]
fn net_save_carries_the_log_and_the_reload_replays() {
    let Some((mut n, saves)) = loaded_match() else {
        return;
    };
    let logs: Vec<_> = saves
        .iter()
        .map(|bytes| GameSaveFile::from_bytes(bytes).unwrap().host.log)
        .collect();
    assert!(logs[0].is_some(), "log in the savegame of the host");
    assert_eq!(logs[1], logs[0], "Bert");
    assert_eq!(logs[2], logs[0], "Carl");
    let saved = logs[0].clone().unwrap();
    let origin = steps_of(&saves[0]);
    n.frames_until("all compute", |l| {
        l.runs.iter().all(|r| r.view.steps() >= origin + 20)
    });
    for i in 0..3 {
        let sim = n.runs[i].view.sim().unwrap();
        assert!(sim.log_complete(), "machine {i}");
        let replay = sim.replay().unwrap();
        assert!(replay.commands.starts_with(&saved), "machine {i}");
    }
}

/// If the start of a loaded lobby fails on the host (Bert leaves between `{0x14}` and `{0x0D}`), `LAN4` opens again
/// (not `LAN2`), and nothing loads.
#[test]
fn rejected_loaded_start_reopens_lan4() {
    let Some((mut n, _)) = full_lan4() else {
        return;
    };
    n.runs[0].click("lan4", "start");
    n.runs[1].apply(UiEffect::Net(AppCmd::CloseNet));
    n.settle();
    n.frames(70);
    let d = n.dialogs(0);
    assert!(d.iter().any(|x| x == "lan4"), "{d:?}");
    assert!(!d.iter().any(|x| x == "lan2" || x == "levelinfo"), "{d:?}");
    assert_eq!(n.phase(0), NetPhase::Lobby);
    assert!(!n.runs[0].flow.running(), "no match");
    assert!(
        n.runs[0].shown.is_empty(),
        "no loading: {:?}",
        n.runs[0].shown
    );
    assert!(
        n.lobby(0).rows.iter().any(|r| r.peer.is_none()),
        "Bert's row open"
    );
    assert!(!n.lan4_start_shown(0));
}

// ---------------------------------------------------------------------------------------------------------------
// Recording of a loaded network match.

/// Anna (host, `SAVEREPLAY` in `LAN3`) and Bert play with walk commands, Anna saves, ends, loads the savegame via
/// `LAN4`. Every machine begins a new recording with the whole log of its savegame from tick 0 (switch from the own
/// host part, not from the session of the loaded lobby); both files carry the same commands, and the replay equals the
/// match across the load point.
#[test]
fn loaded_net_game_records_from_the_start() {
    use crate::game_flow::replay_tests::{
        LATER, LATER_NAME, NAME, read_recording, recordings, replayed_hash, walk,
    };
    let Some(mut n) = Lan::corpus_with(&["Anna", "Bert"], |i, cfg| {
        if i == 0 {
            cfg.save_replay = 1;
        }
    }) else {
        return;
    };
    n.host(0, Some("tinte"));
    n.join(1);
    n.all_ready(&[(0, 0), (1, 1)]);
    lan_match(&mut n, 10);
    walk(&mut n.runs[0], 400.0);
    walk(&mut n.runs[1], -400.0);
    n.frames_until("tick 200", |l| l.runs.iter().all(|r| r.view.steps() >= 200));
    n.escape(0);
    n.click(0, NET_MENU, "savegame2");
    n.runs[0].host.app.screen_state().save_name.set(SAVED);
    n.click(0, "savegame", "save");
    n.frames_until("both have saved", |l| {
        (0..2).all(|i| l.paths(i).net_save(SAVED).exists())
    });
    let saves = [0, 1].map(|i| {
        GameSaveFile::from_bytes(&std::fs::read(n.paths(i).net_save(SAVED)).unwrap()).unwrap()
    });
    let origin = saves[0].sim.state.steps;
    for (i, save) in saves.iter().enumerate() {
        assert!(
            save.host.save_replay,
            "machine {i}: SAVEREPLAY in the host part"
        );
        assert!(
            save.host.log.as_ref().is_some_and(|l| l.len() >= 2),
            "machine {i}: log with the walk commands"
        );
    }
    n.host_ends_to_the_main_menu();
    for run in &mut n.runs {
        run.flow.set_fixed_clock(Some(LATER));
    }
    // Loading as in `saved_game_resumes_with_equal_hashes`.
    n.settle();
    n.click(0, "start", "lan1");
    n.click(0, "lan1", "create");
    let combo = element(&n, 0, "lan3", "leveltype").expect("LEVELTYPE");
    n.combo_row(0, "lan3", "leveltype", combo.items.len() - 1);
    let list = element(&n, 0, "loadsavednetworkgame", "dir").expect("list");
    let row = list
        .items
        .iter()
        .position(|t| t.ends_with(&format!(") {SAVED}")))
        .unwrap_or_else(|| panic!("{SAVED} in {:?}", list.items));
    n.list_row(0, "loadsavednetworkgame", "dir", row);
    n.click(0, "lan3", "create");
    n.join_saved(1);
    assert!(
        !n.lobby(1).session.settings.save_replay,
        "the loaded lobby does not carry the switch"
    );
    n.start_loaded();
    for (i, save) in saves.iter().enumerate() {
        assert_eq!(recordings(&n.runs[i]), [NAME, LATER_NAME], "machine {i}");
        let file = read_recording(&n.runs[i], LATER_NAME);
        let saved_log = save.host.log.as_ref().unwrap();
        assert!(
            file.commands.starts_with(saved_log),
            "machine {i}: the whole log from tick 0"
        );
        assert_eq!(file.head.start, save.sim.start, "machine {i}");
    }
    walk(&mut n.runs[0], -200.0);
    walk(&mut n.runs[1], 200.0);
    let at = origin + 100;
    let seqs = n.hashes_until(at);
    let hash = window(&seqs[0], at, at)[0].1;
    assert_eq!(window(&seqs[1], at, at)[0].1, hash);
    let rules = n.runs[0].view.sim().unwrap().rules.clone();
    let files = [0, 1].map(|i| read_recording(&n.runs[i], LATER_NAME));
    let until = |f: &crate::replay_file::ReplayFile| -> Vec<_> {
        f.commands.iter().filter(|c| c.step < at).cloned().collect()
    };
    assert_eq!(until(&files[0]), until(&files[1]), "same commands");
    assert!(
        until(&files[0]).len() > saves[0].host.log.as_ref().unwrap().len(),
        "the walk commands after loading"
    );
    for (i, file) in files.iter().enumerate() {
        assert_eq!(
            replayed_hash(file, &rules, at),
            hash,
            "machine {i}: across the load point"
        );
    }
}
