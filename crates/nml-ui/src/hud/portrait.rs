//! Portrait, values, tooltip areas of the bar and counter-upgrade row.
//!
//! Original functions: portrait and values `0x00456B70` (value check and formats `0x00455A30`), area list
//! `0x00455180`/`0x00453AD0`, area check `0x00453B50`, counter-upgrade row `0x00455C00` (re/game-ui N.6, N.14.5;
//! quirks O-238, O-239, O-240, O-242, O-259).
//!
//! The module draws only what the client hands it as a model ([`PortraitModel`]); it does not know `nml-sim`. The
//! order within the frame is: background -> [`draw_portrait`] -> [`draw_counter_row`] -> area check
//! ([`TooltipAreas::hit`]) -> children of the bar (buttons). The areas belong to the host (`Hud`), not the bar;
//! drawing does not change the bar.

use super::toolbar::button::{TooltipRequest, tile_at};
use super::toolbar::data::ToolbarData;
use super::{PANEL_INFO_FONT, TEXT_FONT};
use crate::bkg_paint::image_quad;
use crate::coords::toolbar_rect;
use crate::dialog::UiContext;
use crate::draw::{DrawCmd, Filter};
use crate::geom::contains;
use crate::text::{
    LayoutParams, PLAYER_COLORS, TextLayout, atlas_codes, draw_runs, draw_string, layout_text,
    measure,
};

/// White, the color of most draw commands of this module.
const WHITE: u32 = 0xFFFF_FFFF;
/// Black for the shadow of the counter-upgrade time.
const BLACK: u32 = 0xFF00_0000;
/// Kind of the tooltip event of the bar (`0x00453B50`: `{0xC, ..., guid, 0x17000000, 1}`).
pub const KIND_BAR_AREA: u32 = 0x1700_0000;
/// Wrap width and height of the subtitle (`0x82 x 0x22`, `0x00456E5B`); the layout does not evaluate the height.
const SUBTITLE_WIDTH: i32 = 130;
/// Layout flags of the subtitle `0x46` = font + color + wrap; without `0x200` the layout swallows `[KEYID]` (O-242).
const SUBTITLE_FLAGS: u32 = 0x46;
/// Layout flags of the value lines `6` = font + color, no wrap (`0x004570C9`).
const VALUE_FLAGS: u32 = 0x06;
/// Bottom edge of the subtitle relative to its slot: `y += 12 - text height` (`0x00456EEA`).
const SUBTITLE_BASELINE: i32 = 12;
/// Group text (subtitle of the nation icon), fixed (`0x00456C2F`: `PUSH 0x1F4`).
pub const GROUP_TEXT: i32 = 500;
/// Nation icon: GUID `nation + 500` (`0x00456C62`).
pub const NATION_SYMBOL_BASE: i32 = 500;
/// First slot of `PORTRAITPOSITION` for values (0 = portrait, 1 = subtitle).
const FIRST_VALUE_SLOT: usize = 2;
/// Text and icon of the value line overlap by 1 px: text at `x + 15` (`0x004571CC`).
const VALUE_TEXT_DX: i32 = 15;
/// Icon size of the value line (`0x0045714C`).
const VALUE_ICON: i32 = 16;
/// Color mark for delta >= 0 (`[COLOR FF00FF00]`, green).
const GREEN: &str = "[COLOR FF00FF00]";
/// Color mark for delta < 0 (`[COLOR FFFF0000]`, red).
const RED: &str = "[COLOR FFFF0000]";

/// Attribute 30 (experience): special format with rank.
pub const ATTR_EXPERIENCE: u32 = 30;
/// Attribute 23 (intake capacity): `occupied/max`.
pub const ATTR_CAPACITY: u32 = 23;
/// From this rank on, experience shows only `[rank]` (`0x00619E90`).
pub const RANK_ONLY_FROM: u8 = 5;

/// Counter-upgrade row: step from icon to icon to the left (`0x37`, `0x00455C00`).
pub const COUNTER_STEP: i32 = 55;
/// Counter-upgrade row: top edge of the icons (absolute, `0x1E`).
pub const COUNTER_Y: i32 = 30;
/// Counter-upgrade row: side length of the tooltip area, fixed (`0x32`), not the cell size (O-259).
pub const COUNTER_AREA: i32 = 50;

/// One of the 13 value kinds: attribute, icon in `info_icons16x16.tga` and tooltip GUID (`icon + 20`,
/// `ADD EDI,0x14` at `0x004571AA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueKind {
    /// Attribute ID of the configuration.
    pub attribute: u32,
    /// Tile in `info_icons16x16.tga`.
    pub icon: i32,
}

impl ValueKind {
    /// GUID of the tooltip text (`guids.deu` 20-32).
    pub const fn tooltip_guid(self) -> i32 {
        self.icon + 20
    }
}

/// The 13 value kinds in the fixed sequence of the loop `0x00456F96`...`0x004571FA` (tables on the stack
/// `0x00456F96` and `0x00457014`): hit points, experience, food, gold, wood, mana, stamina, morale, intake capacity,
/// ranged attack, melee attack, carrying capacity, living space.
pub const VALUE_ORDER: [ValueKind; 13] = [
    ValueKind {
        attribute: 14,
        icon: 0,
    },
    ValueKind {
        attribute: 30,
        icon: 7,
    },
    ValueKind {
        attribute: 24,
        icon: 1,
    },
    ValueKind {
        attribute: 26,
        icon: 2,
    },
    ValueKind {
        attribute: 25,
        icon: 3,
    },
    ValueKind {
        attribute: 15,
        icon: 4,
    },
    ValueKind {
        attribute: 28,
        icon: 5,
    },
    ValueKind {
        attribute: 29,
        icon: 6,
    },
    ValueKind {
        attribute: 23,
        icon: 8,
    },
    ValueKind {
        attribute: 20,
        icon: 9,
    },
    ValueKind {
        attribute: 19,
        icon: 10,
    },
    ValueKind {
        attribute: 17,
        icon: 11,
    },
    ValueKind {
        attribute: 27,
        icon: 12,
    },
];

