//! Tests of the player slots: `SINGLEPLAYERSTART` (`FUN_004D81A0`) per row of the table re/singleplayer-menus part 2,
//! 4.1 with the checks 4.2 (`FUN_004BA710`, `FUN_004BA980`, `FUN_004BA8F0`) and the command `0x0D`. The logic alone
//! runs through [`Logic`], in operation through `MenuApp` with the corpus dialogs (they need `NML_GAME_DIR`).

use super::create_game::{CREATE, live_game, pick, pick_map, settle};
use super::*;
use crate::draft::{LevelInfoData, MatchDraft, MatchRequest, SlotDraft};

const SPS: &str = "singleplayerstart";
const DISABLE: i32 = 0x2000_0000;

fn rq(element: &str, code: u32) -> Requery {
    Requery::new(SPS, element, code)
}

/// Draft with one slot per kind in `kinds` (keys -1, 0, ...): team = color = row, nation 0, handicap 0; the own slot
/// is named "Anna", the others "EMPTY".
fn draft(kinds: &[i32], game_type: i32) -> MatchDraft {
    MatchDraft {
        map: Some(0),
        info: LevelInfoData {
            name: "Map".into(),
            path: "data/levels/multiplayer/[4Players]-Map.lvl".into(),
            size_class: 1,
            players: i32::try_from(kinds.len()).unwrap(),
            level_type: 0,
            ..LevelInfoData::default()
        },
        slots: kinds
            .iter()
            .zip(0..)
            .map(|(&kind, row)| SlotDraft {
                key: row - 1,
                name: if row == 0 { "Anna" } else { "EMPTY" }.into(),
                kind,
                nation: 0,
                team: row,
                handicap: 0,
                color: row,
            })
            .collect(),
        settings: crate::draft::DraftSettings {
            game_type,
            ..Default::default()
        },
    }
}

impl Logic {
    fn with_draft(kinds: &[i32], game_type: i32) -> Logic {
        let mut l = Logic::new();
        l.data.draft = draft(kinds, game_type);
        l
    }

    fn slot(&self, key: i32) -> SlotDraft {
        self.data.draft.slot(key).cloned().expect("slot")
    }

    fn sps_status(&mut self, element: &str) -> Option<i32> {
        self.status(SPS, element).1
    }

    fn pick_int(&mut self, element: &str, v: i32) -> i32 {
        self.send_int(SPS, element, code::ITEM_SELECTED, v).0
    }

    fn index_array(&mut self, element: &str) -> Vec<i32> {
        let (_, msg) = self.send(SPS, element, code::QUERY_ITEMS);
        match msg.params.get("indexarray") {
            Some(Param::IndexArray(v)) => v.clone(),
            other => panic!("{element}: {other:?}"),
        }
    }

    fn take_requery(&mut self) -> Vec<Requery> {
        std::mem::take(&mut self.requery)
    }
}

#[test]
fn start2_hidden_below_two_occupied_slots() {
    // `start2` code 5: `FUN_004BA8F0()` (slots with kind != 0) >= 2 -> 0, otherwise `HIDE`.
    let mut l = Logic::with_draft(&[999, 0, 0], 0);
    assert_eq!(l.sps_status("start2"), Some(HIDE));
    l.data.draft.slots[2].kind = 3;
    assert_eq!(l.sps_status("start2"), Some(0));
    // Draft without a map (O-188): only the own slot -> hidden.
    l.data.draft = draft(&[999], 0);
    l.data.draft.info.path.clear();
    assert_eq!(l.sps_status("start2"), Some(HIDE));
}

