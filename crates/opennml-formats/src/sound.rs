//! Sound database and audio configuration, see `docs/formats/sounddatabase.md`:
//! `sounddatabase.dat` (`SoundDatabase_Load` `0x005501C0`), `soundguidlist.txt` and
//! `actionidlist.txt` (`0x0054FEE0`, `0x00550050`), `otherevents.ini`
//! (`SoundManager_LoadOtherEvents` `0x005514B0`), `musicconfig.ini` (`MusicManager_ParseConfig`
//! `0x004E6DD0`) and `sound.txt` (`Game_LoadSoundTxt` `0x0043A7F0`). All files are text in
//! cp1252 with CRLF; they are read line by line with prefix comparisons as in the original.

use std::collections::BTreeMap;

use crate::pef::{c_float, c_int};

/// Paths in the game.
pub const SOUND_DATABASE_PATH: &str = "data/sfx/sounddatabase.dat";
pub const OTHER_EVENTS_PATH: &str = "data/sfx/otherevents.ini";
pub const MUSIC_CONFIG_PATH: &str = "data/gifcon/musicconfig.ini";
pub const SOUND_TXT_PATH: &str = "data/txt/sound.txt";

/// Nations in the order of `musicconfig.ini` (`NATION 0` ... `NATION 5`) and
/// `dropdown.ini`; `PLAYER_PLAYER` of the `.lvl` names them with these abbreviations.
pub const NATIONS: [&str; 6] = ["WID", "PRI", "SPA", "SID", "ENG", "PAT"];

/// Nation index of an abbreviation (`SPA` -> 2), case-insensitive.
pub fn nation_index(name: &str) -> Option<usize> {
    NATIONS
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name.trim()))
}

/// Lines without line ending (`\r\n` or `\n`).
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// Text from character `n` (`CString::Mid`); shorter lines yield "".
fn mid(line: &str, n: usize) -> &str {
    line.char_indices().nth(n).map_or("", |(i, _)| &line[i..])
}

/// `sounddatabase.dat`: object GUID -> action -> sound GUIDs in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundDatabase {
    /// Value of `SOUNDGUIDLIST=` (the game loads the list immediately).
    pub sound_guid_list: Option<String>,
    /// Value of `ACTIONIDLIST=`.
    pub action_id_list: Option<String>,
    pub objects: BTreeMap<i32, BTreeMap<i32, Vec<i32>>>,
}

impl SoundDatabase {
    /// Sound GUIDs of an action of an object type (`None`: GUID or action missing).
    pub fn sounds(&self, guid: i32, action: i32) -> Option<&[i32]> {
        self.objects.get(&guid)?.get(&action).map(Vec::as_slice)
    }
}

/// Reads `sounddatabase.dat`: lines left-trimmed, prefixes of fixed length in the order
/// `SOUNDGUIDLIST=`, `ACTIONIDLIST=`, `GUID`, `ACTIONID`, `SOUNDGUID` (case
/// matters), numbers via `sscanf("%d")` from column 5, 9 or 10.
///
/// Deviation (shipped data unaffected): If `sscanf` reads no number, the game takes the value that was
/// previously on the stack; here the line then counts as "no GUID/action". So
/// `GUIDLIST=Data/TXT/guids.deu` creates no entry, and the following lines attach to no
/// GUID until the next valid `GUID` line comes.
pub fn parse_sound_database(text: &str) -> SoundDatabase {
    let mut db = SoundDatabase::default();
    let mut guid: Option<i32> = None;
    let mut action: Option<i32> = None;
    for line in lines(text) {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("SOUNDGUIDLIST=") {
            db.sound_guid_list = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("ACTIONIDLIST=") {
            db.action_id_list = Some(rest.to_string());
        } else if line.starts_with("GUID") {
            guid = c_int(mid(line, 5));
            action = None;
            if let Some(g) = guid {
                db.objects.entry(g).or_default();
            }
        } else if line.starts_with("ACTIONID") {
            action = c_int(mid(line, 9));
            if let (Some(g), Some(a)) = (guid, action) {
                db.objects.entry(g).or_default().entry(a).or_default();
            }
        } else if line.starts_with("SOUNDGUID")
            && let (Some(g), Some(a), Some(sound)) = (guid, action, c_int(mid(line, 10)))
        {
            db.objects
                .entry(g)
                .or_default()
                .entry(a)
                .or_default()
                .push(sound);
        }
    }
    db
}

/// An entry of the `soundguidlist.txt` (`GUID=factor:path`).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundEntry {
    /// Volume factor before the first `:` (`sscanf("%f")`; effective only for 3D sounds).
    pub factor: f32,
    /// WAV path after the first `:` (VFS, `\` and case arbitrary).
    pub path: String,
}

