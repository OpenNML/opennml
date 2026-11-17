//! Profile management of the campaign management (`GameApp+0x08`, vtable `0x00615E0C`;
//! re/singleplayer-menus part 1, section 2, re/campaign-ui 2). The content of `profiles.ini` lies in
//! [`ProfilesFile`]; reading and writing the file is done by the host (`UiEffect::WriteProfiles`), `nml-ui` writes
//! no files.
//!
//! Deviation: deleting a profile before the active one adjusts the index (O-121); a corrupted file is already
//! rejected by `nml_formats::profiles::parse_profiles` (O-126), so the index of the active profile is always -1 or
//! valid here.

use nml_formats::campaign::CampaignTable;
use nml_formats::profiles::{LevelRecord, ProfileRecord, ProfilesFile};
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;

/// Path of the campaign table (`FUN_0040E080`, re/campaign-ui 1).
pub const CAMPAIGN_PATH: &str = "data/gifcon/campaign.ini";

/// Default name of a profile (`L"Jon Doe"`, constructor `FUN_0040C9F0`); also the name without an active profile
/// (`FUN_0040C060`).
pub const DEFAULT_NAME: &str = "Jon Doe";

/// Prefix that unlocks all entries on creation (`FUN_004C8430`, O-27).
const RD_PREFIX: &str = "[RD]";

/// Number of campaigns (four entry lists `KV+0x08 + c*0x14`).
const CAMPAIGNS: i32 = 4;

/// The profiles with the flag "newly unlocked" (`KV+0x04`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profiles {
    pub file: ProfilesFile,
    pub new_flag: bool,
}

/// Length of a name up to the null terminator (`wcslen`).
fn name_len(name: &[u16]) -> usize {
    name.iter().position(|&u| u == 0).unwrap_or(name.len())
}

/// Name up to the null terminator.
pub fn name_units(name: &[u16]) -> &[u16] {
    &name[..name_len(name)]
}

/// Name as text (up to the null terminator).
pub fn name_text(name: &[u16]) -> String {
    String::from_utf16_lossy(name_units(name))
}

/// Field `wchar_t[40]` from a text, rest 0 (at most 39 characters and the terminator).
pub fn name_field(text: &str) -> [u16; 40] {
    let mut field = [0u16; 40];
    for (slot, unit) in field.iter_mut().take(39).zip(text.encode_utf16()) {
        *slot = unit;
    }
    field
}

impl Profiles {
    pub fn new(file: ProfilesFile) -> Profiles {
        Profiles {
            file,
            new_flag: false,
        }
    }

    /// The active profile (`m_CurProfileID` in range).
    fn current(&self) -> Option<&ProfileRecord> {
        usize::try_from(self.file.current)
            .ok()
            .and_then(|i| self.file.profiles.get(i))
    }

    /// The active profile for modification (for booking, `crate::campaign::book`).
    pub(crate) fn current_mut(&mut self) -> Option<&mut ProfileRecord> {
        usize::try_from(self.file.current)
            .ok()
            .and_then(|i| self.file.profiles.get_mut(i))
    }

    /// Profile variables (`m_VarMap`) of the active profile in file order; empty without a profile.
    pub fn current_vars(&self) -> &[(String, i32)] {
        self.current().map_or(&[], |p| p.vars.as_slice())
    }

