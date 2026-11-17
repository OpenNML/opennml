# AI modules `.mod` and module database `nml.db`

Rust: `nml_formats::kimodule` (reader, disassembler; see section 9).

## 1. Purpose and locations

- 291 files `data/kimodules/%08d.mod` (in `NML2.RDA`), file name = **module ID** (decimal, eight digits with leading
  zeros). Each file is a compiled AI script (source `*.aml`; the compiler is at the developer and is not shipped).
- In addition `data/kimodules/nml.db` (ID `MODB`): names, source paths and parameter types of 280 of the 291 modules
  (section 8).
- The path is built as `data/kimodules/%08d.mod`. The AI debugger additionally uses `%08d.mod` (only for messages).
- The source paths name the folders of the development (`D:\NML\Game\KIEngine\Scripts\...`): `Ablauf_Module` (55, helper
  modules called by level scripts such as `add_attacker`, `stopki`, `startplayer`), `Allg_Level_Normal` (101, shared
  building blocks), `ENG_NORMAL`/`SPA_NORMAL`/`PAT_NORMAL`/`PRI_NORMAL`/`SID_NORMAL`/`WID_NORMAL` (14-16 per nation),
  `Multiplayer` (34). Numbers from `nml.db` (280 entries); the 11 modules without an entry are listed in section 8.

## 2. Layout

All numbers little-endian, 4 bytes. The file is a sequence of blocks with a 12-byte header:

```text
Block   = ID[8] (ASCII, padded with spaces)  size:u32  content
File    = KIMODULE HEADER VARIABLE CONSTDAT CODEDATA VARMAP DEBUGGER     (all 291 files exactly like this)
```

The loader reads in a loop an ID (8 bytes) and a size (4 bytes) and compares them with `strncmp` (`KIMODULE` 8,
`HEADER` 6, `VARIABLE` 8, `CONSTDAT` 8, `CODEDATA` 8, `DEBUGGER` 8, `VARMAP` 6 characters):

| Block | Loader | Content |
|---|---|---|
| `KIMODULE` | nothing (continues with the next header) | Envelope; the size is **not** the remaining length (distance to `file length - 12` between -15 and +227 bytes), it is not used |
| `HEADER` | 4 bytes into the instance | Version, `1` in all files; not checked |
| `VARIABLE` | into the variable table | Variable ranges (section 3) |
| `CONSTDAT` | writes the values into the variables | Initial values (section 4) |
| `CODEDATA` | into the code object | Code (section 5) |
| `VARMAP` | inline, **without** using the block size | Variable names (section 6) |
| `DEBUGGER` | inline, **without** using the block size; reading ends after it | Source path, parameter count, signature, line table (section 7) |
| other | skip ahead by `size` | do not occur |

If `VARIABLE`, `CONSTDAT` or `CODEDATA` returns 0, the loader aborts with 0 (the module counts as not loaded);
otherwise it marks the instance as loaded. A read error in the header ends reading without an error (everything up to
then counts). The size fields of `VARMAP` (289 files) and `DEBUGGER` (all 291) do not match the content; since both
are read by content and `DEBUGGER` is the last block, this does no harm.

## 3. `VARIABLE`: variable ranges

```text
count:u32
count x { range ID:u32  length:u32 }
```

The **range ID** has the same form as a variable address with index 0 (in all files the index is 0):

| Bits | Meaning |
|---|---|
| 24-27 | **Kind** `k` (0-15; occurring 0-3): bit 0 = reference (the value is an address in the caller), bit 1 = list (object) |
| 16-23 | **Type** `t` (0-9; occurring 0-4) |
| 0-15 | Index within the range |

The variable table of the instance has a pointer to the table of the caller and 160 entries `{length, pointer}`
(8 bytes), entry `t + 10*k`. For each range, a `u32` field of that length filled with 0 is created; for kind 2 (bit 1
without bit 0) additionally a list object per element (a map with hash size 10). The checks are only `kind < 16`
(always true because of the mask `& 0xF`), `type < 10` (otherwise silently ignored) and `length > 0`. The reader of the
reimplementation additionally rejects lengths above `0x1_0000` (`MAX_RANGE_LEN`, the index has 16 bits; largest value
in the shipped data: 93).

Meaning (from the names in `VARMAP`, the parameter types in `nml.db` and the use):

| Type | Meaning | Examples from `VARMAP` |
|---|---|---|
| 0 | Integer | `x`, `y`, `count`, `guid`, `player`, `rand` |
| 1 | Boolean | `bFlag`, `bWait`, `upgrademade`, upgrade names (`pest`, `habgier`) |
| 2 | Building (object ID) | `haupthaus`, `lager`, `farm`, `hafen` |
| 3 | Unit (object ID) | `held`, `enemy`, `einheit`, `train` |
| 4 | Other object (nature, resource, flag) | `feld`, `gold`, `baum`, `pferd`, `flagge` |

