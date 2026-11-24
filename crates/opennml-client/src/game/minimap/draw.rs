//! Draw list of the minimap (`0x00449C30`; re/game-ui D5.1, O-39, O-288).
//!
//! Order as in the original: frame `map_a_01.tga` -> terrain with fog (`0x004E3B70`) -> border
//! `map_umrandung.tga` -> fog memory dots (`FogMemory_DrawMinimapDots` `0x00416760`) -> object dots (`0x00480210`) ->
//! pings ([`super::pings`]) -> camera frame (`0x004E4290`); afterwards `Hud::draw_windows` draws the corner buttons
//! as children of the node.
//!
//! The map is rotated by theta ([`MinimapLayout`]). Terrain, border, dots and camera frame therefore go out as
//! [`DrawCmd::Quad4`] with floating-point corners (deviation from the original: it draws rotated strips and lines).
//! Theta is calculated by `f32::atan2`, `cos` and `sin` (deviation from the original): presentation and click
//! conversion only, no simulation state.

use glam::Vec3;
use opennml_sim::object::status;
use opennml_ui::bkg_paint::image_quad;
use opennml_ui::coords::{Screen, minimap_rect};
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::{DrawCmd, Filter, TextureId, line};
use opennml_world::camera::GameCamera;
use opennml_world::markers::marker_argb;

use super::Minimap;
use crate::game::sim_host::SimHost;

/// Frame of the minimap (string `0x00619AB4`), drawn as a 176-texel tile.
pub const FRAME_IMAGE: &str = "data/icons/map_a_01.tga";
/// Border of the diamond (string `0x00620644`), stretched to UV 0...1 over the four terrain corners.
pub const BORDER_IMAGE: &str = "data/icons/map_umrandung.tga";
/// Fixed path under which the front fog image lies in the image collection ([`Minimap::texture`]).
pub const FOG_TEXTURE: &str = "<minimap>/front";
/// Tile of the frame (`0x0044A75D...0x0044A782`): 176 x 176.
pub const FRAME_TILE: [i32; 2] = [176, 176];
/// Side length of the diamond in screen pixels (`0x006158F8` = 128.0), for every map size.
pub const MAP_SIDE: f32 = 128.0;
/// Distance of the map center from the right and bottom screen edge (`0x00557690`: `W - 89`, `H - 89`); the window
/// is 176 wide, so the center lies 1 px left of and above its center (O-39).
pub const CENTER_INSET: f32 = 89.0;
/// White (`0xFFFFFFFF`): frame, terrain, border (diffuse) and camera frame (`Lines`).
pub const WHITE: u32 = 0xFFFF_FFFF;
/// Gold deposit 3500 as a fog memory dot (`0x00416760`).
pub const GOLD_3500: (u32, u32) = (3500, 0xFFD7_D724);
/// Resource 3503 as a fog memory dot (`0x00416760`).
pub const GOLD_3503: (u32, u32) = (3503, 0xFF50_5000);
/// Slot of the trees (fog memories without a dot) and of the resources.
const TREE_SLOT: u8 = 9;
const RESOURCE_SLOT: u8 = 10;
/// Whole texture in the corner order of [`MinimapLayout::corners`].
const FULL_UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Position of the diamond of a frame (`0x004E3B70`): center, angle theta, scale per axis and map size. The matrix
/// stays until the next drawn frame ([`Minimap::last_layout`]); dots and clicks calculate with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapLayout {
    /// `(W - 89, H - 89)`.
    pub center: [f32; 2],
    /// `theta = atan2(eye.z - target.z, eye.x - target.x) + pi/2` (`Camera_Apply` `0x004EB920`, `Camera+0x18`).
    pub theta: f32,
    /// `128 / dimX`.
    pub sx: f32,
    /// `128 / dimZ`.
    pub sz: f32,
    /// Map `dimX x dimZ` in tiles (for `hx = dimX/2`, `hz = dimZ/2`).
    pub dim: [u32; 2],
}

impl MinimapLayout {
    /// Tile (floating point) -> screen like the minimap matrix (D3DX row vector): `u = mx - dimX/2`,
    /// `v = dimZ/2 - mz`, `x = x0 + sx*c*u - sz*s*v`, `y = y0 + sx*s*u + sz*c*v`.
    pub fn tile_to_screen(&self, mx: f32, mz: f32) -> [f32; 2] {
        let u = mx - self.dim[0] as f32 * 0.5;
        let v = self.dim[1] as f32 * 0.5 - mz;
        self.matrix(u, v)
    }

    /// The vector `(u, v, 0)` through the minimap matrix (`Vector3_TransformCoord` with `Minimap+0x0C`):
    /// `M._11 = sx*c`, `M._12 = sx*s`, `M._21 = -sz*s`, `M._22 = sz*c`, `M._41/42 = center`.
    fn matrix(&self, u: f32, v: f32) -> [f32; 2] {
        let (s, c) = self.theta.sin_cos();
        [
            u * (self.sx * c) + v * (-self.sz * s) + self.center[0],
            u * (self.sx * s) + v * (self.sz * c) + self.center[1],
        ]
    }

    /// Screen -> tile (floating point) like `0x004E4840`: if the point lies in one of the two halves of the diamond,
    /// the inverse matrix (without translation, on `p - center`) calculates back: `u = (dx*c + dy*s)/sx`,
    /// `v = (-dx*s + dy*c)/sz`, `mx = dimX/2 + u`, `mz = dimZ/2 - v`. Otherwise `None`.
    ///
    /// The halves are the triangles `(P(-hx, hz), P(-hx, -hz), P(hx, hz))` and `(P(-hx, -hz), P(hx, hz),
    /// P(hx, -hz))` of the matrix vectors `(u, v)` with `hx = dimX/2`, `hz = dimZ/2` ([`point_in_triangle`]). The
    /// original forms the inverse matrix with `Matrix_InverseAffine`; the reimplementation takes the closed form
    /// (equal to within a few ulp, presentation and clicks only, deviation from the original).
    pub fn screen_to_tile(&self, p: [f32; 2]) -> Option<(f32, f32)> {
        let hx = self.dim[0] as f32 * 0.5;
        let hz = self.dim[1] as f32 * 0.5;
        let b0 = self.matrix(-hx, -hz);
        let p58 = self.matrix(-hx, hz);
        let p64 = self.matrix(hx, -hz);
        let bc = self.matrix(hx, hz);
        if !point_in_triangle(p, p58, b0, bc) && !point_in_triangle(p, b0, bc, p64) {
            return None;
        }
        let (s, c) = self.theta.sin_cos();
        let dx = p[0] - self.center[0];
        let dy = p[1] - self.center[1];
        let u = (dx * c + dy * s) / self.sx;
        let v = (-dx * s + dy * c) / self.sz;
        // `(v - hz) * (-1)` of the original, the same as `hz - v`.
        Some((hx + u, hz - v))
    }

