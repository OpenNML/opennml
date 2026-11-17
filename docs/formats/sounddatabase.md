# Sound database and audio configuration

Rust: `nml_formats::sound`, run time in `nml_audio` (database `nml_audio::db::SoundDb`).

All files are text in cp1252 with CRLF. The game reads files with a `data/...` path through the VFS ([rda.md](rda.md)),
that is, from the archives; exceptions are music streams (see "Audio files").

| File | Content |
|---|---|
| `data/sfx/sounddatabase.dat` | Object GUID -> action -> list of sound GUIDs |
| `data/sfx/soundguidlist.txt` | Sound GUID -> volume factor and WAV path |
| `data/sfx/actionidlist.txt` | Action ID -> name (description only) |
| `data/sfx/otherevents.ini` | Event name (`SFX_...`) -> sound GUID |
| `data/gifcon/musicconfig.ini` | Music per nation, combat music thresholds |
| `Data/TXT/sound.txt` | Music ID (script actions) -> OGG path |
| `data/gifcon/messages.ini` | Among others `SOUND`/`SOUNDONCE` per message (sound GUID) |
| `data/menü/*.dlg` | `MUSIC <path>` and `MENUFRAME_MUSIC`/`MENUFRAME_DONOTLOOPMUSIC` |

The game loads `data/sfx/sounddatabase.dat` first and then `data/sfx/otherevents.ini`, when it loads `Engine.ini`
(block `<SoundManager>`).

## `sounddatabase.dat`

```
GUIDLIST=Data/TXT/guids.deu
SOUNDGUIDLIST=Data/sfx/soundguidlist.txt
ACTIONIDLIST=Data/sfx/actionidlist.txt


GUID 2215
	ACTIONID 00103
		SOUNDGUID 00232
		SOUNDGUID 00234
	ACTIONID 00100
		SOUNDGUID 00008
```

Line by line; every line is trimmed on the left (so the tab indentation is only cosmetic) and then checked by a
**prefix comparison of fixed length** (`left(n)` + exact comparison, case-sensitive) in this order:

| Prefix (length) | Value from column | Effect |
|---|---|---|
| `SOUNDGUIDLIST=` (14) | 14 | Path; loads the sound GUID list immediately |
| `ACTIONIDLIST=` (13) | 13 | Path; loads the action list immediately |
| `GUID` (4) | 5, `sscanf("%d")` | Current object GUID; creates its action map if needed |
| `ACTIONID` (8) | 9, `sscanf("%d")` | Current action of the current GUID; creates its list if needed |
| `SOUNDGUID` (9) | 10, `sscanf("%d")` | Appends the sound GUID to the list of the current action (`AddTail`) |

Other lines are skipped. Consequences:

- `GUIDLIST=Data/TXT/guids.deu` is **not** evaluated: the line matches the prefix `GUID`, `sscanf` on `IST=Data/...`
  reads no number, and the key is the stack value that has not been set yet. At most this creates an empty entry; there
  is no further read access.
- Leading zeros (`00103`) are meaningless.
- Duplicate `GUID` or `ACTIONID` blocks would be merged; there are none in the shipped data.

**Data model**: `Map<object_guid, Map<action, List<sound_guid>>>`; the list order is the file order.

**Object GUID** is the type GUID of an object (`OBJ_GUID` of the `.prj`, see [prj.md](prj.md); the same number appears
as `[KEYID n]` in `data/TXT/guids.deu`). For spell sounds (action 30/31) it is the GUID of the spell. The `GUID` line
in object `.cfg` files ([object-cfg.md](object-cfg.md), per animation) has nothing to do with it; in the shipped data no
`.cfg` sets a `GUID` anyway.

### Choosing a sound

`pick(object_guid, action, index)`:

1. If the GUID or the action is missing or the list is empty -> no sound (-1).
2. `index == -1` -> `index = rand()` (MSVC CRT `rand`).
3. Result = list entry `index % count`.

There is **no repeat lock**. The callers almost always pass -1 (random); only unit responses count an index up in the
calling object (one counter for all responses), so responses come in turn; several calls for spell hit/cast (action
31/30) pass index 0 (always the first entry), some combat calls a value from the caller.

### Statistics (shipped data)

| Quantity | Value |
|---|---|
| Lines | 6,856 |
| Object GUIDs | 314 (range 436-4105; 278 of them are `OBJ_GUID` in `nml-wiese.prj`, the other 36 are spells, elk, prairie dog, field, trees, tree stumps and GUID 1406) |
| Distinct sound GUIDs | 1,098, all present in `soundguidlist.txt` |
| Empty lists | 2 (GUID 1313 action 110, GUID 1306 action 100) |
| List lengths | 1: 443, 2: 213, 3: 166, 4: 190, 5-12: 257, 13-14: 72 (all 72 for action 101) |

Actions in the database (number of GUIDs that have them; main folder of the sounds):

