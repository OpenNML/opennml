//! Step 14: texture number -> GUID per tile (`FUN_00542F60` `0x00542F60`, re/random-map Z.6.12).

use anyhow::bail;
use opennml_formats::prj::Project;
use opennml_formats::rnd::RandomMapScript;

use crate::grid::Grid;
use crate::rng::GenRand;

/// `FUN_00542F60`: per tile in memory order (z outer, x inner) the name for the number
/// (`FUN_005428D0`, unknown -> ""), **first** definition with this name (`FUN_00541640`, `_mbscmp`),
/// `k` = number of its GUIDs, `idx = k < 2 ? 0 : rand() % k` (`0x005416A0`). Without a definition,
/// `FUN_00541640` returns -1 without a draw. If the project knows the GUID (`Project_FindTexture`), it
/// is stored in the tile, otherwise the **number** stays (D-335; -1 -> `0xFFFFFFFF`).
///
/// A `#Texture` without `GUID` on a tile reads `GUID[0]` from an empty list in the original
/// (null pointer, D-335); the reimplementation reports the error.
pub(crate) fn assign_texture_guids(
    grid: &Grid,
    script: &RandomMapScript,
    project: &Project,
    rand: &mut GenRand,
) -> anyhow::Result<Vec<u32>> {
    let mut tiles = Vec::with_capacity(grid.tiles.len());
    for (index, &number) in grid.tiles.iter().enumerate() {
        // `FUN_005428D0`: name of the definition with this number (numbers are consecutive, unique).
        let name = script
            .textures
            .iter()
            .find(|t| t.number == number)
            .map_or("", |t| t.name.as_str());
        let guid = match script.textures.iter().find(|t| t.name == name) {
            None => u32::MAX,
            Some(def) => {
                let k = def.guids.len();
                if k == 0 {
                    bail!(
                        "random map: #Texture {} has no GUID but lies on tile ({}, {})",
                        def.name,
                        index % grid.dim_x,
                        index / grid.dim_x
                    );
                }
                let idx = if k < 2 {
                    0
                } else {
                    // `rand()` lies in 0 ... 32767, `k` >= 2: no divisor 0.
                    rand.rand() as usize % k
                };
                def.guids[idx]
            }
        };
        tiles.push(if project.textures.contains_key(&guid) {
            guid
        } else {
            number as u32
        });
    }
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Phase;
    use opennml_formats::prj::ProjectTexture;
    use opennml_formats::rnd::TextureDef;
    use opennml_world::rand::MsvcRand;

    fn texture(number: i32, name: &str, guids: &[u32]) -> TextureDef {
        TextureDef {
            number,
            name: name.to_string(),
            height: 0.0,
            guids: guids.to_vec(),
        }
    }

    fn project(guids: &[u32]) -> Project {
        let mut project = Project::default();
        for &guid in guids {
            project.textures.insert(
                guid,
                ProjectTexture {
                    guid,
                    name: format!("t{guid}"),
                    file: String::new(),
                    bits: 0,
                    group: 0,
                    map_color: 0,
                },
            );
        }
        project
    }

    fn grid(dim_x: usize, tiles: &[i32]) -> Grid {
        let mut grid = Grid::new(dim_x, tiles.len() / dim_x);
        grid.tiles = tiles.to_vec();
        grid
    }

    // QUIRK D-335: deviation
    #[test]
    fn guid_draw_runs_in_memory_order_and_keeps_unknown_numbers() {
        let script = RandomMapScript {
            textures: vec![
                texture(1, "X", &[7]),
                // Second definition with the same name: number 2 -> name "X" -> first definition (one
                // GUID, no draw), not its own two GUIDs.
                texture(2, "X", &[9, 9]),
                texture(3, "NONE", &[]),
                texture(4, "D", &[12]),
                texture(5, "E", &[7, 7, 9]),
            ],
            ..RandomMapScript::default()
        };
        // The project does not know GUID 9.
        let project = project(&[7, 12]);
        // Row z = 0: 5, -1, 4; z = 1: 5, 99, 2, 5 - memory order z outer, x inner.
        let grid = grid(3, &[5, -1, 4, 5, 99, 2, 5, 1, 1]);
        let mut rand = GenRand::new(77);
        rand.set_phase(Phase::TextureGuids);
        let tiles = assign_texture_guids(&grid, &script, &project, &mut rand).unwrap();
        // Exactly one draw `rand() % 3` per tile with number 5, in memory order.
        let mut plain = MsvcRand::new(77);
        let mut draw = || match [7u32, 7, 9][plain.rand() as usize % 3] {
            9 => 5, // GUID not in the project -> the number stays
            guid => guid,
        };
        let expected = [
            draw(),
            u32::MAX, // -1: no definition, no draw
            12,
            draw(),
            99, // number without definition: no draw, the number stays
            7,
            draw(),
            7,
            7,
        ];
        assert_eq!(tiles, expected);
        assert_eq!(rand.counts().texture_guids, 3);
        assert_eq!(rand.state(), plain.state());
        // Seed 77 draws 290 % 3 = 2 (GUID 9, unknown), then 0 and 0 (GUID 7): both cases occur.
        assert_eq!([tiles[0], tiles[3], tiles[6]], [5, 7, 7]);
    }

    // QUIRK D-335: deviation
    #[test]
    fn texture_without_guid_on_a_tile_is_an_error() {
        let script = RandomMapScript {
            textures: vec![texture(1, "GRASS", &[7]), texture(2, "EMPTY", &[])],
            ..RandomMapScript::default()
        };
        let project = project(&[7]);
        // A definition without a GUID does not matter as long as no tile carries it.
        let mut rand = GenRand::new(1);
        let tiles = assign_texture_guids(&grid(2, &[1, 1]), &script, &project, &mut rand).unwrap();
        assert_eq!(tiles, [7, 7]);
        let error = assign_texture_guids(&grid(2, &[1, 1, 1, 2]), &script, &project, &mut rand)
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("EMPTY") && text.contains("(1, 1)"), "{text}");
    }
}
