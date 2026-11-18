//! Placers of the object step (re/random-map Z.7.1, Z.7.2, ZK-b.3; quirks D-338, D-353, D-354,
//! D-359): GUID choice (`FUN_005417D0`), scaling of the quantities at block end (`FUN_0053FB20`) and
//! the general placer `FUN_005451E0` with groups.
//!
//! Randomness only from [`GenRand`] in exactly the sequence of the original (ZK-b.3): R1 per object
//! (with `k ≥ 2` GUIDs), per attempt R2/R3 (`% W`, `% H`) for a single object and group center or
//! R4...R6 (`% 361`, `% (R+1)`, `% (R+1)`) for a group member; the checks ([`check_attempt`]) draw
//! nothing, placing (hook [`ObjectCtx::set_object`], `FUN_0053FD00`) draws R7; afterwards the placer
//! itself paints `GroundTexture` ([`ground_texture`], `FUN_00543260`) like the original.

use std::rc::Rc;

use anyhow::{Context, bail};
use opennml_formats::prj::ProjectObject;
use opennml_formats::rnd::{ObjectDef, PlaceObjectDef, RandomMapScript};
use opennml_rda::normalize_path;
use opennml_sim::fx::{Fx, fixed_mul};
use opennml_sim::math::{cos_deg, sin_deg};

use crate::area::scaled;
use crate::checks::{ObjectCtx, check_attempt, fill_attempt, tile_ok};
use crate::geometry::{ModelGeometry, load_geometry};
use crate::objects::{NEUTRAL, ground_texture};
use crate::rng::GenRand;

/// GUID of an unknown `#Object` name (`FUN_005417D0` returns `0xFFFFFFFF`; no project object
/// carries it, the object is dropped at `Project_FindObject`).
pub(crate) const UNKNOWN_GUID: u32 = u32::MAX;

/// Last attempt counter (`CMP EDX,0x64` `0x0054568E`): attempts 0 ... 100, so at most 101.
const LAST_TRY: u32 = 100;

impl ObjectCtx<'_> {
    /// Model geometry via the cache of the object step (`FUN_0057DB80`: look in the cache, otherwise
    /// load `FUN_0057DA20`); a model that cannot be loaded is tried once and remembered as `None`
    /// (D-355).
    pub(crate) fn geometry(&mut self, model: &str) -> Option<Rc<ModelGeometry>> {
        let key = normalize_path(model);
        if let Some(entry) = self.geometries.get(&key) {
            return entry.clone();
        }
        let entry = load_geometry(&mut *self.read, &key).map(Rc::new);
        self.geometries.insert(key, entry.clone());
        entry
    }

    /// `OBJ_XROT/YROT/ZROT` of the project object as the checks and placing read them
    /// (`+0x24/+0x28/+0x2C`); `YROT` from the rotation table (D-336).
    pub(crate) fn rotation(&self, po: &ProjectObject) -> [f32; 3] {
        let [x, y, z] = po.default_rotation;
        [x, self.rotations.get(&po.guid).copied().unwrap_or(y), z]
    }
}

/// GUID choice from a `#Object` block (`FUN_005417D0` from `0x0054183E`, R1 `0x0054184F`):
/// `k < 2 → 0`, otherwise `rand() % k`. `k == 0` (block without `GUID`) reads through a null
/// pointer in the original (D-353); the reimplementation reports it without drawing.
pub(crate) fn pick_guid(def: &ObjectDef, rng: &mut GenRand) -> anyhow::Result<u32> {
    let k = def.guids.len();
    if k == 0 {
        bail!(
            "random map: #Object \"{}\" has no GUID line, the object cannot be chosen",
            def.name
        );
    }
    let index = if k < 2 {
        0
    } else {
        (rng.rand() % k as i32) as usize
    };
    Ok(def.guids[index])
}

/// `FUN_005417D0`: the **first** `#Object` block with exactly this name (`_mbscmp`), then
/// [`pick_guid`]; unknown name -> [`UNKNOWN_GUID`] without a draw.
pub(crate) fn object_guid(
    script: &RandomMapScript,
    name: &str,
    rng: &mut GenRand,
) -> anyhow::Result<u32> {
    match script.objects.iter().find(|o| o.name == name) {
        Some(def) => pick_guid(def, rng),
        None => Ok(UNKNOWN_GUID),
    }
}

