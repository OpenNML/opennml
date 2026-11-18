//! Description of a random map (menu values, re/random-map Z.2.2). Lives in `opennml-sim` because the savegame and the
//! start package carry it; the generator (`opennml-randommap`) re-exports it.

use serde::{Deserialize, Serialize};

/// The menu values of a random map (info fields, re/random-map Z.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RandomMapSpec {
    /// Seed text (`Info+0x000`, `wchar[40]`).
    pub seed_text: String,
    /// Size index 0/1/2 (small/medium/large, `MapSize`).
    pub size: i32,
    /// Player count 2 ... 8 (`SpielerCount`).
    pub players: i32,
    /// Map type = index into `MSCRIPT` (`RandomTerrain`).
    pub terrain: usize,
    /// Landscape = index into `LTYPE` (`RandomLandscape`).
    pub landscape: usize,
    /// Time of day = index into `DTYPE` (`RandomDaytime`).
    pub daytime: usize,
    /// Game mode = index into `GTYPE` (session `NS+0x1F4`).
    pub game_type: usize,
}

/// What the rules of a random map need instead of the `.lvl`: the description, the occupied slots, the island flag,
/// the final state of the generator's `rand()` sequence and a checksum over what was generated. Built by
/// `opennml_randommap::rules_input`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomMapRules {
    pub spec: RandomMapSpec,
    /// Slots 0...7 with a placed object of this owner (`RM+0x50`, `FUN_005473B0`) = the active slots of the match.
    pub slots: [bool; 8],
    /// `GameEngine+0x30C` (`m_IsIslandMap`): `Right(script_path, 11) == "Islands.rnd"`
    /// (`GameEngine_CreateRandomMapLevel` `0x004402F0`, E-82); read by the AI query `0x5130`.
    pub island: bool,
    /// State of the `rand()` sequence at the end of the generator (D-342): start value of `SimState.crt` on
    /// restart.
    pub crt_start: u32,
    /// FNV-1a 64 over all checksums of the generator and the description; it stands in the rules checksum in place of
    /// `.lvl`, `.wld` and `.hlm`.
    pub digest: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_map_spec_round_trips_through_serde() {
        let spec = RandomMapSpec {
            seed_text: "ÄÖ ü".to_string(),
            size: 2,
            players: 8,
            terrain: 3,
            landscape: 1,
            daytime: 2,
            game_type: 4,
        };
        let bytes = postcard::to_stdvec(&spec).unwrap();
        assert_eq!(postcard::from_bytes::<RandomMapSpec>(&bytes).unwrap(), spec);
    }
}
