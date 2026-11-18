//! Tooltip window of the game UI: request from bar and areas, content by tooltip kind, measuring and drawing of the
//! box.
//!
//! Basis: re/game-ui 2.5, N.8 and N.14.6; functions of the original: constructor `0x004592E0`, event `0x00459000`
//! (`0xC` set via `0x00458A80`, `0xD` clear via `0x004589F0`), measuring `0x00459030`, drawing `0x00459260`, box
//! `0x00458DA0`. Quirks O-44, O-241 ... O-243, O-245, O-264.
//!
//! **Lifetime (O-44):** The original draws the tooltip and then sets the kind to 0; without a new `0xC` in the next
//! frame it disappears. The reimplementation separates state and drawing: per frame the host first calls
//! [`TooltipWindow::begin_frame`] (the request of the previous frame expires), then the areas
//! (`TooltipAreas::hit`) and the buttons (`Toolbar::update`, writing via [`TooltipWindow::set`] or directly into
//! [`TooltipWindow::request`]; the last one wins) and then draws with the purely reading [`TooltipWindow::draw`]. A
//! request thus lives from one frame step to the next.

use crate::bkg_paint::fill_rect;
use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::fonts::FontBook;
use crate::hud::TEXT_FONT;
use crate::hud::toolbar::button::TooltipRequest;
use crate::text::{
    GameTags, GameText, LayoutParams, TagParams, TextContext, TextLayout, draw_runs, layout_flags,
    layout_text_game, resolve_text, text_or_number,
};

/// Width `W` of the layout (`0x00459030`: `{300, 400}`; the layout does not evaluate the 400).
pub const TOOLTIP_WIDTH: i32 = 300;
/// Layout flags of the three sections (`599`): font, color, width, `[KEYID]` (`0x200`) and the unused bits
/// `0x01`/`0x10`.
pub const TOOLTIP_FLAGS: u32 = 0x257;
/// Base color of sections 1 and 2 (`0x00459260` passes `0xFFFFFFFF`).
pub const TOOLTIP_COLOR: u32 = 0xFFFF_FFFF;
/// Base color of the extended text (`0x00459030`: `0xFFFFFF96`).
pub const ENHANCED_COLOR: u32 = 0xFFFF_FF96;
/// Offset of the extended texts (`extendedtooltips.*`: GUID + 10000).
pub const ENHANCED_OFFSET: i32 = 10_000;
/// Area of the box (`0x00458DA0`, state block `alpharect1`): semi-transparent.
pub const BOX_COLOR: u32 = 0x8000_0080;
/// Outline of the box: the same vertex color, but the state block `alpharect2` does not blend - measured in the image
/// as opaque `(0, 0, 128)`.
pub const OUTLINE_COLOR: u32 = 0xFF00_0080;
/// Distance of the box bottom edge from the bottom screen edge (`Renderer+0xA8 - 200`).
pub const BOX_BOTTOM_GAP: i32 = 200;

/// Factor `0x00618804` (100.0) for the progress in percent.
const PERCENT: f32 = 100.0;

/// Tooltip kind `+0x3C` (`0x00458A80`, re/game-ui N.8); the code is the number of the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipArt {
    /// 1: BUILD, ONLYONCE, MAKE, UPGRADE, MAKEONLY_ONCE - name, text 4 (`[RESOURCES]`).
    Costs = 1,
    /// 2: requirement missing (`0x9000000`) - name, text 5 (`[REQUIREMENTS]`).
    Requirements = 2,
    /// 3: multi-selection, occupant (`0xB000000`), hero - type name of the object.
    ObjectType = 3,
    /// 4: production - name, text 301 (`[PROGRESS] fertiggestellt`) with switch `0x20`.
    Progress = 4,
    /// 5: everything else (target player `0xC000000`, stance, formation, areas `0x17000000`, without kind) - name.
    Name = 5,
    /// 7: SELECTHEADHUNTER, headhunter (`0x16000000`) - name, text 9 (`[HEADHUNTERREWARD]`).
    Headhunter = 7,
    /// 8: COUNTERUPGRADE - name, text 4.
    CounterUpgrade = 8,
    /// 9: DO, TARGET, TARGET_DO - name, text 4.
    Action = 9,
    /// 10: SHORTCUT - name, text 302 with switch `0x20` (302 is missing in all files and appears as "302", O-241).
    Shortcut = 10,
}

