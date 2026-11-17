//! Models as glTF: meshes with materials and textures, skeleton with weights and the
//! animations of the object `.cfg` (bone and node tracks). Morph tracks are not
//! exported (vertex animation; viewer only).

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::{Affine3A, Mat4, Quat, Vec3};
use nml_formats::rdm::{PositionKey, RdmFile, RotationKey, bone_track_key};
use nml_formats::rdo::{MeshKind, RdoMaterial, RdoMesh};
use nml_rda::Vfs;
use nml_world::anim::skin_weights;
use nml_world::assets::{ModelAsset, load_texture};
use nml_world::math::{game_quat, mesh_node_matrix};
use serde_json::{Value, json};

use crate::glb::GltfBuilder;
use crate::mirror;
use crate::srgb_to_linear;

/// Textures and materials of an output file; each texture is embedded only once.
#[derive(Default)]
pub(crate) struct MaterialCache {
    textures: HashMap<String, Option<usize>>,
    materials: HashMap<(String, usize), usize>,
}

impl MaterialCache {
    fn texture(&mut self, b: &mut GltfBuilder, vfs: &Vfs, path: &str) -> Option<usize> {
        if let Some(&index) = self.textures.get(path) {
            return index;
        }
        let index = match load_texture(vfs, path) {
            Ok(img) => Some(b.texture_png(&img)),
            Err(e) => {
                log::warn!("{e:#}");
                None
            }
        };
        self.textures.insert(path.to_string(), index);
        index
    }

    /// glTF material per material of the model (index as in `RdoFile::materials`).
    pub(crate) fn materials(
        &mut self,
        b: &mut GltfBuilder,
        vfs: &Vfs,
        model: &ModelAsset,
    ) -> Vec<usize> {
        let mut result = Vec::with_capacity(model.rdo.materials.len());
        for (i, (material, texture)) in model
            .rdo
            .materials
            .iter()
            .zip(&model.material_textures)
            .enumerate()
        {
            let key = (model.path.clone(), i);
            if let Some(&index) = self.materials.get(&key) {
                result.push(index);
                continue;
            }
            let texture = texture
                .as_deref()
                .and_then(|path| self.texture(b, vfs, path));
            let index = b.material(material_json(material, texture));
            self.materials.insert(key, index);
            result.push(index);
        }
        result
    }
}

/// Material as in the game (`ModelInstance_DrawOpaqueGroup`, re/renderer): color from the
/// `.rdo` only with `EF-MATERIAL`, otherwise white; alpha `1 - transparency`. Opaque groups with
/// alpha test `GREATER 0` -> `MASK`, transparent ones -> `BLEND`. The RGB part is converted from
/// sRGB to linear (glTF `baseColorFactor` is linear, the game computes in sRGB), alpha stays.
pub(crate) fn material_json(material: &RdoMaterial, texture: Option<usize>) -> Value {
    let rgb = if material.name.eq_ignore_ascii_case("EF-MATERIAL") {
        material.diffuse
    } else {
        [1.0; 3]
    };
    let rgb = rgb.map(srgb_to_linear);
    let alpha = 1.0 - material.transparency;
    let mut pbr = json!({
        "baseColorFactor": [rgb[0], rgb[1], rgb[2], alpha],
        "metallicFactor": 0.0,
        "roughnessFactor": 1.0,
    });
    if let Some(texture) = texture {
        pbr["baseColorTexture"] = json!({ "index": texture });
    }
    let mut result = json!({ "name": material.name, "pbrMetallicRoughness": pbr });
    if material.transparency > 0.0 {
        result["alphaMode"] = json!("BLEND");
    } else {
        result["alphaMode"] = json!("MASK");
        result["alphaCutoff"] = json!(0.5 / 255.0);
    }
    result
}

/// Normal of a triangle corner: vertex normal, for invalid values (NaN, length 0) the face
/// normal, otherwise up; normalized and mirrored.
fn corner_normal(mesh: &RdoMesh, face: usize, vertex: usize) -> [f32; 3] {
    let valid = |n: [f32; 3]| {
        let v = Vec3::from(n);
        (v.is_finite() && v.length() > 1e-6).then(|| v.normalize())
    };
    let normal = valid(mesh.vertex_normals[vertex])
        .or_else(|| valid(mesh.face_normals[face]))
        .unwrap_or(Vec3::Y);
    mirror::point(normal)
}

