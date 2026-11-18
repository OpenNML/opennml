//! A movement step (`Motion_StepTowardsTarget` `0x00446310`, re/motion 4): set onto the
//! target, turn in place or turn while driving; positions in fixed point, angles in
//! degrees*4096, sine/cosine via the degree tables.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::speed::SpeedInfo;
use crate::fx::{fixed_hypot, fixed_mul};
use crate::math::{atan2_deg, cos_deg, sin_deg};
use crate::world::wrap_angle;

/// Terrain outside the map (`0x0061EA6C`).
pub const COST_OUTSIDE: i32 = 0x3FFF_FFFF;
/// Terrain blocked (`0x0061EA70`).
pub const COST_BLOCKED: i32 = 0x3FFF_FFFE;
/// From these costs on, units collide hard (`0x006719E8`).
pub const COST_COLLISION: i32 = 0x1000;

/// Movement state or trajectory point (VTable `0x00615AEC`, re/motion 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrajPoint {
    /// `+0x04`, `+0x08`: position (tile*4096).
    pub x: i32,
    pub z: i32,
    /// `+0x0C`: facing direction.
    pub dir: i32,
    /// `+0x10`: 0 arrived/start, 1 end of a B* section, 2 turns in place, 3 drives,
    /// 4 instant turn executed, 5 drives, facing direction catches up.
    pub mode: i32,
    /// `+0x14`: turn sense (0 shortest way, 1 negative, 2 positive).
    pub sense: i32,
    /// `+0x18`: stands in an obstacle (relaxes the check).
    pub inside: bool,
    /// `+0x1C`: speed of the point (only passed through by `step`).
    pub speed: i32,
}

/// What a step needs to know about the terrain and other units (the queries to other
/// units go through their modules, hence mutable).
pub trait StepWorld {
    /// `CollisionMgr_GetFootprintTerrainCost` at `(x, z)` for the unit.
    fn footprint_cost(&mut self, x: i32, z: i32) -> i32;
    /// `CollisionMgr_QueryCostAt(x, z, unit, layer, 0, friends, 0)`.
    fn query_cost(&mut self, x: i32, z: i32, friends: bool) -> i32;
}

/// `Motion_TurnTowards` (`0x00446110`): at most `max` in direction `diff`; turn sense 0 by the
/// sign, 2 positive, otherwise negative (with a forced turn sense the unit also turns away from the
/// target, quirk).
pub fn turn_towards(cur: i32, diff: i32, max: i32, sense: i32) -> i32 {
    let step = if diff.wrapping_abs() >= max {
        max
    } else {
        diff.wrapping_abs()
    };
    let positive = if sense == 0 { diff >= 0 } else { sense == 2 };
    wrap_angle(if positive {
        cur.wrapping_add(step)
    } else {
        cur.wrapping_sub(step)
    })
}

/// `Motion_IsTargetInTurnCircle` (`0x00446080`): target `(dx, dz)` relative to the unit in the circle with
/// radius `r` beside the direction of travel (side by the sign of `diff`, 0 -> -90 degrees).
pub fn in_turn_circle(dir: i32, diff: i32, dx: i32, dz: i32, r: i32) -> bool {
    let c = wrap_angle(if diff > 0 {
        dir.wrapping_add(90 << 12)
    } else {
        dir.wrapping_sub(90 << 12)
    });
    let ox = dx.wrapping_sub(fixed_mul(cos_deg(c), r));
    let oz = dz.wrapping_sub(fixed_mul(sin_deg(c), r));
    fixed_hypot(ox, oz) < r
}

/// `Motion_IsStepBlocked` (`0x00446170`): check kind 1 or in an obstacle -> only the terrain
/// footprint (outside always blocks, blocked only outside an obstacle, free clears the
/// obstacle flag); otherwise other units via `0x1000` (check kind 2 without friends).
fn is_step_blocked(check: i32, st: &mut TrajPoint, world: &mut impl StepWorld) -> bool {
    if check == 1 || st.inside {
        let c = world.footprint_cost(st.x, st.z);
        if c == COST_OUTSIDE || (c == COST_BLOCKED && !st.inside) {
            return true;
        }
        if c == 0 {
            st.inside = false;
        }
        return false;
    }
    world.query_cost(st.x, st.z, check == 2) > COST_COLLISION
}

