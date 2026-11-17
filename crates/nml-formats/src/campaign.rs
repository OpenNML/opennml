//! `data/gifcon/campaign.ini`: the entries of the four campaigns (re/campaign-ui 1).
//!
//! The parser of the original is `FUN_0040D200` (`0x0040D200`). It reads the file line by line and keeps a working
//! entry {type, name, text, `STOP`, path}, which it appends to the current campaign at `HEADLINE`, `LEVELPATH` and
//! `VIDEOPATH`. The reimplementation adopts the quirks.

use crate::cscan::{atol, scan_int};

/// Kind of a campaign entry (`+0` of the original entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Type 0: a level.
    Level,
    /// Type 1: a video.
    Video,
    /// Type 2: a headline.
    Headline,
}

/// An entry of a campaign (0x14 bytes in the original).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignEntry {
    pub kind: EntryKind,
    /// Text ID of the list display (`+4`).
    pub name_id: i32,
    /// Second text field (`+8`): `LEVELTEXT`, `AUTOSAVENAME` and `VIDEOTEXT` write here.
    pub text_id: i32,
    /// `STOP` (`+0xC`): no automatic continuation of play.
    pub stop: bool,
    /// Path of the level file or the video (`+0x10`), empty for headlines.
    pub path: String,
}

/// The four campaigns in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignTable {
    pub campaigns: [Vec<CampaignEntry>; 4],
}

/// The working entry of the parser (the stack variables of `FUN_0040D200`): it persists between the lines; only
/// the path lines clear the path after appending.
struct Working {
    kind: EntryKind,
    name_id: i32,
    text_id: i32,
    stop: bool,
}

