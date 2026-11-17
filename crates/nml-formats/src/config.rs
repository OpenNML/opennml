//! Configuration values (`CONFIG_GUID`) from `data/projects/default.dat`, the `.lvl` and the
//! dropdown lists `data/projects/dropdown.ini`, read **line by line like the game**
//! (`PlayerSetup_ParseConfigSections` `0x00482870`, `PlayerSetup_ParseLvl` `0x00486640`,
//! `GameWorld_LoadDropdownIni` `0x00482610`), see `docs/formats/config-guids.md`.
//!
//! Common rules: every `\r` is dropped, lines are only trimmed on the left, keys are
//! substrings anywhere in the line (case matters), values start at a fixed column
//! (`Mid(n)` from the line start), a failed `sscanf("%d")` leaves the old value in place.

use std::collections::BTreeMap;

use crate::FormatError;
use crate::property::atol;

/// Path of the default configuration.
pub const DEFAULT_DAT_PATH: &str = "data/projects/default.dat";
/// Path of the dropdown lists.
pub const DROPDOWN_PATH: &str = "data/projects/dropdown.ini";

/// Lines like `GameFile_ReadLine` (`0x00574C00`): `\r` discarded, `\n` separates, a last line
/// without `\n` counts if it is not empty.
pub fn game_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(|l| l.replace('\r', "")).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// `CString_TrimLeft` (`0x004058B0`): whitespace `_ismbcspace` (space, `\t`-`\r`).
pub fn trim_left(line: &str) -> &str {
    line.trim_start_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r'])
}

/// `CString_Mid(n)`: from character `n` (cp1252 = one byte per character), empty beyond the end.
pub fn mid(line: &str, n: usize) -> &str {
    line.char_indices().nth(n).map_or("", |(i, _)| &line[i..])
}

/// `CString_Left(n)`: the first `n` characters.
pub fn left(line: &str, n: usize) -> &str {
    line.char_indices().nth(n).map_or(line, |(i, _)| &line[..i])
}

/// Without the last character (`Left(len - 1)`).
fn without_last(text: &str) -> &str {
    let mut chars = text.chars();
    chars.next_back();
    chars.as_str()
}

/// `sscanf("%d")`: whitespace, sign, digits; `None` without a digit.
pub fn scan_int(text: &str) -> Option<i32> {
    let t = trim_left(text);
    let digits = t.strip_prefix(['-', '+']).unwrap_or(t);
    digits
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| atol(t) as i32)
}

/// `CString_MakeLowerGerman` (`0x00574DD0`): only `A`-`Z`, `Ä`, `Ö`, `Ü`.
pub fn lower_german(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'Ä' => 'ä',
            'Ö' => 'ö',
            'Ü' => 'ü',
            _ => c,
        })
        .collect()
}

/// Dropdown lists per `CONFIG_GUID` (`GameWorld+0x974`): number and lowercased text in
/// file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dropdowns {
    pub lists: BTreeMap<i32, Vec<(i32, String)>>,
}

impl Dropdowns {
    /// `Dropdown_LookupValue` (`0x00481230`): lowercase the text; if there is a list, the
    /// first exact match, otherwise `sscanf("%d")`. Where the game would return a pointer value
    /// (text missing from the list, no number), the result is `None` (load error).
    pub fn lookup(&self, cfg: i32, text: &str) -> Option<i32> {
        let text = lower_german(text);
        match self.lists.get(&cfg) {
            Some(list) => list.iter().find(|(_, t)| *t == text).map(|(v, _)| *v),
            None => scan_int(&text),
        }
    }
}

