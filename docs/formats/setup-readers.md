# Text readers: `default.dat`, `.lvl`, `defaultobjects.ini`, `upgrades.ini`, `dropdown.ini`

Rust: `nml_formats::config`, `nml_formats::gifcon`, `nml_formats::targets`. All readers work with MBCS strings
(cp1252 bytes). A data check against the extracted files is at the end.

## 0. Common building blocks

| Function | Behavior |
|---|---|
| `read_line` | Reads byte by byte; **every `\r` is discarded** (also in the middle of a line), `\n` ends the line. Returns `false` only at EOF **and** an empty line (a last line without `\n` is delivered). No trimming. |
| `trim_left` | Removes leading characters for which `_ismbcspace` holds (space, `\t`, `\n`, `\v`, `\f`, `\r`) |
| `trim_right` | Counterpart (used only in the upgrade reader) |
| `left(n)` / `mid(n)` | `n` counts from the start of the line (not from the place where something was found!); `n >= len` -> the whole line or empty |
| `contains(key)` (`_mbsstr`) | Substring anywhere in the line, case-sensitive |
| `==` (`_mbscmp`) | Exact comparison, case-sensitive |
| `make_lower_german` | Only `A-Z` -> `a-z`, `Ä`(0xC4)->`ä`(0xE4), `Ö`(0xD6)->`ö`(0xF6), `Ü`(0xDC)->`ü`(0xFC); everything else unchanged (no `É` etc.) |
| `sscanf("%d")` | MSVC: skips whitespace, reads an optional sign + digits; on failure the target stays **unchanged** |
| `atol` | As in C, failure -> 0 |

**Value objects of the configuration** (value set, a map from `u32` to value): a value stores **text** and a type.
Setting an integer `v` stores `text = format!("{}", v)`, type 1. Reading an integer from a value set is `atol(text)`,
missing -> 0. Setting replaces an existing entry (delete the old one, set the new one). Copying a value set copies
every entry of the source into the target.

## 1. Configuration and resource readers

### Dropdown lookup `dropdown_lookup(cfgGuid, &text)`

```rust
fn dropdown_lookup(w, cfg: i32, text: &mut CString) -> i32 {
    make_lower_german(text);                       // IN PLACE
    if let Some(list) = w.dropdowns.get(cfg) {
        // entries in insertion order, exact comparison, first hit
        if let Some(e) = list.iter().find(|e| e.text == *text) { return e.value; }
        return text as *const _ as i32;            // QUIRK: pointer value of the string (output slot unchanged)
    }
    let mut out = text as *const _ as i32;         // the same slot
    sscanf(text, "%d", &mut out);                  // failure (empty/text) -> pointer value
    out
}
```

In the shipped data every text hits (all 2,699 selection values in `default.dat`/`.lvl`), there are no empty values;
the pointer case does not occur -> treated as a load error in the reimplementation.

### Config section parser

`parse_config_sections(file, target: &mut Map<u32,ValueSet>, kind: 1|2|3, released_only: bool)`

State `st`: 0 outside, 1 in the section, 2 in the object, 3 in the value block. `n_cfg` (start value =
`released_only`), `obj_guid`, `cfg_id`, `cur_set` and `enable` are further variables.

