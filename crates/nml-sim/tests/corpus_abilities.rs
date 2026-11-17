//! Abilities in the shipped data: time markers of all ability animations (output), type values of
//! the camouflagers, assassins and healers (re/masking 1.1, re/healing 1.1), buttons, button kinds
//! and MY filters (re/masking 2.4, re/healing 2.4) and the value set of self-healing 555 (K-311).
//! Time markers of the evocations and banishers, type values, buttons and filters of the ghosts,
//! banishers, animal evokers, animals and their auras (re/ghosts-banish 1.1, 2.6; re/evoke-aura 1.1;
//! re/temp-upgrades 1.4) and of the plague watchers (re/ghosts-banish 3.7). Time markers, type
//! values, buttons and filters of the spells and special attacks (re/spells 1.1, 2.4, 2.5;
//! re/special-attacks 1.1, 2.5). Time markers, type values, buttons, filters, texts and sounds of
//! the headhunter and of robbing (re/headhunter-robbing 1.1-1.4).
//!
//! Read the time marker table with
//! `cargo test -p nml-sim --test corpus_abilities ability_animation_labels -- --nocapture`.

use nml_formats::gifcon::{ToolbarButton, button_kind};
use nml_formats::property::ObjectType;
use nml_formats::temp_upgrades::flags::TEMPORARY;
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};
use nml_sim::rules::Rules;
use nml_world::assets::ModelCache;
use nml_world::level::{level_paths, load_level};

/// Map of the individual checks (types, command bar and `targets.ini` are the same files for all
/// maps).
const LEVEL: &str = "data/levels/multiplayer/[2players]-battlebeach.lvl";

/// Modules of the abilities: camouflage 25, banishing 26, spell defense 27, digging in 28, healing
/// (29) and 31, 32, 34, 36-38, 48, 49.
fn is_ability_module(index: u32) -> bool {
    matches!(index, 25..=29 | 31 | 32 | 34 | 36..=38 | 48 | 49)
}

/// Rules of the map [`LEVEL`]; `None` without the shipped data.
fn corpus() -> Option<(Vfs, Rules)> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, LEVEL).unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    Some((vfs, rules))
}

/// Time marker `label` of animation `anim` of the type in the rules (like
/// `GameObject_GetAnimLabelTime`; 0 without a marker).
fn label(rules: &Rules, guid: u32, anim: i32, label: i32) -> i32 {
    rules.anim_label_time_at(guid, anim, label)
}

#[test]
fn ability_animation_labels() {
    let Some((vfs, rules)) = corpus() else {
        return;
    };
    let mut cache = ModelCache::new();
    let mut table = String::from("# guid anim id ms source\n");
    let mut types = 0;
    for (&guid, ty) in &rules.types {
        if !ty.modules.iter().any(|m| is_ability_module(m.index)) {
            continue;
        }
        types += 1;
        // The rules load time markers for all types with a loadable model (E-212); the fallback to
        // the game model remains for types without a model in the rules.
        let (labels, source): (Vec<Vec<(u32, i32)>>, &str) = match rules.anim_labels.get(&guid) {
            Some(l) => (l.clone(), "rules"),
            None => {
                let model = rules.models.get(&guid).and_then(|m| cache.get(&vfs, m));
                let labels = model.map_or_else(Vec::new, |m| {
                    m.animations
                        .iter()
                        .map(|a| {
                            a.as_ref().map_or_else(Vec::new, |a| {
                                a.labels.iter().map(|l| (l.id, l.time_ms as i32)).collect()
                            })
                        })
                        .collect()
                });
                (labels, "model")
            }
        };
        for (anim, list) in labels.iter().enumerate() {
            for (id, ms) in list {
                table += &format!("{guid} {anim} {id} {ms} {source}\n");
            }
        }
    }
    eprint!("{table}");
    assert_eq!(types, 53, "types with an ability module");
    // Camouflage, digging in/out/stab, healing trance.
    for (guid, anim, id, ms) in [
        (1402, 7, 1, 1600),
        (1410, 9, 1, 1600),
        (1500, 7, 1, 1600),
        (1403, 9, 1, 3199),
        (1404, 3, 1, 2106),
        (2306, 4, 1, 2133),
        (1407, 8, 1, 1843),
        (1407, 0, 1, 773),
        (1407, 1, 1, 1180),
        (1407, 1, 2, 640),
        (3422, 5, 1, 33),
        (3422, 4, 1, 2000),
        (2014, 7, 1, 3999),
        (2101, 7, 1, 3999),
        (2303, 3, 1, 4500),
        (2303, 6, 1, 4500),
    ] {
        assert_eq!(
            label(&rules, guid, anim, id),
            ms,
            "{guid}/{anim} marker {id}"
        );
    }
    // Evoke ghost (anim 0) and banish (10) of the ghost evokers, evoke animal (0), banish animation
    // 320004 of the other banishers, ghost 1413 (280001 = 0).
    for (guid, anim, id, ms) in [
        (1411, 0, 1, 3999),
        (1502, 0, 1, 3999),
        (1411, 10, 1, 5332),
        (1502, 10, 1, 5332),
        (1412, 0, 1, 8000),
        (1501, 0, 1, 8000),
        (1113, 6, 1, 9332),
        (2615, 7, 1, 9332),
        (2702, 3, 1, 9332),
        (1715, 6, 1, 4199),
        (1800, 6, 1, 4199),
        (2014, 6, 1, 4666),
        (2101, 6, 1, 4666),
        (2313, 4, 1, 5499),
        (1413, 0, 1, 1666),
    ] {
        assert_eq!(
            label(&rules, guid, anim, id),
            ms,
            "{guid}/{anim} marker {id}"
        );
    }
    // Spells and special attacks (re/spells 1.1, re/special-attacks 1.1): casting (1716/1802 anim 2,
    // 1113 anim 9), laying (2315/2400 anim 1 land, 2 water), knife (1714/1801 anim 5), stabbing
    // (2015/2100/2102/2701 anim 8), sniping (1115/1200 anim 9), shooting (2616 anim 1), whirlwind
    // (2013 anim 4) and hammering (2013 anim 5, only marker 1).
    let mut c4c3 = vec![
        (1716, 2, 1, 4340),
        (1802, 2, 1, 4340),
        (1113, 9, 1, 4332),
        (2616, 1, 3, 1333),
        (2616, 1, 1, 3066),
        (2013, 4, 2, 1283),
        (2013, 4, 22, 1843),
        (2013, 4, 1, 3266),
        (2013, 5, 1, 3333),
        (2013, 5, 2, 0),
    ];
    for guid in [2315, 2400] {
        c4c3.extend([(guid, 1, 1, 5333), (guid, 2, 1, 5333)]);
    }
    for guid in [1714, 1801] {
        c4c3.extend([(guid, 5, 2, 580), (guid, 5, 1, 1000)]);
    }
    for guid in [2015, 2100, 2102, 2701] {
        c4c3.extend([(guid, 8, 2, 300), (guid, 8, 22, 633), (guid, 8, 1, 750)]);
    }
    for guid in [1115, 1200] {
        c4c3.extend([(guid, 9, 3, 1627), (guid, 9, 1, 3733)]);
    }
    for (guid, anim, id, ms) in c4c3 {
        assert_eq!(
            label(&rules, guid, anim, id),
            ms,
            "{guid}/{anim} marker {id}"
        );
    }
    // Headhunter (re/headhunter-robbing 1.1): melee kill 350002 and throw 350003 of the
    // headhunters, robbing 490002 of 2615 (anim 3) and 2702 (anim 7).
    let mut c4c4 = vec![
        (4101, 4, 3, 863),
        (4101, 4, 1, 1400),
        (4101, 3, 2, 760),
        (4101, 3, 1, 1720),
        (2615, 3, 1, 2499),
        (2702, 7, 1, 2499),
    ];
    for (guid, throw) in [(4100, 5), (4102, 4)] {
        c4c4.extend([
            (guid, 1, 2, 720),
            (guid, 1, 22, 1300),
            (guid, 1, 1, 1800),
            (guid, throw, 2, 760),
            (guid, throw, 1, 1720),
        ]);
    }
    for (guid, anim, id, ms) in c4c4 {
        assert_eq!(
            label(&rules, guid, anim, id),
            ms,
            "{guid}/{anim} marker {id}"
        );
    }
    // Alarm post 1610: game model `alarm_baum.rdo` (`OBJ_GAME_FILENAME`, re/masking 1.1) with two
    // animations, both with marker 1 at 33 ms. The rules also load them for this type without a
    // stand/combat module (like `GameObject_GetAnimLabelTime`, which asks every model);
    // `m_MaskTime = max(marker, 1000)` stays 1000 ms.
    assert_eq!(
        rules.models.get(&1610).map(String::as_str),
        Some("data/r3d/pri/buildings/summer/alarmposten/alarm_baum.rdo")
    );
    assert!(rules.anim_labels.contains_key(&1610));
    assert_eq!(label(&rules, 1610, 0, 1), 33);
    assert_eq!(label(&rules, 1610, 1, 1), 33);
    for anim in [0, 1] {
        assert!(
            table.contains(&format!("1610 {anim} 1 33 rules\n")),
            "1610/{anim}"
        );
    }
    assert!(!table.contains("1610 2 "), "1610 has only two animations");
}

