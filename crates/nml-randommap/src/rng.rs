//! Randomness of the generator: `srand(Hash)` in `RandomMap_Generate` (`0x00546AC6`), after that only
//! `rand()` (`_rand` `0x005D2D4C`) from the module itself (re/random-map Z.4). The reimplementation
//! counts the consumption per phase and hands out the final state of the sequence.

use nml_world::rand::MsvcRand;

/// Section of the generator to which a `rand()` call is attributed (re/random-map Z.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `#random` lines of the preprocessor (`FUN_005424B0`).
    Preprocessor,
    /// `create_LandArea` (`FUN_00542930`).
    LandAreas,
    /// `create_PlayerAreas` (`FUN_00542A60`).
    PlayerAreas,
    /// `create_TerrainArea` (`FUN_005466F0`).
    TerrainAreas,
    /// Texture number -> GUID (`FUN_00542F60`).
    TextureGuids,
    /// Object step 13 (`FUN_005468D0`: placers `FUN_005451E0`/`FUN_00544B40`/`FUN_00545760`,
    /// random rotation in `FUN_0053FD00`).
    Objects,
}

/// Number of `rand()` calls per phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RandCounts {
    pub preprocessor: u32,
    pub land_areas: u32,
    pub player_areas: u32,
    pub terrain_areas: u32,
    pub texture_guids: u32,
    pub objects: u32,
}

impl RandCounts {
    /// The five phases of the load path without the object step.
    pub fn terrain(&self) -> [u32; 5] {
        [
            self.preprocessor,
            self.land_areas,
            self.player_areas,
            self.terrain_areas,
            self.texture_guids,
        ]
    }
}

/// The MSVC sequence of the generator with a counter.
#[derive(Debug, Clone)]
pub(crate) struct GenRand {
    rng: MsvcRand,
    phase: Phase,
    counts: RandCounts,
}

impl GenRand {
    /// `srand(seed)`; the first phase is the preprocessor.
    pub(crate) fn new(seed: u32) -> Self {
        Self {
            rng: MsvcRand::new(seed),
            phase: Phase::Preprocessor,
            counts: RandCounts::default(),
        }
    }

    /// `rand()` (0 ... 32767), attributed to the current phase.
    pub(crate) fn rand(&mut self) -> i32 {
        let count = match self.phase {
            Phase::Preprocessor => &mut self.counts.preprocessor,
            Phase::LandAreas => &mut self.counts.land_areas,
            Phase::PlayerAreas => &mut self.counts.player_areas,
            Phase::TerrainAreas => &mut self.counts.terrain_areas,
            Phase::TextureGuids => &mut self.counts.texture_guids,
            Phase::Objects => &mut self.counts.objects,
        };
        *count = count.wrapping_add(1);
        self.rng.rand() as i32
    }

    pub(crate) fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    pub(crate) fn counts(&self) -> RandCounts {
        self.counts
    }

    /// State of the sequence (`_getptd()+0x14`) after the last call.
    pub(crate) fn state(&self) -> u32 {
        self.rng.state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_phase_counts_draws() {
        let mut rand = GenRand::new(1);
        rand.set_phase(Phase::Objects);
        (0..3).for_each(|_| {
            rand.rand();
        });
        assert_eq!(rand.counts().objects, 3);
        assert_eq!(rand.counts().preprocessor, 0);
    }

    #[test]
    fn draws_are_counted_per_phase() {
        let mut rand = GenRand::new(1);
        assert_eq!(rand.rand(), 41);
        rand.set_phase(Phase::PlayerAreas);
        assert_eq!(rand.rand(), 18467);
        assert_eq!(rand.rand(), 6334);
        rand.set_phase(Phase::TextureGuids);
        rand.rand();
        assert_eq!(
            rand.counts(),
            RandCounts {
                preprocessor: 1,
                land_areas: 0,
                player_areas: 2,
                terrain_areas: 0,
                texture_guids: 1,
                objects: 0,
            }
        );
        rand.set_phase(Phase::Objects);
        rand.rand();
        assert_eq!(rand.counts().objects, 1);
        let mut plain = MsvcRand::new(1);
        (0..5).for_each(|_| {
            plain.rand();
        });
        assert_eq!(rand.state(), plain.state());
    }
}
