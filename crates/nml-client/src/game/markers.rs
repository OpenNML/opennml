//! Ground markers of the viewer as in the original (re/renderer, section "Ground markers"):
//! selection, target marker of the right-click and object under the mouse (`FUN_0047A490`, mask
//! `0x8000000`). Presentation only: simulation and checksum stay untouched. Without winit and GPU
//! (testable); drawing goes through [`crate::game::map::MapView::set_ground_markers`].

use nml_sim::command::CommandKind;
use nml_sim::id::ObjectId;
use nml_sim::object::{GameObject, status};
use nml_sim::state::SimState;

use crate::game::economy::is_ground_action;

/// Duration of the target marker: deleted as soon as `start + 300 < now` (`0x0047FC70`).
pub const TARGET_HIGHLIGHT_MS: f32 = 300.0;

/// Base modules without a marker under the mouse: building (3), nature (5), wall piece (`0x29`,
/// pattern) and tower (`0x32`) (`FUN_0047A490`, `GameObject_HasModule(n, 0)`).
const NO_HOVER_MODULES: [usize; 4] = [3, 5, 0x29, 0x32];

/// Target marker (`GameWorld+0x318` object, `+0x9B4` start time, `GameWorld_SetTargetHighlight`
/// `0x00475F70`).
///
/// Deviation: the original measures the 300 ms in real time (`GetTickCount`, also during the pause);
/// the viewer uses the virtual clock (`Clock`) so that the control channel and check images are
/// repeatable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetHighlight {
    pub id: ObjectId,
    pub start_ms: f32,
}

impl TargetHighlight {
    /// Expired (`start + 300 < now`, strict `<`).
    pub fn expired(&self, now_ms: f32) -> bool {
        self.start_ms + TARGET_HIGHLIGHT_MS < now_ms
    }

    /// Remaining time until expiry (0 at the last valid instant).
    pub fn remaining_ms(&self, now_ms: f32) -> f32 {
        (self.start_ms + TARGET_HIGHLIGHT_MS - now_ms).max(0.0)
    }
}

/// Object of the target marker of a dispatched target action (`Input_OnTargetButtonUp`
/// `0x00457C50`: action found and object target, bit `0x578 & 1`): `issued` as
/// `SimHost::issue_target_action` returns it; ground actions (also wrapped) mark nothing.
pub fn highlight_target(
    issued: Option<&(i32, CommandKind)>,
    target: Option<ObjectId>,
) -> Option<ObjectId> {
    let (_, kind) = issued?;
    let target = target?;
    let inner = match kind {
        CommandKind::Queue { inner, .. } => inner.as_ref(),
        other => other,
    };
    (!is_ground_action(inner, Some(target))).then_some(target)
}

/// After a target action: an object target sets or replaces the marker (one pointer), a
/// ground target changes nothing (a running marker runs out).
pub fn on_target_action(
    highlight: &mut Option<TargetHighlight>,
    issued: Option<&(i32, CommandKind)>,
    target: Option<ObjectId>,
    now_ms: f32,
) {
    if let Some(id) = highlight_target(issued, target) {
        *highlight = Some(TargetHighlight {
            id,
            start_ms: now_ms,
        });
    }
}

/// `sim_action` (control channel) like a right-click: target action of the active player
/// ([`crate::game::sim_host::SimHost::control_action_issued`]), then the target marker at time
/// `now_ms` as after a click ([`on_target_action`]).
#[allow(clippy::too_many_arguments)]
pub fn control_action_marked(
    host: &mut crate::game::sim_host::SimHost,
    highlight: &mut Option<TargetHighlight>,
    units: &[ObjectId],
    target: Option<ObjectId>,
    to: Option<[f32; 2]>,
    queue: Option<bool>,
    order: crate::game::formation_order::FormationOrder,
    now_ms: f32,
) -> Result<nml_control::SimActionResult, String> {
    let (result, issued) = host.control_action_issued(units, target, to, queue, order)?;
    on_target_action(highlight, Some(&issued), target, now_ms);
    Ok(result)
}

/// The target marker while it applies: not expired (`0x0047FC70`) and the object still
/// present (`GameWorld_DestroyObject` `0x00481BB0` deletes it). Dead objects that are still present
/// stay marked.
pub fn live_highlight(
    highlight: Option<TargetHighlight>,
    state: &SimState,
    now_ms: f32,
) -> Option<TargetHighlight> {
    highlight.filter(|h| !h.expired(now_ms) && state.object(h.id, false).is_some())
}

/// Once per frame before drawing: delete a marker that no longer applies ([`live_highlight`]).
pub fn retain_highlight(highlight: &mut Option<TargetHighlight>, state: &SimState, now_ms: f32) {
    *highlight = live_highlight(*highlight, state, now_ms);
}

