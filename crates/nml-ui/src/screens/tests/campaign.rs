//! Tests of the campaign menus (re/singleplayer-menus part 1, 4.1-4.4 and 6.2): `SELECTCAMPAIGN`
//! (`FUN_004DA4A0`), `KAMPAGNE0...3` (`FUN_004D6DA0`), `CAMPAIGNLEVEL`/`CAMPAIGNLEVEL2` (`FUN_004DABD0`),
//! `CAMPAIGNVIDEO` (`FUN_004DA8A0`), the commands `0x0C`, `0x1C`, `0x28` and "Continue" of the accounting screen.
//! The logic alone runs through [`Logic`] with a small table, live through `MenuApp` with the corpus dialogs and the
//! real `campaign.ini` (these need `NML_GAME_DIR`).

use nml_formats::profiles::LevelRecord;

use super::*;
use crate::msg::MenuLevelEffect;
use crate::profiles::record;

const SELECT: &str = "selectcampaign";
const LEVEL: &str = "campaignlevel";
const LEVEL2: &str = "campaignlevel2";
const VIDEO: &str = "campaignvideo";

fn entry(kind: EntryKind, name_id: i32, path: &str) -> CampaignEntry {
    CampaignEntry {
        kind,
        name_id,
        text_id: name_id + 1000,
        stop: false,
        path: path.to_string(),
    }
}

/// Campaign 0: headline, level, level. Campaign 1: headline, video, level, level, headline, level.
/// Campaigns 2 and 3: headline, level. Name ID = 100 * campaign + entry + 1.
fn table() -> CampaignTable {
    use EntryKind::{Headline as H, Level as L, Video as V};
    let kinds: [&[EntryKind]; 4] = [&[H, L, L], &[H, V, L, L, H, L], &[H, L], &[H, L]];
    let mut t = CampaignTable::default();
    for (c, kinds) in kinds.iter().enumerate() {
        t.campaigns[c] = kinds
            .iter()
            .enumerate()
            .map(|(e, &kind)| {
                let path = match kind {
                    H => String::new(),
                    V => format!("data/videos/v{c}-{e}.bik"),
                    L => format!("data/levels/k{c}/l{e}.lvl"),
                };
                entry(kind, (100 * c + e + 1) as i32, &path)
            })
            .collect();
    }
    t
}

/// Logic with the profiles `names` (active `current`), the table [`table`] and the current entry (`c`, `e`).
fn logic(names: &[&str], current: i32, c: i32, e: i32) -> Logic {
    let mut l = Logic::with_profiles(names, current);
    l.data.campaigns = table();
    l.data.profiles.file.current_campaign = c;
    l.data.profiles.file.current_level = e;
    l
}

impl Logic {
    /// Current entry (campaign, entry).
    fn current(&self) -> (i32, i32) {
        let f = &self.data.profiles.file;
        (f.current_campaign, f.current_level)
    }

    fn set_current(&mut self, c: i32, e: i32) {
        self.data.profiles.file.current_campaign = c;
        self.data.profiles.file.current_level = e;
    }

    /// Adds `levels` to profile `i`.
    fn unlock(&mut self, i: usize, levels: impl IntoIterator<Item = LevelRecord>) {
        self.data.profiles.file.profiles[i].levels.extend(levels);
    }
}

/// Profile file with `names` (active `current`) and the current entry (`c`, `e`).
fn file_at(names: &[&str], current: i32, c: i32, e: i32) -> ProfilesFile {
    let mut file = profiles_with(names, current);
    file.current_campaign = c;
    file.current_level = e;
    file
}

/// Corpus `MenuApp` with these profiles and the starting difficulty `difficulty` (`CampaignDifficulty`).
fn live_campaign(file: ProfilesFile, difficulty: i32) -> Option<Driver<Corpus>> {
    let config = EngineConfig {
        campaign_difficulty: difficulty,
        ..EngineConfig::default()
    };
    let opts = MenuOptions {
        profiles: file,
        ..MenuOptions::default()
    };
    live_with(config, opts, Box::new(dispatch))
}

/// Opens `selectcampaign` and waits until the UI is idle; then discards the effects.
fn open_select(d: &mut Driver<Corpus>) {
    d.push(open(SELECT, true));
    d.idle();
    d.app.take_effects();
}

fn sorted(names: &[String]) -> Vec<String> {
    let mut v = names.to_vec();
    v.sort();
    v
}

fn names(list: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

#[test]
fn selectcampaign_opens_the_current_campaign_list() {
    // 4.1 code 5 of the frame: `0 <= k < 4` -> `{6,"kampagne<k>"}`, return value 1 without `status`; no reselecting.
    let mut l = logic(&["Anna"], 0, 2, 1);
    let (r, msg) = l.send(SELECT, SELECT, code::QUERY_STATUS);
    assert_eq!(r, 1);
    assert!(!msg.params.contains_key("status"));
    assert_eq!(l.take_queue(), [open("kampagne2", false)]);
    assert_eq!(l.current(), (2, 1), "campaign/entry stay");
    // Campaign outside 0...3: no list.
    l.set_current(4, 0);
    assert_eq!(l.send(SELECT, SELECT, code::QUERY_STATUS).0, 1);
    assert!(l.take_queue().is_empty());
    // Other codes of the frame and unknown elements: -1.
    assert_eq!(l.click(SELECT, SELECT), -1);
    assert_eq!(l.click(SELECT, "nowhere"), -1);
    assert_eq!(l.send(SELECT, "kampagne1", code::QUERY_STATUS).0, -1);

    // Live: campaign 1, entry 1 (video `SPA-1`) -> list and video page.
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 1, 1), 1) else {
        return;
    };
    open_select(&mut d);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne1", VIDEO])
    );
    // Campaign 0, entry 1 (introduction) -> `campaignlevel2`.
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 0, 1), 1) else {
        return;
    };
    open_select(&mut d);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne0", LEVEL2])
    );
}

