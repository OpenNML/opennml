# Viewer control channel and MCP server

With `--control`, `opennml-viewer` opens a **control channel**: synthetic input, a virtual clock, state queries,
screenshots rendered by the viewer, window mode and monitor. The MCP server `opennml-viewer-mcp` makes it available to
Claude as tools.

Without `--control` the viewer behaves as before (the clock then always runs in real time).

## Start

    opennml-viewer --control 127.0.0.1:0 [--paused] [further arguments]

- Only loopback addresses are allowed (`127.0.0.1`, `::1`); port `0` lets the operating system pick a free port.
- The viewer binds the port before the event loop starts and then writes the log line
  `Steuerkanal: 127.0.0.1:<PORT>` (stderr, level `info`).
- `--paused` (only with `--control`) starts the virtual clock stopped at 0 ms. Without the switch it runs in real
  time, and the state changes even before the first command.
- `--control` together with `--screenshot` is an error.
- `--script on|off` switches the level script like `sim_setup script_enabled` (default off); `--mission <name>`
  starts a campaign mission (see "Script commands (protocol 32)").
- `--random-map <SEED>` shows a random map as a preview without simulation instead of a level; command
  `random_map` (see "Random map").
- One connection at a time; further ones wait until the first is closed.
- With `--dialog`, the dialogs run on the virtual clock; stopped only with `step`. Without `--settings <file>`, their
  settings exist only in memory (defaults including `defaultkeys.ini`, nothing is ever saved), so check runs and
  images do not depend on the settings of the game; with `--settings` the viewer reads the file and writes it on
  `OPTIONS OK` (never into the installation). `opennml-game` has its own control channel with a subset of the commands
  (`docs/programs.md`, "opennml-game"): `hello`, `pause`, `resume`, `step`, `screenshot`, `state`, the `ui_*` commands,
  `sim_hash`, `sim_query`, `sim_command` (only with a running match), `sim_result` (likewise),
  `ui_present` (likewise), `mouse_move`, `mouse_button`, `key`, `ui_focus_lost`,
  `random_map`, `net_status`, `net_part_hashes`, `net_save`, `net_debug` (see "Network play (command line)" and
  "Network play (menus and lobby)") and `quit`; `state` and `step` answer there with the state of the UI
  (like `ui_state`).
- With `--hud`, the viewer draws the game UI like `opennml-game` (message bar, status bar, command bar, and the minimap)
  instead of the message lines in the overlay; at run time the command `hud` switches it. See "Game UI". With
  `--hud`, keys without a viewer binding also go into the key path of the original (see "Input in the game").
- The MCP server starts the game (`OpenNML.exe`) with `viewer_start` and `program: game` (`target/<profile>/OpenNML`, the same
  switches `--control 127.0.0.1:0`, `--paused`, `--mute`). It does not build: if the exe is missing, the message
  names `cargo build --release -p opennml-game`. `viewer_state`, `viewer_input` and `viewer_time` then return the state of
  the UI; `viewer_attach` always assumes the viewer (`hello` does not distinguish the programs).

## Protocol

One JSON line (`\n`) per request, one line per response. The response arrives when the command has been executed on
the main thread. Request lines are at most 1 MiB long.

    {"id": 1, "cmd": "key", "code": "ArrowUp", "action": "down"}
    {"id": 1, "ok": null}
    {"id": 2, "cmd": "key", "code": "F13", "action": "tap"}
    {"id": 2, "error": "unknown key \"F13\" (allowed: ...)"}

| Command | Parameters | Result |
|---|---|---|
| `hello` | - | `{"protocol": 51, "pid", "backend", "adapter"}` (backend/adapter `null` as long as the GPU is not ready) |
| `key` | `code` (winit spelling, e.g. `ArrowUp`, `KeyB`, `F11`), `action`: `down`/`up`/`tap`, optional `repeat` (only with `down`: that many further press events as automatic repeat; otherwise the error `repeat only with down`) | `null` |
| `mouse_move` | `x`, `y` (window pixels) | `null` |
| `mouse_leave` | - | `null` |
| `mouse_button` | `button`: `left`/`right`/`middle` (`middle` only in `opennml-game`, quirk O-331; the viewer answers with an error), `down` | `null` |
| `wheel` | `up` | `null` |
| `pause` / `resume` | - | `null` |
| `step` | `frames` (1-10,000), `frame_ms` (0 < x <= 100) | state snapshot |
| `screenshot` | `overlay` (default `false`), `max_size` (optional), `pointer` (default `false`, only `opennml-game`) | `{"width", "height", "rgba_hash", "png_width", "png_height", "png_base64"}` |
| `state` | - | state snapshot |
| `window` | `mode`: `windowed`/`borderless`/`exclusive`, `monitor` (both optional) | `null` |
| `hud` | `on` | `null`: game UI of the viewer on/off like `--hud` (see "Game UI"; not available in `opennml-game`) |
| `ui_state` | - | state of the menu UI (protocol 24; see "UI (`ui_*`)") |
| `ui_click` | `dialog`, `element`, `button`: `left` (default)/`right` (`middle` is rejected by the check) | `{"x", "y"}`: click on the center of the element in the next frame (protocol 24) |
| `ui_key` | `chord` (key names of the original, e.g. `ESCAPE`, `CONTROL H`), `char` (cp1252, optional) | `{"vk"}`: key event in the next frame (protocol 24) |
| `ui_open` | `dialog` | `null`: dialog queued for opening like command 5 (protocol 24) |
| `ui_video` | `name` (video name without path and extension, e.g. `eng`) | `null`: video `data/videos/<name>.bik` queued like `videostart` (open menus closed, video, menus opened again); only with an idle UI, otherwise an error (protocol 25; see "UI (`ui_*`)") |
| `ui_present` | `event` (JSON form of `Presentation`: object with one key or a string, e.g. `{"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}`) | `null`: presentation event triggered by hand, rendering only (checksum unchanged until the next step; after that, host consequences - selection, game speed - can generate commands; the game speed and fog mode of the host change with it); only with a match or map (see "Campaign UI") |
| `quit` | - | `null`, after which the viewer exits |
| `random_map` | - | random map of the preview (`--random-map`): `{"seed_text", "hash", "script", "defines", "dim", "players", "rand_counts", "rand_state", "digests", "objects", "slots_used", "start_points"}`; without a random map `null` (see "Random map"; in `opennml-game` the random map of the running match) |
| `lights` | - | list of the light names in the order of F6: `level` (values of the `.wld`), then the 32 presets from `lightdata.ini` |
| `set_light` | `name`: `level` or the name of a preset (case-insensitive) | `null`; unknown name: error with the known names |
| `set_weather` | `kind`: `none`/`rain`/`snow` | `null`; the particles are recreated |
| `set_quality` | `shadow` (0-3), `particles` (0-2), `terrain` (0-2), each optional | `null`; `shadow` reclassifies all objects of the map (like F8) and applies in the model browser to models without an F8 choice, `particles` recreates the effects of the map (from 1 ambient effects, from 2 permanent effects), `terrain` >= 2 draws the water in detail texture mode |
| `spawn_effect` | `path` (`.pef`), `position` `[x, y, z]` or `object_index` (map only), `scale` (> 0, default 1) | `null`; world effect of the current view, unlimited (one-shot effects end by themselves); without a position at the camera target or in the center of the ground plane; a file that cannot be loaded or a file without effect components (e.g. `.hef`, `.tga`): error |
| `clear_effects` | - | `null`; removes all effects of the current view created via `spawn_effect` |
| `set_damage` | `value` (0-3) | `null`; fire intensity in the model browser (rounded to 0.25, takes effect in the next game tick) |
| `play_sound` | `guid` + `action` (object type and action from `sounddatabase.dat`) or `path` (`.wav`; `.ogg` = dialog language), `position` `[x, y, z]` (optional: 3D sound) | `{"played", "id"}`; database sounds with the flags of the game (responses 12/14 as speech with category 8), 3D sounds farther than 20 tiles from the listener do not start and, as in the game, return no ID; missing file or occupied category: `played` = `false` |
| `music` | `action`: `play` (+ `track`: `title`/`win`/`lose`/`credits` or path), `stop`, `attack` (+ `on`) | `null`; `play` fades the level music out in 2 s and the menu music in in 1 s (`win`/`lose` without a loop), `stop` fades everything out, `attack` switches the combat music by hand like Shift+F9 (the times of the music state machine apply) |
| `set_volume` | `music`, `sfx`, `speech` (0-1000), `mute`, `unit_response`, each optional | `null`; sliders like `m_Volumes` (running 2D samples keep their volume, as in the game), `mute` = no device output, `unit_response` = `Engine.ini UnitResponse` (selection voice of the units and command acknowledgment; default from the settings file, where it is `true` as in the original; also takes effect without sound) |
| `audio_capture` | `action`: `start` (+ `path`) / `stop` | `start`: `null` (a running recording is finished first); `stop`: `{"path", "frames", "length_ms", "peak", "rms", "envelope"}` (levels left/right, full scale 1; `envelope` = RMS per 100 ms); without a running recording: error |
| `sim_setup` | `mode` (-1, 0, 1, 2 Domination, 3, 4/5 "Railroad"), `seed`, `nations` (e.g. `["SPA", "ENG"]`), `teams`, `game_speed` (0-30), `resource_setting` (0 = starting balance of the map, 1-3 = stages of `resourcenconfig.ini`), `living_space` (0-5 = living space 75, 100, 125, 150, 175, 200), `map_explored` (`true`/`false`, "map explored", `MapExplored`: "explored" fully set for all slots at the start; protocol 10), `slots` (lobby slots `{"kind": "closed"/"human"/"ai", "nation", "team", "color" 0-8, "handicap" 0-5, "ki_kind" 1-4}`, excludes `nations`/`teams`), `ki_enabled`, `campaign_difficulty` (0-2; protocol 26, see "AI match"), `script_enabled`, `time_per_char` (1-1000 ms per character; protocol 28, see "Scripts"), `time_limit` (time limit in minutes, 1-35791, default 20, otherwise as last time; see "Flag game modes"), each optional | `sim` section; with `mode`/`seed`/`nations`/`teams`/`resource_setting`/`living_space`/`map_explored`/`slots`/`ki_enabled`/`campaign_difficulty`/`script_enabled`/`time_per_char`/`time_limit` a new match on the same map (missing values as last time, selection empty), `game_speed` alone changes only the game speed; game type >= 0 only on multiplayer maps, 6 does not exist (error; 2 Domination); the resource and living space settings take effect only on multiplayer maps (campaign: starting balance of the map, living space 125); the same values are in the settings file (`resource_setting`, `living_space`, `map_explored`; command line `--map-explored`) |
| `sim_result` | `player` (slot 0-7), `result` (1 lost, 2 won); protocol 29 | `{"step"}` like `sim_command`: command `0x56` `SetPlayerResult` of the active player for the next tick (like the cheat of the original, but in the command log); errors `player = <n> invalid: allowed 0 to 7`, `result = <n> invalid: allowed 1 (lost) or 2 (won)`; see "Protocol 29" |
| `sim_command` | `command`: JSON form of `opennml_sim::command::Command`, e.g. `{"player": 0, "kind": {"Halt": {"units": [65537]}}}`, `{"player": 0, "kind": {"Diplomacy": {"p": 0, "q": 1, "mode": 0}}}`, `{"player": 0, "kind": {"SharedVision": {"p": 0, "q": 1, "on": true}}}`, `{"player": 0, "kind": {"Queue": {"cyclic": false, "inner": {"Halt": {"units": [65537]}}}}}`, combat (protocol 11, see "Combat section of `sim_query`"): `{"player": 0, "kind": {"Attack": {"units": [2048], "target": 6144, "pos": null, "mods": 0}}}`, `{"player": 0, "kind": {"SetBehavior": {"units": [2048], "mode": 3}}}` | `{"step"}` = engine tick before which the command takes effect; sender check of the simulation (foreign units are ignored) |
| `sim_query` | `player`, `guid`, `ids`, `limit` (1-10,000, default 100), each optional | `{"tick", "steps", "total", "objects", "players", "counter_upgrades", "daytime", "headhunters", "kill_counts", "button_locks", "slot_running", "area_views"}`: objects in table order (ID, type, owner, position, tile, direction, state, animation, target, tick group, modules with name, behavior `inert`/`walk`/`stand`/`building`/`tree`/`natur`/`resource`/`quarter`/`selfheal`/`construct`/`attack`/`fightnear`/`dying`/`fightfar`/`shoot`/`animal`/`edible`/`hunting`/`riding`/`swim`/`diving`/`train`/`multihead`/`multipart`/`globalupgrade`/`spell`/`exectempupgrade`/`transporttent`/`tent`/`trading`/`masking`/`assassin`/`healing`/`evokeghost`/`ghost`/`banish`/`evokeanimal`/`bomberman`/`killer`/`specialmove`/`headhunter`/`robbing`, on/off, idle, flags, timers; with a walk module `walk` = `{"state", "ende", "goal_fx", "goal" (target of the FINDTO, otherwise the last waypoint), "waypoints", "trajectory", "speed", "cruising", "running", "layer", "planner", "wait_for", "group" (group ID or `null`), "formation_num" (rank)}`; economy sections and the combat section `combat` only if the object has the module or hit points, see below; protocol 17: `tent`, `transport_tent`, `trade` and `in_collision`, see "Tents and trade"; protocol 18: per building `gate`, `area_watch` and `watch_rect`, see "Gates and enemy watch"; protocol 19: `masking`, `assassin`, `healing`, `message_filter` and `look`, see "Camouflage, digging in, healing"; protocol 20: `evoke_ghost`, `ghost`, `banish`, `evoke_animal` and `influence`, see "Ghosts, banishing, animal evocation, auras"; protocol 21: `spell` with `target` and `corpse`, `bomberman`, `killer` and `special_move`, see "Spells and special attacks"; protocol 22: `headhunter` and `robbing`, see "Headhunters and robbing"; protocol 23: `ai_marks` = the slots 0-7 (ascending) that have marked the object for the AI, absent if empty), player slots 0-7 (control, name, nation, team, color, resources in whole units - living space unclamped -, friends as a bit mask, `enemies` = enemies and `vision` = shared vision as bit masks of the slots (`vision`: this slot sees what the slots of the mask see, its own included; protocol 10), `population` = `[occupied, living space clamped to the limit]`, `counts` = stock per GUID without groups and `pending` = "in progress" per GUID (construction sites, queued production), both only entries != 0 and only if not empty, `upgrades` = applied upgrade GUIDs in order of application, only if not empty; protocol 16, see "Upgrades and arming"; `attack_reports` = attack reports of the slot `[{"victim", "tile": [x, z]}]` in the order in which `0x512E` selects them, only if not empty; protocol 23); `headhunters` = headhunter table per kind 0-2 (protocol 22, see "Headhunters and robbing"); protocol 23: per object `ai_marks` and per player `attack_reports`, see "AI queries"; protocol 30: per object `name` (script name, reverse lookup in the name table; absent without) and `hidden` (hidden; absent if not), plus `kill_counts`, `button_locks`, `slot_running`, `area_views`, see "Scripts" |
| `sim_save` | `path`, `replay` (default `false`) | `{"path", "bytes", "steps", "checksum"}`; savegame or replay since the start; a replay after loading a savegame is an error |
| `sim_load` | `path`, `steps` (replay only, at most 576,000 = eight hours of game time) | `sim` section; savegame or replay (recognized by the identifier) of the same map and rules, otherwise an error with the reason (format version, map, rules checksum); a replay runs up to `steps` (default: one tick after the last command) |
| `sim_hash` | - | `{"tick", "steps", "checksum", "history"}`: checksum (16 hex digits) and `[engine tick, checksum]` of the last 64 ticks |
| `sim_path` | `unit` (ID; current layer of its walk module, 8 when diving) or `from` (world point `[x, z]`, then with `layer`), `to` (`[x, z]`), `layer` (0-8, replaces that of the unit), `allow_override` (default `false`, like FINDTO `0x100`), `show` (default `true`) | `{"unit", "layer", "layer_name", "from_fx", "to_fx", "failure", "goal_fx", "goal", "start_tile", "tile_path", "smoothed", "waypoints_fx", "waypoints", "expansions", "message"}`: planning like `GameWorld_PlanHighPath`, fixed point in 1/4096 tile; `failure` `StartBlocked`/`GoalNotAdjacent`/`OutsideMap`/`Unreachable`/`NoPath`/`TooLong`; `message` = arguments of the message `0x500`; with `show` the image shows the path (tile outlines yellow, line turquoise), with `show: false` the call hides a path shown earlier |
| `sim_move` | `units` (IDs, default: selection), `to` (`[x, z]`), `append` (default `false`, like Shift), `cyclic` (default `false`, like Ctrl; together with `append` Shift wins), `drag_to` (`[x, z]`, optional; protocol 13: release point of the drag, see "Unit bar and formation dragging") | `{"step"}`: Go (`0x04`) of the active player like a right-click "Go to" (foreign units and units without a walk module are dropped; none left: error); `append`/`cyclic` queue per unit (without running and target direction, like the original) |
| `sim_unit_bar` | `button`, `target`, `ground` (`target` and `ground` only for the target buttons of protocol 21, see below); `button` (511-515 formation none/line/square/column/pincer, 519 scatter; protocol 14: 405 dismount, 406 disembark, 432 dive, 433 surface; protocol 15: 449 train forward, 450 train backward, 451 train stop; protocol 16: 485 arm, 486 disarm, the abilities 543, 544, 545, 549, 552, 557, 559, 560, 561, 563 and the counter-upgrades 3370-3377; protocol 17: 452 pack tent, 453 unpack travois; protocol 18: 430 open gate, 431 close gate; protocol 19: 434 camouflage, 435 uncamouflage, 437 dig in, 438 dig out, 458 and 554 start healing, 459 and 564 stop healing, 555 self-healing; protocol 20: 436 evoke ghost, 439, 547, 550, 553, 558 and 562 banish, 441 wolf, 442 bear and 443 evoke wildcat; protocol 21: 480 explosion, 481 storm, 482 reincarnation, 484 scout area, 541 drain mana, 542 whirlwind, 454/455 dynamite at building/ship, 456/548 kill, 457/546 snipe, 551 damage building, 556 scout map; protocol 22: 540 rob) | `{"step", "formation"}`: button of the unit bar for the current selection (protocol 13, see "Unit bar and formation dragging", protocol 14 "Riding and diving"); 511-515 choose the formation (viewer state) and re-form when more than one own unit is selected (`SetFormation` `0x44`), 519 gives `Scatter` (`0x75`), 405/406/432/433 give `Dismount` (`0x0B`), `LeaveBoat` (`0x0D`), `Dive` (`0x1B`) and `Surface` (`0x1C`) with the own units of the selection, 449/450/451 give `Train` (`0x29`/`0x2A`/`0x2B`) to the first own locomotive of the selection (protocol 15, see "Ships, trains and carts"), 485/486 give `ToggleArming` (`0x38`/`0x72`), the abilities `CastSpell` (`0x36`, kind 4) to every fitting wizard, 3370-3377 `CounterUpgrade` (`0x2F`) to the first own source of the selection (protocol 16, see "Upgrades and arming"), 452 gives `PackTent` (`0x2D`) for the first selected object if it is an own tent, 453 only opens the unpack cursor (no command; that comes with the left click; protocol 17, see "Tents and trade"), 430 gives `OpenGate` (`0x19`) and 431 `CloseGate` (`0x1A`) for the first selected object if it is an own, finished gate in the matching state (protocol 18, see "Gates and enemy watch"), 434/435/437/438/458/459/554/564 give `Mask` (`0x1D`), `Unmask` (`0x1E`), `DigIn` (`0x20`), `DigOut` (`0x21`), `StartHealing` (`0x34`) and `StopHealing` (`0x35`) with the own objects of the selection, 555 gives `SelfHeal` (`0x60`) per object with the button (protocol 19, see "Camouflage, digging in, healing"), 436 gives `EvokeGhost` (`0x1F`), 441/442/443 give `EvokeAnimal` (`0x22`/`0x23`/`0x24`) and 439/547/550/553/558/562 `Banish` (`0x25`) - unlike the buttons of protocol 19 **only for the first selected own object**, and only if the bar shows it the button (protocol 20, see "Ghosts, banishing, animal evocation, auras"); the target buttons of protocol 21 (480, 481, 484, 541, 454-457, 546, 548, 551, 556) require `target` (object) or `ground` (world coordinates `[x, z]`) and give the command immediately for the first selected own object, without targeting mode (482 and 542 go like the abilities to every selected own object with the button; see "Spells and special attacks"); 540 gives `RobCorpses` (`0x4C`) to every selected own robber whose bar shows the button (menu, research 3326; protocol 22, see "Headhunters and robbing"); the buttons 430, 431, 436, 439, 441-443, 452, 453, 485 to 3377, 547, 550, 553, 558 and 562 (and 555) act only on objects whose menu in `toolbar.ini` lists the button (like the bar), otherwise no command is sent and the response stays `{"step", "formation"}`; 434, 435, 437, 438, 458, 459, 554 and 564 go without a menu check of the viewer to all own selected objects, the simulation checks per object and rejects what does not fit (debug log); errors `Button <n> does not exist in the unit bar`, for target buttons `Button <n> needs a target (target or ground)` or the message of the target check |
| `sim_build` | `guid` (building type), `to` (`[x, z]` world), `rotation_steps` (0-3 quarter turns, default 0), `units` (workers, default: selection), `append` (default `false`, like Shift) | `{"step", "guid", "pos", "rotation", "rect", "workers", "affordable", "append"}`: like a left click in placement mode - minimum corner of the hull snapped to the tile boundary, building site check with the layer and region of the first worker, then Build (`0x01`) with all own objects from `units` as workers and the snapped position `pos` (world) and rotation `rotation` (radians); the height `pos[1]` is, as when building with the mouse (mouse point `0x004CCF00`, `Terrain::cursor_point`), the ground height vertically below the unsnapped target point `to`, below the water level and outside the map the water height; the rally point of the construction site is thus at this height plus node `ausgang`; `rect` = checked tile rectangle `[x0, z0, x1, z1]`; `affordable: false` is not an error (the simulation then rejects with the message of the missing resource, see `notices`). Errors: `no own workers (player p)`, `building blocked: <reason of the command check>` (e.g. `prerequisite for 1004 missing`, `1000 exists only once`, `no own worker has 1001 in the build menu`, `type 1100 is not a building`), `type <guid> cannot be rotated (rotation lock of the build cursor, ...): rotation_steps must be 0` (tower, gate, field, gold mine, Spanish shipyard 1008: building flags `& 0x10000000`; the command check would otherwise reject the command silently), `type ... has no model hull ...`, `invalid site for <guid> at (x, z), tiles [x0, z0, x1, z1]` (or `outside the map`). The simulation accepts only the snapped position and builds exactly there |
| `sim_train` | `building`, `guid` (unit or trade 490-493; != 0) | `{"step", "affordable"}`: train/trade (`0x0E`) in the own building; errors `training blocked: <reason>` (no own building, not finished or dead, not in the menu, locked on the map, prerequisite missing, only once; research (`UPGRADE`) is accepted like training, a `COUNTERUPGRADE` button is not) and `queue full (26 orders waiting)` (one running and 26 waiting orders of the building, orders not yet executed included) |
| `sim_cancel` | `building`, `index` (0 = running order, default; 1-26 = waiting) | `{"step", "guid"}`: cancel (`0x0F`) with the type of the entry; error if the building is not own or no order with the index is waiting |
| `sim_complete` | `building`, `index` (0 = running order, default; 1-26 = waiting), optional `guid` (expected type of the entry, not 0) | `{"step", "guid"}`: **debug** "complete immediately" (command `DebugCompleteOrder`, type `0xF0`, no counterpart in the original; like the debug button "DBG now" in the queue of the "Economy" window) with the type of the entry; check and errors like `sim_cancel`, plus an error if `guid` differs from the current entry. The command carries the type (without `guid` the viewer fills it from the current entry); if something else is at the position when it runs (the queue has moved up), the simulation rejects it (log). The simulation completes the order at the next tick through the normal flow: an unpaid one (waiting, or running before the start) goes through the start check and pays, a trade pays as at the end of the 8 s; if resources or goods are missing, the order stays unchanged and the message of the game appears (`notices`). Several calls take effect in command order; the running one hands over to the next waiting one (start in the next building tick) |
| `sim_sell` | `buildings` (default: selection) | `{"step", "buildings", "skipped"}`: sale (`0x67`) of the own living buildings, the rest under `skipped`; none left: error |
| `sim_action` | `units` (default: selection), `target` (object) and/or `to` (`[x, z]`; without `to` the position of `target`), `append`, `cyclic` (like Shift/Ctrl; both together: Shift wins, so append, like `Input_SubmitCommandWithQueueKeys`) | `{"step", "button", "action", "command", "code", "queued", "cyclic", "kind"}`: right-click of the active player according to `targets.ini` as with the right mouse button (see "Command interface"); `button` = `BUTTONMSG` (e.g. 400), `action` its name (`Chop wood`), `command` the command variant without queue wrapping (`ChopWood`) with type code `code` (6), `queued`/`cyclic` = wrapped in `0x7B`/`0x7C` (only for the actions the original wraps), `kind` the sent command in JSON form; if the target is an object and the action is not a ground action, the viewer sets the target marker as on a right-click (`sim.target_marker`). Errors: `no units (selection empty)`, `no action: <id> does not belong to player p`, `target <id> missing`, `no action for <id> (targets.ini)` or `no action for the ground (targets.ini)`, `no action for the ground outside the map` (ground actions - Go to, rally point on the ground - outside the map are discarded by the UI of the original); at least `target` or `to`. Limit: `target` is always the living object (without a fog check, like a hit of the render list); memory images under the fog (buildings and trees the active player saw last) cannot be addressed over the control channel, only with the right mouse button |
| `sim_menu` | `units` (workers, `[]` = selection) **or** `building` | `{"building", "workers", "entries", "queue"}`: build list of the own workers (build menu of the type including the submenus of its buttons - `TARGET_DO` and, as with female settler/female pioneer, buttons without a kind) or production list and queue of an own building; per entry `{"guid", "name" (type name, for research the text from `guids.deu`, otherwise `Type n`), "kind" (kind of the button: `BUILD`, `ONLYONCE`, `MAKE`, `MAKEONLY_ONCE`, `UPGRADE`; counter-upgrades are not in the production list but are bar buttons 3370-3377 of the unit bar), "costs" ([gold, wood, food, population, living space] or `null`), "affordable", "blocked" (reason for the lock as in the "Economy" window, `null` = selectable)}`, queue `{"index", "guid", "name", "progress"}` (index like `sim_cancel`); error without own workers or with a foreign building, `units and building are mutually exclusive` (MCP `viewer_sim` `menu`: `ids and building are mutually exclusive`) |
| `sim_hire_menu` | `player` (viewer 0-7), `art` (0 SPA, 1 PRI, 2 SID), optional `target_player` (0-7) | `{"players", "targets"}`: headhunter menu (protocol 22) like levels 2 and 3 in the "Economy" window: `players` = target players of the slot `player` (`FUN_0044A8C0`: the slots 0-7 ascending that are still playing, are an enemy of `player` and whose color button `470 + color` is not locked), `targets` = target GUIDs of the `target_player` for the kind (`FUN_0044AA10`: his elites in table order, which he owns; without an elite, and only if the headhunter of the kind already exists, the headquarters of his nation); without `target_player` empty, with it always computed, even if he is not in `players` (like the original). Read only: checksum unchanged. Hiring goes through `sim_command` with `HireHeadhunter`. Errors `player = <n> invalid: allowed 0 to 7`, `art = <n> invalid: allowed 0 to 2`, `target_player = <n> invalid: allowed 0 to 7` |
| `ki_query` | `player` (slot 0-7), `code` (query code `0x4E20`...`0x5130` as a decimal number, `0x4E24` = 20004), optional `words` (request words from `[2]`, at most 128, default `[player]`), optional `sight` (`"omniscient"`, default, or `"fog"`) | `{"code", "request", "direct"}`: an AI query (protocol 23) against the running savegame, as a script poses it through the query facade of the AI. `request` is the whole request field `[code, argc, [2], [3], ...]` after the response (`argc` = number of `words`; the result is usually in `[2]`), padded with 0 to `2 + max(argc, 128)` words; a failure (unknown code, wrong `argc`) leaves the inputs unchanged (quirk E-27). `direct` lists the direct changes the query would make in the game, in order of execution, each `{"kind", "object", "player"}`: `kind` `"mark"` or `"unmark"` (AI mark of the slot `player` on the object `object` set or deleted; also dead objects, but not missing ones: a mark of a missing ID has no effect in the game and is not in `direct`) or `"take_report"` (the attack report of the victim `object` would be taken from the table of the slot `player`). **`ki_query` leaves the checksum unchanged; direct changes are only reported, not executed.** Errors `player = <n> invalid: allowed 0 to 7`, `words has <n> words: allowed at most 128`, `sight = "<s>" invalid: allowed "omniscient" or "fog"`; see "AI queries" |
| `ki_state` | optional `slot` (0-7), optional `values` (default `false`) | `{"paused", "engines", "rejected", "commands"}`: AI state (protocol 26): per engine threads in execution order (module, name, pc, status, sleep, semaphore, stack depth, top 8 stack values, child), semaphores, with `values` also `m_iValues`; counters of the AI commands (host state since the last restart). **Leaves the checksum unchanged**; see "AI match" |
| `ki_command` | `action` (`start`/`pause`/`step`); `start`: `slot` (0-7), `module` (module ID from `nml.db`), optional `args` (at most 64), `sync` (default `false`); `pause`: optional `paused` (default `true`); `step`: `slot`, optional `passes` (1-1000, default 1); fields of other actions are errors | `{"step"}`: AI intervention as a command of the simulation at the next tick (protocol 26, `KiStart`/`KiPause`/`KiStep`, in the command log): start a module (synchronous or as a thread), pause/resume the whole AI, single steps while the AI is paused (per pass, each thread of the slot takes exactly one step with `dt = 20`). Errors up front: `slot` missing or without an engine, module unknown, `AI is running: ki_command pause first`; see "AI match" |
| `script_state` | optional `block` (`{"container", "index"}`: container level 1, ... or dialog -1 and block index in the script file), optional `names` (substring), optional `log` (0-1000 steps, default 50) | `{"enabled", "dialog_running", "containers", "conditions", "speech", "names", "log", "main_quests", "sub_quests", "countdown_container", "countdown_s", "task_status", "area_overrides", "commands", "results", "ui"}`: script state (protocol 28): containers with countdown, name spaces and blocks (index, name, state, action index, wait reason), with `block` its conditions with the current value (evaluated on a copy), speech state, object names, block log (host state of the last 1,000 steps), since protocol 30 also mission objectives, countdown display, task status and changed areas, since protocol 32 counters and last rejections of the script commands (`commands`) and the decided slots (`results`), since protocol 33 control lock, cinemascope, selection mirror and last button (`ui`). **Leaves the checksum unchanged**; see "Scripts", "Script commands (protocol 32)" and "Presentation (protocol 33)" |
| `script_events` | optional `steps` (0-1000, default 50) | `{"events": [{"step", "event"}], "presentation", "ui", "profile_vars", "camera"}`: presentation events of the last steps, presentation state, UI (lock, cinemascope, selection mirror, last button), profile variables and camera of the scripts (protocol 33). **Leaves the checksum unchanged.** Only in the viewer, not in `opennml-game`; see "Presentation (protocol 33)" |
| `script_command` | `action` (`skip_dialog`) | `{"step"}`: debug intervention in the script flow as a command of the simulation at the next tick (protocol 32, `ScriptSkipDialog`, in the command log, therefore also in replays and savegames): ends the running dialog at the next tick (like the skip flag `WaitForKIDebugger`). The sender is the active player. Error: `script_command skip_dialog: no dialog running` (nothing queued); unknown action when the line is read. Only in the viewer, not in `opennml-game`; see "Script commands (protocol 32)" |
| `sim_upgrade` | `player` (target slot 0-10), `guid` (!= 0), `remove` (default `false`) | `{"step"}`: apply an upgrade like the console (`0x39`, `ApplyUpgrade`) or remove it (`0x3A`, `RemoveUpgrade`); the sender is the active player, `player` only the target slot; applying with `guid` < 0 applies all GUIDs 2800 to 3399, removing needs `guid` > 0; without an ownership check as in the original (protocol 16, see "Upgrades and arming"); errors `player = <n> invalid: allowed 0 to 10 (target slot)`, `guid = 0 invalid`, `guid = <n> invalid: removing needs a GUID > 0` |
| `sim_trade` | `player` (sender), `units` (carts; empty = selection), `ware` (0 buy wood, 1 buy food, 2 sell wood, 3 sell food), `target` (partner slot 0-8) | `{"step"}`: trade with player (`0x45`, `TradeWithPlayer`): per own cart with a trade module `{0x12B, ware, target}` with abort; the sender of the command is `player`, not the active player, so that rejections and the ignoring of foreign carts stay testable; the simulation alone checks the values (kind 0-3, target 0-8 and not the sender, the target slot has a trading building), a rejection is in the debug log and leaves the checksum unchanged (protocol 17, see "Tents and trade"); error `no units (selection empty)` |
| `sim_tribute` | `player` (giver and sender), `recipients` (bit mask of the slots 0-7), `resource` (0 gold, 1 wood, 2 food), `amount` | `{"step"}`: tribute (`0x65`, `Tribute`): `player` pays `amount` to every slot 0-7 with a set bit (except itself); the simulation checks resource, amount and stock, a rejection is in the debug log (protocol 17, see "Tents and trade") |
| `sim_grid` | `layer` (0-8 path grid, 9 occupancy), `rect` (`[x0, z0, x1, z1]` in tiles, inclusive, at most 4,096 cells) | `{"layer", "name", "width", "height", "blocked", "regions", "counter", "built", "rect", "rows"}`: blocked (or occupied) cells, regions and ID counter; `rows` per z the cells (-1 blocked, otherwise region ID; the occupancy grid has 0/1; -2 outside the map) |
| `sim_fog` | `player` (viewer 0-7, default: active player), `rect` (`[x0, z0, x1, z1]` in tiles, inclusive, at most 4,096 cells), each optional | `{"player", "width", "height", "explored", "seen", "cleared", "processed", "cycle_player", "per_tick", "sources", "rect", "rows"}`: tile counts of the fog grids "explored", "seen" and "cleared" (foreground) with the vision mask of the player (shared vision included), cycle fields `m_FogHandlesProcessed` (0 new cycle, > 0 running, -1 idle tick, -2 alpha), `m_FogCurPlayerID`, `m_FogHandlesPerTick`, number of vision sources; `rows` per z the states `GameWorld_GetFogOfWarStateAt` (0 unexplored, 1 explored, 3 seen, 7 cleared; -2 outside the map), without the "Fog off" switch. Read only: checksum and `sim_hash` stay unchanged |
| `sim_view` | `active_player` (0-7), `fog_shown` (`false` = "Fog off", `m_FogShowMode` 7; `true` = fog on), `fog_overlay` (0 off, 1 explored, 3 seen, 7 cleared), each optional | `sim` section: view of the viewer in this order - active player like key `KeyP`, but directly (brightness immediately without crossfade, memory images rechecked, selection empty after a change, nation music of the new player like command 28 `0x0047B510`; an empty slot is the error `slot p is not occupied`), "Fog off" like key `KeyO` (takes effect wherever the original queries `m_FogShowMode` - terrain, objects, selection; the brightness table stays as in the original), overlay like key `KeyF`. The simulation stays untouched (checksum unchanged) |

