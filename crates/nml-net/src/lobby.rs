//! Lobby state and lobby rules (waiting room `LAN2`): pure rules without a network.
//!
//! In `Gathering` the host holds a [`LobbyBook`] and distributes the whole [`LobbyState`] after every change.
//! Clients send only wishes ([`LobbyWish`]) for their own row; the sender comes from the connection, not from the
//! message (E-280 deviation). The rights are those of the original's UI (re/multiplayer-menus L.3.1, L.3.3, L.3.4):
//! the host changes the type of open and computer rows and humans only to "Open" (= kick), everyone sets nation,
//! team, color, handicap and Ready of their row, the host additionally those of open and computer rows; row 0
//! (`NAME0`) can never be changed.
//!
//! Loaded lobby (waiting room `LAN4`, [`LobbyBook::loaded`]): one row per human slot in the match with the slot per
//! row in [`LobbyState::seats`]; the host stands in the row of its slot, clients join only in their own slot
//! ([`LobbyBook::join_seat`]). There are no wishes, Ready, host changes or map changes there.

use serde::{Deserialize, Serialize};

use crate::lockstep::HOST_PEER;
use crate::msg::{clean_name, clean_session_name, valid_save_name};
use crate::turn::TurnConfig;

/// Type "Human" of a row (like `SlotDraft::kind`).
pub const KIND_HUMAN: i32 = 999;
/// Game type 4 ("Train"): `FUN_004BA980` does not apply.
const RAILROAD: i32 = 4;
/// Last color number (0 ... 8).
const LAST_COLOR: i32 = 8;
/// Largest handicap (computer "Easy" = 5).
const MAX_HANDICAP: i32 = 5;
/// Largest player count of a map.
pub const MAX_ROWS: usize = 8;
/// Reason for rejected wishes, host changes and map changes in a loaded lobby.
const LOADED_FIXED: &str = "The lobby cannot be changed in a loaded match.";
/// Reason when a new lobby is to get a network savegame as its map: that works only through [`LobbyBook::loaded`].
const SAVED_NEEDS_LOADED: &str = "A savegame as a map needs a loaded lobby.";

/// Map of a session: fixed map (path and checksum), random map, or network savegame (name under
/// `savegame/multiplayer/` and checksum of the simulation part).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum MapId {
    Fixed {
        path: String,
        checksum: u64,
    },
    Random {
        seed_text: String,
        size: i32,
        players: i32,
        terrain: i32,
        landscape: i32,
        daytime: i32,
    },
    Saved {
        name: String,
        checksum: u64,
    },
}

/// Game settings of the session (values as in the settings of the UI).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct SessionSettings {
    pub game_type: i32,
    pub unit_limit: i32,
    pub resources: i32,
    pub time_limit: i32,
    pub lock_teams: bool,
    pub map_explored: bool,
    pub save_replay: bool,
}

/// Session data: name, map, settings, player count and turn raster (`T`/`V` in ticks).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub name: String,
    pub map: MapId,
    pub map_name: String,
    pub settings: SessionSettings,
    pub players: u8,
    pub spacing: u8,
    pub lead: u8,
}

impl SessionInfo {
    /// Session with fixed map `path` (checksum 0, map name = path), default settings and grid `cfg` (command-line path
    /// and tests; the menu path fills all fields itself).
    pub fn plain(name: &str, path: &str, players: u8, cfg: TurnConfig) -> Self {
        let (spacing, lead) = cfg.raw();
        Self {
            name: name.to_string(),
            map: MapId::Fixed {
                path: path.to_string(),
                checksum: 0,
            },
            map_name: path.to_string(),
            settings: SessionSettings::default(),
            players,
            spacing,
            lead,
        }
    }

    /// Checks player count (2 ... 8) and grid (`TurnConfig::new`).
    fn validate(&self) -> Result<(), String> {
        if !(2..=MAX_ROWS as u8).contains(&self.players) {
            return Err(format!(
                "The player count {} is not between 2 and {MAX_ROWS}.",
                self.players
            ));
        }
        TurnConfig::new(self.spacing, self.lead).map(|_| ())
    }

