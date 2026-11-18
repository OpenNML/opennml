//! Decodes all .tga and parses all .fnt of the original installation.

use opennml_formats::fnt::parse_fnt;
use opennml_formats::text::decode_cp1252;
use opennml_formats::tga::{decode_tga, decode_tga_game};
use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;

#[test]
fn all_textures_and_fonts_load() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let (mut tga, mut fnt) = (0, 0);
    let mut flipped = Vec::new();
    let mut failures = Vec::new();
    for (archive, entry) in vfs.files() {
        let lower = entry.path.to_lowercase();
        if lower.ends_with(".tga") {
            let bytes = archive.read(entry).unwrap();
            match (decode_tga(&bytes), decode_tga_game(&bytes)) {
                (Ok(img), Ok(game)) if img.width() > 0 && img.height() > 0 => {
                    tga += 1;
                    // Files stored bottom-left yield the same image; files stored top-left are displayed
                    // upside down by the game (origin bit ignored).
                    if img != game {
                        flipped.push(entry.path.clone());
                        assert_eq!(image::imageops::flip_vertical(&img), game, "{}", entry.path);
                    }
                }
                (Ok(_), Ok(_)) => failures.push(format!("{}: empty image", entry.path)),
                (Err(e), _) | (_, Err(e)) => failures.push(format!("{}: {e}", entry.path)),
            }
        } else if lower.ends_with(".fnt") {
            match parse_fnt(&decode_cp1252(&archive.read(entry).unwrap())) {
                Ok(font) if !font.glyphs.is_empty() => fnt += 1,
                Ok(_) => failures.push(format!("{}: no characters", entry.path)),
                Err(e) => failures.push(format!("{}: {e}", entry.path)),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} errors:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
    assert!(tga > 1000, "only {tga} textures");
    assert_eq!(fnt, 63);
    // In the shipped data: three effect textures (`data/effects/neu/...`) are stored top-left.
    assert_eq!(flipped.len(), 3, "{flipped:?}");
    assert!(
        flipped
            .iter()
            .all(|p| p.to_lowercase().starts_with("data/effects/")),
        "{flipped:?}"
    );
}
