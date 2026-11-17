//! Economy in the control channel (protocol 9, `docs/viewer-control.md`): `sim_build`, `sim_train`,
//! `sim_cancel`, `sim_complete` (debug), `sim_sell`, `sim_action` and `sim_menu` create the same commands as the
//! UI ([`crate::game::economy`]: placement mode, production list, right-click) and check beforehand
//! what the UI makes visible (building site, block reason, queue), so that check scripts get a stated
//! reason instead of a silently rejected command. Plus the economy sections of `sim_query` (building, gatherer,
//! construction worker, quarters, deposit, tree), the combat section (protocol 11) and the ranged-combat section
//! with animals, hunting and projectiles (protocol 12). `sim_upgrade` and the upgrade sections of `sim_query` (spell,
//! arming, execution list; protocol 16). `sim_trade`, `sim_tribute` and the tent and trade sections of `sim_query`
//! (protocol 17). Gate state and area watch of `sim_query` (protocol 18). Stealth, assassin and healing state,
//! message filter and presentation per viewer of `sim_query` (protocol 19). Ghost, banishment, animal evocation and
//! influence state of `sim_query` (protocol 20). Target point and corpse of the spell, demolition expert, killer and
//! smith state, storm pulse of `sim_query` (protocol 21). Headhunter and robber state, headhunter table and throw of
//! `sim_query`, `sim_hire_menu` (protocol 22). `ki_query` (protocol 23). `ki_state` and `ki_command` (protocol 26).
//! `script_state` (protocol 28). `sim_hash`, `sim_query` and `sim_command` for the viewer and `nml-game`
//! ([`execute_sim`]).
//! Without winit and GPU (testable).

use glam::Vec3;
use nml_control::Command as ControlCommand;
use nml_control::{
    DEFAULT_SCRIPT_LOG_STEPS, MAX_SCRIPT_LOG_STEPS, ScriptAction, ScriptBlockInfo, ScriptBlockRef,
    ScriptConditionInfo, ScriptContainerInfo, ScriptEventEntry, ScriptEventsResult, ScriptLineInfo,
    ScriptLogEntry, ScriptQuestInfo, ScriptSpeechInfo, ScriptStateResult, ScriptTipInfo,
    ScriptUiInfo,
};
use nml_control::{
    KI_STACK_TOP, KiAction, KiCommandCount, KiEngineInfo, KiRejectCount, KiStateResult,
    KiThreadInfo,
};
use nml_control::{KiDirectInfo, KiQueryResult, MAX_KI_QUERY_WORDS};
use nml_control::{
    SimActionResult, SimAnimalInfo, SimAreaWatchInfo, SimAssassinInfo, SimAttackInfo,
    SimBanishInfo, SimBombermanInfo, SimBuildResult, SimBuildingInfo, SimCombatInfo,
    SimConstructInfo, SimDoorTile, SimDyingInfo, SimEdibleInfo, SimEvokeAnimalInfo,
    SimEvokeGhostInfo, SimGateInfo, SimGathererInfo, SimGhostInfo, SimHeadhunterInfo,
    SimHealingInfo, SimHireEntry, SimHireMenuResult, SimInfluenceInfo, SimKillerInfo,
    SimMaskingInfo, SimMenuEntry, SimMenuResult, SimMessageFilterInfo, SimProductionInfo,
    SimProjectileInfo, SimQuarterInfo, SimQueueEntry, SimRangedInfo, SimResourceInfo,
    SimRobbingInfo, SimShootInfo, SimSpecialMoveInfo, SimSpellInfo, SimTreeInfo,
};
use nml_formats::config::lower_german;
use nml_formats::gifcon::button_kind;
use nml_sim::combat::projectile::ProjectileKind;
use nml_sim::command::headhunter::{hire_players, hire_price_in, hire_targets};
use nml_sim::command::validate::{check_build, check_production, is_dead, own_building};
use nml_sim::command::{Command, CommandKind};
use nml_sim::fx::Fx;
use nml_sim::game_end::{DEFEAT, VICTORY};
use nml_sim::id::ObjectId;
use nml_sim::id::PLAYER_SLOTS;
use nml_sim::ki::KiPause;
use nml_sim::ki::query::{Direct, query_once};
use nml_sim::ki::run::{MAX_START_ARGS, MAX_STEP_PASSES};
use nml_sim::ki::vm::Thread;
use nml_sim::message::MessageFilter;
use nml_sim::module::building::MAX_QUEUE;
use nml_sim::module::ghosts::{banish, evoke, ghost};
use nml_sim::module::special::{bomberman, killer, special_move};
use nml_sim::module::{
    Module, ModuleKind, animal, arming, assassin, attack, building, construct, dying, edible,
    evoke_animal, exec_temp, fight_far, fight_near, gather, headhunter, healing, influence,
    masking, quarter, resource, robbing, shoot, spell, tree,
};
use nml_sim::object::{GameObject, status};
use nml_sim::path::grid::COST_FREE;
use nml_sim::placement::{ROTATION_LOCK, rotation_locked};
use nml_sim::player::Sight;
use nml_sim::script::Container;
use nml_world::terrain::Terrain;

use crate::game::economy::{MenuEntry, QueueEntry, action_name, affordable};
use crate::game::formation_order::FormationOrder;
use crate::game::sim_host::SimHost;

/// Parameters of `ki_command` (protocol 26) as in the control channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiCommandRequest {
    pub action: KiAction,
    pub slot: Option<u8>,
    pub module: Option<u32>,
    pub args: Option<Vec<i32>>,
    pub sync: Option<bool>,
    /// `pause`: default `true`.
    pub paused: Option<bool>,
    /// `step`: default 1.
    pub passes: Option<u32>,
}

/// ID for the protocol: `None` for "none" (`NONE` and the start ID 0 of the modules).
fn handle(id: ObjectId) -> Option<u32> {
    (id != ObjectId::NONE && id != ObjectId(0)).then_some(id.0)
}

/// Name of the command variant (`ChopWood`, `ConstructAt`, ...) from the JSON form.
pub fn command_name(kind: &CommandKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::Object(map)) => map.keys().next().cloned().unwrap_or_default(),
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

/// Name of the button kind of a list (`toolbar.ini` flag without `_MSG`).
pub fn button_kind_name(kind: u32) -> String {
    match kind {
        button_kind::BUILD => "BUILD".into(),
        button_kind::ONLYONCE => "ONLYONCE".into(),
        button_kind::MAKE => "MAKE".into(),
        button_kind::MAKEONLY_ONCE => "MAKEONLY_ONCE".into(),
        button_kind::UPGRADE => "UPGRADE".into(),
        button_kind::COUNTERUPGRADE => "COUNTERUPGRADE".into(),
        other => format!("{other:#x}"),
    }
}

impl From<MenuEntry> for SimMenuEntry {
    fn from(e: MenuEntry) -> Self {
        Self {
            guid: e.guid,
            name: e.name,
            kind: button_kind_name(e.kind),
            costs: e.costs,
            affordable: e.affordable,
            blocked: e.blocked,
        }
    }
}

impl From<QueueEntry> for SimQueueEntry {
    fn from(e: QueueEntry) -> Self {
        Self {
            index: e.index,
            guid: e.guid,
            name: e.name,
            progress: e.progress,
        }
    }
}

/// Error message of the simulation commands without a match (`nml-game` in the menu).
pub const NO_MATCH: &str = "no match";

/// `sim_hash`, `sim_query`, `sim_command` and `sim_result` on the simulation `host` (`None` = no match ->
/// [`NO_MATCH`]), executed as in the viewer: `sim_command` queues the command for the next tick
/// ([`SimHost::control_submit`]), likewise `sim_result` the command `0x56` ([`SimHost::control_result`]); the two
/// others only read. Other commands are an error.
pub fn execute_sim(
    host: Option<&mut SimHost>,
    command: &ControlCommand,
) -> Result<serde_json::Value, String> {
    // A broken command is reported before accessing the simulation (as in the viewer).
    let parsed = match command {
        ControlCommand::SimCommand { command } => Some(parse_sim_command(command.clone())?),
        ControlCommand::SimResult { .. } => {
            command.validate()?;
            None
        }
        _ => None,
    };
    let host = host.ok_or(NO_MATCH)?;
    let value = |result: Result<serde_json::Value, serde_json::Error>| {
        result.map_err(|e| format!("response not serializable: {e}"))
    };
    match (command, parsed) {
        (ControlCommand::SimHash, _) => value(serde_json::to_value(host.hash_result())),
        (
            ControlCommand::SimQuery {
                player,
                guid,
                ids,
                limit,
            },
            _,
        ) => value(serde_json::to_value(host.query(
            *player,
            *guid,
            ids.as_deref(),
            limit.unwrap_or(100),
        ))),
        (ControlCommand::SimCommand { .. }, Some(parsed)) => Ok(host.control_submit(parsed)),
        (ControlCommand::SimResult { player, result }, _) => {
            Ok(host.control_result(*player, *result))
        }
        _ => Err(
            "not a simulation command (expected: sim_hash, sim_query, sim_command, sim_result)"
                .into(),
        ),
    }
}

/// `sim_command`, first step: read the command from the JSON form of [`Command`] ("command invalid: ...") and check
/// the player (0 to 7). Runs before accessing the simulation so that a broken command always gets the same
/// message, even without a simulation (viewer without a map, `nml-game` without a match).
pub fn parse_sim_command(value: serde_json::Value) -> Result<Command, String> {
    let command: Command =
        serde_json::from_value(value).map_err(|e| format!("command invalid: {e}"))?;
    if command.player >= 8 {
        return Err(format!(
            "player = {} invalid: allowed 0 to 7",
            command.player
        ));
    }
    Ok(command)
}

impl SimHost {
    /// `sim_command`, second step: queue the checked command ([`parse_sim_command`]) for the next tick.
    /// Response `{"step": <engine ticks so far>}`.
    pub fn control_submit(&mut self, command: Command) -> serde_json::Value {
        self.submit(command);
        serde_json::json!({ "step": self.state.steps })
    }

    /// `sim_result`: command `0x56` of the active player - slot `slot` loses (1) or wins (2) - queued for the next
    /// tick, like the cheat of the original via `GameEngine_SetPlayerResult` (`0x00430CE0`), but in the command log
    /// (replay, savegame). Slot and result are checked by `Command::validate` beforehand, and once more by the
    /// simulation (any slot, not only the own one). Response `{"step"}` like [`Self::control_submit`].
    pub fn control_result(&mut self, slot: u8, result: i32) -> serde_json::Value {
        let player = self.active_player;
        self.control_submit(Command {
            player,
            kind: CommandKind::SetPlayerResult { slot, result },
        })
    }

    /// World point (x, z) in fixed-point tiles of the grid.
    fn world_xz(&self, x: i32, z: i32) -> [f32; 2] {
        let cell = self.rules.map.cell;
        [
            Fx(x).to_f32() * cell[0] as f32,
            Fx(z).to_f32() * cell[2] as f32,
        ]
    }

    /// `sim_build`: like a left click in placement mode ([`SimHost::placement_preview`] and
    /// [`SimHost::place_building`]) with the own objects from `units` as workers;
    /// `append` = Shift. The position snaps like the build cursor
    /// ([`nml_sim::placement::cursor_placement`]); the command carries the snapped position that the
    /// command check demands. Errors: no own workers, blocked (reason of the
    /// command check [`check_build`]), rotation of a type with a rotation lock ([`rotation_locked`]),
    /// without a model hull, invalid site. A balance that is too low
    /// is not an error: the simulation then rejects with a message like the original
    /// (`affordable` in the result). The height is that of the mouse point when building ([`ground_below`]):
    /// terrain at the unsnapped target point, at least the water height - the rally point of the construction site
    /// thus lies at ground height plus node `ausgang`, as in the original (re/world-display W.1).
    pub fn control_build(
        &mut self,
        guid: u32,
        to: [f32; 2],
        steps: u8,
        units: &[ObjectId],
        append: bool,
    ) -> Result<SimBuildResult, String> {
        let player = self.active_player;
        let workers = self.own_objects(units);
        if workers.is_empty() {
            return Err(format!("no own workers (player {player})"));
        }
        check_build(&self.sim_context(), player, guid, &workers)
            .map_err(|why| format!("building blocked: {why}"))?;
        if steps & 3 != 0 && rotation_locked(&self.rules, guid) {
            return Err(format!(
                "type {guid} cannot be rotated (rotation lock of the build cursor, building flags & \
                 {ROTATION_LOCK:#x}): rotation_steps must be 0"
            ));
        }
        let ground = Vec3::new(
            to[0],
            ground_below(&self.rules.terrain, to[0], to[1]),
            to[1],
        );
        let preview = self
            .placement_preview(guid, ground, steps, &workers)
            .ok_or_else(|| {
                format!("type {guid} has no model hull (no building site can be checked)")
            })?;
        if !preview.valid {
            let tiles = preview
                .rect
                .map_or_else(|| "outside the map".to_string(), |r| format!("{r:?}"));
            return Err(format!(
                "invalid site for {guid} at ({:.1}, {:.1}), tiles {tiles}",
                preview.pos[0], preview.pos[2]
            ));
        }
        let affordable = affordable(&self.sim_context(), player, guid);
        self.place_building(&preview, &workers, append.then_some(false));
        Ok(SimBuildResult {
            step: self.state.steps,
            guid,
            pos: preview.pos,
            rotation: preview.rot,
            rect: preview.rect,
            workers: workers.iter().map(|w| w.0).collect(),
            affordable,
            append,
        })
    }

    /// `sim_train`: training or trade `guid` in the own building (`0x0E`) after the check
    /// [`check_production`] (own, finished, not blocked on the map, in the menu, not blocked;
    /// research like training). If the GUID is in the production list, its block reason applies,
    /// otherwise that of the command check. A full queue (one
    /// running and [`MAX_QUEUE`] waiting orders, counting the orders of this
    /// building that have not been executed yet) is an error - the simulation would silently discard the order.
    pub fn control_train(
        &mut self,
        building_id: ObjectId,
        guid: u32,
    ) -> Result<serde_json::Value, String> {
        let player = self.active_player;
        let listed = self
            .production_menu(building_id)
            .into_iter()
            .find(|e| e.guid == guid)
            .and_then(|e| e.blocked);
        let sim = self.sim_context();
        listed
            .map_or_else(|| check_production(&sim, player, building_id, guid), Err)
            .map_err(|why| format!("training blocked: {why}"))?;
        let affordable = affordable(&sim, player, guid);
        let (running, waiting) = match sim
            .state
            .object(building_id, false)
            .and_then(|o| o.module(building::INDEX))
            .map(|m| &m.kind)
        {
            Some(ModuleKind::Building(b)) => (usize::from(b.make_guid != 0), b.make_queue.len()),
            _ => (0, 0),
        };
        let queued = self
            .pending_commands()
            .iter()
            .filter(|c| {
                c.player == player
                    && matches!(c.kind, CommandKind::EnqueueProduction { building, .. }
                        if building == building_id)
            })
            .count();
        if running + waiting + queued > MAX_QUEUE {
            return Err(format!("queue full ({MAX_QUEUE} orders waiting)"));
        }
        self.submit_own(CommandKind::EnqueueProduction {
            building: building_id,
            guid,
        });
        Ok(serde_json::json!({ "step": self.state.steps, "affordable": affordable }))
    }

    /// `sim_cancel`: cancel order `index` (0 running, 1 ... waiting) of an own building
    /// (`0x0F`); the index must be in the queue.
    pub fn control_cancel(
        &mut self,
        building_id: ObjectId,
        index: i32,
    ) -> Result<serde_json::Value, String> {
        let guid = self.queue_entry(building_id, index)?;
        self.submit_own(CommandKind::CancelProduction {
            building: building_id,
            index,
            arg: 0,
        });
        Ok(serde_json::json!({ "step": self.state.steps, "guid": guid }))
    }

    /// `sim_complete`: debug "complete immediately" for order `index` (0 running, 1 ... waiting)
    /// of an own building (`DebugCompleteOrder`); the index must be in the queue.
    /// `expected` is the expected type of the entry: if it already differs now, error; if it is absent,
    /// the current entry applies. The command carries the type along, the simulation rejects if a different one
    /// is at that index on execution (the queue has moved up). Whether the
    /// start check or the payment of the trade succeeds is not decided until the execution at the
    /// next tick (message as in the game).
    pub fn control_complete(
        &mut self,
        building_id: ObjectId,
        index: i32,
        expected: Option<u32>,
    ) -> Result<serde_json::Value, String> {
        let guid = self.queue_entry(building_id, index)?;
        if let Some(e) = expected.filter(|&e| e != guid) {
            return Err(format!(
                "order {index} in {} is {guid}, not {e}",
                building_id.0
            ));
        }
        self.submit_own(CommandKind::DebugCompleteOrder {
            building: building_id,
            index,
            guid,
        });
        Ok(serde_json::json!({ "step": self.state.steps, "guid": guid }))
    }

    /// GUID of the order `index` of an own building of the active player (check of
    /// `sim_cancel` and `sim_complete`).
    fn queue_entry(&mut self, building_id: ObjectId, index: i32) -> Result<u32, String> {
        let player = self.active_player;
        if !own_building(&self.sim_context(), player, building_id) {
            return Err(format!(
                "{} is not a building of player {player}",
                building_id.0
            ));
        }
        let queue = self.production_queue(building_id);
        let entry = queue.iter().find(|q| q.index == index).ok_or_else(|| {
            format!(
                "no order with index {index} (queue length: {})",
                queue.len()
            )
        })?;
        Ok(entry.guid)
    }

    /// `sim_sell`: sell the own living buildings from `buildings` (`0x67`); the rest
    /// are listed under `skipped`. None left: error.
    pub fn control_sell(&mut self, buildings: &[ObjectId]) -> Result<serde_json::Value, String> {
        let player = self.active_player;
        let sim = self.sim_context();
        let (sold, skipped): (Vec<ObjectId>, Vec<ObjectId>) = buildings
            .iter()
            .partition(|&&b| own_building(&sim, player, b) && !is_dead(&sim, b));
        if sold.is_empty() {
            return Err(format!("no own living building of player {player} to sell"));
        }
        self.submit_own(CommandKind::SellBuildings {
            buildings: sold.clone(),
        });
        Ok(serde_json::json!({
            "step": self.state.steps,
            "buildings": sold.iter().map(|b| b.0).collect::<Vec<_>>(),
            "skipped": skipped.iter().map(|b| b.0).collect::<Vec<_>>(),
        }))
    }

    /// `sim_upgrade`: apply upgrade `guid` to the slot `slot` (`0x39`; `guid` < 0 = all GUIDs
    /// 2800 to 3399) or remove it (`0x3A`, only `guid` > 0). The sender is the active player, `slot`
    /// only the target; as in the original without an ownership check (console, re/upgrades 2), also for foreign
    /// slots and slot 8 (W-213). The effect begins at the next tick.
    pub fn control_upgrade(
        &mut self,
        slot: u8,
        guid: i32,
        remove: bool,
    ) -> Result<serde_json::Value, String> {
        let slot = i32::from(slot);
        let kind = if remove {
            let guid = u32::try_from(guid)
                .ok()
                .filter(|&g| g > 0)
                .ok_or_else(|| format!("guid = {guid} invalid: removing needs a GUID > 0"))?;
            CommandKind::RemoveUpgrade { slot, guid }
        } else {
            CommandKind::ApplyUpgrade { slot, guid }
        };
        self.submit_own(kind);
        Ok(serde_json::json!({ "step": self.state.steps }))
    }

    /// `sim_trade`: trade with player (`0x45`, re/trading 2.2): the carts `units` drive with trade kind
    /// `ware` (0 to 3) to the partner slot `target` (0 to 8). The sender is `player`, not the active
    /// player, so that the rejections of the simulation (kind, target, trade building) and the ignoring of
    /// foreign carts remain checkable; only the simulation checks the values. An error
    /// without units.
    pub fn control_trade(
        &mut self,
        player: u8,
        units: &[ObjectId],
        ware: i32,
        target: i32,
    ) -> Result<serde_json::Value, String> {
        if units.is_empty() {
            return Err("no units (selection empty)".into());
        }
        self.submit(Command {
            player,
            kind: CommandKind::TradeWithPlayer {
                ware,
                target,
                units: units.to_vec(),
            },
        });
        Ok(serde_json::json!({ "step": self.state.steps }))
    }

    /// `sim_tribute`: tribute (`0x65`, re/heroes-resources 2.4): the slot `player` pays each slot
    /// 0 to 7 with a bit in `recipients` `amount` of the resource `resource`. Sender and giver are
    /// `player`; resource, amount and stock are checked by the simulation.
    pub fn control_tribute(
        &mut self,
        player: u8,
        recipients: u32,
        resource: i32,
        amount: i32,
    ) -> Result<serde_json::Value, String> {
        self.submit(Command {
            player,
            kind: CommandKind::Tribute {
                from: i32::from(player),
                recipients,
                resource,
                amount,
            },
        });
        Ok(serde_json::json!({ "step": self.state.steps }))
    }

    /// `sim_action`: right-click of the units `units` on `target` or the world point `to`
    /// (without `to` the position of the target) via [`SimHost::issue_target_action`]; `queue` like
    /// [`crate::game::economy::queue_keys`]. Errors: no units, the first object not own,
    /// target missing or no action.
    #[cfg(test)]
    pub fn control_action(
        &mut self,
        units: &[ObjectId],
        target: Option<ObjectId>,
        to: Option<[f32; 2]>,
        queue: Option<bool>,
    ) -> Result<SimActionResult, String> {
        self.control_action_issued(units, target, to, queue, FormationOrder::default())
            .map(|(result, _)| result)
    }

    /// Like [`SimHost::control_action`], plus `BUTTONMSG` and the issued command (for the
    /// target marker of the viewer, [`crate::game::markers::on_target_action`]). `order` carries the selected
    /// formation of the bar into a walk command.
    pub fn control_action_issued(
        &mut self,
        units: &[ObjectId],
        target: Option<ObjectId>,
        to: Option<[f32; 2]>,
        queue: Option<bool>,
        order: FormationOrder,
    ) -> Result<(SimActionResult, (i32, CommandKind)), String> {
        let player = self.active_player;
        let Some(&first) = units.first() else {
            return Err("no units (selection empty)".into());
        };
        if self.own_objects(&[first]).is_empty() {
            return Err(format!(
                "no action: {} does not belong to player {player}",
                first.0
            ));
        }
        let ground = match (to, target) {
            (Some([x, z]), _) => Vec3::new(x, 0.0, z),
            (None, Some(t)) => {
                let o = self
                    .state
                    .object(t, false)
                    .ok_or_else(|| format!("target {} missing", t.0))?;
                Vec3::from(o.world_pos)
            }
            (None, None) => return Err("sim_action needs target or to".into()),
        };
        if let Some(t) = target
            && self.state.object(t, false).is_none()
        {
            return Err(format!("target {} missing", t.0));
        }
        let map = self.rules.map;
        let outside = !(0.0..=(map.dim_x * map.cell[0]) as f32).contains(&ground.x)
            || !(0.0..=(map.dim_z * map.cell[2]) as f32).contains(&ground.z);
        let (button, kind) = self
            .issue_target_action_keys(
                units,
                target.map(crate::game::selection::RightClickTarget::Object),
                ground,
                queue,
                queue == Some(true),
                false,
                order,
            )
            .ok_or_else(|| match target {
                Some(t) => format!("no action for {} (targets.ini)", t.0),
                None if outside => "no action for the ground outside the map".to_string(),
                None => "no action for the ground (targets.ini)".to_string(),
            })?;
        let (inner, queued, cyclic) = match &kind {
            CommandKind::Queue { cyclic, inner } => (inner.as_ref(), true, *cyclic),
            other => (other, false, false),
        };
        let result = SimActionResult {
            step: self.state.steps,
            button,
            action: action_name(button).into(),
            command: command_name(inner),
            code: inner.type_code() & 0xFF,
            queued,
            cyclic,
            kind: serde_json::to_value(&kind).unwrap_or_default(),
        };
        Ok((result, (button, kind)))
    }

