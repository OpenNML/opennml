//! Selection as in the original (re/commands, section "Selection"): a click selects by the
//! click ranks with priority for units (`0x0047A210`, ray test `opennml_render::pick`; trees and
//! nature never), the right click searches its own ranks (`0x00480E80`, trees too), a box all own objects
//! (`0x004891D0`), at most 30, a multi-selection keeps only units (`0x00488F60`,
//! `0x00480150`). Every addition first inserts the selection proxy (`0x00476010`): a carrier
//! with a target is replaced by its target (canoe -> occupant, horse -> rider), a mount with `SADDLE`
//! is never selectable. Shift (`KD_ADD_TO_GROUP`) adds or removes a selected object.
//! The double click (`0x0048A0C0`) maps a carrier to its occupant and selects all
//! visible own units of the same kind. Without winit and GPU (testable).

use opennml_formats::targets::MatchFilter;
use opennml_render::pick::{PickDistances, PickFlags};
use opennml_sim::fog::{FogView, MemorySnapshot, memory_matches};
use opennml_sim::id::ObjectId;
use opennml_sim::object::{GameObject, status};
use opennml_sim::rules::Rules;
use opennml_sim::state::{SimEvent, SimState};

/// At most this many objects in the selection (`GameWorld+0x37C < 0x1E`).
pub const MAX_SELECTION: usize = 30;

/// Module 0 (unit).
const UNIT: usize = 0;
/// Module 3 (building).
const BUILDING: usize = 3;
/// Module 6 (resource: gold deposit, field, fishing basket).
const RESOURCE: usize = 6;
/// Modules that put an object into the render list `GameWorld+0x88` instead of `+0xF8`: unit (0),
/// `0x33` and `0x21` (multipart objects, trains).
const UNIT_LIST_MODULES: [usize; 3] = [UNIT, 0x33, 0x21];
/// Bits that exclude any selection (`0x00466AD0`: 8, 24, 26; `0x00488F60`: 14).
const UNSELECTABLE: u32 = 0x100 | 0x0100_0000 | 0x0400_0000 | 0x4000;
/// Quartered or dead: not in the box and not in the first click rank (`0x40100`).
const HIDDEN: u32 = 0x100 | 0x40000;
/// Dead (bit 18).
const DIED: u32 = 0x40000;
/// Quartered (bit 8, `QUARTERED`).
const QUARTERED: u32 = opennml_sim::object::status::QUARTERED;

/// Modules whose occupancy the double click (`0x0048A0C0`) compares between target and candidate:
/// unit (0), 13 and 10.
const DOUBLE_CLICK_MODULES: [usize; 3] = [UNIT, 0xD, 0xA];
/// Module 0 flag `SADDLE` (mount, `MID_PROP_PTTOPT_SADDLE`).
const SADDLE: u32 = 0x8000;
/// Riding state of the double click: `ISMOUNTED | CARRIING` (`status & 0x6000`).
const RIDING_BITS: u32 = 0x6000;

/// Time between two releases of the left mouse button below which the second one is a double click
/// (`FUN_004145B0`): event 2 followed by `0x13` if the release comes less than
/// `DoubleClickTimeout` (`Engine.ini`, [`crate::game::options::Controls::double_click_ms`]) after the
/// previous one. Every release resets the time, also after a double click.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DoubleClickTimer {
    last_ms: Option<f64>,
}

impl DoubleClickTimer {
    /// Reports the release at time `now_ms` (viewer clock); `true` if it is a double click.
    pub fn release(&mut self, now_ms: f64, timeout_ms: u32) -> bool {
        let double = self
            .last_ms
            .is_some_and(|last| (0.0..f64::from(timeout_ms)).contains(&(now_ms - last)));
        self.last_ms = Some(now_ms);
        double
    }
}

/// Mouse button of a release for the double click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseSide {
    Left,
    Right,
}

/// The two double-click clocks: `FUN_004145B0` keeps a separate timestamp of the last release per
/// button (left `+0x4C`, right `+0x54`); a release of the other button therefore does not break
/// the double click.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClickClocks {
    left: DoubleClickTimer,
    right: DoubleClickTimer,
}

impl ClickClocks {
    /// Reports the release of the button `side` at time `now_ms`; `true` if it is a double click.
    pub fn release(&mut self, side: MouseSide, now_ms: f64, timeout_ms: u32) -> bool {
        match side {
            MouseSide::Left => self.left.release(now_ms, timeout_ms),
            MouseSide::Right => self.right.release(now_ms, timeout_ms),
        }
    }
}

/// Entry in the selection log ([`Selection::log`]) for the control groups of the host
/// ([`crate::game::control_groups`]): the original intervenes in the groups of the local player at two places of
/// the selection, on clearing and on adding; the host works through the log in the same sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionLog {
    /// Selection cleared (`0x0047A130` -> `FUN_004FAB00`: last group of the local player -1), also an empty one.
    Cleared,
    /// `0x00488F60` accepted the ID (the selection proxy), **before** the unit filter `0x00480150`:
    /// with `KD_ADD_TO_GROUP` held it goes to the last group (`FUN_004FB180`, O-310), even if the
    /// filter removes it again right away.
    Added(ObjectId),
}

/// The selection in the order of adding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub ids: Vec<ObjectId>,
    /// Clearing and adding since the last [`Selection::take_log`] in their sequence, on all paths (click, box,
    /// double click, bar buttons, scripts, tent handover, groups). The game host fetches it after every input, every
    /// key command and every frame (`GameSession::sync_groups`); deselecting and removal by the game
    /// are not in it (the groups stay, O-310/O-311).
    pub log: Vec<SelectionLog>,
    /// Lock of the replay (`GameWorld+0x2420`, writer `FUN_00479710` from `Game_FinishLevelSetup`):
    /// [`Selection::add`] takes nothing any more, clearing and deselecting continue.
    pub locked: bool,
}

/// An object may enter the selection (`0x00466AD0` and `0x00488F60`): inside the map, none
/// of the excluding bits, selectable (`GO+0x1A8`: nature never, destroyed and hidden buildings
/// not). The fog check (`0x00467D70`) is done by the host
/// ([`crate::game::sim_host::SimHost::fog_selectable`]).
pub fn selectable(rules: &Rules, object: &GameObject) -> bool {
    let [x, _, z] = object.world_pos;
    let width = (rules.map.dim_x * rules.map.cell[0]) as f32;
    let depth = (rules.map.dim_z * rules.map.cell[2]) as f32;
    (0.0..=width).contains(&x)
        && (0.0..=depth).contains(&z)
        && object.status & UNSELECTABLE == 0
        && object.selectable
}

/// Class `MID_PROP_GLOBAL_POLYGONCHECK` (`0x800FF`): category bit `0x80000`.
const POLYGONCHECK: u32 = 0x8_0000;
/// Class `MID_PROP_GLOBAL_ONLY_HITBOX` (`0x1000FF`): category bit `0x100000`.
const ONLY_HITBOX: u32 = 0x10_0000;

/// Check levels of the ray test (`FUN_0046CEA0`); `None` = never hittable: status bit 15, or bit 14
/// with the flag `0x8000` of the unit module (`GameObject_MatchesClass(0x8000)`). Triangles for
/// non-units and `POLYGONCHECK` (`MatchesClass(0x800FF, 0, 0)`: not dead, not
/// quartered), only `hitbox` for `ONLY_HITBOX` (`MatchesClass(0x1000FF, 0, 1)`: dead too). The
/// render-bit check (`& 0x65` or module 6) is done by the renderer (`opennml_render::scene::pick_entry`).
pub fn pick_flags(object: &GameObject) -> Option<PickFlags> {
    let st = object.status;
    if st & 0x8000 != 0 {
        return None;
    }
    let alive = st & (DIED | QUARTERED) == 0;
    if st & 0x4000 != 0
        && alive
        && object
            .module(UNIT)
            .is_some_and(|m| m.base.flags & SADDLE != 0)
    {
        return None;
    }
    Some(PickFlags {
        triangles: (alive && object.category & POLYGONCHECK != 0)
            || !object.has_module(UNIT, false),
        only_hitbox: st & QUARTERED == 0 && object.category & ONLY_HITBOX != 0,
    })
}

/// Render list of the object (`GameWorld_BuildRenderListsAndFrameUpdate` `0x0047FC70`,
/// `0x0047FDE4...0x0047FE53`): `+0x88` with module 0, `0x33` or `0x21` (also switched off,
/// `GameObject_HasModule(n, 1)`), otherwise `+0xF8` (everything static: buildings, trees, nature, resources).
pub fn in_unit_list(object: &GameObject) -> bool {
    UNIT_LIST_MODULES
        .iter()
        .any(|&m| object.has_module(m, true))
}

/// A rank of a target search: list (`true` = `+0x88`, `false` = `+0xF8`) and condition.
type Rank = (bool, fn(&GameObject) -> bool);

/// Ranks of the click target `0x0047A210` with the second argument 0 (all three callers: hover ring
/// and bar `0x0047A490`, left click `0x004498A0`, `0x00449450`).
const CLICK_RANKS: [Rank; 5] = [
    // 1. Unit, neither dead nor quartered (`status & 0x40100 == 0`).
    (true, |o| {
        o.has_module(UNIT, false) && o.status & HIDDEN == 0
    }),
    // 2. dead unit.
    (true, |o| o.has_module(UNIT, false) && o.status & DIED != 0),
    // 3. `+0x88` without (active) module 0: multipart objects, trains.
    (true, |o| !o.has_module(UNIT, false)),
    // 4. `+0xF8` with module 3: building.
    (false, |o| o.has_module(BUILDING, false)),
    // 5. `+0xF8` with module 6: resources. Trees (module 4) and nature (module 5) never.
    (false, |o| o.has_module(RESOURCE, false)),
];

