//! Object names of the scripts (`m_UnitNames`, `GameEngine+0x2558`, MFC `CMapStringToString`; re/scripts 6,
//! E-145 ... E-147): name (lowercase) -> `"id"` or `"id;id;…"`.
//!
//! The iteration order is that of the MFC table and observable (the reverse search ID -> name takes the first match):
//! 17 buckets (constructor `FUN_005E95F0`, `+8 = 0x11`), hash `h = h·33 + c` over the signed cp1252 bytes (`HashKey`
//! `0x005E92E2`, `MOVSX`), bucket `h % 17` unsigned; a new key goes to the front of its bucket, an existing one keeps
//! its position (`operator[]` `0x005E989E`); iteration buckets ascending, each chain from the front (`GetNextAssoc`
//! `0x005E978A`). A `HashMap` or `BTreeMap` would change this order.

use nml_formats::config::lower_german;
use nml_formats::property::atol;
use nml_formats::text::encode_cp1252;
use serde::{Deserialize, Serialize};

use crate::fixed_vec::FixedVec;
use crate::id::ObjectId;

/// Number of buckets of the name table (`InitHashTable` with `+8 = 0x11`, never changes).
pub const NAME_BUCKETS: usize = 17;

/// Name table with the chain order of MFC `CMapStringToString`. The type fixes the number of buckets; a key in the
/// wrong bucket or duplicated (only from a corrupted savegame) stays harmless: `get`/`remove` do not find it or take
/// the first occurrence, respectively, and `iter` returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectNames {
    /// The chain `(key, value)` per bucket, head first.
    buckets: FixedVec<Vec<(String, String)>, NAME_BUCKETS>,
}

impl ObjectNames {
    /// `HashKey<LPCSTR>` (`0x005E92E2`): `h = h·33 + c` over the cp1252 bytes, `c` signed (`i8`), wrapped to 32
    /// bits.
    pub fn hash(key: &str) -> u32 {
        encode_cp1252(key).iter().fold(0u32, |h, &b| {
            h.wrapping_mul(33).wrapping_add(i32::from(b as i8) as u32)
        })
    }

    /// Bucket of a key: `hash % 17` unsigned (`GetAssocAt` `0x005E961E`).
    pub fn bucket_of(key: &str) -> usize {
        Self::hash(key) as usize % NAME_BUCKETS
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.iter().all(Vec::is_empty)
    }

