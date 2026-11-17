use std::collections::BTreeMap;

use nml_formats::gifcon::button_kind as k;
use nml_formats::localisation::parse_localisation;

use super::*;
use crate::assets::{Texts, UiAssets};
use crate::coords::Screen;
use crate::draw::TextureId;
use crate::hud::toolbar::button::{
    KIND_HERO, KIND_MISSING_REQUIREMENT, KIND_PRODUCTION, KIND_SELECTION,
};
use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
use crate::text::{RunContent, TradeCost};

const WHITE: u32 = 0xFFFF_FFFF;
const RED: u32 = 0xFFFF_0000;
const GREY: u32 = 0xFF88_8888;
/// Kind of the areas (portrait, values, counter-upgrades).
const KIND_AREA: u32 = 0x1700_0000;

/// Game values for the tests: local slot 0; texts from `extrasymbols.txt` (1-9) and a few GUIDs.
struct Game {
    texts: BTreeMap<i32, String>,
    objects: BTreeMap<i32, i32>,
    available: Option<[i32; 4]>,
    costs: BTreeMap<i32, [i32; 5]>,
    mana: Option<i32>,
    requirements: BTreeMap<i32, [i32; 5]>,
    counts: BTreeMap<i32, i32>,
    formation: i32,
}

impl Game {
    fn new() -> Self {
        let texts = [
            (1, "[COLOR FFFF0000]"),
            (2, "[DEFAULTCOLOR]"),
            (3, "[COLOR FF888888]"),
            (4, "[RESOURCES]"),
            (5, "[REQUIREMENTS]"),
            (7, "[CR] - "),
            (9, "[HEADHUNTERREWARD]"),
            (300, "needs:"),
            (301, "[PROGRESS] completed"),
            (1000, "Kaserne"),
            (1001, "Tower"),
            (1100, "Worker"),
            (1200, "Smithy"),
        ];
        Self {
            texts: texts.iter().map(|&(i, t)| (i, t.to_string())).collect(),
            objects: BTreeMap::new(),
            available: Some([100, 100, 0, 1]),
            costs: BTreeMap::new(),
            mana: None,
            requirements: BTreeMap::new(),
            counts: BTreeMap::new(),
            formation: -1,
        }
    }

    fn with_text(mut self, id: i32, text: &str) -> Self {
        self.texts.insert(id, text.to_string());
        self
    }
}

impl GameText for Game {
    fn player_name(&self, _: i32) -> String {
        String::new()
    }
    fn trade_cost(&self, _: TradeCost) -> i32 {
        0
    }
    fn guid_count(&self, slot: i32, guid: i32) -> i32 {
        assert_eq!(slot, 0, "object counter of the local slot");
        self.counts.get(&guid).copied().unwrap_or(0)
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
        self.texts.get(&id).cloned()
    }
    fn local_slot(&self) -> i32 {
        0
    }
    fn object_type(&self, hid: i32) -> Option<i32> {
        self.objects.get(&hid).copied()
    }
    fn available(&self, slot: i32) -> Option<[i32; 4]> {
        self.available.filter(|_| slot == 0)
    }
    fn costs(&self, _: i32, guid: i32) -> Option<[i32; 5]> {
        self.costs.get(&guid).copied()
    }
    fn first_selected_mana(&self) -> Option<i32> {
        self.mana
    }
    fn requirements(&self, guid: i32) -> Option<[i32; 5]> {
        self.requirements.get(&guid).copied()
    }
    fn formation(&self) -> i32 {
        self.formation
    }
}

struct Rig {
    vfs: TestVfs,
    assets: UiAssets,
    texts: Texts,
    fonts: FontBook,
    keys: TextContext,
}

/// `TextFont`: 10 px advance (also the space), line height 15; icon sets like `localisation.deu` (O-64).
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
            "data/icons/resourcen_icons16x16.tga",
            tga_bottom_up(64, 16, |_, _| [255; 4]),
        ),
        (
            "data/icons/info_icons16x16.tga",
            tga_bottom_up(208, 16, |_, _| [255; 4]),
        ),
    ]);
    let mut assets = UiAssets::new();
    let loc = parse_localisation(
        "Font TextFont\nFontSize 15\nTextureFont\nFontName Test\n\
Icons INFOICONS\nIconsName data\\icons\\resourcen_icons16x16.tga\nIconsSize 16 16\n\
Icons RESOURCENICONS\nIconsName data\\icons\\info_icons16x16.tga\nIconsSize 16 16\n",
    );
    let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
    Rig {
        vfs,
        assets,
        texts: Texts::from_entries(&[]),
        fonts,
        keys: TextContext {
            key_names: BTreeMap::from([(7, "B".to_string())]),
            undefined: "0x00 0x00".to_string(),
            ..TextContext::default()
        },
    }
}