/// `dropdown.ini` like `GameWorld_LoadDropdownIni`: `VALUEGUID` (value from column 10) begins a
/// list (a duplicate replaces it), then `DROPDOWN number text` up to the first line without
/// `DROPDOWN` (that line is consumed); text = everything after the first space, lowercase.
pub fn parse_dropdowns(text: &str) -> Dropdowns {
    let lines = game_lines(text);
    let mut out = Dropdowns::default();
    let mut guid = 0i32;
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        i += 1;
        if !line.contains("VALUEGUID") {
            continue;
        }
        if let Some(g) = scan_int(mid(line, 10)) {
            guid = g;
        }
        let mut entries = Vec::new();
        let mut value = 0i32;
        while i < lines.len() {
            let line = &lines[i];
            i += 1;
            if !line.contains("DROPDOWN") {
                break;
            }
            let rest = mid(line, 9);
            if let Some(v) = scan_int(rest) {
                value = v;
            }
            let text = match rest.find(' ') {
                Some(p) => &rest[p + 1..],
                None => rest,
            };
            entries.push((value, lower_german(text)));
        }
        out.lists.insert(guid, entries);
    }
    out
}

/// Configuration values per object or upgrade GUID (`CMap<GUID, ValueSet>`).
pub type ConfigSets = BTreeMap<u32, BTreeMap<u32, i32>>;

/// Kind of a section of [`parse_config_sections`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// `[CONFIGDATA]`, key `OBJECT_GUID`.
    Objects,
    /// `[UPGRADES]`, key `UPGRADE_GUID`.
    Upgrades,
    /// `[PATTERNS]`, key `PATTERN_GUID`.
    Patterns,
}

impl SectionKind {
    const ALL: [Self; 3] = [Self::Objects, Self::Upgrades, Self::Patterns];

    fn header(self) -> &'static str {
        match self {
            Self::Objects => "[CONFIGDATA]",
            Self::Upgrades => "[UPGRADES]",
            Self::Patterns => "[PATTERNS]",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Objects => "OBJECT_GUID",
            Self::Upgrades => "UPGRADE_GUID",
            Self::Patterns => "PATTERN_GUID",
        }
    }
}

/// Read cursor over the lines of a file (the readers of the game share one file pointer).
pub struct LineReader {
    lines: Vec<String>,
    next: usize,
}

impl LineReader {
    pub fn new(text: &str) -> Self {
        Self {
            lines: game_lines(text),
            next: 0,
        }
    }

    /// Next line (`None` at the end of the file).
    pub fn read(&mut self) -> Option<String> {
        let line = self.lines.get(self.next).cloned()?;
        self.next += 1;
        Some(line)
    }

    /// Line number (1-based) of the last line read.
    pub fn line_no(&self) -> usize {
        self.next
    }
}

/// `PlayerSetup_ParseConfigSections` (`0x00482870`): reads a section `kind` into `target`, starting at
/// the current line. Every `}` before the section header or at the section end ends the reader.
/// `CONFIG_ENABLE` applies per `CONFIG_GUID` (substring `TRUE`); with `only_enabled` (`default.dat`)
/// only enabled values count, otherwise (`.lvl`) all, and an object that is missing in the target is
/// first copied from `defaults`. `CONFIG_VALUE` (text from column 14 without the last character)
/// always goes through the dropdown list.
pub fn parse_config_sections(
    reader: &mut LineReader,
    target: &mut ConfigSets,
    kind: SectionKind,
    only_enabled: bool,
    defaults: Option<&ConfigSets>,
    dropdowns: &Dropdowns,
) -> Result<(), FormatError> {
    let mut state = 0u8;
    let mut object: u32 = 0;
    let mut cfg: i32 = 0;
    let mut enable = false;
    while let Some(raw) = reader.read() {
        let line = trim_left(&raw);
        if line.contains(kind.header()) {
            state = 1;
            reader.read();
            continue;
        }
        if line.contains('}') {
            match state {
                0 | 1 => return Ok(()),
                2 => {
                    state = 1;
                    continue;
                }
                _ => {
                    state = 2;
                    continue;
                }
            }
        }
        if state == 0 {
            continue;
        }
        if line.contains(kind.key()) {
            if let Some(g) = scan_int(mid(line, 12)) {
                object = g as u32;
            }
            state = 2;
            reader.read();
            if !only_enabled
                && let Some(def) = defaults.and_then(|d| d.get(&object))
                && !target.contains_key(&object)
            {
                target.insert(object, def.clone());
            }
            continue;
        }
        if line.contains("CONFIG_GUID") {
            target.entry(object).or_default();
            if let Some(c) = scan_int(mid(line, 12)) {
                cfg = c;
            }
            reader.read();
            state = 3;
            enable = false;
            continue;
        }
        if line.contains("CONFIG_ENABLE") {
            enable = line.contains("TRUE");
            continue;
        }
        if line.contains("CONFIG_VALUE") {
            if !enable && only_enabled {
                continue;
            }
            let text = without_last(mid(line, 14));
            let value = dropdowns.lookup(cfg, text).ok_or_else(|| {
                FormatError::Invalid(format!(
                    "line {}: value \"{text}\" for CONFIG_GUID {cfg} is neither a dropdown text nor a number",
                    reader.line_no()
                ))
            })?;
            target.entry(object).or_default().insert(cfg as u32, value);
        }
    }
    Ok(())
}

