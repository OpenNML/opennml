# Textures (`.tga`)

Decoders: `opennml_formats::tga::decode_tga_game` (as the game loads, see below) and `opennml_formats::tga::decode_tga` (via the
`image` crate, shows the file as stored).

Variants that occur (all 1,062 effective `.tga` files):

| Image type | Bits | Alpha bits (descriptor) | Origin | Count |
|---|---|---|---|---|
| 2 (RGB/RGBA, uncompressed) | 32 | 0 | bottom left | 611 |
| 2 | 32 | 8 | bottom left | 168 |
| 2 | 24 | 0 | bottom left | 279 |
| 2 | 24 | 0 | top left | 3 |
| 3 (grayscale, uncompressed) | 8 | 0 | bottom left | 1 |

No color palettes, no RLE compression.

## 32 bits without alpha bits

For 611 of the 779 32-bit files the image descriptor (byte 17, bits 0-3) reports **0 alpha bits**, although every pixel
has a fourth byte. These bytes are neither all 0 nor all 255 (only one file is uniformly 255). The `image` crate (and
with it `decode_tga`/`opennml-extract convert`) takes the fourth byte as alpha anyway.

**How the game loads TGA files:**

- The loader checks only color map type 0 and image type 2/3, reads width, height and bits per pixel from offset `0x0C`
  and the pixels from offset 18. The length of the image ID and the descriptor (alpha bits, origin) are **not**
  evaluated.
- The upload writes file row `r` to surface row `H - 1 - r` and, per mip level, the nearest source texels; with 32 bits
  byte 3 **always** becomes alpha, with 24 bits alpha is full. 8-bit images are not copied when uploading (only 24 and
  32 bits are handled).
- Consequences: for the 611 files with 0 alpha bits the fourth byte acts as alpha (objects: the alpha test `GREATER 0`
  discards texels with alpha 0). The three effect textures stored top left (`data/effects/neu/licht03`, `licht04`,
  `rauch02`) appear upside down in the game.
- Rust: `opennml_formats::tga::decode_tga_game` reproduces this (image top first, RGBA8); the 8-bit file is returned,
  deviating from the original, as grayscale with alpha 255. `decode_tga` (via `image`) remains for the PNG export,
  which shows the file as it is stored.
- Not reproduced: conversion to A4R4G4B4 at `ScreenDepth` 16 or to DXT3 with `CompressedTextures` (quality loss of the
  original).
