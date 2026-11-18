//! Tests of the message bar with a synthetic `messages.ini` (re/game-ui N.7).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use opennml_formats::localisation::{Localisation, parse_localisation};
use opennml_formats::messages::parse_messages;

use super::*;
use crate::assets::{Texts, UiAssets};
use crate::coords::Screen;
use crate::fonts::FontBook;
use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};
use crate::text::TradeCost;

/// Three kinds as in the shipped data (ECONOMY, MILITARY, OTHER) and messages:
/// - 1 ECONOMY with `SOUND 877`, 2 and 3 ECONOMY without sound (sound field 877 inherited, without the bit), 4
///   ECONOMY with `PING`;
/// - 10 MILITARY `SOUNDONCE 872` with `PING`, 11 MILITARY `SOUNDONCE 873`;
/// - 20 OTHER with `SOUNDONCE 874`, 21 OTHER without sound (tribute text);
/// - 30 without `MSGTYPE`, 31 with unknown kind `NAVY`, 40 with missing text.
const INI: &str = "MSGTAG ECONOMY
POSITION 0 400
FONT Info
COLOR E000FF00
/MSGTAG
MSGTAG MILITARY
POSITION 0 500
FONT Info
COLOR E0FF0000
/MSGTAG
MSGTAG OTHER
POSITION 0 600
FONT Info
COLOR E00000FF
/MSGTAG
MSGID 1
MSGTYPE ECONOMY
SOUND 877
TEXT 101
/MSGID
MSGID 2
MSGTYPE ECONOMY
TEXT 102
/MSGID
MSGID 3
MSGTYPE ECONOMY
TEXT 103
/MSGID
MSGID 4
MSGTYPE ECONOMY
PING
TEXT 104
/MSGID
MSGID 10
MSGTYPE MILITARY
SOUNDONCE 872
PING
TEXT 110
/MSGID
MSGID 11
MSGTYPE MILITARY
SOUNDONCE 873
TEXT 111
/MSGID
MSGID 20
MSGTYPE OTHER
SOUNDONCE 874
TEXT 120
/MSGID
MSGID 21
MSGTYPE OTHER
TEXT 121
/MSGID
MSGID 30
TEXT 101
/MSGID
MSGID 31
MSGTYPE NAVY
TEXT 101
/MSGID
MSGID 40
MSGTYPE ECONOMY
TEXT 999
/MSGID
";

/// Tribute text of the shipped data (49 characters including tags).
const TRIBUTE: &str = "[PLAYERNAMEPLID1] zahlt [PLAYERNAMEPLID2] Tribut!";

fn texts() -> BTreeMap<i32, String> {
    [
        (101, "Nahrung"),
        (102, "Wood"),
        (103, "Gold"),
        (104, "Feld"),
        (110, "Angriff!"),
        (111, "Building"),
        (120, "Flagge"),
        (121, TRIBUTE),
    ]
    .into_iter()
    .map(|(id, t)| (id, t.to_string()))
    .collect()
}

fn bar_with(ini: &str) -> MessageBar {
    MessageBar::new(&parse_messages(ini), &texts(), 55)
}

fn bar() -> MessageBar {
    bar_with(INI)
}

/// Flag word with priority `p` and enqueuing.
fn queued(p: u32) -> u32 {
    (p << 24) | bar_flags::QUEUE
}

/// Priority of the messages of a queue (top byte).
fn priorities(bar: &MessageBar, tag: usize) -> Vec<u32> {
    bar.queue(tag).iter().map(|m| m.flags >> 24).collect()
}

