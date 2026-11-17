//! Synthetic economy world of the command and right-click tests: SPA colonist with walk, build,
//! stand, gold mining, wood chopping and harvest, fishing boat, the SPA buildings of build menu 110 with
//! `default.dat`-like values, deciduous tree and fishing basket; build and production menus like
//! `toolbar.ini` and the economic right-click entries in file order.

use nml_formats::gifcon::parse_toolbar;
use nml_formats::targets::{parse_target_defines, parse_targets};

use crate::command::{Command, CommandKind, F32Bits, execute};
use crate::id::ObjectId;
use crate::rules::{BuildingShape, ModelBox, Rules};
use crate::save::checksum;
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{ObjectDesc, create_object, desc_flags};

pub const COLONIST: u32 = 1100;
pub const BOAT: u32 = 1117;
pub const HQ: u32 = 1000;
pub const HOUSE: u32 = 1001;
pub const FARM: u32 = 1002;
pub const STORE: u32 = 1003;
pub const MINE: u32 = 1004;
pub const BARRACKS: u32 = 1005;
pub const MARKET: u32 = 1007;
/// SID_Bahnhof: menu with the button "Track 2518".
pub const STATION: u32 = 2517;
pub const WALL: u32 = 1011;
pub const TREE: u32 = 3602;
pub const BASKET: u32 = 3505;
pub const SOIL: u32 = 3503;
/// Research of the headquarters menu (`UPGRADE_MSG`, "Arm").
pub const RESEARCH: u32 = 2806;

/// Fertile soil (nature, module 6 with bit `0x200`) - only for tests that add it to [`TYPES`].
pub const SOIL_TYPE: &str = "GUID 3503
NAME Boden
VALUEID 100 6 1 512
PROPID 6
VALUEID 70002 6 1 512
";

pub const TYPES: &str = concat!(
    "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 1\nVALUEID 20001 6 1 5\nVALUEID 20002 6 1 0\nVALUEID 20003 6 1 7\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 7\nVALUEID 80001 6 1 6\nVALUEID 80004 6 1 0\nVALUEID 80005 1 1 7\n",
    "VALUEID 80006 6 1 -1\nVALUEID 80007 1 1 16 17\n",
    "PROPID 8\nVALUEID 90001 6 1 4\nVALUEID 90002 1 1 5\nVALUEID 90005 6 1 10\n",
    "VALUEID 90006 6 1 3\nVALUEID 90007 6 1 -1\n",
    "PROPID 12\nVALUEID 130001 1 1 9\nVALUEID 130002 1 1 10\nVALUEID 130003 6 1 -1\n",
    "VALUEID 130006 6 1 8\n",
    "GUID 1117\nNAME SPA_Fischerboot\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 55\nVALUEID 80001 6 1 0\nVALUEID 80002 1 1 2 3\nVALUEID 80003 1 1 4 5\n",
    "VALUEID 80004 1 1 9\nVALUEID 80005 1 1 6 7\n",
    "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    "PROPID 23\n",
    "GUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "GUID 1002\nNAME SPA_Finca\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\n",
    "GUID 1003\nNAME SPA_Lagerhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    "GUID 1004\nNAME SPA_Goldgrube\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 256\n",
    "GUID 1005\nNAME SPA_Kaserne\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "GUID 1007\nNAME SPA_Warenkontor\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "GUID 1011\nNAME SPA_Mauer\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 3145728\n",
    "GUID 2517\nNAME SID_Bahnhof\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "GUID 2518\nNAME Schienen\nVALUEID 100 6 1 0\n",
    "GUID 3505\nNAME Fischerkorb\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 0\n",
    "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n",
);

/// Colonist menu with the build menu 110 (like `toolbar.ini`), HQ menu (colonist, research 2806; entered as an
/// upgrade in [`rules`], 10 s),
/// station menu (track 2518, only there as in `toolbar.ini`) and trading post (trade 490-493).
pub const TOOLBAR: &str = concat!(
    "GUID 1100\nBUTTONID 0\nBUTTONMSG 110\nTARGET_DO_MSG\nBUTTONID 2\nBUTTONMSG 413\nTARGET_MSG\nENDGUID\n",
    "GUID 110\nBUTTONID 0\nBUTTONMSG 1000\nBUILD_MSG\nGROUP_AND_MSG\nONLYONCE_MSG\n",
    "BUTTONID 1\nBUTTONMSG 1001\nBUILD_MSG\nGROUP_AND_MSG\n",
    "BUTTONID 2\nBUTTONMSG 1002\nBUILD_MSG\nGROUP_AND_MSG\n",
    "BUTTONID 3\nBUTTONMSG 1003\nBUILD_MSG\nGROUP_AND_MSG\n",
    "BUTTONID 4\nBUTTONMSG 1004\nBUILD_MSG\nGROUP_AND_MSG\nREQUIRED_GUID 1000\n",
    "BUTTONID 8\nBUTTONMSG 1007\nBUILD_MSG\nGROUP_AND_MSG\nREQUIRED_GUID 1000\nREQUIRED_GUID 1003\n",
    "BUTTONID 9\nBUTTONMSG 1005\nBUILD_MSG\nGROUP_AND_MSG\nREQUIRED_GUID 1000\n",
    "BUTTONID 10\nBUTTONMSG 1011\nBUILD_MSG\nGROUP_AND_MSG\nENDGUID\n",
    "GUID 1000\nBUTTONID 0\nBUTTONMSG 1100\nMAKE_MSG\nREQUIRED_GUID 1000\n",
    "BUTTONID 7\nBUTTONMSG 2806\nUPGRADE_MSG\nREQUIRED_GUID 1000\nENDGUID\n",
    "GUID 2517\nBUTTONID 0\nBUTTONMSG 2518\nMAKE_MSG\nREQUIRED_GUID 2517\nENDGUID\n",
    "GUID 1007\nBUTTONID 0\nBUTTONMSG 490\nMAKE_MSG\nREQUIRED_GUID 1007\n",
    "BUTTONID 1\nBUTTONMSG 491\nMAKE_MSG\nREQUIRED_GUID 1007\n",
    "BUTTONID 7\nBUTTONMSG 492\nMAKE_MSG\nREQUIRED_GUID 1007\n",
    "BUTTONID 8\nBUTTONMSG 493\nMAKE_MSG\nREQUIRED_GUID 1007\nENDGUID\n",
);

