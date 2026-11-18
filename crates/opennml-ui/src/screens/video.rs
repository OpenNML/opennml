//! Video options `VIDEOOPTIONS` (`FUN_004CE2A0`, call `0x004DC318`; re/menu-flow, `VIDEOOPTIONS`). On opening (code 5
//! of the frame) the working copy [`VideoCopy`] is created; the comboboxes change only it, only `ACCEPT` writes
//! resolution and qualities back. Gamma takes effect at once (O-98, O-99). No back button.

use super::{ScreenEnv, VideoCopy};
use crate::msg::{AppCmd, DisplayMode, ElementMsg, Param, UiEffect, code};
use crate::widgets::int_param;

/// Allowed widths of the mode list (`0x004D1475`...`0x004D149D`); the height is arbitrary.
const WIDTHS: [i32; 5] = [800, 1024, 1280, 1400, 1600];

/// Texts of the comboboxes in the order of the entries (`FUN_0055FFE0`).
const PARTICLES_TEXTS: [i32; 3] = [15043, 15042, 15041];
const SHADOW_TEXTS: [i32; 4] = [15054, 15053, 15052, 15051];
const TEXTURE_TEXTS: [i32; 3] = [15010, 15011, 15012];
const TERRAIN_TEXTS: [i32; 3] = [15014, 15015, 15016];

/// Mode list like the constructor of the `MenuListener` (`0x004D1380`, once at start): width in {800, 1024, 1280,
/// 1400, 1600}, 16 or 32 bits (formats `0x15`/`0x16` or `0x17`...`0x19`), for each (W, H, bits) the first entry
/// (`0x004D1558`...`0x004D158B`; [`DisplayMode`] does not know the frame rate), order of the input (O-98).
pub fn filter_display_modes(raw: &[DisplayMode]) -> Vec<DisplayMode> {
    let mut out: Vec<DisplayMode> = Vec::new();
    for mode in raw {
        if WIDTHS.contains(&mode.width) && matches!(mode.bits, 16 | 32) && !out.contains(mode) {
            out.push(*mode);
        }
    }
    out
}

/// `f32` -> `i32` like `__ftol2` (`0x005D2A58`): `FISTP qword` rounding toward 0 and the low dword of it. NaN, +-inf
/// and |x| >= 2^63 give the indefinite value `0x8000_0000_0000_0000`, so 0; 2^31 <= |x| < 2^63 the low 32 bits.
/// (`opennml-sim` `fx.rs` has an older, simplified `ftol` ("indefinite outside `i32`"); that stays as it is.)
pub(super) fn ftol(value: f32) -> i32 {
    if !value.is_finite() || value.abs() >= 9.223_372e18 {
        0
    } else {
        (value as i64) as i32
    }
}

/// Slider value for gamma (`0x004CE449`...`0x004CE465`): `min(ftol((G - 0.5f) * 50f), 99)` in f32 (24-bit x87),
/// without a lower limit (O-99).
pub fn gamma_to_slider(g: f32) -> i32 {
    ftol((g - 0.5) * 50.0).min(99)
}

/// Gamma for the slider value (`0x004CE40A`...`0x004CE420`): `(f32)v * 0.02f + 0.5f`, each step rounded in f32.
/// The round trip yields a slider one smaller for 33 of the values 0...99 (O-99).
pub fn slider_to_gamma(v: i32) -> f32 {
    v as f32 * 0.02 + 0.5
}

/// The working copy; before the first opening the statics of the original (all 0).
fn copy<'a>(env: &'a mut ScreenEnv) -> &'a mut VideoCopy {
    env.state.video_copy.get_or_insert(VideoCopy::ZERO)
}

/// Combobox entries as texts (`array`, code 2).
fn put_texts(msg: &mut ElementMsg, env: &ScreenEnv, ids: &[i32]) {
    let items = ids
        .iter()
        .map(|&id| {
            env.texts.resolve(id);
            env.texts.display(id)
        })
        .collect();
    msg.params.insert("array".into(), Param::Array(items));
}

