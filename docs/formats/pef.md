# Effect file `.pef` (particle, billboard and model effects)

Rust: `opennml_formats::pef::parse_pef`.

Text file (cp1252, CRLF) from the internal effect editor. A `.pef` describes **one effect** made of any number of
**components** of three kinds: particle emitter (`PARTICLEEFFECT`), billboard effect with layers
(`BILLBOARDEFFECT` / `BILLBOARDLAYER`) and model effect (`OBJECTEFFECT`). All components run at the same time;
the order in the file is the drawing order.

## Purpose and locations

| Files | Count | Read by the game |
|---|---|---|
| `data/effects/*.pef` | 127 | yes, through paths in the code, in `effectlist.ini`, `.hef` and `data/gifcon/tempupgrades.ini`; 36 files are named nowhere (among others `affe.pef`, `object_test.pef`, `exlosion1.pef`) |
| Textures `data/effects/neu/<name>/<name>.tga` etc. | 39 paths | through `TEXTURE`/`TEXTUREn`; missing only in unreferenced files (`exlosion1.pef`, `einschlag_building.pef`) |

Five paths named in the code do not exist (`einsturzstaubwolken.pef`, `pest.pef`, `pestwolke.pef`,
`gouver_kampfsicht.pef`, `unithasgoldeffekt.pef` from `effectlist.ini` - the file is called `...effect.pef`); the
loader then returns no effect, and the envelope is discarded at the next game tick.

## Loading

When an effect is loaded, the game opens the file, creates a template and a first instance and runs the main loop
(below); at the end the scale is set to 1. A template is looked up in the effect library by its lowercased path; if
found, a new instance is created from the stored descriptions (type 1/2/3), with **no** new file read. Each component
kind has its own key parser.

## Layout

Line-by-line substring search (case-sensitive) as with the object `.cfg`
([object-cfg.md](object-cfg.md)); values are read with `sscanf("%s %f ...")`; strings between the first and the last
`"`; `{` is on its own line and is not evaluated (it contains no key), `}` ends the current block. Reading a line
removes only `\r`, not spaces/tabs - the substring search finds indented keys anyway.

**Note for a parser following [text.md](text.md):** the curves are an *own line* after their key line (100 numbers);
the bracket grammar delivers them as a statement with the first number as the key. A `.pef` parser must assign the
following line to a `*_MIN_MAX`.

```
EFFECT
{
	FILE "data\effects\building_smoke.pef"      <- not read
	MOVEMENTTIME 1.000000
	PARTICLEEFFECT { ... }                       <- 0...n components of any kind
	BILLBOARDEFFECT { ... BILLBOARDLAYER { ... } ... }
	OBJECTEFFECT { ... }
}
```

Main loop: lines up to the end of the file; a line with `EFFECT` opens the effect (the first line; any further line
with `EFFECT` outside would start another pass into the same instance - not present in the shipped data). Inside,
per line in this checking order: `}` (end), `MOVEMENTTIME`, `PARTICLEEFFECT`, `BILLBOARDEFFECT`, `OBJECTEFFECT`. The
sub-parsers read by themselves up to their closing `}`.

### `EFFECT { ... }`

| Key | Format | Meaning | In the shipped data |
|---|---|---|---|
| `FILE` | `"path"` | **not read** (the path comes from the call) | 127 |
| `MOVEMENTTIME` | `%f` | Seconds per 128 world units (1 tile) for particles that fly to a target; otherwise no effect | 127 (always `1.0`) |
| `RENDERPARTICLESFIRST` | `%d` | **does not occur in `NML.exe`**, ignored | 15 (always 1) |
| `PARTICLEEFFECT`, `BILLBOARDEFFECT`, `OBJECTEFFECT` | block | Component list of the template | 186 / 89 / 2 |

Distribution per file (particle, billboard, model): (0,1,0) 37, (1,0,0) 19, (2,0,0) 18, (1,1,0) 11,
(2,1,0) 8, (3,1,0) 7, (0,2,0) 6, otherwise <= 4; at most 9 emitters + 3 billboards (one file).

### `PARTICLEEFFECT { ... }`

