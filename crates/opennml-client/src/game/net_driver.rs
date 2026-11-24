//! Turn driver: connects the turn core from `opennml-net` ([`HostCore`]/[`ClientCore`], sans-IO) with the connections
//! ([`Link`]) and the game host ([`SimHost`] in turn mode, [`SimHost::enter_rounds`]).
//!
//! Per frame ([`NetDriver::advance`]):
//! 1. [`NetDriver::pump`]: read connections -> core, silence per connection to the core (`age_ms - last_received_ms`
//!    of the same connection), send the core's outbox. After a sent `Reject`/`Bye`, after `CoreEvent::Lost` and after
//!    `CoreEvent::Failed` the driver closes the affected connection.
//! 2. The driver closes turn 0 empty right after `Go` (`close_round(0, hash_at(0))`), before an own command reaches the
//!    core.
//! 3. Game time as in single player via [`SimHost::advance_rounds`]; before every tick `s`:
//!    - Turn tick `b(k)`: own commands ([`SimHost::take_outgoing`]) to the core, then
//!      `close_round(k, hash_at(b(k)))` (checksum of the state before `b(k)`);
//!    - Execution tick `e(k)`: without bundle `k` wait (the remaining game time is forfeited), otherwise compute the
//!      tick with the commands of the bundle ([`SimHost::step_with`], accounted with `step = e(k)`, E-290).
//!
//!    After every tick the host reports the results of the slots to the core ([`HostCore::set_results`]). After `e(k)`
//!    the control entries of the bundle go out in a fixed order (speed, save, stop/end, resync, halt); the driver sets
//!    the game speed itself (`SimHost::game_speed`, checked <= [`MAX_GAME_SPEED`]). If the bundle carried a halt,
//!    resync or stop and the core is then in `Halted`/`Stopped`, the driver computes `e(k)+1` only after the lift
//!    (`Running`), after a stop never again. For this it checks the state of the core, not the event: if the `Resume`
//!    came before `e(k)`, it does not halt (only time, `Phase::Halted`).
//!
//! In the original `NetSession_HostAdvanceTurn` (`0x004E9FB0`) drives the turns from `GameEngine_Update`
//! (`0x00445650`); commands take effect there only every fourth step (E-255), the host itself bypasses the lock (E-256)
//! and the order follows arrival (E-257). In the reimplementation the host submits its contribution like every client,
//! and the bundle orders by slot.
//!
//! State resync: if an executed bundle carries `Resync { slots }`, every machine stands still after `e(k)` (halt as
//! above). The host then builds - only now, in the same tick as the clients - its net state ([`SimHost::net_state`],
//! [`NetState`]: savegame without commands plus the whole command log) and gives it to the core
//! ([`HostCore::send_state`]), which sends it in pieces to the targets. A target keeps the received state
//! (`CoreEvent::ResyncState`, even if it arrives before its own `e(k)`) until it itself stands after `e(k)`, then
//! writes the desync report ([`report::write_desync`] under [`NetPaths::netlog`]; a write error does not stop the
//! resync), adopts the state ([`SimHost::adopt`]) and reports [`ClientCore::synced`]. It continues with the `Resume` of
//! the host; if the resync ends in a pause from the same bundle, the machine then stands in the pause (without
//! `Resumed`). On the third desync the bundle carries `Stop` instead of `Resync`: everyone stands after `e(k)` forever
//! (the network savegame is written from [`NetFrame::controls`]). Each frame the host sets the local time of the core
//! ([`HostCore::set_clock`]) for the name of the stop.
//!
//! Session and flow: The network logic of pause, game speed and saving lives here, without GPU; `GameSession` and the
//! fake of the tests call it the same way.
//! - Pause ([`NetDriver::toggle_pause`], `GameEngine_TogglePauseNet` `0x0042D780`): a `Pause` wish, or the lift if the
//!   running pause is from the own slot (O-316); nothing for a foreign pause or during a resync. The focus loss
//!   ([`NetDriver::focus_lost`], O-154) only wishes. The display ([`NetDriver::pause_shown`]) follows the core: only in
//!   the state after `e(k)` with `Halted(Pause)`, i.e. on all machines at the same time, not during a resync.
//! - Game speed ([`NetDriver::change_game_speed`], `FUN_0042F050`): only the host wishes `Speed(v)` with `v` from the
//!   running game speed (two presses in one turn give +1 once, E-294); the game speed is set only by the control entry
//!   after `e(k)`, reported in [`NetFrame::speed`] for message 9/10 on all machines ([`SpeedChange::message`],
//!   `GameEngine_CmdSetGameSpeed` `0x0042EF30`).
//! - Saving ([`NetDriver::wish_save`], E-274): a wish with a checked name. After the execution tick of the bundle with
//!   `Save` (likewise `Stop`/`End`) the driver captures the simulation part ([`NetFrame::saves`]); the flow adds the
//!   host part and writes (`GameFlow`, [`NetPaths::net_save`], network part [`NetDriver::save_info`]).
//! - Resync: if the driver adopts a state, [`NetFrame::adopted`] reports it; the events of the frame belong to the
//!   discarded state and are dropped, the session rebuilds the presentation as after loading.
//!
//! End, abort and leaving: How the network match ends for this machine is reported by [`NetFrame::finish`] exactly
//! once ([`NetFinish`]); after that the driver stands forever, the flow shows the message ([`NetFinish::message`]) and
//! after "OK" goes via the accounting to the main menu (before `Go` directly).
//! - Stop after the third desync: [`NetFinish::Stopped`] after the execution tick of the bundle with `Stop`.
//! - Orderly end ([`NetDriver::end`], only the host, [`HostCore::end`]; also when the host leaves the running match via
//!   `0x0E`): The bundle with `End` brings every core into `Phase::Ended`; the driver still computes until its
//!   execution tick, captures the network savegame there as for `Save` ([`NetFrame::saves`]) and reports
//!   [`NetFinish::HostEnded`]. All simulation parts are equal.
//! - Abort of the host (client: `Failed` after `Go`, e.g. [`opennml_net::HOST_LOST`] after `Closed`, 30 s of silence or a
//!   protocol error): The driver immediately computes with the bundles it has, up to before the execution tick of the
//!   first missing bundle (at most `T + V` ticks, without game time; pause and resync do not halt in this, a state
//!   after a pause is lifted), captures the network savegame `<prefix>YYYY_MM_DD_hhmm` (`net_` or `netz_` by
//!   edition) there by its own local time and
//!   reports [`NetFinish::HostLost`]: clients with the same last bundle have the same state, even if one had already
//!   computed further at the abort.
//! - Handover by the host ([`NetDriver::hand_over`]): The old host computes until the execution tick of the bundle with
//!   `Handover`, writes nothing and reports [`NetFinish::HandedOver`] (without a message into the accounting); the
//!   clients continue computing, one of them becomes host. If nobody can take over when the bundle is formed, it
//!   carries `End`, and the orderly end applies.
//! - Otherwise failed (resync, error of the host after `Go`): [`NetFinish::Failed`] without savegame.
//! - A machine could not load the match (`LoadFailed` from the host, also for the host itself; deviation from E-352):
//!   before `Go` [`NetFinish::LoadFailed`] with the player name, message 99 113 on every machine in the match.
//! - Leaving ([`NetDriver::leave`]): `Bye` to the peer(s), connections closed, no end event; the host treats the `Bye`
//!   of a client like a dropout, a leaving host is an abort for the clients.
//!
//! Lobby: The driver collects the events of the lobby (`LobbyChanged`, `Chat`, `Kicked`, `HostLeft`, `Rejected`) for
//! the menu host, which fetches them before the start with [`NetDriver::take_lobby_events`] (`NetFrame` stays for the
//! match). On `Rejected`, `Kicked` and `HostLeft` it sets [`NetDriver::failure`] with the reason text and closes the
//! connection, as on `Failed`.

mod migration;
mod net_state;
pub mod report;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;

use opennml_net::{
    ClientCore, Connecting, CoreEvent, HOST_LEFT, HOST_LOST, HOST_PEER, HaltReason, HostCore,
    KICKED, Link, LinkEvent, Listener, LocalTime, LocalWords, Lockstep, MAX_GAME_SPEED, NetControl,
    NetMessage, Peer, Phase, REJOIN_WINDOW_MS, Rejoin, SUCCESSOR_CONNECT_MS, Seat, StartInfo,
    Successor, Transport, TurnBundle, Wish, seat_of, valid_save_name,
};
use opennml_sim::command::Command;
use opennml_sim::save::{RecordedCommand, SaveGame};
use opennml_sim::state::SimEvent;
use opennml_ui::assets::Texts;
use opennml_ui::own_texts::{
    LOAD_FAILED, NET_FAILED, NET_HOST_ENDED_SAVED, NET_HOST_ENDED_UNSAVED, NET_HOST_LOST,
    NET_HOST_LOST_SAVED, NET_HOST_LOST_UNSAVED, NET_STOPPED_SAVED, NET_STOPPED_UNSAVED, fill,
};