#[test]
fn campaign_button_switches_list_and_selects_highest_entry() {
    // 4.1 `kampagne*` code 8: other campaign -> `{3,"kampagne<k>"}`, `{4, detail}` (video -> `campaignvideo`, otherwise
    // `campaignlevel(2)` - also for a headline), `{5,"kampagne<n>"}`, `FUN_004B9860(n)`.
    let mut l = logic(&["Anna"], 0, 0, 1);
    l.unlock(0, [record(1, 2), record(1, 3)]);
    assert_eq!(l.click(SELECT, "kampagne1"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("kampagne0", true),
            close(LEVEL2, false),
            open("kampagne1", true)
        ]
    );
    assert_eq!(l.current(), (1, 3), "highest free entry");
    // Same campaign: nothing.
    assert_eq!(l.click(SELECT, "kampagne1"), 1);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.current(), (1, 3));
    // From the video, the video page closes.
    l.set_current(1, 1);
    assert_eq!(l.click(SELECT, "kampagne2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("kampagne1", true),
            close(VIDEO, false),
            open("kampagne2", true)
        ]
    );
    assert_eq!(l.current(), (2, 1));
    // From a headline (campaign > 0) still `{4,"campaignlevel"}`.
    l.set_current(2, 0);
    assert_eq!(l.click(SELECT, "kampagne3"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("kampagne2", true),
            close(LEVEL, false),
            open("kampagne3", true)
        ]
    );
    // Without a valid old campaign, just open and select.
    l.set_current(-1, 0);
    assert_eq!(l.click(SELECT, "kampagne0"), 1);
    assert_eq!(l.take_queue(), [open("kampagne0", true)]);
    assert_eq!(l.current(), (0, 1));
    // Other codes: -1.
    assert_eq!(l.send(SELECT, "kampagne2", code::QUERY_STATUS).0, -1);

    // Live: from campaign 0 to 1 - list and detail change, entry = highest free one (1, video).
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 0, 1), 1) else {
        return;
    };
    open_select(&mut d);
    d.click(SELECT, "kampagne1");
    d.idle();
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne1", VIDEO])
    );
    let f = &d.app.profiles().file;
    assert_eq!((f.current_campaign, f.current_level), (1, 1));
}

// QUIRK O-161: kept
#[test]
fn list_stops_before_first_locked_entry() {
    // 4.2 `dir` code 2: entries of the current campaign (not of the dialog number) in file order, with a profile up to
    // before the first locked one; code 0: `int` = entry.
    let mut l = logic(&["Anna"], 0, 1, 1);
    let first_two = l.texts(&[101, 102]);
    assert_eq!(l.items("kampagne1", "dir"), first_two);
    l.unlock(0, [record(1, 3)]);
    assert_eq!(l.items("kampagne1", "dir"), first_two, "gap at 2");
    l.unlock(0, [record(1, 2)]);
    assert_eq!(l.items("kampagne3", "dir"), l.texts(&[101, 102, 103, 104]));
    assert_eq!(l.query("kampagne1", "dir", code::QUERY_VALUE), (1, Some(1)));
    // Without a profile, all entries (O-161).
    l.data.profiles.file.current = -1;
    assert_eq!(
        l.items("kampagne1", "dir"),
        l.texts(&[101, 102, 103, 104, 105, 106])
    );
    // Other codes and elements: 0.
    assert_eq!(l.send("kampagne1", "dir", code::FREE_ITEMS).0, 0);
    assert_eq!(l.send("kampagne1", "dir", code::DOUBLE_CLICK).0, 0);
    assert_eq!(l.click("kampagne1", "nowhere"), 0);

    // Live: new profile, campaign 1 -> headline and `SPA-1`.
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 1, 1), 1) else {
        return;
    };
    open_select(&mut d);
    let list = element(&d, "kampagne1", "dir");
    assert_eq!(list.items.len(), 2, "{:?}", list.items);
    assert_eq!(list.selection, Some(1));
}