/// Reads `campaign.ini` (`FUN_0040D200`).
///
/// The parser skips lines that are not a known statement. The statements apply to the beginning of the line
/// including the space (`HEADLINE `, `LEVELNAME `, ...); `STOP` applies only to the whole line. Quirks of the
/// original:
///
/// - `HEADLINE` appends immediately (type 2, name = text = ID, `STOP` reset, path empty).
/// - `LEVELNAME` begins a level (type 0, `STOP` reset), `VIDEONAME` a video (type 1) -
///   without resetting `STOP`.
/// - `LEVELTEXT`, `AUTOSAVENAME` and `VIDEOTEXT` write the same text field; if they come after the
///   path line, they do not take effect until the next appended entry.
/// - `LEVELPATH` sets the type to 0, `VIDEOPATH` to 1 and appends the working entry.
/// - `CAMPAIGN n` without range check: for n outside 0 to 3 the original writes past the end of its list, the
///   reimplementation discards the entries of such a campaign.
pub fn parse_campaign_ini(text: &str) -> CampaignTable {
    let mut table = CampaignTable::default();
    let mut campaign: Option<usize> = Some(0);
    let mut work = Working {
        kind: EntryKind::Level,
        name_id: 0,
        text_id: 0,
        stop: false,
    };
    let append =
        |table: &mut CampaignTable, campaign: Option<usize>, work: &Working, path: &str| {
            if let Some(list) = campaign.and_then(|c| table.campaigns.get_mut(c)) {
                list.push(CampaignEntry {
                    kind: work.kind,
                    name_id: work.name_id,
                    text_id: work.text_id,
                    stop: work.stop,
                    path: path.to_string(),
                });
            }
        };
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("HEADLINE ") {
            let id = atol(rest);
            work.kind = EntryKind::Headline;
            work.name_id = id;
            work.text_id = id;
            work.stop = false;
            append(&mut table, campaign, &work, "");
        } else if let Some(rest) = line.strip_prefix("LEVELNAME ") {
            let id = atol(rest);
            work.kind = EntryKind::Level;
            work.name_id = id;
            work.text_id = id;
            work.stop = false;
        } else if line == "STOP" {
            work.stop = true;
        } else if let Some(rest) = line.strip_prefix("LEVELTEXT ") {
            work.text_id = atol(rest);
        } else if let Some(rest) = line.strip_prefix("AUTOSAVENAME ") {
            work.text_id = atol(rest);
        } else if let Some(rest) = line.strip_prefix("VIDEONAME ") {
            let id = atol(rest);
            work.kind = EntryKind::Video;
            work.name_id = id;
            work.text_id = id;
        } else if let Some(rest) = line.strip_prefix("VIDEOTEXT ") {
            work.text_id = atol(rest);
        } else if let Some(rest) = line.strip_prefix("LEVELPATH ") {
            work.kind = EntryKind::Level;
            append(&mut table, campaign, &work, rest);
        } else if let Some(rest) = line.strip_prefix("VIDEOPATH ") {
            work.kind = EntryKind::Video;
            append(&mut table, campaign, &work, rest);
        } else if let Some(rest) = line.strip_prefix("CAMPAIGN ")
            && let Some(n) = scan_int(rest)
        {
            campaign = usize::try_from(n).ok().filter(|&c| c < 4);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, name_id: i32, text_id: i32, stop: bool, path: &str) -> CampaignEntry {
        CampaignEntry {
            kind,
            name_id,
            text_id,
            stop,
            path: path.to_string(),
        }
    }

    #[test]
    fn campaign_parser_quirks() {
        // Line ending as in the original: CR LF.
        let text = [
            "CAMPAIGN 1",
            "",
            "HEADLINE 5",
            "VIDEONAME 7",
            "VIDEOTEXT 8",
            "STOP",
            "VIDEOPATH data/videos/a.bik",
            // VIDEONAME does not reset STOP: the second video inherits it.
            "VIDEONAME 9",
            "VIDEOPATH data/videos/b.bik",
            "LEVELNAME 10",
            "LEVELTEXT 11",
            "LEVELPATH data/levels/l.lvl",
            // Changed after appending: lost for the finished entry.
            "VIDEOTEXT 99",
            // LEVELNAME resets STOP; AUTOSAVENAME shares the text field with LEVELTEXT.
            "LEVELNAME 12",
            "AUTOSAVENAME 13",
            "LEVELPATH data/levels/m.lvl",
            "CAMPAIGN 3",
            "LEVELNAME 20x",
            "LEVELPATH data/levels/n.lvl",
            "",
        ]
        .join("\r\n");
        let table = parse_campaign_ini(&text);
        assert_eq!(
            table.campaigns[1],
            vec![
                entry(EntryKind::Headline, 5, 5, false, ""),
                entry(EntryKind::Video, 7, 8, true, "data/videos/a.bik"),
                entry(EntryKind::Video, 9, 9, true, "data/videos/b.bik"),
                entry(EntryKind::Level, 10, 11, false, "data/levels/l.lvl"),
                entry(EntryKind::Level, 12, 13, false, "data/levels/m.lvl"),
            ]
        );
        // `atol`: leading digits count, the rest does not.
        assert_eq!(
            table.campaigns[3],
            vec![entry(EntryKind::Level, 20, 20, false, "data/levels/n.lvl")]
        );
        assert!(table.campaigns[0].is_empty() && table.campaigns[2].is_empty());
    }

    #[test]
    fn path_lines_set_the_entry_kind() {
        // `LEVELPATH` sets type 0, `VIDEOPATH` type 1 - independent of the name line.
        let text = "VIDEONAME 1\nLEVELPATH a.lvl\nLEVELNAME 2\nVIDEOPATH b.bik\nHEADLINE 3\nLEVELPATH c.lvl\n";
        let table = parse_campaign_ini(text);
        assert_eq!(
            table.campaigns[0],
            vec![
                entry(EntryKind::Level, 1, 1, false, "a.lvl"),
                entry(EntryKind::Video, 2, 2, false, "b.bik"),
                entry(EntryKind::Headline, 3, 3, false, ""),
                entry(EntryKind::Level, 3, 3, false, "c.lvl"),
            ]
        );
    }

    #[test]
    fn stop_needs_the_whole_line_and_unknown_lines_are_ignored() {
        let text = "LEVELNAME 1\nSTOP \nstop\nSTOPP\nUNKNOWN 5\nLEVELPATH a.lvl\n";
        let table = parse_campaign_ini(text);
        assert_eq!(
            table.campaigns[0],
            vec![entry(EntryKind::Level, 1, 1, false, "a.lvl")]
        );
    }

    #[test]
    fn invalid_campaign_numbers_drop_their_entries() {
        // The original writes past its list here; the reimplementation discards the entries.
        let text = "CAMPAIGN 4\nHEADLINE 1\nCAMPAIGN -1\nLEVELNAME 2\nLEVELPATH a.lvl\nCAMPAIGN 2\nHEADLINE 3\n";
        let table = parse_campaign_ini(text);
        assert_eq!(
            table.campaigns[2],
            vec![entry(EntryKind::Headline, 3, 3, false, "")]
        );
        assert!(table.campaigns[0].is_empty() && table.campaigns[1].is_empty());
        assert!(table.campaigns[3].is_empty());
    }
}
