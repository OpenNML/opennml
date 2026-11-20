//! Mutable state of a match (`SimState`): pure data, `Clone` and
//! serializable; no `HashMap`, no time, no local player.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bstar::{PLANNER_COUNT, Planner};
use crate::collision::CollisionGrid;
use crate::collision::records::CollisionRecords;
use crate::command::Outcome;
use crate::economy::Economy;
use crate::fixed_vec::{FixedVec, at_most};
use crate::fog::Fog;
use crate::id::{MAX_SLOT_CAPACITY, ObjectId, PLAYER_SLOTS, SLOT_CAPACITY, SLOT_COUNT};
use crate::mfc::MfcMap;
use crate::object::GameObject;
use crate::occupancy::OccupancyGrid;
use crate::path::grid::PathGrid;
use crate::path::terrain::classify_all;
use crate::rng::{CrtRand, TableRandom};
use crate::rules::Rules;
use crate::upgrades::PERCENT_SLOTS;

/// Object table of a player slot (`GameWorld+0x234/+0x260/+0x28C/+0x2E4`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotTable {
    /// Loading reads at most [`MAX_SLOT_CAPACITY`] entries: an empty entry takes one byte in the savegame and the
    /// size of an object in memory ([`Self::check`] then requires the capacity of the slot).
    #[serde(deserialize_with = "at_most::<MAX_SLOT_CAPACITY, _, _>")]
    pub objects: Vec<Option<GameObject>>,
    /// Generation per index (16 bits; the ID carries only 11 bits).
    #[serde(deserialize_with = "at_most::<MAX_SLOT_CAPACITY, _, _>")]
    pub generations: Vec<u16>,
    /// Ring pointer of the allocation.
    pub ring: usize,
    /// Occupied entries.
    pub count: usize,
}

impl SlotTable {
    pub fn new(capacity: usize) -> Self {
        Self {
            objects: vec![None; capacity],
            generations: vec![0; capacity],
            ring: 0,
            count: 0,
        }
    }

    /// `GameWorld_AllocObjectIndex` (`0x00475280`): from the ring pointer on, the next free index;
    /// the pointer stays on the allocated index. Full -> `None`.
    pub fn alloc(&mut self) -> Option<usize> {
        let cap = self.objects.len();
        if self.count >= cap {
            return None;
        }
        while self.objects[self.ring].is_some() {
            self.ring += 1;
            if self.ring >= cap {
                self.ring = 0;
            }
        }
        Some(self.ring)
    }

    /// Load check: entries and generations have the capacity of the slot (the accesses via the
    /// ID read both with the same index), the ring pointer lies within it, and the counter matches the
    /// occupied entries (otherwise [`Self::alloc`] searches endlessly in a full table).
    pub fn check(&self, capacity: usize) -> Result<(), String> {
        let occupied = self.objects.iter().flatten().count();
        if self.objects.len() != capacity
            || self.generations.len() != capacity
            || self.ring >= capacity
            || self.count != occupied
        {
            return Err(format!(
                "{} entries, {} generations, ring {}, counter {} with {occupied} occupied, capacity {capacity}",
                self.objects.len(),
                self.generations.len(),
                self.ring,
                self.count
            ));
        }
        Ok(())
    }
}

/// Control kind of a player slot (`Player+0x234`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    /// 0: slot empty or closed.
    Empty,
    /// 1: human.
    Human,
    /// 2: AI (living space +20 per building, no continuation of work).
    Ai,
    /// 3: passive AI.
    Passive,
}

/// Player (`Player`, `0x364` bytes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// Slot present in the `.lvl` (`+0x218`).
    pub slot_active: bool,
    pub control: Control,
    pub name: String,
    /// Nation 0-5 (`+0x238`).
    pub nation: i32,
    /// Display color 0-7 (`+0x210`).
    pub color: i32,
    /// Team (`+0x214`).
    pub team: i32,
    /// Handicap (`+0x20C`, 0 = 100%).
    pub handicap: i32,
    /// Result (`+0x358`): 0 playing, 1 lost, 2 won, 3 not participating.
    pub result: i32,
    /// Resources (`+0x21C`) as fixed point `value << 12`: 0 gold, 1 wood, 2 food, 3 population
    /// occupied, 4 living space available.
    pub resources_fx: [i32; 5],
}

impl Player {
    /// `Player_Construct` (`0x004FAF60`) and `Player_InitSlot`: resources 30,000 (gold, wood,
    /// food), color = slot, result 3.
    pub fn new(slot: u8) -> Self {
        Self {
            slot_active: false,
            control: Control::Empty,
            name: String::new(),
            nation: 0,
            color: i32::from(slot),
            team: 0,
            handicap: 0,
            result: 3,
            resources_fx: [30000 << 12, 30000 << 12, 30000 << 12, 0, 0],
        }
    }

    /// `Player_SetResource` (`0x004FAE60`): `v << 12`, indices 0-4 only.
    pub fn set_resource(&mut self, index: usize, value: i32) {
        if let Some(r) = self.resources_fx.get_mut(index) {
            *r = value.wrapping_shl(12);
        }
    }

    /// `Player_AddResource` (`0x004FAE30`).
    pub fn add_resource(&mut self, index: usize, value: i32) {
        if let Some(r) = self.resources_fx.get_mut(index) {
            *r = r.wrapping_add(value.wrapping_shl(12));
        }
    }

