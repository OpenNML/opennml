# World file `.wld`

Rust: `opennml_formats::wld::parse_world`; placement `opennml_world::level::load_level`.

Text file (bracket grammar from [text.md](text.md), cp1252, CRLF) with the terrain parameters, named areas and
cameras, light/shadow/fog and **all placed objects** of a level. The height and texture maps lie next to it in binary
form ([hlm.md](hlm.md), [tlm.md](tlm.md)); players, resources and object configuration are in the `.lvl`
([lvl.md](lvl.md)); object types in the project file ([prj.md](prj.md)).

## Purpose and locations

- 57 effective files under `data/levels/**`: for each of the 55 `.lvl` a `.wld` of the same name, plus two orphaned ones
  (`kampagne1/kamp1-6/kamp2-10.wld`, `multiplayer/[5players]-vicious circle.wld`).
- Header line `WORLDFILE V...` (versions `V0.9b` ... `V1.09i`, mostly `V1.08e`); the game does not check it.
- In the shipped data: 126,085 objects, 2,948 areas, 415 cameras in the 55 levels.

## Reading

The game reads **one** block of the `.wld` per call and reports the file position (for the loading bar) or -1 at the
end. A mode "terrain only" skips `[OBJECT]` blocks, forts and the script file; it is used when loading a savegame
(`.lvl` -> project file, `.wld` without objects). Blocks are recognized by a substring search on the trimmed line;
sub-lines are read up to the first line with `}` (`[OBJECT]`) or up to a line that is exactly `}`. A line with
`LEVELFILE` would be read as a version line (a leftover); `WORLDFILE` is ignored. At the end of the `[TERRAIN]` block
the game builds the grid; afterwards it loads `.hlm`, `.tlm` and the `.lvl` (order in [lvl.md](lvl.md)).

## Layout

Blocks in any order; in the shipped data always `WORLDFILE`, `[TERRAIN]`, `[AREA]`*, `[CAMERA]`*, `[LIGHT]`,
`[SHADOW]`, `[FOG]`, `[OBJECT]`*.

### `[TERRAIN]`

| Key | Format | Meaning (values in the shipped data) |
|---|---|---|
| `TERRAIN_DIM_X`, `TERRAIN_DIM_Z` | `%i` | Tiles in x/z (16, 64, 128, 256; always square) |
| `TERRAIN_SIZE_X`, `TERRAIN_SIZE_Z` | `%i` | Edge length of a tile in world units (always 128) |
| `LIGHT_POSITION` | `%f %f %f` | Direction to the terrain light (used normalized), e.g. `800 400 -1300` |
| `LIGHT_COLOR` | `%i %i %i` | Terrain light color R G B (stored as `0x00BBGGRR`) |
| `LIGHT_INTENSITY` | `%i` | Brightness: factor `n*L + 1 - 0.01 * I` (see [hlm.md](hlm.md)) |
| `WATER_LEVEL` | `%f` | Height of the water surface (-800 ... 200, mostly 0/-10) |
| `WATER_USETEXTURE` | `%i` | Use the water texture (0/1) |
| `WATER_BASE_TEXTUREGUID`, `WATER_REFRACTION_TEXTUREGUID` | `%u` | `TEX_GUID` of the water textures |
| `WATER_TEXTURETILING` | `%i` | Tiling of the water texture |
| `WATER_MAXDEPTH`, `WATER_MINALPHA`, `WATER_MAXALPHA` | `%i` | Water alpha per grid point (formula in [hlm.md](hlm.md)) |
| `PATH_NORMAL` | `%f` | Steepness limit (0 ... 0.9): per tile the game marks a tile as steep if `-(n_A.y + n_B.y)/2 <= PATH_NORMAL`, that is, if the upward component of the mean tile normal is at most `PATH_NORMAL` (use in pathfinding not examined), see [hlm.md](hlm.md) |
| `CHUNK_SIZE` | `%i` | Tiles per drawing block (always 16; Rust accepts at most 256, see "Validation") |
| `TEXTURE_BASE_XSIZE`, `_YSIZE` | `%i` | Texels of the base texture per block (8; set to 32 without the detail switch; Rust: at most 1024) |
| `TEXTURE_ALPHA_XSIZE`, `_YSIZE` | `%i` | Texels of the alpha maps per block (32; Rust: at most 1024) |
| `TEXTURE_TILING` | `%i` | Tiles per repetition of the ground texture (4) |
| `BUILD_AREA_NORMAL` | - | **Ignored by the game** (49 files, editor only) |

### `[AREA]`

| Key | Format | Meaning |
|---|---|---|
| `AREA_NAME` | `"text"` | Name (stored in lowercase, key for scripts `.abl`) |
| `AREA_RECT` | `%d %d %d %d` | `x0 z0 x1 z1` in **tile coordinates, inclusive** (`noshadow` areas are taken over with `x1 + 1`, `z1 + 1` as an exclusive end) |

