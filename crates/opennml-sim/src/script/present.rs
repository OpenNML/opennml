//! Presentation of the scripts: the presentation actions report their effect as a [`Presentation`] event
//! (`SimEvent::Present`, not part of the state) and keep the state for loading and replay in
//! [`PresentationMemory`] (`MissionState::presentation`). The host (`opennml-client`) turns both into light,
//! weather, music, fog, bars, lock and text.
//!
//! This module defines the types, [`emit`] and the hook [`dispatch_present`]; the effect of the action
//! numbers comes in groups: camera and object light (`camera`), scene (fade, cinemascope, lock, portraits,
//! marker, info window, on-screen text, ping: `scene`), sound, light, weather, fog and quicksave
//! (`environment`) and the profile variables (`profile`); the speech lines, tips and the speech window
//! (150, 183, 213) report their events from [`super::speech`]. All numbers stay the raw script numbers, no
//! floating point in the state. No catalog number reports "no effect yet" (`Pending`) any more.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::actions::{Dispatched, MsgValue, ScriptMsg};
use super::container::Container;
use super::{ScriptTrace, trace};
use crate::id::ObjectId;
use crate::sim::Sim;
use crate::state::SimEvent;

/// Maximum number of remembered info windows ([`PresentationMemory::info_frames`], unlimited in the original).
pub const MAX_INFO_FRAMES: usize = 4096;

/// Effect of a presentation action, reported as `SimEvent::Present`. The values are those of
/// re/script-actions; the comments give the action numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presentation {
    /// 2 (area center, C division; player unread).
    CameraCenter {
        tile_x: i32,
        tile_z: i32,
    },
    /// 170 (first ID of the name if the object is present; otherwise no event).
    CameraToObject {
        id: ObjectId,
    },
    /// 141.
    CameraCut {
        camera: String,
    },
    /// 142, 220, 240.
    CameraMove {
        camera: String,
        ms: i32,
    },
    /// 143.
    CameraBackup,
    /// 144.
    CameraRestoreBackup,
    /// 216, 222, 242.
    CameraMoveToBackup {
        ms: i32,
    },
    /// 172, 223, 243.
    CameraToGame {
        ms: i32,
    },
    /// 175 (unknown name -> `None`), 176-178 -> `None`.
    CameraBind {
        id: Option<ObjectId>,
    },
    /// 212, 221, 241.
    CameraRotate {
        degrees: i32,
        ms: i32,
    },
    /// 145 (`into = true`), 146.
    Fade {
        into: bool,
        ms: i32,
    },
    /// 147/148.
    Cinemascope {
        on: bool,
    },
    /// 18/19.
    UiLock {
        on: bool,
    },
    /// 149.
    Portrait {
        slot: i32,
        portrait: i32,
        frame: i32,
    },
    /// 156.
    PortraitHide {
        slot: i32,
    },
    /// 152 (last present ID), 155 -> `None`.
    Highlight {
        id: Option<ObjectId>,
    },
    /// 150; 213 with `slot + 4`.
    Speech {
        text: i32,
        slot: i32,
        ms: i32,
    },
    /// End of a line with `slot < 4`.
    SpeechEnd {
        text: i32,
        slot: i32,
    },
    /// 183 (slot 0/1 only).
    Tip {
        text: i32,
        slot: i32,
        ms: i32,
    },
    TipEnd {
        text: i32,
        slot: i32,
    },
    /// 168 (`style` 0/1/2 from the flag).
    InfoFrame {
        text: i32,
        x: i32,
        y: i32,
        style: i32,
    },
    /// 213.
    InfoSpeechFrame {
        text: i32,
        slot: i32,
        portrait: i32,
        frame: i32,
    },
    /// 169 and implicit closing.
    InfoFrameClose {
        text: i32,
    },
    /// 5.
    ScreenText {
        text: i32,
        seconds: i32,
    },
    /// 7 (unknown area -> no event).
    MinimapPing {
        player: i32,
        tile_x: i32,
        tile_z: i32,
    },
    /// 9.
    Music {
        id: i32,
    },
    /// 370.
    AttackMusic {
        id: i32,
    },
    /// 371.
    AttackThreshold {
        points: i32,
    },
    /// 507.
    Sound2d {
        id: i32,
    },
    /// 171.
    Light {
        preset: String,
    },
    /// 381.
    LightRestoreLevel,
    /// 209 (`kind = n`), 210 (`0`).
    Weather {
        kind: i32,
    },
    /// 501/502.
    DistanceFog {
        on: bool,
    },
    /// 505 (`ms` 0, `swap_red_blue = false`), 506 (`true`).
    FogValues {
        color: i32,
        start: i32,
        end: i32,
        ms: i32,
        swap_red_blue: bool,
    },
    /// 504 -> `true`, 503 -> `false`.
    FogShowAll {
        on: bool,
    },
    /// 508 (campaign only).
    QuickSave,
    /// 382.
    ProfileVar {
        name: String,
        value: i32,
    },
}