/// Ranks of `Input_PickTargetAction` `0x00480E80` (mouse pointer action and right click).
const TARGET_RANKS: [Rank; 5] = [
    // 1. Unit, not dead (`status` bit 18; quartered ones count, but draw nothing).
    (true, |o| o.has_module(UNIT, false) && o.status & DIED == 0),
    // 2. dead unit.
    (true, |o| o.has_module(UNIT, false) && o.status & DIED != 0),
    // 3. `+0x88` without (active) module 0.
    (true, |o| !o.has_module(UNIT, false)),
    // 4. `+0xF8` with module 3: building.
    (false, |o| o.has_module(BUILDING, false)),
    // 5. `+0xF8` without module 3: trees, nature, resources.
    (false, |o| !o.has_module(BUILDING, false)),
];

/// Candidate of a target search: object, key for the ray test, ranks (bit `r` = rank `r`)
/// and check levels.
type Candidate<K> = (ObjectId, K, u32, PickFlags);

/// Candidates from the render list `objects` in drawing order (with a key for the
/// ray test); `+0x88` and `+0xF8` are the subsequences of it by [`in_unit_list`]. Without a rank
/// or never hittable ([`pick_flags`]): no candidate.
fn candidates<K: Copy>(
    state: &SimState,
    objects: &[(ObjectId, K)],
    ranks: &[Rank],
) -> Vec<Candidate<K>> {
    objects
        .iter()
        .filter_map(|&(id, key)| {
            let object = state.object(id, false)?;
            let unit_list = in_unit_list(object);
            let mask = ranks
                .iter()
                .enumerate()
                .filter(|(_, (list, in_rank))| *list == unit_list && in_rank(object))
                .fold(0, |m, (r, _)| m | 1 << r);
            if mask == 0 {
                return None;
            }
            Some((id, key, mask, pick_flags(object)?))
        })
        .collect()
}

/// Common rank loop of both target searches. In each rank the **last** accepted hit in list order
/// wins. `pick` is the ray test (`ObjectRenderer_PickScreenPoint`) with the
/// shared distances `d1`/`d2` (start 1e7), which the original does not reset between the ranks. `accept` decides
/// whether the hit of the rank counts: for the click target always; for the
/// right-click target only with an action, otherwise the rank falls through and the next one keeps searching with the
/// distances of this hit (`local_15` in `0x00480E80`).
fn ranked_target<K: Copy>(
    candidates: &[Candidate<K>],
    ranks: usize,
    d: &mut PickDistances,
    mut pick: impl FnMut(K, PickFlags, &mut PickDistances) -> bool,
    mut accept: impl FnMut(ObjectId) -> bool,
) -> Option<ObjectId> {
    for rank in 0..ranks {
        let mut hit = None;
        for &(id, key, mask, flags) in candidates {
            if mask & 1 << rank != 0 && pick(key, flags, d) {
                hit = Some(id);
            }
        }
        if let Some(id) = hit
            && accept(id)
        {
            return hit;
        }
    }
    None
}

/// Click target like `0x0047A210(pos, 0)`: object under the mouse for the overlay, the hover ring
/// ([`crate::game::markers::hover_marked`]) and the left-click selection. Ranks: living units, dead ones,
/// the rest of the list `+0x88`, then from `+0xF8` buildings (module 3), then resources (module 6). Trees
/// and nature are never click targets. The first rank with a hit decides.
pub fn click_target<K: Copy>(
    state: &SimState,
    objects: &[(ObjectId, K)],
    pick: impl FnMut(K, PickFlags, &mut PickDistances) -> bool,
) -> Option<ObjectId> {
    let candidates = candidates(state, objects, &CLICK_RANKS);
    let mut d = PickDistances::default();
    ranked_target(&candidates, CLICK_RANKS.len(), &mut d, pick, |_| true)
}

/// Target of a right click on an object: a game object or the active fog memory of the
/// object with this ID (list `+0x1C8`, re/vision 9; the target of the command is the
/// real ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightClickTarget {
    Object(ObjectId),
    Memory(ObjectId),
}

impl RightClickTarget {
    /// ID of the target (for the fog memory that of the remembered object).
    pub fn id(self) -> ObjectId {
        match self {
            Self::Object(id) | Self::Memory(id) => id,
        }
    }
}

/// Bits `+0x70 & 0x25` that make a fog memory hittable (`FUN_00416440`).
const MEMORY_PICK_BITS: u32 = 0x25;

/// Check levels of the ray test of a fog memory (`FUN_00416440`); `None` = never hittable:
/// remembered render bits without `0x25` or state with bit 14/15. Triangles always (the original asks
/// `MatchesClass(0x800FF)` **or** "module 0 missing" - the present flag for module 0 is never set by the
/// snapshot); only `hitbox` for `ONLY_HITBOX` via the filter copy like `0x004160C0`
/// (`0x00415AC0` with the built filter `mode` 2 = `CHECKHANDLETYPE`, `0x1000FF`, player -1,
/// fog player 0: not dead, not quartered, category bit `0x100000`).
pub fn memory_pick_flags(snap: &MemorySnapshot, view: &FogView) -> Option<PickFlags> {
    if snap.render_bits & MEMORY_PICK_BITS == 0 || snap.status & 0xC000 != 0 {
        return None;
    }
    let only_hitbox = MatchFilter {
        mode: 2,
        match_bits: 0x10_00FF,
        ..MatchFilter::constructed()
    };
    Some(PickFlags {
        triangles: true,
        only_hitbox: memory_matches(snap, view, &only_hitbox, -1, 0),
    })
}

/// Right-click target like `Input_PickTargetAction` (`0x00480E80`): for the hint "Right-click: ..."
/// and the right-click. Ranks: units (not dead), dead ones, the rest of the list `+0x88`, then from
/// `+0xF8` buildings (module 3), then all the rest (trees, nature, resources), then the active
/// fog memories `memories` (list `+0x1C8`, ray test `pick_memory` with the check levels from
/// [`memory_pick_flags`]); after that the ground (`None`).
///
/// The original determines the action for each accepted hit (`TargetActions_FindForTargetObject`
/// `0x00480600`, for fog memories `0x0047D770`); if the last accepted hit of a
/// rank has none (`has_action`), the rank falls through and the next one keeps searching with its distances.
/// A tree in front of a foreign building is thus the target (Chop wood), a tree behind it
/// lies behind `d1` (ground). The fog memories also measure against the distances of the ranks before them.
pub fn right_click_target<K: Copy>(
    state: &mut SimState,
    objects: &[(ObjectId, K)],
    memories: &[(ObjectId, PickFlags)],
    pick: impl FnMut(K, PickFlags, &mut PickDistances) -> bool,
    mut pick_memory: impl FnMut(ObjectId, PickFlags, &mut PickDistances) -> bool,
    mut has_action: impl FnMut(&mut SimState, RightClickTarget) -> bool,
) -> Option<RightClickTarget> {
    let candidates = candidates(state, objects, &TARGET_RANKS);
    let mut d = PickDistances::default();
    let object = ranked_target(&candidates, TARGET_RANKS.len(), &mut d, pick, |id| {
        has_action(state, RightClickTarget::Object(id))
    });
    if let Some(id) = object {
        return Some(RightClickTarget::Object(id));
    }
    let mut hit = None;
    for &(id, flags) in memories {
        if pick_memory(id, flags, &mut d) {
            hit = Some(RightClickTarget::Memory(id));
        }
    }
    hit.filter(|&t| has_action(state, t))
}

/// Selection proxy `FUN_00476010` (called only from `0x00488F60`): a carrier (`CARRIING`) with a
/// target is represented by its target (canoe -> occupant, horse -> rider). A mount (`SADDLE`,
/// alive and not quartered) is never selectable (`None`). A carrier without a target (orphaned compound)
/// stays itself; then status bit 14 blocks in [`selectable`]. The branches status bit 24 and module
/// `0x33` (multipart object, message `0x633`) are missing: the reimplementation sets neither.
fn selection_proxy<'a>(state: &'a SimState, object: &'a GameObject) -> Option<&'a GameObject> {
    let target = object.target;
    let proxy = (object.status & status::CARRIING != 0 && !target.is_none() && target.0 != 0)
        .then(|| state.object(target, false))
        .flatten()
        .unwrap_or(object);
    let saddle = proxy.status & (DIED | QUARTERED) == 0
        && proxy
            .module(UNIT)
            .is_some_and(|m| m.base.flags & SADDLE != 0);
    (!saddle).then_some(proxy)
}

impl Selection {
    /// Selection from `ids` (without a check, empty log).
    pub fn of(ids: Vec<ObjectId>) -> Self {
        Self {
            ids,
            log: Vec::new(),
            locked: false,
        }
    }

