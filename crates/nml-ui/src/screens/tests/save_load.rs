//! Tests of saving and loading: `SAVEGAME`/`LOADSAVEDGAME2` (`FUN_004D6100`) and `INGAMELOADGAME` (`FUN_004D9830`) per
//! row of re/ingame-menus 3.2/3.3, the commands `0x0A`, `0x26`, `0x0B`, `0x2A` (re/ingame-menus 4, 4.1) and quicksave/
//! quickload (5). Savegames come from [`FakeSaveFiles`]; the logic alone runs through [`Logic`], in operation through
//! `MenuApp` with the corpus dialogs (they need `NML_GAME_DIR`).

use crate::saves::{LocalStamp, SaveDir, SaveFiles, Saves};
use crate::testutil::{FakeSaveFiles, stamp};

use super::*;

/// Free play (game type 0).
const FREE: GameInfo = GameInfo {
    game_type: 0,
    local_slot: 0,
    ui_locked: false,
    net: None,
    playback: false,
};
/// Network match as a client.
const NET: GameInfo = GameInfo {
    net: Some(NetRoleView::Client),
    ..FREE
};
const MENU3: &str = "ingameoptions3";
/// GUID of the profile "Bert" (second profile, active).
const BERT: i32 = 1;

/// Clock of the store: October 27, 2026, 9:05.
fn now() -> LocalStamp {
    stamp(2026, 10, 27, 9, 5)
}

/// Savegames of two profiles in list order (newest first): "new" and "old" of Bert, "foreign" of Anna.
fn fake() -> Rc<FakeSaveFiles> {
    Rc::new(FakeSaveFiles::new(
        &[
            (BERT, "new", stamp(2026, 10, 26, 14, 35)),
            (0, "foreign", stamp(2026, 10, 26, 15, 0)),
            (BERT, "old", stamp(2025, 1, 2, 9, 5)),
        ],
        now(),
    ))
}

/// The logic with the profiles Anna (GUID 0) and Bert (GUID 1, active), running match and store `saves`.
fn logic(saves: Rc<FakeSaveFiles>) -> Logic {
    let mut l = Logic::with_profiles(&["Anna", "Bert"], 1);
    l.data.saves = Saves(saves);
    l.data.game = Some(FREE);
    l
}

/// Corpus `MenuApp` with Anna and Bert (active) and the store `saves`.
fn live_saves(saves: &Rc<FakeSaveFiles>) -> Option<Driver<Corpus>> {
    let opts = MenuOptions {
        profiles: profiles_with(&["Anna", "Bert"], 1),
        saves: Some(saves.clone() as Rc<dyn SaveFiles>),
        ..MenuOptions::default()
    };
    live_with(EngineConfig::default(), opts, Box::new(dispatch))
}

/// Like [`live_saves`] with a running match, without an open menu.
fn live_game(saves: &Rc<FakeSaveFiles>) -> Option<Driver<Corpus>> {
    let mut d = live_saves(saves)?;
    d.app.set_game(Some(FREE));
    d.idle();
    Some(d)
}

fn queued(d: &Driver<Corpus>) -> Vec<AppCmd> {
    d.app.queue().iter().cloned().collect()
}

/// Only the effects of savegame and match (without click sounds, music and `0x20`).
fn save_effects(effects: Vec<UiEffect>) -> Vec<UiEffect> {
    effects
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                UiEffect::LoadGame { .. }
                    | UiEffect::SaveGame { .. }
                    | UiEffect::DeleteSave { .. }
                    | UiEffect::ResumeGame { .. }
            )
        })
        .collect()
}

fn load_game(name: &str) -> UiEffect {
    UiEffect::LoadGame { name: name.into() }
}

/// Double click in the center of an element (two clicks in the same frame, `DoubleClickTimeout` not expired).
fn double_click(d: &mut Driver<Corpus>, dialog: &str, element: &str) {
    d.click(dialog, element);
    d.click(dialog, element);
}

