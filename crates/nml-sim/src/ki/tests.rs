//! Tests of the AI basics (`MfcMap`, variable table, list objects) and the VM.

use super::vars::{self, KiList, LIST_BUCKETS, VarTable, copy_list};
use crate::mfc::MfcMap;

/// Address from kind, type and index.
fn addr(kind: u32, ty: u32, index: u32) -> u32 {
    (kind << 24) | (ty << 16) | index
}

fn keys<V>(map: &MfcMap<V>) -> Vec<i32> {
    map.iter().map(|(k, _)| k).collect()
}

fn list_entries(list: &KiList) -> Vec<(i32, i32)> {
    list.iter().map(|(k, v)| (k, *v)).collect()
}

#[test]
fn mfc_map_iterates_buckets_ascending_newest_first() {
    let mut map = MfcMap::new(17);
    for key in 0..20 {
        map.insert(key, key * 10);
    }
    // Keys 0-15 are in bucket 0, 16-19 in bucket 1; per chain the newest first.
    let expected: Vec<i32> = (0..16).rev().chain((16..20).rev()).collect();
    assert_eq!(keys(&map), expected);
    assert_eq!(map.len(), 20);
    assert_eq!(map.bucket_of(15), 0);
    assert_eq!(map.bucket_of(16), 1);
    // HashKey is unsigned: (-1 as u32) >> 4 = 0x0FFF_FFFF, and that is 15 mod 17.
    assert_eq!(map.bucket_of(-1), 15);
    map.insert(-1, 7);
    assert_eq!(keys(&map).last(), Some(&-1));
    assert_eq!(map.first_key(), Some(15));
}

#[test]
fn mfc_map_overwrite_keeps_position_and_remove_unlinks() {
    let mut map = MfcMap::new(17);
    for key in [0, 1, 2] {
        map.insert(key, key);
    }
    assert_eq!(keys(&map), vec![2, 1, 0]);
    map.insert(1, 99);
    assert_eq!(keys(&map), vec![2, 1, 0]);
    assert_eq!(map.get(1), Some(&99));

    assert_eq!(map.remove(1), Some(99));
    assert_eq!(map.remove(1), None);
    assert_eq!(keys(&map), vec![2, 0]);
    assert!(!map.contains_key(1));
    assert_eq!(map.len(), 2);

    // A key inserted again later is at the front again.
    map.insert(1, 5);
    assert_eq!(keys(&map), vec![1, 2, 0]);

    *map.get_mut(0).unwrap() += 1;
    assert_eq!(map.get(0), Some(&1));
    map.clear();
    assert!(map.is_empty());
    assert_eq!(map.bucket_count(), 17);
}

#[test]
fn entry_or_default_inserts_at_the_head_once() {
    let mut map: MfcMap<i32> = MfcMap::new(10);
    *map.entry_or_default(3) += 4;
    *map.entry_or_default(5) += 1;
    *map.entry_or_default(3) += 1;
    assert_eq!(keys(&map), vec![5, 3]);
    assert_eq!(map.get(3), Some(&5));
}

#[test]
fn next_key_follows_the_chain_then_the_next_bucket() {
    let mut map = MfcMap::new(17);
    for key in [0, 1, 16] {
        map.insert(key, 0);
    }
    // Bucket 0: 1 -> 0; bucket 1: 16.
    assert_eq!(map.first_key(), Some(1));
    assert_eq!(map.next_key(1), Some(0));
    assert_eq!(map.next_key(0), Some(16));
    assert_eq!(map.next_key(16), None);
    assert_eq!(map.next_key(99), None);

    // An insertion before the remembered position does not change the successor.
    map.insert(2, 0);
    assert_eq!(map.next_key(1), Some(0));
    assert_eq!(map.next_key(2), Some(1));

    let empty: MfcMap<i32> = MfcMap::new(17);
    assert_eq!(empty.first_key(), None);
}

#[test]
fn mfc_map_consistency_check_and_serde_roundtrip() {
    let mut map = MfcMap::new(10);
    for key in [-5, 0, 7, 160, 161, 4000] {
        map.insert(key, key + 1);
    }
    assert_eq!(map.check(10), Ok(()));
    assert!(map.check(17).is_err());

    let bytes = postcard::to_stdvec(&map).unwrap();
    let back: MfcMap<i32> = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, map);
    assert_eq!(keys(&back), keys(&map));

    // Corrupted bytes (postcard: bucket count, per bucket length and entries as zigzag numbers).
    // Key 16 belongs in bucket 1, not in bucket 0.
    let wrong_bucket: MfcMap<i32> = postcard::from_bytes(&[2, 1, 0x20, 0, 0]).unwrap();
    assert!(wrong_bucket.check(2).is_err());
    // The same key twice in a chain.
    let duplicate: MfcMap<i32> = postcard::from_bytes(&[2, 2, 0, 0, 0, 0, 0]).unwrap();
    assert!(duplicate.check(2).is_err());
    let fine: MfcMap<i32> = postcard::from_bytes(&[2, 1, 0, 0, 0]).unwrap();
    assert_eq!(fine.check(2), Ok(()));
}

