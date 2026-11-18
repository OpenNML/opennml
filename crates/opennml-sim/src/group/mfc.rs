//! Re-creation of the MFC `CMap<int, …>` (re/groups-formations 6): 17 buckets, bucket = `(key >> 4) % 17`
//! (unsigned), new entries at the front of the bucket, iteration buckets ascending and within the bucket from the
//! front. The order is part of the behavior (pack building, send order) and of the checksum.
//!
//! The type fixes the number of buckets. A key duplicated or in the wrong bucket (only from a corrupted
//! savegame) stays harmless: `get`/`remove` hit the first entry or do not find it, the iteration sees it.

use serde::{Deserialize, Serialize};

use crate::fixed_vec::FixedVec;

/// Number of buckets of the MFC default.
const BUCKETS: usize = 17;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MfcMap<V> {
    pub(crate) buckets: FixedVec<Vec<(u32, V)>, BUCKETS>,
}

impl<V> Default for MfcMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

fn bucket(k: u32) -> usize {
    ((k >> 4) as usize) % BUCKETS
}

impl<V> MfcMap<V> {
    pub fn new() -> Self {
        Self {
            buckets: FixedVec::from_fn(|_| Vec::new()),
        }
    }
    pub fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.buckets.iter().all(Vec::is_empty)
    }
    pub fn get(&self, k: u32) -> Option<&V> {
        self.buckets[bucket(k)]
            .iter()
            .find(|e| e.0 == k)
            .map(|e| &e.1)
    }
    pub fn get_mut(&mut self, k: u32) -> Option<&mut V> {
        self.buckets[bucket(k)]
            .iter_mut()
            .find(|e| e.0 == k)
            .map(|e| &mut e.1)
    }
    pub fn contains(&self, k: u32) -> bool {
        self.get(k).is_some()
    }
    /// `CMap::SetAt` or `operator[]`: replace an existing entry, otherwise insert at the front.
    pub fn set(&mut self, k: u32, v: V) {
        let b = &mut self.buckets[bucket(k)];
        match b.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 = v,
            None => b.insert(0, (k, v)),
        }
    }
    /// `CMap::RemoveKey`.
    pub fn remove(&mut self, k: u32) -> Option<V> {
        let b = &mut self.buckets[bucket(k)];
        let i = b.iter().position(|e| e.0 == k)?;
        Some(b.remove(i).1)
    }
    pub fn clear(&mut self) {
        self.buckets.iter_mut().for_each(Vec::clear);
    }
    /// `GetStartPosition`/`GetNextAssoc`.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &V)> {
        self.buckets
            .iter()
            .flat_map(|b| b.iter().map(|e| (e.0, &e.1)))
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.buckets
            .iter_mut()
            .flat_map(|b| b.iter_mut().map(|e| &mut e.1))
    }
    pub fn keys(&self) -> Vec<u32> {
        self.iter().map(|e| e.0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_buckets_in_order_and_inserts_at_the_head() {
        // Bucket = (k >> 4) % 17: 0x10 -> 1, 0x110 -> 0, 0x220 -> 0 (MFC `CMap<int, …>`, re/groups-formations 6).
        let mut m = MfcMap::new();
        m.set(0x10, 'a');
        m.set(0x110, 'b');
        m.set(0x220, 'c');
        assert_eq!(m.keys(), vec![0x220, 0x110, 0x10]);
        // SetAt replaces in place, the order stays.
        m.set(0x110, 'B');
        assert_eq!(
            m.iter().collect::<Vec<_>>(),
            vec![(0x220, &'c'), (0x110, &'B'), (0x10, &'a')]
        );
        assert_eq!(m.remove(0x220), Some('c'));
        assert_eq!(m.keys(), vec![0x110, 0x10]);
        assert_eq!((m.len(), m.get(0x10), m.get(0x30)), (2, Some(&'a'), None));
    }
}
