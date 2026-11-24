# Programs and switches

How to use `opennml-game`, `opennml-viewer`, `opennml-extract`, `opennml-icon` and the MCP server. Introduction and overview:
[README](../README.md). Every program lists all switches with a short description under `--help`.

## Game directory

All programs look for the original installation with the same search. The first valid installation wins, even if
more exist. Order:

1. `--game-dir <folder>`: if the switch is set, only this location is used.
2. Environment variable `NML_GAME_DIR` (an empty value counts as not set): if it is set, only it is used.
3. The remembered folder from `game-dir.txt` (see below).
4. Next to the program: the folder of the exe and each of its parent folders, in each case the folder itself,
   `No Man's Land` and `original\No Man's Land` inside it (so a ZIP next to the game folder also finds the
   installation).
5. Relative to the working directory: `../original/No Man's Land` and `original/No Man's Land`.
6. Steam (app 3863620): the Steam libraries from the registry and `libraryfolders.vdf`, in them the folder from the
   app manifest.
7. Uninstall entries of the registry whose name contains "No Man's Land" (location from `InstallLocation`).
8. Usual paths under `%ProgramFiles(x86)%` and `%ProgramFiles%` (`No Man's Land`, each also with `CDV`,
   `CDV Software` and `Related Designs` as parent folder).

Items 6 to 8 exist on Windows only.

Locations given explicitly (1 and 2) are never replaced by another one: if the location is invalid, the result is an
error, with no further search and no dialog. They only check that the archives `NML0.RDA` to `NML<n>.RDA` can be
opened without a gap; for an unknown edition the log shows a warning and the program keeps running (so partial copies
and demos remain usable). Locations that were found (3 to 8) and a choice in the dialog must also be recognized as the
German or English edition (11 archives). Invalid candidates are skipped. If everything fails, the error message names
every location checked with the reason and points to `--game-dir` and `NML_GAME_DIR`.

**Remembered folder:** `game-dir.txt` (UTF-8, one line, absolute path) is next to the settings file.
`opennml-game`: next to `--settings`, otherwise next to `NML_SETTINGS`, otherwise in `%LOCALAPPDATA%\OpenNML\`.
`opennml-viewer`: next to `--settings`, otherwise in `%LOCALAPPDATA%\OpenNML\` (it ignores `NML_SETTINGS`).
`opennml-extract` and the random map examples: in `%LOCALAPPDATA%\OpenNML\`. Only `opennml-game` writes the file, and only
after a valid choice in the dialog. If the remembered folder is no longer valid later, the program keeps searching
and asks anew as the last step.

**Dialog:** if `opennml-game` finds nothing, on Windows it opens the system folder dialog before the window ("Choose the
folder of No Man's Land"; the parent folder is also enough if `No Man's Land` is in it). An invalid choice is
reported with the reason and can be repeated. On cancel, or on No to the retry prompt, the error message also appears
as a message box. No folder dialog opens if `--game-dir` or `NML_GAME_DIR` is set (a wrong `NML_GAME_DIR` at a normal
start shows only the message box with the reason), with `--screenshot`, `--loading-shot`, `--control`, `--host` and
`--join`, and in `opennml-viewer`, `opennml-extract` and the tests.

The installation is never written to. The corpus tests do not use the search; they use only `NML_GAME_DIR` or the
fixed location `original/No Man's Land` next to the repository.

## opennml-game

    cargo run --release -p opennml-game
    cargo run --release -p opennml-game -- --mute --window-mode windowed --settings ../test-run/Engine.ini
    cargo run --release -p opennml-game -- --settings ../test-run/Engine.ini --screenshot ../start.png --time-ms 1500

The program of the reimplementation (without egui). Like the original, it starts into the main menu `START` with the
fly-in and the title music, and it exits through `QUIT` in the main menu or by closing the window; both write the
settings file (like the destructor of the original).

### What it can do

- **Videos:** with `IsIntro 1` the CDV logo, the Related logo and the intro play before the main menu. The `.bik`
  files are read from `DATA/Videos` at run time and played with the own decoder `crates/opennml-bink` (no conversion, no
  intermediate files). Any key aborts a video, the mouse never does.
- **Single player:** profile selection (`profiles.ini` next to the settings file), "Custom game" on multiplayer maps
  or random maps with game modes including Domination and King of the Hill, computer opponents, loading screen,
  command bar, portrait, minimap, diplomacy. ESC opens the game menu and pauses the match; saving, loading, quicksave
  F1 and quickload F4 work as in the original. Savegames are in `savegame/` next to the settings file
  (`<name>.<GUID>`, own container format).