use crate::game::pause::PauseState;
use crate::game::sim_host::{RoundStep, Rounds, SimHost};
use crate::game_save::NetSaveInfo;

use migration::MigrationRun;
pub use net_state::{NET_STATE_FORMAT, NET_STATE_MAGIC, NetState};

#[cfg(test)]
mod tests;

/// This many most recently executed bundles are kept by the driver for the desync report.
const REPORT_BUNDLES: usize = 3;

/// Storage locations of the driver: the savegame folder `savegame/` (network savegames in it under `multiplayer/`,
/// [`Self::net_save`]) and desync reports (`netlog/`) next to the settings file, in tests a folder of the
/// caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPaths {
    pub saves: PathBuf,
    pub netlog: PathBuf,
}

impl NetPaths {
    /// Network savegame `name`: `saves/multiplayer/<name>` without extension (E-274). The caller checks the name
    /// beforehand with [`valid_save_name`].
    pub fn net_save(&self, name: &str) -> PathBuf {
        self.saves.join("multiplayer").join(name)
    }
}

/// Game speed change by a `Speed` control entry after the execution tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeedChange {
    /// Tick after the change (`e(k) + 1`).
    pub step: u64,
    pub old: u8,
    pub new: u8,
}

impl SpeedChange {
    /// Message of the change like `GameEngine_CmdSetGameSpeed` (`0x0042EF30`): 10 when lowering, 9 when raising, none
    /// without a change (`MessageBar_ShowMessage(id, {-1, -1}, 0x3010000)` only for `id != 0`).
    pub fn message(&self) -> Option<i32> {
        use crate::game::hud::{SPEED_LOWERED_MESSAGE, SPEED_RAISED_MESSAGE};
        match self.new.cmp(&self.old) {
            std::cmp::Ordering::Less => Some(SPEED_LOWERED_MESSAGE),
            std::cmp::Ordering::Greater => Some(SPEED_RAISED_MESSAGE),
            std::cmp::Ordering::Equal => None,
        }
    }
}

/// Result of a frame ([`NetDriver::advance`]).
#[derive(Debug, Default)]
pub struct NetFrame {
    /// Events of the computed ticks (like [`SimHost::advance`]); empty if the frame adopted a state.
    pub events: Vec<SimEvent>,
    /// Control entries of the bundles executed in this frame, per bundle in the fixed order.
    pub controls: Vec<NetControl>,
    /// Slots that dropped out, with reason (host; the defeat arrives as a command in the next bundle).
    pub lost: Vec<(u8, String)>,
    /// The session has failed (error message); afterwards the driver no longer computes.
    pub failed: Option<String>,
    /// Game speed changes of the executed `Speed` entries (message 9/10, [`SpeedChange::message`]).
    pub speed: Vec<SpeedChange>,
    /// The machine adopted the state of the host in the resync ([`SimHost::adopt`]): the session rebuilds the
    /// presentation as after loading.
    pub adopted: bool,
    /// Network savegames of the executed `Save`, `Stop` and `End` entries, captured right after their execution tick,
    /// and the state at the abort of the host: the flow only adds the host part and writes.
    pub saves: Vec<NetSave>,
    /// The network match is over for this machine (once per match); its savegame is in [`Self::saves`] beforehand.
    pub finish: Option<NetFinish>,
    /// This client has become host after a host migration: the driver set [`SimHost::set_net_host`] in the same
    /// frame; the flow updates the network role of the UI.
    pub became_host: bool,
}

/// How the network match ended for this machine ([`NetFrame::finish`]); the flow shows [`Self::message`] as a
/// message with a button and then goes via the accounting to the main menu (before `Go` directly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetFinish {
    /// Third desync: stopped, network savegame `name` (`checksumerror_…`).
    Stopped { name: String },
    /// The host has ended the match (orderly): network savegame `name` (`<prefix>…`), the same on all machines.
    HostEnded { name: String },
    /// The connection to the host broke off (abort): network savegame `name` (`<prefix>…`) after the last tick the
    /// machine could compute.
    HostLost { name: String },
    /// Otherwise failed (error message), without savegame.
    Failed(String),
    /// The machine of the player `name` could not load the match (`LoadFailed` of the host, deviation from E-352):
    /// message 99 113 with the name, without savegame; the reason stays in the log.
    LoadFailed { name: String },
    /// The host handed the match over in an orderly way: it computed until the execution tick of the handover bundle,
    /// writes nothing and goes into the accounting without a message; a client plays on as host.
    HandedOver,
}

impl NetFinish {
    /// Name of the network savegame for it.
    pub fn save_name(&self) -> Option<&str> {
        match self {
            NetFinish::Stopped { name }
            | NetFinish::HostEnded { name }
            | NetFinish::HostLost { name } => Some(name),
            NetFinish::Failed(_) | NetFinish::LoadFailed { .. } | NetFinish::HandedOver => None,
        }
    }

    /// Message for the player; `saved`: the network savegame is written. The texts are the own texts 99 116 to 99 123
    /// of the edition of `texts` (`NET_*` in [`opennml_ui::own_texts`]); the abort with the cause [`HOST_LOST`] gets
    /// the sentence without a frame (99 122), the one for a failed load is 99 113 ([`load_failed_message`]).
    ///
    /// Only characters up to U+00FF (E-296): The `MSGBOX` sets the text via the path of the original, which takes only
    /// the low byte of each UTF-16 unit as atlas code (O-61); typographic quotation marks would become boxes. The
    /// name is therefore enclosed in ASCII `"`, and foreign text in [`NetFinish::Failed`] (error chains of other
    /// parts) goes through [`latin1_text`].
    pub fn message(&self, saved: bool, texts: &Texts) -> String {
        let own = |id: i32, args: &[&str]| fill(&texts.display(id), args);
        match (self, saved) {
            (NetFinish::Stopped { name }, true) => own(NET_STOPPED_SAVED, &[name]),
            (NetFinish::Stopped { .. }, false) => own(NET_STOPPED_UNSAVED, &[]),
            (NetFinish::HostEnded { name }, true) => own(NET_HOST_ENDED_SAVED, &[name]),
            (NetFinish::HostEnded { .. }, false) => own(NET_HOST_ENDED_UNSAVED, &[]),
            (NetFinish::HostLost { name }, true) => own(NET_HOST_LOST_SAVED, &[name]),
            (NetFinish::HostLost { .. }, false) => own(NET_HOST_LOST_UNSAVED, &[]),
            (NetFinish::Failed(text), _) if text == HOST_LOST => own(NET_HOST_LOST, &[]),
            (NetFinish::Failed(text), _) => own(NET_FAILED, &[&latin1_text(text)]),
            (NetFinish::LoadFailed { name }, _) => load_failed_message(name, texts),
            // Without a message; the text is only for the log and for a caller that shows one anyway.
            (NetFinish::HandedOver, _) => "The match was handed over to another player.".into(),
        }
    }
}

/// Message 99 113 ("<name> could not load the game. The start was cancelled.") of the edition of `texts`, the name
/// converted for the `MSGBOX` ([`latin1_text`]): on every machine of a start that failed because one machine could
/// not load it (deviation from E-352).
pub fn load_failed_message(name: &str, texts: &Texts) -> String {
    fill(&texts.display(LOAD_FAILED), &[&latin1_text(name)])
}

/// Converts a text for the text path of the original (O-61: atlas code = low byte of the UTF-16 unit): characters up
/// to U+00FF stay (umlauts, sharp s), typographic quotation marks become ASCII `"` or `'`, dashes `-`, `…` becomes
/// `...`, `≠` becomes `!=`, everything else `?` (E-296).
pub fn latin1_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{0}'..='\u{FF}' => out.push(c),
            '„' | '“' | '”' | '‟' => out.push('"'),
            '‚' | '‘' | '’' | '‛' => out.push('\''),
            '–' | '—' | '‐' | '‑' | '−' => out.push('-'),
            '…' => out.push_str("..."),
            '≠' => out.push_str("!="),
            _ => out.push('?'),
        }
    }
    out
}

/// Simulation part of a network savegame, captured after the execution tick `e(k)` of the bundle with `Save`, `Stop`
/// or `End`, i.e. at `steps == e(k) + 1` the same on every machine, even if a machine continues computing in the same
/// frame (up to two ticks per frame). The flow adds the host part (`GameFlow`).
#[derive(Debug, Clone, PartialEq)]
pub struct NetSave {
    /// Name from the control entry (checked by the host, [`valid_save_name`] once more before writing).
    pub name: String,
    /// Savegame of the simulation ([`SimHost::save`]) at `e(k) + 1`.
    pub sim: SaveGame,
    /// Game speed at `e(k) + 1` (after a `Speed` of the same bundle) for the host part.
    pub game_speed: u8,
    /// Command log from the start of the match at `e(k) + 1` ([`SimHost::save_log`]), taken at the same moment as
    /// `sim`; every machine thereby writes the same log into its file.
    pub log: Option<Vec<RecordedCommand>>,
}