// QUIRK O-133, O-189: kept
#[test]
fn computer_kinds_set_handicap_50_20_0_and_lock_it() {
    // `name<n>` code 4 (`0x004D8BF4`...`0x004D8DF0`, table `0x004D9018`): 1 -> handicap 5, 2 -> 2, 3 -> 0; then
    // `handicap<n>` and `start2` code 5 again (O-133). The original builds the requery to `team<n>` but does not
    // deliver it (O-189).
    for (kind, handicap) in [(1, 5), (2, 2), (3, 0)] {
        let mut l = Logic::with_draft(&[999, 0, 0], 0);
        l.data.draft.slots[1].handicap = 4;
        assert_eq!(l.pick_int("name1", kind), 1);
        let s = l.slot(0);
        assert_eq!((s.kind, s.handicap), (kind, handicap), "{kind}");
        assert_eq!(
            l.take_requery(),
            [
                rq("handicap1", code::QUERY_STATUS),
                rq("start2", code::QUERY_STATUS)
            ]
        );
        // `handicap<n>` code 5: computer easy/normal/hard -> `DISABLE`.
        assert_eq!(l.sps_status("handicap1"), Some(DISABLE), "{kind}");
    }
}

// QUIRK O-133: kept
#[test]
fn manual_computer_keeps_handicap_free() {
    // Kind 4 "Computer (Manual)": handicap 0 and freely selectable.
    let mut l = Logic::with_draft(&[999, 0, 0], 0);
    assert_eq!(l.pick_int("name2", 4), 1);
    assert_eq!((l.slot(1).kind, l.slot(1).handicap), (4, 0));
    assert_eq!(l.sps_status("handicap2"), Some(0));
    assert_eq!(l.pick_int("handicap2", 3), 1);
    assert_eq!(l.slot(1).handicap, 3);
    assert_eq!(l.query(SPS, "handicap2", code::QUERY_VALUE), (1, Some(3)));
    // List 15140...15145 "0 %...50 %" without `indexarray`; the own slot (human) is free.
    assert_eq!(
        l.items(SPS, "handicap0"),
        l.texts(&[15140, 15141, 15142, 15143, 15144, 15145])
    );
    assert_eq!(l.sps_status("handicap0"), Some(0));
    // Without a slot: hidden, code 0/4 -> 0.
    assert_eq!(l.sps_status("handicap5"), Some(HIDE));
    assert_eq!(l.query(SPS, "handicap5", code::QUERY_VALUE), (0, None));
    assert_eq!(l.pick_int("handicap5", 1), 0);
}

#[test]
fn open_slot_resets_handicap_and_team() {
    // Kind 0 "Open": handicap 0, team = row (key + 1).
    let mut l = Logic::with_draft(&[999, 1, 2], 0);
    l.data.draft.slots[2].team = 0;
    l.data.draft.slots[2].handicap = 2;
    assert_eq!(l.pick_int("name2", 0), 1);
    let s = l.slot(1);
    assert_eq!((s.kind, s.handicap, s.team), (0, 0, 2));
    // `name<n>` code 0: kind, then `handicap<n>` code 5 again; without a slot -1.
    l.take_requery();
    assert_eq!(l.query(SPS, "name2", code::QUERY_VALUE), (1, Some(0)));
    assert_eq!(l.take_requery(), [rq("handicap2", code::QUERY_STATUS)]);
    assert_eq!(l.query(SPS, "name6", code::QUERY_VALUE), (1, Some(-1)));
}

// QUIRK O-134: kept
#[test]
fn color_list_excludes_other_slots_even_open_ones() {
    // `FUN_004BD0A0`: colors 0...8 (8 = "White"), without the colors of all other slots - also open ones (O-134);
    // `indexarray` = color value.
    let mut l = Logic::with_draft(&[999, 0, 0], 0);
    l.data.draft.slots[1].color = 4;
    assert_eq!(l.index_array("color0"), [0, 1, 3, 5, 6, 7, 8]);
    assert_eq!(
        l.items(SPS, "color0"),
        l.texts(&[15150, 15151, 15153, 15155, 15156, 15157, 15158])
    );
    assert_eq!(l.index_array("color1"), [1, 3, 4, 5, 6, 7, 8]);
    // A slot without an entry (row 5) sees all colors of the existing slots as occupied.
    assert_eq!(l.index_array("color5"), [1, 3, 5, 6, 7, 8]);
    assert_eq!(l.query(SPS, "color1", code::QUERY_VALUE), (1, Some(4)));
    assert_eq!(l.sps_status("color1"), Some(0));
    assert_eq!(l.sps_status("color5"), Some(HIDE));
}

