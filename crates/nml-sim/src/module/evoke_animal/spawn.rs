//! Position search of the animal evocation (`0x0050FF10`, re/evoke-aura 3.5): rings around the
//! evoker in 10-degree steps from its facing direction, the first point in its region with
//! collision cost below `0x1000` wins.

use crate::collision::cost_at;
use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::Message;
use crate::messaging::send;
use crate::module::walk;
use crate::sim::Sim;

/// Ring distance `DAT_006727A8` (= `Fixed(1)`, initializer `0x00611170`): one tile.
const STEP: i32 = 4096;
/// Angle step 10 degrees in degrees * 4096.
const ANGLE_STEP: i32 = 10 << 12;
/// Ring onward as soon as the angle exceeds 360 degrees (37 attempts per ring, 0 and 360 degrees twice).
const FULL_TURN: i32 = 360 << 12;
/// Cost threshold `[0x006719E8]` (= `0x1000`): below it the point is free.
const COST_LIMIT: i32 = 0x1000;
/// From this ring counter on the point is offset by one tile in the facing direction after the check
/// (`rings > 3`, i.e. from ring 5; C-280).
const SHIFT_AFTER_RINGS: i32 = 3;

/// `EvokeAnimal_FindSpawnPos` (`0x0050FF10`, re/evoke-aura 3.5, C-280): fetch the region in the walk layer
/// (`0x1400`) and the cost layer 1x1 (`0xB00`) once (without walk -1 -> outside 0...8 as in
/// `find_lowest_cost_spot_near`). From ring 1 (distance one tile) and angle 0 relative to the
/// facing direction per attempt: point `(x0, z0) + r · (cos, sin)(facing + a)`, then `a += 10°` (above
/// 360 degrees -> next ring); hit if the region (`Fixed_ToInt` = `>> 12`) equals that of the evoker
/// and `CollisionMgr_GetCostAt(point, evoker, layer 1×1, 0, 0, 0) < 0x1000` (footprint
/// of the **evoker**). From ring 5 on, one tile in the facing direction is added **after** the check -
/// the returned position is unchecked. Height 0. Return value in the order `(x, y, z)`.
///
/// The facing direction is `walk::direction`; the offset `+0x168` at status `0x5000000` exists only for
/// SiegeEngine (like `group/march.rs` `facing`).
///
/// Deviation (exception A7, K-312): the original keeps searching without abort and hangs in an endless loop
/// without a free place in the region. The reimplementation returns `None` as soon as the
/// ring distance exceeds the larger map side (`r > max(dim_x, dim_z) · 4096`); there
/// no point of the map lies on a ring any more.
pub fn find_spawn_pos(sim: &mut Sim, id: ObjectId) -> Option<[i32; 3]> {
    let (x0, z0) = sim.object(id).map(|o| (o.grid.x.0, o.grid.z.0))?;
    let region_layer = layer(send(sim, id, &Message::new(0x1400, &[])));
    let cost_layer = layer(send(sim, id, &Message::new(0xB00, &[])));
    let region0 = sim.state.path.region_id(region_layer, x0 >> 12, z0 >> 12);
    let map = sim.rules.map;
    let limit = i64::from(map.dim_x.max(map.dim_z)) << 12;
    let (mut r, mut a, mut rings) = (STEP, 0, 0);
    while i64::from(r) <= limit {
        let face = walk::direction(sim, id);
        let angle = face.wrapping_add(a);
        let mut x = x0.wrapping_add(fixed_mul(cos_deg(angle), r));
        let mut z = z0.wrapping_add(fixed_mul(sin_deg(angle), r));
        a += ANGLE_STEP;
        if a > FULL_TURN {
            r += STEP;
            a = 0;
            rings += 1;
        }
        let found = sim.state.path.region_id(region_layer, x >> 12, z >> 12) == region0
            && cost_at(sim, (x, z), Some(id), cost_layer, false, false) < COST_LIMIT;
        if rings > SHIFT_AFTER_RINGS {
            x = x.wrapping_add(cos_deg(face));
            z = z.wrapping_add(sin_deg(face));
        }
        if found {
            return Some([x, 0, z]);
        }
    }
    None
}

/// Layer response as an index; -1 (no walk) lies outside 0...8.
fn layer(v: i32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}
