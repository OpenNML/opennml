//! Messages to the player: definitions from `data/gifcon/messages.ini` (reader `0x00493BC0`,
//! query `MessageBar_LookupMessageDef` `0x00493740`) and the text table `data/txt/texte.deu`
//! (number -> text), see re/garrison-and-commands and `docs/formats/sounddatabase.md`.

use std::collections::BTreeMap;

use crate::config::{game_lines, left, mid};
use crate::property::atol;

/// Path of the message definitions.
pub const MESSAGES_PATH: &str = "data/gifcon/messages.ini";
/// Path of the text table with the message texts (UTF-16 with BOM).
pub const TEXTS_PATH: &str = "data/txt/texte.deu";

/// Bits of a definition (`uStack_8c` of the reader): which lines were set.
pub mod flags {
    /// `TEXT n`.
    pub const TEXT: u32 = 0x1;
    /// `SOUND n`.
    pub const SOUND: u32 = 0x2;
    /// `MSGTYPE name`.
    pub const TAG: u32 = 0x4;
    /// `PING` (minimap).
    pub const PING: u32 = 0x8;
    /// `SOUNDONCE n` (locked for a while afterwards).
    pub const SOUND_ONCE: u32 = 0x10;
}

/// `MSGTAG name ... /MSGTAG`: presentation kind of a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageTag {
    pub name: String,
    /// Order in the file: counter at `/MSGTAG` (`+0x7C`, reader `0x00493BC0`); the original
    /// keeps a separate message bar queue per kind under this number. A duplicate name
    /// keeps the index of the first block (deviation from the original, O-249).
    pub index: usize,
    /// `POSITION x y` (virtual screen).
    pub position: (i32, i32),
    /// `FONT name`.
    pub font: String,
    /// `COLOR AARRGGBB` (`%X`).
    pub color: u32,
}

/// A message (`MSGID n ... /MSGID` or copy with `MSGCOPY n`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageDef {
    /// [`flags`].
    pub flags: u32,
    /// Text number in [`TEXTS_PATH`].
    pub text: i32,
    /// Sound GUID (`soundguidlist.txt`) of `SOUND` or `SOUNDONCE`.
    pub sound: i32,
    /// Name of the presentation kind (`MSGTYPE`), as written.
    pub tag: String,
}

impl MessageDef {
    /// Text number if `TEXT` was set.
    pub fn text_id(&self) -> Option<i32> {
        (self.flags & flags::TEXT != 0).then_some(self.text)
    }

    /// Sound GUID if `SOUND` or `SOUNDONCE` was set.
    pub fn sound_id(&self) -> Option<i32> {
        (self.flags & (flags::SOUND | flags::SOUND_ONCE) != 0).then_some(self.sound)
    }
}

/// All messages and presentation kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Messages {
    /// Presentation kinds by name (later ones replace earlier ones).
    pub tags: BTreeMap<String, MessageTag>,
    /// Definitions by message number (later ones replace earlier ones, `CMap::SetAt`).
    pub defs: BTreeMap<i32, MessageDef>,
}

impl Messages {
    /// `MessageBar_LookupMessageDef` (`0x00493740`): unknown number -> `None` (no message).
    pub fn get(&self, id: i32) -> Option<&MessageDef> {
        self.defs.get(&id)
    }

    /// The presentation kinds in file order (by [`MessageTag::index`]).
    pub fn tags_in_order(&self) -> Vec<&MessageTag> {
        let mut tags: Vec<&MessageTag> = self.tags.values().collect();
        tags.sort_by_key(|t| t.index);
        tags
    }

    /// Presentation kind of a message (only with `MSGTYPE` and a known name).
    pub fn tag_of(&self, def: &MessageDef) -> Option<&MessageTag> {
        if def.flags & flags::TAG == 0 {
            return None;
        }
        self.tags.get(&def.tag)
    }
}

