//! Text layout: measuring and drawing of strings, formatted text with tags, wrapping and centering.
//!
//! Basis: re/ui-rendering 3.4 (drawing a texture font), 3.8.2 (splitting), 3.8.3 (laying out, wrapping, line
//! height), 3.8.4 (centering), 3.8.5 (drawing the runs); quirks O-60 ... O-63, O-77 ... O-82 and O-86.

use std::collections::BTreeMap;

use crate::bkg_paint::tile_quad;
use crate::draw::{Blend, DrawCmd, Filter, TextureId};
use crate::fonts::{FontBook, FontRef};
use crate::geom::intersect;

/// Layout switches (`param_4[0]` of `FUN_00560FF0`, re/ui-rendering 3.8.3).
pub mod layout_flags {
    /// Start font from the parameters (`param_4[4]`).
    pub const START_FONT: u32 = 0x02;
    /// Start color from the parameters (`param_4[5]`).
    pub const START_COLOR: u32 = 0x04;
    /// Without effect in the layout (`FUN_00560FF0` does not check the bit, re/ui-rendering 3.8.3); set only by the
    /// message bar (switch `0x56`, `MessageBar_ShowMessage` `0x00494A60`, re/game-ui N.7.1).
    pub const UNUSED_10: u32 = 0x10;
    /// Enable `[PROGRESS]`.
    pub const PROGRESS: u32 = 0x20;
    /// Wrapping or shortening to the width; prerequisite for the tags `[CENTER]`, `[CENTERXPOS]` and the switch
    /// [`CENTER`].
    pub const WIDTH: u32 = 0x40;
    /// No word wrap: every overhanging word is shortened.
    pub const TRUNCATE: u32 = 0x80;
    /// Center lines (like `[CENTER]`).
    pub const CENTER: u32 = 0x100;
    /// Enable `[KEYID]`.
    pub const KEYID: u32 = 0x200;
}

/// Parameters of a layout call (`param_3`, `param_4` of `FUN_00560FF0`).
///
/// O-82: Without the switches `0x02`/`0x04` the start font and color stayed uninitialized in the original; all
/// callers seen set both (`0x46`). The reimplementation always requires font and color and takes them independently
/// of the switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutParams {
    pub flags: u32,
    /// Width `W` in screen pixels (`*param_3`).
    pub width: i32,
    pub font: FontRef,
    /// Start color ARGB; also the value that `[DEFAULTCOLOR]` checks (O-82).
    pub color: u32,
}

/// Menu state for placeholders of the text layout: the key layout for `[KEYID n]` (only with
/// [`layout_flags::KEYID`], re/ui-rendering 3.8.2). On every layout the original queries the `m_KeyMap` of the
/// keyboard object (`FUN_0049D4E0`), reads the text as a combination (`FUN_0049C2B0`) and assembles the display names
/// `50000 + VK` (`FUN_0049BDF0`); the reimplementation computes this in advance (`screens::text_context`). Also the
/// details of the chosen map for `[CURLEVEL…]` ([`CurLevelText`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextContext {
    /// Action ID -> display name of the binding (e.g. `ALT N`).
    pub key_names: BTreeMap<i32, String>,
    /// Display name for an action without an entry: `FUN_0049D4E0` then returns "not defined", which is likewise
    /// read as a combination (two unknown words -> code 0 twice, O-113).
    pub undefined: String,
    /// Texts for `[CURLEVELNAME]`, `[CURLEVELSIZE]`, `[CURLEVELPLAYERS]`, `[CURLEVELINFOS]`; `None` = no menu state
    /// (static drawing: nothing inserted). `MenuApp` always sets one; without a chosen map, name, size and players
    /// are empty (the pointers in `GameApp` are 0, `FUN_0055FA90` inserts nothing).
    pub cur_level: Option<CurLevelText>,
}

/// Details of the chosen level for the placeholders `[CURLEVEL…]` (`FUN_00560FF0` `0x00561FD8`...`0x0056212F`):
/// `name` = `GameApp+0xD734` (`FUN_004B9900`), `size` = `+0xD738` (text `15450 + size class`, `FUN_004B9910`),
/// `players` = `+0xD73C` (text `15458 + player count`, `FUN_004B9920`), `infos` = `FUN_004B9C90(1)` (`LEVELINFO`,
/// from the session, `draft::level_infos`). An empty text inserts nothing (like a null pointer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurLevelText {
    pub name: String,
    pub size: String,
    pub players: String,
    pub infos: String,
}

/// Which of the four trade price tags `[TRADE…COST]` (`FUN_00560FF0` `0x00561942`...`0x00561AD6`). The name follows
/// the tag: `WoodGold` = `Market_ConvertWoodGold(+100)` (wood price), `FoodGold` = `Market_ConvertFoodGold(+100)`,
/// `GoldWood` = `Market_ConvertWoodGold(-100)`, `GoldFood` = `Market_ConvertFoodGold(-100)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeCost {
    WoodGold,
    FoodGold,
    GoldWood,
    GoldFood,
}

/// Game values for the game tags of the text layout (re/game-ui N.9): the original reads them through the game object
/// (`FUN_004B9140` != 0 = running match); the text layout stays free of simulation in `nml-ui` and asks through this
/// trait. All answers are pure display values (no state change).
pub trait GameText {
    /// Name of slot `slot` (`GameEngine_GetPlayer` `0x0042D560` -> `Player+0xC`); empty outside 0...7.
    fn player_name(&self, slot: i32) -> String;
    /// `Market_ConvertWoodGold`/`...FoodGold` with the market price, +/-100.
    fn trade_cost(&self, tag: TradeCost) -> i32;
    /// `Player_GetGuidCount` (`0x0047DD90`): object counter of `guid` at slot `slot`.
    fn guid_count(&self, slot: i32, guid: i32) -> i32;
    /// `FUN_0047E7F0`: kill counter of `guid` at slot `slot`.
    fn kill_count(&self, slot: i32, guid: i32) -> i32;
    /// `FUN_00432B60(n)` (`TASKSTATUS`): `None` = task unknown (return value 0), `Some(true)` = value != 0 (1),
    /// `Some(false)` = value 0 (2).
    fn task_status(&self, n: i32) -> Option<bool>;
    /// `FUN_00431580(n)`: remaining time of the timer `n` in whole seconds.
    fn countdown_seconds(&self, n: i32) -> i32;

    // Values of the additional tags of the tooltip (re/game-ui N.8) and texts. The defaults answer "nothing present"
    // so that implementers without these values (message bar, tests) stay unchanged; the additional tags then stay
    // empty.

    /// Text table (`Lookup` in `FUN_0055FFE0`/`FUN_0055FFC0`): `Some(text)` with an entry (even an empty one), `None`
    /// without an entry. The caller applies the replacement rule "missing text -> its number" (O-4, O-241) where the
    /// original calls `FUN_0055FFE0`; there it formats the number in the same call, so the number of the text itself
    /// appears. Default: no text.
    fn text(&self, _id: i32) -> Option<String> {
        None
    }

    /// Slot of the local player (`GameEngine_GetLocalPlayer` -> `Player_GetSlotIndex`); default -1 (no player).
    fn local_slot(&self) -> i32 {
        -1
    }

    /// Type GUID of the object with the HID `hid` (`GameWorld_GetObjectById(hid, 0)` -> `+0x54`); `None` if it is
    /// missing.
    fn object_type(&self, _hid: i32) -> Option<i32> {
        None
    }

    /// Stock of slot `slot` for `[RESOURCES]` (`0x0043DEB0`): gold (R0), wood (R1), food (R2) and the **free** living
    /// space `R4 − R3`. `None` if the player does not exist (`GameEngine_GetPlayer` = 0; the tag then inserts
    /// nothing).
    fn available(&self, _slot: i32) -> Option<[i32; 4]> {
        None
    }

    /// Costs of the type `guid` from the value set of slot `slot` (`GameWorld_GetTypeValueSet`, `ValueSet_GetInt`):
    /// gold (value 12), wood (11), food (10), living space (22), mana (15). `None` without a value set (the tag then
    /// shows only `[DEFAULTCOLOR]`).
    fn costs(&self, _slot: i32, _guid: i32) -> Option<[i32; 5]> {
        None
    }

    /// Mana (attribute 15, `AttributeSet_GetCurrentValue`) of the **first** selected object (`Selection_GetFirst`);
    /// `None` without a selection (no mana row, O-243).
    fn first_selected_mana(&self) -> Option<i32> {
        None
    }

    /// `REQUIRED_GUID` of the button with the `BUTTONMSG` `guid` (`GameWorld_LookupToolbarEntry`); `None` if there is
    /// no entry. Non-positive slots do not count.
    fn requirements(&self, _guid: i32) -> Option<[i32; 5]> {
        None
    }

    /// Finished insertion text of `[HEADHUNTERREWARD]` for the button GUID `guid` (re/headhunter-robbing 2.5): in the
    /// order of the original price (`0x00438850`), target (`0x0042F7B0`), contract giver (`0x0042F8B0`) - three
    /// insertions at the same point, the last stands in front. Default: empty.
    fn headhunter_reward(&self, _guid: i32) -> String {
        String::new()
    }

    /// Chosen formation (`FUN_00477950`); outside 0...5 `[FORMATION]` inserts nothing. Default -1.
    fn formation(&self) -> i32 {
        -1
    }
}

/// Text `id` like `FUN_0055FFE0`: the entry, otherwise the own number as `"%d"` (O-241).
pub fn text_or_number(values: &dyn GameText, id: i32) -> String {
    values.text(id).unwrap_or_else(|| id.to_string())
}

/// Parameter block of a tooltip layout (`param_4[1]`, `[2]`, `[3]` of `FUN_00560FF0`, filled by `0x00459030`):
/// GUID, local slot and value. In the original the value is an integer word: with `[RESOURCES]` the factor of the
/// costs, with `[PROGRESS]` the percentage (`⌊f · 100⌋`, already computed in `0x00458A80`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagParams {
    pub guid: i32,
    pub slot: i32,
    pub value: i32,
}

/// Game values of a layout call: the source of the numbers and the parameter block `param_4[0xB]` (flag word with
/// the player bytes: byte 0 = PLID1, byte 1 = PLID2). Without [`GameTags`] the game tags disappear (no game context).
///
/// `params` is the parameter block of the additional tags (`[RESOURCES]`, `[REQUIREMENTS]`, `[HEADHUNTERREWARD]`,
/// `[FORMATION]`, `[PROGRESS]`); only the tooltip window sets it. Without it these tags stay empty (message bar,
/// menus; `DIALOG_DRAW_GOLDEN` unaffected).
#[derive(Clone, Copy)]
pub struct GameTags<'a> {
    pub values: &'a dyn GameText,
    pub players: u16,
    pub params: Option<TagParams>,
}

impl<'a> GameTags<'a> {
    /// Game values without the parameter block of the additional tags.
    pub fn new(values: &'a dyn GameText, players: u16) -> Self {
        Self {
            values,
            players,
            params: None,
        }
    }

    /// The same game values with the parameter block of the tooltip layout.
    pub fn with_params(self, params: TagParams) -> Self {
        Self {
            params: Some(params),
            ..self
        }
    }
}

/// Maximum number of inserted texts (`[KEYID]`, `[CURLEVEL…]`, game tags) per layout call (O-252). The original
/// (`FUN_0055FA90`) splits inserted text again without a limit: a name like `[PLAYERNAME 0]` in slot 0 would grow
/// endlessly. The reimplementation does not output further insertions after this number (the tags then act as
/// without a value); normal texts have only a few insertions.
pub const MAX_INSERTIONS: usize = 256;

/// Color of `[TASKSTATUS n]` at return value 0 (task unknown).
const TASK_UNKNOWN_COLOR: u32 = 0xFF96_9696;
/// ... at return value 1 (value != 0).
const TASK_DONE_COLOR: u32 = 0xFF00_FF00;
/// ... at return value 2 (value 0).
const TASK_OPEN_COLOR: u32 = 0xFFFF_0000;

/// Content of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunContent {
    /// Atlas codes (low byte per UTF-16 unit, O-61).
    Text(Vec<u8>),
    /// Icon: image of the set, cell size of the set, index of the cell.
    Icon {
        set: TextureId,
        size: [i32; 2],
        index: i32,
    },
}

/// A laid-out run (`0x24` bytes in the original, re/ui-rendering 3.8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Position relative to the origin of the layout (`+0x10`/`+0x14`).
    pub x: i32,
    pub y: i32,
    /// Measured size (`+0x08`/`+0x0C`).
    pub w: i32,
    pub h: i32,
    pub font: FontRef,
    pub color: u32,
    pub content: RunContent,
}

/// A layout (`this+0x260` of the global text object): runs, greatest x end coordinate reached, total height.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextLayout {
    pub runs: Vec<Run>,
    /// `+0x1C`: greatest x end coordinate reached.
    pub width: i32,
    /// `+0x20`: bottom edge of the last laid-out run (start of the next call).
    pub height: i32,
}

