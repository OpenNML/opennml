# nml-bink

Custom decoder for Bink 1 videos ("No Man's Land" ships twelve `.bik` files).

## Origin

Ported from FFmpeg, tag **n9.0.2** (LGPL-2.1-or-later):

| File here | Source in FFmpeg |
|---|---|
| `src/container.rs` | `libavformat/bink.c` (`read_header`, `read_packet`) |
| `src/bits.rs` | `libavcodec/get_bits.h` (reader with `BITSTREAM_READER_LE`) |
| `src/video.rs` | `libavcodec/bink.c` (`decode_frame`, `bink_decode_plane`, bundles, trees, block kinds; without `binkb_*` and the branches of revision `k`) |
| `src/dsp.rs` | `libavcodec/binkdsp.c` (IDCT, scaling, adding), `put_pixels8_8_c` from `libavcodec/pel_template.c` (in `hpeldsp.c`), `fill_block16_c`/`fill_block8_c` from `libavcodec/blockdsp.c` |
| `src/tables.rs` | `libavcodec/binkdata.h` (without `binkb_*`), `bink_rlelens` from `libavcodec/bink.c`, `ff_wma_critical_freqs` from `libavcodec/wma_freqs.c`, `rle_length_tab` from `libavcodec/binkaudio.c` |
| `src/audio.rs` | `libavcodec/binkaudio.c` (`decode_init`, `get_float`, `decode_block`, `binkaudio_receive_frame`; DCT path only) |
| `src/dct.rs` | `libavutil/tx_template.c` (`ff_tx_dct_init`, `ff_tx_dctIII`, `ff_tx_rdft_init`, `ff_tx_rdft_c2r`; the FFT underneath is a custom radix-2 FFT) |

Own code without an FFmpeg source (also under the license of the crate): `src/lib.rs`, `src/error.rs`,
`src/color.rs` (YUV -> RGBA according to BT.601, limited range) and the acceptance tool `examples/compare.rs`.
The copyright lines of the sources appear verbatim in the header of every ported file, followed by the modification
notice with date according to LGPL-2.1 section 2 b ("Ported to Rust and modified for `nml-bink`, 2026-10-25.");
deviations from the behavior of FFmpeg (only for corrupted data) are named in the module documentation of the
respective file.

## Acceptance against FFmpeg

`examples/compare.rs` compares the output with reference files that FFmpeg generated beforehand (the example does
not call FFmpeg):

    cargo run --release -p nml-bink --example compare -- video <bik> <frames.txt>
    cargo run --release -p nml-bink --example compare -- audio <bik> <ref.f32le>
    cargo run --release -p nml-bink --example compare -- rgb <bik> <frame> <ref.rgb>
    cargo run --release -p nml-bink --example compare -- shot <png> <bik> <ref.rgb|eigen:frame> [x0,y0,x1,y1 ...]

Result (measured values in `re/video`): all 15,666 frames of the twelve videos are YUV bit-identical,
and the sound of all eleven audio tracks has the same sample count and a deviation of at most 7.2 * 10^-7.

## Scope

Only what the game needs: Bink revision `i` without alpha plane and without grayscale, sound only in the DCT path.
Revision `b`, the `k` branches, the alpha plane and RDFT sound are not implemented; the decoder rejects such files
with an error message (`BinkError::Unsupported`). Corrupted input results in an error, never a panic.

## License

This crate - and only this crate - is licensed under the **GNU Lesser General Public License, version 2.1 or
later** (`LGPL-2.1-or-later`); the license text is in `COPYING.LGPLv2.1`. The rest of the repository
contains no line from FFmpeg. Anyone who distributes a program with this crate must provide or offer its complete
source code and allow rebuilding with a modified version.