/// Quantities of a `place_Object` block at block end (`FUN_0053FB20`, call `0x00544984`, once
/// per block for all three placers); `players` = `G+0x120`, `scale` = map factor `G+0x268`:
///
/// 1. `ScaleToPlayers`: `number · n`, with `numGroups > 1` also `n · numGroups` (`IMUL`, 32 bits).
/// 2. `ScaleToMapSize`: `number` times map factor, with `numGroups ≥ 2` (after 1.) also `numGroups`;
///    **otherwise** `ScaleGroupsToMapSize`: `numGroups` times map factor without a condition.
///
/// Map factor via [`scaled`] = `Fixed_ToInt(Fixed_Mul(f, Fixed_FromInt(v)))`.
pub(crate) fn scale_place_object(def: &PlaceObjectDef, players: i32, scale: Fx) -> PlaceObjectDef {
    let mut d = def.clone();
    if d.scale_to_players {
        d.number = d.number.wrapping_mul(players);
        if d.num_groups > 1 {
            d.num_groups = players.wrapping_mul(d.num_groups);
        }
    }
    if d.scale_to_map_size {
        d.number = scaled(scale, d.number);
        if d.num_groups >= 2 {
            d.num_groups = scaled(scale, d.num_groups);
        }
    } else if d.scale_groups_to_map_size {
        d.num_groups = scaled(scale, d.num_groups);
    }
    d
}

/// `value % divisor` like `CDQ; IDIV` (C remainder, sign of the dividend, a negative divisor acts
/// with its absolute value). Divisor 0 divides by zero in the original (D-359) -> error.
pub(crate) fn c_rem(value: i32, divisor: i32) -> anyhow::Result<i32> {
    value
        .checked_rem(divisor)
        .with_context(|| format!("random map: divisor {divisor} in object placement"))
}

/// Tile corner `(rand() % W) · S` (`0x0054533D…0x00545374`): `IMUL` integer, `FILD`, with a
/// negative product `+ 2³²` (`[0x00618A58]`), `FSTP f32` - one rounding like `u32 → f32`.
fn tile_corner(rng: &mut GenRand, cells: i32, s: i32) -> anyhow::Result<f32> {
    let cell = c_rem(rng.rand(), cells)?;
    Ok(s.wrapping_mul(cell) as u32 as f32)
}

/// Offset of a group member per axis (`0x005454B3…0x005454EE`):
/// `Fixed_ToFloat(Fixed_Mul(FromInt(S), Fixed_Mul(FromInt(r), trig)))`, argument order as in the
/// original.
fn member_offset(s: i32, r: i32, trig: i32) -> f32 {
    Fx(fixed_mul(
        Fx::from_int(s).0,
        fixed_mul(Fx::from_int(r).0, trig),
    ))
    .to_f32()
}

