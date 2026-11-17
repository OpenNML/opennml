//! Tests of the chat windows in the flow: `showchatwindow(k)` through the bindings (ENTER, Shift+ENTER, Ctrl+ENTER),
//! sending as `0x5D` with the recipients of the window, ESC closes only the window, locks (cinemascope, open menu) and
//! the rectangle rule for clicks. With the fake `FakeSession` over the corpus dialogs (`NML_GAME_DIR`, otherwise
//! silently skipped).

use nml_sim::command::{Command as SimCommand, CommandKind};

use super::tests::{Run, S1024};
use super::*;

/// Free play on the test world of the fake, up to the first step.
fn started() -> Option<Run> {
    let mut r = Run::corpus()?;
    r.start();
    r.until_step();
    Some(r)
}

/// Type text (one key event with the character per character, like `ui_key` with `char`).
fn type_text(r: &mut Run, text: &str) {
    for c in text.encode_utf16() {
        r.ui(nml_control::Command::UiKey {
            chord: "A".into(),
            char: Some(c),
        });
    }
}

/// The chat commands in the command log and in the queue of the match.
fn chats(r: &Run) -> Vec<SimCommand> {
    let sim = r.view.sim().unwrap();
    sim.log
        .iter()
        .map(|c| &c.command)
        .chain(sim.pending_commands())
        .filter(|c| matches!(c.kind, CommandKind::Chat { .. }))
        .cloned()
        .collect()
}

fn chat(mask: u32, text: &str) -> SimCommand {
    SimCommand {
        player: 0,
        kind: CommandKind::Chat {
            player: 0,
            mask,
            text: text.encode_utf16().collect(),
        },
    }
}

/// Shift+ENTER opens `CHATWINDOW2` (the game keeps running, menu state 0), "hello" and ENTER send `0x5D` to the
/// friends of the sender including the sender itself - in single player only slot 0, mask `0x01` - and close the
/// window; ENTER opens window 1 (all), Ctrl+ENTER window 3 (chat mask). `GameSession` shows the line "Name: hello"
/// (`chat_line_name_text_8_s`).
// QUIRK O-325: kept
#[test]
fn shift_return_chats_to_friends_d6() {
    let Some(mut r) = started() else {
        return;
    };
    r.key("SHIFT RETURN");
    r.idle();
    assert_eq!(r.dialogs(), ["chatwindow2"]);
    assert_eq!(r.state(), 0, "no menu state");
    // The game keeps running (otherwise panic "no step").
    r.until_step();
    type_text(&mut r, "hello");
    r.key("RETURN");
    r.idle();
    assert!(r.dialogs().is_empty(), "closed after sending");
    assert_eq!(chats(&r), [chat(0x01, "hello")]);
    // ENTER: window 1, all; Ctrl+ENTER: window 3, chat mask.
    r.key("RETURN");
    r.idle();
    assert_eq!(r.dialogs(), ["chatwindow"]);
    type_text(&mut r, "to all");
    r.click("chatwindow", "send");
    r.idle();
    r.view.masks.chat[0] = 0b101;
    r.key("CONTROL RETURN");
    r.idle();
    assert_eq!(r.dialogs(), ["chatwindow3"]);
    type_text(&mut r, "mask");
    r.key("RETURN");
    r.idle();
    assert_eq!(
        chats(&r),
        [
            chat(0x01, "hello"),
            chat(0xFFFF_FFFF, "to all"),
            chat(0b101, "mask")
        ]
    );
    // Empty text: no command, the window closes anyway.
    r.key("RETURN");
    r.idle();
    r.key("RETURN");
    r.idle();
    assert!(r.dialogs().is_empty());
    assert_eq!(chats(&r).len(), 3);
}

/// `FUN_0042EA20`: in cinemascope and with an open menu (game menu, diplomacy) ENTER opens no chat window; the
/// binding is taken anyway.
// QUIRK O-291, O-325: kept
#[test]
fn no_chat_window_in_cinemascope_or_with_menu_open() {
    let Some(mut r) = started() else {
        return;
    };
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = true;
    let now = r.now;
    assert!(
        r.flow.key("RETURN", &mut r.host.app, &mut r.view, now),
        "binding taken"
    );
    r.idle();
    assert!(r.dialogs().is_empty(), "Cinemascope");
    r.view.sim_mut().unwrap().state.script.mission.cinemascope = false;
    // Diplomacy open (state 10).
    assert!(r.host.app.toggle_diplomacy(true));
    r.idle();
    let now = r.now;
    r.flow.key("RETURN", &mut r.host.app, &mut r.view, now);
    r.idle();
    assert_eq!(r.dialogs(), ["diplomatie"]);
    r.host.app.toggle_diplomacy(false);
    r.idle();
    // Game menu open (state 1).
    assert!(r.host.app.open_game_menu());
    r.idle();
    let now = r.now;
    r.flow.key("RETURN", &mut r.host.app, &mut r.view, now);
    r.idle();
    assert_eq!(r.dialogs(), ["ingameoptions3"]);
}

