//! Test world of the groups: SPA infantryman (rank 4), pikeman (rank 5), cannon (rank 2, radius 1000,
//! slower), WID warrior (rank 0), amphibian (layer 1); all with walk and stand, 64 x 64 tiles.

use crate::command::{Command, CommandKind, NO_SPREAD};
use crate::id::ObjectId;
use crate::message::Posted;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{ObjectDesc, create_object};

pub const INFANTRY: u32 = 1105;
pub const PIKEMAN: u32 = 1106;
pub const CANNON: u32 = 1111;
pub const WARRIOR: u32 = 1400;
/// Like the infantry, but movement mask `0x29` (land + water): layer 1 instead of 0.
pub const AMPHIBIAN: u32 = 1112;

/// Walk like the colonist of the walk tests, plus radius and rank.
fn unit(guid: u32, name: &str, radius: i32, rank: i32, flags: i32) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\n\
         VALUEID 10003 6 1 {flags}\nVALUEID 10004 6 1 {radius}\nVALUEID 10005 6 1 1024\n\
         VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nVALUEID 10011 6 1 {rank}\n\
         PROPID 2\nVALUEID 30001 1 1 0\n"
    )
}

pub fn rules() -> Rules {
    let types = [
        unit(INFANTRY, "SPA_Infanterist", 512, 4, 264448),
        unit(PIKEMAN, "SPA_Pikenier", 512, 5, 264448),
        unit(CANNON, "SPA_Kanone", 1000, 2, 264448),
        unit(WARRIOR, "WID_Krieger", 512, 0, 264448),
        unit(AMPHIBIAN, "SPA_Amphibie", 512, 4, 272640),
    ]
    .concat();
    let mut r = Rules::synthetic(&types, 64);
    for (guid, speed) in [
        (INFANTRY, 183),
        (PIKEMAN, 183),
        (CANNON, 120),
        (WARRIOR, 183),
        (AMPHIBIAN, 183),
    ] {
        r.defaults.insert(guid, [(16, speed)].into_iter().collect());
    }
    r
}

pub fn state(rules: &Rules) -> SimState {
    let mut s = SimState::new(0, 0);
    s.init_map(rules);
    s
}

pub fn tile_center(t: i32) -> i32 {
    (t << 12) + 0x800
}

/// Unit of slot 0 on the tile center.
pub fn spawn(sim: &mut Sim, guid: u32, tile: (i32, i32)) -> ObjectId {
    let world = |t: i32| t as f32 * 128.0 + 64.0;
    create_object(
        sim,
        &mut ObjectDesc::new(guid, 0, [world(tile.0), 0.0, world(tile.1)]),
    )
    .unwrap()
}

/// Move command of slot 0 for `units` (selection order) onto the tile `tile`.
pub fn mv(units: &[ObjectId], tile: (i32, i32), formation: i32, direction: i32) -> Command {
    Command {
        player: 0,
        kind: CommandKind::Move {
            units: units.to_vec(),
            x: tile_center(tile.0),
            z: tile_center(tile.1),
            target: -1,
            formation,
            direction,
            spread: NO_SPREAD,
            flags: 0,
        },
    }
}

/// Target and flags of the last posted FINDTO of a unit.
pub fn posted_find_to(sim: &Sim, id: ObjectId) -> Option<(i32, i32, i32)> {
    sim.object(id)?.fifo.iter().rev().find_map(|p| match p {
        Posted::Command { env, .. } if env.msg.code & 0xFF00 <= 0x300 => Some((
            env.msg.arg(0),
            env.msg.arg(1),
            *env.msg.args.last().unwrap(),
        )),
        _ => None,
    })
}
