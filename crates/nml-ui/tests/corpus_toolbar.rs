//! Corpus test of the toolbar data against the original installation (`NML_GAME_DIR`; without an installation the test
//! silently skips itself). `toolbar.cfg` and `icons.ini` are position descriptions without language texts and the same
//! in both editions.

use nml_formats::gifcon::{TOOLBAR_PATH, parse_toolbar};
use nml_formats::statusbar::{STATUSBAR_PATH, parse_statusbar};
use nml_formats::targets::{TARGETDEFINES_PATH, TARGETS_PATH, parse_target_defines, parse_targets};
use nml_formats::text::decode_cp1252;
use nml_formats::tga::decode_tga_game;
use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_ui::coords::Screen;
use nml_ui::hud::tile_grids::TileGrids;
use nml_ui::hud::toolbar::data::{
    COLOR_COUNT, COUNTER_OVERLAY_PATH, INFO_ICONS_PATH, RESOURCE_ICONS_PATH, ToolbarData,
};
use nml_ui::hud::toolbar::set::{MenuChoice, build_set, choose_menu};
use nml_ui::hud::toolbar::{SelectedObject, ToolbarModel};

fn open() -> Option<Vfs> {
    let dir = corpus_game_dir()?;
    Some(Vfs::open_game_dir(dir).expect("installation cannot be opened"))
}

