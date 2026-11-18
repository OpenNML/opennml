//! Evaluated description of a menu dialog: a tree of [`WidgetDesc`] with the descriptors of the eight
//! element kinds, as the element parsers of the original fill them (`docs/formats/dlg.md`, section "Element kinds").
//!
//! Fields, defaults and flag bits come from the descriptor constructors (`FUN_004615E0` frame,
//! `FUN_0045EA10` button, `FUN_00463F50` list, `FUN_0045FBB0` edit field, `FUN_00465070` slider, `FUN_00465C10`
//! radio button, `FUN_00461FE0` label). Colors are `u32` ARGB, rectangles `[x1, y1, x2, y2]` as read; only
//! the widget swaps reversed corners (`FUN_0045A270`, `normalize_rect`). Text IDs stay numbers; the
//! lookup in the text table (`FUN_0055FFE0`) is done by `opennml-ui`.
//!
//! Deviation from the original (quirk O-9): fields without a default in the original (stack garbage when the
//! key is missing) are `Option<u32>` here; `opennml-ui` sets the substitute value.

use super::ElementKind;

/// Flags every element kind knows (`0x00620024`, `0x0062001C`); also run-time state.
pub mod common_flags {
    pub const HIDE: u32 = 0x1000_0000;
    pub const DISABLE: u32 = 0x2000_0000;
}