/// What the session itself needs from a frame of the driver ([`NetFrame::into_session`]).
#[derive(Debug, Default)]
pub struct SessionStep {
    pub events: Vec<SimEvent>,
    pub speed: Vec<SpeedChange>,
    pub adopted: bool,
}

impl NetFrame {
    /// Splits a frame for the session: events, game speed changes and adoption are handled by the session itself
    /// (presentation, message bar); control entries, network savegames, dropouts and failure it collects in `pending`
    /// for the flow (`SessionView::take_net_frame`).
    pub fn into_session(self, pending: &mut NetFrame) -> SessionStep {
        let NetFrame {
            events,
            controls,
            lost,
            failed,
            speed,
            adopted,
            saves,
            finish,
            became_host,
        } = self;
        pending.controls.extend(controls);
        pending.became_host |= became_host;
        pending.saves.extend(saves);
        pending.lost.extend(lost);
        if pending.failed.is_none() {
            pending.failed = failed;
        }
        if pending.finish.is_none() {
            pending.finish = finish;
        }
        SessionStep {
            events,
            speed,
            adopted,
        }
    }
}

/// Role of the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRole {
    Host,
    Client,
}

/// Why the machine is currently not computing (debug status line like `FUN_004EA5D0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    /// Before `Go`: sign-on or loading.
    Starting,
    /// Before the execution tick of turn `turn`, whose bundle is missing.
    Bundle { turn: u32 },
    /// Stands after an execution tick (pause or resync).
    Halted(HaltReason),
    /// Stands forever after a stop.
    Stopped,
    /// The session is over.
    Ended,
}

/// State of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStatus {
    /// Connection number (for the client the host, [`HOST_PEER`]).
    pub peer: Peer,
    /// Slot of the peer after the start.
    pub slot: Option<u8>,
    /// Time since the last reception in ms (also heartbeat).
    pub silent_ms: u64,
}

/// Network status for the control channel and the log (`net_status`).
#[derive(Debug, Clone, PartialEq)]
pub struct NetStatus {
    pub role: NetRole,
    /// Phase of the core (waiting for clients, loading, running, halted, ended).
    pub phase: Phase,
    /// Own slot from the start.
    pub own_slot: Option<u8>,
    /// Slot assignment from the start (slot -> machine).
    pub seats: Option<[Seat; 8]>,
    /// Turn whose bundle is executed next.
    pub round: u32,
    /// Execution tick of this turn.
    pub next_exec_step: u64,
    /// Reason for waiting; `None` while the machine is computing.
    pub waiting: Option<WaitReason>,
    /// Host: slots whose contribution to the turn is missing, which the host bundles next ([`HostCore::missing`]);
    /// client: empty (it does not know the contributions of the others).
    pub missing: Vec<u8>,
    /// Host: number of desyncs of the match; client: `None`.
    pub desyncs: Option<u32>,
    /// Per open connection the time since the last reception; closed ones (dropout, failure, leaving) are missing.
    pub links: Vec<LinkStatus>,
    /// Successor list: host from its core, client from `Go` or the last `Migrated`.
    pub successors: Vec<Successor>,
    /// Running host migration of this machine, otherwise `None`.
    pub migration: Option<MigrationStatus>,
}

/// State of a running host migration ([`NetStatus::migration`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    /// Client: connects to the candidate `peer`.
    Dialing { peer: Peer },
    /// Client: has sent its `Rejoin` to the candidate `peer` and waits for `Migrated`.
    Rejoining { peer: Peer },
    /// New host: gathers the `Rejoin`s; `expected` are the expected clients, `arrived` those already accepted.
    Gathering {
        expected: Vec<Peer>,
        arrived: Vec<Peer>,
    },
}

/// The core by role (boxed: the cores are large and of different sizes).
enum Core {
    Host(Box<HostCore>),
    Client(Box<ClientCore>),
}

/// An executed bundle until its `after_step`.
struct Executing {
    turn: u32,
    controls: Vec<NetControl>,
    /// The bundle itself (for the last bundles of the report).
    bundle: TurnBundle,
}

/// Connection that the listener of a client accepted and that does not yet belong to a peer.
struct Accepted {
    link: Box<dyn Link>,
    /// Program time of the acceptance or of the `Rejoin` ([`NetDriver::pump`]); after [`REJOIN_WINDOW_MS`] the driver
    /// closes it.
    since_ms: u64,
    /// A `Rejoin` that the driver holds as long as it is not host itself.
    rejoin: Option<Box<Rejoin>>,
}

/// Probe of the host: connection attempt to the listening port of the client `peer`. Without `connecting` (no address,
/// port or transport) the result "unreachable" is already fixed.
struct Probe {
    peer: Peer,
    addr: Option<SocketAddr>,
    connecting: Option<Box<dyn Connecting>>,
}

/// Turn driver of a machine.
pub struct NetDriver {
    core: Core,
    /// Second transport of the machine for probe and migration ([`Self::set_transport`]).
    transport: Option<Box<dyn Transport>>,
    /// Client: listener that the session opened before the `Hello` ([`Self::set_listener`]).
    listener: Option<Box<dyn Listener>>,
    /// Client: accepted connections that still wait for a message.
    accepted: Vec<Accepted>,
    /// Host: running probes.
    probes: Vec<Probe>,
    /// Number of connections of the listener accepted since creation (tests).
    #[cfg(test)]
    accepted_total: u32,
    links: Vec<(Peer, Box<dyn Link>)>,
    /// Connections that the driver has closed (dropout, `Reject`/`Bye`, failure, leaving). They stay in
    /// [`Self::links`] so that queued messages still go out, but no longer count toward [`NetStatus::links`]. The
    /// host assigns numbers consecutively, a closed one does not return.
    closed: Vec<Peer>,
    paths: NetPaths,
    /// Start information of the own machine (`CoreEvent::Start`).
    start: Option<StartInfo>,
    /// Next turn to close.
    next_close: u32,
    /// Next turn to execute.
    next_exec: u32,
    /// Origin tick: turn 0 begins here, `b(k) = origin + k*T`, `e(k) = b(k) + V`; 0 for a new match, `state.steps` of
    /// the savegame for a loaded one ([`Self::set_origin`]).
    origin: u64,
    /// Stands after the execution tick of a bundle with halt, resync or stop.
    stand: bool,
    /// Waits before the execution tick of this turn for its bundle.
    blocked: Option<u32>,
    executing: Option<Executing>,
    /// Collected since the last [`NetDriver::advance`] (also from [`NetDriver::pump`] alone).
    frame: NetFrame,
    /// The most recently executed bundles (at most [`REPORT_BUNDLES`], oldest first) for the desync report.
    recent: VecDeque<TurnBundle>,
    /// Client: state of the host for the resync (`CoreEvent::ResyncState`) until the machine stands after `e(k)`.
    resync_bytes: Option<Vec<u8>>,
    /// Client: the own slot is a target of the resync from the bundle of this turn; the state is adopted after `e(k)`.
    adopt_due: Option<u32>,
    /// Number of desync reports written (number `n` in the folder name).
    reports: u32,
    /// Fixed local time instead of the clock ([`Self::set_fixed_clock`]).
    fixed_clock: Option<LocalTime>,
    /// The resync failed: the driver no longer computes (connections closed, `NetFrame::failed`).
    gave_up: bool,
    /// `Go` came: the match was running (a failure afterwards ends it with a message).
    in_play: bool,
    /// The network match is over ([`NetFrame::finish`] reported or left): the driver never computes again.
    finished: bool,
    /// Client: the host dropped out after `Go`; the next [`Self::advance`] computes up to the waiting point and
    /// captures the network savegame ([`Self::settle_abort`]).
    abort_due: bool,
    /// [`Self::leave`] ran.
    left: bool,
    /// Host: [`Self::end`] is accepted, the end bundle is still to come.
    ending: bool,
    /// [`Self::settle_abort`] is recomputing: only `Stop`/`End` halt.
    settling: bool,
    /// Reason of the first failure of the session (`CoreEvent::Failed`), stays after the frame is fetched
    /// ([`Self::failure`]: start abort and `net_status`).
    failure: Option<String>,
    /// Name of the player whose machine could not load the match, if that was the first failure
    /// (`CoreEvent::LoadFailed`): the end before `Go` is then [`NetFinish::LoadFailed`].
    load_failed: Option<String>,
    /// Events of the lobby since the last [`Self::take_lobby_events`].
    lobby_events: Vec<CoreEvent>,
    /// Client: running election of the new host ([`MigrationRun`]); for the new host [`HostCore::migrating`]
    /// applies.
    migration: Option<MigrationRun>,
    /// Reason for the dropout of the old host (`CoreEvent::HostGone`), for the message about its slot.
    gone_reason: Option<String>,
    /// Debug aid `net_debug mute` ([`Self::set_muted`]): every connection stays silent, also ones added later.
    muted: bool,
    /// Host: [`Self::hand_over`] is accepted, the handover bundle is still to come.
    handing_over: bool,
    /// Program end at the host ([`Self::close_program`]): until this time (ms of the caller's clock) the program waits
    /// for the handover or end bundle.
    close_deadline: Option<u64>,
    /// Program time of the last [`Self::pump`] (deadlines of the migration from events without their own time).
    now_ms: u64,
    /// Slots with a result in the own state after the last tick ([`SimHost::results`]) for
    /// [`HostCore::from_client`].
    results: [bool; 8],
}

