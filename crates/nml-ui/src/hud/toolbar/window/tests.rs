use std::collections::BTreeMap;

use nml_formats::gifcon::{Toolbar as Menus, ToolbarButton, button_kind as kinds};
use nml_formats::localisation::Localisation;
use nml_formats::toolbar_cfg::ToolbarIcon;

use super::*;
use crate::assets::{Texts, UiAssets};
use crate::bkg_paint::image_quad;
use crate::draw::Filter;
use crate::fonts::FontBook;
use crate::hud::toolbar::ToolbarSound;
use crate::hud::toolbar::button::ButtonState;
use crate::hud::toolbar::data::Sheet;
use crate::hud::toolbar::model::{HuntTarget, Production, SelectedObject, ToolbarKeys};
use crate::hud::tree::{GameWindows, HudSwitches};
use crate::testutil::{test_vfs, tga_bottom_up};

const SCREEN: Screen = Screen {
    width: 1024,
    height: 768,
};
/// Top edge of the bar at 768 pixel rows.
const BAR_Y: i32 = 768 - 176;
const SELECTION: u32 = 0x500_0000;
const PRODUCTION: u32 = 0x600_0000;
const HEADHUNTER: u32 = 0x1600_0000;
const TARGET_PLAYER: u32 = 0xC00_0000;

/// Positions as in the shipped data (`toolbar.cfg`, cf. `set::tests`); icons: each GUID with sheet 0,
/// tile = GUID mod 50.
fn data(icons: &[u32]) -> ToolbarData {
    let mut ini = String::from("ICONS data/icons/i0.tga\r\n");
    for g in icons {
        ini.push_str(&format!("GUID {g} 0 0 0 {}\r\n", g % 50));
    }
    let mut d = ToolbarData::from_texts("", &ini);
    d.normal = (0..14)
        .map(|i| [245 + 53 * (i % 7), if i < 7 { 17 } else { 71 }])
        .collect();
    d.selection = (0..30)
        .map(|i| [234 + 26 * (i / 2), 128 + 25 * (i % 2)])
        .collect();
    d.stance = (0..5).map(|i| [10 + 30 * i, 120]).collect();
    d.formation = (0..9).map(|i| [169 + 32 * i, 13]).collect();
    d.heroes = (0..5).map(|k| [-55 * (k + 1), -50]).collect();
    d.headhunters = (0..5).map(|k| [-55 * (k + 1), -110]).collect();
    d
}

fn row(slot: i32, msg: i32, flags: u32) -> ToolbarButton {
    ToolbarButton {
        id: slot,
        msg,
        flags,
        ..ToolbarButton::default()
    }
}

fn menus(blocks: &[(i32, Vec<ToolbarButton>)]) -> Menus {
    Menus {
        menus: blocks.iter().cloned().collect(),
        entries: BTreeMap::new(),
    }
}

fn unit(hid: u32, guid: u32) -> SelectedObject {
    SelectedObject {
        hid,
        guid,
        own: true,
        neutral: false,
        building: false,
        site: false,
        has_unit_module: true,
        production: None,
        occupants: Vec::new(),
        stance_bits: 0,
    }
}

fn building(hid: u32, guid: u32) -> SelectedObject {
    SelectedObject {
        building: true,
        has_unit_module: false,
        ..unit(hid, guid)
    }
}

/// Center of normal slot `i` at 1024 x 768.
fn normal_at(i: i32) -> [i32; 2] {
    [
        245 + 53 * (i % 7) + 25,
        BAR_Y + if i < 7 { 17 } else { 71 } + 25,
    ]
}

/// Center of a button.
fn center(b: &Button) -> [i32; 2] {
    let [l, t, r, bo] = b.spec.rect;
    [(l + r) / 2, (t + bo) / 2]
}

/// An event like the dispatch: first slot 3 (`event`), then children and slot 2 (`mouse`).
fn send(bar: &mut Toolbar, m: &ToolbarModel, ev: HudEvent) -> (bool, Vec<ToolbarOut>) {
    let mut out = Vec::new();
    let taken =
        bar.handle(Slot::Event, &ev, m, &mut out) || bar.handle(Slot::Mouse, &ev, m, &mut out);
    (taken, out)
}

fn raw(bar: &mut Toolbar, m: &ToolbarModel, kind: HudEventKind, at: [i32; 2]) -> Vec<ToolbarOut> {
    send(bar, m, HudEvent::with_args(kind, &at)).1
}

/// Movement, press, release at `at` (left or right).
fn click_with(bar: &mut Toolbar, m: &ToolbarModel, at: [i32; 2], right: bool) -> Vec<ToolbarOut> {
    let (down, up) = if right {
        (HudEventKind::RightDown, HudEventKind::RightUp)
    } else {
        (HudEventKind::LeftDown, HudEventKind::LeftUp)
    };
    let mut out = raw(bar, m, HudEventKind::MouseMove, at);
    out.extend(raw(bar, m, down, at));
    out.extend(raw(bar, m, up, at));
    out
}

fn click(bar: &mut Toolbar, m: &ToolbarModel, at: [i32; 2]) -> Vec<ToolbarOut> {
    click_with(bar, m, at, false)
}

/// Double click left like `FUN_004145B0`: 3, 2, 3, 2, `0x13`.
fn double_click(bar: &mut Toolbar, m: &ToolbarModel, at: [i32; 2]) -> Vec<ToolbarOut> {
    let mut out = click(bar, m, at);
    out.extend(raw(bar, m, HudEventKind::LeftDown, at));
    out.extend(raw(bar, m, HudEventKind::LeftUp, at));
    out.extend(raw(bar, m, HudEventKind::DoubleLeft, at));
    out
}

fn ev(kind: HudEventKind, args: &[i32]) -> HudEvent {
    HudEvent::with_args(kind, args)
}

fn intents(out: &[ToolbarOut]) -> Vec<ToolbarIntent> {
    out.iter()
        .filter_map(|o| match o {
            ToolbarOut::Intent(i) => Some(i.clone()),
            _ => None,
        })
        .collect()
}

fn pressed(out: &[ToolbarOut]) -> Vec<i32> {
    out.iter()
        .filter_map(|o| match o {
            ToolbarOut::ButtonPressed(v) => Some(*v),
            _ => None,
        })
        .collect()
}

fn normal_msgs(bar: &Toolbar) -> [Option<i32>; 14] {
    std::array::from_fn(|i| bar.set.normal[i].as_ref().map(|b| b.spec.msg))
}

/// Bar after the full build `{8}`.
fn built(d: ToolbarData, m: &ToolbarModel) -> Toolbar {
    let mut bar = Toolbar::new(d, SCREEN);
    let mut out = Vec::new();
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), m, &mut out);
    assert!(out.is_empty(), "{{8}} without its own sound: {out:?}");
    bar
}

/// Dispatch over the window tree with the bar contents; the minimap takes nothing.
fn route(
    w: &GameWindows,
    bar: &mut Toolbar,
    m: &ToolbarModel,
    e: HudEvent,
) -> (bool, Vec<ToolbarOut>) {
    route_with_map(w, bar, m, e, false)
}

