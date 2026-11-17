//! Tests of the control groups.

use super::*;
use crate::game::options::DEFAULT_GAME_SPEED;
use crate::game::voices::Voices;
use crate::game_save::{CameraSave, GameSaveFile, HostPart};
use nml_sim::object::status;
use nml_sim::rules::Rules;
use nml_sim::setup::MatchSetup;
use nml_sim::sim::Sim;
use nml_sim::world::{ObjectDesc, create_object, destroy_object};

use HotkeyCommand as C;

/// Colonist (unit) and HQ (building) like `sim_host::plain_testkit`.
const TYPES: &str = crate::game::sim_host::plain_testkit::TYPES;
const UNIT: u32 = 1100;
const HQ: u32 = 1000;

/// Match on 16 x 16 tiles, player 0 active, map revealed; plus groups, clusters and selection of the host.
struct World {
    host: SimHost,
    groups: ControlGroups,
    clusters: SelectionClusters,
    selection: Selection,
    /// `KD_ADD_TO_GROUP` held.
    add_held: bool,
    /// Last press of `selectnextguid` (transient like `GameSession::next_guid_tap`).
    tap: GuidTap,
}

impl World {
    fn new() -> Self {
        let rules = Rules::synthetic(TYPES, 16);
        let mut host = SimHost::new(rules, setup(), DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        nml_sim::fog::reveal_map(&mut host.state, -1);
        Self {
            host,
            groups: ControlGroups::default(),
            clusters: SelectionClusters::default(),
            selection: Selection::default(),
            add_held: false,
            tap: GuidTap::default(),
        }
    }

    /// Object `guid` of slot `slot` at the center of the tile `tile`.
    fn spawn(&mut self, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let rules = self.host.rules.clone();
        let mut sim = Sim::new(&mut self.host.state, &rules);
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        create_object(
            &mut sim,
            &mut ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]),
        )
        .unwrap()
    }

    fn cx(&mut self) -> GroupCx<'_> {
        GroupCx {
            groups: &mut self.groups,
            clusters: &mut self.clusters,
            selection: &mut self.selection,
            sim: &self.host,
            add_held: self.add_held,
        }
    }

    /// Key command like `GameSession::run_hotkey`.
    fn key(&mut self, cmd: C) -> GroupOutcome {
        self.cx().run(&cmd)
    }

    /// `selectnextguid(guid)` at the program clock `now` like `GameSession::select_next_guid`.
    fn next(&mut self, guid: u32, now: u64) -> GroupOutcome {
        GroupCx {
            groups: &mut self.groups,
            clusters: &mut self.clusters,
            selection: &mut self.selection,
            sim: &self.host,
            add_held: self.add_held,
        }
        .select_next_guid(guid as i32, now, &mut self.tap)
    }

    /// Click on `target` (with Shift `add`) like `finish_selection`, then the sync of the host.
    fn click(&mut self, target: Option<ObjectId>, add: bool) {
        let state = &self.host.state;
        self.selection
            .click(state, &self.host.rules, |_| true, 0, target, add);
        self.cx().sync();
    }

    /// Selection exactly `ids` (click, then Shift-clicks).
    fn select(&mut self, ids: &[ObjectId]) {
        self.click(None, false);
        for &id in ids {
            self.click(Some(id), true);
        }
        assert_eq!(self.selection.ids, ids);
    }

    /// World position (x, z) of an object.
    fn at(&self, id: ObjectId) -> Vec2 {
        let [x, _, z] = self.host.state.object(id, false).unwrap().world_pos;
        Vec2::new(x, z)
    }

    /// Selection sound of the current selection (`Voices::selection`, re/game-ui N.2).
    fn sound(&self) -> bool {
        Voices::new(true)
            .selection(&self.host.state, &self.selection.ids, 0)
            .is_some()
    }

    /// `sim_hash` of the host (checksum and sequence).
    fn hash(&self) -> String {
        format!("{:?}", self.host.hash_result())
    }
}