Names that begin with `fort` create a fort object when fully loading; names that begin (after lowercasing) with
`noshadow` (`left(8)`) switch off shadows for objects whose tile `floor(x/SIZE_X)`, `floor(z/SIZE_Z)` lies in the
rectangle at placement (point-in-rectangle test with `x1 + 1`, `z1 + 1`); in the shipped data 10 areas, all in
`kamp0-1`. All others go into a name table for scripts. In the shipped data 2,942 rectangles lie in the map, 6 are
faulty (negative, beyond the edge, memory garbage in `kamp1-2`).

### `[CAMERA]`

`CAMERA_NAME "..."`, `CAMERA_FROM x y z` (eye point), `CAMERA_AT x y z` (target point), `CAMERA_ROLL r` (always 0);
world coordinates. Named camera positions for script sequences; eye points often lie far outside the map.

### `[LIGHT]`, `[SHADOW]`, `[FOG]`

| Block | Keys | Meaning |
|---|---|---|
| `[LIGHT]` | `LIGHT_POSITION %f*3`, `LIGHT_COLOR %i*3`, `LIGHT_INTENSITY %i` | Start light for objects as numbers (corresponds to `[OBJECT_LIGHT]` in `data/projects/lightdata.ini`; `[TERRAIN] LIGHT_*` corresponds to `[TERRAIN_LIGHT]`). Many levels switch to a preset by script right at the start ([lightdata.md](lightdata.md)) |
| `[SHADOW]` | `SHADOW_LIGHT_POSITION %f*3`, `SHADOW_INTENSITY %i` | Direction **to** the light (normalized when reading) and opacity of the shadow textures (alpha `I*k/255`; has no effect on the decals of kind 1) |
| `[FOG]` | `FOG_COLOR %d*3`, `FOG_START %f`, `FOG_END %f` | Fog color and range (`FOG_START/END` are missing in 2 files); in the renderer `start = distance(eye, target) + FOG_START`, `end = start + FOG_END` |

### `[OBJECT]` (skipped in "terrain only" mode)

| Key | Format | Meaning |
|---|---|---|
| `OBJ_GUID` | `%u` | Object type = `OBJ_GUID` of the project file |
| `OBJ_POSITION` | `%f %f %f` | World position of the object origin |
| `OBJ_SCALE` | `%f %f %f` | Scale (1,1,1 without it); never set in the shipped data |
| `OBJ_XROT`, `OBJ_YROT`, `OBJ_ZROT` | `%f` | Rotation angles in **radians** (not normalized, e.g. -21.99); X/Z always 0 in the shipped data |
| `OBJ_COLOR` | `%d` | Player slot 0...7 (= `PLAYER_COLOR` of a `[PLAYER]` of the `.lvl`) or 8 = neutral |
| `OBJ_NAME` | `"text"` | Optional script name (lowercase), 2,373 objects |
| `OBJ_STAND_ON` | `%i` | Always 0 in the shipped data (690 objects without the key) |
| `[CONFIG]` | block | Property values of this object (`CONFIG_GUID` from `data/config/*.cfg`, layout as in [lvl.md](lvl.md)); 606 objects |

**What the game does with an object:**

1. `OBJ_GUID` 4200 (`0x1068`, start point) creates no object but sets the start position of player `OBJ_COLOR`.
2. GUIDs 4104-4106 (surf), 3411 (fish) and 3408 (eagle) are created as ambient effects (only if `Engine.ini`
   `ParticlesQuality` >= 1).
3. Otherwise create a game object: translate the GUID via `data/gifcon/translate.ini` (rider -> rider + horse), look the
   type up in `data/gifcon/property.ini` - types without an entry are **not** created; the object only if
   `0 <= x < DIM_X * SIZE_X` and `0 <= z < DIM_Z * SIZE_Z` (upper limit **exclusive**, otherwise silently discarded).
   The player slot comes from `OBJ_COLOR`; types with the category bits `0x100`/`0x200`/`0x400` (`VALUEID 100` in
   `property.ini`) are moved to the internal slots 9/10/8. The description carries the flag `0x100` ("preplaced"), so
   the modules initialize as finished (buildings with their dummy effects).
4. Apply the placement: the position is taken over unchanged, scale 1 without `OBJ_SCALE`; internal facing =
   `deg(OBJ_YROT) - 90 degrees`.
5. Model: the model of the type from the project file (rule in [prj.md](prj.md)); then the shadow kind.

At the end of the file the game sets the weather to 0: the `.wld` has no weather key, every level starts without
weather; snow/rain are switched on only by scripts.

## Coordinate system and units

