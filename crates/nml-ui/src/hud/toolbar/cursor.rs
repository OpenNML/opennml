//! Cursors of the command bar: building, walls, target and defense radius.
//!
//! Basis: re/game-ui N.1 (table of the windows), N.14.8, re/construction, section "Senders" and Ghidra (read only):
//! build cursor `FUN_0044FC80` with mouse (slot 2) `0x0044FBE0`, event (slot 3) `0x00450020` and
//! `BuildCursor_UpdatePlacement` `0x0044FDB0`; wall cursor `FUN_00451260` with event `0x004513B0`, steps `0x004504C0`
//! (center), `0x00451100` (size), `0x00450560` (move), check `0x00450870`, submitting `0x00450650` and ring
//! `FUN_00450BE0`; targeting mode `FUN_004517B0` with `0x00451890`, `0x004515B0`, `0x00451720`, `0x00451770`; defense
//! radius `FUN_00450070` with `0x00450190`.
//!
//! # Context
//!
//! In the original the cursors are children of the bar behind the buttons (`AddTail`). The reimplementation keeps a
//! [`Cursor`] in [`super::Toolbar::cursor`]; [`super::Toolbar::mouse`] gives it the raw mouse after the buttons (after
//! a silent rebuild before them, O-265). The per-frame work (frame event 6 for the build cursor, pointer movement for
//! the wall, target and defense cursors; `0x15` is the drawing for the wall cursor) is done by [`Cursor::update`] from
//! the [`CursorFrame`] that the client puts into the model each frame.
//!
//! | Raw kind | Build | Wall | Target | Defense |
//! |---|---|---|---|---|
//! | 3 left press | rotation start, consumed | consumed | consumed | consumed |
//! | 2 left release | place (valid, not dragged) | phase 1->2, 2->3 or submit | select (valid) | point |
//! | 5 right press | consumed | consumed | consumed | consumed |
//! | 4 right release | all cursors gone (`{0x0A}`) | like build | cancel (`{0x17}`, full build) | `{0x30}` |
//!
//! The reimplementation's cursor never consumes the movement (1) (the original consumes it for wall, target and
//! defense; in the reimplementation the world needs the movement for the selection box and hints). Both drawing methods
//! of the vtables `0x00619BE8` and `0x00619C90` are empty (`0x004EC990` = `RET`): the defense ring is a terrain decal
//! of the engine (`defenceradius.tga`, `0x0042F750`), the target cursor draws nothing.
//!
//! # After placing
//!
//! After every placement (building, tent, wall) and after `{0x2F}`, the bar closes all cursors (`FUN_00452C10`),
//! without `{0x0A}` and without a rebuild, also with Shift. Target selected and canceled (`0x16`, `0x17`) rebuild
//! in full.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::hud::HudEventKind;

use super::button::ToolbarOut;
use super::window::ToolbarIntent;

/// Smallest side of a ring in tiles from which it is submitted (`|dx| > 3 && |dz| > 3`, `0x00450650`).
const MIN_SIDE: i32 = 4;
/// `dx`, `dz` when dragging at most +/-22 (`FUN_00451100`, constants `0x00619C40`/`0x00619C44`).
const MAX_SIDE: f32 = 22.0;
/// Dragging while rotating: more than 10 px sets "dragged" (`0x0044FDEA`).
const DRAG_PX: i32 = 10;
/// Pixels per quarter turn (`0x0044FDF6`: division by 50).
const ROTATE_PX: i32 = 50;

/// A wall corner or wall piece of the ring: GUID, rotation in radians and world point (the five words per part of
/// event `0x14`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallPart {
    pub guid: u32,
    pub rotation: f32,
    pub pos: [f32; 3],
}

