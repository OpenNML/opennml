//! Simulation in the viewer: converts the virtual clock to game time according to `GameSpeed`, runs at most two
//! ticks per frame (remainder discarded), collects commands until the next tick boundary, records them for replays,
//! keeps checksums and the previous state for interpolation; savegames, replays and queries for the control
//! channel. Without winit and GPU (testable).

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Context, bail};
use glam::Vec3;
use opennml_control::SimAttackReport;
use opennml_control::{
    SimAreaView, SimCounterUpgrade, SimDivingInfo, SimFlagInfo, SimFlagsInfo, SimFormationInfo,
    SimGroupInfo, SimGroupTarget, SimHashResult, SimLookInfo, SimModuleInfo, SimMultiPartInfo,
    SimNotice, SimObjectInfo, SimPlayerInfo, SimQueryResult, SimRidingInfo, SimSection,
    SimTempUpgrade, SimTentInfo, SimTradeInfo, SimTrainInfo, SimTransportTentInfo, SimWalkInfo,
    hex_checksum,
};
use opennml_render::EntryLook;
use opennml_sim::command::Outcome;
use opennml_sim::command::{Command, CommandKind};
use opennml_sim::economy::resource;
use opennml_sim::fog::FogView;
use opennml_sim::fx::{Fx, FxVec3, fixed_mul};
use opennml_sim::id::ObjectId;
use opennml_sim::module::tent::{self, transport};
use opennml_sim::module::train::{self, Train};
use opennml_sim::module::walk::Walk;
use opennml_sim::module::{
    MODULE_NAMES, ModuleKind, building, diving, masking, multipart, riding, trading,
};
use opennml_sim::motion::NO_DIRECTION;
use opennml_sim::object::{GameObject, status};
use opennml_sim::random_map::RandomMapRules;
use opennml_sim::rules::Rules;
use opennml_sim::save::{MAX_REPLAY_STEPS, RecordedCommand, Replay, SaveGame, checksum};
use opennml_sim::script::ui_mirror;
use opennml_sim::setup::{MatchSetup, new_match, start_package};
use opennml_sim::state::{Control, SimEvent, SimState};
use opennml_sim::tick::{OBJECT_TICK_MS, STEP_MS, step};
use opennml_world::classes::in_no_shadow_area;
use opennml_world::level::{LevelScene, ObjectClass, PlacedObject};
use opennml_world::markers::player_rgb;
use opennml_world::math::object_world_matrix;

use crate::game::formation_order::FormationOrder;
use crate::game::net_driver::NetState;

/// At most this many ticks per frame (`GameEngine_Update`).
pub const MAX_STEPS_PER_FRAME: u32 = 2;
/// This many checksums are kept for display and control channel.
pub const HASH_HISTORY: usize = 64;
/// This many messages per player are kept for the control channel (section `sim`).
pub const MESSAGE_HISTORY: usize = 16;

/// Opacity of a diver (`DIVING`): `GameObject_SetRenderAlpha(0.6)` (`0x00468090`,
/// `PropertyDiving_DiveDown`, re/swim-dive 3.3).
pub const DIVER_ALPHA: f32 = 0.6;

/// Opacity of a camouflaged unit and of the alarm post for viewers with sight:
/// `GameObject_SetRenderAlpha(0.5)` (`0x00468090`, re/masking 7).
pub const MASKED_ALPHA: f32 = 0.5;

/// Sinking of a destroyed building (display only): `PropertyBuilding_Destroy` (`0x00506300`)
/// sets two timers `0x3B03 {speed, duration}` - after 100 ms `{-800, 300}`, after 1300 ms
/// `{-200, 2000}`; `PropertyBuilding_OnMsgStartSink` (`0x00507200`) remembers the position, and
/// `PropertyBuilding_FrameUpdate` (`0x005066B0`) sets `y = start + speed * t^2` per frame (t in
/// seconds, at most duration) while the building is dead (`+0x360` = `m_IsDead`;
/// `PropertyBuilding_StartBuild` `0x00504840` clears it and snaps the position back onto the grid). The
/// second phase begins at the end position of the first. Columns: delay, speed, duration.
pub const SINK_PHASES: [(f64, f32, f64); 2] = [(100.0, -800.0, 300.0), (1300.0, -200.0, 2000.0)];

/// Height offset of a destroyed building for the timer clock `clock_ms` ([`SimHost::sink_clock`]):
/// a phase begins when its timer fires - in the first object tick (100 ms) in which the clock
/// **exceeds** the delay (`Module_UpdateEvents`), i.e. at 200 or 1400 ms. The
/// timers run with game time; the original computes the frame with frame time, here with
/// game time so that frames are reproducible while the clock is halted. Deviation: the original
/// updates the display only for visible buildings.
pub fn sink_offset(clock_ms: f64) -> f32 {
    let tick = f64::from(OBJECT_TICK_MS);
    SINK_PHASES
        .iter()
        .map(|&(delay, speed, duration)| {
            let fired = ((delay / tick).floor() + 1.0) * tick;
            let t = ((clock_ms - fired).clamp(0.0, duration) * 0.001) as f32;
            speed * t * t
        })
        .sum()
}

/// Counter of the AI commands from the events `SimEvent::KiCommand`: host state,
/// not part of the simulation; cleared on restart, loading and replaying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KiStats {
    /// (slot, network command, reason) -> number of rejected commands (`Outcome::Rejected`).
    pub rejected: BTreeMap<(u8, u32, String), u64>,
    /// (slot, network command, result `accepted`/`partial`/`rejected`) -> number of commands.
    pub commands: BTreeMap<(u8, u32, &'static str), u64>,
}

impl KiStats {
    /// Counts the AI commands of a tick.
    pub fn record(&mut self, events: &[SimEvent]) {
        for event in events {
            let SimEvent::KiCommand {
                player,
                code,
                outcome,
            } = event
            else {
                continue;
            };
            let kind = match outcome {
                Outcome::Accepted => "accepted",
                Outcome::Partial(_) => "partial",
                Outcome::Rejected(why) => {
                    *self
                        .rejected
                        .entry((*player, *code, why.clone()))
                        .or_default() += 1;
                    "rejected"
                }
            };
            *self.commands.entry((*player, *code, kind)).or_default() += 1;
        }
    }
}

/// Steps whose script messages the host collects (`script_state` `log` at most this many).
pub const SCRIPT_LOG_STEPS: u64 = opennml_control::MAX_SCRIPT_LOG_STEPS as u64;

/// Block log for `script_state` and the "Script" window: the messages
/// `SimEvent::Script` of the last [`SCRIPT_LOG_STEPS`] steps as a ring, each with the step (1-based,
/// `SimState::steps` after the step). Host state, not part of the simulation; cleared on restart, loading and
/// replaying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptLog {
    pub entries: VecDeque<(u64, opennml_sim::script::ScriptTrace)>,
}

impl ScriptLog {
    /// Records the script messages of step `step` and discards those older than
    /// [`SCRIPT_LOG_STEPS`] steps.
    pub fn record(&mut self, step: u64, events: &[SimEvent]) {
        for event in events {
            if let SimEvent::Script(trace) = event {
                self.entries.push_back((step, trace.clone()));
            }
        }
        while self
            .entries
            .front()
            .is_some_and(|(s, _)| s.saturating_add(SCRIPT_LOG_STEPS) <= step)
        {
            self.entries.pop_front();
        }
    }

    /// The messages of the last `steps` steps up to `now` (inclusive), oldest first.
    pub fn last(
        &self,
        now: u64,
        steps: u64,
    ) -> impl Iterator<Item = &(u64, opennml_sim::script::ScriptTrace)> {
        // If `s + steps` overflows (ticks from a savegame up to `u64::MAX`), the end lies beyond `now`.
        self.entries
            .iter()
            .filter(move |(s, _)| s.checked_add(steps).is_none_or(|end| end > now))
    }
}

/// This many presentation events at most are kept by [`PresentLog`] (besides the limit [`SCRIPT_LOG_STEPS`]).
pub const PRESENT_LOG_EVENTS: usize = 4096;

/// Presentation events for `script_events` and the "Script" window: the `SimEvent::Present` of the
/// last [`SCRIPT_LOG_STEPS`] steps, at most [`PRESENT_LOG_EVENTS`], each with the step (1-based like
/// [`ScriptLog`]). Host state, not part of the simulation; cleared on restart, loading and replaying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentLog {
    pub entries: VecDeque<(u64, opennml_sim::script::Presentation)>,
}

impl PresentLog {
    /// Records the presentation events of step `step` and discards those older than [`SCRIPT_LOG_STEPS`] steps
    /// or, beyond [`PRESENT_LOG_EVENTS`], the oldest ones.
    pub fn record(&mut self, step: u64, events: &[SimEvent]) {
        for event in events {
            if let SimEvent::Present(p) = event {
                self.entries.push_back((step, p.clone()));
            }
        }
        while self.entries.len() > PRESENT_LOG_EVENTS
            || self
                .entries
                .front()
                .is_some_and(|(s, _)| s.saturating_add(SCRIPT_LOG_STEPS) <= step)
        {
            self.entries.pop_front();
        }
    }

    /// The events of the last `steps` steps up to `now` (inclusive), oldest first.
    pub fn last(
        &self,
        now: u64,
        steps: u64,
    ) -> impl Iterator<Item = &(u64, opennml_sim::script::Presentation)> {
        // If `s + steps` overflows (ticks from a savegame up to `u64::MAX`), the end lies beyond `now`.
        self.entries
            .iter()
            .filter(move |(s, _)| s.checked_add(steps).is_none_or(|end| end > now))
    }
}

/// This many rejections of script commands are kept with their reason ([`ScriptCommandStats`]).
pub const SCRIPT_REJECTIONS_KEPT: usize = 20;

/// Counter of the script commands for `script_state` and the "Script" window: per
/// action number, how many of its commands (`ScriptTrace::Command`, script path) were accepted completely, accepted
/// partially and rejected, plus the last [`SCRIPT_REJECTIONS_KEPT`] rejections with reason. Host state
/// like [`ScriptLog`], not part of the simulation; cleared on restart, loading and replaying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptCommandStats {
    /// Action -> (accepted, partial, rejected).
    counts: BTreeMap<i32, (u64, u64, u64)>,
    rejections: VecDeque<opennml_control::ScriptRejection>,
}

impl ScriptCommandStats {
    /// Records the script commands of step `step` (1-based like [`ScriptLog::record`]).
    pub fn record(&mut self, step: u64, events: &[SimEvent]) {
        for event in events {
            let SimEvent::Script(opennml_sim::script::ScriptTrace::Command {
                container,
                block,
                action,
                code,
                outcome,
            }) = event
            else {
                continue;
            };
            let entry = self.counts.entry(*action).or_default();
            match outcome {
                Outcome::Accepted => entry.0 += 1,
                Outcome::Partial(_) => entry.1 += 1,
                Outcome::Rejected(why) => {
                    entry.2 += 1;
                    if self.rejections.len() == SCRIPT_REJECTIONS_KEPT {
                        self.rejections.pop_front();
                    }
                    self.rejections.push_back(opennml_control::ScriptRejection {
                        step,
                        container: *container,
                        block: *block,
                        action: *action,
                        code: *code,
                        reason: why.clone(),
                    });
                }
            }
        }
    }

    /// The response for `script_state`: counters ascending by action number, rejections oldest first.
    pub fn info(&self) -> opennml_control::ScriptCommandsInfo {
        opennml_control::ScriptCommandsInfo {
            actions: self
                .counts
                .iter()
                .map(|(&action, &(accepted, partial, rejected))| {
                    opennml_control::ScriptActionCount {
                        action,
                        accepted,
                        partial,
                        rejected,
                    }
                })
                .collect(),
            rejections: self.rejections.iter().cloned().collect(),
        }
    }
}

/// The height of a walk object last shown with `MOVE`, together with the grid position at which it
/// arose (`SimHost::track_shown_heights`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ShownHeight {
    /// Grid x, y, z (fixed point); a different position supersedes the height (step or snap).
    grid: [i32; 3],
    /// Shown height (world units).
    y: f32,
    /// `MOVE` was set on the last pass; the first pass after that (the halt) adopts
    /// the grid position of the halt.
    moving: bool,
}

/// Response of the turn driver before a tick ([`SimHost::advance_rounds`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RoundStep {
    /// Compute the tick with these commands (empty outside the execution ticks).
    Step(Vec<Command>),
    /// Do not compute (bundle missing, halt): the game time remainder lapses, the frame is over.
    Wait,
}

/// Turn driver for [`SimHost::advance_rounds`] (`crate::game::net_driver::NetDriver`).
pub trait Rounds {
    /// Before each tick: close the turn contribution (at the turn tick), commands of the bundle (at the execution
    /// tick) or wait.
    fn before_step(&mut self, sim: &mut SimHost) -> RoundStep;
    /// After each computed tick (control entries after the execution tick, results to the core).
    fn after_step(&mut self, sim: &mut SimHost);
}

/// Replay of a recording in the host ([`SimHost::begin_playback`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Playback {
    /// Commands of the file in file order (ordered by tick as recorded; unchecked, a command that is too early comes
    /// along in the next tick, [`SimHost::take_playback_step`]).
    commands: Vec<RecordedCommand>,
    /// Index of the next command in `commands`.
    next: usize,
    /// View slot (`first_player`, the host for a network recording): only its giving up goes through.
    view: u8,
    /// How often the end of the file was reported (exactly once).
    end_notices: u32,
}

/// State of the replay for flow and control channel ([`SimHost::playback`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackStatus {
    /// Tick of the next command of the file; `None` after the last one.
    pub next_step: Option<u64>,
    /// All commands of the file have been executed (end of file).
    pub ended: bool,
}

pub struct SimHost {
    pub rules: Rules,
    pub setup: MatchSetup,
    pub state: SimState,
    /// World position of the ticked objects before the last tick (interpolation).
    previous: BTreeMap<ObjectId, [f32; 3]>,
    /// Height last shown with `MOVE` per walk object (`track_shown_heights`): the original
    /// has only **one** `world_pos`, which `PropertyWalk_FrameUpdate` overwrites with `MOVE` and which
    /// stays after the halt (`walk_display`).
    shown_heights: BTreeMap<ObjectId, ShownHeight>,
    /// Game time that has not yet yielded a tick (ms).
    carry_ms: f64,
    pub game_speed: u8,
    /// The match is currently not ticking (pause `Engine+0x24E4` or a halted match such as the open game menu):
    /// [`SimHost::submit`] then executes immediately (O-318). Writer:
    /// `GameSession::advance` per frame and when toggling the pause, the fake of the tests likewise; the viewer leaves
    /// it out (its time stands still only for checking, commands wait there for the next step).
    pub halted: bool,
    /// Events of the immediately executed commands ([`Self::execute_now`]) since the last tick that the pause frame
    /// has not yet fetched ([`Self::take_immediate_events`]). They also stay in `state.events` and come out once more
    /// with the next tick; the tick therefore empties this buffer.
    immediate_events: Vec<SimEvent>,
    /// Commands for the next tick boundary.
    pending: Vec<Command>,
    /// Turn mode: the host computes ticks only via [`Self::advance_rounds`] with the commands of the turn bundles.
    /// Own commands ([`Self::submit`], [`Self::execute_now`], [`Self::flush_pending`])
    /// go to `outgoing` into the next turn contribution ([`Self::take_outgoing`]), only with the own slot as
    /// sender; the slot switch is off. Set by [`Self::enter_rounds`].
    pub net: bool,
    /// In turn mode: this machine is the host. Only it sends the mirror commands `ScriptSelection`/`ScriptButton`
    /// (they mirror the UI of the host in the network).
    pub net_host: bool,
    /// Turn mode: own commands for the next turn contribution, in input order.
    outgoing: Vec<Command>,
    /// Turn mode: selection of the last `ScriptSelection` issued. Until its execution tick it is
    /// in transit and the mirror in the state is still old; [`Self::sync_selection`] therefore compares against this.
    mirror_sent: Option<Vec<ObjectId>>,
    /// All commands executed since the start (replay).
    pub log: Vec<RecordedCommand>,
    /// After loading a savegame the commands before it are missing: no replay.
    log_complete: bool,
    /// (tick, checksum) of the last ticks.
    pub hashes: VecDeque<(u64, u64)>,
    /// Player whose view, selection and commands apply (the host knows it, the simulation does not).
    pub active_player: u8,
    /// Fog view of the active player (alpha table, "Fog off"; re/vision 8).
    pub fog: crate::game::fog_view::FogDisplay,
    /// Fog memories of the active player (re/vision 9).
    pub memories: crate::game::fog_memory::FogMemories,
    /// Last messages (`SimEvent::PlayerMessage`, from protocol 22 also `SimEvent::TextMessage`)
    /// per player slot, oldest first, without text (the application sets it from `texte.deu`).
    messages: BTreeMap<u8, VecDeque<SimNotice>>,
    /// Combat points per target owner, as last reported (`SimEvent::AttackPoints`, every
    /// double tick). The value stays until the next event, also during the pause: the
    /// original passes the music state machine the last set value per frame (C-161).
    pub attack_points: [i32; 8],
    /// Texts of the researches and counter-upgrades from `guids.deu` (no types, no name in the
    /// rules), set at the start of the simulation ([`start_sim`],
    /// [`crate::game::texts::upgrade_texts`]); empty without the file.
    pub upgrade_texts: BTreeMap<u32, String>,
    /// Counter of the AI commands (`ki_state`); host state.
    pub ki_stats: KiStats,
    /// Script messages of the last [`SCRIPT_LOG_STEPS`] steps (`script_state`); host state.
    pub script_log: ScriptLog,
    /// Presentation events of the last [`SCRIPT_LOG_STEPS`] steps (`script_events`, "Script" window);
    /// host state.
    pub present_log: PresentLog,
    /// Counter of the script commands (`script_state` `commands`, "Script" window); host state.
    pub script_commands: ScriptCommandStats,
    /// Counter of the idle ticks of the fog cycle (steps that began with `fog.processed == -1`; `fog_step` sets
    /// -2 then): in exactly this branch the original calls the minimap (re/vision 3.2,
    /// O-287). Does not advance without a step; host state, neither in the checksum nor in the savegame.
    fog_idle_ticks: u32,
    /// Counter of the immediate rebuilds of the minimap (`GameWorld_UpdateFogVertexAlpha(1)` `0x00479160`: level start,
    /// loading, player switch, end of the cinemascope); host state like [`Self::fog_idle_ticks`].
    fog_rebuilds: u32,
    /// Replay mode: the commands come from the recording, the input only passes on the giving up of the view slot.
    /// Host state, neither in the checksum nor in the savegame.
    playback: Option<Playback>,
}

/// The IDs sorted (comparison as a set in [`SimHost::sync_selection`]).
/// Flag query of the control channel: `state.sim.flags` in the viewer
/// ([`SimHost::section`]) and `ui_state.game.flags` in `opennml-game` (`GameFlow::game_data`) - the same function.
/// Set in game type 2 (Domination) and 3 (King of the Hill) or as soon as a player flag (GUID 4300) is alive,
/// otherwise `None`. Read-only: the checksum stays the same.
///
/// - `flags`: the living player flags in object order (slot, then index; `SimState::objects`);
///   `holder` = first slot with the flag color, otherwise 8 (`FUN_00478DE0`, [`opennml_sim::flags::slot_of_color`]);
///   `check_ms` = check counter of nature (`m_Timer`, -1 off).
/// - `counts`/`leader` = `m_FlagsCounter` and `m_FlagOwner` (`flag_counts`, `flag_leader`), `time_limit_s` =
///   `m_TimeLimit` in seconds, `remaining_ms` = remaining time of the leader timer (ID 0), `None` without.
pub fn flags_info(state: &SimState) -> Option<SimFlagsInfo> {
    let flags: Vec<SimFlagInfo> = state
        .objects()
        .filter(|o| o.guid == opennml_sim::setup::PLAYER_FLAG && o.status & status::DIED == 0)
        .map(|o| SimFlagInfo {
            id: o.id.0,
            tile: [o.tile.0, o.tile.1],
            color: o.color,
            holder: opennml_sim::flags::slot_of_color(state, o.color),
            check_ms: match o.module(opennml_sim::module::natur::INDEX).map(|m| &m.kind) {
                Some(ModuleKind::Natur(n)) => n.timer,
                _ => -1,
            },
        })
        .collect();
    if !matches!(state.mode, 2 | 3) && flags.is_empty() {
        return None;
    }
    Some(SimFlagsInfo {
        flags,
        counts: state.flag_counts,
        leader: state.flag_leader,
        time_limit_s: state.time_limit,
        remaining_ms: state
            .countdowns
            .get(&opennml_sim::flags::LEADER_COUNTDOWN)
            .copied(),
    })
}

fn sorted(ids: &[ObjectId]) -> Vec<ObjectId> {
    let mut v = ids.to_vec();
    v.sort_unstable();
    v
}

/// Status bits for which `PropertyWalk_FrameUpdate` (`0x00538BA0`) does not take the height from the
/// terrain: wall, diving, occupant in a boat.
const NO_TERRAIN: u32 = status::ONWALL | status::DIVING | status::ISMOUNTEDONBOAT;

/// Does the unit stand at least at water height? Walk flag `0x2000` (with `0x20000` or without
/// `0x800`) or `SWIMMING`, never with [`NO_TERRAIN`]. Common height rule of the rendering
/// (`walk_frame`) and the helper lines (remaining path).
pub(crate) fn clamps_to_water(o: &GameObject) -> bool {
    let Some(m) = o.module(0) else {
        return false;
    };
    let swims = m.base.has_flags(0x2000) && (m.base.has_flags(0x20000) || !m.base.has_flags(0x800));
    o.status & NO_TERRAIN == 0 && (swims || o.status & status::SWIMMING != 0)
}

/// Slots whose special list `L[p]` the rendering draws (`GameWorld_BuildRenderListsAndFrameUpdate`: slots 0-8).
const DRAWN_INACTIVE_LISTS: usize = 9;

/// Not drawn (`GameWorld_BuildRenderListsAndFrameUpdate` `0x0047FC70`): from the sight cells only objects
/// of a running owner slot (`GameWorld+0x62C`), of hidden slots 0-8 only the special list `L[p]`
/// (actions 159/160/161/162, re/script-actions 2.5). Without an entry in the render lists also no shadow and
/// no click target.
pub fn hidden_from_view(state: &SimState, o: &GameObject) -> bool {
    let p = usize::from(o.owner());
    p < opennml_sim::id::SLOT_COUNT
        && !state.slot_running[p]
        && !(p < DRAWN_INACTIVE_LISTS && state.inactive_lists[p].contains(&o.id))
}

impl SimHost {
    pub fn new(rules: Rules, setup: MatchSetup, game_speed: u8) -> Result<Self, String> {
        let state = new_match(&rules, &setup)?;
        let mut host = Self {
            rules,
            setup,
            state,
            previous: BTreeMap::new(),
            shown_heights: BTreeMap::new(),
            carry_ms: 0.0,
            game_speed,
            halted: false,
            immediate_events: Vec::new(),
            pending: Vec::new(),
            net: false,
            net_host: false,
            outgoing: Vec::new(),
            mirror_sent: None,
            log: Vec::new(),
            log_complete: true,
            hashes: VecDeque::new(),
            active_player: 0,
            fog: crate::game::fog_view::FogDisplay::new(0, 0),
            memories: crate::game::fog_memory::FogMemories::default(),
            messages: BTreeMap::new(),
            attack_points: [0; 8],
            upgrade_texts: BTreeMap::new(),
            ki_stats: KiStats::default(),
            script_log: ScriptLog::default(),
            present_log: PresentLog::default(),
            script_commands: ScriptCommandStats::default(),
            fog_idle_ticks: 0,
            fog_rebuilds: 0,
            playback: None,
        };
        host.active_player = host.first_player().unwrap_or(0);
        host.reset_fog(true);
        host.record_hash();
        Ok(host)
    }

    /// First occupied player slot (human before AI).
    fn first_player(&self) -> Option<u8> {
        let players = &self.state.players;
        (0..8u8)
            .find(|&p| players[usize::from(p)].control == Control::Human)
            .or_else(|| (0..8u8).find(|&p| players[usize::from(p)].control != Control::Empty))
    }

    /// New fog view in map size with the switch and overlay of the old one; target values immediately
    /// (level start `GameEngine_InitLevelView` `0x004458B0`, likewise loading and restart). The
    /// fog memories are rebuilt: `preplaced` (match start) with a snapshot for every
    /// object, otherwise (loading) only on explored positions.
    fn reset_fog(&mut self, preplaced: bool) {
        let mut fog =
            crate::game::fog_view::FogDisplay::new(self.rules.map.dim_x, self.rules.map.dim_z);
        fog.show_mode = self.fog.show_mode;
        fog.overlay = self.fog.overlay;
        fog.update_targets(&self.state, self.active_player, true);
        self.fog = fog;
        self.request_minimap_rebuild();
        self.memories.rebuild(
            &self.state,
            &self.rules.map,
            &self.fog.view(&self.state),
            self.active_player,
            preplaced,
        );
    }

    /// Counter of the idle ticks of the fog cycle (see [`Self::fog_idle_ticks`]); the minimap writes its fog rows
    /// per count step (`Minimap::sync`).
    pub fn fog_idle_ticks(&self) -> u32 {
        self.fog_idle_ticks
    }

    /// Counter of the immediate rebuilds of the minimap (see [`Self::fog_rebuilds`]).
    pub fn fog_rebuilds(&self) -> u32 {
        self.fog_rebuilds
    }

    /// Reports an immediate rebuild of the fog to the minimap (`0x00479160(1)`). The places of the host where the
    /// original sets the alpha table immediately call it: level start and loading, player switch and the end of the
    /// cinemascope (`Presentation::Cinemascope { on: false }`).
    pub fn request_minimap_rebuild(&mut self) {
        self.fog_rebuilds = self.fog_rebuilds.wrapping_add(1);
    }

    /// Checks the fog memories of the active player (`0x00416230`) against the view switch
    /// of the host: after every step ([`Self::step_once`]), after an immediate command, on player switch and on
    /// toggling "Fog off". All fog memories of the map, not only the sectors of the camera +-1 like the original
    /// (`0x00416660`; re/vision, section "Deviations", item 4); measured at most ~0.6 ms per call (release, about
    /// 3,300 fog memories on `[2players]-crossrail`).
    pub fn update_memories(&mut self) {
        self.memories.update(
            &self.state,
            &self.rules.map,
            &self.fog.view(&self.state),
            self.active_player,
        );
    }

    /// Next occupied player slot in direction `dir` (+-1); without occupied slots it stays.
    /// The brightness follows immediately, without crossfade (`GameWorld_CmdSwitchLocalPlayer`
    /// `0x0047B510`).
    pub fn switch_player(&mut self, dir: i32) {
        if self.net {
            log::info!("Network: slot switch blocked in the network match");
            return;
        }
        for k in 1..=8 {
            let p = (i32::from(self.active_player) + dir * k).rem_euclid(8) as u8;
            if self.state.players[usize::from(p)].control != Control::Empty {
                self.activate(p);
                return;
            }
        }
    }

    /// `sim_view active_player`: like [`Self::switch_player`], but directly to slot `slot`;
    /// an empty slot is an error.
    pub fn set_active_player(&mut self, slot: u8) -> Result<(), String> {
        if slot > 7 {
            return Err(format!("slot {slot} invalid: allowed 0 to 7"));
        }
        if self.state.players[usize::from(slot)].control == Control::Empty {
            return Err(format!("slot {slot} is not occupied"));
        }
        if self.net && slot != self.active_player {
            log::info!("Network: switch to slot {slot} blocked in the network match");
            return Err(format!(
                "slot switch blocked in the network match (own slot {})",
                self.active_player
            ));
        }
        self.activate(slot);
        Ok(())
    }

    /// Switches on turn mode: own slot `slot` (a human slot) as local player, `host` = this machine is the
    /// host. Commands already waiting go into the first contribution. After that [`Self::net`] and
    /// [`Self::net_host`] apply; there is no way back (the match ends in the network).
    pub fn enter_rounds(&mut self, slot: u8, host: bool) -> Result<(), String> {
        if slot > 7 {
            return Err(format!("slot {slot} invalid: allowed 0 to 7"));
        }
        if self.state.players[usize::from(slot)].control != Control::Human {
            return Err(format!("slot {slot} is not a human slot"));
        }
        self.activate(slot);
        self.net = true;
        self.net_host = host;
        for command in std::mem::take(&mut self.pending) {
            self.push_outgoing(command);
        }
        Ok(())
    }

    /// Updates the role in turn mode: if a client becomes host after a host migration
    /// (`NetFrame::became_host`), from now on it sends the mirror commands `ScriptSelection`/`ScriptButton`.
    /// Without turn mode it has no effect (log).
    pub fn set_net_host(&mut self, host: bool) {
        if !self.net {
            log::warn!("Network: role without turn mode discarded");
            return;
        }
        self.net_host = host;
    }

    /// Turn mode: own command into the next contribution. A command with a foreign sender is discarded
    /// (every machine submits its own commands only with its own slot; the host would discard it from a client
    /// likewise).
    fn push_outgoing(&mut self, command: Command) {
        if command.player != self.active_player {
            log::warn!(
                "Network: command with sender {} instead of own slot {} discarded: {:?}",
                command.player,
                self.active_player,
                command.kind
            );
            return;
        }
        self.outgoing.push(command);
    }

