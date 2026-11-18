//! Synthetic test world for camouflage (module 25), dig in (28) and healing (29) (pattern
//! [`crate::module::building::fort_testkit`]): 64 x 64 tiles, flat, fully revealed; slots 0 and 1 are human and enemies
//! of each other.
//!
//! The types carry the values of the shipped data from re/masking 1.1 and re/healing 1.1 (ability parameters
//! 260001/260002, 290001-290004, 300001-300008, category, walk flags and radii, hit points 14, extra energy 15, speed
//! 16, sight 18; checked by the corpus test `tests/corpus_abilities.rs`). Of the other modules each type has only an
//! excerpt: Walk (0), Stand (2), Dying (16), Attack (17, flags 0) and Swimming (15) for the camouflagers, Riding (13)
//! for the scout, Building (3) and Quarters (23) for the alarm post; Shoot, ranged combat, hunting, diving, boat and
//! self-healing are missing. The stand, dying, swimming and riding animations are test values. In addition a horse
//! (without modules) for mounting and with [`lake_rules`] a lake.
//!
//! Variants like `upgrades.ini`: bit 9 = 2922 (assassins dig in), bit 10 = 2924 (Indians camouflage). [`unlock`] before
//! [`spawn`] switches the variant on at creation; whoever unlocks afterwards calls
//! `variants::update_variant_mask(sim, id, u32::MAX, 0)`.
//!
//! Command bar and `targets.ini` as in the shipped data (kinds according to
//! `ability_buttons_and_filters_match_the_analysis`: all ability buttons `TARGET_DO`, 554/564/458/459 with `GROUP_AND`,
//! 555 `DO | GROUP_AND`). Button 555 "Self-healing" is in the shipped data only on Trapper 2015, Sanders 2100 and
//! McBride 2102 (all without module 29, `REQUIRED_GUID 3126`); here it is on the Pilgrim 2014 (without prerequisite,
//! test value for the commands) and on the Saber fighter 1107 (with 3126, test carrier without module 29).
//!
//! The world is created via [`SimState::new`], not via `setup::new_match`.
//!
//! [`evoke_rules`] is a world of its own for ghosts, banishing, animal evocation and auras (types, command bars,
//! filters, costs, auras and time markers in own tables `EVOKE_*`); [`rules`] stays unchanged
//! (`STEALTH_GOLDEN`). [`state_fogged`] is [`state`] without revealing.
//!
//! [`spell_rules`] is the world of the spells and special attacks (tables `SPELL_*`, with the bonus table of the
//! shipped data); [`rules`] and [`evoke_rules`] stay unchanged. [`run_ticks_collect`] returns the events of the steps.
//!
//! [`hunter_rules`] is the world of the headhunter and of robbing (tables `HUNTER_*`, built on [`spell_rules_from`]);
//! [`rules`], [`evoke_rules`] and [`spell_rules`] stay unchanged. [`hunter_state`] sets the nations of slots 0 and 1,
//! [`wall_ring`] blocks a ring (unreachable terrain); [`corpse`] and [`run_until_robbable`] create corpses for robbing.

use opennml_formats::gifcon::{parse_toolbar, parse_variant_rules};
use opennml_formats::targets::{parse_target_defines, parse_targets};
use opennml_formats::temp_upgrades::parse_temp_upgrades;

use crate::id::ObjectId;
use crate::rules::{BuildingShape, ModelBox, Rules};
use crate::sim::Sim;
use crate::state::{Control, SimEvent, SimState};
use crate::world::{ObjectDesc, create_object, desc_flags};

/// `WID_Bogenschuetze`: module 25 in variant 10 (2924), camouflage animation 7.
pub const ARCHER: u32 = 1402;
/// `WID_Waldlaeufer`: module 25 always on, `EVERYTIMETARNED`, camouflage animation 9.
pub const RANGER: u32 = 1410;
/// `PRI_Alarmposten`: building with module 25 (animations `0 1`, `EVERYTIMETARNED`) and quarters 2.
pub const ALARM_POST: u32 = 1610;
/// `PAT_Kundschafter`: module 25 always on, camouflage animation 4; button 434 with `REQUIRED_GUID 3220`.
pub const SCOUT: u32 = 2306;
/// `WID_Meucheleinheit`: module 28 in variant 9 (2922), animations 8/0/1, damage 250.
pub const ASSASSIN: u32 = 1407;
/// `ANI_Zombie`: module 28 always on, animations 5/4/5, damage 0 (re/masking 1.1, V11).
pub const ZOMBIE: u32 = 3422;
/// `SPA_Kolonist` (HP 160).
pub const COLONIST: u32 = 1100;
/// `SPA_Saebelkaempfer` (HP 190); carries button 555 here without module 29.
pub const SABRE: u32 = 1107;
/// `ENG_Pilgervater`: healer (module 29, `0x3400`), buttons 554/564 with `REQUIRED_GUID 3125`.
pub const PILGRIM: u32 = 2014;
/// `PAT_Feldarzt`: healer (module 29, `0x3400`), buttons 458/459.
pub const MEDIC: u32 = 2303;
/// `PAT_Farmerin` (HP 100).
pub const FARMER: u32 = 2302;
/// `SPA_Kloster`: healing building (`0x1500`).
pub const MONASTERY: u32 = 1015;
/// `SPA_Werft`: healing building with GUID list `1108 1109 1117` (`0x1700`).
pub const SHIPYARD: u32 = 1008;
/// `SPA_Transportschiff` (HP 1400).
pub const SHIP: u32 = 1108;
/// Horse (land, RUN, SADDLE; no module) like `riding::testkit::HORSE`.
pub const HORSE: u32 = 3410;

/// Upgrade "Assassins can dig in" (variant bit 9).
pub const DIG_IN_UPGRADE: u32 = 2922;
/// Upgrade "Indians can camouflage" (variant bit 10).
pub const MASK_UPGRADE: u32 = 2924;
/// Upgrade "Research healing" (buttons 554/564 of the pilgrim).
pub const HEAL_UPGRADE: u32 = 3125;
/// Prerequisite of button 555 in the shipped data.
pub const SELF_HEAL_UPGRADE: u32 = 3126;
/// Prerequisite of the camouflage button 434 on the scout.
pub const SCOUT_MASK_UPGRADE: u32 = 3220;
/// Sheriff "Uncamouflage units": temporary radius-opponent upgrade, config 115 = -100.
pub const SHERIFF: u32 = 4458;

/// Walk with the values of the shipped data (`10001`, `10003` flags, `10004`/`10005` radii, `10008`).
fn walk(anim: i32, flags: u32, hard: i32, soft: i32, w8: i32) -> String {
    format!(
        "PROPID 0\nVALUEID 10001 6 1 {anim}\nVALUEID 10003 6 1 {flags}\nVALUEID 10004 6 1 {hard}\n\
         VALUEID 10005 6 1 {soft}\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 {w8}\n"
    )
}

/// Stand and dying (test values: stand animation 0, dying animation 3).
const STAND_AND_DYING: &str =
    "PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n";
/// Attack module without flags (for `queue_module_tick_change(17, …)` of the abilities).
const ATTACK: &str = "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n";
/// Swimming (test animations like `riding::testkit`).
const SWIM: &str = "PROPID 15\nVALUEID 160002 1 1 3\nVALUEID 160003 1 1 4\nVALUEID 160004 1 1 5\n";
/// Riding (module 13) with the test animations of the colonist from `riding::testkit`.
const RIDING: &str = "PROPID 13\nVALUEID 140001 6 1 0\nVALUEID 140002 1 1 2\nVALUEID 140003 1 1 27\n\
VALUEID 140004 1 1 15\nVALUEID 140005 1 1 14\nVALUEID 140006 1 1 32\n";
/// `data/effects/healing_circle.hef` (300008 of all healers in the shipped data).
const HEF: &str = "data/effects/healing_circle.hef";

/// Unit: header with category, walk, stand, dying and the modules `rest`.
fn unit(guid: u32, name: &str, category: u32, walk: &str, rest: &str) -> String {
    format!("GUID {guid}\nNAME {name}\nVALUEID 100 6 1 {category}\n{walk}{STAND_AND_DYING}{rest}")
}

/// Module 29 with the values 300001-300008 (re/healing 1.1): removal animation or GUID list, flags, build-up, dying,
/// standing and healing animation, healing points per minute.
fn healing(
    remove: &str,
    flags: u32,
    establish: i32,
    die: i32,
    stand: &str,
    heal_anim: i32,
    points: i32,
) -> String {
    format!(
        "PROPID 29\nVALUEID 300001 1 1 {remove}\nVALUEID 300002 6 1 {flags}\n\
         VALUEID 300003 6 1 {establish}\nVALUEID 300004 6 1 {die}\nVALUEID 300005 1 1 {stand}\n\
         VALUEID 300006 6 1 {heal_anim}\nVALUEID 300007 6 1 {points}\nVALUEID 300008 7 1 {HEF}\n"
    )
}

/// Synthetic `property.ini`.
pub fn types() -> String {
    // Categories of the shipped data: `0x2000000` unit, `0x2800000` (ranger, pilgrim), `0x2001000` scout
    // (`CANCLEARUP`), `0x1000000` monastery, `0x2100000` shipyard, `0x2110000` ship.
    let soldier = walk(2, 276_736, 512, 1024, 0);
    [
        unit(
            ARCHER,
            "WID_Bogenschuetze",
            0x200_0000,
            &soldier,
            &format!("{ATTACK}{SWIM}PROPID 25 10\nVALUEID 260001 1 1 7\nVALUEID 260002 6 1 0\n"),
        ),
        unit(
            RANGER,
            "WID_Waldlaeufer",
            0x280_0000,
            &walk(0, 801_024, 512, 1024, 0),
            &format!("{ATTACK}{SWIM}PROPID 25\nVALUEID 260001 1 1 9\nVALUEID 260002 6 1 256\n"),
        ),
        // Alarm post: building like the dwelling tent of the tent world (finished variant, `0x4080000`), quarters and
        // module 25 (animations `0 1`, `EVERYTIMETARNED`).
        format!(
            "GUID {ALARM_POST}\nNAME PRI_Alarmposten\nVALUEID 100 6 1 33554432\n\
             PROPID 3\nVALUEID 40003 6 1 67633152\nVALUEID 40004 1 1 0\n\
             PROPID 25\nVALUEID 260001 1 1 0 1\nVALUEID 260002 6 1 256\n\
             PROPID 23\nVALUEID 240001 6 1 0\n"
        ),
        unit(
            SCOUT,
            "PAT_Kundschafter",
            0x200_1000,
            &walk(2, 268_544, 256, 512, 0),
            &format!("{RIDING}{ATTACK}PROPID 25\nVALUEID 260001 1 1 4\nVALUEID 260002 6 1 0\n"),
        ),
        unit(
            ASSASSIN,
            "WID_Meucheleinheit",
            0x200_0000,
            &walk(2, 276_736, 512, 1024, 2),
            &format!(
                "{ATTACK}PROPID 28 9\nVALUEID 290001 6 1 8\nVALUEID 290002 6 1 0\n\
                 VALUEID 290003 6 1 1\nVALUEID 290004 6 1 250\n"
            ),
        ),
        // Zombie: walk, stand, dying and dig in as in the shipped data; the animal module 20 and melee 18 are missing
        // (attack 17 without flags like the other types).
        unit(
            ZOMBIE,
            "ANI_Zombie",
            0x200_0000,
            &walk(0, 6400, 512, 1024, 2),
            &format!(
                "{ATTACK}PROPID 28
VALUEID 290001 6 1 5
VALUEID 290002 6 1 4
                 VALUEID 290003 6 1 5
VALUEID 290004 6 1 0
"
            ),
        ),
        unit(
            COLONIST,
            "SPA_Kolonist",
            0x200_0000,
            &walk(1, 264_448, 512, 1024, 0),
            "",
        ),
        unit(
            SABRE,
            "SPA_Saebelkaempfer",
            0x200_0000,
            &walk(1, 4_458_752, 512, 1024, 0),
            ATTACK,
        ),
        unit(
            PILGRIM,
            "ENG_Pilgervater",
            0x280_0000,
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!("{ATTACK}{}", healing("7", 0x3400, 7, 2, "0", 7, 288)),
        ),
        unit(
            MEDIC,
            "PAT_Feldarzt",
            0x200_0000,
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!("{ATTACK}{}", healing("6", 0x3400, 3, 5, "4", 7, 288)),
        ),
        unit(
            FARMER,
            "PAT_Farmerin",
            0x200_0000,
            &walk(1, 264_448, 512, 1024, 0),
            "",
        ),
        format!(
            "GUID {MONASTERY}\nNAME SPA_Kloster\nVALUEID 100 6 1 16777216\n\
             PROPID 3\nVALUEID 40003 6 1 0\n{}",
            healing("0", 0x1500, 15, 0, "0", 0, 180)
        ),
        format!(
            "GUID {SHIPYARD}\nNAME SPA_Werft\nVALUEID 100 6 1 34603008\n\
             PROPID 3\nVALUEID 40003 6 1 0\n{}",
            healing("1108 1109 1117", 0x1700, 0, 0, "0", 0, 600)
        ),
        // Ship: walk of the shipped data (water, radii 2100/4000) without variants.
        unit(
            SHIP,
            "SPA_Transportschiff",
            0x211_0000,
            &walk(1, 139_776, 2100, 4000, 0),
            "",
        ),
        // Horse: land, RUN, SADDLE (`0x20C900`, re/riding 1.3).
        unit(HORSE, "ENG_Pferd", 0, &walk(3, 0x20_C900, 512, 1024, 4), ""),
    ]
    .concat()
}