/// `default.dat` like `PlayerSetup_ParseDefaultDat` (`0x004864D0`): `[CONFIGDATA]`,
/// `[UPGRADES]`, `[PATTERNS]` in sequence with only enabled values into **one** table (object and
/// upgrade GUIDs are separate ranges).
pub fn parse_default_dat(text: &str, dropdowns: &Dropdowns) -> Result<ConfigSets, FormatError> {
    let mut reader = LineReader::new(text);
    let mut sets = ConfigSets::new();
    for kind in SectionKind::ALL {
        parse_config_sections(&mut reader, &mut sets, kind, true, None, dropdowns)?;
    }
    Ok(sets)
}

/// A `[PLAYER]` of the `.lvl` on reaching `PLAYER_COLOR` (only slots 0-7).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelPlayer {
    /// `PLAYER_COLOR` = player slot.
    pub slot: u8,
    /// `PLAYER_NAME` without quotation marks.
    pub name: String,
    /// Nation 0-5 (`PLAYER_PLAYER` via dropdown list 0).
    pub nation: i32,
    /// `PLAYER_TYPE` (0 human, 1 AI, 2 passive); control kind in the game = value + 1.
    pub kind: i32,
    /// Starting resources by resource index (0 gold, 1 wood, 2 food) in whole units, as they are set
    /// one after another: first the defaults of the level `[RESOURCES]`, then the player's own
    /// block.
    pub resources: Vec<(usize, i32)>,
}

/// What `PlayerSetup_ParseLvl` reads from the `.lvl`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelSetup {
    /// Level `[RESOURCES]` (`GameWorld+0x3C4`): `CONFIG_GUID` -> value (0 wood, 1 food, 2 gold,
    /// 40 season/time of day). `None` where the game stores a pointer value: the value goes through
    /// the dropdown list with the `CONFIG_GUID` as list number, and for 0 (nations) and 1
    /// ("enabled") a number never matches. In the shipped data every player sets all three resources itself.
    pub level_resources: BTreeMap<i32, Option<i32>>,
    /// Configuration sets per slot 0-8 (`[NEUTRAL]` = 8).
    pub slot_configs: BTreeMap<u8, ConfigSets>,
    /// Players in file order.
    pub players: Vec<LevelPlayer>,
}

/// Resource index of the `[RESOURCES]` entries 0/1/2 (wood, food, gold).
fn resource_index(cfg: i32) -> Option<usize> {
    match cfg {
        0 => Some(1),
        1 => Some(2),
        2 => Some(0),
        _ => None,
    }
}

