//! Module 5 `CPropertyNatur` (re/property-modules): decoration, flags, wrecks.
//! Path lock and occupancy bit according to re/occupancy 3; with `VISIONRANGE` a sight source
//! for all players; the capture of the flags (`CHECKNEARESTPLAYER`) searches the nearest walk object with the
//! spatial grid of the collision manager. `GENERATE_PEST` does not occur in
//! the shipped data.

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::economy::{announce, message};
use crate::footprint::snap_and_block_footprint;
use crate::fx::Fx;
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::Message;
use crate::sim::Sim;

use super::ModuleKind;
use super::events::update_events;

/// Module slot of `CPropertyNatur`.
pub const INDEX: usize = 5;
/// Flags (60002).
pub const ANIM: u32 = 0x100;
pub const CHECK_NEAREST_PLAYER: u32 = 0x400;
/// Sight source at the tile (`VISIONRANGE`).
pub const VISION_RANGE: u32 = 0x200;
/// Occupancy bit per tile (`BLOCKFFBRASTER`).
pub const BLOCK_FFB_RASTER: u32 = 0x1000;
/// Color of the neutral flag (`GO+0x2D0 == 9`), from which the animal and player colors 0 ... 8 differ.
const NEUTRAL_COLOR: i32 = 9;
/// Search radius of the flag check: `DAT_00672A30 << 2` = four tiles of 4096 each (re/domination F.3).
pub const FLAG_RADIUS: i32 = 4 << 12;
/// Message "[PLAYERNAME p] has captured a flag!" is `FLAG_MESSAGE + p` (`0xDC + p`).
pub const FLAG_MESSAGE: i32 = 220;
/// Message "You have captured a flag!" for the captor himself (`0xE4`, slot 8).
pub const FLAG_MESSAGE_SELF: i32 = 228;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Natur {
    /// `m_NaturSize` (60001): grid mode of the path lock.
    pub size: i32,
    /// `m_Timer` (flag check, -1 = off).
    pub timer: i32,
}

impl Default for Natur {
    fn default() -> Self {
        Self { size: 0, timer: -1 }
    }
}

fn natur_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Natur> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Natur(n) => Some(n),
        _ => None,
    }
}

/// `PropertyNatur_Init` (`0x0051F0E0`).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = desc.value_int(60002).unwrap_or(0) as u32;
    let size = desc.value_int(60001).unwrap_or(0) as i32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(n) = natur_mut(sim, id) {
        n.size = size;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 3;
    }
    // Path lock only with `OBJ_BLOCK_PATH` and above water (floating point, without distance), occupancy bit
    // only with `BLOCKFFBRASTER`.
    let occupancy = flags & BLOCK_FFB_RASTER != 0;
    let land = sim.object(id).is_some_and(|o| {
        sim.rules.block_path.contains(&o.guid)
            && sim.rules.ground_height(o.world_pos[0], o.world_pos[2]) >= sim.rules.map.water_level
    });
    if land || occupancy {
        snap_and_block_footprint(sim, id, size, 0x1F, occupancy, land, true);
    }
    // Natur is not selectable (`GameObject_SetSelectable(0)`).
    if let Some(o) = sim.object_mut(id) {
        o.selectable = false;
    }
    if flags & ANIM != 0
        && let Some(o) = sim.object_mut(id)
    {
        o.set_animation(0, 0, false, false, false);
    }
    // `VISIONRANGE` (`0x0051F1D2`…): sight source at the tile with the current sight range
    // (attribute 18), all players, unlimited, explored + seen (re/vision 4.4).
    if flags & VISION_RANGE != 0 {
        let tile = sim.object(id).map_or((0, 0), |o| o.tile);
        let size = sim.attribute(id, 18);
        crate::fog::set_vision_source(
            &mut sim.state.fog,
            crate::fog::VisionSource {
                x: tile.0,
                z_or_hid: tile.1,
                size,
                player: -1,
                remaining_ms: -1,
                flags: crate::fog::EXPLORE | crate::fog::SEE,
            },
        );
    }
    if flags & CHECK_NEAREST_PLAYER != 0 {
        if let Some(n) = natur_mut(sim, id) {
            n.timer = 1000;
        }
        let owner = id.slot();
        if owner == NEUTRAL_SLOT
            && let Some(o) = sim.object_mut(id)
        {
            o.color = 9;
        }
        adjust_flag_count(sim, owner, true);
    }
}