/// glTF mesh with one primitive per material. As in the game, three vertices per triangle;
/// UV `(u, -v)`.
pub(crate) fn mesh_json(
    b: &mut GltfBuilder,
    mesh: &RdoMesh,
    materials: &[usize],
    skin: Option<&[([u16; 4], [f32; 4])]>,
) -> Value {
    let mut by_material: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, face) in mesh.faces.iter().enumerate() {
        by_material.entry(face.material).or_default().push(i);
    }
    let primitives: Vec<Value> = by_material
        .into_iter()
        .map(|(material, faces)| {
            let (mut positions, mut normals, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
            let (mut joints, mut weights) = (Vec::new(), Vec::new());
            for &f in &faces {
                let face = &mesh.faces[f];
                for k in [0, 2, 1] {
                    let v = face.vertices[k] as usize;
                    positions.push(mirror::point(Vec3::from(mesh.positions[v])));
                    normals.push(corner_normal(mesh, f, v));
                    uvs.push(match &mesh.uv_faces {
                        Some(uv_faces) => {
                            let [u, vv] = mesh.uvs[uv_faces[f][k] as usize];
                            [u, -vv]
                        }
                        None => [0.0, 0.0],
                    });
                    if let Some(skin) = skin {
                        joints.push(skin[v].0);
                        weights.push(skin[v].1);
                    }
                }
            }
            let mut attributes = json!({
                "POSITION": b.positions(&positions),
                "NORMAL": b.attribute(&normals),
                "TEXCOORD_0": b.attribute(&uvs),
            });
            if skin.is_some() {
                attributes["JOINTS_0"] = json!(b.joints(&joints));
                attributes["WEIGHTS_0"] = json!(b.attribute(&weights));
            }
            json!({ "attributes": attributes, "material": materials[material as usize], "mode": 4 })
        })
        .collect();
    json!({ "name": mesh.name, "primitives": primitives })
}

fn trs_node(name: &str, game: Affine3A) -> Value {
    let (translation, rotation, scale) = mirror::trs(game);
    json!({ "name": name, "translation": translation, "rotation": rotation, "scale": scale })
}

/// Seconds of a key (`RdmFile::ticks_to_ms`).
fn seconds(anim: &RdmFile, ticks: u32) -> f32 {
    anim.ticks_to_ms(ticks) / 1000.0
}

/// Keys with strictly increasing times (required by glTF): for equal times the last one is used,
/// decreasing times are discarded.
fn strictly_increasing<T>(keys: impl Iterator<Item = (f32, T)>) -> (Vec<f32>, Vec<T>) {
    let (mut times, mut values): (Vec<f32>, Vec<T>) = (Vec::new(), Vec::new());
    for (t, v) in keys {
        match times.last() {
            Some(&last) if t == last => *values.last_mut().expect("same length") = v,
            Some(&last) if t < last => {}
            _ => {
                times.push(t);
                values.push(v);
            }
        }
    }
    (times, values)
}

fn position_keys(anim: &RdmFile, keys: &[PositionKey]) -> (Vec<f32>, Vec<[f32; 3]>) {
    strictly_increasing(
        keys.iter()
            .map(|k| (seconds(anim, k.time), mirror::point(Vec3::from(k.value)))),
    )
}

/// Rotation keys mirrored, with the same sign as the predecessor (shortest path).
fn rotation_keys(anim: &RdmFile, keys: &[RotationKey]) -> (Vec<f32>, Vec<[f32; 4]>) {
    let mut previous: Option<Quat> = None;
    strictly_increasing(keys.iter().map(|k| {
        let mut q = mirror::quat(game_quat(k.wxyz));
        if previous.is_some_and(|p| p.dot(q) < 0.0) {
            q = -q;
        }
        previous = Some(q);
        (seconds(anim, k.time), q.to_array())
    }))
}

/// Collects samplers and channels of a glTF animation.
#[derive(Default)]
struct AnimationParts {
    samplers: Vec<Value>,
    channels: Vec<Value>,
}

