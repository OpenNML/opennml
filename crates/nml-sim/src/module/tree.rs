//! Module 4 `CPropertyTree` (re/occupancy 1, re/gathering): trees snap to the
//! tile center and lock their tile in layers 0, 1 and 4 (forest layers 2/3 stay
//! free) together with the occupancy bit. Lumberjacks register at two slots and take wood; an empty
//! tree falls in 16 object ticks of the extra tick list, then frees layers 0-3 and the occupancy bit
//! and is destroyed. Swaying, fall angle and fall duration are presentation.

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::footprint::{mark_tile_for_layers, snap_and_block_footprint};
use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;
use crate::state::SimEvent;
use crate::world::{add_extra_tick, queue_destroy, remove_extra_tick};

use super::ModuleKind;

pub const INDEX: usize = 4;
/// Wood supply (attribute `0x19`).
pub const WOOD: u32 = 0x19;
/// Module flag "slot free": the tree is visible to the lumberjack search.
pub const SEARCHABLE: u32 = 0x100;
/// Sound `SFX_TREEFALL`: the original resolves the name at run time via
/// `data/sfx/otherevents.ini` (shipped data: 1176) and plays it with `0x32` and a 3D position.
pub const SFX_TREEFALL: i32 = 1176;
/// Half the distance of the registration slots to the tree center (`[0x00672DC8]` = fixed-point 1/4 tile).
const REG_OFFSET: i32 = 0x400;

/// Queries of the tree (`PropertyTree_Query` `0x00532290`).
pub mod msg {
    /// `{n}` -> wood taken (`PMSG_TREE_GETWOOD`).
    pub const GET_WOOD: u32 = 0x104;
    /// `{x, z}` -> center of the neighboring tile toward the requester (`PMSG_TREE_GETNEARESTPOS`).
    pub const GET_NEAREST_POS: u32 = 0x204;
    /// `{_, hid}` -> `{x, z}` of the registration slot, return value slot 0/1 or -1 (`PMSG_TREE_REGPOS`).
    pub const REGISTER: u32 = 0x304;
    /// `{hid}` -> unregister, 1 or 0 (`PMSG_TREE_UNREGPOS`).
    pub const UNREGISTER: u32 = 0x404;
    /// Set the occupancy rectangle (dead field, no sender).
    pub const SET_OCCUPANCY_RECT: u32 = 0x504;
}

/// State `m_Status` (`+0x88`).
pub mod state {
    /// Standing.
    pub const STANDING: i32 = 0;
    /// Falling (in the extra tick list).
    pub const FALLING: i32 = 1;
}

/// State of `CPropertyTree` (`PropertyTree_Construct` `0x00531BB0`). Not tracked are the
/// fall angle `m_Angle` (`+0x8C`), the sway direction `delta` (`+0x84`) and `m_FallingDuration`
/// (`+0xA8`): only the frame update reads them (the presentation calculates the fall duration from
/// `delta_time`); the rectangle `+0x98` of query `0x504` is never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    /// `m_WaldFlag` (50001).
    pub wald_flag: i32,
    /// `m_WaldSize` (50002).
    pub wald_size: i32,
    /// `m_RotCount` (`+0x78`): start value of the swaying in [-50, 50] (presentation).
    pub rot_count: i32,
    /// `m_RegPos` (`+0x7C/+0x80`): registered lumberjacks per slot.
    pub reg_pos: [ObjectId; 2],
    /// `m_Status` (`+0x88`): 0 standing, 1 falling (the original never sets 2 "growing").
    pub status: i32,
    /// `m_deltaTime` (`+0x90`): fall time in ms, + 100 per tree tick.
    pub delta_time: i32,
    /// `m_IsTreeBlocked`: the tile is locked.
    pub blocked: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            wald_flag: 0,
            wald_size: 0,
            rot_count: 0,
            reg_pos: [ObjectId::NONE; 2],
            status: state::STANDING,
            delta_time: 0,
            blocked: false,
        }
    }
}