- **End of a match and campaign:** a match ends by the rules of its game type or by giving up, with a victory or
  defeat screen and the accounting (technology, military, economy, game time). The four campaigns run with level
  scripts, AI, cutscenes and mission objectives; results are booked in `profiles.ini`.
- **Multiplayer on a LAN:** see [Network play](#network-play).

The texts that the game writes into the game screen itself (own messages and rows, from `crates/opennml-ui/texts/`)
follow the language of the installation; without a detected edition they follow the language of Windows. System windows
(game directory, error messages) are English.

### Switches

- `--game-dir <folder>`: only this location as the installation; without it the search under "Game directory".
- `--settings <file>`: settings file in the format of `Engine.ini`. Without it the environment variable
  `NML_SETTINGS`, otherwise `%LOCALAPPDATA%\OpenNML\Engine.ini`. A location inside the installation is an error:
  the `Engine.ini` of the original is never read or written. If the file is missing, the defaults of the original
  apply together with `defaultkeys.ini`; if it is corrupted, the defaults apply per key (warning in the log); it is
  overwritten only at the next save. It is written on `OPTIONS OK` and at program exit.
- `--window-mode windowed|borderless|exclusive`: without it from `ScreenFormat` (!= 0 window, 0 fullscreen with the
  video mode `ScreenXSize` x `ScreenYSize`). The window size comes from `ScreenXSize`/`ScreenYSize`, at least
  640 x 480, at most 8192 per side; the fonts are created at this width. Exclusive switches to this video mode right
  at the start (if the monitor does not have it, to its best mode). `ACCEPT` in the video options changes the window
  size or the fullscreen mode, the fonts stay until the next start (as in the original); with
  `--window-mode borderless`, `ACCEPT` only has an effect in the log (the size follows the monitor).
- `--monitor <n>`: monitor (0-based). `--mute`: no sound output.
- `--no-gamma`: no gamma ramp. Otherwise `Gamma` of the settings (slider in the video options) acts as in the
  original as linear brightness on the window image; screenshots never contain it.
- `--control <IP:PORT>` (loopback only, port 0 = free), `--paused` (only with `--control`, not in network play):
  control channel as in the viewer with the log line `Steuerkanal: <addr>`, but only with the subset for UI, match,
  input and network (`docs/viewer-control.md`); other commands answer with `"<name>" is not available in opennml-game`.
  While the clock is halted, menu and match advance only with `step`.
- Pointer and screenshot: `HardwareCursor 0` (setting, can be switched live in the options menu) draws the pointer as
  the 3D model from `mouse.rdo`; with `HardwareCursor 1` the system shows the pointer (the reimplementation the TGA).
  As in the original, the Print Screen key takes the image after drawing: with the software pointer it is in the
  photo, with the hardware pointer it is not. The control channel command `screenshot` draws it only with
  `pointer: true`.
- `--screenshot <png>` with `--time-ms <ms>` (default 1000): without window and sound, run the menu in steps of
  16 ms up to the time and save the image without pointer (writes no settings; not with `--control`). With
  `IsIntro 1` the start videos run first (at 1000 ms the photo shows the CDV logo); for the menu use `IsIntro 0`.
- **Menu level:** with `IsMenuLevel 1` (default of `Engine.ini`; in the game the `menulevel` checkbox in the game
  options) the world of `menü2.lvl` runs behind `START` and `OPTIONS` as in the original; the switch takes effect at
  the next construction of the dialog. `--host`, `--join` and `--screenshot` run without a menu level:
  `IsMenuLevel` is set to off in memory only, the settings file stays unchanged. If the user switches the checkbox on
  while running, the user's value applies. If the user switches it off while running, `--host`/`--join` still write
  the previously saved value back at exit (`keep_saved_menu_level`); in a run without `--host`/`--join` the value of
  the checkbox applies. `ui_state.menu_level` reports it on the control channel (`docs/viewer-control.md`).
- `--loading-shot <folder>` (check runs): one image `loading-<n>-<percent>.png` per phase of the loading screen in
  the folder (not with `--screenshot`).

### Feature `gui`

    cargo build --release -p opennml-game --features gui

The cargo feature `gui` builds `OpenNML.exe` for the Windows GUI subsystem: a double click opens no console window.
The release archive is built with it ([releasing.md](releasing.md)). Without the feature (the default for
`cargo run`) the exe is a console program and the log is visible in the terminal. With `gui` the log still goes to
stderr when it is redirected (for example to a file or a pipe, as the MCP server does), and always to the log file.

### Log

The program writes its log to stderr and to `logs\OpenNML.log` in the folder of the settings file (by default
`%LOCALAPPDATA%\OpenNML\logs\OpenNML.log`; with `--settings` or `NML_SETTINGS` the `logs` folder next to that
file). The release build has no console, so this file is the log to send in with a report. At each start the log of
the previous run is moved to `OpenNML.1.log`; older logs are dropped. If two programs share one user folder (host and
client on one machine), the second one rotates the file of the first, which then continues in `OpenNML.1.log`.

The file is created only after the settings file has been checked (a location inside the installation is an error),
so the program never writes a log into the installation. Until then the lines are kept in memory. If the start fails
before that point, the log goes to the default user folder, and only without `--settings` and `NML_SETTINGS`. A log
file that cannot be created or written never stops the program; the log then goes to stderr only.

### Errors and exit codes

When the program stops on an error or a panic, it logs the error (a panic with a backtrace) and ends with exit code
1 (error) or 101 (panic). On a plain start (no `--screenshot`, `--loading-shot`, `--control`, `--host` or `--join`)
it also shows a message box titled `OpenNML` with the error text and the path of the log file, or "No log file was
written." when there is none (for example with `--settings`, when the start fails before the settings file is
checked). The folder search shows its own message boxes; such an error is not shown a second time. Check runs never
show a message box, because it would block scripts and the MCP server: the error goes to stderr and the log only.

