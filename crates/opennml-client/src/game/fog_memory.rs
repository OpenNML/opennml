//! Fog memories under the fog (game host, per viewer; re/vision 9).
//!
//! Deviations:
//! - All memories are checked after every step (`SimHost::step_once`, plus player switch, "Fog off" and immediate
//!   commands). In every frame after the ticks, the original checks the movable memories and the fixed ones only in
//!   the sectors of the camera +-1 (`0x0047E560`, `0x00416660`); the reimplementation checks all fixed ones.
//!   On screen, "active" is the same; only snapshot and removal happen earlier outside the camera.
//! - The view-cone test is dropped (the renderer clips on its own).
//! - On a player switch the memories stay in place as in the original.
//! - After loading they are rebuilt.
//! - Movable memories (tents, module `0x2A`) follow the position of their object (`update_movable`,
//!   re/tents 7.3); the view-cone test `0x00415FD0` is dropped there too.
//! - The memories are in ID order, not in the sector/check order of `0x0047E560` (this only affects hitbox hits
//!   at rank 6 of the right-click).
//! - Drawn are the model and the special-class decal, not the shadow decal and the shadow texture (re/vision 9).
//!
//! No deviation: The original (`0x00416230`) sets "active" (`+0x1C1`) without checking "snapshot valid" (`+0x1C2`),
//! but does not draw such a memory (`0x004158A0` and `0x00415F70` check `+0x1C2`); the reimplementation leaves it
//! out in [`FogMemories::active`]. The real object is then invisible (render bit `0x1000`).

use std::collections::BTreeMap;

use opennml_sim::fog::{FogView, MemorySnapshot};
use opennml_sim::id::ObjectId;
use opennml_sim::object::GameObject;
use opennml_sim::rules::MapInfo;
use opennml_sim::state::SimState;

/// A fog memory (`0x1C4` bytes, VTable `0x006163AC`).
#[derive(Debug, Clone, PartialEq)]
pub struct FogMemory {
    /// Last snapshot; `None` as long as the object was never seen (`+0x1C2` = 0).
    pub snapshot: Option<MemorySnapshot>,
    /// Tile of the memory (`+8/+0xC`).
    pub tile: (i32, i32),
    /// World position for the visibility check (`FUN_00415900`).
    pub position: [f32; 3],
    /// Is drawn (`+0x1C1`).
    pub active: bool,
    /// Color index of the object (`GameObject+0x2D0`) at the last check at which it was still alive:
    /// The memory draws via the object renderer with its player color. If the object is missing,
    /// the memory takes over the renderer (`0x00415860`: with no object for the HID, only the memory deletes
    /// it) with the last color set (viewer state, not in the savegame).
    pub color: i32,
    /// Movable memory (module `0x2A`, tents; `+0x18`, `0x00416170`): follows the current position of the
    /// object (re/tents 7.3). Viewer state, not in the savegame.
    pub movable: bool,
    /// "Seen" (`+0x1C0`): 1 for fixed, 0 for movable, until the current position of the object has been
    /// visible once; without that a movable memory never becomes active. Viewer state, not in the savegame:
    /// after loading it is `false` again (the rebuild creates the memories anew, as in the original),
    /// a foreign tent in the fog does not appear until it is seen again.
    pub seen: bool,
}

/// Fog memories of the viewer in ID order.
#[derive(Debug, Clone, Default)]
pub struct FogMemories {
    memories: BTreeMap<ObjectId, FogMemory>,
}

impl FogMemories {
    /// Objects with a fog memory (`GameObject_FinishPlacement` `0x0047048B`...): modules 3, 4, 6,
    /// `0x29`, `0x32` (also when switched off).
    pub fn wants_memory(o: &GameObject) -> bool {
        [3, 4, 6, 0x29, 0x32].iter().any(|&m| o.has_module(m, true))
    }

    fn create(o: &GameObject, snapshot: bool) -> FogMemory {
        // `HasModule(0x2A, 0)` in `GameObject_FinishPlacement` (`0x00470491`) makes the memory movable:
        // only an existing and switched-on tent module (second parameter 0 = without inactive ones).
        let movable = o.has_module(0x2A, false);
        FogMemory {
            snapshot: snapshot.then(|| MemorySnapshot::take(o, o.tile, None)),
            tile: o.tile,
            position: o.world_pos,
            active: false,
            color: o.color,
            movable,
            // Fixed: 1; movable: 0, also pre-placed and with an immediate snapshot.
            seen: !movable,
        }
    }

