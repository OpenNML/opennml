//! State of the random map for the control command `random_map`. Viewer and `nml-game` deliver the same response from
//! the same function.

use std::collections::BTreeMap;

use nml_control::{RandomMapState, hex_checksum};
use nml_sim::setup::START_POINT;

/// State of the random map for `random_map`: seed, script, defines, size, player areas, random number consumption per
/// phase, checksums (16 hex digits like `sim_hash`), object count per GUID, occupied slots and start points.
pub fn random_map_state(map: &nml_randommap::RandomMap) -> RandomMapState {
    let counts = &map.rand_counts;
    let digests = map.digests();
    let mut per_guid: BTreeMap<u32, u32> = BTreeMap::new();
    for object in &map.objects {
        *per_guid.entry(object.guid).or_default() += 1;
    }
    RandomMapState {
        seed_text: map.spec.seed_text.clone(),
        hash: map.hash,
        script: map.script_path.clone(),
        defines: map.defines.clone(),
        dim: map.dim as u32,
        players: map
            .player_areas
            .iter()
            .map(|p| [p.id, p.cx, p.cz, p.min_size])
            .collect(),
        rand_counts: [
            counts.preprocessor,
            counts.land_areas,
            counts.player_areas,
            counts.terrain_areas,
            counts.texture_guids,
            counts.objects,
        ],
        rand_state: map.rand_state,
        digests: [
            digests.heights,
            digests.tile_numbers,
            digests.tiles,
            digests.steep,
            digests.id_grid,
            digests.objects,
            digests.object_block,
        ]
        .map(hex_checksum),
        objects: per_guid.into_iter().map(|(guid, n)| [guid, n]).collect(),
        slots_used: map.slots_used,
        // The town centers (`TOWN_CENTER`, GUID 4200 = start point) are the start points.
        start_points: map
            .objects
            .iter()
            .filter(|o| o.guid == START_POINT)
            .map(|o| o.position)
            .collect(),
    }
}