/// Modules of a type in file order without repetition (variants of the same module count
/// once).
fn module_list(ty: &ObjectType) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for m in &ty.modules {
        if !out.contains(&m.index) {
            out.push(m.index);
        }
    }
    out
}

/// Value `id` of module `index` as text.
fn value(ty: &ObjectType, index: u32, id: u32) -> &str {
    ty.module(index)
        .and_then(|m| m.values.get(&id))
        .map_or("", String::as_str)
}

/// Row of re/masking 1.1: GUID, module, variant mask, 260001 or 290001-290004, 260002,
/// extra energy, hard radius (0 = no walk), modules.
type MaskingRow = (
    u32,
    u32,
    u32,
    &'static str,
    &'static str,
    i32,
    i64,
    &'static [u32],
);
/// Row of re/healing 1.1: GUID, 300001, 300002, 300003-300007, HP, mana, sight, modules.
type HealingRow = (
    u32,
    &'static str,
    u32,
    [i32; 5],
    i32,
    i32,
    i32,
    &'static [u32],
);

#[test]
fn ability_type_params_match_the_analysis() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults[&guid].get(&cfg).copied();
    // re/masking 1.1: GUID, variant mask of module 25/28, 260001 or 290001-290004, 260002,
    // extra energy, walk radius 10004, modules.
    let masking: [MaskingRow; 9] = [
        (
            1402,
            25,
            0x400,
            "7",
            "0",
            200,
            512,
            &[0, 2, 16, 10, 17, 19, 22, 15, 25, 24, 40, 14],
        ),
        (
            1403,
            25,
            0x400,
            "9",
            "0",
            200,
            512,
            &[0, 2, 14, 16, 10, 19, 17, 22, 24, 25, 15, 40],
        ),
        // "like 1402 (18 instead of 10/19)": hunt 22 is also missing in the shipped data.
        (
            1404,
            25,
            0x400,
            "3",
            "0",
            200,
            512,
            &[2, 0, 16, 18, 17, 15, 24, 25, 40, 14],
        ),
        (
            1410,
            25,
            u32::MAX,
            "9",
            "256",
            600,
            512,
            &[2, 0, 16, 15, 25, 24, 40, 14, 17, 18],
        ),
        (
            1500,
            25,
            u32::MAX,
            "7",
            "0",
            100,
            512,
            &[0, 2, 16, 10, 17, 19, 22, 15, 25, 24, 40],
        ),
        (
            1610,
            25,
            u32::MAX,
            "0 1",
            "256",
            500,
            0,
            &[3, 42, 25, 23, 40],
        ),
        // The scout has the hard radius 256 (re/masking 1.1 gives 512 for all).
        (
            2306,
            25,
            u32::MAX,
            "4",
            "0",
            200,
            256,
            &[0, 2, 13, 16, 17, 18, 25],
        ),
        (
            1407,
            28,
            0x200,
            "8 0 1 250",
            "",
            0,
            512,
            &[0, 2, 16, 15, 17, 18, 28, 24, 40, 14],
        ),
        (
            3422,
            28,
            u32::MAX,
            "5 4 5 0",
            "",
            0,
            512,
            &[0, 2, 16, 17, 18, 28, 20],
        ),
    ];
    for (guid, index, mask, first, flags, energy, radius, modules) in masking {
        let ty = &rules.types[&guid];
        let m = ty
            .module(index)
            .unwrap_or_else(|| panic!("{guid}: module {index}"));
        assert_eq!(m.mask, mask, "{guid}: variant mask");
        if index == 25 {
            assert_eq!(value(ty, 25, 260001), first, "{guid}: 260001");
            assert_eq!(value(ty, 25, 260002), flags, "{guid}: 260002");
        } else {
            let params: Vec<&str> = (290_001..=290_004).map(|id| value(ty, 28, id)).collect();
            assert_eq!(params.join(" "), first, "{guid}: 290001-290004");
        }
        assert_eq!(config(guid, 15), Some(energy), "{guid}: extra energy");
        if radius != 0 {
            assert_eq!(
                ty.module(0).and_then(|w| w.value_int(10004)),
                Some(radius),
                "{guid}: hard radius"
            );
        }
        assert_eq!(module_list(ty), modules, "{guid}: modules");
    }
    // re/healing 1.1: 300001-300008, HP, mana, sight; all without a variant mask.
    let hef = "data/effects/healing_circle.hef";
    let healing: [HealingRow; 10] = [
        (
            2014,
            "7",
            0x3400,
            [7, 2, 0, 7, 288],
            1000,
            950,
            32,
            &[2, 0, 16, 17, 19, 10, 31, 29],
        ),
        (
            2101,
            "7",
            0x3400,
            [7, 2, 0, 7, 180],
            800,
            950,
            32,
            &[2, 0, 16, 17, 19, 10, 31, 29, 40],
        ),
        (
            2303,
            "6",
            0x3400,
            [3, 5, 4, 7, 288],
            200,
            950,
            40,
            &[0, 2, 16, 29, 17, 18],
        ),
        (
            1015,
            "0",
            0x1500,
            [15, 0, 0, 0, 180],
            3000,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            1911,
            "0",
            0x1500,
            [15, 0, 0, 0, 180],
            2700,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            2509,
            "0",
            0x1500,
            [15, 0, 0, 0, 180],
            1500,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            1008,
            "1108 1109 1117",
            0x1700,
            [0, 0, 0, 0, 600],
            3000,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            1908,
            "2009 2010 2017",
            0x1700,
            [0, 0, 0, 0, 600],
            2400,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            2208,
            "2310",
            0x1700,
            [0, 0, 0, 0, 600],
            2100,
            0,
            40,
            &[3, 29, 40],
        ),
        (
            2508,
            "2610",
            0x1700,
            [0, 0, 0, 0, 600],
            2400,
            0,
            40,
            &[3, 29, 40],
        ),
    ];
    for (guid, remove, flags, [establish, die, stand, heal, points], hp, mana, sight, modules) in
        healing
    {
        let ty = &rules.types[&guid];
        let m = ty.module(29).unwrap_or_else(|| panic!("{guid}: module 29"));
        assert_eq!(m.mask, u32::MAX, "{guid}: variant mask");
        assert_eq!(value(ty, 29, 300001), remove, "{guid}: 300001");
        let int = |id| m.value_int(id).map(|v| v as i32);
        assert_eq!(int(300002), Some(flags as i32), "{guid}: 300002");
        assert_eq!(
            [300_003, 300_004, 300_005, 300_006, 300_007].map(int),
            [establish, die, stand, heal, points].map(Some),
            "{guid}: 300003-300007"
        );
        assert_eq!(value(ty, 29, 300008), hef, "{guid}: 300008");
        assert_eq!(
            (config(guid, 14), config(guid, 15), config(guid, 18)),
            (Some(hp), Some(mana), Some(sight)),
            "{guid}: HP, mana, sight"
        );
        assert_eq!(module_list(ty), modules, "{guid}: modules");
    }
    // Targets of the test world (`ability_testkit`): hit points of the shipped data.
    for (guid, hp) in [(1100, 160), (1107, 190), (2302, 100), (1108, 1400)] {
        assert_eq!(config(guid, 14), Some(hp), "{guid}: HP");
    }
    // Variants (`upgrades.ini`) and the sheriff (re/temp-upgrades 1.4).
    let bits: Vec<(i32, Vec<i32>)> = rules
        .variant_rules
        .iter()
        .filter(|v| v.bit == 9 || v.bit == 10)
        .map(|v| (v.bit, v.terms.clone()))
        .collect();
    assert_eq!(bits, [(9, vec![2922]), (10, vec![2924])]);
    let sheriff = &rules.temp_upgrades[&4458];
    assert_eq!(sheriff.flags, 0x2D9);
    assert_eq!(config(4458, 13), Some(60));
    assert_eq!(config(4458, 115), Some(-100));
}