/// Queries to the world for the open cursor, supplied by the client each frame (frame event 6). Only the client
/// knows the checks (`GameEngine_CheckBuildingPlacement`, sight, `targets.ini`).
///
/// `valid` and `ring_valid` apply to the position that the cursor assumed in the **previous** `update` (the client
/// reads the position and rotation, or the parts of the ring, from the cursor, checks them and puts the result into
/// the model of the next frame). The cursor only counts the validity if its position has not changed since; whoever
/// moves in the same frame can therefore only place in the next one. The original checks in the same step
/// (`0x0044FE72` ... `0x0044FFC8`, `0x004513E6`); once the pointer is at rest the result is the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorFrame {
    /// World point under the pointer (`Cursor3D`, `0x00671B00`); `None` if the pointer does not hit the ground.
    ///
    /// **Building and tent:** the **snapped** point: the original sets the position of the build cursor to
    /// `BuildCursor_GetSnappedPosition` (`0x0044FE3D` -> `0x004CD240`), which snaps the minimum corner of the hull of
    /// the type, at the rotation of the cursor, to the tile boundary (re/construction 5.5). The client does the
    /// snapping with the rotation from [`BuildCursor::rotation`] (`nml_sim::placement::cursor_placement`); the cursor
    /// takes the point over unchanged. Wall, target and defense get the raw world point (the wall snaps to the tile
    /// center itself).
    pub world: Option<[f32; 3]>,
    /// Pointer in UI pixels (rotating the build cursor reads `y`).
    pub mouse: [i32; 2],
    /// Building/tent: placement check **and** visible. Target: the target is valid (action for the object under the
    /// pointer or ground allowed, `targets.ini`). Defense: point on explored terrain (ring visible).
    pub valid: bool,
    /// Wall: all parts of the ring valid (`BuildCursor_CheckWallSegments`, `0x00450870`); `None` without a verdict.
    pub ring_valid: Option<bool>,
    /// Tile size of the world (`GameWorld+0x34/+0x3c`, 128 x 128 in the shipped data).
    pub tile: [i32; 2],
    /// Building: the type is rotatable. The original checks `Flags(40003) & 0xFFFFFF00 & 0x10000003`; the constructor
    /// clears the low byte, so only the gate bit `0x10000000` counts (O-279).
    pub rotatable: bool,
    /// Target: HID of the object under the pointer if the target is an object (`FUN_0047A6B0`).
    pub object: Option<u32>,
}

impl Default for CursorFrame {
    /// Empty frame: no world point, nothing valid, tile 128 x 128, rotatable.
    fn default() -> Self {
        Self {
            world: None,
            mouse: [0, 0],
            valid: false,
            ring_valid: None,
            tile: [128, 128],
            rotatable: true,
            object: None,
        }
    }
}

/// Build cursor (`FUN_0044FC80`) and tent cursor (kind `0x1A`, button 453).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildCursor {
    /// Type GUID of the building (`+0x40`); for the tent, that of the tent.
    pub guid: u32,
    /// Tent: HID of the unit that unpacks (first selected object). `None` for an ordinary building.
    pub tent: Option<u32>,
    /// The type is rotatable (from [`CursorFrame::rotatable`]).
    pub rotatable: bool,
    /// Rotation in radians (`+0x64`).
    pub rotation: f32,
    /// Key held, rotating is running (`+0x78`).
    pub dragging: bool,
    /// Moved more than 10 px while rotating; the release then does not place (`+0x84`).
    pub dragged: bool,
    /// Pointer `y` on pressing, until then the last position (`+0x80`).
    pub start_y: i32,
    /// Rotation on pressing (`+0x68`).
    pub start_rotation: f32,
    /// Position (`+0x4C`): last snapped world point ([`CursorFrame::world`]) that did not arise while rotating.
    pub pos: [f32; 3],
    /// Placement and sight check of the client (`+0x3C`).
    pub valid: bool,
}

impl BuildCursor {
    /// Build cursor for a building: rotation 0, nothing valid.
    pub fn new(guid: u32) -> Self {
        Self {
            guid,
            tent: None,
            rotatable: true,
            rotation: 0.0,
            dragging: false,
            dragged: false,
            start_y: 0,
            start_rotation: 0.0,
            pos: [0.0; 3],
            valid: false,
        }
    }

    /// Tent cursor (kind `0x1A`) for the tent `guid` of the unit `unit`.
    pub fn tent(guid: u32, unit: u32) -> Self {
        Self {
            tent: Some(unit),
            ..Self::new(guid)
        }
    }

