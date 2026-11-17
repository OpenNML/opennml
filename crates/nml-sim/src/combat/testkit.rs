//! Melee test world: the economy world from [`crate::command::testkit`] plus fighters with values
//! like `default.dat` (HP 14, sight 18, attack 19/20, range 21, minimum distance 34), the
//! rank upgrades 4400-4404, the speed from `default.dat` (attribute 16: "Medium" 183,
//! "Medium fast" 200), the time markers of their combat and death animations and `bonus.ini`.

use crate::rules::Rules;

pub use crate::command::testkit::{HQ, run, spawn, state};

pub const SABRE: u32 = 1107;
pub const PIKEMAN: u32 = 1106;
pub const WARRIOR: u32 = 1400;
pub const ARCHER: u32 = 1402;
pub const ENG_INFANTRY: u32 = 2006;
pub const SPA_INFANTRY: u32 = 1105;
pub const WARSHIP: u32 = 1109;
pub const SHADOW_WARRIOR: u32 = 1714;
pub const SPA_CANNON: u32 = 1111;
/// PAT cannon: shot animation without marker 3 (C-81), no cannon attack flag.
pub const PAT_CANNON: u32 = 2312;
/// Combat animation without Hit1 (re/combat-melee 8.1): melee always aborts (C-63).
pub const CONTINENTAL: u32 = 2308;
/// Powder house (`BUILDINGEXPLODE`).
pub const POWDER: u32 = 1009;
/// Castle powder house (`BUILDINGEXPLODE | CASTLEBUILDING`).
pub const CASTLE_POWDER: u32 = 1010;
/// Moduleless observer of the combat tests (`Schienen` of the economy world; GUID 1000 is the HQ there).
pub const OBSERVER: u32 = 2518;
/// Wolf: neutral (category 0x400), aggressive, roamer (re/animals 6).
pub const WOLF: u32 = 3401;
/// Snow goat: neutral, shy, roamer.
pub const GOAT: u32 = 3405;
/// Hunter: PRI archer with shoot and hunting module (re/animals 4.3).
pub const HUNTER: u32 = 1703;
/// Shark: only walk and animal module, underwater (C-36).
pub const SHARK: u32 = 3416;

/// Walk like the colonist (hard radius 512, soft radius 1024).
const WALK: &str = "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n\
VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n\
PROPID 2\nVALUEID 30001 1 1 0\n";

/// Type with category, module 16 (death animation), 17 (flags) and optionally 18/19.
fn fighter(
    guid: u32,
    name: &str,
    cat: u32,
    die: i32,
    attack: u32,
    near: Option<i32>,
    far: bool,
) -> String {
    let mut s = format!("GUID {guid}\nNAME {name}\nVALUEID 100 6 1 {cat}\n{WALK}");
    s += &format!("PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 {die}\n");
    s += &format!("PROPID 17\nVALUEID 180001 6 1 {attack}\nVALUEID 180002 6 1 0\n");
    if let Some(a) = near {
        s += &format!("PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 {a}\n");
    }
    if far {
        s += "PROPID 19\nVALUEID 200001 6 1 0\n";
    }
    s
}