#[test]
fn ability_buttons_and_filters_match_the_analysis() {
    let Some((vfs, rules)) = corpus() else {
        return;
    };
    let menus = &rules.toolbar.menus;
    let tdo = button_kind::TARGET_DO;
    let ga = button_kind::GROUP_AND;
    // Menu, button, slot, kind bits, prerequisite (re/masking 2.4, re/healing 1.1/2.4).
    let expected: [(i32, i32, i32, u32, i32); 22] = [
        (1402, 434, 5, tdo, 2924),
        (1402, 435, 5, tdo, 0),
        (1403, 434, 5, tdo, 2924),
        (1403, 435, 5, tdo, 0),
        (1404, 434, 5, tdo, 2924),
        (1404, 435, 5, tdo, 0),
        (1500, 434, 5, tdo, 2924),
        (1500, 435, 5, tdo, 0),
        (2306, 434, 5, tdo, 3220),
        (2306, 435, 5, tdo, 0),
        (1407, 437, 4, tdo, 2922),
        (1407, 438, 4, tdo, 0),
        (2014, 554, 8, tdo | ga, 3125),
        (2014, 564, 8, tdo | ga, 3125),
        (2101, 554, 8, tdo | ga, 0),
        (2101, 564, 8, tdo | ga, 0),
        (2303, 458, 7, tdo | ga, 0),
        (2303, 459, 7, tdo | ga, 0),
        // K-311: only the trapper, Sanders and McBride have self-healing 555 (`DO`) - all without
        // module 29.
        (2015, 555, 8, button_kind::DO | ga, 3126),
        (2100, 555, 8, button_kind::DO | ga, 3126),
        (2102, 555, 8, button_kind::DO | ga, 3126),
        // The dig-in buttons are also in menu 431 (without a prerequisite).
        (431, 437, 6, tdo, 0),
    ];
    let buttons = [434, 435, 437, 438, 458, 459, 554, 555, 564];
    let mut found = Vec::new();
    for (&menu, list) in menus {
        for b in list.iter().filter(|b| buttons.contains(&b.msg)) {
            eprintln!(
                "Menu {menu}: button {} slot {} kind {:#X} flags {:#X} REQUIRED_GUID {:?}",
                b.msg,
                b.id,
                b.kind(),
                b.flags,
                b.required
            );
            found.push((menu, b.msg, b.id, b.flags, b.required));
        }
    }
    for (menu, msg, id, flags, required) in expected {
        assert!(
            found.contains(&(menu, msg, id, flags, [required, 0, 0, 0, 0])),
            "Menu {menu}, button {msg}"
        );
    }
    // Further occurrences: only 438 in menu 431.
    assert_eq!(found.len(), expected.len() + 1);
    assert!(found.contains(&(431, 438, 6, tdo, [0; 5])));
    // All ability buttons are of a kind that `Toolbar_IsButtonLocked` checks.
    for &(_, _, _, flags, _) in &found {
        let kind = flags & button_kind::KIND_MASK;
        assert!(kind == tdo || kind == button_kind::DO, "kind {kind:#X}");
    }
    // Ranger and alarm post have no camouflage button.
    for guid in [1410, 1610] {
        assert!(
            menus
                .get(&guid)
                .is_none_or(|l| l.iter().all(|b| b.msg != 434))
        );
    }

    // MY filters (`targets.ini`): 434 not `MASKED` and not `SWIMMING` (next filter), 435 `MASKED`,
    // 437/438 on `WAITING4KILLING` with TARGET `RETURNFALSE` (re/masking 2.4); 458/554 not `HEAL`,
    // 459/564 `HEAL` with an empty TARGET filter and `MAUSICONID 7` (re/healing 2.4 does not mention
    // the TARGET filter). The `DISABLERBUTTON` is outside the blocks (no effect). 555 has no
    // entry.
    const CHECKSTATUS: u32 = 0x1;
    const CHECKNOTSTATUS: u32 = 0x2000;
    const ANDALL: u32 = 0x10;
    const ANDNEXTMATCH: u32 = 0x1000_0000;
    const RETURNFALSE: u32 = 0x800_0000;
    const MASKED: u32 = 0x10_0000;
    const SWIMMING: u32 = 0x1_0000;
    const WAITING4KILLING: u32 = 0x40_0000;
    const HEAL: u32 = 0x8_0000;
    let filters = [
        (
            434,
            ANDNEXTMATCH | CHECKNOTSTATUS | ANDALL,
            MASKED,
            RETURNFALSE,
            0,
        ),
        (435, CHECKSTATUS, MASKED, RETURNFALSE, 0),
        (437, CHECKNOTSTATUS, WAITING4KILLING, RETURNFALSE, 0),
        (438, CHECKSTATUS, WAITING4KILLING, RETURNFALSE, 0),
        (458, CHECKNOTSTATUS, HEAL, 0, 7),
        (459, CHECKSTATUS, HEAL, 0, 7),
        (554, CHECKNOTSTATUS, HEAL, 0, 7),
        (564, CHECKSTATUS, HEAL, 0, 7),
    ];
    for (msg, mode, status, target, icon) in filters {
        let e = rules
            .target_entry(msg)
            .unwrap_or_else(|| panic!("entry {msg}"));
        assert!(e.enabled, "{msg}");
        assert_eq!(
            (e.my.mode, e.my.mode2, e.my.status),
            (mode, 0, status),
            "{msg}"
        );
        assert_eq!(
            (e.target.mode, e.target.status, e.target.next.is_none()),
            (target, 0, true),
            "{msg}: TARGET"
        );
        assert_eq!(e.mouse_icon, icon, "{msg}: MAUSICONID");
        let next =
            e.my.next
                .as_deref()
                .map(|n| (n.mode, n.status, n.next.is_none()));
        let expected_next = (msg == 434).then_some((CHECKNOTSTATUS, SWIMMING, true));
        assert_eq!(next, expected_next, "{msg}: next filter");
    }
    assert!(rules.target_entry(555).is_none());

    // K-311: value set of button GUID 555 in `default.dat` and in every slot set of all multiplayer
    // maps and the first campaign map: cost (config 15) 40. The set of slot 8 (`[NEUTRAL]`) is
    // missing; `GetTypeValueSet` then falls back to `default.dat`.
    assert_eq!(
        rules.defaults.get(&555),
        Some(&[(1, 1), (15, 40)].into_iter().collect())
    );
    let mut levels: Vec<String> = level_paths(&vfs)
        .into_iter()
        .filter(|p| p.contains("/multiplayer/"))
        .collect();
    levels.push("data/levels/kampagne1/kamp1-1/kamp1-1.lvl".into());
    assert_eq!(levels.len(), 21);
    for lvl in &levels {
        let scene = load_level(&vfs, lvl).unwrap();
        let r = Rules::load(&vfs, &scene).unwrap();
        for (slot, sets) in &r.level.slot_configs {
            let cost = sets.get(&555).and_then(|s| s.get(&15)).copied();
            eprintln!("{lvl}: slot {slot} value set 555 cost {cost:?}");
            if *slot == 8 {
                assert_eq!(cost, None, "{lvl}: slot 8");
            } else {
                assert_eq!(cost, Some(40), "{lvl}: slot {slot}");
            }
        }
        for slot in 0..=8 {
            assert_eq!(
                r.type_value_set(slot, 555)
                    .and_then(|s| s.get(&15))
                    .copied(),
                Some(40),
                "{lvl}: slot {slot}"
            );
        }
        let with_555: Vec<i32> = r
            .toolbar
            .menus
            .iter()
            .filter(|(_, l)| l.iter().any(|b| b.msg == 555))
            .map(|(&g, _)| g)
            .collect();
        assert_eq!(with_555, [2015, 2100, 2102], "{lvl}");
        for g in with_555 {
            assert!(!r.types[&(g as u32)].has_module(29), "{lvl}: {g}");
        }
    }
}

/// Types with module `index` (ascending).
fn with_module(rules: &Rules, index: u32) -> Vec<u32> {
    rules
        .types
        .iter()
        .filter(|(_, ty)| ty.has_module(index))
        .map(|(&g, _)| g)
        .collect()
}

/// Row of re/ghosts-banish 1.1 (banishers, module 31): GUID, 320003, 320004, 320005, button, mana
/// of the button, `REQUIRED_GUID`, sight (config 18).
type BanishRow = (u32, i64, i64, &'static str, i32, i32, i32, i32);

/// The ten banishers (re/ghosts-banish 1.1).
const BANNERS: [BanishRow; 10] = [
    (1113, 0xD00, 6, "bannspruch", 439, 25, 2827, 40),
    (1411, 0x500, 10, "geisterhand", 547, 25, 2931, 52),
    (1502, 0x500, 10, "geisterhand", 547, 25, 2931, 52),
    (1715, 0xD00, 6, "geister_bannen", 550, 25, 3029, 40),
    (1800, 0xD00, 6, "geister_bannen", 550, 25, 3029, 40),
    (2014, 0x500, 6, "bannspruch", 553, 200, 3124, 32),
    (2101, 0x500, 6, "bannspruch", 553, 200, 3124, 32),
    (2313, 0x500, 4, "fahnentraeger_bannen", 558, 25, 3223, 40),
    (2615, 0x500, 7, "leichenbestatter_bannen", 562, 25, 3324, 32),
    (2702, 0x500, 3, "leichenbestatter_bannen", 562, 25, 3324, 32),
];

/// Button `msg` in menu `menu`.
fn button(rules: &Rules, menu: i32, msg: i32) -> Option<&ToolbarButton> {
    rules
        .toolbar
        .menus
        .get(&menu)?
        .iter()
        .find(|b| b.msg == msg)
}

/// Types whose `OBJ_UPGRADES` in the project file of map [`LEVEL`] name the upgrade `upgrade`, in
/// file order of the `[OBJECT]` sections.
fn obj_upgrades_in_file_order(vfs: &Vfs, upgrade: u32) -> Vec<u32> {
    let scene = load_level(vfs, LEVEL).unwrap();
    let bytes = vfs
        .read(&normalize_path(&scene.level.project_file))
        .unwrap();
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    let (mut object, mut out) = (None::<u32>, Vec::new());
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            object = None;
        } else if let Some(v) = line.strip_prefix("OBJ_GUID ") {
            object = v.trim().parse().ok();
        } else if let (Some(g), Some(v)) = (object, line.strip_prefix("UPG_GUID "))
            && v.trim().parse() == Ok(upgrade)
            && out.last() != Some(&g)
        {
            out.push(g);
        }
    }
    out
}

