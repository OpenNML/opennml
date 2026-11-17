//! Synthetic upgrade test world (pattern of [`crate::command::testkit`]): 64 x 64 tiles, flat; colonist
//! with module variants 6/7 (unarmed/armed), buildings with storage and quarters values, governor with spell
//! (38) and execute (44), missionary with module 30, transport ship with the variants 12/13 and an undertaker
//! with the hostile and permanent records. Values like `default.dat`, `toolbar.ini` and `tempupgrades.ini`
//! of the shipped data (re/upgrades 1.1, 9; re/temp-upgrades 1.4, 9), simplified where noted.
//!
//! The state is built as in [`crate::command::testkit`] via [`SimState::new`], not via
//! `setup::new_match`: the level start attaches the time-of-day upgrades to every list.

use nml_formats::gifcon::{parse_toolbar, parse_variant_rules};
use nml_formats::targets::{parse_target_defines, parse_targets};
use nml_formats::temp_upgrades::parse_temp_upgrades;

use crate::id::ObjectId;
use crate::rules::{BuildingShape, ModelBox, Rules};
use crate::sim::Sim;
use crate::state::{Control, SimState};
use crate::world::{ObjectDesc, create_object, desc_flags};

pub const COLONIST: u32 = 1100;
pub const HQ: u32 = 1000;
/// House with `BuildTime 6000` (config 13 = 60, re/temp-upgrades 9 V4).
pub const HOUSE: u32 = 1001;
pub const STORE: u32 = 1003;
pub const TOWER: u32 = 1010;
pub const PIKEMAN: u32 = 1106;
pub const SHIP: u32 = 1108;
pub const GOVERNOR: u32 = 1112;
pub const MISSIONARY: u32 = 1113;
pub const ARCHER: u32 = 1402;
/// Undertaker (re/temp-upgrades 1.4: trigger of 4459) with module 44 `4459 4460`.
pub const UNDERTAKER: u32 = 2615;

/// Module 39 (`PropertyArming`), flag `ISARMED` (`MID_PROP_ARMING_ISARMED` = `0x127`).
pub use crate::module::arming::{INDEX as ARMING, ISARMED};

/// Synthetic `property.ini`. All units walk on land (walk like the colonist of the economy world, also the
/// ship: the tests only need its variants).
pub const TYPES: &str = concat!(
    // Colonist: walk, construction, stand, lumbering, melee and dying in variant 6
    // (unarmed), walk, stand, ranged combat, shot and dying in variant 7 (armed),
    // attack in both, arming 39 in both (property.ini SPA_Kolonist).
    "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n",
    "PROPID 0 6\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 0 7\nVALUEID 10001 6 1 19\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 1 6\nVALUEID 20001 6 1 5\nVALUEID 20002 6 1 0\nVALUEID 20003 6 1 7\n",
    "PROPID 2 6\nVALUEID 30001 1 1 0\n",
    "PROPID 2 7\nVALUEID 30001 1 1 18\n",
    "PROPID 8 6\nVALUEID 90001 6 1 4\nVALUEID 90002 1 1 5\nVALUEID 90005 6 1 10\n",
    "VALUEID 90006 6 1 3\nVALUEID 90007 6 1 -1\n",
    "PROPID 10 7\nVALUEID 110001 6 1 68608\nVALUEID 110002 1 1 22\nVALUEID 110003 1 1 21\n",
    "VALUEID 110004 1 1 23\nVALUEID 110005 6 1 1\n",
    "PROPID 16 6\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 13\n",
    "PROPID 16 7\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 20\n",
    "PROPID 17 6\nVALUEID 180001 6 1 768\nVALUEID 180002 6 1 0\n",
    "PROPID 17 7\nVALUEID 180001 6 1 1054720\nVALUEID 180002 6 1 0\n",
    "PROPID 18 6\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 31\n",
    "PROPID 19 7\nVALUEID 200001 6 1 0\n",
    "PROPID 39 6 7\nVALUEID 400001 6 1 0\n",
    // Headquarters and warehouse: storage bits 0x3800 (gold, wood, food).
    "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    "GUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "GUID 1003\nNAME SPA_Lagerhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
    // Tower: building with quarters (capacity from attribute 23).
    "GUID 1010\nNAME SPA_Turm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nPROPID 23\n",
    // Pikeman: modules without a variant mask (all bits) - its variant switch depends on no
    // filter term (K-282 on the dying one).
    "GUID 1106\nNAME SPA_Pikenier\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
    // Transport ship: walk and stand in 12/13, attack and shot only in 13 (fire arrows, C-224).
    "GUID 1108\nNAME SPA_Transportschiff\nVALUEID 100 6 1 0\n",
    "PROPID 0 12\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
    "PROPID 0 13\nVALUEID 10001 6 1 4\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
    "PROPID 2 12\nVALUEID 30001 1 1 0\n",
    "PROPID 2 13\nVALUEID 30001 1 1 3\n",
    "PROPID 17 13\nVALUEID 180001 6 1 133120\n",
    "PROPID 10 13\nVALUEID 110001 6 1 131072\nVALUEID 110002 1 1 5\nVALUEID 110003 1 1 6\n",
    "VALUEID 110004 1 1 3\n",
    // Governor: execution list 4452 4450 4451 (index 0 combat view, 1 taxes, 2 build time),
    // spell kind 4 with animations 7 7 7 (property.ini SPA_Gouverneur).
    "GUID 1112\nNAME SPA_Gouverneur\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n",
    "PROPID 17\nVALUEID 180001 6 1 1050624\n",
    "PROPID 38\nVALUEID 390001 6 1 0\nVALUEID 390002 1 1 7 7 7\nVALUEID 390003 1 1 5 6 11\n",
    "PROPID 44\nVALUEID 450001 1 1 4452 4450 4451\nVALUEID 450004 1 1 data/effects/r.hef\n",
    // Missionary: module 30 with 2840 (undefined, re/upgrades 3.7), dying for the expiry of the
    // counter-upgrades of its source (`dying::start_dying`).
    "GUID 1113\nNAME SPA_Missionar\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n",
    "PROPID 30\nVALUEID 310001 1 1 2840\n",
    "GUID 1402\nNAME WID_Bogenschuetze\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    // Undertaker: execution list 4459 (hostile, radius) and 4460 (permanent, without radius
    // and handle list).
    "GUID 2615\nNAME PAT_Leichenbestatter\nVALUEID 100 6 1 0\n",
    "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
    "PROPID 2\nVALUEID 30001 1 1 0\n",
    "PROPID 44\nVALUEID 450001 1 1 4459 4460\n",
);

