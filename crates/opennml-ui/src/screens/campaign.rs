//! Campaign menus (re/singleplayer-menus part 1, 4.1-4.4): `SELECTCAMPAIGN`
//! (`FUN_004DA4A0`), the lists `KAMPAGNE0...3` (`FUN_004D6DA0`), the level pages `CAMPAIGNLEVEL`/`CAMPAIGNLEVEL2`
//! (`FUN_004DABD0`) and the video page `CAMPAIGNVIDEO` (`FUN_004DA8A0`).
//!
//! Campaign and entry are global (`current_campaign`/`current_level` of the profile file, O-162); the
//! handlers read the entry through [`current_entry`]. An entry outside the list (after O-203/O-205) is read by the
//! original with an exception (`FUN_0040BF80` -> `FUN_005E86BF`); the reimplementation treats it like a headline
//! (no detail, no start, O-206). `VIDEOSTART` books the video (`0x1C`), plays it (`0x1D`) and continues with `0x28`
//! ([`video_commands`]).

use opennml_formats::campaign::EntryKind;

use super::{ScreenEnv, close, open};
use crate::campaign::{current_entry, select_campaign, select_entry};
use crate::draft::{atol, text_of};
use crate::msg::{AppCmd, ElementMsg, Param, code};

/// Texts of the box `LEVELLEVEL`: "Easy", "Normal", "Hard" (15770 ... 15772).
const EASY_TEXT: i32 = 15770;

/// Kind of the current entry (`FUN_004BA550` -> `FUN_0040BF80`); `None` outside the list.
fn current_kind(env: &ScreenEnv) -> Option<EntryKind> {
    current_entry(&env.data.profiles, &env.data.campaigns).map(|e| e.kind)
}

/// Campaign (`FUN_004BA540`).
fn campaign(env: &ScreenEnv) -> i32 {
    env.data.profiles.file.current_campaign
}

/// Level page of campaign `k`: `campaignlevel` for k > 0, otherwise `campaignlevel2` (introduction).
fn level_page(k: i32) -> &'static str {
    if k > 0 {
        "campaignlevel"
    } else {
        "campaignlevel2"
    }
}

/// Detail page d(k, kind): level -> [`level_page`], video -> `campaignvideo`, headline (and an entry outside the
/// list, O-206) -> none.
fn detail(env: &ScreenEnv) -> Option<&'static str> {
    match current_kind(env)? {
        EntryKind::Level => Some(level_page(campaign(env))),
        EntryKind::Video => Some("campaignvideo"),
        EntryKind::Headline => None,
    }
}

/// Name of the list of campaign `k` (`kampagne<k>`; the original takes it from a field with four names).
fn list_name(k: i32) -> String {
    format!("kampagne{k}")
}

/// Command 4 `{4, name}` (flag 0).
fn close_now(name: &str) -> AppCmd {
    close(name, false)
}

/// `SELECTCAMPAIGN` (`FUN_004DA4A0`, 4.1). k = campaign. On opening, **no** new choice is made: the list opens with the
/// saved entry.
pub(super) fn handle_select(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let k = campaign(env);
    let valid = (0..4).contains(&k);
    match (msg.element.as_str(), msg.code) {
        // `0x004DA54F`...`0x004DA586`: `0 <= k < 4` -> `{6,"kampagne<k>"}`; return value 1 without `status`.
        ("selectcampaign", code::QUERY_STATUS) => {
            if valid {
                env.queue.push(open_now(&list_name(k)));
            }
            1
        }
        // `0x004DA5A6`...`0x004DA65E`: `{3, frame}`; `0 <= k < 4` -> `{4,"kampagne<k>"}` and `{4, d}`, if there is a d;
        // `{5,"singleplayermenu"}`.
        ("cancel", code::TRIGGERED) => {
            env.queue.push(close(&msg.dialog, true));
            if valid {
                env.queue.push(close_now(&list_name(k)));
                if let Some(d) = detail(env) {
                    env.queue.push(close_now(d));
                }
            }
            env.queue.push(open("singleplayermenu"));
            1
        }
        // `0x004DA69C`...`0x004DA7E4`: buttons `kampagne*` (`Left(8)`), n = `atol(Mid(8))`. n == k -> nothing;
        // otherwise `{3,"kampagne<k>"}` and `{4, detail}` (video -> `campaignvideo`, otherwise the level page, also
        // for a headline without an open detail), `{5,"kampagne<n>"}` (index unchecked), `FUN_004B9860(n)` (campaign
        // = n, entry = highest free entry).
        (element, code::TRIGGERED) if element.starts_with("kampagne") => {
            let n = atol(&element.as_bytes()["kampagne".len()..]);
            if n != k {
                if valid {
                    env.queue.push(close(&list_name(k), true));
                    let old = if current_kind(env) == Some(EntryKind::Video) {
                        "campaignvideo"
                    } else {
                        level_page(k)
                    };
                    env.queue.push(close_now(old));
                }
                env.queue.push(open(&list_name(n)));
                select_campaign(&mut env.data.profiles, n);
            }
            1
        }
        // `0x004DA841`: everything else -1 (also `kampagne*` with another code).
        _ => -1,
    }
}