#[test]
fn save_list_shows_date_and_name_newest_first() {
    // re/ingame-menus 3.1: entries in the order of the store (newest first), display "(YY/MM/DD H:MM) Name"; code 4
    // takes over the name of the entry (`FUN_004BC3E0`).
    let shown = ["(26/10/26 14:35) new", "(25/01/02 9:05) old"];
    let mut l = logic(fake());
    assert_eq!(l.items("savegame", "dir"), shown);
    assert_eq!(l.state.save_entries, ["new", "old"]);
    assert_eq!(l.send_int("savegame", "dir", code::ITEM_SELECTED, 1).0, 1);
    assert_eq!(l.state.save_name.get(), "old");
    // Outside the list: flag set, name stays (no entry there).
    assert_eq!(l.send_int("savegame", "dir", code::ITEM_SELECTED, 5).0, 1);
    assert_eq!(l.state.save_name.get(), "old");
    assert_eq!(l.items("loadsavedgame2", "dir"), shown);
    assert_eq!(l.items("ingameloadgame", "dir"), shown);
    assert_eq!(l.send("ingameloadgame", "dir", code::FREE_ITEMS).0, 1);
    assert_eq!(l.send("savegame", "dir", code::FREE_ITEMS).0, 1);

    // In operation: `SAVEGAME` shows the list, `FILENAME` the newest name.
    let saves = fake();
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    d.push(open("savegame", true));
    d.idle();
    let dir = element(&d, "savegame", "dir");
    assert_eq!(dir.items, shown);
    assert_eq!(
        element(&d, "savegame", "filename").text.as_deref(),
        Some("new")
    );
}

#[test]
fn save_list_filters_by_profile_guid() {
    // Pattern `"*.%03X"` with the GUID of the active profile (`FUN_0040BC40`): Anna sees only her savegame, and "new"
    // (from Bert) counts as nonexistent for her.
    let mut l = logic(fake());
    l.data.profiles.select(0);
    assert_eq!(l.items("savegame", "dir"), ["(26/10/26 15:00) foreign"]);
    l.state.save_name.set("new");
    assert_eq!(l.click("savegame", "save"), 1);
    assert_eq!(
        l.take_queue(),
        [close("savegame", false), AppCmd::SubmitSave]
    );
    // Bert: "new" exists -> confirmation prompt.
    l.data.profiles.select(1);
    assert_eq!(l.items("ingameloadgame", "dir").len(), 2);
    l.click("savegame", "save");
    assert_eq!(l.state.info_text, Some(15785));
}

#[test]
fn filename_filter_rejects_reserved_characters() {
    // `0x004D617E`...`0x004D61ED`: `" * . / : < > ? \ |` -> 1 (discard), otherwise -1, without `key` -1; code 7 pointer
    // to `GameApp+0x177C`, `size 40`, `inputbuffer 1`; other codes 0.
    let mut l = logic(fake());
    let key = |l: &mut Logic, dialog: &str, k: i32| {
        let mut msg = ElementMsg::new(dialog, "filename", code::EDIT_KEY);
        msg.params.insert("key".into(), Param::Int(k));
        l.deliver(msg).0
    };
    for c in ['"', '*', '.', '/', ':', '<', '>', '?', '\\', '|'] {
        assert_eq!(key(&mut l, "savegame", c as i32), 1, "{c}");
    }
    for c in ['a', 'Z', '0', ' ', '-', '_', 'ä', '\r', '\u{1b}'] {
        assert_eq!(key(&mut l, "savegame", c as i32), -1, "{c:?}");
    }
    assert_eq!(l.send("savegame", "filename", code::EDIT_KEY).0, -1);
    let (r, msg) = l.send("savegame", "filename", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.save_name.clone()))
    );
    assert_eq!(int_of(&msg, "size"), Some(40));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    assert_eq!(l.send("savegame", "filename", code::EDIT_CHANGED).0, 0);
    // `INGAMELOADGAME` (`FUN_004D9830`): dead `filename` with `size 0x104`, without filter (-1).
    assert_eq!(key(&mut l, "ingameloadgame", '/' as i32), -1);
    let (_, msg) = l.send("ingameloadgame", "filename", code::QUERY_TEXT);
    assert_eq!(int_of(&msg, "size"), Some(0x104));

    // In operation: typing in `FILENAME` omits the reserved characters and writes into the name buffer.
    let saves = Rc::new(FakeSaveFiles::new(&[], now()));
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    d.push(open("savegame", true));
    d.idle();
    d.click("savegame", "filename");
    d.frame();
    // Characters with unassigned virtual key codes (`:` 0x3A, `?` 0x3F, `<` 0x3C), so that no control key takes part.
    type_text(&mut d, "a:b?c<d");
    assert_eq!(d.app.screen_state().save_name.get(), "abcd");
    assert_eq!(
        element(&d, "savegame", "filename").text.as_deref(),
        Some("abcd")
    );
}

