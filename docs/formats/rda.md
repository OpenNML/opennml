# RDA archives (`Resource File V1.0`)

Archives `NML0.RDA` ... `nml10.rda` in the game directory. Reader: `nml_rda::Archive`, virtual file system:
`nml_rda::Vfs`.

## Layout

All values are little-endian.

| Offset | Size | Content |
|---|---|---|
| `0x000` | 256 | Text `"Resource File V1.0"`, padded with `0x00`/spaces |
| `0x100` | 4 | `u32` number of entries `n` |
| `0x104` | `n * 272` | Directory entries |

Directory entry (272 bytes):

| Offset | Size | Content |
|---|---|---|
| `0x00` | 256 | Path, null-terminated, cp1252, padded with spaces, `/` as separator |
| `0x100` | 4 | `u32` offset of the data in the archive |
| `0x104` | 4 | `u32` stored (compressed) size |
| `0x108` | 4 | `u32` unpacked size |
| `0x10C` | 4 | `u32` flags; `1` = zlib-compressed, `0` = uncompressed |

Shipped data of the installation: 11 archives, 8,370 entries, of which 8,334 have flags `1` and 36 have flags `0`
(for all 36 the stored size equals the unpacked size). All paths are lowercase, begin with `data/` and contain neither
`\` nor a leading `.`/`/`. 19 paths occur twice, each in `NML0.RDA` or `NML7.RDA` and again in `nml10.rda`; within one
archive there are no duplicates.

**Flags:** The game checks only bit 0 (`flags & 1`). Other flag bits are not evaluated and do not occur in the shipped
data.

The game does **not** check the header text: it reads the `0x104` header bytes into a buffer and uses only the count
at `0x100`.

## Loading

The game opens the archives with the prefix `nml` (`nml0.rda`, `nml1.rda`, ...), reads the directory of each and
enters every entry in a map from path to record. The path of a record is cut after 255 bytes.

## Load order and overriding

**Which archives, in which order:** Neither a fixed list nor a directory scan, but a name pattern. The game builds the
names `nml0.rda`, `nml1.rda`, ... from the prefix `nml`, a number and `.rda` (format `"%s%d%s"`) and opens them
relative to the working directory. The counting ends at the **first** failure; archives behind a gap are never loaded.
`%d` produces no leading zeros, so `nml01.rda` is not found. Under Windows the case of the file names does not matter
(`NML0.RDA` is opened as `nml0.rda`).

A second pass opens the archives `0 ... count-1` again in ascending order, reads the header and `count * 0x110` bytes
of directory and copies every entry with the archive name into the record table.

**Which one wins for a duplicate path:** the **last loaded** archive. For every entry `path_map[entry.path] = &record`
is set. For a key that already exists, the map operator returns the existing slot, so the assignment overwrites the
earlier pointer. So a higher archive number beats a lower one, and within one archive the later entry wins. In the
shipped data `nml10.rda` replaces 19 files of `NML0.RDA`/`NML7.RDA` this way. The earlier record stays in the table
but is no longer reachable.

The map key is the **raw** path from the directory (up to the first null byte), without normalization.

## Path lookup

**Normalization.** The game normalizes the requested path only after the attempt on the disk has failed (see "Loose
files"), in this order:

1. empty path -> not found;
2. remove whitespace at the start and at the end;
3. `\` -> `/`;
4. lowercase: only `A`-`Z` and `Ä`/`Ö`/`Ü` (cp1252 `0xC4`/`0xD6`/`0xDC` -> `0xE4`/`0xF6`/`0xFC`); other characters stay;
5. remove **one** leading `.`, then **one** leading `/` - so `./data/...` and `.\data\...` become `data/...`;
6. lookup in the map - hash and comparison are case-sensitive.

Because the request is lowercased but the keys are entered raw, archive entries with uppercase letters would be
unreachable for the game. In the shipped data all paths are lowercase, so the lookup behaves practically like a
case-insensitive comparison.

On a hit: open the archive of the record, seek to the data offset and read the stored size (a short read is an
error). With `flags & 1` the data is unpacked with zlib into a buffer of the unpacked size and returned as a memory
file; otherwise the buffer that was read is returned directly as a memory file with the unpacked size as length.

The existence check normalizes the same way, but **without** the whitespace trimming.

## Loose files

**Disk before or after the archives:** **before.** The game first opens the path unchanged as a file. If that works, a
real file is returned for the loose file and the archives are not asked at all. The existence check also looks at the
disk first. The path is relative to the working directory; the game does not set it at start and so assumes it is
started in the game directory (`nml<n>.rda` is also opened relatively). All places that change the current directory
do so only temporarily and restore the previous one; the Windows file systems ignore case and accept `/` like `\`.

So a loose file overrides any archive file of the same path - a mod or patch mechanism. In the installation there is
no overlap, however: only `DATA/MUSIC/*.OGG` (25 files), `DATA/Videos/*.bik` (12 files) and files in the root
directory (`Engine.ini`, `profiles.ini` and others) are loose; no archive contains `data/music/...` or
`data/videos/...`.

FMOD streams (music) and Bink videos open their files themselves by file name, bypassing the VFS; inside archives
they would be unreachable for these calls. The stream functions get a file name without the memory-loading flag, and
the video code passes the file name directly to Bink. The other FMOD calls with the memory-loading flag load from a
memory buffer; where that buffer comes from is not established.

## Behavior of the Rust implementation

Behavior of `crates/nml-rda/src/vfs.rs`, matching the original:

- `Vfs::open_game_dir` loads `nml0.rda`, `nml1.rda`, ... (case does not matter) up to the first missing number, like the
  original. For the installation (`nml0`-`nml10` without a gap) this changes nothing; other directories (a gap,
  `nml01.rda`) behave as in the game.
- Loose files take precedence: `Vfs::read` first reads `Vfs::loose_file(path)`, otherwise the archive. `Vfs::get` and
  `Vfs::len` refer only to archive entries; `Vfs::from_archives` knows no loose files.
- `normalize_path` removes whitespace at the edges as well as empty and `.` segments (`a//b`, `a/./b` -> `a/b`). The
  original trims whitespace (step 2 above) but does not merge `//` and `/./`. `..` stays and finds no loose file.
- Loose files are read once recursively when opening (an index of normalized paths). The original asks the disk on
  every access; files created later are seen by the Rust VFS only after opening again. Rust resolves case through the
  index, the original through the Windows file system.
- `Vfs::exists` matches the existence check of the original: first a loose file, then the archives. Unlike there, Rust
  trims the whitespace here too, because `exists` uses the same normalization as `read`.
- `Vfs::files` returns only archive files; `nml-extract list` therefore shows no loose files.

Kept deliberately (no effect on the shipped data):

- **Keys:** Rust also normalizes the archive paths (lowercase, `\` -> `/`) and therefore finds entries with uppercase
  letters; the original enters them raw and would never find them. Likewise `b.txt` and `B.TXT` are the same file in
  Rust. In the shipped data all paths are lowercase.
- **Lowercasing of the request:** Rust uses Unicode `to_lowercase`, the original only `A`-`Z` and `ÄÖÜ`. Identical for
  the paths of the shipped data (ASCII plus `ä`/`ö`/`ü`).
- **Prefixes:** Rust removes any number of leading `./` and `/`, plus all `.` and empty segments and whitespace at the
  start/end. After the trim the original removes exactly one `.` and then one `/` (`.data/x` becomes `data/x` there,
  `/./x` does not become `x`).
- **Loose files:** `loose_file` rejects `..` and searches only inside the game directory; the original passes the raw
  path on to the file open.
- **Header:** `Archive::open` checks the text `Resource File V1.0` (error `InvalidHeader`), the original does
  not. All 11 archives carry the text.
- **Sizes and errors:** Rust checks that the unpacked length equals the value in the directory (`SizeMismatch`) and
  reports zlib errors. For flags `0` the original returns a memory file of the unpacked length over a buffer of the
  stored size, and on a zlib error it returns `true` without setting the output file.
