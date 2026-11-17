# Menu dialogs `.dlg` (with `.bkg` and `dialogs.map`)

Word-oriented text file (cp1252/Latin-1, CRLF) that describes a menu screen as a tree of elements:
`FRAME` (window) with `BUTTON`, `LABEL`, `COMBOBOX`, `LISTBOX`, `EDITFIELD`, `SLIDER`, `RADIOBUTTON` and
nested `FRAME`s, plus a small macro language (`VAR`, `ADD`, `LOOP`). Frame and tile graphics are in `.bkg` files,
the assignment dialog name -> file is in `dialogs.map`.

Scope of this document: grammar, meaning of every keyword, coordinates, references, `.bkg`, `.map`. Not covered:
the run time of the widgets (events, drawing, modality, sounds), which screen is opened when, the dialog stack and
the logic per dialog.

## Locations

- 63 `.dlg` under `data/menü/`, 11 `.bkg` under `data/menü/`, plus `data/gifcon/editfield.bkg` and
  `data/gifcon/infoframe.bkg`; `data/menü/dialogs.map`, `data/menü/dialogs2.map`. Both maps are only in
  `nml9.rda`.
- The program loads **only** `data/menü/dialogs.map` once, when the menu system is created.
  `dialogs2.map` is never referenced. There is
  therefore no condition for `dialogs2.map`: the four variants `start2.dlg`, `options2.dlg`, `selectcampaign2.dlg`,
  `singleplayermenu2.dlg` are dead in the shipped game.

## Loading

The menu system creates a root widget (1280 x 1024 grid) and reads `dialogs.map` (word pairs `key path`). To fetch a
dialog, the name is uppercased and looked up in the name table; if it is missing and loading is allowed, the path is
taken from the map, the file is read and the lookup is repeated. Reading a file starts the child parser with the
root widget as the parent; the caller does **not** check its result. The reader state consists of a line buffer, a line
counter (only counted, never output), a name prefix (name of the `FRAME` begun last), the name table (name -> widget),
the map, the variables (`VAR`/`ADD`) and the loop stack.

## Lexical rules

- **Lines:** reading a line goes up to `\n` and drops every `\r`; the last line counts even without a line ending.
  There is no length limit.
- **Words:** the tokenizer cuts white space off on the left; if the buffer is empty, it reads lines until one is not
  empty after trimming on both sides. The word extends up to the first space or tab; the rest stays in the buffer.
  Line endings are therefore **meaningless**: `FLAGS ... /FLAGS` may span several lines (corpus: `loadnetwork.dlg`).
- **Uppercasing:** every word is uppercased before it is returned (`a`-`z` and `ä ö ü` -> `Ä Ö Ü`, `ß` stays). All
  comparisons afterwards are `_mbscmp`-like, i.e. in effect **without** regard to case. Names, variables and **paths**
  also arrive uppercased; the file lookup lowercases ([rda.md](rda.md)), so they work.
- **Comments:** none. `//`, `#` or `;` are ordinary words; a word with `/` at the start ends the child area (see
  "Error behavior"). No comments occur in the corpus.
- **Paths** are a single word: spaces in paths are not possible.
- **Umlauts in paths:** cp1252 (`ü` = `0xFC`), `0xDC` after uppercasing; the file lookup maps `Ä Ö Ü` back to `ä ö ü`.

### Values

| Type | Rule |
|---|---|
| Number expression | Parts separated by `+`, sum. Each part is looked up in the variable table first, otherwise read with `atol`. No `-` as an operator: `YPOS-20` is an unknown name, `atol` yields 0. Negative numbers as a part work (`-440`, `A+-5`) |
| Hex color | `sscanf("%x")`: `AARRGGBB`; a `0x` prefix is accepted (corpus: 4 files) |
| Text ID | Number expression, looked up in the text table. If the ID is missing, the number itself appears as the text (`swprintf("%d")`). The descriptor receives a pointer to the text, not the ID |
| Word | Name or path, one word, uppercased |

## Layout

### Child area

The file itself is a child area with the root widget as the parent. It reads words until one starts with `/`
(push it back, success) or the file ends (success):

| Word | Effect |
|---|---|
| `LOOP n` | pushes `{i=0, n, file position after the word n}` onto the loop stack |
| `/LOOP` | take the top element; `i+1 < n` -> set the file to the remembered position and push `{i+1,n,pos}` back; empty stack -> **error** |
| `VAR name expression` | set a variable |
| `ADD name expression` | increase a variable by the expression; unknown variable -> **error** |
| `COMBOBOX`, `LISTBOX`, `EDITFIELD`, `BUTTON`, `RADIOBUTTON`, `FRAME`, `SLIDER`, `LABEL` | element parser |
| word with `/` at the start | push back, success |
| otherwise | **error** |

A `LOOP` body runs `max(n, 1)` times. The repetition jumps back in the **file**, not in the word stream.

### Elements

All eight element parsers follow the same scheme:

```
ELEMENT name
    FLAGS word... /FLAGS        (any number of times, also across lines)
    KEY arguments...            (head: fills the descriptor)
    children...                 (elements, VAR, ADD, LOOP - allowed for every element kind)
/ELEMENT
```

1. A local **descriptor** is created with defaults, then the name is read.
2. The loop reads words. `FLAGS` switches to flag mode, `/FLAGS` back; in flag mode every unknown word is an error
   (also `EXTENT`). Head keys fill the descriptor; a key may appear several times, the last one wins.
3. At the **first word that is neither a flag nor a head key nor an end word**, the widget is built from the
   descriptor, attached to the parent widget and entered into the name table; then the word is pushed back and the
   child area is read with the new widget as the parent. When it returns, the loop continues.
