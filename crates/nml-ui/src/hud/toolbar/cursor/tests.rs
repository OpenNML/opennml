use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use nml_formats::gifcon::{Toolbar as Menus, ToolbarButton, button_kind as kinds};

use super::*;
use crate::coords::Screen;
use crate::hud::toolbar::button::ToolbarOut;
use crate::hud::toolbar::data::ToolbarData;
use crate::hud::toolbar::model::{SelectedObject, ToolbarModel};
use crate::hud::toolbar::window::{Toolbar, ToolbarIntent};
use crate::hud::tree::Slot;
use crate::hud::{HudEvent, HudEventKind};

const SCREEN: Screen = Screen {
    width: 1024,
    height: 768,
};
const BAR_Y: i32 = 768 - 176;
/// Free bar area at the top left (no button, no stance or formation button).
const FREE: [i32; 2] = [5, BAR_Y + 5];

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

fn normal_at(i: i32) -> [i32; 2] {
    [
        245 + 53 * (i % 7) + 25,
        BAR_Y + if i < 7 { 17 } else { 71 } + 25,
    ]
}

fn raw(bar: &mut Toolbar, m: &ToolbarModel, kind: HudEventKind, at: [i32; 2]) -> Vec<ToolbarOut> {
    let ev = HudEvent::with_args(kind, &at);
    let mut out = Vec::new();
    let _ = bar.handle(Slot::Event, &ev, m, &mut out) || bar.handle(Slot::Mouse, &ev, m, &mut out);
    out
}

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

fn built(d: ToolbarData, m: &ToolbarModel) -> Toolbar {
    let mut bar = Toolbar::new(d, SCREEN);
    let mut out = Vec::new();
    bar.event(&HudEvent::new(HudEventKind::SelectionChanged), m, &mut out);
    bar
}

/// Frame: frame into the model, then `update` (frame event 6).
fn frame(bar: &mut Toolbar, m: &mut ToolbarModel, f: CursorFrame) {
    m.cursor = Some(f);
    bar.update(m, &mut None);
}

fn at_world(x: f32, z: f32) -> CursorFrame {
    CursorFrame {
        world: Some([x, 7.0, z]),
        valid: true,
        ..CursorFrame::default()
    }
}

/// Menu of a unit with a build button (slot 0), wall button (1) and a dispatcher button 399 (2).
fn build_menus() -> Menus {
    menus(&[(
        1100,
        vec![
            row(0, 1500, kinds::BUILD),
            row(1, 1011, kinds::BUILD),
            row(2, 399, kinds::DO),
        ],
    )])
}

#[test]
fn build_click_over_free_bar_area_places_at_the_last_world_point() {
    // O-235 corrected: no rectangle comparison; the click is placed at the world point of the last frame.
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    assert!(matches!(bar.cursor, Some(Cursor::Build(_))));
    // Invalid: nothing, the cursor stays.
    let mut f = at_world(500.0, 700.0);
    f.valid = false;
    frame(&mut bar, &mut m, f);
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());
    assert!(matches!(bar.cursor, Some(Cursor::Build(_))));
    // Valid: the world point of the last frame (behind the bar), rotation 0.
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    // Shift does not change the closing (no rebuild, all cursors closed).
    m.keys.shift = true;
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceBuilding {
            guid: 1500,
            rotation: 0.0,
            pos: [500.0, 7.0, 700.0]
        }]
    );
    assert!(pressed(&out).is_empty());
    assert_eq!(bar.cursor, None);
    assert_eq!(bar.shown_menu(), 1100, "no rebuild");
    assert!(
        !out.iter().any(|o| matches!(o, ToolbarOut::Sound(_))),
        "no full build, so no selection sound"
    );
}