/// One value of the portrait, read by the client from the attribute set of the object (`0x00455A30`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortraitValue {
    /// Attribute ID (one from [`VALUE_ORDER`]).
    pub attribute: u32,
    /// Base value (`GetBaseValue`).
    pub base: i32,
    /// Value with upgrades and modifiers (`GetMaxValue`); `delta = max - base`. For attribute 23 the maximum of the
    /// capacity.
    pub max: i32,
    /// Current value (`GetCurrentValue`). For attribute 23 the occupancy (response to `0x2217`).
    pub cur: i32,
    /// "Tracked" (byte `AttributeSet+0xB8+idx`, `0x0048F7A0`).
    pub tracked: bool,
    /// Rank (`GO+0x7A4`), read only for experience.
    pub rank: u8,
}

/// The object whose portrait is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortraitObject {
    /// Type GUID (`GO+0x54`).
    pub guid: u32,
    /// The present values **in the fixed sequence** of [`VALUE_ORDER`]: an attribute that the object does not have
    /// (`AttributeSet_IsPresent`) is missing from the list; the tracked intake capacity is missing if the response
    /// to `0x2217` is negative. Ownership and class play no role (O-239).
    pub values: Vec<PortraitValue>,
}

/// A running counter-upgrade (entry `{guid, rest_ms, source}` of the list `GameWorld+0x990`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterRow {
    /// GUID of the upgrade (icon and tooltip).
    pub guid: u32,
    /// Remaining time in milliseconds.
    pub rest_ms: i32,
    /// Color number of the source player (`Player_GetColor`, `0x004FABC0`); `None` if the player does not exist
    /// (then white). Numbers from 8 on are gray.
    pub source_color: Option<u32>,
    /// Enemy source (`GameWorld_HasEnemyRelation(local, source)`, `0x00476290`): only then does the row show the
    /// tinted square.
    pub enemy: bool,
}

/// Model for portrait, values and counter-upgrades that the client fills per frame. The case distinction (nothing /
/// group / object) is made by [`draw_portrait`] from `selected` and `hover` of the bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortraitModel {
    /// The object that the case "object" shows: the object for `Toolbar+0x524` (the only one selected), otherwise the
    /// one for `Toolbar+0x48` (under the mouse) - as `0x00456B70` resolves it. `None` if both objects are missing.
    pub object: Option<PortraitObject>,
    /// Nation of the local player (`Player+0x238`) for the nation icon of the group; `None` without a local player.
    pub group_nation: Option<i32>,
    /// The running counter-upgrades in list order.
    pub counter_upgrades: Vec<CounterRow>,
}

/// Text of a value line per the formats of `0x00455A30` (re/game-ui N.6.2, strings `0x00615978`, `0x00619E98`,
/// `0x00619CE0`, `0x00619E68`, `0x00619E84`, `0x00619E90`).
///
/// `delta = max - base`, the color mark is green for delta >= 0 and red for delta < 0. The "tracked" check comes first
/// (`0x00455A4F`...), the special cases apply only to tracked attributes:
/// - untracked (also experience and capacity): `base` or `base[COLOR ...]+/-delta`;
/// - tracked, experience (30): `cur/base [rank]`, from rank 5 only `[rank]` (never `max`);
/// - tracked, intake capacity (23): `cur/max` (occupied/maximum);
/// - tracked otherwise: `cur/base` or `cur/(base[COLOR ...]+/-delta[DEFAULTCOLOR])`.
pub fn value_text(v: &PortraitValue) -> String {
    let delta = v.max.wrapping_sub(v.base);
    let color = if delta >= 0 { GREEN } else { RED };
    // `FUN_00455A30` checks "tracked" before the special cases 30 and 23.
    if !v.tracked {
        return if delta == 0 {
            format!("{}", v.base)
        } else {
            format!("{}{color}{delta:+}", v.base)
        };
    }
    match v.attribute {
        ATTR_EXPERIENCE if v.rank < RANK_ONLY_FROM => {
            format!("{}/{} [{}]", v.cur, v.base, v.rank)
        }
        ATTR_EXPERIENCE => format!("[{}]", v.rank),
        ATTR_CAPACITY => format!("{}/{}", v.cur, v.max),
        _ if delta == 0 => format!("{}/{}", v.cur, v.base),
        _ => format!("{}/({}{color}{delta:+}[DEFAULTCOLOR])", v.cur, v.base),
    }
}

/// The tooltip areas of the bar (`Toolbar+0x54` rectangles, `+0x70` GUIDs; `0x00455180`). The owner is the host
/// (`Hud`), not the bar.
///
/// Deviation / O-240: instead of growing, the lists are rebuilt per frame. Portrait and values ([`draw_portrait`])
/// and the counter-upgrades ([`draw_counter_row`]) are kept separate so that in the case "nothing" the areas of
/// portrait and values stay (as in the original), while the counter-upgrade areas are replaced every frame instead of
/// doubled. On a hit the area appended last wins, counter-upgrades last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TooltipAreas {
    portrait: Vec<([i32; 4], i32)>,
    counter: Vec<([i32; 4], i32)>,
}

impl TooltipAreas {
    pub fn new() -> Self {
        Self::default()
    }

    /// All areas in append order: portrait and values, then counter-upgrades. Rectangle `(l, t, r, b)` and GUID.
    pub fn areas(&self) -> impl DoubleEndedIterator<Item = &([i32; 4], i32)> {
        self.portrait.iter().chain(self.counter.iter())
    }

    /// Areas of portrait and values from the last drawing: for an object first the portrait, then the value lines in
    /// drawing order; empty for a group or without a portrait entry (for `ui_click`).
    pub fn portrait_areas(&self) -> &[([i32; 4], i32)] {
        &self.portrait
    }

    /// Areas of the counter-upgrade row in drawing order (for `ui_click`).
    pub fn counter_areas(&self) -> &[([i32; 4], i32)] {
        &self.counter
    }

