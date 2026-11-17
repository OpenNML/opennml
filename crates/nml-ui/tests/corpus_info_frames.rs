//! Corpus test of the info windows 168 and 213 against the original installation (`NML_GAME_DIR`, without an
//! installation it silently skips itself).

use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use nml_ui::assets::{Texts, UiAssets, load_localisation};
use nml_ui::coords::Screen;
use nml_ui::dialog::UiContext;
use nml_ui::draw::{DrawCmd, Filter, draw_checksum};
use nml_ui::fonts::FontBook;
use nml_ui::hud::cinemascope::CinemascopeData;
use nml_ui::hud::info_frame::{InfoFrameKind, InfoFrameParams, InfoFrames};
use nml_ui::text::{GameTags, GameText, TradeCost};

const SIZES: [Screen; 2] = [
    Screen {
        width: 1024,
        height: 768,
    },
    Screen {
        width: 1280,
        height: 1024,
    },
];

/// Measured; compared **only on the German edition** with the patch (the English one has different texts and thus
/// different sizes). If a value changes, look for the cause.
/// (Scene, checksum at 1024 x 768, checksum at 1280 x 1024; `draw_checksum`.)
const INFO_FRAME_DRAW_GOLDEN: [(&str, u64, u64); 2] = [
    ("info168", 0x573E581F844469E5, 0x2CFA4BD49B0D7B6C),
    ("speech213", 0xA0A13A94AFBE9502, 0x552D7E5F35B6EF55),
];

/// Text of the scene `info168`: the only 168 in `kamp1-5.abl` (`20950; 1250; 35; 1`), contains `[GUIDCOUNTER 3 1401]`.
const INFO168_TEXT: i32 = 20950;
/// Fixed counter value of the scene `info168`.
const COUNTER: i32 = 7;

/// Game values without a game: the object counter fixed, otherwise 0 and no names.
struct FixedValues;

impl GameText for FixedValues {
    fn player_name(&self, _: i32) -> String {
        String::new()
    }
    fn trade_cost(&self, _: TradeCost) -> i32 {
        0
    }
    fn guid_count(&self, _: i32, _: i32) -> i32 {
        COUNTER
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

/// The two scenes with one open window each:
/// - `info168`: text 20950 at (1250, 35), flag 1 (right edge);
/// - `speech213`: pos 1, portrait 1000, frame 101, text 20002.
fn scenes() -> Vec<(&'static str, InfoFrames)> {
    let open = |kind: InfoFrameKind, text: i32| {
        let mut frames = InfoFrames::new();
        frames.open(text, InfoFrameParams::of(&kind, text));
        frames
    };
    vec![
        (
            "info168",
            open(
                InfoFrameKind::Window {
                    x: 1250,
                    y: 35,
                    style: 1,
                },
                INFO168_TEXT,
            ),
        ),
        (
            "speech213",
            open(
                InfoFrameKind::Speech {
                    slot: 1,
                    portrait: 1000,
                    frame: 101,
                },
                20002,
            ),
        ),
    ]
}

/// A drawn scene: name, rectangle in the grid, commands of the first and the second frame.
type Drawn = (&'static str, [i32; 4], Vec<DrawCmd>, Vec<DrawCmd>);

/// Draws every scene at `size` twice (the second frame with the window already measured); fonts once per size.
/// Returns (scene, rectangle, commands of the first and the second frame) and the image collection.
fn draw_all(
    vfs: &Vfs,
    texts: &Texts,
    data: &CinemascopeData,
    size: Screen,
) -> (Vec<Drawn>, UiAssets) {
    let loc = load_localisation(vfs).expect("localisation loads");
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(vfs, &loc, size.width, &mut assets, None);
    let values = FixedValues;
    let tags = GameTags::new(&values, 0);
    let mut out = Vec::new();
    for (name, frames) in scenes() {
        let mut ctx = UiContext {
            vfs,
            assets: &mut assets,
            fonts: &fonts,
            texts,
            screen: size,
        };
        let mut first = Vec::new();
        frames.draw(data, &mut ctx, &tags, &mut first);
        let mut second = Vec::new();
        frames.draw(data, &mut ctx, &tags, &mut second);
        let rects = frames.rects();
        assert_eq!(rects.len(), 1, "{name}");
        out.push((name, rects[0].1, first, second));
    }
    (out, assets)
}

/// The scenes draw with the data of the installation; background, frame and portrait are loaded, the position follows
/// re/campaign-ui D6.5, the command lists are stable (drawn twice the same) and, on the German edition, equal to the
/// measured checksums.
// QUIRK O-358, O-359: kept
#[test]
fn info_frame_draw_checksums_are_stable() {
    let Some(dir) = corpus_game_dir() else { return };
    let vfs = Vfs::open_game_dir(dir).expect("installation cannot be opened");
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let loc = load_localisation(&vfs).expect("localisation loads");
    let texts = Texts::load(&vfs, &loc);
    let data = CinemascopeData::load(&vfs);
    assert!(data.portraits.contains_key(&1000) && data.frames.contains_key(&101));
    assert!(texts.get(20002).is_some());
    let info = texts.get(INFO168_TEXT).expect("text 20950 is missing");
    assert!(info.contains("[GUIDCOUNTER 3 1401]"), "{info}");

    let mut sums = Vec::new();
    for size in SIZES {
        let (drawn, assets) = draw_all(&vfs, &texts, &data, size);
        assert!(assets.missing().is_empty(), "{:?}", assets.missing());
        let [info168, speech213] = [&drawn[0], &drawn[1]];
        // 168 with flag 1 at the right edge, position y = 35; 213 at pos 1: 630 wide at (650, 40), at least 160 high.
        let [_, y, r, _] = info168.1;
        assert_eq!((y, r), (35, 1280), "{size:?}");
        let [l, t, r, b] = speech213.1;
        assert_eq!([l, t, r], [650, 40, 1280], "{size:?}");
        assert!(b - t >= 160, "{size:?}: {b}");
        // Frame and portrait of the speech window: the two LINEAR quads with full UV (`billboardalpha`).
        let images = speech213
            .2
            .iter()
            .filter(|c| {
                matches!(c, DrawCmd::Quad { texture, uv, filter, .. }
                    if texture.is_some() && *filter == Filter::Linear && *uv == [0.0, 0.0, 1.0, 1.0])
            })
            .count();
        assert_eq!(images, 2, "{size:?}: frame and portrait");
        for (name, _, first, second) in &drawn {
            assert!(!first.is_empty(), "{name} at {size:?}: no commands");
            assert_eq!(first, second, "{name} at {size:?}: second frame differs");
        }
        let (again, assets2) = draw_all(&vfs, &texts, &data, size);
        let sum = |d: &[Drawn], a: &UiAssets| -> Vec<u64> {
            d.iter().map(|(_, _, c, _)| draw_checksum(c, a)).collect()
        };
        assert_eq!(
            sum(&drawn, &assets),
            sum(&again, &assets2),
            "{size:?}: differs when drawn twice"
        );
        sums.push(sum(&drawn, &assets));
    }
    let got: Vec<(&str, u64, u64)> = INFO_FRAME_DRAW_GOLDEN
        .iter()
        .enumerate()
        .map(|(i, &(name, ..))| (name, sums[0][i], sums[1][i]))
        .collect();
    if version == GameVersion::German && got.as_slice() != INFO_FRAME_DRAW_GOLDEN {
        let table: String = got
            .iter()
            .map(|(n, a, b)| format!("    (\"{n}\", 0x{a:016X}, 0x{b:016X}),\n"))
            .collect();
        panic!("checksums differ; measured:\n{table}");
    }
}
