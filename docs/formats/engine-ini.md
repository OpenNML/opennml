# `Engine.ini` - settings file of the original

How the values are operated in the menus: the options pages and the other menus read and write the keys listed
below. The reimplementation never writes to the installation; it keeps the same values in a file of its own.

## Loading, defaults, writing

- The engine configuration starts with defaults (including `defaultkeys.ini` for the key mapping; the keyboard object
  also loads `keys.ini`) and then reads the file (archive, root block `InitFile`). On success a "read" flag is set.
- On failure the original reads an **old line-based text format** of the same file (`TIMEPERCHAR `,
  `SCREEN_XSIZE` ...); then it writes the file in the new format and reads it again, otherwise it exits the program.
- Missing keys keep the default; a fixed field with a wrong `fixedsize` stays unchanged.
- Writing creates the file (create and write) with the root `InitFile`. It is triggered by `OPTIONS OK`, by the
  destructor (if the "read" flag is set) and by the constructor (new file).

## Format

- A single line without line breaks: `<key>value</>`, blocks `<Name>...</>`.
- int decimal, bool `0`/`1`, float `%f`, string raw, color decimal.
- List: `<listsize>n</>` and `<i%04d>...</>`; fixed field: `<fixedsize>n</>` and `<i%04d>...</>`.
- `NetName`, `NetSession`, `RandomMapSeed`: `wchar_t[40]`, 40 UTF-16 units **unsigned decimal**, the rest 0.
- `Keyboard`: `<m_KeyMap><mapsize>n</>` with `<k%04d>ID</><v%04d>KEY</>` (key as a combination name, e.g.
  `CONTROL H`).
- `SoundManager` (last entry): `m_Output`, `m_DriverID`, `m_IsHardwareOn`, `m_RevertChannels`, `m_Volumes`
  (`fixedsize 3`: music, sounds, speech).

## Keys in write order

Defaults are those of the original. Column M: **O** = options pages, **S** = other menus, **G** = in game,
- = start only.

| # | Key | Type | Default | M |
|---|---|---|---|---|
| 1 | RenderDebugBlock | int | 0 | - |
| 2 | IsMenuLevel | bool | 1 | O |
| 3 | IsIntro | bool | 1 | - |
| 4 | HardwareCursor | bool | 1 | O |
| 5 | DoubleClickTimeout | int | 200 | - |
| 6 | CheckSumEnabled | int | 100 | - |
| 7 | CampaignDifficulty | int | 1 | - |
| 8 | TimePerChar | int | 55 | - |
| 9 | ConsoleKey | int | 1 | - |
| 10 | ActivateAltF4 | int | 1 | - |
| 11 | TextureQuality | int | 0 | O |
| 12 | CompressedTextures | bool | 1 | - |
| 13 | ScreenSaverTime | int | 0 | - |
| 14 | ScreenSaverFile | String | "" | - |
| 15 | IsAutoRecord | int | 0 | - |
| 16 | TerrainSoftwareProcessing | int | 0 | - |
| 17 | RefreshRate | int | 75 | - |
| 18 | Direct3DDriver | int | 0 | - |
| 19 | Direct3DDevice | int | 1 | - |
| 20 | NetHostAdr | String | "" | - |
| 21 | NetIsHost | int | 0 | - |
| 22 | NetProvider | int | 0 | - |
| 23 | NetPort | int | 0 | - |
| 24 | NetClientCount | int | 1 | - |
| 25 | NetAutoStart | int | 0 | - |
| 26 | StopOnCheckSumError | int | 0 | - |
| 27 | TextureArrayPreLoad | list | empty | - |
| 28 | IsShowInfo | int | 0 | - |
| 29 | IsMap | int | 1 | - |
| 30 | IsStatusBar | int | 1 | - |
| 31 | IsToolBar | int | 1 | - |
| 32 | IsLogFileOn | int | 0 | - |
| 33 | ScreenXSize | int | 1024 | O |
| 34 | ScreenYSize | int | 768 | O |
| 35 | ScreenDepth | int | 16 | O |
| 36 | ScreenFormat | int | 0 | - |
| 37 | WaitForKIDebugger | int | 0 | - |
| 38 | ScriptFilename | String | "Console.ini" | - |
| 39 | IntroFilename | String | "" | - |
| 40 | LogoFilename | String | "" | - |
| 41 | LevelFileName | String | "" | - |
| 42 | LanguageTAG | String | "DEU" | - |
| 43 | KIServerPort | int | 5479 | - |
| 44 | Version | float | 0.0 | - |
| 45 | ShowOccluded | int | 0 | O |
| 46 | BackColor | color | 0 | - |
| 47 | AmbientLight | color | 0xFFFFFF | - |
| 48-54 | RenderSpecular, RenderDither, RenderSolid, RenderGouraud, RenderPerspective, RenderSubpixel, RenderFog | bool | 0, 1, 1, 1, 1, 1, 0 | - |
| 55 | MipMapLevel | int | 1 | - |
| 56 | TerrainQuality | int | 0 | O |
| 57 | FogColor | color | 0x10000 | - |
| 58 | FogStart | float | 950.0 | - |
| 59 | FogEnd | float | 1100.0 | - |
| 60 | Gamma | float | 1.0 | O |
| 61 | AmbientSound | int | 1 | - |
| 62 | UnitResponse | int | 1 | O |
| 63 | ShadowQuality | int | 2 | O |
| 64 | ParticlesQuality | int | 2 | O |
| 65 | MouseScroll | int | 10 | O |
| 66 | EnableMouseScroll | int | 1 | O |
| 67 | KeyboardScroll | int | 10 | O |
| 68 | GameSpeed | int | 14 | O, G |
| 69 | EnhancedTooltips | int | 1 | O |
| 70 | MouseSpeed | int | 10 | O |
| 71 | LockCamera | int | 1 | O |
| 72 | EnableZoom | int | 1 | O |
| 73 | Nation | int | 0 | S |
| 74 | Handicap | int | 0 | S |
| 75 | Color | int | 0 | S |
| 76 | NetName | wchar[40] | empty | S |
| 77 | NetSession | wchar[40] | empty | S |
| 78 | RandomMapSeed | wchar[40] | empty | S |
| 79-83 | RandomMapSize, RandomPlayers, RandomTerrain, RandomLandscape, RandomDaytime | int | 0 | S |
| 84 | LevelType | int | 0 | S |
| 85 | SaveReplay | int | 0 | S |
| 86 | LockTeams | int | 0 | S |
| 87 | MapExplored | int | 0 | S |
| 88 | GameType | int | 0 | S |
| 89 | UnitLimit | int | 2 | S |
| 90 | Resourcen | int | 0 | S |
| 91 | TimeLimit | int | 20 | S |
| 92 | InitialAttackStatus | int | 2 | ? |
| 93 | Keyboard | object | `defaultkeys.ini` | O |
| 94 | SoundManager | object | 0, 0, 0, 0, volumes 349/599/499 | O |