impl TooltipArt {
    /// Code of the original.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Text of the additional line (section 2) and whether it is laid out with switch `0x20` and the percentage.
    fn extra(self) -> Option<(i32, bool)> {
        match self {
            Self::Costs | Self::CounterUpgrade | Self::Action => Some((4, false)),
            Self::Requirements => Some((5, false)),
            Self::Progress => Some((301, true)),
            Self::Shortcut => Some((302, true)),
            Self::Headhunter => Some((9, false)),
            Self::ObjectType | Self::Name => None,
        }
    }
}

/// Content per `0x00458A80`: kind, GUID of the text (`+0x248`; for kind 3 the type of the object), value of the
/// parameter block (`+0x278`: factor, only BUILD/ONLYONCE take over the value of the event, otherwise 1) and the
/// percentage (`+0x298 = floor(value * 100)`, only kinds 4 and 10, otherwise 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooltipContent {
    pub art: TooltipArt,
    pub guid: i32,
    pub value: i32,
    pub percent: i32,
}

/// Progress in percent like `FLD float [ESP+0xC]; FMUL float [0x00618804]; CALL _ftol2` (`0x00458B96`): both
/// operands `f32`; with the x87 precision of 24 bits that the project assumes (see re/sim-core and re/combat on x87
/// precision), the product rounds to `f32`, then it is truncated toward 0. 0.29 yields 29 this way (in extended
/// precision it would be 28).
fn percent_of(value: f32) -> i32 {
    (value * PERCENT) as i32
}

/// `0x00458A80` (without the unused fields `x`, `bottom` and the name text `+0x40`): kind by `kind & 0xFF000000`.
///
/// The request carries only the kind (masked); in the original, `0x00458A80` stores the full flag word in `+0x260`,
/// but reads none of its lower bits, and no other access of the window (`0x00459030`, `0x00459260`, `0x00458A00`,
/// event `0x00459000`) reads `+0x260`: the masking changes nothing.
///
/// Kind 3 looks up the object of the HID (`GameWorld_GetObjectById`); if it is missing, the kind stays 0 (no tooltip).
pub fn tooltip_content(request: &TooltipRequest, game: &dyn GameText) -> Option<TooltipContent> {
    use opennml_formats::gifcon::button_kind as k;
    let kind = request.kind & k::KIND_MASK;
    let simple = |art| TooltipContent {
        art,
        guid: request.guid,
        value: 1,
        percent: 0,
    };
    let content = match kind {
        // `0x00458C02`: `+0x278 = ev[5]`. For these kinds the event carries the integer word 1 (`0x0044F742`); the
        // reimplementation keeps the value as `f32` and truncates it.
        k::BUILD | k::ONLYONCE => TooltipContent {
            value: request.value as i32,
            ..simple(TooltipArt::Costs)
        },
        k::MAKE | k::UPGRADE | k::MAKEONLY_ONCE => simple(TooltipArt::Costs),
        k::DO | k::TARGET | k::TARGET_DO => simple(TooltipArt::Action),
        // `0x00458CF8`: selection, occupant, hero -> type of the object.
        KIND_SELECTION_ART | KIND_OCCUPANT | KIND_HERO_ART => TooltipContent {
            guid: game.object_type(request.guid)?,
            ..simple(TooltipArt::ObjectType)
        },
        KIND_PRODUCTION_ART => TooltipContent {
            percent: percent_of(request.value),
            ..simple(TooltipArt::Progress)
        },
        k::SHORTCUT => TooltipContent {
            percent: percent_of(request.value),
            ..simple(TooltipArt::Shortcut)
        },
        KIND_MISSING => simple(TooltipArt::Requirements),
        k::SELECTHEADHUNTER | KIND_HEADHUNTER => simple(TooltipArt::Headhunter),
        k::COUNTERUPGRADE => simple(TooltipArt::CounterUpgrade),
        // `0xC000000` (target player, `0x00458A00` only sets the unused name text), `0x17000000` and all others.
        _ => simple(TooltipArt::Name),
    };
    Some(content)
}

