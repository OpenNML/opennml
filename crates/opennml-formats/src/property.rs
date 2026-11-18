//! Object types `data/gifcon/property.ini` and rider translations `data/gifcon/translate.ini`,
//! see re/object-classes. `property.ini` is read like `GameWorld_LoadPropertyIni`
//! (`0x00489240`): line by line, substring search for `GUID`, `NAME`, `PROPID`, `VALUEID`.

use std::collections::BTreeMap;

/// Path of the type file in the game.
pub const PROPERTY_PATH: &str = "data/gifcon/property.ini";
/// Path of the rider translations in the game.
pub const TRANSLATE_PATH: &str = "data/gifcon/translate.ini";

/// A `PROPID` entry: module (behavior class) with its values.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDesc {
    /// Module index `n` (`GameObject+0x64 + 4n`).
    pub index: u32,
    /// `OR(1 << argument)` of the further numbers; `0xFFFFFFFF` without arguments.
    pub mask: u32,
    /// `VALUEID`s of the module (number `(n + 1) * 10000 + k`) -> text from the fourth field.
    pub values: BTreeMap<u32, String>,
}

impl ModuleDesc {
    /// Value as an integer like `ValueSet_GetInt` (`0x005702C0`, `atol`); `None` if it is missing.
    pub fn value_int(&self, id: u32) -> Option<i64> {
        self.values.get(&id).map(|v| atol(v))
    }
}

/// An `[OBJECT]` of the type file.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectType {
    pub guid: u32,
    /// `NAME` (only stored).
    pub name: String,
    /// `VALUEID`s before the first `PROPID` (type values, `ObjectType+0x28`); 100 = category bits.
    pub values: BTreeMap<u32, String>,
    /// Modules in file order (an index can occur several times).
    pub modules: Vec<ModuleDesc>,
}

impl ObjectType {
    /// Type value as an integer (`ObjectType_GetValueInt` `0x0054CF00`); `None` if it is missing.
    pub fn value_int(&self, id: u32) -> Option<i64> {
        self.values.get(&id).map(|v| atol(v))
    }

    /// Category bits like `GameObject_InitFromType` (`0x00471060`): `VALUEID 100 & 0xFFFFFF00`.
    pub fn category(&self) -> u32 {
        self.value_int(100).unwrap_or(0) as u32 & 0xFFFF_FF00
    }

    /// First descriptor of the module `index` (`ObjectType_FindModuleDesc` `0x0054CF30` with
    /// mask -1).
    pub fn module(&self, index: u32) -> Option<&ModuleDesc> {
        self.modules.iter().find(|m| m.index == index)
    }

    /// `PROPID index` present (`GameObject_HasModule`).
    pub fn has_module(&self, index: u32) -> bool {
        self.module(index).is_some()
    }
}

/// C `isspace` in the standard locale: only ASCII 9-13 and 32. A U+00A0 decoded from byte `0xA0`
/// and other Unicode spaces do not count (unlike `str::trim`).
pub fn is_c_space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ')
}

/// `atol`: leading C whitespace ([`is_c_space`]), optional sign, digits; otherwise 0.
pub fn atol(text: &str) -> i64 {
    let text = text.trim_start_matches(is_c_space);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let value = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |acc, d| {
            acc.wrapping_mul(10).wrapping_add(i64::from(d - b'0'))
        });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// Reads all types; a later GUID replaces an earlier one (`CMap::SetAt`). Lines without a
/// current type are ignored.
pub fn parse_property_ini(text: &str) -> BTreeMap<u32, ObjectType> {
    let mut types: BTreeMap<u32, ObjectType> = BTreeMap::new();
    let mut current: Option<ObjectType> = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if line.contains("GUID") {
            // `sscanf("%s %d")`.
            let guid = fields.nth(1).and_then(|v| v.parse::<u32>().ok());
            if let Some(done) = current.take() {
                types.insert(done.guid, done);
            }
            current = guid.map(|guid| ObjectType {
                guid,
                name: String::new(),
                values: BTreeMap::new(),
                modules: Vec::new(),
            });
        } else if line.contains("NAME") {
            if let Some(object) = &mut current {
                object.name = fields.skip(1).collect::<Vec<_>>().join(" ");
            }
        } else if line.contains("PROPID") {
            let numbers: Vec<u32> = fields.skip(1).filter_map(|v| v.parse().ok()).collect();
            if let (Some(object), Some(&index)) = (&mut current, numbers.first()) {
                let mask = if numbers.len() > 1 {
                    numbers[1..]
                        .iter()
                        .fold(0u32, |m, &a| m | 1u32.checked_shl(a).unwrap_or(0))
                } else {
                    0xFFFF_FFFF
                };
                object.modules.push(ModuleDesc {
                    index,
                    mask,
                    values: BTreeMap::new(),
                });
            }
        } else if line.contains("VALUEID") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let (Some(object), Some(id)) =
                (&mut current, parts.get(1).and_then(|v| v.parse().ok()))
            else {
                continue;
            };
            let value = parts.get(4..).map_or(String::new(), |v| v.join(" "));
            match object.modules.last_mut() {
                Some(module) => module.values.insert(id, value),
                None => object.values.insert(id, value),
            };
        }
    }
    if let Some(done) = current {
        types.insert(done.guid, done);
    }
    types
}

/// Reads `translate.ini` like `GameWorld_LoadTranslateIni` (`0x004875D0`) and
/// `GameWorld_CreateObject` (`0x00485F70`): lines with `#` as the **first** character and lines without
/// `=` are ignored; keys and parts with `atol`, only the first `+` counts (`K=A+B+C`
/// produces A and B), an empty value does not translate.
pub fn parse_translate(text: &str) -> BTreeMap<u32, Vec<u32>> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let mut parts = vec![atol(value) as u32];
        if let Some((_, second)) = value.split_once('+') {
            parts.push(atol(second) as u32);
        }
        map.insert(atol(key) as u32, parts);
    }
    map
}