fn tree_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Tree> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Tree(t) => Some(t),
        _ => None,
    }
}

fn tree<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Tree> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Tree(t) => Some(t),
        _ => None,
    }
}

/// `m_RotCount` from a CRT `rand()` value: `-50 - ftol(r * (1/32767) * (-100.0))` (x87 with
/// `double` constants, `_ftol2` truncates).
pub fn rot_count(r: i32) -> i32 {
    -50 - (f64::from(r) * (1.0 / 32767.0) * -100.0) as i32
}

/// `PropertyTree_Construct` (`0x00531BB0`, CRT `rand()` at `0x00531BCF`) and `PropertyTree_Init`
/// (`0x00531C30`): the constructor draws exactly one value from the shared CRT state (like the
/// animation choice of stand and walk; the value itself only affects the swaying); init sets the
/// flags `50003 | 0x104`, snaps and locks (mode 1, layer `0x13`, occupancy bit), fills the
/// wood supply and tracks it as the current value. When loading a savegame the original draws
/// again (constructor before reading in); here the saved CRT state applies.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let r = sim.state.crt.rand();
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let (wald_flag, wald_size) = (value(50001), value(50002));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = value(50003) as u32 | 0x104;
        if let ModuleKind::Tree(t) = &mut m.kind {
            t.rot_count = rot_count(r);
            t.wald_flag = wald_flag;
            t.wald_size = wald_size;
        }
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 3;
    }
    snap_and_block_footprint(sim, id, 1, 0x13, true, true, true);
    if let Some(t) = tree_mut(sim, id) {
        t.blocked = true;
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.fill_current(WOOD);
        o.attributes.set_tracks_current(WOOD, true);
    }
}

/// `PropertyTree_Query` (`0x00532290`, slot 7); unknown codes -1. `OnMessage` (slot 12) and
/// abort (slot 11) are the base (`true`): command wrappers sent to trees fizzle.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::GET_WOOD => get_wood(sim, id, m.arg(0)),
        msg::GET_NEAREST_POS => get_nearest_pos(sim, id, m),
        msg::REGISTER => register_lumberjack(sim, id, m),
        msg::UNREGISTER => unregister_lumberjack(sim, id, ObjectId::from_i32(m.arg(0))),
        // `PropertyTree_SetOccupancyRect` (`0x00531EA0`): sets the never-written rectangle
        // `+0x98` - no sender in the original, has no effect here.
        msg::SET_OCCUPANCY_RECT => 1,
        _ => -1,
    }
}

/// Take wood (`0x104 {n}`, `0x005322E5...`): at most the remainder (attribute `0x19`,
/// `AttributeSet_AddToCurrentValue` `0x0048FAF0`). If the standing tree becomes empty, it falls: status 1,
/// fall time 0, into the extra tick list (`GameWorld_AddExtraTickObject` `0x0047E460`), sound
/// `SFX_TREEFALL`. **Quirk:** the flags are reduced with `& 0x100` instead of `& !0x100` - if at
/// most one lumberjack was registered (bit `0x100` set), the falling tree stays searchable.
/// Leaving the selection (`0x004801B0`) is UI.
fn get_wood(sim: &mut Sim, id: ObjectId, want: i32) -> i32 {
    let Some(o) = sim.object(id) else { return -1 };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, WOOD);
    let current = o.attributes.current_value(WOOD, p);
    let n = if current < want { current } else { want };
    let Some(o) = sim.object_mut(id) else {
        return -1;
    };
    o.attributes.add_to_current_value(WOOD, n.wrapping_neg(), p);
    let empty = o.attributes.current_value(WOOD, p) == 0;
    if empty && tree(sim, id).is_some_and(|t| t.status == state::STANDING) {
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.flags &= SEARCHABLE;
        }
        if let Some(t) = tree_mut(sim, id) {
            t.status = state::FALLING;
            t.delta_time = 0;
        }
        add_extra_tick(sim, id);
        sim.state.events.push(SimEvent::Sound {
            object: id,
            sound: SFX_TREEFALL,
        });
    }
    n
}

