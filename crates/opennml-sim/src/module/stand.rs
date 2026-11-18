//! Module 2 `CPropertyStand` (re/property-modules): resting state and
//! idle animation. The re-selection of the idle animation, in the original per drawn frame
//! (`PropertyStand_FrameUpdate` `0x0052D790`), runs here at the end of every object tick.

use opennml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::fx::Fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::object::status;
use crate::sim::Sim;
use crate::world::snap_height_to_terrain;

use super::ModuleKind;
use super::events::update_events;

pub const INDEX: usize = 2;
/// `ONWATER`.
pub const ONWATER: u32 = 0x100;
/// Group of the stand timers.
const GROUP: u8 = 0x80;
/// Status bit "swimming".
const SWIMMING: u32 = 0x10000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stand {
    /// `m_StandAnim` (VALUEID 30001).
    pub stand_anim: Vec<i32>,
    /// `m_LastLoadType`, `m_LastLoad`, `m_WasMounted`.
    pub last_load_type: i32,
    pub last_load: i32,
    pub was_mounted: bool,
    /// The next change crossfades (`+0x8C`).
    pub blend: bool,
}

impl Default for Stand {
    /// Constructor `0x0052CDD0`.
    fn default() -> Self {
        Self {
            stand_anim: Vec::new(),
            last_load_type: -1,
            last_load: -1,
            was_mounted: false,
            blend: true,
        }
    }
}

/// `IntList_ParseFromString` (`0x004059F0`): trimmed, separated by spaces; an empty
/// string leaves the old list in place. Non-numbers yield 0 (the original leaves the entry
/// uninitialized, exception A7); a double space ends the list.
pub fn parse_int_list(text: &str, old: &[i32]) -> Vec<i32> {
    let text = text.trim();
    if text.is_empty() {
        return old.to_vec();
    }
    let mut list = Vec::new();
    for (i, part) in text.split(' ').enumerate() {
        if part.is_empty() && i > 0 {
            break;
        }
        list.push(opennml_formats::config::scan_int(part).unwrap_or(0));
    }
    list
}

fn stand_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Stand> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Stand(s) => Some(s),
        _ => None,
    }
}

fn stand<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Stand> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Stand(s) => Some(s),
        _ => None,
    }
}

/// `PropertyStand_ReadParams` (`0x0052D7B0`).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let text = desc.values.get(&30001).cloned().unwrap_or_default();
    if let Some(s) = stand_mut(sim, id) {
        s.stand_anim = parse_int_list(&text, &s.stand_anim);
    }
}

/// `PropertyStand_Init` (`0x0052D170`): flags (30003), list, `ONWATER` in deep water (more
/// than 2300 fixed-point units below the water level), then immediately the first
/// idle animation without crossfade. At the end the mercenaries 2720-2725 create a vision source
/// for all players.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = desc.value_int(30003).unwrap_or(0) as u32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    // Visible (model and shadow), except with flag 0x200 (fishing baskets).
    if flags & 0x200 == 0
        && let Some(o) = sim.object_mut(id)
    {
        o.render_bits |= 3;
    }
    read_params(sim, id, desc);
    let map = sim.rules.map;
    let Some(o) = sim.object(id) else { return };
    let water = Fx::from_f32(map.water_level).0 / map.cell[1];
    let ground =
        Fx::from_f32(sim.rules.ground_height(o.world_pos[0], o.world_pos[2])).0 / map.cell[1];
    if ground < water.wrapping_sub(0x8FC) && o.status & SWIMMING == 0 {
        if let Some(m) = sim.module_mut(id, INDEX) {
            m.base.flags |= ONWATER;
        }
        snap_height_to_terrain(sim, id);
    }
    let anim = select_idle_anim(sim, id, 0);
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, 0, true, true, false);
    }
    // Mercenaries (`0xA9F < guid < 0xAA6`, `0x0052D2A6`...): vision source on the object, size 8, all players,
    // unlimited, explored + seen (re/vision 4.4).
    if sim
        .object(id)
        .is_some_and(|o| (2720..=2725).contains(&o.guid))
    {
        crate::fog::set_vision_source(
            &mut sim.state.fog,
            crate::fog::VisionSource {
                x: -1,
                z_or_hid: id.as_i32(),
                size: 8,
                player: -1,
                remaining_ms: -1,
                flags: crate::fog::EXPLORE | crate::fog::SEE,
            },
        );
    }
}