/// Target marker in the state snapshot (`sim.target_marker`): as in the next frame, so even without a
/// frame in between only while it applies ([`live_highlight`]).
pub fn target_marker_state(
    highlight: Option<TargetHighlight>,
    state: &SimState,
    now_ms: f32,
) -> Option<nml_control::SimTargetMarker> {
    live_highlight(highlight, state, now_ms).map(|h| nml_control::SimTargetMarker {
        id: h.id.0,
        remaining_ms: h.remaining_ms(now_ms),
    })
}

/// `FUN_00466720`: quartered objects draw nothing.
fn drawable(object: &GameObject) -> bool {
    object.status & status::QUARTERED == 0
}

/// `GameObject_GetTargetObject` (`0x00466910`): `+0x2CC`, except 0 and -1.
fn target_object(state: &SimState, object: &GameObject) -> Option<ObjectId> {
    let t = object.target;
    if t == ObjectId::NONE || t.0 == 0 {
        return None;
    }
    state.object(t, false).map(|o| o.id)
}

/// The object under the mouse (click target `0x0047A210`, [`crate::game::selection::click_target`]: never
/// trees or nature) gets the ring (`FUN_0047A490`): visible (`0x00467D70`), not dead
/// (`0x40000`), not quartered, none of the modules building, nature, wall piece, tower - so only
/// units and resources (module 6). The object under the mouse must pass the fog check
/// `0x00467D70` (caller).
pub fn hover_marked(object: &GameObject) -> bool {
    object.status & (status::DIED | status::QUARTERED) == 0
        && NO_HOVER_MODULES
            .iter()
            .all(|&m| !object.has_module(m, false))
}

