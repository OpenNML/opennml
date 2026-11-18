//! Turn core.
//!
//! [`HostCore`] and [`ClientCore`] are pure state machines ("sans-IO"): events of the connections and the time `now_ms`
//! go in; messages to send, events for the driver and released turn bundles come out. No thread, no clock, no
//! connection in the core; the driver (`opennml-client`) pumps.
//!
//! Flow of a match:
//! 1. Session: the client sends `Hello`, the host answers `Welcome` or `Reject` (phase `Gathering`). The host calls
//!    [`HostCore::start`] -> `Start` to every client (phase `Loading`); everyone loads and reports `Loaded`; once the
//!    host has all of them and is finished itself ([`HostCore::loaded_self`]), it sends `Go` (phase `Running`). No
//!    machine computes a tick before `Go` (start lock, E-293 deviation; original
//!    `NetSession_SendClientListWhenAllLoaded` `0x004E83B0`).
//! 2. Turns: every machine (the host too) closes its contribution **before** the turn tick `b(k)`
//!    ([`Lockstep::close_round`]): all commands since the previous contribution in input order. The host hands in its
//!    contribution directly without the network - the same delay for everyone (E-256 deviation; original
//!    `NetSession_SubmitMessage` `0x004E9B60`). Once it has the contribution `k` of every machine, it forms the bundle
//!    `k`: commands by slot, within a slot in input order (E-257 deviation; original `NetSession_UnpackCommandList`
//!    `0x004E9860`).
//! 3. A machine computes the execution tick `e(k)` only when [`Lockstep::bundle`] returns the bundle `k`; the grid is
//!    computed by [`TurnConfig`] alone (E-255 adopted; original `NetSession_Construct` `0x004EA240`).
//! 4. Wishes travel in the contribution; per turn the host accepts at most one `Pause` (smallest slot), one `Speed`
//!    (only from its own contribution) and one `Save` (smallest slot, only with [`crate::valid_save_name`]) and
//!    appends them as control entries to the bundle (in a fixed order). Commands of a client
//!    contribution with a foreign sender and the mirror commands `ScriptSelection`/`ScriptButton` it discards.
//! 5. Dropout: `Closed`, `Bye` or [`TIMEOUT_MS`] of silence ([`Lockstep::silence`]). Before `Go` it aborts the start
//!    (in `Gathering` it only removes the machine); during the match the host reports [`CoreEvent::Lost`] once,
//!    appends to the next bundle the command `SetPlayerResult { slot: p, result: 1 }` with sender `p` (original
//!    `NetSession_OnClientLost` `0x004E9C70`) and no longer waits for `p`.
//! 6. Checksum: at bundle `k` the host compares the checksum of every client contribution with its own (state before
//!    `b(k)`); contributions of machines that dropped out do not count. If clients deviate, that is a desync (several
//!    in one turn count once, [`HostCore::desyncs`]). Desync 1 and 2: control entry `Resync { slots }`, everyone halts
//!    after `e(k)` in `Halted(Resync)`; host driver: `CoreEvent::SendState` -> [`HostCore::send_state`]; client driver
//!    of the targets: `CoreEvent::ResyncState` -> take over the state -> [`ClientCore::synced`]; once the host has all
//!    `Synced`, it sends `Resume`. Desync 3: `Stop { name }` with [`LocalTime::checksum_error_name`] (local time of the
//!    host, [`HostCore::set_clock`]), everyone stays in `Phase::Stopped`. The original never compares the checksum
//!    (E-260, E-261 deviation).
//!
//! 7. End and abort: [`HostCore::end`] appends `End { name }` with [`LocalTime::net_end_name`] to the next bundle (it
//!    lifts a running pause beforehand); whoever forms or receives this bundle goes into `Phase::Ended`, the driver
//!    still computes until `e(k)` and then stands. Right after the bundle the host sends `Bye` to everyone. If the
//!    host drops out during the match (`Closed`, `Bye`, [`TIMEOUT_MS`] of silence), the client reports
//!    `CoreEvent::Failed` with [`HOST_LOST`]. Before `Go` the same holds for a closed connection and silence (the raw
//!    reason of the system only in the log); only a `Bye` of the host carries its reason in the text there, cleaned
//!    with [`crate::msg::clean_reason`]. [`Lockstep::leave`] leaves the session in an orderly way (`Bye`,
//!    `Phase::Ended`, without an event); the host treats the `Bye` of a client like any dropout.
//! 8. Protocol errors: whatever an honest machine never sends over the ordered wire ends the connection with a
//!    message in the log and counts as a dropout - for the host a contribution during the match with a turn other than
//!    the expected one (duplicate, from the future, already bundled) or before the bundle of the previous turn, for
//!    the client a bundle during the match with a turn other than the next one. Messages in a phase in which they
//!    achieve nothing (say a contribution before `Go`, a second `Welcome`, a `Resume` without a halt) the core only
//!    discards with a log entry: they change nothing, and some can also arrive that way honestly because of latency.
//!
//! 9. Lobby (only in `Gathering`): the host holds a [`crate::LobbyBook`]. An accepted `Hello` takes the first free
//!    client index ([`crate::LobbyBook::join`]); after **every** change (join after the `Welcome`, leaving, kick,
//!    wish, host change, latency) the whole state goes as `Lobby` individually to every client (E-303, E-304, E-306
//!    deviation), backed by a changed `revision`; both cores then report `CoreEvent::LobbyChanged`. Clients send only
//!    `LobbyWish` for their own row, the host sets the row from the connection (E-280, E-310 deviation). Chat: the
//!    host sanitizes the text ([`crate::msg::clean_chat`]), the sender is the row of the connection, `Chat` goes to
//!    everyone (the sender too) and as `CoreEvent::Chat` to the host (E-286). Kick = `Bye { KICKED }`, the host leaves
//!    the lobby with `Bye { HOST_LEFT }` to everyone; the client then reports `CoreEvent::Kicked` or
//!    `CoreEvent::HostLeft` instead of `Failed`, a `Reject` before the `Welcome` as `CoreEvent::Rejected`. Latency:
//!    every [`PROBE_INTERVAL_MS`] a `Probe` to every client, the response `ProbeAck` is measured by the host with
//!    `now_ms` (last value per machine, largest in the state). The start is either checked with
//!    [`HostCore::start_from_lobby`] (slot from `info_for`) or unchecked with [`HostCore::start`] (tests,
//!    `NET_GOLDEN`, command line).
//!
//! 10. Host migration: every client reports in the `Hello` the port it listens on (`Hello::listen_port`). The host
//!     holds per client the result of the reachability probe ([`HostCore::set_reachable`], from the driver); without a
//!     listening port the client counts as not reachable at once. `Go` comes only when every active client has loaded
//!     and its result is there, and carries the successor list ([`HostCore::successors`]: all clients by peer with
//!     slot and address, `None` = not reachable). The client holds it ([`ClientCore::successors`]); more than eight
//!     entries or a duplicate peer result in an empty list, the match continues.
//!
//!     The client retains each of its own contributions until the bundle of its turn is there, and counts the desyncs
//!     from the bundles (`Resync` and `Stop` one each, [`ClientCore::desyncs`]); the driver forgets the bundles only
//!     [`TurnConfig::kept_bundles`] turns after their execution. If the host drops out after `Go` (as in point 7) and
//!     the list has a candidate ([`Succession`], after a `Handover` only among its peers), the client reports
//!     `CoreEvent::HostGone` instead of `Failed`; the phase stays, further dropouts of the old connection no longer
//!     count. Before `Go` and without a candidate it stays with the abort. The driver chooses with [`Succession`]:
//!     [`ClientCore::rejoin`] builds the `Rejoin` (contributions closed after that are held back by the core),
//!     [`ClientCore::rehome`] takes over the `Migrated` of the new host (`CoreEvent::Rehomed`), [`ClientCore::abandon`]
//!     gives up (`Failed` with [`HOST_LOST`]).
//!
//!     If the client becomes host itself, [`HostCore::from_client`] makes a host from its core (peer 0, the others keep
//!     their number). It collects the `Rejoin` ([`HostCore::on_rejoin`]) until every expected client is there (entries
//!     without a result) or [`REJOIN_WINDOW_MS`] are over, does not bundle until then, then fills in the margin (`K` =
//!     highest bundle of all), sends each its `Migrated`, reports `CoreEvent::MigrationDone` and continues bundling
//!     from `K + 1`, with the defeat of the old host and of those who did not return. If a bundle `<= K` is missing at
//!     all of them, it aborts (`CoreEvent::MigrationFailed`).
//!
//!     Orderly handover: [`HostCore::hand_over`] appends to the next bundle the dropout of its own slot (without a
//!     result) and `Handover { peers }` with the active clients; afterwards the host is `Ended` and sends `Bye` with
//!     [`crate::msg::HANDED_OVER`]. Without an active reachable client it refuses (then [`HostCore::end`] as before);
//!     `end` and `hand_over` exclude each other. The client then reports the dropout as `HostGone { planned: true }`,
//!     and collecting happens only among `peers` (`only` in [`HostCore::from_client`]).
//!
//! The driver closes a connection after it has sent it a `Reject` or `Bye` from [`Lockstep::outbox`].