## Meaning

- `ScreenFormat` != 0: window mode; `ScreenDepth` 32 selects 32-bit formats, otherwise 16 bit.
- `IsMenuLevel`: 3D menu level behind `START`; `IsIntro`: logos and intro at start.
- `HardwareCursor`: TGA pointer instead of the 3D model.
- `TimePerChar`: message duration `length * TimePerChar + 1500` ms; `DoubleClickTimeout`: double-click window in ms.
- `GameSpeed` 0...19: game time = clock * (n + 10)/20; `MouseScroll`/`KeyboardScroll`: scroll speed `(n + 1) * 0.1`;
  `MouseSpeed`: input speed value `n * 6 + 40`; `EnableMouseScroll`: edge scrolling; `LockCamera`: rotation locked;
  `EnableZoom`: wheel zoom; `EnhancedTooltips`: additional texts; `ShowOccluded` 0/1/2: occluded units.
- `ShadowQuality` 0...3, `ParticlesQuality` 0...2, `TerrainQuality` 0...2, `TextureQuality` 0...2;
  `Gamma`: gamma ramp, slider 0...99 <-> `v * 0.02 + 0.5`.

## Open

- Who writes `InitialAttackStatus`; the S marks come from the menu flow and were not checked again.

## Implementation

Modules in `opennml-formats`: `engine_ini` (archive, `EngineConfig`) and `keys` (`defaultkeys.ini`). The format above is
the reference; this section says what the reimplementation does with it.

- **Archive:** `parse_archive`/`write_archive` (`ArchiveNode::{Value, Block}`). A node is a block if `<Name>` is
  immediately followed by `<` (not `</>`), otherwise a value up to the next `</>` (also empty:
  `<ScreenSaverFile></>`). Whitespace is allowed only before and after the root (an appended line ending does no
  harm); the depth is limited to 64. Unbalanced marks are an error with a message, never a panic.
- **`EngineConfig`:** one field per key of the table (snake_case, `int`/`bool`/`float`/`String`, colors as `i32`),
  `Default` = the defaults without keys, `with_default_keys` sets `key_map` from `defaultkeys.ini`.
  The order of the fields and of writing is that of the table.
- **Reading** (`parse_engine_ini_onto(base, text)`, with `_bytes_onto` for Windows-1252): like the original, the
  reimplementation starts with the defaults as a base (including `defaultkeys.ini`:
  `EngineConfig::default().with_default_keys(...)`) and reads the file onto it. `parse_engine_ini`/
  `parse_engine_ini_bytes` are short forms with `EngineConfig::default()` (empty key mapping). Missing keys and blocks
  keep the value of the base; unknown keys (also in the block `SoundManager`) are ignored with a warning; a value that
  does not fit the type (also `nan`/`inf`, overflow, negative UTF-16 unit), and a fixed field or list with a wrong
  count (`fixedsize`, `listsize`) leave exactly this field unchanged (warning). If the file is unreadable as a whole
  (empty, not closed, garbage, without `InitFile`), the base stays and a warning is issued, instead of the exit of the
  original after the second failure (quirk D-245); the reimplementation does not read the old line format (D-247).
