//! Projectile trajectories of the host (re/combat-host 2): arrow (parabola, target tracking,
//! ghost trail, fall, sticking), cannonball (parabola, invisible first quarter, smoke every
//! 10 ms) and the headhunter's dynamite throw (arrow trajectory with a whole model, re/headhunter-robbing 7).
//! Presentation only: not in the savegame; rebuilt from the sim projectiles after loading.

use glam::{Affine3A, Vec3};

/// Gravity of the trajectories (`0x00620F94`): `y -= e^2*0.0005`.
pub const ARROW_GRAVITY: f32 = 0.0005;
/// Lifetime of a ghost copy (`0x004F25E0`).
pub const GHOST_MS: f32 = 150.0;
/// Time a missing arrow stays stuck (`+0xC0`).
pub const STUCK_MS: f32 = 2000.0;
/// Interval between the ball's smoke puffs.
pub const PUFF_MS: f32 = 10.0;

/// Whole game-time milliseconds of a frame with `dt_ms` of virtual time (`factor` =
/// [`crate::game::sim_host::SimHost::game_time_factor`]); the fraction stays in `carry`. The original
/// advances the trajectories per frame with game time (re/combat-host 2.1, re/effects), not with the clock.
pub fn game_ms(carry: &mut f64, dt_ms: f64, factor: f64) -> f32 {
    let total = *carry + dt_ms * factor;
    let whole = total.floor();
    *carry = total - whole;
    whole as f32
}

/// `ArrowProjectile_FallToGround` (`0x004ED620`, re/combat-projectiles 15.5): remaining flight time to the
/// ground.
pub fn fall_ms(vy: f32, above_ground: f32) -> i32 {
    ((vy + (vy * vy + above_ground * 0.002).max(0.0).sqrt()) * 999.999_94) as i32
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Arrow {
        fire: bool,
    },
    Ball {
        yaw_extra: f32,
    },
    /// Dynamite throw (`ArrowProjectile` with flags `0x201`, re/combat-projectiles 2.2): trajectory, orientation
    /// and ghost trail of the arrow without fire, but the whole model [`Flight::model`]
    /// (`dynamit.rdo`) instead of a mesh in the shooter model (as in the original).
    Throw,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Flying,
    Falling { rest_ms: f32 },
    Stuck { rest_ms: f32 },
    Done,
}

/// Model of an arrow mesh: path of the model that contains it, and the mesh's node matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshRef {
    pub path: String,
    pub node: Affine3A,
}

/// One copy of the ghost trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghost {
    pub pos: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub age_ms: f32,
}

impl Ghost {
    pub fn alpha(&self) -> f32 {
        1.0 - self.age_ms / GHOST_MS
    }
}

/// Output of an advance.
#[derive(Debug, Default)]
pub struct Advance {
    /// Ball: smoke puffs `(position, start time = remaining frame time)`.
    pub puffs: Vec<(Vec3, u32)>,
    /// Ball visible (after the first quarter).
    pub ball_visible: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flight {
    pub id: u32,
    pub kind: Kind,
    /// Mesh of the arrow (110007 of the shooter); `None` = no model.
    pub mesh: Option<String>,
    /// Model that contains this mesh (main model or sub-object of the shooter model); the host
    /// sets it on creation, `None` = not found.
    pub model: Option<MeshRef>,
    /// Type of the shooter (arrow model); 0 = unknown.
    pub shooter_guid: u32,
    pub start: Vec3,
    pub t_ms: f32,
    pub elapsed_ms: f32,
    pub vel: Vec3,
    pub arc: f32,
    pub phase: Phase,
    pub pos: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub ghosts: Vec<Ghost>,
    pub next_puff_ms: f32,
    /// Key of the ball or fire-arrow fire effect.
    pub effect: Option<u32>,
    /// Terrain height below the impact point of a missing arrow (set by [`Flight::miss`]).
    pub ground: f32,
}

impl Flight {
    pub fn arrow(
        id: u32,
        start: Vec3,
        aim: Vec3,
        t_ms: f32,
        mesh: Option<String>,
        fire: bool,
    ) -> Self {
        let t = t_ms.max(1.0);
        let mut f = Self {
            id,
            kind: Kind::Arrow { fire },
            mesh,
            model: None,
            shooter_guid: 0,
            start,
            t_ms: t,
            elapsed_ms: 0.0,
            vel: (aim - start) / t,
            arc: t * ARROW_GRAVITY,
            phase: Phase::Flying,
            pos: start,
            pitch: 0.0,
            yaw: 0.0,
            ghosts: Vec::new(),
            next_puff_ms: 0.0,
            effect: None,
            ground: 0.0,
        };
        f.orient(0.0);
        f
    }

