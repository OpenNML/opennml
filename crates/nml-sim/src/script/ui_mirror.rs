//! Selection mirror and last button (deviation from the original, E-136, E-228 ... E-231, O-246).
//!
//! The conditions 35-38 read the UI of the machine in the original: the selection list
//! `GameWorld+0x370` (35-37, queries 1600/1700/1800) and `GameEngine+0x4E0` (38, query 1900). The
//! simulation knows neither; the game host reports them through the host commands `ScriptSelection` and
//! `ScriptButton`, and this module keeps the mirror in [`crate::script::MissionState`] and evaluates it.

use crate::command::{CommandKind, Origin, reject};
use crate::id::ObjectId;
use crate::names;
use crate::object::status;
use crate::sim::Sim;

/// Maximum number of entries of the original's selection list (`MAX_SELECTION` of the game host).
pub const MAX_SELECTION: usize = 30;

/// Effect of the host commands `ScriptSelection` and `ScriptButton` (type `0xF6`/`0xF7`; called from
/// [`crate::command::execute`] at the tick boundary before the world tick). Only the origin host
/// ([`Origin::External`]) may send them, not the AI and not the scripts; the ownership check does not
/// apply (the original's selection list also takes foreign objects). The command rejects more than
/// [`MAX_SELECTION`] entries and the invalid ID [`ObjectId::NONE`] and leaves the mirror unchanged; dead
/// IDs are allowed. A replacement is a set, not a change list: the game host only sends it when it
/// changes (`SimHost::sync_selection`).
pub(crate) fn execute(sim: &mut Sim, player: u8, kind: &CommandKind) {
    let what = match kind {
        CommandKind::ScriptSelection { .. } => "Script selection",
        _ => "Script button",
    };
    if sim.origin != Origin::External {
        reject(sim, player, what, "only allowed from the host");
        return;
    }
    match kind {
        CommandKind::ScriptSelection { ids } => {
            if ids.len() > MAX_SELECTION {
                reject(
                    sim,
                    player,
                    what,
                    format!(
                        "selection with {} entries (at most {MAX_SELECTION})",
                        ids.len()
                    ),
                );
                return;
            }
            // Dead IDs are allowed, the invalid `ObjectId::NONE` is not.
            if let Some(i) = ids.iter().position(|id| id.is_none()) {
                reject(
                    sim,
                    player,
                    what,
                    format!("selection with invalid ID at index {i}"),
                );
                return;
            }
            sim.state.script.mission.selection.clone_from(ids);
        }
        CommandKind::ScriptButton { word } => sim.state.script.mission.last_button = *word,
        _ => {}
    }
}

/// Does the mirror entry `id` count as selected? It must be in the mirror, its object must exist
/// (`GetObjectById(id, 0)`: also dying and hidden ones, until final destruction) and it must not be
/// quartered ([`status::QUARTERED`]). This models the removal from the original's selection list
/// in the world tick (destruction, change of owner, quartering, sale; E-230): the mirror itself only
/// changes through the host.
pub fn is_selected(sim: &Sim, id: ObjectId) -> bool {
    sim.state.script.mission.selection.contains(&id) && counts(sim, id).is_some()
}

/// The object of a mirror entry if it counts as selected (present and not quartered).
fn counts<'a>(sim: &'a Sim, id: ObjectId) -> Option<&'a crate::object::GameObject> {
    sim.state
        .object(id, false)
        .filter(|o| o.status & status::QUARTERED == 0)
}

/// Does a counted mirror entry have the type GUID `guid` (`GameObject::guid`, `+0x54`)? A pure
/// equality comparison without filter ID, owner, dead or sight filter (E-231).
pub fn any_selected_guid(sim: &Sim, guid: i32) -> bool {
    sim.state
        .script
        .mission
        .selection
        .iter()
        .filter_map(|&id| counts(sim, id))
        .any(|o| o.guid as i32 == guid)
}

