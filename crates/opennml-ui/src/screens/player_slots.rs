//! Player slots before the match start: `SINGLEPLAYERSTART` (`FUN_004D81A0`, dispatcher of the dialog names
//! `0x004DC280`). See re/singleplayer-menus part 2, 4 (table 4.1, checks 4.2, slots 4.3).
//!
//! The elements are called `name<n>`, `race<n>`, `team<n>`, `handicap<n>`, `color<n>` (`LOOP 8`, the reader appends
//! the counter); the handler compares the beginning (`CString_Left`), reads the row with `atol` and takes
//! **key = row - 1** (row 0 = own slot). The slots are in the session ([`MatchDraft`]); the handler writes changes
//! back (`FUN_004E9750`). `LEVELINFO` (only the label `INFO` with `[CURLEVELINFOS]`, `draft::level_infos`) needs no
//! handler.
//!
//! The color list runs 0...8 up to "White" like the original (O-134).

use super::{ScreenEnv, close, open, requery};
use crate::draft::{HUMAN, MatchDraft, SlotDraft, atol, text_of};
use crate::msg::{AppCmd, ElementMsg, Param, code};

/// Status "hidden" (`HIDE`).
pub(super) const HIDE: i32 = 0x1000_0000;
/// Status "locked" (`DISABLE`).
pub(super) const DISABLE: i32 = 0x2000_0000;
/// `race<n>`: 15120...15125 WID, PRI, SPA, SID, ENG, PAT.
pub(super) const RACE_TEXT: i32 = 15120;
/// `handicap<n>`: 15140...15145 "0%...50%".
pub(super) const HANDICAP_TEXT: i32 = 15140;
/// `color<n>`: 15150...15158 Yellow ... White (`FUN_004BD0A0`, `0x3B2E`).
pub(super) const COLOR_TEXT: i32 = 15150;
/// Colors of the list: 0...8, last 15158 "White" (`FUN_004BD0A0`, loop up to 8).
const LAST_COLOR: i32 = 8;
/// `team<n>`: 15130... "Team 1...n", for game type 4 15590... "A-Def., A-Destr., B-Def., ..." (`FUN_004BCFD0`).
const TEAM_TEXT: i32 = 15130;
const RAILROAD_TEAM_TEXT: i32 = 15590;
/// `name<n>`: 15190 "Open", 15191...15194 "Computer (Easy/Normal/Hard/Manual)".
pub(super) const OPEN_TEXT: i32 = 15190;
/// Query interval of `team<n>` (code 9, `0x004D884E`): the lobby queries every second.
const TEAM_TIMEOUT: i32 = 1000;
/// Game type "destroy/defend railroad": slot = team (`NS+0x1F4 == 4`).
const RAILROAD: i32 = 4;

pub(super) fn int(msg: &ElementMsg) -> Option<i32> {
    match msg.params.get("int") {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}

pub(super) fn put(msg: &mut ElementMsg, name: &str, value: Param) {
    msg.params.insert(name.into(), value);
}

pub(super) fn put_texts(msg: &mut ElementMsg, env: &ScreenEnv, ids: impl IntoIterator<Item = i32>) {
    let items = ids.into_iter().map(|id| text_of(env.texts, id)).collect();
    put(msg, "array", Param::Array(items));
}

/// List of `team<n>` (`FUN_004BCFD0`): as many entries as slots, values 0...n-1; texts starting at 15130, or at
/// 15590 for game type 4.
pub(super) fn put_team_items(msg: &mut ElementMsg, env: &ScreenEnv) {
    let first = if env.data.draft.settings.game_type == RAILROAD {
        RAILROAD_TEAM_TEXT
    } else {
        TEAM_TEXT
    };
    let n = env.data.draft.slot_count();
    put_texts(msg, env, (0..n).map(|i| first.wrapping_add(i)));
    put(msg, "indexarray", Param::IndexArray((0..n).collect()));
}

/// Status of all columns except `name`/`handicap`: slot present -> 0, otherwise `HIDE` (`FUN_004E97E0`).
fn exists_status(msg: &mut ElementMsg, draft: &MatchDraft, key: i32) -> i32 {
    let status = if draft.slot(key).is_some() { 0 } else { HIDE };
    put(msg, "status", Param::Int(status));
    1
}

/// Columns of a row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Race,
    Handicap,
    Color,
    Team,
    Name,
}