#[test]
fn evocation_type_params_match_the_analysis() {
    let Some((vfs, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults[&guid].get(&cfg).copied();
    let cost = |b: i32| config(b as u32, 15);
    let int = |guid: u32, index: u32, id: u32| {
        rules.types[&guid]
            .module(index)
            .and_then(|m| m.value_int(id))
    };

    // re/ghosts-banish 1.1: module 26 only for 1411/1502 (270001 = 0), 27 only for ghost 1413
    // (280001 = 0), 31 for the ten banishers.
    assert_eq!(with_module(&rules, 26), [1411, 1502]);
    assert_eq!(with_module(&rules, 27), [1413]);
    for guid in [1411, 1502] {
        assert_eq!(
            value(&rules.types[&guid], 26, 270_001),
            "0",
            "{guid}: 270001"
        );
    }
    assert_eq!(value(&rules.types[&1413], 27, 280_001), "0");
    let mut banners: Vec<u32> = BANNERS.iter().map(|r| r.0).collect();
    banners.sort_unstable();
    assert_eq!(with_module(&rules, 31), banners);
    for (guid, flags, anim, node, msg, mana, required, sight) in BANNERS {
        let ty = &rules.types[&guid];
        let m = ty.module(31).unwrap();
        eprintln!(
            "Banisher {guid}: 320001 {:?} 320003 {:?} 320004 {:?} 320005 {:?} 320006 {:?} sight {:?}",
            value(ty, 31, 320_001),
            m.value_int(320_003),
            m.value_int(320_004),
            value(ty, 31, 320_005),
            value(ty, 31, 320_006),
            config(guid, 18)
        );
        assert_eq!(m.mask, u32::MAX, "{guid}: variant mask");
        assert_eq!(value(ty, 31, 320_001), "440", "{guid}: 320001");
        assert!(!m.values.contains_key(&320_002), "{guid}: 320002 missing");
        assert_eq!(m.value_int(320_003), Some(flags), "{guid}: 320003");
        assert_eq!(m.value_int(320_004), Some(anim), "{guid}: 320004");
        assert_eq!(value(ty, 31, 320_005), node, "{guid}: 320005");
        let b = button(&rules, guid as i32, msg).unwrap_or_else(|| panic!("{guid}: button {msg}"));
        assert_eq!(b.required, [required, 0, 0, 0, 0], "{guid}: REQUIRED_GUID");
        assert_eq!(cost(msg), Some(mana), "{guid}: mana {msg}");
        assert_eq!(config(guid, 18), Some(sight), "{guid}: sight");
    }

    // Evokers (re/ghosts-banish 1.1): HP, mana, sight; modules of 1411 with spell 38 (390001 = 768).
    for (guid, hp, mana) in [(1411, 900, 150), (1502, 800, 100)] {
        assert_eq!(
            (config(guid, 14), config(guid, 15), config(guid, 18)),
            (Some(hp), Some(mana), Some(52)),
            "{guid}: HP, mana, sight"
        );
        assert_eq!(int(guid, 38, 390_001), Some(768), "{guid}: 390001");
    }
    assert_eq!(
        module_list(&rules.types[&1411]),
        [2, 0, 16, 26, 15, 38, 40, 17, 18, 14, 31]
    );
    // 436 "Evoke ghost": mana 25.
    assert_eq!(cost(436), Some(25));

    // Ghost 1413: HP 250, mana 0, ranged 0, sight 40, range 8; floating walk (0x1900), dying with
    // flags and animation 0, attack FIGHTFAR, FightFar.
    let ghost = &rules.types[&1413];
    eprintln!(
        "Ghost 1413: category {:#X}, config {:?}",
        ghost.category(),
        rules.defaults[&1413]
    );
    assert_eq!(module_list(ghost), [0, 2, 16, 27, 17, 19]);
    assert_eq!(
        [14, 15, 18, 20, 21].map(|c| config(1413, c)),
        [Some(250), Some(0), Some(40), Some(0), Some(8)]
    );
    assert_eq!(int(1413, 0, 10_003), Some(0x1900));
    assert_eq!(int(1413, 16, 170_001).unwrap_or(0), 0);
    assert_eq!(int(1413, 16, 170_002).unwrap_or(0), 0);
    assert_eq!(int(1413, 17, 180_001), Some(0x800));

    // re/evoke-aura 1.1: animal evokers 1412/1501 (330001 = 0, 330002 = 0).
    assert_eq!(with_module(&rules, 32), [1412, 1501]);
    for (guid, hp, w40) in [(1412, 1200, 100), (1501, 800, 350)] {
        let ty = &rules.types[&guid];
        assert_eq!(
            module_list(ty),
            [2, 0, 16, 15, 32, 40, 17, 18, 14, 24],
            "{guid}"
        );
        assert_eq!(value(ty, 32, 330_001), "0", "{guid}: 330001");
        assert_eq!(value(ty, 32, 330_002), "0", "{guid}: 330002");
        assert_eq!(int(guid, 40, 410_001), Some(w40), "{guid}: 410001");
        assert_eq!(
            [14, 15, 16, 18, 19].map(|c| config(guid, c)),
            [Some(hp), Some(100), Some(145), Some(52), Some(20)],
            "{guid}: HP, mana, speed, sight, melee"
        );
    }
    // Animals: modules, 210001/210002, 450001-450003, HP, melee, speed, sight.
    type AnimalRow = (
        u32,
        &'static [u32],
        i64,
        &'static str,
        &'static str,
        [i32; 4],
    );
    let animals: [AnimalRow; 3] = [
        (
            3401,
            &[0, 2, 16, 20, 21, 17, 18, 44],
            0,
            "4460",
            "data/effects/wolf.hef",
            [80, 15, 227, 20],
        ),
        (
            3402,
            &[2, 0, 20, 16, 17, 18, 44, 21],
            1,
            "4461",
            "data/effects/baer.hef",
            [120, 30, 145, 20],
        ),
        (
            3400,
            &[2, 0, 16, 17, 18, 20, 44],
            2,
            "4462",
            "data/effects/wildkatze.hef",
            [80, 10, 227, 20],
        ),
    ];
    for (guid, modules, kind, upgrade, hef, [hp, near, speed, sight]) in animals {
        let ty = &rules.types[&guid];
        eprintln!(
            "Animal {guid}: category {:#X} 210001 {:?} 450002 {:?} 450003 {:?}",
            ty.category(),
            int(guid, 20, 210_001),
            value(ty, 44, 450_002),
            value(ty, 44, 450_003)
        );
        assert_eq!(module_list(ty), modules, "{guid}: modules");
        assert_eq!(int(guid, 20, 210_001), Some(0x2100), "{guid}: 210001");
        assert_eq!(int(guid, 20, 210_002), Some(kind), "{guid}: 210002");
        assert_eq!(value(ty, 44, 450_001), upgrade, "{guid}: 450001");
        assert_eq!(value(ty, 44, 450_002).trim(), "", "{guid}: 450002 empty");
        assert_eq!(value(ty, 44, 450_003), hef, "{guid}: 450003");
        assert_eq!(
            [14, 19, 16, 18].map(|c| config(guid, c)),
            [Some(hp), Some(near), Some(speed), Some(sight)],
            "{guid}: HP, melee, speed, sight"
        );
    }
    // Buttons 441-443 (re/evoke-aura 1.1): mana 25/50/30.
    for (msg, mana) in [(441, 25), (442, 50), (443, 30)] {
        assert_eq!(cost(msg), Some(mana), "mana {msg}");
    }

    // re/temp-upgrades 1.4: 4460-4462 with flag 0x49 (not temporary), values and `OBJ_UPGRADES`.
    let obj_upgrades = [
        1402, 1401, 1411, 1403, 1405, 1400, 1407, 1404, 1412, 1410, 1501, 1502, 1500,
    ];
    for (guid, values) in [
        (4460, &[(119, 60), (120, 60)][..]),
        (4461, &[(114, 60)][..]),
        (4462, &[(116, 60)][..]),
    ] {
        let rec = &rules.temp_upgrades[&guid];
        eprintln!(
            "Upgrade {guid}: flag {:#X} effect {:?} config {:?}",
            rec.flags,
            rec.effect_path,
            rules.defaults.get(&guid)
        );
        assert_eq!(rec.flags, 0x49, "{guid}: flag");
        assert_eq!(rec.flags & TEMPORARY, 0, "{guid}: not temporary");
        for &(cfg, v) in values {
            assert_eq!(config(guid, cfg), Some(v), "{guid}: config {cfg}");
        }
        // The rules keep the types by GUID (`Project::objects` is a map); the file order is in the
        // project file itself.
        let mut sorted = obj_upgrades.to_vec();
        sorted.sort_unstable();
        assert_eq!(rules.upgrade_types[&guid], sorted, "{guid}: OBJ_UPGRADES");
        assert_eq!(
            obj_upgrades_in_file_order(&vfs, guid),
            obj_upgrades,
            "{guid}: file order"
        );
    }

    // Types of the test world `ability_testkit::evoke_rules()`: missionary 1113 (re/spells 1.1),
    // warrior 1400 (re/evoke-aura V7: melee 9) and the ranged fighters (values printed for
    // comparison). V7 gives ranged 11 for the archer 1402; the rifleman 1403 has ranged 22 in the
    // shipped data - the test world gives him the archer's 11 as a test value (1402 stays unchanged
    // there, `STEALTH_GOLDEN`).
    assert_eq!(
        [14, 15, 18].map(|c| config(1113, c)),
        [Some(1000), Some(200), Some(40)]
    );
    // Speed "Slow" 145 of the evokers, the ghost and the missionary (1412 and 1501 above).
    for guid in [1411, 1413, 1412, 1113] {
        assert_eq!(config(guid, 16), Some(145), "{guid}: speed");
    }
    for guid in [1113, 1400, 1402, 1403] {
        eprintln!("Test type {guid}: config {:?}", rules.defaults[&guid]);
    }
    assert_eq!(
        [14, 16, 19].map(|c| config(1400, c)),
        [Some(160), Some(183), Some(9)]
    );
    assert_eq!(config(1402, 20), Some(11));
    assert_eq!(
        [20, 21, 34].map(|c| config(1403, c)),
        [Some(22), Some(28), Some(4)]
    );
}

#[test]
fn evocation_buttons_and_filters_match_the_analysis() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let tdo = button_kind::TARGET_DO;
    let ga = button_kind::GROUP_AND;
    let dm = button_kind::DO;
    // Menu, button, slot, kind bits, prerequisite (re/ghosts-banish 1.1/2.6, re/evoke-aura 1.1).
    let expected: [(i32, i32, i32, u32, i32); 18] = [
        (1411, 436, 8, tdo | ga, 2930),
        (1502, 436, 8, tdo | ga, 2930),
        // 547 has no `GROUP_AND_MSG` for 1411, but does for 1502.
        (1411, 547, 9, dm, 2931),
        (1502, 547, 9, dm | ga, 2931),
        (1412, 441, 8, tdo | ga, 2926),
        (1412, 442, 9, tdo | ga, 2928),
        (1412, 443, 7, tdo | ga, 2927),
        (1501, 441, 8, tdo | ga, 2926),
        (1501, 442, 9, tdo | ga, 2928),
        (1501, 443, 7, tdo | ga, 2927),
        (1113, 439, 9, dm | ga, 2827),
        (1715, 550, 9, dm | ga, 3029),
        (1800, 550, 9, dm | ga, 3029),
        (2014, 553, 9, dm | ga, 3124),
        (2101, 553, 9, dm | ga, 3124),
        (2313, 558, 9, dm | ga, 3223),
        (2615, 562, 9, dm | ga, 3324),
        (2702, 562, 9, dm | ga, 3324),
    ];
    let buttons = [436, 439, 441, 442, 443, 547, 550, 553, 558, 562];
    let mut found = Vec::new();
    for (&menu, list) in &rules.toolbar.menus {
        for b in list.iter().filter(|b| buttons.contains(&b.msg)) {
            eprintln!(
                "Menu {menu}: button {} slot {} kind {:#X} flags {:#X} REQUIRED_GUID {:?}",
                b.msg,
                b.id,
                b.kind(),
                b.flags,
                b.required
            );
            found.push((menu, b.msg, b.id, b.flags, b.required));
        }
    }
    for (menu, msg, id, flags, required) in expected {
        assert!(
            found.contains(&(menu, msg, id, flags, [required, 0, 0, 0, 0])),
            "Menu {menu}, button {msg}"
        );
    }
    assert_eq!(found.len(), expected.len(), "further occurrences");

    // MY filter 436: `PROP_EVOKEGHOST` and not `SWIMMING` (TARGET `RETURNFALSE`); the
    // `DISABLERBUTTON` is outside the blocks (no effect).
    const CHECKPROPERTY: u32 = 0x4;
    const CHECKNOTSTATUS: u32 = 0x2000;
    const ANDALL: u32 = 0x10;
    const RETURNFALSE: u32 = 0x800_0000;
    const CHECKFOG: u32 = 0x200_0000;
    const ENEMYLIST: u32 = 0x400;
    const SWIMMING: u32 = 0x1_0000;
    let e = rules.target_entry(436).expect("entry 436");
    eprintln!("436: {e:?}");
    assert!(e.enabled);
    assert_eq!(
        (e.my.mode, e.my.mode2, e.my.prop_id, e.my.status),
        (CHECKPROPERTY | ANDALL | CHECKNOTSTATUS, 0, 26, SWIMMING)
    );
    assert!(e.my.next.is_none());
    assert_eq!(
        (e.target.mode, e.target.next.is_none()),
        (RETURNFALSE, true)
    );
    // 440 "ghost as target" (target filter 320001 of banishing): MY `RETURNFALSE`; TARGET ANDALL,
    // CHECKPROPERTY `PROP_GHOST`, CHECKFOG, ENEMYLIST.
    let e = rules.target_entry(440).expect("entry 440");
    eprintln!("440: {e:?}");
    assert!(e.enabled);
    assert_eq!((e.my.mode, e.my.next.is_none()), (RETURNFALSE, true));
    assert_eq!(
        (
            e.target.mode,
            e.target.mode2,
            e.target.prop_id,
            e.target.status
        ),
        (ANDALL | CHECKPROPERTY | CHECKFOG | ENEMYLIST, 0, 27, 0)
    );
    assert!(e.target.next.is_none());
    // The banish buttons have no entry (never grayed out).
    for msg in [439, 547, 550, 553, 558, 562] {
        assert!(rules.target_entry(msg).is_none(), "{msg}");
    }
    // Finding: in the shipped data 441-443 have the same MY filter as 436, with `PROP_EVOKEANIMAL`
    // (re/evoke-aura 2.4 only mentions "no right click").
    for msg in [441, 442, 443] {
        let e = rules.target_entry(msg).expect("entry");
        eprintln!("{msg}: {e:?}");
        assert_eq!(
            (e.my.mode, e.my.prop_id, e.my.status, e.target.mode),
            (
                CHECKPROPERTY | ANDALL | CHECKNOTSTATUS,
                32,
                SWIMMING,
                RETURNFALSE
            ),
            "{msg}"
        );
    }
}

/// Plague watchers (re/ghosts-banish 1.1/3.7, C-156, C-272 to C-275): no dying descriptor with
/// `DIES_WITH_PEST_CLOUD` (170001 `& 0x800`) and no nature descriptor with `GENERATE_PEST` (60002
/// `& 0x800`). The plague hooks in `module/dying.rs` have no effect.
#[test]
fn no_type_carries_the_plague() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let (mut dying, mut nature) = (0, 0);
    for (&guid, ty) in &rules.types {
        for m in &ty.modules {
            match m.index {
                16 => {
                    dying += 1;
                    let f = m.value_int(170_001).unwrap_or(0);
                    assert_eq!(f & 0x800, 0, "{guid}: DIES_WITH_PEST_CLOUD");
                }
                5 => {
                    nature += 1;
                    let f = m.value_int(60_002).unwrap_or(0);
                    assert_eq!(f & 0x800, 0, "{guid}: GENERATE_PEST");
                }
                _ => {}
            }
        }
    }
    eprintln!("Dying descriptors {dying}, nature descriptors {nature}");
    assert!(dying > 0 && nature > 0);
}

