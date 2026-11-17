# Upgrade files in `data/gifcon/`: `Upgrades.ini`, `tempupgrades.ini`, `bonus.ini`, `heroes.ini`

Four line-oriented text files (cp1252, CRLF, **no** bracket grammar) that the game reads when it builds the
game world. Effect in the simulation: upgrades and variants, and combat (bonuses).

Corrections that apply throughout:

- Term >= 1000: the GUID is in the **list of applied upgrades** of the player, not in the object counter. Reader:
  [setup-readers.md](setup-readers.md) section 3.
- Starting a temporary upgrade begins an **influence**; it is executed through a separate execution step.
- `UPGRADETIME`: own duration **in ms** (raw), otherwise config 13 in s.
- The tick of 250 ms applies only to auras; the sequence per object tick is a fixed 100 ms.
- `heroes.ini`: a list, untrimmed, duplicates allowed; 14 campaign heroes, 6 multiplayer heroes, 18 elites.

## Purpose and locations

| File | Lines | Content |
|---|---|---|
| `data/gifcon/upgrades.ini` (in the code `data\gifcon\Upgrades.ini`) | 129 | Rules for the variant bits per object |
| `data/gifcon/tempupgrades.ini` | 123 | Temporary upgrades (effects of abilities) |
| `data/gifcon/bonus.ini` | 25 | Attack bonuses, rider bonus |
| `data/gifcon/heroes.ini` | 70 | List of hero GUIDs |

## Loading

- `upgrades.ini`: the `BIT` lines go into a rule table; the evaluation per object yields a bit mask.
- `tempupgrades.ini`: the records go into a map of temporary upgrades.
- `bonus.ini`: an attack bonus table and a rider bonus table.
- `heroes.ini`: the list of heroes.

## Layout

### `upgrades.ini`

```
# comment                   only if '#' is the first character
BIT 4 = 1000 2000 -3000     bit 4 applies if 1000 AND 2000 are present AND 3000 NOT
BIT 4 = 5000                further line: OR
BIT 0 = 1                   constant: always set
```

Reading: the line is lowercased and must start with `bit`; the rest up to `=` is trimmed -> bit number (`atol`,
must be >= 0); after `=` values separated by spaces/tabs, each read with `atol`.

| Value | Meaning in the evaluation |
|---|---|
| >= 1000 | The GUID is in the list of applied upgrades of the player, not in the GUID counter |
| 2 ... 999 | "Target" check on the object itself (filter from `targets.ini`), e.g. 485 unarmed, 486 armed, 487 |
| negative | NOT of the absolute value |
| 0 | false |
| 1 | true - **sets the AND result back to true**, even after a preceding false term |

A rule without values always sets its bit. The result (32 bits) selects the active variant of each module through the
`PROPID n m` masks in `property.ini`.

### `tempupgrades.ini`

```
TEMPUPGRADE 4451            start, GUID (%d); flags = 0
TEMPHANDLELISTUPGRADE       whole line; flags |= 0xA0 (all own units of the player list)
EFFECT 0 data/effects/units_bauzeit.pef      flags |= 0x41; height (%d), then path
MSGSTART 601 1 300          flags |= 0x02; message %X, then up to 9 parameters %d
MSGEND 601 1 100            flags |= 0x04; like MSGSTART
END                         whole line; store the record
```

