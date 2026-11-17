//! Reads all `.pef`, `.hef` and the `effectlist.ini` of the original installation and checks the shipped-data
//! counts from `docs/formats/pef.md` and `docs/formats/hef.md`. Skipped if the installation is missing.

use nml_formats::hef::{EFFECT_LIST_PATH, parse_effect_list, parse_hef};
use nml_formats::pef::{PefComponent, parse_pef};
use nml_formats::text::decode_cp1252;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};

/// All files with the extension `suffix` (normalized, sorted) and their text.
fn files(vfs: &Vfs, suffix: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(suffix))
        .map(|p| {
            let text = decode_cp1252(&vfs.read(&p).unwrap());
            (p, text)
        })
        .collect();
    files.sort();
    files.dedup_by(|a, b| a.0 == b.0);
    files
}

#[test]
fn all_pef_files_parse_with_the_documented_counts() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let pefs = files(&vfs, ".pef");
    assert_eq!(pefs.len(), 127);
    let mut totals = [0; 3];
    let (mut layers, mut directional, mut sized, mut sequences) = (0, 0, 0, 0);
    let (mut move_with_object, mut ring, mut untextured) = (0, 0, 0);
    for (path, text) in &pefs {
        let pef = parse_pef(text);
        let counts = pef.counts();
        // As many components per file as block keys in the text.
        let expected = [
            text.matches("PARTICLEEFFECT").count(),
            text.matches("BILLBOARDEFFECT").count(),
            text.matches("OBJECTEFFECT").count(),
        ];
        assert_eq!(counts, expected, "{path}");
        assert_eq!(pef.movement_time, 1.0, "{path}");
        for k in 0..3 {
            totals[k] += counts[k];
        }
        for component in &pef.components {
            match component {
                PefComponent::Particles(p) => {
                    untextured += usize::from(p.texture.is_empty());
                    assert!(p.life > 0.0 && p.emit > 0, "{path}");
                    move_with_object += usize::from(p.move_with_object);
                    ring += usize::from(p.emitter_type == 2);
                }
                PefComponent::Billboard(b) => {
                    for l in &b.layers {
                        layers += 1;
                        directional += usize::from(l.directional);
                        sized += usize::from(l.size.is_some());
                        sequences += usize::from(l.textures.len() == 20);
                        assert!(l.textures.iter().all(Option::is_some), "{path}");
                        assert!(l.duration > 0.0, "{path}");
                        // All curves are read: finite values.
                        for c in [&l.x_scale, &l.y_scale, &l.fading, &l.rotation] {
                            assert!(c.values.iter().all(|v| v.is_finite()), "{path}");
                        }
                    }
                }
                PefComponent::Object(o) => {
                    assert_eq!(o.object, "data\\r3d\\misc\\sonstiges\\strahl\\strahl01.rdo");
                }
            }
        }
    }
    assert_eq!(totals, [186, 89, 2]);
    assert_eq!((layers, directional, sized, sequences), (185, 96, 181, 2));
    assert_eq!((move_with_object, ring), (31, 102));
    // `TEXTURE ""` in `schatten-kampfsicht.pef`, `units-kampfsicht.pef`, `units-panzerung.pef`.
    assert_eq!(untextured, 3);
}

#[test]
fn all_hef_files_and_the_effect_list_parse() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let hefs = files(&vfs, ".hef");
    assert_eq!(hefs.len(), 24);
    let records: Vec<_> = hefs.iter().flat_map(|(_, t)| parse_hef(t)).collect();
    assert_eq!(records.len(), 48);
    assert!(records.iter().all(|r| r.arc && r.effect.ends_with(".pef")));
    assert_eq!(records.iter().filter(|r| r.half_vision_range).count(), 32);
    assert_eq!(records.iter().filter(|r| r.center).count(), 16);
    assert_eq!(records.iter().filter(|r| r.count_mode).count(), 16);
    let list = parse_effect_list(&decode_cp1252(&vfs.read(EFFECT_LIST_PATH).unwrap()));
    assert_eq!(list.len(), 25);
    assert_eq!(list[0].node, "Feuer");
    assert_eq!(list[0].effect, "data/effects/feuer.pef");
    let buechse = list.iter().find(|e| e.node == "Feuer_Buechse").unwrap();
    assert_eq!(buechse.scale, 6.0);
}
