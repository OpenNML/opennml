//! Ground marks of layer 4 (re/world-display W.3.2, W.3.3): footprints and craters
//! (`FootprintEffect`, vtable `0x00620E18`) and tread marks (`TreadMarkEffect`, vtable `0x0062199C`).
//! Both are world effects of the EffectManager, drawn in step 7 of `GameEngine_RenderWorld`
//! before the shadows in state `Footprint`. The triggers (units, buildings, cannonballs) are set by
//! the host.

use std::collections::VecDeque;

use glam::Vec3;
use nml_world::visibility::{Containment, Frustum};

use crate::output::TreadQuad;

/// Directory of the track textures (`0x004F1CA0`, fixed "Summer", no season; lowercase in the
/// archive, `Vfs` normalizes).
pub const FOOTSTEP_DIR: &str = "Data/R3D/Nature/Summer/Footsteps/";

/// Deduction per game tick (`0x004F1B50`).
const TICK_MS: i32 = 100;

/// Radius of the sight sphere around the first mark (`0x004F1AD0`).
const SIGHT_RADIUS: f32 = 12.0;

/// Half width of a tread mark (`0x004F5EA0` -> `0x004F5560(4.0, 0xFF000000)`).
pub const TREAD_HALF_WIDTH: f32 = 4.0;

/// Highest alpha of a tread mark.
const TREAD_ALPHA: i32 = 0x60;

/// Smallest x extent of the hull from which a tread mark has a sight sphere (`0x004F5440`,
/// `float` at `0x0061623C`).
const TREAD_MIN_EXTENT: f32 = 0.01;

/// A footprint or a crater (`FootprintEffect_Construct` `0x004F1E00`, `0x54` bytes): up to
/// four marks (`used` as a bit mask, at most two are occupied), a rotation, a type (bits
/// select texture and size). No randomness.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    quad: [Vec3; 4],
    used: u8,
    yaw: f32,
    kind: u32,
    life_ms: i32,
    visible: bool,
    dead: bool,
}

impl Footprint {
    /// `FootprintEffect_Construct` (`0x004F1E00`): without type bit `0x10` (crater) the mark lies
    /// +/-10 perpendicular to the yaw (`side` 0 -> -, otherwise +); with bit 2 it is a pair lengthwise
    /// (`(cos, sin)(yaw +- pi/2) * 30`), both with the side offset. Lifetime 20 s, with
    /// `ParticlesQuality < 2` 10 s.
    pub fn new(pos: Vec3, yaw: f32, side: i32, kind: u32, quality: u8) -> Footprint {
        let mut quad = [pos; 4];
        let mut used = 1;
        if kind & 0x10 == 0 {
            let pair = kind & 2 != 0;
            if pair {
                used = 3;
                let front = yaw + std::f32::consts::FRAC_PI_2;
                let back = yaw - std::f32::consts::FRAC_PI_2;
                quad[0].x += front.cos() * 30.0;
                quad[0].z += front.sin() * 30.0;
                quad[1].x += back.cos() * 30.0;
                quad[1].z += back.sin() * 30.0;
            }
            let (dx, dz) = (yaw.cos() * 10.0, yaw.sin() * 10.0);
            let s = if side == 0 { -1.0 } else { 1.0 };
            quad[0].x += s * dx;
            quad[0].z += s * dz;
            if pair {
                quad[1].x += s * dx;
                quad[1].z += s * dz;
            }
        }
        Footprint {
            quad,
            used,
            yaw,
            kind,
            life_ms: if quality < 2 { 10_000 } else { 20_000 },
            visible: false,
            dead: false,
        }
    }

    /// Texture of the type (`0x004F1CA0`), full original path; checked in this order:
    /// `0x400` `bomb.tga`, `0x80` `bombbuilding1.tga`, `0x100` `bombbuilding2.tga`, `0x200`
    /// `bombbuilding3.tga`, `0x1` `Footstep.tga`, `0x800` `Footstep02.tga`, `0x4`
    /// `footstep_animal.tga`, otherwise `Footstep03.tga` (reached by no caller).
    pub fn texture(kind: u32) -> &'static str {
        const TABLE: [(u32, &str); 7] = [
            (0x400, "Data/R3D/Nature/Summer/Footsteps/bomb.tga"),
            (0x80, "Data/R3D/Nature/Summer/Footsteps/bombbuilding1.tga"),
            (0x100, "Data/R3D/Nature/Summer/Footsteps/bombbuilding2.tga"),
            (0x200, "Data/R3D/Nature/Summer/Footsteps/bombbuilding3.tga"),
            (0x1, "Data/R3D/Nature/Summer/Footsteps/Footstep.tga"),
            (0x800, "Data/R3D/Nature/Summer/Footsteps/Footstep02.tga"),
            (0x4, "Data/R3D/Nature/Summer/Footsteps/footstep_animal.tga"),
        ];
        TABLE.iter().find(|(bit, _)| kind & bit != 0).map_or(
            "Data/R3D/Nature/Summer/Footsteps/Footstep03.tga",
            |(_, t)| t,
        )
    }

    /// Half edge length (`FootprintEffect_Render` `0x004F1B90`): bit `0x40` -> 300, `0x20` -> 96,
    /// otherwise 12.
    pub fn radius(kind: u32) -> f32 {
        if kind & 0x40 != 0 {
            300.0
        } else if kind & 0x20 != 0 {
            96.0
        } else {
            12.0
        }
    }

    /// Vertex alpha (`0x004F1BD3`): `ftol(life * 0.0002f * 255f)` under 24-bit x87, here as an
    /// `f32` sequence in this order (5,000 -> 255; in `f64` it would be 254), limited to 0 ... 255.
    pub fn alpha(&self) -> u8 {
        let a = (self.life_ms as f32 * 0.0002_f32) * 255.0_f32;
        (a as i32).clamp(0, 255) as u8
    }

    /// Positions of the marks (only those set in [`Footprint::used`] count).
    pub fn quad(&self) -> &[Vec3; 4] {
        &self.quad
    }

    /// Bit mask of the occupied marks (1 single, 3 pair).
    pub fn used(&self) -> u8 {
        self.used
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn kind(&self) -> u32 {
        self.kind
    }

    /// Remaining lifetime (ms).
    pub fn life_ms(&self) -> i32 {
        self.life_ms
    }

    /// Visible since the last game tick.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Game tick (`0x004F1B50`): life -100; if it falls below 0, the mark dies and is deleted in the same
    /// tick (return `false`). Otherwise visible (`0x004F1AD0`) if the sphere around the
    /// **first** mark (r 12) is in the view frustum and its tile is currently seen by the local player
    /// (`QueryFogOfWarArea(..., -1, 0) > 1`, D-272). Return `true` = keep.
    pub fn tick(&mut self, frustum: &Frustum, fog: &dyn Fn(Vec3) -> u8) -> bool {
        self.life_ms -= TICK_MS;
        if self.life_ms < 0 {
            self.dead = true;
        }
        if self.dead {
            return false;
        }
        self.visible = frustum.classify_sphere(self.quad[0], SIGHT_RADIUS) != Containment::Outside
            && fog(self.quad[0]) > 1;
        true
    }
}