// QUIRK O-42, O-210: kept
#[test]
fn enqueue_vectors_follow_n_7_2() {
    let mut b = bar();
    let mut out = Vec::new();
    // [1] + 3 -> [1, 3]; [1, 3] + 2 -> [1, 3, 2].
    assert_eq!(b.show(2, (1, 1), queued(1), &mut out), ShowResult::Shown);
    assert_eq!(b.show(2, (2, 2), queued(3), &mut out), ShowResult::Cached);
    assert_eq!(priorities(&b, 0), [1, 3]);
    assert_eq!(b.show(2, (3, 3), queued(2), &mut out), ShowResult::Cached);
    assert_eq!(priorities(&b, 0), [1, 3, 2]);
    // [0k, 0, 0] + 0 -> [0k, new, 0, 0]: with equal priority directly behind the head.
    let mut b = bar();
    for (i, tile) in [(1, 1), (2, 2), (3, 3), (4, 4)].into_iter().enumerate() {
        let r = b.show(2, tile, queued(0), &mut out);
        assert_eq!(
            r,
            if i == 0 {
                ShowResult::Shown
            } else {
                ShowResult::Cached
            }
        );
    }
    let tiles: Vec<(i32, i32)> = b.queue(0).iter().map(|m| m.tile).collect();
    assert_eq!(tiles, [(1, 1), (4, 4), (3, 3), (2, 2)]);
    // Without 0x10000 for an occupied kind: rejected, no sound (message 1 would have SOUND).
    out.clear();
    assert_eq!(
        b.show(1, (9, 9), 0x0100_0000, &mut out),
        ShowResult::Ignored
    );
    assert!(out.is_empty());
    assert_eq!(b.queue(0).len(), 4);
    // Equal at the anchor (flag word, text, location): rejected.
    assert_eq!(b.show(2, (1, 1), queued(0), &mut out), ShowResult::Ignored);
    // Without 0x10000, but empty kind: appears (return value 1) including sound.
    assert_eq!(
        b.show(10, NO_TILE, 0x0100_0000, &mut out),
        ShowResult::Shown
    );
    assert_eq!(out, [BarEffect::Sound(872)]);
    // Codes of the original.
    assert_eq!(
        [
            ShowResult::NotExist,
            ShowResult::Shown,
            ShowResult::Cached,
            ShowResult::Ignored
        ]
        .map(ShowResult::code),
        [0, 1, 2, 3]
    );
}

// QUIRK O-211: kept
#[test]
fn duplicate_is_checked_only_at_the_anchor() {
    let mut b = bar();
    let mut out = Vec::new();
    // [H, X] with priority 0: the anchor for X is the head H, so X is enqueued a second time.
    b.show(2, (0, 0), queued(0), &mut out);
    b.show(3, (5, 5), queued(0), &mut out);
    assert_eq!(b.show(3, (5, 5), queued(0), &mut out), ShowResult::Cached);
    let ids: Vec<i32> = b.queue(0).iter().map(|m| m.id).collect();
    assert_eq!(ids, [2, 3, 3]);
    // Equal to the head (anchor) -> rejected; different location, different flag word or different text -> enqueued.
    assert_eq!(b.show(2, (0, 0), queued(0), &mut out), ShowResult::Ignored);
    assert_eq!(b.show(2, (0, 1), queued(0), &mut out), ShowResult::Cached);
    assert_eq!(
        b.show(2, (0, 0), queued(0) | 0x0102, &mut out),
        ShowResult::Cached
    );
    assert_eq!(b.show(3, (0, 0), queued(0), &mut out), ShowResult::Cached);
    // Priority 1 behind [0 ...]: anchor is the end; equal to the end -> rejected.
    b.show(2, (7, 7), queued(1), &mut out);
    assert_eq!(b.show(2, (7, 7), queued(1), &mut out), ShowResult::Ignored);
    // The def bits belong to the flag word: message 4 (PING) at the same location is not equal.
    assert_eq!(b.queue(0).back().map(|m| m.flags), Some(queued(1)));
    assert_eq!(b.show(4, (7, 7), queued(1), &mut out), ShowResult::Cached);
    assert_eq!(
        b.queue(0).back().map(|m| m.flags),
        Some(queued(1) | bar_flags::PING)
    );
}

// QUIRK O-210: kept
#[test]
fn long_queue_keeps_the_head_and_order() {
    let mut b = bar();
    let mut out = Vec::new();
    // 50 economy messages of equal priority: no maximum length, the head stays, the newest stands directly behind it
    // (LIFO behind the head); at most one sound (when the head appears).
    for i in 0..50 {
        b.show(1, (i, i), queued(3), &mut out);
    }
    assert_eq!(b.queue(0).len(), 50);
    assert_eq!(b.queue(0)[0].tile, (0, 0));
    let tiles: Vec<i32> = b.queue(0).iter().skip(1).map(|m| m.tile.0).collect();
    assert_eq!(tiles, (1..50).rev().collect::<Vec<_>>());
    assert_eq!(out, [BarEffect::Sound(877)]);
    // Enqueued ones have no duration yet.
    assert!(
        b.queue(0)
            .iter()
            .skip(1)
            .all(|m| m.display_ms == 0 && m.fade_ms == 0)
    );
    // A message of higher priority goes to the end, one of lower priority behind the head.
    b.show(2, (0, 0), queued(4), &mut out);
    b.show(2, (0, 0), queued(2), &mut out);
    assert_eq!(priorities(&b, 0)[..2], [3, 2]);
    assert_eq!(b.queue(0).back().map(|m| m.flags >> 24), Some(4));
    assert_eq!(b.queue(0).len(), 52);
}