#[test]
fn address_parts_and_range_keys() {
    let a = addr(2, 3, 17);
    assert_eq!(vars::kind(a), 2);
    assert_eq!(vars::ty(a), 3);
    assert_eq!(vars::index(a), 17);
    assert_eq!(vars::range_key(a), Some(23));
    assert_eq!(vars::range_key(addr(0, 9, 0)), Some(9));
    assert_eq!(vars::range_key(addr(15, 9, 0)), Some(159));
    assert_eq!(vars::range_key(addr(1, 10, 0)), None);
}

// QUIRK E-1: kept
#[test]
fn vars_outside_ranges_read_zero_and_ignore_writes() {
    let mut table = VarTable::from_module(&[(addr(0, 0, 0), 3)], &[(addr(0, 0, 1), 42)]);
    assert_eq!(table.get(addr(0, 0, 1)), 42);

    // Type >= 10, index >= length, missing range: read 0, write nothing.
    for bad in [
        addr(0, 10, 0),
        addr(0, 0, 3),
        addr(0, 0, 0xFFFF),
        addr(0, 1, 0),
    ] {
        assert_eq!(table.get(bad), 0);
        assert_eq!(table.resolve(bad), 0);
        assert_eq!(table.load(bad), 0);
        let before = table.clone();
        table.set(bad, 7);
        table.store(bad, 7);
        assert_eq!(table, before);
    }

    table.set(addr(0, 0, 2), -5);
    assert_eq!(table.get(addr(0, 0, 2)), -5);
    assert_eq!(table.load(addr(0, 0, 2)), -5);
    assert_eq!(table.resolve(addr(0, 0, 2)), -5);
    table.store(addr(0, 0, 0), 9);
    assert_eq!(table.get(addr(0, 0, 0)), 9);
}

// QUIRK E-1: kept
#[test]
fn references_into_the_caller_are_dead() {
    // Kind 1 (reference, type 0, length 2) and kind 3 (list reference, type 3, length 1).
    let mut table = VarTable::from_module(&[(addr(1, 0, 0), 2), (addr(3, 3, 0), 1)], &[]);
    let r = addr(1, 0, 1);
    let lr = addr(3, 3, 0);

    // E-1: opcode 0 and 1 follow the reference into the (missing) caller.
    table.set(r, 11); // set writes into its own cell
    assert_eq!(table.get(r), 11);
    assert_eq!(table.load(r), 0);
    assert_eq!(table.resolve(r), 0);
    table.store(r, 5);
    assert_eq!(table.get(r), 11);

    // Kind 3: no list object, set writes nothing, list returns nothing.
    assert!(table.list(lr).is_none());
    assert!(table.list_mut(lr).is_none());
    assert_eq!(table.load(lr), 0);
    let before = table.clone();
    table.set(lr, 3);
    table.store(lr, 3);
    assert_eq!(table, before);
    assert_eq!(table.get(lr), 0);
}

#[test]
fn list_variables_load_their_address_and_store_copies() {
    let a = addr(2, 0, 0);
    let b = addr(2, 0, 1);
    let var = addr(0, 0, 0);
    let mut table = VarTable::from_module(&[(a, 2), (var, 1)], &[]);

    // re/ki-vm 12.3 P2: reading and loading return the own address; it resolves to the list again.
    assert_eq!(table.load(a), a as i32);
    assert_eq!(table.load(b), b as i32);
    assert_eq!(table.get(a), a as i32);
    assert_eq!(table.resolve(a), a as i32);
    assert!(table.list(a).is_some());
    assert_eq!(table.list(a).unwrap().bucket_count(), LIST_BUCKETS);

    // set does not write to lists.
    table.set(a, 5);
    assert_eq!(table.get(a), a as i32);

    {
        let list = table.list_mut(a).unwrap();
        for k in [3, 14, 40, 41] {
            list.insert(k, k * 2);
        }
    }
    // STORE b <- a: content copied in MFC order (merging, nothing deleted).
    table.list_mut(b).unwrap().insert(100, 1);
    table.store(b, a as i32);
    let mut expected = KiList::new(LIST_BUCKETS);
    expected.insert(100, 1);
    copy_list(table.list(a).unwrap(), &mut expected);
    assert_eq!(
        list_entries(table.list(b).unwrap()),
        list_entries(&expected)
    );
    assert!(table.list(b).unwrap().contains_key(100));
    assert_eq!(table.list(b).unwrap().get(41), Some(&82));
    assert_eq!(table.list(a).unwrap().len(), 4);

    // Copy onto itself changes nothing.
    let before = table.clone();
    table.store(a, a as i32);
    assert_eq!(table, before);

    // An ordinary variable may hold the list address; list resolves it.
    table.set(var, a as i32);
    assert_eq!(
        list_entries(table.list(var).unwrap()),
        list_entries(table.list(a).unwrap())
    );
    // Source invalid (address without a list) -> nothing.
    let before = table.clone();
    table.store(b, addr(0, 0, 5) as i32);
    table.store(b, 0);
    assert_eq!(table, before);
}

