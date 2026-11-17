//! Paths and fog of the dispatcher (re/ki-queries 5.17, 5.18, 5.20; E-33, E-77, E-78, E-79): reachable cells, path
//! length in tile cells, fog in the square.
//!
//! Quirks of the original that are listed here:
//! - `0x4E34` always returns the input cell: start and goal of the ring search in layer 4 are the same cell, the goal
//!   cell always fits, even blocked and outside the map (E-77).
//! - `0x4E35` asks the walk layer of the object (`0x1400`): object missing -> push 0, the outputs stay the variable
//!   addresses (E-33); without walk or dead -> the input cell, push 0; unsuccessful ring search -> original: stack
//!   leftovers, reimplementation 0 and 0 (E-33).
//! - `0x4FB0` counts the cells of the tile path; no path, missing or walkless object and start = goal give all 0, the
//!   free start cell is not checked (E-78). Paths over 1000 cells overflow the buffer in the original, the
//!   reimplementation reports 0. The side effect on the PathPlanner (`+0x2C`, `+0xAC`) has no game effect and is
//!   missing (5.20). **Reimplementation substitute:** if `FindFreeCellNear` (`0x004F9380`) fails, the original
//!   continues with the uninitialized stack values `local_10`/`local_c` as the start (like E-33); the reimplementation
//!   keeps the start (exceptions A1/A7, the value cannot be reproduced from the state). The ring search in the
//!   reimplementation computes in `i64`: a start with `x` near `i32::MIN` overflows in 32 bits in the original (no
//!   search, garbage), the reimplementation searches (K-110, exception A7).
//! - `0x4FF6`/`0x500A` have no `argc` check and no clipping at the map; the reimplementation limits the loop to the
//!   map, the result is the same (E-79). Deviation (exception A7): if an upper bound after the swap is exactly
//!   `i32::MAX`, the signed `<=` loop of the original (`FUN_0047B650`) never ends regularly, it wraps over `i32::MIN`
//!   and returns a late 1 or hangs; the reimplementation evaluates only the map tiles of the square (e.g.
//!   `x = i32::MAX`, `r = 0` -> 0).

use super::{Direct, argc, put, word};
use crate::id::ObjectId;
use crate::message::Message;
use crate::path::LAYER_COUNT;
use crate::player::View;

/// Walk: layer of the unit (`m_PPHandleNxN`), without walk or dead -1.
const WALK_LAYER: u32 = 0x1400;

/// Layer of the paths of the dispatcher `0x4E34` (`PathPlanner_FindNearestReachableCell(4, …)`).
const WATER_LAYER: i32 = 4;

/// Fog bit of `0x4FF6`: explored, seen or scouted (state >= 1).
const EXPLORED: u8 = 1;
/// Fog bit of `0x500A`: currently seen or scouted (state `& 2`).
const SEEN: u8 = 2;

/// Answers the codes of this group; `false` for foreign codes.
pub(super) fn answer(
    view: &mut View<'_, '_>,
    code: u32,
    req: &mut [i32],
    _direct: &mut Vec<Direct>,
) -> bool {
    match code {
        0x4E34 => reachable_cell(view, req),
        0x4E35 => object_reachable_cell(view, req),
        0x4FB0 => tile_path_length(view, req),
        0x4FF6 => fog_square(view, req, EXPLORED),
        0x500A => fog_square(view, req, SEEN),
        _ => return false,
    }
    true
}

/// Object from word `i`.
fn id_at(req: &[i32], i: usize) -> ObjectId {
    ObjectId::from_i32(word(req, i))
}

/// Walk layer of the object `id` (`0x1400`); `None` if the object is missing. A layer outside 0...8 (without walk,
/// dead) is kept so that the callers can tell it apart from "missing".
fn walk_layer(view: &mut View<'_, '_>, id: ObjectId) -> Option<i32> {
    view.message(id, &mut Message::new(WALK_LAYER, &[]))
}

/// Layer 0...8?
fn valid_layer(layer: i32) -> bool {
    usize::try_from(layer).is_ok_and(|l| l < LAYER_COUNT)
}

