//! Module 6 `CPropertyResource` (re/gathering): gold deposits (3500), fertile soil
//! (3503) and fishing baskets (3505). The amount is an attribute value with a fraction (gold 26, otherwise 24)
//! from the `[CONFIG]` of the `.wld`; mines, fields and fishing boats take from it via `0x106`/`0x206`.
//! Exhausted deposits remain. The fish school of the baskets is presentation
//! (`nml-effects`).

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::ModuleKind;

pub const INDEX: usize = 6;
/// Attribute of the gold amount (config 26).
pub const GOLD_AMOUNT: u32 = 26;
/// Attribute of the food amount of soil and fishing basket (config 24).
pub const FOOD_AMOUNT: u32 = 24;
/// Fishing basket: invisible until the first access.
pub const FISH_BASKET: u32 = 3505;
/// Module flag `VISIBLE` (`Module_HasFlags(0x406)`).
const VISIBLE: u32 = 0x400;

/// Queries of the resource (`PropertyResource_Query` `0x00521310`).
pub mod msg {
    /// `{n}` -> amount taken (gold from the mine).
    pub const TAKE_GOLD: u32 = 0x106;
    /// `{n}` -> amount taken (food from the field or from the basket); equivalent to `0x106`.
    pub const TAKE_FOOD: u32 = 0x206;
}

/// State of `CPropertyResource` (`PropertyResource_Construct` `0x005211E0`): the pointer to the
/// attribute block of the object (`+0x70`) is access via the object here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// `m_ResourceID` (`+0x74`): attribute of the amount (26 gold, 24 food). Without an init branch
    /// the original leaves the field uninitialized (quirk, no case in the shipped data); here 0 = none.
    pub resource_id: u32,
}

/// `PropertyResource_Init` (`0x00521340`): flags 70002; `VISIBLE` -> render bits 3. **Quirk:**
/// the branches test the raw flags with `TEST 0x106` or `TEST 0x206` (including bits 1/2):
/// gold (amount 26, occupancy of 3 x 3), soil (24, 7 x 7), otherwise only the fishing basket (24, without
/// occupancy); other resources stay without an amount. The amount replaces the base value with the
/// `[CONFIG]` value of the `.wld` (`GameEngine+0x2B4`; if it is missing: 0, even over a value from
/// `default.dat`), is kept as the current value and filled (fraction 100,000). The fish school
/// of the basket (`FishEffect_Construct` `0x004F1330`) draws 10 CRT `rand()` calls from the
/// shared state in the original; it is presentation and arises in the viewer (`nml-effects`) - visual
/// randomness stays separate, the simulation state draws nothing here.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = super::init_flags(INDEX, desc);
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(o) = sim.object_mut(id)
        && flags & VISIBLE != 0
    {
        o.render_bits = 3;
    }
    let guid = sim.object(id).map_or(0, |o| o.guid);
    let config = if flags & 0x106 != 0 {
        GOLD_AMOUNT
    } else if flags & 0x206 != 0 || guid == FISH_BASKET {
        FOOD_AMOUNT
    } else {
        return;
    };
    // Gold 3 x 3, soil 7 x 7 (never cleared; the basket falls through both tests).
    crate::footprint::resource_occupancy(sim, id, flags);
    let amount = sim.state.object_config.get(&config).copied().unwrap_or(0);
    if let Some(m) = sim.module_mut(id, INDEX)
        && let ModuleKind::Resource(r) = &mut m.kind
    {
        r.resource_id = config;
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.set_modifier(config, amount, false, 0);
        o.attributes.set_tracks_current(config, true);
        o.attributes.fill_current(config);
    }
}

/// `PropertyResource_Query` (`0x00521310`): `0x106` and `0x206` both take
/// ([`take`]); otherwise -1. `OnMessage` and the tick are the base.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::TAKE_GOLD | msg::TAKE_FOOD => take(sim, id, m.arg(0)),
        _ => -1,
    }
}