| Kind | Meaning | Sum of the lengths in the corpus |
|---|---|---|
| 0 | Ordinary variable (per instance) | 10,453 (of which 7,238 `#INTERNAL#`: compiler constants and intermediate values) |
| 1 | Reference variable: holds an address; reading/writing through bit 0 follows the pointer into the table of the caller (output parameters, names `rx`, `ry`, `sammelx`) | 398 (type 0 only) |
| 2 | List (map object) of the type | 117 (type 0, 2, 3, 4) |
| 3 | Reference to a list | 2 (type 3) |

Types are not checked at run time; all values are `i32`. The types serve the compiler and the debugger.

## 4. `CONSTDAT`: initial values

```text
count:u32
count x { address:u32  value:i32 }
```

Each value is written into the variable (index < length, not into kind 2/3, otherwise silently ignored). In the
corpus all 7,173 entries are kind 0 (mostly `#INTERNAL#` variables; type 0: 7,034, type 1: 87, type 2: 14, type 3: 29,
type 4: 9). **Numeric constants of the code are found only here**: opcode 2 (immediate value) pushes in the corpus only
variable addresses of reference variables and lists (all 6,517 occurrences have kind != 0: kind 1 4,832, kind 2 1,683,
kind 3 2). When an instance is cloned, the values are copied with it, so constants apply anew to each instance.

## 5. `CODEDATA`: code

```text
size/4 x word:u32          word = opcode[31..28] | operand[27..0]
```

`size >> 2` words are read (a remainder is not read), the program counter starts at 0. Opcodes:

| Op | Name here | Operand | Frequency in the corpus |
|---|---|---|---|
| 0 | `LOAD` | Variable address | 122,381 |
| 1 | `STORE` | Variable address or `0x0FFFFFFF` (discard) | 12,992 |
| 2 | `PUSHA` | Address as a value | 6,517 |
| 3 | `STORE3` | Variable address (simple write) | 2 |
| 4 | `CALLB` | Builtin ID (1...757) | 106,115 |
| 5 | `CALLM` | Module ID (synchronous call) | 4,543 |
| 6 | `SPAWN` | Module ID (new thread) | 799 |
| 7 | `JMP` | Target (word index, absolute) | 20,710 |
| 8 | `JZ` | Target | 17,405 |
| 9 | `JNZ` | Target | 0 |
| A-F | - | - (step does nothing) | 0 |

Corpus findings: no jump target lies beyond the end of the code; 1,889 jumps target **exactly** the end (= return).
All variable operands lie in their ranges. All targets of `CALLM` (25 different modules) and `SPAWN` (183) exist as a
file. Every code starts with `STORE` of the parameters in reverse order (`parameter n-1` ... `0`).
`00052554.mod` (`multi_leben`) has empty code.

## 6. `VARMAP`: variable names (debugger only)

```text
count:u32
count x { length:u32  name[length]  address:u32 }
```

Each entry becomes a fixed-size record: the name goes into a 64-byte field (including the terminating zero), followed by
the address. The loader does not check the length (**quirk**: names of 64 characters or more would write into the next
record; the longest name in the corpus is 18). The reader of the reimplementation takes names of at most 63 bytes and
rejects longer ones with a message (E-15). Used only by the AI debugger (displaying variables). `#INTERNAL#` stands for
compiler variables.

## 7. `DEBUGGER`: source, signature, line table

```text
length:u32  path[length]        (256 bytes in the instance, without a length check; longest path 88)
parameter count:u32
length:u32  signature[length]   (e.g. "startplayer( 0, 0, ..., 0 ) -1")
lines:u32
lines x code index:u32          (line i starts at code index ..., ascending)
(a second table of the same size is empty: breakpoints)
```

For each parameter the signature gives the type as `kind*256 + type` (`515` = list of units, `771` = reference to
such a list) and at the end the return type (`-1` = none). The parameter count matches `nml.db` for all 280 modules.
The run time does not need the block; the debugger uses it to show source and lines.

## 8. `nml.db` (`MODB`)

```text
"MODB"  version:u32 (=1)
length:u32  root[length]        ("D:\NML\Data\KIModules")
count:u32 (=280)
count x {
  length:u32  name[length]      (Latin-1, e.g. "wid_normal_tierbeschwörer")
  length:u32  source[length]    (path of the .aml)
  n:u32  n x parameter type:u32 (0 integer, 1 boolean, 2 building, 3 unit, 4 object)
  return type:i32               (-1 none: 251; 0: 19; 2: 5; 3: 3; 4: 2)
  module ID:u32
}
```

