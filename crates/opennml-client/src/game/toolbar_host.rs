//! Snapshot of the match for the command bar: the model per frame ([`toolbar_model`], [`portrait_model`]), the
//! signature of the selected objects for the rebuild ([`Signature`], [`rebuild_kind`]), the object under the mouse
//! ([`hover_target`]), the events of the game host to the bar ([`ToolbarHost`]) and the frame of the world queries
//! for an open cursor ([`cursor_frame`]). The intents of the bar become commands in `commands` ([`execute`]).
//!
//! The snapshot only reads: no commands, no state of the simulation, no checksum effect. The bar itself
//! (`opennml_ui::hud::toolbar`) does not know `opennml-sim`; what it needs from the match is in these snapshots.
//!
//! # Events of the game host to the bar
//!
//! The original sends the events from the simulation (`GameObject_NotifyUiIfSelected` `0x00469380` at 45 places,
//! re/game-ui N.2) and from the selection. The reimplementation detects them per frame ([`ToolbarHost::frame`]):
//!
//! 1. `{8}` from [`ToolbarHost::selection_changed`]: click and box, double-click, the selection changes of a
//!    simulation step (quartering, death, tent handover, deselecting the locomotive) and the selection
//!    buttons of the bar. The headhunter button and SHORTCUT send none (O-270). The host plays the selection sound
//!    for `{8}` (`GameSession::selection_changed`), not the bar.
//! 2. `{0x10, n}` from the comparison of the [`Signature`] with the previous one (deviation from the original): `n`
//!    according to [`rebuild_kind`]; a change of arming sends `{0x10, 2}` and afterwards `{0x10, 0}`, like
//!    `0x004FF750`. After a `{8}` the host only re-measures (the full rebuild covers everything). The only message
//!    of the simulation for this: the cancellation of an order ([`SimEvent::ProductionCancelled`], `0x00507960` ->
//!    `{0x10, 0}`), which the signature does not tell apart from a finished order (`{0x10, 3}`).
//! 3. Hero row `{0x25}` + `{0x23, 1, HID}` per hero (`GameWorld_PostLocalPlayerObjectListEvents` `0x0047F48E`), if
//!    `GameWorld_NotifyLocalPlayerObjectChange` (`0x0047F500`) would run for the local slot: when creating an
//!    object (`GameWorld_CreateObject` `0x004864A2`), when destroying one (`GameWorld_DestroyObject` `0x00481BDB`)
//!    and when a walk unit leaves the slot (`PropertyWalk_OnOwnerChanged` `0x00532C34`, old ID, O-274).
//!    See [`ToolbarHost::frame`] on O-271.
//! 4. Headhunter row `{0x32}` + `{0x31, 1, GUID}` per existing headhunter (`FUN_00479720`), when `GameWorld+0x4DC`
//!    changes (registering and unregistering `FUN_0047B580`/`FUN_0047B5B0`) and after loading.
//! 5. `{0x29, 0}` and `{0x29, HID}` (`FUN_0047A490`, twice per frame from `GameEngine_RenderWorld`).
//!
//! 6. The UI messages of the script presentation to the bar ([`presentation_events`], distributed by
//!    `Hud::present`): lock 18/19 `{0x19, on}` (`FUN_0042D920`), cinemascope 147/148 `{0x11, on}`
//!    (`FUN_00430920`); the `{8}` of the cinemascope comes via clearing the selection after `{0x11, 1}` and is
//!    lost. With a hidden bar no `{0x10, n}` (as in the original, O-234).
//!
//! The initial state (session start, loading, restart) is sent by the first frame after [`ToolbarHost::reset`]:
//! hero row = `heroes_of`, headhunter row = existing headhunters; a loaded cinemascope hides the bar again
//! (`GameSession::restore_presentation`).

use std::collections::BTreeSet;

use glam::Vec3;
use opennml_sim::command::headhunter::NATION_HQ;
use opennml_sim::id::{ObjectId, PLAYER_SLOTS};
use opennml_sim::module::building::flags as bflags;
use opennml_sim::module::{ModuleKind, arming, attack, building, headhunter as hunter, quarter};
use opennml_sim::object::{GameObject, status};
use opennml_sim::rules::Rules;
use opennml_sim::state::{SimEvent, SimState};
use opennml_ui::hud::portrait::{
    ATTR_CAPACITY, CounterRow, PortraitModel, PortraitObject, PortraitValue, VALUE_ORDER,
};
use opennml_ui::hud::toolbar::button::ButtonBars;
use opennml_ui::hud::toolbar::set::hunt_target_types;
use opennml_ui::hud::toolbar::{
    Cursor, CursorFrame, HuntTarget, Production, SelectedObject, Toolbar, ToolbarModel,
};
use opennml_ui::hud::{HudEvent, HudEventKind};

use crate::game::sim_host::SimHost;
use crate::game::unit_bar::UnitBar;

mod commands;
pub use commands::*;
mod frame;
pub(crate) use frame::surface_point;
pub use frame::{
    ActiveTargetDecal, DEFENSE_RING_TEXTURE, TARGET_DECAL_MS, TARGET_DECAL_TEXTURE,
    target_decal_size,
};

#[cfg(test)]
mod tests;

