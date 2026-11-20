# OpenNML - instructions for coding agents

## Project

Faithful reimplementation of "No Man's Land" (2003). The original program is the reference. The game data is read at
run time from the user's own installation.

## Language

Everything is in English: documentation, comments, messages, commit messages, identifiers, file and folder names.
The one exception is `tools/opennml-release/templates/LIESMICH.txt`, the German readme for users in the release
archive; it follows `README.txt` in content.

## Game data

- No original game data in the repository: no images, savegames or tables from the exe.
- Corpus tests need `NML_GAME_DIR` (the original installation). Without the variable they skip themselves.
- The German and the English edition are supported. The tests detect the edition with
  `opennml_rda::testutil::game_version` and check the numbers per edition.

## Checks before a commit

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- The tests the commit affects, for example `cargo test -p <crate> <filter>` (corpus tests with `NML_GAME_DIR`).
- If `Cargo.toml` or `Cargo.lock` change: also `cargo deny check licenses`.

## Simulation rules

- `opennml-sim`, `opennml-formats`, `opennml-world`, `opennml-control` and `opennml-ui` stay free of `wgpu`, `winit` and `egui`.
  `opennml-client` and `opennml-game` stay free of `egui`.
- Determinism: integer or fixed-point arithmetic, or `f32` in a fixed order of operations. No libm transcendentals,
  no `HashMap` iteration, no time. Randomness comes only from the state.
- The viewer changes the simulation only through commands. Previews and queries leave the checksum unchanged.
- The golden checksum sequences (`*_GOLDEN`) are measured values. If one changes, find the cause. Adapt it only for
  an intended change of state, with a reason.
- New state belongs in the savegame. Savegames (`NMLGAME`) are MessagePack with field names (`game_save.rs`); every
  release loads the savegames of all earlier releases of the same major version.
- New state field: `#[serde(default)]` (or `default = "…"`) with the value an older savegame needs; `SAVE_FORMAT`
  stays. `FORMAT_VERSION` still rises (checksum, replays).
- Renaming or removing a field or enum variant, changing a field's type or meaning, or adding an element to a
  tuple: bump `SAVE_FORMAT` and add a step to `game_save/upgrade.rs` with a test on a savegame of the previous
  number.
- A changed derivation from the rules that the state caches: bump `SAVE_FORMAT`, add a recompute step in
  `opennml-sim/src/save/upgrade.rs` and an identity structure step (`|_| Ok(())`) in `game_save/upgrade.rs`.
- Behaviour change of the simulation: bump `FORMAT_VERSION` only.
- Never drop an upgrade step within a major version; never change a file in `crates/opennml-client/testdata/savegames/`
  after its release.
- Attributes on state types stay neutral for `postcard` (checksum, network, replays): no `skip_serializing_if`,
  `flatten` or `untagged`.
- Load rule: loading (including network sync, which is external input) rejects with a message only what would
  otherwise crash, hang or use unbounded memory (index, divisor, circles, lengths), and what does not fit the loaded
  match (format, checksum, rules, map). Fixed lengths and ranges use types with `try_from` (same bytes in the savegame). No plausibility checks
  and no rejection test per field: a new crash check gets one row in the combined test `crates/opennml-sim/src/save/tests/crash.rs`.

## References in comments

- Addresses like `0x004BA620` and names like `FUN_...` denote places in the original program.
- `re/<topic> <section>` and IDs like `O-163` refer to analysis notes that are not part of this repository.
- A comment line `QUIRK O-163: kept` or `QUIRK O-163: deviation (A7)` marks the place that implements a quirk of the
  original or deliberately deviates from it, preferably on the test that shows the behavior. The line holds nothing
  else; several IDs are separated by `, `. Plain mentions like `(O-163)` remain references. Keep marker lines with
  the code they belong to when moving it; do not invent new IDs.
- A1-A8 are the deliberate deviations listed in `docs/deviations.md`.

## Viewer MCP

`.mcp.json` registers the MCP server `opennml-viewer` (`opennml-viewer-mcp`). The control channel is described in
`docs/viewer-control.md`. In automated runs start the programs with `--mute --window-mode windowed`.

## Licensing

Code is GPL-3.0-or-later (`LICENSE`), `crates/opennml-bink` is LGPL-2.1-or-later, `docs/` and `assets/` are
CC BY-SA 4.0. New crates use `license.workspace = true`. New dependencies need a license that is compatible with
GPL-3.0-or-later (see `docs/licensing.md`).
