//! Replaying a recording in the game (re/multiplayer-saves S.14.3, S.14.5): `LOADAUTOPLAY` -> `START` -> `{0x19}`
//! ([`UiEffect::PlayReplay`](opennml_ui::msg::UiEffect)) -> [`GameFlow::start_replay`] (`FUN_004C9000` -> `FUN_004C8B80`).
//!
//! - **Checks before the menu level ends** (deviates from E-343): read the file, identifier `NMLAUTO` (`opennml-game` does
//!   not replay an `NMLREPL` replay of the viewer), file edition, `FORMAT_VERSION`, header and game type >= 0. If this
//!   fails, the menu level keeps running, `LOADAUTOPLAY` opens again and over it the own message 99 109
//!   ([`REPLAY_NOT_PLAYABLE`]), plus a log warning with the reason. The original has already ended the menu level at
//!   that point and stays on a black screen.
//! - **Start** like a new single-player match from the start package (`Game_StartLevel`): menu level off, background
//!   images gone, loading screen of the map (`ldg_path_for(…, false, …)`, random map `randommap.ldg`), game speed from
//!   the header (E-340), no network session, no recording (`SaveReplay` 0). Whether the start package fits the game
//!   data does not show until the rules of the loaded map are known ([`replay_setup`]); if that fails, it is a failed
//!   start: `LOADAUTOPLAY` with 99 109, the menu level is off and `START` restarts it.
//! - **Replay** ([`SessionView::begin_playback`]): the commands of the file at their tick, fog off, selection locked,
//!   command bar hidden, view slot `first_player`; watching only (F1, F4, 508, saving, loading and restart have no
//!   effect, surrendering works). A truncated file replays up to the last whole section (warning).

use anyhow::{Context, bail};
use opennml_sim::rules::Rules;
use opennml_sim::save::REPLAY_MAGIC;
use opennml_sim::setup::{MatchSetup, StartPackage, start_package};
use opennml_ui::app::MenuApp;
use opennml_ui::loading::{RANDOM_MAP_LDG, ldg_path_for};
use opennml_ui::msg::AppCmd;
use opennml_ui::own_texts::REPLAY_NOT_PLAYABLE;
use opennml_ui::saves::SaveDir;

use super::{FlowEnv, GameFlow, MatchMeta, SessionView, active_guid, check_name};
use crate::game::map::MapSource;
use crate::game_session::LoadPhase;
use crate::replay_file::ReplayFile;

impl GameFlow {
    /// Is a replay running (`GameApp+0x19F3`)?
    pub fn playing_back(&self) -> bool {
        self.meta.as_ref().is_some_and(|m| m.playback.is_some())
    }

