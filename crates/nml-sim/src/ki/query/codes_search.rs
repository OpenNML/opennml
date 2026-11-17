//! Nearest searches, AI mark, production check and attack reports of the dispatcher (re/ki-queries 5.3-5.6, 5.9-5.11).
//!
//! All searches run through [`View::find_nearest`] (`CollisionMgr_FindNearestMatching` with center distance, reference
//! player `[3]`; under `Sight::Fog` with a sight check per candidate). Quirks of the original that are listed here:
//! search center = **tile corner** `(x << 12, z << 12)` (E-54), head filter from the filter ID with **OR-ed** bits
//! (E-49), `mode2` of the head cleared per code (E-50), radius `[8]` raw for `0x4F24`/`0x4F25` (E-29), fishing basket
//! (GUID 3505) as a hit -> 0 without further search (E-53). Marking and taking a report are direct changes
//! ([`Direct`]), generated before `[2]` is overwritten.

use nml_formats::targets::MatchFilter;

use super::filters::filter_from_id;
use super::{Direct, argc, put, word};
use crate::filter::class_filter;
use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::player::View;

/// Main house GUID per nation 0-5 (`FUN_00477C00`); also the base of the nation buildings "xx02" ...
pub(super) const HQ_BY_NATION: [i32; 6] = [1300, 1600, 1000, 2500, 1900, 2200];

/// GUID of the fishing basket: always 0 as a search hit (E-53).
const FISH_BASKET: u32 = 0xDB1;

/// `Fixed_FromInt(1)` (`DAT_00671948`).
const ONE: i32 = 4096;

/// Answers the codes of this group; `false` for foreign codes.
pub(super) fn answer(
    view: &mut View<'_, '_>,
    code: u32,
    req: &mut [i32],
    direct: &mut Vec<Direct>,
) -> bool {
    if let Some(spec) = search_spec(code) {
        search(view, req, direct, &spec);
        return true;
    }
    match code {
        0x4E21 => hq_near_centre(view, req),
        0x4E44 | 0x4E45 => mark(req, direct, code == 0x4E44),
        0x4F97 => is_marked(view, req),
        0x4FA6 => unmark_list(req, direct),
        0x50D2 => all_producing(view, req),
        0x512E => take_report(view, req, direct),
        _ => return false,
    }
    true
}

/// Head filter of a search.
#[derive(Clone, Copy)]
enum Head {
    /// Fresh filter (`FUN_00405EF0`), `prop_id` = `f[4]`.
    Fresh { prop_id: u32 },
    /// Filter ID in word `i` (`FUN_0047E390`).
    Id(usize),
}

/// Following filter (`f[0x14]`).
#[derive(Clone, Copy)]
enum Chain {
    None,
    /// `f1 → f2` with `f2` from the filter ID in word `i`.
    Two(usize),
    /// `f1 → f2 → f3`; `f2` (word `i`) gets `ANDNEXT 0x10000000`, `f3` (word `j`) stays.
    Three(usize, usize),
}

/// Radius at `argc 7` (otherwise -1 = whole map).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Radius {
    /// Always -1.
    Unbounded,
    /// `[8] · 4096`, wrapping (negative = unbounded).
    Scaled,
    /// `[8]` raw (E-29).
    Raw,
}

/// A row of the table of the search codes (re/ki-queries 5.3).
#[derive(Clone, Copy)]
struct Spec {
    /// Allowed `argc` (`lo..=hi`).
    argc: (i32, i32),
    /// Word of the center x (z = next word).
    centre: usize,
    head: Head,
    /// Bits OR-ed onto `f1.mode`.
    bits: u32,
    /// `f1.mode2 = 0` (E-50).
    clear_mode2: bool,
    chain: Chain,
    /// `f1.guid = [i]`.
    guid: Option<usize>,
    radius: Radius,
    /// Mark hits for `[2]`.
    mark: bool,
}

/// Chain `f1 → f2` around `([4], [5])`: head `[6]`, following filter `[7]`, argc 6/7.
const fn chained(bits: u32, clear_mode2: bool, radius: Radius, mark: bool) -> Spec {
    Spec {
        argc: (6, 7),
        centre: 4,
        head: Head::Id(6),
        bits,
        clear_mode2,
        chain: Chain::Two(7),
        guid: None,
        radius,
        mark,
    }
}

/// GUID `[4]` around `([5], [6])`: head `[7]`, argc 6/7, `mode2` of the head cleared.
const fn typed(bits: u32, radius: Radius, mark: bool) -> Spec {
    Spec {
        argc: (6, 7),
        centre: 5,
        head: Head::Id(7),
        bits,
        clear_mode2: true,
        chain: Chain::None,
        guid: Some(4),
        radius,
        mark,
    }
}