#[test]
fn unknown_or_untyped_messages_do_not_exist() {
    let mut b = bar();
    let mut out = Vec::new();
    for id in [5, 30, 31] {
        assert_eq!(
            b.show(id, (1, 1), queued(1), &mut out),
            ShowResult::NotExist,
            "{id}"
        );
    }
    assert!(out.is_empty());
    assert!((0..3).all(|t| b.queue(t).is_empty()));
    // Missing text: the number as text (O-241), duration by its length.
    assert_eq!(b.show(40, NO_TILE, queued(1), &mut out), ShowResult::Shown);
    let head = &b.queue(0)[0];
    assert_eq!((head.text.as_str(), head.display_ms), ("999", 3 * 55));
    // The kinds in file order.
    let names: Vec<&str> = b.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["ECONOMY", "MILITARY", "OTHER"]);
}

// QUIRK O-212: kept
#[test]
fn duration_is_len_times_time_per_char_plus_fade() {
    let mut b = bar();
    let mut out = Vec::new();
    // Tribute (flag word `0x10000 | E << 8 | G`, here 0x10302): 49 characters * 55 ms = 2695 ms display, then 1500 ms.
    assert_eq!(b.show(21, NO_TILE, 0x1_0302, &mut out), ShowResult::Shown);
    let head = &b.queue(2)[0];
    assert_eq!(TRIBUTE.encode_utf16().count(), 49);
    assert_eq!((head.display_ms, head.fade_ms), (2695, FADE_MS));
    assert_eq!(head.players, 0x0302);
    // Tick 5 ms: 539 ticks with alpha 255, then fade-out, removed in the 839th tick.
    let alpha = |b: &MessageBar| {
        b.queue(2)
            .front()
            .map(|h| MessageBar::head_color(&b.tags()[2], h) >> 24)
    };
    for _ in 0..539 {
        b.tick(5, &mut out);
        assert_eq!(alpha(&b), Some(255));
    }
    assert_eq!(b.queue(2)[0].display_ms, 0);
    b.tick(5, &mut out);
    assert_eq!(b.queue(2)[0].fade_ms, 1495);
    assert_eq!(alpha(&b), Some(254));
    for _ in 541..839 {
        b.tick(5, &mut out);
    }
    assert_eq!(b.queue(2)[0].fade_ms, 5, "after 838 ticks");
    b.tick(5, &mut out);
    assert!(b.queue(2).is_empty(), "removed in the 839th tick");
    assert!(out.is_empty(), "message 21 without sound");
}

// QUIRK O-212: kept
#[test]
fn head_is_replaced_without_fade_when_another_waits() {
    let mut b = bar();
    let mut out = Vec::new();
    // "Nahrung" (7 characters, 385 ms) with sound, behind it a second one with sound.
    b.show(1, (1, 1), queued(3), &mut out);
    b.show(1, (2, 2), queued(3), &mut out);
    assert_eq!(out, [BarEffect::Sound(877)]);
    out.clear();
    b.tick(385, &mut out);
    assert_eq!(b.queue(0)[0].display_ms, 0);
    // Display expired, another one waits: in the next tick remainder 0 (one tick with alpha 0) ...
    b.tick(16, &mut out);
    assert_eq!(b.queue(0)[0].fade_ms, 0);
    assert_eq!(b.lines(&NoValues)[0].1 >> 24, 0);
    assert!(out.is_empty());
    // ... after that the next one appears with sound, duration and full remainder.
    b.tick(16, &mut out);
    assert_eq!(b.queue(0).len(), 1);
    let head = &b.queue(0)[0];
    assert_eq!(
        (head.tile, head.display_ms, head.fade_ms),
        ((2, 2), 385, FADE_MS)
    );
    assert_eq!(out, [BarEffect::Sound(877)]);
    // Without carry-over: a display with remainder 1 runs only to below 1 despite a large dt, then the fade-out
    // follows.
    let mut b = bar();
    b.show(1, (1, 1), queued(3), &mut out);
    b.tick(10_000, &mut out);
    assert_eq!(b.queue(0)[0].fade_ms, FADE_MS, "display first");
    b.tick(1_000, &mut out);
    assert_eq!(b.queue(0)[0].fade_ms, 500);
    b.tick(500, &mut out);
    assert!(b.queue(0).is_empty(), "dt = remainder removes");
}

