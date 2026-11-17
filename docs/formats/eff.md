# Old effect format `.eff` (and texture lists `*.txt`)

Rust: no parser needed. **The game does not read `.eff`.**

Text files (cp1252, CRLF) of an older effect editor in the folder `data/effects/`. They describe layer effects
similar to the billboard layers of the [`.pef`](pef.md) (same curve technique), but no shipped program file reads
them.

## Purpose and locations

| Files | Count | Read by the game |
|---|---|---|
| `data/effects/*.eff` | 20 (`blood`, `einschlag*`, `feuer`, `feuer2/3`, `funken`, `hausrauch`, `kanoneneinschlag*`, `kugel*`, `muednungsfeuer`, `muendungsrauch`, `rauch_besch`, `test`, `zombie*`) | **no** |
| `data/effects/*.txt` | 12 (`blood`, `einschlag`, `einschlag2/3`, `explosion`, `explosion2`, `fire`, `kugel`, `licht`, `licht2`, `rauch`, `smoke`) | **no** |

Evidence: none of the keys `ANZLAYERS`, `[STARTLAYER]`, `ENDLOSLOOP`, `PINGPONG`, `XSCALING`, `NAMETEXTURE` appears
as a string in `NML.exe`, `ConfigTool.exe`, `fmod.dll` or `BINKW32.DLL` (byte search in the files). No data record and
no code names an `.eff` file (the character sequence `.eff` in three `transportboot_pat_*.rdm` is memory content of
a name field). Several `.eff` have a `.pef` of the same name (`feuer`, `funken`, `kugel`, `kugelrauch`,
`rauch_besch`, `zombie`, `zombie2`, `blood`, `einschlag_*`); the `.pef` are the successors that are used.

## `.eff` layout (for completeness only)

```
ANZLAYERS n                 n = number of layers that follow (correct in all 20 files)
[STARTLAYER]
TEXTURE <name>              name of a texture list (*.txt, NAMETEXTURE), e.g. smoke, fire, Licht
ENDLOSLOOP 0|1
ALPHABLENDING 0|1
PAUSE1 ms                   pause before
DAUER ms                    duration
PAUSE2 ms                   pause after (always 0 in the shipped data)
RED g / GREEN g / BLUE g    0...255
XOFF px / YOFF px           reference point in the texture
LOOP 0|1
PINGPONG 0|1                (always 0)
FADING      MAX a  MIN b  <100 raw values>
XSCALING    MAX a  MIN b  <100 raw values>
YSCALING    ...
ROTATION    ...             MAX/MIN in degrees
XMOVE       ...
YMOVE       ...
[ENDLAYER]
```

Curves: key line (`FADING` ...), then a `MAX` line and a `MIN` line (order MAX before MIN, the reverse of `.pef`),
then a line with exactly 100 integer raw values 0...64 (all 600 curves). Following the pattern of the `.pef`, the
value would be `value = MIN + raw * (MAX - MIN) / 64`; for `.eff` this is **not confirmed**, because no reading code
exists.

Statistics (100 layers): `ENDLOSLOOP` 0: 52, 1: 48; `ALPHABLENDING` 0: 47, 1: 53; `DAUER` 50...8000 ms;
`PAUSE1` 0...400; texture names `rauch` 30, `smoke` 29, `explosion` 10, `Licht` 6, ...

## `*.txt` layout (texture lists)

```
NAMETEXTURE blood
ANZTEXTURES 20
TEXTURE data/effects/blood/effekt0000.tga
...
```

The image folders named there (`data/effects/blood/`, `.../einschlag/` etc.) do not exist in the shipped VFS.

## Open questions

None (irrelevant for the game).