/// Combat types (flags 180001 from re/combat-attack 11).
pub fn types() -> String {
    let mut t = String::from(crate::command::testkit::TYPES);
    t += &fighter(SABRE, "SPA_Saebelkaempfer", 0, 3, 256, Some(2), false);
    t += &fighter(
        PIKEMAN,
        "SPA_Pikenier",
        0x0200_2000,
        3,
        65792,
        Some(1),
        false,
    );
    t += &fighter(WARRIOR, "WID_Krieger", 0, 11, 16640, Some(14), false);
    t += &fighter(ARCHER, "WID_Bogenschuetze", 0, 3, 10240, None, true);
    // Shoot module of the archer (re/combat-ranged 10): ARROW | OBJECTOFF | PCT | CANSHOOTFIREARROW.
    t += "PROPID 10\nVALUEID 110001 6 1 2146560\nVALUEID 110002 1 1 5\nVALUEID 110007 7 1 pfeil\n";
    t += &fighter(HUNTER, "PRI_Bogenschuetze", 0, 3, 10240, None, true);
    t += "PROPID 10\nVALUEID 110001 6 1 2146560\nVALUEID 110002 1 1 5\nVALUEID 110007 7 1 pfeil\n";
    // Hunting (re/gathering A.2): gutting 12, run with meat 13, stand with meat "14 15", delivery -1.
    t += "PROPID 22\nVALUEID 230001 6 1 12\nVALUEID 230002 6 1 13\nVALUEID 230003 1 1 14 15\nVALUEID 230004 6 1 -1\n";
    t += &fighter(
        ENG_INFANTRY,
        "ENG_Infanterist",
        0,
        3,
        1116416,
        Some(4),
        true,
    );
    // Rifle with magazine 1 (re/combat-ranged 10): GUN | MOUTHFIRE | RELOAD.
    t += "PROPID 10
VALUEID 110001 6 1 68608
VALUEID 110002 1 1 6
VALUEID 110003 1 1 7
VALUEID 110004 1 1 8
VALUEID 110005 6 1 1
";
    t += &fighter(SPA_INFANTRY, "SPA_Infanterist", 0, 2, 1050624, None, true);
    // Cannon (re/combat-ranged 10): CANON | PCT | ... (6656), shot animation 9.
    t += &fighter(SPA_CANNON, "SPA_Kanone", 0, 3, 133120, None, true);
    t += "PROPID 10
VALUEID 110001 6 1 6656
VALUEID 110002 1 1 9
";
    t += &fighter(PAT_CANNON, "PAT_Kanone", 0, 3, 2048, None, true);
    t += "PROPID 10
VALUEID 110001 6 1 14848
VALUEID 110002 1 1 9
";
    t += &fighter(
        SHADOW_WARRIOR,
        "PRI_Schattenkrieger",
        0,
        3,
        256,
        Some(2),
        false,
    );
    t += &fighter(
        CONTINENTAL,
        "PAT_Kontinentalkaempfer",
        0,
        3,
        256,
        Some(6),
        false,
    );
    // Warship: walk NOTHUMAN|WATER, attack CANNON, ranged combat ISBOUNTY (0x100).
    t += "GUID 1109\nNAME SPA_Kriegsschiff\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10003 6 1 139264\n\
VALUEID 10004 6 1 1024\nPROPID 16\nVALUEID 170002 6 1 4\nPROPID 17\nVALUEID 180001 6 1 133120\n\
PROPID 19\nVALUEID 200001 6 1 256\n";
    t += &format!(
        "GUID {WOLF}\nNAME ANI_Wolf\nVALUEID 100 6 1 1024\n{WALK}PROPID 16\nVALUEID 170002 6 1 3\n\
PROPID 17\nVALUEID 180001 6 1 256\nPROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 2\n\
PROPID 20\nVALUEID 210001 6 1 8448\nVALUEID 210002 6 1 0\nPROPID 21\n"
    );
    t += &format!(
        "GUID {GOAT}\nNAME ANI_Schneeziege\nVALUEID 100 6 1 1024\n{WALK}PROPID 16\nVALUEID 170002 6 1 3\n\
PROPID 20\nVALUEID 210001 6 1 12288\nVALUEID 210002 6 1 2\nPROPID 21\n"
    );
    t += &format!(
        "GUID {SHARK}\nNAME ANI_Hai\nVALUEID 100 6 1 1024\n{WALK}PROPID 20\nVALUEID 210001 6 1 9472\n"
    );
    t +=
        "GUID 1009\nNAME SPA_Pulverhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 16777216\n";
    t += "GUID 1010\nNAME SPA_Burgpulverhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 285212672\n";
    t
}

/// Synthetic `bonus.ini` with the shipped values (re/combat-damage 1.3).
pub const BONUS: &str = "ATTACKBONUS_SWORD_PEEK 40\nATTACKBONUS_PEEK_MOUNTY 60\nATTACKBONUS_MOUNTY_SWORD 50\n\
ATTACKBONUS_CANNON_BUILDING 150\nATTACKBONUS_FIRE_BUILDING 100\nATTACKBONUS_MELEE_ARTILLERY 50\n\
ATTACKBONUS_MELEE_MELEEARTILLERY 25\nATTACKBONUS_UNIT_BUILDING -20\nMOUNTY_BONUS 18 20\n";

/// Priorities following the pattern of the default groups of `priorities.ini` (re/combat-targeting 1.12):
/// 0 attacks me, 1 fighters (not `WAITING4KILLING`), 2 non-fighters, 3 finished buildings; plus
/// the group of the neutrals (slot 8): human units of players 0-7.
pub const PRIORITIES: &str = "\
BEGIN\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_PLAYERLIST\nMM_MODE2 MM2_DONOTIGNOREQUARTEREDUNITS\nMM_END\n\
TARGET_MM\nMM_MODE2 MM2_IS_ATTACKING\nMM_IS_ATTACKING_WHO ME\nMM_END\nEND\n\
BEGIN\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_PLAYERLIST\nMM_MODE2 MM2_DONOTIGNOREQUARTEREDUNITS\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ATTACK\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS WAITING4KILLING\nMM_END\nEND\n\
BEGIN\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_PLAYERLIST\nMM_MODE2 MM2_DONOTIGNOREQUARTEREDUNITS\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS WAITING4KILLING\nMM_END\nEND\n\
BEGIN\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_PLAYERLIST\nMM_MODE2 MM2_DONOTIGNOREQUARTEREDUNITS\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED\nMM_END\nEND\n\
BEGIN\nMY_MM\nMM_MODE MM_HL_NOMANSLIST\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MATCHBITS MID_PROP_PTTOPT_NOTHUMAN\nMM_MODE MM_HL_PLAYERLIST\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS WAITING4KILLING\nMM_END\nEND\n";

