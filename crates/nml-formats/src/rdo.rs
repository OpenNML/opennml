//! Models `.rdo` and `.rdw` (container `0x7DB`), layout in `docs/formats/rdo.md`.
//! Read like `RdoFile_Read` (`0x005A20B0`); fields that the game does not evaluate
//! (exporter garbage, hull details, mesh scale) are skipped.

use crate::FormatError;
use crate::binary::{Reader, index};

/// Upper limit for meshes, bones, materials and children per file (shipped data: < 100).
pub const MAX_ITEMS: usize = 500;
/// Upper limit for vertices, triangles, UVs and influences per block (shipped data: < 20,000).
pub const MAX_ELEMENTS: usize = 1_000_000;

/// Standard (header `0x390`, mesh header 460 bytes) or legacy format of an earlier exporter
/// (header `0x314`, mesh header 732 bytes; `kanone_spa_stehen.rdo` and `bear.rdw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdoLayout {
    Standard,
    Legacy,
}

/// Mesh type: 3001 skin, 3002/3003 rigid, 3005 helper node without geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshKind {
    Skin,
    Rigid,
    Helper,
}

impl MeshKind {
    pub fn from_type_id(type_id: i32) -> Option<Self> {
        match type_id {
            3001 => Some(Self::Skin),
            3002 | 3003 => Some(Self::Rigid),
            3005 => Some(Self::Helper),
            _ => None,
        }
    }
}

