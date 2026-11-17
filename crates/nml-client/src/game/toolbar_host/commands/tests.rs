//! Tests of the bar commands, without a GPU with the test worlds of the client.

use nml_formats::gifcon::button_kind as kinds;
use nml_sim::command::{CommandKind, NO_SPREAD};
use nml_sim::id::ObjectId;
use nml_sim::motion::NO_DIRECTION;
use nml_ui::coords::Screen;
use nml_ui::hud::toolbar::button::{ButtonEvent, KIND_PRODUCTION, KIND_STANCE};
use nml_ui::hud::toolbar::data::ToolbarData;
use nml_ui::hud::toolbar::{ButtonSet, Cursor, Toolbar, ToolbarEvent, ToolbarIntent, ToolbarOut};

use super::*;
use crate::game::economy::testkit::{COLONIST, HQ, host, spawn};
use crate::game::selection::Selection;
use crate::game::sim_host::SimHost;
use crate::game::toolbar_host::toolbar_model;
use crate::game::unit_bar::{self, BarAction, UnitBar};

const SCREEN: Screen = Screen {
    width: 1024,
    height: 768,
};

/// Bar without positions and icons: the tests press the buttons through the click path [`Toolbar::press`].
fn toolbar() -> Toolbar {
    Toolbar::new(ToolbarData::from_texts("", ""), SCREEN)
}

/// The commands the host holds for the next tick boundary; empty afterwards.
fn sent(host: &mut SimHost) -> Vec<CommandKind> {
    let out = host
        .pending_commands()
        .iter()
        .map(|c| c.kind.clone())
        .collect();
    host.step_once();
    out
}

/// Without a step: the held commands.
fn pending(host: &SimHost) -> Vec<CommandKind> {
    host.pending_commands()
        .iter()
        .map(|c| c.kind.clone())
        .collect()
}

/// The intents of the outputs.
fn intents(outs: &[ToolbarOut]) -> Vec<ToolbarIntent> {
    outs.iter()
        .filter_map(|o| match o {
            ToolbarOut::Intent(i) => Some(i.clone()),
            _ => None,
        })
        .collect()
}

/// Execute intents with the selection `selection` (without keys).
fn run(
    host: &mut SimHost,
    selection: &mut Selection,
    bar: &mut UnitBar,
    groups: &ButtonSet,
    shift: bool,
    list: Vec<ToolbarIntent>,
) -> Vec<ToolbarEffects> {
    list.into_iter()
        .map(|i| {
            execute_in(
                i,
                &mut CommandContext {
                    sim: host,
                    selection,
                    bar,
                    groups,
                    shift,
                    ctrl: false,
                },
            )
        })
        .collect()
}

/// Click sequence of a double-click on the button: 9, 9, `0x27` (re/game-ui N.14.7).
const DOUBLE_CLICK: [ButtonEvent; 3] = [ButtonEvent::Left, ButtonEvent::Left, ButtonEvent::Double];

/// Press button `msg` of kind `kind` with the click sequence `events`; outputs.
fn press(
    toolbar: &mut Toolbar,
    host: &SimHost,
    selection: &[ObjectId],
    shift: bool,
    (msg, kind, object): (i32, u32, Option<u32>),
    events: &[ButtonEvent],
) -> Vec<ToolbarOut> {
    let mut model = toolbar_model(host, selection, false);
    model.keys.shift = shift;
    let mut out = Vec::new();
    for &ev in events {
        toolbar.press(
            ToolbarEvent::Click {
                msg,
                kind,
                object,
                ev,
            },
            &model,
            &mut out,
        );
    }
    out
}

fn enqueued(cmds: &[CommandKind], building: ObjectId) -> usize {
    cmds.iter()
        .filter(|c| {
            matches!(c, CommandKind::EnqueueProduction { building: b, guid } if *b == building && *guid == COLONIST)
        })
        .count()
}