Protocol versions: 1 = control MCP, 2 = light and weather (commands above, section `environment`, keys
`F6`/`F7`), 3 = shadows (section `shadows`, class and shadow kind in `hovered`, shadows in the `browser`, key `F8`),
4 = effects (sections `effects` and `effect_browser`, effects in the `browser`, `view` = `effects`, commands
`spawn_effect`, `clear_effects`, `set_damage`, keys `KeyN`, `KeyV`, `Minus`, `Equal`), 5 = sound (section `audio`,
sound actions in the `browser`, commands `play_sound`, `music`, `set_volume`, `audio_capture`, keys `F9`, `KeyT`,
`Enter`), 6 = simulation (section `sim`, `sim_id` in `hovered`, commands `sim_setup`, `sim_command`, `sim_query`,
`sim_save`, `sim_load`, `sim_hash`, keys `KeyP`, `KeyH`, `KeyI`), 7 = pathfinding (commands `sim_path`, `sim_grid`,
`grid_layer` and `path` in the `sim` section, key `KeyG`), 8 = movement (command `sim_move`, `walk` in `sim_query`,
`moving` in the `sim` section), 9 = economy (commands `sim_build`, `sim_train`, `sim_cancel`, `sim_sell`,
`sim_action`, `sim_menu`, later added `sim_complete` (debug), `resource_setting`/`living_space` in `sim_setup`,
sections `building`, `gatherer`, `construct`, `quarter`, `resource`, `tree` in `sim_query`,
`population`/`counts`/`pending` per player, `wood_price`, `food_price`, `max_living_space` and `notices` in the `sim`
section, keys `KeyR`, `ControlLeft`), 10 = vision (commands `sim_fog`, `sim_view`, `map_explored` in `sim_setup`,
`enemies`/`vision` per player, `fog_shown`/`fog_overlay` in the `sim` section, keys `KeyF`, `KeyO`), 11 = melee
(`combat` per object in `sim_query`, `attack_points`/`attack_points_active` in the `sim` section,
`attack_points`/`attack_source` in `audio.music`, `recent_sounds` in `audio`, commands `Attack`, `SetBehavior`,
`GarrisonAttack` via `sim_command`, keys `Digit1`-`Digit5`, `KeyX`), 12 = ranged combat (`ranged`, `animal`, `edible`
per object and `prey` in the gatherer section of `sim_query`, `projectiles` and `flights` in the `sim` section,
commands `Hunt` and `Follow` via `sim_command`), 13 = groups and formations (command `sim_unit_bar`, `drag_to` in
`sim_move`, `groups` and `formation` in the `sim` section, `group`/`formation_num` in the `walk` section, window
"Unit", commands `SetFormation` and `Scatter` via `sim_command`), 14 = riding, canoes, swimming and diving
(riding and diving section `riding`/`diving` per object in `sim_query`, bar buttons 405/406/432/433 in `sim_unit_bar`
and in the "Unit" window, commands `Mount`, `Dismount`, `BoardBoat`, `LeaveBoat`, `Dive` and `Surface` via
`sim_command`), 15 = ships, trains and multipart objects (train, multipart, meeting point and broadside fields in
`sim_query`, event bits, bar buttons 449-451 in `sim_unit_bar` and in the "Unit" window, command `Train` via
`sim_command`), 16 = upgrades and arming (player lists `upgrades`, counter-upgrades `counter_upgrades`, time of day
`daytime`, temporary upgrades `temp_upgrades`, counter `show_counter_upgrade`, spell `spell`, `armed`, `exec_list` in
`sim_query`, command `sim_upgrade`, bar buttons 485/486, 543 ff. and 3370-3377 in `sim_unit_bar` and in the "Unit"
window, commands `CounterUpgrade`, `CastSpell`, `ExecTempUpgrade`, `ToggleArming`, `SetArrowType` via `sim_command`),
17 = tents, travois, trade and tribute (sections `tent`, `transport_tent`, `trade` and `in_collision` per object in
`sim_query`, commands `sim_trade` and `sim_tribute`, bar buttons 452/453 in `sim_unit_bar` and in the "Unit" window,
commands `PackTent` and `UnpackTent` via `sim_command`), 18 = gates and enemy watch (sections `gate`, `area_watch`
and field `watch_rect` per building in `sim_query`, bar buttons 430/431 in `sim_unit_bar` and in the "Unit" window,
commands `OpenGate` and `CloseGate` via `sim_command`), 19 = camouflage, digging in, healing (sections `masking`,
`assassin`, `healing`, `message_filter` and `look` per object in `sim_query`, bar buttons 434/435, 437/438, 458/459
and 554/555/564 in `sim_unit_bar` and in the "Unit" window, commands `Mask`, `Unmask`, `DigIn`, `DigOut`,
`StartHealing`, `StopHealing` and `SelfHeal` via `sim_command`), 20 = ghosts, banishing, animal evocation, auras
(sections `evoke_ghost`, `ghost`, `banish`, `evoke_animal` and `influence` per object in `sim_query`, the module
kinds `evokeghost`, `ghost`, `banish` and `evokeanimal`, bar buttons 436, 439, 441-443, 547, 550, 553, 558 and 562 in
`sim_unit_bar` and in the "Unit" window, commands `EvokeGhost`, `EvokeAnimal` and `Banish` via `sim_command`),
21 = spells and special attacks (`target` and `corpse` in the `spell` section, sections `bomberman`, `killer` and
`special_move` per object in `sim_query`, the module kinds `bomberman`, `killer` and `specialmove`, the projectile
kinds `dynamite`, `explosion` and `storm` with `pulse` in the `sim` section, target buttons 480, 481, 484, 541,
454-457, 546, 548, 551, 556 as well as 482 and 542 in `sim_unit_bar` with `target` and `ground` and in the "Unit"
window, commands `CastSpell` (kind 1-3 and 5), `ScoutArea`, `DrainMana`, `DynamiteBuilding`, `DynamiteShip`,
`KillUnit`, `SnipeUnit`, `DamageBuilding` and `Whirlwind` via `sim_command`), 22 = headhunters and robbing (sections
`headhunter` and `robbing` per object and the headhunter table `headhunters` in `sim_query`, the module kinds
`headhunter` and `robbing`, the projectile kind `throw` with `impact` in the `sim` section, the text messages of the
headhunter in `notices`, command `sim_hire_menu`, bar button 540 in `sim_unit_bar` and in the "Unit" window, the
headhunter menu in the "Economy" window, commands `HireHeadhunter` and `RobCorpses` via `sim_command`),
23 = AI queries (command `ki_query`, `ai_marks` per object and `attack_reports` per player in `sim_query`),
24 = menu UI (`ui_state`, `ui_click`, `ui_key`, `ui_open`), 25 = videos (`ui_video`, `video` and `pointer_hidden` in
`ui_state`, effects `play_video`/`stop_video`), 26 = AI match (commands `ki_state` and `ki_command`,
`slots`/`ki_enabled`/`campaign_difficulty` in `sim_setup`), 27 = match from the menu (`ui_state.game`, game effects,
`sim_hash`/`sim_query`/`sim_command` in `opennml-game`), 28 = `script_state`, `sim_setup`
`script_enabled`/`time_per_char` (key `KeyJ`, "Script" window), 29 = game end, accounting, campaign (`sim_result`,
`ui_state.game.result`, `ui_state.accounting`/`play_time`, effects `game_end`, `accounting`, `surrender`,
`start_campaign_level`, `book_campaign`; see "Protocol 29"), 30 = world state and mission objectives of the scripts
(`sim_query` `name`/`hidden` per object, `kill_counts`, `button_locks`, `slot_running`, `area_views`; `script_state`
`main_quests`/`sub_quests`, `countdown_container`/`countdown_s`, `task_status`, `area_overrides`; level container
ID 1), 31 = game UI (`ui_state.game.hud`, viewer command `hud`; see "Game UI"), 32 = script commands
(`script_command`, `script_state` `commands`/`results` and the block log message `Command`, `DebugDestroy` in
`sim_command`; see "Script commands (protocol 32)"), 33 = presentation (`script_events`, `script_state` `ui`,
`ScriptSelection`/`ScriptButton` in `sim_command`, MCP `viewer_script`; see "Presentation (protocol 33)"),
34 = command bar (`ui_click`/`ui_state` for `GAMEUI`, mouse commands in `opennml-game`; see "Command bar").
35 = campaign UI (`ui_present`, `ui_state.game.presentation`; see "Campaign UI"). 36 = input in the game (`key`
with `repeat`, `ui_focus_lost`, `ui_state.game.input`; see "Input in the game"). 37 = minimap, diplomacy and chat
(`ui_state.game.minimap`/`diplomacy_open`/`chat_lines`, the elements `minimap*` of `GAMEUI`, the effects `diplomacy`
and `send_chat`, `sim_command` with `Signal` and `Chat`; see "Minimap, diplomacy and chat"). 38 = world rendering
(`world_display` in the state snapshot and `ui_state.game.world`, `sim_build` with ground height; see "World
rendering"). 39 = flag game modes: `sim.flags`/`ui_state.game.flags`, lobby color 0 to 8, `sim_setup` with
`time_limit` and game type 2, `ui_state.game.presentation.timers` (remaining time of the timers); see "Flag game
modes" and "Campaign UI" (`ui_state.game.presentation`). 40 = random map: command `random_map`; see "Random map".
41 = random map with objects: `random_map` with object counters and checksums (`rand_counts` six, `digests` seven
entries), `objects`, `slots_used`, `start_points`; `random_map` also in `opennml-game`; `start_match` of a random map with
`level` = `randommap:<seed>`. The response of `random_map` is not readable with protocol 40 (fixed lengths changed).
42 = network play in `opennml-game`: `net_status`, `net_part_hashes`, `net_save`, `net_debug`; `pause` and `step` rejected
on the network, `sim_command`/`sim_result` only for the own slot; see "Network play (command line)". 43 = middle mouse
button (quirk O-331): `mouse_button` with `middle` (`opennml-game`), `ui_click` rejects `middle`; see "Middle mouse button
(quirk O-331)". 44: effects `net`, `search`, `lobby` in `ui_state.effects`; `net_status` with `role` `null` during the
search, the phases `searching`, `connecting`, `lobby` and the fields `spacing`, `lead`, `build`, `rtt_ms`, `lobby`,
`found`; `ui_*` free during search and lobby; see "Network play (menus and lobby)". 45: `ui_state.menu_level` and the
effect `menu_level` in `ui_state.effects`; `sim_hash`/`sim_query` also read the menu level in `opennml-game`; see "Menu
level". 46: `net_status.lobby.saved` and `net_status.lobby.rows[].seat`; see "Network play (network savegames)". 47:
`sim_load` up to 576,000 ticks, `ui_state.game.replay`, effect `play_replay`; see "Replays". 48 (3D mouse pointer):
`screenshot.pointer` and `ui_state.pointer_model`; see "3D mouse pointer". 49 (sound driver): effect
`set_sound_driver` (`index`) in `ui_state.effects`.
Each version can read the responses of the previous one; the viewer and the MCP server must nevertheless speak the
same version (`hello` checks this).

Valid key names: `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `PageUp`, `PageDown`,
`Home`, `End`, `Insert`, `Delete`, `KeyW`, `KeyA`, `KeyS`, `KeyD`, `KeyQ`, `KeyE`, `KeyB`, `KeyN`,
`KeyV`, `KeyT`, `KeyP`, `KeyH`, `KeyI`, `KeyG`, `KeyR`, `KeyF`, `KeyO`, `KeyX`, `KeyK`, `KeyL`, `KeyJ`, `KeyU`,
`Digit0`-`Digit9`, `Minus`, `Equal`, `BracketRight`, `Slash`, `Period`, `Comma`, `NumpadAdd`, `NumpadSubtract`,
`ShiftLeft`, `ShiftRight`, `ControlLeft`, `Tab`, `Space`, `Enter`, `Escape`, `Pause`, `PrintScreen`, `F1`-`F12`
(the names after `KeyU` and `Digit0`/`Digit6`-`Digit9` are for the keys of the original; `opennml-game` maps the
physical key to the VK of the **German** layout: main-block `+` is `BracketRight`, main-block `-` is `Slash`,
`Equal` there is the acute accent and `Minus` the sharp s)
(`F9`: sound on/off, with Shift held combat music on/off; in the model browser `KeyT`
next sound action of the type, with Shift the previous one, `Enter` plays it;
`F6` with Shift held: light back; `F8` on the map: `shadow_quality`
0 -> 1 -> 2 -> 3, in the model browser: shadow kind none -> 1 -> 2 -> 3; `KeyN`: effect browser on/off;
in the model browser `KeyV` preview switch and `Minus`/`Equal` fire intensity -/+ 0.25; in the
effect browser arrows left/right file, `Space` restart, `Minus`/`Equal` radius of the
`.hef` preview -/+ 128). `Minus`/`Equal` are the physical keys of the US layout (German keyboard:
sharp s and acute accent); on the keyboard the viewer additionally accepts the German -/+ (`Slash`/`BracketRight`)
and the numeric keypad (`NumpadSubtract`/`NumpadAdd`); the control channel stays with `Minus`/`Equal`.
On the map: `KeyP` next occupied player slot (with Shift the previous one), `KeyH` halt for
the own selected objects, `KeyK` "AI" window and `KeyL` AI overlay (section "AI match"), `KeyJ` "Script" window
(section "Scripts"), `KeyI` debug window (the "Simulation" window, egui) on/off ("Economy" section with
living space limit, market prices and resource factor; the inspector shows construction, production with
queue, docking slots and rally point of buildings, state, load, source and storage of
gatherers, construction workers and quarter occupants; the window does not interfere with the simulation - the
debug button "complete immediately" is in the "Economy" window, see below), `KeyG` overlay of the
pathfinding (off -> layer 0 ... 8 -> occupancy -> off, backwards with Shift; +/-24 tiles around the
camera, blocked red, regions colored per ID, occupied orange; with exactly one selected own
unit the path to the ground point under the mouse pointer (over water the water surface), lines in the debug window
under "Pathfinding"), `KeyF` fog overlay of the active player (off -> explored -> seen -> cleared
-> off; +/-24 tiles around the camera, each tile with at least the level: explored gray, seen
blue, cleared red; shared vision included), `KeyO` "Fog off" on/off (debug switch
`m_FogShowMode` 7 like F4 in the original; the "Fog" section of the debug window shows cycle,
vision sources, tile counts, switch and overlay, the player table enemies and vision). `Digit1`-`Digit5` with Shift
held set the stance of the own selected units
(command `0x12` mode 0-4: cease fire, hold position, defensive, offensive, defend area;
without Shift the digits do nothing), `KeyX` switches the attack mode for the next
right-click (overlay line "Attack: select target"; the right-click on the map sends attack `0x11`
with modifier 4 or stance 3 and march with flag 4, after which the mode is off; `Escape` ends it
instead of the viewer). A normal right-click on an enemy (411) with `ControlLeft` held sets
modifier 4. Above the selection and the object under the mouse the viewer draws the health bar, above
units with rank > 0 the rank symbol (rendering only, the checksum stays unchanged). The
left mouse button selects:
releasing after less than 2 pixels is a click (units before buildings, foreign objects only
inspected), otherwise a box over the own objects (at most 30, several only units); with
Shift held it adds or removes.

**Command interface.** On release, the right mouse button is a click at the press point, even if the mouse moves in
between (as in the original); only with the free camera does it rotate after 3 pixels of movement and not click. The
click chooses the action according to `targets.ini`: the first entry in file order that the selection (MY filter) and
the object under the mouse satisfy, otherwise the ground (447 rally point before 448 "Go to"). As in the original,
two target searches determine the object under the mouse (the "click target" and the "right-click target"), the last
accepted hit per rank, and the first rank with a hit decides (for the right-click target only with an action, see
below): the overlay line "under the mouse" (`hovered` in the state snapshot), the ground ring under the mouse and the
left click take the **click target** (`0x0047A210`: living units, dead units, other objects with module
0/`0x33`/`0x21`, then buildings, then resources - **never trees or nature**; a left click on a tree selects
nothing); the hint `Right-click: ...` and the right-click take the **right-click target** (`0x00480E80`: units, dead
ones, others as above, then buildings, then all other static objects including trees and nature). Both use the ray
test of the original (`ObjectRenderer_PickScreenPoint`): the box of the helper node `hitbox` hits without a distance
comparison. Otherwise the bounding sphere and box of the overall hull (during a running animation its
`MAXBOUNDINGBOX`) are checked: for non-units they are only a prefilter, the triangles of the model are decisive
(first triangle hit, distance in model space, foliage gaps hit); for units they are the hit area, and of several
hit ones the one with the smallest distance of the hull center to the ray wins. A ray over the roof of a building
therefore hits the tree behind it for the right-click (chop wood), while `hovered` stays empty there. Like
`Input_PickTargetAction` (`0x00480E80`), a rank falls through if no object action fits its last accepted hit
(`0x00480600`); the next rank measures against its distance: a tree in front of a foreign building is chopped, a
tree behind a foreign building is not (ground, "Go to"). Without a selection no object has an action.
As in the ray test of the original (`FUN_0046CEA0`), all objects with visible render bits (`& 0x65`) are hit, and
invisible ones with module 6 (resource): a fishing basket before the first fishing is clickable (fishing 398),
occupants are not.
The viewer discards ground actions outside the map like `Input_IssueTargetAction`. The command goes to the whole
selection if its first object belongs to the active player - workers on a tree chop wood (`0x06`), on a gold
mine/farm/fishing basket mine/harvest/fish, on an own construction site build (`0x02`), with a load deliver to a
matching storage (`0x09`), at a quarter quarter in (`0x13`), a selected building on the ground rally point (`0x27`),
otherwise go (`0x04`). Shift queues (`0x7B`), Ctrl cyclic (`0x7C`), Shift takes precedence - only for the actions
the original wraps (398, 400-402, 412/413/569, 444-446, 448); quartering and rally point always go directly. If no
action fits, nothing happens. "428 Follow" (`0x74`, right-click on an own unit, also on a selected one whose hull the
ray hits) as well as "418 Gut" and "419 Hunt" (`0x16`/`0x17`, on a wild animal or its carcass), "410 Garrison attack"
and "411 Attack" (`0x5B`, `0x11`, on memory images `0x71`) exist. With a selection, the overlay shows the action under
the mouse (`Right-click: 400 Chop wood (command 0x06)` or `Right-click: no action`).

`KeyT` opens/closes the egui window **"Economy"**: header with the balance of the active player; with own selected
workers the **build list** (build menu of the type from `toolbar.ini` including the submenus of its buttons -
`TARGET_DO` and those without a kind -, per entry name, cost - red if the balance is not enough - and, for locked
ones, the reason of the command check: prerequisite missing, only once); with an own building the **production
list** (training, trade and research, also those from submenus like 600; GUIDs locked by the level data are missing),
the **queue** with progress of the running order and "x" per entry (cancel `0x0F`, index 0 = running, n = n-th
waiting); to the right of it, set off by spacing and a separator and outlined with a dashed red line, the **debug
button "DBG now"** (not a game feature, tooltip "Debug: complete order immediately (not a game feature)"): command
`DebugCompleteOrder` with index and type of the entry like `sim_complete`, enabled only for living (not sleeping)
buildings of the active player (check like A8, `validate::object_is_dead`), otherwise disabled with a hint line; the
**quarters** ("Let out all" `0x15`, "Let out" per occupant `0x14`), the rally point and **Sell** (`0x67` only on the
second click). A click in the build list starts the **placement mode**: as in the original
(`Cursor3D_RenderWorldModel` `0x004CD130`) the whole building model with sub-objects, opaque, lit and textured, is
drawn at the position and rotation after the building site check (tower on the wall, field/mine at the source),
after the objects and without shadow, all materials (also the player color) multiplicatively tinted green
`(0, 1, 0)` if valid, otherwise red `(1, 0, 0)` (minimum corner of the hull on the tile boundary, building site check
with the layer and region of the first selected own object). Only with the path grid overlay (`KeyG`) switched on does
the viewer additionally show, as an aid, the checked tile rectangle and a cross at the snapped position (same
colors); the original has neither. The preview does not touch the simulation.
Overlay line `Build: guid name, Rotation ... degrees, position (x, z) = tile (...), valid|invalid | R: rotate,
Esc/Right-click: end` (for locked types `Rotation locked` and without the hint for R). Left click places (`0x01` with all own selected objects as
workers and the snapped position) and ends the mode, Shift+left click places queued and stays in the mode; `KeyR`
rotates by 90 degrees (deviation: the original rotates by dragging in 90-degree steps). Types with the building flags
`& 0x10000000` (tower, gate, field, gold mine, Spanish shipyard 1008) are never rotated by the build cursor of the
original (`BuildCursor_UpdatePlacement` `0x0044FDB0`): `KeyR` then has no effect. After each rotation the preview
snaps the rotated hull to the tile boundary again (the original does not snap again while dragging, only the rotation
changes; only the next placement snaps with the new rotation) - for non-square buildings the rotated position is
therefore different, in the steady state the same. `Escape` or a right-click ends the mode (`Escape` then does **not**
end the viewer), as do closing the window or a selection without own objects. Walls are placed only as single pieces
like a building (dragging chains `0x18` is not part of this). An own selected building shows its rally point as a
yellow line with a flag (rendering only). Over the control channel: `KeyR` rotates, a held `ShiftLeft` or
`ControlLeft` on a right or left click queues or cycles (protocol 9); without the mouse path the same commands go
through `sim_build`, `sim_action`, `sim_train`, `sim_cancel`, `sim_sell` and `sim_menu`, the debug button "complete
immediately" through `sim_complete`.

Mouse events of the control channel (`mouse_move`, `mouse_button`) also reach egui on the map (debug and economy
windows) and take effect in the next frame; a press over an egui window does not reach the map (as with the user),
a release always does. Press and release before the same `step` are one click. Two clicks within 200 ms of viewer
time (`DoubleClickTimeout`, `controls.double_click_ms`) are a double-click (second release after the first, on the
map, selection like `0x0048A0C0`); with a stopped clock this means: two clicks without a `step` in between are a
double-click, between separate clicks set at least 200 ms (e.g. 5 steps of 50 ms) of `step`. On the right a separate
clock counts: two right-clicks within the double-click time (with a stopped clock without a `step` in between) are a
right double-click; it issues the right-click action a second time and makes "Go to" and attack run (quirk D-227).
`sim_move` and `sim_action` have no parameter for this: run with `sim_command` `Move` and `flags: 1` (mounts need
stamina, a freshly created horse has 0, quirk K-236).

Commands that arrive before the level is loaded are executed afterward; only `hello` is answered immediately.
`Escape` over the control channel ends the viewer without waiting for the response (except in placement mode) - use
`quit` to exit.

**AI queries** (protocol 23; `ki_query` and the new fields of `sim_query` change nothing, the checksum stays the
same):

- `ki_query` poses a query of the AI interface (request codes `0x4E20`...`0x5130`) against the running savegame, with
  the sight of a slot. `sight` `"omniscient"` (default) reads everything like the AI of the original, `"fog"` only what
  the slot sees. The request is the word field `[code, argc, [2], ...]`; `words` fills `[2]` onward, `argc` is their
  number. Without `words`, `[player]` applies (`argc` 1), which does not fit most codes and then leaves the field
  unchanged.
- **On `request`:** The response always contains the whole field with `2 + max(argc, 128)` words (padded with 0),
  never a shortened list; this also makes it possible to read codes that write beyond their inputs without an `argc`
  check.
- **On `direct`:** The query facade returns direct changes (set or delete the AI mark on the object, take an attack
  report) as a list, and only the AI tick applies them. `ki_query` **never** applies them and only reports them. The
  original also marks IDs that no longer exist, without effect; `ki_query` does not report such marks (check against
  the object table without visibility: dead objects count, missing ones do not).
- `sim_query` shows per object `ai_marks` (slots 0-7 with the bit of the AI mark `GameObject+4` set, ascending; absent
  if none) and per player `attack_reports` (victim ID `victim`, tile `tile` of the last hit, in the order of the MFC
  table of the original, in which `0x512E` chooses at equal distance; absent if empty). The simulation fills attack
  reports only for victims on slots with control kind AI.
- MCP: action `ki_query` of `viewer_sim` with `player`, `code`, `words`, `sight`. The parameters appear only in a new
  MCP session after rebuilding `opennml-viewer-mcp`; until then `ki_query` goes through the raw control channel.

Example (real responses of the viewer; IDs `id` renumbered, `request` shortened to the first ten words, the response
has 130). Loaded is the savegame `ai_fire.nmlsave` on `[4players]-exile`: slot 1 is AI, three warriors (GUID 1400) of
slot 0 have attacked its main house 2049 on tile (28, 21) and a settler, two of the warriors have been marked by the
AI. `sim_query` shows this as follows (excerpt):

```json
{"id": 1, "cmd": "sim_query", "limit": 10000}
{"id": 1, "ok": {..., "objects": [... {"id": 1572865, "guid": 1400, "owner": 0, "tile": [31, 23], "ai_marks": [1], ...},
                 {"id": 1638401, "guid": 1400, "owner": 0, "tile": [25, 23], "ai_marks": [1], ...} ...],
                 "players": [{...}, {"control": "ai", ..., "attack_reports": [{"tile": [34, 27], "victim": 67585},
                                                                          {"tile": [28, 21], "victim": 2049}]}, ...]}}
```

The marking nearest search `0x4E24` (20004) with `[marking slot, reference player, x, z, head filter ID, follow-up
filter ID]` around the main house finds the third, not yet marked warrior: head filter ID 0 matches no entry and
becomes, as a head, "all objects of the reference player 0" (quirk E-52), the follow-up filter ID 1400 (>= 1000) is the
GUID filter of the warrior; marked objects are skipped by the search (bit `0x80000`). The mark is only reported.
`0x512E` (20782) with `[slot, x, z]` would take the next report - here that of the main house (distance 0) - and
returns its tile:

```json
{"id": 2, "cmd": "ki_query", "player": 1, "code": 20004, "words": [1, 0, 28, 21, 0, 1400]}
{"id": 2, "ok": {"code": 20004, "direct": [{"kind": "mark", "object": 1703937, "player": 1}],
                 "request": [20004, 6, 1703937, 0, 28, 21, 0, 1400, 0, 0]}}
{"id": 3, "cmd": "ki_query", "player": 1, "code": 20782, "words": [1, 28, 21], "sight": "fog"}
{"id": 3, "ok": {"code": 20782, "direct": [{"kind": "take_report", "object": 2049, "player": 1}],
                 "request": [20782, 3, 1, 28, 21, 0, 0, 0, 0, 0]}}
```

Afterward `sim_hash`, the marks and the report table are unchanged. All new fields are absent in responses before
protocol 23 and then read as empty.

Control channel: `words` instead of `args`, MCP through `viewer_sim` action `ki_query` or (since protocol 26)
`viewer_ki` action `query` with `slot` instead of `player`.

**AI match** (protocol 26):

- **Setup:** `sim_setup` with `slots` (lobby slots in order, each `{"kind": "closed"|"human"|"ai", "nation",
  "team", "color", "handicap", "ki_kind"}`, everything except `kind` optional), `ki_enabled` and
  `campaign_difficulty`. The slots go in order onto the active `.lvl` slots (a `closed` consumes the slot); slots
  behind the list stay like the default of the map (human, nation of the `.lvl`, team and color = slot). The AI kind
  1-4 sets the handicap like the original (1 -> 5, 2 -> 2, 3 -> 0, 4 -> 0; `opennml_sim::setup::ki_kind_handicap`), a
  free handicap 0-5 (the six stages of the lobby) excludes it. `ki_enabled` starts the AI on restart like the
  original: one engine per occupied slot, per AI slot `setschwierigkeitsgrad` (351152) synchronously and - except in
  the campaign - `multi_startplayer` (195235) as a thread with `[slot, nation, 5 - handicap]` (King of the Hill:
  stage 5). Viewer default: on (Rust test scenes: off); checksums of earlier viewer check runs and scripts are
  therefore comparable only with `--ki off`. An AI slot without `ki_kind` and without `handicap` gets the handicap of
  the map default (0). The values stay in the settings for later restarts; `slots` replaces `nations`/`teams` and vice
  versa. Errors without a restart (error message): `slots` together with `nations` or `teams`, more than 8 entries or
  more than the lobby of the map has, `ki_kind` outside 1-4 or not with `ai`, `ki_kind` and `handicap` together,
  `handicap` outside 0-5, `nation` outside 0-5, `color` outside 0-8 (8 = white), `slots` on a campaign map or in game
  type -1, `campaign_difficulty` outside 0-2. In the settings file the fields have the same names (`ki_enabled`,
  `campaign_difficulty`, `[[slots]]`). Command line: `--slots` as a comma list (`c` closed, `h` human, `a1`...`a4`
  AI kind, `a:h0`...`a:h5` AI with a free handicap, e.g. `--slots h,a3,h,a3`), nation and team still through
  `--nations`/`--teams` (in order into the slots; more entries than slots is an error), `--ki on|off` (default `on`),
  `--campaign-difficulty 0-2`. In the "Simulation" window (key `KeyI`), the button "Match ..." opens the "Match"
  window: per lobby slot kind, nation, team, color and AI kind or handicap, plus AI on/off and difficulty; "Restart"
  takes the same path as `sim_setup` (errors appear in the window, the match then stays unchanged).
- **`ki_state`** only reads (the checksum stays the same): `paused` (the whole AI paused); per engine ascending by slot
  `slot`, `next_thread_id`, `threads` in execution order (MFC order of the thread table) with `id`, `module`, `name`
  (from `nml.db`, empty without an entry), `pc`, `status` (1 running, 4 finished, 0x10 waiting for the child),
  `pause_ms`, `wait_semaphore` (-1 none), `stack_depth`, `stack_top` (the top 8 values at most, topmost last) and
  `child` (running `CALLM`, absent otherwise); `semaphores` as `[key, occupied]`; with `values: true` also `values` =
  `m_iValues` as `[key, value]` ascending (otherwise empty). Plus the counters of the AI commands from the events
  `SimEvent::KiCommand`: `rejected` (per slot, network command `code` 0x01...0x7E and reason `reason`, rejections
  only) and `commands` (per slot, `code` and `outcome` `accepted`/`partial`/`rejected`). The counters are host state,
  not part of the simulation: restart, load and replay clear them; commands of the AI start in `new_match` do not count
  (their events lapse with the setup). `slot` shows only this slot.
- **`ki_command`** issues interventions as commands of the simulation: `start` -> `KiStart { slot, module, args, sync }`
  (like the console `runmodule` or `.abl` `n0` with `sync: true`, `n1` without), `pause` -> `KiPause { paused }`,
  `step` -> `KiStep { slot, passes }`. The sender is the active player (irrelevant). They take effect at the next tick
  boundary (with a stopped clock with `step`), are in the command log and run along in replays (AI commands from the
  modules, by contrast, do not: they arise anew on every playback). Response `{"step"}`. The viewer first checks what
  does not change until the tick boundary (slot with an engine, module in the catalog, limits) and whether the AI is
  paused by then (for `step`, counting the `pause` already waiting); the simulation checks again and otherwise rejects.
  `ki_command` does not report such a rejection at the tick boundary (the response was already `{"step"}`): it is
  recognizable only from `ki_state` (`paused` unchanged, threads without a step) and in the debug log of the
  simulation. The counters `rejected`/`commands` count only AI commands from the modules, no control commands.
  From within an AI step (world builtin), all three control commands are rejected with "AI control command in the AI
  step".
- **Single step:** The world keeps running tick by tick; with the AI paused (`pause`), `step` executes `passes` passes
  at the tick boundary in which **each thread of the engine of `slot` takes exactly one step with `dt = 20`** (like
  the debugger of the original `0x4B0`; a sleep counts down by 20 ms per step). No new state: checksums and
  savegames do not change as a result. With the AI running, the simulation rejects `KiStep` with "AI is running".
- **Limits:** `args` at most 64, `passes` 1-1000, slots 0-7; an unknown module, a slot without an engine or the thread
  limit 512 reject the command without changing the state.
- **"AI" window** (key `KeyK`, egui): reads the same data as `ki_state` (the checksum stays the same): per engine the
  threads in execution order (ID, module ID and name, program counter, status, pause, semaphore, stack depth with the
  top values; a running `CALLM` child indented under the caller), `m_iValues` with a search field (key or value
  contains the text), semaphores, rejected commands per slot and reason, and the command statistics. The buttons
  "Pause"/"Continue" and "Step" (slot, passes) go as `ki_command` `pause`/`step` into the simulation (at the next tick,
  in the command log); errors of the pre-check appear in the window.
- **AI overlay** (key `KeyL`, like `ToggleShowKISelection` of the original): switches off -> AI slots with an engine
  (control kind AI, ascending) -> off; frame (magenta) around every object with the `ai_marks` bit of the slot set and
  a drawn model, plus a line at the top with the slot and the number of marked objects. Rendering only.
- MCP: tool `viewer_ki` with `action` `state`/`query`/`start`/`pause`/`step`; `viewer_sim setup` with `slots`,
  `ki_enabled`, `campaign_difficulty`. New parameters appear only in a new MCP session after rebuilding
  `opennml-viewer-mcp`; until then the same commands go through the raw control channel. Unlike the control channel,
  `viewer_ki` silently ignores fields that do not belong to the chosen action (e.g. `passes` with `pause`) instead of
  reporting an error.

Example (raw control channel, clock stopped): set up a match with AI on slot 1, pause the AI, two single steps, read
the state, resume.

```json
{"id": 1, "cmd": "sim_setup", "slots": [{"kind": "human"}, {"kind": "ai", "nation": 1, "ki_kind": 3}], "ki_enabled": true}
{"id": 2, "cmd": "ki_command", "action": "pause"}
{"id": 3, "cmd": "step", "frames": 1, "frame_ms": 50}
{"id": 4, "cmd": "ki_command", "action": "step", "slot": 1, "passes": 2}
{"id": 5, "cmd": "step", "frames": 1, "frame_ms": 50}
{"id": 6, "cmd": "ki_state", "slot": 1}
{"id": 7, "cmd": "ki_command", "action": "pause", "paused": false}
```


**Scripts** (protocol 28; world state protocol 30):

- **Setup:** `sim_setup` with `script_enabled` (start the level script `.abl` of the map) and `time_per_char`
  (speaking time per character without a speech file in ms, 1-1000, like `Engine.ini` `TimePerChar`; default 55).
  Viewer default: **off** (`--mission` and "Start" in the "Match" window switch it on for the mission). Both values
  start a new match and stay in the settings for later restarts; in the settings file they have the same names
  (`script_enabled`, `time_per_char`). In the "Match" window (button "Match ..." in the "Simulation" window, key
  `KeyI`) there are "Start level script" and the speaking time. Errors without a restart (error message):
  `time_per_char` outside 1-1000; a script of the map that the load check rejects (message "Script {path}, line
  {line}: ..."). Without a script there is no container; a map without `.abl` has none even when switched on. When
  **loading the map** with `script_enabled = true` from the settings file, such a script does not lead to a load
  error: the viewer warns in the log ("Script not loaded, match without script (load errors: {n}): ...") and starts
  the match without a script (`script_state.enabled` = `false`); the setting stays on, so a later restart through
  `sim_setup` reports the load error again (switch off with `script_enabled: false`).
- **`script_state`** only reads (state and checksum stay the same): `enabled` (script on in the setup),
  `dialog_running` (a dialog is running, AI and level script rest), `containers` (first the level container, then the
  dialog container) each with `id` (level 1, ...; dialog -1; since protocol 30 the level container has ID 1 as in the
  original), `path`, `counter` (countdown in ms, -1 off), the name spaces `switches`, `timers` (ms), `variables`,
  `random_switches` as `[name, value]` ascending and `blocks` (remaining blocks in file order: `index` in the file,
  `name` = `[BESCHREIBUNG` text from the 16th character like `m_TriggerName`, `state` `"bereit"`/`"wartet"`/
  `"wiederholbar"` (action 10), `cur_action`, `wait` only with `"wartet"`, e.g. `"Action 2 (w40;1000): 900 ms
  remaining"` or `"Action 0 (w150;4711): until the text line ends"`); `speech` with `line` (`text`, `remaining_ms`,
  `slot`; absent without a line), `tips` (slots 0 and 1: `remaining_ms`, `text`), `events` (text events "text ended"
  in insertion order) and `info_history`; `names` = object names as `[name, value]` in the order of the name table (MFC
  order), with `names` only those whose name or value contains the text (the text is lowercased like the names); `log`
  = block log of the last `log` steps (default 50, at most 1,000, 0 = empty).
- **Condition values:** With `block`, under `conditions` each condition of the block has `number`, `or` (prefix other
  than `&`; with the first one without effect), `params` and `value`: `true`/`false` or `null` = not yet implemented
  (false in the tick with `ConditionPending`; there is no such number anymore). Numbers without a case are `false` as
  in the tick. Evaluation is in order on **a copy** of the state (`opennml_sim::script::condition_values`), because
  conditions have side effects (19 consumes text events, query 2500 clears event bits): a second 19 on the same ID
  sees the event already consumed. It is the state between two steps (the tick first counts the timers down). A block
  of the file that has already been deleted still has values. Errors: container not running, block behind the end of
  the file.
- **Block log:** The viewer collects the messages `SimEvent::Script` of the last 1,000 steps (host state like the AI
  counters: restart, load and replay clear it). Each entry has `step` (1-based: the step in which it was reported,
  `steps` afterward) and `trace` = JSON form of `opennml_sim::script::ScriptTrace`: `{"Fired": {"container", "block"}}`
  (block executes its actions), `{"Deleted": {...}}`, `{"Suspended": {"container", "block", "action"}}` (stops),
  `{"Pending": {"container", "block", "action"}}` (action still without effect), `{"ConditionPending": {"container",
  "block", "condition"}}`, `{"DialogStarted": {"path"}}`, `"DialogEnded"`, `{"Guarded": {"container", "block",
  "number", "why"}}` (run-time error, no effect), `{"Limit": {"what"}}`; entries in report order.
- **World state of the scripts** (protocol 30; state in the savegame, queries leave the checksum unchanged):
  `sim_query` gets per object `name` (script name from the name table, reverse lookup like the original; absent
  without a name) and `hidden` (action 159/161: the slot is hidden and the object is not shown individually; absent if
  not; the viewer and client hide hidden objects only with the presentation), plus `kill_counts` = kill counters as
  `[slot 0-7, type GUID of the victim, count]` (ascending), `button_locks` = build button lock of the scripts as
  `[type GUID, 0/1]` (global for all slots), `slot_running` = eleven values, `false` = slot 0-10 hidden, and
  `area_views` = named vision rectangles `[{"name", "rect": [x0, z0, x1, z1], "player", "clear"}]` in append order
  (rectangle unnormalized as created; `clear` also stamps "cleared"). `script_state` gets `main_quests` and
  `sub_quests` (always ten entries `{"status", "text"}`: status 0 empty, 1 present, 3 set, 6 completed, 10 failed; text
  = text ID), `countdown_container` (ID of the container whose counter is displayed; 0 none) with `countdown_s`
  (seconds, `counter / 1000` after the container tick), `task_status` (action 200, `[task, 0/1]`) and `area_overrides`
  (areas changed by scripts, `[name, [x0, z0, x1, z1]]`, take precedence over the rectangles of the map). Older
  responses without these fields stay readable (`default`). Conditions that have side effects when read (condition 6
  normalizes the area, query 2500 clears event bits) run for `conditions` only on the copy: only the tick itself
  changes `area_overrides`.
- **"Script" window** (key `KeyJ` or button "Script ..." in the "Simulation" window, egui): reads the same data as
  `script_state`: per container path, countdown, block table (a click on the block index selects the block, again
  deselects) and name spaces; the conditions of the selected block with catalog text and value; speech state; object
  names with a search field; block log (newest message first, number of steps adjustable); section "Mission
  objectives" (protocol 30: table of the set main and sub-objectives with status as text - set, completed, failed,
  present -, below it the countdown display, task status and changed areas). Does not intervene.
