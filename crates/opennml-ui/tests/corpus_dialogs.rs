//! Corpus tests against the original installation (`NML_GAME_DIR`, otherwise fallback path; without an installation
//! they silently skip themselves).

use std::collections::{BTreeMap, BTreeSet};

use opennml_formats::dlg::{ExpandOptions, WidgetDesc, WidgetKind, expand, parse_dlg};
use opennml_rda::testutil::corpus_game_dir;
use opennml_rda::{Vfs, normalize_path};
use opennml_ui::assets::{Texts, UiAssets, load_localisation};
use opennml_ui::fonts::{AtlasSource, FontBook, relative_px};
use opennml_ui::text::{LayoutParams, Run, RunContent, TextLayout, layout_text};

fn open() -> Option<Vfs> {
    let dir = corpus_game_dir()?;
    Some(Vfs::open_game_dir(dir).expect("installation cannot be opened"))
}

/// Every font from `localisation.deu` has its atlas in the installation at the widths 1024 and 1280
/// (re/ui-rendering 3.3), so without fallback and without generating.
// QUIRK O-58: kept
#[test]
fn all_menu_fonts_load() {
    let Some(vfs) = open() else { return };
    let loc = load_localisation(&vfs).expect("localisation loads");
    assert_eq!(loc.fonts.len(), 18);
    for width in [1024, 1280] {
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, width, &mut assets, None);
        for def in &loc.fonts {
            let font = book.font(book.by_name(&def.name));
            assert_eq!(font.name, def.name);
            assert_eq!(
                font.source,
                AtlasSource::Installed,
                "{} at width {width} ({} px) needs a fallback",
                def.name,
                font.requested_px
            );
            assert_eq!(font.px, font.requested_px, "{}", def.name);
            assert!(font.max_height > 0, "{}", def.name);
            assert!(font.cell[0] > 0 && font.cell[1] > 0, "{}", def.name);
            assert!(
                font.advance.iter().any(|&a| a > 0),
                "{} has no glyphs",
                def.name
            );
            // Size according to the formula for relative fonts.
            if let opennml_formats::localisation::FontSize::Relative(n) = def.size {
                assert_eq!(font.requested_px, relative_px(width, n), "{}", def.name);
            }
        }
        assert!(assets.missing().is_empty(), "{:?}", assets.missing());
    }
}

/// The icon sets load too, and `TextFont` is the 16-px Garamond.
#[test]
fn icon_sets_and_text_font_load() {
    let Some(vfs) = open() else { return };
    let loc = load_localisation(&vfs).unwrap();
    let mut assets = UiAssets::new();
    let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
    for def in &loc.icons {
        let set = book.icon_set(&def.name);
        assert_eq!(set.size, def.size, "{}", def.name);
        assert!(assets.image(set.texture).width() > 0, "{}", def.name);
    }
    let text_font = book.font(book.by_name("TextFont"));
    assert_eq!(text_font.px, 16);
}

/// The text tables of the language load and contain menu texts.
#[test]
fn texts_load_from_the_installation() {
    let Some(vfs) = open() else { return };
    let loc = load_localisation(&vfs).unwrap();
    let texts = Texts::load(&vfs, &loc);
    assert_eq!(loc.text_files.len(), 6);
    let present = (0..30000).filter(|&id| texts.get(id).is_some()).count();
    assert!(present > 1000, "only {present} texts");
}

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