    /// Frame event 6, `BuildCursor_UpdatePlacement` (`0x0044FDB0`): while rotating, `delta_y = y_press - y`, more than
    /// 10 px sets "dragged", the rotation is start + 90 degrees * ((delta_y / 50) mod 4) with the remainder of the
    /// dividend (C); types without rotation stay at 0. Otherwise the position follows the world point and `y` is
    /// remembered as the press point.
    fn update(&mut self, f: &CursorFrame) {
        // Position for which the client checked the validity (state after the previous `update`).
        let checked = (self.pos, self.rotation);
        self.rotatable = f.rotatable;
        if self.dragging {
            if self.rotatable {
                let dy = self.start_y - f.mouse[1];
                if dy.abs() > DRAG_PX {
                    self.dragged = true;
                }
                let quarters = (dy / ROTATE_PX) % 4;
                self.rotation = quarters as f32 * FRAC_PI_2 + self.start_rotation;
            } else {
                self.rotation = 0.0;
            }
        } else {
            if let Some(w) = f.world {
                self.pos = w;
            }
            self.start_y = f.mouse[1];
        }
        // The check only counts for the position that it checked.
        self.valid = f.valid && (self.pos, self.rotation) == checked;
    }

    fn mouse(&mut self, raw: HudEventKind, out: &mut Vec<ToolbarOut>) -> Option<CursorEnd> {
        match raw {
            // Slot 2, kind 3: rotation start, unless rotating is already in progress.
            HudEventKind::LeftDown => {
                if !self.dragging {
                    self.start_rotation = self.rotation;
                }
                self.dragging = true;
                Some(CursorEnd::Keep)
            }
            // Kind 2: place with a valid position and without dragging; otherwise only reset "dragged". Always
            // consumed.
            HudEventKind::LeftUp => {
                self.dragging = false;
                if self.valid && !self.dragged {
                    let intent = match self.tent {
                        Some(unit) => ToolbarIntent::PlaceTent {
                            unit,
                            pos: [self.pos[0], self.pos[2]],
                        },
                        None => ToolbarIntent::PlaceBuilding {
                            guid: self.guid,
                            rotation: self.rotation,
                            pos: self.pos,
                        },
                    };
                    out.push(ToolbarOut::Intent(intent));
                    Some(CursorEnd::Close)
                } else {
                    self.dragged = false;
                    Some(CursorEnd::Keep)
                }
            }
            // Slot 3: 5 consumed, 4 -> `{0x0A}`.
            HudEventKind::RightDown => Some(CursorEnd::Keep),
            HudEventKind::RightUp => Some(CursorEnd::Close),
            _ => None,
        }
    }
}

/// Phase of the wall cursor (`+0x8C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FortPhase {
    /// 1: the center follows the pointer (tile center); a corner piece is shown.
    Place,
    /// 2: center is fixed, `dx`, `dz` follow the pointer.
    Size,
    /// 3: size is fixed, the ring follows the pointer in tile steps.
    Move,
}

/// Wall cursor (`FUN_00451260`) for walls and palisades: ring of corners and wall pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct FortCursor {
    /// Wall piece GUID (`+0x68`).
    pub wall: u32,
    /// Corner GUID (`+0x64` = wall + 1).
    pub corner: u32,
    pub phase: FortPhase,
    /// Center of the ring (`+0x74`): tile center in `x`/`z`, height of the pointer point.
    pub center: [f32; 3],
    /// Reference point in phase 3 (`+0x80`).
    pub end: [f32; 3],
    /// Side lengths in tiles, signed, +/-22 (`+0x6C`, `+0x70`); 4 x 4 at the start.
    pub dx: i32,
    pub dz: i32,
    /// Tile size (from the last frame).
    pub tile: [i32; 2],
    /// Parts of the ring in ring order with world positions (part 0 a corner); in phase 1 only this one corner piece.
    /// The client checks them and reports the verdict as [`CursorFrame::ring_valid`]; they are drawn as a preview.
    pub parts: Vec<WallPart>,
    /// Verdict of the client over all parts (`None` without).
    pub ring_valid: Option<bool>,
    /// Last world point under the pointer.
    pub point: Option<[f32; 3]>,
}

impl FortCursor {
    /// Wall cursor for the wall piece `wall`; the corner is `wall + 1` (1011 -> 1012, 0x3F3 -> 0x3F4 ...).
    pub fn new(wall: u32) -> Self {
        Self {
            wall,
            corner: wall + 1,
            phase: FortPhase::Place,
            center: [0.0; 3],
            end: [0.0; 3],
            dx: MIN_SIDE,
            dz: MIN_SIDE,
            tile: [128, 128],
            parts: Vec::new(),
            ring_valid: None,
            point: None,
        }
    }