- MCP: `viewer_sim setup` with `script_enabled` and `time_per_char`; the tool `viewer_script` reads `script_state`
  (see "Presentation (protocol 33)"). It appears in the schema only after rebuilding the MCP server, in a new session;
  until then `script_state` goes through the raw control channel.

### Script commands (protocol 32)

- **Campaign start in the viewer:** `--mission <name>` (file stem of the level from `data/gifcon/campaign.ini`,
  case-insensitive, all four campaigns: `kamp0-1` ... `kamp3-14`) loads this level and sets it up like the campaign
  start of the game (`match_request::campaign_setup`): game type -1 without a lobby (`slots`, `nations`, `teams` do not
  apply), AI and level script on, difficulty from `--campaign-difficulty` (default 1), speaking time from the
  settings. An explicit `--script off` wins over the switching on. `--mission` excludes `--level`, `--game-mode`,
  `--slots`, `--nations` and `--teams`. An unknown name is an error before window and GPU:
  `Mission "x" unknown (known: kamp0-1, kamp0-2, kamp1-1, ...)`. In the "Match" window (key `KeyI`, button
  "Match ...") the list "Mission" (tutorial and campaigns 1-3) with the campaign difficulty above it chooses the
  level; "Start" loads it as a new match like `--mission` (on an error the running match stays; the message appears in
  the window). The control channel has no command for this: `sim_setup` restarts only the map that is currently
  running.
- **`script_command`** `{"action": "skip_dialog"}`: queues `CommandKind::ScriptSkipDialog` of the active player for
  the next tick (response `{"step"}` like `sim_command`; error `script_command skip_dialog: no dialog running`). The
  dialog container ends in the same tick without a further dialog tick (`DialogEnded` in the block log), the level
  script continues in the following step. The command is in the command log of the host; a replay from the start
  (`sim_save` with `replay`, `sim_load`) yields the same checksum sequence. `ScriptSkipDialog` has no fields (JSON
  form `"ScriptSkipDialog"`, not an object: `{"player": p, "kind": "ScriptSkipDialog"}`). It also works through
  `sim_command`, but there **without a pre-check**: the command is always queued, and without a running dialog only
  the simulation rejects it in the tick ("no dialog running").
- **`sim_command` `DebugDestroy`** `{"player": p, "kind": {"DebugDestroy": {"objects": [ids]}}}`: a check aid without
  a counterpart in the original (like `DebugCompleteOrder`): destroys own objects of the sender, units like `Die`,
  buildings through the normal destruction (quirk E-227). This makes it possible to reach victory and defeat of a
  mission without long battles. A script action can never send it: under `Origin::Script` it rejects with "not from
  scripts" (no A8 on this path, otherwise foreign objects would fall).
- **`script_state`** gets:
  - in `log` the entry `{"Command": {"container", "block", "action", "code", "outcome"}}` for every command a script
    action sends (script path): `code` is the command type of the original (`0x3C` = 60 Halt), `outcome` `"Accepted"`,
    `{"Partial": "reason"}` or `{"Rejected": "reason"}`. (The JSON form is that of `ScriptTrace`, like the other
    messages: variant capitalized.) An action without a command (unknown name, n = 0) stays silent, as in the
    original;
  - `commands` = `{"actions": [{"action", "accepted", "partial", "rejected"}], "rejections": [{"step", "container",
    "block", "action", "code", "reason"}]}`: counters per action number (ascending, only actions with at least one
    command) and the last 20 rejections with reason (oldest first). Host state like the block log (restart, load and
    replay clear it), independent of the parameter `log`;
  - `results` = `[[slot, result], ...]` for the decided slots (1 lost, 2 won), ascending; playing (0) and unoccupied
    slots (3) are absent.
  Older responses without `commands` and `results` stay readable. **Like `script_state` itself, the new fields leave
  the checksum unchanged.**
- **"Script" window:** new sections "Script commands (n accepted or partial, m rejected)" (table per action with
  accepted/partial/rejected, below it the last rejections with block, command and reason, newest first) and "Game
  result" (slots with the result lost/won).
- MCP: The tool `viewer_script` (see "Presentation (protocol 33)") sends `script_command` (`skip_dialog`) and starts
  missions (`start`); until the MCP server is rebuilt, `script_command` and `--mission` go through the raw control
  channel or the start arguments (`viewer_start args`).
- **Skipping and presentation:** `skip_dialog` ends the dialog without executing its remaining blocks - like the skip
  flag of the original (E-226). If only the end of the dialog switches cinemascope and lock off (148/19, e.g. the
  introduction of `tutorial0-1`), both stay on afterward: the level continues in cinemascope at game speed 10 and with
  locked controls. To check the controls, let the dialog run through.

Example (raw control channel): start a mission (`viewer_start` with `args: ["--mission", "kamp1-1"]`), until a dialog
runs, and skip it.

```json
{"id": 1, "cmd": "script_state", "log": 0}
{"id": 2, "cmd": "script_command", "action": "skip_dialog"}
{"id": 3, "cmd": "step", "frames": 1, "frame_ms": 50}
```

Example (raw control channel, clock stopped): switch the script on, two steps, state with the conditions of block 3
and the names that contain "held".

```json
{"id": 1, "cmd": "sim_setup", "script_enabled": true}
{"id": 2, "cmd": "step", "frames": 2, "frame_ms": 50}
{"id": 3, "cmd": "script_state", "block": {"container": 1, "index": 3}, "names": "held", "log": 2}
```

Response (shortened, values for illustration):

```json
{"id": 3, "ok": {"enabled": true, "dialog_running": false,
  "containers": [{"id": 1, "path": "data/levels/kampagne1/kamp1-1/kamp1-1.abl", "counter": -1,
                  "switches": [["start", 1]], "timers": [], "variables": [], "random_switches": [],
                  "blocks": [{"index": 3, "name": "Held tot]", "state": "bereit", "cur_action": 0}]}],
  "conditions": [{"number": 24, "or": true, "params": ["held"], "value": false}],
  "speech": {"tips": [{"remaining_ms": 0, "text": 0}, {"remaining_ms": 0, "text": 0}], "events": [],
             "info_history": []},
  "names": [["held", "65537"]],
  "log": [{"step": 1, "trace": {"Fired": {"container": 1, "block": 0}}}],
  "main_quests": [{"status": 3, "text": 105}, {"status": 0, "text": 0}, "... ten entries"],
  "sub_quests": ["... ten entries"], "countdown_container": 0, "countdown_s": 0,
  "task_status": [], "area_overrides": []}}
```

### Presentation (protocol 33)

- **`script_events`** `{"steps": 0...1000}` (default 50): the presentation events (`SimEvent::Present`) of the last
  `steps` steps, the camera of the scripts and the state of the UI. Response
  `{"events": [{"step", "event"}], "presentation", "ui", "profile_vars", "camera"}`:
  - `events`: oldest first, `step` 1-based like the block log, `event` in the JSON form of
    `opennml_sim::script::Presentation` (e.g. `{"Cinemascope": {"on": true}}`, `{"Speech": {"text", "slot", "ms"}}`,
    `"QuickSave"`, `"LightRestoreLevel"`). Host state like the block log: the events of the last 1,000 steps (at most
    4,096), restart, load and replay clear them;
  - `presentation`: the presentation state (`MissionState::presentation`: `light`, `weather`, `music`, `attack_music`,
    `attack_threshold`, `distance_fog`, `fog`, `fog_show_all`, `faded_out`, `portraits`, `info_frames`, `highlight`);
  - `ui` = `{"ui_locked", "cinemascope", "selection", "last_button"}` (see below);
  - `profile_vars`: profile variables of the match as `[name, value]`, ascending by name;
  - `camera`: the camera of the scripts (JSON form of `ScriptCamera`: `backup`, `moves`, `rest_ms`, `eye_speed`,
    `target_speed`, `rotation`, `bind`, `pending_cut`). It belongs to the session, not the simulation; the viewer
    puts it into the response.
  Error: `steps = 1001 invalid: allowed 0 to 1000 steps`. **Leaves the checksum unchanged.** Only in the viewer:
  `opennml-game` does not get `script_events` (like `script_state` it stays outside `AVAILABLE`); there only
  `sim_command` with `ScriptSelection`/`ScriptButton` works.
- **`script_state.ui`** = `{"ui_locked", "cinemascope", "selection": [ids], "last_button"}`: control lock (actions
  18/19), cinemascope (147/148), the selection mirror (at most 30 IDs in report order) and the last button (0 = none).
  Older responses without `ui` stay readable.
- **`sim_command`** accepts the host commands of the conditions 35-38: `{"player": 0, "kind": {"ScriptSelection":
  {"ids": [2048]}}}` sets the selection mirror (more than 30 IDs and the invalid ID `4294967295` = `ObjectId::NONE`
  are rejected by the simulation, the mirror then stays as it was), `{"player": 0, "kind": {"ScriptButton":
  {"word": 511}}}` the last button. They take effect at the next tick and are in the command log. The host sends
  `ScriptSelection` itself when a script is running and the level uses the conditions 35-38: per frame before the
  steps the selection of the session, only on a change, as a set (`SimHost::sync_selection`). The host sends
  `ScriptButton` for every button of the command bar (`GameSession::button_pressed` -> `SimHost::press_button`, also
  only with a script and 35-38); in addition `sim_command` sets the last button. **The selection of the host is
  authoritative:** in a level with 35-38 the sync resets a `ScriptSelection` from `sim_command` to the selection of
  the viewer in the same step (it differs from the last queued command, so `sync_selection` queues it behind). The
  control channel therefore does not set the mirror directly there; for checks, make the selection through the map
  image (`mouse_move`, `mouse_button`; e.g. a click on Umak makes lesson A fire). In levels without 35-38 the host
  sends nothing, a raw command stays there. The host does not sync `ScriptButton`, the command always takes effect.
- **"Script" window** (key `KeyJ`): new section "Presentation" with the running line including its text from the text
  table, the tips, the info windows, the portraits, the highlight, the control lock, cinemascope, "Screen: faded
  out"/"visible", light/weather/music, the selection mirror with the last button and the camera of the scripts; plus
  "Events" (the messages of the last 50 steps, at most 50 lines, newest first).
- **Map image:** cinemascope draws a black bar at the top and bottom, each 170/1024 of the window height (`BARSIZE` of
  the original); the bars of the game UI are off.
- **Game end:** If the active slot has won or lost, the viewer shows "Victory" or "Defeat" in the center and sends the
  simulation **no more steps** (frames and `step` keep running, the clock of the simulation stands). With the steps,
  the camera of the scripts (a running move stops), the player camera, the fog crossfade and the sound of the map also
  rest (`GameSession::advance_by` with `running = false`). A restart (`sim_setup`), `sim_load` or a new mission resets
  the result. This applies to **every match** of the viewer, with or without a script, campaign like free play: every
  viewer match is a single-player game (one local slot, no network match), so the host stops at the game end in every
  game on the machine, like `opennml-game` (end screen after `GameFlow::check_result`, free play and campaign). The **active slot** is decisive: an AI match that is supposed to run beyond the
  defeat of this slot switches with `sim_view` `active_player` to an undecided slot, then the steps keep running (check scripts that send `step`
  after the game end otherwise see a stopped clock: check `sim_hash` `steps`).
- **MCP `viewer_script`** `action`: `state` (`script_state` without the block log; + `block_container`/`block_index`,
  `names`), `log` (only the block log, + `steps`), `events` (`script_events`, + `steps`), `skip_dialog`
  (`script_command`), `start` (`mission`, + `difficulty` 0-2, `profile`, `paused`): first `viewer_stop`, then
  `viewer_start` with `--mission <name> --campaign-difficulty <n> --mute --window-mode windowed`. New tool parameters
  appear in the schema only in a new session; until then the same commands go through the raw control channel.
- A mission that the viewer starts with `--mission` gets no profile variables (`profile_vars` empty; the variables of
  the player are read from the profile only by `opennml-game`).

Example (raw control channel, clock stopped, mission `kamp1-1` started): 200 steps, then the events of the last 100
steps and the UI state.

```json
{"id": 1, "cmd": "step", "frames": 200, "frame_ms": 50}
{"id": 2, "cmd": "script_events", "steps": 100}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"ScriptButton": {"word": 511}}}}
```

## Virtual clock

Map (animations, water in 25 ms steps, weather, simulation), model browser and camera hang on one clock:

- **Real time** (start without `--paused`): frame time = real time since the last frame, at most 100 ms.
- **Stopped** (`pause`): The clock stands. Drawing continues (object under the mouse, overlay), but no logic is
  executed: mouse movements with a pressed button (free camera, rotating in the browser) and a running mouse wheel
  transition wait for the next step.
- **`step`**: `frames` logic steps of `frame_ms` each, then one frame (not with a minimized window); the clock stays
  stopped afterward. The same inputs and steps give the same states and pixel-identical images, also across restarts -
  provided the session starts stopped at 0 ms (`--paused`; `viewer_start` does this by default). Otherwise the clock
  runs with real time until the first `pause`.
- The space-bar pause of the browser is independent of this and also runs on this clock.
- **Sound**: Every logic step mixes exactly `frame_ms * 44.1` stereo frames (fractions carry over), also in `step`;
  while stopped nothing is mixed and the device plays silence. The same steps give bit-identical offline mixes
  (`audio_capture`, `--audio-capture`), also 10 + 20 instead of 30 steps. In real time a ring buffer (at most 250 ms)
  balances the real playback with silence or discarding, never with a speed change.

- **Simulation**: Every logic step brings `frame_ms * (GameSpeed + 10) / 20` of game time (`GameSpeed` 14: 1.2 times);
  one engine tick per full 50 ms, at most two per logic step, the rest lapses as in the original. Commands take effect
  at the next engine tick. The viewer shows the position and animations of the objects interpolated between the
  previous and the last tick.
- **Economy:** After the simulation line, the overlay shows the balance of the active player (`Balance player p: Gold
  ... | Wood ... | Food ... | Population occupied/living space`, living space clamped to the limit) and its messages
  (`messages.ini` -> `texte.deu`, in the color of the display kind, 5 s of the virtual clock each, at most three).
  Credits float for 2 s above the delivering object (color of the resource kind, rising 100 units per second, fading),
  only above own objects. The display follows the render bits of the simulation: construction sites grow with the
  construction progress (with scaffold first the scaffold, then the building, cut off at the height), burning
  buildings darken from the top, destroyed ones sink (by game time; the clock comes from the timers of the
  destruction, so a loaded savegame keeps sinking where it was; sleeping palisades under a tower do not sink), ships
  and fishing baskets show their scaffold (`OBJ_SCAFFOLD_FILENAME`) with the model, fishing baskets appear at the first
  fishing (invisible before, but clickable), occupants are invisible and neither clickable nor in the selection.
  Sounds of the simulation (building, chopping, finished, destroyed, tree falls, messages) are played by the viewer as
  far as the sound database knows the number; effects of the simulation (explosion) start as a world effect.

FPS and frame times in the state snapshot measure real time.

## Input

- Keys are kept separately: held by the user (`keys_real`) and held through the control channel (`keys_control`);
  both together are effective. When the window loses focus, only the keys of the user are released.
- `tap` presses and releases immediately: takes effect exactly once like a key press.
- `down` with `repeat`: after the press, `repeat` further press events with the repeat flag set, like holding by
  hand (autorepeat). In the viewer the repeats affect only the key path of the original (`--hud`), its own keys react
  only to the first press; in `opennml-game` every repeat is a separate key event (quirk O-300).
- Mouse pointer and mouse buttons are shared by the user and the control channel; the last input counts.
  **Mouse movements of the user over the window change the result of a running check** (e.g. edge scrolling, object
  under the mouse). Deviations are visible in the state snapshot.

## State snapshot