Checking order in the parser: `}`, `BLENDALPHA`, `NAME`, `TEXTURE`, `LOOP`, `START`, `DURATION`, `BURST`,
`BDELAY`, `POS`, `EMIT`, `SPREAD`, `GRAVITY`, `YAW`, `YVAR`, `PITCH`, `PVAR`, `SPEED`, `ACCEL`, `ATTRACT`,
`AAXIS`, `COLOR0...3`, `ALPHA0...3`, `SIZE0...3`, `SPIN`, `LIFE`, `ROTY`, `EMTTYPE`, `EMTRADIUS`,
`MOVEPARTICLESWITHOBJECT`. Angles in **radians**, times in **seconds**, colors/alpha/size 0...255.

| Key | Format | Meaning | Shipped data (range) |
|---|---|---|---|
| `BLENDALPHA` | `%d` | 0 = additive (`eff_billboard`, SRCALPHA/ONE), != 0 = alpha blending (`eff_billboard_alpha`) | 186 (0: 121, 1: 65) |
| `NAME` | `"..."` | Editor name | 186 |
| `TEXTURE` | `"path"` | Lowercased, loaded without mip levels; if the file is missing or the path is empty (`TEXTURE ""` in 3 blocks), it is drawn without a texture (white) | 186 |
| `LOOP` | `%d` | 0 = emitter ends at `START + DURATION` | 186 (0/1) |
| `START` | `%f` | Start of emission (effect time) | 0...1 |
| `DURATION` | `%f` | Duration of emission (only with `LOOP 0`) | 0.04...5 |
| `BURST` | `%d` | != 0: emission in bursts | 186 (0: 115, 1: 71) |
| `BDELAY` | `%f` | Interval of the bursts | 0...1 |
| `POS` | `%f %f %f` | Center of emission (effect space) | -69...1635 |
| `EMIT` | `%d` | Particles per second or per burst (`1/EMIT` is stored as the interval) | 1...100 |
| `SPREAD` | `%f` | Half edge length of the start square in x/z | 0...60 |
| `GRAVITY` | `%f %f %f` | Acceleration (acts by half) | -500...377 |
| `YAW`, `YVAR` | `%f` | Direction around the vertical axis +- spread | 0...2pi |
| `PITCH`, `PVAR` | `%f` | Elevation angle (pi/2 = up) +- spread | 0...3pi/2 |
| `SPEED` | `%f` | Start speed | 0...719 |
| `ACCEL` | `%f` | **Time factor of the movement** (position, rotation, attraction run with `dt*ACCEL`), not an acceleration | 0...15 |
| `ATTRACT` | `%f` | Attraction to the emission center | -40...395 |
| `AAXIS` | `%d %d %d` | Per axis 0 = attraction in this axis off | 0/1 |
| `COLOR0...3` | `%f %f %f %f` | Knot: age (s), R, G, B | 744 |
| `ALPHA0...3` | `%f %f` | Knot: age (s), alpha | 744 |
| `SIZE0...3` | `%f %f` | Knot: age (s), half edge length in world units | 744 |
| `SPIN` | `%f` | Rotation of the quads over the lifetime (rad; `SPIN/LIFE` is stored); < 0: random amount | -48...14 |
| `LIFE` | `%f` | Lifetime of each particle | 0.5...6.5 |
| `ROTY` | `%f` | Revolution around the vertical axis over the lifetime (rad; `ROTY/LIFE` is stored) | -21...24 |
| `EMTTYPE` | `%d` | 1 = square (`SPREAD`), 2 = additionally a ring `EMTRADIUS` | 181 (1: 79, 2: 102) |
| `EMTRADIUS` | `%f` | Ring radius (only `EMTTYPE 2`) | 181 (0...500) |
| `MOVEPARTICLESWITHOBJECT` | `%d` | != 0: store particles relative to the effect position (they move along) | 69 (0: 38, 1: 31) |

After reading, the parser computes the slopes of the curves: after `COLOR3`, `ALPHA3` and `SIZE3` respectively:
segment `i` (0...2) = `(value[i+1] - value[i]) / (t[i+1] - t[i])`. `LIFE` scales all knot times by `new/old` - since
`LIFE` has already been stored before, the factor is 1; the knot times therefore stay **absolute seconds** (all files
have `t3 = LIFE`). In all 186 blocks, `SPIN`, `LIFE`, `ROTY` appear in this order (necessary because `ROTY` divides by
`LIFE`).