/// A point of a tread mark (`{x, y, z, time, gap}`); a gap lies at (0, 0, 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreadPoint {
    pub position: Vec3,
    /// Game time at creation (`0x006736D0`, in the reimplementation [`crate::world::EffectWorld::now_ms`]).
    pub time_ms: u32,
    pub gap: bool,
}

/// A tread mark (`TreadMarkEffect` `0x004F5350`, one permanent effect per wheel): point list with
/// lifetime per point (20 s) and maximum count (100; with `ParticlesQuality < 2` 10 s and 50).
/// Plus the hull `+0x30...+0x44`, from which the game tick forms the sight sphere (D-286).
#[derive(Debug, Clone, PartialEq)]
pub struct TreadMark {
    points: VecDeque<TreadPoint>,
    life_ms: u32,
    max: usize,
    /// Owner gone (`+0x2C`): deleted as soon as fewer than two points are left.
    owner_gone: bool,
    /// Hull: smallest (`+0x30`) and largest corner (`+0x3C`).
    bounds_min: Vec3,
    bounds_max: Vec3,
    /// In the render list since the last game tick (`0x004F5EB0` -> 2).
    listed: bool,
}

/// A segment of the drawing pass `0x004F5560` (point pair previous -> current) that is not already
/// dropped because of age or fog.
enum Segment {
    /// Gap at one of the two points: the strip starts anew.
    Break,
    /// Drawn; `index` counts backward from the newest point 0.
    Draw {
        index: i32,
        prev: TreadPoint,
        cur: TreadPoint,
        prev_age: i32,
    },
}

impl TreadMark {
    /// `TreadMarkEffect_Construct` (`0x004F5350`).
    pub fn new(quality: u8) -> TreadMark {
        let (life_ms, max) = if quality < 2 {
            (10_000, 50)
        } else {
            (20_000, 100)
        };
        TreadMark {
            points: VecDeque::new(),
            life_ms,
            max,
            owner_gone: false,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            listed: false,
        }
    }

    /// All points (gaps included), oldest first.
    pub fn points(&self) -> &VecDeque<TreadPoint> {
        &self.points
    }

    /// New point (`0x004F5ED0`): append, above the maximum count the oldest one drops. The hull
    /// starts at the first point (all axes) and afterwards grows only in x and z.
    pub fn add_point(&mut self, pos: Vec3, now_ms: u32) {
        self.points.push_back(TreadPoint {
            position: pos,
            time_ms: now_ms,
            gap: false,
        });
        if self.points.len() > self.max {
            self.points.pop_front();
        }
        if self.points.len() == 1 {
            self.bounds_min = pos;
            self.bounds_max = pos;
        } else {
            self.extend_bounds(pos);
        }
    }

    /// Extend the hull by the x and z of a position (`0x004F5ED0`, `0x004F5560`; y stays).
    fn extend_bounds(&mut self, pos: Vec3) {
        if pos.x < self.bounds_min.x {
            self.bounds_min.x = pos.x;
        }
        if pos.z < self.bounds_min.z {
            self.bounds_min.z = pos.z;
        }
        if self.bounds_max.x < pos.x {
            self.bounds_max.x = pos.x;
        }
        if self.bounds_max.z < pos.z {
            self.bounds_max.z = pos.z;
        }
    }