/// `Motion_StepTowardsTarget(state, target, unit, layer, SpeedInfo, check kind)`: 0 = continue
/// (step made or turned), 1 = blocked (the blocked position is taken over,
/// quirk), 2 = arrived. The state is always written back completely.
pub fn step(
    st: &mut TrajPoint,
    target: (i32, i32),
    sp: &mut SpeedInfo,
    check: i32,
    world: &mut impl StepWorld,
) -> i32 {
    let start = *st;
    let mut l = *st;
    if l.mode == 4 {
        l.mode = 3;
    }
    let (dx, dz) = (target.0.wrapping_sub(l.x), target.1.wrapping_sub(l.z));
    let ang = if dx == 0 && dz == 0 {
        l.dir
    } else {
        atan2_deg(dz, dx)
    };
    let mut diff = wrap_angle(ang.wrapping_sub(l.dir));
    let spin;
    if l.mode == 2 {
        // Turn in place: no arrival check (quirk, overshoot).
        spin = true;
    } else {
        let r = sp.cur;
        if fixed_hypot(l.x.wrapping_sub(target.0), l.z.wrapping_sub(target.1)) <= r {
            l.x = target.0;
            l.z = target.1;
            l.mode = 0;
            // With the old turn sense (also turns the wrong way around after a turning maneuver).
            l.dir = turn_towards(l.dir, diff, sp.turn, l.sense);
            let ret = if is_step_blocked(check, &mut l, world) {
                l.mode = 0;
                l.inside = true;
                1
            } else {
                2
            };
            *st = l;
            return ret;
        }
        if l.mode == 0 || l.mode == 1 {
            l.mode = 3;
            l.sense = 0;
            if sp.min == 0 && (l.inside || diff.wrapping_abs() >= sp.spin_threshold) {
                l.mode = 2;
            } else if in_turn_circle(l.dir, diff, dx, dz, sp.turn_radius()) {
                if sp.min == 0 {
                    l.mode = 2;
                } else {
                    // Turn away from the target (large arc).
                    l.sense = if diff < 0 { 2 } else { 1 };
                }
            }
        }
        spin = l.mode == 2;
        if !spin && diff == 0 {
            l.sense = 0;
        }
    }
    if spin {
        if sp.instant_turn {
            l.dir = ang;
            diff = 0;
            l.mode = 4;
            l.sense = 0;
        } else if diff == 0 {
            l.mode = 3;
            l.sense = 0;
        } else {
            l.dir = turn_towards(l.dir, diff, sp.spin, l.sense);
            *st = l;
            return 0;
        }
    }
    // Driving (`0x004465C3`).
    let nd = turn_towards(l.dir, diff, sp.turn, l.sense);
    l.dir = nd;
    let mv = match l.mode {
        5 => {
            if nd == ang {
                l.mode = 3;
                nd
            } else {
                ang
            }
        }
        3 => nd,
        _ => ang,
    };
    let r = sp.cur;
    l.x = l.x.wrapping_add(fixed_mul(cos_deg(mv), r));
    l.z = l.z.wrapping_add(fixed_mul(sin_deg(mv), r));
    if !is_step_blocked(check, &mut l, world) {
        *st = l;
        return 0;
    }
    if diff != 0 && l.mode != 5 {
        if sp.instant_turn {
            // Straight at the target from the old point.
            l.x = start.x.wrapping_add(fixed_mul(cos_deg(ang), r));
            l.z = start.z.wrapping_add(fixed_mul(sin_deg(ang), r));
            l.mode = 4;
            l.dir = ang;
            if !is_step_blocked(check, &mut l, world) {
                *st = l;
                return 0;
            }
        } else if sp.min == 0 {
            // Back up and turn in place.
            l.x = start.x;
            l.z = start.z;
            l.mode = 2;
            l.dir = turn_towards(start.dir, diff, sp.spin, l.sense);
            *st = l;
            return 0;
        }
    }
    l.mode = 0;
    l.inside = true;
    *st = l;
    1
}

