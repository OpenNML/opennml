//! Data of the command bar: positions from `toolbar.cfg`, portraits and icons from `icons.ini` and the three fixed
//! image sheets.
//!
//! The constructor of the bar `0x00456630` reads `toolbar.cfg` (reader `0x00455F30`) and `icons.ini` (reader
//! `0x004553B0`) and loads the fixed sheets (`resourcen_icons.tga` `0x00456A5D`, `info_icons16x16.tga`,
//! `counterupgrade.tga` `0x00456B14`; re/game-ui 2.1 and N.14.1). Only the texts arise here as data: drawing resolves
//! the images ([`Sheet::texture`]), because `Hud::load` has no image collection and a `TextureId` is only valid for
//! the collection that issued it.

use opennml_formats::icons_ini::{
    ICON_TILE, ICONS_INI_PATH, IconsIni, PORTRAIT_TILE, parse_icons_ini,
};
use opennml_formats::text::decode_cp1252;
use opennml_formats::toolbar_cfg::{
    TOOLBAR_CFG_PATH, ToolbarConfig, ToolbarIcon, parse_toolbar_cfg,
};
use opennml_rda::Vfs;

use crate::dialog::UiContext;
use crate::draw::TextureId;
use crate::hud::tile_grids::TileGrids;

/// Icon sheet of the resources and stances (`0x00456A5D`), tiles **24 x 24** according to the call of the bar. The
/// grid is determined, however, by whoever loads the file first (O-280, [`TileGrids`]): with a status bar it is its
/// `ICONSIZE 32 32` (`0x0045229A`), only without a status bar do the 24 x 24 apply.
pub const RESOURCE_ICONS_PATH: &str = "data/icons/resourcen_icons.tga";
/// Edge length of a tile in [`RESOURCE_ICONS_PATH`] that the bar specifies when loading (`PUSH 0x18, 0x18`).
pub const RESOURCE_TILE: i32 = 24;
/// Small icons of the tooltips (`INFO_Icons16x16`), tiles 16 x 16.
pub const INFO_ICONS_PATH: &str = "data/icons/info_icons16x16.tga";
/// Edge length of a tile in [`INFO_ICONS_PATH`].
pub const INFO_TILE: i32 = 16;
/// Overlay of running counter-upgrades (`0x00456B14`), one image 64 x 64.
pub const COUNTER_OVERLAY_PATH: &str = "data/icons/counterupgrade.tga";
/// Edge length of the tile in [`COUNTER_OVERLAY_PATH`].
pub const COUNTER_TILE: i32 = 64;

/// Number of colors of `BUTTONNORMALCOLORS`.
pub const COLOR_COUNT: usize = 10;
/// Color of a slot that the file does not occupy (quirk O-273).
pub const DEFAULT_COLOR: u32 = 0xFFFF_FFFF;

/// An image sheet of equally sized tiles: path and tile size. The texture is not stored here but resolved when
/// drawing via [`Sheet::texture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub path: String,
    /// Width and height of a tile in pixels.
    pub tile: [i32; 2],
}

impl Sheet {
    pub fn new(path: impl Into<String>, tile: [i32; 2]) -> Self {
        Self {
            path: path.into(),
            tile,
        }
    }

    /// Texture of the sheet: loads the image on the first draw and also remembers a failure
    /// ([`crate::assets::UiAssets::texture`]); `None` if the sheet is missing or not readable.
    pub fn texture(&self, ctx: &mut UiContext<'_>) -> Option<TextureId> {
        ctx.assets.texture(ctx.vfs, &self.path)
    }
}

