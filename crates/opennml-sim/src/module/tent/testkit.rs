//! Synthetic test world for tents, travois, trade, tribute and heroes (pattern
//! [`crate::upgrades::testkit`]): 64 x 64 tiles, flat; three human players (0 PRI, 1 SPA,
//! 2 ENG), the Prairie Indian tents 1600/1601/1603/1610 with the hulls from re/tents 9.2, the
//! travois 1717, a PRI worker as occupant or colonist, the transport cart 1110 and the
//! headquarters 1000/1900 with the trading office 1907. Values from `property.ini`, `default.dat`
//! and `toolbar.ini` of the shipped data (re/tents 1.1, re/trading 1.1), simplified where noted
//! ("test value").
//!
//! The state is created as in [`crate::upgrades::testkit`] via [`SimState::new`], not via
//! `setup::new_match`: the level start attaches time-of-day upgrades to every list. Module 3 of each type
//! only has the descriptor of the finished variant (without construction site descriptor
//! `PROPID 3 1`), because the variant rules of the test world know no construction site variant.

use opennml_formats::gifcon::parse_toolbar;

use crate::id::ObjectId;
use crate::rules::{BuildingShape, ModelBox, Rules};
use crate::setup::snap_start_object;
use crate::sim::Sim;
use crate::state::{Control, SimState};
use crate::world::{ObjectDesc, create_object, desc_flags};

pub use crate::upgrades::testkit::world;

/// `PRI_Haeuptlingszelt`: modules 3, 23, 40, 42; capacity 5, living space 25.
pub const CHIEF_TENT: u32 = 1600;
/// `PRI_Wohnzelt`: modules 3, 40, 42; living space 15.
pub const DWELLING_TENT: u32 = 1601;
/// `PRI_Lagerstaette`: building without module 42 (no tent; button 452 is still in its menu,
/// test value for the rejection without a module).
pub const STORAGE_TENT: u32 = 1603;
/// `PRI_Alarmposten`: modules 3, 23, 25, 40, 42; capacity 2.
pub const ALARM_POST: u32 = 1610;
/// `PRI_ZeltTravois`: modules 0, 2, 35, 16; button 453.
pub const TRAVOIS: u32 = 1717;
/// Worker of the Prairie Indians (`PRI_Frau`): modules 0, 2, 16; occupant and colonist.
pub const COLONIST: u32 = 1702;
/// `SPA_Transportkarren`: modules 0, 2, 16, 43; load capacity 200.
pub const CART: u32 = 1110;
/// `SPA_Hauptquartier`.
pub const SPA_HQ: u32 = 1000;
/// `ENG_Buergerhaus`.
pub const ENG_HQ: u32 = 1900;
/// `ENG_Handelskontor`: trade building (`TRADEBUILDING`, 40003 bit `0x8000000`).
pub const TRADE_POST: u32 = 1907;