// QUIRK O-206: deviation
#[test]
fn headline_selection_opens_no_detail() {
    // 4.2 `dir` code 4: close the detail of the old entry (`{4}`), entry = i, open the detail of the new one (`{5}`); a
    // headline opens none.
    let mut l = logic(&["Anna"], 0, 1, 2);
    l.unlock(0, (2..6).map(|e| record(1, e)));
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 4).0, 1);
    assert_eq!(l.take_queue(), [close(LEVEL, false)]);
    assert_eq!(l.current(), (1, 4));
    // From the headline to the level: no closing.
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 5).0, 1);
    assert_eq!(l.take_queue(), [open(LEVEL, true)]);
    // Level -> video, video -> level.
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 1).0, 1);
    assert_eq!(l.take_queue(), [close(LEVEL, false), open(VIDEO, true)]);
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 3).0, 1);
    assert_eq!(l.take_queue(), [close(VIDEO, false), open(LEVEL, true)]);
    // Same kind in a row: close and reopen.
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 2).0, 1);
    assert_eq!(l.take_queue(), [close(LEVEL, false), open(LEVEL, true)]);
    // The frame opens no detail on a headline.
    l.set_current(1, 4);
    assert_eq!(l.send("kampagne1", "kampagne1", code::QUERY_STATUS).0, 1);
    assert!(l.take_queue().is_empty());
    // Without `int` no effect, return value 0.
    assert_eq!(l.send("kampagne1", "dir", code::ITEM_SELECTED).0, 0);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.current(), (1, 4));
    // An entry beyond the list (O-203) acts like a headline (an exception in the original, O-206).
    assert_eq!(l.send_int("kampagne1", "dir", code::ITEM_SELECTED, 9).0, 1);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.current(), (1, 9));
    assert_eq!(l.send("kampagne1", "kampagne1", code::QUERY_STATUS).0, 1);
    assert!(l.take_queue().is_empty());
}

#[test]
fn level_entry_opens_campaignlevel_campaign_0_opens_campaignlevel2() {
    // 4.2 code 5 of the frame `kampagne<n>`: detail of the current entry by campaign (not by n), `{6}`, without a
    // lock check; other codes 0.
    let cases = [
        ((1, 2), "kampagne1", vec![open(LEVEL, false)]),
        ((0, 1), "kampagne0", vec![open(LEVEL2, false)]),
        ((1, 1), "kampagne1", vec![open(VIDEO, false)]),
        ((1, 0), "kampagne1", vec![]),
        ((0, 2), "kampagne3", vec![open(LEVEL2, false)]),
    ];
    for ((c, e), frame, want) in cases {
        let mut l = logic(&["Anna"], 0, c, e);
        assert_eq!(l.send(frame, frame, code::QUERY_STATUS).0, 1, "{c}/{e}");
        assert_eq!(l.take_queue(), want, "{c}/{e}");
        assert_eq!(l.current(), (c, e));
    }
    let mut l = logic(&["Anna"], 0, 1, 2);
    assert_eq!(l.click("kampagne1", "kampagne1"), 0);
    assert!(l.take_queue().is_empty());

    // Live: campaign 1, entry 2 (`kamp1-1`) -> `campaignlevel` with box and start button.
    let mut file = file_at(&["Anna"], 0, 1, 2);
    file.profiles[0].levels.push(record(1, 2));
    let Some(mut d) = live_campaign(file, 1) else {
        return;
    };
    open_select(&mut d);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne1", LEVEL])
    );
    assert!(element(&d, LEVEL, "levelstart").visible);
}

// QUIRK O-156: kept
#[test]
fn three_levels_only_after_a_loss_and_resets_to_easy() {
    // 4.3 code 5 of the frame: without a profile or after a defeat (`m_TrialCount > 0`) flag 1 and
    // difficulty 0, otherwise flag 0; return value -1 (O-156).
    let mut l = logic(&["Anna"], 0, 1, 2);
    l.unlock(0, [record(1, 2)]);
    l.data.campaign_difficulty = 2;
    assert_eq!(l.send(LEVEL, LEVEL, code::QUERY_STATUS).0, -1);
    assert!(!l.state.campaign_three_levels);
    assert_eq!(l.data.campaign_difficulty, 2);
    l.data.profiles.file.profiles[0]
        .levels
        .last_mut()
        .unwrap()
        .trials = 1;
    assert_eq!(l.send(LEVEL, LEVEL, code::QUERY_STATUS).0, -1);
    assert!(l.state.campaign_three_levels);
    assert_eq!(l.data.campaign_difficulty, 0, "\"Easy\" preselected");
    assert_eq!(
        l.items(LEVEL, "levellevel"),
        l.texts(&[15770, 15771, 15772])
    );
    assert_eq!(
        l.query(LEVEL, "levellevel", code::QUERY_VALUE),
        (1, Some(0))
    );
    // Code 4: index 0/1/2 -> 0/1/2, otherwise 1.
    for (index, want) in [(2, 2), (1, 1), (0, 0), (7, 1), (-1, 1)] {
        assert_eq!(
            l.send_int(LEVEL, "levellevel", code::ITEM_SELECTED, index)
                .0,
            1
        );
        assert_eq!(l.data.campaign_difficulty, want, "index {index}");
    }
    // Every opening sets "Easy" again.
    l.data.campaign_difficulty = 2;
    l.send(LEVEL, LEVEL, code::QUERY_STATUS);
    assert_eq!(l.data.campaign_difficulty, 0);
    // Without a profile likewise, also in `campaignlevel2`.
    let mut l = logic(&["Anna"], -1, 0, 1);
    l.data.campaign_difficulty = 1;
    assert_eq!(l.send(LEVEL2, LEVEL2, code::QUERY_STATUS).0, -1);
    assert!(l.state.campaign_three_levels);
    assert_eq!(l.data.campaign_difficulty, 0);
    // An invalid profile index (!= -1) counts as "no defeat" (`FUN_0040C0C0` returns 0).
    let mut l = logic(&["Anna"], 0, 0, 1);
    l.data.profiles.file.current = 5;
    l.data.campaign_difficulty = 1;
    l.send(LEVEL2, LEVEL2, code::QUERY_STATUS);
    assert!(!l.state.campaign_three_levels);
    assert_eq!(l.data.campaign_difficulty, 1);

    // Live: after a defeat the box shows three stages with "Easy".
    let mut file = file_at(&["Anna"], 0, 1, 2);
    file.profiles[0].levels.push(LevelRecord {
        trials: 2,
        ..record(1, 2)
    });
    let Some(mut d) = live_campaign(file, 2) else {
        return;
    };
    assert_eq!(
        d.app.data().campaign_difficulty,
        2,
        "starting value from Engine.ini"
    );
    open_select(&mut d);
    let combo = element(&d, LEVEL, "levellevel");
    assert_eq!(combo.items.len(), 3, "{:?}", combo.items);
    assert_eq!(combo.selection, Some(0));
    assert_eq!(d.app.data().campaign_difficulty, 0);
}

