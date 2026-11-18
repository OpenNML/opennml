//! Control groups of the host (re/game-ui D5.4): ten lists of object IDs per player slot (`Player+0x23C`, MFC
//! `CPtrList` of HIDs in selection order) with the most recently chosen group (`Player+0x354`, -1 none), the spatial
//! clusters of the selection for the camera cycle (`GameWorld+0x390`, index `+0x3A0`) and the Shift path
//! (`FUN_004FB180`).
//!
//! In the original the commands `storegroup`, `selectgroup` and `addtogroup` run in the console interpreter directly
//! on the local player, without `NetSession_SubmitMessage`: no simulation command, no recording, no effect on the
//! checksum. Groups are never maintained (O-311): death, destruction, selling and quartering leave the ID in place;
//! it drops out when selecting. The groups of all 8 slots are in the host part of the savegame
//! (`HostPart::control_groups`, a deviation from the original, O-312); the last group and the clusters are not
//! (deviation: after loading -1, selection empty).
//!
//! In addition `selectnextguid` (`0x0048B4A0`, O-314): the cycle index per GUID (`m_LastSelectedHandleIndex`
//! `GameWorld+0x22B8`) lives here per slot ([`ControlGroups::next_guid_index`]) and is in the host part of the
//! savegame (`HostPart::next_guid_index`); the double-tap deadline ([`GuidTap`]) is volatile.
//!
//! Deviation (O-315): the clusters are rebuilt on **every** change of the selection, also when the game removes a
//! selected object (the original rebuilds them only on adding and deselecting, `GameWorld_DestroyObject`
//! `0x00481BB0` leaves the pointer in the cluster).

use std::collections::BTreeMap;

use glam::{Vec2, Vec3};
use opennml_sim::collision::fixed_pos_distance;
use opennml_sim::fx::Fx;
use opennml_sim::id::{ObjectId, PLAYER_SLOTS};

use crate::game::hotkeys::HotkeyCommand;
use crate::game::selection::{Selection, SelectionLog};
use crate::game::sim_host::SimHost;

/// Groups per player (`Player+0x23C`, `fixedsize 10`; keys 0-9).
pub const GROUPS: usize = 10;
/// The Shift path appends only to a group with fewer than 30 entries (`FUN_004FB180`, `0x004FB196`
/// `CMP [ESI+0x248],0x1E`).
pub const SHIFT_GROUP_LIMIT: usize = 30;
/// Cluster radius 5 tiles in the fixed point of the grid position (`FUN_0047F8B0`: `Fixed(1)*5`, `DAT_0067137C` =
/// 4096).
pub const CLUSTER_RADIUS: i32 = Fx::from_int(5).0;

/// The ten groups of a slot in the savegame (IDs as `u32`).
pub type GroupsSave = [Vec<u32>; GROUPS];

/// Cycle index of `selectnextguid` per GUID (`m_LastSelectedHandleIndex` `GameWorld+0x22B8`, MFC map GUID -> index):
/// object slot of the last match + 1 (`0x00580FD0`).
pub type NextGuidIndex = BTreeMap<i32, u32>;

/// Double-tap deadline of `selectnextguid` in ms (`0x0048B4FD`: `GetTickCount() + 300`).
pub const NEXT_GUID_DOUBLE_TAP_MS: u64 = 300;

/// Last press of `selectnextguid` (`DAT_00671368` GUID, `DAT_00671364` deadline; program clock instead of
/// `GetTickCount`). Volatile as in the original (static variables, not in the savegame); initial value 0/0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuidTap {
    pub guid: i32,
    /// End of the deadline (program clock in ms, exclusive).
    pub until: u64,
}

/// Groups of a player slot (`Player+0x23C`) and the most recently chosen or stored group (`+0x354`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlGroups {
    /// Per group the IDs in selection order; without duplicates (also not across groups).
    pub lists: [Vec<ObjectId>; GROUPS],
    /// Most recently chosen group 0..9, -1 none (constructor, every clearing of the selection `FUN_004FAB00`,
    /// `addtogroup`).
    pub last: i8,
    /// Cycle index of `selectnextguid` per GUID ([`NextGuidIndex`]). The original keeps **one** table in the world
    /// and searches only in the local slot; here per slot, so that the index and the searched slot belong
    /// together (the same as long as the local slot does not change). In the savegame (`HostPart::next_guid_index`).
    pub next_guid_index: NextGuidIndex,
}

impl Default for ControlGroups {
    fn default() -> Self {
        Self {
            lists: Default::default(),
            last: -1,
            next_guid_index: NextGuidIndex::new(),
        }
    }
}

