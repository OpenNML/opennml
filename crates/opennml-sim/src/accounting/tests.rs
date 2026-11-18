//! Tests of the statistics log (re/accounting): write path, evaluation, filling at setup, savegame and
//! the call sites of the modules in the combat and economy world ([`crate::combat::testkit`]).

use opennml_formats::config::LevelPlayer;

use super::*;
use crate::combat::testkit::{SABRE, rules as combat_rules, spawn, state as combat_state};
use crate::command::testkit::{COLONIST, HOUSE, MARKET, MINE, RESEARCH, WALL, account, site};
use crate::command::{CommandKind, testkit::run};
use crate::economy::market_convert;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send_mut;
use crate::module::building::{self, msg as bmsg};
use crate::module::gather::{self, load};
use crate::rules::Rules;
use crate::save::{SaveGame, checksum};
use crate::setup::{LobbyEntry, MatchSetup, SlotKind, new_match, start_package};

/// Entries of a record as `(kind, value)`.
fn kinds(state: &SimState, slot: usize) -> Vec<(i32, i32)> {
    state.accounting.records[slot]
        .entries
        .iter()
        .map(|e| (e.kind, e.parm))
        .collect()
}

/// Records 0 ... 3 active (name "P<slot>", color = team = slot).
fn activate(state: &mut SimState) {
    for p in 0..4 {
        state.accounting.init_record(p, &format!("P{p}"), p, 0, p);
    }
}

// QUIRK O-176: kept
#[test]
fn inactive_or_foreign_slots_are_ignored() {
    let rules = Rules::synthetic("", 16);
    let mut state = SimState::new(0, 0);
    state.tick = 17;
    let mut sim = Sim::new(&mut state, &rules);
    // Inactive record: nothing.
    add_statistic(&mut sim, 0, kind::UNIT_LOST, 1100);
    assert!(sim.state.accounting.records[0].entries.is_empty());
    sim.state.accounting.init_record(0, "A", 0, 0, 0);
    sim.state.accounting.init_record(7, "B", 7, 1, 7);
    for slot in [-1, 8, 9, 31, i32::MIN, 1] {
        add_statistic(&mut sim, slot, kind::UNIT_KILLED, 5);
    }
    assert!(
        sim.state
            .accounting
            .records
            .iter()
            .all(|r| r.entries.is_empty())
    );
    add_statistic(&mut sim, 0, kind::GOLD, 40);
    sim.state.tick = 18;
    add_statistic(&mut sim, 7, kind::TRADE, -3);
    assert_eq!(
        sim.state.accounting.records[0].entries,
        [AccountEntry {
            kind: kind::GOLD,
            time: 17,
            parm: 40
        }]
    );
    assert_eq!(
        sim.state.accounting.records[7].entries,
        [AccountEntry {
            kind: kind::TRADE,
            time: 18,
            parm: -3
        }]
    );
    // Filling the same slot again empties its list (`FUN_004031E0`, `RemoveAll`).
    sim.state.accounting.init_record(0, "A2", 1, 2, 3);
    let r = &sim.state.accounting.records[0];
    assert!(r.active && r.entries.is_empty());
    assert_eq!(
        (r.name.as_str(), r.color, r.nation, r.team),
        ("A2", 1, 2, 3)
    );
}

// QUIRK O-163, O-176, O-177, O-178: kept
#[test]
fn table_counts_and_sums_without_time_filter() {
    let mut state = SimState::new(0, 0);
    let a = &mut state.accounting;
    a.init_record(0, "", 4, 0, 0);
    a.init_record(2, "Two", 2, 1, 5);
    a.init_record(5, "Five", 9, 0, 5);
    // Slot 1 inactive with entries (only from a savegame): not in the table.
    a.records[1].entries.push(AccountEntry {
        kind: 0,
        time: 0,
        parm: 0,
    });
    for (kind, time, parm) in [
        (0, 1, 1100),
        (0, 999_999, 1101),
        (4, 3, 1100),
        (7, 2, 1001),
        (3, 4, 7),
        (8, 5, 5),
        (8, 6, -2),
        (11, 7, i32::MAX),
        (11, 8, 2),
        (12, 9, 50),
        (-1, 9, 50),
    ] {
        a.add(0, kind, time, parm);
    }
    a.add(2, 10, 0, 300);
    let rows = accounting_table(&state, "Computer");
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        AccountingRow {
            name: "Computer".into(),
            color: 4,
            // Kind 3 counts (no writer in the original, here from the test); 12 and -1 nowhere;
            // sum 11 wraps like `i32`.
            values: [2, 0, 0, 1, 1, 0, 0, 1, 3, 0, 0, i32::MIN + 1],
        }
    );
    assert_eq!(rows[1].name, "Two");
    assert_eq!(rows[1].values, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 300, 0]);
    // Color without range check, order of the slots.
    assert_eq!((rows[2].name.as_str(), rows[2].color), ("Five", 9));
    assert_eq!(rows[2].values, [0; COLUMNS]);
}

