# Level `.lvl` and composition of a level

Rust: `opennml_formats::lvl::parse_level` (project file, players); composition `opennml_world::level`.

A level consists of several files with the same base name. The `.lvl` is the entry point
(text file, bracket grammar from [text.md](text.md), cp1252, CRLF): it names the project file and
contains players, starting resources and the object/upgrade configuration. Terrain, areas, cameras and
placed objects are in the `.wld`.

## Purpose and locations

**55 effective levels** (`.lvl` by archive order; the archives contain 56 `.lvl` entries -
`kampagne1/kamp1-6/kamp1-6.lvl` is in `NML7.RDA` and is overwritten by `nml10.rda`):

| Folder under `data/levels/` | Count | Remark |
|---|---|---|
| `kampagne1/` | 6 | `kamp1-1` ... `kamp1-6` |
| `kampagne2/` | 10 | `kamp2-1` ... `kamp2-10` |
| `kampagne3/` | 15 | `kamp3-1` ... `kamp3-14` and `kamp3-4/oxdebug` (16 x 16 tiles, test level) |
| `tutorial0-1/`, `tutorial0-2/` | 2 | `kamp0-1`, `kamp0-2` |
| `menülevel/` | 2 | `menü`, `menü2` (background of the main menu) |
| `multiplayer/` | 20 | `[2players]-...` to `[8players]-tribes`; list for the menu in `multiplayer/levels0000.ini` (`NAME` text ID, `TYPE`, `PATH`) |

**Partial files** (base = path of the `.lvl` without the extension):

| File | Content | Format | In the shipped data |
|---|---|---|---|
| `.lvl` | Project file, players, resources, object/upgrade configuration | Text, this document | 55 |
| `.wld` | Terrain parameters, areas, cameras, light, **objects** | Text, [wld.md](wld.md) | 55 (+2 orphaned) |
| `.hlm` | Heights and vertex colors | binary, [hlm.md](hlm.md) | 55 (+2) |
| `.tlm` | Ground texture per tile | binary, [tlm.md](tlm.md) | 55 (+2) |
| `.ldg` | Loading screen | Text, [ldg.md](ldg.md) | 32 (+1 in `kampagne2/kamp2-10 old/`) |
| `.cmm` | Camera paths | Text (see below) | 27 (+1), **all 0 bytes** |
| `.abl` | Level script | Script | one `<base>.abl` per level, plus further scene `.abl` |
| `map*.tga`, `[n players]-*.tga` | Height image (only `kamp1-6/map1.6.tga`) or 256 x 256 preview image of the multiplayer maps | TGA | 1 + 20; not loaded through the level path by `NML.exe` (preview: loading code not examined) |

Orphaned partial files without a `.lvl` (not loadable by the game): `kampagne1/kamp1-6/kamp2-10.{hlm,tlm,wld}`
(empty map, `.tlm` only `0xFFFFFFFF`), `multiplayer/[5players]-vicious circle.{hlm,tlm,wld}` (older
version of `[5players]-viciouscircle`), `multiplayer/[4players]-railroadriot.cmm`.

Files shared by all levels: `data/projects/nml-wiese.prj` (the only project file, named by all 55 `.lvl`
through `PROJECT_FILE`) and `data/projects/default.dat` ([prj.md](prj.md)),
`data/projects/dropdown.ini` (nation abbreviations), `data/gifcon/gametypevictory.ini`/`gametypedefeat.ini`.

## Loading order

The `.lvl` is read twice: first line by line up to `PROJECT_FILE` (the path is taken from the quotation marks),
and later completely.

1. Create the game object.
2. Read the `.lvl` up to `PROJECT_FILE` -> **project file** -> open the `.wld` -> **`.cmm`**.
3. **`.ldg`**: single player `<base>.ldg`; if it is missing, or in multiplayer mode,
   `data/gifcon/multiplayer.ldg` or `data/gifcon/default.ldg`.