| Key | Comparison | Flag | Meaning (according to the file header and the code) |
|---|---|---|---|
| `TEMPUPGRADE n` | prefix of 12 characters | - | new record |
| `TAKEUPGRADEVALUES` | whole line | `0x08` | The percentage values of the upgrade (config 114 ... 133) are applied to the units |
| `TEMPRADIUSUPGRADE` | whole line | `0x90` | Units around the executor |
| `TEMPHANDLELISTUPGRADE` | whole line | `0xA0` | All own units to which the upgrade is assigned |
| `UPGRADETEMPORARY` | whole line | `0x80` | (unused in the shipped data; the same bit is contained in `TEMPRADIUSUPGRADE`/`TEMPHANDLELISTUPGRADE`) |
| `ENEMYUPGRADE` | whole line | `0x200` | Acts on opponents (only with radius/influence); execution then uses the target selection `0x400` instead of `0x800` |
| `UPGRADETIME n` | prefix of 12 characters | `0x100` | Own duration **in ms** (raw); otherwise from the upgrade (config 13, in s). The file header names `TEMPUPGRADETIME`, which would **not** be recognized because of the prefix comparison (unused in the shipped data) |
| `MSGSTART x a ...` | prefix of 9 characters | `0x02` | Message at the start |
| `MSGEND x a ...` | prefix of 7 characters | `0x04` | Message at the end |
| `MSGTICK x a ...` | prefix of 7 characters | `0x400` | Message per tick while active |
| `EFFECT h path` | prefix of 7 characters | `0x41` | Effect at the target, height `h` |
| `END` | whole line | - | store |

Reader in the reimplementation: `nml_formats::temp_upgrades::parse_temp_upgrades` -> `TempUpgradeRecord` (GUID, flags
`flags::*`, `msg_start`, `msg_end`, effect path and height, `time_ms`), in the rules as `Rules::temp_upgrades` (path
`TEMP_UPGRADES_PATH`). It follows the prefix comparisons above and quirk W-18: the current record lives on beyond
`END`, `TEMPUPGRADETIME` is not recognized, `MSGTICK` and `MSGEND` share the slot (the later line wins). Deviation (A7):
more than 9 numbers in a message line, or an `argc` above 8 or below 0, are a load error with a line number instead of
a stack overflow. The simulation always decides by the flags, not by filled fields (tests `reads_records_like_the_game`,
`more_than_nine_numbers_in_a_message_line_are_a_load_error`, corpus `corpus_text.rs`).

Shipped data: 13 records 4450 ... 4462 (governor, shadow warrior, smith, flag bearer, military advisor,
sheriff, undertaker, animal evoker). Relevant for the economy are 4450 (`MSGTICK 57F 2 0 1000`,
"collect taxes") and 4451 (`MSGSTART/MSGEND 601`, construction speed 300 -> 100).

### `bonus.ini`

```
ATTACKBONUS_SWORD_PEEK 40          prefix comparison on the key, value = atol(rest)
MOUNTY_BONUS 18 20                 config GUID (18 = sight range) and percent for riders
```

Lines are trimmed on both sides, `#` at the start is a comment. Keys: `ATTACKBONUS_SWORD_PEEK`,
`..._PEEK_MOUNTY`, `..._MOUNTY_SWORD`, `..._CANNON_BUILDING`, `..._FIRE_BUILDING`, `..._MELEE_ARTILLERY`,
`..._MELEE_MELEEARTILLERY`, `..._UNIT_BUILDING` (negative = malus if the target does not accept the bonus),
`MOUNTY_BONUS id percent`. Evaluation: combat.

### `heroes.ini`

`HERO n` (prefix `HERO `, number after it), otherwise a comment. Order = sorting in the selection; heroes get
their own buttons and priority in voice output (file comment). Shipped data: heroic single figures
1200 ... 2725 and three elite units per nation. The list is untrimmed, duplicates are allowed; 14 campaign heroes,
6 multiplayer heroes, 18 elites.

## Coordinate system and units

No coordinates. Percentages are integers.

## Ignored areas

All lines that match none of the comparisons (comments, empty lines). `upgrades.ini`: bits with a negative number.

## Validation

None of its own. Proposed check for Rust: parse all `BIT` lines and compare them with the `PROPID` masks in
`property.ini` (every mask that is used must be defined as a bit: bits 0 ... 13 in the shipped data).

## Open questions

- `tempupgrades.ini`: `MSGSTART`/`MSGEND`/`MSGTICK` write as many parameters as the line has into a field of
  9 entries on the stack - more than 9 parameters overwrite the stack (at most 3 in the shipped data).
  `MSGTICK` and `MSGEND` share the same slot in the record; a record with both loses the first (not present in the
  shipped data). The reimplementation rejects more than 9 numbers when loading (A7, W-18).