/// All 408 dialog texts lay out with `MenuButton` (width 1024, 21 px) and the flags `0x46` at 400 px without a
/// panic. Every line stays within 400 px except for the overhang per O-80: a word that wraps after others stands
/// alone at the start of the line and sticks out. The shipped data uses only known tags, so no tag appears
/// literally.
// QUIRK O-77, O-80: kept
#[test]
fn menu_texts_lay_out() {
    let Some(vfs) = open() else { return };
    let loc = load_localisation(&vfs).unwrap();
    let texts = Texts::load(&vfs, &loc);
    let mut assets = UiAssets::new();
    let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
    let font = book.by_name("MenuButton");
    assert_eq!(book.font(font).px, 21);
    let params = LayoutParams {
        flags: 0x46,
        width: 400,
        font,
        color: 0xFFFF_FFFF,
    };
    let paths: BTreeSet<String> = vfs
        .files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.starts_with("data/menü/") && p.ends_with(".dlg"))
        .collect();
    assert_eq!(paths.len(), 63);
    let (mut uses, mut lines, mut overhangs, mut wrapped) = (0, 0, 0, 0);
    for path in &paths {
        let (doc, _) = parse_dlg(&vfs.read(path).unwrap());
        let out = expand(
            &doc,
            ExpandOptions {
                is_menu_level: false,
            },
        );
        let mut all = Vec::new();
        flatten(&out.widgets, &mut all);
        for widget in all {
            let Some(id) = text_id(&widget.kind) else {
                continue;
            };
            uses += 1;
            let text = texts
                .get(id)
                .unwrap_or_else(|| panic!("{path}: text {id} is missing"));
            let mut layout = TextLayout::default();
            layout_text(text, &params, &book, &mut layout);
            // Same input, same layout.
            let mut again = TextLayout::default();
            layout_text(text, &params, &book, &mut again);
            assert_eq!(layout, again);
            let mut by_line: BTreeMap<i32, Vec<&Run>> = BTreeMap::new();
            for run in &layout.runs {
                by_line.entry(run.y).or_default().push(run);
                if let RunContent::Text(codes) = &run.content {
                    assert!(
                        !codes.starts_with(b"["),
                        "{path} {}: tag literal in {text:?}",
                        widget.name
                    );
                }
            }
            lines += by_line.len();
            if by_line.len() > 1 {
                wrapped += 1;
            }
            for line in by_line.values() {
                let end = line.iter().map(|r| r.x + r.w).max().unwrap();
                if end > 400 {
                    overhangs += 1;
                    assert!(
                        line.len() == 1 && line[0].x == 0,
                        "{path} {}: line {end} px wide without an O-80 overhang: {line:?} in {text:?}",
                        widget.name
                    );
                }
            }
        }
    }
    assert_eq!(uses, 408);
    eprintln!("{uses} texts, {lines} lines, {wrapped} wrapped, {overhangs} overhangs per O-80");
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing all dialogs statically
// ---------------------------------------------------------------------------------------------------------------

use opennml_formats::localisation::Localisation;
use opennml_ui::coords::Screen;
use opennml_ui::dialog::{DialogLibrary, UiContext, paint_static, placements};
use opennml_ui::draw::{DrawCmd, draw_checksum};

const fn screen(width: i32, height: i32) -> Screen {
    Screen { width, height }
}

/// Installation, language, texts and the library with `is_menu_level = false` (like the reference images).
fn library() -> Option<(Vfs, Localisation, Texts, DialogLibrary)> {
    let vfs = open()?;
    let loc = load_localisation(&vfs).expect("localisation loads");
    let texts = Texts::load(&vfs, &loc);
    let library = DialogLibrary::load(&vfs, &texts, false);
    Some((vfs, loc, texts, library))
}

/// Paints every dialog of the library individually at `size`; fonts once per size (O-58), one image collection for
/// all dialogs. Returns (name, commands) and the image collection.
fn paint_all(
    vfs: &Vfs,
    loc: &Localisation,
    texts: &Texts,
    library: &DialogLibrary,
    size: Screen,
) -> (Vec<(String, Vec<DrawCmd>)>, UiAssets) {
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(vfs, loc, size.width, &mut assets, None);
    let mut out = Vec::new();
    for name in library.names() {
        let dialog = library.find(&name).expect("dialog present");
        let mut ctx = UiContext {
            vfs,
            assets: &mut assets,
            fonts: &fonts,
            texts,
            screen: size,
        };
        out.push((name, paint_static(&[dialog], &mut ctx)));
    }
    (out, assets)
}