#[test]
fn constdat_skips_lists_and_indices_out_of_range() {
    let list = addr(2, 0, 0);
    let table = VarTable::from_module(
        &[
            (addr(0, 0, 0), 2),
            (addr(0, 1, 0), 0),  // length 0: no range
            (addr(0, 10, 0), 4), // type >= 10: ignored
            (addr(0, 2, 0), 1),
            (addr(0, 2, 0), 3), // duplicate ID: the last one wins
            (list, 1),
        ],
        &[
            (addr(0, 0, 0), 1),
            (addr(0, 0, 1), 2),
            (addr(0, 0, 2), 3), // index >= length
            (addr(0, 1, 0), 4), // no range
            (addr(0, 10, 0), 5),
            (addr(0, 2, 2), 6),
            (list, 7),          // list
            (addr(1, 0, 0), 8), // range missing
        ],
    );
    assert_eq!(
        table.ranges.keys().copied().collect::<Vec<_>>(),
        vec![0, 2, 20]
    );
    assert_eq!(table.ranges[&0].values, vec![1, 2]);
    assert_eq!(table.ranges[&2].values, vec![0, 0, 6]);
    assert!(table.ranges[&0].lists.is_empty());
    assert_eq!(table.ranges[&20].lists.len(), 1);
    assert!(table.list(list).unwrap().is_empty());
}

#[test]
fn var_table_survives_a_serde_round_trip() {
    let ranges = [(addr(0, 0, 0), 3), (addr(2, 2, 0), 2)];
    let mut a = VarTable::from_module(&ranges, &[(addr(0, 0, 1), 5)]);
    a.list_mut(addr(2, 2, 1)).unwrap().insert(1, 1);
    let bytes = postcard::to_stdvec(&a).unwrap();
    let back: VarTable = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, a);
}

// QUIRK E-45: kept
#[test]
fn address_bits_28_to_31_are_ignored_like_in_the_original() {
    // FUN_00413840/FUN_004137f0 mask the kind with 0xF (`AND EAX,0xF`); "kind < 16" is always true.
    let mut table = VarTable::from_module(&[(addr(0, 0, 0), 6), (addr(2, 0, 0), 1)], &[]);
    let plain = addr(0, 0, 5);
    let high = plain | 0x1000_0000;
    table.set(plain, 7);
    assert_eq!(table.get(high), 7);
    assert_eq!(table.load(high), 7);
    assert_eq!(table.resolve(high), 7);
    table.set(high, 9);
    assert_eq!(table.get(plain), 9);
    table.store(plain | 0xF000_0000, 11);
    assert_eq!(table.get(plain), 11);
    assert_eq!(vars::kind(high), 0);
    assert_eq!(vars::range_key(high), Some(0));

    // Also lists: the address is returned unchanged (with the high bits).
    let list = addr(2, 0, 0);
    assert_eq!(table.get(list | 0x2000_0000), (list | 0x2000_0000) as i32);
    assert!(table.list(list | 0x2000_0000).is_some());
}

// ---------------------------------------------------------------------------------------------
// VM: opcodes, step, CALLM/SPAWN, error cases
// ---------------------------------------------------------------------------------------------

use super::builtins::{self, has_case};
use super::catalog::{KiCatalog, KiPrototype};
use super::engine::EngineMem;
use super::testkit::{self, TestWorld, int, word};
use super::vm::{self, Effect, Exec, Thread, op, status};
use super::world::{KiLimit, KiNote, Peers};
use super::{MAX_CALL_DEPTH, MAX_STACK, MAX_THREADS};
use crate::rng::CrtRand;

/// Address "discard" for `STORE`/`STORE3`.
const DROP: u32 = 0x0FFF_FFFF;
/// Step size of a pass in the tick (20 / 3).
const DT: i32 = 6;

/// Test rig: catalog, randomness, test world and engine memory for single VM steps.
struct Rig {
    catalog: KiCatalog,
    rand: CrtRand,
    world: TestWorld,
    mem: EngineMem,
    next_id: i32,
    thread_count: usize,
    slot: u8,
}