#[test]
fn dragging_more_than_10_px_rotates_and_blocks_placing() {
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    let cur = |bar: &Toolbar| match &bar.cursor {
        Some(Cursor::Build(b)) => b.clone(),
        c => panic!("no build cursor: {c:?}"),
    };
    let at = |y: i32| CursorFrame {
        mouse: [300, y],
        ..at_world(500.0, 700.0)
    };
    frame(&mut bar, &mut m, at(200));
    raw(&mut bar, &m, HudEventKind::LeftDown, FREE);
    assert!(cur(&bar).dragging);
    // 8 px: no dragging, quarter 8 / 50 = 0.
    frame(&mut bar, &mut m, at(192));
    assert!(!cur(&bar).dragged);
    assert_eq!(cur(&bar).rotation, 0.0);
    // 50 px upward: dragged, one quarter (90 degrees); the release does not place.
    frame(&mut bar, &mut m, at(150));
    assert!(cur(&bar).dragged);
    assert_eq!(cur(&bar).rotation, FRAC_PI_2);
    let out = raw(&mut bar, &m, HudEventKind::LeftUp, FREE);
    assert!(intents(&out).is_empty());
    let b = cur(&bar);
    assert!(!b.dragging && !b.dragged, "release resets both");
    // The rotation stays; a second press starts from it, delta_y = 250 -> (250 / 50) mod 4 = 1 more quarter.
    frame(&mut bar, &mut m, at(200));
    raw(&mut bar, &m, HudEventKind::LeftDown, FREE);
    assert_eq!(cur(&bar).start_rotation, FRAC_PI_2);
    frame(&mut bar, &mut m, at(-50));
    assert_eq!(cur(&bar).rotation, FRAC_PI_2 + FRAC_PI_2);
    // Downward: delta_y = -100 -> -2 quarters (remainder with the sign of the dividend).
    frame(&mut bar, &mut m, at(300));
    assert_eq!(cur(&bar).rotation, -2.0 * FRAC_PI_2 + FRAC_PI_2);
    // Back to under 10 px: the rotation is start + 0 again, but "dragged" stays until the release.
    frame(&mut bar, &mut m, at(195));
    assert!(cur(&bar).dragged);
    raw(&mut bar, &m, HudEventKind::LeftUp, FREE);
    // Without dragging, the release places with the remembered rotation.
    frame(&mut bar, &mut m, at(200));
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceBuilding {
            guid: 1500,
            rotation: FRAC_PI_2,
            pos: [500.0, 7.0, 700.0]
        }]
    );
}

#[test]
fn types_with_the_gate_bit_do_not_rotate() {
    // `+0x44 & 0x10000003` (the constructor clears the low byte): the client reports it as `rotatable`.
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    let f = |y: i32| CursorFrame {
        mouse: [300, y],
        rotatable: false,
        ..at_world(500.0, 700.0)
    };
    frame(&mut bar, &mut m, f(200));
    raw(&mut bar, &m, HudEventKind::LeftDown, FREE);
    frame(&mut bar, &mut m, f(100));
    let Some(Cursor::Build(b)) = &bar.cursor else {
        panic!()
    };
    assert_eq!(
        (b.rotation, b.dragged),
        (0.0, false),
        "neither rotation nor dragging"
    );
}