/// Size of an image of the installation in pixels.
fn image_size(vfs: &Vfs, path: &str) -> [i32; 2] {
    let bytes = vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let image = decode_tga_game(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
    [image.width() as i32, image.height() as i32]
}

#[test]
fn toolbar_data_reads_the_installation() {
    let Some(vfs) = open() else { return };
    let d = ToolbarData::load(&vfs, &mut TileGrids::new());

    // 17 background tiles, each from its own sheet; `status_a_00` is 128 x 128 and lies at y = -128.
    assert_eq!(d.background.len(), 17);
    assert_eq!(d.sheets.len(), 17);
    assert_eq!(d.sheets[0].path, "data/icons/status_a_00.tga");
    assert_eq!(d.sheets[0].tile, [128, 128]);
    assert_eq!(d.background[0].pos[1], -128);
    assert_eq!(d.sheets[d.background[0].sheet as usize].tile, [128, 128]);
    assert!(
        d.background
            .iter()
            .all(|b| (b.sheet as usize) < d.sheets.len())
    );

    // Multi-selection and queue: 30 slots, column by column.
    assert_eq!(d.selection.len(), 30);
    assert_eq!(d.selection[0], [234, 128]);
    assert_eq!(d.selection[1], [234, 153]);
    assert_eq!(d.selection[2], [260, 128]);
    assert_eq!(d.selection[29], [598, 153]);

    // Normal buttons: 2 rows of 7 in the grid 53.
    assert_eq!(d.normal.len(), 14);
    assert_eq!(d.normal[0], [245, 17]);
    assert_eq!(d.normal[13], [245 + 53 * 6, 71]);

    // Heroes and headhunters hang at the bottom right edge (negative positions, spacing 55).
    assert_eq!(d.heroes.len(), 5);
    assert_eq!(d.headhunters.len(), 5);
    assert_eq!(d.heroes[0][1], -50);
    assert_eq!(d.headhunters[0][1], -110);
    for (k, (hero, hunter)) in d.heroes.iter().zip(&d.headhunters).enumerate() {
        let x = -55 * (k as i32 + 1);
        assert_eq!((hero[0], hunter[0]), (x, x), "{k}");
    }

    // Stance (`STATUSBUTTONS`, 5) and formation (9): buttons from `resourcen_icons.tga` (24 px without, 32 px with
    // the status bar, O-280); the formation buttons of the row are spaced 32 apart, so also with 32-pixel tiles
    // without overlap.
    assert_eq!(d.stance.len(), 5);
    assert_eq!(d.formation.len(), 9);
    assert_eq!(d.formation[0], [169, 13]);
    for pair in d.formation[1..].windows(2) {
        assert_eq!(pair[1][0] - pair[0][0], 32);
        assert_eq!(pair[1][1], pair[0][1]);
        assert!(pair[1][0] - pair[0][0] >= 32);
    }
    assert_eq!(d.portrait.len(), 11);
    assert_eq!(d.portrait[..2], [[-2, 48], [10, 152]]);

    // Ten colors, as the file assigns them (nothing falls back to the fallback color, O-273 has no effect).
    assert_eq!(COLOR_COUNT, 10);
    assert_eq!(d.colors[2], 0xEEF5_562E);
}

#[test]
fn toolbar_sheets_have_the_documented_tile_sizes() {
    let Some(vfs) = open() else { return };
    let d = ToolbarData::load(&vfs, &mut TileGrids::new());

    // Portrait and icon sheets: 6 and 13 files, all present, tiles 80 x 80 and 50 x 50.
    assert_eq!(d.portrait_sheets.len(), 6);
    assert_eq!(d.icon_sheets.len(), 13);
    for (sheets, tile) in [(&d.portrait_sheets, 80), (&d.icon_sheets, 50)] {
        for s in sheets {
            assert_eq!(s.tile, [tile, tile], "{}", s.path);
            let [w, h] = image_size(&vfs, &s.path);
            assert!(w >= tile && h >= tile, "{} {w}x{h}", s.path);
        }
    }

    // In the shipped data no file appears under both `PORTRAIT` and `ICONS` (the load order within `icons.ini`
    // is therefore irrelevant, O-280).
    assert!(
        d.portrait_sheets
            .iter()
            .all(|p| d.icon_sheets.iter().all(|i| i.path != p.path))
    );

    // `resourcen_icons.tga` (O-280): without the status bar the grid of the bar is 24 x 24; with the status bar its
    // `ICONSIZE 32 32`, because it loads the file first (`0x004492BC` before `0x00449342`).
    assert_eq!(d.resource_icons.path, RESOURCE_ICONS_PATH);
    assert_eq!(d.resource_icons.tile, [24, 24]);
    let status = parse_statusbar(&decode_cp1252(&vfs.read(STATUSBAR_PATH).unwrap()));
    assert_eq!(status.icon_file, RESOURCE_ICONS_PATH, "same name");
    let mut grids = TileGrids::new();
    grids.register(&status.icon_file, status.icon_size);
    let with_status = ToolbarData::load(&vfs, &mut grids);
    assert_eq!(with_status.resource_icons.tile, [32, 32]);
    let [w, h] = image_size(&vfs, RESOURCE_ICONS_PATH);
    for tile in [24, 32] {
        let (cols, rows) = (w / tile, h / tile);
        // Stance and formation take tile 15; it must lie within the image in both grids.
        assert!(
            cols > 0 && 15 < cols * rows,
            "{w}x{h}: {cols}x{rows} tiles of {tile}"
        );
        let (x, y) = (15 % cols * tile, 15 / cols * tile);
        assert!(x + tile <= w && y + tile <= h);
    }

    // `info_icons16x16.tga` with 16 x 16: at least the 4 resource icons and the mana icon.
    assert_eq!(d.info_icons.path, INFO_ICONS_PATH);
    assert_eq!(d.info_icons.tile, [16, 16]);
    let [w, h] = image_size(&vfs, INFO_ICONS_PATH);
    assert!((w / 16) * (h / 16) >= 5, "{w}x{h}");

    // `counterupgrade.tga`: one image 64 x 64.
    assert_eq!(d.counter_overlay.path, COUNTER_OVERLAY_PATH);
    assert_eq!(d.counter_overlay.tile, [64, 64]);
    assert_eq!(image_size(&vfs, COUNTER_OVERLAY_PATH), [64, 64]);
}

/// O-263: The reimplementation omits a button with a negative icon where the original aborts. The shipped data has
/// no negative value; the strict and the clamping lookup return the same for every GUID.
#[test]
fn icons_ini_has_no_negative_icon_in_the_installation() {
    let Some(vfs) = open() else { return };
    let d = ToolbarData::load(&vfs, &mut TileGrids::new());
    assert_eq!(d.icons.guids.len(), 743);
    for (&guid, e) in &d.icons.guids {
        assert!(e.icon_sheet >= 0 && e.icon >= 0, "GUID {guid}: icon {e:?}");
        assert!(
            e.portrait_sheet >= 0 && e.portrait >= 0,
            "GUID {guid}: portrait {e:?}"
        );
        assert_eq!(d.icon_of(guid), d.icon_clamped(guid), "GUID {guid}");
        assert_eq!(
            d.icon_of(guid),
            Some((e.icon_sheet as usize, e.icon)),
            "GUID {guid}"
        );
    }
}

/// O-227 in the shipped data: The colonist (menu 1100) shows "Mount" (403) in slot 4 and "Arm" (485, locked without a
/// weapon smithy) in slot 12; "Dismount" (405) and "Disarm" (486) come after them and never appear, because locks do
/// not let later entries move up. Empty stocks: 485 is locked, but present.
#[test]
fn colonist_menu_never_shows_405_and_486() {
    let Some(vfs) = open() else { return };
    let text =
        |path: &str| decode_cp1252(&vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")));
    let data = ToolbarData::load(&vfs, &mut TileGrids::new());
    let menus = parse_toolbar(&text(TOOLBAR_PATH));
    let defines = parse_target_defines(&text(TARGETDEFINES_PATH));
    let targets = parse_targets(&text(TARGETS_PATH), &defines);
    let mut model = ToolbarModel::new(&menus);
    // `ToolbarModel::targets` is the **active** target list `GameWorld+0x354` (O-281), which the host sets each frame
    // from the MY filters of the selection. Deliberately every entry of `targets.ini` is in it here, i.e. the case that
    // all filters are met: only then is 403 "Mount" (TARGET, slot 4) active and the question about O-227 meaningful
    // (405 does not move up even then). A lone selected colonist in the test world of nml-client has only
    // {400, 412, 413, 414, 448}, without 403 (test `occupant_button_415_needs_an_occupant`). In the reference run
    // (a female colonist at the start of the match) slot 4 is empty in the original and in `nml-game`.
    model.targets = targets.entries.iter().map(|e| e.button_msg).collect();
    model.selection = vec![SelectedObject {
        hid: 1,
        guid: 1100,
        own: true,
        neutral: false,
        building: false,
        site: false,
        has_unit_module: true,
        production: None,
        occupants: Vec::new(),
        stance_bits: 0,
    }];
    assert_eq!(choose_menu(&model), MenuChoice::Menu(1100));
    let screen = Screen {
        width: 1024,
        height: 768,
    };
    let set = build_set(&model, 1100, true, &data, screen, None);
    let msgs: Vec<_> = set.normal.iter().flatten().map(|b| b.spec.msg).collect();
    assert_eq!(set.normal[4].as_ref().map(|b| b.spec.msg), Some(403));
    let armed = set.normal[12].as_ref().expect("slot 12 occupied");
    assert_eq!(armed.spec.msg, 485);
    assert!(model.locks_for(&armed.spec).missing_required);
    assert!(!msgs.contains(&405) && !msgs.contains(&486), "{msgs:?}");
    // Slot 0 lies at (245, H - 176 + 17).
    assert_eq!(set.normal[0].as_ref().unwrap().spec.rect[..2], [245, 609]);
}