    /// `Player_GetResource` (`0x004FAD30`): `>> 12` (arithmetic, negative stocks round toward
    /// -infinity), index outside 0-4 -> 0; the living space (index 4) is limited to `max_living_space`
    /// (`GameEngine+0x5EC`, [`crate::economy::Economy::max_living_space`]) - only when
    /// reading, the stock itself stays (quirk).
    pub fn resource(&self, index: usize, max_living_space: i32) -> i32 {
        let v = self.resources_fx.get(index).map_or(0, |r| r >> 12);
        if index == 4 && v >= max_living_space {
            max_living_space
        } else {
            v
        }
    }

    /// `Player_GetResourceRaw` (`0x004FA900`): fixed point, index outside 0-4 -> 0.
    pub fn resource_raw(&self, index: usize) -> i32 {
        self.resources_fx.get(index).copied().unwrap_or(0)
    }

    /// `Player_AddResourceRaw` (`0x004FAAA0`): add fixed point (overflow as on x86).
    pub fn add_resource_raw(&mut self, index: usize, value: i32) {
        if let Some(r) = self.resources_fx.get_mut(index) {
            *r = r.wrapping_add(value);
        }
    }

    /// `Player_SetResourceRaw` (`0x004FAB60`).
    pub fn set_resource_raw(&mut self, index: usize, value: i32) {
        if let Some(r) = self.resources_fx.get_mut(index) {
            *r = value;
        }
    }
}

/// Friend, enemy and vision masks (`GameWorld+0xCD0/+0xCF0/+0xD10`, re/combat-and-vision).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diplomacy {
    pub friend: [u8; PLAYER_SLOTS],
    pub enemy: [u8; PLAYER_SLOTS],
    pub view: [u8; PLAYER_SLOTS],
}

impl Default for Diplomacy {
    /// `GameWorld_InitDiplomacy` (`0x00476100`): everyone is their own friend and vision partner
    /// and enemy of all others.
    fn default() -> Self {
        let own: [u8; PLAYER_SLOTS] = std::array::from_fn(|p| 1 << p);
        Self {
            friend: own,
            enemy: own.map(|b| !b),
            view: own,
        }
    }
}

impl Diplomacy {
    fn in_range(p: i32, q: i32) -> bool {
        (0..PLAYER_SLOTS as i32).contains(&p) && (0..PLAYER_SLOTS as i32).contains(&q)
    }

    /// `GameWorld_HasFriendRelation` (`0x004761E0`).
    pub fn is_friend(&self, p: i32, q: i32) -> bool {
        Self::in_range(p, q) && self.friend[p as usize] & (1 << q) != 0
    }

    /// `GameWorld_HasEnemyRelation` (`0x00476290`): slot 8 is an enemy of all players.
    pub fn is_enemy(&self, p: i32, q: i32) -> bool {
        if p == 8 {
            return (0..PLAYER_SLOTS as i32).contains(&q);
        }
        Self::in_range(p, q) && self.enemy[p as usize] & (1 << q) != 0
    }

    /// `GameWorld_HasSharedVision` (`0x00476B60`).
    pub fn shares_vision(&self, p: i32, q: i32) -> bool {
        Self::in_range(p, q) && self.view[p as usize] & (1 << q) != 0
    }

    fn set(mask: &mut [u8; PLAYER_SLOTS], p: i32, q: i32, on: bool) {
        if Self::in_range(p, q) {
            if on {
                mask[p as usize] |= 1 << q;
            } else {
                mask[p as usize] &= !(1 << q);
            }
        }
    }

    /// `GameWorld_SetFriendRelation` (`0x00477710`).
    pub fn set_friend(&mut self, p: i32, q: i32, on: bool) {
        Self::set(&mut self.friend, p, q, on);
    }

    /// `GameWorld_SetEnemyRelation` (`0x004776B0`).
    pub fn set_enemy(&mut self, p: i32, q: i32, on: bool) {
        Self::set(&mut self.enemy, p, q, on);
    }

    /// `GameWorld_SetSharedVision` (`0x00477770`).
    pub fn set_vision(&mut self, p: i32, q: i32, on: bool) {
        Self::set(&mut self.view, p, q, on);
    }
}