impl AnimationParts {
    fn add<const N: usize>(
        &mut self,
        b: &mut GltfBuilder,
        node: usize,
        path: &str,
        (times, values): (Vec<f32>, Vec<[f32; N]>),
    ) {
        if times.is_empty() {
            return;
        }
        let input = b.times(&times);
        let output = b.keyframes(&values);
        self.samplers
            .push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
        self.channels.push(json!({
            "sampler": self.samplers.len() - 1,
            "target": { "node": node, "path": path },
        }));
    }
}

/// Exports a model with skeleton and animations as `.glb`.
///
/// Deliberate limits (plausible, not faithful to the original; the viewer is the faithful
/// reference):
/// - Materials are unlit PBR with a base color (`metallic 0`, `roughness 1`); glTF does not know
///   the lighting and material model of the original.
/// - Morph tracks (vertex animation) are not exported.
/// - There is neither the crossfade at the loop end (`interp_erster_letzter`) nor the 200 ms
///   crossfade between animations.
/// - The `#pivot` offset is set only in animations with a node track; programs that do not
///   return to the rest position when the animation changes keep showing it.
/// - Node tracks of helper nodes (3005) are not animated.
///
/// Structure: one root node per bone in the bind pose (`translation` = rest position,
/// `inverseBindMatrix` = `T(-rest)`, i.e. `v' = R*(v - rest) + p` as in `blendingN.vsh`);
/// per rigid mesh a node with the node matrix and below it a child `<name>#pivot` that takes the
/// pivot offset of a node track (`v' = ((v - X∘S)*(S(t)⊘S))*R(q) + p`); skin meshes
/// in object space with skin; helper nodes (3005) as empty nodes.
pub fn model_to_glb(vfs: &Vfs, model: &ModelAsset) -> Result<Vec<u8>> {
    let mut b = GltfBuilder::new();
    let mut cache = MaterialCache::default();
    let materials = cache.materials(&mut b, vfs, model);
    let rdo = &model.rdo;
    let mut roots = Vec::new();

    let joints: Vec<usize> = rdo
        .bones
        .iter()
        .map(|bone| {
            b.node(json!({
                "name": bone.name,
                "translation": mirror::point(Vec3::from(bone.rest_position)),
            }))
        })
        .collect();
    roots.extend(&joints);
    let skin = (!joints.is_empty()).then(|| {
        let inverse_bind: Vec<[f32; 16]> = rdo
            .bones
            .iter()
            .map(|bone| {
                let rest = Vec3::from(mirror::point(Vec3::from(bone.rest_position)));
                Mat4::from_translation(-rest).to_cols_array()
            })
            .collect();
        let inverse_bind = b.matrices(&inverse_bind);
        b.skin(json!({ "joints": joints, "inverseBindMatrices": inverse_bind }))
    });

    let mut rigid: HashMap<usize, (usize, usize)> = HashMap::new();
    for (i, mesh) in rdo.meshes.iter().enumerate() {
        match mesh.kind {
            MeshKind::Helper => roots.push(b.node(trs_node(&mesh.name, mesh_node_matrix(mesh)))),
            MeshKind::Rigid => {
                let mut pivot = json!({ "name": format!("{}#pivot", mesh.name) });
                if !mesh.faces.is_empty() {
                    let gltf_mesh = mesh_json(&mut b, mesh, &materials, None);
                    pivot["mesh"] = json!(b.mesh(gltf_mesh));
                }
                let pivot = b.node(pivot);
                let node = b.node(trs_node(&mesh.name, mesh_node_matrix(mesh)));
                b.add_children(node, &[pivot]);
                roots.push(node);
                rigid.insert(i, (node, pivot));
            }
            MeshKind::Skin => {
                if mesh.faces.is_empty() {
                    continue;
                }
                let weights = skin.map(|_| skin_weights(rdo, i));
                let gltf_mesh = mesh_json(&mut b, mesh, &materials, weights.as_deref());
                let mut node = json!({ "name": mesh.name, "mesh": b.mesh(gltf_mesh) });
                if let Some(skin) = skin {
                    node["skin"] = json!(skin);
                }
                roots.push(b.node(node));
            }
        }
    }

    for (entry, anim) in model.config.animations.iter().zip(&model.animations) {
        let (Some(file), Some(anim)) = (entry.file.as_deref(), anim) else {
            continue;
        };
        let mut parts = AnimationParts::default();
        // Bone tracks: matched by the name without the first word (`RdmFile_FindBoneTrack`).
        for (bone, &joint) in rdo.bones.iter().zip(&joints) {
            let key = bone_track_key(&bone.name);
            let Some(track) = anim
                .bone_tracks
                .iter()
                .find(|t| bone_track_key(&t.name) == key)
            else {
                continue;
            };
            parts.add(
                &mut b,
                joint,
                "translation",
                position_keys(anim, &track.positions),
            );
            parts.add(
                &mut b,
                joint,
                "rotation",
                rotation_keys(anim, &track.rotations),
            );
        }
        // Node tracks of rigid meshes: exact name (`RdoFile_FindMeshByName`).
        for (i, mesh) in rdo.meshes.iter().enumerate() {
            let Some(&(node, pivot)) = rigid.get(&i) else {
                continue;
            };
            let Some(track) = anim.node_tracks.iter().find(|t| t.name == mesh.name) else {
                continue;
            };
            let rest_scale = Vec3::from(track.scale);
            parts.add(
                &mut b,
                node,
                "translation",
                position_keys(anim, &track.positions),
            );
            parts.add(
                &mut b,
                node,
                "rotation",
                rotation_keys(anim, &track.rotations),
            );
            let scales = strictly_increasing(track.scales.iter().map(|k| {
                let s = Vec3::from(k.scale);
                let ratio = Vec3::select(rest_scale.cmpeq(Vec3::ZERO), Vec3::ONE, s / rest_scale);
                (seconds(anim, k.time), ratio.to_array())
            }));
            parts.add(&mut b, node, "scale", scales);
            let offset = mirror::point(-(Vec3::from(track.pivot) * rest_scale));
            let end = anim.duration_ms() / 1000.0;
            let times = if end > 0.0 { vec![0.0, end] } else { vec![0.0] };
            let values = vec![offset; times.len()];
            parts.add(&mut b, pivot, "translation", (times, values));
        }
        if parts.channels.is_empty() {
            continue;
        }
        let name = file
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file)
            .trim_end_matches(".rdm");
        b.animation(
            json!({ "name": name, "samplers": parts.samplers, "channels": parts.channels }),
        );
    }
    Ok(b.finish(&roots))
}