/// Menus like `toolbar.ini` (simplified: 544/545 without prerequisite, 3375 with `REQUIRED_GUID 3354`
/// instead of 3355): headquarters with training, research and submenu 600, colonist with
/// arm/disarm, governor with the three abilities, missionary with submenu 601
/// (execute counter-upgrades).
pub const TOOLBAR: &str = concat!(
    "GUID 1000\n",
    "BUTTONID 0\nBUTTONMSG 1100\nMAKE_MSG\nREQUIRED_GUID 1000\n",
    "BUTTONID 2\nBUTTONMSG 2800\nUPGRADE_MSG\n",
    "BUTTONID 3\nBUTTONMSG 2801\nUPGRADE_MSG\nREQUIRED_GUID 2800\n",
    "BUTTONID 7\nBUTTONMSG 2806\nUPGRADE_MSG\n",
    "BUTTONID 8\nBUTTONMSG 2828\nUPGRADE_MSG\n",
    "BUTTONID 10\nBUTTONMSG 600\n",
    "ENDGUID\n",
    "GUID 600\nBUTTONID 7\nBUTTONMSG 3354\nUPGRADE_MSG\nENDGUID\n",
    "GUID 1100\n",
    "BUTTONID 12\nBUTTONMSG 485\nTARGET_DO_MSG\nREQUIRED_GUID 2806\n",
    "BUTTONID 12\nBUTTONMSG 486\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 1112\n",
    "BUTTONID 7\nBUTTONMSG 544\nDO_MSG\n",
    "BUTTONID 8\nBUTTONMSG 543\nDO_MSG\nREQUIRED_GUID 2828\n",
    "BUTTONID 9\nBUTTONMSG 545\nDO_MSG\n",
    "ENDGUID\n",
    "GUID 1113\nBUTTONID 10\nBUTTONMSG 601\nENDGUID\n",
    "GUID 601\n",
    "BUTTONID 7\nBUTTONMSG 3374\nCOUNTERUPGRADE_MSG\nREQUIRED_GUID 3354\n",
    "BUTTONID 8\nBUTTONMSG 3375\nCOUNTERUPGRADE_MSG\nREQUIRED_GUID 3354\n",
    "ENDGUID\n",
);

/// Variant rules (`upgrades.ini`, excerpt): 6 unarmed, 7 armed with 2806, 12/13
/// fire arrows 2823.
pub const UPGRADES: &str =
    "BIT 0 = 1\nBIT 6 = 485\nBIT 7 = 2806 486\nBIT 12 = -2823\nBIT 13 = 2823\n";