/// Presentation state: what loading and replay have to restore. The exception is the camera (host state,
/// deviation from the original).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationMemory {
    /// Light preset of 171; `None` until then (the level keeps its start light).
    pub light: Option<String>,
    /// 171 sets `false`, 381 `true` (object light back to the state of the `.wld`).
    pub object_light_restored: bool,
    /// 209/210, start 0.
    pub weather: i32,
    pub music: Option<i32>,
    pub attack_music: Option<i32>,
    pub attack_threshold: Option<i32>,
    /// 501/502.
    pub distance_fog: Option<bool>,
    /// Target values of 505/506.
    pub fog: Option<FogMemory>,
    /// 503/504, 147 -> `true`, 148 -> `false`.
    pub fog_show_all: Option<bool>,
    /// 146 -> `true`, 145 -> `false`.
    pub faded_out: bool,
    /// 149/156 (slot not in 0 to 3 ignored), 147/148 clear.
    pub portraits: [Option<PortraitMemory>; 4],
    /// 168/169/213, limit [`MAX_INFO_FRAMES`]; the key is the text ID.
    pub info_frames: BTreeMap<i32, InfoFrameMemory>,
    /// 152/155, 147/148 -> `None`.
    pub highlight: Option<ObjectId>,
}

/// Target values of the distance fog (505/506), the raw script numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogMemory {
    pub color: i32,
    pub start: i32,
    pub end: i32,
    pub swap_red_blue: bool,
}

/// Portrait in one of the four slots (149).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortraitMemory {
    pub portrait: i32,
    pub frame: i32,
}

/// Open info window: window with text (168) or speech window with portrait (213).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InfoFrameMemory {
    Window {
        x: i32,
        y: i32,
        style: i32,
    },
    Speech {
        slot: i32,
        portrait: i32,
        frame: i32,
    },
}

/// Reports `p` as the step's `SimEvent::Present`.
pub(crate) fn emit(sim: &mut Sim, p: Presentation) {
    sim.state.events.push(SimEvent::Present(p));
}

/// Hook of the presentation actions in [`super::actions::dispatch`] (layer D): `Some` if the number is a
/// presentation action, otherwise `None` and the message is passed on to the command and world actions. The
/// message is mutable because 383 puts its answer into `values[0]`; the values of 142, 212, 216 and 172 stay
/// unchanged (the wait time reads them afterwards). Layer E computes the percent forms 220 to 223 and 240 to
/// 243 and builds messages 142, 212, 216 and 172 from them; only those arrive here.
pub(crate) fn dispatch_present(
    sim: &mut Sim,
    _c: &Container,
    msg: &mut ScriptMsg,
) -> Option<Dispatched> {
    if let Some(done) = camera(sim, msg) {
        return Some(done);
    }
    if let Some(done) = scene(sim, msg) {
        return Some(done);
    }
    if let Some(done) = environment(sim, msg) {
        return Some(done);
    }
    if let Some(done) = profile(sim, msg) {
        return Some(done);
    }
    match msg.number {
        // 381: not a camera action but the object light back to the level value (E-236); the terrain
        // keeps the light of the last preset (171).
        381 => {
            sim.state.script.mission.presentation.object_light_restored = true;
            emit(sim, Presentation::LightRestoreLevel);
            Some(Dispatched::Done)
        }
        _ => None,
    }
}

