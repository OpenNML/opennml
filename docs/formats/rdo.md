# Models `.rdo` (container `0x7DB`)

Rust: `nml_formats::rdo::parse_rdo` (standard and old format), paths and textures `nml_world::assets`.

glTF export: `nml_export::model_to_glb` (mirroring `z -> -z`, corners `i0, i2, i1`, UV `(u, -v)`; skin with
`inverseBindMatrix = T(-rest position)`, node tracks via child nodes `<mesh>#pivot`). Deliberate limits: materials are
unlit PBR (`metallic 0`, `roughness 1`), colors converted from sRGB to linear - plausible, not faithful to the
original; node tracks of helper nodes (3005) are not animated; water (level export) without waves and texture motion.

Binary model format of the 3ds Max exporter: meshes with positions, normals, UVs and materials, optionally a skeleton
with vertex weights. The `.rdw` files use the same container format (identifier `0x7DB`, [rdw.md](rdw.md));
animations are stored separately in `.rdm` ([rdm.md](rdm.md)), the composition (model + animations + sub-objects) in the
object `.cfg` ([object-cfg.md](object-cfg.md)).

The building footprint takes the half extent from the header corners (`(corner7 - corner0) * 0.5`), not from
`halbe_ausdehnung`; the loader does not overwrite the mesh scale. `skalierung` is read as `RdoMesh::scale`; the random
map generator applies it to the hull `blocking01` (snapping and ground rectangle). The static node matrix of the meshes
computes with `(1,1,1)`, though.

## Purpose and locations

- 994 files, almost all under `data/r3d/**` (units, buildings, nature, tools, shadow meshes `*_shd.rdo` - flat plates of
  which the game uses only the hull and the texture of material 0), plus `data/r3d/misc/mousecursor/mouse.rdo` (3D mouse
  pointer) and a stray file `data/rdm/kanone_spa_stehen.rdo` (old format, see below).
- A `.rdo` is loaded as a sub-object from an object `.cfg` (`SUBOBJECT`/`FILE`; the game does not read `OBJECT`/`FILE`,
  see [object-cfg.md](object-cfg.md)), via project files (`OBJ_FILENAME`, `OBJ_SHADOW_FILENAME`, ... in
  `data/projects/*.prj`; the parser replaces every extension there by `.rdo`, see [rdw.md](rdw.md)) and via built-in
  paths (mouse pointer, rally point flag, dynamite, surf).
- The loader does **not** check the identifier `0x7DB` (= 2011); it is the same in all 994 `.rdo` and all 99 `.rdw`.

## Reading

The game loads the model (cached by path), reads the file and then loads the materials and textures and builds the
vertex buffers (static meshes: one vertex per triangle corner with position, vertex normal and UV with a **negated v**;
skin meshes: position, bone indexes, weights for 2 to 4 influences, vertex normal, UV, face normal) and one node matrix
per mesh. Per mesh it builds material groups (runs of the same material ID in the triangle list; the file field is
computed by the loader only if it is `-1`). Skin meshes are additionally sorted by number of influences (1-4) and
bone palette and batched. The meshes are found by exact name (byte comparison) for the node and morph tracks of the
`.rdm`.

## Layout

All values are little-endian, `f32` = IEEE-754 single, `i32`/`u32` 32 bits. Strings are null-terminated fields of fixed
length in cp1252; what follows after the first NUL is memory content of the exporter. The file is a sequence of blocks
without alignment; it ends exactly after the last material.

```
Header (0x390 bytes)
Mesh header x mesh_anzahl            (460 bytes each)
Mesh data x mesh_anzahl              (variable, in the order of the headers)
Bone header x knochen_anzahl         (280 bytes each)
Bone data x knochen_anzahl           (variable)
Material x material_anzahl          (572 bytes each)
```

