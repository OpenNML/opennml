//! "Handle Effect" (`.hef`) like `HandleEffect_Tick` (`0x004226F0`) and `HandleEffect_SpawnStep`
//! (`0x00421840`): circle arcs of `.pef` effects around a carrier, in time or count mode
//! (re/effects, section "HEF radius indicators").

use glam::Vec3;
use opennml_formats::hef::HefRecord;

use crate::particles::DEG_TO_RAD;

/// Upper limit for `COUNT` in count mode (effects per trigger in one frame): one effect per
/// degree of the full circle. The shipped data uses only 1 and 20 (16 records); the original has
/// no limit and, with a corrupted value, would create that many effects at once.
pub const MAX_COUNT: i32 = 360;

/// An effect to be placed: path, offset to the carrier, rotation (rad), lifetime (ms).
#[derive(Debug, Clone, PartialEq)]
pub struct HandleSpawn {
    pub effect: String,
    pub offset: Vec3,
    pub yaw: f32,
    pub lifetime_ms: i32,
}

/// Run-time counters of a record (`+0x10`, `+0x20`, `+0x24`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Counters {
    /// Count mode: timer (initially `TIMEOUT`).
    timer: i32,
    counter: i32,
    total: i32,
}

/// A `.hef` on a carrier.
#[derive(Debug, Clone, PartialEq)]
pub struct HandleEffect {
    pub path: String,
    records: Vec<HefRecord>,
    counters: Vec<Counters>,
    /// Base radius `floor(E/4) * SIZE_X` (adjustable in the viewer; `E` = carrier property `0x12`).
    pub radius: f32,
}

impl HandleEffect {
    pub fn new(path: &str, records: Vec<HefRecord>, radius: f32) -> Self {
        let counters = records
            .iter()
            .map(|r| Counters {
                timer: r.timeout,
                counter: 0,
                total: 0,
            })
            .collect();
        Self {
            path: path.to_string(),
            records,
            counters,
            radius,
        }
    }

    pub fn records(&self) -> &[HefRecord] {
        &self.records
    }

    /// One frame with `dt_ms`: counters advance, due effects go to `spawn` (height of the point from
    /// `height`, at least `floor`, relative to the carrier at `carrier`).
    pub fn tick(
        &mut self,
        dt_ms: i32,
        carrier: Vec3,
        height: &dyn Fn(f32, f32) -> f32,
        floor: f32,
        spawn: &mut dyn FnMut(HandleSpawn),
    ) {
        for (r, c) in self.records.iter().zip(&mut self.counters) {
            c.counter = c.counter.wrapping_add(dt_ms);
            c.total = c.total.wrapping_add(dt_ms);
            if !r.arc {
                continue;
            }
            let mut radius = if r.center { 0.0 } else { self.radius };
            if r.half_vision_range && !r.center {
                radius = radius * 2.0 / 3.0;
            }
            let n = if r.count_mode {
                r.duration_or_count.clamp(1, MAX_COUNT)
            } else {
                r.duration_or_count.max(1)
            };
            let step = (r.arc_to - r.arc_from) as f32 / n as f32;
            let mut put = |angle_deg: f32| {
                let a = angle_deg * DEG_TO_RAD;
                let x = carrier.x + a.cos() * radius;
                let z = carrier.z + a.sin() * radius;
                let y = height(x, z).max(floor);
                spawn(HandleSpawn {
                    effect: r.effect.clone(),
                    offset: Vec3::new(x, y, z) - carrier,
                    yaw: a,
                    lifetime_ms: r.effect_duration,
                });
            };
            if r.count_mode {
                if c.timer == 0 {
                    for i in 0..n {
                        put(r.arc_from as f32 + i as f32 * step);
                    }
                    c.timer = r.effect_duration;
                    c.counter = 0;
                } else if c.counter > c.timer {
                    c.timer = 0;
                }
            } else if c.counter >= r.timeout {
                c.counter = 0;
                put(r.arc_from as f32 + c.total.rem_euclid(n) as f32 * step);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> HefRecord {
        HefRecord {
            arc: true,
            arc_from: 0,
            arc_to: 180,
            duration_or_count: 2000,
            timeout: 10,
            effect: "data/effects/circle_effect_1.pef".into(),
            effect_duration: 2000,
            ..HefRecord::default()
        }
    }

    fn run(handle: &mut HandleEffect, steps: usize, dt: i32) -> Vec<HandleSpawn> {
        let mut out = Vec::new();
        for _ in 0..steps {
            handle.tick(
                dt,
                Vec3::new(100.0, 5.0, 100.0),
                &|_, _| 0.0,
                -10.0,
                &mut |s| out.push(s),
            );
        }
        out
    }

    #[test]
    fn time_mode_walks_the_arc_once_per_duration() {
        let mut handle = HandleEffect::new("a.hef", vec![record()], 300.0);
        // 20 ms per frame: one effect per frame (TIMEOUT 10).
        let spawns = run(&mut handle, 50, 20);
        assert_eq!(spawns.len(), 50);
        // Total time 20 ms -> angle 20 * 180/2000 = 1.8 degrees.
        let first = &spawns[0];
        assert!((first.yaw - 1.8 * DEG_TO_RAD).abs() < 1e-6);
        assert!((first.offset.x - 300.0 * first.yaw.cos()).abs() < 1e-3);
        // Height of the ground (0) minus carrier height.
        assert_eq!(first.offset.y, -5.0);
        assert_eq!(first.lifetime_ms, 2000);
    }

    #[test]
    fn count_mode_sets_a_ring_and_waits_for_the_effect_duration() {
        let mut ring = HefRecord {
            count_mode: true,
            center: false,
            half_vision_range: true,
            duration_or_count: 4,
            timeout: 0,
            arc_to: 360,
            effect_duration: 100,
            ..record()
        };
        ring.arc_from = 0;
        let mut handle = HandleEffect::new("b.hef", vec![ring], 300.0);
        let spawns = run(&mut handle, 1, 10);
        assert_eq!(spawns.len(), 4);
        // Radius 300 * 2/3, angles 0, 90, 180, 270 degrees.
        assert!((spawns[1].offset.z - 200.0).abs() < 1e-3);
        // Timer 100 ms: free after 110 ms, the new ring in the next frame.
        assert!(run(&mut handle, 10, 10).is_empty());
        assert!(run(&mut handle, 1, 10).is_empty());
        assert_eq!(run(&mut handle, 1, 10).len(), 4);
        // CENTERPOS: radius 0; without ARCRANGE nothing.
        let center = HefRecord {
            center: true,
            ..record()
        };
        let silent = HefRecord {
            arc: false,
            ..record()
        };
        let mut handle = HandleEffect::new("c.hef", vec![center, silent], 300.0);
        let spawns = run(&mut handle, 1, 20);
        assert_eq!(spawns.len(), 1);
        assert_eq!((spawns[0].offset.x, spawns[0].offset.z), (0.0, 0.0));
    }

    #[test]
    fn an_absurd_count_is_capped_at_one_effect_per_degree() {
        let ring = HefRecord {
            count_mode: true,
            duration_or_count: i32::MAX,
            timeout: 0,
            arc_to: 360,
            ..record()
        };
        let mut handle = HandleEffect::new("d.hef", vec![ring], 300.0);
        let spawns = run(&mut handle, 1, 10);
        assert_eq!(spawns.len(), MAX_COUNT as usize);
        // Evenly across the arc: step 1 degree.
        assert!((spawns[1].yaw - DEG_TO_RAD).abs() < 1e-6);
    }
}
