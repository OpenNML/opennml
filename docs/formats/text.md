# Bracket text formats (`.cfg`, `.lvl`, `.wld`, `.prj`, `.pef`)

Encoding: Windows-1252, line endings CRLF. Parser: `opennml_formats::text::parse_blocks`.

## Grammar

- Line oriented: one line = key + arguments (`GUID 1`, `LABEL "Holz"`).
- Tokens: words, `"strings"` (without escapes; `\` is a path separator), `{`, `}`.
- `{` attaches a block to the last statement read at the same level, also across a line break
  (`OBJECT_GUID 3602` / `{`).
- `[NAME]` headings (`[RESOURCES]`, `[CONFIGDATA]`) are ordinary keys, usually with a block.
- Comment lines begin with `#` or `//`.

- Deviation of the Rust implementation: `parse_blocks` rejects more than 64 nested `{ ... }` blocks with
  `FormatError::Syntax` (line of the excessive opening). `Node` is recursive; `Drop`, `Clone`, `PartialEq` and
  `find_all` would overflow the stack for corrupted files with thousands of open braces. The shipped data nests only a
  few levels deep.

## Use

| Extension | Root key | Example |
|---|---|---|
| `data/config/*.cfg` | `PROPERTY` | Editor properties (GUID, LABEL, SLIDER/DROPDOWN) |
| `data/r3d/**/*.cfg` | `OBJECT` | Model composition, see `object-cfg.md` |
| `.lvl` | `LEVELFILE V1.08e`, `[RESOURCES]`, `[NEUTRAL]`, ... | see `lvl.md` |
| `.wld` | `WORLDFILE V1.08e`, `[TERRAIN]`, `[AREA]`, `[OBJECT]`, ... | see `wld.md` |
| `.prj`, `default.dat` | `PROJECTFILE V1.08f` or `PROJECT CONFIGFILE V1.4b` | see `prj.md` |
| `.pef` | `EFFECT` | Particle effects |

## Special cases

None. All 1805 `.cfg`/`.lvl`/`.pef` files of the corpus (original installation) parse without errors with the grammar
above. All 57 `.wld` files, `nml-wiese.prj` and `default.dat` parse with the same grammar as well. The Rust corpus test
`corpus_text` covers `.cfg`, `.lvl`, `.pef`, `.wld`, `.prj` and `default.dat`.
