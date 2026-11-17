# Remaining control files in `data/gifcon/`

Line-oriented text files (cp1252, CRLF, **no** bracket grammar) that were needed for the economy and the user interface
or were undocumented before: `guidgroups.ini`, `defaultobjects.ini`, `gametypevictory.ini` /
`gametypedefeat.ini`, `priorities.ini` (short form), `randommaptypes.ini`, the economy keys of
`toolbar.ini` and an overview of the remaining files. Upgrade files: [gifcon-upgrades.md](gifcon-upgrades.md),
resources: [resourcenconfig.md](resourcenconfig.md).

Corrections that apply throughout:

- Start objects do not rotate with the start point (the transformation is a vector addition); only buildings (corner
  of the hull) and flags (free tile) snap into place. Reader: [setup-readers.md](setup-readers.md) section 2.
- `toolbar.ini`: 520-528 "trade target" are dynamic menu entries (not in bars); 490-493 appear only as `MAKE_MSG` in
  the six trading buildings.

## Purpose and locations

| File | Lines | Content |
|---|---|---|
| `guidgroups.ini` | 23 | GUID groups for counts |
| `defaultobjects.ini` | 401 | Start objects per game type and nation, supply |
| `gametypevictory.ini`, `gametypedefeat.ini` | 289 / 400 | Victory/defeat conditions (read when a level or a random map level is created) |
| `priorities.ini` | 216 | Target priorities (common "MM" parser with `targets.ini`, `flowcontroltragets.ini`) |
| `randommaptypes.ini` | 97 | Random map types, game modes, nations |
| `toolbar.ini` | 8105 | Buttons per object, build menus, prerequisites |
| `statusbar.ini` | 18 | Status bar: height, colors, position of the resource displays |
| `toolbar.cfg` | 211 | Position of the command bar: sheets, background, button positions, colors |
| `icons.ini` | 2255 | Portrait and icon sheets, GUID -> tile |

## Reading

All readers open the file through the VFS and read line by line. Keys are compared with a prefix or with the whole
line; comments (`#`) and empty lines fall through all comparisons.

## Layout

### `guidgroups.ini`

```
GROUP 1114          group GUID (prefix "GROUP ", number after it); clears the list
GUID 1113           member (prefix "GUID ")
/GROUP              whole line: Map[group] = {count, GUIDs...} (nothing for an empty list)
```

Effect: for a group GUID, the count is the sum of the members (recursive, groups may contain groups); used by
scripts (`GetGuidCount`, `GetBodyCount`), the upgrade rules (`upgrades.ini`) and production (e.g. GUID 3374). Shipped
data: 1114 -> 1113 (missionary on horseback counts as a missionary), 3411 -> 3412 + 3413 (buffalo). A group that
contains itself (directly or indirectly) leads to endless recursion.

### `defaultobjects.ini`

```
GAMETYPE 0                  game type 0 ... 5 (0 normal, 1 kill hero, 2 flags, 3 King of the Hill,
                            4/5 railroad); prefix, number
NATION SPA                  nation via the choice list (dropdown.ini VALUEGUID 0: WID 0, PRI 1, SPA 2,
                            SID 3, ENG 4, PAT 5)
PLACEOBJECT 1000 0 0        GUID, dx, dz in tiles relative to the start point (scanf "%d %d %d")
/NATION                     whole line
/GAMETYPE                   whole line
SUPPLY n ... /SUPPLY        supply list n; a PLACEOBJECT inside goes into the supply list
```

The lists are stored per (nation + game type * 6). At the start point object 4200 (only for players whose control
type is not 0), the game creates each object at `start + (dx * tile width, 0, dz * tile width)` (transformed by the
rotation of the start point), snaps buildings into place, creates them **finished** and sends a message to everything
except flags (4300). Shipped data: per game type and nation the HQ at (0, 0) and four workers; game type 1 swaps a
worker for the hero 2720 ... 2725, game type 2 adds the flag 4300 at (3, 0). Supply objects are placed by a separate
step.

Error behavior: `PLACEOBJECT` outside a `NATION` block aborts reading (result 0); a `GAMETYPE`/`NATION` block that is
still open at the end of the file does the same.

### `gametypevictory.ini` / `gametypedefeat.ini`