    /// `Lookup`: value of the key (without lowercasing).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.buckets
            .get(Self::bucket_of(key))?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `operator[]` with assignment: existing key -> overwrite the value (position stays), otherwise new at the
    /// front of the bucket.
    pub fn set(&mut self, key: &str, value: String) {
        let chain = &mut self.buckets[Self::bucket_of(key)];
        if let Some(entry) = chain.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value;
        } else {
            chain.insert(0, (key.to_owned(), value));
        }
    }

    /// `RemoveKey`: unlinks the entry, the rest of the chain keeps its order. Returns whether it was found.
    pub fn remove(&mut self, key: &str) -> bool {
        let chain = &mut self.buckets[Self::bucket_of(key)];
        match chain.iter().position(|(k, _)| k == key) {
            Some(i) => {
                chain.remove(i);
                true
            }
            None => false,
        }
    }

    /// All entries in `GetNextAssoc` order: buckets ascending, each chain from the front.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.buckets
            .iter()
            .flatten()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `GameEngine_RegisterObjectName` (`0x00439360`): lowercase the name with `CString_MakeLowerGerman` (only `A-Z`,
/// `ÄÖÜ`; not trimmed) and enter `Format("%d", id)`; an existing entry is overwritten (the one registered last
/// wins, E-145).
pub fn register(names: &mut ObjectNames, name: &str, id: ObjectId) {
    names.set(&lower_german(name), id.as_i32().to_string());
}

/// `FUN_00436240`: all IDs of a name (lowercased like [`register`]); the value is split at every `;` and each piece
/// is read with `atol` (empty pieces give 0). Unknown name -> empty.
pub fn ids(names: &ObjectNames, name: &str) -> Vec<ObjectId> {
    names.get(&lower_german(name)).map_or_else(Vec::new, |v| {
        v.split(';')
            .map(|part| ObjectId::from_i32(atol(part) as i32))
            .collect()
    })
}

/// `FUN_00439460`: the first ID of a name; an unknown name and a first ID of 0 are "none".
pub fn first_id(names: &ObjectNames, name: &str) -> Option<ObjectId> {
    ids(names, name).first().copied().filter(|id| id.0 != 0)
}

/// `FUN_004339B0`: reverse search ID -> name, first entry in `GetNextAssoc` order with `atol(value) == id` (for a
/// list only the first ID counts).
pub fn reverse(names: &ObjectNames, id: ObjectId) -> Option<String> {
    names
        .iter()
        .find(|(_, v)| atol(v) as i32 == id.as_i32())
        .map(|(k, _)| k.to_owned())
}

/// Name part of `GameWorld_ChangeObjectOwner` (`0x00481730`, from `0x0048185A`): reverse search of the old ID; if
/// found -> delete the key and, for a target slot other than 8, register it again under the new ID (a group `"a;b"`
/// shrinks to the new ID, on death the name disappears, E-145).
pub fn owner_changed(names: &mut ObjectNames, old: ObjectId, new: ObjectId, slot: u8) {
    let Some(key) = reverse(names, old) else {
        return;
    };
    names.remove(&key);
    if slot != 8 {
        register(names, &key, new);
    }
}

/// `FUN_0043E310(old, new, suffix)`: if `old` has a name, `new` gets the name `name + suffix`; the old entry stays
/// (evocation `"%02d"`, multipart `"tail"`, train `"tender"`/`"wagon"`, E-146).
pub fn transfer(names: &mut ObjectNames, old: ObjectId, new: ObjectId, suffix: &str) {
    if let Some(name) = reverse(names, old) {
        register(names, &format!("{name}{suffix}"), new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(v: i32) -> ObjectId {
        ObjectId::from_i32(v)
    }

    fn keys(names: &ObjectNames) -> Vec<&str> {
        names.iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn hash_vectors() {
        let h = |k: &str| (ObjectNames::hash(k), ObjectNames::bucket_of(k));
        assert_eq!(h("a"), (0x61, 12));
        assert_eq!(h("ab"), (0xCE3, 1));
        // `ü` = 0xFC counts as -4 when signed; unsigned it would be bucket 7.
        assert_eq!(h("bogenschütze"), (0x2CB0_8A18, 8));
        assert_eq!(h("lilaochstail"), (0x6199_5DF9, 14));
        assert_eq!(ObjectNames::bucket_of("drake hauptquartier"), 9);
        assert_eq!(ObjectNames::bucket_of("spa haupthaus"), 1);
        assert_eq!(ObjectNames::default().buckets.len(), NAME_BUCKETS);
    }

    #[test]
    fn new_keys_go_first_overwrite_keeps_position() {
        let mut names = ObjectNames::default();
        for k in ["unit9", "unit10", "unit21", "unit32"] {
            assert_eq!(ObjectNames::bucket_of(k), 2, "{k}");
        }
        for (i, k) in ["unit9", "unit10", "unit21", "unit32"]
            .into_iter()
            .enumerate()
        {
            names.set(k, i.to_string());
        }
        assert_eq!(keys(&names), ["unit32", "unit21", "unit10", "unit9"]);
        names.set("unit10", "77".into());
        assert_eq!(keys(&names), ["unit32", "unit21", "unit10", "unit9"]);
        assert_eq!(names.get("unit10"), Some("77"));
        assert_eq!(names.len(), 4);
        // Removal keeps the order of the rest of the chain.
        assert!(names.remove("unit21"));
        assert!(!names.remove("unit21"));
        assert_eq!(keys(&names), ["unit32", "unit10", "unit9"]);
    }

    #[test]
    fn reverse_finds_first_in_mfc_order() {
        // Bucket 0 before bucket 1, regardless of the insertion order.
        assert_eq!(ObjectNames::bucket_of("unit7"), 0);
        assert_eq!(ObjectNames::bucket_of("unit8"), 1);
        let mut names = ObjectNames::default();
        names.set("unit8", "5".into());
        names.set("unit7", "5;9".into());
        assert_eq!(reverse(&names, id(5)).as_deref(), Some("unit7"));
        // Only the first ID of a list counts.
        assert_eq!(reverse(&names, id(9)), None);
        // In the same bucket: the most recently added one is at the front.
        let mut same = ObjectNames::default();
        same.set("unit9", "5".into());
        same.set("unit10", "5".into());
        assert_eq!(reverse(&same, id(5)).as_deref(), Some("unit10"));
        // Overwriting does not change the position.
        same.set("unit9", "5".into());
        assert_eq!(reverse(&same, id(5)).as_deref(), Some("unit10"));
    }

    #[test]
    fn lists_are_split_and_parsed_with_atol() {
        let mut names = ObjectNames::default();
        names.set("group", "5;7; 9".into());
        assert_eq!(ids(&names, "Group"), [id(5), id(7), id(9)]);
        assert_eq!(first_id(&names, "GROUP"), Some(id(5)));
        // Empty pieces read as 0; a first 0 is "none".
        names.set("empty", "0;;3;".into());
        assert_eq!(ids(&names, "empty"), [id(0), id(0), id(3), id(0)]);
        assert_eq!(first_id(&names, "empty"), None);
        assert!(ids(&names, "missing").is_empty());
        assert_eq!(first_id(&names, "missing"), None);
    }

    #[test]
    fn register_lowers_and_overwrites() {
        let mut names = ObjectNames::default();
        register(&mut names, "Bogenschütze ÄÖÜ É ", id(7));
        // Only A-Z and ÄÖÜ are lowercased, nothing is trimmed (E-147).
        assert_eq!(names.get("bogenschütze äöü É "), Some("7"));
        register(&mut names, "BOGENSCHÜTZE äöü É ", id(-3));
        assert_eq!(names.get("bogenschütze äöü É "), Some("-3"));
        assert_eq!(names.len(), 1);
        assert_eq!(ids(&names, "Bogenschütze ÄÖÜ É "), [id(-3)]);
    }

    #[test]
    fn owner_change_and_transfer_follow_the_reverse_search() {
        let mut names = ObjectNames::default();
        register(&mut names, "group", id(5));
        names.set("group", "5;6".into());
        transfer(&mut names, id(5), id(11), "tail");
        assert_eq!(names.get("grouptail"), Some("11"));
        assert_eq!(names.get("group"), Some("5;6"), "the old entry stays");
        // Without a name no transfer.
        transfer(&mut names, id(6), id(12), "tail");
        assert_eq!(names.len(), 2);
        owner_changed(&mut names, id(5), id(20), 2);
        assert_eq!(names.get("group"), Some("20"), "the group shrinks");
        owner_changed(&mut names, id(20), id(21), 8);
        assert_eq!(names.get("group"), None);
        owner_changed(&mut names, id(99), id(100), 1);
        assert_eq!(names.len(), 1);
    }
}
