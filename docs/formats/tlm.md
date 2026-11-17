# Texture map `.tlm`

Rust: `nml_formats::tlm::parse_tlm`; block textures `nml_world::chunks::build_chunk_textures`.

glTF export: `nml_export::bake_chunk_texture` recomputes the base texture and the alpha pass per block on the CPU
(bilinear, without mipmaps); the vertex color is stored as `COLOR_0` in the mesh.

Binary texture map of a level: per terrain tile the GUID of a ground texture from the project file. Grid size from the
`[TERRAIN]` block of the `.wld` ([wld.md](wld.md)); heights in [hlm.md](hlm.md); the overall flow in [lvl.md](lvl.md).

## Purpose and locations

- 57 effective files under `data/levels/**` (55 levels and two orphaned part files, see [lvl.md](lvl.md)); size
  `DIM_X * DIM_Z * 4` bytes: 16,384 (64^2), 65,536 (128^2), 262,144 (256^2), 1,024 (16^2, `oxdebug`).
- File name = level path with the extension `.tlm`.
- The textures themselves are in the project file (`[TEXTURE]`, see [prj.md](prj.md)); in the shipped data there are 48
  textures (`data/textures/**.tga`); the `.tlm` files of the 55 levels use 44 of them.

## Reading

The game opens the file and reads 4 bytes per tile into the tile's texture GUID. It looks the texture up in the project
file by `TEX_GUID` and loads it (no texture for an unknown GUID). Afterwards it computes the face normals of all tiles.
It builds `(DIM_X / CHUNK_SIZE)^2` drawing blocks; per block it builds the base texture (`TEXTURE_BASE_XSIZE x
_YSIZE` texels) as a weighted mix of the tile textures and, if the alpha switch is set, one alpha map
`TEXTURE_ALPHA_XSIZE x _YSIZE` per texture group (`TEX_GROUP_ID`). The minimap is computed from `TEX_MAP_COLOR` (only
if the engine configuration enables it). The `TEX_BITS` of the tile under a position are queried for walkability and
footprints (bits 0 and 1).

## Layout

Little-endian, without a header: `DIM_X * DIM_Z` values of `u32`; value `i` belongs to tile `x = i mod DIM_X`,
`z = i div DIM_X` (row by row, index `DIM_X * z + x`).

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `4 * i` | `u32` | `textur_guid` | `TEX_GUID` of the project file; unknown GUID -> tile without a texture |

Tile `(x, z)` covers, in world units, `[x*SIZE_X, (x+1)*SIZE_X] x [z*SIZE_Z, (z+1)*SIZE_Z]` and the grid points
`(x,z)`, `(x+1,z)`, `(x,z+1)`, `(x+1,z+1)` of the `.hlm`.

Excerpt of `kamp1-1.tlm`: `0500 0000 0500 0000 ...` = GUID 5 (`Karibik Wasser Dunkel`, sea floor) in the first tiles.

### Texturing in the game

- **Tile UVs**: `u = (x mod T + {0,1}) / T`, `v = 1 - (z mod T + {0,1}) / T` with `T = TEXTURE_TILING`; they are not
  used for the vertex buffer (its UVs are built per block, see below).
- **Blocks**: The terrain is split into blocks of `CHUNK_SIZE x CHUNK_SIZE` tiles (16 in the shipped data); each block
  gets its own textures.
- **Blend weight** of a texture `t` at a texel with block coordinate `p = (i * C / W, j * C / H)` (tile units,
  `C = CHUNK_SIZE`, `W x H` = texel size of the base or alpha map): the sum, over the 3 x 3 neighboring tiles `k`
  around `floor(p)` that carry texture `t`, of `max(0, 1 - |p - center(k)|^2 * 16/49)` (radius 1.75 tiles, constant
  `0.3265306`); afterwards normalized per texel to a sum of 1. The texture weight compares the `TEX_GUID`, the group
  weight the `TEX_GROUP_ID` (group 0 does not count).
- **Base texture**: texel `(i, j)` (`W x H` = `TEXTURE_BASE_XSIZE x _YSIZE`) = sum over the textures `t` of the block
  of weight * sample of `t` at `(i/W, j/H)`; every ground texture is sampled **once over the whole block**, not
  tiled. Sampling `(u, v)` reads column `int(u * width)`, row `height - 1 - int(v * height)`.
