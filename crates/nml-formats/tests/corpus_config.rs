//! Reads configuration, player setup, start objects and variant rules of the shipped data the way the
//! simulation loads them.

use nml_formats::config::{
    DEFAULT_DAT_PATH, DROPDOWN_PATH, parse_default_dat, parse_dropdowns, parse_level_setup,
};
use nml_formats::gifcon::{
    DEFAULT_OBJECTS_PATH, UPGRADES_PATH, parse_default_objects, parse_variant_rules,
};
use nml_formats::targets::{
    FLOWCONTROL_TARGETS_PATH, PRIORITIES_PATH, TARGETDEFINES_PATH, TARGETS_PATH,
    parse_target_defines, parse_targets,
};
use nml_formats::text::decode_cp1252;
use nml_formats::wld::parse_world;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use nml_rda::{Vfs, normalize_path};

#[test]
fn configuration_files_parse_completely() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let text =
        |path: &str| decode_cp1252(&vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")));
    let dropdowns = parse_dropdowns(&text(DROPDOWN_PATH));
    assert_eq!(
        dropdowns.lists.keys().copied().collect::<Vec<_>>(),
        [0, 1, 16, 18, 21, 34, 40]
    );
    let defaults = parse_default_dat(&text(DEFAULT_DAT_PATH), &dropdowns).unwrap();
    // 512 object types and 232 upgrades in one table.
    assert_eq!(defaults.len(), 744);
    assert_eq!(defaults[&4200][&18], 16);

    let mut players = 0;
    let mut human = 0;
    let mut object_configs = 0;
    for lvl in vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".lvl"))
    {
        let setup = parse_level_setup(&text(&lvl), &defaults, &dropdowns)
            .unwrap_or_else(|e| panic!("{lvl}: {e}"));
        players += setup.players.len();
        human += setup.players.iter().filter(|p| p.kind == 0).count();
        for p in &setup.players {
            // Every player sets gold, wood and food itself.
            let mut set: Vec<usize> = p.resources.iter().map(|r| r.0).collect();
            set.sort_unstable();
            set.dedup();
            assert_eq!(set, [0, 1, 2], "{lvl}: player {}", p.slot);
            assert!((0..6).contains(&p.nation), "{lvl}");
        }
        let world = parse_world(&text(&lvl.replace(".lvl", ".wld"))).unwrap();
        object_configs += world
            .objects
            .iter()
            .filter(|o| !o.config.is_empty())
            .count();
    }
    assert_eq!((players, human), (243, 108));
    assert!(object_configs > 0);

    let objects = parse_default_objects(&text(DEFAULT_OBJECTS_PATH), &dropdowns).unwrap();
    assert!(objects.complete);
    // Game types 0-5 x 6 nations, each HQ at (0, 0); supply only list 0.
    assert_eq!(objects.start.len(), 36);
    assert_eq!(objects.start.values().map(Vec::len).sum::<usize>(), 186);
    assert!(
        objects
            .start
            .values()
            .all(|l| l.iter().any(|o| (o.dx, o.dz) == (0, 0)))
    );
    assert!(objects.supply.keys().all(|&(list, _)| list == 0));

    let rules = parse_variant_rules(&text(UPGRADES_PATH));
    let mut bits: Vec<i32> = rules.iter().map(|r| r.bit).collect();
    bits.sort_unstable();
    bits.dedup();
    assert_eq!(bits, (0..=13).collect::<Vec<_>>());

    let defines = parse_target_defines(&text(TARGETDEFINES_PATH));
    assert_eq!(defines.entries.len(), 212);
    for (path, count) in [
        (TARGETS_PATH, 77),
        (FLOWCONTROL_TARGETS_PATH, 73),
        (PRIORITIES_PATH, 12),
    ] {
        let file = parse_targets(&text(path), &defines);
        assert_eq!(file.aborted, None, "{path}");
        assert_eq!(file.entries.len(), count, "{path}");
        assert!(file.entries.iter().all(|e| e.enabled), "{path}");
    }
}

