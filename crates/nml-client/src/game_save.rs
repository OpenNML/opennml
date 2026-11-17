//! Savegame container of a match: the savegame of the simulation
//! ([`SaveGame`], unchanged in its own bytes) and the host part ([`HostPart`]: camera like
//! `m_NextFrom/At/FOV`, map description for the loading screen, game mode, name of the local player,
//! game speed, remembered speed of the cinemascope, camera of the scripts, control groups,
//! `selectnextguid` indices, idle worker index, chat and tribute masks, fixed teams, the fog switch and
//! in network savegames the network part [`NetSaveInfo`]). No influence on checksums.
//!
//! File: identifier `NMLGAME\0`, [`FORMAT`] as u32 LE, FNV-1a 64 of the body as u64 LE, body = `postcard` of
//! `{ sim: SaveGame::to_bytes(), host }`. The simulation part stays in its own bytes so that a change
//! of `nml_sim::setup::FORMAT_VERSION` is noticed on loading as a "format version" (messages of
//! [`SaveGame::from_bytes`]) and not as a random error while reading.
//!
//! The original also saves the game speed (re/game-ui 3).

use anyhow::{Context, bail};
use nml_sim::rules::Rules;
use nml_sim::save::{RecordedCommand, SaveGame, read_container, write_container};
use nml_world::camera::GameCamera;

use crate::game::control_groups::{GroupsSave, NextGuidIndex};
use crate::game::diplomacy::Masks;
use crate::game::script_camera::ScriptCamera;
use serde::{Deserialize, Serialize};

/// Identifier of the file.
pub const GAME_MAGIC: &[u8; 8] = b"NMLGAME\0";
/// Format version of the container. History: 1 initial; 2 `HostPart::cinemascope_speed`, `HostPart::script_camera`;
/// 3 moves of the player in the script camera (`MoveLeg::player`, `ScriptCamera::player_leg`),
/// `HostPart::control_groups`, `HostPart::next_guid_index`; 4 `HostPart::idle_worker_index`, `HostPart::masks`,
/// `HostPart::lock_teams`, `HostPart::fog_show_mode`; 5 menu values of the random map in `HostPart::level_info`
/// (`LevelInfoData::random_terrain`, `random_landscape`, `random_daytime`); 6 network part `HostPart::net`
/// ([`NetSaveInfo`]); 7 `NetSaveInfo` with turn spacing and lead (`spacing`, `lead`) instead of `turn_steps`;
/// 8 command log and `SAVEREPLAY` in the host part (`HostPart::log`, `HostPart::save_replay`).
pub const FORMAT: u32 = 8;

/// Network part of the host in a network savegame: what loading in the network needs and the simulation does not
/// know. Each machine writes its own file with the same simulation part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetSaveInfo {
    /// Name of the session (`StartInfo::session`).
    pub session: String,
    /// Own slot of this machine (0 ... 7, a human slot).
    pub local_slot: u8,
    /// Turn spacing `T` in ticks (valid as for [`nml_net::TurnConfig::new`]: `1 <= V < T <= 40`).
    pub spacing: u8,
    /// Lead `V` in ticks.
    pub lead: u8,
    /// Slot assignment 0 ... 7: machine (`None` = no machine, AI or empty) and name.
    pub seats: Vec<(Option<u8>, String)>,
}

impl NetSaveInfo {
    /// Cleans the names on reading like names from the network: the session name with
    /// [`nml_net::clean_session_name`], each slot name with [`nml_net::clean_name`]. An empty slot name (slot without
    /// player, as `start_infos` writes it) stays empty instead of becoming "Spieler".
    pub fn cleaned(self) -> Self {
        Self {
            session: nml_net::clean_session_name(&self.session),
            seats: self
                .seats
                .into_iter()
                .map(|(peer, name)| {
                    let name = if name.is_empty() {
                        name
                    } else {
                        nml_net::clean_name(&name)
                    };
                    (peer, name)
                })
                .collect(),
            ..self
        }
    }
}