#[test]
fn sound_once_blocks_15_seconds_per_tag() {
    let mut b = bar();
    let mut out = Vec::new();
    // "Angriff!" 8 characters: 440 ms display; three ticks of 1000 ms empty the queue.
    let run_out = |b: &mut MessageBar, out: &mut Vec<BarEffect>| {
        for _ in 0..3 {
            b.tick(1000, out);
        }
    };
    b.show(11, NO_TILE, queued(1), &mut out);
    assert_eq!(out, [BarEffect::Sound(873)]);
    assert_eq!(b.sound_lock(1), SOUND_ONCE_MS);
    run_out(&mut b, &mut out);
    assert!(b.queue(1).is_empty());
    assert_eq!(b.sound_lock(1), 12_000);
    // Locked: appears without sound, the lock is not set anew in the process.
    out.clear();
    assert_eq!(b.show(10, NO_TILE, queued(1), &mut out), ShowResult::Shown);
    assert!(out.is_empty());
    assert_eq!(b.sound_lock(1), 12_000);
    // Other kind: own lock.
    b.show(20, NO_TILE, queued(1), &mut out);
    assert_eq!(out, [BarEffect::Sound(874)]);
    out.clear();
    run_out(&mut b, &mut out);
    // After 15 s (lock 0, i.e. < 1) with sound again; the lock does not fall below 0.
    for _ in 0..9 {
        b.tick(1000, &mut out);
    }
    assert_eq!(b.sound_lock(1), 0);
    b.show(11, NO_TILE, queued(1), &mut out);
    assert_eq!(out, [BarEffect::Sound(873)]);
}

// QUIRK O-42, O-219: kept
#[test]
fn ping_only_with_tile_and_pushes_five_latest() {
    let mut b = bar();
    let mut out = Vec::new();
    // PING of the def (message 10) and a location: ping with the color of the kind and 5 repetitions, after the sound.
    b.show(10, (3, 4), queued(1), &mut out);
    assert_eq!(
        out,
        [
            BarEffect::Sound(872),
            BarEffect::Ping {
                tile: (3, 4),
                color: 0xE0FF_0000,
                repeats: PING_REPEATS
            }
        ]
    );
    assert_ne!(b.queue(1)[0].flags & bar_flags::PING, 0);
    // Without a location no ping.
    let mut b = bar();
    out.clear();
    b.show(4, NO_TILE, queued(1), &mut out);
    assert!(out.is_empty());
    // Discard branch (O-42): ping only with the caller bit 0x800 and a location, one repetition, never a sound; the
    // PING of the def does not count there.
    b.show(4, (1, 1), 0x0100_0000, &mut out);
    assert!(out.is_empty());
    b.show(1, (1, 1), 0x0100_0800, &mut out);
    assert_eq!(
        out,
        [BarEffect::Ping {
            tile: (1, 1),
            color: 0xE000_FF00,
            repeats: IGNORED_PING_REPEATS
        }]
    );
    out.clear();
    b.show(1, NO_TILE, 0x0100_0800, &mut out);
    assert!(out.is_empty());
    // Action stack (O-219): the newest in front, at most 5; every removal consumes the newest.
    let mut stack = ActionStack::default();
    for i in 0..7 {
        stack.push((i, -i));
    }
    let tiles: Vec<(i32, i32)> = stack.tiles().iter().copied().collect();
    assert_eq!(tiles, [(6, -6), (5, -5), (4, -4), (3, -3), (2, -2)]);
    assert_eq!(stack.pop(), Some((6, -6)));
    assert_eq!(stack.tiles().len(), 4);
    stack.clear();
    assert_eq!(stack.pop(), None);
}

