# Animations `.rdm` (identifier `0x3E9`/`0x3EA`)

Rust: `opennml_formats::rdm::parse_rdm` (standard and old format). `RdmFile::ticks_per_second` forms `ticks_je_frame * fps`
in `u64` so that corrupted files do not panic by overflow (deviation from the original; 0 -> 4,800 unchanged).

glTF export: bone and node tracks as an animation per `ANIMATION` of the object `.cfg` (time = `ticks_to_ms` / 1000);
morph tracks are not exported. Without blending at the end of a loop (`interp_erster_letzter`) and without the 200 ms
blend between animations; the `#pivot` offset stays set in programs that do not return to the rest pose when the
animation changes.

Keyframe animations from the 3ds Max exporter. An `.rdm` can contain three kinds of tracks:

- **Bone tracks** (skeleton animation for skin meshes, type 3001 in [rdo.md](rdo.md)),
- **Node tracks** (rigid-body animation of individual rigid meshes, e.g. weapons, gates, windmill),
- **Morph tracks** (vertex animation: complete positions and normals per frame, e.g. sacks),

plus **time markers** (named frames, e.g. `EndLabel`, `Schuß`). Which `.rdm` belong to a model is set by the object
`.cfg` ([object-cfg.md](object-cfg.md)).

## Purpose and locations

- 1,581 files: 1,580 under `data/rdm/`, plus `data/r3d/pri/units/kanu/ruder.rdm` (old format, not referenced).
- Identifier: 1,423 files `0x3EA` (1002), 158 files `0x3E9` (1001). The layout is identical; the loader does not
  check the identifier.
- Next to many `.rdm` lies a `.cfg` of the same name (`ANIMATION { FILE, SPEED, STARTFRAME, LOOP, INTERPFIRSTLAST }`).
  The game does **not** read these companion files (the keys `SPEED`, `STARTFRAME`, `INTERPFIRSTLAST` do not occur in
  the game); their values appear identically in the header of the `.rdm`.
- The game loads the animations only at the first playback, not together with the model (quirk O-461); the
  reimplementation loads them together with the model.

## Reading

The game reads the `ANIMATION`/`FILE` entries of the object `.cfg`, looks the animation up by path and otherwise loads
the file. Defaults before reading: time factor 1.0, loop 1, interpolation first/last 1, time marker capacity 20. A time
marker is appended only if its `id` is new and the capacity is not exhausted. When an animation is attached to a model,
a bone track is looked up per bone (the track whose name, in lowercase and without the part up to the first space,
equals the bone name), and a node or morph track per mesh (exact name). Keys are searched and interpolated linearly
(position) or spherically (rotation); the transition from the last to the first key applies when `schleife` and
`interp_erster_letzter` are set.

## Layout

Little-endian, without alignment. Times are `u32` in **ticks**: 160 ticks per frame at 30 frames/s (= 4,800 ticks/s as
in 3ds Max); all files have `ticks_je_frame = 160`, `fps = 30`.

```
Header (0x334 bytes)
Bone track header  x A   (300 bytes each)    then per bone track: children, position, rotation, extra keys
Node track header  x B   (368 bytes each)    then per node track: position, rotation, scale keys
Morph track header x C   (316 bytes each)    then per morph track: frame headers, then per frame positions + normals
Time marker        x D   (268 bytes each)
```

### Header

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `u32` | `version` | `0x3EA` or `0x3E9`; unchecked |
| `0x004` | `char[256]` | `name` | File name; overwritten by the loader (as for `.rdo`) |
| `0x104` | `char[256]` | `pfad` | Exporter path; overwritten by the loader with the load path, key for finding an animation by path |
| `0x204` | `u32` | `laenge_ticks` | Length = time of the last key (1,376 files) or this + 1 (170 files); 35 files without keys |
| `0x208` | `u32` | `ticks_je_frame` | always 160 |
| `0x20C` | `u32` | `frames_je_sekunde` | always 30 |
| `0x210` | `f32` | `zeitfaktor` | `SPEED` of the companion `.cfg`; duration factor (time marker `zeit_ms = floor(floor(frame*1000/fps) * zeitfaktor)`, e.g. 0.7 for walking, 4.0 for grain growth); the playback divides it by an instance value/128 |
| `0x214` | `u32` | `startframe` | `STARTFRAME` (0 in the shipped data) |
| `0x218` | `u32` | `schleife` | `LOOP` 0/1 |
| `0x21C` | `u32` | `interp_erster_letzter` | `INTERPFIRSTLAST` 0/1: interpolate from the last to the first key at the end of the loop |
| `0x220` | `i32` | `knochenspur_anzahl` (A) | |
| `0x224` | `i32` | `knotenspur_anzahl` (B) | |
| `0x228` | `i32` | `morphspur_anzahl` (C) | |
| `0x22C` | `i32` | `zeitmarken_kapazitaet` | always 20; size of the time marker list |
| `0x230` | `i32` | `zeitmarken_anzahl` (D) | (recounted by the loader afterwards) |
| `0x234` | 256 bytes | `reserve` | Exporter garbage (`0xCD` fill), read but never used |