impl Rig {
    fn draw(
        &mut self,
        w: &TooltipWindow,
        game: &Game,
        enhanced: bool,
    ) -> (Vec<DrawCmd>, Option<[i32; 4]>) {
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
        let rect = w.draw(&mut ctx, game, &self.keys, enhanced, &mut out);
        (out, rect)
    }

    /// Runs as `(text, color)`; icons as `<set index>`.
    fn runs(&self, w: &TooltipWindow, game: &Game, enhanced: bool) -> Vec<(String, u32)> {
        let info = self.fonts.icon_set("INFOICONS").texture;
        let res = self.fonts.icon_set("RESOURCENICONS").texture;
        let set_name = |t: TextureId| {
            if t == info {
                "INFO"
            } else if t == res {
                "RES"
            } else {
                "?"
            }
        };
        w.layout(&self.fonts, game, &self.keys, enhanced)
            .expect("Tooltip")
            .runs
            .iter()
            .map(|r| {
                let text = match &r.content {
                    RunContent::Text(codes) => codes.iter().map(|&b| char::from(b)).collect(),
                    RunContent::Icon { set, index, .. } => format!("<{} {index}>", set_name(*set)),
                };
                (text, r.color)
            })
            .collect()
    }

    fn text(&self, w: &TooltipWindow, game: &Game, enhanced: bool) -> String {
        w.text(game, &self.keys, enhanced).expect("Tooltip")
    }
}

fn req(guid: i32, kind: u32, value: f32) -> TooltipRequest {
    TooltipRequest {
        guid,
        kind,
        value,
        required: [0; 5],
    }
}

fn window(r: TooltipRequest) -> TooltipWindow {
    let mut w = TooltipWindow::new();
    w.begin_frame();
    w.set(r);
    w
}

fn s(text: &str, color: u32) -> (String, u32) {
    (text.to_string(), color)
}

#[test]
fn tooltip_lives_one_frame() {
    // O-44: The request applies from one frame start to the next; without a new `0xC` it is gone.
    let mut rig = rig();
    let game = Game::new();
    let mut w = window(req(1000, KIND_AREA, 1.0));
    let (cmds, rect) = rig.draw(&w, &game, false);
    assert!(rect.is_some() && !cmds.is_empty());
    // Drawing is read-only: a second drawing in the same frame shows the same.
    assert_eq!(rig.draw(&w, &game, false), (cmds, rect));
    w.begin_frame();
    assert_eq!(rig.draw(&w, &game, false), (Vec::new(), None));
    assert_eq!(w.text(&game, &rig.keys, false), None);
    // `0xD` clears immediately.
    w.set(req(1000, KIND_AREA, 1.0));
    w.clear();
    assert_eq!(rig.draw(&w, &game, false), (Vec::new(), None));
}

#[test]
fn last_request_wins() {
    // O-264, re/game-ui N.14.6: areas report before the buttons; the button overrides.
    let rig = rig();
    let game = Game::new().with_text(20, "Hit points");
    let mut w = window(req(20, KIND_AREA, 1.0));
    w.set(req(1000, k::MAKE, 1.0));
    assert_eq!(
        w.content(&game),
        Some(TooltipContent {
            art: TooltipArt::Costs,
            guid: 1000,
            value: 1,
            percent: 0
        })
    );
    // Without a value set `[RESOURCES]` only shows `[DEFAULTCOLOR]`.
    assert_eq!(rig.text(&w, &game, false), "Kaserne\n[DEFAULTCOLOR]");
}