// QUIRK O-174: kept
#[test]
fn dummy_29320_names_are_skipped() {
    for name in ["29320", "29320 Dummy", "  29320", "+29320x", "\t029320"] {
        assert!(is_dummy_name(name), "{name}");
        let mut a = Accounting::default();
        a.init_record(3, name, 3, 0, 3);
        assert!(!a.records[3].active, "{name}");
    }
    for name in ["", "293201", "29321", "Spieler 29320", "-29320", "x29320"] {
        assert!(!is_dummy_name(name), "{name}");
        let mut a = Accounting::default();
        a.init_record(3, name, 3, 0, 3);
        assert!(a.records[3].active, "{name}");
    }
    // Via the setup: the placeholder of the campaign gets no record.
    let rules = campaign_rules(&[(0, "29320 RED", 0), (1, "Gegner", 1)]);
    let state = new_match(&rules, &MatchSetup::default()).unwrap();
    let active: Vec<bool> = state.accounting.records.iter().map(|r| r.active).collect();
    assert_eq!(
        active,
        [false, true, false, false, false, false, false, false]
    );
}

// QUIRK O-175: deviation
#[test]
fn names_are_cut_to_39_characters() {
    let long: String = "ä".repeat(45);
    let mut a = Accounting::default();
    a.init_record(0, &long, 0, 0, 0);
    assert_eq!(a.records[0].name.chars().count(), 39);
    a.set_local_name(0, &"x".repeat(40));
    assert_eq!(a.records[0].name, "x".repeat(39));
}

/// Rules of a campaign map with `.lvl` players `(slot, name, PLAYER_TYPE)`; nation = slot % 6.
fn campaign_rules(players: &[(u8, &str, i32)]) -> Rules {
    let mut r = crate::command::testkit::rules();
    r.level.players = players
        .iter()
        .map(|&(slot, name, kind)| LevelPlayer {
            slot,
            name: name.into(),
            nation: i32::from(slot % 6),
            kind,
            resources: Vec::new(),
        })
        .collect();
    r
}

// QUIRK O-173: kept
#[test]
fn local_player_carries_the_profile_name() {
    // Campaign: records from the `.lvl` with color = team = slot (O-173), nation from `PLAYER_PLAYER`; the
    // first human (control kind 1, `GameEngine_FindNthHumanPlayerSlot(0)`) gets the profile name.
    let rules = campaign_rules(&[(1, "KI", 1), (2, "Held", 0), (3, "Second", 0)]);
    let setup = MatchSetup {
        local_name: Some("Profil".into()),
        ..MatchSetup::default()
    };
    let state = new_match(&rules, &setup).unwrap();
    let r = &state.accounting.records;
    assert!(!r[0].active);
    assert_eq!(
        [&r[1], &r[2], &r[3]].map(|r| (r.name.as_str(), r.color, r.nation, r.team, r.active)),
        [
            ("KI", 1, 1, 1, true),
            ("Profil", 2, 2, 2, true),
            ("Second", 3, 3, 3, true)
        ]
    );
    // Without a profile the name of the map stays.
    let state = new_match(&rules, &MatchSetup::default()).unwrap();
    assert_eq!(state.accounting.records[2].name, "Held");
    // Free play: lobby values (color, nation, team), no profile name (`FUN_00403F50` only with game type
    // < 0); closed entries (in the reimplementation only fillers of the railroad lobby) create no record.
    let entry = |kind, name: &str, nation, team, color| LobbyEntry {
        kind,
        name: name.into(),
        nation,
        team,
        color,
        handicap: 0,
    };
    let setup = MatchSetup {
        mode: 0,
        lobby: vec![
            entry(SlotKind::Human, "Ich", 4, 0, 5),
            entry(SlotKind::Closed, "Zu", 0, 0, 1),
            entry(SlotKind::Ai, "Computer (Rot)", 2, 1, 3),
        ],
        local_name: Some("Profil".into()),
        ..MatchSetup::default()
    };
    let state = new_match(&rules, &setup).unwrap();
    let r = &state.accounting.records;
    assert_eq!(
        [&r[1], &r[2], &r[3]].map(|r| (r.name.as_str(), r.color, r.nation, r.team, r.active)),
        [
            ("Ich", 5, 4, 0, true),
            ("Jon Doe", 0, 0, 0, false),
            ("Computer (Rot)", 3, 2, 1, true)
        ]
    );
}