impl Rig {
    fn new(protos: Vec<KiPrototype>) -> Self {
        Self {
            catalog: testkit::catalog(protos),
            rand: CrtRand::new(1),
            world: TestWorld::default(),
            mem: EngineMem::default(),
            next_id: 1,
            thread_count: 1,
            slot: 0,
        }
    }

    /// Thread 0 of the module `module` on the slot of the test rig.
    fn thread(&self, module: u32, stack: Vec<i32>) -> Thread {
        let p = self.catalog.get(module).expect("test module missing");
        Thread::from_prototype(p, 0, self.slot, stack)
    }

    /// One step in a pass (thread in the table, depth 0); returns status and effects.
    fn step(&mut self, t: &mut Thread) -> (u32, Vec<Effect>) {
        let mut ex = Exec {
            catalog: &self.catalog,
            rand: &mut self.rand,
            world: &mut self.world,
            slot: self.slot,
            mem: &mut self.mem,
            next_thread_id: &mut self.next_id,
            thread_count: self.thread_count,
            top_in_table: true,
            effects: Vec::new(),
            peers: Peers::none(),
        };
        let s = vm::step(&mut ex, t, DT, 0);
        self.thread_count = ex.thread_count;
        (s, ex.effects)
    }
}

/// Module with the code `code` and the integer variables `0..8` with the initial values `inits`.
fn module(id: u32, inits: &[(u32, i32)], code: &[u32]) -> KiPrototype {
    testkit::proto(id, &[(0, 8)], inits, code)
}

/// A `CALLB id` on the stack `stack`; returns the stack afterwards and the anomalies.
fn calc(id: u32, stack: &[i32]) -> (Vec<i32>, Vec<KiNote>) {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::CALLB, id)])]);
    let mut t = rig.thread(1, stack.to_vec());
    rig.step(&mut t);
    assert!(
        rig.world.calls.is_empty(),
        "Builtin {id:#x} went to the world"
    );
    (t.stack, rig.world.notes)
}

/// Result of `a ∘ b` for the builtin `id`, without anomaly.
fn binary(id: u32, a: i32, b: i32) -> i32 {
    let (stack, notes) = calc(id, &[a, b]);
    assert_eq!(stack.len(), 1, "Builtin {id:#x}: (a, b) -> one value");
    assert!(notes.is_empty(), "Builtin {id:#x}: {notes:?}");
    stack[0]
}

#[test]
fn opcodes_follow_the_original() {
    // x = int(0), c5 = int(1) = 5, y = int(2), c0 = int(3) = 0.
    let code = [
        word(op::LOAD, int(1)),   // 0: [5]
        word(op::STORE, int(0)),  // 1: x = 5
        word(op::PUSHA, int(1)),  // 2: [1]
        word(op::STORE3, int(2)), // 3: y = 1 (STORE3 does not resolve after PUSHA)
        word(0xA, 1),             // 4-9: ops 10-15 have no effect
        word(0xB, 2),
        word(0xC, 3),
        word(0xD, 4),
        word(0xE, 5),
        word(0xF, DROP),
        word(op::LOAD, int(2)),  // 10: [1]
        word(op::JZ, 20),        // 11: no jump
        word(op::LOAD, int(3)),  // 12: [0]
        word(op::JNZ, 20),       // 13: no jump
        word(op::LOAD, int(2)),  // 14: [1]
        word(op::JNZ, 17),       // 15: jump
        word(op::STORE, int(0)), // 16: skipped
        word(op::LOAD, int(3)),  // 17: [0]
        word(op::JZ, 21),        // 18: jump
        word(op::STORE, int(0)), // 19, 20: skipped
        word(op::STORE, int(0)),
        word(op::JMP, 0), // 21
    ];
    let mut rig = Rig::new(vec![module(1, &[(int(1), 5), (int(3), 0)], &code)]);
    let mut t = rig.thread(1, Vec::new());
    assert_eq!((t.pc, t.last_op, t.status), (0, -1, status::RUNNING));
    // (pc afterwards, last_op, stack)
    let expected: [(u32, i32, &[i32]); 19] = [
        (1, 0, &[5]),
        (2, 1, &[]),
        (3, 2, &[1]),
        (4, 3, &[]),
        (5, 10, &[]),
        (6, 11, &[]),
        (7, 12, &[]),
        (8, 13, &[]),
        (9, 14, &[]),
        (10, 15, &[]),
        (11, 0, &[1]),
        (12, 8, &[]),
        (13, 0, &[0]),
        (14, 9, &[]),
        (15, 0, &[1]),
        (17, 9, &[]),
        (18, 0, &[0]),
        (21, 8, &[]),
        (0, 7, &[]),
    ];
    for (i, (pc, last, stack)) in expected.into_iter().enumerate() {
        let (s, effects) = rig.step(&mut t);
        assert_eq!((t.pc, t.last_op), (pc, last), "step {}", i + 1);
        assert_eq!(t.stack, stack, "step {}", i + 1);
        assert_eq!(s, status::RUNNING);
        assert!(effects.is_empty());
    }
    assert_eq!(t.vars.get(int(0)), 5);
    assert_eq!(t.vars.get(int(2)), 1);
    assert!(rig.world.calls.is_empty() && rig.world.notes.is_empty());
}

