use super::*;
use crate::combat::projectile::ProjectileKind;
use crate::command::CommandKind;
use crate::command::testkit::run;
use crate::command::upgrade::{ABILITY_BUTTONS, check_cast_spell};
use crate::fx::Fx;
use crate::messaging::post_message;
use crate::module::ability_testkit as kit;
use crate::module::walk::facing_dir_with_offset;
use crate::save::checksum;
use crate::state::{SimEvent, SimState};
use crate::tick::object_tick;
use crate::upgrades::testkit::*;
use nml_formats::gifcon::parse_toolbar;

/// Collect taxes: button 543 (mana 75, `REQUIRED_GUID 2828`), execution index 1.
const TAX: u32 = 4450;
const TAX_BUTTON: i32 = 543;
const TAX_RESEARCH: u32 = 2828;
/// Dying (`PropertyDying_StartDying`, module 16).
const DIE: u32 = 0x110;

/// `0x36` kind 4 with the values of the UI for `button`.
fn cast(caster: ObjectId, button: i32) -> CommandKind {
    let &(_, anim, idx) = ABILITY_BUTTONS
        .iter()
        .find(|&&(b, _, _)| b == button)
        .expect("ability button");
    CommandKind::CastSpell {
        caster,
        art: ART_EXEC_TEMP,
        anim,
        idx,
        button,
    }
}

fn mana(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, MANA)
}

fn locked(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).unwrap().status & status::ANIM_LOCKED != 0
}

fn attack_on(sim: &Sim, id: ObjectId) -> bool {
    let o = sim.object(id).unwrap();
    o.has_module(ATTACK, false) && o.tick_list.contains(&(ATTACK as u8))
}

fn temp_guids(sim: &Sim, id: ObjectId) -> Vec<u32> {
    sim.object(id)
        .unwrap()
        .temp_upgrades
        .iter()
        .map(|e| e.guid)
        .collect()
}

/// Set mana explicitly.
fn set_mana(sim: &mut Sim, id: ObjectId, v: i32) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_value(MANA, v, 0);
}

/// World position of the tile center at height 0.
fn at_tile(x: i32, z: i32) -> [f32; 3] {
    [kit::world(x), 0.0, kit::world(z)]
}

/// `0x126 {kind, x, z, button}` with the target point as `f32` bits (like the command `0x36`).
fn cast_at(art: i32, p: [f32; 3], button: i32) -> Message {
    Message::new(
        msg::CAST,
        &[art, p[0].to_bits() as i32, p[2].to_bits() as i32, button],
    )
}

/// Positions of the world effects `kind` in the projectile list.
fn world_effects(sim: &Sim, kind: ProjectileKind) -> Vec<[f32; 3]> {
    sim.state
        .projectiles
        .list
        .iter()
        .filter(|p| p.kind == kind)
        .map(|p| p.target_pos)
        .collect()
}

fn explosions(sim: &Sim) -> Vec<[f32; 3]> {
    world_effects(sim, ProjectileKind::Explosion)
}

fn hp(sim: &Sim, id: ObjectId) -> i32 {
    sim.attribute(id, 14)
}

/// Face order of the cast (re/spells 3.1): Walk FINDTO argc 7 to the own grid position with the
/// target point as look-at point.
fn face_order(sim: &Sim, id: ObjectId, p: [f32; 3]) -> Message {
    let g = sim.object(id).unwrap().grid;
    let fx = Fx::from_f32(p[0] / 128.0).0;
    let fz = Fx::from_f32(p[2] / 128.0).0;
    Message::new(0x200, &[g.x.0, g.z.0, -1, fx, fz, -1, 1])
}

fn find_to(sim: &Sim, id: ObjectId) -> Option<Message> {
    crate::module::walk::walk(sim, id).and_then(|w| w.find_to.clone())
}

/// Medicine man (slot 0, mana 100) on `tile` in the spell world, facing +x (direction 0; created
/// without a rotation he faces -90 degrees).
fn medicine_man(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let m = kit::spawn(sim, kit::MEDICINE_MAN, 0, tile);
    crate::world::set_direction(sim, m, 0);
    set_mana(sim, m, 100);
    m
}

/// Sounds `action` (flags `0x42`) as `(object, position, button)`.
fn sounds(events: &[SimEvent], action: i32) -> Vec<(ObjectId, [f32; 3], u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Sound3d {
                object,
                action: a,
                flags: 0x42,
                pos,
                guid,
            } if *a == action => Some((*object, *pos, *guid)),
            _ => None,
        })
        .collect()
}

fn ticks(sim: &mut Sim, id: ObjectId, n: usize) {
    for _ in 0..n {
        object_tick(sim, id, 100);
    }
}

/// Governor of the slot with researched 2828 (button 543 free).
fn governor(sim: &mut Sim, slot: u8, tile: (i32, i32)) -> ObjectId {
    sim.state.add_guid_count(slot, TAX_RESEARCH, 1);
    spawn(sim, GOVERNOR, i32::from(slot), tile)
}

