//! Place arrangement of the formations (vf48, re/groups-formations 3.3, 12.1) and helpers of the march
//! (12.3). Pure fixed-point calculation in the order of the original; all products `Fixed_Mul`,
//! start offset unrotated (quirk K-211).

use super::MfcMap;
use crate::fx::fixed_mul as mul;
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::world::wrap_angle;

pub const DEG90: i32 = 90 << 12;
/// `SqrtTab[2] / 2` (`[0x0066FD8C]`).
pub const SQRT2_HALF: i32 = 2896;
/// Center gap of the pincer q (`[0x006723CC]`).
pub const PINCER_GAP: i32 = 0x800;

/// `0x00401470`: `b + R(a)·v`.
pub fn rot(b: (i32, i32), v: (i32, i32), a: i32) -> (i32, i32) {
    let (c, s) = (cos_deg(a), sin_deg(a));
    (
        mul(c, v.0).wrapping_add(b.0.wrapping_sub(mul(s, v.1))),
        mul(c, v.1).wrapping_add(b.1).wrapping_add(mul(s, v.0)),
    )
}

pub struct Layout<'a> {
    pub kind: u8,
    pub ranks: &'a [Vec<ObjectId>; 10],
    pub n: usize,
    pub s: i32,
    pub sp: i32,
    pub anchor: (i32, i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ends {
    Keep,
    Set { max: ObjectId, min: ObjectId },
    FindByOffsets,
    FindByRanks,
}

/// One place: member, local place (lx, lz) unscaled, world target (x, z, direction).
pub type Place = (ObjectId, (i32, i32), (i32, i32, i32));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub places: Vec<Place>,
    pub depth: Option<i32>,
    pub corner_margin: Option<i32>,
    pub ends: Ends,
    pub ok: bool,
}

fn empty(ok: bool) -> Placement {
    Placement {
        places: Vec::new(),
        depth: None,
        corner_margin: None,
        ends: Ends::Keep,
        ok,
    }
}

/// Members in rank order, within the rank in command order.
fn in_ranks(ranks: &[Vec<ObjectId>; 10], order: impl Iterator<Item = usize>) -> Vec<ObjectId> {
    order.flat_map(|k| ranks[k].iter().copied()).collect()
}

/// vf44 (`0x0040EFF0`).
fn depth(a: i32, b: i32) -> i32 {
    b.wrapping_add(1).wrapping_mul(a).wrapping_mul(2)
}

pub fn arrange(l: &Layout) -> Placement {
    match l.kind {
        1 => line(l),
        2 => square(l),
        3 => column(l),
        4 => pincer(l),
        _ => empty(false), // Type 6 (`0x00532A90`) and unknown: 0
    }
}

/// Type 1 line (`0x005475F0`).
fn line(l: &Layout) -> Placement {
    let n = l.n as i32;
    let d = l.s / 2;
    let rows = if n > 2 { 2 } else { 1 };
    let per = if rows == 2 { (n + 1) / 2 } else { n };
    let half = d.wrapping_mul(per).wrapping_neg();
    let start = (
        l.anchor.0.wrapping_add(mul(0, l.sp)),
        l.anchor.1.wrapping_add(mul(half, l.sp)),
    );
    let a = wrap_angle(DEG90.wrapping_add(l.anchor.2));
    let w = mul(d, l.sp);
    let (dx, dz) = (mul(cos_deg(a), w), mul(sin_deg(a), w));
    let members = in_ranks(l.ranks, (0..10).rev());
    let mut out = Placement {
        places: Vec::new(),
        depth: Some(depth(d, rows)),
        corner_margin: Some(mul(d.wrapping_mul(per), SQRT2_HALF)),
        ends: Ends::Keep,
        ok: true,
    };
    let Some(&first) = members.first() else {
        return out;
    };
    let mut it = members.into_iter();
    let (mut cur, mut lx, mut lz) = (start, 0, half);
    let mut last = first;
    for _ in 0..rows {
        for _ in 0..per {
            let Some(m) = it.next() else { break };
            cur = (cur.0.wrapping_add(dx), cur.1.wrapping_add(dz));
            lz = lz.wrapping_add(d);
            out.places.push((m, (lx, lz), (cur.0, cur.1, l.anchor.2)));
            cur = (cur.0.wrapping_add(dx), cur.1.wrapping_add(dz));
            lz = lz.wrapping_add(d);
            last = m;
        }
        cur = (
            start.0.wrapping_sub(dz.wrapping_mul(2)),
            start.1.wrapping_add(dx.wrapping_mul(2)),
        );
        lz = half;
        if n % 2 == 1 {
            lz = lz.wrapping_add(d);
            cur = (cur.0.wrapping_add(dx), cur.1.wrapping_add(dz));
        }
        lx = d.wrapping_mul(-2);
    }
    out.ends = Ends::Set {
        max: last,
        min: first,
    };
    out
}