// QUIRK O-46, O-157: kept
#[test]
fn two_levels_show_normal_for_difficulty_0() {
    // 4.3 `levellevel` with flag 0: texts 15771, 15772; code 0 shows `v - 1` or 0 for v = 0 ("Normal" although
    // "Easy" is played, O-157); code 4: 0 -> 1, 1 -> 2, otherwise 1.
    let mut l = logic(&["Anna"], 0, 1, 2);
    l.unlock(0, [record(1, 2)]);
    l.data.campaign_difficulty = 0;
    l.send(LEVEL, LEVEL, code::QUERY_STATUS);
    assert!(!l.state.campaign_three_levels);
    assert_eq!(l.items(LEVEL, "levellevel"), l.texts(&[15771, 15772]));
    for (difficulty, shown) in [(0, 0), (1, 0), (2, 1)] {
        l.data.campaign_difficulty = difficulty;
        assert_eq!(
            l.query(LEVEL, "levellevel", code::QUERY_VALUE),
            (1, Some(shown)),
            "difficulty {difficulty}"
        );
    }
    assert_eq!(l.data.campaign_difficulty, 2, "code 0 changes nothing");
    for (index, want) in [(0, 1), (1, 2), (2, 1), (-3, 1)] {
        l.send_int(LEVEL, "levellevel", code::ITEM_SELECTED, index);
        assert_eq!(l.data.campaign_difficulty, want, "index {index}");
    }
    // Without `array`/`int`: return value 0; `leveltitel` code 7 returns the text of the text field (+8), never shown
    // (O-46).
    assert_eq!(l.send(LEVEL, "levellevel", code::ITEM_SELECTED).0, 0);
    let (r, msg) = l.send(LEVEL, "leveltitel", code::QUERY_TEXT);
    assert_eq!(r, 1);
    let want = l.texts(&[1103]);
    assert!(
        matches!(msg.params.get("text"), Some(Param::Text(t)) if *t == want[0]),
        "{:?}",
        msg.params
    );
    // Other elements and codes: -1.
    assert_eq!(l.click(LEVEL, "nowhere"), -1);
    assert_eq!(l.send(LEVEL, "levelstart", code::QUERY_STATUS).0, -1);

    // Live with Engine.ini difficulty 0 and without a defeat: two stages, "Normal" marked.
    let mut file = file_at(&["Anna"], 0, 1, 2);
    file.profiles[0].levels.push(record(1, 2));
    let Some(mut d) = live_campaign(file, 0) else {
        return;
    };
    open_select(&mut d);
    let combo = element(&d, LEVEL, "levellevel");
    assert_eq!(combo.items.len(), 2, "{:?}", combo.items);
    assert_eq!(combo.selection, Some(0));
    assert_eq!(d.app.data().campaign_difficulty, 0);
}

#[test]
fn levelstart_closes_three_frames_waits_and_starts() {
    // 4.3 `levelstart` code 8: `{4,"selectcampaign"}`, `{4,"kampagne<campaign>"}`, `{4, frame}`, `{0x15}`, `{0x0C}`.
    let mut l = logic(&["Anna"], 0, 1, 2);
    assert_eq!(l.click(LEVEL, "levelstart"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(SELECT, false),
            close("kampagne1", false),
            close(LEVEL, false),
            AppCmd::WaitDialogs,
            AppCmd::StartCampaignLevel
        ]
    );
    let mut l = logic(&["Anna"], 0, 0, 1);
    assert_eq!(l.click(LEVEL2, "levelstart"), 1);
    assert_eq!(
        l.take_queue()[1..3],
        [close("kampagne0", false), close(LEVEL2, false)]
    );

    // Live: introduction `kamp0-1` with difficulty 2 from Engine.ini: all menus closed, then
    // `UiEffect::StartCampaignLevel` with the entry's path; the session's game type becomes -1 (6.2 step 4).
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 0, 1), 2) else {
        return;
    };
    open_select(&mut d);
    let path = d.app.data().campaigns.campaigns[0][1].path.clone();
    assert!(path.ends_with(".lvl"), "{path}");
    assert_eq!(d.app.data().draft.settings.game_type, 0);
    d.click(LEVEL2, "levelstart");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    let starts: Vec<UiEffect> = d
        .app
        .take_effects()
        .into_iter()
        .filter(|e| matches!(e, UiEffect::StartCampaignLevel { .. }))
        .collect();
    assert_eq!(
        starts,
        [UiEffect::StartCampaignLevel {
            level: path,
            difficulty: 2
        }]
    );
    assert_eq!(d.app.data().draft.settings.game_type, -1);
}