/// Command 6 `{6, name}` (flag 0).
fn open_now(name: &str) -> AppCmd {
    AppCmd::Open {
        name: name.to_string(),
        wait: false,
    }
}

/// `KAMPAGNE0...3` (`FUN_004D6DA0`, 4.2). The list always shows the current campaign; the handler does not use the
/// number in the name (the `atol` is discarded). It answers messages it does not handle with 0.
pub(super) fn handle_list(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // `0x004D6E8A`...`0x004D6F15`: frame (`Left(8) == "kampagne"`) code 5 -> `FUN_004B9830(entry)` (path, in the
        // reimplementation from the table), then the detail of the current entry with `{6}` - **without lock check**
        // (O-162); headline: none.
        (element, code::QUERY_STATUS) if element.starts_with("kampagne") => {
            if let Some(d) = detail(env) {
                env.queue.push(open_now(d));
            }
            1
        }
        // `0x004D7083`...`0x004D7093`: `int` = entry (list index = entry index).
        ("dir", code::QUERY_VALUE) => {
            let e = env.data.profiles.file.current_level;
            msg.params.insert("int".into(), Param::Int(e));
            1
        }
        // `0x004D7030`...`0x004D705B`, `FUN_0040BED0`: names (`+4`) of the entries of the campaign in file order,
        // with a profile up to before the first locked one, without one all (O-161).
        ("dir", code::QUERY_ITEMS) => {
            let k = campaign(env);
            let list = env.data.profiles.unlocked_list(k, &env.data.campaigns);
            let entries = usize::try_from(k)
                .ok()
                .and_then(|k| env.data.campaigns.campaigns.get(k));
            let items = list
                .into_iter()
                .filter_map(|e| entries.and_then(|x| x.get(e)))
                .map(|x| text_of(env.texts, x.name_id))
                .collect();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        // `0x004D6F51`...`0x004D7016`: close the detail of the old entry (`{4}`), entry = i (`FUN_004B9830`), open
        // the detail of the new one (`{5}`); a headline opens none. Without `int` 0.
        ("dir", code::ITEM_SELECTED) => {
            let Some(Param::Int(i)) = msg.params.get("int").cloned() else {
                return 0;
            };
            if let Some(d) = detail(env) {
                env.queue.push(close_now(d));
            }
            select_entry(&mut env.data.profiles, i);
            if let Some(d) = detail(env) {
                env.queue.push(open(d));
            }
            1
        }
        // `0x004D70D4`: anything else (also code 3 and double click: no start by double click) 0.
        _ => 0,
    }
}