fn assert_loads(state: &SimState, rules: &crate::rules::Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

#[test]
fn the_tax_button_casts_and_starts_taxes_at_half_the_label_time() {
    // Governor (mana 100, button 543 costs 75, 2828 researched): CastSpell {4, 2, 1, 543}.
    // In the first object tick (FIFO): mana 25, ANIM_LOCKED, attack off, animation 390002[2] = 7;
    // the timers run from the next tick and fire as soon as more than 500 or 1000 ms have
    // accumulated (time marker 1 of animation 7 at 1000 ms): 0x326 in the 7th tick -> 4450 at the
    // governor, 0x426 in the 12th tick -> lock off, attack on again.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = governor(&mut sim, 0, (10, 10));
    assert_eq!(mana(&sim, g), 100);
    assert!(attack_on(&sim, g) && !locked(&sim, g));
    run(&mut sim, 0, cast(g, TAX_BUTTON));
    assert_eq!(mana(&sim, g), 100, "posted, not yet executed");
    sim.state.events.clear();
    ticks(&mut sim, g, 1);
    assert_eq!(mana(&sim, g), 25, "mana deducted immediately (C-222)");
    assert!(locked(&sim, g) && !attack_on(&sim, g));
    assert_eq!(sim.object(g).unwrap().anim.wanted, 7);
    let s = spell(&sim, g).unwrap();
    assert_eq!((s.art, s.button, s.cost, s.anim, s.idx), (4, 543, 75, 2, 1));
    // Kind 4 gets the same sounds - 0x1E at the cast with a value set, 0x1F after
    // 0x326, both at the world position of the governor.
    let at = sim.object(g).unwrap().world_pos;
    assert_eq!(sounds(&sim.state.events, 0x1E), [(g, at, 543)]);
    assert_loads(sim.state, &rules);
    ticks(&mut sim, g, 5);
    assert!(temp_guids(&sim, g).is_empty());
    assert!(sounds(&sim.state.events, 0x1F).is_empty());
    ticks(&mut sim, g, 1);
    assert_eq!(temp_guids(&sim, g), [TAX], "0x326 after more than 500 ms");
    assert_eq!(sounds(&sim.state.events, 0x1F), [(g, at, 543)]);
    assert!(locked(&sim, g));
    ticks(&mut sim, g, 4);
    assert!(locked(&sim, g) && !attack_on(&sim, g));
    ticks(&mut sim, g, 1);
    assert!(
        !locked(&sim, g) && attack_on(&sim, g),
        "0x426 after more than 1000 ms"
    );
    assert!(sim.module(g, INDEX).unwrap().base.events.is_empty());
}

#[test]
fn the_cast_times_by_the_first_animation_of_the_list() {
    // C-229 (re/temp-upgrades 15 no. 4): T = time marker 1 of 390002[0], not of the selected
    // animation. List 7 8 9 with markers 200 ms (7) and 5000 ms (9); button 543 selects
    // 390002[2] = 9, timing is by 200 ms.
    let mut rules = rules();
    rules.set_anim_labels(GOVERNOR, 7, &[(1, 200)]);
    rules.set_anim_labels(GOVERNOR, 9, &[(1, 5000)]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = governor(&mut sim, 0, (10, 10));
    spell_mut(&mut sim, g).unwrap().anims = vec![7, 8, 9];
    run(&mut sim, 0, cast(g, TAX_BUTTON));
    ticks(&mut sim, g, 1);
    assert_eq!(sim.object(g).unwrap().anim.wanted, 9);
    // 0x326 at more than 100 ms (2nd tick after the cast), 0x426 at more than 200 ms.
    ticks(&mut sim, g, 2);
    assert_eq!(temp_guids(&sim, g), [TAX]);
    assert!(locked(&sim, g));
    ticks(&mut sim, g, 1);
    assert!(!locked(&sim, g));
}

#[test]
fn an_abort_before_the_cast_point_loses_the_mana() {
    // re/temp-upgrades 15 no. 6, C-230: Halt in the 3rd tick deletes the timers, lock off, attack
    // on again - no entry, the mana stays deducted (only the own regeneration:
    // 0.1 per tick from the cast tick, 23 ticks -> 25 + 2.3).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = governor(&mut sim, 0, (10, 10));
    run(&mut sim, 0, cast(g, TAX_BUTTON));
    ticks(&mut sim, g, 3);
    assert!(locked(&sim, g));
    run(&mut sim, 0, CommandKind::Halt { units: vec![g] });
    assert!(!locked(&sim, g));
    ticks(&mut sim, g, 20);
    assert!(temp_guids(&sim, g).is_empty());
    assert!(attack_on(&sim, g));
    assert!(sim.module(g, INDEX).unwrap().base.events.is_empty());
    assert_eq!(mana(&sim, g), 27);
    assert_eq!(sim.object(g).unwrap().attributes.raw_fraction(MANA), 27_300);
}

#[test]
fn mounted_casters_do_not_cast() {
    // Mounted (`ISMOUNTED`): the cast has no effect and the mana stays; the command is not
    // rejected. The cast sets the kind before the check - with kind 4 as after an
    // earlier spell the simulation afterwards equals that without the command.
    let rules = rules();
    let mut sums = Vec::new();
    for with_command in [false, true] {
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = governor(&mut sim, 0, (10, 10));
        spell_mut(&mut sim, g).unwrap().art = ART_EXEC_TEMP;
        sim.object_mut(g).unwrap().status |= status::ISMOUNTED;
        if with_command {
            check_cast_spell(&mut sim, 0, g, 4, 2, 1, TAX_BUTTON).expect("accepted");
            run(&mut sim, 0, cast(g, TAX_BUTTON));
        }
        ticks(&mut sim, g, 15);
        assert_eq!(mana(&sim, g), 100);
        assert!(temp_guids(&sim, g).is_empty() && !locked(&sim, g));
        sums.push(checksum(sim.state));
    }
    assert_eq!(sums[0], sums[1], "simulation unchanged");
    // Kind 1: the message 0x126 directly (with abort like 0x36; the command path is covered by
    // `command::ability::tests::cast_spell_accepts_arts_one_to_three`). The kind
    // is already 1 beforehand; the height of the target point is 0 - the cast returning
    // mounted therefore changes nothing: no sound 0x1E, no lock, mana full.
    let rules = kit::spell_rules();
    let mut sums = Vec::new();
    for with_message in [false, true] {
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = kit::spawn(&mut sim, kit::MEDICINE_MAN, 0, (10, 10));
        set_mana(&mut sim, m, 100);
        spell_mut(&mut sim, m).unwrap().art = 1;
        sim.object_mut(m).unwrap().status |= status::ISMOUNTED;
        if with_message {
            assert!(post_message(
                &mut sim,
                m,
                cast_at(1, at_tile(16, 10), 480),
                true
            ));
        }
        let events = kit::run_ticks_collect(&mut sim, 30);
        assert!(sounds(&events, 0x1E).is_empty());
        assert_eq!(mana(&sim, m), 100);
        assert!(!locked(&sim, m) && explosions(&sim).is_empty());
        sums.push(checksum(sim.state));
    }
    assert_eq!(sums[0], sums[1], "simulation unchanged (kind 1)");
}

#[test]
fn spell_regeneration_stops_at_max_minus_reserve() {
    // re/temp-upgrades 15 no. 1/2: init stops the general regeneration; the tick gives
    // 1/(10*Max) per tick (0.1 mana) up to Max - reserve and above that sets to Max - reserve.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = governor(&mut sim, 0, (10, 10));
    spell_mut(&mut sim, g).unwrap().reserve = 30;
    ticks(&mut sim, g, 1);
    assert_eq!(mana(&sim, g), 70, "above Max - reserve: reset");
    let set_mana = |sim: &mut Sim, id: ObjectId, v: i32| {
        sim.object_mut(id)
            .unwrap()
            .attributes
            .set_current_value(MANA, v, 0);
    };
    set_mana(&mut sim, g, 60);
    ticks(&mut sim, g, 50);
    assert_eq!(mana(&sim, g), 65);
    ticks(&mut sim, g, 50);
    assert_eq!(mana(&sim, g), 70);
    ticks(&mut sim, g, 50);
    assert_eq!(mana(&sim, g), 70);
    assert_eq!(sim.object(g).unwrap().attributes.raw_fraction(MANA), 70_000);
    // Without module 38 the general regeneration runs (rate 1).
    let p = spawn(&mut sim, PIKEMAN, 0, (20, 10));
    set_mana(&mut sim, p, 50);
    let before = sim.object(p).unwrap().attributes.raw_fraction(MANA);
    ticks(&mut sim, p, 1);
    assert!(sim.object(p).unwrap().attributes.raw_fraction(MANA) > before);
}

#[test]
fn cast_spell_checks_button_mapping_owner_and_art() {
    // Exception A8: rejection with message, checksum unchanged: (anim, idx) does not match 543,
    // kind 2 with a button that is not 481, kind 5 (only via `0x4E`; the acceptance of the
    // kinds 1-3 is covered by `command::ability::tests::cast_spell_accepts_arts_one_to_three`),
    // foreign governor, dead governor, 2828 missing, button not in the menu, not an
    // ability button, no module 38; no module 44 or idx outside the list.
    let extra = concat!(
        "GUID 1201\nNAME Zauberer_kurz\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
        "PROPID 38\nVALUEID 390001 6 1 0\nVALUEID 390002 1 1 7\n",
        "PROPID 44\nVALUEID 450001 1 1 4452\n",
        "GUID 1202\nNAME Zauberer_ohne44\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\n",
        "PROPID 38\nVALUEID 390001 6 1 0\nVALUEID 390002 1 1 7\n",
    );
    let menus = concat!(
        "GUID 1201\nBUTTONID 7\nBUTTONMSG 544\nDO_MSG\nENDGUID\n",
        "GUID 1202\nBUTTONID 9\nBUTTONMSG 545\nDO_MSG\nENDGUID\n",
    );
    let mut rules = rules_from(&format!("{TYPES}{extra}"));
    rules.toolbar = parse_toolbar(&format!("{TOOLBAR}{menus}"));
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
    let foreign = governor(&mut sim, 1, (40, 10));
    let dead = spawn(&mut sim, GOVERNOR, 0, (12, 10));
    send(&mut sim, dead, &Message::new(DIE, &[]));
    let colonist = spawn(&mut sim, COLONIST, 0, (14, 10));
    let short = spawn(&mut sim, 1201, 0, (16, 10));
    let no_exec = spawn(&mut sim, 1202, 0, (18, 10));
    let reject = |sim: &mut Sim, caster, (art, anim, idx, button), why: &str| {
        let err = check_cast_spell(sim, 0, caster, art, anim, idx, button).unwrap_err();
        assert!(err.contains(why), "{button}: {err}");
        let before = checksum(sim.state);
        let kind = CommandKind::CastSpell {
            caster,
            art,
            anim,
            idx,
            button,
        };
        run(sim, 0, kind);
        assert_eq!(checksum(sim.state), before, "{button}: {err}");
    };
    reject(&mut sim, g, (4, 0, 1, 543), "do not match");
    reject(&mut sim, g, (2, 2, 1, 543), "does not match spell kind 2");
    reject(&mut sim, g, (5, 0, 0, 543), "only via drain mana");
    reject(&mut sim, foreign, (4, 2, 1, 543), "not an own");
    reject(&mut sim, dead, (4, 2, 1, 543), "not an own");
    reject(&mut sim, g, (4, 2, 1, 543), "prerequisite");
    reject(&mut sim, g, (4, 0, 0, 549), "not in the menu");
    reject(&mut sim, g, (4, 0, 0, 546), "not an ability button");
    reject(&mut sim, colonist, (4, 0, 2, 544), "no spell module");
    reject(&mut sim, short, (4, 0, 2, 544), "outside");
    reject(&mut sim, no_exec, (4, 1, 0, 545), "no execute module");
    sim.state.add_guid_count(0, TAX_RESEARCH, 1);
    check_cast_spell(&mut sim, 0, g, 4, 2, 1, 543).expect("accepted");
    check_cast_spell(&mut sim, 0, g, 4, 0, 2, 544).expect("accepted");
    assert!(sim.object(g).unwrap().fifo.is_empty());
}

#[test]
fn the_spell_state_starts_without_target_and_corpse() {
    use crate::module::ability_testkit as kit;
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let medicine_man = kit::spawn(&mut sim, kit::MEDICINE_MAN, 0, (10, 10));
    let s = spell(&sim, medicine_man).expect("module 38");
    assert_eq!(s.target, [0.0; 3]);
    assert_eq!(s.corpse, ObjectId::NONE);
}

// Kinds 1 (explosion) and 2 (storm), re/spells 3.1-3.3, 9.1. Spell world via
// `ability_testkit`; "N" = object tick in which `0x226` takes effect; one tick is two
// `tick::step`s (`kit::run_ticks`). The medicine man faces +x ([`medicine_man`]).

/// Measured: ticks after the tick of the message until `0x226` when the medicine man has to turn
/// by 180 degrees (`the_caster_turns_to_the_target_first`).
const START_TURN: usize = 2;
/// Likewise for the clamped look-at point (`a_face_point_off_the_map_is_clamped_and_starts`).
const START_CLAMPED: usize = 2;

#[test]
fn explosion_follows_its_timeline() {
    // re/spells 9.1, C-294: mana 100, button 480 costs 80; the target point is the world position
    // of a group of enemy colonists (HP 160) in front of the caster. The facing is already right,
    // the face order reports success synchronously: N is the tick of the message.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let colonists: Vec<ObjectId> = [(18, 10), (19, 10), (18, 11)]
        .into_iter()
        .map(|t| kit::spawn(&mut sim, kit::COLONIST, 1, t))
        .collect();
    let target = crate::combat::hit_zone::world_pos(&sim, colonists[0]).unwrap();
    let ground = [target[0], 0.0, target[2]];
    assert!(attack_on(&sim, m));
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    // N: lock, animation 390002[0] = 2, attack marked off, no deduction yet, sound 0x1E.
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert!(locked(&sim, m) && !attack_on(&sim, m));
    assert_eq!(sim.object(m).unwrap().anim.wanted, 2);
    assert_eq!(mana(&sim, m), 100, "kinds 1/2 pay not until the effect");
    let at = sim.object(m).unwrap().world_pos;
    assert_eq!(sounds(&events, 0x1E), [(m, at, 480)]);
    let s = spell(&sim, m).unwrap();
    assert_eq!((s.art, s.button, s.cost, s.anim), (1, 480, 80, 0));
    assert_eq!(s.target, ground);
    assert_loads(sim.state, &rules);
    // N+1 ... N+21: no effect yet.
    let events = kit::run_ticks_collect(&mut sim, 21);
    assert!(explosions(&sim).is_empty() && sounds(&events, 0x1F).is_empty());
    assert_eq!(mana(&sim, m), 100);
    // N+22: 0x326 - explosion at the target point at height 0 (D-237), mana 100 -> 20, sound 0x1F there.
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!(mana(&sim, m), 20);
    assert_eq!(explosions(&sim), [ground]);
    assert_eq!(sounds(&events, 0x1F), [(m, ground, 480)]);
    let shown: Vec<[f32; 3]> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::EffectAt { path, pos, .. } if path.ends_with("explosion_medicineman.pef") => {
                Some(*pos)
            }
            _ => None,
        })
        .collect();
    assert_eq!(shown, [ground], "presentation effect at height 0 (D-237)");
    assert_loads(sim.state, &rules);
    // The first effect tick still runs in N+22, remainder 400/300/200/100 in N+22 ... N+25.
    kit::run_ticks(&mut sim, 3);
    assert!(colonists.iter().all(|&c| hp(&sim, c) == 160));
    assert_eq!(explosions(&sim).len(), 1);
    // N+26 (fifth effect tick): -75 % at all colonists (160 - 120).
    kit::run_ticks(&mut sim, 1);
    for &c in &colonists {
        assert_eq!(hp(&sim, c), 40, "{c:?}");
    }
    assert!(explosions(&sim).is_empty());
    // N+44: 0x426 - lock off, attack on again.
    kit::run_ticks(&mut sim, 17);
    assert!(locked(&sim, m) && !attack_on(&sim, m));
    kit::run_ticks(&mut sim, 1);
    assert!(!locked(&sim, m) && attack_on(&sim, m));
}