/// `0x4E34` (argc 5): `[5] = [3]`, `[6] = [4]` (E-77). The ring search of the original runs with start = goal and
/// always finds the goal cell; it runs along here so that hostile cells take the same path as in the original.
fn reachable_cell(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 5 {
        return;
    }
    let cell = (word(req, 3), word(req, 4));
    let (x, z) = view.reachable_cell(WATER_LAYER, cell, cell).unwrap_or(cell);
    put(req, 5, x);
    put(req, 6, z);
}

/// `0x4E35` (argc 6): `[2]` found, `[6]`/`[7]` the cell nearest to the goal `([4], [5])` in the region of the object's
/// tile in the walk layer of the object.
fn object_reachable_cell(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 6 {
        return;
    }
    let id = id_at(req, 3);
    let Some(start) = view.object(id, false).map(|o| o.tile) else {
        put(req, 2, 0);
        return;
    };
    let goal = (word(req, 4), word(req, 5));
    let layer = walk_layer(view, id).unwrap_or(-1);
    let (found, (x, z)) = if valid_layer(layer) {
        view.reachable_cell(layer, start, goal)
            .map_or((0, (0, 0)), |c| (1, c))
    } else {
        (0, goal)
    };
    put(req, 6, x);
    put(req, 7, z);
    put(req, 2, found);
}

/// `0x4FB0` (argc 6): `[2]` = cell count of the tile path from `([4], [5])` to `([6], [7])` in the walk layer of the
/// object `[3]`; otherwise 0. The start moves to the nearest free cell; success is not checked (E-78). If the search
/// fails, the reimplementation keeps the start (reimplementation substitute for the stack values `local_10`/`local_c`
/// of the original, like E-33).
fn tile_path_length(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 6 {
        return;
    }
    let id = id_at(req, 3);
    let layer = walk_layer(view, id).unwrap_or(-1);
    let mut n = 0;
    if valid_layer(layer) {
        let start = (word(req, 4), word(req, 5));
        let start = view.free_cell(layer, start).unwrap_or(start);
        let goal = (word(req, 6), word(req, 7));
        n = view
            .tile_path_len(layer, start, goal)
            .map_or(0, |cells| i32::try_from(cells).unwrap_or(i32::MAX));
    }
    put(req, 2, n);
}

/// `0x4FF6` (`bit` [`EXPLORED`]) and `0x500A` (`bit` [`SEEN`]), without `argc` check (5.17): `[3]` player, `[4]`/`[5]`
/// center, `[6]` half-width. `[2] = 1` if any tile of the square `x ± r`, `z ± r` (both bounds inclusive) carries the
/// fog bit for the player; player not in 0...7 -> 0. Upper bound exactly `i32::MAX`: the original wraps (late 1 or
/// hang), the reimplementation only the map (E-79).
fn fog_square(view: &View<'_, '_>, req: &mut [i32], bit: u8) {
    let p = word(req, 3);
    let (x, z, r) = (word(req, 4), word(req, 5), word(req, 6));
    let hit = (0..8).contains(&p) && any_fog_bit(view, p, square(x, z, r), bit);
    put(req, 2, i32::from(hit));
}

/// Square `{x − r, z − r, x + r, z + r}` wrapped in 32 bits, bounds swapped if the upper one is below the lower one (a
/// negative `r` acts like `|r|`; `FUN_0047B650`).
fn square(x: i32, z: i32, r: i32) -> [i32; 4] {
    let mut rect = [
        x.wrapping_sub(r),
        z.wrapping_sub(r),
        x.wrapping_add(r),
        z.wrapping_add(r),
    ];
    if rect[2] < rect[0] {
        rect.swap(0, 2);
    }
    if rect[3] < rect[1] {
        rect.swap(1, 3);
    }
    rect
}

/// Does any tile of the square (inclusive) carry the fog bit for `p`? The loop runs only over the map: tiles outside
/// return 0 in the original (E-79).
fn any_fog_bit(view: &View<'_, '_>, p: i32, rect: [i32; 4], bit: u8) -> bool {
    let (w, h) = view.map_size();
    let (x0, x1) = (rect[0].max(0), rect[2].min(w - 1));
    let (z0, z1) = (rect[1].max(0), rect[3].min(h - 1));
    (z0..=z1).any(|z| {
        (x0..=x1).any(|x| {
            let state = view.fog_state(x, z, p);
            if bit == EXPLORED {
                state >= 1
            } else {
                state & bit != 0
            }
        })
    })
}