/// Print type values for comparison with the test world `ability_testkit::spell_rules()`
/// (category, modules, `default.dat` values).
fn print_configs(rules: &Rules, guids: &[u32]) {
    for guid in guids {
        eprintln!(
            "Type {guid}: category {:#X}, modules {:?}, config {:?}",
            rules.types[guid].category(),
            module_list(&rules.types[guid]),
            rules.defaults.get(guid)
        );
    }
}

#[test]
fn spell_type_params_match_the_analysis() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults.get(&guid).and_then(|d| d.get(&cfg).copied());
    print_configs(&rules, &[1716, 1802, 1715, 1113, 3420, 1403, 1015, 1108]);
    // re/spells 1.1: 390001 (flags) and the lists 390002 (animations), 390003 (effect indices).
    for (guid, flags, anims, effects) in [
        (1716, "768", "2", "0"),
        (1802, "768", "2", "0"),
        (1411, "768", "0 0", "0 0"),
        (1502, "768", "0 0", "0 0"),
        (1113, "0", "9 10", "1"),
    ] {
        let ty = &rules.types[&guid];
        assert_eq!(
            [390_001, 390_002, 390_003].map(|id| value(ty, 38, id)),
            [flags, anims, effects],
            "{guid}: 390001-390003"
        );
    }
    // The eleven casters of spell kind 4 have 390001 = 0; together with the five above, all types
    // with module 38.
    let art4 = [
        1112, 1201, 1714, 1801, 2013, 2313, 2314, 2402, 2614, 2615, 2702,
    ];
    for guid in art4 {
        assert_eq!(
            value(&rules.types[&guid], 38, 390_001),
            "0",
            "{guid}: 390001"
        );
    }
    let mut casters: Vec<u32> = art4.to_vec();
    casters.extend([1716, 1802, 1411, 1502, 1113]);
    casters.sort_unstable();
    assert_eq!(with_module(&rules, 38), casters, "types with module 38");
    // HP, mana, sight (re/spells 1.1).
    for (guid, hp, mana, sight) in [
        (1716, 800, 100, 52),
        (1802, 800, 100, 52),
        (1113, 1000, 200, 40),
    ] {
        assert_eq!(
            [14, 15, 18].map(|c| config(guid, c)),
            [Some(hp), Some(mana), Some(sight)],
            "{guid}: HP, mana, sight"
        );
    }
    assert_eq!(config(1715, 15), Some(100), "1715: mana");
    // HP, mana, speed, sight of the types of the test world `spell_rules()` (sight/3 is the range
    // of `MM_METHODINDEX 3`).
    for (guid, values) in [
        (1716, [800, 100, 145, 52]),
        (1715, [1000, 100, 183, 40]),
        (1113, [1000, 200, 145, 40]),
        (3420, [200, 0, 227, 20]),
    ] {
        assert_eq!(
            [14, 15, 16, 18].map(|c| config(guid, c)),
            values.map(Some),
            "{guid}: HP, mana, speed, sight"
        );
    }
    // Targets of the test world with test values: monastery 1015 HP 3000 (world 10,000), ship 1108
    // HP 1400 (world 10,000), rifleman 1403 HP 250 (world 1000); sight as in the world.
    for (guid, hp, sight) in [(1015, 3000, 40), (1108, 1400, 40), (1403, 250, 40)] {
        assert_eq!(
            [14, 18].map(|c| config(guid, c)),
            [Some(hp), Some(sight)],
            "{guid}: HP, sight"
        );
    }
    // War wolf 3420 (reincarnation, re/spells 1.1): modules 0, 2, 16, 17, 18, 20, 21, animal flags
    // 0x2100 (not `IS_EVOKED`), HP 200, category 0x2000000 (without 0x400: `create_object` keeps
    // the slot of the caster, re/spells 4).
    let wolf = &rules.types[&3420];
    let mut modules = module_list(wolf);
    modules.sort_unstable();
    assert_eq!(modules, [0, 2, 16, 17, 18, 20, 21]);
    assert_eq!(
        wolf.module(20).and_then(|m| m.value_int(210_001)),
        Some(0x2100)
    );
    assert_eq!(config(3420, 14), Some(200));
    assert_eq!(wolf.category(), 0x200_0000);
    assert_eq!(wolf.category() & 0x400, 0, "not neutral");
}

