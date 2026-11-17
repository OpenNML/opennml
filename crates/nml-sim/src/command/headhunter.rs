//! Command `0x2C` hire headhunter (`GameWorld_CmdHireHeadhunter` `0x0048A8F0`,
//! re/headhunter-robbing 2.4, 3.1) and the menu functions of the command bar (re/headhunter-robbing 2.5): price
//! ([`hire_price`], `FUN_0047DEE0`), target players ([`hire_players`], `FUN_0044A8C0`) and target GUIDs
//! ([`hire_targets`], `FUN_0044AA10`).

use nml_formats::gifcon::button_kind;

use super::validate::{button_locked, is_dead, is_finished, menu_button};
use super::{CommandKind, owns, reject, script_path};
use crate::economy::guid_count;
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::Message;
use crate::messaging::{post_message, send, send_mut};
use crate::module::ModuleKind;
use crate::module::building::msg as building_msg;
use crate::module::headhunter as hunter;
use crate::rules::Rules;
use crate::setup::player_color;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState, TextArg};

/// `texte.deu` 10002, message about missing gold (red, 4000 ms, only to the payer, re/headhunter-robbing 7).
pub const TEXT_NO_GOLD: i32 = 10002;
/// `texte.deu` 10010, message about hiring with two `%s` (payer, type name of the headhunter; green, 6000 ms, to all).
pub const TEXT_HIRED: i32 = 10010;
/// `texte.deu` 10011, success message with one `%s` (type name of the headhunter; green, 3000 ms, to all).
pub const TEXT_SUCCESS: i32 = 10011;
/// Sound on hiring (923, `sfx/otherevents.ini`).
pub const SFX_START: &str = "SFX_HEADHUNTERSTART";
/// Sound on success (924).
pub const SFX_SUCCESS: &str = "SFX_HEADHUNTERSUCCESS";
/// Red of message 10002.
pub const COLOR_RED: u32 = 0xFF0000;
/// Green of messages 10010/10011.
pub const COLOR_GREEN: u32 = 0x00FF00;

/// Elites per nation (0 WID, 1 PRI, 2 SPA, 3 SID, 4 ENG, 5 PAT; `FUN_0044AA10`, re/headhunter-robbing 2.5) in
/// menu order.
pub const ELITES: [[u32; 3]; 6] = [
    [1410, 1411, 1412],
    [1714, 1715, 1716],
    [1112, 1113, 1115],
    [2614, 2615, 2616],
    [2013, 2014, 2015],
    [2313, 2314, 2315],
];
/// Headquarters per nation (target button only without an elite and with an already existing headhunter, C-228).
pub const NATION_HQ: [u32; 6] = [1300, 1600, 1000, 2500, 1900, 2200];
/// Color buttons of the target players: `BUTTONMSG 470 + color` (`FUN_0044A8C0`).
pub const COLOR_BUTTON_BASE: u32 = 470;
/// Button "Back" of the two submenus.
pub const BACK_BUTTON: u32 = 409;

/// `FUN_0047DEE0` (tooltip price, re/headhunter-robbing 2.5) on state and rules (the viewer queries
/// without `&mut`). If the headhunter of the kind exists ([`hunter::current`], even dead), it
/// reproduces `0x222`: module 34 switched on -> `m_CurReward`, switched off (dead, `send` -> -1, the
/// arguments stay 0) -> 0 (C-338). Otherwise config 12 of the value set at slot 8 like `Init` (a missing
/// value counts as 0), without a value set 500.
pub fn hire_price_in(state: &SimState, rules: &Rules, art: usize) -> i32 {
    if let Some(id) = hunter::current(state, art) {
        let module = state
            .object(id, false)
            .and_then(|o| o.module(hunter::INDEX));
        return match module {
            Some(m) if m.base.enabled => match &m.kind {
                ModuleKind::HeadHunter(h) => h.cur_reward,
                _ => 0,
            },
            _ => 0,
        };
    }
    let Some(&guid) = hunter::GUIDS.get(art) else {
        return hunter::DEFAULT_PRICE;
    };
    rules
        .type_value_set(8, guid)
        .map_or(hunter::DEFAULT_PRICE, |set| {
            set.get(&12).copied().unwrap_or(0)
        })
}