/// All positions and sheets of the bar. Missing blocks in `toolbar.cfg` yield empty lists (the original reads them
/// unchecked; the reimplementation must not abort).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarData {
    /// `TAG TOOLBAR`: the background tiles (`ICON x y sheet tile`, above the bar for y < 0).
    pub background: Vec<ToolbarIcon>,
    /// `ICONS w h file`: sheets of the background tiles in file order, size from the line.
    pub sheets: Vec<Sheet>,
    /// `NORMALBUTTONS`: 2 rows of 7 (grid 53).
    pub normal: Vec<[i32; 2]>,
    /// `SELECTIONBUTTONS`: buttons of the multi-selection and the queue.
    pub selection: Vec<[i32; 2]>,
    /// `STATUSBUTTONS`: the stance buttons (the name trap: the block is called "status" but means the stance).
    pub stance: Vec<[i32; 2]>,
    /// `FORMATIONBUTTONS`.
    pub formation: Vec<[i32; 2]>,
    /// `HEROBUTTONS`: positions relative to the lower right edge (y < 0).
    pub heroes: Vec<[i32; 2]>,
    /// `HEADHUNTERBUTTONS`: like `heroes`.
    pub headhunters: Vec<[i32; 2]>,
    /// `PORTRAITPOSITION`: positions of portrait and values.
    pub portrait: Vec<[i32; 2]>,
    /// `BUTTONNORMALCOLORS` (constructor `0x0044F080`). Assignment according to the code (the comment of the file "1
    /// normal, 2 mouse over" is wrong):
    /// - `[0]` icon with mouse over or pressed, `[1]` normal, `[2]` locked;
    /// - `[3]` ... `[5]` the same for the selected stance (buttons of kind `0xA000000`, `& 4`);
    /// - `[6]` ... `[9]` veil per button state 0 ... 3.
    ///
    /// The bar colors are fixed in the code. A missing slot stays [`DEFAULT_COLOR`] (O-273: the original leaves a
    /// missing block undetermined and throws on one that is too short).
    pub colors: [u32; COLOR_COUNT],
    /// `icons.ini` unchanged: sheet numbers and tiles per GUID.
    pub icons: IconsIni,
    /// The `PORTRAIT` sheets of `icons.ini`, 80 x 80.
    pub portrait_sheets: Vec<Sheet>,
    /// The `ICONS` sheets of `icons.ini`, 50 x 50.
    pub icon_sheets: Vec<Sheet>,
    /// `resourcen_icons.tga` (stance, formation, mana): 24 x 24 or the grid of the status bar (O-280).
    pub resource_icons: Sheet,
    /// `info_icons16x16.tga`, 16 x 16 (tooltips).
    pub info_icons: Sheet,
    /// `counterupgrade.tga`, 64 x 64 (running counter-upgrades).
    pub counter_overlay: Sheet,
    /// `map_icons.tga` (30 x 30): only for the corner buttons of the minimap
    /// ([`crate::hud::toolbar::minimap_buttons::MinimapButtons`]); the bar does not load it.
    pub map_icons: Option<Sheet>,
}

impl Default for ToolbarData {
    /// Empty bar without sheets (only the three fixed images carry their path).
    fn default() -> Self {
        Self::from_parts(
            &ToolbarConfig::default(),
            IconsIni::default(),
            &mut TileGrids::new(),
        )
    }
}

impl ToolbarData {
    /// Reads `toolbar.cfg` and `icons.ini` from the installation (like the pattern of the status bar: Windows-1252
    /// text via `vfs.read`). If a file is not readable, there is a warning and empty data for it. The grids of the
    /// sheets run through `grids` ([`TileGrids`], O-280): what is already there (the status bar) keeps its grid; new
    /// sheets are entered in the loading order of the constructor `0x00456630`.
    pub fn load(vfs: &Vfs, grids: &mut TileGrids) -> Self {
        let read = |path: &str| match vfs.read(path) {
            Ok(bytes) => decode_cp1252(&bytes),
            Err(e) => {
                log::warn!("Command bar: {path} not read: {e:#}");
                String::new()
            }
        };
        Self::from_texts_with(&read(TOOLBAR_CFG_PATH), &read(ICONS_INI_PATH), grids)
    }

    /// Like [`ToolbarData::load`] from the texts of both files, without previously loaded sheets (like `IsStatusBar
    /// 0`).
    pub fn from_texts(toolbar_cfg: &str, icons_ini: &str) -> Self {
        Self::from_texts_with(toolbar_cfg, icons_ini, &mut TileGrids::new())
    }

    /// Like [`ToolbarData::from_texts`] with the cache `grids` (O-280).
    pub fn from_texts_with(toolbar_cfg: &str, icons_ini: &str, grids: &mut TileGrids) -> Self {
        Self::from_parts(
            &parse_toolbar_cfg(toolbar_cfg),
            parse_icons_ini(icons_ini),
            grids,
        )
    }