#[test]
fn empty_name_uses_default_save_name() {
    // `SAVE` (`0x004D6742`...): empty name -> `savegame_%04d_%02d_%02d_%02d%02d` from the local time of the host;
    // `{4, frame}`, not present -> `{0x26}`.
    let mut l = logic(Rc::new(FakeSaveFiles::new(&[], now())));
    assert_eq!(l.click("savegame", "save"), 1);
    assert_eq!(l.state.save_name.get(), "savegame_2026_10_27_0905");
    assert_eq!(
        l.take_queue(),
        [close("savegame", false), AppCmd::SubmitSave]
    );
    // A typed name stays.
    l.state.save_name.set("castle");
    l.click("savegame", "save");
    assert_eq!(l.state.save_name.get(), "castle");
    assert_eq!(
        l.take_queue(),
        [close("savegame", false), AppCmd::SubmitSave]
    );
    assert!(l.effects.is_empty());
}

#[test]
fn existing_save_asks_before_overwriting() {
    // File exists (`FUN_004C6B30`) -> confirmation prompt 15785, OK `{0x26}`, cancel `{5, frame}`.
    let mut l = logic(fake());
    l.state.save_name.set("old");
    assert_eq!(l.click("savegame", "save"), 1);
    assert_eq!(
        l.take_queue(),
        [close("savegame", false), open("infoframe", true)]
    );
    assert_eq!(l.state.info_text, Some(15785));
    assert_eq!(l.state.info_ok, Some(AppCmd::SubmitSave));
    assert_eq!(l.state.info_cancel, Some(open("savegame", true)));

    // In operation: OK writes, cancel opens `SAVEGAME` again.
    let saves = fake();
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    d.app.open_game_menu();
    d.idle();
    d.click(MENU3, "savegame3");
    d.idle();
    d.click("savegame", "save");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["infoframe"]);
    d.click("infoframe", "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["savegame"]);
    d.app.take_effects();
    d.click("savegame", "save");
    d.idle();
    d.click("infoframe", "ok");
    d.idle();
    assert_eq!(
        save_effects(d.app.take_effects()),
        [
            UiEffect::ResumeGame { game_speed: None },
            UiEffect::SaveGame { name: "new".into() }
        ]
    );
}

