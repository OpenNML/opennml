//! Tests of the builtins for the engine memory (`builtins::memory`): values and jump marks, tables,
//! global list, attacker lists, building queue, quarter list and position lists. Expectations from
//! re/ki-builtins and the decompilation of `FUN_004ad380`.

use std::collections::BTreeMap;

use super::builtins;
use super::catalog::KiCatalog;
use super::engine::{EngineMem, PosList};
use super::testkit::{self, TestWorld, int};
use super::vm::{Exec, Thread};
use super::world::{KiNote, Peers};
use crate::rng::CrtRand;

/// Reference variable 0 (kind 1, type 0): `get`/`set` hit the own cell (without reference).
pub(super) const REF0: u32 = 1 << 24;

/// Test rig: a thread (integer variables `0..8`, two reference variables) and the
/// engine memory; each builtin runs directly through the dispatcher.
pub(super) struct Rig {
    catalog: KiCatalog,
    pub(super) rand: CrtRand,
    world: TestWorld,
    mem: EngineMem,
    next_id: i32,
    pub(super) thread: Thread,
}

impl Rig {
    pub(super) fn new() -> Self {
        Self::with_ranges(&[(0, 8), (REF0, 2)])
    }

    /// Test rig with own variable ranges `(address, length)`.
    pub(super) fn with_ranges(ranges: &[(u32, u32)]) -> Self {
        let catalog = testkit::catalog(vec![testkit::proto(1, ranges, &[], &[])]);
        let p = catalog.get(1).expect("test module");
        let thread = Thread::from_prototype(p, 0, 0, Vec::new());
        Self {
            catalog,
            rand: CrtRand::new(1),
            world: TestWorld::default(),
            mem: EngineMem::new(),
            next_id: 1,
            thread,
        }
    }

    /// `CALLB id` with the stack `args` (in source order); returns what is on the stack
    /// afterwards.
    pub(super) fn call(&mut self, id: u32, args: &[i32]) -> Vec<i32> {
        self.thread.stack = args.to_vec();
        let mut ex = Exec {
            catalog: &self.catalog,
            rand: &mut self.rand,
            world: &mut self.world,
            slot: 0,
            mem: &mut self.mem,
            next_thread_id: &mut self.next_id,
            thread_count: 1,
            top_in_table: true,
            effects: Vec::new(),
            peers: Peers::none(),
        };
        builtins::call(&mut ex, &mut self.thread, id, 0);
        assert!(ex.effects.is_empty(), "Builtin {id:#x} with effect");
        assert!(
            self.world.calls.is_empty(),
            "Builtin {id:#x} went to the world"
        );
        std::mem::take(&mut self.thread.stack)
    }

    pub(super) fn var(&self, addr: u32) -> i32 {
        self.thread.vars.get(addr)
    }

    pub(super) fn set_var(&mut self, addr: u32, v: i32) {
        self.thread.vars.set(addr, v);
    }

    /// IDs of the crash cases caught so far (re/ki-vm 12.3 P7), cleared afterwards; other notes are an
    /// error.
    pub(super) fn guarded(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.world.notes)
            .into_iter()
            .map(|n| match n {
                KiNote::Guarded { builtin, .. } => builtin,
                other => panic!("unexpected note {other:?}"),
            })
            .collect()
    }
}

fn pos(entries: &[(i32, i32, i32)]) -> PosList {
    let mut list = PosList::default();
    for &(x, y, kind) in entries {
        list.push(x, y, kind);
    }
    list
}

fn entries(list: &PosList) -> Vec<(i32, i32, i32)> {
    (0..list.len()).filter_map(|i| list.get(i)).collect()
}

fn table(entries: &[(i32, i32)]) -> BTreeMap<i32, i32> {
    entries.iter().copied().collect()
}

#[test]
fn engine_mem_default_is_new() {
    let mem = EngineMem::default();
    assert_eq!(mem, EngineMem::new());
    assert_eq!(
        (
            mem.attackers.len(),
            mem.not_attackers.len(),
            mem.resources.len(),
            mem.units.len(),
            mem.buildings.len()
        ),
        (20, 20, 5, 8, 8)
    );
}