pub const DEFINES: &str = "PROP_PTTOPT 0\nPROP_CONSTRUCT 1\nPROP_BUILDING 3\nPROP_TREE 4\nPROP_MINING 7\nPROP_LUMBERING 8\nPROP_HARVESTING 12\nPROP_RIDING 13\nPROP_FISHING 55\nMID_PROP_BUILDING_GOLDMINE 259\nMID_PROP_BUILDING_FARM 515\nMID_PROP_BUILDING_CONSTRUCTED_FINISHED 1027\nMID_PROP_BUILDING_GOLDLAGER 2051\nMID_PROP_BUILDING_LUMBERYARD 4099\nMID_PROP_BUILDING_GRANARY 8195\nMID_PROP_BUILDING_HAFEN 32771\nMID_PROP_PTTOPT_SADDLE 32768\nMID_PROP_PTTOPT_CANQUARTER 262144\nMID_PROP_QUARTER_FREEFORQUARTER 279\n";

/// The economic entries of `targets.ini` in file order
/// (re/garrison-and-commands): 400, 401, 402, 398, 446 fish, 444, 445, 446 food,
/// 403 mount, 412, 413 damaged, 414, 413, 447, 448.
pub const TARGETS: &str = concat!(
    "BEGIN\nBUTTONMSG 400\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_LUMBERING\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_MODE MM_HL_MYLIST\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_PROPID PROP_TREE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 401\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_HARVESTING\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_CHECKHANDLETYPE\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_HL_MYLIST\nMM_MATCHBITS MID_PROP_BUILDING_FARM\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 4\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 402\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_MINING\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKHANDLETYPE\nMM_MODE MM_HL_MYLIST\nMM_MATCHBITS MID_PROP_BUILDING_GOLDMINE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 398\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_FISHING\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKGUID\nMM_GUID 3505\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 446\nMY_MM\nMM_MODE MM_CHECKLOADFOOD\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_FISHING\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_HAFEN\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 444\nMY_MM\nMM_MODE MM_CHECKLOADWOOD\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_LUMBERYARD\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 445\nMY_MM\nMM_MODE MM_CHECKLOADGOLD\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_GOLDLAGER\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 446\nMY_MM\nMM_MODE MM_CHECKLOADFOOD\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKNOTSTATUS\nMM_MODE MM_HL_MYLIST\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_GRANARY\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 403\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_HL_MYLIST\nMM_PROPID PROP_RIDING\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_PTTOPT_SADDLE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 412\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_CONSTRUCT\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MODE MM_HL_MYLIST\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 413\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_CONSTRUCT\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ENERGYLOST\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 414\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKHANDLETYPE\nMM_MODE MM_HL_MYLIST\nMM_MATCHBITS MID_PROP_PTTOPT_CANQUARTER\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_QUARTER_FREEFORQUARTER\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ANDNEXTMATCH\nMM_NEXT\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 413\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_CONSTRUCT\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_HL_MYLIST\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 447\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 448\nMY_MM\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_PTTOPT\nMM_MATCHBITS MID_PROP_PTTOPT_SADDLE\nMM_END\nTARGET_MM\nMM_END\nEND\n",
);

/// Rules on a 64 x 64 map: buildings about 3 x 3 tiles (wall segment one tile), finca with
/// the nodes `farmer01…05`.
pub fn rules() -> Rules {
    rules_from(TYPES)
}

