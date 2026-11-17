//! Tests of the list, randomness and other VM builtins: `builtins::lists`, `0x8D`
//! (`builtins::flow`), `0x17`, `0x82`, `0x89`, `0x91`, `0xB6`, `0xBE`, `0xD1` (`builtins::arith`)
//! as well as `vars::{remove_index, insert_index}`. Expectations from re/ki-builtins and the
//! decompilations of `FUN_004e7c60`, `FUN_004e7d70`, `FUN_004ad380`; the random vectors
//! were computed with the Python reference.

use super::memory_tests::{REF0, Rig};
use super::testkit::list;
use super::vars::{self, KiList, LIST_BUCKETS};
use crate::rng::CrtRand;

const L0: i32 = list(0) as i32;
const L1: i32 = list(1) as i32;
const L2: i32 = list(2) as i32;
/// Integer variables 0 and 1.
const I0: i32 = 0;
const I1: i32 = 1;

/// Test rig with integer variables, two reference variables and three lists.
fn rig() -> Rig {
    Rig::with_ranges(&[(0, 8), (REF0, 2), (list(0), 3)])
}

fn fill(r: &mut Rig, which: i32, entries: &[(i32, i32)]) {
    let list = r.thread.vars.list_at_mut(which).expect("list");
    for &(k, v) in entries {
        list.insert(k, v);
    }
}

fn entries(r: &Rig, which: i32) -> Vec<(i32, i32)> {
    list_entries(r.thread.vars.list_at(which).expect("list"))
}

fn plain(entries: &[(i32, i32)]) -> KiList {
    let mut list = KiList::new(LIST_BUCKETS);
    for &(k, v) in entries {
        list.insert(k, v);
    }
    list
}

fn list_entries(list: &KiList) -> Vec<(i32, i32)> {
    list.iter().map(|(k, v)| (k, *v)).collect()
}

fn values_at(list: &KiList, keys: [i32; 3]) -> [i32; 3] {
    keys.map(|k| *list.get(k).expect("key"))
}

#[test]
fn list_reads_create_missing_keys_and_invalid_lists_push_nothing() {
    let mut r = rig();
    fill(&mut r, L0, &[(3, 7)]);
    // All four variants read the same.
    for id in [0x33, 0x37, 0x3D, 0x41] {
        assert_eq!(r.call(id, &[L0, 3]), [7], "{id:#x}");
    }
    // A missing key is created with 0, at the front of the chain of its bucket (E-32).
    assert_eq!(r.call(0x33, &[L0, 5]), [0]);
    assert_eq!(entries(&r, L0), [(5, 0), (3, 7)]);
    assert_eq!(r.call(0x41, &[L0, 5]), [0]);
    assert_eq!(entries(&r, L0), [(5, 0), (3, 7)]);

    // Pointer 0 (integer variable 0, reference variable, address outside): no push, no note
    // (E-34, `JZ 0x004B2A57`).
    for id in [0x33, 0x37, 0x3D, 0x41] {
        assert_eq!(r.call(id, &[I0, 1]), Vec::<i32>::new(), "{id:#x}");
        assert_eq!(r.call(id, &[REF0 as i32, 1]), Vec::<i32>::new());
        assert_eq!(r.call(id, &[0x00FF_0000, 1]), Vec::<i32>::new());
    }
    assert!(r.guarded().is_empty());
    // A list variable beyond the range reads as pointer 0.
    assert_eq!(r.call(0x33, &[list(3) as i32, 1]), Vec::<i32>::new());
    assert!(r.guarded().is_empty());

    // Pointer not 0 without a list behind it: access to nonsense in the original, here `Guarded`
    // and 0 (re/ki-vm 12.3 P7).
    r.set_var(I1 as u32, 5);
    assert_eq!(r.call(0x37, &[I1, 1]), [0]);
    assert_eq!(r.guarded(), [0x37]);
}

