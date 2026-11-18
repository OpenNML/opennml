//! Tests of the snapshot of the match for the command bar, without a GPU with the testkit of the economy.

use opennml_sim::module::ModuleKind;
use opennml_sim::module::building::flags as bflags;
use opennml_sim::object::status;
use opennml_sim::upgrades::CounterUpgrade;
use opennml_ui::coords::Screen;
use opennml_ui::hud::toolbar::data::ToolbarData;
use opennml_ui::hud::toolbar::{BuildCursor, ToolbarOut, ToolbarSound};
use opennml_ui::text::{GameTags, GameText, TagParams, resolve_text};

use super::*;
use crate::game::economy::testkit::{COLONIST, HOUSE, HQ, host, spawn};
use crate::game::hud::SimGameText;
use crate::game::selection::Selection;
use crate::game::voices::Voices;

const SCREEN: Screen = Screen {
    width: 1024,
    height: 768,
};

/// Bar data with positions as in the shipped data and one icon per GUID (sheet 0, tile = GUID mod 50).
fn data(icons: &[u32]) -> ToolbarData {
    let mut ini = String::from("ICONS data/icons/i0.tga\r\n");
    for g in icons {
        ini.push_str(&format!("GUID {g} 0 0 0 {}\r\n", g % 50));
    }
    let mut d = ToolbarData::from_texts("", &ini);
    d.normal = (0..14)
        .map(|i| [245 + 53 * (i % 7), if i < 7 { 17 } else { 71 }])
        .collect();
    d.selection = (0..30)
        .map(|i| [234 + 26 * (i / 2), 128 + 25 * (i % 2)])
        .collect();
    d.heroes = (0..5).map(|k| [-55 * (k + 1), -50]).collect();
    d.headhunters = (0..5).map(|k| [-55 * (k + 1), -110]).collect();
    d
}

/// Bar with icons for the buttons of the testkit.
fn toolbar() -> Toolbar {
    Toolbar::new(data(&[413, 1000, 1001, 1003, 1100, 2806, 600]), SCREEN)
}

/// The events of a frame to the bar like the host does (model of this frame with the active target list); outputs
/// of the bar.
fn feed(
    toolbar: &mut Toolbar,
    sim: &mut SimHost,
    selection: &[ObjectId],
    events: &[HudEvent],
) -> Vec<ToolbarOut> {
    let targets = active_target_buttons(sim, selection);
    let mut model = toolbar_model(sim, selection, false);
    model.targets = targets;
    let mut out = Vec::new();
    for e in events {
        toolbar.event(e, &model, &mut out);
    }
    out
}

/// The selection part of `GameSession::advance_by` without a GPU: step of the selection, `{8}` on change.
fn step_selection(sel: &mut Selection, sim: &SimHost, th: &mut ToolbarHost) -> bool {
    let before = sel.ids.clone();
    let reselected = sel.apply_sim_step(
        &[],
        &sim.state,
        &sim.rules,
        |o| sim.fog_selectable(o),
        sim.active_player,
    );
    if reselected || sel.ids != before {
        th.selection_changed();
    }
    reselected
}

fn kinds(events: &[HudEvent]) -> Vec<(HudEventKind, Vec<i32>)> {
    events.iter().map(|e| (e.kind, e.args.clone())).collect()
}

fn rebuilds(events: &[HudEvent]) -> Vec<i32> {
    events
        .iter()
        .filter(|e| e.kind == HudEventKind::Rebuild)
        .map(|e| e.args[0])
        .collect()
}

fn building_mut(sim: &mut SimHost, id: ObjectId) -> &mut opennml_sim::module::building::Building {
    match &mut sim
        .state
        .object_mut(id, false)
        .unwrap()
        .module_mut(3)
        .unwrap()
        .kind
    {
        ModuleKind::Building(b) => b,
        _ => panic!("not a building"),
    }
}

fn quarter_mut(sim: &mut SimHost, id: ObjectId) -> &mut opennml_sim::module::quarter::Quarter {
    match &mut sim
        .state
        .object_mut(id, false)
        .unwrap()
        .module_mut(23)
        .unwrap()
        .kind
    {
        ModuleKind::Quarter(q) => q,
        _ => panic!("not a quarters module"),
    }
}

// QUIRK O-236: kept
#[test]
fn signature_kinds_follow_n_2() {
    // re/game-ui N.2 (deviation from the original): order queued -> 0, order done/continues -> 3, gate -> 2, an
    // occupant out -> 1, occupant in -> 0, type change without `{8}` -> 0; unchanged nothing.
    let mut sim = host();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let sel = [hq];
    let mut last = Signature::of(&sim, &sel);
    let mut next = |sim: &SimHost| {
        let s = Signature::of(sim, &sel);
        let k = rebuild_kind(&last, &s, &[]);
        last = s;
        k
    };
    assert_eq!(next(&sim), None, "unchanged");
    building_mut(&mut sim, hq).make_guid = COLONIST;
    assert_eq!(next(&sim), Some(0), "order queued (empty -> running)");
    building_mut(&mut sim, hq).make_queue.push_back(COLONIST);
    assert_eq!(next(&sim), Some(0), "order queued (queue)");
    let b = building_mut(&mut sim, hq);
    b.make_queue.pop_front();
    assert_eq!(next(&sim), Some(3), "order done, the next one runs");
    let flags = &mut sim
        .state
        .object_mut(hq, false)
        .unwrap()
        .module_mut(3)
        .unwrap()
        .base
        .flags;
    *flags |= bflags::TOROPEN;
    assert_eq!(next(&sim), Some(2), "gate open");
    let c = spawn(&mut sim, COLONIST, 0, (12, 12));
    let q = quarter_mut(&mut sim, hq);
    q.occupants.push(c);
    q.cur = 1;
    assert_eq!(next(&sim), Some(0), "occupant in");
    let q = quarter_mut(&mut sim, hq);
    q.occupants.clear();
    q.cur = 0;
    assert_eq!(next(&sim), Some(1), "occupant out");
    sim.state.object_mut(hq, false).unwrap().guid = HOUSE;
    assert_eq!(next(&sim), Some(0), "type change without {{8}}");
    sim.state.object_mut(hq, false).unwrap().load_amount = 5;
    assert_eq!(next(&sim), Some(0), "load 0 -> > 0");
    sim.state.button_locks.insert(1100, 1);
    assert_eq!(next(&sim), Some(0), "script lock 207");
    assert_eq!(next(&sim), None);

    // An occupant out and an order done at the same time: the silent rebuild covers both (0).
    let mut a = Signature::of(&sim, &sel);
    a.objects[0].occupants = 2;
    a.objects[0].running = COLONIST;
    let mut b = a.clone();
    b.objects[0].occupants = 1;
    b.objects[0].running = 0;
    assert_eq!(rebuild_kind(&a, &b, &[]), Some(0));
    // Arming: `{0x10, 2}`, then `{0x10, 0}`.
    let mut armed = a.clone();
    armed.objects[0].armed = true;
    assert_eq!(
        rebuild_events(&a, &armed, &[])
            .iter()
            .map(|e| e.args[0])
            .collect::<Vec<_>>(),
        [2, 0]
    );
    // Gate alone without the second message.
    let mut gate = a.clone();
    gate.objects[0].gate ^= bflags::TOROPEN & 0xFFFF_FF00;
    assert_eq!(
        rebuild_events(&a, &gate, &[])
            .iter()
            .map(|e| e.args[0])
            .collect::<Vec<_>>(),
        [2]
    );
    // Foreign objects report nothing (`0x00469390`).
    let mut foreign = a.clone();
    foreign.objects[0].own = false;
    let mut foreign2 = foreign.clone();
    foreign2.objects[0].gate ^= 0x40_0000;
    assert_eq!(rebuild_kind(&foreign, &foreign2, &[]), None);
    // An object selected only in the new frame triggers nothing (selection without `{8}`, O-270).
    let empty = Signature {
        objects: Vec::new(),
        ..a.clone()
    };
    assert_eq!(rebuild_kind(&empty, &a, &[]), None);
}