/// Synthetic `property.ini`.
pub const TYPES: &str = concat!(
    // Chief's tent (category `0x2000000`, 40003 of the finished variant `0xC3800`).
    "GUID 1600\nNAME PRI_Haeuptlingszelt\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 800768\nVALUEID 40004 1 1 0\n",
    "PROPID 23\nVALUEID 240001 6 1 0\n",
    "PROPID 42\nVALUEID 430001 1 1 1717\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    // Dwelling tent (40003 `0x4080000`).
    "GUID 1601\nNAME PRI_Wohnzelt\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 67633152\nVALUEID 40004 1 1 0\n",
    "PROPID 42\nVALUEID 430001 1 1 1717\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    // Storage camp (40003 `0x83800`): no module 42.
    "GUID 1603\nNAME PRI_Lagerstaette\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 538624\nVALUEID 40004 1 1 0\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    // Alarm post (like the dwelling tent, plus module 25 and quarters).
    "GUID 1610\nNAME PRI_Alarmposten\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 67633152\nVALUEID 40004 1 1 0\n",
    "PROPID 42\nVALUEID 430001 1 1 1717\n",
    "PROPID 25\nVALUEID 260001 1 1 0 1\nVALUEID 260002 6 1 256\n",
    "PROPID 23\nVALUEID 240001 6 1 0\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    // Travois: walk (test values, shipped data 10003 = `0x220900`, radii 1500/3000), stand, module 35
    // with flags 0, dying.
    "GUID 1717\nNAME PRI_ZeltTravois\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 1\nVALUEID 30002 1 1 0\nVALUEID 30003 6 1 0\n",
    "PROPID 40 11\nVALUEID 410001 1 1 80\n",
    "PROPID 35\nVALUEID 360001 6 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 2\n",
    // Worker (PRI_Frau): walk, stand, dying.
    "GUID 1702\nNAME PRI_Frau\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 7\n",
    // Transport cart: walk (test values), stand, dying, trade (all data values 0 as in the shipped data).
    "GUID 1110\nNAME SPA_Transportkarren\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 2\n",
    "PROPID 43\nVALUEID 440001 1 1 0\nVALUEID 440002 1 1 0\nVALUEID 440003 1 1 0\n",
    // Headquarters and trading office: storage bits `0x3800` (gold, wood, food; test value), the
    // trading office with `TRADEBUILDING` (`0x8080000`).
    "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    "GUID 1900\nNAME ENG_Buergerhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    "GUID 1907\nNAME ENG_Handelskontor\nVALUEID 100 6 1 16777216\n",
    "PROPID 3\nVALUEID 40003 6 1 134742016\nVALUEID 40004 1 1 0\n",
);

/// Menus like `toolbar.ini`: chief's tent with training of the worker and packing (452),
/// dwelling tent, storage camp and alarm post with packing, travois with unpacking (453) and dying
/// (407), the worker with "Repair building" (413, test value for an entry without 452/453).
pub const TOOLBAR: &str = concat!(
    "GUID 1600\n",
    "BUTTONID 1\nBUTTONMSG 1702\nMAKE_MSG\nREQUIRED_GUID 1600\n",
    "BUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 1601\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\nENDGUID\n",
    "GUID 1603\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\nENDGUID\n",
    "GUID 1610\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\nENDGUID\n",
    "GUID 1717\nBUTTONID 5\nBUTTONMSG 453\nTARGET_DO_MSG\nBUTTONID 13\nBUTTONMSG 407\nENDGUID\n",
    "GUID 1702\nBUTTONID 2\nBUTTONMSG 413\nTARGET_MSG\nENDGUID\n",
);

/// `default.dat` values (config) of the types: 13 construction time in s, 14 hit points, 16 speed,
/// 17 load capacity, 23 capacity, 27 living space (re/tents 1.1, re/trading 1.1; test values:
/// storage camp, worker, headquarters, trading office as well as the speed of travois (that of the
/// worker) and cart (as `walk::tests`); without them both would run with the fallback value 10 and
/// would need hundreds of `step`s per tile).
pub const DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (CHIEF_TENT, &[(13, 63), (14, 2400), (23, 5), (27, 25)]),
    (DWELLING_TENT, &[(13, 14), (14, 540), (27, 15)]),
    (STORAGE_TENT, &[(13, 21), (14, 1000)]),
    (ALARM_POST, &[(13, 18), (14, 1600), (23, 2)]),
    (TRAVOIS, &[(14, 500), (16, 145)]),
    (COLONIST, &[(10, 50), (13, 15), (14, 160), (16, 145)]),
    (CART, &[(14, 300), (16, 183), (17, 200)]),
    (SPA_HQ, &[(14, 4000)]),
    (ENG_HQ, &[(14, 4000)]),
    (TRADE_POST, &[(14, 1500)]),
];