    /// The ring is large enough (`|dx| > 3 && |dz| > 3`).
    pub fn big_enough(&self) -> bool {
        self.dx.abs() >= MIN_SIDE && self.dz.abs() >= MIN_SIDE
    }

    /// Pointer movement (event 1 of `0x004513B0`; `0x15` is the drawing there): phase 1 `0x004504C0`, phase 2
    /// `0x00451100`, phase 3 `0x00450560`, each with a subsequent check (the verdict comes from the client and only
    /// counts for the parts that it checked).
    fn update(&mut self, f: &CursorFrame) {
        let checked = self.parts.clone();
        self.tile = f.tile;
        self.ring_valid = f.ring_valid;
        self.update_geometry(f);
        if self.parts != checked {
            self.ring_valid = None;
        }
    }

    fn update_geometry(&mut self, f: &CursorFrame) {
        let Some(p) = f.world else {
            return;
        };
        self.point = Some(p);
        match self.phase {
            FortPhase::Place => {
                self.center = [snap(p[0], f.tile[0]), p[1], snap(p[2], f.tile[1])];
                self.parts = vec![WallPart {
                    guid: self.corner,
                    rotation: 0.0,
                    pos: self.center,
                }];
            }
            FortPhase::Size => {
                self.dx = side(p[0], self.center[0], f.tile[0]);
                self.dz = side(p[2], self.center[2], f.tile[1]);
                self.rebuild_ring();
            }
            FortPhase::Move => {
                // The ring follows the pointer: center += pointer - reference, snapped to the tile center; the
                // reference remembers the remainder of the snapping so that small movements add up.
                let c0 = [
                    self.center[0] + (p[0] - self.end[0]),
                    self.center[1] + (p[1] - self.end[1]),
                    self.center[2] + (p[2] - self.end[2]),
                ];
                let c1 = [snap(c0[0], f.tile[0]), c0[1], snap(c0[2], f.tile[1])];
                self.center = c1;
                self.end = [
                    p[0] + (c1[0] - c0[0]),
                    p[1] + (c1[1] - c0[1]),
                    p[2] + (c1[2] - c0[2]),
                ];
                self.rebuild_ring();
            }
        }
    }

    fn rebuild_ring(&mut self) {
        self.parts = ring(
            self.wall,
            self.corner,
            self.center,
            self.tile,
            self.dx,
            self.dz,
        );
    }

    fn mouse(&mut self, raw: HudEventKind, out: &mut Vec<ToolbarOut>) -> Option<CursorEnd> {
        match raw {
            HudEventKind::LeftDown | HudEventKind::RightDown => Some(CursorEnd::Keep),
            HudEventKind::RightUp => Some(CursorEnd::Close),
            // Kind 2 (`0x00451422`): phase 1 -> 2; phase 2 -> 3 if at least 4 x 4, otherwise `{0x0A}`; phase 3 ->
            // submit (`0x00450650`) for a large ring and valid parts.
            HudEventKind::LeftUp => match self.phase {
                FortPhase::Place => {
                    self.phase = FortPhase::Size;
                    Some(CursorEnd::Keep)
                }
                FortPhase::Size => {
                    if !self.big_enough() {
                        return Some(CursorEnd::Close);
                    }
                    self.phase = FortPhase::Move;
                    self.end = self.point.unwrap_or(self.center);
                    Some(CursorEnd::Keep)
                }
                FortPhase::Move => {
                    if self.big_enough() && self.ring_valid == Some(true) {
                        let parts = self.parts.clone();
                        out.push(ToolbarOut::Intent(ToolbarIntent::PlaceWall { parts }));
                        Some(CursorEnd::Close)
                    } else {
                        Some(CursorEnd::Keep)
                    }
                }
            },
            _ => None,
        }
    }
}

/// Targeting mode (`FUN_004517B0`) of the TARGET button.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetCursor {
    /// `BUTTONMSG` of the button.
    pub msg: i32,
    /// `MAUSICONID` from `targets.ini`.
    pub mouse_icon: i32,
    /// The target under the pointer is valid (`+0x50 != 0`).
    pub valid: bool,
    /// Object under the pointer if the target is an object.
    pub object: Option<u32>,
    /// Ground point `(x, z)` under the pointer.
    pub ground: Option<[f32; 2]>,
}