    /// Script action 382 (`FUN_0040DAA0` -> `FUN_0040DA80`): set the profile variable `name` of the active profile to
    /// `value`; nothing without an active profile. The name is lowercased the German way (`CString_MakeLowerGerman`),
    /// but the search in `m_VarMap` compares case-sensitively (`FUN_0040C1B0`: `_mbscmp`): an entry with uppercase
    /// letters (only by hand in `profiles.ini`) stays, the lowercase one is added (E-252).
    pub fn set_current_var(&mut self, name: &str, value: i32) {
        let name = nml_formats::config::lower_german(name);
        let Some(profile) = self.current_mut() else {
            return;
        };
        match profile.vars.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = value,
            None => profile.vars.push((name, value)),
        }
    }

    /// Name of the active profile as a field, "Jon Doe" without a profile (`FUN_0040C060`).
    pub fn current_name_units(&self) -> [u16; 40] {
        self.current()
            .map_or_else(|| name_field(DEFAULT_NAME), |p| p.name)
    }

    /// Name of the active profile, "Jon Doe" without a profile (`FUN_0040C060`).
    pub fn current_name(&self) -> String {
        name_text(&self.current_name_units())
    }

    /// GUID of the active profile (`KV+0x7C`, set by `FUN_0040BFE0`).
    pub fn current_guid(&self) -> Option<i32> {
        self.current().map(|p| p.guid)
    }

    /// `FUN_0040BFE0(i)`: only for `0 ≤ i < count` does `i` become active. Copying the name to `NetName` and writing
    /// the file is left to the handler (`FUN_004C83F0`).
    pub fn select(&mut self, i: i32) {
        if usize::try_from(i).is_ok_and(|i| i < self.file.profiles.len()) {
            self.file.current = i;
        }
    }

    /// `FUN_004C8430(&Sel)`: a prefix `[RD]` is dropped from `net_name` (`wcscpy(NetName, NetName + 4)`), even if
    /// the creation fails afterwards (O-27); a name that already exists (`FUN_0040BDF0`, `_wcscmp`: case counts) is a
    /// failure (message 15748 in the handler, O-123). Otherwise `FUN_0040CB60` creates the profile (name, GUID =
    /// `m_NextProfileGUID++`, per campaign the records of entries 0 and 1, no upper limit, O-125), with `[RD]`
    /// `FUN_0040C380` unlocks all entries of all campaigns; then `FUN_004B9860(0)`: campaign 0 and the highest
    /// unlocked entry of campaign 0 **of the active** profile (O-124). Returns the new index; the handler writes the
    /// file (in the original before `FUN_004B9860`, see re/singleplayer-menus 2.5).
    ///
    /// Empty name: the handler checks it beforehand (return -1 without a message).
    #[allow(clippy::result_unit_err)]
    pub fn create(
        &mut self,
        net_name: &mut [u16; 40],
        campaigns: &CampaignTable,
    ) -> Result<usize, ()> {
        let prefix: Vec<u16> = RD_PREFIX.encode_utf16().collect();
        let rd = net_name.starts_with(&prefix);
        if rd {
            // `wcscpy(EngineConfig+0x13C, EngineConfig+0x144)`: forward by four characters, rest 0.
            net_name.copy_within(prefix.len().., 0);
            let len = net_name.len();
            net_name[len - prefix.len()..].fill(0);
        }
        let name = name_units(net_name);
        if self
            .file
            .profiles
            .iter()
            .any(|p| name_units(&p.name) == name)
        {
            return Err(());
        }
        // FUN_0040CB60: name via `wcscpy` (the rest of the field is 0 in the reimplementation), GUID, per campaign
        // entry 0 and 1.
        let mut field = [0u16; 40];
        field[..name.len()].copy_from_slice(name);
        let guid = self.file.next_guid;
        self.file.next_guid = guid.wrapping_add(1);
        let levels = (0..CAMPAIGNS)
            .flat_map(|c| [record(c, 0), record(c, 1)])
            .collect();
        self.file.profiles.push(ProfileRecord {
            name: field,
            guid,
            levels,
            vars: Vec::new(),
        });
        let index = self.file.profiles.len() - 1;
        if rd {
            unlock_all(&mut self.file.profiles[index], campaigns);
        }
        // FUN_004B9860(0): campaign 0, entry = FUN_0040C090() of the active profile.
        crate::campaign::select_campaign(self, 0);
        Ok(index)
    }

    /// `FUN_004C8550(i)` without deleting the savegames (effect `DeleteProfileSaves` of command `0x29`): GUID of
    /// profile `i` (`FUN_0040BE50`), remove the profile (`FUN_0040C130`); if it was the active one, the one before
    /// becomes active (if there is none, the first one, or none at all). Deviation (O-121): if `i` is before the
    /// active one, the index follows so that the active profile stays (the original does not adjust it). Invalid
    /// `i`: nothing, `None`.
    pub fn delete(&mut self, i: i32) -> Option<i32> {
        let index = usize::try_from(i)
            .ok()
            .filter(|&n| n < self.file.profiles.len())?;
        let guid = self.file.profiles.remove(index).guid;
        // FUN_0040C130: only the active one itself moves back; the reimplementation also for one before it (deviation).
        if i <= self.file.current {
            self.file.current -= 1;
        }
        if self.file.current < 0 {
            self.file.current = if self.file.profiles.is_empty() { -1 } else { 0 };
        }
        Some(guid)
    }

    /// `FUN_0040BDB0`: does the active profile have a record for entry `e` of campaign `c`? No without a profile.
    pub fn is_unlocked(&self, c: i32, e: i32) -> bool {
        self.current().is_some_and(|p| has_record(p, c, e))
    }

    /// `FUN_0040C090` -> `FUN_0040BD80(c)`: highest `m_LevelID` of the records of campaign `c` in the active profile,
    /// at least 0; 0 without a profile.
    pub fn highest_unlocked(&self, c: i32) -> i32 {
        self.current().map_or(0, |p| {
            p.levels
                .iter()
                .filter(|l| l.campaign == c)
                .fold(0, |best, l| best.max(l.level))
        })
    }

    /// `FUN_0040BED0`: indices of the entries of campaign `c` in file order up to before the first locked one; all
    /// without a profile (re/singleplayer-menus part 1, E14). Campaign outside 0 ... 3: none.
    pub fn unlocked_list(&self, c: i32, table: &CampaignTable) -> Vec<usize> {
        let Some(entries) = usize::try_from(c).ok().and_then(|c| table.campaigns.get(c)) else {
            return Vec::new();
        };
        let profile = self.current();
        (0..entries.len())
            .take_while(|&e| {
                profile.is_none_or(|p| has_record(p, c, i32::try_from(e).unwrap_or(i32::MAX)))
            })
            .collect()
    }
}