Reader in the reimplementation: `nml-formats/src/gametype_rules.rs` (`parse_gametype_rules`, path constants
`GAMETYPE_VICTORY_PATH`/`GAMETYPE_DEFEAT_PATH`); it reads like the original.

```
GAMETYPE n                 game type 0 ... 6 (table 7 x 6; 0 ... 5 in the shipped data)
NATION XXX                 nation (dropdown.ini, list 0: WID 0, PRI 1, SPA 2, SID 3, ENG 4, PAT 5)
OR | AND                   root node (only without "!" at the top level); sub-blocks and leaves up to /OR or /AND
  <leaf> [numbers]         one per line; keyword = start of the line, numbers separated by spaces
  !OR | !AND               sub-block with a flag: negates the results of its children (never used in the shipped data)
/OR | /AND                 closes the innermost block (either word closes any block)
/NATION                    table[game type * 6 + nation] = the root read last
/GAMETYPE                  no effect
```

Conditions in the order of the leaf type 0 ... 8 (parameter count in parentheses): `MATCHLOWERTHAN` (2),
`MATCHGREATERTHAN` (2), `RESOURCELOWERTHAN` (2), `RESOURCEGREATERTHAN` (2), `CHECKGUIDTRIGGER` (2),
`CHECKPLAYERID` (1), `CHECKTIMER` (1), `CHECKENEMY` (0), `CHECKFLAGOWNER` (0) (target numbers from
`flowcontroltragets.ini`, e.g. 8 = building, 2 = units, 2720 ... = hero). The original does not read missing numbers
as 0 but repeats the previous one (`MATCHLOWERTHAN 8` = `(8, 8)`). A `/OR` or `/AND` at the top level ends reading
the file; if the file ends inside an open block, the original reports a load error (the reimplementation returns the
entries read up to then). Evaluation: a slot without a root or outside the table is false.

### `priorities.ini` (short form)

```
BEGIN
MY_MM            filter for the own unit ...
MM_MODE MM_ANDALL | MM_HL_PLAYERLIST | MM_CHECKHANDLETYPE[_ANDMODE] | MM_CHECKPROPERTY | MM_CHECKNOTSTATUS
MM_MODE2 MM2_...   MM_MATCHBITS MID_...   MM_PROPID PROP_...   MM_STATUS ...   MM_IS_ATTACKING_WHO ME
MM_END
TARGET_MM        ... filter for the target
MM_END
END              one priority level; order = priority (highest first)
```

Symbol names (`MID_PROP_...`, `PROP_...`) are resolved through `targetdefines.ini`. Content and evaluation belong to
combat.

### `randommaptypes.ini`

Path `data/gifcon/randommaptypes.ini`; reader in the reimplementation: `nml-formats/src/mp_levels.rs`
(`parse_random_map_types`). It ignores lines with `#` and empty lines. The line starts (with space, exact, without
trimming) are `GNAME `, `MNAME `, `LNAME `, `DNAME `, `NNAME ` (name = text ID) and `GTYPE `, `MSCRIPT `, `LTYPE `,
`DTYPE `, `NTYPE ` (word or script path). All `*NAME` lines write the **same** intermediate variable; the entry
is created only by the following `*TYPE`/`MSCRIPT` line with the name that is valid at that moment (pairing as usual:
name, then type).

| List | Key | Shipped data (file order) | Use |
|---|---|---|---|
| Map types | `MNAME` / `MSCRIPT data/randommapscripts/*.rnd` | 15470 ... 15475 (continental, flatland, Highland, Islands, Lakes, Rivers) | List `terrain` in `RANDOMMAP` |
| Landscapes | `LNAME` / `LTYPE` | 15490 ... 15494: CARIBBEAN, PRAIRIE, DESERT, WINTER, MEADOW | List `landscape` |
| Times of day | `DNAME` / `DTYPE` | 15500 DAY, 15501 NIGHT | List `daytime` |
| Game modes | `GNAME` / `GTYPE` | 15540 ... 15545: NORMAL, NORMAL, NORMAL, KING_OF_THE_HILL, RAILROAD1, RAILROAD2 | List `gametype`; `GTYPE` filters the map list (`TYPE`) |
| Nations | `NNAME` / `NTYPE` | 15120 ... 15125: WID, PRI, SPA, SID, ENG, PAT | Nation number = index of `PLAYER_PLAYER` in `NTYPE` |