/// `PropertyNatur_Destruct` (`0x0051F040`): lift the path lock if `OBJ_BLOCK_PATH` and the
/// ground (fixed point) is not below the water; the occupancy bit stays (quirk).
pub fn destruct(sim: &mut Sim, id: ObjectId) {
    let map = sim.rules.map;
    let Some(o) = sim.object(id) else { return };
    let height =
        Fx::from_f32(sim.rules.ground_height(o.world_pos[0], o.world_pos[2])).0 / map.cell[1];
    let water = Fx::from_f32(map.water_level).0 / map.cell[1];
    if !sim.rules.block_path.contains(&o.guid) || height < water {
        return;
    }
    let size = natur_mut(sim, id).map_or(0, |n| n.size);
    snap_and_block_footprint(sim, id, size, 0x1F, false, true, false);
}

/// `GameWorld_AdjustFlagCount` (`0x00478400`): slots outside 0-7 count to 8. After counting up
/// the leader check of the flag game types 2 and 3 follows
/// (`GameEngine_EvaluateFlagLeader` `0x00445440`, [`crate::flags::evaluate_flag_leader`]), not when
/// subtracting (D-295).
pub fn adjust_flag_count(sim: &mut Sim, slot: u8, increase: bool) {
    let slot = if slot < 8 { usize::from(slot) } else { 8 };
    let c = &mut sim.state.flag_counts[slot];
    *c = c.wrapping_add(if increase { 1 } else { -1 });
    if increase {
        crate::flags::evaluate_flag_leader(sim);
    }
}

/// `PropertyNatur_Tick` (`0x0051F6F0`): timers, then the flag counter - fixed -100 per tick,
/// independent of `dt` (quirk).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
    let Some(n) = natur_mut(sim, id) else { return };
    if n.timer > 0 {
        n.timer -= 100;
        if n.timer <= 0 {
            n.timer = 1000;
            check_nearest_player(sim, id);
        }
    }
}

/// `PropertyNatur_CheckNearestPlayer` (`0x0051F3C0`, re/domination F.3): the capture of the flag
/// every ten natur ticks. The search uses `CollisionMgr_FindNearestMatching`
/// (`0x004B7620`) at the fixed-point position of the flag within the radius [`FLAG_RADIUS`] (four tiles,
/// center distance): the filter `0x14` (`ANDALL | CHECKPROPERTY`, module 0 "walk" even when switched off;
/// dead and quartered objects are excluded) finds **every** object with a walk module - animals,
/// ships, allies and neutrals included, no buildings (D-292).
///
/// - Neutral flag (color 9): the nearest walk object `u` takes it: color of `u`, message, slot 8
///   -1, slot of `u` +1.
/// - Occupied flag: the holder is the first slot 0 ... 7 with the flag color, whether occupied or not
///   (`FUN_00478DE0`, D-299/D-300). If no unit of this slot (filter `0x114`, owner = slot) stands
///   in the surroundings, the nearest walk object `u` with another color takes it: holder -1 (without
///   leader check), color, message, slot of `u` +1.
///
/// The **slot** of the captor is counted (`u.slot()`, folded to 8 above 7), the flag itself stays
/// on slot 8 - an animal colors it with 8 and the counter of the white player can become negative.
fn check_nearest_player(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(flag) = sim.object(id) else { return 1 };
    let pos = (flag.grid.x.0, flag.grid.z.0);
    let (flag_color, tile) = (flag.color, flag.tile);
    let walker = MatchFilter {
        mode: 0x14,
        ..MatchFilter::default()
    };
    if flag_color == NEUTRAL_COLOR {
        let Some(u) = find_nearest_matching(sim, pos, &walker, FLAG_RADIUS, -1, -1) else {
            return 1;
        };
        capture(sim, id, u, tile, None);
        return 1;
    }
    let own = crate::flags::slot_of_color(sim.state, flag_color);
    let guard = MatchFilter {
        mode: 0x114,
        ..MatchFilter::default()
    };
    if find_nearest_matching(sim, pos, &guard, FLAG_RADIUS, i32::from(own), -1).is_some() {
        return 1;
    }
    if let Some(u) = find_nearest_matching(sim, pos, &walker, FLAG_RADIUS, -1, -1)
        && sim.object(u).is_some_and(|o| o.color != flag_color)
    {
        capture(sim, id, u, tile, Some(own));
    }
    1
}