/// Reason in the `Bye` of the machine that leaves the match: main menu, program end, window closed.
pub const LEAVE_REASON: &str = "The player has left the match.";

/// Deadline for the `Hello` of an accepted connection: if it sends none by then, the host closes it itself
/// (otherwise a silent peer would hold a thread and a connection slot until the timeout of the core, which does not
/// know it before the `Hello` at all).
pub const HELLO_TIMEOUT_MS: u64 = 10_000;

/// Reason when closing a connection without `Hello`.
const NO_HELLO: &str = "No sign-on (Hello) within 10 s";

/// Reason when the debug aid `net_debug drop` cuts the connections.
const DROPPED: &str = "Check run: connection cut without Bye";

/// Order of the control entries after the execution tick: speed, save, stop/end, resync, halt.
fn control_rank(c: &NetControl) -> u8 {
    match c {
        NetControl::Speed(_) => 0,
        NetControl::Save { .. } => 1,
        NetControl::Stop { .. } | NetControl::End { .. } | NetControl::Handover { .. } => 2,
        NetControl::Resync { .. } => 3,
        NetControl::Halt { .. } => 4,
    }
}

impl NetDriver {
    /// Driver of the host; the connections of the clients come with [`Self::add_link`].
    pub fn host(core: HostCore, paths: NetPaths) -> Self {
        Self::new(Core::Host(Box::new(core)), Vec::new(), paths)
    }

    /// Driver of a client with its connection to the host.
    pub fn client(core: ClientCore, link: Box<dyn Link>, paths: NetPaths) -> Self {
        Self::new(Core::Client(Box::new(core)), vec![(HOST_PEER, link)], paths)
    }

    fn new(core: Core, links: Vec<(Peer, Box<dyn Link>)>, paths: NetPaths) -> Self {
        Self {
            core,
            transport: None,
            listener: None,
            accepted: Vec::new(),
            probes: Vec::new(),
            #[cfg(test)]
            accepted_total: 0,
            links,
            closed: Vec::new(),
            paths,
            start: None,
            next_close: 0,
            next_exec: 0,
            origin: 0,
            stand: false,
            blocked: None,
            executing: None,
            frame: NetFrame::default(),
            recent: VecDeque::new(),
            resync_bytes: None,
            adopt_due: None,
            reports: 0,
            fixed_clock: None,
            gave_up: false,
            in_play: false,
            finished: false,
            abort_due: false,
            left: false,
            ending: false,
            settling: false,
            failure: None,
            load_failed: None,
            lobby_events: Vec::new(),
            migration: None,
            gone_reason: None,
            muted: false,
            handing_over: false,
            close_deadline: None,
            now_ms: 0,
            results: [false; 8],
        }
    }

    /// Fixed local time (tests, check runs) instead of the clock of the machine; `None` = clock. At the host it names
    /// the network savegame of the stop ([`HostCore::set_clock`]) and at every machine the desync report.
    pub fn set_fixed_clock(&mut self, clock: Option<LocalTime>) {
        self.fixed_clock = clock;
    }

    /// Second transport of this machine (`Transport::fork`): the host probes the listening ports of its clients with
    /// it, later the client connects with it to the new host. Without a transport every client counts as
    /// unreachable.
    pub fn set_transport(&mut self, transport: Box<dyn Transport>) {
        self.transport = Some(transport);
    }

    /// Client: the listener whose port is in the `Hello`. Accepted connections and their `Rejoin` are handled by
    /// [`Self::pump_accepted`].
    pub fn set_listener(&mut self, listener: Box<dyn Listener>) {
        self.listener = Some(listener);
    }

    /// The words of the local installation that the core carries (prefix of the network savegame names).
    fn words(&self) -> &LocalWords {
        match &self.core {
            Core::Host(h) => h.words(),
            Core::Client(c) => c.words(),
        }
    }

    /// Local time of this machine ([`Self::set_fixed_clock`] or [`report::local_now`]).
    fn local_time(&self) -> LocalTime {
        self.fixed_clock.unwrap_or_else(report::local_now)
    }

    /// Host: connection to the client with the number `peer` (1 to 7, assigned by the caller; `Welcome` tells it to
    /// it).
    pub fn add_link(&mut self, peer: Peer, link: Box<dyn Link>) {
        if !matches!(self.core, Core::Host(_)) || peer == HOST_PEER {
            log::error!("Network: connection {peer} discarded, only the host accepts clients");
            return;
        }
        if self.links.iter().any(|(p, _)| *p == peer) {
            log::error!("Network: connection {peer} already exists");
            return;
        }
        let mut link = link;
        if self.muted {
            link.mute(true);
        }
        self.links.push((peer, link));
    }

    /// Core of the host (start, clock, resync).
    pub fn host_core(&self) -> Option<&HostCore> {
        match &self.core {
            Core::Host(h) => Some(h.as_ref()),
            Core::Client(_) => None,
        }
    }

    pub fn host_core_mut(&mut self) -> Option<&mut HostCore> {
        match &mut self.core {
            Core::Host(h) => Some(h.as_mut()),
            Core::Client(_) => None,
        }
    }

    /// Core of the client.
    pub fn client_core(&self) -> Option<&ClientCore> {
        match &self.core {
            Core::Client(c) => Some(c.as_ref()),
            Core::Host(_) => None,
        }
    }

    pub fn client_core_mut(&mut self) -> Option<&mut ClientCore> {
        match &mut self.core {
            Core::Client(c) => Some(c.as_mut()),
            Core::Host(_) => None,
        }
    }

    /// Storage locations.
    pub fn paths(&self) -> &NetPaths {
        &self.paths
    }

    /// Start information of the own machine, as soon as the core has reported the start.
    pub fn start_info(&self) -> Option<&StartInfo> {
        self.start.as_ref()
    }

    /// Loading finished (`None`) or failed. Host: `loaded_self` or abort of the session for everyone
    /// ([`HostCore::load_failed`]: `LoadFailed` and `Bye` to every client, connections closed); client: `Loaded` to the
    /// host.
    pub fn loaded(&mut self, error: Option<String>) {
        match (&mut self.core, error) {
            (Core::Host(h), None) => h.loaded_self(),
            (Core::Host(h), Some(e)) => {
                // Abort for everyone; `flush` sends `LoadFailed` and `Bye`, closes the connections and reports
                // `failed`.
                log::error!("Network: the host could not load the match: {e}");
                h.load_failed(e);
                self.flush();
            }
            (Core::Client(c), error) => c.loaded(error),
        }
    }

    fn lockstep(&mut self) -> &mut dyn Lockstep {
        match &mut self.core {
            Core::Host(h) => h.as_mut(),
            Core::Client(c) => c.as_mut(),
        }
    }

    /// Raster of the match (`T`/`V`; before the start the configured one).
    pub fn raster(&self) -> opennml_net::TurnConfig {
        self.lockstep_ref().config()
    }

    /// Set the origin tick: turn 0 begins at the tick `step` of the loaded state, `b(k) = step + k*T`,
    /// `e(k) = b(k) + V`. Ticks, command log and network savegames keep counting absolutely; the origin only shifts
    /// the raster. 0 without a call (new match). To be called before `Go` (`GameFlow::start_saved_net_match`).
    pub fn set_origin(&mut self, step: u64) {
        self.origin = step;
    }

    /// Origin tick of the raster ([`Self::set_origin`]).
    pub fn origin(&self) -> u64 {
        self.origin
    }

    /// Turn that begins at the (absolute) tick `step`: `b(k) = origin + k*T`.
    fn round_at(&self, step: u64) -> Option<u32> {
        let cfg = self.lockstep_ref().config();
        cfg.round_at(step.checked_sub(self.origin)?)
    }

    /// Turn whose commands are executed at the (absolute) tick `step`: `e(k) = origin + k*T + V`.
    fn exec_at(&self, step: u64) -> Option<u32> {
        let cfg = self.lockstep_ref().config();
        cfg.exec_at(step.checked_sub(self.origin)?)
    }

