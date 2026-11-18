//! Height map `.hlm`: one 36-byte record per grid point, `(DIM_X + 1) * (DIM_Z + 1)`
//! records, row by row (`z` outer), see `docs/formats/hlm.md`
//! (`Terrain_ReadHlm` `0x0055C090`).

use crate::FormatError;
use crate::binary::Reader;

const RECORD: usize = 36;

#[derive(Debug, Clone, PartialEq)]
pub struct HlmPoint {
    /// `(SIZE_X * column, height, SIZE_Z * row)`.
    pub position: [f32; 3],
    /// Stored normal; the game recomputes it (`opennml_world::terrain`).
    pub normal: [f32; 3],
    /// `COLOR_GUID` of the project file; `None` for `0xFFFFFFFF` (white).
    pub color_guid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Heightmap {
    pub verts_x: usize,
    pub verts_z: usize,
    pub points: Vec<HlmPoint>,
}

impl Heightmap {
    /// Grid point `(x, z)`, `x < verts_x`, `z < verts_z`.
    pub fn point(&self, x: usize, z: usize) -> &HlmPoint {
        &self.points[z * self.verts_x + x]
    }
}

/// Reads an `.hlm` for a grid of `dim_x x dim_z` tiles (from `[TERRAIN]` of the `.wld`).
/// The file size must be exactly `(dim_x + 1) * (dim_z + 1) * 36` bytes.
pub fn parse_hlm(bytes: &[u8], dim_x: u32, dim_z: u32) -> Result<Heightmap, FormatError> {
    let verts_x = dim_x as usize + 1;
    let verts_z = dim_z as usize + 1;
    let expected = verts_x
        .checked_mul(verts_z)
        .and_then(|n| n.checked_mul(RECORD))
        .ok_or_else(|| FormatError::Invalid(format!("grid {dim_x} x {dim_z} too large")))?;
    if bytes.len() != expected {
        return Err(FormatError::Invalid(format!(
            ".hlm has {} bytes, expected {expected} for {dim_x} x {dim_z} tiles",
            bytes.len()
        )));
    }
    let mut r = Reader::new(bytes);
    let mut points = Vec::with_capacity(verts_x * verts_z);
    for _ in 0..verts_x * verts_z {
        let position = r.vec3("position")?;
        let normal = r.vec3("normal")?;
        // `unknown_18`/`unknown_1c`: read, never used in the terrain code.
        r.skip(8, "unknown_18/1c")?;
        let guid = r.u32("color_guid")?;
        points.push(HlmPoint {
            position,
            normal,
            color_guid: (guid != u32::MAX).then_some(guid),
        });
    }
    r.finish()?;
    Ok(Heightmap {
        verts_x,
        verts_z,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{flat_heightmap, write_hlm};

    #[test]
    fn hlm_roundtrip_and_indexing() {
        let mut map = flat_heightmap(2, 3, 128.0, -300.0);
        map.points[5].color_guid = Some(16);
        let parsed = parse_hlm(&write_hlm(&map), 2, 3).unwrap();
        assert_eq!(parsed, map);
        assert_eq!((parsed.verts_x, parsed.verts_z), (3, 4));
        assert_eq!(parsed.point(2, 1).position, [256.0, -300.0, 128.0]);
        assert_eq!(parsed.point(2, 1).color_guid, Some(16));
    }
}
