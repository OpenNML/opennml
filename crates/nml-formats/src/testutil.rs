//! Writer for synthetic test files - counterpart to the parsers, for tests only (also in
//! other crates). Fill areas are filled with `0xCD`, as the exporter does.

use crate::hlm::{Heightmap, HlmPoint};
use crate::rdm::{
    BoneTrack, MorphFrame, MorphTrack, NodeTrack, PositionKey, RdmFile, RdmLayout, RotationKey,
    ScaleKey, TimeLabel,
};
use crate::rdo::{
    Bounds, Face, Influence, MeshKind, RdoBone, RdoFile, RdoLayout, RdoMaterial, RdoMesh,
};
use crate::tlm::TextureMap;

pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_f32s(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

pub(crate) fn put_junk(out: &mut Vec<u8>, len: usize) {
    out.extend(std::iter::repeat_n(0xCD, len));
}

/// Null-terminated cp1252 text in a fixed-length field, remainder `0xCD`.
pub(crate) fn put_str(out: &mut Vec<u8>, text: &str, len: usize) {
    let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(text);
    assert!(
        !unmappable && bytes.len() < len,
        "test text {text:?} does not fit"
    );
    out.extend_from_slice(&bytes);
    out.push(0);
    put_junk(out, len - bytes.len() - 1);
}

fn put_bounds(out: &mut Vec<u8>, b: &Bounds) {
    put_f32s(out, &b.center);
    for corner in &b.corners {
        put_f32s(out, corner);
    }
    put_f32s(out, &[b.radius]);
    let half: Vec<f32> = (0..3).map(|k| (b.max[k] - b.min[k]) / 2.0).collect();
    put_f32s(out, &half);
}

/// Writes an `.rdo` in the layout of `file.layout`.
pub fn write_rdo(file: &RdoFile) -> Vec<u8> {
    let legacy = file.layout == RdoLayout::Legacy;
    let mut out = Vec::new();
    put_u32(&mut out, 0x7DB);
    put_str(&mut out, "test.rdo", 256);
    put_str(&mut out, r".\data\r3d\test.rdo", 256);
    for n in [file.meshes.len(), file.bones.len(), file.materials.len(), 0] {
        put_i32(&mut out, n as i32);
    }
    if legacy {
        put_junk(&mut out, 132);
        put_bounds(&mut out, &file.bounds);
    } else {
        put_bounds(&mut out, &file.bounds);
        put_junk(&mut out, 256);
    }
    for mesh in &file.meshes {
        put_i32(&mut out, mesh.type_id);
        put_str(&mut out, &mesh.name, 256);
        put_f32s(&mut out, &mesh.pivot);
        put_f32s(&mut out, &mesh.position);
        put_f32s(&mut out, &mesh.rotation_wxyz);
        put_f32s(&mut out, &mesh.scale);
        let uv_faces = mesh.uv_faces.as_ref().map_or(0, Vec::len);
        for n in [
            mesh.positions.len(),
            mesh.faces.len(),
            mesh.uvs.len(),
            uv_faces,
        ] {
            put_i32(&mut out, n as i32);
        }
        if legacy {
            put_junk(&mut out, 28);
        }
        put_bounds(&mut out, &mesh.bounds);
        if legacy {
            put_junk(&mut out, 252);
        } else {
            put_i32(&mut out, -1);
            put_i32(&mut out, -1);
        }
    }
    for mesh in &file.meshes {
        for p in &mesh.positions {
            put_f32s(&mut out, p);
        }
        for f in &mesh.faces {
            for v in f.vertices {
                put_i32(&mut out, v as i32);
            }
            put_i32(&mut out, f.material as i32);
        }
        for uv in &mesh.uvs {
            put_f32s(&mut out, uv);
        }
        for f in mesh.uv_faces.iter().flatten() {
            for t in f {
                put_i32(&mut out, *t as i32);
            }
            put_i32(&mut out, -1);
        }
        for n in &mesh.face_normals {
            put_f32s(&mut out, n);
        }
        for n in &mesh.vertex_normals {
            put_f32s(&mut out, n);
        }
    }
    for bone in &file.bones {
        put_str(&mut out, &bone.name, 256);
        put_f32s(&mut out, &bone.rest_position);
        put_i32(&mut out, bone.children.len() as i32);
        put_i32(&mut out, bone.influences.len() as i32);
        put_junk(&mut out, 4);
    }
    for bone in &file.bones {
        for child in &bone.children {
            put_i32(&mut out, child.map_or(-1, |c| c as i32));
        }
        for influence in &bone.influences {
            put_junk(&mut out, 4);
            put_i32(&mut out, influence.mesh as i32);
            put_i32(&mut out, influence.vertices.len() as i32);
            put_junk(&mut out, 12);
        }
        for influence in &bone.influences {
            for v in &influence.vertices {
                put_i32(&mut out, v.map_or(-1, |v| v as i32));
            }
            put_f32s(&mut out, &influence.weights);
        }
    }
    for m in &file.materials {
        put_str(&mut out, &m.name, 256);
        put_f32s(&mut out, &m.ambient);
        put_f32s(&mut out, &m.diffuse);
        put_f32s(&mut out, &m.specular);
        put_f32s(&mut out, &[m.transparency, 1.0, 1.0, 0.0, 0.0]);
        put_str(&mut out, &m.texture, 256);
        put_junk(&mut out, 4);
    }
    out
}

/// Bounding box from minimum and maximum (for test data); corner `i` = (`i & 1` -> max.x, `i & 2` ->
/// max.y, `i & 4` -> max.z).
pub fn bounds(min: [f32; 3], max: [f32; 3]) -> Bounds {
    let center = std::array::from_fn(|k| (min[k] + max[k]) / 2.0);
    let half: [f32; 3] = std::array::from_fn(|k| (max[k] - min[k]) / 2.0);
    let pick = |i: usize, bit: usize, k: usize| if i & bit != 0 { max[k] } else { min[k] };
    Bounds {
        center,
        min,
        max,
        corners: std::array::from_fn(|i| [pick(i, 1, 0), pick(i, 2, 1), pick(i, 4, 2)]),
        radius: (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt(),
    }
}

/// Small model for tests:
/// - mesh 0 `kiste`: rigid (3002), 1 triangle with UVs, node position `(5, 0, 5)`, material 0;
/// - mesh 1 `koerper`: skin (3001), 4 vertices, 2 triangles without UVs, material 1;
/// - mesh 2 `zauber01`: helper node (3005) at `(0, 120, 0)`;
/// - bone 0 `bip01 pelvis` (rest position `(10, 50, 0)`), weight 1 on all vertices of mesh 1;
/// - material 0 `EF-MATERIAL` with texture `holz.tga`, material 1 `Standard` without texture,
///   transparency 0.5.
pub fn sample_rdo(layout: RdoLayout) -> RdoFile {
    let kiste = RdoMesh {
        kind: MeshKind::Rigid,
        type_id: 3002,
        name: "kiste".into(),
        pivot: [5.0, 0.0, 5.0],
        position: [5.0, 0.0, 5.0],
        rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
        scale: [1.0; 3],
        positions: vec![[0.0, 0.0, 0.0], [0.0, 10.0, 0.0], [10.0, 0.0, 0.0]],
        faces: vec![Face {
            vertices: [0, 1, 2],
            material: 0,
        }],
        uvs: vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]],
        uv_faces: Some(vec![[0, 1, 2]]),
        face_normals: vec![[0.0, 0.0, -1.0]],
        vertex_normals: vec![[0.0, 0.0, -1.0]; 3],
        bounds: bounds([0.0, 0.0, 0.0], [10.0, 10.0, 0.0]),
    };
    let koerper = RdoMesh {
        kind: MeshKind::Skin,
        type_id: 3001,
        name: "koerper".into(),
        pivot: [0.0; 3],
        position: [0.0; 3],
        rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
        scale: [1.0; 3],
        positions: vec![
            [0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0],
            [20.0, 0.0, 0.0],
            [20.0, 100.0, 0.0],
        ],
        faces: vec![
            Face {
                vertices: [0, 1, 2],
                material: 1,
            },
            Face {
                vertices: [1, 3, 2],
                material: 1,
            },
        ],
        uvs: vec![],
        uv_faces: None,
        face_normals: vec![[0.0, 0.0, -1.0]; 2],
        vertex_normals: vec![[0.0, 0.0, -1.0]; 4],
        bounds: bounds([0.0, 0.0, 0.0], [20.0, 100.0, 0.0]),
    };
    let zauber = RdoMesh {
        kind: MeshKind::Helper,
        type_id: 3005,
        name: "zauber01".into(),
        pivot: [0.0, 120.0, 0.0],
        position: [0.0, 120.0, 0.0],
        rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
        scale: [1.0; 3],
        positions: vec![],
        faces: vec![],
        uvs: vec![],
        uv_faces: None,
        face_normals: vec![],
        vertex_normals: vec![],
        bounds: bounds([0.0; 3], [0.0; 3]),
    };
    let pelvis = RdoBone {
        name: "bip01 pelvis".into(),
        rest_position: [10.0, 50.0, 0.0],
        children: vec![None],
        influences: vec![Influence {
            mesh: 1,
            vertices: vec![Some(0), Some(1), Some(2), Some(3)],
            weights: vec![1.0; 4],
        }],
    };
    let material = |name: &str, texture: &str, transparency: f32| RdoMaterial {
        name: name.into(),
        ambient: [0.5, 0.5, 0.5],
        diffuse: [0.8, 0.6, 0.4],
        specular: [0.0; 3],
        transparency,
        texture: texture.into(),
    };
    RdoFile {
        layout,
        bounds: bounds([0.0, 0.0, 0.0], [20.0, 120.0, 10.0]),
        meshes: vec![kiste, koerper, zauber],
        bones: vec![pelvis],
        materials: vec![
            material("EF-MATERIAL", "holz.tga", 0.0),
            material("Standard", "", 0.5),
        ],
    }
}