    /// World position (x, z) -> screen via the tile `(x / SIZE_X, z / SIZE_Z)` (`0x0047AA60`, continuous, no rounding).
    pub fn world_to_screen(&self, x: f32, z: f32, cell: [i32; 3]) -> [f32; 2] {
        self.tile_to_screen(x / cell[0] as f32, z / cell[2] as f32)
    }

    /// The four corners of the diamond clockwise from "top left" of the unrotated map: tiles (0, dimZ), (dimX,
    /// dimZ), (dimX, 0), (0, 0); UV (0,0), (1,0), (1,1), (0,1) - the top image row is the largest z
    /// (`0x004E3B70`: P1 ... P4 with the same UV).
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let [dx, dz] = self.dim.map(|d| d as f32);
        [
            self.tile_to_screen(0.0, dz),
            self.tile_to_screen(dx, dz),
            self.tile_to_screen(dx, 0.0),
            self.tile_to_screen(0.0, 0.0),
        ]
    }
}

/// Target sum of the angles (`0x00631FC8`: 6.28 as `double`, not 2*pi).
#[allow(clippy::approx_constant)]
const ANGLE_SUM: f64 = 6.28;
/// Margin of the angle sum (`0x00631FC0`: 0.005).
const ANGLE_TOLERANCE: f64 = 0.005;

/// Point in triangle by the angle sum (`FUN_00577010`): the directions from the point to the three corners
/// normalized (`Vector3_Normalize` `0x00575170`: `v * (1 / |v|)` without a check for 0), the three pairwise dot
/// products limited to -1 ... 1, `acos` summed; inside if `|sum - 6.28| < 0.005` (6.28 instead of 2*pi, both
/// `double`).
///
/// So the edges count (sum pi + remainder = 2*pi) and points up to about 0.2 px outside an edge. If the point lies
/// **exactly on a corner**, the direction there is `0 * inf = NaN`; NaN survives the limiting (the comparisons are
/// unordered) and the sum, the triangle reports "outside" (O-387). The reimplementation calculates the sum in `f64`
/// instead of 80 bits (deviation from the original).
fn point_in_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    // `Vector3_Subtract` in `float`, length and reciprocal on the FPU stack (here `f64`), each component as `float`.
    let dir = |q: [f32; 2]| {
        let (x, y) = (f64::from(p[0] - q[0]), f64::from(p[1] - q[1]));
        let inv = 1.0 / (x * x + y * y).sqrt();
        [(inv * x) as f32, (inv * y) as f32]
    };
    let (va, vb, vc) = (dir(a), dir(b), dir(c));
    // `Vector3_Dot` (`0x005750B0`), stored as `float`.
    let dot = |m: [f32; 2], n: [f32; 2]| {
        let d = (f64::from(m[0]) * f64::from(n[0]) + f64::from(m[1]) * f64::from(n[1])) as f32;
        // `FCOMP 1.0` / `FCOMP -1.0`: only an ordered comparison limits, NaN stays (like `clamp`).
        d.clamp(-1.0, 1.0)
    };
    let sum = f64::from(dot(va, vb)).acos()
        + f64::from(dot(vb, vc)).acos()
        + f64::from(dot(vc, va)).acos();
    (sum - ANGLE_SUM).abs() < ANGLE_TOLERANCE
}

/// Position of the diamond for a frame `size` (W x H), the game camera `camera` and the map `dim` (re/game-ui D5.1).
pub fn layout(size: (u32, u32), camera: &GameCamera, dim: (u32, u32)) -> MinimapLayout {
    let d = camera.eye - camera.target;
    MinimapLayout {
        center: [size.0 as f32 - CENTER_INSET, size.1 as f32 - CENTER_INSET],
        theta: d.z.atan2(d.x) + std::f32::consts::FRAC_PI_2,
        sx: MAP_SIDE / dim.0.max(1) as f32,
        sz: MAP_SIDE / dim.1.max(1) as f32,
        dim: [dim.0, dim.1],
    }
}

/// A dot of the minimap: position in tiles (continuous) and color (D3DCOLOR).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapDot {
    pub tile: [f32; 2],
    pub color: u32,
}

/// What the host delivers for a frame of the minimap (analogous to `BarFrame`): position, dots, camera frame and
/// the texture of the front fog image.
#[derive(Debug, Clone, PartialEq)]
pub struct MinimapFrame {
    pub layout: MinimapLayout,
    /// Fog memory and object dots in draw order ([`dots`]).
    pub dots: Vec<MinimapDot>,
    /// Ring points of the pings in list order, alpha in the color ([`super::MinimapPings::dots`]).
    pub pings: Vec<MinimapDot>,
    /// Camera frame in screen pixels ([`camera_frame`]).
    pub camera: [[f32; 2]; 4],
    /// Front fog image ([`Minimap::texture`]).
    pub texture: TextureId,
}

/// Module 3 (building) with one of the class bits `bits` like `GameObject_MatchesClass(bits | 3, 0, 0)`
/// (`0x0046B9D0`, `opennml_sim::filter::matches_class`), without `&mut Sim`: one bit suffices, switched-off modules
/// count (`PROPID` 0), quartered and dead objects do not match.
fn building_class(o: &opennml_sim::object::GameObject, bits: u32) -> bool {
    o.status & (status::DIED | status::QUARTERED) == 0
        && o.has_module(3, true)
        && o.module(3).is_some_and(|m| m.base.flags & bits != 0)
}

/// Does the object belong on the map (`0x00480210`) in map mode `mode` (`GameWorld+0x22D4`)?
/// - 0 all;
/// - 1 module 8, module `0x37`, class `0x2003`, class `0x1003` or module 6 (resources are in slot 10, which the
///   loop never reaches: the branch has no effect, O-288);
/// - 2 module `0x11` without module 8.
///
/// `GameObject_HasModule(n, 0)`: only switched-on modules.
fn shown_in_mode(o: &opennml_sim::object::GameObject, mode: u8) -> bool {
    match mode {
        0 => true,
        1 => {
            o.has_module(8, false)
                || o.has_module(0x37, false)
                || building_class(o, 0x2000)
                || building_class(o, 0x1000)
                || o.has_module(6, false)
        }
        2 => o.has_module(0x11, false) && !o.has_module(8, false),
        _ => false,
    }
}

