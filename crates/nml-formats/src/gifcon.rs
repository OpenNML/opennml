//! Control files from `data/gifcon/` that the simulation needs: start objects
//! (`defaultobjects.ini`), variant rules (`upgrades.ini`), buttons and prerequisites
//! (`toolbar.ini`), GUID groups (`guidgroups.ini`), hero list (`heroes.ini`) and
//! resource presets (`resourcenconfig.ini`) as well as the mouse pointers of the UI
//! (`mousepointer.ini`), see `docs/formats/gifcon-misc.md`,
//! `docs/formats/gifcon-upgrades.md` and `docs/formats/resourcenconfig.md`. Line by line like the
//! readers of the game (`DefaultObjects_Load` `0x0043E4B0`, `VariantBitTable_ParseUpgradesIni`
//! `0x0056F9B0`, `GameWorld_LoadToolbarIni` `0x00484620`, `GameWorld_LoadGuidGroupsIni`
//! `0x004850B0`, `GameWorld_LoadHeroesIni` `0x00485400`, `GameEngine_LoadResourceConfigIni`
//! `0x00438270`, mouse pointers `FUN_004CD790` from `FUN_004CDC40`).

use std::collections::BTreeMap;

use crate::FormatError;
use crate::config::{Dropdowns, game_lines, left, lower_german, mid, scan_int, trim_left};
use crate::property::atol;

/// Path of the start objects.
pub const DEFAULT_OBJECTS_PATH: &str = "data/gifcon/defaultobjects.ini";
/// Path of the variant rules (`Upgrades.ini` in the code).
pub const UPGRADES_PATH: &str = "data/gifcon/upgrades.ini";
/// Path of the buttons.
pub const TOOLBAR_PATH: &str = "data/gifcon/toolbar.ini";
/// Path of the GUID groups.
pub const GUID_GROUPS_PATH: &str = "data/gifcon/guidgroups.ini";
/// Path of the hero list.
pub const HEROES_PATH: &str = "data/gifcon/heroes.ini";
/// Path of the resource presets.
pub const RESOURCE_CONFIG_PATH: &str = "data/gifcon/resourcenconfig.ini";
/// Mouse pointers (`0x0061F700`, read by `FUN_004CD790`).
pub const MOUSE_POINTER_PATH: &str = "data/gifcon/mousepointer.ini";
/// Pointer 7 "Normal": over buttons, bars and in the menu frames (re/game-ui 2.11, re/menu-system 12.9).
pub const POINTER_NORMAL: i32 = 7;

/// `PLACEOBJECT guid dx dz` (tiles relative to the start point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceObject {
    pub guid: u32,
    pub dx: i32,
    pub dz: i32,
}

/// Start objects per (game type, nation) and supply per (list, nation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefaultObjects {
    pub start: BTreeMap<(i32, i32), Vec<PlaceObject>>,
    pub supply: BTreeMap<(i32, i32), Vec<PlaceObject>>,
    /// The file did not end in an open `GAMETYPE`/`NATION` block and was not
    /// aborted (return value of the game).
    pub complete: bool,
}