#[test]
fn the_caster_turns_to_the_target_first() {
    // C-295: ground point behind the caster (-x), no enemy within scan range. The cast sends the
    // face order (argc 7, own grid position, target point as look-at point); 0x226 comes not
    // until the success after turning, the position stays.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (20, 20));
    let grid = sim.object(m).unwrap().grid;
    let target = at_tile(12, 20);
    let order = face_order(&sim, m, target);
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    let mut started = None;
    for k in 0..40 {
        let events = kit::run_ticks_collect(&mut sim, 1);
        if k == 0 {
            assert_eq!(sounds(&events, 0x1E).len(), 1, "sound at the cast");
        }
        if locked(&sim, m) {
            started = Some(k);
            break;
        }
        assert_eq!(
            find_to(&sim, m),
            Some(order.clone()),
            "face order until 0x226"
        );
        assert_eq!(mana(&sim, m), 100);
    }
    // Measured: 0x226 in tick START_TURN after the tick of the message.
    assert_eq!(started, Some(START_TURN));
    assert_eq!(sim.object(m).unwrap().grid, grid, "position unchanged");
    assert_eq!(sim.object(m).unwrap().anim.wanted, 2);
    // The effect follows 22 ticks after 0x226.
    kit::run_ticks(&mut sim, 21);
    assert!(explosions(&sim).is_empty());
    kit::run_ticks(&mut sim, 1);
    assert_eq!(explosions(&sim), [target]);
    assert_eq!(mana(&sim, m), 20);
}

#[test]
fn a_face_point_off_the_map_is_clamped_and_starts() {
    // re/walk 16.6: a look-at point outside the map is clamped to the edge tile (x to the hard
    // radius); walk turns there and reports success - 0x226 comes.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let (g, r) = {
        let o = sim.object(m).unwrap();
        (o.grid, o.hard_radius)
    };
    let target = [-64.0 * 128.0, 0.0, kit::world(20)];
    let order = face_order(&sim, m, target);
    let (fx, fz) = (order.arg(3), order.arg(4));
    assert!(fx >> 12 < 0, "look-at point outside");
    let clamped = facing_dir_with_offset((g.x.0, g.z.0, 0), (r, fz), 0);
    assert_ne!(
        clamped,
        facing_dir_with_offset((g.x.0, g.z.0, 0), (fx, fz), 0)
    );
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    kit::run_ticks(&mut sim, 1);
    let w = crate::module::walk::walk(&sim, m).unwrap();
    assert_eq!(w.target_dir, clamped, "direction to the edge tile");
    let mut started = None;
    for k in 0..40 {
        if locked(&sim, m) {
            started = Some(k);
            break;
        }
        kit::run_ticks(&mut sim, 1);
    }
    // Measured: 0x226 START_CLAMPED ticks after the tick of the message.
    assert_eq!(started, Some(START_CLAMPED));
    assert_eq!(spell(&sim, m).unwrap().target, target);
}