fn put_position_keys(out: &mut Vec<u8>, keys: &[PositionKey]) {
    for k in keys {
        put_u32(out, k.time);
        put_f32s(out, &k.value);
    }
}

fn put_rotation_keys(out: &mut Vec<u8>, keys: &[RotationKey]) {
    for k in keys {
        put_u32(out, k.time);
        put_f32s(out, &k.wxyz);
    }
}

fn put_node_header(
    out: &mut Vec<u8>,
    name: &str,
    pivot: &[f32; 3],
    position: &[f32; 3],
    rotation: &[f32; 4],
    scale: &[f32; 3],
) {
    put_str(out, name, 256);
    put_f32s(out, pivot);
    put_f32s(out, position);
    put_f32s(out, rotation);
    put_f32s(out, scale);
}

/// Writes an `.rdm` in the layout of `file.layout` (time marker capacity 20).
pub fn write_rdm(file: &RdmFile) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, 0x3EA);
    put_str(&mut out, "test.rdm", 256);
    put_str(&mut out, r".\data\rdm\test.rdm", 256);
    put_u32(&mut out, file.length_ticks);
    put_u32(&mut out, file.ticks_per_frame);
    put_u32(&mut out, file.fps);
    put_f32s(&mut out, &[file.time_factor]);
    put_u32(&mut out, file.start_frame);
    put_u32(&mut out, file.looping as u32);
    put_u32(&mut out, file.interp_first_last as u32);
    let (a, b, c) = (
        file.bone_tracks.len() as i32,
        file.node_tracks.len() as i32,
        file.morph_tracks.len() as i32,
    );
    match file.layout {
        RdmLayout::Standard => {
            for n in [a, b, c, 20, file.labels.len() as i32] {
                put_i32(&mut out, n);
            }
            put_junk(&mut out, 256);
        }
        RdmLayout::Legacy => {
            for n in [a, b, c] {
                put_i32(&mut out, n);
                put_junk(&mut out, 4);
            }
        }
    }
    for t in &file.bone_tracks {
        put_str(&mut out, &t.name, 256);
        put_f32s(&mut out, &t.rest_position);
        put_i32(&mut out, t.children.len() as i32);
        put_junk(&mut out, 4);
        put_i32(&mut out, t.positions.len() as i32);
        put_i32(&mut out, t.rotations.len() as i32);
        put_junk(&mut out, 8);
        put_i32(&mut out, 0);
        put_junk(&mut out, 4);
    }
    for t in &file.bone_tracks {
        for c in &t.children {
            put_i32(&mut out, c.map_or(-1, |c| c as i32));
        }
        put_position_keys(&mut out, &t.positions);
        put_rotation_keys(&mut out, &t.rotations);
    }
    for t in &file.node_tracks {
        put_node_header(
            &mut out,
            &t.name,
            &t.pivot,
            &t.position,
            &t.rotation_wxyz,
            &t.scale,
        );
        for n in [t.positions.len(), t.rotations.len(), t.scales.len()] {
            put_i32(&mut out, n as i32);
        }
        for n in [t.positions.len(), t.rotations.len(), t.scales.len()] {
            put_i32(&mut out, (n > 0) as i32);
        }
        put_junk(&mut out, 12);
        out.extend_from_slice(&[0; 24]);
    }
    for t in &file.node_tracks {
        put_position_keys(&mut out, &t.positions);
        put_rotation_keys(&mut out, &t.rotations);
        for k in &t.scales {
            put_u32(&mut out, k.time);
            put_f32s(&mut out, &k.scale);
            put_f32s(&mut out, &k.axis_wxyz);
        }
    }
    for t in &file.morph_tracks {
        put_node_header(
            &mut out,
            &t.name,
            &t.pivot,
            &t.position,
            &t.rotation_wxyz,
            &t.scale,
        );
        put_i32(&mut out, t.frames.len() as i32);
        put_junk(&mut out, 4);
    }
    for t in &file.morph_tracks {
        for f in &t.frames {
            put_u32(&mut out, f.time);
            put_i32(&mut out, f.positions.len() as i32);
            put_junk(&mut out, 8);
        }
        for f in &t.frames {
            for p in &f.positions {
                put_f32s(&mut out, p);
            }
            for n in &f.normals {
                put_f32s(&mut out, n);
            }
        }
    }
    if file.layout == RdmLayout::Standard {
        for l in &file.labels {
            put_u32(&mut out, l.id);
            put_str(&mut out, &l.name, 256);
            put_i32(&mut out, l.frame);
            put_u32(&mut out, l.time_ms);
        }
    }
    out
}