fn put_int(msg: &mut ElementMsg, name: &str, v: i32) {
    msg.params.insert(name.into(), Param::Int(v));
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    // `0x004CE30B`...`0x004CE366`: "frame opened" fills the working copy (anew on every opening).
    if msg.code == code::QUERY_STATUS && msg.element == msg.dialog {
        env.state.video_copy = Some(VideoCopy::from_config(env.config));
    }
    let v = int_param(msg, "int");
    match (msg.element.as_str(), msg.code) {
        // `0x004CE449`: value from the copy, without a lower limit (O-99).
        ("gamma", code::QUERY_VALUE) => {
            let g = copy(env).gamma;
            put_int(msg, "int", gamma_to_slider(g));
            1
        }
        ("gamma", code::QUERY_RESOLUTION) => {
            put_int(msg, "resolution", 100);
            1
        }
        // `0x004CE3EA`...`0x004CE43E`: copy, gamma ramp (`Renderer_SetGammaRamp`, in the reimplementation without a
        // display effect) and `EngineConfig.Gamma` at once.
        ("gamma", code::VALUE_CHANGED) => {
            if let Some(v) = v {
                let g = slider_to_gamma(v);
                copy(env).gamma = g;
                env.effects.push(UiEffect::SetGamma(g));
                env.config.gamma = g;
            }
            1
        }
        // `0x004CE4B0`...`0x004CE540`: list backwards, index = 2 - value (without a range check).
        ("particles", code::QUERY_VALUE) => {
            let index = 2i32.wrapping_sub(copy(env).particles);
            put_int(msg, "int", index);
            1
        }
        ("particles", code::QUERY_ITEMS) => {
            put_texts(msg, env, &PARTICLES_TEXTS);
            1
        }
        ("particles", code::ITEM_SELECTED) => {
            if let Some(i) = v {
                copy(env).particles = 2i32.wrapping_sub(i);
            }
            1
        }
        // `0x004CE58B`...`0x004CE60E`: list backwards, index = 3 - value.
        ("shadow", code::QUERY_VALUE) => {
            let index = 3i32.wrapping_sub(copy(env).shadow);
            put_int(msg, "int", index);
            1
        }
        ("shadow", code::QUERY_ITEMS) => {
            put_texts(msg, env, &SHADOW_TEXTS);
            1
        }
        ("shadow", code::ITEM_SELECTED) => {
            if let Some(i) = v {
                copy(env).shadow = 3i32.wrapping_sub(i);
            }
            1
        }
        // `0x004CE64C`...`0x004CE6D6`, `0x004CE727`...`0x004CE7A6`: index = value.
        ("texturequality", code::QUERY_VALUE) => {
            let value = copy(env).texture;
            put_int(msg, "int", value);
            1
        }
        ("texturequality", code::QUERY_ITEMS) => {
            put_texts(msg, env, &TEXTURE_TEXTS);
            1
        }
        ("texturequality", code::ITEM_SELECTED) => {
            if let Some(i) = v {
                copy(env).texture = i;
            }
            1
        }
        ("terrainquality", code::QUERY_VALUE) => {
            let value = copy(env).terrain;
            put_int(msg, "int", value);
            1
        }
        ("terrainquality", code::QUERY_ITEMS) => {
            put_texts(msg, env, &TERRAIN_TEXTS);
            1
        }
        ("terrainquality", code::ITEM_SELECTED) => {
            if let Some(i) = v {
                copy(env).terrain = i;
            }
            1
        }
        ("resolution", c) => resolution(msg, env, c, v),
        // `0x004CE7E6`...`0x004CE8F8`; then jump to the return value -1 (`0x004CE8FD`).
        ("accept", code::TRIGGERED) => {
            accept(env);
            -1
        }
        _ => -1,
    }
}

/// Combobox `resolution` over the filtered mode list (`0x004CE940`...`0x004CEB5A`).
fn resolution(msg: &mut ElementMsg, env: &mut ScreenEnv, c: u32, v: Option<i32>) -> i32 {
    let modes = filter_display_modes(env.modes);
    match c {
        // `0x004CEAC8`...`0x004CEB3A`: first entry equal to the copy; without a match no answer, -1 (O-98).
        code::QUERY_VALUE => {
            let current = copy(env).mode;
            match modes.iter().position(|m| *m == current) {
                Some(i) => {
                    put_int(msg, "int", i32::try_from(i).unwrap_or(i32::MAX));
                    1
                }
                None => -1,
            }
        }
        // `0x004CE962`...`0x004CE9E9`: `swprintf(L"%d x %d x %d", W, H, bits)` per entry.
        code::QUERY_ITEMS => {
            let items = modes
                .iter()
                .map(|m| format!("{} x {} x {}", m.width, m.height, m.bits))
                .collect();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        // `0x004CE9FE`...`0x004CEA3D`: free the texts (the reimplementation holds none).
        code::FREE_ITEMS => 1,
        // `0x004CEA52`...`0x004CEAA3`: only an index in the list changes the copy, otherwise return value 0.
        code::ITEM_SELECTED => {
            let picked = v
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| modes.get(i).copied());
            match picked {
                Some(mode) => {
                    copy(env).mode = mode;
                    1
                }
                None => 0,
            }
        }
        _ => -1,
    }
}

/// `ACCEPT`: 1. gamma from the copy; 2. resolution changed -> `GameApp_SetDisplayMode` (effect to the host, which
/// also resets the pointer - `FUN_004CD6A0(1)`, `FUN_004CD4C0(7, 1)`), then the three fields; 3. one of the four
/// qualities changed -> write all four, queue `{0x1F}`.
fn accept(env: &mut ScreenEnv) {
    let copy = *copy(env);
    let c = &mut *env.config;
    c.gamma = copy.gamma;
    let mode = copy.mode;
    if (c.screen_x_size, c.screen_y_size, c.screen_depth) != (mode.width, mode.height, mode.bits) {
        env.effects.push(UiEffect::SetDisplayMode(mode));
        c.screen_x_size = mode.width;
        c.screen_y_size = mode.height;
        c.screen_depth = mode.bits;
    }
    let quality = (copy.particles, copy.shadow, copy.texture, copy.terrain);
    let current = (
        c.particles_quality,
        c.shadow_quality,
        c.texture_quality,
        c.terrain_quality,
    );
    if quality != current {
        c.terrain_quality = copy.terrain;
        c.texture_quality = copy.texture;
        c.shadow_quality = copy.shadow;
        c.particles_quality = copy.particles;
        env.queue.push(AppCmd::ReloadLevel);
    }
}