/// `toolbar.ini`, `guidgroups.ini` and `resourcenconfig.ini` as the simulation loads them.
#[test]
fn economy_control_files_parse_like_the_game() {
    use nml_formats::gifcon::{
        GUID_GROUPS_PATH, RESOURCE_CONFIG_PATH, TOOLBAR_PATH, button_kind, parse_guid_groups,
        parse_resource_config, parse_toolbar,
    };
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let text =
        |path: &str| decode_cp1252(&vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")));
    let toolbar = parse_toolbar(&text(TOOLBAR_PATH));
    let buttons: Vec<_> = toolbar.menus.values().flatten().collect();
    let count = |kind: u32| buttons.iter().filter(|b| b.kind() == kind).count();
    let menus = toolbar.menus.len();
    let required: usize = buttons
        .iter()
        .map(|b| b.required.iter().filter(|&&g| g != 0).count())
        .sum();
    // 743 menus without duplicate GUID, 1155 buttons, 446 distinct BUTTONMSG.
    assert_eq!(
        (menus, buttons.len(), toolbar.entries.len()),
        (743, 1155, 446)
    );
    // The counts of the format doc (89 BUILD_MSG, 100 MAKE_MSG, 169 UPGRADE_MSG) are lines; the kind is the OR
    // of the flags: the 6 HQ buttons with BUILD_MSG and ONLYONCE_MSG have the kind ONLYONCE. MAKEONLY_ONCE_MSG
    // (18) never occurs together with MAKE_MSG.
    assert_eq!(count(button_kind::BUILD) + count(button_kind::ONLYONCE), 89);
    assert_eq!(count(button_kind::ONLYONCE), 6);
    assert_eq!(count(button_kind::MAKE), 100);
    assert_eq!(count(button_kind::UPGRADE), 169);
    assert_eq!(count(button_kind::MAKEONLY_ONCE), 18);
    assert_eq!(required, 502);
    // The HQ is unique.
    assert_eq!(toolbar.lookup(1000).unwrap().kind(), button_kind::ONLYONCE);
    let groups = parse_guid_groups(&text(GUID_GROUPS_PATH));
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[&1114], [1113]);
    assert_eq!(groups[&3411], [3412, 3413]);
    let resources = parse_resource_config(&text(RESOURCE_CONFIG_PATH));
    let start: Vec<_> = resources
        .start_records()
        .map(|r| (r.config_guid, r.values))
        .collect();
    assert_eq!(
        start,
        [
            (0, [2000, 500, 1500, 3000]),
            (1, [2000, 500, 1500, 3000]),
            (2, [2000, 500, 1500, 3000])
        ]
    );
    for (guid, cfg, values) in [
        (3500, 26, [2000, 3000, 5000, 8000]),
        (3503, 24, [2000, 3000, 5000, 8000]),
        (3505, 24, [250, 250, 500, 750]),
    ] {
        let r = resources.object_record(guid).unwrap();
        assert_eq!((r.config_guid, r.values), (cfg, values), "{guid}");
    }
}

// QUIRK O-115: kept
#[test]
fn mouse_pointer_seven_is_the_normal_pointer() {
    use nml_formats::gifcon::{MOUSE_POINTER_PATH, POINTER_NORMAL, parse_mouse_pointers};
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let pointers = parse_mouse_pointers(&decode_cp1252(&vfs.read(MOUSE_POINTER_PATH).unwrap()));
    // Twelve pointers 0 ... 11 (re/game-ui 2.11), one file per variant, all in the VFS.
    assert_eq!(
        pointers.keys().copied().collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>()
    );
    for (id, p) in &pointers {
        assert_eq!(
            (p.normal.len(), p.red.len(), p.green.len()),
            (1, 1, 1),
            "pointer {id}"
        );
        for file in p.normal.iter().chain(&p.red).chain(&p.green) {
            assert!(vfs.exists(file), "pointer {id}: {file} missing");
        }
    }
    let normal = &pointers[&POINTER_NORMAL];
    assert_eq!(normal.normal, ["data/pointer/pointer-normal.tga"]);
    assert_eq!(normal.offset, [2, 2]);
}

/// Text of a file from the game directory; `None` if there is no installation.
fn gifcon_text(path: &str) -> Option<String> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    Some(decode_cp1252(
        &vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")),
    ))
}

