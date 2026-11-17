//! Hotkeys of the match (re/game-ui D5.3): the bindings from `bindings2.ini`, the lines of `keys.ini` and the `KD_`
//! keys from `keymap.ini`, plus the command form of the console commands that a key triggers ([`HotkeyCommand`]).
//!
//! The original executes the commands via its console (binding `FUN_00411CE0` -> `0x0040FC50`, `keys.ini`
//! `FUN_0049D110` -> `FUN_00411200`). The reimplementation has no console: [`parse_command`] reads exactly the
//! commands that the three files use, the match executes them (`SessionView::run_hotkey`). The keyboard context
//! for `keys.ini` ([`KeyContext`]) is held by the layouts of the command bar
//! (`nml_ui::hud::toolbar::Toolbar::key_context`).

use nml_formats::bindings_ini::{BINDINGS_INI_PATH, parse_bindings};
use nml_formats::keymap_ini::{KEYMAP_INI_PATH, parse_keymap_ini};
use nml_formats::keys_ini::{KEYS_INI_PATH, KeysIniLine, parse_keys_ini};
use nml_formats::property::atol;
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;
use nml_ui::keys::{binding_matches, chord_name, keydef_down, parse_chord};

use crate::game::input_gate::InputGate;

pub use nml_ui::keys::KeyContext;

/// A console command of a key, with the directories and cases of the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyCommand {
    /// `/game/toolbar/button(n)` (index 0, `0x0044B090`): button `n` of the bar, only if present and not locked.
    Button(i32),
    /// `/game/toolbar/xbutton(n)` (index 1, `0x0044B1B0`): directly `{0x186AA, n, DO}`, without button and lock check.
    XButton(i32),
    /// `/game/hm/selectnextguid(g)` (`0x0048B4A0`): select the next object of the GUID.
    SelectNextGuid(i32),
    /// `/game/storegroup(n)` (`0xC`).
    StoreGroup(u8),
    /// `/game/selectgroup(n)` (`0xD`).
    SelectGroup(u8),
    /// `/game/addtogroup(n)` or `/game/addgroup(n)` (`0x51`/`0x55`).
    AddToGroup(u8),
    /// `/game/togglepause` (`0xF`, `0x0042D780`).
    TogglePause,
    /// `/game/changegamespeed(d)` (`0x52`).
    ChangeGameSpeed(i32),
    /// `/game/quicksave()` (`0x58`, `FUN_0042F490`).
    QuickSave,
    /// `/game/quickload()` (`0x5D`, `FUN_0042F5C0`).
    QuickLoad,
    /// `/setcamera` (root, `FUN_00420230` -> `0x00433FC0`): camera smoothly to the home position.
    SetCamera,
    /// `/game/showchatwindow(k)` (`0x49`, `FUN_0042EA20`): chat window k = 1..3 (ENTER, Shift+ENTER, Ctrl+ENTER); the
    /// flow opens it (`GameFlow::run_command`).
    ShowChatWindow(u8),
    /// `/game/ShowQuestMenu(a)` (`0x45`, `0x0042E3B0`).
    ShowQuestMenu(i32),
}

/// Command text -> [`HotkeyCommand`]. The console lowercases path and names before comparing
/// (`/game/toolbar/Xbutton(530)` works, O-309); `addgroup` is a second name of `addtogroup`. `_atol` reads the
/// argument between the parentheses (leading spaces, sign, digits, otherwise 0); commands without an argument
/// apply with and without `()`. Unknown commands and groups outside 0..255 -> `None`.
pub fn parse_command(text: &str) -> Option<HotkeyCommand> {
    use HotkeyCommand as C;
    let lower = text.to_ascii_lowercase();
    let (name, arg) = match lower.split_once('(') {
        Some((name, rest)) => (name, rest.split(')').next().unwrap_or("")),
        None => (lower.as_str(), ""),
    };
    let number = || i32::try_from(atol(arg)).ok();
    let small = || u8::try_from(atol(arg)).ok();
    Some(match name {
        "/game/toolbar/button" => C::Button(number()?),
        "/game/toolbar/xbutton" => C::XButton(number()?),
        "/game/hm/selectnextguid" => C::SelectNextGuid(number()?),
        "/game/storegroup" => C::StoreGroup(small()?),
        "/game/selectgroup" => C::SelectGroup(small()?),
        "/game/addtogroup" | "/game/addgroup" => C::AddToGroup(small()?),
        "/game/togglepause" => C::TogglePause,
        "/game/changegamespeed" => C::ChangeGameSpeed(number()?),
        "/game/quicksave" => C::QuickSave,
        "/game/quickload" => C::QuickLoad,
        "/setcamera" => C::SetCamera,
        "/game/showchatwindow" => C::ShowChatWindow(small()?),
        "/game/showquestmenu" => C::ShowQuestMenu(number()?),
        _ => return None,
    })
}