#[test]
fn values_and_goto_builtins() {
    let mut r = Rig::new();
    // 0x1F: missing -> 0, nothing created.
    assert_eq!(r.call(0x1F, &[5]), [0]);
    assert!(r.mem.values.is_empty());
    // 0x1E (k, v) and 0x1F.
    assert_eq!(r.call(0x1E, &[3, 9]), []);
    assert_eq!(r.call(0x1F, &[3]), [9]);
    // 0x1B (k, &v) reads the variable without a reference: the cell of the reference variable itself.
    r.set_var(REF0, 42);
    assert_eq!(r.call(0x1B, &[7, REF0 as i32]), []);
    assert_eq!(r.mem.values.get(&7), Some(&42));
    r.set_var(int(2), -6);
    assert_eq!(r.call(0x1B, &[8, int(2) as i32]), []);
    assert_eq!(r.mem.values.get(&8), Some(&-6));

    // 0x54: missing -> 1 and created; 0 -> 1; otherwise the value.
    assert_eq!(r.call(0x54, &[4]), [1]);
    assert_eq!(r.mem.goto.get(&4), Some(&1));
    r.mem.goto.insert(4, 0);
    assert_eq!(r.call(0x54, &[4]), [1]);
    assert_eq!(r.mem.goto.get(&4), Some(&1));
    r.mem.goto.insert(4, 5);
    assert_eq!(r.call(0x54, &[4]), [5]);
    // 0xDB deletes, nothing if missing.
    assert_eq!(r.call(0xDB, &[4]), []);
    assert_eq!(r.call(0xDB, &[4]), []);
    assert!(r.mem.goto.is_empty());

    // 0x128/0x129 (delta, key): only if the key exists, 32-bit wraps.
    assert_eq!(r.call(0x128, &[2, 3]), []);
    assert_eq!(r.mem.values.get(&3), Some(&11));
    assert_eq!(r.call(0x129, &[5, 3]), []);
    assert_eq!(r.mem.values.get(&3), Some(&6));
    assert_eq!(r.call(0x128, &[1, 99]), []);
    assert_eq!(r.call(0x129, &[1, 99]), []);
    assert!(!r.mem.values.contains_key(&99));
    r.mem.values.insert(3, i32::MAX);
    r.call(0x128, &[1, 3]);
    assert_eq!(r.mem.values.get(&3), Some(&i32::MIN));
    r.call(0x129, &[1, 3]);
    assert_eq!(r.mem.values.get(&3), Some(&i32::MAX));

    // 0x1BF (start, end, v): key - start of the first hit in start ... end-1, otherwise -1.
    r.mem.values = table(&[(10, 7), (12, 7), (13, 8)]);
    assert_eq!(r.call(0x1BF, &[10, 14, 8]), [3]);
    assert_eq!(r.call(0x1BF, &[11, 14, 7]), [1]);
    assert_eq!(r.call(0x1BF, &[10, 13, 8]), [-1]);
    assert_eq!(r.call(0x1BF, &[13, 13, 8]), [-1]);
    assert_eq!(r.call(0x1BF, &[14, 10, 8]), [-1]);
    assert_eq!(
        r.call(0x1BF, &[i32::MIN, i32::MAX, 7]),
        [10i32.wrapping_sub(i32::MIN)]
    );
    assert!(r.guarded().is_empty());
}

