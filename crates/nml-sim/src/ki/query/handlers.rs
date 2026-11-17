//! Handler per builtin ID (re/ki-queries 2.1, re/ki-builtins A 2/B 3/C 3): **one** table [`ROWS`] with code, pops,
//! push, outputs and pause per ID; it is also the single source of [`super::is_query`]. Pauses are in the table, not
//! from `signature()` (only the corpus IDs have one).
//!
//! Flow per call ([`call`]): pop `pops` arguments `a1 … aN` (first pop = `aN`), then
//! - [`Run::Ask`]: build the field `{code, argc, words…}` (words from `[2]`), [`super::dispatch`] over a [`View`] of
//!   slot `me`, apply the direct changes immediately ([`super::apply`]), with `push` push the word `[2]` and write
//!   `var(a) = [k]` for each output `(a, k)` (`FUN_004137f0`, in table order);
//! - [`Run::Own`]: own flow with the popped arguments (pushes itself);
//!
//! then `pause_ms ≠ 0` -> `m_iPauseCounter` (`FUN_004b4180`).
//!
//! **Extension:** insert rows sorted by ID and without overlap (test `rows_are_sorted_disjoint_world_builtins`); own
//! flows of larger groups in `handlers_<group>.rs`.

use super::codes_player::PEOPLE;
use super::codes_search::HQ_BY_NATION;
use super::handlers_list::{all_match, any_match, distance, pair};
use super::{SimKiWorld, apply, dispatch, word};
use crate::ki::world::{BuiltinCx, KiNote};
use crate::player::View;

use self::W::{Arg, Me};

/// Word of the query field from `[2]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum W {
    /// `me` (`Instance+0x6AC`, player of the instance).
    Me,
    /// Argument `a(i+1)` in source order.
    Arg(u8),
}

