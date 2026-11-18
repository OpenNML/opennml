# Contributing

Thank you for helping with OpenNML. This page explains how to build, test and submit changes.

## Build and test

You need the Rust toolchain (the version in `rust-toolchain.toml` is installed automatically) and, for the game data
tests, an original installation of *No Man's Land*.

```bash
cargo build --release
cargo test -p <crate> <filter>      # the tests your change affects
cargo test --workspace              # everything; takes a long time
```

Corpus tests read the original game data. Point `NML_GAME_DIR` at the installation folder (the one that contains
`NML0.RDA`). Without the variable these tests skip themselves. German and English editions are supported.

## Checks before a commit

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- The tests your change affects.
- If you change `Cargo.toml` or `Cargo.lock`: `cargo deny check licenses` (install with
  `cargo install --locked cargo-deny`).

Further rules for the simulation (determinism, crate boundaries, savegame loading) are in [AGENTS.md](AGENTS.md).

## License of contributions

By contributing you agree that your work is published under the license of the part you change:

- source code: GPL-3.0-or-later
- `crates/opennml-bink`: LGPL-2.1-or-later
- `docs/` and `assets/`: CC BY-SA 4.0

Details: [docs/licensing.md](docs/licensing.md). New dependencies need a license that is compatible with
GPL-3.0-or-later.

## What must not be in a contribution

- No original game data: no images, sounds, texts, savegames or tables taken from the game files.
- No decompiled code from the original program. Describe observed behavior in your own words instead.