// QUIRK O-395: kept
#[test]
fn production_cancel_event_makes_a_silent_rebuild() {
    // `0x00507960`: a cancel reports `{0x10, 0}` (after "in progress" -1), only for a selected own building
    // (`0x00469380`). Without a cancel the shorter queue stays a completion (`{0x10, 3}`).
    let mut sim = host();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let other = spawn(&mut sim, HQ, 0, (20, 20));
    let mut a = Signature::of(&sim, &[hq]);
    a.objects[0].running = COLONIST;
    a.objects[0].queue = 2;
    let mut b = a.clone();
    b.objects[0].queue = 1;
    assert_eq!(rebuild_kind(&a, &b, &[]), Some(3), "completion");
    assert_eq!(
        rebuild_kind(&a, &b, &[hq.0]),
        Some(0),
        "cancel of a waiting one"
    );
    // Cancel of the running one without a successor, and a cancel without a visible change of the signature.
    let mut c = a.clone();
    c.objects[0].running = 0;
    c.objects[0].queue = 0;
    assert_eq!(rebuild_kind(&a, &c, &[hq.0]), Some(0));
    assert_eq!(rebuild_kind(&c, &c, &[hq.0]), Some(0));
    assert_eq!(
        rebuild_events(&a, &b, &[hq.0])
            .iter()
            .map(|e| e.args[0])
            .collect::<Vec<_>>(),
        [0]
    );
    // Building not selected: nothing.
    assert_eq!(rebuild_kind(&c, &c, &[other.0]), None);
    // Foreign building: nothing.
    let mut foreign = c.clone();
    foreign.objects[0].own = false;
    assert_eq!(rebuild_kind(&foreign, &foreign, &[hq.0]), None);
    // Not selected until this frame (no previous frame): nothing, the `{8}` rebuilds fully anyway.
    let empty = Signature {
        objects: Vec::new(),
        ..c.clone()
    };
    assert_eq!(rebuild_kind(&empty, &c, &[hq.0]), None);
    // Gate and cancel in the same step: 2 before 0.
    let mut gate = b.clone();
    gate.objects[0].gate ^= bflags::TOROPEN & 0xFFFF_FF00;
    assert_eq!(rebuild_kind(&a, &gate, &[hq.0]), Some(2));
}

/// The `BUTTONMSG` of the occupied normal buttons.
fn normal_msgs(bar: &Toolbar) -> Vec<i32> {
    bar.set
        .normal
        .iter()
        .flatten()
        .map(|b| b.spec.msg)
        .collect()
}

/// A cancelled upgrade appears again in the bar at once, whether it waited or ran (original `0x00507960` ->
/// `{0x10, 0}` -> silent rebuild `0x00457DB0` with the hide check of `0x004557C0`).
// QUIRK O-395: kept
#[test]
fn cancelled_upgrade_returns_to_the_bar() {
    const RESEARCH: u32 = 2806;
    let mut sim = host();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let sel = [hq];
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    th.selection_changed();
    let ev = th.frame(&sim, &sel, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel, &ev);
    assert!(
        normal_msgs(&bar).contains(&(RESEARCH as i32)),
        "upgrade button at the start"
    );
    // One step with the commands, then one frame of the bar; returns the `{0x10, n}` of the frame.
    let run = |sim: &mut SimHost,
               th: &mut ToolbarHost,
               bar: &mut Toolbar,
               commands: &[opennml_sim::command::CommandKind]| {
        for c in commands {
            sim.submit_own(c.clone());
        }
        let events = sim.step_once();
        let ev = th.frame(sim, &sel, &events, 0).to_vec();
        feed(bar, sim, &sel, &ev);
        rebuilds(&ev)
    };
    use opennml_sim::command::CommandKind::{CancelProduction, EnqueueProduction};
    let enqueue = |guid| EnqueueProduction { building: hq, guid };
    let cancel = |index| CancelProduction {
        building: hq,
        index,
        arg: 0,
    };

    // Waiting: colonist runs, the upgrade waits; the button disappears on queueing.
    assert_eq!(
        run(
            &mut sim,
            &mut th,
            &mut bar,
            &[enqueue(COLONIST), enqueue(RESEARCH)]
        ),
        [0]
    );
    assert_eq!(building_mut(&mut sim, hq).make_queue.len(), 1);
    assert!(!normal_msgs(&bar).contains(&(RESEARCH as i32)), "queued");
    // Cancel of the waiting one: silent rebuild, the button is back at once.
    assert_eq!(run(&mut sim, &mut th, &mut bar, &[cancel(1)]), [0]);
    assert!(
        normal_msgs(&bar).contains(&(RESEARCH as i32)),
        "cancelled while waiting"
    );

    // Running: queue again, cancel the colonist, the upgrade moves up and starts.
    assert_eq!(run(&mut sim, &mut th, &mut bar, &[enqueue(RESEARCH)]), [0]);
    assert_eq!(run(&mut sim, &mut th, &mut bar, &[cancel(0)]), [0]);
    assert!(
        !normal_msgs(&bar).contains(&(RESEARCH as i32)),
        "still running"
    );
    run(&mut sim, &mut th, &mut bar, &[]);
    let b = building_mut(&mut sim, hq);
    assert_eq!(b.make_guid, RESEARCH);
    assert!(b.make_time > 0, "upgrade runs");
    assert!(!normal_msgs(&bar).contains(&(RESEARCH as i32)));
    // Cancel of the running one: the button is back at once.
    assert_eq!(run(&mut sim, &mut th, &mut bar, &[cancel(0)]), [0]);
    assert!(
        normal_msgs(&bar).contains(&(RESEARCH as i32)),
        "cancelled while running"
    );
}