#[test]
fn double_click_make_orders_four_and_twelve_with_shift() {
    // O-224: double-click on MAKE gives 1 + 1 + 2 = 4 orders, with Shift 5 + 5 + 2 = 12; all to the head of the list.
    let mut host = host();
    let hq = spawn(&mut host, HQ, 0, (10, 10));
    let mut sel = Selection::of(vec![hq]);
    let mut bar = UnitBar::default();
    let groups = ButtonSet::default();
    for (shift, counts, total) in [(false, [1, 1, 2], 4), (true, [5, 5, 2], 12)] {
        let mut tb = toolbar();
        let outs = press(
            &mut tb,
            &host,
            &sel.ids,
            shift,
            (COLONIST as i32, kinds::MAKE, None),
            &DOUBLE_CLICK,
        );
        let list = intents(&outs);
        let got: Vec<u8> = list
            .iter()
            .map(|i| match i {
                ToolbarIntent::Make { count, .. } => *count,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, counts);
        run(&mut host, &mut sel, &mut bar, &groups, shift, list);
        assert_eq!(enqueued(&pending(&host), hq), total, "shift {shift}");
        host.step_once();
    }
}

#[test]
fn queue_double_click_cancels_three_with_the_same_index() {
    // O-225: a double-click on a waiting order gives three cancels with the same index (kind & 0xFF) and the GUID as
    // the second word.
    let mut host = host();
    let hq = spawn(&mut host, HQ, 0, (10, 10));
    let mut sel = Selection::of(vec![hq]);
    let mut tb = toolbar();
    let outs = press(
        &mut tb,
        &host,
        &sel.ids,
        false,
        (COLONIST as i32, KIND_PRODUCTION | 2, None),
        &DOUBLE_CLICK,
    );
    let list = intents(&outs);
    assert_eq!(list.len(), 3);
    run(
        &mut host,
        &mut sel,
        &mut UnitBar::default(),
        &ButtonSet::default(),
        false,
        list,
    );
    let cancel = CommandKind::CancelProduction {
        building: hq,
        index: 2,
        arg: COLONIST as i32,
    };
    assert_eq!(pending(&host), vec![cancel; 3]);
}

#[test]
fn head_only_commands_take_the_first_object() {
    // K-88: `0x0E` (MAKE) and `0x2D` (pack tent) go only to the head of the list, also with three objects; with an
    // empty list no command (`+0x34`, `FUN_0044B490`).
    let mut host = host();
    let hqs = [(10, 10), (20, 10), (30, 10)].map(|t| spawn(&mut host, HQ, 0, t));
    let mut sel = Selection::of(hqs.to_vec());
    let mut bar = UnitBar::default();
    let groups = ButtonSet::default();
    let make = ToolbarIntent::Make {
        guid: COLONIST,
        count: 1,
        upgrade: false,
    };
    run(
        &mut host,
        &mut sel,
        &mut bar,
        &groups,
        false,
        vec![make.clone()],
    );
    assert_eq!(
        sent(&mut host),
        [CommandKind::EnqueueProduction {
            building: hqs[0],
            guid: COLONIST
        }]
    );
    let mut empty = Selection::default();
    run(&mut host, &mut empty, &mut bar, &groups, false, vec![make]);
    assert!(pending(&host).is_empty());

    use crate::game::testworld::{DWELLING_TENT, host as tent_host, spawn as tent_spawn};
    let mut host = tent_host();
    let tents = [(10, 10), (20, 10), (30, 10)].map(|t| tent_spawn(&mut host, DWELLING_TENT, 0, t));
    let mut sel = Selection::of(tents.to_vec());
    let pack = ToolbarIntent::Dispatch {
        msg: 452,
        objects: tents.iter().map(|t| t.0).collect(),
    };
    run(&mut host, &mut sel, &mut bar, &groups, false, vec![pack]);
    assert_eq!(pending(&host), [CommandKind::PackTent { tent: tents[0] }]);
}

#[test]
fn group_and_targets_only_matching_objects() {
    // O-272: if the BUTTONMSG hits a GROUP_AND list, the command takes only it (head or list), otherwise the whole
    // selection. Through the click path `Dispatch` already carries the list.
    let mut host = host();
    let hqs = [(10, 10), (20, 10)].map(|t| spawn(&mut host, HQ, 0, t));
    let units = [(30, 30), (31, 30), (32, 30)].map(|t| spawn(&mut host, COLONIST, 0, t));
    let mut bar = UnitBar::default();
    let mut groups = ButtonSet::default();
    groups.groups.insert(COLONIST, vec![hqs[1].0]);
    groups.groups.insert(423, vec![units[1].0]);
    groups.groups.insert(408, vec![units[2].0]);
    // MAKE: head of the list, not of the selection.
    let mut sel = Selection::of(hqs.to_vec());
    let make = ToolbarIntent::Make {
        guid: COLONIST,
        count: 1,
        upgrade: false,
    };
    run(&mut host, &mut sel, &mut bar, &groups, false, vec![make]);
    assert_eq!(
        sent(&mut host),
        [CommandKind::EnqueueProduction {
            building: hqs[1],
            guid: COLONIST
        }]
    );
    // Stance 423 (mode 2): only the list; stance 420 (mode 3) without a match: the whole selection.
    let mut sel = Selection::of(units.to_vec());
    run(
        &mut host,
        &mut sel,
        &mut bar,
        &groups,
        false,
        vec![ToolbarIntent::Stance(2), ToolbarIntent::Stance(3)],
    );
    assert_eq!(
        sent(&mut host),
        [
            CommandKind::SetBehavior {
                units: vec![units[1]],
                mode: 2
            },
            CommandKind::SetBehavior {
                units: units.to_vec(),
                mode: 3
            }
        ]
    );
    // Die through the click path of the bar: the bar looks up the list itself.
    let mut tb = toolbar();
    tb.set.groups = groups.groups.clone();
    let outs = press(
        &mut tb,
        &host,
        &sel.ids,
        false,
        (408, kinds::DO, None),
        &[ButtonEvent::Left],
    );
    run(
        &mut host,
        &mut sel,
        &mut bar,
        &groups,
        false,
        intents(&outs),
    );
    assert_eq!(
        pending(&host),
        [CommandKind::Die {
            units: vec![units[2]]
        }]
    );
}

#[test]
fn stance_buttons_send_set_behavior_3_4_1_2_0() {
    // re/game-ui N.5.3: stance 420 ... 424 -> `0x12` mode 3, 4, 1, 2, 0 to the list; 421 on a building mode 4 (on units
    // it opens the defense radius, see below).
    let mut host = host();
    let hq = spawn(&mut host, HQ, 0, (10, 10));
    let mut sel = Selection::of(vec![hq]);
    let mut tb = toolbar();
    let mut outs = Vec::new();
    for msg in 420..=424 {
        outs.extend(press(
            &mut tb,
            &host,
            &sel.ids,
            false,
            (msg, KIND_STANCE, None),
            &[ButtonEvent::Left],
        ));
    }
    run(
        &mut host,
        &mut sel,
        &mut UnitBar::default(),
        &ButtonSet::default(),
        false,
        intents(&outs),
    );
    let modes: Vec<i32> = pending(&host)
        .iter()
        .map(|c| match c {
            CommandKind::SetBehavior { units, mode } => {
                assert_eq!(units, &[hq]);
                *mode
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(modes, [3, 4, 1, 2, 0]);
}

#[test]
fn die_408_sends_die_to_the_group() {
    // re/game-ui N.5.4: 408 (submenu 407) -> `0x10` with the list, one command (`FUN_0044BF90`,
    // `Input_BuildSelectionCommand`).
    let mut host = host();
    let units = [(30, 30), (31, 30)].map(|t| spawn(&mut host, COLONIST, 0, t));
    let mut sel = Selection::of(units.to_vec());
    let mut tb = toolbar();
    let outs = press(
        &mut tb,
        &host,
        &sel.ids,
        false,
        (408, kinds::DO, None),
        &[ButtonEvent::Left],
    );
    assert!(matches!(
        intents(&outs)[..],
        [ToolbarIntent::Dispatch { msg: 408, .. }]
    ));
    run(
        &mut host,
        &mut sel,
        &mut UnitBar::default(),
        &ButtonSet::default(),
        false,
        intents(&outs),
    );
    assert_eq!(
        pending(&host),
        [CommandKind::Die {
            units: units.to_vec()
        }]
    );
}

#[test]
fn defense_point_moves_then_queues_mode_4() {
    // `0x0044BD60`: 421 on units opens the defense radius cursor; its click gives `0x12` mode 0, the walk in the chosen
    // formation (fixed-point tiles, no direction, spread -1) and a wrapped `0x12` mode 4.
    let mut host = host();
    let units = [(30, 30), (31, 30)].map(|t| spawn(&mut host, COLONIST, 0, t));
    let mut sel = Selection::of(units.to_vec());
    let mut tb = toolbar();
    let outs = press(
        &mut tb,
        &host,
        &sel.ids,
        false,
        (421, KIND_STANCE, None),
        &[ButtonEvent::Left],
    );
    assert!(intents(&outs).is_empty());
    assert!(matches!(tb.cursor, Some(Cursor::Defense(_))));
    let mut bar = UnitBar {
        formation: 2,
        ..UnitBar::default()
    };
    run(
        &mut host,
        &mut sel,
        &mut bar,
        &ButtonSet::default(),
        false,
        vec![ToolbarIntent::DefensePoint {
            x: 5.0 * 128.0 + 64.0,
            z: 10.0 * 128.0,
        }],
    );
    assert_eq!(
        pending(&host),
        [
            CommandKind::SetBehavior {
                units: units.to_vec(),
                mode: 0
            },
            CommandKind::Move {
                units: units.to_vec(),
                x: (5 << 12) + (1 << 11),
                z: 10 << 12,
                target: -1,
                formation: 2,
                direction: NO_DIRECTION,
                spread: NO_SPREAD,
                flags: 0,
            },
            CommandKind::Queue {
                cyclic: false,
                inner: Box::new(CommandKind::SetBehavior {
                    units: units.to_vec(),
                    mode: 4
                }),
            },
        ]
    );
    // Without a selection nothing.
    host.step_once();
    run(
        &mut host,
        &mut Selection::default(),
        &mut bar,
        &ButtonSet::default(),
        false,
        vec![ToolbarIntent::DefensePoint { x: 1.0, z: 1.0 }],
    );
    assert!(pending(&host).is_empty());
}

#[test]
fn target_decal_only_for_walk_targets() {
    // O-237: the target point decal only for 427, 448, 530 (ground, y = 0) and 529, 533 (object with height); not for
    // 413, 447 and ability targets; only with a command that went out.
    let mut host = host();
    let unit = spawn(&mut host, COLONIST, 0, (30, 30));
    let hq = spawn(&mut host, HQ, 0, (10, 10));
    let mut sel = Selection::of(vec![unit]);
    let mut bar = UnitBar::default();
    let groups = ButtonSet::default();
    let chosen = |msg: i32, object: Option<ObjectId>, ground: Option<[f32; 2]>| {
        ToolbarIntent::TargetChosen {
            msg,
            object: object.map(|o| o.0),
            ground,
        }
    };
    let effects = run(
        &mut host,
        &mut sel,
        &mut bar,
        &groups,
        false,
        vec![
            chosen(427, None, Some([700.0, 900.0])),
            chosen(448, None, Some([710.0, 910.0])),
            chosen(413, Some(hq), None),
            chosen(447, None, Some([720.0, 920.0])),
            chosen(480, None, Some([730.0, 930.0])),
            chosen(533, Some(hq), None),
        ],
    );
    let decals: Vec<Option<TargetDecal>> = effects.iter().map(|e| e.target_decal).collect();
    assert_eq!(
        decals,
        [
            Some(TargetDecal {
                button: 427,
                pos: [700.0, 0.0, 900.0]
            }),
            Some(TargetDecal {
                button: 448,
                pos: [710.0, 0.0, 910.0]
            }),
            None,
            None,
            None,
            // 533 has no entry in `targets.ini` in the test world: no command, so no target point (the decal belongs
            // to a command that went out, `Input_IssueTargetAction`). The position with height of the object targets
            // is tested by `game_session::tests::decal_lies_on_the_ground_or_at_the_object`.
            None,
        ]
    );
    // Without an own object in the selection no command goes out and there is no target point (the original returns
    // before `Input_IssueTargetAction`).
    let foreign = spawn(&mut host, COLONIST, 1, (40, 40));
    let mut sel = Selection::of(vec![foreign]);
    let effects = run(
        &mut host,
        &mut sel,
        &mut bar,
        &groups,
        false,
        vec![chosen(448, None, Some([710.0, 910.0]))],
    );
    assert_eq!(effects[0].target_decal, None);
}

#[test]
fn button_413_on_own_building_is_repair_not_attack() {
    // Target button 413 takes its own entry in `targets.ini` (repair `0x02`), not the right-click action of the target
    // and no attack.
    let mut host = host();
    let unit = spawn(&mut host, COLONIST, 0, (30, 30));
    let hq = spawn(&mut host, HQ, 0, (10, 10));
    let mut sel = Selection::of(vec![unit]);
    let right_click = host.target_action(&sel.ids, Some(hq), glam::Vec3::ZERO);
    assert_ne!(right_click.map(|a| a.0), Some(413));
    run(
        &mut host,
        &mut sel,
        &mut UnitBar::default(),
        &ButtonSet::default(),
        false,
        vec![ToolbarIntent::TargetChosen {
            msg: 413,
            object: Some(hq.0),
            ground: None,
        }],
    );
    let cmds = pending(&host);
    assert_eq!(
        cmds,
        [CommandKind::ConstructAt {
            building: hq,
            units: vec![unit]
        }]
    );
    assert!(!cmds.iter().any(|c| matches!(c, CommandKind::Attack { .. })));
    // A target that does not meet the TARGET filter (foreign building, `MM_HL_MYLIST`) gives nothing.
    host.step_once();
    let foreign = spawn(&mut host, HQ, 1, (40, 40));
    assert!(
        host.button_action(413, &sel.ids, Some(foreign), glam::Vec3::ZERO)
            .is_none()
    );
}

#[test]
fn same_command_as_sim_unit_bar() {
    // For every button of the unit bar with an unchanged form (not the gates 430/431, whose form K-88 corrects; not
    // the target buttons, unpacking 453 and the counter-upgrades, which go through the cursor or the head of the list)
    // the bar path gives the same command sequence as `sim_unit_bar` (`control_unit_bar`).
    use crate::game::testworld::{
        ANIMAL_EVOKER, ARCHER, DWELLING_TENT, GHOST_EVOKER, MEDIC, SPA_COLONIST, SPA_SOLDIER,
        host as world, spawn as put,
    };
    let make = || {
        let mut h = world();
        let ids: Vec<ObjectId> = [
            SPA_SOLDIER,
            SPA_COLONIST,
            ARCHER,
            MEDIC,
            GHOST_EVOKER,
            ANIMAL_EVOKER,
        ]
        .iter()
        .enumerate()
        .map(|(i, &g)| put(&mut h, g, 0, (10 + i as i32, 10)))
        .collect();
        let tent = put(&mut h, DWELLING_TENT, 0, (30, 30));
        (h, ids, tent)
    };
    let mut buttons: Vec<i32> = (511..=515)
        .chain([519, 405, 406, 432, 433, 449, 450, 451, 452, 485, 486])
        .chain([434, 435, 437, 438, 458, 459, 554, 555, 564])
        .chain([
            436, 439, 441, 442, 443, 547, 550, 553, 558, 562, 482, 540, 542,
        ])
        .chain(unit_bar::ABILITY_BUTTONS.iter().map(|b| b.0))
        .collect();
    buttons.dedup();
    let mut with_commands = Vec::new();
    for b in buttons {
        let action = BarAction::from_button(b).unwrap_or_else(|| panic!("button {b}"));
        assert!(!matches!(
            action,
            BarAction::Target(_) | BarAction::UnpackTent
        ));
        for tent_first in [false, true] {
            let (mut a, ids, tent) = make();
            let (mut c, _, _) = make();
            let sel: Vec<ObjectId> = if tent_first { vec![tent] } else { ids };
            unit_bar::control_unit_bar(&mut UnitBar::default(), &mut a, &sel, b, None, None)
                .unwrap();
            let intent = if b == 519 {
                ToolbarIntent::Scatter
            } else {
                ToolbarIntent::Dispatch {
                    msg: b,
                    objects: sel.iter().map(|o| o.0).collect(),
                }
            };
            let mut selection = Selection::of(sel.clone());
            run(
                &mut c,
                &mut selection,
                &mut UnitBar::default(),
                &ButtonSet::default(),
                false,
                vec![intent],
            );
            assert_eq!(pending(&c), pending(&a), "button {b}");
            if !pending(&a).is_empty() {
                with_commands.push(b);
            }
        }
    }
    // The comparison is not empty: these buttons give commands in the test world.
    eprintln!("Buttons with commands: {with_commands:?}");
    assert!(with_commands.len() >= 12, "{with_commands:?}");
}

#[test]
fn unquarter_all_goes_to_each_object() {
    // K-88: 416 sends one `0x15` per object of the list (`Toolbar_SubmitUnquarterAll`); the "Economy" window of the
    // viewer follows (per own selected object).
    use crate::game::economy::{EconomyUi, PanelAction};
    let mut host = host();
    let hqs = [(10, 10), (20, 10)].map(|t| spawn(&mut host, HQ, 0, t));
    let mut sel = Selection::of(hqs.to_vec());
    run(
        &mut host,
        &mut sel,
        &mut UnitBar::default(),
        &ButtonSet::default(),
        false,
        vec![ToolbarIntent::UnquarterAll],
    );
    let each: Vec<CommandKind> = hqs
        .iter()
        .map(|&container| CommandKind::UnquarterAll { container })
        .collect();
    assert_eq!(sent(&mut host), each);
    EconomyUi::default().apply(&mut host, &sel.ids, PanelAction::UnquarterAll);
    assert_eq!(pending(&host), each);
}