fn setup() -> MatchSetup {
    MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: nml_sim::ki::KiSetup::default(),
        script: nml_sim::script::ScriptSetup::default(),
        ..MatchSetup::default()
    }
}

/// O-43: select A, B, Ctrl+1 -> G1 = [A, B], no camera; 1 -> only the camera on the mean; select C, 1 ->
/// [A, B] without camera and without sound; 1 -> camera. The camera cycle keeps the offset (`MoveCameraTo(x, z, 0)`).
#[test]
fn store_select_and_cycle() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let b = w.spawn(UNIT, 0, (3, 1));
    let c = w.spawn(UNIT, 0, (8, 8));
    w.select(&[a, b]);
    assert_eq!(w.key(C::StoreGroup(1)), GroupOutcome::Nothing);
    assert_eq!(w.groups.lists[1], [a, b]);
    assert_eq!(w.groups.last, 1);
    let mean = (w.at(a) + w.at(b)) / 2.0;
    assert_eq!(w.key(C::SelectGroup(1)), GroupOutcome::Camera(mean));
    assert_eq!(w.selection.ids, [a, b], "only camera, selection stays");
    // The host moves the camera with the current offset.
    let (mut camera, terrain) = crate::game::input::testkit::setup();
    assert!(camera.yaw(0.3));
    let offset = camera.eye - camera.target;
    camera.move_to_keeping_offset(mean.x, mean.y, &terrain);
    assert_eq!(camera.target, Vec3::new(mean.x, 0.0, mean.y));
    assert!((camera.eye - camera.target - offset).length() < 1e-2);
    // Selecting C clears the selection: the last group is -1, 1 selects again.
    w.click(Some(c), false);
    assert_eq!(w.groups.last, -1);
    assert_eq!(w.key(C::SelectGroup(1)), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [a, b]);
    assert_eq!(w.groups.last, 1);
    assert!(!w.sound(), "two objects: no selection sound");
    assert_eq!(w.key(C::SelectGroup(1)), GroupOutcome::Camera(mean));
    // A group with exactly one own object gives the sound (re/game-ui N.2).
    w.click(Some(c), false);
    w.key(C::StoreGroup(4));
    w.click(None, false);
    assert_eq!(w.key(C::SelectGroup(4)), GroupOutcome::Selected);
    assert!(w.sound());
    // Groups outside 0..9 do nothing.
    for cmd in [C::StoreGroup(10), C::SelectGroup(10), C::AddToGroup(255)] {
        let before = (w.groups.clone(), w.selection.ids.clone());
        assert_eq!(w.key(cmd), GroupOutcome::Nothing);
        assert_eq!((w.groups.clone(), w.selection.ids.clone()), before);
    }
}

