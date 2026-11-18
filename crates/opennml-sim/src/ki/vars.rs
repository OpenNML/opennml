//! Variable addresses, variable table and list objects of the AI VM (`FUN_00413680`, `FUN_00413840`, `FUN_004137f0`,
//! `FUN_00413920`, `FUN_00413970`, `FUN_00413a40`, `FUN_00413ab0`; re/ki-vm 2, `docs/formats/kimodule.md` 3-4).
//!
//! A value is always `i32`. An address has 28 bits: kind (24-27), type (16-23), index (0-15). Kind bit 0 = reference
//! variable, kind bit 1 = list. References into the table of the caller are dead in the reimplementation (E-1): there
//! are no callers. A list variable returns its own address instead of the pointer to the list object (re/ki-vm 12.3
//! P2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::mfc::MfcMap;

/// Bucket count of a list (`FUN_004e7a20`).
pub const LIST_BUCKETS: usize = 10;

/// List object: `CMap` with hash size 10.
pub type KiList = MfcMap<i32>;

/// Kind of an address, bits 24-27. Bits 28-31 are ignored as in the original (`FUN_00413840`/`FUN_004137f0`:
/// `AND EAX,0xF` before `CMP EAX,0x10`, so the check "kind < 16" is always true).
pub fn kind(addr: u32) -> u32 {
    (addr >> 24) & 0xF
}

/// Type of an address, bits 16-23.
pub fn ty(addr: u32) -> u32 {
    (addr >> 16) & 0xFF
}

/// Index within the range, bits 0-15.
pub fn index(addr: u32) -> usize {
    (addr & 0xFFFF) as usize
}

/// Key of the range of an address: `type + 10·kind`; `None` for type >= 10 (entry 0-159).
pub fn range_key(addr: u32) -> Option<u8> {
    let t = ty(addr);
    (t < 10).then(|| (t + 10 * kind(addr)) as u8)
}

/// A variable range: `values` has the length of the range; `lists` only for kind bit 1 without bit 0, then one list
/// object per element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarRange {
    pub values: Vec<i32>,
    pub lists: Vec<KiList>,
}

/// Variable table of an instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarTable {
    pub ranges: BTreeMap<u8, VarRange>,
}

impl VarTable {
    /// `FUN_00413680` (ranges) and `FUN_00413a40` (initial values): one range per `(id, len)` with `len > 0` and a
    /// valid type; for a duplicate ID the last one wins. Initial values go through [`VarTable::set`]; lists and indexes
    /// >= length are silently ignored.
    pub fn from_module(ranges: &[(u32, u32)], inits: &[(u32, i32)]) -> Self {
        let mut table = Self::default();
        for &(id, len) in ranges {
            let Some(key) = range_key(id) else { continue };
            if len == 0 {
                continue;
            }
            let len = len as usize;
            let lists = if kind(id) & 3 == 2 {
                (0..len).map(|_| KiList::new(LIST_BUCKETS)).collect()
            } else {
                Vec::new()
            };
            table.ranges.insert(
                key,
                VarRange {
                    values: vec![0; len],
                    lists,
                },
            );
        }
        for &(addr, value) in inits {
            table.set(addr, value);
        }
        table
    }

    /// `FUN_00413840`: read a value without resolving references. 0 outside the ranges; a list variable returns its own
    /// address (re/ki-vm 12.3 P2).
    pub fn get(&self, addr: u32) -> i32 {
        let Some(range) = range_key(addr).and_then(|key| self.ranges.get(&key)) else {
            return 0;
        };
        let Some(value) = range.values.get(index(addr)) else {
            return 0;
        };
        if kind(addr) & 3 == 2 {
            addr as i32
        } else {
            *value
        }
    }

    /// `FUN_004137f0`: write a value without resolving references. Not for kind bit 1; otherwise silently nothing.
    pub fn set(&mut self, addr: u32, v: i32) {
        if kind(addr) & 2 != 0 {
            return;
        }
        let Some(key) = range_key(addr) else { return };
        if let Some(slot) = self
            .ranges
            .get_mut(&key)
            .and_then(|r| r.values.get_mut(index(addr)))
        {
            *slot = v;
        }
    }

    /// `FUN_00413920` (opcode 0): reference -> 0 (E-1), list -> the address itself, otherwise the value.
    pub fn load(&self, addr: u32) -> i32 {
        match kind(addr) & 3 {
            0 => self.get(addr),
            2 => addr as i32,
            _ => 0,
        }
    }

