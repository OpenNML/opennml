//! Tests of the script presentation in the game host. GPU-free: fog crossfade, selection of the game host at
//! cinemascope and lock; with installation and graphics card (otherwise silently skipped): start tables, defaults of
//! the viewer and the re-creation of all channels after loading.

use super::*;
use crate::game::script_camera::ScriptCamera;
use crate::game::sim_host::SimHost;
use crate::game::testworld::abl;
use crate::game_save::{CameraSave, HostPart};
use crate::game_session::{GameSession, SessionGpu};
use nml_audio::music::MusicStatus;
use nml_render::BackendChoice;
use nml_sim::command::CommandKind;
use nml_sim::rules::Rules;
use nml_sim::setup::MatchSetup;
use nml_sim::state::SimEvent;

/// Byte `shift` of an ARGB color.
fn channel(color: u32, shift: u32) -> u32 {
    (color >> shift) & 0xFF
}

// QUIRK E-237: kept
#[test]
fn fog_506_fades_in_game_time_and_swaps() {
    // E-237 (`FUN_0042F200`/`FUN_0042F380`): 506 crossfades in game time (frame time * (speed + 10) / 20) and writes
    // byte 0 of the color to red: red and blue swap, alpha becomes FF. 505 takes the number unchanged.
    assert_eq!(game_ms(100.0, 20), 150.0);
    assert_eq!(game_ms(100.0, 10), 100.0);
    let level = FogValues::of_level(None, -100.0);
    assert_eq!(
        level,
        FogValues {
            color: 0,
            start: -100.0,
            end: 1000.0
        }
    );
    // Shipped `kamp1-2.abl`: 5192239 = 0x4F3A2F.
    let color = 0x4F_3A2F;
    let mut values = level;
    let mut fade = FogFade::new(values, color, 2000, 5000, 1000);
    // Speed 20, frames of 100 ms: 150 ms of game time per frame.
    let dt = game_ms(100.0, 20);
    assert!(!fade.advance(&mut values, dt));
    assert_eq!(channel(values.color, 24), 0xFF, "alpha FF");
    assert_eq!(
        channel(values.color, 16),
        0x2F * 15 / 100,
        "red from byte 0"
    );
    assert_eq!(channel(values.color, 8), 0x3A * 15 / 100);
    assert_eq!(
        channel(values.color, 0),
        0x4F * 15 / 100,
        "blue from byte 2"
    );
    assert!((values.start - 215.0).abs() < 1e-3, "{}", values.start);
    assert!((values.end - 1600.0).abs() < 1e-3, "{}", values.end);
    let mut frames = 1;
    while !fade.advance(&mut values, dt) {
        frames += 1;
    }
    assert_eq!(
        frames + 1,
        7,
        "1000 ms of game time in seven frames of 150 ms"
    );
    let target = FogValues::of_memory(&FogMemory {
        color,
        start: 2000,
        end: 5000,
        swap_red_blue: true,
    });
    assert_eq!(target.color, 0xFF2F_3A4F);
    for shift in [0, 8, 16, 24] {
        let (got, want) = (channel(values.color, shift), channel(target.color, shift));
        assert!(
            got.abs_diff(want) <= 1,
            "channel {shift}: {got:#x} instead of {want:#x}"
        );
    }
    assert!((values.start - 2000.0).abs() < 0.5 && (values.end - 5000.0).abs() < 0.5);
    // 505: the same number without swap (brown instead of blue-gray).
    let direct = FogValues::of_memory(&FogMemory {
        color,
        start: 2000,
        end: 5000,
        swap_red_blue: false,
    });
    assert_eq!(direct.color, 0x004F_3A2F);
    assert_eq!(direct.params().color, [0x4F, 0x3A, 0x2F]);
    assert_eq!(target.params().color, [0x2F, 0x3A, 0x4F]);
    // Already in the first frame the start color appears swapped (also for the same color).
    let mut same = direct;
    let mut fade = FogFade::new(same, color, 2000, 5000, 1000);
    fade.advance(&mut same, 1.0);
    assert_eq!(channel(same.color, 16), 0x2F);
    assert_eq!(channel(same.color, 0), 0x4F);
}

/// Host with the level script `level` and the script switched on, without objects.
fn script_host(level: &str) -> SimHost {
    let rules = crate::game::testworld::script_rules(level);
    let mut setup = MatchSetup::default_for(&rules);
    setup.script.enabled = true;
    SimHost::new(rules, setup, crate::game::options::DEFAULT_GAME_SPEED).unwrap()
}

/// A frame like `GameSession::advance_by` without a map: report the selection, one step, events to the game host.
fn frame(host: &mut SimHost, p: &mut ScriptPresentation, selection: &mut Selection) {
    host.sync_selection(&selection.ids.clone());
    for event in host.step_once() {
        if let SimEvent::Present(e) = event {
            p.apply_host(&e, host, selection);
        }
    }
}