/// General placer `FUN_005451E0` (neither `Fill` nor `ForAllPlayers`), owner 8. `def`
/// carries the already scaled quantities ([`scale_place_object`]).
///
/// ```text
/// for g in 0..numGroups, j in 0..number:
///   R1 GUID; project object missing or model not loadable -> next j (D-355)
///   attempts t = 0 ... 100:
///     single object or center (numGroups == 1 or j == 0): R2/R3 tile corner;
///         with groups center = this unsnapped position (even if the attempt fails, D-354)
///     member: R4 angle, R5/R6 radius per axis around the center
///     checks (8 only for the group center; passed -> center into the list)
///     t == 100 and group center -> j = number (members dropped, D-338)
///     passed -> place; end of attempts
/// ```
pub(crate) fn place_general(
    ctx: &mut ObjectCtx<'_>,
    def: &PlaceObjectDef,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    // List of group centers per block (`CList` at `[ESP+0x3C]`, `0x00545200`); the center is an
    // uninitialized stack value in the original.
    ctx.centres.clear();
    ctx.centre = None;
    let (script, project) = (ctx.script, ctx.project);
    // `SX`/`SZ` (`Terrain+0x124/+0x128`) and `W`/`H` (`+0x11C/+0x120`) as integers.
    let s = ctx.tile_size as i32;
    let groups = def.num_groups > 1;
    for _ in 0..def.num_groups {
        let mut j = 0i32;
        while j < def.number {
            let guid = object_guid(script, &def.name, rng)?;
            // `Project_FindObject` (`0x00545296`) and cache instance (`0x00545323`): if one is
            // missing, continue with j + 1 without a further draw.
            if let Some(po) = project.objects.get(&guid)
                && let Some(geo) = ctx.geometry(&po.model)
            {
                let rot = ctx.rotation(po);
                let group_centre = groups && j == 0;
                for t in 0..=LAST_TRY {
                    let (x, z) = if !groups || j == 0 {
                        #[cfg(test)]
                        {
                            ctx.attempts_run[0] += 1;
                        }
                        let x = tile_corner(rng, ctx.dim, s)?;
                        let z = tile_corner(rng, ctx.dim, s)?;
                        if groups {
                            // `0x00545453`: before snapping.
                            ctx.centre = Some([x, z]);
                        }
                        (x, z)
                    } else {
                        #[cfg(test)]
                        {
                            ctx.attempts_run[1] += 1;
                        }
                        member_position(ctx, def, s, rng)?
                    };
                    let attempt = check_attempt(ctx, def, &geo, rot, x, z, group_centre)?;
                    if let Some(a) = &attempt
                        && group_centre
                    {
                        // `FUN_005410E0` appends on "yes" (`0x00541175`).
                        ctx.centres.push(a.position);
                    }
                    if t == LAST_TRY && group_centre {
                        // `0x005456B6`: even if exactly this attempt succeeded (D-338).
                        j = def.number;
                    }
                    if let Some(a) = attempt {
                        // `0x005456E2` placing, `0x00545715` `GroundTexture` (D-362).
                        let set = ctx.set_object;
                        set(ctx, guid, &a, NEUTRAL, rng);
                        ground_texture(ctx, def, po, &geo, &a);
                        break;
                    }
                }
            }
            j = j.wrapping_add(1);
        }
    }
    Ok(())
}

/// Position of a group member of the general placer (`0x0054548F…0x00545551`):
/// `a = FromInt(rand() % 361)` (R4), then [`member_around_centre`] (R5/R6). Without a drawn center
/// (first group of a block whose object `j == 0` failed, D-354) an error instead of undetermined
/// values, before R4.
fn member_position(
    ctx: &ObjectCtx<'_>,
    def: &PlaceObjectDef,
    s: i32,
    rng: &mut GenRand,
) -> anyhow::Result<(f32, f32)> {
    let centre = group_centre_of(ctx, def)?;
    let a = Fx::from_int(rng.rand() % 361).0;
    member_around_centre(centre, def, s, a, rng)
}

/// The center for a group member ([`ObjectCtx::centre`]); if it is missing (D-354), an error.
fn group_centre_of(ctx: &ObjectCtx<'_>, def: &PlaceObjectDef) -> anyhow::Result<[f32; 2]> {
    let Some(centre) = ctx.centre else {
        bail!(
            "random map: place_Object \"{}\": group member without group center - the first object \
             of the first group failed",
            def.name
        );
    };
    Ok(centre)
}

/// Member around the center with the angle `a` (fixed-point degrees): `r1 = rand() % (R+1)`,
/// `x = center.x + offset(SX, r1, SinDeg(a))`, `r2 = rand() % (R+1)`,
/// `z = center.z + offset(SZ, r2, CosDeg(a))` (sums `FADD`/`FSTP f32`); general R5/R6
/// (`0x005454AA`, `0x00545501`), `ForAllPlayers` R10/R11 (`0x00544EBE`, `0x00544F15`). With
/// `R + 1 == 0` (D-359) an error instead of division by zero.
fn member_around_centre(
    [cx, cz]: [f32; 2],
    def: &PlaceObjectDef,
    s: i32,
    a: i32,
    rng: &mut GenRand,
) -> anyhow::Result<(f32, f32)> {
    let divisor = def.max_group_radius.wrapping_add(1);
    let radius = |rng: &mut GenRand| {
        c_rem(rng.rand(), divisor).with_context(|| {
            format!(
                "random map: place_Object \"{}\": maxGroupRadius {} divides by zero",
                def.name, def.max_group_radius
            )
        })
    };
    let r1 = radius(rng)?;
    let x = cx + member_offset(s, r1, sin_deg(a));
    let r2 = radius(rng)?;
    let z = cz + member_offset(s, r2, cos_deg(a));
    Ok((x, z))
}

