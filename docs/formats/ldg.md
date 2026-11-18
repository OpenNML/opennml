# Loading screen `.ldg`

Line-oriented text file (cp1252, CRLF, **no** bracket grammar) that describes the loading screen of a level:
background image, progress bar and text blocks.

## Purpose and locations

- 33 files under `data/levels/**` (32 for a level, one in `kampagne2/kamp2-10 old/`), plus
  `data/gifcon/default.ldg`, `default2.ldg`, `default3.ldg`, `multiplayer.ldg`, `randommap.ldg`.
- Single player: `<levelbase>.ldg`; if it is missing, `data/gifcon/default.ldg`;
  in multiplayer mode `data/gifcon/multiplayer.ldg` (flow in [lvl.md](lvl.md)).

## Layout

| Line | Format | Meaning |
|---|---|---|
| `LoadingScreen <path>` | path | Background image (`data/gfx/*.tga`) |
| `LoadingTimber <path>` | path | Image of the progress bar |
| `LoadingRect x0 y0 x1 y1` | `%d %d %d %d` | Rectangle of the bar (image coordinates of the template, e.g. 834 690 1072 1120) |
| `LoadingColor AARRGGBB` | `%X` | Color of the bar |
| `StartTextBlock` ... `EndTextBlock` | block | One text field with the following lines |
| `TextExtent x0 y0 x1 y1` | `%d %d %d %d` | Text rectangle |
| `TextGuid n` | `%d` | Text ID of the text table |
| `TextColor AARRGGBB` | `%X` | Text color |
| `TextFont name` | name | Font (`LoadingFont`, `LoadingFontKapitel`; default `LoadingFont`) |

In the shipped data (38 files, checked by the corpus test `multiplayer_maps_and_ldg_read`): all 33 files under
`data/levels/**` (also `kamp2-10 old`) have the four `Loading*` lines and **four** text blocks;
`data/gifcon/multiplayer.ldg` and `randommap.ldg` have the four `Loading*` lines and **three** text blocks;
`default.ldg`, `default2.ldg` and `default3.ldg` have only the `Loading*` lines and **no** text block.
Colors are written in lowercase (`ffff00ff`, `aa463910`) or uppercase (`80FFFFFF`).

## Reading rules of the original

More precise than the table:

- Lines are split at `\n`, **every** `\r` is dropped; the last line needs no line ending (in `multiplayer.ldg` it is
  missing after `EndTextBlock`).
- Every line is trimmed on the left, then compared at the start: `LoadingScreen `, `LoadingTimber `, `LoadingRect `,
  `LoadingColor `, `TextExtent `, `TextGuid `, `TextColor `, `TextFont ` each **with the space** and in exact
  spelling (case-sensitive); `StartTextBlock` and `EndTextBlock` count only as a **whole line** (one space after
  them is enough to stop them from matching). Other lines are ignored.
- The rest after the key goes unchanged to `sscanf`: `%d %d %d %d` (`LoadingRect`, `TextExtent`; if only two numbers
  succeed, those two apply and the rest stays), `%d` (`TextGuid`), `%X` (`LoadingColor`, `TextColor`; case does not
  matter); if reading fails, the old value stays. Image and font names stay unchanged (`LoadingScreen`/`LoadingTimber`:
  the path after the space; `TextFont`: up to the end of the line).
- Initial values per call: bar color `0xFFFFFFFF`; text block template `TextGuid 20012`, color `0xFFFFFFFF`,
  `TextExtent 20 20 800 600`, font `LoadingFont`. `StartTextBlock` does nothing, `EndTextBlock` appends the
  **template**: fields that are not named inherit from the previous block (quirk O-136).

Reader in the reimplementation: `opennml-formats/src/ldg.rs` (`parse_ldg`).

## Coordinate system and units

Screen/image coordinates in pixels of the template; no world coordinates.

## Ignored areas

None known.

## Validation

A check script verifies for every `.ldg` of a level the keys, argument count, hex colors, the nesting of the text
blocks and that the images exist in the VFS (`ldg: 32`, all without errors).

## Presentation

Which loading screen appears when, and how background, bar and text blocks are converted to the screen: the text
`y0` is divided by 1280, `y1` and the bar by 1024. Progress is the share of the `.wld` that has been read, in percent.
`StartTextBlock` resets nothing. Renderer in the reimplementation: `opennml-ui/src/loading.rs`.