/// `CString_Left` + `_atol(Mid)` in the order of the original: `race`, `handicap`, `color`, `team`, `name`.
fn column(element: &str) -> Option<(Column, i32)> {
    [
        ("race", Column::Race),
        ("handicap", Column::Handicap),
        ("color", Column::Color),
        ("team", Column::Team),
        ("name", Column::Name),
    ]
    .into_iter()
    .find_map(|(prefix, c)| {
        element
            .strip_prefix(prefix)
            .map(|rest| (c, atol(rest.as_bytes()).wrapping_sub(1)))
    })
}

/// `FUN_004D81A0` for `SINGLEPLAYERSTART`.
pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let dialog = msg.dialog.clone();
    if let Some((col, key)) = column(&msg.element) {
        return match col {
            Column::Name => name(msg, env, &dialog, key),
            _ => cell(msg, env, col, key),
        };
    }
    match (msg.element.as_str(), msg.code) {
        // `0x004D8E9B`...`0x004D8EB3`: `FUN_004BA8F0()` >= 2 -> 0, otherwise `HIDE`.
        ("start2", code::QUERY_STATUS) => {
            let status = if occupied(&env.data.draft) >= 2 {
                0
            } else {
                HIDE
            };
            put(msg, "status", Param::Int(status));
            1
        }
        // `0x004D8E21`...`0x004D8E96`: `{3, dialog}`, `{4,"levelinfo"}`, `{0x15}`, `{0x0D}`.
        ("start2", code::TRIGGERED) => {
            env.queue.push(close(&dialog, true));
            env.queue.push(close("levelinfo", false));
            env.queue.push(AppCmd::WaitDialogs);
            env.queue.push(AppCmd::StartMatch);
            1
        }
        // `0x004D8ED0`...`0x004D8F46`: `{3, dialog}`, `{4,"levelinfo"}`, `{5,"createsingleplayergame"}`; the slots
        // stay until the next `CREATE2`.
        ("exit2", code::TRIGGERED) => {
            env.queue.push(close(&dialog, true));
            env.queue.push(close("levelinfo", false));
            env.queue.push(open("createsingleplayergame"));
            1
        }
        _ => -1,
    }
}

/// `race<n>`, `handicap<n>`, `color<n>`, `team<n>` (`0x004D8357`...`0x004D88F0`). Code 0 and 4 without a slot -> 0;
/// unknown codes -> -1.
fn cell(msg: &mut ElementMsg, env: &mut ScreenEnv, col: Column, key: i32) -> i32 {
    let draft = &mut env.data.draft;
    match msg.code {
        code::QUERY_VALUE => {
            let Some(s) = draft.slot(key) else {
                return 0;
            };
            let v = match col {
                Column::Race => s.nation,
                Column::Handicap => s.handicap,
                Column::Color => s.color,
                _ => s.team,
            };
            put(msg, "int", Param::Int(v));
            1
        }
        code::QUERY_ITEMS => {
            match col {
                Column::Race => put_texts(msg, env, RACE_TEXT..RACE_TEXT + 6),
                Column::Handicap => put_texts(msg, env, HANDICAP_TEXT..HANDICAP_TEXT + 6),
                Column::Color => {
                    let colors = color_list(&env.data.draft, key);
                    put_texts(msg, env, colors.iter().map(|c| COLOR_TEXT + c));
                    put(msg, "indexarray", Param::IndexArray(colors));
                }
                _ => put_team_items(msg, env),
            }
            1
        }
        code::ITEM_SELECTED => {
            let (Some(v), Some(mut slot)) = (int(msg), draft.slot(key).cloned()) else {
                return 0;
            };
            match col {
                // Nation and handicap without a check.
                Column::Race => slot.nation = v,
                Column::Handicap => slot.handicap = v,
                Column::Color => {
                    slot.color = v;
                    check_change(draft, &mut slot);
                }
                _ => {
                    slot.team = v;
                    check_change(draft, &mut slot);
                }
            }
            draft.set_slot(slot);
            1
        }
        code::QUERY_STATUS => match col {
            // `0x004D848F`...`0x004D84CA`: `HIDE` without a slot; computer easy/normal/hard -> `DISABLE` (O-133).
            Column::Handicap => {
                let status = match draft.slot(key).map(|s| s.kind) {
                    None => HIDE,
                    Some(1..=3) => DISABLE,
                    Some(_) => 0,
                };
                put(msg, "status", Param::Int(status));
                1
            }
            _ => exists_status(msg, draft, key),
        },
        code::QUERY_TIMEOUT if col == Column::Team => {
            put(msg, "timeout", Param::Int(TEAM_TIMEOUT));
            1
        }
        _ => -1,
    }
}

