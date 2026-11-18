//! Game options `GAMEPLAYOPTIONS` / `GAMEPLAYOPTIONS2` (/`3`, missing in the shipped data) (`FUN_004CEB80`, called at
//! `0x004DC334` and `0x004DC9FE`; re/menu-flow, `GAMEPLAYOPTIONS`). All values go into the
//! settings immediately; saving happens via `OPTIONS OK` or at program end.

use opennml_formats::engine_ini::EngineConfig;

use super::{NetRoleView, ScreenEnv};
use crate::msg::{ElementMsg, Param, UiEffect, code};
use crate::widgets::int_param;

/// Status "hidden" (`HIDE`).
const HIDE: i32 = 0x1000_0000;

/// Number of steps of the four sliders (the `.dlg` files say `RESOLUTION 10`, O-101).
const SLIDER_STEPS: i32 = 20;

/// Texts of the combobox `occlusion` (Off, Own, All), index = value.
const OCCLUSION_TEXTS: [i32; 3] = [15081, 15082, 15083];

/// Field of a slider or switch with `int` value (1:1, unchecked) and whether it is a slider.
fn int_field<'a>(config: &'a mut EngineConfig, element: &str) -> Option<(&'a mut i32, bool)> {
    Some(match element {
        "mousescroll" => (&mut config.mouse_scroll, true),
        "enablemousescroll" => (&mut config.enable_mouse_scroll, false),
        "keyboardscroll" => (&mut config.keyboard_scroll, true),
        "gamespeed" => (&mut config.game_speed, true),
        "enhancedtooltips" => (&mut config.enhanced_tooltips, false),
        "mousespeed" => (&mut config.mouse_speed, true),
        "lockcamera" => (&mut config.lock_camera, false),
        "zoom" => (&mut config.enable_zoom, false),
        _ => return None,
    })
}

fn put_int(msg: &mut ElementMsg, name: &str, v: i32) {
    msg.params.insert(name.into(), Param::Int(v));
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    let v = int_param(msg, "int");
    let c = msg.code;
    if let Some((field, is_slider)) = int_field(env.config, &msg.element) {
        return match c {
            code::QUERY_VALUE => {
                put_int(msg, "int", *field);
                1
            }
            // In the original, `MOUSESPEED` additionally sets the input object immediately (`0x0066FA60+0x6C = n * 6
            // + 40`, `0x004CEFA2`...`0x004CEFCD`); the reimplementation reads the value where it is needed (in the
            // game).
            code::VALUE_CHANGED => {
                if let Some(v) = v {
                    *field = v;
                }
                1
            }
            code::QUERY_RESOLUTION if is_slider => {
                put_int(msg, "resolution", SLIDER_STEPS);
                1
            }
            // `0x004CF274[5]` -> `0x004CEE7B`...`0x004CEECC`: `gamespeed` hidden for non-hosts in a network session
            // (`0x004CEEB5`...`0x004CEEC2`, `FUN_004E85F0`: `status 0x10000000`, re/multiplayer-menus L.7.1, E-315;
            // in the network only reachable via `gameplayoptions2`); otherwise - and always without a network session
            // - no `status`, return value 1 (`JZ 0x004CF215`). The network role comes from the match
            // (`GameInfo::net`; after a host migration the host sets it anew, `MenuApp::set_net_role`). The other
            // sliders do not answer on code 5.
            code::QUERY_STATUS if msg.element == "gamespeed" => {
                if env.data.game.and_then(|g| g.net) == Some(NetRoleView::Client) {
                    put_int(msg, "status", HIDE);
                }
                1
            }
            _ => -1,
        };
    }
    let config = &mut *env.config;
    match (msg.element.as_str(), c) {
        // `0x004CEBFB`...`0x004CECD0`: `ShowOccluded`, index = value (unchecked).
        ("occlusion", code::QUERY_VALUE) => {
            put_int(msg, "int", config.show_occluded);
            1
        }
        ("occlusion", code::QUERY_ITEMS) => {
            let items = OCCLUSION_TEXTS
                .iter()
                .map(|&id| {
                    env.texts.resolve(id);
                    env.texts.display(id)
                })
                .collect();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        ("occlusion", code::ITEM_SELECTED) => {
            if let Some(i) = v {
                config.show_occluded = i;
            }
            1
        }
        // `0x004CF0F8`...`0x004CF209`: `HardwareCursor` (code 0 "== 1"), code 1 = (v != 0) and immediately
        // `Cursor_SetHardwareCursorVisible`; without `int` return value 0 without call (`0x004CF10B` -> `0x004CF1A3`).
        ("hardwaremaus", code::QUERY_VALUE) => {
            put_int(msg, "int", i32::from(config.hardware_cursor));
            1
        }
        ("hardwaremaus", code::VALUE_CHANGED) => {
            let Some(v) = v else {
                return 0;
            };
            config.hardware_cursor = v != 0;
            env.effects
                .push(UiEffect::SetHardwareCursor(config.hardware_cursor));
            1
        }
        // `0x004CF18E`...`0x004CF1F8`: only `IsMenuLevel` = (v != 0), no command `0x20`/`0x21` - takes effect only at
        // the next reading of `start.dlg` (O-101); without `int` return value 0 (`0x004CF1A1` -> `0x004CF1A3`).
        ("menulevel", code::QUERY_VALUE) => {
            put_int(msg, "int", i32::from(config.is_menu_level));
            1
        }
        ("menulevel", code::VALUE_CHANGED) => {
            let Some(v) = v else {
                return 0;
            };
            config.is_menu_level = v != 0;
            1
        }
        _ => -1,
    }
}