/// Command bars like `toolbar.ini` (kinds and prerequisites from the corpus; 555 see module doc).
pub const TOOLBAR: &str = concat!(
    "GUID 1402\n",
    "BUTTONID 5\nBUTTONMSG 434\nTARGET_DO_MSG\nREQUIRED_GUID 2924\n",
    "BUTTONID 5\nBUTTONMSG 435\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 2306\n",
    "BUTTONID 5\nBUTTONMSG 434\nTARGET_DO_MSG\nREQUIRED_GUID 3220\n",
    "BUTTONID 5\nBUTTONMSG 435\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 1407\n",
    "BUTTONID 4\nBUTTONMSG 437\nTARGET_DO_MSG\nREQUIRED_GUID 2922\n",
    "BUTTONID 4\nBUTTONMSG 438\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 2014\n",
    "BUTTONID 8\nBUTTONMSG 554\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 3125\n",
    "BUTTONID 8\nBUTTONMSG 564\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 3125\n",
    "BUTTONID 9\nBUTTONMSG 555\nGROUP_AND_MSG\nDO_MSG\n",
    "ENDGUID\n",
    "GUID 2303\n",
    "BUTTONID 7\nBUTTONMSG 458\nGROUP_AND_MSG\nTARGET_DO_MSG\n",
    "BUTTONID 7\nBUTTONMSG 459\nGROUP_AND_MSG\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 1107\n",
    "BUTTONID 8\nBUTTONMSG 555\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3126\n",
    "ENDGUID\n",
);

/// Entry of `targets.ini` with the MY filter `my` and the TARGET block `target` (up to `MM_END`, plus the lines after
/// the block such as `MAUSICONID`).
macro_rules! entry {
    ($msg:literal, $my:literal, $target:literal) => {
        concat!(
            "BEGIN\nBUTTONMSG ",
            $msg,
            "\nMY_MM\n",
            $my,
            "MM_END\nTARGET_MM\n",
            $target,
            "END\n"
        )
    };
}

/// Entries of the ability buttons as in the shipped data: MY filter 434 not `MASKED` and (following filter) not
/// `SWIMMING`, 435 `MASKED`, 437/438 by `WAITING4KILLING` (TARGET in each case `RETURNFALSE`, re/masking 2.4); 458/554
/// not `HEAL`, 459/564 `HEAL` (TARGET empty, `MAUSICONID 7`, re/healing 2.4). 555 has no entry.
pub const TARGETS: &str = concat!(
    entry!(
        "434",
        "MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS MASKED\nMM_MODE MM_ANDALL\nMM_MODE MM_ANDNEXTMATCH\n\
         MM_NEXT\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\n"
    ),
    entry!(
        "435",
        "MM_MODE MM_CHECKSTATUS\nMM_STATUS MASKED\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\n"
    ),
    entry!(
        "437",
        "MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS WAITING4KILLING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\n"
    ),
    entry!(
        "438",
        "MM_MODE MM_CHECKSTATUS\nMM_STATUS WAITING4KILLING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\n"
    ),
    entry!(
        "458",
        "MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS HEAL\n",
        "MM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "459",
        "MM_MODE MM_CHECKSTATUS\nMM_STATUS HEAL\n",
        "MM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "554",
        "MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS HEAL\n",
        "MM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "564",
        "MM_MODE MM_CHECKSTATUS\nMM_STATUS HEAL\n",
        "MM_END\nMAUSICONID 7\n"
    ),
);

/// Variant rules (`upgrades.ini`, excerpt).
pub const UPGRADES: &str = "BIT 0 = 1\nBIT 9 = 2922\nBIT 10 = 2924\n";

/// Sheriff 4458 like `tempupgrades.ini` (flag `0x2D9`; the messages `0x601` in the record of the shipped data are
/// leftovers of the predecessor without flag, W-18).
pub const TEMP_UPGRADES: &str = concat!(
    "TEMPUPGRADE 4458\nTEMPRADIUSUPGRADE\nENEMYUPGRADE\nTAKEUPGRADEVALUES\n",
    "EFFECT 0 data/effects/units_enttarnen.pef\nEND\n",
);

/// `default.dat` values (config) of the shipped data: 13 build time, 14 hit points, 15 extra energy, 16 speed, 18
/// sight, 23 capacity.
pub const DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (ARCHER, &[(14, 155), (15, 200), (16, 200), (18, 40)]),
    (RANGER, &[(14, 1800), (15, 600), (16, 227), (18, 64)]),
    (
        ALARM_POST,
        &[(13, 18), (14, 1600), (15, 500), (18, 64), (23, 2)],
    ),
    (SCOUT, &[(14, 155), (15, 200), (16, 200), (18, 52)]),
    (ASSASSIN, &[(14, 200), (15, 0), (16, 145), (18, 40)]),
    (ZOMBIE, &[(14, 300), (15, 0), (16, 128), (18, 32)]),
    (COLONIST, &[(14, 160), (16, 183), (18, 32)]),
    (SABRE, &[(14, 190), (16, 200), (18, 32)]),
    (PILGRIM, &[(14, 1000), (15, 950), (16, 183), (18, 32)]),
    (MEDIC, &[(14, 200), (15, 950), (16, 200), (18, 40)]),
    (FARMER, &[(14, 100), (16, 183), (18, 32)]),
    (MONASTERY, &[(13, 50), (14, 3000), (15, 0), (18, 40)]),
    (SHIPYARD, &[(13, 50), (14, 3000), (15, 0), (18, 40)]),
    (SHIP, &[(14, 1400), (16, 128), (18, 40)]),
    (HORSE, &[(14, 100), (16, 227), (18, 32)]),
    // Sheriff: 60 s, extra energy -100 %.
    (SHERIFF, &[(13, 60), (115, -100)]),
    // Value set of the button GUID 555 (cost in config 15): test value 50; the shipped data states 40 in `default.dat`
    // and in all slot sets (corpus, K-311).
    (555, &[(15, 50)]),
];

/// `[UPGRADE]` of the project file: the upgrades of the test world.
pub const UPGRADE_GUIDS: &[u32] = &[
    DIG_IN_UPGRADE,
    MASK_UPGRADE,
    HEAL_UPGRADE,
    SELF_HEAL_UPGRADE,
    SCOUT_MASK_UPGRADE,
    SHERIFF,
];

/// Upgrade -> types (`OBJ_UPGRADES` of the shipped data, restricted to the types of the test world; 3126 names in the
/// shipped data only Trapper 2015 and Sanders 2100).
pub const UPGRADE_TYPES: &[(u32, &[u32])] = &[
    (DIG_IN_UPGRADE, &[ASSASSIN]),
    (MASK_UPGRADE, &[ARCHER]),
    (HEAL_UPGRADE, &[PILGRIM]),
    (SCOUT_MASK_UPGRADE, &[SCOUT]),
];