#[test]
fn box_sits_left_with_bottom_h_minus_200_and_no_padding() {
    // O-242/O-264: box (0, H - 200 - h, w, H - 200) without padding, area semi-transparent 0x80000080, text
    // without shadow from the top left corner. 1024 x 768 -> bottom edge 568; "Kaserne" = 70 x 15.
    // Outline (original): the line list `alpharect2` (`0x00458DA0`, indices 0-1, 1-3, 3-2, 2-0 over the
    // corners of the area) opaque in (0, 0, 128) and **outside inclusive**: rows y = top and y = H - 200, columns
    // x = 0 and x = w; only the corner (w, H - 200) stays free (last pixel of each line missing).
    let mut rig = rig();
    let game = Game::new();
    let w = window(req(1000, KIND_AREA, 1.0));
    let (cmds, rect) = rig.draw(&w, &game, false);
    assert_eq!(rect, Some([0, 553, 70, 568]));
    let quads: Vec<([i32; 4], u32, bool)> = cmds
        .iter()
        .map(|c| {
            let DrawCmd::Quad {
                dst,
                color,
                texture,
                ..
            } = *c
            else {
                unreachable!()
            };
            (dst, color, texture.is_some())
        })
        .collect();
    assert_eq!(
        quads[..5],
        [
            ([0, 553, 70, 568], BOX_COLOR, false),
            ([0, 553, 70, 554], 0xFF00_0080, false),
            ([70, 553, 71, 568], 0xFF00_0080, false),
            ([0, 568, 70, 569], 0xFF00_0080, false),
            ([0, 553, 1, 568], 0xFF00_0080, false),
        ]
    );
    // Seven characters, no shadow; the first at (0, 553) in white.
    assert_eq!(quads.len(), 5 + 7);
    assert_eq!(quads[5], ([0, 553, 16, 569], WHITE, true));
}

#[test]
fn width_can_exceed_300_after_a_long_word() {
    // O-264: line break before the long word; afterwards it extends beyond W = 300, the box becomes that wide.
    let mut rig = rig();
    let long = "b".repeat(35);
    let game = Game::new().with_text(1300, &format!("aaaa {long}"));
    let w = window(req(1300, KIND_AREA, 1.0));
    let (_, rect) = rig.draw(&w, &game, false);
    assert_eq!(rect, Some([0, 568 - 30, 350, 568]));
}

#[test]
fn resources_turn_red_when_lacking_including_icon() {
    // re/game-ui N.8, O-243: Wood 150 with stock 100 -> `[COLOR 0xFFFF0000]` for icon and number; Gold 50 <= 100 in
    // the base color; living space against the free space (R4 - R3 = 1, supplied by `GameText::available`); Food 0 is
    // omitted.
    let rig = rig();
    let mut game = Game::new();
    game.costs.insert(1000, [50, 150, 0, 2, 9]);
    let w = window(req(1000, k::MAKE, 1.0));
    assert_eq!(
        rig.runs(&w, &game, false),
        [
            s("Kaserne", WHITE),
            s("<INFO 0>", WHITE),
            s("50 ", WHITE),
            s("<INFO 1>", RED),
            s("150 ", RED),
            s("<INFO 3>", RED),
            s("2 ", RED),
        ]
    );
    assert_eq!(
        rig.text(&w, &game, false),
        "Kaserne\n[DEFAULTCOLOR][ICON INFOICONS 0]50 [COLOR 0xFFFF0000][ICON INFOICONS 1]150 \
[COLOR 0xFFFF0000][ICON INFOICONS 3]2 [DEFAULTCOLOR]"
    );
    // BUILD takes the value of the event as factor (`0x00458C02`); MAKE does not.
    let w = window(req(1000, k::BUILD, 2.0));
    assert_eq!(rig.runs(&w, &game, false)[2], s("100 ", WHITE));
    let w = window(req(1000, k::MAKE, 2.0));
    assert_eq!(rig.runs(&w, &game, false)[2], s("50 ", WHITE));
    // Without a player in the slot the tag inserts nothing.
    game.available = None;
    let w = window(req(1000, k::MAKE, 1.0));
    assert_eq!(rig.runs(&w, &game, false), [s("Kaserne", WHITE)]);
}

#[test]
fn ability_shows_mana_of_the_first_selected_even_at_zero() {
    // O-243: GUID < 1000 shows only the mana costs (also 0) against the mana of the first selected object, icon
    // `[ICON RESOURCENICONS 4]`; the other costs do not count; without selection no row.
    let rig = rig();
    let mut game = Game::new().with_text(450, "Heal");
    game.costs.insert(450, [50, 0, 0, 0, 0]);
    game.mana = Some(0);
    let w = window(req(450, k::DO, 1.0));
    assert_eq!(
        rig.runs(&w, &game, false),
        [s("Heal", WHITE), s("<RES 4>", WHITE), s("0 ", WHITE)]
    );
    game.costs.insert(450, [0, 0, 0, 0, 10]);
    game.mana = Some(9);
    assert_eq!(
        rig.runs(&w, &game, false),
        [s("Heal", WHITE), s("<RES 4>", RED), s("10 ", RED)]
    );
    game.mana = Some(10);
    assert_eq!(rig.runs(&w, &game, false)[1], s("<RES 4>", WHITE));
    game.mana = None;
    assert_eq!(rig.runs(&w, &game, false), [s("Heal", WHITE)]);
}