    /// `sim_menu`: build list of the own workers from `units` ([`SimHost::build_menu`]) or
    /// production list and queue of an own building
    /// ([`SimHost::production_menu`], [`SimHost::production_queue`]).
    pub fn control_menu(
        &mut self,
        units: Option<&[ObjectId]>,
        building_id: Option<ObjectId>,
    ) -> Result<SimMenuResult, String> {
        let player = self.active_player;
        if let Some(b) = building_id {
            if !own_building(&self.sim_context(), player, b) {
                return Err(format!("{} is not a building of player {player}", b.0));
            }
            return Ok(SimMenuResult {
                building: Some(b.0),
                workers: Vec::new(),
                entries: self
                    .production_menu(b)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                queue: self
                    .production_queue(b)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            });
        }
        let units = units.unwrap_or_default();
        let workers = self.own_objects(units);
        if workers.is_empty() {
            return Err(format!("no own workers (player {player})"));
        }
        Ok(SimMenuResult {
            building: None,
            workers: workers.iter().map(|w| w.0).collect(),
            entries: self
                .build_menu(&workers)
                .into_iter()
                .map(Into::into)
                .collect(),
            queue: Vec::new(),
        })
    }

    /// Building module for `sim_query` (protocol 9).
    pub(crate) fn building_info(&self, o: &GameObject) -> Option<SimBuildingInfo> {
        let m = o.module(building::INDEX)?;
        let ModuleKind::Building(b) = &m.kind else {
            return None;
        };
        let mp = b.meeting_point;
        Some(SimBuildingInfo {
            finished: m.base.has_flags(building::flags::CONSTRUCTED_FINISHED),
            under_construction: b.is_build_building,
            build_rest: b.build_counter,
            build_time: b.build_time,
            progress: o.progress_scale,
            render_bits: o.render_bits,
            flags: m.base.flags,
            production: (b.make_guid != 0).then_some(SimProductionInfo {
                make_guid: b.make_guid,
                rest: b.make_time,
                total: b.make_start,
            }),
            queue: b.make_queue.iter().copied().collect(),
            docks: [
                b.parking_lots
                    .iter()
                    .filter(|l| l.hid != ObjectId::NONE)
                    .count(),
                b.parking_lots.len(),
            ],
            meeting_point_fx: [mp.x.0, mp.z.0],
            meeting_point: self.world_xz(mp.x.0, mp.z.0),
            master: handle(b.master),
            slaves: b.slaves.iter().map(|s| s.0).collect(),
            sleeping: b.is_sleeping,
            dead: b.is_dead,
            gate: self.gate_info(m, b),
            area_watch: self.area_watch_info(b),
            watch_rect: (b.check_area_rect != [0; 4]).then_some(b.check_area_rect),
        })
    }

    /// Gate state for `sim_query` (protocol 18, re/arming-gates 3.C): only with `TOR` or a
    /// gate bit. The door tiles come from rectangle and mask ([`gate::door_tiles_of`], without `Sim`),
    /// "blocked" means layer 0 of the path grid blocked.
    fn gate_info(&self, m: &Module, b: &building::Building) -> Option<SimGateInfo> {
        use building::flags::{TOR, TORCLOSE, TOROPEN};
        if !m.base.has_flags(TOR | TOROPEN | TORCLOSE) {
            return None;
        }
        let path = &self.state.path;
        Some(SimGateInfo {
            open: m.base.has_flags(TOROPEN),
            closed: m.base.has_flags(TORCLOSE),
            doors: building::gate::door_tiles_of(b)
                .into_iter()
                .map(|(x, z)| SimDoorTile {
                    x,
                    z,
                    blocked: path.cell_cost(x, z, 0) != COST_FREE,
                })
                .collect(),
        })
    }

    /// Area watch for `sim_query` (protocol 18): only with a running query with a valid
    /// key.
    fn area_watch_info(&self, b: &building::Building) -> Option<SimAreaWatchInfo> {
        if b.check_area_id == -1 {
            return None;
        }
        let q = self.state.area_queries.get(b.check_area_id)?;
        Some(SimAreaWatchInfo {
            key: b.check_area_id,
            mode: q.mode,
            rect: q.rect,
            player: q.player,
            members: q.list.iter().map(|id| id.0).collect(),
        })
    }

    /// Gatherer module (7, 8, 12, 55) and carried load for `sim_query` (protocol 9). Workers
    /// with several gatherer modules (pioneer: gold, wood, harvest) show the active one: first one
    /// that is not free, then one with a source, then that of the carried load kind, otherwise the
    /// first in module order.
    pub(crate) fn gatherer_info(&self, o: &GameObject) -> Option<SimGathererInfo> {
        let present: Vec<(&gather::GatherKind, &Module, &gather::Gather)> = gather::KINDS
            .iter()
            .filter_map(|k| {
                let m = o.module(k.index)?;
                Some((k, m, m.kind.gather()?))
            })
            .collect();
        let (k, m, g) = *present
            .iter()
            .find(|(_, _, g)| g.status != gather::status::IDLE)
            .or_else(|| {
                present
                    .iter()
                    .find(|(_, _, g)| handle(g.resource_hid).is_some())
            })
            .or_else(|| {
                present
                    .iter()
                    .find(|(k, _, _)| o.load_amount > 0 && k.resource == o.load_kind)
            })
            .or_else(|| present.first())?;
        let kind = match m.kind {
            ModuleKind::Mining(_) => "mining",
            ModuleKind::Lumber(_) => "lumber",
            ModuleKind::Harvest(_) => "harvest",
            ModuleKind::Fishing(_) => "fishing",
            ModuleKind::Hunting(_) => "hunting",
            _ => "?",
        };
        Some(SimGathererInfo {
            module: k.index as u8,
            kind: kind.into(),
            state: g.status,
            load_amount: o.load_amount,
            load_kind: o.load_kind,
            // The hunt carries its target in `m_TargetHID` instead of `m_ResourceHID`: output as
            // source and as `prey`.
            source: match &m.kind {
                ModuleKind::Hunting(h) => handle(h.target_hid),
                _ => handle(g.resource_hid),
            },
            store: handle(g.hq_hid),
            prey: match &m.kind {
                ModuleKind::Hunting(h) => handle(h.target_hid),
                _ => None,
            },
        })
    }

    /// Construction worker module for `sim_query` (protocol 9).
    pub(crate) fn construct_info(&self, o: &GameObject) -> Option<SimConstructInfo> {
        let ModuleKind::Construct(c) = &o.module(construct::INDEX)?.kind else {
            return None;
        };
        Some(SimConstructInfo {
            state: c.status,
            building: handle(c.building),
        })
    }

    /// Quarters module for `sim_query` (protocol 9).
    pub(crate) fn quarter_info(&self, o: &GameObject) -> Option<SimQuarterInfo> {
        let ModuleKind::Quarter(q) = &o.module(quarter::INDEX)?.kind else {
            return None;
        };
        Some(SimQuarterInfo {
            occupied: q.cur,
            max: q.max,
            occupants: q.occupants.iter().map(|u| u.0).collect(),
            meeting_point: q.meeting_point.map(|(x, z)| [x, z]),
            waiting: q.waiting.iter().map(|u| u.0).collect(),
            status: q.status,
        })
    }

    /// Upgrade percentages of the owner for the type of the object
    /// (`GameWorld_GetTypeUpgradePercent`, like `Sim::type_upgrade_percent`). Read-only.
    fn upgrade_percent(&self, o: &GameObject, attribute: u32) -> i32 {
        nml_sim::upgrades::type_upgrade_percent(&self.state, o.owner(), o.guid, attribute)
    }

    /// Spell module 38 for `sim_query` (protocol 16, re/temp-upgrades 15): the fields of the module,
    /// plus extra energy (attribute 15) with the upgrade percentages of the owner and `casting` =
    /// status `ANIM_LOCKED`. Protocol 21 (re/spells 1.2): `target` = target point `m_Position` of the
    /// kinds 1, 2 and 5 while `casting`; `corpse` = corpse of the reincarnation (empty is absent, stale
    /// is retained).
    pub(crate) fn spell_info(&self, o: &GameObject) -> Option<SimSpellInfo> {
        let ModuleKind::Spell(s) = &o.module(spell::INDEX)?.kind else {
            return None;
        };
        let p = self.upgrade_percent(o, spell::MANA);
        let casting = o.status & status::ANIM_LOCKED != 0;
        Some(SimSpellInfo {
            art: s.art,
            button: s.button,
            cost: s.cost,
            anim: s.anim,
            idx: s.idx,
            reserve: s.reserve,
            mana: o.attributes.current_value(spell::MANA, p),
            mana_max: o.attributes.max_value(spell::MANA, p),
            casting,
            target: (casting && matches!(s.art, 1 | 2 | 5)).then_some(s.target),
            corpse: handle(s.corpse),
        })
    }

    /// Demolition expert module 36 for `sim_query` (protocol 21, re/special-attacks 1.2, 5): the fields
    /// of the module, plus the extra energy (attribute 15) as in [`SimHost::healing_info`]. An empty
    /// target is absent, a stale one is retained.
    pub(crate) fn bomberman_info(&self, o: &GameObject) -> Option<SimBombermanInfo> {
        let ModuleKind::Bomberman(b) = &o.module(bomberman::INDEX)?.kind else {
            return None;
        };
        Some(SimBombermanInfo {
            target: handle(b.target),
            extra_energy: b.extra_energy,
            return_point: b.return_point,
            busy: b.busy,
            mana: self.extra_energy(o),
        })
    }

    /// Killer module 37 for `sim_query` (protocol 21, re/special-attacks 1.3, 5); target as in
    /// [`SimHost::bomberman_info`].
    pub(crate) fn killer_info(&self, o: &GameObject) -> Option<SimKillerInfo> {
        let ModuleKind::Killer(k) = &o.module(killer::INDEX)?.kind else {
            return None;
        };
        Some(SimKillerInfo {
            target: handle(k.target),
            extra_energy: k.extra_energy,
            near_damage: k.near_damage,
            far_damage: k.far_damage,
            mana: self.extra_energy(o),
        })
    }

    /// Smith module 49 for `sim_query` (protocol 21, re/special-attacks 1.4, 5).
    pub(crate) fn special_move_info(&self, o: &GameObject) -> Option<SimSpecialMoveInfo> {
        let ModuleKind::SpecialMove(m) = &o.module(special_move::INDEX)?.kind else {
            return None;
        };
        Some(SimSpecialMoveInfo {
            busy: m.busy,
            mode: m.mode,
            mana: self.extra_energy(o),
        })
    }

    /// Headhunter module 34 for `sim_query` (protocol 22, re/headhunter-robbing 1.2, 5): the
    /// fields of the module; `art` from the type GUID ([`headhunter::art_of`], 255 for a foreign GUID).
    /// Empty IDs are absent, stale ones are retained.
    pub(crate) fn headhunter_info(o: &GameObject) -> Option<SimHeadhunterInfo> {
        let ModuleKind::HeadHunter(h) = &o.module(headhunter::INDEX)?.kind else {
            return None;
        };
        Some(SimHeadhunterInfo {
            art: headhunter::art_of(o.guid).map_or(u8::MAX, |a| a as u8),
            state: h.state,
            target_guid: h.target_guid,
            target_player: h.target_player,
            target: handle(h.target_hid),
            last_target: handle(h.last_target),
            price: h.cur_reward,
            customer: h.customer,
            customer_cost: h.customer_cost,
        })
    }

    /// Robber module 48 for `sim_query` (protocol 22, re/headhunter-robbing 1.3, 3.9): `anim` =
    /// `m_RobAnim[0]` (-1 empty), `robbing` = lock `ANIM_LOCKED` with created timer `0x330`,
    /// `attack_enabled` = attack module `0x11` switched on, plus the extra energy.
    pub(crate) fn robbing_info(&self, o: &GameObject) -> Option<SimRobbingInfo> {
        let module = o.module(robbing::INDEX)?;
        let ModuleKind::Robbing(r) = &module.kind else {
            return None;
        };
        Some(SimRobbingInfo {
            anim: r.rob_anim.first().copied().unwrap_or(-1),
            robbing: o.status & status::ANIM_LOCKED != 0
                && module.base.has_pending_timer(robbing::msg::ROB),
            attack_enabled: o.module(robbing::ATTACK).is_some_and(|m| m.base.enabled),
            mana: self.extra_energy(o),
        })
    }

    /// Headhunter table for `sim_query` (protocol 22, re/headhunter-robbing 1.4, 2.5): per kind 0-2 the
    /// entry of `GameWorld+0x4DC` (raw) and the price like the tooltip ([`hire_price_in`]).
    /// Read-only.
    pub(crate) fn headhunter_table(&self) -> Vec<SimHireEntry> {
        (0..headhunter::GUIDS.len())
            .map(|art| SimHireEntry {
                art: art as u8,
                hunter: self.state.headhunters[art].map(|id| id.0),
                price: hire_price_in(&self.state, &self.rules, art),
            })
            .collect()
    }

    /// `sim_hire_menu` (protocol 22, re/headhunter-robbing 2.5): the target players of the slot
    /// `player` ([`hire_players`], level 2) and with `target_player` its target GUIDs for the kind
    /// `art` ([`hire_targets`], level 3; even if it is not a target player, like `FUN_0044AA10`).
    /// The same lists as the menu in the "Economy" window; reads only. Error: `art` > 2.
    pub fn control_hire_menu(
        &mut self,
        player: u8,
        art: u8,
        target_player: Option<u8>,
    ) -> Result<SimHireMenuResult, String> {
        if usize::from(art) >= headhunter::GUIDS.len() {
            return Err(format!("art = {art} invalid: allowed 0 to 2"));
        }
        let sim = self.sim_context();
        Ok(SimHireMenuResult {
            players: hire_players(&sim, player),
            targets: target_player
                .map_or_else(Vec::new, |t| hire_targets(&sim, usize::from(art), t)),
        })
    }

    /// `ki_query` (protocol 23, re/ki-queries): the AI query `code` of the slot `player`
    /// against the running match ([`query_once`], request words `words`, default `[player]`;
    /// sight `sight` `"omniscient"` = default or `"fog"`). Reads only: the direct changes of the
    /// request (marking, taking an attack report) are reported and **not**
    /// executed. A marking whose object does not (any more) exist has no effect in the game
    /// ([`nml_sim::ki::query::apply`] ignores it) and is therefore absent in `direct`; this is checked by
    /// `state.object(id, false)` without visibility, dead objects count too (E-55).
    pub fn control_ki_query(
        &mut self,
        player: u8,
        code: u32,
        words: Option<&[i32]>,
        sight: Option<&str>,
    ) -> Result<KiQueryResult, String> {
        if usize::from(player) >= PLAYER_SLOTS {
            return Err(format!("player = {player} invalid: allowed 0 to 7"));
        }
        let sight = match sight {
            None | Some("omniscient") => Sight::Omniscient,
            Some("fog") => Sight::Fog,
            Some(other) => {
                return Err(format!(
                    "sight = \"{other}\" invalid: allowed \"omniscient\" or \"fog\""
                ));
            }
        };
        let default = [i32::from(player)];
        let words = words.unwrap_or(&default);
        if words.len() > MAX_KI_QUERY_WORDS {
            return Err(format!(
                "words has {} words: at most {MAX_KI_QUERY_WORDS} allowed",
                words.len()
            ));
        }
        let (request, direct) = query_once(&mut self.sim_context(), player, sight, code, words);
        let direct = direct
            .into_iter()
            .filter_map(|d| match d {
                Direct::Mark { object, player, on } => {
                    self.state.object(object, false)?;
                    Some(KiDirectInfo {
                        kind: if on { "mark" } else { "unmark" }.into(),
                        object: Some(object.0),
                        player,
                    })
                }
                Direct::TakeReport { player, victim } => Some(KiDirectInfo {
                    kind: "take_report".into(),
                    object: Some(victim as u32),
                    player,
                }),
            })
            .collect();
        Ok(KiQueryResult {
            code,
            request,
            direct,
        })
    }

    // --- Scripts (protocol 28) ------------------------------------------------------------------

    /// `script_state` (protocol 28): level and dialog containers with blocks (state,
    /// wait reason, action index), namespaces and countdown; speech state; the object names in the order
    /// of the name table, with `names` only those whose name or value contains the text (case handling
    /// like the names, `lower_german`); the block log of the last `log` steps (default
    /// [`nml_control::DEFAULT_SCRIPT_LOG_STEPS`], host state [`crate::sim_host::ScriptLog`]); with
    /// `block` the conditions of this block with value ([`nml_sim::script::condition_values`], on a
    /// copy). Reads only: state and checksum stay the same.
    pub fn control_script_state(
        &self,
        block: Option<ScriptBlockRef>,
        names: Option<&str>,
        log: Option<u32>,
    ) -> Result<ScriptStateResult, String> {
        let log = log.unwrap_or(DEFAULT_SCRIPT_LOG_STEPS);
        if log > MAX_SCRIPT_LOG_STEPS {
            return Err(format!(
                "log = {log} invalid: allowed 0 to {MAX_SCRIPT_LOG_STEPS} steps"
            ));
        }
        let script = &self.state.script;
        let containers: Vec<&Container> = script.level.iter().chain(&script.dialog).collect();
        let conditions = match block {
            None => None,
            Some(r) => Some(self.script_conditions(&containers, r)?),
        };
        let speech = &script.mission.speech;
        let quests = |list: &[nml_sim::script::Quest]| {
            list.iter()
                .map(|q| ScriptQuestInfo {
                    status: q.status,
                    text: q.text,
                })
                .collect()
        };
        let filter = names.map(|n| lower_german(n.trim()));
        let names = self
            .state
            .names
            .iter()
            .filter(|(k, v)| {
                filter
                    .as_deref()
                    .is_none_or(|f| k.contains(f) || v.contains(f))
            })
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let log = self
            .script_log
            .last(self.state.steps, u64::from(log))
            .map(|(step, trace)| ScriptLogEntry {
                step: *step,
                trace: serde_json::to_value(trace).unwrap_or(serde_json::Value::Null),
            })
            .collect();
        Ok(ScriptStateResult {
            enabled: self.setup.script.enabled,
            dialog_running: nml_sim::script::dialog_running(&self.state),
            containers: containers
                .iter()
                .map(|c| self.script_container_info(c))
                .collect(),
            conditions,
            speech: ScriptSpeechInfo {
                line: speech.line.map(|l| ScriptLineInfo {
                    text: l.text,
                    remaining_ms: l.remaining_ms,
                    slot: l.slot,
                }),
                tips: (0..2)
                    .map(|i| ScriptTipInfo {
                        remaining_ms: speech.tip_ms[i],
                        text: speech.tip_text[i],
                    })
                    .collect(),
                events: speech.events.clone(),
                info_history: script.mission.info_history.clone(),
            },
            names,
            log,
            main_quests: quests(&script.mission.main_quests),
            sub_quests: quests(&script.mission.sub_quests),
            countdown_container: script.mission.countdown_container,
            countdown_s: script.mission.countdown_s,
            task_status: script
                .mission
                .task_status
                .iter()
                .map(|(&task, &status)| (task, status))
                .collect(),
            area_overrides: script
                .area_overrides
                .iter()
                .map(|(name, &rect)| (name.clone(), rect))
                .collect(),
            commands: self.script_commands.info(),
            results: self
                .state
                .players
                .iter()
                .zip(0u8..)
                .filter(|(p, _)| p.result == DEFEAT || p.result == VICTORY)
                .map(|(p, slot)| (slot, p.result))
                .collect(),
            ui: self.script_ui_info(),
        })
    }

    /// State of the controls from `MissionState` (protocol 33): lock, cinemascope, selection mirror, last button.
    fn script_ui_info(&self) -> ScriptUiInfo {
        let mission = &self.state.script.mission;
        ScriptUiInfo {
            ui_locked: mission.ui_locked,
            cinemascope: mission.cinemascope,
            selection: mission.selection.iter().map(|id| id.0).collect(),
            last_button: mission.last_button,
        }
    }

    /// `script_events` (protocol 33): the presentation events of the last `steps` steps (default
    /// [`DEFAULT_SCRIPT_LOG_STEPS`], host state [`crate::game::sim_host::PresentLog`]) in JSON form of
    /// [`nml_sim::script::Presentation`], the presentation state (`MissionState::presentation`), the state of the
    /// controls, the profile variables (ascending by name). The camera of the scripts belongs to the session, not to
    /// the `SimHost`: `camera` stays `null`, the viewer adds it. Reads only: state and checksum stay the same.
    pub fn control_script_events(&self, steps: Option<u32>) -> Result<ScriptEventsResult, String> {
        let steps = steps.unwrap_or(DEFAULT_SCRIPT_LOG_STEPS);
        if steps > MAX_SCRIPT_LOG_STEPS {
            return Err(format!(
                "steps = {steps} invalid: allowed 0 to {MAX_SCRIPT_LOG_STEPS} steps"
            ));
        }
        let mission = &self.state.script.mission;
        Ok(ScriptEventsResult {
            events: self
                .present_log
                .last(self.state.steps, u64::from(steps))
                .map(|(step, event)| ScriptEventEntry {
                    step: *step,
                    event: serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
                })
                .collect(),
            presentation: serde_json::to_value(&mission.presentation)
                .unwrap_or(serde_json::Value::Null),
            ui: self.script_ui_info(),
            profile_vars: mission
                .profile_vars
                .iter()
                .map(|(name, &value)| (name.clone(), value))
                .collect(),
            camera: serde_json::Value::Null,
        })
    }

    /// `script_command` (protocol 32): debug intervention in the script flow as a command of the simulation
    /// (at the next tick, in the command log, hence also in replays and savegames). `skip_dialog`
    /// queues `ScriptSkipDialog` of the active player if a dialog is running (the simulation checks again);
    /// otherwise an error message, nothing queued. Response `{"step"}` like [`Self::control_submit`].
    pub fn control_script_command(
        &mut self,
        action: ScriptAction,
    ) -> Result<serde_json::Value, String> {
        match action {
            ScriptAction::SkipDialog => {
                if !nml_sim::script::dialog_running(&self.state) {
                    return Err("script_command skip_dialog: no dialog running".into());
                }
                let player = self.active_player;
                Ok(self.control_submit(Command {
                    player,
                    kind: CommandKind::ScriptSkipDialog,
                }))
            }
        }
    }