/// Maximum depth of the recursive translation (protection against cycles in broken files).
const MAX_TRANSLATE_DEPTH: usize = 8;

/// GUIDs that `GameWorld_CreateObject` (`0x00485F70`) produces for `guid`: translated recursively
/// (`GameWorld_FindGuidTranslation` `0x004856E0`), otherwise `guid` itself.
pub fn expand_guid(translations: &BTreeMap<u32, Vec<u32>>, guid: u32) -> Vec<u32> {
    fn walk(map: &BTreeMap<u32, Vec<u32>>, guid: u32, depth: usize, out: &mut Vec<u32>) {
        match map.get(&guid) {
            Some(parts) if depth < MAX_TRANSLATE_DEPTH => {
                for &part in parts {
                    walk(map, part, depth + 1, out);
                }
            }
            _ => out.push(guid),
        }
    }
    let mut out = Vec::new();
    walk(translations, guid, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[OBJECT]\r\nGUID 1000\r\nNAME SPA_Hauptquartier\r\nVALUEID 100 6 1 33554432\r\n[PROPERTY]\r\nPROPID 3 2\r\nVALUEID 40003 6 1 800768\r\nVALUEID 40004 1 1 0\r\n[PROPERTY]\r\nPROPID 23\r\nVALUEID 240001 6 1 0\r\n[PROPERTY]\r\nPROPID 3 1\r\nVALUEID 40003 6 1 276480\r\n[OBJECT]\r\nGUID 3500\r\nNAME Goldvorkommen\r\nVALUEID 100 6 1 1280\r\n[PROPERTY]\r\nPROPID 6\r\nVALUEID 70002 6 1 1024\r\nVALUEID 300001 1 1 1108 1109 1117\r\nVALUEID 450002 1 1 \r\n";

    #[test]
    fn types_have_category_modules_and_values() {
        let types = parse_property_ini(SAMPLE);
        assert_eq!(types.len(), 2);
        let hq = &types[&1000];
        assert_eq!(hq.name, "SPA_Hauptquartier");
        assert_eq!(hq.category(), 0x200_0000);
        assert_eq!(
            hq.modules.iter().map(|m| m.index).collect::<Vec<_>>(),
            [3, 23, 3]
        );
        // The first descriptor counts (mask 1 << 2), `0xFFFFFFFF` without arguments.
        let building = hq.module(3).unwrap();
        assert_eq!(building.mask, 4);
        assert_eq!(building.value_int(40003), Some(800_768));
        assert_eq!(hq.module(23).unwrap().mask, 0xFFFF_FFFF);
        assert!(hq.has_module(23) && !hq.has_module(0));
        let gold = &types[&3500];
        // 1280 = 0x500: the low byte is dropped.
        assert_eq!(gold.category(), 0x500);
        let resource = gold.module(6).unwrap();
        assert_eq!(resource.value_int(70002), Some(0x400));
        // Several numbers: `atol` reads the first; an empty value yields 0; missing `None`.
        assert_eq!(resource.value_int(300_001), Some(1108));
        assert_eq!(resource.value_int(450_002), Some(0));
        assert_eq!(resource.value_int(1), None);
    }

    // QUIRK E-178: kept
    #[test]
    fn atol_reads_like_the_c_library() {
        assert_eq!(atol("  -42abc"), -42);
        assert_eq!(atol("+7"), 7);
        assert_eq!(atol("x1"), 0);
        assert_eq!(atol(""), 0);
        // Only ASCII whitespace (9-13, 32) is skipped, not U+00A0 (byte 0xA0).
        assert_eq!(atol("\t\u{b}\u{c}\r\n 5"), 5);
        assert_eq!(atol("\u{a0}5"), 0);
        // Limit value and overflow as in C (two's complement), without a panic in the debug build.
        assert_eq!(atol("-9223372036854775808"), i64::MIN);
        assert_eq!(atol("9223372036854775807"), i64::MAX);
        assert_eq!(
            atol("-99999999999999999999"),
            atol("99999999999999999999").wrapping_neg()
        );
    }

    #[test]
    fn later_guid_replaces_and_lines_without_type_are_skipped() {
        let text = "VALUEID 100 6 1 5\nPROPID 3\nGUID 7\nNAME A\nGUID 7\nNAME B\nGUID x\nNAME C\n";
        let types = parse_property_ini(text);
        assert_eq!(types.len(), 1);
        assert_eq!(types[&7].name, "B");
        assert!(types[&7].values.is_empty());
    }

    #[test]
    fn translations_expand_recursively() {
        let text = "# SPA_Kolonist auf Pferd\r\n1101=1100+3409\r\n\r\n 9 = 1101 + 5 + 6\r\nbroken\r\n8=\r\n # 7=1\r\n";
        let map = parse_translate(text);
        // `atol` skips spaces; only the first `+`; " # 7=1" is not a comment (key 0).
        assert_eq!(map.len(), 3);
        assert_eq!(map[&1101], [1100, 3409]);
        assert_eq!(map[&9], [1101, 5]);
        assert_eq!(map[&0], [1]);
        assert_eq!(expand_guid(&map, 1101), [1100, 3409]);
        assert_eq!(expand_guid(&map, 9), [1100, 3409, 5]);
        assert_eq!(expand_guid(&map, 3409), [3409]);
        assert_eq!(expand_guid(&map, 8), [8]);
        // A cycle ends at the depth limit.
        let cycle = parse_translate("1=2\n2=1\n");
        assert_eq!(expand_guid(&cycle, 1).len(), 1);
    }
}