### Header

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `u32` | `version` | `0x7DB`; unchecked |
| `0x004` | `char[256]` | `name` | File name with extension (e.g. `xyz.rdo`); **overwritten by the loader** (derived from the actual path, extension removed, lowercase) |
| `0x104` | `char[256]` | `path` | Exporter path (`.\data\r3d\...`); **overwritten by the loader** with the load path (lowercase); serves as a key |
| `0x204` | `i32` | `mesh_anzahl` | Number of meshes (including helper nodes) |
| `0x208` | `i32` | `knochen_anzahl` | Number of bones |
| `0x20C` | `i32` | `material_anzahl` | Number of materials |
| `0x210` | `i32` | `animations_anzahl_exporter` | Number of animations according to the exporter (450 of 484 cases = number of `ANIMATION` blocks of the `.cfg`); **set to 0 by the loader** and recounted when animations are attached |
| `0x214` | bounds (124 bytes) | `huelle` | Bounding box of the object, see below |
| `0x290` | 256 bytes | `reserve` | Exporter garbage (mostly `0xCD` fill; other memory content in 323 of 1,091 `.rdo`/`.rdw`); read, used nowhere |

Bounds (124 bytes; also per mesh):

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x00` | `f32[3]` | `mittelpunkt` | Center of the box (object space) |
| `0x0C` | `f32[3] x 8` | `ecke0...7` | Corners; corner `i` = (`i&1 ? max.x : min.x`, `i&2 ? max.y : min.y`, `i&4 ? max.z : min.z`) |
| `0x6C` | `f32` | `radius` | Length of `halbe_ausdehnung` (circumscribed sphere) |
| `0x70` | `f32[3]` | `halbe_ausdehnung` | `(max - min) / 2` |

For mesh hulls of helper nodes (type 3005) the corner order is different (x, z, y) and `halbe_ausdehnung` contains
signs - it comes unchanged from 3ds Max; of it only the corners are evaluated (`hitbox`, below). Of the object hull the
game uses `mittelpunkt`, `halbe_ausdehnung` (footprint of the shadow decals) and `radius` (hull of the model effects);
the mesh hulls of rigid meshes go into the bounding box of the instance (picking and shadow textures), the corners of
the helper node hull `hitbox` serve picking (with the node scale, D-367).

### Mesh header (460 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `i32` | `typ` | 3001 (`0xBB9`) skin mesh, 3002 (`0xBBA`) rigid mesh, 3003 (`0xBBB`) rigid mesh (exporter variant; the game checks only for `0xBB9` and `0xBBD` and treats 3003 like 3002), 3005 (`0xBBD`) helper node without geometry (docking point, e.g. `zauber01`, `farmer01`; also for effects: `<DUMMYOBJECT>NN` from `effectlist.ini`, `dampf01`, `tzone01`, `kielwasser01` ...; is not drawn) |
| `0x004` | `char[256]` | `name` | Node name from 3ds Max (lowercase); key for the node/morph tracks of the `.rdm` |
| `0x104` | `f32[3]` | `pivot` | World position of the Max node (pivot); = start value of the position track of a matching `.rdm` node track |
| `0x110` | `f32[3]` | `position` | Translation node space -> object space (pivot + object offset) |
| `0x11C` | `f32[4]` | `rotation_wxyz` | Rotation node space -> object space as a quaternion **(w, x, y, z)**, normalized |
| `0x12C` | `f32[3]` | `skalierung` | Max scale of the node (Rust `RdoMesh::scale`). For meshes with vertices it is already folded into the vertices; the static node matrix computes with `(1,1,1)`. The loader does **not** overwrite the field: the game applies the file value to helper nodes. For helper nodes (3005) the stored corners are in node space (unscaled), `halbe_ausdehnung`, in contrast, is scaled. In the shipped data: 79 of 110 models with `blocking01` have a scale != 1 (e.g. palme04...06 `20/29`, half edge 36.86 -> 25.42), all 74 models with `hitbox` too: their stored corners are always a cube, the shape of the hit box arises only through the scale (e.g. SPA finca `(1, 0.082, 1)`: height +-29.9 instead of +-364.6). The reimplementation applies the scale to `blocking01` (D-365) and to the box `hitbox` and the frame corners (`nml_world::math::helper_node_matrix`, D-367); the rest position of helper nodes (`helper_rest_position`) still without scale, without effect in the shipped data (D-367) |
| `0x138` | `i32` | `vertex_anzahl` | `nv` |
| `0x13C` | `i32` | `dreieck_anzahl` | `nf` |
| `0x140` | `i32` | `uv_anzahl` | `nuv` |
| `0x144` | `i32` | `uv_dreieck_anzahl` | `nuf`; always `nf` or 0 (0 = no UVs, the game sets UV `(0,0)`) |
| `0x148` | bounds | `huelle` | Bounding box of the mesh in object space |
| `0x1C4` | `i32` | `material` | Material index of the Max node (`-1` for helper nodes); only copied, never evaluated - the material ID per triangle is decisive |
| `0x1C8` | `i32` | `gruppen_anzahl` | Number of runs of the same material ID in the triangle list, or `-1` = to be computed by the loader; in all cases != -1 (1,391 meshes in `.rdo`/`.rdw`) the value is correct |

The file offsets above count from the start of the mesh header.

### Mesh data (per mesh, in header order)

| Block | Type per element | Count | Meaning |
|---|---|---|---|
| `positionen` | `f32[3]` | `nv` | Vertex position (skin mesh: object space; otherwise node space) |
| `dreiecke` | `i32[4]` | `nf` | `i0, i1, i2` vertex indexes (`< nv`), `material_id` (`< material_anzahl`) |
| `uvs` | `f32[2]` | `nuv` | Texture coordinate `(u, v)`; values outside `[0,1]` = tiling |
| `uv_dreiecke` | `i32[4]` | `nuf` | `t0, t1, t2` UV indexes (`< nuv`), fourth field always `-1` |
| `flaechennormalen` | `f32[3]` | `nf` | Normalized `(v1 - v0) x (v2 - v0)`; `0` or NaN for degenerate triangles |
| `vertexnormalen` | `f32[3]` | `nv` | Smoothed normal (length 1; occasionally shorter, 0 or NaN) |

Positions and UVs are **indexed separately** (as in 3ds Max): corner `k` of a triangle `f` has position
`positionen[dreiecke[f][k]]`, normal `vertexnormalen[dreiecke[f][k]]` and UV `uvs[uv_dreiecke[f][k]]`. The game builds
a non-indexed vertex buffer from it with three vertices per triangle (order `i1, i2, i0` - a cyclic rotation, the
winding stays) and sets `v_game = -v`.

### Bone header (280 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `char[256]` | `name` | Biped name, e.g. `bip01 l thigh`; assignment to `.rdm` tracks via the part **after the first space**, lowercase |
| `0x100` | `f32[3]` | `ruheposition` | Joint position in the bind pose (object space) |
| `0x10C` | `i32` | `kind_anzahl` | Number of child bones |
| `0x110` | `i32` | `einfluss_anzahl` | Number of influence records |
| `0x114` | 4 bytes | `hat_gewichte` | Exporter value (0/1 or memory garbage as a float bit pattern); the loader sets `1` if the bone has weights and does not overwrite it otherwise - not reliable |

### Bone data (per bone, in header order)

| Block | Type | Count | Meaning |
|---|---|---|---|
| `kinder` | `i32` | `kind_anzahl` | Index of the child bone or `-1` (-> `NULL`) |
| `einfluss[k]` | 24 bytes | `einfluss_anzahl` | `+0x00` 4 bytes pointer (garbage, set by the loader), `+0x04` `i32` `mesh` (mesh index), `+0x08` `i32` `anzahl`, `+0x0C` 12 bytes pointer (garbage) |
| per `einfluss[k]`: `vertices` | `i32` | `anzahl` | Vertex index in the mesh; values `>= nv` or `< 0` are set to `-1` by the loader (29 cases in the shipped data) |
| per `einfluss[k]`: `gewichte` | `f32` | `anzahl` | Weight `[0,1]` |

Per vertex the weights sum to 1. The game takes at most **4** influences per vertex in reading order (bones ascending);
in the shipped data individual vertices have 5 - the fifth is discarded. All vertices of the 94 skin meshes have at
least one influence.

### Material (572 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `char[256]` | `name` | Max material name; converted to uppercase by the game and only taken into the material table |
| `0x100` | `f32[3]` | `ambient` | RGB `[0,1]` |
| `0x10C` | `f32[3]` | `diffus` | RGB `[0,1]` |
| `0x118` | `f32[3]` | `spekular` | RGB `[0,1]` |
| `0x124` | `f32` | `transparenz` | `[0,1]`; alpha of all three colors = `1 - transparenz` (61 materials > 0) |
| `0x128` | 16 bytes | `unausgewertet` | In the shipped data always `(1.0, 1.0, 0, 0)` as `f32` (all 1,544 materials); not read by the game |
| `0x138` | `char[256]` | `textur` | Texture file (`.tga`, 42 times empty). **Without a directory** (1,080 times) it is searched relative to the directory of the `.rdo`, **with a directory** (173 times) it is used unchanged as a game path |
| `0x238` | 4 bytes | `rest` | Memory content (0, spaces, arbitrary values), not read |

The material ID of a triangle is the index into this list (`< material_anzahl` in all files).

## Coordinate system and units

- **Y is up**, the ground lies at `y = 0` (units stand with their feet at `y ~ 0`). Figures and animals **face -Z**
  (face/head at small z, evidence: wireframe check below). A game figure is about 137 units tall, an ox with a cart
  about 320 long.
- **Model space is left-handed (D3D)**: +X right, +Y up, +Z into the depth; figures face -Z, their **left limbs lie at
  +X**. Evidence `tierbeschwoerer_wald.rdo` (rest positions): `bip01 l hand` x = +48.1, `bip01 r hand` x = -45.8,
  `bip01 l toe0` z = -19.4 against `bip01 l foot` z = +1.0 (toes point to -Z); in the whole shipped data, for all 1,021
  left/right bone pairs the left bone lies at the larger x. A figure facing the camera (at -Z) thus appears the right
  way round in D3D.
- **For a right-handed renderer (e.g. glTF export, other right-handed targets)**: mirror Z - positions, normals and
  position keys `z -> -z`, quaternions `(w, x, y, z) -> (w, -x, -y, z)` (mirroring `M*R*M` with `M = diag(1, 1, -1)`) -
  and reverse the winding at the same time (swap the indexes `i0, i2, i1` or `front_face`), because after mirroring
  `normalized((v1 - v0) x (v2 - v0)) = -n_mirrored` holds. Equivalent is a mirroring in the view matrix plus a
  reversed `front_face`. Without mirroring all models appear mirrored on a right-handed target. `nml-render` (wgpu)
  does **not** need this: it renders with the original left-handed matrices without mirroring and compensates the
  winding with `FrontFace::Cw` (wgpu NDC corresponds to D3D NDC).
- Convention as in Direct3D: row vectors, `v' = v * M`. Transformation rules:
  - **Skin mesh (3001)**: `positionen` already lie in object space (bind pose). Animation:
    `v' = sum_i w_i * ((v - ruheposition_i) * R(q_i(t)) + p_i(t))` with the tracks of the `.rdm` (see
    [rdm.md](rdm.md)); at most 4 influences.
  - **Rigid meshes (3002/3003)**: `positionen` lie in node space; `v_object = v * R(rotation_wxyz) + position`. The
    game builds per mesh the node matrix `T(-a) * S(s) * R(q) * T(b)` with `a = 0, b = position, q = rotation_wxyz, s = (1,1,1)`, i.e.
    `T(-0) * S(1) * R(q) * T(position)` and stores the 64-byte matrix per mesh in the model. It multiplies it with the
    instance matrix to the world matrix of the mesh; this is used without a running animation for all meshes except
    3001/3005, with an animation for helper nodes 3005 and for rigid meshes that have neither a node nor a morph track
    in the current animation (otherwise the node track or morph track is evaluated). Evidence in the data: in 1,303 of
    1,341 rigid meshes of the `.rdo` with geometry the result hits the stored mesh hull within 1 %, likewise for all 267
    meshes of the `.rdw`; the 38 deviations are mostly shadow surfaces `*_shd` with a slightly shifted hull. The 94 skin
    meshes hit their hull without a transformation.
  - Normals are rotated like positions (without translation).