#[test]
fn special_attack_type_params_match_the_analysis() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults.get(&guid).and_then(|d| d.get(&cfg).copied());
    let killers = [1115, 1200, 1202, 1714, 1801, 2015, 2100, 2102, 2616, 2701];
    print_configs(&rules, &[2315, 2400, 2013]);
    print_configs(&rules, &killers);
    // Module 36 (re/special-attacks 1.1): 370001-370006 for 2315 and 2400.
    assert_eq!(with_module(&rules, 36), [2315, 2400]);
    for guid in [2315, 2400] {
        let ty = &rules.types[&guid];
        assert_eq!(
            [370_001, 370_002, 370_003, 370_004, 370_005, 370_006].map(|id| value(ty, 36, id)),
            [
                "768",
                "data/r3d/misc/Tools/Dynamitbuendel/dynamitbuendel.rdo",
                "1",
                "2",
                "2",
                "3"
            ],
            "{guid}: 370001-370006"
        );
    }
    // Module 37: 380001 flags / 380002 melee animation / 380003 ranged animation / 380004 melee
    // damage / 380005 ranged damage.
    assert_eq!(with_module(&rules, 37), killers);
    for (guid, params) in [
        (1115, [512, 0, 9, 0, 300]),
        (1200, [512, 0, 9, 0, 500]),
        (1202, [512, 0, 9, 0, 25]),
        (2616, [512, 0, 1, 0, 300]),
        (1714, [256, 5, 0, 300, 0]),
        (1801, [256, 5, 0, 500, 0]),
        (2015, [256, 8, 0, 300, 0]),
        (2100, [256, 8, 0, 500, 0]),
        (2102, [256, 8, 0, 500, 0]),
        (2701, [256, 8, 0, 500, 0]),
    ] {
        let m = rules.types[&guid].module(37).unwrap();
        assert_eq!(
            [380_001, 380_002, 380_003, 380_004, 380_005].map(|id| m.value_int(id)),
            params.map(Some),
            "{guid}: 380001-380005"
        );
    }
    // Module 49 only for the smith 2013.
    assert_eq!(with_module(&rules, 49), [2013]);
    let smith = &rules.types[&2013];
    assert_eq!(
        [value(smith, 49, 500_001), value(smith, 49, 500_002)],
        ["0", "4 5"]
    );
    // Maximum mana (config 15). Deviation from re/special-attacks 1.1 (there "-"): Grass Bird 1801
    // and McBride 2102 have 100 in the shipped data; only Silva 1202 (campaign type) has no value
    // set.
    for (guid, mana) in [
        (2315, Some(100)),
        (2400, Some(100)),
        (1115, Some(200)),
        (1200, Some(100)),
        (1202, None),
        (2616, Some(250)),
        (1714, Some(200)),
        (1801, Some(100)),
        (2015, Some(150)),
        (2100, Some(100)),
        (2102, Some(100)),
        (2701, Some(0)),
        (2013, Some(100)),
    ] {
        assert_eq!(config(guid, 15), mana, "{guid}: mana");
    }
    // HP, mana, speed, sight of the types of the test world `spell_rules()`.
    for (guid, values) in [
        (2315, [1500, 100, 227, 32]),
        (1714, [1200, 200, 200, 40]),
        (2015, [900, 150, 200, 52]),
        (1115, [1200, 200, 183, 40]),
        (2616, [800, 250, 200, 64]),
        (2013, [1200, 100, 145, 32]),
    ] {
        assert_eq!(
            [14, 15, 16, 18].map(|c| config(guid, c)),
            values.map(Some),
            "{guid}: HP, mana, speed, sight"
        );
    }
    // W-278: SID_Trapper 2701 has module 37, but mana 0 and no button.
    let special = [454, 455, 456, 457, 546, 548];
    assert!(
        rules
            .toolbar
            .menus
            .get(&2701)
            .is_none_or(|l| l.iter().all(|b| !special.contains(&b.msg))),
        "2701 without a special button"
    );
    // Module 18 (melee, bonus "melee against ranged").
    for guid in [1714, 1801, 2015, 2100, 2102, 2701, 2616, 2013] {
        assert!(rules.types[&guid].has_module(18), "{guid}: module 18");
    }
    for guid in [1115, 1200, 2315, 2400] {
        assert!(
            !rules.types[&guid].has_module(18),
            "{guid}: without module 18"
        );
    }
}