// QUIRK O-134: kept
#[test]
fn color_list_offers_white() {
    // O-134: 15158 "White" is the last entry (`indexarray` 8), without the colors of other slots; if another slot
    // occupies white, the entry is missing.
    let mut l = Logic::with_draft(&[999, 1, 0], 0);
    let index = l.index_array("color0");
    assert_eq!(index.last(), Some(&8));
    let items = l.items(SPS, "color0");
    assert_eq!(items.last(), l.texts(&[15158]).last());
    assert_eq!(items.len(), index.len());
    for c in [0, 1, 2] {
        assert_eq!(l.index_array(&format!("color{c}")).last(), Some(&8), "{c}");
    }
    // The choice of white is accepted (the slot is free) and removed from the list for the other slots.
    assert_eq!(l.pick_int("color0", 8), 1);
    assert_eq!(l.slot(-1).color, 8);
    assert!(!l.index_array("color1").contains(&8));
    assert_eq!(l.index_array("color0").last(), Some(&8), "own color stays");
    // Another occupied slot with white rejects the choice.
    assert_eq!(l.pick_int("color1", 8), 1);
    assert_ne!(l.slot(0).color, 8);
}

// QUIRK O-134: kept
#[test]
fn same_color_is_rejected() {
    // `FUN_004BA710`: with more than one occupied slot, a color that another **occupied** slot has rejects the change
    // (slot rebuilt from the session); the color of an open slot is allowed.
    let mut l = Logic::with_draft(&[999, 1, 0], 0);
    assert_eq!(l.pick_int("color0", 1), 1);
    assert_eq!(l.slot(-1).color, 0, "color of the computer");
    assert_eq!(l.pick_int("color0", 2), 1);
    assert_eq!(l.slot(-1).color, 2, "color of the open slot");
    // Only one occupied slot: no check.
    let mut l = Logic::with_draft(&[999, 0, 0], 0);
    assert_eq!(l.pick_int("color0", 1), 1);
    assert_eq!(l.slot(-1).color, 1);
    // `Info.LevelType == 2`: no check.
    let mut l = Logic::with_draft(&[999, 1, 0], 0);
    l.data.draft.info.level_type = 2;
    assert_eq!(l.pick_int("color0", 1), 1);
    assert_eq!(l.slot(-1).color, 1);
}

#[test]
fn all_in_one_team_is_rejected() {
    // `FUN_004BA710` (game type != 4): if all occupied slots would have the same team afterwards -> reject.
    let mut l = Logic::with_draft(&[999, 1, 1], 0);
    assert_eq!(l.pick_int("team0", 1), 1);
    assert_eq!(l.slot(-1).team, 1, "teams 1, 1, 2");
    assert_eq!(l.pick_int("team2", 1), 1);
    assert_eq!(l.slot(1).team, 2, "all in team 1 rejected");
    // Open slots do not count: two occupied -> the team of the computer is forbidden.
    let mut l = Logic::with_draft(&[999, 1, 0], 0);
    assert_eq!(l.pick_int("team0", 1), 1);
    assert_eq!(l.slot(-1).team, 0, "both occupied in team 1");
    // An open slot may not join the common team of all occupied ones either.
    l.data.draft.slots[1].team = 0;
    assert_eq!(l.pick_int("team2", 0), 1);
    assert_eq!(
        l.slot(1).team,
        2,
        "open slot into the team of all occupied rejected"
    );
    assert_eq!(l.pick_int("team2", 1), 1);
    assert_eq!(l.slot(1).team, 1);
    // `team<n>`: list 15130... "Team 1...n" with `indexarray` 0...n-1, code 9 `timeout 1000`.
    assert_eq!(l.items(SPS, "team1"), l.texts(&[15130, 15131, 15132]));
    assert_eq!(l.index_array("team1"), [0, 1, 2]);
    let (r, msg) = l.send(SPS, "team1", code::QUERY_TIMEOUT);
    assert_eq!((r, int_of(&msg, "timeout")), (1, Some(1000)));
}