/// Event of a tick for rendering and sound (not part of the state). Without `Eq`: [`SimEvent::Sound3d`]
/// carries a floating-point rendering position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimEvent {
    Created {
        id: ObjectId,
    },
    Destroyed {
        id: ObjectId,
    },
    OwnerChanged {
        old: ObjectId,
        new: ObjectId,
    },
    /// Message to a player (`MessageBar_ShowMessage` `0x00494A60`, number from `messages.ini`)
    /// at a tile. The original shows it only if the player is the local one (the caller
    /// checks that); the simulation reports it for every player, the rendering filters.
    PlayerMessage {
        /// Recipient; `None` means all (caller without a check of the local player).
        to: Option<u8>,
        id: i32,
        tile: (i32, i32),
        /// Flag word of the caller (`economy::message::flags`): priority, queuing, player.
        flags: u32,
    },
    /// Sound at an object (number of the original: `SFX_*` or sound list).
    Sound {
        object: ObjectId,
        sound: i32,
    },
    /// Credit above the head of an object (amount, resource kind 0 gold, 1 wood, 2 food).
    Credited {
        object: ObjectId,
        amount: i32,
        kind: i32,
    },
    /// A previously invisible object becomes visible (render bits 3): animation 0 immediately and
    /// dummy effects (fisher basket on first access, `PropertyResource_OnTakeResourceMsg`
    /// `0x00521250`).
    Revealed {
        object: ObjectId,
    },
    /// Effect at the position of an object (`AttachedEffect_Construct` with the path of the `.pef` file
    /// of the original, e.g. the explosion of a destroyed building); rendering only.
    Effect {
        object: ObjectId,
        path: String,
    },
    /// Command acknowledgment of a unit (`Unit_PlayResponseSound` `0x0047BBC0`) at exactly the places
    /// of the command handlers of the original: action of the sound database (12 "execute", 14 attack).
    /// The simulation reports it for every player; filter (local player, `UnitResponse`),
    /// counter and sound are done by the rendering. Owner and type GUID at the time of the call (the
    /// original checks the owner at the command, `0x0047BBF8`), so that an object removed or handed over
    /// later in the same step is still acknowledged correctly.
    UnitResponse {
        unit: ObjectId,
        action: i32,
        owner: u8,
        guid: u32,
    },
    /// `GameObject_ReportUnderAttack` (`0x0046CB40`) at the end of every `0x17F`/`0x27F`: the host shows
    /// "attack" for objects of the active player (re/combat-host 4.1).
    UnderAttack {
        object: ObjectId,
        owner: u8,
        guid: u32,
        building: bool,
        tile: (i32, i32),
    },
    /// Own effect at the object, follows it (e.g. `experience_upgrade.pef` on rank-up).
    EffectOnObject {
        object: ObjectId,
        path: String,
    },
    /// Combat points per target owner 0-7 of the elapsed double tick (`Combat_AddAttackMusicPoints`
    /// `0x00478E30`, handover at the phase change 0 -> 2; re/combat-host 5). Comes in every
    /// double tick, also with zeros (the host ends the combat music with it).
    AttackPoints {
        points: [i32; 8],
    },
    /// Object sound with the flags of the caller (`SoundManager_PlayObjectSound3D(action, flags, position,
    /// type)`, re/combat-host 3): without fog check (C-116); `pos` is the rendering position.
    Sound3d {
        object: ObjectId,
        action: i32,
        flags: u32,
        pos: [f32; 3],
        guid: u32,
    },
    /// World effect at a position (`AttachedEffect_Construct` + scaling, yaw and start time;
    /// re/combat-host 6): impacts, sparks, explosion parts. Rendering only.
    EffectAt {
        path: String,
        pos: [f32; 3],
        yaw: f32,
        scale: f32,
        start_ms: u32,
    },
    /// Effect at the object, follows it: at the helper node `node` (animated, computed by the host; e.g.
    /// muzzle flash at `gun01`) or, with an empty `node`, with offset `offset` to the object position
    /// (blood at the hit zone). Rendering only.
    EffectAttached {
        object: ObjectId,
        path: String,
        node: String,
        offset: [f32; 3],
    },
    /// Own effect of the object at a fixed position (`AttachedEffect_SetPositionAndRotation` `0x004F35E0`,
    /// then `GameObject_AddOwnedEffect` `0x0046D420`): stands at `pos`, rotated toward the point `aim`
    /// (`fpatan(aim.x − pos.x, aim.z − pos.z)`, computed by the host), drawn with the object.
    /// Muzzle flash and cannon smoke of a quartered shooter at the building
    /// (re/combat-ranged 3.4, 3.5). Rendering only.
    EffectOwnedAt {
        object: ObjectId,
        path: String,
        pos: [f32; 3],
        aim: [f32; 3],
    },
    /// A projectile flies off (re/combat-host 2.4): trajectory, model and muzzle are computed by the host.
    ProjectileLaunched {
        id: u32,
        kind: crate::combat::projectile::ProjectileKind,
        /// Fire arrow (`m_Info & 0x40`).
        fire: bool,
        shooter: ObjectId,
        shooter_guid: u32,
        /// Mesh name of the projectile model in the model of the shooter (110007), empty = none.
        model: String,
        /// Helper node of the muzzle (`schussposition01`, `gun01`, `Muendung01`); empty if `start`
        /// is already the muzzle (quartered shooter: shot position of the building,
        /// re/combat-ranged 3.2, 3.5). Then the host also draws no muzzle smoke of the
        /// projectile: it would hang on the invisible shooter.
        node: String,
        target: ObjectId,
        zone: i32,
        start: [f32; 3],
        target_pos: [f32; 3],
        flight_ms: i32,
        /// Additional yaw of the muzzle smoke (broadside +-pi/2, otherwise 0).
        yaw_extra: f32,
    },
    /// A projectile ends: `hit` = hit or impact, otherwise an arrow falls (re/combat-host 2.1).
    ProjectileEnded {
        id: u32,
        hit: bool,
        pos: [f32; 3],
    },
    /// Show/hide meshes of the object model (`PropertyShootProjectile_SetMeshesVisible`
    /// `0x00525A50` -> `ModelInstance_SetMeshVisibleByName`; re/combat-host 1.3): `names`
    /// separated by spaces (110007). Every animation start resets the mask; the host applies
    /// the event only as long as `anim_starts` is the running start.
    MeshVisibility {
        object: ObjectId,
        names: String,
        visible: bool,
        anim_starts: u32,
    },
    /// The object is removed from every selection (`GameWorld_DeselectObject` `0x004801B0`,
    /// selection list `GameWorld+0x374`): a locomotive whose grid position leaves the map
    /// (re/train-multipart 3.2). The selection belongs to the host (deviation from the original).
    Deselected {
        object: ObjectId,
    },
    /// A counter-upgrade begins (`GameWorld_CmdStartCounterUpgrade` `0x004848F0` ->
    /// `MessageBar_ShowMessage(guid, tile, slot | 0x10000)` `0x00494A60`, re/temp-upgrades 3.5):
    /// message at the tile of the source, color of the triggering slot (rendering).
    CounterUpgradeStarted {
        guid: u32,
        player: u8,
        tile: (i32, i32),
    },
    /// A counter-upgrade has expired and was revoked for all players
    /// (`0x004127F0` -> `GameWorld_RevokeUpgradeAllPlayers` `0x004812F0`, re/temp-upgrades 3.6).
    CounterUpgradeEnded {
        guid: u32,
    },
    /// A temporary upgrade was applied to the object (`GameObject_ApplyTempUpgradeRecord`
    /// `0x0046D470`, re/temp-upgrades 3.1). Sent on every application; the host shows the effect
    /// (`GameObject_AddOwnedEffect`) only with flag `EFFECT` (`0x01`), with path and
    /// height from the rules.
    TempUpgradeStarted {
        object: ObjectId,
        guid: u32,
    },
    /// A temporary upgrade has ended at the object (`0x0046FC90`, re/temp-upgrades 3.2); the host
    /// ends a shown effect.
    TempUpgradeEnded {
        object: ObjectId,
        guid: u32,
    },
    /// Selection handover at the end of a tent crossfade (re/tents 3.3/3.5): if `from` is selected,
    /// the host removes it from the selection and selects `to` - with `only_if_empty` only if the
    /// selection is empty afterward (`GameWorld_DeselectObject` `0x004801B0`, `Selection_Add`
    /// `0x00488F60`). The simulation always reports the handover; whether `from` is selected is checked by the
    /// host (the selection belongs to it, deviation from the original).
    SelectionHandover {
        from: ObjectId,
        to: ObjectId,
        only_if_empty: bool,
    },
    /// A tribute is booked (`GameWorld_CmdSendTribute` `0x00478840` ->
    /// `MessageBar_ShowMessage(0xE6, {-1, -1}, ((recipient | 0x100) << 8) | giver)`,
    /// re/heroes-resources 3.7): message 230 without a check of the local player, everyone sees it;
    /// `[PLAYERNAMEPLID1]` is the giver, `[PLAYERNAMEPLID2]` the recipient (`FUN_00560FF0`).
    TributeSent {
        giver: u8,
        recipient: u8,
    },
    /// A healer or healing building has healed `target` in this object tick (module 29,
    /// `PropertyHealing_HealArea` `0x00519830` or the single healing `0x00519C00`, re/healing 3.6/3.7);
    /// `points` is the healing amount per minute. The host shows the target effect `heilen.pef` (scale
    /// `points · 0.0055`) and keeps it as long as the message comes every object tick (the two
    /// effect maps of the original).
    Healed {
        healer: ObjectId,
        target: ObjectId,
        points: i32,
    },
    /// Red damage number above `object` (`FUN_00493030` with color `0xDC0000` at the world position of the
    /// target + 200 in height, re/special-attacks 3.6): hit `0x331` of the smith (module
    /// 49, whirlwind and hammering); `points` is the return value of `0x27F` (negative = damage).
    /// Rendering only.
    DamageNumber {
        object: ObjectId,
        points: i32,
    },
    /// Text message with number from `texte.deu` (`FUN_00492A40` to all or `FUN_00492C50` only to the
    /// local player, re/headhunter-robbing 7): `to` = `None` for all players, otherwise only for this
    /// slot (the rendering filters); `args` fill the `%s` of the template in order; `color`
    /// as `0xRRGGBB`, display duration in ms. Rendering only.
    TextMessage {
        to: Option<u8>,
        text: i32,
        args: Vec<TextArg>,
        color: u32,
        duration_ms: i32,
    },
    /// 2D sound for all players (`0x00552FB0(name, 0x14, 1, 0)`, re/headhunter-robbing 7), name from
    /// `sfx/*.ini`. Rendering only.
    Sound2d {
        name: String,
    },
    /// A command of the AI: slot `player` issued the network command `code` (`0x01…0x7E`) from
    /// a command builtin ([`crate::ki::command`]); `outcome` is the result of `execute` on
    /// the AI check path. Comes for every AI command, also accepted ones (statistics and rejections of the
    /// host). Message only, no state.
    KiCommand {
        player: u8,
        code: u32,
        outcome: Outcome,
    },
    /// Run message of the scripts: block log and statistics of the host. Only a
    /// message, no state.
    Script(crate::script::ScriptTrace),
    /// Result of a slot set (`GameEngine_SetPlayerResult` `0x00430CE0`): 1
    /// lost, 2 won. Comes like the "local player" branch of the original (end screen,
    /// campaign booking) at the end of the call, i.e. after the follow-up defeats of a victory; the
    /// rendering evaluates it for the local slot.
    PlayerResult {
        slot: u8,
        result: i32,
    },
    /// Effect of a presentation action of the scripts ([`crate::script::present`]): camera,
    /// fade, bars, lock, text, light, weather, music, fog. Message to the host only, no state
    /// (the state is held by `MissionState::presentation`).
    Present(crate::script::Presentation),
    /// A building is destroyed (`PropertyBuilding_Destroy` `0x00506300` ->
    /// `Building_SpawnDestructionSmoke` `0x0046C350`, re/buildings 8.5a): once per demolition, before sound
    /// and explosion, also for the exploding building. Smoke and crater are drawn by the host;
    /// rendering only, no state.
    BuildingDestroyed {
        object: ObjectId,
    },
    /// Crater of a cannonball (`FootprintEffect_Construct(…, 0, 0x430)` in `CannonBall_Impact`
    /// `0x004EF7C2`, re/combat-projectiles 3.4, D-283): only in the splash branch on land, at the position of
    /// the ground impact. Rendering only, no state.
    Crater {
        pos: [f32; 3],
    },
    /// Signal `0x5C` (`0x0048D4D6`): `player` is the slot field of the command (unchecked),
    /// `color` the color `GameWorld_GetPlayerColor(player)` (`0x004789E0`) at execution, `mask` the recipients
    /// (bit per slot), `tile` the location in tiles. For recipients the host plays `SFX_SIGNAL` and creates ping
    /// and action stack. Message only, no state.
    Signal {
        player: i32,
        color: i32,
        mask: u32,
        tile: (i32, i32),
    },
    /// Chat line `0x5D` without cheat (`FUN_0042E980`): `from` the sender, `mask` the recipients (bit per
    /// slot), `text` the text. The host shows "name: text" for 8000 ms in the player color if the bit of its
    /// slot is set. Message only, no state.
    Chat {
        from: u8,
        mask: u32,
        text: String,
    },
    /// Cheat `rainbowring` (`FUN_00476860(−1)`): the host toggles `m_FogShowMode` 0 <-> 7
    /// (`FogDisplay::show_mode`, deviation from the original). Message only, no state.
    FogToggle,
    /// Cheat `bloodmode` (`FUN_0055FB90`, O-392): the host toggles the blood switch of the localization
    /// (`0x006732BC`), which selects `no_blood.pef` for every `blood.pef` of arrow, rifle and sniper. Applies
    /// until the program ends, is not in the savegame. Message only, no state.
    BloodToggle,
    /// A production order of the building was canceled (`0x1103`, `0x00507960`): on every success path after
    /// lowering "in progress" `GameObject_NotifyUiIfSelected(0)` (`0x00507A29`, `0x00507A56`, `0x00507A9F`), on
    /// failure nothing. The host rebuilds the command bar silently (`{0x10, 0}`) if the building is selected
    /// and belongs to the local player (re/game-ui N.14.13). Message only, no state.
    ProductionCancelled {
        building: ObjectId,
    },
}

