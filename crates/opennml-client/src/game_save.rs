//! Savegame container of a match: the savegame of the simulation ([`SaveGame`]) and the host part ([`HostPart`]:
//! camera like `m_NextFrom/At/FOV`, map description for the loading screen, game mode, name of the local player,
//! game speed, remembered speed of the cinemascope, camera of the scripts, control groups, `selectnextguid` indices,
//! idle worker index, chat and tribute masks, fixed teams, the fog switch and in network savegames the network part
//! [`NetSaveInfo`]). No influence on checksums.
//!
//! File: identifier `NMLGAME\0`, [`SAVE_FORMAT`] as u32 LE, FNV-1a 64 of the body as u64 LE, body =
//! [`encode_named`] (MessagePack with field names, zstd) of the flat `{ sim_version, sim, host, random_map }`:
//! `sim_version` is the [`FORMAT_VERSION`] of the simulation that wrote the file, `sim` the savegame of the simulation
//! itself (not its own file bytes), `random_map` the generated map of a random map match (loading builds scene and
//! rules input from it without the generator).
//!
//! Every version of the same major number loads the savegames of the earlier ones: the body of an older savegame
//! number goes through the structure steps (`upgrade`) before it is typed; after the check against the rules the
//! recompute steps run ([`GameSaveFile::prepare`]). Saving always writes the current number.
//!
//! The original also saves the game speed (re/game-ui 3).

mod upgrade;

use std::fmt;

use anyhow::{Context, anyhow};
use opennml_randommap::RandomMap;
use opennml_sim::rules::Rules;
use opennml_sim::save::codec::{decode_named, decode_value, encode_named, from_value};
use opennml_sim::save::{
    RecordedCommand, RulesCompare, SaveGame, check_state, read_container, write_container,
};
use opennml_sim::setup::FORMAT_VERSION;
use opennml_world::camera::GameCamera;
use upgrade::Step;

use crate::game::control_groups::{GroupsSave, NextGuidIndex};
use crate::game::diplomacy::Masks;
use crate::game::script_camera::ScriptCamera;
use serde::{Deserialize, Serialize};

/// Identifier of the file.
pub const GAME_MAGIC: &[u8; 8] = b"NMLGAME\0";
/// Savegame number of the container. History: 100 MessagePack with field names, zstd, flat body; 1-8 `postcard`, no
/// longer loadable: 1 initial; 2 `HostPart::cinemascope_speed`, `HostPart::script_camera`; 3 moves of the player in
/// the script camera (`MoveLeg::player`, `ScriptCamera::player_leg`), `HostPart::control_groups`,
/// `HostPart::next_guid_index`; 4 `HostPart::idle_worker_index`, `HostPart::masks`, `HostPart::lock_teams`,
/// `HostPart::fog_show_mode`; 5 menu values of the random map in `HostPart::level_info`
/// (`LevelInfoData::random_terrain`, `random_landscape`, `random_daytime`); 6 network part `HostPart::net`
/// ([`NetSaveInfo`]); 7 `NetSaveInfo` with turn spacing and lead (`spacing`, `lead`) instead of `turn_steps`;
/// 8 command log and `SAVEREPLAY` in the host part (`HostPart::log`, `HostPart::save_replay`).
pub const SAVE_FORMAT: u32 = 100;
/// First savegame number in MessagePack with field names.
pub const FIRST_SAVE_FORMAT: u32 = 100;

/// Network part of the host in a network savegame: what loading in the network needs and the simulation does not
/// know. Each machine writes its own file with the same simulation part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetSaveInfo {
    /// Name of the session (`StartInfo::session`).
    pub session: String,
    /// Own slot of this machine (0 ... 7, a human slot).
    pub local_slot: u8,
    /// Turn spacing `T` in ticks (valid as for [`opennml_net::TurnConfig::new`]: `1 <= V < T <= 40`).
    pub spacing: u8,
    /// Lead `V` in ticks.
    pub lead: u8,
    /// Slot assignment 0 ... 7: machine (`None` = no machine, AI or empty) and name.
    pub seats: Vec<(Option<u8>, String)>,
}

