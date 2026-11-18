# "Handle effect" `.hef` (circle/radius indicators made of `.pef` effects)

Rust: `opennml_formats::hef::parse_hef`, the `effectlist.ini` with `opennml_formats::hef::parse_effect_list`.

Text file (cp1252, CRLF; header comment "CFG-File für einen Handle Effect"). A `.hef` describes how `.pef` effects are
set repeatedly around a unit (the "handle"): a circle or arc that either rotates (time mode) or appears as a ring of
`COUNT` effects (count mode). Used for the effect radii of abilities (healing circle, combat view, banishing ...) and
for animal evocations.

## Purpose and locations

| Files | Count | References |
|---|---|---|
| `data/effects/*.hef` | 24 | `data/gifcon/property.ini` (`VALUEID ... 1 1 data/effects/<name>.hef`, 35 lines; `healing_circle`, `radius_*`, `banishghost_circle`, `wildkatze`, `wolf`, `baer`) and in the program code `radius_bannen.hef`; 11 files are named nowhere (e.g. `katze.hef`, `original_wolf.hef`, `radius-kampfsicht.hef`) |

## Layout

Line by line; comparison from column 0 **without** trimming (only `\r` is removed from a line):
`PARTICLEMOVE`, `HALFVISIONRANGE`, `CENTERPOS` as a whole line, the other keys as a
**prefix including the space** (`ARCRANGE `, `COUNT `, `DURATION `, `TIMEOUT `, `EFFECT `,
`EFFECTDURATION `, `/PARTICLEMOVE`). All other lines (comments `#`, empty lines) fall through.
Numbers are read as integers (`atoi`), so they are whole numbers.

```
PARTICLEMOVE            starts a record (flags and counters set to 0)
ARCRANGE a b            arc from a to b degrees
HALFVISIONRANGE         radius * 2/3
CENTERPOS               radius 0 (all effects at the carrier)
DURATION ms             time mode: one revolution takes ms
COUNT n                 count mode: n effects at the same time
TIMEOUT ms              time mode: interval between two placed effects
EFFECT path.pef         effect to place
EFFECTDURATION ms       lifetime of each placed effect
/PARTICLEMOVE           appends the record to the list
```

### Record

| Key | Type | Meaning |
|---|---|---|
| `ARCRANGE` a | `i32` | Arc start (degrees); sets the arc flag (the flag is set only by this key) |
| `ARCRANGE` b | `i32` | Arc end (degrees); if the second value is missing, b = 0 |
| `DURATION` or `COUNT` | `i32` | Revolution time (ms) or count - **the same field**; `COUNT` additionally selects count mode |
| `TIMEOUT` | `i32` | Time mode: minimum interval (ms); count mode: timer (overwritten internally) |
| `EFFECT` | string | `.pef` path |
| `EFFECTDURATION` | `i32` | Lifetime of the placed effects (ms) |
| `HALFVISIONRANGE` | flag | Radius multiplied by 2/3 |
| `CENTERPOS` | flag | Radius 0 |

Runtime state per record: the most recently placed effect, the counter since the last placement (ms) and the total
time (ms), which determines the angle in time mode.

### Shipped data

| Key | Occurrences | Values |
|---|---|---|
| `PARTICLEMOVE` ... `/PARTICLEMOVE` | 48 (2 per file) | - |
| `ARCRANGE` | 48 | `0 180` 16x, `180 360` 16x, `0 360` 16x |
| `HALFVISIONRANGE` | 32 | - |
| `CENTERPOS` | 16 | - |
| `DURATION` | 32 | 2000 |
| `COUNT` | 16 | 20 (15x), 1 (1x) |
| `TIMEOUT` | 48 | 10 (32x), 0 (8x), 5 (7x), 2000 (1x) |
| `EFFECT` | 48 | `circle_effect_1.pef` 22x, `circle_effect_bannen.pef` 4x, `wolf_01.pef` 3x, `wolf_02.pef` 1x, 2x each `baer`, `katze` and seven further `circle_effect_*` |
| `EFFECTDURATION` | 48 | 2000 (47x), 1000 (1x) |

Typical: two records `0 180` and `180 360` with `DURATION 2000`, `TIMEOUT 10` - two effects run in opposite
directions, one half circle each in 2 s, and leave a trail of effects with a lifetime of 2 s.

## Ignored areas

Comments (`#`), empty lines, indented keys (they would not be recognized because of the column 0 comparison;
none in the shipped data).

## Open questions

- Meaning of the carrier property from which the radius is computed; judging by the key name `HALFVISIONRANGE`
  presumably the sight range - not verified.
