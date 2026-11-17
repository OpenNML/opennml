# Height map `.hlm`

Rust: `nml_formats::hlm::parse_hlm`; derived values `nml_world::terrain::Terrain`.

Binary height map of a level: one record per grid point of the terrain with position, normal and color GUID. Grid
size and spacing are not in the file but in the `[TERRAIN]` block of the `.wld` of the same name ([wld.md](wld.md)).
How all level files work together: [lvl.md](lvl.md).

## Purpose and locations

- 57 effective files under `data/levels/**` (55 levels that belong to a `.lvl`, plus the two orphaned partial files
  `kampagne1/kamp1-6/kamp2-10.hlm` and `multiplayer/[5players]-vicious circle.hlm`, see [lvl.md](lvl.md)).
- Sizes in the shipped data: 152,100 bytes (64 x 64 tiles, 7 levels), 599,076 bytes (128 x 128, 45 levels),
  2,377,764 bytes (256 x 256, 2 levels), 10,404 bytes (16 x 16, `kampagne3/kamp3-4/oxdebug`).
- The file name is the level path with the extension `.hlm`.

## Loading

The `.hlm` is loaded when the `[TERRAIN]` block of the `.wld` has been parsed. The game reads one record per grid
point (12 + 12 + 4 + 4 + 4 bytes) into the vertex grid. The grid has `vertsX = DIM_X + 1` by `vertsZ = DIM_Z + 1`
points, initialized with position `(SIZE_X * x, 0, SIZE_Z * z)`, normal `(0, 1, 0)` and color GUID `-1`.

After reading, the game computes the water alpha and the border flag per grid point, then (after the `.tlm` has been
read, [tlm.md](tlm.md)) two triangle normals per tile. It recomputes the grid point normals (overwriting the normals
from the file) and the lit vertex colors, and builds the render chunks (`CHUNK_SIZE`^2 tiles) and the minimap from
heights, normals and texture colors.

## Layout

Little-endian, no header. The file consists of exactly `(DIM_X + 1) * (DIM_Z + 1)` records of 36 bytes;
record `i` belongs to the grid point `x = i mod (DIM_X + 1)`, `z = i div (DIM_X + 1)` (row by row,
`z` outer, `x` inner).

### Record (36 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x00` | `f32[3]` | `position` | `(x, y, z)` in world units; `x = SIZE_X * column`, `z = SIZE_Z * row` (exact in all 910,807 points), `y` = height |
| `0x0C` | `f32[3]` | `normal` | Grid point normal (pointing up, not normalized); in the shipped data exactly equal to the game's formula, but the game recomputes it anyway (see below) |
| `0x18` | `u32` | `unknown_18` | 0 in the shipped data (4 exceptions with garbage values); read by the game, no use found |
| `0x1C` | `u32` | `unknown_1c` | like `unknown_18` (4 exceptions) |
| `0x20` | `u32` | `color_guid` | `COLOR_GUID` from the project file ([prj.md](prj.md)) or `0xFFFFFFFF` = no color (white) |

Short excerpt (`kamp1-1.hlm`, first record and start of the second):

```
00000000: 0000 0000 0000 96c3 0000 0000 0000 0080  x=0.0 y=-300.0 z=0.0 nx=-0.0
00000010: 0000 803f 0000 0080 0000 0000 0000 0000  ny=1.0 nz=-0.0 unknown_18=0 unknown_1c=0
00000020: ffff ffff 0000 0043 ...                  color_guid=-1 | next point x=128.0
```

### Derived values in memory

The game derives these values per grid point after reading:

