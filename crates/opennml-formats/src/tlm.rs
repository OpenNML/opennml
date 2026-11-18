//! Texture map `.tlm`: `DIM_X * DIM_Z` `u32` values (`TEX_GUID` per tile, index
//! `DIM_X * z + x`), see `docs/formats/tlm.md` (`Terrain_ReadTlm` `0x0055CE70`).

use crate::FormatError;
use crate::binary::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct TextureMap {
    pub dim_x: usize,
    pub dim_z: usize,
    pub guids: Vec<u32>,
}

impl TextureMap {
    /// `TEX_GUID` of tile `(x, z)`.
    pub fn guid(&self, x: usize, z: usize) -> u32 {
        self.guids[z * self.dim_x + x]
    }
}

/// Reads a `.tlm` for `dim_x x dim_z` tiles; the size must be exactly `dim_x * dim_z * 4`.
pub fn parse_tlm(bytes: &[u8], dim_x: u32, dim_z: u32) -> Result<TextureMap, FormatError> {
    let (dim_x, dim_z) = (dim_x as usize, dim_z as usize);
    let expected = dim_x
        .checked_mul(dim_z)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| FormatError::Invalid(format!("grid {dim_x} x {dim_z} too large")))?;
    if bytes.len() != expected {
        return Err(FormatError::Invalid(format!(
            ".tlm has {} bytes, expected {expected} for {dim_x} x {dim_z} tiles",
            bytes.len()
        )));
    }
    let mut r = Reader::new(bytes);
    let guids = r
        .ints::<1>(dim_x * dim_z, "texture_guid")?
        .into_iter()
        .map(|[g]| g as u32)
        .collect();
    r.finish()?;
    Ok(TextureMap {
        dim_x,
        dim_z,
        guids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write_tlm;

    #[test]
    fn tlm_roundtrip_and_indexing() {
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![5, 6, 7, 0xFFFF_FFFF],
        };
        let parsed = parse_tlm(&write_tlm(&map), 2, 2).unwrap();
        assert_eq!(parsed, map);
        assert_eq!(parsed.guid(0, 1), 7);
    }

    #[test]
    fn hlm_and_tlm_check_sizes() {
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let bytes = write_tlm(&map);
        assert!(matches!(
            parse_tlm(&bytes, 2, 3),
            Err(FormatError::Invalid(_))
        ));
        assert!(matches!(
            parse_tlm(&bytes[..15], 2, 2),
            Err(FormatError::Invalid(_))
        ));
        let hlm = crate::testutil::write_hlm(&crate::testutil::flat_heightmap(2, 2, 128.0, 0.0));
        assert!(crate::hlm::parse_hlm(&hlm, 3, 2).is_err());
        assert!(crate::hlm::parse_hlm(&hlm, u32::MAX, u32::MAX).is_err());
    }
}