/// `defaultobjects.ini` like `DefaultObjects_Load`: header lines by prefix (`SUPPLY `, `GAMETYPE `,
/// `NATION `, `PLACEOBJECT `), end lines as a whole line; `PLACEOBJECT` outside a
/// `NATION` block aborts (rest unread). An unknown nation is a load error (the game
/// would write through a pointer value into foreign memory).
pub fn parse_default_objects(
    text: &str,
    dropdowns: &Dropdowns,
) -> Result<DefaultObjects, FormatError> {
    let mut out = DefaultObjects::default();
    let (mut in_type, mut in_nation, mut in_supply) = (false, false, false);
    let (mut game_type, mut nation, mut supply) = (0i32, 0i32, 0i32);
    let (mut guid, mut dx, mut dz) = (0i32, 0i32, 0i32);
    for raw in game_lines(text) {
        let line = trim_left(&raw);
        if left(line, 7) == "SUPPLY " {
            supply = atol(mid(line, 7)) as i32;
            in_supply = true;
        } else if line == "/SUPPLY" {
            in_supply = false;
        } else if left(line, 9) == "GAMETYPE " {
            game_type = atol(mid(line, 9)) as i32;
            in_type = true;
        } else if line == "/GAMETYPE" {
            in_type = false;
        } else if left(line, 7) == "NATION " {
            let name = mid(line, 7);
            nation = dropdowns.lookup(0, name).ok_or_else(|| {
                FormatError::Invalid(format!("defaultobjects.ini: nation \"{name}\" unknown"))
            })?;
            in_nation = true;
        } else if line == "/NATION" {
            in_nation = false;
        } else if left(line, 12) == "PLACEOBJECT " {
            if !in_nation {
                return Ok(out);
            }
            // `sscanf("%d %d %d")`: what is not read keeps the old value.
            let mut fields = mid(line, 12).split_whitespace();
            for slot in [&mut guid, &mut dx, &mut dz] {
                match fields.next().and_then(crate::config::scan_int) {
                    Some(v) => *slot = v,
                    None => break,
                }
            }
            let entry = PlaceObject {
                guid: guid as u32,
                dx,
                dz,
            };
            if in_type {
                out.start
                    .entry((game_type, nation))
                    .or_default()
                    .push(entry);
            }
            if in_supply {
                out.supply.entry((supply, nation)).or_default().push(entry);
            }
        }
    }
    out.complete = !(in_type || in_nation);
    Ok(out)
}

/// A line `BIT n = terms...` of the variant rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantRule {
    pub bit: i32,
    pub terms: Vec<i32>,
}

/// `upgrades.ini` like `VariantBitTable_ParseUpgradesIni`: `#` only in column 0, lowercase the line,
/// prefix `bit`, bit number up to `=` (negative -> skipped), values separated by space
/// or tab (`atol`, non-number -> 0); without values no entry. Order and
/// duplicates are preserved.
pub fn parse_variant_rules(text: &str) -> Vec<VariantRule> {
    let mut rules = Vec::new();
    for raw in game_lines(text) {
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let line = lower_german(&raw);
        if left(&line, 3) != "bit" {
            continue;
        }
        let rest = mid(&line, 3);
        let Some(eq) = rest.find('=') else { continue };
        let bit = atol(rest[..eq].trim()) as i32;
        if bit < 0 {
            continue;
        }
        let terms: Vec<i32> = rest[eq + 1..]
            .split([' ', '\t'])
            .filter(|t| !t.is_empty())
            .map(|t| atol(t) as i32)
            .collect();
        if !terms.is_empty() {
            rules.push(VariantRule { bit, terms });
        }
    }
    rules
}

/// Kind bits of a button (`ToolbarButton+0x08`, `Toolbar_ParseIniBlock`); the kind is
/// `flags & KIND_MASK` (the flags are ORed: `BUILD_MSG` and `ONLYONCE_MSG` result in
/// `ONLYONCE`).
pub mod button_kind {
    pub const GROUP_AND: u32 = 0x1;
    pub const DIRECT_ACCEPT: u32 = 0x2;
    pub const BUILD: u32 = 0x100_0000;
    pub const MAKE: u32 = 0x200_0000;
    pub const UPGRADE: u32 = 0x300_0000;
    pub const DO: u32 = 0x400_0000;
    pub const TARGET: u32 = 0x700_0000;
    pub const TARGET_DO: u32 = 0x800_0000;
    pub const ONLYONCE: u32 = 0xD00_0000;
    pub const SELECTHEADHUNTER: u32 = 0xF00_0000;
    pub const COUNTERUPGRADE: u32 = 0x1000_0000;
    pub const MAKEONLY_ONCE: u32 = 0x1100_0000;
    pub const SHORTCUT: u32 = 0x1500_0000;
    /// Mask of the kind.
    pub const KIND_MASK: u32 = 0xFF00_0000;
}