impl TargetCursor {
    pub fn new(msg: i32, mouse_icon: i32) -> Self {
        Self {
            msg,
            mouse_icon,
            valid: false,
            object: None,
            ground: None,
        }
    }

    fn update(&mut self, f: &CursorFrame) {
        self.valid = f.valid;
        self.object = f.object;
        self.ground = f.world.map(|w| [w[0], w[2]]);
    }

    fn mouse(&mut self, raw: HudEventKind, out: &mut Vec<ToolbarOut>) -> Option<CursorEnd> {
        match raw {
            HudEventKind::LeftDown | HudEventKind::RightDown => Some(CursorEnd::Keep),
            // Kind 2 (`0x00451720`): with a valid target `{0x16, ...}`; always consumed. Kind 4 (`0x00451770`)
            // `{0x17}`.
            // Both rebuild the bar in full.
            HudEventKind::LeftUp => {
                if !self.valid {
                    return Some(CursorEnd::Keep);
                }
                let ground = if self.object.is_none() {
                    self.ground
                } else {
                    None
                };
                out.push(ToolbarOut::Intent(ToolbarIntent::TargetChosen {
                    msg: self.msg,
                    object: self.object,
                    ground,
                }));
                Some(CursorEnd::Rebuild)
            }
            HudEventKind::RightUp => Some(CursorEnd::Rebuild),
            _ => None,
        }
    }
}

/// Defense radius cursor (`FUN_00450070`, button 421 on non-buildings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefenseCursor {
    /// World point `(x, z)` under the pointer.
    pub point: Option<[f32; 2]>,
    /// The point is on explored terrain: only then does the engine show the ring (`0x004501AF`).
    pub explored: bool,
}

impl DefenseCursor {
    fn update(&mut self, f: &CursorFrame) {
        if let Some(w) = f.world {
            self.point = Some([w[0], w[2]]);
        }
        self.explored = f.valid;
    }

    fn mouse(&mut self, raw: HudEventKind, out: &mut Vec<ToolbarOut>) -> Option<CursorEnd> {
        match raw {
            HudEventKind::LeftDown | HudEventKind::RightDown => Some(CursorEnd::Keep),
            // Kind 2 (`0x00450295`): `{0x2F, x, z}` with the point under the pointer, then `0x2F` closes the cursor.
            HudEventKind::LeftUp => match self.point {
                Some([x, z]) => {
                    out.push(ToolbarOut::Intent(ToolbarIntent::DefensePoint { x, z }));
                    Some(CursorEnd::Close)
                }
                None => Some(CursorEnd::Keep),
            },
            // Kind 4: `{0x30}`.
            HudEventKind::RightUp => Some(CursorEnd::Close),
            _ => None,
        }
    }
}

/// An open cursor of the bar (`+0x51C` building/walls, `+0x520` target, `+0x40` defense).
#[derive(Debug, Clone, PartialEq)]
pub enum Cursor {
    Build(BuildCursor),
    Fort(FortCursor),
    Target(TargetCursor),
    Defense(DefenseCursor),
}

/// How the bar ends a cursor after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CursorEnd {
    /// Cursor stays.
    Keep,
    /// All cursors closed, no rebuild (placing, `{0x0A}`, `{0x30}`).
    Close,
    /// All cursors closed and full build (`0x16`, `0x17`).
    Rebuild,
}

impl Cursor {
    /// Frame event 6 with the state of the world of this frame.
    pub fn update(&mut self, f: &CursorFrame) {
        match self {
            Cursor::Build(c) => c.update(f),
            Cursor::Fort(c) => c.update(f),
            Cursor::Target(c) => c.update(f),
            Cursor::Defense(c) => c.update(f),
        }
    }

    /// Raw mouse; `None` if the cursor does not take it.
    pub(super) fn mouse(
        &mut self,
        raw: HudEventKind,
        out: &mut Vec<ToolbarOut>,
    ) -> Option<CursorEnd> {
        match self {
            Cursor::Build(c) => c.mouse(raw, out),
            Cursor::Fort(c) => c.mouse(raw, out),
            Cursor::Target(c) => c.mouse(raw, out),
            Cursor::Defense(c) => c.mouse(raw, out),
        }
    }
}