    /// Sanitizes session and map name to 39 characters without brackets; map path and seed stay (identifiers). The
    /// client also accepts the host's state this way.
    pub(crate) fn cleaned(mut self) -> Self {
        self.name = clean_session_name(&self.name);
        self.map_name = clean_session_name(&self.map_name);
        self
    }
}

/// Map change (E-307 deviation): do the rows `rows` fit a session with `players` players? No as soon as a row that
/// would be dropped (`≥ players`) is occupied (type != 0: human or computer); open rows are dropped. The same rule
/// checks [`LobbyBook::set_session`]; the game host asks beforehand (`LAN31` then shows 15402), the core judges, so
/// that a join in between does not delete anything occupied.
pub fn rows_fit(rows: &[LobbyRow], players: u8) -> bool {
    rows.iter().skip(usize::from(players)).all(|r| r.kind == 0)
}

/// A lobby row. `kind`: 0 open, 1 ... 4 computer (Easy, Normal, Hard, Manual), 999 human.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LobbyRow {
    pub kind: i32,
    pub name: String,
    pub nation: i32,
    pub team: i32,
    pub handicap: i32,
    pub color: i32,
    pub ready: bool,
    pub peer: Option<u8>,
}

/// The state that the host distributes to everyone.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LobbyState {
    pub revision: u32,
    pub session: SessionInfo,
    pub rows: Vec<LobbyRow>,
    /// Largest latency of all clients in milliseconds (display in `LEVELINFO`).
    pub max_rtt_ms: Option<u32>,
    /// Player slot per row in a loaded match, strictly increasing; empty for a new match.
    pub seats: Vec<u8>,
}

impl LobbyState {
    /// Lobby state of a loaded match ([`LobbyBook::loaded`]).
    pub fn is_loaded(&self) -> bool {
        !self.seats.is_empty()
    }
}

/// Wish of a client (only for its own row).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LobbyWish {
    Row {
        nation: i32,
        team: i32,
        handicap: i32,
        color: i32,
    },
    Ready(bool),
    Chat(String),
}

/// Change by the host.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum HostEdit {
    Kind {
        row: u8,
        kind: i32,
    },
    Row {
        row: u8,
        nation: i32,
        team: i32,
        handicap: i32,
        color: i32,
    },
    Ready {
        row: u8,
        ready: bool,
    },
}

/// Result of a host change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Changed,
    /// A human was set to "Open": kick of the machine `peer`.
    Kicked {
        peer: u8,
    },
}

/// Verdict of the team/color check (`FUN_004BA710`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleVerdict {
    Accept,
    Reject,
    /// With game type 4 the original sets the team of the other row `row` to `team`.
    Swap {
        row: u8,
        team: i32,
    },
}

/// Team and color rules, set by the game host (in `nml-client` with `FUN_004BA710`).
pub trait LobbyRules: Send {
    /// `proposed` is `rows[row]` with the proposed change.
    fn check(&self, rows: &[LobbyRow], row: u8, proposed: &LobbyRow, game_type: i32)
    -> RuleVerdict;
}

/// Rules that accept everything (tests, command-line path).
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptAll;

impl LobbyRules for AcceptAll {
    fn check(&self, _: &[LobbyRow], _: u8, _: &LobbyRow, _: i32) -> RuleVerdict {
        RuleVerdict::Accept
    }
}

/// The lobby state of the host.
#[derive(Debug, Clone)]
pub struct LobbyBook {
    state: LobbyState,
}