/// In the pause (`halted`) the cancel takes effect at once (`execute_now`, O-318), without a tick. The pause frame
/// fetches the events of the immediate command (`take_immediate_events`, like `GameSession::advance_paused`); from that
/// `{0x10, 0}` results instead of `{0x10, 3}`, the button is back in the same frame. A second pause frame does not
/// rebuild again; the first tick after resuming delivers the event again (second silent rebuild, without effect).
// QUIRK O-395: kept
#[test]
fn cancelled_upgrade_returns_to_the_bar_in_the_pause() {
    const RESEARCH: u32 = 2806;
    use opennml_sim::command::CommandKind::{CancelProduction, EnqueueProduction};
    let mut sim = host();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let sel = [hq];
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    th.selection_changed();
    let ev = th.frame(&sim, &sel, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel, &ev);
    for guid in [COLONIST, RESEARCH] {
        sim.submit_own(EnqueueProduction { building: hq, guid });
    }
    let events = sim.step_once();
    let ev = th.frame(&sim, &sel, &events, 0).to_vec();
    feed(&mut bar, &mut sim, &sel, &ev);
    assert!(!normal_msgs(&bar).contains(&(RESEARCH as i32)), "queued");

    // Pause frame: command at once, then the bar with the events of the immediate command.
    sim.halted = true;
    let steps = sim.state.steps;
    sim.submit_own(CancelProduction {
        building: hq,
        index: 1,
        arg: 0,
    });
    assert_eq!(sim.state.steps, steps, "no tick");
    assert_eq!(building_mut(&mut sim, hq).make_queue.len(), 0, "at once");
    let paused = |sim: &mut SimHost, th: &mut ToolbarHost, bar: &mut Toolbar| {
        let events = sim.take_immediate_events();
        let ev = th.frame(sim, &sel, &events, 0).to_vec();
        feed(bar, sim, &sel, &ev);
        rebuilds(&ev)
    };
    assert_eq!(paused(&mut sim, &mut th, &mut bar), [0]);
    assert!(
        normal_msgs(&bar).contains(&(RESEARCH as i32)),
        "cancelled in the pause"
    );
    assert!(paused(&mut sim, &mut th, &mut bar).is_empty(), "only once");

    // Resume: the tick delivers the event once more, the button stays; the buffer is empty afterwards.
    sim.halted = false;
    let events = sim.step_once();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::ProductionCancelled { building } if *building == hq))
    );
    let ev = th.frame(&sim, &sel, &events, 0).to_vec();
    feed(&mut bar, &mut sim, &sel, &ev);
    assert_eq!(rebuilds(&ev), [0]);
    assert!(normal_msgs(&bar).contains(&(RESEARCH as i32)));
    assert!(sim.take_immediate_events().is_empty());
}

#[test]
fn dead_selection_rebuilds_without_buttons() {
    // The selected object dies: the selection loses it (`retain_existing`), the host sends `{8}`, the bar rebuilds
    // without buttons; no selection sound of the bar.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let mut sel = Selection::of(vec![c]);
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    th.selection_changed();
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel.ids, &ev);
    assert_eq!(bar.selected, c.0 as i32);
    assert!(bar.buttons().count() > 0, "buttons of the colonist (413)");
    sim.state.object_mut(c, false).unwrap().status |= status::DIED;
    assert!(step_selection(&mut sel, &sim, &mut th));
    assert!(sel.ids.is_empty());
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    assert_eq!(ev[0].kind, HudEventKind::SelectionChanged);
    let out = feed(&mut bar, &mut sim, &sel.ids, &ev);
    assert_eq!(bar.selected, 0);
    assert_eq!(bar.set.buttons().count(), 0, "no buttons of dead objects");
    assert!(!out.contains(&ToolbarOut::Sound(ToolbarSound::Select)));
}

#[test]
fn quartered_selection_drops_the_unit() {
    // Quartering (deviation from the original): the unit leaves the selection, `{8}`, full rebuild
    // with the remaining selection.
    let mut sim = host();
    let a = spawn(&mut sim, COLONIST, 0, (10, 10));
    let b = spawn(&mut sim, COLONIST, 0, (11, 10));
    let mut sel = Selection::of(vec![a, b]);
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    th.selection_changed();
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel.ids, &ev);
    assert_eq!(bar.selected, -1, "multi-selection");
    sim.state.object_mut(a, false).unwrap().status |= status::QUARTERED;
    assert!(step_selection(&mut sel, &sim, &mut th));
    assert_eq!(sel.ids, [b]);
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    assert_eq!(
        ev.iter()
            .filter(|e| e.kind == HudEventKind::SelectionChanged)
            .count(),
        1
    );
    assert!(rebuilds(&ev).is_empty(), "after {{8}} only measure again");
    feed(&mut bar, &mut sim, &sel.ids, &ev);
    assert_eq!(bar.selected, b.0 as i32);
    assert_eq!(toolbar_model(&sim, &sel.ids, false).selection.len(), 1);
}

