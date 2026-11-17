//! Command builtins for movement, economy, construction, production, train and quartering (re/ki-queries 7.1/7.2). Each
//! flow builds the command words of the original dispatcher as [`CommandKind`]; the check against the sender (exception
//! A8, AI check path) is done by the simulation in `execute`.

use super::{CmdCall, issue};
use crate::command::economy::next_tree_filter;
use crate::command::{CommandKind, F32Bits, Outcome, TrainOp};
use crate::id::ObjectId;
use crate::ki::query::SimKiWorld;
use crate::ki::query::handlers::checked_list_ids;
use crate::ki::world::BuiltinCx;
use crate::message::Message;
use crate::messaging::send;
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::path::BLOCKED;
use crate::placement::find_building_placement_near;
use crate::player::View;

/// `Fixed_FromInt(1)` (`DAT_00671948`): spread of the AI movements.
const ONE: i32 = 1 << 12;
/// GUID of the fishing basket: as a hit of the tree search it gives 0 (like the nearest searches, E-53).
const FISH_BASKET: u32 = 0xDB1;
/// Walk layer of the unit (`0x1400`).
const WALK_LAYER: u32 = 0x1400;
/// Tiles at which `0x4E84` looks for the worker's region: its tile, x+1, x-1, then (x+1, z-1), (x+1, z+1)
/// (`0x004A8978…0x004A8A3B`); the first one without `0xFFFF` is used.
const REGION_PROBES: [(i32, i32); 5] = [(0, 0), (1, 0), (-1, 0), (1, -1), (1, 1)];

/// `Fixed_FromInt` (wrapping): whole tiles -> fixed point.
fn fixed(v: i32) -> i32 {
    v.wrapping_mul(ONE)
}

pub(super) fn oid(v: i32) -> ObjectId {
    ObjectId::from_i32(v)
}

/// Issues a command; the network command is the type code of the variant.
pub(super) fn give(w: &mut SimKiWorld<'_, '_>, call: &CmdCall<'_>, kind: CommandKind) -> Outcome {
    let code = kind.type_code() & 0xFF;
    issue(w, call.me, code, kind)
}

/// Flags of the movements (`MoveCommand_Construct`, last argument): 0xF5/0xF7 and aliases 2, 0xF6/0xF8 0, 0xFD/0xFF 3,
/// 0xFE/0x100 1 (bit 0 run, bit 1 attack march).
pub(super) fn move_flags(id: u16) -> i32 {
    match id {
        0xF5 | 0xF7 | 0xF9 | 0xFB => 2,
        0xF6 | 0xF8 | 0xFA | 0xFC => 0,
        0xFD | 0xFF | 0x101 | 0x103 => 3,
        _ => 1,
    }
}

/// `0x04` like `MoveCommand_Construct(n, Fixed(x), Fixed(z), −1, formation, no direction, Fixed(1), flags)`; `units` in
/// list order (`AddUnit`).
fn move_units(
    w: &mut SimKiWorld<'_, '_>,
    call: &CmdCall<'_>,
    units: Vec<ObjectId>,
    (x, z, formation): (i32, i32, i32),
) {
    give(
        w,
        call,
        CommandKind::Move {
            units,
            x: fixed(x),
            z: fixed(z),
            target: -1,
            formation,
            direction: NO_DIRECTION,
            spread: ONE,
            flags: move_flags(call.id),
        },
    );
}

/// Present (`GetObjectById(id, 0)`, also dying) and not dead (status bit 18).
fn alive(w: &SimKiWorld<'_, '_>, id: ObjectId) -> bool {
    w.sim
        .object(id)
        .is_some_and(|o| o.status & status::DIED == 0)
}

/// `0xF5`/`0xF6` (aliases `0xF9`/`0xFA`, pattern C): only for a present, not dead unit (`0x4E5C`/`0x5019`), otherwise
/// no command.
pub(super) fn move_alive(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let u = oid(call.arg(0));
    if alive(w, u) {
        move_units(w, call, vec![u], (call.arg(1), call.arg(2), 0));
    }
}

