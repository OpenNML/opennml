//! Sound database at run time (`docs/formats/sounddatabase.md`): type GUID and action ->
//! sound GUID (`SoundDatabase_PickSoundGuid` `0x0054FBD0`), sound GUID -> factor and file,
//! event names, music per nation and script music.

use std::collections::BTreeMap;

use anyhow::Context;
use opennml_formats::sound::{
    MUSIC_CONFIG_PATH, MusicConfig, OTHER_EVENTS_PATH, SOUND_DATABASE_PATH, SOUND_TXT_PATH,
    SoundDatabase, SoundEntry, parse_id_list, parse_music_config, parse_other_events,
    parse_sound_database, parse_sound_guid_list, parse_sound_txt,
};
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;
use opennml_world::rand::MsvcRand;

/// All sound tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundDb {
    pub database: SoundDatabase,
    pub sounds: BTreeMap<i32, SoundEntry>,
    /// Action ID -> name (`actionidlist.txt`, description only).
    pub action_names: BTreeMap<i32, String>,
    pub events: BTreeMap<String, i32>,
    pub music: MusicConfig,
    /// Music ID of the scripts -> OGG path (`sound.txt`).
    pub tracks: BTreeMap<i32, String>,
}

impl SoundDb {
    /// Reads all files like `SoundManager_LoadDatabases` (`0x00552880`), `MusicManager_LoadConfig`
    /// and `Game_LoadSoundTxt`. A missing file leaves its table empty (warning); the game
    /// also starts without it.
    pub fn load(vfs: &Vfs) -> Self {
        let text = |path: &str| -> Option<String> {
            match vfs.read(path).with_context(|| format!("reading {path}")) {
                Ok(bytes) => Some(decode_cp1252(&bytes)),
                Err(e) => {
                    log::warn!("Sound: {e:#}");
                    None
                }
            }
        };
        let database = text(SOUND_DATABASE_PATH)
            .map(|t| parse_sound_database(&t))
            .unwrap_or_default();
        let sounds = database
            .sound_guid_list
            .as_deref()
            .and_then(text)
            .map(|t| parse_sound_guid_list(&t))
            .unwrap_or_default();
        let action_names = database
            .action_id_list
            .as_deref()
            .and_then(text)
            .map(|t| parse_id_list(&t))
            .unwrap_or_default();
        Self {
            database,
            sounds,
            action_names,
            events: text(OTHER_EVENTS_PATH)
                .map(|t| parse_other_events(&t))
                .unwrap_or_default(),
            music: text(MUSIC_CONFIG_PATH)
                .map(|t| parse_music_config(&t))
                .unwrap_or_default(),
            tracks: text(SOUND_TXT_PATH)
                .map(|t| parse_sound_txt(&t))
                .unwrap_or_default(),
        }
    }

    /// `SoundDatabase_PickSoundGuid(guid, action, index)`: `None` if the GUID or the action is
    /// missing or the list is empty; `index` -1 = `rand()`; the result is entry `index % n` (no
    /// repeat lock).
    pub fn pick(&self, guid: i32, action: i32, index: i32, rng: &mut MsvcRand) -> Option<i32> {
        let list = self.database.sounds(guid, action)?;
        if list.is_empty() {
            return None;
        }
        let index = if index == -1 {
            rng.rand() as usize
        } else {
            index.unsigned_abs() as usize
        };
        Some(list[index % list.len()])
    }

    /// Factor and file of a sound GUID (`SoundDatabase_GetSoundFile` `0x0054FD70`).
    pub fn sound(&self, sound_guid: i32) -> Option<&SoundEntry> {
        self.sounds.get(&sound_guid)
    }

    /// Sound GUID of an event (`SoundManager_FindOtherEvent` `0x00550E40`).
    pub fn event(&self, name: &str) -> Option<i32> {
        self.events.get(name).copied()
    }

    /// Actions of an object type with a non-empty list, ascending.
    pub fn actions(&self, guid: i32) -> Vec<i32> {
        self.database
            .objects
            .get(&guid)
            .map(|a| {
                a.iter()
                    .filter(|(_, l)| !l.is_empty())
                    .map(|(&k, _)| k)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Name of an action from `actionidlist.txt` (otherwise the number).
    pub fn action_name(&self, action: i32) -> String {
        self.action_names
            .get(&action)
            .cloned()
            .unwrap_or_else(|| action.to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Small database: type 2215 with actions 100 (three sounds), 12 (two) and 110 (empty).
    pub(crate) fn test_db() -> SoundDb {
        let database = parse_sound_database(
            "GUID 2215\n\tACTIONID 100\n\t\tSOUNDGUID 1\n\t\tSOUNDGUID 2\n\t\tSOUNDGUID 3\n\tACTIONID 12\n\t\tSOUNDGUID 4\n\t\tSOUNDGUID 5\n\tACTIONID 110\nGUID 4104\n\tACTIONID 200\n\t\tSOUNDGUID 6\n",
        );
        SoundDb {
            database,
            sounds: parse_sound_guid_list(
                "1=1.0:a.wav\n2=1.0:b.wav\n3=1.0:c.wav\n4=1.0:d.wav\n5=1.0:e.wav\n6=0.2:surf.wav\n7=1.0:rain.wav\n",
            ),
            action_names: parse_id_list("100=Select Einheit/Building\n12=Execute\n"),
            events: parse_other_events("SFX_REGEN=7\nSFX_WATERWALK3=6\n"),
            music: MusicConfig::default(),
            tracks: BTreeMap::new(),
        }
    }

    #[test]
    fn picking_uses_rand_modulo_or_the_given_index() {
        let db = test_db();
        let mut rng = MsvcRand::new(1);
        // `srand(1)`: 41, 18467, 6334 -> 41 % 3 = 2, 18467 % 3 = 2, 6334 % 3 = 1.
        let picks: Vec<_> = (0..3).map(|_| db.pick(2215, 100, -1, &mut rng)).collect();
        assert_eq!(picks, [Some(3), Some(3), Some(2)]);
        assert_eq!(db.pick(2215, 12, 0, &mut rng), Some(4));
        assert_eq!(db.pick(2215, 12, 3, &mut rng), Some(5));
        assert_eq!(db.pick(2215, 110, -1, &mut rng), None, "empty list");
        assert_eq!(db.pick(2215, 1, -1, &mut rng), None);
        assert_eq!(db.pick(9999, 100, -1, &mut rng), None);
        assert_eq!(db.sound(6).unwrap().factor, 0.2);
        assert_eq!(db.event("SFX_REGEN"), Some(7));
        assert_eq!(db.event("sfx_regen"), None);
        assert_eq!(db.actions(2215), [12, 100]);
        assert_eq!(db.action_name(12), "Execute");
        assert_eq!(db.action_name(14), "14");
    }
}