- `R(q)` with `q = (w, x, y, z)` is the matrix: row 0 = `(1-2(y^2+z^2), 2(xy-wz), 2(xz+wy))`, row 1 =
  `(2(xy+wz), 1-2(x^2+z^2), 2(yz-wx))`, row 2 = `(2(xz-wy), 2(yz+wx), 1-2(x^2+y^2))`; applied as `v * R` (so with the
  transpose in the column-vector sense). In the shipped data quaternions are often stored with `w < 0` (`-q == q`).
- **Winding**: The stored face normal is `normalized((v1 - v0) x (v2 - v0))` for the index order `i0, i1, i2`
  (133,163 of 134,509 evaluable triangles unambiguous, 168 reversed in two faultily modeled meshes, the rest
  rounding/tiny triangles). For a renderer: the front face is the side in which this normal points; in a
  left-handed D3D viewing system front faces appear clockwise. The game draws with `CULLMODE` = CCW, so the triangles
  that appear clockwise are visible - matching the winding above.
- Texture coordinates: `(u, v)` as in 3ds Max (v up); the game uses `(u, -v)`.

## Ignored sections

| Section | Size | Reason |
|---|---|---|
| Rest of the fields `name`/`pfad` after the NUL (header, meshes, bones, materials) | variable | Memory content of the exporter; the loader overwrites `name`/`pfad` of the header anyway (it copies the load path into the path field and derives the name from it) |
| Header `0x290`...`0x38F` (`reserve`) | 256 | Read, never referenced |
| `einfluss[k]` pointer fields `+0x00`, `+0x0C`...`+0x17` | 16 per record | The loader sets them anew |
| Bone `hat_gewichte` | 4 | Loader logic see above; value not reliable |
| Mesh `material`, `skalierung` | - | Read but not evaluated (see tables); for the hulls see "Header" |
| Material `0x128`...`0x137` (constant) and `0x238`...`0x23B` (memory content) | 20 | Not read |