// QUIRK E-238, E-246: kept
#[test]
fn cinemascope_and_lock_clear_the_host_selection() {
    // E-230: 147/148 and 18 without cinemascope clear the selection of the game host together with the mirror;
    // otherwise `sync_selection` would report the old IDs again in the next frame, and condition 35 would become true
    // again in cinemascope. 18 in cinemascope clears nothing (the bar is off).
    let (a, b) = (ObjectId(0x800), ObjectId(0x801));
    let level = abl(&[
        (&["&35;x"], &["n10"]),
        (
            &[],
            &[
                "n147", "w40;200", "n18", "w40;200", "n148", "n19", "w40;200", "n18",
            ],
        ),
    ]);
    let mut host = script_host(&level);
    assert!(host.rules.scripts.uses_ui_conditions);
    let mut p = ScriptPresentation::new(&ViewOptions::default());
    let mut selection = Selection::of(vec![a, b]);
    // Step 1: the mirror gets [a, b], 147 clears it and the selection of the game host.
    frame(&mut host, &mut p, &mut selection);
    assert!(p.cinemascope);
    assert!(selection.ids.is_empty(), "game host cleared");
    assert!(host.state.script.mission.selection.is_empty());
    for _ in 0..2 {
        frame(&mut host, &mut p, &mut selection);
        assert!(
            host.state.script.mission.selection.is_empty(),
            "35 reads the empty mirror"
        );
    }
    assert!(
        !host
            .pending_commands()
            .iter()
            .any(|c| matches!(c.kind, CommandKind::ScriptSelection { .. })),
        "no new selection command"
    );
    // In cinemascope the player (control channel) selects anew; 18 leaves the selection.
    selection.ids = vec![a];
    let until = host.state.steps + 10;
    while !host.state.script.mission.ui_locked {
        frame(&mut host, &mut p, &mut selection);
        assert!(host.state.steps < until, "18 comes");
    }
    assert_eq!(selection.ids, [a], "18 in cinemascope clears nothing");
    // 148 clears; then 19, and 18 without cinemascope clears again.
    selection.ids = vec![b];
    while p.cinemascope || host.state.script.mission.ui_locked {
        frame(&mut host, &mut p, &mut selection);
    }
    assert!(selection.ids.is_empty(), "148 clears");
    selection.ids = vec![a, b];
    let until = host.state.steps + 10;
    while !host.state.script.mission.ui_locked {
        frame(&mut host, &mut p, &mut selection);
        assert!(host.state.steps < until, "second 18 comes");
    }
    assert!(selection.ids.is_empty(), "18 without cinemascope clears");
    frame(&mut host, &mut p, &mut selection);
    assert!(host.state.script.mission.selection.is_empty());
}

#[test]
fn highlight_is_kept_only_for_an_existing_object() {
    // A highlight only for an existing object; a dead ID is silently ignored.
    let mut host = crate::game::sim_host::plain_testkit::host();
    let id = host.state.objects().next().unwrap().id;
    let mut p = ScriptPresentation::new(&ViewOptions::default());
    let mut selection = Selection::default();
    p.apply_host(
        &Presentation::Highlight { id: Some(id) },
        &mut host,
        &mut selection,
    );
    assert_eq!(p.highlight, Some(id));
    p.apply_host(
        &Presentation::Highlight {
            id: Some(ObjectId(0x7777)),
        },
        &mut host,
        &mut selection,
    );
    assert_eq!(p.highlight, None);
    assert!(matches!(
        p.recent.back(),
        Some(Presentation::Highlight { .. })
    ));
}

#[test]
fn music_nation_follows_the_lobby_choice_not_the_level_file() {
    // `Game_StartNationMusic([GetLocalPlayer()+0x238])` (`0x00430153`, `0x004C7828`) reads the nation after
    // `SetupPlayersFromLobby` (`0x00432E6A`) - the choice in the start dialog, not the nation of the `.lvl`.
    let level = nml_formats::lvl::parse_level(
        "LEVELFILE V1.08e\r\nPROJECT_FILE \"x.prj\"\r\n\
         [PLAYER]\r\n{\r\n\tPLAYER_NAME \"A\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 0\r\n}\r\n",
    )
    .unwrap();
    assert_eq!(local_player_nation(&level), Some(2), "map nation SPA");
    assert_eq!(
        music_nation(None, &level),
        Some(2),
        "without simulation the .lvl"
    );
    let rules = crate::game::testworld::ki_rules();
    let mut setup = MatchSetup::default_for(&rules);
    setup.lobby[0].nation = 4;
    let mut host = SimHost::new(rules, setup, crate::game::options::DEFAULT_GAME_SPEED).unwrap();
    assert_eq!(host.active_player, 0);
    assert_eq!(music_nation(Some(&host), &level), Some(4), "choice ENG");
    // Player switch (command 28): the nation of the new local player (slot 1, nation 1 of the map).
    host.set_active_player(1).unwrap();
    assert_eq!(music_nation(Some(&host), &level), Some(1));
    // Outside 0...5 no nation music (`0x0042EF99`/`0x0042EF9B`).
    for nation in [-1, 6] {
        host.state.players[1].nation = nation;
        assert_eq!(music_nation(Some(&host), &level), None, "nation {nation}");
    }
}