A command line that does not parse (an unknown switch such as `--gamedir`, a missing value) ends with exit code 2.
clap prints the message to stderr; on a plain start the same text also appears in the message box, because the release
build has no console. The log file does not exist yet, so the box says "No log file was written." If the command line
contains any of `--screenshot`, `--loading-shot`, `--control`, `--host` or `--join` (also as `--switch=value`), there is
no box, only stderr and the exit code. `--help` and `--version` print to stdout and end with exit code 0, without a box.

### Network play

In the menu: "Multiplayer" -> LAN search (`LAN1`), create match (`LAN3`), lobby with slots, ready, kick and chat
(`LAN2`), then `START`. On the first host start the Windows firewall asks about TCP 2302 and UDP 2303.

On the command line (without lobby):

    cargo run --release -p opennml-game -- --host --net-map "data/levels/multiplayer/[4players]-exile.lvl" --net-players 2
    cargo run --release -p opennml-game -- --join 192.168.0.10

- `--host [PORT]`: listen on PORT (without a value `NetPort` of the settings; 0 = 2302), wait for `--net-players` - 1
  clients and create the match on `--net-map`.
- `--join <ADDRESS[:PORT]>`: connect to the host (without a port 2302); without a host, back to the main menu after
  10 s.
- Only with `--host`: `--net-map <path.lvl>`, `--net-players <N>` (2 ... 8, default 2), `--net-ai <K>` (AI slots
  after the humans, 0 ... 7), `--turn-steps <L>` (turn length, default 2 as in the original) or
  `--turn-raster <T/V>`, `--net-seed <S>` (fixed seed for check runs).
- `--net-name <NAME>`: player name (default: profile name, otherwise the nameless player of the edition, "Player" or "Spieler").
- `--net-loopback` (check runs): the host listens only on 127.0.0.1, the search also asks 127.0.0.1.

## opennml-viewer

    cargo run --release -p opennml-viewer -- --level data/levels/kampagne1/kamp1-1/kamp1-1.lvl
    cargo run --release -p opennml-viewer -- --model data/r3d/wid/units/tierbeschwoerer/tierbeschwoerer_wald.rdo
    cargo run --release -p opennml-viewer -- --screenshot ../opennml-shot.png --time-ms 1500

Tool for development and checking. It shows a level like the original (terrain, water, objects, original camera),
plus models, effects, dialogs and random maps (`--random-map <seed>` with `--random-*`), and runs free matches with a
debug overlay (`--hud`, `--seed`, `--nations`, `--slots`, `--ki`, `--script`, ...).

Keys: arrows/screen edge scroll, Page Up/Page Down and mouse wheel zoom, Home/End tilt, F1 help, F2 free camera, F3
fog, F4 aspect ratio, F10 monitor, F11 window mode, F12 screenshot, Esc exit. F5 animation on/off, F6 light, F7
weather, F8 shadow, B model browser (Left/Right model, Up/Down animation, Space pause, V preview switch of the
effects, -/+ fire strength, left mouse button rotate, mouse wheel zoom), N effect browser (Left/Right effect, Space
restart, -/+ radius of the `.hef` preview). -/+ are the keys next to the period or left of Enter on the German
keyboard, or the numeric keypad; on the US keyboard also `-`/`=`.