/// Three filters `f1 → f2 → f3` (head `[head]`, then `[7]`, `[8]`), argc 7, without radius.
const fn triple(centre: usize, head: usize, bits: u32, mark: bool) -> Spec {
    Spec {
        argc: (7, 7),
        centre,
        head: Head::Id(head),
        bits,
        clear_mode2: false,
        chain: Chain::Three(7, 8),
        guid: None,
        radius: Radius::Unbounded,
        mark,
    }
}

/// Fresh head (`0x4E22`/`0x4E23`), argc 6, without radius and mark.
const fn fresh(centre: usize, prop_id: u32, bits: u32, guid: Option<usize>) -> Spec {
    Spec {
        argc: (6, 6),
        centre,
        head: Head::Fresh { prop_id },
        bits,
        clear_mode2: true,
        chain: Chain::None,
        guid,
        radius: Radius::Unbounded,
        mark: false,
    }
}

/// Table of the search codes (re/ki-queries 5.3); `0x4F27` has no case.
fn search_spec(code: u32) -> Option<Spec> {
    use Radius::{Raw, Scaled};
    Some(match code {
        0x4E22 => fresh(4, 3, 0x80114, None),
        0x4E23 => fresh(5, 0, 0x80118, Some(4)),
        0x4E24 => chained(0x1008_0110, false, Scaled, true),
        0x4E25 => typed(0x80118, Scaled, true),
        0x4E26 => chained(0x1208_0110, true, Scaled, true),
        0x4E27 => typed(0x0208_0118, Scaled, true),
        0x4F24 => chained(0x1000_0110, true, Raw, false),
        0x4F25 => typed(0x118, Raw, false),
        0x4F26 => chained(0x1200_0110, true, Scaled, false),
        0x4F28 => typed(0x0200_0118, Scaled, false),
        0x50C3 => triple(4, 6, 0x1008_0110, true),
        0x50C8 => triple(5, 4, 0x1008_0110, true),
        0x50CD => triple(5, 4, 0x1000_0110, false),
        _ => return None,
    })
}

/// Fixed point of a whole tile (`Fixed_FromInt`, wrapping): tile corner (E-54).
fn corner(v: i32) -> i32 {
    v.wrapping_mul(ONE)
}

/// Mark of `object` for the player `p` (`FUN_00466F90`): only for `p & 31 < 8`.
fn mark_for(direct: &mut Vec<Direct>, object: ObjectId, p: i32, on: bool) {
    let q = (p & 31) as u8;
    if q < 8 {
        direct.push(Direct::Mark {
            object,
            player: q,
            on,
        });
    }
}

/// Hit of the search -> `[2]`: object ID, fishing basket or nothing -> 0 (`0x004AB654…`).
fn found(view: &View<'_, '_>, hit: Option<ObjectId>) -> Option<ObjectId> {
    let id = hit?;
    let guid = view.object(id, false)?.guid;
    (guid != FISH_BASKET).then_some(id)
}

/// Common flow of the nearest searches `0x4E22`...`0x50CD` (5.3): wrong `argc` -> field stays.
fn search(view: &mut View<'_, '_>, req: &mut [i32], direct: &mut Vec<Direct>, s: &Spec) {
    let n = argc(req);
    if n < s.argc.0 || n > s.argc.1 {
        return;
    }
    let rules = view.rules();
    let pos = (corner(word(req, s.centre)), corner(word(req, s.centre + 1)));
    let mut f1 = match s.head {
        Head::Fresh { prop_id } => MatchFilter {
            prop_id,
            ..MatchFilter::constructed()
        },
        Head::Id(i) => filter_from_id(rules, word(req, i)),
    };
    f1.mode |= s.bits;
    if let Some(i) = s.guid {
        f1.guid = word(req, i);
    }
    f1.fog_player = word(req, 2);
    if s.clear_mode2 {
        f1.mode2 = 0;
    }
    match s.chain {
        Chain::None => {}
        Chain::Two(i) => f1.next = Some(Box::new(filter_from_id(rules, word(req, i)))),
        Chain::Three(i, j) => {
            let mut f2 = filter_from_id(rules, word(req, i));
            f2.mode |= 0x1000_0000;
            f2.next = Some(Box::new(filter_from_id(rules, word(req, j))));
            f1.next = Some(Box::new(f2));
        }
    }
    let radius = match (n, s.radius) {
        (7, Radius::Scaled) => word(req, 8).wrapping_mul(ONE),
        (7, Radius::Raw) => word(req, 8),
        _ => -1,
    };
    let hit = view.find_nearest(pos, &f1, radius, word(req, 3));
    match found(view, hit) {
        Some(id) => {
            if s.mark {
                mark_for(direct, id, word(req, 2), true);
            }
            put(req, 2, id.as_i32());
        }
        None => put(req, 2, 0),
    }
}