// QUIRK E-2: kept
#[test]
fn store_right_after_pusha_resolves_the_address() {
    // Reference variable &rx = kind 1, index 0; x = int(0), y = int(1).
    let rx = 0x0100_0000;
    let code = [
        word(op::PUSHA, rx),
        word(op::STORE, int(0)),
        word(op::PUSHA, rx),
        word(op::STORE, DROP),
        word(op::PUSHA, rx),
        word(op::STORE3, int(1)),
    ];
    let mut rig = Rig::new(vec![testkit::proto(1, &[(0, 4), (rx, 2)], &[], &code)]);
    let mut t = rig.thread(1, Vec::new());
    t.vars.set(rx, 7);
    rig.step(&mut t);
    rig.step(&mut t);
    assert_eq!(
        t.vars.get(int(0)),
        7,
        "STORE after PUSHA stores the value, not the address"
    );
    rig.step(&mut t);
    rig.step(&mut t);
    assert!(t.stack.is_empty(), "STORE drop discards");
    assert_eq!((t.vars.get(int(0)), t.vars.get(rx)), (7, 7));
    rig.step(&mut t);
    rig.step(&mut t);
    assert_eq!(t.vars.get(int(1)), rx as i32, "STORE3 stores the address");
}

// QUIRK E-3: kept
#[test]
fn empty_stack_pops_zero() {
    let code = [word(op::CALLB, 1), word(op::STORE, int(0)), word(op::JZ, 0)];
    let mut rig = Rig::new(vec![module(1, &[(int(0), 9)], &code)]);
    let mut t = rig.thread(1, Vec::new());
    rig.step(&mut t);
    assert_eq!(t.stack, [0]);
    rig.step(&mut t);
    assert_eq!(t.vars.get(int(0)), 0);
    rig.step(&mut t);
    assert_eq!(t.pc, 0, "JZ on an empty stack jumps");
}

// QUIRK E-4: kept
#[test]
fn builtins_without_case_clear_the_stack() {
    assert_eq!((1..=757).filter(|&id| has_case(id)).count(), 502);
    for id in [1, 0x258, 0x2EE, 0x2F5] {
        assert!(has_case(id), "{id:#x}");
    }
    for id in [0, 9, 0x2B, 0x1C0, 0x2ED, 758, 0x0FFF_FFFF] {
        assert!(!has_case(id), "{id:#x}");
        let mut rig = Rig::new(vec![module(1, &[], &[word(op::CALLB, id)])]);
        rig.world.pops = 1;
        let mut t = rig.thread(1, vec![1, 2, 3]);
        rig.step(&mut t);
        assert!(t.stack.is_empty(), "{id:#x}");
        assert!(rig.world.calls.is_empty(), "{id:#x}");
        assert_eq!(t.last_op, op::CALLB as i32);
    }
}

#[test]
fn world_builtins_get_the_running_instance() {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::CALLB, 600)])]);
    rig.slot = 3;
    rig.world.pops = 2;
    rig.world.answer = 42;
    let mut t = rig.thread(1, vec![1, 2, 3]);
    t.id = 17;
    assert!(!builtins::is_local(600));
    rig.step(&mut t);
    assert_eq!(rig.world.calls, [(3, 17, 600, vec![2, 3])]);
    assert_eq!(t.stack, [1, 42]);
}

