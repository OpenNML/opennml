# OpenNML - instructions for coding agents

## Project

Faithful reimplementation of "No Man's Land" (2003). The original program is the reference. The game data is read at
run time from the user's own installation.

## Language

Everything is in English: documentation, comments, messages, commit messages, identifiers, file and folder names.

## Game data

- No original game data in the repository: no images, savegames or tables from the exe.
- Corpus tests need `NML_GAME_DIR` (the original installation). Without the variable they skip themselves.
- The German and the English edition are supported. The tests detect the edition with
  `nml_rda::testutil::game_version` and check the numbers per edition.

## Checks before a commit

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- The tests the commit affects, for example `cargo test -p <crate> <filter>` (corpus tests with `NML_GAME_DIR`).
- If `Cargo.toml` or `Cargo.lock` change: also `cargo deny check licenses`.

## Simulation rules

- `nml-sim`, `nml-formats`, `nml-world`, `nml-control` and `nml-ui` stay free of `wgpu`, `winit` and `egui`.
  `nml-client` and `nml-game` stay free of `egui`.
- Determinism: integer or fixed-point arithmetic, or `f32` in a fixed order of operations. No libm transcendentals,
  no `HashMap` iteration, no time. Randomness comes only from the state.
- The viewer changes the simulation only through commands. Previews and queries leave the checksum unchanged.
- The golden checksum sequences (`*_GOLDEN`) are measured values. If one changes, find the cause. Adapt it only for
  an intended change of state, with a reason.
- New state belongs in the savegame.
- Load rule: loading (including network sync, which is external input) rejects with a message only what would
  otherwise crash, hang or use unbounded memory (index, divisor, circles, lengths), and what does not fit the loaded
  match (format, checksum, rules, map). Fixed lengths and ranges use types with `try_from` (same bytes in the savegame). No plausibility checks
  and no rejection test per field: a new crash check gets one row in the combined test `crates/nml-sim/src/save/tests/crash.rs`.

## References in comments

- Addresses like `0x004BA620` and names like `FUN_...` denote places in the original program.
- `re/<topic> <section>` and IDs like `O-163` refer to analysis notes that are not part of this repository.
- A comment line `QUIRK O-163: kept` or `QUIRK O-163: deviation (A7)` marks the place that implements a quirk of the
  original or deliberately deviates from it, preferably on the test that shows the behavior. The line holds nothing
  else; several IDs are separated by `, `. Plain mentions like `(O-163)` remain references. Keep marker lines with
  the code they belong to when moving it; do not invent new IDs.
- A1-A8 are the deliberate deviations listed in `docs/deviations.md`.

## Viewer MCP

`.mcp.json` registers the MCP server `nml-viewer` (`nml-viewer-mcp`). The control channel is described in
`docs/viewer-control.md`. In automated runs start the programs with `--mute --window-mode windowed`.

## Licensing

Code is GPL-3.0-or-later (`LICENSE`), `crates/nml-bink` is LGPL-2.1-or-later, `docs/` and `assets/` are
CC BY-SA 4.0. New crates use `license.workspace = true`. New dependencies need a license that is compatible with
GPL-3.0-or-later (see `docs/licensing.md`).