/// `FUN_0047DEE0`: [`hire_price_in`] of the running simulation.
pub fn hire_price(sim: &Sim, art: usize) -> i32 {
    hire_price_in(sim.state, sim.rules, art)
}

/// `FUN_0044A8C0` (level 1 of the menu, re/headhunter-robbing 2.5, D-239): the slots 0...7 ascending that are
/// still playing (`Player_GetResultState == 0`), are an enemy of `player` and whose color button `470 + color` is
/// not locked for `player` (`Player_IsGuidDisabled`). Only the UI remembers the kind.
pub fn hire_players(sim: &Sim, player: u8) -> Vec<u8> {
    (0..PLAYER_SLOTS as u8)
        .filter(|&q| {
            let slot = i32::from(q);
            sim.state
                .players
                .get(usize::from(q))
                .is_some_and(|p| p.result == 0)
                && sim.state.diplomacy.is_enemy(i32::from(player), slot)
                && !crate::world::is_guid_disabled(
                    sim,
                    i32::from(player),
                    COLOR_BUTTON_BASE.wrapping_add(player_color(sim, slot) as u32),
                )
        })
        .collect()
}

/// `FUN_0044AA10` (level 2 of the menu, re/headhunter-robbing 2.5, C-228): the three elites of the nation of `target`
/// in table order that `target` owns (`Player_GetGuidCount ≠ 0`); only if none is among them **and** the headhunter of
/// the kind already exists ([`hunter::current`]), the HQ of the nation. Nation outside 0...5: nothing.
pub fn hire_targets(sim: &Sim, art: usize, target: u8) -> Vec<u32> {
    let Some(nation) = sim
        .state
        .players
        .get(usize::from(target))
        .and_then(|p| usize::try_from(p.nation).ok())
    else {
        return Vec::new();
    };
    let mut targets: Vec<u32> = ELITES
        .get(nation)
        .into_iter()
        .flatten()
        .copied()
        .filter(|&g| guid_count(sim, i32::from(target), g) != 0)
        .collect();
    if targets.is_empty() && hunter::current(sim.state, art).is_some() {
        targets.extend(NATION_HQ.get(nation));
    }
    targets
}

/// Check like the command bar (A8; deviates from K-316): the building belongs to the sender, lives and
/// is finished; the kind is 0...2 and its button `SELECTHEADHUNTER` is in the menu of the building
/// type, not locked; the target player is in [`hire_players`], the target GUID in [`hire_targets`].
///
/// On the script path ([`script_path`], action 380, E-218) building (own/living/finished),
/// button, lock and the target player and target GUID menus are dropped: the original does not check them
/// (`GameWorld_CmdHireHeadhunter` `0x0048A8F0`). The kind 0...2 remains. Target player unchecked like the
/// original; a payer from slot 8 on pays nothing (`0x0048A8F0`, E-218). Without a headhunter of the kind the
/// check "building exists" remains (re/headhunter-robbing 3.1). Gold and message 10002 are checked by
/// [`execute`]. Return value: the kind, otherwise the reason.
fn check(sim: &Sim, player: u8, kind: &CommandKind) -> Result<usize, String> {
    let CommandKind::HireHeadhunter {
        art,
        target_player,
        target_guid,
        building,
    } = *kind
    else {
        return Err("not a hire".into());
    };
    let script = script_path(sim);
    if !script
        && (!owns(sim, player, building) || is_dead(sim, building) || !is_finished(sim, building))
    {
        return Err(format!(
            "building {} not own, dead or unfinished",
            building.0
        ));
    }
    let Some(a) = usize::try_from(art)
        .ok()
        .filter(|&a| a < hunter::GUIDS.len())
    else {
        return Err(format!("kind {art} invalid (allowed 0 to 2)"));
    };
    if script {
        // Without a headhunter of the kind the original looks the building up (`GetObjectById(building, 0)`) and
        // returns before the payment if it is missing (re/headhunter-robbing 3.1).
        let has_hunter = hunter::current(sim.state, a).is_some();
        if !has_hunter && sim.object(building).is_none() {
            return Err(format!("building {} not present", building.0));
        }
        return Ok(a);
    }
    let menu = sim.object(building).map_or(0, |o| o.guid);
    let button = menu_button(sim.rules, menu, |b| {
        b.kind() == button_kind::SELECTHEADHUNTER && b.msg == hunter::GUIDS[a] as i32
    });
    if button.is_none_or(|b| button_locked(sim, i32::from(player), b)) {
        return Err(format!(
            "type {menu} has no free button {}",
            hunter::GUIDS[a]
        ));
    }
    let Some(target) = u8::try_from(target_player)
        .ok()
        .filter(|t| hire_players(sim, player).contains(t))
    else {
        return Err(format!("target player {target_player} not selectable"));
    };
    if !u32::try_from(target_guid).is_ok_and(|g| hire_targets(sim, a, target).contains(&g)) {
        return Err(format!(
            "target GUID {target_guid} not selectable at slot {target}"
        ));
    }
    Ok(a)
}

