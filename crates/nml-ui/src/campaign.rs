//! Campaign progress (re/singleplayer-menus part 1, sections 4-5): booking a level (`FUN_0040C2B0`), automatic
//! continue (`FUN_004BA6B0` -> `FUN_0040C0F0`) and the choice of campaign and entry (`FUN_004B9860`,
//! `FUN_004B9830`). Pure logic on [`Profiles`] and [`CampaignTable`]; writing `profiles.ini` (`FUN_004C6A90`) and the
//! menus are taken over by the callers.

use nml_formats::campaign::{CampaignEntry, CampaignTable, EntryKind};

use crate::profiles::{Profiles, has_record, record};

/// Last campaign (`FUN_0040C2B0`: `Kamp < 3` -> next campaign).
const LAST_CAMPAIGN: i32 = 3;

/// Entry `e` of campaign `c` of the table; outside the list `None` (in the original `FUN_005E86BF`, presumably an
/// MFC exception).
fn entry_at(table: &CampaignTable, c: i32, e: i32) -> Option<&CampaignEntry> {
    let list = table.campaigns.get(usize::try_from(c).ok()?)?;
    list.get(usize::try_from(e).ok()?)
}

/// Number of entries of campaign `c`. A campaign outside 0...3 (only from a corrupted `profiles.ini`) counts as
/// empty; the original reads unchecked behind its lists there (O-205).
fn entry_count(table: &CampaignTable, c: i32) -> i32 {
    usize::try_from(c)
        .ok()
        .and_then(|c| table.campaigns.get(c))
        .map_or(0, |list| i32::try_from(list.len()).unwrap_or(i32::MAX))
}

/// `FUN_004C6B10` without writing the file: `FUN_0040C2B0(won)`. The current entry is (campaign, entry) of the
/// campaign management (`current_campaign`/`current_level`), not the level played (O-160).
///
/// 1. Without a valid active profile **nothing** happens, the flag "newly unlocked" also stays.
/// 2. `FUN_0040BD40`: **every** record of the profile with these IDs: lost -> trials + 1, won -> complete. Without
///    a record (entry not unlocked) no effect (O-204).
/// 3. Flag `new_flag` = 0, always.
/// 4. Won and entry < count - 1: entry + 1 in any case; only if the new entry has no record yet: create the
///    record, flag 1, and if it is a headline: entry + 1 with a record, if needed - without a bounds check (O-203).
///    If it already has a record (replay), the flag stays 0 and there is no headline jump (O-159).
/// 5. Won and otherwise (last entry, also beyond, O-205): campaign < 3 -> campaign + 1, entry 0, nothing
///    unlocked; campaign 3 stays (O-47).
pub fn book(profiles: &mut Profiles, table: &CampaignTable, won: bool) {
    let (mut c, mut e) = (profiles.file.current_campaign, profiles.file.current_level);
    let mut new_flag = false;
    {
        let Some(profile) = profiles.current_mut() else {
            return;
        };
        for level in profile
            .levels
            .iter_mut()
            .filter(|l| l.campaign == c && l.level == e)
        {
            if won {
                level.complete = true;
            } else {
                level.trials = level.trials.wrapping_add(1);
            }
        }
        if won {
            if e < entry_count(table, c).wrapping_sub(1) {
                e += 1;
                if !has_record(profile, c, e) {
                    profile.levels.push(record(c, e));
                    new_flag = true;
                    if entry_at(table, c, e).is_some_and(|x| x.kind == EntryKind::Headline) {
                        e += 1;
                        if !has_record(profile, c, e) {
                            profile.levels.push(record(c, e));
                        }
                    }
                }
            } else if c < LAST_CAMPAIGN {
                c += 1;
                e = 0;
            }
        }
    }
    profiles.new_flag = new_flag;
    profiles.file.current_campaign = c;
    profiles.file.current_level = e;
}

/// The current entry (campaign, entry) of the table; outside the list `None`.
pub fn current_entry<'a>(
    profiles: &Profiles,
    table: &'a CampaignTable,
) -> Option<&'a CampaignEntry> {
    entry_at(
        table,
        profiles.file.current_campaign,
        profiles.file.current_level,
    )
}

