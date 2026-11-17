# Object `.cfg` (model assembly)

Rust: `nml_formats::object_cfg::{parse_object_cfg, object_cfg_path}`; as in the game, a `.cfg` without an `OBJECT`
block yields an empty configuration (see "Layout"). Deviation from the original: `parse_object_cfg` compares the
first word of the line (through `nml_formats::text::parse_blocks`) instead of searching for a substring as the original
parser does. In the shipped data all keys are the first word, so the result is the same; at the same time the
mistakes of the substring search (a key name in a value or comment, `ID` matching `UPGRADEID`) and the dependence on
the checking order disappear.

Text file in the bracket grammar from [text.md](text.md) (cp1252, CRLF). It assigns to a model
(`.rdo`, [rdo.md](rdo.md)) its animations (`.rdm`, [rdm.md](rdm.md)), their bounding boxes and
time markers, and sub-objects that run along (weapons, tools, windmill ...).

## Purpose and locations

| Files | Count | Root | Read by the game |
|---|---|---|---|
| `data/r3d/**/<model>.cfg` | 547 | `OBJECT` | **yes** - through the name of the `.rdo` |
| `data/rdm/<animation>.cfg` | 1,054 (+ 8 empty) | `ANIMATION { FILE, SPEED, STARTFRAME, LOOP, INTERPFIRSTLAST }` | no - companion file of the exporter; the values are in the `.rdm` header (`SPEED`, `STARTFRAME`, `INTERPFIRSTLAST` do not occur in `NML.exe`) |
| `data/r3d/**/viewer.cfg` | 5 | `[OBJECTPATH]`, `[ANIMATIONPATH]`, ... | no - configuration of a viewer tool |

The object `.cfg` is **not** found through its `FILE`, but the other way round: after a `.rdo` has been loaded, the
game builds the `.cfg` path from its path (same directory, same base name, extension `.cfg`; a prefix `lod_` in the
file name is removed first, so that LOD models share the `.cfg` of the main model). If the `.cfg` is missing, the
model has no animations. The result of this lookup is ignored when the model is created.

The game also contains an equivalent binary parser for `<name>.dat`, but it selects the `.cfg` (a constant mode
value) and never reads a `.dat`; this is dead code, and there is no `.dat` in the shipped data.

## Layout

The main loop reads lines up to the end of the file; only a line with `OBJECT` opens the object block,
all other lines are skipped, and `ANIMATION` is recognized only inside `OBJECT`. At the end of the file
the parser reports success (an error only if the file cannot be opened). A `.cfg` without
`OBJECT` - in the shipped data only the exporter companion file `data/rdm/kanone_spa_stehen.cfg` next to the
old-format model `data/rdm/kanone_spa_stehen.rdo`, which is referenced nowhere - therefore yields a model without
animations and sub-objects.