/// Camera actions. A handled number that ends silently (unknown area, missing object) returns
/// `Some(Done)` without an event; `None` only for numbers this module does not know (otherwise it would
/// fall through to `Pending`). They only report an event and change no state; the script camera belongs
/// to the host (deviation from the original). The values are those of re/script-actions. The player parameter of 2,
/// 141, 142 (and thus 220, 240) is not read (E-167). The camera name arrives lowercase and without "camera " from
/// the parser; the host decides whether the camera exists (unknown -> origin).
fn camera(sim: &mut Sim, msg: &ScriptMsg) -> Option<Dispatched> {
    let event = match msg.number {
        // `{player, area}`: center of the area in tiles. Rust `/` truncates toward 0 like the C division;
        // the sum runs in 32 bits as in the original. An unknown area has no effect (`script::area`, not
        // `query::area_center`, which returns (0, 0)).
        2 => {
            let Some((tile_x, tile_z)) = area_center(sim, msg.text(1)) else {
                return Some(Dispatched::Done);
            };
            Presentation::CameraCenter { tile_x, tile_z }
        }
        // `{player, camera}`.
        141 => Presentation::CameraCut {
            camera: msg.text(1).to_owned(),
        },
        // `{player, ms, camera}` (also 220 and 240 via layer E).
        142 => Presentation::CameraMove {
            camera: msg.text(2).to_owned(),
            ms: msg.int(1),
        },
        143 => Presentation::CameraBackup,
        144 => Presentation::CameraRestoreBackup,
        // 170 `{name}` (`FUN_00439940`): once, the position of the object with the FIRST ID of the name
        // (`names::first_id`, unknown or ID 0 -> nothing); if this object is missing, nothing happens, there
        // is no fallback to another object of the name. Like 175.
        170 => {
            let Some(id) = crate::names::first_id(&sim.state.names, msg.text(0))
                .filter(|id| sim.object(*id).is_some())
            else {
                return Some(Dispatched::Done);
            };
            Presentation::CameraToObject { id }
        }
        // `{ms}` (also 223 and 243).
        172 => Presentation::CameraToGame { ms: msg.int(0) },
        // 175 `{name}`: the first ID of the name; unknown or ID 0 releases the binding. 176 to 178
        // release it (text of 177 and 178 unread).
        175 => Presentation::CameraBind {
            id: crate::names::first_id(&sim.state.names, msg.text(0)),
        },
        176..=178 => Presentation::CameraBind { id: None },
        // `{degrees, ms}` (also 221 and 241).
        212 => Presentation::CameraRotate {
            degrees: msg.int(0),
            ms: msg.int(1),
        },
        // `{ms}` (also 222 and 242).
        216 => Presentation::CameraMoveToBackup { ms: msg.int(0) },
        _ => return None,
    };
    emit(sim, event);
    Some(Dispatched::Done)
}

/// Center of the area `name` in tiles (2 and 7): `(x0 + x1) / 2` with C division, the sum runs in 32
/// bits. An unknown area gives `None` (`script::area`, not `query::area_center`, which returns (0, 0)).
fn area_center(sim: &Sim, name: &str) -> Option<(i32, i32)> {
    let [x0, z0, x1, z1] = super::area(sim, name)?;
    Some((x0.wrapping_add(x1) / 2, z0.wrapping_add(z1) / 2))
}

/// Scene actions: on-screen text, ping, lock, fade, cinemascope, portraits, marker and info window. 150,
/// 183 and 213 (line, tip, speech window) are reported by [`super::speech`]. The values are those of
/// re/script-actions; the original checks the portrait slots in the UI of the original (0 to 3), the event goes out
/// anyway.
fn scene(sim: &mut Sim, msg: &ScriptMsg) -> Option<Dispatched> {
    let event = match msg.number {
        // `{text, (unread), seconds}`.
        5 => Presentation::ScreenText {
            text: msg.int(0),
            seconds: msg.int(2),
        },
        // `{player, area}`: ping at the area center; the player is not checked (E-244, the load check only
        // allows 0 to 9), an unknown area has no effect.
        7 => {
            let Some((tile_x, tile_z)) = area_center(sim, msg.text(1)) else {
                return Some(Dispatched::Done);
            };
            Presentation::MinimapPing {
                player: msg.int(0),
                tile_x,
                tile_z,
            }
        }
        // Input lock (`GE+0x600`). `{0x19, 1}` clears the selection only with the command bar visible, in
        // cinemascope it is off (E-246).
        18 | 19 => {
            let on = msg.number == 18;
            let mission = &mut sim.state.script.mission;
            mission.ui_locked = on;
            if on && !mission.cinemascope {
                mission.selection.clear();
            }
            Presentation::UiLock { on }
        }
        // `{ms}`: 145 fades in, 146 fades out and stays black until the next 145 (E-247).
        145 | 146 => {
            let into = msg.number == 145;
            sim.state.script.mission.presentation.faded_out = !into;
            Presentation::Fade {
                into,
                ms: msg.int(0),
            }
        }
        // Cinemascope: clears the selection, all four portrait slots and the target marker both when switching on
        // **and** off; `m_FogShowMode` 7 (on) or 0 (off). The speed is up to the host (E-238).
        147 | 148 => {
            let on = msg.number == 147;
            let mission = &mut sim.state.script.mission;
            mission.cinemascope = on;
            mission.selection.clear();
            mission.presentation.portraits = [None; 4];
            mission.presentation.highlight = None;
            mission.presentation.fog_show_all = Some(on);
            Presentation::Cinemascope { on }
        }
        // `n149;portrait;frame;slot`.
        149 => {
            let (portrait, frame, slot) = (msg.int(0), msg.int(1), msg.int(2));
            if let Some(entry) = portrait_slot(&mut sim.state.script.mission.presentation, slot) {
                *entry = Some(PortraitMemory { portrait, frame });
            }
            Presentation::Portrait {
                slot,
                portrait,
                frame,
            }
        }
        156 => {
            let slot = msg.int(0);
            if let Some(entry) = portrait_slot(&mut sim.state.script.mission.presentation, slot) {
                *entry = None;
            }
            Presentation::PortraitHide { slot }
        }
        // 152 sets the target marker to the LAST present ID of the name (also dying ones); if there is
        // none, nothing happens. The selection stays untouched (E-228).
        152 => {
            let Some(id) = crate::names::ids(&sim.state.names, msg.text(0))
                .into_iter()
                .rfind(|id| sim.object(*id).is_some())
            else {
                return Some(Dispatched::Done);
            };
            sim.state.script.mission.presentation.highlight = Some(id);
            Presentation::Highlight { id: Some(id) }
        }
        155 => {
            sim.state.script.mission.presentation.highlight = None;
            Presentation::Highlight { id: None }
        }
        // `{text, x, y, flag}`: style 1 or 2 from the flag, otherwise 0; replaces the window of the same ID.
        168 => {
            let (text, x, y, flag) = (msg.int(0), msg.int(1), msg.int(2), msg.int(3));
            let style = if matches!(flag, 1 | 2) { flag } else { 0 };
            open_info_frame(sim, text, InfoFrameMemory::Window { x, y, style });
            Presentation::InfoFrame { text, x, y, style }
        }
        169 => {
            close_info_frame(sim, msg.int(0));
            return Some(Dispatched::Done);
        }
        _ => return None,
    };
    emit(sim, event);
    Some(Dispatched::Done)
}