// QUIRK O-190, O-191: kept
#[test]
fn railroad_teams_are_slots_and_swap() {
    // Game type 4: team texts 15590... "A-Vert., A-Zerst., ..."; team >= number of occupied slots -> reject; another
    // occupied slot with the new team gets the old one (swap), without further color check.
    let mut l = Logic::with_draft(&[999, 1, 0, 0], 4);
    assert_eq!(
        l.items(SPS, "team0"),
        l.texts(&[15590, 15591, 15592, 15593])
    );
    assert_eq!(l.pick_int("team0", 2), 1);
    assert_eq!(l.slot(-1).team, 0, "2 >= two occupied slots");
    assert_eq!(l.pick_int("team0", 1), 1);
    assert_eq!((l.slot(-1).team, l.slot(0).team), (1, 0), "swapped");
    // An open slot also swaps with an occupied one (the computer gets the old team 3 of the open one).
    assert_eq!(l.pick_int("team3", 0), 1);
    assert_eq!((l.slot(2).team, l.slot(0).team), (0, 3));
    // O-191: the computer now has team 3, although only two slots are occupied.
    // The limit counts only occupied slots (two), also for an open one.
    assert_eq!(l.pick_int("team3", 3), 1);
    assert_eq!(l.slot(2).team, 0, "3 >= two occupied slots");
    // O-190: after the first (here ineffective) swap the check returns - the color of the later computer stays
    // unchecked, two slots then have color 2.
    let mut l = Logic::with_draft(&[999, 1, 1], 4);
    l.data.draft.slots[1].team = 0;
    assert_eq!(l.pick_int("color0", 2), 1);
    assert_eq!((l.slot(-1).color, l.slot(1).color), (2, 2));
    // Without a common team the same color is rejected.
    let mut l = Logic::with_draft(&[999, 1, 1], 4);
    assert_eq!(l.pick_int("color0", 2), 1);
    assert_eq!(l.slot(-1).color, 0);
}

#[test]
fn new_computer_joins_the_first_computer_team() {
    // `FUN_004BA980`: computer newly occupied -> team of the first other slot with kind 1...4 (not for game type 4).
    let mut l = Logic::with_draft(&[999, 0, 0, 0], 0);
    assert_eq!(l.pick_int("name3", 1), 1);
    assert_eq!(l.slot(2).team, 3, "no other computer");
    assert_eq!(l.pick_int("name1", 2), 1);
    assert_eq!(
        (l.slot(0).kind, l.slot(0).team),
        (2, 3),
        "team of the computer in row 3"
    );
    assert_eq!(l.pick_int("name2", 4), 1);
    assert_eq!(l.slot(1).team, 3, "first other computer (row 1, team 3)");
    // The human does not count as a computer.
    let mut l = Logic::with_draft(&[999, 0], 0);
    assert_eq!(l.pick_int("name1", 3), 1);
    assert_eq!(l.slot(0).team, 1);
    let mut l = Logic::with_draft(&[999, 1, 0], 4);
    assert_eq!(l.pick_int("name2", 3), 1);
    assert_eq!(l.slot(1).team, 2, "game type 4: no joining");
}

