//! Sound options `SOUNDOPTIONS` / `SOUNDOPTIONS2` / `SOUNDOPTIONS3` (`FUN_004D1930`, calls `0x004DC2FC` and
//! `0x004DCA53`; re/menu-flow, `SOUNDOPTIONS`). All values take effect at once; they are saved via `OPTIONS OK` or at
//! program end. The sound management (`g_SoundManager`) is in the reimplementation the block `SoundManager` of the
//! settings plus effects to the host.

use super::{ScreenEnv, close};
use crate::msg::{ElementMsg, Param, UiEffect, code};
use crate::widgets::int_param;

/// Number of steps of the volume sliders (overrides the 20 of the `.dlg`).
const VOLUME_STEPS: i32 = 1000;

/// Test sound of the sound effects slider (`SoundManager_PlayEvent2D("SFX_TEST", 0x10, cat. 1, loop 0)`).
const SFX_TEST: &str = "SFX_TEST";
const SFX_TEST_FLAGS: u32 = 0x10;

/// Test sentence of the speech slider (`PlayMemoryStream(..., 0x29, ..., cat. 2)`, `0x004D1C31`...`0x004D1C8A`).
const SPEECH_TEST: &str = "data/sprache/testtext.ogg";

/// Category of a volume slider (0 = music, 1 = sound effects, 2 = speech; index in `m_Volumes`).
fn category(element: &str) -> Option<u8> {
    match element {
        "musicvolume" => Some(0),
        "soundvolume" => Some(1),
        "speachvolume" => Some(2),
        _ => None,
    }
}

fn put_int(msg: &mut ElementMsg, name: &str, v: i32) {
    msg.params.insert(name.into(), Param::Int(v));
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let v = int_param(msg, "int");
    if let Some(cat) = category(&msg.element) {
        return volume(msg, env, cat, v);
    }
    let sound = &mut env.config.sound;
    match (msg.element.as_str(), msg.code) {
        // `0x004D19A2`...`0x004D19CE`: `EngineConfig.UnitResponse` raw.
        ("unitresponse", code::QUERY_VALUE) => {
            put_int(msg, "int", env.config.unit_response);
            1
        }
        ("unitresponse", code::VALUE_CHANGED) => {
            if let Some(v) = v {
                env.config.unit_response = v;
            }
            1
        }
        // `0x004D1A53`...`0x004D1A92`: `m_RevertChannels` = (v != 0), no reinitialization.
        ("revertchannels", code::QUERY_VALUE) => {
            put_int(msg, "int", i32::from(sound.revert_channels));
            1
        }
        ("revertchannels", code::VALUE_CHANGED) => {
            if let Some(v) = v {
                sound.revert_channels = v != 0;
            }
            1
        }
        // `0x004D1ADF`...`0x004D1EB5`: `m_IsHardwareOn` = (v != 0), create the device anew
        // (`SoundManager_CreateInterface`, `SoundManager_InitDevice`; not in the reimplementation), then broadcast
        // `0x161` to the desktop root: the menu music restarts.
        ("3dsound", code::QUERY_VALUE) => {
            put_int(msg, "int", i32::from(sound.is_hardware_on));
            1
        }
        ("3dsound", code::VALUE_CHANGED) => {
            if let Some(v) = v {
                sound.is_hardware_on = v != 0;
            }
            env.effects.push(UiEffect::RestartMusic);
            1
        }
        // `0x004D1DC1`...`0x004D1EDD` (jump table `0x004D1FDC`): driver list of the host, `m_DriverID`; a different
        // index creates the device anew (`UiEffect::SetSoundDriver`, the host opens the device) and restarts the menu
        // music (`0x161`).
        ("sounddriver", code::QUERY_VALUE) => {
            put_int(msg, "int", sound.driver_id);
            1
        }
        ("sounddriver", code::QUERY_ITEMS) => {
            let items = env.sound_drivers.to_vec();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        ("sounddriver", code::FREE_ITEMS) => 1,
        ("sounddriver", code::ITEM_SELECTED) => {
            let Some(index) = v else {
                return 0;
            };
            if index != sound.driver_id {
                sound.driver_id = index;
                env.effects.push(UiEffect::SetSoundDriver(index));
                env.effects.push(UiEffect::RestartMusic);
            }
            1
        }
        // `0x004D1F41`, `0x004D1F80`...`0x004D1FB6`: `{3,"soundoptions2"}`, `{4,"gameplayoptions2"}`, then the
        // remembered return command (`FUN_004CDD20`, free afterwards); return value 0. `RETURN2` in `SOUNDOPTIONS3`
        // thus closes `soundoptions2`, not its own dialog (O-100).
        ("return" | "return2", code::TRIGGERED) => {
            env.queue.push(close("soundoptions2", true));
            env.queue.push(close("gameplayoptions2", false));
            if let Some(cmd) = env.state.return_cmd.take() {
                env.queue.push(cmd);
            }
            0
        }
        _ => -1,
    }
}

/// Volume slider: code 6 `resolution 1000`, code 0 volume of the category, code 1 sets it 1:1 (0...999) at once;
/// `soundvolume` plays the test sound on **every** message (O-100), `speachvolume` the test sentence when the
/// previous one has faded out. Without `int` on code 1 return value 0 (`0x004D1FBF`).
fn volume(msg: &mut ElementMsg, env: &mut ScreenEnv, cat: u8, v: Option<i32>) -> i32 {
    let slot = usize::from(cat);
    match msg.code {
        code::QUERY_VALUE => {
            put_int(msg, "int", env.config.sound.volumes[slot]);
            1
        }
        code::QUERY_RESOLUTION => {
            put_int(msg, "resolution", VOLUME_STEPS);
            1
        }
        code::VALUE_CHANGED => {
            let Some(value) = v else {
                return 0;
            };
            env.config.sound.volumes[slot] = value;
            env.effects.push(UiEffect::SetVolume {
                category: cat,
                value,
            });
            match cat {
                // `0x004D1B8D`...`0x004D1BBA`: test sound without a lock.
                1 => env.effects.push(UiEffect::PlaySfx {
                    event: SFX_TEST.into(),
                    flags: SFX_TEST_FLAGS,
                    category: 1,
                }),
                // `IsSoundPlaying(0x00671ED4)`: the host reports after every frame whether the sentence is still
                // playing; one just started counts as playing until then (the original queries the new channel).
                2 if !env.state.speech_playing => {
                    env.effects.push(UiEffect::PlaySpeech {
                        path: SPEECH_TEST.into(),
                    });
                    env.state.speech_playing = true;
                }
                // `0x004D1D0D`...`0x004D1D2A`: music additionally with `resolution` in the response.
                0 => put_int(msg, "resolution", VOLUME_STEPS),
                _ => {}
            }
            1
        }
        _ => -1,
    }
}
