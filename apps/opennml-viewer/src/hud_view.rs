//! Game UI in the viewer (`--hud` and control command `hud`): message bar and status bar as in
//! `opennml-game` through a UI renderer ([`GameSession::hud_cmds`]) instead of the message lines in the
//! overlay. Drawn with the UI layer of the dialog layer (`--dialog`), otherwise with its own
//! ([`HudLayer`], created like the dialog browser). Without `--hud` everything stays as before.
//! Display only: no checksum changes.

use anyhow::Result;
use opennml_client::game_session::GameSession;
use opennml_client::keys::InputSource;
use opennml_client::ui_layer::{UiLayer, atlas_generator, screen_of};
use opennml_client::vk::{HeldVk, vk_of_german, vk_of_layout};
use opennml_rda::Vfs;
use opennml_render::UiRenderer;
use opennml_ui::assets::Texts;
use opennml_ui::draw::DrawCmd;
use winit::keyboard::KeyCode;

use crate::config::Config;
use crate::dialogs::DialogLayer;

/// Own UI layer including renderer for the game UI when there is no dialog layer (a renderer belongs to
/// exactly one `UiAssets`).
pub struct HudLayer {
    layer: UiLayer,
    renderer: UiRenderer,
}

impl HudLayer {
    /// Loads language file, texts and fonts (width `screen_width`, O-58) and creates the renderer for `format`.
    pub fn new(
        vfs: &Vfs,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        language_tag: &str,
        screen_width: u32,
    ) -> Result<Self> {
        let generator = atlas_generator(vfs);
        Ok(Self {
            layer: UiLayer::new(vfs, language_tag, screen_width, generator.as_deref())?,
            renderer: UiRenderer::new(device, format),
        })
    }
}

/// Command `hud` or switch `--hud`: sets the switch of the settings and reports whether the viewer has to
/// create its own drawing layer for it (switched on, without a dialog layer and without an existing layer).
pub fn apply_hud(config: &mut Config, on: bool, dialogs: bool, own_layer: bool) -> bool {
    config.hud = on;
    on && !dialogs && !own_layer
}

/// Draw commands of the game UI for a frame of size `size` with the layer of the dialog layer, otherwise
/// the own one; without both none. The game values come from `texts` (the text table of the viewer). Then
/// the scene fade ([`GameSession::fade_cmds`]) and the info windows ([`GameSession::desktop_cmds`]), as in
/// [`GameSession::over_world_cmds`] (re/campaign-ui D6.4); the viewer draws the dialogs of the dialog layer
/// over it, the combat indicator with egui. The viewer calls this in every drawn frame with the same layer
/// (the info windows measure themselves on the first drawing and hold their texture IDs).
pub fn hud_cmds(
    session: &GameSession,
    texts: &Texts,
    vfs: &Vfs,
    size: (u32, u32),
    dialogs: Option<&mut DialogLayer>,
    own: Option<&mut HudLayer>,
) -> Vec<DrawCmd> {
    let layer = match (dialogs, own) {
        (Some(dialogs), _) => dialogs.ui_layer_mut(),
        (None, Some(own)) => &mut own.layer,
        (None, None) => return Vec::new(),
    };
    let mut ctx = layer.context(vfs, screen_of(size));
    let mut out = session.hud_cmds(&mut ctx, texts);
    out.extend(session.fade_cmds(ctx.screen));
    out.extend(session.desktop_cmds(&mut ctx, texts));
    out
}

/// Does the viewer occupy the key `code` on the map itself (`shift`: Shift held)?
/// The list follows `App::key_pressed` and the help (`opennml_client::game::overlay::HELP`): Esc, F1 to F12, B, N,
/// T, R, I, K, J, L, P, H, X, F, O, G, Shift + 1...5 (stance), the camera (arrows, Page Up/Down, Home/End,
/// Insert/Delete, W/A/S/D and Q/E of the free camera) and Shift/Ctrl (selection, queueing, cyclic, free
/// camera fast). Everything else - the remaining digits, `+`/`-`, Pause, Print, Tab, Enter, space bar ... -
/// is free on the map.
pub fn viewer_key(code: KeyCode, shift: bool) -> bool {
    use KeyCode as K;
    match code {
        K::Digit1 | K::Digit2 | K::Digit3 | K::Digit4 | K::Digit5 => shift,
        K::Escape
        | K::F1
        | K::F2
        | K::F3
        | K::F4
        | K::F5
        | K::F6
        | K::F7
        | K::F8
        | K::F9
        | K::F10
        | K::F11
        | K::F12
        | K::KeyB
        | K::KeyN
        | K::KeyT
        | K::KeyR
        | K::KeyI
        | K::KeyK
        | K::KeyJ
        | K::KeyL
        | K::KeyP
        | K::KeyH
        | K::KeyX
        | K::KeyF
        | K::KeyO
        | K::KeyG
        | K::ArrowUp
        | K::ArrowDown
        | K::ArrowLeft
        | K::ArrowRight
        | K::PageUp
        | K::PageDown
        | K::Home
        | K::End
        | K::Insert
        | K::Delete
        | K::KeyW
        | K::KeyA
        | K::KeyS
        | K::KeyD
        | K::KeyQ
        | K::KeyE
        | K::ShiftLeft
        | K::ShiftRight
        | K::ControlLeft
        | K::ControlRight => true,
        _ => false,
    }
}