/// Sound, light, weather, fog and quicksave. The values are those of re/script-actions; all numbers stay
/// the raw script numbers. The state holds what loading and replay need (`music`, `attack_music`,
/// `attack_threshold`, `light`, `weather`, `distance_fog`, `fog`, `fog_show_all`); 507 is a single sound
/// without state.
fn environment(sim: &mut Sim, msg: &ScriptMsg) -> Option<Dispatched> {
    let event = match msg.number {
        // `Script_PlayMusicById`: script music (the host chooses loop and crossfade according to the
        // status of the music manager).
        9 => {
            let id = msg.int(0);
            memory(sim).music = Some(id);
            Presentation::Music { id }
        }
        // Set the attack music file (plays nothing); `status 0 -> 1` is up to the host. The D case acts
        // only with exactly one value (`CMP [EBX+4], 1` at `0x004256A0`, E-251), otherwise done without
        // effect; from a script exactly one always arrives (layer E `0x0056C526` writes `{370, 1, Z0}`).
        370 if msg.values.len() != 1 => return Some(Dispatched::Done),
        370 => {
            let id = msg.int(0);
            memory(sim).attack_music = Some(id);
            Presentation::AttackMusic { id }
        }
        371 => {
            let points = msg.int(0);
            memory(sim).attack_threshold = Some(points);
            Presentation::AttackThreshold { points }
        }
        // Sound GUID 2D, without loop; no state.
        507 => Presentation::Sound2d { id: msg.int(0) },
        // Light preset `{name}`: an empty name has no effect; every other one is remembered, whether the host
        // knows it is up to the host (unknown -> log, change nothing). 171 sets "object light restored" to
        // `false`.
        171 => {
            let preset = msg.text(0);
            if preset.is_empty() {
                return Some(Dispatched::Done);
            }
            let preset = preset.to_owned();
            let m = memory(sim);
            m.light = Some(preset.clone());
            m.object_light_restored = false;
            Presentation::Light { preset }
        }
        // 209 `{n}` starts weather of kind n, 210 ends it (does not read the parameter).
        209 | 210 => {
            let kind = if msg.number == 209 { msg.int(0) } else { 0 };
            memory(sim).weather = kind;
            Presentation::Weather { kind }
        }
        501 | 502 => {
            let on = msg.number == 501;
            memory(sim).distance_fog = Some(on);
            Presentation::DistanceFog { on }
        }
        // 503 off, 504 on (`m_FogShowMode` 0 or 7); the same channel as cinemascope (147/148).
        503 | 504 => {
            let on = msg.number == 504;
            memory(sim).fog_show_all = Some(on);
            Presentation::FogShowAll { on }
        }
        // 505 `{color, start, end}`: immediately, the color unchanged (`FUN_0042F200` with `ms 0`).
        505 => fog_values(sim, 0, msg.int(0), msg.int(1), msg.int(2)),
        // 506 `{ms, color, start, end}`: crossfade; the original swaps red and blue only in the crossfade
        // (`ms > 0`, E-237), with `ms <= 0` it takes the color unchanged.
        506 => fog_values(sim, msg.int(0), msg.int(1), msg.int(2), msg.int(3)),
        // Quicksave only in the campaign (`GameEngine+0x108 == -1`, re/script-actions C.13); saving happens
        // in the host after the step. No state.
        508 => {
            if sim.state.mode != CAMPAIGN_MODE {
                return Some(Dispatched::Done);
            }
            Presentation::QuickSave
        }
        _ => return None,
    };
    emit(sim, event);
    Some(Dispatched::Done)
}

