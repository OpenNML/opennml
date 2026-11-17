//! Light, weather and music of a level without game simulation: applying light presets
//! (`GameEngine_ApplyLightPreset` `0x00430C40`) and the start state per level from the
//! start blocks of the level scripts (re/weather-and-light, re/audio).
//!
//! The fixed start tables ([`level_start_environment`]) apply **only without a script**: matches with a
//! running level script take light, weather and music from the presentation events of the simulation or, after
//! loading, from its presentation state (`nml_client::game::presentation`).

use anyhow::Context;
use nml_formats::lightdata::{LIGHTDATA_PATH, LightPreset, parse_lightdata};
use nml_formats::lvl::LevelFile;
use nml_formats::sound::nation_index;
use nml_formats::text::decode_cp1252;
use nml_formats::wld::ObjectLight;
use nml_rda::{Vfs, normalize_path};

use crate::level::LevelScene;

/// Weather kind. Note: scripts count 1 = rain, 2 = snow; `Terrain+0x170` 0 = snow,
/// 1 = rain (`GameEngine_SetWeather` `0x0043E420`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherKind {
    Rain,
    Snow,
}

/// Music action of a level script (`.abl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicAction {
    /// `n9;<id>` "Play music": script music (ID from `sound.txt`).
    Play(i32),
    /// `n370;<id>` "Set combat music": set combat music.
    Attack(i32),
    /// `n371;<points>`: start threshold of the combat music.
    AttackStartPoints(i32),
}

/// Light, weather and music with which a level starts in the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartEnvironment {
    /// Name of the light preset; `None` = values of the `.wld`.
    pub light: Option<&'static str>,
    pub weather: Option<WeatherKind>,
    /// Music actions of the start blocks in execution order.
    pub music: &'static [MusicAction],
}

/// Start light per level: script action 171 "Set light preset" under the condition "Always"
/// (re/weather-and-light, start light of a level).
const START_LIGHTS: [(&str, &str); 11] = [
    ("kampagne1/kamp1-2/kamp1-2.lvl", "Sommer Nacht"),
    ("kampagne1/kamp1-3/kamp1-3.lvl", "5"),
    ("kampagne1/kamp1-5/kamp1-5.lvl", "Spa 5 Nacht"),
    ("kampagne1/kamp1-6/kamp1-6.lvl", "6"),
    ("kampagne2/kamp2-2/kamp2-2.lvl", "wid 2 Schnee Nacht"),
    ("kampagne2/kamp2-9/kamp2-9.lvl", "10"),
    ("kampagne2/kamp2-10/kamp2-10.lvl", "9"),
    ("kampagne3/kamp3-2/kamp3-2.lvl", "22"),
    ("kampagne3/kamp3-9/kamp3-9.lvl", "PAt 4 Regen"),
    ("tutorial0-1/kamp0-1.lvl", "18"),
    ("tutorial0-2/kamp0-2.lvl", "10"),
];

/// Start weather per level: script action 209 "Start weather" under the condition "Always"
/// (re/weather-and-light, weather in the levels); changes controlled by time or counters
/// belong to the script evaluation.
const START_WEATHER: [(&str, WeatherKind); 7] = [
    ("kampagne2/kamp2-1/kamp2-1.lvl", WeatherKind::Snow),
    ("kampagne2/kamp2-2/kamp2-2.lvl", WeatherKind::Snow),
    ("kampagne2/kamp2-3/kamp2-3.lvl", WeatherKind::Snow),
    ("kampagne3/kamp3-3/kamp3-3.lvl", WeatherKind::Snow),
    ("multiplayer/[4players]-snowdream.lvl", WeatherKind::Snow),
    ("kampagne2/kamp2-5/kamp2-5.lvl", WeatherKind::Rain),
    (
        "multiplayer/[5players]-viciouscircle.lvl",
        WeatherKind::Rain,
    ),
];

use MusicAction::{Attack, AttackStartPoints, Play};

