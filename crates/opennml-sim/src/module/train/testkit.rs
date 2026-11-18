//! Test world of train and multipart objects: locomotive, tender, wagon (module 33, the wagon with
//! quarters 23) and cart with rear part (modules 0 + 11 or 51 + 16); 64 x 64 tiles of land, flat.
//! The coupling nodes are freely chosen (world units, not from the shipped data). Also tracks (2518,
//! finished with `TRACK`), a colonist and the station 2517 with the button "Tracks 2518".

use opennml_formats::gifcon::parse_toolbar;

use crate::id::ObjectId;
use crate::rules::{Rules, TrainNodes};
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{ObjectDesc, create_object, desc_flags};

pub(crate) use super::{LOCO, TENDER, TRACK, WAGON};

/// SID_PostkutscheVorne (walk 10004 1900, 10005 4096; module 11, rear part 2620).
pub const CART: u32 = 2611;
/// SID_PostkutscheHinten (module 51, death module 16).
pub const CART_TAIL: u32 = 2620;

/// Front part like [`CART`], additionally with a death module (death of the front part).
pub const CART_MORTAL: u32 = 2612;
/// Rear part with a further member (520001 = [`CART_TAIL`], chain; never in the shipped data).
pub const TAIL_CHAIN: u32 = 2621;

/// Colonist with walk (land), as an obstacle on the track.
pub const COLONIST: u32 = 1100;

/// SID_Bahnhof: building with production of tracks (2518).
pub const STATION: u32 = 2517;

/// Walk flags `2048`: land.
const LAND: u32 = 2048;

/// Train module 33: flags 340001 and the four animation numbers (shipped data 0, 2, 1, 1).
fn train_module(flags: u32) -> String {
    format!(
        "PROPID 33\nVALUEID 340001 6 1 {flags}\nVALUEID 340002 6 1 0\nVALUEID 340003 6 1 2\n\
         VALUEID 340004 6 1 1\nVALUEID 340005 6 1 1\n"
    )
}

/// Types of the test world as `property.ini` text.
fn types() -> String {
    [
        // Locomotive: TRAINEFFECTS (512).
        format!(
            "GUID {LOCO}\nNAME SID_Lokomitive\nVALUEID 100 6 1 0\n{}",
            train_module(512)
        ),
        // Tender: TRAINSLAVE (256).
        format!(
            "GUID {TENDER}\nNAME SID_Tender\nVALUEID 100 6 1 0\n{}",
            train_module(256)
        ),
        // Wagon: plus the quarters (capacity via attribute 0x17).
        format!(
            "GUID {WAGON}\nNAME SID_Wagon\nVALUEID 100 6 1 0\nPROPID 23\n{}",
            train_module(256)
        ),
        // Track segment: building with `TRACK` (40003 = `0x0200_0000`).
        format!(
            "GUID {TRACK}\nNAME Schienen\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 33554432\n"
        ),
        // Station: building (module 3) without special features, the production runs via the menu.
        format!(
            "GUID {STATION}\nNAME SID_Bahnhof\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n"
        ),
        // Colonist: only walk (land).
        format!(
            "GUID {COLONIST}\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\n\
             VALUEID 10001 6 1 3\nVALUEID 10003 6 1 {LAND}\nVALUEID 10004 6 1 512\n\
             VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n\
             VALUEID 10008 6 1 4\n"
        ),
        // Front part: walk and MultiHead (type of the rear part 120001).
        format!(
            "GUID {CART}\nNAME SID_PostkutscheVorne\nVALUEID 100 6 1 0\nPROPID 0\n\
             VALUEID 10001 6 1 3\nVALUEID 10003 6 1 {LAND}\nVALUEID 10004 6 1 1900\n\
             VALUEID 10005 6 1 4096\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n\
             VALUEID 10008 6 1 4\nPROPID 11\nVALUEID 120001 6 1 {CART_TAIL}\n"
        ),
        // Rear part: MultiPart (radius 1900, animations 0/1/2, drawbar 5300) and death module.
        format!(
            "GUID {CART_TAIL}\nNAME SID_PostkutscheHinten\nVALUEID 100 6 1 0\nPROPID 51\n\
             VALUEID 520001 6 1 0\nVALUEID 520002 6 1 1900\nVALUEID 520003 6 1 0\n\
             VALUEID 520004 6 1 1\nVALUEID 520005 6 1 2\nVALUEID 520006 6 1 5300\n\
             PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n"
        ),
        // Front part with death module (animation 2).
        format!(
            "GUID {CART_MORTAL}\nNAME SID_PostkutscheVorneSterblich\nVALUEID 100 6 1 0\nPROPID 0\n\
             VALUEID 10001 6 1 3\nVALUEID 10003 6 1 {LAND}\nVALUEID 10004 6 1 1900\n\
             VALUEID 10005 6 1 4096\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n\
             VALUEID 10008 6 1 4\nPROPID 11\nVALUEID 120001 6 1 {CART_TAIL}\n\
             PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n"
        ),
        // Rear part with a further member 2620 (chain).
        format!(
            "GUID {TAIL_CHAIN}\nNAME SID_PostkutscheKette\nVALUEID 100 6 1 0\nPROPID 51\n\
             VALUEID 520001 6 1 {CART_TAIL}\nVALUEID 520002 6 1 1300\nVALUEID 520003 6 1 0\n\
             VALUEID 520004 6 1 1\nVALUEID 520005 6 1 2\nVALUEID 520006 6 1 2000\n\
             PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n"
        ),
    ]
    .concat()
}