#[test]
fn save_selected_flag_is_always_set_and_targets_newest() {
    // O-144: `FUN_004D6100` code 2 always sets the flag (count >= 0) and enters the newest name; `LOAD4` and
    // `DELETEFILE` are visible immediately. With an empty list the old name stays.
    let mut l = logic(fake());
    l.state.save_name.set("before");
    l.items("loadsavedgame2", "dir");
    assert!(l.state.save_selected);
    assert_eq!(l.state.save_name.get(), "new");
    assert_eq!(l.status("loadsavedgame2", "load4"), (1, Some(0)));
    assert_eq!(l.status("loadsavedgame2", "deletefile"), (1, Some(0)));
    assert_eq!(l.click("loadsavedgame2", "load4"), 1);
    assert_eq!(
        l.take_queue(),
        [close("loadsavedgame2", true), AppCmd::LoadGame]
    );

    let mut l = logic(Rc::new(FakeSaveFiles::new(&[], now())));
    l.state.save_name.set("before");
    assert!(l.items("loadsavedgame2", "dir").is_empty());
    assert!(l.state.save_selected);
    assert_eq!(l.state.save_name.get(), "before");
    assert_eq!(l.status("loadsavedgame2", "load4"), (1, Some(0)));
    // Code 0 without `int` (no row marked); code 4 with -1 clears the flag and queries again.
    assert_eq!(
        l.query("loadsavedgame2", "dir", code::QUERY_VALUE),
        (1, None)
    );
    assert_eq!(
        l.send_int("loadsavedgame2", "dir", code::ITEM_SELECTED, -1)
            .0,
        1
    );
    assert!(!l.state.save_selected);
    assert_eq!(l.status("loadsavedgame2", "load4"), (1, Some(HIDE)));
    let rq = |e: &str| Requery::new("loadsavedgame2", e, code::QUERY_STATUS);
    assert_eq!(
        l.requery,
        [rq("load"), rq("load4"), rq("load2"), rq("deletefile")]
    );

    // `INGAMELOADGAME` preselects nothing: flag 0, `LOAD` and `DELETEFILE` hidden, name unchanged.
    let mut l = logic(fake());
    l.state.save_name.set("before");
    l.items("ingameloadgame", "dir");
    assert!(!l.state.load_selected);
    assert_eq!(l.state.save_name.get(), "before");
    assert_eq!(l.status("ingameloadgame", "load"), (1, Some(HIDE)));
    assert_eq!(l.status("ingameloadgame", "deletefile"), (1, Some(HIDE)));

    // In operation: `LOADSAVEDGAME2` from the single-player menu, `LOAD4` visible immediately without a marked row;
    // the click loads the newest savegame.
    let saves = fake();
    let Some(mut d) = live_saves(&saves) else {
        return;
    };
    d.push(open("singleplayermenu", true));
    d.idle();
    d.click("singleplayermenu", "loadsavedgame");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["loadsavedgame2"]);
    let load4 = element(&d, "loadsavedgame2", "load4");
    assert!(load4.visible && !load4.hidden);
    assert_eq!(element(&d, "loadsavedgame2", "dir").selection, Some(-1));
    d.app.take_effects();
    d.click("loadsavedgame2", "load4");
    d.idle();
    assert_eq!(save_effects(d.app.take_effects()), [load_game("new")]);
    assert!(d.app.menu().open_names().is_empty());
}

#[test]
fn delete_in_savegame_uses_typed_name() {
    // O-145: `DELETEFILE` asks 15174 (OK `{0x2A, frame}`, no cancel command); `0x2A` deletes `GameApp+0x177C`, i.e. in
    // `SAVEGAME` the typed name, not the row.
    let mut l = logic(fake());
    l.items("savegame", "dir");
    l.state.save_name.set("typed");
    assert_eq!(l.click("savegame", "deletefile"), 1);
    assert_eq!(l.take_queue(), [open("infoframe", true)]);
    assert_eq!(l.state.info_text, Some(15174));
    assert_eq!(l.state.info_ok, Some(AppCmd::DeleteSave("savegame".into())));
    assert_eq!(l.state.info_cancel, None);

    let saves = fake();
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    d.push(open("savegame", true));
    d.idle();
    d.click("savegame", "filename");
    d.frame();
    type_text(&mut d, "x");
    let typed = element(&d, "savegame", "filename").text.unwrap();
    assert!(typed.contains('x') && typed != "new", "{typed}");
    d.app.take_effects();
    d.click("savegame", "deletefile");
    d.idle();
    d.click("infoframe", "ok");
    d.idle();
    assert_eq!(
        save_effects(d.app.take_effects()),
        [UiEffect::DeleteSave {
            name: typed,
            dir: SaveDir::Main
        }]
    );
}

#[test]
fn return_command_survives_save() {
    // O-146: `SAVE` leaves the return command `{5,"ingameoptions3"}` in place; a later quickload with `CANCEL` then
    // opens the game menu instead of continuing to play with `{0x37}`.
    let saves = Rc::new(FakeSaveFiles::new(&[], now()));
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    assert!(d.app.open_game_menu());
    d.idle();
    d.click(MENU3, "savegame3");
    d.idle();
    assert_eq!(d.app.screen_state().return_cmd, Some(open(MENU3, true)));
    d.app.take_effects();
    d.click("savegame", "save");
    d.idle();
    assert_eq!(
        save_effects(d.app.take_effects()),
        [
            UiEffect::ResumeGame { game_speed: None },
            UiEffect::SaveGame {
                name: "savegame_2026_10_27_0905".into()
            }
        ]
    );
    assert_eq!(d.app.game_menu_state(), 0);
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.screen_state().return_cmd, Some(open(MENU3, true)));

    assert!(d.app.quick_load());
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["ingameloadgame"]);
    d.click("ingameloadgame", "cancel");
    d.idle();
    assert_eq!(d.app.menu().open_names(), [MENU3]);
    assert_eq!(d.app.game_menu_state(), 1);
    assert!(save_effects(d.app.take_effects()).is_empty());
    assert_eq!(d.app.screen_state().return_cmd, None);
}

