//! Menu formats of the original installation (`data/menü/`). Skipped if it is missing.

use std::collections::{BTreeMap, BTreeSet};

use nml_formats::bkg::{Slot, parse_bkg, write_bkg};
use nml_formats::dialog_map::{parse_dialog_map, write_dialog_map};
use nml_formats::dlg::desc::label_flags;
use nml_formats::dlg::{
    AppCommand, ElementKind, ElementPart, ExpandOptions, Item, WidgetDesc, WidgetKind, expand,
    parse_dlg, write_dlg,
};
use nml_formats::localisation::{FontSize, parse_localisation, parse_text_file};
use nml_formats::messages::{decode_text_file, parse_text_table};
use nml_formats::text::decode_cp1252;
use nml_formats::tga::decode_tga_game;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};

/// Counts `LOOP`, `VAR` and `ADD` in the source text (not after loop resolution).
fn count_macros(items: &[Item], counts: &mut [usize; 3]) {
    for item in items {
        match item {
            Item::Loop { body, .. } => {
                counts[0] += 1;
                count_macros(body, counts);
            }
            Item::Var { .. } => counts[1] += 1,
            Item::Add { .. } => counts[2] += 1,
            Item::Element(element) => {
                for part in &element.parts {
                    if let ElementPart::Child(child) = part {
                        count_macros(std::slice::from_ref(child), counts);
                    }
                }
            }
            Item::Stray(_) => {}
        }
    }
}

#[test]
fn all_dlg_files_round_trip_without_diagnostics() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    // `Vfs::files()` can name a path from several archives: once per path, reading goes through the VFS.
    let paths: BTreeSet<String> = vfs
        .files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.starts_with("data/menü/") && p.ends_with(".dlg"))
        .collect();
    assert_eq!(paths.len(), 63, "number of .dlg files under data/menü/");

    let mut counts = [0usize; 3];
    let mut failures = Vec::new();
    for path in &paths {
        let bytes = vfs.read(path).unwrap();
        let (doc, diags) = parse_dlg(&bytes);
        if write_dlg(&doc) != bytes {
            failures.push(format!("{path}: writing is not byte-identical"));
        }
        if let Some(d) = diags.first() {
            failures.push(format!(
                "{path}: {} diagnostic(s), e.g. line {}: {}",
                diags.len(),
                d.line,
                d.message
            ));
        }
        match doc.items.as_slice() {
            [Item::Element(e)] if e.kind == ElementKind::Frame => {}
            other => failures.push(format!(
                "{path}: expected exactly one top-level FRAME, found {} entries",
                other.len()
            )),
        }
        count_macros(&doc.items, &mut counts);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // `docs/formats/dlg.md`, section "Corpus finding": macros in the source text.
    assert_eq!(counts, [13, 54, 55], "LOOP, VAR, ADD");
}

/// Alle Widgets in Baumreihenfolge.
fn flatten<'a>(widgets: &'a [WidgetDesc], out: &mut Vec<&'a WidgetDesc>) {
    for widget in widgets {
        out.push(widget);
        flatten(&widget.children, out);
    }
}

/// Text ID of a widget (`TITLE`, `TEXT`).
fn text_id(kind: &WidgetKind) -> Option<i32> {
    match kind {
        WidgetKind::Frame(f) => f.title,
        WidgetKind::Button(b) | WidgetKind::ComboBox(b, _) => b.text,
        WidgetKind::ListBox(_) => None,
        WidgetKind::EditField(e) => e.text,
        WidgetKind::Slider(s) => s.text,
        WidgetKind::RadioButton(r) => r.text,
        WidgetKind::Label(l) => l.text,
    }
}