/// `campaign.ini` from the VFS (cp1252, `FUN_0040E080` -> `FUN_0040D200`); unreadable: empty table, warning.
pub fn load_campaigns(vfs: &Vfs) -> CampaignTable {
    match vfs.read(CAMPAIGN_PATH) {
        Ok(bytes) => nml_formats::campaign::parse_campaign_ini(&decode_cp1252(&bytes)),
        Err(e) => {
            log::warn!("Campaign table \"{CAMPAIGN_PATH}\" not readable: {e}; no campaigns");
            CampaignTable::default()
        }
    }
}

/// `FUN_0040BDB0`: record for entry `e` of campaign `c` exists.
pub(crate) fn has_record(profile: &ProfileRecord, c: i32, e: i32) -> bool {
    profile
        .levels
        .iter()
        .any(|l| l.campaign == c && l.level == e)
}

/// `FUN_0040C380`: per campaign 0 ... 3 and entry of the table a record where it is missing (appended in this order).
fn unlock_all(profile: &mut ProfileRecord, campaigns: &CampaignTable) {
    for (c, entries) in (0..CAMPAIGNS).zip(&campaigns.campaigns) {
        for e in (0..entries.len()).map(|e| i32::try_from(e).unwrap_or(i32::MAX)) {
            if !has_record(profile, c, e) {
                profile.levels.push(record(c, e));
            }
        }
    }
}