    pub fn ball(id: u32, start: Vec3, aim: Vec3, t_ms: f32, yaw_extra: f32) -> Self {
        let t = t_ms.max(1.0);
        let mut f = Self::arrow(id, start, aim, t, None, false);
        f.kind = Kind::Ball { yaw_extra };
        f.next_puff_ms = t / 4.0;
        f
    }

    /// Dynamite throw (re/headhunter-robbing 7): arrow trajectory without fire, whole model
    /// `model_path` with an identity node, no mesh.
    pub fn throw(id: u32, start: Vec3, aim: Vec3, t_ms: f32, model_path: &str) -> Self {
        let mut f = Self::arrow(id, start, aim, t_ms, None, false);
        f.kind = Kind::Throw;
        f.model = Some(MeshRef {
            path: nml_rda::normalize_path(model_path),
            node: Affine3A::IDENTITY,
        });
        f
    }

    /// After loading: remaining time from the sim projectile.
    pub fn resume_arrow(
        id: u32,
        start: Vec3,
        aim: Vec3,
        flight_ms: i32,
        remaining_ms: i32,
        mesh: Option<String>,
        fire: bool,
    ) -> Self {
        let mut f = Self::arrow(id, start, aim, flight_ms as f32, mesh, fire);
        f.elapsed_ms = flight_ms.wrapping_sub(remaining_ms).max(0) as f32;
        f.pos = f.arrow_pos(f.elapsed_ms);
        f.orient(f.elapsed_ms);
        f
    }

    pub fn resume_ball(
        id: u32,
        start: Vec3,
        aim: Vec3,
        flight_ms: i32,
        remaining_ms: i32,
        yaw_extra: f32,
    ) -> Self {
        let mut f = Self::ball(id, start, aim, flight_ms as f32, yaw_extra);
        f.elapsed_ms = flight_ms.wrapping_sub(remaining_ms).max(0) as f32;
        f.next_puff_ms = f.next_puff_ms.max(f.elapsed_ms);
        f.pos = f.arrow_pos(f.elapsed_ms);
        f.orient(f.elapsed_ms);
        f
    }

    /// Throw after loading: remaining time from the sim projectile like [`Flight::resume_arrow`].
    pub fn resume_throw(
        id: u32,
        start: Vec3,
        aim: Vec3,
        flight_ms: i32,
        remaining_ms: i32,
        model_path: &str,
    ) -> Self {
        let mut f = Self::throw(id, start, aim, flight_ms as f32, model_path);
        f.elapsed_ms = flight_ms.wrapping_sub(remaining_ms).max(0) as f32;
        f.pos = f.arrow_pos(f.elapsed_ms);
        f.orient(f.elapsed_ms);
        f
    }

    /// Position at flight time `e` (arrow, throw and ball; frame `0x004EDE80` or `0x004EFA50`).
    pub fn arrow_pos(&self, e: f32) -> Vec3 {
        let v = Vec3::new(self.vel.x, self.vel.y + self.arc, self.vel.z);
        let mut p = self.start + v * e;
        p.y -= e * e * ARROW_GRAVITY;
        p
    }