/// Ring position of one axis around the player area (`FUN_00544B40` `0x00544CDA…0x00544D34` or
/// `0x00544D38…0x00544D8E`): `ToFloat(Mul(FromInt(S), Mul(FromInt(d), trig) + FromInt(c)))` with
/// `trig` = `SinDeg(a)` for x and `CosDeg(a)` for z, `c` = area center in tiles (tile corner).
/// Argument order of `Fixed_Mul` as in the original; one `FSTP f32`.
pub(crate) fn ring_coord(s: i32, d: i32, trig: i32, c: i32) -> f32 {
    let along = fixed_mul(Fx::from_int(d).0, trig).wrapping_add(Fx::from_int(c).0);
    Fx(fixed_mul(Fx::from_int(s).0, along)).to_f32()
}

/// `ForAllPlayers` placer `FUN_00544B40`. `def` carries the already scaled quantities.
///
/// ```text
/// counter = 0; list of group centers and center apply across all areas of the block
/// for each player area (list order G+0x40), g in 0..numGroups, j in 0..number:
///   R1 GUID; project object missing or model not loadable -> continue (the counter counts anyway)
///   attempts t = 0 ... 100:
///     R8 d = rand() % (max - min + 1) + min;  R9 a = FromInt(rand() % 361)
///     single object or center (numGroups == 1 or j == 0): ring around (cx, cz) with d and a;
///         with groups center = this unsnapped position
///     member: d discarded; R10/R11 radius per axis around the center with the same a
///     checks as in the general placer (8 only for the group center; passed -> center into the list)
///     t == 100 and group center -> j = number (D-338)
///     passed -> place with owner = IsPlayer ? counter : 8; end of attempts
///   counter + 1, j + 1 (0x0054515B: also for failed objects, not for the members
///   skipped by j = number, D-339)
/// ```
pub(crate) fn place_for_all_players(
    ctx: &mut ObjectCtx<'_>,
    def: &PlaceObjectDef,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    // List of group centers (`CList` at `[ESP+0x48]`, created `0x00544B64`, emptied only at the end
    // `0x005451A2`) and center (`[ESP+0x3C…0x44]`, uninitialized): per block, across all
    // areas.
    ctx.centres.clear();
    ctx.centre = None;
    let (script, project, players) = (ctx.script, ctx.project, ctx.players);
    let s = ctx.tile_size as i32;
    let groups = def.num_groups > 1;
    // `INC ESI; IDIV ESI` (`0x00544CA2`) with `ESI = max − min`.
    let ring_divisor = def
        .max_dist_to_players
        .wrapping_sub(def.min_dist_to_players)
        .wrapping_add(1);
    // Owner counter `[ESP+0x1C]` (`0x00544B74`), across all areas and groups of the block.
    let mut counter = 0i32;
    for area in players {
        for _ in 0..def.num_groups {
            let mut j = 0i32;
            while j < def.number {
                let guid = object_guid(script, &def.name, rng)?;
                // `Project_FindObject` (`0x00544BEB`) and cache instance (`0x00544C73`).
                if let Some(po) = project.objects.get(&guid)
                    && let Some(geo) = ctx.geometry(&po.model)
                {
                    let rot = ctx.rotation(po);
                    let group_centre = groups && j == 0;
                    for t in 0..=LAST_TRY {
                        // R8 (`0x00544C9C`), R9 (`0x00544CB3`) in every attempt.
                        let d = c_rem(rng.rand(), ring_divisor)
                            .with_context(|| {
                                format!(
                                    "random map: place_Object \"{}\": maxDistToPlayers {} - \
                                     minDistToPlayers {} + 1 divides by zero",
                                    def.name, def.max_dist_to_players, def.min_dist_to_players
                                )
                            })?
                            .wrapping_add(def.min_dist_to_players);
                        let a = Fx::from_int(rng.rand() % 361).0;
                        let (x, z) = if !groups || j == 0 {
                            #[cfg(test)]
                            {
                                ctx.attempts_run[0] += 1;
                            }
                            let x = ring_coord(s, d, sin_deg(a), area.cx);
                            let z = ring_coord(s, d, cos_deg(a), area.cz);
                            if groups {
                                // `0x00544E6C…0x00544E83`: before snapping.
                                ctx.centre = Some([x, z]);
                            }
                            (x, z)
                        } else {
                            #[cfg(test)]
                            {
                                ctx.attempts_run[1] += 1;
                            }
                            let centre = group_centre_of(ctx, def)?;
                            member_around_centre(centre, def, s, a, rng)?
                        };
                        let attempt = check_attempt(ctx, def, &geo, rot, x, z, group_centre)?;
                        if let Some(a) = &attempt
                            && group_centre
                        {
                            ctx.centres.push(a.position);
                        }
                        if t == LAST_TRY && group_centre {
                            // `0x005450C8`: even if exactly this attempt succeeded (D-338).
                            j = def.number;
                        }
                        if let Some(a) = attempt {
                            // `0x005450DA`: `IsPlayer` -> counter, otherwise `PUSH 0x8`; not in 0...7
                            // -> 8 when placing.
                            let owner = if def.is_player { counter } else { NEUTRAL };
                            let set = ctx.set_object;
                            set(ctx, guid, &a, owner, rng);
                            // `0x00545156` `GroundTexture` (D-362).
                            ground_texture(ctx, def, po, &geo, &a);
                            break;
                        }
                    }
                }
                counter = counter.wrapping_add(1);
                j = j.wrapping_add(1);
            }
        }
    }
    Ok(())
}