// QUIRK O-307: kept
#[test]
fn corpus_keys_ini_has_652_lines_and_the_known_commands() {
    use nml_formats::keys_ini::{KEYS_INI_PATH, parse_keys_ini};
    let Some(text) = gifcon_text(KEYS_INI_PATH) else {
        return;
    };
    let lines = parse_keys_ini(&text);
    assert_eq!(lines.len(), 652);
    let count = |f: &dyn Fn(&str) -> bool| lines.iter().filter(|l| f(&l.command)).count();
    assert_eq!(count(&|c| c.starts_with("/game/toolbar/button(")), 488);
    assert_eq!(count(&|c| c.starts_with("/game/hm/selectnextguid(")), 131);
    // `button(` with a leading `x`/`X` (toolbar check skipped), of which one uppercase `Xbutton(530)`.
    assert_eq!(
        count(
            &|c| c.starts_with("/game/toolbar/xbutton(") || c.starts_with("/game/toolbar/Xbutton(")
        ),
        29
    );
    assert_eq!(count(&|c| c == "/game/toolbar/Xbutton(530)"), 1);
    assert_eq!(count(&|c| c == "/game/quicksave()"), 1);
    assert_eq!(count(&|c| c == "/game/quickload()"), 1);
    assert_eq!(count(&|c| c.starts_with("/game/changegamespeed(")), 2);
    // The commands cover all 652 lines.
    assert_eq!(488 + 131 + 29 + 1 + 1 + 2, 652);
    let kinds = |k: i32| lines.iter().filter(|l| l.kind == k).count();
    assert_eq!((kinds(0), kinds(1), kinds(2), kinds(3)), (143, 0, 214, 295));
    assert_eq!(lines.iter().filter(|l| l.selection != 0).count(), 446);
}

// QUIRK O-307: kept
#[test]
fn corpus_default_keys_have_no_edge_spaces() {
    use nml_formats::keys::{DEFAULT_KEYS_PATH, parse_default_keys};
    let Some(text) = gifcon_text(DEFAULT_KEYS_PATH) else {
        return;
    };
    // The original does not trim the values, `parse_default_keys` does: no consequence in the shipped data
    // because no line has whitespace at the edge.
    for line in text.lines().filter(|l| l.contains('=')) {
        let (id, value) = line.split_once('=').unwrap();
        assert_eq!(id, id.trim(), "{line:?}");
        assert_eq!(value, value.trim(), "{line:?}");
        assert!(!line.starts_with(char::is_whitespace), "{line:?}");
    }
    let keys = parse_default_keys(&text);
    assert_eq!(keys.len(), 460);
    assert_eq!(keys.iter().filter(|(_, k)| k.is_empty()).count(), 10);
}

// QUIRK O-307: kept
#[test]
fn corpus_keys_ini_against_default_keys() {
    use nml_formats::keys::{DEFAULT_KEYS_PATH, parse_default_keys};
    use nml_formats::keys_ini::{KEYS_INI_PATH, parse_keys_ini};
    use std::collections::BTreeSet;
    let (Some(keys_text), Some(default_text)) =
        (gifcon_text(KEYS_INI_PATH), gifcon_text(DEFAULT_KEYS_PATH))
    else {
        return;
    };
    let lines = parse_keys_ini(&keys_text);
    let assigned: BTreeSet<i32> = parse_default_keys(&default_text)
        .into_iter()
        .filter(|(_, key)| !key.is_empty())
        .map(|(id, _)| id)
        .collect();
    // O-307: lines without a binding (can never fire) and bindings without a line (tooltip shows `[KEYID]`).
    let mut no_binding: Vec<i32> = lines
        .iter()
        .map(|l| l.action)
        .filter(|a| !assigned.contains(a))
        .collect();
    assert_eq!(no_binding.len(), 9);
    no_binding.sort_unstable();
    assert_eq!(
        no_binding,
        [494, 1409, 1713, 1914, 2011, 2211, 2311, 2611, 3424]
    );
    let with_line: BTreeSet<i32> = lines.iter().map(|l| l.action).collect();
    let no_line: Vec<i32> = assigned.difference(&with_line).copied().collect();
    assert_eq!(no_line, [448, 449, 450, 451, 1911]);
}

