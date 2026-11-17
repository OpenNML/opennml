//! Tests of the diplomacy dialog in the flow: opening and closing through [`UiRequest::ToggleDiplomacy`]
//! (`FUN_0042E060`), the match stands still while the dialog is open, commands of the dialog take effect at once and
//! the replay equals the live run; masks and locked teams through saving and loading. With the fake `FakeSession`
//! over the corpus dialogs (`NML_GAME_DIR`, otherwise silently skipped).

use nml_ui::screens::DiplomacyAction;

use super::tests::{Run, S1024, request};
use super::*;
use crate::game::diplomacy::Masks;
use crate::game::hotkeys::HotkeyCommand;

const DIALOG: &str = "diplomatie";

/// Left click without effect on the fake: fetches the requests of the match like every input.
fn poke(r: &mut Run) {
    let click = InputEvent::CursorMoved { x: 5.0, y: 5.0 };
    r.flow
        .input(&click, S1024, r.now, &mut r.host.app, &mut r.view);
}

/// Free play on the map of the installation (two slots), up to the first step.
fn started(real_levels: bool) -> Option<Run> {
    let mut r = Run::corpus()?;
    r.view.real_levels = real_levels;
    r.start();
    r.until_step();
    Some(r)
}

/// Game type 0, no lock - the request (key `SHIFT D` = `xbutton(572)`, button 580) opens at once (state 10,
/// `DIPLOMATIE`), the match stands still; the same request closes. Under the lock (`Engine+0x600`) nothing opens, and
/// an open dialog stays open when toggled; "Back" closes.
#[test]
fn open_close_d5() {
    let Some(mut r) = started(false) else {
        return;
    };
    // `SHIFT D` reaches the match as `xbutton(572)` (`keys.ini`); the fake only records it.
    r.key("SHIFT D");
    r.frame();
    assert!(
        r.view.hotkeys.contains(&HotkeyCommand::XButton(572)),
        "{:?}",
        r.view.hotkeys
    );
    let steps = r.view.steps();
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    assert_eq!(r.state(), 10, "at once after the input");
    assert!(r.view.ui_requests.is_empty());
    assert_eq!(r.idle(), 0, "the match stands still");
    assert_eq!(r.view.steps(), steps);
    assert_eq!(r.dialogs(), [DIALOG]);
    assert!(r.view.menu_open, "menu state != 0 to the match");
    // Quicksave is locked at state 10 (`FUN_0042F490`), ESC opens no game menu.
    let queued = r.host.app.queue().len();
    r.host.app.quick_save("quicksave1");
    assert_eq!(r.host.app.queue().len(), queued);
    // State of the dialog from the host: local slot 0, without locked teams.
    let view = r.host.app.data().diplomacy.clone().expect("state");
    assert_eq!((view.local, view.locked_teams), (0, false));
    // Toggling closes; the match runs again.
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    assert_eq!(r.state(), 0);
    assert!(r.idle() > 0, "runs again");
    assert!(r.dialogs().is_empty());
    // Lock: nothing.
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    assert_eq!(r.state(), 0, "do not open under the lock");
    r.idle();
    assert!(r.dialogs().is_empty());
    // Open and then locked: toggling does not close, "Back" does.
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = false;
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    r.idle();
    r.view.sim_mut().unwrap().state.script.mission.ui_locked = true;
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    assert_eq!(r.state(), 10);
    r.click(DIALOG, "back");
    r.idle();
    assert_eq!(r.state(), 0);
    assert!(r.dialogs().is_empty());
}