#[test]
fn double_click_loads_only_in_loadsavedgame2() {
    // O-148: code `0xD` on `DIR` loads with the flag only in `LOADSAVEDGAME2` (`{3, frame}`, `{0x0A}`); no effect in
    // `SAVEGAME` (1), not handled in `INGAMELOADGAME` (-1).
    let mut l = logic(fake());
    assert_eq!(l.send("loadsavedgame2", "dir", code::DOUBLE_CLICK).0, 1);
    assert!(l.take_queue().is_empty(), "without flag");
    l.items("loadsavedgame2", "dir");
    assert_eq!(l.send("loadsavedgame2", "dir", code::DOUBLE_CLICK).0, 1);
    assert_eq!(
        l.take_queue(),
        [close("loadsavedgame2", true), AppCmd::LoadGame]
    );
    l.items("savegame", "dir");
    assert_eq!(l.send("savegame", "dir", code::DOUBLE_CLICK).0, 1);
    assert!(l.take_queue().is_empty());
    l.items("ingameloadgame", "dir");
    l.send_int("ingameloadgame", "dir", code::ITEM_SELECTED, 0);
    assert!(l.state.load_selected);
    assert_eq!(l.send("ingameloadgame", "dir", code::DOUBLE_CLICK).0, -1);
    assert!(l.take_queue().is_empty());

    // In operation with a full list (the center of the list hits a row).
    let entries: Vec<(i32, String, LocalStamp)> = (0..40u32)
        .map(|i| (BERT, format!("slot{i:02}"), stamp(2026, 1, 1, 0, 59 - i)))
        .collect();
    let refs: Vec<(i32, &str, LocalStamp)> = entries
        .iter()
        .map(|(g, n, t)| (*g, n.as_str(), *t))
        .collect();
    let saves = Rc::new(FakeSaveFiles::new(&refs, now()));
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    assert!(d.app.quick_load());
    d.idle();
    double_click(&mut d, "ingameloadgame", "dir");
    d.idle();
    let row = element(&d, "ingameloadgame", "dir").selection.unwrap();
    assert!(row >= 0, "double click selects a row");
    assert!(d.app.screen_state().load_selected);
    assert_eq!(d.app.menu().open_names(), ["ingameloadgame"]);
    assert!(save_effects(d.app.take_effects()).is_empty());
    let load = element(&d, "ingameloadgame", "load");
    assert!(load.visible && !load.hidden);

    let Some(mut d) = live_saves(&saves) else {
        return;
    };
    d.push(open("loadsavedgame2", true));
    d.idle();
    d.app.take_effects();
    double_click(&mut d, "loadsavedgame2", "dir");
    d.idle();
    // The double click selected a row below the center (not the preselected newest savegame) and loaded it.
    let name = d.app.screen_state().save_name.get();
    assert!(name.starts_with("slot") && name != "slot00", "{name}");
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(save_effects(d.app.take_effects()), [load_game(&name)]);
}