    /// Absolute execution tick of turn `k`.
    fn exec_step(&self, k: u32) -> u64 {
        // Wrapped like `state.steps`: the origin comes from a loaded state.
        self.origin
            .wrapping_add(self.lockstep_ref().config().exec_step(k))
    }

    /// Closing deadline of the host at program end in milliseconds for the game speed `game_speed`.
    pub fn close_ms(&self, game_speed: u8) -> u64 {
        self.raster().close_ms(game_speed)
    }

    fn lockstep_ref(&self) -> &dyn Lockstep {
        match &self.core {
            Core::Host(h) => h.as_ref(),
            Core::Client(c) => c.as_ref(),
        }
    }

    /// Stands after an execution tick (halt, resync, stop) and does not compute further.
    pub fn halted(&self) -> bool {
        self.stand
    }

    /// This machine is the host.
    pub fn is_host(&self) -> bool {
        matches!(self.core, Core::Host(_))
    }

    /// Own slot from the start.
    pub fn own_slot(&self) -> Option<u8> {
        self.start.as_ref().map(|s| s.your_slot)
    }

    /// The network pause is visible (text 15788): the machine stands after `e(k)` and the core halts because of a
    /// pause (also after a resync that ends in a pause). Not before `e(k)`, during a resync and after the lift.
    pub fn pause_shown(&self) -> bool {
        self.stand
            && matches!(
                self.lockstep_ref().phase(),
                Phase::Halted(HaltReason::Pause { .. })
            )
    }

    /// The two pause bits of the session on the network: both equal [`Self::pause_shown`] (the network echo that in the
    /// original sets only the visible bit is dropped: all machines stand, O-316).
    pub fn pause_state(&self) -> PauseState {
        let p = self.pause_shown();
        PauseState { sim: p, shown: p }
    }

    /// Pause key on the network (`GameEngine_TogglePauseNet` `0x0042D780`, O-316): if the core halts because of a pause
    /// of the own slot, the lift ([`Lockstep::resume`]); if the match is running, a `Pause` wish in the next
    /// contribution; otherwise (foreign pause, resync, before the start) nothing. `true` if a wish was made or the
    /// pause was lifted.
    pub fn toggle_pause(&mut self) -> bool {
        if self.resume_own_pause() {
            return true;
        }
        match self.lockstep_ref().phase() {
            Phase::Running => {
                self.lockstep().wish(Wish::Pause);
                true
            }
            phase => {
                log::info!("Network: pause key without effect in phase {phase:?}");
                false
            }
        }
    }

    /// Lifts the running pause if it is from the own slot (O-316; surrendering in a network match); otherwise nothing.
    /// `true` if it was lifted.
    pub fn resume_own_pause(&mut self) -> bool {
        let own = self.own_slot();
        match self.lockstep_ref().phase() {
            Phase::Halted(HaltReason::Pause { by }) if Some(*by) == own => {
                self.lockstep().resume();
                true
            }
            _ => false,
        }
    }

    /// Focus loss of the window on the network (O-154): like the pause key, but only as a wish - without a visible
    /// pause and while the match is running; there is no lift. `true` if a wish was made.
    pub fn focus_lost(&mut self) -> bool {
        if self.pause_shown() || self.lockstep_ref().phase() != &Phase::Running {
            return false;
        }
        self.lockstep().wish(Wish::Pause);
        true
    }

    /// Game speed key on the network (`FUN_0042F050`, E-265, E-294): only at the host; target value from the running
    /// game speed `current` as in single player ([`crate::game_session::key_game_speed`]), wished as `Speed(v)`.
    /// Returns `v` for the `GameSpeed` setting of the host (the original writes `EngineConfig+0x34` on pressing); the
    /// game speed itself and the message come only with the control entry. `None` for clients and at the limit.
    pub fn change_game_speed(&mut self, current: u8, d: i32) -> Option<u8> {
        if !self.is_host() {
            log::debug!("Network: game speed key without effect at the client");
            return None;
        }
        let v = crate::game_session::key_game_speed(current, d)?;
        self.lockstep().wish(Wish::Speed(v));
        Some(v)
    }

    /// Saving on the network (E-274): wish `Save(name)` in the next contribution, only with a valid name
    /// ([`valid_save_name`]); `true` if a wish was made.
    pub fn wish_save(&mut self, name: &str) -> bool {
        if !valid_save_name(name) {
            log::warn!("Network: saving as {name:?} discarded, not a valid name");
            return false;
        }
        self.lockstep().wish(Wish::Save(name.to_string()));
        true
    }

    /// Orderly end of the match by the host ([`HostCore::end`]): `End` in the next bundle, everyone then writes their
    /// network savegame ([`NetFinish::HostEnded`]). No effect for a client and outside the match; `true` if
    /// accepted.
    pub fn end(&mut self) -> bool {
        let Some(h) = self.host_core_mut() else {
            log::warn!("Network: only the host can end the match");
            return false;
        };
        let done = h.end();
        self.ending |= done;
        // A lift of the pause (`Resume`) goes out right away.
        self.flush();
        done
    }

    /// Host: the orderly end ([`Self::end`]) or the handover ([`Self::hand_over`]) is accepted and not yet
    /// executed.
    pub fn ending(&self) -> bool {
        (self.ending || self.handing_over) && !self.finished
    }

    /// Debug aid: the handover is accepted and has not fallen back to the end.
    #[cfg(test)]
    pub(crate) fn handing_over(&self) -> bool {
        self.handing_over
    }

    /// The network match is over for this machine ([`NetFrame::finish`] reported or left).
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// `Go` came: the match was running (until then `opennml-game` locks the input, a failure before it ends the network
    /// start).
    pub fn started(&self) -> bool {
        self.in_play
    }

    /// Reason of the first failure of the session (rejection, start abort, dropout of the host ...), also after
    /// [`NetFrame::failed`] was fetched; otherwise `None`.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Events of the lobby since the last fetch (`LobbyChanged`, `Chat`, `Kicked`, `HostLeft`, `Rejected`), filled by
    /// every [`Self::pump`].
    pub fn take_lobby_events(&mut self) -> Vec<CoreEvent> {
        std::mem::take(&mut self.lobby_events)
    }

    /// Debug aid `net_debug drop`: close all connections without `Bye`. The peers see the end of the connection
    /// (dropout or abort of the host); this machine notices it only by the silence ([`opennml_net::TIMEOUT_MS`]), as with
    /// a cut line.
    pub fn drop_links(&mut self) {
        log::warn!("Network: check run cuts all connections without Bye");
        self.close_all(DROPPED);
    }

    /// Host: the match is still running (turns or halt) and is not over; then the host leaves it in an orderly way via
    /// [`Self::end`] instead of [`Self::leave`] ("leaves the match" belongs to the orderly end).
    pub fn host_in_match(&self) -> bool {
        self.is_host() && !self.finished && self.lockstep_ref().phase().in_match()
    }

    /// Leaves the session: `Bye { reason }` to the peer(s) ([`Lockstep::leave`]), all connections closed, afterwards
    /// the driver no longer computes. At the end of the match of the client (accounting), at program end and when
    /// closing the window; a second time without effect. The host treats the `Bye` of a client like a dropout; if the
    /// host leaves, it is an abort for the clients (the orderly end is [`Self::end`]).
    pub fn leave(&mut self, reason: &str) {
        if self.left {
            return;
        }
        self.left = true;
        self.finished = true;
        self.stand = true;
        log::info!("Network: session left: {reason}");
        self.lockstep().leave(reason.to_string());
        self.flush();
        self.close_all(reason);
    }

    /// The network match is over for this machine (once): stand, never compute again, report.
    fn finish(&mut self, finish: NetFinish) {
        if self.finished {
            log::debug!("Network: end {finish:?} discarded after the end");
            return;
        }
        log::info!("Network: match over: {finish:?}");
        self.finished = true;
        self.stand = true;
        // No resync after the end: otherwise `try_adopt` would still try an adoption.
        self.adopt_due = None;
        self.resync_bytes = None;
        // A finished machine no longer migrates: listener and waiting ones closed, election ended.
        self.listener = None;
        for a in &mut self.accepted {
            a.link.close("The match is over");
        }
        self.accepted.clear();
        self.stop_migration("The match is over");
        self.frame.finish = Some(finish);
    }

    /// Network part of the host for a network savegame: session, own slot, raster and slot assignment from the start;
    /// `None` before the start.
    pub fn save_info(&self) -> Option<NetSaveInfo> {
        let s = self.start.as_ref()?;
        Some(NetSaveInfo {
            session: s.session.clone(),
            local_slot: s.your_slot,
            spacing: s.spacing,
            lead: s.lead,
            seats: s.seats.iter().map(|x| (x.peer, x.name.clone())).collect(),
        })
    }