#[test]
fn table_builtins_follow_b5() {
    let mut r = Rig::new();
    // 0x24 creates table and entry; 0x26/0x29/0x2C are the same.
    assert_eq!(r.call(0x24, &[3, 1, 10]), []);
    assert_eq!(r.call(0x2C, &[3, 2, 20]), []);
    assert_eq!(r.mem.tables.get(&3), Some(&table(&[(1, 10), (2, 20)])));
    // 0x25 reads, missing -> 0 without creating.
    assert_eq!(r.call(0x25, &[3, 2]), [20]);
    assert_eq!(r.call(0x25, &[4, 1]), [0]);
    assert_eq!(r.call(0x25, &[3, 9]), [0]);
    assert!(!r.mem.tables.contains_key(&4));
    // 0x27/0x2A/0x2D discard the top argument.
    assert_eq!(r.call(0x27, &[3, 1, 99]), [10]);
    assert_eq!(r.call(0x2D, &[3, 2, 99]), [20]);

    // 0xBA-0xBD (t, k): remove entry, larger keys move up, value or 0.
    r.mem
        .tables
        .insert(5, table(&[(-2, 1), (0, 2), (1, 3), (2, 4), (4, 5)]));
    assert_eq!(r.call(0xBA, &[5, 1]), [3]);
    assert_eq!(
        r.mem.tables.get(&5),
        Some(&table(&[(-2, 1), (0, 2), (1, 4), (3, 5)]))
    );
    assert_eq!(r.call(0xBD, &[5, 9]), [0]);
    assert_eq!(r.call(0xBB, &[6, 0]), [0]);
    assert!(!r.mem.tables.contains_key(&6));
    // The last row removed: the table stays empty (new CMap).
    r.mem.tables.insert(7, table(&[(0, 8)]));
    assert_eq!(r.call(0xBC, &[7, 0]), [8]);
    assert_eq!(r.mem.tables.get(&7), Some(&BTreeMap::new()));

    // 0x10F (K, v1...v7): append row, push index; row count in table 0.
    let mut r = Rig::new();
    assert_eq!(r.call(0x10F, &[2, 11, 12, 13, 14, 15, 16, 17]), [0]);
    assert_eq!(r.call(0x10F, &[2, 21, 22, 23, 24, 25, 26, 27]), [1]);
    assert_eq!(r.mem.tables.get(&0), Some(&table(&[(2, 2)])));
    for c in 1..=7 {
        assert_eq!(
            r.mem.tables.get(&(2 + 10_000 * c)),
            Some(&table(&[(0, 10 + c), (1, 20 + c)])),
            "column {c}"
        );
    }

    // 0x13C (K, v2, v1): column 1 = v1 (pushed last), column 2 = v2 - reverse of 0x10F.
    assert_eq!(r.call(0x13C, &[2, 22, 21]), [1]);
    assert_eq!(r.call(0x13C, &[2, 21, 22]), [0]);
    assert_eq!(r.call(0x13C, &[9, 22, 21]), [0]);

    // 0x110 (K, r, &a1...&a7): read and delete row r, push r.
    let vars: Vec<i32> = (0..7).map(|i| int(i) as i32).collect();
    let args = |k: i32, row: i32| [&[k, row][..], &vars[..]].concat();
    assert_eq!(r.call(0x110, &args(2, 0)), [0]);
    assert_eq!(
        (0..7).map(|i| r.var(int(i))).collect::<Vec<_>>(),
        [11, 12, 13, 14, 15, 16, 17]
    );
    assert_eq!(r.mem.tables.get(&0), Some(&table(&[(2, 1)])));
    assert_eq!(r.mem.tables.get(&10_002), Some(&table(&[(0, 21)])));
    // re/ki-builtins B-5: row 5 is missing - variables are written anyway (with 0) and the count drops.
    assert_eq!(r.call(0x110, &args(2, 5)), [5]);
    assert_eq!((0..7).map(|i| r.var(int(i))).collect::<Vec<_>>(), [0; 7]);
    assert_eq!(r.mem.tables.get(&0), Some(&table(&[(2, 0)])));
    assert_eq!(r.mem.tables.get(&10_002), Some(&table(&[(0, 21)])));
    // Count 0: -1, but the variables are already written - row 0 is unreachable.
    assert_eq!(r.call(0x110, &args(2, 0)), [-1]);
    assert_eq!(
        (0..7).map(|i| r.var(int(i))).collect::<Vec<_>>(),
        [21, 22, 23, 24, 25, 26, 27]
    );
    assert_eq!(r.call(0x13C, &[2, 22, 21]), [0]);

    // 0x111 (K, r): checks nothing, the count may go negative, never a push.
    assert_eq!(r.call(0x111, &[2, 0]), []);
    assert_eq!(r.mem.tables.get(&0), Some(&table(&[(2, -1)])));
    assert_eq!(r.mem.tables.get(&10_002), Some(&BTreeMap::new()));
    assert_eq!(r.call(0x111, &[3, 0]), []);
    assert_eq!(r.mem.tables.get(&0), Some(&table(&[(2, -1)])));

    // Without table 0: 0x110 -> -1, 0x111 nothing, 0x13C -> 0.
    let mut r = Rig::new();
    assert_eq!(r.call(0x110, &args(2, 0)), [-1]);
    assert_eq!(r.call(0x111, &[2, 0]), []);
    assert_eq!(r.call(0x13C, &[2, 0, 0]), [0]);
    assert!(r.mem.tables.is_empty());
    assert!(r.guarded().is_empty());
}

#[test]
fn table_search_0x13c_matches_the_original_loop() {
    // The search does not run row by row but over the entries; checked against the loop of the
    // original - including missing rows (read 0) and a huge row count.
    let col1 = table(&[(0, 5), (1, 0), (2, 5), (4, 7)]);
    let col2 = table(&[(0, 6), (2, 0), (3, 9), (4, 0)]);
    for count in [-3, 0, 1, 2, 3, 4, 5, 6] {
        for (v1, v2) in [(5, 6), (5, 0), (0, 0), (0, 9), (7, 0), (7, 6), (1, 1)] {
            let mut r = Rig::new();
            r.mem.tables.insert(0, table(&[(4, count)]));
            r.mem.tables.insert(10_004, col1.clone());
            r.mem.tables.insert(20_004, col2.clone());
            let get = |m: &BTreeMap<i32, i32>, i| m.get(&i).copied().unwrap_or(0);
            let expected = (0..count).any(|i| get(&col1, i) == v1 && get(&col2, i) == v2);
            assert_eq!(
                r.call(0x13C, &[4, v2, v1]),
                [i32::from(expected)],
                "count {count}, ({v1}, {v2})"
            );
        }
    }
    let mut r = Rig::new();
    r.mem.tables.insert(0, table(&[(4, i32::MAX)]));
    assert_eq!(r.call(0x13C, &[4, 0, 0]), [1]);
    assert_eq!(r.call(0x13C, &[4, 0, 3]), [0]);
}