/// All 63 dialogs paint at 1024 x 768 and 1280 x 1024 without a panic, each with at least one command, all
/// images are loaded (`UiAssets::missing` empty) and no text ID is missing; the evaluation reports nothing.
// QUIRK O-4: kept
#[test]
fn all_dialogs_paint() {
    let Some((vfs, loc, texts, library)) = library() else {
        return;
    };
    assert_eq!(library.names().len(), 63);
    assert!(
        library.diagnostics().is_empty(),
        "{:?}",
        library.diagnostics()
    );
    for name in library.names() {
        let mut all = Vec::new();
        flatten(std::slice::from_ref(library.find(&name).unwrap()), &mut all);
        for widget in all {
            if let Some(id) = text_id(&widget.kind) {
                assert!(
                    texts.get(id).is_some(),
                    "{name} {}: text {id} is missing",
                    widget.name
                );
            }
        }
    }
    for size in [screen(1024, 768), screen(1280, 1024)] {
        let (painted, assets) = paint_all(&vfs, &loc, &texts, &library, size);
        for (name, cmds) in &painted {
            assert!(!cmds.is_empty(), "{name} at {size:?}: no commands");
        }
        assert!(
            assets.missing().is_empty(),
            "{size:?}: {:?}",
            assets.missing()
        );
    }
    // The map entry without a file reports itself with an error message (O-13).
    assert_eq!(
        library.find("gameplayoptions3").unwrap_err(),
        "dialog \"GAMEPLAYOPTIONS3\": file data/menü/gameplayoptions3.dlg is missing"
    );
}