`GTYPE` is the word that `TYPE` of the map list must match. The reimplementation reads `MSCRIPT`, `LTYPE` and `DTYPE`
only as the trigger for the entry (it does not evaluate the script path, landscape and time-of-day word there).
Random maps are free: the page `RANDOMMAP` puts the indices into `MSCRIPT`, `LTYPE`, `DTYPE` and `GTYPE` into the map
description (`RandomMapSpec`), and the generator reads script, landscape and time of day from it.

### `levels%04d.ini` (multiplayer map list)

Files `data/levels/multiplayer/levels0000.ini`, `levels0001.ini`, ... ascending up to the **first missing one**
(at most `levels9999.ini`); no directory scan, no sorting.
Reader in the reimplementation: `nml-formats/src/mp_levels.rs` (`load_map_list`, `parse_level_list` per file).

```
NAME 15800            text ID of the display name (number)
TYPE NORMAL           game mode: NORMAL | KING_OF_THE_HILL | RAILROAD1 | RAILROAD2
PATH data/levels/multiplayer/[2Players]-BattleBeach.lvl
```

Line starts `NAME `, `TYPE `, `PATH ` (with space, exact, without trimming; line reading as in [ldg.md](ldg.md)).
**Only the `PATH` line appends an entry**, with the name and type that are valid at that moment (quirk O-185):
`NAME` and `TYPE` stay valid until they are set again; two `PATH` lines give two entries; a lone `NAME` creates
nothing. The installation has only `levels0000.ini` with **20 entries** (`NAME` 15800 ... 15819: 8 x `NORMAL`,
3 x `KING_OF_THE_HILL`, 4 x `RAILROAD1`, 5 x `RAILROAD2`) and at the end of the file a lone `NAME 15800` without
`TYPE`/`PATH` - this does **not** create a twenty-first entry. The original never resets the name (not even from file
to file), `TYPE` and `PATH` start empty in each file; the reimplementation starts each file with name 0 (differing
only for a file that begins with `PATH` without `NAME`).

### `toolbar.ini` (economically relevant keys)

```
GUID 1000                object/menu GUID whose buttons follow (ENDGUID closes)
BUTTONID 0               slot 0 ... 13
BUTTONMSG 1100           content of the button (GUID)
MAKE_MSG                 kind: train (MAKE), build (BUILD), research (UPGRADE), command (DO/TARGET...)
REQUIRED_GUID 1000       prerequisite (several = AND)
```

Shipped data: 89 `BUILD_MSG`, 100 `MAKE_MSG`, 169 `UPGRADE_MSG`, 502 `REQUIRED_GUID`; further flags
`GROUP_AND_MSG`, `ONLYONCE_MSG` (e.g. HQ only once), `MAKEONLY_ONCE_MSG`, `COUNTERUPGRADE_MSG`,
`SELECTHEADHUNTER_MSG`, `TARGET_MSG`, `TARGET_DO_MSG`, `DO_MSG`; known to the code but unused in the shipped data:
`REQUIRED_GUID_NATION`, `SHORTCUT_MSG`, `DIRECT_ACCEPT`. GUID 20 ... 32 are displays (HP, income, mana,
load, living space ...) without buttons. Only the user interface checks the prerequisites: placing a building and
queueing production do **not** evaluate them. For the economy, `toolbar.ini` is the only source of the technology
tree (who may build/train/research what).

Trade: 520-528 "trade target" are dynamic menu entries (not in bars); 490-493 appear only as `MAKE_MSG` in the six
trading buildings.

### Remaining files

| File | Content |
|---|---|
| `bindings2.ini`, `defaultkeys.ini`, `keys.ini`, `keymap.ini` | Key mapping |
| `bindings.ini`, `keymap2.ini`, `campaign2.ini`, `campaign3.ini`, `default2.ldg`, `default3.ldg` | Older versions/variants (no program file names them; possibly referenced from other files) |
| `camera.ini`, `camerasetup.ini`, `cinemascope.ini` | Camera, cinemascope bars |
| `campaign.ini` | Campaign flow |
| `icons.ini`, `toolbar.cfg`, `statusbar.ini`, `portraits.ini`, `portraitframes.ini`, `mousepointer.ini`, `messages.ini`, `infoframe.bkg`, `editfield.bkg` | User interface |
| `musicconfig.ini` | Music ([sounddatabase.md](sounddatabase.md)) |
| `property.ini`, `translate.ini` | Modules, tab breakdown |
| `targets.ini`, `targetdefines.ini`, `flowcontroltragets.ini` | Filters/targets |
| `default.ldg`, `multiplayer.ldg`, `randommap.ldg` | Loading screen ([ldg.md](ldg.md)) |