    /// Turn mode: fetches the own commands since the last call (for the contribution before the turn tick).
    pub fn take_outgoing(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.outgoing)
    }

    /// Switch viewer: target values of the brightness immediately, check the fog memories.
    fn activate(&mut self, slot: u8) {
        self.active_player = slot;
        self.fog
            .update_targets(&self.state, self.active_player, true);
        self.request_minimap_rebuild();
        self.update_memories();
    }

    /// "Fog off" (`on = false`, `m_FogShowMode` [`crate::game::fog_view::FOG_OFF`]) or on again
    /// (0), then check the fog memories with the new switch. The alpha table stays as it is
    /// (original); with fog on it crossfades from the next update.
    pub fn set_fog_shown(&mut self, on: bool) {
        self.set_fog_show_mode(if on {
            0
        } else {
            crate::game::fog_view::FOG_OFF
        });
    }

    /// Sets `m_FogShowMode` to `mode` (resuming a savegame, `HostPart::fog_show_mode`), then checks the
    /// fog memories with the new switch like [`Self::set_fog_shown`].
    pub fn set_fog_show_mode(&mut self, mode: u8) {
        self.fog.show_mode = mode;
        self.update_memories();
    }

    /// Command for the next tick boundary; if the match is not ticking ([`SimHost::halted`]), immediately via
    /// [`SimHost::execute_now`]. In single player the original executes every command immediately
    /// (`NetSession_SubmitMessage` `0x004E9B60` without a session -> `GameWorld_ExecuteCommand(msg, 1, 1)`, O-318); the
    /// reimplementation attaches running commands to the tick boundary (a deviation) and takes the immediate path
    /// only if there are none. The replay stays the same.
    ///
    /// In turn mode ([`Self::net`]) every command goes into the next turn contribution, also with a halted match.
    /// In replay mode only the giving up of the view slot ([`Self::begin_playback`]).
    pub fn submit(&mut self, command: Command) {
        if self.playback_refuses(&command) {
            return;
        }
        if self.net {
            self.push_outgoing(command);
            return;
        }
        if self.halted {
            self.execute_now(command);
        } else {
            self.pending.push(command);
        }
    }

    /// Does a level script run in this match (setup with script and loaded level script)? Then the
    /// fixed start tables of light, weather and music do not apply.
    pub fn runs_level_script(&self) -> bool {
        self.setup.script.enabled && self.rules.scripts.level.is_some()
    }

    /// Does the running script report the UI (conditions 35-38)? Only then do [`Self::sync_selection`]
    /// and [`Self::press_button`] send commands: in matches without a script or without 35-38 the mirror would be
    /// command noise and would change the checksum.
    fn ui_conditions_active(&self) -> bool {
        self.setup.script.enabled && self.rules.scripts.uses_ui_conditions
    }

    /// Syncs the selection of the host with the mirror of the simulation (conditions 35-37; deviation from the
    /// original): sends the host command `ScriptSelection` if `ids`, as a **set**, differs from the last queued
    /// `ScriptSelection` (otherwise from the mirror in the state). One call per frame is enough
    /// ([`crate::game_session::GameSession::advance_by`]); selection changes without a message of their own (key
    /// shortcuts, headhunter, replacing when mounting) it syncs too. It truncates to 30 entries (the command
    /// would otherwise be rejected every frame). After loading (`reset_to` empties `pending`) it reports
    /// the selection anew in the first frame.
    ///
    /// In replay mode nothing: the mirror commands come from the recording, the live
    /// selection gives nothing into the simulation.
    pub fn sync_selection(&mut self, ids: &[ObjectId]) {
        if self.playback.is_some() || !self.ui_conditions_active() || (self.net && !self.net_host) {
            return;
        }
        let ids = &ids[..ids.len().min(ui_mirror::MAX_SELECTION)];
        if self.net {
            // Turn mode: via the contribution; compared with the last state issued.
            let known = self
                .mirror_sent
                .as_deref()
                .unwrap_or(&self.state.script.mission.selection);
            if sorted(known) == sorted(ids) {
                return;
            }
            self.mirror_sent = Some(ids.to_vec());
            self.push_outgoing(Command {
                player: self.active_player,
                kind: CommandKind::ScriptSelection { ids: ids.to_vec() },
            });
            return;
        }
        let last = self.pending.iter().rev().find_map(|c| match &c.kind {
            CommandKind::ScriptSelection { ids } => Some(ids.as_slice()),
            _ => None,
        });
        let known = last.unwrap_or(&self.state.script.mission.selection);
        if sorted(known) == sorted(ids) {
            return;
        }
        self.pending.push(Command {
            player: self.active_player,
            kind: CommandKind::ScriptSelection { ids: ids.to_vec() },
        });
    }

    /// Reports a command bar event (word 1 of `0x00458130`, O-246/E-229) as host command `ScriptButton`
    /// (condition 38), only with a running script with 35-38. Every report arrives, also the same value once
    /// more. The bar calls it via `GameSession::button_pressed`; until then only the control channel.
    /// In replay mode nothing (the mirror comes from the recording).
    pub fn press_button(&mut self, word: i32) {
        // In the network only the host; `submit` puts the command into the contribution there.
        if self.playback.is_some() || !self.ui_conditions_active() || (self.net && !self.net_host) {
            return;
        }
        // Via `submit` like every command: in the pause or with a halted match immediately (O-318).
        self.submit(Command {
            player: self.active_player,
            kind: CommandKind::ScriptButton { word },
        });
    }

    /// Executes the waiting commands in order immediately ([`Self::execute_now`]; O-318). The caller is
    /// the switching on of the pause ([`crate::game_session::GameSession::toggle_pause`], also via loss of focus):
    /// a command given shortly before would otherwise only be due after resuming and thus **after** the commands
    /// executed immediately in the pause, and a savegame of the pause would not contain it
    /// ([`Self::save`] does not save waiting commands). The original knows no waiting commands (every command
    /// takes effect immediately). Only this path empties the queue early; the open game menu leaves it as it is.
    ///
    /// In turn mode the waiting commands go into the next turn contribution instead.
    pub fn flush_pending(&mut self) {
        if self.net {
            for command in std::mem::take(&mut self.pending) {
                self.push_outgoing(command);
            }
            return;
        }
        for command in std::mem::take(&mut self.pending) {
            self.execute_now(command);
        }
    }

    /// Executes a command **immediately**, without a tick: in single player `NetSession_SubmitMessage` sends a
    /// message without a session directly to `Game_DispatchMessageByClass` - this is how giving up (`0x27` ->
    /// `0x50000056`) works even with the game menu open, in which no tick runs (re/ingame-menus 4). For the replay
    /// the command enters the log with the current step, **before** the commands still waiting (which the next tick
    /// books afterwards): the replay executes it first at the beginning of this step, with the same state as here.
    /// The checksum of the step is updated; events (messages, `PlayerResult`) stay in the state and come out with
    /// the next tick, as in the replay.
    ///
    /// On the checksum history: the entry for step S then shows the state **after** the immediate command, a replay
    /// of the log at S still the state **before** (the command takes effect there only at the beginning of the
    /// transition S -> S+1). Both are equal again from S+1.
    ///
    /// The host additionally remembers the new events for the pause frame ([`Self::take_immediate_events`]): the
    /// original reports an abort to the bar synchronously (`0x00507960` -> `NotifyUiIfSelected(0)`), also in the pause.
    ///
    /// In turn mode there is no immediate execution: the command goes into the next turn contribution (otherwise
    /// the machines would diverge). In replay mode only the giving up of the view slot.
    pub fn execute_now(&mut self, command: Command) {
        if self.playback_refuses(&command) {
            return;
        }
        if self.net {
            self.push_outgoing(command);
            return;
        }
        if self.log_open() {
            self.log.push(RecordedCommand {
                step: self.state.steps,
                command: command.clone(),
            });
        }
        let before = self.state.events.len();
        let mut sim = opennml_sim::sim::Sim::new(&mut self.state, &self.rules);
        opennml_sim::command::execute(&mut sim, &command, true);
        self.immediate_events
            .extend_from_slice(&self.state.events[before..]);
        // The original checks the fog memories every frame, also without a tick (`0x0047E560`); the immediate command
        // may change objects, so check again right away (as after every step).
        self.update_memories();
        if self
            .hashes
            .back()
            .is_some_and(|&(steps, _)| steps == self.state.steps)
        {
            self.hashes.pop_back();
        }
        self.record_hash();
    }

    /// Fetches the events of the immediately executed commands since the last tick that nobody has fetched yet
    /// (caller: the pause frame of the command bar, [`crate::game_session::GameSession::advance_paused`]). Without a
    /// pause the next tick delivers them from `state.events`; the tick empties the buffer.
    pub fn take_immediate_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.immediate_events)
    }

    /// Commands that wait for the next tick boundary (not yet executed).
    pub fn pending_commands(&self) -> &[Command] {
        &self.pending
    }

    /// Game time per millisecond of virtual time: `(GameSpeed + 10) / 20` (`GameEngine_Update`
    /// `0x00445650`, re/effects; 1.2 at the default 14).
    pub fn game_time_factor(game_speed: u8) -> f64 {
        (f64::from(game_speed) + 10.0) / 20.0
    }

    /// Game time of a frame with `dt_ms` milliseconds of virtual time: `dt * (GameSpeed + 10) / 20`.
    ///
    /// In turn mode it has no effect (log): there only [`Self::advance_rounds`] computes, with the bundles.
    pub fn advance(&mut self, dt_ms: f64) -> Vec<SimEvent> {
        if self.net {
            log::warn!("Network: advance has no effect in turn mode (ticks only with a bundle)");
            return Vec::new();
        }
        self.carry_ms += dt_ms * Self::game_time_factor(self.game_speed);
        let mut events = Vec::new();
        let mut steps = 0;
        while self.carry_ms >= f64::from(STEP_MS) {
            if steps == MAX_STEPS_PER_FRAME {
                // Time discarded as in the original (no catching up).
                self.carry_ms = 0.0;
                break;
            }
            self.carry_ms -= f64::from(STEP_MS);
            events.extend(self.step_once());
            steps += 1;
        }
        self.track_shown_heights();
        events
    }

    /// One tick with the collected commands. In turn mode no effect (log): a tick without a bundle would diverge from
    /// the other machines. In replay mode first the commands of the recording for this tick, then the accepted ones
    /// from the input (only the giving up).
    pub fn step_once(&mut self) -> Vec<SimEvent> {
        if self.net {
            log::warn!("Network: step_once has no effect in turn mode (ticks only with a bundle)");
            return Vec::new();
        }
        let mut commands = self.take_playback_step();
        commands.append(&mut self.pending);
        self.step_with(commands)
    }

    /// Switches on replay mode (E-331 adopted with the deviations below): [`Self::step_once`] gives every tick the
    /// commands of the recording `commands` with this tick, before the tick like `Replay::play`. Watching
    /// only (deviation from E-339; the original only blocks adding to the selection, `+0x2420`): [`Self::submit`] and
    /// [`Self::execute_now`] accept only the giving up of the view slot (of the active player now, `first_player`: the
    /// host for a network recording), they discard everything else with a debug log; [`Self::sync_selection`] and
    /// [`Self::press_button`] do nothing. Waiting commands lapse. At the end of the file one log entry, after that the
    /// match continues to compute (AI and scripts); the locks stay until the end of the match.
    pub fn begin_playback(&mut self, commands: Vec<RecordedCommand>) {
        self.pending.clear();
        self.mirror_sent = None;
        log::info!(
            "Replay: {} commands up to tick {}, view slot {}",
            commands.len(),
            commands.last().map_or(0, |c| c.step),
            self.active_player
        );
        self.playback = Some(Playback {
            commands,
            next: 0,
            view: self.active_player,
            end_notices: 0,
        });
    }

    /// State of the replay; `None` without a replay.
    pub fn playback(&self) -> Option<PlaybackStatus> {
        self.playback.as_ref().map(|p| PlaybackStatus {
            next_step: p.commands.get(p.next).map(|c| c.step),
            ended: p.next >= p.commands.len(),
        })
    }

    /// The commands of the recording for tick `state.steps` (replay mode, otherwise empty). After that report the
    /// end of the file once.
    fn take_playback_step(&mut self) -> Vec<Command> {
        let now = self.state.steps;
        let Some(p) = self.playback.as_mut() else {
            return Vec::new();
        };
        let from = p.next;
        while p.commands.get(p.next).is_some_and(|c| c.step <= now) {
            p.next += 1;
        }
        let commands = p.commands[from..p.next]
            .iter()
            .map(|c| c.command.clone())
            .collect();
        if p.next >= p.commands.len() && p.end_notices == 0 {
            p.end_notices += 1;
            log::info!(
                "Replay: end of file at tick {now}; the match continues, the AI continues to compute"
            );
        }
        commands
    }

    /// In replay mode discards every command except the giving up of the view slot; `true` if
    /// discarded.
    fn playback_refuses(&self, command: &Command) -> bool {
        let Some(p) = &self.playback else {
            return false;
        };
        let surrender = command.player == p.view
            && command.kind
                == (CommandKind::SetPlayerResult {
                    slot: p.view,
                    result: opennml_sim::game_end::DEFEAT,
                });
        if !surrender {
            log::debug!(
                "Replay: command discarded, watching only: {:?}",
                command.kind
            );
        }
        !surrender
    }

    /// Game time of a frame in turn mode: like [`Self::advance`] with `carry_ms` and at most
    /// [`MAX_STEPS_PER_FRAME`] ticks, but before every tick `rounds` is asked whether and with which commands it is
    /// computed. With [`RoundStep::Wait`] (execution tick without a bundle, halt) the game time remainder lapses:
    /// waiting does not accumulate time. After every computed tick the host calls [`Rounds::after_step`].
    pub fn advance_rounds(&mut self, dt_ms: f64, rounds: &mut dyn Rounds) -> Vec<SimEvent> {
        self.carry_ms += dt_ms * Self::game_time_factor(self.game_speed);
        let mut events = Vec::new();
        let mut steps = 0;
        while self.carry_ms >= f64::from(STEP_MS) {
            if steps == MAX_STEPS_PER_FRAME {
                self.carry_ms = 0.0;
                break;
            }
            match rounds.before_step(self) {
                RoundStep::Wait => {
                    self.carry_ms = 0.0;
                    break;
                }
                RoundStep::Step(commands) => {
                    self.carry_ms -= f64::from(STEP_MS);
                    events.extend(self.step_with(commands));
                    rounds.after_step(self);
                    steps += 1;
                }
            }
        }
        self.track_shown_heights();
        events
    }

    /// One tick with the commands `commands` instead of the collected ones (turn mode: the commands of the bundle
    /// at the execution tick). Like [`Self::step_once`] it books them in the command log with the tick before
    /// (`steps`), i.e. with `e(k)`: every machine records the commands of all players (E-290).
    pub fn step_with(&mut self, commands: Vec<Command>) -> Vec<SimEvent> {
        self.previous = self.state.tick_groups[0]
            .iter()
            .chain(&self.state.tick_groups[1])
            .filter_map(|&id| Some((id, self.state.object(id, false)?.world_pos)))
            .collect();
        // The events of immediate commands are still in `state.events` and come out with this tick.
        self.immediate_events.clear();
        let now = self.state.steps;
        if self.log_open() {
            self.log.extend(commands.iter().map(|c| RecordedCommand {
                step: now,
                command: c.clone(),
            }));
        }
        // State -2 renews the target values of the alpha table in the same call before the next
        // cycle begins (`GameWorld_UpdateFogOfWarStep` `0x00486D3A`, re/vision 3.2):
        // i.e. before the tick, while the foreground grids are still those of the ended cycle.
        // After the tick it would, with few stamps (S < B), already read the copy of the new cycle.
        let alpha_due = self.state.fog.processed == -2;
        if alpha_due {
            self.fog
                .update_targets(&self.state, self.active_player, false);
        }
        // Idle tick of the fog cycle (`fog_step` sets -1 -> -2): in the original this branch calls the minimap.
        let fog_idle = self.state.fog.processed == -1;
        let events = step(&mut self.state, &self.rules, &commands);
        self.fog_idle_ticks = self.fog_idle_ticks.wrapping_add(u32::from(fog_idle));
        // New objects (also new IDs after a change of owner) get their fog memory; the memories are
        // checked after that in every step ([`Self::update_memories`]).
        for event in &events {
            let id = match *event {
                SimEvent::Created { id } => id,
                SimEvent::OwnerChanged { new, .. } => new,
                // Cheat `rainbowring` (`FUN_00476860(-1)`): the view switch 0 <-> `FOG_OFF` lives in the host
                // (deviation from the original). Only toggle; the fog memories are checked by `update_memories`
                // below with the new switch (once per step, not twice).
                SimEvent::FogToggle => {
                    self.fog.toggle_show_mode();
                    continue;
                }
                // Cheat `bloodmode` (`FUN_0055FB90`, O-392): the blood switch of the program
                // ([`crate::game::blood`]). The map view attaches the effects of this tick only afterwards, i.e.
                // already with the new value.
                SimEvent::BloodToggle => {
                    crate::game::blood::toggle();
                    continue;
                }
                _ => continue,
            };
            self.memories.on_created(
                &self.state,
                &self.rules.map,
                &self.fog.view(&self.state),
                self.active_player,
                id,
            );
        }
        // Check the fog memories fully in every step (fixed and mobile): the original checks them every frame
        // after the ticks and before drawing (`0x0047FC70` marks `Terrain+0x21` itself, `0x00480120`, and calls
        // `0x0047E560`, re/vision 9). Object and memory thus read the same foreground: if the cycle ends with
        // the copy, the object disappears in the same step in which its memory becomes active (D-290). Grids and
        // objects change only in the step (and in [`Self::execute_now`]), so per step has the same effect.
        self.update_memories();
        self.record_hash();
        self.ki_stats.record(&events);
        self.script_log.record(self.state.steps, &events);
        self.present_log.record(self.state.steps, &events);
        self.script_commands.record(self.state.steps, &events);
        self.record_messages(now, &events);
        self.record_attack_points(&events);
        self.track_shown_heights();
        events
    }

    /// Remembers the combat points of the last `AttackPoints` event (re/combat-host 5).
    fn record_attack_points(&mut self, events: &[SimEvent]) {
        for event in events {
            if let SimEvent::AttackPoints { points } = event {
                self.attack_points = *points;
            }
        }
    }

    /// Combat points of the active player for the combat music; after a player switch the reported value of the
    /// new player applies immediately.
    pub fn active_attack_points(&self) -> i32 {
        self.attack_points[usize::from(self.active_player & 7)]
    }

    /// Depth by which a carcass has sunk in the rendering: in status 2 the dying module subtracts
    /// `hide_speed` from the height per tick and counts `die_counter` down by 101
    /// (re/combat-death 5). The counter starts at 3000 (decay, `hide_speed` 1) or 7500
    /// (swimmer); `0` outside the sinking.
    pub fn corpse_sink(&self, o: &GameObject) -> f32 {
        let Some(ModuleKind::Dying(d)) =
            o.module(opennml_sim::module::dying::INDEX).map(|m| &m.kind)
        else {
            return 0.0;
        };
        if d.status != 2 {
            return 0.0;
        }
        let start: i32 = if d.hide_speed == 1 { 3000 } else { 7500 };
        // Edge values of a corrupted but accepted savegame do not crash the viewer.
        let ticks = (start.wrapping_sub(d.die_counter) / 101).max(0);
        d.hide_speed.saturating_mul(ticks) as f32
    }

    /// Records the messages of a tick (`step` = engine tick before the step, as with the
    /// commands) per player, at most [`MESSAGE_HISTORY`]. Plus the text messages
    /// (`SimEvent::TextMessage`, headhunter) with their `texte.deu` number as `id`
    /// and without a tile: to one slot only there, otherwise at all slots 0-7.
    fn record_messages(&mut self, step: u64, events: &[SimEvent]) {
        for event in events {
            match *event {
                SimEvent::PlayerMessage { to, id, tile, .. } => {
                    // `to: None` means all: store at every slot 0-7.
                    let slots = 0..opennml_sim::id::PLAYER_SLOTS as u8;
                    for player in slots.filter(|&q| to.is_none_or(|p| p == q)) {
                        self.push_message(player, step, id, [tile.0, tile.1]);
                    }
                }
                SimEvent::TextMessage { to, text, .. } => {
                    let slots = 0..opennml_sim::id::PLAYER_SLOTS as u8;
                    for player in slots.filter(|&q| to.is_none_or(|p| p == q)) {
                        self.push_message(player, step, text, [-1, -1]);
                    }
                }
                _ => {}
            }
        }
    }

    /// Appends a message to the list of slot `player` (at most [`MESSAGE_HISTORY`]).
    fn push_message(&mut self, player: u8, step: u64, id: i32, tile: [i32; 2]) {
        let list = self.messages.entry(player).or_default();
        list.push_back(SimNotice {
            step,
            id,
            tile,
            text: None,
        });
        while list.len() > MESSAGE_HISTORY {
            list.pop_front();
        }
    }

    /// Last messages of player slot `player`, oldest first (without text).
    pub fn recent_messages(&self, player: u8) -> Vec<SimNotice> {
        self.messages
            .get(&player)
            .map_or_else(Vec::new, |l| l.iter().cloned().collect())
    }

    /// Clock of the sinking of a destroyed building in ms ([`sink_offset`]), derived from the state of the
    /// simulation (also correct after loading): `None` as long as the building is not
    /// destroyed (`m_IsDead`; sleeping palisades are only `DIED`). Otherwise the accumulated time
    /// of the timers that [`destroy`](opennml_sim::module::building::destroy::destroy) sets together
    /// (`0x3B03`, `0x3C03`, `0x3D03` - they count the same), plus the time since the last tick of the
    /// object group; timers not yet adopted count 0. Without these timers (after the
    /// collapse) the sinking is over: infinity.
    pub fn sink_clock(&self, o: &GameObject) -> Option<f64> {
        use opennml_sim::module::building::msg;
        let m = o.module(3)?;
        let ModuleKind::Building(b) = &m.kind else {
            return None;
        };
        if !b.is_dead {
            return None;
        }
        let ours = |code: u32| matches!(code, msg::START_SINK | msg::CLEAR_FIRES | msg::COLLAPSE);
        if m.base.timers_ws.iter().any(|e| ours(e.msg.code)) {
            return Some(0.0);
        }
        let clock = m
            .base
            .events
            .iter()
            .filter(|e| ours(e.msg.code))
            .map(|e| e.a)
            .max();
        Some(clock.map_or(f64::INFINITY, |a| {
            f64::from(a) + f64::from(self.group_progress_ms(o.tick_group))
        }))
    }

    /// Rendering without fog according to the render bits (`GameObject+0x1C4`) and the progress
    /// (`+0x18`) of the simulation; occupants (`QUARTERED`) are not drawn at all by `GameObject_RenderPass`
    /// (`0x0046B8CE`). Brightness 1; divers (`DIVING`) appear with opacity 0.6
    /// (re/swim-dive 3.3), everything else opaque.
    /// Player color from the color index `GameObject+0x2D0` (`GameObject_SetPlayerColor`
    /// `0x004680B0`); it thus follows every change of owner and stays the old one on the carcass (C-142).
    pub fn base_look(object: &GameObject) -> EntryLook {
        EntryLook {
            bits: if object.status & status::QUARTERED != 0 {
                0
            } else {
                object.render_bits
            },
            progress: object.progress_scale,
            brightness: 1.0,
            alpha: if object.status & status::DIVING != 0 {
                DIVER_ALPHA
            } else {
                1.0
            },
            color: Some(player_rgb(object.color)),
        }
    }

    /// Rendering of an object for the active player: [`SimHost::base_look`] with the
    /// render bits after fog ([`crate::game::fog_view::render_bits_for`]; occupants stay hidden),
    /// the opacity of a camouflage ([`masked_look`]) and the brightness at its tile at the
    /// rendering clock `now_ms`.
    pub fn entry_look(&mut self, id: ObjectId, now_ms: u32) -> Option<EntryLook> {
        let viewer = self.active_player;
        // Separate borrows: the state read-only, the alpha table writable (`self.fog.view`
        // would borrow the whole fog view).
        let view = FogView::new(&self.state).with_show_mode(self.fog.show_mode);
        let o = self.state.object(id, false)?;
        let base = Self::base_look(o);
        // Check occupants by status, not via `base.bits`: stealth also delivers the special bit `0x80`
        // with render bits 0 (`GameObject_RenderPass` `0x0046B890`).
        let bits = if o.status & status::QUARTERED != 0 || hidden_from_view(&self.state, o) {
            0
        } else {
            let memory_active = self.memories.get(id).is_some_and(|m| m.active);
            let bits =
                crate::game::fog_view::render_bits_for(&view, o, i32::from(viewer), memory_active);
            stealth_bits(bits, o.status)
        };
        let brightness = self.fog.object_brightness(o, viewer, now_ms);
        // Train objects fade out outside the map (`Train_MoveStep`: `SetAlpha`); then the
        // camouflage ([`masked_look`]), the alpha of ghost and animal ([`fade_alpha`]) and finally the
        // opacity of the diver.
        let alpha = self.train_fade(o).unwrap_or_else(|| {
            masked_look(&self.state, o, viewer)
                .map(|(_, alpha)| alpha)
                .or_else(|| fade_alpha(o))
                .unwrap_or(base.alpha)
        });
        Some(EntryLook {
            bits,
            brightness,
            alpha,
            ..base
        })
    }

    /// Fog check of the selection and the click target (`0x00467D70`) for the active player, with the
    /// view switch (Fog off: everything selectable).
    pub fn fog_selectable(&self, o: &GameObject) -> bool {
        let tile = self.memories.get(o.id).map(|m| m.tile);
        crate::game::fog_view::visible_for_selection(
            &self.fog.view(&self.state),
            o,
            i32::from(self.active_player),
            tile,
        )
    }

    /// Overlay line of the account of the active player: gold, wood, food and population occupied /
    /// living space (`Player_GetResource`, living space clamped to the limit).
    pub fn account_line(&self) -> String {
        let slot = usize::from(self.active_player);
        let r = |i| self.state.resource(slot, i);
        format!(
            "Balance player {slot}: Gold {} | Wood {} | Food {} | Population {}/{}",
            r(resource::GOLD),
            r(resource::WOOD),
            r(resource::FOOD),
            r(resource::POPULATION),
            r(resource::LIVING_SPACE)
        )
    }

    fn record_hash(&mut self) {
        self.hashes
            .push_back((self.state.steps, checksum(&self.state)));
        while self.hashes.len() > HASH_HISTORY {
            self.hashes.pop_front();
        }
    }

    /// Last checksum.
    pub fn checksum(&self) -> u64 {
        self.hashes.back().map_or(0, |h| h.1)
    }

    /// Checksum of the state before tick `steps` (entry after `steps` computed ticks), as long as it is in the
    /// history ([`HASH_HISTORY`]). Turn mode: checksum of the contribution of turn `k` with `steps = b(k)`.
    pub fn hash_at(&self, steps: u64) -> Option<u64> {
        self.hashes
            .iter()
            .rev()
            .find(|&&(s, _)| s == steps)
            .map(|&(_, h)| h)
    }

    /// Slots 0-7 that no longer play (result != 0: lost, won or not participating). The turn core of the
    /// host omits the dropout command for them. Result 3 ("not participating") also counts as "no longer
    /// playing"; the command would have no effect there anyway.
    pub fn results(&self) -> [bool; 8] {
        std::array::from_fn(|p| self.state.players[p].result != 0)
    }

    /// Share of the game time since the last tick (0 to 1) for interpolation.
    pub fn alpha(&self) -> f32 {
        (self.carry_ms / f64::from(STEP_MS)).clamp(0.0, 1.0) as f32
    }

    /// Game time since the last tick of object group `group` in ms, at most 100
    /// (`GameWorld_GetTickGroupFrameProgress` `0x004759E0`): after a step with phase 1 group 0 ticked,
    /// with phase 0 group 1.
    fn group_progress_ms(&self, group: i8) -> i32 {
        let last = if self.state.phase == 1 { 0 } else { 1 };
        let since = if group.clamp(0, 1) == last {
            self.carry_ms
        } else {
            self.carry_ms + f64::from(STEP_MS)
        };
        since.min(100.0) as i32
    }

    /// Rendering of an object with a walk module like `PropertyWalk_FrameUpdate` (re/motion
    /// 7.1), `None` for objects without walk. Without `MOVE` the world position of the simulation stays
    /// (without interpolation), only the height keeps the value of the last `MOVE` frame, as long as
    /// the grid position is that of the halt ([`SimHost::track_shown_heights`]). With `MOVE`: status `0x10`,
    /// progress 0 or an empty trajectory show exactly the grid position; otherwise (state 2, 5, 0xB) the unit lies,
    /// according to the time since the tick of its own object group, between the grid and the **next** trajectory
    /// point (the rendering runs up to one tick ahead), in the other states x/z stays.
    /// The height comes from the terrain (without height offset), with the walk flags `0x2000` (with
    /// `0x20000` or without `0x800`) and with `SWIMMING` at least at water height; on the wall,
    /// while diving and on a boat it stays unchanged. The rotation (yaw) only changes the
    /// interpolation, otherwise that of the simulation stays.
    pub fn walk_display(&self, o: &GameObject) -> Option<(Vec3, f32)> {
        let (mut pos, yaw) = self.walk_frame(o)?;
        if o.status & status::MOVE == 0
            && let Some(shown) = self.shown_heights.get(&o.id)
            && shown.grid == Self::grid_key(o)
        {
            pos.y = shown.y;
        }
        Some((pos, yaw))
    }

    fn grid_key(o: &GameObject) -> [i32; 3] {
        [o.grid.x.0, o.grid.y.0, o.grid.z.0]
    }

    /// Remembers the height that [`SimHost::walk_display`] shows with `MOVE`, per walk object. In the original
    /// it stays in `world_pos.y` after the halt (only `PropertyWalk_FrameUpdate` writes it, and
    /// only with `MOVE`, `0x00538BA0`); the viewer otherwise computes statelessly from the simulation and
    /// would show a swimmer whose `SWIMMING` came on in the last step on the sea floor. The height applies as
    /// long as the grid position stays that of the halt; a step
    /// or snap (different position) supersedes it. Called after every tick and frame.
    pub fn track_shown_heights(&mut self) {
        let mut seen = Vec::new();
        for &id in self.state.tick_groups[0]
            .iter()
            .chain(&self.state.tick_groups[1])
        {
            let Some(o) = self.state.object(id, false) else {
                continue;
            };
            let key = Self::grid_key(o);
            if o.status & status::MOVE != 0 {
                if let Some((pos, _)) = self.walk_frame(o) {
                    seen.push((
                        id,
                        Some(ShownHeight {
                            grid: key,
                            y: pos.y,
                            moving: true,
                        }),
                    ));
                }
            } else if let Some(shown) = self.shown_heights.get(&id) {
                // The halt adopts the position of the halt tick; after that every other position supersedes it.
                let keep = shown.moving || shown.grid == key;
                seen.push((
                    id,
                    keep.then_some(ShownHeight {
                        grid: key,
                        moving: false,
                        ..*shown
                    }),
                ));
            }
        }
        for (id, shown) in seen {
            match shown {
                Some(shown) => self.shown_heights.insert(id, shown),
                None => self.shown_heights.remove(&id),
            };
        }
        // Forget vanished objects.
        let state = &self.state;
        self.shown_heights
            .retain(|&id, _| state.object(id, false).is_some());
    }

    /// The rendering according to `PropertyWalk_FrameUpdate` without the remembered height of the halted state
    /// ([`SimHost::walk_display`]).
    fn walk_frame(&self, o: &GameObject) -> Option<(Vec3, f32)> {
        let m = o.module(0)?;
        let ModuleKind::Walk(w) = &m.kind else {
            return None;
        };
        let rotation_y = o.rotation[1];
        if o.status & status::MOVE == 0 {
            return Some((Vec3::from(o.world_pos), rotation_y));
        }
        let cell = self.rules.map.cell;
        let on_grid = opennml_sim::world::grid_to_world(o.grid, cell);
        let p = if o.status & 0x10 != 0 {
            0
        } else {
            self.group_progress_ms(o.tick_group)
        };
        let (mut pos, mut yaw) = (on_grid, rotation_y);
        if p > 0 && !w.trajectory.is_empty() {
            // The height query starts above the height that was previously in `world_pos`.
            pos[1] = o.world_pos[1];
            if matches!(w.state, 2 | 5 | 0xB) {
                let (x, z, dir) = self.interpolated_walk_point(o, w, p);
                let w =
                    opennml_sim::world::grid_to_world(FxVec3::new(Fx(x), Fx::ZERO, Fx(z)), cell);
                (pos[0], pos[2]) = (w[0], w[2]);
                yaw = yaw_of_direction(dir);
            } else {
                (pos[0], pos[2]) = (o.world_pos[0], o.world_pos[2]);
            }
        }
        if o.status & NO_TERRAIN == 0 {
            pos[1] = opennml_sim::height::terrain_height(&self.rules, pos[0], pos[1], pos[2]);
        }
        if clamps_to_water(o) {
            pos[1] = pos[1].max(self.rules.map.water_level);
        }
        Some((Vec3::from(pos), yaw))
    }

    /// `PropertyWalk_GetInterpolatedPos` (`0x00537E30`): grid x, z and direction (fixed point)
    /// after `p` ms (> 0) on trajectory `w` (not empty); beyond 100 ms it moves on to the following node.
    fn interpolated_walk_point(&self, o: &GameObject, w: &Walk, mut p: i32) -> (i32, i32, i32) {
        let mut points = w.trajectory.iter();
        let mut n = points.next().expect("trajectory not empty");
        let mut s = (o.grid.x.0, o.grid.z.0, o.direction.0);
        while p >= 100 {
            let Some(next) = points.next() else {
                return (n.x, n.z, n.dir);
            };
            p -= 100;
            s = (n.x, n.z, n.dir);
            n = next;
        }
        let lerp = |a: i32, b: i32| b.wrapping_sub(a).wrapping_mul(p) / 100 + a;
        let turn = opennml_sim::world::wrap_angle(n.dir.wrapping_sub(s.2));
        (
            lerp(s.0, n.x),
            lerp(s.1, n.z),
            opennml_sim::world::wrap_angle(turn.wrapping_mul(p) / 100 + s.2),
        )
    }

    /// Position and rotation of an object for rendering: walking units as in the original,
    /// everything else between the previous and the current tick.
    /// Destroyed buildings sink in the process ([`sink_offset`]), carcasses with the dying module
    /// ([`Self::corpse_sink`]).
    ///
    /// Riders and occupants (`ISMOUNTED | ISMOUNTEDONBOAT` with an existing target) take position, yaw
    /// and pitch of their carrier per frame (`GameObject_FrameUpdate` `0x00470BD0`, re/riding 7.1); the
    /// target itself carries no rider bits, one level suffices. Divers lie
    /// `height_offset * cell_y / 4096` lower (`0x00470D72`, re/swim-dive 3.3).
    pub fn display(&self, object: &GameObject) -> (Vec3, Vec3) {
        // One level, no recursion: a boat ring (two objects with `0x8000` that have each other as
        // target) must not send the rendering into an endless loop.
        match self.carrier_of(object) {
            Some(carrier) => self.display_own(carrier),
            None => self.display_own(object),
        }
    }

    /// The carrier of a rider or occupant (`ISMOUNTED | ISMOUNTEDONBOAT`): the target if it is
    /// neither `NONE` nor 0 and the object exists (`walk::mount_of`, re/riding 4.1).
    fn carrier_of(&self, object: &GameObject) -> Option<&GameObject> {
        if object.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) == 0 {
            return None;
        }
        let target = object.target;
        if target.is_none() || target.0 == 0 {
            return None;
        }
        self.state.object(target, false)
    }

    /// [`SimHost::display`] without the takeover from the carrier: the own position of the object.
    fn display_own(&self, object: &GameObject) -> (Vec3, Vec3) {
        let rotation = Vec3::from(object.rotation);
        let (mut p, rotation) = match self.walk_display(object) {
            Some((p, yaw)) => (p, Vec3::new(rotation.x, yaw, rotation.z)),
            // Train and rear part compute their rendering position themselves (re/train-multipart 7).
            None => match self
                .train_display(object)
                .or_else(|| self.tail_display(object))
            {
                Some(shown) => shown,
                None => {
                    let mut p = self.interpolated(object);
                    if let Some(clock) = self.sink_clock(object) {
                        p.y += sink_offset(clock);
                    }
                    (p, rotation)
                }
            },
        };
        // Carcasses (walk stays active, so both branches) sink with the dying module.
        p.y -= self.corpse_sink(object);
        // Diver: the model lies `height_offset * cell_y / 4096` lower (re/swim-dive 3.3).
        // Like `GameObject_FrameUpdate` (`0x00470D72`) unconditionally: without a check for `ONWATER` or
        // `DIVING`, also when surfacing after the loss of `ONWATER`. If the
        // simulation height already contains the offset (`snap_height_to_terrain` without `ONWATER`), it takes
        // effect twice, as in the original.
        p.y += object.height_offset as f32 * self.rules.map.cell[1] as f32 / 4096.0;
        (p, rotation)
    }

    /// Rendering position of a train object while driving (`PropertyTrain_FrameUpdate` `0x0052F500`,
    /// re/train-multipart 7): grid position plus `fixed_mul(drive vector, next_speed) * p / 100` with the
    /// progress `p` (0 to 100 ms) of the object group of the locomotive; tender and wagon ask the locomotive
    /// (`0xB21`, master) and get the same offset. The height comes from the terrain. `None` when
    /// standing (status bit 2 off) and for objects without a train module: then the own position applies.
    fn train_display(&self, o: &GameObject) -> Option<(Vec3, Vec3)> {
        let own = train_of(o)?;
        let (lead, t) = if own.is_slave {
            let master = self.state.object(own.master, false)?;
            (master, train_of(master)?)
        } else {
            (o, own)
        };
        if t.status & train::status::MOVING == 0 {
            return None;
        }
        let p = self.group_progress_ms(lead.tick_group) as f32;
        let step = |dir: i32| fixed_mul(dir, t.next_speed) as f32 * p / 100.0;
        let cell = self.rules.map.cell;
        let mut pos = opennml_sim::world::grid_to_world(o.grid, cell);
        pos[0] += step(t.dir.0) * cell[0] as f32 / 4096.0;
        pos[2] += step(t.dir.1) * cell[2] as f32 / 4096.0;
        pos[1] = opennml_sim::height::terrain_height(&self.rules, pos[0], o.world_pos[1], pos[2]);
        Some((Vec3::from(pos), Vec3::from(o.rotation)))
    }

    /// Alpha of a train object: `max(0, 1 - 0.2 * d)` with the overhang `d` of the grid position beyond the
    /// map edge in tiles (`Train_MoveStep` `0x00530CE0` with `0x0052EEB0`, `GameObject_SetAlpha`),
    /// 1 inside the map. `None` for objects without a train module.
    fn train_fade(&self, o: &GameObject) -> Option<f32> {
        train_of(o)?;
        // Same computation as the drive step of the simulation (`0x0052EEB0`).
        let d = train::drive::overshoot(self.rules.map, (o.grid.x.0, o.grid.z.0));
        Some(if d > 0 {
            (1.0 - 0.2 * (d as f32 / 4096.0)).max(0.0)
        } else {
            1.0
        })
    }

    /// Speed of the wheel animation of a train object (re/train-multipart 7). The original sets it in every
    /// acceleration tick of the locomotive (`0x0052F3F0`) for locomotive and slaves with
    /// `GameObject_RefreshAnimIfCurrent` (`0x00467080`, only when the arrived animation = the wanted one) to
    /// `f32(Fixed_ToFloat(SX * v * 10)) * 1.5`;
    /// `AnimInstance_SetTimeScale` (`0x0058DA30`) takes speed <= 0 as 128 and acts only on the instance of the
    /// carrier. After that `v` stays until braking, so the speed does too; the start of the brake animation sets
    /// it back to 128. The reimplementation derives it from the state instead of storing it:
    /// - locomotive driving without braking and animation arrived: [`WheelRate::Derived`] from `v` of the locomotive
    ///   (in the first tick `v` = 0, so as in the original the normal speed);
    /// - locomotive driving with the animation not yet arrived (start of braking, handover in the next object tick):
    ///   [`WheelRate::Keep`], the original keeps playing the running animation at the old speed;
    /// - otherwise (standing, braking, no train object) `None`: the speed of the simulation.
    pub fn train_wheel_rate(&self, o: &GameObject) -> Option<WheelRate> {
        let own = train_of(o)?;
        let t = if own.is_slave {
            train_of(self.state.object(own.master, false)?)?
        } else {
            own
        };
        if t.status & train::status::MOVING == 0 {
            return None;
        }
        if o.anim.current != o.anim.wanted {
            return Some(WheelRate::Keep);
        }
        if t.status & train::status::BRAKING != 0 {
            return None;
        }
        Some(WheelRate::Derived(wheel_rate(
            self.rules.map.cell[0],
            t.speed,
        )))
    }

    /// Rendering position of the rear part of a multipart object (`MultiPart_FollowHead` in the `FrameUpdate`
    /// `0x0051EAA0`/`0x0051E860`, re/train-multipart 3.8, 7; K-247): the rendering position of the front part,
    /// rotated by the angle from the trailing drawbar point of the simulation to the rendering position of the
    /// head ([`multipart::follow_angle`], limited to +-90 degrees from the head direction) plus 90 degrees. Rendering
    /// only: the original writes the drawbar point here depending on the frame rate, the reimplementation
    /// leaves the state of the simulation alone (deviation). `None` without a rear part module or
    /// without an existing front part with walk.
    fn tail_display(&self, o: &GameObject) -> Option<(Vec3, Vec3)> {
        let ModuleKind::MultiPart(part) = &o.module(multipart::PART)?.kind else {
            return None;
        };
        let head = self
            .state
            .object(part.prev, false)
            .filter(|h| h.id != o.id)?;
        let (pos, _) = self.walk_display(head)?;
        let (x, z, dir) = self.walk_pose(head);
        let angle = multipart::follow_angle((part.centre.0, part.centre.1), (x, z, dir));
        let rotation = Vec3::from(o.rotation);
        Some((
            pos,
            Vec3::new(rotation.x, yaw_of_direction(angle), rotation.z),
        ))
    }

    /// Grid x, z and direction (fixed point) of a walk object in the rendering: on the way interpolated on
    /// the trajectory as in [`SimHost::walk_frame`], otherwise the grid position.
    fn walk_pose(&self, o: &GameObject) -> (i32, i32, i32) {
        if let Some(ModuleKind::Walk(w)) = o.module(0).map(|m| &m.kind)
            && o.status & status::MOVE != 0
            && o.status & 0x10 == 0
            && matches!(w.state, 2 | 5 | 0xB)
            && !w.trajectory.is_empty()
        {
            let p = self.group_progress_ms(o.tick_group);
            if p > 0 {
                return self.interpolated_walk_point(o, w, p);
            }
        }
        (o.grid.x.0, o.grid.z.0, o.direction.0)
    }

    /// "Go to" (action 448) of the control channel (`sim_move`): the own units with a walk module
    /// to the world point; `queue` = Shift (append, `false`) or Ctrl (cyclic, `true`). `false`
    /// if no unit can go. The right-click of the map goes via the mapping to
    /// `targets.ini` ([`SimHost::issue_target_action`]).
    pub fn move_units(
        &mut self,
        units: &[ObjectId],
        target: Vec3,
        queue: Option<bool>,
        order: FormationOrder,
    ) -> bool {
        let player = self.active_player;
        let units: Vec<ObjectId> = units
            .iter()
            .copied()
            .filter(|&id| {
                self.state
                    .object(id, true)
                    .is_some_and(|o| o.owner() == player && o.has_module(0, false))
            })
            .collect();
        if units.is_empty() {
            return false;
        }
        let (x, z) = crate::game::grid::world_to_grid(target, self.rules.map.cell);
        let kind = CommandKind::Move {
            units,
            x,
            z,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: opennml_sim::command::NO_SPREAD,
            flags: 0,
        };
        let kind = order.apply(kind);
        let kind = match queue {
            Some(cyclic) => CommandKind::Queue {
                cyclic,
                inner: Box::new(kind),
            },
            None => kind,
        };
        self.submit(Command { player, kind });
        true
    }

    /// World position between the previous and the current tick.
    pub fn interpolated(&self, object: &GameObject) -> Vec3 {
        let now = Vec3::from(object.world_pos);
        match self.previous.get(&object.id) {
            Some(&before) => Vec3::from(before).lerp(now, self.alpha()),
            None => now,
        }
    }

    /// Savegame (start package, state, no open commands).
    pub fn save(&self) -> SaveGame {
        SaveGame {
            start: start_package(&self.rules, &self.setup),
            state: self.state.clone(),
            commands: Vec::new(),
        }
    }

    /// Resets the state (loading, restart): interpolation, open commands, checksums and
    /// game time remainder start anew; a replay ends.
    fn reset_to(&mut self, state: SimState) {
        self.state = state;
        self.playback = None;
        self.previous.clear();
        self.shown_heights.clear();
        self.pending.clear();
        self.mirror_sent = None;
        self.immediate_events.clear();
        self.log.clear();
        self.hashes.clear();
        self.carry_ms = 0.0;
        self.messages.clear();
        self.attack_points = [0; 8];
        self.ki_stats = KiStats::default();
        self.script_log = ScriptLog::default();
        self.present_log = PresentLog::default();
        self.script_commands = ScriptCommandStats::default();
        self.record_hash();
    }

    /// Loads a savegame of the same map and rules. The caller must check it beforehand against these rules
    /// ([`SaveGame::check`], in the game via [`GameSaveFile::check`](crate::game_save::GameSaveFile::check); once per
    /// load). With `log` (the command log from the start of the match; unchecked) the replay stays complete and
    /// continues the log; without it a replay is no longer possible afterwards (the commands before the
    /// savegame are missing).
    pub fn load(&mut self, save: SaveGame, log: Option<Vec<RecordedCommand>>) {
        self.setup = save.start.setup;
        self.reset_to(save.state);
        self.log_complete = log.is_some();
        self.log = log.unwrap_or_default();
        self.active_player = self.first_player().unwrap_or(0);
        self.reset_fog(false);
    }

    /// The command log for the savegame ([`HostPart::log`](crate::game_save::HostPart::log)): a copy if
    /// it is complete from the start of the match until now, otherwise `None`.
    pub fn save_log(&self) -> Option<Vec<RecordedCommand>> {
        self.log_complete.then(|| self.log.clone())
    }

    /// Network state for sync and desync report: savegame like [`Self::save`] (without commands) and
    /// the whole command log since the start.
    pub fn net_state(&self) -> NetState {
        NetState {
            save: self.save(),
            log: self.log.clone(),
        }
    }

    /// Adopts the state of the host during sync: checks it against the rules
    /// ([`NetState::check`]) and demands the same start package as the own match. After that state and
    /// command log of the host apply; interpolation, checksum history, messages and game time remainder start anew,
    /// fog and fog memories are rebuilt (as with loading only on explored positions). Unlike [`Self::load`]
    /// the own slot, the speed, the fog switch, turn mode, the own commands for the next
    /// contribution and the flag of the complete replay stay (after loading a state without a log in the savegame
    /// the commands before it are also missing from the log of the host, and the replay stays off;
    /// with a log and without loading it stays complete); camera, masks and selection lie outside the
    /// host anyway. On an error everything stays unchanged.
    pub fn adopt(&mut self, net: NetState) -> anyhow::Result<()> {
        net.check(&self.rules)?;
        if net.save.start != start_package(&self.rules, &self.setup) {
            bail!("sync rejected: the state belongs to a different match (start package differs)");
        }
        let NetState { save, log } = net;
        self.reset_to(save.state);
        self.log = log;
        self.reset_fog(false);
        Ok(())
    }

    /// Only for check runs (desync tests and `net_debug corrupt` of the control channel):
    /// corrupts the own state at a harmless spot, the time of day [`SimState::daytime`] (cycling
    /// -1 to 3; read only by the terrain upgrade). The last entry of the checksum history follows so that the
    /// next turn contribution carries the deviation. Returns the name of the corrupted part
    /// ([`opennml_sim::save::part_checksums`]).
    pub fn debug_corrupt(&mut self) -> &'static str {
        let d = &mut self.state.daytime;
        *d = if *d >= 3 { -1 } else { *d + 1 };
        log::warn!(
            "Check run: state corrupted (time of day {} at tick {})",
            self.state.daytime,
            self.state.steps
        );
        let hash = checksum(&self.state);
        if let Some(last) = self
            .hashes
            .back_mut()
            .filter(|(s, _)| *s == self.state.steps)
        {
            last.1 = hash;
        }
        "daytime"
    }

    /// `sim_setup`: new match with `setup` (if given) and speed. On an error everything stays
    /// unchanged (the speed is adopted only after the successful restart). The
    /// setup from the settings of the viewer is built by `config::apply_config` in the viewer.
    pub fn apply_setup(&mut self, setup: Option<MatchSetup>, game_speed: u8) -> Result<(), String> {
        if let Some(setup) = setup {
            self.restart(setup)?;
        }
        self.game_speed = game_speed;
        Ok(())
    }

    /// New match with a different setup.
    pub fn restart(&mut self, setup: MatchSetup) -> Result<(), String> {
        let state = new_match(&self.rules, &setup)?;
        self.setup = setup;
        self.reset_to(state);
        self.log_complete = true;
        self.active_player = self.first_player().unwrap_or(0);
        self.reset_fog(true);
        Ok(())
    }

    /// Plays a replay up to engine tick `steps` (default: one tick after the last
    /// command) and continues the recording with its commands.
    pub fn load_replay(&mut self, replay: Replay, steps: Option<u64>) -> anyhow::Result<()> {
        let end = match steps {
            Some(end) => end,
            None => replay.implied_end()?,
        };
        // `rainbowring` of the replay toggles the fog switch of the host as live (deviation from the original), from
        // the start value of a new match (0); if playing fails, the old value stays.
        // `bloodmode` toggles the blood switch of the program likewise, but from its current value (it applies until
        // the program ends, O-392).
        let previous = std::mem::replace(&mut self.fog.show_mode, 0);
        let previous_blood = crate::game::blood::enabled();
        let played = replay.play_observed(&self.rules, end, &mut |events| {
            for event in events {
                match event {
                    SimEvent::FogToggle => self.fog.toggle_show_mode(),
                    SimEvent::BloodToggle => crate::game::blood::toggle(),
                    _ => {}
                }
            }
        });
        let state = match played {
            Ok(state) => state,
            Err(error) => {
                self.fog.show_mode = previous;
                crate::game::blood::init(previous_blood);
                return Err(error);
            }
        };
        self.setup = replay.start.setup;
        self.reset_to(state);
        self.log = replay
            .commands
            .into_iter()
            .filter(|c| c.step < end)
            .collect();
        self.log_complete = true;
        self.active_player = self.first_player().unwrap_or(0);
        self.reset_fog(false);
        Ok(())
    }

    /// Whether the command log reaches from the start of the match (tick 0) to now.
    pub fn log_complete(&self) -> bool {
        self.log_complete
    }

    /// Whether tick `state.steps` may still enter the command log. From [`MAX_REPLAY_STEPS`] (eight hours) the
    /// log ends: `log_complete` becomes false once (with a log entry), even if no command comes (deviation from
    /// E-332).
    fn log_open(&mut self) -> bool {
        if self.state.steps < MAX_REPLAY_STEPS {
            return true;
        }
        if self.log_complete {
            self.log_complete = false;
            log::info!("Command log ends after eight hours (tick {MAX_REPLAY_STEPS})");
        }
        false
    }

    /// Replay since the start of the match; without a complete log (after loading a savegame without a
    /// log or after eight hours) an error.
    pub fn replay(&self) -> anyhow::Result<Replay> {
        if !self.log_complete {
            bail!(
                "no replay: the command log is incomplete (after loading a savegame without a log or after eight hours)"
            );
        }
        Ok(Replay {
            start: start_package(&self.rules, &self.setup),
            commands: self.log.clone(),
        })
    }

    /// Section `sim` of the state snapshot.
    pub fn section(&self, selection: &[ObjectId]) -> SimSection {
        SimSection {
            tick: self.state.tick,
            steps: self.state.steps,
            mode: self.state.mode,
            seed: self.setup.seed,
            game_speed: self.game_speed,
            active_player: self.active_player,
            checksum: hex_checksum(self.checksum()),
            objects: self.state.tables.iter().map(|t| t.count).sum(),
            selection: selection.iter().map(|id| id.0).collect(),
            commands: self.log.len(),
            grid_layer: None,
            path: None,
            moving: self
                .state
                .objects()
                .filter(|o| o.status & status::MOVE != 0)
                .count(),
            wood_price: self.state.economy.wood_price,
            food_price: self.state.economy.food_price,
            max_living_space: self.state.economy.max_living_space,
            notices: self.recent_messages(self.active_player),
            target_marker: None,
            ground_markers: Vec::new(),
            fog_shown: self.fog.show_mode == 0,
            fog_overlay: self.fog.overlay,
            attack_points: self.attack_points,
            attack_points_active: self.active_attack_points(),
            projectiles: self.projectile_infos(),
            // Only the map knows the flights: `section_with_flights` in the viewer
            // (`apps/opennml-viewer/src/app.rs`) sets the number.
            flights: 0,
            groups: self.group_infos(),
            // The selected formation is viewer state: `section_with_flights` sets it.
            formation: 0,
            flags: flags_info(&self.state),
        }
    }

    /// Groups of the manager in map order (`MfcMap`, protocol 13); read-only.
    fn group_infos(&self) -> Vec<SimGroupInfo> {
        self.state
            .groups
            .map
            .iter()
            .map(|(_, g)| SimGroupInfo {
                id: g.id,
                kind: g.kind,
                type_id: g.type_id(),
                members: g.members.iter().map(|m| m.0).collect(),
                anchor: [g.anchor.0, g.anchor.1, g.anchor.2],
                marching: g.marching,
                matched_speed: g.matched_speed,
                targets: g
                    .targets
                    .iter()
                    .map(|(id, &(x, z, dir))| SimGroupTarget { id, x, z, dir })
                    .collect(),
                formation: g.formation.as_ref().map(|f| SimFormationInfo {
                    spacing: f.spacing,
                    depth: f.depth,
                    layer: f.layer,
                    rear: f.rear,
                    rear_members: f.rear_members.iter().map(|m| m.0).collect(),
                    frames: f.frames.len(),
                    ready: f.ready.iter().map(|m| m.0).collect(),
                    reported: f.reported,
                    arrived: f.arrived,
                    end_max: f.end_max.0,
                    end_min: f.end_min.0,
                }),
            })
            .collect()
    }

    /// `sim_hash`.
    pub fn hash_result(&self) -> SimHashResult {
        SimHashResult {
            tick: self.state.tick,
            steps: self.state.steps,
            checksum: hex_checksum(self.checksum()),
            history: self
                .hashes
                .iter()
                .map(|&(s, h)| (s, hex_checksum(h)))
                .collect(),
        }
    }

    /// `sim_query`: objects in table order, filtered by owner, type and IDs
    /// (all specifications together), at most `limit`; plus all player slots 0-7.
    pub fn query(
        &self,
        player: Option<u8>,
        guid: Option<u32>,
        ids: Option<&[u32]>,
        limit: usize,
    ) -> SimQueryResult {
        let matching: Vec<&GameObject> = self
            .state
            .objects()
            .filter(|o| player.is_none_or(|p| o.owner() == p))
            .filter(|o| guid.is_none_or(|g| o.guid == g))
            .filter(|o| ids.is_none_or(|ids| ids.contains(&o.id.0)))
            .collect();
        SimQueryResult {
            tick: self.state.tick,
            steps: self.state.steps,
            total: matching.len(),
            objects: matching
                .into_iter()
                .take(limit)
                .map(|o| self.object_info(o))
                .collect(),
            players: self
                .state
                .players
                .iter()
                .enumerate()
                .map(|(slot, p)| SimPlayerInfo {
                    slot: slot as u8,
                    control: match p.control {
                        Control::Empty => "empty",
                        Control::Human => "human",
                        Control::Ai => "ai",
                        Control::Passive => "passive",
                    }
                    .into(),
                    name: p.name.clone(),
                    nation: p.nation,
                    team: p.team,
                    color: p.color,
                    resources: p.resources_fx.map(|v| v >> 12),
                    friends: mask(&self.state.diplomacy.friend, slot),
                    population: [
                        self.state.resource(slot, resource::POPULATION),
                        self.state.resource(slot, resource::LIVING_SPACE),
                    ],
                    counts: nonzero(self.state.guid_counts.get(slot)),
                    pending: nonzero(self.state.pending_counts.get(slot)),
                    enemies: mask(&self.state.diplomacy.enemy, slot),
                    vision: mask(&self.state.diplomacy.view, slot),
                    upgrades: self
                        .state
                        .applied_upgrades
                        .get(slot)
                        .cloned()
                        .unwrap_or_default(),
                    attack_reports: self.state.attack_reports[slot]
                        .iter()
                        .map(|(victim, t)| SimAttackReport {
                            victim: victim as u32,
                            tile: [t.x, t.z],
                        })
                        .collect(),
                })
                .collect(),
            counter_upgrades: self
                .state
                .counter_upgrades
                .iter()
                .map(|c| SimCounterUpgrade {
                    guid: c.guid,
                    rest_ms: c.rest_ms,
                    source: c.source.0,
                })
                .collect(),
            daytime: self.state.daytime,
            headhunters: self.headhunter_table(),
            kill_counts: self
                .state
                .kill_counts
                .iter()
                .enumerate()
                .flat_map(|(slot, counts)| {
                    counts
                        .iter()
                        .map(move |(&guid, &count)| (slot as u8, guid, count))
                })
                .collect(),
            button_locks: self
                .state
                .button_locks
                .iter()
                .map(|(&guid, &locked)| (guid, locked))
                .collect(),
            slot_running: self.state.slot_running.to_vec(),
            area_views: self
                .state
                .fog
                .area_views
                .iter()
                .map(|v| SimAreaView {
                    name: v.name.clone(),
                    rect: v.rect,
                    player: v.player,
                    clear: v.clear,
                })
                .collect(),
        }
    }

    fn object_info(&self, o: &GameObject) -> SimObjectInfo {
        SimObjectInfo {
            id: o.id.0,
            guid: o.guid,
            type_name: self
                .rules
                .types
                .get(&o.guid)
                .map_or_else(String::new, |t| t.name.clone()),
            owner: o.owner(),
            position: o.world_pos,
            tile: [o.tile.0, o.tile.1],
            direction: o.direction.to_f32(),
            status: o.status,
            animation: o.anim.current,
            animation_starts: o.anim.starts,
            target: (o.target != ObjectId::NONE).then_some(o.target.0),
            tick_group: o.tick_group,
            // Script name (reverse lookup like `FUN_004339B0`) and hiding (only the response; viewer and
            // client do not hide objects until later).
            name: opennml_sim::names::reverse(&self.state.names, o.id),
            hidden: opennml_sim::world::is_hidden(&self.state, o.id),
            modules: o
                .modules
                .iter()
                .enumerate()
                .filter_map(|(index, m)| {
                    let m = m.as_ref()?;
                    Some(SimModuleInfo {
                        index: index as u8,
                        name: MODULE_NAMES.get(index).copied().unwrap_or("?").into(),
                        kind: match m.kind {
                            ModuleKind::Inert => "inert",
                            ModuleKind::Walk(_) => "walk",
                            ModuleKind::Stand(_) => "stand",
                            ModuleKind::Building(_) => "building",
                            ModuleKind::Tree(_) => "tree",
                            ModuleKind::Natur(_) => "natur",
                            ModuleKind::Resource(_) => "resource",
                            ModuleKind::Lumber(_) => "lumber",
                            ModuleKind::Mining(_) => "mining",
                            ModuleKind::Harvest(_) => "harvest",
                            ModuleKind::Fishing(_) => "fishing",
                            ModuleKind::Quarter(_) => "quarter",
                            ModuleKind::SelfHealing(_) => "selfheal",
                            ModuleKind::Construct(_) => "construct",
                            ModuleKind::Attack(_) => "attack",
                            ModuleKind::FightNear(_) => "fightnear",
                            ModuleKind::Dying(_) => "dying",
                            ModuleKind::FightFar(_) => "fightfar",
                            ModuleKind::Shoot(_) => "shoot",
                            ModuleKind::Animal(_) => "animal",
                            ModuleKind::Edible(_) => "edible",
                            ModuleKind::Hunting(_) => "hunting",
                            ModuleKind::Riding(_) => "riding",
                            ModuleKind::Swim(_) => "swim",
                            ModuleKind::Diving(_) => "diving",
                            ModuleKind::Train(_) => "train",
                            ModuleKind::MultiHead(_) => "multihead",
                            ModuleKind::MultiPart(_) => "multipart",
                            ModuleKind::GlobalUpgrade(_) => "globalupgrade",
                            ModuleKind::Spell(_) => "spell",
                            ModuleKind::ExecTempUpgrade(_) => "exectempupgrade",
                            ModuleKind::TransportTent(_) => "transporttent",
                            ModuleKind::Tent(_) => "tent",
                            ModuleKind::Trading(_) => "trading",
                            ModuleKind::Masking(_) => "masking",
                            ModuleKind::Assassin(_) => "assassin",
                            ModuleKind::Healing(_) => "healing",
                            ModuleKind::EvokeGhost(_) => "evokeghost",
                            ModuleKind::Ghost(_) => "ghost",
                            ModuleKind::Banish(_) => "banish",
                            ModuleKind::EvokeAnimal(_) => "evokeanimal",
                            ModuleKind::Bomberman(_) => "bomberman",
                            ModuleKind::Killer(_) => "killer",
                            ModuleKind::SpecialMove(_) => "specialmove",
                            ModuleKind::HeadHunter(_) => "headhunter",
                            ModuleKind::Robbing(_) => "robbing",
                        }
                        .into(),
                        enabled: m.base.enabled,
                        idle: m.base.idle,
                        flags: m.base.flags,
                        timers: m.base.events.len() + m.base.timers_ws.len(),
                    })
                })
                .collect(),
            walk: self.walk_info(o),
            building: self.building_info(o),
            gatherer: self.gatherer_info(o),
            construct: self.construct_info(o),
            quarter: self.quarter_info(o),
            resource: self.resource_info(o),
            tree: self.tree_info(o),
            combat: self.combat_info(o),
            ranged: self.ranged_info(o),
            animal: self.animal_info(o),
            edible: self.edible_info(o),
            riding: self.riding_info(o),
            diving: self.diving_info(o),
            train: self.train_info(o),
            multipart: self.multipart_info(o),
            event_bits: self
                .state
                .object_event_bits
                .get(&o.id)
                .copied()
                .unwrap_or(0),
            temp_upgrades: o
                .temp_upgrades
                .iter()
                .map(|t| SimTempUpgrade {
                    guid: t.guid,
                    flags: t.flags,
                    rest_ms: t.rest_ms,
                })
                .collect(),
            show_counter_upgrade: o.show_counter_upgrade,
            armed: Self::armed_info(o),
            spell: self.spell_info(o),
            exec_list: Self::exec_list_info(o),
            tent: Self::tent_info(o),
            transport_tent: Self::transport_tent_info(o),
            trade: Self::trade_info(o),
            in_collision: self.in_collision_info(o),
            masking: Self::masking_info(o),
            assassin: Self::assassin_info(o),
            healing: self.healing_info(o),
            message_filter: Self::message_filter_info(o),
            look: masked_look(&self.state, o, self.active_player)
                .map(|(anim, alpha)| SimLookInfo { anim, alpha }),
            evoke_ghost: self.evoke_ghost_info(o),
            ghost: Self::ghost_info(o),
            banish: self.banish_info(o),
            evoke_animal: self.evoke_animal_info(o),
            influence: Self::influence_info(o),
            bomberman: self.bomberman_info(o),
            killer: self.killer_info(o),
            special_move: self.special_move_info(o),
            headhunter: Self::headhunter_info(o),
            robbing: self.robbing_info(o),
            ai_marks: (0..8).filter(|p| o.ai_marks >> p & 1 == 1).collect(),
        }
    }

    /// Tent module 42 for `sim_query` (protocol 17, re/tents 1.2, 5): the fields as in the
    /// state, plus the crossfade factor of the object and `hidden` (building sleep without selectability).
    /// `travois` is the raw ID (`u32::MAX` = none).
    fn tent_info(o: &GameObject) -> Option<SimTentInfo> {
        let ModuleKind::Tent(t) = &o.module(tent::INDEX)?.kind else {
            return None;
        };
        let sleeping = match o.module(building::INDEX).map(|m| &m.kind) {
            Some(ModuleKind::Building(b)) => b.is_sleeping,
            _ => false,
        };
        Some(SimTentInfo {
            status: t.status,
            counter: t.counter,
            progress: o.progress_scale,
            travois_guid: t.travois_guid,
            travois: t.travois.0,
            hidden: sleeping && !o.selectable,
        })
    }

    /// Travois module 35 for `sim_query` (protocol 17, re/tents 1.3): carried tent as
    /// raw ID (`u32::MAX` = none) and the flag `TRANSPORTINGTENT`.
    fn transport_tent_info(o: &GameObject) -> Option<SimTransportTentInfo> {
        let m = o.module(transport::INDEX)?;
        let ModuleKind::TransportTent(t) = &m.kind else {
            return None;
        };
        Some(SimTransportTentInfo {
            tent: t.tent.0,
            transporting: m.base.flags & transport::TRANSPORTING_TENT != 0,
        })
    }

    /// Trading module 43 for `sim_query` (protocol 17, re/trading 1.2, 5): ware, target and
    /// tax values as in the state, plus the load of the cart (it has no gatherer module).
    fn trade_info(o: &GameObject) -> Option<SimTradeInfo> {
        let ModuleKind::Trading(t) = &o.module(trading::INDEX)?.kind else {
            return None;
        };
        Some(SimTradeInfo {
            ware: t.trade_ware,
            target: t.trade_target,
            tax_level: t.tax_level,
            max_tax: t.max_tax,
            tax_interval: t.tax_interval,
            payers: t.payer_guids.clone(),
            load_amount: o.load_amount,
            load_kind: o.load_kind,
        })
    }

    /// Is the building in the collision manager? The search covers all buckets, not only the bucket
    /// of the tile: a tent that is set up on its tile is in none according to W-89
    /// (re/tents 9.3). Only for objects with module 3.
    fn in_collision_info(&self, o: &GameObject) -> Option<bool> {
        o.module(building::INDEX)?;
        Some(self.state.collision.all.iter().any(|b| b.contains(&o.id)))
    }

    /// Train module 33 for `sim_query` (protocol 15, re/train-multipart 1.1, 5); the fields
    /// are as in the state, `master`/`slaves` as raw IDs (`u32::MAX` = none).
    fn train_info(&self, o: &GameObject) -> Option<SimTrainInfo> {
        let ModuleKind::Train(t) = &o.module(train::INDEX)?.kind else {
            return None;
        };
        Some(SimTrainInfo {
            slaves: t.slaves.iter().map(|s| s.0).collect(),
            master: t.master.0,
            speed: t.speed,
            next_speed: t.next_speed,
            acc_time: t.acc_time,
            status: t.status,
            is_slave: t.is_slave,
            dir: [t.dir.0, t.dir.1],
            target: t.target.map(|(x, z)| [x, z]),
            time_counter: t.time_counter,
            has_supply: t.has_supply,
            outside: t.outside,
            raster_dir: [t.raster_dir.0, t.raster_dir.1],
            last_block_tile: [t.last_block_tile.0, t.last_block_tile.1],
        })
    }

    /// Multipart object for `sim_query` (protocol 15, re/train-multipart 1.2, 1.3): front part
    /// (module 11) and/or rear part (module 51); fields that do not apply: IDs `u32::MAX`,
    /// otherwise 0.
    fn multipart_info(&self, o: &GameObject) -> Option<SimMultiPartInfo> {
        let head = match o.module(multipart::HEAD).map(|m| &m.kind) {
            Some(ModuleKind::MultiHead(h)) => Some(h),
            _ => None,
        };
        let part = match o.module(multipart::PART).map(|m| &m.kind) {
            Some(ModuleKind::MultiPart(p)) => Some(p),
            _ => None,
        };
        if head.is_none() && part.is_none() {
            return None;
        }
        Some(SimMultiPartInfo {
            head: head.is_some(),
            linked: head.map_or(u32::MAX, |h| h.linked.0),
            prev: part.map_or(u32::MAX, |p| p.prev.0),
            next: part.map_or(u32::MAX, |p| p.next.0),
            offset: part.map_or(0, |p| p.offset),
            centre: part.map_or([0; 3], |p| [p.centre.0, p.centre.1, p.centre.2]),
            last_pos: head.map_or([0; 3], |h| [h.last_pos.0, h.last_pos.1, h.last_pos.2]),
        })
    }

    /// Riding modules 13 and 14 for `sim_query` (protocol 14, re/riding 1.1); one entry per present
    /// module, 13 first.
    fn riding_info(&self, o: &GameObject) -> Vec<SimRidingInfo> {
        [riding::RIDING, riding::RIDING_BOAT]
            .into_iter()
            .filter_map(|index| {
                let ModuleKind::Riding(r) = &o.module(index)?.kind else {
                    return None;
                };
                Some(SimRidingInfo {
                    index: index as u32,
                    boat: r.boat,
                    // No ID: `ObjectId::NONE` and the start ID 0 report `null`.
                    mount_target: Some(r.mount_target.0)
                        .filter(|&t| t != 0 && !r.mount_target.is_none()),
                    going_to_horse: r.going_to_horse,
                    busy: r.busy,
                    next_mount_anim: r.next_mount_anim,
                    lance_available: r.lance_available,
                })
            })
            .collect()
    }

    /// Diving module 24 for `sim_query` (protocol 14, re/swim-dive 1.2, 7): state, layer,
    /// air (attribute 28, current value with the upgrade percentage of the type like the other
    /// sections of `sim_query`) and depth offset of the object.
    fn diving_info(&self, o: &GameObject) -> Option<SimDivingInfo> {
        let ModuleKind::Diving(d) = &o.module(diving::INDEX)?.kind else {
            return None;
        };
        Some(SimDivingInfo {
            status: d.status,
            count: d.count,
            orig_layer: d.orig_layer,
            dive_layer: d.dive_layer,
            air: o.attributes.current_value(
                28,
                opennml_sim::upgrades::type_upgrade_percent(&self.state, o.owner(), o.guid, 28),
            ),
            height_offset: o.height_offset,
        })
    }

    /// Walk module for `sim_query` (protocol 8).
    fn walk_info(&self, o: &GameObject) -> Option<SimWalkInfo> {
        let Some(ModuleKind::Walk(w)) = o.module(0).map(|m| &m.kind) else {
            return None;
        };
        let cell = self.rules.map.cell;
        let world = |x: i32, z: i32| {
            [
                Fx(x).to_f32() * cell[0] as f32,
                Fx(z).to_f32() * cell[2] as f32,
            ]
        };
        // Goal of the FINDTO; the walk logic discards it on the way (e.g. without a trajectory in planning),
        // then the last waypoint counts.
        let goal_fx = w
            .find_to
            .as_ref()
            .map(|m| [m.arg(0), m.arg(1)])
            .or_else(|| w.high_path.back().map(|p| [p.x, p.z]));
        Some(SimWalkInfo {
            state: w.state,
            ende: w.ende,
            goal_fx,
            goal: goal_fx.map(|[x, z]| world(x, z)),
            waypoints: w.high_path.iter().map(|p| world(p.x, p.z)).collect(),
            trajectory: w.trajectory.len(),
            speed: w.speed.cur,
            cruising: w.speed.cruising,
            running: w.speed.running,
            layer: w.layer as u8,
            planner: w.last_bstar_result,
            wait_for: (w.wait_for != ObjectId::NONE).then_some(w.wait_for.0),
            group: (w.group >= 0).then_some(w.group),
            formation_num: w.formation_num,
        })
    }

    /// Overlay line: tick, engine ticks, game type, speed, active player, objects, checksum.
    pub fn status_line(&self) -> String {
        let objects: usize = self.state.tables.iter().map(|t| t.count).sum();
        format!(
            "Simulation: tick {} ({} steps) | game type {} | GameSpeed {} | player {} | {objects} objects | checksum {:016x}",
            self.state.tick,
            self.state.steps,
            self.state.mode,
            self.game_speed,
            self.active_player,
            self.checksum()
        )
    }
}