/// [`rules`] with other types (e.g. [`TYPES`] with changed flags or [`SOIL_TYPE`]).
pub fn rules_from(types: &str) -> Rules {
    let mut r = Rules::synthetic(types, 64);
    let set = |v: &[(u32, i32)]| v.iter().copied().collect();
    r.defaults.insert(
        COLONIST,
        set(&[
            (10, 60),
            (13, 15),
            (16, 183),
            (17, 20),
            (18, 32),
            (22, 1),
            (31, 60),
            (32, 60),
            (33, 80),
        ]),
    );
    r.defaults
        .insert(BOAT, set(&[(16, 183), (17, 300), (31, 80)]));
    r.defaults
        .insert(HQ, set(&[(11, 500), (13, 90), (14, 4000), (23, 5)]));
    r.defaults
        .insert(HOUSE, set(&[(11, 150), (13, 20), (14, 900), (27, 15)]));
    r.defaults
        .insert(FARM, set(&[(11, 300), (13, 30), (14, 1500)]));
    r.defaults
        .insert(STORE, set(&[(11, 200), (13, 30), (14, 1500)]));
    r.defaults
        .insert(MINE, set(&[(11, 250), (13, 30), (14, 1200)]));
    r.defaults
        .insert(BARRACKS, set(&[(11, 300), (13, 40), (14, 2500)]));
    r.defaults
        .insert(MARKET, set(&[(11, 250), (13, 40), (14, 2000)]));
    r.defaults
        .insert(WALL, set(&[(11, 5), (13, 6), (14, 1000)]));
    r.defaults.insert(TREE, set(&[(25, 200)]));
    // Research 2806 (Arm): 10 s (config 13) and entered as an upgrade; without config 13
    // it would never end (`start_entry`: duration 0).
    r.defaults.insert(RESEARCH, set(&[(13, 10)]));
    r.upgrades.insert(RESEARCH);
    let shape = |half: f32| BuildingShape {
        center: [0.0; 3],
        half: [half, 50.0, half],
        free_nodes: Vec::new(),
        helper_nodes: Default::default(),
    };
    for guid in [HQ, HOUSE, FARM, STORE, MINE, BARRACKS, MARKET, WALL] {
        let half = if guid == WALL { 50.0 } else { 100.0 };
        let mut s = shape(half);
        if guid == FARM {
            for k in 0..5 {
                s.helper_nodes.insert(
                    format!("farmer{:02}", k + 1),
                    [(k as f32 - 2.0) * 128.0, 0.0, -256.0],
                );
            }
        }
        r.building_shapes.insert(guid, s);
        r.model_boxes.insert(
            guid,
            ModelBox {
                center: [0.0; 3],
                half: [half, 50.0, half],
            },
        );
    }
    r.toolbar = parse_toolbar(TOOLBAR);
    r.targets = parse_targets(TARGETS, &parse_target_defines(DEFINES)).entries;
    r
}

/// Empty test world, revealed for all players ([`crate::fog::reveal_map`]): the economy and
/// right-click tests see every target. Fog tests use [`fogged_state`].
pub fn state(rules: &Rules) -> SimState {
    let mut s = fogged_state(rules);
    crate::fog::reveal_map(&mut s, -1);
    s
}

/// Empty test world without a revealed map (fog tests).
pub fn fogged_state(rules: &Rules) -> SimState {
    let mut s = SimState::new(0, 0);
    s.init_map(rules);
    s.path.build_regions(u32::MAX);
    s
}

/// World coordinate of the tile center.
pub fn world(t: i32) -> f32 {
    t as f32 * 128.0 + 64.0
}

/// Finished object (pre-placed) at the center of the tile `tile`.
pub fn spawn(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    desc.flags |= desc_flags::PLACED;
    create_object(sim, &mut desc).expect("object created")
}

/// Construction site (not finished) at the center of the tile `tile`.
pub fn site(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    create_object(sim, &mut desc).expect("construction site created")
}

/// Account of the slot in whole units: gold, wood, food, population, living space.
pub fn account(sim: &mut Sim, slot: usize, v: [i32; 5]) {
    sim.state.players[slot].resources_fx = v.map(|x| x << 12);
}

/// Execute a command directly (as from the input).
pub fn run(sim: &mut Sim, player: u8, kind: CommandKind) {
    execute(sim, &Command { player, kind }, true);
}

/// The command is rejected entirely: the checksum stays the same (and no FIFO gets anything).
pub fn assert_rejected(sim: &mut Sim, player: u8, kind: CommandKind) {
    let before = checksum(sim.state);
    run(sim, player, kind.clone());
    assert_eq!(before, checksum(sim.state), "{kind:?} changes the state");
}

/// `0x01` at the tile center `tile` without rotation.
pub fn place(guid: u32, tile: (i32, i32), workers: &[ObjectId]) -> CommandKind {
    CommandKind::PlaceBuilding {
        guid,
        rotation: F32Bits::new(0.0),
        x: F32Bits::new(world(tile.0)),
        y: F32Bits::new(0.0),
        z: F32Bits::new(world(tile.1)),
        workers: workers.to_vec(),
        border: None,
    }
}

/// Whole simulation steps until `done` (at most `max`); returns the number.
pub fn run_until(
    state: &mut SimState,
    rules: &Rules,
    max: usize,
    done: impl Fn(&Sim) -> bool,
) -> usize {
    for n in 1..=max {
        crate::tick::step(state, rules, &[]);
        if done(&Sim::new(state, rules)) {
            return n;
        }
    }
    panic!("not reached after {max} steps");
}