    /// Read connections -> core, report silence, send outbox. Runs in every pass of the event loop, also without a
    /// match; [`Self::advance`] calls it itself.
    pub fn pump(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        self.poll_probes();
        self.pump_migration(now_ms);
        self.pump_accepted(now_ms);
        let core: &mut dyn Lockstep = match &mut self.core {
            Core::Host(h) => h.as_mut(),
            Core::Client(c) => c.as_mut(),
        };
        for (peer, link) in &mut self.links {
            for ev in link.poll() {
                core.handle(now_ms, *peer, ev);
            }
            // Silence on the clock of the connection, not against `now_ms`.
            let silent = link.age_ms().saturating_sub(link.last_received_ms());
            core.silence(now_ms, *peer, silent);
        }
        core.tick(now_ms);
        self.flush();
        self.close_silent_newcomers();
    }

    /// Host: poll running probes. Succeeded -> close the connection at once and report the address, failed or without
    /// an attempt -> "unreachable" ([`HostCore::set_reachable`]; `Go` waits for the result of every client).
    fn poll_probes(&mut self) {
        let mut results = Vec::new();
        self.probes.retain_mut(|probe| {
            let Some(connecting) = probe.connecting.as_mut() else {
                results.push((probe.peer, None));
                return false;
            };
            match connecting.poll() {
                None => true,
                Some(Ok(mut link)) => {
                    link.close("Reachability probe");
                    log::info!(
                        "Network: peer {} is reachable at {:?}",
                        probe.peer,
                        probe.addr
                    );
                    results.push((probe.peer, probe.addr));
                    false
                }
                Some(Err(e)) => {
                    log::info!(
                        "Network: peer {} is not reachable at {:?} ({e})",
                        probe.peer,
                        probe.addr
                    );
                    results.push((probe.peer, None));
                    false
                }
            }
        });
        if let Core::Host(h) = &mut self.core {
            for (peer, addr) in results {
                h.set_reachable(peer, addr);
            }
        }
    }

    /// Host: create a probe for the client `peer` just accepted (IP of the connection and reported listening port,
    /// deadline [`SUCCESSOR_CONNECT_MS`]).
    fn start_probe(&mut self, peer: Peer) {
        let Core::Host(h) = &self.core else {
            return;
        };
        let ip = self
            .links
            .iter()
            .find(|(p, _)| *p == peer)
            .and_then(|(_, l)| l.peer_ip());
        let addr = ip
            .zip(h.listen_port(peer))
            .map(|(ip, port)| SocketAddr::new(ip, port));
        let connecting = addr
            .zip(self.transport.as_mut())
            .map(|(addr, t)| t.connect(addr, SUCCESSOR_CONNECT_MS));
        self.probes.push(Probe {
            peer,
            addr,
            connecting,
        });
    }

    /// Host: close and forget connections that are not yet an accepted machine after [`HELLO_TIMEOUT_MS`] (on their
    /// own clock). This includes silent connections and rejected ones (`Reject` is then already out and the
    /// connection closed).
    fn close_silent_newcomers(&mut self) {
        let Core::Host(h) = &self.core else {
            return;
        };
        let known: Vec<Peer> = h.peers().into_iter().map(|(peer, _)| peer).collect();
        self.links.retain_mut(|(peer, link)| {
            if known.contains(peer) || link.age_ms() < HELLO_TIMEOUT_MS {
                return true;
            }
            log::warn!("Network: connection {peer} closed without sign-on ({NO_HELLO})");
            link.close(NO_HELLO);
            false
        });
    }

    /// Send the core's outbox and process its events until both are empty (an event can trigger another, `HostGone`
    /// -> `abandon` -> `Failed`, and on a change of role the new core has its own outbox).
    fn flush(&mut self) {
        loop {
            self.send_outbox();
            let events = self.lockstep().events();
            if events.is_empty() {
                break;
            }
            for ev in events {
                self.on_event(ev);
            }
        }
    }

    /// Send the core's outbox. After `Reject`/`Bye` the driver closes the connection (core contract).
    fn send_outbox(&mut self) {
        let outbox = self.lockstep().outbox();
        for (to, msg) in outbox {
            let Some((_, link)) = self.links.iter_mut().find(|(p, _)| *p == to) else {
                log::warn!("Network: no connection to {to}, message discarded: {msg:?}");
                continue;
            };
            // After `Reject`/`Bye` the driver closes (core contract); queued messages still go out.
            let closing = match &msg {
                NetMessage::Reject { reason } | NetMessage::Bye { reason } => Some(reason.clone()),
                _ => None,
            };
            link.send(msg);
            if let Some(reason) = closing {
                link.close(&reason);
                self.mark_closed(to);
            }
        }
        let large = self.lockstep().large_outbox();
        for (to, id, bytes) in large {
            match self.links.iter_mut().find(|(p, _)| *p == to) {
                Some((_, link)) => link.send_large(id, bytes),
                None => log::warn!("Network: no connection to {to}, large content {id} discarded"),
            }
        }
    }

    fn close_link(&mut self, peer: Peer, reason: &str) {
        if let Some((_, link)) = self.links.iter_mut().find(|(p, _)| *p == peer) {
            link.close(reason);
            self.mark_closed(peer);
        }
    }

    /// Close all connections (failure, leaving, debug aid `drop`), also those of a running election.
    fn close_all(&mut self, reason: &str) {
        for a in &mut self.accepted {
            a.link.close(reason);
        }
        self.accepted.clear();
        self.listener = None;
        self.stop_migration(reason);
        for (peer, link) in &mut self.links {
            log::debug!("Network: connection {peer} closed: {reason}");
            link.close(reason);
        }
        let peers: Vec<Peer> = self.links.iter().map(|(p, _)| *p).collect();
        for peer in peers {
            self.mark_closed(peer);
        }
    }

    /// `peer` no longer counts among the open connections ([`NetStatus::links`]).
    fn mark_closed(&mut self, peer: Peer) {
        if !self.closed.contains(&peer) {
            self.closed.push(peer);
        }
    }

