//! Building codes of the tents (re/tents 2.3): `0x2E03` hides the building when packing, `0x2F03`
//! restores it at the new position. Only module 42 sends both codes
//! ([`crate::module::tent`]).

use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::sim::Sim;

use super::{building, building_mut, footprint_rect, mark_footprint, mark_occupancy_rect, parking};

/// `PropertyBuilding_OnMsgHide` (`0x00508E00`, `0x2E03`): take back the occupancy (via the
/// freed list, otherwise the whole rectangle; the list stays, re/occupancy 5.3), path grid
/// free, sleep, back up render bits and set them to 0, not selectable, remove from the collision manager;
/// response 0. Unlike `0x1F03` without `StartBuild` and without `DIED`. BuildingTileMap and
/// vision stamp remain (W-256, W-257).
pub fn on_hide(sim: &mut Sim, id: ObjectId) -> i32 {
    mark_occupancy_rect(sim, id, false);
    mark_footprint(sim, id, false, true);
    let bits = sim.object(id).map_or(0, |o| o.render_bits);
    if let Some(b) = building_mut(sim, id) {
        b.is_sleeping = true;
        b.render_bits_backup = bits;
    }
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = 0;
        o.selectable = false;
    }
    // Also removes from all area queries (`collision::remove` -> `area::notify_remove`).
    crate::collision::remove(sim, id);
    0
}

/// Fragment `0x0050A5F0` (`0x2F03`, re/tents 3.5): the tent stands at the new position. In this
/// order: rectangle from the model at the current position (also for the alarm post 1610,
/// W-254), clear the freed list, occupancy rectangle = rectangle (a `0x3303` border is lost, W-253),
/// set occupancy (the bits were already set by `0x22A`, the freed list stays empty, W-252),
/// path grid with the mask from `Init` (W-255), docking places anew, rally point origin - 3 tiles
/// z without target (W-258), awake, backed-up render bits, selectable; response 0. No entry in the
/// BuildingTileMap (W-256) and in the collision manager (W-89), no `0x1A17`.
pub fn on_tent_placed(sim: &mut Sim, id: ObjectId) -> i32 {
    let rect = footprint_rect(sim, id);
    if let Some(b) = building_mut(sim, id) {
        b.rect = rect;
        b.freed.clear();
        b.occupancy_rect = rect;
    }
    mark_occupancy_rect(sim, id, true);
    mark_footprint(sim, id, true, true);
    let grid = sim.object(id).map_or(FxVec3::ZERO, |o| o.grid);
    if let Some(b) = building_mut(sim, id) {
        // `ClearParkingLots` (`0x00508020`) and `BuildParkingLots` (`0x00509980`).
        b.parking_lots =
            parking::build_parking_lots(&b.mask, (rect[0] << 12) - 0x800, (rect[1] << 12) - 0x800);
        b.meeting_point = FxVec3::new(grid.x, grid.y, Fx(grid.z.0.wrapping_sub(3 << 12)));
        b.meeting_target = ObjectId::NONE;
        b.is_sleeping = false;
    }
    let bits = building(sim, id).map_or(0, |b| b.render_bits_backup);
    if let Some(o) = sim.object_mut(id) {
        o.render_bits = bits;
        o.selectable = true;
    }
    0
}