/// Own flow: ID, arguments `a1 … aN`, instance, world.
pub(crate) type OwnFn = fn(u16, &[i32], &mut BuiltinCx<'_>, &mut SimKiWorld<'_, '_>);

/// Flow of a handler.
#[derive(Clone, Copy)]
pub(crate) enum Run {
    /// Query `{code, argc, words…}`; push `[2]` and outputs as in the row.
    Ask { argc: i32, words: &'static [W] },
    /// Own flow (nation tables, `0x8C`, `0xE6` ...); `push` and `outs` of the row only describe it.
    Own(OwnFn),
}

/// Handler of the IDs `lo..=hi`.
#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub lo: u16,
    pub hi: u16,
    /// Query code, 0 without a query.
    pub code: u32,
    /// Number of arguments.
    pub pops: u8,
    /// Pushes a result in the normal case.
    pub push: bool,
    /// Output variables `(argument position 0-based, word)`, in write order.
    pub outs: &'static [(u8, u8)],
    /// `m_iPauseCounter` afterwards (ms), 0 = none.
    pub pause_ms: i32,
    pub run: Run,
}

impl Row {
    /// With the output variables `outs`.
    const fn outs(self, outs: &'static [(u8, u8)]) -> Self {
        Self { outs, ..self }
    }

    /// With `m_iPauseCounter = ms` afterwards.
    const fn pause(self, pause_ms: i32) -> Self {
        Self { pause_ms, ..self }
    }
}

/// Query with result `[2]`.
const fn ask(lo: u16, hi: u16, code: u32, pops: u8, argc: i32, words: &'static [W]) -> Row {
    Row {
        lo,
        hi,
        code,
        pops,
        push: true,
        outs: &[],
        pause_ms: 0,
        run: Run::Ask { argc, words },
    }
}

/// Query without a result (codes without effect).
const fn quiet(lo: u16, hi: u16, code: u32, pops: u8, argc: i32, words: &'static [W]) -> Row {
    Row {
        push: false,
        ..ask(lo, hi, code, pops, argc, words)
    }
}

/// Nearest search with a 500 ms pause afterwards (`0x4E24`-`0x4E27`, `0x4F24`-`0x4F28`, E-36).
const fn find(lo: u16, hi: u16, code: u32, pops: u8, argc: i32, words: &'static [W]) -> Row {
    ask(lo, hi, code, pops, argc, words).pause(500)
}

/// Words of the searches: `{me, me, a1…a4}`, `{me, a1…a5}`, `{me, me, a1…a5}`, `{me, a1…a6}`.
const OWN4: &[W] = &[Me, Me, Arg(0), Arg(1), Arg(2), Arg(3)];
const ANY5: &[W] = &[Me, Arg(0), Arg(1), Arg(2), Arg(3), Arg(4)];
const OWN5: &[W] = &[Me, Me, Arg(0), Arg(1), Arg(2), Arg(3), Arg(4)];
const ANY6: &[W] = &[Me, Arg(0), Arg(1), Arg(2), Arg(3), Arg(4), Arg(5)];

/// Own flow.
const fn own(lo: u16, hi: u16, code: u32, pops: u8, push: bool, f: OwnFn) -> Row {
    Row {
        lo,
        hi,
        code,
        pops,
        push,
        outs: &[],
        pause_ms: 0,
        run: Run::Own(f),
    }
}

/// All query builtins, sorted by ID.
pub(crate) static ROWS: &[Row] = &[
    // A 2: main house to the map center (`0x4E21`, 5.4; 0x45 pushes `[3]`), nearest searches (5.3) own `(x, z, …)` or
    // for owner `p` `(p, x, z, …)`; only `0x4E24`-`0x4E27` pause.
    own(0x45, 0x45, 0x4E21, 0, true, hq_type),
    ask(0x46, 0x46, 0x4E21, 0, 2, &[Me, Me]),
    ask(0x47, 0x47, 0x4E21, 1, 2, &[Me, Arg(0)]),
    ask(0x48, 0x4A, 0x4E22, 4, 6, OWN4),
    ask(0x4B, 0x4D, 0x4E22, 5, 6, ANY5),
    ask(0x4E, 0x50, 0x4E23, 4, 6, OWN4),
    ask(0x51, 0x53, 0x4E23, 5, 6, ANY5),
    find(0x55, 0x57, 0x4E24, 4, 6, OWN4),
    find(0x58, 0x5A, 0x4E24, 5, 6, ANY5),
    find(0x5B, 0x5D, 0x4E25, 4, 6, OWN4),
    find(0x5E, 0x60, 0x4E25, 5, 6, ANY5),
    find(0x61, 0x63, 0x4E26, 4, 6, OWN4),
    find(0x64, 0x66, 0x4E26, 5, 6, ANY5),
    find(0x67, 0x69, 0x4E27, 4, 6, OWN4),
    find(0x6A, 0x6C, 0x4E27, 5, 6, ANY5),
    // Object queries (5.14, 5.15): filter check, life, position (`var(&z) = [4]` before `var(&x) = [3]`), type with
    // module 0xD.
    ask(0x6E, 0x70, 0x4E28, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(0x71, 0x73, 0x4E29, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(0x74, 0x76, 0x4E2A, 1, 2, &[Me, Arg(0)]),
    ask(0x78, 0x7A, 0x4E2B, 1, 2, &[Me, Arg(0)]),
    ask(0x7B, 0x7D, 0x4E2C, 3, 4, &[Me, Arg(0), Arg(1), Arg(2)]).outs(&[(2, 4), (1, 3)]),
    ask(0x7E, 0x7E, 0x4FC4, 1, 1, &[Arg(0)]),
    // A 2: match and players (5.16).
    ask(0x7F, 0x7F, 0x4E2D, 0, 1, &[Me]),
    ask(0x80, 0x80, 0x4E2E, 0, 1, &[Me]),
    ask(0x81, 0x81, 0x4E2F, 0, 1, &[Me]),
    ask(0x83, 0x83, PEOPLE, 0, 2, &[Me, Me]),
    ask(0x84, 0x84, PEOPLE, 1, 2, &[Me, Arg(0)]),
    ask(0x85, 0x85, 0x4E32, 1, 2, &[Me, Arg(0)]),
    ask(0x86, 0x86, 0x4EF7, 0, 1, &[Me]),
    // 5.18: reachable cell without push `(x, z, &x', &z')` (`var(a3) = [5]`, `var(a4) = [6]`); to the object
    // `(obj, x, z, &x', &z')` with push (`var(a4) = [6]`, `var(a5) = [7]`).
    quiet(
        0x87,
        0x87,
        0x4E34,
        4,
        5,
        &[Me, Arg(0), Arg(1), Arg(2), Arg(3)],
    )
    .outs(&[(2, 5), (3, 6)]),
    ask(0x88, 0x88, 0x4E35, 5, 6, ANY5).outs(&[(3, 6), (4, 7)]),
    ask(0x8A, 0x8A, 0x4E37, 0, 1, &[Me]),
    ask(0x8B, 0x8B, 0x4E38, 0, 1, &[Me]),
    own(0x8C, 0x8C, 0x4E39, 0, true, difficulty),
    ask(0x8F, 0x8F, 0x4E3C, 0, 1, &[Me]),
    ask(0x92, 0x92, 0x4E3E, 1, 3, &[Me, Me, Arg(0)]),
    ask(0x93, 0x93, 0x4E3E, 2, 3, &[Me, Arg(0), Arg(1)]),
    // 5.12.3: own object count of `type` > 0 (`{2, me, a1}`) or of player `p` (`{2, a1, a2}`).
    ask(0x94, 0x94, 0x4E3F, 1, 2, &[Me, Arg(0)]),
    ask(0x95, 0x95, 0x4E3F, 2, 2, &[Arg(0), Arg(1)]),
    ask(0x96, 0x96, 0x4E40, 2, 3, &[Me, Arg(0), Arg(1)]),
    // E-37: queries without a counterpart or with a wrong argc.
    quiet(0x98, 0x98, 0x4E42, 3, 4, &[Me, Arg(0), Arg(1), Arg(2)]),
    ask(0x99, 0x99, 0x4E43, 2, 3, &[Me, Arg(0), Arg(1)]),
    // Type with module 3 (GUID in `[2]`, no player).
    ask(0x9A, 0x9A, 0x4F6F, 1, 1, &[Arg(0)]),
    // A 2: set or clear the mark for `me`, directly (5.9).
    quiet(0x9B, 0x9D, 0x4E44, 1, 2, &[Me, Arg(0)]),
    quiet(0x9E, 0xA0, 0x4E45, 1, 2, &[Me, Arg(0)]),
    // 5.19: nation table mounted -> on foot (E-80).
    own(0xA4, 0xA4, PEOPLE, 1, true, mounted_to_foot),
    // 5.12.3: object counts (`type`/filter ID, without orders), buildings (module 3) and units (module 0) with `argc 1`
    // and `[3]`, rectangle `(x ± r, z ± r)`, with orders (`0x4EFD`), sum over the reference players 0-6 (`0x4F56`,
    // E-64), rectangle with corners (`0x4F8D`).
    ask(0xA5, 0xA5, 0x4E47, 1, 3, &[Me, Me, Arg(0)]),
    ask(0xA6, 0xA6, 0x4E47, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(0xA7, 0xA7, 0x4E48, 0, 1, &[Me, Me]),
    ask(0xA8, 0xA8, 0x4E48, 1, 1, &[Me, Arg(0)]),
    ask(0xA9, 0xA9, 0x4E49, 0, 1, &[Me, Me]),
    ask(0xAA, 0xAA, 0x4E49, 1, 1, &[Me, Arg(0)]),
    ask(0xAB, 0xAB, 0x4E4A, 0, 1, &[Me, Me]),
    ask(0xAC, 0xAC, 0x4E4A, 1, 1, &[Me, Arg(0)]),
    ask(0xAD, 0xAD, 0x4E4B, 4, 6, OWN4),
    ask(0xAE, 0xAE, 0x4E4B, 5, 6, ANY5),
    ask(0xAF, 0xAF, 0x4EFD, 1, 3, &[Me, Me, Arg(0)]),
    ask(0xB0, 0xB0, 0x4EFD, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(
        0xB1,
        0xB1,
        0x4F56,
        4,
        5,
        &[Me, Arg(0), Arg(1), Arg(2), Arg(3)],
    ),
    ask(0xB2, 0xB2, 0x4E35, 5, 6, ANY5).outs(&[(3, 6), (4, 7)]),
    quiet(0xB5, 0xB5, 0x4EFC, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(0xB7, 0xB7, 0x4F8D, 5, 7, OWN5),
    ask(0xB8, 0xB8, 0x4F8D, 6, 7, ANY6),
    ask(
        0xB9,
        0xB9,
        0x4F92,
        5,
        6,
        &[Me, Arg(0), Arg(1), Arg(2), Arg(3), Arg(4)],
    ),
    // Gold and food of an object.
    ask(0xBF, 0xBF, 0x50D7, 1, 2, &[Me, Arg(0)]),
    ask(0xC0, 0xC0, 0x50DC, 1, 2, &[Me, Arg(0)]),
    // 5.16.1: headhunter `(kind, &a, &b, &price)`; `var(c) = [6]`, `var(b) = [5]`, `var(a) = [4]`.
    ask(
        0xCD,
        0xCD,
        0x4E51,
        4,
        5,
        &[Me, Arg(0), Arg(1), Arg(2), Arg(3)],
    )
    .outs(&[(3, 6), (2, 5), (1, 4)]),
    // 5.12.3: running counter-upgrades with source on slot `p`.
    ask(0xCF, 0xCF, 0x510E, 1, 2, &[Me, Arg(0)]),
    // Weather `{1, a1}` (E-39 does not apply: pop 1 without effect).
    quiet(0xD0, 0xD0, 0x5000, 1, 1, &[Arg(0)]),
    // Living space.
    ask(0xD2, 0xD2, 0x4E53, 0, 1, &[Me]),
    ask(0xD3, 0xD3, 0x4E54, 0, 2, &[Me, Me]),
    ask(0xD4, 0xD4, 0x4E54, 1, 2, &[Me, Arg(0)]),
    ask(0xD5, 0xD5, 0x4E55, 0, 2, &[Me, Me]),
    ask(0xD6, 0xD6, 0x4E55, 1, 2, &[Me, Arg(0)]),
    // 5.17: fog in the square `(p, x, z, r)` or `(x, z, r)` for `me`; `0x500A` pushes the result, `0x4FF6` pushes
    // "nothing explored".
    ask(
        0xD7,
        0xD7,
        0x500A,
        4,
        5,
        &[Me, Arg(0), Arg(1), Arg(2), Arg(3)],
    ),
    ask(0xD8, 0xD8, 0x500A, 3, 5, &[Me, Me, Arg(0), Arg(1), Arg(2)]),
    own(0xD9, 0xD9, 0x4FF6, 4, true, unexplored),
    own(0xDA, 0xDA, 0x4FF6, 3, true, unexplored),
    // 5.13: value of a foreign AI `(player, key)`.
    own(0xE6, 0xE6, 0, 2, true, engine_value),
    // B 3: clear marks of a list `(list, start, n)` (`0x4FA6`, 5.9, E-32).
    own(0x105, 0x107, 0x4FA6, 3, false, unmark_list),
    // 5.19: mounted GUID, scout, nation buildings.
    own(0x108, 0x108, 0, 1, true, mounted_guid),
    // 5.8, 5.15.1: lists `(list, start, n, …)` with distance `(x, z, R)`, filter or filter pair; the alias IDs
    // 0x117/0x11E/0x11F/0x121 share the flow.
    own(0x112, 0x112, 0x4E2C, 6, true, distance),
    own(0x113, 0x113, 0x4E28, 4, true, all_match),
    own(0x114, 0x114, 0x4E28, 5, true, pair),
    own(0x115, 0x115, 0x4E28, 4, true, any_match),
    own(0x116, 0x116, PEOPLE, 0, true, scout),
    own(0x117, 0x117, 0x4E2C, 6, true, distance),
    own(0x11E, 0x11E, 0x4E28, 4, true, all_match),
    own(0x11F, 0x11F, 0x4E28, 5, true, pair),
    own(0x120, 0x120, 0, 1, true, scout),
    own(0x121, 0x121, 0x4E28, 4, true, any_match),
    // Object and last attacker exist (5.14).
    ask(0x122, 0x124, 0x50BE, 1, 2, &[Me, Arg(0)]),
    // B-10: `{0x50EB, 3, P, a, b}`, `{0x4F47, 2, P, a}`.
    quiet(0x12A, 0x12A, 0x50EB, 2, 3, &[Me, Arg(0), Arg(1)]),
    quiet(0x12B, 0x12B, 0x4F47, 1, 2, &[Me, Arg(0)]),
    ask(0x131, 0x131, 0x4E89, 1, 2, &[Me, Arg(0)]),
    // 5.12.4: may `me` build or make the GUID (silent, E-67).
    ask(0x132, 0x132, 0x4E8A, 1, 2, &[Me, Arg(0)]),
    own(0x133, 0x135, PEOPLE, 0, true, nation_building),
    // 5.12.3: research button `(a, b)` in the menu of `a` (`{0x4F33, 3, me, a, b}`).
    ask(0x136, 0x136, 0x4F33, 2, 3, &[Me, Arg(0), Arg(1)]),
    // Production (progress, build list), riding/wood type, container, dead or missing.
    ask(0x137, 0x137, 0x4F38, 1, 2, &[Me, Arg(0)]),
    ask(0x138, 0x138, 0x4F42, 1, 2, &[Me, Arg(0)]),
    // 5.12.3: number of construction orders of the GUID.
    ask(0x139, 0x139, 0x4F6A, 1, 2, &[Me, Arg(0)]),
    // 5.10: are all objects of the filter producing right now?
    ask(0x13A, 0x13A, 0x50D2, 1, 2, &[Me, Arg(0)]),
    ask(0x140, 0x140, 0x4F65, 1, 2, &[Me, Arg(0)]),
    ask(0x141, 0x141, 0x4F94, 1, 1, &[Arg(0)]),
    ask(0x143, 0x143, 0x5113, 1, 2, &[Me, Arg(0)]),
    // C 3.
    ask(0x144, 0x144, 0x5118, 0, 1, &[Me]),
    // Color, attack state, type GUID (`[2]` = ID).
    ask(0x147, 0x147, 0x511D, 1, 2, &[Me, Arg(0)]),
    ask(0x15C, 0x15C, 0x5122, 1, 2, &[Me, Arg(0)]),
    ask(0x16A, 0x16C, 0x4FAB, 1, 2, &[Me, Arg(0)]),
    ask(0x16D, 0x16D, 0x502D, 1, 2, &[Me, Arg(0)]),
    // 5.11.3: take an attack report `(x, z, &a, &b)`; `var(b) = [4]`, `var(a) = [3]`.
    ask(0x16F, 0x16F, 0x512E, 4, 3, &[Me, Arg(0), Arg(1)]).outs(&[(3, 4), (2, 3)]),
    ask(0x170, 0x171, 0x4FB5, 1, 2, &[Me, Arg(0)]),
    ask(0x175, 0x175, 0x4FC9, 1, 1, &[Arg(0)]),
    quiet(0x177, 0x177, 0x4FD3, 2, 3, &[Me, Arg(0), Arg(1)]),
    ask(0x179, 0x17A, 0x4FC9, 1, 1, &[Arg(0)]),
    ask(0x18E, 0x18F, 0x4F2E, 1, 2, &[Me, Arg(0)]),
    ask(0x190, 0x190, 0x4EE8, 1, 2, &[Me, Arg(0)]),
    ask(0x196, 0x196, 0x4EEE, 1, 2, &[Me, Arg(0)]),
    ask(0x197, 0x197, 0x4F2E, 1, 2, &[Me, Arg(0)]),
    ask(0x19D, 0x19E, 0x4F79, 1, 2, &[Me, Arg(0)]),
    ask(0x19F, 0x19F, 0x4FA1, 1, 2, &[Me, Arg(0)]),
    ask(0x1A0, 0x1A0, 0x4F9C, 1, 2, &[Me, Arg(0)]),
    // 5.18.3: cell count of the tile path `(obj, sx, sz, tx, tz)`.
    ask(0x1A1, 0x1A1, 0x4FB0, 5, 6, ANY5),
    ask(0x1A3, 0x1A3, 0x4FF1, 1, 1, &[Arg(0)]),
    ask(0x1AA, 0x1AA, 0x4FF1, 1, 1, &[Arg(0)]),
    ask(0x1AB, 0x1AB, 0x4FA1, 1, 2, &[Me, Arg(0)]),
    // C 3: mark bit of `me` or `p` (`(p, id)`).
    ask(0x1AE, 0x1B0, 0x4F97, 1, 2, &[Me, Arg(0)]),
    ask(0x1B1, 0x1B3, 0x4F97, 2, 2, &[Arg(0), Arg(1)]),
    // `{0x5014, 3, P, id, b}`: `b` never has an effect (E-74).
    ask(0x1B9, 0x1BB, 0x5014, 2, 3, &[Me, Arg(0), Arg(1)]),
    quiet(0x1BC, 0x1BE, 0x500F, 1, 2, &[Me, Arg(0)]),
    // C 3: searches without mark (`0x4F2x`) and with radius `[8]` (argc 7); 0x201-0x203 call the 6-word handler of 0x58
    // (5.3).
    find(0x1E5, 0x1E7, 0x4F24, 4, 6, OWN4),
    find(0x1E8, 0x1EA, 0x4F24, 5, 6, ANY5),
    find(0x1EB, 0x1ED, 0x4F25, 4, 6, OWN4),
    find(0x1EE, 0x1F0, 0x4F25, 5, 6, ANY5),
    find(0x1F1, 0x1F3, 0x4F26, 4, 6, OWN4),
    find(0x1F4, 0x1F6, 0x4F26, 5, 6, ANY5),
    find(0x1F7, 0x1F9, 0x4F28, 4, 6, OWN4),
    find(0x1FA, 0x1FC, 0x4F28, 5, 6, ANY5),
    find(0x1FE, 0x200, 0x4E24, 5, 7, OWN5),
    find(0x201, 0x203, 0x4E24, 5, 6, ANY5),
    find(0x204, 0x206, 0x4E25, 5, 7, OWN5),
    find(0x207, 0x209, 0x4E25, 6, 7, ANY6),
    find(0x20A, 0x20C, 0x4E26, 5, 7, OWN5),
    find(0x20D, 0x20F, 0x4E26, 6, 7, ANY6),
    find(0x210, 0x212, 0x4E27, 5, 7, OWN5),
    find(0x213, 0x215, 0x4E27, 6, 7, ANY6),
    find(0x216, 0x218, 0x4F24, 5, 7, OWN5),
    find(0x219, 0x21B, 0x4F24, 6, 7, ANY6),
    find(0x21C, 0x21E, 0x4F25, 5, 7, OWN5),
    find(0x21F, 0x221, 0x4F25, 6, 7, ANY6),
    find(0x222, 0x224, 0x4F26, 5, 7, OWN5),
    find(0x225, 0x227, 0x4F26, 6, 7, ANY6),
    find(0x228, 0x22A, 0x4F28, 5, 7, OWN5),
    find(0x22B, 0x22D, 0x4F28, 6, 7, ANY6),
    // 5.12.3: is the counter-upgrade with GUID `a1` running anywhere?
    ask(0x230, 0x230, 0x5032, 1, 2, &[Me, Arg(0)]),
    // C 3: three chained filters `(x, z, fA, fB, fC)`, without pause. The handler builds `{me, me, a1…a5}` for all
    // three; but `0x50C8`/`0x50CD` read the head from `[4]` and the center from `[5]`/`[6]` (`0x004A3F80`,
    // `0x004AB52E…0x004AB590`).
    ask(0x235, 0x235, 0x50C3, 5, 7, OWN5),
    ask(0x236, 0x236, 0x50C8, 5, 7, OWN5),
    ask(0x237, 0x237, 0x50CD, 5, 7, OWN5),
    ask(0x23B, 0x23B, 0x5130, 0, 1, &[Me]),
];

/// Row of the ID `id` (binary search over the sorted table).
pub(crate) fn row(id: u16) -> Option<&'static Row> {
    let i = ROWS.partition_point(|r| r.hi < id);
    ROWS.get(i).filter(|r| r.lo <= id)
}

/// Handler of the query `id`; `false` only for an ID without a row.
pub(crate) fn call(id: u16, cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) -> bool {
    let Some(row) = row(id) else {
        return false;
    };
    let mut args: Vec<i32> = (0..row.pops).map(|_| cx.pop()).collect();
    args.reverse();
    match row.run {
        Run::Ask { argc, words } => {
            let me = i32::from(cx.me());
            let mut req = Vec::with_capacity(2 + words.len());
            req.extend([row.code as i32, argc]);
            req.extend(words.iter().map(|&x| match x {
                Me => me,
                Arg(i) => word(&args, usize::from(i)),
            }));
            query(w, cx.me(), &mut req);
            if row.push {
                cx.push(word(&req, 2));
            }
            for &(a, k) in row.outs {
                cx.set(
                    word(&args, usize::from(a)) as u32,
                    word(&req, usize::from(k)),
                );
            }
        }
        Run::Own(f) => f(id, &args, cx, w),
    }
    if row.pause_ms != 0 {
        cx.set_pause(row.pause_ms);
    }
    true
}

/// Make the query `req` from the view of slot `me` and apply its direct changes.
pub(crate) fn query(w: &mut SimKiWorld<'_, '_>, me: u8, req: &mut [i32]) {
    let mut direct = Vec::new();
    dispatch(&mut View::new(w.sim, me, w.sight), req, &mut direct);
    apply(w.sim, &direct);
}

/// `0x45` (`FUN_004a0340`): `{0x4E21, 2, me, me}`, push `[3]` = main house GUID of the own nation.
fn hq_type(_: u16, _: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let me = i32::from(cx.me());
    let mut req = [0x4E21, 2, me, me];
    query(w, cx.me(), &mut req);
    cx.push(req[3]);
}

/// `0xD9` (`FUN_004A2250`, `(p, x, z, r)`) and `0xDA` (`FUN_004A22E0`, `(x, z, r)` for `me`): field
/// `{0x4FF6, 5, me, p, x, z, r}`, push `[2] == 0` - "nothing explored in the square" (5.17).
fn unexplored(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let me = i32::from(cx.me());
    let (p, rest) = if id == 0xD9 {
        (word(args, 0), 1)
    } else {
        (me, 0)
    };
    let mut req = [
        0x4FF6,
        5,
        me,
        p,
        word(args, rest),
        word(args, rest + 1),
        word(args, rest + 2),
    ];
    query(w, cx.me(), &mut req);
    cx.push(i32::from(req[2] == 0));
}

/// `0x105`-`0x107` (`0x004A1AB0`): pops `n, start, list`; list via the pointer of the variable (`FUN_00413840`, without
/// reference): 0 -> nothing. Only `0 < n < 100`: the IDs `start…start+n−1` are read with creation (E-32) and
/// `{0x4FA6, n + 2, me, n, ids…}` (clears the marks of `me`). A pointer without a list would be an access to nonsense
/// in the original: `KiNote::Guarded` (re/ki-vm 12.3 P7).
fn unmark_list(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (list, start, n) = (word(args, 0) as u32, word(args, 1), word(args, 2));
    let Some(ids) = checked_list_ids(cx, w, id, list, start, n) else {
        return;
    };
    let me = i32::from(cx.me());
    let mut req = vec![0x4FA6, n + 2, me, n];
    req.extend(ids);
    query(w, cx.me(), &mut req);
}

/// List check of the builtins with "list, start, n" (`0x105`-`0x107`, group movement, behavior and attack on lists):
/// only for `var(list) ≠ 0` and `0 < n < 100` are the IDs read ([`list_ids`]); otherwise `None` without effect. A
/// pointer without a list would be an access to nonsense in the original: `None` and `KiNote::Guarded` for the builtin
/// `builtin` (re/ki-vm 12.3 P7).
pub(crate) fn checked_list_ids(
    cx: &mut BuiltinCx<'_>,
    w: &mut SimKiWorld<'_, '_>,
    builtin: u16,
    list: u32,
    start: i32,
    n: i32,
) -> Option<Vec<i32>> {
    if cx.get(list) == 0 || !(1..100).contains(&n) {
        return None;
    }
    let ids = list_ids(cx, list, start, n);
    if ids.is_none() {
        w.notes.push(KiNote::Guarded {
            slot: cx.slot(),
            thread: cx.thread_id(),
            module: cx.module(),
            builtin,
            what: "list pointer without a list",
        });
    }
    ids
}

/// `n` IDs `start…start+n−1` of the list behind the pointer in the variable `list_addr` (`FUN_00413840`, without
/// reference resolution) with `FUN_004E7B30`: missing keys are created with 0 (E-32, B-9). `None`: there is no list
/// there.
pub(crate) fn list_ids(
    cx: &mut BuiltinCx<'_>,
    list_addr: u32,
    start: i32,
    n: i32,
) -> Option<Vec<i32>> {
    let pointer = cx.get(list_addr);
    let list = cx.list_at_mut(pointer)?;
    Some(
        (0..n.max(0))
            .map(|i| *list.entry_or_default(start.wrapping_add(i)))
            .collect(),
    )
}

/// `FUN_0049e5d0`: nation of `me` (`{20000, 2, me, me}`), pushed in the interpreter and popped immediately.
fn own_nation(cx: &BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) -> i32 {
    let me = i32::from(cx.me());
    let mut req = [PEOPLE as i32, 2, me, me];
    query(w, cx.me(), &mut req);
    req[2]
}

/// `0x8C` (`0x0049E350`): `0x4E39`, then `< 0 → 1`; 0 stays 0 (5.16).
fn difficulty(_: u16, _: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let mut req = [0x4E39, 1, i32::from(cx.me())];
    query(w, cx.me(), &mut req);
    cx.push(if req[2] < 0 { 1 } else { req[2] });
}

/// `0xA4` (`0x004AEBBC`): convert type `a1` per own nation, otherwise unchanged (5.19, E-80).
fn mounted_to_foot(_: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    const MAP: &[(i32, &[(i32, i32)])] = &[
        (0, &[(1406, 1403)]),
        (
            1,
            &[
                (1701, 1700),
                (1704, 1703),
                (1706, 1705),
                (1708, 1707),
                (1710, 1709),
            ],
        ),
        (2, &[(1101, 1100), (1104, 1103), (1100, 1101)]),
        (3, &[(2601, 2600), (2604, 2603), (2606, 2605)]),
        (4, &[(2001, 2000), (2004, 2003), (2008, 2007)]),
        (5, &[(2301, 2300), (2305, 2304), (2307, 2306), (2309, 2308)]),
    ];
    let kind = word(args, 0);
    let nation = own_nation(cx, w);
    let mapped = MAP
        .iter()
        .find(|&&(v, _)| v == nation)
        .and_then(|(_, pairs)| pairs.iter().find(|&&(from, _)| from == kind))
        .map_or(kind, |&(_, to)| to);
    cx.push(mapped);
}

/// `0x108` (`0x004AF6FE`): 1 if `a1` is a mounted GUID of the fixed list (5.19).
fn mounted_guid(_: u16, args: &[i32], cx: &mut BuiltinCx<'_>, _: &mut SimKiWorld<'_, '_>) {
    const MOUNTED: [i32; 22] = [
        1101, 1104, 1114, 1406, 1701, 1704, 1706, 1708, 1710, 2001, 2004, 2008, 2301, 2305, 2307,
        2309, 2316, 2601, 2604, 2606, 2609, 2619,
    ];
    cx.push(i32::from(MOUNTED.contains(&word(args, 0))));
}

/// `0x116` (own nation) / `0x120` (nation `a1`): 4 -> 2005 (ENG_Hund), 5 -> 2306
/// (PAT_Kundschafter), otherwise 0 (5.19).
fn scout(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let nation = if id == 0x116 {
        own_nation(cx, w)
    } else {
        word(args, 0)
    };
    cx.push(match nation {
        4 => 2005,
        5 => 2306,
        _ => 0,
    });
}

/// `0x133`-`0x135`: nation buildings "xx02", "xx04", "xx03" of the own nation; nation not in 0...5 -> no push (5.19,
/// E-34).
fn nation_building(id: u16, _: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let suffix = match id {
        0x133 => 2,
        0x134 => 4,
        _ => 3,
    };
    let nation = own_nation(cx, w);
    if let Some(base) = usize::try_from(nation)
        .ok()
        .and_then(|v| HQ_BY_NATION.get(v))
    {
        cx.push(base + suffix);
    }
}

/// `0xE6` (`KIManager vf[6]` `0x004A4B70`, 5.13): `m_iValues[key]` of the engine on slot `slot` (missing -> 0); slot
/// without an engine -> -1. A slot outside 0-7 reads neighboring fields in the original (slot 8 crashes): here -1 like
/// an empty slot and `KiNote::Guarded` (E-38).
fn engine_value(id: u16, args: &[i32], cx: &mut BuiltinCx<'_>, w: &mut SimKiWorld<'_, '_>) {
    let (slot, key) = (word(args, 0), word(args, 1));
    let v = cx.engine_value(slot, key);
    if v.is_none() && !(0..8).contains(&slot) {
        w.notes.push(KiNote::Guarded {
            slot: cx.slot(),
            thread: cx.thread_id(),
            module: cx.module(),
            builtin: id,
            what: "slot outside 0-7",
        });
    }
    cx.push(v.unwrap_or(-1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ki::builtins::{has_case, is_local};

    #[test]
    fn rows_are_sorted_disjoint_world_builtins() {
        for pair in ROWS.windows(2) {
            assert!(
                pair[0].hi < pair[1].lo,
                "{:#x} before {:#x}",
                pair[0].hi,
                pair[1].lo
            );
        }
        for r in ROWS {
            assert!(r.lo <= r.hi, "{:#x}", r.lo);
            for id in r.lo..=r.hi {
                let id32 = u32::from(id);
                assert!(
                    has_case(id32) && !is_local(id32),
                    "{id:#x} is not a world builtin"
                );
                assert_eq!(row(id).map(|x| (x.lo, x.hi)), Some((r.lo, r.hi)));
            }
        }
        assert!(row(0).is_none() && row(u16::MAX).is_none());
    }

    #[test]
    fn rows_use_only_popped_arguments() {
        for r in ROWS {
            if let Run::Ask { words, .. } = r.run {
                for &w in words {
                    if let Arg(i) = w {
                        assert!(i < r.pops, "{:#x}: a{}", r.lo, i + 1);
                    }
                }
            } else {
                assert!(r.outs.is_empty(), "{:#x}: outputs in an own flow", r.lo);
            }
            for &(a, _) in r.outs {
                assert!(a < r.pops, "{:#x}: output a{}", r.lo, a + 1);
            }
        }
    }
}
