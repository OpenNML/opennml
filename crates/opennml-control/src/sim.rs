//! Results of the simulation commands (protocol 6) and the `sim` section of the state snapshot.
//! Checksums as a 16-digit hexadecimal number (64 bits do not fit losslessly into JSON numbers).
//! Protocol 9 adds the economy: building, gatherer, construction, quarters, deposit and
//! tree state in `sim_query`, per-player account counters, market prices and messages in the
//! `sim` section, plus the results of `sim_build`, `sim_action` and `sim_menu`. Protocol 10 adds
//! sight: `sim_fog`, enemies and shared vision per player, fog switch in the `sim` section. Protocol 11
//! adds melee (re/combat-host 7): combat, attack and dying state in
//! `sim_query` and the attack points in the `sim` section. Protocol 12 adds ranged combat, projectiles,
//! animals and hunting (re/combat-host 7): `ranged`, `animal`, `edible` and hunting in the
//! gatherer section of `sim_query`, the projectiles in the `sim` section. Protocol 15 adds train,
//! multipart objects, ships (re/train-multipart, re/ships): `train`, `multipart` and
//! `event_bits` per object, meeting point and waiting objects in the quarters section, the broadside in the
//! shot section.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `sim` section of the state snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSection {
    /// Object tick (100 ms) and engine ticks (50 ms).
    pub tick: u32,
    pub steps: u64,
    /// Game type (-1 campaign, 0 normal, 1 kill hero, 2 Domination, 3 King of the Hill, 4/5 Railroad).
    pub mode: i32,
    pub seed: u32,
    pub game_speed: u8,
    pub active_player: u8,
    pub checksum: String,
    /// All objects of the simulation.
    pub objects: usize,
    /// Selected objects (IDs) in selection order.
    pub selection: Vec<u32>,
    /// Commands since the start (replay).
    pub commands: usize,
    /// Pathfinding overlay (key G): layer 0-8, 9 = occupancy, `None` off (protocol 7).
    #[serde(default)]
    pub grid_layer: Option<u8>,
    /// Displayed path (path preview or `sim_path`) as a line (protocol 7).
    #[serde(default)]
    pub path: Option<String>,
    /// Objects with the walk bit set (`status` bit 0, protocol 8).
    #[serde(default)]
    pub moving: usize,
    /// Market prices for wood and food (`m_WoodPrice`/`m_FoodPrice`, start values 1024/1351) and the
    /// living space limit of the match (protocol 9).
    #[serde(default)]
    pub wood_price: i32,
    #[serde(default)]
    pub food_price: i32,
    #[serde(default)]
    pub max_living_space: i32,
    /// Latest messages of the active player, oldest first (protocol 9).
    #[serde(default)]
    pub notices: Vec<SimNotice>,
    /// Target marker after a right-click or `sim_action` on an object (`GameWorld+0x318`,
    /// viewer display only, 300 ms on the virtual clock; protocol 9).
    #[serde(default)]
    pub target_marker: Option<SimTargetMarker>,
    /// Objects with a ground marker in the last frame in drawing order (selection, target marker,
    /// object under the mouse; protocol 9).
    #[serde(default)]
    pub ground_markers: Vec<u32>,
    /// Fog on (protocol 10; `m_FogShowMode` 0, "fog off" = `false`). When reading old
    /// responses without the field: `false`.
    #[serde(default)]
    pub fog_shown: bool,
    /// Fog overlay of the viewer (key F): 1 explored, 3 seen, 7 cleared, `None` off
    /// (protocol 10).
    #[serde(default)]
    pub fog_overlay: Option<u8>,
    /// Attack points per target owner as last reported (`SimEvent::AttackPoints`, every
    /// double tick; protocol 11). The value stays until the next event, also while
    /// paused.
    #[serde(default)]
    pub attack_points: [i32; 8],
    /// Attack points of the active player: the value that the music state machine receives per frame
    /// (protocol 11).
    #[serde(default)]
    pub attack_points_active: i32,
    /// Flying projectiles of the simulation, in tick and damage order (protocol 12).
    #[serde(default)]
    pub projectiles: Vec<SimProjectileInfo>,
    /// Flight paths of the host (display only; protocol 12).
    #[serde(default)]
    pub flights: usize,
    /// All groups of the group manager in map order (protocol 13).
    #[serde(default)]
    pub groups: Vec<SimGroupInfo>,
    /// Selected formation of the unit bar 0-4 (viewer state, not in the savegame;
    /// protocol 13).
    #[serde(default)]
    pub formation: i32,
    /// Flags of the game types Domination and King of the Hill (protocol 39):
    /// set in game type 2/3 or as soon as a player flag (GUID 4300) is alive, otherwise the field is missing. The
    /// same structure is in `ui_state.game.flags` (`opennml-game`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<SimFlagsInfo>,
}

/// State of the flags of a match (re/domination F). Filled by
/// `opennml_client::game::sim_host::flags_info`, read-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimFlagsInfo {
    /// The player flags (GUID 4300) in object order (slot, then index).
    #[serde(default)]
    pub flags: Vec<SimFlagInfo>,
    /// Counter per slot 0 ... 7 and 8 = neutral (`m_FlagsCounter`, `SimState::flag_counts`); the counter of the white
    /// player can become negative (D-299).
    #[serde(default)]
    pub counts: [i32; 9],
    /// Flag leader (`m_FlagOwner`, `Engine+0x4E4`): slot 0 ... 7, 8 = nobody, 9 = unreachable (game type 3 without
    /// a flag).
    #[serde(default)]
    pub leader: i32,
    /// Time limit in seconds (`m_TimeLimit`, minutes * 60).
    #[serde(default)]
    pub time_limit_s: i32,
    /// Remaining time of the leader timer (ID 0) in ms; `None` if none is running.
    #[serde(default)]
    pub remaining_ms: Option<i32>,
}

/// A player flag (GUID 4300).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimFlagInfo {
    /// Object ID.
    #[serde(default)]
    pub id: u32,
    /// Tile of the flag.
    #[serde(default)]
    pub tile: [i32; 2],
    /// Color of the flag (9 neutral, otherwise the color of the conqueror; an animal colors it with 8).
    #[serde(default)]
    pub color: i32,
    /// Holder: the first slot 0 ... 7 with this color, otherwise 8 (`FUN_00478DE0`, D-299/D-300).
    #[serde(default)]
    pub holder: u8,
    /// Check counter of the flag in ms (`m_Timer`, -1 off); at 0 the next conquest check runs.
    #[serde(default)]
    pub check_ms: i32,
}

/// A group (`Group`, re/groups-formations 1.3; protocol 13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGroupInfo {
    /// Group ID (`+0x60`).
    pub id: i32,
    /// Kind 0-6: 0 without formation (also the secondary group for rank 0), 1-4 line/square/column/pincer,
    /// 5 like 0 with speed matching without compression, 6 unordered body.
    pub kind: u8,
    /// `vf04`: type ID (type 5 reports 0).
    pub type_id: i32,
    /// Members in command order, head = leader.
    pub members: Vec<u32>,
    /// Click point (x, z, direction) in fixed point; x = -1 -> center of gravity.
    pub anchor: [i32; 3],
    /// Realignment or march is running (`+0x5C`).
    pub marching: bool,
    /// Smallest `DefaultSpeed` of the members (`+0x88`).
    pub matched_speed: i32,
    /// Target slots per member.
    pub targets: Vec<SimGroupTarget>,
    /// State of the formation (kind 1-4, 6).
    #[serde(default)]
    pub formation: Option<SimFormationInfo>,
}

/// Target slot of a member (fixed point 20.12, direction in degrees * 4096).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGroupTarget {
    pub id: u32,
    pub x: i32,
    pub z: i32,
    pub dir: i32,
}

/// Formation part of a group (`FormationState`, re/groups-formations 1.4; protocol 13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimFormationInfo {
    /// Spreading factor (`+0x1F0`).
    pub spacing: i32,
    /// Depth (`+0x1EC`).
    pub depth: i32,
    /// Shared NxN layer (`+0x1F4`), -1 = recompute.
    pub layer: i32,
    /// Secondary group for rank 0 (`+0x1F8`), -1 without.
    pub rear: i32,
    pub rear_members: Vec<u32>,
    /// Number of march frames.
    pub frames: usize,
    /// Members in step.
    pub ready: Vec<u32>,
    /// Reported members or members arrived at the frame slot.
    pub reported: i32,
    pub arrived: i32,
    /// Wing ends (object ID, `u32::MAX` = none).
    pub end_max: u32,
    pub end_min: u32,
}

/// Active target marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimTargetMarker {
    /// ID of the marked object.
    pub id: u32,
    /// Remaining time in ms of the virtual clock (0 in the last valid instant).
    pub remaining_ms: f32,
}

/// A message of the simulation to a player (`SimEvent::PlayerMessage`,
/// `MessageBar_ShowMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimNotice {
    /// Engine tick in which the message arose (counted before the step like the `step` of the
    /// commands).
    pub step: u64,
    /// Number from `messages.ini` (e.g. 2 wood is missing).
    pub id: i32,
    /// Tile the message points to (-1, -1 without).
    pub tile: [i32; 2],
    /// Text from `texte.deu` (`None`: unknown or without text).
    #[serde(default)]
    pub text: Option<String>,
}

/// A module of an object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimModuleInfo {
    pub index: u8,
    /// `PROPID` name (`STAND`, `NATURE`, ...).
    pub name: String,
    /// Behavior: `inert` (not yet implemented), `walk`, `stand`, `building`, `tree`, `natur`,
    /// `resource`, `lumber`, `mining`, `harvest`, `fishing`, `quarter`, `selfheal`, `construct`,
    /// `attack`, `fightnear`, `dying`, `fightfar`, `shoot`, `animal`, `edible`, `hunting`, `riding`,
    /// `swim`, `diving`, `train`, `multihead`, `multipart`, `globalupgrade`, `spell`,
    /// `exectempupgrade`, `transporttent`, `tent`, `trading`, `masking`, `assassin`, `healing`,
    /// `evokeghost`, `ghost`, `banish`, `evokeanimal`.
    pub kind: String,
    pub enabled: bool,
    pub idle: bool,
    pub flags: u32,
    /// Running and newly created timers.
    pub timers: usize,
}

/// An object of the simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimObjectInfo {
    /// ID `index<<16 | slot<<11 | generation`.
    pub id: u32,
    pub guid: u32,
    pub type_name: String,
    pub owner: u8,
    pub position: [f32; 3],
    pub tile: [i32; 2],
    /// Facing direction in degrees.
    pub direction: f32,
    pub status: u32,
    /// Animation (index of the object's `.cfg`, -1 none) and number of starts.
    pub animation: i32,
    pub animation_starts: u32,
    /// Target (`None` without).
    pub target: Option<u32>,
    pub tick_group: i8,
    /// Script name of the object (`names::reverse`, reverse lookup in the name table; protocol 30;
    /// `None` without a name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hidden (`world::is_hidden`: slot hidden and the object not individually shown;
    /// protocol 30). Missing from the JSON if not.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    pub modules: Vec<SimModuleInfo>,
    /// Walk module (protocol 8; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk: Option<SimWalkInfo>,
    /// Building module 3 (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building: Option<SimBuildingInfo>,
    /// Gatherer module 7, 8, 12 or 55 with the carried load (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gatherer: Option<SimGathererInfo>,
    /// Construction worker module 1 (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub construct: Option<SimConstructInfo>,
    /// Quarters module 23 (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarter: Option<SimQuarterInfo>,
    /// Deposit module 6 (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<SimResourceInfo>,
    /// Tree module 4 (protocol 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<SimTreeInfo>,
    /// Combat state for objects with hit points (attribute 14) or module 16/17/18
    /// (protocol 11; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat: Option<SimCombatInfo>,
    /// Ranged combat module 19 with shot module 10 (protocol 12; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranged: Option<SimRangedInfo>,
    /// Animal module 20 (protocol 12; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animal: Option<SimAnimalInfo>,
    /// Carcass module 21 (protocol 12; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edible: Option<SimEdibleInfo>,
    /// Riding modules 13 (rider on horse) and 14 (occupant in canoe), one entry per existing module
    /// (protocol 14; empty without).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub riding: Vec<SimRidingInfo>,
    /// Dive module 24 (protocol 14; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diving: Option<SimDivingInfo>,
    /// Train module 33 (protocol 15; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub train: Option<SimTrainInfo>,
    /// Multipart object: front part (module 11) and/or rear part (module 51) (protocol 15; `None`
    /// without either).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multipart: Option<SimMultiPartInfo>,
    /// Event word of the object (`SimState::object_event_bits`, protocol 15):
    /// bit 0 halt, 8 out forward, 9 out backward (train); 0 without entry and then not in the
    /// JSON.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub event_bits: u32,
    /// List of temporary upgrades of the object (`GameObject+0x788`, protocol 16; empty without).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub temp_upgrades: Vec<SimTempUpgrade>,
    /// `m_ShowCounterUpgrade`: number of running counter-upgrades this object shows
    /// (protocol 16; 0 is missing from the JSON).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub show_counter_upgrade: i32,
    /// `ISARMED` of the arming module 39 (protocol 16; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed: Option<bool>,
    /// Spell module 38 with the extra energy of the object (protocol 16; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spell: Option<SimSpellInfo>,
    /// Execution list of module 44 (temporary upgrades as GUIDs, protocol 16; empty without).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_list: Vec<u32>,
    /// Tent module 42 (protocol 17; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tent: Option<SimTentInfo>,
    /// Travois module 35 (protocol 17; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_tent: Option<SimTransportTentInfo>,
    /// Trade module 43 (protocol 17; `None` without).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade: Option<SimTradeInfo>,
    /// Only for buildings (module 3, protocol 17): is the object in the collision manager? According to W-89
    /// a hidden tent is removed from it (`false`); without module 3 the field is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_collision: Option<bool>,
    /// Stealth module 25 (protocol 19; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masking: Option<SimMaskingInfo>,
    /// Assassin module 28 (protocol 19; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assassin: Option<SimAssassinInfo>,
    /// Healing module 29 (protocol 19; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healing: Option<SimHealingInfo>,
    /// Message filter of the object (`GameObject+0x730`, protocol 19): only if it differs from the
    /// default (mode 1, empty list), for example while digging in and in trance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_filter: Option<SimMessageFilterInfo>,
    /// Appearance of stealth for the querying player (protocol 19, from the point of view of the player
    /// selected in the viewer); missing without `MASKED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<SimLookInfo>,
    /// Ghost evoker module 26 (protocol 20; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evoke_ghost: Option<SimEvokeGhostInfo>,
    /// Ghost module 27 (protocol 20; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ghost: Option<SimGhostInfo>,
    /// Banishment module 31 (protocol 20; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banish: Option<SimBanishInfo>,
    /// Animal evoker module 32 (protocol 20; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evoke_animal: Option<SimEvokeAnimalInfo>,
    /// Influence and aura of module 44 (protocol 20): only with a non-empty influence list or an
    /// existing set of influenced objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub influence: Option<SimInfluenceInfo>,
    /// Demolition expert module 36 (protocol 21; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bomberman: Option<SimBombermanInfo>,
    /// Killer module 37 (protocol 21; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killer: Option<SimKillerInfo>,
    /// Smith module 49 (protocol 21; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub special_move: Option<SimSpecialMoveInfo>,
    /// Headhunter module 34 (protocol 22; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headhunter: Option<SimHeadhunterInfo>,
    /// Robbing module 48 (protocol 22; `None` without the module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robbing: Option<SimRobbingInfo>,
    /// AI mark (`GameObject+4`, protocol 23): the slots 0-7 that have marked the object
    /// (ascending); empty is missing from the JSON.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_marks: Vec<u8>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

