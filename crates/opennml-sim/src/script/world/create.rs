//! Creating objects: actions 3 and 396 (units in an area, `FUN_0043D970`), 151 and 217
//! (object with a name, `FUN_00436440`) and 14 and 36 (object on a tile, `FUN_0043DD50`).
//!
//! All six call `GameWorld_CreateObject` with the flags `0x8D3` (finished, color, slot, rotation Y,
//! position, type) and give the object the color of the slot (`GameWorld_GetPlayerColor`). There is no
//! occupancy check; if creating fails, the action ends silently. Floating point like the original in
//! single precision per step, randomness only from [`ScriptState::rand`].
//!
//! [`ScriptState::rand`]: crate::script::ScriptState::rand

use opennml_formats::config::lower_german;

use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::Message;
use crate::module::quarter::ai_attack_mode_msg;
use crate::names;
use crate::script;
use crate::sim::Sim;
use crate::state::Control;
use crate::world::{ObjectDesc, create_object_named, desc_flags};

/// Creation flags of the script objects (`0x8D3`): finished, color, slot, rotation Y, position, type.
const SCRIPT_FLAGS: u32 = desc_flags::START_OBJECT | desc_flags::ROT_Y;
/// Degrees -> radians of 151/217 (`0.017453292f`, `[0x00616254]`).
const DEG_TO_RAD: f32 = f32::from_bits(0x3C8E_FA35);
/// `pi` (`[0x00618A4C]`) and `1/180` (`[0x0061900C]`) of 14/36.
const PI: f32 = f32::from_bits(0x4049_0FDB);
const INV_180: f32 = f32::from_bits(0x3BB6_0B61);
/// World height of the script objects (`1.0f`; the terrain sets the height afterwards).
const START_HEIGHT: f32 = 1.0;

/// Description of a script object: flags `0x8D3`, color of the slot (8 for an invalid slot).
fn desc(sim: &Sim, slot: i32, guid: i32, position: [f32; 3], rotation_y: f32) -> ObjectDesc {
    let mut d = ObjectDesc::new(guid as u32, slot, position);
    d.flags = SCRIPT_FLAGS;
    d.color = crate::setup::player_color(sim, slot);
    d.rotation[1] = rotation_y;
    d
}

/// AI stance after creating (`0x004365C0...0x0043664F`): only for a slot `0...7` (`GameEngine_GetPlayer`)
/// whose control kind is not human (also empty and passive slots); stance 2 sends `0x611 {4, 3, 1, 0}`,
/// stance 3 `{4, 3, 2, 0}` - to the returned object (for rider types the rider, not the horse). Other
/// stances send nothing.
pub fn ai_posture(sim: &mut Sim, slot: i32, id: ObjectId, mode: i32) {
    let Some(player) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| sim.state.players.get(s))
    else {
        return;
    };
    if player.control == Control::Human {
        return;
    }
    let persistence = match mode {
        2 => 1,
        3 => 2,
        _ => return,
    };
    let msg: Message = ai_attack_mode_msg(4, persistence);
    crate::messaging::send(sim, id, &msg);
}

/// Actions 151 and 217 (`FUN_00436440(slot, guid, area, name, angle)`): an object for `slot` on the
/// center of the area `area`, under the script name `name` (empty: without a name). `angle = -1` (151)
/// and any angle < 0 draw `rand() % 360` exactly once; 217 with an angle >= 0 draws nothing.
///
/// Center: `(left + right) / 2` and `(top + bottom) / 2`, signed, rounded toward 0, **without**
/// ordering the corners; if `right - left` is greater than 1 and at the same time `bottom - top` is
/// greater than 1, 1 is added on both axes. Position `(center + 0.5) * cell`. An unknown area does
/// nothing and draws nothing. Return value: the created object (for rider types the rider).
pub fn place_named(
    sim: &mut Sim,
    slot: i32,
    guid: i32,
    area: &str,
    name: &str,
    angle: i32,
) -> Option<ObjectId> {
    let [left, top, right, bottom] = script::area(sim, area)?;
    let mut cx = left.wrapping_add(right) / 2;
    let mut cz = top.wrapping_add(bottom) / 2;
    if right.wrapping_sub(left) > 1 && bottom.wrapping_sub(top) > 1 {
        cx = cx.wrapping_add(1);
        cz = cz.wrapping_add(1);
    }
    let cell = sim.rules.map.cell;
    let x = (cx as f32 + 0.5) * cell[0] as f32;
    let z = (cz as f32 + 0.5) * cell[2] as f32;
    let angle = if angle < 0 {
        sim.state.script.rand.rand() % 360
    } else {
        angle
    };
    let mut d = desc(
        sim,
        slot,
        guid,
        [x, START_HEIGHT, z],
        angle as f32 * DEG_TO_RAD,
    );
    let id = create_object_named(sim, &mut d, Some(name))?;
    let mode = u8::try_from(slot).map_or(0, |s| sim.state.combat.behavior_mode(s));
    ai_posture(sim, slot, id, mode);
    Some(id)
}