#[test]
fn right_release_cancels_every_cursor() {
    let t = menus(&[(1100, vec![row(0, 399, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let all = [
        Cursor::Build(BuildCursor::new(1500)),
        Cursor::Fort(FortCursor::new(1011)),
        Cursor::Target(TargetCursor::new(413, 0)),
        Cursor::Defense(DefenseCursor::default()),
    ];
    for c in all {
        let mut bar = built(data(&[399]), &m);
        bar.cursor = Some(c.clone());
        let out = click_with(&mut bar, &m, FREE, true);
        assert_eq!(bar.cursor, None, "{c:?}");
        assert!(intents(&out).is_empty(), "{c:?}");
        assert!(pressed(&out).is_empty(), "{c:?}");
        assert!(!bar.right_down_in_bar, "the cursor takes the right press");
        if matches!(c, Cursor::Target(_)) {
            // Targeting mode: cancel (`0x17`) rebuilds in full.
            assert_eq!(
                out,
                [ToolbarOut::Sound(crate::hud::toolbar::ToolbarSound::Select)]
            );
        } else {
            assert!(out.is_empty(), "{c:?}: {out:?}");
        }
    }
}

/// The ring from `nml_sim::module::building::fort_testkit::fort_ring` (only `cfg(test)`, `nml-ui` does not know
/// `nml-sim`): part 0 corner, `dx` wall pieces, corner, `dz` pieces, corner, `dx` back, corner, `dz` back; rotations in
/// quarter steps in `f32`. Only for `dx, dz > 0`; tile width 128, center of the tile `base`.
fn reference_ring(base: (i32, i32), dx: i32, dz: i32, wall: u32, corner: u32) -> Vec<WallPart> {
    let (sx, sz) = (128.0f32, 128.0f32);
    let world = |t: i32| t as f32 * 128.0 + 64.0;
    let (b0, b2) = (world(base.0), world(base.1));
    let mut parts = Vec::new();
    let mut push = |guid, (ox, oz): (f32, f32), rot: f32| {
        parts.push(WallPart {
            guid,
            rotation: rot,
            pos: [b0 + ox, 0.0, b2 + oz],
        });
    };
    let (f40, f38) = (-1.570_796_4f32, 1.570_796_4f32);
    let mut f9 = std::f32::consts::PI;
    let (ex, ez) = ((dx + 1) as f32 * sx, (dz + 1) as f32 * sz);
    push(corner, (0.0, 0.0), f40 + f9);
    for i in 1..=dx {
        push(wall, (i as f32 * sx, 0.0), f9);
    }
    f9 += f38;
    push(corner, (ex, 0.0), f40 + f9);
    for k in 1..=dz {
        push(wall, (ex, k as f32 * sz), f9);
    }
    f9 += f38;
    push(corner, (ex, ez), f40 + f9);
    for i in (1..=dx).rev() {
        push(wall, (i as f32 * sx, ez), f9);
    }
    push(corner, (0.0, ez), f40 + f38 + f9);
    for k in (1..=dz).rev() {
        push(wall, (0.0, k as f32 * sz), f38 + f9);
    }
    parts
}

#[test]
fn fort_ring_has_4_plus_2dx_plus_2dz_parts_and_needs_more_than_3() {
    // Part 0 is a corner, m = 4 + 2|dx| + 2|dz| in [20, 92] for |dx|, |dz| in [4, 22].
    let center = [3.0 * 128.0 + 64.0, 0.0, 5.0 * 128.0 + 64.0];
    let mut sizes = Vec::new();
    for dx in (-22..=22).filter(|d| *d != 0) {
        for dz in (-22..=22).filter(|d| *d != 0) {
            let parts = ring(1011, 1012, center, [128, 128], dx, dz);
            let m = (4 + 2 * dx.abs() + 2 * dz.abs()) as usize;
            assert_eq!(parts.len(), m, "dx {dx} dz {dz}");
            assert_eq!(parts[0].guid, 1012, "part 0 is a corner");
            assert_eq!(parts.iter().filter(|p| p.guid == 1012).count(), 4);
            // Pairwise different edge tiles.
            let mut tiles: Vec<(i32, i32)> = parts
                .iter()
                .map(|p| {
                    (
                        (p.pos[0] / 128.0).floor() as i32,
                        (p.pos[2] / 128.0).floor() as i32,
                    )
                })
                .collect();
            tiles.sort_unstable();
            tiles.dedup();
            assert_eq!(tiles.len(), m, "dx {dx} dz {dz}");
            if dx.abs() >= 4 && dz.abs() >= 4 {
                sizes.push(m);
            }
        }
    }
    assert_eq!(
        (sizes.iter().min(), sizes.iter().max()),
        (Some(&20), Some(&92))
    );

    // Counter-check against the formula from re/construction, section "Senders" / the test kit of the simulation: same
    // parts in the same order; the rotation of the fourth corner is calculated by the original as `f40 + (f38 + f9)`,
    // by the test kit as `(f40 + f38) + f9` (at most one bit of difference).
    for (dx, dz) in [(4, 4), (4, 5), (9, 6), (22, 22)] {
        let c = [3.0 * 128.0 + 64.0, 0.0, 5.0 * 128.0 + 64.0];
        let got = ring(1011, 1012, c, [128, 128], dx, dz);
        let want = reference_ring((3, 5), dx, dz, 1011, 1012);
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!((g.guid, g.pos), (w.guid, w.pos), "Teil {i} dx {dx} dz {dz}");
            assert!(
                (g.rotation - w.rotation).abs() < 1e-5,
                "Teil {i}: {} gegen {}",
                g.rotation,
                w.rotation
            );
        }
    }
    // The rotation of the first corner per sign position (`0x00450C83` ff.): (+,+) pi - pi/2, (+,-) 2pi,
    // (-,-) 2pi - pi/2, (-,+) pi.
    let first = |dx, dz| ring(1, 2, [0.0; 3], [128, 128], dx, dz)[0].rotation;
    assert_eq!(first(5, 5), -FRAC_PI_2 + PI);
    assert_eq!(first(5, -5), 0.0 + TAU);
    assert_eq!(first(-5, -5), -FRAC_PI_2 + TAU);
    assert_eq!(first(-5, 5), 0.0 + PI);
    // Negative direction: the first piece is to the left of the center.
    let neg = ring(1, 2, [0.0; 3], [128, 128], -5, 5);
    assert_eq!(neg[1].pos, [-128.0, 0.0, 0.0]);
}

#[test]
fn fort_cursor_sizes_the_ring_by_two_clicks_and_places_with_the_third() {
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(1));
    let fort = |bar: &Toolbar| match &bar.cursor {
        Some(Cursor::Fort(f)) => f.clone(),
        c => panic!("no wall cursor: {c:?}"),
    };
    assert_eq!((fort(&bar).wall, fort(&bar).corner), (1011, 1012));
    // Phase 1: center on the tile center under the pointer (192, 320 for 200, 300), one corner piece.
    frame(&mut bar, &mut m, at_world(200.0, 300.0));
    let f = fort(&bar);
    assert_eq!((f.phase, f.center), (FortPhase::Place, [192.0, 7.0, 320.0]));
    assert_eq!(f.parts.len(), 1);
    assert_eq!(f.parts[0].guid, 1012);
    // Click 1: center fixed. Pointer 3.5 tiles / 9.2 tiles further -> dx 3 (truncated), dz 9.
    click(&mut bar, &m, FREE);
    assert_eq!(fort(&bar).phase, FortPhase::Size);
    frame(
        &mut bar,
        &mut m,
        at_world(192.0 + 3.5 * 128.0, 320.0 + 9.2 * 128.0),
    );
    assert_eq!((fort(&bar).dx, fort(&bar).dz), (3, 9));
    assert_eq!(fort(&bar).parts.len(), 4 + 6 + 18);
    // Click 2 with dx = 3: too small, all cursors gone, nothing submitted.
    let out = click(&mut bar, &m, FREE);
    assert!(intents(&out).is_empty());
    assert_eq!(bar.cursor, None);

    // New: 4 x 4 is enough. The pointer far away is clamped to +/-22.
    click(&mut bar, &m, normal_at(1));
    frame(&mut bar, &mut m, at_world(200.0, 300.0));
    click(&mut bar, &m, FREE);
    frame(
        &mut bar,
        &mut m,
        at_world(192.0 + 99.0 * 128.0, 320.0 - 99.0 * 128.0),
    );
    assert_eq!((fort(&bar).dx, fort(&bar).dz), (22, -22));
    frame(
        &mut bar,
        &mut m,
        at_world(192.0 + 4.9 * 128.0, 320.0 + 4.1 * 128.0),
    );
    assert_eq!((fort(&bar).dx, fort(&bar).dz), (4, 4));
    // Click 2: phase 3 (move the ring), nothing submitted yet.
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());
    assert_eq!(fort(&bar).phase, FortPhase::Move);
    // The ring follows the pointer in tile steps: 2.2 tiles to the right -> 2 tiles.
    let p = 192.0 + 4.9 * 128.0;
    let before = fort(&bar).center;
    frame(
        &mut bar,
        &mut m,
        at_world(p + 2.2 * 128.0, 320.0 + 4.1 * 128.0),
    );
    let after = fort(&bar).center;
    assert_eq!(after[0] - before[0], 256.0);
    assert_eq!(after[2], before[2]);
    // An invalid ring (client: ring_valid) is not submitted ...
    let mut f = at_world(p + 2.2 * 128.0, 320.0 + 4.1 * 128.0);
    f.ring_valid = Some(false);
    frame(&mut bar, &mut m, f);
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());
    assert!(matches!(bar.cursor, Some(Cursor::Fort(_))));
    // ... a valid one with all 20 parts, part 0 a corner; afterwards all cursors are closed (also with Shift).
    f.ring_valid = Some(true);
    frame(&mut bar, &mut m, f);
    m.keys.shift = true;
    let out = click(&mut bar, &m, FREE);
    let placed = intents(&out);
    let [ToolbarIntent::PlaceWall { parts }] = placed.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(parts.len(), 20);
    assert_eq!(parts[0].guid, 1012);
    assert_eq!(parts[0].pos, [after[0], after[1], after[2]]);
    assert_eq!(bar.cursor, None);
}

#[test]
fn cursor_gets_button_clicks_first_after_a_silent_rebuild() {
    // O-265: `{0x10, 0}` recreates the buttons, they sit behind the cursor.
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0)); // validity only applies to the position of the previous frame
    // Before: the buttons see the click first, the cursor stays open.
    let out = click(&mut bar, &m, normal_at(2));
    assert_eq!(pressed(&out), [399]);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::Dispatch {
            msg: 399,
            objects: vec![1]
        }]
    );
    assert!(matches!(bar.cursor, Some(Cursor::Build(_))));
    assert!(!bar.cursor_before_buttons);
    // Silent rebuild.
    let mut out = Vec::new();
    bar.event(
        &HudEvent::with_args(HudEventKind::Rebuild, &[0]),
        &m,
        &mut out,
    );
    assert!(bar.cursor_before_buttons);
    // After: the same click on the button places the building, the button hears nothing.
    let out = click(&mut bar, &m, normal_at(2));
    assert!(pressed(&out).is_empty(), "{out:?}");
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceBuilding {
            guid: 1500,
            rotation: 0.0,
            pos: [500.0, 7.0, 700.0]
        }]
    );
    assert_eq!(bar.cursor, None);
    assert!(!bar.cursor_before_buttons);
}

