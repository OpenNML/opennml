//! Player actions: resources 21/22, diplomacy and shared vision (28-32), stance 203/204, damage factor 342/343,
//! time of day 173/205 and task status 200. Source: the dispatcher cases `0x12`, `0x13`, `0x16`...`0x1A`,
//! `0x41`, `0x58`, `0x5B`, `0x5C`, `0x6E`, `0x6F`, `FUN_00437910` (204) and `FUN_0047BA50` (stance per
//! object).
//!
//! All of them act immediately and without a command, silently and without a message to players. A stance
//! outside 0...4 does nothing and reports that as `Err` (the caller turns it into `Guarded`, exception A7): the
//! original then leaves words of the messages `0x611`/`0x2100` unset (heap leftovers, not deterministic,
//! C-341). The time of day is passed on unchecked, as in the original (E-199).

use crate::id::ObjectId;
use crate::message::Message;
use crate::module::quarter::{ai_attack_mode_msg, attack_mode_msg, walk_behavior_msg};
use crate::script::area;
use crate::script::query::filter_or_raw_guid;
use crate::sim::Sim;
use crate::world::apply_player_behavior;

/// Message "home of the animal" (`0x914 {x, y, z}`).
const SET_ANIMAL_HOME: u32 = 0x914;
/// Largest valid stance (0 aggressive ... 4).
const LAST_MODE: i32 = 4;
/// Slots with a stance field (0...8).
const PLACES: i32 = 9;

/// Actions 21 and 22 (`Player_AddResource`, `0x004230F0`/`0x00423145`): `amount` to the resource `kind`
/// of the player `slot` - 22 gives the **negated** amount; there is no lower bound (E-159). Kind 3
/// (population used) and 4 (living space) are resources like the others; a kind outside 0...4 changes no
/// resource (the statistics still book kind 9).
/// A slot outside 0...7 (a crash in the original) does nothing; the load check of the `.abl` does not allow it.
///
/// Afterwards the original unconditionally books the game statistics `FUN_00478A10(p, kind, +-amount)`
/// (`0x0042313B` and `0x00423194`): kind 10 (gold) for resource 0, 8 (food) for 2, **every other** 9 (wood) -
/// O-170.
pub fn add_resource(sim: &mut Sim, slot: i32, kind: i32, amount: i32) {
    let Some(player) = usize::try_from(slot)
        .ok()
        .and_then(|s| sim.state.players.get_mut(s))
    else {
        return;
    };
    if let Ok(k) = usize::try_from(kind) {
        player.add_resource(k, amount);
    }
    let statistic = match kind {
        0 => crate::accounting::kind::GOLD,
        2 => crate::accounting::kind::FOOD,
        _ => crate::accounting::kind::WOOD,
    };
    crate::accounting::add_statistic(sim, slot, statistic, amount);
}

/// Actions 28-32 (`SetSharedVision` `0x00477770`, `SetFriendRelation` `0x00477710`, `SetEnemyRelation`
/// `0x004776B0`): only the row of `p` changes (`p` looks at `q`), `q == p` is allowed, slots outside 0...7 do
/// nothing (E-160). There is no message to the players and no notification of running fights (unlike the
/// game command `0x64`).
///
/// 28/29 set/clear the vision; 30 "allied" sets friend and clears enemy, 31 "hostile" the reverse,
/// 32 "neutral" clears both.
pub fn diplomacy(sim: &mut Sim, number: i32, p: i32, q: i32) {
    let d = &mut sim.state.diplomacy;
    match number {
        28 => d.set_vision(p, q, true),
        29 => d.set_vision(p, q, false),
        30 => {
            d.set_friend(p, q, true);
            d.set_enemy(p, q, false);
        }
        31 => {
            d.set_friend(p, q, false);
            d.set_enemy(p, q, true);
        }
        32 => {
            d.set_friend(p, q, false);
            d.set_enemy(p, q, false);
        }
        _ => {}
    }
}

