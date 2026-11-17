//! Tests of packing, unpacking and the error cases (re/tents 3.1-3.6, 9.1-9.3) in the test world
//! [`super::testkit`]. One object tick is two `tick::step`s; direct `send` calls run outside the
//! tick.

use nml_formats::targets::MatchFilter;

use super::testkit::*;
use super::transport::{self, TransportTent};
use super::*;
use crate::fog::Fog;
use crate::fx::{Fx, FxVec3};
use crate::message::Message;
use crate::messaging::send;
use crate::module::building::{self, msg as building_msg};
use crate::module::quarter;
use crate::object::status as object_status;
use crate::path::grid::COST_FREE;
use crate::state::SimEvent;

/// Snapped position of the tents (re/tents 9.2: 1601 -> `[37,38,38,39]`, 1600 -> `[37,37,40,40]`).
const SNAP: (f32, f32) = (5000.0, 5000.0);

fn tent_of(sim: &Sim, id: ObjectId) -> Tent {
    match &sim.module(id, INDEX).expect("module 42").kind {
        ModuleKind::Tent(t) => t.clone(),
        k => panic!("not a tent: {k:?}"),
    }
}

fn transport_of(sim: &Sim, id: ObjectId) -> (u32, TransportTent) {
    let m = sim.module(id, transport::INDEX).expect("module 35");
    match &m.kind {
        ModuleKind::TransportTent(t) => (m.base.flags, t.clone()),
        k => panic!("not a travois: {k:?}"),
    }
}

fn pack_msg(sim: &mut Sim, id: ObjectId) -> i32 {
    send(sim, id, &Message::new(msg::PACK, &[]))
}

/// `n` object ticks; the events of the steps.
fn object_ticks(sim: &mut Sim, n: usize) -> Vec<SimEvent> {
    let mut events = Vec::new();
    for _ in 0..2 * n {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

fn in_collision(sim: &Sim, id: ObjectId) -> bool {
    let c = &sim.state.collision;
    c.all.iter().chain(c.movers.iter()).any(|l| l.contains(&id))
}

fn inside(rect: [i32; 4], (x, z): (i32, i32)) -> bool {
    (rect[0]..=rect[2]).contains(&x) && (rect[1]..=rect[3]).contains(&z)
}

fn travois_count(sim: &Sim, slot: u8) -> usize {
    sim.state.tables[usize::from(slot)]
        .objects
        .iter()
        .flatten()
        .filter(|o| o.guid == TRAVOIS)
        .count()
}

/// `0x223 {x, z}` to the travois (walk success of the command `0x2E`).
fn arrive(sim: &mut Sim, travois: ObjectId, (x, z): (i32, i32)) -> i32 {
    send(
        sim,
        travois,
        &Message::new(transport::msg::ARRIVED, &[x, z]),
    )
}

/// Packed, snapped tent after the fade-out: `(tent, travois)`.
fn packed(sim: &mut Sim, guid: u32) -> (ObjectId, ObjectId) {
    let tent = spawn_snapped(sim, guid, 0, SNAP);
    assert_eq!(pack_msg(sim, tent), 0);
    object_ticks(sim, 25);
    assert_eq!(tent_of(sim, tent).status, status::REST);
    (tent, tent_of(sim, tent).travois)
}

/// Grid position `(x, z)` plus `(dx, dz)` whole tiles (destinations from the UI are snapped).
fn grid_plus(sim: &Sim, id: ObjectId, (dx, dz): (i32, i32)) -> (i32, i32) {
    let g = sim.object(id).unwrap().grid;
    (g.x.0 + (dx << 12), g.z.0 + (dz << 12))
}

fn shifted(r: [i32; 4], (dx, dz): (i32, i32)) -> [i32; 4] {
    [r[0] + dx, r[1] + dz, r[2] + dx, r[3] + dz]
}

fn tiles(r: [i32; 4]) -> impl Iterator<Item = (i32, i32)> {
    (r[1]..=r[3]).flat_map(move |z| (r[0]..=r[2]).map(move |x| (x, z)))
}

fn path_blocked(sim: &Sim, (x, z): (i32, i32)) -> bool {
    sim.state.path.cell_cost(x, z, 0) != COST_FREE
}

fn handovers(events: &[SimEvent]) -> Vec<SimEvent> {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::SelectionHandover { .. }))
        .cloned()
        .collect()
}

