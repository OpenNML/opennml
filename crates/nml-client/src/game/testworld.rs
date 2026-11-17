//! Shared viewer test world: the Prairie Indian tents 1600, 1601 and 1603 with the hulls from re/tents 9.2, the travois
//! 1717 and the worker 1702; also the units 1100, 1102, 1112, 1200, 1201 with the start of the hero list
//! (re/heroes-resources 1.2). It adds the trade cart 1110 (module 43) and the gate 1013 (`TOR | OHNERAND |
//! CASTLEBUILDING`, hull 4 x 1 tiles, `frei01/frei02`, buttons 430/431 like `module::building::fort_testkit` in
//! `nml_sim`). The `nml_sim` test kit (`module::tent::testkit`) is not visible in the viewer; types, menus, values and
//! hulls come from there (`property.ini`, `default.dat`, `toolbar.ini` of the shipped data, re/tents 1.1 and 2.5,
//! simplified where noted). The host is created like the other viewer test worlds via [`SimHost::new`] (pattern
//! `economy::tests::host`).
//!
//! It also adds camouflage, dig in and healing after the pattern of `nml_sim::module::ability_testkit` (there, type
//! texts, buttons and filters are created synthetically; here the same values stand, but only the types and buttons of
//! the bar): the archer 1402 (module 25 in variant 10 = 2924), the assassin unit 1407 (module 28 in variant 9 = 2922),
//! the pilgrim 2014 and the medic 2303 (module 29), the alarm post 1610 (building with modules 25 and 23), the variant
//! rules, the bar, `targets.ini` and the value set of button 555. Further tests need the same alarm post.
//!
//! It also adds ghosts, banishing and animal evocation after the pattern of
//! `nml_sim::module::ability_testkit::evoke_rules` (invisible in the viewer, everything is copied from there): the
//! ghost evoker 1411 (modules 26, 38, 17, 31), the animal evoker 1412 (module 32), the ghost 1413 (module 27), the wolf
//! 3401 (animal 20, meat 21, dying 16, aura 44 with 4460) and the missionary 1113 (riding, banishing 439); also
//! `EVOKE_DEFINES`, bar rows with button kinds and `REQUIRED_GUID`, the `targets.ini` entries 436, 440 and 441-443, the
//! values, costs, time markers and the aura 4460 as `temp_upgrades` with `upgrade_types`. The world leaves the pilgrim
//! 2014 unchanged (no banishing, no button 553): its previous behavior stays.
//!
//! It also adds the spells and special attacks after `nml_sim::module::ability_testkit::spell_rules` (invisible in the
//! viewer, copied), but only what the viewer tests need: the medicine man 1716 (spell 38, 480-482), the chief 1715
//! (484), the trapper 2015 (killer near, 456/556), the sharpshooter 2616 (simplified: only the modules 0, 2, 16 and 37
//! for the MY filter of 457), the patriot 2315 (demolition expert 36, 454/455), the shadow warrior 1714 (548) and the
//! smith 2013 (SpecialMove 49, 551/542); also bar rows, the `targets.ini` entries of these buttons with the `PROP_` and
//! `MID_PROP_` names ([`SPELL_DEFINES`]), values, costs 454-556, time markers and research. The missionary 1113 stays
//! the evocation missionary (no module 38, no button 541): button 541 is untested in the viewer, the simulation covers
//! it.
//!
//! It also adds the headhunter and robbing after `nml_sim::module::ability_testkit::hunter_rules` (invisible in the
//! viewer, copied): the headquarters 1000 and 2500 (buildings, quarters 23, hire buttons 4100-4102), the hunters
//! 4100-4102 (module 34), the sheriff 2614 and the robbers 2615 and 2702 (module 48, button 540 with `REQUIRED_GUID
//! 3326`), also values, time markers, hulls with an exit and the research 3326. The other elite units stay the existing
//! types: 2616 the sharpshooter of the spell world, 1112 and 1113 governor and missionary (no new entries).
//! [`hunter_host`] lets slots 0 (SPA) and 1 (SID) play.

use nml_formats::gifcon::{parse_toolbar, parse_variant_rules};
use nml_formats::targets::{parse_target_defines, parse_targets};
use nml_formats::temp_upgrades::parse_temp_upgrades;
use nml_sim::id::ObjectId;
use nml_sim::message::Message;
use nml_sim::messaging::send;
use nml_sim::module::tent::transport;
use nml_sim::rules::{BuildingShape, ModelBox, Rules};
use nml_sim::setup::MatchSetup;
use nml_sim::sim::Sim;
use nml_sim::state::Control;
use nml_sim::world::{ObjectDesc, create_object, desc_flags};

use crate::game::options::DEFAULT_GAME_SPEED;
use crate::game::sim_host::SimHost;

/// `PRI_Haeuptlingszelt`: modules 3, 23, 40, 42; button 452.
pub const CHIEF_TENT: u32 = 1600;
/// `PRI_Wohnzelt`: modules 3, 40, 42; button 452.
pub const DWELLING_TENT: u32 = 1601;
/// `PRI_Lagerstaette`: building without module 42, button 452 in the menu anyway (W-262).
pub const STORAGE_TENT: u32 = 1603;
/// `PRI_ZeltTravois`: modules 0, 2, 35, 16; button 453.
pub const TRAVOIS: u32 = 1717;
/// Worker of the Prairie Indians (`PRI_Frau`): modules 0, 2, 16.
pub const COLONIST: u32 = 1702;
/// `SPA_Transportkarren`: modules 0, 2, 16, 43 (trade, all data values 0 as in the shipped data).
pub const CART: u32 = 1110;
/// Spanish colonist and soldier (no heroes): modules 0, 2.
pub const SPA_COLONIST: u32 = 1100;
pub const SPA_SOLDIER: u32 = 1102;
/// Governor (hero index 20), Carvinez (0) and Mendoza (1): modules 0, 2.
pub const GOVERNOR: u32 = 1112;
pub const CARVINEZ: u32 = 1200;
pub const MENDOZA: u32 = 1201;
/// `SPA_Tor`: building with `TOR | OHNERAND | CASTLEBUILDING` (`0x10024000`), buttons 430 and 431.
pub const GATE: u32 = 1013;

/// `WID_Bogenschuetze`: module 25 in variant 10 (2924), camouflage animation 7.
pub const ARCHER: u32 = 1402;
/// `WID_Meucheleinheit`: module 28 in variant 9 (2922).
pub const ASSASSIN: u32 = 1407;
/// `PRI_Alarmposten`: building with module 25 (animations `0 1`, `EVERYTIMETARNED`) and quarters 2.
pub const ALARM_POST: u32 = 1610;
/// `ENG_Pilgervater`: healer (module 29), buttons 554/564 with `REQUIRED_GUID 3125`, 555 without.
pub const PILGRIM: u32 = 2014;
/// `PAT_Feldarzt`: healer (module 29), buttons 458/459.
pub const MEDIC: u32 = 2303;
/// Upgrade "Assassins can dig in" (variant bit 9).
pub const DIG_IN_UPGRADE: u32 = 2922;
/// Upgrade "Indians can camouflage themselves" (variant bit 10).
pub const MASK_UPGRADE: u32 = 2924;
/// Upgrade "Research healing" (buttons 554/564 of the pilgrim).
pub const HEAL_UPGRADE: u32 = 3125;

