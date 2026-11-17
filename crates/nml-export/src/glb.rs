//! Minimal writer for glTF 2.0 in binary format (`.glb`): one buffer, buffer views and
//! accessors for the required data types, embedded PNG textures.

use image::RgbaImage;
use serde_json::{Value, json};

const ARRAY_BUFFER: u32 = 34962;
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// Collects the parts of a glTF file; the indices of the return values are glTF indices.
#[derive(Default)]
pub struct GltfBuilder {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    images: Vec<Value>,
    skins: Vec<Value>,
    animations: Vec<Value>,
}

impl GltfBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(bytes);
        self.buffer_views.push(view);
        self.buffer_views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        target: Option<u32>,
        component: u32,
        count: usize,
        kind: &str,
        bounds: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let view = self.view(bytes, target);
        let mut accessor = json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": kind,
        });
        if let Some((min, max)) = bounds {
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn floats<const N: usize>(
        &mut self,
        data: &[[f32; N]],
        target: Option<u32>,
        kind: &str,
        with_bounds: bool,
    ) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let bounds = (with_bounds && !data.is_empty()).then(|| {
            let mut min = vec![f32::INFINITY; N];
            let mut max = vec![f32::NEG_INFINITY; N];
            for item in data {
                for k in 0..N {
                    min[k] = min[k].min(item[k]);
                    max[k] = max[k].max(item[k]);
                }
            }
            (min, max)
        });
        self.accessor(&bytes, target, FLOAT, data.len(), kind, bounds)
    }

    /// `POSITION` (with `min`/`max`, as glTF requires).
    pub fn positions(&mut self, data: &[[f32; 3]]) -> usize {
        self.floats(data, Some(ARRAY_BUFFER), "VEC3", true)
    }

    /// `NORMAL`, `TEXCOORD_n`, `COLOR_n`, `WEIGHTS_n` and other vertex attributes from `f32`.
    pub fn attribute<const N: usize>(&mut self, data: &[[f32; N]]) -> usize {
        let kind = match N {
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            _ => panic!("vertex attribute with {N} components"),
        };
        self.floats(data, Some(ARRAY_BUFFER), kind, false)
    }

    /// `JOINTS_0` als `u16`.
    pub fn joints(&mut self, data: &[[u16; 4]]) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        self.accessor(
            &bytes,
            Some(ARRAY_BUFFER),
            UNSIGNED_SHORT,
            data.len(),
            "VEC4",
            None,
        )
    }

    /// Indices (`u32`).
    pub fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.accessor(
            &bytes,
            Some(ELEMENT_ARRAY_BUFFER),
            UNSIGNED_INT,
            data.len(),
            "SCALAR",
            None,
        )
    }

    /// Times of an animation sampler in seconds (with `min`/`max`).
    pub fn times(&mut self, data: &[f32]) -> usize {
        let items: Vec<[f32; 1]> = data.iter().map(|&t| [t]).collect();
        self.floats(&items, None, "SCALAR", true)
    }

    /// Output values of an animation sampler (`VEC3` or `VEC4`).
    pub fn keyframes<const N: usize>(&mut self, data: &[[f32; N]]) -> usize {
        let kind = if N == 4 { "VEC4" } else { "VEC3" };
        self.floats(data, None, kind, false)
    }

    /// Matrices (column-major), e.g. `inverseBindMatrices`.
    pub fn matrices(&mut self, data: &[[f32; 16]]) -> usize {
        self.floats(data, None, "MAT4", false)
    }

    /// Embeds an image as PNG and returns the texture index (sampler: linear, repeat).
    pub fn texture_png(&mut self, img: &RgbaImage) -> usize {
        self.texture_with_sampler(img, 0)
    }

    /// Like [`Self::texture_png`], but with a linear sampler clamped at the edge (for textures whose
    /// UV cover exactly 0..1 and must not blend into the opposite side at the edge).
    pub fn texture_png_clamped(&mut self, img: &RgbaImage) -> usize {
        self.texture_with_sampler(img, 1)
    }

    fn texture_with_sampler(&mut self, img: &RgbaImage, sampler: usize) -> usize {
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("writing PNG to memory");
        let view = self.view(&png, None);
        self.images
            .push(json!({ "bufferView": view, "mimeType": "image/png" }));
        self.textures
            .push(json!({ "source": self.images.len() - 1, "sampler": sampler }));
        self.textures.len() - 1
    }

    pub fn material(&mut self, material: Value) -> usize {
        self.materials.push(material);
        self.materials.len() - 1
    }

    pub fn mesh(&mut self, mesh: Value) -> usize {
        self.meshes.push(mesh);
        self.meshes.len() - 1
    }

    pub fn node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Appends `children` to the node `parent`.
    pub fn add_children(&mut self, parent: usize, children: &[usize]) {
        if children.is_empty() {
            return;
        }
        let node = &mut self.nodes[parent];
        if node.get("children").is_none() {
            node["children"] = json!([]);
        }
        let list = node["children"].as_array_mut().expect("children is a list");
        list.extend(children.iter().map(|&c| json!(c)));
    }

    pub fn skin(&mut self, skin: Value) -> usize {
        self.skins.push(skin);
        self.skins.len() - 1
    }

    pub fn animation(&mut self, animation: Value) -> usize {
        self.animations.push(animation);
        self.animations.len() - 1
    }

    /// Creates the `.glb` bytes with one scene from the root nodes `roots`.
    pub fn finish(mut self, roots: &[usize]) -> Vec<u8> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        // glTF requires at least one element in `scenes[0].nodes` (the `gltf` crate also rejects a
        // missing list): without roots, an empty node serves as the root. An empty binary buffer
        // (`byteLength` >= 1 required) is omitted along with the BIN chunk.
        let placeholder;
        let roots = if roots.is_empty() {
            self.nodes.push(json!({ "name": "empty" }));
            placeholder = [self.nodes.len() - 1];
            &placeholder[..]
        } else {
            roots
        };
        let mut root = json!({
            "asset": { "version": "2.0", "generator": "nml-export" },
            "scene": 0,
            "scenes": [{ "nodes": roots }],
        });
        if !self.bin.is_empty() {
            root["buffers"] = json!([{ "byteLength": self.bin.len() }]);
        }
        // glTF requires at least one element per present list: leave out empty lists.
        for (key, list) in [
            ("bufferViews", self.buffer_views),
            ("accessors", self.accessors),
            ("meshes", self.meshes),
            ("nodes", self.nodes),
            ("materials", self.materials),
            ("textures", self.textures),
            ("images", self.images),
            ("skins", self.skins),
            ("animations", self.animations),
        ] {
            if !list.is_empty() {
                root[key] = Value::Array(list);
            }
        }
        if root.get("textures").is_some() {
            // LINEAR 9729, LINEAR_MIPMAP_LINEAR 9987, REPEAT 10497, CLAMP_TO_EDGE 33071.
            // Sampler 0 repeats, sampler 1 clamps at the edge; both are always present so that the
            // indices stay valid.
            root["samplers"] = json!([
                { "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 },
                { "magFilter": 9729, "minFilter": 9987, "wrapS": 33071, "wrapT": 33071 }
            ]);
        }
        let mut json_bytes = serde_json::to_vec(&root).expect("serializing JSON");
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let bin_chunk = if self.bin.is_empty() {
            0
        } else {
            8 + self.bin.len()
        };
        let total = 12 + 8 + json_bytes.len() + bin_chunk;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        if !self.bin.is_empty() {
            out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&self.bin);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_glb_with_mesh_texture_skin_and_animation() {
        let mut b = GltfBuilder::new();
        let positions = b.positions(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        let normals = b.attribute(&[[0.0, 0.0, 1.0]; 3]);
        let uvs = b.attribute(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        let joints = b.joints(&[[0, 0, 0, 0]; 3]);
        let weights = b.attribute(&[[1.0, 0.0, 0.0, 0.0]; 3]);
        let indices = b.indices(&[0, 1, 2]);
        let texture = b.texture_png(&RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])));
        let material = b.material(json!({
            "pbrMetallicRoughness": { "baseColorTexture": { "index": texture } }
        }));
        let mesh = b.mesh(json!({ "primitives": [{
            "attributes": {
                "POSITION": positions, "NORMAL": normals, "TEXCOORD_0": uvs,
                "JOINTS_0": joints, "WEIGHTS_0": weights
            },
            "indices": indices, "material": material
        }]}));
        let joint = b.node(json!({ "name": "bip01", "translation": [0.0, 1.0, 0.0] }));
        let ibm = b.matrices(&[
            glam::Mat4::from_translation(glam::Vec3::new(0.0, -1.0, 0.0)).to_cols_array(),
        ]);
        let skin = b.skin(json!({ "joints": [joint], "inverseBindMatrices": ibm }));
        let body = b.node(json!({ "name": "koerper", "mesh": mesh, "skin": skin }));
        let parent = b.node(json!({ "name": "gruppe" }));
        b.add_children(parent, &[body]);
        let input = b.times(&[0.0, 1.0]);
        let output = b.keyframes(&[[0.0, 1.0, 0.0], [0.0, 2.0, 0.0]]);
        b.animation(json!({
            "name": "laufen",
            "samplers": [{ "input": input, "output": output, "interpolation": "LINEAR" }],
            "channels": [{ "sampler": 0, "target": { "node": joint, "path": "translation" } }]
        }));
        let glb = b.finish(&[parent, joint]);

        let (doc, buffers, images) = gltf::import_slice(&glb).unwrap();
        assert_eq!(doc.meshes().count(), 1);
        assert_eq!(images.len(), 1);
        assert_eq!(doc.animations().count(), 1);
        let prim = doc.meshes().next().unwrap().primitives().next().unwrap();
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        let read: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        assert_eq!(read[1], [1.0, 0.0, 0.0]);
        assert_eq!(doc.skins().next().unwrap().joints().count(), 1);
    }

    #[test]
    fn empty_builder_yields_valid_glb_without_buffers_and_with_placeholder_root() {
        let glb = GltfBuilder::new().finish(&[]);
        let parsed = gltf::Gltf::from_slice(&glb).unwrap();
        assert!(parsed.blob.is_none());
        // Raw JSON from the first chunk (after header and chunk header).
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        assert!(json.get("buffers").is_none(), "{json}");
        assert_eq!(json["scenes"][0]["nodes"], json!([0]), "{json}");
        assert_eq!(parsed.document.nodes().count(), 1);
    }

    #[test]
    fn clamped_texture_uses_clamp_to_edge_sampler() {
        let mut b = GltfBuilder::new();
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let repeat = b.texture_png(&img);
        let clamped = b.texture_png_clamped(&img);
        let node = b.node(json!({ "name": "empty" }));
        let glb = b.finish(&[node]);
        let (doc, _, _) = gltf::import_slice(&glb).unwrap();
        let wraps: Vec<_> = doc
            .textures()
            .map(|t| (t.sampler().wrap_s(), t.sampler().wrap_t()))
            .collect();
        use gltf::texture::WrappingMode::{ClampToEdge, Repeat};
        assert_eq!(wraps[repeat], (Repeat, Repeat));
        assert_eq!(wraps[clamped], (ClampToEdge, ClampToEdge));
    }
}