/// Slot without a player (neutral).
const NEUTRAL_SLOT: u8 = 8;
/// Module 0 WALK.
const WALK: usize = 0;
/// Module 3 BUILDING.
const BUILDING: usize = building::INDEX;
/// Module 5 NATURE: never the object under the mouse (`FUN_0047A490`).
const NATURE: usize = 5;
/// Module `0x17` and class `0x217`: building with a rally point (`FUN_00452EB0`).
const RALLY_MODULE: usize = 0x17;
const RALLY_CLASS: u32 = 0x217;
/// Status bits of the signature (gait `0x20000`, `0x100000`, `0x800000`; re/game-ui N.2).
const SIGNATURE_STATUS: u32 = status::DIVING | status::MASKED | status::ANIM_LOCKED;
/// `ISARMED` in module 39 ARMING (`+0xC & 0x100`, `FUN_004FF510`/`FUN_004FF540`).
const ISARMED: u32 = 0x100;
/// Arrow type: variant bits 12/13 in the prop bits of the modules (`GameWorld_CmdSetArrowType` `0x00475B90`).
const ARROW_BITS: u32 = 0x3000;
/// Gate open/closed (`PropertyBuilding_OpenDoor`/`CloseDoor`).
const GATE_BITS: u32 = (bflags::TOROPEN | bflags::TORCLOSE) & 0xFFFF_FF00;
/// Occupant list `0x1817 {12}`: at most twelve IDs.
const OCCUPANT_LIST: usize = 12;
/// Bars of the selection and hero buttons (`0x0044F250`): hit points, mana, stamina, experience.
const BAR_ATTRIBUTES: [u32; 4] = [14, 15, 28, 30];
/// Color buttons of the target players `470 + color` (colors 0 ... 11, `0x006189D0`).
const COLOR_BUTTONS: std::ops::Range<u32> = 470..482;

/// Object for the HID (`GameWorld_GetObjectById(hid, 0)`, dying ones too).
fn object(state: &SimState, hid: u32) -> Option<&GameObject> {
    state.object(ObjectId(hid), false)
}

/// Building module of an object (independent of "enabled").
fn building_of(o: &GameObject) -> Option<&building::Building> {
    match &o.module(BUILDING)?.kind {
        ModuleKind::Building(b) => Some(b),
        _ => None,
    }
}

/// Quarters module of an object (independent of "enabled").
fn quarter_of(o: &GameObject) -> Option<&quarter::Quarter> {
    match &o.module(quarter::INDEX)?.kind {
        ModuleKind::Quarter(q) => Some(q),
        _ => None,
    }
}

/// Response to `0x711` (`PropertyAttack_HandleMessage`): attack mode; -1 without an enabled module 17 or for a dead
/// object.
fn unit_attack_mode(o: &GameObject) -> i32 {
    if !o.has_module(attack::INDEX, false) || o.status & status::DIED != 0 {
        return -1;
    }
    match &o.module(attack::INDEX).map(|m| &m.kind) {
        Some(ModuleKind::Attack(a)) => a.mode,
        _ => -1,
    }
}

/// Response to `0x3603` (`PropertyBuilding_GetAttackMode` `0x00505010`); -1 without an enabled module 3.
fn building_attack_mode(o: &GameObject) -> i32 {
    if !o.has_module(BUILDING, false) {
        return -1;
    }
    building_of(o).map_or(-1, |b| b.attack_mode)
}

/// Response to `0x2217` (occupancy of the quarters); -1 without an enabled module 23 (`GameObject_SendMessage`).
fn occupant_count(o: &GameObject) -> i32 {
    if !o.has_module(quarter::INDEX, false) {
        return -1;
    }
    quarter_of(o).map_or(-1, |q| q.cur)
}

/// Stance bits according to `FUN_00452EB0`: first `0x711`; if the response is 0 ... 4, its bit; otherwise, for
/// buildings with a rally point (button 447 in the menu of the type or module `0x17` with class `0x217`), the bit
/// from `0x3603`.
fn stance(rules: &Rules, o: &GameObject) -> u32 {
    let rally = crate::game::rally_flag::has_flag(rules, o.guid)
        || (o.has_module(RALLY_MODULE, false)
            && o.module(RALLY_MODULE)
                .is_some_and(|m| m.base.has_flags(RALLY_CLASS)));
    let bit = |mode: i32| {
        if (0..5).contains(&mode) { 1 << mode } else { 0 }
    };
    let mode = unit_attack_mode(o);
    if (0..5).contains(&mode) {
        return bit(mode);
    }
    if !rally {
        return 0;
    }
    bit(building_attack_mode(o))
}

/// Bars of a selection or hero button (`0x0044F250` step 5): for each tracked attribute 14, 15, 28, 30
/// `AttributeSet_GetCurrentFraction * 1e-5` in `f32`.
fn bars_of(o: &GameObject) -> ButtonBars {
    ButtonBars {
        tracked: BAR_ATTRIBUTES.map(|a| {
            o.attributes
                .tracks(a)
                .then(|| o.attributes.current_fraction(a) as f32 * 1e-5_f32)
        }),
    }
}

/// A selected object for building the button set (`Toolbar_BuildButtonSet` `0x004557C0`, full rebuild `0x00457980`);
/// `None` if the object does not exist.
pub fn selected_object(sim: &SimHost, id: ObjectId) -> Option<SelectedObject> {
    let state = &sim.state;
    let o = state.object(id, false)?;
    let local = sim.active_player;
    let building = o.has_module(BUILDING, false);
    // `GameObject_MatchesClass(0x403)`: module 3 with `CONSTRUCTED_FINISHED`.
    let finished = o
        .module(BUILDING)
        .is_some_and(|m| m.base.has_flags(bflags::CONSTRUCTED_FINISHED));
    // `0xF03`/`0x1003` on module 3 (`FUN_00454FD0`): running order with progress, then the queue.
    let production = building.then(|| {
        let mut p = Production {
            running: None,
            queue: Vec::new(),
        };
        for e in sim.production_queue(id) {
            if e.index == 0 {
                p.running = Some((e.guid, e.progress.unwrap_or(0.0)));
            } else {
                p.queue.push(e.guid);
            }
        }
        p
    });
    // `0x1817 {12}` and for each occupant `GetObjectById` -> type (`FUN_004548E0`).
    let occupants = if o.has_module(quarter::INDEX, false) {
        quarter_of(o).map_or_else(Vec::new, |q| {
            q.occupants
                .iter()
                .take(OCCUPANT_LIST)
                .filter_map(|&u| state.object(u, false).map(|x| (u.0, x.guid)))
                .collect()
        })
    } else {
        Vec::new()
    };
    let stance_bits = stance(&sim.rules, o);
    Some(SelectedObject {
        hid: id.0,
        guid: o.guid,
        own: o.owner() == local,
        neutral: o.owner() == NEUTRAL_SLOT,
        building,
        site: building && !finished,
        has_unit_module: o.has_module(WALK, false),
        production,
        occupants,
        stance_bits,
    })
}

