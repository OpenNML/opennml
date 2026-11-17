//! Reads the sound database and the audio configuration of the original installation and checks the
//! counts of the shipped data from `docs/formats/sounddatabase.md`. Skipped if the installation
//! is missing.

use std::collections::BTreeSet;

use nml_formats::sound::{
    MUSIC_CONFIG_PATH, OTHER_EVENTS_PATH, SOUND_DATABASE_PATH, SOUND_TXT_PATH, parse_id_list,
    parse_music_config, parse_other_events, parse_sound_database, parse_sound_guid_list,
    parse_sound_txt,
};
use nml_formats::text::decode_cp1252;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use nml_rda::{Vfs, normalize_path};

fn read(vfs: &Vfs, path: &str) -> String {
    decode_cp1252(&vfs.read(path).unwrap())
}

/// Paths in `soundguidlist.txt` without a file in the shipped data (normalized, sorted), in both editions.
const MISSING_SFX: [&str; 12] = [
    "data/sfx/gebäude auswählen/eng_warenkontor.wav",
    "data/sfx/gebäude auswählen/wid_pferdekoppel.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_01.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_02.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_03.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_04.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_05.wav",
    "data/sfx/gebäude bauen/son_gebaeude_bauen_06.wav",
    "data/sfx/tier auswählen/son_galoppieren_01.wav",
    "data/sfx/tier auswählen/son_galoppieren_02.wav",
    "data/sfx/tier stirbt/tie_adler_tot.wav",
    "data/sfx/tier stirbt/tie_schneeziege_tot.wav",
];

/// Additionally missing hero voice lines of the English edition; the German patch (`NML4.RDA`) provides them.
const MISSING_HEROES_ENGLISH: [&str; 11] = [
    "data/sprache/helden/pat_made_attack_013.wav",
    "data/sprache/helden/pat_made_attack_06.wav",
    "data/sprache/helden/spa_mend_attack_02.wav",
    "data/sprache/helden/spa_mend_attack_03.wav",
    "data/sprache/helden/spa_mend_attack_04.wav",
    "data/sprache/helden/spa_mend_attack_05.wav",
    "data/sprache/helden/spa_mend_attack_06.wav",
    "data/sprache/helden/wid_tegl_select_01.wav",
    "data/sprache/helden/wid_tegl_select_06.wav",
    "data/sprache/helden/wid_umak_attack_06.wav",
    "data/sprache/helden/wid_umak_select_06.wav",
];

/// Sound GUIDs from database lists whose file is missing (the game then plays nothing), in both editions.
const MISSING_USED: [i32; 1] = [145];

/// Additionally in the English edition: the GUIDs of the missing hero voice lines.
const MISSING_USED_HEROES_ENGLISH: [i32; 10] =
    [931, 1018, 1019, 1020, 1021, 1022, 1047, 1087, 1181, 1186];

#[test]
fn sound_database_resolves_except_the_known_missing_files() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let db = parse_sound_database(&read(&vfs, SOUND_DATABASE_PATH));
    let list_path = db.sound_guid_list.clone().unwrap();
    let sounds = parse_sound_guid_list(&read(&vfs, &list_path));
    let actions = parse_id_list(&read(&vfs, db.action_id_list.as_deref().unwrap()));
    assert_eq!(db.objects.len(), 314);
    assert_eq!(sounds.len(), 1182);
    let used: BTreeSet<i32> = db
        .objects
        .values()
        .flat_map(|a| a.values().flatten().copied())
        .collect();
    assert_eq!(used.len(), 1098);
    assert!(used.iter().all(|g| sounds.contains_key(g)));
    let empty = db
        .objects
        .iter()
        .flat_map(|(g, a)| a.iter().map(move |(k, v)| (*g, *k, v.len())))
        .filter(|&(_, _, n)| n == 0)
        .map(|(g, a, _)| (g, a))
        .collect::<Vec<_>>();
    assert_eq!(empty, [(1306, 100), (1313, 110)]);
    let all_actions: BTreeSet<i32> = db
        .objects
        .values()
        .flat_map(|a| a.keys())
        .copied()
        .collect();
    assert!(all_actions.iter().all(|a| actions.contains_key(a)));
    let mut missing: Vec<String> = sounds
        .values()
        .filter(|e| !vfs.exists(&e.path))
        .map(|e| normalize_path(&e.path))
        .collect();
    missing.sort();
    let mut expected: Vec<&str> = MISSING_SFX.to_vec();
    let mut expected_used: Vec<i32> = MISSING_USED.to_vec();
    match game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) {
        GameVersion::English => {
            expected.extend(MISSING_HEROES_ENGLISH);
            expected_used.extend(MISSING_USED_HEROES_ENGLISH);
        }
        GameVersion::German => {}
    }
    assert_eq!(missing, expected);
    let missing_used: Vec<i32> = used
        .iter()
        .copied()
        .filter(|g| !vfs.exists(&sounds[g].path))
        .collect();
    assert_eq!(missing_used, expected_used);
    // Volume factors: 0.2 for surf and "Building burns".
    assert_eq!(sounds.values().filter(|e| e.factor == 1.0).count(), 1152);
    assert_eq!(sounds.values().filter(|e| e.factor == 0.2).count(), 9);
}

#[test]
fn events_music_and_script_tracks_exist() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let events = parse_other_events(&read(&vfs, OTHER_EVENTS_PATH));
    assert_eq!(events.len(), 19);
    assert_eq!(events["SFX_REGEN"], 895);
    let music = parse_music_config(&read(&vfs, MUSIC_CONFIG_PATH));
    assert!(music.playlists.iter().all(|p| p.len() == 1));
    assert!(music.attack.iter().all(Option::is_some));
    assert_eq!(
        [
            music.attack_start_points,
            music.attack_start_time,
            music.attack_stop_points,
            music.attack_stop_time,
            music.attack_pause,
            music.attack_max_time
        ],
        [500, 1000, 0, 5000, 10000, 720000]
    );
    let tracks = parse_sound_txt(&read(&vfs, SOUND_TXT_PATH));
    assert_eq!(tracks.len(), 21);
    assert_eq!(
        tracks.keys().copied().collect::<Vec<_>>(),
        (100..=120).collect::<Vec<_>>()
    );
    // Music is stored as loose files in the game directory (`DATA/MUSIC`), not in the archives.
    let paths = music
        .playlists
        .iter()
        .flatten()
        .chain(music.attack.iter().flatten())
        .chain(tracks.values());
    for path in paths {
        assert!(vfs.loose_file(path).is_some(), "{path}");
        assert!(vfs.get(path).is_none(), "{path} in the archive");
    }
}