/// The dots of a frame for the active player of the host in map mode `mode`, in draw order:
///
/// 1. **Fog memory dots** (`FogMemory_DrawMinimapDots` `0x00416760`, mode 0 only): every fog memory with a snapshot
///    (`+0x1C2`) whose position is explored (`0x0047B840(position, -1, 0)`, with the view switch), at its world
///    position. Slot 10 only gold 3500 `FFD7D724` and 3503 `FF505000`, and only while the object lives; slot 9
///    never; otherwise the **current** player color of the slot from the ID (`GameWorld_GetPlayerColor`, like
///    `opennml_sim::setup::player_color`). Deviations: the memories come in ID order instead of the order of the sector
///    grid (affects only dots lying on top of each other); movable memories (tents) are not in the sector grid in
///    the original and get no dot (hypothesis).
/// 2. **Object dots** (`0x00480210`): slots 0-8 in table order, without `DIED`, with render bit 0 for the local
///    player (`GameObject_GetRenderBitsForPlayer(-1, 0) & 1`, here
///    [`crate::game::fog_view::render_bits_for`] without a fog memory; the same for bit 0), filtered by the mode
///    ([`shown_in_mode`]), at the continuous display position ([`SimHost::display`],
///    `GameObject_GetWorldPosition`), color `0x0061C268[min(color index, 8)]`. According to Ghidra, the original
///    checks neither `QUARTERED` nor the hiding of whole slots; occupants keep render bit 0 and lie at the grid
///    position of their container, so their dot coincides with that of the building (no extra check).
pub fn dots(mode: u8, host: &SimHost) -> Vec<MinimapDot> {
    let state = &host.state;
    let map = &host.rules.map;
    let view = host.fog.view(state);
    let player = i32::from(host.active_player);
    let tile = |pos: [f32; 3]| [pos[0] / map.cell[0] as f32, pos[2] / map.cell[2] as f32];
    let mut out = Vec::new();
    if mode == 0 {
        for (id, m) in host.memories.iter() {
            if m.snapshot.is_none()
                || m.movable
                || !view.world_pos_explored(map, m.position, player, false)
            {
                continue;
            }
            let color = match id.slot() {
                TREE_SLOT => None,
                RESOURCE_SLOT => state.object(id, true).and_then(|o| {
                    [GOLD_3500, GOLD_3503]
                        .into_iter()
                        .find(|&(guid, _)| guid == o.guid)
                        .map(|(_, color)| color)
                }),
                slot => {
                    let index = state.players.get(usize::from(slot)).map_or(8, |p| p.color);
                    Some(marker_argb(index.min(8)))
                }
            };
            if let Some(color) = color {
                out.push(MinimapDot {
                    tile: tile(m.position),
                    color,
                });
            }
        }
    }
    for table in state.tables.iter().take(9) {
        for o in table.objects.iter().flatten() {
            if o.status & status::DIED != 0
                || crate::game::fog_view::render_bits_for(&view, o, player, false) & 1 == 0
                || !shown_in_mode(o, mode)
            {
                continue;
            }
            let (pos, _) = host.display(o);
            out.push(MinimapDot {
                tile: tile(pos.to_array()),
                color: marker_argb(o.color.min(8)),
            });
        }
    }
    out
}

/// Camera frame (`0x004E4290`): the screen corners (0, 0), (W-1, 0), (W-1, H-1), (0, H-1) - without half pixels, also
/// under bars and minimap - as a ray `ray(point) = (origin, direction)` of the game view, intersected with the plane
/// y = 0 (not with the terrain) and converted to the map. If a ray does not point downward (`0x00576F60` returns 0),
/// the value of the previous corner stays; for the first corner the reimplementation takes the next valid one (in the
/// original stack content; unreachable at a tilt of 20-80 degrees). Without any valid corner the map center.
pub fn camera_frame(
    layout: &MinimapLayout,
    size: (u32, u32),
    cell: [i32; 3],
    ray: impl Fn((f64, f64)) -> (Vec3, Vec3),
) -> [[f32; 2]; 4] {
    let (b, h) = (f64::from(size.0.max(1) - 1), f64::from(size.1.max(1) - 1));
    let hits = [(0.0, 0.0), (b, 0.0), (b, h), (0.0, h)].map(|p| {
        let (origin, direction) = ray(p);
        (direction.y < 0.0).then(|| {
            let t = -origin.y / direction.y;
            layout.world_to_screen(origin.x + direction.x * t, origin.z + direction.z * t, cell)
        })
    });
    let first = hits
        .iter()
        .flatten()
        .next()
        .copied()
        .unwrap_or(layout.center);
    let mut last = first;
    hits.map(|hit| {
        last = hit.unwrap_or(last);
        last
    })
}

/// A dot (`0x0047AA60`): quad `(px - 1, py - 1)...(px + 1, py + 1)` around the floating-point position, unrotated,
/// without texture, opaque in the color (state `Map`).
fn dot_quad(at: [f32; 2], color: u32) -> DrawCmd {
    let [x, y] = at;
    DrawCmd::Quad4 {
        texture: None,
        corners: [
            [x - 1.0, y - 1.0],
            [x + 1.0, y - 1.0],
            [x + 1.0, y + 1.0],
            [x - 1.0, y + 1.0],
        ],
        uv: [[0.0; 2]; 4],
        color,
        filter: Filter::Point,
    }
}

/// Draws the minimap of a frame into `out` (`0x00449C30`, order in the module header) and remembers the position for
/// dots and clicks ([`Minimap::last_layout`]). Whether it is drawn at all (node with `IsMap`, `+0x4D`, cinemascope)
/// is decided by the host and `Hud`. If an image is missing, only its command is dropped.
pub fn draw(m: &Minimap, frame: &MinimapFrame, ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
    // 1. Frame: tile 0 of the 176 grid at (W - 176, H - 176), state `3dicons-point` (`FUN_004020E0`).
    if let Some(tex) = ctx.assets.texture(ctx.vfs, FRAME_IMAGE) {
        let dst = minimap_rect(ctx.screen);
        image_quad(
            ctx.assets,
            tex,
            FRAME_TILE,
            0,
            dst,
            dst,
            WHITE,
            Filter::Point,
            out,
        );
    }
    // 2. Terrain with fog (front texture), point filter, state `Map` (without blending; the image is opaque).
    let corners = frame.layout.corners();
    out.push(DrawCmd::Quad4 {
        texture: Some(frame.texture),
        corners,
        uv: FULL_UV,
        color: WHITE,
        filter: Filter::Point,
    });
    // 3. Border on the same corners with UV 0...1 without half texel, `3dicons-point`.
    if let Some(tex) = ctx.assets.texture(ctx.vfs, BORDER_IMAGE) {
        out.push(DrawCmd::Quad4 {
            texture: Some(tex),
            corners,
            uv: FULL_UV,
            color: WHITE,
            filter: Filter::Point,
        });
    }
    // 4. Fog memory and object dots, then the ring points of the pings (state `alpharect1`: with alpha), all as
    // unrotated 2 x 2 dots (`0x0047AA60`).
    for dot in frame.dots.iter().chain(&frame.pings) {
        out.push(dot_quad(
            frame.layout.tile_to_screen(dot.tile[0], dot.tile[1]),
            dot.color,
        ));
    }
    // 5. Camera frame: closed quad of four white lines (indices 0,1, 1,2, 2,3, 3,0), unclipped.
    for i in 0..4 {
        out.push(line(frame.camera[i], frame.camera[(i + 1) % 4], WHITE));
    }
    m.last_layout.set(Some(frame.layout));
}

