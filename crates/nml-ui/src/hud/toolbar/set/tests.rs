use nml_formats::gifcon::{Toolbar, parse_toolbar};

use super::*;
use crate::hud::toolbar::button::{ButtonBars, ButtonState};
use crate::hud::toolbar::model::{HuntTarget, Production, SelectedObject};

const DO: u32 = 0x400_0000;
const GROUP_AND: u32 = 0x1;
const SCREEN: Screen = Screen {
    width: 1024,
    height: 768,
};
/// Top edge of the bar at 768 pixel rows.
const BAR_Y: i32 = 768 - 176;

/// Positions as in the shipped data (`toolbar.cfg`): normal 7 x 2 in grid 53, selection column by column 26 x 25,
/// heroes and headhunters from the right edge. Icons: every GUID of the list with sheet 0 and tile = GUID mod 50;
/// plus GUID 98 with negative values and GUID 97 with sheet 5 (does not exist).
fn data(icons: &[u32]) -> ToolbarData {
    let mut ini = String::from("ICONS data/icons/i0.tga\r\n");
    for g in icons {
        ini.push_str(&format!("GUID {g} 0 0 0 {}\r\n", g % 50));
    }
    ini.push_str("GUID 98 0 0 -1 -2\r\nGUID 97 0 0 5 1\r\n");
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

/// A menu row in the test: slot, BUTTONMSG, kind bits, `REQUIRED_GUID`.
type Row<'a> = (i32, i32, u32, &'a [i32]);