/// `Traj_SpreadTurnInPlace(list, node, turn)` (`0x00446200`, re/motion 13): distribute an
/// instant turn (state 4) of the point `node` backwards onto the previous points (at most
/// `turn` per point, the head stays); if that does not reach the head, redistribute forwards from the head
/// (also beyond the pivot). The pivot gets state 3 (accommodated)
/// or 5 (turns afterwards). `false` for `None` or the head.
pub fn spread_turn_in_place(
    list: &mut VecDeque<TrajPoint>,
    node: Option<usize>,
    turn: i32,
) -> bool {
    let Some(n) = node else { return false };
    if n == 0 || n >= list.len() {
        return false;
    }
    if list[n].mode != 4 {
        return true;
    }
    let mut d = list[n].dir;
    for q in (1..n).rev() {
        d = turn_towards(d, wrap_angle(list[q].dir.wrapping_sub(d)), turn, 0);
        if d == list[q].dir {
            list[n].mode = 3;
            return true;
        }
        list[q].dir = d;
    }
    if list.len() > 1 {
        let target = list[n].dir;
        for c in 1..list.len() {
            let prev = list[c - 1].dir;
            let v = turn_towards(prev, wrap_angle(target.wrapping_sub(prev)), turn, 0);
            list[c].dir = v;
            if v == target {
                list[n].mode = 3;
                return true;
            }
        }
    }
    list[n].mode = 5;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i32 = 4096;

    /// Terrain by a rule, no units.
    struct Terrain(fn(i32, i32) -> i32);

    impl StepWorld for Terrain {
        fn footprint_cost(&mut self, x: i32, z: i32) -> i32 {
            (self.0)(x, z)
        }
        fn query_cost(&mut self, _: i32, _: i32, _: bool) -> i32 {
            0
        }
    }

    fn speeds(kind: &str) -> SpeedInfo {
        let mut s = match kind {
            "man" => SpeedInfo::for_unit(200, 1, 183, 512),
            "cart" => SpeedInfo::for_unit(100, 0, 183, 1900),
            _ => SpeedInfo::for_unit(7, 1, 128, 2100),
        };
        s.reset_to_cruising();
        s
    }

    #[test]
    fn steps_match_the_vectors_of_the_disassembly_model() {
        let free: fn(i32, i32) -> i32 = |_, _| 0;
        let block_x: fn(i32, i32) -> i32 = |x, _| if x > 10 * T + 100 { COST_BLOCKED } else { 0 };
        let block_z: fn(i32, i32) -> i32 = |_, z| if z > 10 * T + 10 { COST_BLOCKED } else { 0 };
        // (unit, start (x, z, dir, mode, sense, inside), target, terrain) ->
        // (return value, x, z, dir, mode, sense, inside); re/motion 4.3.
        type Start = (i32, i32, i32, i32, i32, bool);
        type Want = (i32, i32, i32, i32, i32, i32, bool);
        type Case = (&'static str, Start, (i32, i32), fn(i32, i32) -> i32, Want);
        let cases: [Case; 17] = [
            (
                "man",
                (10 * T, 10 * T, 0, 3, 0, false),
                (20 * T, 10 * T),
                free,
                (0, 41544, 40960, 0, 3, 0, false),
            ),
            (
                "man",
                (10 * T, 10 * T, 0, 3, 0, false),
                (10 * T + 500, 10 * T + 100),
                free,
                (2, 41460, 41060, 46314, 0, 0, false),
            ),
            (
                "man",
                (10 * T, 10 * T, 0, 0, 0, false),
                (10 * T, 20 * T),
                free,
                (0, 40960, 41544, 368_640, 4, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 3, 0, false),
                (18 * T, 14 * T + 2731),
                free,
                (0, 41465, 41251, 123_873, 3, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 0, 0, false),
                (9 * T, 10 * T),
                free,
                (0, 41455, 40650, -131_072, 3, 0, false),
            ),
            (
                "ship",
                (10 * T, 10 * T, 0, 0, 0, false),
                (9 * T, 10 * T),
                free,
                (0, 40960, 40960, -18344, 2, 0, false),
            ),
            (
                "ship",
                (10 * T, 10 * T, -57344, 2, 0, false),
                (9 * T, 10 * T),
                free,
                (0, 40960, 40960, -75688, 2, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 0, 0, false),
                (10 * T + 200, 10 * T + 1200),
                free,
                (0, 41455, 40650, -131_072, 3, 1, false),
            ),
            (
                "man",
                (5 * T, 5 * T, 1234, 3, 0, false),
                (5 * T, 5 * T),
                free,
                (2, 20480, 20480, 1234, 0, 0, false),
            ),
            (
                "man",
                (10 * T, 10 * T, 0, 1, 0, false),
                (5 * T, 10 * T),
                free,
                (0, 40376, 40960, -737_280, 4, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 5, 0, false),
                (10 * T, 20 * T),
                free,
                (0, 40960, 41544, 131_072, 5, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 3, 0, false),
                (20 * T, 10 * T),
                block_x,
                (1, 41544, 40960, 0, 0, 0, true),
            ),
            (
                "ship",
                (10 * T, 10 * T, 0, 3, 0, false),
                (20 * T, 12 * T),
                block_z,
                (0, 40960, 40960, 18344, 2, 0, false),
            ),
            (
                "man",
                (10 * T, 10 * T, 0, 0, 0, false),
                (14 * T, 12 * T + 1500),
                free,
                (0, 41465, 41251, 125_237, 3, 0, false),
            ),
            (
                "ship",
                (10 * T, 10 * T, 0, 0, 0, false),
                (30 * T, 30 * T),
                free,
                (0, 41367, 40974, 9172, 3, 0, false),
            ),
            (
                "ship",
                (10 * T, 10 * T, 0, 2, 0, false),
                (10 * T + 100, 10 * T),
                free,
                (0, 41368, 40960, 0, 3, 0, false),
            ),
            (
                "cart",
                (10 * T, 10 * T, 0, 3, 1, false),
                (10 * T + 300, 10 * T + 300),
                free,
                (2, 41260, 41260, -131_072, 0, 1, false),
            ),
        ];
        for (i, (kind, s, target, terrain, want)) in cases.into_iter().enumerate() {
            let mut sp = speeds(kind);
            let mut st = TrajPoint {
                x: s.0,
                z: s.1,
                dir: s.2,
                mode: s.3,
                sense: s.4,
                inside: s.5,
                speed: 77,
            };
            let r = step(&mut st, target, &mut sp, 1, &mut Terrain(terrain));
            let got = (r, st.x, st.z, st.dir, st.mode, st.sense, st.inside);
            assert_eq!(got, want, "vector {:02}", i + 1);
            assert_eq!(st.speed, 77);
        }
    }

    #[test]
    fn instant_turns_are_spread_over_the_previous_points() {
        // (dir, mode) head ... end, pivot, turn -> result (re/motion 13, S1 to S8).
        let list = |v: &[(i32, i32)]| -> VecDeque<TrajPoint> {
            v.iter()
                .map(|&(dir, mode)| TrajPoint {
                    dir,
                    mode,
                    ..TrajPoint::default()
                })
                .collect()
        };
        let dirs = |l: &VecDeque<TrajPoint>| l.iter().map(|p| (p.dir, p.mode)).collect::<Vec<_>>();
        let man = 262_144;
        for (before, node, turn, after) in [
            (
                vec![(0, 0), (0, 3), (368_640, 4)],
                2,
                man,
                vec![(0, 0), (262_144, 3), (368_640, 3)],
            ),
            (
                vec![(0, 0), (368_640, 4)],
                1,
                man,
                vec![(0, 0), (262_144, 5)],
            ),
            (
                vec![(0, 0), (245_760, 3), (327_680, 3), (368_640, 4)],
                3,
                man,
                vec![(0, 0), (245_760, 3), (327_680, 3), (368_640, 3)],
            ),
            (
                vec![(0, 0), (-40960, 3), (0, 3), (0, 3), (614_400, 4)],
                4,
                man,
                vec![(0, 0), (-40960, 3), (90112, 3), (352_256, 3), (614_400, 3)],
            ),
            (
                vec![(0, 0), (0, 3), (-737_280, 4)],
                2,
                9172,
                vec![(0, 0), (-9172, 3), (-18344, 5)],
            ),
            (
                vec![(0, 0), (368_640, 4), (368_640, 3), (368_640, 3)],
                1,
                man,
                vec![(0, 0), (262_144, 3), (368_640, 3), (368_640, 3)],
            ),
        ] {
            let mut l = list(&before);
            assert!(spread_turn_in_place(&mut l, Some(node), turn));
            assert_eq!(dirs(&l), after, "{before:?}");
        }
        // S8: loop like the B* postprocessing, the second turn overwrites the first.
        let mut l = list(&[
            (0, 0),
            (368_640, 4),
            (368_640, 3),
            (-368_640, 4),
            (-368_640, 3),
        ]);
        for i in 1..l.len() {
            spread_turn_in_place(&mut l, Some(i), man);
        }
        assert_eq!(
            dirs(&l),
            [
                (0, 0),
                (-262_144, 3),
                (-368_640, 3),
                (-368_640, 3),
                (-368_640, 3)
            ]
        );
        let mut l = list(&[(0, 0), (5, 4)]);
        assert!(
            !spread_turn_in_place(&mut l, Some(0), man) && !spread_turn_in_place(&mut l, None, man)
        );
    }

    #[test]
    fn turning_helpers_clip_and_pick_the_circle_side() {
        assert_eq!(turn_towards(0, 100, 50, 0), 50);
        assert_eq!(turn_towards(0, 30, 50, 0), 30);
        assert_eq!(turn_towards(0, 30, 50, 1), -30);
        assert_eq!(turn_towards(0, -30, 50, 2), 30);
        // -180 degrees = +180 degrees: turn sense 0 turns negative.
        assert_eq!(
            turn_towards(-737_280 + 10, -737_280, 4096, 0),
            wrap_angle(-737_280 + 10 - 4096)
        );
        // Circle left (diff > 0) or right (diff <= 0) of the direction of travel +x, radius 1044.
        assert!(in_turn_circle(0, 1, 200, 1200, 1044));
        assert!(!in_turn_circle(0, -1, 200, 1200, 1044));
        assert!(!in_turn_circle(0, 1, 200, 1200, 0));
    }

    /// World with a fixed answer; remembers which query came (and whether with friends).
    struct Fixed {
        footprint: i32,
        units: i32,
        asked: Vec<&'static str>,
        friends: Option<bool>,
    }

    impl StepWorld for Fixed {
        fn footprint_cost(&mut self, _: i32, _: i32) -> i32 {
            self.asked.push("terrain");
            self.footprint
        }
        fn query_cost(&mut self, _: i32, _: i32, friends: bool) -> i32 {
            self.asked.push("units");
            self.friends = Some(friends);
            self.units
        }
    }

    fn blocked(check: i32, inside: bool, footprint: i32, units: i32) -> (bool, bool, Fixed) {
        let mut w = Fixed {
            footprint,
            units,
            asked: Vec::new(),
            friends: None,
        };
        let mut st = TrajPoint {
            inside,
            ..TrajPoint::default()
        };
        let r = is_step_blocked(check, &mut st, &mut w);
        (r, st.inside, w)
    }

    #[test]
    fn step_blocking_follows_the_check_kind_and_the_obstacle_flag() {
        // Check kinds 0 and 2 query other units: blocked only above 0x1000, check kind 2 without friends.
        for (check, friends) in [(0, false), (2, true)] {
            let (r, _, w) = blocked(check, false, COST_BLOCKED, COST_COLLISION);
            assert!(!r, "check kind {check}: exactly 0x1000 does not block yet");
            assert_eq!(
                (w.asked.as_slice(), w.friends),
                (&["units"][..], Some(friends))
            );
            assert!(blocked(check, false, 0, COST_COLLISION + 1).0);
        }
        // Check kind 1: only the terrain footprint; outside always blocks, blocked only outside
        // an obstacle.
        let (r, _, w) = blocked(1, false, 0, COST_BLOCKED);
        assert!(!r && w.asked == ["terrain"]);
        assert!(blocked(1, false, COST_OUTSIDE, 0).0);
        assert!(blocked(1, true, COST_OUTSIDE, 0).0);
        assert!(blocked(1, false, COST_BLOCKED, 0).0);
        // In an obstacle (flag): check kinds 0/2 also query the terrain; blocked terrain does not block,
        // the flag stays; only costs 0 clear it (re/motion 4.1).
        for check in [0, 1, 2] {
            let (r, inside, w) = blocked(check, true, COST_BLOCKED, COST_BLOCKED);
            assert!(!r && inside && w.asked == ["terrain"], "check kind {check}");
            let (r, inside, _) = blocked(check, true, 5, 0);
            assert!(!r && inside, "check kind {check}: costs != 0 keep the flag");
            let (r, inside, _) = blocked(check, true, 0, COST_BLOCKED);
            assert!(!r && !inside, "check kind {check}: costs 0 clear the flag");
        }
        // Outside the obstacle the flag stays off even with costs 0 (nothing to clear).
        assert!(!blocked(1, false, 0, 0).1);
    }
}