/// Clusters (`FUN_0047F8B0`, `0x0047BE00`): A, D more than 5 tiles apart, Ctrl+2; 2, 2, 2 -> camera A, D, A. An object
/// joins the nearest cluster whose **first** member is closer than 5 tiles (strictly), otherwise a new one.
#[test]
fn clusters_use_the_first_member_and_five_tiles() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let d = w.spawn(UNIT, 0, (12, 1));
    w.select(&[a, d]);
    w.key(C::StoreGroup(2));
    let cameras: Vec<_> = (0..3).map(|_| w.key(C::SelectGroup(2))).collect();
    assert_eq!(
        cameras,
        [
            GroupOutcome::Camera(w.at(a)),
            GroupOutcome::Camera(w.at(d)),
            GroupOutcome::Camera(w.at(a))
        ]
    );
    // Grid positions in tiles: the first cluster takes P1 (4 tiles), P2 (8 from P0, 4 from P1) opens a new one,
    // P3 (5 from P0, 3 from P2) goes to the nearest under 5, P4 exactly 5 tiles from the first -> new.
    let tile = |x: i32, z: i32| (Fx::from_int(x).0, Fx::from_int(z).0);
    let ids: Vec<_> = (0..5u16).map(|i| ObjectId::new(i, 0, 1)).collect();
    let pos = [tile(0, 0), tile(4, 0), tile(8, 0), tile(5, 0), tile(0, 5)];
    let mut clusters = SelectionClusters::default();
    assert_eq!(clusters.index(), -1);
    clusters.rebuild(&ids, |id| Some(pos[id.index()]));
    assert_eq!(
        clusters.clusters(),
        [vec![ids[0], ids[1]], vec![ids[2], ids[3]], vec![ids[4]]]
    );
    assert_eq!(clusters.index(), 0);
    // Mean of the world positions, cyclic; without a world position a member does not count.
    let world =
        |id: ObjectId| (id != ids[1]).then(|| Vec3::new(id.index() as f32 * 10.0, 7.0, 2.0));
    assert_eq!(clusters.next_target(world), Some(Vec2::new(0.0, 2.0)));
    assert_eq!(clusters.next_target(world), Some(Vec2::new(25.0, 2.0)));
    assert_eq!(clusters.next_target(world), Some(Vec2::new(40.0, 2.0)));
    assert_eq!(clusters.index(), 0);
    clusters.rebuild(&[], |_| None);
    assert_eq!(clusters.next_target(world), None);
}

/// Shift+2 with [B] -> G2 = [A, D, B], B out of G1 (`addtogroup` `0x004FB0E0`: each ID first out of all lists, then
/// at the back; `last = -1`; empty selection: nothing). Ctrl+2 with [A, D] takes A out of G1.
#[test]
fn add_moves_ids_between_groups() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let b = w.spawn(UNIT, 0, (3, 1));
    let d = w.spawn(UNIT, 0, (12, 1));
    w.select(&[a, b]);
    w.key(C::StoreGroup(1));
    w.select(&[a, d]);
    w.key(C::StoreGroup(2));
    assert_eq!((w.groups.lists[1].clone(), w.groups.last), (vec![b], 2));
    w.select(&[b]);
    assert_eq!(w.key(C::AddToGroup(2)), GroupOutcome::Nothing);
    assert_eq!(w.groups.lists[2], [a, d, b]);
    assert!(w.groups.lists[1].is_empty());
    assert_eq!(w.groups.last, -1);
    // An object already in the group moves to the end.
    w.select(&[a]);
    w.key(C::AddToGroup(2));
    assert_eq!(w.groups.lists[2], [d, b, a]);
    // Empty selection: nothing, `last` stays too.
    w.key(C::StoreGroup(5));
    w.click(None, false);
    w.groups.last = 5;
    w.key(C::AddToGroup(2));
    assert_eq!((w.groups.lists[2].clone(), w.groups.last), (vec![d, b], 5));
    assert_eq!(w.groups.lists[5], [a]);
    // `first_group_of` (`FUN_004FAF20`): the first group.
    assert_eq!(w.groups.first_group_of(b), Some(2));
    assert_eq!(w.groups.first_group_of(a), Some(5));
    assert_eq!(w.groups.first_group_of(ObjectId::NONE), None);
}