#[test]
fn own_row_name_is_disabled() {
    // `name<n>` code 5: row 0 (key -1) -> `DISABLE`, otherwise slot -> 0, without a slot `HIDE`; code 2: 15190
    // "Open" (0), for the human his name (999), otherwise 15191...15194 (1...4).
    let mut l = Logic::with_draft(&[999, 0], 0);
    assert_eq!(l.sps_status("name0"), Some(DISABLE));
    assert_eq!(l.sps_status("name1"), Some(0));
    assert_eq!(l.sps_status("name2"), Some(HIDE));
    let mut own = l.texts(&[15190]);
    own.push("Anna".into());
    assert_eq!(l.items(SPS, "name0"), own);
    assert_eq!(l.index_array("name0"), [0, 999]);
    assert_eq!(
        l.items(SPS, "name1"),
        l.texts(&[15190, 15191, 15192, 15193, 15194])
    );
    assert_eq!(l.index_array("name1"), [0, 1, 2, 3, 4]);
    // `race<n>`: 15120...15125 without `indexarray`, code 4 without check.
    assert_eq!(
        l.items(SPS, "race1"),
        l.texts(&[15120, 15121, 15122, 15123, 15124, 15125])
    );
    assert_eq!(l.pick_int("race1", 5), 1);
    assert_eq!(l.slot(0).nation, 5);
    assert_eq!(l.query(SPS, "race1", code::QUERY_VALUE), (1, Some(5)));
    assert_eq!(l.sps_status("race2"), Some(HIDE));
    assert_eq!(l.send(SPS, "nowhere", code::TRIGGERED).0, -1);
}

#[test]
fn start2_queues_close_wait_and_0x0d() {
    // `start2` Code 8: `{3, Dialog}`, `{4,"levelinfo"}`, `{0x15}`, `{0x0D}`.
    let mut l = Logic::with_draft(&[999, 1], 0);
    assert_eq!(l.click(SPS, "start2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(SPS, true),
            close("levelinfo", false),
            AppCmd::WaitDialogs,
            AppCmd::StartMatch
        ]
    );

    // In operation: select a map, computer in row 1, start -> `UiEffect::StartMatch` without the open slots
    // (`FUN_004E9800`), afterwards no dialogs are open anymore.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [SPS, "levelinfo"]);
    assert!(
        element(&d, SPS, "start2").hidden,
        "only the own slot occupied"
    );
    pick(&mut d, SPS, "name1", 1);
    assert_eq!(d.app.data().draft.slot(0).map(|s| s.kind), Some(1));
    assert!(!element(&d, SPS, "start2").hidden);
    assert!(
        !element(&d, SPS, "handicap1").enabled,
        "computer easy locks"
    );
    let before = d.app.data().draft.clone();
    d.app.take_effects();
    settle(&mut d);
    d.click(SPS, "start2");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    let starts: Vec<MatchRequest> = d
        .app
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            UiEffect::StartMatch(r) => Some(r),
            _ => None,
        })
        .collect();
    let slots: Vec<SlotDraft> = before
        .slots
        .iter()
        .filter(|s| s.kind != 0)
        .cloned()
        .collect();
    assert_eq!(slots.len(), 2);
    assert_eq!(
        starts,
        [MatchRequest {
            level: before.info.path.clone(),
            game_type: before.settings.game_type,
            slots: slots.clone(),
            settings: before.settings.clone(),
            info: before.info.clone(),
        }]
    );
    assert_eq!(
        d.app.data().draft.slots,
        slots,
        "session without open slots"
    );
}

#[test]
fn exit2_keeps_slots_until_next_create2() {
    // `exit2` code 8: `{3, dialog}`, `{4,"levelinfo"}`, `{5,"createsingleplayergame"}`; the slots stay until
    // `CREATE2` rebuilds them.
    let mut l = Logic::with_draft(&[999, 1], 0);
    let before = l.data.draft.clone();
    assert_eq!(l.click(SPS, "exit2"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close(SPS, true),
            close("levelinfo", false),
            open(CREATE, true)
        ]
    );
    assert_eq!(l.data.draft, before);

    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "create2");
    d.idle();
    pick(&mut d, SPS, "name1", 3);
    assert_eq!(d.app.data().draft.slot(0).map(|s| s.kind), Some(3));
    settle(&mut d);
    d.click(SPS, "exit2");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [CREATE, "loadmultiplayermap"]);
    assert_eq!(d.app.data().draft.slot(0).map(|s| s.kind), Some(3), "stays");
    settle(&mut d);
    d.click(CREATE, "create2");
    d.idle();
    assert_eq!(
        d.app.data().draft.slot(0).map(|s| s.kind),
        Some(0),
        "rebuilt"
    );
}

