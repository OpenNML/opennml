# Program icon

Original artwork for OpenNML ("tin medallion, red N"): a tin medallion with a rusty rim and four slotted screws, with a
red "N" with an ember gradient on it. No original game data, no logo of the original; the "N" is a hand-drawn path,
not a font.

License: CC BY-SA 4.0 (`LICENSE-CC-BY-SA-4.0` in the root directory, `docs/licensing.md`); this applies to the SVG
sources and to the PNGs and `opennml.ico` generated from them.

## Sources

| File | For | Contents |
|---|---|---|
| `opennml.svg` | 48 px and larger | full version with rust and ember patterns, screws, gloss |
| `opennml-32.svg` | 24 and 32 px | without patterns and screws, ember as a gradient, grid 32 × 32 |
| `opennml-16.svg` | 16 px | only rusty rim, tin disc and N without outline, grid 16 × 16 |

The small versions sit on whole pixels of their grid. Colors: background `#2a2420`, rust `#9a4a1a`/`#3a1e10`, tin
`#807d77`/`#a3a09a` (inner `#6f6c66`/`#8c8a84`), screws `#b3afa6`/`#d6d2ca`, N `#b5221c` with gradient `#c8381a` →
`#d9561c` → `#e6782a` → `#f0a030`, outline `#1a0e0a`.

## Generating

    cargo run --release -p nml-icon            # rewrites the PNGs and opennml.ico
    cargo run --release -p nml-icon -- --check # only checks whether they match the sources

The tool (`tools/nml-icon`, using `resvg`) generates `opennml-<size>.png` for 16, 24, 32, 48, 64, 128 and 256 px
and `opennml.ico` with all sizes (up to 128 px as bitmap, 256 px as PNG). The `assets_are_current` test of
`nml-icon` fails if a source was changed but the output was not regenerated.

## Usage

- Exe icon (Explorer): `apps/nml-game/build.rs` and `apps/nml-viewer/build.rs` embed `opennml.ico` on Windows
  with `embed-resource`.
- Window and taskbar: `nml_client::app_icon` sets the PNGs 16 to 128 px when the window is created, matching
  the monitor scaling (title bar 16 px, taskbar 32 px at 100 %).