/// Bitmask of diplomacy (friends, enemies, sight) of a slot; slots from 8 on have none.
fn mask(masks: &[u8], slot: usize) -> u32 {
    masks.get(slot).map_or(0, |&m| u32::from(m))
}

/// Entries != 0 of a counter table (object counter or "in progress") for `sim_query`.
fn nonzero(counts: Option<&BTreeMap<u32, i32>>) -> BTreeMap<u32, i32> {
    counts.map_or_else(BTreeMap::new, |c| {
        c.iter()
            .filter(|&(_, &n)| n != 0)
            .map(|(&g, &n)| (g, n))
            .collect()
    })
}

/// Fallback when loading the map: if the script is switched on but a script does not pass the load check
/// (`ScriptRules::errors`), it switches the script off in the setup and returns the
/// warning; the map then starts without a script. An explicit `sim_setup`
/// with a script does not go through this and passes the load error on.
pub fn script_fallback(setup: &mut MatchSetup, rules: &Rules) -> Option<String> {
    if !setup.script.enabled {
        return None;
    }
    let first = rules.scripts.errors.first()?;
    setup.script.enabled = false;
    Some(format!(
        "Script not loaded, match without script (load errors: {}): {first}",
        rules.scripts.errors.len()
    ))
}

/// Starts the simulation of a loaded map with speed `game_speed`. If the rules cannot be
/// loaded, the map shows the objects of the `.wld` without simulation (warning); if a script does not pass the
/// load check, the match starts without a script (warning, [`script_fallback`]); an invalid
/// setup (`setup` builds it from the rules just loaded, in the viewer from the settings)
/// is an error, on the other hand.
pub fn start_sim(
    vfs: &opennml_rda::Vfs,
    scene: &LevelScene,
    game_speed: u8,
    setup: impl FnOnce(&Rules) -> anyhow::Result<MatchSetup>,
) -> anyhow::Result<Option<SimHost>> {
    start_sim_with(vfs, scene, None, game_speed, setup)
}

