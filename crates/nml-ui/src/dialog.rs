//! Dialog library ([`DialogLibrary`]) and static drawing of opened dialogs in their end position
//! ([`paint_static`]) - no mouse, no open list, no focus, no fly-in, no scrolling.
//!
//! Basis: re/menu-system 5.3/5.4 (opening, desktop image), 8.4 (drawing in detail), 12 (run time),
//! re/ui-rendering 3.8.5 (credits), `docs/formats/dlg.md` (section "Coordinates", element kinds). What the
//! dialog logic sets at run time (combo box entries, images of `MENULABEL_TGA`, texts of `MENULABEL_TEXTARRAY`,
//! locks via `status`) is missing here (a deviation from the original).
//!
//! [`paint_static`] builds a widget tree in the **file state** (`UiTree::build_dialog_static`:
//! end position of the fly-in, only the first queries without a listener, no focus, no scroll bars) and draws it with
//! the same code as the run time ([`crate::paint`]). Deviations from the run-time image: a button locked in the file
//! shows `button_disabled.bkg` here (static file state, `DIALOG_DRAW_GOLDEN`); at run time set 0 stays
//! `button_normal.bkg` and only the text color changes (O-91, re/menu-system 12.5). The desktop images stand
//! fully faded in without a crossfade (`DesktopImages::finish_fades`; at run time they fade in over time,
//! 12.3), and the credits stand at their initial position (scroll position 0, no scrolling, re/ui-rendering 3.8.5).
//!
//! Flow like `FUN_004BCA90` -> `Widget_RenderTree(Desktop)` (8.4.12): per dialog in opening order first the
//! attaching (for the credits the first image change), then the opening (`FUN_004617F0`: with `MOVE` `0x15E` with
//! `BACKGROUND`, otherwise only with `MENUFRAME_BACKGROUND`); the images are fully faded in and drawn in `CMap` order
//! (12.3). `levelbackground.bkg` is drawn by the desktop only while a 3D world is running (`FUN_0045CAF0`), so
//! never here.

use std::collections::BTreeSet;

use nml_formats::dialog_map::parse_dialog_map;
use nml_formats::dlg::desc::frame_flags;
use nml_formats::dlg::{
    Diagnostic, ElementKind, ExpandOptions, Expanded, FrameDesc, WidgetDesc, WidgetKind, expand,
    parse_dlg, upper_word,
};
use nml_rda::{Vfs, normalize_path};

use crate::assets::{Texts, UiAssets};
use crate::coords::{Screen, to_screen_rect};
use crate::draw::DrawCmd;
use crate::fonts::FontBook;
use crate::msg::NoListener;
use crate::paint::{paint_ui, tick_tree, visited};
use crate::text::layout_flags;
use crate::tree::{Broadcast, NodeKind, UiEnv, UiTree};

/// Everything that drawing needs. Images and `.bkg` textures are loaded by drawing into `assets` on demand.
pub struct UiContext<'a> {
    pub vfs: &'a Vfs,
    pub assets: &'a mut UiAssets,
    pub fonts: &'a FontBook,
    pub texts: &'a Texts,
    pub screen: Screen,
}

/// Folder of the menu dialogs (`data/menü/`, `docs/formats/dlg.md`, section "Locations").
const DIALOG_DIR: &str = "data/menü/";
/// The only map that the program loads (`0x006201F0`, `FUN_004DEBB0`).
const DIALOG_MAP: &str = "data/menü/dialogs.map";

/// Fixed `.bkg` files of the widgets (strings `0x00619429`-`0x0061AAED`, all under `data/menü` except
/// `editfield.bkg`).
const FRAME_BKG: &str = "data/menü/menuframe.bkg";
/// Background sets of the button in constructor order 0-4 (`FUN_0045EE80`, re/menu-system 3.5).
pub(crate) const BUTTON_NORMAL_BKG: &str = "data/menü/button_normal.bkg";
pub(crate) const BUTTON_MOUSEOVER_BKG: &str = "data/menü/button_mouseover.bkg";
pub(crate) const BUTTON_PRESSED_BKG: &str = "data/menü/button_pressed.bkg";
pub(crate) const BUTTON_DISABLED_BKG: &str = "data/menü/button_disabled.bkg";
pub(crate) const BUTTON_BORDER_BKG: &str = "data/menü/button_border.bkg";
pub(crate) const COMBO_FRAME_BKG: &str = "data/menü/comboframe.bkg";
pub(crate) const EDITFIELD_BKG: &str = "data/gifcon/editfield.bkg";
pub(crate) const SLIDER_HORZ_BKG: &str = "data/menü/slider_horz.bkg";
pub(crate) const SLIDER_VERT_BKG: &str = "data/menü/slider_vert.bkg";

/// Fonts fixed in the code (`0x0061A4DC` `MenuButton`, credits `0x0061A880`; titles in [`crate::paint`]).
pub(crate) const FONT_BUTTON: &str = "MenuButton";
pub(crate) const FONT_CREDITS: &str = "Credits";

/// Credits: color `0xC8FFFFFF` (`0x00462B9A`).
pub(crate) const CREDITS_COLOR: u32 = 0xC8FF_FFFF;

/// Layout flags of label and credits `0x46` (`0x40|0x04|0x02`).
pub(crate) const LABEL_LAYOUT: u32 =
    layout_flags::WIDTH | layout_flags::START_COLOR | layout_flags::START_FONT;

/// Credits: lines with this prefix note an image change and are not laid out (`0x0061A86C`, comparison
/// `CString::Left(0x12)` + `_mbscmp`, case matters).
const CHANGEBACKGROUND: &[u8] = b"[CHANGEBACKGROUND ";
/// Replacement of an empty credits line (`0x0061A860`): the run of spaces carries the line height.
const CREDITS_EMPTY_LINE: &str = " [CR]";
/// Maximum number of characters per credits line (`FUN_0055F720(..., 0x100)`, `0x00462D39`).
const CREDITS_LINE_MAX: usize = 0x100;

// ---------------------------------------------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------------------------------------------

/// All `.dlg` under `data/menü/` (63 in the shipped data), evaluated with `ExpandOptions { is_menu_level }`.
///
/// - Order = path order (normalized paths, sorted).
/// - Name = key from `dialogs.map` that points to the file (the first one if there are several), otherwise the
///   uppercased file stem (e.g. `START2` for the dead `...2.dlg`, O-13). The search with [`Self::find`]
///   uppercases the name like `FUN_004E24F0` (`FUN_00574EA0`), so it does not distinguish case
///   and takes the file stem as a second key (`load` finds `LOADSAVEDGAME`); the name takes precedence.
/// - On loading, the text IDs of all dialogs are resolved in path and file order through [`Texts::resolve`]
///   (O-4: the shared buffer then shows the most recently missing number; the original resolves per
///   opened dialog, unreachable in the shipped data).
/// - Diagnostics of the evaluation are notes, not load errors: the partial result applies as in the original
///   (`FUN_004E24F0` does not check the return value); [`Self::diagnostics`] reports them.
///
/// Viewer, checksums and reference comparison load with `is_menu_level = false` (like the
/// reference images with `Engine.ini` `IsMenuLevel 0`): `LEVELBACKGROUND` frames with a
/// `BACKGROUND` key then become `MENUFRAME_BACKGROUND` frames with an image (O-7).
#[derive(Debug, Clone, Default)]
pub struct DialogLibrary {
    dialogs: Vec<LibraryDialog>,
    /// Map entries without a file (`GAMEPLAYOPTIONS3`, O-13): (key, normalized path).
    missing: Vec<(String, String)>,
    /// The `is_menu_level` of loading: the variant for [`Self::expanded`] and [`Self::find`].
    default_level: bool,
}

#[derive(Debug, Clone)]
struct LibraryDialog {
    name: String,
    /// Uppercased file stem (`SAVEGAME` -> `SAVE` for `save.dlg`): second search key.
    stem: String,
    path: String,
    /// Evaluation with the `is_menu_level` of loading ([`DialogLibrary::load`]).
    expanded: Expanded,
    /// Evaluation with the other value, only if it differs from `expanded` (files with `LEVELBACKGROUND`):
    /// the original reads the `.dlg` at build time and sees `IsMenuLevel` at that point.
    other: Option<Expanded>,
}

impl LibraryDialog {
    /// The evaluation for `is_menu_level`: the same one if there is only one variant.
    fn variant(&self, default_level: bool, is_menu_level: bool) -> &Expanded {
        match &self.other {
            Some(other) if is_menu_level != default_level => other,
            _ => &self.expanded,
        }
    }
}

