//! Builtins for the player-wide engine memory (re/ki-builtins A 2, B 3/4, C 3/5): values and jump labels (`m_iValues`,
//! `m_mapGoto`), tables (`m_arrayGlobal`, B-5), global list, attacker lists (C 5 A5, C 5 A7), building list with repair
//! counters (C 5 A6), quarters list and position lists (C 5 A3/E-31, C 5 A4/E-30, B-4, re/ki-vm 12.3 P11). The
//! reference is the decompilation of `FUN_004ad380`.
//!
//! Pop order as in the original: the first pop is the last pushed argument. Outputs to variables go through `get`/`set`
//! without reference resolution (`FUN_00413840`/`FUN_004137f0`). Crash cases of the original report `KiNote::Guarded`
//! and push 0 where the normal path pushes; if the original pushes nothing for invalid input, the reimplementation
//! pushes nothing either (re/ki-vm 12.3 P7).

use std::collections::{BTreeMap, BTreeSet};

use super::arith::{guarded, in_type_range};
use crate::ki::engine::{ATTACKER_LISTS, EngineMem, POSITION_PLAYERS, PosList, RESOURCE_SLOTS};
use crate::ki::vm::{Exec, Thread};

/// `m_iValues` key of the reference point of the position lists (C 2).
const BASE_X: i32 = 200;
const BASE_Y: i32 = 201;
/// `0x151`/`0x152` check the player against 5 instead of 8 (C 5 A4, E-30).
const TYPE_READ_PLAYERS: usize = 5;
/// Column `c` of table `k` lies in `m_arrayGlobal[k + 10000·c]`, the row count in `m_arrayGlobal[0][k]` (B 2).
const COLUMN_STEP: i32 = 10_000;
const COLUMNS: i32 = 7;

type Tables = BTreeMap<i32, BTreeMap<i32, i32>>;

/// `true` for the IDs of this module.
pub fn handles(id: u32) -> bool {
    matches!(
        id,
        0x18 | 0x1B..=0x2A
            | 0x2C
            | 0x2D
            | 0x54
            | 0xBA..=0xBD
            | 0xDB
            | 0x109..=0x111
            | 0x128
            | 0x129
            | 0x13C..=0x13F
            | 0x142
            | 0x145
            | 0x146
            | 0x148..=0x153
            | 0x15E..=0x165
            | 0x169
            | 0x17C..=0x188
            | 0x1BF
    )
}

/// Memory builtin `id` of the instance `t`.
pub fn call(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    match id {
        0x18 | 0x1C | 0x1D | 0x20..=0x23 | 0x28 => global_list(ex, t, id),
        0x1B | 0x1E | 0x1F | 0x54 | 0xDB | 0x128 | 0x129 | 0x1BF => values(ex, t, id),
        0x24..=0x27 | 0x29 | 0x2A | 0x2C | 0x2D | 0xBA..=0xBD | 0x10F..=0x111 | 0x13C => {
            tables(ex, t, id)
        }
        0x15E..=0x165 | 0x169 | 0x17C..=0x182 | 0x187 | 0x188 => attackers(ex, t, id),
        0x183..=0x186 => building_queue(ex, t, id),
        0x13D..=0x13F | 0x142 => quarters(ex, t, id),
        0x109..=0x10E | 0x145 | 0x146 | 0x148..=0x153 => positions(ex, t, id),
        // Reachable only through `handles`.
        _ => {}
    }
}