// QUIRK W-248, W-249: kept
#[test]
fn packing_hides_the_tent_and_creates_a_travois() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    let (rect, occ_rect) = {
        let b = building::building(&sim, tent).unwrap();
        (b.rect, b.occupancy_rect)
    };
    assert_eq!(rect, [37, 38, 38, 39], "snapped (re/tents 9.2)");
    let (hp, bits_before, color) = {
        let o = sim.object(tent).unwrap();
        (sim.attribute(tent, 14), o.render_bits, o.color)
    };
    assert!(in_collision(&sim, tent));

    assert_eq!(pack_msg(&mut sim, tent), 0);

    // Travois: new object 1717 on slot 0, color of the tent, rotation 0.
    let t = tent_of(&sim, tent);
    let travois = t.travois;
    let o = sim.object(travois).expect("travois created");
    assert_eq!((o.guid, travois.slot(), o.color), (TRAVOIS, 0, color));
    assert_eq!(o.rotation, [0.0; 3]);
    // W-249: created in the still locked footprint, set by walk init to the nearest free tile
    // next to it.
    assert!(
        !inside(rect, o.tile),
        "travois {:?} in the footprint",
        o.tile
    );
    let near = [rect[0] - 1, rect[1] - 1, rect[2] + 1, rect[3] + 1];
    assert!(inside(near, o.tile), "travois {:?} not at the edge", o.tile);
    let (flags, tt) = transport_of(&sim, travois);
    assert_eq!(tt.tent, tent);
    assert_eq!(flags, transport::TRANSPORTING_TENT);
    // W-248: without "finished" no type counter +1.
    assert_eq!(sim.state.guid_counts[0].get(&TRAVOIS).copied(), None);
    assert_eq!(
        send(
            &mut sim,
            travois,
            &Message::new(transport::msg::GET_TENT, &[])
        ),
        tent.as_i32()
    );

    // Tent: the same object, hidden.
    let o = sim.object(tent).expect("tent stays");
    assert_eq!(sim.attribute(tent, 14), hp);
    assert_eq!(o.render_bits, 4);
    assert!(!o.selectable);
    assert_eq!(o.progress_scale, 1.0);
    assert!(!in_collision(&sim, tent));
    let b = building::building(&sim, tent).unwrap();
    assert!(b.is_sleeping && !b.blocked);
    assert_eq!(b.render_bits_backup, bits_before);
    for z in occ_rect[1]..=occ_rect[3] {
        for x in occ_rect[0]..=occ_rect[2] {
            assert!(!sim.state.occupancy.test(x, z), "occupancy ({x}, {z})");
        }
    }
    for z in rect[1]..=rect[3] {
        for x in rect[0]..=rect[2] {
            assert_eq!(
                sim.state.path.cell_cost(x, z, 0),
                COST_FREE,
                "path ({x}, {z})"
            );
        }
    }
    assert_eq!(
        send(
            &mut sim,
            tent,
            &Message::new(building_msg::IS_SLEEPING, &[])
        ),
        1
    );
    assert_eq!(
        t,
        Tent {
            travois_guid: TRAVOIS,
            status: status::FADE_OUT,
            counter: FADE_MS,
            delta_bits: DELTA_OUT,
            backup_show: 0,
            travois,
        }
    );
}

// QUIRK D-228: kept
#[test]
fn the_fade_is_bit_exact() {
    // re/tents 9.1: fade-out via the simulation. Two f32 roundings per tick (24-bit x87);
    // a single rounding would end at 0xB43F_E000.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    assert_eq!(pack_msg(&mut sim, tent), 0);
    let mut out = Vec::new();
    for k in 1..=25 {
        object_ticks(&mut sim, 1);
        if [1, 2, 13, 24, 25].contains(&k) {
            out.push(sim.object(tent).unwrap().progress_scale.to_bits());
        }
    }
    assert_eq!(
        out,
        [
            0x3F75_C28F,
            0x3F6B_851E,
            0x3EF5_C287,
            0x3D23_D6DA,
            0xB440_0000
        ]
    );
    // Fade-in via the simulation: unpacking one tile further, from factor 0.0.
    let travois = tent_of(&sim, tent).travois;
    let g = sim.object(tent).unwrap().grid;
    assert_eq!(arrive(&mut sim, travois, (g.x.0 + (3 << 12), g.z.0)), 0);
    assert_eq!(sim.object(tent).unwrap().progress_scale, 0.0);
    let mut inn = Vec::new();
    for k in 1..=25 {
        object_ticks(&mut sim, 1);
        if [1, 2, 13, 24, 25].contains(&k) {
            inn.push(sim.object(tent).unwrap().progress_scale.to_bits());
        }
    }
    assert_eq!(
        inn,
        [
            0x3D23_D70A,
            0x3DA3_D70A,
            0x3F05_1EB7,
            0x3F75_C292,
            0x3F80_0001
        ]
    );
}

#[test]
fn the_pack_finishes_in_the_25th_object_tick() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    pack_msg(&mut sim, tent);
    let travois = tent_of(&sim, tent).travois;
    let early = object_ticks(&mut sim, 24);
    let t = tent_of(&sim, tent);
    assert_eq!((t.status, t.counter), (status::FADE_OUT, 100));
    assert_eq!(sim.object(tent).unwrap().render_bits, 4);
    assert!(handovers(&early).is_empty());

    let last = object_ticks(&mut sim, 1);
    let t = tent_of(&sim, tent);
    assert_eq!((t.status, t.counter), (status::REST, 0));
    assert_eq!(sim.object(tent).unwrap().render_bits, 0, "backup_show");
    assert_eq!(
        handovers(&last),
        [SimEvent::SelectionHandover {
            from: tent,
            to: travois,
            only_if_empty: false,
        }]
    );
    // Afterwards the tent rests: no further step, no further event.
    let later = object_ticks(&mut sim, 3);
    assert!(handovers(&later).is_empty());
    assert_eq!(
        sim.object(tent).unwrap().progress_scale.to_bits(),
        0xB440_0000
    );
}

