//! Pathfinding (re/path-grid, re/tile-paths): grid of the nine movement classes with regions, tile A*, smoothing and
//! waypoint list as in the original.

pub mod astar;
pub mod grid;
pub mod highpath;
pub mod reach;
pub mod region;
pub mod smooth;
pub mod terrain;

/// Blocked cell.
pub const BLOCKED: u16 = 0xFFFF;

/// Number of layers (movement classes).
pub const LAYER_COUNT: usize = 9;

/// Movement mask per layer (`0x00621A0C`): bit `0x01` 1 x 1, `0x02` 2 x 2, `0x04` 3 x 3, `0x08` land, `0x10` forest
/// (trees passable), `0x20` water.
pub const LAYER_MASKS: [u32; LAYER_COUNT] = [0x09, 0x29, 0x19, 0x39, 0x21, 0x0A, 0x22, 0x2A, 0x24];

/// `PathPlanner_LayerFromMoveMask` (`0x004F6C10`): layer for the movement mask (walk flags `10003 >> 8`); layer 0
/// without a match.
pub fn layer_from_move_mask(mask: u32) -> usize {
    let m = mask & 0x3F;
    if m == 0 {
        return 0;
    }
    LAYER_MASKS.iter().position(|&l| l == m).unwrap_or(0)
}

/// Layer of a unit (`PropertyWalk_InitBase` `0x00532850`): walk flags (10003 of the first walk descriptor) `>> 8`;
/// `None` without a walk module.
pub fn unit_layer(rules: &crate::rules::Rules, guid: u32) -> Option<usize> {
    let walk = rules.types.get(&guid)?.module(0)?;
    let flags = walk.value_int(10003).unwrap_or(0) as u32;
    Some(layer_from_move_mask(flags >> 8))
}

/// Short name of a layer (display).
pub fn layer_name(layer: usize) -> &'static str {
    [
        "Land 1x1",
        "Land+Water 1x1",
        "Land+Forest 1x1",
        "Land+Forest+Water 1x1",
        "Water 1x1",
        "Land 2x2",
        "Water 2x2",
        "Land+Water 2x2",
        "Water 3x3",
    ]
    .get(layer)
    .copied()
    .unwrap_or("?")
}

/// Base layer of a derived layer (`mask & !6 | 1`, first match): 5 -> 0, 6 -> 4,
/// 7 -> 1, 8 -> 4.
pub fn base_layer(layer: usize) -> usize {
    let target = (LAYER_MASKS[layer] & !6) | 1;
    LAYER_MASKS.iter().position(|&l| l == target).unwrap_or(0)
}

/// Cells as a byte sequence (little endian) instead of individually: faster in the checksum and savegame.
pub(crate) mod cell_bytes {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(cells: &[u16], s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = cells.iter().flat_map(|c| c.to_le_bytes()).collect();
        s.serialize_bytes(&bytes)
    }

    struct CellVisitor;

    impl<'de> Visitor<'de> for CellVisitor {
        type Value = Vec<u16>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("grid cells as a byte sequence")
        }

        fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Vec<u16>, E> {
            if !v.len().is_multiple_of(2) {
                return Err(E::custom("odd length of the grid cells"));
            }
            Ok(v.chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u16>, A::Error> {
            let mut bytes = Vec::new();
            while let Some(b) = seq.next_element::<u8>()? {
                bytes.push(b);
            }
            self.visit_bytes(&bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u16>, D::Error> {
        d.deserialize_byte_buf(CellVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_masks_select_layers_and_derived_layers_have_bases() {
        assert_eq!(layer_from_move_mask(0x08 | 0x01), 0);
        assert_eq!(layer_from_move_mask(0x39), 3);
        assert_eq!(layer_from_move_mask(0x24 | 0xC0), 8);
        // PAT_Milizsoldat: only the size -> layer 0; unknown masks likewise.
        assert_eq!(layer_from_move_mask(0x01), 0);
        assert_eq!(layer_from_move_mask(0), 0);
        assert_eq!([5, 6, 7, 8].map(base_layer), [0, 4, 1, 4]);
    }

    #[test]
    fn units_get_their_layer_from_the_walk_flags() {
        let rules = crate::rules::Rules::synthetic(
            concat!(
                "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 2304\n",
                "GUID 1200\nNAME Krieger\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 14592\n",
                "GUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\n",
            ),
            16,
        );
        assert_eq!(unit_layer(&rules, 1100), Some(0));
        assert_eq!(unit_layer(&rules, 1200), Some(3));
        assert_eq!(unit_layer(&rules, 1000), None);
        assert_eq!(layer_name(8), "Water 3x3");
    }
}