#[test]
fn list_writes_use_the_variable_without_reference_resolution() {
    let mut r = rig();
    for (n, id) in [0x34, 0x38, 0x3E, 0x42].into_iter().enumerate() {
        let k = n as i32;
        assert_eq!(r.call(id, &[L1, k, 10 + k]), Vec::<i32>::new());
    }
    assert_eq!(entries(&r, L1), [(3, 13), (2, 12), (1, 11), (0, 10)]);
    // Overwriting keeps its index in the chain.
    r.call(0x34, &[L1, 2, 99]);
    assert_eq!(entries(&r, L1), [(3, 13), (2, 99), (1, 11), (0, 10)]);

    // `0x36`: `L[k] = get(&v)` without reference resolution - a reference variable yields its own
    // cell.
    r.set_var(I1 as u32, 41);
    r.set_var(REF0, 33);
    for id in [0x36, 0x3A, 0x40, 0x44] {
        r.call(id, &[L2, 8, I1]);
        assert_eq!(entries(&r, L2), [(8, 41)], "{id:#x}");
        r.call(id, &[L2, 9, REF0 as i32]);
        assert_eq!(r.thread.vars.list_at(L2).unwrap().get(9), Some(&33));
        r.thread.vars.list_at_mut(L2).unwrap().clear();
    }

    // Pointer 0: nothing, no note; pointer without a list: `Guarded`.
    for id in [0x34, 0x36] {
        assert_eq!(r.call(id, &[I0, 1, 2]), Vec::<i32>::new());
    }
    assert!(r.guarded().is_empty());
    r.set_var(I1 as u32, 5);
    r.call(0x38, &[I1, 1, 2]);
    r.call(0x3A, &[I1, 1, 2]);
    assert_eq!(r.guarded(), [0x38, 0x3A]);
}

#[test]
fn list_copy_merges_in_mfc_order() {
    for id in [0x35, 0x39, 0x3F, 0x43] {
        let mut r = rig();
        // Source: bucket 0 has the chain [2, 1], bucket 2 the key 40.
        fill(&mut r, L0, &[(40, 4), (1, 10), (2, 20)]);
        assert_eq!(entries(&r, L0), [(2, 20), (1, 10), (40, 4)]);
        // Destination: foreign keys 2 (is overwritten) and 100.
        fill(&mut r, L1, &[(2, 99), (100, 5)]);
        assert_eq!(r.call(id, &[L0, L1]), Vec::<i32>::new());
        // Source order 2, 1, 40: 2 replaces in place, 1 goes to the front of bucket 0, 40 new in
        // bucket 2; 100 stays.
        assert_eq!(
            entries(&r, L1),
            [(1, 10), (2, 20), (40, 4), (100, 5)],
            "{id:#x}"
        );
        // The source stays unchanged.
        assert_eq!(entries(&r, L0), [(2, 20), (1, 10), (40, 4)]);

        // Source with pointer 0: nothing, no note.
        assert_eq!(r.call(id, &[I0, L1]), Vec::<i32>::new());
        assert!(r.guarded().is_empty());
        // Destination invalid, source empty: nothing; source non-empty: crash case (address 4).
        assert_eq!(r.call(id, &[L2, I0]), Vec::<i32>::new());
        assert!(r.guarded().is_empty());
        assert_eq!(r.call(id, &[L0, I0]), Vec::<i32>::new());
        assert_eq!(r.guarded(), [id as u16]);
        assert_eq!(entries(&r, L1).len(), 4);
        // Source without a list (pointer not 0): also caught.
        r.set_var(I1 as u32, 5);
        assert_eq!(r.call(id, &[I1, L1]), Vec::<i32>::new());
        assert_eq!(r.guarded(), [id as u16]);
    }
    // Copying a list onto itself changes nothing.
    let mut r = rig();
    fill(&mut r, L0, &[(1, 1), (17, 2), (2, 3)]);
    let before = entries(&r, L0);
    r.call(0x35, &[L0, L0]);
    assert_eq!(entries(&r, L0), before);
}