### Bone track header (300 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `char[256]` | `name` | Biped name (`bip01 l thigh`); assignment to the `.rdo` bone via the part after the first space |
| `0x100` | `f32[3]` | `ruheposition` | Mostly = position key 0 (17,293 of 21,523 tracks); not used by the game - the rest position comes from the `.rdo` |
| `0x10C` | `i32` | `kind_anzahl` | Number of children |
| `0x110` | 4 bytes | pointer | Garbage; the loader sets its own pointer |
| `0x114` | `i32` | `positionsschluessel_anzahl` | `npos` |
| `0x118` | `i32` | `rotationsschluessel_anzahl` | `nrot` |
| `0x11C` | 8 bytes | pointers | Garbage |
| `0x124` | `i32` | `zusatz_anzahl` | Number of 312-byte extra records; always 0 in the shipped data |
| `0x128` | 4 bytes | pointer | Garbage |

All A headers follow one another (read as one block), then per track:

| Block | Type per element | Count |
|---|---|---|
| `kinder` | `i32` (track index or `-1`) | `kind_anzahl` |
| `positionsschluessel` | position key: `u32` time, `f32[3]` position (object space) | `npos` |
| `rotationsschluessel` | rotation key: `u32` time, `f32[4]` quaternion **(w, x, y, z)** | `nrot` |
| `zusatz` | 312 bytes (meaning unknown) | `zusatz_anzahl` (always 0) |

### Node track header (368 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `char[256]` | `name` | Name of the rigid mesh (exact comparison with `RdoMesh.name`) |
| `0x100` | `f32[3]` | `pivot` | Object offset `X` of the Max node (unscaled): rest position `position = P - (X o S)*R` with world pivot `P` |
| `0x10C` | `f32[3]` | `position` | Rest position as `RdoMesh.position`, but from the Max scene of the animation (matches the model in only 1,139 of 2,210 cases) |
| `0x118` | `f32[4]` | `rotation_wxyz` | Rest rotation (scene of the animation) |
| `0x128` | `f32[3]` | `skalierung` | Rest scale `S` of the node (already included in the mesh vertices) |
| `0x134` | `i32` | `positionsschluessel_anzahl` | |
| `0x138` | `i32` | `rotationsschluessel_anzahl` | |
| `0x13C` | `i32` | `skalierungsschluessel_anzahl` | |
| `0x140` | `i32[3]` | `spur_vorhanden` | 0/1 per track (position, rotation, scale) according to the exporter; the game goes by the counts |
| `0x14C` | 12 bytes | pointers | Garbage |
| `0x158` | 24 bytes | `reserve` | Zeros |

Headers as one block, then per track `positionsschluessel`, `rotationsschluessel` and `skalierungsschluessel` (scale
key: `u32` time, `f32[3]` scale, `f32[4]` axis quaternion `w,x,y,z`).

### Morph track header (316 bytes)

Like the node track up to `0x133` (name, `pivot`, `position`, `rotation_wxyz`, `skalierung`; the rest of the name
field is often filled with spaces here), then `0x134` `i32 frame_anzahl`, `0x138` 4 bytes of pointer (garbage). Headers
as one block, then per track `frame_anzahl` frame headers (16 bytes: `u32` time, `i32` `vertex_anzahl`, 8 bytes of
pointer) and then per frame `vertex_anzahl * f32[3]` positions and `vertex_anzahl * f32[3]` normals. `vertex_anzahl`
is the same in all frames of a track and equal to `nv` of the mesh of the same name (213 of 213 resolvable cases). The
positions replace the mesh positions (object space, not node space; see "Evaluation").

### Time marker (268 bytes)

| Offset | Type | Name | Meaning |
|---|---|---|---|
| `0x000` | `u32` | `id` | Identifier; unique per animation (duplicates are discarded). `1` is mostly `EndLabel` (1,437 times) |
| `0x004` | `char[256]` | `name` | e.g. `EndLabel`, `LabelName` (default), `Schuß`, `Nahkampftreffer` |
| `0x104` | `i32` | `frame` | Frame number |
| `0x108` | `u32` | `zeit_ms` | `floor(floor(frame*1000/fps) * zeitfaktor)` (1,655 of 1,658 exact, 3 with a rounding deviation of 2-3 ms) |

## Evaluation (playback)

- **Key search**: last key with time <= t and its successor; interpolated linearly (position) or spherically
  (rotation). Times ascend per track. 17 rotation keys in `dynamit_flug.rdm` are not normalized (length down to 0.87) -
  normalize before use. If the time lies behind the last key, the last key applies. The interpolation factor is
  `(t - t0) / (t1 - t0)`, 1 for the same key. At the end of the loop with `schleife` and `interp_erster_letzter` the
  first key follows the last. Key times are scaled with the playback factor.