/// Type 2 square (`0x00554540`, re/groups-formations 12.1): axis-parallel spiral, never rotated.
fn square(l: &Layout) -> Placement {
    let s = l.s;
    let w = mul(s, l.sp);
    let dir = l.anchor.2;
    let members = in_ranks(l.ranks, 0..10);
    let mut out = empty(true);
    let Some((&first, rest)) = members.split_first() else {
        return out;
    };
    let (mut wx, mut wz) = (l.anchor.0, l.anchor.1);
    let (mut lx, mut lz) = (0i32, 0i32);
    out.places.push((first, (0, 0), (wx, wz, dir)));
    let (mut side, mut r, mut cnt) = (1, 3, 0);
    for &m in rest {
        cnt += 1;
        if cnt == side {
            if r == 3 {
                side += 2;
                (wx, wz) = (wx.wrapping_sub(w), wz.wrapping_add(w));
                (lx, lz) = (lx.wrapping_sub(s), lz.wrapping_add(s));
                r = 0;
            } else {
                r += 1;
            }
            cnt = 1;
        }
        out.places.push((m, (lx, lz), (wx, wz, dir)));
        match r {
            0 => (wx, lx) = (wx.wrapping_add(w), lx.wrapping_add(s)),
            1 => (wz, lz) = (wz.wrapping_sub(w), lz.wrapping_sub(s)),
            2 => (wx, lx) = (wx.wrapping_sub(w), lx.wrapping_sub(s)),
            _ => (wz, lz) = (wz.wrapping_add(w), lz.wrapping_add(s)),
        }
    }
    let h = s / 2;
    out.corner_margin = Some(mul(h.wrapping_mul(side), SQRT2_HALF));
    out.depth = Some(depth(h, side));
    out
}

/// Type 3 column (`0x0040F240`): rows of two, no vf44, ends by ranks.
fn column(l: &Layout) -> Placement {
    let s = l.s;
    let h = s / 2;
    let a = wrap_angle(l.anchor.2.wrapping_sub(DEG90));
    let w = mul(s, l.sp);
    let (dx, dz) = (mul(cos_deg(a), w), mul(sin_deg(a), w));
    let (mut wx, mut wz) = (
        l.anchor.0.wrapping_add(mul(0, l.sp)),
        l.anchor.1.wrapping_add(mul(h, l.sp)),
    );
    let (mut lx, mut lz) = (0, h);
    let rows = l.n.div_ceil(2);
    let mut it = in_ranks(l.ranks, (0..10).rev()).into_iter();
    let mut out = empty(true);
    out.corner_margin = Some(mul(h.wrapping_mul(2), SQRT2_HALF));
    for _ in 0..rows {
        for _ in 0..2 {
            let Some(m) = it.next() else { break };
            out.places.push((m, (lx, lz), (wx, wz, l.anchor.2)));
            (wx, wz) = (wx.wrapping_add(dx), wz.wrapping_add(dz));
            lz = lz.wrapping_sub(s);
        }
        wx = wx.wrapping_add(dz.wrapping_sub(dx.wrapping_mul(2)));
        wz = wz.wrapping_add(dz.wrapping_mul(-2).wrapping_sub(dx));
        lz = lz.wrapping_add(s.wrapping_mul(2));
        lx = lx.wrapping_sub(s);
    }
    out.ends = Ends::FindByRanks;
    out
}