/// Reads a `number=text` list like `SoundDatabase_LoadSoundGuidList` (`0x0054FEE0`): lines
/// left-trimmed, only lines with `=` at position > 0, key via `sscanf("%d")`, value = rest
/// of the line. Later entries with the same number replace earlier ones (MFC map).
pub fn parse_id_list(text: &str) -> BTreeMap<i32, String> {
    let mut map = BTreeMap::new();
    for line in lines(text) {
        let line = line.trim_start();
        let Some(eq) = line.find('=') else { continue };
        if eq == 0 {
            continue;
        }
        if let Some(id) = c_int(&line[..eq]) {
            map.insert(id, line[eq + 1..].to_string());
        }
    }
    map
}

/// `soundguidlist.txt`: sound GUID -> factor and path. If the `:` is missing, the whole value counts as the path
/// (factor 1);
/// if the factor is unreadable (not present in the shipped data), the factor is 1.
pub fn parse_sound_guid_list(text: &str) -> BTreeMap<i32, SoundEntry> {
    parse_id_list(text)
        .into_iter()
        .map(|(id, value)| {
            let entry = match value.split_once(':') {
                Some((factor, path)) => SoundEntry {
                    factor: c_float(factor).unwrap_or(1.0),
                    path: path.to_string(),
                },
                None => SoundEntry {
                    factor: 1.0,
                    path: value,
                },
            };
            (id, entry)
        })
        .collect()
}

/// `otherevents.ini`: event name -> sound GUID. Lines are not trimmed; only lines with `=`
/// at position > 0; name = text before the `=` (unchanged), value = `atol` of the rest (unreadable: 0).
pub fn parse_other_events(text: &str) -> BTreeMap<String, i32> {
    let mut map = BTreeMap::new();
    for line in lines(text) {
        let Some(eq) = line.find('=') else { continue };
        if eq == 0 {
            continue;
        }
        map.insert(line[..eq].to_string(), c_int(&line[eq + 1..]).unwrap_or(0));
    }
    map
}

/// `musicconfig.ini`: music per nation and the thresholds of the combat music.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicConfig {
    /// Playlist per nation (`MUSIC`), index as [`NATIONS`].
    pub playlists: [Vec<String>; 6],
    /// Combat music per nation (`ATTACK`, the last line counts).
    pub attack: [Option<String>; 6],
    /// `ATTACKSTARTPOINTS`: combat points per step from which the start timer runs.
    pub attack_start_points: i32,
    /// `ATTACKSTARTTIME` (ms): the threshold must be reached for this long.
    pub attack_start_time: i32,
    /// `ATTACKSTOPPOINTS`: combat points above which the combat music keeps playing.
    pub attack_stop_points: i32,
    /// `ATTACKSTOPTIME` (ms): the stop threshold must be undershot for this long.
    pub attack_stop_time: i32,
    /// `ATTACKPAUSE` (ms): lock after a combat music.
    pub attack_pause: i32,
    /// `ATTACKMAXTIME` (ms): maximum duration of a combat music.
    pub attack_max_time: i32,
}