/// Color table for `[SETPLAYERCOLOR n]` (`0x006321B4`, re/ui-rendering 3.8.2).
pub(crate) const PLAYER_COLORS: [u32; 9] = [
    0xFFFF_CE0D,
    0xFF00_73E6,
    0xFF17_A710,
    0xFFC4_1800,
    0xFF82_01D8,
    0xFFB9_0066,
    0xFFFF_9F14,
    0xFF00_A8A8,
    0xFFAC_ACAC,
];

/// Effect of a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Font,
    Cr,
    CenterXPos,
    Center,
    Color,
    DefaultColor,
    Icon,
    Progress,
    SetPlayerColor,
    KeyId,
    /// `[RESOURCES]` (`0x0043DEB0`, parameter block).
    Resources,
    /// `[REQUIREMENTS]` (`0x00430790`, parameter block).
    Requirements,
    /// `[HEADHUNTERREWARD]` (`0x00438850`, `0x0042F7B0`, `0x0042F8B0`, parameter block).
    HeadhunterReward,
    /// `[FORMATION]` (`0x0042DCA0`).
    Formation,
    /// `[PLAYERNAME n]` (`%d`).
    PlayerName,
    /// `[PLAYERNAMEPLID1]`/`[PLAYERNAMEPLID2]`: player byte 0 or 1 of the parameter block.
    PlayerNamePlid(u8),
    /// `[TRADE…COST]`.
    Trade(TradeCost),
    /// `[BODYCOUNTER a b]`.
    BodyCounter,
    /// `[GUIDCOUNTER a b]`.
    GuidCounter,
    /// `[TASKSTATUS n]`: sets the color.
    TaskStatus,
    /// `[COUNTDOWN n]`.
    Countdown,
    /// Details of the chosen level from `GameApp` (`FUN_004B9900/9910/9920/9C90`, without a check for game context
    /// and without a layout switch): text from [`TextContext::cur_level`], nothing without menu state.
    CurLevel(CurLevelField),
}

/// Which of the four `[CURLEVEL…]` placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurLevelField {
    Name,
    Size,
    Players,
    Infos,
}

/// Keys in the comparison order of `FUN_00560FF0` (`0x00561210` and following; strings `0x006324AC` ...
/// `0x006322F0` descending). Comparison as a prefix, case-sensitive (`CString::Left(n)` + `__mbscmp`, O-77);
/// `[CENTERXPOS` comes before `[CENTER`.
const TAGS: [(&[u8], TagKind); 29] = [
    (b"[FONT ", TagKind::Font),
    (b"[CR", TagKind::Cr),
    (b"[CENTERXPOS", TagKind::CenterXPos),
    (b"[CENTER", TagKind::Center),
    (b"[COLOR ", TagKind::Color),
    (b"[DEFAULTCOLOR", TagKind::DefaultColor),
    (b"[RESOURCES", TagKind::Resources),
    (b"[REQUIREMENTS", TagKind::Requirements),
    (b"[ICON ", TagKind::Icon),
    (b"[HEADHUNTERREWARD", TagKind::HeadhunterReward),
    (b"[PROGRESS", TagKind::Progress),
    (b"[FORMATION", TagKind::Formation),
    (b"[TRADEWOODGOLDCOST", TagKind::Trade(TradeCost::WoodGold)),
    (b"[TRADEFOODGOLDCOST", TagKind::Trade(TradeCost::FoodGold)),
    (b"[TRADEGOLDWOODCOST", TagKind::Trade(TradeCost::GoldWood)),
    (b"[TRADEGOLDFOODCOST", TagKind::Trade(TradeCost::GoldFood)),
    (b"[SETPLAYERCOLOR ", TagKind::SetPlayerColor),
    (b"[PLAYERNAME ", TagKind::PlayerName),
    (b"[BODYCOUNTER ", TagKind::BodyCounter),
    (b"[GUIDCOUNTER ", TagKind::GuidCounter),
    (b"[TASKSTATUS ", TagKind::TaskStatus),
    (b"[COUNTDOWN ", TagKind::Countdown),
    (b"[CURLEVELNAME", TagKind::CurLevel(CurLevelField::Name)),
    (b"[CURLEVELSIZE", TagKind::CurLevel(CurLevelField::Size)),
    (
        b"[CURLEVELPLAYERS",
        TagKind::CurLevel(CurLevelField::Players),
    ),
    (b"[CURLEVELINFOS", TagKind::CurLevel(CurLevelField::Infos)),
    (b"[KEYID ", TagKind::KeyId),
    (b"[PLAYERNAMEPLID1", TagKind::PlayerNamePlid(0)),
    (b"[PLAYERNAMEPLID2", TagKind::PlayerNamePlid(1)),
];

const SPACE: u16 = 0x20;
const OPEN: u16 = 0x5B; // `[`
const CLOSE: u16 = 0x5D; // `]`
const SEMICOLON: u16 = 0x3B;

/// Atlas codes of a text: per UTF-16 unit the low byte (`(c & 0xFF)`, `0x0057A2C0`; O-61), e.g.
/// `…` (U+2026) -> `0x26`, `ä` -> `0xE4`, `Ā` (U+0100) -> `0x00` (glyph 0). Characters outside the BMP yield two
/// units (surrogate pair) as in the original, hence two codes. The text ends like a C string at the first null
/// character (`wcslen`); in the result `0` therefore always means glyph 0, the end of the slice replaces the null of
/// the original.
pub fn atlas_codes(text: &str) -> Vec<u8> {
    utf16_units(text).into_iter().map(low_byte).collect()
}

/// UTF-16 units up to before the first null character (`wcslen`, `FUN_005D2D29`).
fn utf16_units(text: &str) -> Vec<u16> {
    text.encode_utf16().take_while(|&u| u != 0).collect()
}

fn low_byte(unit: u16) -> u8 {
    (unit & 0xFF) as u8
}

/// Sum of the advances up to `\n` (`FUN_00579D60`: `ftol(width[c & 0xFF] * 1.0)`, scale 1.0 in all callers). 32-bit
/// addition like the original (wrapping instead of a panic).
fn advance_sum(codes: impl Iterator<Item = u8>, advance: &[i32; 256]) -> i32 {
    codes
        .take_while(|&c| c != b'\n')
        .fold(0i32, |sum, c| sum.wrapping_add(advance[usize::from(c)]))
}

/// Size of a text (`FUN_00428450` -> `FUN_00579D60`): width = sum of the advances up to `\n` or to the end of the
/// slice (where the null stood in the original, see [`atlas_codes`]), height = `max_height` of the font, independent
/// of the text (also for `""`).
pub fn measure(text: &[u8], font: FontRef, fonts: &FontBook) -> [i32; 2] {
    let font = fonts.font(font);
    [
        advance_sum(text.iter().copied(), &font.advance),
        font.max_height,
    ]
}

/// Height of a short sample text in screen pixels (`FUN_00428450(…, L"X")` among others): the elements measure their
/// row height with it - ListBox `L"Dy'"` (`AUTOYSIZE`) and `L"X"` (rows, hover), Label `L"D"` (scrollbar) and `L"O"`
/// (scroll step; re/menu-system 12.6). By [`measure`] this is always `max_height` of the font.
pub fn text_height(text: &str, font: FontRef, fonts: &FontBook) -> i32 {
    measure(&atlas_codes(text), font, fonts)[1]
}

/// Draws a string without wrapping and without shadow (`FUN_00428390` -> state block `Font` +
/// `FUN_0057A2C0`; re/ui-rendering 3.4, 3.8.5).
///
/// Per code one quad `FUN_00579E60` (O-60): the **whole cell** `cell` (texture size `>> 4`) from the pen position,
/// all four UVs from the `.fnt` (`u0, v0` top left ... `u1, v1` bottom right; in [`DrawCmd`] as
/// `[u0, v0, u1, v1]`), diffuse = color, POINT filter, no pixel offset; then `x += advance`. `\n` jumps to `pos.x`
/// and one `max_height` lower. No kerning; characters may overlap.
pub fn draw_string(
    text: &[u8],
    pos: [i32; 2],
    color: u32,
    font: FontRef,
    fonts: &FontBook,
    out: &mut Vec<DrawCmd>,
) {
    let font = fonts.font(font);
    let [cell_w, cell_h] = font.cell;
    let [mut x, mut y] = pos;
    for &c in text {
        if c == b'\n' {
            x = pos[0];
            y = y.wrapping_add(font.max_height);
            continue;
        }
        let [u0, u1, v0, v1] = font.uv[usize::from(c)];
        out.push(DrawCmd::Quad {
            texture: Some(font.atlas),
            dst: [x, y, x.wrapping_add(cell_w), y.wrapping_add(cell_h)],
            uv: [u0, v0, u1, v1],
            color,
            filter: Filter::Point,
            blend: Blend::Alpha,
        });
        x = x.wrapping_add(font.advance[usize::from(c)]);
    }
}

/// Content of a run under construction: text as UTF-16 units like in the original, so that the space at the wrap
/// (`FUN_0055F860`) is checked on the whole unit (U+0120 is not a space).
enum Pending {
    Text(Vec<u16>),
    Icon {
        set: TextureId,
        size: [i32; 2],
        index: i32,
    },
}

/// A run under construction.
struct PendingRun {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: FontRef,
    color: u32,
    content: Pending,
}

impl PendingRun {
    /// `FUN_0055F7A0`: no text (icon) or empty text.
    fn text_is_empty(&self) -> bool {
        match &self.content {
            Pending::Text(units) => units.is_empty(),
            Pending::Icon { .. } => true,
        }
    }

    /// Remeasure the text (`FUN_00428450`).
    fn remeasure(&mut self, fonts: &FontBook) {
        if let Pending::Text(units) = &self.content {
            let font = fonts.font(self.font);
            self.w = advance_sum(units.iter().map(|&u| low_byte(u)), &font.advance);
            self.h = font.max_height;
        }
    }

    /// `FUN_0055F860`: if the text ends with U+0020, it is removed and remeasured.
    fn strip_trailing_space(&mut self, fonts: &FontBook) {
        if let Pending::Text(units) = &mut self.content
            && units.last() == Some(&SPACE)
        {
            units.pop();
            self.remeasure(fonts);
        }
    }

    /// `FUN_0055F7C0`: drop the last character, remeasure.
    fn pop_char(&mut self, fonts: &FontBook) {
        if let Pending::Text(units) = &mut self.content {
            units.pop();
        }
        self.remeasure(fonts);
    }

    fn finish(self) -> Run {
        Run {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
            font: self.font,
            color: self.color,
            content: match self.content {
                Pending::Text(units) => RunContent::Text(units.into_iter().map(low_byte).collect()),
                Pending::Icon { set, size, index } => RunContent::Icon { set, size, index },
            },
        }
    }
}

/// A piece of the splitting (`FUN_0055F9B0`).
enum Piece {
    /// Return value 1: word (with its space) or a `[` without `]` up to the end of the text.
    Word(Vec<u16>),
    /// Return value 2: tag including brackets.
    Tag(Vec<u16>),
}

/// Splitting (`FUN_0055F9B0`, re/ui-rendering 3.8.2) from `*i` (`units[*i]` exists). A word extends up to and
/// including the first U+0020, up to before `[` or to the end. A tag extends from `[` up to and including `]`; a `;`
/// before that becomes `]` in the piece and `[` in the text, and `*i` points to it (the next piece is again a tag).
/// If `]` is missing up to the end, the whole rest from `[` is a word.
fn next_piece(units: &mut [u16], i: &mut usize) -> Piece {
    let mut piece = Vec::new();
    if units[*i] != OPEN {
        while let Some(&c) = units.get(*i) {
            if c == OPEN {
                break;
            }
            piece.push(c);
            *i += 1;
            if c == SPACE {
                break;
            }
        }
        return Piece::Word(piece);
    }
    piece.push(OPEN);
    *i += 1;
    loop {
        let Some(&c) = units.get(*i) else {
            return Piece::Word(piece);
        };
        piece.push(c);
        *i += 1;
        if c == CLOSE {
            return Piece::Tag(piece);
        }
        if c == SEMICOLON {
            *piece.last_mut().expect("piece is not empty") = CLOSE;
            *i -= 1;
            units[*i] = OPEN;
            return Piece::Tag(piece);
        }
    }
}

/// C-string view: up to before the first null (the original converts the piece to 8 bit and passes on a `char*`; a
/// character with low byte 0 ends the specification there).
fn c_str(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// `isspace` of the C runtime for `sscanf`.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn skip_space(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&b| !is_c_space(b))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

/// Sign for `%d`/`%X`.
fn sign(bytes: &[u8]) -> (bool, &[u8]) {
    match bytes {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, bytes),
    }
}

/// `sscanf(…, "%d")`: whitespace, sign, at least one digit; 32 bits wrapping. `None` on failure.
fn scan_int(bytes: &[u8]) -> Option<(i32, &[u8])> {
    let (negative, rest) = sign(skip_space(c_str(bytes)));
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let value = rest[..digits].iter().fold(0i32, |v, &d| {
        v.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
    });
    Some((
        if negative {
            value.wrapping_neg()
        } else {
            value
        },
        &rest[digits..],
    ))
}

