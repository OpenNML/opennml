# Models `.rdw` (container `0x7DB`)

Rust: `nml_formats::rdo::parse_rdo` (standard and old format), paths and textures `nml_world::assets`.

`.rdw` is **the same binary format as `.rdo`** (identifier `0x7DB`; for layout, fields and transformation rules see
[rdo.md](rdo.md)). It differs only in content.

## Purpose and locations

- 99 files under `data/r3d/**`; 71 lie next to a `.rdo` of the same name (`tierbeschwoerer_wald.rdo` / `.rdw`), 28
  have no `.rdo` counterpart (`*_zu_pferd.rdw`, `*_wb.rdw`, `fish_wb.rdw`, ...).
- Content: a **rigid standing pose of the whole object** - only meshes of type 3002, no bones, no weights, animation
  count 0. Where the `.rdo` has a skin mesh (3001), the `.rdw` usually contains a 3002 mesh of the same name with the
  same vertex/triangle count (51 of 61; 10 differ, e.g. `frau_eng`); in addition the sub-objects (weapons, mount,
  load) are built in as further meshes (e.g. `dragoner_eng_zu_pferd.rdw`: rider + horse, `ochsenkarre_eng.rdw`: 6
  meshes against 1 in the `.rdo`). The `w` presumably stands for "world"/editor preview.
- **The game never reads `.rdw`.** The only references are in the project file `data/projects/nml-wiese.prj` (106
  times `OBJ_FILENAME "...\....rdw"`); its parser replaces every extension by `.rdo` for `OBJ_FILENAME` and
  `OBJ_SHADOW_FILENAME`. For 81 of the 106 references the `.rdo` exists; for 25 (`*_zu_pferd`, `*_wb` ...) it does
  not. For these types `OBJ_GAME_FILENAME` (overrides the model if longer than 1 character) names an existing `.rdo`
  (e.g. `dragoner_eng.rdo`); so the model of all 512 object types can be resolved ([prj.md](prj.md)).
- `.rdw` is therefore only interesting as an editor/preview format; a reader for `.rdo` reads `.rdw` without changes.

## Layout

As in [rdo.md](rdo.md). Observed values (all 99 files): `knochen_anzahl = 0`, `animations_anzahl_exporter = 0`, all
meshes of type 3002; `material` per mesh >= 0 in 98 files.

Short header excerpt (`tierbeschwoerer_wald.rdw`, offset `0x200`):

```
00000200: 0000 0000 0200 0000 0000 0000 0200 0000  ....  mesh_anzahl=2 knochen_anzahl=0 material_anzahl=2
00000210: 0000 0000 ...                            animations_anzahl_exporter=0, then hull
```

## Coordinate system and units

As in [rdo.md](rdo.md). Since all meshes are rigid, `v_object = v * R + position` holds throughout; the results hit
the stored mesh hull for all 267 meshes.

## Ignored sections

As in [rdo.md](rdo.md).

## Old format (1 file)

`data/r3d/animals/bär/bear.rdw` has the older layout from [rdo.md](rdo.md#old-format-1-file) (header `0x314` bytes,
mesh header 732 bytes); not referenced.

## Validation

Measured on all 99 files: 99 of 99 parse; variants: 1 old, 98 standard.

| Check | Count |
|---|---|
| face normals of unit length | 47,697 |
| face normals zero | 122 |
| mesh hull fits | 267 |
| meshes of type 3002 | 267 |
| vertex normals of unit length | 26,654 |
| vertex normals shorter | 69 |
| vertex normals NaN | 8 |
| winding matches the cross product | 47,689 |
| winding unclear | 8 |

Visual check: the ox, the pole, the cart with load and the wheels of `ochsenkarre_eng.rdw` stand in the right
position.

## Open questions

- Which tool originally used the `.rdw` (and the project file with `.rdw` references) - presumably the level editor; it
  has no function in the game itself.