#[test]
fn list_insert_loses_the_last_element() {
    // E-32: `[a, b, c]` at 0...2, inserting `x` at 1 -> `[a, x, b]`.
    let (a, b, c, x) = (11, 22, 33, 99);
    let mut list = plain(&[(0, a), (1, b), (2, c)]);
    vars::insert_index(&mut list, x, 1);
    assert_eq!(list.len(), 3);
    assert_eq!(values_at(&list, [0, 1, 2]), [a, x, b], "c is lost");
    // At the last index it only replaces.
    let mut list = plain(&[(0, a), (1, b), (2, c)]);
    vars::insert_index(&mut list, x, 2);
    assert_eq!(values_at(&list, [0, 1, 2]), [a, b, x]);
    // An empty list stays empty.
    let mut empty = plain(&[]);
    vars::insert_index(&mut empty, x, 0);
    assert!(empty.is_empty());
    // Gap: only `L[5]`.
    let mut gap = plain(&[(0, a), (1, b), (2, c)]);
    vars::insert_index(&mut gap, x, 5);
    assert_eq!(gap.len(), 4);
    assert_eq!(gap.get(5), Some(&x));
    assert_eq!(values_at(&gap, [0, 1, 2]), [a, b, c]);
    // Shifting goes only up to the first gap.
    let mut run = plain(&[(0, a), (1, b), (3, c)]);
    vars::insert_index(&mut run, x, 0);
    assert_eq!(run.len(), 3);
    assert_eq!(values_at(&run, [0, 1, 3]), [x, a, c]);

    // Via the builtins `0xE1`-`0xE4`: stack (list, index, value).
    for id in [0xE1, 0xE2, 0xE3, 0xE4] {
        let mut r = rig();
        fill(&mut r, L0, &[(0, a), (1, b), (2, c)]);
        assert_eq!(r.call(id, &[L0, 1, x]), Vec::<i32>::new());
        let list = r.thread.vars.list_at(L0).unwrap();
        assert_eq!(values_at(list, [0, 1, 2]), [a, x, b], "{id:#x}");
        // Empty list: nothing.
        assert_eq!(r.call(id, &[L1, 0, x]), Vec::<i32>::new());
        assert!(r.thread.vars.list_at(L1).unwrap().is_empty());
        // Pointer 0: nothing, no note; pointer without a list: `Guarded`.
        assert_eq!(r.call(id, &[I0, 0, x]), Vec::<i32>::new());
        assert!(r.guarded().is_empty());
        r.set_var(I1 as u32, 5);
        r.call(id, &[I1, 0, x]);
        assert_eq!(r.guarded(), [id as u16]);
    }
}

#[test]
fn list_remove_shifts_even_without_a_hit() {
    // re/ki-vm 12.3 P10, E-43: keys {0, 1, 2, 7} (bucket 0, chain [7, 2, 1, 0]), removing 5 hits
    // nothing but lowers 7 to 6. The rebuild via the helper list reverses the chain twice:
    // afterwards [6, 2, 1, 0].
    let mut list = plain(&[(0, 100), (1, 101), (2, 102), (7, 107)]);
    assert_eq!(
        list_entries(&list),
        [(7, 107), (2, 102), (1, 101), (0, 100)]
    );
    vars::remove_index(&mut list, 5);
    assert_eq!(
        list_entries(&list),
        [(6, 107), (2, 102), (1, 101), (0, 100)]
    );

    // Hit: 1 drops out, 2 -> 1, 7 -> 6. Helper list [0, 1, 6] (insert at the front), then the list
    // [6, 1, 0].
    let mut list = plain(&[(0, 100), (1, 101), (2, 102), (7, 107)]);
    vars::remove_index(&mut list, 1);
    assert_eq!(list_entries(&list), [(6, 107), (1, 102), (0, 100)]);

    // Several buckets: {0, 20, 40} (buckets 0, 1, 2) without a hit at 5 -> {0, 19, 39}.
    let mut list = plain(&[(0, 1), (20, 2), (40, 3)]);
    vars::remove_index(&mut list, 5);
    assert_eq!(list_entries(&list), [(0, 1), (19, 2), (39, 3)]);

    // Remove the last element -> empty; an empty list stays untouched.
    let mut single = plain(&[(4, 1)]);
    vars::remove_index(&mut single, 4);
    assert!(single.is_empty());
    vars::remove_index(&mut single, 0);
    assert!(single.is_empty());
    // Negative keys count too.
    let mut neg = plain(&[(-3, 1), (0, 2)]);
    vars::remove_index(&mut neg, -5);
    assert_eq!(neg.get(-4), Some(&1));
    assert_eq!(neg.get(-1), Some(&2));

    // Via the builtins `0xDC`-`0xDF`: stack (list, index).
    for id in [0xDC, 0xDD, 0xDE, 0xDF] {
        let mut r = rig();
        fill(&mut r, L0, &[(0, 100), (1, 101), (2, 102), (7, 107)]);
        assert_eq!(r.call(id, &[L0, 5]), Vec::<i32>::new());
        assert_eq!(
            entries(&r, L0),
            [(6, 107), (2, 102), (1, 101), (0, 100)],
            "{id:#x}"
        );
        // Pointer 0 (also a reference variable: `get` without resolution reads its empty cell):
        // nothing.
        assert_eq!(r.call(id, &[I0, 5]), Vec::<i32>::new());
        assert_eq!(r.call(id, &[REF0 as i32, 5]), Vec::<i32>::new());
        assert!(r.guarded().is_empty());
        r.set_var(I1 as u32, 5);
        r.call(id, &[I1, 0]);
        assert_eq!(r.guarded(), [id as u16]);
    }
}