#[test]
fn packing_empties_the_quarter_first() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, CHIEF_TENT, 0, SNAP);
    let rect = building::building(&sim, tent).unwrap().rect;
    assert_eq!(rect, [37, 37, 40, 40]);
    let units = [
        spawn(&mut sim, COLONIST, 0, (30, 30)),
        spawn(&mut sim, COLONIST, 0, (31, 30)),
    ];
    for u in units {
        assert_eq!(quarter::accept_unit(&mut sim, tent, u), 0);
    }
    let occupants = |sim: &Sim| match &sim.module(tent, quarter::INDEX).unwrap().kind {
        ModuleKind::Quarter(q) => q.occupants.clone(),
        _ => unreachable!(),
    };
    assert_eq!(occupants(&sim), units);

    assert_eq!(pack_msg(&mut sim, tent), 0);
    assert!(occupants(&sim).is_empty());
    for u in units {
        let o = sim.object(u).unwrap();
        assert_eq!(o.status & object_status::QUARTERED, 0);
        // Got out while the footprint was still locked: outside.
        assert!(!inside(rect, o.tile), "occupant {u:?} at {:?}", o.tile);
    }
    assert!(building::building(&sim, tent).unwrap().is_sleeping);
}

// QUIRK W-247: deviation
#[test]
fn packing_during_a_fade_does_nothing() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    pack_msg(&mut sim, tent);
    object_ticks(&mut sim, 3);
    let before = tent_of(&sim, tent);
    assert_eq!(before.status, status::FADE_OUT);
    assert_eq!(pack_msg(&mut sim, tent), 0);
    assert_eq!(tent_of(&sim, tent), before);
    assert_eq!(travois_count(&sim, 0), 1);
}

// QUIRK D-228: kept
// QUIRK W-247: deviation
#[test]
fn a_construction_site_can_be_packed() {
    // W-247, D-228: no check of the construction state; the crossfade overwrites the progress.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = site(&mut sim, DWELLING_TENT, 0, (20, 20));
    assert!(sim.object(tent).unwrap().has_module(INDEX, false));
    assert!(sim.object(tent).unwrap().has_module(building::INDEX, false));
    let build = |sim: &mut Sim| {
        send(
            sim,
            tent,
            &Message::new(building_msg::BUILD_TICK, &[100, 0]),
        )
    };
    assert_eq!(build(&mut sim), 1, "construction site builds");
    let built = sim.object(tent).unwrap().progress_scale;
    assert!(built < 1.0);

    assert_eq!(pack_msg(&mut sim, tent), 0);
    assert!(building::building(&sim, tent).unwrap().is_sleeping);
    assert_eq!(sim.object(tent).unwrap().progress_scale, 1.0);
    assert_eq!(build(&mut sim), 0, "sleeping: build tick answers 0");
    object_ticks(&mut sim, 1);
    assert_eq!(
        sim.object(tent).unwrap().progress_scale.to_bits(),
        0x3F75_C28F
    );
    object_ticks(&mut sim, 24);
    assert_eq!(build(&mut sim), 0);
    assert_eq!(
        sim.object(tent).unwrap().progress_scale.to_bits(),
        0xB440_0000
    );
    assert_eq!(tent_of(&sim, tent).status, status::REST);
}

// QUIRK W-256, W-257: kept
#[test]
fn packing_keeps_tile_map_and_sight_stamp() {
    // W-256, W-257: the BuildingTileMap and the sight stamp stay at the old position.
    let mut rules = rules();
    rules
        .defaults
        .get_mut(&DWELLING_TENT)
        .unwrap()
        .insert(18, 12);
    let mut state = state(&rules);
    // Without the revealed map: only the stamps of the objects count.
    state.fog = Fog::new(64, 64);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    let map = |sim: &Sim| sim.state.tile_map.occupied().collect::<Vec<_>>();
    let before = map(&sim);
    assert!(
        before.iter().any(|&(_, _, hid)| hid == tent.as_i32()),
        "tent in the BuildingTileMap"
    );
    let tile = sim.object(tent).unwrap().tile;

    pack_msg(&mut sim, tent);
    object_ticks(&mut sim, 25);
    assert_eq!(map(&sim), before);
    // Many full fog cycles after hiding: the tent keeps stamping at its tile.
    sim.state.fog.seen.clear();
    object_ticks(&mut sim, 40);
    assert_eq!(sim.object(tent).unwrap().tile, tile);
    assert_ne!(sim.state.fog.seen.get(tile.0, tile.1).unwrap() & 1, 0);
    assert_eq!(sim.state.fog.seen.get(5, 5).unwrap() & 1, 0);
}