/// O-310 (`FUN_004FB180` from `0x00488F60`): after 1 (G1 = [C]) Shift-click D -> G1 = [C, D], D out of G2;
/// Shift-click D (deselect) -> G1 stays. The path applies at every accepted addition before the unit filter
/// (also a building that drops out again right away), only for `last` 0..9 and fewer than 30 entries.
#[test]
fn shift_click_extends_the_last_group() {
    let mut w = World::new();
    let c = w.spawn(UNIT, 0, (1, 1));
    let d = w.spawn(UNIT, 0, (3, 1));
    let h = w.spawn(HQ, 0, (8, 8));
    w.select(&[d]);
    w.key(C::StoreGroup(2));
    w.select(&[c]);
    w.key(C::StoreGroup(1));
    w.click(None, false);
    assert_eq!(w.key(C::SelectGroup(1)), GroupOutcome::Selected);
    assert_eq!(w.groups.last, 1);
    w.add_held = true;
    w.click(Some(d), true);
    assert_eq!(w.selection.ids, [c, d]);
    assert_eq!(w.groups.lists[1], [c, d]);
    assert!(w.groups.lists[2].is_empty(), "D out of G2");
    w.click(Some(d), true);
    assert_eq!(w.selection.ids, [c]);
    assert_eq!(w.groups.lists[1], [c, d], "deselecting takes nothing out");
    // A building enters the group before the filter, the selection keeps only units.
    w.click(Some(h), true);
    assert_eq!(w.selection.ids, [c]);
    assert_eq!(w.groups.lists[1], [c, d, h]);
    // Nothing without the key held.
    w.add_held = false;
    w.click(Some(d), true);
    assert_eq!(w.groups.lists[1], [c, d, h]);
    // After clearing (click without Shift) the path no longer works.
    w.add_held = true;
    w.click(Some(c), false);
    assert_eq!(w.groups.last, -1);
    w.click(Some(d), true);
    assert_eq!(w.groups.lists[1], [c, d, h]);
    // At most 30 entries via the Shift path; `addtogroup` knows no limit.
    let mut groups = ControlGroups::default();
    let many: Vec<_> = (0..31u16).map(|i| ObjectId::new(i, 0, 1)).collect();
    groups.store(3, &many[..29]);
    groups.add_to_last(many[29]);
    groups.add_to_last(many[30]);
    assert_eq!(groups.lists[3], many[..30]);
    groups.add(3, &many[30..]);
    assert_eq!(groups.lists[3].len(), 31);
    groups.last = -1;
    groups.add_to_last(ObjectId::new(99, 0, 1));
    assert_eq!(groups.lists[3].len(), 31);
}

/// O-311: groups are never maintained. A dies -> G1 = [A, B] stays, 1 -> [B]; a removed object (stale ID)
/// silently drops out; G1 = [A, H] -> 1 -> [A] (building drops out); [H1, H2] -> empty, `last` stays -1;
/// a building alone can be selected.
#[test]
fn groups_are_never_pruned() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let b = w.spawn(UNIT, 0, (3, 1));
    let e = w.spawn(UNIT, 0, (5, 1));
    let h1 = w.spawn(HQ, 0, (8, 8));
    let h2 = w.spawn(HQ, 0, (12, 12));
    // One tick: buildings count for the fog via their fog memory (`0x00467D70`).
    w.host.step_once();
    w.select(&[a, b, e]);
    w.key(C::StoreGroup(1));
    // A dies (carcass, not selectable), E disappears completely.
    let o = w.host.state.object_mut(a, false).unwrap();
    o.status |= status::DIED;
    o.selectable = false;
    let rules = w.host.rules.clone();
    destroy_object(&mut Sim::new(&mut w.host.state, &rules), e);
    assert!(w.host.state.object(e, false).is_none());
    w.selection.retain_existing(&w.host.state);
    w.cx().sync();
    assert_eq!(w.selection.ids, [b]);
    assert_eq!(w.groups.lists[1], [a, b, e], "no maintenance");
    // Deviation (O-315): the clusters follow the smaller selection.
    assert_eq!(w.clusters.clusters(), [vec![b]]);
    w.click(None, false);
    assert_eq!(w.key(C::SelectGroup(1)), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [b]);
    assert_eq!(w.groups.lists[1], [a, b, e]);
    // Group [B, H]: the building drops out but stays in the group.
    w.groups.lists[6] = vec![b, h1];
    w.click(None, false);
    w.key(C::SelectGroup(6));
    assert_eq!(w.selection.ids, [b]);
    assert_eq!(w.groups.lists[6], [b, h1]);
    // Two buildings -> empty selection, `last` stays -1, the event comes anyway.
    w.groups.lists[7] = vec![h1, h2];
    assert_eq!(w.key(C::SelectGroup(7)), GroupOutcome::Selected);
    assert!(w.selection.ids.is_empty());
    assert_eq!(w.groups.last, -1);
    // A building alone can be selected.
    w.groups.lists[8] = vec![h2];
    w.key(C::SelectGroup(8));
    assert_eq!((w.selection.ids.clone(), w.groups.last), (vec![h2], 8));
}