#[test]
fn global_list_builtins() {
    let mut r = Rig::new();
    // Empty: 0x20 -> 0, 0x21 nothing, 0x28 -> 0.
    assert_eq!(r.call(0x20, &[]), [0]);
    assert_eq!(r.call(0x21, &[]), []);
    assert_eq!(r.call(0x28, &[]), [0]);
    // 0x22 does not accept 0.
    for x in [0, 5, 7, 5] {
        assert_eq!(r.call(0x22, &[x]), []);
    }
    assert_eq!(r.mem.global_list, [5, 7, 5]);
    assert_eq!(r.call(0x28, &[]), [3]);
    assert_eq!(r.call(0x18, &[7]), [1]);
    assert_eq!(r.call(0x18, &[9]), [0]);
    assert_eq!(r.call(0x1D, &[5]), [2]);
    assert_eq!(r.call(0x1D, &[0]), [0]);
    assert_eq!(r.call(0x20, &[]), [5]);
    assert_eq!(r.call(0x21, &[]), []);
    assert_eq!(r.mem.global_list, [7, 5]);
    // 0x23 (v, i): only i < count; i < 0 is an exception in the original.
    assert_eq!(r.call(0x23, &[9, 2]), []);
    assert_eq!(r.mem.global_list, [7, 5]);
    assert_eq!(r.call(0x23, &[9, 1]), []);
    assert_eq!(r.mem.global_list, [7, 9, 5]);
    assert!(r.guarded().is_empty());
    assert_eq!(r.call(0x23, &[9, -1]), []);
    assert_eq!(r.mem.global_list, [7, 9, 5]);
    assert_eq!(r.guarded(), [0x23]);

    // 0x1C (n): entries with type in [0xAF0, 0xD48) among the first min(n, count).
    r.mem.global_list = vec![0xAF0, 0xD47, 0xD48, 0xAEF, 0xB00];
    assert_eq!(r.call(0x1C, &[3]), [2]);
    assert_eq!(r.call(0x1C, &[100]), [3]);
    assert_eq!(r.call(0x1C, &[0]), [0]);
    assert_eq!(r.call(0x1C, &[-1]), [0]);
    r.mem.global_list.clear();
    assert_eq!(r.call(0x1C, &[3]), [0]);
    assert!(r.guarded().is_empty());
}

// QUIRK E-34: deviation
#[test]
fn attacker_lists_keep_the_original_balance() {
    let mut r = Rig::new();
    // 0x15E (L, v, n): append n times, n <= 0 nothing.
    assert_eq!(r.call(0x15E, &[3, 8, 3]), []);
    assert_eq!(r.call(0x15E, &[3, 9, 0]), []);
    assert_eq!(r.call(0x15E, &[3, 9, -2]), []);
    assert_eq!(r.mem.attackers[3], [8, 8, 8]);
    r.call(0x15E, &[3, 9, 1]);
    r.call(0x15E, &[3, 8, 1]);
    assert_eq!(r.call(0x162, &[3]), [5]);
    assert_eq!(r.call(0x163, &[3, 8]), [4]);

    // 0x15F (L, v, n): n = 0 removes all if the first element matches, otherwise none.
    assert_eq!(r.call(0x15F, &[3, 8, 0]), []);
    assert_eq!(r.mem.attackers[3], [9]);
    r.mem.attackers[3] = vec![9, 8, 8];
    assert_eq!(r.call(0x15F, &[3, 8, 0]), []);
    assert_eq!(r.mem.attackers[3], [9, 8, 8]);
    // n < 0: all; n > 0: the first n.
    assert_eq!(r.call(0x15F, &[3, 8, -1]), []);
    assert_eq!(r.mem.attackers[3], [9]);
    r.mem.attackers[3] = vec![8, 9, 8, 8];
    assert_eq!(r.call(0x15F, &[3, 8, 2]), []);
    assert_eq!(r.mem.attackers[3], [9, 8]);

    // 0x169 (L, i): element, i >= length -> -1, i < 0 -> 0 with Guarded (NULL access).
    assert_eq!(r.call(0x169, &[3, 1]), [8]);
    assert_eq!(r.call(0x169, &[3, 2]), [-1]);
    assert!(r.guarded().is_empty());
    assert_eq!(r.call(0x169, &[3, -1]), [0]);
    assert_eq!(r.guarded(), [0x169]);
    // 0x165 (L, i): remove only for i < length; i < 0 -> Guarded, no push.
    assert_eq!(r.call(0x165, &[3, 5]), []);
    assert_eq!(r.call(0x165, &[3, -1]), []);
    assert_eq!(r.guarded(), [0x165]);
    assert_eq!(r.mem.attackers[3], [9, 8]);
    assert_eq!(r.call(0x165, &[3, 0]), []);
    assert_eq!(r.mem.attackers[3], [8]);
    // 0x164 clears and fills, 0x160 removes all occurrences, 0x161 clears.
    assert_eq!(r.call(0x164, &[3, 4, 2]), []);
    assert_eq!(r.mem.attackers[3], [4, 4]);
    r.call(0x15E, &[3, 5, 1]);
    assert_eq!(r.call(0x160, &[3, 4]), []);
    assert_eq!(r.mem.attackers[3], [5]);
    assert_eq!(r.call(0x161, &[3]), []);
    assert!(r.mem.attackers[3].is_empty());

    // The non-attackers (0x17C-0x182, 0x187, 0x188) go to the other list.
    assert_eq!(r.call(0x17C, &[2, 4, 1]), []);
    assert_eq!(r.call(0x182, &[1, 6, 2]), []);
    assert_eq!(r.mem.not_attackers[2], [4]);
    assert_eq!(r.mem.not_attackers[1], [6, 6]);
    assert!(r.mem.attackers.iter().all(Vec::is_empty));
    assert_eq!(r.call(0x180, &[1]), [2]);
    assert_eq!(r.call(0x181, &[1, 6]), [2]);
    assert_eq!(r.call(0x187, &[1, 1]), [6]);
    assert_eq!(r.call(0x187, &[1, 2]), [-1]);
    r.call(0x17D, &[1, 6, 1]);
    assert_eq!(r.mem.not_attackers[1], [6]);
    r.call(0x17E, &[1, 6]);
    r.call(0x17F, &[2]);
    assert!(r.mem.not_attackers.iter().all(Vec::is_empty));

    // 0x188 (L, v): 1/0 - except v = -1 without a hit: no push (re/ki-builtins C 5 A7).
    r.mem.not_attackers[2] = vec![4, -1];
    assert_eq!(r.call(0x188, &[2, 4]), [1]);
    assert_eq!(r.call(0x188, &[2, 5]), [0]);
    assert_eq!(r.call(0x188, &[2, -1]), [1]);
    assert_eq!(r.call(0x188, &[3, -1]), []);
    assert!(r.guarded().is_empty());

    // L outside 0-19 (re/ki-builtins C 5 A5, re/ki-vm 12.3 P7): Guarded; what otherwise pushes, pushes 0, otherwise
    // nothing changes.
    let before = r.mem.clone();
    assert_eq!(r.call(0x162, &[20]), [0]);
    assert_eq!(r.call(0x169, &[-1, 0]), [0]);
    assert_eq!(r.call(0x188, &[20, -1]), [0]);
    assert_eq!(r.call(0x15E, &[20, 1, 1]), []);
    assert_eq!(r.call(0x161, &[20]), []);
    assert_eq!(r.mem, before);
    assert_eq!(r.guarded(), [0x162, 0x169, 0x188, 0x15E, 0x161]);
}