/// `targets.ini` for the variant terms 485 (not armed) and 486 (armed).
pub const TARGETS: &str = concat!(
    "BEGIN\nBUTTONMSG 485\nMY_MM\nMM_MODE MM_CHECKNOTHANDLETYPE\n",
    "MM_MATCHBITS MID_PROP_ARMING_ISARMED\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 486\nMY_MM\nMM_MODE MM_CHECKHANDLETYPE\n",
    "MM_MATCHBITS MID_PROP_ARMING_ISARMED\nMM_END\nEND\n",
);

pub const DEFINES: &str = "MID_PROP_ARMING_ISARMED 295\n";

/// Records per re/temp-upgrades 1.4 (flags 4450 `0x4E1`, 4451 `0xE7`, 4452/4457 `0xD9`, 4459
/// `0x2D9`, 4460 `0x49` without `0x80`); effect paths synthetic.
pub const TEMP_UPGRADES: &str = concat!(
    "TEMPUPGRADE 4450\nTEMPHANDLELISTUPGRADE\nMSGTICK 57F 2 0 1000\n",
    "EFFECT 0 data/effects/test_steuern.pef\nEND\n",
    "TEMPUPGRADE 4451\nTEMPHANDLELISTUPGRADE\nEFFECT 0 data/effects/test_bauzeit.pef\n",
    "MSGSTART 601 1 300\nMSGEND 601 1 100\nEND\n",
    "TEMPUPGRADE 4452\nTEMPRADIUSUPGRADE\nTAKEUPGRADEVALUES\n",
    "EFFECT 0 data/effects/test_kampfsicht.pef\nEND\n",
    "TEMPUPGRADE 4457\nTEMPRADIUSUPGRADE\nTAKEUPGRADEVALUES\n",
    "EFFECT 0 data/effects/test_artillerie.pef\nEND\n",
    "TEMPUPGRADE 4459\nTEMPRADIUSUPGRADE\nENEMYUPGRADE\nTAKEUPGRADEVALUES\n",
    "EFFECT 0 data/effects/test_verdunkeln.pef\nEND\n",
    "TEMPUPGRADE 4460\nTAKEUPGRADEVALUES\nEFFECT 0 data/effects/test_wolf.pef\nEND\n",
);

/// Upgrade -> types (`OBJ_UPGRADES`, ascending by type GUID; synthetic, restricted to the types of the
/// test world).
pub const UPGRADE_TYPES: &[(u32, &[u32])] = &[
    (2800, &[COLONIST]),
    (2801, &[COLONIST]),
    (2806, &[COLONIST]),
    (2810, &[PIKEMAN]),
    (2811, &[PIKEMAN]),
    (2812, &[PIKEMAN]),
    (2815, &[TOWER]),
    (2817, &[TOWER]),
    // Ship hull: second upgrade of the ship without a variant effect.
    (2822, &[SHIP]),
    (2823, &[SHIP]),
    (2910, &[ARCHER]),
    (3023, &[PIKEMAN]),
    (3120, &[COLONIST]),
    (3372, &[COLONIST]),
    (3374, &[COLONIST]),
    (3375, &[COLONIST, PIKEMAN, ARCHER]),
    (3376, &[HQ, TOWER]),
    (4450, &[GOVERNOR]),
    (4451, &[COLONIST, GOVERNOR]),
    (4452, &[PIKEMAN, GOVERNOR]),
    (4460, &[ARCHER]),
];