/// `name<n>` (`0x004D89D3`...`0x004D8DF0`).
fn name(msg: &mut ElementMsg, env: &mut ScreenEnv, dialog: &str, key: i32) -> i32 {
    let row = key.wrapping_add(1);
    match msg.code {
        // Type (`FUN_004E8D20`, -1 without a slot), then `handicap<n>` code 5 again.
        code::QUERY_VALUE => {
            put(msg, "int", Param::Int(env.data.draft.kind_of(key)));
            requery(env, dialog, &format!("handicap{row}"), code::QUERY_STATUS);
            1
        }
        // 15190 "Open" (0); for the human his name (999), otherwise 15191...15194 (1...4).
        code::QUERY_ITEMS => {
            let mut items = vec![text_of(env.texts, OPEN_TEXT)];
            let mut values = vec![0];
            if env.data.draft.kind_of(key) == HUMAN {
                items.push(
                    env.data
                        .draft
                        .slot(key)
                        .map_or_else(String::new, |s| s.name.clone()),
                );
                values.push(HUMAN);
            } else {
                for kind in 1..=4 {
                    items.push(text_of(env.texts, OPEN_TEXT + kind));
                    values.push(kind);
                }
            }
            put(msg, "array", Param::Array(items));
            put(msg, "indexarray", Param::IndexArray(values));
            1
        }
        // `0x004D8BF4`...`0x004D8DF0` (table `0x004D9018`): set type; 0 -> handicap 0, team = row; 1 -> handicap
        // 5 (50%), 2 -> 2 (20%), 3 and 4 -> 0, for 1...4 `FUN_004BA980` (team of the first other computer);
        // write back; then `handicap<n>` and `start2` code 5 again. The original also builds the re-query for
        // `team<n>` but does not deliver it (`0x004D8D06`, O-189): the team shows the new value only with the query
        // interval of one second.
        code::ITEM_SELECTED => {
            let draft = &mut env.data.draft;
            let (Some(kind), Some(mut slot)) = (int(msg), draft.slot(key).cloned()) else {
                return 0;
            };
            slot.kind = kind;
            match kind {
                0 => {
                    slot.handicap = 0;
                    slot.team = row;
                }
                1..=4 => {
                    slot.handicap = match kind {
                        1 => 5,
                        2 => 2,
                        _ => 0,
                    };
                    join_first_computer(draft, &mut slot);
                }
                _ => {}
            }
            draft.set_slot(slot);
            requery(env, dialog, &format!("handicap{row}"), code::QUERY_STATUS);
            requery(env, dialog, "start2", code::QUERY_STATUS);
            1
        }
        // Row 0 -> `DISABLE`; slot present -> 0; otherwise `HIDE`.
        code::QUERY_STATUS => {
            let status = if key == -1 {
                DISABLE
            } else if env.data.draft.slot(key).is_some() {
                0
            } else {
                HIDE
            };
            put(msg, "status", Param::Int(status));
            1
        }
        _ => -1,
    }
}

/// Keys of the slots over which the checks run (-1 ... n-2). If a key is missing, the original would leave the
/// previous record in the buffer; the keys are gapless after `CREATE2`, the reimplementation skips missing ones.
fn keys(draft: &MatchDraft) -> std::ops::Range<i32> {
    -1..draft.slot_count().wrapping_sub(1)
}

/// `FUN_004BA8F0`: number of slots with type != 0.
pub(super) fn occupied(draft: &MatchDraft) -> i32 {
    keys(draft)
        .filter(|&k| draft.slot(k).is_some_and(|s| s.kind != 0))
        .count() as i32
}