// QUIRK E-34: deviation
#[test]
fn building_queue_lists_drift_apart_like_a6() {
    let mut r = Rig::new();
    // 0x184 (b, a): b to the building queue, a to the repairer counts.
    for (b, a) in [(100, 1), (200, 2), (300, 3)] {
        assert_eq!(r.call(0x184, &[b, a]), []);
    }
    assert_eq!(r.call(0x183, &[]), [3]);
    // 0x186 at the last element: the repairer count stays.
    assert_eq!(r.call(0x186, &[2]), []);
    assert_eq!(r.mem.building_queue, [100, 200]);
    assert_eq!(r.mem.repairer_counts, [1, 2, 3]);
    r.call(0x184, &[400, 4]);
    // 0x185 (i, &r) then reads the shifted value: 3 instead of 4.
    let out = int(0) as i32;
    assert_eq!(r.call(0x185, &[2, out]), [400]);
    assert_eq!(r.var(int(0)), 3);
    // Value 0 in the building queue: variable unchanged; i >= length -> 0.
    r.mem.building_queue[1] = 0;
    r.set_var(int(0), -9);
    assert_eq!(r.call(0x185, &[1, out]), [0]);
    assert_eq!(r.call(0x185, &[4, out]), [0]);
    assert_eq!(r.var(int(0)), -9);
    // 0x186 in the middle removes from both lists.
    assert_eq!(r.call(0x186, &[0]), []);
    assert_eq!(r.mem.building_queue, [0, 400]);
    assert_eq!(r.mem.repairer_counts, [2, 3, 4]);
    assert_eq!(r.call(0x186, &[9]), []);
    assert!(r.guarded().is_empty());
    // i < 0: C++ exception in the original -> Guarded; 0x185 pushes 0, 0x186 nothing.
    assert_eq!(r.call(0x185, &[-1, out]), [0]);
    assert_eq!(r.call(0x186, &[-1]), []);
    assert_eq!(r.guarded(), [0x185, 0x186]);
    assert_eq!(r.mem.building_queue, [0, 400]);
}

// QUIRK E-34: deviation
#[test]
fn quarter_list_builtins() {
    let mut r = Rig::new();
    assert_eq!(r.call(0x13D, &[5]), []);
    assert_eq!(r.call(0x13D, &[6]), []);
    assert_eq!(r.call(0x142, &[]), [2]);
    assert_eq!(r.call(0x13E, &[1]), [6]);
    assert_eq!(r.call(0x13E, &[2]), [0]);
    assert_eq!(r.call(0x13F, &[2]), []);
    assert_eq!(r.call(0x13F, &[0]), []);
    assert_eq!(r.mem.quarterable, [6]);
    assert!(r.guarded().is_empty());
    assert_eq!(r.call(0x13E, &[-1]), [0]);
    assert_eq!(r.call(0x13F, &[-1]), []);
    assert_eq!(r.guarded(), [0x13E, 0x13F]);
    assert_eq!(r.mem.quarterable, [6]);
}