/// Meshes of a model in the bind pose without skeleton, each part with its transformation in
/// object space (game coordinates): rigid meshes with node matrix, skin meshes with identity
/// matrix. For level exports in which many objects use the same meshes.
pub(crate) fn static_parts(
    b: &mut GltfBuilder,
    cache: &mut MaterialCache,
    vfs: &Vfs,
    model: &ModelAsset,
) -> Vec<(usize, Affine3A)> {
    let materials = cache.materials(b, vfs, model);
    model
        .rdo
        .meshes
        .iter()
        .filter(|m| m.kind != MeshKind::Helper && !m.faces.is_empty())
        .map(|mesh| {
            let local = match mesh.kind {
                MeshKind::Rigid => mesh_node_matrix(mesh),
                _ => Affine3A::IDENTITY,
            };
            let gltf_mesh = mesh_json(b, mesh, &materials, None);
            (b.mesh(gltf_mesh), local)
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nml_formats::rdm::RdmLayout;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{sample_rdm, sample_rdo, write_rdm, write_rdo};
    use nml_rda::testutil::{TestFile, build_rda};
    use nml_world::assets::ModelCache;

    /// Game directory with an archive of the given files.
    pub(crate) fn game_dir(files: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<TestFile> = files
            .iter()
            .map(|(path, data)| TestFile {
                path,
                data: data.as_slice(),
                compress: true,
            })
            .collect();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&entries)).unwrap();
        dir
    }

    /// 24-bit TGA 1 x 1 in the color `rgb`.
    pub(crate) fn tga(rgb: [u8; 3]) -> Vec<u8> {
        let mut bytes = vec![0u8; 18];
        bytes[2] = 2;
        bytes[12] = 1;
        bytes[14] = 1;
        bytes[16] = 24;
        bytes.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        bytes
    }

    fn sample_model_dir() -> tempfile::TempDir {
        let cfg = "OBJECT\r\n{\r\n\tANIMATION\r\n\t{\r\n\t\tFILE \".\\data\\rdm\\laufen.rdm\"\r\n\t}\r\n}\r\n";
        game_dir(&[
            (
                "data/r3d/test/kiste.rdo",
                write_rdo(&sample_rdo(RdoLayout::Standard)),
            ),
            ("data/r3d/test/kiste.cfg", cfg.as_bytes().to_vec()),
            (
                "data/rdm/laufen.rdm",
                write_rdm(&sample_rdm(RdmLayout::Standard)),
            ),
            ("data/r3d/test/holz.tga", tga([200, 100, 50])),
        ])
    }

    #[test]
    fn model_export_has_meshes_skin_materials_and_animation() {
        let dir = sample_model_dir();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let model = ModelCache::new()
            .get(&vfs, "data/r3d/test/kiste.rdo")
            .unwrap();
        let glb = model_to_glb(&vfs, &model).unwrap();
        let (doc, buffers, images) = gltf::import_slice(&glb).unwrap();

        // Bones, kiste + kiste#pivot, koerper, zauber01.
        let names: Vec<&str> = doc.nodes().filter_map(|n| n.name()).collect();
        assert_eq!(
            names,
            [
                "bip01 pelvis",
                "kiste#pivot",
                "kiste",
                "koerper",
                "zauber01"
            ]
        );
        assert_eq!(doc.meshes().count(), 2);
        assert_eq!(images.len(), 1);
        let skin = doc.skins().next().unwrap();
        assert_eq!(skin.joints().count(), 1);

        // Node matrix of kiste mirrored: (5, 0, 5) -> (5, 0, -5).
        let kiste = doc.nodes().find(|n| n.name() == Some("kiste")).unwrap();
        let (t, _, _) = kiste.transform().decomposed();
        assert_eq!(t, [5.0, 0.0, -5.0]);

        // Winding order: the cross product of the corners points the same way as the normal.
        let mesh = kiste.children().next().unwrap().mesh().unwrap();
        let prim = mesh.primitives().next().unwrap();
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        let p: Vec<Vec3> = reader.read_positions().unwrap().map(Vec3::from).collect();
        let n: Vec<Vec3> = reader.read_normals().unwrap().map(Vec3::from).collect();
        assert!((p[1] - p[0]).cross(p[2] - p[0]).dot(n[0]) > 0.0);

        // EF-MATERIAL takes the color, the second material is white and semi-transparent.
        let colors: Vec<[f32; 4]> = doc
            .materials()
            .map(|m| m.pbr_metallic_roughness().base_color_factor())
            .collect();
        let expected = [
            [
                srgb_to_linear(0.8),
                srgb_to_linear(0.6),
                srgb_to_linear(0.4),
                1.0,
            ],
            [1.0, 1.0, 1.0, 0.5],
        ];
        for (got, want) in colors.iter().zip(&expected) {
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() < 1e-5, "{colors:?}");
            }
        }
        // sRGB 0.8 / 0.6 / 0.4 -> linear ~ 0.604 / 0.319 / 0.133, alpha unchanged.
        assert!((colors[0][0] - 0.604).abs() < 1e-3 && (colors[0][3] - 1.0).abs() < 1e-6);
        assert_eq!(
            doc.materials().nth(1).unwrap().alpha_mode(),
            gltf::material::AlphaMode::Blend
        );

        // Animation: bones (translation, rotation), kiste (translation, rotation, scale),
        // kiste#pivot (translation). 4,800 ticks = 1 s.
        let anim = doc.animations().next().unwrap();
        assert_eq!(anim.name(), Some("laufen"));
        assert_eq!(anim.channels().count(), 6);
        let channel = anim.channels().next().unwrap();
        let times: Vec<f32> = channel
            .reader(|b| Some(&buffers[b.index()]))
            .read_inputs()
            .unwrap()
            .collect();
        assert_eq!(times, [0.0, 1.0]);
    }

    #[test]
    fn times_are_made_strictly_increasing() {
        let (times, values) = strictly_increasing(
            [(0.0, 'a'), (0.5, 'b'), (0.5, 'c'), (0.2, 'd'), (1.0, 'e')].into_iter(),
        );
        assert_eq!(times, [0.0, 0.5, 1.0]);
        assert_eq!(values, ['a', 'c', 'e']);
    }
}