/// Combat right-clicks: 411 Attack (unit with attack module -> enemy unit or - as a second
/// variant, also for fog memories - enemy building seen in the fog), 410 garrison attack (own
/// building with occupants, `CALLMETHOD 2`), 427 attack in attack mode (also ground position).
pub const TARGETS: &str = "\
BEGIN\nBUTTONMSG 411\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ATTACK\nMM_END\n\
TARGET_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_END\nMAUSICONID 7\nEND\n\
BEGIN\nBUTTONMSG 411\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ATTACK\nMM_END\n\
TARGET_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nMAUSICONID 7\nEND\n\
BEGIN\nBUTTONMSG 410\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_HL_MYLIST\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 2\nMM_PROPID PROP_BUILDING\nMM_END\n\
TARGET_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_END\nMAUSICONID 6\nEND\n\
BEGIN\nBUTTONMSG 427\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ATTACK\nMM_END\n\
TARGET_MM\nMM_MODE2 MM2_POSITIONALLOWED\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_END\nEND\n";

/// Right-clicks 428 Follow (own unit -> own unit), 418 Gut (hunter, OR without owner -> carcass with
/// `ISROT`, also dead) and 419 Hunt (hunter not mounted -> animal without `ISROT`).
pub const HUNT_TARGETS: &str = "\
BEGIN\nBUTTONMSG 428\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_PROPID PROP_PTTOPT\nMM_MODE MM_HL_MYLIST\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_END\nMAUSICONID 7\nEND\n\
BEGIN\nBUTTONMSG 418\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_HUNTING\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_RETURN_DIED\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_EDIBLEANIMAL\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_EDIBLEANIMAL_ISROT\nMM_END\nMAUSICONID 10\nEND\n\
BEGIN\nBUTTONMSG 419\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_HUNTING\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_END\n\
TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_EDIBLEANIMAL\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MATCHBITS MID_PROP_EDIBLEANIMAL_ISROT\nMM_END\nMAUSICONID 6\nEND\n";

/// `targetdefines.ini` names of the test world.
pub fn defines() -> nml_formats::targets::TargetDefines {
    let text = format!(
        "{}PROP_ATTACK 17\nMID_PROP_PTTOPT_NOTHUMAN 131072\nPROP_EDIBLEANIMAL 21\nMID_PROP_EDIBLEANIMAL_ISROT 277\nPROP_HUNTING 22\n",
        crate::command::testkit::DEFINES
    );
    nml_formats::targets::parse_target_defines(&text)
}

pub fn rules() -> Rules {
    rules_with("")
}