/// `PropertyTree_GetNearestPos` (`0x00531CC0`): tile of the requester (`Fixed_ToInt` = `>> 12`) pulled
/// per axis toward the neighboring tile of the tree (equal stays equal), tile center written back into
/// the arguments. Return value 0.
fn get_nearest_pos(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let Some(o) = sim.object(id) else { return -1 };
    let (tx, tz) = (o.grid.x.0 >> 12, o.grid.z.0 >> 12);
    let toward = |p: i32, t: i32| {
        if p < t {
            t - 1
        } else if p > t {
            t + 1
        } else {
            p
        }
    };
    let px = toward(m.arg(0) >> 12, tx);
    let pz = toward(m.arg(1) >> 12, tz);
    m.set_arg(0, (px << 12).wrapping_add(0x800));
    m.set_arg(1, (pz << 12).wrapping_add(0x800));
    0
}

/// `PropertyTree_RegisterLumberjack` (`0x00531FB0`, `0x304 {_, hid}`): first slot that is free
/// or already belongs to this ID; slot 0 lies 1/4 tile at the bottom left, slot 1 at the top right
/// (response in `{x, z}`). If both are occupied, the tree leaves the search (`& !0x100`). No slot
/// -> -1. **Quirk:** if the ID is already in slot 1 and slot 0 is free, it also occupies
/// slot 0 - the tree counts as full, unregistering clears only slot 0.
fn register_lumberjack(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let hid = ObjectId::from_i32(m.arg(1));
    let Some(grid) = sim.object(id).map(|o| o.grid) else {
        return -1;
    };
    let Some(t) = tree_mut(sim, id) else {
        return -1;
    };
    let Some(i) = (0..2).find(|&i| t.reg_pos[i].is_none() || t.reg_pos[i] == hid) else {
        return -1;
    };
    t.reg_pos[i] = hid;
    let full = t.reg_pos.iter().all(|r| !r.is_none());
    let (x, z) = if i == 0 {
        (
            grid.x.0.wrapping_sub(REG_OFFSET),
            grid.z.0.wrapping_sub(REG_OFFSET),
        )
    } else {
        (
            grid.x.0.wrapping_add(REG_OFFSET),
            grid.z.0.wrapping_add(REG_OFFSET),
        )
    };
    m.set_arg(0, x);
    m.set_arg(1, z);
    if full && let Some(md) = sim.module_mut(id, INDEX) {
        md.base.flags &= !SEARCHABLE;
    }
    i as i32
}

/// `PropertyTree_UnregisterLumberjack` (`0x00531D60`, `0x404 {hid}`): free the first slot of this
/// ID; a standing tree becomes searchable again (`| 0x104`, **quirk:** even if
/// the other slot stays occupied), a falling one does not. Return value 1, 0 without a slot.
fn unregister_lumberjack(sim: &mut Sim, id: ObjectId, hid: ObjectId) -> i32 {
    let Some(t) = tree_mut(sim, id) else {
        return 0;
    };
    let Some(i) = (0..2).find(|&i| t.reg_pos[i] == hid) else {
        return 0;
    };
    t.reg_pos[i] = ObjectId::NONE;
    let standing = t.status == state::STANDING;
    if standing && let Some(md) = sim.module_mut(id, INDEX) {
        md.base.flags |= 0x104;
    }
    1
}

/// `PropertyTree_Tick` (`0x005320C0`, slot 13; `dt` unused): reached only via the extra tick list
/// (trees, slot 9, are in no tick group), so once per 100 ms step in the
/// phase 0 call. Falling + 100 ms (fixed); `1500.0 < t` -> `OnFallen` in the 16th tick (t = 1600).
/// The "growing" branch (status 2) is dead.
pub fn tick(sim: &mut Sim, id: ObjectId) {
    let Some(t) = tree_mut(sim, id) else { return };
    if t.status != state::FALLING {
        return;
    }
    t.delta_time = t.delta_time.wrapping_add(100);
    if 1500.0f32 < t.delta_time as f32 {
        on_fallen(sim, id);
    }
}