4. Read the `.wld` block by block with a loading bar. In the `[TERRAIN]` block:
   1. create the grid,
   2. initialize the per-player grids (`DIM_X`, `DIM_Z`),
   3. **`.hlm`**,
   4. initialize the game world (`DIM_X`, `DIM_Z`, `SIZE_X`, `SIZE_X`, `SIZE_Z`; cell size x/y/z; loads
      `data/gifcon/property.ini`),
   5. **`.tlm`**, then the vertex lighting and the render chunks, possibly the minimap,
   6. `data/projects/dropdown.ini`, `gametypevictory.ini`, `gametypedefeat.ini`,
   7. **`default.dat`**,
   8. the **`.lvl`** completely,
   9. camera limits = `DIM * SIZE`.

   Then `[AREA]`, `[CAMERA]`, `[LIGHT]`, `[SHADOW]`, `[FOG]` and every `[OBJECT]`.
   At the end of the file: **`.abl`** (the level script).

Consequences: `[TERRAIN]` must come before the objects (always the first block in the shipped data); the `.lvl`
is read twice (first only `PROJECT_FILE`, then completely); players already exist when the objects are placed.

When a level is loaded from a savegame, the `.wld` is read without its `[OBJECT]` blocks.

## Layout of the `.lvl`

Fixed order (in all 55 files of the shipped data): `LEVELFILE`, `PROJECT_FILE`, `[RESOURCES]`,
`[NEUTRAL]`, then 1-8 `[PLAYER]`. The game searches the keys per line as a substring.

| Key | Format | Meaning | Read by |
|---|---|---|---|
| `LEVELFILE` | `V1.08e` etc. (`V1.00b` ... `V1.09i`) | Version; not evaluated | - |
| `PROJECT_FILE` | `"path"` | Project file (always `Data\Projects\nml-wiese.prj`) | first pass |
| `[RESOURCES]` | block of `CONFIG_GUID` | Default starting resources and season/time of day (`CONFIG_GUID` 0 wood, 1 food, 2 gold, 40 season/time of day from `data/config/levelresources.cfg`) | top-level resources |
| `[NEUTRAL]` | `{ [CONFIGDATA] {...} }` | Configuration of the neutral object types | `[CONFIGDATA]` section parser (1) |
| `[PLAYER]` | block, see below | one player | - |

### `[PLAYER]`

Keys in exactly this order (all 243 players in the shipped data); after `PLAYER_COLOR` the game immediately reads the
four sub-blocks:

| Key | Format | Meaning |
|---|---|---|
| `PLAYER_NAME` | `"text"` | Display name: if the text starts with a number >= 1 other than 29320, it is a text ID (the rest = editor comment, e.g. `"29081 RED"`); otherwise the text itself |
| `PLAYER_PLAYER` | `"SPA"` etc. | Nation: `WID`, `PRI`, `SPA`, `SID`, `ENG`, `PAT` (index from `data/projects/dropdown.ini`, `VALUEGUID 0`) |
| `PLAYER_TYPE` | `0`/`1`/`2` | 0 = human player (campaign player, all multiplayer slots; 108x), 1 = AI opponent (92x), 2 = passive AI/extras (43x, e.g. "Dummy", animals) - meaning derived from the shipped data |
| `PLAYER_COLOR` | `0`...`7` | **Player slot and color**; index for `OBJ_COLOR` of the `.wld`; unique within the level |
| `PLAYER_LINETOOL` | 4 numbers | Ignored by the game (editor; mostly `0 128 128 0`) |
| `[RESOURCES]` | `CONFIG_GUID` blocks | Starting resources of this player: `CONFIG_VALUE "n"` of `CONFIG_GUID` 0/1/2 overrides the default, **regardless of `CONFIG_ENABLE`** (only `CONFIG_GUID` and `CONFIG_VALUE` are read) |
| `[CONFIGDATA]` | `OBJECT_GUID n { CONFIG_GUID ... }` | One configuration entry per object type (project `OBJ_GUID`) (copy of the entry from `default.dat`) with the given property values; empty block = defaults |
| `[UPGRADES]` | `UPGRADE_GUID n { ... }` | Available upgrades (GUIDs from the project file or the upgrade stock of the game) |
| `[PATTERNS]` | `PATTERN_GUID n { ... }` | Always empty in the shipped data |