// QUIRK W-248, W-258: kept
#[test]
fn unpacking_on_another_tile_restores_the_tent() {
    // re/tents 9.3 steps 3-4.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    let (old_rect, mask, old_lots) = {
        let b = building::building(&sim, tent).unwrap();
        (b.rect, b.mask.clone(), b.parking_lots.clone())
    };
    let bits_before = sim.object(tent).unwrap().render_bits;
    assert_eq!(pack_msg(&mut sim, tent), 0);
    object_ticks(&mut sim, 25);
    let travois = tent_of(&sim, tent).travois;
    let step = (-10, -8);
    let (x, z) = grid_plus(&sim, tent, step);
    let rect = shifted(old_rect, step);

    assert_eq!(arrive(&mut sim, travois, (x, z)), 0);
    // Tent: at `(x, z)`, in the collision manager again, occupancy set directly, freed list still
    // the old tiles (re/occupancy 5.3), render bits 4, fade-in.
    let o = sim.object(tent).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (x, z));
    assert_eq!(o.render_bits, 4);
    assert!(!o.selectable);
    assert!(in_collision(&sim, tent));
    assert!(tiles(rect).all(|(x, z)| sim.state.occupancy.test(x, z)));
    let b = building::building(&sim, tent).unwrap();
    assert!(!b.freed.is_empty() && b.freed.iter().all(|&t| inside(old_rect, t)));
    assert!(b.is_sleeping);
    let t = tent_of(&sim, tent);
    assert_eq!(
        (t.status, t.counter, t.delta_bits),
        (status::FADE_IN, FADE_MS, DELTA_IN)
    );
    // Travois: unselectable, without tent and flag.
    assert!(!sim.object(travois).unwrap().selectable);
    let (flags, tt) = transport_of(&sim, travois);
    assert_eq!((flags, tt.tent), (0, ObjectId::NONE));

    // The tent is the first object: tick group 0, so in the first step of an object tick.
    assert_eq!(sim.object(tent).unwrap().tick_group, 0);
    let mut events = object_ticks(&mut sim, 24);
    events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    assert_eq!(sim.state.phase, 1);
    let b = building::building(&sim, tent).unwrap();
    assert_eq!((b.rect, b.occupancy_rect), (rect, rect));
    assert!(b.freed.is_empty(), "freed list empty after 0x2F03");
    assert_eq!(b.mask, mask);
    assert!(b.blocked);
    for t in tiles(rect) {
        assert!(path_blocked(&sim, t), "path {t:?} blocked");
        assert!(sim.state.occupancy.test(t.0, t.1));
    }
    for t in tiles(old_rect) {
        assert!(!path_blocked(&sim, t), "old path {t:?} free");
        assert!(!sim.state.occupancy.test(t.0, t.1));
    }
    let lots = building::parking::build_parking_lots(
        &mask,
        (rect[0] << 12) - 0x800,
        (rect[1] << 12) - 0x800,
    );
    assert_eq!(b.parking_lots, lots);
    assert_ne!(b.parking_lots, old_lots);
    let y = sim.object(tent).unwrap().grid.y;
    assert_eq!(b.meeting_point, FxVec3::new(Fx(x), y, Fx(z - 0x3000)));
    assert_eq!(b.meeting_target, ObjectId::NONE);
    assert!(!b.is_sleeping);
    let o = sim.object(tent).unwrap();
    assert!(o.selectable);
    assert_eq!(o.render_bits, bits_before);
    assert_eq!(tent_of(&sim, tent).status, status::REST);
    assert_eq!(
        handovers(&events),
        [SimEvent::SelectionHandover {
            from: travois,
            to: tent,
            only_if_empty: true,
        }]
    );
    // The travois is in the destroy list and gone in the next phase-0 step; never counted without
    // "finished", -1 on destruction (W-248).
    assert!(sim.object(travois).is_some());
    assert_eq!(sim.state.destroy_queue, [travois]);
    crate::tick::step(sim.state, sim.rules, &[]);
    assert_eq!(sim.state.phase, 0);
    assert!(sim.object(travois).is_none());
    assert_eq!(sim.state.guid_counts[0].get(&TRAVOIS).copied(), Some(-1));
    // The tent keeps remembering the ID (`+0x84` stays).
    assert_eq!(tent_of(&sim, tent).travois, travois);
}

// QUIRK W-89: kept
#[test]
fn unpacking_on_the_same_tile_leaves_the_tent_outside_the_collision_manager() {
    // W-89, re/tents 9.3 step 5.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let filter = MatchFilter {
        mode: 0x10 | 0x8,
        guid: DWELLING_TENT as i32,
        ..MatchFilter::default()
    };
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let (x, z) = grid_plus(&sim, tent, (0, 0));
    assert!(!in_collision(&sim, tent));

    assert_eq!(arrive(&mut sim, travois, (x, z)), 0);
    object_ticks(&mut sim, 25);
    let o = sim.object(tent).unwrap();
    assert!(o.selectable && o.render_bits != 0);
    assert!(!building::building(&sim, tent).unwrap().is_sleeping);
    assert!(!in_collision(&sim, tent));
    assert_eq!(
        crate::collision::find_nearest_matching(&mut sim, (x, z), &filter, -1, 0, -1),
        None
    );
    // Counter-check: a second tent, unpacked on another tile, is found by the search.
    let other = spawn_snapped(&mut sim, DWELLING_TENT, 0, (2000.0, 2000.0));
    assert_eq!(pack_msg(&mut sim, other), 0);
    object_ticks(&mut sim, 25);
    let travois = tent_of(&sim, other).travois;
    let target = grid_plus(&sim, other, (2, 0));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    object_ticks(&mut sim, 25);
    assert!(in_collision(&sim, other));
    assert_eq!(
        crate::collision::find_nearest_matching(&mut sim, (x, z), &filter, -1, 0, -1),
        Some(other)
    );
}