/// Stealth module 25 (re/masking 1.2, 3; protocol 19). Times in ms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimMaskingInfo {
    /// `m_IsMasked`: camouflaging begun (not only once the status is `MASKED`).
    pub is_masked: bool,
    /// `m_IsBusy`: the camouflage or uncamouflage animation is running.
    pub is_busy: bool,
    /// `m_MaskTime`: duration of camouflaging and uncamouflaging.
    pub mask_time: i32,
    /// `m_NeverUnmasked` (`EVERYTIMETARNED`): never uncamouflages.
    pub never_unmasked: bool,
    /// Extra energy (attribute 15) as a fraction 0 to 100,000.
    pub energy: i32,
}

/// Assassin module 28 (re/masking 1.3, 4; protocol 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimAssassinInfo {
    /// `m_IsDown`: dug in (from the end of the dig-in animation).
    pub is_down: bool,
    /// `m_TargetHID`: target of the current stab (`None` without; the ID may point to a
    /// removed object).
    #[serde(default)]
    pub target: Option<u32>,
    /// `m_AttackPoints`: damage of a stab.
    pub attack_points: i32,
}

/// Healing module 29 (re/healing 1.2, 9; protocol 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimHealingInfo {
    /// `m_ReadyToHeal`: heals in this tick.
    pub ready: bool,
    /// `SINGLEHEALING`: single healing (the healer units), otherwise area healing.
    pub single: bool,
    /// `m_SickPersonHID`: target of the single healing (`None` without; may point to a removed object).
    #[serde(default)]
    pub sick: Option<u32>,
    /// `m_NewTarget`: requested target (`None` without).
    #[serde(default)]
    pub new_target: Option<u32>,
    /// `m_Healingpoints`: hit points per minute.
    pub healing_points: i32,
    /// Extra energy (attribute 15) as the current value with the upgrade percentages of the owner.
    pub mana: i32,
}

/// Message filter of an object (`MessageFilter_IsAllowed` `0x004E3400`, protocol 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimMessageFilterInfo {
    /// 0 = only the list allowed, 1 = list forbidden (default).
    pub mode: u8,
    /// Message codes of the list.
    pub codes: Vec<u32>,
}

/// Appearance of stealth for a viewer (re/masking 7, protocol 19).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimLookInfo {
    /// Replacement animation of the presentation (alarm post for foreign players: only the tree); `None` = that of the
    /// simulation.
    #[serde(default)]
    pub anim: Option<i32>,
    /// Opacity: 0.5 camouflaged, 1.0 opaque.
    pub alpha: f32,
}

/// Ghost evoker module 26 (re/ghosts-banish 1.2, 3; protocol 20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimEvokeGhostInfo {
    /// Status `EVOKING`: the evocation is running (from `0x41A` to `0x31A` or abort).
    pub evoking: bool,
    /// `m_DeadBodyHID`: corpse of the evocation (`None` without; may point to a removed object).
    #[serde(default)]
    pub dead_body: Option<u32>,
    /// `m_GhostHID`: most recently created ghost (`None` without; is never reset and may point to a
    /// removed object).
    #[serde(default)]
    pub ghost: Option<u32>,
    /// `m_ExtraEnergy`: cost of the last `0x11A`.
    pub extra_energy: i32,
    /// Extra energy (attribute 15) as the current value with the upgrade percentages of the owner.
    pub mana: i32,
    /// Base value of the extra energy (key-0 absolute value): a running ghost ties up its
    /// cost from it, its death and the abort give it back (C-257).
    pub mana_base: i32,
}

/// Ghost module 27 (re/ghosts-banish 1.4, 3; protocol 20). Without `Eq` because of `alpha`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGhostInfo {
    /// `m_Status`: 0 calm, 1 appearing, 2 fading out, 3 suction strike due.
    pub status: u8,
    /// `m_Alpha`: opacity while fading out.
    pub alpha: f32,
    /// `m_TargetHID`: suction target (`None` without; may point to a removed object).
    #[serde(default)]
    pub target: Option<u32>,
}

/// Banishment module 31 (re/ghosts-banish 1.3, 3; protocol 20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimBanishInfo {
    /// `ANIM_LOCKED` of the object: the banishment animation is running. The healing trance uses the same bit
    /// (C-254), so `active` shows it as well.
    pub active: bool,
    /// A timer `0x41F` is running: the banishing spell (effects) is shown.
    pub effects: bool,
    /// `m_SpellGuid`: button GUID of the last `0x11F`.
    pub spell_guid: i32,
    /// Banishment radius in fixed point (1/4096 tile) as with `0x21F`: sight (attribute 18, with the
    /// upgrade percentages of the owner) * 4096 >> 3, so 26,624 for sight 52.
    pub radius: i32,
}

/// Animal evoker module 32 (re/evoke-aura 1.2, 3; protocol 20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimEvokeAnimalInfo {
    /// Status `EVOKING`: the evocation is running (from `0x120` to `0x220` or abort).
    pub evoking: bool,
    /// `m_AnimType`: kind 0 wolf, 1 bear, 2 cat.
    pub art: i32,
    /// `m_ExtraEnergy`: cost of the running evocation.
    pub cost: i32,
    /// `m_UsedExtraEnergy`: reserve, sum of the costs of living animals (and of the running one).
    pub reserve: i32,
    /// `m_SpawnPos`: position at the start of casting in fixed point, `[x, y, z]`.
    pub spawn: [i32; 3],
    /// Extra energy (attribute 15) as the current value with the upgrade percentages of the owner.
    pub mana: i32,
}

/// Influence and aura of module 44 (re/evoke-aura 1.4, 3.6-3.9; protocol 20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimInfluenceInfo {
    /// GUIDs of the running influences (`m_InfluenceUpgrade`) in insertion order.
    pub guids: Vec<u32>,
    /// Influenced objects (`m_InfluenceHandles`; `None` = no set). The IDs may be
    /// stale.
    #[serde(default)]
    pub influenced: Option<Vec<u32>>,
    /// The aura ring (effect 450003) is to be shown (`influence::ring_shown`).
    pub ring: bool,
}

/// Entry of the list of temporary upgrades of an object (re/temp-upgrades 1.2;
/// protocol 16): flags from the record, GUID and remaining time in ms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimTempUpgrade {
    pub guid: u32,
    pub flags: u32,
    pub rest_ms: i32,
}

/// Spell module 38 (re/temp-upgrades 15, protocol 16). `mana`/`mana_max` are the current
/// value and maximum of the extra energy (attribute 15, with the upgrade percentages of the owner),
/// `casting` is status `ANIM_LOCKED` (the spell is running).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSpellInfo {
    /// Kind of the running spell (`+0x98`).
    pub art: i32,
    /// Button (`+0x94`).
    pub button: i32,
    /// Cost in extra energy (`+0xA4`).
    pub cost: i32,
    /// Index into the animation list (`+0xB0`).
    pub anim: i32,
    /// Index into the execution list of module 44 (`+0xAC`).
    pub idx: i32,
    /// Reserve of kind 3 (`+0xA8`).
    pub reserve: i32,
    pub mana: i32,
    pub mana_max: i32,
    pub casting: bool,
    /// `m_Position`: target point of kinds 1, 2 and 5 (world) while the spell is running (`casting`);
    /// otherwise it is missing (protocol 21, re/spells 1.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<[f32; 3]>,
    /// `m_deadBodyHID`: corpse of the reincarnation (kind 3; protocol 21); missing without a corpse, may
    /// point to a removed object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corpse: Option<u32>,
}

/// Demolition expert module 36 (re/special-attacks 1.2, 5; protocol 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimBombermanInfo {
    /// `m_TargetHID`: target of the order (`None` without; may point to a removed object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,
    /// `m_ExtraEnergy`: mana cost of the order (0 before the first order).
    pub extra_energy: i32,
    /// `m_ReturnPoint`: return point in fixed point, `[x, y, z]`.
    pub return_point: [i32; 3],
    /// `m_IsBusy`: is currently laying a charge.
    pub busy: bool,
    /// Extra energy (attribute 15) as the current value with the upgrade percentages of the owner.
    pub mana: i32,
}

/// Killer module 37 (re/special-attacks 1.3, 5; protocol 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimKillerInfo {
    /// `m_TargetHID`: target of the order (`None` without; may point to a removed object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,
    /// `m_ExtraEnergy`: mana cost of the order (0 before the first order).
    pub extra_energy: i32,
    /// `m_KillNearDamage`: damage with the knife (HP).
    pub near_damage: i32,
    /// `m_KillFarDamage`: damage at a distance (HP).
    pub far_damage: i32,
    /// Extra energy (attribute 15) as for [`SimBombermanInfo::mana`].
    pub mana: i32,
}

/// Smith module 49 (`SPECIALMOVE`, re/special-attacks 1.4, 5; protocol 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimSpecialMoveInfo {
    /// `m_IsTurnAround`: the attack animation is running.
    pub busy: bool,
    /// Kind of the running attack: 0 whirlwind, 1 hammering.
    pub mode: i32,
    /// Extra energy (attribute 15) as for [`SimBombermanInfo::mana`].
    pub mana: i32,
}

/// Headhunter module 34 (re/headhunter-robbing 1.2, 5; protocol 22): the fields of the
/// module in protocol names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimHeadhunterInfo {
    /// Kind of the type: 0 SPA (4100), 1 PRI (4101), 2 SID (4102); 255 for a foreign type GUID.
    pub art: u8,
    /// `m_LastTargetStatus`: 0 search, 1 to the unit, 2 to the building.
    pub state: i32,
    /// `m_TargetGUID`: target GUID of the contract (-1 after success).
    pub target_guid: i32,
    /// `m_TargetPlayerID`: target player (-1 after success).
    pub target_player: i32,
    /// `m_TargetHID`: most recently found object (missing without; may point to a removed one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,
    /// `m_LastTarget`: current target, unit or building (missing without; may be stale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_target: Option<u32>,
    /// `m_CurReward`: current price in gold.
    pub price: i32,
    /// `m_CustomerHL`: slot of the customer, -1 none.
    pub customer: i32,
    /// `m_CustomerCost`: what the customer has paid (refund on takeover).
    pub customer_cost: i32,
}

/// Robbing module 48 (re/headhunter-robbing 1.3, 3.9; protocol 22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRobbingInfo {
    /// `m_RobAnim[0]`: the robbing animation being played (-1 with an empty list).
    pub anim: i32,
    /// Currently robbing: status `ANIM_LOCKED` and timer `0x330` created.
    pub robbing: bool,
    /// Attack module `0x11` enabled (robbing disables it, only an abort enables it
    /// again).
    pub attack_enabled: bool,
    /// Extra energy (attribute 15) as for [`SimBombermanInfo::mana`].
    pub mana: i32,
}

/// An entry of the world-wide headhunter table (`GameWorld+0x4DC`, re/headhunter-robbing 1.4; protocol
/// 22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimHireEntry {
    /// Kind 0 SPA, 1 PRI, 2 SID.
    pub art: u8,
    /// ID of the headhunter of this kind (`null` without; a dead one that has not yet been removed
    /// stays).
    pub hunter: Option<u32>,
    /// Price of the next hiring like the tooltip (`FUN_0047DEE0`, `hire_price`).
    pub price: i32,
}

/// Running counter-upgrade (`m_CounterUpgrades`, re/upgrades 3.6; protocol 16). The
/// source (raw ID) may be missing if the building has died or been removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimCounterUpgrade {
    pub guid: u32,
    pub rest_ms: i32,
    pub source: u32,
}

/// Tent module 42 (re/tents 1.2, 5; protocol 17). `travois` is the raw ID
/// (`u32::MAX` = none, `ObjectId::NONE`; may point to a removed object). `progress` is
/// the crossfade factor of the object (`progress_scale`), `hidden` is building sleep without selectability
/// (tent hidden).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimTentInfo {
    /// `m_TentStatus`: 0 at rest, 1 fade-out, 2 fade-in.
    pub status: i32,
    /// `m_TentCounter`: remaining time of the crossfade in ms.
    pub counter: i32,
    /// `progress_scale` of the object (crossfade factor).
    pub progress: f32,
    /// `m_TravoisGuid` (430001).
    pub travois_guid: u32,
    /// `m_TravoisHID`: most recently created travois (`u32::MAX` = none).
    pub travois: u32,
    /// `is_sleeping` and not selectable.
    pub hidden: bool,
}

/// Travois module 35 (re/tents 1.3; protocol 17). `tent` is the raw ID of the
/// carried tent (`u32::MAX` = none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimTransportTentInfo {
    /// `m_TentHID`.
    pub tent: u32,
    /// Flag `TRANSPORTINGTENT` (`0x100`) set.
    pub transporting: bool,
}

