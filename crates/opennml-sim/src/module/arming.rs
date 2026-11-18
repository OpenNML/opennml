//! Module 39 `ARMING` (`CPropertyArming`, constructor `0x004FF4C0`, vtable `0x00622120`, re/arming-gates 1.A, 3.A):
//! arming and disarming of the workers. The module has no tick, no `ReadParams` and no state of its own except the flag
//! word of the base (`+0x0C`, init: data value 400001); it therefore stays [`super::ModuleKind::Inert`], and
//! [`super::query`] routes index 39 here. Arming costs nothing, has no duration and no animation: the unit walks to the
//! nearest friendly store and toggles `ISARMED` on arrival, after which the variant mask reselects the modules (bit 6
//! unarmed, bit 7 armed with upgrade).

use opennml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::dispatch_command;
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;
use crate::variants::update_variant_mask;
use crate::world::set_object_event_bit;

/// Module index of arming.
pub const INDEX: usize = 0x27;
/// Flag `ISARMED` in the flag word of the module (`MID_PROP_ARMING_ISARMED 295` = `0x127`: low byte module 39, `0x100`
/// the bit).
pub const ISARMED: u32 = 0x100;

/// Messages to module 39 (query `0x004FF790`, re/arming-gates 2.A).
pub mod msg {
    /// Walk to the store (from the FIFO with abort, commands `0x38`/`0x72`).
    pub const GO_TO_ARMORY: u32 = 0x127;
    /// Success message of the walk order: arrived, toggle.
    pub const ARRIVED: u32 = 0x227;
    /// Fault message of the walk order (`0x0051D7F0`: nothing).
    pub const FAULT: u32 = 0x327;
}

/// `ISARMED` set (regardless of whether the module is switched on).
pub fn is_armed(sim: &Sim, id: ObjectId) -> bool {
    sim.module(id, INDEX)
        .is_some_and(|m| m.base.flags & ISARMED != 0)
}

/// `MID_PROP_ARMING_ISARMED` as the mask of the arrival check (C-83: no effect, except that `0x100` sets no bits in the
/// flag word, 400001 is 0 in the shipped data).
const ISARMED_MID: u32 = 0x127;
/// Store filter: `FRIENDLIST | HANDLETYPE_ANDMODE | built-in position check | ANDALL | HANDLETYPE`.
const STORE_FILTER_MODE: u32 = 0x872;
/// Module 3 with all three store bits `GOLDLAGER | LUMBERYARD | GRANARY` (finished buildings only).
const STORE_BITS: u32 = 0x3803;
/// Event bit for scripts after the toggle.
const EVENT_BIT: u32 = 6;

/// `CPropertyArming::Query` (`0x004FF790`): `0x127`, `0x227`, `0x327`; otherwise the base (-1).
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        msg::GO_TO_ARMORY => go_to_armory(sim, id),
        msg::ARRIVED => on_arrived(sim, id),
        msg::FAULT => 0,
        _ => -1,
    }
}

/// `PropertyArming_GoToArmory` (`0x004FF590`, `0x127` after `AbortAllModules` from the FIFO): the nearest friendly (own
/// or allied, `FRIENDLIST`) building with all three store bits and a docking place in the region of the unit (built-in
/// check `0x20` without a reference object: `0x2C03 {layer, region}` != -1), on the whole map with edge distance (own
/// hard radius). No store -> 0, silent (W-265: the work is already aborted). Otherwise the walk order
/// `{0x200, [store, Fixed(−200), 1]}` with success `0x227` and fault `0x327` to itself (synchronous), return value 1.
fn go_to_armory(sim: &mut Sim, id: ObjectId) -> i32 {
    let layer = crate::module::gather::walk_layer(sim, id);
    let region = i32::from(crate::world::region_id_at(sim, id, layer, -1, -1));
    let f = MatchFilter {
        mode: STORE_FILTER_MODE,
        match_bits: STORE_BITS,
        ref_object: 0,
        layer,
        region,
        ..MatchFilter::constructed()
    };
    let Some(o) = sim.object(id) else { return 0 };
    let (pos, owner, hard) = (
        (o.grid.x.0, o.grid.z.0),
        i32::from(o.owner()),
        o.hard_radius,
    );
    let Some(store) = find_nearest_matching(sim, pos, &f, -1, owner, hard) else {
        return 0;
    };
    let go = Envelope {
        msg: Message::new(0x200, &[store.as_i32(), NO_DIRECTION, 1]),
        success: Some(Message::new(msg::ARRIVED, &[])),
        fault: Some(Message::new(msg::FAULT, &[])),
        success_target: id,
        fault_target: id,
    };
    dispatch_command(sim, id, &go);
    1
}