// QUIRK W-252: kept
#[test]
fn the_next_pack_clears_the_whole_rect() {
    // W-252: after unpacking the freed list is empty; the next release runs over the whole
    // rectangle without a freed list. A bit that another building (e.g. with its
    // `0x3303` border) sets there is indistinguishable in the shared occupancy - `0x22A`
    // has already occupied the whole rectangle, setting changes nothing - and is lost with it.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    object_ticks(&mut sim, 25);
    let rect = building::building(&sim, tent).unwrap().rect;
    assert!(building::building(&sim, tent).unwrap().freed.is_empty());
    let foreign = (rect[2], rect[3]);
    assert!(
        sim.state.occupancy.test(foreign.0, foreign.1),
        "already occupied"
    );
    sim.state.occupancy.set(foreign.0, foreign.1, true);

    assert_eq!(pack_msg(&mut sim, tent), 0);
    assert!(!sim.state.occupancy.test(foreign.0, foreign.1));
    assert!(tiles(rect).all(|(x, z)| !sim.state.occupancy.test(x, z)));
}

// QUIRK W-253: kept
#[test]
fn placing_resets_a_widened_occupancy_rect() {
    // W-253: the `0x3303` border is lost on unpacking.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    assert_eq!(
        send(
            &mut sim,
            tent,
            &Message::new(building_msg::EXPAND_OCCUPANCY, &[1])
        ),
        0
    );
    let b = building::building(&sim, tent).unwrap();
    assert_ne!(b.occupancy_rect, b.rect);
    assert_eq!(pack_msg(&mut sim, tent), 0);
    object_ticks(&mut sim, 25);
    let travois = tent_of(&sim, tent).travois;
    let target = grid_plus(&sim, tent, (-6, -6));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    object_ticks(&mut sim, 25);
    let b = building::building(&sim, tent).unwrap();
    assert_eq!(b.occupancy_rect, b.rect);
    let ring = [b.rect[0] - 1, b.rect[1] - 1, b.rect[2] + 1, b.rect[3] + 1];
    for (x, z) in tiles(ring) {
        assert_eq!(sim.state.occupancy.test(x, z), inside(b.rect, (x, z)));
    }
}

// QUIRK W-254: kept
#[test]
fn an_alarm_post_gets_the_model_rect_with_its_init_mask() {
    // W-254: init sets rectangle and mask to the origin tile; `0x2F03` takes the
    // model rectangle (re/tents 9.2: snapped `[37,38,38,39]`).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, ALARM_POST);
    let init_rect = building::building(&sim, tent).unwrap().rect;
    assert_eq!(init_rect, [38, 38, 38, 38]);
    let step = (-6, -4);
    let target = grid_plus(&sim, tent, step);
    assert_eq!(arrive(&mut sim, travois, target), 0);
    object_ticks(&mut sim, 25);
    let rect = shifted([37, 38, 38, 39], step);
    assert_eq!(building::building(&sim, tent).unwrap().rect, rect);
    for (x, z) in tiles(rect) {
        assert!(sim.state.occupancy.test(x, z), "occupancy ({x}, {z})");
        assert_eq!(
            path_blocked(&sim, (x, z)),
            (x, z) == (rect[0], rect[1]),
            "path ({x}, {z})"
        );
    }
}

// QUIRK W-255: kept
#[test]
fn an_unsnapped_position_keeps_the_init_mask() {
    // W-255, re/tents 9.2: unpacked raw at world (1000, y, 2000) -> rectangle 3 x 3 with the
    // 2 x 2 mask from init.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    assert_eq!(arrive(&mut sim, travois, (32_000, 64_000)), 0);
    object_ticks(&mut sim, 25);
    let rect = building::building(&sim, tent).unwrap().rect;
    assert_eq!(rect, [6, 14, 8, 16]);
    for (x, z) in tiles(rect) {
        assert!(sim.state.occupancy.test(x, z), "occupancy ({x}, {z})");
        assert_eq!(
            path_blocked(&sim, (x, z)),
            x <= 7 && z <= 15,
            "path ({x}, {z})"
        );
    }
}

// QUIRK W-256, W-258: kept
#[test]
fn unpacking_keeps_the_tile_map_and_resets_the_meeting_point() {
    // W-256: the BuildingTileMap stays at the old position, no stamp at the new one. W-258:
    // meeting point origin - 3 tiles z, meeting target -1.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    let colonist = spawn(&mut sim, COLONIST, 0, (20, 20));
    assert_eq!(
        send(
            &mut sim,
            tent,
            &Message::new(building_msg::SET_MEETING_POINT, &[-1, colonist.as_i32()])
        ),
        0
    );
    assert_eq!(
        building::building(&sim, tent).unwrap().meeting_target,
        colonist
    );
    let map = |sim: &Sim| sim.state.tile_map.occupied().collect::<Vec<_>>();
    let before = map(&sim);
    assert!(before.iter().any(|&(_, _, hid)| hid == tent.as_i32()));
    pack_msg(&mut sim, tent);
    object_ticks(&mut sim, 25);
    let travois = tent_of(&sim, tent).travois;
    let (x, z) = grid_plus(&sim, tent, (-8, 3));
    assert_eq!(arrive(&mut sim, travois, (x, z)), 0);
    object_ticks(&mut sim, 25);
    assert_eq!(map(&sim), before);
    let y = sim.object(tent).unwrap().grid.y;
    let b = building::building(&sim, tent).unwrap();
    assert_eq!(b.meeting_point, FxVec3::new(Fx(x), y, Fx(z - 0x3000)));
    assert_eq!(b.meeting_target, ObjectId::NONE);
}