/// Whole-line flags of the buttons in check order (the first seven via `_mbscmp`, the
/// others via `CString_EqualsMbs`, both exact).
const BUTTON_FLAGS: [(&str, u32); 13] = [
    ("GROUP_AND_MSG", button_kind::GROUP_AND),
    ("DIRECT_ACCEPT", button_kind::DIRECT_ACCEPT),
    ("BUILD_MSG", button_kind::BUILD),
    ("MAKE_MSG", button_kind::MAKE),
    ("DO_MSG", button_kind::DO),
    ("TARGET_MSG", button_kind::TARGET),
    ("TARGET_DO_MSG", button_kind::TARGET_DO),
    ("UPGRADE_MSG", button_kind::UPGRADE),
    ("SHORTCUT_MSG", button_kind::SHORTCUT),
    ("COUNTERUPGRADE_MSG", button_kind::COUNTERUPGRADE),
    ("ONLYONCE_MSG", button_kind::ONLYONCE),
    ("MAKEONLY_ONCE_MSG", button_kind::MAKEONLY_ONCE),
    ("SELECTHEADHUNTER_MSG", button_kind::SELECTHEADHUNTER),
];

/// A button (`0x34` bytes, zeroed on creation).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolbarButton {
    /// `BUTTONID` (word 0, `%d`).
    pub id: i32,
    /// `BUTTONMSG` (word 1, `%d`): content (GUID or command number).
    pub msg: i32,
    /// Kind bits (word 2, [`button_kind`]).
    pub flags: u32,
    /// `REQUIRED_GUID` (words 3-7, `atol`, first free slot = 0; at most five, further
    /// lines have no effect).
    pub required: [i32; 5],
    /// `REQUIRED_GUID_NATION` (words 8-12, as above; unused in the shipped data).
    pub required_nation: [i32; 5],
}

impl ToolbarButton {
    /// Kind of the button (`flags & 0xFF000000`).
    pub fn kind(&self) -> u32 {
        self.flags & button_kind::KIND_MASK
    }
}

/// `toolbar.ini`: buttons per menu GUID (`GameWorld+0x1E0`) and the map `BUTTONMSG` ->
/// button (`GameWorld+0x1FC`, `GameWorld_LookupToolbarEntry` `0x0047E440`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolbar {
    /// Buttons per menu GUID in file order; a block with an already known menu GUID is
    /// discarded (the first one stays).
    pub menus: BTreeMap<i32, Vec<ToolbarButton>>,
    /// First button per `BUTTONMSG` across all accepted menus (menus in file order,
    /// within them the buttons in order).
    pub entries: BTreeMap<i32, ToolbarButton>,
}

impl Toolbar {
    /// `GameWorld_LookupToolbarEntry` (`0x0047E440`).
    pub fn lookup(&self, msg: i32) -> Option<&ToolbarButton> {
        self.entries.get(&msg)
    }
}

/// First free (0) slot of five; full -> `None` (the line has no effect).
fn first_free(slots: &mut [i32; 5]) -> Option<&mut i32> {
    slots.iter_mut().find(|v| **v == 0)
}