/// Like [`start_sim`]; with `random` (random map) the rules load without `.lvl`
/// ([`Rules::load_random`]), and their error aborts the start (`Err`, no silent "without simulation": the scene
/// of a random map has no `.wld` whose objects it could show instead).
pub fn start_sim_with(
    vfs: &opennml_rda::Vfs,
    scene: &LevelScene,
    random: Option<&RandomMapRules>,
    game_speed: u8,
    setup: impl FnOnce(&Rules) -> anyhow::Result<MatchSetup>,
) -> anyhow::Result<Option<SimHost>> {
    let rules = if let Some(random) = random {
        Rules::load_random(vfs, scene, random.clone())
            .with_context(|| format!("{}: loading rules of the random map", scene.lvl_path))?
    } else {
        match Rules::load(vfs, scene) {
            Ok(rules) => rules,
            Err(error) => {
                log::warn!(
                    "{}: rules not loaded, map without simulation: {error:#}",
                    scene.lvl_path
                );
                return Ok(None);
            }
        }
    };
    let mut setup = setup(&rules)?;
    if let Some(warning) = script_fallback(&mut setup, &rules) {
        log::warn!("{}: {warning}", scene.lvl_path);
    }
    let mut host = SimHost::new(rules, setup, game_speed)
        .map_err(anyhow::Error::msg)
        .context("setting up the match")?;
    host.upgrade_texts = crate::game::texts::load_upgrade_texts(vfs, &host.rules);
    Ok(Some(host))
}

/// Module indices whose animation does not come from the simulation yet (buildings): their models
/// keep showing the idle animation. Train, front and rear part play the animation of the simulation
/// (locomotive and slaves get the numbers of the locomotive, re/train-multipart 7).
const ANIMATED_LATER: [u32; 1] = [3];

/// Yaw of the model for the object direction `dir` (fixed-point degrees): `rad(dir + 90 degrees)` like
/// `GameObject_SetDirection` and `PropertyWalk_FrameUpdate`.
fn yaw_of_direction(dir: i32) -> f32 {
    dir.wrapping_add(90 << 12) as f32 / 4096.0 * f32::from_bits(0x3C8E_FA35)
}

/// Train state (module 33) of an object.
/// Speed of the wheel animation of a train object ([`SimHost::train_wheel_rate`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelRate {
    /// Playback rate of the carrier (1.0 = normal) from the speed of the locomotive; the sub-objects keep
    /// the speed of the simulation.
    Derived(f32),
    /// The previous rate stays.
    Keep,
}

/// Playback rate of the wheel animation at cell `sx` (`GameWorld+0x34`) and speed `v` (fixed point per
/// tick): speed `f32(Fixed_ToFloat(sx * v * 10) * 1.5)` (`0x0052F3F0`, product in 32 bits, x87 exact), through
/// `AnimInstance_SetTimeScale` (`0x0058DA30`) as `tempo * 0.0078125`, speed <= 0 as 128.
fn wheel_rate(sx: i32, v: i32) -> f32 {
    let tempo = (f64::from(sx.wrapping_mul(v).wrapping_mul(10)) / 4096.0 * 1.5) as f32;
    let tempo = if tempo <= 0.0 { 128.0 } else { tempo };
    tempo * 0.007_812_5
}

fn train_of(o: &GameObject) -> Option<&Train> {
    match &o.module(train::INDEX)?.kind {
        ModuleKind::Train(t) => Some(t),
        _ => None,
    }
}

/// Which animation the rendering shows for a simulation object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayAnim {
    /// Animation of the simulation (index in the object `.cfg`; -1 = none).
    Sim(i32),
    /// Idle animation of the model (module still inert).
    Idle,
}

pub fn display_anim(rules: &Rules, object: &GameObject) -> DisplayAnim {
    let later = rules
        .types
        .get(&object.guid)
        .is_some_and(|t| ANIMATED_LATER.iter().any(|&m| t.has_module(m)));
    if object.anim.current < 0 && later {
        DisplayAnim::Idle
    } else {
        DisplayAnim::Sim(object.anim.current)
    }
}

/// Silhouettes only while walking (`ObjectRenderer_Draw` `0x0054A3E1`, re/world-display W.2.1, D-263):
/// if the sight check for a foreign camouflaged unit delivers only the special bit `0x80`, the original
/// draws it as a silhouette only with `MOVE`, standing not at all - no own effects either.
/// The host then passes on render bits 0 (the renderer draws `0x80` as a silhouette).
pub fn stealth_bits(bits: u32, object_status: u32) -> u32 {
    if bits == opennml_render::scene::RENDER_STEALTH && object_status & status::MOVE == 0 {
        0
    } else {
        bits
    }
}

/// Camouflage in the rendering for the viewer `viewer` (re/masking 7, K-13, D-235):
/// `(animation, opacity)`, the animation `None` = that of the simulation; `None` without camouflage
/// (`MASKED`) and for every object without module 25.
///
/// - Unit with module 25 (no building module 3): alpha 0.5 for every viewer (`0x0051D740` ->
///   `0x00468090`); foreigners additionally see only render bit `0x80`
///   ([`crate::game::fog_view::render_bits_for`]), the silhouette sets its own alpha 0.1.
/// - Camouflaged without module 25 (dug-in assassin, module 28 `0x004FFA00`): no alpha, so opaque
///   for the owner and shared sight (re/world-display W.2.2, D-267).
/// - Building with module 25 (alarm post): `PropertyMasking` in the original remembers the local player
///   (`m_DummyAktPlayer` `+0x88`, K-13) and chooses according to `HasSharedVision(owner, local)`: with
///   sight `260001[0]` (tree with post, which is what the simulation always sets) and alpha 0.5, otherwise
///   `260001[1 % n]` (tree only) opaque. The direction is the reverse of the object visibility
///   (`shares_vision(viewer, owner)`), D-235; the reimplementation adopts it.
pub fn masked_look(
    state: &SimState,
    object: &GameObject,
    viewer: u8,
) -> Option<(Option<i32>, f32)> {
    if object.status & status::MASKED == 0 {
        return None;
    }
    // `HasModule(3, 0)` like `PropertyMasking` itself. Only module 25 (`0x319`) sets the alpha 0.5.
    if !object.has_module(3, false) {
        return object
            .has_module(masking::INDEX, false)
            .then_some((None, MASKED_ALPHA));
    }
    let ModuleKind::Masking(m) = &object.module(masking::INDEX)?.kind else {
        return None;
    };
    if state
        .diplomacy
        .shares_vision(i32::from(object.owner()), i32::from(viewer))
    {
        Some((None, MASKED_ALPHA))
    } else {
        // `IntList_GetModulo(1)`; an empty list (loading no longer checks it) gives no choice.
        let tree = *m.anim.get(1 % m.anim.len().max(1))?;
        Some((Some(tree), 1.0))
    }
}

/// Own opacity of a ghost or animal: the ghost `Ghost.alpha`
/// (`GameObject_SetAlpha` `0x00468090` in the fade-out `0x005159F0`,
/// re/ghosts-banish 3.4, 7), the animal `Animal.alpha` (`PropertyAnimal_Tick` `0x004FDF90`,
/// re/evoke-aura 7, re/animals 1.4). This applies to **all** animals with module 20, also to the
/// fade-out via `0x114`, not only to the appearance of evoked animals via `0x414`.
/// `None` without both modules.
pub fn fade_alpha(object: &GameObject) -> Option<f32> {
    let kind = |index| object.module(index).map(|m| &m.kind);
    if let Some(ModuleKind::Ghost(g)) = kind(opennml_sim::module::ghosts::ghost::INDEX) {
        return Some(g.alpha);
    }
    match kind(opennml_sim::module::animal::INDEX) {
        Some(ModuleKind::Animal(a)) => Some(a.alpha),
        _ => None,
    }
}

/// [`display_anim`] for the viewer `viewer`: the animation of a camouflage ([`masked_look`])
/// replaces that of the simulation (alarm post for foreigners).
pub fn viewer_anim(
    state: &SimState,
    rules: &Rules,
    object: &GameObject,
    viewer: u8,
) -> DisplayAnim {
    match masked_look(state, object, viewer) {
        Some((Some(anim), _)) => DisplayAnim::Sim(anim),
        _ => display_anim(rules, object),
    }
}

/// Object list of the map from the simulation: ambient effects of the `.wld` unchanged, then all
/// simulation objects in table order (slot, index) whose type has a loadable model.
/// `index` is then the position in the list.
pub fn map_objects(
    scene: &LevelScene,
    state: &SimState,
    model_exists: impl Fn(&str) -> bool,
) -> Vec<PlacedObject> {
    let mut objects: Vec<PlacedObject> = scene
        .objects
        .iter()
        .filter(|o| o.class == ObjectClass::Ambient)
        .cloned()
        .collect();
    let cell = scene.terrain.params.size_x;
    for o in state.objects() {
        let Some(kind) = scene.project.objects.get(&o.guid) else {
            continue;
        };
        let model = opennml_rda::normalize_path(&kind.model);
        if !model_exists(&model) {
            continue;
        }
        let position = Vec3::from(o.world_pos);
        let rotation = Vec3::from(o.rotation);
        let tile = (
            (position.x / cell).floor() as i64,
            (position.z / cell).floor() as i64,
        );
        objects.push(PlacedObject {
            index: 0,
            guid: o.guid,
            class: ObjectClass::Game,
            type_name: kind.name.clone(),
            name: None,
            model,
            player: Some(i32::from(o.owner())),
            parts: vec![o.guid],
            no_shadow: in_no_shadow_area(&scene.world.areas, tile),
            position,
            rotation,
            transform: object_world_matrix(position, rotation, Vec3::from(o.scale)),
            sim_id: Some(o.id.0),
        });
    }
    for (i, o) in objects.iter_mut().enumerate() {
        o.index = i;
    }
    objects
}

/// Common test types of the viewer tests on riding and diving: rider, horse and
/// diver with walk and stand like `opennml_sim::module::riding::testkit` (values there), built with
/// `Rules::synthetic`; the `opennml-sim` testkit is not visible in the viewer.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use opennml_sim::world::{ObjectDesc, create_object};

    /// ENG_Dragoner: rider (land).
    pub const RIDER: u32 = 2007;
    /// Horse (land, `RUN`, `SADDLE`).
    pub const HORSE: u32 = 3410;
    /// WID_Gewehrschuetze as diver (land + water, stand with `ONWATER`).
    pub const DIVER: u32 = 1403;
    /// WID_Krieger: foot troops (land), occupant and comparison figure of the double-click.
    pub const WARRIOR: u32 = 1400;
    /// WID_Kanu: watercraft (walk `0x32200`, without `SADDLE`), carries an occupant.
    pub const CANOE: u32 = 1408;

    /// Land (`2048`) or land + water (`0x42900`) and the horse flags (`0x20C900`).
    const LAND: u32 = 2048;
    const AMPHIBIOUS: u32 = 0x42900;
    const HORSE_FLAGS: u32 = 0x20_C900;
    const CANOE_FLAGS: u32 = 0x3_2200;
    /// Stand flag `ONWATER` (30003): the diver stays at water height in the simulation.
    const ONWATER: u32 = 0x100;

    /// Unit with walk (hard radius 512, soft 1024) and stand with the flags `stand_flags`;
    /// `modules` appends further modules.
    fn unit(guid: u32, name: &str, walk_flags: u32, stand_flags: u32, modules: &str) -> String {
        format!(
            "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\n\
             VALUEID 10003 6 1 {walk_flags}\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\n\
             VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n\
             PROPID 2\nVALUEID 30001 1 1 0\nVALUEID 30003 1 1 {stand_flags}\n{modules}"
        )
    }

    /// Setup like `match_setup` of the viewer with its defaults (`Config::default()`): default
    /// of the map, AI on (campaign difficulty 1).
    pub fn default_setup(rules: &Rules) -> anyhow::Result<MatchSetup> {
        let mut setup = MatchSetup::default_for(rules);
        setup.ki.enabled = true;
        Ok(setup)
    }

    /// Type text with rider, horse, diver, warrior and canoe.
    pub fn types() -> String {
        unit(RIDER, "ENG_Dragoner", LAND, 0, "PROPID 13\n")
            + &unit(HORSE, "ENG_Pferd", HORSE_FLAGS, 0, "")
            + &unit(WARRIOR, "WID_Krieger", LAND, 0, "")
            + &unit(CANOE, "WID_Kanu", CANOE_FLAGS, 0, "")
            + &unit(
                DIVER,
                "WID_Gewehrschuetze",
                AMPHIBIOUS,
                ONWATER,
                "PROPID 24\n",
            )
    }

    /// Host on a 32 x 32 map (cells 128) with active player 0, speed 183 for all types.
    pub fn host() -> SimHost {
        host_with(&types())
    }

    /// Like [`host`] with the type text `types` (rider, horse and diver get their speed).
    pub fn host_with(types: &str) -> SimHost {
        let mut rules = Rules::synthetic(types, 32);
        for guid in [RIDER, HORSE, DIVER] {
            rules
                .defaults
                .insert(guid, [(16, 183), (28, 500)].into_iter().collect());
        }
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host
    }

    /// Creates an object of `guid` for player 0 at the world position `(x, z)`.
    pub fn spawn(host: &mut SimHost, guid: u32, x: f32, z: f32) -> ObjectId {
        spawn_for(host, guid, 0, x, z)
    }

    /// Like [`spawn`] for the player `owner`.
    pub fn spawn_for(host: &mut SimHost, guid: u32, owner: i32, x: f32, z: f32) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        create_object(&mut sim, &mut ObjectDesc::new(guid, owner, [x, 0.0, z])).unwrap()
    }

    /// Sets a compound directly (like `riding::testkit::link`): `target` on both sides, the rider gets
    /// `ISMOUNTED`, the mount `CARRIING`.
    pub fn link(host: &mut SimHost, rider: ObjectId, mount: ObjectId) {
        let r = host.state.object_mut(rider, false).unwrap();
        r.target = mount;
        r.set_status_flags(status::ISMOUNTED, true);
        let m = host.state.object_mut(mount, false).unwrap();
        m.target = rider;
        m.set_status_flags(status::CARRIING, true);
    }

    /// Occupant in a boat (like `link`): the occupant gets `ISMOUNTEDONBOAT`, the boat `CARRIING`.
    pub fn link_boat(host: &mut SimHost, passenger: ObjectId, boat: ObjectId) {
        let p = host.state.object_mut(passenger, false).unwrap();
        p.target = boat;
        p.set_status_flags(status::ISMOUNTEDONBOAT, true);
        let b = host.state.object_mut(boat, false).unwrap();
        b.target = passenger;
        b.set_status_flags(status::CARRIING, true);
    }

    // --- Train, cart and ships ---

    pub use opennml_sim::module::train::{LOCO, TENDER, TRACK, WAGON};

    /// SID_PostkutscheVorne (walk, module 11, rear part 2620).
    pub const CART: u32 = 2611;
    /// SID_PostkutscheHinten (module 51).
    pub const CART_TAIL: u32 = 2620;
    /// SPA_Transportschiff (walk `0x22200`, stand `ONWATER`, quarters 12).
    pub const TRANSPORT: u32 = 1108;
    /// SPA_Kriegsschiff (walk `0x22400`, stand `ONWATER`, dying, shot module `SHIP`, attack,
    /// ranged combat).
    pub const WARSHIP: u32 = 1109;

    /// Train module 33 with the flags 340001 and the four animation numbers (shipped data 0, 2, 1, 1).
    fn train_module(flags: u32) -> String {
        format!(
            "PROPID 33\nVALUEID 340001 6 1 {flags}\nVALUEID 340002 6 1 0\nVALUEID 340003 6 1 2\n\
             VALUEID 340004 6 1 1\nVALUEID 340005 6 1 1\n"
        )
    }

    /// Types of locomotive (512), tender, wagon (with quarters), track (`TRACK`), stagecoach (front and
    /// rear part) and transport ship (like `opennml_sim::module::train::testkit` and `boarding::testkit`,
    /// which the viewer does not see).
    pub fn train_types() -> String {
        let walk = |guid: u32, name: &str, flags: u32, stand: &str, extra: &str| {
            format!(
                "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\n\
                 VALUEID 10003 6 1 {flags}\nVALUEID 10004 6 1 1900\nVALUEID 10005 6 1 4096\n\
                 VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n{stand}{extra}"
            )
        };
        [
            format!(
                "GUID {LOCO}\nNAME SID_Lokomitive\nVALUEID 100 6 1 0\n{}",
                train_module(512)
            ),
            format!(
                "GUID {TENDER}\nNAME SID_Tender\nVALUEID 100 6 1 0\n{}",
                train_module(256)
            ),
            format!(
                "GUID {WAGON}\nNAME SID_Wagon\nVALUEID 100 6 1 0\nPROPID 23\n{}",
                train_module(256)
            ),
            format!(
                "GUID {TRACK}\nNAME Schienen\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 33554432\n"
            ),
            walk(
                CART,
                "SID_PostkutscheVorne",
                2048,
                "",
                &format!("PROPID 11\nVALUEID 120001 6 1 {CART_TAIL}\n"),
            ),
            format!(
                "GUID {CART_TAIL}\nNAME SID_PostkutscheHinten\nVALUEID 100 6 1 0\nPROPID 51\n\
                 VALUEID 520001 6 1 0\nVALUEID 520002 6 1 1900\nVALUEID 520003 6 1 0\n\
                 VALUEID 520004 6 1 1\nVALUEID 520005 6 1 2\nVALUEID 520006 6 1 5300\n"
            ),
            // Warship (re/ships 1.1/1.2): starboard 2, port 3, standing 0.
            walk(
                WARSHIP,
                "SPA_Kriegsschiff",
                140_288,
                "PROPID 2
VALUEID 30001 6 1 0
VALUEID 30003 6 1 256
",
                "PROPID 16
VALUEID 170001 6 1 0
VALUEID 170002 6 1 4
                 PROPID 10
VALUEID 110001 6 1 131072
VALUEID 110002 1 1 2
VALUEID 110003 1 1 3
                 VALUEID 110004 1 1 0
PROPID 17
VALUEID 180001 6 1 133120
VALUEID 180002 6 1 0
                 PROPID 19
VALUEID 200001 6 1 256
",
            ),
            // Transport ship: walk layer 6, stand `ONWATER`, dying, quarters.
            walk(
                TRANSPORT,
                "SPA_Transportschiff",
                139_776,
                "PROPID 2\nVALUEID 30001 6 1 0\nVALUEID 30003 6 1 256\n",
                "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\nPROPID 23\n",
            ),
        ]
        .concat()
    }

    /// Host on a 64 x 64 map (land) with train, cart and ship: train nodes like in the testkit of the
    /// simulation (locomotive `vhinten01` at `(0, 0, 100)`, tender `60` / `-60`, wagon `60` / `-80`),
    /// hit points for the ship, active player 0.
    pub fn train_host() -> SimHost {
        use opennml_sim::rules::TrainNodes;
        let mut rules = Rules::synthetic(&train_types(), 64);
        for guid in [LOCO, TENDER, WAGON, CART, CART_TAIL, TRANSPORT, WARSHIP] {
            rules.defaults.insert(
                guid,
                [(14, 1400), (16, 183), (18, 32)].into_iter().collect(),
            );
        }
        rules.defaults.insert(
            WAGON,
            [(14, 100), (16, 183), (18, 32), (23, 10)]
                .into_iter()
                .collect(),
        );
        rules.defaults.insert(
            TRANSPORT,
            [(14, 1400), (16, 183), (18, 52), (23, 12)]
                .into_iter()
                .collect(),
        );
        let node = |hitch: Option<f32>, coupling: Option<f32>| TrainNodes {
            hitch: hitch.map(|z| [0.0, 0.0, z]),
            coupling: coupling.map(|z| [0.0, 0.0, z]),
        };
        rules.train_nodes.insert(LOCO, node(Some(100.0), None));
        rules
            .train_nodes
            .insert(TENDER, node(Some(60.0), Some(-60.0)));
        rules
            .train_nodes
            .insert(WAGON, node(Some(60.0), Some(-80.0)));
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host
    }

    /// Finished object at the center of tile `tile` with the direction `dir` in degrees (multiples of
    /// 90 degrees); the rotation is the one `world::set_direction` writes (`rad(dir + 90 degrees)`).
    pub fn spawn_dir(
        host: &mut SimHost,
        guid: u32,
        owner: i32,
        tile: (i32, i32),
        dir: i32,
    ) -> ObjectId {
        use opennml_sim::world::desc_flags;
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, owner, [at(tile.0), 0.0, at(tile.1)]);
        desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
        desc.rotation[1] = ((dir + 90) << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
        create_object(&mut sim, &mut desc).expect("object created")
    }

    /// Finished track pieces from `from` to `to` along an axis, direction from the route.
    pub fn lay_track(host: &mut SimHost, from: (i32, i32), to: (i32, i32)) {
        let (dx, dz) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
        let dir = match (dx, dz) {
            (-1, _) => 180,
            (_, 1) => 90,
            (_, -1) => -90,
            _ => 0,
        };
        let n = (to.0 - from.0).abs().max((to.1 - from.1).abs());
        for i in 0..=n {
            spawn_dir(host, TRACK, 0, (from.0 + i * dx, from.1 + i * dz), dir);
        }
    }

    /// Command `0x29`/`0x2A`/`0x2B` to the locomotive; takes effect in the next step.
    pub fn train_command(host: &mut SimHost, train: ObjectId, op: opennml_sim::command::TrainOp) {
        host.submit(Command {
            player: 0,
            kind: CommandKind::Train {
                train,
                op,
                target: None,
            },
        });
    }

    /// One object tick of the simulation (two steps).
    pub fn object_tick(host: &mut SimHost) {
        host.step_once();
        host.step_once();
    }

    /// Coast like `boarding::testkit::shore_state`: land for `x < 10`, water (layer 4) for
    /// `x >= 10`; the ships move in the water (layer 6 free for `x >= 11`).
    pub fn make_coast(host: &mut SimHost) {
        let dim = host.rules.map.dim_x;
        let path = &mut host.state.path;
        for z in 0..dim {
            for x in 0..dim {
                path.mark_cell(0x1F, x, z, true);
            }
        }
        for z in 0..dim {
            for x in 0..dim {
                path.mark_cell(if x < 10 { 1 } else { 1 << 4 }, x, z, false);
            }
        }
        path.flush_dirty_rect();
        path.build_regions(u32::MAX);
    }
}

/// Smallest test world of the simulation (colonist and headquarters on a 16 x 16 map), also for the
/// tests of the settings in the viewer.
#[cfg(any(test, feature = "testkit"))]
pub mod plain_testkit {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use opennml_sim::world::{ObjectDesc, create_object};

    pub const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\n";

    pub fn host() -> SimHost {
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [64.0, 0.0, 64.0])).unwrap();
        create_object(&mut sim, &mut ObjectDesc::new(1000, 0, [640.0, 0.0, 64.0])).unwrap();
        host
    }
}

#[cfg(test)]
mod tests {
    use super::plain_testkit::{TYPES, host};
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use opennml_sim::world::{ObjectDesc, create_object};

    /// Host with the level script `level` and a script that is (not) switched on, without objects.
    fn script_host(level: &str, enabled: bool) -> SimHost {
        let rules = crate::game::testworld::script_rules(level);
        let mut setup = MatchSetup::default_for(&rules);
        setup.script.enabled = enabled;
        SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap()
    }

    /// Type codes of the commands in the log.
    fn logged_codes(host: &SimHost) -> Vec<u32> {
        host.log
            .iter()
            .map(|c| c.command.kind.type_code())
            .collect()
    }

    fn selections(host: &SimHost) -> Vec<Vec<ObjectId>> {
        host.pending_commands()
            .iter()
            .filter_map(|c| match &c.kind {
                CommandKind::ScriptSelection { ids } => Some(ids.clone()),
                _ => None,
            })
            .collect()
    }

    /// `GameWorld_BuildRenderListsAndFrameUpdate` (`0x0047FC70`): the rendering does not draw objects of a hidden
    /// slot (render bits 0, so no model, no shadow, no click target), unless they are in the special list `L[p]`
    /// of the slot.
    // QUIRK E-253: kept
    #[test]
    fn objects_of_a_hidden_slot_are_not_drawn_unless_listed_for_slots_0_to_8() {
        let mut host = host();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let a = create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [96.0, 0.0, 96.0])).unwrap();
        let b = create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [128.0, 0.0, 96.0])).unwrap();
        host.set_fog_shown(false);
        let drawn = |host: &mut SimHost, id| host.entry_look(id, 0).unwrap().bits != 0;
        assert!(drawn(&mut host, a) && drawn(&mut host, b));
        host.state.slot_running[1] = false;
        host.state.inactive_lists[1].push(b);
        assert!(!drawn(&mut host, a), "hidden");
        assert!(drawn(&mut host, b), "special list of slot 1");
        let o = host.state.object(a, false).unwrap();
        assert!(hidden_from_view(&host.state, o));
    }

    /// The host sends `ScriptSelection` only when the set changes and
    /// only if the running script uses 35-38; otherwise the command log stays free of it.
    // QUIRK E-230: deviation
    #[test]
    fn sync_selection_sends_only_changes_and_only_with_ui_conditions() {
        use crate::game::testworld::abl;
        let (a, b) = (ObjectId(0x800), ObjectId(0x801));
        let with_35 = abl(&[(&["&35;x"], &["n10"])]);
        let mut host = script_host(&with_35, true);
        assert!(host.rules.scripts.uses_ui_conditions);
        // Empty against an empty mirror: nothing.
        host.sync_selection(&[]);
        assert!(host.pending_commands().is_empty());
        host.sync_selection(&[a, b]);
        assert_eq!(selections(&host), [vec![a, b]]);
        // The same set (also rearranged) against the last queued command: nothing new.
        host.sync_selection(&[b, a]);
        host.sync_selection(&[a, b]);
        assert_eq!(selections(&host).len(), 1);
        // A change appends a command; all go into the log with the next tick.
        host.sync_selection(&[a]);
        assert_eq!(selections(&host), [vec![a, b], vec![a]]);
        host.step_once();
        assert!(host.pending_commands().is_empty());
        assert_eq!(host.state.script.mission.selection, [a]);
        assert_eq!(logged_codes(&host).len(), 2);
        // After the tick the host syncs against the mirror: the same set sends nothing, the empty one does.
        host.sync_selection(&[a]);
        assert!(host.pending_commands().is_empty());
        host.sync_selection(&[]);
        assert_eq!(selections(&host), [Vec::<ObjectId>::new()]);
        host.step_once();
        assert!(host.state.script.mission.selection.is_empty());
        // The host truncates more than 30 entries to 30 (the command would otherwise be rejected every frame).
        let many: Vec<ObjectId> = (0..40).map(|i| ObjectId(0x800 + i)).collect();
        host.sync_selection(&many);
        assert_eq!(selections(&host), [many[..30].to_vec()]);
        host.step_once();
        host.sync_selection(&many);
        assert!(host.pending_commands().is_empty(), "no continuous firing");

        // After a restart/loading (`reset_to` empties `pending`) the host reports the selection anew.
        host.sync_selection(&[b]);
        let state = host.state.clone();
        host.reset_to(state);
        assert!(host.pending_commands().is_empty());

        // Without condition 35-38 in the script: never a command, also not with a changing selection.
        let without = abl(&[(&["&5;x"], &["n10"])]);
        let mut host = script_host(&without, true);
        assert!(!host.rules.scripts.uses_ui_conditions);
        for ids in [vec![a], vec![a, b], vec![]] {
            host.sync_selection(&ids);
            host.step_once();
        }
        assert!(host.pending_commands().is_empty());
        assert!(
            host.log.iter().all(|c| !matches!(
                c.command.kind,
                CommandKind::ScriptSelection { .. } | CommandKind::ScriptButton { .. }
            )),
            "{:?}",
            logged_codes(&host)
        );
        // With 35 but a script switched off (match without a running script): also nothing.
        let mut host = script_host(&with_35, false);
        host.sync_selection(&[a]);
        host.press_button(7);
        assert!(host.pending_commands().is_empty());
        host.step_once();
        assert_eq!(
            host.state.script,
            opennml_sim::script::ScriptState::default()
        );
    }

    /// `press_button` sends the button value only with a running script with 35-38; every
    /// report arrives (no sync: even the same button twice is an event).
    // QUIRK E-229: deviation
    #[test]
    fn press_button_without_ui_conditions_sends_nothing() {
        use crate::game::testworld::abl;
        let without = abl(&[(&["&5;x"], &["n10"])]);
        let mut host = script_host(&without, true);
        host.press_button(2904);
        assert!(host.pending_commands().is_empty());
        host.step_once();
        assert!(logged_codes(&host).is_empty());
        assert_eq!(host.state.script.mission.last_button, 0);

        let with_38 = abl(&[(&["&38;0;2904"], &["n10", "n361;gedrueckt;1"])]);
        let mut host = script_host(&with_38, true);
        host.press_button(2904);
        host.press_button(2904);
        assert_eq!(
            host.pending_commands()
                .iter()
                .map(|c| c.kind.clone())
                .collect::<Vec<_>>(),
            vec![CommandKind::ScriptButton { word: 2904 }; 2]
        );
        host.step_once();
        assert_eq!(host.state.script.mission.last_button, 2904);
        assert_eq!(logged_codes(&host), [0x5000_0000 | 0xF7; 2]);
        // The script saw the button in the same tick (host commands run before the script tick).
        let hits = host.state.script.level.as_ref().unwrap().variables["gedrueckt"];
        assert_eq!(hits, 1);
    }

    /// O-318: if the match halts (`halted`), `ScriptButton` goes into the state immediately via `submit`, like every
    /// command, not to the tick boundary.
    // QUIRK O-318: kept
    #[test]
    fn press_button_while_halted_executes_at_once() {
        use crate::game::testworld::abl;
        let with_38 = abl(&[(&["&38;0;2904"], &["n10"])]);
        let mut host = script_host(&with_38, true);
        host.halted = true;
        let steps = host.state.steps;
        host.press_button(2904);
        assert!(
            host.pending_commands().is_empty(),
            "not at the tick boundary"
        );
        assert_eq!(host.state.script.mission.last_button, 2904, "immediately");
        assert_eq!(host.state.steps, steps);
        assert_eq!(logged_codes(&host), [0x5000_0000 | 0xF7]);
    }

    /// O-318: `flush_pending` executes the waiting commands in order immediately; a command executed
    /// immediately afterwards follows them in the log and in the state, the savegame contains them, and the
    /// replay reaches the same checksum.
    // QUIRK O-318: kept
    #[test]
    fn flushed_pending_commands_run_before_later_immediate_ones() {
        // Without objects inserted afterwards (like `immediate_commands_replay_like_the_next_step`): the replay
        // begins at the start package.
        let mut rules = Rules::synthetic(TYPES, 16);
        rules.level.players = (0..2)
            .map(|slot| opennml_formats::config::LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation: 0,
                kind: 0,
                resources: Vec::new(),
            })
            .collect();
        let setup = MatchSetup {
            mode: -1,
            living_space: 2,
            ..MatchSetup::default()
        };
        let mut live = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        for _ in 0..3 {
            live.step_once();
        }
        let a = friendship(0, 1);
        live.submit(a.clone());
        live.halted = true;
        live.flush_pending();
        assert!(live.pending_commands().is_empty());
        assert!(
            live.state.diplomacy.is_friend(0, 1),
            "A takes effect immediately"
        );
        assert!(
            live.save().state.diplomacy.is_friend(0, 1),
            "the savegame of the pause contains A"
        );
        let b = opennml_sim::command::Command {
            player: 0,
            kind: CommandKind::Diplomacy {
                p: 0,
                q: 1,
                mode: 1,
            },
        };
        live.submit(b.clone());
        assert!(
            live.state.diplomacy.is_enemy(0, 1) && !live.state.diplomacy.is_friend(0, 1),
            "B after A"
        );
        let logged: Vec<_> = live
            .log
            .iter()
            .map(|c| (c.step, c.command.clone()))
            .collect();
        assert_eq!(logged, [(3, a), (3, b)]);
        live.halted = false;
        for _ in 0..3 {
            live.step_once();
        }
        let replay = Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        let played = replay.play(&live.rules, live.state.steps).unwrap();
        assert_eq!(checksum(&played), live.checksum(), "replay equal");
    }

    #[test]
    fn messages_to_everyone_are_recorded_at_every_slot() {
        // Diplomacy reports 231-233 to everyone (`to: None`): the list of every slot 0-7 carries
        // them, with tick and without location.
        let mut host = host();
        host.active_player = 0;
        host.submit_own(opennml_sim::command::CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 1,
        });
        let step = host.state.steps;
        let events = host.step_once();
        assert!(events.iter().any(|e| matches!(
            e,
            SimEvent::PlayerMessage {
                to: None,
                id: 232,
                ..
            }
        )));
        for slot in 0..8u8 {
            let ids: Vec<(u64, i32, [i32; 2])> = host
                .recent_messages(slot)
                .iter()
                .map(|n| (n.step, n.id, n.tile))
                .collect();
            assert_eq!(ids, [(step, 232, [-1, -1])], "slot {slot}");
        }
    }

    const WALKER: &str = "GUID 1100