/// Trade module 43 (re/trading 1.2, 5; protocol 17). The carts have no gatherer module,
/// so their load is here (`GameObject+0x2A8/+0x2AC`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimTradeInfo {
    /// `m_TradeWare`: trade kind 0 to 3 (buy wood, buy food, sell wood, sell
    /// food).
    pub ware: i32,
    /// `m_TradeTarget`: target slot 0 to 8.
    pub target: i32,
    /// `m_TaxLevel` ("tax dial" 0 to 100).
    pub tax_level: i32,
    /// `m_MaxTax` (440003).
    pub max_tax: i32,
    /// `m_TaxInterval` in ms.
    pub tax_interval: i32,
    /// `m_PayerGUIDs` (440002).
    pub payers: Vec<i32>,
    /// Carried amount (`+0x2A8`).
    pub load_amount: i32,
    /// Kind of the load (`+0x2AC`, resource index).
    pub load_kind: i32,
}

/// Train module 33 (re/train-multipart 1.1, 5; protocol 15). IDs as numbers, `u32::MAX`
/// = none (`ObjectId::NONE`); grid and fixed-point values as in the state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimTrainInfo {
    /// `m_SlaveList`: only the locomotive; tender and wagons in coupling order.
    pub slaves: Vec<u32>,
    /// `m_MasterHID`: slave -> locomotive (`u32::MAX` for the locomotive).
    pub master: u32,
    /// `m_GameSpeed` and `m_NextGameSpeed`: distance per tick (fixed point).
    pub speed: i32,
    pub next_speed: i32,
    /// `m_AccTime`: time index of the travel curve in ms.
    pub acc_time: i32,
    /// `m_Status`: bit 0 accelerate, 1 travel, 2 brake, 3 forward, 4 backward.
    pub status: u32,
    /// `m_IsSlave`: tender or wagon.
    pub is_slave: bool,
    /// `m_Direction`: travel vector (fixed point x, z).
    pub dir: [i32; 2],
    /// Target tile (`m_ZielRasterPos`, only with `m_IsZielRasterPosValid`).
    pub target: Option<[i32; 2]>,
    /// `m_TimeCounter`: time left until automatic departure in ms.
    pub time_counter: i32,
    /// `m_HasSupplyLoaded`.
    pub has_supply: bool,
    /// `m_OutsideStatus`: 0 inside the map, 1 out backward, 2 out forward.
    pub outside: i32,
    /// `m_RasterDirection` and last blocked tile (`+0xC8/+0xCC`).
    pub raster_dir: [i32; 2],
    pub last_block_tile: [i32; 2],
}

/// Multipart object (re/train-multipart 1.2, 1.3; protocol 15). `head` indicates a front part
/// (module 11: `linked`, `last_pos` apply); without `head` the object is a rear part (module 51:
/// `prev`, `next`, `offset`, `centre` apply). Fields that do not apply: IDs `u32::MAX`, otherwise 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimMultiPartInfo {
    /// Front part (`MultiHead`).
    pub head: bool,
    /// `m_LinkedHID` of the front part: the rear part.
    pub linked: u32,
    /// `m_PreviousHID` and `m_NextHID` of the rear part.
    pub prev: u32,
    pub next: u32,
    /// `m_CentreOffset`: drawbar length (fixed point).
    pub offset: i32,
    /// `m_CentreOfRotation` of the rear part and `m_LastPos` of the front part (x, z, direction;
    /// fixed point).
    pub centre: [i32; 3],
    pub last_pos: [i32; 3],
}

/// Riding module 13 or 14 (re/riding 1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimRidingInfo {
    /// Module index (13 rider, 14 canoe occupant).
    pub index: u32,
    /// Modul 14 (`RidingBoat`).
    pub boat: bool,
    /// `m_mountHID`: mount or canoe, also during the approach (`None` = none).
    pub mount_target: Option<u32>,
    /// Approach to the mount is running (`m_GoingToHorse`).
    pub going_to_horse: bool,
    /// `m_isBusy`.
    pub busy: bool,
    /// `m_nextMountAnim`: 0 left, 1 right, otherwise none.
    pub next_mount_anim: i32,
    /// `m_lanceAvailable`.
    pub lance_available: bool,
}

/// Dive module 24 (re/swim-dive 1.2, 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimDivingInfo {
    /// `m_Status`: 0 surfaced, 1 submerged, 2 sinking, 3 rising.
    pub status: i32,
    /// `m_DiveCount`: dive depth, -1024 ... 0.
    pub count: i32,
    /// Walk layer at init and dive layer.
    pub orig_layer: i32,
    pub dive_layer: i32,
    /// Air (attribute 28, current), `0` without the attribute.
    pub air: i32,
    /// Depth offset of the object (fixed point, `height_offset`).
    pub height_offset: i32,
}

/// Ranged combat module 19 and shot module 10 (re/combat-ranged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimRangedInfo {
    /// Minimum and maximum distance (fixed point, `m_MinDist`/`m_MaxDist`).
    pub min_dist: i32,
    pub max_dist: i32,
    pub shoot: Option<SimShootInfo>,
}

/// Shot module 10.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimShootInfo {
    /// Module flags (kind, `RELOAD`, `CANSHOOTFIREARROW` ...).
    pub flags: u32,
    pub target: Option<u32>,
    /// Magazine: fired (`m_BulletsCur`, shots since the last reload) and size; empty when
    /// fired >= size.
    pub bullets: [i32; 2],
    pub shoot_delay: i32,
    pub busy: bool,
    /// 1: target is a game animal.
    pub gun_select: i32,
    /// Broadside of the warships (protocol 15, re/ships 3.6): selected side
    /// (`m_IsSteuerboard`), cannon counter (`m_CannonCounter`, after aiming number of cannons + 1) and
    /// `m_CanShoot`.
    #[serde(default)]
    pub is_steuerboard: bool,
    #[serde(default)]
    pub cannon_counter: i32,
    #[serde(default)]
    pub can_shoot: bool,
}

/// Animal module 20 (re/animals 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimAnimalInfo {
    pub aggressive: bool,
    pub shy: bool,
    pub roamer: bool,
    pub roaming: bool,
    pub escaping: bool,
    pub underwater: bool,
    /// Home (x, z, height in fixed point).
    pub home: [i32; 3],
    pub alpha: f32,
}

/// Carcass module 21 (re/gathering A.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimEdibleInfo {
    /// Meat current and maximum (attribute 24).
    pub meat: [i32; 2],
    /// Flag "has meat" (`0x200`) and "rotting" (`0x100`, from `0x215`).
    pub has_meat: bool,
    pub rotting: bool,
}

/// A projectile of the simulation (re/combat-projectiles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimProjectileInfo {
    pub id: u32,
    /// `arrow`, `ball`, `dynamite`, `explosion` or `storm` (the last three are world effects:
    /// in the savegame from format 16, in the query from protocol 21); `throw` (dynamite throw of the
    /// headhunter) from protocol 22.
    pub kind: String,
    /// Fire arrow (flag `0x40`).
    pub fire: bool,
    pub remaining_ms: i32,
    pub flight_ms: i32,
    pub shooter: Option<u32>,
    pub target: Option<u32>,
    pub target_pos: [f32; 3],
    /// Pulse counter of the storm (`kind` `storm`; protocol 21): 0 or 1; otherwise missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse: Option<i32>,
    /// Impact message of the throw (`kind` `throw`; protocol 22): `0xD22` unit, `0xE22`
    /// building; otherwise missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<u32>,
}

/// Combat state of an object (re/combat-host 7, re/combat-damage).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimCombatInfo {
    /// Hit points current and maximum (attribute 14).
    pub hp: [i32; 2],
    /// Hit point fraction in 1/100,000 (`AttributeSet_GetCurrentFraction`, not negative).
    pub hp_fraction: i32,
    /// Rank 0 ... 5 (`m_LastExperience`).
    pub rank: u8,
    /// Experience fraction (attribute 30) in 1/100,000.
    pub experience: i32,
    /// Last attacker (`None` without).
    pub last_attacker: Option<u32>,
    /// Ticks since the last hit.
    pub ticks_since_hit: i32,
    /// Render bits (`+0x1C4`).
    pub render_bits: u32,
    /// Attack module 17 (`None` without).
    pub attack: Option<SimAttackInfo>,
    /// Dying module 16 (`None` without).
    pub dying: Option<SimDyingInfo>,
}

/// State of attack module 17 (re/combat-attack).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimAttackInfo {
    /// `m_Status`: 0 calm, 1 approach, 2 blow/shot in progress.
    pub status: i32,
    /// Target (`m_TargetHID`; `None` without).
    pub target: Option<u32>,
    /// Target chosen by the target search (`m_AutomaticTarget`); false for a commanded attack.
    pub automatic: bool,
    /// `m_Aggressiveness`: 1 no combat, 2 retaliation, 3 escalatable, 4 target search.
    pub aggressiveness: i32,
    /// Stance (`m_AttackMode`) 0 ... 4.
    pub mode: i32,
    /// `m_Stubbornness`: stance, command, movement.
    pub stubbornness: [i32; 3],
    /// `m_SearchCounter`.
    pub search_counter: i32,
}

/// State of dying module 16 (re/combat-death).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimDyingInfo {
    /// 0 calm, 1 died (callbacks pending), 2 sinking.
    pub status: u8,
    /// Remaining time of sinking in ms (`m_DieCounter`).
    pub die_counter: i32,
    /// Gold when robbing (`m_RobGold`).
    pub rob_gold: i32,
    /// Number of registered death callbacks.
    pub listeners: usize,
}

/// State of a building (re/buildings, re/construction). Times in ms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimBuildingInfo {
    /// Finished (flag `CONSTRUCTED_FINISHED` `0x403`).
    pub finished: bool,
    /// Construction site counts as "in progress" (`m_IsBuildBuilding`).
    pub under_construction: bool,
    /// Remaining work and construction time (`m_BuildCounter`, `m_BuildTime`).
    pub build_rest: i32,
    pub build_time: i32,
    /// Construction progress of the presentation 0.1 ... 1 (`GameObject+0x18`) and render bits (`+0x1C4`).
    pub progress: f32,
    pub render_bits: u32,
    /// Flags of the building module.
    pub flags: u32,
    /// Running order (`None` without).
    pub production: Option<SimProductionInfo>,
    /// Waiting GUIDs (`m_MakeQueue`), index 1 ... of `sim_cancel`.
    pub queue: Vec<u32>,
    /// Docking slots occupied and total.
    pub docks: [usize; 2],
    /// Rally point (`m_MeetingPoint`) in fixed-point tiles and world (x, z).
    pub meeting_point_fx: [i32; 2],
    pub meeting_point: [f32; 2],
    /// Master and members of the wall group (`None`/empty without).
    pub master: Option<u32>,
    pub slaves: Vec<u32>,
    /// Sleeping (`m_IsSleeping`), dying or dead (`m_IsDead`).
    pub sleeping: bool,
    pub dead: bool,
    /// Gate state (protocol 18): only for a building with `TOR` or a gate bit
    /// (re/arming-gates 3.C).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<SimGateInfo>,
    /// Area watch (protocol 18): only with a running query (`check_area_id != -1`,
    /// re/arming-gates 3.D).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_watch: Option<SimAreaWatchInfo>,
    /// Stored watch rectangle `{x0, z0, x1, z1}` in tiles (`check_area_rect`), also with a
    /// finished query; only if it is not 0 (protocol 18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_rect: Option<[i32; 4]>,
}

/// Gate state of a building (protocol 18, re/arming-gates 3.C): `TOROPEN` (`open`), `TORCLOSE`
/// (`closed`) and the door tiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimGateInfo {
    pub open: bool,
    pub closed: bool,
    /// Door tiles (rectangle tiles with mask byte 0, z outer, x inner).
    pub doors: Vec<SimDoorTile>,
}

/// A door tile: tile and whether it is blocked in the path grid (layer 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimDoorTile {
    pub x: i32,
    pub z: i32,
    pub blocked: bool,
}

/// Area query of a building (protocol 18, re/arming-gates 3.D and 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimAreaWatchInfo {
    /// Key of the query (`check_area_id`).
    pub key: i32,
    /// Filter mode of the query (`0xA410` enemy watch, `0x4A010` capture).
    pub mode: u32,
    /// Rectangle `{x0, z0, x1, z1}` in tiles (inclusive).
    pub rect: [i32; 4],
    /// Reference player of the filter (`< 0` = owner).
    pub player: i32,
    /// Listed objects in entry order (IDs).
    pub members: Vec<u32>,
}

/// Running production order (`m_MakeGuid`, `m_MakeTime`, `m_MakeStartTime`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimProductionInfo {
    pub make_guid: u32,
    /// Remaining and total duration in ms (total 0 = not yet paid).
    pub rest: i32,
    pub total: i32,
}

/// State of a gatherer (re/gathering).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGathererInfo {
    /// Module slot 7 gold, 8 wood, 12 harvest, 22 hunting, 55 fishing and name (`mining`, `lumber`,
    /// `harvest`, `hunting`, `fishing`).
    pub module: u8,
    pub kind: String,
    /// `m_Status`: 0 free, 1 to the source, 2 to the drop-off building, 3 drop-off, 6 search again; work 300
    /// gold, 200/201 wood, 100 harvest, 400 fishing, 100/101/102 hunting.
    pub state: i32,
    /// Carried load: amount and kind (0 gold, 1 wood, 2 food, -1 none).
    pub load_amount: i32,
    pub load_kind: i32,
    /// Source (`m_ResourceHID`; hunting: `m_TargetHID`, the game or the carcass) and drop-off building
    /// (`m_HQHID`); `None` without (also ID 0).
    pub source: Option<u32>,
    pub store: Option<u32>,
    /// Hunting: animal or carcass (`m_TargetHID`, protocol 12); `None` without and for other gatherers.
    #[serde(default)]
    pub prey: Option<u32>,
}

/// State of a construction worker (re/construction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimConstructInfo {
    /// `m_Status`: 0 free, 1 begun, 2 building, 3 arrived, 4 on the way, 5 repairing, 6 target finished.
    pub state: i32,
    /// Target (`m_BuildingHID`; `None` without, also ID 0).
    pub building: Option<u32>,
}

/// State of quarters (re/garrison-and-commands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimQuarterInfo {
    /// Occupied slots and capacity (`m_CurQuarterCount`, `m_MaxQuarterCount`).
    pub occupied: i32,
    pub max: i32,
    /// Occupants in order of moving in.
    pub occupants: Vec<u32>,
    /// Meeting point of the mobile container (`+0xD4/+0xD8`, fixed-point x, z; protocol 15): set
    /// with `0x1E17` (status 1) and then left unchanged, also after arrival (status 2) and
    /// after the trip continues (status 0); `None` as long as none has ever been set.
    #[serde(default)]
    pub meeting_point: Option<[i32; 2]>,
    /// Those waiting at the meeting point (`m_WaitingList`, protocol 15).
    #[serde(default)]
    pub waiting: Vec<u32>,
    /// `m_Status` of the mobile container: 0 free, 1 driving to the meeting point, 2 arrived
    /// (protocol 15).
    #[serde(default)]
    pub status: i32,
}