#[test]
fn corpus_keymap_has_the_kd_names() {
    use nml_formats::keymap_ini::{KEYMAP_INI_PATH, parse_keymap_ini};
    let Some(text) = gifcon_text(KEYMAP_INI_PATH) else {
        return;
    };
    let map = parse_keymap_ini(&text);
    let get = |name: &str| {
        map.iter()
            .find(|(n, _)| n == name)
            .map(|(_, chord)| chord.as_str())
    };
    for (name, chord) in [
        ("KD_CAM_MOVE_FORWARD", "UP"),
        ("KD_CAM_MOVE_BACKWARD", "DOWN"),
        ("KD_CAM_MOVE_LEFT", "LEFT"),
        ("KD_CAM_MOVE_RIGHT", "RIGHT"),
        ("KD_CAM_YROT_CLOCKWISE", "END"),
        ("KD_CAM_YROT_ANTICLOCKWISE", "DELETE"),
        ("KD_CAM_ZOOM_IN", "NEXT"),
        ("KD_CAM_ZOOM_OUT", "PRIOR"),
        ("KD_CHECK_LBUTTON", "LBUTTON"),
        ("KD_ADD_TO_GROUP", "LSHIFT"),
        ("KD_HARDCOPY", "SNAPSHOT"),
        ("KD_STOP_DIALOG", "ESCAPE"),
        ("KD_SHOW_CONSOLE", "GRAVE"),
        ("KD_SPEEDUP_GAME", "TAB"),
        ("KD_CAM_FREEHAND", "MBUTTON"),
    ] {
        assert_eq!(get(name), Some(chord), "{name}");
    }
    // No tilting by key, no quest menu (O-305).
    assert!(map.iter().all(|(n, _)| !n.starts_with("KD_CAM_XROT")));
    assert_eq!(get("KD_CHECK_QUESTMENU"), None);
    assert_eq!(map.len(), 18);
}

#[test]
fn corpus_bindings2_has_36_bindings() {
    use nml_formats::bindings_ini::{BINDINGS_INI_PATH, parse_bindings};
    let Some(text) = gifcon_text(BINDINGS_INI_PATH) else {
        return;
    };
    let bindings = parse_bindings(&text);
    assert_eq!(bindings.len(), 36);
    let command = |chord: &str| {
        bindings
            .iter()
            .find(|b| b.chord == chord)
            .map(|b| b.command.clone())
    };
    for n in 0..10 {
        assert_eq!(
            command(&format!("CONTROL {n}")),
            Some(format!("/game/storegroup({n})"))
        );
        assert_eq!(
            command(&n.to_string()),
            Some(format!("/game/selectgroup({n})"))
        );
        assert_eq!(
            command(&format!("SHIFT {n}")),
            Some(format!("/game/addtogroup({n})"))
        );
    }
    for (chord, expected) in [
        ("HOME", "/setcamera"),
        ("RETURN", "/game/showchatwindow(1)"),
        ("SHIFT RETURN", "/game/showchatwindow(2)"),
        ("CONTROL RETURN", "/game/showchatwindow(3)"),
        ("CONTROL P", "/game/togglepause"),
        ("PAUSE", "/game/togglepause"),
    ] {
        assert_eq!(command(chord).as_deref(), Some(expected), "{chord}");
    }
}

// QUIRK O-115: kept
#[test]
fn corpus_mouse_pointers_all_twelve() {
    use nml_formats::gifcon::{MOUSE_POINTER_PATH, parse_mouse_pointers};
    use nml_formats::tga::decode_tga_game;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let pointers = parse_mouse_pointers(&decode_cp1252(&vfs.read(MOUSE_POINTER_PATH).unwrap()));
    assert_eq!(
        pointers.keys().copied().collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>()
    );
    let hotspots: [[i32; 2]; 12] = [
        [14, 16],
        [12, 11],
        [20, 16],
        [16, 16],
        [15, 15],
        [15, 14],
        [2, 2],
        [2, 2],
        [17, 14],
        [13, 13],
        [14, 21],
        [22, 12],
    ];
    let mut umlaut_found = false;
    for (id, p) in &pointers {
        assert_eq!(p.offset, hotspots[*id as usize], "pointer {id}");
        // Three variants, exactly one file each.
        for (variant, files) in [("normal", &p.normal), ("red", &p.red), ("green", &p.green)] {
            assert_eq!(files.len(), 1, "pointer {id} {variant}");
            let file = &files[0];
            umlaut_found |= file.contains('ä');
            let bytes = vfs
                .read(file)
                .unwrap_or_else(|e| panic!("pointer {id} {variant}: {file}: {e}"));
            // 32 bits per pixel (header byte 16), 64 x 64.
            assert_eq!(bytes[16], 32, "{file}");
            let image = decode_tga_game(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(image.dimensions(), (64, 64), "{file}");
        }
    }
    // The name with `ä` (cp1252, pointer 6 "Dämpfen") is found.
    assert!(umlaut_found);
    assert!(vfs.exists(&pointers[&6].normal[0]));
}

/// `statusbar.ini`, `toolbar.cfg` and `icons.ini` are in `NML0.RDA` under the paths of the game; they are
/// layout descriptions without language texts, the same in both editions.
fn game_ui_text(path: &str) -> Option<String> {
    let dir = corpus_game_dir()?;
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    Some(decode_cp1252(
        &vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}")),
    ))
}

