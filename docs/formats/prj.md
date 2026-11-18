# Project file `.prj` and `default.dat`

Rust: `opennml_formats::prj::parse_project`. It also reads `OBJ_UPGRADES` (`ProjectObject::upgrades`, the upgrades of the
type in file order) and `[UPGRADE]` (`Project::upgrades`, `ProjectUpgrade { guid, name }` by `UPG_GUID`); the rules
build `Rules::upgrade_types` (upgrade -> types) and `Rules::upgrades` from them. Corpus test
`nml_wiese_lists_upgrades_and_their_types`.

The project file is the level editor's catalog: all placeable object types (GUID -> model), ground textures, vertex
colors and upgrades. Every `.lvl` names it in `PROJECT_FILE` ([lvl.md](lvl.md)); `.wld` (`OBJ_GUID`), `.tlm`
(`TEX_GUID`) and `.hlm` (`COLOR_GUID`) refer to its GUIDs. `data/projects/default.dat` holds the default configuration
of all object types and upgrades.

Text format: bracket grammar from [text.md](text.md), cp1252, CRLF.

`OBJ_BLOCK_PATH` takes effect only with the value exactly 1 and only for nature objects.

## Purpose and locations

| File | Header | Content |
|---|---|---|
| `data/projects/nml-wiese.prj` | `PROJECTFILE V1.08f` | 52 `[OBJECTFOLDER]`, 512 `[OBJECT]`, 10 `[UPGRADEFOLDER]`, 232 `[UPGRADE]`, 48 `[TEXTURE]`, 31 `[HEIGHT]`, 24 `[COLOR]`, 62 `[CLOSE]`, 10 `BRUSH` |
| `data/projects/default.dat` | `PROJECT CONFIGFILE V1.4b` | `[CONFIGDATA]` (512 `OBJECT_GUID`), `[UPGRADES]` (232 `UPGRADE_GUID`), `[PATTERNS]` (empty); from `nml10.rda` |
| `data/projects/dropdown.ini` | - | Selection lists of the editor (`VALUEGUID n` + `DROPDOWN value text`); the game reads them for the nation abbreviations (`VALUEGUID 0`: `WID PRI SPA SID ENG PAT`) |
| `data/projects/lightdata.ini` | - | 32 named light presets (`[TERRAIN_LIGHT]`, `[OBJECT_LIGHT]`, corresponding to `[TERRAIN] LIGHT_*` and `[LIGHT]` of the `.wld`), switched at run time by script; not part of the project file, see [lightdata.md](lightdata.md) |

There is only one project file; all 55 levels use it.

## Reading

The game reads the project file line by line. It keeps three maps: object types by `OBJ_GUID`, textures by `TEX_GUID`
and colors by `COLOR_GUID`. The model of a game object comes from the `OBJ_FILENAME`/`OBJ_GAME_FILENAME` of its type.
`default.dat` has three sections and only the released values are used (see [lvl.md](lvl.md)).

## Layout

### `[OBJECT]` (object type)