// QUIRK E-239: deviation
#[test]
fn viewer_load_does_not_stick_at_cinemascope_speed() {
    // The viewer loads without a tempo memory in the savegame (`reset_view`); the memory follows from the loaded state.
    // Without cinemascope in the state, a cinemascope before loading returns to the remembered speed and the memory is
    // empty; with cinemascope, speed 10 applies and the memory stays or becomes the speed before loading. Loading also
    // resets the camera of the scripts (the viewer starts with a free camera).
    let on = Presentation::Cinemascope { on: true };
    let mut host = crate::game::sim_host::plain_testkit::host();
    let mut selection = Selection::default();
    let mut p = ScriptPresentation::new(&ViewOptions::default());
    host.game_speed = 14;
    p.apply_host(&on, &mut host, &mut selection);
    p.camera.bind = Some(ObjectId(0x800));
    let plain = host.state.script.mission.clone();
    let mut cinema = plain.clone();
    cinema.cinemascope = true;
    // State with cinemascope, loaded in cinemascope: the memory stays 14.
    p.restore_host(&cinema, None, &mut host);
    assert_eq!(
        (host.game_speed, p.tempo_memory),
        (CINEMASCOPE_SPEED, Some(14))
    );
    assert_eq!(
        p.camera,
        ScriptCamera::default(),
        "free camera after loading"
    );
    // State without cinemascope: back to 14, memory empty, no sticking at 10.
    p.restore_host(&plain, None, &mut host);
    assert_eq!((host.game_speed, p.tempo_memory), (14, None));
    assert!(!p.cinemascope);
    // Once more without cinemascope: the speed of the player stays.
    host.game_speed = 17;
    p.restore_host(&plain, None, &mut host);
    assert_eq!((host.game_speed, p.tempo_memory), (17, None));
    // State with cinemascope loaded outside cinemascope: memory = speed before loading.
    p.restore_host(&cinema, None, &mut host);
    assert_eq!(
        (host.game_speed, p.tempo_memory),
        (CINEMASCOPE_SPEED, Some(17))
    );
    // `nml-game` specifies the memory of the host part; it takes precedence.
    let mut q = ScriptPresentation::new(&ViewOptions::default());
    q.restore_host(&cinema, Some(5), &mut host);
    assert_eq!(
        (host.game_speed, q.tempo_memory),
        (CINEMASCOPE_SPEED, Some(5))
    );
}

// QUIRK O-287: kept
#[test]
fn the_cinemascope_end_rebuilds_the_minimap_fog_at_once() {
    // The end of cinemascope sets the alpha table at once (`0x00479160(1)`), the minimap rebuilds its fog with it; the
    // start does not.
    let mut host = crate::game::sim_host::plain_testkit::host();
    let mut selection = Selection::default();
    let mut p = ScriptPresentation::new(&ViewOptions::default());
    let before = host.fog_rebuilds();
    p.apply_host(
        &Presentation::Cinemascope { on: true },
        &mut host,
        &mut selection,
    );
    assert_eq!(host.fog_rebuilds(), before);
    p.apply_host(
        &Presentation::Cinemascope { on: false },
        &mut host,
        &mut selection,
    );
    assert_eq!(host.fog_rebuilds(), before + 1);
}

// --- UI messages to the cinemascope area (GPU-free) ------------------------------------------------------------------

/// Kind and words of the UI messages of a presentation event.
fn hud_words(event: Presentation, campaign: bool, screen_h: i32) -> Vec<(u32, Vec<i32>)> {
    crate::game::toolbar_host::presentation_events(&event, campaign, screen_h)
        .into_iter()
        .map(|e| (e.kind.code(), e.args))
        .collect()
}