/// Time markers of an animation: type, animation, `(marker, ms)`.
type AnimLabels = (u32, usize, &'static [(u32, i32)]);

/// Time markers of the ability animations as in the corpus (`ability_animation_labels`). The alarm post has marker 1 at
/// 33 ms in both animations in the game model `alarm_baum.rdo`; the real rules load them (time markers for all types
/// with animations, not only for modules 2, 10, 16-20); because of `m_MaskTime = max(marker, 1000)` the masking time
/// stays 1000 ms.
const LABELS: &[AnimLabels] = &[
    (ARCHER, 7, &[(1, 1600)]),
    (RANGER, 9, &[(1, 1600)]),
    (ALARM_POST, 0, &[(1, 33)]),
    (ALARM_POST, 1, &[(1, 33)]),
    (SCOUT, 4, &[(1, 2133)]),
    (ASSASSIN, 0, &[(1, 773)]),
    (ASSASSIN, 1, &[(1, 1180), (2, 640), (22, 960)]),
    (ASSASSIN, 8, &[(1, 1843)]),
    // Zombie: dig in and stab 5 (only marker 1, no marker 2), dig out 4.
    (ZOMBIE, 4, &[(1, 2000)]),
    (ZOMBIE, 5, &[(1, 33)]),
    (PILGRIM, 7, &[(1, 3999)]),
    (MEDIC, 3, &[(1, 4500)]),
    (MEDIC, 6, &[(1, 4500)]),
];

/// Hull of the alarm post (re/tents 9.2) and test hulls of the healing buildings (3 x 3 tiles).
const SHAPES: &[(u32, [f32; 3], [f32; 3])] = &[
    (
        ALARM_POST,
        [-36.554_054, 234.338_36, -11.222_935],
        [121.663_98, 234.551_22, 97.467_606],
    ),
    (MONASTERY, [0.0; 3], [100.0, 50.0, 100.0]),
    (SHIPYARD, [0.0; 3], [100.0, 50.0, 100.0]),
];

/// Rules of the test world.
pub fn rules() -> Rules {
    rules_from(&types())
}

/// [`rules`] with other types (e.g. [`types`] with additional types of a test).
pub fn rules_from(types: &str) -> Rules {
    let mut r = Rules::synthetic(types, 64);
    for &(guid, values) in DEFAULTS {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    r.upgrades = UPGRADE_GUIDS.iter().copied().collect();
    for &(guid, types) in UPGRADE_TYPES {
        r.upgrade_types.insert(guid, types.to_vec());
    }
    r.temp_upgrades = parse_temp_upgrades(TEMP_UPGRADES)
        .expect("synthetic tempupgrades.ini")
        .into_iter()
        .map(|t| (t.guid, t))
        .collect();
    for &(guid, center, half) in SHAPES {
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
    r.toolbar = parse_toolbar(TOOLBAR);
    r.targets = parse_targets(TARGETS, &parse_target_defines("")).entries;
    r.variant_rules = parse_variant_rules(UPGRADES);
    for &(guid, anim, labels) in LABELS {
        r.set_anim_labels(guid, anim, labels);
    }
    r
}

// --- Test world of the evocations ----------------------------------------------------------------
//
// Own tables beside those of `rules()`: the scene of `STEALTH_GOLDEN` uses 1402 and 2014 from `rules()`, so this world
// stays unchanged. `evoke_rules()` takes `types()` and appends the evocation types; the Pilgrim 2014 appears there a
// second time, complete and with module 31 (in `property.ini` the later GUID replaces the earlier). Values from
// re/ghosts-banish 1.1, re/evoke-aura 1.1 and re/temp-upgrades 1.4, checked by the corpus test
// `tests/corpus_abilities.rs` (`evocation_type_params_match_the_analysis`,
// `evocation_buttons_and_filters_match_the_analysis`). Category of all evocation units `0x2000000` (animals as in the
// shipped data; with `0x400`, `create_object` would set slot 8). The melee and ranged values are copied from
// `combat::testkit` (its building blocks are private).

/// `WID_Geisterbeschwoerer`: modules 26, 38 (spell, mana tick) and 31 (banishing, anim 10).
pub const GHOST_EVOKER: u32 = 1411;
/// `WID_Geist`: floating (walk `0x1900`), module 27, FightFar; the GUID creates module 26.
pub const GHOST: u32 = 1413;
/// `WID_Tierbeschwoerer`: module 32.
pub const ANIMAL_EVOKER: u32 = 1412;
/// `ANI_Katze` (puma): animal kind 2, aura 4462, no meat module 21.
pub const CAT: u32 = 3400;
/// `ANI_Wolf`: animal kind 0, aura 4460.
pub const WOLF: u32 = 3401;
/// `ANI_Baer`: animal kind 1, aura 4461.
pub const BEAR: u32 = 3402;
/// `WID_Krieger` (melee 9, re/evoke-aura V7).
pub const WARRIOR: u32 = 1400;
/// `WID_Gewehrschuetze` as the ranged fighter of the aura tests. Ranged 11 is the value of the archer 1402 from V7
/// (shipped data 1403: 22); 1402 stays unchanged (`STEALTH_GOLDEN`).
pub const RANGED: u32 = 1403;
/// `SPA_Missionar`: module 31 (`0xD00`, anim 6, `bannspruch`) and riding.
pub const MISSIONARY: u32 = 1113;

/// Aura of the wolf (melee/ranged +60 %).
pub const WOLF_AURA: u32 = 4460;
/// Aura of the bear (hit points +60 %).
pub const BEAR_AURA: u32 = 4461;
/// Aura of the wildcat (speed +60 %).
pub const CAT_AURA: u32 = 4462;
/// Research "Evoke wolf" (button 441).
pub const EVOKE_WOLF_UPGRADE: u32 = 2926;
/// Research "Evoke wildcat" (button 443).
pub const EVOKE_CAT_UPGRADE: u32 = 2927;
/// Research "Evoke bear" (button 442).
pub const EVOKE_BEAR_UPGRADE: u32 = 2928;
/// Research "Evoke ghost" (button 436).
pub const EVOKE_GHOST_UPGRADE: u32 = 2930;
/// Research "Banish ghosts" of the ghost evoker (button 547).
pub const BANISH_GHOSTS_UPGRADE: u32 = 2931;
/// Research "Speak banishment" of the missionary (button 439).
pub const MISSIONARY_BANISH_UPGRADE: u32 = 2827;
/// Research "Banish ghosts" of the pilgrim (button 553).
pub const PILGRIM_BANISH_UPGRADE: u32 = 3124;

/// Spell (module 38) with the flags 390001 = 768 of the ghost evoker.
const SPELL: &str = "PROPID 38\nVALUEID 390001 6 1 768\n";
/// Melee of the warrior (values like `combat::testkit::WARRIOR`): attack 17 with `16640`, FightNear 18 with combat
/// animation 14.
const MELEE: &str = "PROPID 17\nVALUEID 180001 6 1 16640\nVALUEID 180002 6 1 0\n\
PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 14\n";
/// Melee of the animals (values like `combat::testkit::WOLF`): attack 17 with `256`, FightNear 18 with combat animation
/// 2.
const ANIMAL_MELEE: &str = "PROPID 17\nVALUEID 180001 6 1 256\nVALUEID 180002 6 1 0\n\
PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 2\n";

/// Module 31 (re/ghosts-banish 1.3): target filter 320001 = 440, flags 320003, banish animation 320004, node prefix
/// 320005 (the original does not read 320006).
fn banish(flags: u32, anim: i32, node: &str) -> String {
    format!(
        "PROPID 31\nVALUEID 320001 6 1 440\nVALUEID 320003 6 1 {flags}\nVALUEID 320004 6 1 {anim}\n\
         VALUEID 320005 7 1 {node}\n"
    )
}

/// Evocable animal (re/evoke-aura 1.1): walk of the wolf in the shipped data, stand, dying, animal 20 (`0x2100`, kind
/// `kind`), meat 21 (only `edible`), melee and module 44 with the aura `aura` (450001) and the influence effect 450003;
/// 450002 missing (empty in the shipped data).
fn animal(guid: u32, name: &str, kind: i32, edible: bool, aura: u32, hef: &str) -> String {
    let edible = if edible { "PROPID 21\n" } else { "" };
    unit(
        guid,
        name,
        0x200_0000,
        &walk(1, 2_119_936, 1024, 2048, 1),
        &format!(
            "PROPID 20\nVALUEID 210001 6 1 8448\nVALUEID 210002 6 1 {kind}\n{edible}{ANIMAL_MELEE}\
             PROPID 44\nVALUEID 450001 1 1 {aura}\nVALUEID 450003 7 1 data/effects/{hef}\n"
        ),
    )
}

/// Attack 17 of the ranged fighters (flags `10240` like the archer of `combat::testkit`).
const FAR_ATTACK: &str = "PROPID 17\nVALUEID 180001 6 1 10240\nVALUEID 180002 6 1 0\n";
/// FightFar 19 (values like in `combat::testkit`).
const FIGHT_FAR: &str = "PROPID 19\nVALUEID 200001 6 1 0\n";

/// Ranged fighter 1403 (attack `10240` and FightFar like the archer of `combat::testkit`), shared building block of the
/// evocation world and the spell world (`evoke_types()` stays byte-identical).
fn ranged_unit() -> String {
    unit(
        RANGED,
        "WID_Gewehrschuetze",
        0x200_0000,
        &walk(1, 276_736, 512, 1024, 0),
        &format!("{FAR_ATTACK}{FIGHT_FAR}"),
    )
}

/// The evocation types (to be appended behind [`types`]).
fn evoke_only_types() -> String {
    let soldier = walk(1, 276_736, 512, 1024, 0);
    [
        unit(
            GHOST_EVOKER,
            "WID_Geisterbeschwoerer",
            0x200_0000,
            &soldier,
            &format!(
                "PROPID 26\nVALUEID 270001 6 1 0\n{SPELL}{ATTACK}{}",
                banish(0x500, 10, "geisterhand")
            ),
        ),
        // Ghost: walk without hard radius, `SIZE1x1 | LAND | FOREST` without `SETTOTERRAIN`; attack `FIGHTFAR`
        // (`0x800`) and FightFar (values like `combat::testkit`).
        unit(
            GHOST,
            "WID_Geist",
            0x200_0000,
            &walk(0, 0x1900, 0, 1024, 0),
            "PROPID 27\nVALUEID 280001 6 1 0\nPROPID 17\nVALUEID 180001 6 1 2048\n\
             VALUEID 180002 6 1 0\nPROPID 19\nVALUEID 200001 6 1 0\n",
        ),
        unit(
            ANIMAL_EVOKER,
            "WID_Tierbeschwoerer",
            0x200_0000,
            &soldier,
            &format!("{SWIM}PROPID 32\nVALUEID 330001 6 1 0\n{ATTACK}"),
        ),
        animal(CAT, "ANI_Katze", 2, false, CAT_AURA, "wildkatze.hef"),
        animal(WOLF, "ANI_Wolf", 0, true, WOLF_AURA, "wolf.hef"),
        animal(BEAR, "ANI_Baer", 1, true, BEAR_AURA, "baer.hef"),
        unit(
            WARRIOR,
            "WID_Krieger",
            0x200_0000,
            &walk(3, 276_736, 512, 1024, 0),
            MELEE,
        ),
        ranged_unit(),
        // Missionary: riding with the test banish animation 12 in the saddle (140010, `0x50D {4}`; shipped data 7),
        // banishing and attack without flags (banishing switches module 17 off).
        unit(
            MISSIONARY,
            "SPA_Missionar",
            0x200_0000,
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!(
                "{RIDING}VALUEID 140010 1 1 12\n{ATTACK}{}",
                banish(0xD00, 6, "bannspruch")
            ),
        ),
        // Pilgrim like in `types()` and additionally banishing (anim 6, button 553).
        unit(
            PILGRIM,
            "ENG_Pilgervater",
            0x280_0000,
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!(
                "{ATTACK}{}{}",
                healing("7", 0x3400, 7, 2, "0", 7, 288),
                banish(0x500, 6, "bannspruch")
            ),
        ),
    ]
    .concat()
}

/// Synthetic `property.ini` of the evocation world: [`types`] followed by the evocation types.
pub fn evoke_types() -> String {
    format!("{}{}", types(), evoke_only_types())
}

/// Command bars of the evocation world (button kinds and slots as in the shipped data,
/// `evocation_buttons_and_filters_match_the_analysis`). To be parsed before [`TOOLBAR`] (`parse_toolbar` skips a later
/// block of the same GUID): the 2014 block repeats 554/564/555 from [`TOOLBAR`] and puts 553 on slot 9. 555 (test value
/// of the world of [`rules`]) is here, as in the shipped data (Trapper, Sanders, McBride), on slot 8 beside 554/564 so
/// that slot 9 carries only 553; in [`TOOLBAR`] it stays on 9.
pub const EVOKE_TOOLBAR: &str = concat!(
    "GUID 1411\n",
    "BUTTONID 8\nBUTTONMSG 436\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 2930\n",
    "BUTTONID 9\nBUTTONMSG 547\nDO_MSG\nREQUIRED_GUID 2931\n",
    "ENDGUID\n",
    "GUID 1412\n",
    "BUTTONID 7\nBUTTONMSG 443\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 2927\n",
    "BUTTONID 8\nBUTTONMSG 441\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 2926\n",
    "BUTTONID 9\nBUTTONMSG 442\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 2928\n",
    "ENDGUID\n",
    "GUID 1113\n",
    "BUTTONID 9\nBUTTONMSG 439\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 2827\n",
    "ENDGUID\n",
    "GUID 2014\n",
    "BUTTONID 8\nBUTTONMSG 554\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 3125\n",
    "BUTTONID 8\nBUTTONMSG 564\nGROUP_AND_MSG\nTARGET_DO_MSG\nREQUIRED_GUID 3125\n",
    "BUTTONID 8\nBUTTONMSG 555\nGROUP_AND_MSG\nDO_MSG\n",
    "BUTTONID 9\nBUTTONMSG 553\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3124\n",
    "ENDGUID\n",
);

/// `targetdefines.ini` excerpt for [`EVOKE_TARGETS`]: without the names `parse_targets` silently aborts at
/// `MM_PROPERTY` and discards all entries after it (`opennml-formats/src/targets.rs`).
pub const EVOKE_DEFINES: &str = "PROP_EVOKEGHOST 26\nPROP_GHOST 27\nPROP_EVOKEANIMAL 32\n";

/// Entries as in the shipped data (re/ghosts-banish 2.6): 436 MY `PROP_EVOKEGHOST` and not `SWIMMING` (TARGET
/// `RETURNFALSE`); 440 "ghost as target", the target filter 320001 of banishing: MY `RETURNFALSE`, TARGET ANDALL,
/// CHECKPROPERTY `PROP_GHOST`, CHECKFOG, ENEMYLIST (without `MM_CHECKPROPERTY` the filter would banish every visible
/// enemy). The banish buttons have no entry. 441-443 have, like 436, a MY filter, with `PROP_EVOKEANIMAL` (corpus
/// finding, `evocation_buttons_and_filters_match_the_analysis`, re/evoke-aura 2.4). The `DISABLERBUTTON` before
/// `BUTTONMSG` has no effect in the shipped data and is missing here.
pub const EVOKE_TARGETS: &str = concat!(
    entry!(
        "436",
        "MM_MODE MM_CHECKPROPERTY\nMM_PROPERTY PROP_EVOKEGHOST\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\nMAUSICONID 0\n"
    ),
    entry!(
        "440",
        "MM_MODE MM_RETURNFALSE\n",
        "MM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_CHECKFOG\n\
         MM_MODE MM_HL_ENEMYLIST\nMM_PROPERTY PROP_GHOST\nMM_END\nMAUSICONID 0\n"
    ),
    entry!(
        "441",
        "MM_MODE MM_CHECKPROPERTY\nMM_PROPERTY PROP_EVOKEANIMAL\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\nMAUSICONID 0\n"
    ),
    entry!(
        "442",
        "MM_MODE MM_CHECKPROPERTY\nMM_PROPERTY PROP_EVOKEANIMAL\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\nMAUSICONID 0\n"
    ),
    entry!(
        "443",
        "MM_MODE MM_CHECKPROPERTY\nMM_PROPERTY PROP_EVOKEANIMAL\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKNOTSTATUS\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_RETURNFALSE\nMM_END\nMAUSICONID 0\n"
    ),
);

/// Auras 4460-4462 like `tempupgrades.ini` (flag `0x49` = `TAKEUPGRADEVALUES | EFFECT`, without `0x80` permanent;
/// re/temp-upgrades 1.4).
pub const EVOKE_TEMP_UPGRADES: &str = concat!(
    "TEMPUPGRADE 4460\nTAKEUPGRADEVALUES\nEFFECT 0 data/effects/units_wolf.pef\nEND\n",
    "TEMPUPGRADE 4461\nTAKEUPGRADEVALUES\nEFFECT 0 data/effects/units_bär.pef\nEND\n",
    "TEMPUPGRADE 4462\nTAKEUPGRADEVALUES\nEFFECT 0 data/effects/units_puma.pef\nEND\n",
);

/// `default.dat` values of the evocation world (in addition to [`DEFAULTS`]): 14 hit points, 15 extra energy, 16 speed,
/// 18 sight, 19 melee, 20 ranged, 21 range, 34 minimum distance; the values of the auras (like the sheriff 4458) and
/// the costs of the buttons (config 15 of the button GUID).
pub const EVOKE_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    // Speed "Slow" 145 as in the shipped data.
    (GHOST_EVOKER, &[(14, 900), (15, 150), (16, 145), (18, 52)]),
    (
        GHOST,
        &[(14, 250), (15, 0), (16, 145), (18, 40), (20, 0), (21, 8)],
    ),
    (
        ANIMAL_EVOKER,
        &[(14, 1200), (15, 100), (16, 145), (18, 52), (19, 20)],
    ),
    (WOLF, &[(14, 80), (16, 227), (18, 20), (19, 15)]),
    (BEAR, &[(14, 120), (16, 145), (18, 20), (19, 30)]),
    (CAT, &[(14, 80), (16, 227), (18, 20), (19, 10)]),
    (WARRIOR, &[(14, 160), (16, 183), (18, 40), (19, 9), (21, 8)]),
    // HP 250 as in the shipped data, ranged 11 as test value (see [`RANGED`]).
    (RANGED, &[(14, 250), (20, 11), (21, 28), (34, 4)]),
    // Without hit points and mana the missionary could never banish (re/spells 1.1).
    (MISSIONARY, &[(14, 1000), (15, 200), (16, 145), (18, 40)]),
    (WOLF_AURA, &[(119, 60), (120, 60)]),
    (BEAR_AURA, &[(114, 60)]),
    (CAT_AURA, &[(116, 60)]),
    (436, &[(15, 25)]),
    (547, &[(15, 25)]),
    (439, &[(15, 25)]),
    (553, &[(15, 200)]),
    (441, &[(15, 25)]),
    (442, &[(15, 50)]),
    (443, &[(15, 30)]),
];

/// Research of the evocation world (in addition to [`UPGRADE_GUIDS`]).
pub const EVOKE_UPGRADE_GUIDS: &[u32] = &[
    EVOKE_WOLF_UPGRADE,
    EVOKE_CAT_UPGRADE,
    EVOKE_BEAR_UPGRADE,
    EVOKE_GHOST_UPGRADE,
    BANISH_GHOSTS_UPGRADE,
    MISSIONARY_BANISH_UPGRADE,
    PILGRIM_BANISH_UPGRADE,
];

/// `OBJ_UPGRADES` of the auras: the 13 types of the shipped data, restricted to the types of the world, by GUID as in
/// the loaded rules (`Rules::load` collects via the type map).
const AURA_TYPES: &[u32] = &[
    WARRIOR,
    ARCHER,
    RANGED,
    ASSASSIN,
    RANGER,
    GHOST_EVOKER,
    ANIMAL_EVOKER,
];

/// Time markers of the evocation world (`ability_animation_labels`); 1113 anim 12 is the test banish animation in the
/// saddle.
const EVOKE_LABELS: &[AnimLabels] = &[
    (GHOST_EVOKER, 0, &[(1, 3999)]),
    (GHOST_EVOKER, 10, &[(1, 5332)]),
    (ANIMAL_EVOKER, 0, &[(1, 8000)]),
    (MISSIONARY, 6, &[(1, 9332)]),
    (MISSIONARY, 12, &[(1, 2000)]),
    (PILGRIM, 6, &[(1, 4666)]),
    (GHOST, 0, &[(1, 1666)]),
];

/// Rules of the evocation world: [`evoke_rules_from`] with [`evoke_types`].
pub fn evoke_rules() -> Rules {
    evoke_rules_from(&evoke_types())
}

/// [`evoke_rules`] with other types (e.g. [`evoke_types`] with additional types of a test): [`rules_from`] and above it
/// the tables of the evocations.
pub fn evoke_rules_from(types: &str) -> Rules {
    let mut r = rules_from(types);
    for &(guid, values) in EVOKE_DEFAULTS {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    r.upgrades.extend(EVOKE_UPGRADE_GUIDS.iter().copied());
    for guid in [WOLF_AURA, BEAR_AURA, CAT_AURA] {
        r.upgrade_types.insert(guid, AURA_TYPES.to_vec());
    }
    r.temp_upgrades.extend(
        parse_temp_upgrades(EVOKE_TEMP_UPGRADES)
            .expect("synthetic tempupgrades.ini")
            .into_iter()
            .map(|t| (t.guid, t)),
    );
    r.toolbar = parse_toolbar(&format!("{EVOKE_TOOLBAR}{TOOLBAR}"));
    r.targets = parse_targets(
        &format!("{TARGETS}{EVOKE_TARGETS}"),
        &parse_target_defines(EVOKE_DEFINES),
    )
    .entries;
    for &(guid, anim, labels) in EVOKE_LABELS {
        r.set_anim_labels(guid, anim, labels);
    }
    r
}

// --- Test world of the spells and special attacks -----------------------------------------------
//
// Like the evocation world a world of its own beside `rules()`/`evoke_rules()` (both stay byte-identical,
// `STEALTH_GOLDEN`/`EVOKE_GOLDEN`): `spell_types()` takes `types()` and appends the spell types. Values from re/spells
// 1.1, 2.4, 2.5 and re/special-attacks 1.1, 2.5, checked by the corpus test `tests/corpus_abilities.rs`
// (`spell_type_params_match_the_analysis`, `special_attack_type_params_match_the_analysis`,
// `spell_buttons_and_filters_match_the_analysis`). Category, walk and ability values as in the shipped data; stand and
// dying are the test values of `STAND_AND_DYING`, attack 17 without flags (ranged fighter `10240` as in
// `combat::testkit`), melee 18 with the combat animation of the shipped data. The targets come from `types()` (colonist
// 1100, monastery 1015, ship 1108 with stand flag) and from the evocation world (rifleman 1403 as a pure ranged
// fighter). The ghost evoker 1411 is missing.

/// `PRI_Medizinmann`: spell 38 (`SPELLEXPLOSION | SPELLSTORM`, animation 2), buttons 480-482.
pub const MEDICINE_MAN: u32 = 1716;
/// `PRI_Hauptling`: scout area 484 (no module for it, only the command `0x37`).
pub const CHIEF: u32 = 1715;
/// `PAT_Patriot`: demolition expert 36, riding module 13; buttons 454/455.
pub const PATRIOT: u32 = 2315;
/// `PRI_Schattenkrieger`: killer 37 near (animation 5, damage 300), button 548.
pub const SHADOW_WARRIOR: u32 = 1714;
/// `ENG_Trapper`: killer 37 near (animation 8, damage 300), buttons 456 and 556.
pub const TRAPPER: u32 = 2015;
/// `SPA_Pistolenschuetze`: killer 37 far (animation 9, damage 300), **without** melee 18; button 546.
pub const PISTOLERO: u32 = 1115;
/// `SID_Scharfschuetze`: killer 37 far (animation 1, damage 300) with melee 18; button 457.
pub const SHARPSHOOTER: u32 = 2616;
/// `ENG_Schmied`: SpecialMove 49 (animations `4 5`), buttons 551 and 542.
pub const SMITH: u32 = 2013;
/// `ANI_Kampfwolf`: reincarnation (animal flags `0x2100`, category `0x2000000` without `0x400`).
pub const BATTLE_WOLF: u32 = 3420;

/// Spell 38 (re/spells 1.1): flags 390001, animations 390002, effect indices 390003.
fn spell(flags: u32, anims: &str, effects: &str) -> String {
    format!(
        "PROPID 38\nVALUEID 390001 6 1 {flags}\nVALUEID 390002 1 1 {anims}\n\
         VALUEID 390003 1 1 {effects}\n"
    )
}

/// FightNear 18 with the combat animation `anim`.
fn fight_near(anim: i32) -> String {
    format!("PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 {anim}\n")
}

/// Killer 37 (re/special-attacks 1.1): 380001 flags (`0x100` near, `0x200` far), 380002 near animation, 380003 far
/// animation, 380004 near damage, 380005 far damage.
fn killer(flags: u32, near: i32, far: i32, near_damage: i32, far_damage: i32) -> String {
    format!(
        "PROPID 37\nVALUEID 380001 6 1 {flags}\nVALUEID 380002 1 1 {near}\n\
         VALUEID 380003 1 1 {far}\nVALUEID 380004 1 1 {near_damage}\n\
         VALUEID 380005 1 1 {far_damage}\n"
    )
}

/// Demolition expert 36 of the patriot (re/special-attacks 1.1): flags 768 (`CANBLOWUPBUILDING | CANBLOWUPSHIP`),
/// charge model, 370003-370006. The model is given with the full path of the shipped data, not just
/// `dynamitbuendel.rdo`.
const BOMBERMAN: &str = "PROPID 36\nVALUEID 370001 6 1 768\n\
VALUEID 370002 7 1 data/r3d/misc/Tools/Dynamitbuendel/dynamitbuendel.rdo\n\
VALUEID 370003 1 1 1\nVALUEID 370004 1 1 2\nVALUEID 370005 1 1 2\nVALUEID 370006 1 1 3\n";

/// SpecialMove 49 of the smith (re/special-attacks 1.1): 500001 = 0, animations `4 5` (whirlwind, hammering).
const SPECIAL_MOVE: &str = "PROPID 49\nVALUEID 500001 6 1 0\nVALUEID 500002 1 1 4 5\n";

/// The spell types (to be appended behind [`types`]). The ship 1108 appears a second time (the later GUID replaces the
/// earlier): stand with `30003 = 256` (`STAND_ONWATER`, shipped data and `boarding::testkit`), otherwise like in
/// [`types`].
fn spell_only_types() -> String {
    // Category `0x2800000` of all spell-world units in the shipped data; the battle wolf `0x2000000`.
    const CATEGORY: u32 = 0x280_0000;
    [
        unit(
            MEDICINE_MAN,
            "PRI_Medizinmann",
            CATEGORY,
            &walk(1, 276_736, 512, 1024, 0),
            &format!("{ATTACK}{}", spell(768, "2", "0")),
        ),
        unit(
            CHIEF,
            "PRI_Hauptling",
            CATEGORY,
            &walk(1, 272_640, 512, 1024, 0),
            ATTACK,
        ),
        // Missionary of the spell world (kind 5, animations `9 10`); independent of the 1113 of the evocation world.
        unit(
            MISSIONARY,
            "SPA_Missionar",
            CATEGORY,
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!("{ATTACK}{}", spell(0, "9 10", "1")),
        ),
        unit(
            PATRIOT,
            "PAT_Patriot",
            CATEGORY,
            &walk(0, 4_466_944, 512, 1024, 15),
            &format!("{RIDING}{ATTACK}{BOMBERMAN}"),
        ),
        unit(
            SHADOW_WARRIOR,
            "PRI_Schattenkrieger",
            CATEGORY,
            &walk(1, 276_736, 512, 1024, 0),
            &format!("{ATTACK}{}{}", fight_near(5), killer(256, 5, 0, 300, 0)),
        ),
        unit(
            TRAPPER,
            "ENG_Trapper",
            CATEGORY,
            &walk(1, 272_640, 512, 1024, 0),
            &format!("{ATTACK}{}{}", fight_near(8), killer(256, 8, 0, 300, 0)),
        ),
        unit(
            PISTOLERO,
            "SPA_Pistolenschuetze",
            CATEGORY,
            &walk(1, 4_466_944, 512, 1024, 0),
            &format!("{FAR_ATTACK}{FIGHT_FAR}{}", killer(512, 0, 9, 0, 300)),
        ),
        unit(
            SHARPSHOOTER,
            "SID_Scharfschuetze",
            CATEGORY,
            &walk(0, 4_471_040, 512, 1024, 0),
            &format!(
                "{FAR_ATTACK}{}{FIGHT_FAR}{}",
                fight_near(6),
                killer(512, 0, 1, 0, 300)
            ),
        ),
        unit(
            SMITH,
            "ENG_Schmied",
            CATEGORY,
            &walk(0, 4_458_752, 512, 1024, 0),
            &format!("{ATTACK}{}{SPECIAL_MOVE}", fight_near(3)),
        ),
        // Battle wolf: walk of the shipped data, animal 20 (`0x2100`, kind 0), melee like the animals of the evocation
        // world, meat 21.
        unit(
            BATTLE_WOLF,
            "ANI_Kampfwolf",
            0x200_0000,
            &walk(1, 2_119_936, 1024, 2048, 1),
            &format!(
                "PROPID 20\nVALUEID 210001 6 1 8448\nVALUEID 210002 6 1 0\n{ANIMAL_MELEE}PROPID 21\n"
            ),
        ),
        ranged_unit(),
        format!(
            "GUID {SHIP}\nNAME SPA_Transportschiff\nVALUEID 100 6 1 {}\n{}\
             PROPID 2\nVALUEID 30001 1 1 0\nVALUEID 30003 6 1 256\n\
             PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
            0x211_0000,
            walk(1, 139_776, 2100, 4000, 0)
        ),
    ]
    .concat()
}

/// Synthetic `property.ini` of the spell world: [`types`] followed by the spell types.
pub fn spell_types() -> String {
    format!("{}{}", types(), spell_only_types())
}

/// Command bars of the spell world like `toolbar.ini` (slots, kinds and `REQUIRED_GUID` of the shipped data,
/// `spell_buttons_and_filters_match_the_analysis`; all with `GROUP_AND_MSG`). To be parsed before [`TOOLBAR`] (no GUID
/// occurs in both).
pub const SPELL_TOOLBAR: &str = concat!(
    "GUID 1716\n",
    "BUTTONID 7\nBUTTONMSG 480\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3024\n",
    "BUTTONID 8\nBUTTONMSG 481\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3025\n",
    "BUTTONID 9\nBUTTONMSG 482\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3026\n",
    "ENDGUID\n",
    "GUID 1715\n",
    "BUTTONID 8\nBUTTONMSG 484\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3022\n",
    "ENDGUID\n",
    "GUID 1113\n",
    "BUTTONID 8\nBUTTONMSG 541\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 2832\n",
    "ENDGUID\n",
    "GUID 2315\n",
    "BUTTONID 7\nBUTTONMSG 455\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3227\n",
    "BUTTONID 8\nBUTTONMSG 454\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3226\n",
    "ENDGUID\n",
    "GUID 1714\n",
    "BUTTONID 7\nBUTTONMSG 548\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3027\n",
    "ENDGUID\n",
    "GUID 2015\n",
    "BUTTONID 7\nBUTTONMSG 556\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3128\n",
    "BUTTONID 9\nBUTTONMSG 456\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3127\n",
    "ENDGUID\n",
    "GUID 1115\n",
    "BUTTONID 7\nBUTTONMSG 546\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 2831\n",
    "ENDGUID\n",
    "GUID 2616\n",
    "BUTTONID 7\nBUTTONMSG 457\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3327\n",
    "ENDGUID\n",
    "GUID 2013\n",
    "BUTTONID 8\nBUTTONMSG 551\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3122\n",
    "BUTTONID 9\nBUTTONMSG 542\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3121\n",
    "ENDGUID\n",
);

/// `targetdefines.ini` excerpt (values of the shipped data) for [`SPELL_TARGETS`]: without the names `parse_targets`
/// silently aborts (as with [`EVOKE_DEFINES`]).
pub const SPELL_DEFINES: &str = "PROP_PTTOPT 0\nMID_PROP_PTTOPT_NOTHUMAN 131072\nPROP_STAND 2\n\
MID_PROP_STAND_ONWATER 258\nPROP_BUILDING 3\nPROP_GHOST 27\nPROP_BOMBERMAN 36\n\
MID_PROP_BOMBERMAN_CANBLOWUPBUILDING 292\nMID_PROP_BOMBERMAN_CANBLOWUPSHIP 548\nPROP_KILLER 37\n\
MID_PROP_KILLER_KILLNEAR 293\nMID_PROP_KILLER_KILLFAR 549\nPROP_SPELL 38\n\
MID_PROP_SPELL_SPELLEXPLOSION 294\nMID_PROP_SPELL_SPELLSTORM 550\nPROP_SPECIALMOVE 49\n";

/// Entries of the target buttons as in the shipped data, line by line and in file order, without the ineffective
/// `DISABLERBUTTON` (re/spells 2.4, re/special-attacks 2.5): 454 module 36 `CANBLOWUPBUILDING` not mounted -> enemy
/// building visible in the fog; 455 `CANBLOWUPSHIP` -> enemy stand with `STAND_ONWATER`; 456/548 killer near -> (enemy
/// or neutral) with walk, `MM_METHODINDEX 3`, not `NOTHUMAN`; 480/481 spell -> enemy with walk and building,
/// respectively, fog, `MM_METHODINDEX 3`; 484/556 own -> any ground point; 541 only 1113 -> enemy with walk without fog
/// and range; 551 module 49 -> enemy building; 457 killer far, not `SWIMMING` -> enemy, `MM_METHODINDEX 3`, no ghost;
/// 546 like 457 -> (enemy or neutral) without exclusions.
pub const SPELL_TARGETS: &str = concat!(
    entry!(
        "454",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_BOMBERMAN\n\
         MM_MODE MM_CHECKNOTSTATUS\nMM_MODE MM_CHECKHANDLETYPE\nMM_STATUS ISMOUNTED\n\
         MM_MATCHBITS MID_PROP_BOMBERMAN_CANBLOWUPBUILDING\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "455",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_BOMBERMAN\n\
         MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BOMBERMAN_CANBLOWUPSHIP\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_STAND\nMM_MODE MM_CHECKHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_STAND_ONWATER\nMM_END\nMAUSICONID 6\n"
    ),
    entry!(
        "456",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_KILLER\n\
         MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_KILLER_KILLNEAR\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\n\
         MM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_PTTOPT_NOTHUMAN\nMM_MODE MM_ORNEXTMATCH\nMM_NEXT\n\
         MM_MODE MM_HL_NOMANSLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\n\
         MM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_PTTOPT_NOTHUMAN\nMM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "548",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_KILLER\n\
         MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_KILLER_KILLNEAR\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\n\
         MM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_PTTOPT_NOTHUMAN\nMM_MODE MM_ORNEXTMATCH\nMM_NEXT\n\
         MM_MODE MM_HL_NOMANSLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\n\
         MM_PROPID PROP_PTTOPT\nMM_MODE MM_CHECKNOTHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_PTTOPT_NOTHUMAN\nMM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "480",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_SPELL\n\
         MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_SPELL_SPELLEXPLOSION\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_MODE2 MM2_CALLMETHOD\n\
         MM_METHODINDEX 3\nMM_END\nMAUSICONID 7\n"
    ),
    entry!(
        "481",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_SPELL\n\
         MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_SPELL_SPELLSTORM\n",
        "MM_MODE MM_CHECKFOG\nMM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\n\
         MM_PROPID PROP_BUILDING\nMM_END\nMAUSICONID 7\n"
    ),
    entry!("484", "MM_MODE MM_HL_MYLIST\n", "MM_END\nMAUSICONID 7\n"),
    entry!(
        "541",
        "MM_MODE MM_CHECKGUID\nMM_GUID 1113\n",
        "MM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_HL_ENEMYLIST\n\
         MM_PROPID PROP_PTTOPT\nMM_END\nMAUSICONID 7\n"
    ),
    entry!("556", "MM_MODE MM_HL_MYLIST\n", "MM_END\nMAUSICONID 7\n"),
    entry!(
        "551",
        "MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_SPECIALMOVE\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nMAUSICONID 6\n"
    ),
    entry!(
        "457",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_CHECKNOTSTATUS\nMM_MODE MM_ANDALL\n\
         MM_PROPID PROP_KILLER\nMM_MODE MM_CHECKHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_KILLER_KILLFAR\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_MODE2 MM2_CALLMETHOD\n\
         MM_METHODINDEX 3\nMM_MODE MM_ANDNEXTMATCH\nMM_NEXT\nMM_MODE MM_CHECKNOTPROPERTY\n\
         MM_PROPID PROP_GHOST\nMM_END\nMAUSICONID 9\n"
    ),
    entry!(
        "546",
        "MM_MODE MM_CHECKPROPERTY\nMM_MODE MM_CHECKNOTSTATUS\nMM_MODE MM_ANDALL\n\
         MM_PROPID PROP_KILLER\nMM_MODE MM_CHECKHANDLETYPE\n\
         MM_MATCHBITS MID_PROP_KILLER_KILLFAR\nMM_STATUS SWIMMING\n",
        "MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\n\
         MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT\nMM_MODE2 MM2_CALLMETHOD\n\
         MM_METHODINDEX 3\nMM_MODE MM_ORNEXTMATCH\nMM_NEXT\nMM_MODE MM_HL_NOMANSLIST\n\
         MM_MODE MM_CHECKFOG\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\n\
         MM_PROPID PROP_PTTOPT\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 3\nMM_END\n\
         MAUSICONID 9\n"
    ),
);

/// `default.dat` values of the spell world (in addition to [`DEFAULTS`]; `insert` replaces the whole value set of a
/// GUID, hence complete sets for monastery and ship): 14 hit points, 15 mana, 16 speed, 18 sight (sight/3 = range of
/// `MM_METHODINDEX 3`) of the spell types as in the shipped data; the costs of the buttons (config 15 of the button
/// GUID, re/spells 2.5, re/special-attacks 1.1).
pub const SPELL_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (MEDICINE_MAN, &[(14, 800), (15, 100), (16, 145), (18, 52)]),
    (CHIEF, &[(14, 1000), (15, 100), (16, 183), (18, 40)]),
    (MISSIONARY, &[(14, 1000), (15, 200), (16, 145), (18, 40)]),
    (PATRIOT, &[(14, 1500), (15, 100), (16, 227), (18, 32)]),
    (
        SHADOW_WARRIOR,
        &[(14, 1200), (15, 200), (16, 200), (18, 40)],
    ),
    (TRAPPER, &[(14, 900), (15, 150), (16, 200), (18, 52)]),
    (PISTOLERO, &[(14, 1200), (15, 200), (16, 183), (18, 40)]),
    (SHARPSHOOTER, &[(14, 800), (15, 250), (16, 200), (18, 64)]),
    (SMITH, &[(14, 1200), (15, 100), (16, 145), (18, 32)]),
    (BATTLE_WOLF, &[(14, 200), (15, 0), (16, 227), (18, 20)]),
    // Test values: large hits measurable. Rifleman HP 1000 (shipped data 250), ranged 11 as in the evocation world;
    // monastery and ship HP 10,000 (shipped data 3000 and 1400, respectively).
    (RANGED, &[(14, 1000), (18, 40), (20, 11), (21, 28), (34, 4)]),
    (MONASTERY, &[(13, 50), (14, 10_000), (15, 0), (18, 40)]),
    (SHIP, &[(14, 10_000), (16, 128), (18, 40)]),
    (480, &[(15, 80)]),
    (481, &[(15, 80)]),
    (482, &[(15, 40)]),
    (484, &[(15, 60)]),
    (541, &[(15, 50)]),
    (556, &[(15, 25)]),
    (454, &[(15, 70)]),
    (455, &[(15, 40)]),
    (456, &[(15, 60)]),
    (457, &[(15, 50)]),
    (542, &[(15, 75)]),
    (546, &[(15, 40)]),
    (548, &[(15, 50)]),
    (551, &[(15, 60)]),
];

/// Research of the spell world (`REQUIRED_GUID` of the buttons in [`SPELL_TOOLBAR`]).
pub const SPELL_UPGRADE_GUIDS: &[u32] = &[
    2831, 2832, 3022, 3024, 3025, 3026, 3027, 3121, 3122, 3127, 3128, 3226, 3227, 3327,
];

/// Time markers of the spell world (`ability_animation_labels`).
const SPELL_LABELS: &[AnimLabels] = &[
    (MEDICINE_MAN, 2, &[(1, 4340)]),
    (MISSIONARY, 9, &[(1, 4332)]),
    (PATRIOT, 1, &[(1, 5333)]),
    (PATRIOT, 2, &[(1, 5333)]),
    (SHADOW_WARRIOR, 5, &[(1, 1000), (2, 580)]),
    (TRAPPER, 8, &[(1, 750), (2, 300), (22, 633)]),
    (PISTOLERO, 9, &[(1, 3733), (3, 1627)]),
    (SHARPSHOOTER, 1, &[(1, 3066), (3, 1333)]),
    (SMITH, 4, &[(1, 3266), (2, 1283), (22, 1843)]),
    (SMITH, 5, &[(1, 3333)]),
];

/// Rules of the spell world: [`spell_rules_from`] with [`spell_types`].
pub fn spell_rules() -> Rules {
    spell_rules_from(&spell_types())
}

/// [`spell_rules`] with other types: [`rules_from`] and above it the tables of the spells and special attacks as well
/// as the bonus table of the shipped data (`combat::testkit::BONUS`, building reduction -20 %, "melee against ranged"
/// +50 %).
pub fn spell_rules_from(types: &str) -> Rules {
    let mut r = rules_from(types);
    for &(guid, values) in SPELL_DEFAULTS {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    r.upgrades.extend(SPELL_UPGRADE_GUIDS.iter().copied());
    r.bonus = opennml_formats::bonus::parse_bonus_ini(crate::combat::testkit::BONUS);
    r.toolbar = parse_toolbar(&format!("{SPELL_TOOLBAR}{TOOLBAR}"));
    r.targets = parse_targets(
        &format!("{TARGETS}{SPELL_TARGETS}"),
        &parse_target_defines(SPELL_DEFINES),
    )
    .entries;
    for &(guid, anim, labels) in SPELL_LABELS {
        r.set_anim_labels(guid, anim, labels);
    }
    r
}

// --- Test world of the headhunter and of robbing ------------------------------------------------
//
// Like the spell world a world of its own beside `rules()`/`evoke_rules()`/`spell_rules()` (all three stay
// byte-identical, `STEALTH_GOLDEN`/`EVOKE_GOLDEN`/`SPELL_GOLDEN`): `hunter_types()` takes `spell_types()` and appends
// the headhunter types, `hunter_rules_from` builds on `spell_rules_from` (targets and defines unchanged, button 540 has
// no entry). Values from re/headhunter-robbing 1.1, values missing there as in the shipped data; checked by the corpus
// test `tests/corpus_abilities.rs` (`hunter_type_params_match_the_analysis`, `robbing_type_params_match_the_analysis`).
// The headhunters carry stand, walk, dying, swimming and module 34 as in the shipped data (no attack module); the
// robbers carry the walk of the shipped data, the test values of `STAND_AND_DYING`, attack 17 without flags and module
// 48. The headquarters are buildings like the monastery with quarters 23 as in the shipped data (the monastery 1015
// stays without quarters, C-250) and with the exit in front of the building. Elites: SPA 1112, 1113, 1115, SID 2614,
// 2615, 2616 (1113, 1115, 2616 from the spell world; 1112 and 2614 with the values of the colonist). Targets: colonist
// 1100 (corpses), monastery 1015.

/// `Kopfgeldjaeger_SPA` (kind 0): near-kill animation 1, throw 5, HP 4000, sight 32.
pub const HUNTER_SPA: u32 = 4100;
/// `Kopfgeldjaeger_PRI` (kind 1): near-kill animation 4, throw 3, HP 4000, sight 40.
pub const HUNTER_PRI: u32 = 4101;
/// `Kopfgeldjaeger_SID` (kind 2): near-kill animation 1, throw 4, HP 5000, sight 40.
pub const HUNTER_SID: u32 = 4102;
/// `SPA_Hauptquartier`: hiring 4100-4102, quarters 23 (capacity 5).
pub const HQ_SPA: u32 = 1000;
/// `SID_Rathaus` (headquarters of the SID): like [`HQ_SPA`].
pub const HQ_SID: u32 = 2500;
/// `SPA_Gouverneur`: elite of the SPA, here with the values of the colonist.
pub const GOVERNOR: u32 = 1112;
/// `SID_Sheriff`: elite of the SID, here with the values of the colonist.
pub const SID_SHERIFF: u32 = 2614;
/// `SID_Leichenbestatter`: robbing 48 (490001 = 256, `m_RobAnim` 3), attack 17, mana 150.
pub const UNDERTAKER: u32 = 2615;
/// `SID_WilliamSanders`: robbing 48 (490001 = 256, `m_RobAnim` 7), attack 17, mana 100.
pub const SANDERS: u32 = 2702;
/// Button "Rob corpses" (command `0x4C`, `DO`, mana = config 15 = 10); the constant of the command.
pub use crate::command::ability::ROB_BUTTON;
/// Prerequisite of button 540.
pub const ROB_UPGRADE: u32 = 3326;

/// Headhunter as in the shipped data (re/headhunter-robbing 1.1): category `0x2000000`, stand (animation `stand`), walk
/// (`0xC3900`, radii 512/1024), dying (170001 = 0, animation `die`), swimming (160002-160004 = `swim`), module 34
/// (350001 = 0, near kill 350002 = `near`, throw 350003 = `far`).
fn hunter(
    guid: u32,
    name: &str,
    stand: i32,
    die: i32,
    swim: [i32; 3],
    near: i32,
    far: i32,
) -> String {
    let [s2, s3, s4] = swim;
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 33554432\n\
         PROPID 2\nVALUEID 30001 1 1 {stand}\nVALUEID 30002 1 1 0\nVALUEID 30003 6 1 0\n{}\
         PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 {die}\n\
         PROPID 15\nVALUEID 160001 6 1 0\nVALUEID 160002 1 1 {s2}\nVALUEID 160003 1 1 {s3}\n\
         VALUEID 160004 1 1 {s4}\n\
         PROPID 34\nVALUEID 350001 6 1 0\nVALUEID 350002 1 1 {near}\nVALUEID 350003 1 1 {far}\n",
        walk(0, 801_024, 512, 1024, 0)
    )
}

/// Headquarters: building like the monastery (category `0x2000000` of the shipped data) with quarters 23 (240001 = 0)
/// as in the shipped data.
fn headquarters(guid: u32, name: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 33554432\n\
         PROPID 3\nVALUEID 40003 6 1 0\nPROPID 23\nVALUEID 240001 6 1 0\n"
    )
}

/// Robber: category `0x2800000`, walk of the shipped data, attack 17 without flags and module 48 with 490001 = 256
/// (`CANROBDEADUNITS`) and `m_RobAnim` = `anim` (490002).
fn robber(guid: u32, name: &str, anim: i32) -> String {
    unit(
        guid,
        name,
        0x280_0000,
        &walk(1, 4_458_752, 512, 1024, 0),
        &format!("{ATTACK}PROPID 48\nVALUEID 490001 6 1 256\nVALUEID 490002 1 1 {anim}\n"),
    )
}

/// The headhunter types (to be appended behind [`spell_types`]).
fn hunter_only_types() -> String {
    let colonist_walk = walk(1, 264_448, 512, 1024, 0);
    [
        hunter(HUNTER_SPA, "Kopfgeldjaeger_SPA", 3, 4, [2, 2, 2], 1, 5),
        hunter(HUNTER_PRI, "Kopfgeldjaeger_PRI", 2, 6, [1, 5, 5], 4, 3),
        hunter(HUNTER_SID, "Kopfgeldjaeger_SID", 3, 6, [2, 5, 5], 1, 4),
        headquarters(HQ_SPA, "SPA_Hauptquartier"),
        headquarters(HQ_SID, "SID_Rathaus"),
        unit(GOVERNOR, "SPA_Gouverneur", 0x280_0000, &colonist_walk, ""),
        unit(SID_SHERIFF, "SID_Sheriff", 0x280_0000, &colonist_walk, ""),
        robber(UNDERTAKER, "SID_Leichenbestatter", 3),
        robber(SANDERS, "SID_WilliamSanders", 7),
    ]
    .concat()
}

/// Synthetic `property.ini` of the headhunter world: [`spell_types`] followed by the headhunter types.
pub fn hunter_types() -> String {
    format!("{}{}", spell_types(), hunter_only_types())
}

/// Command bars of the headhunter world like `toolbar.ini`: hiring 4100-4102 (slots 2-4, `SELECTHEADHUNTER_MSG`,
/// without prerequisite) in both HQs, button 540 (slot 7, `GROUP_AND_MSG`, `DO_MSG`, `REQUIRED_GUID 3326`) on 2615 and
/// 2702. To be parsed before [`SPELL_TOOLBAR`] and [`TOOLBAR`] (no GUID occurs twice).
pub const HUNTER_TOOLBAR: &str = concat!(
    "GUID 1000\n",
    "BUTTONID 2\nBUTTONMSG 4100\nSELECTHEADHUNTER_MSG\n",
    "BUTTONID 3\nBUTTONMSG 4101\nSELECTHEADHUNTER_MSG\n",
    "BUTTONID 4\nBUTTONMSG 4102\nSELECTHEADHUNTER_MSG\n",
    "ENDGUID\n",
    "GUID 2500\n",
    "BUTTONID 2\nBUTTONMSG 4100\nSELECTHEADHUNTER_MSG\n",
    "BUTTONID 3\nBUTTONMSG 4101\nSELECTHEADHUNTER_MSG\n",
    "BUTTONID 4\nBUTTONMSG 4102\nSELECTHEADHUNTER_MSG\n",
    "ENDGUID\n",
    "GUID 2615\n",
    "BUTTONID 7\nBUTTONMSG 540\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3326\n",
    "ENDGUID\n",
    "GUID 2702\n",
    "BUTTONID 7\nBUTTONMSG 540\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3326\n",
    "ENDGUID\n",
);

/// `default.dat` values of the headhunter world (in addition to [`DEFAULTS`] and [`SPELL_DEFAULTS`]) as in the shipped
/// data: 12 price of the headhunters (slot 8 reads `defaults`), 13 build time, 14 hit points, 15 mana, 16 speed ("Very
/// fast" 254), 18 sight, 23 capacity; the elites 1112/2614 like the colonist; cost of button 540 (config 15).
pub const HUNTER_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (HUNTER_SPA, &[(12, 300), (14, 4000), (16, 254), (18, 32)]),
    (HUNTER_PRI, &[(12, 300), (14, 4000), (16, 254), (18, 40)]),
    (HUNTER_SID, &[(12, 300), (14, 5000), (16, 254), (18, 40)]),
    (HQ_SPA, &[(13, 90), (14, 4000), (15, 0), (18, 40), (23, 5)]),
    (HQ_SID, &[(13, 63), (14, 3200), (15, 0), (18, 40), (23, 5)]),
    (GOVERNOR, &[(14, 160), (16, 183), (18, 32)]),
    (SID_SHERIFF, &[(14, 160), (16, 183), (18, 32)]),
    (UNDERTAKER, &[(14, 1200), (15, 150), (16, 183), (18, 32)]),
    (SANDERS, &[(14, 900), (15, 100), (16, 183), (18, 32)]),
    (540, &[(15, 10)]),
];