/// File references of a widget (`BACKGROUND`/`LEVELBACKGROUND` following the background logic, `MUSIC`, `BKGFILE`,
/// `CREDITS`).
fn paths(kind: &WidgetKind) -> Vec<&str> {
    match kind {
        WidgetKind::Frame(f) => [&f.background, &f.music, &f.bkg_file]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect(),
        WidgetKind::Label(l) => l.credits.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Counts the element kinds, text IDs and samples of an evaluation of the whole corpus.
#[derive(Default)]
struct CorpusStats {
    kinds: BTreeMap<&'static str, usize>,
    text_uses: usize,
    combos_without_pressed: usize,
    combos_with_default_list_disabled: usize,
    combos_without_selection: usize,
    sliders_with_resolution_10: usize,
    labels_background_without_color: usize,
}

impl CorpusStats {
    fn add(&mut self, widget: &WidgetDesc) {
        let kind = match &widget.kind {
            WidgetKind::Frame(_) => "FRAME",
            WidgetKind::Button(_) => "BUTTON",
            WidgetKind::ComboBox(..) => "COMBOBOX",
            WidgetKind::ListBox(_) => "LISTBOX",
            WidgetKind::EditField(_) => "EDITFIELD",
            WidgetKind::Slider(_) => "SLIDER",
            WidgetKind::RadioButton(_) => "RADIOBUTTON",
            WidgetKind::Label(_) => "LABEL",
        };
        *self.kinds.entry(kind).or_default() += 1;
        self.text_uses += usize::from(text_id(&widget.kind).is_some());
        match &widget.kind {
            WidgetKind::ComboBox(b, l) => {
                self.combos_without_pressed += usize::from(b.pressed_text_color.is_none());
                self.combos_with_default_list_disabled +=
                    usize::from(l.disabled_text_color == 0xFFA0_A0A0);
                self.combos_without_selection += usize::from(l.cur_selection == -1);
            }
            WidgetKind::Slider(s) => {
                self.sliders_with_resolution_10 += usize::from(s.resolution == 10)
            }
            WidgetKind::Label(l) => {
                self.labels_background_without_color += usize::from(
                    l.flags & label_flags::BACKGROUND != 0 && l.background_color.is_none(),
                )
            }
            _ => {}
        }
    }
}

#[test]
fn all_dialogs_expand_to_the_documented_counts() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let dlg_paths: BTreeSet<String> = vfs
        .files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.starts_with("data/menü/") && p.ends_with(".dlg"))
        .collect();
    assert_eq!(dlg_paths.len(), 63);

    for is_menu_level in [false, true] {
        let opts = ExpandOptions { is_menu_level };
        let mut failures = Vec::new();
        let mut stats = CorpusStats::default();
        let mut expanded = BTreeMap::new();
        for path in &dlg_paths {
            let bytes = vfs.read(path).unwrap();
            let (doc, _) = parse_dlg(&bytes);
            let out = expand(&doc, opts);
            if let Some(d) = out.diagnostics.first() {
                failures.push(format!("{path}: line {}: {}", d.line, d.message));
            }
            let mut all = Vec::new();
            flatten(&out.widgets, &mut all);
            for widget in &all {
                stats.add(widget);
                for p in paths(&widget.kind) {
                    if !vfs.exists(p) {
                        failures.push(format!("{path}: {} refers to missing {p}", widget.name));
                    }
                }
            }
            for p in &out.preload {
                if !vfs.exists(p) {
                    failures.push(format!("{path}: PRELOAD refers to missing {p}"));
                }
            }
            for command in &out.commands {
                if let AppCommand::LoadLevelBackground(p) = command
                    && !vfs.exists(p)
                {
                    failures.push(format!("{path}: LEVELBACKGROUND refers to missing {p}"));
                }
            }
            expanded.insert(path.clone(), out);
        }
        assert!(
            failures.is_empty(),
            "IsMenuLevel {is_menu_level}:\n{}",
            failures.join("\n")
        );
        // `docs/formats/dlg.md`, section "Corpus finding" (named pairs).
        let expected: BTreeMap<&str, usize> = [
            ("FRAME", 63),
            ("BUTTON", 141),
            ("COMBOBOX", 114),
            ("LABEL", 250),
            ("RADIOBUTTON", 58),
            ("SLIDER", 18),
            ("EDITFIELD", 17),
            ("LISTBOX", 15),
        ]
        .into_iter()
        .collect();
        assert_eq!(stats.kinds, expected, "IsMenuLevel {is_menu_level}");
        // 408 text ID uses after resolution (existence is checked by `dialog_text_ids_exist`).
        assert_eq!(stats.text_uses, 408, "IsMenuLevel {is_menu_level}");
        // O-9: no combobox sets PRESSEDBUTTONTEXTCOLOR; no label sets MENULABEL_BACKGROUND without a color.
        assert_eq!(stats.combos_without_pressed, 114);
        assert_eq!(stats.labels_background_without_color, 0);
        // O-10: DISABLEDTEXTCOLOR of the combobox never reaches the list.
        assert_eq!(stats.combos_with_default_list_disabled, 114);
        // Combobox default CURSELECTION -1: no combobox preselects an entry (some spell out -1).
        assert_eq!(stats.combos_without_selection, 114);
        // All 18 sliders set RESOLUTION themselves (8 x 10, 9 x 20, 1 x 100); the default 10 (FUN_00465070)
        // is checked by `dlg::expand::tests::defaults_follow_the_original`.
        assert_eq!(stats.sliders_with_resolution_10, 8);

        // Sample start.dlg (per mode).
        let start = &expanded["data/menü/start.dlg"];
        assert_eq!(start.widgets.len(), 1);
        let frame = &start.widgets[0];
        assert_eq!(frame.name, "START");
        let WidgetKind::Frame(f) = &frame.kind else {
            panic!("START is not a FRAME");
        };
        assert_eq!(f.extent, [-440, 212, 0, 912]);
        assert_eq!(f.target, [40, 212]);
        assert_eq!(f.duration_ms, 300);
        assert_eq!(f.bar_extent, [0, 140, 550, 225]);
        assert_eq!(f.title_extent, [-490, 50, -50, 185]);
        assert_eq!(f.title_color, 0xDDFF_F5BF);
        assert_eq!(f.title, Some(15099));
        assert_eq!(f.music.as_deref(), Some("DATA/MUSIC/TITLE.OGG"));
        let level = "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL";
        if is_menu_level {
            assert_eq!(f.flags, 0x40 | 0x80 | 0x4 | 0x1000 | 0x2 | 0x8);
            assert_eq!(f.background.as_deref(), Some(level));
            assert_eq!(
                start.commands,
                [AppCommand::LoadLevelBackground(level.to_string())]
            );
        } else {
            assert_eq!(f.flags, 0x40 | 0x1 | 0x4 | 0x1000 | 0x2 | 0x8);
            assert_eq!(
                f.background.as_deref(),
                Some("DATA/MENÜ/BACKGROUND_1024X1024_PAT.TGA")
            );
            assert_eq!(start.commands, [AppCommand::HideLevelBackground]);
        }

        // Sample account0.dlg: nested loop, name with two counters, EXTENT from XPOS/YPOS.
        let mut all = Vec::new();
        flatten(&expanded["data/menü/account0.dlg"].widgets, &mut all);
        let value = all
            .iter()
            .find(|w| w.name == "ACCOUNT0VALUE17")
            .expect("ACCOUNT0VALUE17");
        let WidgetKind::Label(l) = &value.kind else {
            panic!("ACCOUNT0VALUE17 is not a LABEL");
        };
        // XPOS = 295 + 1*(140 + 10), YPOS = 100 + 7*50, YSIZE 39.
        assert_eq!(l.extent, [445, 450, 585, 489]);
    }
}