/// `sscanf(…, "%X")`: whitespace, sign, optional `0x`/`0X`, at least one hex digit; 32 bits wrapping.
fn scan_hex(bytes: &[u8]) -> Option<u32> {
    let (negative, mut rest) = sign(skip_space(c_str(bytes)));
    if let [b'0', b'x' | b'X', next, ..] = rest
        && next.is_ascii_hexdigit()
    {
        rest = &rest[2..];
    }
    let digits = rest.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    if digits == 0 {
        return None;
    }
    let value = rest[..digits].iter().fold(0u32, |v, &d| {
        let digit = char::from(d).to_digit(16).expect("hex digit");
        v.wrapping_mul(16).wrapping_add(digit)
    });
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

/// `sscanf(…, "%s")`: skip whitespace, then up to the next whitespace.
fn scan_word(bytes: &[u8]) -> (&[u8], &[u8]) {
    let rest = skip_space(c_str(bytes));
    let len = rest.iter().take_while(|&&b| !is_c_space(b)).count();
    (&rest[..len], &rest[len..])
}

/// Byte name (low byte per character) as key of the font and icon tables: their names come from the cp1252 file
/// `localisation.*`, the original compares the bytes - cp1252 maps byte and character onto each other reversibly.
fn table_name(bytes: &[u8]) -> String {
    nml_formats::text::decode_cp1252(c_str(bytes))
}

/// `Mid(n)` without the last character (`]`), with an empty rest for a piece that is too short.
fn tag_argument(bytes: &[u8], key_len: usize) -> &[u8] {
    bytes
        .get(key_len..bytes.len().saturating_sub(1))
        .unwrap_or(&[])
}

/// Kind of a tag (piece including brackets as bytes) per [`TAGS`]; `None` = unknown.
fn tag_kind(bytes: &[u8]) -> Option<TagKind> {
    TAGS.iter()
        .find(|(key, _)| bytes.starts_with(key))
        .map(|&(_, kind)| kind)
}

/// Text that an inserting game tag places behind itself (re/game-ui N.8, N.9, `FUN_0055FA90`); `None` if `kind` is
/// not one, the number in the tag is missing (a failing `%d` would read a stack value in the original, the
/// reimplementation inserts nothing) or an additional tag has no parameter block (outside the tooltip: nothing).
/// `flags` are the layout switches (`[PROGRESS]` only with [`layout_flags::PROGRESS`]).
fn game_tag_text(kind: TagKind, bytes: &[u8], game: &GameTags<'_>, flags: u32) -> Option<String> {
    match kind {
        // `0x00561B7E`: `sscanf(from 12, "%d")`, `GameEngine_GetPlayer(n)` (only 0 <= n < 8) -> `Player+0xC`.
        TagKind::PlayerName => {
            let (n, _) = scan_int(tag_argument(bytes, 12))?;
            Some(game.values.player_name(n))
        }
        // `GameEngine_GetPlayer(player byte)` of the parameter block `param_4[0xB]`.
        TagKind::PlayerNamePlid(byte) => {
            let slot = i32::from((game.players >> (8 * u16::from(byte))) as u8);
            Some(game.values.player_name(slot))
        }
        TagKind::Trade(cost) => Some(game.values.trade_cost(cost).to_string()),
        // `sscanf(…, "%d %d")` (slot, GUID).
        TagKind::BodyCounter | TagKind::GuidCounter => {
            let (slot, rest) = scan_int(tag_argument(bytes, 13))?;
            let (guid, _) = scan_int(rest)?;
            let n = if kind == TagKind::BodyCounter {
                game.values.kill_count(slot, guid)
            } else {
                game.values.guid_count(slot, guid)
            };
            Some(n.to_string())
        }
        TagKind::Countdown => {
            let (n, _) = scan_int(tag_argument(bytes, 11))?;
            Some(game.values.countdown_seconds(n).to_string())
        }
        // `0x005615E0`: `0x0043DEB0(param_4[1], param_4[2], param_4[3])`.
        TagKind::Resources => resources_text(game.values, &game.params?),
        // `0x0056164F`: `0x00430790(param_4[1], param_4[2])`.
        TagKind::Requirements => requirements_text(game.values, &game.params?),
        // `0x005617B8`: three texts for the button GUID `param_4[1]`.
        TagKind::HeadhunterReward => Some(game.values.headhunter_reward(game.params?.guid)),
        // `0x005618E3`: `0x0042DCA0()` without a parameter; the reimplementation requires the parameter block as a
        // feature of the tooltip layout, with no difference in the shipped data.
        TagKind::Formation => {
            game.params?;
            Some(formation_text(game.values))
        }
        // `0x0056186A`: with switch `0x20` `swprintf(L"%d%%", param_4[3])` (`0x0063242C`). The original does not check
        // a game context here; the reimplementation carries the value in the parameter block of the game values.
        TagKind::Progress => {
            if flags & layout_flags::PROGRESS == 0 {
                return None;
            }
            Some(format!("{}%", game.params?.value))
        }
        _ => None,
    }
}

/// Red on shortage (`0x0043DEB0`; color value with `0x`, `%X` reads it).
const LACK_COLOR_TAG: &str = "[COLOR 0xFFFF0000]";
/// Base color.
const DEFAULT_COLOR_TAG: &str = "[DEFAULTCOLOR]";

/// `[RESOURCES]` (`0x0043DEB0`, re/game-ui N.8, O-243): one line with color, icon and `"%d "` per cost type.
///
/// - Without a player at `slot`: nothing (return value `NULL`).
/// - Without a value set: only `[DEFAULTCOLOR]`.
/// - GUID < 1000 (ability): only with a selection the mana cost (even 0) against the mana of the first selected
///   object, red if mana < cost, icon `[ICON RESOURCENICONS 4]`; the costs are not multiplied by the factor.
/// - Otherwise gold, wood, food, living space in this order, only costs != 0; costs = value * factor (32 bit); red if
///   costs > stock (for living space the free space), icon `[ICON INFOICONS i]` with the resource index
///   (`FUN_004FA920` = identity).
/// - At the end always `[DEFAULTCOLOR]`.
///
/// The icons are tinted in the run color, so on shortage icon and number are both red. The set names come from
/// `localisation.*` (O-64: `INFOICONS` = `resourcen_icons16x16.tga`, `RESOURCENICONS` = `info_icons16x16.tga`); the
/// font book knows both under these names.
fn resources_text(values: &dyn GameText, p: &TagParams) -> Option<String> {
    let available = values.available(p.slot)?;
    let mut s = String::new();
    if let Some(costs) = values.costs(p.slot, p.guid) {
        if p.guid < 1000 {
            if let Some(mana) = values.first_selected_mana() {
                let cost = costs[4];
                s.push_str(if mana < cost {
                    LACK_COLOR_TAG
                } else {
                    DEFAULT_COLOR_TAG
                });
                s.push_str("[ICON RESOURCENICONS 4]");
                s.push_str(&format!("{cost} "));
            }
        } else {
            for (i, (&cost, &have)) in costs.iter().zip(&available).enumerate() {
                if cost == 0 {
                    continue;
                }
                let cost = cost.wrapping_mul(p.value);
                s.push_str(if cost > have {
                    LACK_COLOR_TAG
                } else {
                    DEFAULT_COLOR_TAG
                });
                s.push_str(&format!("[ICON INFOICONS {i}]{cost} "));
            }
        }
    }
    s.push_str(DEFAULT_COLOR_TAG);
    Some(s)
}

/// `[REQUIREMENTS]` (`0x00430790`, re/game-ui N.8): without a button entry or without a positive `REQUIRED_GUID`
/// nothing; otherwise text 300 (the heading of the requirement list) and per positive prerequisite text 1 (red) at
/// object counter 0, otherwise text 3 (gray), then text 7 (`[CR] - `) and the name; at the end text 2
/// (`[DEFAULTCOLOR]`). All texts via `FUN_0055FFE0` (missing -> number). The nation list
/// (`REQUIRED_GUID_NATION`) runs in the original through the stub `Player_MapNationGuid_Stub` (0) and never
/// appears; a button with only a nation list would show the heading alone in the original - unused in the shipped
/// data, the reimplementation does not read the nation list.
fn requirements_text(values: &dyn GameText, p: &TagParams) -> Option<String> {
    let required = values.requirements(p.guid)?;
    if !required.iter().any(|&g| g > 0) {
        return None;
    }
    let mut s = text_or_number(values, 300);
    for &guid in required.iter().filter(|&&g| g > 0) {
        let color = if values.guid_count(p.slot, guid) == 0 {
            1
        } else {
            3
        };
        s.push_str(&text_or_number(values, color));
        s.push_str(&text_or_number(values, 7));
        s.push_str(&text_or_number(values, guid));
    }
    s.push_str(&text_or_number(values, 2));
    Some(s)
}

/// `[FORMATION]` (`0x0042DCA0`): formation 0...5 -> text `511 + n` via `FUN_0055FFE0` (516 is missing and appears as
/// "516", O-241), otherwise empty (`0x00615874`).
fn formation_text(values: &dyn GameText) -> String {
    let n = values.formation();
    if (0..6).contains(&n) {
        text_or_number(values, 511 + n)
    } else {
        String::new()
    }
}

/// `[KEYID n]` with switch `0x200` and menu state: `atol(from 7)`, binding of the action (otherwise "not defined") as
/// `"(name)"` (branch `[KEYID ` in `FUN_00560FF0`).
fn keyid_text(bytes: &[u8], ctx: &TextContext) -> String {
    let id = scan_int(&bytes[7..]).map_or(0, |(id, _)| id);
    let name = ctx.key_names.get(&id).unwrap_or(&ctx.undefined);
    format!("({name})")
}

/// Is `kind` a tag that inserts text with game values (for [`resolve_text`] and the layout)?
fn inserts_game_text(kind: TagKind) -> bool {
    matches!(
        kind,
        TagKind::PlayerName
            | TagKind::PlayerNamePlid(_)
            | TagKind::Trade(_)
            | TagKind::BodyCounter
            | TagKind::GuidCounter
            | TagKind::Countdown
            | TagKind::Resources
            | TagKind::Requirements
            | TagKind::HeadhunterReward
            | TagKind::Formation
            | TagKind::Progress
    )
}

/// Display line without layout (viewer overlay): like [`resolve_text`] without layout switches and without menu state.
pub fn resolve_game_tags(text: &str, game: &GameTags<'_>) -> String {
    resolve_text(text, 0, None, game)
}

/// Readable form of a layout (viewer overlay, control channel `ui_state`): the inserting game tags (`[PLAYERNAME n]`,
/// `[PLAYERNAMEPLID1/2]`, `[TRADE…COST]`, `[GUIDCOUNTER]`, `[BODYCOUNTER]`, `[COUNTDOWN]`, with a parameter block the
/// additional tags `[RESOURCES]`, `[REQUIREMENTS]`, `[HEADHUNTERREWARD]`, `[FORMATION]` and with switch `0x20` also
/// `[PROGRESS]`) are replaced by their text and split again as in the layout (limit [`MAX_INSERTIONS`]); with switch
/// `0x200` and `ctx` likewise `[KEYID n]`. `[TASKSTATUS]` (color only) and game tags without a value are dropped; all
/// other tags stay as text (a `;` in the tag becomes `][` in the process). In the game, the drawing goes through
/// [`layout_text_game`]; this is only the readable form.
pub fn resolve_text(
    text: &str,
    flags: u32,
    ctx: Option<&TextContext>,
    game: &GameTags<'_>,
) -> String {
    let mut units = utf16_units(text);
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0;
    let mut insertions = 0usize;
    while i < units.len() {
        match next_piece(&mut units, &mut i) {
            Piece::Word(word) => out.extend(word),
            Piece::Tag(tag) => {
                let bytes: Vec<u8> = tag.iter().map(|&u| low_byte(u)).collect();
                let inserted = match (tag_kind(&bytes), ctx) {
                    (Some(kind), _) if inserts_game_text(kind) => {
                        game_tag_text(kind, &bytes, game, flags)
                    }
                    (Some(TagKind::KeyId), Some(ctx)) if flags & layout_flags::KEYID != 0 => {
                        Some(keyid_text(&bytes, ctx))
                    }
                    (Some(TagKind::TaskStatus), _) => None,
                    _ => {
                        out.extend(tag);
                        None
                    }
                };
                if let Some(inserted) = inserted
                    && insertions < MAX_INSERTIONS
                {
                    insertions += 1;
                    units.splice(i..i, utf16_units(&inserted));
                }
            }
        }
    }
    String::from_utf16_lossy(&out)
}

/// State of a layout call (`FUN_00560FF0`): `x` (`local_718`), line top edge `y_top` (`local_714`), `y_bottom`
/// (`local_71c`), font, color.
struct Setter<'a> {
    params: &'a LayoutParams,
    fonts: &'a FontBook,
    /// Menu state for `[KEYID]` and `[CURLEVEL…]`.
    ctx: Option<&'a TextContext>,
    /// Game values for the game tags (`FUN_004B9140` != 0); `None` = no game context, the tags vanish.
    game: Option<&'a GameTags<'a>>,
    out: &'a mut TextLayout,
    x: i32,
    y_top: i32,
    y_bottom: i32,
    font: FontRef,
    color: u32,
}