`view` (`map`/`browser`/`effects`), `window` (mode, monitor, number of monitors, window and surface size, minimized,
covered), `clock` (`realtime`/`paused`, `now_ms`), `frames` (FPS, frame times of the last 120 shown frames, shown
frames), `camera` (distance, tilt, rotation, target, eye of the original camera; stands still with the free camera),
`free_camera`, `hovered` (type, GUID, index, player, position, model, base module `base_module`, created parts
`parts`, shadow kind `shadow_kind` 0-3, ID of the simulation object `sim_id`; `index` is the position in the object
list of the map: ambient effects of the `.wld` first, then the simulation objects), `toggles` (fog, aspect ratio,
animation, help), `browser` (model, animation with time and duration, pause, crossfade, remaining time of the 200 ms
lock, remembered switch, `shadow_kind`, object type `type_guid`, `shadows` as below, fire intensity `damage`, preview
switch `preview`, hit zones `hit_zones`, `effects` as below, sound actions of the type `sound_actions` and the chosen
`sound_action`), `effect_browser` (file, index, count, error message, run time `time_ms` since selection or restart,
`.hef` radius, camera, `effects` as below; `null` as long as it has not been created), `environment` (light `level` or
name of the preset, weather `none`/`rain`/`snow`, active weather particles, those moved in the last step and those
drawn in the last frame, `shadow_quality`, `particles_quality`, `terrain_quality`; `null` without a map), `shadows`
(`quality`; `kinds` = objects per shadow kind 0-3; `decals`, `groundplanes`, `specials` (gold deposits), `persistent`
(kind 2), `per_frame` (kind 3) drawn in the last frame; `rendered` = shadow textures rendered in this frame - kind 2
only on a camera change, becoming visible, a stage change (F8/`set_quality`), a missing free atlas cell in the
original or a prepared but not drawn frame; otherwise it comes from the atlas; `atlas_used`/`atlas_capacity` in cells
of 128^2, `atlas_full` = texture shadow without a free cell; `null` without a map). F8 on the map does not force a
frame (unlike `set_quality`): until the next frame, `shadows` shows the new `kinds` with zeroed counters, so do a
`step` after F8. In the model browser, by contrast, F8 and the model change take effect immediately:
`browser.shadow_kind` names the new kind without a frame.
`effects` (map; `null` without a map): `quality` (`particles_quality` at setup; in `browser.effects` and
`effect_browser.effects` always 2, the browsers create all effects), `owned` (own effects of the objects: permanent
effects, locomotive smoke, fishing basket fish), `world` (world effects with `.pef`: `schiff_stehend`,
`spawn_effect`, HEF), `surf` (surf of the ships; the water rings of the units count in `world_display.surf_rings`),
`fish`, `eagles` (ambient effects), `particles` (live particles), `frozen` (frozen according to the 200 ms rule),
`quads`/`models` (drawn in the last frame). Effects run in the game tick of 100 ms of game time and per logic step;
the game tick starts with the first step, so world effects appear only after the first tick. `cursor` (mouse, mouse
buttons, held keys per source), `overlay` (lines of the last frame, on the map including balance, messages and
floating credits). `world_display` (map with simulation, otherwise `null`): flag, ground marks, wheel tracks, water
rings, silhouettes, beams and dynamite charges, see "World rendering".
`audio`: `device` (output device, `null` without a device), `mute`, `volumes` (music, sounds, speech 0-1000),
`listener` (listener in tiles `x/128, 0, z/128`: camera target of the game camera, in the browsers the origin),
`frames` (mixed frames), `voices` per occupied channel in mixing order (`id`, `channel`, `file`, `category`
`music`/`sfx`/`speech`/`fixed` (fixed volume without a slider: Bink sound and speech track of the videos,
`Category::Fixed`), `stream`, `position` for 3D sounds, `volume` = FMOD volume 0-255, `vol3d`, `pan` 0-255 or `null`
for stereo, `gains` = gain left/right 0-255 in the next 1024 block, `fading`), `music` (`status`
`script`/`normal`/`attack`, running file `playing`, normal and combat music `current`/`attack_track`, `attack_enabled`
(combat music by hand, Shift+F9 or `music attack`: override of the simulation), `attack_points` (the attack points
the music state machine received last: value of the active player from the simulation) and `attack_source` (`sim` or
`manual` on override; protocol 11), timer `timer_ms`, lock `pause_ms`, running menu music `menu`), `capture` (running
recording: path, frames), `unit_response` (`Engine.ini UnitResponse`), `responses` (counter of the acknowledgment
variants, `GameWorld+0x2424`), `select_lock` (type GUID of the lock of the selection sound, `Ui+0x24`),
`recent_voices` (the last 8 requested selection sounds and command acknowledgments, oldest first: `kind`
`select`/`response`, `object`, `guid` (type), `action` 100/103/12/14, `flags`, `category`, `index` = variant -1
random or counter value, `file` = started file or `null` if the database has no voice or category 8 is occupied),
`recent_sounds` (the last 16 requested combat sounds of the simulation, `SimEvent::Sound3d`, oldest first:
`{"action", "flags", "guid", "object"}` - action of the sound database, flags of the call, type GUID and ID of the
object; protocol 11; the sound itself runs without a fog check).

**Acknowledgments on the map:** Every release of the left mouse button on the map (click and box) and every
shrinking of the selection by the game (a selected object disappears or is quartered; a change of owner does not
deselect in the viewer; the dead are deselected) plays the selection sound if exactly one object of the active player
or of slot 8 is selected: units as a voice (`*_select_*`, category speech, only with `unit_response`), buildings and
resources as a sound (construction sites action 103), these not twice in a row for the same type GUID. Commands
speak in the tick of their execution (`SimEvent::UnitResponse`): direct commands per own unit, move once, chop wood
also queued; only the first voice plays, while one is running further ones are rejected. Check runs stay muted
(`--mute`); `recent_voices` shows what was requested.
`sim` (`null` without a map or if the rules of the map cannot be loaded; the map then shows the objects of the
`.wld`): `tick` (object tick, 100 ms), `steps` (engine ticks, 50 ms), `mode`, `seed`, `game_speed`, `active_player`,
`checksum` (16 hex digits), `objects`, `selection` (IDs in selection order), `commands` (recorded commands),
`grid_layer` (pathfinding overlay: 0-8 layer, 9 occupancy, `null` off), `path` (displayed path as a line: path preview
or `sim_path`), `moving` (objects with the run bit), `attack_points` (attack points per target owner 0-7, as last
reported: `SimEvent::AttackPoints` per double tick; the value stays until the next event, also in pause) and
`attack_points_active` (value of the active player that the music state machine gets per frame; protocol 11),
`wood_price`/`food_price` (market prices `m_WoodPrice`/`m_FoodPrice`, initial values 1024/1351), `max_living_space`
(living space limit of the match), `notices` (the last 16 messages of the active player at most, oldest first:
`{"step", "id", "tile", "text"}` - engine tick, number of the `messages.ini`, tile `[x, z]` or `[-1, -1]`, text from
`texte.deu`; from protocol 22 also the text messages of the headhunter 10002, 10010 and 10011 with their `texte.deu`
number as `id`, tile `[-1, -1]` and text `null`, see "Headhunters and robbing"; in the state snapshot with text, in
the responses of `sim_setup`/`sim_load` without; loading and restart clear the list), `target_marker` (target marker
after a right-click or `sim_action` on an object: `{"id", "remaining_ms"}` - ID and remaining time of the 300 ms on
the **virtual clock**, 0 in the last valid instant; `null` without one or expired; ground targets set nothing, a new
object target replaces it, a destroyed object deletes it - even without a frame in between the state snapshot then
reports `null`; deviation from the original, which measures in real time, so that checks are repeatable),
`ground_markers` (IDs of the objects with a ground marker in the last frame in drawing order: selection - for riders
the mount -, target marker, object under the mouse; an object can appear twice). Simulation and `sim_hash` stay
untouched by both (protocol 9). `fog_shown` (fog on; `false` = "Fog off", key `KeyO`, `sim_view`) and `fog_overlay`
(fog overlay 1/3/7, `null` off; key `KeyF`) belong to the view of the viewer, not to the simulation (protocol 10).

**Groups in the state snapshot** (protocol 13; read only, the checksum stays the same). Section `sim`: `groups` (all
groups of the manager in map order) and `formation` (chosen formation of the unit bar 0-4, viewer state). Per group
`id`, `kind` (0 without a formation - also the secondary group for rank 0 -, 1 line, 2 square, 3 column, 4 pincer, 5
like 0 with speed matching without compression, 6 unordered unit, also takes rank 0), `type_id` (`vf04`, type 5
reports 0), `members` (head = leader), `anchor` (`[x, z, direction]` in fixed point; x = -1 -> centroid), `marching`,
`matched_speed`, `targets` (`{"id", "x", "z", "dir"}` per member) and for formations `formation` = `{"spacing",
"depth", "layer", "rear" (secondary group or -1), "rear_members", "frames" (number of march frames), "ready",
"reported", "arrived", "end_max", "end_min"}` (ends as object ID, 4294967295 = none). Per unit with a walk module there
are `walk.group` (`null` without a group) and `walk.formation_num` (rank). The debug window (key `KeyI`) has the
section "Groups" for this and, in the object rows, "Group" and "Rank".

**Unit bar and formation dragging** (protocol 13):

- "Unit" window (egui, bottom left), as soon as the selection contains own units with a walk module: buttons 511 none,
  512 line, 513 square, 514 column, 515 pincer and 519 scatter with the texts from `data/txt/guids.deu` (if a text is
  missing, "Formation 5nn" appears); header from text 510 with the name of the chosen formation. A formation button
  remembers the formation and, with more than one own unit, issues `SetFormation` (`0x44`, `Toolbar_SubmitFormation`
  `0x0044ADE0`); "Scatter" issues `Scatter` (`0x75`, `0x0044BC30`). Over the control channel, `sim_unit_bar` acts on
  the selection in the same way.
- The chosen formation is **viewer state** (in the original `GameWorld+0x20B8`): it is neither in the savegame nor in
  the checksum; the simulation never reads it. Every go command of the UI (right-click "Go to" 448, attack march 427,
  `sim_move`, `sim_action`) carries it as `formation`; without a choice on the bar it is 0. On loading a savegame or
  restarting it stays as it is.
- Right-click dragging (only with the game camera; with the free camera, dragging stays a camera rotation) like the
  original (mouse move handler `0x00453790`): The action runs as before at the press point. Every mouse movement with
  the button held measures from the ground point at the press to the ground point under the mouse (world
  coordinates); if it is more than 16 world units (3D length) away, `direction = atan2(dz, dx)` in degrees applies -
  **the formation faces the drag direction** (the line stands across the dragged line) - and `spread = min(1 +
  length/256, 2)` (fixed point; 2.0 from two tiles). The last such movement also applies if the mouse then moves
  closer to the press point again (quirk K-263). Only "Go to" (448) gets the dragging, the attack march (427, key X)
  only the formation. Without dragging, `direction` stays "none" and `spread` -1. Over the control channel: `sim_move`
  with `drag_to` (release point `[x, z]`) computes the same from `to` to `drag_to`, with height 0 (only the horizontal
  distance counts); example: `"to": [2560, 2560], "drag_to": [2816, 2560]` -> facing 0 degrees (toward +x), `spread`
  2.0.
- The operative ground point under the mouse (right-click, dragging, hint, path preview, build preview) is, as in the
  original (`FUN_004CCF00`), the water surface over water (`y = WATER_LEVEL`), not the lake bed.
- Group commands also go raw through `sim_command` (`SetFormation` `0x44` only with more than one unit, `formation`
  0-4 and 6; `Scatter` `0x75`):

```json
{"id": 1, "cmd": "sim_command", "command": {"player": 0, "kind": {"SetFormation": {"units": [65537, 65538, 65539], "formation": 2}}}}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"Scatter": {"units": [65537, 65538, 65539]}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"Move": {"units": [65537, 65538], "x": 40960, "z": 24576, "target": -1, "formation": 1, "direction": 368640, "spread": 5734, "flags": 0}}}}
```

`Move` optionally takes `formation` (default 0) and `spread` (default -1); `direction` -819200 means "no direction".

**Riding and diving** (protocol 14; read only or through commands, the checksum changes only through the commands
themselves):

- Bar buttons in the "Unit" window and through `sim_unit_bar`: **405 Dismount**, **406 Disembark**, **432 Dive**,
  **433 Surface** (texts from `data/txt/guids.deu`; fallbacks "Dismount", "Disembark", "Dive", "Surface"). A button
  appears if the condition holds for at least one **own unit with a walk module** in the selection: 405 with
  `ISMOUNTED` (`0x2000`, rider), 406 with `ISMOUNTEDONBOAT` (`0x8000`, canoe occupant), 432 with module 24 without
  `DIVING` (`0x20000`), 433 with `DIVING`. The mount and the canoe themselves get no button. A click issues the
  command with **all own units of the selection** (not only the matching ones); who is affected is decided by the
  simulation (it ignores riders without a mount, non-divers, etc.). `sim_unit_bar` answers as with the formation
  buttons with `{"step", "formation"}`; the chosen formation stays untouched.
- Commands raw through `sim_command` (ownership check of the simulation): `Mount` (`0x0A`, mounting the horse
  `horse`), `Dismount` (`0x0B`), `BoardBoat` (`0x0C`, boarding the canoe `boat`, the target is not checked),
  `LeaveBoat` (`0x0D`), `Dive` (`0x1B`) and `Surface` (`0x1C`):

```json
{"id": 1, "cmd": "sim_command", "command": {"player": 0, "kind": {"Mount": {"horse": 131073, "units": [65537]}}}}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"Dismount": {"units": [65537]}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"BoardBoat": {"boat": 131074, "units": [65538]}}}}
{"id": 4, "cmd": "sim_command", "command": {"player": 0, "kind": {"LeaveBoat": {"units": [65538]}}}}
{"id": 5, "cmd": "sim_command", "command": {"player": 0, "kind": {"Dive": {"units": [65539]}}}}
{"id": 6, "cmd": "sim_command", "command": {"player": 0, "kind": {"Surface": {"units": [65539]}}}}
```

- Section `riding` per object in `sim_query` (list; one entry per present module 13 and 14, 13 first; absent in the
  JSON without a riding module): `index` (13 rider on a horse, 14 occupant in a canoe), `boat` (module 14),
  `mount_target` (ID of the mount or canoe, also during the approach; `null` = none), `going_to_horse` (approach to
  the mount is running), `busy` (`m_isBusy`), `next_mount_anim` (0 left, 1 right, otherwise none) and
  `lance_available` (`m_lanceAvailable`; the lance itself is not built).
- Section `diving` per object (`null`/absent without module 24): `status` (0 up, 1 down, 2 sinking, 3 rising), `count`
  (dive depth -1024 ... 0), `orig_layer` and `dive_layer` (walk layer before and while diving), `air` (current value of
  attribute 28, stamina) and `height_offset` (depth offset of the object in fixed point, `object.height_offset`; the
  model stands `height_offset * cell height / 4096` lower). In the module list the behaviors are called `riding`,
  `swim` and `diving` (modules 13/14, 15, 24). Both sections are absent in responses before protocol 14 and then read
  as empty.

**Ships, trains and carts** (protocol 15; read only or through commands, the checksum changes only through the
commands themselves):

- Bar buttons in the "Unit" window and through `sim_unit_bar`: **449 Train forward**, **450 Train backward**,
  **451 Train stop** (texts from `data/txt/guids.deu`; fallbacks "Forward", "Backward", "Halt"). They appear for the
  **first own object of the selection with train module 33 whose type lists the buttons in `toolbar.ini`** (in the
  shipped data only the locomotive 2613; tender and wagons have none): 449 and 450 while the locomotive stands, 451
  while it is moving (status bit `MOVE`, `targets.ini`). A locomotive alone shows only these buttons, without a
  formation row; with own units they come after the other buttons. The bar sends the train commands like the original
  with `len = 1` (quirk K-88): with several locomotives in the selection only the **first** applies. A click issues
  `Train` (`0x29`, `0x2A`, `0x2B`) without a target tile; `sim_unit_bar` answers as with the other buttons with
  `{"step", "formation"}` and also sends the command when the bar does not currently show the button (e.g. 449 with a
  moving locomotive). The simulation checks the owner and the train module like for every train command (exception
  A8).
- The command raw through `sim_command` (`op` `Forward`, `Backward` or `Stop`; `target` optionally as target tile
  `[x, z]`, in the shipped data only from scripts):

```json
{"id": 1, "cmd": "sim_command", "command": {"player": 0, "kind": {"Train": {"train": 65537, "op": "Forward", "target": null}}}}
```

- Section `train` per object in `sim_query` (absent without module 33; locomotive, tender and wagons): `slaves` (only
  the locomotive; IDs of the tenders and wagons in coupling order), `master` (locomotive of a slave, otherwise
  `4294967295` = none), `speed` and `next_speed` (distance per tick, fixed point), `acc_time` (time index of the
  drive curve in ms), `status` (bit 0 accelerating, 1 driving, 2 braking, 3 forward, 4 backward), `is_slave`, `dir`
  (drive vector, fixed point x, z), `target` (target tile or `null`), `time_counter` (remainder until self-departure
  in ms), `has_supply`, `outside` (0 in the map, 1 out backward, 2 out forward), `raster_dir` and `last_block_tile`.
  In the module list the behavior is called `train`.
- Section `multipart` per object (absent without module 11 and 51; carts): `head` (front part, module 11; otherwise
  rear part, module 51), `linked` (rear part of the front part), `prev` and `next` (front and further rear part of the
  rear part), `offset` (shaft length, fixed point), `centre` (pulled-along shaft point x, z, direction) and `last_pos`
  (last reported position of the front part). Non-applicable IDs are `4294967295`, non-applicable numbers 0. Behaviors
  `multihead` and `multipart`.
- Why `4294967295` instead of `null`: `master`, `linked`, `prev` and `next` return the IDs of the state unchanged as a
  number (`ObjectId::NONE` = `u32::MAX`), like `end_max`/`end_min` in the section `group` (protocol 13); so every
  section stays a fixed set of integers that can be compared with the savegame without a special case. Where a section
  reports a missing ID as a statement of its own (`target`, `mount_target`, `meeting_point`), `null` appears.
- `event_bits` per object (`SimState::object_event_bits`; absent at 0): bit 0 halt, 8 out forward, 9 out backward
  (train).
- In the `quarter` section of mobile containers (ship, wagon): `meeting_point` (meeting point, fixed point x, z, or
  `null`), `waiting` (those waiting at the meeting point) and `status` (0 free, 1 going to the meeting point, 2
  arrived). In the section `ranged.shoot` the broadside: `is_steuerboard` (chosen side), `cannon_counter` (cannon count
  + 1 after aiming) and `can_shoot`. All fields are absent in responses before protocol 15 and then read as empty.

**Upgrades and arming** (protocol 16; read only or through commands, the checksum changes only through the commands
themselves):

- `sim_upgrade` issues the console commands `0x39` (`ApplyUpgrade`) and `0x3A` (`RemoveUpgrade`). **The sender is the
  active player**; `player` names only the **target slot** (`slot`) of the command and can be 0-10, also foreign
  slots, so that quirks like W-213 stay testable: slot 8 (neutral) writes the type table, it is never read. Like the
  original, the command checks neither ownership nor prerequisite nor menu. Applying with `guid` < 0 applies all GUIDs
  2800 to 3399 (`0x39` with a negative GUID); removing needs a GUID > 0; a slot from 11 is already rejected by the
  control channel (the simulation would reject it too). Removing acts only on players 0-7, other slots stay without
  effect (original). The effect begins at the next tick; the response `{"step"}` names the engine tick of acceptance.
- Bar buttons through `sim_unit_bar` and in the "Unit" window, from the menus of the `toolbar.ini` of the selected
  own types including submenus (the same rule as the command check); locked buttons are grayed out:
  - **485 Arm** and **486 Disarm**: `ToggleArming` (`0x38`/`0x72`) to the selected own objects with module 39 that list
    the button in the menu and are in the matching state (485 unarmed, 486 armed). The unit walks to the nearest
    friendly storage and toggles `ISARMED` on arrival.
  - **Abilities 543, 544, 545, 549, 552, 557, 559, 560, 561, 563** (`ABILITY_BUTTONS`): `CastSpell` (`0x36`, kind 4 with
    a fixed `anim`/`idx` per button) to every selected own wizard (module 38) whose menu lists the button.
  - **Counter-upgrades 3370-3377** (`COUNTERUPGRADE_MSG`, submenu 601, directly as buttons without the submenu):
    `CounterUpgrade` (`0x2F`) with the first own selected object as the source; if the GUID is already running (for
    all players, quirk W-226), the button is locked.
  - For these buttons `sim_unit_bar` reports no error if no object fits: then no command goes out. Only an unknown
    button number is an error.
- `sim_query` shows per player `upgrades` (`applied_upgrades`: applied GUIDs in order of application, with duplicates;
  absent if empty; the start applies time-of-day upgrades). In the result there are `counter_upgrades` (running
  counter-upgrades in insertion order: `guid`, `rest_ms`, `source` as a raw ID; the source may be missing if the
  building died) and `daytime` (`m_TerrainUpgradeID`, -1 none yet, otherwise 0-3).
- Per object (all fields absent if empty or without the module):
  - `temp_upgrades`: list of temporary upgrades `{"guid", "flags", "rest_ms"}` (flags from the record of the
    `tempupgrades.ini`, remaining time in ms; the tick subtracts 100 ms per object tick).
  - `show_counter_upgrade`: `m_ShowCounterUpgrade`, the number of running counter-upgrades that the object shows
    (non-negative; 0 is absent).
  - `armed`: `ISARMED` of the arming module 39 (`true`/`false`); only for objects with the module (module 39 is in
    `modules` with the behavior `inert`).
  - `spell` (spell module 38): `{"art", "button", "cost", "anim", "idx", "reserve", "mana", "mana_max", "casting"}`:
    fields of the module, `mana`/`mana_max` = current value and maximum of the extra energy (attribute 15, with the
    upgrade percentages `P` of the owner), `casting` = status `ANIM_LOCKED` (the spell is running).
  - `exec_list`: execution list of module 44 (GUIDs of temporary upgrades), `idx` of the spell of kind 4 points into
    it.
- Values in `combat` and `resource` (hit points, deposits) are computed with the upgrade percentages `P` of the owner
  (`GameWorld_GetTypeUpgradePercent`); the sections are otherwise unchanged.
- Further commands through `sim_command`: `CounterUpgrade`, `CastSpell`, `ExecTempUpgrade`, `ToggleArming`,
  `SetArrowType` (`0x40`/`0x41`, always rejected from the player: no bar generates it). All check like the bar
  (ownership, menu, lock) and reject with a message in the debug log (simulation unchanged, checksum the same).

Examples:

```json
{"id": 1, "cmd": "sim_upgrade", "player": 0, "guid": 2806}
{"id": 2, "cmd": "sim_upgrade", "player": 0, "guid": 2806, "remove": true}
{"id": 3, "cmd": "sim_unit_bar", "button": 485}
```

All fields are absent in responses before protocol 16 and then read as empty.

**Tents and trade** (protocol 17; read only or through commands, the checksum changes only through the commands
themselves):

- Packing a tent and unpacking a travois (Prairie Indians), from the menus of the `toolbar.ini` of the selected own
  objects (like the bar, the simulation checks locks); `sim_unit_bar` and the "Unit" window:
  - **452 Pack tent**: `PackTent` (`0x2D`, `{"PackTent": {"tent": id}}` through `sim_command`) only for the **first**
    selected object if it is an own, selectable tent (module 42). The tent fades out for 2500 ms (`tent.status` 1),
    creates the travois (type from 430001) and then goes into building sleep; the selection passes to the travois at
    the end of the fade-out.
  - **453 Unpack travois**: opens the **unpack cursor** (no command, the response stays `{"step", "formation"}`) if
    the first selected object is an own travois with a tent. The cursor shows the tent in its rotation as a build
    preview; a left click on a valid site issues `UnpackTent` (`0x2E`, `{"UnpackTent": {"unit": travois, "x": fx, "z":
    fx}}`, fixed-point tiles, tile * 4096) to the travois: it walks to the target and sets up the tent there (fade-in
    2500 ms, `tent.status` 2); an invalid site issues no command. Esc and right-click end the cursor, as does a
    selection without a carrying travois; it excludes building placement. At the end of the fade-in the selection goes
    back to the tent (only if it is empty) and the travois disappears.
  - Both commands check like the bar and reject with a message in the debug log (simulation unchanged, checksum the
    same); the original checks nothing.
- `sim_trade` and `sim_tribute` (raw commands `0x45` and `0x65`; the UI of the original does not reach `0x45` in the
  shipped data, quirk W-245). **The sender is `player`**, not the active player (unlike `sim_upgrade`); so the
  rejections of the simulation and the ignoring of foreign units stay testable. Response `{"step"}` in each case, the
  effect begins at the next tick.
  - `sim_trade`: `units` empty = selection. The simulation checks like the partner menu: `ware` 0-3, `target` 0-8 and
    not `player`, the target slot owns a trading building (not dead or quartered); otherwise it rejects (debug log).
    Foreign units and units without a trade module are **ignored**, not rejected (quirk K-289). The cart then shuttles
    between the home HQ and the partner HQ.
  - `sim_tribute`: `player` is giver and sender. Rejected are amount <= 0 and a resource outside 0-2; the simulation
    does not check the recipients (also unoccupied slots 0-7 get the amount, quirk C-231). If the stock of the giver
    does not suffice for the full amount, it reports the shortage and ends; later recipients get nothing. Every
    payment shows message 230 (giver and recipient) at every active player.
- `sim_query` shows per object (all fields absent without the module; raw IDs without an object are `4294967295` =
  `ObjectId::NONE`):
  - `tent` (module 42): `{"status", "counter", "progress", "travois_guid", "travois", "hidden"}`: `status` 0 rests, 1
    fade-out, 2 fade-in; `counter` the remaining time of the crossfade in ms (2500 to 0); `progress` the crossfade
    factor of the object (fade-out 1.0 -> 0.0, fade-in 0.0 -> 1.0); `travois_guid` from 430001; `travois` the last
    created travois (stays after setting up and may point to a removed object); `hidden` = building sleep and not
    selectable (the packed tent).
  - `transport_tent` (module 35, the travois): `{"tent", "transporting"}`: the carried tent as a raw ID and the flag
    `TRANSPORTINGTENT` (`0x100`).
  - `trade` (module 43, the cart): `{"ware", "target", "tax_level", "max_tax", "tax_interval", "payers",
    "load_amount", "load_kind"}`: trade kind and partner slot of the running trip, the tax slider (0-100, start 50),
    maximum tax, tax interval in ms, payer GUIDs and the load of the cart (amount and resource index; the carts have
    no gatherer module, so the load is here and not in the section `gatherer`).
  - `in_collision` (only objects with module 3): is the building in the collision manager? A packed tent is removed,
    and a tent that is set up again on its tile also stays so (quirk W-89).
- Memory images of the tents in the fog follow the object as soon as their place is seen; the selection sorts heroes
  according to `heroes.ini`.

Examples:

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 452}
{"id": 2, "cmd": "sim_trade", "player": 0, "units": [65537], "ware": 2, "target": 1}
{"id": 3, "cmd": "sim_tribute", "player": 0, "recipients": 6, "resource": 0, "amount": 100}
{"id": 4, "cmd": "sim_command", "command": {"player": 0, "kind": {"UnpackTent": {"unit": 65538, "x": 61440, "z": 61440}}}}
```

All fields are absent in responses before protocol 17 and then read as empty.

**Gates and enemy watch** (protocol 18; read only or through commands, the checksum changes only through the commands
themselves):

- Opening and closing gates from the menu of the selected object (`toolbar.ini`, type 1013 and the other gates);
  `sim_unit_bar` and the "Unit" window:
  - **430 Open gate**: `OpenGate` (`0x19`, `{"OpenGate": {"gate": id}}` through `sim_command`) for the **first**
    selected object if it is an own, finished gate in the state `TORCLOSE`.
  - **431 Close gate**: `CloseGate` (`0x1A`, `{"CloseGate": {"gate": id}}`) if it is in the state `TOROPEN`.
  - Both commands check like the bar (own, living building with `TOR`, button in the menu of the type and not locked)
    and reject with a message in the debug log (simulation unchanged, checksum the same). A construction site does not
    carry `TOR` and therefore has no buttons, and `sim_query` shows no `gate` section for it; the only exception to
    the section is the case of quirk K-292 (`TORCLOSE` on a construction site, reachable only through scripts), there
    are no buttons there either. `OpenGate` and `CloseGate` also go through `sim_command`; the effect begins at the
    next tick.
- `sim_query` shows per building (all three fields are absent without content and then read as empty):
  - `gate` (only with `TOR` or a gate bit): `{"open", "closed", "doors"}`: `open` = `TOROPEN`, `closed` = `TORCLOSE`;
    `doors` = the door tiles (tiles of the hull rectangle with mask byte 0, z outside, x inside) as `{"x", "z",
    "blocked"}`, `blocked` = the tile is blocked in the path grid (layer 0). A closed gate blocks its door tiles, an
    open one frees them; after the death of the whole wall ring the tiles of a closed gate stay blocked (quirk K-293,
    hypothesis).
  - `area_watch` (only with a running query, `check_area_id != -1`): `{"key", "mode", "rect", "player", "members"}`:
    key of the area query, filter mode (`0xA410` = enemy watch, `0x4A010` = capture watch, as a number), rectangle
    `[x0, z0, x1, z1]` in tiles (inclusive), reference player (`< 0` = owner) and the listed objects as raw IDs in
    entry order (a corpse of an enemy stays listed, quirk K-296). The watchers of the buildings fire at `count >
    threshold` (`GREATER`, threshold 0, so as soon as the list is no longer empty); quirk K-49 concerns only the time of
    the first check of a newly attached watcher.
  - `watch_rect`: the stored watch rectangle `[x0, z0, x1, z1]` (`check_area_rect`), also when the query has ended
    (`area_watch` is then absent); absent if it contains only zeros.
- The queries change nothing: `sim_query` leaves the checksum the same.

Examples:

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 431}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"OpenGate": {"gate": 65538}}}}
{"id": 3, "cmd": "sim_query", "ids": [65538]}
```

All fields are absent in responses before protocol 18 and then read as empty.

**Camouflage, digging in, healing** (protocol 19; read only or through commands, the checksum changes only through
the commands themselves):