/// A `toolbar.ini` menu made of rows `(slot, msg, flags, required)`.
fn menus(blocks: &[(i32, &[Row])]) -> Toolbar {
    let flag_names = [
        (0x1, "GROUP_AND_MSG"),
        (0x100_0000, "BUILD_MSG"),
        (0x200_0000, "MAKE_MSG"),
        (0x300_0000, "UPGRADE_MSG"),
        (0x400_0000, "DO_MSG"),
        (0x700_0000, "TARGET_MSG"),
        (0x800_0000, "TARGET_DO_MSG"),
        (0xD00_0000, "ONLYONCE_MSG"),
        (0x1000_0000, "COUNTERUPGRADE_MSG"),
        (0x1100_0000, "MAKEONLY_ONCE_MSG"),
    ];
    let mut text = String::new();
    for (guid, rows) in blocks {
        text.push_str(&format!("GUID {guid}\r\n"));
        for (slot, msg, flags, required) in rows.iter() {
            text.push_str(&format!("BUTTONID {slot}\r\nBUTTONMSG {msg}\r\n"));
            if flags & 1 != 0 {
                text.push_str("GROUP_AND_MSG\r\n");
            }
            let kind = flags & 0xFF00_0000;
            if let Some((_, name)) = flag_names.iter().find(|(f, _)| *f == kind && kind != 0) {
                text.push_str(&format!("{name}\r\n"));
            }
            for r in required.iter() {
                text.push_str(&format!("REQUIRED_GUID {r}\r\n"));
            }
        }
        text.push_str("ENDGUID\r\n");
    }
    let t = parse_toolbar(&text);
    // Counter-check of the test setup: the kind bits arrive.
    for (guid, rows) in blocks {
        for (b, (_, msg, flags, _)) in t.menus[guid].iter().zip(rows.iter()) {
            assert_eq!((b.msg, b.flags), (*msg, *flags), "menu {guid}");
        }
    }
    t
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

/// `BUTTONMSG` per normal slot (row, not button).
fn slot_msgs(set: &ButtonSet) -> [Option<i32>; SLOT_COUNT] {
    std::array::from_fn(|i| set.slots[i].as_ref().map(|b| b.msg))
}

/// `BUTTONMSG` je Normalknopf.
fn button_msgs(set: &ButtonSet) -> [Option<i32>; SLOT_COUNT] {
    std::array::from_fn(|i| set.normal[i].as_ref().map(|b| b.spec.msg))
}

fn main_set(model: &ToolbarModel, d: &ToolbarData) -> ButtonSet {
    let menu = choose_menu(model).menu().unwrap_or(0);
    build_set(model, menu, true, d, SCREEN, None)
}

/// Menu 1100 as in the shipped data (O-227): 403 and 405 share slot 4, 485 and 486 slot 12.
fn colonist_menu() -> Toolbar {
    menus(&[(
        1100,
        &[
            (0, 110, 0x800_0000, &[]),
            (2, 413, 0x700_0000, &[]),
            (4, 403, 0x700_0000, &[]),
            (4, 405, 0x800_0000, &[]),
            (6, 414, 0x700_0000, &[]),
            (8, 300, 0xD00_0000, &[]),
            (8, 301, 0x400_0000, &[]),
            (12, 485, 0x800_0000, &[2806]),
            (12, 486, 0x800_0000, &[]),
            (13, 407, 0, &[]),
        ],
    )])
}

// QUIRK O-227: kept
#[test]
fn first_entry_per_slot_wins_and_only_hidden_rows_yield() {
    let t = colonist_menu();
    let d = data(&[110, 413, 403, 405, 414, 300, 301, 485, 486, 407]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    m.targets = [110, 413, 403, 405, 414, 485, 486].into();
    let set = main_set(&m, &d);
    // 403 before 405, 485 (locked, 2806 missing) before 486: locks do not let entries move up.
    let mut want = [None; SLOT_COUNT];
    for (s, msg) in [
        (0, 110),
        (2, 413),
        (4, 403),
        (6, 414),
        (8, 300),
        (12, 485),
        (13, 407),
    ] {
        want[s] = Some(msg);
    }
    assert_eq!(slot_msgs(&set), want);
    assert_eq!(button_msgs(&set), want);
    assert!(
        m.locks_for(&set.normal[12].as_ref().unwrap().spec)
            .missing_required
    );
    assert_eq!(set.shown_menu, 1100);

    // Hidden: script lock (413), TARGET without `targets.ini` (403), one-time kind with an order (300).
    m.disabled = [413].into();
    m.targets.remove(&403);
    m.pending = [(300, 1)].into();
    let set = main_set(&m, &d);
    assert_eq!(slot_msgs(&set)[2], None);
    assert_eq!(slot_msgs(&set)[4], Some(405), "only hidden entries move up");
    assert_eq!(slot_msgs(&set)[8], Some(301));
    // One-time kind with an object counter instead of an order likewise.
    m.pending.clear();
    m.stock = [(300, 1)].into();
    assert_eq!(slot_msgs(&main_set(&m, &d))[8], Some(301));
}

#[test]
fn foreign_or_empty_selection_has_no_buttons() {
    let t = colonist_menu();
    let d = data(&[110, 1100]);
    let mut m = ToolbarModel::new(&t);
    assert_eq!(choose_menu(&m), MenuChoice::Empty);
    let set = main_set(&m, &d);
    assert_eq!(set, ButtonSet::default());

    m.selection = vec![SelectedObject {
        own: false,
        ..unit(1, 1100)
    }];
    m.selection[0].stance_bits = 8;
    assert_eq!(choose_menu(&m), MenuChoice::Foreign);
    let set = build_set(&m, 1100, true, &d, SCREEN, None);
    assert_eq!(set.buttons().count(), 0);
    assert_eq!(set.shown_menu, 0);
}

// QUIRK O-229: kept
#[test]
fn site_shows_only_slot_13() {
    let t = menus(&[(
        1000,
        &[
            (0, 1100, 0x200_0000, &[]),
            (5, 430, DO, &[]),
            (13, 399, DO, &[]),
        ],
    )]);
    let d = data(&[1100, 430, 399]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![SelectedObject {
        site: true,
        ..building(5, 1000)
    }];
    assert_eq!(choose_menu(&m), MenuChoice::SiteOnly13(1000));
    let set = main_set(&m, &d);
    let mut want = [None; SLOT_COUNT];
    want[13] = Some(399);
    assert_eq!(button_msgs(&set), want);
    assert_eq!(set.shown_menu, 1000);
    // Construction sites carry no filter list (`BuildButtonSet(type, 1, 1, 0)`).
    assert!(set.groups.is_empty());
    // Finished: the whole menu.
    m.selection[0].site = false;
    assert_eq!(choose_menu(&m), MenuChoice::Menu(1000));
    assert_eq!(main_set(&m, &d).normal.iter().flatten().count(), 3);
}

// QUIRK O-228, O-272: kept
#[test]
fn multi_selection_adds_only_non_group_and_buttons_on_free_slots() {
    let t = menus(&[
        (
            1100,
            &[(0, 110, 0x800_0000, &[]), (6, 414, DO | GROUP_AND, &[])],
        ),
        (
            1200,
            &[
                (0, 999, DO, &[]),
                (6, 414, DO | GROUP_AND, &[]),
                (7, 432, DO, &[]),
                (8, 433, DO | GROUP_AND, &[]),
            ],
        ),
    ]);
    let d = data(&[110, 414, 999, 432, 433, 1100, 1200]);
    let mut m = ToolbarModel::new(&t);
    m.targets = [110].into();
    m.selection = vec![unit(1, 1100), unit(2, 1200), unit(3, 1200)];
    assert_eq!(choose_menu(&m), MenuChoice::Multi(1100));
    let set = main_set(&m, &d);
    let mut want = [None; SLOT_COUNT];
    want[0] = Some(110);
    want[6] = Some(414);
    want[7] = Some(432);
    assert_eq!(slot_msgs(&set), want, "999 occupied, 433 is GROUP_AND");
    // The addition sets `+0x5CC = 0` (O-228, D-234).
    assert_eq!(set.shown_menu, 0);
    // Filter lists per object type (O-272): the first object only with its GROUP_AND button, the others with the
    // same GROUP_AND button in the same slot or the added button.
    assert_eq!(set.groups.get(&1100), Some(&vec![1]));
    assert_eq!(set.groups.get(&1200), Some(&vec![2, 3]));
    // The search uses the BUTTONMSG: no match, the command goes to the whole selection.
    assert_eq!(set.group_for(414), None);
    assert_eq!(set.group_for(1200), Some(&[2, 3][..]));
    // Multi-selection shows the selection buttons.
    assert_eq!(set.selection.len(), 3);
    // Submenu: no addition, no filter list, `+0x5CC` = submenu.
    let sub = build_set(&m, 1200, false, &d, SCREEN, None);
    assert_eq!(sub.shown_menu, 1200);
    assert!(sub.groups.is_empty());
    assert_eq!(slot_msgs(&sub)[0], Some(999));
    assert_eq!(sub.selection.len(), 3, "the rows remain");
}

// QUIRK O-230: kept
#[test]
fn counter_upgrade_without_requirement_is_hidden_others_locked() {
    let t = menus(&[(
        601,
        &[
            (0, 611, 0x1000_0000, &[2900]),
            (0, 612, 0x1000_0000, &[]),
            (1, 1101, 0x200_0000, &[1001]),
            (2, 640, 0, &[1001]),
        ],
    )]);
    let d = data(&[611, 612, 1101, 640]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(7, 601)];
    let set = main_set(&m, &d);
    // 611 lacks its prerequisite: hidden, 612 moves up (O-230).
    assert_eq!(slot_msgs(&set)[0], Some(612));
    // MAKE with a missing prerequisite stays visible but is locked.
    let make = &set.normal[1].as_ref().unwrap().spec;
    assert!(m.locks_for(make).missing_required);
    // Only the checked kinds lock (like `CHECKED_KINDS` of the simulation): a submenu does not.
    let sub = &set.normal[2].as_ref().unwrap().spec;
    assert!(!m.locks_for(sub).missing_required);
    m.stock = [(1001, 2)].into();
    assert!(!m.locks_for(make).missing_required);
    // Running counter-upgrade: the GUID anywhere or the own slot.
    let counter = &set.normal[0].as_ref().unwrap().spec;
    assert!(!m.locks_for(counter).counter_running);
    m.counter_busy = [612].into();
    assert!(m.locks_for(counter).counter_running);
    m.counter_busy.clear();
    m.slot_counter_busy = true;
    assert!(m.locks_for(counter).counter_running);
    assert!(!m.locks_for(make).counter_running, "COUNTERUPGRADE only");
    // With an object counter above 0, 611 appears again in slot 0.
    m.stock.insert(2900, 1);
    assert_eq!(slot_msgs(&main_set(&m, &d))[0], Some(611));
}

// QUIRK O-37: kept
#[test]
fn production_row_starts_at_slot_4_and_stops_at_26() {
    let t = menus(&[(1000, &[(0, 1101, 0x200_0000, &[])])]);
    let d = data(&[1101, 1102]);
    let mut m = ToolbarModel::new(&t);
    let mut hq = building(5, 1000);
    hq.production = Some(Production {
        running: Some((1101, 0.25)),
        queue: [1101, 1102].repeat(14),
    });
    m.selection = vec![hq];
    let set = main_set(&m, &d);
    // Running order in slot 0 (50 px), then at most 26 waiting ones from slot 4 (25 px).
    assert_eq!(set.production.len(), 1 + 26);
    let running = &set.production[0];
    assert_eq!(running.spec.rect, [234, BAR_Y + 128, 284, BAR_Y + 178]);
    assert_eq!(
        (running.spec.msg, running.spec.flags),
        (1101, KIND_PRODUCTION)
    );
    assert_eq!((running.spec.scale, running.progress()), (1.0, 0.25));
    assert_eq!(m.progress_for(&running.spec), Some(0.25));
    let first = &set.production[1];
    assert_eq!(
        first.spec.rect,
        [234 + 52, BAR_Y + 128, 234 + 52 + 25, BAR_Y + 153]
    );
    assert_eq!(first.spec.flags, KIND_PRODUCTION | 1, "index = slot - 3");
    assert_eq!((first.spec.scale, first.progress()), (0.5, 1.0));
    assert_eq!(m.progress_for(&first.spec), None);
    let last = set.production.last().unwrap();
    assert_eq!(last.spec.rect[..2], [598, BAR_Y + 153]);
    assert_eq!(last.spec.flags & 0xFF, 26);
    // Identical orders without a count: each its own button (O-37).
    assert_eq!(set.production[2].spec.msg, 1102);
    // Without a running order no button in slot 0, the queue still begins in slot 4.
    m.selection[0].production = Some(Production {
        running: None,
        queue: vec![1102],
    });
    let set = main_set(&m, &d);
    assert_eq!(set.production.len(), 1);
    assert_eq!(set.production[0].spec.rect[..2], [286, BAR_Y + 128]);
    // Multi-selection: no production row.
    m.selection.push(building(6, 1000));
    assert!(main_set(&m, &d).production.is_empty());
}

// QUIRK O-232: kept
#[test]
fn single_selection_shows_only_scatter() {
    let t = menus(&[(1100, &[])]);
    let d = data(&[]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    // Single selection: `+0x18C = -1`, only 519 with icon 14 (O-232) - even if the world has a formation: the
    // full build `0x00457980` sets `+0x18C = -1` and takes over `GameWorld+0x20B8` (`FUN_00477950`) only for a
    // multi-selection; the host supplies the formation of the world.
    m.formation = 0;
    let set = main_set(&m, &d);
    assert_eq!(set.formation.len(), 1);
    let f = &set.formation[0].spec;
    assert_eq!((f.msg, f.flags), (MSG_SCATTER, DO));
    assert_eq!(
        (f.base, f.icon),
        ((SheetRef::Resource, 15), (SheetRef::Resource, 14))
    );
    assert_eq!(f.rect, [169, BAR_Y + 13, 194, BAR_Y + 38]);
    assert_eq!(f.scale, 1.0);
    // Multi-selection with formation 2: collapsed 510 with icon 9 + 2.
    m.selection.push(unit(2, 1100));
    m.formation = 2;
    let set = main_set(&m, &d);
    let f: Vec<_> = set
        .formation
        .iter()
        .map(|b| (b.spec.msg, b.spec.icon.1))
        .collect();
    assert_eq!(f, [(MSG_FORMATION_CURRENT, 11)]);
    // Expanded: 509, 511 ... 515 (icons 9 ... 13), 519 (icon 14).
    let open = formation_buttons(2, true, &d, SCREEN, None);
    let f: Vec<_> = open.iter().map(|b| (b.spec.msg, b.spec.icon.1)).collect();
    assert_eq!(
        f,
        [
            (509, 11),
            (511, 9),
            (512, 10),
            (513, 11),
            (514, 12),
            (515, 13),
            (519, 14)
        ]
    );
    assert_eq!(open[6].spec.rect[0], 169 + 32 * 6);
    // Buildings (without module 0): no formation.
    m.selection = vec![building(5, 1100)];
    assert!(main_set(&m, &d).formation.is_empty());
}

#[test]
fn stance_bits_are_the_union_and_mark_several() {
    let t = menus(&[(1100, &[])]);
    let d = data(&[]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100), unit(2, 1100), unit(3, 1100)];
    // Mode 3 (bit 8) and mode 1 (bit 2); the third object without stance.
    m.selection[0].stance_bits = 8;
    m.selection[1].stance_bits = 2;
    let set = main_set(&m, &d);
    let s: Vec<_> = set
        .stance
        .iter()
        .map(|b| (b.spec.msg, b.spec.flags))
        .collect();
    let sel = KIND_STANCE | STANCE_SELECTED;
    assert_eq!(
        s,
        [
            (420, sel),
            (421, KIND_STANCE),
            (422, sel),
            (423, KIND_STANCE),
            (424, KIND_STANCE)
        ]
    );
    // Slot i -> 420 + i; icon `resourcen_icons` tile i + 4, base icon tile 15, 25 px, scale 1.
    for (i, b) in set.stance.iter().enumerate() {
        assert_eq!(b.spec.icon, (SheetRef::Resource, i as i32 + 4));
        assert_eq!(b.spec.base, (SheetRef::Resource, 15));
        let [x, y] = d.stance[i];
        assert_eq!(b.spec.rect, [x, BAR_Y + y, x + 25, BAR_Y + y + 25]);
        assert_eq!(b.spec.scale, 1.0);
    }
    // Bits 0x10, 4, 1: the remaining three.
    m.selection[0].stance_bits = 0x10;
    m.selection[1].stance_bits = 4 | 1;
    let set = main_set(&m, &d);
    let marked: Vec<_> = set
        .stance
        .iter()
        .filter(|b| b.spec.flags & STANCE_SELECTED != 0)
        .map(|b| b.spec.msg)
        .collect();
    assert_eq!(marked, [421, 423, 424]);
    // Without stance bits no stance buttons.
    for o in &mut m.selection {
        o.stance_bits = 0;
    }
    assert!(main_set(&m, &d).stance.is_empty());
}

// QUIRK O-258: kept
// QUIRK O-263: deviation
#[test]
fn selection_buttons_stop_after_30_and_leave_gaps() {
    let t = menus(&[(1100, &[]), (1300, &[])]);
    let d = data(&[1100]);
    let mut m = ToolbarModel::new(&t);
    m.selection = (0..31).map(|i| unit(100 + i, 1100)).collect();
    // Object 2 has a type without icon: gap (O-258).
    m.selection[2].guid = 1300;
    m.bars.insert(
        103,
        ButtonBars {
            tracked: [Some(0.5), None, None, None],
        },
    );
    let set = main_set(&m, &d);
    assert_eq!(set.selection.len(), 29, "30 slots, one gap, no 31st.");
    let hids: Vec<_> = set
        .selection
        .iter()
        .map(|b| b.spec.object.unwrap())
        .collect();
    assert_eq!(hids[..3], [100, 101, 103]);
    assert_eq!(*hids.last().unwrap(), 129);
    // Column order: slot 3 = (260, 153), 25 px, scale 0.5; BUTTONMSG = HID, kind 5 with the slot.
    let b3 = &set.selection[2];
    assert_eq!(b3.spec.rect, [260, BAR_Y + 153, 285, BAR_Y + 178]);
    assert_eq!((b3.spec.msg, b3.spec.flags), (103, KIND_SELECTION | 3));
    assert_eq!(b3.spec.scale, 0.5);
    // Object buttons are never locked at first (`+0x48` = HID >= 0).
    assert_eq!(b3.state, ButtonState::Normal);
    assert_eq!(m.bars_for(&b3.spec).unwrap().tracked[0], Some(0.5));
    assert!(m.bars_for(&set.selection[0].spec).is_none());
    // Icon by the strict rule: negative values -> gap (O-263).
    m.selection[0].guid = 98;
    assert_eq!(main_set(&m, &d).selection.len(), 28);
}

// QUIRK D-232, O-35, O-36: kept
#[test]
fn hero_buttons_fill_from_the_right_with_no_sixth_button() {
    let t = menus(&[]);
    let d = data(&[1200, 1201, 1202, 4100]);
    let mut m = ToolbarModel::new(&t);
    m.heroes = vec![
        (10, 1200),
        (11, 1201),
        (12, 1202),
        (13, 1200),
        (14, 1201),
        (15, 1202),
    ];
    m.headhunters = vec![4100];
    let mouse = Some([980, 550]);
    let row = hero_buttons(&m, &d, SCREEN, mouse);
    // At most five slots: no sixth button (O-35).
    assert_eq!(row.buttons().count(), 5);
    let ids: Vec<_> = row.buttons().map(|b| b.spec.msg).collect();
    assert_eq!(ids, [10, 11, 12, 13, 14]);
    let h0 = &row.slots[0].button.as_ref().unwrap().spec;
    assert_eq!(h0.rect, [969, 542, 1019, 592]);
    assert_eq!((h0.flags, h0.object), (KIND_HERO, Some(10)));
    assert_eq!(h0.icon, (SheetRef::Icons(0), 0));
    assert_eq!(
        row.slots[1].button.as_ref().unwrap().spec.rect[..2],
        [914, 542]
    );
    // The button takes over the mouse.
    assert_eq!(
        row.slots[0].button.as_ref().unwrap().state,
        ButtonState::Hover
    );
    // The position is written back (O-35).
    assert_eq!(row.slots[0].pos, Some([969, -50]));
    let hunters = headhunter_buttons(&m, &d, SCREEN, None);
    let j0 = &hunters.slots[0].button.as_ref().unwrap().spec;
    assert_eq!(j0.rect, [969, 482, 1019, 532]);
    assert_eq!((j0.msg, j0.flags, j0.object), (4100, KIND_HEADHUNTER, None));

    // Rebuild after a resolution change: the reused slot keeps the old position.
    let mut row = row;
    row.clear();
    assert_eq!(row.buttons().count(), 0);
    let small = Screen {
        width: 800,
        height: 600,
    };
    row.add(10, &m, &d, small, None);
    let r = row.slots[0].button.as_ref().unwrap().spec.rect;
    assert_eq!(r, [969, 600 - 176 - 50, 1019, 600 - 176]);
    // No duplicates; a hero without icon occupies no slot; removing frees the slot.
    row.add(10, &m, &d, small, None);
    assert_eq!(row.buttons().count(), 1);
    m.heroes.push((16, 98));
    row.add(16, &m, &d, small, None);
    assert_eq!(row.buttons().count(), 1);
    row.add(11, &m, &d, small, None);
    // Slot 1 was already occupied at 1024: 914 instead of 800 - 110.
    assert_eq!(row.slots[1].button.as_ref().unwrap().spec.rect[0], 914);
    // A fresh row calculates from the new edge.
    let fresh = hero_buttons(&m, &d, small, None);
    assert_eq!(
        fresh.slots[1].button.as_ref().unwrap().spec.rect[0],
        800 - 110
    );
    row.remove(10);
    assert!(row.slots[0].button.is_none());
    row.add(12, &m, &d, small, None);
    assert_eq!(row.slots[0].button.as_ref().unwrap().spec.msg, 12);

    // O-36: the headhunter row has its own count, the positions only come for as many slots as there are hero slots.
    let mut d2 = d.clone();
    d2.heroes.truncate(2);
    let hunters = ObjectRow::headhunters(&d2);
    assert_eq!(hunters.slots.len(), 5);
    assert_eq!(hunters.slots[1].pos, Some([-110, -110]));
    assert_eq!(hunters.slots[2].pos, None);
}

// QUIRK O-266: kept
#[test]
fn auto_occupant_view_needs_five_free_slots_and_has_no_417() {
    let t = menus(&[(
        1000,
        &[
            (0, 1101, 0x200_0000, &[]),
            (1, 1102, 0x200_0000, &[]),
            (2, 1103, 0x200_0000, &[]),
            (9, 415, DO, &[]),
            (13, 399, DO, &[]),
        ],
    )]);
    let d = data(&[1101, 1102, 1103, 415, 416, 417, 399, 1100]);
    let mut m = ToolbarModel::new(&t);
    let mut hq = building(5, 1000);
    hq.occupants = (0..6).map(|i| (50 + i, 1100)).collect();
    m.selection = vec![hq];
    // Slots 0-2 occupied: only 4 of slots 0-6 free, no view.
    let set = main_set(&m, &d);
    assert_eq!(slot_msgs(&set)[9], Some(415));
    assert_eq!(slot_msgs(&set)[3], None);
    // With the script lock on 1103 five are free: 415 -> 416, occupants into free slots ascending, no 417.
    m.disabled = [1103].into();
    let set = main_set(&m, &d);
    let mut want = [None; SLOT_COUNT];
    want[0] = Some(1101);
    want[1] = Some(1102);
    for (k, s) in [2, 3, 4, 5, 6, 7].into_iter().enumerate() {
        want[s] = Some(50 + k as i32);
    }
    want[9] = Some(MSG_UNQUARTER_ALL);
    want[13] = Some(399);
    assert_eq!(slot_msgs(&set), want);
    let occ = &set.normal[2].as_ref().unwrap().spec;
    assert_eq!((occ.flags, occ.object), (KIND_OCCUPANT, Some(50)));
    assert_eq!(occ.icon, (SheetRef::Icons(0), 0), "icon of the type GUID");
    assert_eq!(set.normal[9].as_ref().unwrap().spec.flags, DO);
    // Multi-selection of units: no automatic view.
    let t2 = menus(&[(1100, &[(9, 415, DO, &[])])]);
    let mut m2 = ToolbarModel::new(&t2);
    let mut u = unit(1, 1100);
    u.occupants = vec![(60, 1100)];
    m2.selection = vec![u.clone(), unit(2, 1100)];
    assert_eq!(slot_msgs(&main_set(&m2, &d))[0], None);
    m2.selection = vec![u];
    assert_eq!(slot_msgs(&main_set(&m2, &d))[0], Some(60));

    // Full view (415 pressed): everything empty, occupants from 0, 416 at 12, 417 at 13.
    m.disabled.clear();
    let mut set = main_set(&m, &d);
    occupant_view(&mut set, &m, &d, SCREEN, true);
    let mut want = [None; SLOT_COUNT];
    for (k, w) in want.iter_mut().take(6).enumerate() {
        *w = Some(50 + k as i32);
    }
    want[12] = Some(MSG_UNQUARTER_ALL);
    want[13] = Some(MSG_CANCEL_OCCUPANTS);
    assert_eq!(slot_msgs(&set), want);
    assert_eq!(button_msgs(&set), want);
}

// QUIRK O-268: kept
#[test]
fn trade_offers_neutral_slot_8() {
    let t = menus(&[]);
    let d = data(&[520, 525, 528, 494, 522]);
    let mut m = ToolbarModel::new(&t);
    m.local_slot = 2;
    m.trade_partners = vec![0, 2, 5, 8];
    let b = dynamic_buttons(DynamicKind::Trade(490), &m, &d, SCREEN);
    let msgs: [Option<i32>; SLOT_COUNT] =
        std::array::from_fn(|i| b[i].as_ref().map(|b| b.spec.msg));
    let mut want = [None; SLOT_COUNT];
    want[0] = Some(520);
    want[1] = Some(525);
    want[2] = Some(528);
    want[13] = Some(MSG_TRADE_BACK);
    assert_eq!(
        msgs, want,
        "local slot 2 is missing, slot 8 is included (O-268)"
    );
    assert!(b.iter().flatten().all(|b| b.spec.flags == DO));
    assert_eq!(
        b[0].as_ref().unwrap().spec.rect,
        [245, BAR_Y + 17, 295, BAR_Y + 67]
    );
}

// QUIRK O-267: kept
#[test]
fn target_guid_level_keeps_hero_gaps_and_hq_only_without_heroes() {
    let t = menus(&[]);
    let d = data(&[1410, 1411, 1412, 1300, 409, 470, 473]);
    let mut m = ToolbarModel::new(&t);
    let level = |m: &ToolbarModel| -> [Option<i32>; SLOT_COUNT] {
        let b = dynamic_buttons(DynamicKind::TargetGuid { art: 1, slot: 3 }, m, &d, SCREEN);
        std::array::from_fn(|i| b[i].as_ref().map(|b| b.spec.msg))
    };
    // Target unknown: no buttons, not even 409.
    assert_eq!(level(&m), [None; SLOT_COUNT]);
    m.hunt_targets.insert(
        3,
        HuntTarget {
            nation: 0,
            stock: [1, 0, 2, 5],
        },
    );
    m.hunters_present = [false, true, false];
    // Heroes at slot = index (gap at 1), no HQ, 409 at 13 (O-267).
    let mut want = [None; SLOT_COUNT];
    want[0] = Some(1410);
    want[2] = Some(1412);
    want[13] = Some(MSG_BACK);
    assert_eq!(level(&m), want);
    let b = dynamic_buttons(DynamicKind::TargetGuid { art: 1, slot: 3 }, &m, &d, SCREEN);
    assert_eq!(b[0].as_ref().unwrap().spec.flags, KIND_TARGET_GUID);
    // No hero: the HQ at slot 3, only with a headhunter of the kind and an object counter of the HQ above 0.
    m.hunt_targets.get_mut(&3).unwrap().stock = [0, 0, 0, 5];
    let mut want = [None; SLOT_COUNT];
    want[3] = Some(1300);
    want[13] = Some(MSG_BACK);
    assert_eq!(level(&m), want);
    m.hunters_present = [true, false, true];
    want[3] = None;
    assert_eq!(level(&m), want);
    // Nation outside 0 ... 5: only 409.
    m.hunt_targets.get_mut(&3).unwrap().nation = 7;
    assert_eq!(level(&m), want);
    assert_eq!(hunt_target_types(5), Some([2313, 2314, 2315, 2200]));

    // Target player: enemies in slot order, `470 + color` locked -> no button; slots consecutive.
    m.enemy_players = vec![(1, 3), (4, 0)];
    m.disabled = [473].into();
    let b = dynamic_buttons(DynamicKind::TargetPlayer(0), &m, &d, SCREEN);
    let msgs: Vec<_> = b
        .iter()
        .enumerate()
        .filter_map(|(i, b)| b.as_ref().map(|b| (i, b.spec.msg, b.spec.flags)))
        .collect();
    assert_eq!(msgs, [(0, 470, KIND_TARGET_PLAYER), (13, MSG_BACK, DO)]);
}

// QUIRK O-263: deviation
#[test]
fn normal_buttons_sit_bar_relative_and_clamp_negative_icons() {
    let t = menus(&[(
        1100,
        &[
            (0, 413, DO, &[]),
            (1, 98, DO, &[]),
            (2, 97, DO, &[]),
            (3, 77, DO, &[]),
        ],
    )]);
    let d = data(&[413]);
    let mut m = ToolbarModel::new(&t);
    m.selection = vec![unit(1, 1100)];
    let set = build_set(&m, 1100, true, &d, SCREEN, Some([250, 620]));
    let b = set.normal[0].as_ref().unwrap();
    assert_eq!(b.spec.rect, [245, BAR_Y + 17, 295, BAR_Y + 67]);
    assert_eq!(b.spec.base, (SheetRef::Icons(0), 0));
    assert_eq!(b.spec.icon, (SheetRef::Icons(0), 413 % 50));
    assert_eq!((b.spec.scale, b.spec.object), (1.0, None));
    assert_eq!(b.state, ButtonState::Hover, "mouse replay");
    // Negative icon: clamped to sheet 0, tile 0 (`0x00453D70`).
    assert_eq!(
        set.normal[1].as_ref().unwrap().spec.icon,
        (SheetRef::Icons(0), 0)
    );
    // Sheet outside the list (original: abort) and missing entry: no button, the row remains.
    assert!(set.normal[2].is_none() && set.slots[2].is_some());
    assert!(set.normal[3].is_none() && set.slots[3].is_some());
    // Lower row and position at another resolution: always relative to the bar.
    let small = Screen {
        width: 800,
        height: 600,
    };
    let set = build_set(&m, 1100, true, &d, small, None);
    assert_eq!(set.normal[0].as_ref().unwrap().spec.rect[1], 600 - 176 + 17);
}

// QUIRK O-229: kept
#[test]
fn silent_rebuild_keeps_the_shown_menu_or_asks_for_a_full_one() {
    let t = menus(&[
        (1100, &[(0, 110, 0x800_0000, &[])]),
        (110, &[(0, 1001, 0x100_0000, &[])]),
    ]);
    let d = data(&[110, 1001]);
    let mut m = ToolbarModel::new(&t);
    m.targets = [110].into();
    m.selection = vec![unit(1, 1100)];
    // An open submenu remains (D-234).
    let set = rebuild_shown(&m, 110, &d, SCREEN, None).unwrap();
    assert_eq!((set.shown_menu, slot_msgs(&set)[0]), (110, Some(1001)));
    assert!(set.groups.is_empty());
    // Without a shown menu, without selection or at a construction site: full build.
    assert!(rebuild_shown(&m, 0, &d, SCREEN, None).is_none());
    m.selection[0] = SelectedObject {
        site: true,
        ..building(5, 1100)
    };
    assert!(rebuild_shown(&m, 1100, &d, SCREEN, None).is_none());
    m.selection.clear();
    assert!(rebuild_shown(&m, 1100, &d, SCREEN, None).is_none());
}