// QUIRK O-350, O-356: kept
#[test]
fn presentation_events_follow_fun_00430920() {
    // `FUN_00430920`: on -> `{0x11, 1}`, `{0x1D, 0}`, `{0x1C, 0}`, only with game type -1 additionally `{0x1B, H/6}`;
    // off -> `{0x11, 0}`, `{0x1D, 1}`, `{0x1C, 1}`, `{0x1B, 0}` in every game type (re/campaign-ui D6.1, O-350).
    let on = Presentation::Cinemascope { on: true };
    let off = Presentation::Cinemascope { on: false };
    assert_eq!(
        hud_words(on.clone(), true, 768),
        [
            (0x11, vec![1]),
            (0x1D, vec![0]),
            (0x1C, vec![0]),
            (0x1B, vec![128])
        ]
    );
    assert_eq!(
        hud_words(on, false, 768),
        [(0x11, vec![1]), (0x1D, vec![0]), (0x1C, vec![0])],
        "no bars without a campaign"
    );
    for campaign in [true, false] {
        assert_eq!(
            hud_words(off.clone(), campaign, 600),
            [
                (0x11, vec![0]),
                (0x1D, vec![1]),
                (0x1C, vec![1]),
                (0x1B, vec![0])
            ]
        );
    }
    // 149 (`FUN_0042DBD0`): `msg[2]` = frame, `msg[4]` = portrait, colors -1; 156 closes.
    assert_eq!(
        hud_words(
            Presentation::Portrait {
                slot: 1,
                portrait: 7,
                frame: 3
            },
            true,
            768
        ),
        [(0x1E, vec![1, 3, -1, 7, -1])]
    );
    assert_eq!(
        hud_words(Presentation::PortraitHide { slot: 2 }, true, 768),
        [(0x1F, vec![2])]
    );
    // 150 (`Dialog_PlaySpeech` `0x00436A50`) and its end (`FUN_00443450`); 183 on slot + 2 (`FUN_0043E250`).
    assert_eq!(
        hud_words(
            Presentation::Speech {
                text: 500,
                slot: 1,
                ms: 3000
            },
            false,
            768
        ),
        [(0x20, vec![1, 500, -1])]
    );
    assert_eq!(
        hud_words(Presentation::SpeechEnd { text: 500, slot: 1 }, false, 768),
        [(0x21, vec![1])]
    );
    assert_eq!(
        hud_words(
            Presentation::Tip {
                text: 600,
                slot: 1,
                ms: 900
            },
            false,
            768
        ),
        [(0x20, vec![3, 600, -1])]
    );
    assert_eq!(
        hud_words(Presentation::TipEnd { text: 600, slot: 0 }, false, 768),
        [(0x21, vec![2])]
    );
    // Lock as before `{0x19, on}` (`FUN_0042D920`); other events send nothing to the UI.
    assert_eq!(
        hud_words(Presentation::UiLock { on: false }, true, 768),
        [(0x19, vec![0])]
    );
    assert!(hud_words(Presentation::Music { id: 3 }, true, 768).is_empty());
}

// QUIRK E-240: kept
#[test]
fn speech_frame_213_sends_no_text_slot() {
    // 213 plays the line on slot + 4 (`Dialog_PlaySpeech` with type >= 4): no `{0x20}`, and its end closes the info
    // window instead of `{0x21}` (`FUN_00443450`).
    for slot in [4, 5] {
        let speech = Presentation::Speech {
            text: 24021,
            slot,
            ms: 2000,
        };
        assert!(hud_words(speech, true, 768).is_empty());
        let end = Presentation::SpeechEnd { text: 24021, slot };
        assert!(hud_words(end, true, 768).is_empty());
    }
    let frame = Presentation::InfoSpeechFrame {
        text: 24021,
        slot: 0,
        portrait: 1,
        frame: 1,
    };
    assert!(hud_words(frame, true, 768).is_empty());
}

/// Game UI without installation (1024 x 768) with status bar and `BARSIZE 170`.
fn cinemascope_hud() -> crate::game::hud::Hud {
    use crate::game::hud::{Hud, HudOptions};
    let screen = nml_ui::coords::Screen {
        width: 1024,
        height: 768,
    };
    let mut hud = Hud::new(
        &nml_formats::messages::parse_messages(""),
        &std::collections::BTreeMap::new(),
        &HudOptions::default(),
        screen,
    );
    hud.status = Some(nml_ui::hud::status_bar::StatusBar::new(
        nml_formats::statusbar::parse_statusbar(""),
    ));
    hud.cinemascope_data =
        nml_ui::hud::cinemascope::CinemascopeData::from_texts("BARSIZE 170\n", "", "");
    hud
}