    /// Rebuild (match start: `all` = pre-placed, every memory immediately with a snapshot; after
    /// loading: snapshot only at a position explored for the viewer, rectangle +-2), then
    /// [`Self::update`].
    pub fn rebuild(
        &mut self,
        state: &SimState,
        map: &MapInfo,
        view: &FogView,
        viewer: u8,
        all: bool,
    ) {
        self.memories.clear();
        let p = i32::from(viewer);
        for o in state.objects().filter(|o| Self::wants_memory(o)) {
            let snap = all || view.world_pos_explored(map, o.world_pos, p, true);
            self.memories.insert(o.id, Self::create(o, snap));
        }
        self.update(state, map, view, viewer);
    }

    /// New object (event `Created`, new ID after a change of owner): create the memory,
    /// snapshot only if the position is visible to the viewer (rectangle +-2).
    pub fn on_created(
        &mut self,
        state: &SimState,
        map: &MapInfo,
        view: &FogView,
        viewer: u8,
        id: ObjectId,
    ) {
        let Some(o) = state.object(id, false) else {
            return;
        };
        if !Self::wants_memory(o) {
            return;
        }
        let snap = view.world_pos_visible(map, o.world_pos, i32::from(viewer), true);
        self.memories.insert(id, Self::create(o, snap));
    }

    /// `FUN_00416230` per memory (after every step, see module header); movable memories (tents) go to
    /// [`Self::update_movable`]. Fixed: position visible (rectangle +-2, without view switch) -> new
    /// snapshot, or remove the memory if the object is missing; then active if the position is explored
    /// (rectangle +-2) and not visible without the rectangle (with view switch).
    pub fn update(&mut self, state: &SimState, map: &MapInfo, view: &FogView, viewer: u8) {
        let p = i32::from(viewer);
        self.memories.retain(|&id, m| {
            m.active = false;
            // Player color of the renderer while the object lives (see `FogMemory::color`).
            if let Some(o) = state.object(id, false) {
                m.color = o.color;
            }
            if m.movable {
                return Self::update_movable(m, state.object(id, false), map, view, p);
            }
            if view.world_pos_visible(map, m.position, p, true) {
                let Some(o) = state.object(id, false) else {
                    return false;
                };
                m.snapshot = Some(MemorySnapshot::take(o, m.tile, m.snapshot.as_ref()));
                m.position = o.world_pos;
            }
            if view.world_pos_explored(map, m.position, p, true)
                && !view.world_pos_visible(map, m.position, p, false)
            {
                m.active = true;
            }
            true
        });
    }

    /// `FUN_00416230`, movable branch from `0x00416249` (re/tents 7.3, module `0x2A`); returns
    /// whether the memory stays (`retain`).
    ///
    /// The visibility check applies to the **current** position of the object, the drawing to the remembered
    /// one (`position`): A tent moved unobserved leaves its memory at the old place until
    /// the new position becomes visible. Without a first sighting (`seen`) the memory never becomes active.
    fn update_movable(
        m: &mut FogMemory,
        obj: Option<&GameObject>,
        map: &MapInfo,
        view: &FogView,
        p: i32,
    ) -> bool {
        let pos = match obj {
            Some(o) => {
                if view.world_pos_visible(map, o.world_pos, p, true) {
                    // `0x00415920` also remembers the world position (`+0x64`).
                    m.snapshot = Some(MemorySnapshot::take(o, o.tile, m.snapshot.as_ref()));
                    m.tile = o.tile;
                    m.position = o.world_pos;
                    m.seen = true;
                }
                o.world_pos
            }
            None => m.position,
        };
        // `0x004162DB`: never seen -> never active. This also holds if the object is missing and the remembered
        // position is visible: A never-seen memory is not removed but stays (not active).
        if !m.seen {
            return true;
        }
        if view.world_pos_visible(map, pos, p, true) {
            // Missing object at a visible position: remove the memory (`0x0047E4C0`, as with fixed ones).
            // With an object, the original continues with the comparison `tile != tile` (`FUN_00415A90`, dead: `tile`
            // was just set above, `seen` would stay 1) and a second snapshot with the same arguments as the first;
            // both are dropped.
            if obj.is_none() {
                return false;
            }
        }
        if view.world_pos_explored(map, m.position, p, true)
            && !view.world_pos_visible(map, m.position, p, false)
        {
            m.active = true;
        }
        true
    }

    pub fn get(&self, id: ObjectId) -> Option<&FogMemory> {
        self.memories.get(&id)
    }