/// `0x4E21` (argc 2, 5.4): nearest main house of the nation of `[3]` to the map center (half width as fixed point,
/// `SAR 1`); GUID filter without `HANDLETYPE` (also construction sites, E-51), `[3]` = GUID in both branches. Player
/// missing or nation outside 0-5 -> GUID 0, nothing found.
fn hq_near_centre(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let p = word(req, 3);
    let guid = view
        .players(p)
        .and_then(|info| usize::try_from(info.nation).ok())
        .and_then(|v| HQ_BY_NATION.get(v).copied())
        .unwrap_or(0);
    let (w, h) = view.map_size();
    let pos = (corner(w) >> 1, corner(h) >> 1);
    let f = MatchFilter {
        mode: 0x118,
        guid,
        ..MatchFilter::constructed()
    };
    let hit = view.find_nearest(pos, &f, -1, p);
    let id = found(view, hit).map_or(0, ObjectId::as_i32);
    put(req, 2, id);
    put(req, 3, guid);
}

/// `0x4E44`/`0x4E45` (argc 2): set or clear the mark of `[3]` for `[2]` (`FUN_0042DA90`: also dead ones, missing ->
/// nothing; [`super::apply`] checks that).
fn mark(req: &mut [i32], direct: &mut Vec<Direct>, on: bool) {
    if argc(req) == 2 {
        mark_for(direct, ObjectId::from_i32(word(req, 3)), word(req, 2), on);
    }
}

/// `0x4FA6` (**without** argc check, 5.9): clear the mark of `[2]` on the `[3]` objects `[4…]` (only `[3] > 0`). Words
/// beyond the field do not exist (the original would read behind it).
fn unmark_list(req: &mut [i32], direct: &mut Vec<Direct>) {
    let n = usize::try_from(word(req, 3)).unwrap_or(0);
    let p = word(req, 2);
    for i in 0..n.min(req.len().saturating_sub(4)) {
        mark_for(direct, ObjectId::from_i32(word(req, 4 + i)), p, false);
    }
}

/// `0x4F97` (argc 2, 5.9): bit `[2] & 31` of the mark of `[3]` (also dead); missing -> 0; bits >= 8 do not exist.
fn is_marked(view: &View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let q = word(req, 2) & 31;
    let bit = view
        .object(ObjectId::from_i32(word(req, 3)), false)
        .is_some_and(|o| q < 8 && o.ai_marks >> q & 1 == 1);
    put(req, 2, i32::from(bit));
}

/// `0x50D2` (argc 2, 5.10): do all objects of the filter `[3]` of slot `[2]` (object table, tile in the rectangle
/// 0...1000 inclusive) have the building flag `0x4000000` ("currently producing", `MatchesClass(0x4000003)`)? Without
/// objects the result is 1. `FUN_0047EA20` reads the tables of slots 0-10, the reference of the filter is the owner;
/// for slot > 10 the original reads behind the table (exception A7), here empty.
fn all_producing(view: &mut View<'_, '_>, req: &mut [i32]) {
    if argc(req) != 2 {
        return;
    }
    let f = filter_from_id(view.rules(), word(req, 3));
    let class = class_filter(0x0400_0003);
    let mut all = 1;
    for id in view.objects_in_slot_table(word(req, 2), &f) {
        let inside = view.object(id, false).is_some_and(|o| {
            let (x, z) = o.tile;
            (0..=1000).contains(&x) && (0..=1000).contains(&z)
        });
        if inside && !view.matches(id, &class, -1) {
            all = 0;
            break;
        }
    }
    put(req, 2, all);
}

/// `0x512E` (**without** argc check, 5.11.3): nearest attack report of slot `[2]` (0-7) to the tile `([3], [4])`;
/// distance `Fixed_Hypot` of the whole tiles (wrapping), strictly smaller wins (tie: first in MFC order, E-62). Hit:
/// `[2] = 1`, `[3]`/`[4]` = victim tile, take the report; otherwise `[2] = 0`, `[3]`/`[4]` stay.
fn take_report(view: &View<'_, '_>, req: &mut [i32], direct: &mut Vec<Direct>) {
    let (p, x, z) = (word(req, 2), word(req, 3), word(req, 4));
    let mut best: Option<(i32, i32, i32)> = None;
    if let Some(reports) = view.attack_reports(p) {
        let mut bd = i32::MAX;
        for (victim, t) in reports.iter() {
            let d = fixed_hypot(x.wrapping_sub(t.x), z.wrapping_sub(t.z));
            if d < bd {
                bd = d;
                best = Some((victim, t.x, t.z));
            }
        }
    }
    match (best, u8::try_from(p)) {
        (Some((victim, tx, tz)), Ok(player)) => {
            put(req, 2, 1);
            put(req, 3, tx);
            put(req, 4, tz);
            direct.push(Direct::TakeReport { player, victim });
        }
        _ => put(req, 2, 0),
    }
}