// QUIRK O-275: kept
#[test]
fn one_selection_change_gives_exactly_one_select_sound() {
    // One `{8}` => exactly one selection sound - that of the host (`selection_changed` -> `Voices::selection`); the
    // bar reports none on `{8}`.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let sel = [c];
    let mut voices = Voices::new(true);
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    // Like `GameSession::selection_changed`: `{8}` to the bar and the sound of the host.
    th.selection_changed();
    let host_sounds = usize::from(voices.selection(&sim.state, &sel, 0).is_some());
    let ev = th.frame(&sim, &sel, &[], 0).to_vec();
    let out = feed(&mut bar, &mut sim, &sel, &ev);
    let bar_sounds = out
        .iter()
        .filter(|o| **o == ToolbarOut::Sound(ToolbarSound::Select))
        .count();
    assert_eq!(host_sounds + bar_sounds, 1);
    assert_eq!(
        ev.iter()
            .filter(|e| e.kind == HudEventKind::SelectionChanged)
            .count(),
        1
    );
}

// QUIRK O-236, O-275: kept
#[test]
fn gate_toggle_rebuilds_fully_with_select_sound() {
    // O-236, O-275: `{0x10, 2}` rebuilds fully and reports the selection sound; the host maps it to
    // `Voices::selection`: buildings only for a new type (lock `PB+0x24`), units every time.
    let mut sim = host();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let sel = [hq];
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    let mut voices = Voices::new(true);
    th.selection_changed();
    assert!(
        voices.selection(&sim.state, &sel, 0).is_some(),
        "sound for {{8}}"
    );
    let ev = th.frame(&sim, &sel, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel, &ev);
    for open in [true, false] {
        let flags = &mut sim
            .state
            .object_mut(hq, false)
            .unwrap()
            .module_mut(3)
            .unwrap()
            .base
            .flags;
        if open {
            *flags |= bflags::TOROPEN;
        } else {
            *flags &= !bflags::TOROPEN;
        }
        let ev = th.frame(&sim, &sel, &[], 0).to_vec();
        assert_eq!(rebuilds(&ev), [2]);
        let out = feed(&mut bar, &mut sim, &sel, &ev);
        assert!(out.contains(&ToolbarOut::Sound(ToolbarSound::Select)));
        // Same building type: the lock allows no second sound.
        assert!(voices.selection(&sim.state, &sel, 0).is_none());
    }
    // Unit: no lock, every full rebuild speaks (O-275).
    let c = spawn(&mut sim, COLONIST, 0, (12, 12));
    assert!(voices.selection(&sim.state, &[c], 0).is_some());
    assert!(voices.selection(&sim.state, &[c], 0).is_some());
}

// QUIRK O-270: kept
#[test]
fn headhunter_button_changes_selection_without_rebuild() {
    // O-270: The headhunter button selects the HQ without `{8}`; the bar keeps the button set, `+0x524` and portrait.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let hq = spawn(&mut sim, HQ, 0, (14, 14));
    let mut sel = Selection::of(vec![c]);
    let mut th = ToolbarHost::new();
    let mut bar = toolbar();
    th.selection_changed();
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    feed(&mut bar, &mut sim, &sel.ids, &ev);
    let shown = bar.shown_menu();
    assert_eq!(shown, COLONIST as i32);
    // Selection without `selection_changed` (like `HeadhunterHq`).
    sel.ids = vec![hq];
    let ev = th.frame(&sim, &sel.ids, &[], 0).to_vec();
    assert!(!ev.iter().any(|e| matches!(
        e.kind,
        HudEventKind::SelectionChanged | HudEventKind::Rebuild
    )));
    feed(&mut bar, &mut sim, &sel.ids, &ev);
    assert_eq!(bar.shown_menu(), shown);
    assert_eq!(bar.selected, c.0 as i32);
    // The portrait follows `+0x524`, so still the colonist.
    assert_eq!(
        portrait_model(&sim, &bar).object.map(|o| o.guid),
        Some(COLONIST)
    );
}

#[test]
fn hover_skips_nature_module_5_and_hidden_objects() {
    // re/game-ui N.6.1 / `FUN_0047A490`: only visible objects, not module 5, not dead, not quartered; otherwise 0, no
    // fallback.
    let mut sim = host();
    sim.set_fog_shown(false);
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let hq = spawn(&mut sim, HQ, 0, (14, 14));
    // Buildings are visible through their fog memory (`FUN_00467D70`, `+0x7C4`): create the memory as on the event
    // `Created`.
    let view = sim.fog.view(&sim.state);
    let mut memories = std::mem::take(&mut sim.memories);
    memories.on_created(&sim.state, &sim.rules.map, &view, 0, hq);
    sim.memories = memories;
    sim.update_memories();
    assert_eq!(hover_target(&sim, None), 0);
    assert_eq!(hover_target(&sim, Some(c)), c.0);
    assert_eq!(hover_target(&sim, Some(hq)), hq.0, "buildings yes");
    // Module 5 NATURE (e.g. a secret passage building): no.
    let o = sim.state.object_mut(hq, false).unwrap();
    let mut nature = o.module(3).unwrap().clone();
    nature.base.index = 5;
    o.modules[5] = Some(nature);
    assert_eq!(hover_target(&sim, Some(hq)), 0, "module 5");
    sim.state.object_mut(c, false).unwrap().status |= status::QUARTERED;
    assert_eq!(hover_target(&sim, Some(c)), 0, "occupant");
    sim.state.object_mut(c, false).unwrap().status = status::DIED;
    assert_eq!(hover_target(&sim, Some(c)), 0, "dead");
    // Invisible: foreign unit on an unseen tile with fog.
    let mut sim = crate::game::sim_host::plain_testkit::host();
    let enemy = spawn(&mut sim, COLONIST, 1, (2, 2));
    sim.set_fog_shown(true);
    assert!(!sim.fog_selectable(sim.state.object(enemy, false).unwrap()));
    assert_eq!(hover_target(&sim, Some(enemy)), 0, "in the fog");
    sim.set_fog_shown(false);
    assert_eq!(hover_target(&sim, Some(enemy)), enemy.0);
}