/// `WID_Geisterbeschwoerer`: modules 26, 38 (spell, mana tick), 17 and 31 (banishing, anim 10).
pub const GHOST_EVOKER: u32 = 1411;
/// `WID_Tierbeschwoerer`: module 32 (evoke animals), swimming.
pub const ANIMAL_EVOKER: u32 = 1412;
/// `WID_Geist`: floating (walk `0x1900`), module 27, FightFar.
pub const GHOST: u32 = 1413;
/// `ANI_Wolf`: animal kind 0, modules 0, 2, 16, 17, 18, 20, 21 and 44 (aura 4460).
pub const WOLF: u32 = 3401;
/// `SPA_Missionar`: riding, banishing (module 31, anim 6, button 439).
pub const MISSIONARY: u32 = 1113;
/// Aura of the wolf (near/far +60 %).
pub const WOLF_AURA: u32 = 4460;
/// Research "Evoke wolf" (button 441), "Wildcat" (443), "Bear" (442), "Evoke ghost" (436), "Banish ghosts" of
/// the ghost evoker (547) and "Cast banishment" of the missionary (439).
pub const EVOKE_WOLF_UPGRADE: u32 = 2926;
pub const EVOKE_CAT_UPGRADE: u32 = 2927;
pub const EVOKE_BEAR_UPGRADE: u32 = 2928;
pub const EVOKE_GHOST_UPGRADE: u32 = 2930;
pub const BANISH_GHOSTS_UPGRADE: u32 = 2931;
pub const MISSIONARY_BANISH_UPGRADE: u32 = 2827;

/// `PRI_Medizinmann`: spell 38 (`SPELLEXPLOSION | SPELLSTORM`, animation 2), buttons 480-482.
pub const MEDICINE_MAN: u32 = 1716;
/// `PRI_Hauptling`: scout 484.
pub const CHIEF: u32 = 1715;
/// `ENG_Trapper`: killer 37 near (animation 8, damage 300), buttons 456 and 556.
pub const TRAPPER: u32 = 2015;
/// `SID_Scharfschuetze` (simplified): killer 37 far (animation 1, damage 300), button 457.
pub const SHARPSHOOTER: u32 = 2616;
/// `PAT_Patriot`: demolition expert 36, riding module 13; buttons 454/455.
pub const PATRIOT: u32 = 2315;
/// `PRI_Schattenkrieger`: killer 37 near (animation 5, damage 300), button 548.
pub const SHADOW_WARRIOR: u32 = 1714;
/// `ENG_Schmied`: SpecialMove 49 (animations `4 5`), buttons 551 and 542.
pub const SMITH: u32 = 2013;
/// Research of the spells and special attacks (`REQUIRED_GUID` of the buttons in [`SPELL_TOOLBAR`]):
/// Scout 484, Explosion 480, Storm 481, Reincarnation 482, Eliminate unit 548, Whirlwind 542, Damage building 551,
/// Kill unit 456, Scout 556 of the trapper, Dynamite at building 454 and at ship 455, Eliminate (ranged) 457.
pub const SCOUT_UPGRADE: u32 = 3022;
pub const EXPLOSION_UPGRADE: u32 = 3024;
pub const STORM_UPGRADE: u32 = 3025;
pub const REINCARNATION_UPGRADE: u32 = 3026;
pub const SHADOW_KILL_UPGRADE: u32 = 3027;
pub const WHIRLWIND_UPGRADE: u32 = 3121;
pub const DAMAGE_BUILDING_UPGRADE: u32 = 3122;
pub const TRAPPER_KILL_UPGRADE: u32 = 3127;
pub const TRAPPER_SCOUT_UPGRADE: u32 = 3128;
pub const DYNAMITE_BUILDING_UPGRADE: u32 = 3226;
pub const DYNAMITE_SHIP_UPGRADE: u32 = 3227;
pub const SNIPE_UPGRADE: u32 = 3327;

/// `Kopfgeldjaeger_SPA` (kind 0), `Kopfgeldjaeger_PRI` (kind 1), `Kopfgeldjaeger_SID` (kind 2): module 34.
pub const HUNTER_SPA: u32 = 4100;
pub const HUNTER_PRI: u32 = 4101;
pub const HUNTER_SID: u32 = 4102;
/// `SPA_Hauptquartier` and `SID_Rathaus`: hire 4100-4102, quarters 23.
pub const HQ_SPA: u32 = 1000;
pub const HQ_SID: u32 = 2500;
/// `SID_Sheriff`: elite of the SID, here with the values of the colonist.
pub const SID_SHERIFF: u32 = 2614;
/// `SID_Leichenbestatter`: robbing 48 (`m_RobAnim` 3), attack 17, mana 150, button 540.
pub const UNDERTAKER: u32 = 2615;
/// `SID_WilliamSanders`: robbing 48 (`m_RobAnim` 7), attack 17, mana 100, button 540.
pub const SANDERS: u32 = 2702;
/// Research "Rob corpses" (`REQUIRED_GUID` of button 540).
pub const ROB_UPGRADE: u32 = 3326;

/// The first 21 entries of `heroes.ini` (re/heroes-resources 1.2): 1200 has index 0, 1201 index 1, 1112 index 20.
pub const HEROES: [u32; 21] = [
    1200, 1201, 1500, 1501, 1502, 1800, 1801, 1802, 2100, 2101, 2400, 2401, 2402, 2702, 2720, 2721,
    2722, 2723, 2724, 2725, 1112,
];

