//! Randomness of the simulation like `GameEngine_NextTableRandom` (`0x0042D8D0`): a fixed table of the first 1000
//! values of the MSVC `rand()` after `srand(1)` (`0x0065E8C0`), one index per match, incremented before each value,
//! period 1000. In a match without AI and scripts, the original's CRT `rand()` affects only the rendering
//! (re/sim-core 4).

use serde::{Deserialize, Serialize};

/// Length of the table.
pub const TABLE_LEN: usize = 1000;

/// The first 1000 values of `rand()` after `srand(1)` (`x = x · 214013 + 2531011`,
/// `(x >> 16) & 0x7FFF`).
pub const RAND_TABLE: [i16; TABLE_LEN] = {
    let mut table = [0i16; TABLE_LEN];
    let mut state: u32 = 1;
    let mut i = 0;
    while i < TABLE_LEN {
        state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
        table[i] = ((state >> 16) & 0x7FFF) as i16;
        i += 1;
    }
    table
};

/// State of the table sequence (`GameEngine+0x2B0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableRandom {
    index: u16,
}

impl TableRandom {
    /// Sequence with start index `start % 1000`; the original begins every level at 0.
    pub fn new(start: u32) -> Self {
        Self {
            index: (start % TABLE_LEN as u32) as u16,
        }
    }

    /// Next value (0 ... 32767): increment the index, back to 0 after 999, then read.
    pub fn rand(&mut self) -> i32 {
        // Without a `u16` overflow: a corrupted index (e.g. 65535) wraps back to 0.
        let next = usize::from(self.index) + 1;
        self.index = if next >= TABLE_LEN { 0 } else { next as u16 };
        i32::from(RAND_TABLE[usize::from(self.index)])
    }

    /// Current index (display, check).
    pub fn index(&self) -> u16 {
        self.index
    }
}

/// CRT `rand()` (`_rand` `0x005D2D4C`) for the module places that in the original only choose animations (idle
/// animation, start time): deterministic in the state, but separate from the table sequence so that rendering
/// randomness does not shift the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrtRand {
    state: u32,
}

impl CrtRand {
    /// Generator after `srand(seed)` (program start: 1).
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// `x = x · 214013 + 2531011`, result `(x >> 16) & 0x7FFF`.
    pub fn rand(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.state >> 16) & 0x7FFF) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_holds_the_msvc_sequence_and_the_index_wraps_after_999() {
        assert_eq!(RAND_TABLE[..5], [41, 18467, 6334, 26500, 19169]);
        let mut rng = TableRandom::new(0);
        // Pre-increment: the first value is table[1].
        assert_eq!(rng.rand(), 18467);
        assert_eq!(rng.index(), 1);
        let mut late = TableRandom::new(998);
        assert_eq!(late.rand(), i32::from(RAND_TABLE[999]));
        assert_eq!(late.rand(), 41);
        assert_eq!(late.index(), 0);
        assert_eq!(TableRandom::new(1999), TableRandom::new(999));
        let mut a = TableRandom::new(7);
        let mut b = a;
        assert!((0..3000).all(|_| a.rand() == b.rand()));
        let mut crt = CrtRand::new(1);
        let first: Vec<i32> = (0..3).map(|_| crt.rand()).collect();
        assert_eq!(first, [41, 18467, 6334]);
    }

    #[test]
    fn a_corrupt_index_wraps_to_zero_instead_of_overflowing() {
        // A corrupted savegame can carry an index of 65535 (debug: overflow otherwise).
        let mut rng = TableRandom { index: 65535 };
        assert_eq!(rng.rand(), i32::from(RAND_TABLE[0]));
        assert_eq!(rng.index(), 0);
    }
}