impl DialogLibrary {
    /// Reads `dialogs.map` and all `.dlg` (see the type documentation). If the map is missing, only the file stems
    /// apply.
    pub fn load(vfs: &Vfs, texts: &Texts, is_menu_level: bool) -> DialogLibrary {
        let map: Vec<(String, String)> = match vfs.read(DIALOG_MAP) {
            Ok(bytes) => parse_dialog_map(&bytes)
                .entries()
                .into_iter()
                .map(|(key, path)| (key, normalize_path(&path)))
                .collect(),
            Err(e) => {
                log::warn!("Dialog list \"{DIALOG_MAP}\" not readable: {e}");
                Vec::new()
            }
        };
        let paths: BTreeSet<String> = vfs
            .files()
            .map(|(_, entry)| normalize_path(&entry.path))
            .filter(|p| p.starts_with(DIALOG_DIR) && p.ends_with(".dlg"))
            .collect();
        let options = ExpandOptions { is_menu_level };
        let mut dialogs = Vec::new();
        for path in &paths {
            let name = match map.iter().find(|(_, p)| p == path) {
                Some((key, _)) => key.clone(),
                None => upper_word(file_stem(path)),
            };
            let (expanded, other) = match vfs.read(path) {
                Ok(bytes) => {
                    let (doc, _) = parse_dlg(&bytes);
                    let expanded = expand(&doc, options);
                    let other = expand(
                        &doc,
                        ExpandOptions {
                            is_menu_level: !is_menu_level,
                        },
                    );
                    let other = (other != expanded).then_some(other);
                    (expanded, other)
                }
                Err(e) => {
                    log::warn!("Dialog \"{path}\" not readable: {e}");
                    (Expanded::default(), None)
                }
            };
            for diagnostic in &expanded.diagnostics {
                log::warn!(
                    "Dialog \"{name}\" ({path}), line {}: {}",
                    diagnostic.line,
                    diagnostic.message
                );
            }
            // O-4: resolve in file order (pre-order of the tree = order of the keys in the file);
            // the second variant directly after the first.
            for widget in expanded
                .widgets
                .iter()
                .chain(other.iter().flat_map(|o| o.widgets.iter()))
            {
                resolve_texts(widget, texts);
            }
            dialogs.push(LibraryDialog {
                name,
                stem: upper_word(file_stem(path)),
                path: path.clone(),
                expanded,
                other,
            });
        }
        let missing = map
            .into_iter()
            .filter(|(_, path)| !paths.contains(path))
            .collect();
        DialogLibrary {
            dialogs,
            missing,
            default_level: is_menu_level,
        }
    }

    /// Names of the loaded dialogs in path order (without map entries without a file).
    pub fn names(&self) -> Vec<String> {
        self.dialogs.iter().map(|d| d.name.clone()).collect()
    }

    /// The topmost `FRAME` of a dialog: the top-level widget with the name of the dialog (that is how
    /// `FUN_004E24F0` finds it through the name table), otherwise the first `FRAME` or the first widget of the file.
    /// The search ignores case for the name, then for the file stem (`load` finds
    /// `LOADSAVEDGAME`). Errors: unknown name, map entry without a file, file without a widget.
    pub fn find(&self, name: &str) -> Result<&WidgetDesc, String> {
        self.find_for(name, self.default_level)
    }

    /// Like [`Self::find`] for the variant that the build reads with `is_menu_level`.
    pub fn find_for(&self, name: &str, is_menu_level: bool) -> Result<&WidgetDesc, String> {
        let key = upper_word(name);
        // The name (map key) takes precedence over the file stem.
        let found = self
            .dialogs
            .iter()
            .find(|d| d.name == key)
            .or_else(|| self.dialogs.iter().find(|d| d.stem == key));
        if let Some(dialog) = found {
            let widgets = &dialog.variant(self.default_level, is_menu_level).widgets;
            return widgets
                .iter()
                .find(|w| w.name == dialog.name)
                .or_else(|| {
                    widgets
                        .iter()
                        .find(|w| w.kind.element_kind() == ElementKind::Frame)
                })
                .or_else(|| widgets.first())
                .ok_or_else(|| {
                    format!(
                        "dialog \"{}\": {} contains no widget",
                        dialog.name, dialog.path
                    )
                });
        }
        if let Some((key, path)) = self.missing.iter().find(|(k, _)| *k == key) {
            return Err(format!("dialog \"{key}\": file {path} is missing"));
        }
        Err(format!(
            "unknown dialog \"{name}\" (known: {})",
            self.names().join(", ")
        ))
    }

    /// The whole evaluation of the file that the dialog stack builds for `name` (`FUN_004E24F0`: key uppercased in the
    /// map `MenuSystem+0x3C`, `FUN_004E23E0` parses the whole file): all top-level widgets in file order
    /// and the application commands `0x20`/`0x21` from the `FRAME`s ([`Expanded::commands`]). Unlike [`Self::find`]
    /// only through the name (map key, for files without a map entry the file stem), without a fallback to the
    /// file stem - the original knows only the map.
    pub fn expanded(&self, name: &str) -> Option<&Expanded> {
        self.expanded_for(name, self.default_level)
    }

    /// Like [`Self::expanded`] for the variant that the build reads with `is_menu_level` (`IsMenuLevel` at build time):
    /// with `true` `START` sends `0x21` and carries the `.bkg`, with `false` `0x20` and the image (O-7). Files without
    /// a difference have only one variant.
    pub fn expanded_for(&self, name: &str, is_menu_level: bool) -> Option<&Expanded> {
        let key = upper_word(name);
        self.dialogs
            .iter()
            .find(|d| d.name == key)
            .map(|d| d.variant(self.default_level, is_menu_level))
    }

    /// Library from `.dlg` texts (name, Latin-1 source text), evaluated without `IsMenuLevel` (tests only).
    #[cfg(test)]
    pub(crate) fn from_sources(sources: &[(&str, &str)]) -> DialogLibrary {
        let dialogs = sources
            .iter()
            .map(|(name, src)| {
                let bytes: Vec<u8> = src.chars().map(|c| c as u8).collect();
                let (doc, _) = parse_dlg(&bytes);
                LibraryDialog {
                    name: upper_word(name),
                    stem: upper_word(name),
                    path: format!("{DIALOG_DIR}{name}.dlg"),
                    expanded: expand(&doc, ExpandOptions::default()),
                    other: None,
                }
            })
            .collect();
        DialogLibrary {
            dialogs,
            missing: Vec::new(),
            default_level: false,
        }
    }

    /// Diagnostics of the evaluation per dialog (name, diagnostic) in path order.
    pub fn diagnostics(&self) -> Vec<(&str, &Diagnostic)> {
        self.dialogs
            .iter()
            .flat_map(|d| d.expanded.diagnostics.iter().map(|x| (d.name.as_str(), x)))
            .collect()
    }
}

/// File stem of a normalized path (`data/menü/start2.dlg` -> `start2`).
fn file_stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".dlg").unwrap_or(file)
}

/// Text ID of a widget (`TITLE` or `TEXT`; the list box has none).
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