- **Row direction**: The base texture and the alpha maps are written **from bottom to top**: texel row `j` (block z)
  lands in surface row `H - 1 - j`. Together with the mirrored v coordinates of the vertex buffer, row `j` thus lies at
  block z `~ j`.
- **Alpha maps** (only if the alpha switch is set; it is set when `TerrainQuality` < 2): per texture group one map
  `TEXTURE_ALPHA_XSIZE x _YSIZE` with the normalized group weights; the ground textures are blended over it, tiled,
  with UV set 2 of the vertex buffer. If the switch is off, `TEXTURE_BASE_XSIZE/_YSIZE` are set to 32 and only the base
  texture exists.
- **Details of the alpha maps and drawing:**
  - The alpha map of **group 0** is constant 255; group 0 contributes to no weight (its group weight is 0). Sum 0 ->
    all groups 0.
  - Textures of a group: tiles of the block plus a tile border, a known texture of this group, ascending by
    `TEX_GUID`.
  - Tile list of texture `t` (index `a*C + b`): a tile with `t`, or - if `t` lies in a group != 0 - a tile of another
    group != 0 in whose neighborhood `x - 2 ... x + 1`, `z - 2 ... z + 1` (asymmetric) `t` occurs.
  - Drawing: per group ascending, per texture its tile list (ground texture UV set 2, alpha map UV set 1).
- **Deviations and additions of the Rust implementation** (`crates/nml-world/src/chunks.rs`):
  - `Terrain::chunk_count` is `(DIM_X / C) * (DIM_Z / C)`, the original computes `(DIM_X / C)^2`. For the square maps
    of the shipped data this is the same.
  - `sample_texel` clamps the coordinates at the edge of the texture (column and row are limited). This is a
    defensive addition without effect for `u, v` in `[0, 1)`.
  - Alpha maps are created for all texture groups that occur in the block plus a tile border (not verified against the
    original: whether it uses the same set of groups).
- **Tile bits** (`TEX_BITS` of the texture, names in [texturebits.md](texturebits.md)): bit 0 = snow and sand tracks,
  bit 1 = not walkable; in the shipped data only the values 0, 1 and 4.
- **Minimap**: one pixel per tile. If all four corner heights lie below `WATER_LEVEL`, it is `(0, 0, 0xC4)` (blue).
  Otherwise `TEX_MAP_COLOR` (COLORREF `0x00BBGGRR`) per channel times
  `f = n * normalized(LIGHT_POSITION) + 1 - 0.01 * LIGHT_INTENSITY` with `n` = mean (not normalized) of the four
  grid-point normals of the corners (pointing up, see [hlm.md](hlm.md)); results above the starting value (or negative)
  stay at the starting value (unsigned comparison). Unknown texture: black.

### Terrain mesh (chunk vertex buffer)

This is how the game builds and draws the terrain geometry.

**Blocks**: The game creates `(DIM_X / C)^2` blocks (`C = CHUNK_SIZE` = 16), block `k` with tile origin
`x0 = (C*k) mod DIM_X`, `z0 = C * ((C*k) div DIM_X)` (x runs faster). A block has `n = C + 1` vertices per side, a
base texture, alpha maps per texture group, and `C^2` global tile indexes; entry `k` = tile
`(x0 + k div C, z0 + k mod C)`.

**Vertex buffer** (FVF `D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX3`, `n^2 * 0x28` bytes): `n^2` vertices, **x outer, z
inner**: vertex `a * n + b` = grid point `(x0 + a, z0 + b)`, `a, b` in `0...C` (blocks share the edge points, every
block has its own copies).

| Offset | Type | Content |
|---|---|---|
| `0x00` | `f32[3]` | Position (exactly the `.hlm` position) |
| `0x0C` | `D3DCOLOR` | Lit vertex color `0x00RRGGBB` from [hlm.md](hlm.md) (alpha byte 0) |
| `0x10` | `f32[2]` | UV set 0 - base texture: `u = 1/(2*Wb) + a * (1 - 1/Wb)/(n - 1)`, `v = 1 - (1/(2*Hb) + b * (1 - 1/Wb)/(n - 1))` |
| `0x18` | `f32[2]` | UV set 1 - alpha maps: `u = 1/(2*Wa) + a * (1 - 1/Wa)/(n - 1)`, `v = 1 - (1/(2*Ha) + b * (1 - 1/Wa)/(n - 1))` |
| `0x20` | `f32[2]` | UV set 2 - tiled ground texture: `u = a / T`, `v = 1 - b / T` |