    /// Hull (smallest, largest corner).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.bounds_min, self.bounds_max)
    }

    /// In the render list since the last game tick: only then does the effect world return its
    /// strips ([`crate::world::EffectWorld::ground_marks`]).
    pub fn listed(&self) -> bool {
        self.listed
    }

    /// Sight sphere (`0x004F5440`) from the hull, `None` below two points or with an
    /// x extent below 0.01. **Quirk:** only the x extent `dx` counts, in all three axes:
    /// center `min + (dx, dx, dx) / 2`, radius `|(dx, dx, dx)| * 0.5` (~ 0.87 * dx). A track
    /// exactly along z has no sphere, otherwise it lies at the smallest corner instead of around the track
    /// (D-286).
    pub fn sight_sphere(&self) -> Option<(Vec3, f32)> {
        if self.points.len() < 2 {
            return None;
        }
        // `FSUB float` and `FCOMP float` in the x87 register; the unit calculates with 24 bits (Direct3D
        // without `D3DCREATE_FPU_PRESERVE`, O-99), so the difference is already rounded to `f32`.
        let dx = self.bounds_max.x - self.bounds_min.x;
        if dx < TREAD_MIN_EXTENT {
            return None;
        }
        let e = Vec3::splat(dx);
        Some((self.bounds_min + e / 2.0, e.length() * 0.5))
    }

    /// Visibility check (`0x004F5440`): sight sphere against the view frustum
    /// (`Renderer_TestSphereInFrustum` `0x00571D70`).
    pub fn visible(&self, frustum: &Frustum) -> bool {
        self.sight_sphere().is_some_and(|(center, radius)| {
            frustum.classify_sphere(center, radius) != Containment::Outside
        })
    }

    /// Game tick after deleting (`0x004F5EB0`): listed if [`TreadMark::visible`]. A
    /// listed track is drawn by the original in the following frames (`0x004F5560`); the
    /// drawing pass resets the hull to the oldest point (all axes) and extends it
    /// by the current point of each drawn segment (x, z). The reimplementation executes this part
    /// of the drawing pass once here, with the clock and fog of the tick, so that the hull follows only from the
    /// state and does not depend on the frame rate.
    pub(crate) fn tick(&mut self, frustum: &Frustum, now_ms: u32, fog: &dyn Fn(Vec3) -> u8) {
        self.listed = self.visible(frustum);
        if self.listed {
            let oldest = self.points[0].position;
            self.bounds_min = oldest;
            self.bounds_max = oldest;
            for s in self.segments(now_ms, fog) {
                if let Segment::Draw { cur, .. } = s {
                    self.extend_bounds(cur.position);
                }
            }
        }
    }

    /// Gap (`0x004F5FC0`): only with at least two points and if the last one is not a gap.
    /// As in the original without checking the maximum count (`0x00451A30` only appends): a gap at the
    /// maximum extends the list by one point that [`TreadMark::add_point`] does not remove again
    /// (it removes only one per point); only aging shortens it.
    pub fn add_gap(&mut self, now_ms: u32) {
        if self.points.len() > 1 && self.points.back().is_some_and(|p| !p.gap) {
            self.points.push_back(TreadPoint {
                position: Vec3::ZERO,
                time_ms: now_ms,
                gap: true,
            });
        }
    }

    /// Owner gone (`+0x2C`).
    pub fn release(&mut self) {
        self.owner_gone = true;
    }

    /// Is the owner gone?
    pub fn released(&self) -> bool {
        self.owner_gone
    }

    fn age(now_ms: u32, p: &TreadPoint) -> i32 {
        now_ms.wrapping_sub(p.time_ms) as i32
    }

    /// Removes the points at the front that are older than the lifetime (never the last one). The original
    /// does this when drawing (`0x004F5560` -> `0x004F54E0`); the reimplementation in the game tick for all
    /// tread marks, [`TreadMark::quads`] additionally skips them.
    pub(crate) fn prune(&mut self, now_ms: u32) {
        while self.points.len() >= 2 && Self::age(now_ms, &self.points[0]) > self.life_ms as i32 {
            self.points.pop_front();
        }
    }

    /// Delete in the game tick (`0x004F5EB0`): owner gone and fewer than two points.
    pub(crate) fn finished(&self) -> bool {
        self.owner_gone && self.points.len() < 2
    }

    /// Strips of the frame (`0x004F5560`): per segment of two points without a gap a quad with
    /// half width 4 perpendicular to the direction (`Vector3_Cross(normalized(previous - current), up)`),
    /// corners at terrain height `height(x, z)` (`Terrain_GetHeightAt`). A segment is drawn only
    /// if `fog` returns bit 2 at the tile (x of the previous, z of the current point). A gap
    /// starts the strip anew, an unseen segment does not: the next seen segment attaches
    /// to the end corners of the last drawn one (D-284). Alpha `clamp(min(a_index, a_age), 0,
    /// 0x60)` with integer division: start corners with the age of the previous, end corners with that of the
    /// current point, both with its index (counting backward from the newest point 0). Segments whose
    /// previous point is older than the lifetime are dropped.
    pub fn quads(
        &self,
        now_ms: u32,
        height: &dyn Fn(f32, f32) -> f32,
        fog: &dyn Fn(Vec3) -> u8,
    ) -> Vec<TreadQuad> {
        let mut out = Vec::new();
        let life = self.life_ms as i32;
        let max = self.max as i32;
        let alpha = |index: i32, age: i32| -> u8 {
            let by_index = if index < 31 {
                TREAD_ALPHA
            } else {
                (max - index) * TREAD_ALPHA / (max - 30)
            };
            let by_age = if age <= 3000 {
                TREAD_ALPHA
            } else {
                (life - age) * TREAD_ALPHA / 3000
            };
            by_index.min(by_age).clamp(0, TREAD_ALPHA) as u8
        };
        let on_ground = |v: Vec3| Vec3::new(v.x, height(v.x, v.z), v.z);
        // Segments of the running strip (`iStack_284`) and end corners of the last drawn one.
        let mut strip = 0u32;
        let mut end = [Vec3::ZERO; 2];
        for s in self.segments(now_ms, fog) {
            let (index, p, cur, prev_age) = match s {
                Segment::Break => {
                    strip = 0;
                    continue;
                }
                Segment::Draw {
                    index,
                    prev,
                    cur,
                    prev_age,
                } => (index, prev, cur, prev_age),
            };
            // `Vector3_Normalize` (`0x00575170`: `1/sqrt(z^2 + y^2 + x^2)`, without zero check) and
            // `Vector3_Cross(n, (0, 1, 0))` (`0x005751F0`) = `(-n.z, 0, n.x)`.
            let d = p.position - cur.position;
            let inv = 1.0 / (d.z * d.z + d.y * d.y + d.x * d.x).sqrt();
            let side = Vec3::new(-(d.z * inv), 0.0, d.x * inv) * TREAD_HALF_WIDTH;
            let (a, b) = if strip == 0 {
                (on_ground(p.position + side), on_ground(p.position - side))
            } else {
                (end[0], end[1])
            };
            end = [
                on_ground(cur.position + side),
                on_ground(cur.position - side),
            ];
            strip += 1;
            let quad = TreadQuad {
                corners: [a, b, end[0], end[1]],
                alpha: [
                    alpha(index, prev_age),
                    alpha(index, Self::age(now_ms, &cur)),
                ],
            };
            // Two equal points give undefined corners (NaN) in the original: draw nothing.
            if quad.corners.iter().all(|c| c.is_finite()) {
                out.push(quad);
            }
        }
        out
    }

    /// Selection of the segments in the drawing pass (`0x004F5560`), in point order: segments whose
    /// previous point is older than the lifetime are dropped without consequences; a gap at
    /// one of the points starts the strip anew; a segment whose tile (x of the previous, z of the
    /// current point) `fog` returns without bit 2 is dropped without ending the strip.
    fn segments(&self, now_ms: u32, fog: &dyn Fn(Vec3) -> u8) -> Vec<Segment> {
        let mut out = Vec::new();
        if self.points.len() < 2 {
            return out;
        }
        let life = self.life_ms as i32;
        let mut index = self.points.len() as i32 - 1;
        let mut prev = self.points[0];
        for &cur in self.points.iter().skip(1) {
            index -= 1;
            let p = std::mem::replace(&mut prev, cur);
            let prev_age = Self::age(now_ms, &p);
            if prev_age > life {
                continue;
            }
            if p.gap || cur.gap {
                out.push(Segment::Break);
                continue;
            }
            let probe = Vec3::new(p.position.x, cur.position.y, cur.position.z);
            if fog(probe) & 2 == 0 {
                continue;
            }
            out.push(Segment::Draw {
                index,
                prev: p,
                cur,
                prev_age,
            });
        }
        out
    }
}