/// Like [`route`]; the minimap takes everything (`map_takes`) or nothing.
fn route_with_map(
    w: &GameWindows,
    bar: &mut Toolbar,
    m: &ToolbarModel,
    e: HudEvent,
    map_takes: bool,
) -> (bool, Vec<ToolbarOut>) {
    let mut out = Vec::new();
    let taken = w.dispatch_with_toolbar(
        4,
        &e,
        &mut |_| map_takes,
        &mut |_| false,
        &mut |slot, e| bar.handle(slot, e, m, &mut out),
        &mut |_, _, _| false,
    );
    (taken, out)
}

// QUIRK O-222: kept
#[test]
fn clicks_outside_the_bar_rect_go_to_the_world() {
    let t = menus(&[(1100, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[300]), &m);
    let w = GameWindows::new(SCREEN, HudSwitches::default(), 26);
    let route_click = |bar: &mut Toolbar, at: [i32; 2]| {
        [
            HudEventKind::MouseMove,
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::DoubleLeft,
            HudEventKind::RightDown,
            HudEventKind::RightUp,
        ]
        .map(|k| route(&w, bar, &m, ev(k, &at)).0)
    };
    // Overhang of the background (128 px above the bar), strip up to the minimap, right edge exclusive.
    for at in [[100, BAR_Y - 50], [100, BAR_Y - 1], [700, 700], [640, 700]] {
        assert_eq!(route_click(&mut bar, at), [false; 6], "{at:?}");
        assert!(!bar.right_down_in_bar);
    }
    // Free bar area: everything taken, a right-click there leads to the base menu.
    assert_eq!(route_click(&mut bar, [100, 700]), [true; 6]);
    assert_eq!(route_click(&mut bar, [0, BAR_Y]), [true; 6]);
    assert_eq!(route_click(&mut bar, [639, 767]), [true; 6]);
}

/// In the 16-px overlap at 800 x 600 the tree order applies. If the minimap takes the event (diamond, rectangle per
/// re/game-ui D5.1), the bar sees nothing; if it does not take it, the bar gets it.
// QUIRK O-222: kept
#[test]
fn overlap_at_800x600_follows_the_tree_order() {
    let small = Screen {
        width: 800,
        height: 600,
    };
    let t = menus(&[
        (1100, vec![row(0, 407, 0)]),
        (407, vec![row(0, 409, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    // A button protrudes into the overlap strip x 624 ... 640 (bar [0, 424, 640, 600], minimap
    // [624, 424, 800, 600]). **Dummy:** the original has no such button (at 800 x 600 the wooden frame of the bar
    // ends at x ~ 627, the right button column at x ~ 613; no bar button lies in the strip, a click there goes to the
    // bar or the minimap, never to the world). The test only proves the tree order of the windows, not a real
    // button.
    let mut d = data(&[407, 409]);
    d.normal[0] = [600, 17];
    let mut bar = Toolbar::new(d.clone(), small);
    bar.event(
        &HudEvent::new(HudEventKind::SelectionChanged),
        &m,
        &mut Vec::new(),
    );
    let w = GameWindows::new(small, HudSwitches::default(), 26);
    let at = [630, 424 + 30];
    let kinds = [
        HudEventKind::MouseMove,
        HudEventKind::LeftDown,
        HudEventKind::LeftUp,
        HudEventKind::DoubleLeft,
        HudEventKind::RightDown,
        HudEventKind::RightUp,
    ];
    // The minimap takes it: the bar sees nothing.
    for k in kinds {
        let (taken, out) = route_with_map(&w, &mut bar, &m, ev(k, &at), true);
        assert!(taken, "{k:?} is taken by the minimap");
        assert!(out.is_empty(), "{k:?}: {out:?}");
    }
    let b = bar.set.normal[0].as_ref().unwrap();
    assert_eq!(b.state, ButtonState::Normal, "no button saw the event");
    assert_eq!(bar.shown_menu(), 1100);
    assert!(!bar.right_down_in_bar);
    // The minimap does not take it (point outside the diamond): the button in the strip takes the click.
    let (taken, out) = route(&w, &mut bar, &m, ev(HudEventKind::MouseMove, &at));
    assert!(taken && out.is_empty());
    assert!(route(&w, &mut bar, &m, ev(HudEventKind::LeftDown, &at)).0);
    let (_, out) = route(&w, &mut bar, &m, ev(HudEventKind::LeftUp, &at));
    assert_eq!(pressed(&out), [407]);
    assert_eq!(bar.shown_menu(), 407);
}

// QUIRK O-231, O-246: kept
#[test]
fn button_pressed_comes_first_even_when_paused_or_locked_path() {
    let t = menus(&[(
        1100,
        vec![
            row(0, 1500, kinds::BUILD),
            row(1, 1011, kinds::BUILD),
            row(2, 399, kinds::DO),
        ],
    )]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.visible_pause = true;
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    // BUILD during the visible pause: the button counts as pressed but has no effect (O-231, O-246).
    let out = click(&mut bar, &m, normal_at(0));
    assert_eq!(
        out,
        [
            ToolbarOut::Sound(ToolbarSound::Click),
            ToolbarOut::ButtonPressed(1500)
        ]
    );
    assert_eq!(bar.cursor, None);
    // Other buttons also work during the pause; ButtonPressed comes before the intent.
    let out = click(&mut bar, &m, normal_at(2));
    assert_eq!(
        out,
        [
            ToolbarOut::Sound(ToolbarSound::Click),
            ToolbarOut::ButtonPressed(399),
            ToolbarOut::Intent(ToolbarIntent::Dispatch {
                msg: 399,
                objects: vec![1]
            })
        ]
    );
    // Without pause: construction cursor or wall cursor; on the right nothing except ButtonPressed.
    m.visible_pause = false;
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.cursor, Some(Cursor::Build(BuildCursor::new(1500))));
    click(&mut bar, &m, normal_at(1));
    assert_eq!(bar.cursor, Some(Cursor::Fort(FortCursor::new(1011))));
    bar.cursor = None;
    let out = click_with(&mut bar, &m, normal_at(0), true);
    assert_eq!(pressed(&out), [1500]);
    assert_eq!(bar.cursor, None);
}

// QUIRK O-246: kept
#[test]
fn locked_button_is_silent_and_keeps_last_button() {
    let mut make = row(0, 1101, kinds::MAKE);
    make.required[0] = 2806;
    let t = menus(&[(1100, vec![make])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1101]), &m);
    raw(&mut bar, &m, HudEventKind::MouseMove, normal_at(0));
    bar.update(&m, &mut None);
    // Locked: taken but silent, without ButtonPressed and without intent.
    for k in [
        HudEventKind::LeftDown,
        HudEventKind::LeftUp,
        HudEventKind::DoubleLeft,
    ] {
        let (taken, out) = send(&mut bar, &m, ev(k, &normal_at(0)));
        assert!(taken, "{k:?}");
        assert!(out.is_empty(), "{k:?}: {out:?}");
    }
    // With an object counter above 0: free again.
    m.stock.insert(2806, 1);
    bar.update(&m, &mut None);
    let out = click(&mut bar, &m, normal_at(0));
    assert_eq!(pressed(&out), [1101]);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::Make {
            guid: 1101,
            count: 1,
            upgrade: false
        }]
    );
}

// QUIRK O-226: kept
#[test]
fn submenus_have_no_stack_and_right_click_returns_to_the_base_menu() {
    let t = menus(&[
        (1100, vec![row(0, 407, 0), row(1, 490, kinds::DO)]),
        (407, vec![row(0, 600, 0), row(13, 409, kinds::DO)]),
        (600, vec![row(13, 417, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.trade_partners = vec![2];
    let mut bar = built(data(&[407, 600, 409, 417, 490, 494, 522]), &m);
    assert_eq!(bar.shown_menu(), 1100);
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.shown_menu(), 407);
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.shown_menu(), 600);
    // 417 in slot 13 does not jump back one level but to the base menu.
    click(&mut bar, &m, normal_at(13));
    assert_eq!(bar.shown_menu(), 1100);
    click(&mut bar, &m, normal_at(0));
    click(&mut bar, &m, normal_at(13));
    assert_eq!(bar.shown_menu(), 1100, "409");
    // Trade menu 490: dynamic buttons 520 + p and 494; 494 to the base menu, 522 base menu and trade.
    click(&mut bar, &m, normal_at(1));
    assert_eq!(normal_msgs(&bar)[0], Some(522));
    assert_eq!(normal_msgs(&bar)[13], Some(494));
    let out = click(&mut bar, &m, normal_at(0));
    assert_eq!(intents(&out), [ToolbarIntent::Trade { kind: 0, slot: 2 }]);
    assert_eq!(bar.shown_menu(), 1100);
    click(&mut bar, &m, normal_at(1));
    click(&mut bar, &m, normal_at(13));
    assert_eq!(normal_msgs(&bar)[0], Some(407), "494");
    // Press and release right on free bar area: base menu, taken (O-226).
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.shown_menu(), 407);
    let free = [100, 700];
    let (taken, _) = send(&mut bar, &m, ev(HudEventKind::MouseMove, &free));
    assert!(taken);
    assert!(send(&mut bar, &m, ev(HudEventKind::RightDown, &free)).0);
    assert!(bar.right_down_in_bar);
    assert!(send(&mut bar, &m, ev(HudEventKind::RightUp, &free)).0);
    assert_eq!(bar.shown_menu(), 1100);
    assert!(!bar.right_down_in_bar);
    // Right press in the world, release in the bar: not taken, the submenu remains.
    click(&mut bar, &m, normal_at(0));
    send(&mut bar, &m, ev(HudEventKind::MouseMove, &[100, 100]));
    assert!(!send(&mut bar, &m, ev(HudEventKind::RightDown, &[100, 100])).0);
    send(&mut bar, &m, ev(HudEventKind::MouseMove, &free));
    assert!(!send(&mut bar, &m, ev(HudEventKind::RightUp, &free)).0);
    assert_eq!(bar.shown_menu(), 407);
    // Right press in the bar, release in the world: not taken, flag gone.
    send(&mut bar, &m, ev(HudEventKind::RightDown, &free));
    send(&mut bar, &m, ev(HudEventKind::MouseMove, &[100, 100]));
    assert!(!send(&mut bar, &m, ev(HudEventKind::RightUp, &[100, 100])).0);
    assert!(!bar.right_down_in_bar);
    assert_eq!(bar.shown_menu(), 407);
}

// QUIRK D-234: kept
#[test]
fn silent_rebuild_keeps_the_shown_menu_and_the_cursor() {
    let t = menus(&[
        (1100, vec![row(2, 413, kinds::TARGET)]),
        (413, vec![row(0, 409, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.targets.insert(413);
    let mut bar = built(data(&[413, 409]), &m);
    let out = click(&mut bar, &m, normal_at(2));
    assert_eq!(pressed(&out), [413]);
    assert_eq!(bar.shown_menu(), 413);
    assert_eq!(bar.cursor, Some(Cursor::Target(TargetCursor::new(413, 0))));
    // `{0x10, 0}`: silent, submenu and cursor remain (D-234).
    let mut out = Vec::new();
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[0]), &m, &mut out));
    assert!(out.is_empty());
    assert_eq!(bar.shown_menu(), 413);
    assert_eq!(normal_msgs(&bar)[0], Some(409));
    assert_eq!(bar.cursor, Some(Cursor::Target(TargetCursor::new(413, 0))));
    // `{0x10, 2}`: full build, cursor gone, selection sound.
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[2]), &m, &mut out));
    assert_eq!(out, [ToolbarOut::Sound(ToolbarSound::Select)]);
    assert_eq!(bar.shown_menu(), 1100);
    assert_eq!(bar.cursor, None);
    // Multi-selection (`+0x5CC = 0`): `{0x10, 0}` becomes a full build.
    m.selection = vec![unit(1, 1100), unit(2, 1100)];
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), &m, &mut out);
    assert_eq!(bar.shown_menu(), 0);
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    out.clear();
    bar.event(&ev(HudEventKind::Rebuild, &[0]), &m, &mut out);
    assert!(out.is_empty(), "no sound for multi-selection");
    assert_eq!(bar.cursor, None);
    assert_eq!(bar.selected, -1);
}

// QUIRK O-275: kept
#[test]
fn full_rebuild_plays_the_select_sound_only_for_single_selection() {
    let t = menus(&[(1100, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    let mut bar = Toolbar::new(data(&[300]), SCREEN);
    let rebuild = |bar: &mut Toolbar, m: &ToolbarModel, e: HudEvent| {
        let mut out = Vec::new();
        bar.event(&e, m, &mut out);
        out
    };
    let select = [ToolbarOut::Sound(ToolbarSound::Select)];
    let full = || ev(HudEventKind::Rebuild, &[2]);
    // Without selection: nothing, `+0x524 = 0`.
    assert!(rebuild(&mut bar, &m, full()).is_empty());
    assert_eq!(bar.selected, 0);
    // Single selection, own: sound (O-236, O-275); `{8}` but without (that one is played by `selection_changed`).
    m.selection = vec![unit(7, 1100)];
    assert_eq!(rebuild(&mut bar, &m, full()), select);
    assert_eq!(bar.selected, 7);
    assert!(rebuild(&mut bar, &m, HudEvent::new(HudEventKind::SelectionChanged)).is_empty());
    // Neutral alone: sound; foreign alone: none.
    m.selection = vec![SelectedObject {
        own: false,
        neutral: true,
        ..unit(7, 1100)
    }];
    assert_eq!(rebuild(&mut bar, &m, full()), select);
    m.selection[0].neutral = false;
    assert!(rebuild(&mut bar, &m, full()).is_empty());
    assert_eq!(bar.selected, 7);
    // Multi-selection: none, `+0x524 = -1`.
    m.selection = vec![unit(7, 1100), unit(8, 1100)];
    assert!(rebuild(&mut bar, &m, full()).is_empty());
    assert_eq!(bar.selected, -1);
}

// QUIRK O-262: kept
#[test]
fn background_draws_unclipped_in_file_order() {
    let vfs = test_vfs(&[
        ("data/icons/a.tga", tga_bottom_up(256, 128, |_, _| [255; 4])),
        ("data/icons/b.tga", tga_bottom_up(256, 64, |_, _| [255; 4])),
    ]);
    let mut assets = UiAssets::new();
    let loc = Localisation::default();
    let texts = Texts::load(&vfs, &loc);
    let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
    let mut d = data(&[]);
    d.sheets = vec![
        Sheet::new("data/icons/a.tga", [128, 128]),
        Sheet::new("data/icons/b.tga", [64, 64]),
    ];
    let icon = |pos: [i32; 2], sheet: i32, tile: i32| ToolbarIcon { pos, sheet, tile };
    d.background = vec![
        icon([-2, -128], 0, 0),
        icon([-2, 126], 1, 3),
        icon([300, 0], 7, 0),
        icon([126, -128], 0, 1),
    ];
    let bar = Toolbar::new(d, SCREEN);
    let mut ctx = UiContext {
        vfs: &vfs,
        assets: &mut assets,
        fonts: &fonts,
        texts: &texts,
        screen: SCREEN,
    };
    let mut got = Vec::new();
    bar.draw_background(&mut ctx, &mut got);
    let a = ctx.assets.find("data/icons/a.tga").unwrap();
    let b = ctx.assets.find("data/icons/b.tga").unwrap();
    // File order, clip = target (no clip, O-262): 128 px above the bar, 14 px below the screen edge; a sheet
    // outside the list is dropped (the original would abort).
    let mut want = Vec::new();
    for (tex, tile, idx, dst) in [
        (a, [128, 128], 0, [-2, BAR_Y - 128, 126, BAR_Y]),
        (b, [64, 64], 3, [-2, BAR_Y + 126, 62, BAR_Y + 190]),
        (a, [128, 128], 1, [126, BAR_Y - 128, 254, BAR_Y]),
    ] {
        image_quad(
            ctx.assets,
            tex,
            tile,
            idx,
            dst,
            dst,
            0xFFFF_FFFF,
            Filter::Point,
            &mut want,
        );
    }
    assert_eq!(got, want);
    assert!(matches!(got[1], DrawCmd::Quad { dst, .. } if dst[3] == 782));
    // Hidden bar: nothing.
    let mut hidden = bar.clone();
    hidden.shown = false;
    let mut none = Vec::new();
    hidden.draw_background(&mut ctx, &mut none);
    assert!(none.is_empty());
}

#[test]
fn make_clicks_give_1_5_and_double_click_4() {
    let t = menus(&[(
        1100,
        vec![row(0, 1101, kinds::MAKE), row(1, 1102, kinds::UPGRADE)],
    )]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![building(1, 1100)];
    let mut bar = built(data(&[1101, 1102]), &m);
    let counts = |out: &[ToolbarOut]| -> Vec<u8> {
        intents(out)
            .iter()
            .map(|i| match i {
                ToolbarIntent::Make {
                    guid: 1101,
                    count,
                    upgrade: false,
                } => *count,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    // Double click 9, 9, `0x27`: 1 + 1 + 2 = 4 (O-224), three click sounds and three times ButtonPressed.
    let out = double_click(&mut bar, &m, normal_at(0));
    assert_eq!(counts(&out), [1, 1, 2]);
    assert_eq!(pressed(&out), [1101; 3]);
    assert_eq!(
        out.iter()
            .filter(|o| **o == ToolbarOut::Sound(ToolbarSound::Click))
            .count(),
        3
    );
    // Right like left.
    assert_eq!(counts(&click_with(&mut bar, &m, normal_at(0), true)), [1]);
    // With Shift (or Ctrl/Alt) 5, 5, 2 = 12; the double click stays 2.
    for keys in [
        ToolbarKeys {
            shift: true,
            ..ToolbarKeys::default()
        },
        ToolbarKeys {
            control: true,
            ..ToolbarKeys::default()
        },
        ToolbarKeys {
            alt: true,
            ..ToolbarKeys::default()
        },
    ] {
        m.keys = keys;
        assert_eq!(counts(&double_click(&mut bar, &m, normal_at(0))), [5, 5, 2]);
    }
    // UPGRADE: once per event, with flag.
    let out = double_click(&mut bar, &m, normal_at(1));
    assert_eq!(
        intents(&out),
        vec![
            ToolbarIntent::Make {
                guid: 1102,
                count: 1,
                upgrade: true
            };
            3
        ]
    );
}

#[test]
fn queue_click_any_way_cancels() {
    let t = menus(&[(1300, vec![])]);
    let mut m = ToolbarModel::new(&t);
    let mut hq = building(5, 1300);
    hq.production = Some(Production {
        running: Some((1101, 0.5)),
        queue: vec![1102, 1103],
    });
    m.selection = vec![hq];
    m.local_slot = 3;
    let mut bar = built(data(&[1101, 1102, 1103]), &m);
    assert_eq!(bar.set.production.len(), 3);
    let queued = center(&bar.set.production[1]);
    let running = center(&bar.set.production[0]);
    let cancel = |index, guid| ToolbarIntent::Cancel {
        index,
        slot: 3,
        guid,
    };
    assert_eq!(intents(&click(&mut bar, &m, queued)), [cancel(1, 1102)]);
    assert_eq!(
        intents(&click_with(&mut bar, &m, queued, true)),
        [cancel(1, 1102)]
    );
    // Double click: three aborts with the same index (O-225).
    let out = double_click(&mut bar, &m, queued);
    assert_eq!(intents(&out), vec![cancel(1, 1102); 3]);
    assert_eq!(pressed(&out), [1102; 3]);
    // Running order: index 0.
    assert_eq!(intents(&click(&mut bar, &m, running)), [cancel(0, 1101)]);
}

// QUIRK O-223: kept
#[test]
fn hidden_bar_takes_only_its_always_events() {
    let t = menus(&[
        (1100, vec![row(0, 300, kinds::DO)]),
        (1200, vec![row(0, 301, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[300, 301]), &m);
    let mut out = Vec::new();
    // `{0x11, w}`: w != 0 hides, w = 0 shows (`FUN_00452F90`).
    assert!(bar.event(&ev(HudEventKind::ToolbarOnOff, &[1]), &m, &mut out));
    assert!(!bar.shown);
    m.selection = vec![unit(2, 1200)];
    assert!(!bar.event(&HudEvent::new(HudEventKind::SelectionChanged), &m, &mut out));
    assert_eq!(bar.shown_menu(), 1100, "{{8}} only with a visible bar");
    assert!(!bar.event(
        &ev(HudEventKind::Button, &[300, kinds::DO as i32]),
        &m,
        &mut out
    ));
    assert!(out.is_empty());
    // Movement inside the rectangle is taken by the bar even when hidden, not on the left; the buttons see nothing.
    assert!(send(&mut bar, &m, ev(HudEventKind::MouseMove, &normal_at(0))).0);
    assert!(!send(&mut bar, &m, ev(HudEventKind::LeftDown, &normal_at(0))).0);
    assert_eq!(
        bar.set.normal[0].as_ref().unwrap().state,
        ButtonState::Normal
    );
    // Slot 3 always runs for `0x10`.
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[2]), &m, &mut out));
    assert_eq!(bar.shown_menu(), 1200);
    assert!(bar.event(&ev(HudEventKind::ToolbarOnOff, &[0]), &m, &mut out));
    assert!(bar.shown);
    // An event 9 from outside takes the click path like a button.
    out.clear();
    assert!(bar.event(
        &ev(HudEventKind::Button, &[301, kinds::DO as i32]),
        &m,
        &mut out
    ));
    assert_eq!(pressed(&out), [301]);
    // `0x28` (right double) never reaches the click path (O-223): only the click sound.
    bar.event(&ev(HudEventKind::Rebuild, &[2]), &m, &mut Vec::new());
    out = raw(&mut bar, &m, HudEventKind::MouseMove, normal_at(0));
    out.extend(raw(&mut bar, &m, HudEventKind::DoubleRight, normal_at(0)));
    assert_eq!(out, [ToolbarOut::Sound(ToolbarSound::Click)]);
}

#[test]
fn update_runs_only_while_shown_and_requests_the_tooltip() {
    let t = menus(&[(1100, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[300]), &m);
    raw(&mut bar, &m, HudEventKind::MouseMove, normal_at(0));
    let mut tip = None;
    bar.update(&m, &mut tip);
    assert_eq!(tip.map(|t| t.guid), Some(300));
    bar.shown = false;
    let mut tip = None;
    bar.update(&m, &mut tip);
    assert_eq!(tip, None);
    assert_eq!(bar.mouse_pos, normal_at(0));
    // Mouse position from every movement, even outside.
    raw(&mut bar, &m, HudEventKind::MouseMove, [5, 5]);
    assert_eq!(bar.mouse_pos, [5, 5]);
}

#[test]
fn hero_rows_change_only_through_events() {
    let t = menus(&[(1100, vec![])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.heroes = vec![(50, 1410)];
    let mut bar = built(data(&[1410]), &m);
    assert_eq!(bar.heroes.buttons().count(), 0);
    let mut out = Vec::new();
    assert!(!bar.event(&ev(HudEventKind::HeroOn, &[1, 50]), &m, &mut out));
    let hero = center(bar.heroes.buttons().next().unwrap());
    assert_eq!(hero, [1024 - 55 + 25, BAR_Y - 50 + 25]);
    // A full build leaves the row standing.
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), &m, &mut out);
    assert_eq!(bar.heroes.buttons().count(), 1);
    let out = click(&mut bar, &m, hero);
    assert_eq!(pressed(&out), [50]);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::Hero {
            hid: 50,
            camera: false
        }]
    );
    m.selection = vec![unit(50, 1410)];
    assert_eq!(
        intents(&click(&mut bar, &m, hero)),
        [ToolbarIntent::Hero {
            hid: 50,
            camera: true
        }]
    );
    // Dead hero (O-271): the button remains, a click has no effect.
    m.heroes.clear();
    let out = click(&mut bar, &m, hero);
    assert_eq!(pressed(&out), [50]);
    assert!(intents(&out).is_empty());
    assert!(!bar.event(&HudEvent::new(HudEventKind::HeroClear), &m, &mut Vec::new()));
    assert_eq!(bar.heroes.buttons().count(), 0);
}

// QUIRK O-246: kept
#[test]
fn headhunter_button_presses_twice_and_leads_to_the_hire() {
    let t = menus(&[(1300, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![building(77, 1300)];
    m.headhunters = vec![4101];
    m.enemy_players = vec![(2, 5)];
    m.hunt_targets.insert(
        2,
        HuntTarget {
            nation: 0,
            stock: [0, 1, 0, 0],
        },
    );
    let mut bar = built(data(&[300, 4101, 475, 409, 1411]), &m);
    bar.event(
        &ev(HudEventKind::HeadhunterOn, &[1, 4101]),
        &m,
        &mut Vec::new(),
    );
    let hunter = center(bar.headhunters.buttons().next().unwrap());
    assert_eq!(
        bar.headhunters.buttons().next().unwrap().spec.flags,
        HEADHUNTER
    );
    // Without own HQ: only ButtonPressed.
    assert_eq!(pressed(&click(&mut bar, &m, hunter)), [4101]);
    assert_eq!(normal_msgs(&bar)[0], Some(300));
    // With HQ: select HQ, then `{9, msg, 0xF000000}` - ButtonPressed twice (O-269), target-player layer of kind 1.
    m.headquarters = Some(77);
    let out = click(&mut bar, &m, hunter);
    assert_eq!(
        out[1..],
        [
            ToolbarOut::ButtonPressed(4101),
            ToolbarOut::Intent(ToolbarIntent::HeadhunterHq { art: 1, hq: 77 }),
            ToolbarOut::ButtonPressed(4101),
        ]
    );
    assert_eq!(normal_msgs(&bar)[0], Some(475));
    assert_eq!(
        bar.set.normal[0].as_ref().unwrap().spec.kind(),
        TARGET_PLAYER
    );
    assert_eq!(normal_msgs(&bar)[13], Some(409));
    // Target player color 5 = slot 2: target-GUID layer with the hero at slot = index.
    assert_eq!(pressed(&click(&mut bar, &m, normal_at(0))), [475]);
    assert_eq!(bar.hunt_target, 2);
    assert_eq!(normal_msgs(&bar)[1], Some(1411));
    let out = click(&mut bar, &m, normal_at(1));
    assert_eq!(pressed(&out), [1411]);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::Hire {
            art: 1,
            target: 2,
            guid: 1411
        }]
    );
    assert_eq!(normal_msgs(&bar)[0], Some(300), "base menu afterwards");
}

// QUIRK O-282: kept
#[test]
fn dynamic_levels_clear_stance_and_selection_but_keep_formation() {
    // `0x186A3` (`0x00457F7C`) clears the list `+0xE0` (`FUN_00453450`: normal **and stance buttons**, `0x00453EF8`,
    // `0x0045460F`) and `+0xFC` (`FUN_00453BF0`: production **and multi-selection**, `0x004542EF`, `0x00454077`);
    // the formation is in `+0x118` (`0x00454739`) and remains. Afterwards `0x186A4` only creates normal buttons.
    // In the original the stance buttons are missing in target-player layer 472.
    let t = menus(&[
        (1100, vec![row(0, 490, kinds::DO), row(1, 4100, kinds::DO)]),
        (1101, vec![]),
    ]);
    let mut m = ToolbarModel::new(&t);
    let mut a = unit(1, 1100);
    a.stance_bits = 8;
    m.selection = vec![a, unit(2, 1101)];
    m.enemy_players = vec![(2, 2)];
    m.trade_partners = vec![3];
    let fresh = |m: &ToolbarModel| built(data(&[1100, 1101, 490, 494, 4100, 472, 409, 523]), m);
    for (slot, back) in [(0, 494), (1, 409)] {
        let mut bar = fresh(&m);
        assert_eq!(bar.set.stance.len(), 5);
        assert_eq!(bar.set.selection.len(), 2);
        let formation = bar.set.formation.clone();
        assert!(!formation.is_empty());
        click(&mut bar, &m, normal_at(slot));
        assert_eq!(normal_msgs(&bar)[13], Some(back), "dynamic layer");
        assert!(bar.set.stance.is_empty(), "stance gone (+0xE0)");
        assert!(bar.set.selection.is_empty(), "multi-selection gone (+0xFC)");
        assert!(bar.set.production.is_empty());
        assert_eq!(bar.set.formation, formation, "formation remains (+0x118)");
        // Back (494 or 409): full build with all rows.
        click(&mut bar, &m, normal_at(13));
        assert_eq!(bar.set.stance.len(), 5);
        assert_eq!(bar.set.selection.len(), 2);
    }
}

// QUIRK O-282: kept
#[test]
fn target_cursor_keeps_stance_and_production() {
    // Target button (TARGET, `FUN_00458130`): submenu of the BUTTONMSG (`FUN_00454120`, `BuildButtonSet(msg, 1)`) and
    // `FUN_00457710` re-creates **all** rows - stance and queue remain, only the normal slots are empty
    // (447 has no menu of its own). In the original a barracks with an open target cursor 447 shows stance and
    // queue.
    let t = menus(&[(
        1300,
        vec![row(0, 1100, kinds::MAKE), row(8, 447, kinds::TARGET)],
    )]);
    let mut m = ToolbarModel::new(&t);
    let mut barracks = building(5, 1300);
    barracks.stance_bits = 8;
    barracks.production = Some(Production {
        running: Some((1100, 0.5)),
        queue: vec![1100],
    });
    m.selection = vec![barracks];
    m.targets.insert(447);
    let mut bar = built(data(&[1100, 1300, 447]), &m);
    assert_eq!(bar.set.production.len(), 2);
    click(&mut bar, &m, normal_at(8));
    assert!(matches!(bar.cursor, Some(Cursor::Target(_))));
    assert_eq!(normal_msgs(&bar), [None; 14]);
    assert_eq!(bar.set.stance.len(), 5);
    assert_eq!(bar.set.production.len(), 2);
}

// QUIRK O-277: kept
#[test]
fn occupant_view_follows_the_full_view_flag() {
    // Building with 415 and only three free slots among 0-6: no automatic view, the flag stays 1.
    let busy = (0..3)
        .map(|i| row(i, 300 + i, kinds::DO))
        .chain([row(3, 415, kinds::DO)])
        .collect();
    let t = menus(&[(1300, busy), (1400, vec![row(3, 415, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    let mut house = building(5, 1300);
    house.occupants = vec![(60, 1101), (61, 1102)];
    m.selection = vec![house.clone()];
    let mut bar = built(data(&[300, 301, 302, 415, 416, 417, 1101, 1102]), &m);
    assert!(bar.occupants_full);
    assert_eq!(normal_msgs(&bar)[3], Some(415));
    // `{0x10, 1}` then opens the full view (O-277).
    let mut out = Vec::new();
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[1]), &m, &mut out));
    assert!(out.is_empty());
    assert_eq!(
        &normal_msgs(&bar)[..2],
        [Some(60), Some(61)],
        "occupants from slot 0"
    );
    assert_eq!(normal_msgs(&bar)[12..], [Some(416), Some(417)]);
    // Click an occupant: unquarter with the HID.
    let mut out = click(&mut bar, &m, normal_at(1));
    assert_eq!(pressed(&out), [61]);
    assert_eq!(intents(&out), [ToolbarIntent::Unquarter(61)]);
    // 417: base menu.
    click(&mut bar, &m, normal_at(13));
    assert_eq!(normal_msgs(&bar)[3], Some(415));
    // Press 415: full view.
    click(&mut bar, &m, normal_at(3));
    assert_eq!(normal_msgs(&bar)[13], Some(417));
    // Automatic view (enough free): the flag drops, `{0x10, 1}` rebuilds fully (with selection sound).
    house.guid = 1400;
    m.selection = vec![house];
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), &m, &mut out);
    assert!(!bar.occupants_full);
    assert_eq!(normal_msgs(&bar)[3], Some(416));
    out.clear();
    bar.event(&ev(HudEventKind::Rebuild, &[1]), &m, &mut out);
    assert_eq!(out, [ToolbarOut::Sound(ToolbarSound::Select)]);
    assert_eq!(normal_msgs(&bar)[13], None, "no 417");
    let out = click(&mut bar, &m, normal_at(3));
    assert_eq!(intents(&out), [ToolbarIntent::UnquarterAll]);
}

// QUIRK O-278: kept
#[test]
fn stance_formation_and_selection_buttons() {
    let t = menus(&[(1100, vec![]), (1300, vec![])]);
    let mut m = ToolbarModel::new(&t);
    let mut a = unit(1, 1100);
    a.stance_bits = 8;
    m.selection = vec![a.clone()];
    let mut bar = built(data(&[1100]), &m);
    let stance = |bar: &Toolbar, i: usize| center(&bar.set.stance[i]);
    let s0 = stance(&bar, 0);
    assert_eq!(
        intents(&click(&mut bar, &m, s0)),
        [ToolbarIntent::Stance(3)]
    );
    // Right and double also work (`0x0044D0F0`, O-278).
    let s2 = stance(&bar, 2);
    assert_eq!(
        intents(&click_with(&mut bar, &m, s2, true)),
        [ToolbarIntent::Stance(1)]
    );
    let s4 = stance(&bar, 4);
    assert_eq!(
        intents(&double_click(&mut bar, &m, s4)),
        vec![ToolbarIntent::Stance(0); 3]
    );
    // 421 on a unit: defense-radius cursor; on a building stance 4.
    let s1 = stance(&bar, 1);
    assert!(intents(&click(&mut bar, &m, s1)).is_empty());
    assert_eq!(bar.cursor, Some(Cursor::Defense(DefenseCursor::default())));
    let mut tower = building(9, 1300);
    tower.stance_bits = 0x10;
    m.selection = vec![tower];
    bar.event(
        &HudEvent::new(HudEventKind::SelectionChanged),
        &m,
        &mut Vec::new(),
    );
    let s1 = stance(&bar, 1);
    assert_eq!(
        intents(&click(&mut bar, &m, s1)),
        [ToolbarIntent::Stance(4)]
    );

    // Formation with multi-selection: 510 expands, 513 selects and collapses, right on 510 scatters.
    let b = unit(2, 1100);
    m.selection = vec![a, b];
    m.formation = 2;
    bar.event(
        &HudEvent::new(HudEventKind::SelectionChanged),
        &m,
        &mut Vec::new(),
    );
    assert_eq!(bar.formation, 2);
    let msgs = |bar: &Toolbar| {
        bar.set
            .formation
            .iter()
            .map(|b| b.spec.msg)
            .collect::<Vec<_>>()
    };
    assert_eq!(msgs(&bar), [510]);
    let f0 = center(&bar.set.formation[0]);
    click(&mut bar, &m, f0);
    assert_eq!(msgs(&bar), [509, 511, 512, 513, 514, 515, 519]);
    let f4 = center(&bar.set.formation[4]);
    let out = click(&mut bar, &m, f4);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::Dispatch {
            msg: 514,
            objects: vec![1, 2]
        }]
    );
    assert_eq!(bar.formation, 3);
    assert_eq!(msgs(&bar), [510]);
    assert_eq!(bar.set.formation[0].spec.icon.1, 9 + 3);
    assert_eq!(
        intents(&click_with(&mut bar, &m, f0, true)),
        [ToolbarIntent::Scatter]
    );

    // Multi-selection: left/double to the front, right alone, deselect with KD_ADD_TO_GROUP.
    let sel = center(&bar.set.selection[1]);
    assert_eq!(bar.set.selection[1].spec.kind(), SELECTION);
    let out = click(&mut bar, &m, sel);
    assert_eq!(pressed(&out), [2]);
    assert_eq!(intents(&out), [ToolbarIntent::ToFront(2)]);
    assert_eq!(
        intents(&click_with(&mut bar, &m, sel, true)),
        [ToolbarIntent::SelectOnly(2)]
    );
    m.keys.add_to_group = true;
    assert_eq!(
        intents(&click_with(&mut bar, &m, sel, true)),
        [ToolbarIntent::Deselect(2)]
    );
}

#[test]
fn group_and_list_or_whole_selection_and_counter_upgrades() {
    let t = menus(&[(
        1100,
        vec![
            row(0, 431, kinds::DO | kinds::GROUP_AND),
            row(1, 3001, kinds::COUNTERUPGRADE),
        ],
    )]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100), unit(2, 1200)];
    let mut bar = built(data(&[431, 3001]), &m);
    // No match under the BUTTONMSG (O-272): whole selection.
    assert_eq!(
        intents(&click(&mut bar, &m, normal_at(0))),
        [ToolbarIntent::Dispatch {
            msg: 431,
            objects: vec![1, 2]
        }]
    );
    // Match when the message happens to be a type: only the list.
    bar.set.groups.insert(431, vec![2]);
    assert_eq!(
        intents(&click(&mut bar, &m, normal_at(0))),
        [ToolbarIntent::Dispatch {
            msg: 431,
            objects: vec![2]
        }]
    );
    // Counter-upgrade to the head of the list (`FUN_0044B770`).
    assert_eq!(
        intents(&click(&mut bar, &m, normal_at(1))),
        [ToolbarIntent::CounterUpgrade {
            guid: 3001,
            source: 1
        }]
    );
}

#[test]
fn unknown_do_messages_open_a_submenu_only_on_the_left() {
    let t = menus(&[
        (1100, vec![row(0, 578, kinds::DO)]),
        (578, vec![row(5, 300, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[578, 300]), &m);
    // 578 has no branch in the dispatcher: right nothing, left the submenu 578.
    let out = click_with(&mut bar, &m, normal_at(0), true);
    assert_eq!(pressed(&out), [578]);
    assert!(intents(&out).is_empty());
    assert_eq!(bar.shown_menu(), 1100);
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.shown_menu(), 578);
    assert_eq!(normal_msgs(&bar)[5], Some(300));
}

// QUIRK O-34: kept
#[test]
fn resize_moves_buttons_with_the_bar() {
    let t = menus(&[(1100, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.heroes = vec![(50, 1410)];
    let mut bar = built(data(&[300, 1410]), &m);
    bar.event(&ev(HudEventKind::HeroOn, &[1, 50]), &m, &mut Vec::new());
    let before = bar.set.normal[0].as_ref().unwrap().spec.rect;
    let hero = bar.heroes.buttons().next().unwrap().spec.rect;
    let small = Screen {
        width: 1024,
        height: 600,
    };
    bar.resize(small);
    assert_eq!(bar.rect(), [0, 424, 640, 600]);
    let after = bar.set.normal[0].as_ref().unwrap().spec.rect;
    assert_eq!(
        after,
        [before[0], before[1] - 168, before[2], before[3] - 168]
    );
    let moved = bar.heroes.buttons().next().unwrap().spec.rect;
    assert_eq!(moved, [hero[0], hero[1] - 168, hero[2], hero[3] - 168]);
    // Same size: nothing.
    bar.resize(small);
    assert_eq!(bar.set.normal[0].as_ref().unwrap().spec.rect, after);
}

#[test]
fn production_only_rebuild_and_hover_event() {
    let t = menus(&[(1300, vec![row(0, 300, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    let mut hq = building(5, 1300);
    hq.production = Some(Production {
        running: None,
        queue: vec![],
    });
    m.selection = vec![hq.clone()];
    let mut bar = built(data(&[300, 1101]), &m);
    assert!(bar.set.production.is_empty());
    hq.production = Some(Production {
        running: Some((1101, 0.0)),
        queue: vec![1101],
    });
    m.selection = vec![hq];
    bar.set.normal[0] = None;
    let mut out = Vec::new();
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[3]), &m, &mut out));
    assert_eq!(bar.set.production.len(), 2);
    assert!(bar.set.production[1].spec.flags == PRODUCTION | 1);
    assert!(bar.set.normal[0].is_none(), "production row only");
    // `{0x10, n >= 4}`: nothing, taken.
    assert!(bar.event(&ev(HudEventKind::Rebuild, &[4]), &m, &mut out));
    // `0x29` only with no selection or a multi-selection.
    assert!(!bar.event(&ev(HudEventKind::Hover, &[33]), &m, &mut out));
    assert_eq!(bar.hover, 0);
    m.selection.clear();
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), &m, &mut out);
    bar.event(&ev(HudEventKind::Hover, &[33]), &m, &mut out);
    assert_eq!(bar.hover, 33);
    // `0x0A` and `0x19` close the cursor; `0x19` with w != 0 rebuilds without selection and does not take.
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    assert!(bar.event(&HudEvent::new(HudEventKind::CursorsOff), &m, &mut out));
    assert_eq!(bar.cursor, None);
    let t2 = menus(&[(1100, vec![row(0, 300, kinds::DO)])]);
    let mut m2 = ToolbarModel::new(&t2);
    m2.selection = vec![unit(1, 1100)];
    bar.event(
        &HudEvent::new(HudEventKind::SelectionChanged),
        &m2,
        &mut out,
    );
    bar.cursor = Some(Cursor::Target(TargetCursor::new(413, 0)));
    assert!(!bar.event(&ev(HudEventKind::Lock, &[1]), &m2, &mut out));
    assert_eq!(bar.cursor, None);
    assert_eq!(normal_msgs(&bar)[0], None);
    assert_eq!(bar.selected, 0);
    // `0x2F`/`0x30` close the defense cursor, even when hidden.
    bar.shown = false;
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    assert!(bar.event(&ev(HudEventKind::DefenseCancel, &[]), &m2, &mut out));
    assert_eq!(bar.cursor, None);
}

/// O-303 (`FUN_0044B110`, `0x00457980`, `Toolbar_BuildButtonSet` `0x00455824`): the type comes from the
/// **first** own object, the selection GUID from the **last** set build (multi-selection: last object; submenu: its
/// GUID). A foreign first object leaves the type as it is, and alone also the selection; empty -> type 1.
// QUIRK O-303: kept
#[test]
fn key_context_is_the_last_object_and_ignores_foreign_ones() {
    let t = menus(&[
        (1100, vec![row(0, 407, 0)]),
        (1200, vec![row(1, 490, kinds::DO)]),
        (407, vec![row(0, 600, 0)]),
    ]);
    let ctx = |kind, selection| KeyContext {
        kind,
        nation: 4,
        selection,
    };
    let mut bar = Toolbar::new(data(&[407, 490, 600]), SCREEN);
    assert_eq!(bar.key_context(4), ctx(1, 0), "before the first set build");
    let mut m = ToolbarModel::new(&t);
    let rebuild = |bar: &mut Toolbar, m: &ToolbarModel| {
        send(bar, m, ev(HudEventKind::SelectionChanged, &[]));
    };
    // Unit, then building: type of the unit, GUID of the building.
    m.selection = vec![unit(1, 1100), building(2, 1200)];
    rebuild(&mut bar, &m);
    assert_eq!(bar.key_context(4), ctx(2, 1200));
    // Building alone: type 3.
    m.selection = vec![building(2, 1200)];
    rebuild(&mut bar, &m);
    assert_eq!(bar.key_context(4), ctx(3, 1200));
    // Unit alone, then its submenu 407: selection = 407, type remains.
    m.selection = vec![unit(1, 1100)];
    rebuild(&mut bar, &m);
    assert_eq!(bar.key_context(4), ctx(2, 1100));
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.shown_menu(), 407);
    assert_eq!(bar.key_context(4), ctx(2, 407));
    // Silent rebuild `{0x10, 0}` of the shown menu: same GUID.
    send(&mut bar, &m, ev(HudEventKind::Rebuild, &[0]));
    assert_eq!(bar.key_context(4), ctx(2, 407));
    // A foreign object alone: no `FUN_0044B110`, no set build - the old context remains.
    let mut enemy = unit(3, 1500);
    enemy.own = false;
    m.selection = vec![enemy.clone()];
    rebuild(&mut bar, &m);
    assert_eq!(bar.key_context(4), ctx(2, 407));
    // Foreign first in a multi-selection: the type remains, the further set builds set the GUID.
    m.selection = vec![enemy, building(4, 1200)];
    rebuild(&mut bar, &m);
    assert_eq!(bar.key_context(4), ctx(2, 1200));
    // Button 415 (occupants) in the dispatcher: selection = 415 (`0x0044C1FD`, `0x0049CF60(0x19F)`), the type remains;
    // the full occupant view builds no set. `{0x10, 1}` does not go through the dispatcher and changes nothing.
    let busy = (0..3)
        .map(|i| row(i, 300 + i, kinds::DO))
        .chain([row(3, 415, kinds::DO)])
        .collect();
    let t2 = menus(&[(1300, busy)]);
    let mut m2 = ToolbarModel::new(&t2);
    let mut house = building(5, 1300);
    house.occupants = vec![(60, 1101), (61, 1102)];
    m2.selection = vec![house];
    let mut bar2 = built(data(&[300, 301, 302, 415, 416, 417, 1101, 1102]), &m2);
    assert_eq!(bar2.key_context(4), ctx(3, 1300));
    send(&mut bar2, &m2, ev(HudEventKind::Rebuild, &[1]));
    assert_eq!(
        bar2.key_context(4),
        ctx(3, 1300),
        "{{0x10, 1}} changes nothing"
    );
    click(&mut bar2, &m2, normal_at(13));
    assert_eq!(normal_msgs(&bar2)[3], Some(415));
    assert_eq!(bar2.key_context(4), ctx(3, 1300), "417: full build");
    click(&mut bar2, &m2, normal_at(3));
    assert_eq!(normal_msgs(&bar2)[13], Some(417), "full occupant view");
    assert_eq!(bar2.key_context(4), ctx(3, 415));
    // If 415 falls back to the full build (flag "full occupant view" off), that overwrites the GUID.
    click(&mut bar2, &m2, normal_at(13));
    bar2.occupants_full = false;
    let mut out = Vec::new();
    bar2.press(
        ToolbarEvent::Shortcut {
            msg: 415,
            kind: kinds::DO,
        },
        &m2,
        &mut out,
    );
    assert_eq!(bar2.key_context(4), ctx(3, 1300), "fallback: full build");
    // Empty: type 1, selection 0; the caller supplies the nation.
    m.selection.clear();
    rebuild(&mut bar, &m);
    assert_eq!(
        bar.key_context(2),
        KeyContext {
            kind: 1,
            nation: 2,
            selection: 0
        }
    );
}

/// The minimap GUIDs 570...577, 579, 580 go to the host as one intent, for every click kind of the kind `0x13000000`
/// (`FUN_0044CF50` -> dispatcher); `double` only for a double click, never for a key (`xbutton`, event 9). The
/// dispatcher does not know 578 and 581 (no branch, re/game-ui N.5.4).
// QUIRK O-219: kept
#[test]
fn minimap_guids_go_to_the_host_with_the_double_click() {
    let t = menus(&[]);
    let m = ToolbarModel::new(&t);
    let mut bar = built(data(&[]), &m);
    let minimap = |msg, double| [ToolbarIntent::Minimap { msg, double }];
    for (ev, double) in [
        (ButtonEvent::Left, false),
        (ButtonEvent::Right, false),
        (ButtonEvent::Double, true),
    ] {
        let mut out = Vec::new();
        let click = ToolbarEvent::Click {
            msg: 576,
            kind: KIND_MINIMAP,
            object: None,
            ev,
        };
        bar.press(click, &m, &mut out);
        assert_eq!(pressed(&out), [576], "{ev:?}");
        assert_eq!(intents(&out), minimap(576, double), "{ev:?}");
    }
    for msg in (570..=577).chain([579, 580]) {
        let mut out = Vec::new();
        let key = ToolbarEvent::Shortcut {
            msg,
            kind: kinds::DO,
        };
        bar.press(key, &m, &mut out);
        assert_eq!(intents(&out), minimap(msg, false), "key {msg}");
    }
    for msg in [578, 581] {
        let mut out = Vec::new();
        let key = ToolbarEvent::Shortcut {
            msg,
            kind: kinds::DO,
        };
        bar.press(key, &m, &mut out);
        assert_eq!(pressed(&out), [msg]);
        assert!(intents(&out).is_empty(), "{msg} without branch");
    }
}