    /// `FUN_00413970` (opcode 1): reference -> nothing (E-1); list -> copy the content of the source list `get(v)` into
    /// the target list `get(addr)`; otherwise write.
    pub fn store(&mut self, addr: u32, v: i32) {
        match kind(addr) & 3 {
            0 => self.set(addr, v),
            2 => {
                let src = self.list_slot(self.get(v as u32) as u32);
                let dst = self.list_slot(self.get(addr) as u32);
                let (Some(src), Some(dst)) = (src, dst) else {
                    return;
                };
                if src == dst {
                    return;
                }
                let copy = self.ranges[&src.0].lists[src.1].clone();
                if let Some(target) = self
                    .ranges
                    .get_mut(&dst.0)
                    .and_then(|r| r.lists.get_mut(dst.1))
                {
                    copy_list(&copy, target);
                }
            }
            _ => {}
        }
    }

    /// `FUN_00413ab0`: reference -> 0 (E-1), otherwise the value read.
    pub fn resolve(&self, addr: u32) -> i32 {
        if kind(addr) & 1 != 0 {
            0
        } else {
            self.get(addr)
        }
    }

    /// Location `(range key, index)` of an address that points to an existing list object (kind exactly 2, index <
    /// length).
    fn list_slot(&self, addr: u32) -> Option<(u8, usize)> {
        if kind(addr) & 3 != 2 {
            return None;
        }
        let key = range_key(addr)?;
        let range = self.ranges.get(&key)?;
        (index(addr) < range.lists.len()).then(|| (key, index(addr)))
    }

    /// List behind `resolve(addr)`; `None` if there is no list there.
    pub fn list(&self, addr: u32) -> Option<&KiList> {
        let (key, i) = self.list_slot(self.resolve(addr) as u32)?;
        self.ranges.get(&key)?.lists.get(i)
    }

    pub fn list_mut(&mut self, addr: u32) -> Option<&mut KiList> {
        let (key, i) = self.list_slot(self.resolve(addr) as u32)?;
        self.ranges.get_mut(&key)?.lists.get_mut(i)
    }

    /// List behind an already read list pointer (re/ki-vm 12.3 P2: the address of the list variable); `None` if there
    /// is no list there.
    pub fn list_at(&self, pointer: i32) -> Option<&KiList> {
        let (key, i) = self.list_slot(pointer as u32)?;
        self.ranges.get(&key)?.lists.get(i)
    }

    pub fn list_at_mut(&mut self, pointer: i32) -> Option<&mut KiList> {
        let (key, i) = self.list_slot(pointer as u32)?;
        self.ranges.get_mut(&key)?.lists.get_mut(i)
    }
}

/// `FUN_004e7b50`: write every entry of `src` to `dst` in MFC order (`dst[k] = v`, merging, nothing is deleted).
pub fn copy_list(src: &KiList, dst: &mut KiList) {
    for (key, value) in src.iter() {
        dst.insert(key, *value);
    }
}

/// `FUN_004e7c60(i)` (re/ki-vm 12.3 P10, E-43): for a non-empty list remove the key `i` and then lower **all** keys
/// `> i` by 1 - even if `i` was missing. The list is rebuilt through an auxiliary `CMap(10)`, its order afterwards is
/// the MFC order of the rebuild.
pub fn remove_index(l: &mut KiList, i: i32) {
    if l.is_empty() {
        return;
    }
    l.remove(i);
    let mut shifted = KiList::new(LIST_BUCKETS);
    for (key, value) in l.iter() {
        if key < i {
            shifted.insert(key, *value);
        } else if key > i {
            shifted.insert(key - 1, *value);
        }
    }
    l.clear();
    for (key, value) in shifted.iter() {
        l.insert(key, *value);
    }
}

/// `FUN_004e7d70(v, i)` (re/ki-vm 12.3 P10, E-32): empty list -> nothing; if `i` is missing, only `L[i] = v`; otherwise
/// shift the values from `i` up to the first gap back by one position. The last value of the chain is lost (it is never
/// written), the length stays the same.
pub fn insert_index(l: &mut KiList, v: i32, i: i32) {
    if l.is_empty() {
        return;
    }
    let Some(&first) = l.get(i) else {
        l.insert(i, v);
        return;
    };
    let (mut key, mut carry, mut old) = (i, v, first);
    loop {
        l.insert(key, carry);
        carry = old;
        key = key.wrapping_add(1);
        match l.get(key) {
            Some(&next) => old = next,
            None => break,
        }
    }
}