    /// Clear the selection (`0x0047A130`); it is in the log ([`SelectionLog::Cleared`]), even if it was already empty.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.log.push(SelectionLog::Cleared);
    }

    /// The log since the last call ([`Selection::log`]); empty afterwards.
    pub fn take_log(&mut self) -> Vec<SelectionLog> {
        std::mem::take(&mut self.log)
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.ids.contains(&id)
    }

    /// `0x00480150(0)`: keep only units.
    fn keep_units(&mut self, state: &SimState) {
        self.ids.retain(|&id| {
            state
                .object(id, false)
                .is_some_and(|o| o.has_module(UNIT, true))
        });
    }

    /// `0x00488F60`: map the object `id` through the selection proxy ([`selection_proxy`], `0x00476010`) to
    /// the selectable one and add it if it is selectable ([`selectable`] and the fog check
    /// `fog`, `0x00467D70`), not yet selected and the selection is smaller than 30; afterwards a
    /// multi-selection keeps only units. All paths into the selection pass through here: click, box,
    /// double click. It is inserted according to the hero list `heroes.ini` (`GameWorld+0x232C`,
    /// [`Rules::hero_index`], re/heroes-resources 3.1, from `0x00489056`): into an empty selection and
    /// a non-hero at the end; a hero before the first entry that is not a hero (or has no object
    /// any more) or has a larger index - if none is found, **before the last** entry
    /// (`PtrList_InsertBefore` with the last node, D-231).
    ///
    /// Nothing in the replay ([`Selection::locked`]): `0x00488F60` reads `GameWorld+0x2420` first
    /// (`0x00488F69`) and returns at once if the lock is set.
    fn add(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: &impl Fn(&GameObject) -> bool,
        id: ObjectId,
    ) {
        if self.locked {
            return;
        }
        let Some(object) = state
            .object(id, false)
            .and_then(|o| selection_proxy(state, o))
        else {
            return;
        };
        let id = object.id;
        if self.ids.len() >= MAX_SELECTION
            || self.contains(id)
            || !selectable(rules, object)
            || !fog(object)
        {
            return;
        }
        let hero = rules.hero_index(object.guid);
        if self.ids.is_empty() || hero < 0 {
            self.ids.push(id);
        } else {
            let index_of = |s: ObjectId| {
                state
                    .object(s, false)
                    .map_or(-1, |o| rules.hero_index(o.guid))
            };
            let mut pos = 0;
            loop {
                let k = index_of(self.ids[pos]);
                if k < 0 || hero < k || pos + 1 == self.ids.len() {
                    break;
                }
                pos += 1;
            }
            self.ids.insert(pos, id);
        }
        // `KD_ADD_TO_GROUP` -> `FUN_004FB180` is here in the original, before the filter (`0x004890DC...0x00489110`).
        self.log.push(SelectionLog::Added(id));
        if self.ids.len() > 1 {
            self.keep_units(state);
        }
    }

    /// Select a control group (`selectgroup` `0x004FAE90`, loop over `0x004891B0`): every ID in
    /// group order via [`Selection::add`] (`0x00488F60`: proxy, selectability, fog `fog`, not selected,
    /// at most 30, with more than one object only units). Stale IDs (`GameWorld_GetObjectById`
    /// `0x00475300`: generation, slot, index) drop out silently; foreign objects do not. Does not clear (the
    /// caller does that beforehand, `0x0047A130`).
    pub fn select_list(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        ids: &[ObjectId],
    ) {
        for &id in ids {
            self.add(state, rules, &fog, id);
        }
    }

    /// Select only this object: clear the selection (`0x0047A130`), then `0x004891B0` -> [`Selection::add`]
    /// (`0x00488F60`, with proxy, selectability and fog check `fog`). Selection button on the right, hero button and
    /// the headquarters of the headhunter button (`FUN_0048B910`).
    pub fn select_only(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        id: ObjectId,
    ) {
        self.clear();
        self.add(state, rules, &fog, id);
    }

    /// Deselect (`GameWorld_RemoveFromSelection` `0x004801B0`; selection button on the right with `KD_ADD_TO_GROUP`).
    pub fn deselect(&mut self, id: ObjectId) {
        self.ids.retain(|&s| s != id);
    }

    /// To the front of the selection (`0x0047FAB0`; selection button on the left and double): if the object is in
    /// the selection and not already at the front, it is taken out and inserted at the front. An object outside
    /// the selection is left alone by the reimplementation (the selection buttons show only selected ones; the
    /// original inserted it at the front via `PtrList_RemoveAt(NULL)`).
    pub fn to_front(&mut self, id: ObjectId) {
        if let Some(pos) = self.ids.iter().position(|&s| s == id)
            && pos > 0
        {
            self.ids.remove(pos);
            self.ids.insert(0, id);
        }
    }

    /// A selection of foreign objects ends as soon as a new selection is made (`0x004498A0`).
    fn drop_foreign(&mut self, state: &SimState, player: u8) {
        let foreign = self
            .ids
            .first()
            .and_then(|&id| state.object(id, false))
            .is_some_and(|o| o.owner() != player);
        if foreign {
            self.clear();
        }
    }

    /// Click (box under 2 pixels): without `add` new, with `add` keep only units and toggle the
    /// hit object. A foreign object replaces the selection (view only). `fog`
    /// is the fog check (`0x00467D70`, [`crate::game::sim_host::SimHost::fog_selectable`]).
    pub fn click(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        player: u8,
        target: Option<ObjectId>,
        add: bool,
    ) {
        self.drop_foreign(state, player);
        if add {
            self.keep_units(state);
        } else {
            self.clear();
        }
        let Some(id) = target else {
            return;
        };
        let Some(object) = state.object(id, false) else {
            return;
        };
        if object.owner() != player {
            self.clear();
        }
        if self.contains(id) {
            self.ids.retain(|&s| s != id);
        } else {
            self.add(state, rules, &fog, id);
        }
    }

    /// Double click on `target` (`0x0048A0C0`, event `0x13` after the click): only for a unit
    /// (module 0) of the player. A carrier (`CARRIING`) with a target is mapped to this target
    /// (canoe -> occupant, horse -> rider). Without `add` the selection is cleared, then from the
    /// visible list `visible` (list `+0x88` of the frame, in drawing order; the target is appended at
    /// the end if needed) every own,
    /// non-quartered unit with the same GUID, the same riding state (`status & 0x6000`) and
    /// the same occupancy of modules 0, 13 and 10 is added via `0x00488F60` (fog check `fog`,
    /// at most 30). Selectability is decided only by adding ([`Selection::add`] with the
    /// proxy `0x00476010`); the own mapping here supplies GUID and riding state of the
    /// occupant as the comparison, which the original also forms in `0x0048A0C0`.
    #[allow(clippy::too_many_arguments)]
    pub fn double_click(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        player: u8,
        target: Option<ObjectId>,
        add: bool,
        visible: &[ObjectId],
    ) {
        let Some(clicked) = target.and_then(|id| state.object(id, false)) else {
            return;
        };
        if !clicked.has_module(UNIT, false) || clicked.owner() != player {
            return;
        }
        let carried = (clicked.status & status::CARRIING != 0 && !clicked.target.is_none())
            .then(|| state.object(clicked.target, false))
            .flatten();
        let reference = carried.unwrap_or(clicked);
        if !add {
            self.clear();
        }
        // The mapped target is always in `+0x88` in the original; the visible list of the viewer is
        // an approximation and can lose it (large model at the window edge).
        let own = (!visible.contains(&reference.id)).then_some(reference.id);
        for id in visible.iter().copied().chain(own) {
            let Some(object) = state.object(id, false) else {
                continue;
            };
            if in_unit_list(object)
                && object.guid == reference.guid
                && object.owner() == player
                && object.status & QUARTERED == 0
                && object.status & RIDING_BITS == reference.status & RIDING_BITS
                && DOUBLE_CLICK_MODULES
                    .iter()
                    .all(|&m| object.has_module(m, false) == reference.has_module(m, false))
            {
                self.add(state, rules, &fog, id);
            }
        }
    }

    /// Box: all objects of the player (not quartered, not dead) whose pixel is
    /// `inside`, in table order; `fog` as for [`Selection::click`].
    pub fn rect(
        &mut self,
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        player: u8,
        inside: impl Fn(&GameObject) -> bool,
        add: bool,
    ) {
        self.drop_foreign(state, player);
        if !add {
            self.clear();
        }
        let Some(table) = state.tables.get(usize::from(player)) else {
            return;
        };
        for object in table.objects.iter().flatten() {
            if object.status & HIDDEN == 0 && inside(object) {
                self.add(state, rules, &fog, object.id);
            }
        }
    }

    /// Vanished objects leave the selection, as do quartered units
    /// (`GameWorld_RemoveFromSelection` `0x004801B0` from `GameObject_EnterContainer`) and dead ones:
    /// dead -> deselected (`GameWorld_DeselectObject` `0x00480940` on change of owner; the
    /// carcass is not selectable, re/combat-death 2.1). A change of owner assigns a new
    /// ID (the slot is in the ID): the object thus also drops out of the selection.
    pub fn retain_existing(&mut self, state: &SimState) {
        self.ids.retain(|&id| {
            state
                .object(id, false)
                .is_some_and(|o| o.status & (QUARTERED | DIED) == 0)
        });
    }

    /// `SimEvent::Deselected` (`GameWorld_DeselectObject` `0x004801B0`, selection list
    /// `GameWorld+0x374`): removes the object from the selection - a locomotive whose grid position
    /// leaves the map (re/train-multipart 3.2). The selection belongs to the host (deviation from the original);
    /// the other objects keep their order.
    pub fn apply_events(&mut self, events: &[SimEvent]) {
        for event in events {
            if let SimEvent::Deselected { object } = event {
                self.ids.retain(|id| id != object);
            }
        }
    }

    /// `SimEvent::SelectionHandover` at the end of a tent crossfade (re/tents 3.3, 3.5, 7.2): if
    /// `from` is selected, it leaves the selection (`GameWorld_DeselectObject` `0x004801B0`); afterwards
    /// `to` is added the ordinary way ([`Selection::add`] = `Selection_Add` `0x00488F60`:
    /// selectability, fog, at most 30, a multi-selection keeps only units) - when
    /// packing always, when pitching (`only_if_empty`) only into a selection that is empty afterwards. If
    /// `from` is not selected, nothing happens. The selection belongs to the host (deviation from the original);
    /// the events apply in their order.
    ///
    /// `Selection_Add` also checks the owner in the original; `add` does not (the paths into the
    /// selection check it themselves). Here `to` is selected only if it belongs to the active player `player`:
    /// whoever merely looks at an enemy tent does not get its travois.
    ///
    /// Returns whether at least one handover was effective (`from` selected). In the original the
    /// selection sound is then due for the selection after the handover (re/tents 3.3):
    /// `0x004801B0` reports `{8}` -> `FUN_00457980` (`0x00458510`), `Selection_Add` (`0x00488F60`)
    /// is silent, and `{0x10, 0}` (`0x0042D700(0)`) -> `FUN_00457DB0` falls back, because of `Ui+0x5CC == 0`,
    /// to `FUN_00457980` with `Selection_PlaySelectSound` (`0x0044BB60`). See
    /// [`Selection::apply_sim_step`].
    pub fn apply_handovers(
        &mut self,
        events: &[SimEvent],
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        player: u8,
    ) -> bool {
        let mut handed = false;
        for event in events {
            let SimEvent::SelectionHandover {
                from,
                to,
                only_if_empty,
            } = *event
            else {
                continue;
            };
            if !self.contains(from) {
                continue;
            }
            handed = true;
            self.ids.retain(|&id| id != from);
            let own = state.object(to, false).is_some_and(|o| o.owner() == player);
            if own && (!only_if_empty || self.ids.is_empty()) {
                self.add(state, rules, &fog, to);
            }
        }
        handed
    }

    /// The selection changes of a simulation step in the order of the host: riders take
    /// over ([`Selection::follow_riders`]), tent handovers ([`Selection::apply_handovers`]),
    /// `Deselected` ([`Selection::apply_events`]) and vanished objects ([`Selection::retain_existing`]).
    ///
    /// Returns whether the selection sound is then due for the new selection (`FUN_00457980` ->
    /// `Selection_PlaySelectSound` `0x0044BB60`, in the viewer `Voices::selection` with a type lock and
    /// the rule "exactly one object"): after every effective handover (case a
    /// travois, case b remaining unit, case c tent) and when the game otherwise
    /// shrank the selection (`{8}` or `{0x10, 2}` from `GameWorld_DeselectObject` `0x004801B0`). The original
    /// has no deselect sound of its own; the intermediate stage with an empty selection never plays
    /// anything. At most one sound per step: several effective changes in the same step
    /// play only the sound of the final selection (simplification as before when deselecting; in the original
    /// every `{8}` runs `FUN_00457980` once). Merging horse and rider does not
    /// count as shrinking (counted only after `follow_riders`).
    pub fn apply_sim_step(
        &mut self,
        events: &[SimEvent],
        state: &SimState,
        rules: &Rules,
        fog: impl Fn(&GameObject) -> bool,
        player: u8,
    ) -> bool {
        self.follow_riders(state);
        let handed = self.apply_handovers(events, state, rules, fog, player);
        let selected = self.ids.len();
        self.apply_events(events);
        self.retain_existing(state);
        handed || self.ids.len() < selected
    }

    /// Selection handover mount -> rider (re/riding 4.7, `DoMount`). Reachable in the game only for
    /// an **empty** selected canoe that takes in an occupant; a horse is never selectable
    /// ([`selection_proxy`]). If a selected object gets
    /// `CARRIING` and its target carries `ISMOUNTED | ISMOUNTEDONBOAT`, the selection passes to the target
    /// (without duplicates, the order of the selection stays). In the original `DoMount` deselects the
    /// mount and selects the rider; the simulation does not know the selection, the viewer makes the
    /// change (deviation from the original, like deselecting quartered units). The call
    /// comes before the count for the deselect sound: the handover changes the number only for duplicates.
    pub fn follow_riders(&mut self, state: &SimState) {
        const RIDER_BITS: u32 = status::ISMOUNTED | status::ISMOUNTEDONBOAT;
        let ids = std::mem::take(&mut self.ids);
        for id in ids {
            let rider = state.object(id, false).and_then(|o| {
                let target = o.target;
                (o.status & status::CARRIING != 0 && !target.is_none() && target.0 != 0)
                    .then(|| state.object(target, false))
                    .flatten()
                    .filter(|t| t.status & RIDER_BITS != 0)
                    .map(|t| t.id)
            });
            let id = rider.unwrap_or(id);
            if !self.ids.contains(&id) {
                self.ids.push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_sim::setup::{MatchSetup, new_match};
    use opennml_sim::sim::Sim;
    use opennml_sim::world::{ObjectDesc, create_object};

    const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\nGUID 4201\nNAME Busch\nVALUEID 100 6 1 0\nPROPID 5\nVALUEID 60002 6 1 0\nGUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 0\nPROPID 4\nGUID 3700\nNAME Goldvorkommen\nVALUEID 100 6 1 0\nPROPID 6\nVALUEID 70002 6 1 0\n";

    /// Player 0: two colonists, an HQ; player 1: a colonist; nature (bush), tree, gold.
    fn scene() -> (Rules, SimState, Vec<ObjectId>) {
        let rules = Rules::synthetic(TYPES, 16);
        let mut state = new_match(
            &rules,
            &MatchSetup {
                mode: -1,
                seed: 0,
                lobby: Vec::new(),
                resource_setting: 0,
                living_space: 2,
                map_explored: false,
                ki: opennml_sim::ki::KiSetup::default(),
                script: opennml_sim::script::ScriptSetup::default(),
                ..MatchSetup::default()
            },
        )
        .unwrap();
        let mut sim = Sim::new(&mut state, &rules);
        let mut ids = Vec::new();
        for (guid, owner, x) in [
            (1100, 0, 64.0),
            (1100, 0, 192.0),
            (1000, 0, 640.0),
            (1100, 1, 900.0),
            (4201, 10, 1200.0),
            (3602, 10, 1400.0),
            (3700, 10, 1600.0),
        ] {
            ids.push(
                create_object(&mut sim, &mut ObjectDesc::new(guid, owner, [x, 0.0, 64.0])).unwrap(),
            );
        }
        (rules, state, ids)
    }

    /// Ray test for hits `(object, t)` in list order: accepted like a
    /// triangle hit (only `t < d1`); objects not named are missed.
    fn triangle_hit(t: f32, _: PickFlags, d: &mut PickDistances) -> bool {
        if d.d1 <= t {
            return false;
        }
        d.d1 = t;
        d.d2 = 0.0;
        true
    }

    /// Click target (`0x0047A210`) from hits `(object, t)`.
    fn target(state: &SimState, hits: &[(ObjectId, f32)]) -> Option<ObjectId> {
        click_target(state, hits, triangle_hit)
    }

    /// Right-click target (`0x00480E80`) from hits `(object, t)`; every object has an action.
    fn right_target(state: &mut SimState, hits: &[(ObjectId, f32)]) -> Option<ObjectId> {
        right_click_target(state, hits, &[], triangle_hit, |_, _, _| false, |_, _| true)
            .map(RightClickTarget::id)
    }

    fn object_mut(state: &mut SimState, id: ObjectId) -> &mut GameObject {
        state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
            .find(|o| o.id == id)
            .unwrap()
    }

    #[test]
    fn trees_and_nature_are_never_click_targets() {
        let (_, state, ids) = scene();
        let (a, hq, bush, tree, gold) = (ids[0], ids[2], ids[4], ids[5], ids[6]);
        // `0x0047A210` checks in the list `+0xF8` only module 3 and module 6: tree and nature never.
        assert_eq!(target(&state, &[(tree, 5.0)]), None);
        assert_eq!(target(&state, &[(bush, 5.0)]), None);
        // A tree in front of the HQ does not hide it: the HQ.
        assert_eq!(target(&state, &[(tree, 0.5), (hq, 1.0)]), Some(hq));
        // Building (rank 4) before resources (rank 5), even if the resource is nearer.
        assert_eq!(target(&state, &[(gold, 3.0)]), Some(gold));
        assert_eq!(target(&state, &[(gold, 0.5), (hq, 1.0)]), Some(hq));
        // Units before everything.
        assert_eq!(target(&state, &[(gold, 0.5), (a, 9.0)]), Some(a));
    }

    #[test]
    fn right_clicks_hit_trees_after_buildings() {
        let (_, mut state, ids) = scene();
        let (a, hq, bush, tree, gold) = (ids[0], ids[2], ids[4], ids[5], ids[6]);
        // Above the roof the ray misses the triangles of the HQ: the tree behind it.
        assert_eq!(right_target(&mut state, &[(tree, 5.0)]), Some(tree));
        assert_eq!(right_target(&mut state, &[(bush, 5.0)]), Some(bush));
        assert_eq!(right_target(&mut state, &[(gold, 5.0)]), Some(gold));
        // On the roof: the HQ (rank 4), the tree behind it would be only rank 5.
        assert_eq!(
            right_target(&mut state, &[(hq, 1.0), (tree, 5.0)]),
            Some(hq)
        );
        // Ranks of the list `+0xF8`: module 3 before all others, even before a nearer tree.
        assert_eq!(
            right_target(&mut state, &[(tree, 0.5), (hq, 1.0)]),
            Some(hq)
        );
        // In the rank "without module 3" the last accepted (nearer) hit.
        assert_eq!(
            right_target(&mut state, &[(tree, 5.0), (gold, 1.0)]),
            Some(gold)
        );
        assert_eq!(
            right_target(&mut state, &[(gold, 1.0), (tree, 5.0)]),
            Some(gold)
        );
        // Units before everything.
        assert_eq!(right_target(&mut state, &[(hq, 0.5), (a, 9.0)]), Some(a));
    }

    #[test]
    fn a_rank_without_action_falls_through_with_its_distances() {
        let (_, mut state, ids) = scene();
        let (a, b, hq, tree, gold) = (ids[0], ids[1], ids[2], ids[5], ids[6]);
        let without = |state: &mut SimState, hits: &[(ObjectId, f32)], none: &[ObjectId]| {
            right_click_target(
                state,
                hits,
                &[],
                triangle_hit,
                |_, _, _| false,
                |_, t| !none.contains(&t.id()),
            )
            .map(RightClickTarget::id)
        };
        // Tree in front of the building without an action: rank 4 falls through, rank 5 takes the nearer tree.
        assert_eq!(
            without(&mut state, &[(tree, 0.5), (hq, 1.0)], &[hq]),
            Some(tree)
        );
        // Building in front of the tree: the tree lies behind `d1` -> ground.
        assert_eq!(without(&mut state, &[(hq, 0.5), (tree, 1.0)], &[hq]), None);
        // The last accepted hit of the rank counts: the nearer gold without an action lets
        // the rank fall through, even if the tree before it had one.
        assert_eq!(
            without(&mut state, &[(tree, 5.0), (gold, 1.0)], &[gold]),
            None
        );
        // Unit without an action: on in the list `+0xF8` with the distance of the unit.
        assert_eq!(without(&mut state, &[(a, 2.0), (hq, 1.0)], &[a]), Some(hq));
        assert_eq!(without(&mut state, &[(a, 1.0), (hq, 2.0)], &[a]), None);
        // With an action the first rank with a hit decides.
        assert_eq!(without(&mut state, &[(b, 9.0), (hq, 1.0)], &[]), Some(b));
    }

    /// Rank 6 of `0x00480E80`: the active fog memories (list `+0x1C8`) after all
    /// object ranks, with their distances; the last accepted hit decides
    /// (`0x0047D770`), without an action the ground remains.
    #[test]
    fn right_clicks_end_with_the_active_memories() {
        let (_, mut state, ids) = scene();
        let (hq, tree) = (ids[2], ids[5]);
        let (m1, m2) = (ObjectId::new(0, 9, 70), ObjectId::new(0, 9, 71));
        let pick = |state: &mut SimState,
                    hits: &[(ObjectId, f32)],
                    memories: &[(ObjectId, f32)],
                    none: &[ObjectId]| {
            let list: Vec<_> = memories
                .iter()
                .map(|&(id, _)| (id, PickFlags::default()))
                .collect();
            let t = |id| memories.iter().find(|m| m.0 == id).unwrap().1;
            right_click_target(
                state,
                hits,
                &list,
                triangle_hit,
                |id, flags, d| triangle_hit(t(id), flags, d),
                |_, t| !none.contains(&t.id()),
            )
        };
        use RightClickTarget::{Memory, Object};
        assert_eq!(
            pick(&mut state, &[], &[(m1, 2.0), (m2, 1.0)], &[]),
            Some(Memory(m2))
        );
        assert_eq!(
            pick(&mut state, &[], &[(m1, 2.0), (m2, 1.0)], &[m2]),
            None,
            "the last accepted one without an action"
        );
        assert_eq!(
            pick(&mut state, &[], &[(m1, 1.0), (m2, 2.0)], &[m1]),
            None,
            "m2 lies behind d1"
        );
        // An object with an action beats a nearer memory.
        assert_eq!(
            pick(&mut state, &[(tree, 5.0)], &[(m1, 1.0)], &[]),
            Some(Object(tree))
        );
        // A building without an action in front: the memory behind it lies behind `d1`, one in front hits.
        assert_eq!(pick(&mut state, &[(hq, 1.0)], &[(m1, 2.0)], &[hq]), None);
        assert_eq!(
            pick(&mut state, &[(hq, 1.0)], &[(m1, 0.5)], &[hq]),
            Some(Memory(m1))
        );
    }

    #[test]
    fn memory_pick_flags_follow_fun_00416440() {
        let (_, state, ids) = scene();
        let o = state.object(ids[2], false).unwrap().clone();
        let mut snap = MemorySnapshot::take(&o, o.tile, None);
        snap.render_bits = 0x1;
        let flags = |s: &MemorySnapshot| memory_pick_flags(s, &FogView::new(&state));
        assert_eq!(
            flags(&snap),
            Some(PickFlags {
                triangles: true,
                only_hitbox: false
            })
        );
        snap.flags[0] |= ONLY_HITBOX;
        assert!(flags(&snap).unwrap().only_hitbox);
        snap.status |= DIED;
        assert!(
            !flags(&snap).unwrap().only_hitbox,
            "filter copy: dead never"
        );
        snap.render_bits = 0x1000;
        assert_eq!(flags(&snap), None, "bits without 0x25");
        snap.render_bits = 0x4;
        snap.status = 0x4000;
        assert_eq!(flags(&snap), None, "state bit 14");
        snap.status = 0x8000;
        assert_eq!(flags(&snap), None, "state bit 15");
    }

    #[test]
    fn dead_units_and_disabled_unit_modules_keep_their_ranks() {
        let (_, mut state, ids) = scene();
        let (a, b, hq) = (ids[0], ids[1], ids[2]);
        // Dead units after living ones, before buildings - in both target searches.
        object_mut(&mut state, a).status |= DIED;
        assert_eq!(target(&state, &[(b, 9.0), (a, 1.0)]), Some(b));
        assert_eq!(target(&state, &[(hq, 0.5), (a, 1.0)]), Some(a));
        assert_eq!(right_target(&mut state, &[(b, 9.0), (a, 1.0)]), Some(b));
        assert_eq!(right_target(&mut state, &[(hq, 0.5), (a, 1.0)]), Some(a));
        // Disabled unit module: on in the list `+0x88` (rank 3), before buildings.
        object_mut(&mut state, b).modules[UNIT]
            .as_mut()
            .unwrap()
            .base
            .enabled = false;
        assert!(in_unit_list(state.object(b, false).unwrap()));
        assert_eq!(target(&state, &[(hq, 0.5), (b, 1.0)]), Some(b));
        assert_eq!(right_target(&mut state, &[(hq, 0.5), (b, 1.0)]), Some(b));
    }

    #[test]
    fn pick_flags_follow_the_object_classes() {
        let (_, mut state, ids) = scene();
        let flags = |state: &SimState, id| pick_flags(state.object(id, false).unwrap());
        let unit = PickFlags {
            triangles: false,
            only_hitbox: false,
        };
        let polygons = PickFlags {
            triangles: true,
            only_hitbox: false,
        };
        // Unit: center distance; building and nature: triangles.
        assert_eq!(flags(&state, ids[0]), Some(unit));
        assert_eq!(flags(&state, ids[2]), Some(polygons));
        assert_eq!(flags(&state, ids[4]), Some(polygons));
        // POLYGONCHECK (0x80000) only alive; ONLY_HITBOX (0x100000) dead too.
        object_mut(&mut state, ids[0]).category |= POLYGONCHECK | ONLY_HITBOX;
        assert_eq!(
            flags(&state, ids[0]),
            Some(PickFlags {
                triangles: true,
                only_hitbox: true
            })
        );
        object_mut(&mut state, ids[0]).status |= DIED;
        assert_eq!(
            flags(&state, ids[0]),
            Some(PickFlags {
                triangles: false,
                only_hitbox: true
            })
        );
        // Status bit 15: never hittable.
        object_mut(&mut state, ids[1]).status |= 0x8000;
        assert_eq!(flags(&state, ids[1]), None);
        assert_eq!(target(&state, &[(ids[1], 1.0)]), None);
    }

    #[test]
    fn clicks_prefer_units_over_nearer_buildings_and_shift_toggles() {
        let (rules, state, ids) = scene();
        let (a, b, hq) = (ids[0], ids[1], ids[2]);
        assert_eq!(target(&state, &[(hq, 1.0), (a, 5.0)]), Some(a));
        assert_eq!(target(&state, &[(hq, 1.0)]), Some(hq));
        let mut sel = Selection::default();
        sel.click(&state, &rules, |_| true, 0, Some(a), false);
        sel.click(&state, &rules, |_| true, 0, Some(b), true);
        assert_eq!(sel.ids, [a, b]);
        sel.click(&state, &rules, |_| true, 0, Some(a), true);
        assert_eq!(sel.ids, [b]);
        // A building alone is selectable, not together with units.
        sel.click(&state, &rules, |_| true, 0, Some(hq), true);
        assert_eq!(sel.ids, [b]);
        sel.click(&state, &rules, |_| true, 0, Some(hq), false);
        assert_eq!(sel.ids, [hq]);
        sel.click(&state, &rules, |_| true, 0, None, false);
        assert!(sel.ids.is_empty());
    }

    /// E-339: the lock of the replay (`+0x2420`, read only in `0x00488F60`)
    /// lets nothing into the selection on any path - click, box, double click, group; clearing continues.
    // QUIRK E-339: deviation
    #[test]
    fn a_locked_selection_takes_nothing() {
        let (rules, state, ids) = scene();
        let mut sel = Selection {
            locked: true,
            ..Selection::default()
        };
        sel.click(&state, &rules, |_| true, 0, Some(ids[0]), false);
        assert!(sel.ids.is_empty(), "click");
        sel.click(&state, &rules, |_| true, 0, Some(ids[3]), false);
        assert!(sel.ids.is_empty(), "foreign object");
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        assert!(sel.ids.is_empty(), "box");
        sel.double_click(&state, &rules, |_| true, 0, Some(ids[0]), false, &ids);
        assert!(sel.ids.is_empty(), "double click");
        sel.select_list(&state, &rules, |_| true, &ids);
        assert!(sel.ids.is_empty(), "group");
        sel.select_only(&state, &rules, |_| true, ids[1]);
        assert!(sel.ids.is_empty(), "selection button");
        assert!(
            sel.take_log()
                .iter()
                .all(|e| matches!(e, SelectionLog::Cleared)),
            "nothing added"
        );
        // Without the lock the same click selects.
        sel.locked = false;
        sel.click(&state, &rules, |_| true, 0, Some(ids[0]), false);
        assert_eq!(sel.ids, [ids[0]]);
    }

    #[test]
    fn foreign_objects_replace_the_selection_and_nature_is_never_selected() {
        let (rules, state, ids) = scene();
        let mut sel = Selection::default();
        sel.click(&state, &rules, |_| true, 0, Some(ids[0]), false);
        sel.click(&state, &rules, |_| true, 0, Some(ids[3]), true);
        assert_eq!(sel.ids, [ids[3]]);
        // After that every own selection starts anew.
        sel.click(&state, &rules, |_| true, 0, Some(ids[1]), true);
        assert_eq!(sel.ids, [ids[1]]);
        sel.click(&state, &rules, |_| true, 0, Some(ids[4]), false);
        assert!(sel.ids.is_empty());
    }

    #[test]
    fn the_dead_leave_the_selection_at_once() {
        // Dead objects (DIED) leave the selection at once (re/combat-death 2.1).
        let (_rules, mut state, ids) = scene();
        let mut sel = Selection::of(ids[..2].to_vec());
        state.object_mut(ids[0], false).unwrap().status |= DIED;
        sel.retain_existing(&state);
        assert_eq!(sel.ids, [ids[1]]);
    }

    #[test]
    fn a_selected_horse_that_gets_a_rider_hands_the_selection_to_the_rider() {
        use crate::game::sim_host::testkit::*;
        // re/riding 4.7 (deviation from the original): `DoMount` deselects the mount and selects the
        // rider; the viewer does that as soon as a selected object carries `CARRIING`.
        let mut host = host();
        let horse = spawn(&mut host, HORSE, 704.0, 704.0);
        let other = spawn(&mut host, HORSE, 900.0, 704.0);
        let rider = spawn(&mut host, RIDER, 704.0, 800.0);
        let other_rider = spawn(&mut host, RIDER, 900.0, 800.0);
        // Still without a rider: nothing changes.
        let mut sel = Selection::of(vec![horse, other]);
        sel.follow_riders(&host.state);
        assert_eq!(sel.ids, [horse, other]);
        // The selected horse gets a rider: the selection passes to the rider.
        link(&mut host, rider, horse);
        sel.follow_riders(&host.state);
        assert_eq!(sel.ids, [rider, other]);
        // A horse that is not selected, with a rider, leaves the selection unchanged.
        link(&mut host, other_rider, other);
        let mut unrelated = Selection::of(vec![rider]);
        unrelated.follow_riders(&host.state);
        assert_eq!(unrelated.ids, [rider]);
        // If the rider is already selected, no duplicate arises.
        let mut both = Selection::of(vec![rider, horse]);
        both.follow_riders(&host.state);
        assert_eq!(both.ids, [rider]);
        // `CARRIING` with a target without rider bits (no compound) stays untouched.
        host.state
            .object_mut(rider, false)
            .unwrap()
            .set_status_flags(opennml_sim::object::status::ISMOUNTED, false);
        let mut stale = Selection::of(vec![horse]);
        stale.follow_riders(&host.state);
        assert_eq!(stale.ids, [horse]);
        // Reachable in the game is only the empty canoe (a horse is never selectable): an occupant
        // boards, the selection passes to it.
        let canoe = spawn(&mut host, CANOE, 704.0, 1000.0);
        let passenger = spawn(&mut host, WARRIOR, 704.0, 1000.0);
        let mut boat = Selection::of(vec![canoe]);
        boat.follow_riders(&host.state);
        assert_eq!(boat.ids, [canoe]);
        link_boat(&mut host, passenger, canoe);
        boat.follow_riders(&host.state);
        assert_eq!(boat.ids, [passenger]);
    }

    /// Canoe with occupant, free and foreign warrior, horse with rider, free horse and two free dragoons
    /// (player 0; the foreign one belongs to player 1).
    struct Boats {
        host: crate::game::sim_host::SimHost,
        canoe: ObjectId,
        passenger: ObjectId,
        free: ObjectId,
        foreign: ObjectId,
        horse: ObjectId,
        rider: ObjectId,
        free_horse: ObjectId,
        free_riders: [ObjectId; 2],
    }

    fn boats() -> Boats {
        use crate::game::sim_host::testkit::*;
        let mut host = host();
        let canoe = spawn(&mut host, CANOE, 704.0, 704.0);
        let passenger = spawn(&mut host, WARRIOR, 704.0, 704.0);
        let free = spawn(&mut host, WARRIOR, 900.0, 704.0);
        let foreign = spawn_for(&mut host, WARRIOR, 1, 1000.0, 704.0);
        let horse = spawn(&mut host, HORSE, 704.0, 900.0);
        let rider = spawn(&mut host, RIDER, 704.0, 900.0);
        let free_horse = spawn(&mut host, HORSE, 1100.0, 700.0);
        let free_riders = [
            spawn(&mut host, RIDER, 900.0, 900.0),
            spawn(&mut host, RIDER, 1000.0, 900.0),
        ];
        link_boat(&mut host, passenger, canoe);
        link(&mut host, rider, horse);
        Boats {
            host,
            canoe,
            passenger,
            free,
            foreign,
            horse,
            rider,
            free_horse,
            free_riders,
        }
    }

    // QUIRK D-223, D-225, D-226: kept
    #[test]
    fn a_click_on_a_carrier_selects_its_rider_and_free_mounts_are_never_selected() {
        let b = boats();
        let (state, rules) = (&b.host.state, &b.host.rules);
        // Click target unchanged: canoe (the occupant is never hittable), for the horse the rider.
        assert_eq!(
            target(state, &[(b.passenger, 1.0), (b.canoe, 2.0)]),
            Some(b.canoe)
        );
        assert_eq!(
            target(state, &[(b.horse, 1.0), (b.rider, 2.0)]),
            Some(b.rider)
        );
        // `0x004498A0` -> `0x00488F60` -> `0x00476010`: canoe -> occupant.
        let mut sel = Selection::of(vec![b.free]);
        sel.click(state, rules, |_| true, 0, Some(b.canoe), false);
        assert_eq!(sel.ids, [b.passenger]);
        // Shift click on the canoe: `0x0047F180` asks the canoe, the occupant stays selected.
        sel.click(state, rules, |_| true, 0, Some(b.canoe), true);
        assert_eq!(sel.ids, [b.passenger]);
        // With Shift to another unit: the occupant is added, not duplicated.
        let mut more = Selection::of(vec![b.free]);
        more.click(state, rules, |_| true, 0, Some(b.canoe), true);
        assert_eq!(more.ids, [b.free, b.passenger]);
        // Horse with rider -> rider; free horse (`SADDLE`) -> nothing.
        let mut h = Selection::default();
        h.click(state, rules, |_| true, 0, Some(b.horse), false);
        assert_eq!(h.ids, [b.rider]);
        h.click(state, rules, |_| true, 0, Some(b.free_horse), false);
        assert!(h.ids.is_empty());
        // Even with Shift the existing selection stays, the free horse is not added.
        let mut keep = Selection::of(vec![b.free]);
        keep.click(state, rules, |_| true, 0, Some(b.free_horse), true);
        assert_eq!(keep.ids, [b.free]);
        // Box over everything: no horse, no canoe; occupant and rider once each.
        let mut r = Selection::default();
        r.rect(state, rules, |_| true, 0, |_| true, false);
        assert!(
            ![b.canoe, b.horse, b.free_horse]
                .iter()
                .any(|&i| r.contains(i))
        );
        assert_eq!(r.ids.iter().filter(|&&i| i == b.passenger).count(), 1);
        assert_eq!(r.ids.iter().filter(|&&i| i == b.rider).count(), 1);
        // Box only around the loaded canoe (`0x004891D0`): the occupant comes via the proxy.
        let mut canoe_only = Selection::default();
        canoe_only.rect(state, rules, |_| true, 0, |o| o.id == b.canoe, false);
        assert_eq!(canoe_only.ids, [b.passenger]);
        // Box only around the free horse: nothing.
        let mut horse_only = Selection::default();
        horse_only.rect(state, rules, |_| true, 0, |o| o.id == b.free_horse, false);
        assert!(horse_only.ids.is_empty());
    }

    // QUIRK D-223: kept
    #[test]
    fn a_carrier_without_a_target_is_never_selected() {
        // Orphaned compound: `0x00476010` finds no target and leaves the carrier standing; there
        // status bit 14 takes effect on the proxy (`0x00488F60`).
        let mut b = boats();
        b.host.state.object_mut(b.canoe, false).unwrap().target = ObjectId::NONE;
        let (state, rules) = (&b.host.state, &b.host.rules);
        let mut sel = Selection::default();
        sel.click(state, rules, |_| true, 0, Some(b.canoe), false);
        assert!(sel.ids.is_empty());
    }

    // QUIRK D-223: kept
    #[test]
    fn a_double_click_on_a_loaded_canoe_selects_its_passenger_and_alike() {
        let b = boats();
        let (state, rules) = (&b.host.state, &b.host.rules);
        let visible = [
            b.free,
            b.passenger,
            b.foreign,
            b.canoe,
            b.rider,
            b.free_riders[0],
        ];
        // `0x0048A0C0`: the canoe (`CARRIING`) is mapped to its occupant; selected are
        // all visible own warriors without a riding state (`status & 0x6000 == 0`), in list order.
        let mut sel = Selection::default();
        sel.double_click(state, rules, |_| true, 0, Some(b.canoe), false, &visible);
        assert_eq!(sel.ids, [b.free, b.passenger]);
        // The occupant itself as the target: the same.
        let mut again = Selection::default();
        again.double_click(
            state,
            rules,
            |_| true,
            0,
            Some(b.passenger),
            false,
            &visible,
        );
        assert_eq!(again.ids, [b.free, b.passenger]);
    }

    // QUIRK D-223: kept
    #[test]
    fn the_double_clicked_object_is_selected_even_if_the_visible_list_misses_it() {
        // The visible list is an approximation in the viewer (center point in the window): a large model
        // hit at the edge can be missing. In the original the click target is always in `+0x88`.
        let b = boats();
        let (state, rules) = (&b.host.state, &b.host.rules);
        let mut sel = Selection::default();
        sel.double_click(state, rules, |_| true, 0, Some(b.canoe), false, &[b.free]);
        assert_eq!(sel.ids, [b.free, b.passenger]);
        let mut alone = Selection::default();
        alone.double_click(state, rules, |_| true, 0, Some(b.rider), false, &[]);
        assert_eq!(alone.ids, [b.rider]);
        // If the target is already in the list, it does not occur twice.
        let mut once = Selection::default();
        once.double_click(
            state,
            rules,
            |_| true,
            0,
            Some(b.canoe),
            false,
            &[b.passenger],
        );
        assert_eq!(once.ids, [b.passenger]);
    }

    // QUIRK D-223: kept
    #[test]
    fn a_double_click_on_a_rider_selects_only_riders_in_the_same_state() {
        let b = boats();
        let (state, rules) = (&b.host.state, &b.host.rules);
        let visible = [
            b.free_riders[0],
            b.rider,
            b.horse,
            b.passenger,
            b.free_riders[1],
        ];
        // For the rider (`0x2000`) neither a free dragoon (status 0) nor the horse
        // (`CARRIING`, other GUID) is added.
        let mut sel = Selection::default();
        sel.double_click(state, rules, |_| true, 0, Some(b.rider), false, &visible);
        assert_eq!(sel.ids, [b.rider]);
        // The horse as the target is mapped to the rider.
        let mut from_horse = Selection::default();
        from_horse.double_click(state, rules, |_| true, 0, Some(b.horse), false, &visible);
        assert_eq!(from_horse.ids, [b.rider]);
        // A free dragoon fetches the other free ones, not the rider.
        let mut free = Selection::default();
        free.double_click(
            state,
            rules,
            |_| true,
            0,
            Some(b.free_riders[0]),
            false,
            &visible,
        );
        assert_eq!(free.ids, [b.free_riders[0], b.free_riders[1]]);
    }

    // QUIRK D-223: kept
    #[test]
    fn a_double_click_selects_all_visible_own_units_of_the_same_kind() {
        let (rules, mut state, ids) = scene();
        let (a, b, hq, foreign) = (ids[0], ids[1], ids[2], ids[3]);
        let all = [a, b, hq, foreign];
        // All visible own colonists, not the HQ and not the foreign colonist.
        let mut sel = Selection::default();
        sel.double_click(&state, &rules, |_| true, 0, Some(a), false, &all);
        assert_eq!(sel.ids, [a, b]);
        // What is not in the visible list is not added.
        let mut seen = Selection::default();
        seen.double_click(&state, &rules, |_| true, 0, Some(a), false, &[a, hq]);
        assert_eq!(seen.ids, [a]);
        // Without Shift it replaces the selection, with Shift a unit selection stays.
        let mut replaced = Selection::of(vec![hq]);
        replaced.double_click(&state, &rules, |_| true, 0, Some(a), false, &all);
        assert_eq!(replaced.ids, [a, b]);
        let mut kept = Selection::of(vec![b]);
        kept.double_click(&state, &rules, |_| true, 0, Some(a), true, &all);
        assert_eq!(kept.ids, [b, a]);
        // Fog (`0x00467D70`) and quartered units apply as for every addition.
        let mut fogged = Selection::default();
        fogged.double_click(&state, &rules, |o| o.id != b, 0, Some(a), false, &all);
        assert_eq!(fogged.ids, [a]);
        object_mut(&mut state, b).status |= QUARTERED;
        let mut quartered = Selection::default();
        quartered.double_click(&state, &rules, |_| true, 0, Some(a), false, &all);
        assert_eq!(quartered.ids, [a]);
        // Foreign target, no target, target without a unit module (HQ): nothing changes.
        let mut untouched = Selection::of(vec![a]);
        for target in [Some(foreign), None, Some(hq)] {
            untouched.double_click(&state, &rules, |_| true, 0, target, false, &all);
            assert_eq!(untouched.ids, [a]);
        }
    }

    // QUIRK D-223: kept
    #[test]
    fn two_releases_count_as_a_double_click_only_below_the_timeout() {
        let mut timer = DoubleClickTimer::default();
        // First release: never a double click.
        assert!(!timer.release(1_000.0, 200));
        // 199 ms later: yes (`<` as in `0x004145B0`).
        assert!(timer.release(1_199.0, 200));
        // Every release resets the time, also after a double click: a third
        // quick click is one again.
        assert!(timer.release(1_300.0, 200));
        // Exactly 200 ms or more: two single clicks.
        assert!(!timer.release(1_500.0, 200));
        assert!(!timer.release(3_000.0, 200));
        // A jump back of the clock is not a double click.
        assert!(!timer.release(100.0, 200));
    }

    // QUIRK D-227: kept
    #[test]
    fn left_and_right_double_clicks_have_separate_clocks() {
        // Separate timestamps per button (`+0x4C` left, `+0x54` right in `FUN_004145B0`).
        let mut clocks = ClickClocks::default();
        assert!(!clocks.release(MouseSide::Right, 1_000.0, 200));
        // A release on the left in between does not break the right double click and does not count itself.
        assert!(!clocks.release(MouseSide::Left, 1_050.0, 200));
        assert!(clocks.release(MouseSide::Right, 1_100.0, 200));
        // Two slow clicks on the right stay single clicks, even if the left is clicked quickly.
        assert!(!clocks.release(MouseSide::Right, 1_400.0, 200));
        assert!(!clocks.release(MouseSide::Left, 1_450.0, 200));
        assert!(clocks.release(MouseSide::Left, 1_500.0, 200));
        assert!(!clocks.release(MouseSide::Right, 1_700.0, 200));
    }

    #[test]
    fn quartered_units_leave_the_selection_and_cannot_be_picked() {
        let (rules, mut state, ids) = scene();
        let mut sel = Selection::default();
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        assert_eq!(sel.ids, [ids[0], ids[1]]);
        let unit = state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
            .find(|o| o.id == ids[0])
            .unwrap();
        unit.status |= QUARTERED;
        // Quartering takes the occupant out of the selection.
        sel.retain_existing(&state);
        assert_eq!(sel.ids, [ids[1]]);
        // No click and no box hit any more; the building behind remains clickable.
        assert_eq!(target(&state, &[(ids[0], 1.0)]), None);
        assert_eq!(
            target(&state, &[(ids[0], 1.0), (ids[2], 2.0)]),
            Some(ids[2])
        );
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        assert_eq!(sel.ids, [ids[1]]);
        // Non-selectable objects (`GO+0x1A8`, e.g. destroyed buildings) stay out.
        let hq = state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
            .find(|o| o.id == ids[2])
            .unwrap();
        hq.selectable = false;
        sel.click(&state, &rules, |_| true, 0, Some(ids[2]), false);
        assert!(sel.ids.is_empty());
    }

    #[test]
    fn rectangles_take_own_units_up_to_thirty() {
        let (rules, mut state, ids) = scene();
        let mut sel = Selection::default();
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        // Two colonists, the HQ drops out as a non-unit.
        assert_eq!(sel.ids, [ids[0], ids[1]]);
        sel.rect(
            &state,
            &rules,
            |_| true,
            0,
            |o| o.world_pos[0] > 100.0,
            false,
        );
        assert_eq!(sel.ids, [ids[1]]);
        // Not the dead and not the carried.
        state
            .tables
            .iter_mut()
            .flat_map(|t| t.objects.iter_mut().flatten())
            .find(|o| o.id == ids[0])
            .unwrap()
            .status |= 0x40000;
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        assert_eq!(sel.ids, [ids[1]]);
        let mut sim = Sim::new(&mut state, &rules);
        for i in 0..40 {
            create_object(
                &mut sim,
                &mut ObjectDesc::new(1100, 0, [64.0 + i as f32, 0.0, 300.0]),
            )
            .unwrap();
        }
        sel.rect(&state, &rules, |_| true, 0, |_| true, false);
        assert_eq!(sel.ids.len(), MAX_SELECTION);
    }

    #[test]
    fn a_train_leaving_the_map_is_deselected() {
        let (_, _, ids) = scene();
        let (loco, other) = (ids[0], ids[1]);
        let mut sel = Selection::of(vec![ids[2], loco, other]);
        // Other events and a non-selected object change nothing.
        sel.apply_events(&[
            SimEvent::Created { id: loco },
            SimEvent::Deselected { object: ids[3] },
        ]);
        assert_eq!(sel.ids, [ids[2], loco, other]);
        // `Deselected { loco }` removes the locomotive, the others keep their order.
        sel.apply_events(&[SimEvent::Deselected { object: loco }]);
        assert_eq!(sel.ids, [ids[2], other]);
        // A second time (or with an empty selection) has no effect.
        sel.apply_events(&[SimEvent::Deselected { object: loco }]);
        assert_eq!(sel.ids, [ids[2], other]);
        let mut empty = Selection::default();
        empty.apply_events(&[SimEvent::Deselected { object: loco }]);
        assert!(empty.ids.is_empty());
    }

    /// re/heroes-resources 3.1, 9.2: click sequence (Shift click) -> selection. Heroes come to the
    /// front in the order of the hero list, non-heroes at the end; if the selection consists only of
    /// heroes with a smaller index, the new one lands before the last (D-231).
    // QUIRK D-231: kept
    #[test]
    fn selection_sorts_heroes_like_the_original() {
        use crate::game::testworld::{
            CARVINEZ, GOVERNOR, MENDOZA, SPA_COLONIST, SPA_SOLDIER, host, spawn,
        };
        let mut host = host();
        assert_eq!(
            [CARVINEZ, MENDOZA, GOVERNOR, SPA_COLONIST, SPA_SOLDIER]
                .map(|g| host.rules.hero_index(g)),
            [0, 1, 20, -1, -1]
        );
        let units: Vec<(u32, ObjectId)> = [SPA_COLONIST, SPA_SOLDIER, GOVERNOR, CARVINEZ, MENDOZA]
            .into_iter()
            .zip(0..)
            .map(|(g, i)| (g, spawn(&mut host, g, 0, (30 + 2 * i, 30))))
            .collect();
        let id = |g: u32| units.iter().find(|(guid, _)| *guid == g).unwrap().1;
        let clicks = |order: &[u32]| {
            let mut sel = Selection::default();
            for &g in order {
                sel.click(&host.state, &host.rules, |_| true, 0, Some(id(g)), true);
            }
            sel.ids
                .iter()
                .map(|&s| host.state.object(s, false).unwrap().guid)
                .collect::<Vec<u32>>()
        };
        for (order, expected) in [
            (&[1100, 1200][..], &[1200, 1100][..]),
            (&[1100, 1200, 1201], &[1200, 1201, 1100]),
            (&[1100, 1200, 1201, 1112], &[1200, 1201, 1112, 1100]),
            (&[1200, 1201], &[1201, 1200]),
            (&[1200, 1201, 1112], &[1201, 1112, 1200]),
            (&[1112, 1200], &[1200, 1112]),
            (&[1100, 1102], &[1100, 1102]),
        ] {
            assert_eq!(clicks(order), expected, "click sequence {order:?}");
        }
    }

    #[test]
    fn selection_handover_follows_the_original() {
        use crate::game::testworld::{COLONIST, DWELLING_TENT, TRAVOIS, host, spawn};
        let mut host = host();
        let tent = spawn(&mut host, DWELLING_TENT, 0, (10, 10));
        let travois = spawn(&mut host, TRAVOIS, 0, (20, 10));
        let colonist = spawn(&mut host, COLONIST, 0, (22, 10));
        let foreign_tent = spawn(&mut host, DWELLING_TENT, 1, (40, 40));
        let foreign_travois = spawn(&mut host, TRAVOIS, 1, (50, 40));
        let handover = |from, to, only_if_empty| {
            [SimEvent::SelectionHandover {
                from,
                to,
                only_if_empty,
            }]
        };
        let pack = handover(tent, travois, false);
        let unpack = handover(travois, tent, true);
        let run = |ids: Vec<ObjectId>, events: &[SimEvent]| {
            let mut sel = Selection::of(ids);
            sel.apply_handovers(events, &host.state, &host.rules, |_| true, 0);
            sel.ids
        };
        // Packing (re/tents 3.3): tent out, travois appended. A tent next to a unit
        // is unreachable in the game (a multi-selection keeps only units, `keep_units`);
        // the selection is set directly to test the rule.
        assert_eq!(run(vec![tent, colonist], &pack), [colonist, travois]);
        assert_eq!(run(vec![tent], &pack), [travois]);
        // Unpacking (re/tents 3.5): travois out; the tent only into a selection that is empty afterwards.
        assert_eq!(run(vec![travois, colonist], &unpack), [colonist]);
        assert_eq!(run(vec![travois], &unpack), [tent]);
        // `from` not selected: nothing, not even with an empty selection.
        assert_eq!(run(vec![colonist], &unpack), [colonist]);
        assert!(run(Vec::new(), &unpack).is_empty());
        assert_eq!(run(vec![colonist], &pack), [colonist]);
        // Foreign `to`: whoever merely looks at an enemy tent does not select its travois;
        // the tent leaves the selection anyway.
        let foreign = handover(foreign_tent, foreign_travois, false);
        assert!(run(vec![foreign_tent], &foreign).is_empty());
        // Other events change nothing.
        assert_eq!(
            run(vec![tent], &[SimEvent::Deselected { object: tent }]),
            [tent]
        );
    }

    /// Selection sound after the handover: exactly one sound of the
    /// selection afterwards, no deselect sound of its own. What is tested is the decision of the host
    /// (`apply_sim_step` + `Voices::selection`), never audible.
    #[test]
    fn a_selection_handover_plays_the_selection_sound_of_the_new_selection() {
        use crate::game::testworld::{COLONIST, DWELLING_TENT, TRAVOIS, host, spawn};
        use crate::game::voices::{OBJECT_FLAGS, SELECT_ACTION, VOICE_FLAGS, VoiceKind, Voices};
        use opennml_audio::volume::Category;
        let mut host = host();
        let tent = spawn(&mut host, DWELLING_TENT, 0, (10, 10));
        let travois = spawn(&mut host, TRAVOIS, 0, (20, 10));
        let colonist = spawn(&mut host, COLONIST, 0, (22, 10));
        let second = spawn(&mut host, COLONIST, 0, (24, 10));
        let pack = [SimEvent::SelectionHandover {
            from: tent,
            to: travois,
            only_if_empty: false,
        }];
        let unpack = [SimEvent::SelectionHandover {
            from: travois,
            to: tent,
            only_if_empty: true,
        }];
        // Selection, events, lock before -> (selection after, sound: object, action, flags, category).
        let run = |ids: Vec<ObjectId>, events: &[SimEvent], voices: &mut Voices| {
            let mut sel = Selection::of(ids);
            let reselected = sel.apply_sim_step(events, &host.state, &host.rules, |_| true, 0);
            let sound = reselected
                .then(|| voices.selection(&host.state, &sel.ids, 0))
                .flatten()
                .map(|r| {
                    assert_eq!((r.kind, r.index), (VoiceKind::Select, -1));
                    (r.object, r.action, r.flags, r.category)
                });
            (sel.ids, sound)
        };
        // (a) Tent alone, packing ends: voice of the travois (unit, without a lock check).
        let mut v = Voices::new(true);
        v.lock = Some(TRAVOIS);
        assert_eq!(
            run(vec![tent], &pack, &mut v),
            (
                vec![travois],
                Some((travois, SELECT_ACTION, VOICE_FLAGS, Category::Speech))
            )
        );
        assert_eq!(v.lock, Some(TRAVOIS));
        // (b) Travois and a unit, pitching ends: voice of the remaining unit.
        let mut v = Voices::new(true);
        assert_eq!(
            run(vec![travois, colonist], &unpack, &mut v),
            (
                vec![colonist],
                Some((colonist, SELECT_ACTION, VOICE_FLAGS, Category::Speech))
            )
        );
        // (b') If two units remain, there is no sound (`count > 1`).
        assert_eq!(
            run(vec![travois, colonist, second], &unpack, &mut v),
            (vec![colonist, second], None)
        );
        // (c) Travois alone, pitching ends: sound of the tent (finished building) with lock.
        let mut v = Voices::new(true);
        assert_eq!(
            run(vec![travois], &unpack, &mut v),
            (
                vec![tent],
                Some((tent, SELECT_ACTION, OBJECT_FLAGS, Category::Sfx))
            )
        );
        assert_eq!(v.lock, Some(DWELLING_TENT));
        // (c') If the lock already stands on the tent type (travois without `UnitResponse` selected), the
        // tent stays silent.
        let mut v = Voices::new(false);
        v.lock = Some(DWELLING_TENT);
        assert_eq!(run(vec![travois], &unpack, &mut v), (vec![tent], None));
        // `from` not selected: no handover, no sound.
        let mut v = Voices::new(true);
        assert_eq!(run(vec![colonist], &unpack, &mut v), (vec![colonist], None));
        assert_eq!(run(vec![colonist], &pack, &mut v), (vec![colonist], None));
        assert_eq!(run(Vec::new(), &pack, &mut v), (Vec::new(), None));
        assert_eq!(v.lock, None);
        // Handover and `Deselected` of the same object in the same step: one sound, no second.
        let both = [pack[0].clone(), SimEvent::Deselected { object: tent }];
        let mut v = Voices::new(true);
        let (ids, sound) = run(vec![tent], &both, &mut v);
        assert_eq!(ids, [travois]);
        assert!(sound.is_some());
    }

    /// The real packing through the simulation: at the end of the fade-out it reports the handover,
    /// the selected tent hands the selection to its travois (fog check of the host).
    #[test]
    fn a_packed_tent_hands_the_selection_to_its_travois() {
        use crate::game::testworld::{DWELLING_TENT, TRAVOIS, host, spawn};
        use opennml_sim::command::CommandKind;
        let mut host = host();
        let tent = spawn(&mut host, DWELLING_TENT, 0, (10, 10));
        let mut sel = Selection::of(vec![tent]);
        host.submit_own(CommandKind::PackTent { tent });
        let mut handed = false;
        for _ in 0..80 {
            let events = host.step_once();
            sel.apply_handovers(
                &events,
                &host.state,
                &host.rules,
                |o| host.fog_selectable(o),
                host.active_player,
            );
            if events
                .iter()
                .any(|e| matches!(e, SimEvent::SelectionHandover { .. }))
            {
                handed = true;
                break;
            }
            assert_eq!(
                sel.ids,
                [tent],
                "the tent stays until the end of the fade-out"
            );
        }
        assert!(handed, "the crossfade ends");
        assert_eq!(sel.ids.len(), 1);
        let travois = host.state.object(sel.ids[0], false).unwrap();
        assert_eq!(travois.guid, TRAVOIS);
    }

    /// The real trip: a locomotive that rolls over the map edge reports `Deselected`; the selection
    /// loses it, a second selected unit stays.
    #[test]
    fn the_simulated_train_leaves_the_selection_when_its_raster_position_leaves_the_map() {
        use crate::game::sim_host::testkit::*;
        let mut host = train_host();
        lay_track(&mut host, (50, 20), (63, 20));
        let loco = spawn_dir(&mut host, LOCO, 0, (52, 20), 0);
        let cart = spawn_dir(&mut host, CART, 0, (20, 20), 0);
        let mut sel = Selection::of(vec![loco, cart]);
        train_command(&mut host, loco, opennml_sim::command::TrainOp::Forward);
        let mut seen = false;
        for _ in 0..400 {
            let events = host.step_once();
            sel.apply_events(&events);
            if events
                .iter()
                .any(|e| matches!(e, SimEvent::Deselected { object } if *object == loco))
            {
                seen = true;
                break;
            }
            assert!(
                sel.contains(loco),
                "the locomotive stays selected before leaving"
            );
        }
        assert!(seen, "the locomotive leaves the map");
        assert_eq!(sel.ids, [cart]);
    }
}