**Missing keys:** the description is created at loading **without defaults** (uninitialized memory); only
`MOVEPARTICLESWITHOBJECT` is always preset to 0. In the shipped data, `MOVEPARTICLESWITHOBJECT` is missing in 117
blocks (-> 0) and `EMTTYPE`/`EMTRADIUS` in 5 blocks (`exlosion1.pef` 4x, `geist.pef` 1x) - their values are undefined
in the original (heap content). Recommendation: assume the editor defaults `EMTTYPE 1`, `EMTRADIUS 0`; `EMTTYPE`
!= 2 acts like 1 anyway.

### `BILLBOARDEFFECT { ... }`

| Key | Format | Meaning | Shipped data |
|---|---|---|---|
| `NAME` | `"..."` | Editor name | 89 |
| `POSITION` | `%f %f %f` | Origin of all layers (effect space) | 89 (-580...151) |
| `BILLBOARDLAYER` | block | One layer (list of layers) | 185 (per effect 1: 38x, 2: 32x, 3: 6x, 5: 13x) |

### `BILLBOARDLAYER { ... }`

Checking order: `}`, `NAME`, `NUMTEX` (with texture lines), `DIRECTIONAL`, `LOOP`, `BLENDALPHA`, `OFFSET`,
`START`; after that **additionally on the same line** `DURATION`, `PAUSE`, `COLOR`, `ROTOFF`, `SIZE`,
`XSCALE_MIN_MAX`, `YSCALE_MIN_MAX`, `XMOVE_MIN_MAX`, `YMOVE_MIN_MAX`, `ROTATION_MIN_MAX`, `FADING_MIN_MAX`,
`MOVEUD_MIN_MAX` (without double hits in the shipped data).

| Key | Format | Meaning | Shipped data |
|---|---|---|---|
| `NAME` | `"..."` | Editor name | 185 |
| `NUMTEX` | `%d` | Number of textures; sets `SIZE` provisionally to 64x64 and then reads **exactly `NUMTEX` following lines**; each with `TEXTURE` supplies texture no. *line index* (the number in the key `TEXTURE0...19` is not evaluated). Texture 0 sets `SIZE` to its pixel size | 185 (1: 183, 20: 2) |
| `TEXTUREn` | `"path"` | Single image `i` of an image sequence | 223 |
| `DIRECTIONAL` | `%d %f %f` | != 0: flat quad in world coordinates instead of a billboard; the floats tilt it (`RotX(first)`, `RotZ(second)`, rad). If the key is missing: 0 | 114 (1: 96, 0: 18) |
| `LOOP` | `%d` | Restart the layer after `DURATION + PAUSE` | 185 |
| `BLENDALPHA` | `%d` | As for particles (0 = additive) | 185 (0: 169, 1: 16) |
| `OFFSET` | `%f %f %f` | Offset to `POSITION` | 185 |
| `START` | `%f` | Start (s of effect time) | 0...4 |
| `DURATION` | `%f` | Visible duration; time base of the curves | 0.04...8 |
| `PAUSE` | `%f` | Invisible pause after it | 0...1 |
| `COLOR` | `%f %f %f` | RGB 0...255 | 185 |
| `ROTOFF` | `%d` | != 0: pivot moved by `OFFSET` | 185 (1: 2) |
| `SIZE` | `%d %d` | Reference size x/y (world units) | 181 (32...128); missing in 4 layers -> pixel size of texture 0 |
| `XSCALE_MIN_MAX` | `%f %f` + curve line | Factor half width = `XSCALE * SIZE.x` | 185 |
| `YSCALE_MIN_MAX` | same | Factor half height = `YSCALE * SIZE.y` | 185 |
| `XMOVE_MIN_MAX` | same | Offset x (billboard: screen right; flat: world x) | 185 |
| `YMOVE_MIN_MAX` | same | Offset y (billboard: screen up; flat: world z) | 185 |
| `ROTATION_MIN_MAX` | same | Rotation in **degrees**, converted to rad after reading (`* 0.017453292`) | 185 |
| `FADING_MIN_MAX` | same | Alpha 0...255 | 185 |
| `MOVEUD_MIN_MAX` | same | Offset world y, evaluated **only with `DIRECTIONAL` != 0** | 114 (exactly the layers with `DIRECTIONAL`) |