// QUIRK O-206: deviation
#[test]
fn start_without_level_entry_reopens_selectcampaign() {
    // Reimplementation (O-206): `0x0C` with an entry outside the list (an exception when copying the path in the
    // original) starts nothing and reopens `selectcampaign`.
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 1, 40), 1) else {
        return;
    };
    d.push(AppCmd::StartCampaignLevel);
    d.idle();
    assert!(
        !d.app
            .take_effects()
            .iter()
            .any(|e| matches!(e, UiEffect::StartCampaignLevel { .. }))
    );
    assert!(d.app.menu().open_names().iter().any(|n| n == SELECT));
}

#[test]
fn cancel_closes_list_and_detail() {
    // 4.1 `cancel` code 8: `{3, frame}`, `{4,"kampagne<k>"}`, `{4, detail}` (level/video only),
    // `{5,"singleplayermenu"}`.
    let mut l = logic(&["Anna"], 0, 1, 2);
    assert_eq!(l.click(SELECT, "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(SELECT, true),
            close("kampagne1", false),
            close(LEVEL, false),
            open("singleplayermenu", true)
        ]
    );
    l.set_current(1, 1);
    l.click(SELECT, "cancel");
    assert_eq!(l.take_queue()[2], close(VIDEO, false));
    l.set_current(0, 0);
    l.click(SELECT, "cancel");
    assert_eq!(
        l.take_queue(),
        [
            close(SELECT, true),
            close("kampagne0", false),
            open("singleplayermenu", true)
        ],
        "headline: no detail"
    );
    l.set_current(7, 0);
    l.click(SELECT, "cancel");
    assert_eq!(
        l.take_queue(),
        [close(SELECT, true), open("singleplayermenu", true)]
    );
    assert_eq!(l.send(SELECT, "cancel", code::QUERY_STATUS).0, -1);

    // Live.
    let Some(mut d) = live_campaign(file_at(&["Anna"], 0, 0, 1), 1) else {
        return;
    };
    open_select(&mut d);
    d.click(SELECT, "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["singleplayermenu"]);
}

// QUIRK O-162: kept
#[test]
fn locked_level_startable_after_profile_switch() {
    // O-162: campaign/entry are global. Anna has `kamp1-3` (campaign 1, entry 4) free and selected; after switching
    // to Bert (only entries 0 and 1) the list shows two entries, the detail opens without a lock check, and
    // `LEVELSTART` starts the locked level.
    let mut file = profiles_with(&["Anna", "Bert"], 0);
    file.profiles[0].levels.extend((2..5).map(|e| record(1, e)));
    file.current_campaign = 1;
    file.current_level = 4;
    let mut profiles = Profiles::new(file);
    profiles.select(1);
    assert!(!profiles.is_unlocked(1, 4));

    // In the small table entry 4 is a headline; there the level is at entry 3.
    let mut l = Logic::new();
    l.data.profiles = profiles.clone();
    l.data.profiles.file.current_level = 3;
    l.data.campaigns = table();
    assert!(!l.data.profiles.is_unlocked(1, 3));
    assert_eq!(l.items("kampagne1", "dir").len(), 2);
    assert_eq!(l.send("kampagne1", "kampagne1", code::QUERY_STATUS).0, 1);
    assert_eq!(l.take_queue(), [open(LEVEL, false)]);

    let Some(mut d) = live_campaign(profiles.file, 1) else {
        return;
    };
    open_select(&mut d);
    assert_eq!(element(&d, "kampagne1", "dir").items.len(), 2);
    assert!(d.app.menu().open_names().iter().any(|n| n == LEVEL));
    let path = d.app.data().campaigns.campaigns[1][4].path.clone();
    d.click(LEVEL, "levelstart");
    d.idle();
    assert!(
        d.app
            .take_effects()
            .contains(&UiEffect::StartCampaignLevel {
                level: path,
                difficulty: 1
            })
    );
}

// QUIRK O-46: kept
#[test]
fn campaignvideo_shows_no_title() {
    // 4.4: `videotitel` code 7 returns the video text (text field +8), the element is in no `.dlg` (O-46);
    // `videostart` see `videostart_books_plays_and_returns_once`. Otherwise -1, also code 5.
    let mut l = logic(&["Anna"], 0, 1, 1);
    let (r, msg) = l.send(VIDEO, "videotitel", code::QUERY_TEXT);
    assert_eq!(r, 1);
    let want = l.texts(&[1102]);
    assert!(matches!(msg.params.get("text"), Some(Param::Text(t)) if *t == want[0]));
    assert_eq!(l.send(VIDEO, VIDEO, code::QUERY_STATUS).0, -1);
    assert_eq!(l.send(VIDEO, "videostart", code::QUERY_STATUS).0, -1);
    assert_eq!(l.click(VIDEO, "nowhere"), -1);
}

#[test]
fn campaign_difficulty_starts_from_engine_ini() {
    // `GameApp+0x19B4` starts with `Engine.ini CampaignDifficulty` (default 1).
    let app = MenuApp::from_library(
        crate::dialog::DialogLibrary::default(),
        EngineConfig {
            campaign_difficulty: 2,
            ..EngineConfig::default()
        },
        MenuOptions::default(),
    );
    assert_eq!(app.data().campaign_difficulty, 2);
    assert_eq!(EngineConfig::default().campaign_difficulty, 1);
}

// --- Campaign video, continuation, `0x1C`, `0x28`, `ACCOUNTING` "Continue" ---------------------------------------

/// Path of the entry (`c`, `e`) of the table.
fn path_of(d: &Driver<Corpus>, c: usize, e: usize) -> String {
    d.app.data().campaigns.campaigns[c][e].path.clone()
}

/// Campaign/entry and the "newly free" flag in the live app.
fn position(d: &Driver<Corpus>) -> (i32, i32, bool) {
    let p = d.app.profiles();
    (p.file.current_campaign, p.file.current_level, p.new_flag)
}

/// Records (trials, completed) of the active profile for (`c`, `e`).
fn records_of(d: &Driver<Corpus>, c: i32, e: i32) -> Vec<(i32, bool)> {
    let p = d.app.profiles();
    let i = usize::try_from(p.file.current).unwrap();
    p.file.profiles[i]
        .levels
        .iter()
        .filter(|l| l.campaign == c && l.level == e)
        .map(|l| (l.trials, l.complete))
        .collect()
}

/// Profile "Anna" with the records 0 ... `free` of campaign `c`, current entry (`c`, `e`), starting difficulty 1.
fn live_at(c: i32, e: i32, free: i32) -> Option<Driver<Corpus>> {
    let mut file = file_at(&["Anna"], 0, c, e);
    file.profiles[0].levels = (0..=free).map(|i| record(c, i)).collect();
    live_campaign(file, 1)
}

/// Accounting open as after `0x0E` in the campaign: game type of the session -1 (set by `0x0C`), table stored.
fn open_accounting(d: &mut Driver<Corpus>) {
    d.app.data_mut().draft.settings.game_type = -1;
    d.app
        .set_accounting(crate::screens::AccountingView::default());
    d.idle();
    d.app.take_effects();
}

/// The effects without click sounds and without the command `0x20` from `selectcampaign.dlg` (`BACKGROUND`, goes as
/// `MenuLevel(Hide)` to the host).
fn effects(d: &mut Driver<Corpus>) -> Vec<UiEffect> {
    d.app
        .take_effects()
        .into_iter()
        .filter(|e| {
            !matches!(
                e,
                UiEffect::PlaySfx { .. } | UiEffect::MenuLevel(MenuLevelEffect::Hide)
            )
        })
        .collect()
}

/// Video `path` as `{0x1D, path}` requests it.
fn play(path: &str) -> UiEffect {
    UiEffect::PlayVideo {
        path: path.into(),
        abortable: true,
    }
}

// QUIRK O-33: kept
// QUIRK O-206: deviation
#[test]
fn videostart_books_plays_and_returns_once() {
    // 4.4 `videostart` code 8: `{4,"selectcampaign"}`, `{4,"kampagne<campaign>"}`, `{4, frame}`, `{0x15}`, `{0x1C}`,
    // `{0x1D, path of the current entry}`, `{0x1A}`, `{0x28}`, `{5,"selectcampaign"}`; return value 1. Booking happens
    // only with `0x1C` (the entry stays when queuing).
    let mut l = logic(&["Anna"], 0, 1, 1);
    assert_eq!(l.click(VIDEO, "videostart"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(SELECT, false),
            close("kampagne1", false),
            close(VIDEO, false),
            AppCmd::WaitDialogs,
            AppCmd::BookCampaign,
            AppCmd::PlayVideo("data/videos/v1-1.bik".into()),
            AppCmd::WaitVideo,
            AppCmd::AfterVideo,
            open(SELECT, true),
        ]
    );
    assert_eq!(l.current(), (1, 1));
    // An entry outside the list (O-206): the path the original would take without a match.
    l.set_current(1, 40);
    l.click(VIDEO, "videostart");
    assert_eq!(
        l.take_queue()[5],
        AppCmd::PlayVideo("sorry for that!".into())
    );

    // Live, repeat of `SPA-1` (the following entry `kamp1-1` is already free): `0x1C` books the video as won
    // (entry + 1, flag 0, `profiles.ini`), afterwards no automatic continue. `0x28` empties the queue - the appended
    // `{5,"selectcampaign"}` lapses (O-33) - and opens `selectcampaign` exactly once; the list shows the following
    // entry. Menu music restarts, pointer shown, game type of the session -1.
    let Some(mut d) = live_at(1, 1, 2) else {
        return;
    };
    open_select(&mut d);
    let path = path_of(&d, 1, 1);
    d.click(VIDEO, "videostart");
    d.idle();
    let fx = effects(&mut d);
    assert_eq!(
        fx,
        [
            UiEffect::BookCampaign { won: true },
            UiEffect::WriteProfiles,
            play(&path),
            UiEffect::RestartMusic,
        ]
    );
    let open_names = d.app.menu().open_names().to_vec();
    assert_eq!(open_names.iter().filter(|n| *n == SELECT).count(), 1);
    assert_eq!(sorted(&open_names), names(&[SELECT, "kampagne1", LEVEL]));
    assert_eq!(records_of(&d, 1, 1), [(0, true)]);
    assert_eq!(position(&d), (1, 2, false));
    assert!(!d.app.pointer_hidden());
    assert_eq!(d.app.data().draft.settings.game_type, -1);
    assert!(d.app.queue().is_empty());
}

// QUIRK O-158: kept
#[test]
fn video_auto_continues_into_a_level_without_dialog() {
    // 6.1: `0x28` with automatic continue (flag "newly free" from the booking, no `STOP`) and a level as successor:
    // `FUN_004B9830(entry)`, empty the queue, `{0x0C}` - no dialog, no menu music. First viewing of
    // `SPA-1`: `kamp1-1` becomes free and starts.
    let Some(mut d) = live_at(1, 1, 1) else {
        return;
    };
    open_select(&mut d);
    let video = path_of(&d, 1, 1);
    let level = path_of(&d, 1, 2);
    d.click(VIDEO, "videostart");
    d.idle();
    assert_eq!(
        effects(&mut d),
        [
            UiEffect::BookCampaign { won: true },
            UiEffect::WriteProfiles,
            play(&video),
            UiEffect::StartCampaignLevel {
                level,
                difficulty: 1
            },
        ]
    );
    assert!(d.app.menu().open_names().is_empty(), "no dialog");
    assert_eq!(position(&d), (1, 2, true));
    assert_eq!(records_of(&d, 1, 2), [(0, false)]);
    assert!(!d.app.pointer_hidden());
    assert!(d.app.queue().is_empty());
}

// QUIRK O-33, O-158: kept
#[test]
fn video_never_auto_continues_into_a_video() {
    // O-158 (6.1 branch "true and kind != 0"): if a newly free video follows a video, `0x28` does not start it but
    // empties the queue, restarts the menu music and opens `selectcampaign` (flag 0). In the shipped data a video never
    // follows a video; for this the table makes `kamp1-1` a video.
    let Some(mut d) = live_at(1, 1, 1) else {
        return;
    };
    let mut table = d.app.data().campaigns.clone();
    table.campaigns[1][2].kind = EntryKind::Video;
    table.campaigns[1][2].path = path_of(&d, 1, 8);
    d.app.data_mut().campaigns = table;
    d.push(AppCmd::BookCampaign);
    d.push(AppCmd::AfterVideo);
    d.push(open("start", true));
    d.idle();
    assert_eq!(
        effects(&mut d),
        [
            UiEffect::BookCampaign { won: true },
            UiEffect::WriteProfiles,
            UiEffect::RestartMusic,
        ]
    );
    assert_eq!(position(&d), (1, 2, true), "newly free, but no continue");
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne1", VIDEO]),
        "`start` after `0x28` discarded"
    );
}