/// `Toolbar_ParseIniBlock` (`0x0040A5E0`): lines up to `ENDGUID` (whole line) or end of file.
/// Check order: `BUTTONID` (`Left(8)`, value `%d` from `Mid(9)`, new zeroed button),
/// `BUTTONMSG` (`Left(9)`, `Mid(10)`), the flags as a whole line, `REQUIRED_GUID_NATION`
/// (`Left(20)`, `atol` from `Mid(21)`), `REQUIRED_GUID` (`Left(13)`, `Mid(14)`), `ENDGUID`. Lines before
/// the first `BUTTONID` would be written by the original into the file object (memory error, exception A7):
/// no effect here.
fn parse_toolbar_block<'a>(lines: &mut impl Iterator<Item = &'a String>) -> Vec<ToolbarButton> {
    let mut buttons: Vec<ToolbarButton> = Vec::new();
    for line in lines {
        let line = line.as_str();
        if left(line, 8) == "BUTTONID" {
            let mut b = ToolbarButton::default();
            if let Some(v) = scan_int(mid(line, 9)) {
                b.id = v;
            }
            buttons.push(b);
            continue;
        }
        let flag = BUTTON_FLAGS
            .iter()
            .find(|(n, _)| *n == line)
            .map(|&(_, b)| b);
        let is_msg = left(line, 9) == "BUTTONMSG";
        let is_nation = left(line, 20) == "REQUIRED_GUID_NATION";
        let is_required = left(line, 13) == "REQUIRED_GUID";
        if !is_msg && flag.is_none() && !is_nation && !is_required {
            if line == "ENDGUID" {
                return buttons;
            }
            continue;
        }
        let Some(b) = buttons.last_mut() else {
            continue;
        };
        if is_msg {
            if let Some(v) = scan_int(mid(line, 10)) {
                b.msg = v;
            }
        } else if let Some(bit) = flag {
            b.flags |= bit;
        } else if is_nation {
            if let Some(slot) = first_free(&mut b.required_nation) {
                *slot = atol(mid(line, 21)) as i32;
            }
        } else if let Some(slot) = first_free(&mut b.required) {
            *slot = atol(mid(line, 14)) as i32;
        }
    }
    buttons
}

/// `toolbar.ini` like `GameWorld_LoadToolbarIni` (`0x00484620`): `Left(4) == "GUID"` begins a
/// menu (GUID via `%d` from `Mid(5)`, failure keeps the previous one), then the block reader reads up to
/// `ENDGUID`. If the menu GUID is already known, the new block is discarded (also for the map);
/// otherwise every button whose `BUTTONMSG` is still missing goes into the map (first mention
/// wins).
pub fn parse_toolbar(text: &str) -> Toolbar {
    let lines = game_lines(text);
    let mut out = Toolbar::default();
    let mut guid = 0i32;
    let mut iter = lines.iter();
    while let Some(line) = iter.next() {
        if left(line, 4) != "GUID" {
            continue;
        }
        if let Some(v) = scan_int(mid(line, 5)) {
            guid = v;
        }
        let buttons = parse_toolbar_block(&mut iter);
        if out.menus.contains_key(&guid) {
            continue;
        }
        for b in &buttons {
            out.entries.entry(b.msg).or_insert_with(|| b.clone());
        }
        out.menus.insert(guid, buttons);
    }
    out
}

/// `guidgroups.ini` like `GameWorld_LoadGuidGroupsIni` (`0x004850B0`): `GROUP ` (`Left(6)`, `atol`
/// from `Mid(6)`) sets the group and clears the list, `/GROUP` (whole line) stores a non-empty
/// list (a later identical group replaces it), `GUID ` (`Left(5)`, `atol` from `Mid(5)`)
/// appends. Quirk: the list is only cleared at `GROUP `; `GUID` lines after `/GROUP`
/// would go into a further store of the same group.
pub fn parse_guid_groups(text: &str) -> BTreeMap<u32, Vec<u32>> {
    let mut groups = BTreeMap::new();
    let mut group = 0u32;
    let mut list: Vec<u32> = Vec::new();
    for line in game_lines(text) {
        if left(&line, 6) == "GROUP " {
            group = atol(mid(&line, 6)) as u32;
            list.clear();
        } else if line == "/GROUP" {
            if !list.is_empty() {
                groups.insert(group, list.clone());
            }
        } else if left(&line, 5) == "GUID " {
            list.push(atol(mid(&line, 5)) as u32);
        }
    }
    groups
}

/// `heroes.ini` like `GameWorld_LoadHeroesIni` (`0x00485400`, re/heroes-resources 1.1): lines
/// untrimmed, `HERO ` (`Left(5)`) appends `atol(Mid(5))`. A leading space makes the
/// line a comment; duplicates, values <= 0 and unknown GUIDs stay (the first index
/// applies, `GameWorld_GetHeroIndex` `0x0047BD30`).
pub fn parse_heroes(text: &str) -> Vec<u32> {
    game_lines(text)
        .iter()
        .filter(|line| left(line, 5) == "HERO ")
        .map(|line| atol(mid(line, 5)) as u32)
        .collect()
}

