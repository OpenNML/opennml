//! Object IDs and player slots as in the original (re/sim-core 2):
//! `ID = index << 16 | slot << 11 | generation (11 bits)`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Player slots 0-7.
pub const PLAYER_SLOTS: usize = 8;
/// Pseudo slot neutral (animals and others, category bit `0x400`).
pub const NEUTRAL_SLOT: u8 = 8;
/// Pseudo slot trees (category bit `0x100`).
pub const TREE_SLOT: u8 = 9;
/// Pseudo slot resources and nature (category bit `0x200`).
pub const NATURE_SLOT: u8 = 10;
/// Number of slots (0-10).
pub const SLOT_COUNT: usize = 11;
/// Fixed capacity of the slot tables (`World_ParseNextBlock` `0x00441C50`).
pub const SLOT_CAPACITY: [usize; SLOT_COUNT] = [
    1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 300, 6000, 5000,
];

/// ID (HID) of an object; -1 (`0xFFFFFFFF`) = none.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u32);

impl ObjectId {
    /// "No object" (-1).
    pub const NONE: Self = Self(u32::MAX);

    /// ID from index, slot and the lower 11 bits of the generation.
    pub const fn new(index: u16, slot: u8, generation: u16) -> Self {
        Self((index as u32) << 16 | ((slot as u32) & 0x1F) << 11 | (generation as u32 & 0x7FF))
    }

    /// Index in the slot table (`ID >> 16`).
    pub const fn index(self) -> usize {
        (self.0 >> 16) as usize
    }

    /// Player slot (`ID >> 11 & 0x1F`, `GameObject_GetOwnerSlot` `0x004669C0`).
    pub const fn slot(self) -> u8 {
        ((self.0 >> 11) & 0x1F) as u8
    }

    /// Generation (11 bits).
    pub const fn generation(self) -> u16 {
        (self.0 & 0x7FF) as u16
    }

    /// As a signed value, as it appears in messages.
    pub const fn as_i32(self) -> i32 {
        self.0 as i32
    }

    pub const fn from_i32(v: i32) -> Self {
        Self(v as u32)
    }

    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            write!(f, "ObjectId(-)")
        } else {
            write!(
                f,
                "ObjectId({}:{}#{})",
                self.slot(),
                self.index(),
                self.generation()
            )
        }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_pack_index_slot_and_generation() {
        let id = ObjectId::new(999, 9, 0x801);
        assert_eq!((id.index(), id.slot(), id.generation()), (999, 9, 1));
        assert_eq!(id.0, 999 << 16 | 9 << 11 | 1);
        assert_eq!(ObjectId::from_i32(-1), ObjectId::NONE);
        assert!(ObjectId::NONE.is_none() && !id.is_none());
        assert_eq!(format!("{id:?}"), "ObjectId(9:999#1)");
        assert_eq!(SLOT_CAPACITY.iter().sum::<usize>(), 19_300);
    }
}