#[test]
fn requirements_list_red_or_grey() {
    // re/game-ui N.8 (`0x00430790`): "needs:" and per positive prerequisite "[CR] - Name", red at object counter 0,
    // otherwise gray.
    let rig = rig();
    let mut game = Game::new();
    game.requirements.insert(1001, [1000, 0, 1200, -5, 0]);
    game.counts.insert(1200, 1);
    let w = window(req(1001, KIND_MISSING_REQUIREMENT, 0.0));
    assert_eq!(
        w.content(&game).map(|c| c.art),
        Some(TooltipArt::Requirements)
    );
    let runs = rig.runs(&w, &game, false);
    assert_eq!(
        runs,
        [
            s("Tower", WHITE),
            s("needs:", WHITE),
            s(" ", RED),
            s("- ", RED),
            s("Kaserne", RED),
            s(" ", GREY),
            s("- ", GREY),
            s("Smithy", GREY),
        ]
    );
    let layout = w.layout(&rig.fonts, &game, &rig.keys, false).unwrap();
    let ys: Vec<i32> = layout.runs.iter().map(|r| r.y).collect();
    assert_eq!(ys, [0, 15, 30, 30, 30, 45, 45, 45]);
    assert_eq!(
        rig.text(&w, &game, false),
        "Tower\nneeds:[COLOR FFFF0000][CR] - Kaserne[COLOR FF888888][CR] - Smithy[DEFAULTCOLOR]"
    );
}

#[test]
fn progress_prints_floor_percent() {
    // O-245: `floor(f * 100)` (`0x00458B96`) with switch 0x20; 0.756 -> 75, 0.29 -> 29 (product rounded to `f32`, x87
    // with 24 bits), 0 -> 0 (waiting orders report 0).
    let rig = rig();
    let game = Game::new();
    for (value, line) in [
        (0.756, "75% completed"),
        (0.29, "29% completed"),
        (0.0, "0% completed"),
    ] {
        let w = window(req(1100, KIND_PRODUCTION, value));
        assert_eq!(rig.text(&w, &game, false), format!("Worker\n{line}"));
    }
    let w = window(req(1100, KIND_PRODUCTION, 0.756));
    assert_eq!(
        rig.runs(&w, &game, false),
        [s("Worker", WHITE), s("75% ", WHITE), s("completed", WHITE)]
    );
}

#[test]
fn shortcut_tooltip_shows_302_and_formation_5_shows_516() {
    // O-241: missing texts appear as their number (`FUN_0055FFE0`): text 302 of the SHORTCUT row, text 516 of the
    // formation 5 and a button without text of its own.
    let rig = rig();
    let mut game = Game::new()
        .with_text(1300, "Group 1")
        .with_text(509, "Formation: [FORMATION]")
        .with_text(513, "Wedge");
    let w = window(req(1300, k::SHORTCUT, 1.0));
    assert_eq!(rig.text(&w, &game, false), "Group 1\n302");
    let w = window(req(509, KIND_AREA, 1.0));
    game.formation = 5;
    assert_eq!(rig.text(&w, &game, false), "Formation: 516");
    game.formation = 2;
    assert_eq!(rig.text(&w, &game, false), "Formation: Wedge");
    game.formation = 6;
    assert_eq!(rig.text(&w, &game, false), "Formation: ");
    let w = window(req(1999, KIND_AREA, 1.0));
    assert_eq!(rig.text(&w, &game, false), "1999");
}

#[test]
fn enhanced_text_only_with_the_switch() {
    // re/game-ui N.8: text GUID + 10000 only with `EnhancedTooltips`, in 0xFFFFFF96 and only if it exists (no number).
    let rig = rig();
    let game = Game::new().with_text(11000, "Trains men.");
    let w = window(req(1000, KIND_AREA, 1.0));
    assert_eq!(rig.runs(&w, &game, false), [s("Kaserne", WHITE)]);
    assert_eq!(
        rig.runs(&w, &game, true),
        [
            s("Kaserne", WHITE),
            s("Trains ", ENHANCED_COLOR),
            s("men.", ENHANCED_COLOR)
        ]
    );
    assert_eq!(rig.text(&w, &game, true), "Kaserne\nTrains men.");
    let w = window(req(1001, KIND_AREA, 1.0));
    assert_eq!(rig.text(&w, &game, true), "Tower");
}