/// Remaining amount of a deposit (attribute 26 gold or 24 food).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimResourceInfo {
    pub attribute: u32,
    pub amount: i32,
}

/// State of a tree (re/gathering).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimTreeInfo {
    /// Remaining wood (attribute 25).
    pub wood: i32,
    /// Falling (`m_Status` 1).
    pub falling: bool,
    /// Registered lumberjacks per slot (`m_RegPos`, at most two).
    pub registered: Vec<u32>,
}

/// State of the walk module (re/walk). Fixed-point values in 1/4096 tile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimWalkInfo {
    /// `m_Status`: 1 rest, 2/5/11 walking, 3 waiting, 4 planning, 6 parked, 7 check position,
    /// 8 wait patiently, 9 new attempt.
    pub state: i32,
    /// `m_EndeErreicht`: 0 goal, 1 continue, 2 blocked, 3 ended without goal, 4 error.
    pub ende: i32,
    /// Goal of the running FINDTO, otherwise the last waypoint (fixed point and world, x, z).
    pub goal_fx: Option<[i32; 2]>,
    pub goal: Option<[f32; 2]>,
    /// Remaining waypoints (world, x, z) and trajectory points of the look-ahead.
    pub waypoints: Vec<[f32; 2]>,
    pub trajectory: usize,
    /// Speed per tick (current, cruising) and whether the unit is running.
    pub speed: i32,
    pub cruising: i32,
    pub running: bool,
    /// Path grid layer 0-8.
    pub layer: u8,
    /// Last result of the local planner (8 = replanning requested).
    pub planner: i32,
    /// Object the unit is waiting for.
    pub wait_for: Option<u32>,
    /// Group ID (`+0x208`), `None` without a group (protocol 13).
    #[serde(default)]
    pub group: Option<i32>,
    /// Rank in the formation (`formation_num`, protocol 13).
    #[serde(default)]
    pub formation_num: i32,
}

/// A player slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimPlayerInfo {
    pub slot: u8,
    /// `empty`, `human`, `ai`, `passive`.
    pub control: String,
    pub name: String,
    pub nation: i32,
    pub team: i32,
    pub color: i32,
    /// Gold, wood, food, population used, living space in whole units.
    pub resources: [i32; 5],
    /// Friends (bit mask of the slots).
    pub friends: u32,
    /// Population used and living space clamped to the limit (`Player_GetResource`,
    /// protocol 9).
    #[serde(default)]
    pub population: [i32; 2],
    /// Object counter per GUID (`Player_GetGuidCount` without groups) and "in progress" (construction sites,
    /// queued production); only entries != 0 (protocol 9).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<u32, i32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending: BTreeMap<u32, i32>,
    /// Enemies as a bit mask of the slots (protocol 10).
    #[serde(default)]
    pub enemies: u32,
    /// Shared vision as a bit mask: this player sees what the slots see (protocol 10).
    #[serde(default)]
    pub vision: u32,
    /// Applied upgrades in order of application (`applied_upgrades`, protocol 16; empty
    /// is missing from the JSON).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upgrades: Vec<u32>,
    /// Attack reports of the slot (protocol 23) in MFC order, i.e. in the order in which
    /// `0x512E` chooses them on a tie; empty is missing from the JSON.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attack_reports: Vec<SimAttackReport>,
}

/// An attack report (protocol 23, re/ki-queries 5.11): the hit object and
/// the tile it stood on at the last hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimAttackReport {
    /// ID of the victim (`index<<16 | slot<<11 | generation`).
    pub victim: u32,
    /// Tile `[x, z]` of the victim at the last hit.
    pub tile: [i32; 2],
}

/// Result of `sim_query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimQueryResult {
    pub tick: u32,
    pub steps: u64,
    /// Total matches of the filter (`objects` contains at most `limit`).
    pub total: usize,
    pub objects: Vec<SimObjectInfo>,
    pub players: Vec<SimPlayerInfo>,
    /// Running counter-upgrades in insertion order (protocol 16).
    #[serde(default)]
    pub counter_upgrades: Vec<SimCounterUpgrade>,
    /// Time of day of the time-of-day upgrades (`daytime`, -1 none yet, otherwise 0-3; protocol 16).
    #[serde(default)]
    pub daytime: i32,
    /// Headhunter table per kind 0-2 (protocol 22): always three entries.
    #[serde(default)]
    pub headhunters: Vec<SimHireEntry>,
    /// Kill counters of the scripts (`SimState::kill_counts`, protocol 30) as `[slot 0-7, type GUID of the
    /// victim, count]`, ascending by slot and GUID.
    #[serde(default)]
    pub kill_counts: Vec<(u8, u32, i32)>,
    /// Build button lock of the scripts (`SimState::button_locks`, protocol 30) as `[type GUID, 0/1]`,
    /// ascending by GUID; the lock applies to all slots.
    #[serde(default)]
    pub button_locks: Vec<(u32, u8)>,
    /// Is slot 0-10 running (`SimState::slot_running`, protocol 30)? `false` = hidden. Always eleven
    /// entries.
    #[serde(default)]
    pub slot_running: Vec<bool>,
    /// Named view rectangles of the scripts (`Fog::area_views`, protocol 30) in append order.
    #[serde(default)]
    pub area_views: Vec<SimAreaView>,
}

/// A named view rectangle in `sim_query` (protocol 30, `Fog::area_views`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimAreaView {
    pub name: String,
    /// Rectangle `[x0, z0, x1, z1]` in tiles, as the area was when created (not normalized).
    pub rect: [i32; 4],
    /// Slot 0-7 to which the rectangle gives sight.
    pub player: u8,
    /// Also stamps "cleared".
    pub clear: bool,
}

/// Result of `sim_hash`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimHashResult {
    pub tick: u32,
    pub steps: u64,
    pub checksum: String,
    /// (Engine tick, checksum) of the last ticks, oldest first.
    pub history: Vec<(u64, String)>,
}

/// Result of `sim_save`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSaveResult {
    pub path: String,
    pub bytes: usize,
    pub steps: u64,
    pub checksum: String,
}

/// Result of `sim_path`. Fixed-point values in 1/4096 tile, world values in world units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimPathResult {
    /// Unit (`None` for `from`).
    pub unit: Option<u32>,
    pub layer: u8,
    pub layer_name: String,
    pub from_fx: [i32; 2],
    pub to_fx: [i32; 2],
    /// Failure (`StartBlocked`, `GoalNotAdjacent`, `OutsideMap`, `Unreachable`, `NoPath`,
    /// `TooLong`); then the lists are empty.
    pub failure: Option<String>,
    /// Goal point of the message (tile center for a substitute goal).
    pub goal_fx: Option<[i32; 2]>,
    pub goal: Option<[f32; 2]>,
    /// Start tile after a substitute start.
    pub start_tile: Option<[i32; 2]>,
    /// Tile path (start tile to the cell before the goal) and the tiles kept after smoothing.
    pub tile_path: Vec<[i32; 2]>,
    pub smoothed: Vec<[i32; 2]>,
    pub waypoints_fx: Vec<[i32; 2]>,
    pub waypoints: Vec<[f32; 2]>,
    pub expansions: u32,
    /// Arguments of the message `0x500` to the unit.
    pub message: Vec<i32>,
}

/// Result of `sim_grid`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGridResult {
    /// 0-8 layer, 9 occupancy.
    pub layer: u8,
    pub name: String,
    pub width: i32,
    pub height: i32,
    /// Blocked (or occupied) cells of the whole map.
    pub blocked: usize,
    /// Regions and ID counter of the layer (occupancy: 0).
    pub regions: usize,
    pub counter: i32,
    /// Regions built.
    pub built: bool,
    pub rect: Option<[i32; 4]>,
    /// Rows of the excerpt (z ascending, x inner): layer -1 = blocked, otherwise region ID;
    /// 0/1 for occupancy; outside the map -2.
    pub rows: Vec<Vec<i32>>,
}

/// Result of `sim_fog` (protocol 10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimFogResult {
    /// Viewer (default: active player).
    pub player: u8,
    /// Size of the fog grids in tiles.
    pub width: i32,
    pub height: i32,
    /// Tile counts of the grids "explored", "seen" and "cleared" (foreground) with the
    /// sight mask of the player (shared vision included).
    pub explored: usize,
    pub seen: usize,
    pub cleared: usize,
    /// Cycle fields `m_FogHandlesProcessed` (0 new cycle, > 0 running, -1 idle tick, -2 alpha),
    /// `m_FogCurPlayerID` (0-8) and `m_FogHandlesPerTick`.
    pub processed: i32,
    pub cycle_player: u8,
    pub per_tick: i32,
    /// Number of sight sources.
    pub sources: usize,
    pub rect: Option<[i32; 4]>,
    /// Rows of the excerpt (z ascending, x inner): state `GameWorld_GetFogOfWarStateAt`
    /// (0 unexplored, 1 explored, 3 seen, 7 cleared); outside the map -2.
    pub rows: Vec<Vec<i32>>,
}

/// Result of `sim_build`: the dispatched construction (`0x01`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimBuildResult {
    /// Engine tick before which the command takes effect.
    pub step: u64,
    pub guid: u32,
    /// Snapped and checked position (world) and rotation (radians).
    pub pos: [f32; 3],
    pub rotation: f32,
    /// Tile rectangle `[x0, z0, x1, z1]` of the check.
    pub rect: Option<[i32; 4]>,
    /// Workers of the command (own objects of the given list or the selection).
    pub workers: Vec<u32>,
    /// The account suffices; otherwise the simulation rejects the construction with a message.
    pub affordable: bool,
    /// Queued (Shift).
    pub append: bool,
}

/// Result of `sim_action`: the selected right-click action and its command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimActionResult {
    pub step: u64,
    /// `BUTTONMSG` of the selected `targets.ini` entry (e.g. 400 chop wood) and its name.
    pub button: i32,
    pub action: String,
    /// Command (variant of `opennml_sim::command::CommandKind`, e.g. `ChopWood`) and its type code
    /// (e.g. 6), without the wrapping of the queue keys.
    pub command: String,
    pub code: u32,
    /// Wrapped in `0x7B` (append) or `0x7C` (cyclic).
    pub queued: bool,
    pub cyclic: bool,
    /// The dispatched command in JSON form.
    pub kind: serde_json::Value,
}

/// Entry of a construction or production list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimMenuEntry {
    pub guid: u32,
    pub name: String,
    /// Kind of the button (`toolbar.ini`): `BUILD`, `ONLYONCE`, `MAKE`, `MAKEONLY_ONCE`,
    /// `UPGRADE` (otherwise the number in hexadecimal notation); counter-upgrades are not in the
    /// lists but appear as buttons of the unit bar.
    pub kind: String,
    /// Gold, wood, food, population, living space; `None` without a value set.
    pub costs: Option<[i32; 5]>,
    /// The account suffices.
    pub affordable: bool,
    /// Blocking reason of the command check (`None` = selectable).
    pub blocked: Option<String>,
}

/// Entry of the queue of a building (index as in `sim_cancel`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimQueueEntry {
    pub index: i32,
    pub guid: u32,
    pub name: String,
    /// Progress 0 ... 1 of the running order; waiting ones `None`.
    pub progress: Option<f32>,
}

/// Result of `sim_menu`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimMenuResult {
    /// Building of the production list (`None` for the construction list).
    pub building: Option<u32>,
    /// Workers of the construction list (empty for the production list).
    pub workers: Vec<u32>,
    pub entries: Vec<SimMenuEntry>,
    /// Queue of the building (empty for the construction list).
    pub queue: Vec<SimQueueEntry>,
}

/// Result of `sim_hire_menu` (protocol 22, re/headhunter-robbing 2.5): levels 2 and 3 of the
/// headhunter menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimHireMenuResult {
    /// Target players (`FUN_0044A8C0`): playing enemies with a free color button, ascending.
    pub players: Vec<u8>,
    /// Target GUIDs (`FUN_0044AA10`) of the requested target player; empty without one.
    pub targets: Vec<u32>,
}

/// A direct change that an AI query would perform in the game (protocol 23); `ki_query`
/// only reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiDirectInfo {
    /// `"mark"` / `"unmark"` (set / clear the AI mark of slot `player` on the object) or
    /// `"take_report"` (remove the attack report of victim `object` from the table of slot `player`).
    pub kind: String,
    /// Affected object (mark) or victim (report), raw as ID. Always set today;
    /// the `Option` stays open for future kinds without an object.
    pub object: Option<u32>,
    /// Slot whose mark or report table is affected.
    pub player: u8,
}

/// Result of `ki_query` (protocol 23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiQueryResult {
    /// The requested query code (`0x4E20`...`0x5130`).
    pub code: u32,
    /// The whole request field after the response: `[code, argc, [2], [3], ...]` (indices as in
    /// re/ki-queries), padded to `2 + max(argc, 128)` words; a failure leaves
    /// the inputs unchanged (E-27).
    pub request: Vec<i32>,
    /// Direct changes in execution order; not executed.
    pub direct: Vec<KiDirectInfo>,
}

/// A thread of an AI engine in `ki_state` (protocol 26), `KIModule` fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiThreadInfo {
    /// Thread ID (`m_iPositionID`; a child carries that of its caller).
    pub id: i32,
    pub module: u32,
    /// Name of the module from `nml.db`; empty without an entry.
    pub name: String,
    /// `m_iCodePt`.
    pub pc: u32,
    /// `m_lStatus` (1 running, 4 finished, 0x10 waiting for the child).
    pub status: u32,
    /// Remaining sleep in ms (`m_iPauseCounter`).
    pub pause_ms: i32,
    /// Semaphore the thread is waiting for; -1 none.
    pub wait_semaphore: i32,
    pub stack_depth: usize,
    /// The top 8 values at most, topmost last.
    pub stack_top: Vec<i32>,
    /// Running `CALLM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<Box<KiThreadInfo>>,
}