#[test]
fn portrait_values_read_base_max_cur_and_tracked() {
    // `0x00456B70`/`0x00455A30`: object `+0x524`, otherwise `+0x48`; values in fixed order with base value, maximum,
    // current value and "tracked"; capacity 23 tracked = occupancy/maximum (`0x2217`), negative dropped, untracked
    // without a query.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let hq = spawn(&mut sim, HQ, 0, (14, 14));
    let pm = portrait_model_for(&sim, hq.0 as i32, c.0);
    let o = pm.object.expect("HQ");
    assert_eq!(o.guid, HQ);
    let attrs: Vec<u32> = o.values.iter().map(|v| v.attribute).collect();
    assert_eq!(
        attrs,
        [14, 23],
        "hit points and capacity, in the order of VALUE_ORDER"
    );
    let hp = o.values[0];
    assert_eq!(
        (hp.base, hp.max, hp.cur, hp.tracked),
        (4000, 4000, 4000, true)
    );
    let cap = o.values[1];
    assert_eq!((cap.base, cap.max, cap.cur, cap.tracked), (5, 5, 0, true));
    quarter_mut(&mut sim, hq).cur = 2;
    assert_eq!(
        portrait_model_for(&sim, hq.0 as i32, 0)
            .object
            .unwrap()
            .values[1]
            .cur,
        2
    );
    // Without an enabled quarters module `0x2217` answers -1: the tracked capacity is dropped ...
    sim.state
        .object_mut(hq, false)
        .unwrap()
        .module_mut(23)
        .unwrap()
        .base
        .enabled = false;
    let attrs: Vec<u32> = portrait_model_for(&sim, hq.0 as i32, 0)
        .object
        .unwrap()
        .values
        .iter()
        .map(|v| v.attribute)
        .collect();
    assert_eq!(attrs, [14]);
    // ... the untracked one does not ask and stays (display `base`).
    sim.state
        .object_mut(hq, false)
        .unwrap()
        .attributes
        .set_tracks_current(23, false);
    let v = portrait_model_for(&sim, hq.0 as i32, 0)
        .object
        .unwrap()
        .values;
    assert_eq!(v.iter().map(|v| v.attribute).collect::<Vec<_>>(), [14, 23]);
    assert!(!v[1].tracked);
    // Colonist: carrying capacity 17 tracked (current = load), living space 27 missing.
    let pm = portrait_model_for(&sim, -1, c.0);
    let o = pm.object.expect("hover with multi-selection");
    let carry = o.values.iter().find(|v| v.attribute == 17).expect("17");
    assert_eq!(
        (carry.base, carry.max, carry.cur, carry.tracked),
        (20, 20, 0, true)
    );
    // The case "nothing" has no object; the group nation is that of the local player.
    let none = portrait_model_for(&sim, 0, 0);
    assert!(none.object.is_none());
    assert_eq!(none.group_nation, Some(sim.state.players[0].nation));
    // Selected object gone: the hover object (`GetObjectById(+0x524)` = 0 -> `+0x48`).
    assert_eq!(
        portrait_model_for(&sim, ObjectId::NONE.0 as i32, c.0)
            .object
            .map(|o| o.guid),
        Some(COLONIST)
    );
    // Counter-upgrades in list order with color number and enmity of the source.
    let enemy = spawn(&mut sim, COLONIST, 1, (20, 20));
    sim.state.players[1].color = 3;
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid: 3354,
        rest_ms: 65_000,
        source: enemy,
    });
    let row = portrait_model_for(&sim, 0, 0).counter_upgrades[0];
    assert_eq!(
        (row.guid, row.rest_ms, row.source_color),
        (3354, 65_000, Some(3))
    );
    assert_eq!(row.enemy, sim.state.diplomacy.is_enemy(0, 1));
}

#[test]
fn resources_line_reads_costs_and_stock_from_the_sim() {
    // `[RESOURCES]` (`0x0043DEB0`) from the value set (wood 150 for the house) against the stock; lack of wood -> red.
    let mut sim = host();
    let texts = {
        let vfs = opennml_rda::Vfs::from_archives(Vec::new());
        opennml_ui::assets::Texts::load(
            &vfs,
            &opennml_formats::localisation::Localisation::default(),
        )
    };
    sim.state.players[0].resources_fx = [1000 << 12, 100 << 12, 1000 << 12, 3 << 12, 50 << 12];
    let g = SimGameText::new(&sim, &texts);
    assert_eq!(g.costs(0, HOUSE as i32), Some([0, 150, 0, 0, 0]));
    assert_eq!(g.costs(0, 4242), None, "no value set");
    assert_eq!(
        g.available(0),
        Some([1000, 100, 1000, 47]),
        "free living space R4 - R3"
    );
    assert_eq!(g.available(9), None);
    assert_eq!(g.local_slot(), 0);
    let params = TagParams {
        guid: HOUSE as i32,
        slot: 0,
        value: 1,
    };
    let line = resolve_text(
        "[RESOURCES]",
        0,
        None,
        &GameTags::new(&g, 0).with_params(params),
    );
    assert!(
        line.contains("[COLOR 0xFFFF0000][ICON INFOICONS 1]150"),
        "{line}"
    );
    // With enough wood in the base color.
    sim.state.players[0].resources_fx[1] = 200 << 12;
    let g = SimGameText::new(&sim, &texts);
    let line = resolve_text(
        "[RESOURCES]",
        0,
        None,
        &GameTags::new(&g, 0).with_params(params),
    );
    assert!(
        line.contains("[DEFAULTCOLOR][ICON INFOICONS 1]150"),
        "{line}"
    );
    // Requirements from `Rules.toolbar` (the gold mine needs the warehouse), type of the HID, formation.
    assert_eq!(g.requirements(1004), Some([1003, 0, 0, 0, 0]));
    assert_eq!(g.requirements(4242), None);
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let g = SimGameText::new(&sim, &texts).with_formation(2);
    assert_eq!(g.object_type(c.0 as i32), Some(COLONIST as i32));
    assert_eq!(g.formation(), 2);
    assert_eq!(
        g.first_selected_mana(),
        None,
        "no mana line without a selection"
    );
    let sel = [c];
    let g = SimGameText::new(&sim, &texts).with_selection(&sel);
    assert!(g.first_selected_mana().is_some());
    // Headhunter without a hunter: price 500 (no value set at slot 8), gold 1000 is enough.
    assert_eq!(g.headhunter_reward(4100), "[ICON INFOICONS 0]500");
    sim.state.players[0].resources_fx[0] = 100 << 12;
    let g = SimGameText::new(&sim, &texts);
    assert_eq!(
        g.headhunter_reward(4100),
        "[COLOR FFFF0000][ICON INFOICONS 0]500[DEFAULTCOLOR]"
    );
}