/// Measured; if a value changes, look for the cause.
/// (Name, checksum at 1024 x 768, checksum at 1280 x 1024; `draw_checksum`.)
/// `LAN3` was once measured anew with its own selection field `TURNRASTER` (`0x0396C9A080807B98`,
/// `0xEABF2517B09B21E1`); the field was taken out again (it covered the right halves of the switches), so `LAN3`
/// has its original value again. All other values are unaffected.
const DIALOG_DRAW_GOLDEN: [(&str, u64, u64); 63] = [
    ("ACCOUNT0", 0xF01C852DCA6896C3, 0xDF3A75BD74F31E24),
    ("ACCOUNT1", 0x21C1E06EDDDEDABB, 0x91F7B5DF733C0A64),
    ("ACCOUNT2", 0xA1340007E1F9D6FB, 0x34813F02BCA9C817),
    ("ACCOUNTING", 0x9DC79803E34230FE, 0xEDB4D46FF27A61DA),
    ("CAMPAIGNLEVEL", 0x8FF490982B45F7C2, 0x9A573B82D0CCD66C),
    ("CAMPAIGNLEVEL2", 0xD3C98CED5BA8E8CA, 0xF26676053C29CECA),
    ("CAMPAIGNVIDEO", 0x8CE0E172E54516A5, 0x9AF616890C2FC6B1),
    ("CHATFRAME", 0x3B297D68B244AE95, 0x4A15DCBAE0605C62),
    ("CHATWINDOW", 0x2D255A4402541E9E, 0xC61974B2AF8EAEC1),
    ("CHATWINDOW2", 0x95CC97B0AFDA5005, 0x5A62019A3D96B4C0),
    ("CHATWINDOW3", 0xB1E1299F2E6EDE02, 0xAE83510393EC08A9),
    ("CONTROLOPTIONS", 0x3750B99B1DA14C3A, 0xC66E25F13EE8D842),
    (
        "CREATESINGLEPLAYERGAME",
        0x7507E1B7EBD88B3F,
        0x0C8FD2C84E2CB1EB,
    ),
    ("CREDITS", 0x1F78C7575FD0257C, 0xC57B8B22950B759C),
    ("DIPLOMATIE", 0x47914F798B478903, 0x2D0493C546BF0DDC),
    ("GAMEEND_DEFEAT", 0xB3457B6A1A912CCA, 0xCFAAEBF67A25721A),
    ("GAMEEND_VICTORY", 0xB40DE7E3C11B8846, 0xAE4AB0200C3BE58C),
    ("GAMEPLAYOPTIONS", 0xF6EE41E43E07EB83, 0xCDE8A05ADC9F63B0),
    ("GAMEPLAYOPTIONS2", 0x250E6DF330E198A7, 0x03578BBB1B287440),
    ("INFOFRAME", 0xB1583324AB512A21, 0x42D81107E8A501C2),
    ("INGAMELOADGAME", 0x8BFA6A2CBF99B205, 0x737F9652D9093B23),
    ("INGAMEOPTIONS", 0xA3B78CF0012D3181, 0x5C01A1C5060A2201),
    ("INGAMEOPTIONS2", 0x7F71C19E117A8B17, 0x195A4FA3AC51690B),
    ("INGAMEOPTIONS3", 0x84CF7A464DECA273, 0xACF8282BB1AC1FE8),
    ("KAMPAGNE0", 0x35A6EAB707315377, 0x6328E8EC17A01C7A),
    ("KAMPAGNE1", 0x1F847ECCEEC6DB86, 0x2957BF2539D7E343),
    ("KAMPAGNE2", 0xD494E4C8470A7C06, 0xA7D0E4AC451FD5A6),
    ("KAMPAGNE3", 0xF614A6C8647C8307, 0x6705B1FFE1A625DA),
    ("KAMPAGNEN", 0x91C0EF9EBF08F7D2, 0x5861ABE1ACC1A717),
    ("LAN1", 0x12767B3CF49D3979, 0x3AD7DD6F1DFACA06),
    ("LAN2", 0xAC5BC4D7C32F0153, 0x21658A0D549BD073),
    ("LAN3", 0x40573DB96D059352, 0xB60E0FF6BD18D03A),
    ("LAN31", 0xFA7D271D1FEDC9C8, 0x53A552C50D4E8BE2),
    ("LAN4", 0x5A84D74949EADC5B, 0x603E8AB8A6A06426),
    ("LEVELINFO", 0x3709923E980B5921, 0x176CBC9B60DE848C),
    ("LOADSAVEDGAME", 0x27B01B88228914BA, 0x484CEE88D0C5FC46),
    ("LOADSAVEDGAME2", 0xAF4D8D4111D89332, 0x91521373E6A4276D),
    ("LOADAUTOPLAY", 0x8F2D894097C967CA, 0xEF90E6025AE765AA),
    ("LOADMULTIPLAYERMAP", 0x4C47626D7679C90F, 0x14FF0BEDDB57075E),
    (
        "LOADSAVEDNETWORKGAME",
        0x0BACE101D09FA762,
        0x2640DB440C618FB3,
    ),
    ("LOGO", 0xBEA5BFC3D81BC1D4, 0x35C7376DF4721419),
    ("MSGBOX", 0xFA9B92BBBA0E560E, 0xC15D2363B88BCFC5),
    ("OPTIONS", 0xBB9523FE875E4F8D, 0xF6696B74ED987610),
    ("OPTIONS2", 0xE81D1FB535F36B37, 0x5C51E154343DA8BA),
    ("QUESTMENU", 0x73E0E7F5858965EA, 0xC0DBD02267D61752),
    ("QUESTMENU2", 0x73E0E7F5858965EA, 0xC0DBD02267D61752),
    ("RANDOMMAP", 0x9F83A4AFED0FE0DE, 0x4E064936DB361040),
    ("SAVEGAME", 0x3860E3A1CB883CCC, 0xCF7DD45C4A1C4D05),
    ("SAVINGMESSAGE", 0xB6E8C5D20F4898F3, 0x12A5ACB1E661498E),
    ("SELECTCAMPAIGN", 0xFD68E860148A5774, 0xFAC5D354EE12F3BF),
    ("SELECTCAMPAIGN2", 0x93D28B5BA0781E14, 0x4E23249E23064CC5),
    ("SELECTPROFILE", 0xE126A82F8E15A82E, 0x24CA767679CCC50D),
    ("SINGLEPLAYER", 0xE9F6AFA32955BAE3, 0x261B21056A31C4CC),
    ("SINGLEPLAYERMENU", 0x60D515357653BCC0, 0xF96C43DB65D4FF0A),
    ("SINGLEPLAYERMENU2", 0x2670325B05BD128E, 0x6B4F797573C6DABA),
    ("SINGLEPLAYERSTART", 0x9826CABCDB2A7C26, 0xC32715410F127374),
    ("SOUNDOPTIONS", 0x969F40E321A1D6A4, 0x7232A00EBE91CF3B),
    ("SOUNDOPTIONS2", 0x7204B36EA2556F81, 0x89D533674C80990C),
    ("SOUNDOPTIONS3", 0x7204B36EA2556F81, 0x89D533674C80990C),
    ("START", 0xD0493C832B0DD293, 0x9CD2E5FBF140E88A),
    ("START2", 0x46E6F8D7CFA0FB3A, 0xD6C621F11FE8650E),
    ("TIPLOGMENU", 0x3505FF9412BA2458, 0xB48103613EB70D16),
    ("VIDEOOPTIONS", 0xB1236E7BCD8A34AA, 0x7452F7E6A036BB67),
];