/// Dialog open (match stopped), set shared vision, status and tribute - the commands are in the state and in the log at
/// once, quicksave (F1) stays locked; dialog closed, play on; the replay of the log gives the same checksum as the
/// live run.
#[test]
fn diplomacy_commands_while_halted_replay_like_live() {
    let Some(mut r) = started(true) else {
        return;
    };
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    r.idle();
    assert_eq!(r.dialogs(), [DIALOG]);
    let sim = r.view.sim().unwrap();
    assert!(sim.halted, "stopped: commands at once");
    let (steps, logged) = (sim.state.steps, sim.log.len());
    // The map occupies slots 0 (human, local) and 2 (computer); the others are unoccupied (result 3).
    let results: Vec<i32> = r
        .host
        .app
        .data()
        .diplomacy
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.result)
        .collect();
    assert_eq!(results, [0, 3, 0, 3, 3, 3, 3, 3]);
    // Clicks on the checkmarks and buttons of the dialog (row 2 = the computer): shared vision on, tribute bit on,
    // send gold.
    r.click(DIALOG, "view2");
    r.idle();
    r.click(DIALOG, "tribut2");
    r.idle();
    assert_eq!(r.view.masks.tribute[0], 0b101, "local tribute bit");
    r.click(DIALOG, "tributgold");
    r.idle();
    // The status selection does not reach the rows in the control channel: as an effect like from the combo box.
    r.apply(UiEffect::Diplomacy(DiplomacyAction::Status {
        slot: 2,
        mode: 0,
    }));
    r.key("F1");
    r.idle();
    let sim = r.view.sim().unwrap();
    assert_eq!(sim.state.steps, steps, "no tick");
    assert!(sim.pending_commands().is_empty());
    let kinds: Vec<&CommandKind> = sim.log[logged..].iter().map(|c| &c.command.kind).collect();
    assert_eq!(
        kinds,
        [
            &CommandKind::SharedVision {
                p: 2,
                q: 0,
                on: true
            },
            &CommandKind::Tribute {
                from: 0,
                recipients: 0b101,
                resource: 0,
                amount: 100
            },
            &CommandKind::Diplomacy {
                p: 0,
                q: 2,
                mode: 0
            },
        ]
    );
    assert!(sim.log[logged..].iter().all(|c| c.step == steps));
    assert!(sim.state.diplomacy.shares_vision(2, 0));
    assert!(sim.state.diplomacy.is_friend(0, 2));
    assert!(
        !std::fs::exists(r.files.path("quicksave1", 0)).unwrap(),
        "F1 at state 10 without effect"
    );
    // Closed, continue, replay.
    r.view.ui_requests.push(UiRequest::ToggleDiplomacy);
    poke(&mut r);
    assert_eq!(r.state(), 0);
    for _ in 0..40 {
        r.frame();
    }
    let live = r.view.sim().unwrap();
    assert!(live.state.steps > steps);
    let replay = live.replay().unwrap();
    let mut other = SimHost::new(live.rules.clone(), live.setup.clone(), live.game_speed).unwrap();
    other.load_replay(replay, Some(live.state.steps)).unwrap();
    assert_eq!(other.checksum(), live.checksum(), "replay like live");
}

/// Masks (bits of dropped-out slots included), locked teams and the fog switch survive saving and loading.
#[test]
fn mask_bits_survive_save_and_load() {
    let Some(mut r) = Run::corpus() else {
        return;
    };
    let mut req = request(0);
    req.settings.lock_teams = true;
    r.apply(UiEffect::StartMatch(req));
    r.flow.begin_round();
    r.flow.begin_round();
    r.until_step();
    assert!(r.host.app.data().diplomacy.as_ref().unwrap().locked_teams);
    r.view.masks.chat[0] = 0b1001_0101;
    r.view.masks.tribute[0] = 0b0000_0111;
    r.view.masks.tribute[5] = 0xFF;
    // The fog switch of the host (`rainbowring`) goes into the host part and back.
    r.view
        .sim_mut()
        .unwrap()
        .set_fog_show_mode(crate::game::fog_view::FOG_OFF);
    let host = r.flow.host_part(&r.host.app, &r.view).unwrap();
    assert!(host.lock_teams);
    assert_eq!(host.masks, r.view.masks);
    assert_eq!(host.fog_show_mode, crate::game::fog_view::FOG_OFF);
    r.apply(UiEffect::SaveGame {
        name: "diplo".into(),
    });
    let saved = r.view.masks;
    r.view.masks = Masks::initial();
    r.apply(UiEffect::LoadGame {
        name: "diplo".into(),
    });
    assert!(r.flow.running());
    assert_eq!(r.view.masks, saved);
    assert_eq!(
        r.view.sim().unwrap().fog.show_mode,
        crate::game::fog_view::FOG_OFF
    );
    r.frame();
    let view = r.host.app.data().diplomacy.clone().unwrap();
    assert!(view.locked_teams, "locked teams from the host part");
    assert!(view.rows[2].chat && !view.rows[1].chat, "bits of the mask");
}