/// Group number `n` as an index, only 0..9 (`0 <= n < 10` in all three commands).
fn group_index(n: u8) -> Option<usize> {
    (usize::from(n) < GROUPS).then_some(usize::from(n))
}

impl ControlGroups {
    /// `Player_RemoveFromGroups` (`0x004FB0A0`): from each of the ten lists the **first** occurrence of `id`.
    fn remove_everywhere(&mut self, id: ObjectId) {
        for list in &mut self.lists {
            if let Some(pos) = list.iter().position(|&s| s == id) {
                list.remove(pos);
            }
        }
    }

    /// `storegroup(n)` (`0x004FB1C0`): clear group `n`, [`ControlGroups::add`] with the selection, then `last = n` -
    /// also for an empty selection (then the group is empty and `n` only presses the empty camera cycle). The group
    /// then has at most as many entries as the selection (<= 30, `MAX_SELECTION`).
    pub fn store(&mut self, n: u8, selection: &[ObjectId]) {
        let Some(i) = group_index(n) else {
            return;
        };
        self.lists[i].clear();
        self.add(n, selection);
        self.last = n as i8;
    }

    /// `addtogroup(n)` (`0x004FB0E0`): each selected ID first out of all ten lists, then at the end of `n`; afterward
    /// `last = -1`. No check of owner, life or kind. Empty selection: nothing (`last` stays too).
    ///
    /// Without a limit, like the original (O-332): each ID appears at most once in the ten groups, the length is
    /// bounded by the objects of the match. The other lists only become shorter here.
    pub fn add(&mut self, n: u8, selection: &[ObjectId]) {
        let Some(i) = group_index(n) else {
            return;
        };
        if selection.is_empty() {
            return;
        }
        for &id in selection {
            self.remove_everywhere(id);
            self.lists[i].push(id);
        }
        self.last = -1;
    }

    /// Shift path (`FUN_004FB180`, from `0x00488F60` while `KD_ADD_TO_GROUP` is held): only for `last` 0..9 and a
    /// group with fewer than [`SHIFT_GROUP_LIMIT`] entries; the ID first out of all lists, then at the end.
    /// The group thus grows to at most 30; a longer one stays unchanged.
    pub fn add_to_last(&mut self, id: ObjectId) {
        let Ok(i) = usize::try_from(self.last) else {
            return;
        };
        if i >= GROUPS || self.lists[i].len() >= SHIFT_GROUP_LIMIT {
            return;
        }
        self.remove_everywhere(id);
        self.lists[i].push(id);
    }

    /// First group 0..9 containing `id` (`FUN_004FAF20`, group digit on the health bar).
    pub fn first_group_of(&self, id: ObjectId) -> Option<u8> {
        self.lists
            .iter()
            .position(|list| list.contains(&id))
            .map(|i| i as u8)
    }

    /// The selection log ([`Selection::take_log`]) in its sequence: clearing -> `last = -1` (`FUN_004FAB00`),
    /// adding while `KD_ADD_TO_GROUP` is held (`add_held`) -> [`ControlGroups::add_to_last`] (O-310).
    pub fn apply_log(&mut self, log: &[SelectionLog], add_held: bool) {
        for entry in log {
            match *entry {
                SelectionLog::Cleared => self.last = -1,
                SelectionLog::Added(id) if add_held => self.add_to_last(id),
                SelectionLog::Added(_) => {}
            }
        }
    }

    /// The groups for the savegame (`m_Groups`); not `last` (deviation).
    pub fn to_save(&self) -> GroupsSave {
        self.lists
            .each_ref()
            .map(|list| list.iter().map(|id| id.0).collect())
    }

    /// Groups from the savegame, `last = -1` (the original does not save `+0x354`); without the
    /// `selectnextguid` index (which is kept separately in `HostPart::next_guid_index`).
    pub fn from_save(save: &GroupsSave) -> Self {
        Self {
            lists: save
                .each_ref()
                .map(|list| list.iter().map(|&v| ObjectId(v)).collect()),
            last: -1,
            next_guid_index: NextGuidIndex::new(),
        }
    }
}

/// Empty groups of all 8 slots for the savegame.
pub fn empty_save() -> Vec<GroupsSave> {
    vec![GroupsSave::default(); PLAYER_SLOTS]
}

/// Empty `selectnextguid` indices of all 8 slots for the savegame.
pub fn empty_next_guid_index() -> Vec<NextGuidIndex> {
    vec![NextGuidIndex::new(); PLAYER_SLOTS]
}