## Old format (1 file)

`data/rdm/kanone_spa_stehen.rdo` (referenced nowhere; not correctly readable with the loader of the game, because it
reads fixed sizes) comes from an older exporter version:

- Header `0x314` instead of `0x390` bytes: after the four counters at `0x204` follow 132 bytes of memory content
  (`0x214`...`0x297`), then the object hull (`0x298`...`0x313`); the 256-byte block `reserve` is missing.
- Mesh header 732 instead of 460 bytes: after the counters 28 bytes (the seven array pointers of the memory image), then
  the hull, then 252 bytes of unknown content; `material`/`gruppen_anzahl` are missing as separate fields.
- Mesh data, bones and materials as above.

`data/r3d/animals/bär/bear.rdw` has the same old format ([rdw.md](rdw.md)).

## Validation

Every effective file from the RDA archives is read; every byte is assigned to a field or a garbage area, and the read
pointer must end exactly at the end of the file. Then plausibility checks (indexes `< nv`/`< nuv`, material IDs
`< material_anzahl`, `gruppen_anzahl`, UV triangle field 4 = `-1`, child indexes, influence meshes, weights in `[0,1]`,
colors/transparency in `[0,1]`, quaternion normalized, finite positions) and statistical checks (hulls, normals,
winding). Result: 994 of 994 files parse; variants: 993 standard, 1 old.