    fn orient(&mut self, e: f32) {
        let v = Vec3::new(self.vel.x, self.vel.y + self.arc, self.vel.z);
        let tang_y = v.y - e * 2.0 * ARROW_GRAVITY;
        let len = Vec3::new(v.x, tang_y, v.z).length().max(1e-6);
        self.pitch = -(tang_y / len).clamp(-1.0, 1.0).asin();
        self.yaw = std::f32::consts::PI - v.x.atan2(v.z);
    }

    /// Target tracking (`TrackTarget` `0x004ED690`, in the host per simulation step): new target point,
    /// x/z velocity for the remaining time, position continuous.
    pub fn track(&mut self, aim: Vec3) {
        let rest = self.t_ms - self.elapsed_ms;
        if self.phase != Phase::Flying || rest <= 399.0 {
            return;
        }
        self.vel.x = (aim.x - self.pos.x) / rest;
        self.vel.z = (aim.z - self.pos.z) / rest;
        let at = self.arrow_pos(self.elapsed_ms);
        self.start += self.pos - at;
    }

    /// Missed (`ProjectileEnded { hit: false }`): fall to the ground `ground` with `vy` = field
    /// `+0xE8` (velocity without arc) like `FallToGround`; the trajectory then runs past `t`.
    pub fn miss(&mut self, ground: f32) {
        let vy = self.vel.y;
        let rest = fall_ms(vy, self.pos.y - ground) as f32;
        self.ground = ground;
        self.phase = if rest > 0.0 {
            Phase::Falling { rest_ms: rest }
        } else {
            self.stick();
            self.phase
        };
    }

    /// Transition to sticking (state 4, re/combat-host 2.1): `pos.y = terrain`, 2,000 ms.
    fn stick(&mut self) {
        self.pos.y = self.ground;
        self.phase = Phase::Stuck { rest_ms: STUCK_MS };
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Advance by `dt` ms.
    pub fn advance(&mut self, dt: f32) -> Advance {
        let mut out = Advance::default();
        for g in &mut self.ghosts {
            g.age_ms += dt;
        }
        self.ghosts.retain(|g| g.age_ms < GHOST_MS);
        match self.phase {
            Phase::Flying | Phase::Falling { .. } => {
                let e = self.elapsed_ms + dt;
                if let Kind::Ball { .. } = self.kind {
                    while e > self.next_puff_ms
                        && self.next_puff_ms >= self.t_ms / 4.0
                        && self.next_puff_ms < self.t_ms
                    {
                        let at = self.arrow_pos(self.next_puff_ms);
                        out.puffs.push((at, (e - self.next_puff_ms) as u32));
                        // From 2^28 ms on, the addition no longer changes the `f32` (flight time unchecked from
                        // the savegame): then the loop ends. Below that it always changes it, so bit-identical.
                        let next = self.next_puff_ms + PUFF_MS;
                        if next == self.next_puff_ms {
                            break;
                        }
                        self.next_puff_ms = next;
                    }
                    out.ball_visible = e >= self.t_ms / 4.0;
                }
                self.elapsed_ms = e.min(if self.phase == Phase::Flying {
                    self.t_ms
                } else {
                    e
                });
                self.pos = self.arrow_pos(self.elapsed_ms);
                self.orient(self.elapsed_ms);
                // Ghost trail: arrow without fire with a mesh, throw with a model (same class
                // `ArrowProjectile`, re/combat-projectiles 2.2).
                let ghosted = match self.kind {
                    Kind::Arrow { fire: false } => self.mesh.is_some(),
                    Kind::Throw => self.model.is_some(),
                    _ => false,
                };
                if dt > 0.0 && ghosted {
                    self.ghosts.push(Ghost {
                        pos: self.pos,
                        pitch: self.pitch,
                        yaw: self.yaw,
                        age_ms: 0.0,
                    });
                }
                if let Phase::Falling { rest_ms } = self.phase {
                    let r = rest_ms - dt;
                    if r > 0.0 {
                        self.phase = Phase::Falling { rest_ms: r };
                    } else {
                        self.stick();
                    }
                }
            }
            Phase::Stuck { rest_ms } => {
                let r = rest_ms - dt;
                self.phase = if r > 0.0 {
                    Phase::Stuck { rest_ms: r }
                } else {
                    Phase::Done
                };
            }
            Phase::Done => {}
        }
        out
    }
}

/// All trajectories of a savegame.
#[derive(Debug, Default)]
pub struct Flights {
    pub list: Vec<Flight>,
}

impl Flights {
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Flight> {
        self.list.iter_mut().find(|f| f.id == id)
    }

