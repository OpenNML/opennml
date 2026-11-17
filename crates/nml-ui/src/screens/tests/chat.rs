//! Tests of the chat windows: handler `FUN_004CFF80` per code, `showchatwindow` (`FUN_0042EA20`) via
//! `MenuApp::show_chat_window` and live with the corpus dialogs (these need `NML_GAME_DIR`): input up to 79
//! characters, ENTER sends and closes, ESC only closes, empty text does not send.

use super::*;
use crate::dialog::DialogLibrary;
use crate::screens::chat::INPUT_SIZE;

const C1: &str = "chatwindow";
const C2: &str = "chatwindow2";
const C3: &str = "chatwindow3";
const FREE: GameInfo = GameInfo {
    game_type: 0,
    local_slot: 0,
    ui_locked: false,
    net: None,
    playback: false,
};

fn with_key(dialog: &str, key: Option<i32>) -> ElementMsg {
    let mut msg = ElementMsg::new(dialog, "input", code::EDIT_KEY);
    if let Some(k) = key {
        msg.params.insert("key".into(), Param::Int(k));
    }
    msg
}

fn sent(window: u8, text: &str) -> UiEffect {
    UiEffect::SendChat {
        window,
        text: text.into(),
    }
}

/// `FUN_004CFF80`: code 7 hands out the buffer (`size 0x50`, `inputbuffer 1`); ENTER sends to the frame name and
/// closes, ESC only closes, other keys -1, without `key` 0; "Send" (code 8) sends and closes; empty text
/// does not send (`FUN_004B9940`: `wcslen == 0`) but closes; after sending the buffer is empty.
#[test]
fn chat_handler_follows_004cff80() {
    let mut l = Logic::new();
    let (r, msg) = l.send(C1, "input", code::QUERY_TEXT);
    assert_eq!(r, 1);
    assert_eq!(int_of(&msg, "size"), Some(INPUT_SIZE));
    assert_eq!(int_of(&msg, "inputbuffer"), Some(1));
    let Some(Param::Shared(buffer)) = msg.params.get("text").cloned() else {
        panic!("no shared buffer: {:?}", msg.params);
    };
    // The field writes into the shared buffer.
    buffer.set("hello");
    assert_eq!(l.deliver(with_key(C2, Some(i32::from(b'x')))).0, -1);
    assert_eq!(l.deliver(with_key(C2, None)).0, 0);
    assert!(l.take_queue().is_empty());
    assert_eq!(l.deliver(with_key(C2, Some(13))).0, 1);
    assert_eq!(std::mem::take(&mut l.effects), [sent(2, "hello")]);
    assert_eq!(l.take_queue(), [close(C2, false)]);
    assert_eq!(l.state.chat_text.get(), "", "empty after sending");
    // Empty: no sending, still closed.
    assert_eq!(l.deliver(with_key(C1, Some(13))).0, 1);
    assert!(l.effects.is_empty());
    assert_eq!(l.take_queue(), [close(C1, false)]);
    // ESC: only closes, the text stays.
    l.state.chat_text.set("remain");
    assert_eq!(l.deliver(with_key(C3, Some(27))).0, 1);
    assert!(l.effects.is_empty());
    assert_eq!(l.take_queue(), [close(C3, false)]);
    assert_eq!(l.state.chat_text.get(), "remain");
    // "Send" (code 8) from window 3; other codes and elements -1.
    assert_eq!(l.click(C3, "send"), 1);
    assert_eq!(std::mem::take(&mut l.effects), [sent(3, "remain")]);
    assert_eq!(l.take_queue(), [close(C3, false)]);
    assert_eq!(l.send(C3, "send", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send(C1, "input", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send(C1, "title", code::TRIGGERED).0, -1);
}

/// `FUN_0042EA20`: without a running match, with menu state != 0 and in the menu level (game type -2) nothing;
/// otherwise empty the buffer (also for `k` not in 1...3) and `{6, "chatwindow..."}`. No menu state.
#[test]
fn show_chat_window_follows_0042ea20() {
    let mut app = MenuApp::from_library(
        DialogLibrary::default(),
        EngineConfig::default(),
        MenuOptions::default(),
    );
    let queued = |app: &MenuApp| -> Vec<AppCmd> { app.queue().iter().cloned().collect() };
    assert!(!app.show_chat_window(1), "no match running");
    app.set_game(Some(GameInfo {
        game_type: -2,
        ..FREE
    }));
    assert!(!app.show_chat_window(1), "menu level");
    app.set_game(Some(FREE));
    app.screen_state().chat_text.set("old");
    assert!(!app.show_chat_window(4));
    assert_eq!(
        app.screen_state().chat_text.get(),
        "",
        "also emptied for k = 4"
    );
    assert!(queued(&app).is_empty());
    for (k, name) in [(1, C1), (2, C2), (3, C3)] {
        app.screen_state().chat_text.set("old");
        assert!(app.show_chat_window(k));
        assert_eq!(queued(&app).last(), Some(&open(name, false)));
        assert_eq!(app.screen_state().chat_text.get(), "");
        assert_eq!(app.game_menu_state(), 0, "no menu state");
    }
    assert!(app.open_game_menu());
    let before = queued(&app).len();
    assert!(!app.show_chat_window(1), "game menu open");
    assert_eq!(queued(&app).len(), before);
}

/// Live (corpus): ENTER sends at most 79 characters (`size 0x50`) and closes; ESC only closes the window and is
/// consumed (no game menu, `take_untaken_keys` empty); umlauts arrive; click point in the frame.
#[test]
fn chat_window_takes_79_chars_and_escape_closes_only_it() {
    let Some(mut d) = live_with(
        EngineConfig::default(),
        MenuOptions::default(),
        Box::new(dispatch),
    ) else {
        return;
    };
    d.app.set_game(Some(FREE));
    assert!(d.app.show_chat_window(2));
    d.idle();
    assert_eq!(d.app.menu().open_names(), [C2]);
    assert_eq!(d.app.game_menu_state(), 0);
    let rect = d.app.chat_window_rect(S1280).expect("frame drawn");
    // Virtual (30, 732)-(1250, 856) at 1280 x 1024.
    assert_eq!(rect, [30, 732, 1250, 856]);
    for c in "ä"
        .encode_utf16()
        .chain(std::iter::repeat_n(u16::from(b'a'), 84))
    {
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: vec![0x41],
                ch: Some(c),
            },
            S1280,
            now,
        );
    }
    d.frame();
    assert!(
        d.app.take_untaken_keys().is_empty(),
        "the field takes every key"
    );
    assert_eq!(d.app.screen_state().chat_text.units().len(), 79);
    d.key(RETURN);
    d.idle();
    let text = format!("ä{}", "a".repeat(78));
    assert!(d.app.take_effects().contains(&sent(2, &text)));
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.screen_state().chat_text.get(), "");
    assert!(d.app.chat_window_rect(S1280).is_none());
    // ESC: only the window closes, the key is consumed.
    assert!(d.app.show_chat_window(1));
    d.idle();
    d.key(ESCAPE);
    d.frame();
    assert!(
        d.app.take_untaken_keys().is_empty(),
        "ESC does not reach the match"
    );
    d.idle();
    assert!(d.app.menu().open_names().is_empty());
    assert_eq!(d.app.game_menu_state(), 0);
    assert!(
        !d.app
            .take_effects()
            .iter()
            .any(|e| matches!(e, UiEffect::SendChat { .. }))
    );
}