/// Research of the headhunter world (`REQUIRED_GUID` of button 540).
pub const HUNTER_UPGRADE_GUIDS: &[u32] = &[ROB_UPGRADE];

/// Time markers of the headhunter world (`ability_animation_labels`): near kill 350002 and throw 350003 of the
/// headhunters, robbing 490002 of 2615 and 2702.
const HUNTER_LABELS: &[AnimLabels] = &[
    (HUNTER_SPA, 1, &[(2, 720), (22, 1300), (1, 1800)]),
    (HUNTER_SPA, 5, &[(2, 760), (1, 1720)]),
    (HUNTER_PRI, 4, &[(3, 863), (1, 1400)]),
    (HUNTER_PRI, 3, &[(2, 760), (1, 1720)]),
    (HUNTER_SID, 1, &[(2, 720), (22, 1300), (1, 1800)]),
    (HUNTER_SID, 4, &[(2, 760), (1, 1720)]),
    (UNDERTAKER, 3, &[(1, 2499)]),
    (SANDERS, 7, &[(1, 2499)]),
];

/// Hulls of the headquarters: 3 x 3 tiles like the monastery, exit 300 in front of the center (outside the hull;
/// pattern `building/destroy.rs`).
const HUNTER_SHAPES: &[(u32, [f32; 3], [f32; 3])] = &[
    (HQ_SPA, [0.0; 3], [100.0, 50.0, 100.0]),
    (HQ_SID, [0.0; 3], [100.0, 50.0, 100.0]),
];