- **World space = model space** from [rdo.md](rdo.md): left-handed (D3D), +y up, same units. The map covers
  `x` in `[0, DIM_X * SIZE_X]`, `z` in `[0, DIM_Z * SIZE_Z]` (16,384 x 16,384 for 128 maps); 1 tile = 128 units, about
  one figure height (137).
- **Height**: `OBJ_POSITION.y` is already the final height: for 123,573 of 126,085 objects = terrain height at `(x, z)`
  on the triangulation from [hlm.md](hlm.md) (|dy| < 0.5), for 2,510 = `WATER_LEVEL` (surf, fish, ships, harbors - types
  with `OBJ_ONWATER 1`), 2 others.
- **World matrix** (row vectors `v' = v * M`):
  `M = Ry(OBJ_YROT) * Rz(OBJ_ZROT) * Rx(OBJ_XROT) * T(OBJ_POSITION)` (the game's matrix multiplication of `A` and `B` yields `B * A`). The
  rotation matrices of the game are the **transposed** D3DX matrices:
  `Ry(theta)` = rows `(cos theta, 0, sin theta)`, `(0, 1, 0)`, `(-sin theta, 0, cos theta)` =
  `D3DXMatrixRotationY(-theta)`.
- **Facing**: models face -Z; after the rotation the facing points to `(sin theta, 0, -cos theta)`. In the top view
  (+x right, +z up) a positive `OBJ_YROT` therefore turns **counterclockwise**; `theta = 0` faces down (-z). Confirmed
  by the debug line of the game (end point `(cos(theta - 90 degrees), sin(theta - 90 degrees))` in x/z).
- **For right-handed targets (e.g. glTF export; z mirrored as in [rdo.md](rdo.md))**: position `(x, y, -z)`; rotation
  `glam::Mat4::from_rotation_y(OBJ_YROT)` (column vectors) - the mirroring `M * Ry(theta) * M` with
  `M = diag(1, 1, -1)` gives exactly this matrix. So the world matrix is
  `Mat4::from_translation(vec3(x, y, -z)) * Mat4::from_rotation_y(theta)`, applied to the z-mirrored model vertices.
  `opennml-render` (wgpu) does **not** mirror but uses the left-handed original matrices with `FrontFace::Cw`.

## Ignored sections

| Section | Reason |
|---|---|
| `WORLDFILE V...` | Header line, not evaluated |
| `[TERRAIN] BUILD_AREA_NORMAL` | Not a key of the terrain block (49 files) |
| `OBJ_STAND_ON` | Read, no use found; always 0 |

## Validation

Every `.wld` parses; every key checks out against the schema (argument count, block structure), the grid sizes against
`.hlm`/`.tlm`, every object (type in the project file, model present, position in the map, `OBJ_COLOR` matching a
`[PLAYER]`) and all `CONFIG_GUID`. Result for the 55 levels, without errors:

| Count | Value |
|---|---|
| Areas outside the map (data errors) | 6 |
| Areas in the map | 2,942 |
| Objects with y on the terrain (\|dy\| < 0.5) | 123,573 |
| Objects with y on the water surface | 2,510 |
| Objects with other y | 2 |
| Objects | 126,085 |
| Neutral objects | 114,858 |
| Player objects | 11,227 |
| `WORLDFILE` lines ignored by the game | 55 |
| `[TERRAIN]` `BUILD_AREA_NORMAL` ignored by the game | 49 |

Orphaned and not loadable: `multiplayer/[5players]-vicious circle.wld` (objects with `OBJ_COLOR` 0-4 without a
matching `.lvl`; the loadable version is called `[5players]-viciouscircle`).

Visual check: `kampagne1/kamp1-1` lists 1,212 objects (palms and other trees, fish, surf; Spanish buildings such as
headquarters, shipyard, warehouse, houses, towers; English warships, pioneers, guards) with coordinates in `[0, 16384]`.

**Deviations of the Rust implementation** (`opennml_formats::wld::parse_terrain`, plausibility limits only against
corrupted files; the game does not check this): `CHUNK_SIZE` above 256 and `TEXTURE_BASE_*SIZE`/`TEXTURE_ALPHA_*SIZE`
above 1024 are rejected with `FormatError::Invalid`. The texture sizes determine image and alpha map allocations in
`opennml-world`; `CHUNK_SIZE` <= 256 ensures that the tile index `a * C + b` of a block tile list fits into `u16`. The
shipped data uses texture sizes 8/32 and `CHUNK_SIZE` 16.

## Open questions

- Meaning of `OBJ_STAND_ON`; how pathfinding uses the steepness flag from `PATH_NORMAL`.
- Meaning of the slots 8-10 for neutral object types - for the map view "`OBJ_COLOR` 8 = neutral" is enough. The bits
  themselves are clear: category bits from `property.ini`.