/// `BUTTONMSG` of the active target actions for the selection `selection` ([`ToolbarModel::targets`]):
/// `Toolbar_BuildButtonSet` (`0x004557C0`) keeps a row of kind TARGET/TARGET_DO (`0x7000000`/`0x8000000`) only if
/// `FUN_0047B0A0(BUTTONMSG)` (`0x004558B4...0x004558C5`, `ECX = [bar + 0xD4]` = `GameEngine+0x294`, the world) finds
/// an entry with `+0xB0 == BUTTONMSG` in the list `GameWorld+0x354` (head `+0x358`). `GameWorld_RebuildTargetActions`
/// (`0x0047D6D0`) builds this list on selection and status changes, when the quarters flags are set
/// (`Toolbar_SubmitSetQuarterFlag` `0x0044AD87`) and when the load changes: every active entry of `targets.ini`
/// (`+0x320`) whose MY filter is satisfied by an object of the selection (player -1). The reimplementation computes
/// it per frame from the state ([`opennml_sim::targeting::active_actions`]); this covers the same triggers. So 415
/// (occupant view, MY = quarters flag `QUARTERED`) appears only for a building with occupants.
///
/// Read-only: the filters query modules via messages (hence `&mut`) but change nothing (like the right-click under
/// the mouse, [`SimHost::right_click_target`]).
pub fn active_target_buttons(sim: &mut SimHost, selection: &[ObjectId]) -> BTreeSet<i32> {
    let active = {
        let mut ctx = opennml_sim::sim::Sim::new(&mut sim.state, &sim.rules);
        opennml_sim::targeting::active_actions(&mut ctx, selection)
    };
    active
        .into_iter()
        .map(|(i, _)| sim.rules.targets[i].button_msg)
        .collect()
}

/// The snapshot of the match for a frame of the command bar: selection (in selection order, missing objects are
/// dropped), locks and object counters of the local slot for all `BUTTONMSG` and `REQUIRED_GUID` of `toolbar.ini`,
/// counter-upgrades, bars, targets, heroes, headhunters, trade, enemies, target GUID level, nation, headquarters,
/// target mouse pointer and tent. Read-only.
///
/// Not set here (the host fills them): `keys` (key state of the input), `formation` (the chosen formation of the
/// world `GameWorld+0x20B8` lives in the host; here 0 like the initial value of the world), `cursor`
/// ([`cursor_frame`]) and `targets` ([`active_target_buttons`]: the filters need the simulation mutable, the
/// snapshot only borrows it; if it is empty, the bar hides all TARGET/TARGET_DO rows).
pub fn toolbar_model<'a>(
    sim: &'a SimHost,
    selection: &[ObjectId],
    visible_pause: bool,
) -> ToolbarModel<'a> {
    let state = &sim.state;
    let rules = &sim.rules;
    let local = sim.active_player;
    let slot = i32::from(local);
    let mut m = ToolbarModel::new(&rules.toolbar);
    m.selection = selection
        .iter()
        .filter_map(|&id| selected_object(sim, id))
        .collect();
    // All buttons of `toolbar.ini` and their prerequisites: what building the button set checks is in there.
    let mut guids: BTreeSet<u32> = BTreeSet::new();
    for b in rules.toolbar.menus.values().flatten() {
        guids.insert(b.msg as u32);
        guids.extend(b.required.iter().filter(|&&g| g != 0).map(|&g| g as u32));
    }
    for g in guids.iter().copied().chain(COLOR_BUTTONS) {
        if opennml_sim::world::is_guid_disabled_in(state, rules, slot, g) {
            m.disabled.insert(g);
        }
    }
    for &g in &guids {
        let stock = opennml_sim::economy::guid_count_in(state, rules, slot, g);
        if stock != 0 {
            m.stock.insert(g, stock);
        }
        let pending = opennml_sim::economy::pending_guid_count_in(state, rules, slot, g);
        if pending != 0 {
            m.pending.insert(g, pending);
        }
    }
    // `0x00412620` (runs anywhere) and `0x00479690` (the local slot has one).
    m.counter_busy = state.counter_upgrades.iter().map(|c| c.guid).collect();
    m.slot_counter_busy = opennml_sim::upgrades::counter::count_of_slot(state, local) > 0;
    // `MAUSICONID` of the first `targets.ini` entry per `BUTTONMSG` (targeting mode). Which target buttons the bar
    // shows (`0x0047B0A0`) is in the active list: [`active_target_buttons`], set by the host.
    for t in &rules.targets {
        m.target_icons.entry(t.button_msg).or_insert(t.mouse_icon);
    }
    let heroes = opennml_sim::economy::heroes_of(state, rules, local);
    m.heroes = heroes
        .iter()
        .filter_map(|&id| state.object(id, false).map(|o| (id.0, o.guid)))
        .collect();
    // Bars per HID of the selection and of the heroes (`GetObjectById(+0x48, 0)`).
    for &id in selection.iter().chain(&heroes) {
        if let Some(o) = state.object(id, false) {
            m.bars.insert(id.0, bars_of(o));
        }
    }
    // `FUN_00477100(kind)`: the headhunter of this kind exists.
    for art in 0..hunter::GUIDS.len() {
        if let Some(o) = hunter::current(state, art).and_then(|id| state.object(id, false)) {
            m.headhunters.push(o.guid);
            m.hunters_present[art] = true;
        }
    }
    m.formation = 0;
    // `FUN_0044B9A0`: slots 0 ... 8 except the local one with a trading building.
    m.trade_partners = (0..=NEUTRAL_SLOT)
        .filter(|&p| {
            p != local && opennml_sim::command::trade::has_trade_building_in(state, i32::from(p))
        })
        .collect();
    // `FUN_0044A8C0`: slots 0 ... 7 that are still playing and are enemies of the local one, with color.
    for (p, player) in state.players.iter().enumerate().take(PLAYER_SLOTS) {
        let q = p as i32;
        if player.result == 0 && state.diplomacy.is_enemy(slot, q) {
            m.enemy_players.push((p as u8, player.color as u8));
        }
        // `FUN_0044AA10`: nation and object counters of the heroes and of the HQ of the nation.
        let stock = hunt_target_types(player.nation).map_or([0; 4], |types| {
            types.map(|g| opennml_sim::economy::guid_count_in(state, rules, q, g))
        });
        m.hunt_targets.insert(
            p as u8,
            HuntTarget {
                nation: player.nation,
                stock,
            },
        );
    }
    m.visible_pause = visible_pause;
    m.local_slot = local;
    let nation = state
        .players
        .get(usize::from(local))
        .map_or(-1, |p| p.nation);
    // `FUN_0048B910`: first headquarters of the nation in the object table of the local slot.
    m.headquarters = usize::try_from(nation)
        .ok()
        .and_then(|n| NATION_HQ.get(n))
        .and_then(|&hq| {
            state
                .tables
                .get(usize::from(local))?
                .objects
                .iter()
                .flatten()
                .find(|o| o.guid == hq)
                .map(|o| o.id.0)
        });
    m.tent_guid = selection.first().and_then(|&id| tent_guid(state, id));
    m
}