- **Values without range check:** The original takes numbers of the file unchecked; so does the reimplementation
  (`GameSpeed 99`, `Gamma 0.1`, `ScreenXSize 0` are valid for the file, without a warning). Limiting is the job of the
  menus (sliders, comboboxes) and of the window start.
- **Reading the key mapping:** when reading, the original finds the sub-block `m_KeyMap` of the `Keyboard` block, reads
  `mapsize`, then per entry `k%04d`/`v%04d`, and assigns each entry to the map by ID (insert or overwrite). There is
  **no clearing** of the map before the loop; a read error in the middle of the list ends it with a failure result,
  and the entries read up to then are already in the map. The reimplementation does this: entries of the file are
  **inserted** into the mapping of the base (known ID: new key at the old position; new ID: appended at the end, in
  file order); if `Keyboard`/`m_KeyMap`/`mapsize` is missing, the base stays; an error in the middle of the list
  (name `k%04d`/`v%04d` wrong, no number, fewer entries than `mapsize`) keeps the entries read up to then and warns.
  More than `mapsize` entries are not read.
- **Writing** (`write_engine_ini`, with `_bytes`): the order of the table; `float` as `%f` with six decimals, `bool`
  as `0`/`1`; characters without a Windows-1252 equivalent become `?`. A file of the installation yields the same
  bytes when read and written (`installation_engine_ini_round_trips`).
- **Deviation `m_KeyMap` order (D-246):** In the installation, the mapping is in the order of the original's `CMap`
  (e.g. `8, 559, 558, 557, 7, 555 ...`, by bucket). The reimplementation keeps it as a `Vec` and writes back the
  order of the base or of the file (lossless without a base). A mapping newly created from the defaults or read onto
  the defaults carries the order of `defaultkeys.ini`, not that of the `CMap`. Reason: the original reads both orders
  the same way (assignment ID -> key); reproducing the `CMap` arrangement would gain nothing.
- **Settings file of the reimplementation** (`opennml-client/src/settings.rs`): location `--settings <path>` or
  `NML_SETTINGS`, otherwise `%LOCALAPPDATA%\OpenNML\Engine.ini` (shared helper `opennml_ui::atlas_gen::user_path_from`
  with the cache of the atlases); a location inside the installation is rejected (`settings_path_for`, checked with
  `is_inside`). `load_settings` reads like the original's configuration constructor onto the defaults including
  `defaultkeys.ini`; if the file is missing, these apply (the `Engine.ini` of the installation is not adopted);
  warnings go to the log, and an unreadable file stays until the next save. `save_settings` creates the folder,
  writes `<file>.tmp` (cp1252) and renames it. Writing happens at `OPTIONS OK` (`UiEffect::WriteSettings`) and at
  program end (`MenuHost::shutdown`, the original's destructor, quirk O-32).
- **Check:** Starting from the `Engine.ini` of the installation with the values of the reference runs, the options
  path with a new resolution changes only `ScreenXSize`/`ScreenYSize` when writing; all other values come back the
  same, the mapping with the same 460 pairs in the order of `defaultkeys.ini` (deviation above). After `RESET`,
  exactly the mapping of `defaultkeys.ini` is in the file.
- **`defaultkeys.ini`:** `parse_default_keys` returns `(ID, KEY)` in file order. Comments (`#`), empty lines, lines
  without `=` and without a number before the `=` are dropped; `0=` (empty value, heading) is a valid entry and stays.
  The file of the installation has 460 unique IDs, the same as `m_KeyMap` of the `Engine.ini`.
- **Tests:** module tests in `engine_ini.rs` and `keys.rs`; corpus tests in `tests/corpus_engine_ini.rs`
  (`installation_engine_ini_round_trips`, `default_keys_parse`,
  `installation_engine_ini_onto_the_default_keys_keeps_the_mapping`).

## Own keys

The settings file of the reimplementation (never the installation) knows keys that the original does not have. They
are in `EngineConfig` after the fields of the original, are read like the others and **written only if their value
differs from the default** - then behind the block `SoundManager`. A file of the installation therefore still yields
the same bytes when read and written (`installation_engine_ini_round_trips`).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `NetTurnRaster` | int | 0 | Turn raster level for a match created in the menu (`LAN3`); `LAN3` has no choice field for it, so set it by hand: 0 "Original" (`T` 8/`V` 4), 1 "LAN" (4/2), 2 "LAN fast" (2/1). A value outside 0 ... 2 counts as 0 when reading (warning). |

Tests: `engine_ini::tests::engine_ini_keeps_net_turn_raster`.