/// `PropertyArming_OnArrived` (`0x004FF750`, `0x227`): toggle according to the **current** flag (W-265) -
/// `flags & 0x127 == 0` arms (`0x004FF510`), otherwise disarms (`0x004FF540`; C-83 literal, no effect); then recompute
/// the variant mask (`UpdateVariantMask(−1, 0)`: targets 485/486 read the new flag) and set event bit 6.
/// `NotifyUiIfSelected` is UI. Without the arming upgrade neither bit 6 nor bit 7 remains: walk, stand, dying and
/// attack switch off (W-264, reachable only via foreign commands). Module 39 itself also goes off
/// (`0x00467B60`/`0x0054CF30` switch off without exception, `0x0046E800`/`0x004668A0` deliver only to switched-on
/// modules): neither `0x38` nor `0x72` arrives afterwards, the unit stays permanently paralyzed (re/arming-gates 3.A).
fn on_arrived(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(m) = sim.module_mut(id, INDEX) {
        if m.base.flags & ISARMED_MID == 0 {
            m.base.flags |= ISARMED;
        } else {
            m.base.flags &= !ISARMED;
        }
    }
    update_variant_mask(sim, id, u32::MAX, 0);
    set_object_event_bit(sim, id, EVENT_BIT, true);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandKind;
    use crate::command::testkit::{run, run_until};
    use crate::message::Posted;
    use crate::messaging::{post_message, send};
    use crate::module::ModuleKind;
    use crate::module::gather::status as gather_status;
    use crate::module::{
        attack, construct, dying, fight_far, fight_near, gather::lumber, shoot, stand, walk,
    };
    use crate::save::checksum;
    use crate::state::SimState;
    use crate::upgrades::testkit::*;
    use crate::upgrades::{apply_upgrade, set_arrow_type};
    use crate::variants::player_variant_mask;
    use crate::world::object_event_bits;

    /// Arming upgrade of the Spaniards (command bar 485 `REQUIRED_GUID 2806`).
    const ARM_UPGRADE: u32 = 2806;
    /// Ship hull: upgrade of the ship without variant effect.
    const HULL_UPGRADE: u32 = 2822;
    /// Event bit of arming (scripts).
    const EVENT_BIT: u32 = 1 << 6;

    fn enabled(sim: &Sim, id: ObjectId, index: usize) -> bool {
        sim.module(id, index).is_some_and(|m| m.base.enabled)
    }

    fn prop_bits(sim: &Sim, id: ObjectId, index: usize) -> u32 {
        sim.module(id, index).map_or(0, |m| m.base.prop_bits)
    }

    /// Walk order of the unit: `FINDTO` message, success and fault message with targets.
    fn walk_order(sim: &Sim, id: ObjectId) -> (Option<Message>, Envelope) {
        let m = sim.module(id, walk::INDEX).expect("Walk");
        let find_to = match &m.kind {
            ModuleKind::Walk(w) => w.find_to.clone(),
            _ => None,
        };
        let env = Envelope {
            msg: Message::new(0, &[]),
            success: m.base.success.clone(),
            fault: m.base.fault.clone(),
            success_target: m.base.success_target,
            fault_target: m.base.fault_target,
        };
        (find_to, env)
    }

    /// The expected walk order to the store `b` (re/arming-gates 9.A2).
    fn assert_walks_to(sim: &Sim, id: ObjectId, b: ObjectId) {
        let (find_to, env) = walk_order(sim, id);
        assert_eq!(
            find_to,
            Some(Message::new(0x200, &[b.as_i32(), NO_DIRECTION, 1])),
            "walk order {{0x200, [store, -819200, 1]}}"
        );
        assert_eq!(env.success, Some(Message::new(msg::ARRIVED, &[])));
        assert_eq!(env.fault, Some(Message::new(msg::FAULT, &[])));
        assert_eq!((env.success_target, env.fault_target), (id, id));
    }

    fn fifo(sim: &Sim, id: ObjectId) -> Vec<Posted> {
        sim.object(id)
            .map_or_else(Vec::new, |o| o.fifo.iter().cloned().collect())
    }

    fn go_to_armory() -> Posted {
        Posted::Message {
            msg: Message::new(msg::GO_TO_ARMORY, &[]),
            abort: true,
        }
    }

    fn toggle(units: &[ObjectId], arm: bool) -> CommandKind {
        CommandKind::ToggleArming {
            units: units.to_vec(),
            arm,
        }
    }

    /// Steps until the walk order of arming is in place (the FIFO runs in the object tick).
    fn until_walking(state: &mut SimState, rules: &crate::rules::Rules, id: ObjectId) {
        run_until(state, rules, 4, |sim| {
            walk_order(sim, id).1.success == Some(Message::new(msg::ARRIVED, &[]))
        });
    }

    // QUIRK W-266: kept
    #[test]
    fn arming_walks_to_the_nearest_friendly_store_and_switches_variants() {
        // re/arming-gates 9.A2: HQ (finished) 20 tiles away, warehouse (0x3800) 30, construction site 5 -> target HQ.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        let hq = spawn(&mut sim, HQ, 0, (30, 10));
        spawn(&mut sim, STORE, 0, (10, 40));
        let s = site(&mut sim, STORE, 0, (10, 15));
        let store_bits = |sim: &Sim, b| sim.module(b, 3).map_or(0, |m| m.base.flags & 0x3800);
        assert_eq!(
            (store_bits(&sim, hq), store_bits(&sim, s)),
            (0x3800, 0),
            "construction site without store bits"
        );
        apply_upgrade(&mut sim, 0, ARM_UPGRADE, false);
        assert!(enabled(&sim, c, fight_near::INDEX) && !enabled(&sim, c, fight_far::INDEX));
        run(&mut sim, 0, toggle(&[c], true));
        assert_eq!(fifo(&sim, c), [go_to_armory()]);
        until_walking(&mut state, &rules, c);
        let sim = Sim::new(&mut state, &rules);
        assert_walks_to(&sim, c, hq);
        assert!(!is_armed(&sim, c), "not until arrival");
        run_until(&mut state, &rules, 3000, |sim| is_armed(sim, c));
        let mut sim = Sim::new(&mut state, &rules);
        // Player mask with the test rules: bits 0, 7, 12 (0x108D in the shipped data according to 9.A).
        assert_eq!(player_variant_mask(&mut sim, c), 0x1081);
        for n in [
            walk::INDEX,
            stand::INDEX,
            dying::INDEX,
            attack::INDEX,
            fight_far::INDEX,
            shoot::INDEX,
        ] {
            assert_eq!(prop_bits(&sim, c, n), 1 << 7, "module {n} in variant 7");
            assert!(enabled(&sim, c, n), "module {n} switched on");
        }
        for n in [fight_near::INDEX, lumber::INDEX, construct::INDEX] {
            assert!(!enabled(&sim, c, n), "module {n} off");
        }
        assert_ne!(object_event_bits(&sim, c) & EVENT_BIT, 0, "event bit 6");
    }

    // QUIRK W-266: kept
    #[test]
    fn an_allied_store_counts() {
        // Allied slot 1 with a store 10 tiles away, own HQ 20 away: FRIENDLIST -> its store.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        spawn(&mut sim, HQ, 0, (30, 10));
        let ally = spawn(&mut sim, STORE, 1, (10, 20));
        sim.state.diplomacy.friend[0] |= 1 << 1;
        assert!(sim.state.diplomacy.is_friend(0, 1));
        post_message(&mut sim, c, Message::new(msg::GO_TO_ARMORY, &[]), true);
        until_walking(&mut state, &rules, c);
        assert_walks_to(&Sim::new(&mut state, &rules), c, ally);
    }

    // QUIRK W-265: kept
    #[test]
    fn no_store_aborts_the_work_silently() {
        // W-265: lumberjack without a store - the FIFO abort mark has already ended the work, the search finds nothing
        // and it stays silent.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        spawn(&mut sim, STORE, 1, (20, 10)); // enemy
        site(&mut sim, STORE, 0, (10, 15)); // without store bits
        match &mut sim.module_mut(c, lumber::INDEX).unwrap().kind {
            ModuleKind::Lumber(l) => l.gather.status = gather_status::TO_SOURCE,
            _ => panic!("module 8 is not a lumberjack"),
        }
        assert_eq!(send(&mut sim, c, &Message::new(msg::GO_TO_ARMORY, &[])), 0);
        assert_eq!(walk_order(&sim, c).0, None, "no walk order");
        post_message(&mut sim, c, Message::new(msg::GO_TO_ARMORY, &[]), true);
        let before = object_event_bits(&sim, c);
        run_until(&mut state, &rules, 4, |sim| fifo(sim, c).is_empty());
        let sim = Sim::new(&mut state, &rules);
        match &sim.module(c, lumber::INDEX).unwrap().kind {
            ModuleKind::Lumber(l) => assert_eq!(l.gather.status, gather_status::IDLE),
            _ => unreachable!(),
        }
        assert_eq!(walk_order(&sim, c).0, None);
        assert!(!is_armed(&sim, c));
        assert_eq!(object_event_bits(&sim, c), before);
    }

    // QUIRK W-264: deviation
    #[test]
    fn arming_without_the_upgrade_paralyses_the_unit() {
        // W-264, only via the direct `post_message` (the command rejects): neither bit 6 nor bit 7.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        spawn(&mut sim, HQ, 0, (14, 10));
        let before = checksum(sim.state);
        run(&mut sim, 0, toggle(&[c], true));
        assert_eq!(checksum(sim.state), before, "command rejected");
        post_message(&mut sim, c, Message::new(msg::GO_TO_ARMORY, &[]), true);
        run_until(&mut state, &rules, 3000, |sim| is_armed(sim, c));
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(player_variant_mask(&mut sim, c) & (1 << 6 | 1 << 7), 0);
        for n in [
            walk::INDEX,
            stand::INDEX,
            dying::INDEX,
            attack::INDEX,
            INDEX,
        ] {
            assert!(!enabled(&sim, c, n), "module {n} off: paralyzed");
        }
        // Module 39 is itself off: a further `0x127` no longer arrives (permanently paralyzed).
        assert_eq!(send(&mut sim, c, &Message::new(msg::GO_TO_ARMORY, &[])), -1);
    }

    // QUIRK W-265: kept
    #[test]
    fn toggling_follows_the_current_flag_not_the_command() {
        // W-265: ISARMED set, FightFar off (variant 6) -> "Arm" disarms.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        spawn(&mut sim, HQ, 0, (14, 10));
        apply_upgrade(&mut sim, 0, ARM_UPGRADE, false);
        set_armed(&mut sim, c, true);
        assert!(!enabled(&sim, c, fight_far::INDEX));
        run(&mut sim, 0, toggle(&[c], true));
        assert_eq!(fifo(&sim, c), [go_to_armory()]);
        run_until(&mut state, &rules, 3000, |sim| {
            object_event_bits(sim, c) & EVENT_BIT != 0
        });
        let sim = Sim::new(&mut state, &rules);
        assert!(!is_armed(&sim, c), "disarmed");
        assert_eq!(prop_bits(&sim, c, walk::INDEX), 1 << 6);
        assert!(!enabled(&sim, c, fight_far::INDEX));
    }

    // QUIRK W-264: deviation
    #[test]
    fn arming_commands_check_the_upgrade_like_the_bar() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        let armed = spawn(&mut sim, COLONIST, 0, (12, 10));
        let foreign = spawn(&mut sim, COLONIST, 1, (14, 10));
        let pikeman = spawn(&mut sim, PIKEMAN, 0, (16, 10));
        spawn(&mut sim, HQ, 0, (20, 20));
        // Without 2806: button 485 locked -> rejected.
        let before = checksum(sim.state);
        run(&mut sim, 0, toggle(&[c], true));
        assert_eq!(checksum(sim.state), before, "without arming upgrade");
        let check = crate::command::upgrade::check_toggle_arming;
        assert_eq!(check(&sim, 0, &[c], true).unwrap_err(), "Arm not possible");
        apply_upgrade(&mut sim, 0, ARM_UPGRADE, false);
        // Foreign unit and unit without module 39 drop out.
        run(&mut sim, 0, toggle(&[foreign, pikeman, c], true));
        assert_eq!(fifo(&sim, c), [go_to_armory()]);
        assert!(fifo(&sim, foreign).is_empty() && fifo(&sim, pikeman).is_empty());
        let before = checksum(sim.state);
        run(&mut sim, 0, toggle(&[foreign, pikeman], true));
        assert_eq!(checksum(sim.state), before, "no matching unit");
        // Disarming only with FightFar switched on.
        run(&mut sim, 0, toggle(&[armed], false));
        assert_eq!(checksum(sim.state), before, "disarming without FightFar");
        assert_eq!(
            check(&sim, 0, &[armed], false).unwrap_err(),
            "Disarm not possible"
        );
        set_armed(&mut sim, armed, true);
        update_variant_mask(&mut sim, armed, u32::MAX, 0);
        assert!(enabled(&sim, armed, fight_far::INDEX));
        run(&mut sim, 0, toggle(&[armed], false));
        assert_eq!(fifo(&sim, armed), [go_to_armory()]);
    }

    // QUIRK C-224: deviation
    #[test]
    fn arrow_type_commands_are_rejected_for_players() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ship = spawn(&mut sim, SHIP, 0, (10, 10));
        let before = checksum(sim.state);
        for fire in [true, false] {
            let kind = CommandKind::SetArrowType {
                units: vec![ship],
                fire,
            };
            run(&mut sim, 0, kind);
            assert_eq!(checksum(sim.state), before, "fire arrow command {fire}");
        }
        assert_eq!(prop_bits(&sim, ship, walk::INDEX), 1 << 12);
    }

    // QUIRK C-96: kept
    // QUIRK C-224: deviation
    #[test]
    fn set_arrow_type_arms_the_transport_ship_without_upgrade() {
        // C-224: ship 1108 without 2823 -> variant 13 (attack, shot on), until the next mask recalculation (upgrade of
        // the slot that affects 1108).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ship = spawn(&mut sim, SHIP, 0, (10, 10));
        assert_eq!(prop_bits(&sim, ship, walk::INDEX), 1 << 12);
        assert!(!enabled(&sim, ship, attack::INDEX) && !enabled(&sim, ship, shoot::INDEX));
        set_arrow_type(&mut sim, &[ship], true);
        for n in [walk::INDEX, stand::INDEX, attack::INDEX, shoot::INDEX] {
            assert_eq!(
                prop_bits(&sim, ship, n),
                1 << 13,
                "module {n} in variant 13"
            );
            assert!(enabled(&sim, ship, n), "module {n} switched on");
        }
        apply_upgrade(&mut sim, 0, HULL_UPGRADE, false);
        assert_eq!(prop_bits(&sim, ship, walk::INDEX), 1 << 12);
        assert!(!enabled(&sim, ship, attack::INDEX) && !enabled(&sim, ship, shoot::INDEX));
        // Opposite direction: with 2823, `0x41` disarms the ship.
        apply_upgrade(&mut sim, 0, 2823, false);
        assert!(enabled(&sim, ship, attack::INDEX));
        set_arrow_type(&mut sim, &[ship], false);
        assert_eq!(prop_bits(&sim, ship, walk::INDEX), 1 << 12);
        assert!(!enabled(&sim, ship, attack::INDEX) && !enabled(&sim, ship, shoot::INDEX));
    }
}