/// Objects with a ground marker in drawing order as in `FUN_0047A490` with mask `0x8000000`:
/// the selection (`GameWorld+0x374`, visible ones only; riding or carried, state
/// `0x2000`/`0x8000`: the ring sits on the carrier `+0x2CC`, the object's own is omitted), then the
/// target marker (without a visibility check), then the object under the mouse ([`hover_marked`], visible).
/// An object can appear several times (the original then draws it several times). `visible` is the
/// fog check `0x00467D70` ([`crate::game::sim_host::SimHost::fog_selectable`]).
pub fn ground_marker_objects(
    state: &SimState,
    selection: &[ObjectId],
    highlight: Option<&TargetHighlight>,
    hovered: Option<ObjectId>,
    visible: impl Fn(&GameObject) -> bool,
) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let push = |id: ObjectId, out: &mut Vec<ObjectId>| {
        if state.object(id, false).is_some_and(drawable) {
            out.push(id);
        }
    };
    for &id in selection {
        let Some(o) = state.object(id, false).filter(|o| visible(o)) else {
            continue;
        };
        if o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0 {
            if let Some(t) = target_object(state, o) {
                push(t, &mut out);
            }
            continue;
        }
        push(id, &mut out);
    }
    if let Some(h) = highlight {
        push(h.id, &mut out);
    }
    if let Some(id) = hovered
        && state
            .object(id, false)
            .is_some_and(|o| hover_marked(o) && visible(o))
    {
        push(id, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{COLONIST, GOLD, HQ, TREE, host, spawn};
    use crate::game::selection::RightClickTarget::Object;
    use glam::Vec3;
    use nml_world::markers::marker_color;

    fn ground(t: (i32, i32)) -> Vec3 {
        Vec3::new(t.0 as f32 * 128.0 + 64.0, 0.0, t.1 as f32 * 128.0 + 64.0)
    }

    #[test]
    fn right_click_on_an_object_sets_the_highlight_ground_does_not() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let sel = [worker];
        let mut h = None;
        // Chopping wood at the tree: marks the tree.
        let issued = host.issue_target_action(&sel, Some(Object(tree)), ground((14, 10)), None);
        on_target_action(&mut h, issued.as_ref(), Some(tree), 100.0);
        assert_eq!(
            h,
            Some(TargetHighlight {
                id: tree,
                start_ms: 100.0
            })
        );
        // Ground ("Go to"): sets and deletes nothing.
        let issued = host.issue_target_action(&sel, None, ground((5, 5)), None);
        assert!(issued.is_some());
        on_target_action(&mut h, issued.as_ref(), None, 150.0);
        assert_eq!(h.map(|h| h.id), Some(tree));
        // No action: nothing.
        on_target_action(&mut h, None, Some(hq), 160.0);
        assert_eq!(h.map(|h| h.id), Some(tree));
        // A new object target replaces it (also wrapped, with Shift).
        let issued =
            host.issue_target_action(&sel, Some(Object(hq)), ground((30, 30)), Some(false));
        on_target_action(&mut h, issued.as_ref(), Some(hq), 200.0);
        assert_eq!(
            h,
            Some(TargetHighlight {
                id: hq,
                start_ms: 200.0
            })
        );
        // "Go to" with an object under the mouse (no object action): no object target.
        let move_kind = CommandKind::Move {
            units: vec![worker],
            x: 0,
            z: 0,
            target: -1,
            formation: 0,
            direction: 0,
            spread: nml_sim::command::NO_SPREAD,
            flags: 0,
        };
        assert_eq!(highlight_target(Some(&(448, move_kind)), Some(tree)), None);
    }

    #[test]
    fn sim_action_on_a_tree_marks_it_for_300_ms_without_touching_the_simulation() {
        let run = |mark: bool| {
            let mut host = host();
            let worker = spawn(&mut host, COLONIST, 0, (10, 10));
            let tree = spawn(&mut host, TREE, 9, (14, 10));
            let mut h = None;
            let result = if mark {
                control_action_marked(
                    &mut host,
                    &mut h,
                    &[worker],
                    Some(tree),
                    None,
                    None,
                    Default::default(),
                    1000.0,
                )
            } else {
                host.control_action(&[worker], Some(tree), None, None)
            }
            .unwrap();
            assert_eq!(result.command, "ChopWood");
            if mark {
                let state = |now| target_marker_state(h, &host.state, now);
                assert_eq!(
                    state(1000.0).map(|m| (m.id, m.remaining_ms)),
                    Some((tree.0, 300.0))
                );
                assert_eq!(state(1300.0).map(|m| m.remaining_ms), Some(0.0));
                assert_eq!(state(1316.0), None);
                // A ground target ("Go to") does not change the marker.
                let ground = control_action_marked(
                    &mut host,
                    &mut h,
                    &[worker],
                    None,
                    Some([5.0 * 128.0, 5.0 * 128.0]),
                    None,
                    Default::default(),
                    1100.0,
                );
                assert!(ground.is_ok());
                assert_eq!(h.map(|h| (h.id, h.start_ms)), Some((tree, 1000.0)));
            } else {
                host.control_action(&[worker], None, Some([5.0 * 128.0, 5.0 * 128.0]), None)
                    .unwrap();
            }
            for _ in 0..10 {
                host.step_once();
            }
            host.checksum()
        };
        assert_eq!(run(true), run(false));
    }

    #[test]
    fn highlight_lasts_300_ms_and_ends_with_the_object() {
        let mut host = host();
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let mut h = Some(TargetHighlight {
            id: tree,
            start_ms: 1000.0,
        });
        retain_highlight(&mut h, &host.state, 1300.0);
        assert_eq!(h.map(|h| h.remaining_ms(1300.0)), Some(0.0));
        retain_highlight(&mut h, &host.state, 1301.0);
        assert_eq!(h, None);
        // Destroyed (no longer in the table): deleted immediately.
        let mut h = Some(TargetHighlight {
            id: tree,
            start_ms: 1000.0,
        });
        assert_eq!(h.map(|h| h.remaining_ms(1100.0)), Some(200.0));
        let gone = ObjectId::new(tree.index() as u16, tree.slot(), tree.generation() + 1);
        let mut g = Some(TargetHighlight {
            id: gone,
            start_ms: 1000.0,
        });
        retain_highlight(&mut g, &host.state, 1001.0);
        assert_eq!(g, None);
        retain_highlight(&mut h, &host.state, 1001.0);
        assert!(h.is_some());
    }

    #[test]
    fn the_state_reports_the_highlight_only_while_its_object_exists() {
        let mut host = host();
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let h = Some(TargetHighlight {
            id: tree,
            start_ms: 1000.0,
        });
        assert_eq!(
            target_marker_state(h, &host.state, 1100.0),
            Some(nml_control::SimTargetMarker {
                id: tree.0,
                remaining_ms: 200.0
            })
        );
        assert_eq!(target_marker_state(h, &host.state, 1301.0), None);
        // Destroyed, without a frame in between (the marker itself still exists): not reported.
        nml_sim::world::destroy_object(&mut host.sim_context(), tree);
        assert!(host.state.object(tree, false).is_none());
        assert_eq!(target_marker_state(h, &host.state, 1100.0), None);
    }

    /// Selection and object under the mouse only with a passed fog check (`0x00467D70`), the
    /// target marker without.
    #[test]
    fn rings_need_the_fog_check_but_the_target_highlight_does_not() {
        let mut host = host();
        host.state.fog = nml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let gold = spawn(&mut host, GOLD, 9, (20, 20));
        let target = TargetHighlight {
            id: gold,
            start_ms: 0.0,
        };
        let marked = |host: &crate::game::sim_host::SimHost| {
            ground_marker_objects(&host.state, &[worker], Some(&target), Some(gold), |o| {
                host.fog_selectable(o)
            })
        };
        assert_eq!(marked(&host), [gold], "only the target marker");
        host.state.fog.seen.or(10, 10, 1);
        host.state.fog.seen.or(20, 20, 1);
        assert_eq!(marked(&host), [worker, gold, gold]);
    }

    #[test]
    fn hover_rings_units_and_resources_only() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let gold = spawn(&mut host, GOLD, 9, (20, 20));
        // Object under the mouse = click target (`0x0047A210`) of a ray that hits only `id`.
        let hovered = |host: &crate::game::sim_host::SimHost, id| {
            crate::game::selection::click_target(&host.state, &[(id, ())], |_, _, _| true)
        };
        let marked = |host: &crate::game::sim_host::SimHost, id| {
            ground_marker_objects(&host.state, &[], None, hovered(host, id), |_| true)
        };
        assert_eq!(marked(&host, worker), [worker]);
        assert_eq!(marked(&host, gold), [gold]);
        // Trees are never a click target: no ring.
        assert_eq!(hovered(&host, tree), None);
        assert!(marked(&host, tree).is_empty());
        // Buildings are a click target but get no ring; as a target marker they do (no
        // exception for target kinds).
        assert_eq!(hovered(&host, hq), Some(hq));
        assert!(marked(&host, hq).is_empty());
        let target = TargetHighlight {
            id: hq,
            start_ms: 0.0,
        };
        assert_eq!(
            ground_marker_objects(&host.state, &[], Some(&target), None, |_| true),
            [hq]
        );
        // Nature, wall piece, tower: like buildings (module in its place, on the resource).
        for m in [5, 0x29, 0x32] {
            let module = host.state.object(hq, false).unwrap().modules[3].clone();
            let o = host.state.object_mut(gold, false).unwrap();
            let saved = std::mem::replace(&mut o.modules[m], module);
            assert!(marked(&host, gold).is_empty(), "module {m:#x}");
            host.state.object_mut(gold, false).unwrap().modules[m] = saved;
        }
        // Dead and quartered objects get no ring.
        for bit in [status::DIED, status::QUARTERED] {
            host.state.object_mut(worker, false).unwrap().status |= bit;
            assert!(marked(&host, worker).is_empty());
            host.state.object_mut(worker, false).unwrap().status &= !bit;
        }
    }

    #[test]
    fn selection_rings_sit_on_the_carrier_and_never_on_quartered_objects() {
        let mut host = host();
        let rider = spawn(&mut host, COLONIST, 0, (10, 10));
        let horse = spawn(&mut host, COLONIST, 0, (11, 10));
        let other = spawn(&mut host, COLONIST, 0, (12, 10));
        let o = host.state.object_mut(rider, false).unwrap();
        o.status |= status::ISMOUNTED;
        o.target = horse;
        let sel = [rider, other];
        assert_eq!(
            ground_marker_objects(&host.state, &sel, None, None, |_| true),
            [horse, other]
        );
        // Carried without a carrier: no ring.
        let o = host.state.object_mut(rider, false).unwrap();
        o.status = status::ISMOUNTEDONBOAT;
        o.target = ObjectId::NONE;
        assert_eq!(
            ground_marker_objects(&host.state, &sel, None, None, |_| true),
            [other]
        );
        // Quartered: neither selection nor target nor mouse.
        host.state.object_mut(other, false).unwrap().status |= status::QUARTERED;
        let target = TargetHighlight {
            id: other,
            start_ms: 0.0,
        };
        assert!(
            ground_marker_objects(&host.state, &sel, Some(&target), Some(other), |_| true)
                .is_empty()
        );
    }

    #[test]
    fn marker_color_follows_the_color_index_of_the_owner() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let enemy = spawn(&mut host, COLONIST, 3, (12, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let color = |id| marker_color(host.state.object(id, false).unwrap().color);
        assert_eq!(color(worker), [0xFF, 0xCE, 0x0D, 0xFF]);
        assert_eq!(color(enemy), [0xC4, 0x18, 0x00, 0xFF]);
        assert_eq!(color(tree), [0xAC, 0xAC, 0xAC, 0xFF]);
    }

    #[test]
    fn markers_leave_the_simulation_checksum_alone() {
        let run = |mark: bool| {
            let mut host = host();
            let worker = spawn(&mut host, COLONIST, 0, (10, 10));
            let tree = spawn(&mut host, TREE, 9, (14, 10));
            let issued =
                host.issue_target_action(&[worker], Some(Object(tree)), ground((14, 10)), None);
            let mut h = None;
            for step in 0..20 {
                if mark {
                    on_target_action(&mut h, issued.as_ref(), Some(tree), 0.0);
                    retain_highlight(&mut h, &host.state, step as f32 * 50.0);
                    ground_marker_objects(&host.state, &[worker], h.as_ref(), Some(tree), |_| true);
                }
                host.step_once();
            }
            host.checksum()
        };
        assert_eq!(run(true), run(false));
    }
}