/// The key data of an installation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hotkeys {
    /// `bindings2.ini` in file order: chord ([`parse_chord`]) and command; bindings with an unknown command
    /// are missing (warning when loading).
    pub bindings: Vec<(Vec<u8>, HotkeyCommand)>,
    /// `keys.ini` in file order (evaluation with context: [`Hotkeys::keys_ini`]).
    pub keys: Vec<KeysIniLine>,
    /// `keymap.ini` in file order: name (`KD_...`) and chord ([`parse_chord`]).
    pub keydefs: Vec<(String, Vec<u8>)>,
}

impl Hotkeys {
    /// Reads `bindings2.ini`, `keys.ini` and `keymap.ini` (cp1252) from `data/gifcon`. A missing or unreadable
    /// file yields its part empty, with a warning (the keys then have no effect).
    pub fn load(vfs: &Vfs) -> Hotkeys {
        let read = |path: &str| match vfs.read(path) {
            Ok(bytes) => Some(decode_cp1252(&bytes)),
            Err(e) => {
                log::warn!("Key file \"{path}\" not readable, its keys have no effect: {e}");
                None
            }
        };
        let bindings = read(BINDINGS_INI_PATH)
            .map(|text| {
                parse_bindings(&text)
                    .into_iter()
                    .filter_map(|b| match parse_command(&b.command) {
                        Some(cmd) => Some((parse_chord(&b.chord), cmd)),
                        None => {
                            log::warn!(
                                "Binding \"{}\": command \"{}\" unknown, skipped",
                                b.chord,
                                b.command
                            );
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let keys = read(KEYS_INI_PATH)
            .map(|text| parse_keys_ini(&text))
            .unwrap_or_default();
        let keydefs = read(KEYMAP_INI_PATH)
            .map(|text| {
                parse_keymap_ini(&text)
                    .into_iter()
                    .map(|(name, chord)| {
                        let chord = parse_chord(&chord);
                        (name, chord)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Hotkeys {
            bindings,
            keys,
            keydefs,
        }
    }

    /// The first binding that the event `held` (codes of the held keys) matches
    /// ([`binding_matches`], `0x0040FC50`).
    pub fn binding(&self, held: &[u8]) -> Option<&HotkeyCommand> {
        self.bindings
            .iter()
            .find(|(chord, _)| binding_matches(held, chord))
            .map(|(_, cmd)| cmd)
    }

    /// Stage 4 of the key path: `keys.ini` in context `ctx` (`FUN_0049D110`, re/game-ui D5.3, O-302). The active list
    /// is built in three stages type (`0x0049D950`) -> nation (`0x0049D750`) -> selection (`0x0049CF60`); each takes
    /// from the previous one, in its order, **first** the lines with exactly its value (only for value != 0),
    /// **then** those with 0 - 0 is not a wildcard. The match is the first line of the active list whose assignment
    /// (`key_map[action]`, `Keyboard+0x20`) is exactly `name` as text (`_mbscmp`), even if its button is missing;
    /// its command, if [`parse_command`] knows it (otherwise `None`, the console of the original executed nothing).
    pub fn keys_ini(
        &self,
        name: &str,
        ctx: &KeyContext,
        key_map: &[(i32, String)],
    ) -> Option<HotkeyCommand> {
        fn stage(
            lines: Vec<&KeysIniLine>,
            value: i32,
            field: fn(&KeysIniLine) -> i32,
        ) -> Vec<&KeysIniLine> {
            let exact = lines.iter().filter(|l| value != 0 && field(l) == value);
            let zero = lines.iter().filter(|l| field(l) == 0);
            exact.chain(zero).copied().collect()
        }
        let lines = stage(self.keys.iter().collect(), ctx.kind, |l| l.kind);
        let lines = stage(lines, ctx.nation, |l| l.nation);
        let lines = stage(lines, ctx.selection, |l| l.selection);
        let hit = lines.into_iter().find(|l| {
            key_map
                .iter()
                .find(|(action, _)| *action == l.action)
                .is_some_and(|(_, key)| key == name)
        })?;
        parse_command(&hit.command)
    }

    /// Stages 2 to 4 of the key path (re/game-ui D5.3) for the event `held` (codes of the held keys) with its chord
    /// name ([`chord_name`]); for the viewer with `--hud`. See [`Self::resolve_named`].
    pub fn resolve(
        &self,
        held: &[u8],
        gate: &InputGate,
        ctx: &KeyContext,
        key_map: &[(i32, String)],
    ) -> Option<HotkeyCommand> {
        self.resolve_named(&chord_name(held), held, gate, ctx, key_map)
    }

    /// Stages 2 to 4 of the key path (re/game-ui D5.3), the one place for `GameFlow::key` (`nml-game`) and the viewer:
    /// first the binding ([`Self::binding`] with `held`, always, [`InputGate::bindings`]), under the lock
    /// ([`InputGate::keys_ini`] false) then end (O-304), otherwise the line from `keys.ini` ([`Self::keys_ini`]
    /// with the chord name `name` of the event). The binding takes precedence: if it matches, `keys.ini` is not
    /// asked anymore.
    /// ESC, quicksave/quickload and the screenshot are handled by the caller (`GameFlow::key`).
    pub fn resolve_named(
        &self,
        name: &str,
        held: &[u8],
        gate: &InputGate,
        ctx: &KeyContext,
        key_map: &[(i32, String)],
    ) -> Option<HotkeyCommand> {
        if gate.bindings()
            && let Some(cmd) = self.binding(held)
        {
            return Some(*cmd);
        }
        if !gate.keys_ini() {
            return None;
        }
        self.keys_ini(name, ctx, key_map)
    }

    /// Chord of the `KD_` key `name` (table `Input+0xB8`, `CMapStringToPtr`: case counts, a later equal name replaces
    /// the earlier one); unknown -> empty.
    pub fn keydef(&self, name: &str) -> &[u8] {
        self.keydefs
            .iter()
            .rev()
            .find(|(known, _)| known == name)
            .map_or(&[], |(_, chord)| chord.as_slice())
    }

    /// Is the `KD_` key `name` pressed in `held` (`FUN_004143A0`, [`keydef_down`])? Unknown name -> false.
    pub fn keydef_down(&self, name: &str, held: &[u8]) -> bool {
        keydef_down(held, self.keydef(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HotkeyCommand as C;

    /// `resolve` puts the binding before `keys.ini`; under the lock only the bindings take effect (O-304).
    #[test]
    fn resolve_takes_the_binding_first_and_keys_ini_only_without_lock() {
        let hotkeys = Hotkeys {
            bindings: vec![
                (parse_chord("CONTROL 1"), C::StoreGroup(1)),
                (parse_chord("PAUSE"), C::TogglePause),
            ],
            keys: parse_keys_ini(
                "KEY 113 0 0 0 /game/changegamespeed(+1)\nKEY 200 0 0 0 /game/togglepause\n",
            ),
            keydefs: Vec::new(),
        };
        let key_map = [(113, "+".to_string()), (200, "PAUSE".to_string())];
        let ctx = KeyContext::default();
        let free = InputGate::default();
        let locked = InputGate {
            ui_locked: true,
            ..free
        };
        let resolve = |held: &[u8], gate| hotkeys.resolve(held, &gate, &ctx, &key_map);
        assert_eq!(resolve(&[0x11, 0x31], free), Some(C::StoreGroup(1)));
        assert_eq!(resolve(&[0x11, 0x31], locked), Some(C::StoreGroup(1)));
        // The binding takes precedence over the `keys.ini` line with the same key.
        assert_eq!(resolve(&[0x13], free), Some(C::TogglePause));
        assert_eq!(resolve(&[0xBB], free), Some(C::ChangeGameSpeed(1)));
        assert_eq!(resolve(&[0xBB], locked), None);
        assert_eq!(resolve(&[0x31], free), None);
    }

    // QUIRK O-309: kept
    #[test]
    fn parse_command_lowercases_and_reads_the_argument() {
        assert_eq!(
            parse_command("/game/toolbar/button(408)"),
            Some(C::Button(408))
        );
        // O-309: compared in lowercase.
        assert_eq!(
            parse_command("/game/toolbar/Xbutton(530)"),
            Some(C::XButton(530))
        );
        assert_eq!(parse_command("/GAME/StoreGroup(3)"), Some(C::StoreGroup(3)));
        assert_eq!(parse_command("/game/addgroup(2)"), Some(C::AddToGroup(2)));
        assert_eq!(parse_command("/game/addtogroup(2)"), Some(C::AddToGroup(2)));
        assert_eq!(
            parse_command("/game/changegamespeed(+1)"),
            Some(C::ChangeGameSpeed(1))
        );
        assert_eq!(
            parse_command("/game/changegamespeed(-1)"),
            Some(C::ChangeGameSpeed(-1))
        );
        assert_eq!(parse_command("/game/quicksave()"), Some(C::QuickSave));
        assert_eq!(parse_command("/game/quickload"), Some(C::QuickLoad));
        assert_eq!(parse_command("/setcamera"), Some(C::SetCamera));
        assert_eq!(parse_command("/game/togglepause"), Some(C::TogglePause));
        assert_eq!(
            parse_command("/game/showchatwindow(3)"),
            Some(C::ShowChatWindow(3))
        );
        assert_eq!(
            parse_command("/game/ShowQuestMenu(1)"),
            Some(C::ShowQuestMenu(1))
        );
        assert_eq!(
            parse_command("/game/hm/selectnextguid(1200)"),
            Some(C::SelectNextGuid(1200))
        );
        // `_atol`: 0 without digits.
        assert_eq!(
            parse_command("/game/selectgroup(x)"),
            Some(C::SelectGroup(0))
        );
        // Unknown commands and groups outside a byte.
        assert_eq!(parse_command("/game/unknown(1)"), None);
        assert_eq!(parse_command("game/togglepause"), None);
        assert_eq!(parse_command("/game/storegroup(-1)"), None);
        assert_eq!(parse_command(""), None);
    }

    #[test]
    fn binding_and_keydef_lookups() {
        let hotkeys = Hotkeys {
            bindings: vec![
                (parse_chord("1"), C::SelectGroup(1)),
                (parse_chord("CONTROL 1"), C::StoreGroup(1)),
                (parse_chord("1"), C::SelectGroup(9)),
            ],
            keys: Vec::new(),
            keydefs: vec![
                ("KD_ADD_TO_GROUP".into(), parse_chord("LSHIFT")),
                ("KD_HARDCOPY".into(), parse_chord("F12")),
                ("KD_HARDCOPY".into(), parse_chord("SNAPSHOT")),
            ],
        };
        // First binding wins; exactly the set of keys.
        assert_eq!(hotkeys.binding(&[0x31]), Some(&C::SelectGroup(1)));
        assert_eq!(hotkeys.binding(&[0x11, 0x31]), Some(&C::StoreGroup(1)));
        assert_eq!(hotkeys.binding(&[0x10, 0x31]), None);
        assert_eq!(hotkeys.binding(&[0x01, 0x31]), None);
        // `KD_`: other keys do not interfere, both Shift keys; a later name replaces; unknown -> false.
        assert!(hotkeys.keydef_down("KD_ADD_TO_GROUP", &[0x10, 0x41]));
        assert!(hotkeys.keydef_down("KD_HARDCOPY", &[0x01, 0x2C]));
        assert!(!hotkeys.keydef_down("KD_HARDCOPY", &[0x7B]));
        assert!(!hotkeys.keydef_down("KD_CHECK_QUESTMENU", &[0x10]));
        assert!(!hotkeys.keydef_down("kd_add_to_group", &[0x10]));
        assert!(hotkeys.keydef("KD_CAM_XROT_UP").is_empty());
    }

    /// Every line of `keys.ini` and every binding of `bindings2.ini` of the corpus yields a command; `keymap.ini` has
    /// neither `KD_CHECK_QUESTMENU` nor `KD_CAM_XROT_*` (O-305).
    // QUIRK O-305: kept
    #[test]
    fn parse_command_knows_every_command_of_the_corpus() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let text = |path: &str| decode_cp1252(&vfs.read(path).unwrap());
        let keys = parse_keys_ini(&text(KEYS_INI_PATH));
        assert_eq!(keys.len(), 652);
        for line in &keys {
            assert!(
                parse_command(&line.command).is_some(),
                "keys.ini {}: \"{}\"",
                line.action,
                line.command
            );
        }
        let bindings = parse_bindings(&text(BINDINGS_INI_PATH));
        assert_eq!(bindings.len(), 36);
        for b in &bindings {
            assert!(
                parse_command(&b.command).is_some(),
                "{}: \"{}\"",
                b.chord,
                b.command
            );
        }
        let hotkeys = Hotkeys::load(&vfs);
        assert_eq!(hotkeys.bindings.len(), 36);
        assert_eq!(hotkeys.keys.len(), 652);
        assert_eq!(
            hotkeys.binding(&parse_chord("CONTROL P")),
            Some(&C::TogglePause)
        );
        assert_eq!(hotkeys.binding(&[0x13]), Some(&C::TogglePause));
        assert_eq!(hotkeys.binding(&[0x24]), Some(&C::SetCamera));
        assert_eq!(hotkeys.binding(&[0x10, 0x0D]), Some(&C::ShowChatWindow(2)));
        assert_eq!(hotkeys.binding(&[0x10, 0x35]), Some(&C::AddToGroup(5)));
        assert_eq!(hotkeys.keydef("KD_HARDCOPY"), [0x2C]);
        assert_eq!(hotkeys.keydef("KD_ADD_TO_GROUP"), [0xA0]);
        for dead in ["KD_CHECK_QUESTMENU", "KD_CAM_XROT_UP", "KD_CAM_XROT_DOWN"] {
            assert!(hotkeys.keydef(dead).is_empty(), "{dead}");
        }
        // TAB and ESC are in the file, the code never queries them (O-305).
        assert_eq!(hotkeys.keydef("KD_SPEEDUP_GAME"), [0x09]);
        assert_eq!(hotkeys.keydef("KD_STOP_DIALOG"), [0x1B]);
    }
    /// Three filter stages type -> nation -> selection, per stage first the exact value, then 0; the first match
    /// counts, even if its button is missing. Assignment = `defaultkeys.ini`. (O-302)
    // QUIRK O-302, O-307: kept
    #[test]
    fn keys_ini_filters_selection_then_nation_then_type() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let hotkeys = Hotkeys::load(&vfs);
        let key_map = nml_ui::screens::load_default_keys(&vfs);
        let ctx = |kind, nation, selection| KeyContext {
            kind,
            nation,
            selection,
        };
        // A GUID without lines of its own: in the selection stage only the 0 lines apply.
        const OTHER: i32 = 99_999;
        // Unit selected: `Q` is 414, never 404 Board (414 comes first) - in every nation and in every selection
        // context of the shipped data.
        let mut selections: Vec<i32> = hotkeys.keys.iter().map(|l| l.selection).collect();
        selections.extend([0, OTHER]);
        selections.sort_unstable();
        selections.dedup();
        for nation in 0..=6 {
            assert_eq!(
                hotkeys.keys_ini("Q", &ctx(2, nation, OTHER), &key_map),
                Some(C::Button(414)),
                "nation {nation}"
            );
            for &s in &selections {
                assert_ne!(
                    hotkeys.keys_ini("Q", &ctx(2, nation, s), &key_map),
                    Some(C::Button(404)),
                    "nation {nation}, selection {s}"
                );
            }
        }
        // `SHIFT D`: with a unit 423, without a selection the diplomacy 572.
        assert_eq!(
            hotkeys.keys_ini("SHIFT D", &ctx(2, 1, OTHER), &key_map),
            Some(C::XButton(423))
        );
        assert_eq!(
            hotkeys.keys_ini("SHIFT D", &ctx(1, 1, 0), &key_map),
            Some(C::XButton(572))
        );
        // Building selected: `H` selects the HQ of the nation (nation stage before the type stage), not 490.
        for (nation, hq) in [
            (1, 1300),
            (2, 1600),
            (3, 1000),
            (4, 2500),
            (5, 1900),
            (6, 2200),
        ] {
            assert_eq!(
                hotkeys.keys_ini("H", &ctx(3, nation, OTHER), &key_map),
                Some(C::SelectNextGuid(hq)),
                "nation {nation}"
            );
        }
        // Without a nation (0 is not a wildcard): only the 0 lines, so 490.
        assert_eq!(
            hotkeys.keys_ini("H", &ctx(3, 0, OTHER), &key_map),
            Some(C::Button(490))
        );
        // Selection stage: 407 has a line with selection 407 (`button(408)`); in submenu 407 it matches.
        let key_407 = key_map.iter().find(|(id, _)| *id == 407).unwrap().1.clone();
        assert_eq!(
            hotkeys.keys_ini(&key_407, &ctx(2, 1, 407), &key_map),
            Some(C::Button(408))
        );
        // Quicksave/quickload and game speed in the type-1 context without a nation (default of the fake).
        assert_eq!(
            hotkeys.keys_ini("F1", &KeyContext::default(), &key_map),
            Some(C::QuickSave)
        );
        assert_eq!(
            hotkeys.keys_ini("F4", &KeyContext::default(), &key_map),
            Some(C::QuickLoad)
        );
        assert_eq!(
            hotkeys.keys_ini("+", &KeyContext::default(), &key_map),
            Some(C::ChangeGameSpeed(1))
        );
        // Exact name: other keys (mouse button) or the numeric keypad match nothing (O-307, O-301).
        for name in ["LBUTTON F1", "ADD", "SUBTRACT", "f1", ""] {
            assert_eq!(
                hotkeys.keys_ini(name, &KeyContext::default(), &key_map),
                None,
                "{name:?}"
            );
        }
    }
}
