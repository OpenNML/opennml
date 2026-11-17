//! Filter IDs of the AI queries (`FUN_0047E390`, re/ki-queries 5.2). The conversion itself lives in
//! [`crate::filter::from_id`] because `economy` (and thus `player`, which the AI does not know) needs it too.

pub use crate::filter::from_id as filter_from_id;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::filter::matches;
    use crate::sim::Sim;
    use nml_formats::targets::{MatchFilter, parse_target_defines, parse_targets};

    const TEXT: &str = concat!(
        "BEGIN\nBUTTONMSG 5\nMY_MM\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ANDALL\nMM_END\nTARGET_MM\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 5\nMY_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_END\nTARGET_MM\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 183\nMY_MM\nMM_MODE2 MM2_MORETHAN70\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    );

    fn rules_with_entries() -> crate::rules::Rules {
        let mut rules = kit::rules();
        let defines = parse_target_defines(kit::DEFINES);
        rules.flowcontrol_targets = parse_targets(TEXT, &defines).entries;
        rules
    }

    #[test]
    fn filter_ids_from_1000_are_guid_filters_and_buildings_need_finished() {
        let rules = rules_with_entries();
        // A type without a building module (the type does not even have to be known).
        let f = filter_from_id(&rules, 1107);
        assert_eq!((f.mode, f.mode2, f.guid), (0x118, 0, 1107));
        assert_eq!(f.mode & 0x2, 0);
        let f = filter_from_id(&rules, kit::COLONIST as i32);
        assert_eq!(f.mode, 0x118);
        // Building type: only finished buildings (`HANDLETYPE`, `0x403`).
        let f = filter_from_id(&rules, kit::HQ as i32);
        assert_eq!((f.mode, f.match_bits, f.mode2), (0x11A, 0x403, 0));
        assert_eq!(f.guid, kit::HQ as i32);
        // Rest like `constructed()`.
        let blank = MatchFilter::constructed();
        assert_eq!(
            (f.layer, f.region, f.tether, f.next.is_none()),
            (blank.layer, blank.region, blank.tether, true)
        );
        // A finished building matches, a construction site of the same type does not (E-51).
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let done = kit::spawn(&mut sim, kit::HQ, 1, (10, 10));
        let site = kit::site(&mut sim, kit::HQ, 1, (30, 30));
        assert!(matches(&mut sim, done, &f, 1));
        assert!(!matches(&mut sim, site, &f, 1));
        // Another type does not match, nor does a foreign owner.
        let unit = kit::spawn(&mut sim, kit::COLONIST, 1, (20, 20));
        assert!(!matches(&mut sim, unit, &f, 1));
        assert!(!matches(&mut sim, done, &f, 0));
    }

    #[test]
    fn filter_ids_below_1000_copy_the_my_filter_of_the_button() {
        let rules = rules_with_entries();
        let f = filter_from_id(&rules, 5);
        let first = rules.flowcontrol_target(5).unwrap();
        assert_eq!(f, first.my);
        assert_eq!(f.mode, 0x110, "the first entry, not the second");
        assert_eq!(filter_from_id(&rules, 183).mode2, 0x800);
    }

    #[test]
    fn module_descriptors_with_mask_zero_do_not_count() {
        // 5.15.3: `ObjectType_FindModuleDesc(m, 0xFFFFFFFF)` ignores descriptors with variant mask 0 - for the
        // building branch of `filter_from_id` and the type queries.
        use super::super::query_once;
        use crate::player::Sight;
        use nml_formats::property::{ModuleDesc, ObjectType};
        let mut rules = rules_with_entries();
        let desc = |index, mask| ModuleDesc {
            index,
            mask,
            values: Default::default(),
        };
        for (guid, mask) in [(2990, 0), (2991, 4)] {
            rules.types.insert(
                guid,
                ObjectType {
                    guid,
                    name: format!("Type{guid}"),
                    values: Default::default(),
                    modules: vec![desc(3, mask), desc(8, mask), desc(13, mask)],
                },
            );
        }
        assert_eq!(filter_from_id(&rules, 2990).mode, 0x118);
        assert_eq!(filter_from_id(&rules, 2991).mode, 0x11A);
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        for code in [0x4F6F, 0x4F94, 0x4FC4] {
            for (guid, want) in [(2990, 0), (2991, 1)] {
                let req = query_once(&mut sim, 1, Sight::Omniscient, code, &[guid]).0;
                assert_eq!(req[2], want, "{code:#x} {guid}");
            }
        }
    }

    #[test]
    fn unknown_filter_ids_give_an_empty_filter_that_never_matches() {
        let rules = rules_with_entries();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = kit::spawn(&mut sim, kit::COLONIST, 1, (20, 20));
        let dead = kit::spawn(&mut sim, kit::COLONIST, 1, (22, 20));
        sim.object_mut(dead).unwrap().status |= crate::object::status::DIED;
        for id in [0, -5, 999, i32::MIN] {
            let f = filter_from_id(&rules, id);
            assert_eq!((f.mode, f.mode2), (0, 0), "id {id}");
            assert!(!matches(&mut sim, unit, &f, 1), "id {id}");
            assert!(!matches(&mut sim, dead, &f, 1), "id {id}");
        }
    }
}