// QUIRK O-355, O-356: kept
#[test]
fn portrait_speech_and_tip_reach_the_view() {
    // The game host distributes (`Hud::present`): `0x1B`, `0x1E` ... `0x21` to the area, `0x1C` to the status bar,
    // `0x11` to the command bar. Tip 1 lands on slot 3 (`{0x20, Pos + 2}`, re/campaign-ui D6.8).
    use crate::game::toolbar_host::{ToolbarHost, presentation_events};
    use nml_ui::hud::cinemascope::{Fade, state};
    let mut hud = cinemascope_hud();
    let mut toolbar = ToolbarHost::new();
    let mut send = |hud: &mut crate::game::hud::Hud, p: Presentation| {
        hud.present(presentation_events(&p, true, 768), &mut toolbar);
    };
    send(&mut hud, Presentation::Cinemascope { on: true });
    assert_eq!(hud.cinemascope.bar, 170, "bar = BARSIZE");
    assert!(!hud.status.as_ref().unwrap().shown, "{{0x1C, 0}}");
    send(
        &mut hud,
        Presentation::Portrait {
            slot: 0,
            portrait: 7,
            frame: 3,
        },
    );
    let p = hud.cinemascope.portraits[0];
    assert_eq!((p.portrait, p.frame, p.fade), (7, 3, Fade::show()));
    send(
        &mut hud,
        Presentation::Speech {
            text: 500,
            slot: 1,
            ms: 3000,
        },
    );
    send(
        &mut hud,
        Presentation::Tip {
            text: 600,
            slot: 1,
            ms: 900,
        },
    );
    let texts = hud.cinemascope.texts;
    assert_eq!((texts[1].text, texts[1].fade), (500, Fade::show()));
    assert_eq!(
        (texts[3].text, texts[3].fade),
        (600, Fade::show()),
        "tip 1 -> slot 3"
    );
    assert_eq!(texts[2].fade.state, 0);
    send(&mut hud, Presentation::TipEnd { text: 600, slot: 1 });
    send(&mut hud, Presentation::SpeechEnd { text: 500, slot: 1 });
    send(&mut hud, Presentation::PortraitHide { slot: 0 });
    for fade in [
        hud.cinemascope.texts[3].fade,
        hud.cinemascope.texts[1].fade,
        hud.cinemascope.portraits[0].fade,
    ] {
        assert_ne!(fade.state & state::CLOSING, 0, "fades out");
    }
    send(&mut hud, Presentation::Cinemascope { on: false });
    assert_eq!(hud.cinemascope.bar, 0);
    assert!(hud.status.as_ref().unwrap().shown, "{{0x1C, 1}}");
    // The bar got `{0x11, 1}` and `{0x11, 0}`.
    let mut toolbar = ToolbarHost::new();
    hud.present(
        presentation_events(&Presentation::Cinemascope { on: true }, false, 768),
        &mut toolbar,
    );
    assert!(toolbar.hidden());
    assert_eq!(hud.cinemascope.bar, 0, "free play: no bars");
    hud.present(
        presentation_events(&Presentation::Cinemascope { on: false }, false, 768),
        &mut toolbar,
    );
    assert!(!toolbar.hidden());
}

// --- With installation and graphics card ---------------------------------------------------------------------------

const KAMP1_2: &str = "data/levels/kampagne1/kamp1-2/kamp1-2.lvl";

/// Original installation and graphics card (silently skipped without either).
fn corpus_gpu() -> Option<(Vfs, wgpu::Device, wgpu::Queue)> {
    let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let Ok((device, queue, _)) = nml_render::headless_device(BackendChoice::Auto) else {
        eprintln!("no GPU, test skipped");
        return None;
    };
    Some((Vfs::open_game_dir(&dir).unwrap(), device, queue))
}

fn gpu<'a>(device: &'a wgpu::Device, queue: &'a wgpu::Queue) -> SessionGpu<'a> {
    SessionGpu {
        device,
        queue,
        format: nml_render::CAPTURE_FORMAT,
    }
}

/// Starts `level` with (or without) a level script, AI off, speed 10.
fn start(
    gpu: &SessionGpu,
    vfs: &Vfs,
    level: &str,
    script: bool,
    view: &ViewOptions,
) -> GameSession {
    GameSession::start(
        gpu,
        vfs,
        level,
        &mut |rules: &Rules| {
            let mut setup = MatchSetup::default_for(rules);
            setup.script.enabled = script;
            Ok(setup)
        },
        10,
        view,
        (320, 200),
        &mut |_| {},
    )
    .unwrap()
}

/// `n` frames of 50 ms (speed 10: one tick each).
fn steps(session: &mut GameSession, gpu: &SessionGpu, vfs: &Vfs, n: u32) {
    for _ in 0..n {
        session.advance_by(gpu, vfs, 50.0, true, None);
    }
}

fn light_of(session: &GameSession) -> String {
    session.map.light_name().to_lowercase()
}

