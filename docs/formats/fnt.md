# Bitmap fonts (`.fnt`)

Path: `data/fonts/<name>.fnt`, texture `data/fonts/<name>.tga` (same base name).
Parser: `opennml_formats::fnt::parse_fnt`.

| Line | Content |
|---|---|
| 1 | `FONTFILE V1.0` |
| 2 | empty |
| 3 | `[CHARINFOS]` |
| from 4 | per character: `code width height u0 u1 v0 v1` (`%i %i %i %f %f %f %f`, `\r\n`) |

Line 2 is empty; the game reads lines up to one that contains `[CHARINFOS]` and does not check the header line. The
cell size when drawing is the texture size / 16, all four UV values come from the `.fnt`, and the line height is the
largest `height`.

`u0..u1` and `v0..v1` are normalized texture coordinates (0-1) in the 16x16 grid of the texture.
Width/height are in pixels. The shipped fonts have 256 characters (codes 0-255, Windows-1252).
Orientation of `v`: the atlas is stored top to bottom (`v = 0` is the top edge), and the character sits at the top
left of its cell (`width x height` pixels from `u0`/`v0`); `opennml-render/src/text.rs` draws it this way without
mirroring (checked against the texture).
