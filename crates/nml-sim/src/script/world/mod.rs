//! World actions of the scripts (layer D of `Script_DispatchAction` `0x00422B60`).
//!
//! Each file covers a group of D cases ([`create`]: creating, [`objects`]: removing, ownership, hit points,
//! attributes, collision, groups; [`players`]: resources, diplomacy, stance, damage factor, time of day,
//! task status; [`hide`]: hiding whole slots and showing/hiding single objects; [`view`]: named view
//! rectangles, clearing "explored", build button lock; [`mission`]: mission objectives); they call the
//! world code directly, without commands (the command actions live in [`super::command`]); [`ki`]: start
//! AI, delete, set a value. [`dispatch_world`] is the common entry point.

use super::actions::ScriptMsg;
use crate::sim::Sim;

pub mod create;
pub mod hide;
pub mod ki;
pub mod mission;
pub mod objects;
pub mod players;
pub mod view;

/// World action of the message `msg`. `Some(Ok(()))`: the number belongs to the world actions and has been
/// executed (even if the original silently discards it); `Some(Err(why))`: a value that the original
/// processed unchecked and that would not result in a loadable state here (A7) - no effect, the caller
/// reports `Guarded` with the block index and ends the block as with any run-time error; `None`: not a
/// world action, the caller ([`super::actions::dispatch`]) reports it as "no effect yet".
///
/// Value slots per `actions::message`: 3 `{slot, count, guid, area}`, 396 plus `{..., name}`, 14
/// `{guid, x, z}` (slot 8), 36 `{guid, x, z, slot}`, 151 `{slot, guid, name, area}`, 217 `{slot,
/// guid, angle, name, area}`.
pub fn dispatch_world(sim: &mut Sim, msg: &mut ScriptMsg) -> Option<Result<(), String>> {
    let mut result = Ok(());
    match msg.number {
        // AI: 0/1 `{slot, guid, count, words...}`, 37 `{slot}`, 340 `{slot, index, value}`.
        0 | 1 => ki::start(sim, msg, msg.number == 0),
        37 => ki::delete(sim, msg.int(0)),
        340 => ki::set_global(sim, msg.int(0), msg.int(1), msg.int(2)),
        3 => create::spawn_in_area(sim, msg.int(0), msg.int(1), msg.int(2), msg.text(3), None),
        396 => create::spawn_in_area(
            sim,
            msg.int(0),
            msg.int(1),
            msg.int(2),
            msg.text(3),
            Some(msg.text(4)),
        ),
        14 => {
            create::spawn_at_tile(
                sim,
                i32::from(crate::id::NEUTRAL_SLOT),
                msg.int(0),
                msg.int(1),
                msg.int(2),
            );
        }
        36 => {
            create::spawn_at_tile(sim, msg.int(3), msg.int(0), msg.int(1), msg.int(2));
        }
        151 => {
            create::place_named(sim, msg.int(0), msg.int(1), msg.text(3), msg.text(2), -1);
        }
        217 => {
            create::place_named(
                sim,
                msg.int(0),
                msg.int(1),
                msg.text(4),
                msg.text(3),
                msg.int(2),
            );
        }
        // Removing: `{player, guid, area}`; 194 `{name}`.
        6 | 195 | 196 => objects::remove_in_area(sim, msg.int(1), msg.int(0), msg.text(2)),
        194 => objects::remove_named(sim, msg.text(0)),
        // Halting: 174 `{area}`.
        174 => objects::halt_in_area(sim, msg.text(0)),
        // Ownership: 35/197 `{guid, slot, area}`, 202 `{value (slot), name}`, 341 `{p, q}`.
        35 | 197 => objects::give_in_area(sim, msg.int(0), msg.int(1), msg.text(2)),
        202 => objects::give_named(sim, msg.text(1), msg.int(0)),
        341 => objects::give_slot(sim, msg.int(0), msg.int(1)),
        // Hit points: `{value, name}`; 354/215 `{name}`.
        192 => objects::set_hp_base(sim, msg.text(1), msg.int(0)),
        201 => objects::set_hp(sim, msg.text(1), msg.int(0)),
        353 => objects::add_hp(sim, msg.text(1), msg.int(0)),
        354 => objects::fill_hp(sim, msg.text(0)),
        215 => objects::fill_special(sim, msg.text(0)),
        // Attributes: `{attribute, value, flag (384), name}`.
        384 | 385 => {
            objects::set_attribute(sim, msg.text(3), msg.int(0), msg.int(1), msg.int(2) != 0)
        }
        // Clear event bit: `{bit, name}`.
        231 => objects::clear_event_bit(sim, msg.text(1), msg.int(0)),
        // Collision: 391/392 `{word (392 = 1), name}`; groups: 393/394 `{name, group}`, 395 `{name}`,
        // 397 `{player, area, group}`.
        391 | 392 => objects::set_collision(sim, msg.text(1), msg.int(0) == 0),
        393 => objects::group_add(sim, msg.text(1), msg.text(0)),
        394 => objects::group_remove(sim, msg.text(1), msg.text(0)),
        395 => objects::remove_name(sim, msg.text(0)),
        397 => objects::group_from_area(sim, msg.int(0), msg.text(1), msg.text(2)),
        // Player actions: 21/22 `{slot, amount, kind}`, 28-32 `{p, q}`, 203 `{p, mode}`, 204 `{player,
        // mode, guid, area}`, 342/343 `{p}`, 173/205 `{time of day}`, 200 `{Z0, task}`.
        21 => players::add_resource(sim, msg.int(0), msg.int(2), msg.int(1)),
        22 => players::add_resource(sim, msg.int(0), msg.int(2), msg.int(1).wrapping_neg()),
        28..=32 => players::diplomacy(sim, msg.number, msg.int(0), msg.int(1)),
        203 => result = players::set_player_behavior(sim, msg.int(0), msg.int(1)),
        204 => {
            result =
                players::set_behavior_in_area(sim, msg.int(2), msg.int(0), msg.text(3), msg.int(1));
        }
        342 => players::set_hit_factor(sim, msg.int(0), 0),
        343 => players::set_hit_factor(sim, msg.int(0), 100),
        173 | 205 => players::daytime(sim, msg.int(0)),
        200 => players::task_status(sim, msg.int(1)),
        // Hiding: 159/160 `{slot}`, 161/162 `{name}`, 365/366 `{slot, guid}`, 211/214 `{slot, type,
        // area}` (211/162/366 show, 214/161/365 hide).
        159 => result = hide::deactivate(sim, msg.int(0)),
        160 => result = hide::activate(sim, msg.int(0)),
        161 | 162 => hide::named(sim, msg.text(0), msg.number == 162),
        365 | 366 => hide::by_type(sim, msg.int(0), msg.int(1), msg.number == 366),
        211 | 214 => {
            result = hide::in_area(
                sim,
                msg.number,
                msg.int(0),
                msg.int(1),
                msg.text(2),
                msg.number == 211,
            );
        }
        // View rectangles and clearing "explored": 20/24/347/372 `{slot, area}`; build button lock 207/208 `{guid}`.
        20 => view::add_area_view(sim, msg.int(0), msg.text(1)),
        24 => view::remove_area_view(sim, msg.int(0), msg.text(1)),
        372 => view::clear_explored(sim, msg.int(0), msg.text(1)),
        347 => view::clear_explored_open(sim, msg.int(0), msg.text(1)),
        207 | 208 => view::lock_button(sim, msg.int(0) as u32, msg.number == 207),
        // Mission objectives: 27/182 `{no, text}`, 184-187 and 198/199 `{no}` (184/186 and 198 secondary
        // objectives, 185/187 and 199 main objectives; 184/185 status 6, 186/187 status 10).
        27 => mission::set_quest(sim, true, msg.int(0), msg.int(1)),
        182 => mission::set_quest(sim, false, msg.int(0), msg.int(1)),
        184 => mission::set_quest_status(sim, false, msg.int(0), 6),
        185 => mission::set_quest_status(sim, true, msg.int(0), 6),
        186 => mission::set_quest_status(sim, false, msg.int(0), 10),
        187 => mission::set_quest_status(sim, true, msg.int(0), 10),
        198 => mission::prefill_quests(sim, false, msg.int(0)),
        199 => mission::prefill_quests(sim, true, msg.int(0)),
        _ => return None,
    }
    Some(result)
}