/// An AI engine in `ki_state` (protocol 26).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiEngineInfo {
    pub slot: u8,
    /// Next thread ID (`m_iHandleID`).
    pub next_thread_id: i32,
    /// Threads in execution order (MFC order of the table).
    pub threads: Vec<KiThreadInfo>,
    /// `m_iValues` as `[key, value]`, ascending; empty without `values`.
    pub values: Vec<(i32, i32)>,
    /// Semaphores as `[key, occupied]`, ascending.
    pub semaphores: Vec<(i32, u8)>,
}

/// Rejected AI commands per slot, network command and reason (`ki_state`, from `SimEvent::KiCommand`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiRejectCount {
    pub slot: u8,
    /// Network command `0x01...0x7E`.
    pub code: u32,
    pub reason: String,
    pub count: u64,
}

/// AI commands per slot, network command and result (`ki_state`, command statistics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiCommandCount {
    pub slot: u8,
    pub code: u32,
    /// `"accepted"`, `"partial"` or `"rejected"`.
    pub outcome: String,
    pub count: u64,
}

/// Result of `ki_state` (protocol 26). The counters are host state since the last
/// restart or load, not part of the simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiStateResult {
    /// The whole AI is paused (`KiPause`).
    pub paused: bool,
    /// Engines ascending by slot.
    pub engines: Vec<KiEngineInfo>,
    /// Rejections (outcome `Rejected`), by slot, code, reason.
    pub rejected: Vec<KiRejectCount>,
    /// All AI commands by slot, code, result.
    pub commands: Vec<KiCommandCount>,
}

/// A block of a script container in `script_state` (protocol 28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptBlockInfo {
    /// Index in the script file.
    pub index: u32,
    /// `[BESCHREIBUNG` text from the 16th character (like `m_TriggerName`, untrimmed).
    pub name: String,
    /// `"bereit"` (conditions are checked, deleted after execution), `"wartet"` (stops at
    /// `cur_action`) or `"wiederholbar"` (action 10: not deleted after execution).
    pub state: String,
    /// Wait reason (only `"wartet"`): action, number and kind or remaining time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<String>,
    /// `m_CurActionIndex`.
    pub cur_action: i32,
}

/// A script container in `script_state` (protocol 28): level 1 (protocol 30: the level container has ID 1),
/// dialog -1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptContainerInfo {
    pub id: i32,
    /// Script file (normalized path).
    pub path: String,
    /// Countdown in ms (-1 = not set).
    pub counter: i32,
    /// Namespaces as `[name, value]`, ascending by name.
    pub switches: Vec<(String, i32)>,
    pub timers: Vec<(String, i32)>,
    pub variables: Vec<(String, i32)>,
    pub random_switches: Vec<(String, i32)>,
    /// Remaining blocks in file order.
    pub blocks: Vec<ScriptBlockInfo>,
}

/// A condition of the selected block in `script_state` (protocol 28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptConditionInfo {
    pub number: i32,
    /// Prefix other than `&` (OR combination; has no effect for the first condition).
    pub or: bool,
    pub params: Vec<String>,
    /// Current value, evaluated on a copy of the state; `None` = not yet implemented.
    pub value: Option<bool>,
}

/// Running dialog line in `script_state` (protocol 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptLineInfo {
    pub text: i32,
    pub remaining_ms: i32,
    pub slot: i32,
}

/// Tip of one of the two slots in `script_state` (protocol 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptTipInfo {
    pub remaining_ms: i32,
    pub text: i32,
}

/// Speech state in `script_state` (protocol 28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSpeechInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<ScriptLineInfo>,
    /// Slots 0 and 1.
    pub tips: Vec<ScriptTipInfo>,
    /// Text events "text ended" in insertion order.
    pub events: Vec<i32>,
    /// Info history (text IDs).
    pub info_history: Vec<i32>,
}

/// Entry of the block log in `script_state` (protocol 28): step (1-based, the step in which it was
/// reported) and the message as the JSON form of `opennml_sim::script::ScriptTrace`, e.g.
/// `{"Fired": {"container": 0, "block": 3}}` or `"DialogEnded"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptLogEntry {
    pub step: u64,
    pub trace: serde_json::Value,
}

/// Result of `script_state` (protocol 28). The block log is host state (the last
/// 1,000 steps since the last restart or load), not part of the simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptStateResult {
    /// Script enabled in the setup of the match.
    pub enabled: bool,
    /// A dialog is running (AI and level script rest).
    pub dialog_running: bool,
    /// Level container, then the dialog container.
    pub containers: Vec<ScriptContainerInfo>,
    /// Conditions of the requested block; missing without `block`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<ScriptConditionInfo>>,
    pub speech: ScriptSpeechInfo,
    /// Object names as `[name, value]` in the order of the name table (MFC order).
    pub names: Vec<(String, String)>,
    pub log: Vec<ScriptLogEntry>,
    /// Main objectives 0-9 (protocol 30): always ten entries.
    #[serde(default)]
    pub main_quests: Vec<ScriptQuestInfo>,
    /// Sub-objectives 0-9 (protocol 30): always ten entries.
    #[serde(default)]
    pub sub_quests: Vec<ScriptQuestInfo>,
    /// ID of the container whose countdown is displayed (action 15; 0 = none).
    #[serde(default)]
    pub countdown_container: i32,
    /// Displayed countdown seconds (`counter / 1000`).
    #[serde(default)]
    pub countdown_s: i32,
    /// Task status of action 200 as `[task, 0/1]`, ascending.
    #[serde(default)]
    pub task_status: Vec<(i32, u8)>,
    /// Changed areas (`ScriptState::area_overrides`) as `[name, [x0, z0, x1, z1]]`, ascending by
    /// name; they take precedence over the rectangles of the map.
    #[serde(default)]
    pub area_overrides: Vec<(String, [i32; 4])>,
    /// Script commands (protocol 32): counter per action and the latest rejections with reason.
    #[serde(default)]
    pub commands: ScriptCommandsInfo,
    /// Game result (protocol 32): the decided slots 0-7 as `[slot, result]` (1 lost, 2 won),
    /// ascending by slot; playing (0) and unoccupied slots (3) are missing.
    #[serde(default)]
    pub results: Vec<(u8, i32)>,
    /// State of the controls (protocol 33): lock, cinemascope, selection mirror and last button.
    #[serde(default)]
    pub ui: ScriptUiInfo,
}

/// State of the controls in `script_state` and `script_events` (protocol 33, `MissionState`): `ui_locked` (actions
/// 18/19), `cinemascope` (147/148), the selection mirror `selection` (host command `ScriptSelection`, in report order,
/// at most 30 IDs) and the last button `last_button` (host command `ScriptButton`, 0 = none).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptUiInfo {
    pub ui_locked: bool,
    pub cinemascope: bool,
    pub selection: Vec<u32>,
    pub last_button: i32,
}

/// A presentation message in `script_events` (protocol 33): step (1-based like the block log) and the
/// event as the JSON form of `opennml_sim::script::Presentation`, e.g. `{"Cinemascope": {"on": true}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptEventEntry {
    pub step: u64,
    pub event: serde_json::Value,
}

/// Result of `script_events` (protocol 33). The events are host state (the last 1,000 steps since the
/// last restart or load), the rest is read from the state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptEventsResult {
    /// Events of the last `steps` steps, oldest first.
    pub events: Vec<ScriptEventEntry>,
    /// Presentation state (`MissionState::presentation`, JSON form of `PresentationMemory`): light, weather, music,
    /// fog, fade, portraits, info window, marker.
    pub presentation: serde_json::Value,
    pub ui: ScriptUiInfo,
    /// Profile variables of the match as `[name, value]`, ascending by name.
    pub profile_vars: Vec<(String, i32)>,
    /// Camera of the scripts (JSON form of `ScriptCamera`, host state of the session); `null` where the host
    /// does not set it.
    #[serde(default)]
    pub camera: serde_json::Value,
}

/// Counters of the script commands of an action in `script_state` (protocol 32): how many of its commands were
/// accepted in full, accepted partially and rejected (host state since the last restart or load).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptActionCount {
    pub action: i32,
    pub accepted: u64,
    pub partial: u64,
    pub rejected: u64,
}

/// A rejected script command in `script_state` (protocol 32): step (1-based like the block log),
/// block, action, command type of the original and the reason of the command check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRejection {
    pub step: u64,
    pub container: i32,
    pub block: u32,
    pub action: i32,
    pub code: u32,
    pub reason: String,
}

/// Script commands in `script_state` (protocol 32): `actions` ascending by action number (only actions with
/// at least one command), `rejections` the latest rejections, oldest first (at most 20).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptCommandsInfo {
    pub actions: Vec<ScriptActionCount>,
    pub rejections: Vec<ScriptRejection>,
}

/// A mission objective in `script_state` (protocol 30): status (0 empty, 1 present, 3 set, 6 fulfilled,
/// 10 failed) and text ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptQuestInfo {
    pub status: i32,
    pub text: i32,
}

