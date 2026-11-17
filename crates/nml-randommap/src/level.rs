//! Chain of a random map in one place: reading map types -> [`generate`](crate::generate) ->
//! [`to_scene`](crate::to_scene) -> [`RandomMapRules`]. Game, viewer and tests build a random map only
//! through here (`GameEngine_CreateRandomMapLevel` `0x004402F0` without the lobby and the world
//! objects that `nml-sim` creates).

use nml_rda::Vfs;
use nml_sim::random_map::RandomMapRules;
use nml_world::level::LevelScene;

use crate::{RandomMap, RandomMapSpec, fnv1a, generate, load_types, to_scene};

/// Template whose maps carry the island flag (`GameEngine_CreateRandomMapLevel` `0x004402F0`,
/// comparison is case-sensitive, E-82).
const ISLAND_SCRIPT: &str = "Islands.rnd";

/// A generated random map with scene and rules input.
#[derive(Debug, Clone)]
pub struct RandomLevel {
    pub map: RandomMap,
    pub scene: LevelScene,
    pub rules: RandomMapRules,
}

/// Generates the random map for the description like the restart of the original (with object step)
/// and builds scene and rules input. Loading a savegame also goes through here.
pub fn build_level(vfs: &Vfs, spec: &RandomMapSpec) -> anyhow::Result<RandomLevel> {
    let types = load_types(vfs)?;
    let map = generate(vfs, &types, spec)?;
    let scene = to_scene(vfs, &map)?;
    let rules = rules_input(&map);
    Ok(RandomLevel { map, scene, rules })
}

/// `Right(script path, 11) == "Islands.rnd"` (`GameEngine_CreateRandomMapLevel` `0x004402F0`): the
/// last 11 characters exactly so, case matters; a shorter path is never an island. The path stands
/// as in `randommaptypes.ini` (CP1252, one character per byte as in the `CString`).
pub fn is_island_script(script_path: &str) -> bool {
    let n = script_path.chars().count();
    n >= ISLAND_SCRIPT.len()
        && script_path
            .chars()
            .skip(n - ISLAND_SCRIPT.len())
            .eq(ISLAND_SCRIPT.chars())
}

/// Rules input from the generator: slots = `slots_used`, island flag from the script path,
/// `crt_start` = final state of the `rand()` sequence, `digest` = FNV-1a 64 over
/// [`digest_bytes`].
pub fn rules_input(map: &RandomMap) -> RandomMapRules {
    RandomMapRules {
        spec: map.spec.clone(),
        slots: map.slots_used,
        island: is_island_script(&map.script_path),
        crt_start: map.rand_state,
        digest: fnv1a(digest_bytes(map)),
    }
}

/// Bytes of the rules checksum of a random map, little-endian: the seven [`crate::Digests`] in
/// field order (heights, texture numbers, tile GUIDs, steepness, ID grid, objects, blocking map) each
/// as `u64`, then the description: length of the seed text in bytes (`u32`) and its UTF-8 bytes,
/// size and number of players (`i32`), map type, landscape, time of day, game mode (each `u64`).
fn digest_bytes(map: &RandomMap) -> Vec<u8> {
    let d = map.digests();
    let spec = &map.spec;
    let mut bytes = Vec::new();
    for v in [
        d.heights,
        d.tile_numbers,
        d.tiles,
        d.steep,
        d.id_grid,
        d.objects,
        d.object_block,
    ] {
        bytes.extend(v.to_le_bytes());
    }
    bytes.extend((spec.seed_text.len() as u32).to_le_bytes());
    bytes.extend(spec.seed_text.as_bytes());
    bytes.extend(spec.size.to_le_bytes());
    bytes.extend(spec.players.to_le_bytes());
    for v in [spec.terrain, spec.landscape, spec.daytime, spec.game_type] {
        bytes.extend((v as u64).to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    // QUIRK E-82: kept
    #[test]
    fn level_island_flag_needs_the_exact_last_11_characters() {
        assert!(is_island_script(r"data\RandomMapScripts\Islands.rnd"));
        assert!(is_island_script("Islands.rnd"));
        assert!(is_island_script("xIslands.rnd"));
        // Case matters (E-82).
        assert!(!is_island_script(r"data\randommapscripts\islands.rnd"));
        assert!(!is_island_script("ISLANDS.RND"));
        assert!(!is_island_script(r"data\RandomMapScripts\Islands.rnd "));
        assert!(!is_island_script("slands.rnd"));
        assert!(!is_island_script(""));
        assert!(!is_island_script(r"data\RandomMapScripts\Continental.rnd"));
        // Characters instead of bytes (CP1252 has one character per byte).
        assert!(is_island_script("ÄIslands.rnd"));
    }

    #[test]
    fn level_digest_bytes_cover_digests_and_description() {
        let map = crate::tests::relief_map();
        let base = digest_bytes(&map);
        assert_eq!(
            base.len(),
            7 * 8 + 4 + map.spec.seed_text.len() + 2 * 4 + 4 * 8
        );
        let mut other = map.clone();
        other.spec.daytime += 1;
        assert_ne!(digest_bytes(&other), base);
        let mut other = map.clone();
        other.heights[0] += 1.0;
        assert_ne!(rules_input(&other).digest, rules_input(&map).digest);
        let mut other = map.clone();
        other.object_block = vec![1];
        assert_ne!(rules_input(&other).digest, rules_input(&map).digest);
        // Seed text with length: "ab" + size != "a" + ...
        let mut a = map.clone();
        a.spec.seed_text = "ab".into();
        let mut b = map.clone();
        b.spec.seed_text = "a".into();
        assert_ne!(rules_input(&a).digest, rules_input(&b).digest);
    }
}