// QUIRK O-158: kept
#[test]
fn accounting_continue_starts_the_next_level() {
    // re/accounting 5, re/menu-flow section "Game end" (`0x004D9506`...`0x004D95AE`): `QUIT` code 8 -> `{3, frame}`,
    // `{4, flag}`; automatic continue (`FUN_004BA6B0`) -> `{0x15}`; level -> `FUN_004B9830(entry)`, `{0x0C}`.
    let mut l = logic(&["Anna"], 0, 1, 2);
    l.data.draft.settings.game_type = -1;
    l.data.profiles.new_flag = true;
    l.state.accounting_tab = "account1".into();
    assert_eq!(l.click("accounting", "quit"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("accounting", true),
            close("account1", false),
            AppCmd::WaitDialogs,
            AppCmd::StartCampaignLevel,
        ]
    );
    // Without campaign mode no continue, even with the flag (`FUN_004BA6B0`: game type != -1 -> false).
    l.data.draft.settings.game_type = 0;
    l.click("accounting", "quit");
    assert_eq!(l.take_queue()[2], open("singleplayermenu", true));

    // Live: a win in `kamp1-1` books (like `GameFlow` at game end) and unlocks `kamp1-2`; "Continue" starts
    // it without a dialog.
    let Some(mut d) = live_at(1, 2, 2) else {
        return;
    };
    d.app.book_campaign(true);
    assert_eq!(position(&d), (1, 3, true));
    open_accounting(&mut d);
    d.click("accounting", "quit");
    d.idle();
    let level = path_of(&d, 1, 3);
    assert_eq!(
        effects(&mut d),
        [UiEffect::StartCampaignLevel {
            level,
            difficulty: 1
        }]
    );
    assert!(d.app.menu().open_names().is_empty());
}