impl Setter<'_> {
    /// Text run at the pen position (`FUN_0055FC30`): measured, `y_bottom = y_top + h`.
    fn text_run(&mut self, units: Vec<u16>) -> PendingRun {
        let mut run = PendingRun {
            x: self.x,
            y: self.y_top,
            w: 0,
            h: 0,
            font: self.font,
            color: self.color,
            content: Pending::Text(units),
        };
        run.remeasure(self.fonts);
        self.y_bottom = run.y.wrapping_add(run.h);
        run
    }

    /// Icon run (`FUN_0055FCA0`): size = cell size of the set.
    fn icon_run(&mut self, set: TextureId, size: [i32; 2], index: i32) -> PendingRun {
        let run = PendingRun {
            x: self.x,
            y: self.y_top,
            w: size[0],
            h: size[1],
            font: self.font,
            color: self.color,
            content: Pending::Icon { set, size, index },
        };
        self.y_bottom = run.y.wrapping_add(run.h);
        run
    }

    /// Wrapping, shortening and appending of a run (`0x005624D9` and following, re/ui-rendering 3.8.3).
    fn place(&mut self, mut run: PendingRun) {
        let width = self.params.width;
        let flags = self.params.flags;
        let wraps = flags & layout_flags::WIDTH != 0;
        let x_end = self.x.wrapping_add(run.w);
        // `extent` is the value that goes into the greatest x end coordinate (`Layout+0x1C`).
        let extent;
        if x_end > width {
            if wraps && flags & layout_flags::TRUNCATE == 0 && self.x != 0 {
                // Wrap before this run (O-78): the advance uses the width with the space, the run loses its space;
                // overlong words then extend beyond `W` (O-80). Line height = height of this run (O-79).
                let full_width = run.w;
                run.x = 0;
                run.y = self.y_bottom;
                self.y_top = self.y_bottom;
                self.y_bottom = self.y_bottom.wrapping_add(run.h);
                run.strip_trailing_space(self.fonts);
                self.x = full_width;
                extent = full_width;
            } else if !wraps || run.text_is_empty() {
                // Without 0x40 no wrapping; icons and empty runs are never shortened.
                self.x = x_end;
                extent = x_end;
            } else {
                // Shortening (at the line start or with 0x80, O-80): character by character, the rest is dropped;
                // then the next line at unchanged x.
                let y_top = self.y_top;
                let mut end = x_end;
                while end > width && !run.text_is_empty() {
                    run.pop_char(self.fonts);
                    run.x = self.x;
                    run.y = y_top;
                    end = self.x.wrapping_add(run.w);
                    self.y_bottom = y_top.wrapping_add(run.h);
                }
                self.y_top = self.y_bottom;
                self.x = end;
                if run.text_is_empty() {
                    // Shortened to empty: discarded, y has advanced anyway.
                    return;
                }
                extent = end;
            }
        } else {
            self.x = x_end;
            extent = x_end;
        }
        self.out.runs.push(run.finish());
        self.out.width = self.out.width.max(extent);
        if self.y_bottom > 0 {
            self.out.height = self.y_bottom;
        }
    }

    /// Effect of a tag (`0x00561210` and following, re/ui-rendering 3.8.2); `center` is the centering flag. The piece
    /// is compared converted to 8 bit (low byte per character). Return value: text that the tag inserts behind itself
    /// (`FUN_0055FA90`).
    fn tag(&mut self, piece: Vec<u16>, center: &mut bool) -> Option<Vec<u16>> {
        let bytes: Vec<u8> = piece.iter().map(|&u| low_byte(u)).collect();
        let kind = tag_kind(&bytes);
        let flags = self.params.flags;
        match kind {
            Some(TagKind::Font) => {
                // `FUN_00560E70`: unknown name -> replacement font (original: "MS Sanserif" 10 px).
                self.font = self.fonts.by_name(&table_name(tag_argument(&bytes, 6)));
            }
            Some(TagKind::Cr) => {
                self.x = 0;
                self.y_top = self.y_bottom;
            }
            Some(TagKind::CenterXPos) => {
                if flags & layout_flags::WIDTH != 0 {
                    self.x = self.params.width / 2;
                }
            }
            Some(TagKind::Center) => *center = true,
            Some(TagKind::Color) => {
                if let Some(color) = scan_hex(tag_argument(&bytes, 7)) {
                    self.color = color;
                }
            }
            Some(TagKind::DefaultColor) => {
                // O-82: checks bit 2 of the color value instead of the switch 0x04.
                self.color = if self.params.color & 0x04 != 0 {
                    self.params.color
                } else {
                    0xFFFF_FFFF
                };
            }
            Some(TagKind::Icon) => self.icon(&bytes),
            Some(TagKind::SetPlayerColor) => {
                // Index < 9 (signed) -> table. A negative one would read before the table in the original (O-86,
                // deviation: the color stays), likewise a failing `%d` (original: stack value).
                if let Some((index, _)) = scan_int(&bytes[16..])
                    && let Ok(index) = usize::try_from(index)
                    && let Some(&color) = PLAYER_COLORS.get(index)
                {
                    self.color = color;
                }
            }
            // With 0x200 (branch `[KEYID ` in `FUN_00560FF0`): `atol(from 7)`, binding of the action (`FUN_0049D4E0`,
            // otherwise "not defined") as display name (`FUN_0049C2B0`, `FUN_0049BDF0`), `swprintf(L"(%s)")`. Without
            // menu state (static drawing) nothing.
            Some(TagKind::KeyId) => {
                if flags & layout_flags::KEYID != 0
                    && let Some(ctx) = self.ctx
                {
                    return Some(utf16_units(&keyid_text(&bytes, ctx)));
                }
            }
            // `0x00561FD8`...`0x0056212F`: without a switch check `FUN_0055FA90(&text, pointer, position, 0)` - inserts
            // the text at the point behind the tag into the working text (null pointer: nothing); the splitting
            // continues over the inserted text, so tags in it take effect (as with `[KEYID]`).
            Some(TagKind::CurLevel(field)) => {
                if let Some(cur) = self.ctx.and_then(|c| c.cur_level.as_ref()) {
                    let text = match field {
                        CurLevelField::Name => &cur.name,
                        CurLevelField::Size => &cur.size,
                        CurLevelField::Players => &cur.players,
                        CurLevelField::Infos => &cur.infos,
                    };
                    return Some(utf16_units(text));
                }
            }
            // Game tags (re/game-ui N.8, N.9): all only with game context (`local_734 = FUN_004B9140`), otherwise
            // nothing; the additional tags (`[RESOURCES]` & co., `[PROGRESS]`) additionally only with a parameter
            // block (tooltip). As with `[CURLEVEL…]` the text comes behind the tag via `FUN_0055FA90` and is then split
            // again, so tags in it take effect (colors and icons of the additional lines).
            Some(kind) if inserts_game_text(kind) => {
                if let Some(game) = self.game
                    && let Some(text) = game_tag_text(kind, &bytes, game, flags)
                {
                    return Some(utf16_units(&text));
                }
            }
            Some(TagKind::TaskStatus) => {
                // `FUN_00432B60(n)`: 0 gray, 1 green, 2 red - only the color changes, nothing is inserted.
                if let Some(game) = self.game
                    && let Some((n, _)) = scan_int(tag_argument(&bytes, 12))
                {
                    self.color = match game.values.task_status(n) {
                        None => TASK_UNKNOWN_COLOR,
                        Some(true) => TASK_DONE_COLOR,
                        Some(false) => TASK_OPEN_COLOR,
                    };
                }
            }
            Some(_) => {}
            None => {
                // Unknown (`0x00562459`): text run with the tag text including brackets, laid out like a word
                // (O-77).
                let run = self.text_run(piece);
                self.place(run);
            }
        }
        None
    }

    /// `[ICON <set> <index>]`: `sscanf(from 6, "%s %d")`, set via `FUN_00560F50` (unknown -> replacement set
    /// `INFO_Icons16x16`, 16 x 16).
    fn icon(&mut self, bytes: &[u8]) {
        let (name, rest) = scan_word(&bytes[6..]);
        // Without a number the index would stay a stack value in the original; the reimplementation takes -1
        // (not drawn).
        let index = scan_int(rest).map_or(-1, |(index, _)| index);
        let set = *self.fonts.icon_set(&table_name(name));
        let run = self.icon_run(set.texture, set.size, index);
        self.place(run);
    }
}

/// Lays out a text with tags (`FUN_00560FF0`, re/ui-rendering 3.8.2-3.8.4) and appends the runs to `out`: start at
/// `x = 0`, `y = out.height`.
///
/// - Splitting into words (with their space) and tags (`[…]`, divided by `;`), O-77.
/// - Wrapping only with [`layout_flags::WIDTH`] at word boundaries to `params.width` (O-78), line height = height of
///   the wrapping run (O-79), shortening at the line start or with [`layout_flags::TRUNCATE`] (O-80).
/// - Centering with [`layout_flags::CENTER`] or `[CENTER]`, both only with [`layout_flags::WIDTH`] (O-81).
/// - Start font and color always apply (O-82).
///
/// Without game context and without menu state placeholders insert nothing (`[KEYID]` with menu state:
/// [`layout_text_with`]). A `\n` in the text only ends the measurement of a word (the layout knows nothing about it);
/// when drawing, [`draw_string`] jumps back to the start of the run.
pub fn layout_text(text: &str, params: &LayoutParams, fonts: &FontBook, out: &mut TextLayout) {
    layout_text_with(text, params, fonts, out, None);
}

/// Like [`layout_text`] with menu state: with [`layout_flags::KEYID`] and `ctx`, `[KEYID n]` inserts the display name
/// of the binding of action `n` as `"(name)"` (`swprintf(L"(%s)")`, `FUN_0055FA90`); the inserted text is then split
/// like ordinary text (re/ui-rendering 3.8.2).
pub fn layout_text_with(
    text: &str,
    params: &LayoutParams,
    fonts: &FontBook,
    out: &mut TextLayout,
    ctx: Option<&TextContext>,
) {
    layout_text_game(text, params, fonts, out, ctx, None);
}

/// Like [`layout_text_with`] with game values: with `game` the game tags take effect (`[PLAYERNAME n]`,
/// `[PLAYERNAMEPLID1/2]`, `[TRADE…COST]`, `[GUIDCOUNTER a b]`, `[BODYCOUNTER a b]`, `[COUNTDOWN n]`, `[TASKSTATUS n]`,
/// re/game-ui N.9); inserted text is split again as with `[CURLEVEL…]`. Without `game` (no game context,
/// `FUN_004B9140` = 0) they vanish. The additional tags `[RESOURCES]`, `[REQUIREMENTS]`, `[HEADHUNTERREWARD]`,
/// `[FORMATION]` and `[PROGRESS]` (the latter only with switch `0x20`) take effect only with the parameter block
/// [`GameTags::params`] of the tooltip window (re/game-ui N.8), otherwise they stay empty.
pub fn layout_text_game(
    text: &str,
    params: &LayoutParams,
    fonts: &FontBook,
    out: &mut TextLayout,
    ctx: Option<&TextContext>,
    game: Option<&GameTags<'_>>,
) {
    // Copy of the text (the `;` of the tag is overwritten in it), up to the first null character.
    let mut units = utf16_units(text);
    let first_run = out.runs.len();
    let mut center = params.flags & layout_flags::CENTER != 0;
    let mut setter = Setter {
        params,
        fonts,
        ctx,
        game,
        x: 0,
        y_top: out.height,
        y_bottom: out.height,
        out,
        font: params.font,
        color: params.color,
    };
    let mut i = 0;
    // O-252: number of insertions of this call (limit [`MAX_INSERTIONS`]).
    let mut insertions = 0usize;
    while i < units.len() {
        match next_piece(&mut units, &mut i) {
            Piece::Word(word) => {
                let run = setter.text_run(word);
                setter.place(run);
            }
            Piece::Tag(tag) => {
                if let Some(inserted) = setter.tag(tag, &mut center)
                    && insertions < MAX_INSERTIONS
                {
                    insertions += 1;
                    units.splice(i..i, inserted);
                }
            }
        }
    }
    if center && params.flags & layout_flags::WIDTH != 0 {
        center_lines(&mut out.runs[first_run..], params.width);
    }
}

/// Centering (`0x00562646`, re/ui-rendering 3.8.4): groups of consecutive runs with the same `y`, offset
/// `max(0, (W − Σ w) / 2)` (integer, toward 0). Bug in the original (O-81): the **last** run is not compared with
/// `y` but always added to the current group.
fn center_lines(runs: &mut [Run], width: i32) {
    fn shift(group: &mut [Run], width: i32, sum: i32) {
        let offset = (width.wrapping_sub(sum) / 2).max(0);
        for run in group {
            run.x = run.x.wrapping_add(offset);
        }
    }
    let mut start = 0;
    let mut group_y = 0;
    let mut sum = 0i32;
    for i in 0..runs.len() {
        let last = i + 1 == runs.len();
        if !last && runs[i].y != group_y {
            shift(&mut runs[start..i], width, sum);
            start = i;
            group_y = runs[i].y;
            sum = 0;
        }
        sum = sum.wrapping_add(runs[i].w);
    }
    shift(&mut runs[start..], width, sum);
}