#[test]
fn spell_buttons_and_filters_match_the_analysis() {
    let Some((_, rules)) = corpus() else {
        return;
    };
    let t = button_kind::TARGET | button_kind::GROUP_AND;
    let d = button_kind::DO | button_kind::GROUP_AND;
    // Menu, button, slot, kind bits, prerequisite (re/spells 2.5, re/special-attacks 1.1).
    let expected: [(i32, i32, i32, u32, i32); 26] = [
        (1716, 480, 7, t, 3024),
        (1716, 481, 8, t, 3025),
        (1716, 482, 9, d, 3026),
        (1802, 480, 7, t, 3024),
        (1802, 481, 8, t, 3025),
        (1802, 482, 9, d, 3026),
        (1715, 484, 8, t, 3022),
        (1800, 484, 8, t, 3022),
        (1113, 541, 8, t, 2832),
        (2315, 455, 7, t, 3227),
        (2315, 454, 8, t, 3226),
        (2400, 455, 7, t, 3227),
        (2400, 454, 8, t, 3226),
        (2015, 556, 7, t, 3128),
        (2015, 456, 9, t, 3127),
        (2100, 556, 7, t, 3128),
        (2100, 456, 9, t, 3127),
        (2102, 556, 7, t, 3128),
        (2102, 456, 9, t, 3127),
        (2616, 457, 7, t, 3327),
        (1714, 548, 7, t, 3027),
        (1801, 548, 7, t, 3027),
        (1115, 546, 7, t, 2831),
        (1200, 546, 7, t, 2831),
        (2013, 551, 8, t, 3122),
        (2013, 542, 9, d, 3121),
    ];
    let buttons = [
        454, 455, 456, 457, 480, 481, 482, 484, 541, 542, 546, 548, 551, 556,
    ];
    let mut found = Vec::new();
    for (&menu, list) in &rules.toolbar.menus {
        for b in list.iter().filter(|b| buttons.contains(&b.msg)) {
            eprintln!(
                "Menu {menu}: button {} slot {} kind {:#X} flags {:#X} REQUIRED_GUID {:?}",
                b.msg,
                b.id,
                b.kind(),
                b.flags,
                b.required
            );
            found.push((menu, b.msg, b.id, b.flags, b.required));
        }
    }
    for (menu, msg, id, flags, required) in expected {
        assert!(
            found.contains(&(menu, msg, id, flags, [required, 0, 0, 0, 0])),
            "Menu {menu}, button {msg}"
        );
    }
    // Plus 457 in submenu 437 of the dug-in sharpshooter.
    assert!(found.iter().any(|f| f.0 == 437 && f.1 == 457));
    assert_eq!(found.len(), expected.len() + 1, "further occurrences");
    // Cost (config 15 of the button GUID, re/spells 2.5, re/special-attacks 1.1).
    for (msg, mana) in [
        (480, 80),
        (481, 80),
        (482, 40),
        (484, 60),
        (541, 50),
        (556, 25),
        (454, 70),
        (455, 40),
        (456, 60),
        (457, 50),
        (542, 75),
        (546, 40),
        (548, 50),
        (551, 60),
    ] {
        let cost = rules
            .defaults
            .get(&(msg as u32))
            .and_then(|s| s.get(&15))
            .copied();
        assert_eq!(cost, Some(mana), "mana {msg}");
    }

    // MY/TARGET filters (re/spells 2.4, re/special-attacks 2.5); all entries active
    // (`DISABLERBUTTON` is outside the blocks).
    const CHECKHANDLETYPE: u32 = 0x2;
    const CHECKPROPERTY: u32 = 0x4;
    const CHECKGUID: u32 = 0x8;
    const ANDALL: u32 = 0x10;
    const MYLIST: u32 = 0x100;
    const NOMANSLIST: u32 = 0x200;
    const ENEMYLIST: u32 = 0x400;
    const CHECKNOTSTATUS: u32 = 0x2000;
    const CHECKNOTHANDLETYPE: u32 = 0x4000;
    const CHECKNOTPROPERTY: u32 = 0x8000;
    const CHECKFOG: u32 = 0x200_0000;
    const ANDNEXTMATCH: u32 = 0x1000_0000;
    const ORNEXTMATCH: u32 = 0x2000_0000;
    const CALLMETHOD: u32 = 0x2;
    const ISMOUNTED: u32 = 0x2000;
    const SWIMMING: u32 = 0x1_0000;
    const NOTHUMAN: u32 = 131_072;
    // Mode, mode 2, status, match bits, module, GUID, method of a filter.
    type Words = (u32, u32, u32, u32, u32, i32, i32);
    let words = |f: &nml_formats::targets::MatchFilter| -> Words {
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
    let my_cp = CHECKPROPERTY | ANDALL | CHECKHANDLETYPE;
    let fog_cp = ENEMYLIST | CHECKFOG | ANDALL | CHECKPROPERTY;
    let kill_head = (
        fog_cp | CHECKNOTHANDLETYPE | ORNEXTMATCH,
        CALLMETHOD,
        0,
        NOTHUMAN,
        0,
        0,
        3,
    );
    let kill_next = (
        NOMANSLIST | CHECKFOG | ANDALL | CHECKPROPERTY | CHECKNOTHANDLETYPE,
        CALLMETHOD,
        0,
        NOTHUMAN,
        0,
        0,
        3,
    );
    let none = (0, 0, 0, 0, 0, 0, 0);
    let filters: [(i32, Words, Words, Option<Words>); 12] = [
        (
            454,
            (my_cp | CHECKNOTSTATUS, 0, ISMOUNTED, 292, 36, 0, 0),
            (fog_cp, 0, 0, 0, 3, 0, 0),
            None,
        ),
        (
            455,
            (my_cp, 0, 0, 548, 36, 0, 0),
            (fog_cp | CHECKHANDLETYPE, 0, 0, 258, 2, 0, 0),
            None,
        ),
        (
            456,
            (my_cp, 0, 0, 293, 37, 0, 0),
            kill_head,
            Some(kill_next),
        ),
        (
            548,
            (my_cp, 0, 0, 293, 37, 0, 0),
            kill_head,
            Some(kill_next),
        ),
        (
            457,
            (my_cp | CHECKNOTSTATUS, 0, SWIMMING, 549, 37, 0, 0),
            (fog_cp | ANDNEXTMATCH, CALLMETHOD, 0, 0, 0, 0, 3),
            Some((CHECKNOTPROPERTY, 0, 0, 0, 27, 0, 0)),
        ),
        (
            546,
            (my_cp | CHECKNOTSTATUS, 0, SWIMMING, 549, 37, 0, 0),
            (fog_cp | ORNEXTMATCH, CALLMETHOD, 0, 0, 0, 0, 3),
            Some((
                NOMANSLIST | CHECKFOG | ANDALL | CHECKPROPERTY,
                CALLMETHOD,
                0,
                0,
                0,
                0,
                3,
            )),
        ),
        (
            480,
            (my_cp, 0, 0, 294, 38, 0, 0),
            (fog_cp, CALLMETHOD, 0, 0, 0, 0, 3),
            None,
        ),
        (
            481,
            (my_cp, 0, 0, 550, 38, 0, 0),
            (fog_cp, CALLMETHOD, 0, 0, 3, 0, 3),
            None,
        ),
        (484, (MYLIST, 0, 0, 0, 0, 0, 0), none, None),
        (556, (MYLIST, 0, 0, 0, 0, 0, 0), none, None),
        (
            541,
            (CHECKGUID, 0, 0, 0, 0, 1113, 0),
            (ANDALL | CHECKPROPERTY | ENEMYLIST, 0, 0, 0, 0, 0, 0),
            None,
        ),
        (
            551,
            (CHECKPROPERTY, 0, 0, 0, 49, 0, 0),
            (fog_cp, 0, 0, 0, 3, 0, 0),
            None,
        ),
    ];
    for (msg, my, target, next) in filters {
        let e = rules
            .target_entry(msg)
            .unwrap_or_else(|| panic!("entry {msg}"));
        eprintln!("{msg}: {e:?}");
        assert!(e.enabled, "{msg}");
        assert_eq!(words(&e.my), my, "{msg}: MY");
        assert!(e.my.next.is_none(), "{msg}: MY without next filter");
        assert_eq!(words(&e.target), target, "{msg}: TARGET");
        let n = e.target.next.as_deref();
        assert_eq!(n.map(words), next, "{msg}: next filter");
        assert!(n.is_none_or(|n| n.next.is_none()), "{msg}: one next stage");
    }
    for msg in [482, 542] {
        assert!(rules.target_entry(msg).is_none(), "{msg} without an entry");
    }
    // K-315 (open, hypothesis): file order of the right-click entries - attack (411) and "Go to"
    // (448) come before 454, 480, 484 and 541.
    let index = |msg: i32| rules.targets.iter().position(|e| e.button_msg == msg);
    let order: Vec<(i32, Option<usize>)> = [411, 448, 454, 480, 484, 541]
        .into_iter()
        .map(|m| (m, index(m)))
        .collect();
    eprintln!("File order (button, index): {order:?}");
    let first = |msg| index(msg).unwrap_or_else(|| panic!("entry {msg}"));
    for late in [454, 480, 484, 541] {
        assert!(
            first(411) < first(late) && first(448) < first(late),
            "{late}"
        );
    }
}

/// Speed (config 16) of the headhunters, "Very fast" (re/headhunter-robbing 1.1; value from the
/// shipped data).
const HUNTER_TEMPO: i32 = 254;

/// The six headquarters with the hire buttons (re/headhunter-robbing 1.1).
const HEADQUARTERS: [u32; 6] = [1000, 1300, 1600, 1900, 2200, 2500];

/// Text table `path` of the shipped data (`number=text`).
fn text_table(vfs: &Vfs, path: &str) -> std::collections::BTreeMap<i32, String> {
    use nml_formats::messages::{decode_text_file, parse_text_table};
    parse_text_table(&decode_text_file(&vfs.read(path).unwrap()))
}

#[test]
fn hunter_type_params_match_the_analysis() {
    let Some((vfs, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults.get(&guid).and_then(|d| d.get(&cfg).copied());
    print_configs(
        &rules,
        &[4100, 4101, 4102, 1000, 2500, 1112, 2614, 1100, 1015],
    );
    // Module 34 only for the three headhunters.
    assert_eq!(with_module(&rules, 34), [4100, 4101, 4102]);
    // re/headhunter-robbing 1.1: modules, 350001-350003, HP, sight; speed "Very fast".
    for (guid, params, hp, sight) in [
        (4100, ["0", "1", "5"], 4000, 32),
        (4101, ["0", "4", "3"], 4000, 40),
        (4102, ["0", "1", "4"], 5000, 40),
    ] {
        let ty = &rules.types[&guid];
        assert_eq!(module_list(ty), [2, 0, 16, 15, 34], "{guid}: modules");
        assert_eq!(value(ty, 16, 170_001), "0", "{guid}: 170001");
        assert_eq!(
            [350_001, 350_002, 350_003].map(|id| value(ty, 34, id)),
            params,
            "{guid}: 350001-350003"
        );
        assert_eq!(value(ty, 0, 10_003), "801024", "{guid}: 10003");
        for index in [17, 18, 19] {
            assert!(!ty.has_module(index), "{guid}: without module {index}");
        }
        assert_eq!(
            [14, 16, 18].map(|c| config(guid, c)),
            [Some(hp), Some(HUNTER_TEMPO), Some(sight)],
            "{guid}: HP, speed, sight"
        );
        // Price: config 12 of the value set at slot 8 (re/headhunter-robbing 3.1).
        assert_eq!(
            rules
                .type_value_set(8, guid)
                .and_then(|s| s.get(&12))
                .copied(),
            Some(300),
            "{guid}: config 12 at slot 8"
        );
        assert_eq!(ty.category(), 0x200_0000, "{guid}: category");
    }
    // The six HQs: hire buttons (kind `SELECTHEADHUNTER`, without a prerequisite) and quarters
    // (module 23).
    for hq in HEADQUARTERS {
        let list = &rules.toolbar.menus[&(hq as i32)];
        let hire: Vec<(i32, i32, u32, i32)> = list
            .iter()
            .filter(|b| b.kind() == button_kind::SELECTHEADHUNTER)
            .map(|b| (b.msg, b.id, b.flags, b.required[0]))
            .collect();
        eprintln!("HQ {hq}: hiring {hire:?}");
        assert_eq!(
            hire,
            [
                (4100, 2, button_kind::SELECTHEADHUNTER, 0),
                (4101, 3, button_kind::SELECTHEADHUNTER, 0),
                (4102, 4, button_kind::SELECTHEADHUNTER, 0),
            ],
            "HQ {hq}"
        );
        let ty = &rules.types[&hq];
        assert!(
            ty.has_module(3) && ty.has_module(23),
            "HQ {hq}: modules 3, 23"
        );
        assert_eq!(value(ty, 23, 240_001), "0", "HQ {hq}: 240001");
        eprintln!(
            "HQ {hq}: config 13 {:?} 14 {:?} 18 {:?} 23 {:?}",
            config(hq, 13),
            config(hq, 14),
            config(hq, 18),
            config(hq, 23)
        );
    }
    // Only the HQs carry the buttons 4100-4102.
    let mut with_hire: Vec<i32> = rules
        .toolbar
        .menus
        .iter()
        .filter(|(_, l)| l.iter().any(|b| (4100..=4102).contains(&b.msg)))
        .map(|(&g, _)| g)
        .collect();
    with_hire.sort_unstable();
    assert_eq!(with_hire, HEADQUARTERS.map(|g| g as i32));
    // Build time, HP, sight, capacity of the HQs of the test world `hunter_rules()`; the colonist
    // 1100 is the model for the elites without values of their own (HP, speed, sight).
    for (guid, cfgs, values) in [
        (1000, [13, 14, 18, 23], [90, 4000, 40, 5]),
        (2500, [13, 14, 18, 23], [63, 3200, 40, 5]),
        (1100, [14, 16, 18, 15], [160, 183, 32, 0]),
    ] {
        assert_eq!(cfgs.map(|c| config(guid, c)), values.map(Some), "{guid}");
    }
    // Color buttons 470-477 (target player) and "Back" 409 are named in `guids.deu`.
    let texts = text_table(&vfs, "data/txt/guids.deu");
    for guid in (470..=477).chain([409, 4100, 4101, 4102]) {
        let t = texts.get(&guid);
        eprintln!("guids.deu {guid}: {t:?}");
        assert!(t.is_some_and(|t| !t.trim().is_empty()), "guids.deu {guid}");
    }
}

#[test]
fn robbing_type_params_match_the_analysis() {
    let Some((vfs, rules)) = corpus() else {
        return;
    };
    let config = |guid: u32, cfg: u32| rules.defaults.get(&guid).and_then(|d| d.get(&cfg).copied());
    print_configs(&rules, &[2615, 2702]);
    // Module 48 only for 2615 and 2702 (re/headhunter-robbing 1.1), with the attack module 17.
    assert_eq!(with_module(&rules, 48), [2615, 2702]);
    // Module list, 490002, max mana (config 15) and the values of the test world (HP, speed, sight).
    for (guid, anim, mana, modules) in [
        (2615, "3", 150, &[0, 2, 16, 48, 10, 17, 19, 44, 31, 38][..]),
        (
            2702,
            "7",
            100,
            &[0, 2, 16, 48, 10, 17, 19, 31, 38, 40, 13, 44][..],
        ),
    ] {
        let ty = &rules.types[&guid];
        assert_eq!(module_list(ty), modules, "{guid}: modules");
        assert_eq!(
            [value(ty, 48, 490_001), value(ty, 48, 490_002)],
            ["256", anim],
            "{guid}: 490001/490002"
        );
        assert!(
            ty.has_module(17) && !ty.has_module(15),
            "{guid}: 17 yes, 15 no"
        );
        assert_eq!(value(ty, 0, 10_003), "4458752", "{guid}: 10003");
        assert_eq!(ty.category(), 0x280_0000, "{guid}: category");
        assert_eq!(config(guid, 18), Some(32), "{guid}: sight");
        assert_eq!(config(guid, 15), Some(mana), "{guid}: max mana");
        assert_eq!(config(guid, 16), Some(183), "{guid}: speed");
    }
    assert_eq!(
        [config(2615, 14), config(2702, 14)],
        [Some(1200), Some(900)]
    );
    // Button 540 (slot 7, `GROUP_AND | DO`, `REQUIRED_GUID 3326`) only for 2615 and 2702, mana 10,
    // without an entry in `targets.ini` (MY filter "no entry = satisfied").
    let mut found = Vec::new();
    for (&menu, list) in &rules.toolbar.menus {
        for b in list.iter().filter(|b| b.msg == 540) {
            eprintln!(
                "Menu {menu}: button 540 slot {} flags {:#X} REQUIRED_GUID {:?}",
                b.id, b.flags, b.required
            );
            found.push((menu, b.id, b.flags, b.required[0]));
        }
    }
    found.sort_unstable();
    let d = button_kind::DO | button_kind::GROUP_AND;
    assert_eq!(found, [(2615, 7, d, 3326), (2702, 7, d, 3326)]);
    assert_eq!(config(540, 15), Some(10), "mana 540");
    assert!(rules.target_entry(540).is_none(), "540 without an entry");
    // Corpse filter names (`targetdefines.ini`).
    let defines = nml_formats::targets::parse_target_defines(&nml_formats::text::decode_cp1252(
        &vfs.read(nml_formats::targets::TARGETDEFINES_PATH).unwrap(),
    ));
    for (name, v) in [
        ("PROP_HEADHUNTER", 34),
        ("PROP_ROBBING", 48),
        ("MID_PROP_DIE_CANROBBED", 528),
        ("MID_PROP_ROBBING_CANROBDEADUNITS", 304),
        ("MID_PROP_ROBBING_CANROBNOTHUMANUNITS", 560),
        ("MID_PROP_DIE_RESOURCESCANROBBED", 1040),
    ] {
        assert_eq!(defines.lookup(name), Some(v), "{name}");
    }
    // Messages (`texte.deu`) and 2D sounds (`otherevents.ini`) of the headhunter.
    let texts = text_table(&vfs, nml_formats::messages::TEXTS_PATH);
    for id in [10002, 10010, 10011] {
        let t = texts.get(&id);
        eprintln!("texte.deu {id}: {t:?}");
        assert!(t.is_some_and(|t| !t.trim().is_empty()), "texte.deu {id}");
    }
    let events = nml_formats::sound::parse_other_events(&nml_formats::text::decode_cp1252(
        &vfs.read(nml_formats::sound::OTHER_EVENTS_PATH).unwrap(),
    ));
    for (name, sound) in [("SFX_HEADHUNTERSTART", 923), ("SFX_HEADHUNTERSUCCESS", 924)] {
        eprintln!("otherevents.ini {name}: {:?}", events.get(name));
        assert_eq!(events.get(name).copied(), Some(sound), "{name}");
    }
}