| Key | Format | Meaning |
|---|---|---|
| `OBJ_NAME` | `"text"` | Display name in the editor (e.g. `"Hauptquartier (Spa)"`) |
| `OBJ_GUID` | `%u` | Type GUID (key of the map; unique in the shipped data) |
| `OBJ_FILENAME` | `"path"` | Model; **the extension is replaced by `.rdo`**, also for `.rdw` |
| `OBJ_GAME_FILENAME` | `"path"` | **Overrides** the model path unchanged (without replacing the extension) if the text is longer than 1 character; 38 types, e.g. `dragoner_eng_zu_pferd.rdw` -> `dragoner_eng.rdo` (horse via `SUBOBJECT` of the object `.cfg`) |
| `OBJ_SHADOW_FILENAME` | `"path"` | Shadow model (`_shd.rdo`), extension -> `.rdo`; the game uses only the hull and the texture of material 0 as a terrain decal (shadow kind 1; for buildings with a ground plate as a decal lit like the terrain) |
| `OBJ_GROUNDPLANE_FILENAME`, `OBJ_SCAFFOLD_FILENAME`, `OBJ_SELECTION_FILENAME` | `"path"` | Ground plate (`.rdo` under `ground\`, 75 buildings: decal with terrain light), scaffold (set together with GUID 1108/1109/1117/2009/2010/3505 = ship marker for the shadows), selection marker (`.tga`) |
| `OBJ_SELECTION_RADIUS` | `%f` | Half edge length of the selection ring (`opennml_formats::prj::SelectionMarker`; the ring decal applies only for file names longer than 1 character, otherwise a line rectangle) |
| `OBJ_SELECTION_USE_BBOX` | `%i` | != 0: box over the model hull instead of a ring (96 types: buildings, gold deposits, fields) |
| `OBJ_XROT`, `OBJ_YROT`, `OBJ_ZROT` | `%f` | Default rotation of the type (33 types with `OBJ_YROT` != 0); no reader found in the placement code, probably applied only by the editor when placing |
| `OBJ_ONSURFACE`, `OBJ_ONWATER` | `%i` | Can be placed on terrain or on water (23 types on water) |
| `OBJ_BLOCK_PATH` | `%i` | Blocks paths |
| `OBJ_RANDOM_ROT` | `%i` | Random rotation when placing (editor) |
| `OBJ_UPGRADES` | `{ UPG_GUID n ... }` | List of the upgrades of the type; reader `ProjectObject::upgrades` |
| `OBJ_REPLACE`, `OBJ_REALTIME_SHADOW`, `OBJ_LOWEST_POINT`, `OBJ_USE_CONFIG`, `OBJ_CONFIG_FILENAME` | - | **Ignored by the game** (editor only) |

The **class** of an object type (modules, category bits; from it the shadow kind, dummy effects, player slot) is not
in the project file but in `data/gifcon/property.ini`; types without an entry there are not created by the game.

**Model resolution** (GUID -> model, as the game reads the project file):
`model = OBJ_GAME_FILENAME if its length > 1, else OBJ_FILENAME with the extension .rdo`. So a `.rdo` exists in the
VFS for all 512 types (474 from `OBJ_FILENAME`, 38 from `OBJ_GAME_FILENAME`). The object `.cfg` next to the `.rdo`
supplies animations and sub-objects ([object-cfg.md](object-cfg.md)). For a plain map view the `.rdw` from
`OBJ_FILENAME` is an alternative (rigid overall pose including the mount, [rdw.md](rdw.md)).

### `[TEXTURE]` (ground texture)

| Key | Format | Meaning |
|---|---|---|
| `TEX_GUID` | `%u` | GUID (`.tlm`, `WATER_*_TEXTUREGUID`) |
| `TEX_NAME` | `"text"` | Name |
| `TEX_FILENAME` | `"path"` | Texture (`.tga` under `data/textures/`) |
| `TEX_BITS` | `%i` | Tile bits ([texturebits.md](texturebits.md)); values 0, 1, 4 |
| `TEX_GROUP_ID` | `%i` | Blend group for the alpha maps ([tlm.md](tlm.md)); 0 = no group |
| `TEX_MAP_COLOR` | `%i` | Minimap color as COLORREF `0x00BBGGRR` |

### `[COLOR]`

`COLOR_NAME "..."`, `COLOR_GUID n`, `COLOR_DIFFUSE c` (COLORREF `0x00BBGGRR`). Vertex colors of the `.hlm` (gray,
sand, water and blood colors).

### Editor only (ignored by the game)

`[OBJECTFOLDER]` (`OBJ_FOLDER_NAME`, `..._NEUTRAL`, `..._FOR_ALL`, `..._USECFGFILE`, `..._FILENAME` = property file
`data/config/*.cfg` of the group) and `[CLOSE]` form the folder structure of the editor; `[UPGRADEFOLDER]` and
`[UPGRADE]` (`UPG_NAME`, `UPG_GUID`) the upgrade list; `[HEIGHT]` (`HEIGHT_NAME`, `HEIGHT_GUID`, `HEIGHT_FACTOR`,
`HEIGHT_TYPE`) and `BRUSH n n` height brushes. `PTN_GUID`/`PTN_FILENAME`/`PTN_UPGRADES` (patterns) are known to the
parser but do not occur in the shipped data.

### `default.dat`

```
PROJECT CONFIGFILE V1.4b
[CONFIGDATA] { OBJECT_GUID n { CONFIG_GUID ... } ... }
[UPGRADES]   { UPGRADE_GUID n { CONFIG_GUID ... } ... }
[PATTERNS]   { }
```

The `CONFIG_GUID` blocks are laid out as in [lvl.md](lvl.md); 8,932 property values (e.g. sight range, cost, hit
points) for all 512 object types and 232 upgrades.

## Coordinate system and units

No coordinates except the default rotations (`OBJ_XROT/YROT/ZROT`, radians) and `OBJ_SELECTION_RADIUS` (world units).

## Ignored sections

See "Editor only" and the ignored `OBJ_*` keys above.

## Corpus counts

Measured on `data/projects/nml-wiese.prj`: every key matches the schema and its argument count, every `OBJ_GUID` is
unique, and every resolved model, shadow model and texture file exists in the VFS.

- 24 colors, 512 objects (474 models from `OBJ_FILENAME`, 38 from `OBJ_GAME_FILENAME`), 48 textures, 232 upgrades.
- `OBJ_CONFIG_FILENAME` occurs in 512 objects and is ignored by the game, as are the other editor keys.

Measured on `default.dat`: all `OBJECT_GUID`, `UPGRADE_GUID` and `CONFIG_GUID` resolve; 8,932 property GUIDs, 512
object GUIDs, 232 upgrade GUIDs.

## Open questions

- Whether the game applies the default rotation of the type anywhere (in the shipped data the `OBJ_YROT` of the `.wld`
  files already contains the final rotation).