    /// Loading order of the constructor `0x00456630`: sheets of `toolbar.cfg` (`0x00455F30`, call `0x004567A4`), of
    /// `icons.ini` (`0x004553B0`, call `0x004567C3`; the original loads per line in file order, the reimplementation
    /// first all `PORTRAIT` sheets, then all `ICONS` sheets - equivalent, because in the shipped data no file is in
    /// both lists, corpus test `toolbar_sheets_have_the_documented_tile_sizes`), then `info_icons16x16.tga`
    /// (`0x00456A58`), `resourcen_icons.tga` (`0x00456A7E`) and `counterupgrade.tga` (`0x00456B42`). Each sheet takes
    /// its grid from `grids` ([`TileGrids::register`]).
    fn from_parts(cfg: &ToolbarConfig, icons: IconsIni, grids: &mut TileGrids) -> Self {
        let positions = |tag: &str| {
            cfg.tags
                .get(tag)
                .map(|t| t.positions.clone())
                .unwrap_or_default()
        };
        let mut colors = [DEFAULT_COLOR; COLOR_COUNT];
        if let Some(tag) = cfg.tags.get("BUTTONNORMALCOLORS") {
            for (slot, &color) in colors.iter_mut().zip(&tag.colors) {
                *slot = color;
            }
        }
        let mut sheet = |path: &str, tile: [i32; 2]| Sheet::new(path, grids.register(path, tile));
        let sheets: Vec<Sheet> = cfg
            .sheets
            .iter()
            .map(|(path, size)| sheet(path, *size))
            .collect();
        let portrait_sheets: Vec<Sheet> = icons
            .portrait_sheets
            .iter()
            .map(|p| sheet(p, [PORTRAIT_TILE; 2]))
            .collect();
        let icon_sheets: Vec<Sheet> = icons
            .icon_sheets
            .iter()
            .map(|p| sheet(p, [ICON_TILE; 2]))
            .collect();
        let info_icons = sheet(INFO_ICONS_PATH, [INFO_TILE; 2]);
        let resource_icons = sheet(RESOURCE_ICONS_PATH, [RESOURCE_TILE; 2]);
        let counter_overlay = sheet(COUNTER_OVERLAY_PATH, [COUNTER_TILE; 2]);
        Self {
            background: cfg
                .tags
                .get("TOOLBAR")
                .map(|t| t.icons.clone())
                .unwrap_or_default(),
            sheets,
            normal: positions("NORMALBUTTONS"),
            selection: positions("SELECTIONBUTTONS"),
            stance: positions("STATUSBUTTONS"),
            formation: positions("FORMATIONBUTTONS"),
            heroes: positions("HEROBUTTONS"),
            headhunters: positions("HEADHUNTERBUTTONS"),
            portrait: positions("PORTRAITPOSITION"),
            colors,
            portrait_sheets,
            icon_sheets,
            icons,
            resource_icons,
            info_icons,
            counter_overlay,
            map_icons: None,
        }
    }

    /// Icon of a GUID, **strict** (`0x00453F20`: multi-selection and waiting orders; likewise occupant, running order
    /// `0x00454180`, hero `0x00454760` and headhunter `0x004549B0`): sheet and tile from `icons.ini`; if the entry is
    /// missing or the sheet or tile is negative, `None`. The original aborts on a negative sheet, the reimplementation
    /// leaves the button out (O-263, deviation; no effect in the shipped data). The sheet number is not checked against
    /// the number of sheets: [`ToolbarData::icon_sheet`].
    pub fn icon_of(&self, guid: i32) -> Option<(usize, i32)> {
        let e = self.icons.guids.get(&guid)?;
        if e.icon_sheet < 0 || e.icon < 0 {
            return None;
        }
        Some((e.icon_sheet as usize, e.icon))
    }

    /// Icon of a GUID **with clamping**, only for the normal button (`0x00453D70`, branch without occupants): a
    /// negative sheet or negative tile counts as 0; if the entry is missing, `None`. Occupant, running order, hero and
    /// headhunter do not clamp and use [`ToolbarData::icon_of`] (re/game-ui N.14.10).
    pub fn icon_clamped(&self, guid: i32) -> Option<(usize, i32)> {
        let e = self.icons.guids.get(&guid)?;
        Some((e.icon_sheet.max(0) as usize, e.icon.max(0)))
    }

    /// Portrait of a GUID (columns 2/3 of `icons.ini`): sheet and tile; `None` if the entry is missing, a value is
    /// negative or the sheet is not among the `PORTRAIT` sheets (the original then aborts).
    pub fn portrait_of(&self, guid: i32) -> Option<(usize, i32)> {
        let e = self.icons.guids.get(&guid)?;
        if e.portrait_sheet < 0 || e.portrait < 0 {
            return None;
        }
        let sheet = e.portrait_sheet as usize;
        (sheet < self.portrait_sheets.len()).then_some((sheet, e.portrait))
    }