// QUIRK O-217: kept
#[test]
fn cinemascope_mutes_and_hides_but_keeps_the_clock() {
    let mut b = bar();
    b.cinemascope = true;
    let mut out = Vec::new();
    // No sound (SOUND and SOUNDONCE, no lock), but ping and duration.
    b.show(1, NO_TILE, queued(1), &mut out);
    b.show(10, (2, 3), queued(1), &mut out);
    assert_eq!(
        out,
        [BarEffect::Ping {
            tile: (2, 3),
            color: 0xE0FF_0000,
            repeats: PING_REPEATS
        }]
    );
    assert_eq!(b.sound_lock(1), 0);
    assert_eq!(b.queue(0)[0].display_ms, 7 * 55);
    // Invisible: no lines, no draw commands; time runs anyway.
    assert!(b.lines(&NoValues).is_empty());
    assert!(
        draw(
            &b,
            Screen {
                width: 1024,
                height: 768
            }
        )
        .is_empty()
    );
    b.tick(100, &mut out);
    assert_eq!(b.queue(0)[0].display_ms, 7 * 55 - 100);
    b.cinemascope = false;
    assert_eq!(b.lines(&NoValues).len(), 2);
}

/// Game values without a match: all names empty, all numbers 0.
struct NoValues;

impl GameText for NoValues {
    fn player_name(&self, _: i32) -> String {
        String::new()
    }
    fn trade_cost(&self, _: TradeCost) -> i32 {
        0
    }
    fn guid_count(&self, _: i32, _: i32) -> i32 {
        0
    }
    fn kill_count(&self, _: i32, _: i32) -> i32 {
        0
    }
    fn task_status(&self, _: i32) -> Option<bool> {
        None
    }
    fn countdown_seconds(&self, _: i32) -> i32 {
        0
    }
}

/// Names "P<slot>".
struct Names;

impl GameText for Names {
    fn player_name(&self, slot: i32) -> String {
        format!("P{slot}")
    }
    fn trade_cost(&self, _: TradeCost) -> i32 {
        0
    }
    fn guid_count(&self, _: i32, _: i32) -> i32 {
        0
    }
    fn kill_count(&self, _: i32, _: i32) -> i32 {
        0
    }
    fn task_status(&self, _: i32) -> Option<bool> {
        None
    }
    fn countdown_seconds(&self, _: i32) -> i32 {
        0
    }
}

/// Font `Info`: every character 10 px wide, line height 15.
fn draw_fonts() -> &'static FontBook {
    static FONTS: OnceLock<FontBook> = OnceLock::new();
    FONTS.get_or_init(|| {
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            (
                "data/fonts/Test_15_0_0_0_0.tga",
                tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]),
            ),
        ]);
        let mut assets = UiAssets::new();
        let loc = parse_localisation(
            "Font Info
FontSize 15
TextureFont
FontName Test
",
        );
        FontBook::load(&vfs, &loc, 1024, &mut assets, None)
    })
}

fn draw(b: &MessageBar, screen: Screen) -> Vec<DrawCmd> {
    let vfs = opennml_rda::Vfs::from_archives(Vec::new());
    let mut assets = UiAssets::new();
    let texts = Texts::load(&vfs, &Localisation::default());
    let ctx = UiContext {
        vfs: &vfs,
        assets: &mut assets,
        fonts: draw_fonts(),
        texts: &texts,
        screen,
    };
    let mut out = Vec::new();
    b.draw(&ctx, &Names, &mut out);
    out
}

/// Top-left corner of the text quads (without shadow) in the color `color`: `(x + 2, y)` of the run.
fn text_origin(cmds: &[DrawCmd], color: u32) -> Option<(i32, i32)> {
    let dsts: Vec<[i32; 4]> = cmds
        .iter()
        .filter_map(|cmd| {
            let DrawCmd::Quad { dst, color: c, .. } = *cmd else {
                unreachable!()
            };
            (c == color).then_some(dst)
        })
        .collect();
    Some((
        dsts.iter().map(|d| d[0]).min()?,
        dsts.iter().map(|d| d[1]).min()?,
    ))
}