#[test]
fn ingame_load_cancel_without_return_resumes_with_0x37() {
    // re/ingame-menus 2.1: `INGAMELOADGAME CANCEL` queues `{0x37}` without a return command (quickload case),
    // otherwise the return command (released); `LOAD` releases it without queuing it, and queues `{0x0A}`.
    let mut l = logic(fake());
    assert_eq!(l.click("ingameloadgame", "cancel"), 1);
    assert_eq!(
        l.take_queue(),
        [close("ingameloadgame", true), AppCmd::ResumeAfterLoadDialog]
    );
    l.state.return_cmd = Some(open(MENU3, true));
    l.click("ingameloadgame", "cancel");
    assert_eq!(
        l.take_queue(),
        [close("ingameloadgame", true), open(MENU3, true)]
    );
    assert_eq!(l.state.return_cmd, None);
    l.state.return_cmd = Some(open(MENU3, true));
    assert_eq!(l.click("ingameloadgame", "load"), 1);
    assert_eq!(
        l.take_queue(),
        [close("ingameloadgame", true), AppCmd::LoadGame]
    );
    assert_eq!(l.state.return_cmd, None);
    // `SAVEGAME CANCEL3` without return command: fixed `{5,"ingameoptions"}`.
    assert_eq!(l.click("savegame", "cancel3"), 1);
    assert_eq!(
        l.take_queue(),
        [close("savegame", true), open("ingameoptions", true)]
    );
    // `LOADSAVEDGAME2 CANCEL4`: back to the single-player menu.
    assert_eq!(l.click("loadsavedgame2", "cancel4"), 1);
    assert_eq!(
        l.take_queue(),
        [
            close("loadsavedgame2", true),
            open("singleplayermenu", true)
        ]
    );

    // In operation: quickload (`FUN_0042F5C0`) pauses the game and opens `INGAMELOADGAME`; `CANCEL` resumes play.
    let saves = fake();
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    assert!(d.app.quick_load());
    assert_eq!(d.app.game_menu_state(), 1);
    assert_eq!(queued(&d), [open("ingameloadgame", false)]);
    assert!(!d.app.quick_load(), "state 1");
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["ingameloadgame"]);
    d.app.take_effects();
    d.click("ingameloadgame", "cancel");
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert_eq!(
        save_effects(d.app.take_effects()),
        [UiEffect::ResumeGame { game_speed: None }]
    );
    // Without a match no quickload.
    d.app.set_game(None);
    assert!(!d.app.quick_load());
    assert!(d.app.queue().is_empty());
}

#[test]
fn save_command_sequence_opens_savingmessage_then_writes() {
    // re/ingame-menus 4.1: `{0x26}` -> state 0, `{6,"SAVINGMESSAGE"}`, `{0x0B}`; frame n opens `SAVINGMESSAGE`, frame
    // n+1 writes (`SaveGame`) and closes it immediately. In between, `saving` is set.
    let saves = fake();
    let Some(mut d) = live_game(&saves) else {
        return;
    };
    d.app.take_effects();
    d.app.quick_save("quicksave1");
    assert_eq!(d.app.screen_state().save_name.get(), "quicksave1");
    assert_eq!(queued(&d), [AppCmd::SubmitSave]);
    assert!(!d.app.saving());
    d.frame();
    assert!(d.app.saving());
    assert_eq!(
        save_effects(d.app.take_effects()),
        [UiEffect::ResumeGame { game_speed: None }]
    );
    assert_eq!(
        queued(&d),
        [open("savingmessage", false), AppCmd::WriteSave]
    );
    d.frame();
    assert_eq!(d.app.menu().open_names(), ["savingmessage"]);
    assert!(d.app.saving());
    assert!(save_effects(d.app.take_effects()).is_empty());
    d.frame();
    assert!(!d.app.saving());
    assert_eq!(
        save_effects(d.app.take_effects()),
        [UiEffect::SaveGame {
            name: "quicksave1".into()
        }]
    );
    d.idle();
    assert!(d.app.menu().open_names().is_empty());

    // Quicksave only in state 0 (`FUN_0042F490`) and with a match.
    assert!(d.app.open_game_menu());
    d.idle();
    d.app.quick_save("quicksave2");
    assert!(d.app.queue().is_empty());
    assert_eq!(d.app.screen_state().save_name.get(), "quicksave1");
    d.app.set_game(None);
    d.app.quick_save("quicksave2");
    assert!(d.app.queue().is_empty());
    // `0x26` without a match does nothing (the original lacks the engine that accepts the message).
    d.push(AppCmd::SubmitSave);
    d.idle();
    assert!(!d.app.saving());
    assert!(save_effects(d.app.take_effects()).is_empty());
}