/// The command lists of all dialogs are stable: painted twice the same and equal to the measured checksums.
#[test]
fn dialog_draw_checksums_are_stable() {
    let Some((vfs, loc, texts, library)) = library() else {
        return;
    };
    let sums = |size| {
        let (painted, assets) = paint_all(&vfs, &loc, &texts, &library, size);
        painted
            .into_iter()
            .map(|(name, cmds)| (name, draw_checksum(&cmds, &assets)))
            .collect::<Vec<_>>()
    };
    let small = sums(screen(1024, 768));
    let large = sums(screen(1280, 1024));
    assert_eq!(small, sums(screen(1024, 768)), "differs when painted twice");
    let got: Vec<(String, u64, u64)> = small
        .iter()
        .zip(&large)
        .map(|((name, a), (_, b))| (name.clone(), *a, *b))
        .collect();
    let want: Vec<(String, u64, u64)> = DIALOG_DRAW_GOLDEN
        .iter()
        .map(|&(n, a, b)| (n.to_string(), a, b))
        .collect();
    if got != want {
        let table: String = got
            .iter()
            .map(|(n, a, b)| format!("    (\"{n}\", 0x{a:016X}, 0x{b:016X}),\n"))
            .collect();
        panic!("checksums differ; measured:\n{table}");
    }
}

/// Unusual sizes (width < 800, widescreen, 4K): no panic, menus stretched.
// QUIRK O-57: kept
#[test]
fn all_dialogs_paint_at_odd_sizes() {
    let Some((vfs, loc, texts, library)) = library() else {
        return;
    };
    for size in [screen(640, 480), screen(1600, 900), screen(3840, 2160)] {
        let (painted, _) = paint_all(&vfs, &loc, &texts, &library, size);
        assert_eq!(painted.len(), 63);
    }
}

/// Position of `START` at 1024 x 768 as in the reference image of the original.
///
/// The constants come from the formulas and were only confirmed against a reference image:
/// - Frame: `ZIELPOSITION 40 212` + size from `EXTENT -440 212 0 912` = grid (40, 212, 480, 912) -> S = (32, 159,
///   384, 684).
/// - Buttons: `EXTENT 55 y1 345 y2` in the client (inset 15/30 from `menuframe.bkg`) -> grid (110, 242 + y1, 400,
///   242 + y2) -> S.
/// - Text origin of "Single player" (upper left cell of the first glyph, without shadow): `MenuButton` 21 px,
///   x = S.l + (W - line width)/2 + 2, y = S.t + (S.h - th)/2.
///
/// Comparison on the image (Python, brightness profiles): frame and button edges lie up to 1 px further inside than
/// the rectangles (edge smoothing of the filter, grid rule - a matter for the drawer and the reference comparison),
/// the ink of "Single player" begins as in the reimplementation at (154, 220).
// QUIRK O-57: kept
#[test]
fn start_matches_the_reference_layout() {
    let Some((vfs, loc, texts, library)) = library() else {
        return;
    };
    let size = screen(1024, 768);
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(&vfs, &loc, size.width, &mut assets, None);
    let start = library.find("start").unwrap();
    let mut ctx = UiContext {
        vfs: &vfs,
        assets: &mut assets,
        fonts: &fonts,
        texts: &texts,
        screen: size,
    };
    let places = placements(&[start], &mut ctx);
    let screen_of = |name: &str| {
        places
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is missing"))
            .screen
    };
    assert_eq!(screen_of("START"), [32, 159, 384, 684]);
    for (name, rect) in [
        ("STARTSINGLEPLAYERMENU", [88, 197, 320, 257]),
        ("STARTLAN1", [88, 272, 320, 332]),
        ("STARTOPTIONS", [88, 347, 320, 407]),
        ("STARTCREDITS", [88, 422, 320, 482]),
        ("STARTGAMESPY", [88, 497, 320, 557]),
        ("STARTQUIT", [88, 594, 320, 654]),
    ] {
        assert_eq!(screen_of(name), rect, "{name}");
    }
    let cmds = paint_static(&[start], &mut ctx);
    let atlas = fonts.font(fonts.by_name("MenuButton")).atlas;
    let first_glyph = cmds
        .iter()
        .find_map(|c| match *c {
            DrawCmd::Quad {
                texture: Some(t),
                dst,
                color: 0xDDFF_EABD,
                ..
            } if t == atlas => Some([dst[0], dst[1]]),
            _ => None,
        })
        .expect("text of Single player");
    // Line width 104 (sum of the advances), th = 21 (greatest height of the `.fnt`): x = 88 + (232 - 104)/2 + 2,
    // y = 197 + (60 - 21)/2. In the reference image the ink of "Single player" begins at (154, 220).
    assert_eq!(first_glyph, [154, 216]);
}