- Bar buttons through `sim_unit_bar` and in the "Unit" window, from the menus of the `toolbar.ini` of the selected own
  objects (the same rule as the command check: button in the menu, `REQUIRED_GUID` fulfilled, otherwise grayed out,
  and the state filter of `targets.ini` fits); the commands of these buttons go to **all** selected own objects, not
  only the first (the buttons of protocol 20, by contrast, follow only the first, see "Ghosts, banishing, animal
  evocation, auras"):
  - **434 Camouflage** (`Mask`, `0x1D`) and **435 Uncamouflage** (`Unmask`, `0x1E`): 434 appears only while the
    object does not have `MASKED` and is not swimming, 435 only with `MASKED`. Camouflaging requires the upgrade 2924
    (Indians) or 3220 (scouts). During the camouflage or uncamouflage animation the bar keeps showing the button; it
    is then without effect (quirk W-277).
  - **437 Dig in** (`DigIn`, `0x20`) and **438 Dig out** (`DigOut`, `0x21`): 437 appears only without
    `WAITING4KILLING`, 438 only with it. Digging in requires the upgrade 2922 (assassins).
  - **458 and 554 Start healing** (`StartHealing`, `0x34`), **459 and 564 Stop healing** (`StopHealing`, `0x35`):
    458/554 appear only without the status `HEAL`, 459/564 only with it. 458 belongs to the medic, 554 and 564 to the
    pilgrim (554 requires the upgrade 3125).
  - **555 Self-healing** (`SelfHeal`, `0x60`, `{"SelfHeal": {"unit": id, "button": 555}}`): one command per selected
    object whose menu lists the button; the simulation deducts the cost (value set of the button GUID, attribute 15);
    with too little mana nothing happens (no rejection, no message).
  - The viewer sends the command to all own selected objects (555 only to those with the button in the menu); the
    simulation checks per object like the bar (module switched on, button in the menu, prerequisite, state filter)
    and rejects what does not fit (message in the debug log, state and checksum stay the same). `Mask`, `Unmask`,
    `DigIn`, `DigOut`, `StartHealing`, `StopHealing` and `SelfHeal` also go through `sim_command`; the effect begins
    at the next tick.
- `sim_query` shows per object (each field is absent without content and then reads as empty; IDs without an object
  are `null`):
  - `masking` (module 25): `{"is_masked", "is_busy", "mask_time", "never_unmasked", "energy"}`: `is_masked` =
    camouflaging has begun (not only with the status `MASKED`), `is_busy` = the camouflage or uncamouflage animation
    is running, `mask_time` in ms (duration of camouflaging **and** uncamouflaging), `never_unmasked` =
    `EVERYTIMETARNED`, `energy` = fraction of the extra energy (attribute 15) 0-100,000.
  - `assassin` (module 28): `{"is_down", "target", "attack_points"}`: `is_down` = dug in (from the end of the dig-in
    animation), `target` = target of the running stab (`null` without; after the death of the target the old ID stays),
    `attack_points` = damage of one stab.
  - `healing` (module 29): `{"ready", "single", "sick", "new_target", "healing_points", "mana"}`: `ready` = heals in
    this tick, `single` = single healing (`SINGLEHEALING`, the healer units) instead of area healing, `sick` = target
    of the single healing and `new_target` = requested target (each `null` without, also the start ID 0),
    `healing_points` = hit points per minute, `mana` = current value of the extra energy (attribute 15, with the
    upgrade percentages of the owner; a freshly created healer starts with 0).
  - `message_filter`: `{"mode", "codes"}` of the message filter of the object (`GameObject+0x730`), **only if it
    differs from the default** (mode 1, empty list), e.g. for the dug-in and in the trance. Mode 0 = only the codes of
    the list are accepted, mode 1 = the list is forbidden; the codes appear as decimal numbers (`540` = `0x21C`, `272`
    = `0x110`, `541` = `0x21D`).
  - `look`: `{"anim", "alpha"}`, the display of a camouflage (status `MASKED`) **from the point of view of the player
    chosen in the viewer** (`sim_view` `active_player`; `sim_query` carries no player of its own): `alpha` 0.5 for a
    camouflaged unit and for the owner (or whoever shares its vision) of an alarm post, 1.0 for an alarm post that a
    stranger sees; `anim` is then the substitute animation (only the tree), otherwise `null` = the animation of the
    simulation. Without camouflage `look` is absent.
- The queries change nothing: `sim_query` leaves the checksum the same.

Examples:

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 434}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"DigIn": {"units": [65538]}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"SelfHeal": {"unit": 65539, "button": 555}}}}
{"id": 4, "cmd": "sim_query", "ids": [65537, 65538, 65539]}
```

Excerpts of the response to 4 (archer camouflaged, assassin dug in with a target, healer in the trance):

```json
{"id": 65537, "masking": {"is_masked": true, "is_busy": false, "mask_time": 1600, "never_unmasked": false, "energy": 62500},
 "look": {"anim": null, "alpha": 0.5}}
{"id": 65538, "assassin": {"is_down": true, "target": 131073, "attack_points": 250},
 "message_filter": {"mode": 0, "codes": [540, 272]}}
{"id": 65539, "healing": {"ready": true, "single": true, "sick": null, "new_target": 131073, "healing_points": 288, "mana": 475},
 "message_filter": {"mode": 0, "codes": [541]}}
```

All fields are absent in responses before protocol 19 and then read as empty.

**Ghosts, banishing, animal evocation, auras** (protocol 20; read only or through commands, the checksum changes
only through the commands themselves):

- Bar buttons through `sim_unit_bar` and in the "Unit" window, from the menu of the `toolbar.ini` of the **first
  selected own object**. Unlike the buttons of the section "Camouflage, digging in, healing", which go to all selected
  own objects, these follow only the first, like the tent buttons (the bar sends `len = 1`; no contradiction to that
  section, which describes only its own buttons). The button appears only if the menu of the type lists it and the
  target filter of `targets.ini` fits (436 and 441-443: property evoker and not `SWIMMING`; the banish buttons have no
  entry); if the prerequisite (`REQUIRED_GUID`) is missing, it is grayed out:
  - **436 Evoke ghost** (`EvokeGhost`, `0x1F`, `{"EvokeGhost": {"unit": id, "button": 436}}`): requires the upgrade
    2930; the simulation looks for a corpse within ten tiles and binds the cost of the button (value set of the button
    GUID, attribute 15) until the ghost dies (quirk C-257).
  - **441 Evoke wolf, 442 bear, 443 wildcat** (`EvokeAnimal`, `0x22`-`0x24`, `{"EvokeAnimal": {"unit": id, "art": 0,
    "button": 441}}`, `art` 0 wolf, 1 bear, 2 wildcat): require the upgrades 2926, 2928 and 2927; the costs (25, 50 and
    30) stay bound as a reserve as long as the animal lives.
  - **439, 547, 550, 553, 558 and 562 Banish** (`Banish`, `0x25`, `{"Banish": {"unit": id, "button": 547}}`): 439
    belongs to the missionary (upgrade 2827), 547 to the ghost evokers (2931), 550 to the chiefs (3029), 553 to the
    pilgrims (3124, cost 200), 558 to the flag bearers (3223) and 562 to the undertakers (3324).
  - The simulation checks again like the bar (`check_ability`: module switched on, button in the menu, prerequisite,
    mana) and rejects what does not fit (message in the debug log, state and checksum stay the same). The commands
    also go through `sim_command`; the effect begins at the next tick.
- `sim_query` shows per object (each field is absent without content and then reads as empty; IDs without an object
  are `null`, stale IDs stay):
  - `evoke_ghost` (module 26, kind `evokeghost`): `{"evoking", "dead_body", "ghost", "extra_energy", "mana",
    "mana_base"}`: `evoking` = status `EVOKING` (the evocation is running), `dead_body` = corpse of the evocation,
    `ghost` = last created ghost (never reset and, after its death, points to a removed object), `extra_energy` = cost
    of the last attempt, `mana` = current value of the extra energy (attribute 15, with the upgrade percentages of the
    owner), `mana_base` = its base value: a living ghost binds its cost from it, its death and the abort return it
    (quirk C-257).
  - `ghost` (module 27, kind `ghost`): `{"status", "alpha", "target"}`: `status` 0 calm, 1 appearing, 2 fading out, 3
    suction hit due; `alpha` = opacity while fading out (resting and appearing 1.0); `target` = suction target (`null`
    without; stays set until the end of the sucking or the death of the ghost, also with a dead target).
  - `banish` (module 31, kind `banish`): `{"active", "effects", "spell_guid", "radius"}`: `active` = status
    `ANIM_LOCKED` of the object - the banish animation is running, **but also the healing trance**, because both use the
    same bit (quirk C-254); `effects` = the timer `0x41F` is running, the banishing spell is shown; `spell_guid` =
    button of the last banishing (0 before the first); `radius` in fixed point (1/4096 tile), as with `0x21F`: sight
    (attribute 18, with the upgrade percentages of the owner) * 4096 >> 3, with sight 52 therefore 26,624 (6.5 tiles).
  - `evoke_animal` (module 32, kind `evokeanimal`): `{"evoking", "art", "cost", "reserve", "spawn", "mana"}`: `evoking`
    = status `EVOKING`, `art` 0 wolf, 1 bear, 2 wildcat (the last attempt), `cost` = cost of the running evocation,
    `reserve` = sum of the costs of living animals and the running evocation, `spawn` = spawn position `[x, y, z]` in
    fixed point (1/4096 tile, searched at the start of the effect; before the first attempt `[0, 0, 0]`), `mana` as
    with `evoke_ghost`.
  - `influence` (module 44 with influence, only with a non-empty influence list or an existing set): `{"guids",
    "influenced", "ring"}`: `guids` = GUIDs of the running influences (auras) in insertion order, `influenced` = IDs of
    those influenced (`null` without a set, so before the first aura tick; the set contains every object with walk in
    the radius, the carrier itself included, and may contain stale IDs), `ring` = the aura ring is shown (non-empty
    influence list and set, or a pending aura tick). The execution list `exec_list` stays as before under "Upgrades
    and arming".
  - In the module list of `sim_query`, the kinds of modules 26, 27, 31 and 32 are called `evokeghost`, `ghost`,
    `banish` and `evokeanimal`.
- The queries change nothing: `sim_query` leaves the checksum the same.

Examples:

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 547}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"EvokeAnimal": {"unit": 131073, "art": 1, "button": 442}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"EvokeGhost": {"unit": 1, "button": 436}}}}
{"id": 4, "cmd": "sim_query", "ids": [1, 458753, 65537, 131073, 196609]}
```

Excerpts of the response to 4, measured twelve object ticks after the commands (ghost evoker 1 in the middle of the
evocation, the ghost 458753 sucks an enemy, the evoker 65537 has banished 547, 131073 evokes a bear, wolf 196609 with
an aura). Corpse (16385) and suction target (67585) are different objects; the evoker has 25 mana bound (base value and
current value 125 of 150), the banisher has only paid (base value 150, current value 126 after regeneration):

```json
{"id": 1, "status": 2097152,
 "evoke_ghost": {"evoking": true, "dead_body": 16385, "ghost": 458753, "extra_energy": 25, "mana": 125, "mana_base": 125}}
{"id": 458753, "ghost": {"status": 3, "alpha": 1.0, "target": 67585}}
{"id": 65537, "status": 8388608,
 "evoke_ghost": {"evoking": false, "dead_body": null, "ghost": null, "extra_energy": 0, "mana": 126, "mana_base": 150},
 "banish": {"active": true, "effects": true, "spell_guid": 547, "radius": 26624}}
{"id": 131073, "status": 2097152,
 "evoke_animal": {"evoking": true, "art": 1, "cost": 50, "reserve": 50, "spawn": [83968, 0, 38912], "mana": 50}}
{"id": 196609, "influence": {"guids": [4460], "influenced": [196609, 262145], "ring": true}}
```

All fields are absent in responses before protocol 20 and then read as empty.

**Spells and special attacks** (protocol 21; the queries change nothing, the checksum changes only through the
commands themselves):

- Bar buttons through `sim_unit_bar` and in the "Unit" window, from the menu of the `toolbar.ini` of the **first
  selected own object**; the bar shows the button only if menu, target filter of `targets.ini` and prerequisite
  (`REQUIRED_GUID`) fit. The abilities (mana in parentheses, according to the value sets of the button GUIDs):
  - Spells of the medicine man: **480 Explosion** (80), **481 Storm** (80), **482 Reincarnation** (40, without a
    target); **484 Scout area** (chief, 60); **541 Drain mana** (missionary, 50).
  - Special attacks: **454/455 Dynamite at building/ship** (demolition expert, 70/40), **456/548 Kill** (melee attack of
    the killer, 60/50), **457/546 Snipe** (ranged attack, 50/40), **542 Whirlwind** (smith, 75, without a target),
    **551 Damage building** (smith, 60), **556 Scout map** (trapper).
- Target buttons (all except 482 and 542) need a target: `sim_unit_bar` takes `target` (object ID) **or** `ground`
  (world coordinates `[x, z]`; in the MCP the parameter is called `to`). The command goes **immediately** to the first
  selected own object; the targeting mode of the UI (mouse click) is not touched. 484 and 556 always take the ground
  point (`ground`), the other target buttons an object (`target`); if both are given, the button decides as in
  targeting mode. The rejection comes as an error response, no command goes out:
  - without both: `Button <n> needs a target (target or ground)`; 484/556 with only `target`: `Button <n> needs a
    ground point (ground)`;
  - the first selected object is not own: `Button <n>: the first selected object is not own`;
  - unsuitable target (target filter of `targets.ini`, fog, range `MM_METHODINDEX 3` = sight/3 of the wizard for 480,
    481, 456, 548 and 457, ground point for an object button or the reverse, cost, lock): the message of the target
    check; it only reads, the checksum stays the same. The target check is a **hypothesis** of the reimplementation: no
    command checks the target, only the UI does.
  The commands also go through `sim_command`: `CastSpell` (`0x36`; 480/481 with kind 1/2 and the `f32` bits of the
  target point in `anim`/`idx`, 482 with kind 3), `ScoutArea` (`0x37`, 484/556), `DrainMana` (`0x4E`),
  `DynamiteBuilding`/`DynamiteShip`, `KillUnit`, `SnipeUnit`, `DamageBuilding`, `Whirlwind` (`0x4F`). 482 and 542 go to
  every selected own object that shows the button; the simulation checks again (module, lock, mana) and rejects what
  does not fit (message in the debug log, checksum stays the same). The effect begins at the next tick.
- `sim_query` shows per object (each field is absent without content and then reads as empty; IDs without an object
  are absent, stale IDs stay):
  - `spell` (module 38) gets two fields: `target` = target point `m_Position` `[x, y, z]` in world coordinates for the
    spell kinds 1, 2 and 5, **while the spell is running** (`casting`); afterward it is absent. `corpse` = corpse of the
    reincarnation (kind 3), absent without a corpse; it may point to a removed object.
  - `bomberman` (module 36, kind `bomberman`): `{"target", "extra_energy", "return_point", "busy", "mana"}`: `target`
    = target of the order, `extra_energy` = cost of the order (0 before the first order), `return_point` = return point
    in fixed point (1/4096 tile, the own position at the order), `busy` = is placing a charge, `mana` = current value of
    the extra energy (attribute 15, with the upgrade percentages of the owner; the cost is paid only when the charge
    lies).
  - `killer` (module 37, kind `killer`): `{"target", "extra_energy", "near_damage", "far_damage", "mana"}`: target and
    cost of the order, damage with the knife or from a distance in hit points, `mana` as above.
  - `special_move` (module 49, kind `specialmove`): `{"busy", "mode", "mana"}`: `busy` = the attack animation is
    running, `mode` = kind of the last attack (0 whirlwind, 1 hammering; stays after the end), `mana` as above.
  - In the module list the kinds of modules 36, 37 and 49 are called `bomberman`, `killer` and `specialmove`.
- In the section `sim` (`projectiles`) there are three new kinds `kind`: `dynamite` (charge of the demolition expert,
  fuse time 10,000 ms; `shooter` = demolition expert, `target` = building or ship, `target_pos` = position of the
  charge), `explosion` (world effect of the spell 480, 500 ms; `shooter` = wizard) and `storm` (world effect of the
  spell 481, 7000 ms). Only the storm carries the new field `pulse` (0 or 1, alternates with every effect tick, the
  storm pulses every second one); it is absent for the other kinds. The storm keeps running after the end of the spell,
  `target_pos` holds its position.

Examples (the numbers were measured in the test world of the viewer, `crates/opennml-client/src/game/testworld.rs` and the
test `spell_special_info_follows_the_real_commands`; times and values of the original differ). Three buttons at once:
the medicine man 1 throws an explosion at the enemy 2049, the Patriot 65537 places dynamite at the enemy building
133121, the trapper 131073 kills the enemy 67585 (all player 0 against 1, mana 100):

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 480, "target": 2049}
{"id": 2, "cmd": "sim_unit_bar", "button": 454, "target": 133121}
{"id": 3, "cmd": "sim_unit_bar", "button": 456, "target": 67585}
{"id": 4, "cmd": "sim_unit_bar", "button": 484, "ground": [2624.0, 1600.0]}
```

Each command goes to the first selected own object of the selection of the viewer (left click or box): the matching
object must therefore be selected before each one (medicine man, Patriot, trapper, for 484 a chief). Query 13 object
ticks after the first three commands: the wizard is casting (target point = center of the tile (20, 12), `casting`),
the demolition expert is placing (`busy`, the cost is not yet paid), the killer has his target (mana 101 through
regeneration, the cost follows with the hit):

```json
{"id": 1, "status": 8388608,
 "spell": {"art": 1, "button": 480, "cost": 80, "anim": 0, "idx": 0, "reserve": 0, "mana": 100, "mana_max": 100,
           "casting": true, "target": [2624.0, 0.0, 1600.0]}}
{"id": 65537, "status": 8388608,
 "bomberman": {"target": 133121, "extra_energy": 70, "return_point": [124928, 0, 165888], "busy": true, "mana": 100}}
{"id": 131073, "status": 1,
 "killer": {"target": 67585, "extra_energy": 60, "near_damage": 300, "far_damage": 0, "mana": 101}}
```

The section `sim` afterward: after 24 object ticks the explosion (400 of 500 ms left), after 66 object ticks the charge
(9900 of 10,000 ms left; the demolition expert is done and has paid 70 mana, `busy` is `false`, `mana` 30):

```json
{"id": 0, "kind": "explosion", "fire": false, "remaining_ms": 400, "flight_ms": 500, "shooter": 1, "target": null,
 "target_pos": [2624.0, 0.0, 1600.0]}
{"id": 1, "kind": "dynamite", "fire": false, "remaining_ms": 9900, "flight_ms": 10000, "shooter": 65537,
 "target": 133121, "target_pos": [4068.6875, 0.0, 5348.6875]}
```

Storm and smith (fresh match): the medicine man 1 storms the enemy building 67585 (481 requires a building within the
radius sight/3), the smith 65537 damages the building 2049 (551):

```json
{"id": 1, "cmd": "sim_unit_bar", "button": 481, "target": 67585}
{"id": 2, "cmd": "sim_unit_bar", "button": 551, "target": 2049}
```

26 object ticks later: the spell has paid 80 mana (rest 20), the target point still applies (center of the tile (16,
12)), the storm has 6700 of 7000 ms and `pulse` 0, the smith is hammering (`mode` 1, mana 100 - 60 + regeneration).
Five object ticks later `pulse` is 1 (rest 6200 ms). After 46 object ticks the spell is over (`casting` `false`,
`target` absent), the storm keeps running:

```json
{"id": 1, "spell": {"art": 2, "button": 481, "cost": 80, "anim": 0, "idx": 0, "reserve": 0, "mana": 20, "mana_max": 100,
                    "casting": true, "target": [2112.0, 0.0, 1600.0]}}
{"id": 65537, "status": 8388608, "special_move": {"busy": true, "mode": 1, "mana": 41}}
{"id": 0, "kind": "storm", "fire": false, "remaining_ms": 6700, "flight_ms": 7000, "shooter": 1, "target": null,
 "target_pos": [2112.0, 0.0, 1600.0], "pulse": 0}
```

All fields are absent in responses before protocol 21 and then read as empty; `sim_unit_bar` did not know `target` and
`ground` before protocol 21 (the target buttons did not exist).

**Headhunters and robbing** (protocol 22; the queries and `sim_hire_menu` change nothing, the checksum changes only
through the commands themselves):

- Hiring: In the "Economy" window of an own, finished headquarters there is the headhunter menu (kind, target player,
  target). Over the control channel `sim_hire_menu` returns levels 2 and 3 (`players`, `targets`, see the command
  table); hiring is done with `sim_command` `HireHeadhunter` (`0x2C`, fields `art`, `target_player`, `target_guid`,
  `building`). The payer is the owner of the building. The simulation checks like the bar (own, finished, living
  building; button of the kind unlocked in the menu of the type; target player in `players`, target GUID in `targets`)
  and otherwise rejects (debug log, checksum the same); if gold is missing, message 10002 goes only to the payer.
- Robbing: `sim_unit_bar` with 540 issues `RobCorpses` (`0x4C`) to every selected own robber whose bar shows the button
  (menu, research 3326); it costs 10 mana (config 15 of 540). Without the research the button is gray and nothing goes
  out. Also through `sim_command`: `{"RobCorpses": {"unit": id, "button": 540}}`.
- `sim_query` shows per object (each field is absent without content and then reads as empty; IDs without an object
  are absent, stale ones stay):
  - `headhunter` (module 34, kind `headhunter`): `{"art", "state", "target_guid", "target_player", "target",
    "last_target", "price", "customer", "customer_cost"}`: `art` = kind from the type GUID (0 SPA 4100, 1 PRI 4101, 2
    SID 4102; 255 for another type with module 34), `state` = `m_LastTargetStatus` (0 searching, 1 walking to the
    unit, 2 walking to the building), `target_guid`/`target_player` = contract (both -1 after a success), `target` =
    last found object (`m_TargetHID`), `last_target` = current target, unit or building (`m_LastTarget`), `price` =
    current price (`m_CurReward`: +50 per hiring, 500 after a success), `customer` = slot of the client (-1 none),
    `customer_cost` = what he paid (refund on a takeover).
  - `robbing` (module 48, kind `robbing`): `{"anim", "robbing", "attack_enabled", "mana"}`: `anim` = `m_RobAnim[0]` (-1
    with an empty list), `robbing` = is robbing right now (status `ANIM_LOCKED` and timer `0x330` created),
    `attack_enabled` = attack module switched on (robbing switches it off one step later through the pending list, only
    an abort switches it on again - it also stays off after the last corpse), `mana` = current value of the extra
    energy (attribute 15, with the upgrade percentages of the owner).
  - In the result `headhunters`: always three entries `{"art", "hunter", "price"}` per kind 0-2: `hunter` = entry of the
    global headhunter table (`GameWorld+0x4DC`; `null` without a headhunter, a dead one not yet removed stays), `price`
    = price of the next hiring like the tooltip (`FUN_0047DEE0`): with a living headhunter its `price`, with a dead one
    0 (quirk C-338), without a headhunter config 12 of the type (300 in the shipped data, 500 without a value).
- In the section `sim` (`projectiles`) the kind `throw` (dynamite throw of the headhunter): `shooter` = headhunter,
  `target` always `null` (the throw does not track), `target_pos` = target point at the throw, flight time like the
  arrow; plus the new field `impact` = impact message to the thrower (3362 = `0xD22` unit, 3618 = `0xE22` building); it
  is absent for the other kinds.
- `notices` (section `sim`) also shows the text messages of the headhunter: 10002 "not enough gold" only to the
  payer, 10010 (hired) and 10011 (success) to all slots 0-7. `id` is the number from `texte.deu`, `tile` `[-1, -1]`,
  `text` `null` (the templates have placeholders for player and type names that only the UI fills in; `messages.ini`
  does not know these numbers).

Examples (the numbers were measured in the test world of the viewer, `crates/opennml-client/src/game/testworld.rs`
`hunter_host` and the tests `headhunter_info_leaves_the_checksum_unchanged` and `hire_menu_matches_the_panel`; times
and values of the original differ). Slot 0 (SPA, 1000 gold) has the headquarters 1, slot 1 (SID) the sheriff 2049; the
undertaker 65537 belongs to slot 0 (research 3326, mana 150). Menu, hiring and robbing:

```json
{"id": 1, "cmd": "sim_hire_menu", "player": 0, "art": 2, "target_player": 1}
{"id": 1, "ok": {"players": [1], "targets": [2614]}}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"HireHeadhunter": {"art": 2, "target_player": 1, "target_guid": 2614, "building": 1}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"RobCorpses": {"unit": 65537, "button": 540}}}}
```

The SID headhunter 16385 is created at slot 8 at the headquarters, slot 0 has paid 300 gold. 24 steps (12 object ticks)
later it is searching and walks to the sheriff (`state` 1); the table shows it with the new price 350:

```json
{"id": 16385, "guid": 4102, "owner": 8,
 "headhunter": {"art": 2, "state": 1, "target_guid": 2614, "target_player": 1, "target": 2049, "last_target": 2049,
                "price": 350, "customer": 0, "customer_cost": 300}}
"headhunters": [{"art": 0, "hunter": null, "price": 300}, {"art": 1, "hunter": null, "price": 300},
                {"art": 2, "hunter": 16385, "price": 350}]
```

The undertaker has paid 10 mana (1 regenerated on top), its attack module is off. Arrived at a corpse (walk success
`0x230`) it robs (lock `ANIM_LOCKED`, `status` 8388608, timer `0x330`). A throw of the headhunter at the unit on tile
(44, 30) is in the projectile list as `throw` (created in the test through `launch_throw`, as the headhunter throws it
at an unreachable target):

```json
{"id": 65537, "status": 8388608, "robbing": {"anim": 3, "robbing": true, "attack_enabled": false, "mana": 141}}
{"id": 0, "kind": "throw", "fire": false, "remaining_ms": 4606, "flight_ms": 4606, "shooter": 16385, "target": null,
 "target_pos": [5696.0, 0.0, 3904.0], "impact": 3362}