// ---- Errors and edge cases (re/tents 3.6, 9.3 steps 6-7) ----

fn travois_ids(sim: &Sim, slot: u8) -> Vec<ObjectId> {
    sim.state.tables[usize::from(slot)]
        .objects
        .iter()
        .flatten()
        .filter(|o| o.guid == TRAVOIS)
        .map(|o| o.id)
        .collect()
}

/// Up to and including the next phase-0 step (destroy list, then owner change list).
fn next_phase_0(sim: &mut Sim) -> Vec<SimEvent> {
    let mut events = crate::tick::step(sim.state, sim.rules, &[]);
    while sim.state.phase != 0 {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    }
    events
}

/// New ID from the event `OwnerChanged` for `old`.
fn new_id_of(events: &[SimEvent], old: ObjectId) -> Option<ObjectId> {
    events.iter().find_map(|e| match e {
        SimEvent::OwnerChanged { old: o, new } if *o == old => Some(*new),
        _ => None,
    })
}

/// Living space of slot 0 unclamped in whole units.
fn living_space(sim: &Sim) -> i32 {
    sim.state.players[0].resources_fx[crate::economy::resource::LIVING_SPACE] >> 12
}

fn object_total(sim: &Sim) -> usize {
    sim.state
        .tables
        .iter()
        .map(|t| t.objects.iter().flatten().count())
        .sum()
}

#[test]
fn a_failed_walk_only_logs() {
    // re/tents 3.6 "target unreachable": `0x323` -> log, 0.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let before = crate::save::checksum(sim.state);
    let (t, tt) = (tent_of(&sim, tent), transport_of(&sim, travois));
    assert_eq!(
        send(
            &mut sim,
            travois,
            &Message::new(transport::msg::WALK_FAILED, &[])
        ),
        0
    );
    assert_eq!(crate::save::checksum(sim.state), before);
    assert_eq!((tent_of(&sim, tent), transport_of(&sim, travois)), (t, tt));
    assert!(sim.object(travois).unwrap().selectable);
    // A new run succeeds.
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    assert_eq!(tent_of(&sim, tent).status, status::FADE_IN);
}

// QUIRK W-251: kept
#[test]
fn an_occupied_target_moves_the_hidden_tent() {
    // W-251: position set before the place check; "built on" -> -1, the tent stays hidden at the
    // target tile, in the collision manager after the tile change.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let old_tile = sim.object(tent).unwrap().tile;
    // A second dwelling tent exactly six tiles further: same snapping, rectangle shifted by 6.
    let cell = rules.map.cell[0] as f32;
    let other = spawn_snapped(&mut sim, DWELLING_TENT, 0, (SNAP.0 - 6.0 * cell, SNAP.1));
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(
        building::building(&sim, other).unwrap().rect,
        shifted([37, 38, 38, 39], (-6, 0))
    );
    assert!(!in_collision(&sim, tent));

    assert_eq!(arrive(&mut sim, travois, target), -1);
    let o = sim.object(tent).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), target);
    assert_ne!(o.tile, old_tile);
    assert!(in_collision(&sim, tent));
    assert_eq!(o.render_bits, 0);
    assert!(!o.selectable);
    assert!(building::building(&sim, tent).unwrap().is_sleeping);
    assert_eq!(tent_of(&sim, tent).status, status::REST);
    // Travois keeps the tent and flag and stays selectable.
    let (flags, tt) = transport_of(&sim, travois);
    assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));
    assert!(sim.object(travois).unwrap().selectable);
    // Even after the duration of a crossfade: nothing runs.
    object_ticks(&mut sim, 30);
    assert_eq!(sim.object(tent).unwrap().render_bits, 0);
    assert!(building::building(&sim, tent).unwrap().is_sleeping);
}

// QUIRK W-259: kept
#[test]
fn a_foreign_travois_hid_fails_once() {
    // W-259: after a change of owner the travois has a new ID; `0x22A` takes it over
    // and fails, the second attempt succeeds.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    crate::world::queue_owner_change(&mut sim, travois, 1);
    let events = next_phase_0(&mut sim);
    let new = new_id_of(&events, travois).expect("owner changed");
    assert_eq!(new.slot(), 1);
    let target = grid_plus(&sim, tent, (-6, 0));
    let grid = sim.object(tent).unwrap().grid;

    assert_eq!(arrive(&mut sim, new, target), -1);
    assert_eq!(tent_of(&sim, tent).travois, new, "foreign HID taken over");
    assert_eq!(sim.object(tent).unwrap().grid, grid, "no effect");
    let (flags, tt) = transport_of(&sim, new);
    assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));

    assert_eq!(arrive(&mut sim, new, target), 0);
    assert_eq!(tent_of(&sim, tent).status, status::FADE_IN);
    let (flags, tt) = transport_of(&sim, new);
    assert_eq!((flags, tt.tent), (0, ObjectId::NONE));
}