#[test]
fn defense_point_emits_coordinates_and_right_cancels() {
    let t = menus(&[(1100, vec![row(0, 399, kinds::DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[399]), &m);
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    // The ring (decal of the engine) only appears on explored terrain.
    let mut f = at_world(1000.0, 2000.0);
    f.valid = false;
    frame(&mut bar, &mut m, f);
    assert_eq!(bar.defense_ring(), None);
    f.valid = true;
    frame(&mut bar, &mut m, f);
    assert_eq!(bar.defense_ring(), Some([1000.0, 2000.0]));
    // The click sends the point under the pointer (also on unexplored terrain) and closes the cursor.
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::DefensePoint {
            x: 1000.0,
            z: 2000.0
        }]
    );
    assert_eq!(bar.cursor, None);
    assert_eq!(bar.defense_ring(), None);
    // Right-click cancels: without intent.
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    frame(&mut bar, &mut m, f);
    let out = click_with(&mut bar, &m, FREE, true);
    assert!(intents(&out).is_empty());
    assert_eq!(bar.cursor, None);
    // Hidden bar: no ring.
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    frame(&mut bar, &mut m, f);
    bar.shown = false;
    assert_eq!(bar.defense_ring(), None);
}

#[test]
fn target_click_chooses_a_valid_target_and_rebuilds_fully() {
    let t = menus(&[
        (1100, vec![row(2, 413, kinds::TARGET)]),
        (413, vec![row(0, 409, kinds::DO)]),
    ]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.targets.insert(413);
    m.target_icons.insert(413, 7);
    let mut bar = built(data(&[413, 409]), &m);
    click(&mut bar, &m, normal_at(2));
    let Some(Cursor::Target(c)) = &bar.cursor else {
        panic!("{:?}", bar.cursor)
    };
    assert_eq!((c.msg, c.mouse_icon), (413, 7));
    assert_eq!(bar.shown_menu(), 413);
    // Invalid target: consumed, nothing, cursor and submenu stay.
    let mut f = at_world(10.0, 20.0);
    f.valid = false;
    frame(&mut bar, &mut m, f);
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());
    assert!(matches!(bar.cursor, Some(Cursor::Target(_))));
    assert_eq!(bar.shown_menu(), 413);
    // Object under the pointer: intent with object, full build (base menu, cursor gone).
    f.valid = true;
    f.object = Some(77);
    frame(&mut bar, &mut m, f);
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::TargetChosen {
            msg: 413,
            object: Some(77),
            ground: None
        }]
    );
    assert_eq!((bar.cursor.clone(), bar.shown_menu()), (None, 1100));
    // Ground target.
    click(&mut bar, &m, normal_at(2));
    f.object = None;
    frame(&mut bar, &mut m, f);
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::TargetChosen {
            msg: 413,
            object: None,
            ground: Some([10.0, 20.0])
        }]
    );
}

