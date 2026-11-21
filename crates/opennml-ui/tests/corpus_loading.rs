//! Corpus test of the loading screen against the original installation (`NML_GAME_DIR`, without an
//! installation it silently skips itself).

use opennml_formats::ldg::parse_ldg;
use opennml_formats::text::decode_cp1252;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_rda::{Vfs, normalize_path};
use opennml_ui::assets::{Texts, UiAssets, load_localisation};
use opennml_ui::coords::Screen;
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::draw_checksum;
use opennml_ui::fonts::FontBook;
use opennml_ui::loading::{DEFAULT_LDG, LoadingScreen, MULTIPLAYER_LDG, ldg_path_for};

const PHASES: [u8; 4] = [0, 40, 70, 100];
const SIZES: [(i32, i32); 2] = [(1024, 768), (1600, 900)];

/// Measured on the German edition with the patch; if a value changes, look for the cause. Re-measured when the bar
/// began to blend additively (O-194; own kind byte in `draw_checksum`).
/// Per file (`kamp1-1.ldg`, `multiplayer.ldg`) and size (1024 x 768, 1600 x 900) the checksums
/// (`draw_checksum`) at 0/40/70/100 %.
const LOADING_DRAW_GOLDEN_GERMAN: &[(&str, (i32, i32), [u64; 4])] = &[
    (
        "kamp1-1.ldg",
        (1024, 768),
        [
            0x5197070D2CBDDC97,
            0x4B50B047B500FCD3,
            0x9F9FA34D8EA6A993,
            0x80A1F2312BABD7E0,
        ],
    ),
    (
        "kamp1-1.ldg",
        (1600, 900),
        [
            0xFCFE31E44C05AD7F,
            0x8434F9FE63C2DF66,
            0x1233D8EBD659FF8F,
            0x93C23C5107A4A040,
        ],
    ),
    (
        "multiplayer.ldg",
        (1024, 768),
        [
            0x89481FF548BF2D70,
            0x16E4F30B01203E6C,
            0xC54FC820DBEB1FB0,
            0x2FECE68517213AAF,
        ],
    ),
    (
        "multiplayer.ldg",
        (1600, 900),
        [
            0xF4975E8A0E1553A3,
            0x191FBA241AE9BB8E,
            0x2B19D6FED9D52913,
            0x1D7B27D33911C164,
        ],
    ),
];

fn sums(
    vfs: &Vfs,
    loc: &opennml_formats::localisation::Localisation,
    texts: &Texts,
    ldg_path: &str,
    size: (i32, i32),
) -> [u64; 4] {
    let screen = Screen {
        width: size.0,
        height: size.1,
    };
    let mut assets = UiAssets::new();
    let fonts = FontBook::load(vfs, loc, screen.width, &mut assets, None);
    let text = decode_cp1252(
        &vfs.read(ldg_path)
            .unwrap_or_else(|e| panic!("{ldg_path}: {e}")),
    );
    let ldg = parse_ldg(&text);
    let mut ctx = UiContext {
        vfs,
        assets: &mut assets,
        fonts: &fonts,
        texts,
        screen,
    };
    let loading = LoadingScreen::new(ldg, &mut ctx);
    let mut out = [0u64; 4];
    for (slot, &percent) in out.iter_mut().zip(&PHASES) {
        let cmds = loading.draw(percent, &ctx);
        assert_eq!(
            cmds,
            loading.draw(percent, &ctx),
            "{ldg_path}: differs when drawn twice"
        );
        *slot = draw_checksum(&cmds, ctx.assets);
    }
    assert!(
        assets.missing().is_empty(),
        "{ldg_path}: {:?}",
        assets.missing()
    );
    out
}

/// All 38 `.ldg` draw without a panic and without missing images; `kamp1-1.ldg` and `multiplayer.ldg` have
/// stable checksums (per edition).
// QUIRK O-138, O-139: kept
#[test]
fn loading_screens_draw_stable() {
    let Some(dir) = corpus_game_dir() else { return };
    let vfs = Vfs::open_game_dir(dir).expect("installation cannot be opened");
    let loc = load_localisation(&vfs, "DEU").expect("localisation loads");
    let texts = Texts::load(&vfs, &loc);
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));

    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.ends_with(".ldg"))
        .collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 38);
    for path in &paths {
        // The rectangle of the bar grows only in x: the four phases differ if the image exists.
        let s = sums(&vfs, &loc, &texts, path, SIZES[0]);
        assert_ne!(s[0], s[3], "{path}: bar does not change");
    }

    // File choice as in the game: campaign levels have their `.ldg`, a multiplayer map does not (fallback
    // `multiplayer.ldg`, when loading `default.ldg`).
    let exists = |p: &str| vfs.exists(p);
    let kamp = ldg_path_for(
        "data/levels/kampagne1/kamp1-1/kamp1-1.lvl",
        -1,
        false,
        exists,
    );
    assert_eq!(kamp, "data/levels/kampagne1/kamp1-1/kamp1-1.ldg");
    let mp_level = vfs
        .files()
        .map(|(_, entry)| normalize_path(&entry.path))
        .filter(|p| p.starts_with("data/levels/multiplayer") && p.ends_with(".lvl"))
        .min()
        .expect("no multiplayer map");
    assert_eq!(ldg_path_for(&mp_level, 0, false, exists), MULTIPLAYER_LDG);
    assert_eq!(ldg_path_for(&mp_level, 0, true, exists), DEFAULT_LDG);
    assert!(vfs.exists(DEFAULT_LDG) && vfs.exists(MULTIPLAYER_LDG));

    let mut got = Vec::new();
    for (name, path) in [
        ("kamp1-1.ldg", kamp.as_str()),
        ("multiplayer.ldg", MULTIPLAYER_LDG),
    ] {
        for size in SIZES {
            got.push((name, size, sums(&vfs, &loc, &texts, path, size)));
        }
    }
    if version == GameVersion::German && got.as_slice() != LOADING_DRAW_GOLDEN_GERMAN {
        let table: String = got
            .iter()
            .map(|(n, s, h)| {
                format!(
                    "    ({n:?}, {s:?}, [0x{:016X}, 0x{:016X}, 0x{:016X}, 0x{:016X}]),\n",
                    h[0], h[1], h[2], h[3]
                )
            })
            .collect();
        panic!("checksums differ; measured:\n{table}");
    }
}