/// Checksum as a 16-digit hexadecimal number.
pub fn hex_checksum(value: u64) -> String {
    format!("{value:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_are_sixteen_hex_digits_and_results_use_stable_names() {
        assert_eq!(hex_checksum(0x3a), "000000000000003a");
        let result = SimHashResult {
            tick: 3,
            steps: 6,
            checksum: hex_checksum(u64::MAX),
            history: vec![(5, hex_checksum(1)), (6, hex_checksum(u64::MAX))],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["checksum"], "ffffffffffffffff");
        assert_eq!(value["history"][0][0], 5);
        let back: SimHashResult = serde_json::from_value(value).unwrap();
        assert_eq!(back, result);
    }

    /// `flags` round-trips as a row; without flags the field is missing, old rows without `flags`
    /// (and without the fields of the flags) read as `None` or with defaults.
    #[test]
    fn flags_section_round_trips_and_old_rows_stay_readable() {
        let info = SimFlagsInfo {
            flags: vec![
                SimFlagInfo {
                    id: 0x0009_4801,
                    tile: [12, 34],
                    color: 2,
                    holder: 2,
                    check_ms: 700,
                },
                SimFlagInfo {
                    id: 0x0009_4802,
                    tile: [40, 8],
                    color: 9,
                    holder: 8,
                    check_ms: 1000,
                },
            ],
            counts: [0, 0, 1, 0, 0, 0, 0, 0, 1],
            leader: 8,
            time_limit_s: 1200,
            remaining_ms: None,
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["flags"][0]["tile"], serde_json::json!([12, 34]));
        assert_eq!(value["counts"][8], 1);
        assert_eq!(value["remaining_ms"], serde_json::Value::Null);
        assert_eq!(serde_json::from_value::<SimFlagsInfo>(value).unwrap(), info);
        // A row without individual fields reads with defaults.
        let sparse: SimFlagsInfo =
            serde_json::from_value(serde_json::json!({"leader": 3})).unwrap();
        assert_eq!(sparse.leader, 3);
        assert!(sparse.flags.is_empty());
        assert_eq!(sparse.remaining_ms, None);
        let one: SimFlagInfo = serde_json::from_value(serde_json::json!({"id": 5})).unwrap();
        assert_eq!((one.id, one.tile, one.holder), (5, [0, 0], 0));
    }

    #[test]
    fn gate_and_watch_sections_round_trip() {
        // Protocol 18: gate, watch and rectangle fields are missing without content and become empty when read.
        let mut building = SimBuildingInfo {
            finished: true,
            under_construction: false,
            build_rest: 0,
            build_time: 0,
            progress: 1.0,
            render_bits: 0,
            flags: 0x2_0403,
            production: None,
            queue: Vec::new(),
            docks: [0, 0],
            meeting_point_fx: [0, 0],
            meeting_point: [0.0, 0.0],
            master: None,
            slaves: Vec::new(),
            sleeping: false,
            dead: false,
            gate: None,
            area_watch: None,
            watch_rect: None,
        };
        let value = serde_json::to_value(&building).unwrap();
        for key in ["gate", "area_watch", "watch_rect"] {
            assert!(value.get(key).is_none(), "{key} is missing without content");
        }
        assert_eq!(
            serde_json::from_value::<SimBuildingInfo>(value).unwrap(),
            building
        );
        building.gate = Some(SimGateInfo {
            open: false,
            closed: true,
            doors: vec![
                SimDoorTile {
                    x: 10,
                    z: 10,
                    blocked: true,
                },
                SimDoorTile {
                    x: 11,
                    z: 10,
                    blocked: false,
                },
            ],
        });
        building.area_watch = Some(SimAreaWatchInfo {
            key: 3,
            mode: 0xA410,
            rect: [8, 8, 20, 14],
            player: 8,
            members: vec![4096, 6144],
        });
        building.watch_rect = Some([8, 8, 20, 14]);
        let value = serde_json::to_value(&building).unwrap();
        assert_eq!(value["gate"]["closed"], true);
        assert_eq!(value["gate"]["doors"][0]["blocked"], true);
        assert_eq!(value["area_watch"]["mode"], 0xA410);
        assert_eq!(
            value["area_watch"]["members"],
            serde_json::json!([4096, 6144])
        );
        assert_eq!(value["watch_rect"], serde_json::json!([8, 8, 20, 14]));
        assert_eq!(
            serde_json::from_value::<SimBuildingInfo>(value).unwrap(),
            building
        );
        // Older responses without the new fields stay readable.
        let mut value = serde_json::to_value(&building).unwrap();
        for key in ["gate", "area_watch", "watch_rect"] {
            value.as_object_mut().unwrap().remove(key);
        }
        let old: SimBuildingInfo = serde_json::from_value(value).unwrap();
        assert_eq!(
            (old.gate, old.area_watch, old.watch_rect),
            (None, None, None)
        );
    }

    #[test]
    fn economy_sections_are_optional_and_round_trip() {
        let building = SimBuildingInfo {
            finished: false,
            under_construction: true,
            build_rest: 1500,
            build_time: 2000,
            progress: 0.25,
            render_bits: 4,
            flags: 0x103,
            production: Some(SimProductionInfo {
                make_guid: 1100,
                rest: 1000,
                total: 15000,
            }),
            queue: vec![1100],
            docks: [1, 12],
            meeting_point_fx: [0x800, 0x1800],
            meeting_point: [64.0, 192.0],
            master: None,
            slaves: Vec::new(),
            sleeping: false,
            dead: false,
            gate: None,
            area_watch: None,
            watch_rect: None,
        };
        let value = serde_json::to_value(&building).unwrap();
        assert_eq!(value["production"]["make_guid"], 1100);
        assert_eq!(value["docks"], serde_json::json!([1, 12]));
        assert_eq!(
            serde_json::from_value::<SimBuildingInfo>(value).unwrap(),
            building
        );
        // Player without counters: fields are missing from the response and become empty when read.
        let mut player = SimPlayerInfo {
            slot: 0,
            control: "human".into(),
            name: "P0".into(),
            nation: 2,
            team: 0,
            color: 0,
            resources: [1000, 500, 200, 3, 400],
            friends: 1,
            population: [3, 125],
            counts: BTreeMap::new(),
            pending: BTreeMap::new(),
            enemies: 0xFE,
            vision: 1,
            upgrades: Vec::new(),
            attack_reports: Vec::new(),
        };
        let value = serde_json::to_value(&player).unwrap();
        assert!(value.get("counts").is_none() && value.get("pending").is_none());
        assert_eq!(value["population"], serde_json::json!([3, 125]));
        player.pending.insert(1001, 1);
        let value = serde_json::to_value(&player).unwrap();
        assert_eq!(value["pending"]["1001"], 1);
        assert_eq!(
            serde_json::from_value::<SimPlayerInfo>(value).unwrap(),
            player
        );
        // Responses of protocol 8 (without the new fields) remain readable.
        let old = serde_json::json!({
            "tick": 1, "steps": 2, "mode": -1, "seed": 0, "game_speed": 14,
            "active_player": 0, "checksum": "0000000000000000", "objects": 3,
            "selection": [], "commands": 0
        });
        let section: SimSection = serde_json::from_value(old).unwrap();
        assert!(section.groups.is_empty() && section.formation == 0);
        assert!(section.notices.is_empty() && section.wood_price == 0);
        assert!(!section.fog_shown && section.fog_overlay.is_none());
        let notice = SimNotice {
            step: 40,
            id: 2,
            tile: [-1, -1],
            text: Some("Wood is missing".into()),
        };
        let value = serde_json::to_value(&notice).unwrap();
        assert_eq!(serde_json::from_value::<SimNotice>(value).unwrap(), notice);
    }

    #[test]
    fn group_info_round_trips_and_old_answers_stay_readable() {
        let group = SimGroupInfo {
            id: 3,
            kind: 2,
            type_id: 2,
            members: vec![0x0001_0001, 0x0001_0002],
            anchor: [40_960, 24_576, 368_640],
            marching: true,
            matched_speed: 1200,
            targets: vec![SimGroupTarget {
                id: 0x0001_0001,
                x: 40_960,
                z: 24_576,
                dir: 0,
            }],
            formation: Some(SimFormationInfo {
                spacing: 8192,
                depth: 2,
                layer: -1,
                rear: -1,
                rear_members: vec![],
                frames: 1,
                ready: vec![0x0001_0001],
                reported: 0,
                arrived: 1,
                end_max: u32::MAX,
                end_min: u32::MAX,
            }),
        };
        let value = serde_json::to_value(&group).unwrap();
        assert_eq!(value["formation"]["spacing"], 8192);
        assert_eq!(
            serde_json::from_value::<SimGroupInfo>(value).unwrap(),
            group
        );
        // Without `formation` (pack) and in old responses without `groups`/`formation`/`group`.
        let pack = serde_json::json!({
            "id": 0, "kind": 0, "type_id": 0, "members": [], "anchor": [-1, 0, 0],
            "marching": false, "matched_speed": 0, "targets": []
        });
        assert_eq!(
            serde_json::from_value::<SimGroupInfo>(pack)
                .unwrap()
                .formation,
            None
        );
        let old = serde_json::json!({
            "state": 1, "ende": 0, "goal_fx": null, "goal": null, "waypoints": [],
            "trajectory": 0, "speed": 0, "cruising": 0, "running": false, "layer": 0,
            "planner": 0, "wait_for": null
        });
        let walk: SimWalkInfo = serde_json::from_value(old).unwrap();
        assert_eq!((walk.group, walk.formation_num), (None, 0));
        let section = serde_json::json!({
            "tick": 1, "steps": 2, "mode": -1, "seed": 0, "game_speed": 14,
            "active_player": 0, "checksum": "0000000000000000", "objects": 3,
            "selection": [], "commands": 0, "groups": [group], "formation": 3
        });
        let section: SimSection = serde_json::from_value(section).unwrap();
        assert_eq!((section.groups.len(), section.formation), (1, 3));
    }

    #[test]
    fn combat_info_round_trips_and_is_omitted_without_combat() {
        let info = SimCombatInfo {
            hp: [150, 160],
            hp_fraction: 93_750,
            rank: 1,
            experience: 500,
            last_attacker: Some(7),
            ticks_since_hit: 3,
            render_bits: 3,
            attack: Some(SimAttackInfo {
                status: 2,
                target: Some(9),
                automatic: false,
                aggressiveness: 2,
                mode: 2,
                stubbornness: [1, 4, 0],
                search_counter: 553,
            }),
            dying: None,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["attack"]["stubbornness"], serde_json::json!([1, 4, 0]));
        assert_eq!(serde_json::from_value::<SimCombatInfo>(v).unwrap(), info);
        // Objects without combat: the field is missing from the response and becomes empty when read.
        let mut object = SimObjectInfo {
            id: 1,
            guid: 1100,
            type_name: "Colonist".into(),
            owner: 0,
            position: [0.0; 3],
            tile: [0, 0],
            direction: 0.0,
            status: 0,
            animation: -1,
            animation_starts: 0,
            target: None,
            tick_group: 0,
            name: None,
            hidden: false,
            modules: Vec::new(),
            walk: None,
            building: None,
            gatherer: None,
            construct: None,
            quarter: None,
            resource: None,
            tree: None,
            combat: None,
            ranged: None,
            animal: None,
            edible: None,
            riding: Vec::new(),
            diving: None,
            train: None,
            multipart: None,
            event_bits: 0,
            temp_upgrades: Vec::new(),
            show_counter_upgrade: 0,
            armed: None,
            spell: None,
            exec_list: Vec::new(),
            tent: None,
            transport_tent: None,
            trade: None,
            in_collision: None,
            masking: None,
            assassin: None,
            healing: None,
            message_filter: None,
            look: None,
            evoke_ghost: None,
            ghost: None,
            banish: None,
            evoke_animal: None,
            influence: None,
            bomberman: None,
            killer: None,
            special_move: None,
            headhunter: None,
            robbing: None,
            ai_marks: Vec::new(),
        };
        let value = serde_json::to_value(&object).unwrap();
        assert!(value.get("combat").is_none());
        object.combat = Some(info);
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["combat"]["hp"], serde_json::json!([150, 160]));
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        // Dying state and `sim` section (protocol 11).
        let dying = SimDyingInfo {
            status: 2,
            die_counter: 2500,
            rob_gold: 25,
            listeners: 1,
        };
        let value = serde_json::to_value(&dying).unwrap();
        assert_eq!(
            serde_json::from_value::<SimDyingInfo>(value).unwrap(),
            dying
        );
        let old = serde_json::json!({
            "tick": 1, "steps": 2, "mode": -1, "seed": 0, "game_speed": 14,
            "active_player": 0, "checksum": "0000000000000000", "objects": 3,
            "selection": [], "commands": 0
        });
        let section: SimSection = serde_json::from_value(old).unwrap();
        assert_eq!(
            (section.attack_points, section.attack_points_active),
            ([0; 8], 0)
        );
        // Ranged combat, animal, carcass and hunting (protocol 12): round trip, missing fields stay empty.
        object.ranged = Some(SimRangedInfo {
            min_dist: 4096,
            max_dist: 28_672,
            shoot: Some(SimShootInfo {
                flags: 0x0A01,
                target: Some(9),
                bullets: [1, 1],
                shoot_delay: 0,
                busy: true,
                gun_select: 0,
                is_steuerboard: false,
                cannon_counter: 0,
                can_shoot: false,
            }),
        });
        object.animal = Some(SimAnimalInfo {
            aggressive: false,
            shy: true,
            roamer: true,
            roaming: false,
            escaping: false,
            underwater: false,
            home: [10_240, 20_480, 0],
            alpha: 1.0,
        });
        object.edible = Some(SimEdibleInfo {
            meat: [50, 50],
            has_meat: true,
            rotting: false,
        });
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(
            value["ranged"]["shoot"]["bullets"],
            serde_json::json!([1, 1])
        );
        assert_eq!(
            value["animal"]["home"],
            serde_json::json!([10_240, 20_480, 0])
        );
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        let gatherer: SimGathererInfo = serde_json::from_value(serde_json::json!({
            "module": 22, "kind": "hunting", "state": 100, "load_amount": 0,
            "load_kind": -1, "source": 5, "store": null
        }))
        .unwrap();
        assert_eq!(gatherer.prey, None);
        // Old responses without projectiles (protocol 12) read as empty.
        assert!(section.projectiles.is_empty());
        assert_eq!(section.flights, 0);
    }

    #[test]
    fn riding_and_diving_sections_round_trip() {
        let riding = SimRidingInfo {
            index: 13,
            boat: false,
            mount_target: Some(65_538),
            going_to_horse: true,
            busy: false,
            next_mount_anim: -1,
            lance_available: true,
        };
        let diving = SimDivingInfo {
            status: 1,
            count: -1024,
            orig_layer: 1,
            dive_layer: 8,
            air: 250,
            height_offset: -1024,
        };
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1403, "type_name": "WID_Gewehrschuetze", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        // Without the fields (responses before protocol 14): empty.
        assert!(object.riding.is_empty() && object.diving.is_none());
        let value = serde_json::to_value(&object).unwrap();
        assert!(value.get("riding").is_none() && value.get("diving").is_none());
        // With the fields: round trip, names as in the protocol.
        object.riding = vec![riding.clone()];
        object.diving = Some(diving.clone());
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["riding"][0]["mount_target"], 65_538);
        assert_eq!(value["riding"][0]["going_to_horse"], true);
        assert_eq!(value["diving"]["height_offset"], -1024);
        assert_eq!(value["diving"]["air"], 250);
        let back = serde_json::from_value::<SimObjectInfo>(value).unwrap();
        assert_eq!(back, object);
        assert_eq!(back.riding[0], riding);
        assert_eq!(back.diving, Some(diving));
        // Without a mount: `null` like the other IDs, not the marker 4294967295.
        object.riding[0].mount_target = None;
        let value = serde_json::to_value(&object).unwrap();
        assert!(value["riding"][0]["mount_target"].is_null());
        let back = serde_json::from_value::<SimObjectInfo>(value).unwrap();
        assert_eq!(back.riding[0].mount_target, None);
    }

    #[test]
    fn vehicle_sections_round_trip() {
        let train = SimTrainInfo {
            slaves: vec![0x0003_0001, 0x0004_0001],
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
            outside: 0,
            raster_dir: [1, 0],
            last_block_tile: [36, 30],
        };
        let multipart = SimMultiPartInfo {
            head: true,
            linked: 0x0005_0001,
            prev: u32::MAX,
            next: u32::MAX,
            offset: 5300,
            centre: [40_960, 24_576, 368_640],
            last_pos: [40_960, 24_576, 0],
        };
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 2613, "type_name": "SID_Lokomitive", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        // Without the fields (responses before protocol 15): empty, and they stay out of the JSON.
        assert!(object.train.is_none() && object.multipart.is_none() && object.event_bits == 0);
        let value = serde_json::to_value(&object).unwrap();
        for key in ["train", "multipart", "event_bits"] {
            assert!(value.get(key).is_none(), "{key}");
        }
        // With the fields: round trip, names as in the protocol.
        object.train = Some(train.clone());
        object.multipart = Some(multipart.clone());
        object.event_bits = 0x101;
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(
            value["train"]["slaves"],
            serde_json::json!([196_609, 262_145])
        );
        assert_eq!(value["train"]["target"], serde_json::json!([35, 30]));
        assert_eq!(
            value["train"]["last_block_tile"],
            serde_json::json!([36, 30])
        );
        assert_eq!(
            value["multipart"]["centre"],
            serde_json::json!([40_960, 24_576, 368_640])
        );
        assert_eq!(value["event_bits"], 0x101);
        let back = serde_json::from_value::<SimObjectInfo>(value).unwrap();
        assert_eq!(back, object);
        // Without a target tile: `null`.
        object.train.as_mut().unwrap().target = None;
        let value = serde_json::to_value(&object).unwrap();
        assert!(value["train"]["target"].is_null());
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );

        // Quarters (meeting point, waiting, state) and shot module (broadside): old responses
        // without the new fields read as empty.
        let old_quarter: SimQuarterInfo = serde_json::from_value(serde_json::json!({
            "occupied": 2, "max": 12, "occupants": [7, 8]
        }))
        .unwrap();
        assert_eq!(
            (
                old_quarter.meeting_point,
                old_quarter.waiting.len(),
                old_quarter.status
            ),
            (None, 0, 0)
        );
        let quarter = SimQuarterInfo {
            meeting_point: Some([40, 41]),
            waiting: vec![9],
            status: 1,
            ..old_quarter
        };
        let value = serde_json::to_value(&quarter).unwrap();
        assert_eq!(value["meeting_point"], serde_json::json!([40, 41]));
        assert_eq!(
            serde_json::from_value::<SimQuarterInfo>(value).unwrap(),
            quarter
        );
        let old_shoot: SimShootInfo = serde_json::from_value(serde_json::json!({
            "flags": 1, "target": null, "bullets": [0, 1], "shoot_delay": 0, "busy": false,
            "gun_select": 0
        }))
        .unwrap();
        assert!(!old_shoot.is_steuerboard && !old_shoot.can_shoot && old_shoot.cannon_counter == 0);
        let shoot = SimShootInfo {
            is_steuerboard: true,
            cannon_counter: 5,
            can_shoot: true,
            ..old_shoot
        };
        let value = serde_json::to_value(&shoot).unwrap();
        assert_eq!(value["cannon_counter"], 5);
        assert_eq!(
            serde_json::from_value::<SimShootInfo>(value).unwrap(),
            shoot
        );
    }

    #[test]
    fn upgrade_sections_round_trip() {
        // Old responses (before protocol 16) without the new fields read as empty.
        let old_player: SimPlayerInfo = serde_json::from_value(serde_json::json!({
            "slot": 0, "control": "human", "name": "P0", "nation": 2, "team": 0, "color": 0,
            "resources": [1000, 500, 200, 3, 400], "friends": 1
        }))
        .unwrap();
        assert!(old_player.upgrades.is_empty());
        let value = serde_json::to_value(&old_player).unwrap();
        assert!(value.get("upgrades").is_none());
        let player = SimPlayerInfo {
            upgrades: vec![2806, 3354],
            ..old_player
        };
        let value = serde_json::to_value(&player).unwrap();
        assert_eq!(value["upgrades"], serde_json::json!([2806, 3354]));
        assert_eq!(
            serde_json::from_value::<SimPlayerInfo>(value).unwrap(),
            player
        );

        let old_result: SimQueryResult = serde_json::from_value(serde_json::json!({
            "tick": 1, "steps": 2, "total": 0, "objects": [], "players": []
        }))
        .unwrap();
        assert!(old_result.counter_upgrades.is_empty() && old_result.daytime == 0);
        let result = SimQueryResult {
            counter_upgrades: vec![SimCounterUpgrade {
                guid: 3375,
                rest_ms: 9_900,
                source: 65_537,
            }],
            daytime: 2,
            ..old_result
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["counter_upgrades"][0]["rest_ms"], 9_900);
        assert_eq!(value["daytime"], 2);
        assert_eq!(
            serde_json::from_value::<SimQueryResult>(value).unwrap(),
            result
        );

        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1112, "type_name": "SPA_Gouverneur", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        // Without the fields: empty, and they stay out of the JSON.
        assert!(object.temp_upgrades.is_empty() && object.show_counter_upgrade == 0);
        assert!(object.armed.is_none() && object.spell.is_none() && object.exec_list.is_empty());
        let value = serde_json::to_value(&object).unwrap();
        for key in [
            "temp_upgrades",
            "show_counter_upgrade",
            "armed",
            "spell",
            "exec_list",
        ] {
            assert!(value.get(key).is_none(), "{key}");
        }
        object.temp_upgrades = vec![SimTempUpgrade {
            guid: 4450,
            flags: 0x80,
            rest_ms: 1_500,
        }];
        object.show_counter_upgrade = 1;
        object.armed = Some(false);
        object.spell = Some(SimSpellInfo {
            art: 4,
            button: 544,
            cost: 50,
            anim: 0,
            idx: 2,
            reserve: 0,
            mana: 100,
            mana_max: 100,
            casting: true,
            target: None,
            corpse: None,
        });
        object.exec_list = vec![4452, 4450, 4451];
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["temp_upgrades"][0]["rest_ms"], 1_500);
        assert_eq!(value["armed"], false);
        assert_eq!(value["spell"]["mana_max"], 100);
        assert_eq!(value["exec_list"], serde_json::json!([4452, 4450, 4451]));
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
    }

    #[test]
    fn tent_and_trade_sections_round_trip() {
        // Old responses (before protocol 17) without the new fields read as empty.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1600, "type_name": "PRI_Haeuptlingszelt", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert!(object.tent.is_none() && object.transport_tent.is_none());
        assert!(object.trade.is_none() && object.in_collision.is_none());
        let value = serde_json::to_value(&object).unwrap();
        for key in ["tent", "transport_tent", "trade", "in_collision"] {
            assert!(value.get(key).is_none(), "{key}");
        }
        // With the fields: lossless round trip (`in_collision: false` stays in the JSON).
        object.tent = Some(SimTentInfo {
            status: 1,
            counter: 1_300,
            progress: 0.48,
            travois_guid: 1717,
            travois: 65_537,
            hidden: false,
        });
        object.transport_tent = Some(SimTransportTentInfo {
            tent: u32::MAX,
            transporting: false,
        });
        object.trade = Some(SimTradeInfo {
            ware: 2,
            target: 3,
            tax_level: 50,
            max_tax: 40,
            tax_interval: 60_000,
            payers: vec![1600, 1601],
            load_amount: 120,
            load_kind: 1,
        });
        object.in_collision = Some(false);
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["tent"]["counter"], 1_300);
        assert_eq!(value["transport_tent"]["tent"], u32::MAX);
        assert_eq!(value["trade"]["payers"], serde_json::json!([1600, 1601]));
        assert_eq!(value["trade"]["load_amount"], 120);
        assert_eq!(value["in_collision"], false);
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
    }

    #[test]
    fn ability_sections_round_trip() {
        // Old responses (before protocol 19) without the new fields read as empty.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1402, "type_name": "WID_Bogenschuetze", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert!(object.masking.is_none() && object.assassin.is_none());
        assert!(object.healing.is_none() && object.message_filter.is_none());
        assert!(object.look.is_none());
        let value = serde_json::to_value(&object).unwrap();
        for key in ["masking", "assassin", "healing", "message_filter", "look"] {
            assert!(value.get(key).is_none(), "{key} is missing without content");
        }
        // With the fields: lossless round trip; empty IDs stay `null`.
        object.masking = Some(SimMaskingInfo {
            is_masked: true,
            is_busy: false,
            mask_time: 1_600,
            never_unmasked: false,
            energy: 62_500,
        });
        object.assassin = Some(SimAssassinInfo {
            is_down: true,
            target: Some(65_537),
            attack_points: 250,
        });
        object.healing = Some(SimHealingInfo {
            ready: true,
            single: true,
            sick: None,
            new_target: Some(131_073),
            healing_points: 288,
            mana: 950,
        });
        object.message_filter = Some(SimMessageFilterInfo {
            mode: 0,
            codes: vec![0x41D, 0x61D],
        });
        object.look = Some(SimLookInfo {
            anim: Some(1),
            alpha: 1.0,
        });
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["masking"]["mask_time"], 1_600);
        assert_eq!(value["assassin"]["target"], 65_537);
        assert!(value["healing"]["sick"].is_null());
        assert_eq!(
            value["message_filter"]["codes"],
            serde_json::json!([1053, 1565])
        );
        assert_eq!(value["look"]["anim"], 1);
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        // A single missing field in a section (`target`, `anim`) reads as `None`.
        let assassin: SimAssassinInfo =
            serde_json::from_value(serde_json::json!({"is_down": false, "attack_points": 0}))
                .unwrap();
        assert_eq!(assassin.target, None);
        let look: SimLookInfo = serde_json::from_value(serde_json::json!({"alpha": 0.5})).unwrap();
        assert_eq!(look.anim, None);
    }

    #[test]
    fn evocation_sections_round_trip() {
        // Old responses (before protocol 20) without the new fields read as empty.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1411, "type_name": "WID_Geisterbeschwoerer", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert!(object.evoke_ghost.is_none() && object.ghost.is_none());
        assert!(object.banish.is_none() && object.evoke_animal.is_none());
        assert!(object.influence.is_none());
        let value = serde_json::to_value(&object).unwrap();
        for key in [
            "evoke_ghost",
            "ghost",
            "banish",
            "evoke_animal",
            "influence",
        ] {
            assert!(value.get(key).is_none(), "{key} is missing without content");
        }
        // With the fields: lossless round trip; empty IDs stay `null`.
        object.evoke_ghost = Some(SimEvokeGhostInfo {
            evoking: true,
            dead_body: Some(65_537),
            ghost: None,
            extra_energy: 25,
            mana: 125,
            mana_base: 125,
        });
        object.ghost = Some(SimGhostInfo {
            status: 3,
            alpha: 1.0,
            target: Some(131_073),
        });
        object.banish = Some(SimBanishInfo {
            active: true,
            effects: true,
            spell_guid: 547,
            radius: 26_624,
        });
        object.evoke_animal = Some(SimEvokeAnimalInfo {
            evoking: true,
            art: 1,
            cost: 50,
            reserve: 80,
            spawn: [20_480, 0, 22_528],
            mana: 50,
        });
        object.influence = Some(SimInfluenceInfo {
            guids: vec![4460, 4461],
            influenced: Some(vec![196_609, 262_145]),
            ring: true,
        });
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["evoke_ghost"]["dead_body"], 65_537);
        assert!(value["evoke_ghost"]["ghost"].is_null());
        assert_eq!(value["evoke_ghost"]["mana_base"], 125);
        assert_eq!(value["ghost"]["alpha"], 1.0);
        assert_eq!(value["banish"]["radius"], 26_624);
        assert_eq!(
            value["evoke_animal"]["spawn"],
            serde_json::json!([20480, 0, 22528])
        );
        assert_eq!(value["influence"]["guids"], serde_json::json!([4460, 4461]));
        assert_eq!(value["influence"]["ring"], true);
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        // A set exists only with running influences: without a set `influenced` is `null`.
        let influence: SimInfluenceInfo =
            serde_json::from_value(serde_json::json!({"guids": [4460], "ring": true})).unwrap();
        assert_eq!(influence.influenced, None);
    }

    #[test]
    fn spell_special_sections_round_trip() {
        // Old responses (before protocol 21) without the new fields read as empty.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1716, "type_name": "WID_Medizinmann", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": [],
            "spell": {"art": 1, "button": 480, "cost": 50, "anim": 0, "idx": 1, "reserve": 0,
                      "mana": 100, "mana_max": 150, "casting": true}
        }))
        .unwrap();
        assert!(object.bomberman.is_none() && object.killer.is_none());
        assert!(object.special_move.is_none());
        let old = object.spell.clone().expect("spell module");
        assert!(old.target.is_none() && old.corpse.is_none());
        let value = serde_json::to_value(&object).unwrap();
        for key in ["bomberman", "killer", "special_move"] {
            assert!(value.get(key).is_none(), "{key} is missing without content");
        }
        // Spell fields without content stay out of the JSON (round trip of old responses stays the same).
        assert!(value["spell"].get("target").is_none() && value["spell"].get("corpse").is_none());
        // With the fields: lossless round trip.
        object.spell = Some(SimSpellInfo {
            target: Some([2624.0, 0.0, 1600.0]),
            corpse: Some(65_537),
            ..old
        });
        object.bomberman = Some(SimBombermanInfo {
            target: Some(131_073),
            extra_energy: 40,
            return_point: [40_960, 0, 36_864],
            busy: true,
            mana: 60,
        });
        object.killer = Some(SimKillerInfo {
            target: None,
            extra_energy: 30,
            near_damage: 200,
            far_damage: 100,
            mana: 120,
        });
        object.special_move = Some(SimSpecialMoveInfo {
            busy: true,
            mode: 1,
            mana: 90,
        });
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(
            value["spell"]["target"],
            serde_json::json!([2624.0, 0.0, 1600.0])
        );
        assert_eq!(value["spell"]["corpse"], 65_537);
        assert_eq!(
            value["bomberman"]["return_point"],
            serde_json::json!([40960, 0, 36864])
        );
        assert!(
            value["killer"].get("target").is_none(),
            "empty target is missing"
        );
        assert_eq!(value["special_move"]["mode"], 1);
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        // Projectiles: `pulse` only for the storm; arrows carry no field.
        let arrow = SimProjectileInfo {
            id: 1,
            kind: "arrow".into(),
            fire: false,
            remaining_ms: 100,
            flight_ms: 300,
            shooter: None,
            target: None,
            target_pos: [0.0; 3],
            pulse: None,
            impact: None,
        };
        assert!(serde_json::to_value(&arrow).unwrap().get("pulse").is_none());
        for kind in ["dynamite", "explosion", "storm"] {
            let p = SimProjectileInfo {
                kind: kind.into(),
                pulse: (kind == "storm").then_some(1),
                ..arrow.clone()
            };
            let value = serde_json::to_value(&p).unwrap();
            assert_eq!(value["kind"], kind);
            assert_eq!(
                serde_json::from_value::<SimProjectileInfo>(value).unwrap(),
                p
            );
        }
        let old: SimProjectileInfo = serde_json::from_value(serde_json::json!({
            "id": 2, "kind": "ball", "fire": false, "remaining_ms": 1, "flight_ms": 2,
            "shooter": null, "target": null, "target_pos": [0.0, 0.0, 0.0]
        }))
        .unwrap();
        assert_eq!(old.pulse, None);
    }

    #[test]
    fn headhunter_sections_round_trip() {
        // Old responses (before protocol 22) without the new fields read as empty.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 4102, "type_name": "Kopfgeldjaeger_SID", "owner": 8,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert!(object.headhunter.is_none() && object.robbing.is_none());
        let value = serde_json::to_value(&object).unwrap();
        for key in ["headhunter", "robbing"] {
            assert!(value.get(key).is_none(), "{key} is missing without content");
        }
        // With the fields: lossless round trip; empty IDs are missing.
        object.headhunter = Some(SimHeadhunterInfo {
            art: 2,
            state: 1,
            target_guid: 2615,
            target_player: 1,
            target: Some(131_073),
            last_target: None,
            price: 350,
            customer: 0,
            customer_cost: 300,
        });
        object.robbing = Some(SimRobbingInfo {
            anim: 3,
            robbing: true,
            attack_enabled: false,
            mana: 140,
        });
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["headhunter"]["art"], 2);
        assert_eq!(value["headhunter"]["target"], 131_073);
        assert!(
            value["headhunter"].get("last_target").is_none(),
            "empty last target is missing"
        );
        assert_eq!(value["robbing"]["robbing"], true);
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );
        // Table of headhunters: old responses without `headhunters` read as empty.
        let old_result: SimQueryResult = serde_json::from_value(serde_json::json!({
            "tick": 1, "steps": 2, "total": 0, "objects": [], "players": []
        }))
        .unwrap();
        assert!(old_result.headhunters.is_empty());
        let result = SimQueryResult {
            headhunters: vec![
                SimHireEntry {
                    art: 0,
                    hunter: None,
                    price: 300,
                },
                SimHireEntry {
                    art: 1,
                    hunter: None,
                    price: 300,
                },
                SimHireEntry {
                    art: 2,
                    hunter: Some(196_609),
                    price: 350,
                },
            ],
            ..old_result
        };
        let value = serde_json::to_value(&result).unwrap();
        assert!(value["headhunters"][0]["hunter"].is_null());
        assert_eq!(value["headhunters"][2]["price"], 350);
        assert_eq!(
            serde_json::from_value::<SimQueryResult>(value).unwrap(),
            result
        );
        // Projectiles: `impact` only for the throw; arrows carry no field, old ones read as empty.
        let arrow = SimProjectileInfo {
            id: 1,
            kind: "arrow".into(),
            fire: false,
            remaining_ms: 100,
            flight_ms: 300,
            shooter: None,
            target: None,
            target_pos: [0.0; 3],
            pulse: None,
            impact: None,
        };
        assert!(
            serde_json::to_value(&arrow)
                .unwrap()
                .get("impact")
                .is_none()
        );
        let throw = SimProjectileInfo {
            kind: "throw".into(),
            shooter: Some(196_609),
            impact: Some(0xD22),
            ..arrow.clone()
        };
        let value = serde_json::to_value(&throw).unwrap();
        assert_eq!(
            (value["kind"].clone(), value["impact"].clone()),
            ("throw".into(), 3362.into())
        );
        assert_eq!(
            serde_json::from_value::<SimProjectileInfo>(value).unwrap(),
            throw
        );
        let old: SimProjectileInfo = serde_json::from_value(serde_json::json!({
            "id": 2, "kind": "arrow", "fire": false, "remaining_ms": 1, "flight_ms": 2,
            "shooter": null, "target": null, "target_pos": [0.0, 0.0, 0.0]
        }))
        .unwrap();
        assert_eq!(old.impact, None);
        // Hire menu: round trip, `targets` empty without a target player.
        let menu = SimHireMenuResult {
            players: vec![1],
            targets: vec![2614, 2615],
        };
        let value = serde_json::to_value(&menu).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"players": [1], "targets": [2614, 2615]})
        );
        assert_eq!(
            serde_json::from_value::<SimHireMenuResult>(value).unwrap(),
            menu
        );
    }

    #[test]
    fn ki_world_info_round_trips_and_old_answers_stay_readable() {
        // Protocol 23: old responses without `ai_marks`/`attack_reports` read as empty and stay
        // out when written.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1112, "type_name": "SPA_Gouverneur", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert!(object.ai_marks.is_empty());
        assert!(
            serde_json::to_value(&object)
                .unwrap()
                .get("ai_marks")
                .is_none()
        );
        object.ai_marks = vec![1, 4];
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(value["ai_marks"], serde_json::json!([1, 4]));
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );

        let mut player: SimPlayerInfo = serde_json::from_value(serde_json::json!({
            "slot": 1, "control": "ai", "name": "P1", "nation": 3, "team": 1, "color": 1,
            "resources": [0, 0, 0, 0, 0], "friends": 2
        }))
        .unwrap();
        assert!(player.attack_reports.is_empty());
        assert!(
            serde_json::to_value(&player)
                .unwrap()
                .get("attack_reports")
                .is_none()
        );
        player.attack_reports = vec![
            SimAttackReport {
                victim: 65_537,
                tile: [12, -3],
            },
            SimAttackReport {
                victim: 131_073,
                tile: [0, 7],
            },
        ];
        let value = serde_json::to_value(&player).unwrap();
        assert_eq!(
            value["attack_reports"],
            serde_json::json!([
                {"victim": 65_537, "tile": [12, -3]},
                {"victim": 131_073, "tile": [0, 7]}
            ])
        );
        assert_eq!(
            serde_json::from_value::<SimPlayerInfo>(value).unwrap(),
            player
        );

        // Result of `ki_query`: whole word field, direct changes (`object` is never missing for
        // marks, but is optional for future kinds).
        let result = KiQueryResult {
            code: 0x4E24,
            request: vec![0x4E24, 6, 1, 1, 10, 10, 0, 1402],
            direct: vec![
                KiDirectInfo {
                    kind: "mark".into(),
                    object: Some(65_537),
                    player: 1,
                },
                KiDirectInfo {
                    kind: "take_report".into(),
                    object: Some(131_073),
                    player: 1,
                },
            ],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["code"], 0x4E24);
        assert_eq!(value["request"][7], 1402);
        assert_eq!(value["direct"][0]["kind"], "mark");
        assert_eq!(
            serde_json::from_value::<KiQueryResult>(value).unwrap(),
            result
        );
    }

    #[test]
    fn ki_state_round_trips() {
        let thread = |id, child| KiThreadInfo {
            id,
            module: 195_235,
            name: "multi_startplayer".into(),
            pc: 2,
            status: 1,
            pause_ms: 99_946,
            wait_semaphore: -1,
            stack_depth: 3,
            stack_top: vec![1, 4, 3],
            child,
        };
        let result = KiStateResult {
            paused: true,
            engines: vec![KiEngineInfo {
                slot: 1,
                next_thread_id: 2,
                threads: vec![thread(1, Some(Box::new(thread(1, None))))],
                values: vec![(235, 3), (302, 500)],
                semaphores: vec![(7, 1)],
            }],
            rejected: vec![KiRejectCount {
                slot: 1,
                code: 0x01,
                reason: "Construction: no worker".into(),
                count: 2,
            }],
            commands: vec![KiCommandCount {
                slot: 1,
                code: 0x01,
                outcome: "rejected".into(),
                count: 2,
            }],
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value["engines"][0]["values"][1],
            serde_json::json!([302, 500])
        );
        assert_eq!(value["engines"][0]["threads"][0]["child"]["id"], 1);
        assert!(
            value["engines"][0]["threads"][0]["child"]
                .get("child")
                .is_none()
        );
        assert_eq!(value["commands"][0]["outcome"], "rejected");
        assert_eq!(
            serde_json::from_value::<KiStateResult>(value).unwrap(),
            result
        );
    }
    #[test]
    fn script_state_round_trips() {
        // Protocol 28: empty optional fields are missing, the log carries the JSON form of the message.
        let result = ScriptStateResult {
            enabled: true,
            dialog_running: false,
            containers: vec![ScriptContainerInfo {
                id: 0,
                path: "data/levels/kampagne1/kamp1-1/kamp1-1.abl".into(),
                counter: -1,
                switches: vec![("on".into(), 1)],
                timers: vec![("clock".into(), 2950)],
                variables: Vec::new(),
                random_switches: Vec::new(),
                blocks: vec![
                    ScriptBlockInfo {
                        index: 0,
                        name: "Start]".into(),
                        state: "bereit".into(),
                        wait: None,
                        cur_action: 0,
                    },
                    ScriptBlockInfo {
                        index: 2,
                        name: String::new(),
                        state: "wartet".into(),
                        wait: Some("Action 1 (n40): 950 ms remaining".into()),
                        cur_action: 1,
                    },
                ],
            }],
            conditions: None,
            speech: ScriptSpeechInfo {
                line: None,
                tips: vec![
                    ScriptTipInfo {
                        remaining_ms: 0,
                        text: 0,
                    };
                    2
                ],
                events: vec![5, 5],
                info_history: Vec::new(),
            },
            names: vec![("hero".into(), "65537".into())],
            log: vec![ScriptLogEntry {
                step: 3,
                trace: serde_json::json!({"Fired": {"container": 0, "block": 2}}),
            }],
            main_quests: Vec::new(),
            sub_quests: Vec::new(),
            countdown_container: 0,
            countdown_s: 0,
            task_status: Vec::new(),
            area_overrides: Vec::new(),
            commands: ScriptCommandsInfo::default(),
            results: Vec::new(),
            ui: ScriptUiInfo::default(),
        };
        let value = serde_json::to_value(&result).unwrap();
        assert!(value.get("conditions").is_none());
        assert!(value["speech"].get("line").is_none());
        assert!(value["containers"][0]["blocks"][0].get("wait").is_none());
        assert_eq!(
            value["containers"][0]["timers"][0],
            serde_json::json!(["clock", 2950])
        );
        assert_eq!(value["names"][0], serde_json::json!(["hero", "65537"]));
        assert_eq!(value["log"][0]["trace"]["Fired"]["block"], 2);
        assert_eq!(
            serde_json::from_value::<ScriptStateResult>(value).unwrap(),
            result
        );
        let with = ScriptStateResult {
            conditions: Some(vec![ScriptConditionInfo {
                number: 2,
                or: true,
                params: vec!["0".into(), "0".into(), "100".into()],
                value: None,
            }]),
            ..result
        };
        let value = serde_json::to_value(&with).unwrap();
        assert_eq!(value["conditions"][0]["value"], serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<ScriptStateResult>(value).unwrap(),
            with
        );
    }

    #[test]
    fn world_state_fields_round_trip_and_are_optional() {
        // Protocol 30: name and hidden state per object are missing without content; the lists of the query and the
        // mission objectives are missing in older responses and become empty when read.
        let mut object: SimObjectInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "guid": 1100, "type_name": "Colonist", "owner": 0,
            "position": [0.0, 0.0, 0.0], "tile": [0, 0], "direction": 0.0, "status": 0,
            "animation": -1, "animation_starts": 0, "target": null, "tick_group": 0,
            "modules": []
        }))
        .unwrap();
        assert_eq!((object.name.as_deref(), object.hidden), (None, false));
        let value = serde_json::to_value(&object).unwrap();
        assert!(value.get("name").is_none() && value.get("hidden").is_none());
        object.name = Some("hero".into());
        object.hidden = true;
        let value = serde_json::to_value(&object).unwrap();
        assert_eq!(
            (&value["name"], &value["hidden"]),
            (&"hero".into(), &true.into())
        );
        assert_eq!(
            serde_json::from_value::<SimObjectInfo>(value).unwrap(),
            object
        );

        let old: SimQueryResult = serde_json::from_value(serde_json::json!({
            "tick": 1, "steps": 2, "total": 0, "objects": [], "players": []
        }))
        .unwrap();
        assert!(old.kill_counts.is_empty() && old.button_locks.is_empty());
        assert!(old.slot_running.is_empty() && old.area_views.is_empty());
        let result = SimQueryResult {
            kill_counts: vec![(1, 1100, 3)],
            button_locks: vec![(1005, 1)],
            slot_running: vec![true; 11],
            area_views: vec![SimAreaView {
                name: "meadow".into(),
                rect: [4, 5, 1, 2],
                player: 2,
                clear: true,
            }],
            ..old
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["kill_counts"][0], serde_json::json!([1, 1100, 3]));
        assert_eq!(value["button_locks"][0], serde_json::json!([1005, 1]));
        assert_eq!(
            value["area_views"][0]["rect"],
            serde_json::json!([4, 5, 1, 2])
        );
        assert_eq!(
            serde_json::from_value::<SimQueryResult>(value).unwrap(),
            result
        );

        let old: ScriptStateResult = serde_json::from_value(serde_json::json!({
            "enabled": false, "dialog_running": false, "containers": [],
            "speech": {"tips": [], "events": [], "info_history": []}, "names": [], "log": []
        }))
        .unwrap();
        assert!(old.main_quests.is_empty() && old.task_status.is_empty());
        assert_eq!((old.countdown_container, old.countdown_s), (0, 0));
        let state = ScriptStateResult {
            main_quests: vec![
                ScriptQuestInfo {
                    status: 3,
                    text: 105
                };
                10
            ],
            sub_quests: vec![ScriptQuestInfo { status: 0, text: 0 }; 10],
            countdown_container: 1,
            countdown_s: 90,
            task_status: vec![(4, 1)],
            area_overrides: vec![("meadow".into(), [3, 4, 9, 9])],
            ..old
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value["main_quests"][0],
            serde_json::json!({"status": 3, "text": 105})
        );
        assert_eq!(value["task_status"][0], serde_json::json!([4, 1]));
        assert_eq!(
            value["area_overrides"][0],
            serde_json::json!(["meadow", [3, 4, 9, 9]])
        );
        assert_eq!(
            serde_json::from_value::<ScriptStateResult>(value).unwrap(),
            state
        );
    }

    #[test]
    fn script_state_commands_and_results_round_trip_and_default() {
        // Protocol 32: older responses without the fields stay readable.
        let old: ScriptStateResult = serde_json::from_value(serde_json::json!({
            "enabled": true, "dialog_running": false, "containers": [],
            "speech": {"tips": [], "events": [], "info_history": []}, "names": [], "log": []
        }))
        .unwrap();
        assert_eq!(old.commands, ScriptCommandsInfo::default());
        assert!(old.results.is_empty());
        let state = ScriptStateResult {
            commands: ScriptCommandsInfo {
                actions: vec![ScriptActionCount {
                    action: 191,
                    accepted: 3,
                    partial: 1,
                    rejected: 2,
                }],
                rejections: vec![ScriptRejection {
                    step: 7,
                    container: 1,
                    block: 4,
                    action: 191,
                    code: 0x3C,
                    reason: "Object is missing".into(),
                }],
            },
            results: vec![(0, 2), (3, 1)],
            ..old
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["commands"]["actions"][0]["rejected"], 2);
        assert_eq!(value["commands"]["rejections"][0]["code"], 0x3C);
        assert_eq!(value["results"], serde_json::json!([[0, 2], [3, 1]]));
        assert_eq!(
            serde_json::from_value::<ScriptStateResult>(value).unwrap(),
            state
        );
    }

    #[test]
    fn script_ui_and_events_round_trip_and_default() {
        // Protocol 33: older responses of `script_state` without `ui` stay readable (lock off, empty mirror).
        let old: ScriptStateResult = serde_json::from_value(serde_json::json!({
            "enabled": true, "dialog_running": false, "containers": [],
            "speech": {"tips": [], "events": [], "info_history": []}, "names": [], "log": []
        }))
        .unwrap();
        assert_eq!(old.ui, ScriptUiInfo::default());
        let ui = ScriptUiInfo {
            ui_locked: true,
            cinemascope: true,
            selection: vec![65_537, 3],
            last_button: 511,
        };
        let state = ScriptStateResult {
            ui: ui.clone(),
            ..old
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value["ui"],
            serde_json::json!({
                "ui_locked": true, "cinemascope": true, "selection": [65537, 3], "last_button": 511
            })
        );
        assert_eq!(
            serde_json::from_value::<ScriptStateResult>(value).unwrap(),
            state
        );
        // `script_events`: events with step, state and camera as JSON; without a camera `null`.
        let result = ScriptEventsResult {
            events: vec![ScriptEventEntry {
                step: 4,
                event: serde_json::json!({"Cinemascope": {"on": true}}),
            }],
            presentation: serde_json::json!({"weather": 1}),
            ui,
            profile_vars: vec![("tutorial".into(), 1)],
            camera: serde_json::Value::Null,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["events"][0]["event"]["Cinemascope"]["on"], true);
        assert_eq!(value["profile_vars"][0], serde_json::json!(["tutorial", 1]));
        assert_eq!(
            serde_json::from_value::<ScriptEventsResult>(value).unwrap(),
            result
        );
    }
}