#[test]
fn keyid_uses_the_key_map() {
    // re/game-ui N.14.6, O-113: `[KEYID n]` with switch 0x200 from the key layout of the host; without binding
    // "(0x00 0x00)".
    let rig = rig();
    let game = Game::new()
        .with_text(1000, "Kaserne [KEYID 7]")
        .with_text(1001, "Tower [KEYID 8]");
    let w = window(req(1000, KIND_AREA, 1.0));
    assert_eq!(rig.text(&w, &game, false), "Kaserne (B)");
    assert_eq!(
        rig.runs(&w, &game, false),
        [s("Kaserne ", WHITE), s("(B)", WHITE)]
    );
    let w = window(req(1001, KIND_AREA, 1.0));
    assert_eq!(rig.text(&w, &game, false), "Tower (0x00 0x00)");
}

#[test]
fn without_game_values_the_window_shows_numbers() {
    // With the defaults of `GameText` (no texts, no values) no tag inserts anything; name and additional line appear
    // as a number (O-241). The tags themselves are checked by `text::tests::without_game_values_tags_stay_empty`.
    struct Bare;
    impl GameText for Bare {
        fn player_name(&self, _: i32) -> String {
            String::new()
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
    let rig = rig();
    for kind in [
        k::MAKE,
        KIND_MISSING_REQUIREMENT,
        k::SELECTHEADHUNTER,
        k::COUNTERUPGRADE,
    ] {
        let w = window(req(1000, kind, 1.0));
        let text = w.text(&Bare, &rig.keys, true).unwrap();
        // Section 2 is the number of the additional line (4, 5, 9); no tag is inserted.
        let extra = w.content(&Bare).unwrap().art.extra().unwrap().0;
        assert_eq!(text, format!("1000\n{extra}"), "{kind:#x}");
    }
    // Selection buttons without object: no tooltip (kind stays 0).
    let w = window(req(77, KIND_SELECTION, 1.0));
    assert_eq!(w.content(&Bare), None);
}

#[test]
fn kinds_map_to_the_tooltip_arts() {
    // `0x00458A80`: kind by `kind & 0xFF000000`; selection, occupant and hero show the type of the object.
    let mut game = Game::new();
    game.objects.insert(77, 1100);
    let art = |kind: u32| {
        window(req(77, kind, 1.0))
            .content(&game)
            .map(|c| (c.art.code(), c.guid))
    };
    for (kind, code) in [
        (k::BUILD, 1),
        (k::MAKE, 1),
        (k::UPGRADE, 1),
        (k::ONLYONCE, 1),
        (k::MAKEONLY_ONCE, 1),
        (KIND_MISSING_REQUIREMENT, 2),
        (KIND_PRODUCTION, 4),
        (0xC00_0000, 5),
        (KIND_AREA, 5),
        (0xA00_0004, 5),
        (0, 5),
        (k::SELECTHEADHUNTER, 7),
        (0x1600_0000, 7),
        (k::COUNTERUPGRADE, 8),
        (k::DO, 9),
        (k::TARGET, 9),
        (k::TARGET_DO, 9),
        (k::SHORTCUT, 10),
    ] {
        assert_eq!(art(kind), Some((code, 77)), "{kind:#x}");
    }
    for kind in [KIND_SELECTION, 0xB00_0000, KIND_HERO] {
        assert_eq!(art(kind), Some((3, 1100)), "{kind:#x}");
    }
    assert_eq!(
        window(req(78, KIND_HERO, 1.0)).content(&game),
        None,
        "object missing"
    );
    // Kind 3 shows the type name without an additional line.
    let rig = rig();
    let w = window(req(77, KIND_SELECTION, 1.0));
    assert_eq!(rig.text(&w, &game, false), "Worker");
    // 602 (locked counter-upgrade) runs as COUNTERUPGRADE: name 602 and `[RESOURCES]`.
    let game = game.with_text(602, "Counter-upgrade is currently running");
    let w = window(req(602, k::COUNTERUPGRADE, 0.0));
    assert_eq!(
        rig.text(&w, &game, false),
        "Counter-upgrade is currently running\n[DEFAULTCOLOR]"
    );
}