// QUIRK E-31: kept
#[test]
fn position_lists_sort_with_the_loop_index() {
    let mut r = Rig::new();
    let (vx, vy) = (int(0), int(1));
    let put = |r: &mut Rig, id: u32, p: i32, key: i32, x: i32, y: i32| {
        r.set_var(vx, x);
        r.set_var(vy, y);
        r.call(id, &[p, key, vx as i32, vy as i32])
    };
    // Without the keys 200 and 201: push 0, nothing entered.
    assert_eq!(put(&mut r, 0x14B, 2, 100, 3, 4), [0]);
    r.mem.values.insert(200, 7);
    assert_eq!(put(&mut r, 0x14B, 2, 100, 3, 4), [0]);
    assert!(r.mem.buildings[2].is_empty());
    // E-31, re/ki-builtins C 5 A3 with V200 = 7 (read, never used) and V201 = 10: entry i has
    // (V201 - X_i)^2 + (i - Y_i)^2, the new one (V201 - y)^2 + (i - x)^2; inserted before the first
    // i whose value is smaller than the new one.
    r.mem.values.insert(201, 10);
    // Empty -> append.
    assert_eq!(put(&mut r, 0x14B, 2, 100, 3, 4), [1]);
    // (1, 1): i = 0 new 81 + 1 = 82 > old 49 + 16 = 65 -> front.
    assert_eq!(put(&mut r, 0x14B, 2, 101, 1, 1), [2]);
    // (5, 9): i = 0 new 1 + 25 = 26 <= 82, i = 1 new 1 + 16 = 17 <= 49 + 9 = 58 -> back.
    assert_eq!(put(&mut r, 0x14B, 2, 102, 5, 9), [3]);
    // (-3, 3): i = 0 new 49 + 9 = 58 <= 82, i = 1 new 49 + 16 = 65 > 58 -> before entry 1.
    assert_eq!(put(&mut r, 0x14B, 2, 103, -3, 3), [4]);
    assert_eq!(
        entries(&r.mem.buildings[2]),
        [(1, 1, 101), (-3, 3, 103), (3, 4, 100), (5, 9, 102)]
    );
    // Existing key: only overwrite, the order stays.
    assert_eq!(put(&mut r, 0x14B, 2, 100, 50, 60), [4]);
    assert_eq!(r.mem.buildings[2].get(2), Some((50, 60, 100)));
    // 0x14C with immediate values calculates the same: (0, 0) -> i = 0 new 100 + 0 > 82 -> front.
    assert_eq!(r.call(0x14C, &[2, 104, 0, 0]), [5]);
    assert_eq!(r.mem.buildings[2].get(0), Some((0, 0, 104)));
    // 0x14D/0x14E go to the unit lists.
    assert_eq!(put(&mut r, 0x14D, 7, 1, 3, 4), [1]);
    assert_eq!(r.call(0x14E, &[7, 2, 1, 1]), [2]);
    assert_eq!(entries(&r.mem.units[7]), [(1, 1, 2), (3, 4, 1)]);
    // Player outside 0-7: 0.
    assert_eq!(put(&mut r, 0x14B, 8, 1, 0, 0), [0]);
    assert_eq!(r.call(0x14E, &[-1, 1, 0, 0]), [0]);

    // 0x151 (p, i) checks p against 5 (E-30): player 5 returns 0 although the list has entries.
    r.mem.buildings[5] = pos(&[(1, 2, 77)]);
    r.mem.units[5] = pos(&[(1, 2, 78)]);
    assert_eq!(r.call(0x148, &[5]), [1]);
    assert_eq!(r.call(0x151, &[5, 0]), [0]);
    assert_eq!(r.call(0x152, &[5, 0]), [0]);
    assert_eq!(r.call(0x151, &[2, 2]), [103]);
    assert_eq!(r.call(0x152, &[7, 1]), [0]);
    r.mem.units[4] = pos(&[(0, 0, 9)]);
    assert_eq!(r.call(0x152, &[4, 0]), [9]);
    assert_eq!(r.call(0x151, &[2, 6]), [0]);
    assert_eq!(r.call(0x151, &[2, -1]), [0]);
    assert!(r.guarded().is_empty());
}