/// `.lvl` like `PlayerSetup_ParseLvl` (`0x00486640`) including `[NEUTRAL]`, level and
/// player `[RESOURCES]`, `[CONFIGDATA]`/`[UPGRADES]`/`[PATTERNS]` per player.
pub fn parse_level_setup(
    text: &str,
    defaults: &ConfigSets,
    dropdowns: &Dropdowns,
) -> Result<LevelSetup, FormatError> {
    let mut reader = LineReader::new(text);
    let mut setup = LevelSetup::default();
    let (mut name, mut nation, mut kind) = (String::new(), 0i32, 0i32);
    while let Some(raw) = reader.read() {
        let line = trim_left(&raw).to_string();
        if line.contains("[PLAYER]") {
            reader.read();
        } else if line.contains("[NEUTRAL]") {
            reader.read();
            let set = setup.slot_configs.entry(8).or_default();
            parse_config_sections(
                &mut reader,
                set,
                SectionKind::Objects,
                false,
                Some(defaults),
                dropdowns,
            )?;
        } else if line.contains("[RESOURCES]") {
            parse_level_resources(&mut reader, &mut setup.level_resources, dropdowns);
        } else if line.contains("PLAYER_NAME") {
            name = without_last(mid(&line, 13)).to_string();
        } else if line.contains("PLAYER_PLAYER") {
            let text = without_last(mid(&line, 15));
            nation = dropdowns.lookup(0, text).ok_or_else(|| {
                FormatError::Invalid(format!("nation \"{text}\" is not in dropdown.ini"))
            })?;
        } else if line.contains("PLAYER_TYPE") {
            kind = atol(mid(&line, 12)) as i32;
        } else if line.contains("PLAYER_COLOR") {
            let color = atol(mid(&line, 13));
            if color < 0 {
                return Err(FormatError::Invalid(format!(
                    "PLAYER_COLOR {color} invalid (the game crashes here)"
                )));
            }
            if color < 8 {
                let slot = color as u8;
                let mut resources: Vec<(usize, i32)> = (0..3)
                    .filter_map(|cfg| {
                        let v = (*setup.level_resources.get(&cfg)?)?;
                        Some((resource_index(cfg)?, v))
                    })
                    .collect();
                parse_player_resources(&mut reader, &mut resources);
                let set = setup.slot_configs.entry(slot).or_default();
                for kind in SectionKind::ALL {
                    parse_config_sections(
                        &mut reader,
                        set,
                        kind,
                        false,
                        Some(defaults),
                        dropdowns,
                    )?;
                }
                setup.players.push(LevelPlayer {
                    slot,
                    name: name.clone(),
                    nation,
                    kind,
                    resources,
                });
            }
        }
    }
    Ok(setup)
}

/// `PlayerSetup_ParseLevelResources` (`0x00482EB0`): `CONFIG_VALUE` via the dropdown list,
/// `CONFIG_ENABLE` irrelevant.
fn parse_level_resources(
    reader: &mut LineReader,
    target: &mut BTreeMap<i32, Option<i32>>,
    dropdowns: &Dropdowns,
) {
    let (mut state, mut cfg) = (0u8, 0i32);
    while let Some(raw) = reader.read() {
        let line = trim_left(&raw);
        if line.contains('{') {
            state = 1;
            continue;
        }
        if line.contains('}') {
            if state <= 1 {
                return;
            }
            state = 1;
            continue;
        }
        if state == 0 {
            continue;
        }
        if line.contains("CONFIG_GUID") {
            if let Some(c) = scan_int(mid(line, 12)) {
                cfg = c;
            }
            reader.read();
            state = 2;
            continue;
        }
        if line.contains("CONFIG_VALUE") {
            let text = without_last(mid(line, 14));
            target.insert(cfg, dropdowns.lookup(cfg, text));
        }
    }
}