/// Payer and customer on the script path (E-218): like the original the slot from the building ID,
/// if the headhunter of the kind already exists (the building is then not looked up), otherwise the
/// owner of the building. Every slot counts (from 8 on [`execute`] pays nothing, E-218); `None` only for a
/// missing building. `Command::player` is ignored (under `Origin::Script` no handler derives an
/// effect from the sender).
fn script_payer(sim: &Sim, building: ObjectId, has_hunter: bool) -> Option<u8> {
    let slot = if has_hunter {
        building.slot()
    } else {
        sim.object(building)?.owner()
    };
    Some(slot)
}

/// `GameWorld_CmdHireHeadhunter` (`0x0048A8F0`, re/headhunter-robbing 3.1) after [`check`]. The payer is
/// the sender (= owner of the building); on the script path the slot of the building
/// ([`script_payer`]):
/// 1. Price: if the headhunter of the kind exists (even dead), `0x222` asks synchronously (`send` with the
///    answer in the arguments); a dead one does not answer, the price is 0 (C-338). Otherwise [`hire_price`].
/// 2. Too little gold -> message 10002 only to the payer, end. Otherwise deduct the gold. A payer from
///    slot 8 on (script path only) pays nothing and is not checked (E-218).
/// 3. Only without a headhunter: `0x2D03 {GUID of the kind, 8}` to the building, result ignored; if there is
///    no headhunter afterwards, the gold stays gone (C-311).
/// 4. `0x122 {target_player, target_guid, payer}` into the FIFO of the headhunter (reaches it in its
///    next object tick, C-339), sound and message 10010 to all (D-238).
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind) {
    let art = match check(sim, player, kind) {
        Ok(art) => art,
        Err(why) => return reject(sim, player, "Hire headhunter", why),
    };
    let CommandKind::HireHeadhunter {
        target_player,
        target_guid,
        building,
        ..
    } = *kind
    else {
        return;
    };
    let existing = hunter::current(sim.state, art);
    let player = if script_path(sim) {
        script_payer(sim, building, existing.is_some()).unwrap_or(player)
    } else {
        player
    };
    let price = match existing {
        Some(hh) => {
            let mut q = Message::new(hunter::msg::PRICE, &[0; 4]);
            send_mut(sim, hh, &mut q);
            q.arg(0)
        }
        None => hire_price(sim, art),
    };
    // A payer from slot 8 on has no money table in the original: it pays nothing (E-218).
    let slot = usize::from(player);
    if slot < PLAYER_SLOTS {
        if sim.state.resource(slot, 0) < price {
            sim.state.events.push(SimEvent::TextMessage {
                to: Some(player),
                text: TEXT_NO_GOLD,
                args: Vec::new(),
                color: COLOR_RED,
                duration_ms: 4000,
            });
            return;
        }
        sim.state.players[slot].add_resource(0, -price);
    }
    let hh = match existing {
        Some(hh) => hh,
        None => {
            let spawn = Message::new(building_msg::SPAWN_UNIT, &[hunter::GUIDS[art] as i32, 8]);
            send(sim, building, &spawn);
            let Some(hh) = hunter::current(sim.state, art) else {
                return;
            };
            hh
        }
    };
    let guid = sim.object(hh).map_or(hunter::GUIDS[art], |o| o.guid);
    let hire = Message::new(
        hunter::msg::HIRE,
        &[target_player, target_guid, i32::from(player)],
    );
    post_message(sim, hh, hire, false);
    sim.state.events.push(SimEvent::Sound2d {
        name: SFX_START.into(),
    });
    sim.state.events.push(SimEvent::TextMessage {
        to: None,
        text: TEXT_HIRED,
        args: vec![TextArg::PlayerName(player), TextArg::TypeName(guid)],
        color: COLOR_GREEN,
        duration_ms: 6000,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::command::testkit::{assert_rejected, run};
    use crate::id::ObjectId;
    use crate::message::Message;
    use crate::messaging::send;
    use crate::module::ability_testkit as tk;
    use crate::module::building::msg::SET_MEETING_POINT;
    use crate::object::status;
    use crate::save::checksum;
    use crate::script::world::view::lock_button;
    use crate::state::{SimEvent, TextArg};
    use crate::world::{ObjectDesc, create_object};

    /// Player A (SPA, HQ 1000) and B (SID, HQ 2500).
    const A: u8 = 0;
    const B: u8 = 1;
    /// The SPA kind (4100).
    const SPA: i32 = 0;

    /// The two headquarters, 1000 gold each.
    fn hqs(sim: &mut Sim) -> (ObjectId, ObjectId) {
        let a = tk::spawn(sim, tk::HQ_SPA, 0, (10, 10));
        let b = tk::spawn(sim, tk::HQ_SID, 1, (40, 40));
        for p in [A, B] {
            sim.state.players[usize::from(p)].set_resource(0, 1000);
        }
        (a, b)
    }

    /// Elites as targets: A has a governor 1112, B a sheriff 2614 and an undertaker 2615 (far enough
    /// apart that nobody attacks).
    fn elites(sim: &mut Sim) {
        tk::spawn(sim, tk::GOVERNOR, 0, (10, 30));
        tk::spawn(sim, tk::SID_SHERIFF, 1, (40, 30));
        tk::spawn(sim, tk::UNDERTAKER, 1, (44, 30));
    }

    fn gold(sim: &Sim) -> [i32; 2] {
        [sim.state.resource(0, 0), sim.state.resource(1, 0)]
    }

    /// `(customer, paid, next price, target player, target GUID)` of the headhunter.
    fn terms(sim: &Sim, hh: ObjectId) -> (i32, i32, i32, i32, i32) {
        let h = hunter::headhunter(sim, hh).expect("module 34");
        (
            h.customer,
            h.customer_cost,
            h.cur_reward,
            h.target_player,
            h.target_guid,
        )
    }

    /// Messages and 2D sounds in order.
    fn announcements(events: &[SimEvent]) -> Vec<SimEvent> {
        events
            .iter()
            .filter(|e| matches!(e, SimEvent::TextMessage { .. } | SimEvent::Sound2d { .. }))
            .cloned()
            .collect()
    }

    /// Sound and message 10010 to all (arguments: name of the payer `payer`, type name of `guid`).
    fn hired(payer: u8, guid: u32) -> Vec<SimEvent> {
        vec![
            SimEvent::Sound2d {
                name: SFX_START.into(),
            },
            SimEvent::TextMessage {
                to: None,
                text: TEXT_HIRED,
                args: vec![TextArg::PlayerName(payer), TextArg::TypeName(guid)],
                color: COLOR_GREEN,
                duration_ms: 6000,
            },
        ]
    }

    #[test]
    fn hiring_follows_the_price_table() {
        // re/headhunter-robbing 9.1 (C-309): new 300, +50 per hire, refund to the predecessor.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, b_hq) = hqs(&mut sim);
        elites(&mut sim);
        assert_eq!(hire_price(&sim, 0), 300);
        let ev = tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        let hh = sim.state.headhunters[0].expect("headhunter hired");
        // C-310: slot 8, color of the HQ of A.
        let (a_color, b_color) = (
            sim.object(a_hq).unwrap().color,
            sim.object(b_hq).unwrap().color,
        );
        assert_ne!(a_color, b_color);
        let o = sim.object(hh).unwrap();
        assert_eq!((hh.slot(), o.guid, o.color), (8, tk::HUNTER_SPA, a_color));
        assert_eq!(gold(&sim), [700, 1000]);
        assert_eq!(terms(&sim, hh), (0, 300, 350, 1, 2614));
        assert_eq!(hire_price(&sim, 0), 350);
        assert_eq!(announcements(&ev), hired(A, tk::HUNTER_SPA));
        // B takes over for 350, A gets 300 back.
        let ev = tk::hire(&mut sim, B, SPA, 0, tk::GOVERNOR as i32, b_hq);
        assert_eq!(sim.state.headhunters[0], Some(hh));
        assert_eq!(gold(&sim), [1000, 650]);
        assert_eq!(terms(&sim, hh), (1, 350, 400, 0, 1112));
        assert_eq!(hire_price(&sim, 0), 400);
        assert_eq!(announcements(&ev), hired(B, tk::HUNTER_SPA));
        // A takes over again for 400, B gets 350 back.
        let ev = tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        assert_eq!(gold(&sim), [600, 1000]);
        assert_eq!(terms(&sim, hh), (0, 400, 450, 1, 2614));
        assert_eq!(hire_price(&sim, 0), 450);
        assert_eq!(announcements(&ev), hired(A, tk::HUNTER_SPA));
    }

    #[test]
    fn a_missing_value_set_prices_the_hunter_at_500() {
        // re/headhunter-robbing 9.1, `FUN_0047DEE0`/`0x0048A8F0`: without a value set at slot 8
        // (neither `slot_configs[8]` nor `defaults`) config 12 is not readable, the price is 500; the headhunter is
        // still created.
        let mut rules = tk::hunter_rules();
        rules.defaults.remove(&tk::HUNTER_SPA);
        assert!(rules.type_value_set(8, tk::HUNTER_SPA).is_none());
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        assert_eq!(hire_price(&sim, 0), 500);
        tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        let hh = sim.state.headhunters[0].expect("headhunter hired");
        assert_eq!(gold(&sim), [500, 1000]);
        assert_eq!(terms(&sim, hh), (0, 500, 550, 1, 2614));
        assert_eq!(hire_price(&sim, 0), 550);
    }

    #[test]
    fn exact_gold_is_enough() {
        // `<` check (re/headhunter-robbing 9.1): gold exactly = price is enough.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        sim.state.players[0].set_resource(0, 300);
        tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        assert!(sim.state.headhunters[0].is_some());
        assert_eq!(gold(&sim), [0, 1000]);
    }

    #[test]
    fn missing_gold_only_tells_the_payer() {
        // re/headhunter-robbing 3.1 `fail_message`: 10002 red, 4000 ms, only to the payer; otherwise nothing.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        sim.state.players[0].set_resource(0, 299);
        sim.state.events.clear();
        let before = checksum(sim.state);
        run(
            &mut sim,
            A,
            tk::hire_command(SPA, 1, tk::SID_SHERIFF as i32, a_hq),
        );
        assert_eq!(checksum(sim.state), before, "only events");
        assert_eq!(
            sim.state.events,
            [SimEvent::TextMessage {
                to: Some(A),
                text: TEXT_NO_GOLD,
                args: Vec::new(),
                color: COLOR_RED,
                duration_ms: 4000,
            }]
        );
        assert_eq!(sim.state.headhunters, [None; 3]);
        assert_eq!(gold(&sim), [299, 1000]);
    }

    #[test]
    fn a_failed_spawn_keeps_the_gold() {
        // C-311: without type 4100 `0x2D03` fails (-> 1); the price (config 12 from
        // `default.dat`) is still deducted, there is no headhunter, no sound, no message.
        let mut rules = tk::hunter_rules();
        rules.types.remove(&tk::HUNTER_SPA);
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        sim.state.events.clear();
        let ev = tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        assert_eq!(gold(&sim), [700, 1000]);
        assert_eq!(sim.state.headhunters, [None; 3]);
        assert!(sim.state.objects().all(|o| o.guid != tk::HUNTER_SPA));
        assert!(announcements(&ev).is_empty(), "{ev:?}");
    }

    #[test]
    fn a_takeover_by_the_same_player_costs_fifty() {
        // re/headhunter-robbing 8 no. 1, 3.2: even the same customer gets his price back, a change of
        // target costs 50 net.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        let hh = sim.state.headhunters[0].unwrap();
        tk::hire(&mut sim, A, SPA, 1, tk::UNDERTAKER as i32, a_hq);
        assert_eq!(gold(&sim), [1000 - 300 - 350 + 300, 1000]);
        assert_eq!(terms(&sim, hh), (0, 350, 400, 1, 2615));
    }

    #[test]
    fn two_hires_in_one_tick_pay_the_old_price() {
        // C-339: `0x122` waits in the FIFO until the object tick of the headhunter; the second hire in
        // the same step still reads the old price (300) via `0x222`, but `on_hire` books the new one
        // (350) as paid.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, b_hq) = hqs(&mut sim);
        elites(&mut sim);
        run(
            &mut sim,
            A,
            tk::hire_command(SPA, 1, tk::SID_SHERIFF as i32, a_hq),
        );
        run(
            &mut sim,
            B,
            tk::hire_command(SPA, 0, tk::GOVERNOR as i32, b_hq),
        );
        assert_eq!(gold(&sim), [700, 700]);
        tk::run_ticks(&mut sim, 1);
        let hh = sim.state.headhunters[0].unwrap();
        assert_eq!(gold(&sim), [1000, 700]);
        assert_eq!(terms(&sim, hh), (1, 350, 400, 0, 1112));
    }

    #[test]
    fn the_hire_reaches_the_new_hunter_before_its_first_tick() {
        // The headhunter is created in the command step (commands before the world tick) and joins the
        // shorter tick group; `0x122` waits in its FIFO and reaches it in its first object tick before
        // every module tick. Then `0x322` in the 11th object tick.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, _) = hqs(&mut sim);
        elites(&mut sim);
        // Rally point a few tiles next to the exit: the new headhunter walks there.
        let point = [(16 << 12) + 0x800, (13 << 12) + 0x800];
        assert_eq!(
            send(&mut sim, a_hq, &Message::new(SET_MEETING_POINT, &point)),
            0
        );
        let cmd = Command {
            player: A,
            kind: tk::hire_command(SPA, 1, tk::SID_SHERIFF as i32, a_hq),
        };
        crate::tick::step(sim.state, sim.rules, &[cmd]);
        let hh = sim.state.headhunters[0].expect("created in the command step");
        let start = sim.object(hh).unwrap().grid;
        // Step (counted from the command step = 1) of the first headhunter tick.
        let mut n1 = 1;
        while hunter::headhunter(&sim, hh).unwrap().customer != 0 {
            crate::tick::step(sim.state, sim.rules, &[]);
            n1 += 1;
            assert!(n1 <= 2, "no tick of the headhunter");
        }
        assert_eq!(n1, 2, "first headhunter tick");
        let h = hunter::headhunter(&sim, hh).unwrap();
        assert_eq!((h.state, h.customer, h.target_guid), (0, 0, 2614));
        let pending = |sim: &Sim| {
            sim.module(hh, hunter::INDEX)
                .unwrap()
                .base
                .has_pending_timer(hunter::msg::THINK)
        };
        assert!(pending(&sim));
        // Ten more headhunter ticks (20 steps): still pending.
        for _ in 0..20 {
            crate::tick::step(sim.state, sim.rules, &[]);
        }
        assert!(pending(&sim), "not yet due after 10 ticks");
        // The 11th headhunter tick after that (step N1 + 22) delivers `0x322`.
        crate::tick::step(sim.state, sim.rules, &[]);
        assert!(pending(&sim));
        crate::tick::step(sim.state, sim.rules, &[]);
        assert!(!pending(&sim), "delivered in the 11th tick");
        // No way home: the headhunter lives and is on its way to the rally point.
        let o = sim.object(hh).expect("headhunter alive");
        assert_eq!(o.status & status::DIED, 0);
        let d = |p: crate::fx::FxVec3| {
            crate::collision::fixed_pos_distance((p.x.0, p.z.0), (point[0], point[1]))
        };
        assert!(d(o.grid) < d(start), "walks to the rally point");
    }

    #[test]
    fn hiring_a_dead_hunter_is_free() {
        // C-338: `FUN_00477100` also returns the dead headhunter; its module 34 is off, `0x222`
        // answers -1 and leaves the arguments at 0 - price 0, nothing deducted. `0x122` discards the
        // FIFO (module off): no refund, no takeover. Sound and message still come.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, b_hq) = hqs(&mut sim);
        elites(&mut sim);
        tk::hire(&mut sim, A, SPA, 1, tk::SID_SHERIFF as i32, a_hq);
        let hh = sim.state.headhunters[0].unwrap();
        send(&mut sim, hh, &Message::new(0x27F, &[-110]));
        let enabled = |sim: &Sim| sim.module(hh, hunter::INDEX).unwrap().base.enabled;
        tk::run_ticks(&mut sim, 1);
        assert_ne!(sim.object(hh).unwrap().status & status::DIED, 0, "dead");
        // Measured: in the death tick module 34 is still on (C-150: off at the end of T+1), the price
        // still `m_CurReward`; one object tick later it is off.
        assert!(enabled(&sim));
        assert_eq!(hire_price(&sim, 0), 350);
        tk::run_ticks(&mut sim, 1);
        assert!(!enabled(&sim));
        assert_eq!(sim.state.headhunters[0], Some(hh));
        assert_eq!(hire_price(&sim, 0), 0);
        let ev = tk::hire(&mut sim, B, SPA, 0, tk::GOVERNOR as i32, b_hq);
        assert_eq!(gold(&sim), [700, 1000]);
        assert_eq!(announcements(&ev), hired(B, tk::HUNTER_SPA));
        assert_eq!(terms(&sim, hh), (0, 300, 350, 1, 2614));
        // Measured: no panic, the contract stays lapsed.
        tk::run_ticks(&mut sim, 20);
        assert_eq!(terms(&sim, hh), (0, 300, 350, 1, 2614));
        assert_eq!(gold(&sim), [700, 1000]);
    }

    #[test]
    fn script_button_lock_removes_the_colour_button_from_the_hire_menu() {
        // `FUN_0044A8C0` queries `Player_IsGuidDisabled`: the script lock 207 (global) hides the color
        // button `470 + color` for every player, 208 lifts it again.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        hqs(&mut sim);
        assert_eq!(hire_players(&sim, A), [B]);
        assert_eq!(hire_players(&sim, B), [A]);
        let button = |sim: &Sim, slot: i32| COLOR_BUTTON_BASE + player_color(sim, slot) as u32;
        let (button_a, button_b) = (button(&sim, i32::from(A)), button(&sim, i32::from(B)));
        lock_button(&mut sim, button_b, true);
        assert_eq!(hire_players(&sim, A), Vec::<u8>::new());
        assert_eq!(hire_players(&sim, B), [A], "the button of A is free");
        lock_button(&mut sim, button_a, true);
        assert_eq!(hire_players(&sim, B), Vec::<u8>::new());
        lock_button(&mut sim, button_b, false);
        lock_button(&mut sim, button_a, false);
        assert_eq!(hire_players(&sim, A), [B]);
        assert_eq!(hire_players(&sim, B), [A]);
    }

    #[test]
    fn hire_menus_follow_the_original() {
        // re/headhunter-robbing 9.4, C-228, D-239: target players only enemies that are still playing and whose
        // color button is free; targets the elites of the nation in possession, the HQ only without an elite and with
        // an existing headhunter.
        let mut rules = tk::hunter_rules();
        // Color button 472 (color 2) locked for slot 0 (config 1 = 0).
        rules.level.slot_configs.insert(
            0,
            [(COLOR_BUTTON_BASE + 2, [(1, 0)].into_iter().collect())]
                .into_iter()
                .collect(),
        );
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        hqs(&mut sim);
        assert_eq!(hire_players(&sim, A), [B]);
        assert_eq!(hire_players(&sim, B), [A]);
        // Slots 2 and 3 take part (enemies of all); color 2 is locked for A.
        for slot in [2, 3] {
            sim.state.players[slot].result = 0;
        }
        assert_eq!(hire_players(&sim, A), [1, 3]);
        assert_eq!(hire_players(&sim, B), [0, 2, 3]);
        // Allies (not an enemy from the view of A) and eliminated players do not.
        sim.state.diplomacy.set_enemy(0, 3, false);
        sim.state.players[1].result = 1;
        assert_eq!(hire_players(&sim, A), Vec::<u8>::new());
        assert_eq!(hire_players(&sim, 3), [0, 2]);
        // Targets for the SID player B: nothing without an elite and without a headhunter.
        assert_eq!(hire_targets(&sim, 0, B), Vec::<u32>::new());
        // Without an elite, with a headhunter of the kind: the HQ of the nation (2500); other kind still empty.
        tk::spawn(&mut sim, tk::HUNTER_SPA, 8, (20, 20));
        assert_eq!(hire_targets(&sim, 0, B), [2500]);
        assert_eq!(hire_targets(&sim, 1, B), Vec::<u32>::new());
        // Elites in table order depending on possession; the HQ then drops out.
        tk::spawn(&mut sim, tk::UNDERTAKER, 1, (40, 30));
        assert_eq!(hire_targets(&sim, 0, B), [2615]);
        tk::spawn(&mut sim, tk::SID_SHERIFF, 1, (44, 30));
        assert_eq!(hire_targets(&sim, 0, B), [2614, 2615]);
        assert_eq!(hire_targets(&sim, 1, B), [2614, 2615]);
        // SPA player A: governor 1112.
        tk::spawn(&mut sim, tk::GOVERNOR, 0, (10, 30));
        assert_eq!(hire_targets(&sim, 2, A), [1112]);
        // Nation outside 0...5: nothing, not even an HQ.
        sim.state.players[1].nation = 6;
        assert_eq!(hire_targets(&sim, 0, B), Vec::<u32>::new());
    }

    #[test]
    fn hire_commands_follow_the_menu() {
        // Deviates from K-316 (A8): the command checks like the command bar.
        let rules = tk::hunter_rules();
        let mut state = tk::hunter_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (a_hq, b_hq) = hqs(&mut sim);
        elites(&mut sim);
        let sheriff = tk::SID_SHERIFF as i32;
        // Foreign HQ.
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, sheriff, b_hq));
        // Unfinished HQ.
        let mut desc = ObjectDesc::new(tk::HQ_SPA, 0, [tk::world(20), 0.0, tk::world(10)]);
        let site = create_object(&mut sim, &mut desc).expect("construction site");
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, sheriff, site));
        // Monastery without a headhunter button.
        let monastery = tk::spawn(&mut sim, tk::MONASTERY, 0, (20, 20));
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, sheriff, monastery));
        // Kind outside 0...2.
        assert_rejected(&mut sim, A, tk::hire_command(3, 1, sheriff, a_hq));
        assert_rejected(&mut sim, A, tk::hire_command(-1, 1, sheriff, a_hq));
        // Own slot, slot 8, allied target player.
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 0, sheriff, a_hq));
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 8, sheriff, a_hq));
        sim.state.diplomacy.set_enemy(0, 1, false);
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, sheriff, a_hq));
        sim.state.diplomacy.set_enemy(0, 1, true);
        // No elite of the target nation; HQ GUID without an existing headhunter.
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, 1100, a_hq));
        assert_rejected(&mut sim, A, tk::hire_command(SPA, 1, 2500, a_hq));
        // Counter-check: the same command with valid fields hires.
        let before = checksum(sim.state);
        run(&mut sim, A, tk::hire_command(SPA, 1, sheriff, a_hq));
        assert_ne!(checksum(sim.state), before);
        assert!(sim.state.headhunters[0].is_some());
    }
}