4. At the **end word** (`/FRAME`, `/BUTTON` ...) the widget is built if that has not happened yet; success.
5. End of file in the loop: error (the widget that has already been built stays attached).

Consequence: after the first child (or after a `VAR`/`ADD`/`LOOP` in the element) the **child area** reads until a
`/...` word comes. A head key there is an unknown word for it: **reading the file aborts** (error, see "Error
behavior"), also for `PRELOAD`, `LEVELBACKGROUND` and `BACKGROUND`. Back in the element parser, a `/...` word is always
pending; head keys therefore never reach the element parser after building.

### Names

- `FRAME`: the name is read directly and entered **without** a prefix; it also becomes the new prefix.
- other elements: the name is read and, for **every active loop** from the outside in, `"%d"` of the counter is
  appended; `prefix + name` is entered (e.g. `ACCOUNT0` + `VALUE` + `1` + `7`). Multi-digit counters are not
  delimited (`VALUE1`+`11` = `VALUE11`+`1`).
- Equal keys silently overwrite. The table is global over all loaded dialogs.
- A dialog is looked up through this table (all entries, also elements); the `FRAME` whose name equals the
  `dialogs.map` key is found.

### Error behavior

- Every error only passes `false` upward, without a message (no error strings; the line counter is never read). The
  caller does not evaluate the result: what has been built and entered up to the error stays and is shown if the
  `FRAME` had already been entered (that happens at the first child).
- **Missing end word** (`/BUTTON` missing): the next foreign end word, e.g. `/FRAME`, counts in the element as an
  "unknown word" -> build the widget, push back, the child area sees `/...`, pushes back, returns -> the same word
  again: **endless loop**. The same applies to `/FLAGS` without `FLAGS`.
- A `/...` word at the top level ends reading the file; the rest is ignored.
- Unknown word in the child area, unknown flag, `ADD` on an unknown variable, `/LOOP` without `LOOP`: abort of the
  whole file.
- A `FRAME` clears the variables and the loop stack at its start; the prefix is not reset after a nested `FRAME`.

## Element kinds

"-" = default 0 or empty. Every kind additionally knows the flags `HIDE` = `0x10000000` and `DISABLE` =
`0x20000000`; both are also run-time state that the dialog logic sets through `status`.

### `FRAME`

With `MENUFRAME_MODAL`, a modal blocker widget is additionally attached to the root and the frame becomes its child.

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `EXTENT` | 4 numbers | 0 | Window rectangle x1 y1 x2 y2 (start position of the animation) |
| `ZIELPOSITION` | 2 numbers | 0, 0 | End position x1 y1 of the fly-in animation |
| `DURATION` | 1 number | 1000 | Animation duration in ms |
| `BAREXTENT` | 4 numbers | 0 | Rectangle of the decorative bar (absolute, see coordinates) |
| `TITLECOLOR` | hex | `0xFFFFFFFF` | Title color |
| `TITLEEXTENT` | 4 numbers | 0 | Title rectangle (absolute) |
| `TITLE` | text ID | - | Title text |
| `BACKGROUND` | word | - | 2D background image, only with a flag (see below) |
| `LEVELBACKGROUND` | word | - | `.lvl` as a 3D background (shares its field with `BACKGROUND`) |
| `MUSIC` | word | - | Music file |
| `BKGFILE` | word | - | own `.bkg` instead of `menuframe.bkg` |
| `PRELOAD` | word | - | Load a texture immediately, any number of times |

| Flag | Bit | Effect (run time) |
|---|---|---|
| `MENUFRAME_BACKGROUND` | `0x1` | Crossfade the 2D background `BACKGROUND` (broadcast `0x15E`) |
| `MENUFRAME_DRAWTAGS` | `0x2` | Two decorative pieces above the top edge (tile 8) |
| `MENUFRAME_DRAWBARL` | `0x4` | Bar in `BAREXTENT` |
| `MENUFRAME_DRAWTITLEL` | `0x8` | Title in `TITLEEXTENT` |
| `MENUFRAME_DRAWBARR` | `0x10` | Bar, mirrored variant |
| `MENUFRAME_DRAWTITLER` | `0x20` | Title, mirrored variant |
| `MENUFRAME_MUSIC` | `0x40` | Play `MUSIC` |
| `MENUFRAME_LEVELBACKGROUND` | `0x80` | 3D level as the background, `levelbackground.bkg` as an overlay |
| `MENUFRAME_MODAL` | `0x200` | Modal (blocker) |
| `MENUFRAME_NOFRAME` | `0x400` | No `.bkg` frame, no client inset |
| `MENUFRAME_DONOTLOOPMUSIC` | `0x800` | Music without a loop |
| `MENUFRAME_MOVE` | `0x1000` | Fly-in from `EXTENT` to `ZIELPOSITION` (no moving with the mouse) |
| `MENUFRAME_DONOTCLEARBACKGROUND` | `0x100000` | Do not clear the desktop background when opening |
| (internal) | `0x100` | Set by the constructor if a title flag (`0x28`) is present without a bar flag (`0x14`): `0x28` is cleared, title without a bar and without an animation offset |

**Background keys act while reading and depend on the order** (flags must come first; in the corpus `FLAGS` always
comes first):

- `LEVELBACKGROUND p`: only if `Engine.ini` `IsMenuLevel` is set (installation: 1): store `p`; if bit `0x80` is
  already set, application command `0x21` with the path as a 32-bit string is sent immediately to the application.
  Without `IsMenuLevel` the word is skipped. Command `0x21` hands the path on: without a running game the level is
  started as a menu level (start parameter game type -2), otherwise only the "draw world" bit is set; command `0x20`
  clears this bit.
- `BACKGROUND p`: if (`0x80` and not `IsMenuLevel`) or `0x1` is set -> set bit `0x1`, store `p`, clear bit `0x80`, send
  command `{0x20, 0}` (3D background off). Otherwise, without `0x80`: only send `{0x20, 0}`, **path discarded**. With
  `0x80` and `IsMenuLevel`: nothing (the `.lvl` stays stored).
- Result: with `IsMenuLevel`, a frame with `MENUFRAME_LEVELBACKGROUND` shows the level, otherwise it becomes a
  `MENUFRAME_BACKGROUND` frame with the `BACKGROUND` image.

Run-time part: the `.bkg` is loaded without `NOFRAME` (`BKGFILE` or `data/menü/menuframe.bkg`); decorative tiles
`data/menü/zusatz_85x85.tga` and `..._gespiegelt.tga` with flags `& 0x13E`; font `MenuTitle` with flags `& 0x128`;
on attaching: music broadcast `{0x160, 2, MUSIC, !DONOTLOOPMUSIC}`, with `0x80` `levelbackground.bkg` into the top
widget, otherwise without `0x100000` clear the desktop background.

### `BUTTON`

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `EXTENT` | 4 numbers | 0 | Rectangle x1 y1 x2 y2 |
| `BORDERSPACE` | 4 numbers | 0 | Inset left, top, right, bottom for the state background |
| `TEXTCOLOR` | hex | `0xFFFFFFFF` | normal |
| `DISABLEDTEXTCOLOR` | hex | `0xFFB0B0B0` | disabled |
| `MOUSEOVERTEXTCOLOR` | hex | **none** | mouse over |
| `PRESSEDBUTTONTEXTCOLOR` | hex | **none** | pressed |
| `TEXT` | text ID | - | Caption |

| Flag | Bit | Effect |
|---|---|---|
| `MENUBUTTON_TEXT` | `0x1` | Draw text |
| `MENUBUTTON_BORDER` | `0x2` | First `button_border.bkg` over the whole rectangle, then the state in the inset rectangle |
| `MENUBUTTON_SELECTABLE` | `0x4` | no evaluation found |
| `MENUBUTTON_SELECTED` | `0x8` | no evaluation found |
| `MENUBUTTON_CHECKESCAPE` | `0x10000` | ESC triggers the button |
| `MENUBUTTON_CHECKENTER` | `0x20000` | ENTER triggers the button |

The constructor always loads `button_normal/_mouseover/_pressed/_disabled/_border.bkg` (state 0...4).

### `COMBOBOX`

Not a type of its own: a `BUTTON` widget (same descriptor, same widget) with bit `0x10` in the flags and an embedded
`LISTBOX` descriptor. The parser fills a list descriptor of its own and copies it into the button before building.
The list is created only when it is opened.

| Key | Arguments | Target | Default |
|---|---|---|---|
| `BUTTONEXTENT` | 4 numbers | Button | 0 |
| `LISTBOXEXTENT` | 4 numbers | List | 0 |
| `TEXTCOLOR` | hex | Button **and** list | `0xFFFFFFFF` / `0xFFB0B0B0` |
| `DISABLEDTEXTCOLOR` | hex | Button only | `0xFFB0B0B0` |
| `SELECTIONBARCOLOR` | hex | List | `0xFF00A000` |
| `MOUSEOVERCOLOR`, `MOUSEOVERTEXTCOLOR` | hex | both: list **and** button | `0xFFFFFFFF` / none |
| `PRESSEDBUTTONTEXTCOLOR` | hex | Button | **none** |
| `SELECTEDTEXTCOLOR` | hex | List | `0xFF00FF00` |
| `TEXT` | text ID | Button | - |
| `CURSELECTION` | number | List | -1 (selected entry, -1 = none) |

Flags (button): `MENUCOMBOBOX_TEXT` `0x1`, `MENUCOMBOBOX_SELECTABLE` `0x4`, `MENUCOMBOBOX_SELECTED` `0x8`. List flags
(`LISTBOX_AUTOYSIZE`) cannot be set here. The entries come from the dialog logic at run time (parameter `array`), not
from the file.

### `LISTBOX`

Always loads `comboframe.bkg`.

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `EXTENT` | 4 numbers | 0 | |
| `TEXTCOLOR` | hex | `0xFFB0B0B0` | |
| `DISABLEDTEXTCOLOR` | hex | `0xFFA0A0A0` | |
| `SELECTIONBARCOLOR` | hex | `0xFF00A000` | 0 = no bar |
| `MOUSEOVERCOLOR` | hex | `0xFFFFFFFF` | |
| `SELECTEDTEXTCOLOR` | hex | `0xFF00FF00` | |
| `CURSELECTION` | number | -1 | The bar only appears for the row equal to the value |

Flags: `LISTBOX_AUTOYSIZE` `0x100` (effect not examined). No `TEXT`.

### `EDITFIELD`

Always loads `data/gifcon/editfield.bkg`.

| Key | Arguments | Default |
|---|---|---|
| `EXTENT` | 4 numbers | 0 |
| `TEXTCOLOR` | hex | `0xFFFFFFFF` |
| `DISABLEDTEXTCOLOR` | hex | `0xFFB0B0B0` |
| `TEXT` | text ID | - |

Flags: `KEYNAME` `0x1`: key capture - with this bit, the key handling takes the name of the pressed key combination
instead of a character (corpus: key bindings in `controloptions.dlg`).

### `SLIDER`

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `EXTENT` | 4 numbers | 0 | |
| `CURVALUE` | number | 0 | Step index `0 ... RESOLUTION-1`, not limited when reading |
| `RESOLUTION` | number | **10** | Number of steps; < 2 -> nothing is drawn |
| `TEXTCOLOR` | hex | `0xFFFFFFFF` | |
| `DISABLEDTEXTCOLOR` | hex | `0xFFB0B0B0` | |
| `THUMBCOLOR` | hex | `0xFFFFFFFF` | Thumb |
| `SCALECOLOR` | hex | `0x80808080` | Scale marks |
| `TEXT` | text ID | - | |

Flags: `MENUSLIDER_TEXT` `0x2`, `MENUSLIDER_SCALE` `0x4`, `MENUSLIDER_VERTICAL` `0x8` (loads `slider_vert.bkg` instead
of `slider_horz.bkg`), `MENUSLIDER_CENTERTEXT` `0x10`. Step width `(length - 84) / (RESOLUTION - 1)` as a `float`,
thumb 42x42.

### `RADIOBUTTON`

Image `data/menü/buttons_51x51_02.tga`.

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `EXTENT` | 4 numbers | 0 | |
| `ICONOFFSET` | number | 0 | Tile index of the icon, +1 if checked |
| `TEXT` | text ID | - | |
| `TEXTCOLOR` | hex | `0xFFFFFFFF` | |
| `DISABLEDTEXTCOLOR` | hex | `0xFFB0B0B0` | read, never used when drawing |

Flags: `RADIOBUTTON_TEXT` `0x1`, `RADIOBUTTON_CHECKED` `0x4`. The widget is a toggle without group logic.

### `LABEL`

The descriptor has a different layout from the others (flags first).

| Key | Arguments | Default | Meaning |
|---|---|---|---|
| `CREDITS` | word (path) | - | Also sets bit `0x8`: credits from a text file |
| `EXTENT` | 4 numbers | 0 | |
| `TEXTCOLOR` | hex | `0xFFFFFFFF` | |
| `BACKGROUNDCOLOR` | hex | **none** | Used only with `MENULABEL_BACKGROUND` |
| `DISABLEDTEXTCOLOR` | hex | `0xFFB0B0B0` | no use found |
| `TEXT` | text ID | - | |

| Flag | Bit | Effect |
|---|---|---|
| `MENULABEL_BACKGROUND` | `0x1` | Fill the rectangle in `BACKGROUNDCOLOR` |
| `MENULABEL_TEXTARRAY` | `0x2` | Text = concatenation of a text field from the dialog logic |
| `MENULABEL_AUTOHSCROLL` | `0x4` | Wrap width -42 px; no further effect found |
| (`CREDITS`) | `0x8` | Credits mode |
| `MENULABEL_NOLINEBREAK` | `0x10` | No line break |
| `MENULABEL_TGA` | `0x20` | Image instead of text; the path comes from the logic at run time (key `tga`), stretched to `EXTENT` |

**Credits file** (`CREDITS`, corpus `data/txt/credits.txt`): line by line; a line `[CHANGEBACKGROUND name]` notes a
background change at the current height (not displayed); empty line -> `[CR]`; otherwise text (<= 256 characters),
wrapped to the label width. Font `Credits`, color `0xC8FFFFFF`. Run time: at least 120 s. The label sends the first
image change as `0x15E` to the desktop already when it is attached.

## Coordinates

- **Virtual grid 1280x1024**, independent of the screen resolution. Each widget carries the real size (default
  1280/1024, set from the back buffer size).
- **`EXTENT x1 y1 x2 y2`**: corners, not width/height; swapped corners are swapped back. The reference is the **top
  left corner of the client area of the parent widget** (the position sums `x1 + client.left` over the parents). The
  client area is the rectangle minus `CLIENTAREA` of the `.bkg` of the parent widget: in a frame with `menuframe.bkg`
  (`CLIENTAREA 15 30 15 30`), `EXTENT 0 0 ...` of a button therefore lies at frame x1 + 15, frame y1 + 30. With
  `MENUFRAME_NOFRAME` there is no inset. Top frames hang on the root (0,0-1280,1024) or on the modal blocker (the same
  size): their `EXTENT` is absolute.
- **Conversion to the screen** (same formula for rectangles and for points or sizes), integer:
  `xs = (B*x + 640) / 1280`, `ys = (H*y + 512) / 1024`, both C division towards 0 (the division by 1024
  rounds toward zero, i.e. **not** `floor`), so rounded for positive values. Inverse:
  `x = (xs*1280 + B/2) / B`. Every rectangle is first formed absolutely in the grid and then converted as a whole.
  Font sizes: `RelativeFontSize n` gives `trunc(B * n / 1280)` pixels, B = back buffer width at program start; atlas
  `data/fonts/<FontName>_<px>_<weight>_<i>_<u>_<s>.fnt`. Not to be confused with the conversion of the game windows
  (messages, cinemascope): there `B * x / 1280` and `(H * y) >> 10` **without** the rounding addend.
- **`ZIELPOSITION x y` / `DURATION ms`** (only with `MENUFRAME_MOVE`): opening moves the window linearly from
  (`EXTENT.x1`, `EXTENT.y1`) to (`ZIELPOSITION`) in `DURATION` ms, closing moves it back
  (tick: `x = target + remaining*(start - target)/duration`, likewise y; the size stays). The clock is
  `GetTickCount`. Without `ZIELPOSITION` the target is (0,0).
- **`BAREXTENT` / `TITLEEXTENT`**: **absolute** grid coordinates, not relative to the frame. The offset glides in the
  first half of opening from `(start.x - target.x)` to 0, in the second half of closing from 0 to
  `-(start.x - target.x)`; after closing `(0, -500)`. Bar: square tiles with an edge length equal to the bar height
  (screen pixels), end pieces tile 0 and 2, middle tile 1. Title: tiles 3...7, text `MenuTitle` centered.
- **`DRAWTAGS`**: two decorative pieces in the frame reference `(x1+55, y1-60, x1+110, y1+25)` and
  `(x2-110, y1-60, x2-55, y1+25)`.
- **`LISTBOXEXTENT`** (combobox): same reference as `BUTTONEXTENT`; opening attaches the list to the root and moves
  it by the absolute position (corpus: same x range as the button).
- **`BORDERSPACE l t r b`**: pixels in the grid, left/top/right/bottom.

## References

| Reference | Target | Resolution |
|---|---|---|
| `TEXT`, `TITLE` | Text table | Files from `data/txt/localisation.<language>` (lines `TextDatei path`); a file only with BOM `FF FE` (UTF-16LE), lines `number=text`, number read with `swscanf`, a later equal number replaces. German: `extrasymbols.txt`, `guids.deu`, `texte.deu`, `dialogtexte.deu`, `extendedtooltips.deu`, `keyboardlayout.deu` |
| `BACKGROUND`, `PRELOAD` | `.tga` | Texture cache of the renderer |
| `LEVELBACKGROUND` | `.lvl` ([lvl.md](lvl.md)) | Application command `0x21`; only with `IsMenuLevel` |
| `MUSIC` | `.ogg` | Broadcast `0x160` -> desktop -> menu music |
| `BKGFILE` | `.bkg` | instead of `data/menü/menuframe.bkg` |
| `CREDITS` | Text file | own line format (above) |
| Fonts | `localisation.*` (`Font name`) | **fixed in the code**: `MenuTitle` (frame title), `MenuButton` (all other elements), `Credits` (credits); `MenuComboBox`, `CreditsTitle` are never used |
| fixed graphics | in the code | `button_*.bkg`, `comboframe.bkg`, `editfield.bkg`, `slider_*.bkg`, `slider_42x42.tga`, `buttons_13x13_normal.tga`, `buttons_51x51_02.tga`, `zusatz_85x85*.tga`, `menuframe.bkg`, `levelbackground.bkg` |

`RESOLUTION` (slider) and `KEYNAME` (edit field flag) are not references, see above. `ADD`/`VAR`/`LOOP` are pure
read-time macros; combobox and list entries never come from the file.

## .bkg

Line-oriented, **unlike `.dlg`**: the keyword must be in column 0, the comparison is on the prefix (case matters),
arguments are read with `sscanf` starting exactly one character after the word; empty lines, comments and unknown
lines silently fall through. Checking order: `ALPHATEST`, `FILE`, `TILE`, `NEWLINE`, `TOPLEFT` ... `FILLTILE`,
`CLIENTAREA`, `FITSIZETOBACKGROUND`, `SETTOPLEFTPOS`, `SETBOTTOMRIGHTPOS`, `SETSIZE`, `STRECHTOBG`. If the file is
missing: return 0, no message, the widget has no background. The executable has no writer.

| Line | Arguments | Effect |
|---|---|---|
| `FILE path w h` | `"FILE %s %d %d"` | Texture with tile size w x h; index = order of the `FILE` lines. Tiles row by row in the grid `TexW/w` x `TexH/h` |
| `TILE f k` | `"TILE %d %d"` | Tile k of file f appended to the tile sequence (`(k<<16)\|f`) |
| `NEWLINE` | - | Line break in the tile sequence |
| `TOPLEFT`, `TOPRIGHT`, `BOTTOMLEFT`, `BOTTOMRIGHT`, `LEFTMID`, `RIGHTMID`, `TOPMID`, `BOTTOMMID`, `FILLTILE` | `f k` | 9-part table (default -1 = missing) |
| `CLIENTAREA l t r b` | 4 int | Client inset left/top/right/bottom |
| `FITSIZETOBACKGROUND` | - | Widget size = extent of the `TILE` lines so far |
| `SETTOPLEFTPOS x y`, `SETBOTTOMRIGHTPOS x y`, `SETSIZE w h` | 2 int | Set position/size of the widget (`SETSIZE` without recomputing the client area) |
| `STRECHTOBG` | - | Stretch every `TILE` tile to the whole widget |
| `ALPHATEST` | - | Hit test pixel-exact over the `TILE` tiles |

- **Drawing:** first the tile sequence from the top left (tiles in screen size), then the 9-part table: corners in
  tile size of the texture of `TOPLEFT`, edges and fill **tiled**, the last tile clipped (through UV); if the
  rectangle is too small, left/top win. Missing parts (-1) are omitted; this way `slider_horz.bkg` (TL, TOPMID, TR) and
  `slider_vert.bkg` (TL, LEFTMID, BL) yield a bar.
- **Several files per widget** (button: five) are collected as sets 0...n; set 0 is active.
- Paths in `.bkg` are **not** uppercased; the texture cache compares paths case-sensitively and, on a hit, keeps the
  tile size given first.
- **Missing images:** every `FILE` line creates an image list entry with path and tile size (it can fail only on
  lack of memory; every `FILE` line is counted); the texture is loaded only when drawing. If the image is missing,
  only the tiles of this file draw nothing; the file indices of the others do not shift, and the tile size of an
  imageless `TOPLEFT` file continues to apply to the corners.
- **Tile sequence packed:** `TILE f k` becomes `(k << 16) | (f & 0xFFFF)`; when drawing, `k = 0xFFFF` is the mark of
  `NEWLINE`, so `TILE f 65535` or `TILE f -1` acts as a line break, and the file counts only with its lower 16 bits.
  A `NEWLINE` advances by the height of the tile set last, two in a row therefore twice.
- **Reimplementation** (`nml-ui/src/bkg_paint.rs`): where the original aborts or divides by 0 (9-part table without
  `TOPLEFT` or with a `TOPLEFT` file outside the `FILE` lines; tile size <= 0), it omits the 9 parts or the tile (not
  present in the shipped data, O-88).

## `dialogs.map`

Word pairs `KEY path` through the `.dlg` tokenizer (so uppercased, lines and tabs do not matter, a last word left over
is discarded) into a string map; duplicate keys overwrite. The key is the dialog name that the logic opens (callers
write it in lowercase, the lookup uppercases it) and that must equal the `FRAME` name in the file. The spelling
`LoadNetWork.dlg` is therefore meaningless. `dialogs2.map` has the same format and differs in four paths (`START`,
`OPTIONS`, `SELECTCAMPAIGN`, `SINGLEPLAYERMENU` -> `...2.dlg`), but is never read (see Locations).

## Corpus finding

A checker that reproduces tokenizer, child area, element parser, number expressions, loops, `.bkg` and map reader
(text IDs from the six `TextDatei` tables, 4157 IDs):

| Check | Result |
|---|---|
| Files | 63 `.dlg` (exactly one `FRAME` each, none nested, no element outside), 13 `.bkg`, 2 maps |
| Elements after loop resolution | 63 FRAME, 141 BUTTON, 114 COMBOBOX, 250 LABEL, 58 RADIOBUTTON, 18 SLIDER, 17 EDITFIELD, 15 LISTBOX |
| Macros (source text) | 13 `LOOP`, 54 `VAR`, 55 `ADD` |
| Load errors, unknown words/flags, missing end words, head keys after children, unresolvable number parts | 0 |
| Text IDs | 408 uses (after resolution), all present in the tables |
| Missing files from `.dlg` (`BACKGROUND` 22, `PRELOAD` 10, `LEVELBACKGROUND` 4, `MUSIC` 5, `BKGFILE` 1, `CREDITS` 1) | 0 |
| `.bkg`: unknown lines, missing textures, invalid file/tile indices | 0 |
| Colors with a `0x` prefix | 4 (valid) |
| `dialogs.map` | 60 entries, 0 duplicates; **1 file missing** (`GAMEPLAYOPTIONS3` -> `gameplayoptions3.dlg`); all other keys = `FRAME` name |
| `dialogs2.map` | 60 entries, 4 different paths, the same missing file; 4 `.dlg` reachable only through it |
| Name key duplicated within a file | 33 (silently overwrite) |
| `COMBOBOX` without `PRESSEDBUTTONTEXTCOLOR` | 114 (all; field without a default) |
| `BUTTON` without `MOUSEOVERTEXTCOLOR`/`PRESSEDBUTTONTEXTCOLOR` | 2 (`singleplayer.dlg`) |
| Title flag without a bar flag (-> bit `0x100`) | 15 |
| `MENUFRAME_LEVELBACKGROUND` without its own `LEVELBACKGROUND`/`BACKGROUND` | 5 (overlay of the level that is already running; without `IsMenuLevel` no 2D substitute) |
| `BACKGROUND` without a matching flag (path discarded) | 1 (`credits.dlg`; image change there through `[CHANGEBACKGROUND]`) |

## Quirks

1. All words are uppercased; keywords, names, variables and paths are therefore case-insensitive, but case-sensitive
   in `.bkg`.
2. Line endings are meaningless in `.dlg`/`.map`, decisive in `.bkg` (column 0, prefix comparison).
3. No comments; a word with `/` at the start ends the child area.
4. Number expressions know only `+`; `YPOS-20` silently becomes 0. A `+` at the start of an expression (`+5`) makes the
   evaluation run endlessly (the division at position 0 does not shorten).
5. Unknown text IDs appear as a number - formatted into **one** shared buffer whose pointer ends up in the
   descriptor: several missing IDs all show the number formatted last (no missing ID in the corpus).
6. A missing end word or a foreign `/...` word in an element leads to an endless loop when loading.
7. Errors are not reported; a partially read dialog is still used if its `FRAME` had already been entered.
8. Head keys after the first child abort reading the file (the child area does not know them), also `PRELOAD`,
   `LEVELBACKGROUND` and `BACKGROUND`.
9. `LOOP 0` and `LOOP 1` each run the body once; the repetition jumps in the file, the rest of the `LOOP` line is not
   repeated, the rest of the `/LOOP` line (line buffer) is read first after the jump back.
10. Loop counters are appended to names without a separator; from ten passes on, names are ambiguous.
11. A `FRAME` clears variables and the loop stack; a nested `FRAME` in a loop breaks it, and the prefix stays the
    name of the inner `FRAME`.
12. `BACKGROUND` and `LEVELBACKGROUND` share a field; their effect depends on the order relative to `FLAGS` and on
    `IsMenuLevel`; `BACKGROUND` without a matching flag discards the path but sends "3D background off".
13. `BAREXTENT`/`TITLEEXTENT` are absolute, `EXTENT` is relative to the client area; `TITLEEXTENT` of `START`
    (`-490 ... -50`) lies outside the image on the left, the title is never visible.
14. `MENUFRAME_MOVE` without `ZIELPOSITION` flies to (0,0) (not present in the corpus).
15. Fields without a default (stack garbage if the key is missing): `MOUSEOVERTEXTCOLOR`, `PRESSEDBUTTONTEXTCOLOR`
    (button, combobox), `BACKGROUNDCOLOR` (label).
16. Combobox: `TEXTCOLOR` and `MOUSEOVERCOLOR`/`MOUSEOVERTEXTCOLOR` write the button and the list field;
    `DISABLEDTEXTCOLOR` only the button; list flags cannot be set.
17. Read without effect: `DISABLEDTEXTCOLOR` for `RADIOBUTTON` and `LABEL`, flags `MENUBUTTON_SELECTABLE`/
    `_SELECTED`; `ALPHATEST` in `button_border.bkg` (only set 0 is active, and the hit test only over `TILE`).
18. The `.bkg` corners take their size from the texture of `TOPLEFT`; the `BOTTOMLEFT` target and the `TOPMID` tile
    use the respective other edge length (no effect with square tiles).
19. The texture cache is case-sensitive (`rahmen_21x21.tga` from `menuframe.bkg` and `Rahmen_21x21.tga` from
    `editfield.bkg` are two entries).
20. `dialogs2.map` and four `...2.dlg` are never loaded; `dialogs.map` names the missing `gameplayoptions3.dlg`
    (opening fails silently).

## Further details of the reader

- **Head keys after the first child:** every word in the child area that is neither a macro nor an element kind and
  does not start with `/` makes the child area return failure (the word is consumed). The element parsers pass this
  on. A head key after the first child therefore aborts the file (this does not occur in the corpus).
- **Loop stack:** it is a stack (appended at the back, taken from the back); name counters run from the outside in;
  `LOOP` remembers the position after evaluating `n`, `/LOOP` only resets the file, the line buffer stays. `ADD` on an
  unknown variable aborts **before** the expression is read. `FRAME` clears loops and variables before reading the name.
- **Number expression:** `+` at position 0 of the rest leads to the endless loop - so also `A++5`, not only a leading
  `+`; an empty part (`A+`) is 0. `atol` is the VS 2003 version: white space, a sign, digits, overflow without
  limitation (32 bit).
- **Colors:** `sscanf("%x")` writes the field only if it reads a digit; otherwise the previous value stays (default or
  earlier key). In the combobox, `TEXTCOLOR` reads into the button and then copies into the list,
  `MOUSEOVERCOLOR`/`MOUSEOVERTEXTCOLOR` read into the list and copy into the button - even if reading fails.
- **No effect in the shipped data:** if only the end word `/FRAME` builds the widget (frame without children), it is
  attached directly to the parent, without the modal blocker for `MENUFRAME_MODAL` (only the way through the first
  child checks `0x200`). A childless modal frame is therefore not modal.

## Open questions

- `LISTBOX_AUTOYSIZE`, `MENULABEL_AUTOHSCROLL` (apart from -42 px): run-time effect not examined.
- Who requests `GAMEPLAYOPTIONS3`.

## Proposed Rust data structure (`nml-formats`)

Goal: read and write losslessly (a later editor) **and** the same meaning as the original. Hence two levels:

1. **Syntax tree with trivia** (`DlgDocument`): unevaluated macros, original spelling, white space.

   ```rust
   pub struct DlgDocument { pub items: Vec<Item>, pub trailing: Trivia }
   pub enum Item {
       Element(Element),
       Var { name: Word, value: Expr },
       Add { name: Word, value: Expr },
       Loop { count: Expr, body: Vec<Item>, end: Word },
   }
   pub struct Element {
       pub kind: ElementKind,          // Frame, Button, ComboBox, ListBox, EditField, Slider, RadioButton, Label
       pub keyword: Word, pub name: Word,
       pub head: Vec<HeadItem>,        // order preserved
       pub children: Vec<Item>,
       pub inert: Vec<HeadItem>,       // head keys after the first child (original: abort; preserved in the tree)
       pub end: Word,
   }
   pub enum HeadItem { Flags { open: Word, words: Vec<Word>, close: Word }, Prop { key: Word, args: Vec<Arg> } }
   pub enum Arg { Expr(Expr), Color(Word), Path(Word) }
   pub struct Expr { pub terms: Vec<Term>, pub raw: Word }   // Term::Number(i32) | Term::Var(String)
   pub struct Word { pub text: String, pub leading: Trivia } // original text (Latin-1), white space before
   ```

   Writing returns the bytes exactly for unchanged nodes (Latin-1, CRLF); new nodes get the house formatting (a tab
   per level, one key per line, `FLAGS ... /FLAGS` on one line). Unknown flags are preserved as a `Word`, so that the
   editor can open faulty files (`Diagnostic` instead of an abort).
2. **Evaluated description** (`expand(&DlgDocument) -> DialogDesc`): loops resolved, variables evaluated, names =
   prefix + name + counter, text IDs as `i32`, fields typed with the defaults of the original (`Option<u32>` for fields
   without a default), flags as bit fields per kind (`FrameFlags`, `ButtonFlags` ...), combobox as a button with
   `ListDesc`. The evaluation reproduces items 4 and 8 to 12 of the quirk list above (clear variables per `FRAME`, `LOOP
   0` = once, background logic with the parameter `is_menu_level`).

In addition `BkgFile { lines: Vec<BkgLine> }` with `BkgLine::{File, Tile, NewLine, Slot(Slot, f, k), ClientArea,
FitSize, SetTopLeft, SetBottomRight, SetSize, StretchToBg, AlphaTest, Other(String)}` and the raw text per line
(lossless), and `DialogMap { entries: Vec<(Word, Word)>, trailing: Trivia }`. All three without
`wgpu`/`winit`/`egui`.

## Proposed corpus test (`nml-formats`)

With `NML_GAME_DIR`, through the VFS:

1. Read all 63 `.dlg`, 13 `.bkg` and both maps; **byte equality** after writing without a change.
2. No diagnostics (unknown words, missing end words, head keys after children, unresolvable number parts).
3. Exactly one `FRAME` per file; element counts after evaluation: 63/141/114/250/58/18/17/15 (FRAME, BUTTON,
   COMBOBOX, LABEL, RADIOBUTTON, SLIDER, EDITFIELD, LISTBOX); macros in the source text 13/54/55 (`LOOP`/`VAR`/`ADD`).
4. All text IDs (408 uses) present in the `TextDatei` tables; all file references in the VFS.
5. `dialogs.map`: 60 entries, key = `FRAME` name of the target file, exactly one missing path
   (`gameplayoptions3.dlg`); `dialogs2.map` differs in exactly four paths.
6. Samples of evaluated values against the documented values: `start.dlg` (`EXTENT -440 212 0 912`, target 40/212,
   `DURATION 300`, flags `0x40|0x80|0x4|0x1000|0x2|0x8`), an element from a nested loop in `account0.dlg` (name with
   two counters, `EXTENT` from `XPOS`/`YPOS`), combobox defaults (`CURSELECTION -1`, list `DISABLEDTEXTCOLOR
   0xFFA0A0A0`), slider default `RESOLUTION 10`.
7. `.bkg`: all file/tile indices in the texture grid; `menuframe.bkg` `CLIENTAREA 15 30 15 30`.

## Implementation

Quirk numbers O-1 ... O-13 and O-87 apply.

| Part | Code (`crates/nml-formats/src/`) | Tests |
|---|---|---|
| Lexical rules, lossless tree, writing | `dlg/mod.rs`: `lex_words`, `parse_dlg` -> `DlgDocument` (words with line and trivia), `write_dlg`, `upper_word`, `prop_arity` | `dlg::tests::*` (among others `malformed_dlg_never_hangs`, `deep_nesting_is_bounded`, `upper_word_follows_the_original`), corpus `tests/corpus_menus.rs` `all_dlg_files_round_trip_without_diagnostics` (63 files byte-identical) |
| Evaluation like the original | `dlg/expand.rs` `expand(doc, ExpandOptions { is_menu_level })` -> `Expanded` (widget tree, commands, diagnostics); `dlg/desc.rs` descriptors with the defaults of the original, `flag_bit` | `dlg::expand::tests::*` (among others `expand_stops_like_the_original`, `head_keys_after_the_first_child_abort_the_file`), corpus `all_dialogs_expand_to_the_documented_counts` (FRAME 63, BUTTON 141, COMBOBOX 114, LABEL 250, RADIOBUTTON 58, SLIDER 18, EDITFIELD 17, LISTBOX 15; `LOOP` 13, `VAR` 54, `ADD` 55), `dialog_text_ids_exist` (408 text IDs) |
| `.bkg` | `bkg.rs` `parse_bkg`/`write_bkg`, `BkgFile::desc` | `bkg::tests::*`, corpus `all_bkg_files_round_trip` (13 files) |
| `dialogs.map` | `dialog_map.rs` `parse_dialog_map`/`write_dialog_map` (through `lex_words`) | corpus `dialog_maps_have_60_entries` (exactly one missing path: `GAMEPLAYOPTIONS3`) |
| Loading, names, drawing | `crates/nml-ui/src/dialog.rs` `DialogLibrary::load` (keys from `dialogs.map` and file stem), `paint_static` | `crates/nml-ui/tests/corpus_dialogs.rs` `all_dialogs_paint`, `dialog_draw_checksums_are_stable` (`DIALOG_DRAW_GOLDEN`, 1024 x 768 and 1280 x 1024) |

Deviations (each with a quirk entry):

- **Faulty files do not hang** (O-3): where the original reads endlessly, the element ends with a diagnostic and a
  line number; what has been built up to then stays as in the original. In addition limits of 1,000,000 words read and
  64 levels.
- **Color fields without a default** (O-9): `Option<u32>` instead of a stack value.
- **`\r` in the lexer** (O-1): `lex_words` also ends a word at `\r` and keeps it as trivia (byte equality when
  writing); the original discards `\r` already when reading a line. In the middle of a word `\r` does not occur in the
  shipped data.
- **Writing outside Latin-1:** `write_dlg` writes characters above U+00FF as `?` (the shipped data is cp1252).
- **Top-level `/...` words** give a hint diagnostic; the original stops silently (same result).
- **Loose `.dlg` files** next to the archives do not count: `Vfs::files()` lists only archive files (all menu files
  are in the archives in the shipped data).
- `is_menu_level` (`Engine.ini` `IsMenuLevel`) selects the flags of `start.dlg`; viewer, `DIALOG_DRAW_GOLDEN` and
  reference comparison use `false`, like the reference images (`IsMenuLevel 0`).

Check: the static rendering of `START`, `OPTIONS`, `OPTIONS`+`VIDEOOPTIONS`/`SOUNDOPTIONS`/`CONTROLOPTIONS`/
`GAMEPLAYOPTIONS`, `SELECTPROFILE` and `CREDITS` at 1024 x 768 and 1280 x 1024 is pixel-identical to photos of the
original outside the run-time areas; `SINGLEPLAYERMENU` has no reference (not reachable without a profile), and
`SELECTPROFILE` shows 1 px with a deviation of 1.