/// All paths in the archives with the extension (normalized, once per path).
fn corpus_paths(vfs: &Vfs, suffix: &str) -> BTreeSet<String> {
    vfs.files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.ends_with(suffix))
        .collect()
}

#[test]
fn all_bkg_files_round_trip() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let paths = corpus_paths(&vfs, ".bkg");
    assert_eq!(paths.len(), 13, "number of .bkg files");
    for must in [
        "data/gifcon/editfield.bkg",
        "data/gifcon/infoframe.bkg",
        "data/menü/menuframe.bkg",
    ] {
        assert!(paths.contains(must), "{must} is missing");
    }
    let mut failures = Vec::new();
    for path in &paths {
        let bytes = vfs.read(path).unwrap();
        let file = parse_bkg(&bytes);
        if write_bkg(&file) != bytes {
            failures.push(format!("{path}: writing is not byte-identical"));
        }
        let desc = file.desc();
        if desc.files.is_empty() {
            failures.push(format!("{path}: no FILE line"));
        }
        // Load the textures and span the grid `TexB/b x TexH/h` (`docs/formats/dlg.md`, section ".bkg"; corpus finding:
        // no invalid file/tile indices, no missing textures).
        let mut grids = Vec::new();
        for tex in &desc.files {
            match vfs.read(&tex.path).map(|b| decode_tga_game(&b)) {
                Ok(Ok(img)) if tex.tile_w > 0 && tex.tile_h > 0 => {
                    let cols = img.width() as i32 / tex.tile_w;
                    let rows = img.height() as i32 / tex.tile_h;
                    grids.push(cols * rows);
                }
                other => {
                    failures.push(format!(
                        "{path}: texture {} not loadable ({other:?})",
                        tex.path
                    ));
                    grids.push(0);
                }
            }
        }
        let refs = desc
            .sequence
            .iter()
            .flatten()
            .chain(desc.slots.iter().flatten());
        for r in refs {
            let ok = usize::try_from(r.file)
                .ok()
                .and_then(|f| grids.get(f))
                .is_some_and(|&n| (0..n).contains(&r.tile));
            if !ok {
                failures.push(format!("{path}: tile {r:?} outside the grid"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    let frame = parse_bkg(&vfs.read("data/menü/menuframe.bkg").unwrap()).desc();
    assert_eq!(frame.client_area, [15, 30, 15, 30]);
    // `slider_horz.bkg` (TL, TOPMID, TR) and `slider_vert.bkg` (TL, LEFTMID, BL) each yield one bar.
    let used = |path: &str| -> Vec<Slot> {
        let desc = parse_bkg(&vfs.read(path).unwrap()).desc();
        Slot::ALL
            .into_iter()
            .filter(|s| desc.slots[s.index()].is_some())
            .collect()
    };
    assert_eq!(
        used("data/menü/slider_horz.bkg"),
        [Slot::TopLeft, Slot::TopMid, Slot::TopRight]
    );
    assert_eq!(
        used("data/menü/slider_vert.bkg"),
        [Slot::TopLeft, Slot::LeftMid, Slot::BottomLeft]
    );
}

#[test]
fn dialog_maps_have_60_entries() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let read = |path: &str| {
        let bytes = vfs.read(path).unwrap();
        let map = parse_dialog_map(&bytes);
        assert_eq!(write_dialog_map(&map), bytes, "{path}: writing");
        map.entries()
    };
    let map1 = read("data/menü/dialogs.map");
    let map2 = read("data/menü/dialogs2.map");
    assert_eq!(map1.len(), 60);
    assert_eq!(map2.len(), 60);

    // Key = FRAME name of the target file; exactly GAMEPLAYOPTIONS3 has no file (O-13).
    let mut missing = Vec::new();
    for (key, path) in &map1 {
        if !vfs.exists(path) {
            missing.push((key.as_str(), path.as_str()));
            continue;
        }
        let (doc, _) = parse_dlg(&vfs.read(path).unwrap());
        let out = expand(&doc, ExpandOptions::default());
        assert_eq!(out.widgets[0].name, *key, "{path}: FRAME-Name");
    }
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert_eq!(missing[0].0, "GAMEPLAYOPTIONS3");

    // dialogs2.map: same keys (different order), exactly four different paths.
    let by_key1: BTreeMap<&str, &str> =
        map1.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let by_key2: BTreeMap<&str, &str> =
        map2.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        by_key1.keys().collect::<Vec<_>>(),
        by_key2.keys().collect::<Vec<_>>()
    );
    let differing: Vec<&str> = by_key1
        .iter()
        .filter(|(k, v)| by_key2[*k] != **v)
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(
        differing,
        ["OPTIONS", "SELECTCAMPAIGN", "SINGLEPLAYERMENU", "START"]
    );
}

#[test]
fn localisation_deu_defines_the_menu_fonts() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    for lang in ["deu", "eng", "fra"] {
        let l = parse_localisation(&decode_cp1252(
            &vfs.read(&format!("data/txt/localisation.{lang}")).unwrap(),
        ));
        assert_eq!(l.text_files.len(), 6, "{lang}: text files");
        assert_eq!(l.fonts.len(), 18, "{lang}: fonts");
        assert_eq!(l.icons.len(), 8, "{lang}: icon sets");
        assert!(l.blood_mode, "{lang}");
        for f in &l.fonts {
            assert!(f.texture_font, "{lang}: {} without TextureFont", f.name);
        }
        // The text files of the other languages (`data\txt_ENG\...`) do not belong to the German installation.
        if lang == "deu" {
            for t in &l.text_files {
                assert!(vfs.exists(t), "{lang}: {t} is missing");
            }
        }
    }

    let l = parse_localisation(&decode_cp1252(
        &vfs.read("data/txt/localisation.deu").unwrap(),
    ));
    let font = |name: &str| l.fonts.iter().find(|f| f.name == name).unwrap();
    let title = font("MenuTitle");
    assert_eq!((title.size, title.weight), (FontSize::Relative(26), 0));
    let button = font("MenuButton");
    assert_eq!((button.size, button.weight), (FontSize::Relative(27), 600));
    assert_eq!(button.face, "Garamond");
    let credits = font("Credits");
    assert_eq!(credits.size, FontSize::Relative(28));
    assert_eq!(credits.face, "Times New Roman");
    assert_eq!(font("ConsoleFont").size, FontSize::Fixed(16));
    assert_eq!(font("PanelInfo").size, FontSize::Fixed(15));

    // O-64: INFOICONS points to the resource icons and vice versa - the data is read unchanged.
    let icon = |name: &str| l.icons.iter().find(|i| i.name == name).unwrap();
    assert_eq!(
        icon("INFOICONS").path,
        "data/icons/resourcen_icons16x16.tga"
    );
    assert_eq!(icon("INFOICONS").size, [16, 16]);
    assert_eq!(
        icon("RESOURCENICONS").path,
        "data/icons/info_icons16x16.tga"
    );
    assert_eq!(icon("rdlogo").size, [256, 130]);
}

#[test]
fn text_files_parse_like_the_old_reader() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let l = parse_localisation(&decode_cp1252(
        &vfs.read("data/txt/localisation.deu").unwrap(),
    ));
    assert_eq!(l.text_files.len(), 6);
    for path in &l.text_files {
        let bytes = vfs.read(path).unwrap();
        let new = parse_text_file(&bytes).unwrap_or_else(|| panic!("{path}: no BOM"));
        // The game's reader (re/ui-rendering 3.8.6) and the former reader (`messages::parse_text_table`)
        // agree on the shipped data; their differences (line without a number, line splitting, `\r` as
        // separator) do not occur in the shipped data.
        let old = parse_text_table(&decode_text_file(&bytes));
        assert_eq!(new, old, "{path}");
        assert!(!new.is_empty(), "{path}");
    }
}