#[test]
fn toolbar_model_reads_locks_stock_and_players() {
    // Locks, object counters, targets, bars, heroes, trade, enemies and HQ of the snapshot from the simulation.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let hq = spawn(&mut sim, HQ, 0, (14, 14));
    sim.state.button_locks.insert(1001, 1);
    // 413 over the colonist, 447 over the HQ; 110 is not a `targets.ini` entry.
    let targets = active_target_buttons(&mut sim, &[c, hq]);
    assert!(targets.contains(&413) && targets.contains(&447) && !targets.contains(&110));
    let m = toolbar_model(&sim, &[c, hq], true);
    assert!(m.visible_pause);
    assert_eq!(
        m.selection.iter().map(|s| s.hid).collect::<Vec<_>>(),
        [c.0, hq.0]
    );
    let s = &m.selection[1];
    assert!(s.own && s.building && !s.site && s.production.is_some());
    assert!(m.is_disabled(1001) && !m.is_disabled(1003));
    assert_eq!(m.stock_of(HQ as i32), 1);
    assert_eq!(m.stock_of(1003), 0);
    // Target actions are set by the host ([`active_target_buttons`], test `occupant_button_415_needs_an_occupant`).
    assert!(m.targets.is_empty());
    assert!(m.bars.contains_key(&c.0) && m.bars.contains_key(&hq.0));
    assert_eq!(m.local_slot, 0);
    assert!(m.heroes.is_empty());
    // Hero list in table order.
    sim.rules.heroes = vec![COLONIST];
    let m = toolbar_model(&sim, &[], false);
    assert_eq!(m.heroes, [(c.0, COLONIST)]);
    // Production with a running and a waiting order.
    let b = building_mut(&mut sim, hq);
    b.make_guid = COLONIST;
    b.make_queue.push_back(COLONIST);
    let m = toolbar_model(&sim, &[hq], false);
    let p = m.selection[0].production.as_ref().unwrap();
    assert_eq!(p.running.map(|r| r.0), Some(COLONIST));
    assert_eq!(p.queue, [COLONIST]);
}

/// Testkit with the occupant button 415 on HQ slot 6 (kind TARGET_DO) and the `targets.ini` block 415 of the shipped
/// data (`DISABLERBUTTON` before `MY_MM` has no effect; MY = quarters flag `QUARTERED` `0x217`).
fn host_with_415() -> SimHost {
    use crate::game::economy::testkit::{DEFINES, TARGETS, TOOLBAR};
    use opennml_formats::gifcon::parse_toolbar;
    use opennml_formats::targets::{parse_target_defines, parse_targets};
    let mut sim = host();
    let toolbar = TOOLBAR.replace(
        "BUTTONID 8\nBUTTONMSG 600\n",
        "BUTTONID 6\nBUTTONMSG 415\nTARGET_DO_MSG\nBUTTONID 8\nBUTTONMSG 600\n",
    );
    sim.rules.toolbar = parse_toolbar(&toolbar);
    let targets = format!(
        "{TARGETS}BEGIN\nDISABLERBUTTON\nBUTTONMSG 415\nMY_MM\nMM_MODE MM_CHECKHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_QUARTER_QUARTERED\nMM_END\nTARGET_MM\nMM_MODE MM_RETURNFALSE\nMM_END\n\
         MAUSICONID 0\nEND\n"
    );
    let defines = format!("{DEFINES}MID_PROP_QUARTER_QUARTERED 535\n");
    sim.rules.targets = parse_targets(&targets, &parse_target_defines(&defines)).entries;
    sim
}

// QUIRK O-281: kept
#[test]
fn occupant_button_415_needs_an_occupant() {
    // `Toolbar_BuildButtonSet` (`0x004558B4...0x004558C5`): kind TARGET/TARGET_DO stays only if
    // `FUN_0047B0A0(BUTTONMSG)` finds it in `GameWorld+0x354` - the **active** list of `GameWorld_RebuildTargetActions`
    // (`0x0047D6D0`): entries of `targets.ini` whose MY filter a selected object meets. 415 demands occupants (flag
    // `QUARTERED`); the HQ with 0/5 shows slot 6 empty (reference run of the original).
    let mut sim = host_with_415();
    let hq = spawn(&mut sim, HQ, 0, (10, 10));
    let data = data(&[415, 1100, 2806, 600]);
    let slot6 = |sim: &mut SimHost| {
        let targets = active_target_buttons(sim, &[hq]);
        let mut m = toolbar_model(sim, &[hq], false);
        assert!(
            m.targets.is_empty(),
            "the snapshot leaves the active list to the host"
        );
        m.targets = targets.clone();
        // Submenu path (`BuildButtonSet(menu, 1, 0, 0)`) without the automatic occupant view: the row itself.
        let set =
            opennml_ui::hud::toolbar::set::build_set(&m, HQ as i32, false, &data, SCREEN, None);
        let row = set.slots[6].as_ref().map(|r| r.msg);
        // Full rebuild: the automatic occupant view turns 415 into the button 416 (O-266).
        let set =
            opennml_ui::hud::toolbar::set::build_set(&m, HQ as i32, true, &data, SCREEN, None);
        (targets, row, set.slots[6].as_ref().map(|r| r.msg))
    };
    let (targets, row, slot) = slot6(&mut sim);
    assert_eq!(
        targets,
        BTreeSet::from([447]),
        "only the rally point (PROP_BUILDING)"
    );
    assert_eq!((row, slot), (None, None));
    // An occupant through the path of the simulation (`PropertyQuarter_AcceptUnit` sets `QUARTERED`).
    let c = spawn(&mut sim, COLONIST, 0, (12, 12));
    let rules = sim.rules.clone();
    let accepted = opennml_sim::module::quarter::accept_unit(
        &mut opennml_sim::sim::Sim::new(&mut sim.state, &rules),
        hq,
        c,
    );
    assert_eq!(accepted, 0);
    let (targets, row, slot) = slot6(&mut sim);
    assert_eq!(targets, BTreeSet::from([415, 447]));
    assert_eq!((row, slot), (Some(415), Some(416)));
    // The colonist alone: Chop wood, Build, Repair, Quarter, Go to (file order does not matter, set).
    let c2 = spawn(&mut sim, COLONIST, 0, (20, 20));
    assert_eq!(
        active_target_buttons(&mut sim, &[c2]),
        BTreeSet::from([400, 412, 413, 414, 448])
    );
    // Mixed selection: one matching object per entry is enough.
    assert_eq!(
        active_target_buttons(&mut sim, &[c2, hq]),
        BTreeSet::from([400, 412, 413, 414, 415, 447, 448])
    );
    // Empty selection: nothing active.
    assert!(active_target_buttons(&mut sim, &[]).is_empty());
}