| Action | Name (`actionidlist.txt`) | GUIDs | Sounds mainly from |
|---|---|---|---|
| 1 | Schuss normal | 47 | `Waffen Fern` |
| 2 | Einschlag Gebäude/Schiff | 11 | `Waffen Fern` |
| 3 | Einschlag Boden | 8 | `Waffen Fern` |
| 4 | Einschlag Wasser | 8 | `Waffen Fern` |
| 5 | Wurfgeschoss | 8 | `Waffen Fern`, `Einheiten_Sonstiges` |
| 6 | Nahkampf Treffer Einheit | 57 | `Waffen Nah`, `Einheiten_Sonstiges` |
| 7 | Nahkampf Treffer Building | 50 | `Waffen Nah` |
| 10 | Schwimmen | 31 | `Fortbewegung` |
| 11 | Gallopieren/Bewegen | 4 | `Fortbewegung` |
| 12 | Ausführen | 96 | Unit/hero voices (`*_done_*`) |
| 13 | Bremsen | 1 | `Fortbewegung` |
| 14 | Angreifen | 87 | Unit/hero voices (`*_attack_*`) |
| 30 | Zauber Cast | 27 | `Magie` |
| 31 | Zauber Hit | 27 | `Magie` |
| 100 | Select Einheit/Building | 256 | `Gebäude Auswählen`, voices (`*_select_*`) |
| 101 | Sterben/Zerstört | 203 | `Einheiten_Sonstiges`, `Gebäude Zerstört`, `Tier Stirbt` |
| 102 | Brennen | 105 | `Gebäude brennt` |
| 103 | Am Bauen | 110 | `Gebäude errichten`, `Arbeit` |
| 104 | Fertig gebaut/errichtet | 188 | `Einheit erstellt`, `Gebäude steht` |
| 105 | Holzhacken | 12 | `Arbeit` |
| 110 | Tor öffnen/schliessen | 5 | `Gebäude Auswählen` |
| 200 | Brandung | 2 (GUID 4104, 4105) | `Natureffekte` (`NAT_Brandung_*`) |

Actions 15 and 20-27 from `actionidlist.txt` do not occur in the database.

## `soundguidlist.txt`

```
00848=1.0:Data\SFX\Einheiten_WID\WID_Speer_select_05.wav
00058=0.2:Data\SFX\Natureffekte\NAT_Brandung_01.wav
```

Line trimmed on the left; only lines with `=` at a position > 0. Key = `sscanf("%d")` before the `=`,
value = rest of the line (unsorted, the order does not matter). When playing, the value is split at the **first** `:`:

| Part | Meaning |
|---|---|
| before `:` | Volume factor, `sscanf("%f")` |
| after `:` | WAV path (VFS, `\` and case arbitrary) |

The **volume factor only affects 3D sounds**: they pass `ftol(factor * 1000)` to the 3D channel, which scales the
channel volume with it (`* factor`). 2D sounds (selection, responses, events) ignore it. If a sound GUID is missing,
the game logs `Soundguid %d not in db found` and plays nothing; if the file is missing, it plays nothing either.

Statistics: 1,182 entries (IDs 0-1288, unique), all `.wav`. Factors: `1.0` x 1,152, `0.2` x 9 (surf x 5, building
burns x 4), `0.4` x 4 (swimming), 17 further single values between 0.5 and 0.98. 84 entries appear in no database list
(12 of them are used by `otherevents.ini`, 8 by `messages.ini`, 64 stay unused). **23 paths are missing** in the shipped
data (e.g. folder `Gebäude bauen\`, animal sounds, some hero voices; 11 of them are in database lists, e.g. GUID 1201
action 14 five times); on the other hand 11 WAVs without an entry are lying around (variants `*-b.wav`, typos like
`pat_made_attack_06 .wav`). One path is assigned to two IDs.

## `actionidlist.txt`

`<id>=<name>`, empty lines allowed; the same parser as `soundguidlist.txt` (number before `=`, rest as text). The names
serve only as a description; the game uses the numbers fixed in its code.

## `otherevents.ini`

```
SFX_REGEN=895
SFX_WATERWALK0=1177
```

Lines are **not** trimmed; only lines with length > 0 and `=` at a position > 0. Key = text before the `=` (exactly
so, without trimming), value = `atol` of the rest -> sound GUID. `#` comments contain no `=` in the shipped data and
therefore drop out. Lookup is case-sensitive.

Names used in the code (19 entries in the file):

| Name | Sound GUID | Use |
|---|---|---|
| `SFX_REGEN` | 895 | Rain, 2D loop at music volume (weather type 1) |
| `SFX_TEXT` | 921 | Text fade-in |
| `SFX_SIGNAL` | 880 | Minimap ping |
| `SFX_KLICK` | 874 | Click in the UI (several places) |
| `SFX_TEST` | 900 | Test sound when moving the sound volume slider |
| `SFX_HEADHUNTERSTART` | 923 | Headhunter hired |
| `SFX_HEADHUNTERSUCCESS` | 924 | Headhunter successful |
| `SFX_TREEFALL` | 1176 | Tree falls (3D) |
| `SFX_BUILDINGEXPLODE` | 257 | Building explodes (3D) |
| `SFX_WATERWALK0`-`7` | 1177-1180, 871, 1286-1288 | Unit walks through water (3D), `rand() % 8`; 4-7 are silent dummies |

`SFX_CONVERT` (924) and `SFX_WRONG_KLICK` (875) are in the file but are not used by the game.

## `musicconfig.ini`

```
NATION 0
MUSIC data/music/wid.ogg
ATTACK data/music/wid_attack.ogg
/NATION
...
ATTACKSTARTPOINTS 500
```

Lines are **not** trimmed; prefix comparison with the length of the key **including the space**; value = rest of the
line:

| Key | Value | Effect |
|---|---|---|
| `NATION ` | `atol` | Current nation (0 WID, 1 PRI, 2 SPA, 3 SID, 4 ENG, 5 PAT as in `dropdown.ini`) |
| `/NATION` | - (whole line) | End of block (no effect except that the line does not count as a key) |
| `MUSIC ` | Path | Append to the playlist of the current nation |
| `ATTACK ` | Path | Set the combat music of the nation, overwrites - "only one file possible" |
| `ATTACKSTARTPOINTS ` | `atol` | Threshold to start |
| `ATTACKSTARTTIME ` | `atol`, ms | How long the threshold must be reached |
| `ATTACKSTOPPOINTS ` | `atol` | Threshold to end |
| `ATTACKSTOPTIME ` | `atol`, ms | How long it must stay below |
| `ATTACKPAUSE ` | `atol`, ms | Lock after a combat music |
| `ATTACKMAXTIME ` | `atol`, ms | Maximum duration of the combat music |

Shipped data: exactly one `MUSIC` and one `ATTACK` per nation; values 500 / 1000 / 0 / 5000 / 10000 / 720000.
`NATION` only sets an index without a range check (>= 6 writes outside the fields; `MUSIC` before the first `NATION`
uses a pointer value as the index), `/NATION` does not reset it. If a threshold key is missing, the value stays
undetermined (no preset in the constructor).

## `sound.txt`

```
100=data/Music/atmo_karibik.ogg
...
120=data/Music/SPA_ambient.ogg
```

21 entries, IDs 100-120 (all music and ambient titles except `title`, `win`, `lose`, `credits`). Lines are not
trimmed; split at the first `=` (`strchr`), key = `atol` of the part before it (an `=` at position 0 gives key 0),
value = rest of the line untrimmed; lines without `=` drop out, later lines with the same ID replace earlier ones. The
script actions "Spiele ... ab" and "Spiele für Angriff ... ab" of the `.abl` pass this ID.

## `messages.ini` (sound part only)

Per `MSGID` block optionally `SOUND x` or `SOUNDONCE x` (sound GUID). When the message is shown, the GUID is played as
a 2D sound (category SFX); with flag `0x1000` set, the sound is then locked for 15,000 ms per message, with flag
`0x400` it is always played. That `0x1000` = `SOUNDONCE` and `0x400` = `SOUND` follows from the comment in
`messages.ini` ("nur einmal ... danach erst mal ein paar Sekunden nicht"); the flag assignment in the parser has not
been traced. Shipped data: `SOUND` 877/878/879/881/925, `SOUNDONCE` 872/873.

## Audio files

| Location | Count | Format | Duration (min / median / max) |
|---|---|---|---|
| `data/sfx/**/*.wav` (RDA) | 925 | RIFF WAV, PCM 16 bit, **mono**, 44,100 Hz (all) | 0.07 / 1.32 / 9.17 s |
| `data/sprache/helden/*.wav` (RDA) | 244 | as above (hero voices, via `soundguidlist.txt`) | 0.34 / 1.41 / 3.27 s |
| `data/sprache/<text-id>.ogg` (RDA) | 692 | Ogg Vorbis, **mono**, 44,100 Hz, nominal 128 kbit/s | 0.74 / 4.39 / 28.4 s |
| `DATA/MUSIC/*.OGG` (loose, not in the RDA) | 25 | Ogg Vorbis, **stereo**, 44,100 Hz, nominal 128 kbit/s | 32.7 / 243 / 372 s |

812 of the SFX WAVs additionally have a `PAD ` chunk (padding), one each `smpl` or `LIST`. FMOD opens music streams
itself by file name (bypassing the VFS, [rda.md](rda.md)); the game reads speech OGGs through the VFS into memory and
opens them as memory streams; WAVs are read completely through the VFS and loaded as an FMOD sample from memory.

Music titles: `ATMO_KARIBIK`, `ATMO_NORMAL`, `ATMO_WIND_01`, `ATMO_WIND_02`, `ATMO_WINTERNIGHT`, `CREDITS`, `ENG`,
`ENG_AMBIENT`, `ENG_ATTACK`, `IND_AMBIENT`, `LOSE`, `PAT`, `PAT_ATTACK`, `PRI`, `PRI_ATTACK`, `SID`, `SID_AMBIENT`,
`SID_ATTACK`, `SPA`, `SPA_AMBIENT`, `SPA_ATTACK`, `TITLE`, `WID`, `WID_ATTACK`, `WIN`.

The statistics were measured from the headers (RIFF or Vorbis identification packet; duration from the `data` size or
the last granule position).