/// Model of the water ring of the units (`0x004EEBE0` type 2, string `0x006210B0`).
pub const SURF_RING_MODEL: &str = "Data/R3D/Nature/Summer/Surf/einheit_brandung01.rdo";

/// Water ring of a wading or swimming unit (`SurfRing`, vtable `0x00620CD8`, type 2,
/// layer 1; re/world-display): a model at the water height that
/// grows from scale 0.9 to 2.3 in 15 drawn game ticks while fading out.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfRing {
    /// Position, y = water height (`GameWorld+0x198`).
    position: Vec3,
    yaw: f32,
    scale: f32,
    alpha: f32,
    phase: f32,
    /// Hull of the model (`Model_GetBoundsCenter`, `Model_GetBoundingRadius`) for the visibility.
    bounds_center: Vec3,
    bounds_radius: f32,
    model: String,
    dead: bool,
}

impl SurfRing {
    /// `SurfRingEffect_Construct` (`0x004EEBE0`) with type 2: scale 0.8, opacity 0, phase pi/2.
    /// `position.y` is already the water height; `bounds_center`/`bounds_radius` from the hull of
    /// [`SURF_RING_MODEL`] (without a model the host creates no ring).
    pub fn new(position: Vec3, yaw: f32, bounds_center: Vec3, bounds_radius: f32) -> SurfRing {
        SurfRing {
            position,
            yaw,
            scale: 0.8,
            alpha: 0.0,
            phase: std::f32::consts::FRAC_PI_2,
            bounds_center,
            bounds_radius,
            model: nml_rda::normalize_path(SURF_RING_MODEL),
            dead: false,
        }
    }