mod client;
mod host;
mod succession;
#[cfg(test)]
mod tests;

pub use client::ClientCore;
pub use host::{HostCore, MAX_COMMANDS_PER_TURN, MAX_TURN_COMMAND_BYTES, MAX_WISHES_PER_TURN};
pub use succession::{Succession, SuccessionStep};

use crate::link::LinkEvent;
use crate::lobby::LobbyState;
use crate::msg::{
    Hello, NetControl, NetMessage, StartInfo, Successor, TurnBundle, TurnContribution, Wish,
};
use crate::turn::TurnConfig;
use opennml_sim::command::Command;
use std::collections::BTreeMap;

/// Connection number: 0 = host, clients as chosen by the driver (`Welcome { slot }` reports it to the client).
pub type Peer = u8;

/// Connection number of the host.
pub const HOST_PEER: Peer = 0;

/// Timeout: this long without reception a peer counts as dropped out.
pub const TIMEOUT_MS: u64 = 30_000;

/// Interval of the host's latency probes in `Gathering`.
pub const PROBE_INTERVAL_MS: u64 = 1_000;

/// Deadline for every connection attempt of a client with a candidate of the successor list.
pub const SUCCESSOR_CONNECT_MS: u64 = 3_000;

/// Deadline `W` for collecting the `Rejoin` at the new host and for waiting for `Migrated`.
pub const REJOIN_WINDOW_MS: u64 = 10_000;