    /// Number of flights (display in the viewer).
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn the_arrow_parabola_ends_at_the_aim_and_peaks_t2_over_8000_above_the_chord() {
        // re/combat-host 2.1.
        let (start, aim) = (Vec3::new(0.0, 10.0, 0.0), Vec3::new(700.0, 30.0, 0.0));
        let f = Flight::arrow(1, start, aim, 1000.0, None, false);
        assert!((f.arrow_pos(1000.0) - aim).length() < 1e-3);
        let chord_mid = (start + aim) * 0.5;
        assert!((f.arrow_pos(500.0).y - chord_mid.y - 1000.0 * 1000.0 / 8000.0).abs() < 1e-3);
    }

    #[test]
    fn the_ball_hides_its_first_quarter_and_puffs_every_10_ms() {
        // re/combat-host 2.2: t = 1792 -> invisible until 448 ms, ~134 puffs.
        let mut f = Flight::ball(2, Vec3::ZERO, Vec3::new(1792.0, 0.0, 0.0), 1792.0, 0.0);
        let mut puffs = 0;
        let mut visible_at = None;
        for step in 0..180 {
            let out = f.advance(10.0);
            puffs += out.puffs.len();
            if visible_at.is_none() && out.ball_visible {
                visible_at = Some(step * 10 + 10);
            }
        }
        assert_eq!(visible_at, Some(450));
        assert!((133..=135).contains(&puffs), "{puffs}");
    }

    #[test]
    fn a_missing_arrow_falls_by_the_formula_and_sticks_2000_ms() {
        // re/combat-projectiles 15.5: t = trunc((vy + sqrt(vy^2 + (y - h) * 0.002)) * 999.99994).
        assert_eq!(fall_ms(0.0, 0.0), 0);
        assert_eq!(fall_ms(0.0, 50.0), 316);
        let mut f = Flight::arrow(
            3,
            Vec3::ZERO,
            Vec3::new(700.0, 50.0, 0.0),
            1000.0,
            None,
            false,
        );
        f.advance(1000.0);
        f.miss(0.0);
        assert!(matches!(f.phase, Phase::Falling { .. }));
        let mut ms = 0.0;
        while matches!(f.phase, Phase::Falling { .. }) && ms < 5000.0 {
            f.advance(10.0);
            ms += 10.0;
        }
        assert!(matches!(f.phase, Phase::Stuck { .. }));
        let mut stuck = 0.0;
        while !f.done() {
            f.advance(100.0);
            stuck += 100.0;
        }
        assert_eq!(stuck, STUCK_MS);
    }

    #[test]
    fn a_missing_arrow_sticks_in_the_ground_not_below_it() {
        // re/combat-host 2.1, state 4: `pos.y = terrain`.
        let mut f = Flight::arrow(
            7,
            Vec3::new(0.0, 30.0, 0.0),
            Vec3::new(700.0, 20.0, 0.0),
            1000.0,
            None,
            false,
        );
        f.advance(1000.0);
        f.miss(20.0);
        while matches!(f.phase, Phase::Falling { .. }) {
            f.advance(10.0);
        }
        assert!(matches!(f.phase, Phase::Stuck { .. }));
        assert_eq!(f.pos.y, 20.0);
        let at = f.pos;
        f.advance(500.0);
        assert_eq!(f.pos, at, "stays stuck");
        // Already on the ground: sticks immediately at terrain height.
        let mut g = Flight::arrow(
            8,
            Vec3::ZERO,
            Vec3::new(700.0, -5.0, 0.0),
            1000.0,
            None,
            false,
        );
        g.advance(1000.0);
        g.miss(-5.0);
        assert!(matches!(g.phase, Phase::Stuck { .. }));
        assert_eq!(g.pos.y, -5.0);
    }

