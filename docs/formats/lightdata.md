# Light presets `lightdata.ini`

Rust: `opennml_formats::lightdata` (`parse_lightdata`, `find_preset`).

Text file with named light presets. Each preset contains a **terrain light**
(corresponds to `[TERRAIN] LIGHT_*` of the `.wld`, precomputed into the vertex colors, see [hlm.md](hlm.md)) and an
**object light** (corresponds to `[LIGHT]` of the `.wld`, D3D light 0, see [wld.md](wld.md)). During the game,
scripts switch to a preset by name (the action "set light ...").

## Purpose and locations

- Exactly one file: `data/projects/lightdata.ini`, 363 lines, cp1252, CRLF.
- 32 presets, all names different (also without regard to case).
- Loaded **once at program start** in the constructor of the game engine; changes to the file take effect only after a
  restart.

## Reading

The file is read line by line. For each `LIGHT_NAME` a record is created; at `[OBJECT_LIGHT]` it is entered into a
table under the lowercased name. A preset is applied by name at run time.

## Layout

Line format, no bracket grammar. Per preset (example from the shipped data):

```
[LIGHT]
LIGHT_NAME "Sommer Tag"
[TERRAIN_LIGHT]
LIGHT_POSITION 800.000000 400.000000 -1300.000000
LIGHT_COLOR 200 200 200
LIGHT_INTENSITY 20
[OBJECT_LIGHT]
LIGHT_POSITION 800.000000 400.000000 -1300.000000
LIGHT_COLOR 255 241 193
LIGHT_INTENSITY 40

```

| Line | Recognition | Effect |
|---|---|---|
| `LIGHT_NAME "Name"` | the first 10 characters = `LIGHT_NAME` (exact, case matters) | Name = line from character 12 on without the last character (so it requires exactly `LIGHT_NAME "` at the front and `"` at the back); new record |
| `[TERRAIN_LIGHT]` | whole line equal | The following block -> terrain light of the record |
| `[OBJECT_LIGHT]` | whole line equal | The following block -> object light of the record, then the name is lowercased and the record is entered into the table |
| `[LIGHT]`, empty lines, everything else | - | ignored |

Block (reads up to `LIGHT_INTENSITY`; other lines in between are ignored):

| Key | Recognition | Format | Field |
|---|---|---|---|
| `LIGHT_POSITION` | first 14 characters | `%f %f %f` | Direction **to** the light (x, y, z), normalized when applied |
| `LIGHT_COLOR` | first 11 characters | `%i %i %i` (R G B) | Stored as COLORREF `0x00BBGGRR` (only the low byte of each value) |
| `LIGHT_INTENSITY` | first 15 characters | `%i` | Intensity `I`; ends the block |

### Record

A record holds six fields: terrain light `LIGHT_POSITION` (`f32[3]`), `LIGHT_COLOR` (`u32`, `0x00BBGGRR`) and
`LIGHT_INTENSITY` (`i32`), then the same three for the object light. When a preset is applied, they are copied into
the terrain light and the object light of the engine.

These are the same fields that `[TERRAIN] LIGHT_*` and `[LIGHT]` of the `.wld` fill when loading
([wld.md](wld.md)); the formulas (terrain light `f = n*L^ + 1 - 0.01*I` per channel times `LIGHT_COLOR`,
object light ambient `1 - 0.01*I`, diffuse `LIGHT_COLOR/255`) are in [hlm.md](hlm.md).

## Shipped data

Terrain and object light per preset (file order). Almost all directions are
`(800, 400, -1300)` (terrain) or `(100, 400, -1300)` (objects).