// QUIRK O-271, O-274: kept
#[test]
fn hero_row_is_rebuilt_on_creation_and_kept_for_a_dead_hero() {
    // O-271, O-274 (Ghidra `0x004864A2`, `0x00481BDB`, `0x00532C34`): initial state = `heroes_of`; creating an object
    // of the local slot rebuilds the hero row; destroying a shown hero alone leaves it standing; a gained hero does not
    // rebuild it.
    let mut sim = host();
    sim.rules.heroes = vec![COLONIST];
    let h1 = spawn(&mut sim, COLONIST, 0, (10, 10));
    let mut th = ToolbarHost::new();
    let ev = th.frame(&sim, &[], &[], 0).to_vec();
    let heroes = |ev: &[HudEvent]| -> Option<Vec<i32>> {
        let i = ev.iter().position(|e| e.kind == HudEventKind::HeroClear)?;
        Some(
            ev[i + 1..]
                .iter()
                .take_while(|e| e.kind == HudEventKind::HeroOn)
                .map(|e| e.args[1])
                .collect(),
        )
    };
    assert_eq!(heroes(&ev), Some(vec![h1.0 as i32]), "initial state");
    assert_eq!(
        heroes(th.frame(&sim, &[], &[], 0)),
        None,
        "nothing without a cause"
    );
    let h2 = spawn(&mut sim, COLONIST, 0, (11, 10));
    let created = [SimEvent::Created { id: h2 }];
    assert_eq!(
        heroes(th.frame(&sim, &[], &created, 0)),
        Some(vec![h1.0 as i32, h2.0 as i32])
    );
    // A shown hero is destroyed (O-271): row stays, its button too.
    let destroyed = [SimEvent::Destroyed { id: h1 }];
    assert_eq!(heroes(th.frame(&sim, &[], &destroyed, 0)), None);
    // A hero comes to the local slot through a change of owner (O-274): nothing.
    let other = spawn(&mut sim, COLONIST, 1, (20, 20));
    let gained = [SimEvent::OwnerChanged {
        old: other,
        new: other,
    }];
    assert_eq!(heroes(th.frame(&sim, &[], &gained, 0)), None);
    // A foreign object appears: nothing; an own one: rebuild from the state.
    assert_eq!(
        heroes(th.frame(&sim, &[], &[SimEvent::Created { id: other }], 0)),
        None
    );
    let c = spawn(&mut sim, HQ, 0, (30, 30));
    assert!(heroes(th.frame(&sim, &[], &[SimEvent::Created { id: c }], 0)).is_some());
    assert_eq!(th.heroes(), [h1.0, h2.0], "h1 lives on in the test");
    // After loading: the initial state again.
    th.reset();
    assert!(heroes(th.frame(&sim, &[], &[], 0)).is_some());
}

#[test]
fn headhunter_row_follows_the_hunter_table() {
    // `FUN_00479720`: `{0x32}` and `{0x31, 1, GUID}` per present headhunter on every change of `GameWorld+0x4DC`.
    let mut sim = host();
    let mut th = ToolbarHost::new();
    let first = th.frame(&sim, &[], &[], 0).to_vec();
    assert!(
        first
            .iter()
            .any(|e| e.kind == HudEventKind::HeadhunterClear)
    );
    assert!(
        !th.frame(&sim, &[], &[], 0)
            .iter()
            .any(|e| e.kind == HudEventKind::HeadhunterClear)
    );
    let hunter = spawn(&mut sim, COLONIST, 8, (5, 5));
    sim.state.headhunters[1] = Some(hunter);
    let ev = kinds(th.frame(&sim, &[], &[], 0));
    let i = ev
        .iter()
        .position(|e| e.0 == HudEventKind::HeadhunterClear)
        .unwrap();
    assert_eq!(
        ev[i + 1],
        (HudEventKind::HeadhunterOn, vec![1, COLONIST as i32])
    );
    assert_eq!(
        toolbar_model(&sim, &[], false).hunters_present,
        [false, true, false]
    );
}

#[test]
fn hover_events_come_last_as_zero_then_the_hit() {
    // `FUN_0047A490` twice per frame: first `{0x29, 0}`, then the hit.
    let sim = host();
    let mut th = ToolbarHost::new();
    let ev = kinds(th.frame(&sim, &[], &[], 77));
    let n = ev.len();
    assert_eq!(ev[n - 2], (HudEventKind::Hover, vec![0]));
    assert_eq!(ev[n - 1], (HudEventKind::Hover, vec![77]));
}

#[test]
fn build_cursor_frame_is_snapped_with_the_cursor_rotation() {
    // Build cursor gets the snapped point (`BuildCursor_GetSnappedPosition`) and its validity.
    let mut sim = host();
    let c = spawn(&mut sim, COLONIST, 0, (10, 10));
    let mut cursor = BuildCursor::new(HOUSE);
    cursor.rotation = 0.0;
    let ground = Vec3::new(1300.0, 0.0, 1310.0);
    let f = cursor_frame(
        &mut sim,
        &Cursor::Build(cursor.clone()),
        Some(ground),
        [5, 6],
        &[c],
        None,
    );
    let preview = sim.placement_preview_rot(HOUSE, ground, 0.0, &[c]).unwrap();
    assert_eq!(f.world, Some(preview.pos));
    assert_ne!(f.world, Some(ground.to_array()), "snapped");
    assert_eq!(f.valid, preview.valid);
    assert_eq!((f.mouse, f.tile), ([5, 6], [128, 128]));
    // Without a ground point nothing is valid.
    let f = cursor_frame(&mut sim, &Cursor::Build(cursor), None, [0, 0], &[c], None);
    assert_eq!((f.world, f.valid), (None, false));
}