/// `CAMPAIGNLEVEL` and `CAMPAIGNLEVEL2` (`FUN_004DABD0`, 4.3; `CAMPAIGNLEVEL2` has no box). Flag "3 levels"
/// [`super::ScreenState::campaign_three_levels`], difficulty [`super::MenuData::campaign_difficulty`].
pub(super) fn handle_level(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let three = env.state.campaign_three_levels;
    match (msg.element.as_str(), msg.code) {
        // `0x004DAC89`...`0x004DACC2`: without profile (`FUN_004BA580() == -1`) or after a defeat in the current
        // entry (`FUN_004BA590` = `m_TrialCount` of the first record with campaign/entry,
        // `FUN_0040C0C0`/`FUN_0040BD00`; invalid profile or no record: 0) flag 1 and difficulty 0 ("Easy", O-156),
        // otherwise flag 0. Return value -1.
        (element, code::QUERY_STATUS) if element == msg.dialog => {
            let file = &env.data.profiles.file;
            let lost = || {
                usize::try_from(file.current)
                    .ok()
                    .and_then(|i| file.profiles.get(i))
                    .and_then(|p| {
                        p.levels.iter().find(|l| {
                            l.campaign == file.current_campaign && l.level == file.current_level
                        })
                    })
                    .is_some_and(|l| l.trials > 0)
            };
            if file.current == -1 || lost() {
                env.state.campaign_three_levels = true;
                env.data.campaign_difficulty = 0;
            } else {
                env.state.campaign_three_levels = false;
            }
            -1
        }
        // `0x004DAD20`...`0x004DAD69`: text of the text field `+8` (`FUN_004BA950`, for levels the `AUTOSAVENAME`); the
        // element is in no `.dlg` (O-46).
        ("leveltitel", code::QUERY_TEXT) => {
            if let Some(e) = current_entry(&env.data.profiles, &env.data.campaigns) {
                let text = text_of(env.texts, e.text_id);
                msg.params.insert("text".into(), Param::Text(text));
            }
            1
        }
        // `0x004DAF5B`...`0x004DAF7A`: v = difficulty, with flag 0 and v > 0 one less: two steps show "Normal" at
        // v = 0 (O-157).
        ("levellevel", code::QUERY_VALUE) => {
            let v = env.data.campaign_difficulty;
            let shown = if !three && v > 0 { v - 1 } else { v };
            msg.params.insert("int".into(), Param::Int(shown));
            1
        }
        // `0x004DAE7E`...`0x004DAF01`: flag 1 15770 ... 15772, flag 0 15771, 15772.
        ("levellevel", code::QUERY_ITEMS) => {
            let first = if three { EASY_TEXT } else { EASY_TEXT + 1 };
            let items = (first..=EASY_TEXT + 2)
                .map(|id| text_of(env.texts, id))
                .collect();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        // `0x004DADEA`...`0x004DAE74`: flag 1 index 0/1/2 -> 0/1/2, otherwise 1; flag 0 index 0 -> 1, 1 -> 2,
        // otherwise 1 (`FUN_004BA5A0`). Without `int` 0.
        ("levellevel", code::ITEM_SELECTED) => {
            let Some(Param::Int(i)) = msg.params.get("int").cloned() else {
                return 0;
            };
            env.data.campaign_difficulty = match (three, i) {
                (true, 0..=2) => i,
                (false, 0 | 1) => i + 1,
                _ => 1,
            };
            1
        }
        // `0x004DAFB3`...`0x004DB034`: `{4,"selectcampaign"}`, `{4,"kampagne<campaign>"}` (index unchecked), `{4,
        // frame}`, `{0x15}`, `{0x0C}`.
        ("levelstart", code::TRIGGERED) => {
            let k = campaign(env);
            env.queue.push(close_now("selectcampaign"));
            env.queue.push(close_now(&list_name(k)));
            env.queue.push(close_now(&msg.dialog));
            env.queue.push(AppCmd::WaitDialogs);
            env.queue.push(AppCmd::StartCampaignLevel);
            1
        }
        // `0x004DB074`: anything else -1.
        _ => -1,
    }
}

/// Path of the current entry for `{0x1D, path}` (`FUN_004BFD90` -> `FUN_0040CB20`): empty for headlines. For
/// an entry outside the list `FUN_0040CB20` throws an exception (`FUN_005E86BF`); the reimplementation then takes the
/// text that `FUN_004BFD90` would insert without a match, "sorry for that!" (no video opens, the sequence continues;
/// O-206).
fn video_path(env: &ScreenEnv) -> String {
    current_entry(&env.data.profiles, &env.data.campaigns)
        .map_or_else(|| "sorry for that!".to_string(), |e| e.path.clone())
}

/// Booking and playing the current entry: `{0x1C}`, `{0x1D, path}`, `{0x1A}`, `{0x28}` (shared by `VIDEOSTART` and
/// "Continue" of the accounting). The call reads the path when queuing; booking happens only with `0x1C`.
pub(super) fn video_commands(env: &ScreenEnv) -> [AppCmd; 4] {
    [
        AppCmd::BookCampaign,
        AppCmd::PlayVideo(video_path(env)),
        AppCmd::WaitVideo,
        AppCmd::AfterVideo,
    ]
}

/// `CAMPAIGNVIDEO` (`FUN_004DA8A0`, 4.4).
pub(super) fn handle_video(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // `0x004DA955`...`0x004DA9AA`: video text from the text field `+8`; `VIDEOTITEL` is in no `.dlg` (O-46).
        ("videotitel", code::QUERY_TEXT) => {
            if let Some(e) = current_entry(&env.data.profiles, &env.data.campaigns) {
                let text = text_of(env.texts, e.text_id);
                msg.params.insert("text".into(), Param::Text(text));
            }
            1
        }
        // `0x004DAA0D`...`0x004DAB18`: `{4,"selectcampaign"}`, `{4,"kampagne<campaign>"}`, `{4, frame}`, `{0x15}`,
        // `{0x1C}`, `{0x1D, path}`, `{0x1A}`, `{0x28}`, `{5,"selectcampaign"}` - the last one is discarded by `0x28`
        // when clearing the queue (O-33), it never takes effect. Return value 1.
        ("videostart", code::TRIGGERED) => {
            let k = campaign(env);
            env.queue.push(close_now("selectcampaign"));
            env.queue.push(close_now(&list_name(k)));
            env.queue.push(close_now(&msg.dialog));
            env.queue.push(AppCmd::WaitDialogs);
            for cmd in video_commands(env) {
                env.queue.push(cmd);
            }
            env.queue.push(open("selectcampaign"));
            1
        }
        // `0x004DAB71`: anything else -1, also code 5.
        _ => -1,
    }
}