/// Hull box (`RdoBounds`, 124 bytes); `min`/`max` are corner 0 and 7.
#[derive(Debug, Clone, PartialEq)]
pub struct Bounds {
    pub center: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// The 8 stored corners in file order (`RdoBounds+0x0C`). Mostly the
    /// min/max combinations (corner `i`: `i & 1` -> max.x, `i & 2` -> max.y, `i & 4` -> max.z), but in a
    /// different order for helper nodes; `ModelBounds_GetTileRect` (`0x00446B10`),
    /// `ModelInstance_GetMeshBoundingCorners` (`0x005887E0`) and
    /// `ModelInstance_GetHelperNodeBox` (`0x0058E790`) read all 8.
    pub corners: [[f32; 3]; 8],
    /// Circumscribed sphere radius (length of the half extent).
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub vertices: [u32; 3],
    /// Index into [`RdoFile::materials`].
    pub material: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RdoMesh {
    pub kind: MeshKind,
    /// Type identifier from the file (3001, 3002, 3003 or 3005).
    pub type_id: i32,
    /// Node name from 3ds Max; key for node and morph tracks.
    pub name: String,
    pub pivot: [f32; 3],
    /// Translation node space -> object space.
    pub position: [f32; 3],
    /// Rotation node space -> object space as quaternion `(w, x, y, z)`.
    pub rotation_wxyz: [f32; 4],
    /// Node scale `scale` (`RdoMesh+0x12C`, read `0x005A22CE`). For meshes with vertices it is
    /// included in the vertices (the static node matrix computes with `(1, 1, 1)`); the loader
    /// does not overwrite the field, though, and `Vector3_ApplyNodeTransform` (`0x00578180`) applies it
    /// to helper nodes - e.g. the hull `blocking01` of palme04...06 with 20/29 (D-365) and the
    /// hit box `hitbox` of the buildings, whose stored corners form a cube (D-367).
    pub scale: [f32; 3],
    /// Skin mesh: object space (bind pose); otherwise node space.
    pub positions: Vec<[f32; 3]>,
    pub faces: Vec<Face>,
    /// `(u, v)` as in 3ds Max; the game uses `(u, -v)`.
    pub uvs: Vec<[f32; 2]>,
    /// UV indices per triangle; `None` if the mesh has no UVs (the game sets UV `(0, 0)`).
    pub uv_faces: Option<Vec<[u32; 3]>>,
    pub face_normals: Vec<[f32; 3]>,
    pub vertex_normals: Vec<[f32; 3]>,
    pub bounds: Bounds,
}

impl RdoMesh {
    /// Runs of the same material ID in the triangle list as `(first triangle, count, material)`
    /// like `RdoMesh_BuildMaterialGroups` (`0x005A3B00`).
    pub fn material_runs(&self) -> Vec<(usize, usize, u32)> {
        let mut runs: Vec<(usize, usize, u32)> = Vec::new();
        for (i, face) in self.faces.iter().enumerate() {
            match runs.last_mut() {
                Some((_, count, material)) if *material == face.material => *count += 1,
                _ => runs.push((i, 1, face.material)),
            }
        }
        runs
    }
}

/// Weights of a bone on a mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct Influence {
    /// Index into [`RdoFile::meshes`].
    pub mesh: u32,
    /// Vertex index; `None` for values outside the mesh (the loader sets them to -1).
    pub vertices: Vec<Option<u32>>,
    pub weights: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RdoBone {
    /// Biped name, e.g. `bip01 l thigh`.
    pub name: String,
    /// Joint position in the bind pose (object space).
    pub rest_position: [f32; 3],
    pub children: Vec<Option<u32>>,
    pub influences: Vec<Influence>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RdoMaterial {
    pub name: String,
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    /// `[0, 1]`; alpha of the colors is `1 - transparency`.
    pub transparency: f32,
    /// Texture file as stored (empty = none); resolution in `nml_world::assets`.
    pub texture: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RdoFile {
    pub layout: RdoLayout,
    pub bounds: Bounds,
    pub meshes: Vec<RdoMesh>,
    pub bones: Vec<RdoBone>,
    pub materials: Vec<RdoMaterial>,
}

impl RdoFile {
    /// Influences `(bone, weight)` per vertex of a mesh like
    /// `RdoFile_BuildVertexInfluences` (`0x005A1B40`): at most 4 per vertex in read order
    /// (bone ascending), further ones are discarded.
    pub fn vertex_influences(&self, mesh: usize) -> Vec<Vec<(u32, f32)>> {
        let mut result = vec![Vec::new(); self.meshes[mesh].positions.len()];
        for (bone_index, bone) in self.bones.iter().enumerate() {
            for influence in bone.influences.iter().filter(|i| i.mesh as usize == mesh) {
                for (vertex, &weight) in influence.vertices.iter().zip(&influence.weights) {
                    if let Some(vertex) = vertex {
                        let list = &mut result[*vertex as usize];
                        if list.len() < 4 {
                            list.push((bone_index as u32, weight));
                        }
                    }
                }
            }
        }
        result
    }
}

/// Reads an `.rdo`/`.rdw`; first in the standard format, then in the legacy format. The file must end exactly
/// after the last material.
pub fn parse_rdo(bytes: &[u8]) -> Result<RdoFile, FormatError> {
    match parse_layout(bytes, RdoLayout::Standard) {
        Ok(file) => Ok(file),
        Err(standard) => parse_layout(bytes, RdoLayout::Legacy).map_err(|legacy| {
            FormatError::Invalid(format!(
                "neither standard format ({standard}) nor legacy format ({legacy})"
            ))
        }),
    }
}

struct MeshHeader {
    kind: MeshKind,
    type_id: i32,
    name: String,
    pivot: [f32; 3],
    position: [f32; 3],
    rotation_wxyz: [f32; 4],
    scale: [f32; 3],
    vertex_count: usize,
    face_count: usize,
    uv_count: usize,
    uv_face_count: usize,
    bounds: Bounds,
}

fn parse_layout(bytes: &[u8], layout: RdoLayout) -> Result<RdoFile, FormatError> {
    let mut r = Reader::new(bytes);
    // Identifier 0x7DB is not checked by the game; name and path are overwritten by the loader.
    r.skip(4 + 256 + 256, "identifier, name and path")?;
    let mesh_count = r.count(MAX_ITEMS, "mesh_count")?;
    let bone_count = r.count(MAX_ITEMS, "bone_count")?;
    let material_count = r.count(MAX_ITEMS, "material_count")?;
    r.skip(4, "exporter_animation_count")?;
    let bounds = match layout {
        RdoLayout::Standard => {
            let bounds = read_bounds(&mut r)?;
            r.skip(256, "reserved")?;
            bounds
        }
        RdoLayout::Legacy => {
            r.skip(132, "legacy format reserved")?;
            read_bounds(&mut r)?
        }
    };
    let headers = (0..mesh_count)
        .map(|_| read_mesh_header(&mut r, layout))
        .collect::<Result<Vec<_>, _>>()?;
    let meshes = headers
        .into_iter()
        .map(|header| read_mesh_data(&mut r, header, material_count))
        .collect::<Result<Vec<_>, _>>()?;
    let bones = read_bones(&mut r, bone_count, &meshes)?;
    let materials = (0..material_count)
        .map(|_| read_material(&mut r))
        .collect::<Result<Vec<_>, _>>()?;
    r.finish()?;
    Ok(RdoFile {
        layout,
        bounds,
        meshes,
        bones,
        materials,
    })
}

fn read_bounds(r: &mut Reader) -> Result<Bounds, FormatError> {
    let center = r.vec3("hull.center")?;
    let corners = r.floats::<3>(8, "hull.corners")?;
    let radius = r.f32("hull.radius")?;
    r.skip(12, "hull.half_extent")?;
    Ok(Bounds {
        center,
        min: corners[0],
        max: corners[7],
        corners: std::array::from_fn(|i| corners[i]),
        radius,
    })
}

fn read_mesh_header(r: &mut Reader, layout: RdoLayout) -> Result<MeshHeader, FormatError> {
    let offset = r.pos();
    let type_id = r.i32("mesh type")?;
    let kind = MeshKind::from_type_id(type_id).ok_or_else(|| {
        FormatError::Invalid(format!("unknown mesh type {type_id} at offset {offset:#x}"))
    })?;
    let name = r.fixed_str(256, "mesh name")?;
    let pivot = r.vec3("pivot")?;
    let position = r.vec3("position")?;
    let rotation_wxyz = r.vec4("rotation_wxyz")?;
    // Included for meshes with vertices; effective for helper nodes (D-365, [`RdoMesh::scale`]).
    let scale = r.vec3("scale")?;
    let vertex_count = r.count(MAX_ELEMENTS, "vertex_count")?;
    let face_count = r.count(MAX_ELEMENTS, "triangle_count")?;
    let uv_count = r.count(MAX_ELEMENTS, "uv_count")?;
    let uv_face_count = r.count(MAX_ELEMENTS, "uv_triangle_count")?;
    if layout == RdoLayout::Legacy {
        r.skip(28, "legacy format pointer")?;
    }
    let bounds = read_bounds(r)?;
    match layout {
        // `material` is never evaluated, `group_count` is computed here.
        RdoLayout::Standard => r.skip(8, "material and group_count")?,
        RdoLayout::Legacy => r.skip(252, "legacy format remainder")?,
    }
    Ok(MeshHeader {
        kind,
        type_id,
        name,
        pivot,
        position,
        rotation_wxyz,
        scale,
        vertex_count,
        face_count,
        uv_count,
        uv_face_count,
        bounds,
    })
}

fn read_mesh_data(
    r: &mut Reader,
    h: MeshHeader,
    material_count: usize,
) -> Result<RdoMesh, FormatError> {
    let positions = r.floats::<3>(h.vertex_count, "positions")?;
    let raw_faces = r.ints::<4>(h.face_count, "triangles")?;
    let uvs = r.floats::<2>(h.uv_count, "uvs")?;
    let raw_uv_faces = r.ints::<4>(h.uv_face_count, "uv_triangles")?;
    let face_normals = r.floats::<3>(h.face_count, "face_normals")?;
    let vertex_normals = r.floats::<3>(h.vertex_count, "vertex_normals")?;
    let what = |field: &str| format!("mesh \"{}\": {field}", h.name);
    let faces = raw_faces
        .iter()
        .map(|f| {
            Ok(Face {
                vertices: [
                    index(f[0], h.vertex_count, &what("vertex index"))?,
                    index(f[1], h.vertex_count, &what("vertex index"))?,
                    index(f[2], h.vertex_count, &what("vertex index"))?,
                ],
                material: index(f[3], material_count, &what("material ID"))?,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    let uv_faces = match h.uv_face_count {
        0 => None,
        n if n == h.face_count => Some(
            raw_uv_faces
                .iter()
                .map(|f| {
                    Ok([
                        index(f[0], h.uv_count, &what("UV index"))?,
                        index(f[1], h.uv_count, &what("UV index"))?,
                        index(f[2], h.uv_count, &what("UV index"))?,
                    ])
                })
                .collect::<Result<Vec<_>, FormatError>>()?,
        ),
        n => {
            return Err(FormatError::Invalid(what(&format!(
                "{n} UV triangles for {} triangles",
                h.face_count
            ))));
        }
    };
    Ok(RdoMesh {
        kind: h.kind,
        type_id: h.type_id,
        name: h.name,
        pivot: h.pivot,
        position: h.position,
        rotation_wxyz: h.rotation_wxyz,
        scale: h.scale,
        positions,
        faces,
        uvs,
        uv_faces,
        face_normals,
        vertex_normals,
        bounds: h.bounds,
    })
}

fn read_bones(
    r: &mut Reader,
    count: usize,
    meshes: &[RdoMesh],
) -> Result<Vec<RdoBone>, FormatError> {
    struct BoneHeader {
        name: String,
        rest_position: [f32; 3],
        child_count: usize,
        influence_count: usize,
    }
    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.fixed_str(256, "bone name")?;
        let rest_position = r.vec3("rest_position")?;
        let child_count = r.count(MAX_ITEMS, "child_count")?;
        let influence_count = r.count(MAX_ITEMS, "influence_count")?;
        r.skip(4, "has_weights")?; // exporter value, not reliable
        headers.push(BoneHeader {
            name,
            rest_position,
            child_count,
            influence_count,
        });
    }
    let mut bones = Vec::with_capacity(count);
    for h in headers {
        let children = r
            .i32s(h.child_count, "children")?
            .into_iter()
            .map(|c| match c {
                -1 => Ok(None),
                c => index(c, count, &format!("bone \"{}\": child index", h.name)).map(Some),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut records = Vec::with_capacity(h.influence_count);
        for _ in 0..h.influence_count {
            r.skip(4, "influence pointer")?;
            let mesh = r.i32("influence mesh")?;
            let len = r.count(MAX_ELEMENTS, "influence count")?;
            r.skip(12, "influence pointer")?;
            records.push((mesh, len));
        }
        let mut influences = Vec::with_capacity(records.len());
        for (mesh, len) in records {
            let mesh = index(mesh, meshes.len(), &format!("bone \"{}\": mesh", h.name))?;
            let vertex_count = meshes[mesh as usize].positions.len();
            // The loader sets values outside to -1 (`0x005A26D4`; 29 cases in the shipped data).
            let vertices = r
                .i32s(len, "influence vertices")?
                .into_iter()
                .map(|v| {
                    usize::try_from(v)
                        .ok()
                        .filter(|&v| v < vertex_count)
                        .map(|v| v as u32)
                })
                .collect();
            let weights = r.f32s(len, "influence weights")?;
            influences.push(Influence {
                mesh,
                vertices,
                weights,
            });
        }
        bones.push(RdoBone {
            name: h.name,
            rest_position: h.rest_position,
            children,
            influences,
        });
    }
    Ok(bones)
}

fn read_material(r: &mut Reader) -> Result<RdoMaterial, FormatError> {
    let name = r.fixed_str(256, "material name")?;
    let ambient = r.vec3("ambient")?;
    let diffuse = r.vec3("diffuse")?;
    let specular = r.vec3("specular")?;
    let transparency = r.f32("transparency")?;
    r.skip(16, "unevaluated")?;
    let texture = r.fixed_str(256, "texture")?;
    r.skip(4, "remainder")?;
    Ok(RdoMaterial {
        name,
        ambient,
        diffuse,
        specular,
        transparency,
        texture,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sample_rdo, write_rdo};

    #[test]
    fn standard_rdo_roundtrip() {
        let file = sample_rdo(RdoLayout::Standard);
        assert_eq!(parse_rdo(&write_rdo(&file)).unwrap(), file);
    }

    #[test]
    fn legacy_rdo_roundtrip() {
        let file = sample_rdo(RdoLayout::Legacy);
        let parsed = parse_rdo(&write_rdo(&file)).unwrap();
        assert_eq!(parsed.layout, RdoLayout::Legacy);
        assert_eq!(parsed, file);
    }

    #[test]
    fn out_of_range_influence_vertices_become_none() {
        let mut file = sample_rdo(RdoLayout::Standard);
        file.bones[0].influences[0].vertices[1] = Some(99);
        let parsed = parse_rdo(&write_rdo(&file)).unwrap();
        assert_eq!(parsed.bones[0].influences[0].vertices[1], None);
    }

    #[test]
    fn vertex_influences_keep_at_most_four() {
        let mut file = sample_rdo(RdoLayout::Standard);
        let template = file.bones[0].clone();
        file.bones = (0..5)
            .map(|i| RdoBone {
                name: format!("bip01 b{i}"),
                influences: vec![Influence {
                    mesh: 1,
                    vertices: vec![Some(0)],
                    weights: vec![0.2],
                }],
                ..template.clone()
            })
            .collect();
        let influences = file.vertex_influences(1);
        assert_eq!(influences[0], [(0, 0.2), (1, 0.2), (2, 0.2), (3, 0.2)]);
        assert!(influences[1].is_empty());
    }

    #[test]
    fn rdo_truncated_and_trailing_data_are_errors() {
        let bytes = write_rdo(&sample_rdo(RdoLayout::Standard));
        assert!(parse_rdo(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(parse_rdo(&longer).is_err());
        assert!(parse_rdo(&[]).is_err());
    }

    #[test]
    fn rdo_rejects_bad_indices_and_types() {
        let mut file = sample_rdo(RdoLayout::Standard);
        file.meshes[0].faces[0].vertices[2] = 3;
        assert!(parse_rdo(&write_rdo(&file)).is_err());
        let mut file = sample_rdo(RdoLayout::Standard);
        file.meshes[0].faces[0].material = 5;
        assert!(parse_rdo(&write_rdo(&file)).is_err());
        let mut file = sample_rdo(RdoLayout::Standard);
        file.meshes[0].type_id = 1234;
        assert!(parse_rdo(&write_rdo(&file)).is_err());
    }

    #[test]
    fn material_runs_group_consecutive_faces() {
        let mut mesh = sample_rdo(RdoLayout::Standard).meshes[1].clone();
        let face = |material| Face {
            vertices: [0, 1, 2],
            material,
        };
        mesh.faces = vec![face(0), face(0), face(1), face(0)];
        assert_eq!(mesh.material_runs(), [(0, 2, 0), (2, 1, 1), (3, 1, 0)]);
    }

    #[test]
    fn mesh_scale_is_read() {
        // `scale` (`+0x12C`): helper nodes like `blocking01` of palme04...06 carry 20/29 (D-365).
        let mut file = sample_rdo(RdoLayout::Standard);
        file.meshes[2].scale = [0.689_655_2, 0.5, 2.0];
        let parsed = parse_rdo(&write_rdo(&file)).unwrap();
        assert_eq!(parsed.meshes[2].scale, [0.689_655_2, 0.5, 2.0]);
        assert_eq!(parsed.meshes[0].scale, [1.0; 3]);
        let mut legacy = sample_rdo(RdoLayout::Legacy);
        legacy.meshes[1].scale = [3.0, 4.0, 5.0];
        let parsed = parse_rdo(&write_rdo(&legacy)).unwrap();
        assert_eq!(parsed.meshes[1].scale, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn bounds_keep_all_eight_stored_corners() {
        // Helper nodes store the corners in a different order (x, z, y): all 8 are preserved.
        let mut file = sample_rdo(RdoLayout::Standard);
        let corners: [[f32; 3]; 8] =
            std::array::from_fn(|i| [i as f32, -(i as f32), 0.5 * i as f32]);
        file.bounds.corners = corners;
        file.bounds.min = corners[0];
        file.bounds.max = corners[7];
        file.meshes[2].bounds.corners = corners;
        file.meshes[2].bounds.min = corners[0];
        file.meshes[2].bounds.max = corners[7];
        let parsed = parse_rdo(&write_rdo(&file)).unwrap();
        assert_eq!(parsed.bounds.corners, corners);
        assert_eq!(parsed.meshes[2].bounds.corners, corners);
        assert_eq!(
            (parsed.bounds.min, parsed.bounds.max),
            (corners[0], corners[7])
        );
    }
}
