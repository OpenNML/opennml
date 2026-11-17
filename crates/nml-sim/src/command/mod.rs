//! Game commands (`0x500000xx`, re/commands): the only input of the simulation. The
//! original carries no sender in the command; here every command has a player slot that the simulation
//! checks against (exception A8). The economy commands are in [`economy`], their check against
//! the sender in [`validate`]. Further command kinds are added as they are implemented.

pub mod ability;
pub mod ai;
pub mod chat;
#[cfg(test)]
mod combat_tests;
pub mod economy;
pub mod gate;
mod group;
pub mod headhunter;
mod riding;
pub mod tent;
#[cfg(test)]
pub(crate) mod testkit;
pub mod trade;
mod train;
pub mod upgrade;
pub mod validate;

use serde::{Deserialize, Serialize};

use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::{Envelope, Message};
use crate::messaging::{abort_all_modules, post_command};
use crate::module::walk::find_to_msg;
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

/// An `f32` word of the command (world coordinate, rotation in radians), lossless as a bit pattern: the
/// command words of the original are 32 bits wide, so equality and savegame are bit-exact (also for
/// NaN and -0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct F32Bits(pub u32);

impl F32Bits {
    pub fn new(v: f32) -> Self {
        Self(v.to_bits())
    }

    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

impl From<f32> for F32Bits {
    fn from(v: f32) -> Self {
        Self::new(v)
    }
}

/// A part of a multipart building (command `0x18`): `{guid, rotation, x, y, z}` as in the
/// original (rotation in second position, re/commands).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingPart {
    pub guid: u32,
    pub rotation: F32Bits,
    pub x: F32Bits,
    pub y: F32Bits,
    pub z: F32Bits,
}

/// Spread "none" of the move command (`MoveCommand.spread` -1, re/groups-formations 12.6).
pub const NO_SPREAD: i32 = -1;

fn no_spread() -> i32 {
    NO_SPREAD
}

/// Type of the debug command "complete immediately": outside the jump table `0x0048DB04`
/// (`0x01…0x7F`), so without a handler in the original.
pub const DEBUG_COMPLETE_ORDER: u32 = 0xF0;

/// Types of the AI control commands: like [`DEBUG_COMPLETE_ORDER`] without a handler in the
/// original (console `runmodule`, `.abl` `n0`/`n1`, debugger `0x4B0`).
pub const KI_START: u32 = 0xF1;
pub const KI_PAUSE: u32 = 0xF2;
pub const KI_STEP: u32 = 0xF3;

/// Types of the acceptance helpers of the script path: like [`DEBUG_COMPLETE_ORDER`]
/// without a handler in the original. `ScriptSkipDialog` and `DebugDestroy`.
pub const SCRIPT_SKIP_DIALOG: u32 = 0xF4;
pub const DEBUG_DESTROY: u32 = 0xF5;

/// Types of the host commands for conditions 35-38 (O-246): like [`DEBUG_COMPLETE_ORDER`] without a
/// handler in the original (there the condition reads the UI of the machine, E-136). `ScriptSelection`
/// and `ScriptButton`.
pub const SCRIPT_SELECTION: u32 = 0xF6;
pub const SCRIPT_BUTTON: u32 = 0xF7;