// QUIRK O-175: deviation
#[test]
fn accounting_survives_saving_and_loading() {
    let rules = campaign_rules(&[(0, "Held", 0), (1, "Gegner", 1)]);
    let setup = MatchSetup::default();
    let mut state = new_match(&rules, &setup).unwrap();
    let before = checksum(&state);
    state.accounting.add(0, kind::UNIT_TRAINED, 3, 1100);
    state.accounting.add(1, kind::TRADE, 4, -7);
    assert_ne!(checksum(&state), before, "the log is part of the checksum");
    let save = |state: SimState| SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save(state.clone()).to_bytes()).unwrap();
    assert_eq!(loaded.state.accounting, state.accounting);
    loaded.check(&rules).unwrap();
    // Kinds outside 0 ... 11 stay allowed (W-41).
    let mut odd = state.clone();
    odd.accounting.records[0].entries.push(AccountEntry {
        kind: 99,
        time: 0,
        parm: 0,
    });
    SaveGame::from_bytes(&save(odd).to_bytes()).unwrap();
}

#[test]
fn format_21_saves_are_rejected() {
    let rules = campaign_rules(&[(0, "Held", 0)]);
    let setup = MatchSetup::default();
    let state = new_match(&rules, &setup).unwrap();
    let mut bytes = SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    }
    .to_bytes();
    // Format 21 = before the statistics log.
    let old: u32 = 21;
    assert!(crate::setup::FORMAT_VERSION > old);
    bytes[8..12].copy_from_slice(&old.to_le_bytes());
    let err = SaveGame::from_bytes(&bytes).unwrap_err().to_string();
    assert_eq!(
        err,
        format!(
            "savegame has format version 21, expected {} (different simulation behavior)",
            crate::setup::FORMAT_VERSION
        )
    );
}

/// One building tick (`PropertyBuilding_Tick(100)`).
fn building_tick(sim: &mut Sim, b: ObjectId) {
    crate::module::tick(sim, b, building::INDEX, 100);
}

/// Building ticks until `done` holds (at most 1000).
fn tick_until(sim: &mut Sim, b: ObjectId, done: impl Fn(&Sim) -> bool) {
    for _ in 0..1000 {
        building_tick(sim, b);
        if done(sim) {
            return;
        }
    }
    panic!("building {b:?} does not finish");
}