// QUIRK O-135: kept
#[test]
fn plt_type_is_ignored_all_foreign_slots_open() {
    // `FUN_004BDDA0` reads `PLAYER_TYPE` and discards it (O-135): all slots except the own one are "Open", although
    // every multiplayer map writes `PLAYER_TYPE 0` (human).
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    let path = d.app.data().level.info.path.clone();
    let lvl = opennml_formats::text::decode_cp1252(&d.world.vfs.read(&path).unwrap());
    assert!(lvl.contains("PLAYER_TYPE "), "{path}");
    d.click(CREATE, "create2");
    d.idle();
    let slots = &d.app.data().draft.slots;
    assert!(slots.len() >= 2);
    assert_eq!(slots[0].kind, 999);
    assert!(slots[1..].iter().all(|s| s.kind == 0 && s.name == "EMPTY"));
    for row in 1..slots.len() {
        let e = element(&d, SPS, &format!("name{row}"));
        assert_eq!(e.selection, Some(0), "row {row} \"Open\"");
    }
    // Rows without a slot are invisible.
    assert!(element(&d, SPS, &format!("name{}", slots.len())).hidden);
}

// QUIRK O-189: kept
#[test]
fn joined_team_shows_after_the_timeout() {
    // O-189: `name<n>` code 4 does not deliver the built requery to `team<n>`; the team shows the join
    // (`FUN_004BA980`) only when the query interval of `team<n>` (1000 ms) expires.
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    // A map with at least three slots.
    let maps = &d.app.data().maps;
    let row = maps
        .filtered(0)
        .iter()
        .position(|&i| maps.maps[i].slots.len() >= 3)
        .expect("map with three slots");
    pick_map(&mut d, i32::try_from(row).unwrap());
    settle(&mut d);
    d.click(CREATE, "create2");
    d.idle();
    pick(&mut d, SPS, "name1", 2);
    // Occupy row 2: it joins the team of the computer in row 1.
    pick(&mut d, SPS, "name2", 1);
    let team = d.app.data().draft.slot(1).map(|s| s.team);
    assert_eq!(team, d.app.data().draft.slot(0).map(|s| s.team));
    assert_ne!(team, Some(2));
    assert_eq!(
        element(&d, SPS, "team2").selection,
        Some(2),
        "still the old display"
    );
    d.run_until(d.now + 1000);
    assert_eq!(
        element(&d, SPS, "team2").selection,
        team,
        "after the query interval"
    );
}

#[test]
fn levelinfo_shows_the_session_lines() {
    // `LEVELINFO` (text 15589 = `[CURLEVELINFOS]`) shows the lines from the session (`FUN_004B9C90(1)`).
    let Some(mut d) = live_game(EngineConfig::default()) else {
        return;
    };
    show(&mut d, CREATE);
    d.click(CREATE, "create2");
    d.idle();
    let texts = &d.world.texts;
    assert_eq!(
        texts.get(11),
        Some("[COLOR FF88FF88]"),
        "extrasymbols.txt 11"
    );
    let infos = crate::draft::level_infos(&d.app.data().draft, None, texts);
    assert!(infos.contains(&d.app.data().draft.info.name));
    assert_eq!(
        d.app
            .text_context()
            .and_then(|c| c.cur_level.as_ref())
            .map(|c| c.infos.clone()),
        Some(infos)
    );
}