#[test]
fn script_match_ignores_the_start_tables() {
    // `kamp1-2` begins at night in the game. With a script the light does not come until 171 ("Sommer Nacht" under
    // "Always"), before that the `.wld` applies; without a script the start table sets it when loading.
    let Some((vfs, device, queue)) = corpus_gpu() else {
        return;
    };
    let gpu = gpu(&device, &queue);
    let view = ViewOptions::default();
    let mut scripted = start(&gpu, &vfs, KAMP1_2, true, &view);
    assert!(scripted.sim().unwrap().runs_level_script());
    assert_eq!(scripted.map.light, None, "before 171 the light of the .wld");
    steps(&mut scripted, &gpu, &vfs, 5);
    let memory = &scripted.sim().unwrap().state.script.mission.presentation;
    assert_eq!(memory.light.as_deref(), Some("sommer nacht"), "171 ran");
    assert_eq!(light_of(&scripted), "sommer nacht");
    let plain = start(&gpu, &vfs, KAMP1_2, false, &view);
    assert!(!plain.sim().unwrap().runs_level_script());
    assert_eq!(
        light_of(&plain),
        "sommer nacht",
        "start table without script"
    );
    // Weather: `kamp2-5` begins without a script with rain from the table; with a script only once 209 runs.
    let kamp2_5 = "data/levels/kampagne2/kamp2-5/kamp2-5.lvl";
    let plain = start(&gpu, &vfs, kamp2_5, false, &view);
    assert_eq!(
        plain.map.weather.as_ref().map(|w| w.kind()),
        Some(WeatherKind::Rain)
    );
    let scripted = start(&gpu, &vfs, kamp2_5, true, &view);
    assert_eq!(scripted.map.weather.as_ref().map(|w| w.kind()), None);
}

#[test]
fn explicit_viewer_light_wins() {
    // `--light <name>` and `--weather <kind>` take precedence also with a script; 171/381 and 209/210 then change
    // nothing.
    let Some((vfs, device, queue)) = corpus_gpu() else {
        return;
    };
    let gpu = gpu(&device, &queue);
    let view = ViewOptions {
        light: LightSetting::Preset("5".into()),
        weather: WeatherSetting::Snow,
        ..ViewOptions::default()
    };
    let mut session = start(&gpu, &vfs, KAMP1_2, true, &view);
    assert_eq!(light_of(&session), "5");
    steps(&mut session, &gpu, &vfs, 5);
    let memory = &session.sim().unwrap().state.script.mission.presentation;
    assert_eq!(memory.light.as_deref(), Some("sommer nacht"), "171 ran");
    assert_eq!(light_of(&session), "5", "default of the viewer stays");
    let object_light = session.map.scene.object_light.clone();
    let mut ctx = PresentCtx {
        map: &mut session.map,
        selection: &mut session.selection,
        queue: &queue,
        audio: None,
        vfs: &vfs,
    };
    for event in [
        Presentation::LightRestoreLevel,
        Presentation::Weather { kind: 1 },
        Presentation::Weather { kind: 0 },
    ] {
        session.presentation.apply(&event, &mut ctx);
    }
    assert_eq!(session.map.scene.object_light, object_light);
    assert_eq!(
        session.map.weather.as_ref().map(|w| w.kind()),
        Some(WeatherKind::Snow)
    );
    // Without a default the script takes effect; an unknown name (also the placeholder of the parser) changes nothing.
    let mut free = start(&gpu, &vfs, KAMP1_2, true, &ViewOptions::default());
    let mut ctx = PresentCtx {
        map: &mut free.map,
        selection: &mut free.selection,
        queue: &queue,
        audio: None,
        vfs: &vfs,
    };
    for preset in ["5", "Mond", "fehlerhafte_parameterliste"] {
        free.presentation.apply(
            &Presentation::Light {
                preset: preset.into(),
            },
            &mut ctx,
        );
    }
    assert_eq!(light_of(&free), "5");
}

/// All channels of the game host that a state re-creates.
#[derive(Debug, Clone, PartialEq)]
struct Channels {
    light: String,
    object_light: Option<nml_formats::wld::ObjectLight>,
    weather: Option<WeatherKind>,
    fog_enabled: Option<bool>,
    fog_color: Option<u32>,
    show_mode: u8,
    cinemascope: bool,
    game_speed: u8,
    tempo_memory: Option<u8>,
    highlight: Option<ObjectId>,
    music: (String, String, String, i32),
}

fn channels(session: &GameSession, audio: &AudioEngine) -> Channels {
    let sim = session.sim().unwrap();
    let m = &audio.music;
    Channels {
        light: light_of(session),
        object_light: session.map.scene.object_light.clone(),
        weather: session.map.weather.as_ref().map(|w| w.kind()),
        fog_enabled: session.map.script_fog.enabled,
        fog_color: session.map.script_fog.values.map(|v| v.color),
        show_mode: sim.fog.show_mode,
        cinemascope: session.presentation.cinemascope,
        game_speed: sim.game_speed,
        tempo_memory: session.presentation.tempo_memory,
        highlight: session.presentation.highlight,
        music: (
            m.current.clone(),
            m.attack.clone(),
            m.status.name().to_string(),
            m.config().attack_start_points,
        ),
    }
}