/// `PropertyTree_OnFallen` (`0x00531F70`): status 0, tile free (`PropertyTree_UnblockTile`
/// `0x00531EE0`: path layers 0-3, mask `0x0F`, then terrain again - layer 4 stays -, occupancy bit
/// off, clearing), queue for destruction (`GameWorld_QueueObjectDestroy` `0x0047C7D0`, still executed in the
/// same phase 0 call) and out of the extra tick list.
fn on_fallen(sim: &mut Sim, id: ObjectId) {
    if let Some(t) = tree_mut(sim, id) {
        t.status = state::STANDING;
        t.blocked = false;
    }
    if let Some((tx, tz)) = sim.object(id).map(|o| (o.grid.x.0 >> 12, o.grid.z.0 >> 12)) {
        mark_tile_for_layers(sim, 0x0F, tx, tz, false);
        sim.state.occupancy.set(tx, tz, false);
        sim.state.path.flush_dirty_rect();
    }
    queue_destroy(sim, id);
    remove_extra_tick(sim, id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::send_mut;
    use crate::path::BLOCKED;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::tick::step;
    use crate::world::{ObjectDesc, create_object};
    use nml_formats::targets::MatchFilter;

    const TYPES: &str =
        "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n";

    fn rules(wood: i32) -> Rules {
        let mut rules = Rules::synthetic(TYPES, 32);
        rules
            .defaults
            .insert(3602, [(25, wood)].into_iter().collect());
        rules
    }

    /// Tree at the center of tile `(x, z)`.
    fn plant(sim: &mut Sim, x: i32, z: i32) -> ObjectId {
        let pos = [(x as f32 + 0.5) * 128.0, 0.0, (z as f32 + 0.5) * 128.0];
        create_object(sim, &mut ObjectDesc::new(3602, 9, pos)).unwrap()
    }

    fn ask(sim: &mut Sim, id: ObjectId, code: u32, args: &[i32]) -> (i32, Vec<i32>) {
        let mut m = Message::new(code, args);
        let r = send_mut(sim, id, &mut m);
        (r, m.args)
    }

    fn flags(sim: &Sim, id: ObjectId) -> u32 {
        sim.module(id, INDEX).unwrap().base.flags
    }

    #[test]
    fn trees_snap_block_three_layers_and_a_second_tree_on_the_tile_is_dropped() {
        let rules = rules(400);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tree = create_object(
            &mut sim,
            &mut ObjectDesc::new(3602, 9, [1000.0, 0.0, 300.0]),
        )
        .unwrap();
        let o = sim.object(tree).unwrap();
        assert_eq!((o.world_pos[0], o.world_pos[2]), (960.0, 320.0));
        assert_eq!(o.attributes.current_value(WOOD, 0), 400);
        let m = o.module(INDEX).unwrap();
        assert_eq!(m.base.flags, 0x104);
        assert!(matches!(&m.kind, ModuleKind::Tree(t) if t.blocked));
        let cell = |l: usize| sim.state.path.layers[l].cell(7, 2);
        assert_eq!([0, 1, 2, 3, 4].map(cell), [BLOCKED, BLOCKED, 0, 0, BLOCKED]);
        assert!(sim.state.occupancy.test(7, 2));
        // Second tree on the same tile (layer 0 locked): not created.
        assert!(
            create_object(
                &mut sim,
                &mut ObjectDesc::new(3602, 9, [1020.0, 0.0, 310.0])
            )
            .is_none()
        );
        assert_eq!(sim.state.tables[9].count, 1);
    }

    // QUIRK W-164: deviation
    #[test]
    fn the_constructor_draws_one_crt_value_for_the_sway() {
        assert_eq!([0, 16383, 32767].map(rot_count), [-50, -1, 50]);
        let rules = rules(200);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut expected = state.crt;
        let mut sim = Sim::new(&mut state, &rules);
        let a = plant(&mut sim, 3, 3);
        let b = plant(&mut sim, 5, 3);
        for t in [a, b] {
            let want = rot_count(expected.rand());
            assert_eq!(tree(&sim, t).unwrap().rot_count, want);
        }
        assert_eq!(sim.state.crt, expected);
        let t = tree(&sim, a).unwrap();
        assert_eq!((t.reg_pos, t.status), ([ObjectId::NONE; 2], 0));
    }

    // QUIRK W-31: kept
    #[test]
    fn lumberjacks_register_on_opposite_corners_and_fill_the_tree() {
        let rules = rules(200);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let t = plant(&mut sim, 10, 20);
        let (a, b, c) = (0x1_0001, 0x2_0001, 0x3_0001);
        // Slot 0 at the bottom left, slot 1 at the top right, each 1/4 tile from the center.
        assert_eq!(
            ask(&mut sim, t, msg::REGISTER, &[0, a]),
            (0, vec![0xA400, 0x14400])
        );
        assert_eq!(flags(&sim, t), 0x104);
        // The same ID gets slot 0 again.
        assert_eq!(ask(&mut sim, t, msg::REGISTER, &[0, a]).0, 0);
        assert_eq!(
            ask(&mut sim, t, msg::REGISTER, &[0, b]),
            (1, vec![0xAC00, 0x14C00])
        );
        // Full: out of the search, a third one gets -1 (arguments unchanged).
        assert_eq!(flags(&sim, t), 0x004);
        assert_eq!(ask(&mut sim, t, msg::REGISTER, &[7, c]), (-1, vec![7, c]));
        // Unregister: 1, searchable again (quirk: although b is still registered); unknown 0.
        assert_eq!(ask(&mut sim, t, msg::UNREGISTER, &[a]).0, 1);
        assert_eq!(flags(&sim, t), 0x104);
        assert_eq!(ask(&mut sim, t, msg::UNREGISTER, &[a]).0, 0);
        // Quirk double registration: b is in slot 1, slot 0 is free -> b also occupies slot 0.
        assert_eq!(ask(&mut sim, t, msg::REGISTER, &[0, b]).0, 0);
        assert_eq!(tree(&sim, t).unwrap().reg_pos, [ObjectId(b as u32); 2]);
        assert_eq!(flags(&sim, t), 0x004);
        assert_eq!(ask(&mut sim, t, msg::UNREGISTER, &[b]).0, 1);
        assert_eq!(
            tree(&sim, t).unwrap().reg_pos,
            [ObjectId::NONE, ObjectId(b as u32)]
        );
        // Neighboring tile toward the requester at (5.3; 20.9): tile (9, 20), center.
        let (px, pz) = ((5.3f64 * 4096.0) as i32, (20.9f64 * 4096.0) as i32);
        assert_eq!(
            ask(&mut sim, t, msg::GET_NEAREST_POS, &[px, pz]),
            (0, vec![0x9800, 0x14800])
        );
        // On the same tile the tile stays; at the top right the neighboring tile (11, 21).
        assert_eq!(
            ask(&mut sim, t, msg::GET_NEAREST_POS, &[0xA100, 0x16000]).1,
            vec![0xA800, 0x15800]
        );
        // Dead field `0x504` answers 1 without effect; unknown -1.
        assert_eq!(ask(&mut sim, t, msg::SET_OCCUPANCY_RECT, &[]).0, 1);
        assert_eq!(ask(&mut sim, t, 0x604, &[]).0, -1);
    }

    // QUIRK W-28, W-29: kept
    #[test]
    fn an_empty_tree_falls_in_sixteen_tree_ticks_and_frees_layers_zero_to_three() {
        let rules = rules(200);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let (t, other) = {
            let mut sim = Sim::new(&mut state, &rules);
            (plant(&mut sim, 10, 20), plant(&mut sim, 12, 20))
        };
        // First step: phase 1 (group 0); the next one is a phase 0 call.
        step(&mut state, &rules, &[]);
        let mut sim = Sim::new(&mut state, &rules);
        // At most the remainder.
        assert_eq!(ask(&mut sim, t, msg::GET_WOOD, &[150]).0, 150);
        assert_eq!(ask(&mut sim, t, msg::GET_WOOD, &[80]).0, 50);
        let tr = tree(&sim, t).unwrap();
        assert_eq!((tr.status, tr.delta_time), (state::FALLING, 0));
        // Quirk `& 0x100`: without full registration only "slot free" remains.
        assert_eq!(flags(&sim, t), SEARCHABLE);
        assert_eq!(sim.state.extra_tick, [t]);
        assert_eq!(
            sim.state.events.last(),
            Some(&SimEvent::Sound {
                object: t,
                sound: SFX_TREEFALL
            })
        );
        // Empty stays empty, does not fall a second time.
        let events = sim.state.events.len();
        assert_eq!(ask(&mut sim, t, msg::GET_WOOD, &[5]).0, 0);
        assert_eq!(sim.state.events.len(), events);
        // Unregistering does not give a falling tree `0x104` back.
        assert_eq!(ask(&mut sim, t, msg::REGISTER, &[0, 0x1_0001]).0, 0);
        assert_eq!(ask(&mut sim, t, msg::UNREGISTER, &[0x1_0001]).0, 1);
        assert_eq!(flags(&sim, t), SEARCHABLE);
        // Quirk: the falling tree (wood 0) stays visible to the lumberjack search.
        let search = MatchFilter {
            mode: 0x1012,
            match_bits: 0x104,
            ..MatchFilter::constructed()
        };
        let found = crate::collision::find_nearest_matching(
            &mut sim,
            (0xA000, 0x14000),
            &search,
            -1,
            9,
            -1,
        );
        assert_eq!(found, Some(t));
        // Steps 2 ... 31: 15 tree ticks (phase 0 steps 2, 4, ..., 30), still there, tile locked.
        for _ in 0..30 {
            step(&mut state, &rules, &[]);
        }
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(tree(&sim, t).unwrap().delta_time, 1500);
        assert_eq!(sim.state.path.layers[0].cell(10, 20), BLOCKED);
        // Step 32 = 16th tree tick: `OnFallen`, destroyed in the same step.
        let events = step(&mut state, &rules, &[]);
        assert!(events.contains(&SimEvent::Destroyed { id: t }));
        let sim = Sim::new(&mut state, &rules);
        assert!(sim.object(t).is_none() && sim.state.extra_tick.is_empty());
        let cell = |l: usize| sim.state.path.layers[l].cell(10, 20);
        assert_eq!([0, 1, 2, 3, 4].map(cell), [0, 0, 0, 0, BLOCKED]);
        assert!(!sim.state.occupancy.test(10, 20));
        // The neighbor stays standing.
        assert!(sim.state.occupancy.test(12, 20) && sim.object(other).is_some());
    }

    // QUIRK W-28: kept
    #[test]
    fn a_tree_emptied_with_two_lumberjacks_leaves_the_search() {
        let rules = rules(100);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let t = plant(&mut sim, 4, 4);
        ask(&mut sim, t, msg::REGISTER, &[0, 0x1_0001]);
        ask(&mut sim, t, msg::REGISTER, &[0, 0x2_0001]);
        assert_eq!(ask(&mut sim, t, msg::GET_WOOD, &[1000]).0, 100);
        assert_eq!(flags(&sim, t), 0);
        // The tree tick counts the fall time no matter who calls it (otherwise only the extra list).
        crate::module::tick(&mut sim, t, INDEX, 100);
        assert_eq!(tree(&sim, t).unwrap().delta_time, 100);
    }
}