// QUIRK O-171: kept
#[test]
fn kinds_follow_accounting_md_production_and_trade() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    activate(&mut state);
    state.tick = 40;
    let mut sim = Sim::new(&mut state, &rules);
    account(&mut sim, 0, [100_000, 100_000, 100_000, 0, 100]);
    // Kind 0: unit trained, at the owner of the building, `Parm` = GUID.
    let hq = spawn(&mut sim, crate::command::testkit::HQ, 0, (20, 20));
    let make = |building, guid| CommandKind::EnqueueProduction { building, guid };
    run(&mut sim, 0, make(hq, COLONIST));
    tick_until(&mut sim, hq, |s| {
        building::building(s, hq).is_some_and(|b| b.make_guid == 0)
    });
    // Kind 2: upgrade researched (`PropertyBuilding_FinishResearch`), `Parm` = upgrade GUID.
    run(&mut sim, 0, make(hq, RESEARCH));
    tick_until(&mut sim, hq, |s| s.state.applied_upgrades[0] == [RESEARCH]);
    assert_eq!(
        kinds(sim.state, 0),
        [(kind::UNIT_TRAINED, 1100), (kind::UPGRADE_RESEARCHED, 2806)]
    );
    assert!(
        sim.state.accounting.records[0]
            .entries
            .iter()
            .all(|e| e.time == 40)
    );
    // Kind 11, market: buying books the gold value **before** the price adjustment (`0x00505371`/`0x005053AE`),
    // selling 100 gold (O-171). Prices such that `Convert(price, 100)` differs before and after the
    // adjustment (4055 * 100 >> 12 = 98, 4059 * 100 >> 12 = 99).
    let market = spawn(&mut sim, MARKET, 1, (30, 40));
    account(&mut sim, 1, [100_000, 100_000, 100_000, 0, 100]);
    sim.state.economy.wood_price = 4055;
    sim.state.economy.food_price = 4055;
    for guid in [490, 491, 492, 493] {
        run(&mut sim, 1, make(market, guid));
        tick_until(&mut sim, market, |s| {
            building::building(s, market).is_some_and(|b| b.make_guid == 0)
        });
    }
    assert_eq!(market_convert(4059, 100), 99);
    assert_eq!(
        kinds(sim.state, 1),
        [
            (kind::TRADE, 98),
            (kind::TRADE, 98),
            (kind::TRADE, 100),
            (kind::TRADE, 100)
        ]
    );
}

// QUIRK O-169: kept
#[test]
fn kinds_follow_accounting_md_construction_and_destruction() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    activate(&mut state);
    let mut sim = Sim::new(&mut state, &rules);
    // Kind 1: construction finished at the owner; tracks (`TRACK`, `0x2000003`) not (O-169).
    let house = site(&mut sim, HOUSE, 0, (20, 20));
    let rails = site(&mut sim, HOUSE, 0, (30, 20));
    // Track flag also in the backup that the completion writes back.
    let track = building::flags::TRACK & 0xFFFF_FF00;
    if let Some(m) = sim.module_mut(rails, building::INDEX) {
        m.base.flags |= track;
    }
    if let Some(b) = building::building_mut(&mut sim, rails) {
        b.flags_backup |= track;
    }
    for b in [house, rails] {
        send_mut(
            &mut sim,
            b,
            &mut Message::new(bmsg::BUILD_TICK, &[1_000_000, 0]),
        );
    }
    let finished = |sim: &Sim, b| {
        sim.module(b, building::INDEX)
            .is_some_and(|m| m.base.has_flags(building::flags::CONSTRUCTED_FINISHED))
    };
    assert!(finished(&sim, house) && finished(&sim, rails));
    assert_eq!(kinds(sim.state, 0), [(kind::BUILDING_BUILT, HOUSE as i32)]);
    sim.state.accounting.records[0].entries.clear();
    // Kinds 7/5: lost at the owner, destroyed at the last attacker without an enemy check (O-168):
    // foreign, allied, own slot; without an attacker only "lost".
    let attacker = |sim: &mut Sim, slot, tile| spawn(sim, SABRE, slot, tile);
    let a1 = attacker(&mut sim, 1, (5, 5));
    let a0 = attacker(&mut sim, 0, (6, 5));
    let a9 = ObjectId::new(3, 9, 0);
    let destroy = |sim: &mut Sim, b: ObjectId, by: ObjectId| {
        sim.object_mut(b).unwrap().last_attacker = by;
        building::destroy::destroy(sim, b);
    };
    destroy(&mut sim, house, a1);
    // Second call (already dead, status bit 18): nothing.
    destroy(&mut sim, house, a1);
    let other = spawn(&mut sim, HOUSE, 0, (40, 20));
    destroy(&mut sim, other, a0);
    // Construction site counts (O-169), slot >= 8 of the attacker ID is discarded by `FUN_00403E10`.
    let mine_site = site(&mut sim, MINE, 0, (40, 40));
    destroy(&mut sim, mine_site, a9);
    let lonely = spawn(&mut sim, HOUSE, 2, (50, 20));
    destroy(&mut sim, lonely, ObjectId::NONE);
    // Palisades (`0x300003`) never.
    let wall = spawn(&mut sim, WALL, 0, (10, 40));
    destroy(&mut sim, wall, a1);
    let lost = kind::BUILDING_LOST;
    let destroyed = kind::BUILDING_DESTROYED;
    assert_eq!(
        kinds(sim.state, 0),
        [
            (lost, 1001),
            (lost, 1001),
            (destroyed, 1001),
            (lost, MINE as i32)
        ]
    );
    assert_eq!(kinds(sim.state, 1), [(destroyed, 1001)]);
    assert_eq!(kinds(sim.state, 2), [(lost, 1001)]);
}