/// `FRAME` (`FUN_004DD9C0`, evidence `0x004DE1F8`...`0x004DE3E4`).
pub mod frame_flags {
    pub const BACKGROUND: u32 = 0x1;
    pub const DRAWTAGS: u32 = 0x2;
    pub const DRAWBARL: u32 = 0x4;
    pub const DRAWTITLEL: u32 = 0x8;
    pub const DRAWBARR: u32 = 0x10;
    pub const DRAWTITLER: u32 = 0x20;
    pub const MUSIC: u32 = 0x40;
    pub const LEVELBACKGROUND: u32 = 0x80;
    /// Not from the file: the constructor `FUN_00461AD0` sets it when a title flag is present without a bar flag
    /// (rule in `opennml-ui`).
    pub const TITLE_WITHOUT_BAR: u32 = 0x100;
    pub const MODAL: u32 = 0x200;
    pub const NOFRAME: u32 = 0x400;
    pub const DONOTLOOPMUSIC: u32 = 0x800;
    pub const MOVE: u32 = 0x1000;
    pub const DONOTCLEARBACKGROUND: u32 = 0x10_0000;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `BUTTON` (`FUN_004E0300`) and the button part of the `COMBOBOX`.
pub mod button_flags {
    pub const TEXT: u32 = 0x1;
    pub const BORDER: u32 = 0x2;
    pub const SELECTABLE: u32 = 0x4;
    pub const SELECTED: u32 = 0x8;
    /// Not a flag word: `COMBOBOX` sets it before reading (`0x004DEE2C`).
    pub const COMBOBOX: u32 = 0x10;
    pub const CHECKESCAPE: u32 = 0x1_0000;
    pub const CHECKENTER: u32 = 0x2_0000;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `COMBOBOX` (`FUN_004DEDE0`, `0x004DF526`...`0x004DF563`); the bits live in the button descriptor.
pub mod combobox_flags {
    pub const TEXT: u32 = 0x1;
    pub const SELECTABLE: u32 = 0x4;
    pub const SELECTED: u32 = 0x8;
    pub const COMBOBOX: u32 = super::button_flags::COMBOBOX;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `LISTBOX` (`FUN_004DF790`, `0x004DFBF7`).
pub mod listbox_flags {
    pub const AUTOYSIZE: u32 = 0x100;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `EDITFIELD` (`FUN_004DFD90`, `0x004E01A8`).
pub mod editfield_flags {
    pub const KEYNAME: u32 = 0x1;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `SLIDER` (`FUN_004E0B20`, `0x004E0FF2`...`0x004E104F`).
pub mod slider_flags {
    pub const TEXT: u32 = 0x2;
    pub const SCALE: u32 = 0x4;
    pub const VERTICAL: u32 = 0x8;
    pub const CENTERTEXT: u32 = 0x10;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `RADIOBUTTON` (`FUN_004E11E0`, `0x004E15F8`, `0x004E1655`).
pub mod radio_flags {
    pub const TEXT: u32 = 0x1;
    pub const CHECKED: u32 = 0x4;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// `LABEL` (`FUN_004E17B0`, `0x004E1C5D`...`0x004E1CD4`).
pub mod label_flags {
    pub const BACKGROUND: u32 = 0x1;
    pub const TEXTARRAY: u32 = 0x2;
    pub const AUTOHSCROLL: u32 = 0x4;
    /// Not a flag word: the key `CREDITS` sets it (`0x004E188B`).
    pub const CREDITS: u32 = 0x8;
    pub const NOLINEBREAK: u32 = 0x10;
    pub const TGA: u32 = 0x20;
    pub const HIDE: u32 = super::common_flags::HIDE;
    pub const DISABLE: u32 = super::common_flags::DISABLE;
}

/// Bit of a flag word (uppercase) in the flag mode of the element kind; `None` = unknown flag (the original
/// then aborts the file). `FLAGS` and `/FLAGS` do not count.
pub fn flag_bit(kind: ElementKind, word: &str) -> Option<u32> {
    use ElementKind::*;
    match word {
        "HIDE" => return Some(common_flags::HIDE),
        "DISABLE" => return Some(common_flags::DISABLE),
        _ => {}
    }
    Some(match (kind, word) {
        (Frame, "MENUFRAME_DONOTCLEARBACKGROUND") => frame_flags::DONOTCLEARBACKGROUND,
        (Frame, "MENUFRAME_NOFRAME") => frame_flags::NOFRAME,
        (Frame, "MENUFRAME_LEVELBACKGROUND") => frame_flags::LEVELBACKGROUND,
        (Frame, "MENUFRAME_BACKGROUND") => frame_flags::BACKGROUND,
        (Frame, "MENUFRAME_MUSIC") => frame_flags::MUSIC,
        (Frame, "MENUFRAME_DONOTLOOPMUSIC") => frame_flags::DONOTLOOPMUSIC,
        (Frame, "MENUFRAME_DRAWTAGS") => frame_flags::DRAWTAGS,
        (Frame, "MENUFRAME_DRAWBARL") => frame_flags::DRAWBARL,
        (Frame, "MENUFRAME_DRAWTITLEL") => frame_flags::DRAWTITLEL,
        (Frame, "MENUFRAME_DRAWBARR") => frame_flags::DRAWBARR,
        (Frame, "MENUFRAME_DRAWTITLER") => frame_flags::DRAWTITLER,
        (Frame, "MENUFRAME_MOVE") => frame_flags::MOVE,
        (Frame, "MENUFRAME_MODAL") => frame_flags::MODAL,
        (Button, "MENUBUTTON_TEXT") => button_flags::TEXT,
        (Button, "MENUBUTTON_SELECTABLE") => button_flags::SELECTABLE,
        (Button, "MENUBUTTON_SELECTED") => button_flags::SELECTED,
        (Button, "MENUBUTTON_BORDER") => button_flags::BORDER,
        (Button, "MENUBUTTON_CHECKESCAPE") => button_flags::CHECKESCAPE,
        (Button, "MENUBUTTON_CHECKENTER") => button_flags::CHECKENTER,
        (ComboBox, "MENUCOMBOBOX_TEXT") => combobox_flags::TEXT,
        (ComboBox, "MENUCOMBOBOX_SELECTABLE") => combobox_flags::SELECTABLE,
        (ComboBox, "MENUCOMBOBOX_SELECTED") => combobox_flags::SELECTED,
        (ListBox, "LISTBOX_AUTOYSIZE") => listbox_flags::AUTOYSIZE,
        (EditField, "KEYNAME") => editfield_flags::KEYNAME,
        (Slider, "MENUSLIDER_TEXT") => slider_flags::TEXT,
        (Slider, "MENUSLIDER_SCALE") => slider_flags::SCALE,
        (Slider, "MENUSLIDER_VERTICAL") => slider_flags::VERTICAL,
        (Slider, "MENUSLIDER_CENTERTEXT") => slider_flags::CENTERTEXT,
        (RadioButton, "RADIOBUTTON_TEXT") => radio_flags::TEXT,
        (RadioButton, "RADIOBUTTON_CHECKED") => radio_flags::CHECKED,
        (Label, "MENULABEL_BACKGROUND") => label_flags::BACKGROUND,
        (Label, "MENULABEL_AUTOHSCROLL") => label_flags::AUTOHSCROLL,
        (Label, "MENULABEL_TGA") => label_flags::TGA,
        (Label, "MENULABEL_TEXTARRAY") => label_flags::TEXTARRAY,
        (Label, "MENULABEL_NOLINEBREAK") => label_flags::NOLINEBREAK,
        _ => return None,
    })
}

/// A built element with its children in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetDesc {
    /// Key of the name table (`MenuSystem+0x20`): for `FRAME` the name, otherwise prefix (name of the most recently
    /// started `FRAME`) + name + loop counters from outside to inside, uppercase (`dlg.md`, section "Names").
    pub name: String,
    /// Element name as the widget carries it (`+0x9C`, `FUN_004DEAE0`): uppercase with the loop counters
    /// from outside to inside, **without** frame prefix; for the `FRAME` its name. The messages to the dialog
    /// logic carry it in lowercase (`CString_MakeLowerGerman` in `FUN_00464970`).
    pub element: String,
    pub kind: WidgetKind,
    pub children: Vec<WidgetDesc>,
}

/// Kind and descriptor of an element. `COMBOBOX` is a `BUTTON` with an embedded list descriptor in the
/// original (`FUN_0045EAD0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind {
    Frame(FrameDesc),
    Button(ButtonDesc),
    ComboBox(ButtonDesc, ListDesc),
    ListBox(ListDesc),
    EditField(EditDesc),
    Slider(SliderDesc),
    RadioButton(RadioDesc),
    Label(LabelDesc),
}

/// `FRAME` descriptor (`FUN_004615E0`, `0x5C` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDesc {
    pub flags: u32,
    /// `EXTENT` (`+0x18...+0x24`): start position of the fly-in animation.
    pub extent: [i32; 4],
    /// `ZIELPOSITION` (`+0x28/+0x2C`).
    pub target: [i32; 2],
    /// `DURATION` (`+0x58`), default 1000 ms.
    pub duration_ms: i32,
    /// `BAREXTENT` (`+0x30...+0x3C`), absolute.
    pub bar_extent: [i32; 4],
    /// `TITLECOLOR` (`+0x54`).
    pub title_color: u32,
    /// `TITLEEXTENT` (`+0x40...+0x4C`), absolute.
    pub title_extent: [i32; 4],
    /// `TITLE` (text ID, `+0x50`).
    pub title: Option<i32>,
    /// Field `+0x08` after the background logic (`BACKGROUND` image or `LEVELBACKGROUND` `.lvl`, O-7).
    pub background: Option<String>,
    /// `MUSIC` (`+0x0C`).
    pub music: Option<String>,
    /// `BKGFILE` (`+0x10`).
    pub bkg_file: Option<String>,
}

impl Default for FrameDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            target: [0; 2],
            duration_ms: 1000,
            bar_extent: [0; 4],
            title_color: 0xFFFF_FFFF,
            title_extent: [0; 4],
            title: None,
            background: None,
            music: None,
            bkg_file: None,
        }
    }
}

/// `BUTTON` descriptor (`FUN_0045EA10`, `0x74` bytes), also the button part of the `COMBOBOX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonDesc {
    pub flags: u32,
    /// `EXTENT` or, for the combobox, `BUTTONEXTENT` (`+0x10...+0x1C`).
    pub extent: [i32; 4],
    /// `BORDERSPACE` (`+0x20...+0x2C`); the combobox does not know the key.
    pub border_space: [i32; 4],
    /// `TEXT` (`+0x0C`).
    pub text: Option<i32>,
    pub text_color: u32,
    pub disabled_text_color: u32,
    /// `MOUSEOVERTEXTCOLOR` (`+0x6C`), without a default (O-9).
    pub mouse_over_text_color: Option<u32>,
    /// `PRESSEDBUTTONTEXTCOLOR` (`+0x70`), without a default (O-9).
    pub pressed_text_color: Option<u32>,
}