    /// Number of all areas.
    pub fn len(&self) -> usize {
        self.portrait.len() + self.counter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all areas (the host calls this when the bar is hidden and not drawn).
    pub fn clear(&mut self) {
        self.portrait.clear();
        self.counter.clear();
    }

    /// Area check `0x00453B50`: the last area that contains the mouse position (`PtInRect`), as a request
    /// `{guid, 0x17000000, 1,0}` to the tooltip window; `None` without a hit. The mouse position is the last one known
    /// from **every** move (`Toolbar::mouse_pos`).
    pub fn hit(&self, mouse: [i32; 2]) -> Option<TooltipRequest> {
        self.areas()
            .rev()
            .find(|(rect, _)| contains(*rect, mouse[0], mouse[1]))
            .map(|&(_, guid)| TooltipRequest {
                guid,
                kind: KIND_BAR_AREA,
                value: 1.0,
                required: [0; 5],
            })
    }
}

/// Draws portrait, subtitle and values and rebuilds the areas of portrait and values (`0x00456B70`, re/game-ui
/// N.6.1/N.6.2).
///
/// `selected` is `Toolbar+0x524` (HID for exactly one selected object, -1 for multi-selection, 0 without selection),
/// `hover` is `Toolbar+0x48` (object under the mouse, 0 none; `Toolbar::selected`/`hover`).
/// - Both 0: draw **nothing**, the areas stay (O-238, O-240).
/// - `selected == -1` and `hover == 0`: nation icon `500 + nation` with subtitle "Group" (text 500), no values, no
///   area.
/// - Otherwise the object from `m.object`; if its type GUID has no portrait entry (or the sheet lies beyond the
///   `PORTRAIT` sheets), everything is dropped - areas empty, no subtitle, no values.
///
/// The areas are cleared only after the early return of the case "nothing" (`0x00453AD0`). Missing textures only omit
/// the image; subtitle, values and areas are still produced.
pub fn draw_portrait(
    ctx: &mut UiContext<'_>,
    data: &ToolbarData,
    selected: i32,
    hover: u32,
    m: &PortraitModel,
    areas: &mut TooltipAreas,
    out: &mut Vec<DrawCmd>,
) {
    if selected == 0 && hover == 0 {
        return;
    }
    // Without `PORTRAITPOSITION` the original aborts (`0x00456BF0`), even before clearing the areas.
    let Some(&slot0) = data.portrait.first() else {
        return;
    };
    areas.portrait.clear();
    let [left, top, ..] = toolbar_rect(ctx.screen);
    let group = selected == -1 && hover == 0;

    let (subtitle, object) = if group {
        // Nation icon: without a local player no image, the subtitle is still shown (`0x00456C4D` -> `0x00456E1F`).
        if let Some(nation) = m.group_nation {
            let Some(portrait) = data.portrait_of(NATION_SYMBOL_BASE.wrapping_add(nation)) else {
                return;
            };
            draw_portrait_tile(ctx, data, portrait, [left + slot0[0], top + slot0[1]], out);
        }
        (GROUP_TEXT, None)
    } else {
        let Some(object) = m.object.as_ref() else {
            return;
        };
        let guid = object.guid as i32;
        let Some(portrait) = data.portrait_of(guid) else {
            return;
        };
        let origin = [left + slot0[0], top + slot0[1]];
        let tile = data.portrait_sheet(portrait.0).map_or([80, 80], |s| s.tile);
        areas.portrait.push((
            [
                origin[0],
                origin[1],
                origin[0] + tile[0],
                origin[1] + tile[1],
            ],
            guid,
        ));
        draw_portrait_tile(ctx, data, portrait, origin, out);
        (guid, Some(object))
    };

    // Without slot 1 the original aborts here (`0x00456E35`).
    let Some(&slot1) = data.portrait.get(1) else {
        return;
    };
    draw_subtitle(
        ctx,
        &ctx.texts.display(subtitle),
        [left + slot1[0], top + slot1[1]],
        out,
    );

    if let Some(object) = object {
        draw_values(ctx, data, object, [left, top], areas, out);
    }
}

/// Portrait tile at `origin` (`FUN_004028C0` with clip = target, fill level 1, point filter; `0x00456E0A`).
fn draw_portrait_tile(
    ctx: &mut UiContext<'_>,
    data: &ToolbarData,
    (sheet, tile): (usize, i32),
    origin: [i32; 2],
    out: &mut Vec<DrawCmd>,
) {
    let Some(sheet) = data.portrait_sheet(sheet) else {
        return;
    };
    let Some(texture) = sheet.texture(ctx) else {
        return;
    };
    let dst = [
        origin[0],
        origin[1],
        origin[0] + sheet.tile[0],
        origin[1] + sheet.tile[1],
    ];
    image_quad(
        ctx.assets,
        texture,
        sheet.tile,
        tile,
        dst,
        dst,
        WHITE,
        Filter::Point,
        out,
    );
}

/// Subtitle: `TextFont`, box 130 wide with wrapping, switch `0x46`, color white, with shadow. The bottom edge of the
/// text lies at `slot.y + 12`, multi-line names grow upward (`y += 12 - text height`, `0x00456EEA`).
fn draw_subtitle(ctx: &UiContext<'_>, text: &str, slot: [i32; 2], out: &mut Vec<DrawCmd>) {
    let layout = subtitle_layout(ctx, text);
    let y = slot[1] + (SUBTITLE_BASELINE - layout.height);
    draw_runs(&layout, [slot[0], y], true, None, ctx.fonts, out);
}

/// Layout of the subtitle (without `0x200`: `[KEYID n]` is swallowed, the space before it stays, O-242).
fn subtitle_layout(ctx: &UiContext<'_>, text: &str) -> TextLayout {
    let params = LayoutParams {
        flags: SUBTITLE_FLAGS,
        width: SUBTITLE_WIDTH,
        font: ctx.fonts.by_name(TEXT_FONT),
        color: WHITE,
    };
    let mut layout = TextLayout::default();
    layout_text(text, &params, ctx.fonts, &mut layout);
    layout
}

/// The value lines (`0x00456F96`...`0x004571FA`): in turn slots 2 and following of `PORTRAITPOSITION`; if there is no
/// slot left, the value is silently omitted (nine with eleven slots, O-239). Per line icon 16 x 16 (POINT, white) at
/// the slot, text `PanelInfo` with switch 6 and shadow at `x + 15`, area `(x, y, x + 16 + text width, y + 16)` with
/// the tooltip GUID `icon + 20`.
fn draw_values(
    ctx: &mut UiContext<'_>,
    data: &ToolbarData,
    object: &PortraitObject,
    [left, top]: [i32; 2],
    areas: &mut TooltipAreas,
    out: &mut Vec<DrawCmd>,
) {
    let font = ctx.fonts.by_name(PANEL_INFO_FONT);
    let mut slot = FIRST_VALUE_SLOT;
    for value in &object.values {
        let Some(kind) = VALUE_ORDER
            .iter()
            .find(|k| k.attribute == value.attribute)
            .copied()
        else {
            continue;
        };
        let Some(place) = data.portrait.get(slot) else {
            continue;
        };
        let [x, y] = [left + place[0], top + place[1]];
        tile_at(
            ctx,
            &data.info_icons,
            kind.icon,
            [x, y],
            WHITE,
            1.0,
            Filter::Point,
            out,
        );
        let params = LayoutParams {
            flags: VALUE_FLAGS,
            width: SUBTITLE_WIDTH,
            font,
            color: WHITE,
        };
        let mut layout = TextLayout::default();
        layout_text(&value_text(value), &params, ctx.fonts, &mut layout);
        areas.portrait.push((
            [x, y, x + VALUE_ICON + layout.width, y + VALUE_ICON],
            kind.tooltip_guid(),
        ));
        draw_runs(&layout, [x + VALUE_TEXT_DX, y], true, None, ctx.fonts, out);
        slot += 1;
    }
}

/// Counter-upgrade row (`0x00455C00`, re/game-ui N.14.5, O-259): start `(W - 55, 30)`, for each further entry
/// `x -= 55`.
///
/// Per entry with an `icons.ini` icon (without an icon skipped, `x` stays):
/// - icon 50 x 50 POINT white, area fixed `(x, 30, x + 50, 80)` (not the cell size);
/// - remaining time `m:ss` in `TextFont` **right-aligned** to the icon edge at `y = 30 + cell height`: first black at
///   `(x_text - 1, y + 1)`, then white at `(x_text, y)` - two [`draw_string`] calls, not the layout shadow;
/// - for an enemy source `counterupgrade.tga` tile 0 as a square at text height left below the icon, LINEAR, tinted
///   with the player color (no frame).
///
/// The areas of this row are replaced on every call (deviation), they do not double.
pub fn draw_counter_row(
    ctx: &mut UiContext<'_>,
    data: &ToolbarData,
    rows: &[CounterRow],
    areas: &mut TooltipAreas,
    out: &mut Vec<DrawCmd>,
) {
    areas.counter.clear();
    let font = ctx.fonts.by_name(TEXT_FONT);
    let mut x = ctx.screen.width - COUNTER_STEP;
    for row in rows {
        let Some((sheet, tile)) = data.icon_of(row.guid as i32) else {
            continue;
        };
        // A sheet beyond the list makes the original abort (`0x00455C45`); the reimplementation omits the entry.
        let Some(icons) = data.icon_sheet(sheet) else {
            continue;
        };
        let [cell_w, cell_h] = icons.tile;
        tile_at(
            ctx,
            icons,
            tile,
            [x, COUNTER_Y],
            WHITE,
            1.0,
            Filter::Point,
            out,
        );
        areas.counter.push((
            [x, COUNTER_Y, x + COUNTER_AREA, COUNTER_Y + COUNTER_AREA],
            row.guid as i32,
        ));

        let y = COUNTER_Y + cell_h;
        let text = format!("{}:{:02}", row.rest_ms / 60_000, (row.rest_ms / 1000) % 60);
        let codes = atlas_codes(&text);
        let [text_w, text_h] = measure(&codes, font, ctx.fonts);
        let text_x = x - text_w + cell_w;
        draw_string(&codes, [text_x - 1, y + 1], BLACK, font, ctx.fonts, out);
        draw_string(&codes, [text_x, y], WHITE, font, ctx.fonts, out);

        if row.enemy
            && let Some(texture) = data.counter_overlay.texture(ctx)
        {
            let dst = [x, y, x + text_h, y + text_h];
            image_quad(
                ctx.assets,
                texture,
                data.counter_overlay.tile,
                0,
                dst,
                dst,
                player_color(row.source_color),
                Filter::Linear,
                out,
            );
        }
        x -= COUNTER_STEP;
    }
}

/// Color of a player from the color number (`0x00619D94`): white without a player, numbers from 8 gray.
fn player_color(color: Option<u32>) -> u32 {
    match color {
        None => WHITE,
        Some(i) => PLAYER_COLORS[(i as usize).min(PLAYER_COLORS.len() - 1)],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use nml_formats::localisation::parse_localisation;

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::draw::TextureId;
    use crate::fonts::FontBook;
    use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};

    /// Top edge of the bar at 1024 x 768.
    const BAR_Y: i32 = 768 - 176;
    const GREEN_COLOR: u32 = 0xFF00_FF00;
    const RED_COLOR: u32 = 0xFFFF_0000;

    const CFG: &str = "TAG PORTRAITPOSITION\r\nPOS -2 48\r\nPOS 10 152\r\nPOS 4 1\r\nPOS 4 32\r\nPOS 90 110\r\nPOS 90 95\r\nPOS 90 80\r\nPOS 90 65\r\nPOS 90 50\r\nPOS 90 35\r\nPOS 90 20\r\n/TAG\r\n";
    /// GUID 100 (headquarters with `[KEYID]`) and 101: portrait sheet 0, tiles 3 and 4, icon 0/7 or 0/8; 102 without
    /// portrait (tile -1); 500/501 nation icons.
    const INI: &str = "PORTRAIT data/icons/p0.tga\r\nICONS data/icons/i0.tga\r\nGUID 100 0 3 0 7\r\nGUID 101 0 4 0 8\r\nGUID 102 0 -1 0 9\r\nGUID 103 5 2 0 9\r\nGUID 500 0 6 0 0\r\nGUID 501 0 7 0 1\r\nGUID 700 0 0 0 11\r\nGUID 701 0 0 0 12\r\n";

    struct Rig {
        vfs: TestVfs,
        assets: UiAssets,
        texts: Texts,
        fonts: FontBook,
        data: ToolbarData,
        areas: TooltipAreas,
    }

    /// `TextFont`: 10 px advance, line height 15. `PanelInfo`: 7 px advance, height 12. Both with cell 16.
    fn rig() -> Rig {
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            (
                "data/fonts/Test_15_0_0_0_0.tga",
                tga_bottom_up(256, 256, |_, _| [255; 4]),
            ),
            (
                "data/fonts/Test_12_0_0_0_0.fnt",
                fnt_text(|_| 7, |_| 12).into_bytes(),
            ),
            (
                "data/fonts/Test_12_0_0_0_0.tga",
                tga_bottom_up(256, 256, |_, _| [255; 4]),
            ),
            (
                "data/icons/p0.tga",
                tga_bottom_up(320, 160, |_, _| [255; 4]),
            ),
            (
                "data/icons/i0.tga",
                tga_bottom_up(200, 200, |_, _| [255; 4]),
            ),
            (
                "data/icons/info_icons16x16.tga",
                tga_bottom_up(208, 16, |_, _| [255; 4]),
            ),
            (
                "data/icons/counterupgrade.tga",
                tga_bottom_up(64, 64, |_, _| [255; 4]),
            ),
        ]);
        let mut assets = UiAssets::new();
        let loc = parse_localisation(
            "Font TextFont\nFontSize 15\nTextureFont\nFontName Test\n\
Font PanelInfo\nFontSize 12\nTextureFont\nFontName Test\n",
        );
        let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let texts = Texts::from_entries(&[
            (100, "Commander [KEYID 1000]"),
            (101, "Worker"),
            (500, "Gruppe"),
            (700, "First Upgrade"),
        ]);
        Rig {
            vfs,
            assets,
            texts,
            fonts,
            data: ToolbarData::from_texts(CFG, INI),
            areas: TooltipAreas::new(),
        }
    }

    impl Rig {
        fn portrait(&mut self, selected: i32, hover: u32, m: &PortraitModel) -> Vec<DrawCmd> {
            let mut ctx = UiContext {
                vfs: &self.vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen: Screen {
                    width: 1024,
                    height: 768,
                },
            };
            let mut out = Vec::new();
            draw_portrait(
                &mut ctx,
                &self.data,
                selected,
                hover,
                m,
                &mut self.areas,
                &mut out,
            );
            out
        }

        fn counter(&mut self, rows: &[CounterRow]) -> Vec<DrawCmd> {
            let mut ctx = UiContext {
                vfs: &self.vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen: Screen {
                    width: 1024,
                    height: 768,
                },
            };
            let mut out = Vec::new();
            draw_counter_row(&mut ctx, &self.data, rows, &mut self.areas, &mut out);
            out
        }

        fn atlas(&self, name: &str) -> TextureId {
            self.fonts.font(self.fonts.by_name(name)).atlas
        }

        fn tex(&self, path: &str) -> TextureId {
            self.assets.find(path).expect("image loaded")
        }
    }

    type Q = (Option<TextureId>, [i32; 4], u32, Filter);

    fn quads(cmds: &[DrawCmd]) -> Vec<Q> {
        cmds.iter()
            .map(|c| {
                let DrawCmd::Quad {
                    texture,
                    dst,
                    color,
                    filter,
                    ..
                } = *c
                else {
                    unreachable!()
                };
                (texture, dst, color, filter)
            })
            .collect()
    }

    /// Position and color of the text quads of an atlas in drawing order.
    fn text_at(cmds: &[DrawCmd], atlas: TextureId) -> Vec<([i32; 2], u32)> {
        quads(cmds)
            .into_iter()
            .filter(|q| q.0 == Some(atlas))
            .map(|q| ([q.1[0], q.1[1]], q.2))
            .collect()
    }

    fn value(attribute: u32, base: i32, max: i32, cur: i32, tracked: bool) -> PortraitValue {
        PortraitValue {
            attribute,
            base,
            max,
            cur,
            tracked,
            rank: 0,
        }
    }

    fn object(guid: u32, values: Vec<PortraitValue>) -> PortraitModel {
        PortraitModel {
            object: Some(PortraitObject { guid, values }),
            ..PortraitModel::default()
        }
    }

    fn counter(guid: u32, rest_ms: i32, enemy: bool) -> CounterRow {
        CounterRow {
            guid,
            rest_ms,
            source_color: Some(3),
            enemy,
        }
    }

    #[test]
    fn nothing_selected_nothing_hovered_draws_nothing_and_keeps_areas() {
        // O-238: nothing selected, nothing under the mouse -> nothing drawn.
        // O-240: the areas of the last portrait stay (tooltip over the empty bar), as in the original.
        let mut rig = rig();
        let m = object(100, vec![value(14, 80, 80, 60, true)]);
        let shown = rig.portrait(5, 0, &m);
        assert!(!shown.is_empty());
        let before = rig.areas.clone();
        assert_eq!(before.len(), 2, "portrait and hit points");

        let out = rig.portrait(0, 0, &m);
        assert!(out.is_empty());
        assert_eq!(rig.areas, before, "areas stay");
        // The areas still hit: portrait (-2, 640)...(78, 720) yields the type GUID.
        let hit = rig.areas.hit([10, 650]).unwrap();
        assert_eq!((hit.guid, hit.kind), (100, 0x1700_0000));
    }

    #[test]
    fn group_shows_the_nation_symbol_with_text_500() {
        let mut rig = rig();
        let m = PortraitModel {
            group_nation: Some(1),
            // An object in the model must have no effect in the group case.
            ..object(101, vec![value(14, 10, 10, 10, true)])
        };
        let out = rig.portrait(-1, 0, &m);
        let q = quads(&out);
        let p0 = rig.tex("data/icons/p0.tga");
        // Nation icon 500 + 1: portrait tile 7 at slot 0 (-2, 48), 80 x 80, POINT, white.
        assert_eq!(q[0].0, Some(p0));
        assert_eq!(q[0].1, [-2, BAR_Y + 48, 78, BAR_Y + 128]);
        assert_eq!((q[0].2, q[0].3), (0xFFFF_FFFF, Filter::Point));
        // Subtitle "Gruppe" (6 characters) in `TextFont`, with shadow: 12 quads; no values, no area.
        let text = text_at(&out, rig.atlas("TextFont"));
        assert_eq!(text.len(), 12);
        assert!(text_at(&out, rig.atlas("PanelInfo")).is_empty());
        assert_eq!(q.len(), 1 + 12, "only image and subtitle");
        assert!(rig.areas.is_empty());
        // Without a local player: no image, the subtitle stays (`0x00456C4D` -> `0x00456E1F`).
        let none = PortraitModel::default();
        let out = rig.portrait(-1, 0, &none);
        assert_eq!(quads(&out).len(), 12);
        // Nation without a portrait entry: everything is dropped.
        let missing = PortraitModel {
            group_nation: Some(3),
            ..PortraitModel::default()
        };
        assert!(rig.portrait(-1, 0, &missing).is_empty());
    }

    #[test]
    fn hovered_foreign_object_replaces_empty_or_group_display() {
        // Without selection and with multi-selection the object under the mouse replaces the display (also foreign
        // ones).
        let m = object(101, vec![value(14, 40, 40, 25, true)]);
        for selected in [0, -1] {
            let mut rig = rig();
            let out = rig.portrait(selected, 9, &m);
            let q = quads(&out);
            // Portrait tile 4 of type 101, behind it "Worker" as subtitle, icon and value.
            assert_eq!(q[0].1, [-2, BAR_Y + 48, 78, BAR_Y + 128]);
            assert_eq!(
                text_at(&out, rig.atlas("TextFont")).len(),
                2 * "Worker".len()
            );
            assert_eq!(
                text_at(&out, rig.atlas("PanelInfo")).len(),
                2 * "25/40".len()
            );
            let guids: Vec<_> = rig.areas.areas().map(|a| a.1).collect();
            assert_eq!(guids, [101, 20], "portrait and hit points");
        }
        // If the type GUID has no portrait (tile -1, missing entry, sheet beyond the list), everything is dropped and
        // the areas are cleared.
        for guid in [102, 999, 103] {
            let mut rig = rig();
            rig.portrait(5, 0, &m);
            assert!(!rig.areas.is_empty());
            let out = rig.portrait(0, 9, &object(guid, vec![value(14, 1, 1, 1, true)]));
            assert!(out.is_empty(), "GUID {guid}");
            assert!(rig.areas.is_empty(), "GUID {guid}");
        }
        // If the object is missing entirely (model empty), nothing remains either.
        let mut rig = rig();
        assert!(rig.portrait(5, 0, &PortraitModel::default()).is_empty());
    }

    #[test]
    fn value_formats_follow_n_6_2() {
        // Untracked `%d` or `base[COLOR FF00FF00]+delta`; tracked `cur/base` or `cur/(base...)`;
        // experience rank 3 `cur/base [3]`, rank 5 `[5]`.
        let v = |a, base, max, cur, tracked| value_text(&value(a, base, max, cur, tracked));
        assert_eq!(v(20, 12, 12, 12, false), "12");
        assert_eq!(v(20, 12, 15, 12, false), "12[COLOR FF00FF00]+3");
        // delta < 0 red.
        assert_eq!(v(20, 12, 10, 12, false), "12[COLOR FFFF0000]-2");
        assert_eq!(v(14, 80, 80, 60, true), "60/80");
        assert_eq!(
            v(14, 80, 90, 60, true),
            "60/(80[COLOR FF00FF00]+10[DEFAULTCOLOR])"
        );
        assert_eq!(
            v(14, 80, 70, 60, true),
            "60/(80[COLOR FFFF0000]-10[DEFAULTCOLOR])"
        );
        // Experience: never `max`; rank below 5 with rank, from 5 only the rank.
        let xp = |rank, max| {
            value_text(&PortraitValue {
                rank,
                ..value(30, 100, max, 40, true)
            })
        };
        assert_eq!(xp(3, 100), "40/100 [3]");
        assert_eq!(xp(4, 130), "40/100 [4]");
        assert_eq!(xp(5, 100), "[5]");
        assert_eq!(xp(9, 100), "[9]");
        // Intake capacity: tracked occupied/maximum; untracked the general rule applies (`!tracked` comes before the
        // special cases in the original, `0x00455A30`).
        assert_eq!(v(23, 8, 8, 3, true), "3/8");
        assert_eq!(v(23, 6, 8, 3, false), "6[COLOR FF00FF00]+2");
        assert_eq!(v(23, 8, 8, 3, false), "8");
        // Experience untracked: only `base` or `base+/-delta`, neither rank nor `cur` (also from rank 5).
        let xp_untracked = |rank, max| {
            value_text(&PortraitValue {
                rank,
                ..value(30, 100, max, 40, false)
            })
        };
        assert_eq!(xp_untracked(3, 100), "100");
        assert_eq!(xp_untracked(5, 100), "100");
        assert_eq!(xp_untracked(3, 90), "100[COLOR FFFF0000]-10");

        // Drawn: delta part green, rest white; the shadow carries only the alpha of the color.
        let mut rig = rig();
        let m = object(100, vec![value(20, 12, 15, 12, false)]);
        let out = rig.portrait(5, 0, &m);
        let colors: BTreeSet<u32> = text_at(&out, rig.atlas("PanelInfo"))
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(
            colors,
            BTreeSet::from([0xFF00_0000, 0xFFFF_FFFF, GREEN_COLOR])
        );
        let m = object(100, vec![value(20, 12, 10, 12, false)]);
        let out = rig.portrait(5, 0, &m);
        assert!(
            text_at(&out, rig.atlas("PanelInfo"))
                .iter()
                .any(|t| t.1 == RED_COLOR)
        );
    }

    #[test]
    fn only_nine_values_fit() {
        // O-239: eleven slots for values... slots 0 and 1 belong to portrait and subtitle, nine remain. More values
        // are silently omitted, without icon, text and area.
        let mut rig = rig();
        let values: Vec<_> = VALUE_ORDER
            .iter()
            .map(|k| value(k.attribute, 5, 5, 5, false))
            .collect();
        assert_eq!(values.len(), 13);
        let out = rig.portrait(5, 0, &object(100, values));
        let info = rig.tex("data/icons/info_icons16x16.tga");
        let icons: Vec<_> = quads(&out)
            .into_iter()
            .filter(|q| q.0 == Some(info))
            .collect();
        assert_eq!(icons.len(), 9);
        // Slots in turn: (4, 1), (4, 32), then x = 90 from bottom to top.
        let places = [
            [4, 1],
            [4, 32],
            [90, 110],
            [90, 95],
            [90, 80],
            [90, 65],
            [90, 50],
            [90, 35],
            [90, 20],
        ];
        for (icon, place) in icons.iter().zip(places) {
            let [x, y] = [place[0], BAR_Y + place[1]];
            assert_eq!(icon.1, [x, y, x + 16, y + 16]);
            assert_eq!(icon.3, Filter::Point);
        }
        // The first nine value kinds in the fixed sequence: areas with GUID icon + 20 (portrait first).
        let guids: Vec<_> = rig.areas.areas().map(|a| a.1).collect();
        assert_eq!(guids, [100, 20, 27, 21, 22, 23, 24, 25, 26, 28]);
        // Width of the area: 16 + text width ("5" in `PanelInfo` = 7).
        let first = rig.areas.areas().nth(1).unwrap().0;
        assert_eq!(first, [4, BAR_Y + 1, 4 + 16 + 7, BAR_Y + 17]);
        // The text stands at x + 15, shadow (x + 15, y + 2) black before it.
        let text = text_at(&out, rig.atlas("PanelInfo"));
        assert_eq!(text[0], ([19, BAR_Y + 3], 0xFF00_0000));
        assert_eq!(text[1], ([21, BAR_Y + 1], 0xFFFF_FFFF));
    }

    #[test]
    fn missing_values_take_no_place() {
        // Only present values count: the second value stands in slot 3 (4, 32), even if it is the third kind.
        let mut rig = rig();
        let m = object(
            100,
            vec![value(14, 5, 5, 5, true), value(26, 7, 7, 7, false)],
        );
        let out = rig.portrait(5, 0, &m);
        let info = rig.tex("data/icons/info_icons16x16.tga");
        let icons: Vec<_> = quads(&out)
            .into_iter()
            .filter(|q| q.0 == Some(info))
            .map(|q| [q.1[0], q.1[1]])
            .collect();
        assert_eq!(icons, [[4, BAR_Y + 1], [4, BAR_Y + 32]]);
        // An unknown attribute does not count.
        let m = object(100, vec![value(99, 1, 1, 1, false)]);
        rig.portrait(5, 0, &m);
        assert_eq!(rig.areas.len(), 1);
    }

    #[test]
    fn subtitle_swallows_keyid_and_keeps_the_space() {
        // O-242: without switch 0x200 `[KEYID 1000]` is dropped, the space before it stays.
        // (The word is chosen short: a word with a space that is wider than 130 px at the start of a line is
        // truncated and would lose the space, O-80.)
        let mut rig = rig();
        let ctx = UiContext {
            vfs: &rig.vfs,
            assets: &mut rig.assets,
            fonts: &rig.fonts,
            texts: &rig.texts,
            screen: Screen {
                width: 1024,
                height: 768,
            },
        };
        let layout = subtitle_layout(&ctx, "Commander [KEYID 1000]");
        let runs: Vec<_> = layout
            .runs
            .iter()
            .map(|r| match &r.content {
                crate::text::RunContent::Text(codes) => codes.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(runs, [b"Commander ".to_vec()]);
        assert_eq!(layout.height, 15);

        // Drawn: bottom edge at slot 1 + 12, so y = 152 + 12 - 15 = 149; x = 10; shadow (x, y + 2).
        let m = object(100, Vec::new());
        let out = rig.portrait(5, 0, &m);
        let text = text_at(&out, rig.atlas("TextFont"));
        assert_eq!(text.len(), 2 * 10);
        let y = BAR_Y + 152 + 12 - 15;
        assert_eq!(text[0], ([10, y + 2], 0xFF00_0000));
        assert_eq!(text[10], ([12, y], 0xFFFF_FFFF));
        // The trailing space is a character like the others (advance 10).
        assert_eq!(text[9].0[0] - text[0].0[0], 9 * 10);
    }

    #[test]
    fn long_subtitle_wraps_at_130_and_grows_upwards() {
        // Two lines (wrap at 130 px): the bottom edge stays at slot 1 + 12, the text grows upward.
        let mut rig = rig();
        rig.texts = Texts::from_entries(&[(101, "Aaaaaaaaaa Bbbbbbbbbb")]);
        let out = rig.portrait(5, 0, &object(101, Vec::new()));
        let text = text_at(&out, rig.atlas("TextFont"));
        let ys: BTreeSet<i32> = text.iter().map(|t| t.0[1]).collect();
        let top = BAR_Y + 152 + 12 - 30;
        assert_eq!(ys, BTreeSet::from([top, top + 2, top + 15, top + 17]));
    }

    #[test]
    fn counter_row_follows_the_right_edge() {
        // 1024 x 768: first icon (969, 30), second (914, 30), 50 x 50 POINT; time right-aligned at y 80 with shadow
        // (-1, +1); enemy source: square at text height (15) at (x, 80), LINEAR, tinted.
        let mut rig = rig();
        let rows = [counter(700, 65_000, true), counter(701, 3_599_000, false)];
        let out = rig.counter(&rows);
        let i0 = rig.tex("data/icons/i0.tga");
        let ov = rig.tex("data/icons/counterupgrade.tga");
        let q = quads(&out);
        let icons: Vec<_> = q.iter().filter(|q| q.0 == Some(i0)).collect();
        assert_eq!(icons[0].1, [969, 30, 1019, 80]);
        assert_eq!(icons[1].1, [914, 30, 964, 80]);
        assert!(
            icons
                .iter()
                .all(|q| (q.2, q.3) == (0xFFFF_FFFF, Filter::Point))
        );

        // "1:05" (4 characters * 10 = 40): x_text = 969 + 50 - 40 = 979; black (978, 81), then white (979, 80).
        let text = text_at(&out, rig.atlas("TextFont"));
        assert_eq!(text.len(), (4 + 5) * 2);
        assert_eq!(text[0], ([978, 81], 0xFF00_0000));
        assert_eq!(text[3], ([1008, 81], 0xFF00_0000));
        assert_eq!(text[4], ([979, 80], 0xFFFF_FFFF));
        // "59:59" (5 characters): x_text = 914 + 50 - 50 = 914.
        assert_eq!(text[8], ([913, 81], 0xFF00_0000));
        assert_eq!(text[13], ([914, 80], 0xFFFF_FFFF));
        // Square only for the enemy source: color no. 3 (red), text height 15, LINEAR.
        let squares: Vec<_> = q.iter().filter(|q| q.0 == Some(ov)).collect();
        assert_eq!(squares.len(), 1);
        assert_eq!(squares[0].1, [969, 80, 984, 95]);
        assert_eq!((squares[0].2, squares[0].3), (0xFFC4_1800, Filter::Linear));
        // Order per entry: icon, shadow, text, square.
        let pos = |tex: Option<TextureId>| q.iter().position(|q| q.0 == tex).unwrap();
        assert!(pos(Some(i0)) < pos(Some(rig.atlas("TextFont"))));
        assert!(pos(Some(rig.atlas("TextFont"))) < pos(Some(ov)));
    }

    #[test]
    fn counter_row_skips_entries_without_icon_and_keeps_the_gap_closed() {
        // Without an `icons.ini` entry or with a sheet beyond the list: skipped, x stays (no gap).
        let mut rig = rig();
        let rows = [counter(999, 1000, true), counter(700, 61_000, false)];
        let out = rig.counter(&rows);
        let i0 = rig.tex("data/icons/i0.tga");
        let icons: Vec<_> = quads(&out)
            .into_iter()
            .filter(|q| q.0 == Some(i0))
            .collect();
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0].1[0], 969);
        assert_eq!(rig.areas.len(), 1);
        // Player unknown -> white, number from 8 -> gray.
        assert_eq!(player_color(None), 0xFFFF_FFFF);
        assert_eq!(player_color(Some(0)), 0xFFFF_CE0D);
        assert_eq!(player_color(Some(8)), 0xFFAC_ACAC);
        assert_eq!(player_color(Some(12)), 0xFFAC_ACAC);
    }

    #[test]
    fn counter_tooltip_area_is_fixed_50() {
        // O-259: the area is 50 x 50, even if the cell of the icon sheet has a different size; the time row follows
        // the cell height instead (here 40 -> y = 70).
        let mut rig = rig();
        rig.data.icon_sheets[0].tile = [40, 40];
        let rows = [counter(700, 5_000, false), counter(701, 5_000, false)];
        let out = rig.counter(&rows);
        let areas: Vec<_> = rig.areas.areas().copied().collect();
        assert_eq!(
            areas,
            [([969, 30, 1019, 80], 700), ([914, 30, 964, 80], 701),]
        );
        let i0 = rig.tex("data/icons/i0.tga");
        let first = quads(&out).into_iter().find(|q| q.0 == Some(i0)).unwrap();
        assert_eq!(first.1, [969, 30, 1009, 70]);
        let text = text_at(&out, rig.atlas("TextFont"));
        // "0:05": x_text = 969 + 40 - 40 = 969; y = 70.
        assert_eq!(text[4], ([969, 70], 0xFFFF_FFFF));
        // Hit: top left belongs to it, the right and bottom edge do not; the last area wins.
        assert_eq!(rig.areas.hit([969, 30]).unwrap().guid, 700);
        assert_eq!(rig.areas.hit([1018, 79]).unwrap().guid, 700);
        assert!(rig.areas.hit([1019, 79]).is_none());
        assert!(rig.areas.hit([969, 80]).is_none());
    }

    #[test]
    fn counter_areas_are_rebuilt_each_frame_and_never_duplicate() {
        // Deviation: the original appends the areas again every frame (the list grows); the reimplementation replaces
        // them. Portrait areas survive that (O-240).
        let mut rig = rig();
        rig.portrait(5, 0, &object(100, vec![value(14, 5, 5, 5, true)]));
        let rows = [counter(700, 5_000, false)];
        for _ in 0..3 {
            rig.counter(&rows);
        }
        assert_eq!(rig.areas.len(), 3);
        // Without entries the counter-upgrade areas are gone, those of the portrait stay.
        rig.counter(&[]);
        assert_eq!(rig.areas.len(), 2);
        // In the case "nothing" both parts stay as they were.
        rig.counter(&rows);
        let before = rig.areas.clone();
        rig.portrait(0, 0, &PortraitModel::default());
        assert_eq!(rig.areas, before);
        rig.areas.clear();
        assert!(rig.areas.is_empty());
    }

    #[test]
    fn hit_prefers_the_last_area_and_reports_the_bar_kind() {
        // N.6.3: `{0xC, ..., guid, 0x17000000, 1}`; with several hits the last one wins (counter-upgrades after the
        // values).
        let mut areas = TooltipAreas::new();
        areas.portrait.push(([0, 0, 100, 100], 1));
        areas.portrait.push(([50, 50, 150, 150], 2));
        areas.counter.push(([60, 60, 70, 70], 3));
        let hit = |p| areas.hit(p).map(|r| (r.guid, r.kind, r.value, r.required));
        assert_eq!(hit([10, 10]), Some((1, 0x1700_0000, 1.0, [0; 5])));
        assert_eq!(hit([55, 55]).unwrap().0, 2);
        assert_eq!(hit([65, 65]).unwrap().0, 3);
        assert_eq!(hit([200, 200]), None);
    }

    #[test]
    fn value_kinds_follow_the_fixed_table() {
        // Tables on the stack of `0x00456F96` (attributes) and `0x00457014` (icons); GUID = icon + 20.
        let attributes: Vec<_> = VALUE_ORDER.iter().map(|k| k.attribute).collect();
        let icons: Vec<_> = VALUE_ORDER.iter().map(|k| k.icon).collect();
        assert_eq!(
            attributes,
            [14, 30, 24, 26, 25, 15, 28, 29, 23, 20, 19, 17, 27]
        );
        assert_eq!(icons, [0, 7, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12]);
        let guids: Vec<_> = VALUE_ORDER.iter().map(|k| k.tooltip_guid()).collect();
        assert_eq!(guids, [20, 27, 21, 22, 23, 24, 25, 26, 28, 29, 30, 31, 32]);
    }

    #[test]
    fn missing_textures_draw_no_images_but_keep_text_and_areas() {
        // Without image sheets only the images are omitted; subtitle, value text and areas are still produced.
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            (
                "data/fonts/Test_15_0_0_0_0.tga",
                tga_bottom_up(256, 256, |_, _| [255; 4]),
            ),
        ]);
        let mut assets = UiAssets::new();
        let loc = parse_localisation("Font TextFont\nFontSize 15\nTextureFont\nFontName Test\n");
        let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let texts = Texts::from_entries(&[(101, "Worker")]);
        let data = ToolbarData::from_texts(CFG, INI);
        let mut ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: &fonts,
            texts: &texts,
            screen: Screen {
                width: 1024,
                height: 768,
            },
        };
        let mut areas = TooltipAreas::new();
        let mut out = Vec::new();
        let m = object(101, vec![value(14, 5, 5, 5, true)]);
        draw_portrait(&mut ctx, &data, 5, 0, &m, &mut areas, &mut out);
        let atlas = fonts.font(fonts.by_name("TextFont")).atlas;
        assert!(quads(&out).iter().all(|q| q.0 == Some(atlas)));
        assert_eq!(areas.len(), 2);
        let mut out = Vec::new();
        draw_counter_row(
            &mut ctx,
            &data,
            &[counter(700, 1000, true)],
            &mut areas,
            &mut out,
        );
        assert!(quads(&out).iter().all(|q| q.0 == Some(atlas)));
        assert_eq!(areas.len(), 3);
    }
}