| Name | Terrain: direction, color, `I` | Objects: direction, color, `I` |
|---|---|---|
| `Sommer Tag` | (800, 400, -1300), (200, 200, 200), 20 | (800, 400, -1300), (255, 241, 193), 40 |
| `Sommer Nacht` | (800, 400, -1300), (82, 90, 93), 15 | (800, 400, -1300), (32, 88, 145), 65 |
| `Winter Tag` | (0, 1, 0), (255, 255, 255), 40 | (100, 400, -1300), (255, 255, 255), 70 |
| `Winter Nacht` | (800, 400, -1300), (255, 255, 255), 40 | (100, 400, -1300), (255, 255, 255), 40 |
| `6` ... `24` | (800, 400, -1300), colors from orange (`6`: 255 135 55) through white (`12`: 254 237 222) to red-violet and blue (`24`: 0 64 128), `I` 20 ... 40 | (100, 400, -1300), correspondingly (`6`: 255 122 34, `I` 70 ... `24`: 0 85 170, `I` 80) |
| `1` ... `5` | (800, 400, -1300), night blue (`1`: 36 74 118, `I` 38) to orange (`5`: 219 125 65, `I` 24) | (100, 400, -1300), (`1`: 26 89 156, `I` 78 ... `5`: 219 117 53, `I` 71) |
| `wid 2 Schnee Nacht` | (800, 500, -1300), (120, 160, 254), 50 | (800, 400, -1300), (62, 109, 198), 70 |
| `PAt 4 Regen` | (800, 800, -1300), (142, 147, 179), 45 | (800, 400, -1300), (182, 171, 139), 69 |
| `Spa 5 Nacht` | (800, 400, -1300), (69, 135, 233), 70 | (800, 600, -1300), (0, 128, 255), 75 |
| `Spa 5 Morgen` | (800, 800, -1300), (247, 148, 49), 45 | (800, 400, -1300), (255, 128, 0), 56 |

The numbers `1` ... `24` form a course of the day (night -> morning -> noon -> evening -> night); the program has no
automatic day cycle, though - only scripts choose presets. Complete values: the file itself.

Used by scripts (action 171, not deactivated): `Sommer Tag`, `Sommer Nacht`, `2`, `5`, `6`, `9`,
`10`, `18`, `20`, `22`, `23`, `wid 2 Schnee Nacht`, `PAt 4 Regen`, `Spa 5 Nacht`, `Spa 5 Morgen` - all
present. `Winter Tag`, `Winter Nacht` and the other numbers are not used.

## Related files (not read by this parser)

| File | Relation |
|---|---|
| `.wld` `[TERRAIN] LIGHT_*`, `[LIGHT]` | Starting light of a level as **numeric values**, not a preset name. Terrain and object light equal to a preset: only 5 levels (`kamp1-1`, `kamp1-2`, `kamp0-1`, `kamp0-2`, `[3players]-canonballcanyon`, each `Sommer Tag`); only the terrain light: `kamp2-6`; only the object light: 8 more (`Sommer Tag`, `kamp1-5`: `Spa 5 Nacht`); all others have values of their own |
| `data/config/levelresources.cfg` `PROPERTY GUID 40` "Jahres-/ Tageszeit" (dropdown `Sommer Tag`, `Sommer Nacht`, `Winter Tag`, `Winter Nacht`) and `data/projects/dropdown.ini` `VALUEGUID 40` (values 0-3) | `CONFIG_GUID 40` of the `.lvl` ([lvl.md](lvl.md)). The game does **not** evaluate the value as light but as an upgrade set for all players |
| `data/randommapscripts/light.inc` | Light of the random maps (`TerrainLight`, `ObjectLight`, `ShadowLight` in bracket blocks, `#ifdef NIGHT`); parser of its own |

## Ignored areas

| Area | Reason |
|---|---|
| `[LIGHT]` lines, empty lines | not recognized by the reader |
| Lines between `[..._LIGHT]` and `LIGHT_INTENSITY` with another key | ignored in the block reader |

## Validation

An independent parser (session script, not in the repository): 32 presets, exactly one `[TERRAIN_LIGHT]` and
`[OBJECT_LIGHT]` each with all three keys, no duplicate names, all names requested by the `.abl` scripts present.

**Notes for a Rust parser** (behavior of the original with broken files, not present in the shipped data): if
`[OBJECT_LIGHT]` is missing, the preset is never entered. If the file ends before `LIGHT_INTENSITY`, the original
copies uninitialized stack values. Leading spaces prevent recognition. A Rust parser should discard such entries and
document that.

## Open questions

None.