/// Spatial clusters of the selection for the camera cycle (`GameWorld+0x390`, index `+0x3A0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionClusters {
    clusters: Vec<Vec<ObjectId>>,
    /// Next cluster of the cycle; -1 without clusters (`FUN_0047F840`).
    index: i32,
    /// The selection the clusters are built from (deviation, O-315: every change rebuilds).
    built_for: Vec<ObjectId>,
}

impl Default for SelectionClusters {
    fn default() -> Self {
        Self {
            clusters: Vec::new(),
            index: -1,
            built_for: Vec::new(),
        }
    }
}

impl SelectionClusters {
    /// `FUN_0047F8B0`: clear the clusters (`index = -1`); with a selection each object, in selection order, goes to
    /// the nearest cluster whose **first** member lies closer than 5 tiles in the grid position (2D, x/z, fixed point)
    /// (`FixedPos_Distance` `0x00401200`, strictly smaller; on a tie the earlier cluster), otherwise into a new one;
    /// then `index = 0`. `pos` gives the grid position (`+0x19C`, `+0x1A0`); without a position the object does not
    /// count.
    pub fn rebuild(&mut self, ids: &[ObjectId], pos: impl Fn(ObjectId) -> Option<(i32, i32)>) {
        self.clusters.clear();
        self.index = -1;
        self.built_for = ids.to_vec();
        if ids.is_empty() {
            return;
        }
        let mut firsts: Vec<(i32, i32)> = Vec::new();
        for &id in ids {
            let Some(p) = pos(id) else {
                continue;
            };
            let mut best = None;
            let mut best_d = CLUSTER_RADIUS;
            for (i, &first) in firsts.iter().enumerate() {
                let d = fixed_pos_distance(p, first);
                if d < best_d {
                    best_d = d;
                    best = Some(i);
                }
            }
            match best {
                Some(i) => self.clusters[i].push(id),
                None => {
                    firsts.push(p);
                    self.clusters.push(vec![id]);
                }
            }
        }
        self.index = 0;
    }

    /// Are the clusters built from exactly this selection?
    pub fn is_built_for(&self, ids: &[ObjectId]) -> bool {
        self.built_for == ids
    }

    /// The clusters in their sequence.
    pub fn clusters(&self) -> &[Vec<ObjectId>] {
        &self.clusters
    }

    /// Next cluster of the cycle (-1 without clusters).
    pub fn index(&self) -> i32 {
        self.index
    }

    /// `CycleClusterCamera` (`0x0047BE00`): target of the next cluster, cyclic - the mean of the world positions of its
    /// members (`GameObject_GetWorldPosition` `0x004669E0`, sum in `f32`, divided by the count) as (x, z).
    /// `None` without clusters. Members without a world position do not count; if none has one, the step has no
    /// target.
    pub fn next_target(&mut self, world: impl Fn(ObjectId) -> Option<Vec3>) -> Option<Vec2> {
        let i = usize::try_from(self.index).ok()?;
        let cluster = self.clusters.get(i)?;
        self.index = ((i + 1) % self.clusters.len()) as i32;
        let mut sum = Vec2::ZERO;
        let mut count = 0u32;
        for &id in cluster {
            if let Some(p) = world(id) {
                sum += Vec2::new(p.x, p.z);
                count += 1;
            }
        }
        (count > 0).then(|| sum / count as f32)
    }
}

/// What a group command does for the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GroupOutcome {
    /// Nothing for the host (storing, adding, empty cycle, group outside 0..9).
    Nothing,
    /// New selection: `{0x10, 2}` -> `selection_changed` (selection sound only for exactly one own object,
    /// re/game-ui N.2).
    Selected,
    /// Camera cycle: target (x, z) for `GameEngine_MoveCameraTo(x, z, 0)` (offset stays).
    Camera(Vec2),
}

/// The parts of the host that touch the groups of the local player.
pub struct GroupCx<'a> {
    /// Groups of the local player (`GameEngine_GetLocalPlayer`).
    pub groups: &'a mut ControlGroups,
    pub clusters: &'a mut SelectionClusters,
    pub selection: &'a mut Selection,
    pub sim: &'a SimHost,
    /// `KD_ADD_TO_GROUP` held (`FUN_004143A0`).
    pub add_held: bool,
}