Settings: copy `apps/opennml-viewer/opennml-viewer.example.toml` to `opennml-viewer.toml` in the working directory or give
`--config <path>` (game directory, level, aspect ratio, fog, animation, controls).

### Dialog browser

    cargo run --release -p opennml-viewer -- --mute --window-mode windowed --dialog options,videooptions
    cargo run --release -p opennml-viewer -- --mute --window-mode windowed --config size-1024.toml --dialog start --screenshot ../start_1024x768.png

`--dialog <NAME>[,<NAME>...]` opens dialogs of the installation over the view, in the order given (name = key from
`dialogs.map` or file stem, case-insensitive; an unknown name is an error with a message, `--dialog` and `--model`
exclude each other). In the window they can be operated: the same menu UI as in `opennml-game`, without the start
sequence, each dialog with fly-in and logic (click sound, subpages, sliders, prompts). While the layer is visible and
a dialog is open, mouse and keys go to the dialogs; Esc exits the viewer only without an open dialog, `QUIT` in the
main menu exits it. Ctrl+Page Down or Page Up opens the next or previous dialog of the library anew and alone, Ctrl+D
hides and shows the layer.

Settings: without a switch the defaults of the original together with `defaultkeys.ini` apply in memory only, nothing
is ever saved (`NML_SETTINGS` does not apply to the viewer). With `--settings <file>` (only together with `--dialog`,
checked as in `opennml-game`, never in the installation) the dialogs read this file and write it on `OPTIONS OK`. Display
mode and hardware pointer have no effect in the viewer.

With `--screenshot` the image stays static: state of the file (what the dialog logic sets at run time, such as
locked buttons or slider values, is missing), without a level on a black background in `width` x `height` of the
configuration. In the window, `ui_state`, `ui_click`, `ui_key` and `ui_open` of the control channel operate the
dialogs (`docs/viewer-control.md`, section "UI (`ui_*`)").

## Fonts

If the installation has no atlas for the window width (e.g. `MenuTitle` 39 px at 1920 x 1080), game and viewer create
it like the original with GDI from the Windows font named in `localisation.*` (e.g. Garamond) and put `.fnt`/`.tga`
into a cache outside the installation: environment variable `NML_FONT_CACHE` (folder), otherwise
`%LOCALAPPDATA%\OpenNML\fonts`. Deleting the folder makes the next start create the atlases anew. Without Windows or
without the font, an existing atlas is used as a fallback.

## opennml-extract

    cargo run --release -p opennml-extract -- list --filter "*.lvl"
    cargo run --release -p opennml-extract -- extract --out ../extracted
    cargo run --release -p opennml-extract -- convert --out ../extracted-png --filter "data/icons/*"
    cargo run --release -p opennml-extract -- convert --format gltf --out ../export-gltf --filter "data/levels/kampagne1/kamp1-1/kamp1-1.lvl"
    cargo run --release -p opennml-extract -- inspect data/levels/kampagne1/kamp1-1/kamp1-1.lvl

Output belongs outside the repository (no original game data in the repository).

## opennml-icon

    cargo run --release -p opennml-icon            # regenerate PNGs and ICO from the SVG sources
    cargo run --release -p opennml-icon -- --check # check only

Creates the program icon in `assets/icon/` (own artwork, may be in the repository); sources and sizes:
[assets/icon/README.md](../assets/icon/README.md).

## Control channel and MCP server

    cargo build --release -p opennml-viewer -p opennml-viewer-mcp
    cargo run --release -p opennml-viewer -- --control 127.0.0.1:0 --window-mode windowed

`--control` opens a control channel (JSON lines over TCP, loopback only) for synthetic input, a haltable virtual
clock, state queries and screenshots. `opennml-viewer-mcp` makes it available to Claude Code as MCP tools (`viewer_start`,
`viewer_input`, `viewer_time`, `viewer_screenshot`, `viewer_state`, ...; `viewer_start` with `program: game` starts
`opennml-game`, `viewer_ui` operates the menu UI). `.mcp.json` registers the server when Claude Code is started in this
directory (confirm at the first start); from another start folder, run once there
`claude mcp add --scope local opennml-viewer -- <repo>/target/release/opennml-viewer-mcp.exe`.

The current protocol version is in `crates/opennml-control/src/protocol.rs` (`PROTOCOL_VERSION`); commands and responses
are in `docs/viewer-control.md`.