// QUIRK E-5: deviation
#[test]
fn arithmetic_wraps_and_guards_division() {
    assert_eq!(binary(0x01, 2, 3), 5);
    assert_eq!(binary(0x01, i32::MAX, 1), i32::MIN);
    assert_eq!(binary(0x02, 2, 3), -1);
    assert_eq!(binary(0x02, i32::MIN, 1), i32::MAX);
    assert_eq!(binary(0x03, 0x10000, 0x10000), 0);
    assert_eq!(binary(0x03, -4, 3), -12);
    assert_eq!(binary(0x04, 7, 2), 3);
    assert_eq!(binary(0x04, -7, 2), -3);
    assert_eq!(binary(0x05, 0b1100, 0b1010), 0b1000);
    assert_eq!(binary(0x06, 0b1100, 0b1010), 0b1110);
    assert_eq!(binary(0x07, 0b1100, 0b1010), 0b0110);
    assert_eq!(binary(0x08, 7, 3), 1);
    assert_eq!(binary(0x08, -7, 2), -1);
    assert_eq!(binary(0x14, 2, 3), 1);
    assert_eq!(binary(0x14, 2, 0), 0);
    assert_eq!(binary(0x15, 0, 0), 0);
    assert_eq!(binary(0x15, 0, -5), 1);

    // `/` by 0 calculates with 1 like the original, without anomaly.
    assert_eq!(binary(0x04, 7, 0), 7);

    // Crash cases of the original (E-5): fixed result and `Guarded` (re/ki-vm 12.3 P7).
    for (id, a, b, result) in [
        (0x04, i32::MIN, -1, i32::MIN),
        (0x08, 7, 0, 0),
        (0x08, i32::MIN, -1, 0),
    ] {
        let (stack, notes) = calc(id, &[a, b]);
        assert_eq!(stack, [result], "{id:#x}: {a} {b}");
        let [
            KiNote::Guarded {
                slot: 0,
                thread: 0,
                module: 1,
                builtin,
                ..
            },
        ] = notes.as_slice()
        else {
            panic!("{id:#x}: a Guarded note expected: {notes:?}");
        };
        assert_eq!(u32::from(*builtin), id);
    }

    // `!` and pop on an empty stack (= 0).
    assert_eq!(calc(0x16, &[0]).0, [1]);
    assert_eq!(calc(0x16, &[9, -3]).0, [9, 0]);
    assert_eq!(calc(0x02, &[4]).0, [-4]);
}

#[test]
fn comparisons_pop_b_then_a() {
    // (ID, 5 <op> 3, 3 <op> 5, 3 <op> 3)
    for (id, gt, lt, eq) in [
        (0x0A, 1, 0, 0),
        (0x0B, 0, 1, 0),
        (0x0C, 0, 0, 1),
        (0x0D, 1, 1, 0),
        (0x0E, 1, 0, 1),
        (0x0F, 0, 1, 1),
        (0x118, 0, 0, 1),
        (0x119, 0, 0, 1),
        (0x11A, 0, 0, 1),
        (0x11B, 1, 1, 0),
        (0x11C, 1, 1, 0),
        (0x11D, 1, 1, 0),
    ] {
        assert_eq!(binary(id, 5, 3), gt, "{id:#x}: 5 <op> 3");
        assert_eq!(binary(id, 3, 5), lt, "{id:#x}: 3 <op> 5");
        assert_eq!(binary(id, 3, 3), eq, "{id:#x}: 3 <op> 3");
    }
    let ids = [0x01..=0x08, 0x0A..=0x0F, 0x14..=0x16, 0x118..=0x11D];
    for id in ids.into_iter().flatten() {
        assert!(builtins::is_local(id) && has_case(id), "{id:#x}");
    }
}

// QUIRK E-7: kept
#[test]
fn callm_passes_the_whole_stack_and_returns_the_rest() {
    // Child: x = int(0), c9 = int(1) = 9; "STORE x; LOAD c9".
    let child = module(
        2,
        &[(int(1), 9)],
        &[word(op::STORE, int(0)), word(op::LOAD, int(1))],
    );
    let parent = module(
        1,
        &[(int(0), 4)],
        &[word(op::CALLM, 2), word(op::LOAD, int(0))],
    );
    let mut rig = Rig::new(vec![parent, child]);
    rig.slot = 3;
    let mut t = rig.thread(1, vec![1, 2, 3]);
    t.id = 17;

    rig.step(&mut t);
    assert!(t.stack.is_empty(), "the whole stack goes to the child");
    assert_eq!(t.status, status::RUNNING | status::CHILD);
    assert_eq!(t.depth(), 1);
    let c = t.child.as_deref().expect("child missing");
    assert_eq!((c.id, c.player, c.module, c.stop), (17, 3, 2, false));
    assert_eq!(c.stack, [1, 2, 3]);
    assert_eq!((c.pc, c.last_op, c.status), (0, -1, status::RUNNING));

    rig.step(&mut t);
    rig.step(&mut t);
    let c = t.child.as_deref().expect("child missing");
    assert_eq!(c.stack, [1, 2, 9]);
    assert_eq!(c.vars.get(int(0)), 3);
    assert_eq!((t.pc, t.last_op), (1, op::CALLM as i32), "the parent waits");

    // Return step: child at end of code, append the rest, no parent instruction.
    let (s, _) = rig.step(&mut t);
    assert_eq!((s, t.status), (status::RUNNING, status::RUNNING));
    assert!(t.child.is_none());
    assert_eq!(t.depth(), 0);
    assert_eq!(t.stack, [1, 2, 9]);
    assert_eq!((t.pc, t.last_op), (1, op::CALLM as i32));

    rig.step(&mut t);
    assert_eq!(t.stack, [1, 2, 9, 4]);

    // The child inherits `stop`.
    let mut t = rig.thread(1, Vec::new());
    t.stop = true;
    rig.step(&mut t);
    assert!(t.child.as_deref().expect("child missing").stop);
}

