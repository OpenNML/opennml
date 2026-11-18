//! Blocking of the track tiles in the path grid (re/train-multipart 4.1, S8, K-207, 13.1). The locomotive keeps
//! a window of 12 or 13 tiles around its tile blocked in all nine layers; tender and wagon block
//! nothing. The tile is the **clamped** `GameObject::tile` (`GameObject_GetTilePos` `0x00466F70`) -
//! unlike the outside check in [`super::drive`], which takes the unclamped grid position.

use crate::id::ObjectId;
use crate::sim::Sim;

use super::{LOCO, status, train_mut};

/// All layers (`PathPlanner_MarkCell(0xFFFFFFFF, ...)`).
const ALL_LAYERS: u32 = u32::MAX;

/// Block or release the tile `tile + k*r`. Raw, without terrain classification (K-207: the release
/// makes the tile walkable for **all** layers, including water and steep slopes) and without
/// flushing. Tiles outside the map stay without effect (13.1; `PathGrid::mark_cell` extends the
/// dirty rectangle without a bounds check like the original and writes only inside the map).
fn mark(sim: &mut Sim, tile: (i32, i32), r: (i32, i32), k: i32, blocked: bool) {
    let (x, z) = (tile.0 + k * r.0, tile.1 + k * r.1);
    sim.state.path.mark_cell(ALL_LAYERS, x, z, blocked);
}

/// `Train_BlockTrackTiles` (`0x0052F1C0`): only the locomotive (GUID 2613). Init of the locomotive; a load does not
/// call it (deviation from the original: the reimplementation saves the path grid and
/// [`super::Train::last_block_tile`]).
/// Remembers the locomotive tile `T` and blocks `T + k*r` for `k = 2, 1, 0, ..., -9` (12 tiles) without
/// `FlushDirtyRect` - the rectangle takes effect at the next flush by another writer.
pub fn block_track_tiles(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else { return };
    if o.guid != LOCO {
        return;
    }
    let tile = o.tile;
    let Some(t) = train_mut(sim, id) else { return };
    t.last_block_tile = tile;
    let r = t.raster_dir;
    for k in (-9..=2).rev() {
        mark(sim, tile, r, k, true);
    }
}

/// `Train_UpdateTrackBlocking` (`0x0052F260`): after every drive step of the locomotive. Only if its
/// tile `T'` has changed relative to the last blocked tile: forward (bit 8) block `T' + 2r`,
/// release `T' - 11r` (window `-10 ... +2`, 13 tiles; after init or driving backward this
/// tile has never been blocked - a foreign obstacle there becomes free, K-207); backward (bit `0x10`)
/// release `T' + 3r`, block `T' - 9r` (window `-9 ... +2`, 12 tiles); then `FlushDirtyRect`.
/// Without a direction bit (does not occur after a drive step) only the remembered tile changes.
pub fn update_track_blocking(sim: &mut Sim, id: ObjectId) {
    let Some(tile) = sim.object(id).map(|o| o.tile) else {
        return;
    };
    let Some(t) = train_mut(sim, id) else { return };
    if t.last_block_tile == tile {
        return;
    }
    t.last_block_tile = tile;
    let (r, st) = (t.raster_dir, t.status);
    if st & status::FORWARD != 0 {
        mark(sim, tile, r, 2, true);
        mark(sim, tile, r, -11, false);
    } else if st & status::BACKWARD != 0 {
        mark(sim, tile, r, 3, false);
        mark(sim, tile, r, -9, true);
    }
    sim.state.path.flush_dirty_rect();
}