/// `FUN_0044ACA0`: `0x423` to the first selected object (module `0x23` enabled, `m_TentHID`), then
/// `GetObjectById(tent, 0)` -> type GUID.
fn tent_guid(state: &SimState, first: ObjectId) -> Option<u32> {
    use opennml_sim::module::tent::transport;
    let o = state.object(first, false)?;
    if !o.has_module(transport::INDEX, false) {
        return None;
    }
    let ModuleKind::TransportTent(t) = &o.module(transport::INDEX)?.kind else {
        return None;
    };
    state.object(t.tent, false).map(|tent| tent.guid)
}

/// Portrait, values and counter-upgrades for the frame ([`portrait_model_for`] with `Toolbar+0x524` and `+0x48`).
pub fn portrait_model(sim: &SimHost, toolbar: &Toolbar) -> PortraitModel {
    portrait_model_for(sim, toolbar.selected, toolbar.hover)
}

/// Portrait according to `0x00456B70`: the object `GetObjectById(+0x524, 0)`, alternatively `GetObjectById(+0x48, 0)`
/// (without a further sight or status check); the nation of the local player for the group icon; the running
/// counter-upgrades in list order (`GameWorld+0x990`) with the color number and the enmity of the source.
pub fn portrait_model_for(sim: &SimHost, selected: i32, hover: u32) -> PortraitModel {
    let state = &sim.state;
    let local = sim.active_player;
    let o = object(state, selected as u32).or_else(|| object(state, hover));
    let counter_upgrades = state
        .counter_upgrades
        .iter()
        .map(|c| {
            let source = c.source.slot();
            CounterRow {
                guid: c.guid,
                rest_ms: c.rest_ms,
                source_color: state
                    .players
                    .get(usize::from(source))
                    .filter(|_| usize::from(source) < PLAYER_SLOTS)
                    .map(|p| p.color as u32),
                enemy: state
                    .diplomacy
                    .is_enemy(i32::from(local), i32::from(source)),
            }
        })
        .collect();
    PortraitModel {
        object: o.map(|o| portrait_object(state, o)),
        group_nation: state.players.get(usize::from(local)).map(|p| p.nation),
        counter_upgrades,
    }
}

/// The existing values of an object in the sequence of [`VALUE_ORDER`] (`0x00455A30`): `base` = base value, `max` =
/// value with the upgrade percentages of the owner for the type (`type_upgrade_percent`), `cur` = current value,
/// `tracked` = byte "tracked" (`AttributeSet::tracks`), rank from `GO+0x7A4`. The **tracked** intake capacity (23)
/// asks `0x2217`: `cur` = occupancy, `max` = maximum; a negative response omits the value. An untracked capacity
/// does not ask `0x2217` (it shows only `base`/`max`).
pub fn portrait_object(state: &SimState, o: &GameObject) -> PortraitObject {
    let mut values = Vec::new();
    for kind in VALUE_ORDER {
        let a = kind.attribute;
        let set = &o.attributes;
        if !set.is_present(a) {
            continue;
        }
        let p = opennml_sim::upgrades::type_upgrade_percent(state, o.owner(), o.guid, a);
        let tracked = set.tracks(a);
        let mut cur = set.current_value(a, p);
        if a == ATTR_CAPACITY && tracked {
            let occupied = occupant_count(o);
            if occupied < 0 {
                continue;
            }
            cur = occupied;
        }
        values.push(PortraitValue {
            attribute: a,
            base: set.base(a),
            max: set.max_value(a, p),
            cur,
            tracked,
            rank: o.rank,
        });
    }
    PortraitObject {
        guid: o.guid,
        values,
    }
}