/// Animation matching [`sample_rdo`], 30 frames long (4,800 ticks), loop:
/// - bone track `Bip01 Pelvis`: position `(10, 50, 0)` -> `(10, 60, 0)`, rotation identity ->
///   90 degrees about y (`(w, x, y, z)` = `(cos 45 degrees, 0, sin 45 degrees, 0)`);
/// - node track `kiste`: pivot 0, scale 1, position `(5, 0, 5)` -> `(5, 20, 5)`;
/// - morph track `kiste`: two frames (time 0 and 4,800) with 3 vertices;
/// - time marker 1 `EndLabel` at frame 30 (default layout; legacy format without time markers).
pub fn sample_rdm(layout: RdmLayout) -> RdmFile {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let rotations = vec![
        RotationKey {
            time: 0,
            wxyz: [1.0, 0.0, 0.0, 0.0],
        },
        RotationKey {
            time: 4800,
            wxyz: [half, 0.0, half, 0.0],
        },
    ];
    let frame = |time: u32, y: f32| MorphFrame {
        time,
        positions: vec![[0.0, y, 0.0], [0.0, 10.0 + y, 0.0], [10.0, y, 0.0]],
        normals: vec![[0.0, 0.0, -1.0]; 3],
    };
    RdmFile {
        layout,
        length_ticks: 4800,
        ticks_per_frame: 160,
        fps: 30,
        time_factor: 1.0,
        start_frame: 0,
        looping: true,
        interp_first_last: false,
        bone_tracks: vec![BoneTrack {
            name: "Bip01 Pelvis".into(),
            rest_position: [10.0, 50.0, 0.0],
            children: vec![None],
            positions: vec![
                PositionKey {
                    time: 0,
                    value: [10.0, 50.0, 0.0],
                },
                PositionKey {
                    time: 4800,
                    value: [10.0, 60.0, 0.0],
                },
            ],
            rotations: rotations.clone(),
        }],
        node_tracks: vec![NodeTrack {
            name: "kiste".into(),
            pivot: [0.0; 3],
            position: [5.0, 0.0, 5.0],
            rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
            scale: [1.0; 3],
            positions: vec![
                PositionKey {
                    time: 0,
                    value: [5.0, 0.0, 5.0],
                },
                PositionKey {
                    time: 4800,
                    value: [5.0, 20.0, 5.0],
                },
            ],
            rotations,
            scales: vec![ScaleKey {
                time: 0,
                scale: [1.0; 3],
                axis_wxyz: [1.0, 0.0, 0.0, 0.0],
            }],
        }],
        morph_tracks: vec![MorphTrack {
            name: "kiste".into(),
            pivot: [0.0; 3],
            position: [5.0, 0.0, 5.0],
            rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
            scale: [1.0; 3],
            frames: vec![frame(0, 0.0), frame(4800, 30.0)],
        }],
        labels: match layout {
            RdmLayout::Standard => vec![TimeLabel {
                id: 1,
                name: "EndLabel".into(),
                frame: 30,
                time_ms: 1000,
            }],
            RdmLayout::Legacy => vec![],
        },
    }
}