/// Screen rows into which the terrain quad `corners` (step 2 of [`draw`]) writes depth 0 in the screen `size` (O-457,
/// re/pointer-3d MZ.7): `FUN_004E3B70` draws it at `0x004E3EB6` in the state block `Map` (`0x004C54AE`:
/// `ZENABLE 1`, `ZWRITEENABLE 1`, no alpha test, no blending) with z = 0 - so the **whole** rotated quad, not only
/// visible texels. Frame, border, dots and camera frame (`3dicons-point`, `alpharect1`, `Lines`) write no depth.
///
/// One rectangle `[x0, y, x1, y + 1]` per screen row (right and bottom exclusive) from the pixels whose point
/// `(x, y)` lies in the quad - the same rule as the drawing path of the UI (D3D8 raster: pixel center on whole
/// coordinates, in the wgpu raster `+0.5`; left and top edges count). The corners are first snapped to 1/256 px like
/// in the raster of the map, so that rounding remainders from `sin`/`cos` add no column. Clamped to the screen;
/// no row without area.
pub fn depth_rects(corners: &[[f32; 2]; 4], size: (u32, u32)) -> Vec<[i32; 4]> {
    let snap = |v: f32| (v * 256.0).round() / 256.0;
    let pts = corners.map(|[x, y]| [snap(x), snap(y)]);
    let (w, h) = (
        i32::try_from(size.0).unwrap_or(i32::MAX),
        i32::try_from(size.1).unwrap_or(i32::MAX),
    );
    let ys = pts.map(|p| p[1]);
    let (y_min, y_max) = (
        ys.iter().copied().fold(f32::INFINITY, f32::min),
        ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    );
    if !(y_min.is_finite() && y_max.is_finite()) {
        return Vec::new();
    }
    let first = (y_min.ceil() as i32).max(0);
    let past = (y_max.ceil() as i32).min(h);
    let mut out = Vec::new();
    for y in first..past {
        let fy = y as f32;
        let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..4 {
            let (a, b) = (pts[i], pts[(i + 1) % 4]);
            let (lo, hi) = if a[1] <= b[1] { (a, b) } else { (b, a) };
            // Half-open in y: horizontal edges and the lower corner do not count.
            if !(lo[1] <= fy && fy < hi[1]) {
                continue;
            }
            let x = lo[0] + (fy - lo[1]) * (hi[0] - lo[0]) / (hi[1] - lo[1]);
            left = left.min(x);
            right = right.max(x);
        }
        if left >= right {
            continue;
        }
        let x0 = (left.ceil() as i32).max(0);
        let x1 = (right.ceil() as i32).min(w);
        if x0 < x1 {
            out.push([x0, y, x1, y + 1]);
        }
    }
    out
}