/// `messages.ini` like the reader of the game (`0x00493BC0`): keywords by prefix
/// (`CString_Left`), `/MSGTAG`, `/MSGID` and `PING` as a whole line. **Quirk:** `MSGID n`
/// sets only the number and clears the bits; text, sound and presentation kind of the previous block
/// stay. `MSGCOPY n` (also `COPYMSG n`) stores the current state under `n` - so
/// the copies after `TEXT x` outside a block inherit sound and kind of the last definition.
///
/// **Deviation (O-249):** The original assigns a new index for every `MSGTAG` and leaves
/// position and color uninitialized without `POSITION`/`COLOR`; the reimplementation takes (0, 0) and 0, and a
/// duplicate kind name replaces the values of the earlier block but keeps its index (the indices
/// stay dense: 0 ... number of names - 1).
pub fn parse_messages(text: &str) -> Messages {
    let mut out = Messages::default();
    let mut tag = MessageTag::default();
    let mut next_index = 0usize;
    let mut def = MessageDef::default();
    let mut id = 0i32;
    for line in game_lines(text) {
        let line = line.as_str();
        if left(line, 7) == "MSGTAG " {
            tag = MessageTag {
                name: mid(line, 7).to_string(),
                ..MessageTag::default()
            };
        } else if line == "/MSGTAG" {
            match out.tags.get(&tag.name) {
                Some(first) => tag.index = first.index,
                None => {
                    tag.index = next_index;
                    next_index += 1;
                }
            }
            out.tags.insert(tag.name.clone(), tag.clone());
        } else if left(line, 9) == "POSITION " {
            let mut v = mid(line, 9).split_whitespace().map(|s| atol(s) as i32);
            tag.position = (v.next().unwrap_or(0), v.next().unwrap_or(0));
        } else if left(line, 5) == "FONT " {
            tag.font = mid(line, 5).to_string();
        } else if left(line, 6) == "COLOR " {
            tag.color = scan_hex(mid(line, 6));
        } else if left(line, 6) == "MSGID " {
            id = atol(mid(line, 6)) as i32;
            def.flags = 0;
        } else if left(line, 8) == "COPYMSG " || left(line, 8) == "MSGCOPY " {
            id = atol(mid(line, 8)) as i32;
            out.defs.insert(id, def.clone());
        } else if line == "/MSGID" {
            out.defs.insert(id, def.clone());
        } else if left(line, 8) == "MSGTYPE " {
            def.tag = mid(line, 8).to_string();
            def.flags |= flags::TAG;
        } else if left(line, 5) == "TEXT " {
            def.text = atol(mid(line, 5)) as i32;
            def.flags |= flags::TEXT;
        } else if left(line, 6) == "SOUND " {
            def.sound = atol(mid(line, 6)) as i32;
            def.flags |= flags::SOUND;
        } else if left(line, 10) == "SOUNDONCE " {
            def.sound = atol(mid(line, 10)) as i32;
            def.flags |= flags::SOUND_ONCE;
        } else if line == "PING" {
            def.flags |= flags::PING;
        }
    }
    out
}

/// `sscanf("%X")`: leading whitespace, hex digits up to the first other character (without digit 0).
fn scan_hex(text: &str) -> u32 {
    text.trim_start()
        .chars()
        .map_while(|c| c.to_digit(16))
        .fold(0u32, |acc, d| acc.wrapping_mul(16).wrapping_add(d))
}

/// Text of a `.deu` table: UTF-16 (little endian) with BOM `FF FE` as in the shipped data, without BOM
/// Windows-1252.
pub fn decode_text_file(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => crate::text::decode_cp1252(bytes),
    }
}