/// `PropertyStand_Tick` (`0x0052CEB0`): quiet units ask the walk module for `0x900` (CHECKPOS) every tick,
/// then the timers run.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    let quiet = sim
        .object(id)
        .is_some_and(|o| o.status & 0xFFEC_A7FF == 0 && o.has_module(0, false));
    if quiet {
        dispatch_command(sim, id, &Envelope::bare(Message::new(0x900, &[])));
    }
    update_events(sim, id, INDEX, dt);
}

/// `PropertyStand_FrameUpdate` (`0x0052D790`) in the object tick: re-select the idle animation
/// when state, load or riding status have changed. For this the original draws `rand()` per
/// drawn frame as the selection value; it only has an effect without a state change for lists with
/// several entries (none in the shipped data) and is omitted here.
pub fn frame_update(sim: &mut Sim, id: ObjectId) {
    update_idle_anim(sim, id, 0, false, false, 0);
}

/// `PropertyStand_Query` (`0x0052D5B0`).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let n = msg.args.len();
    match msg.code {
        0x102 => select_idle_anim(sim, id, if n != 0 { msg.arg(0) } else { 0 }),
        0x202 => {
            let sel = if n == 3 {
                msg.arg(1)
            } else {
                sim.state.crt.rand()
            };
            let force = n >= 1 && msg.arg(0) != 0;
            let start = if n == 3 { msg.arg(2) } else { 0 };
            update_idle_anim(sim, id, sel, force, n == 3, start);
            0
        }
        0x302 | 0x402 => {
            if let Some(m) = sim.module_mut(id, INDEX) {
                if msg.code == 0x302 {
                    m.base.flags |= ONWATER;
                } else {
                    m.base.flags &= !ONWATER;
                }
            }
            0
        }
        0x502 => {
            let anim = msg.arg(0);
            let guid = sim.object(id).map_or(0, |o| o.guid);
            let duration = sim.rules.anim_label_time(guid, anim);
            if let Some(o) = sim.object_mut(id) {
                o.set_status_flags(0x80_0000, true);
                o.set_animation(anim, 0, false, false, false);
            }
            if let Some(m) = sim.module_mut(id, INDEX) {
                m.base.clear_event_group(GROUP);
                m.base
                    .post_delayed_message(duration, Message::new(0x602, &[]), false, GROUP);
            }
            0
        }
        0x602 => {
            if let Some(o) = sim.object_mut(id) {
                o.set_status_flags(0x80_0000, false);
            }
            0
        }
        0x702 => {
            let sel = sim.state.crt.rand();
            update_idle_anim(sim, id, sel, false, true, -1);
            0
        }
        0x802 => {
            if let Some(s) = stand_mut(sim, id) {
                s.blend = false;
            }
            0
        }
        _ => -1,
    }
}

/// `IntList_GetModulo` (`0x00405860`): `list[v % count]`; empty lists or negative indices
/// (a crash in the original) yield -1 (exception A7).
pub(crate) fn list_modulo(list: &[i32], v: i32) -> i32 {
    if list.is_empty() {
        return -1;
    }
    let i = v.wrapping_rem(list.len() as i32);
    usize::try_from(i).map_or(-1, |i| list[i])
}