/// Object under the mouse for `{0x29, HID}` (`FUN_0047A490`, re/game-ui N.6.1): `picked` is the hit of the picker
/// `FUN_0047A210(mouse, 0)` (= `MapView::pick_target`: living, then dead walk units, other mobile objects,
/// buildings, resources; never trees or nature). Then the sender discards: invisible (`FUN_00467D70`,
/// `SimHost::fog_selectable`), dead (bit 18), occupant (bit 8) and module 5 NATURE; it does not fall back to a
/// deeper hit. Result 0 without an object. The rule of the hover ring (`markers::hover_marked`) is a different one
/// and is not shared. Whether the bar adopts the value (only with `+0x524` in {0, -1}) it decides itself.
pub fn hover_target(sim: &SimHost, picked: Option<ObjectId>) -> u32 {
    picked
        .and_then(|id| sim.state.object(id, false))
        .filter(|o| {
            sim.fog_selectable(o)
                && o.status & (status::DIED | status::QUARTERED) == 0
                && !o.has_module(NATURE, false)
        })
        .map_or(0, |o| o.id.0)
}

/// Signature of a selected object (deviation from the original): the triggers of `NotifyUiIfSelected` as values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSignature {
    pub hid: u32,
    /// Type GUID (a type change without `{8}` rebuilds silently, D-234).
    pub guid: u32,
    pub owner: u8,
    /// Belongs to the local player (the original reports only own objects, `0x00469390`).
    pub own: bool,
    /// Module 3 with `CONSTRUCTED_FINISHED` (construction finished `0x00509402`).
    pub finished: bool,
    /// Running order (GUID, 0 without) and length of the queue.
    pub running: u32,
    pub queue: usize,
    /// Occupant count (quarters).
    pub occupants: usize,
    /// Load > 0 (`GameObject_SetCarriedLoad` reports the change 0 <-> > 0).
    pub loaded: bool,
    /// Gate open/closed (`TOROPEN`/`TORCLOSE`).
    pub gate: u32,
    /// `ISARMED` in module 39.
    pub armed: bool,
    /// Attack mode of the building (`PropertyBuilding_OnMsgSetBehavior` reports `{0x10, 2}`, `0x00504FF6`).
    pub building_mode: i32,
    /// Status bits `0x20000`, `0x100000`, `0x800000`.
    pub status: u32,
    /// Variant bits 12/13 of the prop bits per module in module order (arrow type).
    pub arrow: Vec<u32>,
}

impl ObjectSignature {
    fn of(o: &GameObject, local: u8) -> Self {
        let b = building_of(o);
        Self {
            hid: o.id.0,
            guid: o.guid,
            owner: o.owner(),
            own: o.owner() == local,
            finished: o
                .module(BUILDING)
                .is_some_and(|m| m.base.has_flags(bflags::CONSTRUCTED_FINISHED)),
            running: b.map_or(0, |b| b.make_guid),
            queue: b.map_or(0, |b| b.make_queue.len()),
            occupants: quarter_of(o).map_or(0, |q| q.occupants.len()),
            loaded: o.load_amount > 0,
            gate: o.module(BUILDING).map_or(0, |m| m.base.flags & GATE_BITS),
            armed: o
                .module(arming::INDEX)
                .is_some_and(|m| m.base.flags & ISARMED != 0),
            building_mode: b.map_or(-1, |b| b.attack_mode),
            status: o.status & SIGNATURE_STATUS,
            arrow: o
                .modules
                .iter()
                .flatten()
                .map(|m| m.base.prop_bits & ARROW_BITS)
                .collect(),
        }
    }

    /// Orders in total (running + waiting).
    fn orders(&self) -> usize {
        usize::from(self.running != 0) + self.queue
    }
}

/// Signature of the selection for a frame: the selected objects and the global triggers (running counter-upgrades
/// `GameWorld_CmdStartCounterUpgrade` `0x004849F4`, script locks 207/208 `0x00424DBC`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signature {
    pub objects: Vec<ObjectSignature>,
    /// GUID and source HID of the running counter-upgrades in list order.
    pub counter_upgrades: Vec<(u32, u32)>,
    /// `SimState::button_locks` (script lock `GameWorld+0x21E0`).
    pub button_locks: Vec<(u32, u8)>,
}

impl Signature {
    /// Signature of the selection `selection` (missing objects are dropped).
    pub fn of(sim: &SimHost, selection: &[ObjectId]) -> Self {
        let state = &sim.state;
        Self {
            objects: selection
                .iter()
                .filter_map(|&id| state.object(id, false))
                .map(|o| ObjectSignature::of(o, sim.active_player))
                .collect(),
            counter_upgrades: state
                .counter_upgrades
                .iter()
                .map(|c| (c.guid, c.source.0))
                .collect(),
            button_locks: state.button_locks.iter().map(|(&g, &v)| (g, v)).collect(),
        }
    }
}