#[test]
fn delete_reopens_the_dialog() {
    // `0x2A` (`0x004C9D4B`...): delete (`UiEffect::DeleteSave` with `GameApp+0x177C`), then `{3, name}`, `{5, name}`
    // with the frame name from the command - the list rebuilds.
    let saves = fake();
    let Some(mut d) = live_saves(&saves) else {
        return;
    };
    d.push(open("loadsavedgame2", true));
    d.idle();
    d.click("loadsavedgame2", "deletefile");
    d.idle();
    d.app.take_effects();
    d.click("infoframe", "ok");
    // The host deletes at the end of the frame in which the effect arises.
    let until = d.now + 2000;
    loop {
        assert!(d.now < until, "no delete effect");
        d.frame();
        let effects = save_effects(d.app.take_effects());
        if effects.is_empty() {
            continue;
        }
        assert_eq!(
            effects,
            [UiEffect::DeleteSave {
                name: "new".into(),
                dir: SaveDir::Main
            }]
        );
        assert_eq!(
            queued(&d),
            [close("loadsavedgame2", true), open("loadsavedgame2", true)]
        );
        saves.remove(BERT, "new");
        break;
    }
    d.idle();
    assert_eq!(d.app.menu().open_names(), ["loadsavedgame2"]);
    assert_eq!(
        element(&d, "loadsavedgame2", "dir").items,
        ["(25/01/02 9:05) old"]
    );
    assert_eq!(d.app.screen_state().save_name.get(), "old");
}

/// Network folder with two savegames (newest first) next to the main savegames of [`fake`].
fn fake_net() -> Rc<FakeSaveFiles> {
    let f = Rc::try_unwrap(fake()).ok().unwrap();
    Rc::new(f.with_net(&[
        ("net_new", stamp(2026, 11, 7, 18, 30)),
        ("net_old", stamp(2026, 11, 1, 8, 0)),
    ]))
}

const NET_SHOWN: [&str; 2] = ["(26/11/07 18:30) net_new", "(26/11/01 8:00) net_old"];

#[test]
fn loadsavednetworkgame_lists_the_multiplayer_folder() {
    // The frame `LAN3` (code 5) selects the network folder; `LOADSAVEDNETWORKGAME` shares `FUN_004D6100`, `dir` code 0
    // returns `int = 0` there, the list comes from `savegame/multiplayer`.
    let mut l = logic(fake_net());
    assert_eq!(l.send("lan3", "lan3", code::QUERY_STATUS).0, 1);
    assert_eq!(l.state.save_dir, SaveDir::Multiplayer);
    let (r, msg) = l.send("loadsavednetworkgame", "dir", code::QUERY_VALUE);
    assert_eq!((r, int_of(&msg, "int")), (1, Some(0)));
    assert_eq!(l.items("loadsavednetworkgame", "dir"), NET_SHOWN);
    assert_eq!(l.state.save_entries, ["net_new", "net_old"]);
    // No loading by double click.
    l.take_queue();
    let (r, _) = l.send("loadsavednetworkgame", "dir", code::DOUBLE_CLICK);
    assert_eq!(r, 1);
    assert!(l.take_queue().is_empty());
    // `SAVEGAME`/`LOADSAVEDGAME2` report without `int`.
    let (r, msg) = l.send("loadsavedgame2", "dir", code::QUERY_VALUE);
    assert_eq!((r, int_of(&msg, "int")), (1, None));
}

#[test]
fn net_list_preselects_the_newest() {
    // O-144: the silent preselection takes the newest entry into the name buffer (`GameApp+0x177C`), which `{0x12}`
    // reads.
    let mut l = logic(fake_net());
    l.state.save_dir = SaveDir::Multiplayer;
    l.items("loadsavednetworkgame", "dir");
    assert_eq!(l.state.save_name.get(), "net_new");
    assert!(l.state.save_selected);
    assert_eq!(
        l.send_int("loadsavednetworkgame", "dir", code::ITEM_SELECTED, 1)
            .0,
        1
    );
    assert_eq!(l.state.save_name.get(), "net_old");
}