/// `0xFD`/`0xFE` (aliases `0x101`/`0x102`): without existence and death check (`0x4F4C`/`0x5023`).
pub(super) fn move_unchecked(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let u = oid(call.arg(0));
    move_units(w, call, vec![u], (call.arg(1), call.arg(2), 0));
}

/// Group movement (pattern D): only for `var(&obj) ≠ 0` and `0 < n < 100`; the IDs `obj[start + i]` creating missing
/// keys (`FUN_004E7B30`), the dispatcher omits missing, dead and duplicate ones (list order) and sends **one** `0x04`
/// with the rest - even with none (E-90). The formation goes in raw. A pointer without a list would be an access to
/// nonsense in the original: `KiNote::Guarded`, no command (re/ki-vm 12.3 P7).
pub(super) fn move_group(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (list, start, n) = (call.arg(0) as u32, call.arg(1), call.arg(2));
    let Some(ids) = checked_list_ids(cx, w, call.id, list, start, n) else {
        return;
    };
    let mut units: Vec<ObjectId> = Vec::new();
    for u in ids.into_iter().map(oid) {
        if alive(w, u) && !units.contains(&u) {
            units.push(u);
        }
    }
    move_units(w, call, units, (call.arg(3), call.arg(4), call.arg(5)));
}

/// `0xB4` `(u, bld)` -> `0x02 {bld, 1, u}`.
pub(super) fn construct(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::ConstructAt {
        building: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0xC8` `(u, horse)` -> `0x0A {horse, 1, u}`, push 0.
pub(super) fn mount(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Mount {
        horse: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
    cx.push(0);
}

/// `0xC9` `(u)` -> `0x0B {1, u}`, push 0.
pub(super) fn dismount(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Dismount {
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
    cx.push(0);
}

/// `f32(i(cell) · v)` like `FILD/FIMUL/FSTP` (`0x004A88DE…0x004A8902`); the product rounds in the FPU precision of the
/// process (MSVC default 53 bits, **hypothesis**) and then to `f32`.
fn cell_times(cell: i32, v: i32) -> f32 {
    (i64::from(cell) * i64::from(v)) as f64 as f32
}

/// `CRT_ftol2` on an `f64`: truncate; NaN and values outside `i32` -> `i32::MIN`.
fn ftol64(v: f64) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&v) {
        v as i32
    } else {
        i32::MIN
    }
}

/// Building site search of `0x4E84` (`0x004A88CB`): position `f32(cell · tile)`; worker missing
/// (`GetObjectById(id, 0)`) -> layer -1, region -1 (E-89); otherwise walk layer (`0x1400`), negative -> region -1;
/// otherwise the first region != `0xFFFF` at the [`REGION_PROBES`] around the worker's tile (`GameObject_GetTilePos`),
/// none -> `None`. Then `GameEngine_FindBuildingPlacementNear`; finally the ID bits `(worker >> 11) & 31` = `me`.
fn find_site(w: &mut SimKiWorld<'_, '_>, call: &CmdCall<'_>) -> Option<(f32, f32)> {
    let (worker, guid) = (call.arg(0), call.arg(1));
    let cell = w.sim.rules.map.cell;
    let mut fx = cell_times(cell[0], call.arg(2));
    let mut fz = cell_times(cell[2], call.arg(3));
    let id = oid(worker);
    let (layer, region) = match w.sim.object(id).map(|o| o.tile) {
        None => (-1, -1),
        Some(tile) => {
            let layer = send(w.sim, id, &Message::new(WALK_LAYER, &[]));
            match usize::try_from(layer) {
                Err(_) => (layer, -1),
                Ok(l) => {
                    let path = &w.sim.state.path;
                    let region = REGION_PROBES
                        .iter()
                        .map(|&(dx, dz)| {
                            path.region_id(l, tile.0.wrapping_add(dx), tile.1.wrapping_add(dz))
                        })
                        .find(|&r| r != BLOCKED)?;
                    (layer, i32::from(region))
                }
            }
        }
    };
    let me = i32::from(call.me);
    if !find_building_placement_near(w.sim, &mut fx, &mut fz, guid as u32, me, layer, region) {
        return None;
    }
    ((worker >> 11) & 0x1F == me).then_some((fx, fz))
}

/// `0x12C` `(worker, guid, x, z, &x', &z', border)` (pattern F, `0x4E84`): building site per [`find_site`]; found ->
/// `0x01 {guid, 0, fx, 0, fz, 1, worker, border}` (rotation 0, height 0, the extra word is the occupancy border), push
/// 1 and the outputs `trunc(f64(fx) / cell)` or `fz` (`FILD/FDIVR`, `CRT_ftol2`). Otherwise push 0 (corrects B-6) and
/// the output variables get their own address. The pause 500 is set by [`super::call`]. The push depends only on the
/// search, not on the result of the command (the original does not know it).
pub(super) fn place_building(
    call: &CmdCall<'_>,
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let (out_x, out_z) = (call.arg(4), call.arg(5));
    let (found, tile) = match find_site(w, call) {
        Some((fx, fz)) => {
            let kind = CommandKind::PlaceBuilding {
                guid: call.arg(1) as u32,
                rotation: F32Bits::new(0.0),
                x: F32Bits::new(fx),
                y: F32Bits::new(0.0),
                z: F32Bits::new(fz),
                workers: vec![oid(call.arg(0))],
                border: Some(call.arg(6)),
            };
            give(w, call, kind);
            let cell = w.sim.rules.map.cell;
            let tile = (
                ftol64(f64::from(fx) / f64::from(cell[0])),
                ftol64(f64::from(fz) / f64::from(cell[2])),
            );
            (1, tile)
        }
        None => (0, (out_x, out_z)),
    };
    cx.push(found);
    cx.set(out_x as u32, tile.0);
    cx.set(out_z as u32, tile.1);
}

/// `0x12D` `(bld, guid)` -> `0x0E {bld, guid, me}`, push 100.
pub(super) fn enqueue_production(
    call: &CmdCall<'_>,
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::EnqueueProduction {
        building: oid(call.arg(0)),
        guid: call.arg(1) as u32,
    };
    give(w, call, kind);
    cx.push(100);
}

/// `0x12E` `(bld, x, z)` -> `0x27 {Fixed(x), Fixed(z), 1, bld}`.
pub(super) fn rally_point(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::SetRallyPoint {
        x: fixed(call.arg(1)),
        z: fixed(call.arg(2)),
        buildings: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0x130` `(bld)` -> `0x0F {bld, 0}` with len 2; the handler reads slot and `arg` behind the command (stack
/// remainder), `0x1103` does not evaluate `arg`: here `arg = 0` (E-91).
pub(super) fn cancel_production(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::CancelProduction {
        building: oid(call.arg(0)),
        index: 0,
        arg: 0,
    };
    give(w, call, kind);
}

/// `0x15D` (halt `0x2B {train}`, push `me`), `0x167` (forward `0x29`), `0x168` (backward `0x2A`).
pub(super) fn train(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let op = match call.id {
        0x167 => TrainOp::Forward,
        0x168 => TrainOp::Backward,
        _ => TrainOp::Stop,
    };
    let kind = CommandKind::Train {
        train: oid(call.arg(0)),
        op,
        target: None,
    };
    give(w, call, kind);
    if op == TrainOp::Stop {
        cx.push(i32::from(call.me));
    }
}

/// `0x172`/`0x173` `(u, container)` -> `0x13 {container, 1, u}` (both codes the same, re/ki-queries 4 no. 6).
pub(super) fn quarter(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Quarter {
        container: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0x174` `(u, boat)` -> `0x0C {boat, 1, u}`.
pub(super) fn board_boat(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::BoardBoat {
        boat: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0x176` `(container, unit)` (`0x4FCE`): `unit == −1` -> `0x15 {container}`, otherwise `0x14 {container, unit}`.
pub(super) fn unquarter(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let container = oid(call.arg(0));
    let kind = match call.arg(1) {
        -1 => CommandKind::UnquarterAll { container },
        unit => CommandKind::Unquarter {
            container,
            unit: oid(unit),
        },
    };
    give(w, call, kind);
}

/// `0x178` `(u)` -> `0x0D {1, u}` (the handler always sets `[4] = 0`, `[2] = 0` unpushed).
pub(super) fn leave_boat(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::LeaveBoat {
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0x191`/`0x19A` `(u)` -> `0x3C {1, u}`.
pub(super) fn halt(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Halt {
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
}

/// `0x192` `(u, field)` -> `0x07 {field, 1, u}`, push 0.
pub(super) fn harvest(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Harvest {
        farm: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
    cx.push(0);
}

/// `0x193` `(u, x, z, &kx, &kz)` (pattern F, `0x4EEB`): nearest free tree to the tile corner `(Fixed(x), Fixed(z))` (y
/// = 1 raw is not evaluated by the search) with the filter of `0x7F` ([`next_tree_filter`]: `0x1016`, bits `0x104`,
/// module 4), reference `me`, unlimited radius, without fog. Hit (not the fishing basket) -> `0x06 {tree, 1, u}`, push
/// 1, outputs = tile of the tree (`GameObject_GetTilePos`); otherwise push 0 and the output variables get their own
/// address.
pub(super) fn chop_nearest_tree(
    call: &CmdCall<'_>,
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let (out_x, out_z) = (call.arg(3), call.arg(4));
    let pos = (fixed(call.arg(1)), fixed(call.arg(2)));
    let me = i32::from(call.me);
    let hit = View::new(w.sim, call.me, w.sight).find_nearest(pos, &next_tree_filter(), -1, me);
    let tree = hit.and_then(|t| {
        w.sim
            .object(t)
            .filter(|o| o.guid != FISH_BASKET)
            .map(|o| (t, o.tile))
    });
    let (found, tile) = match tree {
        Some((t, tile)) => {
            let kind = CommandKind::ChopWood {
                tree: t,
                units: vec![oid(call.arg(0))],
            };
            give(w, call, kind);
            (1, tile)
        }
        None => (0, (out_x, out_z)),
    };
    cx.push(found);
    cx.set(out_x as u32, tile.0);
    cx.set(out_z as u32, tile.1);
}

/// `0x194` `(u, animal)` -> `0x17 {animal, 1, u}` (hunting), push 100.
pub(super) fn hunt(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::Hunt {
        animal: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
        gut: false,
    };
    give(w, call, kind);
    cx.push(100);
}

/// `0x195` `(u, mine)` -> `0x03 {mine, 1, u}`, push 1.
pub(super) fn mine_gold(call: &CmdCall<'_>, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::MineGold {
        mine: oid(call.arg(1)),
        units: vec![oid(call.arg(0))],
    };
    give(w, call, kind);
    cx.push(1);
}

/// `0x231` `(bld, guid)` -> `0x2F {guid, bld}`.
pub(super) fn counter_upgrade(
    call: &CmdCall<'_>,
    _: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
) {
    let kind = CommandKind::CounterUpgrade {
        guid: call.arg(1) as u32,
        source: oid(call.arg(0)),
    };
    give(w, call, kind);
}

/// `0x23A` `(container)` -> `0x15 {container}`.
pub(super) fn unquarter_all(call: &CmdCall<'_>, _: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let kind = CommandKind::UnquarterAll {
        container: oid(call.arg(0)),
    };
    give(w, call, kind);
}