impl NetSaveInfo {
    /// Cleans the names on reading like names from the network: the session name with
    /// [`opennml_net::clean_session_name`], each slot name with [`opennml_net::clean_name`]. An empty slot name (slot without
    /// player, as `start_infos` writes it) stays empty instead of becoming "Spieler".
    pub fn cleaned(self) -> Self {
        Self {
            session: opennml_net::clean_session_name(&self.session),
            seats: self
                .seats
                .into_iter()
                .map(|(peer, name)| {
                    let name = if name.is_empty() {
                        name
                    } else {
                        opennml_net::clean_name(&name)
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
    opennml_sim::save::checksum_of(&(&save.start, &save.state))
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
    pub level_info: opennml_ui::draft::LevelInfoData,
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

/// Kind of a savegame that does not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveErrorKind {
    /// Another identifier or a number before [`FIRST_SAVE_FORMAT`]: too old or a different format.
    Format,
    /// A number after [`SAVE_FORMAT`]: from a newer version.
    Newer,
    /// Header incomplete, checksum, unpacking, decoding, bytes left over, structure check of the state or of the
    /// random map, or a failed structure or recompute step.
    Corrupted,
    /// Does not fit the loaded data: rules (logic part), map, map description, network part against the network
    /// flag.
    Mismatch,
}

/// A savegame that does not load: its kind and the reason. Displays the whole chain of the reason; as a
/// [`std::error::Error`] it goes on with `?` in functions with `anyhow`.
#[derive(Debug)]
pub struct SaveError {
    pub kind: SaveErrorKind,
    source: anyhow::Error,
}

impl SaveError {
    fn new(kind: SaveErrorKind, source: anyhow::Error) -> Self {
        Self { kind, source }
    }

    /// Error mapper for a reason of kind `kind`.
    fn of(kind: SaveErrorKind) -> impl FnOnce(anyhow::Error) -> Self {
        move |source| Self::new(kind, source)
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.source)
    }
}

impl std::error::Error for SaveError {}

/// Body of the file.
#[derive(Serialize, Deserialize)]
struct Body {
    /// [`FORMAT_VERSION`] of the simulation that wrote the file.
    sim_version: u32,
    sim: SaveGame,
    host: HostPart,
    /// Generated map of a random map match; `None` for fixed maps.
    #[serde(default)]
    random_map: Option<Box<RandomMap>>,
}

/// Body for writing (without a copy of the savegame, the host part and the map).
#[derive(Serialize)]
struct BodyRef<'a> {
    sim_version: u32,
    sim: &'a SaveGame,
    host: &'a HostPart,
    random_map: &'a Option<Box<RandomMap>>,
}

/// Savegame of a match.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSaveFile {
    pub sim: SaveGame,
    pub host: HostPart,
    /// Generated map of a random map match (scene and rules input when loading, [`MapSource::of_save`]); `None` for
    /// fixed maps.
    ///
    /// [`MapSource::of_save`]: crate::game::map::MapSource::of_save
    pub random_map: Option<Box<RandomMap>>,
    /// Savegame number the file was read from; [`SAVE_FORMAT`] for a new savegame. The recompute steps of
    /// [`GameSaveFile::prepare`] depend on it.
    pub loaded_format: u32,
}

/// Savegame number in the header of `bytes`: identifier and number range, before the checksum.
fn read_number(bytes: &[u8]) -> Result<u32, SaveError> {
    if bytes.get(..8) != Some(GAME_MAGIC.as_slice()) {
        return Err(SaveError::new(
            SaveErrorKind::Format,
            anyhow!("not a savegame file (identifier missing)"),
        ));
    }
    let Some(number) = bytes.get(8..12) else {
        return Err(SaveError::new(
            SaveErrorKind::Corrupted,
            anyhow!("savegame corrupted (header incomplete)"),
        ));
    };
    let number = u32::from_le_bytes(number.try_into().expect("4 bytes"));
    if number < FIRST_SAVE_FORMAT {
        return Err(SaveError::new(
            SaveErrorKind::Format,
            anyhow!(
                "savegame has format version {number}: too old or a different format (readable from {FIRST_SAVE_FORMAT})"
            ),
        ));
    }
    if number > SAVE_FORMAT {
        return Err(SaveError::new(
            SaveErrorKind::Newer,
            anyhow!(
                "savegame has format version {number} from a newer version (readable up to {SAVE_FORMAT})"
            ),
        ));
    }
    Ok(number)
}

/// Body of number `from` for the current number `current`: directly typed if they are equal, otherwise as a value
/// through the structure steps `steps` from `from` on ([`upgrade::upgrade`]) and typed afterwards.
fn decode_body(packed: &[u8], from: u32, current: u32, steps: &[Step]) -> Result<Body, SaveError> {
    let corrupted = SaveError::of(SaveErrorKind::Corrupted);
    if from == current {
        return decode_named(packed)
            .context("savegame corrupted")
            .map_err(corrupted);
    }
    let lift = || -> anyhow::Result<Body> {
        let mut value = decode_value(packed).context("savegame corrupted")?;
        upgrade::upgrade(&mut value, from, steps)
            .with_context(|| format!("savegame of format version {from} not convertible"))?;
        from_value(value).context("savegame corrupted")
    };
    lift().map_err(corrupted)
}

impl GameSaveFile {
    /// New savegame of the current number.
    pub fn new(sim: SaveGame, host: HostPart, random_map: Option<Box<RandomMap>>) -> Self {
        Self {
            sim,
            host,
            random_map,
            loaded_format: SAVE_FORMAT,
        }
    }

    /// File: header, then the flat body (always the current number [`SAVE_FORMAT`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = encode_named(&BodyRef {
            sim_version: FORMAT_VERSION,
            sim: &self.sim,
            host: &self.host,
            random_map: &self.random_map,
        });
        write_container(GAME_MAGIC, SAVE_FORMAT, &body)
    }

    /// Reads a file: identifier and savegame number (from [`FIRST_SAVE_FORMAT`] to [`SAVE_FORMAT`]), checksum of the
    /// container ([`read_container`]), the body (an older number through the structure steps), the structure check of
    /// the state ([`check_state`]) and of the random map ([`RandomMap::check`]). The network names are cleaned; the
    /// command log is dropped if the file comes from another simulation version (it replays the match from tick 0
    /// and would compute differently; [`GameSaveFile::prepare`] drops it for other rules). It matches the rules only
    /// after [`GameSaveFile::check`]; a loaded match continues only after [`GameSaveFile::prepare`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SaveError> {
        let corrupted = || SaveError::of(SaveErrorKind::Corrupted);
        let from = read_number(bytes)?;
        let packed = read_container(GAME_MAGIC, from, bytes, "savegame").map_err(corrupted())?;
        let Body {
            sim_version,
            sim,
            mut host,
            random_map,
        } = decode_body(packed, from, SAVE_FORMAT, upgrade::STEPS)?;
        check_state(&sim.state).map_err(corrupted())?;
        if let Some(map) = &random_map {
            map.check()
                .context("savegame corrupted")
                .map_err(corrupted())?;
        }
        host.net = host.net.map(NetSaveInfo::cleaned);
        if sim_version != FORMAT_VERSION {
            host.log = None;
        }
        Ok(Self {
            sim,
            host,
            random_map,
            loaded_format: from,
        })
    }

    /// Does the savegame match the loaded rules (start package, map, state: [`SaveGame::check`]; of the rules checksum
    /// only the logic part, so that the savegame loads in every edition with the same game logic), and do the network
    /// part and the network flag belong together? Loading does not check the rest of the host part: camera,
    /// speed, script camera, control groups, indices, masks, fog switch, game mode and command log are tolerated by
    /// the host with any value.
    pub fn check(&self, rules: &Rules) -> Result<(), SaveError> {
        self.sim
            .check(rules, RulesCompare::Logic)
            .map_err(SaveError::of(SaveErrorKind::Mismatch))?;
        // The network part and the network flag belong together (`SaveGame::check` keeps the flag of the state equal
        // to that of the start package).
        let why = match (&self.host.net, self.sim.start.setup.network) {
            (Some(_), false) => "savegame: network savegame without network flag",
            (None, true) => "savegame: single-player savegame with network flag",
            _ => return Ok(()),
        };
        Err(SaveError::new(SaveErrorKind::Mismatch, anyhow!(why)))
    }

    /// Makes a read savegame ready to continue on the loaded rules: [`GameSaveFile::check`], then the recompute steps
    /// for the number it was read from ([`opennml_sim::save::upgrade::recompute`]), then the start package takes over
    /// the rules checksums of this installation; if they differ, the command log is dropped. Comparisons between
    /// machines (network savegame) use the values before this step; it computes the same on every machine.
    pub fn prepare(&mut self, rules: &Rules) -> Result<(), SaveError> {
        self.check(rules)?;
        opennml_sim::save::upgrade::recompute(&mut self.sim, rules, self.loaded_format)
            .context("savegame not convertible")
            .map_err(SaveError::of(SaveErrorKind::Corrupted))?;
        // The command log replays the match from tick 0 on these rules: with other rules (another edition, other
        // speech lengths) it would compute differently.
        if self.sim.start.rules != rules.checksums {
            self.host.log = None;
        }
        self.sim.start.rules = rules.checksums;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::control_groups;
    use crate::game::fog_view::FOG_OFF;
    use crate::game::sim_host::{SimHost, plain_testkit};
    use opennml_sim::save::HEADER_LEN;

    fn host_part() -> HostPart {
        HostPart {
            camera: CameraSave {
                from: [100.0, 2000.0, -300.0],
                at: [800.0, 150.0, 900.0],
                fov: 0.75,
            },
            level_info: opennml_ui::draft::LevelInfoData {
                name: "Test map".into(),
                path: "data/levels/multiplayer/test.lvl".into(),
                size_class: 1,
                players: 2,
                level_type: 2,
                ..opennml_ui::draft::LevelInfoData::default()
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
        let file = GameSaveFile::new(host.save(), host_part(), None);
        (file, host.rules)
    }

    /// Rewrites the header: body `body` with a valid checksum (for errors behind the header).
    fn with_body(body: &[u8]) -> Vec<u8> {
        write_container(GAME_MAGIC, SAVE_FORMAT, body)
    }

    /// The file `bytes` with one MessagePack byte behind the value of the body, packed again.
    fn with_trailing_byte(bytes: &[u8]) -> Vec<u8> {
        let value = decode_value(&bytes[HEADER_LEN..]).unwrap();
        let mut plain = Vec::new();
        rmpv::encode::write_value(&mut plain, &value).unwrap();
        plain.push(0xC0);
        with_body(&zstd::bulk::compress(&plain, 3).unwrap())
    }

    /// Flat random map of 64 tiles without objects that passes [`RandomMap::check`].
    fn small_random_map() -> RandomMap {
        let spec = opennml_sim::random_map::RandomMapSpec {
            seed_text: "ZKA".into(),
            size: 0,
            players: 2,
            terrain: 0,
            landscape: 0,
            daytime: 0,
            game_type: 0,
        };
        let terrain = opennml_formats::wld::TerrainParams {
            dim_x: 64,
            dim_z: 64,
            ..opennml_formats::wld::TerrainParams::default()
        };
        RandomMap {
            spec,
            hash: 1,
            script_path: "data/randommapscripts/test.rnd".into(),
            defines: Vec::new(),
            script: opennml_formats::rnd::RandomMapScript::default(),
            dim: 64,
            terrain,
            heights: vec![0.0; 65 * 65],
            tile_numbers: vec![-1; 64 * 64],
            tiles: vec![0; 64 * 64],
            steep: vec![0; 64 * 64],
            id_grid: vec![0; 64 * 64],
            player_areas: Vec::new(),
            rand_counts: opennml_randommap::RandCounts::default(),
            rand_state: 0,
            objects: Vec::new(),
            object_block: Vec::new(),
            slots_used: [false; 8],
        }
    }

    /// Every way a file or its check can fail has its kind: identifier and too old numbers are `Format`, a higher
    /// number `Newer`, damage of the header, the packed body or the state `Corrupted`, a file of other rules
    /// `Mismatch`.
    #[test]
    fn error_kinds() {
        use SaveErrorKind::*;
        let (file, rules) = save_file();
        let bytes = file.to_bytes();
        let edit = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = bytes.clone();
            f(&mut b);
            b
        };
        let mut broken_map = file.clone();
        let mut map = small_random_map();
        map.heights.pop();
        broken_map.random_map = Some(Box::new(map));
        let cases = [
            (
                "other identifier",
                edit(&|b| b[..8].copy_from_slice(b"NMLSAVE\0")),
                Format,
            ),
            (
                "old_postcard_save_is_format_error",
                write_container(GAME_MAGIC, 8, b"old postcard body"),
                Format,
            ),
            (
                "newer number",
                edit(&|b| b[8..12].copy_from_slice(&(SAVE_FORMAT + 1).to_le_bytes())),
                Newer,
            ),
            ("truncated", bytes[..15].to_vec(), Corrupted),
            (
                "wrong checksum",
                edit(&|b| *b.last_mut().unwrap() ^= 0x55),
                Corrupted,
            ),
            ("bytes left over", with_trailing_byte(&bytes), Corrupted),
            ("inconsistent random map", broken_map.to_bytes(), Corrupted),
        ];
        for (what, bytes, kind) in cases {
            let error = GameSaveFile::from_bytes(&bytes).unwrap_err();
            assert_eq!(error.kind, kind, "{what}: {error}");
        }
        let failing: [Step; 1] = [|_| anyhow::bail!("step failed")];
        let error = decode_body(
            &bytes[HEADER_LEN..],
            FIRST_SAVE_FORMAT,
            FIRST_SAVE_FORMAT + 1,
            &failing,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, Corrupted, "failing upgrade step: {error}");
        let mut other_rules = file;
        other_rules.sim.start.rules.logic ^= 1;
        let error = other_rules.check(&rules).unwrap_err();
        assert_eq!(error.kind, Mismatch, "other logic: {error}");
    }

    /// A body of an older savegame number goes through the structure steps from its number on and comes out as the
    /// current body; saving it again writes the current number.
    #[test]
    fn older_save_is_lifted_and_saves_current() {
        let (file, _) = save_file();
        let mut value = decode_value(&file.to_bytes()[HEADER_LEN..]).unwrap();
        let speed = upgrade::take(&mut value, &["host", "game_speed"]).unwrap();
        upgrade::put(&mut value, &["host", "old_speed"], speed).unwrap();
        let older = encode_named(&value);
        let current = FIRST_SAVE_FORMAT + 1;
        assert!(decode_body(&older, current, current, &[]).is_err());
        let rename: [Step; 1] = [|v| {
            let speed = upgrade::take(v, &["host", "old_speed"])?;
            upgrade::put(v, &["host", "game_speed"], speed)
        }];
        let body = decode_body(&older, FIRST_SAVE_FORMAT, current, &rename).unwrap();
        assert_eq!(body.sim_version, FORMAT_VERSION);
        assert_eq!(
            (&body.sim, &body.host, &body.random_map),
            (&file.sim, &file.host, &file.random_map)
        );
        let lifted = GameSaveFile::new(body.sim, body.host, body.random_map);
        let bytes = lifted.to_bytes();
        assert_eq!(&bytes[8..12], &SAVE_FORMAT.to_le_bytes());
        assert_eq!(GameSaveFile::from_bytes(&bytes).unwrap(), file);
    }

    /// `prepare` checks the savegame and takes over the rules checksums of this installation (the speech part may
    /// differ, the logic part is equal). With other rules the command log is dropped, with the same it stays.
    #[test]
    fn prepare_adopts_rules_checksums() {
        let (mut file, rules) = save_file();
        file.host.log = Some(Vec::new());
        let mut same = file.clone();
        same.prepare(&rules).unwrap();
        assert_eq!(same.host.log, Some(Vec::new()));
        file.sim.start.rules.speech ^= 1;
        assert_ne!(file.sim.start.rules, rules.checksums);
        file.prepare(&rules).unwrap();
        assert_eq!(file.sim.start.rules, rules.checksums);
        assert_eq!(file.host.log, None);
    }

    /// [`MapSource::of_save`]: a file with the generated map loads the map from the file (`SavedRandom`); neither the
    /// map types nor a map script is read, map, rules input and state of the random map are those of the start. A
    /// file without it loads from the start package.
    // QUIRK D-341: deviation
    #[test]
    fn of_save_takes_the_random_map_of_the_file() {
        use crate::game::map::{MapLoad, MapSource};
        let (file, _) = save_file();
        assert_eq!(
            MapSource::of_save(&file),
            MapSource::of_start(&file.sim.start)
        );
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let types = opennml_randommap::load_types(&vfs).unwrap();
        let spec = opennml_sim::random_map::RandomMapSpec {
            seed_text: "ZKB".into(),
            size: 0,
            players: 2,
            terrain: types
                .map_scripts
                .iter()
                .position(|s| s.to_lowercase().ends_with("/continental.rnd"))
                .unwrap(),
            landscape: types
                .landscape_types
                .iter()
                .position(|l| l == "MEADOW")
                .unwrap(),
            daytime: types.daytime_types.iter().position(|d| d == "DAY").unwrap(),
            game_type: 0,
        };
        let start = MapLoad::open(&vfs, &MapSource::Random(spec)).unwrap();
        let map = start.random_map.clone().expect("generated map");
        let saved = GameSaveFile::new(file.sim, file.host, Some(map.clone()));
        let source = MapSource::of_save(&saved);
        assert_eq!(source, MapSource::SavedRandom(map));
        assert_eq!(source.label(), start.scene.lvl_path);
        let (reader, log) = vfs.recording();
        let load = MapLoad::open(&reader, &source).unwrap();
        let read: Vec<String> = log.take().into_iter().map(|e| e.path).collect();
        assert!(!read.is_empty());
        let generator =
            |p: &String| p == opennml_randommap::RANDOM_MAP_TYPES_PATH || p.ends_with(".rnd");
        assert!(!read.iter().any(generator), "{read:?}");
        assert_eq!(load.random_map, start.random_map);
        assert_eq!(load.random, start.random);
        assert_eq!(load.random_state, start.random_state);
    }

    #[test]
    fn game_save_round_trips() {
        let (file, rules) = save_file();
        let bytes = file.to_bytes();
        assert_eq!(&bytes[..8], GAME_MAGIC);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            SAVE_FORMAT
        );
        let back = GameSaveFile::from_bytes(&bytes).unwrap();
        assert_eq!(back, file);
        back.check(&rules).unwrap();
        // Flat body: version of the simulation, its savegame and the host part.
        let body: Body = decode_named(&bytes[HEADER_LEN..]).unwrap();
        assert_eq!(body.sim_version, FORMAT_VERSION);
        assert_eq!((body.sim, body.host), (file.sim, file.host));
    }

    #[test]
    fn header_carries_save_format() {
        let (file, _) = save_file();
        let bytes = file.to_bytes();
        assert_eq!(&bytes[8..12], &100u32.to_le_bytes());
    }

    /// The command log replays the match from tick 0 and only fits the simulation version that recorded it: with
    /// another `sim_version` loading drops it and keeps everything else; with the same one it stays.
    #[test]
    fn log_is_dropped_for_other_sim_version() {
        let (mut file, rules) = save_file();
        file.host.log = Some(vec![recorded(0), recorded(2)]);
        file.host.save_replay = true;
        let bytes = |sim_version: u32| {
            let body = Body {
                sim_version,
                sim: file.sim.clone(),
                host: file.host.clone(),
                random_map: None,
            };
            with_body(&encode_named(&body))
        };
        let back = GameSaveFile::from_bytes(&bytes(FORMAT_VERSION - 1)).unwrap();
        back.check(&rules).unwrap();
        assert_eq!(back.host.log, None);
        let mut expected = file.clone();
        expected.host.log = None;
        assert_eq!(back, expected);
        let same = GameSaveFile::from_bytes(&bytes(FORMAT_VERSION)).unwrap();
        assert_eq!(same, file);
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
        wrong[8..12].copy_from_slice(&(SAVE_FORMAT + 1).to_le_bytes());
        assert!(
            error(&wrong).contains(&format!("format version {}", SAVE_FORMAT + 1)),
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
        // Check against the rules: only the logic part of the rules checksum counts (other speech files load).
        let mut other_speech = file.clone();
        other_speech.sim.start.rules.speech ^= 1;
        other_speech.sim.start.rules.derived ^= 1;
        other_speech.check(&rules).unwrap();
        let mut bad = file;
        bad.sim.start.rules.logic ^= 1;
        assert!(bad.check(&rules).is_err());
    }

    /// Saving in the middle of a move with binding and rotation; after loading, the camera moves for N frames exactly
    /// as without loading.
    // QUIRK E-239: deviation
    #[test]
    fn host_part_restores_the_script_camera() {
        use crate::game::input::testkit;
        use opennml_sim::script::present::Presentation;
        let (mut camera, terrain) = testkit::setup();
        let (file, rules) = save_file();
        let host = plain_testkit::host();
        let id = host.state.objects().next().unwrap().id;
        let cameras = [opennml_formats::wld::NamedCamera {
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
        // A running and a waiting move of the player (Home) stay player moves after loading and do not lock the player
        // camera.
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
    }

    /// Idle worker index and masks survive saving and loading.
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
        assert!(back.host.lock_teams, "fixed teams");
    }

    /// The network part of the host (session, own slot, turn spacing, slot assignment) survives saving and
    /// loading.
    // QUIRK E-274: kept
    #[test]
    fn host_part_net_round_trips() {
        use opennml_sim::state::Control;
        // Network match with humans on 0 ... 2.
        let sim = crate::game::testworld::net_host(1, false);
        let mut file = GameSaveFile::new(sim.save(), host_part(), None);
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
        (GameSaveFile::new(sim.save(), host, None), sim.rules)
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
        assert_eq!(message, "savegame: network savegame without network flag");
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
            "savegame: single-player savegame with network flag"
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
            opennml_sim::save::checksum_of(&(&file.sim.start, &file.sim.state))
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
            command: opennml_sim::command::Command {
                player: 0,
                kind: opennml_sim::command::CommandKind::Diplomacy {
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
                edit(&|b| b[8..12].copy_from_slice(&(SAVE_FORMAT - 1).to_le_bytes())),
            ),
            ("wrong checksum", edit(&|b| *b.last_mut().unwrap() ^= 0x55)),
            ("one byte left over", with_trailing_byte(&bytes)),
            ("damaged simulation part", damaged.to_bytes()),
        ];
        for (what, bytes) in cases {
            match std::panic::catch_unwind(|| GameSaveFile::from_bytes(&bytes).map(|_| ())) {
                Ok(Err(e)) => println!("{what}: {e:#}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }

    /// Savegames of the releases (`testdata/savegames/*.nmlgame`), sorted by name.
    fn fixture_files() -> Vec<std::path::PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/savegames");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "nmlgame"))
            .collect();
        files.sort();
        files
    }

    /// Every savegame of a release loads with the rules of the fixture world, continues for 50 steps and writes the
    /// same savegame again. The files stay small (synthetic world, no original data).
    #[test]
    fn save_fixtures_load_and_continue() {
        let files = fixture_files();
        assert!(!files.is_empty(), "no savegame in testdata/savegames");
        let rules = fixture::rules();
        for path in files {
            let name = path.display();
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() < 200 * 1024, "{name}: {} bytes", bytes.len());
            let mut file =
                GameSaveFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            // The test shows that the bytes of a release decode, upgrade and continue, not that the synthetic test
            // rules stay byte-stable: take over their logic checksum (see `fixture`).
            file.sim.start.rules.logic = rules.checksums.logic;
            file.prepare(&rules)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut host = SimHost::new(
                rules.clone(),
                file.sim.start.setup.clone(),
                crate::game::options::DEFAULT_GAME_SPEED,
            )
            .unwrap();
            host.load(file.sim.clone(), file.host.log.clone());
            for _ in 0..50 {
                host.step_once();
            }
            assert_eq!(
                host.state.steps,
                file.sim.state.steps + 50,
                "{name}: the match continues"
            );
            let again = GameSaveFile::from_bytes(&file.to_bytes()).unwrap();
            assert_eq!(
                (&again.sim, &again.host, &again.random_map),
                (&file.sim, &file.host, &file.random_map),
                "{name}: saved again"
            );
        }
    }

    /// Writes the savegame of the fixture world to `OPENNML_SAVE_FIXTURE_OUT` (a relative path from the workspace
    /// root), for a new file in `testdata/savegames`.
    #[test]
    #[ignore = "writes a savegame file"]
    fn write_save_fixture() {
        let out = std::env::var_os("OPENNML_SAVE_FIXTURE_OUT").expect("OPENNML_SAVE_FIXTURE_OUT");
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(out);
        let bytes = fixture::save_file().to_bytes();
        std::fs::write(&out, &bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        println!("{}: {} bytes", out.display(), bytes.len());
    }

    /// The fixture world has every kind of state its writer promises.
    #[test]
    fn fixture_world_has_every_state_kind() {
        let file = fixture::save_file();
        let state = &file.sim.state;
        assert_eq!(state.steps, fixture::STEPS);
        assert!(state.objects().any(|o| o.guid == 1000), "building");
        assert!(!state.groups.map.is_empty(), "group");
        let mut next = state.clone();
        opennml_sim::tick::step(&mut next, &fixture::rules(), &[]);
        opennml_sim::tick::step(&mut next, &fixture::rules(), &[]);
        let position = |s: &opennml_sim::state::SimState| -> Vec<[f32; 3]> {
            s.objects()
                .filter(|o| o.id.slot() == 0 && o.guid == 1100)
                .map(|o| o.world_pos)
                .collect()
        };
        assert_ne!(position(&next), position(state), "walking units");
        assert!(!state.projectiles.list.is_empty(), "projectile");
        let engine = state.ki.engines[1].as_ref().expect("AI engine of slot 1");
        assert!(!engine.threads.is_empty(), "AI thread");
        assert!(state.script.level.is_some(), "script container");
        assert!(
            file.host.log.as_ref().is_some_and(|log| !log.is_empty()),
            "command log"
        );
        assert!(file.host.control_groups[0][1].len() > 1, "control group");
        assert_eq!(
            file.sim.start.level, "synthetisch",
            "level path of the synthetic rules"
        );
    }

    /// Synthetic world of the savegame files in `testdata/savegames`: the world of `plain_testkit` (colonist 1100 with
    /// walk and stand, headquarters 1000 as a building, 16 x 16 tiles) as a match of two slots (slot 0 human, slot 1
    /// AI) with a small AI catalog and a level script. No original data.
    ///
    /// The files are frozen after their release and must keep loading with [`rules`]: do not change the rules of this
    /// world (types, slots, AI catalog, script), only what the writer does. The loading test takes over the logic
    /// checksum of [`rules`] before `prepare`: the rules build on `Rules::synthetic` and `script::testing::rules_with`,
    /// which other tests share, and a later change there (or in the checksum) must not make a frozen file
    /// unloadable while its format is still compatible. Level path, map size and the rest of the check still apply.
    mod fixture {
        use super::*;
        use crate::game::options::DEFAULT_GAME_SPEED;
        use opennml_sim::command::{Command, CommandKind, NO_SPREAD};
        use opennml_sim::id::ObjectId;
        use opennml_sim::ki::catalog::{KiCatalog, KiPrototype};
        use opennml_sim::ki::start::{SET_LEVEL, START_PLAYER};
        use opennml_sim::ki::vars::VarTable;
        use opennml_sim::ki::vm::op;
        use opennml_sim::setup::{LobbyEntry, MatchSetup, SlotKind, ki_kind_handicap};
        use opennml_sim::sim::Sim;
        use opennml_sim::world::{ObjectDesc, create_object, desc_flags};

        /// The types of `plain_testkit` (copied: the rules of the frozen files must stay).
        const TYPES: &str = concat!(
            "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
            "GUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\n",
        );
        const COLONIST: u32 = 1100;
        const HQ: u32 = 1000;

        /// Level script: the first block sets a switch, a timer and a variable and then waits (suspended container);
        /// the second block counts the variable up in every tick once the switch is set.
        const SCRIPT: &str = concat!(
            "{\r\n\t[BESCHREIBUNG=start]\r\n",
            "\t[AKTION=n12;started:Text]\r\n",
            "\t[AKTION=n25;clock;600000:Text]\r\n",
            "\t[AKTION=n360;counter;5:Text]\r\n",
            "\t[AKTION=w40;600000:Text]\r\n",
            "}\r\n",
            "{\r\n\t[BESCHREIBUNG=count]\r\n",
            "\t[BEDINGUNG=&5;started:Text]\r\n",
            "\t[AKTION=n10:Text]\r\n",
            "\t[AKTION=n361;counter;1:Text]\r\n",
            "}\r\n",
        );

        /// Steps of the match up to the savegame.
        pub const STEPS: u64 = 200;

        /// Rules of the world.
        pub fn rules() -> Rules {
            let word = |op: u32, operand: u32| (op << 28) | operand;
            let proto = |id, name: &str, code: Vec<u32>| KiPrototype {
                id,
                name: name.into(),
                params: 0,
                code,
                vars: VarTable::from_module(&[(0, 8)], &[]),
                checksum: u64::from(id),
            };
            let mut r = Rules::synthetic(TYPES, 16);
            for slot in 0..2u8 {
                r.level.players.push(opennml_formats::config::LevelPlayer {
                    slot,
                    name: format!("P{slot}"),
                    nation: i32::from(slot) + 1,
                    kind: i32::from(slot),
                    resources: Vec::new(),
                });
            }
            // `setschwierigkeitsgrad` sets value 302; `multi_startplayer` sets value 500 and then pauses 500 ms in a
            // loop (thread with a pause counter).
            r.ki = KiCatalog::from_prototypes([
                proto(
                    SET_LEVEL,
                    "setschwierigkeitsgrad",
                    vec![
                        word(op::PUSHA, 302),
                        word(op::PUSHA, 1000),
                        word(op::CALLB, 0x1E),
                    ],
                ),
                proto(
                    START_PLAYER,
                    "multi_startplayer",
                    vec![
                        word(op::PUSHA, 500),
                        word(op::PUSHA, 1),
                        word(op::CALLB, 0x1E),
                        word(op::PUSHA, 500),
                        word(op::CALLB, 0x8E),
                        word(op::JMP, 3),
                    ],
                ),
            ]);
            r.scripts = opennml_sim::script::testing::rules_with(SCRIPT, &[], &[]);
            assert!(r.scripts.errors.is_empty(), "{:?}", r.scripts.errors);
            r
        }

        fn setup() -> MatchSetup {
            let entry = |slot: i32, kind, handicap| LobbyEntry {
                kind,
                name: format!("P{slot}"),
                nation: slot + 1,
                team: slot,
                color: slot,
                handicap,
            };
            let mut setup = MatchSetup {
                mode: 0,
                seed: 7,
                lobby: vec![
                    entry(0, SlotKind::Human, 0),
                    entry(1, SlotKind::Ai, ki_kind_handicap(3).unwrap()),
                ],
                ..MatchSetup::default()
            };
            setup.ki.enabled = true;
            setup.script.enabled = true;
            setup
        }

        fn spawn(host: &mut SimHost, guid: u32, slot: i32, at: [f32; 2]) -> ObjectId {
            let rules = host.rules.clone();
            let mut sim = Sim::new(&mut host.state, &rules);
            let mut desc = ObjectDesc::new(guid, slot, [at[0], 0.0, at[1]]);
            desc.flags |= desc_flags::PLACED;
            create_object(&mut sim, &mut desc).unwrap()
        }

        /// The match after [`STEPS`] steps with the host part: buildings of both slots, four colonists of slot 0 that
        /// walk as a group across the map, an arrow in flight and a dynamite charge (placed directly, not by a
        /// command, so the command log leads to the state only without them), the AI thread of slot 1, the level
        /// container of the script; camera, a control group, `selectnextguid` index, idle worker index, masks, the
        /// complete command log.
        pub fn save_file() -> GameSaveFile {
            let mut host = SimHost::new(rules(), setup(), DEFAULT_GAME_SPEED).unwrap();
            host.active_player = 0;
            let hq = spawn(&mut host, HQ, 0, [640.0, 64.0]);
            let colonists: Vec<ObjectId> = (0..4)
                .map(|i| spawn(&mut host, COLONIST, 0, [64.0 + 128.0 * i as f32, 192.0]))
                .collect();
            spawn(&mut host, HQ, 1, [1600.0, 1600.0]);
            let shooter = spawn(&mut host, COLONIST, 1, [1600.0, 1200.0]);
            for step in 0..STEPS {
                match step {
                    5 => host.submit(Command {
                        player: 0,
                        kind: CommandKind::Move {
                            units: colonists.clone(),
                            x: 14 * 4096,
                            z: 14 * 4096,
                            target: -1,
                            formation: 0,
                            direction: opennml_sim::motion::NO_DIRECTION,
                            spread: NO_SPREAD,
                            flags: 0,
                        },
                    }),
                    150 => {
                        let rules = host.rules.clone();
                        let mut sim = Sim::new(&mut host.state, &rules);
                        let pos = [640.0, 0.0, 64.0];
                        opennml_sim::combat::projectile::add_dynamite(
                            &mut sim, shooter, hq, pos, 3,
                        );
                    }
                    190 => {
                        let rules = host.rules.clone();
                        let mut sim = Sim::new(&mut host.state, &rules);
                        let launch = opennml_sim::combat::projectile::ArrowLaunch {
                            shooter,
                            target: hq,
                            damage: opennml_sim::combat::damage::explicit_damage_message(
                                -12, shooter, COLONIST,
                            ),
                            start: [1600.0, 0.0, 1200.0],
                            target_pos: [640.0, 40.0, 64.0],
                            zone: 0,
                            fire: false,
                            model: String::new(),
                            node: String::new(),
                        };
                        opennml_sim::combat::projectile::launch_arrow(&mut sim, launch);
                    }
                    _ => {}
                }
                host.step_once();
            }
            let mut groups = crate::game::control_groups::empty_save();
            groups[0][1] = colonists.iter().map(|id| id.0).collect();
            groups[0][2] = vec![hq.0];
            let mut next_guid_index = crate::game::control_groups::empty_next_guid_index();
            next_guid_index[0].insert(COLONIST as i32, 2);
            let mut masks = Masks::initial();
            masks.chat[0] |= 0b10;
            let host_part = HostPart {
                camera: CameraSave {
                    from: [1024.0, 1800.0, -400.0],
                    at: [1024.0, 0.0, 1024.0],
                    fov: 0.8,
                },
                level_info: opennml_ui::draft::LevelInfoData {
                    name: "Synthetic".into(),
                    path: host.rules.level_path.clone(),
                    size_class: 0,
                    players: 2,
                    level_type: 2,
                    ..opennml_ui::draft::LevelInfoData::default()
                },
                game_type: 0,
                local_name: "P0".into(),
                game_speed: host.game_speed,
                cinemascope_speed: None,
                script_camera: ScriptCamera::default(),
                control_groups: groups,
                next_guid_index,
                idle_worker_index: [1, 0, 0, 0, 0, 0, 0, 0],
                masks,
                lock_teams: false,
                fog_show_mode: 0,
                net: None,
                log: host.save_log(),
                save_replay: true,
            };
            GameSaveFile::new(host.save(), host_part, None)
        }
    }
}
