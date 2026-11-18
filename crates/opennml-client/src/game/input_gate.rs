//! Shared query `InputGate`: what input may do in a frame under UI lock, cinemascope, dialog and script camera
//! (re/campaign-ui D6.10). The campaign UI fills it in (`GameSession::input_gate`, `SessionView::input_gate`); the
//! game UI (minimap, pointer, keys, control groups, pause/speed, diplomacy) reads it. Shape, fields, methods and
//! bodies are given by re/campaign-ui D6.10; the additions of the game UI (re/game-ui D5.3) are `campaign`,
//! `menu_open`, [`InputGate::minimap_swallows_clicks`] and [`InputGate::pointer_visible`], plus [`InputGate::names`]
//! for the control channel. Callers read only the methods, not the fields.

use crate::game::sim_host::SimHost;

/// What input may do in this frame (original: GE+0x600, GE+0x104, GE+0x601; re/campaign-ui D6.10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputGate {
    /// UI lock of the scripts (18/19, `MissionState::ui_locked`, `GE+0x600`).
    pub ui_locked: bool,
    /// Cinemascope (147/148, `MissionState::cinemascope`, `GE+0x104`).
    pub cinemascope: bool,
    /// A dialog script is running (`GE+0x601`, `opennml_sim::script::dialog_running`).
    pub dialog_running: bool,
    /// The script camera is traveling, rotating or has an open cut (`ScriptCamera::running`; moves of the player
    /// with Home do not count).
    pub script_camera: bool,
    /// Game type -1 (campaign, direct start; `Engine+0x108`).
    pub campaign: bool,
    /// Game menu, diplomacy or quest menu open (`Engine+0x2CCC ≠ 0`).
    pub menu_open: bool,
    /// Game type -2, the menu level behind the main menu (`Engine+0x108 == −2`, re/menu-level ML.6, ML.8, ML.15
    /// no. 5). No input reaches it except the screenshot (O-441, O-442).
    pub menu_level: bool,
}

impl InputGate {
    /// Player camera (keys, edge, wheel, freehand): not under the lock (`GameEngine_UpdateCamera` `0x0043C7DC`) and
    /// not while the script camera runs (a binding 175 alone does not lock), and never in the menu level
    /// (`0x0043C80F`: game type -2 skips the whole block, O-441).
    pub fn player_camera(&self) -> bool {
        !self.ui_locked && !self.script_camera && !self.menu_level
    }

    /// Left click, selection box, double click and wheel to the world (root): not under the lock (root `+0x60`,
    /// `0x00449B00`; E-246, re/campaign-ui D6.10). The bar sees the mouse first and stays open. Never in the menu level
    /// (O-442): according to the reference run the world takes no click there (`0x0047A490` draws selection rings in
    /// cinemascope too, the photos show none after clicks).
    pub fn world_click(&self) -> bool {
        !self.ui_locked && !self.menu_level
    }

    /// Right-click command to the world: always, as in the original (`0x00457C50` does not check the lock,
    /// re/campaign-ui D6.10, O-233). With a minimap, however, the right press ends there under the lock
    /// ([`Self::minimap_swallows_clicks`]). Never in the menu level (O-442, like [`Self::world_click`]).
    pub fn right_click_command(&self) -> bool {
        !self.menu_level
    }

    /// Shortcuts from `keys.ini` (abilities, speed, F1/F4 and the quicksave/quickload built on them, xbutton): not
    /// under the lock (`FUN_004C6050` `0x004C61FD`, re/game-ui D5.3 stage 6, O-304). Never in the menu level (O-442):
    /// the speed keys (`FUN_0042F050` without a mode check) do not work there in the original, so the stage is not
    /// reached.
    pub fn keys_ini(&self) -> bool {
        !self.ui_locked && !self.menu_level
    }

    /// Bindings from `bindings2.ini` (groups, pause, Home, chat): always, they run before the lock check
    /// (`FUN_00411CE0` `0x004C61E3` before `0x004C61FD`, re/game-ui D5.3 stage 5, O-304, O-369). Never in the menu
    /// level (O-442): like [`Self::keys_ini`]; without clicks to the world the selection would stay empty anyway.
    pub fn bindings(&self) -> bool {
        !self.menu_level
    }

    /// Chat window (`showchatwindow`, `FUN_0042EA20`): not in cinemascope (O-325) and not with a menu open
    /// (`Engine+0x2CCC`), never in the menu level (`0x0042EA42`, O-325).
    pub fn chat(&self) -> bool {
        !self.cinemascope && !self.menu_open && !self.menu_level
    }