/// Synthetic `property.ini` (excerpt of the `nml_sim` test kit).
const TYPES: &str = concat!(
    "GUID 1600\nNAME PRI_Haeuptlingszelt\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 800768\nVALUEID 40004 1 1 0\n",
    "PROPID 23\nVALUEID 240001 6 1 0\n",
    "PROPID 42\nVALUEID 430001 1 1 1717\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    "GUID 1601\nNAME PRI_Wohnzelt\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 67633152\nVALUEID 40004 1 1 0\n",
    "PROPID 42\nVALUEID 430001 1 1 1717\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    "GUID 1603\nNAME PRI_Lagerstaette\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 538624\nVALUEID 40004 1 1 0\n",
    "PROPID 40\nVALUEID 410001 1 1 50\n",
    // Travois: walk (test values), stand, module 35 with flags 0, dying.
    "GUID 1717\nNAME PRI_ZeltTravois\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
    "PROPID 2\nVALUEID 30001 1 1 1\nVALUEID 30002 1 1 0\nVALUEID 30003 6 1 0\n",
    "PROPID 40 11\nVALUEID 410001 1 1 80\n",
    "PROPID 35\nVALUEID 360001 6 1 0\n",
    "PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 1 1 2\n",
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
    // Spanish units and heroes (simplified: walk without values, stand). `PROPID 0` is mandatory, otherwise
    // `Selection::keep_units` removes them from a multi-selection.
    "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
    "GUID 1102\nNAME SPA_Soldat\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
    "GUID 1112\nNAME SPA_Gouverneur\nVALUEID 100 6 1 41943040\n",
    "PROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
    "GUID 1200\nNAME Carvinez\nVALUEID 100 6 1 41943040\n",
    "PROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
    "GUID 1201\nNAME Mendoza\nVALUEID 100 6 1 41943040\n",
    "PROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
    // Gate: TOR | OHNERAND | CASTLEBUILDING (`0x10024000` = 268582912).
    "GUID 1013\nNAME SPA_Tor\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 268582912\n",
    // Camouflage, dig in, healing (excerpt from `ability_testkit`, re/masking 1.1, re/healing 1.1): walk, stand,
    // dying and attack (flags 0) as there, swimming for the archer.
    "GUID 1402\nNAME WID_Bogenschuetze\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 2\nVALUEID 10003 6 1 276736\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 0\n",
    "PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
    "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n",
    "PROPID 15\nVALUEID 160002 1 1 3\nVALUEID 160003 1 1 4\nVALUEID 160004 1 1 5\n",
    "PROPID 25 10\nVALUEID 260001 1 1 7\nVALUEID 260002 6 1 0\n",
    "GUID 1407\nNAME WID_Meucheleinheit\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 2\nVALUEID 10003 6 1 276736\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 2\n",
    "PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
    "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n",
    "PROPID 28 9\nVALUEID 290001 6 1 8\nVALUEID 290002 6 1 0\n",
    "VALUEID 290003 6 1 1\nVALUEID 290004 6 1 250\n",
    // Alarm post: building like the dwelling tent, quarters and module 25 (animations `0 1`).
    "GUID 1610\nNAME PRI_Alarmposten\nVALUEID 100 6 1 33554432\n",
    "PROPID 3\nVALUEID 40003 6 1 67633152\nVALUEID 40004 1 1 0\n",
    "PROPID 25\nVALUEID 260001 1 1 0 1\nVALUEID 260002 6 1 256\n",
    "PROPID 23\nVALUEID 240001 6 1 0\n",
    // Healer: walk of the shipped data (flags 4458752), module 29 (`0x3400`, healing animation 7, 288/min).
    "GUID 2014\nNAME ENG_Pilgervater\nVALUEID 100 6 1 41943040\n",
    "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 4458752\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 0\n",
    "PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
    "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n",
    "PROPID 29\nVALUEID 300001 1 1 7\nVALUEID 300002 6 1 13312\nVALUEID 300003 6 1 7\n",
    "VALUEID 300004 6 1 2\nVALUEID 300005 1 1 0\nVALUEID 300006 6 1 7\nVALUEID 300007 6 1 288\n",
    "VALUEID 300008 7 1 data/effects/healing_circle.hef\n",
    "GUID 2303\nNAME PAT_Feldarzt\nVALUEID 100 6 1 33554432\n",
    "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 4458752\nVALUEID 10004 6 1 512\n",
    "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 0\n",
    "PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n",
    "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n",
    "PROPID 29\nVALUEID 300001 1 1 6\nVALUEID 300002 6 1 13312\nVALUEID 300003 6 1 3\n",
    "VALUEID 300004 6 1 5\nVALUEID 300005 1 1 4\nVALUEID 300006 6 1 7\nVALUEID 300007 6 1 288\n",
    "VALUEID 300008 7 1 data/effects/healing_circle.hef\n",
);

/// Menus like `toolbar.ini` (re/tents 2.5): 452 for 1600, 1601 and 1603, 453 and dying (407) for the travois,
/// the worker with "Repair building" (413).
const TOOLBAR: &str = concat!(
    "GUID 1600\n",
    "BUTTONID 1\nBUTTONMSG 1702\nMAKE_MSG\nREQUIRED_GUID 1600\n",
    "BUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\n",
    "ENDGUID\n",
    "GUID 1601\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\nENDGUID\n",
    "GUID 1603\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\nENDGUID\n",
    "GUID 1717\nBUTTONID 5\nBUTTONMSG 453\nTARGET_DO_MSG\nBUTTONID 13\nBUTTONMSG 407\nENDGUID\n",
    "GUID 1702\nBUTTONID 2\nBUTTONMSG 413\nTARGET_MSG\nENDGUID\n",
    // Gate bar (re/arming-gates 2.C): 430 and 431 both button 0, `TARGET_DO_MSG`, no `REQUIRED_GUID`.
    "GUID 1013\nBUTTONID 0\nBUTTONMSG 430\nTARGET_DO_MSG\n",
    "BUTTONID 0\nBUTTONMSG 431\nTARGET_DO_MSG\nENDGUID\n",
    // Camouflage, dig in, healing like `ability_testkit` (kinds and prerequisites from the corpus).
    "GUID 1402\n",
    "BUTTONID 5\nBUTTONMSG 434\nTARGET_DO_MSG\nREQUIRED_GUID 2924\n",
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
);

/// Entries of the ability buttons from `targets.ini` like `ability_testkit::TARGETS` (re/masking 2.4,
/// re/healing 2.4): MY filter 434 not `MASKED` and not `SWIMMING`, 435 `MASKED`, 437/438 by
/// `WAITING4KILLING`, 458/554 not `HEAL`, 459/564 `HEAL`; 555 has no entry.
const TARGETS: &str = concat!(
    "BEGIN\nBUTTONMSG 434\nMY_MM\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS MASKED\n",
    "MM_MODE MM_ANDALL\nMM_MODE MM_ANDNEXTMATCH\nMM_NEXT\nMM_MODE MM_CHECKNOTSTATUS\n",
    "MM_STATUS SWIMMING\nMM_END\nTARGET_MM\nMM_MODE MM_RETURNFALSE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 435\nMY_MM\nMM_MODE MM_CHECKSTATUS\nMM_STATUS MASKED\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_RETURNFALSE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 437\nMY_MM\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS WAITING4KILLING\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_RETURNFALSE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 438\nMY_MM\nMM_MODE MM_CHECKSTATUS\nMM_STATUS WAITING4KILLING\nMM_END\n",
    "TARGET_MM\nMM_MODE MM_RETURNFALSE\nMM_END\nEND\n",
    "BEGIN\nBUTTONMSG 458\nMY_MM\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS HEAL\nMM_END\n",
    "TARGET_MM\nMM_END\nMAUSICONID 7\nEND\n",
    "BEGIN\nBUTTONMSG 459\nMY_MM\nMM_MODE MM_CHECKSTATUS\nMM_STATUS HEAL\nMM_END\n",
    "TARGET_MM\nMM_END\nMAUSICONID 7\nEND\n",
    "BEGIN\nBUTTONMSG 554\nMY_MM\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS HEAL\nMM_END\n",
    "TARGET_MM\nMM_END\nMAUSICONID 7\nEND\n",
    "BEGIN\nBUTTONMSG 564\nMY_MM\nMM_MODE MM_CHECKSTATUS\nMM_STATUS HEAL\nMM_END\n",
    "TARGET_MM\nMM_END\nMAUSICONID 7\nEND\n",
);

/// Variant rules (`upgrades.ini`, excerpt): bit 9 = 2922, bit 10 = 2924.
const VARIANT_RULES: &str = "BIT 0 = 1\nBIT 9 = 2922\nBIT 10 = 2924\n";

/// Time markers of an animation: type, animation, `(marker, ms)`.
type AnimLabels = (u32, usize, &'static [(u32, i32)]);

/// Time markers of the ability animations like `ability_testkit::LABELS`.
const LABELS: &[AnimLabels] = &[
    (ARCHER, 7, &[(1, 1600)]),
    (ALARM_POST, 0, &[(1, 33)]),
    (ALARM_POST, 1, &[(1, 33)]),
    (ASSASSIN, 0, &[(1, 773)]),
    (ASSASSIN, 1, &[(1, 1180), (2, 640), (22, 960)]),
    (ASSASSIN, 8, &[(1, 1843)]),
    (PILGRIM, 7, &[(1, 3999)]),
    (MEDIC, 3, &[(1, 4500)]),
    (MEDIC, 6, &[(1, 4500)]),
];

/// `default.dat` values (13 build time, 14 hit points, 16 speed, 23 intake, 27 living space).
const DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (CHIEF_TENT, &[(13, 63), (14, 2400), (23, 5), (27, 25)]),
    (DWELLING_TENT, &[(13, 14), (14, 540), (27, 15)]),
    (STORAGE_TENT, &[(13, 21), (14, 1000)]),
    (TRAVOIS, &[(14, 500)]),
    (COLONIST, &[(10, 50), (13, 15), (14, 160), (16, 145)]),
    (CART, &[(14, 300), (16, 183), (17, 200)]),
    (GATE, &[(13, 6), (14, 1000)]),
    (ARCHER, &[(14, 155), (15, 200), (16, 200), (18, 40)]),
    (ASSASSIN, &[(14, 200), (15, 0), (16, 145), (18, 40)]),
    (
        ALARM_POST,
        &[(13, 18), (14, 1600), (15, 500), (18, 64), (23, 2)],
    ),
    (PILGRIM, &[(14, 1000), (15, 950), (16, 183), (18, 32)]),
    (MEDIC, &[(14, 200), (15, 950), (16, 200), (18, 40)]),
    // Value set of the button GUID 555 (cost in config 15), test value 50 as in `ability_testkit`.
    (555, &[(15, 50)]),
];