/// Key route of the original in the viewer with `--hud`: the table of the held keys as in the host of
/// `opennml-game` (`HeldVk`: VK of the active key layout in press order), so that chords like `CONTROL 1` arise.
/// It follows every key (also Ctrl and Shift of the viewer); a press on a free key ([`viewer_key`]) yields
/// the key set of the event for `Hotkeys::resolve`. Without `--hud` nothing is passed on.
#[derive(Debug, Default)]
pub struct OriginalKeys {
    held: HeldVk,
}

/// VK of a key by input source (decision 9.4): the user input follows the active layout like the original, the
/// control channel keeps the fixed German table so that test scripts run independent of the machine layout.
pub fn vk_of_source(source: InputSource, code: KeyCode) -> Option<u8> {
    match source {
        InputSource::User => vk_of_layout(code),
        InputSource::Control => vk_of_german(code),
    }
}

impl OriginalKeys {
    /// One key: `pressed` presses (every repeat again, O-300), otherwise releases (the table syncs at once,
    /// the viewer has no round of the window loop like the original). With `dispatch` (`--hud` and map view)
    /// a press on a free key returns the key set; otherwise `None`.
    pub fn key(
        &mut self,
        dispatch: bool,
        code: KeyCode,
        vk: Option<u8>,
        pressed: bool,
        shift: bool,
    ) -> Option<Vec<u8>> {
        if !pressed {
            self.held.release(code);
            self.held.begin_round();
            return None;
        }
        self.held.press(code, vk)?;
        (dispatch && !viewer_key(code, shift)).then(|| self.held.held())
    }