```rust
let header = ["[CONFIGDATA]", "[UPGRADES]", "[PATTERNS]"][kind-1];
let objkey = ["OBJECT_GUID", "UPGRADE_GUID", "PATTERN_GUID"][kind-1];
while let Some(mut line) = read_line(f) {
    trim_left(&mut line);
    if line.contains(header) { st = 1; read_line(f); continue; }         // next line ("{") discarded
    if line.contains("}") {
        if st <= 1 { return; }                     // also st == 0: every "}" BEFORE the header ends!
        if st <= 2 { st = 1; continue; }
        if st <= 3 { st = 2; continue; }
    }
    if st == 0 { continue; }
    if line.contains(objkey) {                     // mid(12) for all three keys (all 12 characters long)
        line = line.mid(12); sscanf(&line, "%d", &mut obj_guid);   // failure -> old value
        n_cfg = 0; st = 2; read_line(f);           // "{" discarded
        if !released_only
           && let Some(def) = w.cfg_default.get(obj_guid)
           && !target.contains(obj_guid) {
            let mut vs = ValueSet::new(); vs.copy_from(def); target.insert(obj_guid, vs);
        }
        continue;
    }
    if line.contains("CONFIG_GUID") {             // no st check (only st != 0)
        if n_cfg == 0 {
            cur_set = match target.get(obj_guid) { Some(s) => s,
                                                   None => { target.insert(obj_guid, ValueSet::new()) } };
        }                                          // QUIRK: otherwise cur_set from the last time (same object)
        n_cfg += 1;
        line = line.mid(12); sscanf(&line, "%d", &mut cfg_id);
        read_line(f);                              // "{" discarded
        st = 3; enable = false; continue;          // switch reset per CONFIG_GUID (no inheriting)
    }
    if line.contains("CONFIG_ENABLE") { enable = line.contains("TRUE"); continue; }  // case-sensitive, substring
    if line.contains("CONFIG_VALUE") {
        if !enable && released_only { continue; }
        let v = Value::new(); cur_set.set(cfg_id, v);   // replaces an existing value (also one copied from default.dat)
        line = line.mid(14);                       // "CONFIG_VALUE" + 1 separator + '"'
        line = line.left(line.len() - 1);          // drop the last character (closing '"'), NO trim_right
        v.set_int(dropdown_lookup(w, cfg_id, &mut line));
    }
}
```

Quirks: keys are searched by `contains` anywhere in the line (a `CONFIG_LABEL "...TRUE..."` triggers nothing, though;
only `CONFIG_ENABLE` lines count); a `}` anywhere in a line changes the state; `mid(n)` counts from the start of the
line after `trim_left` (format "key + exactly one separator" required); whitespace after the closing `"` stays in the
text (dropdown miss -> pointer value; numbers are uncritical, `%d` stops at the `"`). `CONFIG_LABEL` is not evaluated.

### `default.dat` reader

Opens the file, then calls `parse_config_sections(f, &mut w.cfg_default, kind, true)` **in sequence** for
`kind = 1, 2, 3` (the sections must appear in this order; the header line `PROJECT CONFIGFILE V1.4b` is ignored).
`released_only = true` -> only `CONFIG_ENABLE TRUE`, no copy. Objects (`OBJECT_GUID`) and upgrades (`UPGRADE_GUID`) end
up in the **same map** (GUID spaces are disjoint). Shipped data: 512 objects, 232 upgrades, `[PATTERNS]` empty.

### `.lvl` reader

Main loop, per line after `trim_left`, in this order (all `contains`):

```rust
if line.contains("[PLAYER]")    { read_line(f); }                                   // "{" discarded
else if line.contains("[NEUTRAL]") { read_line(f); parse_config_sections(f, &mut w.cfg_sets[8], 1, false); }
else if line.contains("[RESOURCES]") { parse_level_resources(f); }
else if line.contains("PLAYER_NAME") {
    let t = line.mid(13); let t = t.left(t.len()-1);       // "PLAYER_NAME" + separator + '"'
    let n = atol(&t);
    if n > 0 && n != 29320 { name_buf = text_table(n); }   // wcscpy
    else { for (i, b) in t.bytes().enumerate() { name_buf[i] = b as i8 as i16 as u16; } }  // QUIRK: sign extension,
}                                                          // bytes >= 0x80 become 0xFFxx; buffer of 256 wchar, no bound
else if line.contains("PLAYER_PLAYER") {
    let mut t = line.mid(15); t = t.left(t.len()-1);       // "PLAYER_PLAYER" + separator + '"'
    nation = dropdown_lookup(w, 0, &mut t);
}
else if line.contains("PLAYER_TYPE")  { ptype = atol(line.mid(12)); }
else if line.contains("PLAYER_COLOR") {
    let c = atol(line.mid(13));
    if c < 8 { /* create the player: active, name, control = ptype+1, nation, result 0 */
        parse_resources(f, c);
        for kind in 1..=3 { parse_config_sections(f, &mut w.cfg_sets[c], kind, false); }
        if engine.mode <= -1 { ui.register(c, name_buf, c, nation, c); }
    }
}
```

`name_buf`, `nation` and `ptype` are locals without a start value (a missing key -> the value of the previous player
or garbage). `PLAYER_LINETOOL`, `LEVELFILE` and `PROJECT_FILE` are not recognized.

### Level `[RESOURCES]` (top level) `parse_level_resources(file)`