// --- Ghosts, banishing, animal evocation --------------------------------------------------------
//
// Copied from `nml_sim::module::ability_testkit` (`EVOKE_*`, `evoke_only_types`), which holds the values of the
// shipped data from re/ghosts-banish 1.1/2.6 and re/evoke-aura 1.1/2.4.

/// Walk with the values of the shipped data (`10001`, `10003` flags, `10004`/`10005` radii, `10008`).
fn walk(anim: i32, flags: u32, hard: i32, soft: i32, w8: i32) -> String {
    format!(
        "PROPID 0\nVALUEID 10001 6 1 {anim}\nVALUEID 10003 6 1 {flags}\nVALUEID 10004 6 1 {hard}\n\
         VALUEID 10005 6 1 {soft}\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 {w8}\n"
    )
}

/// Unit: header with category `0x2000000`, walk, stand, dying (test animations 0 and 3) and the modules
/// `rest`.
fn unit(guid: u32, name: &str, walk: &str, rest: &str) -> String {
    unit_in(guid, name, 0x200_0000, walk, rest)
}

/// Like [`unit`] with the category `category` (value 100 of the header).
fn unit_in(guid: u32, name: &str, category: u32, walk: &str, rest: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 {category}\n{walk}\
         PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n{rest}"
    )
}

/// Attack module without flags.
const ATTACK: &str = "PROPID 17\nVALUEID 180001 6 1 0\nVALUEID 180002 6 1 0\n";
/// Swimming (test animations like `riding::testkit`).
const SWIM: &str = "PROPID 15\nVALUEID 160002 1 1 3\nVALUEID 160003 1 1 4\nVALUEID 160004 1 1 5\n";
/// Riding (module 13) with the test animations of the colonist from `riding::testkit`.
const RIDING: &str = "PROPID 13\nVALUEID 140001 6 1 0\nVALUEID 140002 1 1 2\nVALUEID 140003 1 1 27\n\
VALUEID 140004 1 1 15\nVALUEID 140005 1 1 14\nVALUEID 140006 1 1 32\n";
/// Spell (module 38) with the flags 390001 = 768 of the ghost evoker.
const SPELL: &str = "PROPID 38\nVALUEID 390001 6 1 768\n";
/// Melee of the animals (values like `combat::testkit::WOLF`): attack 17 with `256`, FightNear 18 with
/// the combat animation 2.
const ANIMAL_MELEE: &str = "PROPID 17\nVALUEID 180001 6 1 256\nVALUEID 180002 6 1 0\n\
PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 2\n";

/// Module 31 (re/ghosts-banish 1.3): target filter 320001 = 440, flags 320003, banishment animation 320004,
/// node prefix 320005.
fn banish(flags: u32, anim: i32, node: &str) -> String {
    format!(
        "PROPID 31\nVALUEID 320001 6 1 440\nVALUEID 320003 6 1 {flags}\nVALUEID 320004 6 1 {anim}\n\
         VALUEID 320005 7 1 {node}\n"
    )
}

/// The evocation types (to be appended after [`TYPES`]): the wolf with all modules up to dying (16)
/// and the aura (44).
fn evoke_types() -> String {
    let soldier = walk(1, 276_736, 512, 1024, 0);
    [
        unit(
            GHOST_EVOKER,
            "WID_Geisterbeschwoerer",
            &soldier,
            &format!(
                "PROPID 26\nVALUEID 270001 6 1 0\n{SPELL}{ATTACK}{}",
                banish(0x500, 10, "geisterhand")
            ),
        ),
        // Ghost: walk without hard radius, `SIZE1x1 | LAND | FOREST`; attack `FIGHTFAR` (`0x800`).
        unit(
            GHOST,
            "WID_Geist",
            &walk(0, 0x1900, 0, 1024, 0),
            "PROPID 27\nVALUEID 280001 6 1 0\nPROPID 17\nVALUEID 180001 6 1 2048\n\
             VALUEID 180002 6 1 0\nPROPID 19\nVALUEID 200001 6 1 0\n",
        ),
        unit(
            ANIMAL_EVOKER,
            "WID_Tierbeschwoerer",
            &soldier,
            &format!("{SWIM}PROPID 32\nVALUEID 330001 6 1 0\n{ATTACK}"),
        ),
        // Wolf: animal 20 (`0x2100`, kind 0), meat 21, melee, module 44 with the aura 4460 and the
        // influence effect 450003; 450002 is missing as in the shipped data.
        unit(
            WOLF,
            "ANI_Wolf",
            &walk(1, 2_119_936, 1024, 2048, 1),
            &format!(
                "PROPID 20\nVALUEID 210001 6 1 8448\nVALUEID 210002 6 1 0\nPROPID 21\n{ANIMAL_MELEE}\
                 PROPID 44\nVALUEID 450001 1 1 {WOLF_AURA}\nVALUEID 450003 7 1 data/effects/wolf.hef\n"
            ),
        ),
        // Missionary: riding with the test banishment animation 12 in the saddle (140010), banishing without
        // flags for module 17 (anim 6, button 439).
        unit(
            MISSIONARY,
            "SPA_Missionar",
            &walk(1, 4_458_752, 512, 1024, 0),
            &format!(
                "{RIDING}VALUEID 140010 1 1 12\n{ATTACK}{}",
                banish(0xD00, 6, "bannspruch")
            ),
        ),
    ]
    .concat()
}

/// Bar rows of the evocations (button kinds and slots as in the shipped data,
/// `evocation_buttons_and_filters_match_the_analysis`).
const EVOKE_TOOLBAR: &str = concat!(
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
);

/// `targetdefines.ini` excerpt for [`EVOKE_TARGETS`]: without the names `parse_targets` stops silently at
/// `MM_PROPERTY` and discards all entries after it.
const EVOKE_DEFINES: &str = "PROP_EVOKEGHOST 26\nPROP_GHOST 27\nPROP_EVOKEANIMAL 32\n";

/// Entry of `targets.ini` with the MY filter `my` and the TARGET block `target`.
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