    /// Conditions of block `r` with value (evaluated on a copy); error message if the
    /// container is not running or the file does not have the block.
    fn script_conditions(
        &self,
        containers: &[&Container],
        r: ScriptBlockRef,
    ) -> Result<Vec<ScriptConditionInfo>, String> {
        let Some(c) = containers.iter().find(|c| c.id == r.container) else {
            let running: Vec<String> = containers.iter().map(|c| c.id.to_string()).collect();
            return Err(format!(
                "script container {} is not running (running: {})",
                r.container,
                if running.is_empty() {
                    "none".to_string()
                } else {
                    running.join(", ")
                }
            ));
        };
        let file = self.rules.scripts.script(&c.source.path).map(|s| &s.file);
        let Some(rt) = file.and_then(|f| f.blocks.get(r.index as usize)) else {
            return Err(format!(
                "block {} does not exist in script {} ({} blocks)",
                r.index,
                c.source.path,
                file.map_or(0, |f| f.blocks.len())
            ));
        };
        let values =
            nml_sim::script::condition_values(&self.state, &self.rules, r.container, r.index)
                .unwrap_or_default();
        Ok(rt
            .conditions
            .iter()
            .enumerate()
            .map(|(i, k)| ScriptConditionInfo {
                number: k.number,
                or: k.or,
                params: k.params.clone(),
                value: values.get(i).copied().flatten(),
            })
            .collect())
    }

    /// A container for `script_state`: blocks with name from the file, state and wait reason.
    fn script_container_info(&self, c: &Container) -> ScriptContainerInfo {
        let file = self.rules.scripts.script(&c.source.path).map(|s| &s.file);
        let pairs = |map: &std::collections::BTreeMap<String, i32>| {
            map.iter().map(|(k, &v)| (k.clone(), v)).collect()
        };
        ScriptContainerInfo {
            id: c.id,
            path: c.source.path.clone(),
            counter: c.counter,
            switches: pairs(&c.switches),
            timers: pairs(&c.timers),
            variables: pairs(&c.variables),
            random_switches: pairs(&c.random_switches),
            blocks: c
                .blocks
                .iter()
                .map(|b| {
                    let rt = file.and_then(|f| f.blocks.get(b.index as usize));
                    let state = if b.running {
                        "wartet"
                    } else if !b.deleteable {
                        "wiederholbar"
                    } else {
                        "bereit"
                    };
                    ScriptBlockInfo {
                        index: b.index,
                        name: rt.map_or_else(String::new, |r| r.name.clone()),
                        state: state.into(),
                        wait: b.running.then(|| wait_reason(rt, b.cur_action, b.timeout)),
                        cur_action: b.cur_action,
                    }
                })
                .collect(),
        }
    }
    // --- AI match (protocol 26) -----------------------------------------------------------------

    /// `ki_state` (protocol 26): per engine (ascending by slot, only `slot` if given) the
    /// threads in execution order (MFC order) with module name from `nml.db`, state and the top
    /// [`KI_STACK_TOP`] stack values, with `values` also `m_iValues`; plus the counters of the AI commands
    /// ([`crate::game::sim_host::KiStats`], host state). Reads only.
    pub fn control_ki_state(
        &self,
        slot: Option<u8>,
        values: bool,
    ) -> Result<KiStateResult, String> {
        if let Some(s) = slot.filter(|&s| usize::from(s) >= PLAYER_SLOTS) {
            return Err(format!("slot = {s} invalid: allowed 0 to 7"));
        }
        let wanted = |p: u8| slot.is_none_or(|s| s == p);
        let engines = self
            .state
            .ki
            .engines
            .iter()
            .flatten()
            .filter(|e| wanted(e.slot))
            .map(|e| KiEngineInfo {
                slot: e.slot,
                next_thread_id: e.next_thread_id,
                threads: e
                    .threads
                    .iter()
                    .map(|(_, t)| self.ki_thread_info(t))
                    .collect(),
                values: if values {
                    e.mem.values.iter().map(|(&k, &v)| (k, v)).collect()
                } else {
                    Vec::new()
                },
                semaphores: e.mem.semaphores.iter().map(|(&k, &v)| (k, v)).collect(),
            })
            .collect();
        let stats = &self.ki_stats;
        let rejected = stats
            .rejected
            .iter()
            .filter(|((p, ..), _)| wanted(*p))
            .map(|((p, code, reason), &count)| KiRejectCount {
                slot: *p,
                code: *code,
                reason: reason.clone(),
                count,
            })
            .collect();
        let commands = stats
            .commands
            .iter()
            .filter(|((p, ..), _)| wanted(*p))
            .map(|(&(p, code, outcome), &count)| KiCommandCount {
                slot: p,
                code,
                outcome: outcome.into(),
                count,
            })
            .collect();
        Ok(KiStateResult {
            paused: self.state.ki.paused == KiPause::Paused,
            engines,
            rejected,
            commands,
        })
    }

    /// A thread (with its running `CALLM` child) for `ki_state`.
    fn ki_thread_info(&self, t: &Thread) -> KiThreadInfo {
        let name = self
            .rules
            .ki
            .get(t.module)
            .map_or_else(String::new, |p| p.name.clone());
        let top = t.stack.len().saturating_sub(KI_STACK_TOP);
        KiThreadInfo {
            id: t.id,
            module: t.module,
            name,
            pc: t.pc,
            status: t.status,
            pause_ms: t.pause_ms,
            wait_semaphore: t.wait_semaphore,
            stack_depth: t.stack.len(),
            stack_top: t.stack[top..].to_vec(),
            child: t.child.as_deref().map(|c| Box::new(self.ki_thread_info(c))),
        }
    }

    /// `ki_command` (protocol 26): translates the intervention into an AI control command
    /// (`KiStart`, `KiPause`, `KiStep`) of the active player for the next tick boundary; it is thus
    /// in the command log and in replays. Checked beforehand is what does not change until the
    /// tick boundary (slot with engine, module in the catalog, limits) and whether the AI is then
    /// halted (`step`: state after the already waiting `KiPause`); the simulation checks
    /// again.
    pub fn control_ki_command(&mut self, request: KiCommandRequest) -> Result<(), String> {
        let KiCommandRequest {
            action,
            slot,
            module,
            args,
            sync,
            paused,
            passes,
        } = request;
        let engine = |slot: Option<u8>| -> Result<u8, String> {
            let s = slot.ok_or("slot missing")?;
            if usize::from(s) >= PLAYER_SLOTS {
                return Err(format!("slot = {s} invalid: allowed 0 to 7"));
            }
            if self.state.ki.engines[usize::from(s)].is_none() {
                return Err(format!("slot {s} has no AI engine (AI off or slot empty)"));
            }
            Ok(s)
        };
        let kind = match action {
            KiAction::Start => {
                let slot = engine(slot)?;
                let module = module.ok_or("ki_command start needs module")?;
                if self.rules.ki.get(module).is_none() {
                    return Err(format!("module {module} unknown (not in nml.db)"));
                }
                let args = args.unwrap_or_default();
                if args.len() > MAX_START_ARGS {
                    return Err(format!(
                        "args has {} values: at most {MAX_START_ARGS} allowed",
                        args.len()
                    ));
                }
                CommandKind::KiStart {
                    slot,
                    module,
                    args,
                    sync: sync.unwrap_or(false),
                }
            }
            KiAction::Pause => CommandKind::KiPause {
                paused: paused.unwrap_or(true),
            },
            KiAction::Step => {
                let slot = engine(slot)?;
                let passes = passes.unwrap_or(1);
                if !(1..=MAX_STEP_PASSES).contains(&passes) {
                    return Err(format!(
                        "passes = {passes} invalid: allowed 1 to {MAX_STEP_PASSES}"
                    ));
                }
                let mut halted = self.state.ki.paused == KiPause::Paused;
                for c in self.pending_commands() {
                    if let CommandKind::KiPause { paused } = c.kind {
                        halted = paused;
                    }
                }
                if !halted {
                    return Err("AI is running: ki_command pause first".into());
                }
                CommandKind::KiStep { slot, passes }
            }
        };
        let player = self.active_player;
        self.submit(Command { player, kind });
        Ok(())
    }

    /// Extra energy (attribute 15) as current value with the upgrade percentages of the owner.
    fn extra_energy(&self, o: &GameObject) -> i32 {
        o.attributes
            .current_value(EXTRA_ENERGY, self.upgrade_percent(o, EXTRA_ENERGY))
    }

    /// Stealth module 25 for `sim_query` (protocol 19, re/masking 1.2, 3): the fields of the module, plus
    /// the extra energy (attribute 15) as a fraction.
    pub(crate) fn masking_info(o: &GameObject) -> Option<SimMaskingInfo> {
        let ModuleKind::Masking(m) = &o.module(masking::INDEX)?.kind else {
            return None;
        };
        Some(SimMaskingInfo {
            is_masked: m.is_masked,
            is_busy: m.is_busy,
            mask_time: m.mask_time,
            never_unmasked: m.never_unmasked,
            energy: o.attributes.current_fraction(EXTRA_ENERGY),
        })
    }

    /// Assassin module 28 for `sim_query` (protocol 19, re/masking 1.3, 4); an empty target
    /// (`NONE`) is absent.
    pub(crate) fn assassin_info(o: &GameObject) -> Option<SimAssassinInfo> {
        let ModuleKind::Assassin(a) = &o.module(assassin::INDEX)?.kind else {
            return None;
        };
        Some(SimAssassinInfo {
            is_down: a.is_down,
            target: handle(a.target),
            attack_points: a.attack_points,
        })
    }

    /// Healing module 29 for `sim_query` (protocol 19, re/healing 1.2, 9): the fields of the module,
    /// `single` from `SINGLEHEALING`, plus the mana (attribute 15) with the upgrade percentages of the
    /// owner (like `Sim::attribute`). Empty IDs (`ObjectId(0)`) are absent.
    pub(crate) fn healing_info(&self, o: &GameObject) -> Option<SimHealingInfo> {
        let module = o.module(healing::INDEX)?;
        let ModuleKind::Healing(h) = &module.kind else {
            return None;
        };
        Some(SimHealingInfo {
            ready: h.ready,
            single: module.base.flags & healing::flags::SINGLE != 0,
            sick: handle(h.sick),
            new_target: handle(h.new_target),
            healing_points: h.healing_points,
            mana: o
                .attributes
                .current_value(EXTRA_ENERGY, self.upgrade_percent(o, EXTRA_ENERGY)),
        })
    }

    /// Ghost evoker module 26 for `sim_query` (protocol 20, re/ghosts-banish 1.2, 3): the
    /// fields of the module, `evoking` = status `EVOKING`, plus extra energy (attribute 15) as
    /// current value with the upgrade percentages of the owner and its base value (the key-0
    /// absolute value from which the running ghosts bind their costs, C-257). Empty IDs are absent.
    pub(crate) fn evoke_ghost_info(&self, o: &GameObject) -> Option<SimEvokeGhostInfo> {
        let ModuleKind::EvokeGhost(e) = &o.module(evoke::INDEX)?.kind else {
            return None;
        };
        Some(SimEvokeGhostInfo {
            evoking: o.status & status::EVOKING != 0,
            dead_body: handle(e.dead_body),
            ghost: handle(e.ghost),
            extra_energy: e.extra_energy,
            mana: o
                .attributes
                .current_value(EXTRA_ENERGY, self.upgrade_percent(o, EXTRA_ENERGY)),
            mana_base: o.attributes.base(EXTRA_ENERGY),
        })
    }

    /// Ghost module 27 for `sim_query` (protocol 20, re/ghosts-banish 1.4, 3); an empty target
    /// is absent, a stale one is retained.
    pub(crate) fn ghost_info(o: &GameObject) -> Option<SimGhostInfo> {
        let ModuleKind::Ghost(g) = &o.module(ghost::INDEX)?.kind else {
            return None;
        };
        Some(SimGhostInfo {
            status: g.status,
            alpha: g.alpha,
            target: handle(g.target),
        })
    }

    /// Banishment module 31 for `sim_query` (protocol 20, re/ghosts-banish 1.3, 3): `active` is
    /// `ANIM_LOCKED` (also the healing trance, C-254), `effects` a running timer `0x41F`,
    /// `radius` as with `0x21F` from [`banish::banish_radius`].
    pub(crate) fn banish_info(&self, o: &GameObject) -> Option<SimBanishInfo> {
        let module = o.module(banish::INDEX)?;
        let ModuleKind::Banish(b) = &module.kind else {
            return None;
        };
        Some(SimBanishInfo {
            active: o.status & status::ANIM_LOCKED != 0,
            effects: module.base.has_pending_timer(banish::msg::EFFECTS_OFF),
            spell_guid: b.spell_guid,
            radius: banish::banish_radius(&self.state, o.id),
        })
    }

    /// Animal evoker module 32 for `sim_query` (protocol 20, re/evoke-aura 1.2, 3): the fields
    /// of the module, `evoking` = status `EVOKING`, plus the extra energy as with
    /// [`SimHost::evoke_ghost_info`].
    pub(crate) fn evoke_animal_info(&self, o: &GameObject) -> Option<SimEvokeAnimalInfo> {
        let ModuleKind::EvokeAnimal(e) = &o.module(evoke_animal::INDEX)?.kind else {
            return None;
        };
        Some(SimEvokeAnimalInfo {
            evoking: o.status & status::EVOKING != 0,
            art: e.art,
            cost: e.cost,
            reserve: e.reserve,
            spawn: e.spawn,
            mana: o
                .attributes
                .current_value(EXTRA_ENERGY, self.upgrade_percent(o, EXTRA_ENERGY)),
        })
    }

    /// Influence and aura of module 44 for `sim_query` (protocol 20, re/evoke-aura 1.4): only with a
    /// non-empty influence list or an existing set; `ring` as the host shows it
    /// ([`influence::ring_shown`]). The IDs of the set may be stale.
    pub(crate) fn influence_info(o: &GameObject) -> Option<SimInfluenceInfo> {
        let module = o.module(exec_temp::INDEX)?;
        let ModuleKind::ExecTempUpgrade(e) = &module.kind else {
            return None;
        };
        if e.influences.is_empty() && e.influenced.is_none() {
            return None;
        }
        Some(SimInfluenceInfo {
            guids: e.influences.iter().map(|i| i.guid).collect(),
            influenced: e
                .influenced
                .as_ref()
                .map(|set| set.iter().map(|id| id.0).collect()),
            ring: influence::ring_shown(module),
        })
    }

    /// Message filter for `sim_query` (protocol 19): only if it differs from the default (mode 1,
    /// empty list).
    pub(crate) fn message_filter_info(o: &GameObject) -> Option<SimMessageFilterInfo> {
        (o.filter != MessageFilter::default()).then(|| SimMessageFilterInfo {
            mode: o.filter.mode,
            codes: o.filter.codes.clone(),
        })
    }

    /// `ISARMED` of the arming module 39 for `sim_query` (protocol 16); `None` without the module.
    pub(crate) fn armed_info(o: &GameObject) -> Option<bool> {
        Some(o.module(arming::INDEX)?.base.flags & arming::ISARMED != 0)
    }

    /// Execution list of module 44 for `sim_query` (protocol 16); empty without the module.
    pub(crate) fn exec_list_info(o: &GameObject) -> Vec<u32> {
        match o.module(exec_temp::INDEX).map(|m| &m.kind) {
            Some(ModuleKind::ExecTempUpgrade(e)) => e.exec_list.clone(),
            _ => Vec::new(),
        }
    }

    /// Deposit for `sim_query` (protocol 9). Like `Sim::attribute` with the upgrade percentages
    /// of `GameWorld_GetTypeUpgradePercent` (the test compares both).
    pub(crate) fn resource_info(&self, o: &GameObject) -> Option<SimResourceInfo> {
        let ModuleKind::Resource(r) = &o.module(resource::INDEX)?.kind else {
            return None;
        };
        Some(SimResourceInfo {
            attribute: r.resource_id,
            amount: o
                .attributes
                .current_value(r.resource_id, self.upgrade_percent(o, r.resource_id)),
        })
    }

    /// Tree for `sim_query` (protocol 9); remaining wood like [`SimHost::resource_info`].
    pub(crate) fn tree_info(&self, o: &GameObject) -> Option<SimTreeInfo> {
        let ModuleKind::Tree(t) = &o.module(tree::INDEX)?.kind else {
            return None;
        };
        Some(SimTreeInfo {
            wood: o
                .attributes
                .current_value(tree::WOOD, self.upgrade_percent(o, tree::WOOD)),
            falling: t.status == tree::state::FALLING,
            registered: t.reg_pos.iter().filter_map(|&r| handle(r)).collect(),
        })
    }

    /// Combat line of the overlay: first selected object, otherwise the object under the mouse; `None`
    /// without combat values ([`SimHost::combat_info`]). Read-only.
    pub fn combat_line(&self, selection: &[ObjectId], hovered: Option<ObjectId>) -> Option<String> {
        let id = selection.first().copied().or(hovered)?;
        let o = self.state.object(id, false)?;
        self.combat_info(o)
            .map(|c| crate::game::combat_calc::combat_line(&c))
    }

    /// Combat section for `sim_query` (protocol 11, re/combat-host 7): objects with
    /// hit points (attribute 14) or dying, attack or melee module (16/17/18). Read-only;
    /// hit points with the upgrade percentages of the owner.
    pub fn combat_info(&self, o: &GameObject) -> Option<SimCombatInfo> {
        let fights = [dying::INDEX, attack::INDEX, fight_near::INDEX]
            .into_iter()
            .any(|i| o.module(i).is_some());
        if !fights && !o.attributes.is_present(HIT_POINTS) {
            return None;
        }
        let attack = match o.module(attack::INDEX).map(|m| &m.kind) {
            Some(ModuleKind::Attack(a)) => Some(SimAttackInfo {
                status: a.status,
                target: handle(a.target),
                automatic: a.automatic,
                aggressiveness: a.aggressiveness,
                mode: a.mode,
                stubbornness: a.stubbornness,
                search_counter: a.search_counter,
            }),
            _ => None,
        };
        let dying = match o.module(dying::INDEX).map(|m| &m.kind) {
            Some(ModuleKind::Dying(d)) => Some(SimDyingInfo {
                status: d.status,
                die_counter: d.die_counter,
                rob_gold: d.rob_gold,
                listeners: d.listeners.iter().count(),
            }),
            _ => None,
        };
        Some(SimCombatInfo {
            hp: [
                o.attributes
                    .current_value(HIT_POINTS, self.upgrade_percent(o, HIT_POINTS)),
                o.attributes
                    .max_value(HIT_POINTS, self.upgrade_percent(o, HIT_POINTS)),
            ],
            hp_fraction: o.attributes.current_fraction(HIT_POINTS),
            rank: o.rank,
            experience: o.attributes.current_fraction(EXPERIENCE),
            last_attacker: handle(o.last_attacker),
            ticks_since_hit: o.ticks_since_hit,
            render_bits: o.render_bits,
            attack,
            dying,
        })
    }

    /// Ranged-combat and shot module for `sim_query` (protocol 12). Read-only.
    pub(crate) fn ranged_info(&self, o: &GameObject) -> Option<SimRangedInfo> {
        let ModuleKind::FightFar(f) = &o.module(fight_far::INDEX)?.kind else {
            return None;
        };
        let shoot = o.module(shoot::INDEX).and_then(|m| match &m.kind {
            ModuleKind::Shoot(s) => Some(SimShootInfo {
                flags: m.base.flags,
                target: handle(s.target),
                bullets: [s.bullets_cur, s.bullets_count],
                shoot_delay: s.shoot_delay,
                busy: s.is_busy,
                gun_select: s.gun_select,
                is_steuerboard: s.is_steuerboard,
                cannon_counter: s.cannon_counter,
                can_shoot: s.can_shoot,
            }),
            _ => None,
        });
        Some(SimRangedInfo {
            min_dist: f.min_dist,
            max_dist: f.max_dist,
            shoot,
        })
    }

    /// Animal module for `sim_query` (protocol 12).
    pub(crate) fn animal_info(&self, o: &GameObject) -> Option<SimAnimalInfo> {
        let ModuleKind::Animal(a) = &o.module(animal::INDEX)?.kind else {
            return None;
        };
        Some(SimAnimalInfo {
            aggressive: a.aggressive,
            shy: a.shy,
            roamer: a.roamer,
            roaming: a.roaming,
            escaping: a.escaping,
            underwater: a.underwater,
            home: [a.home.0, a.home.1, a.home.2],
            alpha: a.alpha,
        })
    }

    /// Carcass module for `sim_query` (protocol 12).
    pub(crate) fn edible_info(&self, o: &GameObject) -> Option<SimEdibleInfo> {
        let m = o.module(edible::INDEX)?;
        let ModuleKind::Edible(_) = &m.kind else {
            return None;
        };
        Some(SimEdibleInfo {
            meat: [
                o.attributes
                    .current_value(MEAT, self.upgrade_percent(o, MEAT)),
                o.attributes.max_value(MEAT, self.upgrade_percent(o, MEAT)),
            ],
            has_meat: m.base.flags & edible::HAS_MEAT != 0,
            rotting: m.base.flags & edible::IS_ROT != 0,
        })
    }

    /// Projectiles of the simulation for the section `sim` (protocol 12).
    pub(crate) fn projectile_infos(&self) -> Vec<SimProjectileInfo> {
        self.state
            .projectiles
            .list
            .iter()
            .map(|p| SimProjectileInfo {
                id: p.id,
                kind: match p.kind {
                    ProjectileKind::Arrow => "arrow",
                    ProjectileKind::CannonBall => "ball",
                    ProjectileKind::Dynamite => "dynamite",
                    ProjectileKind::Explosion => "explosion",
                    ProjectileKind::Storm => "storm",
                    ProjectileKind::Throw => "throw",
                }
                .into(),
                fire: p.flags & FIRE_ARROW != 0,
                remaining_ms: p.remaining_ms,
                flight_ms: p.flight_ms,
                shooter: handle(p.shooter),
                target: handle(p.target),
                target_pos: p.target_pos,
                // Pulse counter (`zone`) only for the storm; for the other kinds `zone` has a different meaning.
                pulse: (p.kind == ProjectileKind::Storm).then_some(p.zone),
                // Impact message (`0xD22`/`0xE22`) only for the throw of the headhunter.
                impact: (p.kind == ProjectileKind::Throw).then_some(p.damage.code),
            })
            .collect()
    }
}

/// Attribute 24 meat (re/gathering A.2).
const MEAT: u32 = 24;
/// Flag "fire arrow" in `Projectile::flags`.
const FIRE_ARROW: u32 = 0x40;

/// Attribute 15 extra energy: stealth energy (fraction) and mana of the healers (re/masking 3, re/healing 9).
const EXTRA_ENERGY: u32 = 15;

/// Attribute 14 hit points and 30 experience (`default.dat`).
const HIT_POINTS: u32 = 14;
const EXPERIENCE: u32 = 30;

/// Wait reason of a waiting block for `script_state`: action index, the action in the
/// notation of the `.abl` (`w40;1000`) and remaining time or kind of event waiting (catalog
/// `WaitKind`; `timeout = -1` waits for an event).
pub(crate) fn wait_reason(
    block: Option<&nml_formats::abl::RtBlock>,
    cur_action: i32,
    timeout: i32,
) -> String {
    use nml_formats::abl::catalog::{self, WaitKind};
    let Some(a) = usize::try_from(cur_action)
        .ok()
        .and_then(|i| block?.actions.get(i))
    else {
        return format!("Action {cur_action} is missing in the script");
    };
    let mut text = format!("{}{}", if a.wait { "w" } else { "n" }, a.number);
    for p in &a.params {
        text += ";";
        text += p;
    }
    let kind = catalog::action(a.number).map_or(WaitKind::None, |s| s.wait);
    let reason = if timeout != -1 {
        format!("{timeout} ms remaining")
    } else {
        match kind {
            WaitKind::Text => "until the text line ends".into(),
            WaitKind::EventBit(bit) => format!("until event bit {bit} is set"),
            WaitKind::Dialog => "until the dialog ends".into(),
            WaitKind::Time | WaitKind::None => "for an event".into(),
        }
    };
    format!("Action {cur_action} ({text}): {reason}")
}