    /// `{0x19}` (`FUN_004C9000` -> `FUN_004C8B80`): play the recording `savegame/autoplay/<name>` (see module
    /// documentation). A running recording ends with the successful start without catching up
    /// ([`Self::apply`] has caught up beforehand).
    pub(super) fn start_replay(
        &mut self,
        name: &str,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
        present: &mut dyn FnMut(&str, LoadPhase),
    ) {
        let file = match read_replay(name, app, env) {
            Ok(file) => file,
            Err(e) => return self.replay_failed(name, &e, false, app, view, env),
        };
        let ReplayFile {
            head,
            commands,
            torn,
        } = file;
        if torn {
            log::warn!(
                "Recording \"{name}\" is truncated: replaying up to the last whole section ({} commands up to tick {})",
                commands.len(),
                commands.last().map_or(0, |c| c.step)
            );
        }
        // Every start through `Game_StartLevel` ends the menu level before loading.
        self.end_menu_level(app, view);
        self.close_menus(app);
        let source = MapSource::of_start(&head.start);
        let level = source.label();
        // Random map like the start of a new single-player match: `randommap.ldg`, otherwise the loading screen of the
        // map.
        let ldg = match &source {
            MapSource::Random(_) => RANDOM_MAP_LDG.to_string(),
            MapSource::Level(path) => {
                ldg_path_for(path, head.meta.game_type, false, |p| env.vfs.exists(p))
            }
        };
        let speed = head.meta.game_speed;
        view.apply_settings(app.config());
        let start = &head.start;
        let mut loaded = false;
        let result = self
            .load(&ldg, present, |progress| {
                view.start(
                    env.vfs,
                    &source,
                    &mut |rules| replay_setup(rules, start),
                    speed,
                    progress,
                )
            })
            .and_then(|()| {
                loaded = true;
                view.sim()
                    .map(|_| ())
                    .context("the map has no simulation (rules not loadable)")
            });
        if let Err(e) = result {
            if loaded {
                // The session without a simulation has already replaced an old match: it is over as well.
                view.end();
                if self.meta.take().is_some() {
                    self.recorder = None;
                    app.set_game(None);
                    app.set_mission(None);
                    app.set_diplomacy(None);
                }
            }
            return self.replay_failed(name, &e, true, app, view, env);
        }
        let count = commands.len();
        let last = commands.last().map_or(0, |c| c.step);
        view.begin_playback(commands);
        let meta = MatchMeta {
            level_info: head.meta.level_info,
            game_type: head.meta.game_type,
            lock_teams: head.meta.lock_teams,
            save_replay: false,
            playback: Some(name.to_string()),
        };
        self.started(meta, app, view);
        // An old recording ends here; the replay itself never records (`save_replay` off).
        self.restart_recording(view, env);
        log::info!(
            "Replay \"{name}\" started: {level}, {count} commands up to tick {last}, speed {speed}"
        );
    }

    /// Replay not started (deviates from E-343): log warning with the reason, `LOADAUTOPLAY` open again (`START` closed
    /// it with `{3}`) and the message 99 109 over it. After a load attempt without a match also the menu music that
    /// `start` stopped.
    fn replay_failed(
        &self,
        name: &str,
        error: &anyhow::Error,
        after_load: bool,
        app: &mut MenuApp,
        view: &mut dyn SessionView,
        env: &FlowEnv,
    ) {
        log::warn!("Recording \"{name}\" not played: {error:#}");
        if after_load && self.meta.is_none() {
            view.restore_menu_music(env.vfs);
        }
        app.push_command(AppCmd::Open {
            name: "loadautoplay".to_string(),
            wait: true,
        });
        app.tell(REPLAY_NOT_PLAYABLE, None);
    }
}

/// Reads and checks the recording before loading: name, file `savegame/autoplay/<name>` (full name, E-342),
/// identifier, edition, `FORMAT_VERSION` and header ([`ReplayFile::from_bytes`]), game type >= 0 (a campaign or a menu
/// level would have campaign menus and restart). An `NMLREPL` replay of the viewer carries neither game type nor map
/// info nor game speed.
fn read_replay(name: &str, app: &MenuApp, env: &FlowEnv) -> anyhow::Result<ReplayFile> {
    check_name(name).map_err(anyhow::Error::msg)?;
    let path = env.files.path_in(SaveDir::Autoplay, name, active_guid(app));
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.starts_with(REPLAY_MAGIC) {
        bail!(
            "replay of the viewer (identifier NMLREPL) without game type, map info and game speed; opennml-game only plays NMLAUTO"
        );
    }
    let file = ReplayFile::from_bytes(&bytes)?;
    let game_type = file.head.meta.game_type;
    if game_type < 0 {
        bail!("game type {game_type} (campaign or menu level) cannot be played back");
    }
    Ok(file)
}

/// Setup of the replay from the rules of the map that was just loaded: the start package computed again
/// ([`start_package`]) must equal the header - different game data or a different map would not compute the same.
/// Then the setup of the recording unchanged (seed, slots, names as recorded, deviates from E-338).
fn replay_setup(rules: &Rules, start: &StartPackage) -> anyhow::Result<MatchSetup> {
    if start_package(rules, &start.setup) != *start {
        bail!(
            "the recording does not fit the game data (start package differs, rules checksum {:016x} instead of {:016x})",
            rules.checksum,
            start.rules_checksum
        );
    }
    Ok(start.setup.clone())
}