/// Entries like `ability_testkit::EVOKE_TARGETS`: 436 and 441-443 with the MY filter `PROP_EVOKEGHOST` or
/// `PROP_EVOKEANIMAL` and not `SWIMMING`; 440 "ghost as target", the target filter of banishing. The
/// banishing buttons have no entry.
const EVOKE_TARGETS: &str = concat!(
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

/// The aura 4460 like `tempupgrades.ini` (flag `0x49` = `TAKEUPGRADEVALUES | EFFECT`, unlimited without `0x80`;
/// re/temp-upgrades 1.4).
const EVOKE_TEMP_UPGRADES: &str =
    "TEMPUPGRADE 4460\nTAKEUPGRADEVALUES\nEFFECT 0 data/effects/units_wolf.pef\nEND\n";

/// Types of the world on which the aura 4460 acts (`OBJ_UPGRADES` of the shipped data, restricted to the types
/// of this world, by GUID as in the loaded rules).
const AURA_TYPES: &[u32] = &[ARCHER, ASSASSIN, GHOST_EVOKER, ANIMAL_EVOKER];

/// Research of the evocations (`[UPGRADE]` of the project file).
const EVOKE_UPGRADE_GUIDS: &[u32] = &[
    EVOKE_WOLF_UPGRADE,
    EVOKE_CAT_UPGRADE,
    EVOKE_BEAR_UPGRADE,
    EVOKE_GHOST_UPGRADE,
    BANISH_GHOSTS_UPGRADE,
    MISSIONARY_BANISH_UPGRADE,
];

/// Time markers of the evocations like `ability_testkit::EVOKE_LABELS`; 1113 anim 12 is the test banishment
/// animation in the saddle.
const EVOKE_LABELS: &[AnimLabels] = &[
    (GHOST_EVOKER, 0, &[(1, 3999)]),
    (GHOST_EVOKER, 10, &[(1, 5332)]),
    (ANIMAL_EVOKER, 0, &[(1, 8000)]),
    (MISSIONARY, 6, &[(1, 9332)]),
    (MISSIONARY, 12, &[(1, 2000)]),
    (GHOST, 0, &[(1, 1666)]),
];

/// `default.dat` values of the evocations like `ability_testkit::EVOKE_DEFAULTS` (14 hit points, 15 extra
/// energy, 16 speed "Slow" 145, 18 sight, 19 near, 20 far, 21 range), the aura (119/120 = 60) and the costs of
/// the buttons (config 15 of the button GUID). The missionary gets mana 200, otherwise he could never banish.
const EVOKE_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
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
    (MISSIONARY, &[(14, 1000), (15, 200), (16, 145), (18, 40)]),
    (WOLF_AURA, &[(119, 60), (120, 60)]),
    (436, &[(15, 25)]),
    (547, &[(15, 25)]),
    (439, &[(15, 25)]),
    (441, &[(15, 25)]),
    (442, &[(15, 50)]),
    (443, &[(15, 30)]),
];

// --- Spells and special attacks -----------------------------------------------------------------
//
// After `ability_testkit::spell_rules`: `SPELL_*` and `spell_only_types` there; values from re/spells 1.1/2.4/2.5 and
// re/special-attacks 1.1/2.5 (checked by the corpus test `tests/corpus_abilities.rs` in `nml_sim`). Only the types,
// buttons and filters of the viewer tests.

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

/// Killer 37 (re/special-attacks 1.1): 380001 flags (`0x100` near, `0x200` far), 380002 near animation,
/// 380003 far animation, 380004 near damage, 380005 far damage.
fn killer(flags: u32, near: i32, far: i32, near_damage: i32, far_damage: i32) -> String {
    format!(
        "PROPID 37\nVALUEID 380001 6 1 {flags}\nVALUEID 380002 1 1 {near}\n\
         VALUEID 380003 1 1 {far}\nVALUEID 380004 1 1 {near_damage}\n\
         VALUEID 380005 1 1 {far_damage}\n"
    )
}

/// Demolition expert 36 of the patriot (re/special-attacks 1.1): flags 768 (`CANBLOWUPBUILDING |
/// CANBLOWUPSHIP`), charge model with the full path of the shipped data, 370003-370006.
const BOMBERMAN: &str = "PROPID 36\nVALUEID 370001 6 1 768\n\
VALUEID 370002 7 1 data/r3d/misc/Tools/Dynamitbuendel/dynamitbuendel.rdo\n\
VALUEID 370003 1 1 1\nVALUEID 370004 1 1 2\nVALUEID 370005 1 1 2\nVALUEID 370006 1 1 3\n";

/// SpecialMove 49 of the smith (re/special-attacks 1.1): 500001 = 0, animations `4 5`.
const SPECIAL_MOVE: &str = "PROPID 49\nVALUEID 500001 6 1 0\nVALUEID 500002 1 1 4 5\n";

/// The spell types of the viewer world (to be appended after [`TYPES`] and [`evoke_types`]), category
/// `0x2800000` as in the shipped data. The sharpshooter is simplified: walk, stand, dying and killer 37
/// suffice for his button 457 and its MY filter (not `SWIMMING`).
fn spell_types() -> String {
    const CATEGORY: u32 = 0x280_0000;
    [
        unit_in(
            MEDICINE_MAN,
            "PRI_Medizinmann",
            CATEGORY,
            &walk(1, 276_736, 512, 1024, 0),
            &format!("{ATTACK}{}", spell(768, "2", "0")),
        ),
        unit_in(
            CHIEF,
            "PRI_Hauptling",
            CATEGORY,
            &walk(1, 272_640, 512, 1024, 0),
            ATTACK,
        ),
        unit_in(
            PATRIOT,
            "PAT_Patriot",
            CATEGORY,
            &walk(0, 4_466_944, 512, 1024, 15),
            &format!("{RIDING}{ATTACK}{BOMBERMAN}"),
        ),
        unit_in(
            SHADOW_WARRIOR,
            "PRI_Schattenkrieger",
            CATEGORY,
            &walk(1, 276_736, 512, 1024, 0),
            &format!("{ATTACK}{}{}", fight_near(5), killer(256, 5, 0, 300, 0)),
        ),
        unit_in(
            TRAPPER,
            "ENG_Trapper",
            CATEGORY,
            &walk(1, 272_640, 512, 1024, 0),
            &format!("{ATTACK}{}{}", fight_near(8), killer(256, 8, 0, 300, 0)),
        ),
        unit_in(
            SHARPSHOOTER,
            "SID_Scharfschuetze",
            CATEGORY,
            &walk(0, 4_471_040, 512, 1024, 0),
            &killer(512, 0, 1, 0, 300),
        ),
        unit_in(
            SMITH,
            "ENG_Schmied",
            CATEGORY,
            &walk(0, 4_458_752, 512, 1024, 0),
            &format!("{ATTACK}{}{SPECIAL_MOVE}", fight_near(3)),
        ),
    ]
    .concat()
}

/// Bar rows like `ability_testkit::SPELL_TOOLBAR` (slots, kinds and `REQUIRED_GUID` of the shipped data),
/// without the missionary 1113 (541) and the pistoleer 1115 (546).
const SPELL_TOOLBAR: &str = concat!(
    "GUID 1716\n",
    "BUTTONID 7\nBUTTONMSG 480\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3024\n",
    "BUTTONID 8\nBUTTONMSG 481\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3025\n",
    "BUTTONID 9\nBUTTONMSG 482\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3026\n",
    "ENDGUID\n",
    "GUID 1715\n",
    "BUTTONID 8\nBUTTONMSG 484\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3022\n",
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
    "GUID 2616\n",
    "BUTTONID 7\nBUTTONMSG 457\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3327\n",
    "ENDGUID\n",
    "GUID 2013\n",
    "BUTTONID 8\nBUTTONMSG 551\nGROUP_AND_MSG\nTARGET_MSG\nREQUIRED_GUID 3122\n",
    "BUTTONID 9\nBUTTONMSG 542\nGROUP_AND_MSG\nDO_MSG\nREQUIRED_GUID 3121\n",
    "ENDGUID\n",
);