#[test]
fn a_walk_fault_still_starts() {
    // C-295 (success and fault are both 0x226): if walk cannot move, the face order aborts and
    // sends the fault synchronously - animation, lock and timers run.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (20, 20));
    crate::module::walk::walk_mut(&mut sim, m).unwrap().can_move = false;
    let target = at_tile(12, 20);
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    kit::run_ticks(&mut sim, 1);
    assert!(locked(&sim, m), "0x226 in the tick of the message");
    assert_eq!(sim.object(m).unwrap().anim.wanted, 2);
    assert_eq!(mana(&sim, m), 100);
    kit::run_ticks(&mut sim, 21);
    assert!(explosions(&sim).is_empty());
    kit::run_ticks(&mut sim, 1);
    assert_eq!(explosions(&sim), [target]);
    assert_eq!(mana(&sim, m), 20);
}

#[test]
fn an_abort_before_the_effect_costs_nothing() {
    // re/spells 8 no. 1 (counterpart to C-230).
    let rules = kit::spell_rules();
    let halt = |sim: &mut Sim, m: ObjectId| run(sim, 0, CommandKind::Halt { units: vec![m] });
    // The group deletion in `m_Events` takes effect not until the next `update_events`.
    let no_timers = |sim: &Sim, m: ObjectId| {
        let b = &sim.module(m, INDEX).unwrap().base;
        !b.has_pending_timer(msg::EFFECT) && !b.has_pending_timer(msg::END)
    };

    // (a) Abort in N+10: timers deleted, no mana gone, no explosion.
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    assert!(post_message(
        &mut sim,
        m,
        cast_at(1, at_tile(16, 10), 480),
        true
    ));
    kit::run_ticks(&mut sim, 10);
    assert!(locked(&sim, m));
    halt(&mut sim, m);
    assert!(!locked(&sim, m) && no_timers(&sim, m));
    let events = kit::run_ticks_collect(&mut sim, 40);
    assert!(explosions(&sim).is_empty() && sounds(&events, 0x1F).is_empty());
    assert_eq!(mana(&sim, m), 100);

    // (b) Abort in N+23: the explosion is already in the list and takes effect in N+26 anyway.
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let c = kit::spawn(&mut sim, kit::COLONIST, 1, (16, 10));
    let target = crate::combat::hit_zone::world_pos(&sim, c).unwrap();
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    kit::run_ticks(&mut sim, 23);
    assert_eq!(explosions(&sim).len(), 1);
    assert_eq!(mana(&sim, m), 20);
    halt(&mut sim, m);
    assert!(!locked(&sim, m) && no_timers(&sim, m));
    kit::run_ticks(&mut sim, 3);
    assert_eq!(
        explosions(&sim).len(),
        1,
        "still in the list (N+23 ... N+25)"
    );
    assert_eq!(hp(&sim, c), 160);
    kit::run_ticks(&mut sim, 1);
    assert_eq!(hp(&sim, c), 40, "damage in N+26");

    // (c) Abort while turning: walk (slot 0) aborts first and still sends 0x226 via the
    // fault (lock, timers); the abort of module 38 in the same pass deletes
    // both. The animation stays set (as in the original, not checked here).
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (20, 20));
    let target = at_tile(12, 20);
    let order = face_order(&sim, m, target);
    assert!(post_message(&mut sim, m, cast_at(1, target, 480), true));
    kit::run_ticks(&mut sim, 1);
    assert!(!locked(&sim, m), "still turning");
    assert_eq!(find_to(&sim, m), Some(order));
    halt(&mut sim, m);
    assert!(!locked(&sim, m) && no_timers(&sim, m));
    let events = kit::run_ticks_collect(&mut sim, 40);
    assert!(explosions(&sim).is_empty() && sounds(&events, 0x1F).is_empty());
    assert!(!locked(&sim, m));
    assert_eq!(mana(&sim, m), 100);
}

#[test]
fn mana_can_go_negative_at_the_effect() {
    // C-294: the effect deducts without a check (K-62). Mana set to 10 after N+21 -> in
    // N+22 -70 from the effect, +0.1 from the regeneration of the same tick (Max 100).
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    assert!(post_message(
        &mut sim,
        m,
        cast_at(1, at_tile(16, 10), 480),
        true
    ));
    kit::run_ticks(&mut sim, 22);
    set_mana(&mut sim, m, 10);
    kit::run_ticks(&mut sim, 1);
    assert_eq!(explosions(&sim).len(), 1);
    // K-62: the fraction becomes negative, the value reads 0.
    assert_eq!(mana(&sim, m), 0);
    let raw = |sim: &Sim| sim.object(m).unwrap().attributes.raw_fraction(MANA);
    assert_eq!(raw(&sim), -69_900);
    assert_loads(sim.state, &rules);
    kit::run_ticks(&mut sim, 100);
    assert_eq!(mana(&sim, m), 0);
    assert_eq!(raw(&sim), -59_900);
}

#[test]
fn the_storm_spell_creates_the_storm() {
    // re/spells 9.2: kind 2, button 481 (cost 80) -> in N+22 a storm at the target point, mana 20.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let target = at_tile(16, 10);
    assert!(post_message(&mut sim, m, cast_at(2, target, 481), true));
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert!(locked(&sim, m));
    assert_eq!(sounds(&events, 0x1E).len(), 1);
    let events = kit::run_ticks_collect(&mut sim, 21);
    assert!(world_effects(&sim, ProjectileKind::Storm).is_empty());
    assert!(sounds(&events, 0x1F).is_empty());
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!(world_effects(&sim, ProjectileKind::Storm), [target]);
    assert!(explosions(&sim).is_empty());
    assert_eq!(mana(&sim, m), 20);
    assert_eq!(sounds(&events, 0x1F), [(m, target, 481)]);
}

#[test]
fn a_button_without_values_is_free_and_silent() {
    // re/spells 3.1: without a value set of the button cost 0, no check, no sound 0x1E; the effect
    // runs anyway (sound 0x1F as always with kind 1).
    let mut rules = kit::spell_rules();
    rules.defaults.remove(&480);
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    set_mana(&mut sim, m, 5);
    assert!(post_message(
        &mut sim,
        m,
        cast_at(1, at_tile(16, 10), 480),
        true
    ));
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert!(sounds(&events, 0x1E).is_empty());
    assert!(locked(&sim, m));
    assert_eq!(spell(&sim, m).unwrap().cost, 0);
    let events = kit::run_ticks_collect(&mut sim, 22);
    assert_eq!(explosions(&sim), [at_tile(16, 10)]);
    assert_eq!(sounds(&events, 0x1F).len(), 1);
    // Only the regeneration (0.1 per tick): 5 + 22 * 0.1 = 7.2 until N+21; in N+22
    // `AddToCurrentValue(15, −0)` computes from the integer value (7.2 -> 7, `0x0048FAF0`), then +0.1.
    assert_eq!(sim.object(m).unwrap().attributes.raw_fraction(MANA), 7_100);
}

// Kind 3 (reincarnation), re/spells 3.4, 3.5, 9.3. Spell world via `ability_testkit`;
// "N" = object tick of the message. The medicine man faces +x, the corpses lie
// there: the face order reports success synchronously, `0x226` takes effect in N.

/// Button "Reincarnation" (cost 40 in `kit::spell_rules()`).
const REINCARNATION: i32 = 482;
/// Measured: ticks after the death of the caster until the `0x114` to his wolf
/// (`the_wolf_vanishes_with_its_caster`).
const VANISH_AFTER: usize = 1;
/// Measured: ticks after the `0x114` until the wolf is destroyed (timer 1000 ms).
const GONE_AFTER: usize = 11;
/// Measured: ticks after the appearance of an enemy colonist 3 tiles in front of the wolf until
/// the first hit (`the_wolf_fights_for_its_caster`): search in the 1500 ms grid of the wolf,
/// approach, hit at 500 ms of the test animation.
const FIRST_BITE: usize = 28;

/// `0x126 {3, 0, 0, 482}`: kind 3 carries no target point (re/spells 3.1).
fn reincarnation() -> Message {
    Message::new(msg::CAST, &[ART_REINCARNATE, 0, 0, REINCARNATION])
}