// QUIRK E-6: deviation
#[test]
fn callm_to_a_missing_module_does_nothing() {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::CALLM, 99)])]);
    let mut t = rig.thread(1, vec![1, 2]);
    t.id = 5;
    let (s, effects) = rig.step(&mut t);
    assert_eq!((s, t.status), (status::RUNNING, status::RUNNING));
    assert!(t.child.is_none() && effects.is_empty());
    assert_eq!(t.stack, [1, 2]);
    assert_eq!((t.pc, t.last_op), (1, op::CALLM as i32));
    assert_eq!(
        rig.world.notes,
        [KiNote::MissingModule {
            slot: 0,
            thread: 5,
            module: 1,
            target: 99,
            spawn: false,
        }]
    );
}

// QUIRK E-7: kept
#[test]
fn spawn_copies_the_stack_and_the_caller_keeps_it() {
    let mut rig = Rig::new(vec![
        module(1, &[], &[word(op::SPAWN, 2), word(op::STORE, DROP)]),
        module(2, &[(int(0), 6)], &[word(op::LOAD, int(0))]),
    ]);
    rig.slot = 2;
    rig.next_id = 7;
    let mut t = rig.thread(1, vec![4, 5]);
    t.player = 5;
    let (s, effects) = rig.step(&mut t);
    assert_eq!(s, status::RUNNING);
    assert_eq!(t.stack, [4, 5], "the caller keeps its arguments");
    assert_eq!((rig.next_id, rig.thread_count), (8, 2));
    let [Effect::Spawn(new)] = effects.as_slice() else {
        panic!("a spawn expected: {effects:?}");
    };
    let p = rig.catalog.get(2).expect("module 2");
    assert_eq!((new.id, new.player, new.module), (7, 2, 2));
    assert_eq!(new.stack, [4, 5]);
    assert_eq!((new.pc, new.status, new.last_op), (0, status::RUNNING, -1));
    assert_eq!((new.code_len, new.checksum), (1, p.checksum));
    assert_eq!(new.vars, p.vars);
    assert!(new.child.is_none() && !new.stop);
    assert_eq!((new.pause_ms, new.wait_semaphore), (0, -1));
    assert!(rig.world.notes.is_empty());
}

// QUIRK E-6: deviation
#[test]
fn spawn_of_a_missing_module_is_silent() {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::SPAWN, 99)])]);
    rig.next_id = 7;
    let mut t = rig.thread(1, vec![4, 5]);
    let (_, effects) = rig.step(&mut t);
    assert!(effects.is_empty());
    assert_eq!(t.stack, [4, 5]);
    assert_eq!((rig.next_id, rig.thread_count), (7, 1));
    assert_eq!(
        rig.world.notes,
        [KiNote::MissingModule {
            slot: 0,
            thread: 0,
            module: 1,
            target: 99,
            spawn: true,
        }]
    );
}

#[test]
fn spawn_over_the_thread_limit_starts_nothing() {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::SPAWN, 1)])]);
    rig.next_id = 7;
    rig.thread_count = MAX_THREADS;
    let mut t = rig.thread(1, Vec::new());
    let (_, effects) = rig.step(&mut t);
    assert!(effects.is_empty());
    assert_eq!(
        (rig.next_id, rig.thread_count),
        (7, MAX_THREADS),
        "no ID consumed"
    );
    assert_eq!(
        rig.world.notes,
        [KiNote::Limit {
            slot: 0,
            thread: 0,
            module: 1,
            limit: KiLimit::Threads,
        }]
    );
}

#[test]
fn code_end_finishes_the_thread() {
    let mut rig = Rig::new(vec![
        module(1, &[], &[word(op::JMP, 2), word(op::LOAD, int(0))]),
        module(2, &[], &[]),
        module(3, &[], &[word(op::JMP, DROP)]),
    ]);
    let mut t = rig.thread(1, Vec::new());
    assert_eq!(rig.step(&mut t).0, status::RUNNING);
    assert_eq!(t.pc, 2);
    assert_eq!(rig.step(&mut t).0, status::DONE);
    assert_eq!((t.status, t.last_op), (status::DONE, op::JMP as i32));

    let mut t = rig.thread(2, Vec::new());
    assert_eq!(rig.step(&mut t).0, status::DONE);
    assert_eq!(t.last_op, -1, "empty code ends without an instruction");

    let mut t = rig.thread(3, Vec::new());
    rig.step(&mut t);
    assert_eq!(rig.step(&mut t).0, status::DONE, "jump far beyond the end");

    // Prototype missing at run time (conceivable only after a corrupted load).
    let mut t = rig.thread(1, Vec::new());
    t.module = 99;
    assert_eq!(rig.step(&mut t).0, status::DONE);
    assert_eq!(t.pc, 0);
}