/// Action 203 (`FUN_00478B20` + `FUN_0047BA50`): set the default stance of the slot `p` (0...8, also that of
/// the neutrals) and - for slots 0...7 - send the stance to every object of the slot
/// ([`apply_player_behavior`]: `0x2100`, `0x611`, for a control kind other than human the AI addition).
/// Unlike the command `0x69`, the script does not check the control kind: it also changes the stance of human
/// players (E-155). A slot outside 0...8 does nothing.
///
/// The original writes every value into the field; for a stance outside 0...4 it leaves words of the messages
/// `0x611`/`0x2100` unset (heap leftovers, not deterministic, C-341); here it does nothing (`Err`).
/// The load check of the scripts (`abl::catalog::check`) already rejects it; the error stays as an
/// unreachable safeguard.
pub fn set_player_behavior(sim: &mut Sim, p: i32, mode: i32) -> Result<(), String> {
    if !(0..PLACES).contains(&p) {
        return Ok(());
    }
    if !(0..=LAST_MODE).contains(&mode) {
        return Err(format!("stance {mode} outside 0...{LAST_MODE}"));
    }
    sim.state.combat.behavior_mode[p as usize] = mode;
    apply_player_behavior(sim, p, mode);
    Ok(())
}

/// Action 204 (`FUN_00437910(GUID, player, area, mode)`): for each object in the area that passes the filter
/// `guid` with the reference player `p`, **directly** one after the other `0x2100` (walk behavior), `0x611`
/// (attack mode) and `0x914` (animal home to the own grid position `{x, y, z}`); afterwards, for mode 2,
/// `0x611 {4, 3, 1, 0}`, for mode 3 `{4, 3, 2, 0}` - **regardless of the control kind** (E-155).
///
/// An unknown filter ID (< 1000 without an entry in `flowcontroltragets.ini`) filters with the raw GUID
/// (`mode 8`, unlike 6, which does nothing); `MYLIST` stays set. An unknown area does nothing.
/// The original does not check the mode; a mode outside 0...4 does nothing here (`Err`; unreachable after the
/// load check of the `.abl`; the original leaves message words unset in that case, C-341). Without hits
/// nothing happens, not even a message.
pub fn set_behavior_in_area(
    sim: &mut Sim,
    guid: i32,
    p: i32,
    area_name: &str,
    mode: i32,
) -> Result<(), String> {
    let Some(rect) = area(sim, area_name) else {
        return Ok(());
    };
    let f = filter_or_raw_guid(sim, guid);
    let hits = crate::area::collect_in_tile_rect(sim, rect, &f, p);
    if hits.is_empty() {
        return Ok(());
    }
    if !(0..=LAST_MODE).contains(&mode) {
        return Err(format!("stance {mode} outside 0...{LAST_MODE}"));
    }
    let walk = walk_behavior_msg(mode, false);
    let attack = attack_mode_msg(mode, false);
    let extra = match mode {
        2 => Some(ai_attack_mode_msg(4, 1)),
        3 => Some(ai_attack_mode_msg(4, 2)),
        _ => None,
    };
    for id in hits {
        crate::messaging::send(sim, id, &walk);
        crate::messaging::send(sim, id, &attack);
        crate::messaging::send(sim, id, &animal_home_msg(sim, id));
        if let Some(m) = &extra {
            crate::messaging::send(sim, id, m);
        }
    }
    Ok(())
}

/// `{0x914, x, y, z}` with the grid position of the object (`+0x19C`, `+0x1A4`, `+0x1A0`).
fn animal_home_msg(sim: &Sim, id: ObjectId) -> Message {
    let grid = sim.object(id).map(|o| o.grid);
    Message::new(
        SET_ANIMAL_HOME,
        &grid.map_or([0; 3], |g| [g.x.0, g.y.0, g.z.0]),
    )
}

/// Actions 342 (`v = 0`, invulnerable) and 343 (`v = 100`): `GameWorld_SetDefenseDamageFactor`
/// (`0x00478550`) for the slots 0...8. 343 does **not** restore the handicap value (E-165).
pub fn set_hit_factor(sim: &mut Sim, p: i32, v: i32) {
    if let Some(f) = usize::try_from(p)
        .ok()
        .and_then(|p| sim.state.combat.hit_factor.get_mut(p))
    {
        *f = v;
    }
}

/// Actions 173 and 205 (`PlayerSetup_ApplyDaytimeUpgrades(t)`, `0x00482160`; `t` is the time of day, not
/// a player): the same time of day does nothing, otherwise the time-of-day upgrades are swapped
/// ([`crate::upgrades::apply_daytime_upgrades`]). The original does not check `t` and passes every value on (E-199).
pub fn daytime(sim: &mut Sim, t: i32) {
    crate::upgrades::apply_daytime_upgrades(sim, t);
}

/// Action 200 (D case `0x58`, `GameEngine+0x5AC`): mark task `key` (the **second** value) as done.
/// Pure UI in the original (colors `[TASKSTATUS n]` in the text green); here in the state (E-202, deviation
/// from the original).
pub fn task_status(sim: &mut Sim, key: i32) {
    sim.state.script.mission.task_status.insert(key, 1);
}