/// Music per level: the actions `n9`, `n370`, `n371` of the start blocks (condition "Always") of the
/// level `.abl` in execution order (blocks from back to front, actions in sequence;
/// a dialog via `n39` brings in the start blocks of its script at this point). Wait times
/// and dialogs run through statically at once (re/audio, music at level start).
const START_MUSIC: [(&str, &[MusicAction]); 32] = [
    (
        "kampagne1/kamp1-1/kamp1-1.lvl",
        &[AttackStartPoints(1000), Attack(115), Play(100)],
    ),
    ("kampagne1/kamp1-2/kamp1-2.lvl", &[Play(100)]),
    ("kampagne1/kamp1-3/kamp1-3.lvl", &[Play(102)]),
    ("kampagne1/kamp1-4/kamp1-4.lvl", &[Play(118)]),
    ("kampagne1/kamp1-5/kamp1-5.lvl", &[Play(104)]),
    (
        "kampagne1/kamp1-6/kamp1-6.lvl",
        &[Play(104), Attack(115), AttackStartPoints(1000)],
    ),
    ("kampagne2/kamp2-1/kamp2-1.lvl", &[Play(117)]),
    (
        "kampagne2/kamp2-2/kamp2-2.lvl",
        &[Play(119), AttackStartPoints(1000), Attack(117)],
    ),
    ("kampagne2/kamp2-3/kamp2-3.lvl", &[Play(116)]),
    ("kampagne2/kamp2-4/kamp2-4.lvl", &[Play(116)]),
    (
        "kampagne2/kamp2-5/kamp2-5.lvl",
        &[Play(116), AttackStartPoints(1000), Attack(117)],
    ),
    ("kampagne2/kamp2-6/kamp2-6.lvl", &[Play(106)]),
    ("kampagne2/kamp2-7/kamp2-7.lvl", &[Play(106)]),
    (
        "kampagne2/kamp2-8/kamp2-8.lvl",
        &[Play(109), AttackStartPoints(1000), Attack(110)],
    ),
    ("kampagne2/kamp2-9/kamp2-9.lvl", &[Play(109)]),
    ("kampagne2/kamp2-10/kamp2-10.lvl", &[Play(109)]),
    (
        "kampagne3/kamp3-1/kamp3-1.lvl",
        &[Play(103), AttackStartPoints(1000), Attack(105)],
    ),
    (
        "kampagne3/kamp3-2/kamp3-2.lvl",
        &[Play(104), AttackStartPoints(500), Attack(105)],
    ),
    ("kampagne3/kamp3-3/kamp3-3.lvl", &[Play(103)]),
    ("kampagne3/kamp3-4/kamp3-4.lvl", &[Play(116)]),
    ("kampagne3/kamp3-5/kamp3-5.lvl", &[Play(104)]),
    ("kampagne3/kamp3-6/kamp3-6.lvl", &[Play(118)]),
    ("kampagne3/kamp3-7/kamp3-7.lvl", &[Play(103)]),
    (
        "kampagne3/kamp3-8/kamp3-8.lvl",
        &[Play(104), AttackStartPoints(500), Attack(108)],
    ),
    ("kampagne3/kamp3-9/kamp3-9.lvl", &[Play(102)]),
    (
        "kampagne3/kamp3-10/kamp3-10.lvl",
        &[Play(107), AttackStartPoints(1000), Attack(108)],
    ),
    ("kampagne3/kamp3-11/kamp3-11.lvl", &[Play(111)]),
    (
        "kampagne3/kamp3-12/kamp3-12.lvl",
        &[Play(111), AttackStartPoints(100), Attack(113)],
    ),
    ("kampagne3/kamp3-13/kamp3-13.lvl", &[Play(111)]),
    ("kampagne3/kamp3-14/kamp3-14.lvl", &[Play(111)]),
    ("tutorial0-1/kamp0-1.lvl", &[Play(119)]),
    ("tutorial0-2/kamp0-2.lvl", &[Play(116)]),
];

/// Game mode of a level for the music (`GameEngine+0x108`): multiplayer maps start the
/// nation music themselves, menu backgrounds keep the menu music, campaigns only via script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    /// -1: single player/campaign.
    Campaign,
    /// >= 0: multiplayer game type.
    Multiplayer,
    /// -2: menu background level (assumed).
    Menu,
}

/// Prefix of the scene name of a random map (`randommap:<seed text>`); it does not end
/// in `.lvl` and therefore matches no level environment table.
pub const RANDOM_MAP_PREFIX: &str = "randommap:";

/// Is `lvl_path` the scene name of a random map (any capitalization)?
pub fn is_random_map_label(lvl_path: &str) -> bool {
    normalize_path(lvl_path).starts_with(RANDOM_MAP_PREFIX)
}

/// Game mode by the folder of the level (the viewer loads levels directly; in the game the start
/// path determines it). A random map is free play (game type 0 to 3): nation music like fixed
/// multiplayer maps, no music default (re/random-map Z.8).
pub fn level_game_mode(lvl_path: &str) -> GameMode {
    let path = normalize_path(lvl_path);
    let key = path.strip_prefix("data/levels/").unwrap_or(&path);
    if path.starts_with(RANDOM_MAP_PREFIX) || key.starts_with("multiplayer/") {
        GameMode::Multiplayer
    } else if key.starts_with("menülevel/") {
        GameMode::Menu
    } else {
        GameMode::Campaign
    }
}