    /// Screenshot `KD_HARDCOPY`: not under the lock (`FUN_004C6050`, re/game-ui D5.3 stage 8, O-304).
    pub fn hardcopy(&self) -> bool {
        !self.ui_locked
    }

    /// Quest menu by key (`KD_CHECK_QUESTMENU`, `0x0043E1A0`; `ShowQuestMenu` `0x0042E3B0`): without a dialog script,
    /// without the lock and with menu state 0 (re/campaign-ui D6.10). The key is missing from `keymap.ini` (O-305).
    /// Never in the menu level.
    pub fn quest_menu_key(&self) -> bool {
        !self.dialog_running && !self.ui_locked && !self.menu_open && !self.menu_level
    }

    /// Diplomacy dialog: not under the lock (re/campaign-ui D6.10), not in the menu level (`SHIFT D` does not work
    /// there in the original, O-442).
    pub fn diplomacy(&self) -> bool {
        !self.ui_locked && !self.menu_level
    }

    /// Minimap takes effect (clicks, pings): not under the lock (minimap `+0x4C`, re/campaign-ui D6.10), even when
    /// hidden (O-368). Never in the menu level: a click on the hidden minimap does not move the camera (reference run,
    /// O-442).
    pub fn minimap_input(&self) -> bool {
        !self.ui_locked && !self.menu_level
    }

    /// Minimap visible (drawing and ping progress): not in cinemascope (re/campaign-ui D6.10, O-372).
    pub fn minimap_visible(&self) -> bool {
        !self.cinemascope
    }

    /// Under the lock the minimap takes every left press, left release, right press and every right double click on
    /// the whole screen before bar, buttons and world see them (as long as `IsMap` is on; `0x0044A550`/`0x0044A56F`,
    /// O-290). [`Self::minimap_input`] being false therefore does not mean that the click is passed on.
    pub fn minimap_swallows_clicks(&self) -> bool {
        self.ui_locked
    }

    /// Mouse pointer visible: cinemascope hides it only in the campaign (`FUN_00430920` `0x004309FF`, O-299, O-350).
    pub fn pointer_visible(&self) -> bool {
        !(self.cinemascope && self.campaign)
    }

    /// Names of the set fields in a fixed order (fields of the struct) for the control channel
    /// (`ui_state.game.input.gate`); empty without a lock.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.ui_locked, "ui_locked"),
            (self.cinemascope, "cinemascope"),
            (self.dialog_running, "dialog_running"),
            (self.script_camera, "script_camera"),
            (self.campaign, "campaign"),
            (self.menu_open, "menu_open"),
            (self.menu_level, "menu_level"),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect()
    }
}