// QUIRK O-168: kept
#[test]
fn kinds_follow_accounting_md_deaths() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    activate(&mut state);
    let (victim, ally_victim, own_victim, lonely, a1, a2) = {
        let mut sim = Sim::new(&mut state, &rules);
        (
            spawn(&mut sim, SABRE, 0, (10, 10)),
            spawn(&mut sim, SABRE, 0, (20, 10)),
            spawn(&mut sim, SABRE, 0, (30, 10)),
            spawn(&mut sim, SABRE, 3, (40, 10)),
            spawn(&mut sim, SABRE, 1, (10, 30)),
            spawn(&mut sim, SABRE, 2, (20, 30)),
        )
    };
    // Slot 2 is allied with 0: still counts as killed (O-168).
    state.diplomacy.set_friend(0, 2, true);
    state.diplomacy.set_friend(2, 0, true);
    state.diplomacy.set_enemy(0, 2, false);
    state.diplomacy.set_enemy(2, 0, false);
    for (unit, by) in [
        (victim, a1),
        (ally_victim, a2),
        (own_victim, own_victim),
        (lonely, ObjectId::NONE),
    ] {
        let o = state.object_mut(unit, false).unwrap();
        o.last_attacker = by;
        o.attributes.set_current_fraction(14, 0);
    }
    for _ in 0..8 {
        crate::tick::step(&mut state, &rules, &[]);
    }
    let (lost, killed) = (kind::UNIT_LOST, kind::UNIT_KILLED);
    let sabre = SABRE as i32;
    // Per death first 6 at the owner, then 4 at the attacker (`0x0050EE8C`, `0x0050EEB8`).
    let mut own = kinds(&state, 0);
    own.sort_unstable();
    assert_eq!(
        own,
        [(killed, sabre), (lost, sabre), (lost, sabre), (lost, sabre)]
    );
    let r0 = &state.accounting.records[0].entries;
    let pos = r0.iter().position(|e| e.kind == killed).unwrap();
    assert_eq!(r0[pos - 1].kind, lost, "own death: 6 before 4");
    assert_eq!(kinds(&state, 1), [(killed, sabre)]);
    assert_eq!(kinds(&state, 2), [(killed, sabre)]);
    assert_eq!(kinds(&state, 3), [(lost, sabre)]);
}

// QUIRK W-41, O-172: kept
#[test]
fn kinds_follow_accounting_md_delivery() {
    let rules = combat_rules();
    let mut state = combat_state(&rules);
    activate(&mut state);
    let mut sim = Sim::new(&mut state, &rules);
    let colonist = spawn(&mut sim, COLONIST, 0, (10, 10));
    let mining = gather::mining::INDEX;
    sim.state.economy.set_resource_factor(0, 80);
    // Kinds 10/9/8 for gold/wood/food, `Parm` = credited amount floor(load * factor / 100);
    // a load kind outside 0 ... 2 books nothing (W-41, O-172).
    for (kind_of_load, amount) in [
        (load::GOLD, 10),
        (load::WOOD, 7),
        (load::FOOD, 3),
        (load::NONE, 9),
    ] {
        gather::set_carried_load(&mut sim, colonist, amount, kind_of_load);
        gather::deliver_load(&mut sim, colonist, mining);
        gather::deliver_load(&mut sim, colonist, mining);
    }
    assert_eq!(
        kinds(sim.state, 0),
        [(kind::GOLD, 8), (kind::WOOD, 5), (kind::FOOD, 2)]
    );
}