/// Let `0x110` hit the victim and run one object tick; returns the ID of the corpse.
/// `DIED` is set immediately, the new ID on slot 8 comes in the phase-0 step (a victim that
/// already belongs to slot 8 keeps its own).
fn kill(sim: &mut Sim, victim: ObjectId) -> ObjectId {
    send(sim, victim, &Message::new(DIE, &[]));
    let events = kit::run_ticks_collect(sim, 1);
    events
        .iter()
        .find_map(|e| match e {
            SimEvent::OwnerChanged { old, new } if *old == victim => Some(*new),
            _ => None,
        })
        .unwrap_or(victim)
}

/// Corpse of a victim `guid` of the slot `slot` on `tile`.
fn corpse(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let v = kit::spawn(sim, guid, slot, tile);
    let body = kill(sim, v);
    assert!(sim.object(body).unwrap().status & status::DIED != 0);
    body
}

fn reserve(sim: &Sim, id: ObjectId) -> i32 {
    spell(sim, id).unwrap().reserve
}

/// Upper limit of the regeneration `Max(15) − Reserve` (re/spells 3.5).
fn cap(sim: &Sim, id: ObjectId) -> i32 {
    let p = mana_percent(sim, id);
    sim.object(id).unwrap().attributes.max_value(MANA, p) - reserve(sim, id)
}

fn raw(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).unwrap().attributes.raw_fraction(MANA)
}

/// War wolves that were created in `events`.
fn new_wolves(sim: &Sim, events: &[SimEvent]) -> Vec<ObjectId> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Created { id }
                if sim
                    .object(*id)
                    .is_some_and(|o| o.guid == reincarnate::BATTLE_WOLF) =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect()
}

/// Presentation effect `path` on the object `id` in `events`.
fn effect_on(events: &[SimEvent], id: ObjectId, path: &str) -> bool {
    events.iter().any(|e| {
        matches!(e, SimEvent::EffectOnObject { object, path: p }
            if *object == id && p.as_str() == path)
    })
}

/// Death callbacks of the object (module 16).
fn listeners(sim: &Sim, id: ObjectId) -> Vec<(ObjectId, Message)> {
    match &sim.module(id, crate::module::dying::INDEX).unwrap().kind {
        ModuleKind::Dying(d) => d.listeners.iter().cloned().collect(),
        _ => unreachable!(),
    }
}

/// Reincarnation at the nearest corpse: post the message, run N ... N+22; returns the
/// wolf. The animation runs until N+44.
fn reincarnate_now(sim: &mut Sim, m: ObjectId) -> ObjectId {
    assert!(post_message(sim, m, reincarnation(), true));
    let events = kit::run_ticks_collect(sim, 23);
    let wolves = new_wolves(sim, &events);
    assert_eq!(wolves.len(), 1, "one wolf in N+22");
    wolves[0]
}

/// Medicine man on (10, 10), two enemy colonist corpses on (15, 10) and (16, 10), two
/// reincarnations in a row: returns caster and wolves, state after N'+44.
fn two_wolves(sim: &mut Sim) -> (ObjectId, [ObjectId; 2]) {
    let m = medicine_man(sim, (10, 10));
    corpse(sim, kit::COLONIST, 1, (15, 10));
    corpse(sim, kit::COLONIST, 1, (16, 10));
    let first = reincarnate_now(sim, m);
    kit::run_ticks(sim, 22);
    let second = reincarnate_now(sim, m);
    kit::run_ticks(sim, 22);
    assert!(!locked(sim, m));
    (m, [first, second])
}

#[test]
fn reincarnation_follows_its_timeline() {
    // re/spells 9.3: mana 100, cost 40, sight 52; enemy colonist corpse 5 tiles in front of the
    // caster.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let body = corpse(&mut sim, kit::COLONIST, 1, (15, 10));
    assert_eq!(body.slot(), 8, "ID on slot 8");
    set_mana(&mut sim, m, 100);
    assert!(post_message(&mut sim, m, reincarnation(), true));
    // N: sound 0x1E, corpse and its position (via `f32`) remembered, lock, animation 2, no deduction.
    let events = kit::run_ticks_collect(&mut sim, 1);
    let at = sim.object(m).unwrap().world_pos;
    assert_eq!(sounds(&events, 0x1E), [(m, at, 482)]);
    assert!(locked(&sim, m) && !attack_on(&sim, m));
    assert_eq!(sim.object(m).unwrap().anim.wanted, 2);
    assert_eq!(mana(&sim, m), 100);
    let s = spell(&sim, m).unwrap().clone();
    assert_eq!(
        (s.art, s.button, s.cost, s.anim, s.corpse),
        (3, 482, 40, 0, body)
    );
    let g = crate::world::grid_to_world(sim.object(body).unwrap().grid, rules.map.cell);
    assert_eq!(s.target, [g[0], 0.0, g[2]]);
    assert_loads(sim.state, &rules);
    // N+1 ... N+21: nothing.
    let events = kit::run_ticks_collect(&mut sim, 21);
    assert!(new_wolves(&sim, &events).is_empty() && sounds(&events, 0x1F).is_empty());
    assert_eq!(mana(&sim, m), 100);
    assert!(sim.object(body).is_some());
    // N+22: 0x326 - mana 100 -> 60, reserve 40, war wolf 3420 on the slot of the caster
    // on the tile of the corpse, sound 0x1F at the caster, corpse destroyed in the phase-0 step.
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!(
        (mana(&sim, m), reserve(&sim, m), cap(&sim, m)),
        (60, 40, 60)
    );
    let wolves = new_wolves(&sim, &events);
    assert_eq!(wolves.len(), 1);
    let wolf = wolves[0];
    assert_eq!(wolf.slot(), m.slot(), "slot of the caster");
    let w = sim.object(wolf).unwrap();
    assert_eq!(w.tile, (15, 10), "tile of the corpse");
    assert_eq!((w.world_pos[0], w.world_pos[2]), (s.target[0], s.target[2]));
    assert_eq!(w.color, crate::setup::player_color(&sim, 0));
    assert!(effect_on(&events, wolf, "data/effects/Wolf_erscheinen.pef"));
    assert_eq!(sounds(&events, 0x1F), [(m, at, 482)]);
    assert!(sim.object(body).is_none(), "corpse destroyed");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SimEvent::Destroyed { id, .. } if *id == body))
    );
    // Death callbacks: at the caster `0x114` to the wolf, at the wolf `0x526 {0, 40}` to the caster.
    assert_eq!(listeners(&sim, m), [(wolf, Message::new(0x114, &[]))]);
    assert_eq!(
        listeners(&sim, wolf),
        [(m, Message::new(msg::RESERVE, &[0, 40]))]
    );
    assert_loads(sim.state, &rules);
    // The regeneration caps at 60.
    kit::run_ticks(&mut sim, 50);
    assert_eq!((mana(&sim, m), raw(&sim, m)), (60, 60_000));
}

#[test]
fn two_wolves_then_the_third_fails() {
    // re/spells 9.3: the second reincarnation takes the remaining corpse -> mana 20, reserve
    // 80; the third fails at the cost check (20 < 40) without sound.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (m, wolves) = two_wolves(&mut sim);
    assert_eq!(
        (mana(&sim, m), reserve(&sim, m), cap(&sim, m)),
        (20, 80, 20)
    );
    assert_eq!(sim.object(wolves[0]).unwrap().tile, (15, 10));
    assert_eq!(sim.object(wolves[1]).unwrap().tile, (16, 10));
    assert!(post_message(&mut sim, m, reincarnation(), true));
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert!(sounds(&events, 0x1E).is_empty(), "without sound");
    assert!(!locked(&sim, m));
    assert_eq!(spell(&sim, m).unwrap().cost, 40);
    let events = kit::run_ticks_collect(&mut sim, 30);
    assert!(new_wolves(&sim, &events).is_empty());
    assert_eq!((mana(&sim, m), reserve(&sim, m)), (20, 80));
}