### `mousepointer.ini`

Blocks `MOUSEPOINTERID n` ... `/MOUSEPOINTERID` (twelve pointers 0-11). Per line, untrimmed, in this checking order:
`MOUSEPOINTERID ` (number after the prefix), `NORMAL `, `RED `, `GREEN ` (the rest is a file name that is appended to
the list of the edition), `OFFSET ` (`sscanf("%d %d")` -> hotspot). End lines and `#` comments match nothing.
Quirks (O-115): lines before the first number go to an uninitialized number, `OFFSET` with only one number keeps the
second value of the previous `OFFSET` line, the number is not limited. Shipped data: one file per pointer per
edition; pointer 7 "Normal" = `data/pointer/pointer-normal.tga`, `OFFSET 2 2`.

Reimplementation: `nml_formats::gifcon::parse_mouse_pointers` (map number -> `MousePointer { normal, red, green,
offset }`); lines before the first number are ignored, the hotspot starts at 0. The menu host (`nml-client`
`menu_host.rs`) draws pointer 7 with the first `NORMAL` file.

### `statusbar.ini`

Path `data/gifcon/statusbar.ini`; 352 bytes in `NML0.RDA` in the shipped data. Lines untrimmed, plain prefix
comparisons (case matters; the separator is cut off with the key, so a prefix like `STATUSSIZEXYZ` also matches).
Checking order and evaluation:

| Key | Format | Field |
|---|---|---|
| `STATUSSIZE n` | number | Height (shipped data 26) |
| `ICONSIZE w h` | `%d %d` | Icon size (32 x 32); must come before `ICONFILE` because the sheet is loaded with it |
| `ICONFILE path` | rest of the line | Icon sheet (`data/icons/resourcen_icons.tga`) |
| `RESOURCE i ix iy tx ty` | `%d %d %d %d %d` | appends an entry to the list of resource displays |
| `PLAYERNAME ix iy tx ty` | `%d %d %d %d` | entry with index -1 (without icon) |
| `STATUSCOLOR b g r` | `%d %d %d` | `0x80RRGGBB` |
| `STATUSFONTCOLOR b g r` | `%d %d %d` | `0xFFRRGGBB` |

The colors are in **b g r** order (file comment "Aussehen b,g,r"): the **third** value becomes the red byte, the first
the blue byte, each enters only with `& 0xFF`, and the alpha byte is fixed (`0x80` or `0xFF`). `STATUSCOLOR 2 22 35`
gives `0x80231602`, `STATUSCOLOR 10 20 30` gives `0x801E140A`, `STATUSFONTCOLOR 300 1 2` gives `0xFF02012C`.
Duplicate lines: for the single values the last one wins, `RESOURCE`/`PLAYERNAME` append. Shipped content: height 26,
icons 32 x 32, four `RESOURCE` lines 0...3 (icon at x = 1/120/240/360, text 32 or 33 pixels behind it at y = 5) and
`PLAYERNAME 480 1 480 5`.

**Deviation (O-251):** The original has no defaults. Nothing is zeroed, and the return value of `sscanf` is never
checked: if a line or a number is missing, heap or stack content remains; the font `StatusFont` is loaded only if the
file could be opened. The reimplementation takes 0 (or empty) without the line and 0 for missing numbers of a line,
because undetermined values cannot be reproduced; the shipped data has all lines, so this has no effect.
Reimplementation: `nml_formats::statusbar::{parse_statusbar, StatusBarConfig, StatusEntry}`.
### `toolbar.cfg`

Path `data/gifcon/toolbar.cfg`; 3191 bytes in `NML0.RDA`. Prefix comparisons as above, in this order:

| Line | Evaluation |
|---|---|
| `ICONS w h file` | number, search for the first space + the rest, number, again space + the rest: the rest is the file. Without a space the whole rest is taken. The sheet (tile size w x h) is appended to the list of sheets; its number is the order of the lines |
| `TAG name` | the rest is the name; a new object becomes the current block |
| `/TAG` (whole line) | enters the current block into the map under the name read last; a later block of the same name replaces the entry |
| `ICON x y sheet tile` | `%d %d %d %d` -> entry `{x, y, sheet, tile}` in the list of the block; `sheet` is a **number** (not a name, no range check) |
| `POS x y` | `%d %d` -> entry `{x, y, 0, 0}` in the same list |
| `COLOR AARRGGBB` | `%X` -> entry `{0, 0, color, 0}` in the same list |

In the original, all three kinds of lines end up in **one** list per block; a consumer reads it with a fixed index.
The shipped data never mixes them. The reimplementation separates them by kind (`ToolbarTag { icons, positions,
colors }`). `ICON`/`POS`/`COLOR` before the first `TAG` write through an invalid pointer in the original; the
reimplementation ignores them. A block without `/TAG` is lost. Missing numbers: see O-251.

Shipped data: 17 `ICONS` (6 x 128 x 128 `status_a_00 ... 05`, 11 x 64 x 64 `status_b_01 ... 11`) and nine blocks:
`TOOLBAR` (17 `ICON`, each from its sheet, tile 0), `NORMALBUTTONS` (14 `POS`: column `245 + 53 * (k mod 7)`, y 17 or
71), `STATUSBUTTONS` (5), `FORMATIONBUTTONS` (9), `SELECTIONBUTTONS` (30), `PORTRAITPOSITION` (11),
`BUTTONNORMALCOLORS` (10 `COLOR`), `HEROBUTTONS` and `HEADHUNTERBUTTONS` (5 each).
Reimplementation: `nml_formats::toolbar_cfg::{parse_toolbar_cfg, ToolbarConfig, ToolbarTag, ToolbarIcon}`.

### `icons.ini`

Path `data/gifcon/icons.ini`; 37,890 bytes in `NML0.RDA`. Keys **without** a space, prefix comparison in this order:

| Line | Evaluation |
|---|---|
| `PORTRAIT file` | Portrait sheet, tile 80 x 80 (`0x50`), appended to the list of portrait sheets |
| `ICONS file` | Icon sheet, tile 50 x 50 (`0x32`), appended to the list of icon sheets |
| `GUID g pb p ib i` | `%d %d %d %d %d` -> map (key `g`) to the four values `{pb, p, ib, i}`: portrait sheet, portrait, icon sheet, icon; a later line with the same GUID replaces the values |

Comment lines (`#`, also `[GUID] ...`) match nothing. Shipped data: 6 `PORTRAIT`, 13 `ICONS`, 743 `GUID` lines
without a duplicate GUID; GUID 500 ... 505 are the nation icons.
Reimplementation: `nml_formats::icons_ini::{parse_icons_ini, IconsIni, GuidIcons}`.

### `messages.ini`: file order of the kinds

Each `MSGTAG` gets as its index a counter that rises by 1 at `/MSGTAG`; the message bar keeps one queue per kind under
this index. In the shipped data: 0 `ECONOMY` (position 0/400, `E000FF00`), 1 `MILITARY` (0/500, `E0FF0000`),
2 `OTHER` (0/600, `E00000FF`), font `InfoMessages` each. Reimplementation: `MessageTag::index` and
`Messages::tags_in_order`. Deviation O-249: a duplicate kind name replaces the values but keeps the index of the first
block (the original would assign a new index without a queue); if `POSITION`/`COLOR` are missing, (0, 0) and 0 apply.

## Coordinate system and units

`defaultobjects.ini`: whole tiles relative to the start point (before the rotation). Otherwise no coordinates.

## Ignored areas

Comment and empty lines; in `defaultobjects.ini` lines that match no key.

## Validation

None of its own. Proposed checks: in `defaultobjects.ini` at least one HQ per game type x nation; `guidgroups.ini`
without cycles; in `toolbar.ini` every `BUTTONMSG`/`REQUIRED_GUID` present in the project file or the upgrade list.

## Open questions

- Parameters of `RESOURCEGREATERTHAN`/`RESOURCELOWERTHAN` (resource index, threshold).
- Whether a control type of a player != 0 means the human or the AI players (start objects).