/// Argument of a [`SimEvent::TextMessage`]: the host does not insert the text until it is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextArg {
    /// Name of the player on this slot (`FUN_004FAB40`).
    PlayerName(u8),
    /// Display name of the type with this GUID.
    TypeName(u32),
}

/// Bucket count of the attack reports per slot (`m_nHashTableSize` = 17 of the MFC `CMap` at
/// `GameWorld+0x2340 + p·0x1C`, re/ki-queries 5.11.1).
pub const ATTACK_REPORT_BUCKETS: usize = 17;

/// Attack report (re/ki-queries 5.11): tile of the victim (`GO+0x328/+0x32C`)
/// at the last hit. The value 1000 of the original is never read and is missing (E-63).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportTile {
    pub x: i32,
    pub z: i32,
}

/// The state of a match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimState {
    /// Tick counter (`GameWorld+0x74`, 100 ms steps).
    pub tick: u32,
    /// Phase (`GameWorld+0x450`): 0 = next step begins a 100 ms tick.
    pub phase: u8,
    /// Executed engine steps of 50 ms.
    pub steps: u64,
    /// Game type (`GameEngine+0x108`): 0 ... 6 multiplayer game type.
    pub mode: i32,
    pub rng: TableRandom,
    /// CRT `rand()` of the animation choice (state).
    pub crt: CrtRand,
    pub tables: FixedVec<SlotTable, SLOT_COUNT>,
    /// Tick groups (`GameWorld+0x454/+0x470`).
    pub tick_groups: [Vec<ObjectId>; 2],
    /// Destruction list (`GameWorld+0x7D0`).
    pub destroy_queue: Vec<ObjectId>,
    /// Owner changes (`GameWorld+0x7EC/+0x808`).
    pub owner_changes: Vec<(ObjectId, u8)>,
    pub players: FixedVec<Player, PLAYER_SLOTS>,
    pub diplomacy: Diplomacy,
    /// Applied upgrades per slot 0-10 (`GameWorld+0xBB4`, variant conditions >= 1000).
    pub applied_upgrades: FixedVec<Vec<u32>, SLOT_COUNT>,
    /// Type counters per player 0-7 (`GameWorld+0x9BC`).
    pub guid_counts: Vec<BTreeMap<u32, i32>>,
    /// Flags per slot (`GameWorld+0x2208`).
    pub flag_counts: [i32; 9],
    /// Flag leader `m_FlagOwner` (`GameEngine+0x4E4`): slot 0 ... 7, 8 = nobody (default, also
    /// an animal as holder), set only in game type 2 and 3 by [`crate::flags::evaluate_flag_leader`];
    /// read by `CHECKFLAGOWNER` ([`crate::game_end`]).
    pub flag_leader: i32,
    /// Time limit `m_TimeLimit` (`GameEngine+0x4E8`, seconds): `MatchSetup::time_limit · 60`
    /// in every game type ([`crate::setup`]), read by the start of game type 4 and by the leader timer
    /// of the flag game types.
    pub time_limit: i32,
    /// Path grid with regions (`GameEngine+0x298`).
    pub path: PathGrid,
    /// Occupancy grid (`GameWorld+0x1FC0`).
    pub occupancy: OccupancyGrid,
    /// Bucket grid of the collision manager (`GameWorld+0x76C`). The original does not store it
    /// (loading restores nothing); here it belongs to the savegame so that a loaded match
    /// continues the same way.
    pub collision: CollisionGrid,
    /// B* local planners per player slot 0-7 and for all others (`GameWorld+0xD50 + 0x200·p`).
    pub planners: FixedVec<Planner, PLANNER_COUNT>,
    /// Pair tracking of the collisions (`GameWorld+0x3A4`).
    pub collision_records: CollisionRecords,
    /// `[CONFIG]` of the last placed `.wld` object (`GameEngine+0x2B4`); stays after
    /// loading and also applies to objects created later (quirk of the original).
    pub object_config: BTreeMap<u32, i32>,
    /// Living space limit, resource factors and market prices.
    pub economy: Economy,
    /// Counters "in progress" per player 0-7 (`GameWorld+0xAB8`): construction sites and queued
    /// production (`Player_AddGuidCount` `0x00481190`).
    pub pending_counts: Vec<BTreeMap<u32, i32>>,
    /// Extra tick list (`GameWorld+0x1F70`, `GameWorld_AddExtraTickObject` `0x0047E460`): objects
    /// outside the tick groups (falling trees), ticked only in the second half tick (phase 0) after
    /// both groups; without duplicates.
    pub extra_tick: Vec<ObjectId>,
    /// Fog of war (re/vision).
    pub fog: Fog,
    /// Damage and hit factors, behavior modes and combat points (re/combat).
    pub combat: crate::combat::CombatState,
    /// Building stamps for the line of sight (`GameWorld+0x41C`, re/combat-targeting 4).
    pub tile_map: crate::tile_map::BuildingTileMap,
    /// Flying projectiles (re/combat-projectiles).
    pub projectiles: crate::combat::projectile::Projectiles,
    /// Group manager and groups (re/groups-formations).
    pub groups: crate::group::GroupManager,
    /// Event bits per object (`GameWorld+0x20C4`, `GameWorld_SetObjectEventBit` `0x00481610`):
    /// bit 0 halt, 8 leaving forward, 9 leaving backward (train); a missing entry counts
    /// as 0, entries of removed objects stay (as in the original).
    ///
    /// Bits 1 idle (`0x37F`), 2 death, 3 expiry of the animation lock, 5 construction worker builds,
    /// 7 attacked are added for the scripts; the bits are not checked when loading;
    /// they are read only via masks, further bits are harmless.
    pub object_event_bits: BTreeMap<ObjectId, u32>,
    /// Kill counters per slot 0-7 (`m_BODYCounter`, `GameWorld+0x20E0`): type GUID of the
    /// victim -> count; only read by the scripts.
    pub kill_counts: [BTreeMap<u32, i32>; PLAYER_SLOTS],
    /// Build button lock of the scripts (`m_ButtonLocked`, `GameWorld+0x21E0`, global for all players): type GUID
    /// -> 0/1.
    pub button_locks: BTreeMap<u32, u8>,
    /// Does the slot 0-10 run (`m_HandleListRunning`, `GameWorld+0x62C`)? `false` = faded out. Default
    /// all `true`.
    pub slot_running: [bool; SLOT_COUNT],
    /// Individually faded-in objects per slot 0-10 (`m_TempRunningHandles`, `GameWorld+0x638`):
    /// non-empty only for a faded-out slot; dead IDs stay.
    pub inactive_lists: [Vec<ObjectId>; SLOT_COUNT],
    /// Type tables per slot 0-8 (`m_GuidUpgrades`, `GameWorld+0x4E8 + p·0x1C`, re/upgrades 1.2):
    /// type GUID -> percentages per attribute and upgrade GUID.
    pub type_upgrades: FixedVec<BTreeMap<u32, crate::upgrades::TypeUpgrades>, PERCENT_SLOTS>,
    /// World queue `m_UpgradeStack` (`GameWorld+0x1F50`, re/upgrades 3.5), FIFO.
    pub upgrade_queue: Vec<crate::upgrades::UpgradeQueueEntry>,
    /// Running counter-upgrades `m_CounterUpgrades` (`GameWorld+0x990`, re/upgrades 3.6) in
    /// insertion order.
    pub counter_upgrades: Vec<crate::upgrades::CounterUpgrade>,
    /// Time of day `m_TerrainUpgradeID` (`GameWorld+0x21C0`, re/upgrades 3.8): -1 = none, otherwise 0..3.
    pub daytime: i32,
    /// Area queries in the space grid (re/arming-gates 1.D, 5): registry with key counter,
    /// rectangles and lists. In the savegame (deviating from K-297).
    pub area_queries: crate::area::AreaQueries,
    /// Headhunters per kind 0 SPA, 1 PRI, 2 SID (`GameWorld+0x4DC` `m_HeadHunters[3]`,
    /// re/headhunter-robbing 1.4): `Init` of module 34 registers (overwrites), the destructor deregisters
    /// unconditionally. Also read for dead hunters
    /// (`GetObjectById(hid, false)`); a stale ID counts as "no hunter".
    pub headhunters: [Option<ObjectId>; 3],
    /// AI state: engines per slot, pause and AI randomness.
    pub ki: crate::ki::KiState,
    /// Attack reports per slot 0-7 (`GameWorld+0x2340 + p·0x1C`, re/ki-queries 5.11):
    /// key = ID of the victim, value = its tile at the last hit; only
    /// victims on AI slots (E-61), iteration order like MFC `CMap` (E-62). Dead victims stay
    /// (E-60). The original does not store them (deviating from E-59).
    pub attack_reports: [MfcMap<ReportTile>; PLAYER_SLOTS],
    /// Global AI difficulty 0-2 (`GameEngine+0x5F8`): copy of
    /// `MatchSetup::ki.campaign_difficulty` from [`crate::setup::new_match`], read by `0x4E39`.
    pub campaign_difficulty: i32,
    /// Object names of the scripts (`m_UnitNames`, re/scripts 6): always kept,
    /// bucket chains in MFC order, stored unchanged in the savegame (deviating from E-170).
    pub names: crate::names::ObjectNames,
    /// Script state: containers, script randomness, speech state. Without a script exactly
    /// `ScriptState::default()`.
    pub script: crate::script::ScriptState,
    /// Timer list `m_CountDownList` (`GameEngine+0x2734/0x2738`): ID ->
    /// remaining time in ms, -50 per engine tick (`GameEngine_TickTimedActions` `0x004409E0`), read by
    /// `CHECKTIMER` ([`crate::game_end`]). In the reimplementation, the entry ID 0 is created by the start of
    /// game type 4 (time limit) and by a new leader of the game types 2/3. Both have in the
    /// original kind 1 (number on the screen) and position `{0, 0x28}`; kind and position are therefore not
    /// in the state but a constant in the host (`opennml_ui::hud::countdown::TIMER_POS`), which draws every
    /// entry at the top left.
    pub countdowns: BTreeMap<i32, i32>,
    /// Statistics log of the accounting `m_Accounting` (re/accounting): eight
    /// player sets with entries `{kind, tick, value}`. The original holds it in the application
    /// (`GameApp+0xD760`); here it belongs to the match.
    pub accounting: crate::accounting::Accounting,
    /// Idle workers per slot 0-7 (`m_WaitingWorkers`, `GameWorld+0x2428 + p·0x1C`,
    /// re/game-ui D5.4): level start, message `0x87F` and 100 ms tick
    /// ([`crate::idle_workers`]).
    pub idle_workers: crate::idle_workers::IdleWorkers,
    /// Network match: from `MatchSetup::network`. In a network match, debug, AI control and foreign result commands
    /// are locked and chat cheats are off (`GameWorld_HandleChatCheat` `0x00484C30` checks
    /// `NetSession_IsActive`). It is in the state and thus in the checksum and the savegame.
    pub network: bool,
    /// Events of the running step.
    #[serde(skip)]
    pub events: Vec<SimEvent>,
}