#[test]
fn tent_button_opens_the_tent_cursor_and_places_with_the_unit() {
    let t = menus(&[(1717, vec![row(0, 453, kinds::TARGET_DO)])]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(5, 1717)];
    m.targets.insert(453);
    let mut bar = built(data(&[453]), &m);
    // Without a tent or during the pause nothing happens (except ButtonPressed).
    let out = click(&mut bar, &m, normal_at(0));
    assert_eq!(pressed(&out), [453]);
    assert_eq!(bar.cursor, None);
    m.tent_guid = Some(1750);
    m.visible_pause = true;
    click(&mut bar, &m, normal_at(0));
    assert_eq!(bar.cursor, None);
    m.visible_pause = false;
    let out = click(&mut bar, &m, normal_at(0));
    assert!(intents(&out).is_empty(), "opening sends nothing: {out:?}");
    assert_eq!(bar.cursor, Some(Cursor::Build(BuildCursor::tent(1750, 5))));
    // Click with a valid position: tent of the unit at (x, z), afterwards the cursors are closed.
    frame(&mut bar, &mut m, at_world(640.0, 320.0));
    frame(&mut bar, &mut m, at_world(640.0, 320.0));
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceTent {
            unit: 5,
            pos: [640.0, 320.0]
        }]
    );
    assert_eq!(bar.cursor, None);
}

