//! Tests of the speed keys (`FUN_0042F050`) through the flow with a real `GameSession` (corpus and graphics card,
//! otherwise silently skipped): key `+`/`-` from `keys.ini` -> game speed, message 9/10 of the message bar and the
//! setting `GameSpeed`. Pause, cinemascope and the game menu are tested with the fake in `tests.rs`.

use nml_ui::hud::message_bar::{BarMessage, NO_TILE};

use super::gameui_tests::Match;
use crate::game::hotkeys::Hotkeys;
use crate::game::hud::{SPEED_LOWERED_MESSAGE, SPEED_MESSAGE_FLAGS, SPEED_RAISED_MESSAGE};

/// Text "Game speed was increased." or "...slowed down." (`0x0042EF30`).
const TEXT_RAISED: i32 = 17260;
const TEXT_LOWERED: i32 = 17261;

/// Game with the keys of the original, speed `speed` in the match and in the setting, empty message bar.
fn start(speed: u8) -> Option<Match> {
    let mut m = Match::start()?;
    m.flow.set_hotkeys(Hotkeys::load(&m.vfs));
    m.flow.begin_round();
    m.flow.begin_round();
    set_speed(&mut m, speed);
    m.session.as_mut().unwrap().hud.messages.clear();
    Some(m)
}

fn set_speed(m: &mut Match, speed: u8) {
    m.session.as_mut().unwrap().sim_mut().unwrap().game_speed = speed;
    m.host.app.set_game_speed(i32::from(speed));
}

/// A key of the main field as the flow receives it (combination name).
fn key(m: &mut Match, name: &str) -> bool {
    let now = m.now;
    m.with_view(|flow, app, view| flow.key(name, app, view, now))
}

fn speed(m: &Match) -> u8 {
    m.session().sim().unwrap().game_speed
}

fn setting(m: &Match) -> i32 {
    m.host.app.config().game_speed
}

fn clear_bar(m: &mut Match) {
    m.session.as_mut().unwrap().hud.messages.clear();
}

/// Head of message `id` in the bar.
fn head(m: &Match, id: i32) -> Option<BarMessage> {
    m.session()
        .hud
        .messages
        .heads()
        .find(|(_, msg)| msg.id == id)
        .map(|(_, msg)| msg.clone())
}

/// `+` raises the speed 10 -> 11, reports 9 with `0x03010000` (display including fade-out `len * 55 + 1500` ms) and
/// writes the setting `GameSpeed`; `-` undoes it with message 10. The match's message bar gets the message from the
/// host (`Hud::post`), not from the simulation.
#[test]
fn plus_raises_speed_with_message_9_and_setting() {
    let Some(mut m) = start(10) else {
        return;
    };
    assert!(key(&mut m, "+"), "consumed");
    assert_eq!((speed(&m), setting(&m)), (11, 11));
    let msg = head(&m, SPEED_RAISED_MESSAGE).expect("message 9");
    assert_eq!(SPEED_MESSAGE_FLAGS, 0x0301_0000);
    assert_eq!(msg.flags & 0xFFFF_0000, SPEED_MESSAGE_FLAGS);
    assert_eq!(msg.text_id, TEXT_RAISED);
    assert_eq!(msg.tile, NO_TILE);
    let len = msg.text.encode_utf16().count() as i32;
    assert!(len > 0);
    assert_eq!(m.host.app.config().time_per_char, 55);
    assert_eq!(msg.display_ms + msg.fade_ms, len * 55 + 1500);
    assert!(head(&m, SPEED_LOWERED_MESSAGE).is_none());

    clear_bar(&mut m);
    assert!(key(&mut m, "-"));
    assert_eq!((speed(&m), setting(&m)), (10, 10));
    let msg = head(&m, SPEED_LOWERED_MESSAGE).expect("message 10");
    assert_eq!(msg.flags & 0xFFFF_0000, SPEED_MESSAGE_FLAGS);
    assert_eq!(msg.text_id, TEXT_LOWERED);
    assert!(head(&m, SPEED_RAISED_MESSAGE).is_none());
}

/// At the limit (19 or 0) nothing changes: neither speed nor message nor setting (`FUN_0042F050` calls the command
/// only on a change); the key is consumed all the same. 18 + 1 reaches 19 with a message.
#[test]
fn speed_stops_at_19_and_0_without_message() {
    let Some(mut m) = start(18) else {
        return;
    };
    assert!(key(&mut m, "+"));
    assert_eq!((speed(&m), setting(&m)), (19, 19));
    assert!(head(&m, SPEED_RAISED_MESSAGE).is_some());
    // At the limit: the setting stays as the test sets it (is not written).
    clear_bar(&mut m);
    m.host.app.set_game_speed(5);
    assert!(key(&mut m, "+"));
    assert_eq!((speed(&m), setting(&m)), (19, 5));
    assert!(head(&m, SPEED_RAISED_MESSAGE).is_none() && head(&m, SPEED_LOWERED_MESSAGE).is_none());

    set_speed(&mut m, 1);
    clear_bar(&mut m);
    assert!(key(&mut m, "-"));
    assert_eq!((speed(&m), setting(&m)), (0, 0));
    assert!(head(&m, SPEED_LOWERED_MESSAGE).is_some());
    clear_bar(&mut m);
    m.host.app.set_game_speed(5);
    assert!(key(&mut m, "-"));
    assert_eq!((speed(&m), setting(&m)), (0, 5));
    assert!(head(&m, SPEED_RAISED_MESSAGE).is_none() && head(&m, SPEED_LOWERED_MESSAGE).is_none());

    // A loaded speed 20 (slider, old file) clamps the key to 19, in both directions.
    set_speed(&mut m, 20);
    assert!(key(&mut m, "+"));
    assert_eq!(speed(&m), 19);
}

/// O-319: With the game menu open, `+` takes effect (confirmed against the original in a reference run): message 9
/// is at the head at once when the queue is empty; its timer stands still while the menu holds the match
/// (`FUN_00494A00` runs only with the game tick), so the display time remains.
#[test]
fn speed_message_shows_at_once_with_the_game_menu_open_and_waits() {
    let Some(mut m) = start(10) else {
        return;
    };
    m.host.app.open_game_menu();
    m.frame();
    assert_ne!(m.host.app.game_menu_state(), 0, "game menu open");
    assert!(key(&mut m, "+"), "consumed");
    assert_eq!((speed(&m), setting(&m)), (11, 11));
    let msg = head(&m, SPEED_RAISED_MESSAGE).expect("message 9 at the head at once");
    let len = msg.text.encode_utf16().count() as i32;
    assert_eq!(msg.display_ms + msg.fade_ms, len * 55 + 1500);
    for _ in 0..10 {
        m.frame();
    }
    assert_ne!(m.host.app.game_menu_state(), 0, "game menu still open");
    let later = head(&m, SPEED_RAISED_MESSAGE).expect("message 9 is still there");
    assert_eq!(
        (later.display_ms, later.fade_ms),
        (msg.display_ms, msg.fade_ms),
        "timer stands still with the menu open"
    );
}
