# Tile bits `texturebits.bit`

Name list of the 32 bits of `TEX_BITS` (ground textures of the project file, [prj.md](prj.md)) for the level editor.

Bit numbers in this document are indexes counted from 0; the masks are given in parentheses. Bit 1 (`0x2`, not
walkable) blocks in the classification of layers 0-4 (never set in the shipped data); bit 2 (`0x4`, not buildable)
only sets the occupancy bit when loading.

## Purpose and locations

- One file: `data/config/texturebits.bit` (591 bytes, cp1252, CRLF).
- The game does **not read** it. The game uses only the number `TEX_BITS`.

## Reading

None in the game. Use of `TEX_BITS`: the project file parser stores it per texture; the game queries the bits of the
texture under a tile (bit 0 for footprints, bit 1 for walkability, bit 2 when loading).

## Layout

One line per bit, bits 0 ... 31 without a gap:

```
BIT 0 "Schnee- und Sandspuren"
BIT 1 "unbegehbar"
```

| Bit | Name | Meaning in the game |
|---|---|---|
| 0 (`0x1`) | Schnee- und Sandspuren | Footprints on this texture |
| 1 (`0x2`) | unbegehbar | Tile is not walkable |
| 2 (`0x4`) | Nicht bebaubar | Set for one texture in the shipped data; when loading it sets the occupancy bit of the tile |
| 3 (`0x8`) | Schatten aus | not set |
| 4-31 | `"Test 5"` ... `"Test 32"` | Placeholders |

`TEX_BITS` in the shipped data: 0 (40 textures), 1 (7), 4 (1).

## Coordinate system and units

None.

## Ignored sections

The whole file (editor only).

## Validation

The line format and the bits 0 ... 31 check out: 32 bits, named: 0 = Schnee- und Sandspuren, 1 = unbegehbar,
2 = Nicht bebaubar, 3 = Schatten aus.

## Open questions

- None. Bit 2 ("Nicht bebaubar") is evaluated by the game: when loading it sets the occupancy bit.