// QUIRK E-31: kept
#[test]
fn distance_sums_wrap_like_the_x87() {
    let mut r = Rig::new();
    r.mem.values.insert(200, 0);
    r.mem.values.insert(201, 0);
    // New value negative: (0 - (-50000))^2 wraps to -1,794,967,296 -> NaN -> never insert.
    assert_eq!(r.call(0x14C, &[1, 1, 0, 0]), [1]);
    assert_eq!(r.call(0x14C, &[1, 2, 0, -50_000]), [2]);
    assert_eq!(entries(&r.mem.buildings[1]), [(0, 0, 1), (0, -50_000, 2)]);
    // Old value negative: (0 - 50000)^2 wraps; the new one (0) is not "greater" than NaN.
    assert_eq!(r.call(0x14E, &[1, 1, 50_000, 0]), [1]);
    assert_eq!(r.call(0x14E, &[1, 2, 0, 0]), [2]);
    assert_eq!(entries(&r.mem.units[1]), [(50_000, 0, 1), (0, 0, 2)]);
    // Without wrapping it would be the other way round: (0, 30000) new 9*10^8 > old 0 -> front.
    assert_eq!(r.call(0x14C, &[3, 1, 0, 0]), [1]);
    assert_eq!(r.call(0x14C, &[3, 2, 0, 30_000]), [2]);
    assert_eq!(entries(&r.mem.buildings[3]), [(0, 30_000, 2), (0, 0, 1)]);
    // Extreme values run without a debug panic.
    r.mem.values.insert(201, i32::MIN);
    assert_eq!(r.call(0x14C, &[4, 1, i32::MAX, i32::MAX]), [1]);
    assert_eq!(r.call(0x14C, &[4, 2, i32::MIN, i32::MIN]), [2]);
    r.mem.values.insert(200, i32::MAX);
    assert_eq!(r.call(0x10A, &[0, 1, int(0) as i32, int(1) as i32]), [1]);
    r.set_var(int(0), i32::MIN);
    assert_eq!(r.call(0x10A, &[0, 2, int(0) as i32, int(1) as i32]), [2]);
    let pushed = r.call(0x10B, &[0, 3, i32::MAX, i32::MIN]);
    assert_eq!(pushed.last(), Some(&(r.mem.resources[0].len() as i32)));
}

// QUIRK E-31, E-48: kept
#[test]
fn resource_lists_sort_by_the_base_and_0x10b_pushes_twice() {
    let mut r = Rig::new();
    let (vx, vy) = (int(0), int(1));
    let put = |r: &mut Rig, i: i32, kind: i32, x: i32, y: i32| {
        r.set_var(vx, x);
        r.set_var(vy, y);
        r.call(0x10A, &[i, kind, vx as i32, vy as i32])
    };
    // Without keys 200/201: new type -> 0, nothing entered.
    assert_eq!(put(&mut r, 1, 10, 3, 4), [0]);
    assert!(r.mem.resources[1].is_empty());
    // 0x10A sorts correctly ascending by the distance to (V200, V201) = (0, 0);
    // tie -> behind the existing ones.
    r.mem.values.insert(200, 0);
    r.mem.values.insert(201, 0);
    assert_eq!(put(&mut r, 1, 10, 3, 4), [1]); // 25
    assert_eq!(put(&mut r, 1, 11, 1, 0), [2]); // 1 -> front
    assert_eq!(put(&mut r, 1, 12, 0, 5), [3]); // 25 -> behind (3, 4)
    assert_eq!(put(&mut r, 1, 13, 2, 2), [4]); // 8 -> before (3, 4)
    assert_eq!(
        entries(&r.mem.resources[1]),
        [(1, 0, 11), (2, 2, 13), (3, 4, 10), (0, 5, 12)]
    );
    // Finding: 0x10A overwrites an existing type even before the keys are checked.
    r.mem.values.clear();
    assert_eq!(put(&mut r, 1, 13, 9, 9), [4]);
    assert_eq!(r.mem.resources[1].get(1), Some((9, 9, 13)));
    assert_eq!(put(&mut r, 1, 14, 9, 9), [0]);
    // Slot outside 0-4: 0.
    assert_eq!(put(&mut r, 5, 1, 0, 0), [0]);

    // 0x10B (i, type, x, y): keys needed before overwriting; formula like 0x14B; after the
    // insertion additionally appended and pushed twice (re/ki-builtins B-4).
    let mut r = Rig::new();
    r.mem.resources[0] = pos(&[(3, 4, 1)]);
    assert_eq!(r.call(0x10B, &[0, 1, 7, 7]), [0]);
    assert_eq!(entries(&r.mem.resources[0]), [(3, 4, 1)]);
    r.mem.values.insert(200, 0);
    r.mem.values.insert(201, 10);
    assert_eq!(r.call(0x10B, &[0, 1, 7, 7]), [1]);
    assert_eq!(entries(&r.mem.resources[0]), [(7, 7, 1)]);
    // (1, 1): i = 0 new (10 - 1)^2 + (0 - 1)^2 = 82 > old (10 - 7)^2 + (0 - 7)^2 = 58 -> front and back.
    assert_eq!(r.call(0x10B, &[0, 2, 1, 1]), [2, 3]);
    assert_eq!(
        entries(&r.mem.resources[0]),
        [(1, 1, 2), (7, 7, 1), (1, 1, 2)]
    );
    // (9, 9): i = 0 new 1 + 81 = 82, not greater than 82; i = 1 new 1 + 64 = 65 > 9 + 36 = 45
    // -> before entry 1, appended as well.
    assert_eq!(r.call(0x10B, &[0, 3, 9, 9]), [4, 5]);
    assert_eq!(
        entries(&r.mem.resources[0]),
        [(1, 1, 2), (9, 9, 3), (7, 7, 1), (1, 1, 2), (9, 9, 3)]
    );
    assert!(r.guarded().is_empty());
}