#[test]
fn random_follows_the_original_formula() {
    // re/ki-vm 12.3 P5: vectors with `CrtRand::new(1)` (raw sequence 41, 18467, 6334, 26500, 19169, 15724, ...),
    // two draws per call, unless the first one already gives `lo > v`.
    let cases: [((i32, i32), [i32; 4]); 4] = [
        ((0, 100), [56, 80, 47, 89]),
        ((10, 5), [8, 6, 10, 10]),
        ((0, 100_000), [56358, -50201, 47987, -41479]),
        ((-50, 50), [6, 30, -3, 39]),
    ];
    for ((lo, hi), expected) in cases {
        let mut r = rig();
        r.rand = CrtRand::new(1);
        let got: Vec<i32> = (0..4).map(|_| r.call(0x8D, &[lo, hi])[0]).collect();
        assert_eq!(got, expected, "({lo}, {hi})");
    }

    // Consumption of the generator: (10, 5) draws only once from the third call on (first
    // draw 19169 gives `v < lo`), so 2 + 2 + 1 + 1 draws.
    let mut r = rig();
    r.rand = CrtRand::new(1);
    for _ in 0..4 {
        r.call(0x8D, &[10, 5]);
    }
    let mut reference = CrtRand::new(1);
    for _ in 0..6 {
        reference.rand();
    }
    assert_eq!(r.rand, reference);

    // Raw values of `CrtRand::new(0)` (start value 0 of the savegame).
    let mut raw = CrtRand::new(0);
    assert_eq!([(); 4].map(|()| raw.rand()), [38, 7719, 21238, 2437]);

    // Large spans: `hi − lo` and the product wrap like `IMUL`, never a debug panic (E-5,
    // re/ki-vm 12.3 P5). Vectors of the Python reference with 32-bit wrapping.
    let wide: [((i32, i32), [i32; 3]); 4] = [
        ((0, i32::MAX), [65537, 0, 0]),
        ((i32::MIN, i32::MAX), [i32::MIN; 3]),
        ((i32::MAX, i32::MIN), [i32::MAX; 3]),
        ((5, i32::MAX), [65539, 5, 5]),
    ];
    for ((lo, hi), expected) in wide {
        let mut r = rig();
        r.rand = CrtRand::new(1);
        let got: Vec<i32> = (0..3).map(|_| r.call(0x8D, &[lo, hi])[0]).collect();
        assert_eq!(got, expected, "({lo}, {hi})");
    }
}