    fn on_event(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::Joined { peer, name } => {
                log::info!("Network: \"{name}\" joined (peer {peer})");
                // The client reports `Joined` also for itself; only the host probes.
                self.start_probe(peer);
            }
            CoreEvent::Start(info) => {
                log::info!("Network: start, own slot {}", info.your_slot);
                self.start = Some(*info);
            }
            CoreEvent::Go => {
                log::info!("Network: Go, tick 0 begins");
                self.in_play = true;
            }
            CoreEvent::Lost { peer, slot, reason } => {
                // Close the connection of the one who dropped out.
                self.close_link(peer, &reason);
                match slot {
                    Some(slot) => self.frame.lost.push((slot, reason)),
                    None => log::info!("Network: peer {peer} left before the start: {reason}"),
                }
            }
            CoreEvent::Control { turn, control } => {
                // If the handover was no longer possible when the bundle was formed (`HostCore::controls`), it
                // carries `End`; from now on the end applies to everyone (message, network savegame, log text of
                // `close_program`).
                if self.handing_over && matches!(control, NetControl::End { .. }) {
                    log::warn!(
                        "Network: handover no longer possible, the match ends with bundle {turn} for everyone"
                    );
                    self.handing_over = false;
                    self.ending = true;
                }
                // Takes effect after the execution tick; the driver takes the entries from the bundle.
                log::debug!("Network: control entry {control:?} from bundle {turn}");
            }
            CoreEvent::SendState { slots } => {
                // Sent only after the own `e(k)` (`after_step` of the bundle).
                log::warn!(
                    "Network: resync for slots {slots:?}, state follows after the execution tick"
                );
            }
            CoreEvent::ResyncState { bytes } => {
                // Keep until the own `e(k)` (`try_adopt`).
                log::info!(
                    "Network: state ({} bytes) received for the resync",
                    bytes.len()
                );
                self.resync_bytes = Some(bytes);
            }
            CoreEvent::Resumed => log::info!("Network: halt lifted"),
            CoreEvent::Failed(text) => self.failed(text),
            // Like `Failed`; the English sentence is for the log and `net_status`, the message names the player
            // (99 113).
            CoreEvent::LoadFailed { name } => {
                if self.failure.is_none() {
                    self.load_failed = Some(name.clone());
                }
                self.failed(format!(
                    "\"{name}\" could not load the match. The start was cancelled."
                ));
            }
            CoreEvent::LobbyChanged | CoreEvent::Chat { .. } => self.lobby_events.push(ev),
            CoreEvent::Kicked => self.lobby_end(CoreEvent::Kicked, KICKED.to_string()),
            CoreEvent::HostLeft => self.lobby_end(CoreEvent::HostLeft, HOST_LEFT.to_string()),
            CoreEvent::Rejected { reason } => {
                let text = format!("Rejected by the host: {reason}");
                self.lobby_end(CoreEvent::Rejected { reason }, text);
            }
            CoreEvent::HostGone { reason, planned } => self.start_migration(reason, planned),
            CoreEvent::Rehomed => log::info!("Network: signed on with the new host"),
            CoreEvent::MigrationDone { lost } => self.migration_done(lost),
            CoreEvent::MigrationFailed { reason } => self.migration_failed(&reason),
        }
    }

    /// The session has failed (`CoreEvent::Failed`, `CoreEvent::LoadFailed`): reason in [`Self::failure`], connections
    /// closed; in the match the abort follows.
    fn failed(&mut self, text: String) {
        self.failure.get_or_insert_with(|| text.clone());
        self.close_all(&text);
        if self.in_play && !self.finished && !self.gave_up {
            match self.core {
                // Abort of the host: state and message in the next `advance` (it needs the game host).
                Core::Client(_) => self.abort_due = true,
                Core::Host(_) => self.finish(NetFinish::Failed(text.clone())),
            }
        }
        self.frame.failed.get_or_insert(text);
    }

    /// End of the lobby for this client: like a failure before the start (reason in [`Self::failure`], connection
    /// closed), the event itself goes to the menu host.
    fn lobby_end(&mut self, ev: CoreEvent, text: String) {
        log::info!("Network: {text}");
        self.close_all(&text);
        self.failure.get_or_insert(text);
        self.lobby_events.push(ev);
    }

    /// Checksum of the state before tick `steps` (history of the game host; in turn mode equal to the current one).
    fn round_hash(sim: &SimHost, steps: u64) -> u64 {
        sim.hash_at(steps).unwrap_or_else(|| {
            log::error!(
                "Network: no checksum for tick {steps} in the history, taking the current one"
            );
            opennml_sim::save::checksum(&sim.state)
        })
    }

    /// One frame: pump connections, turn 0 after `Go`, game time `dt_ms` (virtual time like [`SimHost::advance`]) in
    /// ticks per the raster, send outbox.
    pub fn advance(&mut self, sim: &mut SimHost, dt_ms: f64, now_ms: u64) -> NetFrame {
        let clock = self.local_time();
        if let Core::Host(h) = &mut self.core {
            // Name of the stop from the local time of the host; applies when the bundle is formed.
            h.set_clock(clock);
        }
        // Results already before the first tick (migration before the first `after_step`,
        // `HostCore::from_client`).
        self.results = sim.results();
        self.pump(now_ms);
        self.follow_role(sim);
        // Failure between `Start` and `Go`: the driver is already in the loaded match (only it calls `advance`), `Go`
        // never comes. Without an end the map would stand without a message; this way the flow shows the message, and
        // its "OK" (`0x0E`) leads to the main menu. Before `Go` there is no state to secure.
        if !self.in_play
            && !self.finished
            && self.start.is_some()
            && let Some(text) = self.failure.clone()
        {
            let finish = match self.load_failed.clone() {
                Some(name) => NetFinish::LoadFailed { name },
                None => NetFinish::Failed(text),
            };
            self.finish(finish);
        }
        // A state that arrived while the machine already stood after `e(k)`.
        self.try_adopt(sim);
        // Abort of the host: compute up to the waiting point, capture the state.
        let mut settled = self.settle_abort(sim);
        if self.next_close == 0 && self.lockstep_ref().phase().in_match() {
            // Turn 0 empty right after `Go`, before any own command - at the origin tick (0 or the tick of the loaded
            // state).
            let origin = self.origin;
            if sim.state.steps != origin {
                log::error!(
                    "Network: turn 0 at tick {} instead of {origin}",
                    sim.state.steps
                );
            }
            let hash = Self::round_hash(sim, origin);
            self.lockstep().close_round(0, hash);
            self.next_close = 1;
        }
        let events = sim.advance_rounds(dt_ms, &mut Turns(self));
        if let Core::Host(h) = &mut self.core {
            h.set_results(sim.results());
        }
        self.flush();
        self.follow_role(sim);
        let mut frame = std::mem::take(&mut self.frame);
        // After an adoption the events of this frame belong to the discarded state (afterwards the machine stands,
        // no tick follows in the same frame).
        settled.extend(events);
        frame.events = if frame.adopted { Vec::new() } else { settled };
        frame
    }

    /// Abort of the host at the client: the driver computes without game time with the bundles it has, until it
    /// stands before the execution tick of the first missing bundle (or after a halt or stop from an existing bundle),
    /// captures the network savegame `<prefix>YYYY_MM_DD_hhmm` there by its own local time and reports
    /// [`NetFinish::HostLost`]. A client has a bundle `k` only after its contribution to `k` (at `b(k)`), so it is at
    /// most `T + V` ticks. If the match already ends otherwise in this (stop in the bundle), it stays that way.
    /// Returns the events of the ticks.
    ///
    /// Pause and resync do not halt in the recomputation: whether a machine stood after the execution tick of a bundle
    /// with halt depends on whether the `Resume` arrived before (`Phase::Halted`); if it stood there, its state would
    /// differ from that of a machine that had already continued. A state after a pause is therefore lifted, and all
    /// clients with the same last bundle `k` compute up to before `e(k + 1)`.
    fn settle_abort(&mut self, sim: &mut SimHost) -> Vec<SimEvent> {
        if !std::mem::take(&mut self.abort_due) {
            return Vec::new();
        }
        if self.stand && !self.finished {
            log::info!(
                "Network: halt at tick {} lifted for the recomputation",
                sim.state.steps
            );
            self.stand = false;
        }
        self.settling = true;
        let cfg = self.lockstep_ref().config();
        let limit = cfg.spacing() + cfg.lead() + 1;
        let mut events = Vec::new();
        let mut steps = 0;
        while steps < limit {
            match self.before_step(sim) {
                RoundStep::Wait => break,
                RoundStep::Step(commands) => {
                    events.extend(sim.step_with(commands));
                    self.after_step(sim);
                    steps += 1;
                }
            }
        }
        self.settling = false;
        if steps == limit {
            log::error!("Network: {limit} ticks computed after the abort without a waiting point");
        }
        if !self.finished {
            let name = self
                .local_time()
                .net_end_name(&self.words().net_save_prefix);
            log::warn!(
                "Network: connection to the host aborted, state \"{name}\" at tick {} ({steps} ticks recomputed)",
                sim.state.steps
            );
            self.frame.saves.push(NetSave {
                name: name.clone(),
                sim: sim.save(),
                game_speed: sim.game_speed,
                log: sim.save_log(),
            });
            self.finish(NetFinish::HostLost { name });
        }
        events
    }

    fn before_step(&mut self, sim: &mut SimHost) -> RoundStep {
        if self.gave_up {
            return RoundStep::Wait;
        }
        if self.stand {
            if self.lockstep_ref().phase() != &Phase::Running {
                return RoundStep::Wait;
            }
            log::info!("Network: continuing after tick {}", sim.state.steps);
            self.stand = false;
        }
        let phase = self.lockstep_ref().phase().clone();
        // After `Stop` and `End` the driver still computes up to the execution tick of the bundle, after an abort of
        // the host up to the waiting point ([`Self::settle_abort`]); afterwards it stands forever.
        let finishing =
            phase == Phase::Stopped || (phase == Phase::Ended && self.in_play && !self.finished);
        if !phase.in_match() && !finishing {
            return RoundStep::Wait;
        }
        // Raster from the origin tick.
        let s = sim.state.steps;
        if let Some(k) = self.round_at(s)
            && k == self.next_close
            && phase.in_match()
        {
            // Own commands, then the contribution with the checksum before `b(k)`.
            let commands: Vec<Command> = sim.take_outgoing();
            for c in commands {
                self.lockstep().submit(c);
            }
            let hash = Self::round_hash(sim, s);
            self.lockstep().close_round(k, hash);
            self.next_close += 1;
        }
        let Some(k) = self.exec_at(s) else {
            self.blocked = None;
            return RoundStep::Step(Vec::new());
        };
        let Some(bundle) = self.lockstep_ref().bundle(k) else {
            // Without a bundle do not compute.
            self.blocked = Some(k);
            return RoundStep::Wait;
        };
        if k != self.next_exec {
            log::error!(
                "Network: bundle {k} at tick {s}, expected turn {}",
                self.next_exec
            );
        }
        let bundle = bundle.clone();
        let commands = bundle.commands.clone();
        let mut controls = bundle.control.clone();
        controls.sort_by_key(control_rank);
        self.executing = Some(Executing {
            turn: k,
            controls,
            bundle,
        });
        self.blocked = None;
        RoundStep::Step(commands)
    }

    fn after_step(&mut self, sim: &mut SimHost) {
        self.results = sim.results();
        if let Core::Host(h) = &mut self.core {
            // The results of the slots to the core.
            h.set_results(self.results);
        }
        let Some(Executing {
            turn,
            controls,
            bundle,
        }) = self.executing.take()
        else {
            return;
        };
        self.next_exec = turn + 1;
        // Forget bundles only `kept_bundles` turns after their execution: they travel in the `Rejoin` on a host
        // migration.
        let kept = self.lockstep_ref().config().kept_bundles();
        self.lockstep()
            .forget_before((turn + 1).saturating_sub(kept));
        self.recent.push_back(bundle);
        while self.recent.len() > REPORT_BUNDLES {
            self.recent.pop_front();
        }
        for c in &controls {
            if let NetControl::Speed(v) = c {
                // The core accepts only game speed <= `MAX_GAME_SPEED`; checked once more here.
                if *v <= MAX_GAME_SPEED {
                    self.frame.speed.push(SpeedChange {
                        step: sim.state.steps,
                        old: sim.game_speed,
                        new: *v,
                    });
                    sim.game_speed = *v;
                } else {
                    log::warn!("Network: game speed {v} above {MAX_GAME_SPEED} discarded");
                }
            }
        }
        // Network savegames after the speed (speed, then save, then stop/end), now at `e(k) + 1` and not only at the
        // end of the frame: afterwards the machine can continue computing in the same frame.
        for c in &controls {
            if let NetControl::Save { name }
            | NetControl::Stop { name }
            | NetControl::End { name } = c
            {
                self.frame.saves.push(NetSave {
                    name: name.clone(),
                    sim: sim.save(),
                    game_speed: sim.game_speed,
                    log: sim.save_log(),
                });
            }
        }
        let resync = controls.iter().find_map(|c| match c {
            NetControl::Resync { slots } => Some(slots.clone()),
            _ => None,
        });
        // Only a running resync counts. A `Resync` from a bundle that a host migration supplemented lapses with it: the
        // core is then no longer in `Halted(Resync)`.
        let resyncing = self.lockstep_ref().phase() == &Phase::Halted(HaltReason::Resync);
        if let Some(slots) = resync.filter(|_| resyncing) {
            match &mut self.core {
                Core::Host(h) => {
                    // The host now stands itself after `e(k)`, in the same tick as the clients.
                    let bytes = sim.net_state().to_bytes();
                    log::info!(
                        "Network: state at tick {} built for the resync of slots {slots:?}",
                        sim.state.steps
                    );
                    h.send_state(bytes);
                }
                Core::Client(c) => {
                    if c.slot().is_some_and(|s| slots.contains(&s)) {
                        self.adopt_due = Some(turn);
                    }
                }
            }
        }
        // During the recomputation after an abort only stop and end halt ([`Self::settle_abort`]).
        let halts = controls.iter().any(|c| match c {
            NetControl::Halt { .. } | NetControl::Resync { .. } => !self.settling,
            NetControl::Stop { .. } | NetControl::End { .. } | NetControl::Handover { .. } => true,
            NetControl::Speed(_) | NetControl::Save { .. } => false,
        });
        // The state of the core decides, not the event.
        if halts
            && matches!(
                self.lockstep_ref().phase(),
                Phase::Halted(_) | Phase::Stopped | Phase::Ended
            )
        {
            log::info!(
                "Network: halts after tick {} (bundle {turn})",
                sim.state.steps.wrapping_sub(1)
            );
            self.stand = true;
        }
        // Stop and end are final: report once, the state is already in `saves`.
        // The handover ends only at the old host that asked for it: clients and a new host that supplemented the
        // bundle continue computing.
        let handing_over = self.handing_over;
        let finish = controls.iter().find_map(|c| match c {
            NetControl::Stop { name } => Some(NetFinish::Stopped { name: name.clone() }),
            NetControl::End { name } => Some(NetFinish::HostEnded { name: name.clone() }),
            NetControl::Handover { .. } if handing_over => Some(NetFinish::HandedOver),
            _ => None,
        });
        if let Some(finish) = finish {
            self.finish(finish);
        }
        self.frame.controls.extend(controls);
        // An early received state is adopted now.
        self.try_adopt(sim);
    }

    /// Client, target of a resync: if it stands after `e(k)` and the state of the host is there, write the report,
    /// adopt the state and report `Synced`. Otherwise without effect, also after the end of the match.
    fn try_adopt(&mut self, sim: &mut SimHost) {
        if self.finished {
            return;
        }
        let Some(turn) = self.adopt_due else {
            return;
        };
        if !self.stand || self.resync_bytes.is_none() {
            return;
        }
        let bytes = self.resync_bytes.take().unwrap_or_default();
        self.adopt_due = None;
        match self.adopt(sim, turn, &bytes) {
            Ok(()) => {
                log::info!(
                    "Network: state of the host at tick {} adopted",
                    sim.state.steps
                );
                self.frame.adopted = true;
                if let Some(c) = self.client_core_mut() {
                    c.synced();
                }
            }
            Err(e) => self.give_up(format!("The resync with the host failed: {e:#}")),
        }
    }

    /// Write the report (error only to the log), then adopt.
    fn adopt(&mut self, sim: &mut SimHost, turn: u32, bytes: &[u8]) -> anyhow::Result<()> {
        let host = NetState::from_bytes(bytes)?;
        if host.save.state.steps != sim.state.steps {
            anyhow::bail!(
                "state of the host at tick {}, own state at tick {}",
                host.save.state.steps,
                sim.state.steps
            );
        }
        let local = sim.net_state();
        let seats = self.start.as_ref().map_or_else(
            || {
                std::array::from_fn(|_| Seat {
                    peer: None,
                    name: String::new(),
                })
            },
            |s| s.seats.clone(),
        );
        self.reports += 1;
        let when = self.local_time();
        let written = report::write_desync(
            &self.paths.netlog,
            self.reports,
            turn,
            sim.state.steps,
            &local,
            &host,
            &seats,
            self.recent.make_contiguous(),
            when,
        );
        match written {
            Ok(dir) => log::warn!("Network: desync report in {}", dir.display()),
            Err(e) => log::error!(
                "Network: desync report under {} not written: {e}",
                self.paths.netlog.display()
            ),
        }
        sim.adopt(host)
    }

    /// The resync failed: connections closed (the host treats that as a dropout), no more computing; the match ends
    /// with the text as a message, without savegame (the own state deviates).
    fn give_up(&mut self, text: String) {
        log::error!("Network: {text}");
        self.close_all(&text);
        self.gave_up = true;
        self.finish(NetFinish::Failed(text.clone()));
        self.frame.failed.get_or_insert(text);
    }

    /// Slot of the peer `peer` after the start.
    fn slot_of(&self, peer: Peer) -> Option<u8> {
        if let Core::Host(h) = &self.core {
            return h.slot_of(peer);
        }
        seat_of(&self.start.as_ref()?.seats, peer)
    }

    fn wait_reason(&self) -> Option<WaitReason> {
        if self.gave_up {
            return Some(WaitReason::Ended);
        }
        match self.lockstep_ref().phase() {
            Phase::Gathering | Phase::Loading => Some(WaitReason::Starting),
            Phase::Ended => Some(WaitReason::Ended),
            Phase::Stopped if self.stand => Some(WaitReason::Stopped),
            Phase::Halted(r) if self.stand => Some(WaitReason::Halted(r.clone())),
            _ => self.blocked.map(|turn| WaitReason::Bundle { turn }),
        }
    }

    /// Network status.
    pub fn status(&self) -> NetStatus {
        let core = self.lockstep_ref();
        let (role, missing, desyncs) = match &self.core {
            Core::Host(h) => (NetRole::Host, h.missing(), Some(h.desyncs())),
            Core::Client(_) => (NetRole::Client, Vec::new(), None),
        };
        NetStatus {
            role,
            phase: core.phase().clone(),
            own_slot: self.start.as_ref().map(|s| s.your_slot),
            seats: self.start.as_ref().map(|s| s.seats.clone()),
            round: self.next_exec,
            next_exec_step: self.exec_step(self.next_exec),
            waiting: self.wait_reason(),
            missing,
            desyncs,
            links: self
                .links
                .iter()
                .filter(|(peer, _)| !self.closed.contains(peer))
                .map(|(peer, link)| LinkStatus {
                    peer: *peer,
                    slot: self.slot_of(*peer),
                    silent_ms: link.age_ms().saturating_sub(link.last_received_ms()),
                })
                .collect(),
            successors: match &self.core {
                Core::Host(h) => h.successors(),
                Core::Client(c) => c.successors().to_vec(),
            },
            migration: self.migration_status(),
        }
    }
}

/// Connection of the driver to [`SimHost::advance_rounds`].
struct Turns<'a>(&'a mut NetDriver);

impl Rounds for Turns<'_> {
    fn before_step(&mut self, sim: &mut SimHost) -> RoundStep {
        self.0.before_step(sim)
    }

    fn after_step(&mut self, sim: &mut SimHost) {
        self.0.after_step(sim);
    }
}