/// Rules on a 64 x 64 map (land) with the coupling nodes: locomotive only hitch `(0, 0, 100)`,
/// tender hitch `(0, 0, 60)` and counterpart `(0, 0, -60)`, wagon `(0, 0, 60)` and `(0, 0, -80)`.
pub fn rules() -> Rules {
    let mut r = Rules::synthetic(&types(), 64);
    for guid in [
        CART,
        CART_TAIL,
        CART_MORTAL,
        TAIL_CHAIN,
        LOCO,
        TENDER,
        WAGON,
        COLONIST,
    ] {
        r.defaults
            .insert(guid, [(14, 100), (16, 183), (18, 32)].into_iter().collect());
    }
    r.defaults.insert(
        WAGON,
        [(14, 100), (16, 183), (18, 32), (23, 10)]
            .into_iter()
            .collect(),
    );
    // Menu of the station like `toolbar.ini`: button 2518 (`MAKE_MSG`), `REQUIRED_GUID 2517`.
    r.toolbar = parse_toolbar(
        "GUID 2517\nBUTTONID 0\nBUTTONMSG 2518\nMAKE_MSG\nREQUIRED_GUID 2517\nENDGUID\n",
    );
    r.train_nodes.insert(
        LOCO,
        TrainNodes {
            hitch: Some([0.0, 0.0, 100.0]),
            coupling: None,
        },
    );
    r.train_nodes.insert(
        TENDER,
        TrainNodes {
            hitch: Some([0.0, 0.0, 60.0]),
            coupling: Some([0.0, 0.0, -60.0]),
        },
    );
    r.train_nodes.insert(
        WAGON,
        TrainNodes {
            hitch: Some([0.0, 0.0, 60.0]),
            coupling: Some([0.0, 0.0, -80.0]),
        },
    );
    r
}

/// Empty test world: grid of the map and regions (`path.build_regions`).
pub fn state(rules: &Rules) -> SimState {
    state_in_mode(rules, 0)
}

/// Like [`state`] in game type `mode` (4 and 5 "Railroad").
pub fn state_in_mode(rules: &Rules, mode: i32) -> SimState {
    let mut s = SimState::new(mode, 0);
    s.init_map(rules);
    s.path.build_regions(u32::MAX);
    s
}

/// World coordinate of the tile center.
fn world(t: i32) -> f32 {
    t as f32 * 128.0 + 64.0
}

/// Finished object at the center of tile `tile`, direction 0. The rotation about the y axis is
/// the one `world::set_direction` writes for direction 0 (`rad(dir + 90 degrees)`); without `ROT_Y`
/// -90 degrees would result (`apply_placement` subtracts 90 degrees).
pub fn spawn(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    spawn_dir(sim, guid, slot, tile, 0)
}

/// Like [`spawn`] with the direction `dir` in degrees (multiples of 90 degrees come back exactly).
pub fn spawn_dir(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32), dir: i32) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
    desc.rotation[1] = ((dir + 90) << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
    create_object(sim, &mut desc).expect("object created")
}

/// Finished track segments (slot 0) on all tiles from `from` to `to` (one axis), direction from
/// the stretch: +x 0 degrees, -x 180 degrees, +z 90 degrees, -z -90 degrees.
pub fn lay_track(sim: &mut Sim, from: (i32, i32), to: (i32, i32)) -> Vec<ObjectId> {
    let (dx, dz) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
    assert!(dx == 0 || dz == 0, "track only along one axis");
    let dir = match (dx, dz) {
        (-1, _) => 180,
        (_, 1) => 90,
        (_, -1) => -90,
        _ => 0,
    };
    let n = (to.0 - from.0).abs().max((to.1 - from.1).abs());
    (0..=n)
        .map(|i| spawn_dir(sim, TRACK, 0, (from.0 + i * dx, from.1 + i * dz), dir))
        .collect()
}

/// Locomotive for slot `slot` at `tile`, direction 0; the init couples tender and wagon.
pub fn spawn_loco(sim: &mut Sim, slot: i32, tile: (i32, i32)) -> ObjectId {
    spawn(sim, LOCO, slot, tile)
}