    #[test]
    fn flights_run_on_game_time_like_the_simulation() {
        // Default GameSpeed 14: game time 1.2 x clock; the trajectory ends with the sim projectile
        // (remaining time counts game time), not 17 % before.
        use crate::game::sim_host::SimHost;
        assert_eq!(SimHost::game_time_factor(14), 1.2);
        assert_eq!(SimHost::game_time_factor(10), 1.0);
        let factor = SimHost::game_time_factor(14);
        let mut f = Flight::arrow(
            9,
            Vec3::ZERO,
            Vec3::new(700.0, 0.0, 0.0),
            1000.0,
            None,
            false,
        );
        let mut carry = 0.0;
        let mut clock = 0.0_f64;
        // Clock in frames of 16.7 ms: trajectory and simulation (`remaining -= game time`) in lockstep.
        let mut sim_remaining = 1000.0_f32;
        while f.elapsed_ms < f.t_ms {
            clock += 16.7;
            let dt = game_ms(&mut carry, 16.7, factor);
            sim_remaining -= dt;
            f.advance(dt);
        }
        assert!(sim_remaining.abs() < 2.0, "sim remainder {sim_remaining}");
        assert!((clock * 1.2 - 1000.0).abs() < 20.0, "clock {clock}");
        // Fractions are not lost; pause (dt = 0) advances nothing.
        let mut c = 0.0;
        let total: f32 = (0..100).map(|_| game_ms(&mut c, 10.0, factor)).sum();
        assert_eq!(total, 1200.0);
        assert_eq!(game_ms(&mut c, 0.0, factor), 0.0);
    }

    #[test]
    fn ghosts_fade_out_in_150_ms() {
        let mut f = Flight::arrow(
            4,
            Vec3::ZERO,
            Vec3::new(700.0, 0.0, 0.0),
            1000.0,
            Some("pfeil".into()),
            false,
        );
        f.advance(16.0);
        assert_eq!(f.ghosts.len(), 1);
        f.advance(100.0);
        assert!((f.ghosts[0].alpha() - (1.0 - 100.0 / GHOST_MS)).abs() < 1e-3);
        f.advance(60.0);
        assert!(f.ghosts.iter().all(|g| g.age_ms < GHOST_MS));
    }

    #[test]
    fn tracking_keeps_the_position_and_ends_at_the_moved_aim() {
        // C-108: new target point, x/z velocity recomputed, position continuous; not any more below 400 ms remaining.
        let (start, aim) = (Vec3::new(0.0, 10.0, 0.0), Vec3::new(700.0, 30.0, 0.0));
        let mut f = Flight::arrow(6, start, aim, 1000.0, None, false);
        f.advance(300.0);
        let before = f.pos;
        let moved = Vec3::new(700.0, 30.0, 140.0);
        f.track(moved);
        assert!((f.arrow_pos(f.elapsed_ms) - before).length() < 1e-3);
        let end = f.arrow_pos(1000.0);
        assert!(
            (end - Vec3::new(moved.x, aim.y, moved.z)).length() < 1e-2,
            "{end}"
        );
        f.advance(350.0);
        let late = f.pos;
        f.track(Vec3::new(0.0, 0.0, 999.0));
        assert_eq!(
            f.vel.z,
            (moved.z - before.z) / 700.0,
            "unchanged below 400 ms remaining"
        );
        assert_eq!(f.pos, late);
    }