// QUIRK E-169, E-237: kept
// QUIRK E-239: deviation
#[test]
fn restore_rebuilds_all_channels() {
    // Deviation from the original (E-239): a savegame with all channels re-creates the same channel values after
    // loading as the individual events (light including object light, weather, fog on and values - the crossfade
    // jumps to the target -, view mode, cinemascope with speed 10 and memory, highlight, script and combat music with
    // threshold).
    let Some((vfs, device, queue)) = corpus_gpu() else {
        return;
    };
    let gpu = gpu(&device, &queue);
    let view = ViewOptions::default();
    let mut a = start(&gpu, &vfs, KAMP1_2, true, &view);
    a.sim_mut().unwrap().game_speed = 14;
    let mut audio_a = AudioEngine::open(&vfs, false);
    a.start_level_audio(&mut audio_a, &vfs);
    let id = a.sim().unwrap().state.objects().next().unwrap().id;
    let fog = (5_192_239, 2000, 5000);
    let events = [
        Presentation::Light {
            preset: "sommer nacht".into(),
        },
        Presentation::LightRestoreLevel,
        Presentation::Weather { kind: 2 },
        Presentation::DistanceFog { on: true },
        Presentation::FogValues {
            color: fog.0,
            start: fog.1,
            end: fog.2,
            ms: 500,
            swap_red_blue: true,
        },
        Presentation::Cinemascope { on: true },
        Presentation::Highlight { id: Some(id) },
        Presentation::AttackThreshold { points: 700 },
        Presentation::AttackMusic { id: 115 },
        Presentation::Music { id: 100 },
    ];
    {
        let mut ctx = PresentCtx {
            map: &mut a.map,
            selection: &mut a.selection,
            queue: &queue,
            audio: Some(&mut audio_a),
            vfs: &vfs,
        };
        for event in &events {
            a.presentation.apply(event, &mut ctx);
        }
        a.presentation.update(1000.0, 1000.0, &mut ctx);
    }
    assert!(a.presentation.fog_fade.is_none(), "crossfade finished");
    // The state as the simulation holds it for these actions.
    {
        let mission = &mut a.sim_mut().unwrap().state.script.mission;
        mission.cinemascope = true;
        let m = &mut mission.presentation;
        m.light = Some("sommer nacht".into());
        m.object_light_restored = true;
        m.weather = 2;
        m.distance_fog = Some(true);
        m.fog = Some(FogMemory {
            color: fog.0,
            start: fog.1,
            end: fog.2,
            swap_red_blue: true,
        });
        m.fog_show_all = Some(true);
        m.highlight = Some(id);
        m.attack_threshold = Some(700);
        m.attack_music = Some(115);
        m.music = Some(100);
    }
    let before = channels(&a, &audio_a);
    assert_eq!(before.game_speed, CINEMASCOPE_SPEED);
    assert_eq!(before.tempo_memory, Some(14));
    assert_eq!(before.weather, Some(WeatherKind::Snow));
    assert_eq!(before.highlight, Some(id));
    assert_eq!(
        before.object_light, a.map.scene.world.light,
        "381: object light of the .wld"
    );
    let host = HostPart {
        camera: CameraSave::of(&a.map.camera),
        level_info: Default::default(),
        game_type: -1,
        local_name: String::new(),
        game_speed: a.sim().unwrap().game_speed,
        cinemascope_speed: a.cinemascope_speed(),
        script_camera: Default::default(),
        control_groups: crate::game::control_groups::empty_save(),
        next_guid_index: crate::game::control_groups::empty_next_guid_index(),
        idle_worker_index: [0; 8],
        masks: crate::game::diplomacy::Masks::initial(),
        lock_teams: false,
        // Like `GameFlow::host_part`: the fog switch of the game host (here 7 from `fog_show_all`).
        fog_show_mode: a.sim().unwrap().fog.show_mode,
        net: None,
        log: None,
        save_replay: false,
    };
    let save = a.save(host).unwrap();
    let mut b = GameSession::resume_from(&gpu, &vfs, save, &view, (320, 200), &mut |_| {}).unwrap();
    assert!(b.presentation.audio_pending(), "music follows with sound");
    let mut audio_b = AudioEngine::open(&vfs, false);
    b.presentation.resume_audio(&b.map, &mut audio_b, &vfs);
    assert!(!b.presentation.audio_pending());
    let after = channels(&b, &audio_b);
    // In the original the crossfade ends up to one byte next to the target; loading jumps exactly to the target.
    let target = FogValues::of_memory(&FogMemory {
        color: fog.0,
        start: fog.1,
        end: fog.2,
        swap_red_blue: true,
    });
    assert_eq!(after.fog_color, Some(target.color));
    for shift in [0, 8, 16, 24] {
        let (x, y) = (
            channel(before.fog_color.unwrap(), shift),
            channel(target.color, shift),
        );
        assert!(x.abs_diff(y) <= 1, "channel {shift}");
    }
    assert_eq!(
        Channels {
            fog_color: None,
            ..after
        },
        Channels {
            fog_color: None,
            ..before
        }
    );
}

const SNOWDREAM: &str = "data/levels/multiplayer/[4players]-snowdream.lvl";