/// `Screen` -> `(W, H)` for [`layout`].
pub fn screen_size(screen: Screen) -> (u32, u32) {
    (
        u32::try_from(screen.width).unwrap_or(0),
        u32::try_from(screen.height).unwrap_or(0),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use image::RgbaImage;
    use opennml_sim::id::ObjectId;
    use opennml_sim::module::base::ModuleBase;
    use opennml_sim::module::{Module, ModuleKind};
    use opennml_sim::rules::Rules;
    use opennml_sim::setup::MatchSetup;
    use opennml_sim::sim::Sim;
    use opennml_sim::state::Control;
    use opennml_sim::world::{ObjectDesc, create_object, desc_flags};
    use opennml_ui::draw::Blend;
    use opennml_world::camera::{CameraSetup, DEFAULT_CAMERA_OFFSET};

    /// Camera with the start offset `CAMERA_OFFSET` (D-254: target at y = 0) above `target`.
    pub(crate) fn start_camera(target: Vec3) -> GameCamera {
        GameCamera {
            eye: target + DEFAULT_CAMERA_OFFSET,
            target,
            up: Vec3::Y,
            setup: CameraSetup::default(),
        }
    }

    /// Camera with theta = 0: eye at -z of the target, so `atan2(dz, dx) = -pi/2`.
    fn north_camera() -> GameCamera {
        let target = Vec3::new(4096.0, 0.0, 4096.0);
        GameCamera {
            eye: target + Vec3::new(0.0, 8000.0, -8000.0),
            target,
            up: Vec3::Y,
            setup: CameraSetup::default(),
        }
    }

    pub(crate) fn close(a: [f32; 2], b: [f32; 2], what: &str) {
        assert!(
            (a[0] - b[0]).abs() <= 0.1 && (a[1] - b[1]).abs() <= 0.1,
            "{what}: {a:?} instead of {b:?}"
        );
    }

    /// O-457: with theta = 0 the diamond covers exactly the pixels (871 ... 998, 615 ... 742) at 1024 x 768 - one
    /// rectangle per screen row; rotated, the area stays at 128^2 and the screen edge clamps.
    #[test]
    fn depth_rects_of_the_terrain_quad() {
        let l = layout((1024, 768), &north_camera(), (128, 128));
        let rects = depth_rects(&l.corners(), (1024, 768));
        assert_eq!(rects.len(), 128);
        for (i, r) in rects.iter().enumerate() {
            assert_eq!(*r, [871, 615 + i as i32, 999, 616 + i as i32]);
        }
        let turned = layout(
            (1024, 768),
            &start_camera(Vec3::new(8192.0, 0.0, 8192.0)),
            (128, 128),
        );
        let rects = depth_rects(&turned.corners(), (1024, 768));
        let area: i32 = rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert!((area - 128 * 128).abs() < 400, "area {area}");
        // Screen smaller than the diamond: only rows and columns on the screen.
        let clipped = depth_rects(&l.corners(), (900, 700));
        assert!(
            clipped
                .iter()
                .all(|r| r[0] >= 0 && r[2] <= 900 && r[3] <= 700 && r[0] < r[2])
        );
        assert_eq!(clipped.len(), 700 - 615);
    }

    // QUIRK O-39: kept
    #[test]
    fn layout_follows_m1() {
        let cam = start_camera(Vec3::new(8192.0, 0.0, 8192.0));
        assert_eq!(layout((1024, 768), &cam, (128, 128)).center, [935.0, 679.0]);
        assert_eq!(layout((800, 600), &cam, (128, 128)).center, [711.0, 511.0]);
        // The diamond is 128 px wide for every map.
        let l = layout((1024, 768), &cam, (64, 256));
        assert_eq!((l.sx, l.sz), (2.0, 0.5));
        // theta = 0: tile (mx, mz) -> (871 + mx, 743 - mz) at 1024 x 768.
        let l = layout((1024, 768), &north_camera(), (128, 128));
        assert!(l.theta.abs() < 1e-6, "theta = {}", l.theta);
        close(l.tile_to_screen(0.0, 0.0), [871.0, 743.0], "(0, 0)");
        close(l.tile_to_screen(64.0, 64.0), [935.0, 679.0], "(64, 64)");
        close(l.tile_to_screen(100.0, 20.0), [971.0, 723.0], "(100, 20)");
        close(
            l.world_to_screen(128.0 * 10.5, 128.0 * 3.25, [128, 128, 128]),
            [881.5, 739.75],
            "world position",
        );
    }

    // QUIRK O-39: kept
    #[test]
    fn rotation_matches_m2() {
        // Preliminary check: theta = -0.76800 rad, corners to 0.1 px; for map 64 the same screen points.
        let cam = start_camera(Vec3::new(4096.0, 0.0, 4096.0));
        // Share of the map (mx/dimX, mz/dimZ), screen point at 1024 x 768, screen point at 800 x 600.
        type Row = ((f32, f32), [f32; 2], [f32; 2]);
        let table: [Row; 5] = [
            ((0.0, 0.0), [933.43, 769.50], [709.43, 601.50]),
            ((1.0, 1.0), [936.57, 588.50], [712.57, 420.50]),
            ((0.0, 1.0), [844.50, 677.43], [620.50, 509.43]),
            ((1.0, 0.0), [1025.50, 680.57], [801.50, 512.57]),
            ((0.5, 0.5), [935.0, 679.0], [711.0, 511.0]),
        ];
        for dim in [128u32, 64] {
            let big = layout((1024, 768), &cam, (dim, dim));
            let small = layout((800, 600), &cam, (dim, dim));
            assert!((big.theta + 0.76800).abs() < 1e-4, "theta = {}", big.theta);
            for ((fx, fz), at_1024, at_800) in table {
                let (mx, mz) = (fx * dim as f32, fz * dim as f32);
                let what = format!("map {dim}, tile ({mx}, {mz})");
                close(big.tile_to_screen(mx, mz), at_1024, &what);
                close(small.tile_to_screen(mx, mz), at_800, &what);
            }
        }
        // The diamond: corners in the order (0, dimZ), (dimX, dimZ), (dimX, 0), (0, 0).
        let c = layout((1024, 768), &cam, (128, 128)).corners();
        close(c[0], [844.50, 677.43], "corner 0");
        close(c[1], [936.57, 588.50], "corner 1");
        close(c[2], [1025.50, 680.57], "corner 2");
        close(c[3], [933.43, 769.50], "corner 3");
    }

    fn tile_near(got: Option<(f32, f32)>, want: (f32, f32), what: &str) {
        let got = got.unwrap_or_else(|| panic!("{what}: outside the diamond"));
        assert!(
            (got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3,
            "{what}: {got:?} instead of {want:?}"
        );
    }

    /// `0x004E4840`: theta = 0, map 128 at 1024 x 768 - screen point (871 + mx, 743 - mz). The edges count, 0.5 px
    /// beside them do not. Exactly on a corner of the diamond the angle sum reports NaN and thus "outside" (O-387):
    /// (871, 743) is tile (0, 0) of the formula, but the click goes to the world.
    // QUIRK O-387: kept
    #[test]
    fn click_maps_to_tile_m3() {
        let l = layout((1024, 768), &north_camera(), (128, 128));
        tile_near(l.screen_to_tile([935.0, 679.0]), (64.0, 64.0), "center");
        tile_near(
            l.screen_to_tile([872.0, 742.0]),
            (1.0, 1.0),
            "next to the corner",
        );
        tile_near(l.screen_to_tile([871.0, 700.0]), (0.0, 43.0), "left edge");
        tile_near(
            l.screen_to_tile([999.0, 615.0 + 0.5]),
            (128.0, 127.5),
            "right edge",
        );
        tile_near(l.screen_to_tile([900.0, 644.0]), (29.0, 99.0), "diagonal");
        assert_eq!(l.screen_to_tile([871.0, 743.0]), None, "corner (0, 0)");
        assert_eq!(l.screen_to_tile([999.0, 615.0]), None, "corner (128, 128)");
        assert_eq!(l.screen_to_tile([870.5, 700.0]), None, "0.5 px to the left");
        assert_eq!(
            l.screen_to_tile([850.0, 600.0]),
            None,
            "in the rectangle, outside"
        );
        let big = layout((1024, 768), &north_camera(), (256, 256));
        tile_near(
            big.screen_to_tile([945.0, 679.0]),
            (148.0, 128.0),
            "map 256",
        );
    }

    /// Maps 64, 96, 100, 256 and `dimX != dimZ`, straight and rotated (start camera, theta = -0.768), at 1024 x 768
    /// and 800 x 600. The diamond lies around (W - 89, H - 89); every whole screen point of the 176-pixel rectangle
    /// that lies in the diamond yields a tile of the map that, calculated back, falls on it again; every tile center
    /// returns to itself via the screen. Plus the ping factor `k` (1 for 96 and 100) and the fog rows per idle tick
    /// `(dimZ / 10) * 2`.
    // QUIRK O-387: kept
    #[test]
    fn minimap_roundtrip_on_odd_and_non_square_maps() {
        let dims = [
            (64, 64),
            (96, 96),
            (100, 100),
            (256, 256),
            (64, 256),
            (100, 96),
            (128, 64),
        ];
        let cameras = [north_camera(), start_camera(Vec3::new(4096.0, 0.0, 4096.0))];
        for screen in [(1024u32, 768u32), (800, 600)] {
            for cam in &cameras {
                for dim in dims {
                    let l = layout(screen, cam, dim);
                    let what = format!("{screen:?}, map {dim:?}, theta = {}", l.theta);
                    let center = [screen.0 as f32 - 89.0, screen.1 as f32 - 89.0];
                    assert_eq!(l.center, center, "{what}");
                    tile_near(
                        l.screen_to_tile(center),
                        (dim.0 as f32 / 2.0, dim.1 as f32 / 2.0),
                        &format!("{what}: center"),
                    );
                    let (b, h) = (screen.0 as i32, screen.1 as i32);
                    let mut inside = 0;
                    for y in h - 176..h {
                        for x in b - 176..b {
                            let p = [x as f32, y as f32];
                            let Some((mx, mz)) = l.screen_to_tile(p) else {
                                continue;
                            };
                            inside += 1;
                            // The angle sum takes points up to about 0.2 px outside an edge (O-387).
                            let (ex, ez) = (0.3 / l.sx, 0.3 / l.sz);
                            assert!(
                                (-ex..=dim.0 as f32 + ex).contains(&mx)
                                    && (-ez..=dim.1 as f32 + ez).contains(&mz),
                                "{what}: {p:?} -> ({mx}, {mz})"
                            );
                            let back = l.tile_to_screen(mx, mz);
                            assert!(
                                (back[0] - p[0]).abs() < 0.01 && (back[1] - p[1]).abs() < 0.01,
                                "{what}: {p:?} -> ({mx}, {mz}) -> {back:?}"
                            );
                        }
                    }
                    // The diamond is 128 x 128 px: about that many whole screen points lie in it.
                    assert!((16_000..=16_900).contains(&inside), "{what}: {inside}");
                    for tz in 0..dim.1 {
                        for tx in (0..dim.0).step_by(3) {
                            let (mx, mz) = (tx as f32 + 0.5, tz as f32 + 0.5);
                            tile_near(
                                l.screen_to_tile(l.tile_to_screen(mx, mz)),
                                (mx, mz),
                                &format!("{what}: tile ({mx}, {mz})"),
                            );
                        }
                    }
                }
            }
        }
        for (dim, k, rows) in [
            (64, 2, 12),
            (96, 1, 18),
            (100, 1, 20),
            (128, 4, 24),
            (256, 8, 50),
        ] {
            assert_eq!(super::super::pings::radius_factor(dim), k, "k at {dim}");
            assert_eq!(
                super::super::fog::rows_per_idle_tick(dim),
                rows,
                "rows at {dim}"
            );
        }
    }

    /// Screen corners of the camera frame without half pixels at 1024 x 768.
    const CORNERS: [(f64, f64); 4] = [(0.0, 0.0), (1023.0, 0.0), (1023.0, 767.0), (0.0, 767.0)];

    // QUIRK O-39: kept
    #[test]
    fn camera_frame_cuts_y0_and_keeps_the_previous_corner() {
        let l = layout((1024, 768), &north_camera(), (128, 128));
        let cell = [128, 128, 128];
        // Oblique rays from (x, 1000, z) with direction (0.5, -2, 0.25): intersection at y = 0 offset by (250, 125).
        let world = [
            (1280.0, 2560.0),
            (2560.0, 2560.0),
            (2560.0, 1280.0),
            (1280.0, 1280.0),
        ];
        let index = |p: (f64, f64)| {
            CORNERS
                .iter()
                .position(|&q| q == p)
                .expect("screen corner without half pixel")
        };
        let ray_at = |p: (f64, f64)| {
            let (x, z) = world[index(p)];
            (Vec3::new(x, 1000.0, z), Vec3::new(0.5, -2.0, 0.25))
        };
        let frame = camera_frame(&l, (1024, 768), cell, ray_at);
        for (k, &(x, z)) in world.iter().enumerate() {
            close(
                frame[k],
                l.world_to_screen(x + 250.0, z + 125.0, cell),
                &format!("corner {k}"),
            );
        }
        // A ray upward (or horizontal) keeps the previous corner; for corner 0 the next valid one.
        let up = |skip: usize| {
            move |p: (f64, f64)| {
                let (o, d) = ray_at(p);
                if index(p) == skip {
                    (o, Vec3::new(0.0, 0.0, 1.0))
                } else {
                    (o, d)
                }
            }
        };
        let frame2 = camera_frame(&l, (1024, 768), cell, up(2));
        assert_eq!(frame2[2], frame[1]);
        assert_eq!(frame2[3], frame[3]);
        let frame0 = camera_frame(&l, (1024, 768), cell, up(0));
        assert_eq!(frame0[0], frame[1]);
        assert_eq!(frame0[1..], frame[1..]);
    }

    // --- Dots -----------------------------------------------------------------------------------------------------

    const COLONIST: u32 = 1100;
    const OTHER_RESOURCE: u32 = 3501;
    const TREE: u32 = 3602;

    /// Types without modules on a 64 x 64 map, map revealed; the modules of the check are set by [`give_module`].
    /// The resources carry the category nature (`0x200`, slot 10), the tree `0x100` (slot 9).
    fn host() -> SimHost {
        let types = concat!(
            "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\n",
            "GUID 3500\nNAME Goldmine\nVALUEID 100 6 1 512\n",
            "GUID 3501\nNAME Stein\nVALUEID 100 6 1 512\n",
            "GUID 3503\nNAME Silber\nVALUEID 100 6 1 512\n",
            "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 256\n",
        );
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host =
            SimHost::new(Rules::synthetic(types, 64), setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host.state.players[0].control = Control::Human;
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        host
    }

    fn spawn(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]);
        desc.flags |= desc_flags::PLACED;
        let id = create_object(&mut sim, &mut desc).expect("object created");
        // The types have no model: render bit 0 ("is drawn") like a drawn object.
        host.state.object_mut(id, false).unwrap().render_bits |= 1;
        id
    }

    /// A plain module `index` with the flags `flags` (the base suffices for the mode filters).
    fn give_module(host: &mut SimHost, id: ObjectId, index: usize, flags: u32, enabled: bool) {
        let o = host.state.object_mut(id, false).unwrap();
        let mut base = ModuleBase::new(index as u8);
        base.flags = flags;
        base.enabled = enabled;
        o.modules[index] = Some(Module {
            base,
            kind: ModuleKind::Inert,
        });
    }

    fn colors(dots: &[MinimapDot]) -> Vec<([f32; 2], u32)> {
        dots.iter().map(|d| (d.tile, d.color)).collect()
    }

    fn center(tile: (i32, i32)) -> [f32; 2] {
        [tile.0 as f32 + 0.5, tile.1 as f32 + 0.5]
    }

    fn dot_of(host: &SimHost, id: ObjectId, tile: (i32, i32)) -> ([f32; 2], u32) {
        let o = host.state.object(id, false).unwrap();
        (center(tile), marker_argb(o.color.min(8)))
    }

    /// According to Ghidra (`0x00480210`/`0x00466BE0`/`0x004712A0`) and a reference run of the original: the loop of
    /// the dots checks only `DIED` and render bit 0, never `QUARTERED`; `GameObject_EnterContainer` sets the grid
    /// position of the occupant to that of the container (`SetGridPosition(container+0x198)`) and leaves the render
    /// bits alone. In the reference run, quartering therefore made only the old dot disappear: the dot of the
    /// occupant now lies exactly on that of the building (same color, same 2 x 2 pixels), so none "was added". The
    /// reimplementation behaves the same (O-288).
    // QUIRK O-288: kept
    #[test]
    fn quartered_units_sit_exactly_on_the_container_dot() {
        let mut host = host();
        let tower = spawn(&mut host, COLONIST, 0, (10, 10));
        let unit = spawn(&mut host, COLONIST, 0, (14, 14));
        let before = colors(&dots(0, &host));
        assert_eq!(before.len(), 2);
        assert_ne!(before[0].0, before[1].0);
        let rules = host.rules.clone();
        {
            let mut sim = Sim::new(&mut host.state, &rules);
            opennml_sim::module::quarter::enter_container(&mut sim, unit, tower);
        }
        let o = host.state.object(unit, false).unwrap();
        assert!(o.status & status::QUARTERED != 0, "quartered");
        assert_eq!(o.render_bits & 1, 1, "render bit 0 stays");
        let after = colors(&dots(0, &host));
        // Two dots, both on the tile of the container and of the same color: one for the eye.
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], after[1]);
        assert_eq!(after[0], dot_of(&host, tower, (10, 10)));
    }

    // QUIRK O-288: kept
    #[test]
    fn dots_are_2x2_and_filtered_by_mode() {
        let mut host = host();
        let plain = spawn(&mut host, COLONIST, 0, (10, 10));
        let soldier = spawn(&mut host, COLONIST, 1, (20, 20));
        give_module(&mut host, soldier, 8, 0, true);
        let worker = spawn(&mut host, COLONIST, 0, (30, 30));
        give_module(&mut host, worker, 0x11, 0, true);
        let armed_worker = spawn(&mut host, COLONIST, 2, (31, 30));
        give_module(&mut host, armed_worker, 0x11, 0, true);
        give_module(&mut host, armed_worker, 8, 0, true);
        // Class `0x2003`: module 3 with bit `0x2000`, also switched off (`PROPID` 0).
        let fort = spawn(&mut host, COLONIST, 0, (40, 40));
        give_module(&mut host, fort, 3, 0x2000, false);
        // Module 8 switched off does not count in mode 1 (`HasModule(8, 0)`).
        let off = spawn(&mut host, COLONIST, 3, (41, 40));
        give_module(&mut host, off, 8, 0, false);
        // Class `0x1003` of an occupant: `MatchesClass` takes no quartered objects.
        let inside = spawn(&mut host, COLONIST, 0, (42, 40));
        give_module(&mut host, inside, 3, 0x1000, true);
        host.state.object_mut(inside, false).unwrap().status |= status::QUARTERED;
        // Never: dead, render bit 0 off, slots 9 and 10.
        let dead = spawn(&mut host, COLONIST, 0, (50, 50));
        host.state.object_mut(dead, false).unwrap().status |= status::DIED;
        let unseen = spawn(&mut host, COLONIST, 1, (51, 50));
        host.state.object_mut(unseen, false).unwrap().render_bits &= !1;
        let tree = spawn(&mut host, TREE, 9, (52, 50));
        let stone = spawn(&mut host, OTHER_RESOURCE, 10, (53, 50));
        assert_eq!((tree.slot(), stone.slot()), (9, 10), "precondition: slots");
        // Color index above 8: gray (`0x0047D4C0` clamps to 8).
        host.state.object_mut(plain, false).unwrap().color = 11;
        assert_eq!(
            host.state.object(plain, false).unwrap().render_bits & 1,
            1,
            "precondition: visible"
        );

        let expect = |ids: &[(ObjectId, (i32, i32))]| -> Vec<([f32; 2], u32)> {
            ids.iter().map(|&(id, t)| dot_of(&host, id, t)).collect()
        };
        // Mode 0: all of slots 0-8 in table order; the minimap does not check occupants.
        let mode0 = colors(&dots(0, &host));
        assert_eq!(
            mode0,
            expect(&[
                (plain, (10, 10)),
                (worker, (30, 30)),
                (fort, (40, 40)),
                (inside, (42, 40)),
                (soldier, (20, 20)),
                (armed_worker, (31, 30)),
                (off, (41, 40)),
            ])
        );
        assert_eq!(mode0[0].1, 0xFFAC_ACAC, "color index 11 -> gray");
        assert_eq!(
            colors(&dots(1, &host)),
            expect(&[
                (fort, (40, 40)),
                (soldier, (20, 20)),
                (armed_worker, (31, 30))
            ])
        );
        assert_eq!(colors(&dots(2, &host)), expect(&[(worker, (30, 30))]));

        // Drawn: per dot an unrotated 2 x 2 quad +-1 px around the position, without texture, after the images.
        let (_dir, vfs) = ui_install();
        let mut ui = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = ui.context(&vfs, SCREEN);
        let map = minimap(64);
        let l = layout(
            (1024, 768),
            &start_camera(Vec3::new(4096.0, 0.0, 4096.0)),
            (64, 64),
        );
        let frame = MinimapFrame {
            layout: l,
            dots: dots(2, &host),
            pings: Vec::new(),
            camera: [[0.0; 2]; 4],
            texture: map.texture(ctx.assets),
        };
        let mut out = Vec::new();
        draw(&map, &frame, &mut ctx, &mut out);
        let [x, y] = l.tile_to_screen(30.5, 30.5);
        assert_eq!(
            out[3],
            DrawCmd::Quad4 {
                texture: None,
                corners: [
                    [x - 1.0, y - 1.0],
                    [x + 1.0, y - 1.0],
                    [x + 1.0, y + 1.0],
                    [x - 1.0, y + 1.0]
                ],
                uv: [[0.0; 2]; 4],
                color: dot_of(&host, worker, (30, 30)).1,
                filter: Filter::Point,
            }
        );
    }

    // QUIRK O-288: kept
    #[test]
    fn memory_dots_only_in_mode_0_and_gold_only_while_alive() {
        let mut host = host();
        let gold = spawn(&mut host, 3500, 10, (5, 5));
        let silver = spawn(&mut host, 3503, 10, (6, 5));
        let stone = spawn(&mut host, OTHER_RESOURCE, 10, (7, 5));
        let tree = spawn(&mut host, TREE, 9, (8, 5));
        assert_eq!((gold.slot(), tree.slot()), (10, 9), "precondition: slots");
        let house = spawn(&mut host, COLONIST, 1, (9, 5));
        for id in [gold, silver, stone] {
            give_module(&mut host, id, 6, 0, true);
        }
        give_module(&mut host, tree, 4, 0, true);
        give_module(&mut host, house, 3, 0, true);
        // Slot 1 has a different color than the object: the fog memory dot takes the current one of the slot.
        host.state.players[1].color = 5;
        host.state.object_mut(house, false).unwrap().color = 1;
        let view = host.fog.view(&host.state);
        host.memories
            .rebuild(&host.state, &host.rules.map, &view, 0, true);
        assert!(
            host.memories.iter().all(|(_, m)| m.snapshot.is_some()),
            "precondition: all fog memories valid"
        );
        // Without the object dot of the house (color of the object, not of the slot) the fog memory dots remain.
        let house_dot = (center((9, 5)), marker_argb(1));
        let memory_dots = |host: &SimHost, mode| {
            let mut all = colors(&dots(mode, host));
            all.retain(|d| *d != house_dot);
            all
        };
        // ID order: slot 1 before slot 10.
        assert_eq!(
            memory_dots(&host, 0),
            [
                (center((9, 5)), marker_argb(5)),
                (center((5, 5)), GOLD_3500.1),
                (center((6, 5)), GOLD_3503.1),
            ],
            "gold 3500/3503 and the house in the color of slot 1; stone and tree without a dot"
        );
        assert_eq!(
            colors(&dots(0, &host)).last(),
            Some(&house_dot),
            "the object dot takes the color of the object and follows the fog memory dots"
        );
        assert!(memory_dots(&host, 1).is_empty() && memory_dots(&host, 2).is_empty());
        // The gold deposit dies: its memory stays, the dot does not.
        host.state.object_mut(gold, false).unwrap().status |= status::DIED;
        assert!(host.memories.get(gold).is_some());
        assert_eq!(
            memory_dots(&host, 0),
            [
                (center((9, 5)), marker_argb(5)),
                (center((6, 5)), GOLD_3503.1)
            ]
        );
    }

    // --- Drawing --------------------------------------------------------------------------------------------------

    pub(crate) const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    /// Installation with a font (for the layer) and the images of the minimap: frame 256 x 256, border 128 x 128.
    pub(crate) fn ui_install() -> (tempfile::TempDir, opennml_rda::Vfs) {
        use crate::testutil::{fnt, install, tga, utf16};
        let loc = "TextDatei data\\txt\\t.txt\n\
                   Font InfoMessages\nFontSize 15\nTextureFont\nFontName Test\n";
        install(&[
            ("data/txt/localisation.deu", loc.as_bytes().to_vec()),
            ("data/txt/t.txt", utf16("1=AB\r\n")),
            ("data/fonts/Test_15_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_15_0_0_0_0.tga", tga(256, 256)),
            (FRAME_IMAGE, tga(256, 256)),
            (BORDER_IMAGE, tga(128, 128)),
            (
                opennml_ui::hud::toolbar::minimap_buttons::MAP_ICONS_PATH,
                tga(240, 60),
            ),
        ])
    }

    /// Minimap `dim x dim` with all tiles identical.
    pub(crate) fn minimap(dim: u32) -> Minimap {
        let image = RgbaImage::from_pixel(dim, dim, image::Rgba([0x71, 0x59, 0x22, 255]));
        Minimap::new(super::super::MinimapTerrain {
            dim_x: dim,
            dim_z: dim,
            colors: vec![0x0022_5971; (dim * dim) as usize],
            image,
        })
    }

    // QUIRK O-39, O-289: kept
    #[test]
    fn draw_order_frame_terrain_border_dots_pings_camera_frame() {
        let (_dir, vfs) = ui_install();
        let mut ui = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = ui.context(&vfs, SCREEN);
        let map = minimap(128);
        assert_eq!(map.last_layout(), None);
        let l = layout(
            (1024, 768),
            &start_camera(Vec3::new(8192.0, 0.0, 8192.0)),
            (128, 128),
        );
        let camera = [
            [900.0, 650.0],
            [960.0, 650.0],
            [970.0, 700.0],
            [890.0, 700.0],
        ];
        let frame = MinimapFrame {
            layout: l,
            dots: vec![MinimapDot {
                tile: [64.0, 64.0],
                color: 0xFFFF_CE0D,
            }],
            pings: vec![MinimapDot {
                tile: [65.0, 64.0],
                color: 0x80FF_0000,
            }],
            camera,
            texture: map.texture(ctx.assets),
        };
        let mut out = Vec::new();
        draw(&map, &frame, &mut ctx, &mut out);
        assert_eq!(out.len(), 1 + 1 + 1 + 1 + 1 + 4);
        let path = |c: &DrawCmd| match c {
            DrawCmd::Quad { texture, .. } | DrawCmd::Quad4 { texture, .. } => {
                texture.map(|t| ctx.assets.path(t).to_string())
            }
        };
        // 1. Frame: tile 0 of the 176 grid in the 256 image, half texel, POINT.
        let t = |px: f32| px / 256.0;
        assert_eq!(
            out[0],
            DrawCmd::Quad {
                texture: ctx.assets.find(FRAME_IMAGE),
                dst: [848, 592, 1024, 768],
                uv: [t(0.5), t(0.5), t(175.0) - t(0.5), t(175.0) - t(0.5)],
                color: WHITE,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }
        );
        // 2./3. Terrain (front fog image) and border on the same rotated corners, UV 0...1.
        for (cmd, image) in [(&out[1], FOG_TEXTURE), (&out[2], BORDER_IMAGE)] {
            let DrawCmd::Quad4 {
                corners,
                uv,
                color,
                filter,
                ..
            } = *cmd
            else {
                unreachable!()
            };
            assert_eq!(
                path(cmd).as_deref(),
                Some(opennml_rda::normalize_path(image).as_str())
            );
            assert_eq!(
                (corners, uv, color, filter),
                (l.corners(), FULL_UV, WHITE, Filter::Point)
            );
        }
        // 4. Dot in the center, then the ring point of a ping (with alpha), 5. four white lines of the camera frame
        // in corner order.
        let DrawCmd::Quad4 { corners, color, .. } = out[3] else {
            unreachable!()
        };
        assert_eq!(
            (corners[0], corners[2], color),
            ([934.0, 678.0], [936.0, 680.0], 0xFFFF_CE0D)
        );
        assert_eq!(out[4], dot_quad(l.tile_to_screen(65.0, 64.0), 0x80FF_0000));
        for i in 0..4 {
            assert_eq!(out[5 + i], line(camera[i], camera[(i + 1) % 4], WHITE));
        }
        assert_eq!(map.last_layout(), Some(l), "the position stays for clicks");
    }

    #[test]
    fn the_fog_image_is_uploaded_only_when_it_changed() {
        let mut assets = opennml_ui::assets::UiAssets::new();
        let mut map = minimap(20);
        let id = map.texture(&mut assets);
        assert_eq!(assets.revision(id), 0);
        assert_eq!(map.texture(&mut assets), id);
        assert_eq!(assets.revision(id), 0, "same image: no new upload");
        // An idle tick writes only the back texture: the front one stays, no upload.
        map.sync(1, 0, 0, |_, _| 1);
        map.texture(&mut assets);
        assert_eq!(assets.revision(id), 0);
        // Immediate rebuild: new front texture, uploaded once.
        map.sync(1, 1, 0, |_, _| 1);
        assert_eq!(map.texture(&mut assets), id);
        assert_eq!(assets.revision(id), 1);
        assert_eq!(assets.image(id), map.fog().front());
        map.texture(&mut assets);
        assert_eq!(assets.revision(id), 1);
    }
}