/// Record of the resource presets (`0x14` bytes): config number and one value per stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceRecord {
    /// Starting resources: resource index (0 gold, 1 wood, 2 food); deposits: config number.
    pub config_guid: i32,
    /// Value per stage 0-3 (uninitialized in the original until `CONFIG_VALUEi` sets it; 0 here).
    pub values: [i32; 4],
}

/// `resourcenconfig.ini`: records and their two stores. As in the original, list and
/// map refer to the same records (later `CONFIG_VALUE` lines also change those already stored).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceConfig {
    pub records: Vec<ResourceRecord>,
    /// Starting resources (`GameEngine+0x2CAC`, `/CONFIG_GUID`) in file order, index into `records`.
    pub start: Vec<usize>,
    /// Deposits per object GUID (`GameEngine+0x2C90`, `/GUID`), index into `records`.
    pub objects: BTreeMap<i32, usize>,
}

impl ResourceConfig {
    /// Starting resource records in file order.
    pub fn start_records(&self) -> impl Iterator<Item = &ResourceRecord> {
        self.start.iter().map(|&i| &self.records[i])
    }

    /// Record of a deposit type.
    pub fn object_record(&self, guid: i32) -> Option<&ResourceRecord> {
        self.objects.get(&guid).map(|&i| &self.records[i])
    }
}

/// `resourcenconfig.ini` like `GameEngine_LoadResourceConfigIni` (`0x00438270`), check order:
/// `GUID ` (`Left(5)`, `atol` from `Mid(5)`), `CONFIG_GUID ` (`Left(12)`, new record, `atol` from
/// `Mid(12)`), `CONFIG_VALUE` (`Left(12)`: index `atol(Mid(12))`, only 0-3; value `atol` from column
/// 14 - fixed columns, quirk: a two-digit index or a double space would read wrongly),
/// `/GUID` (`Left(5)`, map[GUID] = current record), `/CONFIG_GUID` (whole line, to the
/// start list). Without a preceding `CONFIG_GUID` the original would write through an uninitialized
/// pointer (exception A7): no effect here.
pub fn parse_resource_config(text: &str) -> ResourceConfig {
    let mut out = ResourceConfig::default();
    let mut guid = 0i32;
    let mut current: Option<usize> = None;
    for line in game_lines(text) {
        if left(&line, 5) == "GUID " {
            guid = atol(mid(&line, 5)) as i32;
        } else if left(&line, 12) == "CONFIG_GUID " {
            out.records.push(ResourceRecord {
                config_guid: atol(mid(&line, 12)) as i32,
                values: [0; 4],
            });
            current = Some(out.records.len() - 1);
        } else if left(&line, 12) == "CONFIG_VALUE" {
            // The original shortens the line to `Mid(12)`, reads the index and then shortens by two
            // more characters: the value starts at column 14.
            let rest = mid(&line, 12);
            let index = atol(rest);
            if let Some(c) = current
                && (0..4).contains(&index)
            {
                out.records[c].values[index as usize] = atol(mid(rest, 2)) as i32;
            }
        } else if left(&line, 5) == "/GUID" {
            if let Some(c) = current {
                out.objects.insert(guid, c);
            }
        } else if line == "/CONFIG_GUID"
            && let Some(c) = current
        {
            out.start.push(c);
        }
    }
    out
}

/// A pointer from `mousepointer.ini`: image files of the three variants (normal white, red, green) and the hotspot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MousePointer {
    /// `NORMAL` (list `+0x84 + 0x3C*n`), one entry per line (`CStringArray::Add`).
    pub normal: Vec<String>,
    /// `RED` (`+0x98 + 0x3C*n`).
    pub red: Vec<String>,
    /// `GREEN` (`+0xAC + 0x3C*n`).
    pub green: Vec<String>,
    /// `OFFSET x y` (`+0x354 + 8*n`): the pixel that lies on the mouse position.
    pub offset: [i32; 2],
}