/// `LAN3` has no `TURNRASTER` selection field of its own: without covering the switches
/// `SAVEREPLAY`/`LOCKTEAMS`/`MAPEXPLORED` it found no room. All dialogs have exactly the elements of their file; the
/// own texts (99 100 ... for `LEVELINFO`) are still in the table.
#[test]
fn lan3_has_no_turn_raster_element() {
    let Some((vfs, _, texts, library)) = library() else {
        return;
    };
    let (doc, _) = parse_dlg(&vfs.read("data/menü/lan3.dlg").unwrap());
    let original = expand(
        &doc,
        ExpandOptions {
            is_menu_level: false,
        },
    );
    let frame = library.find("LAN3").unwrap();
    let file_frame = original
        .widgets
        .iter()
        .find(|w| w.name == "LAN3")
        .expect("frame LAN3 in the file");
    assert_eq!(frame.children[..], file_frame.children[..]);
    assert_eq!(frame.kind, file_frame.kind);
    assert!(texts.get(99_100).is_some(), "own text inserted");
    for name in library.names() {
        let dialog = library.expanded(&name).expect("dialog");
        let mut all = Vec::new();
        flatten(&dialog.widgets, &mut all);
        assert!(all.iter().all(|w| !w.name.contains("TURNRASTER")), "{name}");
    }
}

/// The search also accepts the file stem: for eight dialogs it differs from the map key (`--dialog load`).
#[test]
fn find_accepts_the_file_stem() {
    let Some((_, _, _, library)) = library() else {
        return;
    };
    for (key, stem) in [
        ("SAVEGAME", "save"),
        ("LOADSAVEDGAME", "load"),
        ("GAMEEND_VICTORY", "gameendvictory"),
        ("GAMEEND_DEFEAT", "gameenddefeat"),
        ("LOADSAVEDNETWORKGAME", "LoadNetWork"),
        ("LOADMULTIPLAYERMAP", "loadmap"),
        ("INGAMELOADGAME", "ingameload"),
        ("LOADSAVEDGAME2", "load2"),
    ] {
        let by_key = library.find(key).unwrap_or_else(|e| panic!("{e}"));
        let by_stem = library.find(stem).unwrap_or_else(|e| panic!("{e}"));
        assert!(std::ptr::eq(by_key, by_stem), "{key} / {stem}");
    }
}

/// O-91: Among the dialogs reachable through `dialogs.map` no file locks a button - static drawing
/// (`button_disabled.bkg`) and run time (set 0 + `DISABLEDTEXTCOLOR`) therefore show the active shipped data the same.
/// Locked by the file there is only the EditField `PROFILENAME` in `singleplayermenu.dlg` (corrected in
/// re/menu-system 12.5); the dead `...2.dlg` have locked buttons.
// QUIRK O-91: kept
#[test]
fn no_active_button_is_disabled_by_file() {
    use opennml_formats::dialog_map::parse_dialog_map;
    use opennml_formats::dlg::desc::common_flags::DISABLE;
    let Some((vfs, _, _, library)) = library() else {
        return;
    };
    let map = parse_dialog_map(&vfs.read("data/menü/dialogs.map").unwrap());
    let disabled = |name: &str| -> Vec<(String, &'static str)> {
        let mut all = Vec::new();
        flatten(std::slice::from_ref(library.find(name).unwrap()), &mut all);
        all.into_iter()
            .filter(|w| w.kind.flags() & DISABLE != 0)
            .map(|w| {
                let kind = match w.kind {
                    WidgetKind::Button(_) | WidgetKind::ComboBox(..) => "button",
                    WidgetKind::EditField(_) => "EditField",
                    _ => "other",
                };
                (w.name.clone(), kind)
            })
            .collect()
    };
    let mut reachable = Vec::new();
    let mut count = 0;
    for (key, _) in map.entries() {
        // GAMEPLAYOPTIONS3 has no file (O-13).
        if library.find(&key).is_err() {
            continue;
        }
        count += 1;
        reachable.extend(disabled(&key));
    }
    assert!(count > 40, "only {count} dialogs reachable");
    assert_eq!(
        reachable,
        [("SINGLEPLAYERMENUPROFILENAME".to_string(), "EditField")]
    );
    for dead in ["OPTIONS2", "SELECTCAMPAIGN2", "SINGLEPLAYERMENU2", "START2"] {
        assert!(
            disabled(dead).iter().any(|(_, kind)| *kind == "button"),
            "{dead}"
        );
    }
}