#[test]
fn stack_limit_drops_values_and_notes() {
    let code = [word(op::PUSHA, 1), word(op::PUSHA, 2), word(op::CALLB, 600)];
    let mut rig = Rig::new(vec![module(1, &[], &code)]);
    rig.world.answer = 3;
    let mut t = rig.thread(1, vec![0; MAX_STACK - 1]);
    rig.step(&mut t);
    assert_eq!(t.stack.len(), MAX_STACK);
    assert!(rig.world.notes.is_empty());
    assert!(!t.push(9));
    assert_eq!(t.stack.len(), MAX_STACK);

    let limit = KiNote::Limit {
        slot: 0,
        thread: 0,
        module: 1,
        limit: KiLimit::Stack,
    };
    rig.step(&mut t);
    assert_eq!((t.stack.len(), t.stack[MAX_STACK - 1]), (MAX_STACK, 1));
    assert_eq!(rig.world.notes, std::slice::from_ref(&limit));

    // A world builtin pushes beyond the limit: value discarded, note after the call.
    rig.step(&mut t);
    assert_eq!(rig.world.calls.len(), 1);
    assert_eq!((t.stack.len(), t.stack[MAX_STACK - 1]), (MAX_STACK, 1));
    assert_eq!(rig.world.notes, [limit.clone(), limit]);
}

#[test]
fn callm_depth_limit_notes_and_continues() {
    let mut rig = Rig::new(vec![module(1, &[], &[word(op::CALLM, 1)])]);
    let mut t = rig.thread(1, vec![8]);
    t.id = 4;
    for _ in 0..MAX_CALL_DEPTH {
        rig.step(&mut t);
    }
    assert_eq!(t.depth(), MAX_CALL_DEPTH);
    assert!(rig.world.notes.is_empty());

    let (s, _) = rig.step(&mut t);
    assert_eq!(s, status::RUNNING | status::CHILD);
    assert_eq!(t.depth(), MAX_CALL_DEPTH, "beyond the depth limit: nothing");
    assert_eq!(
        rig.world.notes,
        [KiNote::Limit {
            slot: 0,
            thread: 4,
            module: 1,
            limit: KiLimit::CallDepth,
        }]
    );

    // The deepest child ends; the thread continues and unwinds the chain.
    rig.step(&mut t);
    assert_eq!(t.depth(), MAX_CALL_DEPTH - 1);
    for _ in 0..(2 * MAX_CALL_DEPTH) {
        rig.step(&mut t);
    }
    assert_eq!(t.status, status::DONE);
    assert_eq!(t.stack, [8], "the stack travels back through all levels");
}

// QUIRK E-42: kept
#[test]
fn step_takes_semaphores_and_counts_pauses_down() {
    let code = [word(op::PUSHA, 1), word(op::PUSHA, 2), word(op::PUSHA, 3)];
    let mut rig = Rig::new(vec![module(1, &[], &code)]);
    let mut t = rig.thread(1, Vec::new());

    // re/ki-vm 12.3 P3: occupied semaphore -> wait; free -> take over and continue in the same step.
    t.wait_semaphore = 5;
    rig.mem.semaphores.insert(5, 1);
    assert_eq!(rig.step(&mut t).0, status::RUNNING);
    assert_eq!((t.pc, t.wait_semaphore), (0, 5));
    rig.mem.semaphores.insert(5, 0);
    rig.step(&mut t);
    assert_eq!((t.pc, t.wait_semaphore), (1, -1));
    assert_eq!(rig.mem.semaphores.get(&5), Some(&1));

    // Pause: subtract `dt` per step as long as it is > 0; no instruction.
    t.pause_ms = 10;
    rig.step(&mut t);
    assert_eq!((t.pc, t.pause_ms), (1, 10 - DT));
    rig.step(&mut t);
    assert_eq!((t.pc, t.pause_ms), (1, 10 - 2 * DT));
    rig.step(&mut t);
    assert_eq!((t.pc, t.pause_ms), (2, 10 - 2 * DT));
}

#[test]
fn a_stopped_thread_with_a_child_runs_its_own_code() {
    // FUN_004b58c0: the child step only for `!stop`; otherwise pause or own instruction.
    let mut rig = Rig::new(vec![
        module(1, &[], &[word(op::CALLM, 2), word(op::PUSHA, 7)]),
        module(2, &[], &[word(op::PUSHA, 9)]),
    ]);
    let mut t = rig.thread(1, Vec::new());
    rig.step(&mut t);
    t.stop = true;
    rig.step(&mut t);
    assert_eq!(t.stack, [7]);
    assert_eq!(t.status, status::RUNNING | status::CHILD);
    assert!(t.child.as_deref().expect("child missing").stack.is_empty());
}