/// Sub-kind of the rebuild between two frames (re/game-ui N.2, deviation from the original), `None` without a trigger.
///
/// Only objects are compared that are selected in both frames and belong to the local player in one of them: a
/// selection change without `{8}` (headhunter button, SHORTCUT) does not rebuild in the original (O-270). Per object:
/// - gate, arming or attack mode of the building -> 2;
/// - type, owner, construction finished, load, status bits, arrow type -> 0;
/// - more orders (queued, `0x00508794`) -> 0; otherwise a changed order or a shorter queue
///   (`PropertyBuilding_TickProduction`) -> 3;
/// - a cancellation in the step (`cancelled` contains the HID, from [`SimEvent::ProductionCancelled`]; `0x00507960`
///   reports `{0x10, 0}` on every success path) -> 0;
/// - fewer occupants (one out, `0x00520599`) -> 1, more (taken in) -> 0.
///
/// In addition globally: changed script locks -> 0; changed counter-upgrades -> 0 if an own object is selected.
/// Combined: 2 before 0; 1 and 3 only alone (both together -> 0, which covers both).
///
/// The signature does not tell the cancellation apart from the finished order (both shorten the orders), so the
/// simulation reports it separately: completion and start stay 3 (only the production row), the cancellation
/// rebuilds silently and in doing so re-checks the hiding of the one-time buttons (UPGRADE, MAKEONLY_ONCE)
/// (re/game-ui N.14.13).
pub fn rebuild_kind(old: &Signature, new: &Signature, cancelled: &[u32]) -> Option<u8> {
    let (mut two, mut zero, mut one, mut three) = (false, false, false, false);
    let mut own = false;
    for n in &new.objects {
        own |= n.own;
        let Some(o) = old.objects.iter().find(|o| o.hid == n.hid) else {
            continue;
        };
        if !(o.own || n.own) {
            continue;
        }
        if o.gate != n.gate || o.armed != n.armed || o.building_mode != n.building_mode {
            two = true;
        }
        if o.guid != n.guid
            || o.owner != n.owner
            || o.finished != n.finished
            || o.loaded != n.loaded
            || o.status != n.status
            || o.arrow != n.arrow
        {
            zero = true;
        }
        if n.orders() > o.orders() || cancelled.contains(&n.hid) {
            zero = true;
        } else if o.running != n.running || o.queue != n.queue {
            three = true;
        }
        match n.occupants.cmp(&o.occupants) {
            std::cmp::Ordering::Less => one = true,
            std::cmp::Ordering::Greater => zero = true,
            std::cmp::Ordering::Equal => {}
        }
    }
    if old.button_locks != new.button_locks {
        zero = true;
    }
    if own && old.counter_upgrades != new.counter_upgrades {
        zero = true;
    }
    if two {
        Some(2)
    } else if zero || (one && three) {
        Some(0)
    } else if one {
        Some(1)
    } else if three {
        Some(3)
    } else {
        None
    }
}

/// The events `{0x10, n}` between two frames ([`rebuild_kind`]); a change of arming of an own selected object
/// afterwards also sends `{0x10, 0}`, like `0x004FF750`. `cancelled`: HIDs of the orders cancelled in the step
/// ([`rebuild_kind`]).
pub fn rebuild_events(old: &Signature, new: &Signature, cancelled: &[u32]) -> Vec<HudEvent> {
    let Some(n) = rebuild_kind(old, new, cancelled) else {
        return Vec::new();
    };
    let mut out = vec![HudEvent::with_args(HudEventKind::Rebuild, &[i32::from(n)])];
    let armed_changed = new.objects.iter().any(|n| {
        old.objects
            .iter()
            .find(|o| o.hid == n.hid)
            .is_some_and(|o| (o.own || n.own) && o.armed != n.armed)
    });
    if n == 2 && armed_changed {
        out.push(HudEvent::with_args(HudEventKind::Rebuild, &[0]));
    }
    out
}

/// UI messages of a presentation event of the scripts in sending order; the host distributes them (`Hud::present`):
/// `0x1B` and `0x1E` ... `0x21` to the cinemascope area, `0x1C` to the status bar, `0x11` and `0x19` to the command
/// bar, `0x1D` to the minimap.
/// - Lock 18/19 -> `{0x19, on}` (`FUN_0042D920`; the bar closes the cursor and rebuilds without a selection, only
///   when the bar is visible).
/// - Cinemascope 147 (`FUN_00430920`) -> `{0x11, 1}`, `{0x1D, 0}`, `{0x1C, 0}`, in the campaign (`campaign`, game
///   type -1, `0x004309BE`) additionally `{0x1B, H/6}` with the screen height `screen_h` (O-350). The `{8}` in
///   between comes via clearing the selection after `{0x11, 1}` and is lost (O-234).
/// - Cinemascope 148 -> `{0x11, 0}`, `{0x1D, 1}`, `{0x1C, 1}`, `{0x1B, 0}` in every game type.
/// - Portrait 149 (`FUN_0042DBD0`) -> `{0x1E, slot, frame, -1, portrait, -1}`; 156 -> `{0x1F, slot}`.
/// - Dialog line 150 (`Dialog_PlaySpeech` `0x00436A50`) -> `{0x20, slot, text, -1}` only for slot < 4; the end
///   (`FUN_00443450`) -> `{0x21, slot}`. From slot 4 on (213) an info window speaks, nothing goes to the area.
/// - Tip 183 (`FUN_0043E250`) -> `{0x20, slot + 2, text, -1}`; the end (`FUN_00432B90`) -> `{0x21, slot + 2}`.
pub fn presentation_events(
    event: &opennml_sim::script::present::Presentation,
    campaign: bool,
    screen_h: i32,
) -> Vec<HudEvent> {
    use opennml_sim::script::present::Presentation;
    let ev = HudEvent::with_args;
    match *event {
        Presentation::UiLock { on } => vec![ev(HudEventKind::Lock, &[on as i32])],
        Presentation::Cinemascope { on: true } => {
            let mut out = vec![
                ev(HudEventKind::ToolbarOnOff, &[1]),
                ev(HudEventKind::MinimapOnOff, &[0]),
                ev(HudEventKind::StatusBarOnOff, &[0]),
            ];
            if campaign {
                out.push(ev(HudEventKind::Cinemascope, &[screen_h / 6]));
            }
            out
        }
        Presentation::Cinemascope { on: false } => vec![
            ev(HudEventKind::ToolbarOnOff, &[0]),
            ev(HudEventKind::MinimapOnOff, &[1]),
            ev(HudEventKind::StatusBarOnOff, &[1]),
            ev(HudEventKind::Cinemascope, &[0]),
        ],
        Presentation::Portrait {
            slot,
            portrait,
            frame,
        } => vec![ev(
            HudEventKind::PortraitShow,
            &[slot, frame, -1, portrait, -1],
        )],
        Presentation::PortraitHide { slot } => vec![ev(HudEventKind::PortraitHide, &[slot])],
        Presentation::Speech { text, slot, .. } if slot < 4 => {
            vec![ev(HudEventKind::TextShow, &[slot, text, -1])]
        }
        Presentation::SpeechEnd { slot, .. } if slot < 4 => {
            vec![ev(HudEventKind::TextHide, &[slot])]
        }
        Presentation::Tip { text, slot, .. } => {
            vec![ev(HudEventKind::TextShow, &[slot + 2, text, -1])]
        }
        Presentation::TipEnd { slot, .. } => vec![ev(HudEventKind::TextHide, &[slot + 2])],
        _ => Vec::new(),
    }
}