/// ESC with an open chat window closes only the window - no game menu, menu state 0, the game keeps running; the typed
/// text does not go out. The next ESC opens the game menu again.
// QUIRK O-325, O-393: kept
#[test]
fn escape_closes_only_the_chat_window_t13() {
    let Some(mut r) = started() else {
        return;
    };
    assert!(!r.host.app.text_focus(), "no focus without a window");
    r.key("RETURN");
    r.idle();
    // The open chat window is an input field with focus (O-393).
    assert!(r.host.app.text_focus());
    type_text(&mut r, "away");
    r.key("ESCAPE");
    r.idle();
    assert!(r.dialogs().is_empty());
    assert!(!r.host.app.text_focus(), "free again after ESC");
    assert_eq!(r.state(), 0, "no game menu");
    // The game runs (otherwise panic "no step").
    r.until_step();
    assert!(chats(&r).is_empty());
    r.key("ESCAPE");
    // The game menu opens in the next frame with a tick (O-152).
    r.until_step();
    r.idle();
    assert_eq!(
        r.state(),
        1,
        "without a chat window ESC opens the game menu"
    );
}

/// Hypothesis: A mouse button press within the frame of the open chat window does not go to the match, nor does the
/// matching release; outside and without a window as before.
// QUIRK O-325: kept
#[test]
fn clicks_on_the_chat_window_stay_out_of_the_match() {
    let Some(mut r) = started() else {
        return;
    };
    let press = |r: &mut Run, x: f64, y: f64| {
        for ev in [
            InputEvent::CursorMoved { x, y },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            r.flow
                .input(&ev, S1024, r.now, &mut r.host.app, &mut r.view);
        }
    };
    let buttons = |r: &mut Run| {
        std::mem::take(&mut r.view.inputs)
            .into_iter()
            .filter(|e| matches!(e, InputEvent::Button { .. }))
            .count()
    };
    press(&mut r, 512.0, 700.0);
    assert_eq!(buttons(&mut r), 2, "without a window to the match");
    r.key("RETURN");
    r.idle();
    let rect = r.host.app.chat_window_rect(S1024).expect("window drawn");
    // Virtual (30, 732)-(1250, 856) at 1024 x 768.
    assert_eq!(rect, [24, 549, 1000, 642]);
    press(&mut r, 512.0, 600.0);
    assert_eq!(buttons(&mut r), 0, "in the window");
    press(&mut r, 512.0, 300.0);
    assert_eq!(buttons(&mut r), 2, "outside");
    // Press in the window, release outside: both stay away.
    for ev in [
        InputEvent::CursorMoved { x: 30.0, y: 560.0 },
        InputEvent::Button {
            button: MouseButton::Right,
            pressed: true,
        },
        InputEvent::CursorMoved { x: 30.0, y: 100.0 },
        InputEvent::Button {
            button: MouseButton::Right,
            pressed: false,
        },
    ] {
        r.flow
            .input(&ev, S1024, r.now, &mut r.host.app, &mut r.view);
    }
    assert_eq!(buttons(&mut r), 0);
}

/// Pointer position and swallowed presses of the rectangle rule belong to the match. A press in the chat window before
/// the change of match does not swallow the release in the new match.
#[test]
fn chat_window_swallow_state_ends_with_the_match() {
    let Some(mut r) = started() else {
        return;
    };
    r.key("RETURN");
    r.idle();
    assert!(r.host.app.chat_window_rect(S1024).is_some(), "window open");
    for ev in [
        InputEvent::CursorMoved { x: 512.0, y: 600.0 },
        InputEvent::Button {
            button: MouseButton::Right,
            pressed: true,
        },
    ] {
        r.flow
            .input(&ev, S1024, r.now, &mut r.host.app, &mut r.view);
    }
    assert_eq!(r.flow.swallowed, [false, true], "press in the window");
    r.start();
    assert_eq!(r.flow.swallowed, [false; 2]);
    assert_eq!(r.flow.cursor, None);
    r.view.inputs.clear();
    r.flow.input(
        &InputEvent::Button {
            button: MouseButton::Right,
            pressed: false,
        },
        S1024,
        r.now,
        &mut r.host.app,
        &mut r.view,
    );
    assert!(
        r.view
            .inputs
            .iter()
            .any(|e| matches!(e, InputEvent::Button { pressed: false, .. })),
        "the release goes to the new match"
    );
}

/// The input field takes at most 79 characters (`size 0x50`); umlauts and `[...]` tags go unchanged into the command.
/// A cheat in capital letters is a chat line, `goldenmonkey` with one character `U+0167` (low byte `g`) is a cheat
/// (the rest is tested in `command::chat` and `hud::screen_text`).
// QUIRK O-390: kept
#[test]
fn chat_text_limits_tags_and_low_byte_cheats() {
    let Some(mut r) = started() else {
        return;
    };
    let gold = |r: &Run| r.view.sim().unwrap().state.players[0].resources_fx[0] >> 12;
    let start = gold(&r);
    r.key("RETURN");
    r.idle();
    let long = format!("[COLOR 0xFF00FF00]grün{}", "x".repeat(100));
    type_text(&mut r, &long);
    r.key("RETURN");
    r.idle();
    let sent: String = long.chars().take(79).collect();
    assert_eq!(sent.encode_utf16().count(), 79);
    assert_eq!(chats(&r), [chat(0xFFFF_FFFF, &sent)]);
    for text in ["GOLDENMONKEY", "\u{0167}oldenmonkey"] {
        r.key("RETURN");
        r.idle();
        type_text(&mut r, text);
        r.key("RETURN");
        r.idle();
    }
    r.idle();
    assert_eq!(chats(&r).len(), 3);
    assert_eq!(gold(&r), start + 1000, "only the cheat with the low byte");
}