impl Default for ButtonDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            border_space: [0; 4],
            text: None,
            text_color: 0xFFFF_FFFF,
            disabled_text_color: 0xFFB0_B0B0,
            mouse_over_text_color: None,
            pressed_text_color: None,
        }
    }
}

/// List descriptor (`FUN_00463F50`, `0x34` bytes): `LISTBOX` and the list of the `COMBOBOX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListDesc {
    pub flags: u32,
    /// `EXTENT` or, for the combobox, `LISTBOXEXTENT` (`+0x0C...+0x18`).
    pub extent: [i32; 4],
    /// `+0x24`; the combobox also writes its `TEXTCOLOR` here (O-10).
    pub text_color: u32,
    pub disabled_text_color: u32,
    /// `+0x30`; 0 = no bar.
    pub selection_bar_color: u32,
    /// `+0x2C`.
    pub mouse_over_color: u32,
    /// `+0x28`.
    pub selected_text_color: u32,
    /// `CURSELECTION` (`+0x1C`), -1 = none.
    pub cur_selection: i32,
}

impl Default for ListDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            text_color: 0xFFB0_B0B0,
            disabled_text_color: 0xFFA0_A0A0,
            selection_bar_color: 0xFF00_A000,
            mouse_over_color: 0xFFFF_FFFF,
            selected_text_color: 0xFF00_FF00,
            cur_selection: -1,
        }
    }
}

/// `EDITFIELD` descriptor (`FUN_0045FBB0`, `0x28` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDesc {
    pub flags: u32,
    pub extent: [i32; 4],
    pub text_color: u32,
    pub disabled_text_color: u32,
    pub text: Option<i32>,
}

impl Default for EditDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            text_color: 0xFFFF_FFFF,
            disabled_text_color: 0xFFB0_B0B0,
            text: None,
        }
    }
}