/// Checksum of the simulation part of a savegame: start package and state, equal on all machines of a
/// network match, independent of the host part and the bytes of the container. Identifies a network savegame in the
/// announcement (`MapId::Saved`) and in the join (`SeatClaim`).
pub fn sim_checksum(save: &SaveGame) -> u64 {
    nml_sim::save::checksum_of(&(&save.start, &save.state))
}

/// Camera in the savegame like `m_NextFrom`/`m_NextAt`/`m_NextFOV`: eye, target and field of view of the game camera
/// ([`GameCamera`]). The free camera of the viewer does not belong in it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraSave {
    pub from: [f32; 3],
    pub at: [f32; 3],
    pub fov: f32,
}

impl CameraSave {
    /// Position of the game camera.
    pub fn of(camera: &GameCamera) -> Self {
        Self {
            from: camera.eye.to_array(),
            at: camera.target.to_array(),
            fov: camera.setup.fov,
        }
    }

    /// Sets eye, target and field of view of the game camera (on loading).
    pub fn apply(&self, camera: &mut GameCamera) {
        camera.eye = self.from.into();
        camera.target = self.at.into();
        camera.setup.fov = self.fov;
    }
}

/// Host part in the savegame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostPart {
    pub camera: CameraSave,
    /// Map description for the loading screen.
    pub level_info: nml_ui::draft::LevelInfoData,
    pub game_type: i32,
    pub local_name: String,
    /// `m_GameSpeed` (0 ... 255); when resuming, the speed of the simulation.
    pub game_speed: u8,
    /// Remembered speed of the cinemascope (`GE+0x4D4`, E-238; 0 ... 255): the speed before the
    /// last 147, `None` before the first 147 of the match (148 then sets 0). The original does not save it
    /// (deviation from the original).
    pub cinemascope_speed: Option<u8>,
    /// Camera of the scripts (deviation from the original): backup, list of moves, running move, rotation,
    /// binding and open cut; after loading they continue with their remaining values.
    pub script_camera: ScriptCamera,
    /// Control groups of all 8 player slots (`m_Groups` in `Player::Serialize` `0x004FB220`, ten lists of IDs per
    /// slot; O-312). Kept in the host part (deviation from the original). The last selected group (`+0x354`) is not
    /// in it (as in the original, O-312): after loading -1, the selection is empty. Stale and foreign IDs stay
    /// (O-311).
    pub control_groups: Vec<GroupsSave>,
    /// `selectnextguid` indices of all 8 slots (`m_LastSelectedHandleIndex` `GameWorld+0x22B8`, stored in the world
    /// state in the original; deviation from the original): per slot GUID -> object slot of the last hit + 1. The
    /// double-tap deadline is not in it (volatile as in the original).
    pub next_guid_index: Vec<NextGuidIndex>,
    /// Idle worker index of all 8 slots (`m_WaitingWorkerIndex`, stored in the world state in the original; deviation
    /// from the original): button/key 576 raises it without a command (`0x0048B610`), so it lives in the host and not
    /// in `SimState`. Unchecked: a value outside the list, even a negative one, counts as 0 on the next press.
    pub idle_worker_index: [i32; 8],
    /// Chat and tribute mask of all 8 slots (`m_ChatMask`, `m_TributMask` in the player part; deviation from the
    /// original). One bit per slot; loading does not check higher bits.
    pub masks: Masks,
    /// Fixed teams (`Engine+0x20`, key `m_IsLockTeams` in the savegame of the engine): locks the status selection of
    /// the diplomacy dialog. Engine field, not in `SimState` (no effect on checksums).
    pub lock_teams: bool,
    /// Fog switch `m_FogShowMode` (key in `GameWorld::Serialize`; in the reimplementation in the host,
    /// `FogDisplay::show_mode`, deviation from the original): 0 fog on, `FOG_OFF` off (`rainbowring`, cinemascope,
    /// viewer). The original has a single variable; therefore on resuming this value applies and not the script
    /// state (`presentation.fog_show_all`). 0 ... `FOG_OFF` (the fog query ORs it onto the flags,
    /// `FogView::with_show_mode`).
    pub fog_show_mode: u8,
    /// Network part: only in network savegames (`savegame/multiplayer/<Name>`), otherwise `None`.
    pub net: Option<NetSaveInfo>,
    /// Command log from the start of the match up to the state (E-329): `Some` if it is complete
    /// (`SimHost::save_log`); a recording after loading thus continues the whole match from tick 0. Not
    /// part of the simulation part and the checksum. Whether it leads to the state is not checked on loading.
    pub log: Option<Vec<RecordedCommand>>,
    /// `SAVEREPLAY` switch of the match: whether recording continues after loading.
    pub save_replay: bool,
}