    /// Icon sheet number `index`; `None` past the end of the list (the original would abort, the reimplementation
    /// draws nothing).
    pub fn icon_sheet(&self, index: usize) -> Option<&Sheet> {
        self.icon_sheets.get(index)
    }

    /// Portrait sheet number `index`.
    pub fn portrait_sheet(&self, index: usize) -> Option<&Sheet> {
        self.portrait_sheets.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = "ICONS 128 128 data/icons/status_a_00.tga\r\nICONS 64 64 data/icons/status_b_01.tga\r\nTAG TOOLBAR\r\nICON -2 -128 0 0\r\nICON -2 126 1 3\r\n/TAG\r\nTAG NORMALBUTTONS\r\nPOS 245 17\r\nPOS 298 17\r\n/TAG\r\nTAG STATUSBUTTONS\r\nPOS 10 20\r\n/TAG\r\nTAG HEROBUTTONS\r\nPOS -5 -50\r\n/TAG\r\nTAG BUTTONNORMALCOLORS\r\nCOLOR 11223344\r\nCOLOR FF0000FF\r\nCOLOR 80808080\r\n/TAG\r\n";
    const INI: &str = "PORTRAIT data/icons/a_80x80.tga\r\nPORTRAIT data/icons/b_80x80.tga\r\nICONS data/icons/i0.tga\r\nICONS data/icons/i1.tga\r\nGUID 20 1 4 0 3\r\nGUID 21 -1 4 -1 -2\r\nGUID 22 0 -3 1 -4\r\nGUID 23 5 1 2 6\r\nGUID 24 1 0 0 0\r\n";

    #[test]
    fn tags_fill_the_named_lists_and_missing_ones_stay_empty() {
        let d = ToolbarData::from_texts(CFG, INI);
        assert_eq!(d.background.len(), 2);
        assert_eq!(d.background[1].pos, [-2, 126]);
        assert_eq!(
            d.sheets,
            [
                Sheet::new("data/icons/status_a_00.tga", [128, 128]),
                Sheet::new("data/icons/status_b_01.tga", [64, 64])
            ]
        );
        assert_eq!(d.normal, [[245, 17], [298, 17]]);
        // `STATUSBUTTONS` is the stance.
        assert_eq!(d.stance, [[10, 20]]);
        assert_eq!(d.heroes, [[-5, -50]]);
        // Missing blocks: empty, no panic.
        assert!(d.selection.is_empty() && d.formation.is_empty());
        assert!(d.headhunters.is_empty() && d.portrait.is_empty());
        let empty = ToolbarData::from_texts("", "");
        assert!(empty.background.is_empty() && empty.sheets.is_empty() && empty.normal.is_empty());
        assert_eq!(empty, ToolbarData::default());
        assert_eq!(empty.icon_of(20), None);
        assert_eq!(empty.icon_clamped(20), None);
        assert_eq!(empty.portrait_of(20), None);
    }

    // QUIRK O-273: deviation
    #[test]
    fn missing_colors_stay_white_without_an_exception() {
        // O-273: a block that is too short leaves the remaining slots white; without a block all are white.
        let d = ToolbarData::from_texts(CFG, INI);
        let mut want = [DEFAULT_COLOR; COLOR_COUNT];
        want[..3].copy_from_slice(&[0x1122_3344, 0xFF00_00FF, 0x8080_8080]);
        assert_eq!(d.colors, want);
        assert_eq!(
            ToolbarData::from_texts("", "").colors,
            [DEFAULT_COLOR; COLOR_COUNT]
        );
        // More than ten colors: the surplus ones are dropped.
        let many = format!(
            "TAG BUTTONNORMALCOLORS\r\n{}/TAG\r\n",
            "COLOR FF000001\r\n".repeat(12)
        );
        assert_eq!(
            ToolbarData::from_texts(&many, "").colors,
            [0xFF00_0001; COLOR_COUNT]
        );
    }

    #[test]
    fn icon_sheets_have_their_tile_sizes_and_icons_keep_the_file() {
        let d = ToolbarData::from_texts(CFG, INI);
        assert_eq!(d.portrait_sheets.len(), 2);
        assert!(d.portrait_sheets.iter().all(|s| s.tile == [80, 80]));
        assert_eq!(d.icon_sheets.len(), 2);
        assert!(d.icon_sheets.iter().all(|s| s.tile == [50, 50]));
        assert_eq!(
            d.icons.portrait_sheets,
            ["data/icons/a_80x80.tga", "data/icons/b_80x80.tga"]
        );
        assert_eq!(d.portrait_sheet(1).unwrap().path, "data/icons/b_80x80.tga");
        assert!(d.icon_sheet(2).is_none() && d.portrait_sheet(2).is_none());
        assert_eq!(
            (d.resource_icons.path.as_str(), d.resource_icons.tile),
            ("data/icons/resourcen_icons.tga", [24, 24])
        );
        assert_eq!(d.info_icons.tile, [16, 16]);
        assert_eq!(d.counter_overlay.tile, [64, 64]);
    }

    // QUIRK O-280: kept
    #[test]
    fn status_bar_first_gives_the_resource_sheet_its_32_grid() {
        // O-280: texture cache by file name (`0x00445F40` -> `0x00445C20`). If the status bar loads
        // `resourcen_icons.tga` beforehand with `ICONSIZE 32 32` (`0x0045229A`), the 24 x 24 of the bar (`0x00456A5D`)
        // do not apply; stance and formation then use 32 tiles.
        let mut grids = TileGrids::new();
        assert_eq!(grids.register(RESOURCE_ICONS_PATH, [32, 32]), [32, 32]);
        let d = ToolbarData::from_texts_with(CFG, INI, &mut grids);
        assert_eq!(d.resource_icons.tile, [32, 32]);
        // Nobody loads the other fixed sheets beforehand: their own grid.
        assert_eq!(d.info_icons.tile, [16, 16]);
        assert_eq!(d.counter_overlay.tile, [64, 64]);
        // Without a status bar (`IsStatusBar 0`) the grid of the bar applies.
        let d = ToolbarData::from_texts_with(CFG, INI, &mut TileGrids::new());
        assert_eq!(d.resource_icons.tile, [24, 24]);
        assert_eq!(
            ToolbarData::from_texts(CFG, INI).resource_icons.tile,
            [24, 24]
        );
    }

    // QUIRK O-280: kept
    #[test]
    fn the_first_load_of_a_file_fixes_its_grid_for_all_toolbar_sheets() {
        // The loading order of the constructor `0x00456630` also applies within the bar: `toolbar.cfg` (`0x004567A4`),
        // `icons.ini` (`0x004567C3`), then the fixed sheets. A sheet that `toolbar.cfg` has already loaded with
        // 128 x 128 keeps this grid in `icons.ini`; the name counts exactly (`_mbscmp`).
        let cfg =
            "ICONS 128 128 data/icons/i0.tga\r\nICONS 64 64 data/icons/resourcen_icons.tga\r\n";
        let ini = "ICONS data/icons/i0.tga\r\nICONS data/icons/I1.tga\r\n";
        let mut grids = TileGrids::new();
        grids.register("data/icons/i1.tga", [10, 10]);
        let d = ToolbarData::from_texts_with(cfg, ini, &mut grids);
        assert_eq!(d.icon_sheets[0].tile, [128, 128]);
        assert_eq!(
            d.icon_sheets[1].tile,
            [50, 50],
            "different name (capitalization)"
        );
        assert_eq!(d.resource_icons.tile, [64, 64]);
        assert_eq!(grids.get("data/icons/I1.tga"), Some([50, 50]));
        assert_eq!(grids.get("data/icons/info_icons16x16.tga"), Some([16, 16]));
    }

    // QUIRK O-263: deviation
    #[test]
    fn icon_lookups_differ_in_missing_and_negative_values() {
        let d = ToolbarData::from_texts(CFG, INI);
        // Valid: columns 4/5 = (sheet, tile) of the icon.
        assert_eq!(d.icon_of(20), Some((0, 3)));
        assert_eq!(d.icon_clamped(20), Some((0, 3)));
        // Negative sheet or negative tile (O-263): strictly no icon, clamped 0.
        assert_eq!(d.icon_of(21), None);
        assert_eq!(d.icon_clamped(21), Some((0, 0)));
        assert_eq!(d.icon_of(22), None);
        assert_eq!(d.icon_clamped(22), Some((1, 0)));
        // Missing entry: both `None`.
        assert_eq!(d.icon_of(99), None);
        assert_eq!(d.icon_clamped(99), None);
        // Portrait: the sheet must be among the portrait sheets, values not negative.
        assert_eq!(d.portrait_of(20), Some((1, 4)));
        assert_eq!(d.portrait_of(21), None);
        assert_eq!(d.portrait_of(22), None);
        assert_eq!(d.portrait_of(23), None, "Blatt 5 von 2");
        assert_eq!(d.portrait_of(24), Some((1, 0)));
    }
}