/// Writes an `.hlm` (fields `0x18`/`0x1C` = 0).
pub fn write_hlm(map: &Heightmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.points.len() * 36);
    for p in &map.points {
        put_f32s(&mut out, &p.position);
        put_f32s(&mut out, &p.normal);
        put_u32(&mut out, 0);
        put_u32(&mut out, 0);
        put_u32(&mut out, p.color_guid.unwrap_or(u32::MAX));
    }
    out
}

/// Writes a `.tlm`.
pub fn write_tlm(map: &TextureMap) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.guids.len() * 4);
    for &g in &map.guids {
        put_u32(&mut out, g);
    }
    out
}

/// Flat grid with `(dim_x + 1) * (dim_z + 1)` points at height `height`, normal pointing up,
/// without color.
pub fn flat_heightmap(dim_x: u32, dim_z: u32, size: f32, height: f32) -> Heightmap {
    let (verts_x, verts_z) = (dim_x as usize + 1, dim_z as usize + 1);
    let points = (0..verts_z)
        .flat_map(|z| {
            (0..verts_x).map(move |x| HlmPoint {
                position: [x as f32 * size, height, z as f32 * size],
                normal: [0.0, 1.0, 0.0],
                color_guid: None,
            })
        })
        .collect();
    Heightmap {
        verts_x,
        verts_z,
        points,
    }
}