| Name | Origin |
|---|---|
| `normal` | Recomputed: the **negated mean (not normalized)** of the adjacent triangle normals (inner points have 6: tile `(x,z)` triangle A, `(x-1,z)` A+B, `(x,z-1)` A+B, `(x-1,z-1)` B; fewer at the edge). Points **up** (flat: `(0, 1, 0)`), length < 1 on slopes |
| `lit_color` | Per channel `clamp(c/255 * f * L, 0, 255)` with `c` = `COLOR_DIFFUSE` of the color GUID (255 if none), `L` = channel of `[TERRAIN] LIGHT_COLOR`, `f = n * normalize(LIGHT_POSITION) + 1 - 0.01 * LIGHT_INTENSITY` with the grid point normal `n` (pointing up); as `0x00RRGGBB`; becomes the vertex color of the terrain mesh ([tlm.md](tlm.md#terrain-mesh-chunk-vertex-buffer)) |
| `display_color` | Default `-1`; per frame `lit_color` times the fog-of-war factor, also the color of the water |
| `water_alpha` | `clamp(int(255 * sqrt(-(y - WATER_LEVEL) / WATER_MAXDEPTH)), WATER_MINALPHA, WATER_MAXALPHA)`; above the water the root is invalid and the result becomes `WATER_MINALPHA`. Edge cases of the Rust implementation (`nml_world::terrain::water_alpha`, not present in the shipped data, behavior of the game not checked): `WATER_MAXDEPTH` <= 0 gives a quotient of +-infinity and Rust saturates to `WATER_MAXALPHA` (the game possibly `WATER_MINALPHA`); swapped limits `MINALPHA` > `MAXALPHA` are ordered by Rust because `i32::clamp` would panic otherwise. The conversion from floating point to integer truncates (`as`) |
| `is_edge` | 1 if `x = 0`, `z = 0`, `x = DIM_X * SIZE_X` or `z = DIM_Z * SIZE_Z` |

**Triangle decomposition**: tile `(x, z)` consists of triangle A = `(x,z)`, `(x,z+1)`, `(x+1,z)` and triangle
B = `(x,z+1)`, `(x+1,z+1)`, `(x+1,z)`; the visible diagonal runs from `(x, z+1)` to `(x+1, z)`.
This is confirmed by the index buffer that is actually drawn (triangle strip with 4 indices per tile,
[tlm.md](tlm.md#terrain-mesh-chunk-vertex-buffer)); the game uses the same decomposition for the tile face normals.
Heights between the grid points are linear on these triangles (this is how the check script verifies the object
heights).

**Normal direction in the game**:

- **Face normals** (per tile): for A with `v0, v1, v2` as above, the game computes
  `cross(v0 - v2, v0 - v1)` = `normalize((v2 - v0) x (v1 - v0))`, likewise for B. On flat terrain this is
  `(0, -1, 0)`: **the face normals point down**.
- **Grid point normals**: the game sums the adjacent face normals, negates the sum and divides it by the count,
  **without normalizing**: the result points **up**, length < 1 on slopes.
- Vertex lighting and the minimap use the grid point normals (pointing up; the minimap averages the four corner
  normals, also without normalization). Only the steepness flag per tile uses the face normals: it is set to 1 if
  `(n_A + n_B)/2 * (0, -1, 0) <= PATH_NORMAL`, i.e. the upward component of the mean tile normal is
  <= `PATH_NORMAL` (steep tile).
- Check: levels with the editor default light (`kamp2-10` in `kamp1-6`, `oxdebug`: `LIGHT_POSITION 0 1 0`,
  `LIGHT_INTENSITY 100`) give the neutral factor `f = 1` with `n = (0,1,0)`.
- The normals stored in the file are **exactly** these grid point normals (all 910,807 points equal to within 1e-4,
  checked by script); the editor therefore uses the same formula.

## Coordinate system and units

- World space = model space from [rdo.md](rdo.md): **left-handed (D3D), y up**, same units. A tile is always
  128 x 128 units in the shipped data (`TERRAIN_SIZE_X/Z` = 128), a game figure is about 137 high.
- The map extends from `(0, 0)` to `(DIM_X * SIZE_X, DIM_Z * SIZE_Z)` in `x`/`z`, e.g. 16,384 x 16,384 for
  128 x 128 tiles.
- Heights in the shipped data between -728 and +1,520; the water surface is `[TERRAIN] WATER_LEVEL` (mostly 0 or
  slightly negative). Underwater ground is typically at -300.
- **Top view**: seen from above (+y), +x is on the right and +z is up; this matches the preview images of the
  multiplayer maps (`[n players]-*.tga`, origin bottom left, file row = `z`) and `map1.6.tga`. The check script writes
  height images in exactly this orientation (top image row = largest `z`).
- For a right-handed renderer the same mirroring applies as for the models (`z -> -z`, reverse the winding),
  otherwise the map appears mirrored.

## Ignored areas

| Area | Size | Reason |
|---|---|---|
| `normal` (`0x0C`) | 12 per point | read, but recomputed with the same value before any use in the lighting; in the shipped data equal to the game's grid point normal to within 1e-4 at all 910,807 points |
| `unknown_18`, `unknown_1c` | 8 per point | read, no read access found in the terrain code (building the render chunks copies only position and the lit color); values 0 in the shipped data except for 4 points each |

## Validation

A check script reads every `.hlm` with the layout above (every byte assigned to a field, read pointer exactly at the
end of the file) and checks the size against `[TERRAIN]` of the `.wld`, the exact grid position of `x`/`z`, finite
values and the color GUIDs against the project file. Result over the corpus:

| Check | Result |
|---|---|
| Levels | 55 of 55 OK |
| Grids | 128 x 128: 45, 16 x 16: 1, 256 x 256: 2, 64 x 64: 7 |
| Color GUID set / none | 72,663 / 838,144 points |
| `unknown_18` zero / non-zero | 910,803 / 4 |
| `unknown_1c` zero / non-zero | 910,803 / 4 |
| `normal` equal to the game's formula (< 1e-4) | 910,807 points |
| Object height on terrain (`|dy| < 0.5`) / on the water surface | 123,573 / 2,510 |
| Partial files without `.lvl` (not loadable by the game) | 2 (`kampagne1/kamp1-6/kamp2-10`: `.tlm` has 16,384 tiles with unknown texture GUID, e.g. `0xffffffff`) |

The two orphaned `.hlm` also read without errors (`kamp2-10` in `kamp1-6` is a flat, empty 128 grid with height 0).

Visual check: a 16-bit grayscale image (black = lowest, white = highest height) of `kamp1-1` shows an island group
in the sea (13,715 of 16,641 points under water); the minimap generated for it shows the same islands with beach
and rock textures. For `kamp1-6` the height image coincides with `map1.6.tga` (8-bit gray, correlation 1.000; gray
value about linear -300 ... 900); the minimap of `[4players]-donut-isle` matches the preview image of the map in
position and shape.

## Open questions

- Meaning of `unknown_18`/`unknown_1c` (practically always 0 in the shipped data).