/// Text of `CoreEvent::Failed` at the client when the host drops out during the match (connection closed, `Bye`,
/// [`TIMEOUT_MS`] of silence, protocol error). The reason is in the log.
pub const HOST_LOST: &str = "The connection to the host was aborted.";

/// Phase of a core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Before the start: clients sign in (for the client also before the `Welcome`).
    Gathering,
    /// `Start` has been distributed, the machines load.
    Loading,
    /// After `Go`: turns are running.
    Running,
    /// Everyone stands after the execution tick `e(k)` of the bundle `k` that carries the halt. The core switches here
    /// as soon as it forms this bundle (host) or receives it (client), and reports `CoreEvent::Control { turn: k,
    /// .. }`; the driver must stop after `e(k)`. Because no machine reaches `b(k+1)` before `e(k)`, no further bundle
    /// arises until it is lifted. Bundles and contributions remain valid ([`Phase::in_match`]).
    ///
    /// A machine at which the `Resume` already arrived before `e(k)` does not halt: after `e(k)` the driver checks the
    /// state of the core, not the `Control` event. This affects only the time; bundle sequence and state stay the
    /// same.
    Halted(HaltReason),
    /// The match is stopped after a stop (third desync) and stays so. The core switches here as soon as it forms or
    /// receives the bundle `k` with `Stop`; the driver still computes until `e(k)` (the bundle continues to be returned
    /// by [`Lockstep::bundle`]), writes the state and then stands. No more contributions, bundles or lifting.
    Stopped,
    /// The session is over (end, abort, rejection, error, leaving); the core accepts nothing more. On an orderly end
    /// the core switches here as soon as it forms or receives the bundle `k` with `End`; as with the stop
    /// [`Lockstep::bundle`] continues to return it, the driver still computes until `e(k)` and then stands.
    Ended,
}

impl Phase {
    /// The match runs or halts at an execution tick: turns and bundles are valid.
    pub fn in_match(&self) -> bool {
        matches!(self, Phase::Running | Phase::Halted(_))
    }
}

/// Reason for a halt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HaltReason {
    /// Pause of the player in slot `by`.
    Pause { by: u8 },
    /// Resync of the state after a desync. Only the host's `Resume` can lift it, when all targets have reported
    /// `Synced`; a `Resume` from a client and [`Lockstep::resume`] have no effect. If a halt stood in the same bundle,
    /// the resync runs first and the match stands afterwards in this pause.
    Resync,
}