impl GroupCx<'_> {
    /// Process the selection log ([`ControlGroups::apply_log`]) and rebuild the clusters if the selection has
    /// changed or something was added or cleared (deviation, O-315).
    pub fn sync(&mut self) {
        let log = self.selection.take_log();
        self.groups.apply_log(&log, self.add_held);
        if !log.is_empty() || !self.clusters.is_built_for(&self.selection.ids) {
            let state = &self.sim.state;
            self.clusters.rebuild(&self.selection.ids, |id| {
                state.object(id, false).map(|o| (o.grid.x.0, o.grid.z.0))
            });
        }
    }

    /// A group command (`storegroup`, `selectgroup`, `addtogroup`); other commands: nothing.
    ///
    /// `selectgroup(n)` (`0x004FAE90`): if `n` is the last group, only the camera cycle (`0x0047BE00`). Otherwise clear
    /// the selection (`0x0047A130`, `last = -1`), each ID through [`Selection::select_list`], for a non-empty
    /// selection `last = n`, then `{0x10, 2}` - also for an empty one. The first press does not move the camera.
    pub fn run(&mut self, cmd: &HotkeyCommand) -> GroupOutcome {
        // What happened since the last sync counts before the command (order of the original).
        self.sync();
        match *cmd {
            HotkeyCommand::StoreGroup(n) => self.groups.store(n, &self.selection.ids),
            HotkeyCommand::AddToGroup(n) => self.groups.add(n, &self.selection.ids),
            HotkeyCommand::SelectGroup(n) => {
                let Some(i) = group_index(n) else {
                    return GroupOutcome::Nothing;
                };
                if self.groups.last == n as i8 {
                    let state = &self.sim.state;
                    return self
                        .clusters
                        .next_target(|id| state.object(id, false).map(|o| o.world_pos.into()))
                        .map_or(GroupOutcome::Nothing, GroupOutcome::Camera);
                }
                let sim = self.sim;
                let ids = self.groups.lists[i].clone();
                self.selection.clear();
                self.selection
                    .select_list(&sim.state, &sim.rules, |o| sim.fog_selectable(o), &ids);
                self.sync();
                if !self.selection.ids.is_empty() {
                    self.groups.last = n as i8;
                }
                return GroupOutcome::Selected;
            }
            _ => {}
        }
        GroupOutcome::Nothing
    }

    /// `selectnextguid(guid)` (`GameWorld_SelectNextGuid` `0x0048B4A0`, O-314) at the program clock `now`:
    ///
    /// 1. Same GUID as at the last press, `now` before the deadline and the **first** selected object has this GUID ->
    ///    only the camera onto this object (double tap; the deadline stays).
    /// 2. Otherwise a new deadline (`now + 300`), cyclic search from the remembered index (from the capacity: 0)
    ///    through the slot table of the local slot for an object with this GUID, **without a status check**
    ///    (construction sites, dying and quartered objects count). No match: nothing.
    /// 3. The match is the first selected object -> only the camera, the index stays.
    /// 4. Otherwise clear the selection (`0x0047A130`), the match through [`Selection::select_list`] (`0x00488F60`, may
    ///    reject -> empty selection), index = slot + 1, `{0x10, 2}` ([`GroupOutcome::Selected`]).
    ///
    /// The first press never moves the camera. Local: no command, no effect on the checksum.
    pub fn select_next_guid(&mut self, guid: i32, now: u64, tap: &mut GuidTap) -> GroupOutcome {
        self.sync();
        let state = &self.sim.state;
        let first = self
            .selection
            .ids
            .first()
            .and_then(|&id| state.object(id, false));
        let camera = |o: &opennml_sim::object::GameObject| {
            GroupOutcome::Camera(Vec2::new(o.world_pos[0], o.world_pos[2]))
        };
        if guid == tap.guid
            && now < tap.until
            && let Some(f) = first.filter(|f| f.guid as i32 == guid)
        {
            return camera(f);
        }
        *tap = GuidTap {
            guid,
            until: now.saturating_add(NEXT_GUID_DOUBLE_TAP_MS),
        };
        let Some(table) = state.tables.get(usize::from(self.sim.active_player)) else {
            return GroupOutcome::Nothing;
        };
        let cap = table.objects.len();
        let mut start = self
            .groups
            .next_guid_index
            .get(&guid)
            .map_or(0, |&i| i as usize);
        if start >= cap {
            start = 0;
        }
        let hit = (start..cap).chain(0..start).find_map(|i| {
            table.objects[i]
                .as_ref()
                .filter(|o| o.guid as i32 == guid)
                .map(|o| (i, o))
        });
        let Some((index, obj)) = hit else {
            return GroupOutcome::Nothing;
        };
        if first.is_some_and(|f| f.id == obj.id) {
            return camera(obj);
        }
        let (sim, id) = (self.sim, obj.id);
        self.selection.clear();
        self.selection
            .select_list(&sim.state, &sim.rules, |o| sim.fog_selectable(o), &[id]);
        self.groups.next_guid_index.insert(guid, index as u32 + 1);
        self.sync();
        GroupOutcome::Selected
    }
}

#[cfg(test)]
mod tests;