```

If the headhunter of the kind already exists, `sim_hire_menu` names the headquarters of its nation for a target player
without an elite (e.g. `{"players": [1], "targets": [1000]}` for kind 2 with `target_player` 0).

All fields are absent in responses before protocol 22 and then read as empty; `sim_hire_menu` and the button 540 came
with protocol 22. The MCP parameters `art` and `target_player` (action `hire_menu`) appear only in a new MCP session
after rebuilding `opennml-viewer-mcp`; until then `sim_hire_menu` goes through the raw control channel.

**Economy sections of `sim_query`** (protocol 9; a section is absent if the object does not have the module; IDs
`null`/empty for "none", also for the start ID 0 of the modules):

- `building` (module 3): `finished` (flag `CONSTRUCTED_FINISHED`), `under_construction` (counts "in progress"),
  `build_rest`/`build_time` (remaining work and construction time, `m_BuildCounter`/`m_BuildTime`), `progress`
  (construction progress of the display 0.1 ... 1), `render_bits` (4 construction site, 3 finished, `0x20` burning),
  `flags`, `production` (`{"make_guid", "rest", "total"}` in ms, `total` 0 = not yet paid; `null` without an order),
  `queue` (waiting GUIDs, index 1 ... of `sim_cancel`), `docks` (`[occupied, total]` docking slots),
  `meeting_point_fx`/`meeting_point` (rally point in fixed-point tiles or world `[x, z]`), `master`/`slaves` (wall
  ring), `sleeping`, `dead`; since protocol 18 also `gate`, `area_watch` and `watch_rect` (see "Gates and enemy
  watch").
- `gatherer` (modules 7, 8, 12, 22, 55; with several gatherer modules - pioneer: gold, wood, harvest - the active one:
  first one that is not free, then one with a source, then the one of the carried load kind, otherwise the first):
  `module`, `kind` (`mining`/`lumber`/`harvest`/`fishing`/`hunting`), `state` (0 free, 1 to the source, 2 to the
  storage, 3 delivery, 6 search again; work 300 gold, 200/201 wood, 100 harvest, 400 fishing, 100/101/102 hunting),
  `load_amount`/`load_kind` (carried load, kind 0 gold, 1 wood, 2 food, -1 none), `source` (tree, mine, farm, basket;
  for hunting the hunting target, `m_TargetHID`, like `prey`), `store` (delivery building), `prey` (hunting only:
  animal or carcass, otherwise `null`; protocol 12).
- `construct` (module 1): `state` (0 free, 1 begun, 2 building, 3 arrived, 4 on the way, 5 repairing, 6 target
  finished), `building` (target).
- `quarter` (module 23): `occupied`, `max`, `occupants`.
- `resource` (module 6): `attribute` (26 gold, 24 food), `amount` (remaining amount).
- `tree` (module 4): `wood` (remaining wood), `falling`, `registered` (registered lumberjacks).

**Combat section of `sim_query`** (protocol 11; read only, the checksum stays the same): `combat` is present for
objects with hit points (attribute 14) or one of the modules 16 (dying), 17 (attack), 18 (melee), otherwise absent:

- `hp` (`[current, maximum]`, attribute 14; with the upgrade percentages `P` of the owner), `hp_fraction` (fraction in
  1/100,000, non-negative), `rank` (0-5), `experience` (experience fraction, attribute 30, 1/100,000), `last_attacker`
  (ID or `null`), `ticks_since_hit`, `render_bits` (`0x40` carcass).
- `attack` (module 17; `null` without): `status` (0 calm, 1 approach, 2 blow/shot in progress), `target`, `automatic`
  (target from the target search; `false` with an ordered attack), `aggressiveness` (1 no combat, 2 resistance, 3
  escalating, 4 target search), `mode` (stance 0-4), `stubbornness` (`[stance, command, movement]`), `search_counter`.
- `dying` (module 16; `null` without): `status` (0 calm, 1 died, callbacks pending, 2 sinking), `die_counter`
  (remaining time in ms), `rob_gold`, `listeners` (number of death callbacks).

**Ranged combat section of `sim_query` and projectiles** (protocol 12; read only, the checksum stays the same). The
sections are absent if the object does not have the module:

- `ranged` (ranged combat module 19): `min_dist`/`max_dist` (fixed point, `m_MinDist`/`m_MaxDist`), `shoot` (shoot
  module 10; `null` without): `flags` (module flags), `target`, `bullets` (`[fired, size]` of the magazine:
  `m_BulletsCur` counts the shots since the last reload, empty when fired >= size), `shoot_delay`, `busy`,
  `gun_select` (1: target is a hunting animal).
- `animal` (module 20): `aggressive`, `shy`, `roamer`, `roaming`, `escaping`, `underwater`, `home` (`[x, z, height]` in
  fixed point), `alpha` (transparency).
- `edible` (module 21): `meat` (`[current, maximum]`, attribute 24), `has_meat` (flag `0x200`), `rotting` (flag
  `0x100`).
- `gatherer.prey` (module 22 hunting): animal or carcass; `gatherer.source` is the same target there.
- Section `sim`: `projectiles` (flying projectiles in tick and damage order: `id`, `kind` `arrow`, `ball` or (from
  protocol 21) `dynamite`, `explosion`, `storm`, (from protocol 22) `throw`, `fire` (fire arrow), `remaining_ms`,
  `flight_ms`, `shooter`, `target`, `target_pos`, for the storm `pulse`, for the throw `impact`) and `flights` (number
  of the paths of the viewer; rendering only, also set in the responses of `sim_setup`/`sim_load`/`sim_view`).

The hunting commands are `sim_command` commands of the simulation (`Hunt` `0x17`, with `gut: true` `0x16` gutting;
`Follow` `0x74`):

```json
{"id": 4, "cmd": "sim_command", "command": {"player": 0, "kind": {"Hunt": {"animal": 6144, "units": [2048], "gut": false}}}}
{"id": 5, "cmd": "sim_command", "command": {"player": 0, "kind": {"Follow": {"target": 6144, "units": [2048]}}}}
```

`0x16` (`gut: true`) and `0x17` act the same (quirk C-182: both post `0x216` to the animal). `Hunt` acts only
**directly**; through the queue (`Queue { Hunt }`) it stays without effect (quirks K-81, W-68), and the UI never wraps
418/419 in `0x7B`. The simulation accepts only own units and rejects an existing target without a carcass module
(exception A8), a missing target goes through as in the original. `Follow` (`0x74`) has no target check, also acts
immediately from the queue and lets the unit follow with a tolerance of 5 degrees (walk `0x3900`, quirk K-193). The
queries (`sim_query`, `sim_hash`) leave the checksum the same; `Hunt` and `Follow` change the simulation like every
command.

The combat commands are not new protocol commands but `sim_command` commands of the simulation (JSON form of
`CommandKind`, external tagging of Serde, IDs as numbers):

```json
{"id": 1, "cmd": "sim_command", "command": {"player": 0, "kind": {"Attack": {"units": [2048], "target": 6144, "pos": null, "mods": 0}}}}
{"id": 2, "cmd": "sim_command", "command": {"player": 0, "kind": {"SetBehavior": {"units": [2048], "mode": 3}}}}
{"id": 3, "cmd": "sim_command", "command": {"player": 0, "kind": {"GarrisonAttack": {"buildings": [4096], "target": 6144}}}}
```

`Attack` (`0x11`): `mods` 1 = right double-click (run), 4 = attack mode; `pos` (fixed-point position * 128 of the
memory image) turns it into `0x71`. `SetBehavior` (`0x12`): `mode` 0 cease fire, 1 hold position, 2 defensive, 3
offensive, 4 defend area. `GarrisonAttack` (`0x5B`): the garrison of the buildings attacks `target`. As with all
`sim_command` commands, the simulation checks the sender: foreign units are ignored. With the mouse: right-click on an
enemy, `KeyX` and Shift+`Digit1`-`Digit5` (section on keys).
Weather particles move only in visible tiles: `weather_moved` counts only particles of visible tiles, the others stand
still. After `set_light`, `set_weather` and `set_quality` the viewer draws a new frame (except with a minimized
window); the state snapshot afterward shows its overlay and `weather_drawn`. Fields and types:
`crates/opennml-control/src/state.rs`.

## UI (`ui_*`)

Protocol 24, plus `ui_video` and the fields `video`/`pointer_hidden` since protocol 25.
The commands operate the menu UI
(`opennml_ui::app::MenuApp`) like mouse, keyboard and command 5: they act only through the input of the **next frame** or
the queue of the UI, never on the simulation (checksum the same). Available in the viewer only with
`--dialog <names>` in the window, in `opennml-game` always; otherwise the error "no UI open". With a stopped clock nothing
happens until `step`. Executed in the window thread (`opennml_client::ui_control::execute_ui`).

- `ui_state`: `{"now_ms", "screen": [W, H], "dialogs", "focus", "hover", "elements", "images", "busy", "queued",
  "effects", "video", "pointer_hidden", "game"}`. Names lowercase as in the messages of the UI, rectangles `[x0, y0,
  x1, y1]` in screen pixels. `dialogs` = open frames in drawing order (non-modal, then modal) with `name`, `rect`,
  `animating` (fly-in or fly-out running), `modal`; `focus`/`hover` = `[dialog, element]` or `null`; `elements` = named
  elements with `dialog`, `name`, `kind` (`button`, `combobox`, `listbox`, `editfield`, `slider`, `radiobutton`,
  `label`), `rect`, `visible` (drawn in the last frame), `enabled`, `hidden` (`HIDE`) and, depending on the kind,
  `checked`, `value`, `selection`, `items`, `text` (only the text of the logic or of the edit field); `images` = images
  of the desktop `[path, opacity]`; `busy` = the dialog stack has orders (triggering refused); `queued` = waiting
  application commands; `effects` = the last 32 effects `{"at_ms", "kind", ...}` with `kind` `play_sfx` (`event`,
  `flags`, `category`), `play_speech` (`path`), `menu_music` (`path`, `looping`), `restart_music`, `set_sound_driver`
  (`index`, device change of the sound options), `play_video` (`path`, `abortable`), `stop_video` (`path`),
  `set_volume` (`category`, `value`), `set_gamma` (`gamma`), `set_hardware_cursor` (`on`), `set_display_mode`
  (`width`, `height`, `bits`), `write_settings`, `quit`, `net` (network commands `7`, `8`, `9`, `0x2D`, `0x2F`: `code`
  of the original, `command`), `search` (`on`, search in `LAN1` on/off), `lobby` (`action`, wish from the lobby as a
  description) or `unhandled` (`code` of the original, `command`). `video` (protocol 25, absent without a running
  video) = `{"path", "frame", "frames", "abortable"}`: path of the `.bik`, frame at the time of the report (from 0, from
  the clock: `floor((now - start) * fps / 1000)`), number of frames, `abortable` = every key aborts (quirk O-68);
  `pointer_hidden` = the mouse pointer is off (start sequence, campaign video). `play_video` (`path`, `abortable`) is
  in the effect log as soon as `0x1D` requests the video, `stop_video` (`path`) when it ends (end after N frames, key
  abort or replacement by a new video). `game` (only `opennml-game` with a match, see "Protocol 27") and the effects of the
  match: see there. Types: `crates/opennml-control/src/ui.rs`.
- `ui_click`: center of the part of the element that the last frame drew, then movement, press and release into the
  input of the next frame - the same sequence as the mouse by hand: a disabled element does nothing (as in the
  original), during the fly-in triggering is refused (the button asks `0x12F` before the sound: no code 8, no click
  sound, no subdialog - `ui_state` then shows `busy`), two clicks within `DoubleClickTimeout` are a double-click.
  Case-insensitive. Errors (error message): dialog not open (with the open ones), element unknown (with the known
  ones), hidden, not drawn in the last frame or outside the screen. The drawn pointer of `opennml-game` stays where the
  real mouse is; the host does not know the position from `ui_click`, so a real movement to the same place as before
  does not arrive. The entries of an open ComboBox list have no name (the list hangs on the desktop) and are not
  reachable; a `ui_click` on the combobox with the list open hits the row below its center. Other entries can be
  chosen with real window messages (DPI-aware).
- `ui_key`: `chord` read like `FUN_0049C050` (trimmed, uppercased, separated at spaces; `escape` hits `ESCAPE`), in
  press order, each key once; one key event with all keys and the character `char`. `ESCAPE` or `RETURN` alone go as a
  broadcast to the buttons with `CHECKESCAPE`/`CHECKENTER`. Unknown name: error with the allowed names; the `0xNN`
  names of unnamed keys are missing there because the original cannot read them back (quirk O-107).
- `ui_open`: queues `{5, dialog}` (opens after running transitions, with fly-in); unknown dialog: error with the known
  names.
- `ui_video` (protocol 25): like `videostart` in the menu `campaignvideo` (`FUN_004DA8A0` `0x004DAA14`...`0x004DAB18`)
  queues `CloseAll` (`0x34`: all open menus closed) and `WaitDialogs` (`0x15`: until the dialog root is empty), then
  `PlayVideo("data/videos/<name>.bik")` (`0x1D`), `WaitVideo` (`0x1A`) and `AfterProbeVideo` (pointer on, menu music
  anew: the part of `0x28` without the campaign - no booking, no emptying of the queue, no `selectcampaign`) and
  finally the reopening of the menus that were open at the command, in the same order (command 5, as a substitute for
  `selectcampaign`). The button `videostart` in the menu `campaignvideo` takes the way of the original with booking
  (`0x1C`) and `0x28`. During the video no menu is therefore open (`dialogs` empty). If no menus are open, closing,
  waiting and reopening are omitted and the video begins in the next frame; otherwise only when the menus are closed
  (without fly-out three frames later). **Only with an idle UI:** if a menu transition is running (fly-in or fly-out,
  `busy`), commands are waiting (`queued` > 0, also during the start sequence) or a video is running (`video`), the
  command answers with an error message ("UI busy (...)") and queues nothing; so first wait with `step` and `ui_state`
  until `busy` is false, `queued` 0 and no `video` is reported. Reason: in the original `videostart` is a button that
  triggers only with idle menus (`0x12F`, quirk O-21); a `CloseAll` in the middle of a transition could skip a dialog
  that is still waiting and opens afterward, and `WaitDialogs` would never end. The name consists only of `A`-`Z`,
  `a`-`z`, `0`-`9`, `_` and `-` (no path, no extension); otherwise also an error message, nothing queued. A missing or
  broken file is reported by the host as a warning in the log, the video is omitted (response `null` nevertheless).
  With a stopped clock every frame needs a `step`; check runs are muted. The frame of the `step` in whose response
  `play_video` first appears is still drawn without this video (the host opens it after drawing, with the time of this
  frame as the start); `screenshot` therefore shows frame 0 only after the next `step` (at 16 ms still frame 0). At
  640 x 480 the video image is texel-identical to `opennml_bink::to_rgba` (UV shifted by half a pixel,
  `opennml_ui::video::video_uv`).

Example (`opennml-game` at 1024 x 768):

    {"id": 1, "cmd": "ui_click", "dialog": "start", "element": "options"}
    {"id": 1, "ok": {"x": 204, "y": 377}}
    {"id": 2, "cmd": "ui_key", "chord": "escape"}
    {"id": 2, "ok": {"vk": [27]}}

### Protocol 27

Match from the menu. Old lines stay readable (new fields with a default).

- **`ui_state.game`** (only `opennml-game`, absent without a match, in the menu and in the viewer): `{"level",
  "game_type", "menu_state", "steps", "checksum", "loading", "load_phases"}`. `level` = `.lvl` path of the map in the
  spelling of the session (lowercase, as the archive search keeps it; `start_match.level` carries the spelling of the
  original - compare case-insensitively), `game_type` = game type of the original, `menu_state` = game menu state
  (`Engine+0x2CCC`: 0 game running, != 0 a game menu holds), `steps` = engine ticks of the simulation, `checksum` =
  checksum of the simulation as hex text with 16 digits (like `sim_hash.checksum`, same number), `loading` = loading
  phase in percent (0, 40, 70, 100), set only during loading; since loading blocks the host, it is never visible over
  the channel - instead `load_phases` gives the course of the last load (percent per drawn phase, absent before the
  first load). Reported in `ui_state`, `state` and the response of `step`.
- **New effects** in `effects` (game effects of the UI, `opennml-game` executes them, the viewer only logs them):
  `start_match` (`level`, `game_type`, `slots` = occupied slots `{"kind" (1...4 computer, 999 human), "nation", "team",
  "color", "handicap"}` in list order), `load_game`/`save_game`/`delete_save` (`name` without the extension
  `.<GUID>`), `reload_level`, `resume_game` (`game_speed` or `null`), `write_profiles` and `delete_profile_saves`
  (`guid`).
- **`opennml-game` and `sim_*`:** `sim_hash`, `sim_query` and `sim_command` as in the viewer (same parameters and
  responses, `opennml_client::game::sim_control::execute_sim`), only with a running match, otherwise the error "no match".
  All other `sim_*` commands stay with the viewer and are "not available" in `opennml-game`. `sim_command` takes effect at
  the next tick of the running match. **Quirk O-318:** As long as the match takes no steps (pause or an open game
  menu), `opennml-game` executes `sim_command` and `sim_result` **immediately** (`SimHost::execute_now`, with an entry in
  the command log); before, they waited until resuming. Check scripts must no longer rely on a command staying put
  with an open menu.
- Hints for check runs: The name field of `SELECTPROFILE` is prefilled with `NetName` from the settings file, and the
  player name enters the checksum - equal checksums across runs only with the same starting folder (fresh copy of a
  fixed settings file per run). Loading screens are shown by the check switch `--loading-shot <folder>` of `opennml-game`
  (per phase `loading-<n>-<percent>.png`). `screenshot` draws a pointer only with `pointer: true`; whether it is
  visible after loading is shown by `pointer_hidden` and a window image. The frame in whose `step` `load_game` appears
  shows steps and checksum of the savegame (no tick yet). The lists of `INGAMELOADGAME`/`LOADSAVEDGAME2` show date and
  time of the file (`(YY/MM/DD h:mm) name`); images of them are equal across runs only within the same minute.

### Protocol 29

Game end, accounting, campaign. Old lines stay readable (new fields with a default).

- **`sim_result`** `{"player", "result"}` (viewer and `opennml-game`, there only with a running match, otherwise "no
  match"): slot `player` (0-7) loses (`result` 1) or wins (2) - like the cheat of the original
  (`GameEngine_SetPlayerResult` `0x00430CE0`), but as the command `0x56` `SetPlayerResult` of the active player through
  `opennml_client::game::sim_control::execute_sim`: takes effect at the **next tick** (with an open game menu only after
  closing), is in the command log and in replays. Only slot and result are checked (the slot need not be the own one);
  the simulation afterward lets every slot that is not mutually friendly lose (quirk O-181). Response `{"step"}` like
  `sim_command`. Equivalent raw:
  `{"cmd": "sim_command", "command": {"player": 0, "kind": {"SetPlayerResult": {"slot": 0, "result": 2}}}}`.
- **`ui_state.game.result`**: result of the local player (`Player+0x358`): 0 playing, 1 lost, 2 won (absent in lines of
  protocol 27 -> 0). `opennml-game` shows the end screen in the frame in which the result is first 1/2 (`menu_state` 7 or
  6, dialog `gameend_defeat`/`gameend_victory`).
- **`ui_state.accounting`** and **`ui_state.play_time`** (`opennml-game`; the viewer builds no accounting, both are absent
  there; only while the frame `accounting` is open): the lines of the accounting in slot order `[{"name", "color",
  "values": [12 texts]}]` - name (empty -> "Computer", in the campaign resolved text IDs), color index and the twelve
  values as `"%d"` (columns 0-3 technology, 4-7 military, 8-11 economy; column 3 stays 0, quirk O-163) - and the play
  time text ("[CENTER]Spielzeit: 0:05", hours:minutes from `steps * 50` ms, quirk O-164; the text comes from the text
  table of the installation, here the German edition). The host builds the table at `0x0E` when the match has already
  ended (`game` is then absent); it comes from `opennml_ui::screens::MenuData::accounting`.
- **New effects** in `effects`: `game_end` (`victory`: end screen of the local player queued, state 6 or 7),
  `accounting` (command `0x0E`: end the match, build the accounting, open the frame `accounting`), `surrender` (`slot`:
  command `0x27` "Surrender", executed immediately; the command takes effect at the start of the step S->S+1 of the
  replay, the entry S in `sim_hash.history` then already shows the state after the command, a replay at S still the
  state before), `start_campaign_level` (`level`, `difficulty` 0-2: command `0x0C`, also automatically after a video or
  after "Continue" of the accounting) and `book_campaign` (`won`: current campaign entry booked - `0x1C` at the video
  or the game end in the campaign; afterward `write_profiles`). `opennml-game` executes `accounting`, `surrender` and
  `start_campaign_level`; `game_end` and `book_campaign` are reports (the UI has already done both). The viewer only
  logs them.
- **MCP:** `viewer_sim` with `action: result`, `player`, `result` (new tool parameters only after restarting the MCP
  session; until then raw over the control channel).
- **Checksum sequence `UI_GOLDEN`** (`crates/opennml-client/src/game_flow/ui_golden.rs`): the whole circle without graphics
  (profile, own game, 200 steps, quicksave/quickload, victory through `sim_result`, accounting with all tabs, main
  menu, campaign video with automatic level start, victory, "Continue") with one checksum per section.

### Game UI

Message bar and status bar, protocol 31. Old lines stay readable (`hud` is then absent).

- **Drawing:** `opennml-game` draws per frame world -> game UI -> menu -> pointer. The game UI
  (`opennml_client::game_session::GameSession::hud_cmds`) is in this order: the message bar in the world pass (`0x00438F2A`,
  per message kind the head at `POSITION`, color from the remaining time), then the game windows in the drawing order
  of the tree (status bar, minimap, command bar, cinemascope area, tooltip), finally provisionally the text messages
  of the headhunters at (16, 16) (`FUN_00492A40`, no message bar; position in the original not traced). The bar runs in
  game time and stands still with an open game menu (quirk O-213). `screenshot` shows the same image without a pointer
  (with `pointer: true` with a pointer).
- **`ui_state.game.hud`** (only `opennml-game` with a match; also in `state` and the response of `step`): `{"dialog",
  "windows", "messages", "action_stack", "status"}`.
  - `dialog` = always `"GAMEUI"` (target of `ui_click` on game windows, see "Command bar").
  - `windows` = windows of the tree in creation order `{"kind", "rect", "layer", "visible"}`; `kind` is `base`, `root`,
    `status_bar`, `minimap`, `toolbar`, `cinemascope` or `tooltip` (only those created according to `IsStatusBar`,
    `IsMap`, `IsToolBar` of the settings; `cinemascope` always, `tooltip` only with the command bar), `rect` = `[x0, y0,
    x1, y1]` in screen pixels, `layer` = level 4 (base) ... 1 (tooltip, quirk O-248).
  - `messages` = per message kind with messages in file order of `messages.ini` the head `{"tag", "id", "text", "flags",
    "display_ms", "fade_ms", "queued"}`: kind, `MSGID`, text with inserted game values (`[PLAYERNAME n]` etc.; other
    tags stay), flag word, remaining display duration and fade time in ms of game time (the display duration runs below
    0 in the last tick, the fade begins in the next; 1500 at appearing) and the `MSGID`s of the queued messages behind
    it. After loading or restart the bar is empty (not in the savegame).
  - `action_stack` = ping locations `[x, z]` (tiles) of the latest 5 messages with `PING` and a location, newest first
    (quirk O-219).
  - `status` = `{"shown", "texts", "red"}`: bar drawn, texts of the entries in file order of `statusbar.ini`
    (resources as `%d`, population as "population/living space", name of the local player, an empty name is absent),
    `red` = population > living space (quirk O-221). Absent without `IsStatusBar` or without a readable
    `statusbar.ini`.
- **Viewer:** `--hud` (or `hud = true` in `opennml-viewer.toml`; default off) draws the game UI like `opennml-game` through the
  UI drawer between world and dialog layer, also in `screenshot`; the message lines in the overlay then disappear. With
  `--dialog` it uses its UI layer, otherwise the viewer creates its own. The command **`hud`** `{"on": true|false}`
  switches at run time (response `null`); rendering only, the checksum stays. The viewer reports no `ui_state.game`.
  `opennml-game` answers `hud` with "is not available in opennml-game" (there the game UI is always on). There is no MCP tool
  and no new tool parameter for it: `hud` goes through the raw control channel.

Example (`opennml-game` at 1024 x 768, excerpt of `ui_state`):

    {"id": 7, "cmd": "ui_state"}
    {"id": 7, "ok": {"now_ms": 41000, "screen": [1024, 768], ..., "game": {"level": "...", ..., "hud": {
      "dialog": "GAMEUI",
      "windows": [{"kind": "base", "rect": [0, 0, 1024, 768], "layer": 4, "visible": true},
                  {"kind": "root", "rect": [0, 0, 1024, 768], "layer": 3, "visible": true},
                  {"kind": "status_bar", "rect": [0, 0, 1024, 26], "layer": 2, "visible": true}, ...],
      "messages": [{"tag": "ECONOMY", "id": 0, "text": "WIR HABEN NICHT GENUG NAHRUNG!", "flags": 50398208,
                    "display_ms": 650, "fade_ms": 1500, "queued": [4]}],
      "action_stack": [[3, 4]],
      "status": {"shown": true, "texts": ["100", "250", "80", "11/10", "Anna"], "red": true}}}}}

Viewer at run time:

    {"id": 8, "cmd": "hud", "on": true}
    {"id": 8, "ok": null}

### Command bar

Command bar, portrait, tooltips, protocol 34. Old lines stay readable (`toolbar` is then absent). Applies to `opennml-game`
with a running match; the viewer operates the bar differently (see below).

- **`ui_click` with `"dialog": "GAMEUI"`** (only `opennml-game`): click on the center of an element of the game UI.
  `opennml-game` intercepts `GAMEUI` before `execute_ui` (`ui_*` for menu dialogs stays unchanged). The center comes from
  the last frame (button rectangles of the bar, areas of the last drawing); then **three mouse events** (move, press,
  release) go through the flow of the match, like the mouse by hand: an open **game menu blocks the click** (the move
  goes through), without a running match the response is an error. Response `{"x", "y"}` = point in window pixels. As
  with the mouse: the button must have been clickable in the last frame; new buttons are built by the frame step, so
  click only after a `step`. The effect of the button and click path (commands at the next tick boundary) comes with
  the next `step`.
- **Element names** (`element`):

  | Name | Target |
  |---|---|
  | `normal:<0...13>` | normal slot of the bar (slot of the `toolbar.ini` row) |
  | `guid:<msg>` | the first normal button with this `BUTTONMSG` (e.g. `guid:534`) |
  | `queue:<i>` | production; 0 = running order, 1 ... the waiting ones (kind bits `& 0xFF`, the index of the cancel command) |
  | `selection:<i>` | multi-selection; `i` = index of the object in the selection |
  | `stance:<i>` | stance `420 + i` (0 ... 4) |
  | `formation:<i>` | formation button in the order of the bar (collapsed only `formation:0`) |
  | `hero:<i>`, `headhunter:<i>` | slot `i` of the hero or headhunter row (occupied slots) |
  | `portrait` | area of the portrait (last drawing) |
  | `value:<i>` | `i`-th value row of the portrait (tooltip area) |
  | `counterupgrade:<i>` | `i`-th running counter-upgrade (area 50 x 50) |
  | `statusbar` | center of the status bar |
  | `toolbar` | free area of the bar `(630, H - 86)` (no button; to check that the bar takes a click on a free area) |

  The names of the buttons are in the report (`ui_state.game.hud.toolbar.buttons[].element`). Errors (error message):
  "element {e} does not exist in GAMEUI" (unknown name, no such button, portrait/values/counter-upgrades not yet
  drawn), "match is not running" (no match), "command bar is hidden". Portrait, values and counter-upgrades arise
  during drawing: first `step` (draws) or `screenshot`, then click or read `ui_state`.
- **Timing (important for scripts):** The bar works in the frame step, the areas of portrait, values and
  counter-upgrades arise only during drawing (`step` draws, as does `screenshot`). After a **change of the selection**
  (click in the world, selection button, command) the bar rebuilds its buttons in the next frame (full rebuild, then
  frame event 6) and the portrait is drawn afterward: wait **at least two frames** (`step` with `frames` >= 2) before
  clicking or reading `ui_state`. Earlier you get either the error "element {e} does not exist in GAMEUI" (new button,
  portrait, value not there yet) or the **state of the previous frame** (old buttons, old portrait, old areas, old
  tooltip). The same holds for the tooltip: first `mouse_move`/`ui_click`, then `step` (the frame step sets the
  request, drawing sets the box `rect`). Commands of the buttons go to the next tick boundary; their effect (new
  production, new buttons) shows in the report only after the frames that reach the tick.
- **Double-click = two `ui_click` before the same `step`** (same virtual clock, within `DoubleClickTimeout`). The
  second click thus becomes the double-click of the original (click sequence 9, 9, `0x27`; with MAKE 1 + 1 + 2 = 4
  orders, with Shift 5 + 5 + 2 = 12). Two clicks with a wait of at least `DoubleClickTimeout` ms in between (`step`
  with enough `frame_ms`) are two single clicks.
- **Mouse commands in `opennml-game`:** `mouse_move` `{x, y}`, `mouse_button` `{button, down}` and `key` `{code, action}`
  as in the viewer ("Input"), response `null`; they act like the real mouse or keyboard in the next frame (menu host
  and match). This makes it possible to check tooltips (hold the pointer over a button or area, `step`) and Shift
  clicks. `mouse_leave` and `wheel` stay unavailable. A loss of window focus releases held keys.
- **Middle mouse button** (quirk O-331): `mouse_button` with `button: "middle"` holds `KD_CAM_FREEHAND`, see "Middle
  mouse button (quirk O-331)".
- **Shift click** with `key`: first press the key, then click, then release:

      {"id": 1, "cmd": "key", "code": "ShiftLeft", "action": "down"}
      {"id": 2, "cmd": "ui_click", "dialog": "GAMEUI", "element": "guid:534"}
      {"id": 3, "cmd": "key", "code": "ShiftLeft", "action": "up"}
      {"id": 4, "cmd": "step", "frames": 2, "frame_ms": 16}

  Double-click (MAKE: 4 orders, with Shift 12):

      {"id": 5, "cmd": "ui_click", "dialog": "GAMEUI", "element": "guid:534"}
      {"id": 6, "cmd": "ui_click", "dialog": "GAMEUI", "element": "guid:534"}
      {"id": 7, "cmd": "step", "frames": 2, "frame_ms": 16}

  Tooltip of a button (hold the pointer over it, two frames, read the report) and of a portrait value:

      {"id": 8, "cmd": "mouse_move", "x": 120.0, "y": 640.0}
      {"id": 9, "cmd": "step", "frames": 2, "frame_ms": 16}
      {"id": 10, "cmd": "ui_state"}
      {"id": 11, "cmd": "ui_click", "dialog": "GAMEUI", "element": "value:0"}
      {"id": 12, "cmd": "step", "frames": 2, "frame_ms": 16}

  Response 10: `game.hud.toolbar.tooltip` carries the tooltip. Through the MCP tool `viewer_ui` with `action: click`,
  `dialog: GAMEUI` and `element`; `viewer_input` with `mouse_move`, `mouse_button` and `key` (with `opennml-game`).
- **`ui_state.game.hud.toolbar`** (only `opennml-game` with a match and `IsToolBar`; also in `state` and the response of
  `step`): state of the last frame.

  | Field | Content |
  |---|---|
  | `shown_menu` | GUID of the shown menu (`+0x5CC`); 0 after the extension of a multi-selection |
  | `selected` | HID of the single selection, -1 with a multi-selection, 0 without a selection (`+0x524`) |
  | `hover` | HID of the object under the mouse, 0 none (`+0x48`) |
  | `buttons` | all buttons in child order: `{"element", "msg", "kind", "state", "rect", "progress"}` |
  | `portrait` | `{"guid", "subtitle", "values": [[attribute, "text"], ...]}`; for the group `guid` 500 and no values; absent if the bar shows nothing. Color tags (`[COLOR ...]`) stay as text. The element `portrait` for `ui_click` does not exist for the group: as in the original (`0x00456B70`) only the portrait of an object creates a tooltip area, the nation symbol does not |
  | `tooltip` | `{"guid", "kind", "text", "rect"}` of the frame; `text` with inserted game values and keys, sections separated by a line break; `rect` = box of the last drawing (`[0, 0, 0, 0]` before the first); absent without a request |
  | `counter_upgrades` | running counter-upgrades `[[GUID, "m:ss"], ...]` in list order |
  | `cursor` | open cursor: `build`, `tent`, `wall`, `target`, `defense`; absent without a cursor |
  | `last_button` | `BUTTONMSG` of the last pressed button (`GameEngine+0x4E0`), for selection, hero and occupant buttons the HID of the object (quirks O-246, O-269); absent before the first click (deviation: the start value is not the heap remainder of the original) |

  `buttons[]`: `element` = name for `ui_click`, `msg` = `BUTTONMSG` (for selection and hero buttons the HID, for
  production the GUID of the order), `kind` = full kind word of the row (top byte the kind, e.g. `0x2000000` MAKE;
  below it flags or the index for selection and queue), `state` = 0 normal, 1 mouse over, 2 pressed, 3 locked (the
  state at the start of the frame; the locks of a frame appear only in the next one), `rect`, `progress` = progress in
  whole percent (100 = without veil).

  Excerpt (headquarters selected, mouse over normal slot 3):

      {"id": 10, "ok": {..., "game": {..., "hud": {..., "toolbar": {
        "shown_menu": 1300, "selected": 65537, "hover": 0,
        "buttons": [{"element": "normal:3", "msg": 534, "kind": 33554432, "state": 1,
                     "rect": [10, 592, 60, 642], "progress": 100}, ...],
        "portrait": {"guid": 1300, "subtitle": "Hauptquartier", "values": [[14, "100/100"], ...]},
        "tooltip": {"guid": 534, "kind": 33554432, "text": "...", "rect": [0, 410, 180, 568]},
        "counter_upgrades": [], "last_button": 534}}}}}

- **Viewer:** The viewer does not know `GAMEUI` (`ui_*` need `--dialog` there, and there is no `ui_state.game`). With
  `--hud` you operate the bar in the viewer with `mouse_move`, `mouse_button` and `key` (the mouse goes first to the UI,
  then to the world) and read the state in the image (`screenshot`); without `--hud` the bar rests. Double-click: two
  clicks (press and release) before the same `step`.

### Input in the game

Keys, pointer, control groups, pause, game speed; protocol 36. Old lines stay readable (`repeat` absent = 0, `input`
absent).

- **Key path:** In `opennml-game` keys of the control channel (`key`) take the same path as the keyboard: menu host, then
  the keys that no widget took, to the dispatcher of the match (ESC, bindings from `bindings2.ini`, under the control
  lock End, then `keys.ini` in the keyboard context, screenshot). Chords arise from the held keys in press order:
  Ctrl+1 is `ControlLeft` `down`, then `Digit1` `tap`, then `ControlLeft` `up`. The keys compute on the program clock
  (double tap of `selectnextguid`: 300 ms virtual time).
- **`key` with `repeat`:** `{"cmd": "key", "code": "BracketRight", "action": "down", "repeat": 5}` presses `+` and
  sends five repeats - each is a separate event (quirk O-300), so six game speed steps. Do not forget `up` afterward.
  `repeat` only with `down`, otherwise the error "repeat only with down".
- **`ui_focus_lost`** (only `opennml-game`, response `null`): like the focus loss of the window - menu host and match
  release all held keys (a held key does not repeat afterward), and the match switches the pause on like the original
  (quirk O-154; not with the pause already visible, not in the menu level). The window itself pauses on a focus loss
  only **without** a control channel (check runs usually run without focus); with `--control` only this command
  triggers the pause. It does not exist in the viewer. MCP: `viewer_ui` with `action: focus_lost`.
- **`ui_state.game.input`** (only `opennml-game` with a match; also in `state` and the response of `step`):

  | Field | Content |
  |---|---|
  | `paused` | pause bit of the match (`Engine+0x24E4`): the world stands, camera and water run (quirk O-317) |
  | `shown_paused` | visible pause (`Engine+0x24E5`, lettering PAUSE) |
  | `game_speed` | game speed of the simulation (0 ... 20; the keys `+`/`-` clamp to 0 ... 19) |
  | `config_speed` | setting `GameSpeed` of the UI (the speed keys write it too) |
  | `groups` | the 10 control groups of the active slot, per group the object IDs in selection order (stale IDs stay, quirk O-311) |
  | `last_group` | last chosen group 0 ... 9, -1 none (`Player+0x354`) |
  | `pointer`, `pointer_variant` | the pointer the host draws (ID 0 ... 11, 7 normal; variant 0 normal, 1 red, 2 green); with an open menu or dialog always 7/0 |
  | `pointer_visible` | the pointer is on (off in the campaign cinemascope) |
  | `key_context` | keyboard context for `keys.ini`: `[type, nation + 1, selection GUID]` (type 1 nothing selected, 2 unit, 3 building; quirk O-303) |
  | `gate` | names of the effective locks in fixed order: `ui_locked`, `cinemascope`, `dialog_running`, `script_camera`, `campaign`, `menu_open`, `menu_level` (empty without a lock; `campaign` = game type -1, `menu_level` = game type -2, practically never in the report of the match) |

  Read only: `ui_state` changes no checksum.
- **Examples** (`opennml-game`, match running, clock stopped):

      {"id": 1, "cmd": "key", "code": "BracketRight", "action": "down", "repeat": 5}
      {"id": 2, "cmd": "key", "code": "BracketRight", "action": "up"}
      {"id": 3, "cmd": "step", "frames": 2, "frame_ms": 16}

  Response 3: `game.input.game_speed` has risen by 6 (at most 19), `config_speed` likewise. Store group 1 from the
  current selection (Ctrl+1) and choose it again (1):

      {"id": 4, "cmd": "key", "code": "ControlLeft", "action": "down"}
      {"id": 5, "cmd": "key", "code": "Digit1", "action": "tap"}
      {"id": 6, "cmd": "key", "code": "ControlLeft", "action": "up"}
      {"id": 7, "cmd": "step", "frames": 2, "frame_ms": 16}
      {"id": 8, "cmd": "key", "code": "Digit1", "action": "tap"}
      {"id": 9, "cmd": "step", "frames": 2, "frame_ms": 16}

  Response 7: `game.input.groups[1]` carries the IDs of the selection; response 9: `last_group` 1. Pause by key (`Pause`
  `tap`) or by focus loss:

      {"id": 10, "cmd": "ui_focus_lost"}
      {"id": 11, "cmd": "step", "frames": 1, "frame_ms": 16}

  Response 11: `game.input.paused` and `shown_paused` are `true`, `steps` stands. `Equal` in `opennml-game` is the acute
  accent (VK `0xDD`) and does not change the game speed.
- **Viewer with `--hud`:** Keys **without** a viewer binding (help F1 and `App::key_pressed`: Esc, F1 to F12, B, N, T, R,
  I, K, J, L, P, H, X, F, O, G, Shift + 1...5, arrows, Page Up/Down, Home/End, Insert/Delete, W/A/S/D, Q/E, Shift,
  Ctrl) go on the map into the same key path (binding, under the lock End, `keys.ini`; `GameSession::run_hotkey`):
  digits (groups, Ctrl+digit stores), `+`/`-` (game speed), bar shortcuts. Ctrl and Shift count in the chord. Not in
  the viewer: quicksave/quickload and `togglepause` (the viewer stops with `pause`/`step`), ESC (ends the viewer or a
  mode) and the screenshot (F12). Without `--hud` everything stays as before. The viewer still changes the simulation
  only through commands.
- **Images with a pointer:** `screenshot` renders in `opennml-game` without a pointer, with `pointer: true` with one (see
  "3D mouse pointer"); the chosen pointer is reported by `ui_state.game.input`. A window image of the program delivers
  an image including the pointer (`PrintWindow` with `PW_RENDERFULLCONTENT`; the system pointer is not included).
- **MCP:** `viewer_input` takes the parameter `repeat` for `key`, `viewer_ui` the action `focus_lost`. New parameters
  appear in the schema only after rebuilding the MCP server in a **new session**; until then send the same commands
  through the raw control channel.

### Campaign UI

The presentation of the scripts in the level (cinemascope bars, portraits, text slots, scene fade, control lock,
pointer) has two accesses in the control channel. Protocol 35.

- **`ui_present`** (`opennml-game` with a match, viewer with a map): triggers a presentation event by hand, as if a script
  had generated it. `event` is the JSON form of `opennml_sim::script::present::Presentation`: an object with exactly one
  key (`{"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}`, `{"Fade": {"into": false, "ms": 500}}`,
  `{"Cinemascope": {"on": true}}`, `{"Speech": {"text": 7, "slot": 0, "ms": 1000}}`) or, for unit variants, a string
  (`"CameraBackup"`). Response `null`; the event takes effect immediately (the fades keep running with the clock, with a
  stopped clock with `step`).
  - **Rendering only, the checksum stays - until the next step.** The command goes through the same distribution as an
    event of the simulation (`GameSession::present_by_hand`: presentation of the scripts, scene fade, target marker, UI
    messages to the cinemascope area, status bar and command bar, speech output), but is not a `SimHost` command: no
    entry in the command log, `sim_hash`/`steps` stay in the same frame. Afterward host consequences can generate
    commands, indirectly like a player action: `Cinemascope`/`UiLock` (147/18) empty the selection of the host, and
    `SimHost::sync_selection` sends a `ScriptSelection` command in the next frame for scripts with the conditions
    35-37 (campaign scripts); the game speed 10 changes the steps per frame. The checksum then runs differently than
    without `ui_present`.
  - **It does change the host state,** as the script event would: `{"Cinemascope": {"on": true}}` sets the game speed
    (`SimHost::game_speed`) to 10 and remembers the old one, empties the selection, switches the fog mode and hides
    command bar and status bar (bars only in game type -1); `{"Cinemascope": {"on": false}}` restores the game speed.
    Check runs that compare the game speed reset it afterward.
  - **Pointer and input locks stay with the simulation:** `InputGate` reads cinemascope and lock from `MissionState`.
    `ui_present` with `Cinemascope` or `UiLock` draws bars and switches the bars, but leaves the pointer on and camera,
    map click and F1/F4 free (`pointer_visible` stays `true`; test
    `ui_present_cinemascope_leaves_pointer_and_gate_to_the_simulation`).
  - Errors (error message): "ui_present: ..." with the message of serde for an unknown variant or missing fields;
    `QuickSave` and `ProfileVar` are requests to the flow, not presentation: "ui_present does not know QuickSave
    (presentation only)". Without a match: "no match" (`opennml-game`) or "no map loaded" (viewer). An `event` form other
    than an object with one key/string is already rejected by the check of the command.
  - MCP: `viewer_ui` with `action: present` and `event`. In sessions that began before the rebuild of the MCP server the
    parameter is missing in the schema; then send the same line through the raw control channel.