/// Hulls of the tents (re/tents 9.2): `(GUID, center, half extent)`. For the storage camp 1603
/// the hull of the dwelling tent applies (test value).
const SHAPES: &[(u32, [f32; 3], [f32; 3])] = &[
    (
        CHIEF_TENT,
        [0.374_481_2, 338.300_72, -5.834_411_6],
        [250.011_02, 341.329_47, 215.300_72],
    ),
    (
        DWELLING_TENT,
        [-16.783_955, 217.760_94, -1.963_535_3],
        [120.710_97, 217.760_94, 106.589_78],
    ),
    (
        STORAGE_TENT,
        [-16.783_955, 217.760_94, -1.963_535_3],
        [120.710_97, 217.760_94, 106.589_78],
    ),
    (
        ALARM_POST,
        [-36.554_054, 234.338_36, -11.222_935],
        [121.663_98, 234.551_22, 97.467_606],
    ),
];

/// Rules of the test world.
pub fn rules() -> Rules {
    rules_from(TYPES)
}

/// [`rules`] with other types (e.g. [`TYPES`] with additional types of a test).
pub fn rules_from(types: &str) -> Rules {
    let mut r = Rules::synthetic(types, 64);
    for &(guid, values) in DEFAULTS {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    // Tents: real hulls; hull and model box are the same values.
    for &(guid, center, half) in SHAPES {
        add_shape(&mut r, guid, center, half);
    }
    // Headquarters and trading office: square hull (test value).
    for guid in [SPA_HQ, ENG_HQ, TRADE_POST] {
        crate::upgrades::testkit::add_building_shape(&mut r, guid, 100.0);
    }
    r.toolbar = parse_toolbar(TOOLBAR);
    r
}

/// Building hull and model box from `center` and `half`.
fn add_shape(r: &mut Rules, guid: u32, center: [f32; 3], half: [f32; 3]) {
    r.building_shapes.insert(
        guid,
        BuildingShape {
            center,
            half,
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        },
    );
    r.model_boxes.insert(guid, ModelBox { center, half });
}

/// Empty, revealed test world with the human players 0 (PRI, nation 1), 1 (SPA, 2) and
/// 2 (ENG, 4), hostile to each other like the diplomacy default, 1000 gold, wood and food each.
pub fn state(rules: &Rules) -> SimState {
    let mut s = SimState::new(0, 0);
    s.init_map(rules);
    s.path.build_regions(u32::MAX);
    for (slot, nation) in [(0, 1), (1, 2), (2, 4)] {
        let p = &mut s.players[slot];
        p.slot_active = true;
        p.control = Control::Human;
        p.nation = nation;
        p.result = 0;
        for index in 0..3 {
            p.set_resource(index, 1000);
        }
    }
    debug_assert!(s.diplomacy.is_enemy(0, 1) && s.diplomacy.is_enemy(1, 2));
    crate::fog::reveal_map(&mut s, -1);
    s
}

/// Finished object (pre-placed) at the center of the tile `tile`.
pub fn spawn(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    desc.flags |= desc_flags::PLACED;
    create_object(sim, &mut desc).expect("object created")
}

/// Finished object whose start position is snapped like that of the start objects
/// (`setup::snap_start_object`, re/tents 9.2): the lower corner of the model box lies on a
/// tile boundary. `(x, z)` are world coordinates; without a model box the position stays unchanged.
pub fn spawn_snapped(sim: &mut Sim, guid: u32, slot: i32, (x, z): (f32, f32)) -> ObjectId {
    let mut pos = [x, 0.0, z];
    snap_start_object(sim.rules, guid, &mut pos);
    let mut desc = ObjectDesc::new(guid, slot, pos);
    desc.flags |= desc_flags::PLACED;
    create_object(sim, &mut desc).expect("object created")
}

/// Construction site (not finished) at the center of the tile `tile`.
pub fn site(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    create_object(sim, &mut desc).expect("construction site created")
}

/// `n` engine ticks without commands (two `step`s are one object tick).
pub fn run_steps(sim: &mut Sim, n: usize) {
    for _ in 0..n {
        crate::tick::step(sim.state, sim.rules, &[]);
    }
}