    #[test]
    fn a_throw_flies_like_an_arrow_with_the_dynamite_model() {
        // re/headhunter-robbing 7, as in the original: trajectory and orientation of the arrow without fire,
        // the whole model `dynamit.rdo` instead of a mesh in the shooter model.
        let path = "data/r3d/misc/Tools/dynamit/dynamit.rdo";
        let (start, aim) = (Vec3::new(0.0, 10.0, 0.0), Vec3::new(700.0, 30.0, 0.0));
        let mut f = Flight::throw(11, start, aim, 1000.0, path);
        let arrow = Flight::arrow(11, start, aim, 1000.0, None, false);
        assert_eq!(f.kind, Kind::Throw);
        assert_eq!(f.mesh, None);
        assert_eq!(
            f.model,
            Some(MeshRef {
                path: nml_rda::normalize_path(path),
                node: Affine3A::IDENTITY,
            })
        );
        assert_eq!(
            (f.t_ms, f.pitch, f.yaw),
            (arrow.t_ms, arrow.pitch, arrow.yaw)
        );
        assert_eq!(f.arrow_pos(500.0), arrow.arrow_pos(500.0));
        // Duration = `flight_ms`: at the target point after 1000 ms, with ghost trail.
        f.advance(400.0);
        assert!((f.pos - arrow.arrow_pos(400.0)).length() < 1e-3);
        assert_eq!(f.ghosts.len(), 1);
        f.advance(600.0);
        assert_eq!(f.elapsed_ms, 1000.0);
        assert!((f.pos - aim).length() < 1e-3);
        // After loading: remaining time from the sim projectile.
        let r = Flight::resume_throw(11, start, aim, 1000, 300, path);
        assert_eq!((r.kind, r.elapsed_ms), (Kind::Throw, 700.0));
        assert_eq!(r.model, f.model);
        assert!((r.pos - r.arrow_pos(700.0)).length() < 1e-3);
    }

    #[test]
    fn rebuilding_after_load_resumes_at_the_elapsed_time() {
        let f = Flight::resume_arrow(
            5,
            Vec3::ZERO,
            Vec3::new(700.0, 0.0, 0.0),
            1000,
            400,
            None,
            false,
        );
        assert_eq!(f.elapsed_ms, 600.0);
        assert!((f.pos - f.arrow_pos(600.0)).length() < 1e-3);
    }

    /// From 2^28 ms on, `next_puff_ms += PUFF_MS` no longer changes the `f32` (spacing of the `f32` values there
    /// 32 ms); the smoke loop ends anyway and creates at most two puffs. The frame lasts 33 ms: 16 ms
    /// rounded `elapsed_ms + dt` back to `elapsed_ms` (even mantissa), and the loop would not run at all.
    #[test]
    fn puffs_end_even_for_huge_flight_times() {
        let (start, aim) = (Vec3::ZERO, Vec3::new(700.0, 0.0, 0.0));
        let mut f = Flight::ball(1, start, aim, (1u32 << 29) as f32, 0.0);
        let at = ((1u32 << 28) + 4096) as f32;
        (f.elapsed_ms, f.next_puff_ms) = (at, at);
        let out = f.advance(33.0);
        assert!(out.puffs.len() <= 2, "{} puffs", out.puffs.len());
    }

    /// Flight and remaining time from the savegame are unchecked; `flight time - remaining time` wraps as in the
    /// release build (x86) instead of overflowing in the debug build.
    #[test]
    fn resume_with_extreme_times_does_not_panic() {
        let (start, aim) = (Vec3::ZERO, Vec3::new(700.0, 0.0, 0.0));
        for (flight_ms, remaining_ms) in [(i32::MAX, -1), (i32::MIN, 1), (0, i32::MIN)] {
            let mut flights = [
                Flight::resume_arrow(1, start, aim, flight_ms, remaining_ms, None, false),
                Flight::resume_ball(2, start, aim, flight_ms, remaining_ms, 0.0),
                Flight::resume_throw(3, start, aim, flight_ms, remaining_ms, "dynamit.rdo"),
            ];
            for f in &mut flights {
                f.advance(16.0);
            }
        }
    }
}