- **`ui_state.game.presentation`** (`opennml-game` with a match, also in `state` and the response of `step`; absent in older
  lines and in the viewer): `{"bars": [top, bottom], "portraits": [{"slot", "portrait", "frame", "alpha"}, ...],
  "texts": [{"slot", "text", "alpha"}, ...], "fade_alpha", "pointer_visible", "clear_color": [r, g, b], "info_frames":
  [[text, [l, t, r, b]], ...], "countdown", "timers", "screen_text"}`.
  - `bars`: height of the bars in screen pixels, truncating as when drawing (`game_rect(0, 0, 1280, b)` or `(0, 1024 -
    b, 1280, 1024)`, quirk O-351); top and bottom can differ by one pixel. `[0, 0]` without bars (bars exist only in
    game type -1, quirk O-350).
  - `portraits`, `texts`: the occupied slots (state bit 1) in slot order with opacity 0...255. Texts: slots 0...3 are
    speech lines, from 2 the hints overlap (`Tip` occupies `slot + 2`). Portraits are in the report even without bars
    (the state), but are drawn only with bars (quirk O-354).
  - `fade_alpha`: opacity of the scene fade, 0 clear, 255 black.
  - `pointer_visible`: the pointer is drawn (opposite of `ui_state.pointer_hidden`).
  - `clear_color`: clear color of the world as RGB 0...255 (cinemascope: fog color or `BackColor`, otherwise black).
  - `info_frames`: the open info windows 168/213 from head to end (the head lies on top) as `[text ID, [left, top,
    right, bottom]]` in the 1280 x 1024 grid. Only measured windows: measuring happens at the first drawing, so a
    window from `ui_present` appears only after a frame (`step`).
  - `countdown`: the script countdown as `"m:ss"` (`"%d:%02d"`), only with a registered container (`script_state`
    `countdown_container != 0`), otherwise `null`. It draws with the world at the top right at y = 20, below status bar
    and bars (quirk O-367).
  - `timers` (protocol 39): the remaining times of the timer list `m_CountDownList` (`sim.state.countdowns`, kind bit 1)
    as `"m:ss"` per entry in ID order, e.g. `["0:59"]`; empty without a timer (and in the menu without a simulation).
    These are the leader timer of the game types 2/3 and the time limit of game type 4. Seconds as in the original `ms /
    1000` (truncating), `"0:00"` at 0...999 ms. They are drawn by `FUN_00435EE0` part 2 at the top left (position `{0,
    0x28}` in the 1280 x 1024 grid, at 1024 x 768 therefore `(0, 30)`), red below 10 s, after the script countdown.
    Absent in older lines (empty).
  - `screen_text`: text ID of the screen text `n5` (kind 2, middle left) as long as it stands, otherwise `null`. It
    runs in game time of the message bar (stands in pause and game menu) and draws with the world.
  - Older lines without `info_frames`, `countdown`, `timers`, `screen_text` stay readable (empty or `null`).
  - **Viewer gap:** The viewer draws the combat display with egui, not through `GameSession::combat_cmds`; countdown,
    timer remaining time and screen text do not appear there, not even with `--hud` (known).