// QUIRK O-38: kept
#[test]
fn installation_statusbar_matches_the_documented_layout() {
    use nml_formats::statusbar::{STATUSBAR_PATH, StatusEntry, parse_statusbar};
    let Some(text) = game_ui_text(STATUSBAR_PATH) else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let c = parse_statusbar(&text);
    assert_eq!(
        (c.size, c.icon_size, c.icon_file.as_str()),
        (26, [32, 32], "data/icons/resourcen_icons.tga")
    );
    // Background `STATUSCOLOR 2 22 35` (b g r) and white font.
    assert_eq!((c.color, c.font_color), (0x8023_1602, 0xFFFF_FFFF));
    // re/game-ui 2.2: four resources and the player name in columns at a spacing of 120 (icon at
    // x = 1/120/240/360, text 32 or 33 pixels behind it at y = 5; the name has no icon and begins at x = 480
    // with the same icon and text start).
    assert_eq!(
        c.entries,
        [
            StatusEntry {
                index: 0,
                icon: [1, 1],
                text: [33, 5]
            },
            StatusEntry {
                index: 1,
                icon: [120, 1],
                text: [153, 5]
            },
            StatusEntry {
                index: 2,
                icon: [240, 1],
                text: [273, 5]
            },
            StatusEntry {
                index: 3,
                icon: [360, 1],
                text: [393, 5]
            },
            StatusEntry {
                index: -1,
                icon: [480, 1],
                text: [480, 5]
            },
        ]
    );
}

#[test]
fn installation_toolbar_cfg_has_the_known_tags() {
    use nml_formats::toolbar_cfg::{TOOLBAR_CFG_PATH, parse_toolbar_cfg};
    let Some(text) = game_ui_text(TOOLBAR_CFG_PATH) else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let c = parse_toolbar_cfg(&text);
    // 6 sheets 128 x 128 (`status_a_00 ... 05`) and 11 sheets 64 x 64 (`status_b_01 ... 11`).
    assert_eq!(c.sheets.len(), 17);
    assert!(c.sheets[..6].iter().all(|(_, size)| *size == [128, 128]));
    assert!(c.sheets[6..].iter().all(|(_, size)| *size == [64, 64]));
    assert_eq!(c.sheets[0].0, "data/icons/status_a_00.tga");
    assert_eq!(c.sheets[16].0, "data/icons/status_b_11.tga");
    let names: Vec<&str> = c.tags.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "BUTTONNORMALCOLORS",
            "FORMATIONBUTTONS",
            "HEADHUNTERBUTTONS",
            "HEROBUTTONS",
            "NORMALBUTTONS",
            "PORTRAITPOSITION",
            "SELECTIONBUTTONS",
            "STATUSBUTTONS",
            "TOOLBAR"
        ]
    );
    // Each block has only one kind of lines (the original puts them in a common list).
    for (name, tag) in &c.tags {
        let kinds = [
            !tag.icons.is_empty(),
            !tag.positions.is_empty(),
            !tag.colors.is_empty(),
        ];
        assert_eq!(kinds.iter().filter(|&&k| k).count(), 1, "{name}");
    }
    // `TOOLBAR`: 17 background images, each from its own sheet, tile 0.
    let toolbar = &c.tags["TOOLBAR"];
    assert_eq!(toolbar.icons.len(), 17);
    assert!(
        toolbar
            .icons
            .iter()
            .enumerate()
            .all(|(i, icon)| icon.sheet == i as i32 && icon.tile == 0)
    );
    assert_eq!(toolbar.icons[0].pos, [-2, -128]);
    // `NORMALBUTTONS`: 2 rows of 7, grid 53 px, rows at y = 17 and 71.
    let normal = &c.tags["NORMALBUTTONS"].positions;
    assert_eq!(normal.len(), 14);
    for (k, pos) in normal.iter().enumerate() {
        let k = k as i32;
        assert_eq!(
            *pos,
            [245 + 53 * (k % 7), if k < 7 { 17 } else { 71 }],
            "{k}"
        );
    }
    assert_eq!(c.tags["BUTTONNORMALCOLORS"].colors.len(), 10);
    assert_eq!(c.tags["BUTTONNORMALCOLORS"].colors[2], 0xEEF5_562E);
    assert_eq!(c.tags["HEROBUTTONS"].positions.len(), 5);
    assert_eq!(c.tags["HEADHUNTERBUTTONS"].positions.len(), 5);
    assert_eq!(c.tags["SELECTIONBUTTONS"].positions.len(), 30);
    assert_eq!(c.tags["STATUSBUTTONS"].positions.len(), 5);
    assert_eq!(c.tags["FORMATIONBUTTONS"].positions.len(), 9);
    let portrait = &c.tags["PORTRAITPOSITION"].positions;
    assert_eq!(portrait.len(), 11);
    assert_eq!(portrait[..2], [[-2, 48], [10, 152]]);
}