/// Actions 3 and 396 (`FUN_0043D970(slot, count, guid, area, name)`): `count` objects for `slot`
/// on the tiles of the area, row by row from the top left corner and, after the end, again from the
/// start. The rectangle is ordered; the rotation is never written (stack remainder in the original, here
/// 0), no randomness is drawn. Position `cell * tile + 10`.
///
/// If creating fails, the action is over - even without writing the name (E-195). With `name` (396; an
/// empty name counts as none) the list of the existing IDs (each piece with `atol`, in text order)
/// stands under the name afterwards and the new ones behind it, joined with `;` - also with
/// `count <= 0` (then the value is only rewritten). The stance of the slot is read once before the loop.
/// An unknown area does nothing.
pub fn spawn_in_area(
    sim: &mut Sim,
    slot: i32,
    count: i32,
    guid: i32,
    area: &str,
    name: Option<&str>,
) {
    let Some([left, top, right, bottom]) = script::area(sim, area) else {
        return;
    };
    let (x0, x1) = (left.min(right), left.max(right));
    let (z0, z1) = (top.min(bottom), top.max(bottom));
    let mode = u8::try_from(slot).map_or(0, |s| sim.state.combat.behavior_mode(s));
    let name = name.filter(|n| !n.is_empty());
    let mut ids: Vec<i32> = name.map_or_else(Vec::new, |n| {
        names::ids(&sim.state.names, n)
            .iter()
            .map(|id| id.as_i32())
            .collect()
    });
    let cell = sim.rules.map.cell;
    let (mut tx, mut tz) = (x0, z0);
    for _ in 0..count.max(0) {
        let x = cell[0].wrapping_mul(tx) as f32 + 10.0;
        let z = cell[2].wrapping_mul(tz) as f32 + 10.0;
        let mut d = desc(sim, slot, guid, [x, START_HEIGHT, z], 0.0);
        let Some(id) = create_object_named(sim, &mut d, None) else {
            return;
        };
        ids.push(id.as_i32());
        ai_posture(sim, slot, id, mode);
        tx = tx.wrapping_add(1);
        if tx > x1 {
            tz = tz.wrapping_add(1);
            tx = x0;
        }
        if tz > z1 {
            tz = z0;
        }
    }
    if let Some(name) = name {
        let list = ids.iter().map(i32::to_string).collect::<Vec<_>>().join(";");
        sim.state.names.set(&lower_german(name), list);
    }
}

/// Actions 14 (slot 8) and 36 (`FUN_0043DD50(slot, guid, x, z)`): an object on the tile corner
/// `cell * tile` (no offset), rotation random 0...179 degrees. The randomness is drawn **before**
/// creating, so also on a failure: `rand() % 180`, radians `(degrees * pi) * (1/180)` in `f32` per
/// step (deviation from the original). A slot outside 0...8 creates the object on slot 0 with color 8. Return value:
/// the created object.
pub fn spawn_at_tile(sim: &mut Sim, slot: i32, guid: i32, tx: i32, tz: i32) -> Option<ObjectId> {
    let cell = sim.rules.map.cell;
    let x = cell[0].wrapping_mul(tx) as f32;
    let z = cell[2].wrapping_mul(tz) as f32;
    let degrees = sim.state.script.rand.rand() % 180;
    let rotation = (degrees as f32 * PI) * INV_180;
    let mut d = desc(sim, slot, guid, [x, START_HEIGHT, z], rotation);
    let id = create_object_named(sim, &mut d, None)?;
    let mode = u8::try_from(slot).map_or(0, |s| sim.state.combat.behavior_mode(s));
    ai_posture(sim, slot, id, mode);
    Some(id)
}