/// `PropertyResource_OnTakeResourceMsg` (`0x00521250`): an invisible fishing basket becomes visible
/// at the first access (render bits 3; animation 0 at once and dummy effects are presentation ->
/// event [`SimEvent::Revealed`]); then take at most the remainder
/// (`AttributeSet_AddToCurrentValue` `0x0048FAF0`), return value not negative. From max > 21,474 on,
/// `Max * fraction` overflows and the deposit looks empty (**quirk**, no case in the shipped data).
pub fn take(sim: &mut Sim, id: ObjectId, want: i32) -> i32 {
    let Some(o) = sim.object_mut(id) else {
        return -1;
    };
    if o.guid == FISH_BASKET && o.render_bits & 1 == 0 {
        o.render_bits = 3;
        sim.state.events.push(SimEvent::Revealed { object: id });
    }
    let config = match sim.module(id, INDEX).map(|m| &m.kind) {
        Some(ModuleKind::Resource(r)) => r.resource_id,
        _ => 0,
    };
    let Some(o) = sim.object(id) else { return -1 };
    let p = sim.type_upgrade_percent(o.owner(), o.guid, config);
    let current = o.attributes.current_value(config, p);
    let n = if current < want { current } else { want };
    if let Some(o) = sim.object_mut(id) {
        o.attributes
            .add_to_current_value(config, n.wrapping_neg(), p);
    }
    n.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::send;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    /// Gold 0x500, soil 0x600, basket 0 (shipped data), all category nature (slot 10).
    pub(crate) const TYPES: &str = "GUID 3500\nNAME Goldvorkommen\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1280\nGUID 3503\nNAME Boden\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1536\nGUID 3505\nNAME Fischerkorb\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 0\n";

    fn spawn(sim: &mut Sim, guid: u32, amount: Option<(u32, i32)>, tile: (i32, i32)) -> ObjectId {
        sim.state.object_config = amount.into_iter().collect();
        let pos = [
            (tile.0 as f32 + 0.5) * 128.0,
            0.0,
            (tile.1 as f32 + 0.5) * 128.0,
        ];
        create_object(sim, &mut ObjectDesc::new(guid, 10, pos)).unwrap()
    }

    fn take_one(sim: &mut Sim, id: ObjectId, code: u32) -> i32 {
        send(sim, id, &Message::new(code, &[1]))
    }

    // QUIRK W-49: kept
    #[test]
    fn deposits_give_exactly_their_config_amount_below_the_overflow_limit() {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gold = spawn(&mut sim, 3500, Some((26, 7500)), (5, 5));
        let r = match &sim.module(gold, INDEX).unwrap().kind {
            ModuleKind::Resource(r) => r.clone(),
            k => panic!("{k:?}"),
        };
        assert_eq!(r.resource_id, GOLD_AMOUNT);
        let mut n = 0;
        while take_one(&mut sim, gold, msg::TAKE_GOLD) == 1 {
            n += 1;
        }
        assert_eq!(n, 7500);
        // Exhausted: remains, yields 0.
        assert_eq!(take_one(&mut sim, gold, msg::TAKE_GOLD), 0);
        assert!(sim.object(gold).is_some());
        // From max > 21,474 on, `Max * fraction` overflows: the deposit looks empty (quirk).
        for (amount, x) in [(21_475, 10), (30_000, 15)] {
            let big = spawn(&mut sim, 3500, Some((26, amount)), (x, 10));
            assert_eq!(take_one(&mut sim, big, msg::TAKE_GOLD), 0);
        }
        // Limit below that: 21,474 full.
        let edge = spawn(&mut sim, 3500, Some((26, 21_474)), (20, 10));
        assert_eq!(
            send(&mut sim, edge, &Message::new(0x106, &[30_000])),
            21_474
        );
    }

    #[test]
    fn fields_and_baskets_share_the_food_attribute_and_both_codes_take() {
        let mut rules = Rules::synthetic(TYPES, 32);
        // A base value from `default.dat` does not count: without `[CONFIG]` the amount is 0.
        rules
            .defaults
            .insert(3503, [(24, 500)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let field = spawn(&mut sim, 3503, Some((24, 10)), (8, 8));
        assert_eq!(send(&mut sim, field, &Message::new(0x206, &[4])), 4);
        assert_eq!(send(&mut sim, field, &Message::new(0x106, &[4])), 4);
        assert_eq!(send(&mut sim, field, &Message::new(0x206, &[4])), 2);
        assert_eq!(send(&mut sim, field, &Message::new(0x306, &[4])), -1);
        // Negative wishes give nothing (and refill at most up to max).
        assert_eq!(send(&mut sim, field, &Message::new(0x206, &[-3])), 0);
        let bare = spawn(&mut sim, 3503, None, (20, 20));
        assert_eq!(take_one(&mut sim, bare, msg::TAKE_FOOD), 0);
        // Visible and occupied: soil 7 x 7, gold 3 x 3.
        assert_eq!(sim.object(field).unwrap().render_bits, 3);
        assert!(sim.state.occupancy.test(5, 5) && sim.state.occupancy.test(11, 11));
        assert!(!sim.state.occupancy.test(4, 8) && !sim.state.occupancy.test(12, 8));
    }

    // QUIRK W-60: kept
    #[test]
    fn fish_baskets_become_visible_at_the_first_access() {
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let crt = state.crt;
        let mut sim = Sim::new(&mut state, &rules);
        let basket = spawn(&mut sim, 3505, Some((24, 500)), (6, 6));
        // Invisible, no occupancy, no draw from the CRT state (fish school = presentation).
        assert_eq!(sim.object(basket).unwrap().render_bits, 0);
        assert!(!sim.state.occupancy.test(6, 6));
        assert_eq!(sim.state.crt, crt);
        sim.state.events.clear();
        assert_eq!(take_one(&mut sim, basket, msg::TAKE_FOOD), 1);
        assert_eq!(sim.object(basket).unwrap().render_bits, 3);
        assert_eq!(sim.state.events, [SimEvent::Revealed { object: basket }]);
        // Only once.
        assert_eq!(take_one(&mut sim, basket, msg::TAKE_FOOD), 1);
        assert_eq!(sim.state.events.len(), 1);
        let r = match &sim.module(basket, INDEX).unwrap().kind {
            ModuleKind::Resource(r) => r.clone(),
            k => panic!("{k:?}"),
        };
        assert_eq!(r.resource_id, FOOD_AMOUNT);
        assert_eq!(
            sim.object(basket)
                .unwrap()
                .attributes
                .current_value(FOOD_AMOUNT, 0),
            498
        );
    }
}