/// The lock of the bar (`ToolbarModel::locks_for` from the snapshot `toolbar_model`, `0x0044F250` step 1) is, for
/// **every** button of the `toolbar.ini` of a real match, that of the simulation (`validate::button_locked_in`,
/// `Toolbar_IsButtonLocked` `0x0044AF60`), per slot and with a running counter-upgrade. The only intended gap:
/// `REQUIRED_GUID_NATION` is checked only by `Toolbar_IsButtonLocked` (key shortcut), not by drawing (O-230); the
/// shipped data does not use the list, which the test also checks.
// QUIRK O-230: kept
#[test]
fn toolbar_locks_match_the_sim_for_every_corpus_button() {
    use opennml_sim::command::validate::{
        CHECKED_KINDS, button_locked_in, counter_blocked_in, missing_required_in,
    };
    use opennml_ui::hud::toolbar::button::{ButtonSpec, SheetRef};
    // Both sides check the same kinds.
    let kinds = |k: &[u32]| k.iter().copied().collect::<BTreeSet<u32>>();
    assert_eq!(
        kinds(&CHECKED_KINDS),
        kinds(&opennml_ui::hud::toolbar::model::CHECKED_KINDS)
    );
    let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
    let level = "data/levels/multiplayer/[2players]-tintebariver.lvl";
    let scene = opennml_world::level::load_level(&vfs, level).unwrap();
    let mut sim = crate::game::sim_host::start_sim(
        &vfs,
        &scene,
        10,
        crate::game::sim_host::testkit::default_setup,
    )
    .unwrap()
    .expect("match");
    let buttons: Vec<opennml_formats::gifcon::ToolbarButton> = sim
        .rules
        .toolbar
        .menus
        .values()
        .flatten()
        .cloned()
        .collect();
    assert!(
        buttons.len() > 100,
        "buttons of `toolbar.ini`: {}",
        buttons.len()
    );
    assert!(
        buttons.iter().all(|b| b.required_nation[0] == 0),
        "REQUIRED_GUID_NATION in the shipped data"
    );
    let spec = |b: &opennml_formats::gifcon::ToolbarButton| ButtonSpec {
        msg: b.msg,
        flags: b.flags,
        required: b.required,
        rect: [0, 0, 50, 50],
        base: (SheetRef::Icons(0), 0),
        icon: (SheetRef::Icons(0), 0),
        scale: 1.0,
        progress: 1.0,
        object: None,
    };
    let mut counts = [0usize; 2];
    let mut compare = |sim: &SimHost, what: &str| {
        let model = toolbar_model(sim, &[], false);
        let slot = i32::from(sim.active_player);
        for b in &buttons {
            let locks = model.locks_for(&spec(b));
            let bar = locks.missing_required || locks.counter_running;
            let locked = button_locked_in(&sim.state, &sim.rules, slot, b);
            assert_eq!(bar, locked, "{what}: button {} kind {:#x}", b.msg, b.kind());
            // The parts separately: lack only for checked kinds, counter-upgrade only for COUNTERUPGRADE.
            assert_eq!(
                locks.missing_required,
                CHECKED_KINDS.contains(&b.kind())
                    && missing_required_in(&sim.state, &sim.rules, slot, b),
                "{what}: lack {}",
                b.msg
            );
            assert_eq!(
                locks.counter_running,
                counter_blocked_in(&sim.state, slot, b),
                "{what}: counter-upgrade {}",
                b.msg
            );
            counts[usize::from(bar)] += 1;
        }
    };
    for p in 0..2 {
        sim.active_player = p;
        compare(&sim, &format!("Start, slot {p}"));
    }
    // A running counter-upgrade of slot 0 (source its headquarters): locks all COUNTERUPGRADE buttons there, at slot 1
    // only the same GUID.
    let counter = buttons
        .iter()
        .find(|b| b.kind() == opennml_formats::gifcon::button_kind::COUNTERUPGRADE)
        .expect("COUNTERUPGRADE in the shipped data");
    let source = sim.state.tables[0]
        .objects
        .iter()
        .flatten()
        .next()
        .map(|o| o.id)
        .unwrap();
    sim.state.counter_upgrades.push(CounterUpgrade {
        guid: counter.msg as u32,
        rest_ms: 60_000,
        source,
    });
    for p in 0..2 {
        sim.active_player = p;
        compare(&sim, &format!("Counter-upgrade, slot {p}"));
    }
    // After a minute of game time (stocks change).
    sim.state.counter_upgrades.clear();
    for _ in 0..1_200 {
        sim.step_once();
    }
    for p in 0..2 {
        sim.active_player = p;
        compare(&sim, &format!("Tick 1200, slot {p}"));
    }
    assert!(
        counts[0] > 0 && counts[1] > 0,
        "locked and free buttons: {counts:?}"
    );
}

/// `REQUIRED_GUID_NATION` locks only `Toolbar_IsButtonLocked` (`0x0044AF60`, key shortcut and the command check of
/// the simulation), not the drawn button (`0x0044F250` reads only `REQUIRED_GUID`, O-230).
// QUIRK O-230: kept
#[test]
fn nation_requirement_locks_only_the_sim_check() {
    use opennml_formats::gifcon::parse_toolbar;
    use opennml_sim::command::validate::button_locked_in;
    use opennml_ui::hud::toolbar::button::{ButtonSpec, SheetRef};
    let mut sim = host();
    sim.rules.toolbar = parse_toolbar(
        "GUID 1100
BUTTONID 0
BUTTONMSG 1001
BUILD_MSG
REQUIRED_GUID_NATION 5
ENDGUID
",
    );
    let b = sim.rules.toolbar.menus[&1100][0].clone();
    assert_eq!(b.required_nation[0], 5);
    assert!(
        button_locked_in(&sim.state, &sim.rules, 0, &b),
        "simulation"
    );
    let model = toolbar_model(&sim, &[], false);
    let locks = model.locks_for(&ButtonSpec {
        msg: b.msg,
        flags: b.flags,
        required: b.required,
        rect: [0, 0, 50, 50],
        base: (SheetRef::Icons(0), 0),
        icon: (SheetRef::Icons(0), 0),
        scale: 1.0,
        progress: 1.0,
        object: None,
    });
    assert!(!locks.missing_required && !locks.counter_running, "bar");
}