/// Body of the file.
#[derive(Serialize, Deserialize)]
struct Body {
    /// [`SaveGame::to_bytes`] with identifier, version and checksum of the simulation.
    sim: Vec<u8>,
    host: HostPart,
}

/// Body for writing (without a copy of the host part).
#[derive(Serialize)]
struct BodyRef<'a> {
    sim: &'a [u8],
    host: &'a HostPart,
}

/// Savegame of a match.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSaveFile {
    pub sim: SaveGame,
    pub host: HostPart,
}

impl GameSaveFile {
    /// File: header, then the body with the savegame of the simulation in its own bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = postcard::to_stdvec(&BodyRef {
            sim: &self.sim.to_bytes(),
            host: &self.host,
        })
        .expect("serializable");
        write_container(GAME_MAGIC, FORMAT, &body)
    }

    /// Reads a file: identifier, format version and checksum of the container ([`read_container`]), then the
    /// savegame of the simulation via [`SaveGame::from_bytes`] (its messages, also "format version" of the
    /// simulation). It matches the rules only after [`GameSaveFile::check`].
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let body = read_container(GAME_MAGIC, FORMAT, bytes, "savegame")?;
        let (Body { sim, host }, rest) =
            postcard::take_from_bytes(body).context("savegame corrupted")?;
        // A body longer than its content does not match the format (e.g. eleven instead of ten group lists).
        if !rest.is_empty() {
            bail!(
                "savegame corrupted ({} extra bytes at the end of the body)",
                rest.len()
            );
        }
        let sim = SaveGame::from_bytes(&sim).context("savegame of the simulation")?;
        let mut host = host;
        host.net = host.net.map(NetSaveInfo::cleaned);
        Ok(Self { sim, host })
    }

    /// Does the savegame match the loaded rules (start package, map, state: [`SaveGame::check`]), and do the
    /// network part and the network flag belong together? Loading does not check the rest of the host part: camera,
    /// speed, script camera, control groups, indices, masks, fog switch, game mode and command log are tolerated by
    /// the host with any value.
    pub fn check(&self, rules: &Rules) -> anyhow::Result<()> {
        self.sim.check(rules)?;
        // The network part and the network flag belong together (`SaveGame::check` keeps the flag of the state equal
        // to that of the start package).
        match (&self.host.net, self.sim.start.setup.network) {
            (Some(_), false) => bail!("savegame corrupted (network savegame without network flag)"),
            (None, true) => bail!("savegame corrupted (single-player savegame with network flag)"),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::control_groups;
    use crate::game::fog_view::FOG_OFF;
    use crate::game::sim_host::plain_testkit;
    use nml_sim::save::HEADER_LEN;

    fn host_part() -> HostPart {
        HostPart {
            camera: CameraSave {
                from: [100.0, 2000.0, -300.0],
                at: [800.0, 150.0, 900.0],
                fov: 0.75,
            },
            level_info: nml_ui::draft::LevelInfoData {
                name: "Test map".into(),
                path: "data/levels/multiplayer/test.lvl".into(),
                size_class: 1,
                players: 2,
                level_type: 2,
                ..nml_ui::draft::LevelInfoData::default()
            },
            game_type: 1,
            local_name: "Tester".into(),
            game_speed: 14,
            cinemascope_speed: Some(14),
            script_camera: ScriptCamera::default(),
            control_groups: control_groups::empty_save(),
            next_guid_index: control_groups::empty_next_guid_index(),
            idle_worker_index: [0, 2, 0, 0, 0, 0, 0, 7],
            masks: Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        }
    }

    fn save_file() -> (GameSaveFile, Rules) {
        let mut host = plain_testkit::host();
        for _ in 0..3 {
            host.step_once();
        }
        let file = GameSaveFile {
            sim: host.save(),
            host: host_part(),
        };
        (file, host.rules)
    }

    /// Rewrites the header: body `body` with a valid checksum (for errors behind the header).
    fn with_body(body: &[u8]) -> Vec<u8> {
        write_container(GAME_MAGIC, FORMAT, body)
    }

    #[test]
    fn game_save_round_trips() {
        let (file, rules) = save_file();
        let bytes = file.to_bytes();
        assert_eq!(&bytes[..8], GAME_MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), FORMAT);
        let back = GameSaveFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, file);
        back.check(&rules).unwrap();
        // The simulation part is in the body in its own bytes.
        let body: Body = postcard::from_bytes(&bytes[HEADER_LEN..]).unwrap();
        assert_eq!(body.sim, file.sim.to_bytes());
    }

    /// O-153: The keys clamp to 0 to 19, loading and resuming accept any speed 0 ... 255.
    // QUIRK O-319: kept
    // QUIRK O-153: deviation
    #[test]
    fn load_accepts_every_speed() {
        let (mut file, rules) = save_file();
        for speed in [0, 19, 20, 30, 255] {
            file.host.game_speed = speed;
            let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
            back.check(&rules).unwrap();
            assert_eq!(back.host.game_speed, speed);
        }
        // The key clamps a loaded speed 20 to 19 (`FUN_0042F050`: `v > 18` gives 19).
        assert_eq!(crate::game_session::key_game_speed(20, 1), Some(19));
        assert_eq!(crate::game_session::key_game_speed(20, -1), Some(19));
        assert_eq!(crate::game_session::key_game_speed(19, 1), None);
        assert_eq!(crate::game_session::key_game_speed(0, -1), None);
    }

    /// O-332: A control group with 10000 IDs loads (no length limit).
    // QUIRK O-332: kept
    #[test]
    fn groups_with_10000_ids_load() {
        let (mut file, rules) = save_file();
        file.host.control_groups[0][1] = (0..10_000).collect();
        let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(
            back.host.control_groups[0][1],
            file.host.control_groups[0][1]
        );
    }

    #[test]
    fn game_save_rejects_wrong_magic_version_and_checksum() {
        let (file, rules) = save_file();
        let bytes = file.to_bytes();
        let error = |b: &[u8]| format!("{:#}", GameSaveFile::from_bytes(b).unwrap_err());
        // Identifier.
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(
            error(&wrong).contains("not a savegame file"),
            "{}",
            error(&wrong)
        );
        assert!(error(&bytes[..5]).contains("not a savegame file"));
        // Format version of the container.
        let mut wrong = bytes.clone();
        wrong[8..12].copy_from_slice(&(FORMAT + 1).to_le_bytes());
        assert!(
            error(&wrong).contains(&format!("format version {}", FORMAT + 1)),
            "{}",
            error(&wrong)
        );
        // Checksum: one byte in the body.
        let mut wrong = bytes.clone();
        let last = wrong.len() - 1;
        wrong[last] ^= 1;
        assert!(
            error(&wrong).contains("corrupted (checksum"),
            "{}",
            error(&wrong)
        );
        assert!(error(&bytes[..15]).contains("corrupted"));
        // Format version of the simulation (another `FORMAT_VERSION`): message from `nml-sim`.
        let mut sim = file.sim.to_bytes();
        sim[8..12].copy_from_slice(&999u32.to_le_bytes());
        let body = postcard::to_stdvec(&Body {
            sim,
            host: file.host.clone(),
        })
        .unwrap();
        let message = error(&with_body(&body));
        assert!(message.contains("format version 999"), "{message}");
        // Check against the rules (rules checksum).
        let mut bad = file;
        bad.sim.start.rules_checksum ^= 1;
        assert!(bad.check(&rules).is_err());
    }

    /// Saving in the middle of a move with binding and rotation; after loading, the camera moves for N frames exactly
    /// as without loading. Container `FORMAT` 1 (without script camera) is rejected with an error message.
    // QUIRK E-239: deviation
    #[test]
    fn host_part_restores_the_script_camera() {
        use crate::game::input::testkit;
        use nml_sim::script::present::Presentation;
        assert_eq!(FORMAT, 8);
        let (mut camera, terrain) = testkit::setup();
        let (file, rules) = save_file();
        let host = plain_testkit::host();
        let id = host.state.objects().next().unwrap().id;
        let cameras = [nml_formats::wld::NamedCamera {
            name: "Start Camera".into(),
            from: [1000.0, 3000.0, 1200.0],
            at: [2000.0, 500.0, 2400.0],
            roll: 0.0,
        }];
        let mut script = ScriptCamera::default();
        for event in [
            Presentation::CameraBackup,
            Presentation::CameraBind { id: Some(id) },
            Presentation::CameraMove {
                camera: "start camera".into(),
                ms: 1500,
            },
            Presentation::CameraToGame { ms: 700 },
            Presentation::CameraMoveToBackup { ms: 400 },
            Presentation::CameraRotate {
                degrees: 40,
                ms: 900,
            },
        ] {
            script.apply(&event, &mut camera, &cameras, &terrain, &host);
        }
        for _ in 0..3 {
            script.update(120.0, &mut camera, &terrain, &host);
        }
        assert!(
            script.rest_ms > 0.0 && script.bind == Some(id),
            "in the middle of the move with binding"
        );
        assert_eq!(script.moves.len(), 2);
        let mut save = file.clone();
        save.host.camera = CameraSave::of(&camera);
        save.host.script_camera = script.clone();
        let back = GameSaveFile::from_bytes(&save.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.script_camera, script);
        let (mut loaded, _) = testkit::setup();
        back.host.camera.apply(&mut loaded);
        let mut restored = back.host.script_camera;
        for frame in 0..40 {
            script.update(120.0, &mut camera, &terrain, &host);
            restored.update(120.0, &mut loaded, &terrain, &host);
            assert_eq!(
                (loaded.eye, loaded.target),
                (camera.eye, camera.target),
                "frame {frame}"
            );
        }
        assert_eq!(restored, script);
        assert!(script.moves.is_empty() && script.rotation.is_none());
        // Container format 3: a running and a waiting move of the player (Home) stay player moves after loading and
        // do not lock the player camera.
        let mut home = ScriptCamera::default();
        home.push_home(&camera);
        home.update(100.0, &mut camera, &terrain, &host);
        home.push_home(&camera);
        assert!(home.player_leg && home.rest_ms > 0.0 && home.moves.len() == 1);
        let mut save_home = save.clone();
        save_home.host.script_camera = home.clone();
        let back = GameSaveFile::from_bytes(&save_home.to_bytes()).unwrap();
        assert_eq!(back.host.script_camera, home);
        assert!(!back.host.script_camera.running() && back.host.script_camera.moving());
        // A savegame with container format 1 (before the script camera).
        let mut old = save.to_bytes();
        old[8..12].copy_from_slice(&1u32.to_le_bytes());
        let message = format!("{:#}", GameSaveFile::from_bytes(&old).unwrap_err());
        assert!(
            message.contains(&format!("format version 1, expected {FORMAT}")),
            "{message}"
        );
    }

    /// Idle worker index and masks survive saving and loading. A savegame with container format 3 (before the new
    /// fields) is rejected with an error message.
    // QUIRK O-293: kept
    #[test]
    fn idle_index_and_masks_round_trip() {
        let (mut file, rules) = save_file();
        file.host.masks.chat[2] = 0b0000_0101;
        file.host.masks.tribute[0] = 0xFF;
        file.host.lock_teams = true;
        let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.idle_worker_index, [0, 2, 0, 0, 0, 0, 0, 7]);
        assert_eq!(back.host.masks, file.host.masks);
        assert!(back.host.lock_teams, "fixed teams (in the same format 4)");
        let mut old = file.to_bytes();
        old[8..12].copy_from_slice(&3u32.to_le_bytes());
        let message = format!("{:#}", GameSaveFile::from_bytes(&old).unwrap_err());
        assert!(
            message.contains(&format!("format version 3, expected {FORMAT}")),
            "{message}"
        );
    }

    /// The network part of the host (session, own slot, turn spacing, slot assignment) survives saving and
    /// loading.
    // QUIRK E-274: kept
    #[test]
    fn host_part_net_round_trips() {
        use nml_sim::state::Control;
        // Network match with humans on 0 ... 2.
        let sim = crate::game::testworld::net_host(1, false);
        let mut file = GameSaveFile {
            sim: sim.save(),
            host: host_part(),
        };
        let rules = sim.rules;
        let players = &file.sim.state.players;
        let human = players
            .iter()
            .position(|p| p.control == Control::Human)
            .expect("human slot") as u8;
        let seats: Vec<(Option<u8>, String)> = (0..8u8)
            .map(|slot| ((slot < 2).then_some(slot), format!("Spieler {slot}")))
            .collect();
        let info = NetSaveInfo {
            session: "Test match".into(),
            local_slot: human,
            spacing: 8,
            lead: 4,
            seats,
        };
        file.host.net = Some(info.clone());
        let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.net, Some(info));
        // Without a network part, a state with the network flag is corrupted
        // (`single_save_with_the_network_flag_is_rejected`).
        file.host.net = None;
        assert!(file.check(&rules).is_err());
    }

    /// Network savegame from a network match (humans on 0 ... 2, network flag on) with network part: own slot 1,
    /// machines on 0 ... 2.
    fn net_file() -> (GameSaveFile, Rules) {
        let sim = crate::game::testworld::net_host(1, false);
        let seats = (0..8u8)
            .map(|slot| ((slot < 3).then_some(slot), format!("Spieler {slot}")))
            .collect();
        let mut host = host_part();
        host.net = Some(NetSaveInfo {
            session: "Test match".into(),
            local_slot: 1,
            spacing: 8,
            lead: 4,
            seats,
        });
        (
            GameSaveFile {
                sim: sim.save(),
                host,
            },
            sim.rules,
        )
    }

    /// Session name and slot names are cleaned on reading like names from the network (`clean_session_name`,
    /// `clean_name`); an empty slot name (slot without player) stays empty.
    #[test]
    fn net_names_are_cleaned_on_load() {
        let (mut file, rules) = net_file();
        let net = file.host.net.as_mut().unwrap();
        net.session =
            "[COLOR 255]Lange\u{200B}Partie   mit einem Namen, der viel zu lang ist".into();
        net.seats[0].1 = "  [b]Anna\u{202E}\n".into();
        net.seats[1].1 = " \t ".into();
        net.seats[6].1 = String::new();
        let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        let net = back.host.net.as_ref().unwrap();
        assert_eq!(net.session, "COLOR 255Lange Partie mit einem Namen,");
        assert_eq!(net.seats[0].1, "bAnna");
        assert_eq!(net.seats[1].1, "Spieler");
        assert_eq!(net.seats[2].1, "Spieler 2");
        assert_eq!(net.seats[6].1, "");
        // `cleaned` has no effect on cleaned names.
        assert_eq!(net.clone().cleaned(), *net);
    }

    /// A network savegame requires the network flag in the start package.
    #[test]
    fn net_save_needs_the_network_flag() {
        let (file, rules) = save_file();
        assert!(!file.sim.start.setup.network);
        let mut bad = file;
        bad.host.net = net_file().0.host.net;
        bad.host.net.as_mut().unwrap().local_slot = 0;
        let message = format!("{:#}", bad.check(&rules).unwrap_err());
        assert_eq!(
            message,
            "savegame corrupted (network savegame without network flag)"
        );
    }

    /// A single-player savegame carries no network flag.
    #[test]
    fn single_save_with_the_network_flag_is_rejected() {
        let (mut file, rules) = net_file();
        file.check(&rules).unwrap();
        file.host.net = None;
        let message = format!("{:#}", file.check(&rules).unwrap_err());
        assert_eq!(
            message,
            "savegame corrupted (single-player savegame with network flag)"
        );
    }

    /// [`sim_checksum`]: only start package and state count, not the host part (each machine writes its own) and not
    /// the bytes of the container.
    #[test]
    fn sim_checksum_ignores_the_host_part() {
        let (file, _) = net_file();
        let mut other = file.clone();
        other.host.camera.fov = 1.0;
        other.host.game_speed = 3;
        other.host.net.as_mut().unwrap().local_slot = 2;
        assert_ne!(file.to_bytes(), other.to_bytes());
        let a = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        let b = GameSaveFile::from_bytes(&other.to_bytes()).unwrap();
        assert_eq!(sim_checksum(&a.sim), sim_checksum(&b.sim));
        assert_eq!(
            sim_checksum(&a.sim),
            nml_sim::save::checksum_of(&(&file.sim.start, &file.sim.state))
        );
        let mut changed = file.clone();
        changed.sim.state.tick += 1;
        assert_ne!(sim_checksum(&changed.sim), sim_checksum(&file.sim));
        let mut changed = file;
        changed.sim.start.setup.seed ^= 1;
        assert_ne!(sim_checksum(&changed.sim), sim_checksum(&a.sim));
    }

    /// The fog switch survives saving and loading (0 and [`FOG_OFF`], also any value in between, which the fog query
    /// ORs onto the flags).
    #[test]
    fn fog_show_mode_round_trips() {
        let (mut file, rules) = save_file();
        for mode in [0, 3, FOG_OFF] {
            file.host.fog_show_mode = mode;
            let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
            back.check(&rules).unwrap();
            assert_eq!(back.host.fog_show_mode, mode);
        }
    }

    fn recorded(step: u64) -> RecordedCommand {
        RecordedCommand {
            step,
            command: nml_sim::command::Command {
                player: 0,
                kind: nml_sim::command::CommandKind::Diplomacy {
                    p: 0,
                    q: 1,
                    mode: 0,
                },
            },
        }
    }

    /// Log and `SAVEREPLAY` survive saving and loading.
    #[test]
    fn save_round_trips_the_command_log_and_save_replay() {
        let (mut file, rules) = save_file();
        file.host.log = Some(vec![recorded(0), recorded(1), recorded(1), recorded(3)]);
        file.host.save_replay = true;
        let back = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.log, file.host.log);
        assert!(back.host.save_replay);
        let mut none = file;
        none.host.log = None;
        let back = GameSaveFile::from_bytes(&none.to_bytes()).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.log, None);
    }

    #[test]
    fn sim_checksum_ignores_the_log() {
        let (file, _) = save_file();
        let mut other = file.clone();
        other.host.log = Some(vec![recorded(1)]);
        other.host.save_replay = true;
        assert_ne!(file.to_bytes(), other.to_bytes());
        assert_eq!(sim_checksum(&file.sim), sim_checksum(&other.sim));
    }

    /// Combined test of the crash checks: damaged containers are errors, not panics.
    #[test]
    fn damaged_containers_are_errors_not_panics() {
        let (file, _) = save_file();
        let bytes = file.to_bytes();
        let edit = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = bytes.clone();
            f(&mut b);
            b
        };
        // Simulation part with a too short object table, validly signed.
        let mut damaged = file.clone();
        damaged.sim.state.tables[0].objects.pop();
        let cases = [
            ("11 bytes", bytes[..11].to_vec()),
            ("19 bytes", bytes[..19].to_vec()),
            (
                "foreign identifier",
                edit(&|b| b[..8].copy_from_slice(b"NMLSAVE\0")),
            ),
            (
                "other version",
                edit(&|b| b[8..12].copy_from_slice(&(FORMAT - 1).to_le_bytes())),
            ),
            ("wrong checksum", edit(&|b| *b.last_mut().unwrap() ^= 0x55)),
            (
                "one byte left over",
                with_body(&[&bytes[HEADER_LEN..], &[0]].concat()),
            ),
            ("damaged simulation part", damaged.to_bytes()),
        ];
        for (what, bytes) in cases {
            match std::panic::catch_unwind(|| GameSaveFile::from_bytes(&bytes).map(|_| ())) {
                Ok(Err(e)) => println!("{what}: {e:#}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }
}