/// Rules of the headhunter world: [`hunter_rules_from`] with [`hunter_types`].
pub fn hunter_rules() -> Rules {
    hunter_rules_from(&hunter_types())
}

/// [`hunter_rules`] with other types: [`spell_rules_from`] and above it the tables of the headhunter and of robbing.
pub fn hunter_rules_from(types: &str) -> Rules {
    let mut r = spell_rules_from(types);
    for &(guid, values) in HUNTER_DEFAULTS {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    r.upgrades.extend(HUNTER_UPGRADE_GUIDS.iter().copied());
    r.toolbar = parse_toolbar(&format!("{HUNTER_TOOLBAR}{SPELL_TOOLBAR}{TOOLBAR}"));
    for &(guid, center, half) in HUNTER_SHAPES {
        let mut shape = BuildingShape {
            center,
            half,
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        };
        shape
            .helper_nodes
            .insert("ausgang".into(), [0.0, 0.0, 300.0]);
        r.building_shapes.insert(guid, shape);
        r.model_boxes.insert(guid, ModelBox { center, half });
    }
    for &(guid, anim, labels) in HUNTER_LABELS {
        r.set_anim_labels(guid, anim, labels);
    }
    r
}

/// [`rules`] with a lake like `riding::testkit::lake_rules`: water level 0.0, all grid points with `20 <= i, j <= 40`
/// at `floor`, the remaining land 16.0 above the water; tile heights and tile classes recomputed.
pub fn lake_rules(floor: f32) -> Rules {
    use crate::path::terrain::classify_tiles;
    use crate::rules::TileTerrain;
    let mut r = rules();
    r.map.water_level = 0.0;
    r.terrain.params.water_level = 0.0;
    let vx = r.terrain.verts_x;
    for (n, v) in r.terrain.vertices.iter_mut().enumerate() {
        let (i, j) = (n % vx, n / vx);
        v.position.y = if (20..=40).contains(&i) && (20..=40).contains(&j) {
            floor
        } else {
            16.0
        };
    }
    r.tile_terrain = TileTerrain::build(&r.terrain, &r.map);
    r.tile_classes = classify_tiles(&r.terrain, |_| 0);
    r
}

/// Empty, fully revealed 64 x 64 world; slots 0 and 1 are human and enemies of each other (diplomacy default), 1000
/// gold, wood and food each.
pub fn state(rules: &Rules) -> SimState {
    let mut s = state_fogged(rules);
    crate::fog::reveal_map(&mut s, -1);
    s
}

/// [`state`] without revealing: the map lies in fog (fog tests, e.g. the target filter 440 with `MM_CHECKFOG`).
pub fn state_fogged(rules: &Rules) -> SimState {
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
    s
}

/// [`state`] of the headhunter world: slot 0 nation 2 (SPA), slot 1 nation 3 (SID), both play (`result = 0`); color =
/// slot, slots 2-7 stay "not participating" (`result = 3`).
pub fn hunter_state(rules: &Rules) -> SimState {
    let mut s = state(rules);
    for (slot, nation) in [(0, 2), (1, 3)] {
        let p = &mut s.players[slot];
        p.nation = nation;
        p.result = 0;
    }
    s
}

/// Unreachable terrain: blocks all cells (= tiles) with `inner < max(|dx|, |dz|) <= outer` around `center` in all base
/// layers (`0x1F`), then dirty rectangle and regions anew (pattern `boarding::testkit::grid_state`). The inner area is
/// afterwards its own region ("island") in every walk layer. Call before creating the objects; the ring must lie
/// entirely on the map.
pub fn wall_ring(state: &mut SimState, center: (i32, i32), inner: i32, outer: i32) {
    for dz in -outer..=outer {
        for dx in -outer..=outer {
            if dx.abs().max(dz.abs()) > inner {
                state
                    .path
                    .mark_cell(0x1F, center.0 + dx, center.1 + dz, true);
            }
        }
    }
    state.path.flush_dirty_rect();
    state.path.build_regions(u32::MAX);
}

/// Command `0x2C` (headhunter of kind `art` from the headquarters `hq` on `guid` of slot `target`).
pub fn hire_command(art: i32, target: i32, guid: i32, hq: ObjectId) -> crate::command::CommandKind {
    crate::command::CommandKind::HireHeadhunter {
        art,
        target_player: target,
        target_guid: guid,
        building: hq,
    }
}

/// Hiring: run [`hire_command`] directly, then one object tick so that `0x122` reaches the headhunter; between two
/// calls the price table (re/headhunter-robbing 9.1) therefore matches. Return value: the events of the command and of
/// the two steps.
pub fn hire(
    sim: &mut Sim,
    player: u8,
    art: i32,
    target: i32,
    guid: i32,
    hq: ObjectId,
) -> Vec<SimEvent> {
    let kind = hire_command(art, target, guid, hq);
    crate::command::execute(sim, &crate::command::Command { player, kind }, true);
    run_ticks_collect(sim, 1)
}

/// Unlock upgrade `guid` for the slot like `Player_ApplyUpgrade` (re/upgrades 3.2), but without percentages and without
/// variant switch of existing objects: object counter +1 (satisfies `REQUIRED_GUID`) and the GUID appended to the list
/// of applied upgrades (variant terms). Called before [`spawn`], the new object has the variants 9/10; existing objects
/// do not switch until `variants::update_variant_mask(sim, id, u32::MAX, 0)` (or right away with
/// `upgrades::apply_upgrade`, which switches the objects of the slot via [`UPGRADE_TYPES`]).
pub fn unlock(state: &mut SimState, slot: u8, guid: u32) {
    state.add_guid_count(slot, guid, 1);
    state.applied_upgrades[usize::from(slot)].push(guid);
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

/// `n` object ticks without commands (two `tick::step` each).
pub fn run_ticks(sim: &mut Sim, n: usize) {
    for _ in 0..2 * n {
        crate::tick::step(sim.state, sim.rules, &[]);
    }
}

/// Like [`run_ticks`], but with the events of all `2·n` steps in order (the first step also returns the events
/// accumulated before).
pub fn run_ticks_collect(sim: &mut Sim, n: usize) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..2 * n {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

/// Corpse: a colonist 1100 of slot `slot` on `tile` dies (`0x110`); after one object tick it is a corpse on slot 8 with
/// a new ID (change of owner in phase 0, re/combat-death 2.5). Returns the new ID. Robbing gold 25; robbable not until
/// the time marker of the dying animation ([`run_until_robbable`]), decay after 30 s.
pub fn corpse(sim: &mut Sim, slot: i32, tile: (i32, i32)) -> ObjectId {
    let before: Vec<ObjectId> = sim
        .state
        .objects()
        .filter(|o| o.id.slot() == 8)
        .map(|o| o.id)
        .collect();
    let victim = spawn(sim, COLONIST, slot, tile);
    crate::messaging::send(sim, victim, &crate::message::Message::new(0x110, &[]));
    run_ticks(sim, 1);
    sim.state
        .objects()
        .find(|o| o.guid == COLONIST && o.id.slot() == 8 && !before.contains(&o.id))
        .map(|o| o.id)
        .expect("corpse on slot 8")
}

/// Object ticks until the corpse `corpse` is robbable (`dying::CANROBBED` in the module flags, with `0x710` at time
/// marker 1 of the dying animation); at most 50. Returns the number of ticks.
pub fn run_until_robbable(sim: &mut Sim, corpse: ObjectId) -> usize {
    let robbable = |sim: &Sim| {
        sim.module(corpse, crate::module::dying::INDEX)
            .is_some_and(|m| m.base.flags & crate::module::dying::CANROBBED != 0)
    };
    for n in 0..=50 {
        if robbable(sim) {
            return n;
        }
        run_ticks(sim, 1);
    }
    panic!("corpse {corpse:?} not robbable after 50 ticks");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Module indices camouflage and dig in.
    const MASKING: usize = 25;
    const DIGGING: usize = 28;

    #[test]
    fn variants_follow_the_unlocked_upgrades() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let archer = spawn(&mut sim, ARCHER, 0, (10, 10));
        let assassin = spawn(&mut sim, ASSASSIN, 0, (12, 10));
        unlock(sim.state, 0, MASK_UPGRADE);
        unlock(sim.state, 0, DIG_IN_UPGRADE);
        let archer_2924 = spawn(&mut sim, ARCHER, 0, (14, 10));
        let assassin_2922 = spawn(&mut sim, ASSASSIN, 0, (16, 10));
        let has = |sim: &Sim, id, index| sim.object(id).unwrap().has_module(index, false);
        assert!(!has(&sim, archer, MASKING));
        assert!(!has(&sim, assassin, DIGGING));
        assert!(has(&sim, archer_2924, MASKING));
        assert!(has(&sim, assassin_2922, DIGGING));
        // Module 25 of the archer is present even without 2924 (switched off).
        assert!(sim.object(archer).unwrap().has_module(MASKING, true));
    }

    #[test]
    fn every_type_spawns_with_its_hitpoints_and_ticks() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let types = [
            (ARCHER, 155),
            (RANGER, 1800),
            (ALARM_POST, 1600),
            (SCOUT, 155),
            (ASSASSIN, 200),
            (ZOMBIE, 300),
            (COLONIST, 160),
            (SABRE, 190),
            (PILGRIM, 1000),
            (MEDIC, 200),
            (FARMER, 100),
            (MONASTERY, 3000),
            (SHIPYARD, 3000),
            (SHIP, 1400),
        ];
        let ids: Vec<ObjectId> = types
            .iter()
            .enumerate()
            .map(|(k, &(guid, _))| spawn(&mut sim, guid, (k % 2) as i32, (6 + 4 * k as i32, 20)))
            .collect();
        run_ticks(&mut sim, 5);
        for (&(guid, hp), &id) in types.iter().zip(&ids) {
            let o = sim.object(id).unwrap_or_else(|| panic!("{guid} alive"));
            let pct = sim.type_upgrade_percent(o.owner(), guid, 14);
            assert_eq!(o.attributes.current_value(14, pct), hp, "{guid}");
        }
        // Buildings: alarm post, monastery and shipyard.
        for k in [2, 11, 12] {
            assert!(sim.object(ids[k]).unwrap().has_module(3, false));
        }
        // Sheriff: radius-opponent upgrade as in the shipped data, extra energy -100 %.
        assert_eq!(rules.temp_upgrades[&SHERIFF].flags, 0x2D9);
        assert_eq!(rules.config_value(0, SHERIFF, 115), -100);
        assert_eq!(rules.anim_label_time_at(ASSASSIN, 1, 2), 640);
    }

    #[test]
    fn every_evoke_type_spawns_with_its_hitpoints_and_ticks() {
        let rules = evoke_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let types = [
            (GHOST_EVOKER, 900),
            (GHOST, 250),
            (ANIMAL_EVOKER, 1200),
            (WOLF, 80),
            (BEAR, 120),
            (CAT, 80),
            (WARRIOR, 160),
            (RANGED, 250),
            (MISSIONARY, 1000),
            (PILGRIM, 1000),
        ];
        // Ghost and wolf on slot 0 (category `0x2000000`, not neutral).
        let slot = |guid| i32::from(!matches!(guid, GHOST | WOLF) && guid % 2 == 1);
        let ids: Vec<ObjectId> = types
            .iter()
            .enumerate()
            .map(|(k, &(guid, _))| spawn(&mut sim, guid, slot(guid), (6 + 4 * k as i32, 20)))
            .collect();
        run_ticks(&mut sim, 1);
        for (&(guid, hp), &id) in types.iter().zip(&ids) {
            let o = sim.object(id).unwrap_or_else(|| panic!("{guid} alive"));
            assert_eq!(o.owner(), slot(guid) as u8, "{guid}: slot");
            let pct = sim.type_upgrade_percent(o.owner(), guid, 14);
            assert_eq!(o.attributes.current_value(14, pct), hp, "{guid}");
        }
        // Modules of the evokers, banishing, ghosts and animals.
        let has = |k: usize, index| sim.object(ids[k]).unwrap().has_module(index, true);
        assert!(has(0, 26) && has(0, 31) && has(0, 0x26));
        assert!(has(1, 27) && has(1, 19));
        assert!(has(2, 32));
        for (k, &(guid, _)) in types.iter().enumerate().take(6).skip(3) {
            assert!(has(k, 20) && has(k, 44) && has(k, 18), "{guid}");
        }
        assert!(has(3, 21) && has(4, 21) && !has(5, 21));
        assert!(has(8, 31) && has(8, 13));
        assert!(has(9, 29) && has(9, 31));
        // Time markers (1113 anim 12 is the test banish animation in the saddle).
        for (guid, anim, ms) in [
            (GHOST_EVOKER, 0, 3999),
            (GHOST_EVOKER, 10, 5332),
            (ANIMAL_EVOKER, 0, 8000),
            (MISSIONARY, 6, 9332),
            (MISSIONARY, 12, 2000),
            (PILGRIM, 6, 4666),
            (PILGRIM, 7, 3999),
            (GHOST, 0, 1666),
        ] {
            assert_eq!(rules.anim_label_time_at(guid, anim, 1), ms, "{guid}/{anim}");
        }
    }

    #[test]
    fn evoke_targets_parse_completely() {
        let text = format!("{TARGETS}{EVOKE_TARGETS}");
        let file = parse_targets(&text, &parse_target_defines(EVOKE_DEFINES));
        assert_eq!(file.aborted, None);
        // Without the defines the loader aborts silently at `MM_PROPERTY PROP_EVOKEGHOST`.
        assert!(
            parse_targets(&text, &parse_target_defines(""))
                .aborted
                .is_some()
        );
        let r = evoke_rules();
        assert_eq!(r.targets, file.entries);
        let e = r.target_entry(436).expect("entry 436");
        assert_eq!(
            (e.my.mode, e.my.prop_id, e.my.status, e.target.mode),
            (0x2014, 26, crate::object::status::SWIMMING, 0x800_0000)
        );
        let e = r.target_entry(440).expect("entry 440");
        assert_eq!(e.my.mode, 0x800_0000);
        // ANDALL | CHECKPROPERTY | CHECKFOG | ENEMYLIST with `PROP_GHOST`.
        assert_eq!(e.target.mode, 0x10 | 0x4 | 0x200_0000 | 0x400);
        assert_eq!(e.target.prop_id, 27);
        // 441-443 as in the shipped data: CHECKPROPERTY | ANDALL | CHECKNOTSTATUS with `PROP_EVOKEANIMAL` and
        // `SWIMMING`, TARGET `RETURNFALSE`.
        for msg in [441, 442, 443] {
            let e = r.target_entry(msg).expect("entry");
            assert_eq!(
                (e.my.mode, e.my.prop_id, e.my.status, e.target.mode),
                (0x2014, 32, crate::object::status::SWIMMING, 0x800_0000),
                "{msg}"
            );
        }
        // The entries of the ability buttons of [`rules`] stay.
        assert!(r.target_entry(434).is_some() && r.target_entry(564).is_some());
        // Command bar: the complete 2014 block comes before `TOOLBAR`.
        let msgs =
            |menu: i32| -> Vec<i32> { r.toolbar.menus[&menu].iter().map(|b| b.msg).collect() };
        assert_eq!(msgs(2014), [554, 564, 555, 553]);
        // Slots: 554/564/555 on 8, only 553 on 9; `rules()` keeps 555 on 9.
        let slots = |r: &Rules| -> Vec<(i32, i32)> {
            r.toolbar.menus[&2014]
                .iter()
                .map(|b| (b.msg, b.id))
                .collect()
        };
        assert_eq!(slots(&r), [(554, 8), (564, 8), (555, 8), (553, 9)]);
        assert_eq!(slots(&rules()), [(554, 8), (564, 8), (555, 9)]);
        assert_eq!(msgs(GHOST_EVOKER as i32), [436, 547]);
        assert_eq!(msgs(ANIMAL_EVOKER as i32), [443, 441, 442]);
        assert_eq!(msgs(MISSIONARY as i32), [439]);
        assert_eq!(msgs(1402), [434, 435]);
        let b553 = &r.toolbar.entries[&553];
        assert_eq!((b553.flags, b553.required[0]), (0x400_0001, 3124));
        assert_eq!(r.toolbar.entries[&547].flags, 0x400_0000);
        assert_eq!(r.toolbar.entries[&436].required[0], 2930);
        assert!(r.types[&PILGRIM].has_module(29) && r.types[&PILGRIM].has_module(31));
        // Counter-check: `rules()` knows neither 553 nor module 31 on the pilgrim.
        let base = rules();
        assert!(!base.toolbar.menus[&2014].iter().any(|b| b.msg == 553));
        assert!(!base.types[&PILGRIM].has_module(31));
        assert!(!base.types.contains_key(&GHOST_EVOKER));
    }

    #[test]
    fn evoke_upgrades_carry_their_values() {
        let r = evoke_rules();
        for (guid, values) in [
            (WOLF_AURA, &[(119, 60), (120, 60)][..]),
            (BEAR_AURA, &[(114, 60)][..]),
            (CAT_AURA, &[(116, 60)][..]),
        ] {
            assert_eq!(r.temp_upgrade(guid).map(|t| t.flags), Some(0x49), "{guid}");
            for &(cfg, v) in values {
                assert_eq!(r.config_value(0, guid, cfg), v, "{guid}: {cfg}");
            }
            for ty in [ARCHER, WARRIOR, RANGED, GHOST_EVOKER, ANIMAL_EVOKER] {
                assert!(r.types_with_upgrade(guid).contains(&ty), "{guid}: {ty}");
            }
        }
        assert_eq!(r.temp_upgrade(SHERIFF).map(|t| t.flags), Some(0x2D9));
        // Button costs (config 15 of the button GUID).
        for (button, cost) in [
            (436, 25),
            (547, 25),
            (439, 25),
            (553, 200),
            (441, 25),
            (442, 50),
            (443, 30),
        ] {
            assert_eq!(r.config_value(0, button, 15), cost, "{button}");
        }
        assert_eq!(r.config_value(0, 555, 15), 50);
        for guid in [2926, 2927, 2928, 2930, 2931, 2827, 3124] {
            assert!(r.upgrades.contains(&guid), "{guid}");
        }
        // The missionary can banish (mana 200).
        assert_eq!(r.config_value(0, MISSIONARY, 15), 200);
        // Speed 145 as in the shipped data.
        for guid in [GHOST_EVOKER, GHOST, ANIMAL_EVOKER, MISSIONARY] {
            assert_eq!(r.config_value(0, guid, 16), 145, "{guid}");
        }
        // Without revealing the map is in fog.
        let fogged = state_fogged(&r);
        let seen = state(&r);
        assert_ne!(fogged.fog, seen.fog);
    }

    #[test]
    fn every_spell_type_spawns_with_its_hitpoints_and_ticks() {
        use opennml_formats::bonus::{ART_MELEE_ARTILLERY, ART_UNIT_BUILDING};
        let rules = spell_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Hit points; monastery, ship and rifleman with the test values.
        let types = [
            (MEDICINE_MAN, 800),
            (CHIEF, 1000),
            (MISSIONARY, 1000),
            (PATRIOT, 1500),
            (SHADOW_WARRIOR, 1200),
            (TRAPPER, 900),
            (PISTOLERO, 1200),
            (SHARPSHOOTER, 800),
            (SMITH, 1200),
            (BATTLE_WOLF, 200),
            (RANGED, 1000),
            (MONASTERY, 10_000),
            (SHIP, 10_000),
            (COLONIST, 160),
        ];
        let slot = |k: usize| (k % 2) as i32;
        let ids: Vec<ObjectId> = types
            .iter()
            .enumerate()
            .map(|(k, &(guid, _))| spawn(&mut sim, guid, slot(k), (4 + 4 * k as i32, 20)))
            .collect();
        // A damage before the tick: `run_ticks_collect` returns the events of the steps, here the attack report of the
        // colonist.
        let colonist = ids[13];
        let hit = crate::combat::damage::explicit_damage_message(-10, ObjectId::NONE, 0);
        crate::messaging::send(&mut sim, colonist, &hit);
        let events = run_ticks_collect(&mut sim, 1);
        assert!(events.iter().any(|e| matches!(
            e,
            crate::state::SimEvent::UnderAttack { object, .. } if *object == colonist
        )));
        assert_eq!(sim.state.steps, 2, "one object tick = two steps");
        for (k, (&(guid, hp), &id)) in types.iter().zip(&ids).enumerate() {
            let o = sim.object(id).unwrap_or_else(|| panic!("{guid} alive"));
            assert_eq!(o.owner(), slot(k) as u8, "{guid}: slot");
            let pct = sim.type_upgrade_percent(o.owner(), guid, 14);
            let expected = if guid == COLONIST { hp - 10 } else { hp };
            assert_eq!(o.attributes.current_value(14, pct), expected, "{guid}");
        }
        // Modules (re/spells 1.1, re/special-attacks 1.1).
        let has = |k: usize, index| sim.object(ids[k]).unwrap().has_module(index, true);
        assert!(has(0, 0x26) && has(2, 0x26) && !has(1, 0x26));
        assert!(has(3, 36) && has(3, 13) && !has(3, 18));
        for k in [4, 5] {
            assert!(has(k, 37) && has(k, 18) && !has(k, 19), "{}", types[k].0);
        }
        assert!(has(6, 37) && has(6, 19) && !has(6, 18));
        assert!(has(7, 37) && has(7, 18) && has(7, 19));
        assert!(has(8, 49) && has(8, 18));
        assert!(has(9, 20) && has(9, 21) && has(9, 18));
        assert!(has(10, 19) && !has(10, 18));
        // Mana (config 15).
        for (guid, mana) in [
            (MEDICINE_MAN, 100),
            (CHIEF, 100),
            (MISSIONARY, 200),
            (PATRIOT, 100),
            (SHADOW_WARRIOR, 200),
            (TRAPPER, 150),
            (PISTOLERO, 200),
            (SHARPSHOOTER, 250),
            (SMITH, 100),
        ] {
            assert_eq!(rules.config_value(0, guid, 15), mana, "{guid}: mana");
        }
        // Module 2 of the ship has `STAND_ONWATER` (`0x100`, `MID_PROP_STAND_ONWATER 258`); not in `rules()`.
        let ship = ids[12];
        assert!(crate::filter::matches_class(&mut sim, ship, 258));
        // Bonus table of the shipped data.
        assert_eq!(rules.bonus.attack_bonus(ART_UNIT_BUILDING), -20);
        assert_eq!(rules.bonus.attack_bonus(ART_MELEE_ARTILLERY), 50);
        // Costs of the buttons (config 15 of the button GUID).
        for &(button, values) in SPELL_DEFAULTS.iter().filter(|(g, _)| *g < 1000) {
            assert_eq!(
                rules.config_value(0, button, 15),
                values[0].1,
                "button {button}"
            );
        }
        assert_eq!(rules.config_value(0, 480, 15), 80);
        assert_eq!(rules.config_value(0, 556, 15), 25);
        for guid in SPELL_UPGRADE_GUIDS {
            assert!(rules.upgrades.contains(guid), "{guid}");
        }
        // Time markers.
        for (guid, anim, label, ms) in [
            (MEDICINE_MAN, 2, 1, 4340),
            (MISSIONARY, 9, 1, 4332),
            (PATRIOT, 1, 1, 5333),
            (PATRIOT, 2, 1, 5333),
            (SHADOW_WARRIOR, 5, 2, 580),
            (SHADOW_WARRIOR, 5, 1, 1000),
            (TRAPPER, 8, 2, 300),
            (TRAPPER, 8, 22, 633),
            (TRAPPER, 8, 1, 750),
            (PISTOLERO, 9, 3, 1627),
            (PISTOLERO, 9, 1, 3733),
            (SHARPSHOOTER, 1, 3, 1333),
            (SHARPSHOOTER, 1, 1, 3066),
            (SMITH, 4, 2, 1283),
            (SMITH, 4, 22, 1843),
            (SMITH, 4, 1, 3266),
            (SMITH, 5, 1, 3333),
            (SMITH, 5, 2, 0),
        ] {
            assert_eq!(
                rules.anim_label_time_at(guid, anim, label),
                ms,
                "{guid}/{anim} marker {label}"
            );
        }
        // Counter-check: `rules()` and `evoke_rules()` stay unchanged.
        let base = super::rules();
        assert!(!base.types.contains_key(&MEDICINE_MAN));
        assert_eq!(base.bonus.attack_bonus(ART_UNIT_BUILDING), 0);
        assert_eq!(base.config_value(0, MONASTERY, 14), 3000);
        assert_eq!(base.config_value(0, SHIP, 14), 1400);
        assert!(!base.types[&SHIP].has_module(36));
        assert_eq!(evoke_rules().config_value(0, RANGED, 14), 250);
        let mut base_state = super::state(&base);
        let mut base_sim = Sim::new(&mut base_state, &base);
        let base_ship = spawn(&mut base_sim, SHIP, 1, (20, 20));
        assert!(!crate::filter::matches_class(&mut base_sim, base_ship, 258));
    }

    #[test]
    fn every_hunter_type_spawns_with_its_hitpoints_and_ticks() {
        let rules = hunter_rules();
        let mut state = hunter_state(&rules);
        assert_eq!((state.players[0].nation, state.players[1].nation), (2, 3));
        assert_eq!((state.players[0].result, state.players[1].result), (0, 0));
        let mut sim = Sim::new(&mut state, &rules);
        // Hit points (re/headhunter-robbing 1.1; elites without values of their own like the colonist).
        let types = [
            (HUNTER_SPA, 8, 4000),
            (HUNTER_PRI, 8, 4000),
            (HUNTER_SID, 8, 5000),
            (HQ_SPA, 0, 4000),
            (HQ_SID, 1, 3200),
            (GOVERNOR, 0, 160),
            (MISSIONARY, 0, 1000),
            (PISTOLERO, 0, 1200),
            (SID_SHERIFF, 1, 160),
            (UNDERTAKER, 1, 1200),
            (SHARPSHOOTER, 1, 800),
            (SANDERS, 1, 900),
            (COLONIST, 0, 160),
            // Monastery with the test value of the spell world (shipped data 3000).
            (MONASTERY, 1, 10_000),
        ];
        let ids: Vec<ObjectId> = types
            .iter()
            .enumerate()
            .map(|(k, &(guid, slot, _))| spawn(&mut sim, guid, slot, (4 + 4 * k as i32, 20)))
            .collect();
        run_ticks(&mut sim, 1);
        for (&(guid, slot, hp), &id) in types.iter().zip(&ids) {
            let o = sim.object(id).unwrap_or_else(|| panic!("{guid} alive"));
            assert_eq!(o.owner(), slot as u8, "{guid}: slot");
            let pct = sim.type_upgrade_percent(o.owner(), guid, 14);
            assert_eq!(o.attributes.current_value(14, pct), hp, "{guid}");
        }
        let has = |k: usize, index| sim.object(ids[k]).unwrap().has_module(index, true);
        // Headhunters: stand, walk, dying, swimming, module 34; no attack module (17/18/19).
        for (k, &(guid, _, _)) in types.iter().enumerate().take(3) {
            for index in [0, 2, 15, 16, 34] {
                assert!(has(k, index), "{guid}: module {index}");
            }
            for index in [17, 18, 19] {
                assert!(!has(k, index), "{guid}: module {index}");
            }
        }
        // Headquarters: building and quarters; the monastery without quarters (C-250).
        for k in [3, 4] {
            assert!(has(k, 3) && has(k, 23), "{}", types[k].0);
        }
        assert!(!has(13, 23));
        // Robbers: module 48 and attack 17.
        for k in [9, 11] {
            assert!(has(k, 48) && has(k, 17) && !has(k, 15), "{}", types[k].0);
        }
        // Headhunter values 350002/350003 and robbing animation 490002.
        let int = |guid: u32, index: u32, id: u32| {
            rules.types[&guid]
                .module(index)
                .and_then(|m| m.value_int(id))
        };
        for (guid, near, far) in [(HUNTER_SPA, 1, 5), (HUNTER_PRI, 4, 3), (HUNTER_SID, 1, 4)] {
            assert_eq!(
                (int(guid, 34, 350_002), int(guid, 34, 350_003)),
                (Some(near), Some(far)),
                "{guid}"
            );
        }
        for (guid, anim, mana) in [(UNDERTAKER, 3, 150), (SANDERS, 7, 100)] {
            assert_eq!(int(guid, 48, 490_001), Some(256), "{guid}");
            assert_eq!(int(guid, 48, 490_002), Some(anim), "{guid}");
            assert_eq!(rules.config_value(1, guid, 15), mana, "{guid}: mana");
            assert_eq!(rules.config_value(1, guid, 18), 32, "{guid}: sight");
        }
        for (guid, sight) in [(HUNTER_SPA, 32), (HUNTER_PRI, 40), (HUNTER_SID, 40)] {
            assert_eq!(rules.config_value(8, guid, 18), sight, "{guid}: sight");
        }
        assert_eq!(rules.config_value(0, HQ_SPA, 23), 5);
        // Command bar: hiring in the HQ (kind `SELECTHEADHUNTER`), button 540 (`DO`, 3326).
        let menu = |guid: u32| -> Vec<(i32, i32, u32, i32)> {
            rules.toolbar.menus[&(guid as i32)]
                .iter()
                .map(|b| (b.msg, b.id, b.flags, b.required[0]))
                .collect()
        };
        let h = opennml_formats::gifcon::button_kind::SELECTHEADHUNTER;
        let d = opennml_formats::gifcon::button_kind::DO | 1;
        for hq in [HQ_SPA, HQ_SID] {
            assert_eq!(
                menu(hq),
                [(4100, 2, h, 0), (4101, 3, h, 0), (4102, 4, h, 0)],
                "{hq}"
            );
        }
        for guid in [UNDERTAKER, SANDERS] {
            assert_eq!(
                menu(guid),
                [(ROB_BUTTON, 7, d, ROB_UPGRADE as i32)],
                "{guid}"
            );
        }
        assert_eq!(rules.config_value(0, ROB_BUTTON as u32, 15), 10);
        assert!(
            rules.target_entry(ROB_BUTTON).is_none(),
            "540 without entry"
        );
        assert!(rules.upgrades.contains(&ROB_UPGRADE));
        // Time markers.
        for (guid, anim, label, ms) in [
            (HUNTER_SPA, 1, 2, 720),
            (HUNTER_SPA, 1, 22, 1300),
            (HUNTER_SPA, 1, 1, 1800),
            (HUNTER_SPA, 5, 2, 760),
            (HUNTER_SPA, 5, 1, 1720),
            (HUNTER_PRI, 4, 3, 863),
            (HUNTER_PRI, 4, 1, 1400),
            (HUNTER_PRI, 3, 2, 760),
            (HUNTER_PRI, 3, 1, 1720),
            (HUNTER_SID, 1, 2, 720),
            (HUNTER_SID, 1, 22, 1300),
            (HUNTER_SID, 1, 1, 1800),
            (HUNTER_SID, 4, 2, 760),
            (HUNTER_SID, 4, 1, 1720),
            (UNDERTAKER, 3, 1, 2499),
            (SANDERS, 7, 1, 2499),
        ] {
            assert_eq!(
                rules.anim_label_time_at(guid, anim, label),
                ms,
                "{guid}/{anim} marker {label}"
            );
        }
        // The spell world remains underneath: targets and command bars from `spell_rules()`.
        assert_eq!(rules.toolbar.menus[&(SHARPSHOOTER as i32)].len(), 1);
        assert!(rules.target_entry(457).is_some() && rules.target_entry(434).is_some());
        // Counter-check: `rules()`, `evoke_rules()`, `spell_rules()` do not know the headhunter world.
        for r in [super::rules(), evoke_rules(), spell_rules()] {
            assert!(!r.types.contains_key(&HUNTER_SPA) && !r.types.contains_key(&UNDERTAKER));
            assert!(!r.toolbar.menus.contains_key(&(HQ_SPA as i32)));
        }
    }

    #[test]
    fn the_hunter_price_is_read_at_slot_eight() {
        let rules = hunter_rules();
        // Slot 8 reads `defaults` (no `slot_configs` in the test world), config 12 = 300.
        for guid in [HUNTER_SPA, HUNTER_PRI, HUNTER_SID] {
            let set = rules.type_value_set(8, guid).expect("value set at slot 8");
            assert_eq!(set.get(&12), Some(&300), "{guid}");
            assert_eq!(rules.config_value(8, guid, 12), 300, "{guid}");
        }
        assert!(rules.level.slot_configs.is_empty());
    }

    #[test]
    fn a_wall_ring_cuts_off_a_region() {
        let rules = hunter_rules();
        let mut state = hunter_state(&rules);
        wall_ring(&mut state, (32, 32), 2, 3);
        let mut sim = Sim::new(&mut state, &rules);
        let hunter = spawn(&mut sim, HUNTER_SPA, 8, (10, 10));
        let robber = spawn(&mut sim, UNDERTAKER, 0, (12, 10));
        let fx = |t: i32| t << 12;
        for id in [hunter, robber] {
            let layer =
                crate::messaging::send(&mut sim, id, &crate::message::Message::new(0x1400, &[]));
            let outside = crate::world::region_id_at(&mut sim, id, -1, -1, -1);
            let inside = crate::world::region_id_at(&mut sim, id, -1, fx(32), fx(32));
            let ring = crate::world::region_id_at(&mut sim, id, -1, fx(35), fx(32));
            let beyond = crate::world::region_id_at(&mut sim, id, -1, fx(50), fx(50));
            assert_ne!(
                inside,
                crate::path::BLOCKED,
                "Layer {layer}: inner area free"
            );
            assert_eq!(ring, crate::path::BLOCKED, "Layer {layer}: ring blocked");
            assert_ne!(inside, outside, "Layer {layer}: inside != outside");
            assert_eq!(beyond, outside, "Layer {layer}: outside connected");
        }
        // The inner area (`max(|dx|, |dz|) <= 2`) stays free, the ring (3) is closed.
        for (x, ok) in [(30, true), (34, true), (29, false), (35, false), (36, true)] {
            let free = sim.state.path.region_id(0, x, 32) != crate::path::BLOCKED;
            assert_eq!(free, ok, "tile ({x}, 32)");
        }
    }

    #[test]
    fn spell_targets_parse_completely() {
        let text = format!("{TARGETS}{SPELL_TARGETS}");
        let file = parse_targets(&text, &parse_target_defines(SPELL_DEFINES));
        assert_eq!(file.aborted, None);
        // Without the defines the loader aborts silently.
        assert!(
            parse_targets(&text, &parse_target_defines(""))
                .aborted
                .is_some()
        );
        let r = spell_rules();
        assert_eq!(r.targets, file.entries);
        // Words as in the shipped data (`spell_buttons_and_filters_match_the_analysis`): mode, mode 2, status, match
        // bits, module, GUID, method of TARGET and following filter.
        type Words = (u32, u32, u32, u32, u32, i32, i32);
        let words = |f: &opennml_formats::targets::MatchFilter| -> Words {
            (
                f.mode,
                f.mode2,
                f.status,
                f.match_bits,
                f.prop_id,
                f.guid,
                f.method,
            )
        };
        let fog_cp = 0x400 | 0x200_0000 | 0x10 | 0x4;
        let kill_head = (fog_cp | 0x4000 | 0x2000_0000, 2, 0, 131_072, 0, 0, 3);
        let kill_next = (
            0x200 | 0x200_0000 | 0x10 | 0x4 | 0x4000,
            2,
            0,
            131_072,
            0,
            0,
            3,
        );
        let expected: [(i32, Words, Option<Words>, i32); 12] = [
            (454, (fog_cp, 0, 0, 0, 3, 0, 0), None, 7),
            (455, (fog_cp | 0x2, 0, 0, 258, 2, 0, 0), None, 6),
            (456, kill_head, Some(kill_next), 7),
            (548, kill_head, Some(kill_next), 7),
            (
                457,
                (fog_cp | 0x1000_0000, 2, 0, 0, 0, 0, 3),
                Some((0x8000, 0, 0, 0, 27, 0, 0)),
                9,
            ),
            (
                546,
                (fog_cp | 0x2000_0000, 2, 0, 0, 0, 0, 3),
                Some((0x200 | 0x200_0000 | 0x10 | 0x4, 2, 0, 0, 0, 0, 3)),
                9,
            ),
            (480, (fog_cp, 2, 0, 0, 0, 0, 3), None, 7),
            (481, (fog_cp, 2, 0, 0, 3, 0, 3), None, 7),
            (484, (0, 0, 0, 0, 0, 0, 0), None, 7),
            (556, (0, 0, 0, 0, 0, 0, 0), None, 7),
            (541, (0x10 | 0x4 | 0x400, 0, 0, 0, 0, 0, 0), None, 7),
            (551, (fog_cp, 0, 0, 0, 3, 0, 0), None, 6),
        ];
        for (msg, target, next, icon) in expected {
            let e = r.target_entry(msg).expect("entry");
            assert!(e.enabled, "{msg}");
            assert_eq!(words(&e.target), target, "{msg}: TARGET");
            assert_eq!(
                e.target.next.as_deref().map(words),
                next,
                "{msg}: following filter"
            );
            assert_eq!(e.mouse_icon, icon, "{msg}: MAUSICONID");
        }
        // MY filters: 454 with `ISMOUNTED`, 457/546 not `SWIMMING`, 541 only 1113, 484/556 own.
        let my = |msg: i32| words(&r.target_entry(msg).unwrap().my);
        assert_eq!(my(454), (0x2016, 0, 0x2000, 292, 36, 0, 0));
        assert_eq!(my(455), (0x16, 0, 0, 548, 36, 0, 0));
        assert_eq!(my(456), (0x16, 0, 0, 293, 37, 0, 0));
        assert_eq!(my(457), (0x2016, 0, 0x1_0000, 549, 37, 0, 0));
        assert_eq!(my(546), my(457));
        assert_eq!(my(548), my(456));
        assert_eq!(my(480), (0x16, 0, 0, 294, 38, 0, 0));
        assert_eq!(my(481), (0x16, 0, 0, 550, 38, 0, 0));
        assert_eq!(my(484), (0x100, 0, 0, 0, 0, 0, 0));
        assert_eq!(my(556), my(484));
        assert_eq!(my(541), (0x8, 0, 0, 0, 0, 1113, 0));
        assert_eq!(my(551), (0x4, 0, 0, 0, 49, 0, 0));
        assert!(r.target_entry(482).is_none() && r.target_entry(542).is_none());
        // The entries of the ability buttons stay.
        assert!(r.target_entry(434).is_some() && r.target_entry(564).is_some());
        // Command bar: slots, kinds and prerequisites as in the shipped data.
        let t = opennml_formats::gifcon::button_kind::TARGET | 1;
        let d = opennml_formats::gifcon::button_kind::DO | 1;
        let menu = |guid: u32| -> Vec<(i32, i32, u32, i32)> {
            r.toolbar.menus[&(guid as i32)]
                .iter()
                .map(|b| (b.msg, b.id, b.flags, b.required[0]))
                .collect()
        };
        assert_eq!(
            menu(MEDICINE_MAN),
            [(480, 7, t, 3024), (481, 8, t, 3025), (482, 9, d, 3026)]
        );
        assert_eq!(menu(CHIEF), [(484, 8, t, 3022)]);
        assert_eq!(menu(MISSIONARY), [(541, 8, t, 2832)]);
        assert_eq!(menu(PATRIOT), [(455, 7, t, 3227), (454, 8, t, 3226)]);
        assert_eq!(menu(SHADOW_WARRIOR), [(548, 7, t, 3027)]);
        assert_eq!(menu(TRAPPER), [(556, 7, t, 3128), (456, 9, t, 3127)]);
        assert_eq!(menu(PISTOLERO), [(546, 7, t, 2831)]);
        assert_eq!(menu(SHARPSHOOTER), [(457, 7, t, 3327)]);
        assert_eq!(menu(SMITH), [(551, 8, t, 3122), (542, 9, d, 3121)]);
        assert_eq!(r.toolbar.menus[&1402].len(), 2, "TOOLBAR stays");
        // Counter-check: `rules()` does not know the spell entries.
        assert!(rules().target_entry(480).is_none());
    }
}