/// Events of a core for the driver.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    /// A machine has joined the session (host: every accepted client; client: itself after `Welcome`).
    Joined { peer: Peer, name: String },
    /// Start information of the own machine (host: from [`HostCore::start`]; client: from `Start`): load the map.
    Start(Box<StartInfo>),
    /// Everyone has loaded: tick 0 begins.
    Go,
    /// A machine has dropped out (host: at most once per peer; `slot` is `None` before the start).
    Lost {
        peer: Peer,
        slot: Option<u8>,
        reason: String,
    },
    /// A control entry from bundle `turn`; takes effect after its execution tick.
    Control { turn: u32, control: NetControl },
    /// Client (only as a target, once per resync): the host's state for the resync is there. The driver takes it over
    /// when it stands after `e(k)`, and then calls [`ClientCore::synced`].
    ResyncState { bytes: Vec<u8> },
    /// Host: send the state to these slots (reported when forming the bundle with `Resync`). The driver calls
    /// [`HostCore::send_state`] as soon as it stands after `e(k)`.
    SendState { slots: Vec<u8> },
    /// A halt has been lifted; the core is `Running` again. If a resync ends in a pause from the same bundle, it comes
    /// only when this pause is lifted.
    Resumed,
    /// The session has failed (text); afterwards the core is in `Phase::Ended`.
    Failed(String),
    /// The lobby state has changed (host: after the broadcast; client: new state from the host).
    LobbyChanged,
    /// Chat line of the lobby from row `row` with the name of that row (host and client).
    Chat { row: u8, name: String, text: String },
    /// Client: the host threw it out of the lobby (`Bye` with [`crate::msg::KICKED`]); afterwards `Phase::Ended`.
    Kicked,
    /// Client: the host left the lobby (`Bye` with [`crate::msg::HOST_LEFT`]); afterwards `Phase::Ended`.
    HostLeft,
    /// Client: the host rejected the sign-in (`Reject` before the `Welcome`); afterwards `Phase::Ended`.
    Rejected { reason: String },
    /// Client: the host dropped out after `Go`, and the successor list has a candidate. The phase stays; the driver
    /// chooses with [`Succession`] and calls [`ClientCore::rejoin`] and [`ClientCore::rehome`] or gives up with
    /// [`ClientCore::abandon`]. `planned` = a `Handover` came beforehand, `reason` is the raw reason for the log.
    HostGone { reason: String, planned: bool },
    /// Client: [`ClientCore::rehome`] has taken over the `Migrated` of the new host; the match continues with it.
    Rehomed,
    /// New host: collecting the `Rejoin` is finished, the match continues with it as host ([`HostCore::from_client`]).
    /// `lost` are the slots that left the migration - the old host and every expected client that did not come -,
    /// ordered by slot; their defeat is in the bundle `K + 1` (except for slots with a result). The driver reports them
    /// like a dropout (17200 + p).
    MigrationDone { lost: Vec<u8> },
    /// New host: the migration has failed because a bundle `≤ K` is missing at all of them. Every returnee has a
    /// `Reject` with [`crate::msg::MIGRATION_FAILED`] in its outbox, the phase is `Ended`, further `Rejoin` the core
    /// rejects with the same reason. The driver treats this like the dropout of the host without a successor: abort
    /// savegame from its own bundles (`NetFinish::HostLost`). `reason` for the log.
    MigrationFailed { reason: String },
}