/// Nation of the local player from the `.lvl` (`Game_SelectLocalPlayer(0)` `0x004BB3B0`): the `[PLAYER]` with
/// `PLAYER_TYPE 0` and the smallest `PLAYER_COLOR` (< 8); `PLAYER_PLAYER` via the nation list
/// (`WID` 0 ... `PAT` 5).
///
/// Only the static reading of the map: it matches the original only in the campaign. In free play
/// the lobby overrides the nation (`GameEngine_SetupPlayersFromLobby` `0x00432E6A`). The nation
/// music therefore takes the nation of the local player in the simulation
/// (`nml_client::game::presentation::music_nation`); this function is only the fallback there
/// without a simulation (map without loadable rules).
pub fn local_player_nation(level: &LevelFile) -> Option<usize> {
    level
        .players
        .iter()
        .filter(|p| p.kind == 0 && (0..8).contains(&p.color))
        .min_by_key(|p| p.color)
        .and_then(|p| nation_index(&p.nation))
}

/// Start state of the level `lvl_path` (game path of the `.lvl`, any capitalization). Static
/// reading of the scripts, **only without a script**: the fallback for matches without a running level script (map
/// without simulation, script off or not loaded); with a script the script evaluation sets the state.
pub fn level_start_environment(lvl_path: &str) -> StartEnvironment {
    let path = normalize_path(lvl_path);
    let key = path.strip_prefix("data/levels/").unwrap_or(&path);
    StartEnvironment {
        light: START_LIGHTS
            .iter()
            .find(|(level, _)| *level == key)
            .map(|&(_, light)| light),
        weather: START_WEATHER
            .iter()
            .find(|(level, _)| *level == key)
            .map(|&(_, weather)| weather),
        music: START_MUSIC
            .iter()
            .find(|(level, _)| *level == key)
            .map_or(&[], |&(_, music)| music),
    }
}

/// Reads `data/projects/lightdata.ini` (once at start, like the constructor of the game engine).
pub fn load_light_presets(vfs: &Vfs) -> anyhow::Result<Vec<LightPreset>> {
    let bytes = vfs
        .read(LIGHTDATA_PATH)
        .with_context(|| format!("reading {LIGHTDATA_PATH}"))?;
    Ok(parse_lightdata(&decode_cp1252(&bytes)))
}

impl LevelScene {
    /// Applies a light preset (`None` = values of the `.wld`): terrain light and
    /// vertex colors anew, object light replaced. Fog, shadow light and water textures stay.
    pub fn apply_light(&mut self, preset: Option<&LightPreset>) {
        match preset {
            Some(preset) => {
                self.terrain.set_light(&preset.terrain);
                self.object_light = Some(ObjectLight {
                    position: preset.object.position,
                    color: preset.object.color,
                    intensity: preset.object.intensity,
                });
            }
            None => {
                let t = &self.world.terrain;
                self.terrain.set_light(&nml_formats::lightdata::LightBlock {
                    position: t.light_position,
                    color: t.light_color,
                    intensity: t.light_intensity,
                });
                self.object_light = self.world.light.clone();
            }
        }
    }