#[test]
fn delete_in_the_net_list_asks_15174_then_0x2a() {
    let saves = fake_net();
    let mut l = logic(saves.clone());
    l.state.save_dir = SaveDir::Multiplayer;
    l.items("loadsavednetworkgame", "dir");
    assert_eq!(l.click("loadsavednetworkgame", "deletefile"), 1);
    assert_eq!(l.take_queue(), [open("infoframe", true)]);
    assert_eq!(l.state.info_text, Some(15174));
    assert_eq!(
        l.state.info_ok,
        Some(AppCmd::DeleteSave("loadsavednetworkgame".into()))
    );
    // In operation (from the network menu): `0x2A` deletes in the current directory, `GameApp+0x177C` is the name.
    let Some(mut d) = live_saves(&saves) else {
        return;
    };
    d.app.set_game(Some(NET));
    assert!(d.app.open_game_menu());
    d.idle();
    d.click("ingameoptions2", "savegame2");
    d.idle();
    d.click("savegame", "deletefile");
    d.idle();
    d.app.take_effects();
    d.click("infoframe", "ok");
    let until = d.now + 2000;
    loop {
        assert!(d.now < until, "no delete effect");
        d.frame();
        let effects = save_effects(d.app.take_effects());
        if effects.is_empty() {
            continue;
        }
        assert_eq!(
            effects,
            [UiEffect::DeleteSave {
                name: "net_new".into(),
                dir: SaveDir::Multiplayer
            }]
        );
        break;
    }
}

#[test]
fn savegame2_lists_the_multiplayer_folder() {
    // `SAVEGAME2` (`0x004D735E`): `FUN_004BFC40(2)`, afterwards `SAVEGAME` shows the network folder; the "exists" check
    // there works without extension.
    let mut l = logic(fake_net());
    l.data.game = Some(NET);
    assert_eq!(l.click("ingameoptions2", "savegame2"), 1);
    assert_eq!(l.state.save_dir, SaveDir::Multiplayer);
    assert_eq!(l.items("savegame", "dir"), NET_SHOWN);
    l.state.save_name.set("net_old");
    l.take_queue();
    assert_eq!(l.click("savegame", "save"), 1);
    assert_eq!(
        l.state.info_text,
        Some(15785),
        "exists in the network folder"
    );
    // There is no "net_old" in the main folder.
    l.state.save_dir = SaveDir::Main;
    l.state.info_text = None;
    l.click("savegame", "save");
    assert_eq!(l.state.info_text, None);
}

#[test]
fn savegame_from_the_game_menu_returns_to_the_main_folder() {
    let mut l = logic(fake_net());
    l.state.save_dir = SaveDir::Multiplayer;
    assert_eq!(l.click(MENU3, "savegame3"), 1);
    assert_eq!(l.state.save_dir, SaveDir::Main);
    l.state.save_dir = SaveDir::Multiplayer;
    assert_eq!(l.click(MENU3, "savegame"), 1);
    assert_eq!(l.state.save_dir, SaveDir::Main);
    l.state.save_dir = SaveDir::Multiplayer;
    assert_eq!(l.click(MENU3, "load"), 1);
    assert_eq!(l.state.save_dir, SaveDir::Main);
    assert_eq!(l.items("ingameloadgame", "dir").len(), 2);
}

#[test]
fn singleplayer_menu_sets_the_main_folder() {
    // `START` -> `SINGLEPLAYERMENU` (`0x004D1764`): `FUN_004BFC40(1)`; likewise `CREATESINGLEPLAYERGAME` code 5.
    let mut l = logic(fake_net());
    l.state.save_dir = SaveDir::Multiplayer;
    assert_eq!(l.click("start", "singleplayermenu"), 1);
    assert_eq!(l.state.save_dir, SaveDir::Main);
    l.state.save_dir = SaveDir::Multiplayer;
    l.send(
        "createsingleplayergame",
        "createsingleplayergame",
        code::QUERY_STATUS,
    );
    assert_eq!(l.state.save_dir, SaveDir::Main);
    l.send("lan3", "lan3", code::QUERY_STATUS);
    assert_eq!(l.state.save_dir, SaveDir::Multiplayer);
}