/// Common interface of [`HostCore`] and [`ClientCore`].
pub trait Lockstep {
    /// An event of the connection to `from` (for the client always [`HOST_PEER`]).
    fn handle(&mut self, now_ms: u64, from: Peer, ev: LinkEvent);
    /// Time without an event (for deadlines).
    fn tick(&mut self, now_ms: u64);
    /// Silence of the connection to `peer` (for the client [`HOST_PEER`]) in ms since its last reception, also of a
    /// heartbeat. The driver reports it every frame for every connection as `link.age_ms() - link.last_received_ms()`
    /// (the same clock of the connection, with `MemLink` played time). From [`TIMEOUT_MS`] on, `peer` has dropped out.
    fn silence(&mut self, now_ms: u64, peer: Peer, silent_ms: u64);
    /// Own command; goes out with the next contribution. Before `Go` it has no effect (log).
    fn submit(&mut self, cmd: Command);
    /// Own wish (pause, game speed, saving); travels with the next contribution.
    fn wish(&mut self, w: Wish);
    /// Lifts the running pause if it is from the own slot (O-316); otherwise no effect (log).
    fn resume(&mut self);
    /// Closes the own contribution of turn `turn` with the checksum of the state before `b(turn)`. Called before the
    /// turn tick `b(turn)`, turns without gaps from 0. Before `Go` it has no effect (log).
    fn close_round(&mut self, turn: u32, hash: u64);
    /// The released bundle of turn `turn`, if it is already there.
    fn bundle(&self, turn: u32) -> Option<&TurnBundle>;
    /// Forgets the bundles before turn `turn`.
    fn forget_before(&mut self, turn: u32);
    /// Leaves the session in an orderly way: `Bye { reason }` to the peer(s) (client: to the host, host: to every
    /// active client), then `Phase::Ended` without an event. In `Phase::Ended` it has no effect. The driver closes the
    /// connections after sending.
    fn leave(&mut self, reason: String);
    /// Messages to send (receiver, message) in send order.
    fn outbox(&mut self) -> Vec<(Peer, NetMessage)>;
    /// Large contents to send (receiver, ID, bytes).
    fn large_outbox(&mut self) -> Vec<(Peer, u32, Vec<u8>)>;
    /// Events since the last collection.
    fn events(&mut self) -> Vec<CoreEvent>;
    /// Current phase.
    fn phase(&self) -> &Phase;
    /// Grid of the match (for the client the default until the `Start`).
    fn config(&self) -> TurnConfig;
}