/// Eight empty report tables with [`ATTACK_REPORT_BUCKETS`] buckets each (GameWorld ctor
/// `FUN_0048C8F0`).
pub fn empty_attack_reports() -> [MfcMap<ReportTile>; PLAYER_SLOTS] {
    std::array::from_fn(|_| MfcMap::new(ATTACK_REPORT_BUCKETS))
}

impl SimState {
    /// Empty match with game type `mode` and start index `seed` of the random sequence.
    pub fn new(mode: i32, seed: u32) -> Self {
        Self {
            tick: 0,
            phase: 0,
            steps: 0,
            mode,
            rng: TableRandom::new(seed),
            crt: CrtRand::new(1),
            tables: FixedVec::from_fn(|slot| SlotTable::new(SLOT_CAPACITY[slot])),
            tick_groups: [Vec::new(), Vec::new()],
            destroy_queue: Vec::new(),
            owner_changes: Vec::new(),
            players: FixedVec::from_fn(|slot| Player::new(slot as u8)),
            diplomacy: Diplomacy::default(),
            applied_upgrades: FixedVec::default(),
            guid_counts: vec![BTreeMap::new(); PLAYER_SLOTS],
            flag_counts: [0; 9],
            flag_leader: crate::flags::NO_LEADER,
            time_limit: crate::setup::DEFAULT_TIME_LIMIT.wrapping_mul(60),
            path: PathGrid::new(0, 0),
            occupancy: OccupancyGrid::new(0, 0),
            collision: CollisionGrid::default(),
            planners: FixedVec::default(),
            collision_records: CollisionRecords::default(),
            object_config: BTreeMap::new(),
            economy: Economy::default(),
            pending_counts: vec![BTreeMap::new(); PLAYER_SLOTS],
            extra_tick: Vec::new(),
            fog: Fog::default(),
            combat: crate::combat::CombatState::default(),
            tile_map: Default::default(),
            projectiles: Default::default(),
            groups: Default::default(),
            object_event_bits: BTreeMap::new(),
            kill_counts: Default::default(),
            button_locks: BTreeMap::new(),
            slot_running: [true; SLOT_COUNT],
            inactive_lists: Default::default(),
            type_upgrades: FixedVec::default(),
            upgrade_queue: Vec::new(),
            counter_upgrades: Vec::new(),
            daytime: -1,
            area_queries: Default::default(),
            headhunters: [None; 3],
            ki: crate::ki::KiState::new(seed),
            attack_reports: empty_attack_reports(),
            campaign_difficulty: 1,
            names: Default::default(),
            script: Default::default(),
            countdowns: BTreeMap::new(),
            accounting: Default::default(),
            idle_workers: Default::default(),
            network: false,
            events: Vec::new(),
        }
    }