/// The part of the query that the simulation carries alone: lock and cinemascope from `MissionState`
/// (`GE+0x600`, `GE+0x104`), the running dialog from [`opennml_sim::script::dialog_running`] (`GE+0x601`). Only the host
/// knows the script camera, game type and game menu (`GameSession::input_gate`).
pub fn sim_gate(sim: &SimHost) -> InputGate {
    let mission = &sim.state.script.mission;
    InputGate {
        ui_locked: mission.ui_locked,
        cinemascope: mission.cinemascope,
        dialog_running: opennml_sim::script::dialog_running(&sim.state),
        ..InputGate::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers of all methods in order: player camera, world click, right click, `keys.ini`, `bindings2.ini`, chat,
    /// screenshot, quest menu key, diplomacy, minimap (input, visible, swallows), pointer visible.
    fn answers(g: &InputGate) -> [bool; 13] {
        [
            g.player_camera(),
            g.world_click(),
            g.right_click_command(),
            g.keys_ini(),
            g.bindings(),
            g.chat(),
            g.hardcopy(),
            g.quest_menu_key(),
            g.diplomacy(),
            g.minimap_input(),
            g.minimap_visible(),
            g.minimap_swallows_clicks(),
            g.pointer_visible(),
        ]
    }

    /// re/campaign-ui D6.10 (table "Cinemascope alone" / "Lock"), `FUN_0042EA20` (chat), `FUN_0042E3B0` (quest menu
    /// key), O-290/O-299/O-350 (additions of the game UI): every method for every switch alone and together.
    // QUIRK O-291, O-304, O-441: kept
    #[test]
    fn gate_matches_campaign_ui_d6_10() {
        let none = InputGate::default();
        let lock = InputGate {
            ui_locked: true,
            ..none
        };
        let cinema = InputGate {
            cinemascope: true,
            ..none
        };
        let dialog = InputGate {
            dialog_running: true,
            ..none
        };
        let camera = InputGate {
            script_camera: true,
            ..none
        };
        let campaign = InputGate {
            campaign: true,
            ..none
        };
        let menu = InputGate {
            menu_open: true,
            ..none
        };
        let campaign_cinema = InputGate {
            cinemascope: true,
            campaign: true,
            ..none
        };
        let scene = InputGate {
            ui_locked: true,
            cinemascope: true,
            dialog_running: true,
            script_camera: true,
            campaign: true,
            menu_open: false,
            menu_level: false,
        };
        let all = InputGate {
            menu_open: true,
            ..scene
        };
        // Menu level as `menü2.abl` runs it: cinemascope on, game type -2 (not campaign).
        let menu_level = InputGate {
            cinemascope: true,
            menu_level: true,
            ..none
        };
        let (t, f) = (true, false);
        #[rustfmt::skip]
        let table: [(&str, InputGate, [bool; 13]); 11] = [
            //                         Cam Wrld Rt  keys bind Chat Shot Qst Dipl MmIn MmVs MmSw Ptr
            ("free", none,             [t,  t,   t,  t,   t,   t,   t,   t,  t,   t,   t,   f,   t]),
            ("lock", lock,             [f,  f,   t,  f,   t,   t,   f,   f,  f,   f,   t,   t,   t]),
            ("cinemascope", cinema,    [t,  t,   t,  t,   t,   f,   t,   t,  t,   t,   f,   f,   t]),
            ("dialog", dialog,         [t,  t,   t,  t,   t,   t,   t,   f,  t,   t,   t,   f,   t]),
            ("script camera", camera,  [f,  t,   t,  t,   t,   t,   t,   t,  t,   t,   t,   f,   t]),
            ("campaign", campaign,     [t,  t,   t,  t,   t,   t,   t,   t,  t,   t,   t,   f,   t]),
            ("game menu", menu,        [t,  t,   t,  t,   t,   f,   t,   f,  t,   t,   t,   f,   t]),
            ("campaign in cinemascope", campaign_cinema,
                                       [t,  t,   t,  t,   t,   f,   t,   t,  t,   t,   f,   f,   f]),
            ("campaign scene", scene,
                                       [f,  f,   t,  f,   t,   f,   f,   f,  f,   f,   f,   t,   f]),
            ("all set", all,           [f,  f,   t,  f,   t,   f,   f,   f,  f,   f,   f,   t,   f]),
            ("menu level", menu_level, [f,  f,   f,  f,   f,   f,   t,   f,  f,   f,   f,   f,   t]),
        ];
        for (name, gate, expected) in table {
            assert_eq!(answers(&gate), expected, "{name}: {gate:?}");
        }
    }

    /// The names of the set fields in field order, regardless of how they were set.
    #[test]
    fn gate_names_follow_the_field_order() {
        assert!(InputGate::default().names().is_empty());
        let gate = InputGate {
            menu_open: true,
            ui_locked: true,
            campaign: true,
            ..InputGate::default()
        };
        assert_eq!(gate.names(), ["ui_locked", "campaign", "menu_open"]);
        let all = InputGate {
            ui_locked: true,
            cinemascope: true,
            dialog_running: true,
            script_camera: true,
            campaign: true,
            menu_open: true,
            menu_level: true,
        };
        assert_eq!(
            all.names(),
            [
                "ui_locked",
                "cinemascope",
                "dialog_running",
                "script_camera",
                "campaign",
                "menu_open",
                "menu_level"
            ]
        );
    }

    /// The simulation part: lock and cinemascope from the mission state; camera, game type and menu stay with the
    /// host.
    #[test]
    fn sim_gate_reads_the_mission() {
        let mut host = crate::game::sim_host::plain_testkit::host();
        assert_eq!(sim_gate(&host), InputGate::default());
        host.state.script.mission.ui_locked = true;
        host.state.script.mission.cinemascope = true;
        assert_eq!(
            sim_gate(&host),
            InputGate {
                ui_locked: true,
                cinemascope: true,
                ..InputGate::default()
            }
        );
    }

    /// O-441: in the menu level the player camera is off, even without a lock and without the script camera; the
    /// script camera (`n241`) runs unaffected in the simulation.
    // QUIRK O-441: kept
    #[test]
    fn player_camera_is_off_in_the_menu_level() {
        let free = InputGate::default();
        assert!(free.player_camera());
        let menu_level = InputGate {
            menu_level: true,
            ..free
        };
        assert!(!menu_level.player_camera());
        assert!(
            !menu_level.minimap_input(),
            "no camera jump over the minimap"
        );
        assert!(menu_level.hardcopy(), "the screenshot works");
        assert!(menu_level.pointer_visible(), "pointer stays (O-350)");
    }
}