/// Type 4 pincer (`0x004FA480`, re/groups-formations 12.1): steps carried forward per pair.
fn pincer(l: &Layout) -> Placement {
    let s = l.s;
    let h = s / 2;
    let n = l.n as i32;
    let rows = (n + 5) / 6;
    let a = wrap_angle(DEG90.wrapping_add(l.anchor.2));
    let (a1, a2) = (mul(cos_deg(a), l.sp), mul(sin_deg(a), l.sp));
    let (p1, p2) = (mul(s, a1), mul(s, a2));
    let q = PINCER_GAP;
    let qh = q / 2;
    let mut out = empty(true);
    out.depth = Some(depth(h, rows));
    out.corner_margin = Some(mul(
        qh.wrapping_add(s.wrapping_mul(3))
            .wrapping_add(rows.wrapping_sub(1).wrapping_mul(s).wrapping_mul(2)),
        SQRT2_HALF,
    ));
    let (mut lx, mut lz) = (0, qh);
    let (mut wx, mut wz) = (
        l.anchor.0.wrapping_add(mul(0, l.sp)),
        l.anchor.1.wrapping_add(mul(qh, l.sp)),
    );
    let mut it = in_ranks(l.ranks, 1..10).into_iter();
    let mut t = s;
    for _ in 0..rows {
        let mut e = t.wrapping_add(q);
        let (mut dx, mut dz) = (mul(a1, e).wrapping_neg(), mul(a2, e).wrapping_neg());
        for _ in 0..3 {
            for _ in 0..2 {
                let Some(m) = it.next() else { break };
                out.places.push((m, (lx, lz), (wx, wz, l.anchor.2)));
                (wx, wz) = (wx.wrapping_add(dx), wz.wrapping_add(dz));
                lz = lz.wrapping_sub(e);
            }
            lz = lz.wrapping_add(h.wrapping_add(e).wrapping_mul(2));
            wx = wx.wrapping_add(p1.wrapping_sub(dx.wrapping_mul(2)));
            wz = wz.wrapping_add(p2.wrapping_sub(dz.wrapping_mul(2)));
            e = e.wrapping_add(s.wrapping_mul(2));
            dx = dx.wrapping_sub(p1.wrapping_mul(2));
            dz = dz.wrapping_sub(p2.wrapping_mul(2));
        }
        lz = lz.wrapping_add(s.wrapping_sub(s.wrapping_mul(3)));
        lx = lx.wrapping_add(s);
        wx = wx.wrapping_add(p2.wrapping_sub(p1.wrapping_mul(2)));
        wz = wz.wrapping_add(p1.wrapping_neg().wrapping_sub(p2.wrapping_mul(2)));
        t = t.wrapping_add(s.wrapping_mul(2));
    }
    out.ends = Ends::FindByOffsets;
    out
}

/// `0x00407210`: first member with the largest or smallest lz (strict), CMap iteration.
pub fn find_ends(offsets: &MfcMap<(i32, i32)>) -> Option<(ObjectId, ObjectId)> {
    let (mut max, mut min) = (None, None);
    let (mut hi, mut lo) = (i32::MIN, i32::MAX);
    for (k, &(_, lz)) in offsets.iter() {
        if hi < lz {
            hi = lz;
            max = Some(ObjectId(k));
        }
        if lz < lo {
            lo = lz;
            min = Some(ObjectId(k));
        }
    }
    Some((max?, min?))
}

/// `0x0040F090` (column): front pair. Searches the ranks 9...0 for the first occupied rank;
/// `end_max = end_min` = its first member (`0x0040F0BE`/`0x0040F0C4`). If the rank has a second member,
/// `end_min` becomes that; otherwise the first member of the next occupied lower rank (loop down to rank 0). If
/// none is found, `end_min = end_max` stays (not -1). Only with no members at all are both -1 (`0x0040F101`).
pub fn column_ends(ranks: &[Vec<ObjectId>; 10]) -> (ObjectId, ObjectId) {
    let mut it = in_ranks(ranks, (0..10).rev()).into_iter();
    let Some(max) = it.next() else {
        return (ObjectId::NONE, ObjectId::NONE);
    };
    (max, it.next().unwrap_or(max))
}

/// vf4C mirror (`0x004073C0`): negate lz (in place); the caller swaps the ends `end_max`/`end_min`.
/// Applies only to line, column and pincer (type 1/3/4); square and type 6 have `0x004EC990` there (nothing,
/// re/groups-formations 1.5 table `+0x4C`). The caller (`add_turn_frames`) must therefore make the type
/// distinction and call `mirror` only for type 1/3/4.
pub fn mirror(offsets: &mut MfcMap<(i32, i32)>) {
    for o in offsets.values_mut() {
        o.1 = o.1.wrapping_neg();
    }
}