#[test]
fn a_dead_wolf_frees_the_reserve() {
    // C-299, re/spells 9.3: if a wolf dies, its callback sends `0x526 {wolf, 40}` -> reserve 40,
    // upper limit 60; the wolf itself gets no `0x114`.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (m, wolves) = two_wolves(&mut sim);
    send(&mut sim, wolves[0], &Message::new(DIE, &[]));
    assert_eq!(reserve(&sim, m), 80, "callback not until the dying tick");
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!((reserve(&sim, m), cap(&sim, m)), (40, 60));
    assert!(!effect_on(
        &events,
        wolves[0],
        "data/effects/Wolf_verschwinden.pef"
    ));
    // The regeneration rises again up to 60 (0.1 per tick).
    kit::run_ticks(&mut sim, 10);
    assert!(raw(&sim, m) > 20_000);
    kit::run_ticks(&mut sim, 400);
    assert_eq!((mana(&sim, m), raw(&sim, m)), (60, 60_000));
    assert_eq!(reserve(&sim, m), 40);
}

#[test]
fn the_wolf_vanishes_with_its_caster() {
    // C-300: if the caster dies, his wolf gets `0x114` (fade-out, destroyed after 1000 ms);
    // the reserve stays because the wolf vanishes without dying (re/spells 8 no. 11).
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    corpse(&mut sim, kit::COLONIST, 1, (15, 10));
    let wolf = reincarnate_now(&mut sim, m);
    kit::run_ticks(&mut sim, 22);
    send(&mut sim, m, &Message::new(DIE, &[]));
    let mut all = Vec::new();
    let mut vanish = None;
    for k in 1..=30 {
        let events = kit::run_ticks_collect(&mut sim, 1);
        let hit = effect_on(&events, wolf, "data/effects/Wolf_verschwinden.pef");
        all.extend(events);
        if hit {
            vanish = Some(k);
            break;
        }
    }
    assert_eq!(vanish, Some(VANISH_AFTER), "0x114 at the wolf");
    let mut gone = None;
    for k in 1..=30 {
        kit::run_ticks(&mut sim, 1);
        if sim.object(wolf).is_none() {
            gone = Some(k);
            break;
        }
    }
    assert_eq!(gone, Some(GONE_AFTER), "destroyed after 1000 ms");
    let caster = all
        .iter()
        .find_map(|e| match e {
            SimEvent::OwnerChanged { old, new } if *old == m => Some(*new),
            _ => None,
        })
        .expect("caster on slot 8");
    assert_eq!(reserve(&sim, caster), 40, "no callback");
}

#[test]
fn reincarnation_needs_twenty_mana() {
    // C-296, re/spells 9.3: cost set to 10; mana 15 (and 19) passes the cost check (sound
    // 0x1E), the corpse search fails at the fixed 20 - no animation, no deduction. Mana 20
    // finds the corpse.
    let mut rules = kit::spell_rules();
    rules
        .defaults
        .insert(482, [(MANA, 10)].into_iter().collect());
    assert_eq!(reincarnate::MIN_MANA, 20);
    for (v, works) in [(15, false), (19, false), (20, true)] {
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = medicine_man(&mut sim, (10, 10));
        let body = corpse(&mut sim, kit::COLONIST, 1, (15, 10));
        set_mana(&mut sim, m, v);
        assert!(post_message(&mut sim, m, reincarnation(), true));
        let events = kit::run_ticks_collect(&mut sim, 1);
        assert_eq!(sounds(&events, 0x1E).len(), 1, "mana {v}");
        assert_eq!(locked(&sim, m), works, "mana {v}");
        if works {
            assert_eq!(spell(&sim, m).unwrap().corpse, body);
            continue;
        }
        assert_ne!(sim.object(m).unwrap().anim.wanted, 2, "no animation");
        assert_eq!(spell(&sim, m).unwrap().corpse, ObjectId::NONE);
        // Directly after the cast tick: no deduction, only the regeneration (+0.1).
        assert_eq!(mana(&sim, m), v);
        assert_eq!(raw(&sim, m), v * 1000 + 100);
        let events = kit::run_ticks_collect(&mut sim, 30);
        assert!(new_wolves(&sim, &events).is_empty());
        assert!(sim.object(body).is_some());
    }
}

#[test]
fn no_corpse_no_spell() {
    // re/spells 3.4, 9.3: search radius sight/4 = Fx(52) >> 2 = 13 tiles, extended in edge mode by
    // the own hard radius; the edge distance of the corpse to the position of the caster is
    // compared. Just outside (+4 grid points, Fixed_Hypot discards 2 bits): sound, no mana, no
    // animation. Exactly on the boundary: found.
    let rules = kit::spell_rules();
    for (extra, found) in [(4, false), (0, true)] {
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = medicine_man(&mut sim, (10, 10));
        let (g, hard) = {
            let o = sim.object(m).unwrap();
            (o.grid, o.hard_radius)
        };
        let radius = Fx::from_int(sim.attribute(m, 18)).0 >> 2;
        assert_eq!(radius, 13 * 4096);
        let reach = radius + hard;
        let v = kit::spawn(&mut sim, kit::COLONIST, 1, (20, 10));
        let vh = sim.object(v).unwrap().hard_radius;
        let x = g.x.0 + reach + vh + extra;
        crate::world::set_grid_position(
            &mut sim,
            v,
            crate::fx::FxVec3::new(Fx(x), Fx(0), g.z),
            true,
        );
        let body = kill(&mut sim, v);
        let d = crate::collision::edge_distance_to_point(&mut sim, body, (g.x.0, g.z.0));
        assert_eq!(d <= reach, found, "edge distance {d}, reach {reach}");
        if found {
            assert_eq!(reincarnate::find_corpse(&mut sim, m), Some(body));
            assert_eq!(spell(&sim, m).unwrap().corpse, body);
            continue;
        }
        assert!(post_message(&mut sim, m, reincarnation(), true));
        let events = kit::run_ticks_collect(&mut sim, 1);
        assert_eq!(sounds(&events, 0x1E).len(), 1, "sound already played");
        assert!(!locked(&sim, m));
        assert_eq!(spell(&sim, m).unwrap().corpse, ObjectId::NONE);
        assert_eq!((mana(&sim, m), raw(&sim, m)), (100, 100_000));
        let events = kit::run_ticks_collect(&mut sim, 30);
        assert!(new_wolves(&sim, &events).is_empty());
        assert_eq!(mana(&sim, m), 100);
    }
}