    /// Grids of the map as when loading the `[TERRAIN]` block: `PathPlanner_Init`, occupancy grid
    /// empty, terrain classification of all tiles (regions not until after the objects), empty
    /// collision manager (`CollisionMgr_Init` `0x004B6490`).
    pub fn init_map(&mut self, rules: &Rules) {
        let (w, h) = (rules.map.dim_x, rules.map.dim_z);
        self.path = PathGrid::new(w, h);
        self.occupancy = OccupancyGrid::new(w, h);
        self.collision = CollisionGrid::new(w, h);
        self.fog = Fog::new(w, h);
        self.tile_map = crate::tile_map::BuildingTileMap::new(w, h);
        classify_all(&mut self.path, &mut self.occupancy, &rules.tile_classes);
    }

    /// `GameWorld_GetObjectById(id, withoutRemoved)` (`0x00475300`): slot <= 10, index in range,
    /// generation equal (11 bits of the ID against the 16-bit generation); with `without_removed` no
    /// dying objects (`0x40000`).
    pub fn object(&self, id: ObjectId, without_removed: bool) -> Option<&GameObject> {
        let table = self.tables.get(usize::from(id.slot()))?;
        if u32::from(*table.generations.get(id.index())?) != u32::from(id.generation()) {
            return None;
        }
        let object = table.objects[id.index()].as_ref()?;
        (!without_removed || object.status & crate::object::status::DIED == 0).then_some(object)
    }