- **Skin mesh** (vertex shader `blending1...4.vsh`):
  `v' = sum_i w_i * ((v - ruheposition_i) * R(q_i(t)) + p_i(t))`, `ruheposition` from the `.rdo`, `p`/`q` from the bone
  track; the rotation keys are thus absolute orientations relative to the bind pose. Check: for 938 of 980 pairs (skin
  mesh, animation) the formula at the middle of the animation changes the edge lengths of the mesh by less than 15 % on
  average (single example `tierbeschwoerer_laufen`: 6-7 %; with the transposed rotation it would be 78-81 %). Bones
  without a matching track keep the rest position.
- **Node track**: the node matrix `T(-a) * S(s) * R(q) * T(b)` with `a = X o S` (component-wise product, `X` = track header
  `pivot`, `S` = track header `skalierung`), `s = S_key(t) / S` (component-wise), `q` = rotation key, `b` = position
  key; then the instance matrix: `v' = ((v - X o S) * (S(t) / S)) * R(q(t)) + p(t)` (vertices in the node space of the
  mesh). Checks: the track header is consistent in itself (`p(0) = position + (X o S)*R` in 1,769 of 1,816 tracks
  whose first rotation key equals the header rotation); if the track starts in the rest position of the model, the
  formula reproduces `v * R + position` of the mesh in 604 of 615 cases.
- **Morph track**: positions and normals of the frame (mixed linearly between two frames) replace the mesh data. They
  lie in **object space**: the world matrix of the mesh is set to the instance matrix, not to the node matrix. Check on
  the shipped data: in all 20 checked cases the frames fit object space, in none node space. Frames with a different
  vertex count are skipped (the mesh stays static). With a blend in progress the result is additionally blended with
  the old animation (200 ms).
- **Blending between two animations** (200 ms): an element is blended only if the track kind is the same in the old and
  the new animation (static <-> node track and morph <-> others jump), node tracks with slerp, morph per vertex
  linearly (normals not normalized), bones only with a track in both animations; a second blended change within the
  200 ms is rejected. Implemented in `opennml_world::anim::AnimationPlayer`; the bone quaternions are normalized there,
  unlike in the original.
- **Playback** (`opennml_world::anim`): ticks = milliseconds / 1,000 * 4,800 / `zeitfaktor` (`zeitfaktor` <= 0 -> 1); with
  a loop modulo `laenge_ticks`, otherwise held at the end; length 0 -> time 0.

## Coordinate system and units

As in [rdo.md](rdo.md): left-handed (D3D), Y up, facing -Z, row vectors (`v * M`), quaternions `(w, x, y, z)`; for a
right-handed renderer mirror keys like vertices (position `z -> -z`, quaternion `(w, -x, -y, z)`). Position keys of
the bone tracks lie in object space (e.g. pelvis at `y ~ 62`).

## Ignored sections

| Section | Size | Reason |
|---|---|---|
| Rest of `name`/`pfad` after the NUL | variable | Memory content; the fields are overwritten by the loader |
| Header `reserve` `0x234`...`0x333` | 256 | Read, no further access |
| Pointer fields in all track headers and frame headers | 4-12 | Replaced by own pointers by the loader |
| Node track `0x158`...`0x16F` | 24 | Zeros, not evaluated |
| Rest of the time marker names | variable | Memory content (`0xCD`) |

## Old format (1 file)

`data/r3d/pri/units/kanu/ruder.rdm` (not referenced; the game loads `data/rdm/kanu_pri_*_ruder.rdm`): header only
`0x238` bytes - at `0x220` there are pairs (count, memory pointer) for A, B, C; `zeitmarken_kapazitaet`,
`zeitmarken_anzahl` and `reserve` are missing; no time markers. Then tracks as above (here a node track `ruder` with 41
keys each). Not correctly readable with the loader of the game.

## Validation

Exact field coverage up to the end of the file; track counts plausible, child indexes `< A`, key times ascending and
`<= laenge_ticks`, quaternions normalized (length >= 0.5 allowed, see above), morph vertex count constant, time marker
frame inside the length, time formula.

Result: 1,581 of 1,581 files parse; variants: 1 old, 1,580 standard.

| Check | Count |
|---|---|
| quaternions not normalized | 17 |
| time marker time fits | 1,658 |

Cross-references to the object `.cfg`:

| Check | Count |
|---|---|
| animations | 1,873 |
| object `.cfg` | 547 |
| bones with a track | 30,006 |
| bones without a track | 99 |
| node track header consistent | 1,769 |
| node track header deviating | 47 |
| node track mesh found | 2,210 |
| node track mesh missing | 166 |
| node track rest position reproduced | 604 |
| node track rest position deviating | 11 |
| max bounding box min <= max | 1,864 |
| morph track mesh missing | 6 |
| morph track vertex count fits | 213 |
| skin edge change < 15 % | 938 |
| skin edge change >= 15 % | 42 |

Node tracks with a missing mesh are tracks for meshes that do not occur in the main model (they belong to sub-objects
of the same animation, e.g. weapons).

## Open questions

- Node tracks: 11 cases with a rest-position start and 47 track headers do not fit the formula; the share of the axis
  quaternion of the scale keys (`axisWxyz`, mostly identity in the shipped data) has not been traced.
- Meaning of the 312-byte extra records of the bone track (never present in the shipped data).