The parser works line by line with a substring search in a fixed order; `{`/`}`
are on their own lines. Values are read with `sscanf("%s %d")` (or `%u`, `%f`);
file names are in `"..."` (text between the first and the last `"`) and are lowercased. `.\data\...` with `\` is
common; the VFS does the resolution ([rda.md](rda.md)).

### `OBJECT { ... }`

| Key | Format | Meaning | Target | In the shipped data |
|---|---|---|---|---|
| `FILE` | `"path.rdo"` | Model that the `.cfg` belongs to | **not read** (no `FILE` branch at object level) | 547 |
| `LODENABLED` | `%d` | Levels of detail allowed | stored; evaluation not followed | 546 (always `0`) |
| `CASTSHADOW` | `%d` | Casts a shadow | only written in the constructor and the parsers, **never read** - no effect. The game chooses the kind of shadow from `ShadowQuality` and the classes in `data/gifcon/property.ini`; decals come from `OBJ_SHADOW_FILENAME` of the project file | 546 (always `0`) |
| `UPGRADEID` | `%d` | Upgrade ID | upgrade table | 0 |
| `TEXTUREUPGRADE` | `%d "texture"` | Replacement texture for an upgrade level | texture upgrade entry | 0 |
| `SUBOBJECT { ... }` | block | Sub-object of the object level: drawn only while **no** animation is running, and only with `INDEX` != -1 (e.g. `farm_windrad.rdo`; 168 of the 175 have `INDEX` -1 - the tool inventory of the figures, never visible through the object level) | list of object-level sub-objects | 175 |
| `ANIMATION { ... }` | block | One animation; its index is the order in the file (0, 1, ...) | see below | 1,873 |

### `SUBOBJECT { ... }` (at object level and at animation level)

| Key | Format | Meaning | Default |
|---|---|---|---|
| `FILE` | `"path.rdo"` | Model of the sub-object (with its own `.cfg`) | - |
| `INDEX` | `%d` | Index of the animation **in the `.cfg` of the sub-object** that is played in parallel (e.g. staff of the animal evoker: main animation "laufen" -> staff animation 1 "laufen_stab") | `-1` |
| `START` | `%d` | only copied, **never read** (always 0 in the shipped data) | `0` |
| `LOOP` | `%d` | only copied, **never read** (always 1 in the shipped data); whether the sub-animation loops is decided by its `.rdm` | `1` |

The link is created at the closing `}` if `FILE` could be loaded (model, `INDEX`, `START`, 0, `LOOP`); at animation
level it goes into the list of that animation index.

At run time: for each model instance and sub-model there is **one** sub-instance, shared by all links; while an
animation is running, only the sub-objects of its list are drawn, without an animation only those of the object level
with `INDEX` != -1. At each animation change of the carrier, every sub-instance of the new list starts its `INDEX`
animation in the same mode (crossfaded or hard) and with the start time of the carrier; the world matrix is that of
the carrier. `farm.cfg` lists the windmill at object and animation level - it is the same sub-instance and is drawn
once.

### `ANIMATION { ... }` (in `OBJECT`)

| Key | Format | Meaning | Target | In the shipped data |
|---|---|---|---|---|
| `FILE` | `"path.rdm"` | Animation file | looked up among the loaded animations by path, otherwise loaded, added to the list of loaded animations and attached to the model (only if a global flag is set, otherwise only a counter, see [rdm.md](rdm.md#open-questions)) | 1,873 |
| `GUID` | `%d` | ID of the animation: starting an animation takes the animation as a GUID and converts it to the index (table of 500 slots; without a matching `GUID` the value itself counts as the index, e.g. `ANIMGUID` of the `.pef`, [pef.md](pef.md)) | GUID per animation index | 0 |
| `MAXBOUNDINGBOX` | 6 x `%f` | `min.x min.y min.z max.x max.y max.z` over the animation | `MAXBOUNDINGBOX` per animation index | 1,864 |
| `TIMELABEL { NAME "..." ID %u FRAME %d TIME %u }` | block | Time marker as in the `.rdm` | at most 20 per animation | 0 |
| `SUBOBJECT { ... }` | block | Sub-object only during this animation | see above | 894 |

`MAXBOUNDINGBOX`: for units (skin meshes) the box encloses the animated figure in object space. For buildings with
rigid meshes it is **not** in object space (e.g. `farm.cfg`:
`0 0 -403 ... 780 100.6 239` compared with the object hull `-355 -60 -373 ... 381 492 372`; the values fit
raw vertex coordinates of individual meshes starting from the value 0). The original uses it anyway: the bounding box
computation (hull for picking) takes, while an animation is running, for **skin meshes and meshes with a morph
track** the `MAXBOUNDINGBOX` of the current animation (during the crossfade united with that of the source animation),
for other meshes their hull with the animated mesh matrix - for buildings with morph tracks (e.g. field growth) these
values too. The same hull determines the clip and the receiver tiles of the shadow textures. The reimplementation does
not use it so far (picking through the bounding sphere of the `.rdo`).

## Coordinate system and units

Paths relative to the game directory. Numbers as in [rdo.md](rdo.md) (Y up).

## Ignored areas

- `OBJECT`/`FILE` (see above), all unknown keys (they fall through the search chain).
- Comments are not treated separately; if a comment line contains a key name, it would be read as a key (not the
  case in the shipped data).
- Because of the substring search, the checking order decides (e.g. in `TIMELABEL`: `NAME` before `ID`,
  `FRAME` before `TIME`; `ID` would otherwise also match `UPGRADEID`-like lines). A parser of its own should compare the
  first word of the line ([text.md](text.md)); in the shipped data all keys are the first word, so the result is the
  same.

## Validation

A check script parses all 547 object `.cfg`, resolves `FILE` of the object and of the animations in the VFS (all
present), checks `MAXBOUNDINGBOX` (min <= max in all 1,864) and compares the track names of the `.rdm` with bone and
mesh names of the `.rdo` (30,006 bones with a track, 99 without; 2,210 node tracks with a matching mesh, 166 for
meshes of sub-objects). The `INDEX` assignment was checked by sampling against the `.cfg` of the sub-objects (animal
evoker: `INDEX 0...7` correspond to the 8 animations of the staff `.cfg` in the same order).

## Open questions

- Effect of `LODENABLED` and of the `lod_` prefix in the renderer; there are no `lod_*.rdo` in the shipped data.
- How the game logic uses the upgrade filter of the sub-objects (`UPGRADEID`); there is no `UPGRADEID` in the shipped
  data.