    pub fn object_mut(&mut self, id: ObjectId, without_removed: bool) -> Option<&mut GameObject> {
        self.object(id, without_removed)?;
        self.tables[usize::from(id.slot())].objects[id.index()].as_mut()
    }

    /// All objects in table order (slot, then index).
    pub fn objects(&self) -> impl Iterator<Item = &GameObject> {
        self.tables.iter().flat_map(|t| t.objects.iter().flatten())
    }

    /// Number of all objects.
    pub fn object_count(&self) -> usize {
        self.tables.iter().map(|t| t.count).sum()
    }

    /// `Player_GetResource` of the slot (0-7) with the living space limit of the match; otherwise 0.
    pub fn resource(&self, slot: usize, index: usize) -> i32 {
        self.players
            .get(slot)
            .map_or(0, |p| p.resource(index, self.economy.max_living_space))
    }

    /// `GameWorld_AddPlayerGuidCount` (`0x004811E0`): slots 0-7 only.
    pub fn add_guid_count(&mut self, slot: u8, guid: u32, delta: i32) {
        if let Some(map) = self.guid_counts.get_mut(usize::from(slot)) {
            let v = map.entry(guid).or_insert(0);
            *v = v.wrapping_add(delta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diplomacy_starts_all_against_all_and_slot_8_is_everybodys_enemy() {
        let mut d = Diplomacy::default();
        assert!(d.is_friend(2, 2) && !d.is_friend(2, 3));
        assert!(d.is_enemy(2, 3) && !d.is_enemy(2, 2));
        assert!(d.is_enemy(8, 0) && !d.is_enemy(0, 8) && !d.is_enemy(8, 8));
        assert!(d.shares_vision(4, 4) && !d.shares_vision(4, 5));
        d.set_friend(0, 1, true);
        d.set_enemy(0, 1, false);
        assert!(d.is_friend(0, 1) && !d.is_friend(1, 0) && !d.is_enemy(0, 1));
        d.set_vision(9, 1, true);
        assert_eq!(d.view, Diplomacy::default().view);
    }

    #[test]
    fn slot_tables_allocate_from_the_ring_pointer() {
        let mut t = SlotTable::new(3);
        assert_eq!(t.alloc(), Some(0));
        t.objects[0] = Some(GameObject::new(ObjectId::new(0, 0, 1), 0));
        t.count = 1;
        // The pointer stood at 0 (occupied) and moves on.
        assert_eq!(t.alloc(), Some(1));
        t.objects[1] = Some(GameObject::new(ObjectId::new(1, 0, 1), 0));
        t.objects[2] = Some(GameObject::new(ObjectId::new(2, 0, 1), 0));
        t.count = 3;
        assert_eq!(t.alloc(), None);
        t.objects[0] = None;
        t.count = 2;
        // From position 1 via 2 back to 0.
        assert_eq!(t.alloc(), Some(0));
    }

    #[test]
    fn players_start_with_30000_and_book_in_fixed_point() {
        let mut p = Player::new(3);
        assert_eq!((p.resource(0, 125), p.color, p.result), (30000, 3, 3));
        p.set_resource(1, 500);
        p.add_resource(1, -20);
        assert_eq!(p.resources_fx[1], 480 << 12);
        p.add_resource(9, 1);
        assert_eq!(p.resource(9, 125), 0);
        p.set_resource_raw(2, -5);
        p.add_resource_raw(2, 2);
        assert_eq!(
            (p.resource_raw(2), p.resource(2, 125), p.resource_raw(9)),
            (-3, -1, 0)
        );
    }
}