/// Local time of the host (set by the driver, [`HostCore::set_clock`]; the core has no clock). It names the network
/// savegame on a stop and the desync report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl LocalTime {
    /// Replacement as long as the driver has not set a clock: 1970-01-01 00:00:00.
    pub const FALLBACK: LocalTime = LocalTime {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// Name of the network savegame on the end or abort of the match by the host: `netz_JJJJ_MM_TT_hhmm` (at the end
    /// local time of the host, at the abort that of the client).
    pub fn net_end_name(&self) -> String {
        format!(
            "netz_{:04}_{:02}_{:02}_{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }

    /// Name of the network savegame on the stop after the third desync: `checksumerror_JJJJ_MM_TT_hhmm` (pattern as
    /// `Game_SubmitSaveGameCommand` `0x004BEEA0` with `checksumerror_%04d_%02d_%02d_%02d%02d`, E-261).
    pub fn checksum_error_name(&self) -> String {
        format!(
            "checksumerror_{:04}_{:02}_{:02}_{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// Own inputs since the last contribution, in input order.
#[derive(Debug, Default)]
struct Inputs {
    commands: Vec<Command>,
    wishes: Vec<Wish>,
}

impl Inputs {
    /// Closes the collected inputs into the contribution of turn `turn`.
    fn close(&mut self, turn: u32, hash: u64) -> TurnContribution {
        TurnContribution {
            turn,
            hash,
            commands: std::mem::take(&mut self.commands),
            wishes: std::mem::take(&mut self.wishes),
        }
    }

    /// Discards waiting `Pause` wishes: a pause that was pressed before its halt is fulfilled by this halt (`Pause`
    /// during a pause has no effect).
    fn drop_pauses(&mut self) {
        let before = self.wishes.len();
        self.wishes.retain(|w| w != &Wish::Pause);
        if self.wishes.len() != before {
            log::info!("Network: pause wish during a pause discarded");
        }
    }
}

/// Role-dependent remainder of a control entry that [`BundleControls::apply`] does not handle itself.
enum RoleControl<'c> {
    /// `Resync { slots }`: the phase is already `Halted(Resync)`; the client checks whether its slot is a target.
    Resync(&'c [u8]),
    /// `Handover { peers }`.
    Handover(&'c [Peer]),
}

/// The state of a core that the control entries of an **adopted** bundle change: for the client the received bundle
/// (`ClientCore::apply_bundle`), for the new host a filled-in bundle of the margin (`HostCore::absorb`). Both treat
/// the entries the same; only `Resync` and `Handover` depend on the role. The host that forms a bundle itself has its
/// own handling (`HostCore::build_bundle`).
struct BundleControls<'a> {
    phase: &'a mut Phase,
    desyncs: &'a mut u32,
    pause_after: &'a mut Option<u8>,
    inputs: &'a mut Inputs,
    events: &'a mut Vec<CoreEvent>,
}

impl BundleControls<'_> {
    /// A control entry from bundle `turn`, in the order of the bundle (stop or end, resync, halt; takes effect after
    /// `e(turn)`, where the driver stops): report as `CoreEvent::Control`, count desyncs (one per `Resync`/`Stop`),
    /// `Stop` -> `Phase::Stopped`, `End` -> `Phase::Ended`; after a stop or end every further entry has no effect
    /// (log). `Resync` -> `Halted(Resync)`; a `Halt` during a resync counts only afterwards (`pause_after`), otherwise
    /// -> `Halted(Pause)`; both discard waiting `Pause` wishes. `who` names the core in the log. What the role still
    /// has to do is returned.
    fn apply<'c>(self, turn: u32, c: &'c NetControl, who: &str) -> Option<RoleControl<'c>> {
        self.events.push(CoreEvent::Control {
            turn,
            control: c.clone(),
        });
        if matches!(c, NetControl::Resync { .. } | NetControl::Stop { .. }) {
            *self.desyncs = self.desyncs.saturating_add(1);
        }
        match c {
            NetControl::Stop { .. } => *self.phase = Phase::Stopped,
            NetControl::End { .. } => *self.phase = Phase::Ended,
            _ if matches!(self.phase, Phase::Stopped | Phase::Ended) => {
                log::warn!("Network ({who}): {c:?} has no effect after stop or end");
            }
            NetControl::Resync { slots } => {
                *self.phase = Phase::Halted(HaltReason::Resync);
                return Some(RoleControl::Resync(slots));
            }
            NetControl::Halt { by } if *self.phase == Phase::Halted(HaltReason::Resync) => {
                // The resync runs first; the pause applies after the host's `Resume`.
                *self.pause_after = Some(*by);
                self.inputs.drop_pauses();
            }
            NetControl::Halt { by } => {
                *self.phase = Phase::Halted(HaltReason::Pause { by: *by });
                self.inputs.drop_pauses();
            }
            NetControl::Handover { peers } => return Some(RoleControl::Handover(peers)),
            NetControl::Speed(_) | NetControl::Save { .. } => {}
        }
        None
    }
}

/// State of a client for the new host: `ClientCore::into_parts` hands it to `HostCore::from_client`, which does not
/// see the private fields of the client. Private in the module `lockstep`: visible for both cores and their
/// submodules, not outside.
struct ClientParts {
    /// Own sign-in.
    hello: Hello,
    /// Own connection number from `Welcome`.
    peer: Option<Peer>,
    /// Own slot from `Start`.
    slot: Option<u8>,
    /// Grid of the match.
    cfg: TurnConfig,
    phase: Phase,
    /// Inputs since the last contribution.
    inputs: Inputs,
    next_close: u32,
    next_bundle: u32,
    /// Retained bundles.
    bundles: BTreeMap<u32, TurnBundle>,
    /// Own contributions whose bundle is still missing (turn >= `next_bundle`), by turn.
    sent: Vec<TurnContribution>,
    /// Last lobby state from the host (source of the frozen state).
    lobby: Option<LobbyState>,
    /// Pause from the same bundle as a running resync (slot of the pausing player).
    pause_after: Option<u8>,
    /// A resync was running (it lapses with the migration).
    resync: bool,
    desyncs: u32,
    /// Successor list from `Go` or the last `Migrated`.
    successors: Vec<Successor>,
    /// Peers from a `Handover`, if one came.
    handover: Option<Vec<Peer>>,
}

/// Dropout command of the slot `slot`: defeat with the slot itself as sender, like `NetSession_OnClientLost`
/// `0x004E9C70` (`0x50000056 {slot, 1}`).
fn loss_command(slot: u8) -> Command {
    Command {
        player: slot,
        kind: opennml_sim::command::CommandKind::SetPlayerResult { slot, result: 1 },
    }
}

/// Reason for a dropout after [`TIMEOUT_MS`] of silence.
fn timeout_reason() -> String {
    format!("Timeout: {} s without a message", TIMEOUT_MS / 1000)
}