#[test]
fn any_dead_walker_is_a_corpse() {
    // C-297: the filter (`0x4000015`, status DIED, module walk) checks neither owner nor fog:
    // own, enemy, animal and former wolf corpses are found, also on a map in
    // fog; building ruins (dead, without walk) never; the nearest wins.
    let rules = kit::spell_rules();
    for (guid, slot) in [
        (kit::COLONIST, 0),
        (kit::COLONIST, 1),
        (kit::HORSE, 8),
        (reincarnate::BATTLE_WOLF, 0),
    ] {
        let mut state = kit::state_fogged(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let body = corpse(&mut sim, guid, slot, (20, 10));
        let m = medicine_man(&mut sim, (10, 10));
        if slot != 0 {
            let pos = sim.object(body).unwrap().world_pos;
            let view = crate::fog::FogView::new(sim.state);
            assert!(
                !view.world_pos_explored(&rules.map, pos, 0, false),
                "in fog"
            );
        }
        assert_eq!(
            reincarnate::find_corpse(&mut sim, m),
            Some(body),
            "{guid}/{slot}"
        );
    }
    // Ruins: a dead building (DIED set) has no walk module.
    let mut state = kit::state_fogged(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let ruin = kit::spawn(&mut sim, kit::MONASTERY, 1, (16, 10));
    sim.object_mut(ruin).unwrap().status |= status::DIED;
    let m = medicine_man(&mut sim, (10, 10));
    assert_eq!(reincarnate::find_corpse(&mut sim, m), None);
    // The nearest wins (all in the same bucket): 2 tiles before 3 and 4.
    let mut state = kit::state_fogged(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    corpse(&mut sim, kit::COLONIST, 1, (14, 10));
    let near = corpse(&mut sim, reincarnate::BATTLE_WOLF, 0, (12, 10));
    corpse(&mut sim, kit::COLONIST, 0, (10, 13));
    let m = medicine_man(&mut sim, (10, 10));
    assert_eq!(reincarnate::find_corpse(&mut sim, m), Some(near));
}

#[test]
fn a_wolf_rises_from_a_vanished_corpse() {
    // C-298: the corpse is not checked again at 0x326. Destroyed between cast and 0x326 ->
    // the wolf appears anyway at the remembered position.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    let body = corpse(&mut sim, kit::COLONIST, 1, (15, 10));
    assert!(post_message(&mut sim, m, reincarnation(), true));
    kit::run_ticks(&mut sim, 1);
    let target = spell(&sim, m).unwrap().target;
    crate::world::queue_destroy(&mut sim, body);
    kit::run_ticks(&mut sim, 1);
    assert!(sim.object(body).is_none());
    kit::run_ticks(&mut sim, 20);
    let events = kit::run_ticks_collect(&mut sim, 1);
    let wolves = new_wolves(&sim, &events);
    assert_eq!(wolves.len(), 1, "wolf in N+22");
    let w = sim.object(wolves[0]).unwrap();
    assert_eq!((w.world_pos[0], w.world_pos[2]), (target[0], target[2]));
    assert_eq!(w.tile, (15, 10));
    assert_eq!((mana(&sim, m), reserve(&sim, m)), (60, 40));

    // Two casters with the same corpse -> two wolves. The second has to turn around first; its
    // effect comes after the first one destroyed the corpse.
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = medicine_man(&mut sim, (10, 10));
    let b = medicine_man(&mut sim, (20, 10));
    let body = corpse(&mut sim, kit::COLONIST, 1, (15, 10));
    assert!(post_message(&mut sim, a, reincarnation(), true));
    assert!(post_message(&mut sim, b, reincarnation(), true));
    kit::run_ticks(&mut sim, 1);
    assert_eq!(spell(&sim, a).unwrap().corpse, body);
    assert_eq!(spell(&sim, b).unwrap().corpse, body);
    let events = kit::run_ticks_collect(&mut sim, 30);
    let wolves = new_wolves(&sim, &events);
    assert_eq!(wolves.len(), 2);
    for (&w, &caster) in wolves.iter().zip(&[a, b]) {
        assert_eq!(sim.object(w).unwrap().tile, (15, 10));
        assert_eq!(
            listeners(&sim, w),
            [(caster, Message::new(msg::RESERVE, &[0, 40]))]
        );
    }
    assert!(sim.object(body).is_none());
    for c in [a, b] {
        assert_eq!((mana(&sim, c), reserve(&sim, c)), (60, 40));
    }
}

#[test]
fn the_wolf_fights_for_its_caster() {
    // re/spells 4, re/animals 2: the wolf belongs to the slot of the caster, is neither aggressive
    // nor roams; it attacks an enemy colonist within reach. The default DEFENSIVE (mode 2)
    // looks for no targets (re/combat-attack 3.1): slot 0 gets OFFENSIVE (mode 3), which the wolf
    // takes over on creation like every unit of the slot. The spell world has no target
    // priorities; the ones of the combat world are added here (`combat::testkit::PRIORITIES`).
    let mut rules = kit::spell_rules();
    let defines = crate::combat::testkit::defines();
    rules.priorities =
        nml_formats::targets::parse_targets(crate::combat::testkit::PRIORITIES, &defines).entries;
    // Test values for the bite (the war wolf of the spell world has neither attack value 19 nor
    // time markers of its combat animation 2): attack 30, end 1000 ms, hit at 500 ms.
    rules
        .defaults
        .get_mut(&reincarnate::BATTLE_WOLF)
        .unwrap()
        .insert(19, 30);
    rules.set_anim_labels(reincarnate::BATTLE_WOLF, 2, &[(1, 1000), (2, 500)]);
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = medicine_man(&mut sim, (10, 10));
    corpse(&mut sim, kit::COLONIST, 1, (15, 10));
    sim.state.combat.behavior_mode[0] = 3;
    let wolf = reincarnate_now(&mut sim, m);
    assert_eq!(wolf.slot(), m.slot());
    let att = crate::module::attack::attack(&sim, wolf).unwrap();
    assert_eq!((att.aggressiveness, att.mode), (4, 3), "stance of the slot");
    let a = crate::module::animal::animal(&sim, wolf).unwrap();
    assert!(!a.aggressive && !a.roamer && !a.roaming);
    let grid = sim.object(wolf).unwrap().grid;
    kit::run_ticks(&mut sim, 50);
    assert_eq!(sim.object(wolf).unwrap().grid, grid, "no roaming");
    let enemy = kit::spawn(&mut sim, kit::COLONIST, 1, (18, 10));
    let mut bite = None;
    for k in 0..200 {
        if hp(&sim, enemy) < 160 {
            bite = Some(k);
            break;
        }
        kit::run_ticks(&mut sim, 1);
    }
    assert_eq!(bite, Some(FIRST_BITE), "first hit");
}

// Kind 5 (drain mana) via the command `0x4E`, re/spells 3.1, 3.3, 9.4, 8 no. 5-8.
// N = the first object tick of the missionary from the command step on: the command runs before
// the world tick; [`drain_tick`] gives it in the first of its two steps, so N is the tick of
// `drain_tick`. Animation, lock and timers apply already after the command step.

/// Research of the button 541 (`REQUIRED_GUID 2832`).
const DRAIN_RESEARCH: u32 = 2832;

/// Missionary 1113 (slot 0, mana 200 of 200, HP 1000, research 2832) on `tile`, facing +x.
fn missionary(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    kit::unlock(sim.state, 0, DRAIN_RESEARCH);
    let m = kit::spawn(sim, kit::MISSIONARY, 0, tile);
    crate::world::set_direction(sim, m, 0);
    set_mana(sim, m, 200);
    m
}

/// Enemy medicine man (slot 1, mana 100) on `tile`.
fn enemy_caster(sim: &mut Sim, tile: (i32, i32)) -> ObjectId {
    let t = kit::spawn(sim, kit::MEDICINE_MAN, 1, tile);
    set_mana(sim, t, 100);
    t
}

fn set_hp(sim: &mut Sim, id: ObjectId, v: i32) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_value(14, v, 0);
}

fn drain_cmd(unit: ObjectId, target: ObjectId) -> CommandKind {
    CommandKind::DrainMana {
        unit,
        target,
        button: 541,
    }
}

/// One object tick (two `tick::step`s), the first step with the command `kind` from slot 0.
fn drain_tick(sim: &mut Sim, kind: CommandKind) -> Vec<SimEvent> {
    let c = crate::command::Command { player: 0, kind };
    let mut events = crate::tick::step(sim.state, sim.rules, &[c]);
    events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    events
}

/// Stored success message of module 38.
fn spell_success(sim: &Sim, id: ObjectId) -> Option<Message> {
    sim.module(id, INDEX).unwrap().base.success.clone()
}

/// Pending timers `code` of module 38 (work list and event list).
fn pending(sim: &Sim, id: ObjectId, code: u32) -> usize {
    let b = &sim.module(id, INDEX).unwrap().base;
    b.timers_ws
        .iter()
        .chain(&b.events)
        .filter(|e| e.msg.code == code)
        .count()
}

/// Target position at height 0 (look-at point of kind 5, C-305).
fn ground_of(sim: &Sim, id: ObjectId) -> [f32; 3] {
    let p = crate::combat::hit_zone::world_pos(sim, id).unwrap();
    [p[0], 0.0, p[2]]
}

#[test]
fn drain_mana_follows_its_timeline() {
    // re/spells 9.4: missionary mana 200, HP 600 of 1000, cost 50, against an enemy medicine man
    // with mana 100 in front of him (+x, out of scan range of both attack modules).
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (10, 10));
    let t = enemy_caster(&mut sim, (40, 10));
    set_hp(&mut sim, m, 600);
    let events = drain_tick(&mut sim, drain_cmd(m, t));
    // N: lock, animation 390002[0] = 9, attack off, sound 0x1E, no deduction; the envelope has
    // placed the success message 0xB7F {target} in module 38.
    assert!(locked(&sim, m) && !attack_on(&sim, m));
    assert_eq!(sim.object(m).unwrap().anim.wanted, 9);
    let at = sim.object(m).unwrap().world_pos;
    assert_eq!(sounds(&events, 0x1E), [(m, at, 541)]);
    assert_eq!(mana(&sim, m), 200, "kind 5 pays not until the effect");
    let s = spell(&sim, m).unwrap();
    assert_eq!((s.art, s.button, s.cost, s.anim), (5, 541, 50, 0));
    assert_eq!(
        s.target,
        ground_of(&sim, t),
        "look-at point = target position (C-305)"
    );
    assert_eq!(
        spell_success(&sim, m),
        Some(Message::new(0xB7F, &[t.as_i32()]))
    );
    assert_loads(sim.state, &rules);
    // N+1 ... N+21: nothing yet.
    let events = kit::run_ticks_collect(&mut sim, 21);
    assert!(sounds(&events, 0x1F).is_empty());
    assert_eq!((mana(&sim, m), mana(&sim, t), hp(&sim, m)), (200, 100, 600));
    // N+22: 0x326 - cost deducted, then 0xB7F: all the mana of the target becomes HP; sound 0x1F at
    // the missionary; the success message is used up.
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!((mana(&sim, m), mana(&sim, t), hp(&sim, m)), (150, 0, 700));
    assert_eq!(sounds(&events, 0x1F), [(m, at, 541)]);
    assert_eq!(spell_success(&sim, m), None);
    assert_loads(sim.state, &rules);
    // N+44: 0x426 - lock off, attack on again.
    kit::run_ticks(&mut sim, 21);
    assert!(locked(&sim, m) && !attack_on(&sim, m));
    kit::run_ticks(&mut sim, 1);
    assert!(!locked(&sim, m) && attack_on(&sim, m));

    // Full missionary: the HP stay at the maximum 1000.
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (10, 10));
    let t = enemy_caster(&mut sim, (40, 10));
    drain_tick(&mut sim, drain_cmd(m, t));
    kit::run_ticks(&mut sim, 22);
    assert_eq!((mana(&sim, m), mana(&sim, t), hp(&sim, m)), (150, 0, 1000));
}