/// Draws the runs of a layout (`FUN_0055FD60`, with window `FUN_0055FDA0`; per run `FUN_0055F8C0`;
/// re/ui-rendering 3.8.5).
///
/// Without `window` every run at `origin + (run.x, run.y)`. With `window = [l, t, r, b]` (scroll window in the
/// layout) only runs whose rectangle intersects the window (width or height 0 never intersects), **whole** at
/// `origin − (l, t) + (run.x, run.y)`. Text run with `shadow` (O-62): first at `(x, y + 2)` in `color & 0xFF000000`,
/// then at `(x + 2, y)` in the color. Icon run: cell of the set image without shadow.
pub fn draw_runs(
    layout: &TextLayout,
    origin: [i32; 2],
    shadow: bool,
    window: Option<[i32; 4]>,
    fonts: &FontBook,
    out: &mut Vec<DrawCmd>,
) {
    let base = match window {
        Some([left, top, ..]) => [origin[0].wrapping_sub(left), origin[1].wrapping_sub(top)],
        None => origin,
    };
    for run in &layout.runs {
        if let Some(window) = window {
            let rect = [
                run.x,
                run.y,
                run.x.wrapping_add(run.w),
                run.y.wrapping_add(run.h),
            ];
            // `IntersectRect` != 0: both rectangles non-empty and with a common area.
            if intersect(rect, window).is_none() {
                continue;
            }
        }
        let [x, y] = [base[0].wrapping_add(run.x), base[1].wrapping_add(run.y)];
        match &run.content {
            RunContent::Text(codes) => {
                if shadow {
                    let shadow_color = run.color & 0xFF00_0000;
                    draw_string(
                        codes,
                        [x, y.wrapping_add(2)],
                        shadow_color,
                        run.font,
                        fonts,
                        out,
                    );
                    draw_string(
                        codes,
                        [x.wrapping_add(2), y],
                        run.color,
                        run.font,
                        fonts,
                        out,
                    );
                } else {
                    draw_string(codes, [x, y], run.color, run.font, fonts, out);
                }
            }
            &RunContent::Icon { set, size, index } => {
                draw_icon(set, size, index, [x, y], run.color, fonts, out)
            }
        }
    }
}