NAME Kolonist
VALUEID 100 6 1 0
PROPID 0
VALUEID 10001 6 1 3
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
PROPID 2
VALUEID 30001 1 1 0
";

    #[test]
    fn query_reports_ai_marks_and_attack_reports() {
        // Protocol 23: `ai_marks` per object (set slots ascending, empty is missing) and
        // `attack_reports` per slot in MFC order (not in insertion order).
        use opennml_sim::id::ObjectId;
        use opennml_sim::state::{Control, ReportTile};
        let mut host = host();
        let colonist = host.query(None, Some(1100), None, 1).objects[0].id;
        assert!(
            host.query(None, None, None, 10).objects[0]
                .ai_marks
                .is_empty()
        );
        host.state
            .object_mut(ObjectId(colonist), false)
            .unwrap()
            .ai_marks = 0b1001_0010;
        let marks = |host: &SimHost| {
            host.query(None, Some(1100), None, 1).objects[0]
                .ai_marks
                .clone()
        };
        assert_eq!(marks(&host), [1, 4, 7]);
        // Each mark is a bit of the byte; the HQ row stays empty.
        assert!(
            host.query(None, Some(1000), None, 1).objects[0]
                .ai_marks
                .is_empty()
        );

        // Slot 1 is AI and has messages for the indices 0, 1, 2 (buckets 9, 8, 7 of 17): the
        // MFC order is 2, 1, 0, the insertion order 0, 1, 2.
        host.state.players[1].control = Control::Ai;
        let victim = |i| ObjectId::new(i, 1, 1);
        for (i, (x, z)) in [(0, (5, 6)), (1, (-3, 8)), (2, (0, 0))] {
            host.state.attack_reports[1].insert(victim(i).as_i32(), ReportTile { x, z });
        }
        let players = host.query(None, None, None, 1).players;
        assert!(
            players[0].attack_reports.is_empty(),
            "slot 0 without report"
        );
        let reports: Vec<(u32, [i32; 2])> = players[1]
            .attack_reports
            .iter()
            .map(|r| (r.victim, r.tile))
            .collect();
        assert_eq!(
            reports,
            [
                (victim(2).0, [0, 0]),
                (victim(1).0, [-3, 8]),
                (victim(0).0, [5, 6]),
            ]
        );
        let value = serde_json::to_value(&players[1]).unwrap();
        assert_eq!(value["attack_reports"][0]["victim"], victim(2).0);
        assert!(
            serde_json::to_value(&players[0])
                .unwrap()
                .get("attack_reports")
                .is_none()
        );
    }

    #[test]
    fn right_clicks_move_own_units_and_walkers_are_shown_ahead_of_the_simulation() {
        let mut rules = Rules::synthetic(WALKER, 32);
        rules
            .defaults
            .insert(1100, [(16, 183)].into_iter().collect());
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, 10).unwrap();
        host.active_player = 0;
        let rules = host.rules.clone();
        let (own, foreign) = {
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            let mut unit = |slot, z| {
                create_object(&mut sim, &mut ObjectDesc::new(1100, slot, [704.0, 0.0, z])).unwrap()
            };
            (unit(0, 704.0), unit(1, 1600.0))
        };
        // Foreign units are dropped; without an own unit no command.
        assert!(!host.move_units(
            &[foreign],
            Vec3::new(1600.0, 0.0, 704.0),
            None,
            FormationOrder::default()
        ));
        assert!(host.move_units(
            &[own, foreign],
            Vec3::new(1600.0, 0.0, 704.0),
            None,
            FormationOrder::default()
        ));
        let CommandKind::Move { units, x, .. } = &host.pending[0].kind else {
            panic!("move expected");
        };
        assert_eq!((units.clone(), *x), (vec![own], 12 << 12 | 0x800));
        // Let it run until the unit is on its way, then half a tick.
        while host.state.object(own, false).unwrap().status & 1 == 0 {
            host.advance(50.0);
        }
        host.advance(50.0);
        host.advance(25.0);
        let o = host.state.object(own, false).unwrap().clone();
        let (shown, yaw) = host.walk_display(&o).expect("walking");
        // The rendering lies between grid and next point, i.e. ahead of the simulation.
        assert!(
            shown.x > o.world_pos[0] && shown.z == o.world_pos[2],
            "{shown} {:?}",
            o.world_pos
        );
        // Still turning from -90 degrees (placement) to 0 degrees (direction +x).
        assert!(yaw > 0.0 && yaw <= std::f32::consts::FRAC_PI_2, "{yaw}");
        // Control channel: walk state with target and waypoints, counter of the moving objects.
        let walk = host
            .query(None, None, Some(&[own.0]), 1)
            .objects
            .remove(0)
            .walk
            .expect("walk info");
        assert_eq!(walk.goal_fx, Some([12 << 12 | 0x800, 5 << 12 | 0x800]));
        assert_eq!(walk.goal, Some([1600.0, 704.0]));
        assert!(walk.waypoints.last() == Some(&[1600.0, 704.0]) && walk.speed > 0);
        assert_eq!(host.section(&[]).moving, 1);
        // Shift queues.
        assert!(host.move_units(
            &[own],
            Vec3::new(64.0, 0.0, 704.0),
            Some(false),
            FormationOrder::default()
        ));
        assert!(matches!(
            host.pending.last().unwrap().kind,
            CommandKind::Queue { cyclic: false, .. }
        ));
    }

    // QUIRK D-89: deviation
    #[test]
    fn carried_objects_take_the_display_of_their_carrier() {
        use super::testkit::*;
        let mut host = host();
        let horse = spawn(&mut host, HORSE, 704.0, 704.0);
        let rider = spawn(&mut host, RIDER, 2000.0, 2000.0);
        let of = |host: &SimHost, id| host.display(host.state.object(id, false).unwrap());
        // Without a target (also with the bit set) the own position stays.
        let alone = of(&host, rider);
        host.state
            .object_mut(rider, false)
            .unwrap()
            .set_status_flags(status::ISMOUNTED, true);
        assert_eq!(of(&host, rider), alone);
        assert_ne!(alone, of(&host, horse));
        // With a target: position and rotation of the horse, also while it walks and between ticks.
        link(&mut host, rider, horse);
        assert!(host.move_units(
            &[horse],
            Vec3::new(1600.0, 0.0, 704.0),
            None,
            FormationOrder::default()
        ));
        while host.state.object(horse, false).unwrap().status & status::MOVE == 0 {
            host.advance(50.0);
        }
        let mut seen = Vec::new();
        for _ in 0..6 {
            host.advance(25.0);
            assert_eq!(of(&host, rider), of(&host, horse));
            seen.push(of(&host, horse).0);
        }
        // The horse really moves (otherwise the comparison would prove nothing).
        assert!(seen.windows(2).any(|w| w[0] != w[1]), "{seen:?}");
        // Occupant of a boat (`0x8000`) likewise.
        let o = host.state.object_mut(rider, false).unwrap();
        o.set_status_flags(status::ISMOUNTED, false);
        o.set_status_flags(status::ISMOUNTEDONBOAT, true);
        assert_eq!(of(&host, rider), of(&host, horse));
        // A target that no longer exists counts as no target.
        host.state.object_mut(rider, false).unwrap().target = ObjectId(0x7FFF_FFF0);
        assert_ne!(of(&host, rider), of(&host, horse));
    }

    // QUIRK K-238: kept
    #[test]
    fn a_diver_sinks_by_its_height_offset() {
        use super::testkit::*;
        let mut host = host();
        let diver = spawn(&mut host, DIVER, 1000.0, 1000.0);
        // Stand `ONWATER` comes from the type; the simulation height then does not contain the offset.
        let flags = |host: &SimHost| {
            host.state
                .object(diver, false)
                .unwrap()
                .module(2)
                .unwrap()
                .base
                .flags
        };
        assert_eq!(flags(&host) & 0x100, 0x100);
        let o = host.state.object_mut(diver, false).unwrap();
        o.set_status_flags(status::DIVING, true);
        o.world_pos[1] = 40.0;
        let y = |host: &SimHost| host.display(host.state.object(diver, false).unwrap()).0.y;
        let surface = y(&host);
        assert_eq!(surface, 40.0);
        // -1024 fixed-point cells * 128 / 4096 = 32 world units lower.
        host.state.object_mut(diver, false).unwrap().height_offset = -1024;
        assert_eq!(y(&host), surface - 32.0);
        // Also without stand `ONWATER` (`GameObject_FrameUpdate` `0x00470D72` adds unconditionally):
        // the offset applies unchanged.
        let off = flags(&host) & !0x100;
        host.state
            .object_mut(diver, false)
            .unwrap()
            .module_mut(2)
            .unwrap()
            .base
            .flags = off;
        assert_eq!(y(&host), surface - 32.0);
    }

    // QUIRK K-238: kept
    #[test]
    fn a_surfacing_diver_without_onwater_is_still_drawn_below_the_surface() {
        use super::testkit::*;
        let mut host = host();
        let diver = spawn(&mut host, DIVER, 1000.0, 1000.0);
        // Surfacing in the shallows: `ONWATER` is already off, `DIVING` too,
        // `height_offset` still runs -616 -> -4 -> 0. Without `MOVE`, simulation height at the water surface.
        let o = host.state.object_mut(diver, false).unwrap();
        o.module_mut(2).unwrap().base.flags &= !0x100;
        o.height_offset = -616;
        o.world_pos[1] = 0.0;
        assert_eq!(o.status & (status::DIVING | status::MOVE), 0);
        let o = host.state.object(diver, false).unwrap();
        // -616 * 128 / 4096 = -19.25 world units.
        assert_eq!(host.display(o).0.y, -19.25);
    }

    #[test]
    fn a_walker_that_stops_stays_at_the_last_height_shown_while_moving() {
        use super::testkit::*;
        let mut host = host();
        host.rules.map.water_level = 5.0;
        let swimmer = spawn(&mut host, DIVER, 1000.0, 1000.0);
        let y = |host: &SimHost| host.display(host.state.object(swimmer, false).unwrap()).0.y;
        // In the last step `SWIMMING` switches on: the step set the simulation height to the
        // bottom, `MOVE` is still set.
        let o = host.state.object_mut(swimmer, false).unwrap();
        o.status |= status::MOVE | status::SWIMMING;
        o.world_pos[1] = -73.0;
        assert_eq!(y(&host), 5.0, "with MOVE: water surface");
        host.track_shown_heights();
        host.state
            .object_mut(swimmer, false)
            .unwrap()
            .set_status_flags(status::MOVE, false);
        assert_eq!(y(&host), 5.0, "after the halt the last height stays");
        host.track_shown_heights();
        assert_eq!(y(&host), 5.0);
        // The halt tick shifts the grid position (last node): the height stays valid.
        let o = host.state.object_mut(swimmer, false).unwrap();
        o.set_status_flags(status::MOVE, true);
        host.track_shown_heights();
        let o = host.state.object_mut(swimmer, false).unwrap();
        o.grid.x = Fx(1 << 12);
        o.set_status_flags(status::MOVE, false);
        host.track_shown_heights();
        assert_eq!(y(&host), 5.0);
        // A new grid position without `MOVE` (snap, relocating) supersedes the remembered height.
        host.state.object_mut(swimmer, false).unwrap().grid.x = Fx(2 << 12);
        host.track_shown_heights();
        assert_eq!(y(&host), -73.0);
        // Walking updates it: on land without `SWIMMING` it is the terrain.
        let o = host.state.object_mut(swimmer, false).unwrap();
        o.status |= status::MOVE;
        o.set_status_flags(status::SWIMMING, false);
        host.track_shown_heights();
        let o = host.state.object_mut(swimmer, false).unwrap();
        o.set_status_flags(status::MOVE, false);
        assert_eq!(y(&host), 0.0);
    }

    #[test]
    fn divers_are_drawn_translucent() {
        use super::testkit::*;
        let mut host = host();
        let diver = spawn(&mut host, DIVER, 1000.0, 1000.0);
        let alpha =
            |host: &SimHost| SimHost::base_look(host.state.object(diver, false).unwrap()).alpha;
        assert_eq!(alpha(&host), 1.0);
        host.state
            .object_mut(diver, false)
            .unwrap()
            .set_status_flags(status::DIVING, true);
        assert_eq!(alpha(&host), 0.6);
        // The fog path (`entry_look`) adopts the value.
        assert_eq!(host.entry_look(diver, 0).unwrap().alpha, 0.6);
    }

    #[test]
    fn riding_info_leaves_the_checksum_unchanged() {
        use super::testkit::*;
        use opennml_sim::module::{diving, riding};
        let mut host = host();
        let rider = spawn(&mut host, RIDER, 640.0, 640.0);
        let horse = spawn(&mut host, HORSE, 704.0, 640.0);
        let diver = spawn(&mut host, DIVER, 1280.0, 640.0);
        // Before mounting: no mount (`null`, not the marker `ObjectId::NONE`).
        let free = host.query(Some(0), None, Some(&[rider.0]), 1);
        assert_eq!(free.objects[0].riding.len(), 1);
        assert_eq!(free.objects[0].riding[0].mount_target, None);
        assert!(free.objects[0].riding[0].lance_available);
        let free_json = serde_json::to_value(&free.objects[0]).unwrap();
        assert!(free_json["riding"][0]["mount_target"].is_null());
        link(&mut host, rider, horse);
        {
            let rules = host.rules.clone();
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            let r = riding::riding_mut(&mut sim, rider, riding::RIDING).unwrap();
            r.mount_target = horse;
            r.going_to_horse = true;
            r.busy = true;
            r.next_mount_anim = 1;
            r.lance_available = false;
            let d = diving::diving_mut(&mut sim, diver).unwrap();
            d.status = 1;
            d.count = -512;
            d.dive_layer = 8;
            d.orig_layer = 1;
            let o = sim.object_mut(diver).unwrap();
            o.height_offset = -512;
            // Full air: attribute 28 (stamina) to the maximum value 500.
            o.attributes.set_current_fraction(28, 100_000);
        }
        // Upgrade percentage of the diver type on attribute 28 (+20 %, synthetic GUID): the air
        // counts it like the other values of `sim_query`.
        let (slot, guid) = {
            let o = host.state.object(diver, false).unwrap();
            (usize::from(o.owner()), o.guid)
        };
        host.state.type_upgrades[slot]
            .entry(guid)
            .or_default()
            .set_percent(9999, 28, 20);
        let before = host.checksum();
        let result = host.query(Some(0), None, Some(&[rider.0, horse.0, diver.0]), 10);
        // Queries do not change the state.
        assert_eq!(host.checksum(), before);
        let info = |id: ObjectId| result.objects.iter().find(|o| o.id == id.0).unwrap();
        // Rider: section of module 13 with the values of the state.
        assert_eq!(
            info(rider).riding,
            vec![opennml_control::SimRidingInfo {
                index: 13,
                boat: false,
                mount_target: Some(horse.0),
                going_to_horse: true,
                busy: true,
                next_mount_anim: 1,
                lance_available: false,
            }]
        );
        assert_eq!(info(rider).diving, None);
        // Diver: section of module 24; air = current value of attribute 28.
        let air = host
            .state
            .object(diver, false)
            .unwrap()
            .attributes
            .current_value(28, 20);
        assert_eq!(air, 600);
        assert_eq!(
            info(diver).diving,
            Some(opennml_control::SimDivingInfo {
                status: 1,
                count: -512,
                orig_layer: 1,
                dive_layer: 8,
                air,
                height_offset: -512,
            })
        );
        assert!(info(diver).riding.is_empty());
        // The horse has neither.
        assert!(info(horse).riding.is_empty() && info(horse).diving.is_none());
        // Without the sections in the JSON (responses without riding): no entry.
        let json = serde_json::to_value(info(horse)).unwrap();
        assert!(json.get("riding").is_none() && json.get("diving").is_none());
        let json = serde_json::to_value(info(rider)).unwrap();
        assert_eq!(json["riding"][0]["mount_target"], horse.0);
        assert_eq!(host.checksum(), before);
    }

    #[test]
    fn upgrade_info_leaves_the_checksum_unchanged() {
        use crate::game::unit_bar::testkit::{
            COLONIST, GOVERNOR, MISSIONARY, host, set_armed, spawn,
        };
        use opennml_control::{SimCounterUpgrade, SimSpellInfo, SimTempUpgrade};
        use opennml_sim::module::spell;
        use opennml_sim::upgrades::{CounterUpgrade, TempUpgradeEntry};
        let mut host = host();
        let colonist = spawn(&mut host, COLONIST, 0, (10, 10));
        let governor = spawn(&mut host, GOVERNOR, 0, (12, 10));
        let missionary = spawn(&mut host, MISSIONARY, 0, (14, 10));
        set_armed(&mut host, colonist, true);
        host.state.applied_upgrades[0] = vec![2806, 3354];
        host.state.counter_upgrades.push(CounterUpgrade {
            guid: 3375,
            rest_ms: 9_900,
            source: missionary,
        });
        host.state.daytime = 2;
        {
            let o = host.state.object_mut(governor, false).unwrap();
            o.temp_upgrades.push(TempUpgradeEntry {
                flags: 0x80,
                guid: 4450,
                rest_ms: 1_500,
            });
            o.show_counter_upgrade = 1;
            o.set_status_flags(status::ANIM_LOCKED, true);
            let ModuleKind::Spell(s) = &mut o.module_mut(spell::INDEX).unwrap().kind else {
                panic!("spell module expected");
            };
            s.art = 4;
            s.button = 544;
            s.cost = 50;
            s.anim = 0;
            s.idx = 2;
            s.reserve = 3;
        }
        let before = host.checksum();
        let result = host.query(Some(0), None, None, 10);
        // Queries do not change the state.
        assert_eq!(host.checksum(), before);
        // Player: list in order of application; counter-upgrades and time of day as in the state.
        assert_eq!(result.players[0].upgrades, vec![2806, 3354]);
        // The other slots show their list unchanged (the start applies time-of-day upgrades).
        assert_eq!(result.players[1].upgrades, host.state.applied_upgrades[1]);
        assert_eq!(
            result.counter_upgrades,
            vec![SimCounterUpgrade {
                guid: 3375,
                rest_ms: 9_900,
                source: missionary.0,
            }]
        );
        assert_eq!(result.daytime, 2);
        let info = |id: ObjectId| result.objects.iter().find(|o| o.id == id.0).unwrap();
        // Governor: temporary list, counter, spell with mana (attribute 15) and execution list.
        let g = info(governor);
        assert_eq!(
            g.temp_upgrades,
            vec![SimTempUpgrade {
                guid: 4450,
                flags: 0x80,
                rest_ms: 1_500,
            }]
        );
        assert_eq!(g.show_counter_upgrade, 1);
        assert_eq!(g.exec_list, vec![4452, 4450, 4451]);
        let (mana, mana_max) = {
            let o = host.state.object(governor, false).unwrap();
            let p =
                opennml_sim::upgrades::type_upgrade_percent(&host.state, 0, GOVERNOR, spell::MANA);
            (
                o.attributes.current_value(spell::MANA, p),
                o.attributes.max_value(spell::MANA, p),
            )
        };
        assert!(mana_max > 0, "extra energy from default.dat");
        assert_eq!(
            g.spell,
            Some(SimSpellInfo {
                art: 4,
                button: 544,
                cost: 50,
                anim: 0,
                idx: 2,
                reserve: 3,
                mana,
                mana_max,
                casting: true,
                target: None,
                corpse: None,
            })
        );
        assert_eq!(g.armed, None, "module 39 missing");
        // Colonist: armed, nothing else; missionary without anything.
        let c = info(colonist);
        assert_eq!(c.armed, Some(true));
        assert!(c.spell.is_none() && c.exec_list.is_empty() && c.temp_upgrades.is_empty());
        set_armed(&mut host, colonist, false);
        let again = host.query(Some(0), None, Some(&[colonist.0]), 1);
        assert_eq!(again.objects[0].armed, Some(false));
        set_armed(&mut host, colonist, true);
        let m = info(missionary);
        assert!(m.armed.is_none() && m.spell.is_none() && m.exec_list.is_empty());
        assert_eq!(m.show_counter_upgrade, 0);
        // Without the sections in the JSON.
        let json = serde_json::to_value(m).unwrap();
        for key in [
            "temp_upgrades",
            "show_counter_upgrade",
            "armed",
            "spell",
            "exec_list",
        ] {
            assert!(json.get(key).is_none(), "{key}");
        }
        assert_eq!(host.checksum(), before);
    }

    /// House 1001 (20 s, 900 HP), fisher's basket 3505 (resource without flags, food 50 from `[CONFIG]`).
    const ECONOMY: &str = "GUID 1001\nNAME Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 3505\nNAME Fischerkorb\nVALUEID 100 6 1 0\nPROPID 6\nVALUEID 70002 6 1 0\n";

    fn economy_host() -> SimHost {
        let mut rules = Rules::synthetic(ECONOMY, 16);
        rules
            .defaults
            .insert(1001, [(13, 20), (14, 900)].into_iter().collect());
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        // `[CONFIG]` of the map: food of the fisher's baskets.
        host.state.object_config.insert(24, 50);
        host
    }

    #[test]
    fn construction_sites_are_shown_cut_at_their_progress_and_grow_with_build_steps() {
        use opennml_render::scene::{LayerModel, entry_layers};
        let mut host = economy_host();
        let rules = host.rules.clone();
        let site = {
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            create_object(&mut sim, &mut ObjectDesc::new(1001, 0, [576.0, 0.0, 576.0])).unwrap()
        };
        let look = |host: &SimHost| SimHost::base_look(host.state.object(site, false).unwrap());
        // Construction site: only render bit 4, progress 0.1 - without scaffold the model up to 10 % of the
        // height, with scaffold only the scaffold up to 20 %.
        let start = look(&host);
        assert_eq!((start.bits, start.progress.to_bits()), (4, 0x3DCC_CCCD));
        let layers = entry_layers(start, false, false);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].model, LayerModel::Main);
        assert_eq!(layers[0].cut, Some((start.progress, false)));
        assert_eq!(
            entry_layers(start, true, false)[0].model,
            LayerModel::Scaffold
        );
        // Build steps (10 of 2000 units per step): the cut moves along.
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        for _ in 0..100 {
            let mut m = opennml_sim::message::Message::new(0x203, &[100, 0]);
            opennml_sim::messaging::send_mut(&mut sim, site, &mut m);
        }
        let half = look(&host);
        assert_eq!(half.progress, 0.5);
        assert_eq!(entry_layers(half, false, false)[0].cut, Some((0.5, false)));
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        for _ in 0..100 {
            let mut m = opennml_sim::message::Message::new(0x203, &[100, 0]);
            opennml_sim::messaging::send_mut(&mut sim, site, &mut m);
        }
        // Finished: render bits 3, the whole model with shadow.
        let done = look(&host);
        assert_eq!((done.bits, done.progress), (3, 1.0));
        assert_eq!(entry_layers(done, true, false).len(), 1);
        assert_eq!(entry_layers(done, true, false)[0].cut, None);
    }

    #[test]
    fn fish_baskets_stay_invisible_until_the_first_access_and_occupants_are_hidden() {
        use opennml_render::scene::entry_layers;
        let mut host = economy_host();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let basket = create_object(
            &mut sim,
            &mut ObjectDesc::new(3505, 10, [320.0, 0.0, 320.0]),
        )
        .unwrap();
        let shown = |sim: &opennml_sim::sim::Sim| {
            !entry_layers(
                SimHost::base_look(sim.object(basket).unwrap()),
                false,
                false,
            )
            .is_empty()
        };
        assert!(!shown(&sim));
        // Invisible, but hittable with module 6 (`MapView::sync_sim` sets `resource`): the
        // right-click hits the basket (fishing 398, `FUN_0046CEA0`).
        assert!(sim.object(basket).unwrap().has_module(6, false));
        let flags = crate::game::selection::pick_flags(sim.object(basket).unwrap());
        assert!(flags.is_some_and(|f| f.triangles));
        assert_eq!(
            crate::game::selection::click_target(sim.state, &[(basket, ())], |_, _, _| true),
            Some(basket)
        );
        assert_eq!(opennml_sim::module::resource::take(&mut sim, basket, 5), 5);
        assert!(shown(&sim));
        assert!(
            sim.state
                .events
                .contains(&SimEvent::Revealed { object: basket })
        );
        // The rendering does not draw quartered objects (render bits 0).
        let mut o = sim.object(basket).unwrap().clone();
        o.status |= status::QUARTERED;
        assert_eq!(SimHost::base_look(&o).bits, 0);
    }

    #[test]
    fn entry_looks_follow_the_fog_and_brightness_of_the_active_player() {
        let mut host = economy_host();
        let rules = host.rules.clone();
        let (own, foreign) = {
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            let mut at = |player, x| {
                create_object(&mut sim, &mut ObjectDesc::new(1001, player, [x, 0.0, x])).unwrap()
            };
            (at(0, 576.0), at(1, 320.0))
        };
        assert_eq!((own.slot(), foreign.slot()), (0, 1));
        // No fog tick yet: nothing seen, both hidden; the own construction site fully bright,
        // the foreign one on an unexplored tile dark.
        let look = |host: &mut SimHost, id| host.entry_look(id, 0).unwrap();
        assert_eq!(
            [look(&mut host, own), look(&mut host, foreign)].map(|l| (l.bits, l.brightness)),
            [(0, 1.0), (0, 0.0)]
        );
        // Revealed: the render bits of the simulation, the brightness of the alpha table.
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        host.fog.update_targets(&host.state, 0, true);
        let base = SimHost::base_look(host.state.object(foreign, false).unwrap());
        assert_eq!(base.bits, 4);
        assert_eq!(look(&mut host, foreign), base);
        // Occupants stay hidden; unknown IDs have no rendering.
        let o = host.state.object_mut(foreign, false).unwrap();
        o.status |= status::QUARTERED;
        assert_eq!(look(&mut host, foreign).bits, 0);
        assert_eq!(host.entry_look(ObjectId::NONE, 0), None);
    }

    /// Player color of the rendering: `g_PlayerColors[GameObject+0x2D0]`
    /// like `GameObject_SetPlayerColor` (`0x004680B0`), i.e. the lobby color of the slot, not the
    /// slot; it follows a change of owner (`GameObject_OnOwnerChanged` `0x0046F850`), a carcass
    /// keeps the color of the old owner (C-142, dying slot 5 `0x0050E460`).
    // QUIRK C-142: kept
    #[test]
    fn entry_looks_take_the_player_colour_and_follow_an_owner_change() {
        use super::testkit::*;
        use opennml_world::markers::marker_color;
        let rgb = |index: i32| {
            let [r, g, b, _] = marker_color(index);
            Some([r, g, b])
        };
        // Type with a dying module (PROPID 16) for the carcass.
        const CORPSE: u32 = 3999;
        let corpse_type = "GUID 3999\nNAME ENG_Kadaver\nVALUEID 100 6 1 0\nPROPID 16\n";
        let mut host = host_with(&(types() + corpse_type));
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        // Lobby color 5 on slot 0: the color belongs to the lobby, not to the slot.
        host.state.players[0].color = 5;
        let horse = spawn_for(&mut host, HORSE, 1, 1000.0, 1000.0);
        let look = |host: &mut SimHost, id| host.entry_look(id, 0).unwrap().color;
        assert_eq!(
            SimHost::base_look(host.state.object(horse, false).unwrap()).color,
            rgb(1)
        );
        assert_eq!(look(&mut host, horse), rgb(1));
        // Takeover like K-224: new ID on slot 0, color of the lobby of slot 0.
        let rules = host.rules.clone();
        let mounted = opennml_sim::world::change_owner(
            &mut opennml_sim::sim::Sim::new(&mut host.state, &rules),
            horse,
            0,
        )
        .unwrap();
        assert_eq!(look(&mut host, mounted), rgb(5));
        // Carcass on slot 8 (C-142): old color instead of light gray.
        let corpse = spawn_for(&mut host, CORPSE, 1, 1500.0, 1500.0);
        host.state
            .object_mut(corpse, false)
            .unwrap()
            .set_status_flags(status::DIED, true);
        let dead = opennml_sim::world::change_owner(
            &mut opennml_sim::sim::Sim::new(&mut host.state, &rules),
            corpse,
            8,
        )
        .unwrap();
        assert_eq!(dead.slot(), 8);
        assert_eq!(look(&mut host, dead), rgb(1));
        assert_ne!(rgb(1), rgb(8));
    }

    /// Finished house at (576, 0, 576).
    fn house(host: &mut SimHost) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let mut desc = ObjectDesc::new(1001, 0, [576.0, 0.0, 576.0]);
        desc.flags |= opennml_sim::world::desc_flags::PLACED;
        create_object(&mut sim, &mut desc).unwrap()
    }

    fn send_building(host: &mut SimHost, id: ObjectId, code: u32) {
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let mut m = opennml_sim::message::Message::new(code, &[]);
        opennml_sim::messaging::send_mut(&mut sim, id, &mut m);
    }

    fn shown_y(host: &SimHost, id: ObjectId) -> f32 {
        host.display(host.state.object(id, false).unwrap()).0.y
    }

    #[test]
    fn destroyed_buildings_sink_in_two_phases_after_their_timers_fire() {
        // The phases begin when their timer fires (clock > delay, in 100 ms ticks):
        // at 200 and 1400 ms.
        assert_eq!(sink_offset(0.0), 0.0);
        assert_eq!(sink_offset(200.0), 0.0);
        // Phase 1: -800 * 0.3^2 = -72 after 300 ms.
        assert!((sink_offset(500.0) + 72.0).abs() < 1e-3);
        assert!((sink_offset(1400.0) + 72.0).abs() < 1e-3);
        // Phase 2: a further -200 * 1^2 after one second, -872 in total.
        assert!((sink_offset(2400.0) + 272.0).abs() < 1e-3);
        assert!((sink_offset(3400.0) + 872.0).abs() < 1e-3);
        assert_eq!(sink_offset(f64::INFINITY), sink_offset(3400.0));
        // The host reads the clock from the timers of the destruction.
        let mut host = economy_host();
        let house = house(&mut host);
        let rules = host.rules.clone();
        opennml_sim::module::building::destroy::destroy(
            &mut opennml_sim::sim::Sim::new(&mut host.state, &rules),
            house,
        );
        assert_eq!(shown_y(&host, house), 0.0);
        host.step_once();
        assert_eq!(shown_y(&host, house), 0.0);
        // 3 s later (60 steps, 30 building ticks): clock 2900 or 3000 ms.
        for _ in 0..60 {
            host.step_once();
        }
        let clock = host.sink_clock(host.state.object(house, false).unwrap());
        assert!(
            matches!(clock, Some(c) if (2900.0..=3000.0).contains(&c)),
            "{clock:?}"
        );
        assert_eq!(shown_y(&host, house), sink_offset(clock.unwrap()));
        assert!(shown_y(&host, house) < -500.0);
        let o = host.state.object(house, false).unwrap();
        assert_eq!(o.world_pos[1], 0.0, "the simulation itself does not sink");
        // Without the timers of the destruction (after the collapse) the sinking is over.
        let mut o = host.state.object(house, false).unwrap().clone();
        if let Some(m) = o.modules.get_mut(3).and_then(Option::as_mut) {
            m.base.events.clear();
        }
        assert_eq!(host.sink_clock(&o), Some(f64::INFINITY));
        assert!((host.display(&o).0.y + 872.0).abs() < 1e-3);
    }

    #[test]
    fn a_sleeping_palisade_does_not_sink_and_wakes_at_ground_level() {
        use opennml_sim::module::building::msg;
        let mut host = economy_host();
        let palisade = house(&mut host);
        // Sleeping (state dead, but not destroyed): no sinking.
        send_building(&mut host, palisade, msg::SET_SLEEPING);
        let o = host.state.object(palisade, false).unwrap();
        assert_ne!(o.status & status::DIED, 0);
        for _ in 0..60 {
            host.step_once();
        }
        assert_eq!(shown_y(&host, palisade), 0.0);
        send_building(&mut host, palisade, msg::WAKE);
        host.step_once();
        assert_eq!(shown_y(&host, palisade), 0.0);
    }

    #[test]
    fn a_loaded_save_keeps_sinking_where_it_was() {
        let mut host = economy_host();
        let house = house(&mut host);
        let rules = host.rules.clone();
        opennml_sim::module::building::destroy::destroy(
            &mut opennml_sim::sim::Sim::new(&mut host.state, &rules),
            house,
        );
        for _ in 0..41 {
            host.step_once();
        }
        let before = shown_y(&host, house);
        assert!(before < -72.0, "{before}");
        let save = host.save();
        let mut loaded = economy_host();
        loaded.load(save, None);
        assert_eq!(shown_y(&loaded, house), before);
    }

    #[test]
    fn the_account_line_names_the_active_players_resources_and_clamped_living_space() {
        let mut host = economy_host();
        let p = &mut host.state.players[0];
        p.resources_fx = [1000 << 12, 250 << 12, 75 << 12, 7 << 12, 400 << 12];
        assert_eq!(
            host.account_line(),
            "Balance player 0: Gold 1000 | Wood 250 | Food 75 | Population 7/125"
        );
        host.active_player = 3;
        assert!(host.account_line().starts_with("Balance player 3: Gold"));
    }

    // QUIRK K-325: deviation
    #[test]
    fn game_time_runs_1_2_times_faster_with_at_most_two_steps_per_frame() {
        let mut host = host();
        // GameSpeed 14: 16 ms -> 19.2 ms game time; 50 ms after three frames (57.6 ms).
        host.advance(16.0);
        host.advance(16.0);
        assert_eq!(host.state.steps, 0);
        host.advance(16.0);
        assert_eq!(host.state.steps, 1);
        assert!((host.alpha() - 7.6 / 50.0).abs() < 1e-5);
        // 500 ms in one frame: two ticks, the rest is discarded.
        host.advance(500.0);
        assert_eq!(host.state.steps, 3);
        assert_eq!(host.alpha(), 0.0);
        // GameSpeed 10 = real time.
        host.game_speed = 10;
        host.advance(50.0);
        assert_eq!(host.state.steps, 4);
        assert_eq!(host.hashes.back().unwrap().0, 4);
        // O-153: speed 255 (factor 13.25) without limit 20: a frame of 1000 ms yields exactly `MAX_STEPS_PER_FRAME`
        // ticks, the rest is discarded (no loop, no overflow).
        host.game_speed = 255;
        let before = host.state.steps;
        host.advance(1000.0);
        assert_eq!(host.state.steps, before + u64::from(MAX_STEPS_PER_FRAME));
        assert_eq!(host.carry_ms, 0.0);
    }

    #[test]
    fn loading_a_save_puts_the_active_player_on_an_occupied_slot() {
        let mut host = host();
        let save = host.save();
        host.active_player = 5;
        host.load(save, None);
        assert_eq!(Some(host.active_player), host.first_player().or(Some(0)));
    }

    #[test]
    fn status_line_names_tick_mode_player_and_checksum() {
        let mut host = host();
        host.step_once();
        host.step_once();
        let line = host.status_line();
        assert!(
            line.starts_with(
                "Simulation: tick 1 (2 steps) | game type -1 | GameSpeed 14 | player 0 | 2 objects"
            ),
            "{line}"
        );
        assert!(
            line.ends_with(&format!("{:016x}", host.checksum())),
            "{line}"
        );
    }

    #[test]
    fn the_map_lists_ambient_objects_first_then_simulation_objects_with_their_ids() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let lvl = "data/levels/multiplayer/[6players]-legends.lvl";
        let scene = opennml_world::level::load_level(&vfs, lvl).unwrap();
        let mut host = start_sim(
            &vfs,
            &scene,
            DEFAULT_GAME_SPEED,
            super::testkit::default_setup,
        )
        .unwrap()
        .unwrap();
        let objects = map_objects(&scene, &host.state, |m| vfs.exists(m));
        let ambient = objects
            .iter()
            .take_while(|o| o.class == ObjectClass::Ambient)
            .count();
        assert_eq!(
            ambient,
            scene
                .objects
                .iter()
                .filter(|o| o.class == ObjectClass::Ambient)
                .count()
        );
        let game = &objects[ambient..];
        assert!(game.iter().all(|o| o.class == ObjectClass::Game));
        assert!(game.iter().all(|o| o.sim_id.is_some()));
        assert!(objects.iter().enumerate().all(|(i, o)| o.index == i));
        // Every player has their start objects (HQ and units) on the map.
        for slot in 0..6 {
            let owned = game.iter().filter(|o| o.player == Some(slot)).count();
            assert!(owned >= 5, "slot {slot}: {owned} objects");
        }
        // Sim objects without a model are missing, otherwise all are there in table order.
        let ids: Vec<u32> = game.iter().filter_map(|o| o.sim_id).collect();
        let all: Vec<u32> = host.state.objects().map(|o| o.id.0).collect();
        assert!(ids.iter().all(|id| all.contains(id)));
        assert!(
            ids.len() * 10 >= all.len() * 9,
            "{} of {}",
            ids.len(),
            all.len()
        );
        // After placement the render bits of the simulation show every model except the
        // fisher's baskets (invisible until the first fishing) - on a revealed map also after
        // the fog of the active player.
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        let placed: Vec<(ObjectId, u32)> = host.state.objects().map(|o| (o.id, o.guid)).collect();
        let hidden: Vec<u32> = placed
            .into_iter()
            .filter(|&(id, _)| host.entry_look(id, 0).unwrap().bits & 1 == 0)
            .map(|(_, guid)| guid)
            .collect();
        assert!(hidden.iter().all(|&g| g == 3505), "{hidden:?}");
    }

    fn friendship(p: i32, q: i32) -> opennml_sim::command::Command {
        opennml_sim::command::Command {
            player: 0,
            kind: opennml_sim::command::CommandKind::Diplomacy { p, q, mode: 0 },
        }
    }

    // QUIRK O-318: kept
    #[test]
    fn immediate_commands_replay_like_the_next_step() {
        // The original executes giving up (`0x27` -> `0x50000056`) immediately in single player, even if no tick
        // is running. The command enters the log with the current step **before** the waiting ones; the
        // replay executes it at the beginning of this step and reaches the same checksum.
        let mut rules = Rules::synthetic(TYPES, 16);
        rules.level.players = (0..2)
            .map(|slot| opennml_formats::config::LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation: 0,
                kind: 0,
                resources: Vec::new(),
            })
            .collect();
        let setup = MatchSetup {
            mode: -1,
            living_space: 2,
            ..MatchSetup::default()
        };
        let mut live = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        for i in 0..6 {
            if i == 2 {
                live.submit(friendship(0, 1));
            }
            live.step_once();
        }
        live.submit(friendship(2, 3));
        let surrender = opennml_sim::command::Command {
            player: 0,
            kind: CommandKind::SetPlayerResult { slot: 0, result: 1 },
        };
        live.execute_now(surrender.clone());
        assert_eq!(
            live.state.players[0].result, 1,
            "immediately, without a tick"
        );
        assert_eq!(live.state.steps, 6);
        assert_eq!(live.checksum(), checksum(&live.state), "checksum updated");
        assert_eq!(live.hashes.iter().filter(|h| h.0 == 6).count(), 1);
        assert_eq!(live.pending_commands().len(), 1, "the waiting one stays");
        live.step_once();
        let steps: Vec<(u64, bool)> = live
            .log
            .iter()
            .map(|c| (c.step, c.command == surrender))
            .collect();
        assert_eq!(steps, [(2, false), (6, true), (6, false)]);
        let replay = Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        let mut other = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        other.load_replay(replay, None).unwrap();
        assert_eq!(other.state.steps, 7);
        assert_eq!(other.checksum(), live.checksum());
        assert_eq!(other.state.players[0].result, 1);
    }

    #[test]
    fn commands_wait_for_the_next_step_and_are_logged_for_replays() {
        let mut host = host();
        host.submit(friendship(0, 1));
        assert!(!host.state.diplomacy.is_friend(0, 1));
        host.step_once();
        assert!(host.state.diplomacy.is_friend(0, 1));
        assert_eq!(host.log[0].step, 0);
        assert_eq!(host.replay().unwrap().commands.len(), 1);
        assert_eq!(host.section(&[]).commands, 1);
    }

    /// Without a log, loading suspends the recording (as before); the state continues bit-identically.
    #[test]
    fn saves_continue_bit_identically_and_end_the_replay_log_without_a_log() {
        let mut host = host();
        host.step_once();
        let save = SaveGame::from_bytes(&host.save().to_bytes()).unwrap();
        host.step_once();
        let after = host.checksum();
        host.load(save, None);
        assert_eq!(host.hashes.len(), 1);
        host.step_once();
        assert_eq!(host.checksum(), after);
        let error = host.replay().unwrap_err().to_string();
        assert!(error.contains("savegame"), "{error}");
        // A restart begins a new recording.
        let setup = host.setup.clone();
        host.restart(setup).unwrap();
        assert_eq!(host.state.steps, 0);
        assert!(host.replay().is_ok());
    }

    /// E-329 after loading: with the log of the savegame the replay stays complete and, after further ticks,
    /// equals the replay of the undivided match.
    // QUIRK E-329: deviation
    #[test]
    fn loaded_save_with_log_keeps_the_replay_complete() {
        let mut whole = host();
        whole.step_once();
        whole.submit(friendship(0, 1));
        for _ in 0..4 {
            whole.step_once();
        }
        let mut first = host();
        first.step_once();
        first.submit(friendship(0, 1));
        first.step_once();
        let save = SaveGame::from_bytes(&first.save().to_bytes()).unwrap();
        let log = first.save_log();
        assert_eq!(log.as_ref().map(Vec::len), Some(1));
        let mut loaded = host();
        loaded.load(save, log);
        assert!(loaded.log_complete());
        for _ in 0..3 {
            loaded.step_once();
        }
        assert_eq!(loaded.checksum(), whole.checksum());
        assert_eq!(loaded.replay().unwrap(), whole.replay().unwrap());
    }

    #[test]
    fn save_after_the_limit_has_no_log() {
        let mut host = host();
        host.state.steps = MAX_REPLAY_STEPS - 1;
        host.step_with(Vec::new());
        assert!(host.save_log().is_some());
        host.step_with(Vec::new());
        assert!(!host.log_complete());
        assert_eq!(host.save_log(), None);
        let mut fresh = self::host();
        fresh.load(host.save(), None);
        assert_eq!(fresh.save_log(), None);
    }

    /// A sync after loading adopts state and log of the host but does not make the replay complete (the commands
    /// before the state are still missing); without loading it stays complete.
    #[test]
    fn adopt_after_load_keeps_the_replay_incomplete() {
        let mut live = host();
        live.step_once();
        let save = SaveGame::from_bytes(&live.save().to_bytes()).unwrap();
        live.step_once();
        let mut loaded = host();
        loaded.load(save, None);
        loaded.step_once();
        loaded.adopt(live.net_state()).unwrap();
        assert_eq!(loaded.checksum(), live.checksum());
        let error = loaded.replay().unwrap_err().to_string();
        assert!(error.contains("savegame"), "{error}");
        let mut fresh = host();
        fresh.step_once();
        fresh.step_once();
        fresh.adopt(live.net_state()).unwrap();
        assert!(fresh.replay().is_ok(), "complete without loading");
    }

    #[test]
    fn replays_reach_the_same_checksum_as_the_live_match() {
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut live = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        for i in 0..12 {
            if i == 3 {
                live.submit(friendship(0, 1));
            }
            if i == 8 {
                live.submit(friendship(2, 3));
            }
            live.step_once();
        }
        let replay = Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        let mut other = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        other.load_replay(replay.clone(), Some(12)).unwrap();
        assert_eq!(other.checksum(), live.checksum());
        assert_eq!(other.log.len(), 2);
        // Without a specification up to one tick after the last command (tick 8).
        other.load_replay(replay, None).unwrap();
        assert_eq!(other.state.steps, 9);
        assert_eq!(other.log.len(), 2);
    }

    #[test]
    fn replays_with_endless_command_steps_fail_instead_of_panicking_or_hanging() {
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        let before = host.checksum();
        for step in [u64::MAX, MAX_REPLAY_STEPS, 1_000_000_000] {
            let replay = Replay {
                start: opennml_sim::setup::start_package(&rules, &setup),
                commands: vec![RecordedCommand {
                    step,
                    command: friendship(0, 1),
                }],
            };
            let err = host.load_replay(replay, None).unwrap_err().to_string();
            // `u64::MAX`: the end one tick after it overflows (`Replay::implied_end`), otherwise the length limit.
            let expected = if step == u64::MAX { "u64" } else { "at most" };
            assert!(err.contains(expected), "{step}: {err}");
        }
        assert_eq!(host.checksum(), before);
    }

    /// NaN positions that loading no longer rejects (arrow, spell target together with the world effect at this
    /// position, `progress_scale`, animal alpha) run through host, interpolation, flights and control channel
    /// queries without a panic.
    #[test]
    fn nan_positions_reach_the_host_without_panic() {
        use crate::game::projectiles::Flight;
        use opennml_sim::combat::projectile::{Projectile, ProjectileKind};
        use opennml_sim::module::{Module, animal, spell};
        let mut live = host();
        live.step_once();
        let mut save = live.save();
        let nan = [f32::NAN; 3];
        let ids: Vec<ObjectId> = save.state.objects().map(|o| o.id).collect();
        let (shooter, target) = (ids[0], ids[1]);
        let projectile = |id, kind| Projectile {
            id,
            kind,
            flags: 0x21,
            remaining_ms: 300,
            flight_ms: 400,
            shooter,
            target,
            damage: opennml_sim::message::Message::new(
                0x17F,
                &[-12, 0, 1100, 0, 0, 0, 0, 0, 0, 0, 1, 0],
            ),
            aim: opennml_sim::fx::FxVec3::default(),
            target_pos: nan,
            start: nan,
            zone: 0,
        };
        save.state.projectiles.list.extend([
            projectile(100, ProjectileKind::Arrow),
            projectile(101, ProjectileKind::CannonBall),
            projectile(102, ProjectileKind::Explosion),
            projectile(103, ProjectileKind::Storm),
        ]);
        for o in save
            .state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
        {
            o.progress_scale = f32::NAN;
            let mut a = Module::new(animal::INDEX as u8);
            if let ModuleKind::Animal(a) = &mut a.kind {
                a.alpha = f32::NAN;
            }
            o.modules[animal::INDEX] = Some(a);
            let mut s = Module::new(spell::INDEX as u8);
            if let ModuleKind::Spell(s) = &mut s.kind {
                s.target = nan;
            }
            o.modules[spell::INDEX] = Some(s);
        }
        let save = SaveGame::from_bytes(&save.to_bytes()).unwrap();
        save.check(&live.rules, opennml_sim::save::RulesCompare::Full)
            .unwrap();
        let mut host = host();
        host.load(save, None);
        let mut flights: Vec<Flight> = host
            .state
            .projectiles
            .list
            .iter()
            .filter_map(|p| {
                let (start, aim) = (Vec3::from(p.start), Vec3::from(p.target_pos));
                let (t, rest) = (p.flight_ms, p.remaining_ms);
                match p.kind {
                    ProjectileKind::Arrow => {
                        Some(Flight::resume_arrow(p.id, start, aim, t, rest, None, false))
                    }
                    ProjectileKind::CannonBall => {
                        Some(Flight::resume_ball(p.id, start, aim, t, rest, 0.0))
                    }
                    _ => None,
                }
            })
            .collect();
        assert_eq!(flights.len(), 2);
        for frame in 0..20u32 {
            host.advance(40.0);
            for id in &ids {
                if let Some(o) = host.state.object(*id, false) {
                    let _ = host.interpolated(o);
                }
                let _ = host.entry_look(*id, frame * 40);
            }
            for f in &mut flights {
                f.track(Vec3::NAN);
                f.advance(40.0);
            }
        }
        let _ = host.query(None, None, None, 10);
        let _ = host.section(&ids);
        // Camera and script camera not finite: move, rotation, clamping, projection, ray.
        use crate::game::script_camera::{CameraRotation, MoveLeg, ScriptCamera};
        let (mut camera, terrain) = crate::game::input::testkit::setup();
        crate::game_save::CameraSave {
            from: nan,
            at: nan,
            fov: f32::NAN,
        }
        .apply(&mut camera);
        let mut script = ScriptCamera {
            backup: Some((nan, nan)),
            moves: [MoveLeg {
                from: nan,
                at: nan,
                ms: 100,
                player: false,
            }]
            .into(),
            rest_ms: f32::NAN,
            eye_speed: nan,
            target_speed: nan,
            rotation: Some(CameraRotation {
                remaining_ms: f32::NAN,
                rad_per_ms: f32::NAN,
            }),
            ..ScriptCamera::default()
        };
        for _ in 0..5 {
            script.update(40.0, &mut camera, &terrain, &host);
            let projection =
                camera.projection(opennml_world::camera::AspectMode::Correct, 800, 600);
            let (origin, direction) = camera.ray(projection, glam::Vec2::ZERO);
            let _ = terrain.cursor_point(origin, direction, 100_000.0);
            camera.scroll(1.0, 1.0, &terrain);
            let _ = camera.view();
        }
    }

    #[test]
    fn a_module_beyond_the_named_ones_is_listed_as_unknown_instead_of_panicking() {
        let mut host = host();
        let object = host
            .state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
            .next()
            .unwrap();
        let module = object.modules.iter().flatten().next().unwrap().clone();
        object.modules[opennml_sim::module::MODULE_NAMES.len()] = Some(module);
        let result = host.query(None, None, None, 10);
        assert!(
            result
                .objects
                .iter()
                .flat_map(|o| &o.modules)
                .any(|m| m.index == 56 && m.name == "?")
        );
    }

    /// The counters of the script commands ("Script" window, `script_state`) count accepted/partial/rejected
    /// per action, keep the last 20 rejections (oldest first) and belong to the host (restart and loading empty
    /// them).
    #[test]
    fn script_command_stats_count_per_action_and_keep_the_last_twenty_rejections() {
        use opennml_sim::command::Outcome;
        use opennml_sim::script::ScriptTrace;
        let command = |action: i32, outcome: Outcome| {
            SimEvent::Script(ScriptTrace::Command {
                container: 1,
                block: 2,
                action,
                code: 0x3C,
                outcome,
            })
        };
        let mut stats = ScriptCommandStats::default();
        assert_eq!(stats.info(), opennml_control::ScriptCommandsInfo::default());
        stats.record(
            5,
            &[
                command(191, Outcome::Accepted),
                command(191, Outcome::Rejected("gone".into())),
                command(153, Outcome::Partial("in part".into())),
                SimEvent::Script(ScriptTrace::DialogEnded),
                SimEvent::Script(ScriptTrace::Fired {
                    container: 1,
                    block: 2,
                }),
            ],
        );
        let info = stats.info();
        let counts: Vec<_> = info
            .actions
            .iter()
            .map(|a| (a.action, a.accepted, a.partial, a.rejected))
            .collect();
        // Ascending by action number; partial is not a rejection.
        assert_eq!(counts, [(153, 0, 1, 0), (191, 1, 0, 1)]);
        assert_eq!(info.rejections.len(), 1);
        assert_eq!(
            (
                info.rejections[0].step,
                info.rejections[0].container,
                info.rejections[0].block,
                info.rejections[0].action,
                info.rejections[0].code,
                info.rejections[0].reason.as_str()
            ),
            (5, 1, 2, 191, 0x3C, "gone")
        );
        // 25 further rejections: only the last 20 remain (steps 11 to 30), the counters count all.
        for step in 6..=30u64 {
            stats.record(
                step,
                &[command(191, Outcome::Rejected(format!("reason {step}")))],
            );
        }
        let info = stats.info();
        assert_eq!(info.actions[1].rejected, 26);
        let steps: Vec<u64> = info.rejections.iter().map(|r| r.step).collect();
        assert_eq!(steps, (11..=30).collect::<Vec<u64>>());
        assert_eq!(info.rejections[19].reason, "reason 30");
    }

    // QUIRK E-332: deviation
    #[test]
    fn log_stops_at_the_replay_limit() {
        let mut host = host();
        host.state.steps = MAX_REPLAY_STEPS - 1;
        host.execute_now(friendship(0, 1));
        assert_eq!(host.log.len(), 1);
        assert!(host.log_complete());
        host.step_with(Vec::new());
        assert_eq!(host.state.steps, MAX_REPLAY_STEPS);
        host.execute_now(friendship(0, 1));
        assert_eq!(
            host.log.len(),
            1,
            "from the limit on nothing more is booked"
        );
        assert!(!host.log_complete());
        let err = host.replay().unwrap_err().to_string();
        assert!(err.contains("incomplete"), "{err}");
        host.step_with(vec![friendship(0, 1)]);
        assert_eq!(host.log.len(), 1);
    }

    #[test]
    fn log_ends_at_the_replay_limit_even_without_commands() {
        let mut host = host();
        host.state.steps = MAX_REPLAY_STEPS - 1;
        host.step_with(Vec::new());
        assert!(host.log_complete(), "tick 575,999 was the last one allowed");
        host.step_with(Vec::new());
        assert!(!host.log_complete());
        assert!(host.replay().is_err());
    }

    #[test]
    fn the_replay_limit_of_the_control_protocol_is_the_one_of_the_simulation() {
        assert_eq!(opennml_control::MAX_REPLAY_STEPS, MAX_REPLAY_STEPS);
    }

    #[test]
    fn queries_filter_by_owner_type_and_id_and_list_all_players() {
        let host = host();
        let all = host.query(None, None, None, 100);
        assert_eq!(all.total, 2);
        assert_eq!(all.players.len(), 8);
        assert_eq!(all.players[0].resources[0], 30000);
        let colonist = &all.objects[0];
        assert_eq!(colonist.type_name, "Kolonist");
        assert!(
            colonist
                .modules
                .iter()
                .any(|m| m.name == "STAND" && m.kind == "stand")
        );
        assert_eq!(host.query(None, Some(1000), None, 100).total, 1);
        assert_eq!(host.query(Some(1), None, None, 100).total, 0);
        let one = host.query(None, None, Some(&[colonist.id]), 100);
        assert_eq!(one.total, 1);
        let limited = host.query(None, None, None, 1);
        assert_eq!((limited.total, limited.objects.len()), (2, 1));
        let hash = host.hash_result();
        assert_eq!(hash.checksum, hex_checksum(host.checksum()));
        assert_eq!(hash.history.len(), 1);
    }

    #[test]
    fn the_fog_targets_are_renewed_before_the_step_that_restarts_the_cycle() {
        let mut host = host();
        for _ in 0..100 {
            if host.state.fog.processed == -2 {
                break;
            }
            host.step_once();
        }
        assert_eq!(host.state.fog.processed, -2);
        // Foreground of the ended cycle: everything seen. The new cycle finishes stamping in the same
        // tick and overwrites that with its copy; the targets apply to the old one.
        host.state.fog.seen.fill(0xFF);
        let mut expected = host.fog.clone();
        expected.update_targets(&host.state, host.active_player, false);
        host.step_once();
        assert_eq!(host.state.fog.processed, -1, "cycle ended in the same tick");
        assert_ne!(host.state.fog.seen.count(0xFF), 16 * 16);
        assert_eq!(host.fog, expected);
    }

    #[test]
    fn the_idle_ticks_are_counted_per_step_that_began_in_state_minus_one() {
        let mut host = host();
        let mut expected = 0u32;
        for _ in 0..200 {
            // Idle tick: state -1 before the step (-2 after); evaluation equivalent to -2 afterwards.
            let idle = host.state.fog.processed == -1;
            host.step_once();
            expected += u32::from(idle);
            assert_eq!(host.fog_idle_ticks(), expected);
            assert_eq!(idle, host.state.fog.processed == -2);
        }
        assert!(expected > 0, "the cycle of the scene has idle ticks");
    }

    // QUIRK O-287: kept
    #[test]
    fn no_rows_while_paused() {
        // Without a step (pause, open game menu, single frame without enough time) no idle tick counts.
        let mut host = host();
        for _ in 0..20 {
            host.step_once();
        }
        let (ticks, state) = (host.fog_idle_ticks(), host.state.fog.processed);
        for _ in 0..10 {
            host.advance(0.0);
            host.advance(1.0);
        }
        assert_eq!(host.fog_idle_ticks(), ticks);
        assert_eq!(host.state.fog.processed, state);
        let mut map = crate::game::minimap::Minimap::new(crate::game::minimap::MinimapTerrain {
            dim_x: 16,
            dim_z: 16,
            colors: vec![0x0022_5971; 16 * 16],
            image: image::RgbaImage::new(16, 16),
        });
        map.sync_host(&host);
        let row = map.fog().next_row();
        for _ in 0..5 {
            map.sync_host(&host);
        }
        assert_eq!(map.fog().next_row(), row, "no new idle tick, no new row");
    }

    #[test]
    fn minimap_rebuilds_are_counted_at_level_start_switch_and_cinemascope_end() {
        let mut host = host();
        let start = host.fog_rebuilds();
        assert!(start >= 1, "level start (reset_fog)");
        host.state.players[3].control = Control::Ai;
        host.set_active_player(3).unwrap();
        assert_eq!(host.fog_rebuilds(), start + 1, "player switch (activate)");
        host.request_minimap_rebuild();
        assert_eq!(host.fog_rebuilds(), start + 2);
    }

    #[test]
    fn fog_view_and_memories_leave_the_checksum_unchanged() {
        let mut host = host();
        for _ in 0..6 {
            host.step_once();
        }
        let before = opennml_sim::save::checksum(&host.state);
        let ids: Vec<_> = host.state.objects().map(|o| o.id).collect();
        for id in ids {
            host.entry_look(id, 1234);
        }
        host.switch_player(1);
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        host.update_memories();
        let _ = host.memories.active().count();
        assert_eq!(opennml_sim::save::checksum(&host.state), before);
    }

    #[test]
    fn objects_with_an_active_memory_show_its_shadow_bit_with_the_fog_brightness() {
        let mut host = host();
        let rules = host.rules.clone();
        let at = 10.0 * 128.0 + 64.0;
        let hq = {
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            create_object(&mut sim, &mut ObjectDesc::new(1000, 1, [at, 0.0, at])).unwrap()
        };
        host.step_once();
        assert!(
            host.memories.get(hq).is_some(),
            "Created creates the memory"
        );
        // Tile (10, 10) only explored for player 0: the memory is active.
        host.state.fog.seen.clear();
        host.state.fog.cleared.clear();
        host.state.fog.explored.clear();
        host.state.fog.explored.or(10, 10, 1);
        host.update_memories();
        host.fog.update_targets(&host.state, 0, true);
        assert!(host.memories.get(hq).unwrap().active);
        let o = host.state.object_mut(hq, false).unwrap();
        o.render_bits |= opennml_render::scene::RENDER_SHADOW;
        let look = host.entry_look(hq, 0).unwrap();
        assert_eq!(look.bits, 0x1000);
        assert_eq!(look.brightness, 128.0 / 255.0);
    }

    /// Host for the transition tests (D-290: an object disappears briefly before its fog memory appears): rules
    /// `rules` with sight range 8 for colonists (1100, stamp 5 x 5); colonist `near` of player 0 on tile (5, 5) next to
    /// the object `target` (`guid`, player 1, tile (7, 5)), a second colonist on tile (20, 20). The second keeps
    /// stamping when `near` no longer sees anything: with B = 1 (below 38 objects) the cycle then lasts several
    /// ticks, the copy of the foreground lies two ticks before state -2 (S >= B).
    fn sight_host(mut rules: Rules, guid: u32) -> (SimHost, ObjectId, ObjectId) {
        rules.defaults.entry(1100).or_default().insert(18, 8);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        let rules = host.rules.clone();
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let near =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [at(5), 0.0, at(5)])).unwrap();
        create_object(
            &mut sim,
            &mut ObjectDesc::new(1100, 0, [at(20), 0.0, at(20)]),
        )
        .unwrap();
        let target =
            create_object(&mut sim, &mut ObjectDesc::new(guid, 1, [at(7), 0.0, at(5)])).unwrap();
        (host, near, target)
    }

    /// (drawn for real, drawn as an active fog memory) for `id` in the current state.
    fn drawn_as(host: &mut SimHost, id: ObjectId) -> (bool, bool) {
        let real = host.entry_look(id, 0).unwrap().bits & !0x1000 != 0;
        let memory = host
            .memories
            .active()
            .any(|m| m.snapshot.as_ref().is_some_and(|s| s.id == id));
        (real, memory)
    }

    /// Steps until the switch between object and memory; in **every** step `id` is drawn exactly once (for real or
    /// as a memory, never none and never both). Returns the cycle state after the switching step.
    fn step_until_switch(host: &mut SimHost, id: ObjectId, real_before: bool) -> i32 {
        for step in 0..200 {
            host.step_once();
            let (real, memory) = drawn_as(host, id);
            assert!(
                real != memory,
                "step {step}: real {real}, memory {memory} (state {})",
                host.state.fog.processed
            );
            if real != real_before {
                return host.state.fog.processed;
            }
        }
        panic!("no switch between object and fog memory");
    }

    /// The original checks the fog memories every frame after the ticks (`0x00480120`/`0x00480127`,
    /// re/vision 9): if an object leaves the sight, its memory appears in the same step in which it disappears
    /// (the step with the copy of the foreground, state afterwards -1), not two ticks later at state -2.
    /// Conversely the memory becomes inactive in the same step in which the object appears again.
    fn assert_no_gap_at_the_fog_edge(rules: Rules, guid: u32) {
        let (mut host, near, target) = sight_host(rules, guid);
        for _ in 0..20 {
            host.step_once();
        }
        assert_eq!(drawn_as(&mut host, target), (true, false), "in sight");
        // `near` sees nothing anymore (sight range 0: does not stamp, does not count toward the budget).
        let blind = |host: &mut SimHost, on: bool| {
            let a = &mut host.state.object_mut(near, false).unwrap().attributes;
            if on {
                a.set_modifier(18, -8, false, 1);
            } else {
                a.remove_modifier(18, 1);
            }
        };
        blind(&mut host, true);
        assert_eq!(
            step_until_switch(&mut host, target, true),
            -1,
            "memory in the step of the copy"
        );
        // A few cycles in the fog: the memory stays, the object is invisible.
        for step in 0..12 {
            host.step_once();
            assert_eq!(
                drawn_as(&mut host, target),
                (false, true),
                "step {step} in the fog"
            );
        }
        blind(&mut host, false);
        assert_eq!(
            step_until_switch(&mut host, target, false),
            -1,
            "object in the step of the copy"
        );
        assert_eq!(drawn_as(&mut host, target), (true, false));
    }

    // QUIRK D-290: kept
    #[test]
    fn an_object_leaving_sight_is_always_drawn_either_as_object_or_as_memory() {
        assert_no_gap_at_the_fog_edge(Rules::synthetic(TYPES, 32), 1000);
    }

    /// The original checks tents (movable fog memories, list `+0x188`) every frame without camera restriction
    /// (`0x0047E5B8`); the reimplementation after every step.
    // QUIRK D-229, D-290: kept
    #[test]
    fn a_tent_leaving_sight_is_always_drawn_either_as_object_or_as_memory() {
        use crate::game::testworld::{self, DWELLING_TENT};
        let (mut host, _, tent) = sight_host(testworld::rules(), DWELLING_TENT);
        host.step_once();
        assert!(
            host.memories.get(tent).is_some_and(|m| m.movable),
            "movable memory"
        );
        assert_no_gap_at_the_fog_edge(testworld::rules(), DWELLING_TENT);
    }

    /// An immediately executed command (pause, `execute_now`) checks the memories right away, like the original in
    /// the next frame without a tick.
    // QUIRK D-290: kept
    #[test]
    fn memories_are_checked_after_an_immediate_command() {
        let (mut host, _, target) = sight_host(Rules::synthetic(TYPES, 32), 1000);
        for _ in 0..20 {
            host.step_once();
        }
        assert_eq!(drawn_as(&mut host, target), (true, false));
        // Foreground cleared without a tick (like a copy), then any immediate command.
        host.state.fog.seen.clear();
        host.state.fog.cleared.clear();
        host.execute_now(Command {
            player: 0,
            kind: CommandKind::ScriptButton { word: 0 },
        });
        assert_eq!(drawn_as(&mut host, target), (false, true));
    }

    #[test]
    fn players_switch_between_occupied_slots() {
        let mut host = host();
        // Campaign-like setup without .lvl players: nobody occupied.
        host.switch_player(1);
        assert_eq!(host.active_player, 0);
        host.state.players[3].control = Control::Ai;
        host.state.players[5].control = Control::Human;
        host.switch_player(1);
        assert_eq!(host.active_player, 3);
        host.switch_player(1);
        assert_eq!(host.active_player, 5);
        host.switch_player(-1);
        assert_eq!(host.active_player, 3);
    }

    /// `sim_view` (protocol 10): active player only on occupied slots, "Fog off" and
    /// overlay; the checksum stays.
    #[test]
    fn the_view_sets_player_and_fog_off_without_touching_the_simulation() {
        let mut host = host();
        assert_eq!(
            host.set_active_player(3),
            Err("slot 3 is not occupied".into())
        );
        assert_eq!(host.active_player, 0);
        assert!(host.set_active_player(8).is_err());
        host.state.players[3].control = Control::Ai;
        host.state.fog.seen.or(2, 2, 1 << 3);
        host.state.diplomacy.set_vision(3, 1, true);
        let before = opennml_sim::save::checksum(&host.state);
        host.set_active_player(3).unwrap();
        assert_eq!(host.active_player, 3);
        assert_eq!(host.fog.level(2, 2, 0), 0xFF, "target values immediately");
        assert!(host.section(&[]).fog_shown);
        host.set_fog_shown(false);
        assert_eq!(host.fog.show_mode, crate::game::fog_view::FOG_OFF);
        host.fog.overlay = Some(7);
        let section = host.section(&[]);
        assert_eq!((section.fog_shown, section.fog_overlay), (false, Some(7)));
        host.set_fog_shown(true);
        assert_eq!(host.fog.show_mode, 0);
        let players = host.query(None, None, None, 1).players;
        assert_eq!((players[3].enemies, players[3].vision), (0xF7, 0b1010));
        assert_eq!(opennml_sim::save::checksum(&host.state), before);
    }

    #[test]
    fn buildings_keep_the_idle_animation_until_their_module_animates_them() {
        let host = host();
        let objects: Vec<&GameObject> = host.state.objects().collect();
        assert_eq!(display_anim(&host.rules, objects[0]), DisplayAnim::Sim(0));
        assert_eq!(display_anim(&host.rules, objects[1]), DisplayAnim::Idle);
    }

    /// Host with an own unit (type 1100, walk) at `(704, 704)` on a flat map.
    fn walker_host() -> (SimHost, ObjectId) {
        let mut rules = Rules::synthetic(WALKER, 32);
        rules
            .defaults
            .insert(1100, [(16, 183)].into_iter().collect());
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, 10).unwrap();
        host.active_player = 0;
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let id =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [704.0, 0.0, 704.0])).unwrap();
        (host, id)
    }

    /// Grid position with terrain height (water clamp excluded): what FrameUpdate shows at progress 0, an
    /// empty trajectory or status `0x10`.
    fn grid_position(host: &SimHost, o: &GameObject) -> [f32; 3] {
        let mut w = opennml_sim::world::grid_to_world(o.grid, host.rules.map.cell);
        w[1] = opennml_sim::height::terrain_height(&host.rules, w[0], w[1], w[2]);
        w
    }

    #[test]
    fn walkers_without_progress_or_trajectory_are_shown_exactly_on_the_grid() {
        let (mut host, own) = walker_host();
        assert!(host.move_units(
            &[own],
            Vec3::new(1600.0, 0.0, 704.0),
            None,
            FormationOrder::default()
        ));
        while host.state.object(own, false).unwrap().status & status::MOVE == 0 {
            host.advance(50.0);
        }
        host.advance(50.0);
        // The unit was moved on between two ticks: grid != world position before.
        let walking = host.state.object(own, false).unwrap().clone();
        assert!(
            !walking
                .module(0)
                .map(|m| match &m.kind {
                    ModuleKind::Walk(w) => w.trajectory.is_empty(),
                    _ => true,
                })
                .unwrap()
        );
        // Progress 0 (tick of its own group just run): exactly the grid position.
        host.state.phase = if walking.tick_group == 0 { 1 } else { 0 };
        host.carry_ms = 0.0;
        let (shown, _) = host.walk_display(&walking).unwrap();
        assert_eq!(shown.to_array(), grid_position(&host, &walking));
        // Arrival tick: MOVE still set, path empty, progress > 0 -> likewise the grid position,
        // no fallback to the interpolation between the previous and the current tick.
        let mut arrived = walking.clone();
        let ModuleKind::Walk(w) = &mut arrived.module_mut(0).unwrap().kind else {
            panic!("walk expected");
        };
        w.trajectory.clear();
        host.carry_ms = 30.0;
        arrived.world_pos[0] -= 18.0;
        let (shown, yaw) = host.walk_display(&arrived).unwrap();
        assert_eq!(shown.to_array(), grid_position(&host, &arrived));
        // The yaw stays that of the simulation.
        assert_eq!(yaw, arrived.rotation[1]);
        // Status 0x10 (stand idle) also shows the grid, also with a path.
        let mut idle = walking.clone();
        idle.status |= 0x10;
        let (shown, _) = host.walk_display(&idle).unwrap();
        assert_eq!(shown.to_array(), grid_position(&host, &idle));
        // Without MOVE: world position of the simulation without interpolation.
        let mut still = walking;
        still.status &= !status::MOVE;
        let (shown, _) = host.walk_display(&still).unwrap();
        assert_eq!(shown.to_array(), still.world_pos);
    }

    #[test]
    fn walkers_are_never_shown_moving_backwards_along_their_path_even_on_arrival() {
        let (mut host, own) = walker_host();
        assert!(host.move_units(
            &[own],
            Vec3::new(1600.0, 0.0, 704.0),
            None,
            FormationOrder::default()
        ));
        let mut last = f32::MIN;
        let mut started = false;
        let mut finished = 0;
        // Frames of 16 ms: shares between ticks, arrival, transition to standing.
        for frame in 0..4000 {
            host.advance(16.0);
            let o = host.state.object(own, false).unwrap();
            let x = host.display(o).0.x;
            started |= o.status & status::MOVE != 0;
            if started {
                assert!(x >= last, "frame {frame}: {x} < {last}");
                last = x;
                finished += i32::from(o.status & status::MOVE == 0);
            }
            if finished > 40 {
                break;
            }
        }
        assert!(finished > 40, "destination not reached");
        // Arrived: exactly the world position of the simulation.
        let o = host.state.object(own, false).unwrap();
        assert_eq!(host.display(o).0.to_array(), o.world_pos);
        assert!(last > 1500.0, "{last}");
    }

    #[test]
    fn swimming_walkers_are_clamped_to_the_water_level_but_not_on_walls_or_boats() {
        let (mut host, own) = walker_host();
        host.rules.map.water_level = 5.0;
        let mut o = host.state.object(own, false).unwrap().clone();
        o.status |= status::MOVE;
        // Grid height 2 (64 * 128 / 4096) above the flat terrain at 0.
        o.grid.y = Fx(64);
        o.world_pos[1] = 2.0;
        let y = |host: &SimHost, o: &GameObject| host.walk_display(o).unwrap().0.y;
        // Terrain at 0: without swimming it stays there.
        assert_eq!(y(&host, &o), 0.0);
        // SWIMMING: at least water height.
        let mut swimming = o.clone();
        swimming.status |= status::SWIMMING;
        assert_eq!(y(&host, &swimming), 5.0);
        // Walk flag 0x2000: clamped without 0x800, with 0x800 only together with 0x20000.
        let set = |o: &GameObject, flags: u32| {
            let mut o = o.clone();
            o.module_mut(0).unwrap().base.flags = flags;
            o
        };
        assert_eq!(y(&host, &set(&o, 0x2000)), 5.0);
        assert_eq!(y(&host, &set(&o, 0x2000 | 0x800)), 0.0);
        assert_eq!(y(&host, &set(&o, 0x2000 | 0x800 | 0x20000)), 5.0);
        assert_eq!(y(&host, &set(&o, 0x800)), 0.0);
        // Wall, diving and boat: height unchanged, neither terrain nor water.
        for bit in [status::ONWALL, status::DIVING, status::ISMOUNTEDONBOAT] {
            let mut o = set(&o, 0x2000);
            o.status |= bit | status::SWIMMING;
            assert_eq!(y(&host, &o), 2.0, "{bit:#x}");
        }
        // The grid rendering (empty trajectory) also clamps to the water.
        let mut arrived = set(&o, 0x2000);
        let ModuleKind::Walk(w) = &mut arrived.module_mut(0).unwrap().kind else {
            panic!("walk expected");
        };
        w.trajectory.clear();
        assert_eq!(y(&host, &arrived), 5.0);
    }

    // QUIRK C-161: kept
    #[test]
    fn attack_points_stay_until_the_next_event_and_follow_the_active_player() {
        let mut host = host();
        assert_eq!(host.active_attack_points(), 0);
        let mut points = [0; 8];
        points[0] = 390;
        points[3] = 120;
        host.record_attack_points(&[SimEvent::AttackPoints { points }]);
        assert_eq!(host.active_attack_points(), 390);
        // Section `sim` (protocol 11): all slots and the value of the active player.
        let section = host.section(&[]);
        assert_eq!(section.attack_points, points);
        assert_eq!(section.attack_points_active, 390);
        // Without an event (e.g. in the pause) the value stays.
        host.record_attack_points(&[]);
        assert_eq!(host.active_attack_points(), 390);
        host.active_player = 3;
        assert_eq!(
            host.active_attack_points(),
            120,
            "the value of the new player right away"
        );
        assert_eq!(host.section(&[]).attack_points_active, 120);
        host.record_attack_points(&[SimEvent::AttackPoints { points: [0; 8] }]);
        assert_eq!(host.active_attack_points(), 0);
    }

    #[test]
    fn corpses_sink_by_hide_speed_per_tick_of_the_dying_module() {
        const DYING: &str = "GUID 1100
NAME Kolonist
VALUEID 100 6 1 0
PROPID 2
VALUEID 30001 1 1 0
PROPID 16
VALUEID 170002 6 1 4
";
        let rules = Rules::synthetic(DYING, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [64.0, 0.0, 64.0])).unwrap();
        let set = |host: &mut SimHost, status: u8, counter: i32, speed: i32| {
            let o = host.state.object_mut(id, false).unwrap();
            let Some(ModuleKind::Dying(d)) = o
                .module_mut(opennml_sim::module::dying::INDEX)
                .map(|m| &mut m.kind)
            else {
                panic!("no dying module");
            };
            d.status = status;
            d.die_counter = counter;
            d.hide_speed = speed;
        };
        let depth = |host: &SimHost| {
            let o = host.state.object(id, false).unwrap();
            (host.corpse_sink(o), host.display(o).0.y)
        };
        set(&mut host, 0, 0, 0);
        assert_eq!(depth(&host), (0.0, 0.0), "alive: nothing");
        // Decay: counter 3000, speed 1, -101 per tick.
        set(&mut host, 2, 3000, 1);
        assert_eq!(depth(&host).0, 0.0);
        set(&mut host, 2, 3000 - 101 * 7 - 5, 1);
        assert_eq!(depth(&host), (7.0, -7.0));
        // Swimmer: counter 7500, speed 3.
        set(&mut host, 2, 7500 - 101 * 10, 3);
        assert_eq!(depth(&host), (30.0, -30.0));
        // Status 1 (callbacks pending) does not sink yet.
        set(&mut host, 1, 7500, 3);
        assert_eq!(depth(&host).0, 0.0);
        // Corrupted but accepted savegame: edge values do not crash the viewer.
        set(&mut host, 2, i32::MIN, 3);
        assert!(depth(&host).0.is_finite());
        set(&mut host, 2, -1_000_000, i32::MAX);
        assert_eq!(depth(&host).0, i32::MAX as f32);
    }

    // --- Train, cart, ships: rendering ---

    /// Train state of the object (copy).
    fn train_of(host: &SimHost, id: ObjectId) -> opennml_sim::module::train::Train {
        match &host
            .state
            .object(id, false)
            .and_then(|o| o.module(opennml_sim::module::train::INDEX))
            .expect("train module")
            .kind
        {
            ModuleKind::Train(t) => (**t).clone(),
            _ => panic!("no train"),
        }
    }

    fn train_mut(host: &mut SimHost, id: ObjectId) -> &mut opennml_sim::module::train::Train {
        match &mut host
            .state
            .object_mut(id, false)
            .and_then(|o| o.module_mut(opennml_sim::module::train::INDEX))
            .expect("train module")
            .kind
        {
            ModuleKind::Train(t) => t,
            _ => panic!("no train"),
        }
    }

    /// Track (10, 20) ... (49, 20), locomotive on (12, 20): returns locomotive, tender, wagon.
    fn train_on_track(host: &mut SimHost) -> (ObjectId, ObjectId, ObjectId) {
        use super::testkit::{LOCO, lay_track, spawn_dir};
        lay_track(host, (10, 20), (49, 20));
        let loco = spawn_dir(host, LOCO, 0, (12, 20), 0);
        let slaves = train_of(host, loco).slaves;
        assert_eq!(slaves.len(), 2, "tender and wagon");
        (loco, slaves[0], slaves[1])
    }

    /// Object group `group` ticked `ms` ms ago (share `ms` of the tick group).
    fn set_group_progress(host: &mut SimHost, group: i8, ms: f64) {
        host.state.phase = if group == 0 { 1 } else { 0 };
        host.carry_ms = ms;
    }

    #[test]
    fn a_moving_train_is_drawn_ahead_by_its_next_speed() {
        use super::testkit::*;
        let mut host = train_host();
        let (loco, tender, wagon) = train_on_track(&mut host);
        let shown = |host: &SimHost, id| {
            let o = host.state.object(id, false).unwrap();
            (host.display(o).0, o.grid)
        };
        let cell = host.rules.map.cell;
        // Standing: the own position of the simulation.
        for id in [loco, tender, wagon] {
            let o = host.state.object(id, false).unwrap();
            assert_eq!(host.display(o).0, Vec3::from(o.world_pos), "{id:?} stands");
        }
        // Driving: departure, then specify the speed of the next tick (value of the drive curve).
        train_command(&mut host, loco, opennml_sim::command::TrainOp::Forward);
        object_tick(&mut host);
        assert_ne!(
            train_of(&host, loco).status & opennml_sim::module::train::status::MOVING,
            0
        );
        train_mut(&mut host, loco).next_speed = 2304;
        let group = host.state.object(loco, false).unwrap().tick_group;
        set_group_progress(&mut host, group, 50.0);
        let sum = checksum(&host.state);
        // Half offset (progress 50 of 100): 2304/2 grid = 0.28125 tiles = 36 units.
        let half = opennml_sim::fx::fixed_mul(4096, 2304) as f32 * 0.5 / 4096.0 * cell[0] as f32;
        assert_eq!(half, 36.0);
        for id in [loco, tender, wagon] {
            let (pos, grid) = shown(&host, id);
            let on_grid = opennml_sim::world::grid_to_world(grid, cell);
            assert!(
                (pos.x - (on_grid[0] + half)).abs() < 1e-3 && (pos.z - on_grid[2]).abs() < 1e-3,
                "{id:?}: {pos} against grid {on_grid:?}"
            );
        }
        // Full 100 ms: the whole offset; progress 0: exactly the grid.
        set_group_progress(&mut host, group, 100.0);
        let (pos, grid) = shown(&host, loco);
        let on_grid = opennml_sim::world::grid_to_world(grid, cell);
        assert!((pos.x - (on_grid[0] + 2.0 * half)).abs() < 1e-3);
        set_group_progress(&mut host, group, 0.0);
        let (pos, grid) = shown(&host, loco);
        let on_grid = opennml_sim::world::grid_to_world(grid, cell);
        assert!((pos.x - on_grid[0]).abs() < 1e-3);
        // Backwards: the offset reverses with the drive vector.
        train_mut(&mut host, loco).dir = (-4096, 0);
        set_group_progress(&mut host, group, 50.0);
        let (pos, grid) = shown(&host, loco);
        let on_grid = opennml_sim::world::grid_to_world(grid, cell);
        assert!((pos.x - (on_grid[0] - half)).abs() < 1e-3);
        train_mut(&mut host, loco).dir = (4096, 0);
        // Rendering only: the checksum stays.
        assert_eq!(checksum(&host.state), sum);
        // After the halt (bit 2 off) the own position again.
        train_mut(&mut host, loco).status = 0;
        for id in [loco, tender, wagon] {
            let o = host.state.object(id, false).unwrap();
            assert_eq!(host.display(o).0, Vec3::from(o.world_pos), "{id:?} stands");
        }
    }

    #[test]
    fn trains_outside_the_map_fade_out() {
        use super::testkit::*;
        let mut host = train_host();
        let (loco, tender, _) = train_on_track(&mut host);
        let dim = host.rules.map.dim_x;
        let alpha = |host: &mut SimHost, id| host.entry_look(id, 0).unwrap().alpha;
        assert_eq!(alpha(&mut host, loco), 1.0, "inside the map");
        // 2.5 tiles beyond the right edge: 1 - 0.2 * 2.5 = 0.5.
        host.state.object_mut(loco, false).unwrap().grid.x.0 = (dim << 12) + 10_240;
        assert!((alpha(&mut host, loco) - 0.5).abs() < 1e-6);
        // The overhang counts per object from its own grid position: the tender is still inside.
        assert_eq!(alpha(&mut host, tender), 1.0);
        // Left and in z likewise (`overshoot`: x < 0, x > W, z < 0, z > H).
        host.state.object_mut(loco, false).unwrap().grid.x.0 = -10_240;
        assert!((alpha(&mut host, loco) - 0.5).abs() < 1e-6);
        {
            let o = host.state.object_mut(loco, false).unwrap();
            o.grid.x.0 = 5 << 12;
            o.grid.z.0 = (dim << 12) + 4096;
        }
        assert!((alpha(&mut host, loco) - 0.8).abs() < 1e-6);
        // Invisible from 5 tiles, never negative.
        host.state.object_mut(loco, false).unwrap().grid.z.0 = (dim << 12) + 5 * 4096;
        assert_eq!(alpha(&mut host, loco), 0.0);
        host.state.object_mut(loco, false).unwrap().grid.z.0 = (dim << 12) + 16 * 4096;
        assert_eq!(alpha(&mut host, loco), 0.0);
        // Other objects outside the map stay opaque.
        let cart = spawn_dir(&mut host, CART, 0, (30, 30), 0);
        host.state.object_mut(cart, false).unwrap().grid.x.0 = (dim << 12) + 10_240;
        assert_eq!(alpha(&mut host, cart), 1.0);
    }

    #[test]
    fn trains_and_carts_play_their_simulated_animation() {
        use super::testkit::*;
        // Only buildings (module 3) show the idle animation.
        assert_eq!(ANIMATED_LATER.as_slice(), [3]);
        let mut host = train_host();
        let (loco, tender, _) = train_on_track(&mut host);
        let anim =
            |host: &SimHost, id| display_anim(&host.rules, host.state.object(id, false).unwrap());
        // Standing: waiting animation 1.
        assert_eq!(anim(&host, loco), DisplayAnim::Sim(1));
        // Driving: driving animation 0 on locomotive and slaves (the number of the locomotive).
        train_command(&mut host, loco, opennml_sim::command::TrainOp::Forward);
        object_tick(&mut host);
        object_tick(&mut host);
        assert_eq!(anim(&host, loco), DisplayAnim::Sim(0));
        assert_eq!(anim(&host, tender), DisplayAnim::Sim(0));
        // Without an animation set, a train object does not fall back to the idle animation.
        let mut unset = host.state.object(loco, false).unwrap().clone();
        unset.anim.current = -1;
        assert_eq!(display_anim(&host.rules, &unset), DisplayAnim::Sim(-1));
        // Buildings keep it.
        let mut track = unset.clone();
        track.guid = TRACK;
        assert_eq!(display_anim(&host.rules, &track), DisplayAnim::Idle);
        // Cart: front and rear part show their animation, the rear part rests with 520003 = 0.
        let cart = spawn_dir(&mut host, CART, 0, (30, 30), 0);
        for _ in 0..4 {
            object_tick(&mut host);
        }
        let tail = cart_tail(&host, cart);
        let mut cart_unset = host.state.object(cart, false).unwrap().clone();
        cart_unset.anim.current = -1;
        assert_eq!(display_anim(&host.rules, &cart_unset), DisplayAnim::Sim(-1));
        assert_eq!(anim(&host, tail), DisplayAnim::Sim(0));
    }

    // QUIRK D-370: kept
    #[test]
    fn wheel_rate_follows_the_original_formula() {
        // Speed <= 0 counts as 128 (normal speed).
        assert_eq!(super::wheel_rate(128, 0), 1.0);
        assert_eq!(super::wheel_rate(128, -5), 1.0);
        // Maximum speed 2304 (`Dist(2200, 2300, 5, 4)`) at SX 128: 128 * 2304 * 10 / 4096 * 1.5 = 1080 -> 8.4375.
        assert_eq!(super::wheel_rate(128, 2304), 8.4375);
        // First acceleration step after standing: `Dist(100, 200, 5, 4)` = 153.
        assert_eq!(super::wheel_rate(128, 153), 153.0 * 0.46875 / 128.0);
    }

    // QUIRK D-370: kept
    #[test]
    fn the_wheels_turn_with_the_speed_of_the_loco() {
        use super::testkit::*;
        use opennml_sim::module::train::status;
        let mut host = train_host();
        let (loco, tender, wagon) = train_on_track(&mut host);
        let rate =
            |host: &SimHost, id| host.train_wheel_rate(host.state.object(id, false).unwrap());
        // Standing: the speed of the simulation; other objects have none.
        for id in [loco, tender, wagon] {
            assert_eq!(rate(&host, id), None);
        }
        // Departure and acceleration: as long as the driving animation is wanted but has not arrived yet, the
        // previous one keeps running at its speed; after that every train object according to `v` of the locomotive.
        train_command(&mut host, loco, opennml_sim::command::TrainOp::Forward);
        let cell = host.rules.map.cell[0];
        let (mut kept, mut derived, mut last) = (0, 0, 0.0);
        for _ in 0..60 {
            host.step_once();
            let t = train_of(&host, loco);
            if t.status & status::MOVING == 0 {
                continue;
            }
            let sum = checksum(&host.state);
            let want = super::wheel_rate(cell, t.speed);
            for id in [loco, tender, wagon] {
                let o = host.state.object(id, false).unwrap();
                if o.anim.current == o.anim.wanted {
                    assert_eq!(rate(&host, id), Some(WheelRate::Derived(want)), "{id:?}");
                    derived += 1;
                } else {
                    assert_eq!(rate(&host, id), Some(WheelRate::Keep), "{id:?}");
                    kept += 1;
                }
            }
            assert_eq!(checksum(&host.state), sum, "rendering only");
            // In the first tick `v` = 0 (normal speed), after that the speed rises with `v`.
            if t.speed > 0 {
                assert!(want >= last, "the speed rises");
                last = want;
            }
            if t.status & status::ACCEL == 0 {
                break;
            }
        }
        assert!(kept > 0 && derived > 0, "{kept} kept, {derived} derived");
        let t = train_of(&host, loco);
        assert_eq!(t.status & (status::ACCEL | status::MOVING), status::MOVING);
        assert_eq!(t.speed, 2304);
        assert_eq!(
            rate(&host, wagon),
            Some(WheelRate::Derived(super::wheel_rate(cell, 2304)))
        );
        // Braking: until the handover of the brake animation the old speed, after that that of the simulation.
        train_mut(&mut host, loco).status |= status::BRAKING;
        host.state.object_mut(loco, false).unwrap().anim.wanted = 99;
        assert_eq!(rate(&host, loco), Some(WheelRate::Keep));
        host.state.object_mut(loco, false).unwrap().anim.current = 99;
        assert_eq!(rate(&host, loco), None);
    }

    /// Rear part of a front part (`linked`).
    fn cart_tail(host: &SimHost, head: ObjectId) -> ObjectId {
        match &host
            .state
            .object(head, false)
            .and_then(|o| o.module(opennml_sim::module::multipart::HEAD))
            .expect("front part")
            .kind
        {
            ModuleKind::MultiHead(h) => {
                assert!(!h.linked.is_none(), "rear part created");
                h.linked
            }
            _ => panic!("no front part"),
        }
    }

    // QUIRK K-247: deviation
    #[test]
    fn the_cart_tail_turns_with_the_interpolated_head() {
        use super::testkit::*;
        let mut host = train_host();
        let cart = spawn_dir(&mut host, CART, 0, (20, 20), 0);
        for _ in 0..4 {
            object_tick(&mut host);
        }
        let tail = cart_tail(&host, cart);
        let yaw_of =
            |a: i32| a.wrapping_add(90 << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
        // Front part standing: position = rendering position of the front part; yaw = angle from the drawbar point
        // to the head (limited to +-90 degrees from the head direction) + 90 degrees.
        for (dx, dz) in [
            (0, 0),
            (0, 12_288),
            (0, -12_288),
            (-12_288, 0),
            (3000, -9000),
        ] {
            let (hx, hz, hdir) = {
                let h = host.state.object(cart, false).unwrap();
                (h.grid.x.0, h.grid.z.0, h.direction.0)
            };
            let centre = (hx - 3 * 4096 + dx, hz + dz);
            if let ModuleKind::MultiPart(m) = &mut host
                .state
                .object_mut(tail, false)
                .unwrap()
                .module_mut(opennml_sim::module::multipart::PART)
                .unwrap()
                .kind
            {
                m.centre = (centre.0, centre.1, 0);
            }
            let sum = checksum(&host.state);
            let angle = opennml_sim::module::multipart::follow_angle(centre, (hx, hz, hdir));
            let (head_pos, _) = host.display(host.state.object(cart, false).unwrap());
            let (pos, rotation) = host.display(host.state.object(tail, false).unwrap());
            assert_eq!(
                pos, head_pos,
                "position of the rear part = rendering position of the front part"
            );
            assert!(
                (rotation.y - yaw_of(angle)).abs() < 1e-6,
                "yaw {} against {} (angle {})",
                rotation.y,
                yaw_of(angle),
                angle as f32 / 4096.0
            );
            // Rendering only: nothing is written back.
            assert_eq!(checksum(&host.state), sum);
        }
        // If the front part drives, the rear part follows the rendering position (ahead of the simulation).
        assert!(host.move_units(
            &[cart],
            Vec3::new(3600.0, 0.0, 20.0 * 128.0 + 64.0),
            None,
            FormationOrder::default()
        ));
        while host.state.object(cart, false).unwrap().status & status::MOVE == 0 {
            host.advance(50.0);
        }
        host.advance(50.0);
        let group = host.state.object(cart, false).unwrap().tick_group;
        set_group_progress(&mut host, group, 60.0);
        let head = host.state.object(cart, false).unwrap();
        let (head_pos, _) = host.display(head);
        let (pos, rotation) = host.display(host.state.object(tail, false).unwrap());
        assert_eq!(pos, head_pos);
        assert!(head_pos.x > head.world_pos[0], "the front part runs ahead");
        // The yaw follows the interpolated head position (angle from the drawbar point of the simulation).
        let centre = match &host
            .state
            .object(tail, false)
            .unwrap()
            .module(opennml_sim::module::multipart::PART)
            .unwrap()
            .kind
        {
            ModuleKind::MultiPart(p) => p.centre,
            _ => panic!("no rear part"),
        };
        let pose = host.walk_pose(head);
        assert_ne!(pose, (head.grid.x.0, head.grid.z.0, head.direction.0));
        let angle = opennml_sim::module::multipart::follow_angle((centre.0, centre.1), pose);
        assert!((rotation.y - yaw_of(angle)).abs() < 1e-6);
    }

    #[test]
    fn tent_and_trade_info_leaves_the_checksum_unchanged() {
        use crate::game::formation_order::FormationOrder;
        use crate::game::testworld::{CART, DWELLING_TENT, STORAGE_TENT, host, spawn};
        use crate::game::unit_bar::UnitBar;
        use opennml_control::{SimTentInfo, SimTradeInfo, SimTransportTentInfo};
        use opennml_sim::message::Message;
        use opennml_sim::messaging::send;
        use opennml_sim::module::gather::set_carried_load;
        use opennml_sim::module::tent;
        use opennml_sim::module::trading;
        let mut host = host();
        let tent = spawn(&mut host, DWELLING_TENT, 0, (10, 10));
        let storage = spawn(&mut host, STORAGE_TENT, 0, (20, 10));
        let cart = spawn(&mut host, CART, 0, (30, 30));
        {
            let rules = host.rules.clone();
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            // Loaded cart: trade kind 2 with target 3 and own tax values; the load (wood) is in the
            // object.
            let ModuleKind::Trading(t) = &mut sim.module_mut(cart, trading::INDEX).unwrap().kind
            else {
                panic!("no trading module");
            };
            *t = trading::Trading {
                trade_ware: 2,
                trade_target: 3,
                payer_guids: vec![1600, 1601],
                max_tax: 40,
                tax_interval: 60_000,
                tax_level: 70,
                ..trading::Trading::default()
            };
            set_carried_load(&mut sim, cart, 120, 1);
        }
        let info = |host: &SimHost, id: ObjectId| {
            let result = host.query(Some(0), None, Some(&[id.0]), 10);
            result.objects.into_iter().find(|o| o.id == id.0).unwrap()
        };
        // Tent before packing: at rest, visible, in the collision manager; a building without module 42 has
        // only `in_collision`.
        let before = info(&host, tent);
        assert_eq!(
            before.tent,
            Some(SimTentInfo {
                status: 0,
                counter: 0,
                progress: host.state.object(tent, false).unwrap().progress_scale,
                travois_guid: 1717,
                travois: u32::MAX,
                hidden: false,
            })
        );
        assert_eq!(before.in_collision, Some(true));
        assert!(before.transport_tent.is_none() && before.trade.is_none());
        let plain = info(&host, storage);
        assert!(plain.tent.is_none() && plain.in_collision == Some(true));
        // The cart has no building module: no `in_collision`.
        let cart_info = info(&host, cart);
        assert_eq!(cart_info.in_collision, None);
        assert_eq!(
            cart_info.trade,
            Some(SimTradeInfo {
                ware: 2,
                target: 3,
                tax_level: 70,
                max_tax: 40,
                tax_interval: 60_000,
                payers: vec![1600, 1601],
                load_amount: 120,
                load_kind: 1,
            })
        );
        assert!(cart_info.tent.is_none() && cart_info.transport_tent.is_none());
        let loaded = host.state.object(cart, false).unwrap();
        assert_eq!((loaded.load_amount, loaded.load_kind), (120, 1));

        // Packing: the tent fades out, the travois arises and drives off.
        {
            let rules = host.rules.clone();
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            assert_eq!(send(&mut sim, tent, &Message::new(tent::msg::PACK, &[])), 0);
        }
        let bar = UnitBar::default();
        let travois = {
            let ModuleKind::Tent(t) = &host
                .state
                .object(tent, false)
                .unwrap()
                .module(tent::INDEX)
                .unwrap()
                .kind
            else {
                panic!("no tent");
            };
            t.travois
        };
        assert!(host.move_units(
            &[travois],
            Vec3::new(5000.0, 0.0, 5000.0),
            None,
            FormationOrder::new(&bar, None)
        ));
        for _ in 0..6 {
            host.step_once();
        }
        let checksum = host.checksum();
        let ids = [tent, travois, storage, cart].map(|id| id.0);
        let result = host.query(Some(0), None, Some(&ids), 10);
        assert_eq!(
            host.checksum(),
            checksum,
            "the query changes the simulation"
        );
        let again = host.query(Some(0), None, Some(&ids), 10);
        assert_eq!(result.objects, again.objects, "the query is repeatable");
        let find = |id: ObjectId| result.objects.iter().find(|o| o.id == id.0).unwrap();
        let fading = host.state.object(tent, false).unwrap();
        let ModuleKind::Tent(state) = &fading.module(tent::INDEX).unwrap().kind else {
            panic!("no tent");
        };
        assert_eq!(state.status, tent::status::FADE_OUT);
        assert!(state.counter < tent::FADE_MS, "the crossfade is running");
        let t = find(tent).tent.as_ref().expect("tent section");
        assert_eq!(
            (t.status, t.counter, t.progress, t.travois_guid, t.travois),
            (1, state.counter, fading.progress_scale, 1717, travois.0)
        );
        assert!(t.progress < 1.0 && t.progress > 0.0);
        // Faded out: sleeping and not selectable; according to W-89 it is removed from the collision
        // manager.
        assert!(t.hidden);
        assert_eq!(find(tent).in_collision, Some(false));
        assert_eq!(
            find(travois).transport_tent,
            Some(SimTransportTentInfo {
                tent: tent.0,
                transporting: true,
            })
        );
        assert!(find(travois).walk.is_some(), "the travois drives");
        assert!(find(travois).tent.is_none() && find(travois).in_collision.is_none());
        // The unchanged neighbors stay as before.
        assert_eq!(find(storage).in_collision, Some(true));
        assert_eq!(find(cart).trade, cart_info.trade);

        // Setting up on the same tile (W-89): the tent is visible and selectable again but is still not in the
        // collision manager.
        for _ in 0..50 {
            host.step_once();
        }
        assert_eq!(info(&host, tent).tent.unwrap().status, 0);
        let grid = host.state.object(tent, false).unwrap().grid;
        {
            let rules = host.rules.clone();
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            let unpack = Message::new(tent::msg::UNPACK, &[travois.as_i32(), grid.x.0, grid.z.0]);
            assert_eq!(send(&mut sim, tent, &unpack), 0);
        }
        for _ in 0..50 {
            host.step_once();
        }
        let placed = info(&host, tent);
        let t = placed.tent.expect("tent section");
        assert_eq!((t.status, t.counter, t.hidden), (0, 0, false));
        assert_eq!(placed.in_collision, Some(false));
    }

    #[test]
    fn vehicle_info_leaves_the_checksum_unchanged() {
        use super::testkit::*;
        use opennml_control::{SimMultiPartInfo, SimTrainInfo};
        use opennml_sim::module::{multipart, quarter, shoot, train};
        let mut host = train_host();
        let loco = spawn_dir(&mut host, LOCO, 0, (10, 10), 0);
        let tender = spawn_dir(&mut host, TENDER, 0, (8, 10), 0);
        let cart = spawn_dir(&mut host, CART, 0, (20, 20), 0);
        let ship = spawn_dir(&mut host, TRANSPORT, 0, (30, 30), 0);
        let warship = spawn_dir(&mut host, WARSHIP, 0, (40, 30), 0);
        // The rear part arises in the third object tick.
        for _ in 0..4 {
            object_tick(&mut host);
        }
        let tail = cart_tail(&host, cart);
        {
            let rules = host.rules.clone();
            let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
            let t = train::train_mut(&mut sim, loco).unwrap();
            t.slaves = vec![tender];
            t.speed = 410;
            t.next_speed = 430;
            t.acc_time = 900;
            t.status = train::status::FORWARD | train::status::MOVING | train::status::ACCEL;
            t.dir = (4096, 0);
            t.target = Some((35, 30));
            t.time_counter = 1500;
            t.has_supply = true;
            t.outside = 2;
            t.raster_dir = (1, 0);
            t.last_block_tile = (36, 30);
            let t = train::train_mut(&mut sim, tender).unwrap();
            t.master = loco;
            t.is_slave = true;
            let ModuleKind::Quarter(q) = &mut sim.module_mut(ship, quarter::INDEX).unwrap().kind
            else {
                panic!("no quarters");
            };
            q.meeting_point = Some((40 << 12, 41 << 12));
            q.waiting = vec![loco, tender];
            q.status = 1;
            let ModuleKind::Shoot(s) = &mut sim.module_mut(warship, shoot::INDEX).unwrap().kind
            else {
                panic!("no shoot module");
            };
            s.is_steuerboard = true;
            s.cannon_counter = 5;
            s.is_busy = true;
            s.can_shoot = false;
            multipart::head_mut(&mut sim, cart).unwrap().last_pos = (7 << 12, 9 << 12, 90 << 12);
            multipart::part_mut(&mut sim, tail).unwrap().centre = (3 << 12, 4 << 12, 45 << 12);
        }
        host.state.object_event_bits.insert(loco, 0x101);
        let before = host.checksum();
        let ids = [loco, tender, cart, tail, ship, warship].map(|id| id.0);
        let result = host.query(Some(0), None, Some(&ids), 10);
        assert_eq!(host.checksum(), before, "the query changes the simulation");
        let info = |id: ObjectId| result.objects.iter().find(|o| o.id == id.0).unwrap();
        // Locomotive: train section as in the state, event bits from the table.
        assert_eq!(
            info(loco).train,
            Some(SimTrainInfo {
                slaves: vec![tender.0],
                master: u32::MAX,
                speed: 410,
                next_speed: 430,
                acc_time: 900,
                status: 0xB,
                is_slave: false,
                dir: [4096, 0],
                target: Some([35, 30]),
                time_counter: 1500,
                has_supply: true,
                outside: 2,
                raster_dir: [1, 0],
                last_block_tile: [36, 30],
            })
        );
        assert_eq!(info(loco).event_bits, 0x101);
        // Tender: slave with a reference to the locomotive, without target and without event bits.
        let slave = info(tender).train.as_ref().expect("train module");
        assert_eq!((slave.master, slave.is_slave), (loco.0, true));
        assert!(slave.slaves.is_empty() && slave.target.is_none());
        assert_eq!(info(tender).event_bits, 0);
        // Front part and rear part of the cart.
        assert_eq!(
            info(cart).multipart,
            Some(SimMultiPartInfo {
                head: true,
                linked: tail.0,
                prev: u32::MAX,
                next: u32::MAX,
                offset: 0,
                centre: [0; 3],
                last_pos: [7 << 12, 9 << 12, 90 << 12],
            })
        );
        let part = info(tail).multipart.as_ref().expect("rear part");
        assert!(!part.head);
        assert_eq!((part.prev, part.next), (cart.0, u32::MAX));
        assert_eq!(part.offset, 5300);
        assert_eq!(part.centre, [3 << 12, 4 << 12, 45 << 12]);
        assert!(info(loco).multipart.is_none() && info(cart).train.is_none());
        // Quarters of the ship: meeting point, waiting, state.
        let q = info(ship).quarter.as_ref().expect("quarters");
        assert_eq!(q.meeting_point, Some([40 << 12, 41 << 12]));
        assert_eq!((q.waiting.clone(), q.status), (vec![loco.0, tender.0], 1));
        // Broadside of the warship.
        let shoot = info(warship)
            .ranged
            .as_ref()
            .and_then(|r| r.shoot.as_ref())
            .expect("shoot module");
        assert_eq!(
            (
                shoot.is_steuerboard,
                shoot.cannon_counter,
                shoot.busy,
                shoot.can_shoot
            ),
            (true, 5, true, false)
        );
        // Fields appear in the JSON only with a section: vehicles without a train lack them.
        let json = serde_json::to_value(info(ship)).unwrap();
        assert!(json.get("train").is_none() && json.get("multipart").is_none());
        assert!(json.get("event_bits").is_none());
        let json = serde_json::to_value(info(loco)).unwrap();
        assert_eq!(json["train"]["slaves"][0], tender.0);
        assert_eq!(json["event_bits"], 0x101);
        assert_eq!(host.checksum(), before);
    }

    /// Object ticks (two `step` each) without commands, bypassing the host (no recording).
    fn run_ticks(host: &mut SimHost, n: usize) {
        for _ in 0..2 * n {
            opennml_sim::tick::step(&mut host.state, &host.rules, &[]);
        }
    }

    /// Synchronous message `code` without values to `id`.
    fn say(host: &mut SimHost, id: ObjectId, code: u32) -> i32 {
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        opennml_sim::messaging::send(&mut sim, id, &opennml_sim::message::Message::new(code, &[]))
    }

    // QUIRK D-267: kept
    #[test]
    fn ghosts_and_evoked_animals_fade_in_the_look() {
        // re/ghosts-banish 3.4/7 (alpha via `0x00468090`), re/evoke-aura 7 (appearing 0 -> 1 in 80
        // ticks): ghost and animal alpha in the rendering; camouflage (module 25) takes precedence, the
        // opacity of the diver after it.
        use crate::game::testworld::{GHOST, WOLF, host, spawn};
        use opennml_sim::module::ghosts::ghost;
        let mut host = host();
        let g = spawn(&mut host, GHOST, 0, (20, 20));
        let wolf = spawn(&mut host, WOLF, 0, (24, 20));
        let alpha = |host: &mut SimHost, id| host.entry_look(id, 0).unwrap().alpha;
        let fade = |host: &SimHost, id| fade_alpha(host.state.object(id, false).unwrap());
        assert_eq!((fade(&host, g), fade(&host, wolf)), (Some(1.0), Some(1.0)));
        assert_eq!((alpha(&mut host, g), alpha(&mut host, wolf)), (1.0, 1.0));
        // A half faded-out ghost (only set, no sequence).
        let o = host.state.object_mut(g, false).unwrap();
        let Some(ModuleKind::Ghost(gh)) = o.module_mut(ghost::INDEX).map(|m| &mut m.kind) else {
            panic!("no ghost");
        };
        gh.alpha = 0.4;
        let sum = checksum(&host.state);
        assert_eq!(fade(&host, g), Some(0.4));
        assert_eq!(alpha(&mut host, g), 0.4);
        // A diving ghost keeps its alpha (precedence over 0.6). Camouflaged (only set) likewise:
        // alpha 0.5 is set only by module 25 (`0x0051D740`), the ghost has none (D-267).
        host.state.object_mut(g, false).unwrap().status |= status::DIVING;
        assert_eq!(alpha(&mut host, g), 0.4);
        host.state.object_mut(g, false).unwrap().status |= status::MASKED;
        assert_eq!(alpha(&mut host, g), 0.4);
        host.state.object_mut(g, false).unwrap().status &= !(status::MASKED | status::DIVING);
        assert_eq!(checksum(&host.state), sum, "rendering changes nothing");
        // Wolf appears (`0x414`, re/animals 1.4): alpha 0, +0.0125 per tick.
        say(&mut host, wolf, 0x414);
        assert_eq!(alpha(&mut host, wolf), 0.0);
        run_ticks(&mut host, 1);
        assert_eq!(alpha(&mut host, wolf), 0.0125);
        // An object without a ghost and animal module has no own alpha.
        let evoker = spawn(&mut host, crate::game::testworld::GHOST_EVOKER, 0, (28, 20));
        assert_eq!(fade(&host, evoker), None);
        assert_eq!(alpha(&mut host, evoker), 1.0);
    }

    // QUIRK K-13: deviation
    #[test]
    fn masked_units_are_half_transparent_for_everyone() {
        // re/masking 7: camouflaged unit alpha 0.5 (`0x00468090`) for every viewer; for foreigners
        // render bit `0x80` is drawn in addition (re/vision 7). Takes precedence over the opacity 0.6 of a
        // diver (SetRenderAlpha last).
        use crate::game::testworld::{ARCHER, MASK_UPGRADE, host, spawn, unlock};
        use opennml_sim::module::masking::msg;
        let mut host = host();
        unlock(&mut host, 0, MASK_UPGRADE);
        let archer = spawn(&mut host, ARCHER, 0, (20, 20));
        let look = |host: &SimHost, viewer| {
            masked_look(
                &host.state,
                host.state.object(archer, false).unwrap(),
                viewer,
            )
        };
        assert_eq!((look(&host, 0), look(&host, 1)), (None, None));
        assert_eq!(host.entry_look(archer, 0).unwrap().alpha, 1.0);
        // `0x119`, camouflaged after 18 ticks (re/masking 9 V1).
        assert_eq!(say(&mut host, archer, msg::START), 0);
        run_ticks(&mut host, 18);
        let o = host.state.object(archer, false).unwrap();
        assert_ne!(o.status & status::MASKED, 0);
        let sum = opennml_sim::save::checksum(&host.state);
        for viewer in [0, 1] {
            assert_eq!(look(&host, viewer), Some((None, MASKED_ALPHA)));
            host.active_player = viewer;
            assert_eq!(host.entry_look(archer, 0).unwrap().alpha, MASKED_ALPHA);
            // The animation stays that of the simulation.
            let o = host.state.object(archer, false).unwrap();
            assert_eq!(
                viewer_anim(&host.state, &host.rules, o, viewer),
                display_anim(&host.rules, o)
            );
        }
        // Camouflaged and diving (only set, no sequence): 0.5 instead of 0.6.
        host.state.object_mut(archer, false).unwrap().status |= status::DIVING;
        assert_eq!(host.entry_look(archer, 0).unwrap().alpha, MASKED_ALPHA);
        host.state.object_mut(archer, false).unwrap().status &= !status::DIVING;
        assert_eq!(opennml_sim::save::checksum(&host.state), sum);
        // Uncamouflaged (`0x219`, after the uncamouflage animation): opaque again.
        assert_eq!(say(&mut host, archer, msg::END), 0);
        run_ticks(&mut host, 18);
        assert_eq!(look(&host, 0), None);
        assert_eq!(host.entry_look(archer, 0).unwrap().alpha, 1.0);
    }

    // QUIRK K-13, D-235: deviation
    #[test]
    fn the_alarm_post_shows_the_tree_to_foreigners() {
        // re/masking 7, D-235: viewers with `View[owner] & (1 << viewer)` see
        // `260001[0]` (tree with post) with alpha 0.5, all others `260001[1 % n]` (tree only) opaque;
        // the direction is the reverse of `render_bits_for` (`shares_vision(viewer, owner)`).
        use crate::game::testworld::{ALARM_POST, host, spawn};
        use opennml_sim::module::masking::msg;
        let mut host = host();
        let post = spawn(&mut host, ALARM_POST, 0, (20, 20));
        let look = |host: &SimHost, viewer| {
            masked_look(&host.state, host.state.object(post, false).unwrap(), viewer)
        };
        let anim = |host: &SimHost, viewer| {
            let o = host.state.object(post, false).unwrap();
            viewer_anim(&host.state, &host.rules, o, viewer)
        };
        // `EVERYTIMETARNED`: camouflaged in the first tick, the simulation sets `anim[0]`.
        run_ticks(&mut host, 1);
        let o = host.state.object(post, false).unwrap();
        assert_ne!(o.status & status::MASKED, 0);
        assert_eq!(o.anim.current, 0);
        let sum = opennml_sim::save::checksum(&host.state);
        // Owner: post half transparent, animation of the simulation.
        assert_eq!(look(&host, 0), Some((None, MASKED_ALPHA)));
        assert_eq!(anim(&host, 0), DisplayAnim::Sim(0));
        // Enemy without shared sight: only the tree, opaque.
        assert_eq!(look(&host, 1), Some((Some(1), 1.0)));
        assert_eq!(anim(&host, 1), DisplayAnim::Sim(1));
        host.active_player = 1;
        assert_eq!(host.entry_look(post, 0).unwrap().alpha, 1.0);
        // The owner shares its sight with 1 (`View[0]` bit 1): 1 sees the post.
        host.state.diplomacy.view[0] |= 1 << 1;
        assert!(host.state.diplomacy.shares_vision(0, 1));
        assert_eq!(look(&host, 1), Some((None, MASKED_ALPHA)));
        assert_eq!(anim(&host, 1), DisplayAnim::Sim(0));
        assert_eq!(host.entry_look(post, 0).unwrap().alpha, MASKED_ALPHA);
        // Only 1 shares with the owner (`View[1]` bit 0): 1 still sees the tree.
        host.state.diplomacy.view[0] &= !(1 << 1);
        host.state.diplomacy.view[1] |= 1;
        assert!(host.state.diplomacy.shares_vision(1, 0));
        assert!(!host.state.diplomacy.shares_vision(0, 1));
        assert_eq!(look(&host, 1), Some((Some(1), 1.0)));
        assert_eq!(anim(&host, 1), DisplayAnim::Sim(1));
        host.state.diplomacy.view[1] &= !1;
        assert_eq!(opennml_sim::save::checksum(&host.state), sum);
        // Uncamouflaged (`0x219` takes effect immediately on the building): everyone sees `anim[0]` opaque.
        assert_eq!(say(&mut host, post, msg::END), 0);
        assert_eq!((look(&host, 0), look(&host, 1)), (None, None));
        assert_eq!(anim(&host, 1), DisplayAnim::Sim(0));
    }

    /// Archer with module 25 (2924) on slot `slot`, camouflaged after `0x119` and 18 ticks.
    fn masked_archer(host: &mut SimHost, slot: u8, tile: (i32, i32)) -> ObjectId {
        use crate::game::testworld::{ARCHER, MASK_UPGRADE, spawn, unlock};
        use opennml_sim::module::masking::msg;
        unlock(host, slot, MASK_UPGRADE);
        let archer = spawn(host, ARCHER, i32::from(slot), tile);
        assert_eq!(say(host, archer, msg::START), 0);
        run_ticks(host, 18);
        let o = host.state.object(archer, false).unwrap();
        assert_ne!(o.status & status::MASKED, 0, "camouflaged");
        archer
    }

    /// Render bits and opacity of `id` for the viewer `viewer`.
    fn look_for(host: &mut SimHost, id: ObjectId, viewer: u8) -> (u32, f32) {
        host.active_player = viewer;
        let look = host.entry_look(id, 0).unwrap();
        (look.bits, look.alpha)
    }

    // QUIRK D-263: kept
    #[test]
    fn stealthed_foreigner_is_drawn_only_while_moving() {
        // re/world-display W.2.1, D-263 (`ObjectRenderer_Draw` `0x0054A3E1`): for a viewer
        // without shared sight the sight check delivers only `0x80`; the silhouette is drawn only with
        // `MOVE`, standing nothing - no own effects either.
        use crate::game::map::draws_own_effects;
        use crate::game::testworld::host;
        use opennml_render::scene::{RENDER_MODEL, RENDER_STEALTH};
        let mut host = host();
        let archer = masked_archer(&mut host, 1, (20, 20));
        assert!(!host.state.diplomacy.shares_vision(0, 1));
        let sum = checksum(&host.state);
        let standing = look_for(&mut host, archer, 0);
        assert_eq!(standing.0, 0, "invisible while standing");
        let masked = host.state.object(archer, false).unwrap().status;
        assert!(!draws_own_effects(standing.0, masked));
        host.state.object_mut(archer, false).unwrap().status |= status::MOVE;
        let walking = look_for(&mut host, archer, 0);
        assert_eq!(
            walking.0, RENDER_STEALTH,
            "only the silhouette while walking"
        );
        let moving = host.state.object(archer, false).unwrap().status;
        assert!(!draws_own_effects(walking.0, moving));
        // The owner sees the real bits, walking as well as standing.
        let own = look_for(&mut host, archer, 1);
        assert_ne!(own.0 & RENDER_MODEL, 0);
        assert_eq!(own.0 & RENDER_STEALTH, 0);
        host.state.object_mut(archer, false).unwrap().status &= !status::MOVE;
        assert_eq!(look_for(&mut host, archer, 1).0, own.0);
        assert_eq!(checksum(&host.state), sum, "rendering changes nothing");
    }

    // QUIRK D-267: kept
    #[test]
    fn owner_sees_half_alpha_with_shadow() {
        // re/masking 7, re/world-display W.2.2: module 25 sets alpha 0.5 in `0x319` (`0x0051D740`), the
        // shadow stays; likewise for a player with shared sight (`View[owner]`).
        use crate::game::testworld::host;
        use opennml_render::scene::{RENDER_MODEL, RENDER_SHADOW, RENDER_STEALTH};
        let mut host = host();
        let archer = masked_archer(&mut host, 0, (20, 20));
        for viewer in [0, 1] {
            if viewer == 1 {
                host.state.diplomacy.view[1] |= 1;
                assert!(host.state.diplomacy.shares_vision(1, 0));
            }
            let (bits, alpha) = look_for(&mut host, archer, viewer);
            assert_eq!(alpha, MASKED_ALPHA, "viewer {viewer}");
            assert_eq!(
                bits & (RENDER_MODEL | RENDER_SHADOW | RENDER_STEALTH),
                RENDER_MODEL | RENDER_SHADOW,
                "viewer {viewer}"
            );
        }
    }

    // QUIRK D-267: kept
    #[test]
    fn buried_assassin_is_opaque_for_the_owner_and_hidden_for_foreigners() {
        // re/world-display W.2.2, D-267: `0x11C` (`0x004FFA00`) sets `MASKED | WAITING4KILLING`,
        // but no alpha; `0x41C` clears render bit 2. Owner: opaque without shadow, with
        // `meuchel_eingraben.pef`. Foreigners without reconnaissance: `0x80` without `MOVE` -> nothing including the
        // effect. Foreigners with reconnaissance (fog bit 4): the real bits.
        use crate::game::map::draws_own_effects;
        use crate::game::testworld::{ASSASSIN, DIG_IN_UPGRADE, host, spawn, unlock};
        use opennml_render::scene::{RENDER_MODEL, RENDER_SHADOW};
        use opennml_sim::module::assassin;
        let mut host = host();
        unlock(&mut host, 0, DIG_IN_UPGRADE);
        let killer = spawn(&mut host, ASSASSIN, 0, (20, 20));
        // The assassin has no module 25 (re/masking 1.1).
        let o = host.state.object(killer, false).unwrap();
        assert!(!o.has_module(opennml_sim::module::masking::INDEX, false));
        assert_eq!(say(&mut host, killer, assassin::msg::GO_DOWN), 0);
        assert!(host.state.events.iter().any(|e| matches!(
            e,
            SimEvent::EffectOnObject { object, path }
                if *object == killer && path == assassin::EFFECT_GO_DOWN
        )));
        run_ticks(&mut host, 40);
        let o = host.state.object(killer, false).unwrap();
        let ModuleKind::Assassin(a) = &o.module(assassin::INDEX).unwrap().kind else {
            panic!("assassin module");
        };
        assert!(a.is_down, "dug in");
        let (render_bits, tile, dug_in) = (o.render_bits, o.tile, o.status);
        assert_ne!(dug_in & status::MASKED, 0);
        assert_eq!(render_bits & RENDER_SHADOW, 0, "no shadow");
        assert_eq!(
            masked_look(&host.state, host.state.object(killer, false).unwrap(), 0),
            None
        );
        let sum = checksum(&host.state);
        // Owner: opaque, without shadow, the effect runs.
        let own = look_for(&mut host, killer, 0);
        assert_eq!(own, (render_bits, 1.0));
        assert_ne!(own.0 & RENDER_MODEL, 0);
        assert!(draws_own_effects(own.0, dug_in));
        // Foreign without reconnaissance: nothing, no effect either.
        assert!(!host.state.diplomacy.shares_vision(1, 0));
        let foreign = look_for(&mut host, killer, 1);
        assert_eq!(foreign.0, 0);
        assert!(!draws_own_effects(foreign.0, dug_in));
        assert_eq!(checksum(&host.state), sum, "rendering changes nothing");
        // Foreign with reconnaissance (`cleared`, bit 4 of the fog): normally visible and opaque.
        host.state.fog.cleared.or(tile.0, tile.1, 1 << 1);
        let cleared = look_for(&mut host, killer, 1);
        assert_eq!(cleared, (render_bits, 1.0));
        assert!(draws_own_effects(cleared.0, dug_in));
    }

    // QUIRK D-267: kept
    #[test]
    fn mounted_carried_or_dead_stealthed_units_draw_no_ghost() {
        // No silhouette bit `0x80` beyond the original: mounting sends `0x619` (`DoMount`, re/riding 2.4)
        // and uncamouflages immediately; quartered yields `entry_look` 0 (`QUARTERED`); death clears
        // `MASKED` (`dying.rs`, `0x0050EE30`), the carcass appears with its real bits.
        use crate::game::testworld::host;
        use opennml_render::scene::RENDER_STEALTH;
        use opennml_sim::module::masking::msg;
        let mut host = host();
        // Quartered (also walking): bits 0.
        let carried = masked_archer(&mut host, 1, (20, 20));
        host.state.object_mut(carried, false).unwrap().status |= status::QUARTERED | status::MOVE;
        assert_eq!(look_for(&mut host, carried, 0).0, 0);
        // Mounting: `0x619` uncamouflages immediately, the rider shows its real bits, also walking.
        let rider = masked_archer(&mut host, 1, (24, 20));
        assert_eq!(say(&mut host, rider, msg::DEMASK_NOW), 1);
        host.state.object_mut(rider, false).unwrap().status |= status::ISMOUNTED | status::MOVE;
        let o = host.state.object(rider, false).unwrap();
        assert_eq!(o.status & status::MASKED, 0);
        let rider_bits = o.render_bits;
        let (bits, alpha) = look_for(&mut host, rider, 0);
        assert_eq!(bits & RENDER_STEALTH, 0);
        assert_eq!((bits, alpha), (rider_bits, 1.0));
        // Dead: `0x110` clears `MASKED`; the carcass appears for everyone with its real bits.
        let dead = masked_archer(&mut host, 1, (28, 20));
        assert_eq!(say(&mut host, dead, 0x110), 0);
        let o = host.state.object(dead, false).unwrap();
        assert_ne!(o.status & status::DIED, 0);
        assert_eq!(o.status & status::MASKED, 0);
        let dead_bits = o.render_bits;
        assert_eq!(look_for(&mut host, dead, 0), (dead_bits, 1.0));
        assert_eq!(dead_bits & RENDER_STEALTH, 0);
    }

    /// Chat command `0x5D` of slot 0.
    fn chat(text: &str) -> Command {
        Command {
            player: 0,
            kind: opennml_sim::command::CommandKind::Chat {
                player: 0,
                mask: 1,
                text: text.encode_utf16().collect(),
            },
        }
    }

    /// Fog switch in the host (deviation from the original): `rainbowring` reports [`SimEvent::FogToggle`], the host
    /// toggles `m_FogShowMode` 0 <-> 7 like `FUN_00476860(-1)`, also a value set from outside (the state is checked
    /// by `command::chat` in `opennml-sim`).
    #[test]
    fn rainbowring_toggles_fog_in_the_host() {
        use crate::game::fog_view::FOG_OFF;
        let mut host = host();
        host.submit(chat("rainbowring"));
        let events = host.step_once();
        assert!(events.contains(&SimEvent::FogToggle));
        assert!(!events.iter().any(|e| matches!(e, SimEvent::Chat { .. })));
        assert_eq!(host.fog.show_mode, FOG_OFF);
        assert!(!host.section(&[]).fog_shown);
        host.submit(chat("rainbowring"));
        host.step_once();
        assert_eq!(host.fog.show_mode, 0);
        // `FUN_00476860(-1)`: every value != 0 becomes 0.
        host.fog.show_mode = 3;
        host.submit(chat("rainbowring"));
        host.step_once();
        assert_eq!(host.fog.show_mode, 0);
        // It also toggles with a halted match, with the next tick (the event stays in the state until then).
        host.halted = true;
        host.submit(chat("rainbowring"));
        assert_eq!(host.fog.show_mode, 0);
        host.halted = false;
        host.step_once();
        assert_eq!(host.fog.show_mode, FOG_OFF);
    }

    /// Chat and cheats are in the command log: the replay gives the same checksum (gold from `goldenmonkey`)
    /// and toggles the fog switch of the host as often as live (`rainbowring` twice -> on, three times -> off).
    #[test]
    fn chat_replays_from_the_command_log() {
        use crate::game::fog_view::FOG_OFF;
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut live = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        let gold = |h: &SimHost| h.state.players[0].resources_fx[0] >> 12;
        let start = gold(&live);
        for (i, text) in [
            "hallo",
            "goldenmonkey",
            "rainbowring",
            "rainbowring",
            "rainbowring",
        ]
        .into_iter()
        .enumerate()
        {
            live.submit(chat(text));
            let events = live.step_once();
            let line = events.iter().any(|e| matches!(e, SimEvent::Chat { .. }));
            assert_eq!(line, i == 0, "{text}: line only without a cheat");
        }
        live.step_once();
        assert_eq!(gold(&live), start + 1000);
        assert_eq!(live.fog.show_mode, FOG_OFF, "toggled three times");
        assert_eq!(
            live.log
                .iter()
                .filter(|c| c.command.kind.type_code() == 0x5000_005D)
                .count(),
            5
        );
        let replay = Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        let mut other = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        other.load_replay(replay, Some(live.state.steps)).unwrap();
        assert_eq!(other.checksum(), live.checksum());
        assert_eq!(other.fog.show_mode, FOG_OFF);
        assert_eq!(gold(&other), start + 1000);
    }

    /// O-392: `bloodmode` toggles the blood switch of the program in the tick (without a chat line, checksum as
    /// without a cheat); the replay toggles it likewise, but from the current value (it applies until the program
    /// ends, no restart per match as with the fog switch). The only test that changes the process-wide switch; it
    /// restores it at the end.
    // QUIRK O-392: kept
    #[test]
    fn bloodmode_toggles_the_program_blood_switch() {
        use crate::game::blood;
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let before = blood::enabled();
        blood::init(true);
        let mut live = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        let mut plain = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
        live.submit(chat("bloodmode"));
        let events = live.step_once();
        plain.step_once();
        assert!(events.contains(&SimEvent::BloodToggle));
        assert!(!events.iter().any(|e| matches!(e, SimEvent::Chat { .. })));
        assert!(!blood::enabled(), "toggled");
        assert_eq!(live.checksum(), plain.checksum(), "rendering only");
        live.step_once();
        assert!(!blood::enabled(), "stays");
        let replay = live.replay().unwrap();
        let mut other = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        other.load_replay(replay, Some(live.state.steps)).unwrap();
        let replayed = blood::enabled();
        blood::init(before);
        assert!(replayed, "the replay keeps toggling from the current value");
        assert_eq!(other.checksum(), live.checksum());
    }

    /// The replay begins with the fog switch of a new match (0) and toggles it per `rainbowring`; the value before
    /// loading (here "Fog off" of the viewer) does not count.
    #[test]
    fn replay_starts_the_fog_switch_from_a_new_match() {
        use crate::game::fog_view::FOG_OFF;
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        for (cheats, expected) in [(0, 0), (1, FOG_OFF), (2, 0)] {
            let mut live = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
            live.submit(chat("hallo"));
            live.step_once();
            for _ in 0..cheats {
                live.submit(chat("rainbowring"));
                live.step_once();
            }
            live.step_once();
            assert_eq!(live.fog.show_mode, expected, "live, {cheats} x rainbowring");
            let replay = live.replay().unwrap();
            let mut other = SimHost::new(rules.clone(), setup.clone(), DEFAULT_GAME_SPEED).unwrap();
            other.set_fog_shown(false);
            other.load_replay(replay, Some(live.state.steps)).unwrap();
            assert_eq!(
                other.fog.show_mode, expected,
                "replay, {cheats} x rainbowring, fog off before"
            );
        }
    }

    // --- Flag query ----------------------------------------------------------------------------

    const FLAG_TYPES: &str = "GUID 4300
NAME Spieler_Fahne
VALUEID 100 6 1 1024
PROPID 5
VALUEID 60001 6 1 1
VALUEID 60002 6 1 5888
";

    /// Host with colonist and HQ (`plain_testkit`) and the flag type; game type `mode`, three neutral flags.
    fn flag_host(mode: i32, flags: usize) -> SimHost {
        let rules = Rules::synthetic(&format!("{TYPES}{FLAG_TYPES}"), 16);
        let setup = MatchSetup {
            mode: -1,
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        host.state.mode = mode;
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        for n in 0..flags {
            let x = 200.0 + 200.0 * n as f32;
            create_object(&mut sim, &mut ObjectDesc::new(4300, 8, [x, 0.0, 200.0])).unwrap();
        }
        host
    }

    #[test]
    fn flags_info_reports_flags_counts_leader_and_time() {
        let mut host = flag_host(2, 3);
        host.state.time_limit = 1200;
        // Neutral flags: color 9, holder 8, counter 8 = 3, no leader, no timer.
        let info = flags_info(&host.state).expect("game type 2 reports flags");
        assert_eq!(info.flags.len(), 3);
        assert!(info.flags.windows(2).all(|w| w[0].id != w[1].id));
        for f in &info.flags {
            assert_eq!((f.color, f.holder, f.check_ms), (9, 8, 1000), "{f:?}");
        }
        assert_eq!(info.counts, [0, 0, 0, 0, 0, 0, 0, 0, 3]);
        assert_eq!(
            (info.leader, info.time_limit_s, info.remaining_ms),
            (8, 1200, None)
        );
        // Slot 3 holds the second flag (color = slot 3), leader 3 with a running timer.
        let second = info.flags[1].id;
        host.state
            .object_mut(ObjectId(second), false)
            .unwrap()
            .color = 3;
        host.state.flag_counts = [0, 0, 0, 1, 0, 0, 0, 0, 2];
        host.state.flag_leader = 3;
        host.state.countdowns.insert(0, 45_000);
        let info = flags_info(&host.state).unwrap();
        assert_eq!(info.flags[1].holder, 3);
        assert_eq!(info.flags[1].color, 3);
        assert_eq!((info.flags[0].holder, info.flags[2].holder), (8, 8));
        assert_eq!(info.counts, [0, 0, 0, 1, 0, 0, 0, 0, 2]);
        assert_eq!((info.leader, info.remaining_ms), (3, Some(45_000)));
        // The tile comes from the object.
        let tile = host.state.object(ObjectId(second), false).unwrap().tile;
        assert_eq!(info.flags[1].tile, [tile.0, tile.1]);
        // The section `sim` carries the same query.
        assert_eq!(host.section(&[]).flags, Some(info));
    }

    #[test]
    fn flags_info_exists_only_with_a_flag_mode_or_a_living_flag() {
        // Game type 2/3 also without a flag; otherwise only with a living flag 4300.
        assert_eq!(flags_info(&flag_host(2, 0).state).unwrap().flags, vec![]);
        assert!(flags_info(&flag_host(3, 0).state).is_some());
        for mode in [-1, 0, 1, 4] {
            assert_eq!(flags_info(&flag_host(mode, 0).state), None, "mode {mode}");
            assert_eq!(flag_host(mode, 0).section(&[]).flags, None);
            let with_flag = flags_info(&flag_host(mode, 1).state);
            assert_eq!(with_flag.map(|i| i.flags.len()), Some(1), "mode {mode}");
        }
        // Without the nature timer (-1 off) the flag reports -1.
        let mut host = flag_host(2, 1);
        let id = flags_info(&host.state).unwrap().flags[0].id;
        if let Some(m) = host
            .state
            .object_mut(ObjectId(id), false)
            .unwrap()
            .module_mut(opennml_sim::module::natur::INDEX)
            && let ModuleKind::Natur(n) = &mut m.kind
        {
            n.timer = -1;
        }
        assert_eq!(flags_info(&host.state).unwrap().flags[0].check_ms, -1);
    }

    #[test]
    fn flags_info_leaves_the_checksum_alone() {
        let mut host = flag_host(2, 3);
        host.state.countdowns.insert(0, 30_000);
        let before = host.checksum();
        let state_before = host.state.clone();
        for _ in 0..3 {
            let _ = flags_info(&host.state);
            let _ = host.section(&[]);
        }
        assert_eq!(host.checksum(), before);
        assert_eq!(host.state, state_before);
    }

    /// Commands of a recording for the replay tests: diplomacy at tick 1 and 2, defeat of slot 0 at tick 3 (the
    /// test host already has result 3 for slot 0; the timing is secured by comparing log and checksum with the
    /// live match).
    fn recorded_commands() -> Vec<RecordedCommand> {
        let diplomacy = |step| RecordedCommand {
            step,
            command: Command {
                player: 0,
                kind: CommandKind::Diplomacy {
                    p: 0,
                    q: 1,
                    mode: 0,
                },
            },
        };
        vec![
            diplomacy(1),
            diplomacy(2),
            RecordedCommand {
                step: 3,
                command: Command {
                    player: 0,
                    kind: CommandKind::SetPlayerResult {
                        slot: 0,
                        result: opennml_sim::game_end::DEFEAT,
                    },
                },
            },
        ]
    }

    /// In replay mode `step_once` takes the commands of the file at their tick (before the tick, like
    /// `Replay::play`), then the accepted ones from `pending`; checksum and log equal the recorded match. The host
    /// discards own commands, only the giving up of the view slot goes through.
    // QUIRK E-331: kept
    #[test]
    fn playback_feeds_commands_before_their_step() {
        let commands = recorded_commands();
        let mut live = host();
        let mut playback = host();
        playback.begin_playback(commands.clone());
        assert_eq!(
            playback.playback(),
            Some(PlaybackStatus {
                next_step: Some(1),
                ended: false
            })
        );
        for step in 0..6u64 {
            let now: Vec<Command> = commands
                .iter()
                .filter(|c| c.step == step)
                .map(|c| c.command.clone())
                .collect();
            live.step_with(now);
            playback.step_once();
            assert_eq!(playback.state.steps, step + 1);
            assert_eq!(playback.checksum(), live.checksum(), "tick {}", step + 1);
        }
        assert_eq!(playback.log, live.log, "log like the file");
        assert_eq!(playback.log, commands);

        // Watching only: commands via `submit`/`execute_now` and the mirror do not go through.
        let mut host = host();
        host.begin_playback(Vec::new());
        let view = host.active_player;
        let diplomacy = Command {
            player: view,
            kind: CommandKind::Diplomacy {
                p: i32::from(view),
                q: 1,
                mode: 0,
            },
        };
        host.submit(diplomacy.clone());
        host.execute_now(diplomacy);
        host.press_button(7);
        host.sync_selection(&[ObjectId(0x800)]);
        assert!(host.pending_commands().is_empty());
        assert!(host.log.is_empty());
        // Foreign giving up (other slot) no, the own one yes - via `submit` at the tick boundary.
        let surrender = |slot| Command {
            player: view,
            kind: CommandKind::SetPlayerResult {
                slot,
                result: opennml_sim::game_end::DEFEAT,
            },
        };
        host.submit(surrender(view + 1));
        host.submit(Command {
            player: view,
            kind: CommandKind::SetPlayerResult {
                slot: view,
                result: opennml_sim::game_end::VICTORY,
            },
        });
        assert!(host.pending_commands().is_empty(), "only the defeat");
        host.submit(surrender(view));
        assert_eq!(host.pending_commands(), [surrender(view)]);
        host.step_once();
        assert_eq!(
            host.log,
            [RecordedCommand {
                step: 0,
                command: surrender(view)
            }]
        );
        // Immediately (giving up in the game menu, `execute_now`) likewise.
        host.execute_now(surrender(view));
        assert_eq!(host.log.len(), 2);
    }

    /// E-331: at the end of the file the replay switches itself off silently - exactly one log entry, the
    /// match continues to compute, the locks stay.
    // QUIRK E-331: kept
    #[test]
    fn playback_end_is_logged_once() {
        let mut host = host();
        host.begin_playback(recorded_commands());
        for _ in 0..3 {
            host.step_once();
        }
        assert_eq!(
            host.playback(),
            Some(PlaybackStatus {
                next_step: Some(3),
                ended: false
            })
        );
        assert_eq!(host.playback.as_ref().unwrap().end_notices, 0);
        for _ in 0..20 {
            host.step_once();
        }
        assert_eq!(host.state.steps, 23, "the match continues");
        assert_eq!(
            host.playback(),
            Some(PlaybackStatus {
                next_step: None,
                ended: true
            })
        );
        assert_eq!(
            host.playback.as_ref().unwrap().end_notices,
            1,
            "logged once"
        );
        host.submit(Command {
            player: 0,
            kind: CommandKind::Diplomacy {
                p: 0,
                q: 1,
                mode: 0,
            },
        });
        assert!(host.pending_commands().is_empty(), "lock stays");
        // Without a replay no status.
        assert_eq!(super::plain_testkit::host().playback(), None);
    }
}