/// `PropertyStand_SelectIdleAnim` (`0x0052CF30`): list first; when mounted, in a boat, diving,
/// swimming or healing, the responsible module answers (without a -1 check, quirk); with a
/// load the gathering module (-1 -> list).
pub fn select_idle_anim(sim: &mut Sim, id: ObjectId, sel: i32) -> i32 {
    let base = stand(sim, id).map_or(-1, |s| list_modulo(&s.stand_anim, sel));
    let Some(o) = sim.object(id) else { return base };
    let (amount, kind, s, target) = (o.load_amount, o.load_kind, o.status, o.target);
    if o.has_module(0x2E, false) {
        send(sim, id, &Message::new(0x72E, &[]));
        return base;
    }
    if s & 0x100_0000 != 0 {
        return if target.is_none() || target.0 == 0 || sim.object(target).is_none() {
            base
        } else {
            send(sim, target, &Message::new(0x32E, &[id.as_i32()]))
        };
    }
    for (bit, code, args) in [
        (0x2000, 0x50D, vec![7, sel]),
        (0x8000, 0x50E, vec![7, sel]),
        (0x20000, 0x418, vec![sel]),
        (0x10000, 0x20F, vec![sel]),
        // Healing trance: `m_StandAnim[sel % n]` of the healing module (`healing::query`, re/healing 2.1, 4).
        (status::HEAL, 0x51D, vec![sel]),
    ] {
        if s & bit != 0 {
            return send(sim, id, &Message::new(code, &args));
        }
    }
    if amount < 1 {
        return base;
    }
    let r = match kind {
        0 => send(sim, id, &Message::new(0x407, &[sel])),
        1 => send(sim, id, &Message::new(0x208, &[sel])),
        2 => {
            let r = send(sim, id, &Message::new(0x20C, &[sel]));
            if r != -1 {
                return r;
            }
            send(sim, id, &Message::new(0x416, &[sel]))
        }
        _ => return base,
    };
    if r != -1 { r } else { base }
}