#[test]
fn dialog_text_ids_exist() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let l = parse_localisation(&decode_cp1252(
        &vfs.read("data/txt/localisation.deu").unwrap(),
    ));
    // The game fills a single table in order (`FUN_005601B0` per `TextDatei`).
    let mut texts: BTreeMap<i32, String> = BTreeMap::new();
    for path in &l.text_files {
        texts.extend(parse_text_file(&vfs.read(path).unwrap()).unwrap());
    }
    let dlg_paths = corpus_paths(&vfs, ".dlg");
    let dlg_paths: Vec<&String> = dlg_paths
        .iter()
        .filter(|p| p.starts_with("data/menü/"))
        .collect();
    assert_eq!(dlg_paths.len(), 63);
    for is_menu_level in [false, true] {
        let mut uses = 0;
        let mut missing = Vec::new();
        for path in &dlg_paths {
            let (doc, _) = parse_dlg(&vfs.read(path).unwrap());
            let out = expand(&doc, ExpandOptions { is_menu_level });
            let mut all = Vec::new();
            flatten(&out.widgets, &mut all);
            for widget in all {
                if let Some(id) = text_id(&widget.kind) {
                    uses += 1;
                    if !texts.contains_key(&id) {
                        missing.push(format!("{path}: {} -> text {id}", widget.name));
                    }
                }
            }
        }
        assert_eq!(uses, 408, "IsMenuLevel {is_menu_level}");
        assert!(missing.is_empty(), "{}", missing.join("\n"));
    }
}