    /// All memories with their ID in ID order (memory points of the minimap).
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &FogMemory)> {
        self.memories.iter().map(|(&id, m)| (id, m))
    }

    /// Active memories with a snapshot (to be drawn, right-click at the last rank).
    pub fn active(&self) -> impl Iterator<Item = &FogMemory> {
        self.memories
            .values()
            .filter(|m| m.active && m.snapshot.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_sim::rules::Rules;
    use opennml_sim::sim::Sim;
    use opennml_sim::tick::step;
    use opennml_sim::world::{ObjectDesc, create_object, desc_flags};

    const TYPES: &str = concat!(
        "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\n",
        "VALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\n",
        "VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
        "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n",
    );

    fn spawn(
        state: &mut SimState,
        rules: &Rules,
        guid: u32,
        slot: i32,
        tile: (i32, i32),
    ) -> ObjectId {
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]);
        desc.flags |= desc_flags::PLACED;
        create_object(&mut Sim::new(state, rules), &mut desc).unwrap()
    }

    // QUIRK D-222: kept
    #[test]
    fn memories_show_what_was_seen_until_the_place_is_seen_again() {
        let mut rules = Rules::synthetic(TYPES, 64);
        rules
            .defaults
            .insert(1100, [(16, 183), (18, 32)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let tree = spawn(&mut state, &rules, 3602, 9, (40, 40));
        spawn(&mut state, &rules, 1100, 0, (10, 10));
        let map = rules.map;
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &map, &FogView::new(&state), 0, true);
        assert!(
            mem.get(tree).unwrap().snapshot.is_some(),
            "pre-placed: snapshot immediately"
        );
        for _ in 0..3 {
            step(&mut state, &rules, &[]);
        }
        mem.update(&state, &map, &FogView::new(&state), 0);
        assert!(!mem.get(tree).unwrap().active, "unexplored");
        // Explored, not seen: active.
        state.fog.explored.or(40, 40, 1);
        mem.update(&state, &map, &FogView::new(&state), 0);
        assert_eq!(mem.active().count(), 1);
        // Color change in the fog: the memory takes along the color of the living object (renderer).
        assert_eq!(mem.get(tree).unwrap().color, 9);
        state.object_mut(tree, false).unwrap().color = 3;
        mem.update(&state, &map, &FogView::new(&state), 0);
        assert_eq!(mem.get(tree).unwrap().color, 3);
        // The tree vanishes in the fog: the memory stays, with the last color set.
        opennml_sim::world::queue_destroy(&mut Sim::new(&mut state, &rules), tree);
        for _ in 0..6 {
            step(&mut state, &rules, &[]);
        }
        state.fog.explored.or(40, 40, 1);
        mem.update(&state, &map, &FogView::new(&state), 0);
        assert!(state.object(tree, false).is_none() && mem.get(tree).unwrap().active);
        assert_eq!(mem.get(tree).unwrap().color, 3);
        // Seen again: the object is missing, the memory is removed.
        state.fog.seen.or(40, 40, 1);
        mem.update(&state, &map, &FogView::new(&state), 0);
        assert!(mem.get(tree).is_none());
    }

    #[test]
    fn units_get_no_memory_and_new_objects_wait_for_sight() {
        let rules = Rules::synthetic(TYPES, 64);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let unit = spawn(&mut state, &rules, 1100, 1, (20, 20));
        let tree = spawn(&mut state, &rules, 3602, 9, (30, 30));
        let mut mem = FogMemories::default();
        mem.on_created(&state, &rules.map, &FogView::new(&state), 0, unit);
        mem.on_created(&state, &rules.map, &FogView::new(&state), 0, tree);
        assert!(mem.get(unit).is_none());
        assert!(
            mem.get(tree).unwrap().snapshot.is_none(),
            "not seen: no snapshot yet"
        );
        state.fog.seen.or(31, 31, 1);
        mem.update(&state, &rules.map, &FogView::new(&state), 0);
        assert!(
            mem.get(tree).unwrap().snapshot.is_some(),
            "rectangle +-2 seen"
        );
    }

    // --- Movable memories of tents (re/tents 7.3). Types from the shared viewer test world:
    // `testworld::rules()` has 1601 with modules 3, 40, 42 (`VALUEID 430001`);
    // the map without a host suffices here, hence a small world of its own on top of it.

    use crate::game::testworld::{self, DWELLING_TENT};

    /// Map 64 x 64 without fog knowledge; player 0 is the viewer.
    fn tent_world() -> (Rules, SimState) {
        let rules = testworld::rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        (rules, state)
    }

    /// Center of the tile (like `spawn`).
    fn tile_center(tile: (i32, i32)) -> [f32; 3] {
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        [at(tile.0), 0.0, at(tile.1)]
    }

    /// Moves the tent unobserved (unpacking at another place): position and tile, no sim.
    fn relocate(state: &mut SimState, id: ObjectId, tile: (i32, i32)) {
        let o = state.object_mut(id, false).unwrap();
        o.tile = tile;
        o.world_pos = tile_center(tile);
    }

    fn update(mem: &mut FogMemories, state: &SimState, rules: &Rules) {
        mem.update(state, &rules.map, &FogView::new(state), 0);
    }

    // QUIRK D-229: kept
    #[test]
    fn movable_memories_need_a_first_sighting() {
        let (rules, mut state) = tent_world();
        let tent = spawn(&mut state, &rules, DWELLING_TENT, 1, (40, 40));
        state.fog.explored.or(40, 40, 1);
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        let m = mem.get(tent).unwrap();
        assert!(m.movable && !m.seen, "movable, never seen");
        assert!(m.snapshot.is_some(), "pre-placed: snapshot immediately");
        assert!(
            !m.active && mem.active().count() == 0,
            "explored, not visible: never seen, never active"
        );
        // See once: not yet active (the position is visible), after looking away it is.
        state.fog.seen.or(40, 40, 1);
        update(&mut mem, &state, &rules);
        let m = mem.get(tent).unwrap();
        assert!(m.seen && !m.active);
        state.fog.seen.clear();
        update(&mut mem, &state, &rules);
        assert!(mem.get(tent).unwrap().active);
        assert_eq!(mem.active().count(), 1);
    }

    /// `HasModule(0x2A, 0)` (`0x00470491`) counts only a switched-on tent module: A tent with a
    /// switched-off module `0x2A` gets a fixed memory.
    #[test]
    fn a_tent_with_a_disabled_tent_module_gets_a_fixed_memory() {
        let (rules, mut state) = tent_world();
        let tent = spawn(&mut state, &rules, DWELLING_TENT, 1, (40, 40));
        state
            .object_mut(tent, false)
            .unwrap()
            .modules
            .get_mut(0x2A)
            .and_then(Option::as_mut)
            .expect("tent module")
            .base
            .enabled = false;
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        let m = mem.get(tent).unwrap();
        assert!(!m.movable && m.seen, "fixed like a building");
    }

    #[test]
    fn fixed_memories_start_as_seen() {
        let mut rules = Rules::synthetic(TYPES, 64);
        rules.map = testworld::rules().map;
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let tree = spawn(&mut state, &rules, 3602, 9, (40, 40));
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        let m = mem.get(tree).unwrap();
        assert!(!m.movable && m.seen);
    }

    // QUIRK D-229: kept
    #[test]
    fn a_moved_tent_memory_stays_until_the_new_place_is_seen() {
        let (rules, mut state) = tent_world();
        let (old, new) = ((20, 20), (50, 50));
        let tent = spawn(&mut state, &rules, DWELLING_TENT, 1, old);
        state.fog.explored.or(old.0, old.1, 1);
        state.fog.seen.or(old.0, old.1, 1);
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        assert!(mem.get(tent).unwrap().seen, "seen at the position");
        // Look away, move unobserved: memory at the old place, active.
        state.fog.seen.clear();
        relocate(&mut state, tent, new);
        update(&mut mem, &state, &rules);
        let m = mem.get(tent).unwrap();
        assert!(m.active && m.tile == old && m.position == tile_center(old));
        // The old place becomes visible: the memory stays, it is just not active there.
        state.fog.seen.or(old.0, old.1, 1);
        update(&mut mem, &state, &rules);
        let m = mem.get(tent).unwrap();
        assert!(!m.active && m.tile == old && m.position == tile_center(old));
        assert_eq!(m.snapshot.as_ref().unwrap().world_pos, tile_center(old));
        // The new position becomes visible: the memory moves.
        state.fog.explored.or(new.0, new.1, 1);
        state.fog.seen.clear();
        state.fog.seen.or(new.0, new.1, 1);
        update(&mut mem, &state, &rules);
        let m = mem.get(tent).unwrap();
        assert!(m.seen && !m.active && m.tile == new && m.position == tile_center(new));
        assert_eq!(m.snapshot.as_ref().unwrap().world_pos, tile_center(new));
        state.fog.seen.clear();
        update(&mut mem, &state, &rules);
        assert!(mem.get(tent).unwrap().active, "now active at the new place");
    }

    // QUIRK D-229: kept
    #[test]
    fn a_packed_tent_memory_draws_nothing() {
        let (rules, mut state) = tent_world();
        let tent = spawn(&mut state, &rules, DWELLING_TENT, 1, (30, 30));
        state.fog.explored.or(30, 30, 1);
        state.fog.seen.or(30, 30, 1);
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        let bits = |mem: &FogMemories| {
            mem.get(tent)
                .unwrap()
                .snapshot
                .as_ref()
                .unwrap()
                .render_bits
        };
        assert_ne!(bits(&mem), 0, "set up: the model is drawn");
        // Packed (render bits 0) and seen: the snapshot has no model.
        state.object_mut(tent, false).unwrap().render_bits = 0;
        update(&mut mem, &state, &rules);
        assert_eq!(bits(&mem), 0);
        state.fog.seen.clear();
        update(&mut mem, &state, &rules);
        assert!(
            mem.get(tent).unwrap().active,
            "the memory is active but draws nothing"
        );
        assert_eq!(bits(&mem), 0);
    }

    // QUIRK D-229: kept
    #[test]
    fn a_vanished_tent_memory_disappears_when_its_place_is_seen() {
        let (rules, mut state) = tent_world();
        let seen_once = spawn(&mut state, &rules, DWELLING_TENT, 1, (20, 20));
        let never_seen = spawn(&mut state, &rules, DWELLING_TENT, 1, (45, 45));
        for t in [20, 45] {
            state.fog.explored.or(t, t, 1);
        }
        // Only the first tent is seen once (the test establishes sight beforehand).
        state.fog.seen.or(20, 20, 1);
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, true);
        assert!(mem.get(seen_once).unwrap().seen);
        assert!(!mem.get(never_seen).unwrap().seen);
        state.fog.seen.clear();
        for id in [seen_once, never_seen] {
            opennml_sim::world::queue_destroy(&mut Sim::new(&mut state, &rules), id);
        }
        for _ in 0..8 {
            step(&mut state, &rules, &[]);
        }
        assert!(
            state.object(seen_once, false).is_none() && state.object(never_seen, false).is_none()
        );
        // In the fog both memories stay; the seen one is active, the never-seen one is not.
        update(&mut mem, &state, &rules);
        assert!(mem.get(seen_once).unwrap().active);
        assert!(!mem.get(never_seen).unwrap().active);
        // Both places become visible: the seen memory disappears. The never-seen one stays
        // (re/tents 7.3, pseudocode `0x004162DB`: `if !seen { return }` comes before the
        // removal check, and without an object nothing sets `seen`); it is just not active there.
        state.fog.seen.or(20, 20, 1);
        state.fog.seen.or(45, 45, 1);
        update(&mut mem, &state, &rules);
        assert!(mem.get(seen_once).is_none());
        let m = mem.get(never_seen).unwrap();
        assert!(!m.seen && !m.active);
    }

    /// Own tents see themselves; still, a tent just set up at another place must not become unselectable
    /// as long as its memory still stands at the old, explored place
    /// (`SimHost::fog_selectable` reads the tile of the memory, `fog_view::visible_for_selection`).
    // QUIRK D-229: kept
    #[test]
    fn a_tent_set_up_at_a_new_place_stays_selectable() {
        use crate::game::fog_view::visible_for_selection;
        let (rules, mut state) = tent_world();
        let (old, new) = ((20, 20), (50, 50));
        let tent = spawn(&mut state, &rules, DWELLING_TENT, 0, old);
        state.fog.explored.or(old.0, old.1, 1);
        state.fog.seen.or(old.0, old.1, 1);
        let mut mem = FogMemories::default();
        mem.rebuild(&state, &rules.map, &FogView::new(&state), 0, false);
        // Set up at a position the fog does not know yet (the vision source follows only in the next
        // fog cycle).
        relocate(&mut state, tent, new);
        update(&mut mem, &state, &rules);
        let view = FogView::new(&state);
        let o = state.object(tent, false).unwrap();
        let tile = mem.get(tent).map(|m| m.tile);
        assert_eq!(
            tile,
            Some(old),
            "the memory stays until the new position is visible"
        );
        assert!(visible_for_selection(&view, o, 0, tile));
        // Counter-check: with the tile of the new, unexplored position it would not be selectable.
        assert!(!visible_for_selection(&view, o, 0, Some(new)));
        // As soon as the new position is visible, the memory follows and stays selectable.
        state.fog.explored.or(new.0, new.1, 1);
        state.fog.seen.or(new.0, new.1, 1);
        update(&mut mem, &state, &rules);
        let view = FogView::new(&state);
        let tile = mem.get(tent).map(|m| m.tile);
        assert_eq!(tile, Some(new));
        let o = state.object(tent, false).unwrap();
        assert!(visible_for_selection(&view, o, 0, tile));
    }
}