```rust
let mut st = 0;
while let Some(mut line) = read_line(f) {
    trim_left(&mut line);
    if line.contains("{") { st = 1; continue; }                 // the header here is "{"
    if line.contains("}") { if st <= 1 { return; } st = 1; continue; }   // QUIRK: 3rd branch is dead code
    if st == 0 { continue; }
    if line.contains("CONFIG_GUID") { sscanf(&line.mid(12), "%d", &mut cfg); read_line(f); st = 2; continue; }
    if line.contains("CONFIG_VALUE") {
        let mut t = line.mid(14); t = t.left(t.len()-1);
        w.level_res /* map i32 -> i32 */.insert(cfg, dropdown_lookup(w, cfg, &mut t));
    }                                                            // CONFIG_ENABLE is not considered
}
```

Values: `0` wood, `1` food, `2` gold (numbers), `40` season/time of day (dropdown "sommer tag" = 0 ... "winter
nacht" = 3). `40` is read only when the player handicaps are applied (`level_res.get(40).unwrap_or(0)`, which then
applies the time-of-day upgrades).

### Player `[RESOURCES]` (inside a `[PLAYER]`) `parse_resources(file, color)`

```rust
let p = engine.get_player(color);                // None for color not in 0..7
if let Some(p) = p {                             // default from the level [RESOURCES], BEFORE the own block
    if let Some(v) = w.level_res.get(0) { p.set_resource(1, v); }   // wood
    if let Some(v) = w.level_res.get(1) { p.set_resource(2, v); }   // food
    if let Some(v) = w.level_res.get(2) { p.set_resource(0, v); }   // gold
}
let mut st = 0;
while let Some(mut line) = read_line(f) {
    trim_left(&mut line);
    if line.contains("{") { st = 1; continue; }
    if line.contains("}") { if st <= 1 { return; } st = 1; continue; }
    if st == 0 { continue; }
    if line.contains("CONFIG_GUID") { sscanf(&line.mid(12), "%d", &mut cfg); read_line(f); st = 2; continue; }
    if line.contains("CONFIG_VALUE") {
        sscanf(&line.mid(13), "\"%d\"", &mut v);  // QUIRK: failure -> v from the previous value (garbage at first)
        if let Some(p) = p { match cfg { 0 => p.set_resource(1, v), 1 => p.set_resource(2, v),
                                         2 => p.set_resource(0, v), _ => {} } }
    }                                            // no dropdown, CONFIG_ENABLE does not matter
}
```

`set_resource(i, v)`: only `0 <= i < 5`, stores **`v << 12`**. Players without a `[PLAYER]` keep the constructor values
30000/30000/30000/0/0 (the level default applies only to slots with a `[PLAYER]`). Multiplayer: afterwards the resource
presets may overwrite them ([resourcenconfig.md](resourcenconfig.md); for all 9 player structures, if the match is on
a random map or the resource setting is > 0, and the setting is in 0 ... 3:
`set_resource(entry.idx, entry.values[setting])`, internal indexes 0 gold/1 wood/2 food).

### `[UPGRADES]` (short)

The same reader with `kind = 2`, key `UPGRADE_GUID`; per upgrade a copy of the `default.dat` entry into the player
set, values as above (`CONFIG_GUID 1` "enabled/disabled" via dropdown -> 1/0, costs 10-13, percentages 114...).
`[PATTERNS]` (`kind = 3`, `PATTERN_GUID`) is empty in the shipped data.

## 2. `defaultobjects.ini` (`data/gifcon/defaultobjects.ini`)

```rust
let (mut in_gt, mut in_nat, mut in_sup) = (false, false, false);
let (mut gt, mut nat, mut sup);                  // all three read from the same uninitialized slot
while let Some(mut line) = read_line(f) {
    trim_left(&mut line);
    if line.left(7) == "SUPPLY "        { sup = atol(line.mid(7)); in_sup = true; }
    else if line == "/SUPPLY"           { in_sup = false; }      // whole line (no trim_right!)
    else if line.left(9) == "GAMETYPE " { gt = atol(line.mid(9)); in_gt = true; }
    else if line == "/GAMETYPE"         { in_gt = false; }
    else if line.left(7) == "NATION "   {
        let mut t = line.mid(7); nat = dropdown_lookup(world, 0, &mut t); in_nat = true;
    }
    else if line == "/NATION"           { in_nat = false; }
    else if line.left(12) == "PLACEOBJECT " {
        if !in_nat { close(f); return false; }   // abort, rest of the file unread
        sscanf(&line.mid(12), "%d %d %d", &mut guid, &mut dx, &mut dz);   // failure -> old values
        if in_gt  { self.start_lists[nat + gt*6].push((guid, dx, dz)); }
        if in_sup { self.supply_lists[nat + sup*6].push((guid, dx, dz)); } // both possible
    }
}
close(f);
!(in_gt || in_nat)                               // open block -> false (lists are filled anyway)
```