/// `Fill` placer `FUN_00545760`, owner 8 (`PUSH 0x8` `0x005458FB`):
///
/// ```text
/// for x in 0..W, z in 0..H:
///   (3)/(4) at tile (x, z) (FUN_00543410), otherwise continue without a draw
///   R1 GUID; project object missing or model not loadable -> continue
///   position (x*SX, 0, z*SZ); snapping; only occupancy (7); free -> place
/// ```
///
/// `number`, `numGroups`, `PutOnFlat`, border and group distance have no effect; each tile reads the
/// tile numbers fresh, so a `GroundTexture` already affects the next one (D-362).
pub(crate) fn place_fill(
    ctx: &mut ObjectCtx<'_>,
    def: &PlaceObjectDef,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let (script, project) = (ctx.script, ctx.project);
    let s = ctx.tile_size as i32;
    // `W`/`H` = `Terrain+0x11C/+0x120` (square, `ctx.dim`).
    for x in 0..ctx.dim {
        for z in 0..ctx.dim {
            if !tile_ok(ctx, def, x, z)? {
                continue;
            }
            let guid = object_guid(script, &def.name, rng)?;
            let Some(po) = project.objects.get(&guid) else {
                continue;
            };
            let Some(geo) = ctx.geometry(&po.model) else {
                continue;
            };
            let rot = ctx.rotation(po);
            #[cfg(test)]
            {
                ctx.attempts_run[0] += 1;
            }
            // `IMUL` integer, `FILD`/`FSTP f32` (`0x0054586E…0x005458AE`).
            let px = s.wrapping_mul(x) as f32;
            let pz = s.wrapping_mul(z) as f32;
            if let Some(a) = fill_attempt(ctx, def, &geo, rot, px, pz, (x, z)) {
                let set = ctx.set_object;
                set(ctx, guid, &a, NEUTRAL, rng);
                // `0x00545944` `GroundTexture` (D-362).
                ground_texture(ctx, def, po, &geo, &a);
            }
        }
    }
    Ok(())
}

/// Object step `FUN_005468D0`: the `place_Object` blocks in script order
/// ([`ObjectCtx::script`]), per block first the quantities scaled ([`scale_place_object`], in the
/// original at the block end of the parser `0x00544984`, once per block for all three kinds), then
/// the switch: `Fill` (`+0x118`) -> [`place_fill`], otherwise `ForAllPlayers` (`+0x119`) ->
/// [`place_for_all_players`], otherwise [`place_general`]. `players` = number of players `G+0x120`
/// of the description (`RandomMapSpec::players`, **not** the number of player areas - several
/// `create_PlayerAreas` blocks create several sets), `scale` = map factor `G+0x268`.
pub(crate) fn place_objects(
    ctx: &mut ObjectCtx<'_>,
    players: i32,
    scale: Fx,
    rng: &mut GenRand,
) -> anyhow::Result<()> {
    let script = ctx.script;
    for raw in &script.placements {
        let def = scale_place_object(raw, players, scale);
        if def.fill {
            place_fill(ctx, &def, rng)?;
        } else if def.for_all_players {
            place_for_all_players(ctx, &def, rng)?;
        } else {
            place_general(ctx, &def, rng)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