/// Draws an icon (`FUN_004020E0` from `FUN_0055F8C0` with scale 1.0, fill level 1.0; grid `FUN_00401DF0`;
/// cross-check, re/ui-rendering 3.8.5): destination `(x, y)…(x + kb, y + kh)`, cell `index` row by row in the grid
/// `TexB / kb` x `TexH / kh`, UV like `ImageList_DrawRect` without clipping - the same tile computation
/// ([`crate::bkg_paint`], `tile_quad`) with clip = destination; state block `3dicons-point` (scale 1.0) = POINT;
/// diffuse = run color. Index outside the grid, cell size <= 0 or image without a grid -> nothing. The only
/// difference from the former own computation: a destination whose corner would run beyond `i32::MAX` (empty
/// rectangle after the wraparound) now draws nothing instead of a twisted quad.
fn draw_icon(
    set: TextureId,
    size: [i32; 2],
    index: i32,
    pos: [i32; 2],
    color: u32,
    fonts: &FontBook,
    out: &mut Vec<DrawCmd>,
) {
    let Some(tex_size) = fonts.icon_texture_size(set) else {
        return;
    };
    let [x, y] = pos;
    let dst = [x, y, x.wrapping_add(size[0]), y.wrapping_add(size[1])];
    tile_quad(
        set,
        tex_size,
        size,
        index,
        dst,
        dst,
        color,
        Filter::Point,
        out,
    );
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::layout_flags::{CENTER, TRUNCATE};
    use super::*;
    use crate::assets::UiAssets;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};
    use nml_formats::localisation::parse_localisation;

    /// Synthetic fonts: cell 16 x 16 (texture 256^2), `Small` with `max_height` 15, `Big` with 30; an icon set `Sym`
    /// 16 x 16 on a 64 x 32 image (4 x 2 cells).
    const LOC: &str = "Font Small\nFontSize 15\nTextureFont\nFontName Test\n\
Font Big\nFontSize 30\nTextureFont\nFontName Test\n\
Icons Sym\nIconsName data\\icons\\sym.tga\nIconsSize 16 16\n";

    /// Advance in `Small`: space 4, code 0 -> 7, code 1 -> 0, `1` -> 11, otherwise 10.
    fn small_advance(c: u32) -> i32 {
        match c {
            0x20 => 4,
            0x00 => 7,
            0x01 => 0,
            0x31 => 11,
            _ => 10,
        }
    }

    /// Advance in `Big`: space 8, otherwise 20.
    fn big_advance(c: u32) -> i32 {
        if c == 0x20 { 8 } else { 20 }
    }

    struct Fixture {
        book: FontBook,
        small: FontRef,
        big: FontRef,
    }

    fn fixture() -> &'static Fixture {
        static FIXTURE: OnceLock<Fixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let white = || tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]);
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_15_0_0_0_0.fnt",
                    fnt_text(small_advance, |_| 15).into_bytes(),
                ),
                ("data/fonts/Test_15_0_0_0_0.tga", white()),
                (
                    "data/fonts/Test_30_0_0_0_0.fnt",
                    fnt_text(big_advance, |_| 30).into_bytes(),
                ),
                ("data/fonts/Test_30_0_0_0_0.tga", white()),
                (
                    "data/icons/sym.tga",
                    tga_bottom_up(64, 32, |_, _| [1, 2, 3, 255]),
                ),
            ]);
            let mut assets = UiAssets::new();
            let book = FontBook::load(&vfs, &parse_localisation(LOC), 1024, &mut assets, None);
            let small = book.by_name("Small");
            let big = book.by_name("Big");
            Fixture { book, small, big }
        })
    }

    fn params(flags: u32, width: i32) -> LayoutParams {
        LayoutParams {
            flags,
            width,
            font: fixture().small,
            color: 0xFFFF_FFFF,
        }
    }

    fn lay_with(text: &str, p: LayoutParams) -> TextLayout {
        let mut out = TextLayout::default();
        layout_text(text, &p, &fixture().book, &mut out);
        out
    }

    fn lay(text: &str, flags: u32, width: i32) -> TextLayout {
        lay_with(text, params(flags, width))
    }

    /// `(x, y, w, text)` per run; atlas codes as Latin-1, icons as `<icon>`.
    fn runs(layout: &TextLayout) -> Vec<(i32, i32, i32, String)> {
        layout
            .runs
            .iter()
            .map(|r| {
                let text = match &r.content {
                    RunContent::Text(codes) => codes.iter().map(|&b| char::from(b)).collect(),
                    RunContent::Icon { .. } => "<icon>".to_string(),
                };
                (r.x, r.y, r.w, text)
            })
            .collect()
    }

    fn r(x: i32, y: i32, w: i32, text: &str) -> (i32, i32, i32, String) {
        (x, y, w, text.to_string())
    }

    fn quad(cmd: &DrawCmd) -> ([i32; 4], [f32; 4], u32, Option<TextureId>, Filter) {
        let DrawCmd::Quad {
            texture,
            dst,
            uv,
            color,
            filter,
            blend: Blend::Alpha,
        } = *cmd
        else {
            unreachable!()
        };
        (dst, uv, color, texture, filter)
    }

    const LAYOUT: u32 = 0x46;

    #[test]
    fn glyphs_draw_whole_cells_and_advance_by_width() {
        // O-60: whole cell (16 x 16) from the pen position, advance = width from the `.fnt`, no kerning; characters
        // overlap (cell 16 > advance 10); `\n` jumps to x0 and one `max_height` lower.
        let f = fixture();
        let font = f.book.font(f.small);
        let mut out = Vec::new();
        draw_string(b"a b\nc", [5, 7], 0x80FF_0000, f.small, &f.book, &mut out);
        let glyph = |c: u8, x: i32, y: i32| {
            let [u0, u1, v0, v1] = font.uv[usize::from(c)];
            DrawCmd::Quad {
                texture: Some(font.atlas),
                dst: [x, y, x + 16, y + 16],
                uv: [u0, v0, u1, v1],
                color: 0x80FF_0000,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }
        };
        assert_eq!(
            out,
            [
                glyph(b'a', 5, 7),
                glyph(b' ', 15, 7),
                glyph(b'b', 19, 7),
                glyph(b'c', 5, 22)
            ]
        );
        // All four UVs from the `.fnt` (code 0x61: cell 1 in row 6).
        assert_eq!(
            font.uv[0x61],
            [1.0 / 16.0, 2.0 / 16.0, 6.0 / 16.0, 7.0 / 16.0]
        );
        // Measuring: up to `\n` or to the end; height independent of the text.
        assert_eq!(measure(b"a b", f.small, &f.book), [24, 15]);
        assert_eq!(measure(b"a b\nccc", f.small, &f.book), [24, 15]);
        assert_eq!(measure(b"", f.small, &f.book), [0, 15]);
        assert_eq!(measure(b"AB", f.big, &f.book), [40, 30]);
    }

    #[test]
    fn utf16_keeps_only_the_low_byte() {
        // O-61: low byte per UTF-16 unit.
        assert_eq!(atlas_codes("…ä"), [0x26, 0xE4]);
        assert_eq!(atlas_codes("aĀ"), [0x61, 0x00], "U+0100 becomes glyph 0");
        assert_eq!(atlas_codes("😀"), [0x3D, 0x00], "surrogate pair D83D DE00");
        assert_eq!(atlas_codes("a\0b"), [0x61], "ends at the null character");
        assert_eq!(atlas_codes(""), Vec::<u8>::new());
        // Glyph 0 is measured and drawn.
        let f = fixture();
        assert_eq!(measure(&[0x00], f.small, &f.book), [7, 15]);
        let mut out = Vec::new();
        draw_string(&[0x00], [0, 0], 0xFFFF_FFFF, f.small, &f.book, &mut out);
        assert_eq!(out.len(), 1);
        // In the layout: runs carry atlas codes.
        let l = lay("…ä", LAYOUT, 400);
        assert_eq!(l.runs[0].content, RunContent::Text(vec![0x26, 0xE4]));
        assert_eq!(l.runs[0].w, 20);
        // Separators apply only to the whole unit: U+0120 (low byte 0x20) does not separate, U+015B (0x5B) does not
        // begin a tag - they are drawn anyway as 0x20 and 0x5B respectively.
        assert_eq!(runs(&lay("aĠb", LAYOUT, 400)), [r(0, 0, 24, "a b")]);
        assert_eq!(runs(&lay("ś]x", LAYOUT, 400)), [r(0, 0, 30, "[]x")]);
    }

    #[test]
    fn shadow_is_two_pixels_down_and_text_two_right() {
        // O-62: shadow at (x, y+2) in `color & 0xFF000000` before the text at (x+2, y).
        let f = fixture();
        let p = LayoutParams {
            color: 0x80FF_FFFF,
            ..params(LAYOUT, 400)
        };
        let l = lay_with("ab", p);
        let mut out = Vec::new();
        draw_runs(&l, [10, 20], true, None, &f.book, &mut out);
        let got: Vec<([i32; 2], u32)> = out
            .iter()
            .map(|c| {
                let (dst, _, color, ..) = quad(c);
                ([dst[0], dst[1]], color)
            })
            .collect();
        assert_eq!(
            got,
            [
                ([10, 22], 0x8000_0000),
                ([20, 22], 0x8000_0000),
                ([12, 20], 0x80FF_FFFF),
                ([22, 20], 0x80FF_FFFF)
            ]
        );
        // Width and wrapping do not depend on the shadow (the text protrudes 2 px beyond the width).
        assert_eq!((l.width, l.height), (20, 15));
        let mut plain = Vec::new();
        draw_runs(&l, [10, 20], false, None, &f.book, &mut plain);
        assert_eq!(plain.len(), 2);
        assert_eq!(quad(&plain[0]).0, [10, 20, 26, 36]);
    }

    #[test]
    fn words_keep_their_space_and_wrap_on_it() {
        // O-78: "cd " ends at 48; with W = 47 only its space overhangs -> wrap.
        assert_eq!(
            runs(&lay("ab cd ef", LAYOUT, 47)),
            [r(0, 0, 24, "ab "), r(0, 15, 20, "cd"), r(24, 15, 20, "ef")]
        );
        // x + width = W still fits.
        assert_eq!(
            runs(&lay("ab cd ef", LAYOUT, 48)),
            [r(0, 0, 24, "ab "), r(24, 0, 24, "cd "), r(0, 15, 20, "ef")]
        );
        // Only U+0020 separates; several spaces are runs of their own, a leading one too.
        assert_eq!(
            runs(&lay(" a  b", LAYOUT, 400)),
            [
                r(0, 0, 4, " "),
                r(4, 0, 14, "a "),
                r(18, 0, 4, " "),
                r(22, 0, 10, "b")
            ]
        );
        assert_eq!(runs(&lay("a\tb", LAYOUT, 400)), [r(0, 0, 30, "a\tb")]);
    }

    #[test]
    fn first_word_of_a_new_line_loses_its_space_width_only() {
        // O-78: The wrapping word loses its space in text and width, the advance uses the old width (the gap stays);
        // the last word of the first line keeps its own.
        let l = lay("ab cd ef", LAYOUT, 47);
        assert_eq!(l.runs[0].w, 24);
        assert_eq!(l.runs[1].content, RunContent::Text(b"cd".to_vec()));
        assert_eq!(l.runs[1].w, 20);
        assert_eq!(l.runs[2].x, 24);
        assert_eq!((l.width, l.height), (44, 30));
    }

    #[test]
    fn line_height_comes_from_the_breaking_run() {
        // O-79: line 1 in Big (30 high), then "b " small; "c" wraps and advances only by 15.
        let f = fixture();
        let l = lay("[FONT Big]AA [FONT Small]b c", LAYOUT, 70);
        assert_eq!(
            runs(&l),
            [r(0, 0, 48, "AA "), r(48, 0, 14, "b "), r(0, 15, 10, "c")]
        );
        assert_eq!(l.runs[0].h, 30);
        assert_eq!(l.runs[0].font, f.big);
        assert_eq!(l.runs[2].font, f.small);
        assert_eq!(l.height, 30);
        // `CR` takes the height of the last laid-out run.
        let l = lay("[FONT Big]A[FONT Small]b[CR]c", LAYOUT, 400);
        assert_eq!(runs(&l)[2], r(0, 15, 10, "c"));
        let l = lay("[FONT Big]A[CR]b", LAYOUT, 400);
        assert_eq!(runs(&l)[1], r(0, 30, 20, "b"));
        // The total height is the bottom edge of the last run, not the largest.
        assert_eq!(lay("[FONT Big]A[FONT Small]b", LAYOUT, 400).height, 15);
    }

    #[test]
    fn double_cr_makes_no_empty_line() {
        // O-79: `CR` only sets the top edge to the bottom edge; two in a row act like one.
        let l = lay("a[CR][CR]b", LAYOUT, 400);
        assert_eq!(runs(&l), [r(0, 0, 10, "a"), r(0, 15, 10, "b")]);
        assert_eq!(l.height, 30);
        assert_eq!(runs(&lay("[CR]a", LAYOUT, 400)), [r(0, 0, 10, "a")]);
    }

    #[test]
    fn cr_space_cr_makes_one() {
        // O-79: The space run carries the line height.
        assert_eq!(
            runs(&lay("a[CR] [CR]b", LAYOUT, 400)),
            [r(0, 0, 10, "a"), r(0, 15, 4, " "), r(0, 30, 10, "b")]
        );
    }

    #[test]
    fn long_word_at_line_start_is_cut_and_after_words_overhangs() {
        // O-80: at the line start shortened character by character (rest lost), then an empty line.
        let l = lay("abcdefgh ij", LAYOUT, 35);
        assert_eq!(runs(&l), [r(0, 0, 30, "abc"), r(0, 30, 20, "ij")]);
        assert_eq!((l.width, l.height), (30, 45));
        // After other words it extends beyond the edge.
        let l = lay("a bcdefgh", LAYOUT, 35);
        assert_eq!(runs(&l), [r(0, 0, 14, "a "), r(0, 15, 70, "bcdefgh")]);
        assert_eq!(l.width, 70);
        // Without switch 0x40 no wrapping and no shortening.
        assert_eq!(
            runs(&lay("abcdefgh ij", 0x06, 35)),
            [r(0, 0, 84, "abcdefgh "), r(84, 0, 20, "ij")]
        );
    }

    #[test]
    fn truncate_flag_cuts_every_word() {
        // 0x80: every overhanging word is shortened; afterwards further words are shortened to empty and discarded,
        // but each advances y by one line height (takes effect after the next CR).
        let l = lay("ab cdef gh[CR]x", LAYOUT | TRUNCATE, 35);
        assert_eq!(
            runs(&l),
            [r(0, 0, 24, "ab "), r(24, 0, 10, "c"), r(0, 30, 10, "x")]
        );
        assert_eq!(l.height, 45);
    }

    #[test]
    fn centering_needs_the_width_flag() {
        assert_eq!(runs(&lay("aaa", 0x06 | CENTER, 200))[0].0, 0);
        assert_eq!(runs(&lay("[CENTER]aaa", 0x06, 200))[0].0, 0);
        assert_eq!(runs(&lay("[CENTERXPOS]aaa", 0x06, 200))[0].0, 0);
        // With 0x40: centered, or `CENTERXPOS` sets x = W/2 (rounded toward 0).
        assert_eq!(runs(&lay("aaa", LAYOUT | CENTER, 200))[0].0, 85);
        assert_eq!(runs(&lay("[CENTERXPOS]aaa", LAYOUT, 201))[0].0, 100);
    }

    #[test]
    fn centering_uses_integer_half() {
        // Line 101 wide in 200 -> (200 - 101) / 2 = 49.
        let l = lay("aaaaaaaaa1", LAYOUT | CENTER, 200);
        assert_eq!(runs(&l), [r(49, 0, 101, "aaaaaaaaa1")]);
        // Own offset per line; a wider line -> 0; line 3 counts the stripped width of "c".
        let l = lay("a bcdefgh c d", LAYOUT | CENTER, 35);
        assert_eq!(
            runs(&l),
            [
                r(10, 0, 14, "a "),
                r(0, 15, 70, "bcdefgh"),
                r(7, 30, 10, "c"),
                r(21, 30, 10, "d")
            ]
        );
        // Centering does not change the layout width.
        assert_eq!(l.width, 74);
    }

    #[test]
    fn center_merges_a_single_run_last_line() {
        // O-81: A last line of one run is centered together with the second-to-last as one group:
        // (100 - (30 + 20)) / 2 = 25 for both.
        assert_eq!(
            runs(&lay("aaa[CR]bb", LAYOUT | CENTER, 100)),
            [r(25, 0, 30, "aaa"), r(25, 15, 20, "bb")]
        );
        // Two runs in the last line: each line on its own.
        assert_eq!(
            runs(&lay("aaa[CR]b c", LAYOUT | CENTER, 100)),
            [r(35, 0, 30, "aaa"), r(38, 15, 14, "b "), r(52, 15, 10, "c")]
        );
    }

    #[test]
    fn layout_appends_at_the_current_height() {
        // A call appends: start at x = 0, y = layout height; only the new runs are centered.
        let f = fixture();
        let mut l = TextLayout::default();
        layout_text("aaa", &params(LAYOUT, 400), &f.book, &mut l);
        layout_text("[CENTER]b", &params(LAYOUT, 100), &f.book, &mut l);
        assert_eq!(runs(&l), [r(0, 0, 30, "aaa"), r(45, 15, 10, "b")]);
        assert_eq!((l.width, l.height), (30, 30));
    }

    #[test]
    fn tags_split_on_semicolon_and_match_by_prefix() {
        let f = fixture();
        // `[CENTER;FONT Big]` -> `[CENTER]` + `[FONT Big]`.
        let l = lay("[CENTER;FONT Big]AB", LAYOUT, 100);
        assert_eq!(runs(&l), [r(30, 0, 40, "AB")]);
        assert_eq!(l.runs[0].font, f.big);
        assert_eq!(l.runs[0].h, 30);
        // Prefix: `[CRxyz]` acts as `CR`, `[CR ]` likewise, `[CENTERED]` as `CENTER`.
        assert_eq!(runs(&lay("a[CRxyz]b", LAYOUT, 400))[1], r(0, 15, 10, "b"));
        assert_eq!(runs(&lay("a[CR ]b", LAYOUT, 400))[1], r(0, 15, 10, "b"));
        assert_eq!(runs(&lay("[CENTERED]aaa", LAYOUT, 100))[0].0, 35);
        // Several `;`: every part takes effect.
        assert_eq!(
            lay("[FONT Big;FONT Small;CR]a", LAYOUT, 400).runs[0].font,
            f.small
        );
        // Font names are case-sensitive: `big` is unknown -> replacement font.
        assert_eq!(
            lay("[FONT big]a", LAYOUT, 400).runs[0].font,
            f.book.by_name("does not exist")
        );
        // A name with a trailing space is unknown.
        assert_eq!(
            lay("[FONT Big ]a", LAYOUT, 400).runs[0].font,
            f.book.by_name("does not exist")
        );
    }

    #[test]
    fn broken_tags_render_as_text() {
        // O-77: lowercase, without space, space after `;` -> literal, as one run (laid out like a word, space
        // included).
        let p = LayoutParams {
            color: 0xFF12_3456,
            ..params(LAYOUT, 400)
        };
        let l = lay_with("[color FF00FF00]a", p);
        assert_eq!(
            runs(&l),
            [r(0, 0, 154, "[color FF00FF00]"), r(154, 0, 10, "a")]
        );
        assert!(l.runs.iter().all(|r| r.color == 0xFF12_3456));
        assert_eq!(
            runs(&lay("[COLOR]a", LAYOUT, 400)),
            [r(0, 0, 70, "[COLOR]"), r(70, 0, 10, "a")]
        );
        // `[CENTER; FONT X]`: `CENTER` takes effect, `[ FONT X]` appears; (400 - 88) / 2 = 156.
        assert_eq!(
            runs(&lay("[CENTER; FONT X]a", LAYOUT, 400)),
            [r(156, 0, 78, "[ FONT X]"), r(234, 0, 10, "a")]
        );
        // Missing `]`: the whole rest from `[` is one run (with space) in the current font.
        let l = lay("ab [FONT Big x", LAYOUT, 400);
        assert_eq!(runs(&l), [r(0, 0, 24, "ab "), r(24, 0, 98, "[FONT Big x")]);
        assert_eq!(l.runs[1].font, fixture().small);
        assert_eq!(runs(&lay("[", LAYOUT, 400)), [r(0, 0, 10, "[")]);
        // Empty tag and `]` without `[`.
        assert_eq!(runs(&lay("[]", LAYOUT, 400)), [r(0, 0, 20, "[]")]);
        assert_eq!(runs(&lay("a]b", LAYOUT, 400)), [r(0, 0, 30, "a]b")]);
        // Empty text and only tags: no runs, layout unchanged.
        for text in ["", "[CR][CENTER][COLOR FF000000]"] {
            let l = lay(text, LAYOUT, 400);
            assert_eq!(l, TextLayout::default(), "{text:?}");
        }
    }

    #[test]
    fn defaultcolor_checks_bit_2_of_the_color() {
        // O-82: `[DEFAULTCOLOR]` takes the start color only if its bit 2 (0x04) is set, otherwise white.
        let colors = |text: &str, color: u32| -> Vec<u32> {
            let p = LayoutParams {
                color,
                ..params(LAYOUT, 400)
            };
            lay_with(text, p).runs.iter().map(|r| r.color).collect()
        };
        let text = "c[COLOR FF112233]a[DEFAULTCOLOR]b";
        assert_eq!(
            colors(text, 0xFF00_FF04),
            [0xFF00_FF04, 0xFF11_2233, 0xFF00_FF04]
        );
        assert_eq!(
            colors(text, 0xFF00_FF00),
            [0xFF00_FF00, 0xFF11_2233, 0xFFFF_FFFF]
        );
        // `%X`: prefix `0x` and lowercase letters work, a failure leaves the color alone.
        assert_eq!(
            colors("[COLOR 0xff00ff00]a[COLOR zz]b[COLOR ]c", 0xFFFF_FFFF),
            [0xFF00_FF00, 0xFF00_FF00, 0xFF00_FF00]
        );
    }

    /// Game values with fixed test numbers: names `Ann` (slot 2), `Bob` (3), `[COLOR FFFF0000]X` (4), otherwise `Pn`;
    /// task 1 open, 2 done, otherwise unknown.
    struct FakeValues;

    impl GameText for FakeValues {
        fn player_name(&self, slot: i32) -> String {
            match slot {
                2 => "Ann".to_string(),
                3 => "Bob".to_string(),
                4 => "[COLOR FFFF0000]X".to_string(),
                0..=7 => format!("P{slot}"),
                _ => String::new(),
            }
        }

        fn trade_cost(&self, tag: TradeCost) -> i32 {
            match tag {
                TradeCost::WoodGold => 25,
                TradeCost::FoodGold => 33,
                TradeCost::GoldWood => -400,
                TradeCost::GoldFood => -300,
            }
        }

        fn guid_count(&self, slot: i32, guid: i32) -> i32 {
            slot * 1000 + guid
        }

        fn kill_count(&self, slot: i32, guid: i32) -> i32 {
            -(slot * 1000 + guid)
        }

        fn task_status(&self, n: i32) -> Option<bool> {
            match n {
                1 => Some(false),
                2 => Some(true),
                _ => None,
            }
        }

        fn countdown_seconds(&self, n: i32) -> i32 {
            n * 60
        }
    }

    /// Layout with game values; the runs as `(text, color)`.
    fn lay_game(text: &str, players: u16) -> Vec<(String, u32)> {
        let mut out = TextLayout::default();
        let tags = GameTags::new(&FakeValues, players);
        layout_text_game(
            text,
            &params(LAYOUT, 4000),
            &fixture().book,
            &mut out,
            None,
            Some(&tags),
        );
        runs(&out)
            .into_iter()
            .zip(&out.runs)
            .map(|((_, _, _, text), run)| (text, run.color))
            .collect()
    }

    fn lay_game_text(text: &str) -> String {
        lay_game(text, 0).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn resolved_lines_insert_game_values_and_keep_other_tags() {
        // Readable form for the viewer overlay: game tags inserted (player bytes 0x0302: PLID1 = 2, PLID2 = 3),
        // `[TASKSTATUS]` and game tags without a number dropped, other tags stay; inserted text is split again
        // (slot 4 is named `[COLOR FFFF0000]X`, the color tag stays as text).
        let tags = GameTags::new(&FakeValues, 0x0302);
        assert_eq!(
            resolve_game_tags(
                "[PLAYERNAMEPLID1] pays [PLAYERNAMEPLID2] [TRADEWOODGOLDCOST]",
                &tags
            ),
            "Ann pays Bob 25"
        );
        assert_eq!(
            resolve_game_tags(
                "[TASKSTATUS 1]A[PLAYERNAME x]B [COLOR FF00FF00]C [PLAYERNAME 4]",
                &tags
            ),
            "AB [COLOR FF00FF00]C [COLOR FFFF0000]X"
        );
        assert_eq!(
            resolve_game_tags("[GUIDCOUNTER 1 7] [BODYCOUNTER 1 7] [COUNTDOWN 2]", &tags),
            "1007 -1007 120"
        );
    }

    #[test]
    fn playername_inserts_slot_names_only_for_0_to_7() {
        // O-220 (`0x00561B7E`): only 0 <= n < 8; -1, 8 and no `%d` yield nothing (the value would otherwise be stack
        // content). The `]` after the number does not disturb `%d`.
        assert_eq!(lay_game_text("a [PLAYERNAME 2] b"), "a Ann b");
        assert_eq!(lay_game_text("a [PLAYERNAME 0] b"), "a P0 b");
        assert_eq!(lay_game_text("a [PLAYERNAME 7] b"), "a P7 b");
        for tag in ["[PLAYERNAME 8]", "[PLAYERNAME -1]", "[PLAYERNAME x]"] {
            assert_eq!(lay_game_text(&format!("a {tag} b")), "a  b", "{tag}");
        }
    }

    #[test]
    fn self_referencing_name_terminates() {
        // O-252: slot 0 is named `[PLAYERNAME 0]`; the original split the inserted name again endlessly. The
        // reimplementation inserts at most `MAX_INSERTIONS` times, after that the insertions are dropped: the layout
        // ends, the tags vanish and the rest stays.
        struct SelfNamed;
        impl GameText for SelfNamed {
            fn player_name(&self, slot: i32) -> String {
                if slot == 0 {
                    "[PLAYERNAME 0]".to_string()
                } else {
                    String::new()
                }
            }
            fn trade_cost(&self, _: TradeCost) -> i32 {
                0
            }
            fn guid_count(&self, _: i32, _: i32) -> i32 {
                0
            }
            fn kill_count(&self, _: i32, _: i32) -> i32 {
                0
            }
            fn task_status(&self, _: i32) -> Option<bool> {
                None
            }
            fn countdown_seconds(&self, _: i32) -> i32 {
                0
            }
        }
        let tags = GameTags::new(&SelfNamed, 0);
        let mut out = TextLayout::default();
        layout_text_game(
            "a [PLAYERNAME 0] b",
            &params(LAYOUT, 4000),
            &fixture().book,
            &mut out,
            None,
            Some(&tags),
        );
        let text: String = runs(&out).into_iter().map(|(_, _, _, t)| t).collect();
        assert_eq!(text, "a  b");
        // A doubling (two tags in the name) is likewise limited by the number of insertions.
        struct Doubling;
        impl GameText for Doubling {
            fn player_name(&self, _: i32) -> String {
                "[PLAYERNAME 0][PLAYERNAME 0]".to_string()
            }
            fn trade_cost(&self, _: TradeCost) -> i32 {
                0
            }
            fn guid_count(&self, _: i32, _: i32) -> i32 {
                0
            }
            fn kill_count(&self, _: i32, _: i32) -> i32 {
                0
            }
            fn task_status(&self, _: i32) -> Option<bool> {
                None
            }
            fn countdown_seconds(&self, _: i32) -> i32 {
                0
            }
        }
        let tags = GameTags::new(&Doubling, 0);
        let mut out = TextLayout::default();
        layout_text_game(
            "[PLAYERNAME 0]x",
            &params(LAYOUT, 4000),
            &fixture().book,
            &mut out,
            None,
            Some(&tags),
        );
        let text: String = runs(&out).into_iter().map(|(_, _, _, t)| t).collect();
        assert_eq!(text, "x");
    }

    #[test]
    fn plid_tags_read_the_player_bytes() {
        // Parameter block `[0xB]`: byte 0 = PLID1, byte 1 = PLID2; a byte outside 0...7 yields nothing.
        assert_eq!(
            lay_game("[PLAYERNAMEPLID1] [PLAYERNAMEPLID2]", 0x0302)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<String>(),
            "Ann Bob"
        );
        assert_eq!(
            lay_game("x [PLAYERNAMEPLID1]y [PLAYERNAMEPLID2]z", 0x08FF)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<String>(),
            "x y z"
        );
    }

    #[test]
    fn inserted_names_are_parsed_again() {
        // The inserted text is split again (`FUN_0055FA90`, like `[CURLEVEL…]`): a name with a tag colors the rest of
        // the layout.
        let l = lay_game("a [PLAYERNAME 4] b", 0);
        let texts: Vec<_> = l.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["a ", "X ", "b"]);
        assert_eq!(l[0].1, 0xFFFF_FFFF);
        assert_eq!(l[1].1, 0xFFFF_0000);
        assert_eq!(l[2].1, 0xFFFF_0000);
    }

    #[test]
    fn taskstatus_sets_the_color() {
        // `FUN_00432B60`: 0 (unknown) `FF969696`, 1 (value != 0) `FF00FF00`, 2 (value 0) `FFFF0000`; only the color
        // changes, nothing is inserted.
        let color_after = |n: i32| lay_game(&format!("a [TASKSTATUS {n}] b"), 0)[2].clone();
        assert_eq!(color_after(0), ("b".to_string(), 0xFF96_9696));
        assert_eq!(color_after(2), ("b".to_string(), 0xFF00_FF00));
        assert_eq!(color_after(1), ("b".to_string(), 0xFFFF_0000));
        assert_eq!(lay_game("a [TASKSTATUS 2] b", 0)[0].1, 0xFFFF_FFFF);
        // No `%d`: the color stays.
        assert_eq!(lay_game("a [TASKSTATUS x] b", 0)[2].1, 0xFFFF_FFFF);
    }

    #[test]
    fn counters_and_trade_costs_print_decimal() {
        // `"%d"`: sign, several digits; `%d %d` = slot and GUID.
        assert_eq!(lay_game_text("a [TRADEWOODGOLDCOST] b"), "a 25 b");
        assert_eq!(lay_game_text("a [TRADEFOODGOLDCOST] b"), "a 33 b");
        assert_eq!(lay_game_text("a [TRADEGOLDWOODCOST] b"), "a -400 b");
        assert_eq!(lay_game_text("a [TRADEGOLDFOODCOST] b"), "a -300 b");
        assert_eq!(lay_game_text("a [GUIDCOUNTER 3 456] b"), "a 3456 b");
        assert_eq!(lay_game_text("a [BODYCOUNTER 2 77] b"), "a -2077 b");
        assert_eq!(lay_game_text("a [COUNTDOWN 3] b"), "a 180 b");
        // If a value is missing, the reimplementation inserts nothing (original: stack value).
        assert_eq!(lay_game_text("a [GUIDCOUNTER 3] b"), "a  b");
        assert_eq!(lay_game_text("a [COUNTDOWN] b"), "a [COUNTDOWN] b");
    }

    #[test]
    fn without_game_values_tags_stay_empty() {
        // Current behavior (no game context, `FUN_004B9140` = 0): all game tags vanish, `[TASKSTATUS]` leaves the
        // color alone.
        let text = "a [PLAYERNAME 2] [PLAYERNAMEPLID1][TRADEWOODGOLDCOST][GUIDCOUNTER 1 2][BODYCOUNTER 1 2]\
[COUNTDOWN 1][TASKSTATUS 2]b";
        let l = lay(text, LAYOUT, 4000);
        assert_eq!(
            l.runs
                .iter()
                .map(|r| (r.color, r.content.clone()))
                .collect::<Vec<_>>(),
            [
                (0xFFFF_FFFF, RunContent::Text(b"a ".to_vec())),
                (0xFFFF_FFFF, RunContent::Text(b" ".to_vec())),
                (0xFFFF_FFFF, RunContent::Text(b"b".to_vec())),
            ]
        );
        // The additional tags stay empty without game context, with game values without a parameter block (message
        // bar) and with a parameter block but the defaults of `GameText` (no values).
        let extra = "a [RESOURCES][REQUIREMENTS][HEADHUNTERREWARD][FORMATION][PROGRESS]b";
        let flags = LAYOUT | layout_flags::PROGRESS;
        let texts = |tags: Option<&GameTags<'_>>| -> Vec<String> {
            let mut out = TextLayout::default();
            layout_text_game(
                extra,
                &params(flags, 4000),
                &fixture().book,
                &mut out,
                None,
                tags,
            );
            runs(&out).into_iter().map(|r| r.3).collect()
        };
        let bare = GameTags::new(&FakeValues, 0);
        assert_eq!(texts(None), ["a ", "b"]);
        assert_eq!(texts(Some(&bare)), ["a ", "b"]);
        assert_eq!(resolve_game_tags(extra, &bare), "a b");
        // With parameter block and defaults: only `[PROGRESS]` (switch 0x20) sets its value.
        let with = bare.with_params(TagParams {
            guid: 1000,
            slot: 0,
            value: 0,
        });
        assert_eq!(texts(Some(&with)), ["a ", "0%b"]);
    }

    /// Game values of the additional tags: slot 0 has gold 100, wood 100, food 0, free living space 1; GUID 1000 costs
    /// gold 50, wood 150, living space 1; GUID 1001 needs 1000 (object counter 0) and 1200 (object counter 1);
    /// formation 5.
    struct Extra;

    impl GameText for Extra {
        fn player_name(&self, _: i32) -> String {
            String::new()
        }
        fn trade_cost(&self, _: TradeCost) -> i32 {
            0
        }
        fn guid_count(&self, _: i32, guid: i32) -> i32 {
            i32::from(guid == 1200)
        }
        fn kill_count(&self, _: i32, _: i32) -> i32 {
            0
        }
        fn task_status(&self, _: i32) -> Option<bool> {
            None
        }
        fn countdown_seconds(&self, _: i32) -> i32 {
            0
        }
        fn text(&self, id: i32) -> Option<String> {
            let t = match id {
                1 => "[COLOR FFFF0000]",
                2 => "[DEFAULTCOLOR]",
                3 => "[COLOR FF888888]",
                7 => "[CR] - ",
                300 => "needs:",
                513 => "Wedge",
                1000 => "Barracks",
                _ => return None,
            };
            Some(t.to_string())
        }
        fn available(&self, slot: i32) -> Option<[i32; 4]> {
            (slot == 0).then_some([100, 100, 0, 1])
        }
        fn costs(&self, _: i32, guid: i32) -> Option<[i32; 5]> {
            (guid == 1000).then_some([50, 150, 0, 1, 7])
        }
        fn requirements(&self, guid: i32) -> Option<[i32; 5]> {
            match guid {
                1001 => Some([1000, 0, 1200, -3, 0]),
                1002 => Some([0, -1, 0, 0, 0]),
                _ => None,
            }
        }
        fn headhunter_reward(&self, guid: i32) -> String {
            format!("[ICON INFOICONS 0]{guid}")
        }
        fn formation(&self) -> i32 {
            5
        }
    }

    #[test]
    fn resolve_knows_the_extra_tags() {
        // The readable form (`ui_state`) inserts the additional tags; their color and icon tags stay as text. Missing
        // texts appear as a number (formation 5 -> 516, prerequisite 1200 without a name -> "1200").
        let p = |guid: i32, value: i32| TagParams {
            guid,
            slot: 0,
            value,
        };
        let tags = GameTags::new(&Extra, 0);
        let r = |text: &str, params: TagParams, flags: u32| {
            resolve_text(text, flags, None, &tags.with_params(params))
        };
        assert_eq!(
            r("[RESOURCES]", p(1000, 1), 0),
            "[DEFAULTCOLOR][ICON INFOICONS 0]50 [COLOR 0xFFFF0000][ICON INFOICONS 1]150 \
[DEFAULTCOLOR][ICON INFOICONS 3]1 [DEFAULTCOLOR]"
        );
        // Factor 2: gold 100 = stock (not red), living space 2 > 1 (red).
        assert_eq!(
            r("[RESOURCES]", p(1000, 2), 0),
            "[DEFAULTCOLOR][ICON INFOICONS 0]100 [COLOR 0xFFFF0000][ICON INFOICONS 1]300 \
[COLOR 0xFFFF0000][ICON INFOICONS 3]2 [DEFAULTCOLOR]"
        );
        // Without a value set only `[DEFAULTCOLOR]`; without a player nothing.
        assert_eq!(r("[RESOURCES]", p(1005, 1), 0), "[DEFAULTCOLOR]");
        let no_player = TagParams {
            slot: 3,
            ..p(1000, 1)
        };
        assert_eq!(r("x[RESOURCES]y", no_player, 0), "xy");
        assert_eq!(
            r("[REQUIREMENTS]", p(1001, 1), 0),
            "needs:[COLOR FFFF0000][CR] - Barracks[COLOR FF888888][CR] - 1200[DEFAULTCOLOR]"
        );
        // No positive prerequisite or no entry: nothing.
        assert_eq!(r("x[REQUIREMENTS]y", p(1002, 1), 0), "xy");
        assert_eq!(r("x[REQUIREMENTS]y", p(1003, 1), 0), "xy");
        assert_eq!(
            r("[HEADHUNTERREWARD]", p(4100, 1), 0),
            "[ICON INFOICONS 0]4100"
        );
        assert_eq!(r("F: [FORMATION]", p(0, 1), 0), "F: 516");
        // `[PROGRESS]` only with switch 0x20.
        assert_eq!(r("[PROGRESS] done", p(0, 75), 0), " done");
        assert_eq!(
            r("[PROGRESS] done", p(0, 75), layout_flags::PROGRESS),
            "75% done"
        );
        // `[KEYID]` only with switch 0x200 and menu state, otherwise the tag stays as text (as before).
        let ctx = TextContext {
            key_names: BTreeMap::from([(4, "B".to_string())]),
            undefined: "0x00 0x00".to_string(),
            ..TextContext::default()
        };
        assert_eq!(
            resolve_text("a [KEYID 4]", layout_flags::KEYID, Some(&ctx), &tags),
            "a (B)"
        );
        assert_eq!(resolve_game_tags("a [KEYID 4]", &tags), "a [KEYID 4]");
    }

    #[test]
    fn extra_tags_color_their_runs() {
        // In the layout the inserted tags take effect: wood (icon and number) red, gold in the base color (O-243).
        let tags = GameTags::new(&Extra, 0).with_params(TagParams {
            guid: 1000,
            slot: 0,
            value: 1,
        });
        let mut out = TextLayout::default();
        layout_text_game(
            "[RESOURCES]",
            &params(LAYOUT, 4000),
            &fixture().book,
            &mut out,
            None,
            Some(&tags),
        );
        let got: Vec<(String, u32)> = runs(&out)
            .into_iter()
            .zip(&out.runs)
            .map(|((_, _, _, t), run)| (t, run.color))
            .collect();
        let s = |t: &str, c: u32| (t.to_string(), c);
        assert_eq!(
            got,
            [
                s("<icon>", 0xFFFF_FFFF),
                s("50 ", 0xFFFF_FFFF),
                s("<icon>", 0xFFFF_0000),
                s("150 ", 0xFFFF_0000),
                s("<icon>", 0xFFFF_FFFF),
                s("1 ", 0xFFFF_FFFF),
            ]
        );
    }

    #[test]
    fn placeholders_vanish_without_game_context() {
        // Game placeholders without game context, `[PROGRESS]` without 0x20, `[KEYID]` without 0x200 and the level
        // details without menu state: nothing.
        let l = lay(
            "[PLAYERNAME 1]a[PROGRESS]b[KEYID 5]c[RESOURCES][CURLEVELNAME][TASKSTATUS 3][PLAYERNAMEPLID1]",
            LAYOUT,
            400,
        );
        assert_eq!(
            runs(&l),
            [r(0, 0, 10, "a"), r(10, 0, 10, "b"), r(20, 0, 10, "c")]
        );
    }

    #[test]
    fn keyid_placeholder_shows_the_key_name() {
        // With 0x200 `[KEYID n]` inserts `"(name)"` at the point behind the tag (`FUN_0055FA90`,
        // re/ui-rendering 3.8.2); the text is then split like ordinary text. `n` is read by `atol` (no value -> 0), an
        // action without an entry shows the display name of "not defined" (O-113).
        let ctx = TextContext {
            key_names: BTreeMap::from([(5, "SHIFT D".to_string()), (0, "Q".to_string())]),
            undefined: "0x00 0x00".to_string(),
            ..TextContext::default()
        };
        let lay_ctx = |text: &str, flags: u32, ctx: Option<&TextContext>| {
            let mut out = TextLayout::default();
            layout_text_with(text, &params(flags, 400), &fixture().book, &mut out, ctx);
            out
        };
        let keyid = LAYOUT | layout_flags::KEYID;
        assert_eq!(
            runs(&lay_ctx("a[KEYID 5]b", keyid, Some(&ctx))),
            [
                r(0, 0, 10, "a"),
                r(10, 0, 64, "(SHIFT "),
                r(74, 0, 30, "D)b")
            ]
        );
        // Without 0x200 or without menu state: nothing.
        assert_eq!(
            runs(&lay_ctx("a[KEYID 5]b", LAYOUT, Some(&ctx))),
            [r(0, 0, 10, "a"), r(10, 0, 10, "b")]
        );
        assert_eq!(
            runs(&lay_ctx("a[KEYID 5]b", keyid, None)),
            [r(0, 0, 10, "a"), r(10, 0, 10, "b")]
        );
        // Unknown action, `atol` without digits (-> 0) and the `;` of the tag (inserting before the next tag).
        assert_eq!(
            runs(&lay_ctx("[KEYID 7]", keyid, Some(&ctx))),
            [r(0, 0, 54, "(0x00 "), r(54, 0, 50, "0x00)")]
        );
        assert_eq!(
            runs(&lay_ctx("[KEYID x][KEYID 5;CR]z", keyid, Some(&ctx))),
            [
                r(0, 0, 30, "(Q)"),
                r(30, 0, 64, "(SHIFT "),
                r(94, 0, 20, "D)"),
                r(0, 15, 10, "z")
            ]
        );
    }

    #[test]
    fn curlevel_placeholders_insert_text_whose_tags_take_effect() {
        // Ghidra `FUN_00560FF0` `0x00561FD8`...`0x0056212F`: `[CURLEVELNAME]`, `[CURLEVELSIZE]`,
        // `[CURLEVELPLAYERS]`, `[CURLEVELINFOS]` call `FUN_0055FA90(&text, pointer, position, 0)` without checking a
        // layout switch - the text goes into the working text at the point behind the tag, the splitting continues
        // over it: **inserted text evaluates its tags** (`[CR]`, `[COLOR]` ..., this is how `LEVELINFO` builds its
        // lines). Without a chosen map (pointer 0) and without menu state: nothing.
        let ctx = TextContext {
            cur_level: Some(CurLevelText {
                name: "Island".into(),
                size: "Small".into(),
                players: "2 Players".into(),
                infos: "[COLOR FF88FF88]X[CR]Y".into(),
            }),
            ..TextContext::default()
        };
        let lay_ctx = |text: &str, ctx: Option<&TextContext>| {
            let mut out = TextLayout::default();
            layout_text_with(text, &params(LAYOUT, 400), &fixture().book, &mut out, ctx);
            out
        };
        let texts = |l: &TextLayout| -> Vec<String> { runs(l).into_iter().map(|r| r.3).collect() };
        let line = "N:[CURLEVELNAME;CR]G:[CURLEVELSIZE;CR]S: [CURLEVELPLAYERS]";
        let l = lay_ctx(line, Some(&ctx));
        assert_eq!(
            texts(&l),
            ["N:", "Island", "G:", "Small", "S: ", "2 ", "Players"]
        );
        let ys: Vec<i32> = l.runs.iter().map(|r| r.y).collect();
        assert_eq!(
            ys,
            [0, 0, 15, 15, 30, 30, 30],
            "[CR] after the inserted text"
        );
        // Inserted tags take effect: color and line break from `infos`; without switch 0x200 (label switches 0x46).
        let l = lay_ctx("a[CURLEVELINFOS]b", Some(&ctx));
        assert_eq!(texts(&l), ["a", "X", "Yb"]);
        assert_eq!(l.runs[1].color, 0xFF88_FF88);
        assert_eq!((l.runs[2].x, l.runs[2].y), (0, 15));
        // Without a map or without menu state: nothing inserted.
        let none = TextContext::default();
        assert_eq!(texts(&lay_ctx(line, Some(&none))), ["N:", "G:", "S: "]);
        assert_eq!(texts(&lay_ctx(line, None)), ["N:", "G:", "S: "]);
    }

    #[test]
    fn setplayercolor_uses_the_table() {
        // Table `0x006321B4` (re/ui-rendering 3.8.2).
        const TABLE: [u32; 9] = [
            0xFFFF_CE0D,
            0xFF00_73E6,
            0xFF17_A710,
            0xFFC4_1800,
            0xFF82_01D8,
            0xFFB9_0066,
            0xFFFF_9F14,
            0xFF00_A8A8,
            0xFFAC_ACAC,
        ];
        for (i, &color) in TABLE.iter().enumerate() {
            let l = lay(&format!("[SETPLAYERCOLOR {i}]a"), LAYOUT, 400);
            assert_eq!(l.runs[0].color, color, "index {i}");
        }
        // >= 9, negative (O-86) or not a number: the color stays.
        for bad in ["9", "-1", "x", ""] {
            let l = lay(&format!("[SETPLAYERCOLOR {bad}]a"), LAYOUT, 400);
            assert_eq!(l.runs[0].color, 0xFFFF_FFFF, "{bad:?}");
        }
    }

    #[test]
    fn icon_runs_take_the_set_size_and_draw_one_tile() {
        let f = fixture();
        let set = *f.book.icon_set("Sym");
        let p = LayoutParams {
            color: 0xFF80_8080,
            ..params(LAYOUT, 400)
        };
        let l = lay_with("a[ICON Sym 5]b", p);
        assert_eq!(
            runs(&l),
            [r(0, 0, 10, "a"), r(10, 0, 16, "<icon>"), r(26, 0, 10, "b")]
        );
        assert_eq!(
            l.runs[1].content,
            RunContent::Icon {
                set: set.texture,
                size: [16, 16],
                index: 5
            }
        );
        assert_eq!(l.runs[1].h, 16);
        // Drawing (`FUN_004020E0`): cell 5 in the 4 x 2 grid, UV like `ImageList_DrawRect` with half texel 0.5/64;
        // POINT (scale 1.0 -> `3dicons-point`), run color, without shadow.
        let mut out = Vec::new();
        draw_runs(&l, [0, 0], true, None, &f.book, &mut out);
        assert_eq!(out.len(), 5);
        assert_eq!(
            out[2],
            DrawCmd::Quad {
                texture: Some(set.texture),
                dst: [10, 0, 26, 16],
                uv: [0.257_812_5, 0.507_812_5, 0.476_562_5, 0.960_937_5],
                color: 0xFF80_8080,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }
        );
        // Index outside the grid or negative: run (space) yes, quad no.
        for index in ["8", "-1"] {
            let l = lay(&format!("[ICON Sym {index}]"), LAYOUT, 400);
            assert_eq!(l.runs.len(), 1);
            let mut out = Vec::new();
            draw_runs(&l, [0, 0], false, None, &f.book, &mut out);
            assert!(out.is_empty(), "{index}");
        }
        // Unknown set: replacement set 16 x 16 (its image is missing here -> nothing drawn).
        let l = lay("[ICON Nope 0]", LAYOUT, 400);
        assert_eq!(l.runs[0].w, 16);
        // Icons are never shortened, they overhang.
        assert_eq!(
            runs(&lay("[ICON Sym 0]", LAYOUT, 10)),
            [r(0, 0, 16, "<icon>")]
        );
    }

    #[test]
    fn window_draws_whole_intersecting_runs() {
        // `FUN_0055FDA0`: runs whose rectangle intersects the window, whole at origin - window.lt + run.xy.
        let f = fixture();
        let l = lay("ab cd", LAYOUT, 400); // "ab " (0,0)-(24,15), "cd" (24,0)-(44,15)
        let mut out = Vec::new();
        draw_runs(
            &l,
            [100, 50],
            false,
            Some([20, 5, 22, 6]),
            &f.book,
            &mut out,
        );
        let xy: Vec<[i32; 2]> = out
            .iter()
            .map(|c| {
                let d = quad(c).0;
                [d[0], d[1]]
            })
            .collect();
        assert_eq!(xy, [[80, 45], [90, 45], [100, 45]]);
        // Touching at the edge does not intersect; window outside -> nothing.
        let mut out = Vec::new();
        draw_runs(&l, [0, 0], false, Some([0, 0, 24, 15]), &f.book, &mut out);
        assert_eq!(out.len(), 3);
        let mut out = Vec::new();
        draw_runs(
            &l,
            [0, 0],
            false,
            Some([200, 0, 300, 15]),
            &f.book,
            &mut out,
        );
        assert!(out.is_empty());
        // A run with width 0 (code 1 has advance 0) is missing with a window, without a window it is drawn.
        let l = lay("\u{1}", LAYOUT, 400);
        assert_eq!(l.runs[0].w, 0);
        let mut out = Vec::new();
        draw_runs(
            &l,
            [0, 0],
            false,
            Some([-10, -10, 10, 10]),
            &f.book,
            &mut out,
        );
        assert!(out.is_empty());
        draw_runs(&l, [0, 0], false, None, &f.book, &mut out);
        assert_eq!(out.len(), 1);
    }
}