/// `mousepointer.ini` like `FUN_004CD790` (disassembly `0x004CD890`...`0x004CDBC7`), check order per line
/// (untrimmed): `MOUSEPOINTERID ` (`Left(15)`, number `atol(Mid(15))`), `NORMAL ` (`Left(7)`), `RED ` (`Left(4)`),
/// `GREEN ` (`Left(6)`) - each `Mid` as a file name to the list of the current pointer -, `OFFSET ` (`Left(7)`,
/// `sscanf(Mid(7), "%d %d")`). End lines (`/MOUSEPOINTERID`) and comments match nothing.
///
/// Quirks: before the first `MOUSEPOINTERID` line the number is uninitialized in the original (the register holds
/// the pointer to the file name) - the reimplementation skips such lines. `sscanf` writes into stack slots that
/// live across all lines: what an `OFFSET` line does not read keeps the value of the previous one (initially 0,
/// uninitialized in the original). The number is not limited (the original would write outside its table); the
/// map accepts any number.
pub fn parse_mouse_pointers(text: &str) -> BTreeMap<i32, MousePointer> {
    let mut out: BTreeMap<i32, MousePointer> = BTreeMap::new();
    let mut id: Option<i32> = None;
    let mut offset = [0i32; 2];
    for line in game_lines(text) {
        if left(&line, 15) == "MOUSEPOINTERID " {
            id = Some(atol(mid(&line, 15)) as i32);
            continue;
        }
        let Some(n) = id else {
            continue;
        };
        let pointer = out.entry(n).or_default();
        if left(&line, 7) == "NORMAL " {
            pointer.normal.push(mid(&line, 7).to_string());
        } else if left(&line, 4) == "RED " {
            pointer.red.push(mid(&line, 4).to_string());
        } else if left(&line, 6) == "GREEN " {
            pointer.green.push(mid(&line, 6).to_string());
        } else if left(&line, 7) == "OFFSET " {
            let mut fields = mid(&line, 7).split_whitespace();
            for slot in &mut offset {
                match fields.next().and_then(crate::config::scan_int) {
                    Some(v) => *slot = v,
                    None => break,
                }
            }
            pointer.offset = offset;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_dropdowns;

    #[test]
    fn default_objects_by_game_type_and_nation() {
        let d = parse_dropdowns("VALUEGUID 0 V\nDROPDOWN 2 SPA\nDROPDOWN 0 WID\n");
        let text = "# Start\r\nGAMETYPE 0\r\nNATION SPA\r\nPLACEOBJECT 1000 0 0\r\nPLACEOBJECT 1100 5\r\n/NATION\r\nNATION WID\r\nPLACEOBJECT 1300 0 0\r\n/NATION\r\n/GAMETYPE\r\nSUPPLY 0\r\nNATION SPA\r\nPLACEOBJECT 1100 1 2\r\n/NATION\r\n/SUPPLY\r\n";
        let objects = parse_default_objects(text, &d).unwrap();
        assert!(objects.complete);
        // Second line: dz is missing and keeps the old value 0.
        assert_eq!(
            objects.start[&(0, 2)],
            [
                PlaceObject {
                    guid: 1000,
                    dx: 0,
                    dz: 0
                },
                PlaceObject {
                    guid: 1100,
                    dx: 5,
                    dz: 0
                }
            ]
        );
        assert_eq!(objects.start[&(0, 0)][0].guid, 1300);
        assert_eq!(
            objects.supply[&(0, 2)][0],
            PlaceObject {
                guid: 1100,
                dx: 1,
                dz: 2
            }
        );
        // PLACEOBJECT outside a nation aborts, an open block reports "incomplete".
        let cut =
            parse_default_objects("GAMETYPE 1\r\nPLACEOBJECT 1 0 0\r\nNATION SPA\r\n", &d).unwrap();
        assert!(cut.start.is_empty() && !cut.complete);
        let open = parse_default_objects("GAMETYPE 1\r\nNATION SPA\r\n", &d).unwrap();
        assert!(!open.complete);
        assert!(parse_default_objects("NATION XYZ\r\n", &d).is_err());
    }

    #[test]
    fn variant_rules_keep_order_duplicates_and_parse_terms() {
        let text = "# BIT 9 = 1\r\nBIT 0 = 1\r\nbit 2 =  -2813\t -2913 \r\n BIT 3 = 1\r\nBit 4 =\r\nBIT -1 = 1\r\nBIT 5 = 2807 x -486\r\nBIT 2 = 0\r\nBIT 7 2806\r\n";
        let rules = parse_variant_rules(text);
        let pairs: Vec<(i32, Vec<i32>)> = rules.into_iter().map(|r| (r.bit, r.terms)).collect();
        assert_eq!(
            pairs,
            [
                (0, vec![1]),
                (2, vec![-2813, -2913]),
                (5, vec![2807, 0, -486]),
                (2, vec![0])
            ]
        );
    }

    #[test]
    fn toolbar_menus_buttons_kinds_requirements_and_the_first_entry_per_message() {
        let text = "# Build menu\r\nGUID 110\r\nBUTTONMSG 7\r\nBUTTONID 0\r\nBUTTONMSG 1000\r\nBUILD_MSG\r\nONLYONCE_MSG\r\nREQUIRED_GUID 1001\r\nBUTTONID 1\r\nBUTTONMSG 1001\r\nBUILD_MSG \r\nREQUIRED_GUID 1000\r\nREQUIRED_GUID x\r\nREQUIRED_GUID 1002\r\nREQUIRED_GUID 3\r\nREQUIRED_GUID 4\r\nREQUIRED_GUID 5\r\nREQUIRED_GUID 6\r\nREQUIRED_GUID_NATION 9\r\nENDGUID\r\nGUID 1000\r\nBUTTONID 3\r\nBUTTONMSG 1100\r\nMAKE_MSG\r\nGROUP_AND_MSG\r\nBUTTONID 4\r\nBUTTONMSG 1000\r\nUPGRADE_MSG\r\nENDGUID\r\nGUID 110\r\nBUTTONID 0\r\nBUTTONMSG 2000\r\nENDGUID\r\nGUIDx\r\nBUTTONID 9\r\nBUTTONMSG 3000\r\nMAKEONLY_ONCE_MSG\r\n";
        let t = parse_toolbar(text);
        // Menu 110: two buttons; BUTTONMSG before the first BUTTONID has no effect.
        let menu = &t.menus[&110];
        assert_eq!(menu.len(), 2);
        assert_eq!((menu[0].id, menu[0].msg), (0, 1000));
        assert_eq!(menu[0].kind(), button_kind::ONLYONCE);
        assert_eq!(menu[0].required, [1001, 0, 0, 0, 0]);
        // "BUILD_MSG " with a space is not a flag; `atol("x")` = 0 occupies no slot, the
        // sixth entry no longer fits.
        assert_eq!(menu[1].flags, 0);
        assert_eq!(menu[1].required, [1000, 1002, 3, 4, 5]);
        assert_eq!(menu[1].required_nation, [9, 0, 0, 0, 0]);
        let hq = &t.menus[&1000];
        assert_eq!(hq[0].flags, button_kind::MAKE | button_kind::GROUP_AND);
        assert_eq!(hq[1].kind(), button_kind::UPGRADE);
        // Duplicate menu 110 discarded (also for the map); "GUIDx" begins a menu with the
        // previous GUID 110 (failure of `%d`) - also discarded. The map takes the first
        // mention per BUTTONMSG.
        assert_eq!(t.menus.len(), 2);
        assert_eq!(t.lookup(1000).unwrap().kind(), button_kind::ONLYONCE);
        assert_eq!(t.lookup(1100).unwrap().id, 3);
        assert!(t.lookup(2000).is_none() && t.lookup(3000).is_none());
    }

    #[test]
    fn guid_groups_collect_members_until_the_closing_line() {
        let text = "#\r\nGROUP 1114\r\nGUID 1113\r\n/GROUP\r\nGROUP 3411\r\nGUID 3412\r\nGUID 3413\r\n/GROUP\r\nGROUP 7\r\n/GROUP\r\nGUID 1\r\nGROUP 1114\r\nGUID 1115\r\n/GROUP \r\n/GROUP\r\nGUID 1116\r\n/GROUP\r\n";
        let g = parse_guid_groups(text);
        assert_eq!(g[&3411], [3412, 3413]);
        // Empty group 7 not stored; the second group 1114 replaces the first, and GUID lines
        // after `/GROUP` go into the next store of the same group.
        assert!(!g.contains_key(&7));
        assert_eq!(g[&1114], [1115, 1116]);
    }

    #[test]
    fn heroes_lines_are_not_trimmed() {
        // re/heroes-resources 1.1: a leading space makes the line a comment, duplicates
        // stay.
        let text = "HERO 1200
 HERO 5
HERO 1201
HERO 1200
";
        assert_eq!(parse_heroes(text), [1200, 1201, 1200]);
    }

    #[test]
    fn resource_config_reads_fixed_columns_and_shares_records() {
        let text = "#Gold\r\nCONFIG_VALUE0 99\r\nCONFIG_GUID 0\r\nCONFIG_VALUE0 2000\r\nCONFIG_VALUE1 500\r\nCONFIG_VALUE2  1500\r\nCONFIG_VALUE3 3000\r\nCONFIG_VALUE4 1\r\n/CONFIG_GUID\r\nCONFIG_GUID 1\r\nCONFIG_VALUE1 7\r\n/CONFIG_GUID\r\nCONFIG_VALUE2 8\r\nGUID 3500 \r\nCONFIG_GUID 26\r\nCONFIG_VALUE3 8000\r\n/GUID\r\n";
        let c = parse_resource_config(text);
        let start: Vec<ResourceRecord> = c.start_records().copied().collect();
        // Double space: column 14 is a space, `atol` skips it.
        assert_eq!(start[0].values, [2000, 500, 1500, 3000]);
        // The record of wood is still changed after `/CONFIG_GUID` (shared pointer).
        assert_eq!((start[1].config_guid, start[1].values), (1, [0, 7, 8, 0]));
        let gold = c.object_record(3500).unwrap();
        assert_eq!((gold.config_guid, gold.values), (26, [0, 0, 0, 8000]));
        assert!(c.object_record(3503).is_none());
    }

    #[test]
    fn mouse_pointers_follow_fun_004cd790() {
        let text = "#
# Comment
NORMAL before.tga
MOUSEPOINTERID 0
OFFSET 14 16
NORMAL data/pointer/a.tga
RED data/pointer/a-red.tga
GREEN data/pointer/a-green.tga
/MOUSEPOINTERID

MOUSEPOINTERID 7
OFFSET 2
NORMAL data/pointer/pointer-normal.tga
NORMAL second.tga
 /MOUSEPOINTERID
MOUSEPOINTERID 99
 OFFSET 5 5
NORMAL	x.tga
";
        let pointers = parse_mouse_pointers(text);
        assert_eq!(pointers.keys().copied().collect::<Vec<_>>(), [0, 7, 99]);
        assert_eq!(
            pointers[&0],
            MousePointer {
                normal: vec!["data/pointer/a.tga".into()],
                red: vec!["data/pointer/a-red.tga".into()],
                green: vec!["data/pointer/a-green.tga".into()],
                offset: [14, 16],
            }
        );
        // `OFFSET 2`: y keeps the value of the previous OFFSET line (stack slot of the `sscanf`); every NORMAL line
        // appends.
        assert_eq!(pointers[&7].offset, [2, 16]);
        assert_eq!(
            pointers[&7].normal,
            ["data/pointer/pointer-normal.tga", "second.tga"]
        );
        // Untrimmed: ` OFFSET` and `NORMAL	` do not match; numbers are not limited.
        assert_eq!(pointers[&99], MousePointer::default());
        // Lines read before the first number are lost.
        assert!(
            pointers
                .values()
                .all(|p| !p.normal.iter().any(|n| n == "before.tga"))
        );
    }
}