#[test]
fn cursor_ignores_the_world_while_the_bar_is_hidden() {
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    bar.shown = false;
    let out = click(&mut bar, &m, FREE);
    assert!(intents(&out).is_empty());
    assert!(matches!(bar.cursor, Some(Cursor::Build(_))));
}

#[test]
fn validity_counts_only_for_the_pose_the_client_checked() {
    // `valid` applies to the position of the previous frame. Movement and click in the same frame do not place, the
    // next frame (pointer at rest) does.
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    // The pointer jumps; the client has still checked the old position (valid).
    frame(&mut bar, &mut m, at_world(628.0, 700.0));
    assert!(intents(&click_with(&mut bar, &m, FREE, false)).is_empty());
    assert!(matches!(bar.cursor, Some(Cursor::Build(_))));
    // Next frame: calm, the client has checked the new position.
    frame(&mut bar, &mut m, at_world(628.0, 700.0));
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceBuilding {
            guid: 1500,
            rotation: 0.0,
            pos: [628.0, 7.0, 700.0]
        }]
    );
    // The same while rotating: a changed rotation makes the verdict invalid.
    click(&mut bar, &m, normal_at(0));
    let at = |y: i32| CursorFrame {
        mouse: [300, y],
        ..at_world(500.0, 700.0)
    };
    frame(&mut bar, &mut m, at(200));
    frame(&mut bar, &mut m, at(200));
    raw(&mut bar, &m, HudEventKind::LeftDown, FREE);
    frame(&mut bar, &mut m, at(150));
    let Some(Cursor::Build(b)) = &bar.cursor else {
        panic!()
    };
    assert!(b.dragged && !b.valid);

    // Wall: if the ring changes in the same frame, the verdict drops; in the next one it applies.
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(1));
    let ok = |x: f32, z: f32| CursorFrame {
        ring_valid: Some(true),
        ..at_world(x, z)
    };
    frame(&mut bar, &mut m, ok(200.0, 300.0));
    click(&mut bar, &m, FREE);
    frame(
        &mut bar,
        &mut m,
        ok(192.0 + 4.5 * 128.0, 320.0 + 4.5 * 128.0),
    );
    click(&mut bar, &m, FREE);
    let Some(Cursor::Fort(f)) = &bar.cursor else {
        panic!()
    };
    assert_eq!(f.phase, FortPhase::Move);
    // Pointer two tiles further: the ring moves, the old "valid" does not count.
    frame(
        &mut bar,
        &mut m,
        ok(192.0 + 6.5 * 128.0, 320.0 + 4.5 * 128.0),
    );
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());
    frame(
        &mut bar,
        &mut m,
        ok(192.0 + 6.5 * 128.0, 320.0 + 4.5 * 128.0),
    );
    let out = click(&mut bar, &m, FREE);
    assert!(matches!(
        intents(&out).as_slice(),
        [ToolbarIntent::PlaceWall { .. }]
    ));
}

