//! `rand()` of the MSVC run time (`_rand` `0x005D2D4C`) as a linear congruential generator. The original uses
//! a global generator; here each subsystem has its own with a fixed start value so that
//! runs are repeatable.

/// Largest value of [`MsvcRand::rand`] (`RAND_MAX`).
pub const RAND_MAX: u32 = 0x7FFF;

/// `1 / 32767`, as the game uses it (`3.051851e-05`, `0x00632160`).
pub const RAND_UNIT: f32 = 3.051_851e-5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsvcRand {
    state: u32,
}

impl MsvcRand {
    /// Generator after `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// `x = x * 214013 + 2531011`, result `(x >> 16) & 0x7FFF` (0 to 32767).
    pub fn rand(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.state >> 16) & RAND_MAX
    }

    /// State of the sequence after the last call (`_getptd()+0x14`; after `srand(seed)` the seed).
    pub fn state(&self) -> u32 {
        self.state
    }

    /// `rand() * 3.051851e-05` as in the game (0 to 1).
    pub fn unit(&mut self) -> f32 {
        self.rand() as f32 * RAND_UNIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_matches_the_msvc_runtime() {
        // Known sequence of `rand()` after `srand(1)`.
        let mut rng = MsvcRand::new(1);
        let values: Vec<u32> = (0..5).map(|_| rng.rand()).collect();
        assert_eq!(values, [41, 18467, 6334, 26500, 19169]);
        let mut a = MsvcRand::new(7);
        let mut b = MsvcRand::new(7);
        assert!((0..1000).all(|_| a.rand() == b.rand()));
        assert!((0..1000).all(|_| a.rand() <= RAND_MAX));
        assert!((RAND_MAX as f32 * RAND_UNIT - 1.0).abs() < 1e-6);
    }

    #[test]
    fn msvc_rand_exposes_its_state() {
        let mut rng = MsvcRand::new(1);
        assert_eq!(rng.state(), 1);
        rng.rand();
        assert_eq!(rng.state(), 214_013 + 2_531_011);
    }
}