/// O-311 (`storegroup` `0x004FB1EA`): Ctrl+n with an empty selection clears the group and sets `last = n`; pressing n
/// afterwards runs only the empty camera cycle (no clusters: nothing).
#[test]
fn store_with_empty_selection_clears_and_sets_last() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    w.select(&[a]);
    w.key(C::StoreGroup(3));
    assert_eq!(w.groups.lists[3], [a]);
    w.click(None, false);
    assert_eq!(w.groups.last, -1);
    assert_eq!(w.key(C::StoreGroup(3)), GroupOutcome::Nothing);
    assert!(w.groups.lists[3].is_empty());
    assert_eq!(w.groups.last, 3);
    assert_eq!(w.key(C::SelectGroup(3)), GroupOutcome::Nothing);
    assert!(w.selection.ids.is_empty());
    assert_eq!(w.groups.last, 3);
}

/// Host part with the groups `groups` (other fields any valid values).
fn host_part(groups: Vec<GroupsSave>) -> HostPart {
    HostPart {
        camera: CameraSave {
            from: [100.0, 2000.0, -300.0],
            at: [800.0, 150.0, 900.0],
            fov: 0.75,
        },
        level_info: nml_ui::draft::LevelInfoData::default(),
        game_type: 1,
        local_name: "Tester".into(),
        game_speed: 10,
        cinemascope_speed: None,
        script_camera: Default::default(),
        control_groups: groups,
        next_guid_index: empty_next_guid_index(),
        idle_worker_index: [0; 8],
        masks: crate::game::diplomacy::Masks::initial(),
        lock_teams: false,
        fog_show_mode: 0,
        net: None,
        log: None,
        save_replay: false,
    }
}

/// Saving and loading via the file of the savegame; the host takes over the groups like
/// `GameSession::resume_from` (selection empty, last group -1, no clusters).
fn save_and_load(w: &World, slots: &[ControlGroups]) -> (World, Vec<ControlGroups>) {
    let mut host = host_part(slots.iter().map(ControlGroups::to_save).collect());
    host.next_guid_index = slots.iter().map(|g| g.next_guid_index.clone()).collect();
    let file = GameSaveFile {
        sim: w.host.save(),
        host,
    };
    let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
    back.check(&w.host.rules).unwrap();
    let mut loaded = World::new();
    loaded.host.load(back.sim, None);
    let groups: Vec<_> = back
        .host
        .control_groups
        .iter()
        .zip(&back.host.next_guid_index)
        .map(|(save, index)| ControlGroups {
            next_guid_index: index.clone(),
            ..ControlGroups::from_save(save)
        })
        .collect();
    loaded.groups = groups[0].clone();
    (loaded, groups)
}

/// O-312 (deviation): G1...G9 come back, the selection is empty, the last group -1; 1 selects (without camera).
#[test]
fn groups_survive_save_and_load_with_empty_selection() {
    let mut w = World::new();
    let units: Vec<_> = (1..10).map(|i| w.spawn(UNIT, 0, (i, 2))).collect();
    for (n, &id) in (1u8..).zip(&units) {
        w.select(&[id]);
        w.key(C::StoreGroup(n));
    }
    w.select(&[units[0]]);
    w.key(C::SelectGroup(1));
    assert_eq!(w.groups.last, 1);
    let mut slots = vec![ControlGroups::default(); PLAYER_SLOTS];
    slots[0] = w.groups.clone();
    slots[3].lists[9] = vec![ObjectId::new(5, 3, 1)];
    let (mut loaded, groups) = save_and_load(&w, &slots);
    assert_eq!(groups.len(), PLAYER_SLOTS);
    for (n, &id) in (1..10).zip(&units) {
        assert_eq!(loaded.groups.lists[n], [id], "G{n}");
    }
    assert_eq!(groups[3].lists[9], [ObjectId::new(5, 3, 1)], "all 8 slots");
    assert_eq!(loaded.groups.last, -1);
    assert!(loaded.selection.ids.is_empty());
    assert_eq!(loaded.clusters.index(), -1);
    assert_eq!(loaded.key(C::SelectGroup(1)), GroupOutcome::Selected);
    assert_eq!(loaded.selection.ids, [units[0]]);
    assert_eq!(
        loaded.key(C::SelectGroup(1)),
        GroupOutcome::Camera(w.at(units[0]))
    );
}