/// `targetdefines.ini` excerpt (values of the shipped data) like `ability_testkit::SPELL_DEFINES`: without the
/// `PROP_` and `MID_PROP_` names `parse_targets` stops silently.
const SPELL_DEFINES: &str = "PROP_PTTOPT 0\nMID_PROP_PTTOPT_NOTHUMAN 131072\nPROP_STAND 2\n\
MID_PROP_STAND_ONWATER 258\nPROP_BUILDING 3\nPROP_GHOST 27\nPROP_BOMBERMAN 36\n\
MID_PROP_BOMBERMAN_CANBLOWUPBUILDING 292\nMID_PROP_BOMBERMAN_CANBLOWUPSHIP 548\nPROP_KILLER 37\n\
MID_PROP_KILLER_KILLNEAR 293\nMID_PROP_KILLER_KILLFAR 549\nPROP_SPELL 38\n\
MID_PROP_SPELL_SPELLEXPLOSION 294\nMID_PROP_SPELL_SPELLSTORM 550\nPROP_SPECIALMOVE 49\n";

/// Entries of the target buttons like `ability_testkit::SPELL_TARGETS` (shipped data line by line; 480/481
/// with `MM_METHODINDEX 3` = distance <= sight/3 of the caster), without 541 and 546.
const SPELL_TARGETS: &str = concat!(
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
);

/// `default.dat` values like `ability_testkit::SPELL_DEFAULTS` (14 hit points, 15 mana, 16 speed,
/// 18 sight; sight/3 = range of `MM_METHODINDEX 3`) and the costs of the buttons 454-556
/// (config 15 of the button GUID, re/spells 2.5, re/special-attacks 1.1).
const SPELL_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (MEDICINE_MAN, &[(14, 800), (15, 100), (16, 145), (18, 52)]),
    (CHIEF, &[(14, 1000), (15, 100), (16, 183), (18, 40)]),
    (PATRIOT, &[(14, 1500), (15, 100), (16, 227), (18, 32)]),
    (
        SHADOW_WARRIOR,
        &[(14, 1200), (15, 200), (16, 200), (18, 40)],
    ),
    (TRAPPER, &[(14, 900), (15, 150), (16, 200), (18, 52)]),
    (SHARPSHOOTER, &[(14, 800), (15, 250), (16, 200), (18, 64)]),
    (SMITH, &[(14, 1200), (15, 100), (16, 145), (18, 32)]),
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

/// Research of the spells and special attacks (`[UPGRADE]` of the project file).
const SPELL_UPGRADE_GUIDS: &[u32] = &[
    SCOUT_UPGRADE,
    EXPLOSION_UPGRADE,
    STORM_UPGRADE,
    REINCARNATION_UPGRADE,
    SHADOW_KILL_UPGRADE,
    WHIRLWIND_UPGRADE,
    DAMAGE_BUILDING_UPGRADE,
    TRAPPER_KILL_UPGRADE,
    TRAPPER_SCOUT_UPGRADE,
    DYNAMITE_BUILDING_UPGRADE,
    DYNAMITE_SHIP_UPGRADE,
    SNIPE_UPGRADE,
];

/// Time markers like `ability_testkit::SPELL_LABELS` (without missionary and pistoleer).
const SPELL_LABELS: &[AnimLabels] = &[
    (MEDICINE_MAN, 2, &[(1, 4340)]),
    (PATRIOT, 1, &[(1, 5333)]),
    (PATRIOT, 2, &[(1, 5333)]),
    (SHADOW_WARRIOR, 5, &[(1, 1000), (2, 580)]),
    (TRAPPER, 8, &[(1, 750), (2, 300), (22, 633)]),
    (SHARPSHOOTER, 1, &[(1, 3066), (3, 1333)]),
    (SMITH, 4, &[(1, 3266), (2, 1283), (22, 1843)]),
    (SMITH, 5, &[(1, 3333)]),
];

// --- Headhunter and robbing ---------------------------------------------------------------------
//
// Copied from `nml_sim::module::ability_testkit` (`hunter_only_types`, `HUNTER_TOOLBAR`, `HUNTER_DEFAULTS`,
// `HUNTER_LABELS`, `HUNTER_SHAPES`); the values from re/headhunter-robbing 1.1 and the time markers are there. Only the
// types that the viewer world does not have yet are new: 2616, 1112 and 1113 stay as they are.

/// Headhunter as in the shipped data (re/headhunter-robbing 1.1): category `0x2000000`, stand, walk,
/// dying, swimming and module 34 (near kill 350002 = `near`, throw 350003 = `far`).
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

/// Headquarters: building (category `0x2000000`) with quarters 23.
fn headquarters(guid: u32, name: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 33554432\n\
         PROPID 3\nVALUEID 40003 6 1 0\nPROPID 23\nVALUEID 240001 6 1 0\n"
    )
}

/// Robber: category `0x2800000`, walk of the shipped data, attack 17 without flags and module 48 with
/// 490001 = 256 (`CANROBDEADUNITS`) and `m_RobAnim` = `anim`.
fn robber(guid: u32, name: &str, anim: i32) -> String {
    unit_in(
        guid,
        name,
        0x280_0000,
        &walk(1, 4_458_752, 512, 1024, 0),
        &format!("{ATTACK}PROPID 48\nVALUEID 490001 6 1 256\nVALUEID 490002 1 1 {anim}\n"),
    )
}

/// The headhunter types of the viewer world (to be appended after [`spell_types`]).
fn hunter_types() -> String {
    [
        hunter(HUNTER_SPA, "Kopfgeldjaeger_SPA", 3, 4, [2, 2, 2], 1, 5),
        hunter(HUNTER_PRI, "Kopfgeldjaeger_PRI", 2, 6, [1, 5, 5], 4, 3),
        hunter(HUNTER_SID, "Kopfgeldjaeger_SID", 3, 6, [2, 5, 5], 1, 4),
        headquarters(HQ_SPA, "SPA_Hauptquartier"),
        headquarters(HQ_SID, "SID_Rathaus"),
        unit_in(
            SID_SHERIFF,
            "SID_Sheriff",
            0x280_0000,
            &walk(1, 264_448, 512, 1024, 0),
            "",
        ),
        robber(UNDERTAKER, "SID_Leichenbestatter", 3),
        robber(SANDERS, "SID_WilliamSanders", 7),
    ]
    .concat()
}

/// Bars like `ability_testkit::HUNTER_TOOLBAR`: hire 4100-4102 (`SELECTHEADHUNTER_MSG`) in
/// both HQs, button 540 (`GROUP_AND_MSG`, `DO_MSG`, `REQUIRED_GUID 3326`) for 2615 and 2702.
const HUNTER_TOOLBAR: &str = concat!(
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

/// `default.dat` values like `ability_testkit::HUNTER_DEFAULTS` (12 price of the headhunters, 13 build time,
/// 14 hit points, 15 mana, 16 speed, 18 sight, 23 capacity; costs of button 540 in config 15).
const HUNTER_DEFAULTS: &[(u32, &[(u32, i32)])] = &[
    (HUNTER_SPA, &[(12, 300), (14, 4000), (16, 254), (18, 32)]),
    (HUNTER_PRI, &[(12, 300), (14, 4000), (16, 254), (18, 40)]),
    (HUNTER_SID, &[(12, 300), (14, 5000), (16, 254), (18, 40)]),
    (HQ_SPA, &[(13, 90), (14, 4000), (15, 0), (18, 40), (23, 5)]),
    (HQ_SID, &[(13, 63), (14, 3200), (15, 0), (18, 40), (23, 5)]),
    (SID_SHERIFF, &[(14, 160), (16, 183), (18, 32)]),
    (UNDERTAKER, &[(14, 1200), (15, 150), (16, 183), (18, 32)]),
    (SANDERS, &[(14, 900), (15, 100), (16, 183), (18, 32)]),
    (540, &[(15, 10)]),
];

/// Time markers like `ability_testkit::HUNTER_LABELS`: near kill and throw of the headhunters, robbing of
/// 2615 and 2702.
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

/// Hulls of the tents (re/tents 9.2): `(GUID, center, half extent)`; the storage camp has that of the
/// dwelling tent (test value).
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
    // Alarm post (re/tents 9.2).
    (
        ALARM_POST,
        [-36.554_054, 234.338_36, -11.222_935],
        [121.663_98, 234.551_22, 97.467_606],
    ),
];