### `CONFIG_GUID` block (all sections, also `default.dat` and `[OBJECT] [CONFIG]` of the `.wld`)

```
CONFIG_GUID 14            property (PROPERTY GUID from data/config/*.cfg)
{
    CONFIG_LABEL  "Hit Points"     editor only
    CONFIG_ENABLE TRUE            enable (see below)
    CONFIG_VALUE  "0"             value as text (number or dropdown entry)
}
```

`CONFIG_VALUE` is taken only if the preceding `CONFIG_ENABLE` contains `TRUE` **or** the call does not require an
enable. `default.dat` is read with the enable required (only enabled values), the `.lvl` without it (all values;
before that, the entry of the type from `default.dat` is copied).

### `.cmm` (camera paths)

The reader takes lines of the form `Name %s`, `AccTime %d`, `BreakTime %d`, `Duration %d`,
`MoveType %d`, `CameraCount %d` and per camera `CameraName %s`, `CameraFrom %f %f %f`, `CameraAt %f %f %f`,
`CameraRoll %f`, `CameraFOV %f`. All 28 `.cmm` in the shipped data are empty; camera paths of the campaigns are
instead `[CAMERA]` blocks in the `.wld` and are used by the scripts.

## Coordinate system and units

The `.lvl` contains no coordinates (`PLAYER_LINETOOL` is ignored). Start points of the players are
objects with `OBJ_GUID 4200` in the `.wld`; coordinates and axes in [wld.md](wld.md).

## Ignored areas

| Area | Reason |
|---|---|
| `LEVELFILE V...` | Version not evaluated |
| `PLAYER_LINETOOL` | not a key of the `.lvl` reader |
| `CONFIG_LABEL` | Editor display; what is read is `CONFIG_GUID`/`CONFIG_ENABLE`/`CONFIG_VALUE` |
| `CONFIG_ENABLE` in the player `[RESOURCES]` | not queried |

## Validation

A check script parses every `.lvl` and checks the schema including the order, that `PLAYER_COLOR` is 0...7 and
unique, that `PLAYER_PLAYER` is in `dropdown.ini` and that every `OBJECT_GUID` (project object), `UPGRADE_GUID`
(project upgrade) and `CONFIG_GUID` (`PROPERTY` in `data/config/*.cfg`) can be resolved. Result over the corpus:

| Check | Result |
|---|---|
| Levels | 55 of 55 OK |
| Property GUIDs in configurations | 5,899 |
| Object GUIDs in configurations | 37,540 |
| Upgrade GUIDs in configurations | 23,678 (of which removed upgrades: 2914: 7, 3114: 10, 3214: 4, 3314: 1) |
| `.ldg` | 32 |
| Empty `.cmm` | 27 |
| Player types 0 / 1 / 2 | 108 / 92 / 43 |
| Ignored by the game | `LEVELFILE`: 55, `PLAYER_LINETOOL`: 243 |
| Partial files without `.lvl` (not loadable by the game) | 2 |

22 `UPGRADE_GUID` entries in 5 levels (`kamp2-1`, `kamp2-2`, `kamp3-6`, `kamp3-7`, `menü`) refer to
removed upgrades (2914, 3114, 3214, 3314: not in the project file, in `data/gifcon/toolbar.ini` only
as an `#empty` slot). The game creates only a configuration entry for them; the script counts them separately, any
other unknown GUID would be an error.

## Open questions

- Exact meaning of `PLAYER_TYPE` 1/2 in the AI code and why text ID 29320 is not translated
  (presumably a placeholder "no name").
- Loading of the preview images `[n players]-*.tga` in the multiplayer menu.