/// Game mode of the campaign (`GameEngine+0x108`, `MatchSetup::mode`).
const CAMPAIGN_MODE: i32 = -1;

/// Presentation state of the mission.
fn memory<'a>(sim: &'a mut Sim<'_>) -> &'a mut PresentationMemory {
    &mut sim.state.script.mission.presentation
}

/// Sets the target values of the distance fog (505, 506) and returns the event. `swap_red_blue` applies
/// only to a crossfade (`ms > 0`); the state carries the target values.
fn fog_values(sim: &mut Sim, ms: i32, color: i32, start: i32, end: i32) -> Presentation {
    let swap_red_blue = ms > 0;
    memory(sim).fog = Some(FogMemory {
        color,
        start,
        end,
        swap_red_blue,
    });
    Presentation::FogValues {
        color,
        start,
        end,
        ms,
        swap_red_blue,
    }
}

/// Profile variables (E-166): 382 writes the value of the script variable into the mirror
/// [`MissionState::profile_vars`](super::MissionState::profile_vars) and reports `ProfileVar` (the host
/// writes the profile); 383 reads the mirror (name missing: 0) and answers like the original in
/// `values[0]` (`msg[2]`), [`super::actions`] then writes the answer to `Var[p0]`. The message of 382 is
/// `{value, name}`, that of 383 `{name}`; empty names already stop in layer E.
fn profile(sim: &mut Sim, msg: &mut ScriptMsg) -> Option<Dispatched> {
    match msg.number {
        382 => {
            let (name, value) = (msg.text(1).to_owned(), msg.int(0));
            sim.state
                .script
                .mission
                .profile_vars
                .insert(name.clone(), value);
            emit(sim, Presentation::ProfileVar { name, value });
        }
        383 => {
            let value = sim
                .state
                .script
                .mission
                .profile_vars
                .get(msg.text(0))
                .copied()
                .unwrap_or(0);
            match msg.values.first_mut() {
                Some(v) => *v = MsgValue::Int(value),
                None => msg.values.push(MsgValue::Int(value)),
            }
        }
        _ => return None,
    }
    Some(Dispatched::Done)
}

/// Portrait slot `slot` of the state; outside 0 to 3 there is none (the UI panel of the original ignores
/// it).
fn portrait_slot(
    presentation: &mut PresentationMemory,
    slot: i32,
) -> Option<&mut Option<PortraitMemory>> {
    presentation.portraits.get_mut(usize::try_from(slot).ok()?)
}

/// Enters the info window of text ID `text` (one per ID, an existing one is replaced). If the table is
/// full, no new window is added and [`ScriptTrace::Limit`] is reported (like `speech::push_bounded`);
/// the caller reports the event anyway.
pub(crate) fn open_info_frame(sim: &mut Sim, text: i32, frame: InfoFrameMemory) {
    let frames = &mut sim.state.script.mission.presentation.info_frames;
    if frames.len() >= MAX_INFO_FRAMES && !frames.contains_key(&text) {
        trace(
            sim,
            ScriptTrace::Limit {
                what: format!("info windows (at most {MAX_INFO_FRAMES})"),
            },
        );
        return;
    }
    frames.insert(text, frame);
}

/// Closes the info window of text ID `text` (169, 213 and the speech end of a line from slot 4,
/// `FUN_004401C0`): always reports `InfoFrameClose`, as the original calls it unchecked, and changes the
/// state only if there is an entry.
pub(crate) fn close_info_frame(sim: &mut Sim, text: i32) {
    sim.state
        .script
        .mission
        .presentation
        .info_frames
        .remove(&text);
    emit(sim, Presentation::InfoFrameClose { text });
}