| Check | Count |
|---|---|
| influence vertex outside | 29 |
| influences per vertex, maximum | 5 |
| face normals of unit length | 134,617 |
| face normals zero | 210 |
| hull radius different | 1 |
| mesh hull deviating | 38 |
| mesh hull fits | 1,397 |
| skin vertices without a weight | 0 |
| meshes of type 3001 | 94 |
| meshes of type 3002 | 1,296 |
| meshes of type 3003 | 46 |
| meshes of type 3005 | 1,471 |
| vertex normals of unit length | 123,274 |
| vertex normals shorter | 197 |
| vertex normals NaN | 17 |
| winding reversed | 168 |
| winding matches the cross product | 133,163 |
| winding unclear | 1,178 |

Visual check: `tierbeschwoerer_wald.rdo` (skin mesh, 177 vertices, 350 triangles) gives a T-pose figure with a hat,
outstretched arms and feet toward -Z; hull `x -57.6...60.2`, `y 0.4...136.5`, `z -19.4...10.7` = stored hull.
`ochsenkarre_eng.rdw` (6 rigid meshes) assembles with `v * R + position` into ox, pole, cart and wheels; without the
node transformation the parts lie scattered.

## Open questions

- Meaning of the 132/252 bytes in the old format; difference between 3002/3003 from the exporter's point of view.