/// Combat test world (saber fighter against warrior), also for the tests of the viewer.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use crate::game::economy::testkit::spawn;

    /// Walk like the colonist (hard radius 512, soft radius 1024).
    pub const WALK: &str = "PROPID 0
VALUEID 10001 6 1 3
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
VALUEID 10008 6 1 4
PROPID 2
VALUEID 30001 1 1 0
";

    pub const SABRE: u32 = 1107;
    pub const WARRIOR: u32 = 1400;
    /// Object without modules and without hit points.
    pub const OBSERVER: u32 = 2518;

    /// Saber fighter and warrior like `combat::testkit` (type rows as text, values from `default.dat`):
    /// walk, dying (16), attack (17) and melee (18).
    pub fn fighter_types() -> String {
        let fighter = |guid: u32, name: &str, die: i32, attack: u32, near: i32| {
            format!(
                "GUID {guid}
NAME {name}
VALUEID 100 6 1 0
{WALK}PROPID 16
VALUEID 170001 6 1 0
VALUEID 170002 6 1 {die}
PROPID 17
VALUEID 180001 6 1 {attack}
VALUEID 180002 6 1 0
PROPID 18
VALUEID 190001 6 1 0
VALUEID 190002 1 1 {near}
"
            )
        };
        fighter(SABRE, "SPA_Saebelkaempfer", 3, 256, 2)
            + &fighter(WARRIOR, "WID_Krieger", 11, 16640, 14)
            + &format!(
                "GUID {OBSERVER}
NAME Observer
VALUEID 100 6 1 0
"
            )
    }

    /// Host with a saber fighter (player 0) and a warrior (player 1) next to each other.
    pub fn combat_host() -> (SimHost, ObjectId, ObjectId) {
        let mut rules = nml_sim::rules::Rules::synthetic(&fighter_types(), 64);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        rules.defaults.insert(
            SABRE,
            set(&[(14, 190), (16, 200), (18, 32), (19, 18), (21, 8)]),
        );
        rules.defaults.insert(
            WARRIOR,
            set(&[(14, 160), (16, 183), (18, 40), (19, 9), (21, 8)]),
        );
        rules.set_anim_labels(SABRE, 2, &[(1, 1200), (2, 320)]);
        rules.set_anim_labels(WARRIOR, 14, &[(1, 1283), (2, 560), (0x16, 1096)]);
        let setup = nml_sim::setup::MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: nml_sim::ki::KiSetup::default(),
            script: nml_sim::script::ScriptSetup::default(),
            ..nml_sim::setup::MatchSetup::default()
        };
        let mut host =
            SimHost::new(rules, setup, crate::game::options::DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host.state.players[0].control = nml_sim::state::Control::Human;
        nml_sim::fog::reveal_map(&mut host.state, -1);
        let sabre = spawn(&mut host, SABRE, 0, (10, 10));
        let warrior = spawn(&mut host, WARRIOR, 1, (11, 10));
        (host, sabre, warrior)
    }
}