    /// Action 381 (`FUN_0042F130`, E-236): only the object light goes back to the `[LIGHT]` block of the `.wld`;
    /// the terrain keeps the light of the last preset (171).
    pub fn restore_object_light(&mut self) {
        self.object_light = self.world.light.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::lightdata::LightBlock;

    #[test]
    fn start_environment_is_looked_up_by_level_path() {
        let kamp1_2 = level_start_environment(r"Data\Levels\Kampagne1\kamp1-2\kamp1-2.lvl");
        assert_eq!(
            kamp1_2,
            StartEnvironment {
                light: Some("Sommer Nacht"),
                weather: None,
                music: &[MusicAction::Play(100)],
            }
        );
        let kamp2_2 = level_start_environment("data/levels/kampagne2/kamp2-2/kamp2-2.lvl");
        assert_eq!(
            (kamp2_2.light, kamp2_2.weather),
            (Some("wid 2 Schnee Nacht"), Some(WeatherKind::Snow))
        );
        assert_eq!(
            level_start_environment("data/levels/multiplayer/[5players]-viciouscircle.lvl").weather,
            Some(WeatherKind::Rain)
        );
        assert_eq!(
            level_start_environment("data/levels/tutorial0-1/kamp0-1.lvl").light,
            Some("18")
        );
        // kamp1-1 begins with the values of the .wld and without weather; kamp2-10 is not kamp2-1.
        let kamp1_1 = level_start_environment("data/levels/kampagne1/kamp1-1/kamp1-1.lvl");
        assert_eq!((kamp1_1.light, kamp1_1.weather), (None, None));
        assert_eq!(
            kamp1_1.music,
            [
                MusicAction::AttackStartPoints(1000),
                MusicAction::Attack(115),
                MusicAction::Play(100)
            ]
        );
        assert_eq!(
            level_start_environment("data/levels/tutorial0-2/kamp0-2.lvl").music,
            [MusicAction::Play(116)]
        );
        assert!(
            level_start_environment("data/levels/multiplayer/[4players]-donut-isle.lvl")
                .music
                .is_empty()
        );
        assert_eq!(
            level_start_environment("data/levels/kampagne2/kamp2-10/kamp2-10.lvl").weather,
            None
        );
    }

    /// A random map (`randommap:<seed>`) matches no start table (D-363) - no start light, no weather, no
    /// start music; its scene does not set the fog
    /// (corpus test `random_rules_load_without_lvl` in `nml-randommap`).
    #[test]
    fn random_map_has_no_start_environment() {
        for label in [
            "randommap:ZKA",
            "randommap:",
            "randommap:kampagne1/kamp1-2/kamp1-2.lvl",
        ] {
            assert_eq!(
                level_start_environment(label),
                StartEnvironment {
                    light: None,
                    weather: None,
                    music: &[],
                },
                "{label}"
            );
        }
    }

    /// The random map is free play (game type >= 0) - the nation music starts like on fixed
    /// multiplayer maps.
    #[test]
    fn random_map_is_multiplayer_mode() {
        for label in ["randommap:ZKA", "randommap:", "RandomMap:Ä b/c"] {
            assert_eq!(level_game_mode(label), GameMode::Multiplayer, "{label}");
        }
        assert!(is_random_map_label("randommap:ZKA"));
        assert!(!is_random_map_label(
            "data/levels/multiplayer/[4players]-exile.lvl"
        ));
    }

    #[test]
    fn game_mode_and_local_nation_follow_the_level() {
        assert_eq!(
            level_game_mode("Data/Levels/Multiplayer/[2players]-crossrail.lvl"),
            GameMode::Multiplayer
        );
        assert_eq!(
            level_game_mode("data/levels/menülevel/menü.lvl"),
            GameMode::Menu
        );
        assert_eq!(
            level_game_mode("data/levels/kampagne1/kamp1-1/kamp1-1.lvl"),
            GameMode::Campaign
        );
        let level = nml_formats::lvl::parse_level(
            "LEVELFILE V1.08e\r\nPROJECT_FILE \"x.prj\"\r\n\
             [PLAYER]\r\n{\r\n\tPLAYER_NAME \"KI\"\r\n\tPLAYER_PLAYER \"PAT\"\r\n\tPLAYER_TYPE 1\r\n\tPLAYER_COLOR 0\r\n}\r\n\
             [PLAYER]\r\n{\r\n\tPLAYER_NAME \"A\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 6\r\n}\r\n\
             [PLAYER]\r\n{\r\n\tPLAYER_NAME \"B\"\r\n\tPLAYER_PLAYER \"wid\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 2\r\n}\r\n",
        )
        .unwrap();
        // Two human players: the one with the smaller color (2, WID).
        assert_eq!(local_player_nation(&level), Some(0));
    }

    #[test]
    fn light_presets_replace_terrain_and_object_light_and_level_restores_them() {
        let (terrain, project) = crate::terrain::tests::test_terrain(2, |_, _| 1);
        let light = Some(ObjectLight {
            position: [0.0, 0.0, -1.0],
            color: [255, 241, 193],
            intensity: 40,
        });
        let world = nml_formats::wld::World {
            terrain: terrain.params.clone(),
            areas: Vec::new(),
            cameras: Vec::new(),
            light: light.clone(),
            shadow: None,
            fog: None,
            objects: Vec::new(),
        };
        let mut scene = LevelScene {
            lvl_path: "data/levels/test/test.lvl".into(),
            level: nml_formats::lvl::parse_level(
                "LEVELFILE V1.08e
PROJECT_FILE \"x.prj\"
",
            )
            .unwrap(),
            world,
            project,
            types: Default::default(),
            terrain,
            objects: Vec::new(),
            start_points: Default::default(),
            object_light: light,
        };
        let loaded: Vec<[u8; 3]> = scene.terrain.vertices.iter().map(|v| v.lit_color).collect();
        let preset = LightPreset {
            name: "Night".into(),
            terrain: LightBlock {
                position: [0.0, 1.0, 0.0],
                color: [82, 90, 93],
                intensity: 15,
            },
            object: LightBlock {
                position: [800.0, 400.0, -1300.0],
                color: [32, 88, 145],
                intensity: 65,
            },
        };
        scene.apply_light(Some(&preset));
        assert_eq!(scene.terrain.vertex(1, 1).lit_color, [151, 166, 172]);
        assert_eq!(scene.object_light.as_ref().unwrap().color, [32, 88, 145]);
        // The level data itself stays unchanged.
        assert_eq!(scene.world.terrain.light_color, [200, 200, 200]);
        scene.apply_light(None);
        let back: Vec<[u8; 3]> = scene.terrain.vertices.iter().map(|v| v.lit_color).collect();
        assert_eq!(back, loaded);
        assert_eq!(scene.object_light, scene.world.light);
    }
}