/// `FUN_004BA6B0`: Does the next level start by itself? Only in campaign mode (game type of the session -1,
/// otherwise false), then `FUN_0040C0F0`: `STOP` of the current entry is 0 **and** the flag "newly unlocked" is set.
/// An entry outside the list triggers an exception in the original, in the reimplementation there is no continue.
pub fn auto_continue(profiles: &Profiles, table: &CampaignTable, campaign_mode: bool) -> bool {
    campaign_mode
        && profiles.new_flag
        && current_entry(profiles, table).is_some_and(|entry| !entry.stop)
}

/// `FUN_004B9860(c)`: campaign = `c`, entry 0 (`FUN_0040BC20`), then the highest unlocked entry of the campaign in
/// the active profile (`FUN_0040C090`, without profile 0) and its path (`FUN_004B9830`).
pub fn select_campaign(profiles: &mut Profiles, c: i32) {
    profiles.file.current_campaign = c;
    profiles.file.current_level = 0;
    profiles.file.current_level = profiles.highest_unlocked(c);
}

/// `FUN_004B9830(e)`: entry = `e` (`FUN_0040BC10`). The path of the entry (`GameApp+0x19C4`, `FUN_0040CB20`)
/// comes in the reimplementation from [`current_entry`]; the bounds check that triggers an exception when copying
/// the path in the original is omitted.
pub fn select_entry(profiles: &mut Profiles, e: i32) {
    profiles.file.current_level = e;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{name_field, record};
    use nml_formats::profiles::{parse_profiles, write_profiles};

    fn entry(kind: EntryKind, stop: bool, path: &str) -> CampaignEntry {
        CampaignEntry {
            kind,
            name_id: 1,
            text_id: 1,
            stop,
            path: path.to_string(),
        }
    }

    /// Campaign 0: 0 headline, 1 level, 2 level, 3 headline, 4 level, 5 level (STOP), 6 video.
    /// Campaign 1: headline, level. Campaign 2: level, headline (headline at the end). Campaign 3: headline,
    /// level, video (STOP, like `SID`).
    fn table() -> CampaignTable {
        use EntryKind::{Headline as H, Level as L, Video as V};
        let c0 = vec![
            entry(H, false, ""),
            entry(L, false, "l1"),
            entry(L, false, "l2"),
            entry(H, false, ""),
            entry(L, false, "l4"),
            entry(L, true, "l5"),
            entry(V, false, "v6"),
        ];
        let c1 = vec![entry(H, false, ""), entry(L, false, "k1")];
        let c2 = vec![entry(L, false, "m0"), entry(H, false, "")];
        let c3 = vec![
            entry(H, false, ""),
            entry(L, false, "s1"),
            entry(V, true, "sid"),
        ];
        CampaignTable {
            campaigns: [c0, c1, c2, c3],
        }
    }

    /// Profile "Anna" active: records for entry 0 and 1 of every campaign, campaign 0, entry 0.
    fn profiles() -> Profiles {
        let mut p = Profiles::default();
        let mut name = name_field("Anna");
        p.create(&mut name, &table()).unwrap();
        p.select(0);
        p
    }

    fn at(p: &mut Profiles, c: i32, e: i32) {
        p.file.current_campaign = c;
        p.file.current_level = e;
    }

    fn pos(p: &Profiles) -> (i32, i32) {
        (p.file.current_campaign, p.file.current_level)
    }

    fn rec(p: &Profiles, c: i32, e: i32) -> Vec<(i32, bool)> {
        p.file.profiles[0]
            .levels
            .iter()
            .filter(|l| l.campaign == c && l.level == e)
            .map(|l| (l.trials, l.complete))
            .collect()
    }

    #[test]
    fn win_unlocks_next_and_sets_new_flag() {
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 1);
        assert!(!p.is_unlocked(0, 2));
        book(&mut p, &t, true);
        assert_eq!(rec(&p, 0, 1), [(0, true)]);
        assert_eq!(pos(&p), (0, 2));
        assert!(p.is_unlocked(0, 2));
        assert_eq!(rec(&p, 0, 2), [(0, false)]);
        assert!(p.new_flag);
        assert!(auto_continue(&p, &t, true));
        assert_eq!(current_entry(&p, &t).unwrap().path, "l2");
    }

    #[test]
    fn win_skips_headline() {
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 2);
        book(&mut p, &t, true);
        // Entry 3 is a headline: both records are created, the entry stands on the level behind it.
        assert_eq!(pos(&p), (0, 4));
        assert!(p.is_unlocked(0, 3) && p.is_unlocked(0, 4));
        assert!(p.new_flag);
        assert_eq!(current_entry(&p, &t).unwrap().path, "l4");
        assert!(auto_continue(&p, &t, true));
    }

    #[test]
    fn loss_counts_trials() {
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 1);
        p.new_flag = true;
        book(&mut p, &t, false);
        book(&mut p, &t, false);
        assert_eq!(rec(&p, 0, 1), [(2, false)]);
        // Entry unchanged, flag reset, nothing unlocked.
        assert_eq!(pos(&p), (0, 1));
        assert!(!p.new_flag);
        assert!(!p.is_unlocked(0, 2));
        // A win sets "complete", the trials stay.
        at(&mut p, 0, 1);
        book(&mut p, &t, true);
        assert_eq!(rec(&p, 0, 1), [(2, true)]);
        // Several records with the same IDs all count (FUN_0040BD40 runs over the whole list).
        let mut p = profiles();
        p.file.profiles[0].levels.push(record(0, 1));
        at(&mut p, 0, 1);
        book(&mut p, &t, false);
        assert_eq!(rec(&p, 0, 1), [(1, false), (1, false)]);
    }

    #[test]
    fn replay_moves_on_without_headline_jump() {
        // O-159: The following entry (here headline 3) is already unlocked: entry + 1, flag 0, no jump.
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 2);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (0, 4));
        at(&mut p, 0, 2);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (0, 3));
        assert_eq!(current_entry(&p, &t).unwrap().kind, EntryKind::Headline);
        assert!(!p.new_flag);
        assert!(!auto_continue(&p, &t, true));
        assert_eq!(rec(&p, 0, 3).len(), 1);
        assert_eq!(rec(&p, 0, 4).len(), 1);
    }

    #[test]
    fn last_entry_moves_to_next_campaign_without_unlock() {
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 6);
        p.file.profiles[0].levels.push(record(0, 6));
        let before = p.file.profiles[0].levels.len();
        p.new_flag = true;
        book(&mut p, &t, true);
        assert_eq!(rec(&p, 0, 6), [(0, true)]);
        assert_eq!(pos(&p), (1, 0));
        assert_eq!(p.file.profiles[0].levels.len(), before);
        // O-47: the campaign end unlocks nothing, no automatic continue either.
        assert!(!p.new_flag);
        assert!(!auto_continue(&p, &t, true));
        // A defeat on the last entry stays there.
        at(&mut p, 0, 6);
        book(&mut p, &t, false);
        assert_eq!(pos(&p), (0, 6));
        assert_eq!(rec(&p, 0, 6), [(1, true)]);
    }

    #[test]
    fn campaign_3_end_stays() {
        let t = table();
        let mut p = profiles();
        p.file.profiles[0].levels.push(record(3, 2));
        at(&mut p, 3, 2);
        let before = p.file.profiles[0].levels.len();
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (3, 2));
        assert!(!p.new_flag);
        assert_eq!(p.file.profiles[0].levels.len(), before);
        assert_eq!(rec(&p, 3, 2), [(0, true)]);
        assert!(!auto_continue(&p, &t, true));
    }

    #[test]
    fn stop_blocks_auto_continue_only_on_new_entries() {
        // O-47: `STOP` of the entry that would come next, and only for a newly unlocked entry.
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 4);
        p.file.profiles[0].levels.push(record(0, 4));
        book(&mut p, &t, true);
        // Entry 5 (STOP) newly unlocked: flag 1, but no automatic continue.
        assert_eq!(pos(&p), (0, 5));
        assert!(p.new_flag);
        assert!(!auto_continue(&p, &t, true));
        // Without STOP and with the flag: continue; without campaign mode (free play, FUN_004BA6B0): never.
        at(&mut p, 0, 4);
        assert!(auto_continue(&p, &t, true));
        assert!(!auto_continue(&p, &t, false));
        // Without the flag (replay) not even without STOP.
        p.new_flag = false;
        assert!(!auto_continue(&p, &t, true));
        // Entry outside the list: an exception in the original, no continue in the reimplementation.
        p.new_flag = true;
        at(&mut p, 0, 99);
        assert!(!auto_continue(&p, &t, true));
        at(&mut p, 9, 0);
        assert!(!auto_continue(&p, &t, true));
        at(&mut p, 0, -1);
        assert!(!auto_continue(&p, &t, true));
    }

    #[test]
    fn no_profile_books_nothing() {
        let t = table();
        let mut p = Profiles::default();
        at(&mut p, 0, 1);
        p.new_flag = true;
        let before = p.clone();
        book(&mut p, &t, true);
        book(&mut p, &t, false);
        // The flag also stays: FUN_0040C2B0 does nothing at all without a valid profile.
        assert_eq!(p, before);
        // Invalid index of the active profile likewise.
        let mut p = profiles();
        p.file.current = 5;
        at(&mut p, 0, 1);
        p.new_flag = true;
        let before = p.clone();
        book(&mut p, &t, true);
        assert_eq!(p, before);
    }

    #[test]
    fn booking_edges_follow_the_original() {
        let t = table();
        // Headline as the chosen entry: booked like any entry (the record counts, the kind does not), the following
        // entry is unlocked (record 1): entry 1, flag 0.
        let mut p = profiles();
        at(&mut p, 0, 0);
        book(&mut p, &t, true);
        assert_eq!(rec(&p, 0, 0), [(0, true)]);
        assert_eq!(pos(&p), (0, 1));
        assert!(!p.new_flag);
        // Record missing (entry not unlocked, O-162/O-204): nothing counted, the win advances anyway and creates the
        // following record.
        let mut p = profiles();
        at(&mut p, 0, 4);
        book(&mut p, &t, true);
        assert!(rec(&p, 0, 4).is_empty());
        assert_eq!(pos(&p), (0, 5));
        assert_eq!(rec(&p, 0, 5), [(0, false)]);
        assert!(p.new_flag);
        // A defeat without a record changes only the flag.
        let mut p = profiles();
        at(&mut p, 0, 4);
        p.new_flag = true;
        let before = p.file.clone();
        book(&mut p, &t, false);
        assert_eq!(p.file, before);
        assert!(!p.new_flag);
        // Headline at the end of the list (O-203): The jump has no bound, the record is created behind the list, the
        // entry then stands outside (original: later accesses trigger an exception).
        let mut p = profiles();
        p.file.profiles[0]
            .levels
            .retain(|l| !(l.campaign == 2 && l.level == 1));
        at(&mut p, 2, 0);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (2, 2));
        assert!(rec(&p, 2, 1).len() == 1 && rec(&p, 2, 2).len() == 1);
        assert!(p.new_flag);
        assert!(current_entry(&p, &t).is_none());
        assert!(!auto_continue(&p, &t, true));
        // Entry behind the end of the list (O-205) counts as last: next campaign.
        let mut p = profiles();
        at(&mut p, 0, 40);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (1, 0));
        // Campaign outside 0...3 (file): no crash, the entry counts as the last of an empty list.
        let mut p = profiles();
        at(&mut p, 7, 0);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (7, 0));
        at(&mut p, -1, 0);
        book(&mut p, &t, true);
        assert_eq!(pos(&p), (0, 0));
    }

    #[test]
    fn new_flag_is_not_saved() {
        let t = table();
        let mut p = profiles();
        at(&mut p, 0, 1);
        book(&mut p, &t, true);
        assert!(p.new_flag);
        let text = write_profiles(&p.file);
        let read = Profiles::new(parse_profiles(&text).unwrap());
        assert!(!read.new_flag);
        assert_eq!(read.file, p.file);
        // After reading no automatic continue (as after a program start).
        assert!(!auto_continue(&read, &t, true));
    }

    #[test]
    fn select_campaign_takes_highest_unlocked_entry() {
        let mut p = profiles();
        p.file.profiles[0].levels.push(record(2, 5));
        at(&mut p, 1, 1);
        select_campaign(&mut p, 2);
        assert_eq!(pos(&p), (2, 5));
        select_campaign(&mut p, 0);
        assert_eq!(pos(&p), (0, 1));
        // Without a profile entry 0.
        let mut p = Profiles::default();
        at(&mut p, 1, 1);
        select_campaign(&mut p, 3);
        assert_eq!(pos(&p), (3, 0));
        select_entry(&mut p, 2);
        assert_eq!(pos(&p), (3, 2));
        assert_eq!(current_entry(&p, &table()).unwrap().path, "sid");
    }
}