/// Events of the host to the command bar (see module header). Host state, not in the savegame; after loading and
/// restart [`ToolbarHost::reset`].
#[derive(Debug, Clone, Default)]
pub struct ToolbarHost {
    /// Signature of the previous frame; `None` = only measure in the next frame (start, after `{8}`).
    signature: Option<Signature>,
    /// `{8}` since the last frame.
    pending: Vec<HudEvent>,
    /// Hero and headhunter rows have been sent (otherwise the next frame sends the initial state).
    rows_sent: bool,
    /// HIDs of the most recently sent hero row.
    heroes: Vec<u32>,
    /// `GameWorld+0x4DC` when the headhunter row was last sent.
    hunters: [Option<ObjectId>; 3],
    /// The events of the last frame in sending order; they are given to the bar (`Toolbar::event`).
    pub frame_events: Vec<HudEvent>,
    /// State of the bar commands: the chosen formation of the world (`GameWorld+0x20B8`, setter `0x00477960` from
    /// `0x00457930`) for the walk commands of the right-clicks and the bar; targeting mode and tent cursor stay empty
    /// (the cursors of the bar are those of `opennml_ui`). Not in the savegame.
    pub bar: UnitBar,
    /// The bar is hidden: the last `{0x11, on}` that came through ([`ToolbarHost::host_event`], also the one from
    /// `GameSession::restore_presentation`). Then [`ToolbarHost::frame`] adopts the signature without `{0x10, n}`
    /// (as in the original, O-234).
    hidden: bool,
}

impl ToolbarHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loading, restart: measure the signature anew, send the rows in the next frame from the state, nothing pending,
    /// bar visible.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Selection changed: `{8}` to the bar (deviation from the original: click, box, double-click, deselecting,
    /// quartering); the signature is only re-measured in the next frame. Without a sound of its own from the bar.
    pub fn selection_changed(&mut self) {
        self.pending
            .push(HudEvent::new(HudEventKind::SelectionChanged));
        self.signature = None;
    }

    /// The pending events to the bar (for tests).
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[HudEvent] {
        &self.pending
    }

    /// Another event of the host to the bar, sent with the next frame in sequence with `{8}`
    /// ([`presentation_events`]). A `{0x11, on}` remembers the visibility of the bar (O-234).
    pub fn host_event(&mut self, event: HudEvent) {
        if event.kind == HudEventKind::ToolbarOnOff {
            self.hidden = event.args.first().copied().unwrap_or(0) != 0;
        }
        self.pending.push(event);
    }

    /// The bar is hidden after the last `{0x11, on}`.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// HIDs of the most recently sent hero row.
    pub fn heroes(&self) -> &[u32] {
        &self.heroes
    }

    /// The most recently sent rows once more (`{0x25}` + `{0x23, 1, HID}`, `{0x32}` + `{0x31, 1, GUID}`): for a new
    /// screen width, at whose right edge heroes and headhunters hang. Same buttons, new positions.
    pub fn row_events(&self, sim: &SimHost) -> Vec<HudEvent> {
        let state = &sim.state;
        let mut out = vec![HudEvent::new(HudEventKind::HeroClear)];
        for &h in &self.heroes {
            out.push(HudEvent::with_args(HudEventKind::HeroOn, &[1, h as i32]));
        }
        out.push(HudEvent::new(HudEventKind::HeadhunterClear));
        for id in self.hunters.iter().flatten() {
            if let Some(o) = state.object(*id, false) {
                out.push(HudEvent::with_args(
                    HudEventKind::HeadhunterOn,
                    &[1, o.guid as i32],
                ));
            }
        }
        out
    }

    /// A frame after the simulation step with the events `events`: pending `{8}`, `{0x10, n}`, hero and headhunter
    /// rows, `{0x29, 0}` and `{0x29, hover}` ([`hover_target`]). The result is also in
    /// [`ToolbarHost::frame_events`].
    ///
    /// Hidden bar (cinemascope, as in the original, O-234): the original discards the `{8}` and the rows
    /// there, whereas `{0x10, n}` also rebuilds while hidden (`0x00458510`); after the cinemascope the buttons from
    /// before remain until `{8}` or `{0x10}` comes. The reimplementation derives `{0x10, n}` from the signature and
    /// adopts it while hidden without a rebuild: what changes during the cinemascope is not rebuilt by the bar until
    /// the next trigger afterwards.
    ///
    /// Hero row (O-271, O-274; Ghidra `0x004864A2`, `0x00481BDB`, `0x00532C34`): the original rebuilds it on **every**
    /// creation and destruction of an object of the local slot and when a walk unit is given away, not when one is
    /// gained (O-274). On destruction the rebuild runs before the table slot is empty; a destroyed hero thus keeps its
    /// button (O-271). The reimplementation rebuilds the row from `heroes_of` after the step, except when the only
    /// triggers of the step were the destruction of displayed heroes: then the row stays (same buttons as in the
    /// original). It deviates only if in the same step a hero is destroyed and afterwards another object is created or
    /// destroyed (original: the dead button would stay if the destruction came last).
    pub fn frame(
        &mut self,
        sim: &SimHost,
        selection: &[ObjectId],
        events: &[SimEvent],
        hover: u32,
    ) -> &[HudEvent] {
        let state = &sim.state;
        let local = sim.active_player;
        let mut out = std::mem::take(&mut self.pending);
        let signature = Signature::of(sim, selection);
        if let Some(old) = self.signature.as_ref().filter(|_| !self.hidden) {
            let cancelled: Vec<u32> = events
                .iter()
                .filter_map(|e| match *e {
                    SimEvent::ProductionCancelled { building } => Some(building.0),
                    _ => None,
                })
                .collect();
            out.extend(rebuild_events(old, &signature, &cancelled));
        }
        self.signature = Some(signature);

        let mut rebuild = !self.rows_sent;
        let mut only_dead_heroes = true;
        for e in events {
            match *e {
                SimEvent::Created { id } if id.slot() == local => {
                    rebuild = true;
                    only_dead_heroes = false;
                }
                SimEvent::Destroyed { id } if id.slot() == local => {
                    rebuild = true;
                    only_dead_heroes &= self.heroes.contains(&id.0);
                }
                SimEvent::OwnerChanged { old, new }
                    if old.slot() == local
                        && state
                            .object(new, false)
                            .is_some_and(|o| o.module(WALK).is_some()) =>
                {
                    rebuild = true;
                    only_dead_heroes = false;
                }
                _ => {}
            }
        }
        if rebuild && (!self.rows_sent || !only_dead_heroes) {
            self.heroes = opennml_sim::economy::heroes_of(state, &sim.rules, local)
                .into_iter()
                .map(|id| id.0)
                .collect();
            out.push(HudEvent::new(HudEventKind::HeroClear));
            for &h in &self.heroes {
                out.push(HudEvent::with_args(HudEventKind::HeroOn, &[1, h as i32]));
            }
        }
        let hunters = state.headhunters;
        if !self.rows_sent || hunters != self.hunters {
            self.hunters = hunters;
            out.push(HudEvent::new(HudEventKind::HeadhunterClear));
            for art in 0..hunter::GUIDS.len() {
                if let Some(o) = hunter::current(state, art).and_then(|id| state.object(id, false))
                {
                    out.push(HudEvent::with_args(
                        HudEventKind::HeadhunterOn,
                        &[1, o.guid as i32],
                    ));
                }
            }
        }
        self.rows_sent = true;
        out.push(HudEvent::with_args(HudEventKind::Hover, &[0]));
        out.push(HudEvent::with_args(HudEventKind::Hover, &[hover as i32]));
        self.frame_events = out;
        &self.frame_events
    }
}