#[test]
fn a_second_drain_pays_twice_and_drains_once() {
    // C-304, re/spells 8 no. 8: 0x4E runs without abort; a second command in N+10 creates new
    // timers, the old ones stay. Two 0x326 (N+22 old, N+32 new) pay 50 each; the first
    // sends the (new) success message, the second finds none. The old 0x426 ends
    // the lock in N+44.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (10, 10));
    let t = enemy_caster(&mut sim, (40, 10));
    set_hp(&mut sim, m, 600);
    drain_tick(&mut sim, drain_cmd(m, t));
    kit::run_ticks(&mut sim, 9);
    let events = drain_tick(&mut sim, drain_cmd(m, t));
    assert_eq!(sounds(&events, 0x1E).len(), 1, "second cast in N+10");
    assert_eq!(pending(&sim, m, msg::EFFECT), 2, "old timers stay");
    assert_eq!(pending(&sim, m, msg::END), 2);
    assert!(spell_success(&sim, m).is_some());
    kit::run_ticks(&mut sim, 11);
    assert_eq!(mana(&sim, m), 200);
    // N+22: first payment, one drain.
    kit::run_ticks(&mut sim, 1);
    assert_eq!((mana(&sim, m), mana(&sim, t), hp(&sim, m)), (150, 0, 700));
    assert_eq!(spell_success(&sim, m), None);
    // N+31: the regeneration (0.1 per tick) has rounded the fraction 150.05 to 151 since N+22.
    // Target back to 100: a second drain would be visible at HP 800 and target 0.
    kit::run_ticks(&mut sim, 9);
    assert_eq!(mana(&sim, m), 151);
    set_mana(&mut sim, t, 100);
    // N+32: second payment 151 -> 101 instead of 100 as in re/spells 9.4 (there without regeneration).
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!(mana(&sim, m), 101);
    assert_eq!((mana(&sim, t), hp(&sim, m)), (100, 700), "only one drain");
    assert_eq!(sounds(&events, 0x1F).len(), 1);
    // N+44: the old 0x426 ends the lock (the new animation keeps running).
    kit::run_ticks(&mut sim, 11);
    assert!(locked(&sim, m));
    kit::run_ticks(&mut sim, 1);
    assert!(!locked(&sim, m) && attack_on(&sim, m));
    assert_eq!(
        pending(&sim, m, msg::END),
        1,
        "the new 0x426 is still waiting"
    );
}

#[test]
fn drain_without_target_still_pays() {
    // C-303, re/spells 8 no. 7: if the target is missing at 0x326, the cost is gone anyway; 0xB7F
    // does nothing without a source.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (10, 10));
    let t = enemy_caster(&mut sim, (40, 10));
    set_hp(&mut sim, m, 600);
    drain_tick(&mut sim, drain_cmd(m, t));
    crate::world::queue_destroy(&mut sim, t);
    kit::run_ticks(&mut sim, 1);
    assert!(sim.object(t).is_none());
    kit::run_ticks(&mut sim, 20);
    let events = kit::run_ticks_collect(&mut sim, 1);
    assert_eq!((mana(&sim, m), hp(&sim, m)), (150, 600));
    assert_eq!(sounds(&events, 0x1F).len(), 1);
    assert_eq!(spell_success(&sim, m), None);
    // Target already gone at the command: the command fails at `target_fits` (checksum unchanged).
    let before = checksum(sim.state);
    run(&mut sim, 0, drain_cmd(m, t));
    assert_eq!(checksum(sim.state), before);
}

#[test]
fn a_mounted_missionary_does_not_drain() {
    // C-306: mounted, the cast ends with 0 before anything else (no sound, no animation, no
    // mana); the riding branch of kind 5 (animation 1) is thus dead. The success message is
    // nevertheless already in module 38 (the envelope sets it before the cast).
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (10, 10));
    let t = enemy_caster(&mut sim, (40, 10));
    sim.object_mut(m).unwrap().status |= status::ISMOUNTED;
    let events = drain_tick(&mut sim, drain_cmd(m, t));
    assert!(sounds(&events, 0x1E).is_empty());
    assert!(!locked(&sim, m));
    assert_eq!(spell(&sim, m).unwrap().art, ART_DRAIN);
    assert!(spell_success(&sim, m).is_some());
    kit::run_ticks(&mut sim, 30);
    assert_eq!((mana(&sim, m), mana(&sim, t)), (200, 100));
}

#[test]
fn the_drain_faces_the_target() {
    // C-305 deviated: the original turns to a look-at point that is never written; the
    // reimplementation takes the target position at the cast. Target behind the missionary (-x,
    // out of scan range): he turns, 0x226 comes only afterwards; until then the face order stays.
    let rules = kit::spell_rules();
    let mut state = kit::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let m = missionary(&mut sim, (40, 20));
    let t = enemy_caster(&mut sim, (10, 20));
    let grid = sim.object(m).unwrap().grid;
    let target = ground_of(&sim, t);
    let order = face_order(&sim, m, target);
    let toward = facing_dir_with_offset((grid.x.0, grid.z.0, 0), (order.arg(3), order.arg(4)), 0);
    let events = drain_tick(&mut sim, drain_cmd(m, t));
    assert_eq!(sounds(&events, 0x1E).len(), 1, "sound at the cast");
    assert_eq!(
        crate::module::walk::walk(&sim, m).unwrap().target_dir,
        toward
    );
    let mut started = None;
    for k in 0..40 {
        if locked(&sim, m) {
            started = Some(k);
            break;
        }
        assert_eq!(
            find_to(&sim, m),
            Some(order.clone()),
            "face order until 0x226"
        );
        kit::run_ticks(&mut sim, 1);
    }
    // Measured like for the explosion: 0x226 START_TURN ticks after N.
    assert_eq!(started, Some(START_TURN));
    assert_eq!(sim.object(m).unwrap().grid, grid, "position unchanged");
    assert_eq!(
        sim.object(m).unwrap().direction.0,
        toward,
        "turned to the target"
    );
    assert_eq!(sim.object(m).unwrap().anim.wanted, 9);
}