/// Kind of the multi-selection (`0x5000000`).
const KIND_SELECTION_ART: u32 = super::toolbar::button::KIND_SELECTION;
/// Kind of the occupants (`0xB000000`).
const KIND_OCCUPANT: u32 = 0xB00_0000;
/// Kind of the hero buttons (`0x14000000`).
const KIND_HERO_ART: u32 = super::toolbar::button::KIND_HERO;
/// Kind of the production row (`0x6000000`).
const KIND_PRODUCTION_ART: u32 = super::toolbar::button::KIND_PRODUCTION;
/// "Requirement missing" (`0x9000000`).
const KIND_MISSING: u32 = super::toolbar::button::KIND_MISSING_REQUIREMENT;
/// Headhunter target (`0x16000000`).
const KIND_HEADHUNTER: u32 = 0x1600_0000;

/// One layout call of `0x00459030`: text, switch, parameter block, base color.
struct Section {
    text: String,
    flags: u32,
    params: TagParams,
    color: u32,
}

/// The up to three sections (`0x00459030`), each starting at `x = 0` below the previous one:
/// 1. Text of the GUID via `FUN_0055FFE0` (missing -> number, O-241), value `+0x278`;
/// 2. the additional line of the kind (likewise via `FUN_0055FFE0`); kinds 4 and 10 first set switch `0x20` and the
///    percentage into the parameter block - both also apply to section 3 further on, as in the original;
/// 3. with `EnhancedTooltips` the text `GUID + 10000`, only if it exists (`FUN_0055FFC0`, without number
///    substitute), in `0xFFFFFF96`.
fn sections(content: &TooltipContent, game: &dyn GameText, enhanced: bool) -> Vec<Section> {
    let mut params = TagParams {
        guid: content.guid,
        slot: game.local_slot(),
        value: content.value,
    };
    let mut flags = TOOLTIP_FLAGS;
    let mut out = vec![Section {
        text: text_or_number(game, content.guid),
        flags,
        params,
        color: TOOLTIP_COLOR,
    }];
    if let Some((id, progress)) = content.art.extra() {
        if progress {
            params.value = content.percent;
            flags |= layout_flags::PROGRESS;
        }
        out.push(Section {
            text: text_or_number(game, id),
            flags,
            params,
            color: TOOLTIP_COLOR,
        });
    }
    if enhanced && let Some(text) = game.text(content.guid.wrapping_add(ENHANCED_OFFSET)) {
        out.push(Section {
            text,
            flags,
            params,
            color: ENHANCED_COLOR,
        });
    }
    out
}

/// The tooltip window (`0x004592E0`): the request of the current frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TooltipWindow {
    /// Request (`+0x248`, `+0x260`, value); `None` = kind 0, nothing to draw.
    pub request: Option<TooltipRequest>,
}

impl TooltipWindow {
    /// Empty window.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frame start: the request of the previous frame expires (O-44; in the original `0x00459260` sets the kind to 0
    /// after drawing).
    pub fn begin_frame(&mut self) {
        self.request = None;
    }

    /// Event `0xC` (`0x00458A80`): clears first and then takes over everything, the last one in the frame wins (O-264).
    pub fn set(&mut self, request: TooltipRequest) {
        self.request = Some(request);
    }

    /// Event `0xD` (`0x004589F0`): kind 0.
    pub fn clear(&mut self) {
        self.request = None;
    }

    /// Content of the current request ([`tooltip_content`]).
    pub fn content(&self, game: &dyn GameText) -> Option<TooltipContent> {
        tooltip_content(self.request.as_ref()?, game)
    }

