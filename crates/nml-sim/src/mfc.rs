//! `MfcMap`: ordered map with exactly the iteration order of MFC `CMap` (key hash `HashKey(key) = (DWORD)key >> 4`).
//! The original iterates the buckets ascending, each bucket's chain from the front; a new key goes to the front of
//! the chain. This order is observable (list builtins, thread table), so a `HashMap` or `BTreeMap` would change it.

use serde::{Deserialize, Serialize};

/// Map with a fixed number of buckets and the chain order of MFC `CMap`.
///
/// A loaded state must be checked with [`MfcMap::check`] before it is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MfcMap<V> {
    /// The chain per bucket, head first.
    buckets: Vec<Vec<(i32, V)>>,
}

impl<V> MfcMap<V> {
    /// Empty map with `n` buckets (MFC `InitHashTable(n)`); `n` must be greater than 0.
    pub fn new(n: usize) -> Self {
        assert!(n > 0, "MfcMap needs at least one bucket");
        Self {
            buckets: (0..n).map(|_| Vec::new()).collect(),
        }
    }

    /// Number of buckets.
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Bucket of a key: `HashKey` is unsigned (`(key as u32) >> 4`).
    pub fn bucket_of(&self, key: i32) -> usize {
        ((key as u32) >> 4) as usize % self.buckets.len()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.iter().all(Vec::is_empty)
    }

    pub fn get(&self, key: i32) -> Option<&V> {
        self.buckets[self.bucket_of(key)]
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: i32) -> Option<&mut V> {
        let b = self.bucket_of(key);
        self.buckets[b]
            .iter_mut()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: i32) -> bool {
        self.get(key).is_some()
    }

    /// `SetAt`: existing key -> replace the value, the position in the chain stays; new key -> to the front of the
    /// chain of its bucket.
    pub fn insert(&mut self, key: i32, value: V) {
        if let Some(slot) = self.get_mut(key) {
            *slot = value;
        } else {
            let b = self.bucket_of(key);
            self.buckets[b].insert(0, (key, value));
        }
    }

    /// `operator[]`: missing key -> default value at the front of the chain.
    pub fn entry_or_default(&mut self, key: i32) -> &mut V
    where
        V: Default,
    {
        let b = self.bucket_of(key);
        let pos = match self.buckets[b].iter().position(|(k, _)| *k == key) {
            Some(pos) => pos,
            None => {
                self.buckets[b].insert(0, (key, V::default()));
                0
            }
        };
        &mut self.buckets[b][pos].1
    }

    /// `RemoveKey`: unlink the entry from the chain.
    pub fn remove(&mut self, key: i32) -> Option<V> {
        let b = self.bucket_of(key);
        let pos = self.buckets[b].iter().position(|(k, _)| *k == key)?;
        Some(self.buckets[b].remove(pos).1)
    }

    /// `RemoveAll`: remove all entries, the number of buckets stays.
    pub fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
    }

    /// Entries in MFC order: buckets ascending, each chain from the front.
    pub fn iter(&self) -> impl Iterator<Item = (i32, &V)> + '_ {
        self.buckets
            .iter()
            .flat_map(|chain| chain.iter().map(|(k, v)| (*k, v)))
    }

    /// Entries mutably in MFC order; the keys stay fixed.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (i32, &mut V)> + '_ {
        self.buckets
            .iter_mut()
            .flat_map(|chain| chain.iter_mut().map(|(k, v)| (*k, v)))
    }

    /// `GetStartPosition`: key of the first entry.
    pub fn first_key(&self) -> Option<i32> {
        self.buckets
            .iter()
            .find_map(|chain| chain.first().map(|(k, _)| *k))
    }

    /// `GetNextAssoc`: successor in the chain, otherwise the head of the next non-empty bucket. `None` at the end of
    /// the sequence and for a missing key.
    pub fn next_key(&self, key: i32) -> Option<i32> {
        let b = self.bucket_of(key);
        let pos = self.buckets[b].iter().position(|(k, _)| *k == key)?;
        if let Some((next, _)) = self.buckets[b].get(pos + 1) {
            return Some(*next);
        }
        self.buckets[b + 1..]
            .iter()
            .find_map(|chain| chain.first().map(|(k, _)| *k))
    }

    /// Check for loaded states: exactly `n` buckets (`bucket_of` divides by the number of buckets), every key in the
    /// bucket of its hash and none duplicated. A duplicate would make the traversal `first_key`/`next_key` run in a
    /// circle (`next_key` looks for the first occurrence); a key in the wrong bucket would be the seed of that:
    /// `insert` does not find it and creates it a second time in the right bucket.
    pub fn check(&self, n: usize) -> Result<(), String> {
        if self.buckets.len() != n {
            return Err(format!("{} buckets instead of {n}", self.buckets.len()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, chain) in self.buckets.iter().enumerate() {
            for (key, _) in chain {
                if ((*key as u32) >> 4) as usize % n != index {
                    return Err(format!("key {key} in the wrong bucket {index}"));
                }
                if !seen.insert(*key) {
                    return Err(format!("key {key} duplicated"));
                }
            }
        }
        Ok(())
    }
}