/// `PlayerSetup_ParseResources` (`0x004831D0`): `CONFIG_VALUE` via `sscanf("\"%d\"")` from column
/// 13; a failure applies the previous value (initially 0) again.
fn parse_player_resources(reader: &mut LineReader, resources: &mut Vec<(usize, i32)>) {
    let (mut state, mut cfg, mut value) = (0u8, 0i32, 0i32);
    while let Some(raw) = reader.read() {
        let line = trim_left(&raw);
        if line.contains('{') {
            state = 1;
            continue;
        }
        if line.contains('}') {
            if state <= 1 {
                return;
            }
            state = 1;
            continue;
        }
        if state == 0 {
            continue;
        }
        if line.contains("CONFIG_GUID") {
            if let Some(c) = scan_int(mid(line, 12)) {
                cfg = c;
            }
            reader.read();
            state = 2;
            continue;
        }
        if line.contains("CONFIG_VALUE") {
            if let Some(v) = mid(line, 13).strip_prefix('"').and_then(scan_int) {
                value = v;
            }
            if let Some(index) = resource_index(cfg) {
                resources.push((index, value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DROPDOWN: &str = "#Comment\r\nVALUEGUID 0 Nations\r\nDROPDOWN 0 WID\r\nDROPDOWN 2 SPA\r\n\r\nVALUEGUID 18 Sight\r\nDROPDOWN 16 Very low\r\nDROPDOWN 32 Medium\r\nDROPDOWN 33 MEDIUM\r\nVALUEGUID 40 lost\r\n";

    #[test]
    fn dropdowns_lower_case_and_end_at_the_first_other_line() {
        let d = parse_dropdowns(DROPDOWN);
        assert_eq!(d.lists[&0], [(0, "wid".into()), (2, "spa".into())]);
        // "MEDIUM" is lowercased when loading; the first match wins.
        assert_eq!(d.lookup(18, "Very Low"), Some(16));
        assert_eq!(d.lookup(18, "medium"), Some(32));
        // The line after a list is consumed: VALUEGUID 40 is missing.
        assert!(!d.lists.contains_key(&40));
        assert_eq!(d.lookup(0, "PAT"), None);
        assert_eq!(d.lookup(13, " 25"), Some(25));
        assert_eq!(d.lookup(13, "much"), None);
        assert_eq!(lower_german("ÄÖÜ ÉA"), "äöü Éa");
    }

    const DEFAULT: &str = "PROJECT CONFIGFILE V1.4b\r\n\r\n[CONFIGDATA]\r\n{\r\n\tOBJECT_GUID 1100\r\n\t{\r\n\t\tCONFIG_GUID 14\r\n\t\t{\r\n\t\t\tCONFIG_LABEL\t\"Hit Points\"\r\n\t\t\tCONFIG_ENABLE\tTRUE\r\n\t\t\tCONFIG_VALUE\t\"120\"\r\n\t\t}\r\n\t\tCONFIG_GUID 18\r\n\t\t{\r\n\t\t\tCONFIG_ENABLE\tFALSE\r\n\t\t\tCONFIG_VALUE\t\"Medium\"\r\n\t\t}\r\n\t\tCONFIG_GUID 16\r\n\t\t{\r\n\t\t\tCONFIG_ENABLE\tTRUE\r\n\t\t\tCONFIG_VALUE\t\"16\"\r\n\t\t}\r\n\t}\r\n}\r\n[UPGRADES]\r\n{\r\n\tUPGRADE_GUID 2800\r\n\t{\r\n\t\tCONFIG_GUID 132\r\n\t\t{\r\n\t\t\tCONFIG_ENABLE\tTRUE\r\n\t\t\tCONFIG_VALUE\t\"25\"\r\n\t\t}\r\n\t}\r\n}\r\n[PATTERNS]\r\n{\r\n}\r\n";

    #[test]
    fn default_dat_takes_only_enabled_values_into_one_table() {
        let d = parse_dropdowns(DROPDOWN);
        let sets = parse_default_dat(DEFAULT, &d).unwrap();
        assert_eq!(sets[&1100], BTreeMap::from([(14, 120), (16, 16)]));
        assert_eq!(sets[&2800], BTreeMap::from([(132, 25)]));
        // An unknown dropdown text is a load error (the game would take a pointer value).
        let bad = DEFAULT.replace("\"16\"", "\"fast\"");
        let d16 = Dropdowns {
            lists: BTreeMap::from([(16, vec![(128, "slow".into())])]),
        };
        assert!(parse_default_dat(&bad, &d16).is_err());
    }

    const LVL: &str = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\nml-wiese.prj\"\r\n[RESOURCES]\r\n{\r\n\tCONFIG_GUID 0\r\n\t{\r\n\t\tCONFIG_ENABLE\tFALSE\r\n\t\tCONFIG_VALUE\t\"500\"\r\n\t}\r\n\tCONFIG_GUID 2\r\n\t{\r\n\t\tCONFIG_VALUE\t\"700\"\r\n\t}\r\n}\r\n[NEUTRAL]\r\n{\r\n[CONFIGDATA]\r\n{\r\n\tOBJECT_GUID 3409\r\n\t{\r\n\t}\r\n}\r\n}\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME\t\"29081 RED\"\r\n\tPLAYER_PLAYER\t\"SPA\"\r\n\tPLAYER_TYPE\t1\r\n\tPLAYER_COLOR\t3\r\n\tPLAYER_LINETOOL 0 128 128 0\r\n\t[RESOURCES]\r\n\t{\r\n\t\tCONFIG_GUID 1\r\n\t\t{\r\n\t\t\tCONFIG_VALUE\t\"250\"\r\n\t\t}\r\n\t}\r\n\t[CONFIGDATA]\r\n\t{\r\n\t\tOBJECT_GUID 1100\r\n\t\t{\r\n\t\t\tCONFIG_GUID 14\r\n\t\t\t{\r\n\t\t\t\tCONFIG_ENABLE\tFALSE\r\n\t\t\t\tCONFIG_VALUE\t\"200\"\r\n\t\t\t}\r\n\t\t}\r\n\t}\r\n\t[UPGRADES]\r\n\t{\r\n\t}\r\n\t[PATTERNS]\r\n\t{\r\n\t}\r\n}\r\n";

    #[test]
    fn level_setup_reads_players_resources_and_copies_defaults() {
        let d = parse_dropdowns(DROPDOWN);
        let defaults = parse_default_dat(DEFAULT, &d).unwrap();
        let setup = parse_level_setup(LVL, &defaults, &d).unwrap();
        // CONFIG_ENABLE is irrelevant for the level resources; wood (0) goes through the nation list
        // and does not match (a pointer value in the game), gold (2) has no list.
        assert_eq!(
            setup.level_resources,
            BTreeMap::from([(0, None), (2, Some(700))])
        );
        assert_eq!(setup.players.len(), 1);
        let p = &setup.players[0];
        assert_eq!((p.slot, p.nation, p.kind), (3, 2, 1));
        assert_eq!(p.name, "29081 RED");
        // Default gold 700 (wood undetermined), then the player's own block (food 250).
        assert_eq!(p.resources, [(0, 700), (2, 250)]);
        // .lvl: all values (including FALSE), the entry is a copy from default.dat.
        assert_eq!(
            setup.slot_configs[&3][&1100],
            BTreeMap::from([(14, 200), (16, 16)])
        );
        // Without a default and without CONFIG_GUID no entry is created.
        assert!(!setup.slot_configs[&8].contains_key(&3409));
    }

    #[test]
    fn lines_drop_carriage_returns_and_columns_count_from_the_start() {
        assert_eq!(game_lines("a\r\nb\rc\nlast"), ["a", "bc", "last"]);
        assert_eq!(game_lines("x\n"), ["x"]);
        assert_eq!(mid("CONFIG_VALUE\t\"7\"", 14), "7\"");
        assert_eq!(mid("ab", 5), "");
        assert_eq!(left("abc", 9), "abc");
        assert_eq!(scan_int("  -12x"), Some(-12));
        assert_eq!(scan_int("x"), None);
    }
}