/// Command kind with the parameters of the original. New variants go at the end (order of the
/// serde encoding in savegames and recordings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandKind {
    /// `0x04` Move (`GameWorld_CmdMoveSelection` `0x0047CA10`, re/walk 15.1): destination in
    /// fixed-point tiles, `target` ends up as the target radius in the FINDTO (always -1 from the UI,
    /// quirk), `direction` facing direction at the destination (`NO_DIRECTION` = any), `flags` bit 0 run,
    /// bits 1 and 2 attack mode (attack march, re/walk 15.1). Field order of
    /// `MoveCommand_Construct` (re/groups-formations 12.6): `formation` 0...6 group type, >= 7 no
    /// group (K-210); `spread` drag length in fixed point, effective 1.0...2.0; optional in JSON.
    Move {
        units: Vec<ObjectId>,
        x: i32,
        z: i32,
        target: i32,
        #[serde(default)]
        formation: i32,
        direction: i32,
        #[serde(default = "no_spread")]
        spread: i32,
        flags: i32,
    },
    /// `0x3C` Halt (`GameWorld_CmdHalt` `0x004770A0`): abort all modules of the units.
    Halt { units: Vec<ObjectId> },
    /// `0x63` shared vision (`GameWorld_CmdSetSharedVision` `0x00478820`): `p` sees what `q`
    /// sees (one-sided). The reimplementation accepts it only if `p` or `q` is the sender (A8).
    SharedVision { p: i32, q: i32, on: bool },
    /// `0x64` Diplomacy (`GameWorld_CmdSetDiplomacy` `0x00478720`): `mode` 0 friend, 1 enemy,
    /// 2 neutral; one-sided for `p`.
    Diplomacy { p: i32, q: i32, mode: i32 },
    /// `0x7B`/`0x7C` wrapper for Shift (append) or Ctrl (cyclic)
    /// (`GameWorld_ExecuteQueuedCommand` `0x0048DD00`). At most one wrapper deep: a wrapper in
    /// a wrapper is rejected already when reading ([`deserialize_queue_inner`]) and when executing (A8).
    Queue {
        cyclic: bool,
        #[serde(deserialize_with = "deserialize_queue_inner")]
        inner: Box<CommandKind>,
    },
    /// `0x01` Place building (`GameWorld_CmdPlaceSingleBuilding` `0x004896D0`): type, rotation about Y
    /// (radians) and world position as `f32` like in the original, the workers and the optional
    /// extra word after the list (`0x3303 {n}`: occupancy border, no "complete immediately").
    PlaceBuilding {
        guid: u32,
        rotation: F32Bits,
        x: F32Bits,
        y: F32Bits,
        z: F32Bits,
        workers: Vec<ObjectId>,
        border: Option<i32>,
    },
    /// `0x02` Build or repair (`GameWorld_CmdConstructAtBuilding` `0x0047C8C0`).
    ConstructAt {
        building: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x03` Mine gold (`GameWorld_CmdMineGold` `0x0047C980`).
    MineGold {
        mine: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x06` Chop wood (`GameWorld_CmdChopWood` `0x0047CD50`).
    ChopWood {
        tree: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x7F` find the next tree at a tile (`GameWorld_CmdLumberNearPosition`
    /// `0x0047BEE0`): only as a cyclic entry that `0x06` appends.
    NextTree {
        tile_x: i32,
        tile_z: i32,
        unit: ObjectId,
    },
    /// `0x07` Harvest (`GameWorld_CmdHarvest` `0x0047D430`).
    Harvest {
        farm: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x09` Deliver (`GameWorld_CmdDeliverLoad` `0x0047CE70`).
    Deliver {
        building: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x0E` Enqueue production (`GameWorld_CmdEnqueueProduction` `0x00475FC0`).
    EnqueueProduction { building: ObjectId, guid: u32 },
    /// `0x0F` Cancel production (`GameWorld_CmdCancelProduction` `0x004760B0`): `index` 0 =
    /// running order, `arg` the second argument of `0x1103`.
    CancelProduction {
        building: ObjectId,
        index: i32,
        arg: i32,
    },
    /// `0x13` Quarter (`GameWorld_CmdQuarterUnits` `0x0047D8D0`).
    Quarter {
        container: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x14` unquarter one unit (`GameWorld_CmdUnquarterUnit` `0x00476560`).
    Unquarter { container: ObjectId, unit: ObjectId },
    /// `0x15` unquarter all (`GameWorld_CmdUnquarterAll` `0x004765B0`).
    UnquarterAll { container: ObjectId },
    /// `0x18` place a multipart building (wall) (`GameWorld_CmdPlaceBuilding` `0x004899B0`):
    /// part 0 becomes the master of the wall group.
    PlaceBuildings {
        parts: Vec<BuildingPart>,
        workers: Vec<ObjectId>,
    },
    /// `0x27` Rally point (`GameWorld_CmdSetRallyPoint` `0x00476E60`): ground point in
    /// fixed-point tiles (`x = −1` would be the object variant with `z` = target, which the right-click
    /// never produces).
    SetRallyPoint {
        x: i32,
        z: i32,
        buildings: Vec<ObjectId>,
    },
    /// `0x67` Sell buildings (`GameWorld_CmdSellBuildings` `0x00478A90`).
    SellBuildings { buildings: Vec<ObjectId> },
    /// `0x7D` Fish (`GameWorld_CmdFish` `0x0047BD70`).
    Fish {
        basket: ObjectId,
        units: Vec<ObjectId>,
    },
    /// Debug "complete immediately" (viewer, no counterpart in the original): complete order `index` of the
    /// queue of `building` (0 = running, 1 ... = waiting like `0x0F`) through the normal
    /// completion ([`crate::module::building::production::complete_now`]); only for
    /// own, living buildings of the sender (A8). `guid` is the type the sender saw at this
    /// point; if a different one is there now (the queue advanced between click and
    /// execution), the command is rejected.
    DebugCompleteOrder {
        building: ObjectId,
        index: i32,
        guid: u32,
    },
    /// `0x11` Attack (`GameWorld_CmdAttack` `0x0047D330`); with ground position `pos` (fixed-point
    /// position multiplied by 128 of the fog memory) type `0x71`: if the target is gone at execution, the
    /// unit runs there. `mods` bit mask: 1 = right double-click (run, message flag `0x100`), 4 =
    /// attack mode (`0x400`).
    Attack {
        units: Vec<ObjectId>,
        target: ObjectId,
        pos: Option<[i32; 3]>,
        mods: i32,
    },
    /// `0x12` Behavior (`GameWorld_CmdSetBehavior` `0x004762E0`): stance 0 ... 4 for units
    /// (attack and walk behavior) and buildings (`0x3503`); can be queued.
    SetBehavior { units: Vec<ObjectId>, mode: i32 },
    /// `0x5B` Garrison attack (`GameWorld_CmdQuarteredAttack` `0x004784C0`): the garrison of the
    /// buildings attacks `target` (`0x1B17`).
    GarrisonAttack {
        buildings: Vec<ObjectId>,
        target: ObjectId,
    },
    /// `0x16` Gut (`gut`) or `0x17` Hunt (`GameWorld_CmdHuntOrGut` `0x004764E0`; both the
    /// same, C-182): **only with `direct`** per unit `{0x216, animal}` with abort and clearing the queue;
    /// no effect via the queue (K-81). No acknowledgment.
    Hunt {
        animal: ObjectId,
        units: Vec<ObjectId>,
        gut: bool,
    },
    /// `0x74` Follow (`GameWorld_CmdFollow` `0x00479300`, re/walk 15.4): per unit `{0x3900, target}`
    /// with abort, with `direct` clear the queue. No acknowledgment, no target check.
    Follow {
        target: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x44` Formation (`GameWorld_CmdSetFormation` `0x0047E710`, re/groups-formations 2.2): only with
    /// n > 1 a move command with anchor (-1, -1), direction "none", spread -1 (realigning or
    /// re-forming around the centroid, re/groups-formations 12.9/12.10); `formation` as with [`CommandKind::Move`].
    SetFormation {
        units: Vec<ObjectId>,
        formation: i32,
    },
    /// `0x75` Scatter (`GameWorld_CmdScatterSelection` `0x0047F530`, re/groups-formations 3.9).
    Scatter { units: Vec<ObjectId> },
    /// `0x0A` Mount (`GameWorld_CmdMountHorse` `0x0047CFB0`, re/riding 2.5, 13.7): `horse` is
    /// the horse of the right-click; if it is not enough for all units, the command distributes them among
    /// the free saddle walkers in the vicinity (any owner, K-224).
    Mount {
        horse: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x0B` Dismount (`GameWorld_CmdDismount` `0x00475B10`).
    Dismount { units: Vec<ObjectId> },
    /// `0x0C` Board (`GameWorld_CmdBoardBoat` `0x0047D2A0`): the original does not check the target.
    BoardBoat {
        boat: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x0D` Disembark (`GameWorld_CmdLeaveBoat` `0x00475CD0`).
    LeaveBoat { units: Vec<ObjectId> },
    /// `0x1B` Dive (`GameWorld_CmdDive` `0x00476960`, re/swim-dive 2.3).
    Dive { units: Vec<ObjectId> },
    /// `0x1C` Surface (`GameWorld_CmdSurface` `0x004769E0`).
    Surface { units: Vec<ObjectId> },
    /// `0x29` Train forward (`GameWorld_CmdTrainForward` `0x00476F80`), `0x2A` backward
    /// (`0x00476FF0`), `0x2B` halt (`0x00477060`; re/train-multipart 2.2, 13.3): the locomotive
    /// receives the message immediately, without queueing and without a response. `target` is the target tile
    /// (only from scripts, `{0x121, a, b}`); the halt does not use it.
    Train {
        train: ObjectId,
        op: TrainOp,
        target: Option<(i32, i32)>,
    },
    /// `0x39` Apply upgrade (console, script; `GameWorld_ExecuteCommand` `0x0048D8B6`,
    /// re/upgrades 2): `guid ≥ 0` -> `Player_ApplyUpgrade(slot, guid, 0)`, otherwise all GUIDs
    /// 2800...3399 in ascending order. Without an ownership check like the original; slot outside 0...10 is
    /// rejected (A7). Execution in [`upgrade`].
    ApplyUpgrade { slot: i32, guid: i32 },
    /// `0x3A` Remove upgrade (console): `Player_RemoveUpgrade(slot, guid)`, outside 0...7 has no
    /// effect (original).
    RemoveUpgrade { slot: i32, guid: u32 },
    /// `0x2F` Execute counter-upgrade (`GameWorld_CmdStartCounterUpgrade` `0x004848F0`,
    /// re/temp-upgrades 3.5): command bar 601 (`COUNTERUPGRADE_MSG`) on the unit `source`. The
    /// original checks neither ownership nor menu nor `REQUIRED_GUID` (W-231); the reimplementation checks like
    /// the UI (A8, [`upgrade`]), then [`crate::upgrades::counter::start`].
    CounterUpgrade { guid: u32, source: ObjectId },
    /// `0x36` Cast spell (`GameWorld_CmdCastSpell` `0x004774F0`, re/temp-upgrades 2.3, 15
    /// no. 8; re/spells 2.3): `{0x126, [art, anim, idx, button]}` with abort to the caster,
    /// `direct` then empties the queue. The reimplementation accepts kinds 1/2/3/4 (kind 1/2
    /// with 480/481 and the target point as `f32` bits in `anim`/`idx`, kind 3 with 482 and `0, 0`,
    /// kind 4 with the ability buttons 543 ff.; kind 5 only via `0x4E`) and checks like the
    /// UI (A8, [`upgrade`]).
    CastSpell {
        caster: ObjectId,
        art: i32,
        anim: i32,
        idx: i32,
        button: i32,
    },
    /// `0x5E` Execute temporary upgrade (`GameWorld_CmdExecTempUpgrade` `0x0047EE90`, AI only):
    /// subtract mana = config 15 of the button, then `{0x12C, [idx]}` with abort to the object.
    /// Checked is an own object with module 44 and `idx` in its list (A7/A8, [`upgrade`]).
    ExecTempUpgrade {
        object: ObjectId,
        button: i32,
        idx: i32,
    },
    /// `0x38` Arm (`arm`) or `0x72` Disarm (`GameWorld_CmdToggleArming` `0x00475C10`,
    /// re/arming-gates 2.A, 3.A): for each present unit with `direct` empty the queue, then post `0x127`
    /// with abort only if `arm` != "FightFar (0x13) switched on". The original checks no upgrade
    /// (W-264); the reimplementation checks like the command bar (A8, [`upgrade`]).
    ToggleArming { units: Vec<ObjectId>, arm: bool },
    /// `0x40` Fire arrows on (`fire`) or `0x41` off (`GameWorld_CmdSetArrowType` `0x00475B90`,
    /// re/arming-gates 2.B, 3.B): no command bar produces it (C-224); player commands are
    /// rejected (A8), the effect is [`crate::upgrades::set_arrow_type`].
    SetArrowType { units: Vec<ObjectId>, fire: bool },
    /// `0x2D` Pack tent (`GameWorld_CmdPackTent` `0x00477170`, re/tents 2.4): `{0x12A}` to the
    /// tent. The original checks nothing; the reimplementation checks like the command bar (A8, [`tent`]).
    PackTent { tent: ObjectId },
    /// `0x2E` Unpack tent (`GameWorld_CmdUnpackTent` `0x004771B0`, re/tents 2.4): the travois
    /// `unit` walks by FINDTO to the destination `(x, z)` (fixed-point tiles); on arrival it sets up the tent
    /// (`0x223`). Check like the command bar (A8, [`tent`]).
    UnpackTent { unit: ObjectId, x: i32, z: i32 },
    /// `0x45` Trade with player (`GameWorld_CmdTradeWithPlayer` `0x00477B90`, re/trading 2.2):
    /// for each unit post `{0x12B, ware, target}` with abort; `ware` 0 to 3 (buy wood/food,
    /// sell wood/food), `target` the partner slot. Check like the partner menu (A8,
    /// [`trade`]).
    TradeWithPlayer {
        ware: i32,
        target: i32,
        units: Vec<ObjectId>,
    },
    /// `0x65` Tribute (`GameWorld_CmdSendTribute` `0x00478840`, re/heroes-resources 2.4, 3.7): the
    /// slot `from` pays every slot 0 to 7 with a set bit in `recipients` (except itself)
    /// `amount` of the resource `resource`. Check like the diplomacy dialog (A8,
    /// [`trade`]).
    Tribute {
        from: i32,
        recipients: u32,
        resource: i32,
        amount: i32,
    },
    /// `0x19` Open gate (`GameWorld_CmdOpenGate` `0x004768A0`, re/arming-gates 2.C): `{0x2303}`
    /// to the gate. The original checks nothing; the reimplementation checks like the command bar (A8, [`gate`]).
    OpenGate { gate: ObjectId },
    /// `0x1A` Close gate (`GameWorld_CmdCloseGate` `0x00476900`, re/arming-gates 2.C): `{0x2403}`
    /// to the gate. Check like the command bar (A8, [`gate`]).
    CloseGate { gate: ObjectId },
    /// `0x1D` Camouflage (`GameWorld_CmdMask` `0x00476A60`, re/masking 2.3): `0x119` per unit with
    /// abort. Check like the command bar (A8, button 434, [`ability`]).
    Mask { units: Vec<ObjectId> },
    /// `0x1E` Uncamouflage (`GameWorld_CmdUnmask` `0x00476AE0`, re/masking 2.3, K-79): `0x219` with
    /// abort; the loop of the original starts at the count instead of at the first ID.
    /// Button 435 ([`ability`]).
    Unmask { units: Vec<ObjectId> },
    /// `0x20` Dig in (`GameWorld_CmdDigIn` `0x00476CC0`, re/masking 2.3): `0x11C` with abort.
    /// Button 437 ([`ability`]).
    DigIn { units: Vec<ObjectId> },
    /// `0x21` Dig out (`GameWorld_CmdDigOut` `0x00476D50`, re/masking 2.3): `0x21C` with abort.
    /// Button 438 ([`ability`]).
    DigOut { units: Vec<ObjectId> },
    /// `0x34` Start healing (`GameWorld_CmdStartHealing` `0x004773F0`, re/healing 2.3): per ID
    /// `0x11D` with abort; a missing ID ends the loop (K-80). Buttons 554/458
    /// ([`ability`]).
    StartHealing { units: Vec<ObjectId> },
    /// `0x35` Stop healing (`GameWorld_CmdStopHealing` `0x00477470`, re/healing 2.3): `0x21D` with
    /// abort, loop like `0x34` (K-80). Buttons 564/459 ([`ability`]).
    StopHealing { units: Vec<ObjectId> },
    /// `0x78` Heal target (`GameWorld_CmdHealTarget` `0x00479010`, re/healing 2.3): `{0x81D, target}` with
    /// abort; missing IDs are skipped (K-310). Without a producer in the command bar (K-89);
    /// check: own, living, module 29 ([`ability`]).
    HealTarget {
        target: ObjectId,
        units: Vec<ObjectId>,
    },
    /// `0x60` Self-healing (`GameWorld_CmdSelfHeal` `0x0047F020`, re/healing 2.3): cost from the
    /// value set of the button (K-91, K-311), full hit points, effect `selbst_heilen.pef`. Button 555
    /// ([`ability`]).
    SelfHeal { unit: ObjectId, button: i32 },
    /// `0x1F` Evoke ghost (`GameWorld_CmdEvokeGhost` `0x00476C50`, re/ghosts-banish 2.5): with
    /// `direct` empty the queue, `{0x11A, button}` with abort, with `direct` empty it again
    /// (C-271). Button 436 ([`ability`]).
    EvokeGhost { unit: ObjectId, button: i32 },
    /// `0x22`-`0x24` Evoke animal (`GameWorld_CmdEvokeAnimal` `0x00475AA0`, re/evoke-aura 2.4):
    /// `{0x120, art, button}` with abort, with `direct` empty the queue. Kind 0/1/2 (wolf, bear,
    /// cat) with button 441/442/443; the type code is `0x22 + art` ([`ability`]).
    EvokeAnimal {
        unit: ObjectId,
        art: u8,
        button: i32,
    },
    /// `0x25` Banish (`GameWorld_CmdBanish` `0x00476DD0`, re/ghosts-banish 2.5): `{0x11F, button}`
    /// with abort, with `direct` empty the queue. Buttons 439/547/550/553/558/562 ([`ability`]).
    Banish { unit: ObjectId, button: i32 },
    /// `0x4E` Drain mana (`GameWorld_CmdDrainMana` `0x00477FD0`, re/spells 2.3): `actor != target`
    /// -> envelope `{0x126, [5, 0, target, button]}` with success `{0xB7F, [target]}` **synchronously** to the
    /// actor (without FIFO, without abort, C-304), with `direct` empty the queue. Button 541
    /// ([`ability`]).
    DrainMana {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x37` Scout area (`GameWorld_CmdScoutArea` `0x0047E170`, re/spells 3.6): cost from the
    /// value set of the button, vision source at the tile `(x, z)` with size `size` (quarter tiles)
    /// for `duration_ms`, `flag != 0` -> also scout; effect `aufklären.pef`, with `direct` empty sound and
    /// queue. Buttons 484/556 with the UI values 30/5000/1 ([`ability`]).
    ScoutArea {
        unit: ObjectId,
        x: i32,
        z: i32,
        size: i32,
        duration_ms: i32,
        flag: i32,
        button: i32,
    },
    /// `0x30` Dynamite on building (`GameWorld_CmdDynamiteBuilding` `0x00477270`,
    /// re/special-attacks 2.4): `{0x124, [target, button]}` with abort to the demolition expert, with
    /// `direct` empty the queue. Button 454, target after the target filter ([`ability`]).
    DynamiteBuilding {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x31` Dynamite on ship (`GameWorld_CmdDynamiteShip` `0x004772D0`): `{0x424, [target,
    /// button]}`, otherwise like `0x30`. Button 455 ([`ability`]).
    DynamiteShip {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x32` Kill unit, close (`GameWorld_CmdKillUnit` `0x00477330`): `{0x125, [target, button]}`
    /// to the killer, otherwise like `0x30`. Buttons 456/548 ([`ability`]).
    KillUnit {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x33` Eliminate unit, ranged (`GameWorld_CmdSnipeUnit` `0x00477390`): `{0x425, [target,
    /// button]}`, otherwise like `0x30`. Buttons 457/546 ([`ability`]).
    SnipeUnit {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x4F` Whirlwind (`GameWorld_CmdWhirlwind` `0x0047E950`, re/special-attacks 2.4):
    /// check and subtract the cost from the value set of the button in the command (K-320), with `direct` sound
    /// and empty the queue, then `{0x131, [0, 0]}` with abort to the smith. Button 542
    /// ([`ability`]).
    Whirlwind { unit: ObjectId, button: i32 },
    /// `0x5F` Damage building (`GameWorld_CmdDamageBuilding` `0x0047EF50`, re/spells 3.7):
    /// only check the cost (without a value set `i32::MAX`, K-87), with `direct` sound and empty the
    /// queue, then `{0x231, [1, 1, target, cost]}` with abort to the smith. Button 551
    /// ([`ability`]).
    DamageBuilding {
        unit: ObjectId,
        target: ObjectId,
        button: i32,
    },
    /// `0x2C` Hire headhunter (`GameWorld_CmdHireHeadhunter` `0x0048A8F0`,
    /// re/headhunter-robbing 2.4, 3.1): kind 0…2 (4100 SPA, 4101 PRI, 4102 SID), target player,
    /// target GUID and the headquarters that hires ([`headhunter`]).
    HireHeadhunter {
        art: i32,
        target_player: i32,
        target_guid: i32,
        building: ObjectId,
    },
    /// `0x4C` Rob corpses (`GameWorld_CmdRobCorpses` `0x0047E890`, re/headhunter-robbing 2.4,
    /// 3.7): check and subtract the cost from the value set of the button in the command (K-91), with
    /// `direct` sound and empty the queue, then `{0x130}` with abort to the robber. Button
    /// 540 ([`ability`]).
    RobCorpses { unit: ObjectId, button: i32 },
    /// `0x10` Die (`GameWorld_CmdDie` `0x00480D90`, re/ki-queries 7.3, E-106): take each object out of the
    /// selection ([`SimEvent::Deselected`]) and send `{0x110}` (module DIE) with abort.
    /// The original checks nothing (no ownership, no building, no death); here only own, living,
    /// non-quartered objects (A8). The sender is the AI ([`ai`]).
    Die { units: Vec<ObjectId> },
    /// `0x59` Building finished (`0x00478EC0`, E-104): shifts the occupancy rectangle by `border`
    /// tiles per side (`0x3303 {border}`, the occupancy bits remain). Border +/-`MAX_BORDER`, the
    /// rectangle stays within the limits of the load check (A7, [`ai`]).
    OccupancyBorder { building: ObjectId, border: i32 },
    /// `0x68` Attack mode of an AI slot (`0x00478B40`, E-101): to all objects of the slot
    /// `{0x611, a, a == 1 ? 0 : 3, b, 0}` (`FUN_004E5610`). Aggressiveness 0 ... 4, stubbornness >= 0
    /// (A7, [`ai`]).
    AiPlayerAttackMode {
        slot: i32,
        aggressiveness: i32,
        stubbornness: i32,
    },
    /// `0x69` Behavior of an AI slot (`0x0047BC70`, E-99, E-100): mode 0 ... 4 in
    /// `CombatState::behavior_mode`, to all objects `0x611`, `0x2100` and with mode 3/2 additionally
    /// `{0x611, 4, 3, 2 or 1, 0}`; buildings without `0x3503` ([`ai`]). Not `SetBehavior`.
    AiPlayerBehavior { slot: i32, mode: i32 },
    /// `0x6A` Walk behavior of an AI slot (`0x00478BD0`, E-102): to all objects `{0x2100, follow,
    /// mobile home, tether, abort, 0}` (`FUN_004E5760`; despite the name "attack flags" no
    /// attack flags). Tether at most [`ai::MAX_AI_TETHER`] (A7).
    AiPlayerWalkFlags {
        slot: i32,
        mobile_home: bool,
        tether: i32,
        follow: bool,
        abort: bool,
    },
    /// `0x6B` Attack mode of an AI object (`0x00478C80`, E-103): like `0x68` for one object, also
    /// dead or quartered; the slot is the one from the ID ([`ai`]).
    AiObjectAttackMode {
        object: ObjectId,
        aggressiveness: i32,
        stubbornness: i32,
    },
    /// `0x6C` Walk behavior of an AI object (`0x00478D20`, E-102, E-103): like `0x6A` for one
    /// object ([`ai`]).
    AiObjectWalkFlags {
        object: ObjectId,
        mobile_home: bool,
        tether: i32,
        follow: bool,
        abort: bool,
    },
    /// `0x7E` Credit resource (`0x00479780`, `Player_AddResource` `0x004FAE30`, E-105):
    /// `amount << 12` to resource 0 ... 4 (gold, wood, food, population, living space), without limit,
    /// also negative, without message. Only the own slot, only AI control kind ([`ai`]).
    AddResource {
        slot: i32,
        resource: i32,
        amount: i32,
    },
    /// AI control command: start module `module` on the engine of slot `slot` with `args`,
    /// with `sync` immediately to the end (`.abl` `n0`, `ki::start::run_sync`), otherwise as a
    /// thread (`n1`, `ki::start::spawn_thread`). At most [`crate::ki::run::MAX_START_ARGS`]
    /// arguments (A7). Takes effect at the tick boundary; always rejected in the AI step ([`crate::ki::run`]).
    KiStart {
        slot: u8,
        module: u32,
        args: Vec<i32>,
        sync: bool,
    },
    /// AI control command: pause or resume the whole AI (`KiState::paused`).
    KiPause { paused: bool },
    /// AI control command: with the AI paused, `passes` passes (1 ...
    /// [`crate::ki::run::MAX_STEP_PASSES`]) in which every thread of the engine of slot `slot`
    /// makes exactly one step with `dt = 20` (debugger `0x4B0`); rejected while the AI is running.
    KiStep { slot: u8, passes: u32 },
    /// `0x56` Player result (`GameEngine_SetPlayerResult` `0x00430CE0`, re/commands 4.6):
    /// slot `slot` loses (`result` 1, also giving up via `0x27`) or wins (2). Checked against the
    /// original (A8): slot 0 ... 7 and result 1 or 2; the sender may name any slot
    /// (like script and cheat). Effect in [`crate::game_end::set_player_result`].
    SetPlayerResult { slot: u8, result: i32 },
    /// `0x42` Start animation lock (`GameWorld_CmdStartAnimationLock` `0x004778D0`,
    /// re/commands 9): aborts all modules of the unit, sets the animation `anim` and locks for
    /// `seconds` seconds (`−1`: until `0x43`; `0`: time marker 1 of the animation, E-212). The original
    /// sends it only from script action 157; the input checks as usual (A8, [`owns`]), the
    /// script path takes any existing object (E-214). Effect in
    /// [`crate::world::start_animation_lock`]; with `direct` then empty the queue, without sound.
    StartAnimationLock {
        unit: ObjectId,
        anim: i32,
        seconds: i32,
    },
    /// `0x43` Release animation lock (`GameWorld_CmdStopAnimationLock` `0x00477910`): deletes the
    /// lock time and the shared status bit also for foreign setters (E-213), without event bit 3
    /// (E-186). Effect in [`crate::world::stop_animation_lock`].
    StopAnimationLock { unit: ObjectId },
    /// Debug "skip dialog" (viewer and control channel, no counterpart in the original):
    /// sets the skip flag of the running dialog container; the next dialog tick ends it without a tick
    /// (`ScriptContainer_TickUnlessSkipped` `0x0056D900` returns -1, re/campaign-ui 4). Any origin, no A8
    /// (the dialog belongs to nobody); rejected without a dialog.
    /// Effect in [`crate::script::skip_dialog`].
    ScriptSkipDialog,
    /// Debug "destroy objects" (acceptance helper, no counterpart in the original):
    /// units like `Die` (`0x10`: out of the selection, `0x110` with abort), buildings through the normal
    /// destruction ([`crate::module::building::destroy::destroy`], event bit 2). Only own objects of the
    /// sender (A8, [`owns`]); foreign, missing, dying and quartered ones are skipped.
    DebugDestroy { objects: Vec<ObjectId> },
    /// Host command "selection mirror" (E-228/E-230; no counterpart in the original):
    /// the host reports its selection when the set changes and the level uses conditions
    /// 35-37. Only origin `External` (host); no ownership check (also foreign objects, like the
    /// selection list of the original); more than 30 entries and the invalid ID `ObjectId::NONE` are
    /// rejected (the mirror stays unchanged). Effect at the tick boundary before the world tick
    /// ([`crate::script::ui_mirror`]).
    ScriptSelection { ids: Vec<ObjectId> },
    /// Host command "last button" (E-229; no counterpart in the original):
    /// `word` is the value of the command bar event (condition 38). Only origin `External`
    /// ([`crate::script::ui_mirror`]).
    ScriptButton { word: i32 },
    /// `0x5C` Signal (sender `0x0044CB6E…0x0044CBE7`, execution `0x0048D4D6…0x0048D539`):
    /// location `(x, z)` in **tiles** (camera target divided by the tile size, clamped at 0), `mask` the
    /// receivers (chat mask of the sender, bit per slot), `player` the slot the ping takes its color from.
    /// Like the original stateless and **without** checking `player` against the sender (there is nothing to
    /// protect); the effect is the event [`SimEvent::Signal`] to the host (sound, ping, stack).
    Signal {
        player: i32,
        mask: u32,
        x: i32,
        z: i32,
    },
    /// `0x5D` Chat (sender `FUN_004B9940`, execution `GameWorld_HandleChatCheat` `0x00484C30`,
    /// then `FUN_0042E980`): `player` is the slot field (the sender, A8: otherwise rejected), `mask` the
    /// receivers (bit per slot: window 1 all, 2 the friends including the sender, 3 the chat mask),
    /// `text` one UTF-16 unit per character (in the original a 32-bit word). First the cheats
    /// ([`chat::cheat`]); otherwise [`SimEvent::Chat`] to the host.
    Chat {
        player: i32,
        mask: u32,
        text: Vec<u16>,
    },
}

/// Train command: forward (`0x29`), backward (`0x2A`) or halt (`0x2B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrainOp {
    Forward,
    Backward,
    Stop,
}

impl CommandKind {
    /// Command type of the original (`0x500000xx`).
    pub fn type_code(&self) -> u32 {
        0x5000_0000
            | match self {
                Self::PlaceBuilding { .. } => 0x01,
                Self::ConstructAt { .. } => 0x02,
                Self::MineGold { .. } => 0x03,
                Self::Move { .. } => 0x04,
                Self::ChopWood { .. } => 0x06,
                Self::Harvest { .. } => 0x07,
                Self::Deliver { .. } => 0x09,
                Self::EnqueueProduction { .. } => 0x0E,
                Self::CancelProduction { .. } => 0x0F,
                Self::Quarter { .. } => 0x13,
                Self::Unquarter { .. } => 0x14,
                Self::UnquarterAll { .. } => 0x15,
                Self::PlaceBuildings { .. } => 0x18,
                Self::SetRallyPoint { .. } => 0x27,
                Self::Halt { .. } => 0x3C,
                Self::SharedVision { .. } => 0x63,
                Self::Diplomacy { .. } => 0x64,
                Self::SellBuildings { .. } => 0x67,
                Self::Queue { cyclic: false, .. } => 0x7B,
                Self::Queue { cyclic: true, .. } => 0x7C,
                Self::Fish { .. } => 0x7D,
                Self::NextTree { .. } => 0x7F,
                Self::DebugCompleteOrder { .. } => DEBUG_COMPLETE_ORDER,
                Self::Attack { pos: None, .. } => 0x11,
                Self::Attack { pos: Some(_), .. } => 0x71,
                Self::SetBehavior { .. } => 0x12,
                Self::GarrisonAttack { .. } => 0x5B,
                Self::Hunt { gut: true, .. } => 0x16,
                Self::Hunt { gut: false, .. } => 0x17,
                Self::Follow { .. } => 0x74,
                Self::SetFormation { .. } => 0x44,
                Self::Scatter { .. } => 0x75,
                Self::Mount { .. } => 0x0A,
                Self::Dismount { .. } => 0x0B,
                Self::BoardBoat { .. } => 0x0C,
                Self::LeaveBoat { .. } => 0x0D,
                Self::Dive { .. } => 0x1B,
                Self::Surface { .. } => 0x1C,
                Self::Train {
                    op: TrainOp::Forward,
                    ..
                } => 0x29,
                Self::Train {
                    op: TrainOp::Backward,
                    ..
                } => 0x2A,
                Self::Train {
                    op: TrainOp::Stop, ..
                } => 0x2B,
                Self::ApplyUpgrade { .. } => 0x39,
                Self::RemoveUpgrade { .. } => 0x3A,
                Self::CounterUpgrade { .. } => 0x2F,
                Self::CastSpell { .. } => 0x36,
                Self::ExecTempUpgrade { .. } => 0x5E,
                Self::ToggleArming { arm: true, .. } => 0x38,
                Self::ToggleArming { arm: false, .. } => 0x72,
                Self::SetArrowType { fire: true, .. } => 0x40,
                Self::SetArrowType { fire: false, .. } => 0x41,
                Self::PackTent { .. } => 0x2D,
                Self::UnpackTent { .. } => 0x2E,
                Self::TradeWithPlayer { .. } => 0x45,
                Self::Tribute { .. } => 0x65,
                Self::OpenGate { .. } => 0x19,
                Self::CloseGate { .. } => 0x1A,
                Self::Mask { .. } => 0x1D,
                Self::Unmask { .. } => 0x1E,
                Self::DigIn { .. } => 0x20,
                Self::DigOut { .. } => 0x21,
                Self::StartHealing { .. } => 0x34,
                Self::StopHealing { .. } => 0x35,
                Self::HealTarget { .. } => 0x78,
                Self::SelfHeal { .. } => 0x60,
                Self::EvokeGhost { .. } => 0x1F,
                Self::EvokeAnimal { art, .. } => 0x22 + u32::from((*art).min(2)),
                Self::Banish { .. } => 0x25,
                Self::DrainMana { .. } => 0x4E,
                Self::ScoutArea { .. } => 0x37,
                Self::DynamiteBuilding { .. } => 0x30,
                Self::DynamiteShip { .. } => 0x31,
                Self::KillUnit { .. } => 0x32,
                Self::SnipeUnit { .. } => 0x33,
                Self::Whirlwind { .. } => 0x4F,
                Self::DamageBuilding { .. } => 0x5F,
                Self::HireHeadhunter { .. } => 0x2C,
                Self::RobCorpses { .. } => 0x4C,
                Self::Die { .. } => 0x10,
                Self::OccupancyBorder { .. } => 0x59,
                Self::AiPlayerAttackMode { .. } => 0x68,
                Self::AiPlayerBehavior { .. } => 0x69,
                Self::AiPlayerWalkFlags { .. } => 0x6A,
                Self::AiObjectAttackMode { .. } => 0x6B,
                Self::AiObjectWalkFlags { .. } => 0x6C,
                Self::AddResource { .. } => 0x7E,
                Self::KiStart { .. } => KI_START,
                Self::KiPause { .. } => KI_PAUSE,
                Self::KiStep { .. } => KI_STEP,
                Self::SetPlayerResult { .. } => 0x56,
                Self::StartAnimationLock { .. } => 0x42,
                Self::StopAnimationLock { .. } => 0x43,
                Self::ScriptSkipDialog => SCRIPT_SKIP_DIALOG,
                Self::DebugDestroy { .. } => DEBUG_DESTROY,
                Self::ScriptSelection { .. } => SCRIPT_SELECTION,
                Self::ScriptButton { .. } => SCRIPT_BUTTON,
                Self::Signal { .. } => 0x5C,
                Self::Chat { .. } => 0x5D,
            }
    }
}

/// Result of [`execute`]: the command took full effect, partial effect (only some of the named
/// units, with the reason for the rest) or none at all (with the reason as an error message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Accepted,
    Partial(String),
    Rejected(String),
}

/// Where a command comes from: from outside (input, network, control channel), from the AI or from a
/// script action. The AI takes its own check path, the script path checks neither ownership nor the
/// UI ([`script_path`]); the origin is held transiently in `Sim::origin` (readable and writable only
/// within the crate) and is set by [`crate::player::submit`], [`crate::player::submit_script`] and
/// the AI step, never from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    External,
    Ki,
    /// Script action: only from the simulation ([`crate::player::submit_script`]).
    Script,
}

/// Transient verdict on the command that is currently running: not in the state, not in the
/// savegame, not in the checksum. `RefCell` because [`owns`] only has `&Sim`.
#[derive(Debug, Default)]
pub(crate) struct Verdict {
    /// Reason that rejects the whole command ([`reject`]; the first one wins).
    pub(crate) whole: Option<String>,
    /// Reason why [`owns`] filtered out a unit (the first one wins).
    pub(crate) filtered: Option<String>,
    /// Number of units that [`owns`] let through.
    pub(crate) passed: u32,
}

impl Verdict {
    fn outcome(self) -> Outcome {
        if let Some(why) = self.whole {
            Outcome::Rejected(why)
        } else if let Some(why) = self.filtered {
            if self.passed > 0 {
                Outcome::Partial(why)
            } else {
                Outcome::Rejected(why)
            }
        } else {
            Outcome::Accepted
        }
    }
}

/// A command with the player slot that issues it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub player: u8,
    pub kind: CommandKind,
}

/// Entry of the command queue of an object (`GameObject_EnqueueCommand` `0x0046CC80`):
/// wrapper `0x7B` (one-time) or `0x7C` (cyclic) around a command for exactly this unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedCommand {
    pub cyclic: bool,
    pub command: Command,
}

/// Maximum number of entries of a command queue.
pub const MAX_QUEUED_COMMANDS: usize = 25;

/// Action of the command acknowledgment "Execute" (`Unit_PlayResponseSound(u, 0xC)`).
pub const RESPONSE_EXECUTE: i32 = 12;
/// Action of the attack acknowledgment (`GameWorld_CmdAttack` `0x0047D330`).
pub const RESPONSE_ATTACK: i32 = 14;

/// Command acknowledgment (`Unit_PlayResponseSound` `0x0047BBC0`): only an event to the presentation
/// ([`SimEvent::UnitResponse`]) - no state, no randomness, not in the checksum. The checks
/// of the original (cutscene, `UnitResponse`, local player) and the counter `+0x2424`
/// belong to the presentation. The event carries owner and type GUID at the time of the call; without
/// an object (all callers pass an existing unit) it is omitted.
pub(crate) fn respond(sim: &mut Sim, unit: ObjectId, action: i32) {
    let Some(o) = sim.object(unit) else {
        return;
    };
    let (owner, guid) = (o.owner(), o.guid);
    sim.state.events.push(SimEvent::UnitResponse {
        unit,
        action,
        owner,
        guid,
    });
}

/// `GameWorld_ExecuteCommand` (`0x0048D340`): `direct` (`GameWorld+0x78`) applies to commands from the
/// input (they empty the command queues of the units); `false` from the queue.
/// Additionally checked against the original (A8): the slot of the sender (0-7), units and
/// buildings only of the sender, diplomacy and vision only for the own slot, the stance 0 ... 4,
/// the economy commands according to [`validate`].
///
/// The result ([`Outcome`]) is derived from the [`Verdict`]: a reason for the whole
/// command ([`reject`]) -> `Rejected`; otherwise [`owns`] filtered out units -> `Partial` if
/// others passed, otherwise `Rejected`; otherwise `Accepted`. The verdict is transient (not in the
/// state); a nested call (e.g. from a module in the tick) leaves that of the outer one
/// untouched.
pub fn execute(sim: &mut Sim, command: &Command, direct: bool) -> Outcome {
    let outer = sim.verdict.replace(Verdict::default());
    dispatch(sim, command, direct);
    sim.verdict.replace(outer).outcome()
}

/// Rejection reason of the commands that must not come from machines in network play.
pub const NET_LOCKED: &str = "locked in network play";

/// Lock in network play (`SimState::network`, only for the origin [`Origin::External`]): debug aids, AI control and
/// result commands that are not the giving up of the own slot. From clients the original sends only
/// `0x56 {slot, 1}` (giving up, dropout); `victory` for oneself would defeat all non-allies
/// (`game_end::set_player_result`). The wrappers (`Queue`) are checked by the inner command.
fn net_lock(kind: &CommandKind, player: u8) -> Option<&'static str> {
    let plain = match kind {
        CommandKind::Queue { inner, .. } => inner.as_ref(),
        kind => kind,
    };
    match plain {
        CommandKind::DebugCompleteOrder { .. }
        | CommandKind::DebugDestroy { .. }
        | CommandKind::KiStart { .. }
        | CommandKind::KiPause { .. }
        | CommandKind::KiStep { .. }
        | CommandKind::ScriptSkipDialog => Some("Debug and AI command"),
        CommandKind::SetPlayerResult { slot, result } if !(*slot == player && *result == 1) => {
            Some("Result only for the own slot (giving up)")
        }
        _ => None,
    }
}

fn dispatch(sim: &mut Sim, command: &Command, direct: bool) {
    let player = command.player;
    if usize::from(player) >= PLAYER_SLOTS {
        reject(
            sim,
            player,
            "Command",
            format!("slot {player} discarded (allowed 0 to 7)"),
        );
        return;
    }
    if sim.state.network
        && sim.origin == Origin::External
        && let Some(what) = net_lock(&command.kind, player)
    {
        reject(sim, player, what, NET_LOCKED.to_owned());
        return;
    }
    // Stance only 0 ... 4 (A8): the original does not check the mode (`0x004762E0`), the UI
    // produces only 0 ... 4; for a stance not in 0…4 the original leaves words of the messages `0x611`/`0x2100`
    // unset (heap remnants, not deterministic, C-341).
    let plain = match &command.kind {
        CommandKind::Queue { inner, .. } => inner.as_ref(),
        kind => kind,
    };
    if let CommandKind::SetBehavior { mode, .. } = plain
        && !(0..=4).contains(mode)
    {
        reject(
            sim,
            player,
            "Stance",
            format!("stance {mode} (allowed 0 to 4)"),
        );
        return;
    }
    match &command.kind {
        CommandKind::Move {
            units,
            x,
            z,
            target,
            formation,
            direction,
            spread,
            flags,
        } => {
            // Several units form a group (type 0/5 distribute the targets around the
            // centroid, re/groups-formations 3.1); acknowledgment once, directly, for the unit selected
            // first (`MoveCommand_GetUnit(n − 1)`, the IDs are in reverse order there).
            group::move_selection(
                sim,
                player,
                units,
                (*x, *z, *target, *formation, *direction, *spread, *flags),
                direct,
            );
        }
        CommandKind::Halt { units } => {
            for &unit in units {
                if !owns(sim, player, unit) {
                    continue;
                }
                abort_all_modules(sim, unit);
                if direct && let Some(o) = sim.object_mut(unit) {
                    o.commands.clear();
                }
            }
        }
        // `GameWorld_CmdSetSharedVision` (`0x00478820`) checks nothing. The reimplementation requires the sender in
        // `p` or `q` (A8): `p` = sender is the AI (`0x97`, "I see what q sees"), `q` = sender is the
        // vision checkbox of the diplomacy dialog (`0x0042EDD0`: `{0x63, N, l, on}`, "N sees what I see", O-322).
        CommandKind::SharedVision { p, q, on } => {
            let me = i32::from(player);
            if *p == me || *q == me {
                sim.state.diplomacy.set_vision(*p, *q, *on);
            } else {
                reject(
                    sim,
                    player,
                    "Share vision",
                    format!("neither slot {p} nor {q} is the sender"),
                );
            }
        }
        CommandKind::Diplomacy { p, q, mode } => {
            if *p != i32::from(player) {
                reject(
                    sim,
                    player,
                    "Diplomacy",
                    format!("slot {p} is not the sender"),
                );
                return;
            }
            let (friend, enemy) = match mode {
                0 => (true, false),
                1 => (false, true),
                2 => (false, false),
                _ => {
                    reject(
                        sim,
                        player,
                        "Diplomacy",
                        format!("mode {mode} does not exist"),
                    );
                    return;
                }
            };
            let d = &mut sim.state.diplomacy;
            d.set_enemy(*p, *q, enemy);
            d.set_friend(*p, *q, friend);
            // `GameWorld_CmdSetDiplomacy` (`0x00478720`): 231/232/233 to all, on every valid
            // command, also without a change of the relationship (O-247).
            crate::economy::announce_all(
                sim,
                231 + *mode,
                (-1, -1),
                crate::economy::message::flags::players(*p as u32, *q as u32),
            );
        }
        CommandKind::Queue { inner, .. } if matches!(**inner, CommandKind::Queue { .. }) => {
            reject(sim, player, "Command wrapper", "wrapper in a wrapper");
        }
        CommandKind::Queue { cyclic, inner } => execute_queued(sim, player, *cyclic, inner),
        CommandKind::SetFormation { units, formation } => {
            group::set_formation(sim, player, units, *formation, direct);
        }
        CommandKind::Scatter { units } => group::scatter(sim, player, units, direct),
        kind @ (CommandKind::Mount { .. }
        | CommandKind::Dismount { .. }
        | CommandKind::BoardBoat { .. }
        | CommandKind::LeaveBoat { .. }
        | CommandKind::Dive { .. }
        | CommandKind::Surface { .. }) => riding::execute(sim, player, kind, direct),
        CommandKind::Train { train, op, target } => {
            train::execute(sim, player, *train, *op, *target)
        }
        kind @ (CommandKind::ApplyUpgrade { .. }
        | CommandKind::RemoveUpgrade { .. }
        | CommandKind::CounterUpgrade { .. }
        | CommandKind::CastSpell { .. }
        | CommandKind::ExecTempUpgrade { .. }
        | CommandKind::ToggleArming { .. }
        | CommandKind::SetArrowType { .. }) => upgrade::execute(sim, player, kind, direct),
        kind @ (CommandKind::PackTent { .. } | CommandKind::UnpackTent { .. }) => {
            tent::execute(sim, player, kind, direct)
        }
        kind @ (CommandKind::TradeWithPlayer { .. } | CommandKind::Tribute { .. }) => {
            trade::execute(sim, player, kind, direct)
        }
        kind @ (CommandKind::OpenGate { .. } | CommandKind::CloseGate { .. }) => {
            gate::execute(sim, player, kind, direct)
        }
        kind @ (CommandKind::Mask { .. }
        | CommandKind::Unmask { .. }
        | CommandKind::DigIn { .. }
        | CommandKind::DigOut { .. }
        | CommandKind::StartHealing { .. }
        | CommandKind::StopHealing { .. }
        | CommandKind::HealTarget { .. }
        | CommandKind::SelfHeal { .. }
        | CommandKind::EvokeGhost { .. }
        | CommandKind::EvokeAnimal { .. }
        | CommandKind::Banish { .. }
        | CommandKind::DrainMana { .. }
        | CommandKind::ScoutArea { .. }
        | CommandKind::DynamiteBuilding { .. }
        | CommandKind::DynamiteShip { .. }
        | CommandKind::KillUnit { .. }
        | CommandKind::SnipeUnit { .. }
        | CommandKind::Whirlwind { .. }
        | CommandKind::DamageBuilding { .. }
        | CommandKind::RobCorpses { .. }) => ability::execute(sim, player, kind, direct),
        kind @ CommandKind::HireHeadhunter { .. } => headhunter::execute(sim, player, kind),
        kind @ (CommandKind::Die { .. }
        | CommandKind::OccupancyBorder { .. }
        | CommandKind::AiPlayerAttackMode { .. }
        | CommandKind::AiPlayerBehavior { .. }
        | CommandKind::AiPlayerWalkFlags { .. }
        | CommandKind::AiObjectAttackMode { .. }
        | CommandKind::AiObjectWalkFlags { .. }
        | CommandKind::AddResource { .. }) => ai::execute(sim, player, kind),
        kind @ (CommandKind::KiStart { .. }
        | CommandKind::KiPause { .. }
        | CommandKind::KiStep { .. }) => crate::ki::run::control(sim, player, kind),
        CommandKind::SetPlayerResult { slot, result } => {
            crate::game_end::execute(sim, player, *slot, *result)
        }
        // Acceptance helpers without a counterpart in the original: no A8 for the dialog.
        CommandKind::ScriptSkipDialog => {
            if let Err(why) = crate::script::skip_dialog(sim) {
                reject(sim, player, "Skip dialog", why);
            }
        }
        kind @ CommandKind::DebugDestroy { .. } => ai::execute(sim, player, kind),
        // Host commands for conditions 35-38 (origin host only).
        kind @ (CommandKind::ScriptSelection { .. } | CommandKind::ScriptButton { .. }) => {
            crate::script::ui_mirror::execute(sim, player, kind);
        }
        // `0x5C` (`0x0048D4D6`): no state, only the event; `player` unchecked.
        CommandKind::Signal { player, mask, x, z } => {
            let color = crate::setup::player_color(sim, *player);
            sim.state.events.push(SimEvent::Signal {
                player: *player,
                color,
                mask: *mask,
                tile: (*x, *z),
            });
        }
        // `0x5D` (`0x00484C30`, `FUN_0042E980`): cheats, otherwise the chat line to the host.
        CommandKind::Chat {
            player: field,
            mask,
            text,
        } => chat::execute(sim, player, *field, *mask, text),
        // `0x42`/`0x43`: no response sound; with `direct` the handler empties the queue afterwards.
        CommandKind::StartAnimationLock {
            unit,
            anim,
            seconds,
        } => {
            if owns(sim, player, *unit) {
                crate::world::start_animation_lock(sim, *unit, *anim, *seconds);
                if direct && let Some(o) = sim.object_mut(*unit) {
                    o.commands.clear();
                }
            }
        }
        CommandKind::StopAnimationLock { unit } => {
            if owns(sim, player, *unit) {
                crate::world::stop_animation_lock(sim, *unit);
                if direct && let Some(o) = sim.object_mut(*unit) {
                    o.commands.clear();
                }
            }
        }
        kind => economy::execute(sim, player, kind, direct),
    }
}

thread_local! {
    /// Currently reading the content of a command wrapper ([`deserialize_queue_inner`]).
    static READING_QUEUE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Read the content of a command wrapper (exception A7): a wrapper inside it is an error **before** its
/// content is read - otherwise deep nesting (savegame, replay, control channel) would blow the stack
/// when reading recursively. The UI produces only one wrapper.
fn deserialize_queue_inner<'de, D>(d: D) -> Result<Box<CommandKind>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    /// Resets the flag also on an abort.
    struct Reading;
    impl Drop for Reading {
        fn drop(&mut self) {
            READING_QUEUE.with(|r| r.set(false));
        }
    }
    if READING_QUEUE.with(|r| r.replace(true)) {
        return Err(serde::de::Error::custom(
            "command wrapper in a command wrapper",
        ));
    }
    let _reading = Reading;
    Box::<CommandKind>::deserialize(d)
}

/// Units of a queueable command and the command for exactly one of them.
type PerUnit<'a> = (&'a [ObjectId], Box<dyn Fn(ObjectId) -> CommandKind>);

/// `GameWorld_ExecuteQueuedCommand` (`0x0048DD00`): place building (`0x01`) places the building immediately and
/// queues only the workers; build, gold, wood, harvest and deliver (`0x02/0x03/0x06/0x07/0x09`)
/// and move are queued per unit; everything else runs immediately (without `direct`, the
/// queues stay as they are - quirk, e.g. fish `0x7D`, quarter `0x13`, formation
/// `0x44` and scatter `0x75`).
/// Behavior (`0x12`) is queued per unit like move (`{type, 5, 0x12, 3, mode, 1, id}`);
/// the original never queues attack (`0x11`, `0x71`) and garrison attack (`0x5B`) and never clears
/// the queue. Only existing units of the sender are queued (A8); the target is not checked until
/// execution.
fn execute_queued(sim: &mut Sim, player: u8, cyclic: bool, inner: &CommandKind) {
    let per_unit: Option<PerUnit> = match inner {
        CommandKind::PlaceBuilding { .. } => {
            economy::place_single_building(sim, player, inner, false, true);
            return;
        }
        // Per unit a move without target radius, direction and flags (running and Ctrl are
        // lost, quirk).
        CommandKind::Move { units, x, z, .. } => {
            let (x, z) = (*x, *z);
            Some((
                units,
                Box::new(move |u| CommandKind::Move {
                    units: vec![u],
                    x,
                    z,
                    target: -1,
                    formation: 0,
                    direction: NO_DIRECTION,
                    spread: NO_SPREAD,
                    flags: 0,
                }),
            ))
        }
        CommandKind::ConstructAt { building, units } => {
            let building = *building;
            Some((
                units,
                Box::new(move |u| CommandKind::ConstructAt {
                    building,
                    units: vec![u],
                }),
            ))
        }
        CommandKind::MineGold { mine, units } => {
            let mine = *mine;
            Some((
                units,
                Box::new(move |u| CommandKind::MineGold {
                    mine,
                    units: vec![u],
                }),
            ))
        }
        CommandKind::ChopWood { tree, units } => {
            let tree = *tree;
            Some((
                units,
                Box::new(move |u| CommandKind::ChopWood {
                    tree,
                    units: vec![u],
                }),
            ))
        }
        CommandKind::Harvest { farm, units } => {
            let farm = *farm;
            Some((
                units,
                Box::new(move |u| CommandKind::Harvest {
                    farm,
                    units: vec![u],
                }),
            ))
        }
        CommandKind::Deliver { building, units } => {
            let building = *building;
            Some((
                units,
                Box::new(move |u| CommandKind::Deliver {
                    building,
                    units: vec![u],
                }),
            ))
        }
        CommandKind::SetBehavior { units, mode } => {
            let mode = *mode;
            Some((
                units,
                Box::new(move |u| CommandKind::SetBehavior {
                    units: vec![u],
                    mode,
                }),
            ))
        }
        _ => None,
    };
    let Some((units, make)) = per_unit else {
        let inner = Command {
            player,
            kind: inner.clone(),
        };
        // No new `execute`: the verdict of the wrapper is that of the inner command.
        dispatch(sim, &inner, false);
        return;
    };
    for &unit in units {
        if !owns(sim, player, unit) {
            continue;
        }
        let entry = QueuedCommand {
            cyclic,
            command: Command {
                player,
                kind: make(unit),
            },
        };
        enqueue(sim, unit, entry);
    }
}

/// Walk flags of a move command from the command flags: `0x201`, `|0x20` with bit 1, `|0x100` with
/// bit 0, `|0x400` with bit 2 (re/walk 15.1).
pub(crate) fn move_walk_flags(flags: i32) -> i32 {
    let mut walk_flags = 0x201;
    if flags & 2 != 0 {
        walk_flags |= 0x20;
    }
    if flags & 1 != 0 {
        walk_flags |= 0x100;
    }
    if flags & 4 != 0 {
        walk_flags |= 0x400;
    }
    walk_flags
}

/// Move for one unit (`GameWorld_CmdMoveSelection` with n = 1): directly empty the
/// command queue, then FINDTO with the walk flags `0x201` (+ `0x20`/`0x100`/`0x400`)
/// as a command envelope with abort into the FIFO; success and failure report `0x97F` (idle).
fn move_unit(
    sim: &mut Sim,
    unit: ObjectId,
    (x, z, target, direction, flags): (i32, i32, i32, i32, i32),
    direct: bool,
) {
    // Order as in the original (re/walk 15.1): if the unit has an object, the queue is emptied
    // before the walk module is checked (a unit without walk stays without commands).
    let Some(o) = sim.object_mut(unit) else {
        return;
    };
    if direct {
        o.commands.clear();
    }
    if !o.has_module(0, false) {
        return;
    }
    let walk_flags = move_walk_flags(flags);
    let idle = Message::new(0x97F, &[]);
    let env = Envelope {
        msg: find_to_msg(x, z, target, direction, 0, flags & 1 != 0, walk_flags),
        success: Some(idle.clone()),
        fault: Some(idle),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    post_command(sim, unit, env, true);
}

/// Rejection of the whole command (A8): log line and verdict ([`Verdict::whole`], the first
/// reason wins). The verdict is transient; no event is created here.
pub(crate) fn reject(sim: &Sim, player: u8, what: &str, why: impl std::fmt::Display) {
    log::debug!("Command \"{what}\" from slot {player} rejected: {why}");
    reject_as(sim, format!("{what}: {why}"));
}

/// Like [`reject`] with the reason `reason` verbatim (without the command name in front).
pub(crate) fn reject_as(sim: &Sim, reason: String) {
    let mut v = sim.verdict.borrow_mut();
    if v.whole.is_none() {
        v.whole = Some(reason);
    }
}

/// Script path (re/commands 9): the command comes from a
/// script action ([`Origin::Script`], only through [`crate::player::submit_script`]). On this path the original checks
/// neither ownership nor selectability, menu button, button lock, target or MY filter: all
/// handlers fetch their objects with `GetObjectById(id, 0)` (`0x00475300`, E-214). For input and AI
/// (A8), the checks that are omitted here are therefore skipped only behind `!script_path(sim)`; checks
/// of the handler itself and validity checks (A7) apply to all origins.
pub(crate) fn script_path(sim: &Sim) -> bool {
    sim.origin == Origin::Script
}

/// The unit exists, belongs to the sender and is selectable: quartered (`QUARTERED`) and
/// dying (`DIED`) units cannot be selected by the UI
/// (re/garrison-and-commands), so they get no commands (A8). Unquartering and
/// quartering are addressed to the container and check it instead of the unit.
///
/// On the script path ([`script_path`]) it is enough that the object exists (generation matches): foreign,
/// quartered and dying ones count too (E-214, `GetObjectById(id, 0)`).
///
/// The verdict counts along: a unit that is let through raises [`Verdict::passed`], a filtered
/// one sets [`Verdict::filtered`] (the first reason stays).
pub(crate) fn owns(sim: &Sim, player: u8, unit: ObjectId) -> bool {
    let ok = sim.object(unit).is_some_and(|o| {
        script_path(sim)
            || (unit.slot() == player && o.status & (status::QUARTERED | status::DIED) == 0)
    });
    let mut v = sim.verdict.borrow_mut();
    if ok {
        v.passed = v.passed.saturating_add(1);
    } else {
        let why = if script_path(sim) {
            format!("object {} not present", unit.0)
        } else {
            format!("unit {} not present, foreign, quartered or dead", unit.0)
        };
        log::debug!("Command from slot {player}: {why}");
        if v.filtered.is_none() {
            v.filtered = Some(why);
        }
    }
    ok
}

/// Rejection of **one element** of a multi-object command: the loop continues with the rest.
/// Log line and [`Verdict::filtered`] (the first reason wins); without elements that went through,
/// this becomes `Rejected`, otherwise `Partial`. Where the whole command ends, [`reject`] applies.
pub(crate) fn skip(sim: &Sim, player: u8, what: &str, why: impl std::fmt::Display) {
    log::debug!("Command \"{what}\" from slot {player}: element skipped: {why}");
    let mut v = sim.verdict.borrow_mut();
    if v.filtered.is_none() {
        v.filtered = Some(format!("{what}: {why}"));
    }
}

/// An element passed its check without [`owns`] counting it (e.g. a building via
/// `own_building`): raises [`Verdict::passed`].
pub(crate) fn pass(sim: &Sim) {
    let mut v = sim.verdict.borrow_mut();
    v.passed = v.passed.saturating_add(1);
}

/// Like [`skip`] for an element that [`owns`] already let through and whose further check failed:
/// takes back the count of `owns` so that it does not count as an effective element.
pub(crate) fn skip_passed(sim: &Sim, player: u8, what: &str, why: impl std::fmt::Display) {
    {
        let mut v = sim.verdict.borrow_mut();
        v.passed = v.passed.saturating_sub(1);
    }
    skip(sim, player, what, why);
}

/// `GameObject_EnqueueCommand` (`0x0046CC80`): append at the end, silently discarded at 25 entries.
pub fn enqueue(sim: &mut Sim, unit: ObjectId, entry: QueuedCommand) {
    if let Some(o) = sim.object_mut(unit)
        && o.commands.len() < MAX_QUEUED_COMMANDS
    {
        o.commands.push(entry);
    }
}

/// `GameObject_ProcessCommandQueue` (`0x0046CCF0`): one entry at the read pointer; one-time entries are
/// removed (the pointer stays), cyclic ones stay and the pointer advances (at the end back to 0).
pub fn process_queue(sim: &mut Sim, unit: ObjectId) -> bool {
    let Some(o) = sim.object_mut(unit) else {
        return false;
    };
    if o.commands.is_empty() {
        return false;
    }
    if o.command_cursor < 0 || o.command_cursor as usize >= o.commands.len() {
        o.command_cursor = 0;
    }
    let at = o.command_cursor as usize;
    let entry = if o.commands[at].cyclic {
        o.command_cursor += 1;
        o.commands[at].clone()
    } else {
        o.commands.remove(at)
    };
    execute(sim, &entry.command, false);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Move` to the tile `(2, 2)` for the named units.
    fn walk(units: &[ObjectId]) -> CommandKind {
        CommandKind::Move {
            units: units.to_vec(),
            x: 2 << 12,
            z: 2 << 12,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        }
    }

    fn outcome(sim: &mut Sim, player: u8, kind: CommandKind) -> Outcome {
        execute(sim, &Command { player, kind }, true)
    }

    /// The command was rejected, and the reason contains `keyword`.
    fn assert_rejected_with(outcome: Outcome, keyword: &str) {
        match outcome {
            Outcome::Rejected(why) => {
                assert!(why.contains(keyword), "\"{why}\" without \"{keyword}\"")
            }
            other => panic!("expected Rejected (\"{keyword}\"), got {other:?}"),
        }
    }

    /// In network play, debug and AI control commands from outside are locked, the state stays the same;
    /// without the network they work as usual.
    #[test]
    fn network_rejects_debug_and_ki_control() {
        let rules = testkit::rules();
        let kinds = || {
            vec![
                CommandKind::DebugCompleteOrder {
                    building: ObjectId(0x800),
                    index: 0,
                    guid: 1100,
                },
                CommandKind::DebugDestroy {
                    objects: vec![ObjectId(0x800)],
                },
                CommandKind::KiStart {
                    slot: 1,
                    module: 1,
                    args: Vec::new(),
                    sync: false,
                },
                CommandKind::KiPause { paused: true },
                CommandKind::KiStep { slot: 1, passes: 1 },
                CommandKind::ScriptSkipDialog,
                CommandKind::Queue {
                    inner: Box::new(CommandKind::KiPause { paused: true }),
                    cyclic: false,
                },
            ]
        };
        let mut state = testkit::state(&rules);
        state.network = true;
        let mut sim = Sim::new(&mut state, &rules);
        for kind in kinds() {
            let before = crate::save::checksum(sim.state);
            let name = format!("{kind:?}");
            assert_rejected_with(outcome(&mut sim, 0, kind), NET_LOCKED);
            assert_eq!(crate::save::checksum(sim.state), before, "{name}");
        }
        // The same commands without the network flag: not rejected because of the lock.
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        for kind in kinds() {
            let name = format!("{kind:?}");
            if let Outcome::Rejected(why) = outcome(&mut sim, 0, kind) {
                assert!(!why.contains(NET_LOCKED), "{name}: {why}");
            }
        }
    }

    /// In the network, `SetPlayerResult` from outside accepts only the own slot with result 1
    /// (giving up, dropout); a foreign slot and victory are rejected, the script path stays free.
    // QUIRK E-267: kept
    #[test]
    fn network_limits_player_result_to_own_slot() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        state.network = true;
        for p in &mut state.players {
            p.result = 0;
        }
        let mut sim = Sim::new(&mut state, &rules);
        let before = crate::save::checksum(sim.state);
        let foreign = CommandKind::SetPlayerResult { slot: 2, result: 1 };
        assert_rejected_with(outcome(&mut sim, 1, foreign), NET_LOCKED);
        let victory = CommandKind::SetPlayerResult { slot: 1, result: 2 };
        assert_rejected_with(outcome(&mut sim, 1, victory), NET_LOCKED);
        assert_eq!(crate::save::checksum(sim.state), before);
        let own = CommandKind::SetPlayerResult { slot: 1, result: 1 };
        assert_eq!(outcome(&mut sim, 1, own), Outcome::Accepted);
        assert_eq!(sim.state.players[1].result, 1);
        // Script path: foreign slot is still accepted.
        let cmd = Command {
            player: 1,
            kind: CommandKind::SetPlayerResult { slot: 3, result: 1 },
        };
        assert_eq!(
            crate::player::submit_script(&mut sim, &cmd),
            Outcome::Accepted
        );
        assert_eq!(sim.state.players[3].result, 1);
    }

    /// Signal `0x5C` changes no state (checksum before = after), only reports the event
    /// with the color of the slot field (`0x004789E0`, at execution) and does **not** check the slot field against the
    /// sender (the original does not check, O-321).
    // QUIRK O-321: kept
    #[test]
    fn signal_changes_no_state() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.players[3].color = 5;
        sim.state.events.clear();
        let before = crate::save::checksum(sim.state);
        // Sender 0, slot field 3: no command error.
        let kind = CommandKind::Signal {
            player: 3,
            mask: 0b1001,
            x: 70,
            z: 101,
        };
        assert_eq!(outcome(&mut sim, 0, kind), Outcome::Accepted);
        assert_eq!(crate::save::checksum(sim.state), before);
        assert_eq!(
            sim.state.events,
            [SimEvent::Signal {
                player: 3,
                color: 5,
                mask: 0b1001,
                tile: (70, 101),
            }]
        );
        // Slot field outside 0 to 7: color 8 (`GameWorld_GetPlayerColor`), still without state.
        sim.state.events.clear();
        let kind = CommandKind::Signal {
            player: -1,
            mask: 1,
            x: 0,
            z: 0,
        };
        assert_eq!(outcome(&mut sim, 1, kind), Outcome::Accepted);
        assert_eq!(crate::save::checksum(sim.state), before);
        assert_eq!(
            sim.state.events,
            [SimEvent::Signal {
                player: -1,
                color: 8,
                mask: 1,
                tile: (0, 0),
            }]
        );
    }

    #[test]
    fn execute_reports_accepted_partial_and_rejected() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = testkit::spawn(&mut sim, testkit::COLONIST, 0, (10, 10));
        let b = testkit::spawn(&mut sim, testkit::COLONIST, 0, (11, 10));
        let foreign = testkit::spawn(&mut sim, testkit::COLONIST, 1, (12, 10));
        let house = testkit::spawn(&mut sim, testkit::HOUSE, 1, (20, 20));

        assert_eq!(outcome(&mut sim, 0, walk(&[a, b])), Outcome::Accepted);
        match outcome(&mut sim, 0, walk(&[a, foreign])) {
            Outcome::Partial(why) => assert!(why.contains(&foreign.0.to_string()), "{why}"),
            other => panic!("expected Partial, got {other:?}"),
        }
        assert_rejected_with(
            outcome(&mut sim, 0, walk(&[foreign])),
            &foreign.0.to_string(),
        );
        // A verdict applies only to its command: after a rejection, commands are accepted again.
        assert_eq!(outcome(&mut sim, 0, walk(&[a])), Outcome::Accepted);
        let sell = CommandKind::SellBuildings {
            buildings: vec![house],
        };
        assert_rejected_with(outcome(&mut sim, 0, sell), "not own");
        let diplomacy = |p, q, mode| CommandKind::Diplomacy { p, q, mode };
        assert_rejected_with(
            outcome(&mut sim, 0, diplomacy(1, 0, 1)),
            "slot 1 is not the sender",
        );
        // `0x63` applies if `p` or `q` is the sender (O-322): `{p 1, q 0}` from slot 0 is the vision checkbox
        // of the diplomacy dialog; without the sender the command rejects.
        let vision = |p, q| CommandKind::SharedVision { p, q, on: true };
        assert_eq!(outcome(&mut sim, 0, vision(1, 0)), Outcome::Accepted);
        assert_rejected_with(
            outcome(&mut sim, 0, vision(1, 2)),
            "neither slot 1 nor 2 is the sender",
        );
        assert_eq!(outcome(&mut sim, 0, diplomacy(0, 1, 1)), Outcome::Accepted);
        assert_rejected_with(outcome(&mut sim, 0, diplomacy(0, 1, 9)), "mode 9");
        // Slot 8 does not exist.
        assert_rejected_with(outcome(&mut sim, 8, walk(&[a])), "slot 8");
        // Stance only 0 ... 4.
        let behavior = |mode| CommandKind::SetBehavior {
            units: vec![a],
            mode,
        };
        assert_rejected_with(outcome(&mut sim, 0, behavior(5)), "stance 5");
        assert_eq!(outcome(&mut sim, 0, behavior(3)), Outcome::Accepted);
        // A train command to an own object without a train module.
        let train = CommandKind::Train {
            train: a,
            op: TrainOp::Stop,
            target: None,
        };
        assert_rejected_with(outcome(&mut sim, 0, train), "no train module");
    }

    /// O-322: The vision checkbox of the diplomacy dialog sends
    /// `{0x63, N, l, on}` - "N sees what I see" - with the sender as `q`. That sets `View[N]` bit `l`; with the
    /// sender as `p`, `View[l]` bit `q`. Without the sender in `p` or `q` the vision stays.
    // QUIRK O-322: kept
    #[test]
    fn shared_vision_accepts_sender_as_q() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let vision = |p, q, on| CommandKind::SharedVision { p, q, on };
        assert_eq!(outcome(&mut sim, 0, vision(3, 0, true)), Outcome::Accepted);
        assert!(sim.state.diplomacy.shares_vision(3, 0), "View[3] Bit 0");
        assert!(!sim.state.diplomacy.shares_vision(0, 3), "one-sided");
        assert_eq!(outcome(&mut sim, 0, vision(0, 5, true)), Outcome::Accepted);
        assert!(sim.state.diplomacy.shares_vision(0, 5));
        assert_eq!(outcome(&mut sim, 0, vision(3, 0, false)), Outcome::Accepted);
        assert!(!sim.state.diplomacy.shares_vision(3, 0), "off");
        let before = sim.state.diplomacy.clone();
        assert_rejected_with(
            outcome(&mut sim, 0, vision(3, 4, true)),
            "neither slot 3 nor 4 is the sender",
        );
        assert_eq!(sim.state.diplomacy, before);
    }

    /// A message: receiver, number, location, flag word.
    type Announced = (Option<u8>, i32, (i32, i32), u32);

    /// Messages among the events.
    fn announced(sim: &Sim) -> Vec<Announced> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage {
                    to,
                    id,
                    tile,
                    flags,
                } => Some((*to, *id, *tile, *flags)),
                _ => None,
            })
            .collect()
    }

    // QUIRK O-216, O-247: kept
    #[test]
    fn diplomacy_announces_231_to_233_to_everyone() {
        // `GameWorld_CmdSetDiplomacy` `0x00478720`: message 231 + mode to all (`to: None`) without location,
        // flag `(q | 0x100) << 8 | p` (= `0x10000 | q << 8 | p`).
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        for (mode, id) in [(0, 231), (1, 232), (2, 233)] {
            sim.state.events.clear();
            let kind = CommandKind::Diplomacy { p: 2, q: 5, mode };
            assert_eq!(outcome(&mut sim, 2, kind), Outcome::Accepted);
            assert_eq!(announced(&sim), [(None, id, (-1, -1), 0x1_0502)]);
        }
    }

    // QUIRK O-247: kept
    #[test]
    fn diplomacy_without_change_still_announces() {
        // O-247: every valid command reports, even if the relationship already stands so.
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let kind = CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 1,
        };
        assert_eq!(outcome(&mut sim, 0, kind.clone()), Outcome::Accepted);
        sim.state.events.clear();
        let before = sim.state.diplomacy.clone();
        assert_eq!(outcome(&mut sim, 0, kind), Outcome::Accepted);
        assert_eq!(sim.state.diplomacy, before);
        assert_eq!(announced(&sim), [(None, 232, (-1, -1), 0x1_0100)]);
    }

    // QUIRK O-247: kept
    #[test]
    fn invalid_diplomacy_announces_nothing() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.events.clear();
        // Foreign sender and mode outside 0 ... 2.
        for (player, p, mode) in [(0, 1, 1), (0, 0, 3), (0, 0, -1)] {
            let kind = CommandKind::Diplomacy { p, q: 1, mode };
            assert!(matches!(
                outcome(&mut sim, player, kind),
                Outcome::Rejected(_)
            ));
        }
        assert_eq!(announced(&sim), []);
    }

    #[test]
    fn queued_commands_report_like_direct_ones() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = testkit::spawn(&mut sim, testkit::COLONIST, 0, (10, 10));
        let foreign = testkit::spawn(&mut sim, testkit::COLONIST, 1, (12, 10));
        let wrap = |inner: CommandKind| CommandKind::Queue {
            cyclic: false,
            inner: Box::new(inner),
        };
        // Queued per unit: like the immediate command.
        assert_rejected_with(
            outcome(&mut sim, 0, wrap(walk(&[foreign]))),
            &foreign.0.to_string(),
        );
        assert!(matches!(
            outcome(&mut sim, 0, wrap(walk(&[a, foreign]))),
            Outcome::Partial(_)
        ));
        assert_eq!(outcome(&mut sim, 0, wrap(walk(&[a]))), Outcome::Accepted);
        // Immediately executed content of the wrapper (halt) and wrapper in a wrapper.
        let halt = CommandKind::Halt {
            units: vec![foreign],
        };
        assert_rejected_with(outcome(&mut sim, 0, wrap(halt)), &foreign.0.to_string());
        assert_rejected_with(
            outcome(&mut sim, 0, wrap(wrap(walk(&[a])))),
            "wrapper in a wrapper",
        );
    }

    #[test]
    fn a_nested_execution_leaves_the_outer_verdict_alone() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = testkit::spawn(&mut sim, testkit::COLONIST, 0, (10, 10));
        let foreign = testkit::spawn(&mut sim, testkit::COLONIST, 1, (12, 10));
        reject(&sim, 0, "Outer", "first reason");
        // The inner verdict is its own ...
        assert!(matches!(
            outcome(&mut sim, 0, walk(&[foreign])),
            Outcome::Rejected(_)
        ));
        assert_eq!(outcome(&mut sim, 0, walk(&[a])), Outcome::Accepted);
        // ... and the outer reason stays, the first one wins.
        reject(&sim, 0, "Outer", "second reason");
        let v = sim.verdict.take();
        assert_eq!(v.whole.as_deref(), Some("Outer: first reason"));
    }
    #[test]
    fn a_queue_inside_a_queue_is_refused_when_read_and_when_executed() {
        let halt = CommandKind::Halt {
            units: vec![ObjectId(0x10800)],
        };
        let wrap = |inner: CommandKind| CommandKind::Queue {
            cyclic: false,
            inner: Box::new(inner),
        };
        let one = postcard::to_stdvec(&wrap(halt.clone())).unwrap();
        assert_eq!(
            postcard::from_bytes::<CommandKind>(&one).unwrap(),
            wrap(halt.clone())
        );
        // Two wrappers: reading fails. The bytes of a wrapper are {variant, cyclic} before the
        // content; a chain of 10^6 wrappers would blow the stack when reading without the limit.
        let two = postcard::to_stdvec(&wrap(wrap(halt.clone()))).unwrap();
        assert!(postcard::from_bytes::<CommandKind>(&two).is_err());
        let head = &one[..one.len() - postcard::to_stdvec(&halt).unwrap().len()];
        let mut deep = head.repeat(1_000_000);
        deep.extend_from_slice(&postcard::to_stdvec(&halt).unwrap());
        assert!(postcard::from_bytes::<CommandKind>(&deep).is_err());
        // After an error a simple wrapper reads again.
        assert!(postcard::from_bytes::<CommandKind>(&one).is_ok());
        // Executing (built in memory): A8 rejects, the unit gets nothing.
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = testkit::spawn(&mut sim, testkit::COLONIST, 0, (10, 10));
        let walk = CommandKind::Move {
            units: vec![w],
            x: 2 << 12,
            z: 2 << 12,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        };
        testkit::run(&mut sim, 0, wrap(wrap(walk.clone())));
        assert!(sim.object(w).unwrap().commands.is_empty());
        testkit::run(&mut sim, 0, wrap(walk));
        assert_eq!(sim.object(w).unwrap().commands.len(), 1);
    }

    #[test]
    fn commands_have_original_type_codes_and_survive_postcard() {
        let c = Command {
            player: 1,
            kind: CommandKind::Queue {
                cyclic: true,
                inner: Box::new(CommandKind::Halt {
                    units: vec![ObjectId(0x10800)],
                }),
            },
        };
        assert_eq!(c.kind.type_code(), 0x5000_007C);
        let bytes = postcard::to_stdvec(&c).unwrap();
        assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), c);
        let d = CommandKind::Diplomacy {
            p: 0,
            q: 1,
            mode: 1,
        };
        assert_eq!(d.type_code(), 0x5000_0064);
        let m = CommandKind::Move {
            units: vec![ObjectId(0x10800)],
            x: 5 << 12,
            z: 6 << 12,
            target: -1,
            formation: 3,
            direction: NO_DIRECTION,
            spread: 0x1800,
            flags: 1,
        };
        assert_eq!(m.type_code(), 0x5000_0004);
        let bytes = postcard::to_stdvec(&m).unwrap();
        assert_eq!(postcard::from_bytes::<CommandKind>(&bytes).unwrap(), m);
        // Group commands: formation `0x44`, scatter `0x75`; riding, boat and
        // diving: `0x0A`-`0x0D`, `0x1B`, `0x1C`.
        let ids = vec![ObjectId(0x10800), ObjectId(0x10801)];
        let target = ObjectId(0x20800);
        for (kind, code) in [
            (
                CommandKind::Mount {
                    horse: target,
                    units: ids.clone(),
                },
                0x0A,
            ),
            (CommandKind::Dismount { units: ids.clone() }, 0x0B),
            (
                CommandKind::BoardBoat {
                    boat: target,
                    units: ids.clone(),
                },
                0x0C,
            ),
            (CommandKind::LeaveBoat { units: ids.clone() }, 0x0D),
            (CommandKind::Dive { units: ids.clone() }, 0x1B),
            (CommandKind::Surface { units: ids }, 0x1C),
            // Train commands: `0x29`-`0x2B`.
            (
                CommandKind::Train {
                    train: target,
                    op: TrainOp::Forward,
                    target: Some((33, 20)),
                },
                0x29,
            ),
            (
                CommandKind::Train {
                    train: target,
                    op: TrainOp::Backward,
                    target: None,
                },
                0x2A,
            ),
            (
                CommandKind::Train {
                    train: target,
                    op: TrainOp::Stop,
                    target: None,
                },
                0x2B,
            ),
            (
                CommandKind::SetFormation {
                    units: vec![ObjectId(0x10800), ObjectId(0x10801)],
                    formation: 3,
                },
                0x44,
            ),
            (
                CommandKind::Scatter {
                    units: vec![ObjectId(0x10800)],
                },
                0x75,
            ),
            // Console upgrades: `0x39`, `0x3A`.
            (CommandKind::ApplyUpgrade { slot: 0, guid: -1 }, 0x39),
            (
                CommandKind::RemoveUpgrade {
                    slot: 0,
                    guid: 3374,
                },
                0x3A,
            ),
            // Counter-upgrade: `0x2F`.
            (
                CommandKind::CounterUpgrade {
                    guid: 3374,
                    source: ObjectId(0x10800),
                },
                0x2F,
            ),
            // Cast spell and execute: `0x36`, `0x5E`.
            (
                CommandKind::CastSpell {
                    caster: ObjectId(0x10800),
                    art: 4,
                    anim: 2,
                    idx: 1,
                    button: 543,
                },
                0x36,
            ),
            (
                CommandKind::ExecTempUpgrade {
                    object: ObjectId(0x10800),
                    button: 544,
                    idx: 2,
                },
                0x5E,
            ),
            // Arm and fire arrows: `0x38`/`0x72`, `0x40`/`0x41`.
            (
                CommandKind::ToggleArming {
                    units: vec![ObjectId(0x10800)],
                    arm: true,
                },
                0x38,
            ),
            (
                CommandKind::ToggleArming {
                    units: vec![ObjectId(0x10800)],
                    arm: false,
                },
                0x72,
            ),
            (
                CommandKind::SetArrowType {
                    units: vec![ObjectId(0x10800)],
                    fire: true,
                },
                0x40,
            ),
            (
                CommandKind::SetArrowType {
                    units: vec![ObjectId(0x10800)],
                    fire: false,
                },
                0x41,
            ),
        ] {
            assert_eq!(kind.type_code(), 0x5000_0000 | code, "{kind:?}");
            let bytes = postcard::to_stdvec(&kind).unwrap();
            assert_eq!(postcard::from_bytes::<CommandKind>(&bytes).unwrap(), kind);
        }
    }

    #[test]
    fn tent_command_codes() {
        // `0x2D` Pack tent, `0x2E` Unpack tent (re/tents 2.4).
        for (kind, code) in [
            (
                CommandKind::PackTent {
                    tent: ObjectId(0x10800),
                },
                0x2D,
            ),
            (
                CommandKind::UnpackTent {
                    unit: ObjectId(0x10800),
                    x: 5 << 12,
                    z: -3,
                },
                0x2E,
            ),
        ] {
            assert_eq!(kind.type_code(), 0x5000_0000 | code, "{kind:?}");
            let command = Command { player: 1, kind };
            let bytes = postcard::to_stdvec(&command).unwrap();
            assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), command);
        }
    }

    #[test]
    fn gate_command_codes() {
        // `0x19` Open gate, `0x1A` Close gate (re/arming-gates 2.C).
        for (kind, code) in [
            (
                CommandKind::OpenGate {
                    gate: ObjectId(0x10800),
                },
                0x19,
            ),
            (
                CommandKind::CloseGate {
                    gate: ObjectId(0x10801),
                },
                0x1A,
            ),
        ] {
            assert_eq!(kind.type_code(), 0x5000_0000 | code, "{kind:?}");
            let command = Command { player: 1, kind };
            let bytes = postcard::to_stdvec(&command).unwrap();
            assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), command);
        }
    }

    #[test]
    fn ability_command_codes() {
        // `0x1D` Camouflage, `0x1E` Uncamouflage, `0x20` Dig in, `0x21` Dig out
        // (re/masking 2.3).
        let units = vec![ObjectId(0x10800), ObjectId(0x10801)];
        for (kind, code) in [
            (
                CommandKind::Mask {
                    units: units.clone(),
                },
                0x1D,
            ),
            (
                CommandKind::Unmask {
                    units: units.clone(),
                },
                0x1E,
            ),
            (
                CommandKind::DigIn {
                    units: units.clone(),
                },
                0x20,
            ),
            (
                CommandKind::DigOut {
                    units: units.clone(),
                },
                0x21,
            ),
            // Healing (re/healing 2.3).
            (
                CommandKind::StartHealing {
                    units: units.clone(),
                },
                0x34,
            ),
            (
                CommandKind::StopHealing {
                    units: units.clone(),
                },
                0x35,
            ),
            (
                CommandKind::HealTarget {
                    target: ObjectId(0x10802),
                    units: units.clone(),
                },
                0x78,
            ),
            (
                CommandKind::SelfHeal {
                    unit: units[0],
                    button: 555,
                },
                0x60,
            ),
            // Evoke ghost, evoke animal (kind 0/1/2), banish
            // (re/ghosts-banish 2.5, re/evoke-aura 2.4).
            (
                CommandKind::EvokeGhost {
                    unit: units[0],
                    button: 436,
                },
                0x1F,
            ),
            (
                CommandKind::EvokeAnimal {
                    unit: units[0],
                    art: 0,
                    button: 441,
                },
                0x22,
            ),
            (
                CommandKind::EvokeAnimal {
                    unit: units[0],
                    art: 1,
                    button: 442,
                },
                0x23,
            ),
            (
                CommandKind::EvokeAnimal {
                    unit: units[0],
                    art: 2,
                    button: 443,
                },
                0x24,
            ),
            (
                CommandKind::Banish {
                    unit: units[0],
                    button: 547,
                },
                0x25,
            ),
            // Drain mana, scout area (re/spells 2.3).
            (
                CommandKind::DrainMana {
                    unit: units[0],
                    target: units[1],
                    button: 541,
                },
                0x4E,
            ),
            (
                CommandKind::ScoutArea {
                    unit: units[0],
                    x: 20,
                    z: 30,
                    size: 30,
                    duration_ms: 5000,
                    flag: 1,
                    button: 484,
                },
                0x37,
            ),
            // Special attacks, whirlwind, damage building
            // (re/special-attacks 2.4).
            (
                CommandKind::DynamiteBuilding {
                    unit: units[0],
                    target: units[1],
                    button: 454,
                },
                0x30,
            ),
            (
                CommandKind::DynamiteShip {
                    unit: units[0],
                    target: units[1],
                    button: 455,
                },
                0x31,
            ),
            (
                CommandKind::KillUnit {
                    unit: units[0],
                    target: units[1],
                    button: 548,
                },
                0x32,
            ),
            (
                CommandKind::SnipeUnit {
                    unit: units[0],
                    target: units[1],
                    button: 546,
                },
                0x33,
            ),
            (
                CommandKind::Whirlwind {
                    unit: units[0],
                    button: 542,
                },
                0x4F,
            ),
            (
                CommandKind::DamageBuilding {
                    unit: units[0],
                    target: units[1],
                    button: 551,
                },
                0x5F,
            ),
            // Hire headhunter (re/headhunter-robbing 2.4).
            (
                CommandKind::HireHeadhunter {
                    art: 2,
                    target_player: 1,
                    target_guid: 2615,
                    building: units[1],
                },
                0x2C,
            ),
            // Rob corpses (re/headhunter-robbing 2.4).
            (
                CommandKind::RobCorpses {
                    unit: units[0],
                    button: 540,
                },
                0x4C,
            ),
            // The network commands of the AI (re/ki-queries 7.3).
            (
                CommandKind::Die {
                    units: vec![units[0], units[1]],
                },
                0x10,
            ),
            (
                CommandKind::OccupancyBorder {
                    building: units[1],
                    border: -2,
                },
                0x59,
            ),
            (
                CommandKind::AiPlayerAttackMode {
                    slot: 1,
                    aggressiveness: 4,
                    stubbornness: 2,
                },
                0x68,
            ),
            (CommandKind::AiPlayerBehavior { slot: 1, mode: 3 }, 0x69),
            (
                CommandKind::AiPlayerWalkFlags {
                    slot: 1,
                    mobile_home: true,
                    tether: -2,
                    follow: true,
                    abort: false,
                },
                0x6A,
            ),
            (
                CommandKind::AiObjectAttackMode {
                    object: units[0],
                    aggressiveness: 1,
                    stubbornness: 0,
                },
                0x6B,
            ),
            (
                CommandKind::AiObjectWalkFlags {
                    object: units[0],
                    mobile_home: false,
                    tether: -1,
                    follow: false,
                    abort: true,
                },
                0x6C,
            ),
            (
                CommandKind::AddResource {
                    slot: 1,
                    resource: 4,
                    amount: -50,
                },
                0x7E,
            ),
            (
                CommandKind::KiStart {
                    slot: 3,
                    module: 195_235,
                    args: vec![3, 1, -5],
                    sync: true,
                },
                KI_START,
            ),
            (CommandKind::KiPause { paused: true }, KI_PAUSE),
            (
                CommandKind::KiStep {
                    slot: 7,
                    passes: 1000,
                },
                KI_STEP,
            ),
            (CommandKind::SetPlayerResult { slot: 2, result: 1 }, 0x56),
            (
                CommandKind::StartAnimationLock {
                    unit: ObjectId(0x800),
                    anim: 3,
                    seconds: -1,
                },
                0x42,
            ),
            (
                CommandKind::StopAnimationLock {
                    unit: ObjectId(0x800),
                },
                0x43,
            ),
            (CommandKind::ScriptSkipDialog, SCRIPT_SKIP_DIALOG),
            (
                CommandKind::DebugDestroy {
                    objects: vec![ObjectId(0x800), ObjectId(0x801)],
                },
                DEBUG_DESTROY,
            ),
            (
                CommandKind::ScriptSelection {
                    ids: vec![ObjectId(0x800), ObjectId(0x801)],
                },
                SCRIPT_SELECTION,
            ),
            (CommandKind::ScriptButton { word: -7 }, SCRIPT_BUTTON),
            (
                CommandKind::Signal {
                    player: 2,
                    mask: 0b101,
                    x: 70,
                    z: 101,
                },
                0x5C,
            ),
            (
                CommandKind::Chat {
                    player: 1,
                    mask: 0xFFFF_FFFF,
                    text: "grüß".encode_utf16().collect(),
                },
                0x5D,
            ),
        ] {
            assert_eq!(kind.type_code(), 0x5000_0000 | code, "{kind:?}");
            let command = Command { player: 1, kind };
            let bytes = postcard::to_stdvec(&command).unwrap();
            assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), command);
        }
    }

    #[test]
    fn economy_commands_have_original_type_codes_and_keep_their_f32_bits() {
        let u = vec![ObjectId(0x800)];
        let b = ObjectId(0x801);
        let nan = F32Bits(0x7FC0_1234);
        let kinds = [
            (
                CommandKind::PlaceBuilding {
                    guid: 1001,
                    rotation: F32Bits::new(std::f32::consts::FRAC_PI_2),
                    x: F32Bits::new(1000.5),
                    y: F32Bits::new(-0.0),
                    z: nan,
                    workers: u.clone(),
                    border: Some(2),
                },
                0x01,
            ),
            (
                CommandKind::ConstructAt {
                    building: b,
                    units: u.clone(),
                },
                0x02,
            ),
            (
                CommandKind::MineGold {
                    mine: b,
                    units: u.clone(),
                },
                0x03,
            ),
            (
                CommandKind::ChopWood {
                    tree: b,
                    units: u.clone(),
                },
                0x06,
            ),
            (
                CommandKind::Harvest {
                    farm: b,
                    units: u.clone(),
                },
                0x07,
            ),
            (
                CommandKind::Deliver {
                    building: b,
                    units: u.clone(),
                },
                0x09,
            ),
            (
                CommandKind::EnqueueProduction {
                    building: b,
                    guid: 1100,
                },
                0x0E,
            ),
            (
                CommandKind::CancelProduction {
                    building: b,
                    index: 1,
                    arg: 0,
                },
                0x0F,
            ),
            (
                CommandKind::Quarter {
                    container: b,
                    units: u.clone(),
                },
                0x13,
            ),
            (
                CommandKind::Unquarter {
                    container: b,
                    unit: u[0],
                },
                0x14,
            ),
            (CommandKind::UnquarterAll { container: b }, 0x15),
            (
                CommandKind::PlaceBuildings {
                    parts: vec![BuildingPart {
                        guid: 1011,
                        rotation: F32Bits::new(0.0),
                        x: F32Bits::new(64.0),
                        y: F32Bits::new(0.0),
                        z: nan,
                    }],
                    workers: u.clone(),
                },
                0x18,
            ),
            (
                CommandKind::SetRallyPoint {
                    x: 3 << 12,
                    z: 4 << 12,
                    buildings: vec![b],
                },
                0x27,
            ),
            (CommandKind::SellBuildings { buildings: vec![b] }, 0x67),
            (
                CommandKind::Fish {
                    basket: b,
                    units: u.clone(),
                },
                0x7D,
            ),
            (
                CommandKind::NextTree {
                    tile_x: 7,
                    tile_z: 8,
                    unit: u[0],
                },
                0x7F,
            ),
            (
                CommandKind::DebugCompleteOrder {
                    building: b,
                    index: 2,
                    guid: 1100,
                },
                DEBUG_COMPLETE_ORDER,
            ),
            (
                CommandKind::Attack {
                    units: u.clone(),
                    target: b,
                    pos: None,
                    mods: 5,
                },
                0x11,
            ),
            (
                CommandKind::Attack {
                    units: u.clone(),
                    target: b,
                    pos: Some([1 << 19, 0, 2 << 19]),
                    mods: 0,
                },
                0x71,
            ),
            (
                CommandKind::SetBehavior {
                    units: u.clone(),
                    mode: 3,
                },
                0x12,
            ),
            (
                CommandKind::GarrisonAttack {
                    buildings: vec![b],
                    target: b,
                },
                0x5B,
            ),
            (
                CommandKind::Hunt {
                    animal: b,
                    units: u.clone(),
                    gut: true,
                },
                0x16,
            ),
            (
                CommandKind::Hunt {
                    animal: b,
                    units: u.clone(),
                    gut: false,
                },
                0x17,
            ),
            (
                CommandKind::Follow {
                    target: b,
                    units: u.clone(),
                },
                0x74,
            ),
        ];
        for (kind, code) in kinds {
            assert_eq!(kind.type_code(), 0x5000_0000 | code, "{kind:?}");
            let bytes = postcard::to_stdvec(&kind).unwrap();
            assert_eq!(postcard::from_bytes::<CommandKind>(&bytes).unwrap(), kind);
        }
        // Bit-exact, also NaN with payload and -0.
        assert_eq!(F32Bits::new(-0.0).0, 0x8000_0000);
        assert_eq!(nan.get().to_bits(), 0x7FC0_1234);
    }
}