    /// Measuring (`0x00459030`): the runs of all sections in one layout. `layout.width` is the largest x end
    /// coordinate reached (a long word after a wrap extends past 300, O-264), `layout.height` the bottom edge of the
    /// last run.
    pub fn layout(
        &self,
        fonts: &FontBook,
        game: &dyn GameText,
        key_names: &TextContext,
        enhanced: bool,
    ) -> Option<TextLayout> {
        let content = self.content(game)?;
        let font = fonts.by_name(TEXT_FONT);
        let mut layout = TextLayout::default();
        for section in sections(&content, game, enhanced) {
            let tags = GameTags::new(game, 0).with_params(section.params);
            let params = LayoutParams {
                flags: section.flags,
                width: TOOLTIP_WIDTH,
                font,
                color: section.color,
            };
            layout_text_game(
                &section.text,
                &params,
                fonts,
                &mut layout,
                Some(key_names),
                Some(&tags),
            );
        }
        Some(layout)
    }

    /// Drawing (`0x00459260`, purely reading): nothing without a request. Otherwise the box
    /// `(0, H - 200 - h, w, H - 200)` at the left screen edge without padding (O-242, O-264; the original never reads
    /// `x`/`bottom` of the event) with a semi-transparent area in `0x80000080` and an opaque outline (`0x00458DA0`),
    /// the text on top without shadow. Return value: the rectangle of the area; the outline extends 1 px beyond it to
    /// the right and bottom.
    ///
    /// The original draws the outline as a line list over the four corners of the area (`alpharect2`, indices 0-1,
    /// 1-3, 3-2, 2-0, `Renderer_DrawIndexedUpLineListScreen`). Which pixels the lines hit is decided by the
    /// rasterization of the graphics driver; measured in the image (under dgVoodoo2) the rows `y = top` and
    /// `y = H - 200` as well as the columns `x = 0` and `x = w` are opaque `(0, 0, 128)`, only the corner
    /// `(w, H - 200)` is missing; with `w = 180` and `h = 48` the box with outline measures 181 x 49. The
    /// reimplementation lays out exactly these pixels as four 1-px strips ([`outline`]).
    pub fn draw(
        &self,
        ctx: &mut UiContext,
        game: &dyn GameText,
        key_names: &TextContext,
        enhanced: bool,
        out: &mut Vec<DrawCmd>,
    ) -> Option<[i32; 4]> {
        let layout = self.layout(ctx.fonts, game, key_names, enhanced)?;
        let bottom = ctx.screen.height.wrapping_sub(BOX_BOTTOM_GAP);
        let top = bottom.wrapping_sub(layout.height);
        let rect = [0, top, layout.width, bottom];
        fill_rect(rect, BOX_COLOR, out);
        for edge in outline(rect) {
            if edge[2] > edge[0] && edge[3] > edge[1] {
                fill_rect(edge, OUTLINE_COLOR, out);
            }
        }
        draw_runs(&layout, [0, top], false, None, ctx.fonts, out);
        Some(rect)
    }

    /// Readable form for the control channel: one line per section, game and additional tags as well as `[KEYID]`
    /// substituted ([`resolve_text`]), color and icon tags stay as text.
    pub fn text(
        &self,
        game: &dyn GameText,
        key_names: &TextContext,
        enhanced: bool,
    ) -> Option<String> {
        let content = self.content(game)?;
        let lines: Vec<String> = sections(&content, game, enhanced)
            .into_iter()
            .map(|section| {
                let tags = GameTags::new(game, 0).with_params(section.params);
                resolve_text(&section.text, section.flags, Some(key_names), &tags)
            })
            .collect();
        Some(lines.join("\n"))
    }
}

/// The four lines of the list `alpharect2` as strips in the sequence of the indices (top, right, bottom, left), with
/// the pixels measured on the original: each row or column from `l`/`t` up to before `r`/`b`, the right column at
/// `x = r` and the bottom row at `y = b` - thus **outside** the area; the corner `(r, b)` stays free as in the image.
/// The selection box ([`crate::hud::selection_rect`]) draws its outline with the same line call.
pub(crate) fn outline([l, t, r, b]: [i32; 4]) -> [[i32; 4]; 4] {
    [
        [l, t, r, t + 1],
        [r, t, r + 1, b],
        [l, b, r, b + 1],
        [l, t, l + 1, b],
    ]
}

#[cfg(test)]
mod tests;