Quirks: comparisons are case-sensitive; `#` comments fall through all comparisons; **no range check** for
`nat + gt*6` (the table holds 42 lists = game type 0...6 x nation 0...5; the pointer value of an unknown nation or a
game type >= 7 writes into foreign memory). Shipped data: 42 `NATION` blocks, 216 `PLACEOBJECT`, one `SUPPLY 0` block
(railroad), every `PLACEOBJECT` inside a `NATION` block, no spaces at the end of a line.

## 3. `upgrades.ini` (variant bit table)

```rust
while let Some(mut line) = read_line(f) {
    if line.is_empty() || line.as_bytes()[0] == b'#' { continue; }   // '#' only in column 0, NO trim_left
    make_lower_german(&mut line);
    if line.left(3) != "bit" { continue; }      // leading whitespace -> line ignored
    line = line.mid(3);
    let Some(p) = line.find('=') else { continue };
    let mut key = line.left(p); trim_left(&mut key); trim_right(&mut key);
    let bit = atol(&key);                        // "" -> 0
    if bit < 0 { continue; }
    line = line.mid(p + 1); trim_left(&mut line); trim_right(&mut line);
    let mut list: Option<CList<i32>> = None;
    while !line.is_empty() {
        let tok = match line.find_any(&[' ', '\t']) {       // _mbspbrk(" \t")
            None => { let t = line.clone(); line.clear(); t }
            Some(q) => { let t = line.left(q); line = line.mid(q + 1); trim_left(&mut line); t }
        };
        list.get_or_insert_with(CList::new).push(atol(&tok));   // not a number -> 0
    }
    if let Some(l) = list { self.entries.push((bit, l)); }  // append, duplicates stay
}
```

Multiple spaces/tabs between values are harmless (`trim_left` after every separator); an empty value (`bit5=`) -> no
entry; a missing `=` -> line ignored; the case of `BIT` does not matter.

## 4. Dropdown reader (`data/projects/dropdown.ini`)

```rust
while let Some(line) = read_line(f) {           // NO trim_left
    if !line.contains("VALUEGUID") { continue; }
    sscanf(&line.mid(10), "%d", &mut guid);     // "VALUEGUID " = 10; failure -> old value
    let list = DropdownList::new();
    w.dropdowns.insert(guid, list);             // duplicate VALUEGUID: replaces (the old list leaks)
    list.parse(f);
}
// DropdownList::parse:
loop {
    let Some(mut line) = read_line(f) else { return };
    if !line.contains("DROPDOWN") { return; }   // QUIRK: this line is consumed (a VALUEGUID line directly
                                                // following would be lost)
    line = line.mid(9);                         // "DROPDOWN " = 9
    sscanf(&line, "%d", &mut value);            // failure -> old value
    let p = line.find(' ').map(|i| i as i32).unwrap_or(-1);
    line = line.mid((p + 1) as usize);          // text = everything after the first space (after the number);
                                                // without a space the whole line; a tab does NOT separate
    make_lower_german(&mut line);               // lowercase when loading
    self.entries.push(Entry { value, text: line });   // duplicate texts: the first one wins when searching
}
```

No `trim_right`: spaces at the end of a line belong to the text (shipped data: none). Shipped data: 7 lists
(VALUEGUID 0 nations, 1 enabled, 40 time of day, 16, 18, 21, 34), cleanly separated by empty/comment lines.

## Data check (extracted files)

- `default.dat` + 55 `.lvl`: 9,755 `CONFIG_ENABLE TRUE`, 2,652 `FALSE` (no other spellings); all `CONFIG_VALUE` lines in
  the form `CONFIG_VALUE\t"..."`, no whitespace after the `"`; all selection texts hit (after lowercasing), no empty
  values; no `}` in text lines.
- `dropdown.ini`: CRLF, no tabs/double spaces in `DROPDOWN` lines, no lost `VALUEGUID`.