/// Queries 1600 (`wanted = true`, condition 35) and 1700 (`wanted = false`, condition 36;
/// `FUN_00425F90`): for each ID of the name with an existing object (`GetObjectById(id, 0)`),
/// "selected" must equal `wanted`, otherwise false; the loop does not stop. An unknown name and
/// lists without an existing object are true (E-135), so with an empty selection 35 is false as soon
/// as an object of the name exists, and 36 is true.
pub(super) fn query_selected(sim: &Sim, name: &str, wanted: bool) -> bool {
    names::ids(&sim.state.names, name)
        .into_iter()
        .filter(|&id| sim.state.object(id, false).is_some())
        .all(|id| is_selected(sim, id) == wanted)
}

/// Query 1900 (condition 38): the last button (`GameEngine+0x4E0`, E-229); only reads, consumes
/// nothing.
pub(super) fn query_last_button(sim: &Sim) -> i32 {
    sim.state.script.mission.last_button
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, CommandKind, Origin, Outcome};
    use crate::object::status;
    use crate::player;
    use crate::script::conditions::condition;
    use crate::script::container::{Container, ScriptSource};
    use crate::script::tests::{cond, rules_for, script, setup, spawn};
    use crate::script::{LEVEL_CONTAINER_ID, ScriptTrace};
    use crate::setup::new_match;
    use crate::state::{SimEvent, SimState};
    use crate::tick::step;
    use crate::world::{change_owner, destroy_object};
    use nml_formats::abl::ScriptFile;

    fn selection(ids: &[ObjectId]) -> Command {
        Command {
            player: 0,
            kind: CommandKind::ScriptSelection { ids: ids.to_vec() },
        }
    }

    fn button(word: i32) -> Command {
        Command {
            player: 0,
            kind: CommandKind::ScriptButton { word },
        }
    }

    /// Condition `number` via the script path (with an empty helper container).
    fn check(sim: &mut Sim, number: i32, params: &[&str]) -> bool {
        let file = ScriptFile { blocks: Vec::new() };
        let source = ScriptSource {
            path: String::new(),
            checksum: 0,
        };
        let mut c = Container::new(0, source, &file);
        let result = condition(sim, &mut c, 0, &cond(number, params));
        let guarded = sim
            .state
            .events
            .iter()
            .any(|e| matches!(e, SimEvent::Script(ScriptTrace::Guarded { .. })));
        assert!(!guarded, "{:?}", sim.state.events);
        let pending = sim
            .state
            .events
            .iter()
            .any(|e| matches!(e, SimEvent::Script(ScriptTrace::ConditionPending { .. })));
        assert!(!pending, "condition {number} reports ConditionPending");
        result
    }

    fn started() -> (SimState, crate::rules::Rules) {
        let rules = rules_for(&script(&[(&[], &["n12;a"])]));
        let state = new_match(&rules, &setup(true)).expect("match with script");
        (state, rules)
    }

    fn list(ids: &[ObjectId]) -> String {
        ids.iter()
            .map(|id| id.as_i32().to_string())
            .collect::<Vec<_>>()
            .join(";")
    }

    #[test]
    fn synthetic_rules_carry_the_ui_flag_for_35_to_38() {
        // `ScriptRules::load` and `testing::rules_with` set the flag through the same helper.
        assert!(
            !rules_for(&script(&[(&["&5;x"], &["n12;a"])]))
                .scripts
                .uses_ui_conditions
        );
        for number in 35..=38 {
            let condition = if number < 37 {
                format!("&{number};x")
            } else {
                format!("&{number};0;1")
            };
            let level = script(&[(&[condition.as_str()], &["n12;a"])]);
            assert!(rules_for(&level).scripts.uses_ui_conditions, "{number}");
        }
        // A dialog script counts as well.
        let dialog = script(&[(&["&38;0;1"], &["n12;a"])]);
        let rules = crate::script::testing::rules_with(
            &script(&[(&[], &["n39;d"])]),
            &[("d", &dialog)],
            &[],
        );
        assert!(rules.errors.is_empty(), "{:?}", rules.errors);
        assert!(rules.uses_ui_conditions);
    }

    #[test]
    fn selection_command_sets_the_mirror_and_rejects_31_ids() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, Some("a"), (3, 3));
        let b = spawn(&mut sim, None, (4, 3));
        assert_eq!(
            player::submit(&mut sim, &selection(&[a, b]), Origin::External),
            Outcome::Accepted
        );
        assert_eq!(sim.state.script.mission.selection, [a, b]);
        // Replacing, also with the empty list; order as reported.
        assert_eq!(
            player::submit(&mut sim, &selection(&[b, a]), Origin::External),
            Outcome::Accepted
        );
        assert_eq!(sim.state.script.mission.selection, [b, a]);
        // No ownership check: slot 1 reports an object of slot 0 (foreign objects can be selected too).
        let from_other = Command {
            player: 1,
            kind: CommandKind::ScriptSelection { ids: vec![a] },
        };
        assert_eq!(
            player::submit(&mut sim, &from_other, Origin::External),
            Outcome::Accepted
        );
        assert_eq!(sim.state.script.mission.selection, [a]);
        assert_eq!(
            player::submit(&mut sim, &selection(&[]), Origin::External),
            Outcome::Accepted
        );
        assert!(sim.state.script.mission.selection.is_empty());
        // 30 entries pass (dead IDs are allowed), the command rejects 31 and leaves the mirror unchanged.
        let thirty: Vec<ObjectId> = (0..30).map(|i| ObjectId(0x800 + i)).collect();
        assert_eq!(
            player::submit(&mut sim, &selection(&thirty), Origin::External),
            Outcome::Accepted
        );
        assert_eq!(sim.state.script.mission.selection, thirty);
        let thirty_one: Vec<ObjectId> = (0..31).map(|i| ObjectId(0x800 + i)).collect();
        let outcome = player::submit(&mut sim, &selection(&thirty_one), Origin::External);
        match outcome {
            Outcome::Rejected(why) => {
                assert!(
                    why.contains("selection with 31 entries (at most 30)"),
                    "{why}"
                );
            }
            other => panic!("expected rejected, got {other:?}"),
        }
        assert_eq!(sim.state.script.mission.selection, thirty);
        // The command rejects the invalid ID `ObjectId::NONE`; the mirror stays unchanged, the savegame
        // loadable.
        let outcome = player::submit(&mut sim, &selection(&[a, ObjectId::NONE]), Origin::External);
        match outcome {
            Outcome::Rejected(why) => {
                assert!(
                    why.contains("selection with invalid ID at index 1"),
                    "{why}"
                );
            }
            other => panic!("expected rejected, got {other:?}"),
        }
        assert_eq!(sim.state.script.mission.selection, thirty);
        crate::save::check_state(sim.state).expect("savegame loads");
    }

    #[test]
    fn script_and_ki_origins_cannot_send_ui_commands() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, Some("a"), (3, 3));
        sim.state.script.mission.last_button = 5;
        for command in [selection(&[a]), button(77)] {
            for outcome in [
                player::submit(&mut sim, &command, Origin::Ki),
                player::submit(&mut sim, &command, Origin::Script),
                player::submit_script(&mut sim, &command),
            ] {
                assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
            }
        }
        assert!(sim.state.script.mission.selection.is_empty());
        assert_eq!(sim.state.script.mission.last_button, 5);
        // The origin host takes effect.
        assert_eq!(
            player::submit(&mut sim, &button(77), Origin::External),
            Outcome::Accepted
        );
        assert_eq!(sim.state.script.mission.last_button, 77);
    }

    #[test]
    fn button_command_sets_last_button_and_38_does_not_consume_it() {
        // Start 0 (E-229, deviation from the original), then the value of the command; 38 consumes nothing.
        let rules = rules_for(&script(&[(&["&38;0;2904"], &["n10", "n361;gedrueckt;1"])]));
        let mut state = new_match(&rules, &setup(true)).unwrap();
        assert_eq!(state.script.mission.last_button, 0);
        step(&mut state, &rules, &[]);
        assert!(
            !state
                .script
                .level
                .as_ref()
                .unwrap()
                .variables
                .contains_key("gedrueckt")
        );
        // The host command takes effect at the tick boundary before the script tick: true in the same step already.
        step(&mut state, &rules, &[button(2904)]);
        assert_eq!(state.script.mission.last_button, 2904);
        for _ in 0..2 {
            step(&mut state, &rules, &[]);
        }
        let hits = state.script.level.as_ref().unwrap().variables["gedrueckt"];
        assert_eq!(hits, 3, "the condition stays true (is not consumed)");
        assert_eq!(state.script.mission.last_button, 2904);
        // A different button replaces it.
        step(&mut state, &rules, &[button(140)]);
        step(&mut state, &rules, &[]);
        assert_eq!(
            state.script.level.as_ref().unwrap().variables["gedrueckt"],
            3
        );
        assert_eq!(state.script.mission.last_button, 140);
        assert_eq!(LEVEL_CONTAINER_ID, 1);
    }

    #[test]
    fn conditions_35_36_unknown_name_is_true_for_both() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, Some("a"), (3, 3));
        // Unknown name: empty list, both true (E-135) - also with an empty and with a filled selection.
        for selected in [vec![], vec![a]] {
            sim.state.script.mission.selection = selected;
            assert!(check(&mut sim, 35, &["unbekannt"]));
            assert!(check(&mut sim, 36, &["unbekannt"]));
        }
        // A name whose objects are all missing (destroyed, ID 0) counts like an unknown one.
        let gone = spawn(&mut sim, Some("weg"), (5, 5));
        destroy_object(&mut sim, gone);
        sim.state.names.set("nurweg", list(&[gone]));
        sim.state.names.set("null", "0".to_owned());
        for name in ["weg", "nurweg", "null"] {
            assert!(check(&mut sim, 35, &[name]), "35 {name}");
            assert!(check(&mut sim, 36, &[name]), "36 {name}");
        }
    }

    #[test]
    fn condition_35_needs_every_present_id() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, Some("a"), (3, 3));
        let b = spawn(&mut sim, Some("b"), (4, 3));
        let gone = spawn(&mut sim, None, (5, 5));
        destroy_object(&mut sim, gone);
        sim.state.names.set("gruppe", list(&[a, b]));
        sim.state.names.set("mitweg", list(&[a, gone]));
        // Empty selection: 35 false as soon as an object of the name exists; 36 true.
        assert!(!check(&mut sim, 35, &["a"]));
        assert!(check(&mut sim, 36, &["a"]));
        assert!(!check(&mut sim, 35, &["gruppe"]));
        assert!(check(&mut sim, 36, &["gruppe"]));
        // Only a selected: the group is not completely selected; 36 fails as soon as one is selected.
        sim.state.script.mission.selection = vec![a];
        assert!(check(&mut sim, 35, &["a"]));
        assert!(!check(&mut sim, 36, &["a"]));
        assert!(!check(&mut sim, 35, &["gruppe"]));
        assert!(!check(&mut sim, 36, &["gruppe"]));
        assert!(!check(&mut sim, 35, &["b"]));
        assert!(check(&mut sim, 36, &["b"]));
        // Missing objects of the list are skipped.
        assert!(check(&mut sim, 35, &["mitweg"]));
        // Both selected: the group is selected, 36 false.
        sim.state.script.mission.selection = vec![b, a];
        assert!(check(&mut sim, 35, &["gruppe"]));
        assert!(!check(&mut sim, 36, &["gruppe"]));
        // The name is found regardless of case, as with the other queries.
        assert!(check(&mut sim, 35, &["GRUPPE"]));
    }

    #[test]
    fn condition_37_compares_the_type_guid_only() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, Some("a"), (3, 3));
        let b = spawn(&mut sim, Some("b"), (4, 3));
        sim.object_mut(a).unwrap().guid = 1007;
        // Foreign object (slot 3) of a different kind.
        let foreign = change_owner(&mut sim, b, 3).expect("change of owner");
        sim.state.script.mission.selection = vec![a, foreign];
        // Equality with the type GUID of any entry; the player (parameter 1) does not count (E-136).
        for player in ["0", "6", "-1", "x"] {
            assert!(check(&mut sim, 37, &[player, "1007"]), "player {player}");
            assert!(
                check(&mut sim, 37, &[player, "1100"]),
                "foreign, player {player}"
            );
            assert!(!check(&mut sim, 37, &[player, "1008"]));
        }
        // No filter ID (E-231): a category below 1000 never matches, nor does 0.
        for filter in ["7", "100", "999", "0"] {
            assert!(!check(&mut sim, 37, &["6", filter]), "filter {filter}");
        }
        // Dead entries count until destruction, quartered ones do not.
        sim.object_mut(a).unwrap().status |= status::DIED;
        assert!(check(&mut sim, 37, &["6", "1007"]));
        sim.object_mut(a).unwrap().status &= !status::DIED;
        sim.object_mut(a).unwrap().status |= status::QUARTERED;
        assert!(!check(&mut sim, 37, &["6", "1007"]));
        assert!(check(&mut sim, 37, &["6", "1100"]));
        // Empty selection: false.
        sim.state.script.mission.selection.clear();
        assert!(!check(&mut sim, 37, &["6", "1100"]));
    }

    #[test]
    fn condition_38_ignores_the_player() {
        let (mut state, rules) = started();
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.script.mission.last_button = 140;
        for player in ["0", "7", "-1", "abc", ""] {
            assert!(check(&mut sim, 38, &[player, "140"]), "player {player}");
            assert!(!check(&mut sim, 38, &[player, "141"]), "player {player}");
        }
        // Read with `atol`: non-numeric is 0, so true at the start.
        assert!(!check(&mut sim, 38, &["0", "abc"]));
        sim.state.script.mission.last_button = 0;
        assert!(check(&mut sim, 38, &["0", "abc"]));
        // Nothing is consumed.
        assert!(check(&mut sim, 38, &["0", "0"]));
        assert_eq!(sim.state.script.mission.last_button, 0);
    }

    #[test]
    fn mirror_follows_world_changes_like_the_selection_list() {
        // The mirror models the original's selection list: dying counts (until final
        // destruction), quartered does not, a change of owner does not (the old ID is gone), 147
        // clears, 18 clears only without cinemascope.
        let rules = rules_for(&script(&[(&[], &["n12;a"])]));
        let mut state = new_match(&rules, &setup(true)).unwrap();
        let mut sim = Sim::new(&mut state, &rules);
        let dying = spawn(&mut sim, Some("sterbend"), (2, 2));
        let quartered = spawn(&mut sim, Some("quartier"), (3, 2));
        let sold = spawn(&mut sim, Some("verkauft"), (4, 2));
        let converted = spawn(&mut sim, Some("bekehrt"), (5, 2));
        let stays = spawn(&mut sim, Some("bleibt"), (6, 2));
        sim.state.script.mission.selection = vec![dying, quartered, sold, converted, stays];
        assert_eq!(
            ["sterbend", "quartier", "verkauft", "bekehrt", "bleibt"].map(|n| check(
                &mut sim,
                35,
                &[n]
            )),
            [true; 5]
        );

        sim.object_mut(dying).unwrap().status |= status::DIED;
        assert!(
            check(&mut sim, 35, &["sterbend"]),
            "dying ones stay until destruction"
        );
        assert!(is_selected(&sim, dying));

        sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
        assert!(!check(&mut sim, 35, &["quartier"]));
        assert!(check(&mut sim, 36, &["quartier"]));
        assert!(!is_selected(&sim, quartered));
        // Unquartering (bit gone) makes the entry valid again: the mirror is only a list of IDs.
        sim.object_mut(quartered).unwrap().status &= !status::QUARTERED;
        assert!(check(&mut sim, 35, &["quartier"]));

        destroy_object(&mut sim, sold);
        assert!(
            check(&mut sim, 35, &["verkauft"]),
            "no object any more: true as in the original"
        );
        assert!(!is_selected(&sim, sold));

        let new_id = change_owner(&mut sim, converted, 2).expect("change of owner");
        assert_ne!(new_id, converted);
        assert!(!is_selected(&sim, converted), "the old ID is gone");
        assert!(
            !is_selected(&sim, new_id),
            "the new ID is not in the mirror"
        );
        // The name moves to the new ID (E-145): 35 sees an object that is not selected.
        assert!(!check(&mut sim, 35, &["bekehrt"]));
        assert!(check(&mut sim, 36, &["bekehrt"]));

        assert!(is_selected(&sim, stays));
        assert!(!is_selected(&sim, ObjectId::NONE));
        drop(sim);

        // 147 clears the mirror, 18 in cinemascope does not (the command bar is off).
        let level = script(&[(&[], &["n147", "w149;1;2;0", "n18"])]);
        let rules = rules_for(&level);
        let mut state = new_match(&rules, &setup(true)).unwrap();
        let ids = [ObjectId(0x800), ObjectId(0x801)];
        state.script.mission.selection = ids.to_vec();
        step(&mut state, &rules, &[]);
        assert!(state.script.mission.cinemascope);
        assert!(state.script.mission.selection.is_empty(), "147 clears");
        // During the portrait's wait (600 ms) the selection comes back through the host command.
        step(&mut state, &rules, &[selection(&ids)]);
        assert_eq!(state.script.mission.selection, ids);
        for _ in 0..30 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.script.mission.ui_locked);
        assert_eq!(
            state.script.mission.selection, ids,
            "18 in cinemascope does not clear"
        );

        // Without cinemascope 18 clears.
        let rules = rules_for(&script(&[(&[], &["w149;1;2;0", "n18"])]));
        let mut state = new_match(&rules, &setup(true)).unwrap();
        step(&mut state, &rules, &[selection(&ids)]);
        assert_eq!(state.script.mission.selection, ids);
        for _ in 0..30 {
            step(&mut state, &rules, &[]);
        }
        assert!(state.script.mission.ui_locked);
        assert!(
            state.script.mission.selection.is_empty(),
            "18 clears with a visible command bar"
        );
    }

    #[test]
    fn replay_with_ui_commands_gives_the_same_sequence() {
        // Replay from the command log gives the same sequence as the run - with the host commands of the
        // mirror. Block 0 waits for button 77, block 1 for button 88 (38); the selection mirror goes into
        // the checksum.
        use crate::save::{RecordedCommand, Replay};
        let rules = rules_for(&script(&[
            (&["&38;0;77"], &["n12;erst"]),
            (&["&38;0;88"], &["n12;dann"]),
        ]));
        let setup = setup(true);
        let replay = Replay {
            start: crate::setup::start_package(&rules, &setup),
            commands: vec![
                RecordedCommand {
                    step: 4,
                    command: button(77),
                },
                RecordedCommand {
                    step: 9,
                    command: selection(&[]),
                },
                RecordedCommand {
                    step: 12,
                    command: selection(&[ObjectId(0x800)]),
                },
                RecordedCommand {
                    step: 15,
                    command: button(88),
                },
            ],
        };
        let replay = Replay::from_bytes(&replay.to_bytes()).unwrap();

        let mut state = new_match(&rules, &setup).unwrap();
        let mut live = Vec::new();
        for n in 0..20u64 {
            let due: Vec<Command> = replay
                .commands
                .iter()
                .filter(|c| c.step == n)
                .map(|c| c.command.clone())
                .collect();
            step(&mut state, &rules, &due);
            live.push(crate::save::checksum(&state));
        }
        assert_eq!(state.script.mission.last_button, 88);
        assert_eq!(state.script.mission.selection, [ObjectId(0x800)]);
        let switches: Vec<&str> = state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(switches, ["dann", "erst"]);
        for steps in [1u64, 4, 5, 10, 13, 16, 20] {
            let at = replay.play(&rules, steps).expect("replay");
            assert_eq!(at.steps, steps);
            assert_eq!(
                crate::save::checksum(&at),
                live[steps as usize - 1],
                "tick {steps}"
            );
        }
        // Counter-check: without the host commands a different sequence results.
        let mut plain = new_match(&rules, &setup).unwrap();
        for _ in 0..20 {
            step(&mut plain, &rules, &[]);
        }
        assert_ne!(crate::save::checksum(&plain), live[19]);
    }
}