/// `PropertyStand_UpdateIdleAnim` (`0x0052D2C0`): new idle animation when state,
/// load or riding status have changed or it is forced; with a random start time
/// `rand() % (duration - 200)` at a state change at rest.
pub fn update_idle_anim(
    sim: &mut Sim,
    id: ObjectId,
    sel: i32,
    force: bool,
    relayed: bool,
    start: i32,
) {
    let mut sel = sel;
    let Some(o) = sim.object(id) else { return };
    if o.status & 0x40000 != 0 {
        return;
    }
    let (amount, kind) = (o.load_amount, o.load_kind);
    let mut dirty = o.dirty;
    let mut st = o.status;
    let mut mounted = false;
    let linked = o.target;
    if o.status & (0x2000 | 0x8000) != 0
        && !linked.is_none()
        && linked.0 != 0
        && let Some(m) = sim.state.object_mut(linked, false)
    {
        st |= m.status & 0x11;
        dirty |= m.dirty;
        mounted = true;
        m.dirty = false;
    }
    let status = sim.object(id).map_or(0, |o| o.status);
    if status & 0x4000 != 0 && !relayed {
        return;
    }
    let Some(s) = stand(sim, id) else { return };
    if !force && !relayed && !dirty {
        if s.was_mounted == mounted && amount == s.last_load && kind == s.last_load_type {
            return;
        }
    } else if dirty {
        sel = 0;
        if let Some(o) = sim.object_mut(id) {
            o.dirty = false;
        }
    }
    if let Some(s) = stand_mut(sim, id) {
        s.last_load = amount;
        s.was_mounted = mounted;
        s.last_load_type = kind;
    }
    let quiet = st & 0xFFFE_5B3F == 0;
    if st & 0xF9EC_07FF != 0 && !force && st & 0x10 == 0 {
        return;
    }
    let anim = select_idle_anim(sim, id, sel);
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let mut duration = sim.rules.anim_label_time(guid, anim);
    if duration <= 0 {
        return;
    }
    let blend = stand(sim, id).is_some_and(|s| s.blend);
    let mut r = 0;
    if quiet && dirty && start != -1 {
        let divisor = duration - 200;
        let roll = sim.state.crt.rand();
        // Quirk: divisor 0 would be a crash in the original (A7: no offset).
        r = if divisor == 0 {
            0
        } else {
            roll.wrapping_rem(divisor)
        };
        if let Some(o) = sim.object_mut(id) {
            o.set_animation(anim, r, force, true, blend);
        }
        duration -= r;
    } else if let Some(o) = sim.object_mut(id) {
        o.set_animation(anim, start.max(0), force, true, blend);
    }
    if let Some(s) = stand_mut(sim, id) {
        s.blend = true;
    }
    if sim.object(id).is_some_and(|o| o.has_module(0x2E, false)) {
        send(sim, id, &Message::new(0x72E, &[]));
        return;
    }
    if mounted {
        send(
            sim,
            linked,
            &Message::new(0x202, &[i32::from(force), sel, r]),
        );
    }
    let count = stand(sim, id).map_or(0, |s| s.stand_anim.len());
    let carrying = sim.object(id).is_some_and(|o| o.status & 0x4000 != 0);
    if count > 1
        && !carrying
        && let Some(m) = sim.module_mut(id, INDEX)
    {
        m.base.clear_event_group(GROUP);
        m.base
            .post_delayed_message(duration, Message::new(0x702, &[]), false, GROUP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 3\nVALUEID 30003 6 1 0\nGUID 1108\nNAME Schiff\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 1 4\nVALUEID 30003 6 1 256\n";

    // QUIRK W-148: kept
    #[test]
    fn int_lists_parse_like_the_game() {
        assert_eq!(parse_int_list(" 3 ", &[]), [3]);
        assert_eq!(parse_int_list("16 17", &[]), [16, 17]);
        assert_eq!(parse_int_list("1  2", &[]), [1]);
        assert_eq!(parse_int_list("", &[9]), [9]);
        assert_eq!(parse_int_list("x 2", &[]), [0, 2]);
        assert_eq!(list_modulo(&[4, 5], 3), 5);
        assert_eq!(list_modulo(&[], 3), -1);
        // C `%`: -1 % 1 = 0 (valid), -1 % 2 = -1 (a crash in the original).
        assert_eq!(list_modulo(&[4], -1), 4);
        assert_eq!(list_modulo(&[4, 5], -1), -1);
    }

    #[test]
    fn init_sets_the_first_idle_animation_and_onwater_on_deep_water() {
        let mut rules = Rules::synthetic(TYPES, 16);
        rules.set_anim_labels(1100, 3, &[(1, 1500)]);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let unit =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [100.0, 0.0, 100.0])).unwrap();
        let o = sim.object(unit).unwrap();
        assert_eq!((o.anim.current, o.anim.wanted, o.anim.starts), (3, 3, 1));
        assert!(!o.anim.blend);
        assert_eq!(o.module(2).unwrap().base.flags, 0);
        // Queries: animation, ONWATER on/off, unknown -1.
        assert_eq!(send(&mut sim, unit, &Message::new(0x102, &[])), 3);
        send(&mut sim, unit, &Message::new(0x302, &[]));
        assert_eq!(sim.module(unit, 2).unwrap().base.flags, ONWATER);
        send(&mut sim, unit, &Message::new(0x402, &[]));
        assert_eq!(send(&mut sim, unit, &Message::new(0x902, &[])), -1);
        // One-shot animation: lock bit, delayed animation, timer of the time marker.
        send(&mut sim, unit, &Message::new(0x502, &[3]));
        let o = sim.object(unit).unwrap();
        assert_ne!(o.status & 0x80_0000, 0);
        assert_eq!(o.module(2).unwrap().base.timers_ws[0].b, 1500);
        // Ship: the map is at 0, water at -100: not deep enough for ONWATER from the
        // terrain, but the flags of the data set it.
        let ship =
            create_object(&mut sim, &mut ObjectDesc::new(1108, 0, [300.0, 0.0, 300.0])).unwrap();
        assert_eq!(sim.module(ship, 2).unwrap().base.flags, ONWATER);
        if let ModuleKind::Stand(s) = &sim.module(ship, 2).unwrap().kind {
            assert_eq!(s.stand_anim, [1, 4]);
        }
    }

    #[test]
    fn idle_updates_follow_load_changes_and_use_the_crt_start_time() {
        let mut rules = Rules::synthetic(TYPES, 16);
        rules.set_anim_labels(1100, 3, &[(1, 1500)]);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let unit =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [100.0, 0.0, 100.0])).unwrap();
        // Unchanged (init has not remembered anything yet: last_load -1) -> re-selection, without randomness.
        let starts = sim.object(unit).unwrap().anim.starts;
        update_idle_anim(&mut sim, unit, 0, false, false, 0);
        assert_eq!(sim.object(unit).unwrap().anim.starts, starts);
        // Second call without a change: nothing.
        update_idle_anim(&mut sim, unit, 0, false, false, 0);
        // State change at rest: random start rand() % 1300 from the CRT generator (first value 41).
        sim.object_mut(unit).unwrap().dirty = true;
        update_idle_anim(&mut sim, unit, 5, false, false, 0);
        let o = sim.object(unit).unwrap();
        assert_eq!((o.anim.wanted, o.anim.start_ms), (3, 41));
        assert!(!o.dirty);
    }
}