/// `default.dat` values (config) of the types, upgrades and ability buttons.
pub const DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    // Colonist: training 50 food, 15 s; HP 160, speed 145, wood rate 60.
    (
        COLONIST,
        &[(10, 50), (13, 15), (14, 160), (16, 145), (32, 60)],
    ),
    (HQ, &[(14, 4000)]),
    (HOUSE, &[(13, 60), (14, 900)]),
    (STORE, &[(13, 30), (14, 1500)]),
    (TOWER, &[(14, 3200), (18, 52), (23, 2)]),
    (PIKEMAN, &[(14, 270), (15, 100)]),
    (SHIP, &[(14, 1400)]),
    (GOVERNOR, &[(14, 1400), (15, 100), (18, 32), (20, 20)]),
    (MISSIONARY, &[(14, 1000)]),
    (ARCHER, &[(14, 155)]),
    (UNDERTAKER, &[(14, 1200), (18, 32)]),
    // Upgrades (re/upgrades 1.1, 9).
    (2800, &[(10, 50), (11, 100), (13, 15), (132, 25)]),
    (2801, &[(10, 100), (11, 200), (13, 30), (132, 30)]),
    (2806, &[(13, 10)]),
    (2810, &[(114, 10)]),
    (2811, &[(114, 15)]),
    (2812, &[(114, 25)]),
    (2815, &[(118, 20)]),
    (2817, &[(123, 100)]),
    (2822, &[(114, 50)]),
    (2828, &[(13, 10)]),
    (2910, &[(114, 10)]),
    (3023, &[(115, 50)]),
    (3120, &[(116, 30)]),
    (3354, &[(12, 200), (13, 20)]),
    (3372, &[(132, -80), (13, 120)]),
    (3374, &[(10, 150), (11, 150), (12, 150), (13, 120)]),
    (3375, &[(114, -40), (13, 60)]),
    (3376, &[(114, -60), (13, 60)]),
    // Rank 1 (object modifier, not type table).
    (4400, &[(114, 5)]),
    // Temporary upgrades: duration from config 13 (no `UPGRADETIME`).
    (4450, &[(13, 60)]),
    (4451, &[(13, 60)]),
    (4452, &[(13, 60), (118, 75), (120, 100)]),
    (4457, &[(13, 60), (120, 175)]),
    (4459, &[(13, 60), (118, -65), (121, -65)]),
    (4460, &[(119, 60), (120, 60)]),
    // Mana of the ability buttons (config 15 of the button GUID).
    (543, &[(15, 75)]),
    (544, &[(15, 50)]),
    (545, &[(15, 40)]),
];

/// `[UPGRADE]` of the project file: all upgrades of the test world (2840 is undefined in the shipped data).
pub const UPGRADE_GUIDS: &[u32] = &[
    2800, 2801, 2806, 2810, 2811, 2812, 2815, 2817, 2822, 2823, 2828, 2910, 3023, 3120, 3354, 3372,
    3374, 3375, 3376, 4400, 4450, 4451, 4452, 4457, 4459, 4460,
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
    for &(guid, types) in UPGRADE_TYPES {
        r.upgrade_types.insert(guid, types.to_vec());
    }
    r.upgrades = UPGRADE_GUIDS.iter().copied().collect();
    r.temp_upgrades = parse_temp_upgrades(TEMP_UPGRADES)
        .expect("synthetic tempupgrades.ini")
        .into_iter()
        .map(|t| (t.guid, t))
        .collect();
    for guid in [HQ, HOUSE, STORE, TOWER] {
        add_building_shape(&mut r, guid, 100.0);
    }
    r.toolbar = parse_toolbar(TOOLBAR);
    r.targets = parse_targets(TARGETS, &parse_target_defines(DEFINES)).entries;
    r.variant_rules = parse_variant_rules(UPGRADES);
    // Time marker 1 of spell animation 7 at 1000 ms (cast after 500 ms, end after 1000 ms).
    r.set_anim_labels(GOVERNOR, 7, &[(1, 1000)]);
    r
}

/// Building shape (hull and model box, half edge length `half`) as in the economy world.
pub fn add_building_shape(r: &mut Rules, guid: u32, half: f32) {
    r.building_shapes.insert(
        guid,
        BuildingShape {
            center: [0.0; 3],
            half: [half, 50.0, half],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        },
    );
    r.model_boxes.insert(
        guid,
        ModelBox {
            center: [0.0; 3],
            half: [half, 50.0, half],
        },
    );
}

/// Empty, revealed test world with two human players 0 and 1 (hostile to each other like the
/// diplomacy default, 1000 gold, wood and food each).
pub fn state(rules: &Rules) -> SimState {
    let mut s = SimState::new(0, 0);
    s.init_map(rules);
    s.path.build_regions(u32::MAX);
    for slot in [0, 1] {
        let p = &mut s.players[slot];
        p.slot_active = true;
        p.control = Control::Human;
        p.result = 0;
        for index in 0..3 {
            p.set_resource(index, 1000);
        }
    }
    debug_assert!(s.diplomacy.is_enemy(0, 1) && s.diplomacy.is_enemy(1, 0));
    crate::fog::reveal_map(&mut s, -1);
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

/// Set or clear `ISARMED` on the arming module (without variant switch).
pub fn set_armed(sim: &mut Sim, id: ObjectId, armed: bool) {
    let m = sim.module_mut(id, ARMING).expect("module 39");
    if armed {
        m.base.flags |= ISARMED;
    } else {
        m.base.flags &= !ISARMED;
    }
}