impl LobbyBook {
    /// New state. Row 0 is the host (type 999, peer 0, Ready 0); all further rows are without a machine and not of type
    /// human. Checks player count, row count and grid. A network savegame as the map goes only through
    /// [`Self::loaded`].
    pub fn new(
        session: SessionInfo,
        mut rows: Vec<LobbyRow>,
        host_name: String,
    ) -> Result<Self, String> {
        session.validate()?;
        if matches!(session.map, MapId::Saved { .. }) {
            return Err(SAVED_NEEDS_LOADED.to_string());
        }
        if rows.len() != usize::from(session.players) {
            return Err(format!(
                "The session has {} players but {} rows.",
                session.players,
                rows.len()
            ));
        }
        for (i, row) in rows.iter_mut().enumerate() {
            row.peer = None;
            if i == 0 {
                row.kind = KIND_HUMAN;
                row.name = clean_name(&host_name);
                row.peer = Some(0);
                row.ready = false;
            } else if !(0..=4).contains(&row.kind) {
                return Err(format!("Row {i} has the invalid type {}.", row.kind));
            }
        }
        Ok(Self {
            state: LobbyState {
                revision: 0,
                session: session.cleaned(),
                rows,
                max_rtt_ms: None,
                seats: Vec::new(),
            },
        })
    }

    /// State with only open rows behind the host (nation 0, team and color = row): command-line path and tests.
    pub fn open(session: SessionInfo, host_name: &str) -> Result<Self, String> {
        let rows = (0..session.players)
            .map(|i| LobbyRow {
                kind: 0,
                name: String::new(),
                nation: 0,
                team: i32::from(i),
                handicap: 0,
                color: i32::from(i),
                ready: false,
                peer: None,
            })
            .collect();
        Self::new(session, rows, host_name.to_string())
    }