// QUIRK W-250: kept
#[test]
fn unpacking_during_the_fade_loses_the_tent() {
    // W-250, re/tents 9.3 step 6: arrival before the 25th tick -> `0x22A` answers 0 without effect,
    // the travois releases the link anyway.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
    assert_eq!(pack_msg(&mut sim, tent), 0);
    let travois = tent_of(&sim, tent).travois;
    object_ticks(&mut sim, 10);
    let grid = sim.object(tent).unwrap().grid;
    let target = grid_plus(&sim, tent, (-6, 0));

    assert_eq!(arrive(&mut sim, travois, target), 0);
    let t = tent_of(&sim, tent);
    assert_eq!((t.status, t.counter), (status::FADE_OUT, 1500));
    assert_eq!(sim.object(tent).unwrap().grid, grid);
    assert!(!sim.object(travois).unwrap().selectable);
    let (flags, tt) = transport_of(&sim, travois);
    assert_eq!((flags, tt.tent), (0, ObjectId::NONE));

    // The fade-out ends regularly, after that everything stays so forever.
    object_ticks(&mut sim, 15);
    let check = |sim: &Sim| {
        let o = sim.object(tent).unwrap();
        assert_eq!((o.grid, o.render_bits, o.selectable), (grid, 0, false));
        assert!(building::building(sim, tent).unwrap().is_sleeping);
        assert_eq!(tent_of(sim, tent).status, status::REST);
        assert!(!sim.object(travois).unwrap().selectable);
        assert_eq!(transport_of(sim, travois), (0, TransportTent::default()));
    };
    check(&sim);
    object_ticks(&mut sim, 100);
    check(&sim);
    // Another run of the travois fizzles (no tent).
    assert_eq!(arrive(&mut sim, travois, target), 0);
    check(&sim);
}

// QUIRK W-260: kept
#[test]
fn a_dying_travois_removes_the_tent() {
    // W-260, re/tents 9.3 step 7: death -> owner change to 8 in the next phase-0 step -> the
    // tent in the destroy list, removed in the phase-0 step after that.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let space = living_space(&sim);
    let total = object_total(&sim);

    // Death (`PropertyDying_StartDying`, `0x110`): `QueueOwnerChange(travois, 8)`.
    assert_eq!(send(&mut sim, travois, &Message::new(0x110, &[])), 0);
    let mut events = next_phase_0(&mut sim);
    let corpse = new_id_of(&events, travois).expect("owner changed");
    assert_eq!(corpse.slot(), crate::id::NEUTRAL_SLOT);
    assert!(
        sim.object(tent).is_some(),
        "not until the next phase-0 step"
    );
    assert_eq!(sim.state.destroy_queue, [tent]);
    let (flags, tt) = transport_of(&sim, corpse);
    assert_eq!(
        (flags, tt.tent),
        (transport::TRANSPORTING_TENT, ObjectId::NONE),
        "flag stays"
    );

    let later = next_phase_0(&mut sim);
    assert!(sim.object(tent).is_none());
    assert!(later.contains(&SimEvent::Destroyed { id: tent }));
    // No death flow, no debris: nothing is created, the living space drops by 15.
    events.extend(later);
    assert!(!events.iter().any(|e| matches!(e, SimEvent::Created { .. })));
    assert_eq!(object_total(&sim), total - 1);
    assert_eq!(living_space(&sim), space - 15);
}

// QUIRK W-260: kept
#[test]
fn an_owner_change_to_another_player_keeps_the_tent() {
    // W-260: owner change != 8 has no effect; the tent stays with slot 0.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    crate::world::queue_owner_change(&mut sim, travois, 1);
    let events = next_phase_0(&mut sim);
    let new = new_id_of(&events, travois).expect("owner changed");
    object_ticks(&mut sim, 5);
    assert!(sim.object(tent).is_some());
    assert_eq!(tent.slot(), 0);
    assert_eq!(tent_of(&sim, tent).travois, travois);
    let (flags, tt) = transport_of(&sim, new);
    assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));
}

// QUIRK W-261: kept
#[test]
fn a_destroyed_tent_leaves_the_travois_flagged() {
    // W-261: `0x223` does not find the tent -> 0 without effect, the flag stays.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let target = grid_plus(&sim, tent, (-6, 0));
    crate::world::destroy_object(&mut sim, tent);
    assert!(sim.object(tent).is_none());
    for _ in 0..2 {
        assert_eq!(arrive(&mut sim, travois, target), 0);
        let (flags, tt) = transport_of(&sim, travois);
        assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));
        assert!(sim.object(travois).unwrap().selectable);
    }
}

