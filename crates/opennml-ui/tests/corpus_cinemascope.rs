//! Corpus test of the cinemascope area against the original installation (`NML_GAME_DIR`, without an
//! installation it silently skips itself).

use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_ui::assets::{Texts, UiAssets, load_localisation};
use opennml_ui::coords::Screen;
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::{DrawCmd, Filter, draw_checksum};
use opennml_ui::fonts::FontBook;
use opennml_ui::hud::cinemascope::{CinemascopeData, CinemascopeView};
use opennml_ui::hud::{HudEvent, HudEventKind};
use opennml_ui::text::{GameTags, GameText, TradeCost};

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

/// Measured on the German edition with the patch; if a value changes, look for the cause.
/// (Scene, checksum at 1024 x 768, checksum at 1280 x 1024; `draw_checksum`.)
const CINEMASCOPE_DRAW_GOLDEN: [(&str, u64, u64); 3] = [
    ("bars", 0x8056834008350AFD, 0x38F12F34C1F482FA),
    ("intro", 0x040810217551D509, 0xE4DF3BBFFE4C072D),
    ("tip3", 0xB7D0C86B0A1E7E54, 0xCCDAAF657AF342EB),
];

/// Game values without a game: all counters 0, no names.
struct NoValues;

impl GameText for NoValues {
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

/// The three scenes after 500 ms of fading:
/// - `bars`: only bars and edge tiles;
/// - `intro`: bars, portrait 1000 with frame 101 in slot 1, dialog line 20002 in slot 1;
/// - `tip3`: tip 20100 in slot 3 without bars.
fn scenes(data: &CinemascopeData) -> Vec<(&'static str, CinemascopeView)> {
    let send = |view: &mut CinemascopeView, kind, args: &[i32]| {
        assert!(view.message(&HudEvent::with_args(kind, args), data));
    };
    let mut bars = CinemascopeView::new();
    send(&mut bars, HudEventKind::Cinemascope, &[128]);
    let mut intro = bars.clone();
    send(
        &mut intro,
        HudEventKind::PortraitShow,
        &[1, 101, -1, 1000, -1],
    );
    send(&mut intro, HudEventKind::TextShow, &[1, 20002, -1]);
    let mut tip3 = CinemascopeView::new();
    send(&mut tip3, HudEventKind::TextShow, &[3, 20100, -1]);
    let mut out = vec![("bars", bars), ("intro", intro), ("tip3", tip3)];
    for (_, view) in &mut out {
        view.tick(500.0);
    }
    out
}

/// Draws every scene at `size`; fonts once per size. Returns (scene, commands) and the image collection.
fn draw_all(
    vfs: &Vfs,
    texts: &Texts,
    data: &CinemascopeData,
    size: Screen,
) -> (Vec<(&'static str, Vec<DrawCmd>)>, UiAssets) {
    let loc = load_localisation(vfs, "DEU").expect("localisation loads");
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(vfs, &loc, size.width, &mut assets, None);
    let values = NoValues;
    let tags = GameTags::new(&values, 0);
    let mut out = Vec::new();
    for (name, view) in scenes(data) {
        let mut ctx = UiContext {
            vfs,
            assets: &mut assets,
            fonts: &fonts,
            texts,
            screen: size,
        };
        let mut cmds = Vec::new();
        view.draw(data, &mut ctx, &tags, &mut cmds);
        out.push((name, cmds));
    }
    (out, assets)
}

/// The scenes draw with the data of the installation without missing images; the command lists are stable (drawn
/// twice the same) and, on the German edition, equal to the measured checksums.
// QUIRK O-351: kept
#[test]
fn cinemascope_draw_checksums_are_stable() {
    let Some(dir) = corpus_game_dir() else { return };
    let vfs = Vfs::open_game_dir(dir).expect("installation cannot be opened");
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let loc = load_localisation(&vfs, "DEU").expect("localisation loads");
    let texts = Texts::load(&vfs, &loc);
    let data = CinemascopeData::load(&vfs);
    assert!(data.area.is_some(), "DIALOGAREA 0 is missing");
    assert!(data.portraits.contains_key(&1000) && data.frames.contains_key(&101));
    assert!(texts.get(20002).is_some() && texts.get(20100).is_some());

    let mut sums = Vec::new();
    for size in SIZES {
        let (drawn, assets) = draw_all(&vfs, &texts, &data, size);
        // Frame and portrait of the scene `intro` are loaded: the two LINEAR quads (`billboardalpha`).
        let images = drawn[1]
            .1
            .iter()
            .filter(|c| {
                matches!(c, DrawCmd::Quad { texture, filter, .. }
                    if texture.is_some() && *filter == Filter::Linear)
            })
            .count();
        assert_eq!(images, 2, "{size:?}: frame and portrait");
        for (name, cmds) in &drawn {
            assert!(!cmds.is_empty(), "{name} at {size:?}: no commands");
        }
        let (again, assets2) = draw_all(&vfs, &texts, &data, size);
        let sum = |d: &[(&str, Vec<DrawCmd>)], a: &UiAssets| -> Vec<u64> {
            d.iter().map(|(_, c)| draw_checksum(c, a)).collect()
        };
        assert_eq!(
            sum(&drawn, &assets),
            sum(&again, &assets2),
            "{size:?}: differs when drawn twice"
        );
        sums.push(sum(&drawn, &assets));
    }
    let got: Vec<(&str, u64, u64)> = CINEMASCOPE_DRAW_GOLDEN
        .iter()
        .enumerate()
        .map(|(i, &(name, ..))| (name, sums[0][i], sums[1][i]))
        .collect();
    if version == GameVersion::German && got.as_slice() != CINEMASCOPE_DRAW_GOLDEN {
        let table: String = got
            .iter()
            .map(|(n, a, b)| format!("    (\"{n}\", 0x{a:016X}, 0x{b:016X}),\n"))
            .collect();
        panic!("checksums differ; measured:\n{table}");
    }
}