#[test]
fn validity_after_the_step_lets_a_scrolling_cursor_place() {
    // While edge scrolling, the position changes every frame, the verdict from the frame before never counts. The host
    // checks exactly the new position after the step and submits the verdict in the same frame; a click between the
    // frames then places.
    let t = build_menus();
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(0));
    for x in [500.0, 628.0, 756.0] {
        frame(&mut bar, &mut m, at_world(x, 700.0));
        let Some(Cursor::Build(b)) = &bar.cursor else {
            panic!()
        };
        assert!(!b.valid || x == 500.0, "invalid without a follow-up");
        bar.set_cursor_validity(true, None);
    }
    let out = click(&mut bar, &m, FREE);
    assert_eq!(
        intents(&out),
        [ToolbarIntent::PlaceBuilding {
            guid: 1500,
            rotation: 0.0,
            pos: [756.0, 7.0, 700.0]
        }]
    );
    // An invalid verdict for the new position also locks a previously valid one.
    click(&mut bar, &m, normal_at(0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    frame(&mut bar, &mut m, at_world(500.0, 700.0));
    bar.set_cursor_validity(false, None);
    assert!(intents(&click(&mut bar, &m, FREE)).is_empty());

    // Wall: the verdict on the parts of the ring applies to the ring after the step.
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    click(&mut bar, &m, normal_at(1));
    frame(&mut bar, &mut m, at_world(200.0, 300.0));
    click(&mut bar, &m, FREE);
    frame(
        &mut bar,
        &mut m,
        at_world(192.0 + 4.5 * 128.0, 320.0 + 4.5 * 128.0),
    );
    click(&mut bar, &m, FREE);
    frame(
        &mut bar,
        &mut m,
        at_world(192.0 + 6.5 * 128.0, 320.0 + 4.5 * 128.0),
    );
    bar.set_cursor_validity(false, Some(true));
    let out = click(&mut bar, &m, FREE);
    assert!(matches!(
        intents(&out).as_slice(),
        [ToolbarIntent::PlaceWall { .. }]
    ));

    // Target and defense also take over the verdict.
    let mut bar = built(data(&[1500, 1011, 399]), &m);
    bar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
    bar.set_cursor_validity(true, None);
    let Some(Cursor::Defense(d)) = &bar.cursor else {
        panic!()
    };
    assert!(d.explored);
}