    /// Focus loss: release everything (winit does not report the release then).
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

/// Draws `cmds` from [`hud_cmds`] with the same layer over the existing image (`LoadOp::Load`).
#[allow(clippy::too_many_arguments)]
pub fn draw_hud(
    cmds: &[DrawCmd],
    dialogs: Option<&mut DialogLayer>,
    own: Option<&mut HudLayer>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    size: (u32, u32),
) {
    if cmds.is_empty() {
        return;
    }
    match (dialogs, own) {
        (Some(dialogs), _) => dialogs.draw_cmds(device, queue, encoder, target, size, cmds),
        (None, Some(own)) => {
            own.layer.draw(
                &mut own.renderer,
                device,
                queue,
                encoder,
                target,
                size,
                cmds,
            );
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_control::{Command, Request};

    #[test]
    fn hud_command_toggles_the_viewer_hud() {
        // `{"cmd": "hud", "on": ...}` sets the switch of the settings (default off); an own layer is needed
        // only without a dialog layer and only once.
        let mut config = Config::default();
        assert!(!config.hud);
        let request: Request = serde_json::from_str(r#"{"id":1,"cmd":"hud","on":true}"#).unwrap();
        let Command::Hud { on } = request.command else {
            panic!("not a hud command");
        };
        assert!(apply_hud(&mut config, on, false, false), "own layer");
        assert!(config.hud);
        assert!(!apply_hud(&mut config, true, false, true), "layer present");
        assert!(!apply_hud(&mut config, true, true, false), "dialog layer");
        assert!(!apply_hud(&mut config, false, false, false));
        assert!(!config.hud);
    }
    /// The control channel keeps the German table on every machine (decision 9.4): Z is `0x59` there, Y `0x5A`.
    #[test]
    fn control_source_keys_use_the_german_table() {
        use winit::keyboard::KeyCode as K;

        assert_eq!(vk_of_source(InputSource::Control, K::KeyZ), Some(0x59));
        assert_eq!(vk_of_source(InputSource::Control, K::KeyY), Some(0x5A));
        let mut keys = OriginalKeys::default();
        let vk = vk_of_source(InputSource::Control, K::KeyZ);
        assert_eq!(keys.key(true, K::KeyZ, vk, true, false), Some(vec![0x59]));
    }

    /// With `--hud` the keys without a viewer binding go as the key set of the event into the key route
    /// (`Hotkeys::resolve`, then `GameSession::run_hotkey`); Ctrl and Shift belong to the viewer, but are in
    /// the table; every repeat is an event (O-300).
    #[test]
    fn viewer_with_hud_passes_free_keys_to_the_hotkeys() {
        use opennml_client::game::hotkeys::{HotkeyCommand as C, Hotkeys, KeyContext};
        use opennml_client::game::input_gate::InputGate;
        use opennml_ui::keys::parse_chord;
        use winit::keyboard::KeyCode as K;

        let mut keys = OriginalKeys::default();
        assert_eq!(
            keys.key(
                true,
                K::ControlLeft,
                vk_of_german(K::ControlLeft),
                true,
                false
            ),
            None
        );
        assert_eq!(
            keys.key(true, K::Digit1, vk_of_german(K::Digit1), true, false),
            Some(vec![0x11, 0x31])
        );
        assert_eq!(
            keys.key(true, K::Digit1, vk_of_german(K::Digit1), true, false),
            Some(vec![0x11, 0x31]),
            "repeat"
        );
        assert_eq!(
            keys.key(true, K::Digit1, vk_of_german(K::Digit1), false, false),
            None
        );
        assert_eq!(
            keys.key(
                true,
                K::ControlLeft,
                vk_of_german(K::ControlLeft),
                false,
                false
            ),
            None
        );
        assert_eq!(
            keys.key(
                true,
                K::BracketRight,
                vk_of_german(K::BracketRight),
                true,
                false
            ),
            Some(vec![0xBB])
        );
        assert_eq!(
            keys.key(true, K::Pause, vk_of_german(K::Pause), true, false),
            Some(vec![0xBB, 0x13])
        );
        keys.clear();
        assert_eq!(
            keys.key(true, K::Digit0, vk_of_german(K::Digit0), true, false),
            Some(vec![0x30])
        );

        // The same resolution as in `opennml-game`: binding before `keys.ini`.
        let hotkeys = Hotkeys {
            bindings: vec![
                (parse_chord("CONTROL 1"), C::StoreGroup(1)),
                (parse_chord("1"), C::SelectGroup(1)),
            ],
            keys: opennml_formats::keys_ini::parse_keys_ini(
                "KEY 113 0 0 0 /game/changegamespeed(+1)\n",
            ),
            keydefs: Vec::new(),
        };
        let key_map = [(113, "+".to_string())];
        let resolve = |held: &[u8], key_map: &[(i32, String)], hotkeys: &Hotkeys| {
            hotkeys.resolve(held, &InputGate::default(), &KeyContext::default(), key_map)
        };
        assert_eq!(
            resolve(&[0x11, 0x31], &key_map, &hotkeys),
            Some(C::StoreGroup(1))
        );
        assert_eq!(
            resolve(&[0x31], &key_map, &hotkeys),
            Some(C::SelectGroup(1))
        );
        assert_eq!(
            resolve(&[0xBB], &key_map, &hotkeys),
            Some(C::ChangeGameSpeed(1))
        );
        // With the data of the installation (`bindings2.ini`, `keys.ini`, `defaultkeys.ini`).
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let hotkeys = Hotkeys::load(&vfs);
        let key_map = opennml_client::settings::default_config(&vfs).key_map;
        assert_eq!(
            resolve(&[0x11, 0x31], &key_map, &hotkeys),
            Some(C::StoreGroup(1))
        );
        assert_eq!(resolve(&[0x13], &key_map, &hotkeys), Some(C::TogglePause));
        assert_eq!(
            resolve(&[0xBB], &key_map, &hotkeys),
            Some(C::ChangeGameSpeed(1))
        );
    }

    /// Without `--hud` no key goes into the key route; with `--hud` the viewer keeps its keys (`key_pressed`,
    /// help), also Shift + 1...5 (stance).
    #[test]
    fn viewer_without_hud_keeps_its_keys() {
        use winit::keyboard::KeyCode as K;

        let mut keys = OriginalKeys::default();
        for code in [K::Digit1, K::BracketRight, K::Pause, K::KeyU, K::Tab] {
            assert_eq!(
                keys.key(false, code, vk_of_german(code), true, false),
                None,
                "{code:?}"
            );
            assert_eq!(
                keys.key(false, code, vk_of_german(code), false, false),
                None,
                "{code:?}"
            );
        }
        for code in [
            K::Escape,
            K::F1,
            K::F2,
            K::F12,
            K::KeyB,
            K::KeyN,
            K::KeyT,
            K::KeyR,
            K::KeyI,
            K::KeyK,
            K::KeyJ,
            K::KeyL,
            K::KeyP,
            K::KeyH,
            K::KeyX,
            K::KeyF,
            K::KeyO,
            K::KeyG,
            K::ArrowUp,
            K::PageDown,
            K::Home,
            K::End,
            K::Insert,
            K::Delete,
            K::KeyW,
            K::KeyQ,
            K::KeyE,
            K::ShiftLeft,
            K::ControlLeft,
        ] {
            assert!(viewer_key(code, false), "{code:?}");
            assert_eq!(
                keys.key(true, code, vk_of_german(code), true, false),
                None,
                "{code:?}"
            );
            keys.key(true, code, vk_of_german(code), false, false);
        }
        // Shift + 1...5 is the stance of the viewer, the digit alone the group of the original.
        assert!(viewer_key(K::Digit3, true));
        assert!(!viewer_key(K::Digit3, false));
        assert!(!viewer_key(K::Digit6, true));
        assert_eq!(
            keys.key(true, K::Digit3, vk_of_german(K::Digit3), true, true),
            None
        );
    }
}