// QUIRK O-158, O-207: kept
#[test]
fn accounting_continue_plays_the_next_video() {
    // `QUIT` with automatic continue to a video (`0x004D95B3`...`0x004D9614`): `{0x15}`, `{0x1C}`, `{0x1D, path}`,
    // `{0x1A}`, `{0x28}` - without `{5,"selectcampaign"}`; so a video follows automatically only here (O-158).
    let mut l = logic(&["Anna"], 0, 1, 1);
    l.data.draft.settings.game_type = -1;
    l.data.profiles.new_flag = true;
    l.state.accounting_tab = "account2".into();
    l.click("accounting", "quit");
    assert_eq!(
        l.take_queue(),
        [
            close("accounting", true),
            close("account2", false),
            AppCmd::WaitDialogs,
            AppCmd::BookCampaign,
            AppCmd::PlayVideo("data/videos/v1-1.bik".into()),
            AppCmd::WaitVideo,
            AppCmd::AfterVideo,
        ]
    );
    // A headline (never newly free in the shipped data) takes the same branch (kind != 0) with an empty path.
    l.set_current(1, 4);
    l.click("accounting", "quit");
    assert_eq!(l.take_queue()[4], AppCmd::PlayVideo(String::new()));

    // Live: a win in `kamp1-6` unlocks `SPA-2`; "Continue" books and plays it. `SPA-2` is the last entry of campaign 1:
    // the booking jumps to campaign 2 (headline, nothing free), `0x28` opens `selectcampaign` with the list of
    // campaign 2 and without detail, the menu music restarts.
    let Some(mut d) = live_at(1, 7, 7) else {
        return;
    };
    d.app.book_campaign(true);
    assert_eq!(position(&d), (1, 8, true));
    open_accounting(&mut d);
    d.click("accounting", "quit");
    d.idle();
    let video = path_of(&d, 1, 8);
    assert_eq!(
        effects(&mut d),
        [
            UiEffect::BookCampaign { won: true },
            UiEffect::WriteProfiles,
            play(&video),
            UiEffect::RestartMusic,
        ]
    );
    assert_eq!(position(&d), (2, 0, false));
    assert_eq!(records_of(&d, 1, 8), [(0, true)]);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne2"])
    );
}