/// Rules on a 64 x 64 map (cells 128).
pub fn rules() -> Rules {
    let types = format!(
        "{TYPES}{}{}{}",
        evoke_types(),
        spell_types(),
        hunter_types()
    );
    let mut r = Rules::synthetic(&types, 64);
    for &(guid, values) in DEFAULTS
        .iter()
        .chain(EVOKE_DEFAULTS)
        .chain(SPELL_DEFAULTS)
        .chain(HUNTER_DEFAULTS)
    {
        r.defaults.insert(guid, values.iter().copied().collect());
    }
    r.upgrades.extend(EVOKE_UPGRADE_GUIDS.iter().copied());
    r.upgrades.extend(SPELL_UPGRADE_GUIDS.iter().copied());
    r.upgrades.insert(ROB_UPGRADE);
    r.upgrade_types.insert(WOLF_AURA, AURA_TYPES.to_vec());
    r.temp_upgrades.extend(
        parse_temp_upgrades(EVOKE_TEMP_UPGRADES)
            .expect("synthetic tempupgrades.ini")
            .into_iter()
            .map(|t| (t.guid, t)),
    );
    for &(guid, center, half) in SHAPES {
        set_shape(&mut r, guid, center, half);
    }
    // Gate: hull center 0, half extents (256, 50, 64) = 4 x 1 tiles; the door nodes `frei01`/`frei02`
    // are in `free_nodes` (mask) and `helper_nodes` (`tile_map::add_building`), re/arming-gates 9.C.
    let frei = [[-67.505, 0.0, 5.378], [57.336, 0.0, 5.378]];
    set_shape(&mut r, GATE, [0.0; 3], [256.0, 50.0, 64.0]);
    if let Some(shape) = r.building_shapes.get_mut(&GATE) {
        shape.free_nodes = frei.to_vec();
        shape.helper_nodes = [("frei01", frei[0]), ("frei02", frei[1])]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
    }
    r.toolbar = parse_toolbar(&format!(
        "{HUNTER_TOOLBAR}{TOOLBAR}{EVOKE_TOOLBAR}{SPELL_TOOLBAR}"
    ));
    // Headquarters: 3 x 3 tiles like the monastery, exit 300 in front of the center (outside the
    // hull; `ability_testkit::HUNTER_SHAPES`).
    for guid in [HQ_SPA, HQ_SID] {
        set_shape(&mut r, guid, [0.0; 3], [100.0, 50.0, 100.0]);
        if let Some(shape) = r.building_shapes.get_mut(&guid) {
            shape
                .helper_nodes
                .insert("ausgang".into(), [0.0, 0.0, 300.0]);
        }
    }
    r.targets = parse_targets(
        &format!("{TARGETS}{EVOKE_TARGETS}{SPELL_TARGETS}"),
        &parse_target_defines(&format!("{EVOKE_DEFINES}{SPELL_DEFINES}")),
    )
    .entries;
    r.variant_rules = parse_variant_rules(VARIANT_RULES);
    for &(guid, anim, labels) in LABELS
        .iter()
        .chain(EVOKE_LABELS)
        .chain(SPELL_LABELS)
        .chain(HUNTER_LABELS)
    {
        r.set_anim_labels(guid, anim, labels);
    }
    r.heroes = HEROES.to_vec();
    r
}

/// Building hull and model box of the type `guid` from `center` and `half`.
pub fn set_shape(r: &mut Rules, guid: u32, center: [f32; 3], half: [f32; 3]) {
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

/// Host with player 0 (human, 1000 each of gold, wood and food, living space 50), map revealed.
pub fn host() -> SimHost {
    let setup = MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: nml_sim::ki::KiSetup::default(),
        script: nml_sim::script::ScriptSetup::default(),
        ..MatchSetup::default()
    };
    let mut host = SimHost::new(rules(), setup, DEFAULT_GAME_SPEED).unwrap();
    host.active_player = 0;
    host.state.players[0].control = Control::Human;
    host.state.players[0].resources_fx = [1000 << 12, 1000 << 12, 1000 << 12, 0, 50 << 12];
    nml_sim::fog::reveal_map(&mut host.state, -1);
    host
}

/// [`host`] of the headhunter world: slot 0 nation 2 (SPA) and slot 1 nation 3 (SID, AI) play
/// (`result = 0`) and are enemies of each other (default of diplomacy); slots 2-7 stay "not in the
/// game" (`result = 3`). The tests set the gold of slot 0 explicitly.
pub fn hunter_host() -> SimHost {
    let mut host = host();
    let p0 = &mut host.state.players[0];
    p0.nation = 2;
    p0.result = 0;
    let p1 = &mut host.state.players[1];
    p1.nation = 3;
    p1.result = 0;
    p1.control = Control::Ai;
    debug_assert!(host.state.diplomacy.is_enemy(0, 1) && host.state.diplomacy.is_enemy(1, 0));
    host
}

/// Finished object (pre-placed) on the center of the tile `tile`.
pub fn spawn(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let rules = host.rules.clone();
    let mut sim = Sim::new(&mut host.state, &rules);
    let at = |t: i32| t as f32 * 128.0 + 64.0;
    let mut desc = ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]);
    desc.flags |= desc_flags::PLACED;
    create_object(&mut sim, &mut desc).expect("object created")
}

/// Construction site (not pre-placed, unfinished) on the center of the tile `tile`: building without `TOR`
/// until completion (`StartBuild`).
pub fn spawn_site(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let rules = host.rules.clone();
    let mut sim = Sim::new(&mut host.state, &rules);
    let at = |t: i32| t as f32 * 128.0 + 64.0;
    let mut desc = ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]);
    create_object(&mut sim, &mut desc).expect("construction site created")
}

/// Unlock the upgrade `guid` for the slot `slot` like `ability_testkit::unlock`: object counter +1
/// (satisfies `REQUIRED_GUID`) and the GUID to the list of applied upgrades (variant terms).
/// Called before [`spawn`], the new object has the variants 9/10; existing objects do not switch
/// over.
pub fn unlock(host: &mut SimHost, slot: u8, guid: u32) {
    host.state.add_guid_count(slot, guid, 1);
    host.state.applied_upgrades[usize::from(slot)].push(guid);
}

/// The travois carries `tent`: `0x123 {tent}` as when packing (`TransportTent.tent`, flag
/// `TRANSPORTINGTENT`); the tent itself stays as it is.
pub fn load_travois(host: &mut SimHost, travois: ObjectId, tent: ObjectId) {
    let rules = host.rules.clone();
    let mut sim = Sim::new(&mut host.state, &rules);
    let msg = Message::new(transport::msg::SET_TENT, &[tent.as_i32()]);
    assert_eq!(send(&mut sim, travois, &msg), 0, "travois takes the tent");
}

// --- AI match ------------------------------------------------------------------------------------