/// `SLIDER` descriptor (`FUN_00465070`, `0x38` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliderDesc {
    pub flags: u32,
    pub extent: [i32; 4],
    /// `CURVALUE` (`+0x1C`), not limited when reading.
    pub cur_value: i32,
    /// `RESOLUTION` (`+0x20`), default 10 (`0x004650C1`).
    pub resolution: i32,
    pub text_color: u32,
    pub disabled_text_color: u32,
    pub thumb_color: u32,
    pub scale_color: u32,
    pub text: Option<i32>,
}

impl Default for SliderDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            cur_value: 0,
            resolution: 10,
            text_color: 0xFFFF_FFFF,
            disabled_text_color: 0xFFB0_B0B0,
            thumb_color: 0xFFFF_FFFF,
            scale_color: 0x8080_8080,
            text: None,
        }
    }
}

/// `RADIOBUTTON` descriptor (`FUN_00465C10`, `0x2C` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioDesc {
    pub flags: u32,
    pub extent: [i32; 4],
    /// `ICONOFFSET` (`+0x1C`): tile index of the icon.
    pub icon_offset: i32,
    pub text: Option<i32>,
    pub text_color: u32,
    /// Read, never used when drawing (O-11).
    pub disabled_text_color: u32,
}

impl Default for RadioDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            icon_offset: 0,
            text: None,
            text_color: 0xFFFF_FFFF,
            disabled_text_color: 0xFFB0_B0B0,
        }
    }
}

/// `LABEL` descriptor (`FUN_00461FE0`, `0x30` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDesc {
    pub flags: u32,
    pub extent: [i32; 4],
    pub text_color: u32,
    /// `BACKGROUNDCOLOR` (`+0x0C`), without a default (O-9); only used with `MENULABEL_BACKGROUND`.
    pub background_color: Option<u32>,
    /// Unused (O-11).
    pub disabled_text_color: u32,
    pub text: Option<i32>,
    /// `CREDITS` (`+0x2C`): credits file; sets [`label_flags::CREDITS`].
    pub credits: Option<String>,
}

impl Default for LabelDesc {
    fn default() -> Self {
        Self {
            flags: 0,
            extent: [0; 4],
            text_color: 0xFFFF_FFFF,
            background_color: None,
            disabled_text_color: 0xFFB0_B0B0,
            text: None,
            credits: None,
        }
    }
}

impl WidgetKind {
    /// Empty descriptor of the kind with the defaults of the original; the combobox already carries bit `0x10`.
    pub fn with_defaults(kind: ElementKind) -> Self {
        match kind {
            ElementKind::Frame => Self::Frame(FrameDesc::default()),
            ElementKind::Button => Self::Button(ButtonDesc::default()),
            ElementKind::ComboBox => Self::ComboBox(
                ButtonDesc {
                    flags: button_flags::COMBOBOX,
                    ..ButtonDesc::default()
                },
                ListDesc::default(),
            ),
            ElementKind::ListBox => Self::ListBox(ListDesc::default()),
            ElementKind::EditField => Self::EditField(EditDesc::default()),
            ElementKind::Slider => Self::Slider(SliderDesc::default()),
            ElementKind::RadioButton => Self::RadioButton(RadioDesc::default()),
            ElementKind::Label => Self::Label(LabelDesc::default()),
        }
    }

    /// Element kind of the descriptor.
    pub fn element_kind(&self) -> ElementKind {
        match self {
            Self::Frame(_) => ElementKind::Frame,
            Self::Button(_) => ElementKind::Button,
            Self::ComboBox(..) => ElementKind::ComboBox,
            Self::ListBox(_) => ElementKind::ListBox,
            Self::EditField(_) => ElementKind::EditField,
            Self::Slider(_) => ElementKind::Slider,
            Self::RadioButton(_) => ElementKind::RadioButton,
            Self::Label(_) => ElementKind::Label,
        }
    }

    /// Flag field of the descriptor (for the combobox that of the button).
    pub fn flags(&self) -> u32 {
        match self {
            Self::Frame(d) => d.flags,
            Self::Button(d) | Self::ComboBox(d, _) => d.flags,
            Self::ListBox(d) => d.flags,
            Self::EditField(d) => d.flags,
            Self::Slider(d) => d.flags,
            Self::RadioButton(d) => d.flags,
            Self::Label(d) => d.flags,
        }
    }

    /// Mutable flag field (for the combobox that of the button).
    pub fn flags_mut(&mut self) -> &mut u32 {
        match self {
            Self::Frame(d) => &mut d.flags,
            Self::Button(d) | Self::ComboBox(d, _) => &mut d.flags,
            Self::ListBox(d) => &mut d.flags,
            Self::EditField(d) => &mut d.flags,
            Self::Slider(d) => &mut d.flags,
            Self::RadioButton(d) => &mut d.flags,
            Self::Label(d) => &mut d.flags,
        }
    }
}