fn resolve_texts(widget: &WidgetDesc, texts: &Texts) {
    if let Some(id) = text_id(&widget.kind) {
        texts.resolve(id);
    }
    for child in &widget.children {
        resolve_texts(child, texts);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------------------------------------------

/// Frame flags after the constructor `FUN_00461AD0`: if a title flag (`0x28`) is set without a bar flag (`0x14`),
/// `0x28` is cleared and the internal bit `0x100` is set (title without bar and without fly-in offset, O-73).
pub fn constructed_frame_flags(flags: u32) -> u32 {
    use frame_flags::*;
    let titles = DRAWTITLEL | DRAWTITLER;
    if flags & titles != 0 && flags & (DRAWBARL | DRAWBARR) == 0 {
        (flags & !titles) | TITLE_WITHOUT_BAR
    } else {
        flags
    }
}

/// Position of a widget after the build (grid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Name from the name table ([`WidgetDesc::name`]).
    pub name: String,
    /// Absolute grid rectangle (`FUN_00459A10`).
    pub abs: [i32; 4],
    /// Client area absolute in the grid (`FUN_00459AC0`).
    pub client: [i32; 4],
    /// `abs` in screen pixels (`FUN_00459A70`).
    pub screen: [i32; 4],
}

/// Position of all drawn widgets in drawing order, as [`paint_static`] computes it (widgets with an empty visible
/// rectangle and their children are missing; `HIDE` stops only their own drawing, 12.5). For checks and
/// hit tests. Desktop, dialog root and blocker do not count.
pub fn placements(dialogs: &[&WidgetDesc], ctx: &mut UiContext) -> Vec<Placement> {
    let tree = static_tree(dialogs, ctx);
    visited(&tree)
        .into_iter()
        .filter_map(|(id, _)| {
            let node = tree.node(id);
            let name = match node.kind {
                NodeKind::Frame(_) => node.name.clone(),
                NodeKind::Desktop | NodeKind::Plain | NodeKind::ModalBlocker { .. } => return None,
                _ => format!("{}{}", node.dialog, node.name),
            };
            let abs = tree.abs_rect(id);
            Some(Placement {
                name,
                abs,
                client: tree.client_rect(id),
                screen: to_screen_rect(ctx.screen, abs),
            })
        })
        .collect()
}

/// `.bkg` of a frame (constructor `FUN_00461AD0`): without `NOFRAME` `BKGFILE` or `menuframe.bkg`.
pub(crate) fn frame_bkg(f: &FrameDesc) -> Option<String> {
    if f.flags & frame_flags::NOFRAME != 0 {
        return None;
    }
    Some(f.bkg_file.clone().unwrap_or_else(|| FRAME_BKG.to_string()))
}

/// Content of a credits file, read like `FUN_00462AF0`.
pub(crate) struct CreditsFile {
    /// Image changes in file order: number of lines set before them and image name (`Mid(0x12)`, then
    /// `Left(len - 1)`), Latin-1. `FUN_00462770` notes the height set so far (list `+0x14C`), the
    /// name goes into the list `+0x130`.
    pub(crate) backgrounds: Vec<(usize, String)>,
    /// The lines to be set in file order: bytes Latin-1 to U+00XX (the original extends
    /// signed, `FUN_0055F720`; measuring, drawing and tags use only the low byte), at most 256
    /// characters, up to a null byte; an empty line becomes `" [CR]"`.
    pub(crate) lines: Vec<String>,
}

/// Lines like `GameFile_ReadLine` (`0x00574C00`): up to `\n`, every `\r` is dropped, the last line counts even without
/// a line ending, an empty line after the last `\n` does not.
pub(crate) fn read_credits(vfs: &Vfs, path: &str) -> Option<CreditsFile> {
    let bytes = vfs.read(path).ok()?;
    let mut raw: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if raw.last().is_some_and(|l| l.is_empty()) {
        raw.pop();
    }
    let latin1 = |b: &[u8]| b.iter().map(|&c| char::from(c)).collect::<String>();
    let mut file = CreditsFile {
        backgrounds: Vec::new(),
        lines: Vec::new(),
    };
    for piece in raw {
        let line: Vec<u8> = piece.iter().copied().filter(|&b| b != b'\r').collect();
        if line.starts_with(CHANGEBACKGROUND) {
            let rest = &line[CHANGEBACKGROUND.len()..];
            let name = &rest[..rest.len().saturating_sub(1)];
            file.backgrounds.push((file.lines.len(), latin1(name)));
            continue;
        }
        if line.is_empty() {
            file.lines.push(CREDITS_EMPTY_LINE.to_string());
        } else {
            let end = line
                .iter()
                .take(CREDITS_LINE_MAX)
                .position(|&b| b == 0)
                .unwrap_or(line.len().min(CREDITS_LINE_MAX));
            file.lines.push(latin1(&line[..end]));
        }
    }
    Some(file)
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------------------------

/// Draws opened dialogs (top-level widgets, opening order) statically in their end position: first the
/// desktop images, then the tree like `Widget_RenderTree` (see the module documentation, 8.4.12). The same inputs yield
/// the same commands.
pub fn paint_static(dialogs: &[&WidgetDesc], ctx: &mut UiContext) -> Vec<DrawCmd> {
    let tree = static_tree(dialogs, ctx);
    paint_ui(&tree, ctx, 0)
}

/// Tree in the file state for [`paint_static`] and [`placements`]: per dialog build ([`UiTree::build_dialog_static`])
/// and open, all crossfades finished (images full), dialog root visible, then one drawing pass at
/// time 0 without a listener (no due queries, no movement, no scrolling).
fn static_tree(dialogs: &[&WidgetDesc], ctx: &mut UiContext) -> UiTree {
    let mut tree = UiTree::new();
    let mut listener = NoListener;
    let mut fx = Vec::new();
    let mut env = UiEnv {
        ui: UiContext {
            vfs: ctx.vfs,
            assets: &mut *ctx.assets,
            fonts: ctx.fonts,
            texts: ctx.texts,
            screen: ctx.screen,
        },
        listener: &mut listener,
        fx: &mut fx,
        now: 0,
    };
    let desktop = tree.desktop();
    for dialog in dialogs {
        tree.build_dialog_static(dialog, &mut env);
        // Open `FUN_004617F0(0x10000)`: with `MOVE` `0x15E` without a flag check, without `MOVE` only with
        // `MENUFRAME_BACKGROUND`; the name is `+0xA0` in both cases (`BACKGROUND`, 5.3).
        if let WidgetKind::Frame(f) = &dialog.kind
            && f.flags & (frame_flags::MOVE | frame_flags::BACKGROUND) != 0
        {
            let path = f.background.clone().unwrap_or_default();
            tree.broadcast(desktop, &Broadcast::ShowImage { path, ms: 0 }, &mut env);
        }
    }
    tree.images.finish_fades();
    let root = tree.dialog_root();
    tree.node_mut(root).visible = true;
    tick_tree(&mut tree, &mut env, &mut |_, _, _| {});
    tree
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bkg_paint::{
        LoadedBkg, image_quad, load_bkg, paint_bkg, paint_frame_decor, paint_fullscreen,
    };
    use crate::coords::to_screen_rect;
    use crate::draw::{Blend, Filter, TextureId};
    use crate::fonts::FontRef;
    use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
    use crate::text::{LayoutParams, TextLayout, atlas_codes, draw_runs, draw_string, layout_text};
    use nml_formats::dlg::{AppCommand, ExpandOptions, WidgetKind, expand, parse_dlg};
    use nml_formats::localisation::parse_localisation;

    const S1280: Screen = Screen {
        width: 1280,
        height: 1024,
    };
    const S1024: Screen = Screen {
        width: 1024,
        height: 768,
    };
    const WHITE: u32 = 0xFFFF_FFFF;

    /// Text as Latin-1 bytes (like the `.dlg`/`.bkg`/credits files of the game).
    fn latin1(text: &str) -> Vec<u8> {
        text.chars().map(|c| u8::try_from(c).unwrap()).collect()
    }

    fn utf16(text: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        out
    }

    fn white(w: u16, h: u16) -> Vec<u8> {
        tga_bottom_up(w, h, |_, _| [255, 255, 255, 255])
    }

    /// 9-patch frame from one file (`TOPLEFT 0 0` ... `BOTTOMRIGHT 0 8`, `FILLTILE 0 4`).
    fn nine(file: &str, tile: i32, client: Option<i32>) -> Vec<u8> {
        let mut s = format!(
            "FILE {file} {tile} {tile}\r\n\r\nTOPLEFT 0 0\r\nTOPMID 0 1\r\nTOPRIGHT 0 2\r\nLEFTMID 0 3\r\n\
             RIGHTMID 0 5\r\nBOTTOMLEFT 0 6\r\nBOTTOMMID 0 7\r\nBOTTOMRIGHT 0 8\r\nFILLTILE 0 4\r\n"
        );
        if let Some(c) = client {
            s.push_str(&format!(
                "\r\nCLIENTAREA {c} {} {c} {}\r\n",
                if c == 15 { 30 } else { c },
                if c == 15 { 30 } else { c }
            ));
        }
        latin1(&s)
    }

    /// Fonts `MenuButton`, `MenuTitle`, `Credits` (15 px, advance 10, cell 16 x 16), texts 1-4, all
    /// fixed frames and images of the widgets in small test versions.
    pub(crate) struct Fixture {
        pub(crate) vfs: TestVfs,
        pub(crate) assets: UiAssets,
        pub(crate) fonts: FontBook,
        pub(crate) texts: Texts,
    }

    pub(crate) fn fixture() -> Fixture {
        let credits = latin1(
            "[CHANGEBACKGROUND data/menü/bg2.tga]\r\n\r\nAB\u{E4}\r\n[CHANGEBACKGROUND data/menü/bg.tga]\r\nCD\r\n",
        );
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            ("data/fonts/Test_15_0_0_0_0.tga", white(256, 256)),
            (
                "data/txt/t.txt",
                utf16("1=AB\r\n2=Hauptmenü\r\n3=AB CD\r\n"),
            ),
            ("data/txt/c.txt", credits),
            ("data/menü/rahmen_21x21.tga", white(64, 64)),
            ("data/menü/rahmen_23x23_aussen.tga", white(128, 128)),
            ("data/menü/buttons_13x13_normal.tga", white(64, 64)),
            ("data/menü/buttons_13x13_deaktivated.tga", white(64, 64)),
            ("data/menü/slider_42x42.tga", white(128, 128)),
            ("data/menü/buttons_51x51_02.tga", white(256, 256)),
            ("data/menü/zusatz_85x85.tga", white(256, 256)),
            ("data/menü/zusatz_85x85_gespiegelt.tga", white(256, 256)),
            ("data/menü/bg.tga", white(4, 4)),
            ("data/menü/bg2.tga", white(4, 4)),
            ("data/menü/background_1024x1024_sid.tga", white(4, 4)),
            ("data/menü/background_1024x1024_wid.tga", white(4, 4)),
            ("data/menü/background_1024x1024_pat.tga", white(4, 4)),
            (
                "data/menü/menuframe.bkg",
                nine("data/menü/rahmen_21x21.tga", 21, Some(15)),
            ),
            (
                "data/menü/button_normal.bkg",
                nine("data/menü/buttons_13x13_normal.tga", 13, None),
            ),
            (
                "data/menü/button_disabled.bkg",
                nine("data/menü/buttons_13x13_deaktivated.tga", 13, None),
            ),
            (
                "data/menü/button_border.bkg",
                nine("data/menü/rahmen_23x23_aussen.tga", 26, None),
            ),
            (
                "data/menü/comboframe.bkg",
                nine("data/menü/buttons_13x13_normal.tga", 13, Some(13)),
            ),
            (
                "data/gifcon/editfield.bkg",
                nine("data/Menü/Rahmen_21x21.tga", 21, Some(16)),
            ),
            (
                "data/menü/slider_horz.bkg",
                latin1(
                    "FILE data/menü/slider_42x42.tga 42 42\r\nTOPLEFT 0 0\r\nTOPMID 0 1\r\nTOPRIGHT 0 2\r\n",
                ),
            ),
            (
                "data/menü/slider_vert.bkg",
                latin1(
                    "FILE data/menü/slider_42x42.tga 42 42\r\nTOPLEFT 0 3\r\nLEFTMID 0 4\r\nBOTTOMLEFT 0 5\r\n",
                ),
            ),
        ]);
        let mut assets = UiAssets::new();
        let font = |name: &str| format!("Font {name}\nFontSize 15\nTextureFont\nFontName Test\n");
        let loc = parse_localisation(&format!(
            "TextDatei data\\txt\\t.txt\n{}{}{}",
            font("MenuButton"),
            font("MenuTitle"),
            font("Credits")
        ));
        let fonts = FontBook::load(&vfs, &loc, 1280, &mut assets, None);
        let texts = Texts::load(&vfs, &loc);
        Fixture {
            vfs,
            assets,
            fonts,
            texts,
        }
    }

    impl Fixture {
        pub(crate) fn ctx(&mut self, screen: Screen) -> UiContext<'_> {
            UiContext {
                vfs: &self.vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen,
            }
        }

        fn paint(&mut self, dialogs: &[&WidgetDesc], screen: Screen) -> Vec<DrawCmd> {
            paint_static(dialogs, &mut self.ctx(screen))
        }

        fn place(&mut self, dialogs: &[&WidgetDesc], screen: Screen) -> Vec<Placement> {
            placements(dialogs, &mut self.ctx(screen))
        }

        pub(crate) fn bkg(&mut self, path: &str) -> LoadedBkg {
            load_bkg(&self.vfs, path, &mut self.assets).expect(path)
        }

        pub(crate) fn tex(&mut self, path: &str) -> TextureId {
            self.assets.texture(&self.vfs, path).expect(path)
        }

        pub(crate) fn font(&self) -> FontRef {
            self.fonts.by_name("MenuButton")
        }
    }

    /// A dialog from `.dlg` text (Latin-1), evaluated without `IsMenuLevel`; the first widget of the file.
    pub(crate) fn dialog(src: &str) -> WidgetDesc {
        let (doc, diags) = parse_dlg(&latin1(src));
        assert!(diags.is_empty(), "{diags:?}");
        let out = expand(&doc, ExpandOptions::default());
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        out.widgets.into_iter().next().expect("a widget")
    }

    /// A frame without a border over the whole grid with one child: the child lies absolutely as read.
    fn bare(child: &str) -> WidgetDesc {
        dialog(&format!(
            "FRAME F\n FLAGS MENUFRAME_NOFRAME /FLAGS\n EXTENT 0 0 1280 1024\n{child}\n/FRAME\n"
        ))
    }

    pub(crate) fn dst(cmd: &DrawCmd) -> [i32; 4] {
        match *cmd {
            DrawCmd::Quad { dst, .. } => dst,
            DrawCmd::Quad4 { .. } => unreachable!(),
        }
    }

    pub(crate) fn color(cmd: &DrawCmd) -> u32 {
        match *cmd {
            DrawCmd::Quad { color, .. } => color,
            DrawCmd::Quad4 { .. } => unreachable!(),
        }
    }

    pub(crate) fn texture(cmd: &DrawCmd) -> Option<TextureId> {
        match *cmd {
            DrawCmd::Quad { texture, .. } => texture,
            DrawCmd::Quad4 { .. } => unreachable!(),
        }
    }

    pub(crate) fn bkg_cmds(
        f: &mut Fixture,
        path: &str,
        abs: [i32; 4],
        clip: [i32; 4],
        screen: Screen,
    ) -> Vec<DrawCmd> {
        let bkg = f.bkg(path);
        let mut out = Vec::new();
        paint_bkg(&bkg, abs, clip, screen, &f.assets, &mut out);
        out
    }

    #[test]
    fn children_sit_in_the_parent_client_area() {
        // menuframe.bkg CLIENTAREA 15 30 15 30: EXTENT 0 0 ... of the button begins at frame x1 + 15, y1 + 30
        // (FUN_004599A0, `docs/formats/dlg.md`, section "Coordinates").
        let mut f = fixture();
        let d = dialog(
            "FRAME F\n EXTENT 100 100 500 500\n BUTTON B\n  EXTENT 0 0 100 50\n /BUTTON\n/FRAME\n",
        );
        let places = f.place(&[&d], S1280);
        assert_eq!(
            places,
            [
                Placement {
                    name: "F".into(),
                    abs: [100, 100, 500, 500],
                    client: [115, 130, 485, 470],
                    screen: [100, 100, 500, 500],
                },
                Placement {
                    name: "FB".into(),
                    abs: [115, 130, 215, 180],
                    client: [115, 130, 215, 180],
                    screen: [115, 130, 215, 180],
                },
            ]
        );
        // At 1024 x 768 the same grid, converted as a whole.
        let places = f.place(&[&d], S1024);
        assert_eq!(places[1].abs, [115, 130, 215, 180]);
        assert_eq!(
            places[1].screen,
            to_screen_rect(S1024, [115, 130, 215, 180])
        );
        // Drawn: frame of the frame widget, then the state of the button in the grid rectangle.
        let mut want = bkg_cmds(
            &mut f,
            "data/menü/menuframe.bkg",
            [100, 100, 500, 500],
            [100, 100, 500, 500],
            S1280,
        );
        want.extend(bkg_cmds(
            &mut f,
            "data/menü/button_normal.bkg",
            [115, 130, 215, 180],
            [115, 130, 215, 180],
            S1280,
        ));
        assert_eq!(f.paint(&[&d], S1280), want);
    }

    #[test]
    fn move_frames_paint_at_the_target() {
        // MENUFRAME_MOVE: end position top left corner = ZIELPOSITION, size from EXTENT (re/menu-system 5.3). An empty
        // image name on opening sets no desktop image (effect confirmed in the reference image, mechanism a
        // hypothesis).
        let mut f = fixture();
        let d = dialog(
            "FRAME F\n FLAGS MENUFRAME_MOVE /FLAGS\n EXTENT -440 212 0 912\n ZIELPOSITION 40 212\n/FRAME\n",
        );
        let places = f.place(&[&d], S1024);
        assert_eq!(places[0].abs, [40, 212, 480, 912]);
        assert_eq!(places[0].screen, [32, 159, 384, 684]);
        let want = bkg_cmds(
            &mut f,
            "data/menü/menuframe.bkg",
            [40, 212, 480, 912],
            [32, 159, 384, 684],
            S1024,
        );
        assert_eq!(f.paint(&[&d], S1024), want);
    }

    #[test]
    fn title_without_bar_sets_bit_0x100() {
        use frame_flags::*;
        // FUN_00461AD0: title flag (0x28) without bar flag (0x14) -> 0x100, 0x28 cleared.
        assert_eq!(constructed_frame_flags(DRAWTITLEL), TITLE_WITHOUT_BAR);
        assert_eq!(
            constructed_frame_flags(DRAWTITLER | MOVE),
            TITLE_WITHOUT_BAR | MOVE
        );
        assert_eq!(
            constructed_frame_flags(DRAWTITLEL | DRAWBARL),
            DRAWTITLEL | DRAWBARL
        );
        assert_eq!(
            constructed_frame_flags(DRAWTITLER | DRAWBARR),
            DRAWTITLER | DRAWBARR
        );
        assert_eq!(
            constructed_frame_flags(DRAWTITLEL | DRAWBARR),
            DRAWTITLEL | DRAWBARR
        );
        assert_eq!(constructed_frame_flags(DRAWTAGS), DRAWTAGS);
        // Drawn via FUN_004612B0: the mirrored texture on the left.
        let mut f = fixture();
        let d = dialog(
            "FRAME F\n FLAGS MENUFRAME_NOFRAME MENUFRAME_DRAWTITLEL /FLAGS\n EXTENT 0 0 600 400\n \
             TITLEEXTENT 0 0 600 100\n TITLE 2\n/FRAME\n",
        );
        let WidgetKind::Frame(desc) = &d.kind else {
            unreachable!()
        };
        let plain = f.tex("data/menü/zusatz_85x85.tga");
        let mirrored = f.tex("data/menü/zusatz_85x85_gespiegelt.tga");
        let title = atlas_codes("Hauptmenü");
        let mut want = Vec::new();
        paint_frame_decor(
            desc,
            TITLE_WITHOUT_BAR | NOFRAME,
            [0, 0, 600, 400],
            [0, 0],
            S1024,
            [Some(plain), Some(mirrored)],
            Some((&title, f.fonts.by_name("MenuTitle"))),
            &f.fonts,
            &f.assets,
            &mut want,
        );
        let got = f.paint(&[&d], S1024);
        assert_eq!(got, want);
        assert!(got.iter().any(|c| texture(c) == Some(mirrored)));
    }

    /// `start.dlg` without buttons (values from the shipped data, `docs/formats/dlg.md` quirk 13).
    const START: &str = "FRAME START\n FLAGS MENUFRAME_MUSIC MENUFRAME_LEVELBACKGROUND MENUFRAME_DRAWBARL \
                         MENUFRAME_MOVE MENUFRAME_DRAWTAGS MENUFRAME_DRAWTITLEL /FLAGS\n EXTENT -440 212 0 912\n \
                         ZIELPOSITION 40 212\n DURATION 300\n BAREXTENT 0 140 550 225\n TITLECOLOR ddFFf5bf\n \
                         BACKGROUND data/menü/bg.tga\n TITLEEXTENT -490 50 -50 185\n TITLE 2\n/FRAME\n";

    #[test]
    fn start_title_stays_offscreen() {
        // O-8: TITLEEXTENT -490 ... -50 lies outside the screen on the left; tiles and text of the title all end
        // at x <= 0, none of it reaches into the screen.
        let mut f = fixture();
        let d = dialog(START);
        for screen in [S1024, S1280] {
            let cmds = f.paint(&[&d], screen);
            let left: Vec<_> = cmds.iter().filter(|c| dst(c)[0] < 0).collect();
            assert!(left.iter().all(|c| dst(c)[2] <= 0), "{left:?}");
            // Four end pieces (the middle falls entirely out of the clip with n = 3) and nine glyphs "Hauptmenü".
            assert_eq!(left.len(), 4 + 9, "{screen:?}");
        }
    }

    #[test]
    fn draw_order_follows_8_4_12() {
        // Desktop image -> frame border -> bar -> ornaments -> title (tiles, text) -> children; button: border,
        // state, shadow, text; combo box: state, shadow, text, arrow.
        let mut f = fixture();
        let d = dialog(
            "FRAME F\n FLAGS MENUFRAME_BACKGROUND MENUFRAME_DRAWBARL MENUFRAME_DRAWTAGS MENUFRAME_DRAWTITLEL /FLAGS\n \
             EXTENT 100 200 600 900\n BAREXTENT 80 130 640 215\n TITLEEXTENT 100 40 600 140\n TITLE 2\n \
             BACKGROUND data/menü/bg.tga\n \
             BUTTON B\n  FLAGS MENUBUTTON_TEXT MENUBUTTON_BORDER /FLAGS\n  TEXT 1\n  TEXTCOLOR ddFFEABD\n  \
             EXTENT 10 10 300 90\n  BORDERSPACE 16 16 16 16\n /BUTTON\n \
             COMBOBOX C\n  FLAGS MENUCOMBOBOX_TEXT /FLAGS\n  TEXT 1\n  BUTTONEXTENT 10 100 300 150\n /COMBOBOX\n/FRAME\n",
        );
        let s = S1024;
        let sr = |r| to_screen_rect(s, r);
        let mut want = Vec::new();
        let bg = f.tex("data/menü/bg.tga");
        paint_fullscreen(bg, s, 255, &mut want);
        let frame = [100, 200, 600, 900];
        want.extend(bkg_cmds(
            &mut f,
            "data/menü/menuframe.bkg",
            frame,
            sr(frame),
            s,
        ));
        let WidgetKind::Frame(desc) = &d.kind else {
            unreachable!()
        };
        let zusatz = [
            Some(f.tex("data/menü/zusatz_85x85.tga")),
            Some(f.tex("data/menü/zusatz_85x85_gespiegelt.tga")),
        ];
        let title = atlas_codes("Hauptmenü");
        paint_frame_decor(
            desc,
            desc.flags,
            frame,
            [0, 0],
            s,
            zusatz,
            Some((&title, f.fonts.by_name("MenuTitle"))),
            &f.fonts,
            &f.assets,
            &mut want,
        );
        // Button: border over the whole rectangle, state inset by BORDERSPACE, then text with shadow.
        let b = [125, 240, 415, 320];
        want.extend(bkg_cmds(&mut f, "data/menü/button_border.bkg", b, sr(b), s));
        want.extend(bkg_cmds(
            &mut f,
            "data/menü/button_normal.bkg",
            [141, 256, 399, 304],
            sr(b),
            s,
        ));
        let text = |f: &Fixture, r: [i32; 4], c: u32, out: &mut Vec<DrawCmd>| {
            let sb = to_screen_rect(s, r);
            let mut layout = TextLayout::default();
            let params = LayoutParams {
                flags: 0x1C6,
                width: sb[2] - sb[0],
                font: f.font(),
                color: c,
            };
            layout_text("AB", &params, &f.fonts, &mut layout);
            let y = sb[1] + (sb[3] - sb[1] - layout.height) / 2;
            draw_runs(&layout, [sb[0], y], true, None, &f.fonts, out);
        };
        text(&f, b, 0xDDFF_EABD, &mut want);
        // Combo box: state, text, arrow (tile 9 of buttons_13x13_normal.tga) at (x2-13, y1, x2, y1+13).
        let c = [125, 330, 415, 380];
        want.extend(bkg_cmds(&mut f, "data/menü/button_normal.bkg", c, sr(c), s));
        text(&f, c, WHITE, &mut want);
        let arrow = f.tex("data/menü/buttons_13x13_normal.tga");
        image_quad(
            &f.assets,
            arrow,
            [13, 13],
            9,
            sr([395, 337, 408, 350]),
            sr(c),
            WHITE,
            Filter::Linear,
            &mut want,
        );
        assert_eq!(f.paint(&[&d], s), want);
    }

    #[test]
    fn button_text_is_centered_with_shadow() {
        // Layout 0x1C6, W = full pixel width, y = S.t + (S.h - th)/2; shadow at (x, y+2) in color & 0xFF000000,
        // text at (x+2, y) (8.4.4, O-62). "AB": width 20 -> offset (200 - 20)/2 = 90; y = (60 - 15)/2 = 22.
        let mut f = fixture();
        let d = bare(
            " BUTTON B\n  FLAGS MENUBUTTON_TEXT /FLAGS\n  TEXT 1\n  TEXTCOLOR ddFFEABD\n  EXTENT 0 0 200 60\n /BUTTON",
        );
        let cmds = f.paint(&[&d], S1280);
        let text: Vec<_> = cmds[cmds.len() - 4..]
            .iter()
            .map(|c| (dst(c), color(c)))
            .collect();
        assert_eq!(
            text,
            [
                ([90, 24, 106, 40], 0xDD00_0000),
                ([100, 24, 116, 40], 0xDD00_0000),
                ([92, 22, 108, 38], 0xDDFF_EABD),
                ([102, 22, 118, 38], 0xDDFF_EABD),
            ]
        );
        // DISABLE from the file: background set button_disabled.bkg (hypothesis, re/menu-system 8.4, section "Open")
        // and
        // DISABLEDTEXTCOLOR.
        let d = bare(
            " BUTTON B\n  FLAGS MENUBUTTON_TEXT DISABLE /FLAGS\n  TEXT 1\n  DISABLEDTEXTCOLOR 667D6A41\n  \
             EXTENT 0 0 200 60\n /BUTTON",
        );
        let cmds = f.paint(&[&d], S1280);
        let disabled = f.tex("data/menü/buttons_13x13_deaktivated.tga");
        assert_eq!(texture(&cmds[0]), Some(disabled));
        assert_eq!(color(cmds.last().unwrap()), 0x667D_6A41);
    }

    #[test]
    fn combobox_arrow_sits_at_8_4_4() {
        // A = (100, 50, 400, 100), BORDERSPACE 0: y1 = 57, x2 = 393 -> (380, 57, 393, 70); tile 9 in the 4 x 4 grid
        // of 64^2 = column 1, row 2: cell origin (13/64, 26/64), cell size 12/64, half texel 0.5/64.
        let mut f = fixture();
        let d = bare(" COMBOBOX C\n  BUTTONEXTENT 100 50 400 100\n /COMBOBOX");
        let cmds = f.paint(&[&d], S1280);
        let arrow = f.tex("data/menü/buttons_13x13_normal.tga");
        let h = 0.5 / 64.0;
        assert_eq!(
            *cmds.last().unwrap(),
            DrawCmd::Quad {
                texture: Some(arrow),
                dst: [380, 57, 393, 70],
                uv: [
                    13.0 / 64.0 + h,
                    26.0 / 64.0 + h,
                    13.0 / 64.0 + 12.0 / 64.0 - h,
                    26.0 / 64.0 + 12.0 / 64.0 - h
                ],
                color: WHITE,
                filter: Filter::Linear,
                blend: Blend::Alpha,
            }
        );
        // Without bit 0x1 no text: state + arrow.
        let state = bkg_cmds(
            &mut f,
            "data/menü/button_normal.bkg",
            [100, 50, 400, 100],
            [100, 50, 400, 100],
            S1280,
        );
        assert_eq!(cmds.len(), state.len() + 1);
        // At 1024 x 768 the same grid rectangle, converted.
        let cmds = f.paint(&[&d], S1024);
        assert_eq!(
            dst(cmds.last().unwrap()),
            to_screen_rect(S1024, [380, 57, 393, 70])
        );
    }

    #[test]
    fn label_text_is_left_aligned_without_shadow() {
        // Layout 0x46, left-aligned, without shadow, at C.lt (8.4.5): "AB " (30) and "CD".
        let mut f = fixture();
        let d = bare(" LABEL L\n  TEXT 3\n  TEXTCOLOR FF112233\n  EXTENT 10 20 600 100\n /LABEL");
        let cmds = f.paint(&[&d], S1280);
        let got: Vec<_> = cmds
            .iter()
            .map(|c| (dst(c)[0], dst(c)[1], color(c)))
            .collect();
        let c = 0xFF11_2233;
        assert_eq!(
            got,
            [
                (10, 20, c),
                (20, 20, c),
                (30, 20, c),
                (40, 20, c),
                (50, 20, c)
            ]
        );
        // MENULABEL_BACKGROUND fills the visible rectangle in BACKGROUNDCOLOR before the text.
        let d = bare(
            " LABEL L\n  FLAGS MENULABEL_BACKGROUND /FLAGS\n  BACKGROUNDCOLOR 80FF0000\n  TEXT 1\n  EXTENT 10 20 600 100\n /LABEL",
        );
        let cmds = f.paint(&[&d], S1280);
        assert_eq!(
            cmds[0],
            DrawCmd::Quad {
                texture: None,
                dst: [10, 20, 600, 100],
                uv: [0.0; 4],
                color: 0x80FF_0000,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }
        );
        assert_eq!(cmds.len(), 3);
        // TEXTARRAY without dialog logic: empty; TGA without an image: no image, the text stays.
        let d = bare(
            " LABEL L\n  FLAGS MENULABEL_TEXTARRAY /FLAGS\n  TEXT 1\n  EXTENT 10 20 600 100\n /LABEL",
        );
        assert!(f.paint(&[&d], S1280).is_empty());
        let d = bare(
            " LABEL L\n  FLAGS MENULABEL_TGA /FLAGS\n  TEXT 1\n  EXTENT 10 20 600 100\n /LABEL",
        );
        assert_eq!(f.paint(&[&d], S1280).len(), 2);
    }

    #[test]
    fn listbox_without_items_draws_only_the_frame() {
        let mut f = fixture();
        let d = bare(" LISTBOX L\n  EXTENT 0 0 300 200\n  CURSELECTION 0\n /LISTBOX");
        let want = bkg_cmds(
            &mut f,
            "data/menü/comboframe.bkg",
            [0, 0, 300, 200],
            [0, 0, 300, 200],
            S1280,
        );
        assert!(!want.is_empty());
        assert_eq!(f.paint(&[&d], S1280), want);
    }

    #[test]
    fn editfield_text_at_client_origin() {
        // editfield.bkg CLIENTAREA 16: text at C.lt via draw_string, without shadow (8.4.7).
        let mut f = fixture();
        let d = bare(
            " EDITFIELD E\n  TEXT 1\n  TEXTCOLOR FF00FF00\n  EXTENT 100 100 400 150\n /EDITFIELD",
        );
        let abs = [100, 100, 400, 150];
        let s = S1024;
        let mut want = bkg_cmds(
            &mut f,
            "data/gifcon/editfield.bkg",
            abs,
            to_screen_rect(s, abs),
            s,
        );
        let c = to_screen_rect(s, [116, 116, 384, 134]);
        draw_string(
            b"AB",
            [c[0], c[1]],
            0xFF00_FF00,
            f.font(),
            &f.fonts,
            &mut want,
        );
        assert_eq!(f.paint(&[&d], s), want);
        // Locked: fixed 0xC8A0A0A0, DISABLEDTEXTCOLOR unused (O-75).
        let d = bare(
            " EDITFIELD E\n  FLAGS DISABLE /FLAGS\n  TEXT 1\n  DISABLEDTEXTCOLOR FF0000FF\n  EXTENT 100 100 400 150\n /EDITFIELD",
        );
        assert_eq!(color(f.paint(&[&d], s).last().unwrap()), 0xC8A0_A0A0);
    }

    #[test]
    fn slider_thumb_and_scale_follow_step() {
        // 8.4.8: strip 42 high at the bottom edge; thumb and scale tile 6 at (A.l+21+o, A.b-42, A.l+63+o,
        // A.b), step = (L - 84)/(R - 1) as f32, offset (i * step) truncated.
        let mut f = fixture();
        let slider = f.tex("data/menü/slider_42x42.tga");
        let quads = |cmds: &[DrawCmd]| -> Vec<([i32; 4], u32)> {
            cmds.iter()
                .filter(|c| texture(c) == Some(slider))
                .map(|c| (dst(c), color(c)))
                .collect()
        };
        // R = 100: step = 364/99 ~ 3.7 <= 21 -> no scale, only the thumb (CURVALUE 50 -> 183).
        let d = bare(
            " SLIDER S\n  FLAGS MENUSLIDER_SCALE /FLAGS\n  EXTENT 0 0 448 100\n  RESOLUTION 100\n  CURVALUE 50\n /SLIDER",
        );
        let cmds = f.paint(&[&d], S1280);
        let strip = bkg_cmds(
            &mut f,
            "data/menü/slider_horz.bkg",
            [0, 58, 448, 100],
            [0, 0, 448, 100],
            S1280,
        );
        assert_eq!(cmds[..strip.len()], strip[..]);
        let thumb_offset = (50.0f32 * (364.0f32 / 99.0)) as i32;
        assert_eq!(thumb_offset, 183);
        assert_eq!(
            quads(&cmds[strip.len()..]),
            [([21 + 183, 58, 63 + 183, 100], WHITE)]
        );
        // R = 10, length 448: step = 40.444...; offsets 0, 40, 80, 121, ... and for i = 9 exactly 364: FILD; FMUL
        // float; _ftol in FUN_00464D90 with 24-bit x87 precision = f32 (with 53 bits it would be 363).
        let d = bare(
            " SLIDER S\n  FLAGS MENUSLIDER_SCALE /FLAGS\n  EXTENT 0 0 448 100\n  RESOLUTION 10\n  CURVALUE 3\n  \
             SCALECOLOR 80808080\n  THUMBCOLOR FF00FF00\n /SLIDER",
        );
        let cmds = f.paint(&[&d], S1280);
        let cmds = &cmds[strip.len()..];
        let offsets = [0, 40, 80, 121, 161, 202, 242, 283, 323, 364];
        let mut want: Vec<_> = offsets
            .iter()
            .map(|o| ([21 + o, 58, 63 + o, 100], 0x8080_8080))
            .collect();
        want.push(([21 + 121, 58, 63 + 121, 100], 0xFF00_FF00));
        assert_eq!(quads(cmds), want);
        // Vertical: strip 42 wide on the right, base (A.r-42, A.t+21, A.r, A.t+63).
        let d =
            bare(" SLIDER V\n  FLAGS MENUSLIDER_VERTICAL /FLAGS\n  EXTENT 0 0 100 448\n /SLIDER");
        let cmds = f.paint(&[&d], S1280);
        assert_eq!(quads(&cmds).last().unwrap().0, [58, 21, 100, 63]);
        // R < 2: nothing except the strip.
        let d = bare(" SLIDER S\n  EXTENT 0 0 448 100\n  RESOLUTION 1\n /SLIDER");
        let cmds = f.paint(&[&d], S1280);
        assert_eq!(cmds, strip);
        // Text above the top edge: without CENTERTEXT at S(A.l+21, A.t), y - th; with CENTERTEXT
        // x = S(A.l) + ((A.r - tw) - A.l)/2 (grid and pixels mixed, O-74).
        let d = bare(
            " SLIDER S\n  FLAGS MENUSLIDER_TEXT /FLAGS\n  TEXT 1\n  EXTENT 100 300 548 400\n /SLIDER",
        );
        let cmds = f.paint(&[&d], S1024);
        let glyph = cmds
            .iter()
            .find(|c| texture(c) == Some(f.font_atlas()))
            .unwrap();
        // P = S(A.l + 21, A.t) = (97, 225), y - th = 210.
        assert_eq!(dst(glyph)[..2], [97, 210]);
        let d = bare(
            " SLIDER S\n  FLAGS MENUSLIDER_TEXT MENUSLIDER_CENTERTEXT /FLAGS\n  TEXT 1\n  EXTENT 100 300 548 400\n /SLIDER",
        );
        let cmds = f.paint(&[&d], S1024);
        let glyph = cmds
            .iter()
            .find(|c| texture(c) == Some(f.font_atlas()))
            .unwrap();
        // x = 80 + ((548 - 20) - 100)/2 = 80 + 214.
        assert_eq!(dst(glyph)[..2], [294, 210]);
    }

    impl Fixture {
        pub(crate) fn font_atlas(&self) -> TextureId {
            self.fonts.font(self.font()).atlas
        }
    }

    #[test]
    fn radio_icon_is_a_square_of_pixel_height() {
        // 8.4.9: S(100, 100, 400, 151) at 1024 x 768 = (80, 75, 320, 113), h = 38: icon (80, 75, 118, 113),
        // tile ICONOFFSET + 1 (checked); text at (S.l + h + 5, S.t + h/2 - th/2) = (123, 75 + 19 - 7).
        let mut f = fixture();
        let d = bare(
            " RADIOBUTTON R\n  FLAGS RADIOBUTTON_TEXT RADIOBUTTON_CHECKED /FLAGS\n  ICONOFFSET 2\n  TEXT 1\n  \
             TEXTCOLOR FF445566\n  EXTENT 100 100 400 151\n /RADIOBUTTON",
        );
        let radio = f.tex("data/menü/buttons_51x51_02.tga");
        let mut want = Vec::new();
        let s = [80, 75, 320, 113];
        image_quad(
            &f.assets,
            radio,
            [51, 51],
            3,
            [80, 75, 118, 113],
            s,
            WHITE,
            Filter::Linear,
            &mut want,
        );
        draw_string(b"AB", [123, 87], 0xFF44_5566, f.font(), &f.fonts, &mut want);
        assert_eq!(f.paint(&[&d], S1024), want);
        // Locked: icon in 0xFF808080, tile ICONOFFSET without a check mark.
        let d = bare(
            " RADIOBUTTON R\n  FLAGS DISABLE /FLAGS\n  ICONOFFSET 2\n  EXTENT 100 100 400 151\n /RADIOBUTTON",
        );
        let mut want = Vec::new();
        image_quad(
            &f.assets,
            radio,
            [51, 51],
            2,
            [80, 75, 118, 113],
            s,
            0xFF80_8080,
            Filter::Linear,
            &mut want,
        );
        assert_eq!(f.paint(&[&d], S1024), want);
    }

    #[test]
    fn hidden_widgets_draw_only_their_children() {
        // 12.5 (also static): `HIDE` does not affect `+0x30`, only the own drawing - the hidden button B draws
        // nothing, the hidden frame G neither frame nor ornaments, but its child C does (in the client of G, whose
        // `.bkg` stays loaded). In the shipped data no element carries `HIDE`.
        let mut f = fixture();
        let plain = dialog("FRAME F\n EXTENT 0 0 600 600\n/FRAME\n");
        let hidden = dialog(
            "FRAME F\n EXTENT 0 0 600 600\n BUTTON B\n  FLAGS HIDE MENUBUTTON_TEXT /FLAGS\n  TEXT 1\n  \
             EXTENT 0 0 100 50\n /BUTTON\n FRAME G\n  FLAGS HIDE /FLAGS\n  EXTENT 0 100 300 300\n  \
             BUTTON C\n   EXTENT 0 0 100 50\n  /BUTTON\n /FRAME\n/FRAME\n",
        );
        let mut want = f.paint(&[&plain], S1024);
        assert!(!want.is_empty());
        let c = f
            .place(&[&hidden], S1024)
            .into_iter()
            .find(|p| p.name == "GC")
            .expect("C drawn");
        assert_eq!(c.abs, [30, 160, 130, 210]);
        want.extend(bkg_cmds(
            &mut f,
            "data/menü/button_normal.bkg",
            c.abs,
            c.screen,
            S1024,
        ));
        assert_eq!(f.paint(&[&hidden], S1024), want);
        // A hidden dialog without children draws nothing at all.
        let d = dialog("FRAME F\n FLAGS HIDE /FLAGS\n EXTENT 0 0 600 600\n/FRAME\n");
        assert!(f.paint(&[&d], S1024).is_empty());
    }

    #[test]
    fn children_are_clipped_to_the_parent_client() {
        // Children get clip = abs(child) intersected with client(abs) intersected with V (8.4.1 "Tree"); client of
        // (0, 0, 400, 400) with menuframe.bkg = (15, 30, 385, 370).
        let mut f = fixture();
        let d = dialog(
            "FRAME F\n EXTENT 0 0 400 400\n \
             LABEL L\n  FLAGS MENULABEL_BACKGROUND /FLAGS\n  BACKGROUNDCOLOR 80FF0000\n  EXTENT 300 300 500 500\n /LABEL\n \
             COMBOBOX OUT\n  BUTTONEXTENT 500 0 600 50\n /COMBOBOX\n \
             COMBOBOX HALF\n  BUTTONEXTENT 200 0 400 40\n /COMBOBOX\n/FRAME\n",
        );
        let cmds = f.paint(&[&d], S1280);
        let frame = bkg_cmds(
            &mut f,
            "data/menü/menuframe.bkg",
            [0, 0, 400, 400],
            [0, 0, 400, 400],
            S1280,
        );
        let mut want = frame;
        // Label: fill over the visible rectangle (315, 330, 515, 530) intersected with the client.
        want.push(DrawCmd::Quad {
            texture: None,
            dst: [315, 330, 385, 370],
            uv: [0.0; 4],
            color: 0x80FF_0000,
            filter: Filter::Point,
            blend: Blend::Alpha,
        });
        // OUT lies entirely outside the client: nothing. HALF: the 9-patch frame is computed in the full rectangle (the
        // clip argument affects only the tile sequence, 8.4.2), the arrow (395, 37, 408, 50) falls out of the clip.
        want.extend(bkg_cmds(
            &mut f,
            "data/menü/button_normal.bkg",
            [215, 30, 415, 70],
            [215, 30, 385, 70],
            S1280,
        ));
        assert_eq!(cmds, want);
    }

    #[test]
    fn desktop_image_follows_the_open_order() {
        // re/menu-system 5.3/5.4: opening with MOVE sends 0x15E with BACKGROUND (+0xA0), without MOVE only with
        // MENUFRAME_BACKGROUND; an empty name sets nothing (effect confirmed in the reference image, mechanism a
        // hypothesis). Images stay in place (no 0x15F in the static stack) and are drawn in `CMap` order
        // (12.3, O-92): keys as read, so uppercase - "DATA/MENÜ/BG.TGA" in bucket 4 before "DATA/MENÜ/BG2.TGA" in
        // bucket 9, in every opening order.
        let mut f = fixture();
        let bg = f.tex("data/menü/bg.tga");
        let bg2 = f.tex("data/menü/bg2.tga");
        let full = |t| DrawCmd::Quad {
            texture: Some(t),
            dst: [0, 0, 1024, 768],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: WHITE,
            filter: Filter::Point,
            blend: Blend::Alpha,
        };
        let start = dialog(
            "FRAME A\n FLAGS MENUFRAME_NOFRAME MENUFRAME_MOVE MENUFRAME_LEVELBACKGROUND /FLAGS\n \
             BACKGROUND data/menü/bg.tga\n/FRAME\n",
        );
        let video = dialog(
            "FRAME B\n FLAGS MENUFRAME_NOFRAME MENUFRAME_MOVE MENUFRAME_LEVELBACKGROUND /FLAGS\n/FRAME\n",
        );
        let plain = dialog(
            "FRAME C\n FLAGS MENUFRAME_NOFRAME MENUFRAME_BACKGROUND /FLAGS\n BACKGROUND data/menü/bg2.tga\n/FRAME\n",
        );
        let ignored = dialog(
            "FRAME D\n FLAGS MENUFRAME_NOFRAME /FLAGS\n BACKGROUND data/menü/bg2.tga\n/FRAME\n",
        );
        assert_eq!(f.paint(&[&start], S1024), [full(bg)]);
        assert_eq!(f.paint(&[&start, &video], S1024), [full(bg)]);
        assert_eq!(f.paint(&[&video], S1024), []);
        assert_eq!(f.paint(&[&start, &plain], S1024), [full(bg), full(bg2)]);
        assert_eq!(f.paint(&[&plain, &start], S1024), [full(bg), full(bg2)]);
        assert_eq!(
            [
                crate::desktop::cmap_bucket("DATA/MENÜ/BG.TGA"),
                crate::desktop::cmap_bucket("DATA/MENÜ/BG2.TGA")
            ],
            [4, 9]
        );
        assert_eq!(f.paint(&[&ignored], S1024), []);
    }

    #[test]
    fn credits_show_the_first_lines_before_scrolling() {
        // Credits (bit 0x8, 0x00463120 / FUN_00462AF0): one layout call 0x46 per line in "Credits" 0xC8FFFFFF,
        // empty line -> " [CR]", [CHANGEBACKGROUND ...] not shown; the first image change goes to the desktop as
        // 0x15E on attaching. Drawn before scrolling (pos = 0) with a shadow at (run.x - 2, run.y + 2).
        let mut f = fixture();
        let d = dialog(
            "FRAME CREDITS\n FLAGS MENUFRAME_NOFRAME MENUFRAME_MUSIC /FLAGS\n EXTENT 0 0 1280 1024\n \
             BACKGROUND data/menü/bg.tga\n LABEL SCROLLTEXT\n  FLAGS /FLAGS\n  CREDITS data/txt/c.txt\n  \
             EXTENT 0 0 1280 1024\n /LABEL\n/FRAME\n",
        );
        let cmds = f.paint(&[&d], S1280);
        let bg2 = f.tex("data/menü/bg2.tga");
        let mut want = Vec::new();
        paint_fullscreen(bg2, S1280, 255, &mut want);
        let credits = f.fonts.by_name("Credits");
        for (codes, y) in [
            (&b" "[..], 0),
            (&[b'A', b'B', 0xE4][..], 15),
            (&b"CD"[..], 30),
        ] {
            draw_string(
                codes,
                [-2, y + 4],
                0xC800_0000,
                credits,
                &f.fonts,
                &mut want,
            );
            draw_string(codes, [0, y + 2], 0xC8FF_FFFF, credits, &f.fonts, &mut want);
        }
        assert_eq!(cmds, want);
    }

    #[test]
    fn library_keeps_both_variants_of_start() {
        let files: Vec<(&str, Vec<u8>)> = vec![
            (
                "data/menü/dialogs.map",
                latin1(
                    "START data/menü/start.dlg
PLAIN data/menü/plain.dlg
",
                ),
            ),
            (
                "data/menü/start.dlg",
                latin1(
                    "FRAME START
 FLAGS MENUFRAME_LEVELBACKGROUND /FLAGS
                      LEVELBACKGROUND data/levels/x.lvl
 BACKGROUND data/menü/bg.tga
/FRAME
",
                ),
            ),
            (
                "data/menü/plain.dlg",
                latin1(
                    "FRAME PLAIN
 TITLE 1
/FRAME
",
                ),
            ),
            (
                "data/txt/t.txt",
                utf16(
                    "1=AB
",
                ),
            ),
        ];
        let vfs = test_vfs(&files);
        let texts = Texts::load(
            &vfs,
            &parse_localisation(
                "TextDatei data\txt\t.txt
",
            ),
        );
        for default in [false, true] {
            let lib = DialogLibrary::load(&vfs, &texts, default);
            let with = lib.expanded_for("start", true).unwrap();
            let without = lib.expanded_for("start", false).unwrap();
            assert_eq!(
                with.commands,
                [AppCommand::LoadLevelBackground("DATA/LEVELS/X.LVL".into())]
            );
            assert_eq!(without.commands, [AppCommand::HideLevelBackground]);
            let flags = |e: &Expanded| match &e.widgets[0].kind {
                WidgetKind::Frame(f) => (f.flags, f.background.clone()),
                _ => panic!("no FRAME"),
            };
            assert_eq!(
                flags(with),
                (
                    frame_flags::LEVELBACKGROUND,
                    Some("DATA/LEVELS/X.LVL".into())
                )
            );
            assert_eq!(
                flags(without),
                (frame_flags::BACKGROUND, Some("DATA/MENÜ/BG.TGA".into()))
            );
            // `expanded` and `find` return the variant of loading.
            assert!(std::ptr::eq(
                lib.expanded("start").unwrap(),
                lib.expanded_for("start", default).unwrap()
            ));
            assert_eq!(
                lib.find("start").unwrap(),
                lib.find_for("start", default).unwrap()
            );
            assert_ne!(
                lib.find_for("start", true).unwrap(),
                lib.find_for("start", false).unwrap()
            );
            // Without `LEVELBACKGROUND` there is only one variant.
            assert!(std::ptr::eq(
                lib.expanded_for("plain", true).unwrap(),
                lib.expanded_for("plain", false).unwrap()
            ));
        }
    }

    #[test]
    fn library_names_follow_the_map_and_the_file_stem() {
        let files: Vec<(&str, Vec<u8>)> = vec![
            (
                "data/menü/dialogs.map",
                latin1("START data/menü/start.dlg\r\nGONE data/menü/gone.dlg\r\n"),
            ),
            (
                "data/menü/start.dlg",
                latin1("FRAME START\n TITLE 1\n/FRAME\n"),
            ),
            (
                "data/menü/start2.dlg",
                latin1("FRAME START\n TITLE 99\n/FRAME\n"),
            ),
            (
                "data/menü/a.dlg",
                latin1("FRAME A\n TITLE 98\n BUTTON B\n  TEXT 1\n /BUTTON\n/FRAME\n"),
            ),
            ("data/txt/t.txt", utf16("1=AB\r\n")),
        ];
        let vfs = test_vfs(&files);
        let texts = Texts::load(&vfs, &parse_localisation("TextDatei data\\txt\\t.txt\n"));
        let lib = DialogLibrary::load(&vfs, &texts, false);
        // Path order; name = map key or uppercased file stem.
        assert_eq!(lib.names(), ["A", "START", "START2"]);
        let start = lib.find("start").unwrap();
        assert_eq!(start.name, "START");
        let WidgetKind::Frame(desc) = &start.kind else {
            panic!()
        };
        assert_eq!(desc.title, Some(1));
        let WidgetKind::Frame(desc) = &lib.find("Start2").unwrap().kind else {
            panic!()
        };
        assert_eq!(desc.title, Some(99));
        assert_eq!(
            lib.find("gone").unwrap_err(),
            "dialog \"GONE\": file data/menü/gone.dlg is missing"
        );
        assert_eq!(
            lib.find("nix").unwrap_err(),
            "unknown dialog \"nix\" (known: A, START, START2)"
        );
        // Text IDs resolved in path order (O-4): the last missing one was 99 (start2.dlg).
        assert_eq!(texts.display(12345), "99");
        assert!(lib.diagnostics().is_empty());
        // Diagnostics do not count as load errors: the partial result stays.
        let vfs = test_vfs(&[("data/menü/x.dlg", latin1("FRAME X\n BUTTON B\n/FRAME\n"))]);
        let lib = DialogLibrary::load(&vfs, &texts, false);
        assert_eq!(lib.names(), ["X"]);
        assert!(lib.find("x").is_ok());
        assert_eq!(lib.diagnostics().len(), 1);
    }

    #[test]
    fn find_accepts_map_key_or_file_stem() {
        // The map key differs from the file stem (SAVEGAME -> save.dlg, as in the shipped data): both ways
        // lead to the same dialog; a name takes precedence over a foreign stem.
        let files: Vec<(&str, Vec<u8>)> = vec![
            (
                "data/menü/dialogs.map",
                latin1("SAVEGAME data/menü/save.dlg\nSAVE data/menü/other.dlg\n"),
            ),
            (
                "data/menü/save.dlg",
                latin1("FRAME SAVEGAME\n TITLE 1\n/FRAME\n"),
            ),
            (
                "data/menü/other.dlg",
                latin1("FRAME SAVE\n TITLE 2\n/FRAME\n"),
            ),
        ];
        let vfs = test_vfs(&files);
        let texts = Texts::load(&vfs, &parse_localisation(""));
        let lib = DialogLibrary::load(&vfs, &texts, false);
        let title = |name: &str| {
            let WidgetKind::Frame(desc) = &lib.find(name).unwrap().kind else {
                panic!()
            };
            desc.title
        };
        assert_eq!(title("savegame"), Some(1));
        // Name before stem: "SAVE" is the key of other.dlg and at the same time the stem of save.dlg.
        assert_eq!(title("save"), Some(2));
        assert_eq!(title("Other"), Some(2));
        let vfs = test_vfs(&[
            (
                "data/menü/dialogs.map",
                latin1("LOADSAVEDGAME data/menü/load.dlg\n"),
            ),
            (
                "data/menü/load.dlg",
                latin1("FRAME LOADSAVEDGAME\n TITLE 7\n/FRAME\n"),
            ),
        ]);
        let lib = DialogLibrary::load(&vfs, &texts, false);
        assert_eq!(
            lib.find("load").unwrap().name,
            lib.find("LoadSavedGame").unwrap().name
        );
        assert_eq!(
            lib.find("nix").unwrap_err(),
            "unknown dialog \"nix\" (known: LOADSAVEDGAME)"
        );
    }

    #[test]
    fn paint_is_deterministic() {
        let mut f = fixture();
        let d = dialog(START);
        let first = f.paint(&[&d], S1024);
        assert_eq!(f.paint(&[&d], S1024), first);
        assert!(!first.is_empty());
    }
}