/// Ground height vertically below `(x, z)` with the query of the mouse point when building
/// ([`Terrain::cursor_point`] like `FUN_004CCF00`, cursor `0x00671B00`): terrain, or, if it lies below
/// the water level, the water height; outside the map the water height. The ray starts one
/// unit above terrain or water and points vertically down; the hit thus lies exactly
/// at `(x, z)`, like a mouse click from above on this point.
fn ground_below(terrain: &Terrain, x: f32, z: f32) -> f32 {
    let level = terrain.params.water_level;
    let origin = Vec3::new(x, terrain.height_at(x, z).max(level) + 1.0, z);
    terrain
        .cursor_point(origin, Vec3::NEG_Y, 100_000.0)
        .map_or(level, |p| p.y)
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::game::economy::testkit::{COLONIST, HOUSE, HQ, MINE, STORE, TREE, host, spawn};
    use nml_sim::economy::{message, resource};
    use nml_sim::save::checksum;
    use nml_sim::sim::Sim;

    fn world(t: i32) -> f32 {
        t as f32 * 128.0 + 64.0
    }

    /// Commands of the next tick from the command log.
    fn logged(host: &mut SimHost) -> Vec<CommandKind> {
        let before = host.log.len();
        host.step_once();
        host.log[before..]
            .iter()
            .map(|c| c.command.kind.clone())
            .collect()
    }

    fn info(host: &SimHost, id: ObjectId) -> nml_control::SimObjectInfo {
        host.query(None, None, Some(&[id.0]), 1).objects.remove(0)
    }

    #[test]
    fn combat_commands_have_the_json_form_of_viewer_control() {
        // The examples from `docs/viewer-control.md` (`sim_command`, protocol 11).
        let command: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"Attack": {"units": [2048], "target": 6144, "pos": null, "mods": 0}}}"#,
        )
        .unwrap();
        assert_eq!(
            command.kind,
            CommandKind::Attack {
                units: vec![ObjectId(2048)],
                target: ObjectId(6144),
                pos: None,
                mods: 0,
            }
        );
        let command: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"SetBehavior": {"units": [2048], "mode": 3}}}"#,
        )
        .unwrap();
        assert_eq!(command_name(&command.kind), "SetBehavior");
        let command: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"GarrisonAttack": {"buildings": [4096], "target": 6144}}}"#,
        )
        .unwrap();
        assert_eq!(command_name(&command.kind), "GarrisonAttack");
    }

    #[test]
    fn script_debug_commands_have_the_json_form_of_viewer_control() {
        // `DebugDestroy` via `sim_command`; `ScriptSkipDialog` is a unit variant (JSON
        // "ScriptSkipDialog", not an object) and also goes via `sim_command`, but there without the pre-check
        // "no dialog running" of `script_command` (the simulation then rejects in the tick).
        let destroy = parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"DebugDestroy": {"objects": [2048, 2049]}}}
        ))
        .unwrap();
        assert_eq!(
            destroy.kind,
            CommandKind::DebugDestroy {
                objects: vec![ObjectId(2048), ObjectId(2049)]
            }
        );
        assert_eq!(command_name(&destroy.kind), "DebugDestroy");
        let skip = parse_sim_command(serde_json::json!(
            {"player": 3, "kind": "ScriptSkipDialog"}
        ))
        .unwrap();
        assert_eq!(skip.kind, CommandKind::ScriptSkipDialog);
        assert_eq!(command_name(&skip.kind), "ScriptSkipDialog");
        assert_eq!(skip.player, 3);
    }

    #[test]
    fn signal_has_the_json_form_of_viewer_control() {
        // `Signal` (`0x5C`) via `sim_command`, slot field unchecked (O-321).
        let signal = parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"Signal": {"player": 5, "mask": 9, "x": 70, "z": 101}}}
        ))
        .unwrap();
        assert_eq!(
            signal.kind,
            CommandKind::Signal {
                player: 5,
                mask: 9,
                x: 70,
                z: 101
            }
        );
        assert_eq!(command_name(&signal.kind), "Signal");
    }

    #[test]
    fn chat_has_the_json_form_of_viewer_control() {
        // `Chat` (`0x5D`) via `sim_command`, text as UTF-16 units (also for the cheats).
        let chat = parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"Chat": {"player": 0, "mask": 1, "text": [104, 105]}}}
        ))
        .unwrap();
        assert_eq!(
            chat.kind,
            CommandKind::Chat {
                player: 0,
                mask: 1,
                text: vec![104, 105]
            }
        );
        assert_eq!(command_name(&chat.kind), "Chat");
    }

    #[test]
    fn tent_commands_have_the_json_form_of_viewer_control() {
        // `PackTent` (`0x2D`) and `UnpackTent` (`0x2E`) via `sim_command`.
        let pack: nml_sim::command::Command =
            serde_json::from_str(r#"{"player": 0, "kind": {"PackTent": {"tent": 2048}}}"#).unwrap();
        assert_eq!(
            pack.kind,
            CommandKind::PackTent {
                tent: ObjectId(2048)
            }
        );
        assert_eq!(pack.kind.type_code(), 0x5000_002D);
        let unpack: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"UnpackTent": {"unit": 2049, "x": 122880, "z": 8192}}}"#,
        )
        .unwrap();
        assert_eq!(
            unpack.kind,
            CommandKind::UnpackTent {
                unit: ObjectId(2049),
                x: 122_880,
                z: 8192,
            }
        );
        assert_eq!(unpack.kind.type_code(), 0x5000_002E);
        assert_eq!(command_name(&unpack.kind), "UnpackTent");
        for command in [pack, unpack] {
            let json = serde_json::to_string(&command).unwrap();
            let back: nml_sim::command::Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, command);
        }
    }

    #[test]
    fn gate_commands_have_the_json_form_of_viewer_control() {
        // `OpenGate` (`0x19`) and `CloseGate` (`0x1A`) via `sim_command`.
        let open: nml_sim::command::Command =
            serde_json::from_str(r#"{"player": 0, "kind": {"OpenGate": {"gate": 4096}}}"#).unwrap();
        assert_eq!(
            open.kind,
            CommandKind::OpenGate {
                gate: ObjectId(4096)
            }
        );
        assert_eq!(open.kind.type_code(), 0x5000_0019);
        let close: nml_sim::command::Command =
            serde_json::from_str(r#"{"player": 1, "kind": {"CloseGate": {"gate": 6144}}}"#)
                .unwrap();
        assert_eq!(
            close.kind,
            CommandKind::CloseGate {
                gate: ObjectId(6144)
            }
        );
        assert_eq!(close.kind.type_code(), 0x5000_001A);
        assert_eq!(command_name(&close.kind), "CloseGate");
        for command in [open, close] {
            let json = serde_json::to_string(&command).unwrap();
            let back: nml_sim::command::Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, command);
        }
    }

    #[test]
    fn gate_and_watch_info_leaves_the_checksum_unchanged() {
        // Protocol 18: closed gate with blocked door tiles, guarded ring with an enemy in the list;
        // the query changes nothing.
        use crate::game::testworld::{CART, DWELLING_TENT, GATE, host, spawn};
        use nml_sim::area::{AreaQuery, ENEMY_WATCH};
        let mut host = host();
        let gate = spawn(&mut host, GATE, 0, (10, 10));
        let enemy = spawn(&mut host, CART, 1, (12, 11));
        let plain = spawn(&mut host, CART, 0, (30, 30));
        let tent = spawn(&mut host, DWELLING_TENT, 0, (40, 40));
        // Open: gate state without blocked door tiles.
        let g = info(&host, gate).building.unwrap().gate.unwrap();
        assert!(g.open && !g.closed && !g.doors.is_empty());
        assert!(g.doors.iter().all(|d| !d.blocked));
        let door_count = g.doors.len();
        host.submit_own(CommandKind::CloseGate { gate });
        host.step_once();
        let g = info(&host, gate).building.unwrap().gate.unwrap();
        assert!(!g.open && g.closed);
        assert_eq!(g.doors.len(), door_count);
        assert!(
            g.doors.iter().all(|d| d.blocked),
            "door closed: layer 0 blocked"
        );
        for d in &g.doors {
            assert_eq!(
                host.state.path.cell_cost(d.x, d.z, 0),
                nml_sim::path::grid::COST_BLOCKED
            );
        }
        // Set the watch and rectangle directly in the state (state as after `register`).
        let rect = [8, 8, 20, 14];
        host.state.area_queries.counter = 1;
        host.state.area_queries.queries.insert(
            1,
            AreaQuery {
                mode: ENEMY_WATCH,
                rect,
                player: 8,
                list: vec![enemy],
            },
        );
        {
            let o = host.state.object_mut(gate, false).unwrap();
            let ModuleKind::Building(b) = &mut o.module_mut(building::INDEX).unwrap().kind else {
                panic!("building module");
            };
            b.check_area_id = 1;
            b.check_area_rect = rect;
            b.check_area_mode = ENEMY_WATCH;
        }
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let b = info(&host, gate).building.unwrap();
        let w = b.area_watch.expect("watch running");
        assert_eq!(
            (w.key, w.mode, w.rect, w.player, w.members.clone()),
            (1, ENEMY_WATCH, rect, 8, vec![enemy.0])
        );
        assert_eq!(b.watch_rect, Some(rect));
        assert!(b.gate.is_some());
        // Unit without a building module: no building section.
        assert!(info(&host, plain).building.is_none());
        // Building without `TOR` (dwelling tent, building module without watch): gate, watch and rectangle are absent.
        let t = info(&host, tent).building.expect("building module");
        assert!(t.gate.is_none() && t.area_watch.is_none() && t.watch_rect.is_none());
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // Building with `check_area_id` but without an entry in the registry: no watch.
        {
            let o = host.state.object_mut(tent, false).unwrap();
            let ModuleKind::Building(b) = &mut o.module_mut(building::INDEX).unwrap().kind else {
                panic!("building module");
            };
            b.check_area_id = 77;
        }
        let t = info(&host, tent).building.unwrap();
        assert!(t.gate.is_none() && t.area_watch.is_none() && t.watch_rect.is_none());
        // Watch ended (`check_area_id = -1`): rectangle remains, section gone.
        {
            let o = host.state.object_mut(gate, false).unwrap();
            let ModuleKind::Building(b) = &mut o.module_mut(building::INDEX).unwrap().kind else {
                panic!("building module");
            };
            b.check_area_id = -1;
        }
        let b = info(&host, gate).building.unwrap();
        assert!(b.area_watch.is_none());
        assert_eq!(b.watch_rect, Some(rect));
    }

    /// Message `code` without values to `id` (synchronous, like a module call of the simulation).
    fn say(host: &mut SimHost, id: ObjectId, code: u32) -> i32 {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        nml_sim::messaging::send(&mut sim, id, &nml_sim::message::Message::new(code, &[]))
    }

    /// `n` object ticks (two `step` calls each).
    fn run_ticks(host: &mut SimHost, n: usize) {
        for _ in 0..2 * n {
            nml_sim::tick::step(&mut host.state, &host.rules, &[]);
        }
    }

    #[test]
    fn ability_info_leaves_the_checksum_unchanged() {
        // Protocol 19: stealthed archer, dug-in assassin with target, medic in trance with filter;
        // the query shows the fields as set and changes nothing.
        use crate::game::testworld::{
            ARCHER, ASSASSIN, CART, DIG_IN_UPGRADE, MASK_UPGRADE, MEDIC, host, spawn, unlock,
        };
        use nml_control::{SimAssassinInfo, SimHealingInfo, SimMessageFilterInfo};
        use nml_sim::message::MessageFilter;
        use nml_sim::module::{assassin, healing, masking};
        let mut host = host();
        unlock(&mut host, 0, MASK_UPGRADE);
        unlock(&mut host, 0, DIG_IN_UPGRADE);
        let archer = spawn(&mut host, ARCHER, 0, (10, 10));
        let killer = spawn(&mut host, ASSASSIN, 0, (14, 10));
        let medic = spawn(&mut host, MEDIC, 0, (18, 10));
        let plain = spawn(&mut host, CART, 0, (30, 30));
        // Before: modules with initial values, without targets, without filter and without presentation.
        let a = info(&host, archer);
        let m = a.masking.expect("stealth module");
        assert!(!m.is_masked && !m.is_busy && !m.never_unmasked);
        assert_eq!((m.mask_time, m.energy), (1_600, 100_000));
        assert!(a.look.is_none() && a.message_filter.is_none());
        let k = info(&host, killer).assassin.expect("assassin module");
        assert_eq!((k.is_down, k.target, k.attack_points), (false, None, 250));
        let h = info(&host, medic).healing.expect("healing module");
        assert_eq!(
            (h.ready, h.single, h.sick, h.new_target),
            (false, true, None, None)
        );
        // C-241: the freshly created medic starts with mana 0.
        assert_eq!((h.healing_points, h.mana), (288, 0));
        // Stealthed (`0x119`, after 18 ticks), the energy set to a fixed fraction.
        assert_eq!(say(&mut host, archer, masking::msg::START), 0);
        run_ticks(&mut host, 18);
        host.state
            .object_mut(archer, false)
            .unwrap()
            .attributes
            .set_current_fraction(15, 62_500);
        // Dug in with target; filter as after `0x11C` (mode 0, only dig out and dying allowed).
        {
            let o = host.state.object_mut(killer, false).unwrap();
            let ModuleKind::Assassin(a) = &mut o.module_mut(assassin::INDEX).unwrap().kind else {
                panic!("assassin module");
            };
            a.is_down = true;
            a.target = archer;
            o.filter = MessageFilter {
                mode: 0,
                codes: vec![0x21C, 0x110],
            };
        }
        // Medic in trance: ready, target, requested target; filter as after `0x11D` (only `0x21D`).
        {
            let o = host.state.object_mut(medic, false).unwrap();
            let ModuleKind::Healing(h) = &mut o.module_mut(healing::INDEX).unwrap().kind else {
                panic!("healing module");
            };
            h.ready = true;
            h.sick = archer;
            h.new_target = killer;
            // Half mana: (950 * 50,000 + 50,000) / 100,000 = 475.
            o.attributes.set_current_fraction(15, 50_000);
            o.filter = MessageFilter {
                mode: 0,
                codes: vec![0x21D],
            };
        }
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let m = info(&host, archer).masking.unwrap();
        assert!(m.is_masked && !m.is_busy);
        assert_eq!(m.energy, 62_500);
        let k = info(&host, killer);
        assert_eq!(
            k.assassin.unwrap(),
            SimAssassinInfo {
                is_down: true,
                target: Some(archer.0),
                attack_points: 250,
            }
        );
        assert_eq!(
            k.message_filter.unwrap(),
            SimMessageFilterInfo {
                mode: 0,
                codes: vec![0x21C, 0x110],
            }
        );
        let h = info(&host, medic);
        assert_eq!(
            h.healing.unwrap(),
            SimHealingInfo {
                ready: true,
                single: true,
                sick: Some(archer.0),
                new_target: Some(killer.0),
                healing_points: 288,
                mana: 475,
            }
        );
        assert_eq!(h.message_filter.unwrap().codes, vec![0x21D]);
        // Unit without these modules: none of them, and the default filter is absent.
        let p = info(&host, plain);
        assert!(p.masking.is_none() && p.assassin.is_none() && p.healing.is_none());
        assert!(p.message_filter.is_none() && p.look.is_none());
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // The target is destroyed: the stale ID stays unchanged, the query survives it.
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            nml_sim::world::destroy_object(&mut sim, archer);
        }
        let sum = (host.checksum(), nml_sim::save::checksum(&host.state));
        assert_eq!(info(&host, killer).assassin.unwrap().target, Some(archer.0));
        assert_eq!(info(&host, medic).healing.unwrap().sick, Some(archer.0));
        assert_eq!((host.checksum(), nml_sim::save::checksum(&host.state)), sum);
    }

    /// Message `code` with arguments to `id` (synchronous, like a module call of the simulation).
    fn say_with(host: &mut SimHost, id: ObjectId, code: u32, args: &[i32]) -> i32 {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        nml_sim::messaging::send(&mut sim, id, &nml_sim::message::Message::new(code, args))
    }

    #[test]
    fn evocation_info_leaves_the_checksum_unchanged() {
        // Protocol 20: a real evocation (corpse, `0x11A`, ghost, attack on an enemy), a banishment, an
        // animal evocation and an aura; the query shows the fields as the simulation left them after twelve
        // object ticks and changes nothing.
        use crate::game::testworld::{
            ANIMAL_EVOKER, ARCHER, CART, GHOST_EVOKER, MISSIONARY, WOLF, host, spawn,
        };
        use nml_control::{SimBanishInfo, SimEvokeGhostInfo, SimGhostInfo};
        use nml_sim::message::Message;
        use nml_sim::messaging::post_message;
        use nml_sim::module::ghosts::{banish, evoke, ghost};
        use nml_sim::module::{evoke_animal, exec_temp, influence};
        const MANA: u32 = 15;
        let mut host = host();
        let evoker = spawn(&mut host, GHOST_EVOKER, 0, (10, 10));
        let caster = spawn(&mut host, GHOST_EVOKER, 0, (14, 10));
        let victim = spawn(&mut host, ARCHER, 1, (10, 5));
        let foe = spawn(&mut host, ARCHER, 1, (12, 5));
        let summoner = spawn(&mut host, ANIMAL_EVOKER, 0, (20, 10));
        let wolf = spawn(&mut host, WOLF, 0, (30, 30));
        let near = spawn(&mut host, ARCHER, 0, (31, 30));
        let missionary = spawn(&mut host, MISSIONARY, 0, (40, 10));
        let plain = spawn(&mut host, CART, 0, (50, 50));
        // Before: idle state, no influence, no set.
        let e = info(&host, evoker).evoke_ghost.expect("module 26");
        assert!(!e.evoking && e.dead_body.is_none() && e.ghost.is_none());
        assert_eq!((e.extra_energy, e.mana_base), (0, 150));
        let b = info(&host, caster).banish.expect("module 31");
        assert!(!b.active && !b.effects && b.spell_guid == 0);
        assert_eq!(b.radius, 26_624, "sight 52 * 4096 >> 3");
        let a = info(&host, summoner).evoke_animal.expect("module 32");
        assert!(!a.evoking && (a.art, a.cost, a.reserve) == (0, 0, 0));
        assert_eq!(a.spawn, [0; 3]);
        // Module kinds in the module list: written as one word, like all the others.
        let kinds = |id| -> Vec<String> {
            info(&host, id)
                .modules
                .into_iter()
                .map(|m| m.kind)
                .collect()
        };
        assert!(kinds(evoker).iter().any(|k| k == "evokeghost"));
        assert!(kinds(caster).iter().any(|k| k == "banish"));
        assert!(kinds(summoner).iter().any(|k| k == "evokeanimal"));
        assert!(info(&host, wolf).influence.is_none());
        assert!(info(&host, victim).ghost.is_none());
        // Corpse: the victim dies (`0x110`), after one tick it lies as a corpse on slot 8.
        say(&mut host, victim, 0x110);
        run_ticks(&mut host, 1);
        let corpse = host
            .state
            .objects()
            .find(|o| o.guid == ARCHER && o.owner() == 8)
            .map(|o| o.id)
            .expect("corpse on slot 8");
        for id in [evoker, caster, summoner] {
            host.state
                .object_mut(id, false)
                .unwrap()
                .attributes
                .set_current_fraction(MANA, 100_000);
        }
        // Banishment and animal evocation: real casts (mana 150 -> 125 or 100 -> 50), wolf: influence 0.
        assert_eq!(say_with(&mut host, caster, banish::msg::START, &[547]), 0);
        assert_eq!(
            say_with(&mut host, summoner, evoke_animal::msg::CAST, &[1, 442]),
            1
        );
        assert_eq!(say_with(&mut host, wolf, 0x22C, &[0]), 0);
        // Ghost evocation like the command `0x1F` (`0x11A {436}` with abort): tick N = 1 sets
        // `EVOKING`, N+3 creates the ghost on the corpse.
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            assert!(post_message(
                &mut sim,
                evoker,
                Message::new(evoke::msg::EVOKE, &[436]),
                true
            ));
        }
        run_ticks(&mut host, 4);
        let phantom = host
            .state
            .objects()
            .find(|o| o.guid == crate::game::testworld::GHOST)
            .map(|o| o.id)
            .expect("ghost after N+3");
        // The ghost appears (state 1, lock bit), two ticks later it rests.
        let g = info(&host, phantom).ghost.expect("module 27");
        assert_eq!((g.status, g.target), (1, None));
        assert_ne!(
            host.state.object(phantom, false).unwrap().status & status::ANIM_LOCKED,
            0
        );
        run_ticks(&mut host, 5);
        // Attack on the enemy (like `CommandKind::Attack`: `{0x111, [target, 0]}` with abort): from the
        // second tick on one suction strike per object tick.
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            assert!(post_message(
                &mut sim,
                phantom,
                Message::new(0x111, &[foe.as_i32(), 0]),
                true
            ));
        }
        run_ticks(&mut host, 4);
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let e = info(&host, evoker).evoke_ghost.unwrap();
        // The evocation is running (mark 3,999 ms: until N+40), the mana is bound: base value and
        // current value 125 after the cost of 25 from 150.
        assert_eq!(
            e,
            SimEvokeGhostInfo {
                evoking: true,
                dead_body: Some(corpse.0),
                ghost: Some(phantom.0),
                extra_energy: 25,
                mana: 125,
                mana_base: 125,
            }
        );
        assert_ne!(
            corpse, foe,
            "corpse and suction target are different objects"
        );
        // Measured: the suction strike is pending between two calls of `step` (state 3, until `0x71B`).
        let g = info(&host, phantom).ghost.unwrap();
        assert_eq!(
            g,
            SimGhostInfo {
                status: 3,
                alpha: 1.0,
                target: Some(foe.0),
            }
        );
        assert_eq!(
            info(&host, caster).banish.unwrap(),
            SimBanishInfo {
                active: true,
                effects: true,
                spell_guid: 547,
                radius: 26_624,
            }
        );
        let c = info(&host, caster).evoke_ghost.unwrap();
        // 150 - 25 + 1 regeneration after twelve object ticks; the base value remains: the banishment only pays.
        assert_eq!((c.mana, c.mana_base), (126, 150));
        let a = info(&host, summoner).evoke_animal.unwrap();
        assert!(a.evoking);
        assert_eq!((a.art, a.cost, a.reserve, a.mana), (1, 50, 50, 50));
        assert_ne!(a.spawn, [0; 3], "spawn position remembered");
        let w = info(&host, wolf).influence.expect("influence");
        assert_eq!(w.guids, vec![4460]);
        assert!(w.ring, "ring with set");
        // The set contains every object with walk in the radius, including the wolf itself.
        let mut set = w.influenced.clone().expect("set");
        set.sort_unstable();
        let mut expected = vec![wolf.0, near.0];
        expected.sort_unstable();
        assert_eq!(set, expected);
        let ring = host
            .state
            .object(wolf, false)
            .unwrap()
            .module(exec_temp::INDEX)
            .map(influence::ring_shown);
        assert_eq!(ring, Some(true));
        // Units without these modules: none of them; the missionary only has module 31 (sight 40).
        for id in [plain, missionary] {
            let p = info(&host, id);
            assert!(p.evoke_ghost.is_none() && p.ghost.is_none() && p.evoke_animal.is_none());
            assert!(p.influence.is_none());
        }
        assert!(info(&host, plain).banish.is_none());
        let m = info(&host, missionary).banish.unwrap();
        assert!(!m.active && !m.effects);
        assert_eq!(m.radius, (40 * 4096) >> 3);
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // The suction target is destroyed: the ID stays, the query survives it.
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            nml_sim::world::destroy_object(&mut sim, foe);
        }
        let sum = (host.checksum(), nml_sim::save::checksum(&host.state));
        assert_eq!(info(&host, phantom).ghost.unwrap().target, Some(foe.0));
        assert_eq!((host.checksum(), nml_sim::save::checksum(&host.state)), sum);
        // Fade-out (`0x21B`): semi-transparent, state 2; the values arrive unchanged.
        {
            let o = host.state.object_mut(phantom, false).unwrap();
            let ModuleKind::Ghost(g) = &mut o.module_mut(ghost::INDEX).unwrap().kind else {
                panic!("module 27");
            };
            g.status = 2;
            g.alpha = 0.5;
        }
        let g = info(&host, phantom).ghost.unwrap();
        assert_eq!((g.status, g.alpha), (2, 0.5));
    }

    #[test]
    fn spell_special_info_leaves_the_checksum_unchanged() {
        // Protocol 21: a spell caster with target point and with corpse, a demolition expert while laying, a
        // killer with target, a busy smith as well as storm, explosion and dynamite charge in the projectile
        // list; the query shows the fields as set and changes nothing.
        use crate::game::testworld::{
            COLONIST, MEDICINE_MAN, PATRIOT, SMITH, TRAPPER, host, spawn,
        };
        use nml_control::{SimBombermanInfo, SimKillerInfo, SimSpecialMoveInfo};
        use nml_sim::combat::projectile::{add_dynamite, add_explosion, add_storm};
        use nml_sim::module::special::{bomberman, killer, special_move};
        const MANA: u32 = 15;
        let mut host = host();
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let patriot = spawn(&mut host, PATRIOT, 0, (14, 10));
        let trapper = spawn(&mut host, TRAPPER, 0, (18, 10));
        let smith = spawn(&mut host, SMITH, 0, (22, 10));
        let plain = spawn(&mut host, COLONIST, 0, (26, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (14, 14));
        for (id, mana) in [(medicine, 80), (patriot, 60), (trapper, 120), (smith, 90)] {
            host.state
                .object_mut(id, false)
                .unwrap()
                .attributes
                .set_current_value(MANA, mana, 0);
        }
        // Idle: all new fields empty, module kinds in the module list.
        let s = info(&host, medicine).spell.expect("module 38");
        assert_eq!((s.mana, s.mana_max), (80, 100));
        assert!(!s.casting && s.target.is_none() && s.corpse.is_none());
        let b = info(&host, patriot).bomberman.expect("module 36");
        assert_eq!(b.target, None);
        assert_eq!(
            (b.extra_energy, b.return_point, b.busy, b.mana),
            (0, [0; 3], false, 60)
        );
        let k = info(&host, trapper).killer.expect("module 37");
        assert_eq!(
            (
                k.target,
                k.extra_energy,
                k.near_damage,
                k.far_damage,
                k.mana
            ),
            (None, 0, 300, 0, 120)
        );
        let m = info(&host, smith).special_move.expect("module 49");
        assert_eq!((m.busy, m.mode, m.mana), (false, 0, 90));
        let kinds = |id| -> Vec<String> {
            info(&host, id)
                .modules
                .into_iter()
                .map(|m| m.kind)
                .collect()
        };
        assert!(kinds(patriot).iter().any(|k| k == "bomberman"));
        assert!(kinds(trapper).iter().any(|k| k == "killer"));
        assert!(kinds(smith).iter().any(|k| k == "specialmove"));
        let p = info(&host, plain);
        assert!(p.bomberman.is_none() && p.killer.is_none() && p.special_move.is_none());
        assert!(p.spell.is_none());
        assert!(host.projectile_infos().is_empty());
        // Spell caster in the middle of a spell (kind 1, target point tile center (20, 12), lock bit).
        let aim = [20.0 * 128.0 + 64.0, 0.0, 12.0 * 128.0 + 64.0];
        {
            let o = host.state.object_mut(medicine, false).unwrap();
            o.status |= status::ANIM_LOCKED;
            let ModuleKind::Spell(s) = &mut o.module_mut(spell::INDEX).unwrap().kind else {
                panic!("module 38");
            };
            s.art = 1;
            s.button = 480;
            s.cost = 50;
            s.target = aim;
        }
        {
            let o = host.state.object_mut(patriot, false).unwrap();
            let ModuleKind::Bomberman(b) = &mut o.module_mut(bomberman::INDEX).unwrap().kind else {
                panic!("module 36");
            };
            b.target = enemy;
            b.extra_energy = 40;
            b.return_point = [57_344, 0, 40_960];
            b.busy = true;
        }
        {
            let o = host.state.object_mut(trapper, false).unwrap();
            let ModuleKind::Killer(k) = &mut o.module_mut(killer::INDEX).unwrap().kind else {
                panic!("module 37");
            };
            k.target = enemy;
            k.extra_energy = 30;
        }
        {
            let o = host.state.object_mut(smith, false).unwrap();
            let ModuleKind::SpecialMove(m) = &mut o.module_mut(special_move::INDEX).unwrap().kind
            else {
                panic!("module 49");
            };
            m.busy = true;
            m.mode = 1;
        }
        // Storm, explosion and charge get into the list via the functions of the simulation.
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            add_storm(&mut sim, medicine, aim);
            add_explosion(&mut sim, medicine, aim);
            add_dynamite(&mut sim, patriot, enemy, [1920.0, 0.0, 1920.0], 2);
        }
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!((s.art, s.button, s.cost, s.casting), (1, 480, 50, true));
        assert_eq!((s.target, s.corpse), (Some(aim), None));
        assert_eq!(
            info(&host, patriot).bomberman.unwrap(),
            SimBombermanInfo {
                target: Some(enemy.0),
                extra_energy: 40,
                return_point: [57_344, 0, 40_960],
                busy: true,
                mana: 60,
            }
        );
        assert_eq!(
            info(&host, trapper).killer.unwrap(),
            SimKillerInfo {
                target: Some(enemy.0),
                extra_energy: 30,
                near_damage: 300,
                far_damage: 0,
                mana: 120,
            }
        );
        assert_eq!(
            info(&host, smith).special_move.unwrap(),
            SimSpecialMoveInfo {
                busy: true,
                mode: 1,
                mana: 90,
            }
        );
        let list = host.projectile_infos();
        let kinds: Vec<(&str, Option<i32>)> =
            list.iter().map(|p| (p.kind.as_str(), p.pulse)).collect();
        assert_eq!(
            kinds,
            [("storm", Some(1)), ("explosion", None), ("dynamite", None)]
        );
        assert_eq!(list[0].target_pos, aim);
        assert_eq!((list[0].remaining_ms, list[0].flight_ms), (7000, 7000));
        assert_eq!((list[1].remaining_ms, list[1].flight_ms), (500, 500));
        assert_eq!(list[2].target, Some(enemy.0));
        assert_eq!(list[2].shooter, Some(patriot.0));
        assert_eq!((list[2].remaining_ms, list[2].flight_ms), (10_000, 10_000));
        // All queries above changed nothing.
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // Reincarnation: the corpse is in the spell, the target point only for the kinds 1, 2 and 5.
        {
            let o = host.state.object_mut(medicine, false).unwrap();
            let ModuleKind::Spell(s) = &mut o.module_mut(spell::INDEX).unwrap().kind else {
                panic!("module 38");
            };
            s.art = 3;
            s.button = 482;
            s.corpse = enemy;
        }
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!((s.art, s.target, s.corpse), (3, None, Some(enemy.0)));
        // Without a running spell (lock bit gone) kind 5 does not show a target point either.
        {
            let o = host.state.object_mut(medicine, false).unwrap();
            o.status &= !status::ANIM_LOCKED;
            let ModuleKind::Spell(s) = &mut o.module_mut(spell::INDEX).unwrap().kind else {
                panic!("module 38");
            };
            s.art = 5;
            s.corpse = ObjectId::NONE;
        }
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!((s.casting, s.target, s.corpse), (false, None, None));
        // The storm pulses with the tick: the counter alternates between 1 and 0 (re/spells 1.4), the list
        // remains until the end; queries in the meantime change nothing.
        run_ticks(&mut host, 1);
        let pulses: Vec<Option<i32>> = host.projectile_infos().iter().map(|p| p.pulse).collect();
        assert_eq!(pulses.first(), Some(&Some(0)));
        let sum = (host.checksum(), nml_sim::save::checksum(&host.state));
        let _ = host.projectile_infos();
        let _ = info(&host, medicine);
        assert_eq!((host.checksum(), nml_sim::save::checksum(&host.state)), sum);
    }

    #[test]
    fn unit_bar_with_target_issues_the_command() {
        // Protocol 21: `sim_unit_bar` with `target` or `ground` issues the target command
        // immediately for the first selected own object, without targeting mode.
        use crate::game::testworld::{
            CHIEF, COLONIST, EXPLOSION_UPGRADE, MEDICINE_MAN, SCOUT_UPGRADE, host, spawn, unlock,
        };
        use crate::game::unit_bar::{UnitBar, control_unit_bar};
        let mut host = host();
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        unlock(&mut host, 0, SCOUT_UPGRADE);
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let chief = spawn(&mut host, CHIEF, 0, (14, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (20, 12));
        host.step_once();
        let mut bar = UnitBar::default();
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        // 480 with object: `CastSpell` kind 1 with the `f32` bits of the tile center of the target.
        assert_eq!(
            control_unit_bar(&mut bar, &mut host, &[medicine], 480, Some(enemy.0), None),
            Ok(())
        );
        assert_eq!(
            logged(&mut host),
            [CommandKind::CastSpell {
                caster: medicine,
                art: 1,
                anim: at(20).to_bits() as i32,
                idx: at(12).to_bits() as i32,
                button: 480,
            }]
        );
        assert_eq!(bar.target_mode, None, "no targeting mode");
        // 484 with ground point (world coordinates x, z): `ScoutArea` with the tile; an object next to it
        // does not count, 484 always takes the ground.
        let ground = [20.0 * 128.0 + 10.0, 30.0 * 128.0 + 100.0];
        assert_eq!(
            control_unit_bar(
                &mut bar,
                &mut host,
                &[chief],
                484,
                Some(enemy.0),
                Some(ground)
            ),
            Ok(())
        );
        assert_eq!(
            logged(&mut host),
            [CommandKind::ScoutArea {
                unit: chief,
                x: 20,
                z: 30,
                size: 30,
                duration_ms: 5000,
                flag: 1,
                button: 484,
            }]
        );
        // Target button without target: error, no command, no targeting mode.
        let err = control_unit_bar(&mut bar, &mut host, &[medicine], 480, None, None).unwrap_err();
        assert_eq!(err, "Button 480 needs a target (target or ground)");
        assert_eq!(bar.target_mode, None);
        assert!(logged(&mut host).is_empty());
        // Unsuitable target (ground point for an object button) and a foreign first one: error without command.
        let err = control_unit_bar(&mut bar, &mut host, &[medicine], 480, None, Some(ground))
            .unwrap_err();
        assert!(!err.is_empty());
        assert!(
            control_unit_bar(
                &mut bar,
                &mut host,
                &[enemy, medicine],
                480,
                Some(enemy.0),
                None
            )
            .is_err()
        );
        assert!(logged(&mut host).is_empty());
        // Non-target buttons do not take `target`/`ground`; unknown buttons remain an error.
        assert_eq!(
            control_unit_bar(&mut bar, &mut host, &[medicine, chief], 519, None, None),
            Ok(())
        );
        let err = control_unit_bar(&mut bar, &mut host, &[medicine], 9999, None, None).unwrap_err();
        // The whole message, without runs of spaces from the line continuation.
        assert_eq!(
            err,
            "Button 9999 does not exist in the unit bar (511-515, 519, 405, 406, 430, 431, 432, \
             433, 449-453, 485, 486, abilities 543, 544, 545, 549, 552, 557, 559, 560, 561, 563, \
             counter-upgrades 3370-3377, camouflage 434, 435, dig in 437, 438, healing 458, 459, 554, \
             555, 564, ghost 436, banish 439, 547, 550, 553, 558, 562, animals 441, 442, 443, spells \
             480, 481, 482, 484, 541, special attacks 454-457, 542, 546, 548, 551, 556, robbing \
             540)"
        );
        assert!(!err.contains("  "), "{err}");
    }

    #[test]
    fn spell_special_info_follows_the_real_commands() {
        // The examples in `docs/viewer-control.md` (section "Spells and special attacks") are reachable
        // states of the viewer test world: real commands via `control_unit_bar`, then object ticks
        // (two `step_once` each); the query changes nothing.
        use crate::game::testworld::*;
        use crate::game::unit_bar::{UnitBar, control_unit_bar};
        use nml_control::{SimBombermanInfo, SimKillerInfo, SimSpecialMoveInfo};
        let ticks = |host: &mut SimHost, n: usize| {
            for _ in 0..2 * n {
                host.step_once();
            }
        };
        // Example 1: explosion (480) of the medicine man, dynamite (454) of the patriot at an
        // enemy building, kill (456) of the trapper.
        let mut host = host();
        for u in [
            EXPLOSION_UPGRADE,
            DYNAMITE_BUILDING_UPGRADE,
            TRAPPER_KILL_UPGRADE,
        ] {
            unlock(&mut host, 0, u);
        }
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let patriot = spawn(&mut host, PATRIOT, 0, (30, 40));
        let trapper = spawn(&mut host, TRAPPER, 0, (45, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (20, 12));
        let victim = spawn(&mut host, COLONIST, 1, (47, 12));
        let post = spawn(&mut host, ALARM_POST, 1, (32, 42));
        assert_eq!(
            [medicine.0, patriot.0, trapper.0, enemy.0, victim.0, post.0],
            [1, 65_537, 131_073, 2049, 67_585, 133_121]
        );
        for id in [medicine, patriot, trapper] {
            host.state
                .object_mut(id, false)
                .unwrap()
                .attributes
                .set_current_value(15, 100, 0);
        }
        let mut bar = UnitBar::default();
        for (unit, button, target) in [
            (medicine, 480, enemy),
            (patriot, 454, post),
            (trapper, 456, victim),
        ] {
            assert_eq!(
                control_unit_bar(&mut bar, &mut host, &[unit], button, Some(target.0), None),
                Ok(())
            );
        }
        ticks(&mut host, 13);
        let sum = (host.checksum(), nml_sim::save::checksum(&host.state));
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!(
            (s.art, s.button, s.cost, s.mana, s.casting),
            (1, 480, 80, 100, true)
        );
        assert_eq!((s.target, s.corpse), (Some([2624.0, 0.0, 1600.0]), None));
        assert_eq!(
            info(&host, patriot).bomberman.unwrap(),
            SimBombermanInfo {
                target: Some(post.0),
                extra_energy: 70,
                return_point: [124_928, 0, 165_888],
                busy: true,
                mana: 100,
            }
        );
        assert_eq!(
            info(&host, trapper).killer.unwrap(),
            SimKillerInfo {
                target: Some(victim.0),
                extra_energy: 60,
                near_damage: 300,
                far_damage: 0,
                mana: 101,
            }
        );
        assert!(host.projectile_infos().is_empty());
        assert_eq!((host.checksum(), nml_sim::save::checksum(&host.state)), sum);
        // Explosion: effect after 24 object ticks, remaining 400 of 500 ms.
        ticks(&mut host, 11);
        let list = host.projectile_infos();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(
            (e.kind.as_str(), e.remaining_ms, e.flight_ms, e.shooter),
            ("explosion", 400, 500, Some(medicine.0))
        );
        assert_eq!(
            (e.target, e.target_pos, e.pulse),
            (None, [2624.0, 0.0, 1600.0], None)
        );
        // Dynamite: the charge lies after 66 object ticks, the demolition expert is no longer busy
        // and has paid 70 of 100 mana (plus regeneration).
        ticks(&mut host, 42);
        let list = host.projectile_infos();
        assert_eq!(list.len(), 1);
        let d = &list[0];
        assert_eq!(
            (
                d.kind.as_str(),
                d.remaining_ms,
                d.flight_ms,
                d.shooter,
                d.target
            ),
            ("dynamite", 9900, 10_000, Some(patriot.0), Some(post.0))
        );
        assert_eq!((d.target_pos, d.pulse), ([4068.6875, 0.0, 5348.6875], None));
        let b = info(&host, patriot).bomberman.unwrap();
        assert_eq!((b.busy, b.mana, b.extra_energy), (false, 30, 70));

        // Example 2: storm (481) on a building and damage building (551) of the smith.
        let mut host = crate::game::testworld::host();
        for u in [STORM_UPGRADE, DAMAGE_BUILDING_UPGRADE] {
            unlock(&mut host, 0, u);
        }
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let smith = spawn(&mut host, SMITH, 0, (40, 25));
        let post = spawn(&mut host, ALARM_POST, 1, (43, 25));
        let storm_post = spawn(&mut host, ALARM_POST, 1, (16, 12));
        assert_eq!(
            [medicine.0, smith.0, post.0, storm_post.0],
            [1, 65_537, 2049, 67_585]
        );
        for id in [medicine, smith] {
            host.state
                .object_mut(id, false)
                .unwrap()
                .attributes
                .set_current_value(15, 100, 0);
        }
        let mut bar = UnitBar::default();
        assert_eq!(
            control_unit_bar(
                &mut bar,
                &mut host,
                &[medicine],
                481,
                Some(storm_post.0),
                None
            ),
            Ok(())
        );
        assert_eq!(
            control_unit_bar(&mut bar, &mut host, &[smith], 551, Some(post.0), None),
            Ok(())
        );
        ticks(&mut host, 2);
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!((s.art, s.button, s.casting, s.mana), (2, 481, true, 100));
        assert_eq!(s.target, Some([2112.0, 0.0, 1600.0]));
        ticks(&mut host, 24);
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!(
            (s.casting, s.mana, s.target),
            (true, 20, Some([2112.0, 0.0, 1600.0]))
        );
        assert_eq!(
            info(&host, smith).special_move.unwrap(),
            SimSpecialMoveInfo {
                busy: true,
                mode: 1,
                mana: 41,
            }
        );
        let list = host.projectile_infos();
        assert_eq!(list.len(), 1);
        let st = &list[0];
        assert_eq!(
            (
                st.kind.as_str(),
                st.remaining_ms,
                st.flight_ms,
                st.shooter,
                st.target
            ),
            ("storm", 6700, 7000, Some(medicine.0), None)
        );
        assert_eq!((st.target_pos, st.pulse), ([2112.0, 0.0, 1600.0], Some(0)));
        // The counter alternates between 1 and 0 with every object tick (five object ticks later it is at 1).
        ticks(&mut host, 5);
        let st = host.projectile_infos().remove(0);
        assert_eq!((st.remaining_ms, st.pulse), (6200, Some(1)));
        // After the end of the spell the target point is absent, the storm continues; the smith is done
        // (mode 1 remains).
        ticks(&mut host, 15);
        let s = info(&host, medicine).spell.unwrap();
        assert_eq!((s.casting, s.target), (false, None));
        assert_eq!(host.projectile_infos().len(), 1);
        ticks(&mut host, 10);
        let m = info(&host, smith).special_move.unwrap();
        assert_eq!((m.busy, m.mode), (false, 1));
    }

    #[test]
    fn look_follows_the_asking_player() {
        // `sim_query` carries no player; the "asking player" is the one chosen in the viewer
        // (`active_player`). The alarm post (re/masking 7, D-235) shows itself
        // semi-transparent to the owner and only the tree, opaque, to the enemy.
        use crate::game::sim_host::MASKED_ALPHA;
        use crate::game::testworld::{ALARM_POST, ARCHER, MASK_UPGRADE, host, spawn, unlock};
        use nml_control::SimLookInfo;
        let mut host = host();
        let post = spawn(&mut host, ALARM_POST, 0, (20, 20));
        // Without 2924 the archer has no module 25: stealthed no alpha (D-267, like the assassin).
        let plain = spawn(&mut host, ARCHER, 0, (34, 20));
        unlock(&mut host, 0, MASK_UPGRADE);
        let archer = spawn(&mut host, ARCHER, 0, (30, 20));
        // `EVERYTIMETARNED`: the post is stealthed in the first tick.
        run_ticks(&mut host, 1);
        assert_ne!(
            host.state.object(post, false).unwrap().status & status::MASKED,
            0
        );
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        host.active_player = 0;
        assert_eq!(
            info(&host, post).look,
            Some(SimLookInfo {
                anim: None,
                alpha: MASKED_ALPHA,
            })
        );
        host.active_player = 1;
        assert_eq!(
            info(&host, post).look,
            Some(SimLookInfo {
                anim: Some(1),
                alpha: 1.0,
            })
        );
        // If the owner shares its vision with 1, 1 also sees the post (not only the tree).
        host.state.diplomacy.view[0] |= 1 << 1;
        assert_eq!(
            info(&host, post).look,
            Some(SimLookInfo {
                anim: None,
                alpha: MASKED_ALPHA,
            })
        );
        host.state.diplomacy.view[0] &= !(1 << 1);
        // Not stealthed: no presentation, for no player.
        for viewer in [0, 1] {
            host.active_player = viewer;
            assert!(info(&host, archer).look.is_none());
        }
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // A stealthed unit with module 25 is semi-transparent for every viewer, without a substitute
        // animation.
        host.state.object_mut(archer, false).unwrap().status |= status::MASKED;
        for viewer in [0, 1] {
            host.active_player = viewer;
            assert_eq!(
                info(&host, archer).look,
                Some(SimLookInfo {
                    anim: None,
                    alpha: MASKED_ALPHA,
                })
            );
        }
        host.state.object_mut(plain, false).unwrap().status |= status::MASKED;
        for viewer in [0, 1] {
            host.active_player = viewer;
            assert!(info(&host, plain).look.is_none());
        }
    }

    #[test]
    fn move_json_without_formation_and_spread_uses_the_defaults() {
        // `formation` and `spread` are optional in JSON (re/groups-formations 12.6).
        let command: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"Move": {"units": [1], "x": 4096, "z": 8192, "target": -1, "direction": -819200, "flags": 0}}}"#,
        )
        .unwrap();
        assert_eq!(
            command.kind,
            CommandKind::Move {
                units: vec![ObjectId(1)],
                x: 4096,
                z: 8192,
                target: -1,
                formation: 0,
                direction: nml_sim::motion::NO_DIRECTION,
                spread: nml_sim::command::NO_SPREAD,
                flags: 0,
            }
        );
        let command: nml_sim::command::Command = serde_json::from_str(
            r#"{"player": 0, "kind": {"Move": {"units": [1], "x": 0, "z": 0, "target": -1, "formation": 3, "direction": -819200, "spread": 6144, "flags": 0}}}"#,
        )
        .unwrap();
        assert!(matches!(
            command.kind,
            CommandKind::Move {
                formation: 3,
                spread: 6144,
                ..
            }
        ));
    }

    #[test]
    fn combat_info_leaves_the_checksum_unchanged() {
        let (mut host, sabre, warrior) = combat_host();
        let observer = spawn(&mut host, OBSERVER, 0, (20, 20));
        host.submit_own(CommandKind::Attack {
            units: vec![sabre],
            target: warrior,
            pos: None,
            mods: 0,
        });
        for _ in 0..40 {
            host.step_once();
        }
        let before = checksum(&host.state);
        let q = host.query(Some(0), None, None, 100);
        let c = q
            .objects
            .iter()
            .find_map(|o| o.combat.clone())
            .expect("combat info");
        assert!(c.attack.is_some() && c.hp[1] > 0, "{c:?}");
        // The commanded attack is running (no automatic target, strike in progress).
        let a = c.attack.expect("attack module");
        assert_eq!(
            (a.status, a.target, a.automatic),
            (2, Some(warrior.0), false),
            "{a:?}"
        );
        // The warrior has been hit: hit points below the maximum, attacker noted,
        // dying module idle.
        let foe = info(&host, warrior)
            .combat
            .expect("combat info of the warrior");
        assert_eq!(foe.hp[1], 160);
        assert!(foe.hp[0] < 160 && foe.hp_fraction < 100_000, "{foe:?}");
        assert_eq!(foe.last_attacker, Some(sabre.0));
        assert_eq!(foe.dying.map(|d| d.status), Some(0));
        // Objects without combat have no section.
        assert!(info(&host, observer).combat.is_none());
        assert_eq!(checksum(&host.state), before);
    }

    /// Archer, warrior, goat and hunter with the values of the combat test world of `nml-sim`
    /// (modules 10, 17, 19, 20, 21, 22; time markers of the shot animation 5).
    fn ranged_host() -> (SimHost, ObjectId, ObjectId, ObjectId, ObjectId) {
        const ARCHER: u32 = 1402;
        const GOAT: u32 = 3405;
        const HUNTER: u32 = 1703;
        let shooter = |guid: u32, name: &str, hunt: bool| {
            let mut s = format!(
                "GUID {guid}
NAME {name}
VALUEID 100 6 1 0
{WALK}PROPID 16
VALUEID 170002 6 1 3
PROPID 17
VALUEID 180001 6 1 10240
PROPID 19
VALUEID 200001 6 1 0
PROPID 10
VALUEID 110001 6 1 2146560
VALUEID 110002 1 1 5
VALUEID 110007 7 1 pfeil
"
            );
            if hunt {
                s += "PROPID 22
VALUEID 230001 6 1 12
VALUEID 230002 6 1 13
VALUEID 230003 1 1 14 15
";
            }
            s
        };
        let types = fighter_types()
            + &shooter(ARCHER, "WID_Bogenschuetze", false)
            + &shooter(HUNTER, "PRI_Bogenschuetze", true)
            + &format!(
                "GUID {GOAT}
NAME ANI_Schneeziege
VALUEID 100 6 1 1024
{WALK}PROPID 16
VALUEID 170002 6 1 3
PROPID 20
VALUEID 210001 6 1 12288
VALUEID 210002 6 1 2
PROPID 21
"
            );
        let mut rules = nml_sim::rules::Rules::synthetic(&types, 64);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        rules.defaults.insert(
            WARRIOR,
            set(&[(14, 160), (16, 183), (18, 40), (19, 9), (21, 8)]),
        );
        for g in [ARCHER, HUNTER] {
            rules.defaults.insert(
                g,
                set(&[
                    (14, 155),
                    (16, 200),
                    (17, 20),
                    (18, 40),
                    (20, 11),
                    (21, 28),
                    (31, 60),
                    (34, 4),
                ]),
            );
            rules.set_anim_labels(g, 5, &[(1, 2333), (2, 833), (3, 700)]);
            rules
                .mesh_names
                .insert(g, ["pfeil".to_string()].into_iter().collect());
        }
        rules.defaults.insert(
            GOAT,
            set(&[(14, 40), (16, 227), (18, 32), (19, 3), (24, 50)]),
        );
        let setup = nml_sim::setup::MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: nml_sim::ki::KiSetup::default(),
            script: nml_sim::script::ScriptSetup::default(),
            ..nml_sim::setup::MatchSetup::default()
        };
        let mut host =
            SimHost::new(rules, setup, crate::game::options::DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host.state.players[0].control = nml_sim::state::Control::Human;
        nml_sim::fog::reveal_map(&mut host.state, -1);
        let archer = spawn(&mut host, ARCHER, 0, (10, 10));
        let warrior = spawn(&mut host, WARRIOR, 1, (16, 10));
        let goat = spawn(&mut host, GOAT, 8, (10, 30));
        let hunter = spawn(&mut host, HUNTER, 0, (4, 30));
        (host, archer, warrior, goat, hunter)
    }

    #[test]
    fn ranged_info_leaves_the_checksum_unchanged() {
        let (mut host, archer, warrior, goat, hunter) = ranged_host();
        host.submit_own(CommandKind::Attack {
            units: vec![archer],
            target: warrior,
            pos: None,
            mods: 0,
        });
        host.submit_own(CommandKind::Hunt {
            animal: goat,
            units: vec![hunter],
            gut: false,
        });
        let mut seen_projectile = false;
        for _ in 0..200 {
            host.step_once();
            let before = checksum(&host.state);
            let q = host.query(Some(0), None, None, 100);
            seen_projectile |= !host.section(&[]).projectiles.is_empty();
            assert!(q.objects.iter().any(|o| o.ranged.is_some()));
            assert_eq!(
                checksum(&host.state),
                before,
                "query changes the simulation"
            );
        }
        assert!(seen_projectile, "an arrow in flight");
        let a = info(&host, archer).ranged.expect("ranged combat");
        assert_eq!(a.max_dist, 28 << 10);
        assert!(a.shoot.is_some());
        let g = info(&host, goat);
        assert!(
            g.animal.as_ref().is_some_and(|x| x.shy && !x.aggressive),
            "{:?}",
            g.animal
        );
        assert_eq!(g.edible.map(|e| e.meat[1]), Some(50));
        let h = info(&host, hunter).gatherer.expect("hunt");
        assert_eq!((h.module, h.kind.as_str()), (22, "hunting"));
        assert_eq!(h.prey, Some(goat.0));
        // The hunt uses `m_TargetHID`: `source` shows the same target as `prey`.
        assert_eq!(h.source, h.prey);
        let json = serde_json::to_string(&host.query(Some(0), None, None, 100)).unwrap();
        assert!(json.contains("\"ranged\""));
        let section = serde_json::to_string(&host.section(&[])).unwrap();
        assert!(section.contains("\"projectiles\""));
    }

    #[test]
    fn the_combat_line_follows_the_selection_then_the_hover_and_leaves_the_checksum() {
        let (mut host, sabre, warrior) = combat_host();
        let observer = spawn(&mut host, OBSERVER, 0, (20, 20));
        host.submit_own(CommandKind::SetBehavior {
            units: vec![sabre],
            mode: 3,
        });
        for _ in 0..3 {
            host.step_once();
        }
        let before = checksum(&host.state);
        let line = host.combat_line(&[sabre, warrior], Some(warrior));
        assert_eq!(
            line.as_deref(),
            Some("Combat: Stance Offensive · HP 190/190 · Rank 0 · Experience 0 %")
        );
        // Without a selection the object under the mouse; without combat values no line.
        let hovered = host.combat_line(&[], Some(warrior)).expect("warrior");
        assert!(hovered.contains("HP 160/160"), "{hovered}");
        assert_eq!(host.combat_line(&[observer], Some(warrior)), None);
        assert_eq!(host.combat_line(&[], None), None);
        assert_eq!(checksum(&host.state), before);
    }

    #[test]
    fn sim_build_places_a_construction_site_like_the_placement_mode() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        let r = host
            .control_build(HOUSE, [1000.0, 1000.0], 0, &[worker], false)
            .unwrap();
        // Snapped like the preview: hull -100...100, minimum corner 896 -> position 996.
        assert_eq!((r.pos[0], r.pos[2], r.rotation), (996.0, 996.0, 0.0));
        assert_eq!(r.rect, Some([7, 7, 8, 8]));
        assert_eq!(
            (r.workers.clone(), r.affordable, r.append),
            (vec![worker.0], true, false)
        );
        assert_eq!(r.step, host.state.steps);
        let [CommandKind::PlaceBuilding { guid, workers, .. }] = &logged(&mut host)[..] else {
            panic!("construction expected");
        };
        assert_eq!((*guid, workers.clone()), (HOUSE, vec![worker]));
        // The construction site stands, wood paid, "in progress" counted.
        let site = host
            .state
            .objects()
            .find(|o| o.guid == HOUSE)
            .map(|o| o.id)
            .expect("construction site");
        let b = info(&host, site).building.expect("building section");
        assert!(!b.finished && b.under_construction && !b.dead, "{b:?}");
        assert_eq!((b.build_time, b.render_bits), (2000, 4));
        assert!(b.build_rest > 0 && b.progress < 1.0, "{b:?}");
        let players = host.query(None, None, None, 1).players;
        assert_eq!(players[0].resources[resource::WOOD], 850);
        assert_eq!(players[0].pending.get(&HOUSE), Some(&1));
        // The worker sets off for the construction site.
        let mut target = None;
        for _ in 0..40 {
            host.step_once();
            target = info(&host, worker).construct.and_then(|c| c.building);
            if target.is_some() {
                break;
            }
        }
        assert_eq!(target, Some(site.0));
        // Shift: queued.
        let r = host
            .control_build(HOUSE, [2000.0, 1000.0], 1, &[worker], true)
            .unwrap();
        assert!(r.append);
        assert!(matches!(
            &logged(&mut host)[..],
            [CommandKind::Queue { cyclic: false, .. }]
        ));
    }

    #[test]
    fn sim_build_explains_why_it_does_not_build() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let foreign = spawn(&mut host, COLONIST, 1, (12, 12));
        spawn(&mut host, HQ, 0, (30, 30));
        let err = |host: &mut SimHost, guid, to, units: &[ObjectId]| {
            host.control_build(guid, to, 0, units, false).unwrap_err()
        };
        let on_hq = [world(30), world(30)];
        let free = [1000.0, 1000.0];
        let e = err(&mut host, HOUSE, on_hq, &[worker]);
        assert!(e.starts_with("invalid site for 1001"), "{e}");
        assert!(e.ends_with("tiles [29, 29, 30, 30]"), "{e}");
        assert_eq!(
            err(&mut host, MINE, free, &[worker]),
            "building blocked: prerequisite for 1004 missing"
        );
        assert_eq!(
            err(&mut host, HQ, free, &[worker]),
            "building blocked: 1000 exists only once"
        );
        assert_eq!(
            err(&mut host, COLONIST, free, &[worker]),
            "building blocked: type 1100 is not a building"
        );
        assert_eq!(
            err(&mut host, HOUSE, free, &[foreign]),
            "no own workers (player 0)"
        );
        assert_eq!(
            err(&mut host, HOUSE, free, &[]),
            "no own workers (player 0)"
        );
        assert!(logged(&mut host).is_empty());
        // Rotation lock: the build cursor never rotates the gold mine (flags & 0x10000000).
        spawn(&mut host, STORE, 0, (40, 40));
        let e = host
            .control_build(MINE, free, 1, &[worker], false)
            .unwrap_err();
        assert_eq!(
            e,
            "type 1004 cannot be rotated (rotation lock of the build cursor, building flags & \
             0x10000000): rotation_steps must be 0"
        );
        assert!(logged(&mut host).is_empty());
        // Too little wood is not an error: the simulation reports the shortage (message 1, wood).
        host.state.players[0].resources_fx[resource::WOOD] = 0;
        let r = host
            .control_build(HOUSE, free, 0, &[worker], false)
            .unwrap();
        assert!(!r.affordable);
        host.step_once();
        let notices = host.section(&[]).notices;
        assert_eq!(
            notices.last().map(|n| n.id),
            Some(message::NO_WOOD),
            "{notices:?}"
        );
        assert!(host.state.objects().all(|o| o.guid != HOUSE));
    }

    #[test]
    fn sim_build_sends_the_ground_height_so_the_site_flag_stands_above_ground() {
        use crate::game::rally_flag::{RALLY_POINT_BUTTON, rally_flag};
        use nml_formats::gifcon::ToolbarButton;
        use nml_sim::rules::TileTerrain;
        use nml_sim::world::grid_to_world;
        // Raised terrain (construction site 1305: terrain 200, node `ausgang` 14 high),
        // house with rally-point button 447 so that it gets a flag.
        let mut host = host();
        for v in &mut host.rules.terrain.vertices {
            v.position.y = 200.0;
        }
        host.rules.tile_terrain = TileTerrain::build(&host.rules.terrain, &host.rules.map);
        let mut shape = host.rules.building_shapes[&HOUSE].clone();
        shape
            .helper_nodes
            .insert("ausgang".into(), [0.0, 14.0, -150.0]);
        host.rules.building_shapes.insert(HOUSE, shape);
        host.rules
            .toolbar
            .menus
            .entry(HOUSE as i32)
            .or_default()
            .push(ToolbarButton {
                id: 9,
                msg: RALLY_POINT_BUTTON,
                ..ToolbarButton::default()
            });
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        let r = host
            .control_build(HOUSE, [1000.0, 1000.0], 0, &[worker], false)
            .unwrap();
        // The command carries the ground height at the target point like the mouse point of the original
        // (`0x004CCF00`).
        assert_eq!(r.pos, [996.0, 200.0, 996.0]);
        host.step_once();
        let site = host
            .state
            .objects()
            .find(|o| o.guid == HOUSE)
            .map(|o| o.id)
            .expect("construction site");
        let o = host.state.object(site, false).unwrap();
        let cell = host.rules.map.cell;
        let ModuleKind::Building(b) = &o.module(building::INDEX).unwrap().kind else {
            panic!("building module expected");
        };
        assert!(b.is_build_building, "construction site");
        // Rally point = terrain height of the building plus node height (`PropertyBuilding_Init` `0x0050B150`).
        assert_eq!(o.grid.y, Fx((200 << 12) / cell[1]));
        assert_eq!(b.meeting_point.y, Fx(o.grid.y.0 + (14 << 12) / cell[1]));
        let f = rally_flag(&host.state, &host.rules, &[site], 0)
            .expect("flag at the construction site");
        assert_eq!(f.position, Vec3::from(grid_to_world(b.meeting_point, cell)));
        assert_eq!(f.position.y, 214.0);
        let ground = host.rules.ground_height(f.position.x, f.position.z);
        assert!(
            f.position.y > ground,
            "flag {} above ground {ground}",
            f.position.y
        );
        // Below the water level and outside the map the water height applies.
        host.rules.terrain.params.water_level = 250.0;
        assert_eq!(ground_below(&host.rules.terrain, 1000.0, 1000.0), 250.0);
        assert_eq!(ground_below(&host.rules.terrain, -5.0, 1000.0), 250.0);
    }

    #[test]
    fn sim_action_reports_the_button_and_the_command_of_the_target_table() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        // Worker on tree (without `to`: position of the tree) -> 400 Chop wood `0x06`.
        let r = host
            .control_action(&[worker], Some(tree), None, None)
            .unwrap();
        assert_eq!(
            (r.button, r.action.as_str(), r.command.as_str(), r.code),
            (400, "Chop wood", "ChopWood", 6)
        );
        assert!(!r.queued && !r.cyclic);
        assert_eq!(r.kind["ChopWood"]["tree"], tree.0);
        assert_eq!(
            logged(&mut host),
            [CommandKind::ChopWood {
                tree,
                units: vec![worker]
            }]
        );
        // The lumberjack has the tree as source.
        let mut source = None;
        for _ in 0..20 {
            host.step_once();
            source = info(&host, worker).gatherer.and_then(|g| g.source);
            if source.is_some() {
                break;
            }
        }
        let g = info(&host, worker).gatherer.expect("gatherer");
        assert_eq!(
            (g.module, g.kind.as_str(), source),
            (8, "lumber", Some(tree.0))
        );
        // With a further, free gatherer module in front (pioneer: gold before wood) the query shows
        // the active one; if all are free, the first in module order.
        let mut pioneer = host.state.object(worker, false).unwrap().clone();
        let lumber = pioneer.module(gather::LUMBERING.index).unwrap().clone();
        pioneer.modules[gather::MINING.index] = Some(Module {
            base: lumber.base.clone(),
            kind: ModuleKind::Mining(Box::default()),
        });
        assert_eq!(host.gatherer_info(&pioneer).unwrap().kind, "lumber");
        let mut idle = lumber;
        if let ModuleKind::Lumber(l) = &mut idle.kind {
            l.gather = gather::Gather::default();
        }
        pioneer.modules[gather::LUMBERING.index] = Some(idle);
        assert_eq!(host.gatherer_info(&pioneer).unwrap().kind, "mining");
        let t = info(&host, tree).tree.expect("tree");
        assert_eq!((t.wood, t.falling), (200, false));
        // Ctrl: wrapped cyclically (`0x7C`), the command name remains the inner one.
        let r = host
            .control_action(&[worker], Some(tree), None, Some(true))
            .unwrap();
        assert_eq!(
            (r.command.as_str(), r.queued, r.cyclic),
            ("ChopWood", true, true)
        );
        host.step_once();
        // Building + ground -> 447 rally point; afterwards it is in the building section.
        let to = [world(7), world(9)];
        let r = host.control_action(&[hq], None, Some(to), None).unwrap();
        assert_eq!(
            (r.button, r.command.as_str(), r.code),
            (447, "SetRallyPoint", 0x27)
        );
        host.step_once();
        let b = info(&host, hq).building.expect("building");
        assert_eq!(b.meeting_point_fx, [7 << 12 | 0x800, 9 << 12 | 0x800]);
        assert_eq!(b.meeting_point, to);
        let q = info(&host, hq).quarter.expect("quarters");
        assert_eq!((q.occupied, q.max, q.occupants.len()), (0, 5, 0));
        // Errors: empty selection, foreign first object, missing target, no action.
        let foreign = spawn(&mut host, HOUSE, 1, (40, 40));
        let e = |host: &mut SimHost, units: &[ObjectId], target| {
            host.control_action(units, target, None, None).unwrap_err()
        };
        assert_eq!(e(&mut host, &[], Some(tree)), "no units (selection empty)");
        assert_eq!(
            e(&mut host, &[foreign], Some(tree)),
            format!("no action: {} does not belong to player 0", foreign.0)
        );
        assert_eq!(
            e(&mut host, &[worker], Some(ObjectId(0x7F_0001))),
            format!("target {} missing", 0x7F_0001)
        );
        // Without a matching entry of `targets.ini` no action.
        host.rules.targets.clear();
        assert_eq!(
            e(&mut host, &[worker], Some(tree)),
            format!("no action for {} (targets.ini)", tree.0)
        );
        assert_eq!(
            host.control_action(&[worker], None, Some(to), None)
                .unwrap_err(),
            "no action for the ground (targets.ini)"
        );
        assert_eq!(
            host.control_action(&[worker], None, Some([-5.0, 100.0]), None)
                .unwrap_err(),
            "no action for the ground outside the map"
        );
        assert!(logged(&mut host).is_empty());
    }

    #[test]
    fn sim_menu_lists_entries_with_costs_and_lock_reasons() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let menu = host.control_menu(Some(&[worker]), None).unwrap();
        assert_eq!(
            (menu.building, menu.workers.clone()),
            (None, vec![worker.0])
        );
        let entry = |m: &SimMenuResult, g| m.entries.iter().find(|e| e.guid == g).unwrap().clone();
        let house = entry(&menu, HOUSE);
        assert_eq!(
            (house.name.as_str(), house.kind.as_str()),
            ("SPA_Wohnhaus", "BUILD")
        );
        assert_eq!(entry(&menu, HQ).kind, "ONLYONCE");
        assert_eq!(house.costs.map(|c| c[resource::WOOD]), Some(150));
        assert!(house.affordable && house.blocked.is_none());
        assert_eq!(
            entry(&menu, MINE).blocked.as_deref(),
            Some("prerequisite for 1004 missing")
        );
        assert_eq!(
            entry(&menu, HQ).blocked.as_deref(),
            Some("1000 exists only once")
        );
        assert!(entry(&menu, STORE).blocked.is_none());
        // Production list with research (submenu 600 included) and queue.
        host.control_train(hq, COLONIST).unwrap();
        host.step_once();
        let menu = host.control_menu(None, Some(hq)).unwrap();
        assert_eq!(menu.building, Some(hq.0));
        let guids: Vec<u32> = menu.entries.iter().map(|e| e.guid).collect();
        assert_eq!(guids, [COLONIST, 2806, 3354]);
        assert_eq!(
            menu.entries
                .iter()
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>(),
            ["MAKE", "UPGRADE", "UPGRADE"]
        );
        assert!(menu.entries.iter().all(|e| e.blocked.is_none()));
        assert_eq!(menu.queue.len(), 1);
        assert_eq!((menu.queue[0].index, menu.queue[0].guid), (0, COLONIST));
        // Foreign building, no workers.
        let foreign = spawn(&mut host, HQ, 1, (50, 50));
        assert_eq!(
            host.control_menu(None, Some(foreign)).unwrap_err(),
            format!("{} is not a building of player 0", foreign.0)
        );
        assert_eq!(
            host.control_menu(Some(&[]), None).unwrap_err(),
            "no own workers (player 0)"
        );
    }

    #[test]
    fn train_cancel_and_sell_check_queue_and_ownership() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        // Research is accepted like training; a missing prerequisite blocks it.
        host.rules.toolbar.menus.get_mut(&1000).unwrap()[1].required[1] = 4711;
        assert_eq!(
            host.control_train(hq, 2806).unwrap_err(),
            "training blocked: prerequisite for 2806 missing"
        );
        host.rules.toolbar.menus.get_mut(&1000).unwrap()[1].required[1] = 0;
        let r = host.control_train(hq, 2806).unwrap();
        assert_eq!(r["affordable"], true);
        host.step_once();
        assert_eq!(host.control_cancel(hq, 0).unwrap()["guid"], 2806);
        host.step_once();
        assert!(info(&host, hq).building.unwrap().production.is_none());
        let r = host.control_train(hq, COLONIST).unwrap();
        assert_eq!(r["affordable"], true);
        host.control_train(hq, COLONIST).unwrap();
        for _ in 0..4 {
            host.step_once();
        }
        let b = info(&host, hq).building.expect("building");
        let p = b.production.expect("running order");
        assert_eq!(p.make_guid, COLONIST);
        assert!(p.total > 0 && p.rest <= p.total, "{p:?}");
        assert_eq!(b.queue, [COLONIST]);
        assert!(b.finished && !b.under_construction);
        assert_eq!(
            host.control_cancel(hq, 2).unwrap_err(),
            "no order with index 2 (queue length: 2)"
        );
        assert_eq!(host.control_cancel(hq, 1).unwrap()["guid"], COLONIST);
        assert_eq!(
            logged(&mut host),
            [CommandKind::CancelProduction {
                building: hq,
                index: 1,
                arg: 0
            }]
        );
        assert!(info(&host, hq).building.unwrap().queue.is_empty());
        // Sell: foreign and dead buildings are listed under `skipped`, without an own one an error.
        let foreign = spawn(&mut host, HQ, 1, (50, 50));
        assert_eq!(
            host.control_sell(&[foreign]).unwrap_err(),
            "no own living building of player 0 to sell"
        );
        let r = host.control_sell(&[hq, foreign]).unwrap();
        assert_eq!(r["buildings"], serde_json::json!([hq.0]));
        assert_eq!(r["skipped"], serde_json::json!([foreign.0]));
        host.step_once();
        assert!(info(&host, hq).building.unwrap().dead);
        assert!(host.control_sell(&[hq]).is_err());
        assert!(host.control_cancel(foreign, 0).is_err());
        assert!(host.control_train(foreign, COLONIST).is_err());
    }

    #[test]
    fn sim_upgrade_sends_the_console_commands_for_any_slot() {
        use crate::game::unit_bar::testkit::{COLONIST, host};
        use nml_sim::upgrades::type_upgrade_percent;
        let mut host = host();
        let hp = |host: &SimHost, slot: u8| type_upgrade_percent(&host.state, slot, COLONIST, 14);
        let before = hp(&host, 0);
        // The start applies the time-of-day upgrades: the lists are not empty.
        let base = host.state.applied_upgrades[0].clone();
        let base8 = host.state.applied_upgrades[8].clone();
        // Applying to the own slot: command from the active player, effect at the next tick.
        let r = host.control_upgrade(0, 3375, false).unwrap();
        assert_eq!(r["step"], host.state.steps);
        assert_eq!(host.state.applied_upgrades[0], base);
        let log = host.log.len();
        host.step_once();
        let sent = &host.log[log];
        assert_eq!(sent.command.player, 0);
        assert_eq!(
            sent.command.kind,
            CommandKind::ApplyUpgrade {
                slot: 0,
                guid: 3375
            }
        );
        assert_eq!(
            [&base[..], &[3375]].concat(),
            host.state.applied_upgrades[0]
        );
        assert_eq!(hp(&host, 0), before - 40);
        // Slot 8 (neutrals): the sender remains player 0; the table is written but never read
        // (W-213).
        host.control_upgrade(8, 3375, false).unwrap();
        let log = host.log.len();
        host.step_once();
        assert_eq!(host.log[log].command.player, 0);
        assert_eq!(
            [&base8[..], &[3375]].concat(),
            host.state.applied_upgrades[8]
        );
        assert!(host.state.type_upgrades[8].contains_key(&COLONIST));
        assert_eq!(hp(&host, 8), 0);
        // Removing: percent and list as before.
        host.control_upgrade(0, 3375, true).unwrap();
        host.step_once();
        assert_eq!(host.state.applied_upgrades[0], base);
        assert_eq!(hp(&host, 0), before);
        // Apply all (guid < 0) goes as one command; removing needs a GUID > 0.
        host.control_upgrade(1, -1, false).unwrap();
        assert_eq!(
            logged(&mut host),
            [CommandKind::ApplyUpgrade { slot: 1, guid: -1 }]
        );
        assert!(host.control_upgrade(1, -1, true).is_err());
        assert!(host.control_upgrade(1, 0, true).is_err());
    }

    #[test]
    fn sim_trade_and_sim_tribute_send_as_the_given_player() {
        use crate::game::testworld::{self, CART};
        let mut host = testworld::host();
        let mine = testworld::spawn(&mut host, CART, 0, (10, 10));
        let theirs = testworld::spawn(&mut host, CART, 1, (14, 10));
        // Trade: the command carries the given sender (player 1), not the active player 0;
        // the simulation rejects it (slot 3 has no trading building), the carts stay idle.
        let r = host.control_trade(1, &[mine, theirs], 2, 3).unwrap();
        assert_eq!(r["step"], host.state.steps);
        assert_eq!(host.pending_commands().len(), 1);
        let log = host.log.len();
        host.step_once();
        assert_eq!(
            host.log[log].command,
            Command {
                player: 1,
                kind: CommandKind::TradeWithPlayer {
                    ware: 2,
                    target: 3,
                    units: vec![mine, theirs],
                },
            }
        );
        assert_eq!(info(&host, mine).trade.unwrap().ware, 0);
        // Without units (empty selection) an error, nothing is sent.
        assert!(host.control_trade(0, &[], 0, 1).is_err());
        assert!(host.pending_commands().is_empty());

        // Tribute: the giver is the sender; gold 1000 -> 800, the recipients 1 and 2 each +100.
        let gold = |host: &SimHost, slot: usize| host.state.resource(slot, resource::GOLD);
        let before = [gold(&host, 0), gold(&host, 1), gold(&host, 2)];
        let r = host.control_tribute(0, 0b110, 0, 100).unwrap();
        assert_eq!(r["step"], host.state.steps);
        assert_eq!(
            logged(&mut host),
            [CommandKind::Tribute {
                from: 0,
                recipients: 0b110,
                resource: 0,
                amount: 100
            }]
        );
        assert_eq!(
            [gold(&host, 0), gold(&host, 1), gold(&host, 2)],
            [before[0] - 200, before[1] + 100, before[2] + 100]
        );
        // An amount <= 0 is rejected by the simulation: nothing is booked.
        host.control_tribute(0, 0b10, 0, 0).unwrap();
        host.step_once();
        assert_eq!(gold(&host, 0), before[0] - 200);
    }

    #[test]
    fn sim_train_reports_a_full_queue_counting_unexecuted_orders() {
        // Without a tick in between: 1 running + 26 waiting are accepted, the 28th is an error
        // (formerly "ok", the simulation silently discarded it).
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        for _ in 0..=MAX_QUEUE {
            host.control_train(hq, COLONIST).unwrap();
        }
        assert_eq!(
            host.control_train(hq, COLONIST).unwrap_err(),
            "queue full (26 orders waiting)"
        );
        host.step_once();
        let b = info(&host, hq).building.unwrap();
        assert_eq!(b.queue.len(), MAX_QUEUE);
        assert!(host.control_train(hq, COLONIST).is_err());
        // One waiting order canceled: room for one again.
        host.control_cancel(hq, 1).unwrap();
        host.step_once();
        host.control_train(hq, COLONIST).unwrap();
        assert!(host.control_train(hq, COLONIST).is_err());
    }

    #[test]
    fn queries_show_accounts_prices_messages_and_nature_without_changing_the_state() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        let section = host.section(&[]);
        assert_eq!(
            (
                section.wood_price,
                section.food_price,
                section.max_living_space
            ),
            (1024, 1351, 125)
        );
        assert!(section.notices.is_empty());
        // Population clamped (living space 50 < limit 125), count per GUID.
        let players = host.query(None, None, None, 1).players;
        assert_eq!(players[0].population[1], 50);
        assert_eq!(players[0].counts.get(&HQ), Some(&1));
        // Slot 1 without objects: only the six time-of-day upgrades of the level start (time of day 0,
        // re/upgrades 3.8).
        let daytime: std::collections::BTreeMap<u32, i32> =
            (0..6).map(|k| (4500 + 10 * k, 1)).collect();
        assert_eq!(players[1].counts, daytime);
        assert!(players[1].pending.is_empty());
        // Queries do not change the state; the remaining wood corresponds to `Sim::attribute`.
        let before = checksum(&host.state);
        let t = info(&host, tree).tree.unwrap();
        let rules = host.rules.clone();
        let wood = Sim::new(&mut host.state, &rules).attribute(tree, tree::WOOD);
        assert_eq!(t.wood, wood);
        assert!(t.registered.is_empty());
        assert!(info(&host, worker).building.is_none());
        assert!(info(&host, worker).construct.is_some());
        assert!(info(&host, worker).gatherer.is_some());
        assert!(info(&host, tree).gatherer.is_none());
        host.control_menu(Some(&[worker]), None).unwrap();
        assert_eq!(checksum(&host.state), before);
        // Messages: only those of the active player, with the tick of the step (the simulation emits
        // collected events at the end of the next tick).
        let rules = host.rules.clone();
        for player in [0, 1] {
            let mut sim = Sim::new(&mut host.state, &rules);
            nml_sim::economy::announce(
                &mut sim,
                player,
                message::NO_GOLD,
                (3, 4),
                message::flags::AFFORD,
            );
        }
        let step = host.state.steps;
        host.step_once();
        let notices = host.section(&[]).notices;
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(
            (notices[0].step, notices[0].id, notices[0].tile),
            (step, message::NO_GOLD, [3, 4])
        );
        host.active_player = 1;
        assert_eq!(host.section(&[]).notices.len(), 1);
    }
    #[test]
    fn sim_complete_checks_the_queue_and_sends_the_debug_command() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        assert_eq!(
            host.control_complete(hq, 0, None).unwrap_err(),
            "no order with index 0 (queue length: 0)"
        );
        host.control_train(hq, COLONIST).unwrap();
        host.control_train(hq, COLONIST).unwrap();
        host.step_once();
        let food = host.state.resource(0, resource::FOOD);
        // Differing expected type: rejected immediately, no command.
        assert_eq!(
            host.control_complete(hq, 1, Some(1112)).unwrap_err(),
            format!("order 1 in {} is {COLONIST}, not 1112", hq.0)
        );
        assert_eq!(
            host.control_complete(hq, 1, Some(COLONIST)).unwrap()["guid"],
            COLONIST
        );
        assert_eq!(
            logged(&mut host),
            [CommandKind::DebugCompleteOrder {
                building: hq,
                index: 1,
                guid: COLONIST,
            }]
        );
        // Paid as at the start (-60 food) and issued immediately; the running one remains.
        assert_eq!(host.state.resource(0, resource::FOOD), food - 60);
        let players = host.query(None, None, None, 1).players;
        assert_eq!(players[0].counts.get(&COLONIST), Some(&1));
        let b = info(&host, hq).building.unwrap();
        assert!(b.queue.is_empty() && b.production.is_some());
        assert_eq!(
            host.control_complete(hq, 1, None).unwrap_err(),
            "no order with index 1 (queue length: 1)"
        );
        let foreign = spawn(&mut host, HQ, 1, (50, 50));
        assert_eq!(
            host.control_complete(foreign, 0, None).unwrap_err(),
            format!("{} is not a building of player 0", foreign.0)
        );
    }

    #[test]
    fn completed_orders_replay_and_continue_from_a_save_bit_identically() {
        // Start only from the rules (HQ in the `.wld`, balance in the `.lvl`) so that the replay
        // yields the same match; afterwards only commands.
        let mut rules = crate::game::economy::testkit::rules();
        rules.level.players.push(nml_formats::config::LevelPlayer {
            slot: 0,
            name: "P0".into(),
            nation: 2,
            kind: 0,
            resources: vec![(0, 1000), (1, 1000), (2, 1000), (4, 50)],
        });
        rules.world_objects.push(nml_formats::wld::WorldObject {
            guid: HQ,
            position: [world(30), 0.0, world(30)],
            scale: [1.0; 3],
            rotation: [0.0; 3],
            color: Some(0),
            name: None,
            config: Default::default(),
        });
        let setup = nml_sim::setup::MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: nml_sim::ki::KiSetup::default(),
            script: nml_sim::script::ScriptSetup::default(),
            ..nml_sim::setup::MatchSetup::default()
        };
        let speed = crate::game::options::DEFAULT_GAME_SPEED;
        let mut live = SimHost::new(rules.clone(), setup.clone(), speed).unwrap();
        live.active_player = 0;
        let hq = nml_sim::setup::find_object(&live.state, 0, HQ).expect("HQ from the .wld");
        // Tick 1: three colonists; tick 3: complete the second waiting one, tick 6: complete the running one.
        let script = |host: &mut SimHost, step: u64| match step {
            1 => {
                for _ in 0..3 {
                    host.control_train(hq, COLONIST).unwrap();
                }
            }
            3 => {
                host.control_complete(hq, 2, None).unwrap();
            }
            6 => {
                host.control_complete(hq, 0, None).unwrap();
            }
            _ => {}
        };
        let mut save = None;
        for step in 0..12 {
            script(&mut live, step);
            if step == 5 {
                save = Some(live.save().to_bytes());
            }
            live.step_once();
        }
        let players = live.query(None, None, None, 1).players;
        assert_eq!(players[0].counts.get(&COLONIST), Some(&2));
        // Replay: the same checksum.
        let replay = nml_sim::save::Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        let mut other = SimHost::new(rules.clone(), setup.clone(), speed).unwrap();
        other.load_replay(replay, Some(12)).unwrap();
        assert_eq!(other.checksum(), live.checksum());
        // Savegame before tick 5, then the same command in tick 6: the same checksum.
        let mut resumed = SimHost::new(rules, setup, speed).unwrap();
        resumed.active_player = 0;
        let save = nml_sim::save::SaveGame::from_bytes(&save.unwrap()).unwrap();
        resumed.load(save, None);
        for step in 5..12 {
            script(&mut resumed, step);
            resumed.step_once();
        }
        assert_eq!(resumed.checksum(), live.checksum());
    }

    // --- Headhunter and robbing (protocol 22) -----------------------------------------------------

    /// State of the simulation and of the host for "the query changes nothing".
    fn sums(host: &SimHost) -> (u64, u64) {
        (host.checksum(), nml_sim::save::checksum(&host.state))
    }

    #[test]
    fn headhunter_info_leaves_the_checksum_unchanged() {
        // A hired hunter on the approach, his throw in the projectile list and a robber at
        // work, all via the paths of the simulation; the query shows the fields as set and
        // changes nothing.
        use crate::game::testworld::{
            HQ_SPA, HUNTER_SID, ROB_UPGRADE, SID_SHERIFF, UNDERTAKER, hunter_host, spawn, unlock,
        };
        use nml_control::{SimHeadhunterInfo, SimHireEntry, SimRobbingInfo};
        use nml_sim::combat::projectile::launch_throw;
        use nml_sim::message::Message;
        use nml_sim::messaging::send;
        use nml_sim::module::headhunter;
        use nml_sim::module::robbing;
        const MANA: u32 = 15;
        let mut host = hunter_host();
        host.state.players[0].resources_fx[0] = 1000 << 12;
        unlock(&mut host, 0, ROB_UPGRADE);
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        let sheriff = spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        let robber = spawn(&mut host, UNDERTAKER, 0, (10, 30));
        host.state
            .object_mut(robber, false)
            .unwrap()
            .attributes
            .set_current_value(MANA, 150, 0);
        // Idle: table empty, price config 12 (300); the robber rests with attack module.
        let table = |host: &SimHost| host.query(None, None, Some(&[]), 0).headhunters;
        assert_eq!(
            table(&host),
            (0..3)
                .map(|art| SimHireEntry {
                    art,
                    hunter: None,
                    price: 300,
                })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            info(&host, robber).robbing,
            Some(SimRobbingInfo {
                anim: 3,
                robbing: false,
                attack_enabled: true,
                mana: 150,
            })
        );
        let r = info(&host, robber);
        assert!(r.headhunter.is_none());
        assert!(r.modules.iter().any(|m| m.kind == "robbing"));
        assert!(info(&host, hq).robbing.is_none() && info(&host, hq).headhunter.is_none());
        // Hiring (kind 2 against the sheriff of slot 1) and robbing via the commands.
        host.submit_own(CommandKind::HireHeadhunter {
            art: 2,
            target_player: 1,
            target_guid: SID_SHERIFF as i32,
            building: hq,
        });
        host.submit_own(CommandKind::RobCorpses {
            unit: robber,
            button: 540,
        });
        // The hunter searches one second after hiring and then walks to the unit (state 1).
        let mut steps = 0;
        let hunter = loop {
            host.step_once();
            steps += 1;
            assert!(steps < 40, "no approach after {steps} steps");
            let Some(h) = host.state.headhunters[2] else {
                continue;
            };
            if info(&host, h).headhunter.is_some_and(|i| i.state == 1) {
                break h;
            }
        };
        // Measured: 24 steps (command in the first step, search with `0x322` at N1 + 11).
        assert_eq!(steps, 24);
        let h = info(&host, hunter);
        assert_eq!(h.guid, HUNTER_SID);
        assert!(h.modules.iter().any(|m| m.kind == "headhunter"));
        assert_eq!(
            h.headhunter,
            Some(SimHeadhunterInfo {
                art: 2,
                state: 1,
                target_guid: SID_SHERIFF as i32,
                target_player: 1,
                target: Some(sheriff.0),
                last_target: Some(sheriff.0),
                price: 350,
                customer: 0,
                customer_cost: 300,
            })
        );
        assert_eq!(
            table(&host)[2],
            SimHireEntry {
                art: 2,
                hunter: Some(hunter.0),
                price: 350,
            }
        );
        // The robber: attack module off, 10 mana paid (150 - 10, plus 1 regeneration in the
        // 24 steps); then the arrival at a corpse
        // (walk success `0x230`, without `CANROBBED` check): lock and timer `0x330`.
        let r = info(&host, robber).robbing.unwrap();
        assert_eq!((r.robbing, r.attack_enabled, r.mana), (false, false, 141));
        {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            let arrived = Message::new(robbing::msg::ARRIVED, &[sheriff.as_i32()]);
            assert_eq!(send(&mut sim, robber, &arrived), 0);
            // Throw at the sheriff (impact at a unit).
            launch_throw(
                &mut sim,
                hunter,
                [5696.0, 0.0, 3904.0],
                headhunter::msg::IMPACT_UNIT,
            );
        }
        let before = sums(&host);
        assert_eq!(
            info(&host, robber).robbing,
            Some(SimRobbingInfo {
                anim: 3,
                robbing: true,
                attack_enabled: false,
                mana: 141,
            })
        );
        let list = host.projectile_infos();
        assert_eq!(list.len(), 1);
        let throw = &list[0];
        assert_eq!(
            (throw.kind.as_str(), throw.impact, throw.pulse),
            ("throw", Some(0xD22), None)
        );
        assert_eq!((throw.shooter, throw.target), (Some(hunter.0), None));
        assert_eq!(throw.target_pos, [5696.0, 0.0, 3904.0]);
        assert_eq!(throw.remaining_ms, throw.flight_ms);
        let _ = info(&host, hunter);
        let _ = table(&host);
        assert_eq!(sums(&host), before);
        // A foreign type with module 34 has no kind (255).
        host.state.object_mut(hunter, false).unwrap().guid = SID_SHERIFF;
        assert_eq!(info(&host, hunter).headhunter.unwrap().art, 255);
    }

    #[test]
    fn hire_menu_matches_the_panel() {
        // `sim_hire_menu` returns the same lists as levels 2 and 3 of the menu in the
        // "Economy" window and changes nothing.
        use crate::game::economy::HireMenu;
        use crate::game::testworld::{
            HQ_SPA, HUNTER_SID, SID_SHERIFF, UNDERTAKER, hunter_host, spawn,
        };
        let mut host = hunter_host();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        spawn(&mut host, UNDERTAKER, 1, (46, 30));
        let compare = |host: &mut SimHost| {
            for art in 0..3u8 {
                for target_player in [None, Some(0), Some(1), Some(2)] {
                    let before = sums(host);
                    let result = host.control_hire_menu(0, art, target_player).unwrap();
                    assert_eq!(sums(host), before);
                    let menu = HireMenu {
                        building: Some(hq),
                        art: Some(usize::from(art)),
                        target_player,
                    };
                    let panel = host
                        .panel_with_hire(&[hq], &menu)
                        .building
                        .and_then(|b| b.hire)
                        .expect("headhunter menu");
                    let players: Vec<u8> = panel.players.iter().map(|p| p.0).collect();
                    let targets: Vec<u32> = panel.targets.iter().map(|t| t.0).collect();
                    assert_eq!(
                        result.players, players,
                        "kind {art}, target {target_player:?}"
                    );
                    assert_eq!(
                        result.targets, targets,
                        "kind {art}, target {target_player:?}"
                    );
                }
            }
        };
        compare(&mut host);
        let r = host.control_hire_menu(0, 2, Some(1)).unwrap();
        assert_eq!((r.players, r.targets), (vec![1], vec![2614, 2615]));
        assert!(
            host.control_hire_menu(0, 2, None)
                .unwrap()
                .targets
                .is_empty()
        );
        // Slot 0 does not play against itself; without elite and without hunter no target.
        assert!(
            host.control_hire_menu(0, 2, Some(0))
                .unwrap()
                .targets
                .is_empty()
        );
        // With the hunter of kind 2 the slot without elite has the HQ of its nation (SPA 1000), even
        // if it is not a target player (like `FUN_0044AA10`).
        spawn(&mut host, HUNTER_SID, 8, (10, 10));
        compare(&mut host);
        let r = host.control_hire_menu(0, 2, Some(0)).unwrap();
        assert_eq!((r.players, r.targets), (vec![1], vec![1000]));
        // From the view of slot 1, slot 0 is the target player.
        assert_eq!(host.control_hire_menu(1, 0, None).unwrap().players, [0]);
        assert_eq!(
            host.control_hire_menu(0, 3, None).unwrap_err(),
            "art = 3 invalid: allowed 0 to 2"
        );
    }

    #[test]
    fn unit_bar_rob_issues_the_command() {
        // `sim_unit_bar` with 540: one `RobCorpses` per selected own robber (via
        // `BarAction::from_button`); nothing without research 3326.
        use crate::game::testworld::{ROB_UPGRADE, UNDERTAKER, hunter_host, spawn, unlock};
        use crate::game::unit_bar::{UnitBar, control_unit_bar};
        let mut host = hunter_host();
        let robber = spawn(&mut host, UNDERTAKER, 0, (10, 10));
        host.state
            .object_mut(robber, false)
            .unwrap()
            .attributes
            .set_current_value(15, 150, 0);
        let mut bar = UnitBar::default();
        assert_eq!(
            control_unit_bar(&mut bar, &mut host, &[robber], 540, None, None),
            Ok(())
        );
        assert!(logged(&mut host).is_empty(), "grayed out without research");
        unlock(&mut host, 0, ROB_UPGRADE);
        assert_eq!(
            control_unit_bar(&mut bar, &mut host, &[robber], 540, None, None),
            Ok(())
        );
        assert_eq!(
            logged(&mut host),
            [CommandKind::RobCorpses {
                unit: robber,
                button: 540,
            }]
        );
        // The simulation accepts it: 10 mana are paid (button 540, config 15); the attack module
        // goes off via the pending list, measured one step later.
        let r = info(&host, robber).robbing.unwrap();
        assert_eq!((r.attack_enabled, r.mana), (true, 140));
        host.step_once();
        let r = info(&host, robber).robbing.unwrap();
        assert_eq!((r.robbing, r.attack_enabled, r.mana), (false, false, 140));
    }

    #[test]
    fn hunter_messages_reach_the_notices() {
        // The text messages of the headhunter are in the messages of the control channel
        // (number from `texte.deu`, without tile): 10002 only for the payer, 10010 for all.
        use crate::game::testworld::{HQ_SPA, SID_SHERIFF, hunter_host, spawn};
        use nml_sim::command::headhunter::{TEXT_HIRED, TEXT_NO_GOLD};
        let mut host = hunter_host();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        let hire = CommandKind::HireHeadhunter {
            art: 2,
            target_player: 1,
            target_guid: SID_SHERIFF as i32,
            building: hq,
        };
        host.state.players[0].resources_fx[0] = 100 << 12;
        host.submit_own(hire.clone());
        host.step_once();
        let ids = |host: &SimHost, p: u8| -> Vec<(i32, [i32; 2])> {
            host.recent_messages(p)
                .iter()
                .map(|n| (n.id, n.tile))
                .collect()
        };
        assert_eq!(ids(&host, 0), [(TEXT_NO_GOLD, [-1, -1])]);
        assert!(ids(&host, 1).is_empty());
        host.state.players[0].resources_fx[0] = 1000 << 12;
        host.submit_own(hire);
        host.step_once();
        assert_eq!(
            ids(&host, 0),
            [(TEXT_NO_GOLD, [-1, -1]), (TEXT_HIRED, [-1, -1])]
        );
        for p in 1..8 {
            assert_eq!(ids(&host, p), [(TEXT_HIRED, [-1, -1])], "slot {p}");
        }
        // The section `sim` shows the messages of the active player.
        let notices = host.section(&[]).notices;
        assert_eq!(notices.last().map(|n| n.id), Some(TEXT_HIRED));
    }

    // --- AI queries (protocol 23) -----------------------------------------------------------------

    #[test]
    fn ki_query_leaves_the_checksum_unchanged() {
        // A scene with AI slot 1: a marked archer with an attack report. `ki_query`
        // reports the direct changes of the codes 0x4E24 (search with marking) and 0x512E
        // (taking the next report) but does not execute them; checksum, marking,
        // report and events remain the same.
        use crate::game::testworld::{ARCHER, hunter_host, spawn};
        use nml_control::KiDirectInfo;
        use nml_sim::state::ReportTile;
        let mut host = hunter_host();
        let archer = spawn(&mut host, ARCHER, 1, (11, 10));
        host.state.object_mut(archer, false).unwrap().ai_marks = 0b0000_0100;
        host.state.attack_reports[1].insert(archer.as_i32(), ReportTile { x: 11, z: 10 });
        let before = sums(&host);
        let events = host.state.events.clone();
        let direct = |kind: &str, object: ObjectId, player: u8| KiDirectInfo {
            kind: kind.into(),
            object: Some(object.0),
            player,
        };
        let max_words = nml_control::MAX_KI_QUERY_WORDS;
        assert_eq!(max_words, nml_sim::ki::query::QUERY_WORDS);

        // 0x4E24 `(marking slot, reference slot, x, z, head filter, GUID)`: finds the archer.
        let words = [1, 1, 10, 10, 0, ARCHER as i32];
        let found = host
            .control_ki_query(1, 0x4E24, Some(&words), None)
            .unwrap();
        assert_eq!(found.code, 0x4E24);
        assert_eq!(found.request.len(), 2 + max_words);
        assert_eq!(&found.request[..3], &[0x4E24, 6, archer.as_i32()]);
        assert_eq!(found.direct, [direct("mark", archer, 1)]);
        // With fog sight slot 1 sees its own archer just the same (the map is revealed).
        let fog = host
            .control_ki_query(1, 0x4E24, Some(&words), Some("fog"))
            .unwrap();
        assert_eq!(fog.request, found.request);
        assert_eq!(fog.direct, found.direct);

        // 0x512E `(slot, x, z)`: the report would come out; `direct` only names it.
        let taken = host
            .control_ki_query(1, 0x512E, Some(&[1, 0, 0]), Some("fog"))
            .unwrap();
        assert_eq!(&taken.request[..5], &[0x512E, 3, 1, 11, 10]);
        assert_eq!(taken.direct, [direct("take_report", archer, 1)]);
        // Without words `[player]` applies: argc 1 fits no code, the field remains unchanged.
        let none = host.control_ki_query(1, 0x4E24, None, None).unwrap();
        assert_eq!(&none.request[..3], &[0x4E24, 1, 1]);
        assert!(none.direct.is_empty());
        // Unmarking (0x4E45 `(slot, object)`) reports "unmark"; a marking of a missing
        // object has no effect and is therefore not in `direct`.
        let off = host
            .control_ki_query(1, 0x4E45, Some(&[1, archer.as_i32()]), None)
            .unwrap();
        assert_eq!(off.direct, [direct("unmark", archer, 1)]);
        let missing = ObjectId::new(900, 1, 1).as_i32();
        let gone = host
            .control_ki_query(1, 0x4E44, Some(&[1, missing]), None)
            .unwrap();
        assert!(gone.direct.is_empty());
        // Unoccupied slot 7 with fog sight: a response without hits, state untouched.
        assert_eq!(
            host.state.players[7].control,
            nml_sim::state::Control::Empty
        );
        let empty = host
            .control_ki_query(
                7,
                0x4E24,
                Some(&[7, 7, 10, 10, 0, ARCHER as i32]),
                Some("fog"),
            )
            .unwrap();
        assert_eq!(&empty.request[..3], &[0x4E24, 6, 0]);
        assert!(empty.direct.is_empty());
        assert_eq!(sums(&host), before);
        assert_eq!(host.state.events, events);
        // Unknown code: field unchanged, nothing direct.
        let unknown = host.control_ki_query(1, 0x1234, Some(&[7]), None).unwrap();
        assert_eq!(&unknown.request[..3], &[0x1234, 1, 7]);
        // Wrong sight and slot outside: error instead of panic.
        assert!(
            host.control_ki_query(1, 0x4E24, None, Some("blind"))
                .is_err()
        );
        assert!(host.control_ki_query(8, 0x4E24, None, None).is_err());
        assert!(
            host.control_ki_query(1, 0x4E24, Some(&vec![0; max_words + 1]), None)
                .is_err()
        );

        assert_eq!(sums(&host), before);
        assert_eq!(host.state.events, events);
        assert_eq!(
            host.state.object(archer, false).unwrap().ai_marks,
            0b0000_0100
        );
        assert_eq!(host.state.attack_reports[1].len(), 1);
    }

    // --- AI match (protocol 26) ---------------------------------------------------------------

    use crate::game::testworld::{KI_MARK, ki_host};

    #[test]
    fn ki_limits_of_the_protocol_are_the_ones_of_the_simulation() {
        assert_eq!(nml_control::MAX_KI_ARGS, MAX_START_ARGS);
        assert_eq!(nml_control::MAX_KI_STEP_PASSES, MAX_STEP_PASSES);
        assert_eq!(
            nml_control::CAMPAIGN_DIFFICULTY_RANGE,
            nml_sim::setup::DIFFICULTY_RANGE
        );
        for kind in nml_control::KI_KIND_RANGE {
            let h = nml_sim::setup::ki_kind_handicap(kind).expect("AI kind 1-4");
            assert!((0..=nml_control::MAX_SLOT_HANDICAP).contains(&h));
        }
    }

    #[test]
    fn ki_state_leaves_the_checksum_unchanged() {
        use nml_sim::command::Outcome;
        use nml_sim::ki::start::START_PLAYER;
        use nml_sim::state::SimEvent;
        let mut host = ki_host();
        for _ in 0..3 {
            host.step_once();
        }
        // Counters from the events (host state): one accepted, two rejected commands.
        let event = |player, code, outcome| SimEvent::KiCommand {
            player,
            code,
            outcome,
        };
        host.ki_stats.record(&[
            event(1, 0x04, Outcome::Accepted),
            event(1, 0x01, Outcome::Rejected("Construction: no space".into())),
            event(1, 0x01, Outcome::Rejected("Construction: no space".into())),
            event(3, 0x0E, Outcome::Partial("Production: foreign".into())),
        ]);
        let before = sums(&host);
        let pending = host.pending_commands().len();
        let all = host.control_ki_state(None, true).unwrap();
        assert!(!all.paused);
        assert_eq!(
            all.engines.iter().map(|e| e.slot).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        let ai = &all.engines[1];
        assert_eq!(ai.values, [(302, 1000)]);
        assert_eq!(ai.next_thread_id, 2);
        let t = &ai.threads[0];
        assert_eq!(
            (t.id, t.module, t.name.as_str()),
            (1, START_PLAYER, "multi_startplayer")
        );
        // 3 ticks * 9 steps, every second one a `PUSHA`: stack 3 + 14, at the top at most 8 values.
        assert_eq!(t.stack_depth, 3 + 14);
        assert_eq!(t.stack_top, [1; 8]);
        assert_eq!(
            (t.status, t.wait_semaphore, t.child.is_none()),
            (1, -1, true)
        );
        assert!(all.engines[0].threads.is_empty());
        assert_eq!(all.rejected.len(), 1);
        assert_eq!(
            (
                all.rejected[0].slot,
                all.rejected[0].code,
                all.rejected[0].count
            ),
            (1, 0x01, 2)
        );
        let outcomes: Vec<_> = all
            .commands
            .iter()
            .map(|c| (c.slot, c.code, c.outcome.as_str(), c.count))
            .collect();
        assert_eq!(
            outcomes,
            [
                (1, 0x01, "rejected", 2),
                (1, 0x04, "accepted", 1),
                (3, 0x0E, "partial", 1)
            ]
        );
        // Only slot 3, without values.
        let one = host.control_ki_state(Some(3), false).unwrap();
        assert_eq!(one.engines.len(), 1);
        assert!(one.engines[0].values.is_empty());
        assert!(one.rejected.is_empty());
        assert_eq!(one.commands.len(), 1);
        assert!(host.control_ki_state(Some(8), false).is_err());
        assert_eq!(sums(&host), before);
        assert_eq!(host.pending_commands().len(), pending);
        // The counters are host state: a restart clears them.
        host.restart(host.setup.clone()).unwrap();
        assert_eq!(host.ki_stats, crate::game::sim_host::KiStats::default());
    }

    #[test]
    fn ki_command_is_recorded_and_replayed() {
        use crate::game::sim_control::KiCommandRequest;
        use nml_control::KiAction;
        use nml_sim::ki::KiPause;
        let mut live = ki_host();
        let request = |action, slot, module, passes| KiCommandRequest {
            action,
            slot,
            module,
            args: None,
            sync: None,
            paused: None,
            passes,
        };
        // Pre-checks of the host: error message, nothing queued.
        for (bad, error) in [
            (
                request(KiAction::Step, Some(1), None, None),
                "AI is running",
            ),
            (
                request(KiAction::Start, Some(5), Some(KI_MARK), None),
                "no AI engine",
            ),
            (
                request(KiAction::Start, Some(1), Some(4711), None),
                "module 4711 unknown",
            ),
            (request(KiAction::Start, Some(1), None, None), "module"),
            (request(KiAction::Step, Some(9), None, None), "slot = 9"),
        ] {
            let e = live.control_ki_command(bad).unwrap_err();
            assert!(e.contains(error), "{e}");
        }
        assert!(live.pending_commands().is_empty());
        let script = |host: &mut SimHost, step: u64| {
            let r = match step {
                2 => request(KiAction::Pause, None, None, None),
                3 => request(KiAction::Step, Some(1), None, Some(2)),
                5 => request(KiAction::Start, Some(1), Some(KI_MARK), None),
                6 => request(KiAction::Step, Some(1), None, Some(4)),
                8 => KiCommandRequest {
                    paused: Some(false),
                    ..request(KiAction::Pause, None, None, None)
                },
                _ => return,
            };
            host.control_ki_command(r).unwrap();
        };
        let mut sums = Vec::new();
        for step in 0..12 {
            script(&mut live, step);
            if step == 2 {
                // The waiting pause already counts for the pre-check of `step`.
                let r = request(KiAction::Step, Some(1), None, Some(1));
                live.control_ki_command(r).unwrap();
            }
            live.step_once();
            sums.push(nml_sim::save::checksum(&live.state));
        }
        // The control commands are in the command log, AI commands are not.
        let logged: Vec<_> = live
            .log
            .iter()
            .map(|c| (c.step, c.command.kind.clone()))
            .collect();
        assert_eq!(
            logged,
            [
                (2, CommandKind::KiPause { paused: true }),
                (2, CommandKind::KiStep { slot: 1, passes: 1 }),
                (3, CommandKind::KiStep { slot: 1, passes: 2 }),
                (
                    5,
                    CommandKind::KiStart {
                        slot: 1,
                        module: KI_MARK,
                        args: Vec::new(),
                        sync: false
                    }
                ),
                (6, CommandKind::KiStep { slot: 1, passes: 4 }),
                (8, CommandKind::KiPause { paused: false }),
            ]
        );
        assert_eq!(live.state.ki.paused, KiPause::Running);
        let e = live.state.ki.engines[1].as_ref().unwrap();
        assert_eq!(e.mem.values.get(&500), Some(&1), "module ran (single step)");
        // Replay: the same checksum sequence tick by tick.
        let replay = nml_sim::save::Replay::from_bytes(&live.replay().unwrap().to_bytes()).unwrap();
        for (n, &sum) in sums.iter().enumerate() {
            let state = replay.play(&live.rules, n as u64 + 1).unwrap();
            assert_eq!(nml_sim::save::checksum(&state), sum, "tick {}", n + 1);
        }
        // Counter-check: without the control commands a different sequence.
        let mut plain = ki_host();
        for _ in 0..12 {
            plain.step_once();
        }
        assert_ne!(nml_sim::save::checksum(&plain.state), sums[11]);
    }

    /// `sim_hash`, `sim_query` and `sim_command` without a match: error message, with a match the same responses
    /// as in the viewer.
    #[test]
    fn execute_sim_needs_a_match_and_answers_like_the_viewer() {
        let hash = ControlCommand::SimHash;
        let query = ControlCommand::SimQuery {
            player: None,
            guid: None,
            ids: None,
            limit: None,
        };
        let command = |json: serde_json::Value| ControlCommand::SimCommand { command: json };
        let halt = |player: u8, unit: u32| {
            command(serde_json::json!({
                "player": player, "kind": {"Halt": {"units": [unit]}}
            }))
        };
        for c in [&hash, &query, &halt(0, 1)] {
            assert_eq!(execute_sim(None, c), Err("no match".to_string()), "{c:?}");
        }

        let mut host = host();
        spawn(&mut host, COLONIST, 0, (10, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        let value = execute_sim(Some(&mut host), &hash).unwrap();
        assert_eq!(value["steps"], 0);
        assert_eq!(
            value["checksum"],
            serde_json::json!(nml_control::hex_checksum(host.checksum()))
        );
        let value = execute_sim(Some(&mut host), &query).unwrap();
        assert_eq!(value["total"], 2, "colonist and headquarters");
        let id = value["objects"][0]["id"].as_u64().unwrap() as u32;

        // Command: queued for the next tick, response with the tick.
        assert_eq!(
            execute_sim(Some(&mut host), &halt(0, id)).unwrap(),
            serde_json::json!({"step": 0})
        );
        assert_eq!(host.pending_commands().len(), 1);
        host.step_once();
        assert!(host.pending_commands().is_empty());
        assert_eq!(host.log.len(), 1);
        // Player above 7 and broken commands: message, nothing queued.
        let message = execute_sim(Some(&mut host), &halt(8, id)).unwrap_err();
        assert!(message.contains("player = 8 invalid"), "{message}");
        let message =
            execute_sim(Some(&mut host), &command(serde_json::json!({"x": 1}))).unwrap_err();
        assert!(message.starts_with("command invalid"), "{message}");
        assert!(host.pending_commands().is_empty());
        // Order of the messages: read and check the command, only then the simulation - also without a match.
        let message = execute_sim(None, &command(serde_json::json!({"x": 1}))).unwrap_err();
        assert!(message.starts_with("command invalid"), "{message}");
        let message = execute_sim(None, &halt(9, id)).unwrap_err();
        assert!(message.contains("player = 9 invalid"), "{message}");
        // Other commands are not simulation commands.
        let message = execute_sim(Some(&mut host), &ControlCommand::Hello).unwrap_err();
        assert!(message.contains("not a simulation command"), "{message}");
    }

    // --- World state of the scripts (protocol 30) -------------------------------------------------

    #[test]
    fn sim_query_reports_names_hidden_kills_locks_and_views() {
        use nml_sim::fog::AreaView;
        let mut host = host();
        let held = spawn(&mut host, COLONIST, 0, (10, 10));
        let hidden = spawn(&mut host, COLONIST, 1, (11, 10));
        let revealed = spawn(&mut host, COLONIST, 1, (12, 10));
        let unnamed = spawn(&mut host, COLONIST, 0, (13, 10));
        nml_sim::names::register(&mut host.state.names, "Hero", held);
        nml_sim::names::register(&mut host.state.names, "Hideout", hidden);
        let fresh = host.query(None, None, None, 100);
        assert_eq!(fresh.slot_running, [true; 11]);
        assert!(fresh.kill_counts.is_empty() && fresh.button_locks.is_empty());
        assert!(fresh.area_views.is_empty());
        assert!(fresh.objects.iter().all(|o| !o.hidden));

        // Slot 1 hidden, one object of it individually shown (`inactive_lists`).
        host.state.slot_running[1] = false;
        host.state.inactive_lists[1].push(revealed);
        host.state.kill_counts[2].insert(1100, 3);
        host.state.kill_counts[0].insert(1004, 1);
        host.state.kill_counts[0].insert(1000, 2);
        host.state.button_locks.insert(1005, 1);
        host.state.button_locks.insert(1000, 0);
        host.state.fog.area_views.push(AreaView {
            name: "meadow".into(),
            rect: [9, 8, 2, 1],
            player: 3,
            clear: true,
        });
        host.state.fog.area_views.push(AreaView {
            name: "forest".into(),
            rect: [0, 0, 1, 1],
            player: 0,
            clear: false,
        });
        let result = host.query(None, None, None, 100);
        let by_id = |id: ObjectId| {
            result
                .objects
                .iter()
                .find(|o| o.id == id.0)
                .expect("object in the response")
        };
        assert_eq!(
            (by_id(held).name.as_deref(), by_id(held).hidden),
            (Some("hero"), false)
        );
        assert_eq!(
            (by_id(hidden).name.as_deref(), by_id(hidden).hidden),
            (Some("hideout"), true),
            "hidden slot, not individually shown"
        );
        assert_eq!(
            (by_id(revealed).name.as_deref(), by_id(revealed).hidden),
            (None, false),
            "individually shown"
        );
        assert_eq!(
            (by_id(unnamed).name.as_deref(), by_id(unnamed).hidden),
            (None, false)
        );
        let mut running = [true; 11];
        running[1] = false;
        assert_eq!(result.slot_running, running);
        assert_eq!(
            result.kill_counts,
            [(0, 1000, 2), (0, 1004, 1), (2, 1100, 3)]
        );
        assert_eq!(result.button_locks, [(1000, 0), (1005, 1)]);
        let views: Vec<_> = result
            .area_views
            .iter()
            .map(|v| (v.name.as_str(), v.rect, v.player, v.clear))
            .collect();
        assert_eq!(
            views,
            [
                ("meadow", [9, 8, 2, 1], 3, true),
                ("forest", [0, 0, 1, 1], 0, false)
            ],
            "append order, rectangle not normalized"
        );
        // With filter: the same world lists, only the filtered objects.
        let one = host.query(Some(1), None, None, 1);
        assert_eq!((one.total, one.objects.len()), (2, 1));
        assert_eq!(one.kill_counts, result.kill_counts);
        // The response is JSON without empty fields per object.
        let value = serde_json::to_value(&result).unwrap();
        let json_of = |id: ObjectId| {
            value["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == id.0)
                .unwrap()
        };
        assert_eq!(json_of(hidden)["hidden"], true);
        assert!(json_of(unnamed).get("name").is_none() && json_of(unnamed).get("hidden").is_none());
    }

    #[test]
    fn queries_do_not_change_the_checksum() {
        // `sim_query` and `script_state` only read: checksum, state and command queue remain the same, even
        // when all world fields and mission objectives are occupied.
        use nml_sim::fog::AreaView;
        use nml_sim::script::Quest;
        let mut host = host();
        let held = spawn(&mut host, COLONIST, 0, (10, 10));
        nml_sim::names::register(&mut host.state.names, "hero", held);
        host.state.slot_running[2] = false;
        host.state.kill_counts[1].insert(1100, 5);
        host.state.button_locks.insert(1005, 1);
        host.state.fog.area_views.push(AreaView {
            name: "meadow".into(),
            rect: [1, 1, 3, 3],
            player: 0,
            clear: true,
        });
        let mission = &mut host.state.script.mission;
        mission.main_quests[0] = Quest { status: 3, text: 7 };
        mission.sub_quests[9] = Quest { status: 6, text: 8 };
        mission.countdown_container = 1;
        mission.countdown_s = 42;
        mission.task_status.insert(3, 1);
        host.state
            .script
            .area_overrides
            .insert("meadow".into(), [1, 1, 3, 3]);
        host.step_once();
        let before = sums(&host);
        let state = host.state.clone();
        let pending = host.pending_commands().len();
        let first = host.query(None, None, None, 100);
        let script = host.control_script_state(None, None, None).unwrap();
        assert_eq!(host.query(None, None, None, 100), first);
        assert_eq!(sums(&host), before);
        assert_eq!(host.state, state);
        assert_eq!(host.pending_commands().len(), pending);
        assert_eq!(script.main_quests[0].status, 3);
    }
}