#[test]
fn installation_icons_ini_has_nation_portraits() {
    use nml_formats::icons_ini::{ICONS_INI_PATH, parse_icons_ini};
    let Some(text) = game_ui_text(ICONS_INI_PATH) else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let c = parse_icons_ini(&text);
    assert_eq!(c.portrait_sheets.len(), 6);
    assert_eq!(c.icon_sheets.len(), 13);
    assert_eq!(c.portrait_sheets[0], "data/icons/patrioten_icons_80x80.tga");
    // Nation icons: GUID 500 + nation (re/game-ui 2.1).
    for guid in 500..=505 {
        let icons = c.guids.get(&guid).unwrap_or_else(|| panic!("GUID {guid}"));
        assert!((0..6).contains(&icons.portrait_sheet), "GUID {guid}");
        assert!((0..13).contains(&icons.icon_sheet), "GUID {guid}");
    }
    assert_eq!(c.guids[&500].portrait_sheet, 2);
    assert_eq!(c.guids[&500].portrait, 30);
    // The 13 values (GUID 20 ... 32) point to sheet 4 of the portrait, tile 3 of the icon.
    for guid in 20..=32 {
        assert!(c.guids.contains_key(&guid), "GUID {guid}");
    }
    // 743 `GUID` lines without a duplicate GUID.
    assert_eq!(c.guids.len(), 743);
}

// QUIRK O-48: kept
#[test]
fn installation_cinemascope_ini_matches_d6_1() {
    use nml_formats::cinemascope_ini::{CINEMASCOPE_INI_PATH, DialogArea, parse_cinemascope_ini};
    let Some(text) = game_ui_text(CINEMASCOPE_INI_PATH) else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let c = parse_cinemascope_ini(&text);
    assert_eq!(c.bar_size, 170);
    let top = c.bar_top.as_ref().expect("BARTEXTURETOP");
    assert_eq!(
        (top.tile, top.path.as_str()),
        ([32, 32], "data/icons/cinemascope_icon_oben.tga")
    );
    let down = c.bar_down.as_ref().expect("BARTEXTUREDOWN");
    assert_eq!(
        (down.tile, down.path.as_str()),
        ([32, 32], "data/icons/cinemascope_icon_unten.tga")
    );
    assert_eq!(c.areas.len(), 1);
    assert_eq!(
        c.area0(),
        Some(DialogArea {
            portrait_pos: [24, 876],
            portrait_size: [128, 128],
            text_pos: [170, 880],
            text_size: [933, 124],
            text_font: "DialogText".into(),
            text_color: 0xFFFF_FFFF,
        })
    );
}

#[test]
fn installation_portraits_ini_has_1000_and_frames_100_to_102() {
    use nml_formats::portraits_ini::{
        PORTRAIT_FRAMES_INI_PATH, PORTRAITS_INI_PATH, parse_portraits_ini,
    };
    let (Some(portraits), Some(frames)) = (
        game_ui_text(PORTRAITS_INI_PATH),
        game_ui_text(PORTRAIT_FRAMES_INI_PATH),
    ) else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let portraits = parse_portraits_ini(&portraits);
    let frames = parse_portraits_ini(&frames);
    assert!(portraits.contains_key(&1000), "portrait 1000");
    assert!(portraits.values().all(|p| p.starts_with("data/portraits/")));
    for id in 100..=102 {
        assert!(frames.contains_key(&id), "frame {id}");
    }
    // Frames and portraits share no IDs; one frame per nation (100-107).
    assert!(frames.keys().all(|id| (100..=107).contains(id)));
    assert!(portraits.keys().all(|id| *id >= 1000));
    // Measured numbers of the German edition (reference): portraits 1000-1070, frames 100-107. The English
    // edition is not measured and only checks the basic form above.
    let vfs = Vfs::open_game_dir(corpus_game_dir().unwrap()).unwrap();
    if game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) == GameVersion::German {
        assert_eq!(frames.len(), 8);
        assert_eq!(
            portraits.keys().copied().collect::<Vec<_>>(),
            (1000..=1070).collect::<Vec<_>>()
        );
    }
}