/// World queries for the open cursor of a frame (`CursorFrame`). `world` is the ground point under the pointer
/// (`Cursor3D`), `mouse` the pointer in UI pixels, `picked` the object under the mouse.
///
/// - **Build and tent:** the snapped point with the rotation of the cursor (`BuildCursor_GetSnappedPosition`
///   `0x004CD240` via `SimHost::placement_preview_rot` = `opennml_sim::placement::cursor_placement`) and its validity
///   (placement check and sight, like the build preview); for the tent with the travois as worker (layer/region of
///   the unit). `rotatable` = type without rotation lock `0x10000000` (O-279).
/// - **Wall:** the raw point; `ring_valid` stays `None` here (the check of the pieces `BuildCursor_CheckWallSegments`
///   `0x00450870` comes later in the frame).
/// - **Target:** the raw point and `object` = HID under the mouse; `valid` stays false here, the caller sets it
///   (`SimHost::button_target_valid`).
/// - **Defense:** the raw point, `valid` = point explored (ring only on explored tiles).
///
/// Calls the placement check of the simulation via `&mut` but changes nothing (like the build preview of the
/// viewer). The point in time within the frame (before or after `Toolbar::update`) is fixed by the caller.
pub fn cursor_frame(
    sim: &mut SimHost,
    cursor: &Cursor,
    world: Option<Vec3>,
    mouse: [i32; 2],
    selection: &[ObjectId],
    picked: Option<ObjectId>,
) -> CursorFrame {
    let cell = sim.rules.map.cell;
    let mut frame = CursorFrame {
        world: world.map(|v| v.to_array()),
        mouse,
        tile: [cell[0], cell[2]],
        ..CursorFrame::default()
    };
    match cursor {
        Cursor::Build(b) => {
            frame.rotatable = !opennml_sim::placement::rotation_locked(&sim.rules, b.guid);
            let tent: Vec<ObjectId> = b.tent.map(ObjectId).into_iter().collect();
            let workers = if b.tent.is_some() { &tent } else { selection };
            match world.and_then(|g| sim.placement_preview_rot(b.guid, g, b.rotation, workers)) {
                Some(preview) => {
                    frame.world = Some(preview.pos);
                    frame.valid = preview.valid;
                }
                None => frame.valid = false,
            }
        }
        Cursor::Fort(_) => {}
        Cursor::Target(_) => {
            frame.object = picked.map(|id| id.0);
        }
        Cursor::Defense(_) => {
            let player = i32::from(sim.active_player);
            let map = sim.rules.map;
            frame.valid = world.is_some_and(|g| {
                sim.fog
                    .view(&sim.state)
                    .world_pos_explored(&map, g.to_array(), player, false)
            });
        }
    }
    frame
}