/// Rules of a multiplayer map with four `.lvl` slots (nation = slot) and a small
/// AI catalog: `setschwierigkeitsgrad` sets value 302 = 1000, `multi_startplayer` pushes
/// ones onto the stack endlessly (every tick changes the state), module 78 sets value 500 = 1.
pub fn ki_rules() -> nml_sim::rules::Rules {
    use nml_sim::ki::catalog::{KiCatalog, KiPrototype};
    use nml_sim::ki::start::{SET_LEVEL, START_PLAYER};
    use nml_sim::ki::vars::VarTable;
    use nml_sim::ki::vm::op;
    let word = |op: u32, operand: u32| (op << 28) | operand;
    let set_value = |k, v| {
        vec![
            word(op::PUSHA, k),
            word(op::PUSHA, v),
            word(op::CALLB, 0x1E),
        ]
    };
    let proto = |id, name: &str, code: Vec<u32>| KiPrototype {
        id,
        name: name.into(),
        params: 0,
        code,
        vars: VarTable::from_module(&[(0, 8)], &[]),
        checksum: u64::from(id),
    };
    let mut r = nml_sim::rules::Rules::synthetic(
        "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\n",
        32,
    );
    r.level_path = "data/levels/multiplayer/test.lvl".into();
    for slot in 0..4u8 {
        r.level.players.push(nml_formats::config::LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: i32::from(slot),
            kind: 0,
            resources: Vec::new(),
        });
    }
    r.ki = KiCatalog::from_prototypes([
        proto(SET_LEVEL, "setschwierigkeitsgrad", set_value(302, 1000)),
        proto(
            START_PLAYER,
            "multi_startplayer",
            vec![word(op::PUSHA, 1), word(op::JMP, 0)],
        ),
        proto(KI_MARK, "mark", set_value(500, 1)),
    ]);
    r
}

/// Module `mark` of the AI catalog of [`ki_rules`].
pub const KI_MARK: u32 = 78;

/// AI slot of `sim_setup` with nation and AI kind.
pub fn ai_slot(nation: i32, ki_kind: u8) -> nml_control::SlotSetup {
    nml_control::SlotSetup {
        kind: nml_control::SlotSetupKind::Ai,
        nation: Some(nation),
        team: None,
        color: None,
        handicap: None,
        ki_kind: Some(ki_kind),
    }
}

/// Human slot of `sim_setup` (values of the map).
pub fn human_slot() -> nml_control::SlotSetup {
    nml_control::SlotSetup {
        kind: nml_control::SlotSetupKind::Human,
        nation: None,
        team: None,
        color: None,
        handicap: None,
        ki_kind: None,
    }
}

/// Host of an AI match: slot 0 human, slot 1 AI kind 3 (nation 1), AI on. The same setup as
/// `sim_setup` with these slots in the defaults of the viewer (its `testworld_viewer::ki_host`):
/// default of the map, the slots on top, then restart via [`SimHost::apply_setup`].
pub fn ki_host() -> SimHost {
    let rules = ki_rules();
    let mut setup = MatchSetup::default_for(&rules);
    let mut host = SimHost::new(rules, setup.clone(), DEFAULT_GAME_SPEED).unwrap();
    setup.lobby[0].kind = nml_sim::setup::SlotKind::Human;
    setup.lobby[1].kind = nml_sim::setup::SlotKind::Ai;
    setup.lobby[1].nation = 1;
    setup.lobby[1].handicap = nml_sim::setup::ki_kind_handicap(3).expect("AI kind 3");
    setup.ki = nml_sim::ki::KiSetup {
        enabled: true,
        campaign_difficulty: 1,
    };
    host.apply_setup(Some(setup), DEFAULT_GAME_SPEED).unwrap();
    host
}

// --- Scripts -------------------------------------------------------------------------------------

/// Script text from blocks `(conditions, actions)` in the format of the `.abl` (conditions with prefix `|`
/// or `&` before the number, actions with `n` or `w`).
pub fn abl(blocks: &[(&[&str], &[&str])]) -> String {
    let mut text = String::new();
    for (i, (conditions, actions)) in blocks.iter().enumerate() {
        text += &format!("{{\r\n\t[BESCHREIBUNG=block{i}]\r\n");
        for c in *conditions {
            text += &format!("\t[BEDINGUNG={c}:Text]\r\n");
        }
        for a in *actions {
            text += &format!("\t[AKTION={a}:Text]\r\n");
        }
        text += "}\r\n";
    }
    text
}

/// Rules of [`rules`] with the level script `level` (synthetic, without dialogs and areas).
pub fn script_rules(level: &str) -> Rules {
    let mut r = rules();
    r.scripts = nml_sim::script::testing::rules_with(level, &[], &[]);
    assert!(r.scripts.errors.is_empty(), "{:?}", r.scripts.errors);
    r
}

// --- Network match -------------------------------------------------------------------------------

/// Slots of the network match: host on 0, two clients on 1 and 2.
pub const NET_SLOTS: u8 = 3;

/// Tile of the soldiers of the slot `slot` in the network match (three per slot, side by side).
pub fn net_unit_tile(slot: u8, i: u8) -> (i32, i32) {
    (10 + 3 * i32::from(i), 10 + 15 * i32::from(slot))
}

/// Rules of the network match: [`rules`] as a multiplayer map with [`NET_SLOTS`] `.lvl` slots (nation 2, 1000
/// each of gold, wood and food, living space 50) and three soldiers 1102 of the slot each in the `.wld`.
/// Everything is in the rules and thus in the start package: every machine builds the same start state from
/// the same rules and the same setup, and the replay of the command log begins there.
pub fn net_rules() -> Rules {
    net_rules_with(NET_SLOTS)
}

/// [`net_rules`] with `slots` slots instead of [`NET_SLOTS`] (host migration with four machines).
pub fn net_rules_with(slots: u8) -> Rules {
    let mut r = rules();
    r.level_path = "data/levels/multiplayer/net.lvl".into();
    for slot in 0..slots {
        r.level.players.push(nml_formats::config::LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: 2,
            kind: 0,
            resources: vec![(0, 1000), (1, 1000), (2, 1000), (4, 50)],
        });
        for i in 0..3 {
            let (x, z) = net_unit_tile(slot, i);
            r.world_objects.push(nml_formats::wld::WorldObject {
                guid: SPA_SOLDIER,
                position: [x as f32 * 128.0 + 64.0, 0.0, z as f32 * 128.0 + 64.0],
                scale: [1.0; 3],
                rotation: [0.0; 3],
                color: Some(i32::from(slot)),
                name: None,
                config: Default::default(),
            });
        }
    }
    r
}

/// Setup of the network match: default of the map (mode 0, [`NET_SLOTS`] human slots), seed 7, map
/// explored, `network: true`; only via `MatchSetup` before `new_match`, no intervention in the state afterwards.
pub fn net_setup(rules: &Rules) -> MatchSetup {
    MatchSetup {
        seed: 7,
        map_explored: true,
        network: true,
        ..MatchSetup::default_for(rules)
    }
}

/// Game host of a machine of the network match in turn mode on slot `slot` (`host`: the machine is the network
/// host), from [`net_rules`] and [`net_setup`].
pub fn net_host(slot: u8, host: bool) -> SimHost {
    net_host_with(slot, host, NET_SLOTS)
}

/// [`net_host`] in [`net_rules_with`] `slots`.
pub fn net_host_with(slot: u8, host: bool, slots: u8) -> SimHost {
    let rules = net_rules_with(slots);
    let setup = net_setup(&rules);
    let mut sim = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).expect("network match");
    sim.enter_rounds(slot, host).expect("human slot");
    sim
}