- Example (`opennml-game` in game type -1, at 1024 x 768):

      {"id": 1, "cmd": "ui_present", "event": {"Cinemascope": {"on": true}}}
      {"id": 2, "cmd": "ui_present", "event": {"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}}
      {"id": 3, "cmd": "step", "frames": 40, "frame_ms": 16.7}
      {"id": 4, "cmd": "ui_state"}   -> ... "game": {..., "presentation": {"bars": [127, 128],
          "portraits": [{"slot": 1, "portrait": 1000, "frame": 101, "alpha": 255}], "texts": [],
          "fade_alpha": 0, "pointer_visible": true, "clear_color": [0, 0, 0], "info_frames": [],
          "countdown": null, "timers": [], "screen_text": null}}

  Then `{"cmd": "ui_present", "event": {"Cinemascope": {"on": false}}}` back (game speed as before).

### Minimap, diplomacy and chat

Minimap, idle workers, signal, diplomacy, chat; protocol 37. Old lines stay readable: `minimap` and `chat_lines` are
then absent, `diplomacy_open` is `false`. Applies to `opennml-game` with a running match.

- **`ui_state.game.minimap`** (absent without the minimap node `IsMap`; read only, changes no checksum):

  | Field | Content |
  |---|---|
  | `visible` | the map is visible: node in the tree, switch `+0x4D` on (event `0x1D`) and no cinemascope; when hidden the node still takes the mouse (quirk O-291) |
  | `mode` | map mode (`GameWorld+0x22D4`): 0 all, 1 economy, 2 military; not in the savegame |
  | `center` | center of the diamond `[W - 89, H - 89]` in UI pixels (position of the last frame, before that from the screen size) |
  | `theta` | rotation angle theta in radians, `atan2(eye.z - target.z, eye.x - target.x) + pi/2` (position of the last frame; 0 before the first) |
  | `pings` | the pings in list order as `[tile x, tile z, remaining repeats]` (signal 1, script action 10, messages 5); not in the savegame |
  | `idle_workers` | length of the list of idle workers of the active slot (buttons 576/577); the list itself is in the savegame of the simulation |
  | `fog_swaps` | number of buffer swaps of the fog image since the start: every finished pass over all tile rows counts one, an immediate rebuild does not (this makes the fog cycle observable, quirk O-287) |

- **`ui_state.game.diplomacy_open`:** the dialog `DIPLOMATIE` is open (game menu state 10, `menu_state` likewise). The
  match stands meanwhile, commands of the dialog take effect immediately.
- **`ui_state.game.chat_lines`:** the active chat lines (screen texts of kind 10) as "name: text" in list order; waiting
  lines not yet (a second line appears only after `duration/6` = 1333 ms), gone after 8000 ms. If empty the field is
  absent.
- **`ui_click` on `GAMEUI`, elements of the minimap** (they take precedence over the bar names, also without and with a
  hidden command bar - the buttons then take the click but have no effect, quirk O-292):

  | Name | Target |
  |---|---|
  | `minimap` | center of the diamond: camera jump to the map center (left click; a right click with a selection gives a move command) |
  | `minimap_mode` | mode button (carries the GUID of the **next** mode 573/574/575, quirk O-292) |
  | `minimap_idle` | button 576: select the next idle worker (double-click: camera goes there) |
  | `minimap_signal` | button 571: signal (`sim_command` `Signal`) |
  | `minimap_idle_all` | button 577: select all idle workers |
  | `minimap_menu` | button 579: game menu (state 1) |
  | `minimap_diplomacy` | button 580: toggle the diplomacy dialog; the button exists only outside the campaign |

  Errors (error message): "element {e} does not exist in GAMEUI" (map without `IsMap`, buttons removed by the control
  lock or a hidden map, button 580 in the campaign).
  - **Double-click on 576/577:** `gameui_click` works with the clock of the program, which stands while the clock is
    stopped: two `ui_click` on `minimap_idle` (576) without a `step` in between are therefore always a double-click
    (event `0x27`: three workers one after another and the camera move to the last). Remedy: a `step` with more time
    than `DoubleClickTimeout` (default 200 ms, e.g. 5 steps of 50 ms) between the clicks. The original evaluates
    `0x27` only for 576 and 577; for the other buttons a double-click changes nothing.
  - **579/580 open a menu:** The flow executes the request immediately (state 1 or 10). A second click without a `step`
    in between is blocked by the open menu (the move goes through, press and release do not) and does not switch back;
    close with `ui_click` `DIPLOMATIE` `back`, ESC or `SHIFT D`.
- **Diplomacy dialog (`ui_click` on `DIPLOMATIE`, dialog name `diplomatie`):** Open with `ui_key` `SHIFT D` or
  `ui_click` `GAMEUI` `minimap_diplomacy`; under the control lock nothing opens. Elements per row i = slot 0...7:
  `view<i>` (vision checkbox, command `SharedVision`), `chat<i>` and `tribut<i>` (checkboxes of the chat and tribute
  mask, local only), plus the buttons `chatall`, `chatfriend`, `chatenemy`, `chatneutral` (chat mask by relation),
  `tributgold`, `tributwood`, `tributfood` (each 100 to the tribute mask) and `back` (close; ESC likewise). The status
  (`status<i>`) is a ComboBox and cannot be chosen through `ui_click`: use `sim_command` `{"player": 0, "kind":
  {"Diplomacy": {"p": 0, "q": 1, "mode": 0}}}` (0 friend, 1 enemy, 2 neutral).
- **Effect `diplomacy`** (in `ui_state.effects`, fixed form): `{"kind": "diplomacy", "action": ..., "slot": ...,
  "value": ...}`. `action` is one of `status` (`slot`, `value` = 0 friend / 1 enemy / 2 neutral), `view`, `chat`,
  `tribute` (each `slot` and `value` 1 on / 0 off), `chat_by` (`value` -1 all, 0 friend, 1 enemy, 2 neutral),
  `send_tribute` (`value` 0 gold, 1 wood, 2 food) or `close`. `slot` and `value` are absent where they do not apply.
  The effect is a report of the UI; the commands themselves (`Diplomacy`, `SharedVision`, `Tribute`) are in the command
  log.
- **Chat through `ui_key`:** `RETURN`, `SHIFT RETURN` and `CONTROL RETURN` open chat window 1 (all), 2 (friends,
  including the sender), 3 (chat mask); not in the cinemascope and not with an open menu. The text arrives character by
  character: one `ui_key` per character with any key (`chord` `A`) and the character as `char` (cp1252, at most 79
  characters); `RETURN` (or `ui_click` `chatwindow` `send`) sends, ESC only closes the window. An empty text sends
  nothing. The effect `{"kind": "send_chat", "window": 1...3, "text": ...}` shows the sending, the command `Chat`
  (`0x5D`) goes to the simulation, the line then appears in `chat_lines` - the cheats of the original (`mudhut`,
  `angelpass`, `bamboozle!`, `wheelofmayhem`, `rainbowring`, `bloodmode`) take effect in single player and show no
  line.
- **`sim_command` `Signal` and `Chat`** (JSON form of `opennml_sim::command::CommandKind`; the command takes effect at the
  next tick boundary): `{"player": 0, "kind": {"Signal": {"player": 0, "mask": 1, "x": 70, "z": 101}}}` - location in
  tiles, `mask` the recipients (bit per slot), `player` gives the color of the ping; the command has no state, the
  effect is sound, ping and an entry in the action stack at the host. `{"player": 0, "kind": {"Chat": {"player": 0,
  "mask": 255, "text": [104, 105]}}}` - `text` one UTF-16 unit per character (here "hi"), `player` must be the sender
  (otherwise rejected, exception A8).
- **Viewer:** With `--hud` the viewer draws the minimap including corner buttons, pings and camera frame through the
  same session as `opennml-game` (`GameSession::hud_cmds`); clicks go to the map with `mouse_move` and `mouse_button`. The
  viewer has no game menu, no diplomacy dialog and no chat window (requests 579/580 are logged) and no `GAMEUI`;
  `ui_state.game` does not exist there.
- **MCP:** `viewer_ui` describes the new elements (`element`) and fields (description of `state`); there is no new
  parameter. The description appears only after rebuilding the MCP server in a new session.

### World rendering

The state of the world rendering of the host (protocol 38) is in the viewer as `world_display` in the state snapshot
(`state`, response of `step`) and in `opennml-game` as `ui_state.game.world`. Without a map or without a simulation it is
absent (`null` in the viewer, field absent in `ui_state`); older lines without the field stay readable, as do lines
that lack individual subfields (then 0, empty, `false` or `null` apply). Read only: the checksum stays the same.
Everything is rendering of the host and is not in the savegame.

    {"rally_flag": {"building", "position": [x, y, z], "color": [r, g, b], "anim_ms"},
     "marks": [{"texture", "count", "visible"}, ...], "tread_points", "surf_rings", "stealth": [id, ...],
     "beams": [{"owner", "path", "position": [x, y, z], "target": [x, y, z], "flight_s", "particles"}, ...],
     "charges": [{"projectile", "position": [x, y, z], "yaw", "last_digit", "follows"}, ...]}

- `rally_flag`: rally point flag of the **last frame** (`null` = none; a single selected own building with button
  447): `building` = ID of the building, `position` = drawn position (stored rally point; with a rally target on top of
  the target), `color` = flag color (table `0x00664158`), `anim_ms` = clock of the effect world (stands in pause and
  game menu). Before the first frame, or after a change of selection without a frame, the old flag still stands here:
  after selecting, do a `step` first.
- `marks`: footprints and craters per texture, sorted by path; `count` all (a pair counts once), `visible` the listed
  ones (visible since the last game tick).
- `tread_points`: points of all wheel tracks (gaps included). `surf_rings`: living water rings of the units (they no
  longer count in `effects.surf`).
- `stealth`: IDs of the objects whose renderer entry in the last frame carries the silhouette bit `0x80` (foreign
  camouflaged units that walk; the original does not draw them when standing, quirk D-263), sorted.
- `beams`: running suction beams (`data/effects/Geist_angriff.pef`, `owner` = ghost) and animal evoker beams
  (`data/effects/Tierbeschwoerer.pef`, `owner` = evoker), sorted by ID: `position` = effect position of the last logic
  step, `target` = target point (`null` until it is set; the suction beam sets it per logic step from the first step in
  which the effect is listed), `flight_s` = flight time to the target (default 1), `particles` = living particles. A
  finished beam that is only fading out is absent.
- `charges`: dynamite charges by projectile ID (`projectile`): `position`/`yaw` = position and yaw of the last frame
  (explosion location), `last_digit` = last displayed digit (`m_LastCount`; 10 before the first digit), `follows` = the
  charge follows its target (charges found after loading do not).
- MCP: no parameters of its own; `viewer_state` delivers the state snapshot, `viewer_ui` with `action: state` the report
  of `opennml-game`.

### Flag game modes

Protocol 39. Old lines stay readable (`flags` and `timers` are then absent).

The game types 2 (Domination) and 3 (King of the Hill) evaluate player flags (GUID 4300). The state is in the viewer
as `state.sim.flags` (section `sim`) and in `opennml-game` as `ui_state.game.flags`; both deliver the same structure from
the same function (`opennml_client::game::sim_host::flags_info`). The field is absent if the game type is neither 2 nor 3
and no player flag is alive; older lines without the field stay readable. Read only: the checksum stays the same.

    {"flags": [{"id", "tile": [x, z], "color", "holder", "check_ms"}, ...],
     "counts": [9 counters], "leader", "time_limit_s", "remaining_ms"}

- `flags`: the living player flags in object order (slot, then index). `id` = object ID, `tile` = tile, `color` = flag
  color (9 = neutral, otherwise the color of the conqueror; an animal colors with 8), `holder` = the first slot 0 ... 7
  with this color, otherwise 8 (`FUN_00478DE0`; unoccupied slots keep color = slot, quirk D-300), `check_ms` = check
  counter of the flag (`m_Timer`, counts down from 1000 ms, at 0 the conquest check runs; -1 off).
- `counts`: flag counter per slot 0 ... 7, index 8 = neutral flags (`m_FlagsCounter`). The counter of the white player
  can become negative (quirk D-299).
- `leader`: flag leader (`m_FlagOwner`): slot 0 ... 7; 8 = nobody; 9 = unreachable (game type 3 without a flag, quirk
  W-160).
- `time_limit_s`: time limit in seconds (minutes * 60), the victory time of the leader.
- `remaining_ms`: remaining time of the leader timer (ID 0, quirk D-296); `null` if none is running - without a leader,
  but also after expiry and with a defeated leader, where `leader` stays set (so do not conclude `leader == 8` from
  `remaining_ms == null`); in game type 4 with a living flag the time limit of the game type is here (the same ID 0).
  `opennml-game` draws the same remaining time at the top left (`ui_state.game.presentation.timers`, quirk D-302).
- `sim_setup`: `mode: 2` (Domination), `time_limit` (minutes, 1-35791, default 20; outside it the error `time_limit = n
  invalid: allowed 1 to 35791 (minutes)`) and `slots[].color` 0-8 (8 = white, quirk D-298). `time_limit` is also in the
  settings file of the viewer (`time_limit`). `time_limit` applies to every game type but takes effect only in 2, 3
  and 4.
- MCP: `viewer_sim setup` with `mode`, `time_limit` and `slots[].color`; the new parameters appear in the schema only
  after rebuilding the MCP server in a new session (until then the raw control channel works). `viewer_state` delivers
  `sim.flags`, `viewer_ui` with `action: state` the report of `opennml-game`.

### Random map

With `--random-map <SEED>` the viewer generates a random map (`opennml-randommap`) and shows it instead of a level:
terrain, textures and light of the map script, **no objects and no simulation** (like a map without loadable rules:
`sim_*` answers "no simulation (map without loadable rules)", the camera stands at the map center). Works with
`--screenshot` and in the window. The seed text is arbitrary (seed = sum of `(i+1) * wchar[i]`); the other menu values:

| Switch | Default | Meaning |
|---|---|---|
| `--random-size` | 1 | size 0/1/2 = 64/128/256 tiles |
| `--random-players` | 4 | number of players 2-8 |
| `--random-terrain` | 0 | map type = index in `MSCRIPT` of `data/gifcon/randommaptypes.ini` (German edition: 0 = `continental`) |
| `--random-landscape` | 4 | landscape = index in `LTYPE` (German edition: 4 = `MEADOW`) |
| `--random-daytime` | 0 | time of day = index in `DTYPE` (0 = `DAY`) |
| `--random-game-type` | 0 | game mode = index in `GTYPE` (0 = `NORMAL`) |

- `--random-map` excludes `--level` and `--mission`; the `--random-*` switches need `--random-map`. There is no
  entry for it in the settings file. A mission from the "Match" window replaces the random map.
- Values outside the menu (players, size, index without an entry) and errors of the generator or map script end the
  viewer with a message like a missing level (with `--screenshot` before the image, in the window at the start).
- The log line `Random map "<seed>": <script> (size <dim>), defines ..., hash ...` names the resolved description; the
  level path of the scene is `randommap:<seed>` (overlay, loading line).
- `random_map` (without parameters, read only) answers:

      {"seed_text": "ZKA", "hash": 435, "script": "data/randommapscripts/continental.rnd",
       "defines": ["MAP_SIZE_MEDIUM", "NORMAL", "MEADOW", "DAY"], "dim": 128,
       "players": [[1, 26, 71, 16], ...], "rand_counts": [3, 514, 28493, 137322, 8272, 160264], "rand_state": 183993271,
       "digests": ["d9b87b721119723a", ...], "objects": [[3411, 20], [3500, 8], ...],
       "slots_used": [true, true, true, true, false, false, false, false],
       "start_points": [[3306.4, 0.0, 9066.3], ...]}

  `defines` = defines from the menu (the `#define`s of the script are absent), `players` = player areas `[id, cx, cz,
  min_size]`, `rand_counts` = `rand()` draws per phase (preprocessor, land areas, player areas, terrain areas, texture
  GUIDs, objects), `rand_state` = state of the sequence at the end, `digests` = FNV-1a 64 (16 hex digits) over heights,
  texture numbers, tile GUIDs, steepness, area IDs, placed objects and the block map of the object step
  (`opennml_randommap::Digests`, the same numbers as the corpus tests `digests_per_script` and
  `digests_with_objects_per_script`, there decimal). `objects` = placed objects as `[GUID, count]`, sorted by GUID;
  `slots_used` = slots 0...7 with a placed object of this owner; `start_points` = positions of the town centers (4200)
  in placement order (y = 0, as the generator places them). The preview lifts the objects to the terrain height when
  drawing. The state is built by `opennml_client::random_map_state::random_map_state`. Without a random map `null`.
- `opennml-game` answers `random_map` with the state of the random map of the running match
  (`GameSession::random_map_state`, the same fields), without a match or with a fixed map `null`. A random map starts
  there from the menu (`RANDOMMAP`, `CREATE2`, `START2`); `ui_state.effects` reports `start_match` with `level` =
  `randommap:<seed>`.
- Protocol 40, with objects and `opennml-game` protocol 41. MCP: no tool of its own; the start goes through `viewer_start`
  with `args: ["--random-map", "<seed>", ...]`, the query `random_map` through the raw control channel.

### Network play (command line)

`opennml-game` starts a network match without a lobby from the command line:

    OpenNML --host [PORT] --net-map <path.lvl> [--net-players N] [--net-ai K] [--turn-steps L] [--net-name NAME]
            [--net-seed S] [--net-loopback]
    OpenNML --join <ADDRESS[:PORT]> [--net-name NAME]

| Switch | Default | Meaning |
|---|---|---|
| `--host [PORT]` | `NetPort` of the settings | game port; 0 (also as `NetPort`) = 2302, any other value really applies (quirk E-254) |
| `--net-map` | required with `--host` | multiplayer map (path with `/multiplayer/`), game type 0 |
| `--net-players` | 2 | humans including the host (2 ... 8); the host waits for `N - 1` clients |
| `--net-ai` | 0 | AI slots after the humans (0 ... 7), the rest closed |
| `--turn-steps` | 2 | turn length `L` in ticks (1 ... 10) |
| `--net-name` | profile name `NetName`, otherwise "Spieler" | player name (cleaned: no control characters, at most 32 characters) |
| `--net-seed` | clock of the host (ms since 1970, folded to `u32`) | fixed seed for check runs |
| `--net-loopback` | off | only for check runs: the host listens on `127.0.0.1` instead of on all interfaces |
| `--join` | - | address of the host (IPv4, `[IPv6]` or name); without a port, 2302 |

- `--host` and `--join` exclude each other, as do `--screenshot` and `--paused`; the other `--net-*` switches and
  `--turn-steps` apply only with `--host` (`--net-name` also with `--join`). The direct start is not remembered in the
  settings.
- The host checks map and slots before the window, then listens (log line `Network: host listening on port <P>, map
  <map>, <N> players, <K> AI, seed <S>, raster <T>/<V>`; on all interfaces the Windows firewall asks for approval at
  the first start) and distributes the start as soon as all clients have signed on: human slots in order to the host and
  the clients in join order, resources, living space, time limit and "map explored" from its settings, its game speed
  for all. It closes a connection without `Hello` after 10 s.
- `--join` retries the connection setup for up to 10 s and waits in the same period for the acceptance; without a host
  the program ends with an error message and exit code 1 (log line `Network start: No host reachable at ..., given up
  after 10 s ...`). A rejection (other version, game edition, match full) ends it the same way.
- Until `Go`, `opennml-game` shows the main menu or the loaded map **with locked input** (window and control channel:
  `ui_click`, `ui_key`, `ui_open`, `mouse_move`, `mouse_button`, `key` answer "Input locked until the start of the
  network match"); it writes the state to the log (`Network: waiting for fellow players: N of M signed on (port P)`,
  `Network: loaded, waiting for the other machines (Go)`, `Network: match running`). If the start fails (load error,
  different rules checksum, the host aborts), the program ends with an exit code.
- If the host closes the program or window (also `quit`) in the running match, it ends in an orderly way for all: end
  in the next bundle, all write `savegame/multiplayer/netz_YYYY_MM_DD_hhmm`; the host waits for it at most 3 s. A
  client leaves the match with `Bye`.

In the network, the following applies at the control channel:

- `pause` and `step` are rejected ("In network play the clock runs in real time: ..."): if one machine stopped its
  clock, all the others would wait. Check scripts run in real time and read the state with `net_status` and `sim_hash`.
- `sim_command` puts a command of the **own** slot into the next turn contribution (it takes effect at the execution
  tick of the turn on all machines); a command with a foreign `player` and `sim_result` for a foreign slot are rejected
  ("In network play only commands for your own slot ...").

New commands (only `opennml-game`; the viewer answers "Network commands exist only in opennml-game (--host/--join)"). Without a
network session all answer with the error "no network session".

| Command | Response |
|---|---|
| `{"cmd": "net_status"}` | state of the session (below) |
| `{"cmd": "net_part_hashes"}` | `{"steps", "checksum", "parts": [{"part", "checksum"}, ...]}`: checksum per field of `SimState` in declaration order (like the desync report), `checksum` like `sim_hash`; without a loaded match "no match" |
| `{"cmd": "net_save", "name": "..."}` | save request like "Save" in the game menu: all machines write `savegame/multiplayer/<name>` after the execution tick of the turn that accepts it. Response `{"name"}`; before `Go`, after the end and with an invalid name an error |
| `{"cmd": "net_debug", "action": "corrupt"}` | check runs only: corrupts the own state (time of day) -> desync at the next comparison; response `{"action", "changed", "steps", "checksum"}` |
| `{"cmd": "net_debug", "action": "drop"}` | check runs only: cuts all connections without `Bye`; response `{"action", "links"}`. The peers see the dropout immediately, this machine only after 30 s of silence |
| `{"cmd": "net_debug", "action": "mute"}` | check runs only: turns all connections silent, also future ones; they send nothing anymore, not even keep-alives, but keep receiving. Response `{"action", "links"}`. After 30 s of silence the peers see a dropout and migrate to the next successor |
| `{"cmd": "net_debug", "action": "unmute"}` | cancels `mute`; response `{"action", "links"}`. Messages discarded while silent are not resent: for the peers the match can no longer be continued afterward (they see the dropout after 30 s of silence) |

`net_status` answers, e.g. at the host of a running match:

    {"role": "host", "phase": "running", "halt": null, "paused_by": null, "port": 23020, "players": 2,
     "peers": [{"peer": 0, "name": "Anna"}, {"peer": 1, "name": "Gast"}], "own_slot": 0,
     "seats": [{"slot": 0, "peer": 0, "name": "Anna"}, {"slot": 1, "peer": null, "name": ""},
               {"slot": 2, "peer": 1, "name": "Gast"}, ...],
     "round": 26, "next_exec_step": 212, "steps": 206, "waiting": null, "waiting_turn": null, "missing": [1],
     "desyncs": 0, "links": [{"peer": 1, "slot": 2, "silent_ms": 12}], "started": true, "finished": false,
     "failure": null}

- `role`: `host` | `client`; `phase`: `gathering` (sign-on), `loading` (start distributed, loading), `running`, `halted`
  (with `halt` = `pause` including `paused_by`, or `resync`), `stopped` (after the third desync), `ended`. `halted`
  appears as soon as the halt is decided (after receiving the halt bundle), while the machine still computes until the
  halt tick; the state stands only after `e(k)`. Check scripts wait for standing ticks or `shown_paused`. Speed key in
  a check run: `BracketRight` (main-block plus of the German layout); `NumpadAdd` has no effect (quirk O-307).
- `port`: at the host the own game port, at the client that of the host; `players` (host): humans of the match;
  `peers` (host): signed-on machines, host first.
- `own_slot`, `seats` (eight slots, `peer` 0 = host, `null` = AI or closed): from the start, before that `null` or
  empty.
- `round`: turn whose bundle is executed next; `next_exec_step` its execution tick; `steps` the engine tick of the
  match (without a loaded match `null`).
- `waiting`: why the machine is not computing - `starting` (before `Go`), `bundle` (bundle `waiting_turn` is missing),
  `halted`, `stopped`, `ended`; `null` as long as it computes. `missing` (host): slots whose contribution to the next
  turn is missing. `desyncs` (host): number of desyncs, at the client `null`.
- `links`: per **open** connection the time since the last reception (also keep-alives) in ms. If the machine closes a
  connection (dropout of the client at the host, abort to the host, `Reject`/`Bye`, failure, leaving, `net_debug
  drop`), it is absent from the list from then on; after a dropout the host therefore leads only the remaining
  clients.
- `started`: `Go` came; `finished`: the network match is over for this machine; `failure`: reason if the session has
  failed.

Protocol 42. MCP: no tool of its own; a check script drives two instances on the raw control channel (one `--control`
port per instance).

### Network play (menus and lobby)

Two to eight `opennml-game` find each other through the menus: `LAN1` searches for open games in the LAN, `LAN3` creates a
match (turn raster from `NetTurnRaster` of the settings file; `LAN3` has no choice field for it), the lobby `LAN2` with
`CHATFRAME` sets slots, ready, kick and chat, `START` starts the match. Check scripts drive the menus with
`ui_click`/`ui_key`/`ui_open` and read the state with `net_status`. Both paths use the transport over `std::net` (TCP
for the game, UDP port 2303 for the search).

Command line (changes compared with the command-line start):

    OpenNML --host [PORT] --net-map <path.lvl> [--net-players N] [--net-ai K] [--turn-raster T/V | --turn-steps L]
            [--net-name NAME] [--net-seed S] [--net-loopback]
    OpenNML --join <ADDRESS[:PORT]> [--net-name NAME] [--net-loopback]
    OpenNML [--net-loopback]                       (menu path)

| Switch | Default | Meaning |
|---|---|---|
| `--turn-raster T/V` | 8/4 (original) | turn spacing `T` and lead `V` in ticks, `1 <= V < T <= 40` (e.g. `4/2`, `2/1`); only with `--host`, excludes `--turn-steps` (`L` = `4L/2L`) |
| `--net-loopback` | off | only for check runs, now also without `--host`: a host (command line or menu) listens on `127.0.0.1` (TCP and search port), the search additionally asks `127.0.0.1:2303` - two instances on one machine without a firewall exception |

- The **command-line path** stays without a lobby dialog: `--host` creates a lobby with one row per human (row 0 the
  host, ready at once; nation, team and color in order from the default of the map; the AI appears only in the setup),
  a client sets itself ready after the first lobby state, and the host starts without a click as soon as all `N` humans
  are there and ready (checked like `START`). One machine too many gets "The match is full". The session name follows
  `NetSession` (empty: "Session - <name>", as in the menu).
- `--join` connects **in the event loop** (the window appears at once, log line `Network: connecting to ...`). Without a
  host, without an answer or on a rejection it leads after at most 10 s **back to the main menu** (the program no
  longer ends; log line `No host reachable at ..., given up after 10 s ...; back to the main menu`), `net_status` then
  reports `phase` = `ended` with `failure`. The same for a load error or an abort of the host before the start. If the
  session fails after the map is loaded but before `Go` (the host aborts or drops out), the standing map shows in both
  paths a `MSGBOX` with the reason, input is free, and "OK" leads to the main menu. Errors of the host before the
  window (map, slots, occupied game port) still end the program with an exit code.
- Input lock: in the command-line path until `Go` as before; in the **menu path** only while the map loads and waits
  for `Go`. In the search (`LAN1`), while connecting and in the lobby (`LAN2`), `ui_*`, `mouse_*` and `key` are free;
  `pause` and `step` are rejected in every network session (search and lobby run in real time like the turns).

`net_status` has new phases and fields; without a network session (no command-line path, in the menu neither search
nor lobby nor match) it stays at the error "no network session". Example at the host in the lobby of the menu path:

    {"role": "host", "phase": "lobby", "port": 2302, "players": null, "peers": [{"peer": 0, "name": "Anna"},
     {"peer": 1, "name": "Bert"}], "own_slot": null, "seats": [], "round": 0, "waiting": "starting", ...,
     "failure": null, "spacing": 8, "lead": 4, "build": "0123456789ab", "rtt_ms": [{"peer": 1, "ms": 3}],
     "lobby": {"session": "Session - Anna", "map": "data/levels/multiplayer/...lvl", "stage": "original", "own_row": 0,
               "rows": [{"row": 0, "kind": 999, "name": "Anna", "nation": 0, "team": 0, "handicap": 0, "color": 0,
                         "ready": true, "peer": 0},
                        {"row": 1, "kind": 999, "name": "Bert", ..., "ready": false, "peer": 1}, ...],
               "chat": ["Bert: Hello"], "max_rtt_ms": 3},
     "found": []}

- `role`: `host` | `client`, **`null` during the search**.
- `phase` new: `searching` (`LAN1` is searching), `connecting` (connection setup, sign-on, first lobby state; menu path
  and `--join`), `lobby` (lobby `LAN2` of the menu path, host and client). The command-line path still reports
  `gathering` before the start; after `START`, `loading`, `running` etc. follow from the turn core as in the
  command-line start.
- `spacing`, `lead`: raster `T`/`V` of the session (lobby) or the match; during the search `null`.
- `build`: build ID of this program (12 characters of the commit, with changes in the working tree `-` and 8 hex
  characters, without Git `unbekannt`). Two machines with different IDs do not find each other (`LAN1`) or are rejected
  ("Different program version: ... instead of ...").
- `rtt_ms` (host, lobby): last measured round-trip time per client; `lobby.max_rtt_ms` the largest.
- `lobby` (before the start, otherwise `null`): session name, `map` (path of the `.lvl`; for a random map the seed
  text), `stage` (`original` 8/4, `lan` 4/2, `lan_fast` 2/1, otherwise `T/V`), `own_row` (own row), `rows` (per row
  `kind` 0 open, 1 ... 4 computer, 999 human; `peer` 0 = host, `null` without a machine), `chat` (finished lines, oldest
  first).
- `found` (only during the search): visible games with `instance` (session ID, 16 hex digits), `name` (session name)
  and `addr` (address of the host).
- `net_debug drop` cuts the connections of the lobby driver in the lobby; without a driver (search, connection setup)
  it answers with `"links": 0`.
- Between the `JOIN` click in `LAN1` and the command `{7}` (it runs after the fly-out of `LAN1`) the search is already
  off and the connection setup has not begun yet: `net_status` reports "no network session" in these frames. Check
  scripts wait across this.
- **Known limit:** After a failed `--join`, `net_status` keeps reporting `phase` = `ended` with `failure`, even if the
  menu is used afterward - even with a search or lobby of the menu path: the state of the failed command-line path
  takes precedence over that of the menu session. Check scripts start their own instance without `--join` for the menu
  path.
- List rows (`OPENGAMES`, `LEVELS`) and entries of opened choice fields (nation, team, color, player in `LAN2`) have no
  name; the checks choose them with `mouse_move` and `mouse_button` (at 1024 x 768 rows about 21 pixels, the list of a
  choice field opens from its top edge).

### Network play (map change, game menu, match end)

No new commands and fields (`PROTOCOL_VERSION` unchanged); flows have changed that check scripts wait for:

- **Map change:** `ui_click` `lan2` `changegame` (visible only at the host) opens `lan31` with `loadmultiplayermap` or
  `randommap`; `lan31` `changegame` closes `lan31` only when the host has accepted the change (afterward `lan2`,
  `levelinfo`, `chatframe` are open again), on a rejection `lan31` stays open with `msgbox` 15402; `exitchange` returns
  without a change. Both see the change in `net_status.lobby`. The UI fills in the text of `LEVELINFO` only when drawing
  (`[CURLEVELINFOS]`); `ui_state` does not report it, check scripts use `screenshot`.
- **Game menu in the network:** ESC opens `ingameoptions2` (`menu_state` 2; buttons `savegame2`, `options2`, `quitgame`,
  `quit`, `return2`); `return2` closes without effect `resume_game`; `options2` opens `gameplayoptions2` and
  `soundoptions2` (return through `soundoptions2` `return`), the slider `gamespeed` is `hidden` at the client. The
  diplomacy (`SHIFT D`, dialog `diplomatie`, `menu_state` 10) and the game menu do not stop the ticks.
- **Match end in the network:** After `Go`, every end leads through the accounting: `gameend_defeat` `end_defeat` or
  `gameend_victory` `end_victory` (at the host with running turns first `msgbox` "The host has ended the match ..."
  with `ok`) -> `accounting` + `account1` (`ui_state.accounting`) -> `accounting` `mainmenu` -> `start`; afterward
  `net_status` answers "no network session". The confirmation of `quitgame` is the dialog `infoframe` (`ok`/`cancel`).
  If the host gives up in the game menu, both see the `msgbox` for the orderly end at once.

### Menu level

Protocol 45. Old lines stay readable (`menu_level` is then absent).

- **`ui_state.menu_level`** (only `opennml-game`; absent as long as no menu level is running, and in the viewer):
  `{"level", "shown", "steps", "checksum"}`. `level` = `.lvl` path of the menu level (normalized,
  `data/levels/menülevel/menü2.lvl`), `shown` = visible and running (bit `0x1` of `GameApp+0xD5E0`; false in submenus
  with their own `BACKGROUND` and under a video, the ticks then stand), `steps` = engine ticks of the simulation since
  the start, `checksum` = checksum of the simulation as hex text with 16 digits (like `sim_hash.checksum`). Reported
  in `ui_state`, `state` and the response of `step`. `game` stays reserved for the match: menu level and match exclude
  each other.
- **Effect `menu_level`** in `effects`: `0x21` with `load` = path of the `.lvl` (start or continue the menu level),
  `0x20` without `load` (stop and fade out).
- **`sim_hash` and `sim_query`** in `opennml-game` also read the simulation of the menu level. **`sim_command` and
  `sim_result`** answer with the error "Menu level takes no commands" as long as it runs (the simulation belongs to the
  scripts of `menü2.abl`). `pause`, `step` and `ui_*` as in the main menu.
- **Command-line paths:** `--host`, `--join` and `--screenshot` run without a menu level (`ui_state.menu_level` is
  absent); the setting `IsMenuLevel` stays unchanged in the settings file.

### 3D mouse pointer

The pointer with `HardwareCursor 0` is the model `mouse.rdo`. Protocol 48. Old commands and responses stay readable.

- **`screenshot.pointer`** (default `false`; only `opennml-game`, the viewer shows the system pointer and ignores the
  field): draws the pointer at the mouse position (`mouse_move`) last into the image, like the window: the TGA square
  with `HardwareCursor 1` (and without a loaded model), the 3D model with `HardwareCursor 0`, clipped over the minimap
  (quirk O-457). Without a mouse position in the window or with `pointer_hidden` the image stays without a pointer.
  The query does not fix the start of the animation: the pointer in the next window image still begins at phase 0, and
  the checksum of the simulation stays the same. The MCP schema of `viewer_screenshot` does not have the parameter yet;
  through the raw control channel `{"cmd": "screenshot", "pointer": true}` works.
- **`ui_state.pointer_model`** (only `opennml-game`, also in the main menu without a match; absent in the viewer and
  without a loaded `mouse.rdo`): `{"id", "animation", "tint", "phase_ms", "software"}`. `id` = chosen pointer 0-11,
  `animation` = running animation of the model, `tint` = `[r, g, b, a]` (build cursor and targeting mode tint only
  with software, quirks O-462/O-464), `phase_ms` = phase of the loop (absent as long as a restart is pending, until
  the next drawn frame), `software` = `HardwareCursor 0` (the model is drawn).

### Network play (network savegames)

Loading network savegames. Old responses stay readable (the new fields are then absent and count as `null`).

- **`net_status.lobby.saved`:** name of the network savegame (`savegame/multiplayer/<name>`) if the host has loaded a
  state (waiting room `LAN4`); otherwise `null`. `lobby.map` is then also the savegame name.
- **`net_status.lobby.rows[].seat`:** player slot 0 ... 7 of the row in a loaded lobby (one row per human slot that is
  still playing in the state, in slot order); in a new lobby `null`. In a loaded lobby the host stands in the row of
  its slot, not in row 0: `own_row` and `rows[].peer` (0 = host) say who stands where.

Example at the host in `LAN4` (Anna plays slot 3, Bert has joined):

    "lobby": {"session": "Session - Anna", "map": "netz1", "stage": "original", "own_row": 1,
              "rows": [{"row": 0, "kind": 999, "name": "Bert", ..., "ready": false, "peer": 1, "seat": 1},
                       {"row": 1, "kind": 999, "name": "Anna", ..., "ready": false, "peer": 0, "seat": 3}],
              "chat": [], "max_rtt_ms": 3, "saved": "netz1"}

### Network play (game data check)

Before a network match from the menu every machine checks in the background whether it has the same game data for the
map of the session as the host (rules checksum of the match). Protocol 51; old
responses stay readable (`session_revision` then counts as 0, `rules_checksum` as `null`, `data` as `equal`).

- **`net_status.lobby.session_revision`:** lobby revision of the last session change (creation, `CHANGEGAME`); the
  checksum and the verdicts belong to it.
- **`net_status.lobby.rules_checksum`:** the host's rules checksum for `session_revision` as 16 hex digits (like
  `sim_hash.checksum`), `null` while it computes, when its computation failed, and without the check (command-line
  path, loaded lobby `LAN4`).
- **`net_status.lobby.rows[].data`:** verdict of the row: `"pending"` (the machine has not reported for this session
  revision yet), `"equal"` (same checksum; also the host's row, computers, open rows and without the check),
  `"missing"` (the fixed map is missing on the machine or its map checksum differs) or
  `{"differs": {"files": [...], "more": n}}` (up to three differing paths, ascending, and the number of the others;
  an empty list means the files are not known). Only rows with `equal` can become ready; `START` needs a checksum and
  `equal` in every row.

Example at the host after Bert joined with a changed `bonus.ini`:

    "lobby": {"session": "Session - Anna", "map": "data/levels/multiplayer/...lvl", "stage": "original", "own_row": 0,
              "rows": [{"row": 0, "name": "Anna", ..., "ready": false, "peer": 0, "seat": null, "data": "equal"},
                       {"row": 1, "name": "Bert", ..., "ready": false, "peer": 1, "seat": null,
                        "data": {"differs": {"files": ["data/gifcon/bonus.ini"], "more": 0}}}, ...],
              "chat": [], "max_rtt_ms": 3, "saved": null, "session_revision": 0,
              "rules_checksum": "0123456789abcdef"}

### Replays

Recording after `SAVEREPLAY` and playback through `LOADAUTOPLAY` in `opennml-game`. Protocol 47. Old lines stay readable
(`replay` is then absent).

- **`ui_state.game.replay`** (only `opennml-game` with a match; also in `state` and the response of `step`; absent without
  recording and playback): `{"mode", "file", "step", "ended"}`. Read only, changes no checksum.

  | Field | Recording (`mode` = `record`) | Playback (`mode` = `play`) |
  |---|---|---|
  | `file` | file name in `savegame/autoplay` without a folder, e.g. `261108-1435` | name of the replayed recording |
  | `step` | tick of the simulation at the last update of the file (in every frame): the file reaches that far, even if the last command is earlier | tick of the next command of the file; after the end of the file the current tick of the simulation |
  | `ended` | the recording has ended (eight hours, write error, sync in the network); the field stays until the end of the match | all commands of the file are executed (end of the file); the match keeps running, the AI keeps computing |

  Example: `"replay": {"mode": "record", "file": "261108-1435", "step": 1200, "ended": false}`.
- **Effect `play_replay`** in `effects`: command `0x19` from `LOADAUTOPLAY` -> `START` with `name` = full file name in
  `savegame/autoplay`.
- **`sim_command` and `sim_result`** during playback still answer with `{"step"}`, but the command is discarded (debug
  log): playback takes only the commands of the file, the only thing accepted is the surrender of the viewing slot
  (`SetPlayerResult` with a defeat for the own slot, like `QUITGAME`). `sim_hash` and `sim_query` read as in the game.
- **`sim_load`** reads replays of the viewer (`NMLREPL`) and recordings of `opennml-game` (`NMLAUTO`) up to 576,000 ticks
  (eight hours); `sim_save replay` still writes `NMLREPL`.

## Middle mouse button (quirk O-331)

Like the original, `opennml-game` passes the middle button only into the table of held keys (VK 4 `MBUTTON`); the UI gets
no events. Held, `KD_CAM_FREEHAND` takes effect: with `LockCamera 0` the mouse movement rotates and tilts the camera,
with `LockCamera 1` (default of the installation) it moves it; the pointer jumps back to its position of the previous
frame every frame. Over the control channel:

    {"id": 1, "cmd": "mouse_move", "x": 512, "y": 400}
    {"id": 2, "cmd": "step", "frames": 1, "frame_ms": 16}
    {"id": 3, "cmd": "mouse_button", "button": "middle", "down": true}
    {"id": 4, "cmd": "step", "frames": 1, "frame_ms": 16}
    {"id": 5, "cmd": "mouse_move", "x": 552, "y": 380}
    {"id": 6, "cmd": "step", "frames": 1, "frame_ms": 16}
    {"id": 7, "cmd": "mouse_button", "button": "middle", "down": false}

The movement counts against the position of the previous frame (`GE+0x252C`); therefore do a step after `mouse_move`,
otherwise the movement since the last frame already takes effect at the press. The key is in the table from the next
round of the menu host, so in the first `step` after the press. After step 6 the pointer of the match stands at (512,
400) again. `opennml-game` resets the system mouse pointer only if the user holds the button on the real mouse; a check
run over the control channel does not move it. The viewer has no free mouse (own camera) and rejects `middle`;
`ui_click` with `middle` is already rejected by the check. MCP: `viewer_input` with `{"type": "mouse_button",
"button": "middle", "down": ...}` (new schema only in a new session).

## Screenshot

The viewer renders the current frame anew at the clock time (surface format like F12), optionally with the overlay of
the last frame (on the map including debug window, selection marks and frames of the last frame); with a minimized
window in the last size other than 0. `rgba_hash` is FNV-1a (64 bit) over the RGBA pixels of the full image: equal
checksum = pixel-identical. `max_size` shrinks only the PNG (longest side).

## MCP server `opennml-viewer-mcp`

stdio server; manages at most one viewer. It finds the repo root through `NML_REPO` or the path of its exe
(`<repo>/target/<profile>/opennml-viewer-mcp.exe`) and starts the viewer there.

| Tool | Parameters | Effect |
|---|---|---|
| `viewer_start` | `args` (list), `program`: `viewer` (default)/`game`, `profile`: `release`/`debug`, `paused` (default `true`), `audio` (default `false`) | starts `target/<profile>/opennml-viewer` (or `OpenNML` with `program: game`) `--control 127.0.0.1:0 [--paused] [--mute] <args>` (clock stopped at 0 ms; `paused: false` = real time; muted with `--mute`, unless `audio: true` or `args` already contains `--mute`), waits at most 30 s for the port line, connects; does not build (if the exe is missing: message with `cargo build ... -p opennml-viewer` or `-p opennml-game`) |
| `viewer_attach` | `port` | connects to a running viewer |
| `viewer_stop` | - | `quit`, wait for the process to end (5 s), otherwise kill the process |
| `viewer_input` | `actions`: list of `{"type": "key", "code", "action", "repeat"}` (`repeat` optional, only with `down`), `mouse_move`, `mouse_leave`, `mouse_button`, `wheel`, `step` | actions in order; result: state snapshot |
| `viewer_time` | `action`: `pause`/`resume`/`step` (+ `frames`, `frame_ms`) | result: state snapshot |
| `viewer_screenshot` | `overlay`, `max_size` (default 1280, `0` = full size) | PNG as image content, plus the text "{w} x {h} pixels, checksum {hash}" |
| `viewer_state` | - | state snapshot as JSON; with `opennml-game` the state of the UI (like `ui_state`, likewise the result of `viewer_input` and `viewer_time`) |
| `viewer_window` | `mode`, `monitor` | like F11/F10; result: state snapshot |
| `viewer_environment` | `action`: `lights`/`set_light` (+ `name`)/`set_weather` (+ `kind`)/`set_quality` (+ `shadow`, `particles`, `terrain`) | `lights`: name list; otherwise state snapshot (sections `environment` and `shadows`) |
| `viewer_effects` | `action`: `spawn` (+ `path`, `position` or `object_index`, `scale`)/`clear`/`set_damage` (+ `value`) | state snapshot (sections `effects`, `browser`, `effect_browser`) |
| `viewer_audio` | `action`: `play_sound` (+ `guid` and `sound_action` or `path`, `position`)/`music_play` (+ `track`)/`music_stop`/`music_attack` (+ `on`)/`set_volume` (+ `music`, `sfx`, `speech`, `mute`, `unit_response`)/`capture_start` (+ `path`)/`capture_stop` | `capture_stop`: length and levels; `play_sound`: `{"played", "id", "audio"}`; otherwise section `audio` |
| `viewer_sim` | `action`: `setup` (+ `mode`, `seed`, `nations`, `teams`, `game_speed`, `resource_setting`, `living_space`)/`command` (+ `command`)/`query` (+ `player`, `guid`, `ids`, `limit`)/`save` (+ `path`, `replay`)/`load` (+ `path`, `steps`)/`hash`/`path` (+ `unit` or `from` and `layer`, `to`, `allow_override`, `show`)/`grid` (+ `layer`, `rect`)/`move` (+ `ids`, `to`, `append`, `cyclic`, `drag_to`)/`unit_bar` (+ `button` 511-515 formation, 519 scatter, 405/406/432/433 riding and diving, 449-451 train forward/backward/stop, 485/486 arm/disarm, 543 ff. abilities, 3370-3377 counter-upgrades, 452/453 pack tent/unpack travois, 430/431 open/close gate, 434/435 camouflage/uncamouflage, 437/438 dig in/dig out, 458/554 start healing, 459/564 stop healing, 555 self-healing, 436 evoke ghost, 439/547/550/553/558/562 banish, 441/442/443 evoke wolf/bear/wildcat, protocol 21: 480/481 explosion/storm, 482 reincarnation, 484 scout area, 541 drain mana, 542 whirlwind, 454/455 dynamite at building/ship, 456/548 kill, 457/546 snipe, 551 damage building, 556 scout map, the target buttons with `target` (object) or `to` (ground point, goes to the viewer as `ground`); the buttons of protocols 20 and 21 act only on the first selected own object; protocol 22: 540 rob)/`build` (+ `guid`, `to`, `rotation_steps`, `ids` = workers, `append`)/`train` (+ `building`, `guid`)/`cancel` (+ `building`, `index`)/`complete` (debug "complete immediately", + `building`, `index`, optional `guid` = expected type)/`sell` (+ `ids` = buildings)/`action` (+ `ids`, `target`, `to`, `append`, `cyclic`)/`menu` (+ `ids` = workers or `building`)/`fog` (+ `player`, `rect`)/`view` (+ `player` = active player, `fog_shown`, `fog_overlay`)/`upgrade` (+ `player` = target slot 0-10, `guid` > 0, `remove`; protocol 16 - "apply all" with a negative GUID works in the MCP only through `command` with `{"ApplyUpgrade": {"slot": p, "guid": -1}}`, in the raw control channel through `sim_upgrade`)/`trade` (+ `player` = sender, `ids` = carts, `ware` 0-3, `target` = target slot 0-8; protocol 17)/`tribute` (+ `player` = giver, `recipients`, `resource` 0-2, `amount`; protocol 17)/`hire_menu` (+ `player` = viewer, `art` 0-2, optional `target_player`; protocol 22)/`ki_query` (+ `player`, `code`, optional `words`, `sight`; protocol 23)/`result` (+ `player` = slot 0-7, `result` 1 lost or 2 won; protocol 29); `setup` also with `map_explored`, (protocol 26) `slots`, `ki_enabled`, `campaign_difficulty` and (protocol 28) `script_enabled`, `time_per_char` as well as `time_limit` | result of the command as JSON (`setup`/`load`/`view`: section `sim`); `ids` empty or missing = selection |
| `viewer_ui` | `action`: `state`/`click` (+ `dialog`, `element`, `button` `left`/`right`)/`key` (+ `chord`, `char`)/`open` (+ `dialog`)/`video` (+ `name`)/`present` (+ `event`)/`focus_lost` (only `opennml-game`: `ui_focus_lost`) | commands `ui_state`/`ui_click`/`ui_key`/`ui_open`/`ui_video`/`ui_present`/`ui_focus_lost` (protocol 24 and later, "UI (`ui_*`)"): `state` = state of the UI (with `opennml-game` and a match including `game`), `click` = `{"x", "y"}`, `key` = `{"vk"}`, `open`, `video`, `present` and `focus_lost` = `null`; missing required fields: error, nothing sent. Only with an open UI (`opennml-game`, viewer with `--dialog` in the window) |
| `viewer_ki` | `action`: `state` (+ `slot`, `values`)/`query` (+ `slot`, `code`, `words`, `sight`)/`start` (+ `slot`, `module`, `args`, `sync`)/`pause` (+ `paused`)/`step` (+ `slot`, `passes`) | commands `ki_state`/`ki_query`/`ki_command` (protocol 26, "AI match"): `state` = AI state, `query` = request field and direct changes, `start`/`pause`/`step` = `{"step"}` (takes effect at the next tick); missing required fields and limits: error, nothing sent; fields of other actions are silently ignored (the raw control channel reports them as errors) |
| `viewer_script` | `action`: `state` (+ `block_container`, `block_index`, `names`)/`log` (+ `steps`)/`events` (+ `steps`)/`skip_dialog`/`start` (+ `mission`, `difficulty`, `profile`, `paused`) | commands `script_state`/`script_events`/`script_command` (protocols 28, 32, 33; "Presentation (protocol 33)"); `log` returns only the block log; `start` ends a running viewer and restarts it with `--mission`, muted, in a window |
| `viewer_log` | `lines` (default 50) | last lines of stderr of the started viewer (2,000 kept) |

If the viewer ends, all tools except `viewer_start` and `viewer_log` report "Viewer is not running (...)" with the last
log lines ("Last log lines:").

### Setup

    cargo build --release -p opennml-viewer -p opennml-viewer-mcp

`.mcp.json` in the repo registers the server for Claude Code when Claude Code is started in the repo directory
(confirm at the first start). Claude Code reads `.mcp.json` only from the start directory, and the memory of Claude
Code also depends on the start directory. Whoever starts Claude Code in a folder above the repo registers the server
there once with an absolute path (run in that folder):

    claude mcp add --scope local opennml-viewer -- <repo>\target\release\opennml-viewer-mcp.exe

`--scope local` applies only to that folder, `--scope user` to all projects. The start folder of Claude Code does not
matter for the server: it finds the repo through the path of its exe.

As long as a Claude Code session has the server running, Windows locks the exe; `cargo build --release -p
opennml-viewer-mcp` then fails when overwriting. End the session before the rebuild, restart it after the rebuild.

### Example: repeatable check

1. `viewer_start` with `args: ["--window-mode", "windowed"]` - the clock then stands at 0 ms, the sound is muted
   (`--mute`; audible only with `audio: true`)
2. (`viewer_time` `pause` is needed only if started with `paused: false`)
3. `viewer_input`: `key ArrowUp down`, `step 30 x 16 ms`, `key ArrowUp up`, `step 60 x 16 ms`
   -> the camera accelerates and brakes (`camera.target` in the result)
4. `viewer_screenshot` (note the checksum), `viewer_stop`; the same sequence after a restart gives the same values.

### Example: economy over the control channel

On a multiplayer map (read IDs and positions from `sim_query`):

    {"id": 1, "cmd": "sim_setup", "resource_setting": 2, "living_space": 3}
    {"id": 2, "cmd": "sim_query", "player": 0, "guid": 1100}
    {"id": 3, "cmd": "sim_menu", "units": [65537]}
    {"id": 3, "ok": {"building": null, "workers": [65537], "entries": [{"guid": 1001, "name": "SPA_Wohnhaus", "kind": "BUILD", "costs": [0, 150, 0, 0, 0], "affordable": true, "blocked": null}, ...], "queue": []}}
    {"id": 4, "cmd": "sim_build", "guid": 1001, "to": [5200.0, 6100.0], "rotation_steps": 1, "units": [65537]}
    {"id": 4, "ok": {"step": 12, "guid": 1001, "pos": [5188.0, 0.0, 6092.0], "rotation": 1.5707964, "rect": [39, 46, 41, 48], "workers": [65537], "affordable": true, "append": false}}
    {"id": 5, "cmd": "sim_action", "units": [65538], "target": 1179649}
    {"id": 5, "ok": {"step": 12, "button": 400, "action": "Chop wood", "command": "ChopWood", "code": 6, "queued": false, "cyclic": false, "kind": {"ChopWood": {"tree": 1179649, "units": [65538]}}}}
    {"id": 6, "cmd": "sim_train", "building": 65540, "guid": 1100}
    {"id": 7, "cmd": "step", "frames": 200, "frame_ms": 50.0}
    {"id": 8, "cmd": "sim_query", "ids": [65537, 65538, 65540]}

The response of `sim_query` then shows the construction site or the construction worker (`construct`), the lumberjack
with load and source (`gatherer`) and the production of the headquarters (`building.production`, `building.queue`);
rejections of the simulation (e.g. too little wood) appear as a message in `sim.notices` of the state snapshot. The
numbers above are examples.

### Host migration

`net_status` has two new fields:

- `successors`: the successor list, at the host from its core, at the client from `Go` or the last `Migrated`; empty
  before the start. Per entry `{"peer", "slot", "addr"}`, `addr` is the address under which the client listens, or
  `null` (not reachable from the host, not a candidate).
- `migration`: running migration of this machine, otherwise `null`. `{"phase", "peer", "expected", "arrived"}` with
  `phase` `dialing` (client connects to the candidate `peer`), `rejoining` (`Rejoin` sent to `peer`, waiting for
  `Migrated`) or `gathering` (new host gathers the `Rejoin`; `expected` the expected, `arrived` the already accepted
  clients, `peer` is `null`). In the first two phases `expected` and `arrived` are empty.

The slots of a machine after a migration are in `seats` and `own_slot`; the `peer` numbers in `lobby` of the frozen
lobby state stay those of the old host (the row of peer 0 belongs to the old host).
