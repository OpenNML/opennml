# Licenses

Short version in the README, section "License".

## Overview

| Part | License | File |
|---|---|---|
| Source code of all own crates, programs and tools | GPL-3.0-or-later | `LICENSE` |
| `crates/opennml-bink` (port from FFmpeg) | LGPL-2.1-or-later | `crates/opennml-bink/COPYING.LGPLv2.1` |
| Documentation `docs/`, artwork `assets/` (program icon) | CC BY-SA 4.0 | `LICENSE-CC-BY-SA-4.0` |

The license texts come from the SPDX license list (crate `spdx` 0.13.4, versions `GPL-3.0` and `CC-BY-SA-4.0`).
`Cargo.toml` has `license = "GPL-3.0-or-later"` under `[workspace.package]`; each crate takes it over with
`license.workspace = true`, only `opennml-bink` sets `LGPL-2.1-or-later` itself.

## Code: GPL-3.0-or-later

- **Modifications stay open.** Anyone who changes the reimplementation and passes it on must provide the source code.
  For the reimplementation of an old game on which many people should work, this is intended; similar projects
  (OpenMW, OpenRA) do the same.
- **It fits the video decoder.** `opennml-bink` is a port from FFmpeg under the LGPL-2.1-or-later and is statically
  linked into every exe. The LGPL allows passing its code on under the GPL; together this gives a program under the
  GPL-3.0-or-later without special rules for distributing the exe. With a permissive license (MIT/Apache-2.0) the
  exes would still have carried the obligations of the LGPL.
- **"or-later"** keeps the way open to later versions of the GPL.

Rejected: `MIT OR Apache-2.0` (usual in Rust, but closed reuse would be possible, and the exe carries LGPL
obligations anyway because of `opennml-bink`).

## Documentation and artwork: CC BY-SA 4.0

Software licenses fit texts and images poorly. CC BY-SA 4.0 requires attribution and the same license for
modifications, which corresponds to the GPL for the code. This applies to the program icon (`assets/icon/`, own
design) and everything in this style in the future. The PNGs and `opennml.ico` are generated from the SVG sources and
are under the same license.

Exception in the documentation: where a file quotes third-party material (for example FFmpeg in
`crates/opennml-bink/README.md`), the license of that material applies to the quotation.

## Name and original

- The project is called OpenNML; "No Man's Land" appears only descriptively (what the reimplementation is for),
  together with the note in the README: not an official product, names and trademarks belong to their owners, no
  affiliation with the rights holders.
- You need your own original installation to play; the repository contains no original data.
- The program shows its own identifier at the bottom right ("OpenNML v..."). As in the original, the window title of
  `opennml-game` is "No Man's Land".

## Dependencies

421 third-party crates at the last check. All are under licenses compatible with the GPL-3.0-or-later (mostly MIT or
Apache-2.0, plus BSD, ISC, Zlib, Unlicense, 0BSD). Worth mentioning:

| License | Crates | Note |
|---|---|---|
| MPL-2.0 | `symphonia*` (Ogg Vorbis sound) | compatible with the GPL (MPL-2.0 section 3.3) |
| OFL-1.1, Ubuntu-font-1.0 | `epaint_default_fonts` (fonts in the viewer) | font licenses, distribution with the license notice |
| Unicode-3.0 | `unicode-ident` | Unicode data license |
| `Apache-2.0 OR GPL-2.0-only` | `self_cell` | used under Apache-2.0 |
| `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | `r-efi` | used under MIT |

No dependency is under a license that would be compatible only with the GPL-2.0 or not at all with the GPL.

### Check

`deny.toml` sets the allowed licenses; `opennml-bink` is the only crate with an exception for the LGPL. The check is

    cargo deny check licenses

(tool: `cargo install --locked cargo-deny`, checked with 0.20.2). Run it whenever a change touches `Cargo.toml` or
`Cargo.lock`. If a new license comes in, it is added to `deny.toml` only after checking its compatibility with the
GPL-3.0-or-later, and added to the table here.

### Distributing the programs

Anyone who distributes an exe provides the source code or offers it (GPL-3.0 section 6), together with the license
notices of the dependencies.