/// Level record of a newly unlocked entry (`FUN_0040C220`: `{0, campaign, entry, 0}`).
pub(crate) fn record(campaign: i32, level: i32) -> LevelRecord {
    LevelRecord {
        campaign,
        level,
        trials: 0,
        complete: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::campaign::{CampaignEntry, EntryKind};

    fn entry(kind: EntryKind) -> CampaignEntry {
        CampaignEntry {
            kind,
            name_id: 1,
            text_id: 1,
            stop: false,
            path: String::new(),
        }
    }

    /// Four campaigns with 3, 1, 4 and 0 entries.
    fn table() -> CampaignTable {
        let lens = [3, 1, 4, 0];
        CampaignTable {
            campaigns: lens.map(|n| {
                (0..n)
                    .map(|i| {
                        entry(if i == 0 {
                            EntryKind::Headline
                        } else {
                            EntryKind::Level
                        })
                    })
                    .collect()
            }),
        }
    }

    fn levels(p: &ProfileRecord) -> Vec<(i32, i32)> {
        p.levels.iter().map(|l| (l.campaign, l.level)).collect()
    }

    fn create(p: &mut Profiles, name: &str) -> (Result<usize, ()>, String) {
        let mut field = name_field(name);
        let r = p.create(&mut field, &table());
        (r, name_text(&field))
    }

    /// Script action 382 (`FUN_0040DAA0`, E-166/E-252): the name is lowercased the German way, the search is
    /// case-sensitive (`FUN_0040C1B0`: hash *33 and `_mbscmp`). A "Gold" entered by hand stays, next to it "gold" is
    /// created (two entries as in the original); without an active profile nothing happens.
    // QUIRK E-252: kept
    #[test]
    fn set_current_var_lowers_the_name_and_compares_case_sensitively() {
        let mut p = Profiles::default();
        p.set_current_var("gold", 1);
        assert!(p.current_vars().is_empty(), "nothing without a profile");
        p.file.profiles.push(ProfileRecord {
            name: name_field("Anna"),
            guid: 1,
            levels: Vec::new(),
            vars: vec![("Gold".to_string(), 3)],
        });
        p.file.current = 0;
        p.set_current_var("Gold", 5);
        assert_eq!(
            p.current_vars(),
            [("Gold".to_string(), 3), ("gold".to_string(), 5)]
        );
        p.set_current_var("GOLD", 6);
        p.set_current_var("Ärger", -2);
        assert_eq!(
            p.current_vars(),
            [
                ("Gold".to_string(), 3),
                ("gold".to_string(), 6),
                ("ärger".to_string(), -2)
            ]
        );
    }

    #[test]
    fn without_profile_the_name_is_jon_doe() {
        let p = Profiles::default();
        assert_eq!(p.current_name(), "Jon Doe");
        assert_eq!(p.current_guid(), None);
        assert!(!p.is_unlocked(0, 0));
        assert_eq!(p.highest_unlocked(0), 0);
        // Without a profile all entries (FUN_0040BED0 only checks with a profile), outside 0...3 none.
        assert_eq!(p.unlocked_list(2, &table()), [0, 1, 2, 3]);
        assert!(p.unlocked_list(4, &table()).is_empty());
        assert!(p.unlocked_list(-1, &table()).is_empty());
    }

    // QUIRK O-124: kept
    #[test]
    fn create_unlocks_entries_0_and_1_of_every_campaign() {
        let mut p = Profiles::default();
        p.file.next_guid = 5;
        p.file.current_campaign = 2;
        p.file.current_level = 3;
        assert_eq!(create(&mut p, "Anna"), (Ok(0), "Anna".to_string()));
        let rec = &p.file.profiles[0];
        assert_eq!(name_text(&rec.name), "Anna");
        assert_eq!(rec.guid, 5);
        assert_eq!(p.file.next_guid, 6);
        // FUN_0040CB60: entries 0 and 1 per campaign, even if entry 1 does not exist (campaigns 1 and 3).
        assert_eq!(
            levels(rec),
            [
                (0, 0),
                (0, 1),
                (1, 0),
                (1, 1),
                (2, 0),
                (2, 1),
                (3, 0),
                (3, 1)
            ]
        );
        assert!(rec.levels.iter().all(|l| l.trials == 0 && !l.complete));
        // Created but not active; campaign 0, entry = highest unlocked one of the active profile (none -> 0, O-124).
        assert_eq!(p.file.current, -1);
        assert_eq!((p.file.current_campaign, p.file.current_level), (0, 0));
        // Behind the terminator 0 (in the original leftovers of "Jon Doe" and unchecked memory).
        assert_eq!(rec.name, name_field("Anna"));
    }

    // QUIRK O-124: kept
    #[test]
    fn create_takes_the_entry_of_the_active_profile() {
        let mut p = Profiles::default();
        create(&mut p, "Anna").0.unwrap();
        p.select(0);
        p.file.profiles[0].levels.push(record(0, 7));
        p.file.current_campaign = 3;
        assert_eq!(create(&mut p, "Bert").0, Ok(1));
        // O-124: entry 7 of the active profile Anna, not 1 of the new Bert.
        assert_eq!((p.file.current_campaign, p.file.current_level), (0, 7));
        assert_eq!(p.file.current, 0);
        assert_eq!(p.current_name(), "Anna");
    }

    // QUIRK O-27: kept
    #[test]
    fn rd_prefix_unlocks_everything_and_is_stripped() {
        let mut p = Profiles::default();
        assert_eq!(create(&mut p, "[RD]Max"), (Ok(0), "Max".to_string()));
        let rec = &p.file.profiles[0];
        assert_eq!(name_text(&rec.name), "Max");
        // First 0/1 per campaign, then the missing ones in campaign and entry order (FUN_0040C380).
        assert_eq!(
            levels(rec),
            [
                (0, 0),
                (0, 1),
                (1, 0),
                (1, 1),
                (2, 0),
                (2, 1),
                (3, 0),
                (3, 1),
                (0, 2),
                (2, 2),
                (2, 3)
            ]
        );
        // The prefix is also dropped if the creation fails.
        assert_eq!(create(&mut p, "[RD]Max"), (Err(()), "Max".to_string()));
        assert_eq!(p.file.profiles.len(), 1);
        // Only at the start, and only exactly "[RD]".
        assert_eq!(create(&mut p, "x[RD]").1, "x[RD]");
        assert_eq!(create(&mut p, "[rd]y").1, "[rd]y");
    }

    // QUIRK O-123: kept
    #[test]
    fn duplicate_names_compare_exactly() {
        let mut p = Profiles::default();
        create(&mut p, "Anna").0.unwrap();
        assert_eq!(create(&mut p, "Anna").0, Err(()));
        // _wcscmp: case counts, spaces are characters (O-123).
        assert_eq!(create(&mut p, "anna").0, Ok(1));
        assert_eq!(create(&mut p, "Anna ").0, Ok(2));
        assert_eq!(create(&mut p, "   ").0, Ok(3));
        assert_eq!(p.file.next_guid, 4);
    }

    #[test]
    fn select_ignores_invalid_indices() {
        let mut p = Profiles::default();
        create(&mut p, "Anna").0.unwrap();
        create(&mut p, "Bert").0.unwrap();
        p.select(1);
        assert_eq!((p.file.current, p.current_guid()), (1, Some(1)));
        p.select(2);
        p.select(-1);
        assert_eq!(p.file.current, 1);
        assert_eq!(p.current_name(), "Bert");
    }

    fn three() -> Profiles {
        let mut p = Profiles::default();
        for name in ["Anna", "Bert", "Cleo"] {
            create(&mut p, name).0.unwrap();
        }
        p
    }

    // QUIRK O-121: deviation
    #[test]
    fn deleting_moves_the_active_index_like_the_original_or_keeps_it() {
        // The active one itself: the one before becomes active (FUN_0040C130, cur - 1).
        let mut p = three();
        p.select(2);
        assert_eq!(p.delete(2), Some(2));
        assert_eq!(p.current_name(), "Bert");
        // The first one active: afterwards the new first one.
        let mut p = three();
        p.select(0);
        assert_eq!(p.delete(0), Some(0));
        assert_eq!((p.file.current, p.current_name().as_str()), (0, "Bert"));
        // Deviation (O-121): one before the active one - the active one stays active.
        let mut p = three();
        p.select(2);
        assert_eq!(p.delete(0), Some(0));
        assert_eq!((p.file.current, p.current_name().as_str()), (1, "Cleo"));
        assert_eq!(p.current_guid(), Some(2));
        // One after it: nothing to do.
        let mut p = three();
        p.select(0);
        assert_eq!(p.delete(1), Some(1));
        assert_eq!(p.current_name(), "Anna");
        // Without an active profile: afterwards the first one; the last one deleted: none.
        let mut p = three();
        assert_eq!(p.delete(1), Some(1));
        assert_eq!(p.file.current, 0);
        let mut p = Profiles::default();
        create(&mut p, "Anna").0.unwrap();
        p.select(0);
        assert_eq!(p.delete(0), Some(0));
        assert_eq!((p.file.current, p.current_guid()), (-1, None));
        // Invalid: nothing.
        let mut p = three();
        assert_eq!(p.delete(3), None);
        assert_eq!(p.delete(-1), None);
        assert_eq!(p.file.profiles.len(), 3);
        // The GUIDs are never reused.
        assert_eq!(create(&mut p, "Dora").0, Ok(3));
        assert_eq!(p.file.profiles[3].guid, 3);
    }

    #[test]
    fn unlock_queries_follow_the_active_profile() {
        let mut p = Profiles::default();
        create(&mut p, "Anna").0.unwrap();
        p.select(0);
        assert!(p.is_unlocked(0, 1) && !p.is_unlocked(0, 2));
        assert_eq!(p.highest_unlocked(0), 1);
        assert_eq!(p.highest_unlocked(5), 0);
        // Up to before the first locked one: campaign 2 has 4 entries, 0 and 1 unlocked, then a gap before 3.
        p.file.profiles[0].levels.push(record(2, 3));
        assert_eq!(p.unlocked_list(2, &table()), [0, 1]);
        assert_eq!(p.highest_unlocked(2), 3);
        // Campaign 1 has only one entry (the record for entry 1 points to nothing).
        assert_eq!(p.unlocked_list(1, &table()), [0]);
        assert_eq!(p.unlocked_list(3, &table()), Vec::<usize>::new());
    }
}