/// Length as push value.
fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// `m_iValues` (`0x1B`, `0x1E`, `0x1F`, `0x128`, `0x129`, `0x1BF`) and `m_mapGoto` (`0x54`, `0xDB`).
fn values(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let mem = &mut *ex.mem;
    let out = match id {
        // (k, &v): `m_iValues[k] = var(&v)`.
        0x1B => {
            let addr = t.pop();
            let k = t.pop();
            mem.values.insert(k, t.vars.get(addr as u32));
            None
        }
        // (k, v).
        0x1E => {
            let v = t.pop();
            let k = t.pop();
            mem.values.insert(k, v);
            None
        }
        // (k) -> value, missing 0 without creating it.
        0x1F => {
            let k = t.pop();
            Some(mem.values.get(&k).copied().unwrap_or(0))
        }
        // (k) -> `m_mapGoto[k]`; missing or 0 -> set to 1.
        0x54 => {
            let k = t.pop();
            match mem.goto.get(&k) {
                Some(&v) if v != 0 => Some(v),
                _ => {
                    mem.goto.insert(k, 1);
                    Some(1)
                }
            }
        }
        // (k): `RemoveKey`.
        0xDB => {
            let k = t.pop();
            mem.goto.remove(&k);
            None
        }
        // (delta, key): only for an existing key.
        0x128 | 0x129 => {
            let key = t.pop();
            let delta = t.pop();
            if let Some(v) = mem.values.get_mut(&key) {
                *v = if id == 0x128 {
                    delta.wrapping_add(*v)
                } else {
                    v.wrapping_sub(delta)
                };
            }
            None
        }
        // (start, end, v) -> key - start of the first key in start..end-1 with the value
        // v, otherwise -1. The original queries each key individually; the result is the same.
        0x1BF => {
            let v = t.pop();
            let end = t.pop();
            let start = t.pop();
            let found = (start < end)
                .then(|| {
                    mem.values
                        .range(start..end)
                        .find(|&(_, &x)| x == v)
                        .map(|(&k, _)| k.wrapping_sub(start))
                })
                .flatten();
            Some(found.unwrap_or(-1))
        }
        _ => None,
    };
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `m_listGlobal` (`0x18`, `0x1C`, `0x1D`, `0x20`-`0x23`, `0x28`).
fn global_list(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let list = &mut ex.mem.global_list;
    let mut fault = None;
    let out = match id {
        // (x) -> 1 if x occurs.
        0x18 => {
            let x = t.pop();
            Some(i32::from(list.contains(&x)))
        }
        // (n) -> number of entries in the type range of `0x91` among the first min(n, count); the original queries
        // `0x4E3D` per entry via the stack (re/ki-vm 12.3 P15).
        0x1C => {
            let n = t.pop();
            let m = usize::try_from(n).unwrap_or(0).min(list.len());
            Some(count(
                list[..m].iter().filter(|&&v| in_type_range(v)).count(),
            ))
        }
        // (x) -> number of occurrences.
        0x1D => {
            let x = t.pop();
            Some(count(list.iter().filter(|&&v| v == x).count()))
        }
        // () -> first element, 0 if empty.
        0x20 => Some(list.first().copied().unwrap_or(0)),
        // () -> `RemoveAt(0)` if not empty.
        0x21 => {
            if !list.is_empty() {
                list.remove(0);
            }
            None
        }
        // (x): `Add`, but never 0.
        0x22 => {
            let x = t.pop();
            if x != 0 {
                list.push(x);
            }
            None
        }
        // (v, i): `InsertAt(i, v)` only for i < count; i < 0 throws an exception in the original.
        0x23 => {
            let i = t.pop();
            let v = t.pop();
            if i < count(list.len()) {
                match usize::try_from(i) {
                    Ok(i) => list.insert(i, v),
                    Err(_) => fault = Some("negative index in InsertAt"),
                }
            }
            None
        }
        // () -> count.
        0x28 => Some(count(list.len())),
        _ => None,
    };
    if let Some(what) = fault {
        guarded(ex, t, id, what);
    }
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `m_arrayGlobal[t][k]`, missing 0 without creating it (`FUN_004acc90`).
fn table_get(tables: &Tables, table: i32, key: i32) -> i32 {
    tables
        .get(&table)
        .and_then(|m| m.get(&key))
        .copied()
        .unwrap_or(0)
}

/// `m_arrayGlobal[t][k] = v`, missing table created (`FUN_004ad1f0`).
fn table_set(tables: &mut Tables, table: i32, key: i32, v: i32) {
    tables.entry(table).or_default().insert(key, v);
}

/// `FUN_004ad270(t, i)`: without a table, empty or without key `i` -> 0 and nothing. Otherwise rebuild the table: `i`
/// is dropped, larger keys move down by 1; the new (possibly empty) table replaces the old one. Returns the removed
/// value.
fn remove_row(tables: &mut Tables, table: i32, i: i32) -> i32 {
    let Some(map) = tables.get_mut(&table) else {
        return 0;
    };
    let Some(&removed) = map.get(&i) else {
        return 0;
    };
    *map = std::mem::take(map)
        .into_iter()
        .filter(|&(k, _)| k != i)
        .map(|(k, v)| (if k > i { k - 1 } else { k }, v))
        .collect();
    removed
}

/// Table of column `c` (1 to 7) of table `k`.
fn column(k: i32, c: i32) -> i32 {
    k.wrapping_add(COLUMN_STEP.wrapping_mul(c))
}

/// Row count of table `k` (`m_arrayGlobal[0][k]`), `None` without table 0 or without an entry.
fn row_count(tables: &Tables, k: i32) -> Option<i32> {
    tables.get(&0).and_then(|m| m.get(&k)).copied()
}

/// `0x13C`: is there a row `i` in `0 … count−1` with column 1 = `v1` and column 2 = `v2` (missing cells read 0)? The
/// original checks every row; here only the existing entries are run through, with the same result even for huge row
/// counts.
fn row_exists(tables: &Tables, k: i32, count: i32, v1: i32, v2: i32) -> bool {
    if count <= 0 {
        return false;
    }
    let empty = BTreeMap::new();
    let col1 = tables.get(&column(k, 1)).unwrap_or(&empty);
    let col2 = tables.get(&column(k, 2)).unwrap_or(&empty);
    let cell = |m: &BTreeMap<i32, i32>, i: i32| m.get(&i).copied().unwrap_or(0);
    if v1 != 0 {
        return col1
            .range(0..count)
            .any(|(&i, &x)| x == v1 && cell(col2, i) == v2);
    }
    if v2 != 0 {
        return col2
            .range(0..count)
            .any(|(&i, &x)| x == v2 && cell(col1, i) == 0);
    }
    // Two zeros are searched for: they exist if not every row has a value != 0.
    let nonzero: BTreeSet<i32> = col1
        .range(0..count)
        .chain(col2.range(0..count))
        .filter(|&(_, &x)| x != 0)
        .map(|(&i, _)| i)
        .collect();
    (nonzero.len() as u64) < count as u64
}

/// Tables in `m_arrayGlobal` (`0x24`-`0x2D` without `0x28`, `0xBA`-`0xBD`, `0x10F`-`0x111`, `0x13C`).
fn tables(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let tables = &mut ex.mem.tables;
    let out = match id {
        // (t, k, v).
        0x24 | 0x26 | 0x29 | 0x2C => {
            let v = t.pop();
            let k = t.pop();
            let table = t.pop();
            table_set(tables, table, k, v);
            None
        }
        // (t, k) or (t, k, x) with a discarded x.
        0x25 | 0x27 | 0x2A | 0x2D => {
            if id != 0x25 {
                t.pop();
            }
            let k = t.pop();
            let table = t.pop();
            Some(table_get(tables, table, k))
        }
        // (t, k) -> removed value or 0.
        0xBA..=0xBD => {
            let k = t.pop();
            let table = t.pop();
            Some(remove_row(tables, table, k))
        }
        // (K, v1..v7) -> index of the appended row.
        0x10F => {
            let mut v = [0; COLUMNS as usize];
            for slot in v.iter_mut().rev() {
                *slot = t.pop();
            }
            let k = t.pop();
            let counts = tables.entry(0).or_default();
            let row = counts.get(&k).copied().unwrap_or(0);
            // If the table is missing, the original sets the count to 1 beforehand (= row + 1).
            counts.entry(k).or_insert(row.wrapping_add(1));
            for (c, &value) in (1..).zip(&v) {
                table_set(tables, column(k, c), row, value);
            }
            table_set(tables, 0, k, row.wrapping_add(1));
            Some(row)
        }
        // (K, r, &a1..&a7) -> r or -1. The variables are written before the check, and the count drops
        // even without a row r (B-5).
        0x110 => {
            let mut addrs = [0; COLUMNS as usize];
            for slot in addrs.iter_mut().rev() {
                *slot = t.pop();
            }
            let r = t.pop();
            let k = t.pop();
            for (c, &addr) in (1..).zip(&addrs) {
                t.vars.set(addr as u32, table_get(tables, column(k, c), r));
            }
            match row_count(tables, k) {
                Some(n) if n.wrapping_sub(1) >= 0 => {
                    table_set(tables, 0, k, n.wrapping_sub(1));
                    for c in 1..=COLUMNS {
                        remove_row(tables, column(k, c), r);
                    }
                    Some(r)
                }
                _ => Some(-1),
            }
        }
        // (K, r): without checking r and the count; never a push (B-5).
        0x111 => {
            let r = t.pop();
            let k = t.pop();
            if let Some(n) = row_count(tables, k) {
                table_set(tables, 0, k, n.wrapping_sub(1));
                for c in 1..=COLUMNS {
                    remove_row(tables, column(k, c), r);
                }
            }
            None
        }
        // (K, v2, v1) -> 1 if a row has column 1 = v1 and column 2 = v2 (reverse of
        // 0x10F, B-5).
        0x13C => {
            let v1 = t.pop();
            let v2 = t.pop();
            let k = t.pop();
            let found = row_count(tables, k).is_some_and(|n| row_exists(tables, k, n, v1, v2));
            Some(i32::from(found))
        }
        _ => None,
    };
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `m_arrayAttacker` (`0x15E`-`0x165`, `0x169`) and `m_arrayNotAttacker` (`0x17C`-`0x182`, `0x187`, `0x188`). The
/// original never checks the list number L; outside 0-19 it counts as invalid (C 5 A5, re/ki-vm 12.3 P7).
fn attackers(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let arity = match id {
        0x161 | 0x162 | 0x17F | 0x180 => 1,
        0x15E | 0x15F | 0x164 | 0x17C | 0x17D | 0x182 => 3,
        _ => 2,
    };
    // n for three arguments; v or i from two.
    let n = if arity == 3 { t.pop() } else { 0 };
    let x = if arity >= 2 { t.pop() } else { 0 };
    let l = t.pop();
    let pushes = matches!(id, 0x162 | 0x163 | 0x169 | 0x180 | 0x181 | 0x187 | 0x188);
    let Some(l) = usize::try_from(l).ok().filter(|&l| l < ATTACKER_LISTS) else {
        guarded(ex, t, id, "list number outside 0-19");
        if pushes {
            ex.push(t, 0);
        }
        return;
    };
    let lists = if id >= 0x17C {
        &mut ex.mem.not_attackers
    } else {
        &mut ex.mem.attackers
    };
    let Some(list) = lists.get_mut(l) else {
        // Only from a corrupted savegame with fewer than 20 lists: nothing.
        return;
    };
    let len = count(list.len());
    let mut fault = None;
    let out = match id {
        // (L, v, n): append n times, n <= 0 nothing.
        0x15E | 0x17C => {
            append(list, x, n);
            None
        }
        0x15F | 0x17D => {
            remove_first(list, x, n);
            None
        }
        // (L, v): remove all occurrences.
        0x160 | 0x17E => {
            list.retain(|&e| e != x);
            None
        }
        // (L): empty it.
        0x161 | 0x17F => {
            list.clear();
            None
        }
        0x162 | 0x180 => Some(len),
        0x163 | 0x181 => Some(count(list.iter().filter(|&&e| e == x).count())),
        // (L, v, n): empty it and fill n times.
        0x164 | 0x182 => {
            list.clear();
            append(list, x, n);
            None
        }
        // (L, i): remove only for i < length; i < 0 -> `RemoveAt(NULL)`.
        0x165 => {
            if x < len {
                match usize::try_from(x) {
                    Ok(i) => {
                        list.remove(i);
                    }
                    Err(_) => fault = Some("negative index in RemoveAt"),
                }
            }
            None
        }
        // (L, i) -> element, i >= length -> -1; i < 0 reads through NULL.
        0x169 | 0x187 => Some(if x >= len {
            -1
        } else {
            match usize::try_from(x) {
                Ok(i) => list[i],
                Err(_) => {
                    fault = Some("negative index in FindIndex");
                    0
                }
            }
        }),
        // (L, v) -> 1/0, but without a hit and v = -1 no push (C 5 A7).
        0x188 => {
            if list.contains(&x) {
                Some(1)
            } else if x != -1 {
                Some(0)
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(what) = fault {
        guarded(ex, t, id, what);
    }
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `n` times `AddTail(v)`; `n ≤ 0` nothing.
fn append(list: &mut Vec<i32>, v: i32, n: i32) {
    if let Ok(n) = usize::try_from(n) {
        list.extend(std::iter::repeat_n(v, n));
    }
}

/// `0x15F`/`0x17D`: the loop of the original. After each element `n == 0` is checked, only then counting continues: for
/// `n = 0` a matching first element removes all occurrences (n becomes -1), otherwise none; `n < 0` removes all (C 5
/// A7).
fn remove_first(list: &mut Vec<i32>, v: i32, mut n: i32) {
    let mut i = 0;
    while i < list.len() {
        if list[i] == v {
            list.remove(i);
            n = n.wrapping_sub(1);
        } else {
            i += 1;
        }
        if n == 0 {
            break;
        }
    }
}

/// `m_listBuildingQueue` and `m_listRepairerCountQueque` (`0x183`-`0x186`).
fn building_queue(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let mem = &mut *ex.mem;
    let mut fault = None;
    let out = match id {
        0x183 => Some(count(mem.building_queue.len())),
        // (b, a): b to the building list, a to the repair counters.
        0x184 => {
            let a = t.pop();
            let b = t.pop();
            mem.building_queue.push(b);
            mem.repairer_counts.push(a);
            None
        }
        // (i, &r) -> building list[i] (0 for i >= length); if it is != 0, r <- repair counter[i]. i < 0
        // throws an exception in the original.
        0x185 => {
            let addr = t.pop();
            let i = t.pop();
            let mut v = 0;
            if i < count(mem.building_queue.len()) {
                match usize::try_from(i) {
                    Ok(i) => {
                        v = mem.building_queue[i];
                        // The repair counters are never shorter during a run (C 5 A6); shorter ones from a corrupted
                        // savegame: variable unchanged.
                        if v != 0
                            && let Some(&r) = mem.repairer_counts.get(i)
                        {
                            t.vars.set(addr as u32, r);
                        }
                    }
                    Err(_) => fault = Some("negative index in GetAt"),
                }
            }
            Some(v)
        }
        // (i): remove from the building list, from the repair counters only if i < new length of the
        // building list - for the last element the lists drift apart (C 5 A6).
        0x186 => {
            let i = t.pop();
            if i < count(mem.building_queue.len()) {
                match usize::try_from(i) {
                    Ok(i) => {
                        mem.building_queue.remove(i);
                        if i < mem.building_queue.len() && i < mem.repairer_counts.len() {
                            mem.repairer_counts.remove(i);
                        }
                    }
                    Err(_) => fault = Some("negative index in RemoveAt"),
                }
            }
            None
        }
        _ => None,
    };
    if let Some(what) = fault {
        guarded(ex, t, id, what);
    }
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `m_listBuildingEinquartierbar` (`0x13D`-`0x13F`, `0x142`).
fn quarters(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let list = &mut ex.mem.quarterable;
    let mut fault = None;
    let out = match id {
        // (v): `Add`.
        0x13D => {
            let v = t.pop();
            list.push(v);
            None
        }
        // (i) -> element, i >= size -> 0; i < 0 throws in the original.
        0x13E => {
            let i = t.pop();
            let mut v = 0;
            if i < count(list.len()) {
                match usize::try_from(i) {
                    Ok(i) => v = list[i],
                    Err(_) => fault = Some("negative index in GetAt"),
                }
            }
            Some(v)
        }
        // (i): remove if i < size; i < 0 throws in the original.
        0x13F => {
            let i = t.pop();
            if i < count(list.len()) {
                match usize::try_from(i) {
                    Ok(i) => {
                        list.remove(i);
                    }
                    Err(_) => fault = Some("negative index in RemoveAt"),
                }
            }
            None
        }
        0x142 => Some(count(list.len())),
        _ => None,
    };
    if let Some(what) = fault {
        guarded(ex, t, id, what);
    }
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// Position list of builtin `id` for slot or player `p`; `None` outside the range the original checks (5 resource
/// slots, 8 players, for `0x151`/`0x152` only 5).
fn pos_list(mem: &mut EngineMem, id: u32, p: i32) -> Option<&mut PosList> {
    let (lists, limit) = match id {
        0x109..=0x10E => (&mut mem.resources, RESOURCE_SLOTS),
        0x151 => (&mut mem.buildings, TYPE_READ_PLAYERS),
        0x152 => (&mut mem.units, TYPE_READ_PLAYERS),
        0x146 | 0x148 | 0x149 | 0x14B | 0x14C | 0x14F => (&mut mem.buildings, POSITION_PLAYERS),
        _ => (&mut mem.units, POSITION_PLAYERS),
    };
    let p = usize::try_from(p).ok().filter(|&p| p < limit)?;
    lists.get_mut(p)
}

/// Position lists: resources `0x109`-`0x10E`, units and buildings `0x145`-`0x153` without `0x147`
/// (world query `0x511D`).
fn positions(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let out = match id {
        // (p) -> count, invalid 0.
        0x109 | 0x148 | 0x14A => {
            let p = t.pop();
            Some(pos_list(ex.mem, id, p).map_or(0, |l| count(l.len())))
        }
        0x10A | 0x10B | 0x14B..=0x14E => {
            put(ex, t, id);
            return;
        }
        // (p, i, &x, &y): x and y into the variables, push type (0x10C) or count; invalid 0
        // and the variables stay.
        0x10C | 0x14F | 0x150 => {
            let ay = t.pop();
            let ax = t.pop();
            let i = t.pop();
            let p = t.pop();
            let entry = pos_list(ex.mem, id, p)
                .and_then(|l| Some((l.get(usize::try_from(i).ok()?)?, l.len())));
            Some(match entry {
                Some(((x, y, kind), n)) => {
                    t.vars.set(ax as u32, x);
                    t.vars.set(ay as u32, y);
                    if id == 0x10C { kind } else { count(n) }
                }
                None => 0,
            })
        }
        // (p, i) -> type, invalid 0.
        0x10D | 0x151 | 0x152 => {
            let i = t.pop();
            let p = t.pop();
            let kind = pos_list(ex.mem, id, p)
                .and_then(|l| l.get(usize::try_from(i).ok()?))
                .map_or(0, |(_, _, kind)| kind);
            Some(kind)
        }
        // (p, i) -> new count after the removal, invalid 0.
        0x10E | 0x149 | 0x153 => {
            let i = t.pop();
            let p = t.pop();
            let n = match (pos_list(ex.mem, id, p), usize::try_from(i)) {
                (Some(l), Ok(i)) if i < l.len() => {
                    l.remove(i);
                    count(l.len())
                }
                _ => 0,
            };
            Some(n)
        }
        // (p, y, x, r): delete all entries in [x-r, x+r) x [y-r, y+r) (`SetRect`/`PtInRect`,
        // right and bottom edge excluded); no push.
        0x145 | 0x146 => {
            let r = t.pop();
            let x = t.pop();
            let y = t.pop();
            let p = t.pop();
            if let Some(l) = pos_list(ex.mem, id, p) {
                let (left, right) = (x.wrapping_sub(r), x.wrapping_add(r));
                let (top, bottom) = (y.wrapping_sub(r), y.wrapping_add(r));
                let mut i = 0;
                while let Some((ex_, ey, _)) = l.get(i) {
                    if left <= ex_ && ex_ < right && top <= ey && ey < bottom {
                        l.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
            None
        }
        _ => None,
    };
    if let Some(v) = out {
        ex.push(t, v);
    }
}

/// `0x10A`/`0x10B`/`0x14B`-`0x14E`: set an entry with the type (key) `kind`.
/// (p, kind, x, y); `0x10A`, `0x14B`, `0x14D` take x and y from variables.
fn put(ex: &mut Exec<'_>, t: &mut Thread, id: u32) {
    let mut y = t.pop();
    let mut x = t.pop();
    if matches!(id, 0x10A | 0x14B | 0x14D) {
        y = t.vars.get(y as u32);
        x = t.vars.get(x as u32);
    }
    let kind = t.pop();
    let p = t.pop();
    let base = (
        ex.mem.values.get(&BASE_X).copied(),
        ex.mem.values.get(&BASE_Y).copied(),
    );
    let pushes = match pos_list(ex.mem, id, p) {
        Some(list) => put_entry(list, id, base, x, y, kind),
        None => [Some(0), None],
    };
    for v in pushes.into_iter().flatten() {
        ex.push(t, v);
    }
}

/// x87 comparison `FSQRT(FILD a) < FSQRT(FILD b)` of two `i32` sums: a negative sum gives NaN, and every comparison
/// with NaN is false (`TEST AH,0x41`). For values < 2^31 the root is strictly monotonic, so the comparison of the sums
/// is equivalent (re/ki-vm 12.3 P11).
fn sqrt_less(a: i32, b: i32) -> bool {
    a >= 0 && b >= 0 && a < b
}

/// `a² + b²` with `IMUL`/`ADD` wrapped to 32 bits.
fn square_sum(a: i32, b: i32) -> i32 {
    a.wrapping_mul(a).wrapping_add(b.wrapping_mul(b))
}

/// Set according to the decompilation; returns up to two push values.
///
/// - `0x10A`: an existing type is overwritten **before** the check of the keys 200/201 (deviating from the
///   catalog, which covers both under "keys missing -> 0"); otherwise sorted ascending by the distance to
///   (V200, V201): inserted before the first entry whose distance is greater than the new one.
/// - `0x10B`, `0x14B`-`0x14E`: first the keys, then overwriting; the formula uses the loop index i instead of
///   V200 and swaps x/y (C 5 A3/E-31, B-4): entry `(V201 − X_i)² + (i − Y_i)²`, new `(V201 − y)² + (i − x)²`,
///   inserted before the first i whose value is smaller than the new one. `0x10B` then additionally appends
///   and pushes twice.
fn put_entry(
    list: &mut PosList,
    id: u32,
    (v200, v201): (Option<i32>, Option<i32>),
    x: i32,
    y: i32,
    kind: i32,
) -> [Option<i32>; 2] {
    let len = |l: &PosList| Some(count(l.len()));
    let n = list.len();
    let at = if id == 0x10A {
        if let Some(i) = list.find_kind(kind) {
            list.set(i, x, y, kind);
            return [len(list), None];
        }
        let (Some(bx), Some(by)) = (v200, v201) else {
            return [Some(0), None];
        };
        let new = square_sum(bx.wrapping_sub(x), by.wrapping_sub(y));
        (0..n).find(|&i| {
            let old = square_sum(bx.wrapping_sub(list.x[i]), by.wrapping_sub(list.y[i]));
            sqrt_less(new, old)
        })
    } else {
        let (Some(_), Some(by)) = (v200, v201) else {
            return [Some(0), None];
        };
        if let Some(i) = list.find_kind(kind) {
            list.set(i, x, y, kind);
            return [len(list), None];
        }
        (0..n).find(|&i| {
            let k = i as i32;
            let old = square_sum(by.wrapping_sub(list.x[i]), k.wrapping_sub(list.y[i]));
            let new = square_sum(by.wrapping_sub(y), k.wrapping_sub(x));
            sqrt_less(old, new)
        })
    };
    match at {
        Some(i) => {
            list.insert(i, x, y, kind);
            let first = len(list);
            if id == 0x10B {
                list.push(x, y, kind);
                [first, len(list)]
            } else {
                [first, None]
            }
        }
        None => {
            list.push(x, y, kind);
            [len(list), None]
        }
    }
}