// QUIRK D-158: kept
#[test]
fn free_play_music_follows_the_lobby_nation_also_after_loading_and_switching() {
    // Snowdream, own slot with map nation SPA, ENG chosen in the start dialog -> ENG music
    // (`Game_StartNationMusic([GetLocalPlayer()+0x238])`, `+0x238` written by `SetupPlayersFromLobby`). After loading
    // the same nation (the original re-creates `m_CurNation` from the state), after a player switch (command 28) the
    // nation of the new local player.
    let Some((vfs, device, queue)) = corpus_gpu() else {
        return;
    };
    let gpu = gpu(&device, &queue);
    let view = ViewOptions::default();
    let mut a = GameSession::start(
        &gpu,
        &vfs,
        SNOWDREAM,
        &mut |rules: &Rules| {
            let mut setup = MatchSetup::default_for(rules);
            setup.lobby[0].nation = 4;
            Ok(setup)
        },
        10,
        &view,
        (320, 200),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        local_player_nation(&a.map.scene.level),
        Some(2),
        "Snowdream: slot 0 is SPA on the map"
    );
    let mut audio_a = AudioEngine::open(&vfs, false);
    a.start_level_audio(&mut audio_a, &vfs);
    let playlists = audio_a.music.config().playlists.clone();
    assert_ne!(playlists[4][0], playlists[2][0]);
    let music = |audio: &AudioEngine| {
        (
            audio.music.nation(),
            audio.music.current.clone(),
            audio.music.status,
        )
    };
    let eng = (Some(4), playlists[4][0].clone(), MusicStatus::Normal);
    assert_eq!(music(&audio_a), eng, "playlist of the choice ENG runs");
    let host = HostPart {
        camera: CameraSave::of(&a.map.camera),
        level_info: Default::default(),
        game_type: 0,
        local_name: String::new(),
        game_speed: a.sim().unwrap().game_speed,
        cinemascope_speed: a.cinemascope_speed(),
        script_camera: Default::default(),
        control_groups: crate::game::control_groups::empty_save(),
        next_guid_index: crate::game::control_groups::empty_next_guid_index(),
        idle_worker_index: [0; 8],
        masks: crate::game::diplomacy::Masks::initial(),
        lock_teams: false,
        fog_show_mode: a.sim().unwrap().fog.show_mode,
        net: None,
        log: None,
        save_replay: false,
    };
    let save = a.save(host).unwrap();
    let mut b = GameSession::resume_from(&gpu, &vfs, save, &view, (320, 200), &mut |_| {}).unwrap();
    let mut audio_b = AudioEngine::open(&vfs, false);
    b.presentation.resume_audio(&b.map, &mut audio_b, &vfs);
    assert_eq!(music(&audio_b), eng, "same nation after loading");
    b.sim_mut().unwrap().set_active_player(1).unwrap();
    let other = usize::try_from(b.sim().unwrap().state.players[1].nation).unwrap();
    b.presentation
        .switch_nation_music(&b.map, &mut audio_b, &vfs);
    assert_eq!(
        music(&audio_b),
        (
            Some(other),
            playlists[other][0].clone(),
            MusicStatus::Normal
        ),
        "player switch: nation music of the new player"
    );
}

// QUIRK D-289: deviation
#[test]
fn corpus_free_play_maps_start_at_slot_0() {
    // Quirk (`Player_Construct` `0x004FAF60` sets `+0x04 = -1` for all slots): in free play the local player is the
    // first slot with lobby key -1 (`FUN_0042DF10(-1)`), so always slot 0 - also an empty slot 0 before the first
    // active one (nation 0 from the constructor). The reimplementation takes the first human slot. Both agree as long
    // as slot 0 has a `[PLAYER]` on every multiplayer map; this test checks that. Plus the levels without a human
    // slot (`FindNthHumanPlayerSlot(0)` = -1 in the original).
    let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut maps = 0;
    let mut without_human = Vec::new();
    for lvl in nml_world::level::level_paths(&vfs) {
        let text = String::from_utf8_lossy(&vfs.read(&lvl).unwrap()).into_owned();
        let level = nml_formats::lvl::parse_level(&text).unwrap();
        if lvl.contains("/multiplayer/") {
            maps += 1;
            assert!(
                level.players.iter().any(|p| p.color == 0),
                "{lvl}: slot 0 without [PLAYER]"
            );
        }
        if !level
            .players
            .iter()
            .any(|p| p.kind == 0 && (0..8).contains(&p.color))
        {
            without_human.push(lvl);
        }
    }
    eprintln!("{maps} multiplayer maps; without a human slot: {without_human:?}");
    assert!(maps > 0);
    // D-289: every level has a human slot (the edge case index -1 does not occur).
    assert!(
        without_human.is_empty(),
        "levels without a human slot: {without_human:?}"
    );
}