### Curves (`*_MIN_MAX`)

- Key line `NAME_MIN_MAX a b`, **following line** with exactly 100 raw values separated by one space each (all 1,224
  curves of the shipped data have 100 values).
- The reader reads `sscanf("%f")` 100 times and jumps behind the next space; the mapping is
  **`value = a + raw * (b - a) / 64`**. Raw values lie in 0...64 (editor: 64 = `b`); in the shipped data 0...64 (once
  `-0.000006`). `a` is called "MIN" in the file but is not checked (`a > b` is possible, e.g. `XSCALE` in `.eff`).
- 43 curves contain `-1.#IND00` instead of numbers (the MSVC `printf` of a NaN); this concerns only curves with
  `a == b` (the editor divided by `b - a = 0`). The MSVC `sscanf` reads `-1.` from it; because of `b - a = 0` the value
  is `a` in any case. A reimplementation should take unreadable values as 0 (or -1).
- Time base: knot `i` (0...99) belongs to layer time `i * DURATION / 99`; interpolated linearly in between.

### `OBJECTEFFECT { ... }`

Checking order: `}`, `NAME`, `OBJECT`, `ANIMGUID`, `POSITION`, `ROTATION`, `CULLING`, `SRCBLEND`,
`DESTBLEND`, `FADE_IN`, `FADE_OUT`, `LOOP_FADING`, `FADEIN_DUR`, `FADEOUT_DUR`, `DURATION`, `MOVEMENT`.
Defaults exist when created anew (they are **not** set when loading).

| Key | Format | Meaning | Shipped data |
|---|---|---|---|
| `NAME` | `"..."` | Editor name | 2 |
| `OBJECT` | `"path.rdo"` | Load the model with its `.cfg` and create an instance | 2 (`strahl01.rdo`) |
| `ANIMGUID` | `%d` | Animation played at the start (default -1) | 0 |
| `POSITION` | `%f %f %f` | Offset in effect space | 0 |
| `ROTATION` | `%f %f %f` | Read, **not used** when drawing | 0 |
| `CULLING` | `%d` | 0 = `CULLMODE NONE`, otherwise CCW (default 1) | 0 |
| `SRCBLEND`, `DESTBLEND` | `%d` | `D3DBLEND` values (default 5/6) | 6/7 and 5/13 |
| `FADE_IN`, `FADE_OUT` | `%d` | Fade-in/fade-out on | 0/1 |
| `LOOP_FADING` | `%d` | From the start again after fading out | 1 file |
| `FADEIN_DUR`, `FADEOUT_DUR` | `%d` | Duration in **ms** | 0/1000 |
| `DURATION` | `%d` | Visible duration in ms before fading out | 0 |
| `MOVEMENT` | `%d` | Playback speed of the animation, 128 = normal; default 128 | 1 file (128) |

`object_test.pef` (not referenced) and `strahl01.pef` (through `effectlist.ini`, animal totem) are the only model
effects.

## Coordinate system and units

Effect space = world space without rotation, origin at the effect position; the effect rotation around y acts on
`POS`, `GRAVITY`, `POSITION`/`OFFSET` and the emission direction. Units like the world (1 tile = 128).

## Ignored areas

`FILE`, `RENDERPARTICLESFIRST`, `ROTATION` of the model effect, unknown lines. `.eff` files and the
`*.txt` texture lists in the same folder belong to an older format ([eff.md](eff.md)).

## Validation

Statistics above come from a pass over all 127 files (counting blocks/keys, value ranges, curve lengths, `#IND` check).
In the corpus test (`crates/opennml-formats/tests/corpus_effects.rs`) the Rust parser reads all 127 files and checks per
file the component count against `count("PARTICLEEFFECT")` etc. as well as the sums above. Three particle blocks have
`TEXTURE ""` (`schatten-kampfsicht.pef`, `units-kampfsicht.pef`, `units-panzerung.pef`).

## Open questions

- The values of the missing particle keys `EMTTYPE`/`EMTRADIUS` (5 blocks) depend on heap content in the original; not
  checked by running it.