/// vf3C pivot: line/column/pincer around the inner wing end (`0x004070E0`), square/type 6 the
/// center (`0x00554510`).
pub fn pivot(
    kind: u8,
    offsets: &MfcMap<(i32, i32)>,
    (end_max, end_min): (ObjectId, ObjectId),
    sp: i32,
    p: (i32, i32, i32),
    d: i32,
) -> (i32, i32) {
    if !matches!(kind, 1 | 3 | 4) {
        return (p.0, p.1);
    }
    let end = if d < 0 { end_min } else { end_max };
    let o = offsets.get(end.0).copied().unwrap_or((0, 0));
    let w = rot((p.0, p.1), (mul(o.0, sp), mul(o.1, sp)), p.2);
    rot(w, (p.0.wrapping_sub(w.0), p.1.wrapping_sub(w.1)), d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u32) -> Vec<ObjectId> {
        (1..=n).map(ObjectId).collect()
    }

    /// All members in one rank (infantry R = 512 -> s = 1024), sp = 0x2000, anchor (0, 0).
    fn place(kind: u8, n: u32, dir_deg: i32) -> Placement {
        let mut ranks: [Vec<ObjectId>; 10] = Default::default();
        ranks[4] = ids(n);
        arrange(&Layout {
            kind,
            ranks: &ranks,
            n: n as usize,
            s: 1024,
            sp: 0x2000,
            anchor: (0, 0, dir_deg << 12),
        })
    }

    fn xz(p: &Placement) -> Vec<(i32, i32)> {
        p.places.iter().map(|e| (e.2.0, e.2.1)).collect()
    }

    #[test]
    fn line_matches_the_vectors() {
        // re/groups-formations 9 "Line".
        let p = place(1, 5, 0);
        assert_eq!(
            xz(&p),
            vec![(0, -2048), (0, 0), (0, 2048), (-2048, -1024), (-2048, 1024)]
        );
        assert_eq!((p.depth, p.ok), (Some(3072), true));
        assert_eq!(p.places[0].1, (0, -1024));
        assert_eq!(p.places[3].1, (-1024, -512));
        let p = place(1, 9, 0);
        assert_eq!(
            xz(&p),
            vec![
                (0, -4096),
                (0, -2048),
                (0, 0),
                (0, 2048),
                (0, 4096),
                (-2048, -3072),
                (-2048, -1024),
                (-2048, 1024),
                (-2048, 3072)
            ]
        );
        let p = place(1, 12, 0);
        let zs: Vec<i32> = (-5..=5).step_by(2).map(|k| k * 1024).collect();
        assert_eq!(xz(&p)[..6].iter().map(|e| e.1).collect::<Vec<_>>(), zs);
        assert!(xz(&p)[6..].iter().all(|e| e.0 == -2048));
        // Quirk K-211: start offset unrotated.
        let p = place(1, 5, 90);
        assert_eq!(
            xz(&p),
            vec![
                (-1024, -3072),
                (-3072, -3072),
                (-5120, -3072),
                (-2048, -5120),
                (-4096, -5120)
            ]
        );
        let p = place(1, 9, 90);
        assert_eq!(
            xz(&p)[..5],
            [
                (-1024, -5120),
                (-3072, -5120),
                (-5120, -5120),
                (-7168, -5120),
                (-9216, -5120)
            ]
        );
        assert_eq!(
            xz(&p)[5..],
            [
                (-2048, -7168),
                (-4096, -7168),
                (-6144, -7168),
                (-8192, -7168)
            ]
        );
        // Wing ends: first and last member.
        assert!(
            matches!(place(1, 5, 0).ends, Ends::Set { max, min } if max == ObjectId(5) && min == ObjectId(1))
        );
    }

    #[test]
    fn square_spirals_clockwise_and_never_turns() {
        let five = [(0, 0), (-2048, 2048), (0, 2048), (2048, 2048), (2048, 0)];
        for dir in [0, 90, 225] {
            let p = place(2, 5, dir);
            assert_eq!(xz(&p), five, "direction {dir}");
            assert!(p.places.iter().all(|e| e.2.2 == dir << 12));
            assert_eq!(p.depth, Some(4096));
        }
        let p = place(2, 9, 0);
        assert_eq!(
            xz(&p)[5..],
            [(2048, -2048), (0, -2048), (-2048, -2048), (-2048, 0)]
        );
        assert_eq!(p.depth, Some(4096));
        let p = place(2, 12, 0);
        assert_eq!(xz(&p)[9..], [(-4096, 4096), (-2048, 4096), (0, 4096)]);
        assert_eq!(p.depth, Some(6144));
        // Local: axes = world axes, unscaled (step s).
        assert_eq!(p.places[1].1, (-1024, 1024));
    }

    #[test]
    fn column_pairs_up_behind_the_click() {
        let p = place(3, 5, 0);
        assert_eq!(
            xz(&p),
            vec![
                (0, 1024),
                (0, -1024),
                (-2048, 1024),
                (-2048, -1024),
                (-4096, 1024)
            ]
        );
        assert_eq!(p.depth, None, "column does not call vf44 (K-214)");
        let p = place(3, 9, 0);
        for k in 0..4 {
            assert_eq!(
                xz(&p)[2 * k..2 * k + 2],
                [(-2048 * k as i32, 1024), (-2048 * k as i32, -1024)]
            );
        }
        assert_eq!(xz(&p)[8], (-8192, 1024));
        let p = place(3, 5, 90);
        assert_eq!(
            xz(&p),
            vec![
                (0, 1024),
                (2048, 1024),
                (0, -1024),
                (2048, -1024),
                (0, -3072)
            ]
        );
        assert!(matches!(p.ends, Ends::FindByRanks));
    }

    #[test]
    fn pincer_opens_two_wings_with_an_offset_gap() {
        // K-217: center gap q + s, offset by -s/2.
        let p = place(4, 5, 0);
        assert_eq!(
            xz(&p),
            vec![(0, 2048), (0, -4096), (0, 4096), (0, -6144), (0, 6144)]
        );
        assert_eq!(p.depth, Some(2048));
        let p = place(4, 9, 0);
        assert_eq!(
            xz(&p)[5..],
            [(0, -8192), (2048, 4096), (2048, -6144), (2048, 6144)]
        );
        assert_eq!(p.depth, Some(3072));
        let p = place(4, 12, 0);
        // re/groups-formations 3.3: row end `lz += s − 3s` (net -2s); the second row starts at lz = 2048.
        assert_eq!(p.places[6].1, (1024, 2048));
        assert_eq!(p.places[11].1, (1024, -5120));
        let row = |x: i32, r: [i32; 3], l: [i32; 3]| {
            vec![
                (x, r[0]),
                (x, l[0]),
                (x, r[1]),
                (x, l[1]),
                (x, r[2]),
                (x, l[2]),
            ]
        };
        assert_eq!(
            xz(&p)[..6],
            row(0, [2048, 4096, 6144], [-4096, -6144, -8192])[..]
        );
        assert_eq!(
            xz(&p)[6..],
            row(2048, [4096, 6144, 8192], [-6144, -8192, -10240])[..]
        );
        let p = place(4, 5, 90);
        assert_eq!(
            xz(&p),
            vec![
                (0, 2048),
                (6144, 2048),
                (-2048, 2048),
                (8192, 2048),
                (-4096, 2048)
            ]
        );
        assert!(matches!(p.ends, Ends::FindByOffsets));
    }

    #[test]
    fn ranks_decide_the_order_and_the_largest_radius_the_scale() {
        // re/groups-formations 9: 4 infantrymen (rank 4) + 2 pikemen (rank 5) in line -> pikemen first.
        let mut ranks: [Vec<ObjectId>; 10] = Default::default();
        ranks[4] = ids(4);
        ranks[5] = vec![ObjectId(10), ObjectId(11)];
        let p = arrange(&Layout {
            kind: 1,
            ranks: &ranks,
            n: 6,
            s: 1024,
            sp: 0x2000,
            anchor: (0, 0, 0),
        });
        let order: Vec<_> = p.places.iter().map(|e| e.0).collect();
        assert_eq!(order[..2], [ObjectId(10), ObjectId(11)]);
        // Square and pincer ascending: infantry first.
        let p = arrange(&Layout {
            kind: 2,
            ranks: &ranks,
            n: 6,
            s: 1024,
            sp: 0x2000,
            anchor: (0, 0, 0),
        });
        assert_eq!(p.places[0].0, ObjectId(1));
        // With cannon (R 1000 -> s = 2000): all distances x1000/512.
        let p = arrange(&Layout {
            kind: 1,
            ranks: &ranks,
            n: 6,
            s: 2000,
            sp: 0x2000,
            anchor: (0, 0, 0),
        });
        assert_eq!(xz(&p)[0], (0, -4000));
    }

    #[test]
    fn type_6_places_nothing_and_fails() {
        // K-210: type 6 always fails for n >= 2.
        let p = place(6, 3, 0);
        assert!(!p.ok && p.places.is_empty());
    }

    #[test]
    fn extreme_spread_and_direction_do_not_panic() {
        let mut ranks: [Vec<ObjectId>; 10] = Default::default();
        ranks[4] = ids(12);
        for kind in [1, 2, 3, 4] {
            for (sp, dir) in [(i32::MAX, i32::MIN), (0, i32::MAX), (-0x2000, 12345)] {
                let _ = arrange(&Layout {
                    kind,
                    ranks: &ranks,
                    n: 12,
                    s: 1024,
                    sp,
                    anchor: (i32::MAX, i32::MIN, dir),
                });
            }
        }
    }
    #[test]
    fn column_ends_follow_the_front_pair_of_the_ranks() {
        let ends = |f: &dyn Fn(&mut [Vec<ObjectId>; 10])| {
            let mut ranks: [Vec<ObjectId>; 10] = Default::default();
            f(&mut ranks);
            column_ends(&ranks)
        };
        // A rank with at least two members: front pair of the same rank.
        assert_eq!(ends(&|r| r[4] = ids(3)), (ObjectId(1), ObjectId(2)));
        // Two ranks with one member each: higher -> end_max, lower -> end_min.
        let r = ends(&|r| {
            r[4] = vec![ObjectId(1)];
            r[7] = vec![ObjectId(9)];
        });
        assert_eq!(r, (ObjectId(9), ObjectId(1)));
        // Highest rank with two members: the second of the same rank, not the lower one.
        let r = ends(&|r| {
            r[4] = vec![ObjectId(1)];
            r[7] = vec![ObjectId(8), ObjectId(9)];
        });
        assert_eq!(r, (ObjectId(8), ObjectId(9)));
        // Exactly one member: end_min stays end_max (not -1).
        assert_eq!(
            ends(&|r| r[5] = vec![ObjectId(3)]),
            (ObjectId(3), ObjectId(3))
        );
        // No members: both -1.
        assert_eq!(ends(&|_| {}), (ObjectId::NONE, ObjectId::NONE));
        // Rank 0 counts too (the search runs down to rank 0).
        let r = ends(&|r| {
            r[0] = vec![ObjectId(2)];
            r[6] = vec![ObjectId(7)];
        });
        assert_eq!(r, (ObjectId(7), ObjectId(2)));
        assert_eq!(
            ends(&|r| r[0] = vec![ObjectId(2)]),
            (ObjectId(2), ObjectId(2))
        );
    }

    #[test]
    fn ends_mirror_and_pivot() {
        let p = place(1, 5, 0);
        let mut offsets = MfcMap::new();
        for e in &p.places {
            offsets.set(e.0.0, e.1);
        }
        // Largest lz: member 3 (1024), smallest: member 1 (-1024).
        assert_eq!(find_ends(&offsets), Some((ObjectId(3), ObjectId(1))));
        // Rotation +90 degrees around the inner wing end (end_max for d >= 0): (0, 0) rotates around (0, 2048).
        let ends = (ObjectId(3), ObjectId(1));
        assert_eq!(
            pivot(1, &offsets, ends, 0x2000, (0, 0, 0), 90 << 12),
            (2048, 2048)
        );
        assert_eq!(
            pivot(2, &offsets, ends, 0x2000, (7, 9, 0), 90 << 12),
            (7, 9),
            "square: center"
        );
        // Missing end -> offset (0, 0) -> pivot = reference.
        assert_eq!(
            pivot(
                1,
                &offsets,
                (ObjectId::NONE, ObjectId::NONE),
                0x2000,
                (5, 6, 0),
                1 << 12
            ),
            (5, 6)
        );
        mirror(&mut offsets);
        assert_eq!(offsets.get(1), Some(&(0, 1024)));
        assert_eq!(find_ends(&MfcMap::new()), None);
    }
}