// QUIRK W-260: kept
#[test]
fn a_travois_dying_during_the_fade_in_leaves_the_tent_standing() {
    // re/tents 3.6: `m_TentHID` is already 0, the death does not hit the tent; `finish_unpack` no
    // longer finds the travois under the old ID (no selection handover, no destruction).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, travois) = packed(&mut sim, DWELLING_TENT);
    let bits = building::building(&sim, tent).unwrap().render_bits_backup;
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    assert_eq!(send(&mut sim, travois, &Message::new(0x110, &[])), 0);
    let mut events = next_phase_0(&mut sim);
    assert!(new_id_of(&events, travois).is_some(), "owner changed");
    assert!(sim.state.destroy_queue.is_empty());

    // The tent is tick group 0: completion in the first step of the 25th object tick.
    events.extend(object_ticks(&mut sim, 23));
    events.extend(crate::tick::step(sim.state, sim.rules, &[]));
    assert_eq!(sim.state.phase, 1);
    assert_eq!(tent_of(&sim, tent).status, status::REST);
    assert!(sim.state.destroy_queue.is_empty(), "no destruction");
    assert!(handovers(&events).is_empty());
    let o = sim.object(tent).unwrap();
    assert!(o.selectable);
    assert_eq!(o.render_bits, bits);
    assert!(!building::building(&sim, tent).unwrap().is_sleeping);
    assert_eq!(tent_of(&sim, tent).travois, travois);
}

// QUIRK W-247: deviation
#[test]
fn packing_a_hidden_tent_creates_a_second_travois() {
    // W-247 (directly via `send`; the player command rejects it, exception A8). The renewed `0x2E03`
    // saves the render bits 0 - after unpacking with the second travois the tent is awake,
    // selectable and invisible.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, first) = packed(&mut sim, DWELLING_TENT);
    assert_ne!(
        building::building(&sim, tent).unwrap().render_bits_backup,
        0
    );

    assert_eq!(pack_msg(&mut sim, tent), 0);
    let ids = travois_ids(&sim, 0);
    assert_eq!(ids.len(), 2);
    let second = tent_of(&sim, tent).travois;
    assert_ne!(second, first);
    assert!(ids.contains(&first) && ids.contains(&second));
    for t in [first, second] {
        let (flags, tt) = transport_of(&sim, t);
        assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));
    }
    assert_eq!(
        building::building(&sim, tent).unwrap().render_bits_backup,
        0
    );
    assert_eq!(tent_of(&sim, tent).status, status::FADE_OUT);

    object_ticks(&mut sim, 25);
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(arrive(&mut sim, second, target), 0);
    object_ticks(&mut sim, 25);
    let o = sim.object(tent).unwrap();
    assert!(o.selectable);
    assert_eq!(o.render_bits, 0, "invisible");
    assert!(!building::building(&sim, tent).unwrap().is_sleeping);
    // The first travois keeps carrying the tent.
    let (flags, tt) = transport_of(&sim, first);
    assert_eq!((flags, tt.tent), (transport::TRANSPORTING_TENT, tent));
}

#[test]
fn a_packed_tent_is_no_target_and_keeps_its_queue() {
    // re/tents 4: packed asleep (`0x4303` = 1, no attack target), production rests and
    // continues after unpacking.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tent = spawn_snapped(&mut sim, CHIEF_TENT, 0, SNAP);
    let sleeping = |sim: &mut Sim| send(sim, tent, &Message::new(building_msg::IS_SLEEPING, &[]));
    let make_time = |sim: &Sim| building::building(sim, tent).unwrap().make_time;
    assert_eq!(
        send(
            &mut sim,
            tent,
            &Message::new(building_msg::ENQUEUE, &[COLONIST as i32])
        ),
        1
    );
    object_ticks(&mut sim, 5);
    let running = make_time(&sim);
    assert!(running > 0 && running < 15_000, "running: {running}");
    assert_eq!(sleeping(&mut sim), 0);

    assert_eq!(pack_msg(&mut sim, tent), 0);
    assert_eq!(sleeping(&mut sim), 1);
    object_ticks(&mut sim, 60);
    assert_eq!(make_time(&sim), running);
    let travois = tent_of(&sim, tent).travois;
    let target = grid_plus(&sim, tent, (-6, 0));
    assert_eq!(arrive(&mut sim, travois, target), 0);
    object_ticks(&mut sim, 25);
    assert_eq!(make_time(&sim), running, "rests until unpacking");
    assert_eq!(sleeping(&mut sim), 0);
    object_ticks(&mut sim, 5);
    assert!(make_time(&sim) < running);
    assert_eq!(building::building(&sim, tent).unwrap().make_guid, COLONIST);
}

#[test]
fn unpack_on_an_object_without_module_35_does_nothing() {
    // re/tents 2.4: `0x223` to an object without module 35 -> -1.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (tent, _) = packed(&mut sim, DWELLING_TENT);
    let colonist = spawn(&mut sim, COLONIST, 0, (20, 20));
    let target = grid_plus(&sim, tent, (-6, 0));
    let before = crate::save::checksum(sim.state);
    assert_eq!(arrive(&mut sim, colonist, target), -1);
    assert_eq!(crate::save::checksum(sim.state), before);
    assert_eq!(tent_of(&sim, tent).status, status::REST);
}