/// A group with a removed object (stale ID), a carcass still present (died, not selectable) and an object of
/// another slot survives saving and loading; all IDs stay in the group, only what `Selection` allows is selected
/// (the foreign object alone; together with an own unit the multi-selection of the units remains).
#[test]
fn stale_group_ids_survive_save_and_load() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let b = w.spawn(UNIT, 0, (3, 1));
    let corpse = w.spawn(UNIT, 0, (4, 1));
    let foreign = w.spawn(UNIT, 1, (6, 1));
    let foreign2 = w.spawn(UNIT, 1, (7, 1));
    w.select(&[a, corpse]);
    w.key(C::StoreGroup(1));
    let rules = w.host.rules.clone();
    destroy_object(&mut Sim::new(&mut w.host.state, &rules), a);
    let o = w.host.state.object_mut(corpse, false).unwrap();
    o.status |= status::DIED;
    o.selectable = false;
    w.groups.lists[1].push(foreign);
    w.groups.lists[2] = vec![b, foreign2];
    let mut slots = vec![ControlGroups::default(); PLAYER_SLOTS];
    slots[0] = w.groups.clone();
    let (mut loaded, _) = save_and_load(&w, &slots);
    let dead = loaded
        .host
        .state
        .object(corpse, false)
        .expect("carcass in the savegame");
    assert!(dead.status & status::DIED != 0 && !dead.selectable);
    assert!(loaded.host.state.object(a, false).is_none());
    assert_eq!(loaded.groups.lists[1], [a, corpse, foreign]);
    assert_eq!(loaded.key(C::SelectGroup(1)), GroupOutcome::Selected);
    assert_eq!(loaded.selection.ids, [foreign], "foreign, not rejected");
    assert_eq!(loaded.groups.lists[1], [a, corpse, foreign]);
    loaded.key(C::SelectGroup(2));
    assert_eq!(loaded.selection.ids, [b, foreign2]);
}

/// O-332: `addtogroup` does not truncate (as in the original): after 300 entries in batches every ID is in the
/// group, and the savegame loads the same list.
#[test]
fn add_keeps_every_entry_beyond_255() {
    let mut w = World::new();
    let ids: Vec<_> = (0..300u16).map(|i| ObjectId::new(i, 0, 1)).collect();
    for batch in ids.chunks(30) {
        w.groups.add(1, batch);
    }
    assert_eq!(w.groups.lists[1], ids);
    // An ID already inside moves to the end without changing the length.
    w.groups.add(1, &[ids[100]]);
    assert_eq!(w.groups.lists[1].len(), 300);
    assert_eq!(w.groups.lists[1].last(), Some(&ids[100]));
    // The Shift path appends nothing to such a long group.
    w.groups.last = 1;
    w.groups.add_to_last(ObjectId::new(400, 0, 1));
    assert_eq!(w.groups.lists[1].len(), 300);
    let mut slots = vec![ControlGroups::default(); PLAYER_SLOTS];
    slots[0] = w.groups.clone();
    let (loaded, _) = save_and_load(&w, &slots);
    assert_eq!(loaded.groups.lists[1], w.groups.lists[1]);
}