/// [`rules`] with additional types (type file text after [`types`]); the caller adds the values and
/// shapes of the additional types afterwards.
pub fn rules_with(extra_types: &str) -> Rules {
    let mut r = crate::command::testkit::rules_from(&(types() + extra_types));
    let set = |v: &[(u32, i32)]| v.iter().copied().collect();
    for (guid, v) in [
        (
            SABRE,
            set(&[(14, 190), (16, 200), (18, 32), (19, 18), (21, 8)]),
        ),
        (
            PIKEMAN,
            set(&[(14, 270), (16, 200), (18, 32), (19, 26), (21, 8)]),
        ),
        (
            WARRIOR,
            set(&[(14, 160), (16, 183), (18, 40), (19, 9), (21, 8)]),
        ),
        (
            ARCHER,
            set(&[(14, 155), (16, 200), (18, 40), (20, 11), (21, 28), (34, 4)]),
        ),
        (
            HUNTER,
            set(&[
                (14, 150),
                (16, 200),
                (17, 20),
                (18, 40),
                (20, 11),
                (21, 28),
                (31, 60),
                (34, 4),
            ]),
        ),
        (
            ENG_INFANTRY,
            set(&[
                (14, 275),
                (16, 200),
                (18, 32),
                (19, 11),
                (20, 24),
                (21, 28),
                (34, 4),
            ]),
        ),
        (
            SPA_INFANTRY,
            set(&[(14, 190), (16, 183), (18, 32), (20, 18), (21, 28), (34, 4)]),
        ),
        (
            SHADOW_WARRIOR,
            set(&[(14, 1200), (16, 200), (18, 32), (19, 18)]),
        ),
        (
            SPA_CANNON,
            set(&[(14, 200), (16, 100), (18, 64), (20, 70), (21, 56), (34, 8)]),
        ),
        (
            PAT_CANNON,
            set(&[(14, 200), (16, 100), (18, 64), (20, 81), (21, 56), (34, 8)]),
        ),
        (WARSHIP, set(&[(14, 2000), (20, 20)])),
        (
            WOLF,
            set(&[(14, 80), (16, 227), (18, 20), (19, 15), (21, 8), (24, 50)]),
        ),
        (
            GOAT,
            set(&[(14, 40), (16, 227), (18, 32), (19, 3), (24, 50)]),
        ),
        (SHARK, set(&[(14, 120), (18, 32), (19, 20)])),
        (CONTINENTAL, set(&[(14, 200), (18, 32), (19, 10)])),
        (POWDER, set(&[(14, 600)])),
        (CASTLE_POWDER, set(&[(14, 600)])),
        // Rank upgrades (config 114 HP, 116 speed, 118 sight, 119 melee, 120 ranged, 121 range).
        (4400, set(&[(114, 5), (119, 5), (120, 5)])),
        (
            4401,
            set(&[(114, 10), (118, 5), (119, 10), (120, 10), (121, 5)]),
        ),
        (
            4402,
            set(&[(114, 15), (118, 10), (119, 15), (120, 15), (121, 10)]),
        ),
        (
            4403,
            set(&[(114, 20), (118, 15), (119, 20), (120, 20), (121, 15)]),
        ),
        (
            4404,
            set(&[
                (114, 50),
                (116, 20),
                (118, 40),
                (119, 40),
                (120, 40),
                (121, 40),
            ]),
        ),
    ] {
        r.defaults.insert(guid, v);
    }
    // Time markers (re/combat-melee 8.1, re/combat-death 2.4): (type, animation, markers).
    for (guid, anim, labels) in [
        (SABRE, 2, &[(1, 1200), (2, 320)][..]),
        (SABRE, 3, &[(1, 1280)][..]),
        (PIKEMAN, 1, &[(1, 1050), (2, 723)][..]),
        (PIKEMAN, 3, &[(1, 1066)][..]),
        (WARRIOR, 14, &[(1, 1283), (2, 560), (0x16, 1096)][..]),
        (WARRIOR, 11, &[(1, 1280)][..]),
        (ENG_INFANTRY, 4, &[(1, 483), (2, 133)][..]),
        (ENG_INFANTRY, 3, &[(1, 933)][..]),
        (ENG_INFANTRY, 7, &[(1, 666)][..]),
        (ENG_INFANTRY, 6, &[(1, 3265), (3, 866), (4, 1999)][..]),
        (ENG_INFANTRY, 8, &[(1, 2266)][..]),
        (SHADOW_WARRIOR, 2, &[(1, 1200), (2, 320)][..]),
        (ARCHER, 5, &[(1, 2333), (2, 833), (3, 700)][..]),
        (HUNTER, 5, &[(1, 2333), (2, 833), (3, 700)][..]),
        (CONTINENTAL, 6, &[(1, 667)][..]),
        (SPA_CANNON, 9, &[(1, 5332), (3, 733)][..]),
        (PAT_CANNON, 9, &[(1, 5332), (2, 733)][..]),
        (WOLF, 2, &[(1, 1000), (2, 500)][..]),
        (WOLF, 3, &[(1, 1000)][..]),
    ] {
        r.set_anim_labels(guid, anim, labels);
    }
    // Powder houses: shape and hull like the house.
    for g in [POWDER, CASTLE_POWDER] {
        let shape = r.building_shapes[&crate::command::testkit::HOUSE].clone();
        let bx = r.model_boxes[&crate::command::testkit::HOUSE];
        r.building_shapes.insert(g, shape);
        r.model_boxes.insert(g, bx);
    }
    r.bonus = nml_formats::bonus::parse_bonus_ini(BONUS);
    r.priorities = nml_formats::targets::parse_targets(PRIORITIES, &defines()).entries;
    // The combat entries come before the economy entries (like 411 early in the file in the original).
    let mut targets = nml_formats::targets::parse_targets(TARGETS, &defines()).entries;
    targets.append(&mut r.targets);
    targets.extend(nml_formats::targets::parse_targets(HUNT_TARGETS, &defines()).entries);
    r.targets = targets;
    r.mesh_names
        .insert(ARCHER, ["pfeil".to_string()].into_iter().collect());
    r.mesh_names
        .insert(HUNTER, ["pfeil".to_string()].into_iter().collect());
    r
}