impl Default for MusicConfig {
    /// Empty lists, thresholds 0.
    fn default() -> Self {
        Self {
            playlists: Default::default(),
            attack: Default::default(),
            attack_start_points: 0,
            attack_start_time: 0,
            attack_stop_points: 0,
            attack_stop_time: 0,
            attack_pause: 0,
            attack_max_time: 0,
        }
    }
}

/// Reads `musicconfig.ini`: lines not trimmed, prefix comparison with the length of the key
/// including the space, value = rest of the line (`atol` for numbers). `MUSIC`/`ATTACK` before
/// the first `NATION` belong to nation 0; a nation outside 0-5 is ignored.
pub fn parse_music_config(text: &str) -> MusicConfig {
    let mut config = MusicConfig::default();
    let mut nation: i32 = 0;
    let number = |rest: &str| c_int(rest).unwrap_or(0);
    for line in lines(text) {
        let slot = usize::try_from(nation).ok().filter(|&n| n < NATIONS.len());
        if let Some(rest) = line.strip_prefix("NATION ") {
            nation = number(rest);
        } else if let Some(rest) = line.strip_prefix("MUSIC ") {
            if let Some(n) = slot {
                config.playlists[n].push(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("ATTACK ") {
            if let Some(n) = slot {
                config.attack[n] = Some(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("ATTACKSTARTPOINTS ") {
            config.attack_start_points = number(rest);
        } else if let Some(rest) = line.strip_prefix("ATTACKSTARTTIME ") {
            config.attack_start_time = number(rest);
        } else if let Some(rest) = line.strip_prefix("ATTACKSTOPPOINTS ") {
            config.attack_stop_points = number(rest);
        } else if let Some(rest) = line.strip_prefix("ATTACKSTOPTIME ") {
            config.attack_stop_time = number(rest);
        } else if let Some(rest) = line.strip_prefix("ATTACKPAUSE ") {
            config.attack_pause = number(rest);
        } else if let Some(rest) = line.strip_prefix("ATTACKMAXTIME ") {
            config.attack_max_time = number(rest);
        }
    }
    config
}

/// `sound.txt`: music ID of the script actions -> OGG path (split at the first `=`).
pub fn parse_sound_txt(text: &str) -> BTreeMap<i32, String> {
    let mut map = BTreeMap::new();
    for line in lines(text) {
        let Some((id, path)) = line.split_once('=') else {
            continue;
        };
        if let Some(id) = c_int(id) {
            map.insert(id, path.to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_lines_are_trimmed_and_matched_by_prefix() {
        let text = "GUIDLIST=Data/TXT/guids.deu\r\nSOUNDGUIDLIST=Data/sfx/soundguidlist.txt\r\nACTIONIDLIST=Data/sfx/actionidlist.txt\r\n\r\n\r\nGUID 2215\r\n\tACTIONID 00103\r\n\t\tSOUNDGUID 00232\r\n\t\tSOUNDGUID 00234\r\n\tACTIONID 00100\r\n\t\tSOUNDGUID 00008\r\nGUID 1313\r\n\tACTIONID 110\r\nguid 99\r\n\tACTIONID 5\r\n\t\tSOUNDGUID 7\r\n";
        let db = parse_sound_database(text);
        assert_eq!(
            db.sound_guid_list.as_deref(),
            Some("Data/sfx/soundguidlist.txt")
        );
        assert_eq!(
            db.action_id_list.as_deref(),
            Some("Data/sfx/actionidlist.txt")
        );
        // `GUIDLIST=...` reads no number and creates nothing.
        assert_eq!(db.objects.keys().copied().collect::<Vec<_>>(), [1313, 2215]);
        assert_eq!(db.sounds(2215, 103), Some(&[232, 234][..]));
        assert_eq!(db.sounds(2215, 100), Some(&[8][..]));
        // An empty list stays as an entry; lowercase "guid" does not count as a key, the following
        // lines still attach to GUID 1313.
        assert_eq!(db.sounds(1313, 110), Some(&[][..]));
        assert_eq!(db.sounds(1313, 5), Some(&[7][..]));
        assert_eq!(db.sounds(99, 5), None);
        assert_eq!(db.sounds(2215, 1), None);
    }

    #[test]
    fn sound_guid_list_splits_factor_and_path_at_the_first_colon() {
        let text = "00848=1.0:Data\\SFX\\Einheiten_WID\\WID_Speer_select_05.wav\r\n  00058=0.2:Data\\SFX\\Natureffekte\\NAT_Brandung_01.wav\r\n=1.0:x.wav\r\nno equals sign\r\n5=0.4:C:\\a.wav\r\n";
        let list = parse_sound_guid_list(text);
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[&848],
            SoundEntry {
                factor: 1.0,
                path: "Data\\SFX\\Einheiten_WID\\WID_Speer_select_05.wav".into()
            }
        );
        assert_eq!(list[&58].factor, 0.2);
        assert_eq!(list[&5].path, "C:\\a.wav");
        let names = parse_id_list("1=Schuss normal\r\n\r\n100=Select Einheit/Building\r\n");
        assert_eq!(names[&100], "Select Einheit/Building");
    }

    #[test]
    fn other_events_keep_the_name_untrimmed() {
        let text = "# Comment without assignment\r\nSFX_REGEN=895\r\nSFX_WATERWALK0=1177\r\n SFX_X=3\r\nSFX_LEER=\r\n=5\r\n";
        let events = parse_other_events(text);
        assert_eq!(events["SFX_REGEN"], 895);
        assert_eq!(events["SFX_WATERWALK0"], 1177);
        assert_eq!(events[" SFX_X"], 3);
        assert_eq!(events["SFX_LEER"], 0);
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn music_config_reads_playlists_attack_tracks_and_thresholds() {
        let text = "NATION 0\r\nMUSIC data/music/wid.ogg\r\nATTACK data/music/wid_attack.ogg\r\n/NATION\r\nNATION 2\r\nMUSIC data/music/spa.ogg\r\nMUSIC data/music/spa2.ogg\r\nATTACK x.ogg\r\nATTACK data/music/spa_attack.ogg\r\n/NATION\r\nNATION 9\r\nMUSIC verloren.ogg\r\n/NATION\r\nATTACKSTARTPOINTS 500\r\nATTACKSTARTTIME 1000\r\nATTACKSTOPPOINTS 0\r\nATTACKSTOPTIME 5000\r\nATTACKPAUSE 10000\r\nATTACKMAXTIME 720000\r\nMUSICX kaputt\r\n";
        let c = parse_music_config(text);
        assert_eq!(c.playlists[0], ["data/music/wid.ogg"]);
        assert_eq!(
            c.playlists[2],
            ["data/music/spa.ogg", "data/music/spa2.ogg"]
        );
        assert_eq!(c.attack[2].as_deref(), Some("data/music/spa_attack.ogg"));
        assert!(c.playlists[1].is_empty() && c.attack[1].is_none());
        assert_eq!(c.playlists.iter().map(Vec::len).sum::<usize>(), 3);
        assert_eq!(
            [
                c.attack_start_points,
                c.attack_start_time,
                c.attack_stop_points,
                c.attack_stop_time,
                c.attack_pause,
                c.attack_max_time
            ],
            [500, 1000, 0, 5000, 10000, 720000]
        );
    }

    #[test]
    fn sound_txt_maps_ids_to_paths_and_nations_map_to_indices() {
        let map = parse_sound_txt(
            "100=data/Music/atmo_karibik.ogg\r\n\r\n120=data/Music/SPA_ambient.ogg\r\nx=y\r\n",
        );
        assert_eq!(map.len(), 2);
        assert_eq!(map[&120], "data/Music/SPA_ambient.ogg");
        assert_eq!(nation_index("SPA"), Some(2));
        assert_eq!(nation_index("pat"), Some(5));
        assert_eq!(nation_index("IND"), None);
    }
}