/// Group commands change neither the checksum nor the command log of the simulation (local, without
/// `NetSession_SubmitMessage`); neither do the Shift path and the camera cycle.
#[test]
fn groups_change_no_checksum() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (1, 1));
    let b = w.spawn(UNIT, 0, (12, 1));
    for _ in 0..3 {
        w.host.step_once();
    }
    let before = (w.hash(), w.host.log.len());
    w.select(&[a]);
    w.key(C::StoreGroup(1));
    w.key(C::SelectGroup(1));
    w.add_held = true;
    w.click(Some(b), true);
    w.key(C::AddToGroup(2));
    w.click(None, false);
    w.key(C::SelectGroup(2));
    w.key(C::SelectGroup(1));
    w.key(C::SelectGroup(1));
    assert_eq!((w.hash(), w.host.log.len()), before);
}

/// Two own HQs (H1 before H2 in the slot table), a foreign HQ and an own unit; one tick so that the buildings have
/// a fog memory (fog check `0x00467D70` when selecting).
fn two_hqs() -> (World, ObjectId, ObjectId, ObjectId, ObjectId) {
    let mut w = World::new();
    let h1 = w.spawn(HQ, 0, (3, 3));
    let foe = w.spawn(HQ, 1, (3, 11));
    let h2 = w.spawn(HQ, 0, (11, 11));
    let unit = w.spawn(UNIT, 0, (8, 2));
    w.host.step_once();
    assert!(h1.index() < h2.index());
    (w, h1, h2, foe, unit)
}

/// O-314 (`0x0048B4A0`): the first press selects without a camera; a second press of the same GUID within 300 ms
/// (program clock) moves only the camera to the first selected object if it has this GUID; otherwise the search
/// goes from the remembered index (object slot + 1) cyclically through the objects of the local slot; a hit already
/// selected moves only the camera. Foreign slots are never searched.
#[test]
fn select_next_guid_cycles_and_double_taps() {
    let (mut w, h1, h2, foe, unit) = two_hqs();
    let g = HQ as i32;
    let index = |id: ObjectId| id.index() as u32 + 1;
    // 1st press: HQ 1, no camera; index = slot + 1.
    assert_eq!(w.next(HQ, 1_000), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h1]);
    assert_eq!(w.groups.next_guid_index[&g], index(h1));
    // After more than 300 ms: HQ 2, then HQ 1 again (cyclic, the foreign HQ never).
    assert_eq!(w.next(HQ, 1_400), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h2]);
    assert_eq!(w.groups.next_guid_index[&g], index(h2));
    assert_eq!(w.next(HQ, 2_000), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h1]);
    // Within 300 ms (deadline `GetTickCount + 300`, strict): only camera on HQ 1, selection and index stay.
    assert_eq!(w.next(HQ, 2_299), GroupOutcome::Camera(w.at(h1)));
    assert_eq!(w.selection.ids, [h1]);
    assert_eq!(w.groups.next_guid_index[&g], index(h1));
    // The double tap does not renew the deadline: exactly 300 ms after the last selection the search continues.
    assert_eq!(w.next(HQ, 2_300), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h2]);
    // Index behind H2: the next press selects H1 again.
    w.click(Some(h2), false);
    assert_eq!(w.next(HQ, 5_000), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h1], "index was behind H2: H1 again");
    // Already selected (first selected object = hit): only camera, index stays (points behind H1, hit H2).
    w.click(Some(h2), false);
    assert_eq!(w.next(HQ, 6_000), GroupOutcome::Camera(w.at(h2)));
    assert_eq!(w.selection.ids, [h2]);
    assert_eq!(w.groups.next_guid_index[&g], index(h1), "index stays");
    // Double tap only if the first selected object has the GUID: unit selected, press within 300 ms -> search.
    assert_eq!(w.next(HQ, 7_000), GroupOutcome::Camera(w.at(h2)));
    w.click(Some(unit), false);
    assert_eq!(w.next(HQ, 7_100), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h2]);
    // Another GUID in between takes the double tap: HQ within 300 ms afterwards continues selecting.
    assert_eq!(w.next(UNIT, 8_000), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [unit]);
    assert_eq!(w.next(HQ, 8_050), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h1]);
    // No GUID of this kind at the local slot: nothing, the selection stays.
    assert_eq!(w.next(9_999, 9_000), GroupOutcome::Nothing);
    assert_eq!(w.selection.ids, [h1]);
    assert!(!w.selection.ids.contains(&foe));
    // Only one object of the kind: every second press moves the camera.
    assert_eq!(w.next(UNIT, 10_000), GroupOutcome::Selected);
    assert_eq!(w.next(UNIT, 11_000), GroupOutcome::Camera(w.at(unit)));
    assert_eq!(w.next(UNIT, 12_000), GroupOutcome::Camera(w.at(unit)));
    assert_eq!(w.selection.ids, [unit]);
}