// QUIRK E-30: kept
#[test]
fn position_list_reads_and_removals() {
    let mut r = Rig::new();
    r.mem.resources[2] = pos(&[(1, 2, 10), (3, 4, 11)]);
    // 0x109 (i): count, slot outside -> 0.
    assert_eq!(r.call(0x109, &[2]), [2]);
    assert_eq!(r.call(0x109, &[5]), [0]);
    assert_eq!(r.call(0x109, &[-1]), [0]);
    // 0x10C (i, k, &x, &y): write variables, push type; invalid -> 0, variables stay.
    let (vx, vy) = (int(0) as i32, int(1) as i32);
    assert_eq!(r.call(0x10C, &[2, 1, vx, vy]), [11]);
    assert_eq!((r.var(int(0)), r.var(int(1))), (3, 4));
    assert_eq!(r.call(0x10C, &[2, 2, vx, vy]), [0]);
    assert_eq!(r.call(0x10C, &[2, -1, vx, vy]), [0]);
    assert_eq!((r.var(int(0)), r.var(int(1))), (3, 4));
    // 0x10D (i, k) and 0x10E (i, k).
    assert_eq!(r.call(0x10D, &[2, 0]), [10]);
    assert_eq!(r.call(0x10D, &[2, 2]), [0]);
    assert_eq!(r.call(0x10E, &[2, 0]), [1]);
    assert_eq!(entries(&r.mem.resources[2]), [(3, 4, 11)]);
    assert_eq!(r.call(0x10E, &[2, 1]), [0]);

    // Buildings: 0x148, 0x149, 0x14F; units: 0x14A, 0x150, 0x153.
    r.mem.buildings[6] = pos(&[(1, 1, 5), (2, 2, 6), (3, 3, 7)]);
    r.mem.units[0] = pos(&[(4, 4, 8)]);
    assert_eq!(r.call(0x148, &[6]), [3]);
    assert_eq!(r.call(0x148, &[8]), [0]);
    assert_eq!(r.call(0x14A, &[0]), [1]);
    assert_eq!(r.call(0x14A, &[-1]), [0]);
    assert_eq!(r.call(0x14F, &[6, 2, vx, vy]), [3]);
    assert_eq!((r.var(int(0)), r.var(int(1))), (3, 3));
    assert_eq!(r.call(0x14F, &[6, 3, vx, vy]), [0]);
    assert_eq!(r.call(0x150, &[0, 0, vx, vy]), [1]);
    assert_eq!((r.var(int(0)), r.var(int(1))), (4, 4));
    assert_eq!(r.call(0x150, &[8, 0, vx, vy]), [0]);
    assert_eq!(r.call(0x149, &[6, 1]), [2]);
    assert_eq!(entries(&r.mem.buildings[6]), [(1, 1, 5), (3, 3, 7)]);
    assert_eq!(r.call(0x149, &[6, 2]), [0]);
    assert_eq!(r.call(0x153, &[0, 0]), [0]);
    assert!(r.mem.units[0].is_empty());
    assert_eq!(r.call(0x153, &[0, 0]), [0]);

    // 0x145/0x146 (p, y, x, r): delete all entries in [x-r, x+r) x [y-r, y+r) (PtInRect).
    r.mem.units[1] = pos(&[
        (8, 18, 1),
        (12, 22, 2),
        (11, 20, 3),
        (7, 20, 4),
        (11, 21, 5),
    ]);
    assert_eq!(r.call(0x145, &[1, 20, 10, 2]), []);
    assert_eq!(entries(&r.mem.units[1]), [(12, 22, 2), (7, 20, 4)]);
    r.mem.buildings[1] = pos(&[(0, 0, 1), (5, 5, 2)]);
    assert_eq!(r.call(0x146, &[1, 0, 0, 1]), []);
    assert_eq!(entries(&r.mem.buildings[1]), [(5, 5, 2)]);
    // r <= 0 gives an empty rectangle; p outside nothing.
    assert_eq!(r.call(0x146, &[1, 5, 5, 0]), []);
    assert_eq!(r.call(0x146, &[8, 5, 5, 3]), []);
    assert_eq!(entries(&r.mem.buildings[1]), [(5, 5, 2)]);
    assert!(r.guarded().is_empty());
}

#[test]
fn builtin_0x147_between_the_position_lists_goes_to_the_world() {
    // 0x147 (id) -> 0/1 is a query (request 0x511D), not a memory builtin: it pops its
    // argument in the world and pushes its answer.
    let mut r = Rig::new();
    r.world.pops = 1;
    r.world.answer = 1;
    r.thread.stack = vec![7, 42];
    let mut ex = Exec {
        catalog: &r.catalog,
        rand: &mut r.rand,
        world: &mut r.world,
        slot: 0,
        mem: &mut r.mem,
        next_thread_id: &mut r.next_id,
        thread_count: 1,
        top_in_table: true,
        effects: Vec::new(),
        peers: Peers::none(),
    };
    builtins::call(&mut ex, &mut r.thread, 0x147, 0);
    assert_eq!(r.world.calls, [(0, 0, 0x147, vec![42])]);
    assert_eq!(r.thread.stack, [7, 1]);
    assert_eq!(r.mem, EngineMem::new());
}