// QUIRK O-158: kept
#[test]
fn stop_entry_returns_to_selectcampaign() {
    // O-47: `STOP` on the newly free entry prevents the automatic continue. `QUIT` then opens `selectcampaign`
    // (campaign mode), likewise `0x28` after a video.
    let mut l = logic(&["Anna"], 0, 1, 2);
    l.data.campaigns.campaigns[1][2].stop = true;
    l.data.draft.settings.game_type = -1;
    l.data.profiles.new_flag = true;
    l.state.accounting_tab = "account1".into();
    l.click("accounting", "quit");
    assert_eq!(
        l.take_queue(),
        [
            close("accounting", true),
            close("account1", false),
            open(SELECT, true)
        ]
    );

    // Live, campaign 2: a win in `kamp2-5` jumps over the headline to `IND-2` (newly free, without `STOP`);
    // "Continue" plays it, the booking unlocks `kamp2-6` - that one has `STOP`: `0x28` opens `selectcampaign` with
    // the level page of `kamp2-6`, menu music restarts.
    let Some(mut d) = live_at(2, 6, 6) else {
        return;
    };
    d.app.book_campaign(true);
    assert_eq!(position(&d), (2, 8, true));
    open_accounting(&mut d);
    d.click("accounting", "quit");
    d.idle();
    let video = path_of(&d, 2, 8);
    assert_eq!(
        effects(&mut d),
        [
            UiEffect::BookCampaign { won: true },
            UiEffect::WriteProfiles,
            play(&video),
            UiEffect::RestartMusic,
        ]
    );
    assert_eq!(position(&d), (2, 9, true));
    assert!(d.app.data().campaigns.campaigns[2][9].stop);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne2", LEVEL])
    );
}

#[test]
fn book_campaign_reports_and_writes_profiles() {
    // `FUN_004C6B10(won)` = `FUN_0040C2B0(won)` + `profiles.ini` (`FUN_004C6A90`): effects `BookCampaign` (report)
    // and `WriteProfiles`, also outside a frame (from `GameFlow` at game end) in the look-back of the report.
    let Some(mut d) = live_at(1, 2, 2) else {
        return;
    };
    d.app.book_campaign(false);
    assert_eq!(records_of(&d, 1, 2), [(1, false)]);
    assert_eq!(position(&d), (1, 2, false));
    assert_eq!(
        d.app.take_effects(),
        [
            UiEffect::BookCampaign { won: false },
            UiEffect::WriteProfiles
        ]
    );
    d.frame();
    let report = d.app.report(S1280, d.now);
    assert!(
        report
            .effects
            .iter()
            .any(|(_, e)| *e == UiEffect::BookCampaign { won: false }),
        "{:?}",
        report.effects
    );
}

// QUIRK O-207: kept
#[test]
fn accounting_routes_leave_the_music_to_the_dialogs() {
    // Open point (O-207): neither `0x0E` (`FUN_004BAF40`) nor `ACCOUNTING` touch the menu music; it only follows the
    // `MUSIC` entries of the dialogs (`MenuMusic_Play` on opening). Of the targets only `start.dlg` has one
    // (`title.ogg`): "Continue" without automatic continue opens `selectcampaign` without a music effect, the menu
    // stays silent after `win.ogg`/`lose.ogg` (without a loop); "Main menu" starts the title music.
    let Some(mut d) = live_at(1, 2, 2) else {
        return;
    };
    open_accounting(&mut d);
    d.click("accounting", "quit");
    d.idle();
    assert_eq!(effects(&mut d), []);
    assert_eq!(
        sorted(d.app.menu().open_names()),
        names(&[SELECT, "kampagne1", LEVEL])
    );

    let Some(mut d) = live_at(1, 2, 2) else {
        return;
    };
    open_accounting(&mut d);
    d.click("accounting", "mainmenu");
    d.idle();
    let music: Vec<UiEffect> = effects(&mut d)
        .into_iter()
        .filter(|e| matches!(e, UiEffect::MenuMusic { .. } | UiEffect::RestartMusic))
        .collect();
    assert_eq!(music.len(), 1, "{music:?}");
    assert!(
        matches!(&music[0], UiEffect::MenuMusic { path, looping: true } if path.to_ascii_lowercase().ends_with("title.ogg")),
        "{music:?}"
    );
}