/// Every dialog flies in at run time without dialog logic and draws itself at 1024 x 768 without a panic (build with
/// `OnAttach`, transition, drawing passes, `paint_ui`). Where the run time shows nothing of its own (no credits, no
/// locked button; lists and input fields are empty without logic, the caret is just off at 2500 ms), the image after
/// the fly-in equals static drawing.
// QUIRK O-91: kept
#[test]
fn runtime_end_state_matches_paint_static() {
    use opennml_ui::msg::NoListener;
    use opennml_ui::paint::{paint_ui, tick_tree};
    use opennml_ui::tree::{FrameMode, UiEnv, UiTree};
    use opennml_ui::widgets::frame::transition;
    let Some((vfs, loc, texts, library)) = library() else {
        return;
    };
    let size = screen(1024, 768);
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(&vfs, &loc, size.width, &mut assets, None);
    let mut differ = Vec::new();
    for name in library.names() {
        let dialog = library.find(&name).unwrap();
        let mut tree = UiTree::new();
        let mut listener = NoListener;
        let mut fx = Vec::new();
        let mut env = UiEnv {
            ui: UiContext {
                vfs: &vfs,
                assets: &mut assets,
                fonts: &fonts,
                texts: &texts,
                screen: size,
            },
            listener: &mut listener,
            fx: &mut fx,
            now: 0,
        };
        let frame = tree.build_dialog(dialog, &mut env);
        let root = tree.dialog_root();
        tree.node_mut(root).visible = true;
        transition(&mut tree, frame, FrameMode::Open, &mut env);
        for now in [0, 16, 100, 299, 300, 1000, 2000, 2500] {
            env.now = now;
            tick_tree(&mut tree, &mut env, &mut |_, _, _| {});
        }
        let mut ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: &fonts,
            texts: &texts,
            screen: size,
        };
        let runtime = paint_ui(&tree, &mut ctx, 2500);
        assert!(!runtime.is_empty(), "{name}");
        if runtime != paint_static(&[dialog], &mut ctx) {
            differ.push(name);
        }
    }
    // Different: the credits scroll (2.5 s), and the dead `...2.dlg` have buttons locked from the file - at run time
    // set 0 instead of `button_disabled.bkg` (O-91).
    assert_eq!(
        differ,
        [
            "CREDITS",
            "OPTIONS2",
            "SELECTCAMPAIGN2",
            "SINGLEPLAYERMENU2",
            "START2"
        ]
    );
}

/// `START` and `OPTIONS` exist in both variants (O-7): with `IsMenuLevel` `0x21` and `.lvl`, without `0x20` and the
/// image; a dialog without `LEVELBACKGROUND` has only one variant.
#[test]
fn library_keeps_both_variants_of_start_and_options() {
    use opennml_formats::dlg::AppCommand;
    let Some((_, _, _, library)) = library() else {
        return;
    };
    for name in ["start", "options"] {
        let with = library.expanded_for(name, true).unwrap();
        let without = library.expanded_for(name, false).unwrap();
        assert_eq!(
            with.commands,
            [AppCommand::LoadLevelBackground(
                "DATA/LEVELS/MENÜLEVEL/MENÜ2.LVL".into()
            )],
            "{name}"
        );
        assert_eq!(
            without.commands,
            [AppCommand::HideLevelBackground],
            "{name}"
        );
        // The library was loaded with `false`: `expanded` is the variant without menu level.
        assert!(std::ptr::eq(library.expanded(name).unwrap(), without));
    }
    let same = |name: &str| {
        std::ptr::eq(
            library.expanded_for(name, true).unwrap(),
            library.expanded_for(name, false).unwrap(),
        )
    };
    assert!(same("msgbox"));
}