    pub fn position(&self) -> Vec3 {
        self.position
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// Scale in x and z (y stays 1).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Opacity (`Material_SetAlpha`).
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Model path (normalized).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Game tick (`0x004EEB70`): scale +0.1, opacity `0.5 * sin(phi)` (before counting on),
    /// phi + 0.1, dead at phi >= pi (`vf+0x0C`). Return (alive, visible): dead -> deleted; otherwise
    /// visible according to `SurfRing_UpdateVisibility` (`0x004EE780`): sphere around `position + hull center`
    /// (unrotated, unscaled) with radius `hull radius * scale` in the view frustum and the tile of the
    /// position currently seen by the local player (`QueryFogOfWarArea(..., -1, 0) > 1`). No
    /// stealth check (D-269).
    pub fn tick(&mut self, frustum: &Frustum, fog: &dyn Fn(Vec3) -> u8) -> (bool, bool) {
        self.scale += 0.1;
        self.alpha = (f64::from(self.phase).sin() as f32) * 0.5;
        self.phase += 0.1;
        if self.phase >= std::f32::consts::PI {
            self.dead = true;
        }
        if self.dead {
            return (false, false);
        }
        let visible = frustum.classify_sphere(
            self.position + self.bounds_center,
            self.bounds_radius * self.scale,
        ) != Containment::Outside
            && fog(self.position) > 1;
        (true, visible)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::f32::consts::FRAC_PI_2;

    use nml_world::camera::{game_projection, look_at_lh};

    use super::*;

    fn close(a: Vec3, b: Vec3) {
        assert!(a.distance(b) < 1e-3, "{a} != {b}");
    }

    /// Camera above the origin (like the tests of the effect world).
    fn frustum() -> Frustum {
        let view = look_at_lh(Vec3::new(0.0, 500.0, -500.0), Vec3::ZERO, Vec3::Y);
        Frustum::from_view_proj(game_projection(0.8, 1.0, 700.0) * view)
    }

    fn seen(_: Vec3) -> u8 {
        7
    }

    #[test]
    fn footprint_offsets_follow_side_and_pair() {
        let p = Vec3::new(1000.0, 5.0, 2000.0);
        let yaw = 0.3_f32;
        let across = Vec3::new(yaw.cos(), 0.0, yaw.sin());
        // Type 1 (human): side 0 -> - (cos, sin) * 10, side 1 -> +; only mark 0.
        let f = Footprint::new(p, yaw, 0, 1, 2);
        assert_eq!(f.used(), 1);
        assert_eq!(f.yaw(), yaw);
        close(f.quad()[0], p - across * 10.0);
        assert_eq!(&f.quad()[1..], &[p; 3]);
        close(Footprint::new(p, yaw, 1, 1, 2).quad()[0], p + across * 10.0);
        // Type 6 (animal, bit 2): pair lengthwise +/-30 (`rot +- pi/2`), both with the side offset.
        let along = Vec3::new((yaw + FRAC_PI_2).cos(), 0.0, (yaw + FRAC_PI_2).sin());
        let f = Footprint::new(p, yaw, 1, 6, 2);
        assert_eq!(f.used(), 3);
        close(f.quad()[0], p + along * 30.0 + across * 10.0);
        close(f.quad()[1], p - along * 30.0 + across * 10.0);
        assert_eq!(&f.quad()[2..], &[p; 2]);
        let f = Footprint::new(p, yaw, 0, 6, 2);
        close(f.quad()[0], p + along * 30.0 - across * 10.0);
        close(f.quad()[1], p - along * 30.0 - across * 10.0);
        // Crater (bit 0x10): no offset.
        let f = Footprint::new(p, yaw, 1, 0xD0, 2);
        assert_eq!(f.used(), 1);
        assert_eq!(f.quad(), &[p; 4]);
        assert_eq!(f.kind(), 0xD0);
    }

    #[test]
    fn footprint_texture_and_radius_follow_the_kind_bits() {
        let name = |kind| Footprint::texture(kind).strip_prefix(FOOTSTEP_DIR).unwrap();
        assert_eq!(name(1), "Footstep.tga");
        assert_eq!(name(6), "footstep_animal.tga");
        assert_eq!(name(0x802), "Footstep02.tga");
        assert_eq!(name(0xD0), "bombbuilding1.tga");
        assert_eq!(name(0x150), "bombbuilding2.tga");
        assert_eq!(name(0x250), "bombbuilding3.tga");
        assert_eq!(name(0x430), "bomb.tga");
        assert_eq!(name(0), "Footstep03.tga");
        // Order 0x400 before 0x80 before ... before 0x1.
        assert_eq!(name(0x481), "bomb.tga");
        assert_eq!(name(0x805), "Footstep.tga");
        assert_eq!(Footprint::radius(1), 12.0);
        assert_eq!(Footprint::radius(0x430), 96.0);
        assert_eq!(Footprint::radius(0xD0), 300.0);
        assert_eq!(Footprint::radius(0x60), 300.0);
    }

    #[test]
    fn footprint_alpha_is_full_until_five_seconds_left() {
        let mut f = Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2);
        let mut alpha = |life| {
            f.life_ms = life;
            f.alpha()
        };
        // `ftol(life * 0.0002f * 255f)` in f32: 5,000 -> 255 (in f64 it would be 254).
        assert_eq!(alpha(20_000), 255);
        assert_eq!(alpha(5_000), 255);
        assert_eq!(alpha(4_999), 254);
        assert_eq!(alpha(2_500), 127);
        assert_eq!(alpha(100), 5);
        assert_eq!(alpha(0), 0);
    }

    #[test]
    fn footprint_lives_20_s_or_10_s_below_quality_2() {
        assert_eq!(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2).life_ms(), 20_000);
        assert_eq!(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 3).life_ms(), 20_000);
        assert_eq!(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 1).life_ms(), 10_000);
        assert_eq!(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 0).life_ms(), 10_000);
        // 200 ticks listed (in the last one with alpha 0), in the 201st tick dead and gone (`0x004F1B50`).
        let mut f = Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2);
        for i in 1..=200 {
            assert!(f.tick(&frustum(), &seen), "tick {i}");
            assert!(f.visible());
        }
        assert_eq!((f.life_ms(), f.alpha()), (0, 0));
        assert!(!f.tick(&frustum(), &seen));
        let mut f = Footprint::new(Vec3::ZERO, 0.0, 0, 1, 1);
        assert_eq!((0..101).filter(|_| f.tick(&frustum(), &seen)).count(), 100);
    }

    #[test]
    fn footprint_is_seen_only_through_its_first_mark() {
        // Pair (type 6) with yaw 0: mark 0 at (0, 0, 30) +/-10 across, mark 1 at (0, 0, -30).
        let mut f = Footprint::new(Vec3::ZERO, 0.0, 1, 6, 2);
        let first = f.quad()[0];
        assert!(first.z > 0.0 && f.quad()[1].z < 0.0);
        // Fog only over the first mark: invisible although mark 1 is clear (D-272).
        let fog_over_first = |p: Vec3| if p.z > 0.0 { 1 } else { 7 };
        assert!(f.tick(&frustum(), &fog_over_first));
        assert!(!f.visible());
        // Currently seen (> 1): visible again; explored (1) is not enough.
        assert!(f.tick(&frustum(), &seen));
        assert!(f.visible());
        let fog_under_second = |p: Vec3| if p.z < 0.0 { 0 } else { 2 };
        assert!(f.tick(&frustum(), &fog_under_second));
        assert!(f.visible());
        // First mark outside the view frustum: invisible, but not dead.
        let mut far = Footprint::new(Vec3::new(100_000.0, 0.0, 0.0), 0.0, 0, 1, 2);
        assert!(far.tick(&frustum(), &seen));
        assert!(!far.visible());
    }

    fn tread(quality: u8, points: &[(Vec3, u32)]) -> TreadMark {
        let mut t = TreadMark::new(quality);
        for &(p, time) in points {
            t.add_point(p, time);
        }
        t
    }

    fn flat(_: f32, _: f32) -> f32 {
        0.0
    }

    #[test]
    fn tread_alpha_by_age_and_index() {
        let (p0, p1) = (Vec3::ZERO, Vec3::new(0.0, 0.0, 100.0));
        let t = tread(2, &[(p0, 0), (p1, 200)]);
        // Corners: side = cross product (previous - current position) x up = (1, 0, 0), half width 4,
        // height from the terrain.
        let height = |x: f32, _z: f32| x * 0.5;
        let q = t.quads(200, &height, &seen);
        assert_eq!(q.len(), 1);
        assert_eq!(
            q[0].corners,
            [
                Vec3::new(4.0, 2.0, 0.0),
                Vec3::new(-4.0, -2.0, 0.0),
                Vec3::new(4.0, 2.0, 100.0),
                Vec3::new(-4.0, -2.0, 100.0),
            ]
        );
        assert_eq!(q[0].alpha, [0x60, 0x60]);
        // Age: start corners with the age of the previous, end corners with that of the current point,
        // linear over 3 s up to the lifetime (integer division).
        assert_eq!(t.quads(18_000, &flat, &seen)[0].alpha, [64, 70]);
        assert_eq!(t.quads(20_000, &flat, &seen)[0].alpha, [0, 6]);
        // Older than the lifetime: segment dropped.
        assert!(t.quads(20_001, &flat, &seen).is_empty());
        // Index counting backward from the newest point (0): from 31 `(max - i) * 0x60 / (max - 30)`.
        let line: Vec<(Vec3, u32)> = (0..40)
            .map(|i| (Vec3::new(0.0, 0.0, 10.0 * i as f32), 1000))
            .collect();
        let q = tread(2, &line).quads(1000, &flat, &seen);
        assert_eq!(q.len(), 39);
        assert_eq!(q[0].alpha, [85, 85]); // Index 38: 62 * 96 / 70
        assert_eq!(q[7].alpha, [94, 94]); // Index 31
        assert_eq!(q[8].alpha, [0x60, 0x60]); // Index 30
        assert_eq!(q[38].alpha, [0x60, 0x60]); // Index 0
        // With `ParticlesQuality < 2` at most 50 points: index 38 -> 12 * 96 / 20.
        let q = tread(1, &line).quads(1000, &flat, &seen);
        assert_eq!(q[0].alpha, [57, 57]);
    }

    #[test]
    fn tread_drops_the_oldest_point_above_max() {
        let line = |n: u32| -> Vec<(Vec3, u32)> {
            (0..n)
                .map(|i| (Vec3::new(i as f32, 0.0, 0.0), 200 * i))
                .collect()
        };
        let t = tread(2, &line(101));
        assert_eq!(t.points().len(), 100);
        assert_eq!(t.points()[0].position.x, 1.0);
        let t = tread(1, &line(51));
        assert_eq!(t.points().len(), 50);
        assert_eq!(t.points()[0].position.x, 1.0);
        assert_eq!(t.points()[49].time_ms, 10_000);
        assert_eq!(TreadMark::new(2).life_ms, 20_000);
        assert_eq!(TreadMark::new(1).life_ms, 10_000);
    }

    #[test]
    fn tread_gap_only_once() {
        let mut t = TreadMark::new(2);
        // Gaps only from two points on (`0x004F5FC0`: count > 1) and never twice in a row.
        t.add_gap(0);
        assert!(t.points().is_empty());
        t.add_point(Vec3::ZERO, 0);
        t.add_gap(0);
        assert_eq!(t.points().len(), 1);
        t.add_point(Vec3::new(0.0, 0.0, 100.0), 200);
        t.add_gap(400);
        t.add_gap(400);
        assert_eq!(t.points().len(), 3);
        assert!(t.points()[2].gap);
        assert_eq!(t.points()[2].time_ms, 400);
        // After the gap a new strip begins perpendicular to its own direction.
        t.add_point(Vec3::new(500.0, 0.0, 100.0), 600);
        t.add_point(Vec3::new(600.0, 0.0, 100.0), 800);
        let q = t.quads(800, &flat, &seen);
        assert_eq!(q.len(), 2);
        // (500 -> 600): direction (-1, 0, 0), side (0, 0, -1).
        assert_eq!(
            q[1].corners,
            [
                Vec3::new(500.0, 0.0, 96.0),
                Vec3::new(500.0, 0.0, 104.0),
                Vec3::new(600.0, 0.0, 96.0),
                Vec3::new(600.0, 0.0, 104.0),
            ]
        );
        // Gaps without checking the maximum count: at the maximum the list stays one point longer.
        let mut t = TreadMark::new(1);
        for i in 0..50 {
            t.add_point(Vec3::X * i as f32, 0);
        }
        t.add_gap(0);
        assert_eq!(t.points().len(), 51);
        t.add_point(Vec3::ZERO, 0);
        assert_eq!(t.points().len(), 51);
    }

    #[test]
    fn tread_bridges_unseen_segments() {
        let p = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, 200.0),
            Vec3::new(100.0, 0.0, 300.0),
        ];
        let t = tread(2, &[(p[0], 0), (p[1], 200), (p[2], 400), (p[3], 600)]);
        let probes = RefCell::new(Vec::new());
        // The tile at z = 200 is unseen.
        let fog = |at: Vec3| {
            probes.borrow_mut().push((at.x, at.z));
            if at.z == 200.0 { 1 } else { 7 }
        };
        let q = t.quads(600, &flat, &fog);
        // Fog probe at the tile (x of the previous, z of the current point).
        assert_eq!(*probes.borrow(), [(0.0, 100.0), (0.0, 200.0), (0.0, 300.0)]);
        // Segment 1 -> 2 is missing; 2 -> 3 attaches to the end corners of segment 0 -> 1 (D-284).
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].corners[0], q[0].corners[2]);
        assert_eq!(q[1].corners[1], q[0].corners[3]);
        // New end corners perpendicular to the direction 2 -> 3: side (sqrt(1/2), 0, -sqrt(1/2)) * 4.
        let side = Vec3::new(1.0, 0.0, -1.0).normalize() * 4.0;
        close(q[1].corners[2], p[3] + side);
        close(q[1].corners[3], p[3] - side);
        // Without fog: three segments, the third attaches to the second.
        let q = t.quads(600, &flat, &seen);
        assert_eq!(q.len(), 3);
        assert_eq!(q[2].corners[0], q[1].corners[2]);
    }

    #[test]
    fn tread_bounds_follow_new_points_and_the_drawn_segments() {
        let (p0, p1, p2) = (
            Vec3::new(10.0, 5.0, 20.0),
            Vec3::new(-30.0, 50.0, 60.0),
            Vec3::new(40.0, -50.0, -10.0),
        );
        let mut t = TreadMark::new(2);
        // First point: all three axes (`0x004F5ED0`, count 1).
        t.add_point(p0, 0);
        assert_eq!(t.bounds(), (p0, p0));
        // Further points only in x and z; y stays at the first point.
        t.add_point(p1, 200);
        t.add_point(p2, 400);
        let added = (Vec3::new(-30.0, 5.0, -10.0), Vec3::new(40.0, 5.0, 60.0));
        assert_eq!(t.bounds(), added);
        // Gaps leave the hull alone (`0x004F5FC0`).
        t.add_gap(600);
        assert_eq!(t.bounds(), added);
        // Drawing pass (`0x004F5560`) in the listed tick: back to the oldest point (all
        // axes), then extended by the current point of each drawn segment (x, z). The
        // segment p0 -> p1 is in fog (probe at z 60): p1 does not count.
        let fog = |at: Vec3| if at.z == 60.0 { 1 } else { 7 };
        t.tick(&frustum(), 600, &fog);
        assert!(t.listed());
        assert_eq!(
            t.bounds(),
            (Vec3::new(10.0, 5.0, -10.0), Vec3::new(40.0, 5.0, 20.0))
        );
        // New points extend the reset hull again only in x and z.
        t.add_point(Vec3::new(-100.0, 900.0, 30.0), 800);
        assert_eq!(
            t.bounds(),
            (Vec3::new(-100.0, 5.0, -10.0), Vec3::new(40.0, 5.0, 30.0))
        );
        // If the oldest point has dropped, the drawing pass resets to the new oldest one,
        // also in y. A gap at the front sets the hull to (0, 0, 0).
        let mut t = tread(2, &[(p0, 0), (p1, 15_000), (p2, 16_000)]);
        t.prune(20_001);
        assert_eq!(t.points()[0].position, p1);
        t.tick(&frustum(), 20_001, &seen);
        assert!(t.listed());
        assert_eq!(
            t.bounds(),
            (Vec3::new(-30.0, 50.0, -10.0), Vec3::new(40.0, 50.0, 60.0))
        );
        let mut t = tread(2, &[(p0, 0), (p1, 0)]);
        t.add_gap(15_000);
        t.add_point(p2, 16_000);
        t.add_point(Vec3::new(60.0, 0.0, 0.0), 16_000);
        t.prune(20_001);
        assert!(t.points()[0].gap);
        t.tick(&frustum(), 20_001, &seen);
        assert_eq!(t.bounds(), (Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0)));
    }

    #[test]
    fn tread_along_z_is_never_visible() {
        // Exactly along z (same x): the track itself would be in view ...
        let mut t = tread(
            2,
            &[
                (Vec3::new(0.0, 0.0, -50.0), 0),
                (Vec3::new(0.0, 0.0, 50.0), 200),
            ],
        );
        assert_eq!(t.quads(200, &flat, &seen).len(), 1);
        assert_ne!(
            frustum().classify_sphere(Vec3::ZERO, TREAD_HALF_WIDTH),
            Containment::Outside
        );
        // ... but without x extent there is no sight sphere (`0x004F5440`: dx < 0.01).
        assert_eq!(t.sight_sphere(), None);
        assert!(!t.visible(&frustum()));
        t.tick(&frustum(), 200, &seen);
        assert!(!t.listed());
        // Boundary: dx 0.009 never, dx 0.01 yes.
        let short =
            |dx: f32| tread(2, &[(Vec3::ZERO, 0), (Vec3::new(dx, 0.0, 100.0), 0)]).sight_sphere();
        assert_eq!(short(0.009), None);
        assert!(short(0.01).is_some());
        // Below two points never.
        let one = tread(2, &[(Vec3::ZERO, 0)]);
        assert_eq!(one.sight_sphere(), None);
        assert!(!one.visible(&frustum()));
    }

    #[test]
    fn tread_sight_sphere_sits_at_the_min_corner() {
        // The track ends in view but begins behind the camera.
        let (start, end) = (Vec3::new(0.0, 0.0, -2000.0), Vec3::new(10.0, 0.0, 0.0));
        let mut t = tread(2, &[(start, 0), (end, 200)]);
        assert_ne!(
            frustum().classify_sphere(end, TREAD_HALF_WIDTH),
            Containment::Outside
        );
        assert_eq!(t.quads(200, &flat, &seen).len(), 1);
        // Sphere: corner (min.x, min.y, min.z) + (dx, dx, dx)/2, radius |(dx, dx, dx)| * 0.5 =
        // 0.87 * dx - from the x extent 10 only, not around the 2000-long track.
        let (center, radius) = t.sight_sphere().unwrap();
        assert_eq!(center, Vec3::new(5.0, 5.0, -1995.0));
        assert!((radius - 8.660_254).abs() < 1e-5, "{radius}");
        assert_eq!(
            frustum().classify_sphere(center, radius),
            Containment::Outside
        );
        assert!(!t.visible(&frustum()));
        t.tick(&frustum(), 200, &seen);
        assert!(!t.listed());
    }

    #[test]
    fn slanted_tread_in_view_is_visible() {
        let mut t = tread(
            2,
            &[
                (Vec3::new(-50.0, 0.0, -50.0), 0),
                (Vec3::new(50.0, 0.0, 50.0), 200),
            ],
        );
        let (center, radius) = t.sight_sphere().unwrap();
        assert_eq!(center, Vec3::new(0.0, 50.0, 0.0));
        assert!((radius - 86.602_54).abs() < 1e-4, "{radius}");
        assert!(t.visible(&frustum()));
        t.tick(&frustum(), 200, &seen);
        assert!(t.listed());
        // Far to the side: not listed.
        let mut far = tread(
            2,
            &[
                (Vec3::new(100_000.0, 0.0, 0.0), 0),
                (Vec3::new(100_050.0, 0.0, 50.0), 200),
            ],
        );
        far.tick(&frustum(), 200, &seen);
        assert!(!far.listed());
    }

    #[test]
    fn tread_without_two_points_draws_nothing() {
        assert!(TreadMark::new(2).quads(0, &flat, &seen).is_empty());
        let t = tread(2, &[(Vec3::ZERO, 0)]);
        assert!(t.quads(0, &flat, &seen).is_empty());
    }

    #[test]
    fn surf_ring_grows_and_fades_for_fifteen_drawn_ticks() {
        let mut ring = SurfRing::new(Vec3::new(0.0, -5.0, 0.0), 0.3, Vec3::ZERO, 10.0);
        assert_eq!(ring.position(), Vec3::new(0.0, -5.0, 0.0));
        assert_eq!((ring.scale(), ring.alpha()), (0.8, 0.0));
        let mut drawn = Vec::new();
        loop {
            let (alive, visible) = ring.tick(&frustum(), &seen);
            if !alive {
                break;
            }
            assert!(visible);
            drawn.push((ring.scale(), ring.alpha()));
        }
        // phi from pi/2 in steps of 0.1: in the 16th tick phi >= pi -> dead; 15 drawn ticks.
        assert_eq!(drawn.len(), 15);
        assert!((drawn[0].0 - 0.9).abs() < 1e-5, "{:?}", drawn[0]);
        assert!((drawn[14].0 - 2.3).abs() < 1e-4, "{:?}", drawn[14]);
        // Opacity `0.5 * sin(phi)` before counting on: 0.5 in the first tick, in the 15th with
        // phi = pi/2 + 1.4 only `0.5 * sin(2.97)` ~ 0.085.
        assert!((drawn[0].1 - 0.5).abs() < 1e-6);
        assert!(drawn.windows(2).all(|w| w[1].1 < w[0].1));
        assert!((drawn[14].1 - 0.085).abs() < 1e-3, "{:?}", drawn[14]);
    }

    #[test]
    fn surf_ring_is_seen_through_its_model_sphere_and_the_fog() {
        // Far to the side: sphere around pos + center with radius r * scale.
        let far = Vec3::new(5000.0, 0.0, 0.0);
        let mut ring = SurfRing::new(far, 0.0, Vec3::ZERO, 10.0);
        assert_eq!(ring.tick(&frustum(), &seen), (true, false));
        // The center of the hull moves the sphere into view.
        let mut ring = SurfRing::new(far, 0.0, Vec3::new(-5000.0, 0.0, 0.0), 10.0);
        assert_eq!(ring.tick(&frustum(), &seen), (true, true));
        // Fog: only currently seen (> 1) counts; queried at the position, not at the center.
        let probes = RefCell::new(Vec::new());
        let explored = |p: Vec3| {
            probes.borrow_mut().push(p);
            1
        };
        let mut ring = SurfRing::new(Vec3::ZERO, 0.0, Vec3::new(3.0, 0.0, 0.0), 10.0);
        assert_eq!(ring.tick(&frustum(), &explored), (true, false));
        assert_eq!(probes.borrow().as_slice(), [Vec3::ZERO]);
    }
}