/// `FUN_004BD0A0(key)`: colors 0...8 without the color of any other slot (also an open one, O-134).
pub(super) fn color_list(draft: &MatchDraft, own: i32) -> Vec<i32> {
    (0..=LAST_COLOR)
        .filter(|&c| {
            !keys(draft)
                .filter(|&k| k != own)
                .any(|k| draft.slot(k).is_some_and(|s| s.color == c))
        })
        .collect()
}

/// Verdict of the team and color check `FUN_004BA710` ([`team_color_check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamColor {
    /// The choice stays.
    Ok,
    /// Reject: the slot is read anew from the session.
    Reject,
    /// Game type 4: the choice stays, the slot with the key `key` gets the old team `team` (swap).
    Swap { key: i32, team: i32 },
}

/// The slot with the key `key` in `slots` (`NetSession_GetLobbyPlayerInfo`).
fn slot_in(slots: &[SlotDraft], key: i32) -> Option<&SlotDraft> {
    slots.iter().find(|s| s.key == key)
}

/// `FUN_004BA710` (`0x004BA710`) after a team or color choice, without the jump for `Info.LevelType == 2` (the
/// caller does that, [`check_change`]): `slots` is the slot list of the session (keys -1 ... n-2), `slot` the
/// changed slot with the new value, `game_type` the game type (`NS+0x1F4`). Occupied slots count (type != 0; the
/// changed one with its type). Game type 4 and count <= new team -> reject. With more than one occupied slot, for
/// each other occupied slot in key order: same color -> reject; game type 4 and same team -> the other gets the old
/// team, done (swap, without checking the remaining colors); otherwise remember whether all others have the same
/// team -> then reject. If a key is missing, the reimplementation skips it (like [`keys`]).
///
/// Public for the lobby rule of the network host (`opennml-client`).
pub fn team_color_check(slots: &[SlotDraft], slot: &SlotDraft, game_type: i32) -> TeamColor {
    let own = slot.key;
    let railroad = game_type == RAILROAD;
    let keys = -1..i32::try_from(slots.len())
        .unwrap_or(i32::MAX)
        .wrapping_sub(1);
    let count = keys
        .clone()
        .filter(|&k| slot_in(slots, k).is_some_and(|s| s.kind != 0) || (k == own && slot.kind != 0))
        .count() as i32;
    if railroad && count <= slot.team {
        return TeamColor::Reject;
    }
    if count == 1 {
        return TeamColor::Ok;
    }
    let mut all_same = true;
    for k in keys.filter(|&k| k != own) {
        let Some(other) = slot_in(slots, k).filter(|s| s.kind != 0) else {
            continue;
        };
        if other.color == slot.color {
            return TeamColor::Reject;
        }
        if railroad {
            if other.team == slot.team {
                let team = slot_in(slots, own).map_or(other.team, |s| s.team);
                return TeamColor::Swap { key: k, team };
            }
        } else if other.team != slot.team {
            all_same = false;
        }
    }
    if !railroad && all_same {
        TeamColor::Reject
    } else {
        TeamColor::Ok
    }
}

/// `FUN_004BA710` on the session ([`team_color_check`]); skipped for `Info.LevelType == 2`. "Reject" = read the slot
/// anew from the session; a swap sets the old team at the other slot.
pub(super) fn check_change(draft: &mut MatchDraft, slot: &mut SlotDraft) {
    if draft.info.level_type == 2 {
        return;
    }
    match team_color_check(&draft.slots, slot, draft.settings.game_type) {
        TeamColor::Ok => {}
        TeamColor::Reject => {
            if let Some(old) = draft.slot(slot.key) {
                *slot = old.clone();
            }
        }
        TeamColor::Swap { key, team } => {
            if let Some(other) = draft.slot(key).cloned() {
                draft.set_slot(SlotDraft { team, ..other });
            }
        }
    }
}

/// `FUN_004BA980` (`0x004BA980`): not for `Info.LevelType == 2` or game type 4; otherwise team of the first other
/// slot with type 1...4 (human and open do not count).
pub(super) fn join_first_computer(draft: &MatchDraft, slot: &mut SlotDraft) {
    if draft.info.level_type == 2 || draft.settings.game_type == RAILROAD {
        return;
    }
    let own = slot.key;
    if let Some(other) = keys(draft)
        .filter(|&k| k != own)
        .filter_map(|k| draft.slot(k))
        .find(|s| s.kind != 0 && s.kind != HUMAN)
    {
        slot.team = other.team;
    }
}