/// O-314: the search checks no status. A hit that `0x00488F60` rejects (here quartered) clears the selection (and
/// the last group), still reports the new selection and advances the index; the next press selects the next object
/// of the kind.
#[test]
fn rejected_hit_clears_the_selection_and_still_advances() {
    let mut w = World::new();
    let a = w.spawn(UNIT, 0, (2, 2));
    let b = w.spawn(UNIT, 0, (6, 2));
    let h = w.spawn(HQ, 0, (11, 11));
    w.host.step_once();
    w.host.state.object_mut(a, false).unwrap().status |= status::QUARTERED;
    w.select(&[h]);
    w.key(C::StoreGroup(1));
    assert_eq!(w.groups.last, 1);
    let before = (w.hash(), w.host.log.len());
    let g = UNIT as i32;
    assert_eq!(w.next(UNIT, 1_000), GroupOutcome::Selected);
    assert!(w.selection.ids.is_empty(), "rejected -> empty");
    assert_eq!(w.groups.last, -1, "clearing resets the last group");
    assert_eq!(w.groups.next_guid_index[&g], a.index() as u32 + 1);
    // Empty selection: no double tap, but the next object.
    assert_eq!(w.next(UNIT, 1_100), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [b]);
    assert_eq!(w.next(UNIT, 2_000), GroupOutcome::Selected);
    assert!(w.selection.ids.is_empty(), "A again, rejected again");
    assert_eq!(w.groups.next_guid_index[&g], a.index() as u32 + 1);
    // Local: no checksum effect, no command.
    assert_eq!((w.hash(), w.host.log.len()), before);
}

/// The `selectnextguid` indices are in the host part of the savegame per slot; after loading the cycle continues.
/// The double-tap deadline is transient.
#[test]
fn next_guid_index_survives_save_and_load() {
    let (mut w, h1, h2, _, _) = two_hqs();
    assert_eq!(w.next(HQ, 1_000), GroupOutcome::Selected);
    assert_eq!(w.selection.ids, [h1]);
    let mut slots = vec![ControlGroups::default(); PLAYER_SLOTS];
    slots[0] = w.groups.clone();
    slots[5].next_guid_index.insert(700, 17);
    let (mut loaded, groups) = save_and_load(&w, &slots);
    assert_eq!(loaded.groups.next_guid_index, w.groups.next_guid_index);
    assert_eq!(groups[5].next_guid_index[&700], 17, "all 8 slots");
    assert!(loaded.selection.ids.is_empty());
    // Continue behind H1 (also within 300 ms of the old clock: the deadline is not in the savegame).
    assert_eq!(loaded.next(HQ, 1_100), GroupOutcome::Selected);
    assert_eq!(loaded.selection.ids, [h2]);
    // An index above the capacity starts at the front (`start >= cap -> 0`): no reason for rejection.
    loaded.groups.next_guid_index.insert(HQ as i32, 5_000);
    assert_eq!(loaded.next(HQ, 3_000), GroupOutcome::Selected);
    assert_eq!(loaded.selection.ids, [h1]);
}