Read from the path `data/KIModules/nml.db`. No entry for 11 modules: `multi_normal_groupattack`
(523821, 524077), `multi_normal_groupattack2` (555071, 555327), `multi_normal_deselctbuilding` (815535) and the six
`*_normal_angriffswellen` (590203, 590204, 590207, 590236, 590281, 590312). Conversely, every `nml.db` entry has a
file.

## 9. Reader and corpus test (Rust: `nml_formats::kimodule`)

Implemented in `crates/nml-formats/src/kimodule.rs`, corpus test `crates/nml-formats/tests/corpus_kimodules.rs`. The
reader follows the original loader: a loop as long as at least 12 bytes are left (a shorter remainder is the end
without an error); the block ID by prefix (`HEADER` and `VARMAP` 6 characters, otherwise 8 like `strncmp`); `CODEDATA`
reads `size >> 2` words, after which reading continues directly; `VARMAP` and `DEBUGGER` read their content without
the block size (after `DEBUGGER` it is over); unknown blocks are skipped by their `size`. If `VARIABLE`, `CONSTDAT` or
`CODEDATA` is missing, this is an error ("AI module without block ...").

- **Interface:** `parse(&[u8]) -> Result<KiModule, FormatError>` with `blocks` (IDs in file order), `header`,
  `ranges: Vec<(u32, u32)>` (range ID, length), `inits: Vec<(u32, i32)>`, `code: Vec<u32>`, `names: Vec<(String,
  u32)>` and `debug: Option<KiDebug { source, argc, signature, lines }>`; `parse_db(&[u8]) -> ModuleDb` with
  `DbEntry` for `MODB` (ID `MODB` required, `version` is not checked, as in the original); `module_path(id)`,
  `MODULE_DIR`, `DB_PATH`, `OP_NAMES`, `DROP`, `MAX_RANGE_LEN`; `disassemble` as a test aid (line `"{i:5}
  {word:08x} {OP:6} {operand}"`, operands of `LOAD`/`STORE`/`STORE3` with names from `VARMAP`, `drop` for
  `0x0FFFFFFF`, otherwise `k{kind} t{type} #{index}`).
- **Deviations from the original (E-15, A7):** All counts are limited by the remaining length (element size taken
  into account), reading past the end of the file is `Truncated`; names in `VARMAP` at most **63** bytes (the field of
  the original holds 64 with the zero byte), source paths at most 255 bytes; the length of a variable range is at most
  `0x1_0000` (`MAX_RANGE_LEN`, the index has 16 bits; largest value in the shipped data 93), otherwise an error instead
  of a memory explosion. The size fields of `VARMAP` and `DEBUGGER` are ignored as in the original. Blocks
  `VARIABLE`/`CONSTDAT`/`CODEDATA`/`VARMAP` that occur several times silently overwrite each other (does not occur in
  the shipped data).
- **Corpus test** `kimodules_parse_with_the_documented_invariants` (skips itself without `NML_GAME_DIR`), over all 291
  files from `NML2.RDA`:
  1. Block sequence exactly `KIMODULE HEADER VARIABLE CONSTDAT CODEDATA VARMAP DEBUGGER`, `HEADER == 1`, reading ends
     at the end of the file.
  2. Range IDs with index 0, kind <= 3, type <= 4; all `CONSTDAT` addresses kind 0 and in range; largest range
     length 93.
  3. All opcodes <= 8; jump targets <= code length (1,889 jumps exactly at the end); variable operands in range;
     `PUSHA` never kind 0; `STORE` on kind >= 2 only in the four unreachable `multi_normal_groupattack*`; 42 `LOAD` on
     kind 1 in the six `*_normal_buildingverwaltung`.
  4. All `CALLM`/`SPAWN` targets exist as a file; all `CALLB` IDs are in the builtin catalog.
  5. `nml.db`: 280 entries, every ID as a file, parameter count = `DEBUGGER.argc`; the 11 modules without an entry as a
     fixed list.
  6. Counts as guards: 106,115 `CALLB`, 325 different builtin IDs (1...757), 25/183 different `CALLM`/`SPAWN` targets.
     All matched on the first run.
- Unit tests in `kimodule.rs`: `parses_the_seven_blocks_in_file_order`,
  `codedata_reads_size_div_4_and_continues_behind_the_words`, `unknown_blocks_are_skipped_by_their_size`,
  `missing_code_block_is_an_error`, `truncated_and_oversized_inputs_fail_in_german` (every abort point, limits 63/64
  bytes and 255/256 bytes, range length `0x1_0000` valid, `0x1_0001` and `u32::MAX` errors), `nml_db_round_trip`,
  `disassembler_names_variables_and_drop`.