/// Text table `number=text` (`texte.deu`): lines without `=` or without a number before it (comments
/// `#...`, empty lines) are dropped; the text is the rest of the line without the line ending. Later
/// entries with the same number replace earlier ones.
// Deviation: The reader of the game (via `data/txt/localisation.deu`, `TextDatei ...`) is not
// traced; the format follows the shipped data (`docs/formats/sounddatabase.md`, `parse_id_list`).
pub fn parse_text_table(text: &str) -> BTreeMap<i32, String> {
    crate::sound::parse_id_list(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#\r\n# Comment\r\nMSGTAG ECONOMY\r\nPOSITION 0 400\r\nFONT InfoMessages\r\nCOLOR E000FF00\r\n/MSGTAG\r\n\r\n#Too little food\r\nMSGID 0\r\nMSGTYPE ECONOMY\r\nPING\r\nSOUND 877\r\nTEXT 17000\r\n/MSGID\r\n\r\nMSGID 9\r\nMSGTYPE OTHER\r\nTEXT 17260\r\n/MSGID\r\n\r\nMSGID 1000\r\nSOUNDONCE 872\r\nMSGTYPE MILITARY\r\nPING\r\nTEXT 17111\r\n/MSGID\r\n\r\nTEXT 17121\r\nMSGCOPY 1300\r\nCOPYMSG 1301\r\n";

    #[test]
    fn messages_keep_text_sound_and_tag_and_copies_inherit_the_last_block() {
        let m = parse_messages(SAMPLE);
        let tag = &m.tags["ECONOMY"];
        assert_eq!(
            (tag.position, tag.font.as_str(), tag.color),
            ((0, 400), "InfoMessages", 0xE000_FF00)
        );
        let food = m.get(0).unwrap();
        assert_eq!(
            food.flags,
            flags::TAG | flags::PING | flags::SOUND | flags::TEXT
        );
        assert_eq!(
            (food.text_id(), food.sound_id(), food.tag.as_str()),
            (Some(17000), Some(877), "ECONOMY")
        );
        assert_eq!(m.tag_of(food), Some(tag));
        // MSGID clears only the bits: sound 877 is still there, but does not count.
        let speed = m.get(9).unwrap();
        assert_eq!((speed.text_id(), speed.sound_id()), (Some(17260), None));
        assert_eq!(speed.sound, 877);
        assert_eq!(m.tag_of(speed), None, "OTHER is not defined here");
        // The copy after TEXT outside a block: new text, sound and kind of 1000.
        let copy = m.get(1300).unwrap();
        assert_eq!(
            (copy.text_id(), copy.sound_id(), copy.tag.as_str()),
            (Some(17121), Some(872), "MILITARY")
        );
        assert_ne!(copy.flags & flags::SOUND_ONCE, 0);
        assert_eq!(m.get(1301), Some(copy));
        assert_eq!(m.get(1000).unwrap().text_id(), Some(17111));
        assert_eq!(m.get(5), None);
    }

    #[test]
    fn tags_keep_their_file_order() {
        // The names sort differently (A < B < Z) than the file (Z, A, B); the index follows the file.
        let m = parse_messages(
            "MSGTAG Z
POSITION 1 2
COLOR 11
/MSGTAG
MSGTAG A
/MSGTAG
MSGTAG B
FONT F
/MSGTAG
",
        );
        let order: Vec<(&str, usize)> = m
            .tags_in_order()
            .into_iter()
            .map(|t| (t.name.as_str(), t.index))
            .collect();
        assert_eq!(order, [("Z", 0), ("A", 1), ("B", 2)]);
        // Without POSITION/COLOR: (0, 0) and 0 (O-249).
        assert_eq!((m.tags["A"].position, m.tags["A"].color), ((0, 0), 0));
        assert_eq!((m.tags["Z"].position, m.tags["Z"].color), ((1, 2), 0x11));
    }

    #[test]
    fn a_repeated_tag_name_replaces_the_values_but_keeps_the_first_index() {
        let m = parse_messages(
            "MSGTAG X
POSITION 1 1
/MSGTAG
MSGTAG Y
/MSGTAG
MSGTAG X
POSITION 2 2
/MSGTAG
MSGTAG W
/MSGTAG
",
        );
        assert_eq!(m.tags["X"].position, (2, 2));
        let order: Vec<(&str, usize)> = m
            .tags_in_order()
            .into_iter()
            .map(|t| (t.name.as_str(), t.index))
            .collect();
        assert_eq!(order, [("X", 0), ("Y", 1), ("W", 2)]);
    }

    #[test]
    fn msgcopy_sets_the_current_number() {
        // `MSGCOPY 5` stores under 5 and makes 5 the current number: the following `/MSGID`
        // stores the state once more under 5 (not under the number of the block before).
        let m = parse_messages(
            "MSGID 1
TEXT 10
/MSGID
MSGCOPY 5
TEXT 20
/MSGID
",
        );
        assert_eq!(m.get(1).unwrap().text_id(), Some(10));
        assert_eq!(m.get(5).unwrap().text_id(), Some(20));
    }

    #[test]
    fn text_tables_are_utf16_with_bom_or_cp1252() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                "#Header\r\n17000=WE DO NOT HAVE ENOUGH FOOD!\r\n\r\n17001=Ölü\r\n"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        let table = parse_text_table(&decode_text_file(&utf16));
        assert_eq!(table.len(), 2);
        assert_eq!(table[&17000], "WE DO NOT HAVE ENOUGH FOOD!");
        assert_eq!(table[&17001], "Ölü");
        assert_eq!(decode_text_file(b"1=\xd6l"), "1=Öl");
    }

    #[test]
    fn the_economy_messages_of_the_original_resolve_to_their_texts() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let messages = parse_messages(&crate::text::decode_cp1252(
            &vfs.read(MESSAGES_PATH).unwrap(),
        ));
        let texts = parse_text_table(&decode_text_file(&vfs.read(TEXTS_PATH).unwrap()));
        let text = |id: i32| {
            let def = messages.get(id).unwrap();
            texts[&def.text_id().unwrap()].clone()
        };
        assert_eq!(text(0), "WIR HABEN NICHT GENUG NAHRUNG!");
        assert_eq!(text(1), "WIR BENÖTIGEN MEHR HOLZ!");
        assert_eq!(text(5), "EINE GOLDQUELLE IST ERSCHÖPFT!");
        assert_eq!(text(7), "EIN UPGRADE WURDE FERTIGGESTELLT!");
        assert_eq!(text(11), "WIR HABEN UNSEREN MAXIMALEN WOHNRAUM ERREICHT!");
        // Sounds and presentation kind as in re/garrison-and-commands 6.
        let sound = |id: i32| messages.get(id).unwrap().sound_id();
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 7, 8, 11].map(sound),
            [877, 877, 877, 878, 879, 879, 881, 877, 879].map(Some)
        );
        assert_eq!(sound(9), None);
        let economy = messages.tag_of(messages.get(0).unwrap()).unwrap();
        assert_eq!(
            (economy.name.as_str(), economy.color),
            ("ECONOMY", 0xE000_FF00)
        );
        // Copies: headquarters of other nations with their own text.
        assert_eq!(messages.get(1300).unwrap().text_id(), Some(17121));
        assert_eq!(
            messages.get(1300).unwrap().sound_id(),
            messages.get(1000).unwrap().sound_id()
        );
        // Counter-upgrades: 3370 as a block, 3371 ... as copies with their own text (corrects
        // re/garrison-and-commands 6, which says "only 3370 has a definition").
        assert_eq!(messages.get(3370).unwrap().sound_id(), Some(876));
        let copy = messages.get(3371).unwrap();
        assert_eq!((copy.text_id(), copy.sound_id()), (Some(17251), Some(876)));
    }

    #[test]
    fn installation_message_tags_are_economy_military_other() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        // The presentation kinds are position and color data and not language dependent; checked is the
        // German edition with patch (reference), the English one has the same file.
        let messages = parse_messages(&crate::text::decode_cp1252(
            &vfs.read(MESSAGES_PATH).unwrap(),
        ));
        let tags: Vec<_> = messages
            .tags_in_order()
            .into_iter()
            .map(|t| {
                (
                    t.index,
                    t.name.as_str(),
                    t.position,
                    t.color,
                    t.font.as_str(),
                )
            })
            .collect();
        assert_eq!(
            tags,
            [
                (0, "ECONOMY", (0, 400), 0xE000_FF00, "InfoMessages"),
                (1, "MILITARY", (0, 500), 0xE0FF_0000, "InfoMessages"),
                (2, "OTHER", (0, 600), 0xE000_00FF, "InfoMessages"),
            ]
        );
    }

    #[test]
    fn message_230_inherits_sound_925_without_the_sound_bit() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let messages = parse_messages(&crate::text::decode_cp1252(
            &vfs.read(MESSAGES_PATH).unwrap(),
        ));
        // Block 228 has sound 925 (O-215/O-216 ...).
        let flag = messages.get(228).unwrap();
        assert_eq!(
            flag.flags,
            flags::TAG | flags::PING | flags::SOUND | flags::TEXT
        );
        assert_eq!((flag.sound, flag.text), (925, 17232));
        // ... block 230 (tribute) sets `MSGID` and clears the bits, the sound field inherits 925 anyway;
        // without the SOUND bit the sound does not count (`sound_id()` = None).
        for (id, text) in [(230, 17240), (231, 17241), (232, 17242), (233, 17243)] {
            let def = messages.get(id).unwrap();
            assert_eq!(def.flags, flags::TAG | flags::TEXT, "message {id}");
            assert_eq!(
                (def.text_id(), def.sound, def.sound_id(), def.tag.as_str()),
                (Some(text), 925, None, "OTHER"),
                "message {id}"
            );
        }
    }
}