/// Rest of the capture: subtract the old holder (only in the occupied branch, before the color changes),
/// color the flag in the color of `u` (`GameObject_SetPlayerColor` `0x004680B0`), announce, and count up
/// for the slot of `u` (`GameWorld_AdjustFlagCount` `0x00478400`, then the leader check).
/// The neutral branch always subtracts slot 8, without checking the color (W-160).
fn capture(sim: &mut Sim, flag: ObjectId, u: ObjectId, tile: (i32, i32), old_holder: Option<u8>) {
    let Some(taker) = sim.object(u) else { return };
    let (color, slot) = (taker.color, taker.id.slot());
    match old_holder {
        Some(own) => adjust_flag_count(sim, own, false),
        None => adjust_flag_count(sim, NEUTRAL_SLOT, false),
    }
    if let Some(f) = sim.object_mut(flag) {
        f.color = color;
    }
    announce_capture(sim, slot, tile);
    adjust_flag_count(sim, slot, true);
}

/// Announcement of the capture to every slot 0 ... 7 (`MessageBar_ShowMessage` `0x00494A60`, flag
/// `0x10000`): to the captor 228 "You have captured a flag!", to the others `220 + p`
/// "[PLAYERNAME p] has captured a flag!". The original compares with the local slot; the
/// display filters. For a captor above 7 (animal) `220 + p` is 228 for everyone (D-294).
fn announce_capture(sim: &mut Sim, taker: u8, tile: (i32, i32)) {
    for q in 0..8u8 {
        let id = if q == taker {
            FLAG_MESSAGE_SELF
        } else {
            FLAG_MESSAGE + i32::from(taker)
        };
        announce(sim, i32::from(q), id, tile, message::flags::QUEUE);
    }
}