/// Snaps `v` to the center of the tile (`0x004504C0`): `floor(v / T) * T + T / 2`.
fn snap(v: f32, tile: i32) -> f32 {
    let t = f64::from(tile);
    // `T / 2` as a signed integer division toward zero, as the compiler of the original writes it:
    // `(T - (T >> 31)) >> 1` first adds 1 for a negative T, then shifts. For the tile width (positive, even)
    // this is exactly `T / 2`.
    let half = (tile - (tile >> 31)) >> 1;
    ((f64::from(v) / t).floor() * t + f64::from(half)) as f32
}

/// Side length in tiles from the distance to the center (`0x00451100`): divided by the tile size, clamped to +/-22,
/// truncated toward zero.
fn side(p: f32, center: f32, tile: i32) -> i32 {
    let v = ((f64::from(p) - f64::from(center)) / f64::from(tile)) as f32;
    v.clamp(-MAX_SIDE, MAX_SIDE) as i32
}

/// Ring of the wall cursor around `center` (`FUN_00450BE0`): part 0 corner, `|dx|` wall pieces, corner, `|dz|` pieces,
/// corner, `|dx|` back, corner, `|dz|` back, so `4 + 2|dx| + 2|dz|` parts. The signs of `dx` (<= 0 counts as -) and
/// `dz` choose direction and initial rotation (pi, 2pi, +/-pi/2); everything in `f32` in the calculation order of the
/// original. The positions are `center` plus offset (the offset grows by repeatedly adding the tile width, as there).
pub fn ring(
    wall: u32,
    corner: u32,
    center: [f32; 3],
    tile: [i32; 2],
    dx: i32,
    dz: i32,
) -> Vec<WallPart> {
    let (tx, tz) = (tile[0] as f32, tile[1] as f32);
    let (sx, sz) = (
        if dx > 0 { 1.0f32 } else { -1.0 },
        if dz > 0 { 1.0f32 } else { -1.0 },
    );
    // Initial rotation f9, step f38 (+/-1 * pi/2) and corner offset f40 per sign position (`0x00450C83`-`0x00450D1A`).
    let (mut f9, unit, f40) = match (dx > 0, dz > 0) {
        (true, true) => (PI, 1.0f32, -FRAC_PI_2),
        (true, false) => (TAU, -1.0, 0.0),
        (false, false) => (TAU, 1.0, -FRAC_PI_2),
        (false, true) => (PI, -1.0, 0.0),
    };
    let f38 = unit * FRAC_PI_2;
    let (nx, nz) = (dx.unsigned_abs() as usize, dz.unsigned_abs() as usize);
    let mut parts = Vec::with_capacity(4 + 2 * nx + 2 * nz);
    let mut push = |guid: u32, off: [f32; 3], rotation: f32| {
        parts.push(WallPart {
            guid,
            rotation,
            pos: [center[0] + off[0], center[1] + off[1], center[2] + off[2]],
        });
    };
    push(corner, [0.0; 3], f40 + f9);
    // Side 1 in x.
    let mut px = tx * sx;
    for _ in 0..nx {
        push(wall, [px, 0.0, 0.0], f9);
        px += tx * sx;
    }
    f9 += f38;
    push(corner, [px, 0.0, 0.0], f40 + f9);
    // Side 2 in z.
    let mut pz = tz * sz;
    for _ in 0..nz {
        push(wall, [px, 0.0, pz], f9);
        pz += tz * sz;
    }
    f9 += f38;
    push(corner, [px, 0.0, pz], f40 + f9);
    // Side 3 back in x: first one step, then the pieces (`FSUBR` before the loop).
    px -= tx * sx;
    for _ in 0..nx {
        push(wall, [px, 0.0, pz], f9);
        px -= tx * sx;
    }
    f9 += f38;
    push(corner, [px, 0.0, pz], f40 + f9);
    // Side 4 back in z.
    pz -= tz * sz;
    for _ in 0..nz {
        push(wall, [px, 0.0, pz], f9);
        pz -= tz * sz;
    }
    parts
}

#[cfg(test)]
mod tests;