`Wb x Hb` = `TEXTURE_BASE_XSIZE x _YSIZE` (8 x 8, without the detail switch 32 x 32), `Wa x Ha` =
`TEXTURE_ALPHA_XSIZE x _YSIZE` (32 x 32), `T` = `TEXTURE_TILING` (4), `n = C + 1` = 17; `a`, `b` = block-local grid
index in x and z. The step in v uses the X size, as the game does (in the shipped data the maps are square). Sets 0/1
map the block with a half-texel border: grid point `a = 0` hits the center of texel 0, `a = C` the center of texel
`W - 1` (texel `i` thus lies at block x `i * C / (W - 1)`; the weights of the maps, in contrast, are computed at
`i * C / W`). Set 2 runs continuously over the block from 0 to `C / T` = 4 and continues seamlessly across block
boundaries (block origins are multiples of `T`).

**Index buffer** (`D3DFMT_INDEX16`, `C^2 * 4` indexes, shared by all blocks): entry `k = a * C + b` (tile with
block-local `(a, b)`): `(C+1)*a + b`, `(C+1)*a + b + 1`, `(C+1)*(a+1) + b`, `(C+1)*(a+1) + b + 1` = grid points
`(x,z)`, `(x,z+1)`, `(x+1,z)`, `(x+1,z+1)`. This order matches the tile list (`tileIndices[k]`).

**Drawing**: vertex buffer as stream 0 with stride `0x28`; then for each of the `C^2` tiles `k` whose visibility byte
is != 0, `DrawIndexedPrimitive(D3DPT_TRIANGLESTRIP, 0, vertexCount, 4*k, 2)` (likewise in the further passes). The
strip yields the triangles `(x,z), (x,z+1), (x+1,z)` and `(x,z+1), (x+1,z+1), (x+1,z)` - **visible diagonal from
`(x, z+1)` to `(x+1, z)`**. In the top view (+x right, +z up) both triangles run clockwise, so they are D3D front
faces for a camera from above.

**Visibility byte of a tile**: not a file value. Every frame the game tests the nodes of a quadtree over the terrain
against the view frustum: outside -> byte of all tiles of the node 0, completely inside -> 1, partly -> test the
children, at leaf level 1. The byte is therefore pure frustum culling; for a map view all tiles are to be drawn (or use
your own culling). Further tile fields: a texture pointer, the `TEX_GUID`, the face normals A/B (pointing down; used
only for the steepness classification and as summands of the grid-point normals, [hlm.md](hlm.md)), the tile UVs, and
one weather particle (snow/rain) per tile.

**For right-handed targets (e.g. glTF export)**: Mirror positions as in [hlm.md](hlm.md) with `z -> -z` and reverse the
winding (or swap the indexes `i0, i2, i1` or `front_face`); the diagonal stays the same grid connection. `nml-render`
(wgpu) does **not** mirror but renders in the original left-handed system.

## Coordinate system and units

As in [hlm.md](hlm.md): tile `(x, z)` begins at `(x * SIZE_X, z * SIZE_Z)`, left-handed, y up. In the top view +x lies
to the right and +z up; the minimap is written this way (top image row = largest `z`).

## Ignored sections

None. All 4 bytes per tile are read and used.

## Validation

Every `.tlm` read sequentially (the read pointer ends exactly at the end of the file), the size checked against
`[TERRAIN]` and every GUID against the `[TEXTURE]` blocks of the project file, as well as
`WATER_BASE_TEXTUREGUID`/`WATER_REFRACTION_TEXTUREGUID`. Result: all 55 levels without errors. The orphaned
`kampagne1/kamp1-6/kamp2-10.tlm` contains only `0xFFFFFFFF` (16,384 unknown GUIDs, no texture) and belongs to no
`.lvl`.

Visual check: the minimap computed with the game formula for `multiplayer/[4players]-donut-isle` shows the shape (ring
with an inner lake and a center island), water channels and forest areas as in the preview image
`[4players]-donut-isle.tga`.

## Open questions

- The tile UVs are used only in the fallback drawing path, not in the terrain mesh.