// QUIRK O-214: kept
#[test]
fn message_position_follows_the_resolution() {
    let mut b = bar();
    let mut out = Vec::new();
    b.show(2, NO_TILE, queued(1), &mut out);
    b.show(11, NO_TILE, queued(1), &mut out);
    b.show(21, NO_TILE, 0x1_0302, &mut out);
    // y = H * POS.y / 1024 (truncating) per kind; x = 0 left-aligned (+2 for the text next to the shadow); during the
    // display alpha 255 instead of E0.
    for (screen, ys) in [
        ((1024, 768), [300, 375, 450]),
        ((1280, 1024), [400, 500, 600]),
        ((800, 600), [234, 292, 351]),
    ] {
        let s = Screen {
            width: screen.0,
            height: screen.1,
        };
        let cmds = draw(&b, s);
        for (color, y) in [0xFF00_FF00, 0xFFFF_0000, 0xFF00_00FF].into_iter().zip(ys) {
            assert_eq!(
                text_origin(&cmds, color),
                Some((2, y)),
                "{screen:?} {color:08X}"
            );
        }
        // Shadow black with the same alpha at (x, y + 2).
        assert!(cmds.iter().any(|c| {
            let DrawCmd::Quad { dst, color, .. } = *c else {
                unreachable!()
            };
            color == 0xFF00_0000 && dst[0] == 0 && dst[1] == ys[0] + 2
        }));
    }
    // Player bytes of the head: PLID1 = 2, PLID2 = 3.
    let lines = b.lines(&Names);
    assert_eq!(lines[2], ("P2 zahlt P3 Tribut!".to_string(), 0xFF00_00FF));
    assert_eq!(lines[0], ("Wood".to_string(), 0xFF00_FF00));
    // Color E000FF00 -> FF00FF00 during the display, at remainder 750 alpha 127.
    b.queues[0][0].fade_ms = 750;
    let s = Screen {
        width: 1024,
        height: 768,
    };
    assert_eq!(text_origin(&draw(&b, s), 0x7F00_FF00), Some((2, 300)));
    // x > 0 indents (W * x / 1280), but wrapping still happens at the full width W: words of 50 px each ("AAAA "),
    // 20 of them = 1000 px > 800 -> second line 15 px lower.
    let ini = INI.replace("POSITION 0 400", "POSITION 100 400");
    let mut texts = texts();
    texts.insert(102, "AAAA ".repeat(20));
    let mut b = MessageBar::new(&parse_messages(&ini), &texts, 55);
    b.show(2, NO_TILE, queued(1), &mut out);
    let s = Screen {
        width: 800,
        height: 600,
    };
    let cmds = draw(&b, s);
    assert_eq!(text_origin(&cmds, 0xFF00_FF00), Some((62 + 2, 234)));
    let rows: std::collections::BTreeSet<i32> = cmds
        .iter()
        .filter_map(|c| {
            let DrawCmd::Quad { dst, color, .. } = *c else {
                unreachable!()
            };
            (color == 0xFF00_FF00).then_some(dst[1])
        })
        .collect();
    assert_eq!(rows.into_iter().collect::<Vec<_>>(), [234, 249]);
}

// QUIRK O-214: kept
#[test]
fn negative_x_aligns_right() {
    // POSITION -5 (the magnitude does not count): x = W - width. "Gold" = 40 px.
    let ini = INI.replace("POSITION 0 400", "POSITION -5 400");
    let mut b = bar_with(&ini);
    let mut out = Vec::new();
    b.show(3, NO_TILE, queued(1), &mut out);
    for (w, h) in [(1024, 768), (800, 600)] {
        let cmds = draw(
            &b,
            Screen {
                width: w,
                height: h,
            },
        );
        assert_eq!(
            text_origin(&cmds, 0xFF00_FF00),
            Some((w - 40 + 2, h * 400 / 1024))
        );
    }
}

// QUIRK O-213: kept
#[test]
fn game_clock_scales_with_game_speed() {
    // Speed 10: 1:1.
    let mut c = GameClock::default();
    assert_eq!([16.0, 17.0, 0.0].map(|dt| c.tick(dt, 10)), [16, 17, 0]);
    // Speed 14, 16 ms: 16 * 24 * 5 = 1920 -> four times 19 (remainder 20, 40, 60, 80), in the fifth frame 20.
    let mut c = GameClock::default();
    let dts: Vec<i32> = (0..5).map(|_| c.tick(16.0, 14)).collect();
    assert_eq!(dts, [19, 19, 19, 19, 20]);
    // Speed 0: half as fast.
    let mut c = GameClock::default();
    assert_eq!([16.0, 16.0, 1.0, 1.0].map(|dt| c.tick(dt, 0)), [8, 8, 0, 1]);
    c.reset();
    assert_eq!(c.tick(1.0, 0), 0);
}
