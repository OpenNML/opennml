//! Module 38 kind 3: reincarnation (re/spells 3.4, 3.5, 4, 8 no. 2-4, 11, 12). The cast searches
//! the nearest corpse ([`find_corpse`]) and remembers its position; the effect `0x326` deducts the
//! cost, puts it on the reserve and lets a war wolf appear at the remembered position
//! ([`reincarnate`]) - with death callbacks at the caster (the wolf disappears) and at the wolf
//! (the reserve is freed).

use nml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::walk;
use crate::object::status;
use crate::sim::Sim;
use crate::world::{ObjectDesc, create_object, desc_flags, grid_to_world, queue_destroy};

use super::{MANA, msg, spell, spell_mut};

/// `ANI_Kampfwolf` (GUID of the reincarnated animal, `0x0052C8F0`).
pub const BATTLE_WOLF: u32 = 3420;
/// Fixed minimum mana of the corpse search, independent of the cost (`0x0052BF15`, C-296).
pub const MIN_MANA: i32 = 20;
/// Sight (attribute 18, in quarter tiles).
const SIGHT: u32 = 18;
/// Filter mode of the corpse search `RETURN_DIED | ANDALL | PROPERTY | STATUS` (`0x0052BF61`).
const CORPSE_MODE: u32 = 0x400_0015;
/// Register a death callback (module 16, `0x310`).
const LISTEN: u32 = 0x310;
/// Animal: vanish (`0x114`, after 1000 ms `0x214` -> destroy).
const VANISH: u32 = 0x114;
/// Animal: fade in (`0x414`, re/animals 2).
const APPEAR: u32 = 0x414;
/// Creation flags of the wolf `0xD3`: GUID, position, rotation Y, slot, color - without `READY`.
const WOLF_FLAGS: u32 = desc_flags::GUID
    | desc_flags::POSITION
    | desc_flags::ROT_Y
    | desc_flags::PLAYER
    | desc_flags::COLOR;

/// Corpse search of kind 3 (`0x0052BEE0`, re/spells 3.4, from the cast): clear the corpse first; mana
/// below [`MIN_MANA`] -> none (C-296). Otherwise the nearest dead unit with a walk module (even
/// switched off) - filter without list bits and without fog check, so any owner (C-297) -
/// within edge distance up to `Fixed_FromInt(sight) >> 2` (sight in quarter tiles -> tiles) plus the
/// own hard radius (`CollisionMgr_FindNearestMatching` `0x004B7620`). Hit: remember the corpse
/// and the target point, `(Fixed_ToFloat(gx * cell), 0, Fixed_ToFloat(gz * cell))`
/// ([`grid_to_world`]).
pub(super) fn find_corpse(sim: &mut Sim, id: ObjectId) -> Option<ObjectId> {
    if let Some(s) = spell_mut(sim, id) {
        s.corpse = ObjectId::NONE;
    }
    if sim.attribute(id, MANA) < MIN_MANA {
        return None;
    }
    let (grid, owner, hard) = sim.object(id).map(|o| (o.grid, o.owner(), o.hard_radius))?;
    let radius = Fx::from_int(sim.attribute(id, SIGHT)).0 >> 2;
    let f = MatchFilter {
        mode: CORPSE_MODE,
        mode2: 0,
        status: status::DIED,
        prop_id: walk::INDEX as u32,
        ..MatchFilter::constructed()
    };
    let corpse = find_nearest_matching(
        sim,
        (grid.x.0, grid.z.0),
        &f,
        radius,
        i32::from(owner),
        hard,
    )?;
    let at = grid_to_world(sim.object(corpse)?.grid, sim.rules.map.cell);
    let s = spell_mut(sim, id)?;
    s.corpse = corpse;
    s.target = [at[0], 0.0, at[2]];
    Some(corpse)
}

/// Reincarnation (`0x0052C8F0`, re/spells 3.5, from `0x326` kind 3): war wolf 3420 with the slot and
/// color of the caster at the remembered position (height 0) and its rotation Y (`+0x158`),
/// flags `0xD3` (`GameWorld_CreateObject` `0x00485F70`; without category `0x400` the slot stays).
/// If that fails, nothing. Otherwise `0x414` to the wolf (fade in), death callback at the
/// **caster** `{0x114}` to the wolf (`0x0052C9D8`, C-300), at the **wolf** `{0x526, 0, cost}` to
/// the caster (`0x0052CA24`, C-299), then queue the corpse for destruction - without checking whether
/// it is still there (`GameWorld_QueueObjectDestroy` `0x0047C7D0`, C-298).
pub(super) fn reincarnate(sim: &mut Sim, id: ObjectId) {
    let Some((owner, rot_y)) = sim.object(id).map(|o| (o.owner(), o.rotation[1])) else {
        return;
    };
    let Some((target, cost, corpse)) = spell(sim, id).map(|s| (s.target, s.cost, s.corpse)) else {
        return;
    };
    let slot = i32::from(owner);
    let mut d = ObjectDesc::new(BATTLE_WOLF, slot, target);
    d.flags = WOLF_FLAGS;
    d.rotation = [0.0, rot_y, 0.0];
    d.color = crate::setup::player_color(sim, slot);
    let Some(wolf) = create_object(sim, &mut d) else {
        return;
    };
    send(sim, wolf, &Message::new(APPEAR, &[]));
    let listen = |success: Message, target: ObjectId| Envelope {
        msg: Message::new(LISTEN, &[]),
        success: Some(success),
        fault: None,
        success_target: target,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &listen(Message::new(VANISH, &[]), wolf));
    dispatch_command(
        sim,
        wolf,
        &listen(Message::new(msg::RESERVE, &[0, cost]), id),
    );
    queue_destroy(sim, corpse);
}