#[test]
fn misc_vm_builtins() {
    let mut r = rig();

    // 0x89 (x1, z1, x2, z2) -> truncated distance.
    assert_eq!(r.call(0x89, &[0, 0, 3, 4]), [5]);
    assert_eq!(r.call(0x89, &[3, 4, 0, 0]), [5]);
    assert_eq!(r.call(0x89, &[0, 0, 1, 1]), [1]);
    assert_eq!(r.call(0x89, &[5, 5, 5, 5]), [0]);
    // x and z belong together in pairs: (1, 2) -> (4, 6) has dx = 3 and dz = 4.
    assert_eq!(r.call(0x89, &[1, 2, 4, 6]), [5]);
    assert_eq!(r.call(0x89, &[0, 0, 46340, 0]), [46340]);
    // 46341^2 = 2147488281 overflows in 32 bits and becomes negative: `FSQRT` of NaN, `_ftol`
    // returns `i32::MIN` (re/ki-vm 12.3 P11). Likewise (40000, 40000).
    assert_eq!(r.call(0x89, &[0, 0, 46341, 0]), [i32::MIN]);
    assert_eq!(r.call(0x89, &[0, 0, 40000, 40000]), [i32::MIN]);
    // 65536^2 wraps to 0.
    assert_eq!(r.call(0x89, &[0, 0, 65536, 0]), [0]);
    // The differences wrap too: `i32::MAX − i32::MIN` = -1.
    assert_eq!(r.call(0x89, &[i32::MIN, 0, i32::MAX, 0]), [1]);

    // 0xBE (l, t, r, b, x, z) with the rectangle (10, 20)-(30, 40): right and bottom edge
    // excluded (`PtInRect`).
    for ((x, z), want) in [
        ((10, 20), 1),
        ((29, 39), 1),
        ((30, 30), 0),
        ((20, 40), 0),
        ((9, 30), 0),
        ((20, 19), 0),
    ] {
        assert_eq!(r.call(0xBE, &[10, 20, 30, 40, x, z]), [want], "({x}, {z})");
    }
    // An empty rectangle never hits.
    assert_eq!(r.call(0xBE, &[5, 5, 5, 5, 5, 5]), [0]);

    // 0x91: [0xAF0, 0xD48).
    for (typ, want) in [(0xAEF, 0), (0xAF0, 1), (0xD47, 1), (0xD48, 0), (-1, 0)] {
        assert_eq!(r.call(0x91, &[typ]), [want], "0x91 {typ:#x}");
    }
    // 0xB6: [0xD48, 0xD57].
    for (typ, want) in [(0xD47, 0), (0xD48, 1), (0xD57, 1), (0xD58, 0), (0, 0)] {
        assert_eq!(r.call(0xB6, &[typ]), [want], "0xB6 {typ:#x}");
    }
    // 0xD1: three types per nation.
    for first in [0x459, 0x582, 0x6B2, 0x7DD, 0x909, 0xA36] {
        assert_eq!(r.call(0xD1, &[first - 1]), [0], "{first:#x} - 1");
        for typ in first..first + 3 {
            assert_eq!(r.call(0xD1, &[typ]), [1], "{typ:#x}");
        }
        assert_eq!(r.call(0xD1, &[first + 3]), [0], "{first:#x} + 3");
    }
    assert_eq!(r.call(0xD1, &[0]), [0]);

    // 0x17 (adr, x): x discarded, value with reference resolution.
    r.set_var(I1 as u32, 77);
    assert_eq!(r.call(0x17, &[I1, 123]), [77]);
    assert_eq!(r.call(0x17, &[I0, -1]), [0]);
    // Kind-1 address (reference): 0 (E-1), even if the cell is occupied.
    r.set_var(REF0, 33);
    assert_eq!(r.call(0x17, &[REF0 as i32, 5]), [0]);
    // Address outside: 0. Empty stack: both pops return 0 (E-3).
    assert_eq!(r.call(0x17, &[0x00FF_0000, 5]), [0]);
    assert_eq!(r.call(0x17, &[]), [0]);

    // 0x82: own slot without a request.
    for slot in [0u8, 3, 7] {
        r.thread.player = slot;
        assert_eq!(r.call(0x82, &[]), [i32::from(slot)]);
    }
}