    /// Lobby state of a loaded match (E-325, E-327 deviation): one row per human slot in the match, `seats` the player
    /// slot per row. Nation, team, color and handicap come from `rows` (the state); the row of the slot `host_seat`
    /// belongs to the host (type 999, peer 0, sanitized name), all others are open (type 0, without name and machine);
    /// nobody is ready (there is no Ready here). Checks session (player count 2 ... 8, grid), map `MapId::Saved` with a
    /// valid savegame name ([`valid_save_name`]), `players == rows.len() == seats.len()`, slots <= 7 strictly
    /// increasing and `host_seat` among them.
    pub fn loaded(
        session: SessionInfo,
        mut rows: Vec<LobbyRow>,
        seats: Vec<u8>,
        host_seat: u8,
        host_name: &str,
    ) -> Result<Self, String> {
        session.validate()?;
        match &session.map {
            MapId::Saved { name, .. } if valid_save_name(name) => {}
            MapId::Saved { .. } => {
                return Err("The savegame name is not a valid file name.".to_string());
            }
            _ => return Err("The session has no savegame as its map.".to_string()),
        }
        let players = usize::from(session.players);
        if rows.len() != players || seats.len() != players {
            return Err(format!(
                "The session has {} players but {} rows and {} slots.",
                session.players,
                rows.len(),
                seats.len()
            ));
        }
        if seats.iter().any(|&s| usize::from(s) >= MAX_ROWS)
            || seats.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(format!("Invalid slots {seats:?}."));
        }
        if !seats.contains(&host_seat) {
            return Err(format!(
                "The host's slot {} is not among the slots.",
                u32::from(host_seat) + 1
            ));
        }
        for (row, &seat) in rows.iter_mut().zip(&seats) {
            row.ready = false;
            if seat == host_seat {
                row.kind = KIND_HUMAN;
                row.name = clean_name(host_name);
                row.peer = Some(HOST_PEER);
            } else {
                row.kind = 0;
                row.name.clear();
                row.peer = None;
            }
        }
        Ok(Self {
            state: LobbyState {
                revision: 0,
                session: session.cleaned(),
                rows,
                max_rtt_ms: None,
                seats,
            },
        })
    }

    pub fn state(&self) -> &LobbyState {
        &self.state
    }

    /// Frozen state for the new host of a running match: the last state the client had from the old host, without any
    /// check. Only after the start: the core then no longer changes it.
    pub fn frozen(state: LobbyState) -> Self {
        Self { state }
    }

    fn bump(&mut self) {
        self.state.revision = self.state.revision.wrapping_add(1);
    }

    /// Row of a machine.
    pub fn row_of(&self, peer: u8) -> Option<u8> {
        self.state
            .rows
            .iter()
            .position(|r| r.peer == Some(peer))
            .map(|i| i as u8)
    }

    /// A machine joins (E-282): first client index without a machine, also replacing a computer. The row becomes
    /// human with name, handicap 0, Ready 0; nation, team and color stay. In a loaded lobby only with a slot claim
    /// ([`Self::join_seat`]).
    pub fn join(&mut self, peer: u8, name: &str) -> Result<u8, String> {
        if self.state.is_loaded() {
            return Err("This match needs the savegame on this machine.".to_string());
        }
        if peer == HOST_PEER || self.row_of(peer).is_some() {
            return Err("The machine is already in the lobby.".to_string());
        }
        let Some(row) = (1..self.state.rows.len()).find(|&i| self.state.rows[i].peer.is_none())
        else {
            return Err("The match is full".to_string());
        };
        let r = &mut self.state.rows[row];
        r.kind = KIND_HUMAN;
        r.name = clean_name(name);
        r.handicap = 0;
        r.ready = false;
        r.peer = Some(peer);
        self.bump();
        Ok(row as u8)
    }

    /// Join a loaded match (E-275 deviation): the machine takes the row of the slot `seat` that it plays in its file.
    /// It becomes human with a sanitized name, not ready; nation, team, color and handicap stay those of the state.
    /// `Err` if the lobby is not loaded, the machine already has a row, the slot has no row (not a human or dropped
    /// out) or is already occupied.
    pub fn join_seat(&mut self, peer: u8, name: &str, seat: u8) -> Result<u8, String> {
        if !self.state.is_loaded() {
            return Err("This match is not a loaded savegame.".to_string());
        }
        if peer == HOST_PEER || self.row_of(peer).is_some() {
            return Err("The machine is already in the lobby.".to_string());
        }
        let shown = u32::from(seat) + 1;
        let Some(row) = self.state.seats.iter().position(|&s| s == seat) else {
            return Err(format!("Slot {shown} has no player left in this savegame."));
        };
        let r = &mut self.state.rows[row];
        if r.peer.is_some() {
            return Err(format!("Slot {shown} is already occupied."));
        }
        r.kind = KIND_HUMAN;
        r.name = clean_name(name);
        r.ready = false;
        r.peer = Some(peer);
        self.bump();
        Ok(row as u8)
    }

    /// A machine leaves: its row becomes open. Returns the row. The host does not leave this way (in a loaded lobby it
    /// does not necessarily stand in row 0). In a loaded lobby the row becomes type 0 again without name and machine;
    /// nation, team, color and handicap of the state stay.
    pub fn leave(&mut self, peer: u8) -> Option<u8> {
        if peer == HOST_PEER {
            return None;
        }
        let row = self.row_of(peer)?;
        if self.state.is_loaded() {
            let r = &mut self.state.rows[usize::from(row)];
            r.kind = 0;
            r.name.clear();
            r.ready = false;
            r.peer = None;
        } else {
            self.open_row(row);
        }
        self.bump();
        Some(row)
    }

    /// Row open: type 0, Ready 0, handicap 0, team = row, no machine; the name stays.
    fn open_row(&mut self, row: u8) {
        let r = &mut self.state.rows[usize::from(row)];
        r.kind = 0;
        r.ready = false;
        r.handicap = 0;
        r.team = i32::from(row);
        r.peer = None;
    }

    /// Wish of a machine for its own row. Chat does not change the state. In a loaded lobby there are no wishes.
    pub fn wish(
        &mut self,
        peer: u8,
        wish: &LobbyWish,
        rules: &dyn LobbyRules,
    ) -> Result<(), String> {
        if self.state.is_loaded() {
            return Err(LOADED_FIXED.to_string());
        }
        let row = self
            .row_of(peer)
            .ok_or_else(|| "The machine has no row in the lobby.".to_string())?;
        match *wish {
            LobbyWish::Chat(_) => Ok(()),
            LobbyWish::Ready(ready) => {
                self.set_ready(row, ready);
                Ok(())
            }
            LobbyWish::Row {
                nation,
                team,
                handicap,
                color,
            } => self.set_row(row, nation, team, handicap, color, rules),
        }
    }

    fn set_ready(&mut self, row: u8, ready: bool) {
        if self.state.rows[usize::from(row)].ready != ready {
            self.state.rows[usize::from(row)].ready = ready;
            self.bump();
        }
    }

    /// Set nation, team, color, handicap of a row; `rules` checks team and color. Ready stays (E-302).
    fn set_row(
        &mut self,
        row: u8,
        nation: i32,
        team: i32,
        handicap: i32,
        color: i32,
        rules: &dyn LobbyRules,
    ) -> Result<(), String> {
        let n = self.state.rows.len() as i32;
        if !(-1..=99).contains(&nation)
            || !(0..n).contains(&team)
            || !(0..=MAX_HANDICAP).contains(&handicap)
            || !(0..=LAST_COLOR).contains(&color)
        {
            return Err("Invalid values for the row.".to_string());
        }
        let old = &self.state.rows[usize::from(row)];
        let mut proposed = old.clone();
        proposed.nation = nation;
        proposed.team = team;
        proposed.handicap = handicap;
        proposed.color = color;
        if proposed == *old {
            return Ok(());
        }
        let mut swap = None;
        if proposed.team != old.team || proposed.color != old.color {
            match rules.check(
                &self.state.rows,
                row,
                &proposed,
                self.state.session.settings.game_type,
            ) {
                RuleVerdict::Accept => {}
                RuleVerdict::Reject => {
                    return Err("Team or color is not allowed.".to_string());
                }
                RuleVerdict::Swap { row: other, team } => {
                    if usize::from(other) >= self.state.rows.len() {
                        return Err("Invalid swap.".to_string());
                    }
                    swap = Some((other, team));
                }
            }
        }
        self.state.rows[usize::from(row)] = proposed;
        if let Some((other, team)) = swap {
            self.state.rows[usize::from(other)].team = team;
        }
        self.bump();
        Ok(())
    }

    /// Change by the host (rights as in the original); not in a loaded lobby.
    pub fn edit(&mut self, e: &HostEdit, rules: &dyn LobbyRules) -> Result<EditOutcome, String> {
        if self.state.is_loaded() {
            return Err(LOADED_FIXED.to_string());
        }
        let (HostEdit::Kind { row, .. } | HostEdit::Row { row, .. } | HostEdit::Ready { row, .. }) =
            *e;
        let Some(current) = self.state.rows.get(usize::from(row)) else {
            return Err(format!("Row {row} does not exist."));
        };
        let current_kind = current.kind;
        let current_handicap = current.handicap;
        match *e {
            HostEdit::Kind { kind, .. } => {
                if row == 0 {
                    return Err("The host cannot change its row.".to_string());
                }
                if current_kind == KIND_HUMAN {
                    // Humans only to "Open" (= kick).
                    if kind != 0 {
                        return Err("A human can only be set to \"Open\".".to_string());
                    }
                    let peer = self.state.rows[usize::from(row)]
                        .peer
                        .ok_or_else(|| "The row has no machine.".to_string())?;
                    self.open_row(row);
                    self.bump();
                    return Ok(EditOutcome::Kicked { peer });
                }
                if !(0..=4).contains(&kind) {
                    return Err(format!("Invalid type {kind}."));
                }
                self.set_kind(row, kind);
                self.bump();
                Ok(EditOutcome::Changed)
            }
            HostEdit::Row {
                nation,
                team,
                handicap,
                color,
                ..
            } => {
                if row != 0 && current_kind == KIND_HUMAN {
                    return Err("A human's row can only be changed by that player.".to_string());
                }
                if (1..=3).contains(&current_kind) && handicap != current_handicap {
                    return Err("This computer's handicap is locked.".to_string());
                }
                self.set_row(row, nation, team, handicap, color, rules)?;
                Ok(EditOutcome::Changed)
            }
            HostEdit::Ready { ready, .. } => {
                if row != 0 && current_kind == KIND_HUMAN {
                    return Err("A human's checkmark can only be set by that player.".to_string());
                }
                self.set_ready(row, ready);
                Ok(EditOutcome::Changed)
            }
        }
    }

    /// Type choice like `NAMEn` (`0x004D8BF4`...`0x004D8DF0`): Open -> handicap 0, Ready 0, team = row; computer
    /// Easy/Normal/Hard/Manual -> handicap 5/2/0/0, Ready 1, plus `FUN_004BA980` (team of the first other computer, not
    /// with game type 4). Nation and color stay.
    fn set_kind(&mut self, row: u8, kind: i32) {
        let join_team =
            if (1..=4).contains(&kind) && self.state.session.settings.game_type != RAILROAD {
                self.state
                    .rows
                    .iter()
                    .enumerate()
                    .find(|&(i, r)| i != usize::from(row) && (1..=4).contains(&r.kind))
                    .map(|(_, r)| r.team)
            } else {
                None
            };
        let r = &mut self.state.rows[usize::from(row)];
        r.kind = kind;
        r.peer = None;
        if kind == 0 {
            r.handicap = 0;
            r.ready = false;
            r.team = i32::from(row);
        } else {
            r.handicap = match kind {
                1 => 5,
                2 => 2,
                _ => 0,
            };
            r.ready = true;
            if let Some(t) = join_team {
                r.team = t;
            }
        }
    }

    /// `START` condition: every row with type != 0 ready, at least two such rows, map name not empty. With a random map
    /// an empty map name suffices: the seed text is never empty, sanitized it can be (only spaces or brackets); the
    /// original starts with such a seed. In a loaded lobby: every row of type 999 with a machine, without Ready.
    pub fn can_start(&self) -> bool {
        if self.state.is_loaded() {
            return self
                .state
                .rows
                .iter()
                .all(|r| r.kind == KIND_HUMAN && r.peer.is_some());
        }
        let used = self.state.rows.iter().filter(|r| r.kind != 0);
        let session = &self.state.session;
        used.clone().count() >= 2
            && used.clone().all(|r| r.ready)
            && (!session.map_name.is_empty() || matches!(session.map, MapId::Random { .. }))
    }

    /// New session data (map change `LAN31 CHANGEGAME`, E-307 deviation). If a row that would be dropped is occupied
    /// ([`rows_fit`]), `Err` without change. Otherwise the rows `≥ players` are dropped or added as open rows (team =
    /// row, first free color); teams `≥ players` of the remaining rows are set to the row number; Ready flags stay
    /// (E-302). Not in a loaded lobby (no map change).
    pub fn set_session(&mut self, session: SessionInfo) -> Result<(), String> {
        if self.state.is_loaded() {
            return Err(LOADED_FIXED.to_string());
        }
        session.validate()?;
        if matches!(session.map, MapId::Saved { .. }) {
            return Err(SAVED_NEEDS_LOADED.to_string());
        }
        let session = session.cleaned();
        let want = usize::from(session.players);
        if !rows_fit(&self.state.rows, session.players) {
            return Err("Rows that would be dropped are still occupied.".to_string());
        }
        if session == self.state.session && want == self.state.rows.len() {
            return Ok(());
        }
        self.state.rows.truncate(want);
        while self.state.rows.len() < want {
            let i = self.state.rows.len();
            let color = (0..=LAST_COLOR)
                .find(|&c| self.state.rows.iter().all(|r| r.color != c))
                .unwrap_or(0);
            self.state.rows.push(LobbyRow {
                kind: 0,
                name: String::new(),
                nation: 0,
                team: i as i32,
                handicap: 0,
                color,
                ready: false,
                peer: None,
            });
        }
        for (i, r) in self.state.rows.iter_mut().enumerate() {
            if r.team >= want as i32 {
                r.team = i as i32;
            }
        }
        self.state.session = session;
        self.bump();
        Ok(())
    }

    /// Set the largest latency; increments `revision` only when the value changes.
    pub fn set_rtt(&mut self, max: Option<u32>) {
        if self.state.max_rtt_ms != max {
            self.state.max_rtt_ms = max;
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests;