/// `PropertyNatur_Query` (`0x0051F720`): `0x105` flag check, `0x205` pest cloud (never created in
/// the shipped data).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        0x105 => check_nearest_player(sim, id),
        0x205 => 1,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::send;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    const TYPES: &str = "GUID 4201\nNAME Fahne\nVALUEID 100 6 1 0\nPROPID 5\nVALUEID 60001 6 1 1\nVALUEID 60002 6 1 256\nGUID 4300\nNAME Spieler_Fahne\nVALUEID 100 6 1 1024\nPROPID 5\nVALUEID 60001 6 1 1\nVALUEID 60002 6 1 5888\n";

    #[test]
    fn natur_animates_later_and_flags_count_with_a_fixed_timer() {
        let rules = Rules::synthetic(TYPES, 16);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let flag =
            create_object(&mut sim, &mut ObjectDesc::new(4201, 8, [100.0, 0.0, 100.0])).unwrap();
        // ANIM: animation 0 wanted, but handed over only in the object tick.
        let o = sim.object(flag).unwrap();
        assert_eq!((o.anim.wanted, o.anim.current), (0, -1));
        let player_flag =
            create_object(&mut sim, &mut ObjectDesc::new(4300, 0, [200.0, 0.0, 200.0])).unwrap();
        assert_eq!(player_flag.slot(), 8);
        assert_eq!(sim.object(player_flag).unwrap().color, 9);
        assert_eq!(sim.state.flag_counts[8], 1);
        for _ in 0..9 {
            tick(&mut sim, player_flag, 37);
        }
        let timer = |sim: &Sim| match &sim.module(player_flag, 5).unwrap().kind {
            ModuleKind::Natur(n) => n.timer,
            _ => panic!(),
        };
        assert_eq!(timer(&sim), 100);
        tick(&mut sim, player_flag, 37);
        assert_eq!(timer(&sim), 1000);
        assert_eq!(send(&mut sim, player_flag, &Message::new(0x105, &[])), 1);
        assert_eq!(send(&mut sim, player_flag, &Message::new(0x305, &[])), -1);
    }

    // ---- Capture (re/domination F.3) ----

    use crate::combat::testkit as ck;
    use crate::command::testkit::HOUSE;
    use crate::economy::message::flags::QUEUE;
    use crate::object::status;
    use crate::state::SimEvent;

    /// Player flag only with `CHECKNEARESTPLAYER`: no occupancy bit, no sight source.
    const FLAG_TYPE: &str = "GUID 4300\nNAME Spieler_Fahne\nVALUEID 100 6 1 1024\nPROPID 5\nVALUEID 60001 6 1 1\nVALUEID 60002 6 1 1024\n";
    /// Tile of the flag and its world position (tile center; one tile is 128 world units).
    const FLAG_TILE: (i32, i32) = (20, 20);
    const FLAG_WORLD: f32 = 20.0 * 128.0 + 64.0;

    fn world() -> (Rules, SimState) {
        let rules = ck::rules_with(FLAG_TYPE);
        let state = ck::state(&rules);
        (rules, state)
    }

    /// Neutral flag (slot 8, color 9, counter of slot 8 at 1).
    fn neutral_flag(sim: &mut Sim) -> ObjectId {
        let mut desc = ObjectDesc::new(4300, 8, [FLAG_WORLD, 0.0, FLAG_WORLD]);
        create_object(sim, &mut desc).unwrap()
    }

    /// Flag that `holder` holds with color `color`: color set, counter from slot 8 to `holder`.
    fn held_flag(sim: &mut Sim, color: i32, holder: u8) -> ObjectId {
        let flag = neutral_flag(sim);
        sim.object_mut(flag).unwrap().color = color;
        sim.state.flag_counts[8] -= 1;
        sim.state.flag_counts[usize::from(holder)] += 1;
        flag
    }

    /// Unit with walk module `dx` world units to the right of the flag.
    fn unit(sim: &mut Sim, guid: u32, slot: i32, dx: f32) -> ObjectId {
        let mut desc = ObjectDesc::new(guid, slot, [FLAG_WORLD + dx, 0.0, FLAG_WORLD]);
        desc.flags |= crate::world::desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    fn color(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().color
    }

    fn natur_ticks(sim: &mut Sim, flag: ObjectId, n: usize) {
        for _ in 0..n {
            tick(sim, flag, 37);
        }
    }

    /// Color of the flag and counters after ten natur ticks with a unit `dx` world units beside it.
    fn takeover(guid: u32, slot: i32, dx: f32) -> (i32, [i32; 9]) {
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        unit(&mut sim, guid, slot, dx);
        natur_ticks(&mut sim, flag, 10);
        (color(&sim, flag), sim.state.flag_counts)
    }

    /// A message of the event list: receiver, number, tile, flag.
    type Shown = (Option<u8>, i32, (i32, i32), u32);

    /// All messages of the event list.
    fn messages(sim: &Sim) -> Vec<Shown> {
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

    #[test]
    fn neutral_flag_takes_the_nearest_walker() {
        // 3.99 tiles = 510 world units (16,320 in 1/4096) lie within the radius 16,384.
        assert_eq!(
            takeover(ck::SABRE, 1, 510.0),
            (1, [0, 1, 0, 0, 0, 0, 0, 0, 0])
        );
        // Exactly four tiles (16,384) still count: the search skips only `d > radius`.
        assert_eq!(takeover(ck::SABRE, 1, 512.0).0, 1);
        // 4.01 tiles = 514 world units (16,448) lie outside.
        assert_eq!(
            takeover(ck::SABRE, 1, 514.0),
            (9, [0, 0, 0, 0, 0, 0, 0, 0, 1])
        );
    }

    #[test]
    fn first_check_after_ten_natur_ticks() {
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        unit(&mut sim, ck::SABRE, 1, 128.0);
        natur_ticks(&mut sim, flag, 9);
        assert_eq!(color(&sim, flag), 9);
        assert_eq!(sim.state.flag_counts[8], 1);
        natur_ticks(&mut sim, flag, 1);
        assert_eq!(color(&sim, flag), 1);
        assert_eq!(sim.state.flag_counts[1], 1);
    }

    #[test]
    fn held_flag_stays_while_an_own_unit_is_near() {
        // The holder has a unit in the surroundings, a stranger stands closer - the flag stays.
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = held_flag(&mut sim, 1, 1);
        unit(&mut sim, ck::SABRE, 1, 300.0);
        unit(&mut sim, ck::SABRE, 2, 64.0);
        natur_ticks(&mut sim, flag, 30);
        assert_eq!(color(&sim, flag), 1);
        assert_eq!(sim.state.flag_counts, [0, 1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn quartered_or_dead_units_do_not_hold() {
        for bit in [status::QUARTERED, status::DIED] {
            let (rules, mut state) = world();
            let mut sim = Sim::new(&mut state, &rules);
            let flag = held_flag(&mut sim, 1, 1);
            let own = unit(&mut sim, ck::SABRE, 1, 300.0);
            unit(&mut sim, ck::SABRE, 2, 64.0);
            sim.object_mut(own).unwrap().status |= bit;
            natur_ticks(&mut sim, flag, 10);
            assert_eq!(color(&sim, flag), 2, "bit {bit:#x}");
            assert_eq!(
                sim.state.flag_counts,
                [0, 0, 1, 0, 0, 0, 0, 0, 0],
                "bit {bit:#x}"
            );
        }
    }

    #[test]
    fn foreign_unit_takes_an_unguarded_flag() {
        // Counter old -1, new +1; only counting up checks the leader (re/domination F.4): slot 2 has the majority.
        let (rules, mut state) = world();
        state.mode = 2;
        let mut sim = Sim::new(&mut state, &rules);
        let flag = held_flag(&mut sim, 1, 1);
        sim.state.flag_leader = 1;
        sim.state
            .countdowns
            .insert(crate::flags::LEADER_COUNTDOWN, 5000);
        unit(&mut sim, ck::SABRE, 2, 200.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 2);
        assert_eq!(sim.state.flag_counts, [0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(sim.state.flag_leader, 2);
        assert!(sim.state.countdowns[&crate::flags::LEADER_COUNTDOWN] > 5000);
    }

    #[test]
    fn allies_take_flags_from_each_other() {
        // D-292: diplomacy does not count.
        let (rules, mut state) = world();
        state.diplomacy.set_friend(1, 2, true);
        state.diplomacy.set_friend(2, 1, true);
        let mut sim = Sim::new(&mut state, &rules);
        let flag = held_flag(&mut sim, 1, 1);
        unit(&mut sim, ck::SABRE, 2, 200.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 2);
        assert_eq!(sim.state.flag_counts, [0, 0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn buildings_do_not_take_flags() {
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        ck::spawn(&mut sim, HOUSE, 1, (FLAG_TILE.0 + 1, FLAG_TILE.1));
        natur_ticks(&mut sim, flag, 20);
        assert_eq!(color(&sim, flag), 9);
        assert_eq!(sim.state.flag_counts[8], 1);
    }

    #[test]
    fn ships_take_flags_too() {
        // D-292: every object with a walk module, ships too (walk flag NOTHUMAN).
        assert_eq!(
            takeover(ck::WARSHIP, 3, 200.0),
            (3, [0, 0, 0, 1, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn animal_takes_a_flag_with_message_228_for_everyone() {
        // D-294: color 8, the counter of slot 8 stays (-1 +1), every slot gets 228.
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        unit(&mut sim, ck::WOLF, 8, 200.0);
        sim.state.events.clear();
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 8);
        assert_eq!(sim.state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let tile = sim.object(flag).unwrap().tile;
        let expected: Vec<_> = (0..8u8)
            .map(|q| (Some(q), FLAG_MESSAGE_SELF, tile, QUEUE))
            .collect();
        assert_eq!(messages(&sim), expected);
    }

    #[test]
    fn messages_go_to_every_slot() {
        // 228 to the captor, 220 + p to the others, location the tile of the flag, flag QUEUE.
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        unit(&mut sim, ck::SABRE, 3, 200.0);
        sim.state.events.clear();
        natur_ticks(&mut sim, flag, 10);
        let tile = sim.object(flag).unwrap().tile;
        assert_eq!(tile, FLAG_TILE);
        let expected: Vec<_> = (0..8u8)
            .map(|q| {
                let id = if q == 3 {
                    FLAG_MESSAGE_SELF
                } else {
                    FLAG_MESSAGE + 3
                };
                (Some(q), id, tile, QUEUE)
            })
            .collect();
        assert_eq!(messages(&sim), expected);
    }

    #[test]
    fn white_counter_may_go_negative() {
        // D-299: slot 5 is white (color 8). An animal takes the neutral flag
        // (color 8, counted on slot 8); a third party takes it back - the white
        // slot 5 is subtracted, whose counter becomes -1. The majority keeps computing with integers.
        let (rules, mut state) = world();
        state.mode = 2;
        state.players[5].color = 8;
        let mut sim = Sim::new(&mut state, &rules);
        let flag = neutral_flag(&mut sim);
        unit(&mut sim, ck::WOLF, 8, 300.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 8);
        assert_eq!(sim.state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 1]);
        unit(&mut sim, ck::SABRE, 2, 64.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 2);
        assert_eq!(sim.state.flag_counts, [0, 0, 1, 0, 0, -1, 0, 0, 1]);
        assert_eq!(sim.state.flag_leader, 2);
    }

    #[test]
    fn empty_lower_slot_owns_a_borrowed_colour() {
        // D-300: slot 3 carries the color 1 of the unoccupied slot 1. Its unit in the surroundings
        // does not hold the flag (owner != slot 1), and the capture subtracts the counter of slot 1.
        let (rules, mut state) = world();
        state.players[3].color = 1;
        let mut sim = Sim::new(&mut state, &rules);
        let flag = held_flag(&mut sim, 1, 3);
        let own = unit(&mut sim, ck::SABRE, 3, 300.0);
        sim.object_mut(own).unwrap().color = 1;
        unit(&mut sim, ck::SABRE, 2, 64.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 2);
        assert_eq!(sim.state.flag_counts, [0, -1, 1, 1, 0, 0, 0, 0, 0]);

        // Counter-check: with its own color 3 the same setup holds the flag.
        let (rules, mut state) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let flag = held_flag(&mut sim, 3, 3);
        unit(&mut sim, ck::SABRE, 3, 300.0);
        unit(&mut sim, ck::SABRE, 2, 64.0);
        natur_ticks(&mut sim, flag, 10);
        assert_eq!(color(&sim, flag), 3);
        assert_eq!(sim.state.flag_counts, [0, 0, 0, 1, 0, 0, 0, 0, 0]);
    }
}
