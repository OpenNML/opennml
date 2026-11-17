//! Test world of the ships: 32 x 32 tiles with the coast of re/ships 9.2 -
//! land for `x <= 9`, water for `x >= 10` - with the transport ship 1108 (walk layer 6, stand
//! `ONWATER`, dying, quarters with capacity 12), the Mayflower 2104 (like the transport ship, but
//! quarters with capacity 0), the warships 1109 and 2010 (walk layer 8, stand, dying,
//! shoot module `SHIP`, attack and ranged combat as in re/ships 1.1/1.2, plus the time markers of the
//! fire animations and cannon nodes in `Rules::cannon_nodes`), a house with hit zones
//! and the colonist (land, `CANQUARTER`, with a dying module so that occupants of a
//! sinking ship can die), plus the right-click entries 414 "into building" and "into
//! vehicles" of `targets.ini`.

use nml_formats::targets::{parse_target_defines, parse_targets};

use crate::id::ObjectId;
use crate::rules::{BuildingShape, ModelBox, Rules, TileTerrain};
use crate::sim::Sim;
use crate::state::SimState;
use crate::world::{ObjectDesc, create_object, desc_flags};

/// SPA_Transportschiff (walk `0x22200` -> layer 6, hard 2100 / soft 4000).
pub const TRANSPORT: u32 = 1108;
/// SPA_Kriegsschiff (walk layer 8, shoot module `SHIP`, 6 cannons per side).
pub const WARSHIP: u32 = 1109;
/// ENG_Kriegsschiff (like 1109, 4 cannons per side).
pub const ENG_WARSHIP: u32 = 2010;
/// House (building with four hit zones) as broadside target.
pub const HOUSE: u32 = 1001;
/// Mayflower (walk like the transport ship, quarters with capacity 0).
pub const MAYFLOWER: u32 = 2104;
/// Own object without modules (no walk, cannot be quartered), e.g. a house among the
/// IDs of a command.
pub const PROP: u32 = 3000;
/// SPA_Kolonist (walk land 1 x 1, `CANQUARTER`, hard 512 / soft 1024).
pub const COLONIST: u32 = 1100;

/// Edge length of the map in tiles.
const DIM: u32 = 32;
/// First water tile in x (land: `x <= 9`).
const FIRST_WATER_X: i32 = 10;

/// Ship type of the test world: walk layer 6 (hard 2100, soft 4000, `NOTHUMAN`), stand `ONWATER`,
/// dying; `quarter` attaches the quarters module.
fn ship(guid: u32, name: &str, quarter: bool) -> String {
    // Walk: 10001 animation, 10003 flags, 10004/10005 hard/soft radius, 10006 speed class,
    // 10007 minimum speed; stand: 30001 animation, 30003 flags; dying: 170002 animation.
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\n\
         PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 139776\nVALUEID 10004 6 1 2100\n\
         VALUEID 10005 6 1 4000\nVALUEID 10006 6 1 7\nVALUEID 10007 6 1 1\n\
         PROPID 2\nVALUEID 30001 6 1 0\nVALUEID 30003 6 1 256\n\
         PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 2\n{}",
        if quarter { "PROPID 23\n" } else { "" }
    )
}

/// Warship per re/ships 1.1/1.2: walk `0x22400` (layer 8, 4200/8000), stand `ONWATER`, dying
/// 4, shoot module `SHIP` (starboard 2, port 3, standing 0), attack `0x20800`, ranged combat
/// `ISBOUNTY` (`m_IsShip`).
fn warship(guid: u32, name: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\n\
         PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 140288\nVALUEID 10004 6 1 4200\n\
         VALUEID 10005 6 1 8000\nVALUEID 10006 6 1 7\nVALUEID 10007 6 1 1\n\
         PROPID 2\nVALUEID 30001 6 1 0\nVALUEID 30003 6 1 256\n\
         PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 4\n\
         PROPID 10\nVALUEID 110001 6 1 131072\nVALUEID 110002 1 1 2\nVALUEID 110003 1 1 3\n\
         VALUEID 110004 1 1 0\n\
         PROPID 17\nVALUEID 180001 6 1 133120\nVALUEID 180002 6 1 0\n\
         PROPID 19\nVALUEID 200001 6 1 256\n"
    )
}

/// Names of the right-click entries 414 (`targets.ini`, re/ships 2.4).
const DEFINES: &str = "PROP_PTTOPT 0
PROP_QUARTER 23
MID_PROP_PTTOPT_CANQUARTER 262144
MID_PROP_QUARTER_FREEFORQUARTER 279
MID_PROP_BUILDING_CONSTRUCTED_FINISHED 1027
";

/// The two entries 414 in file order: "into building" (target `FREEFORQUARTER`, and-next
/// `CONSTRUCTED_FINISHED`) before "into vehicles (mounted units are allowed)" (target: own,
/// fog, module 23, and-next module 0). The vehicle entry checks neither free places nor capacity.
const TARGETS: &str = concat!(
    "BEGIN
BUTTONMSG 414
MY_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKHANDLETYPE
MM_MODE MM_HL_MYLIST
MM_MATCHBITS MID_PROP_PTTOPT_CANQUARTER
MM_END
",
    "TARGET_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKHANDLETYPE
MM_MATCHBITS MID_PROP_QUARTER_FREEFORQUARTER
MM_MODE MM_HL_MYLIST
MM_MODE MM_ANDNEXTMATCH
MM_NEXT
MM_MODE MM_CHECKHANDLETYPE
MM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED
MM_END
MAUSICONID 2
END
",
    "BEGIN
BUTTONMSG 414
MY_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKHANDLETYPE
MM_MODE MM_HL_MYLIST
MM_MATCHBITS MID_PROP_PTTOPT_CANQUARTER
MM_END
",
    "TARGET_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKFOG
MM_MODE MM_HL_MYLIST
MM_MODE MM_CHECKPROPERTY
MM_PROPID PROP_QUARTER
MM_MODE MM_ANDNEXTMATCH
MM_NEXT
MM_MODE MM_CHECKPROPERTY
MM_PROPID PROP_PTTOPT
MM_END
MAUSICONID 11
END
",
);

/// Types of the test world as `property.ini` text (parameters per re/ships 1.1).
fn types() -> String {
    let ships = [
        ship(TRANSPORT, "SPA_Transportschiff", true),
        ship(MAYFLOWER, "Mayflower", true),
        warship(WARSHIP, "SPA_Kriegsschiff"),
        warship(ENG_WARSHIP, "ENG_Kriegsschiff"),
        format!(
            "GUID {HOUSE}
NAME SPA_Wohnhaus
VALUEID 100 6 1 0
PROPID 3
VALUEID 40003 6 1 0
"
        ),
        format!("GUID {PROP}\nNAME Requisit\nVALUEID 100 6 1 0\n"),
    ]
    .concat();
    format!(
        "{ships}GUID {COLONIST}\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n\
         PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n\
         VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n\
         PROPID 2\nVALUEID 30001 6 1 0\nVALUEID 30003 6 1 0\n\
         PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n"
    )
}

/// Rules: types, attributes (hit points 14, speed 16, sight 18, quarter places 23) and a terrain
/// matching the coast - the grid points with `i <= 10` lie 16 above the water level 0, the others 16
/// below, so the tiles `x <= 9` lie above, `x >= 11` below water (tile 10 is a mixed tile).
pub fn rules() -> Rules {
    use crate::path::terrain::classify_tiles;
    let mut r = Rules::synthetic(&types(), DIM);
    r.defaults.insert(
        TRANSPORT,
        [(14, 1400), (16, 183), (18, 52), (23, 12)]
            .into_iter()
            .collect(),
    );
    r.defaults.insert(
        COLONIST,
        [(14, 100), (16, 183), (18, 32)].into_iter().collect(),
    );
    r.defaults.insert(
        MAYFLOWER,
        [(14, 1400), (16, 183), (18, 52), (23, 0)]
            .into_iter()
            .collect(),
    );
    // Hit points 14, speed 16, sight 18, ranged attack 20 (50 per ball), range 21, minimum distance 34
    // (re/ships 1.1); without 18/21 FightFar would have range 0.
    r.defaults.insert(
        WARSHIP,
        [(14, 2000), (16, 183), (18, 64), (20, 50), (21, 48), (34, 0)]
            .into_iter()
            .collect(),
    );
    r.defaults.insert(
        ENG_WARSHIP,
        [(14, 1600), (16, 183), (18, 52), (20, 50), (21, 48), (34, 0)]
            .into_iter()
            .collect(),
    );
    r.defaults.insert(HOUSE, [(14, 900)].into_iter().collect());
    r.building_shapes.insert(
        HOUSE,
        BuildingShape {
            center: [0.0; 3],
            half: [100.0, 50.0, 100.0],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        },
    );
    r.model_boxes.insert(
        HOUSE,
        ModelBox {
            center: [0.0; 3],
            half: [100.0, 50.0, 100.0],
        },
    );
    r.hit_zones.insert(
        HOUSE,
        vec![
            [100.0, 0.0, 0.0],
            [-100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0],
            [0.0, 0.0, -100.0],
        ],
    );
    // Time markers of the fire animations 2 (starboard) and 3 (port), re/ships 1.2: marker 1 =
    // end (2000 ms), markers 3... = cannon 1...; 1109 six, 2010 four cannons.
    let marks = |n: usize| -> Vec<(u32, i32)> {
        let mut v: Vec<(u32, i32)> = [0, 167, 333, 500, 667, 833][..n]
            .iter()
            .enumerate()
            .map(|(k, &ms)| (3 + k as u32, ms))
            .collect();
        v.push((1, 2000));
        v
    };
    for (guid, n) in [(WARSHIP, 6), (ENG_WARSHIP, 4)] {
        for anim in [2, 3] {
            r.set_anim_labels(guid, anim, &marks(n));
        }
        // Cannon nodes (rest positions, freely chosen): entry k belongs to node k + 1.
        for (starboard, anim, z) in [(true, 2, 30.0), (false, 3, -30.0)] {
            let nodes = (0..n).map(|k| [-40.0 + 16.0 * k as f32, 20.0, z]).collect();
            r.cannon_nodes.insert((guid, anim, starboard), nodes);
        }
    }
    r.targets = parse_targets(TARGETS, &parse_target_defines(DEFINES)).entries;
    r.map.water_level = 0.0;
    r.terrain.params.water_level = 0.0;
    let vx = r.terrain.verts_x;
    for (n, v) in r.terrain.vertices.iter_mut().enumerate() {
        v.position.y = if (n % vx) as i32 <= FIRST_WATER_X {
            16.0
        } else {
            -16.0
        };
    }
    r.tile_terrain = TileTerrain::build(&r.terrain, &r.map);
    r.tile_classes = classify_tiles(&r.terrain, |_| 0);
    r
}

/// Path grid from two inputs: all base layers (0-4) locked, then layer 0 (land) free where
/// `land(x, z)`, and layer 4 (water) free where `water(x, z)`; the derived layers 5-8 follow from
/// `flush_dirty_rect`, afterwards the regions are built.
fn grid_state(
    rules: &Rules,
    land: impl Fn(i32, i32) -> bool,
    water: impl Fn(i32, i32) -> bool,
) -> SimState {
    let mut s = SimState::new(0, 0);
    s.init_map(rules);
    let dim = DIM as i32;
    for z in 0..dim {
        for x in 0..dim {
            s.path.mark_cell(0x1F, x, z, true);
        }
    }
    for z in 0..dim {
        for x in 0..dim {
            if land(x, z) {
                s.path.mark_cell(1 << 0, x, z, false);
            }
        }
    }
    for z in 0..dim {
        for x in 0..dim {
            if water(x, z) {
                s.path.mark_cell(1 << 4, x, z, false);
            }
        }
    }
    s.path.flush_dirty_rect();
    s.path.build_regions(u32::MAX);
    s
}

/// Coast grid of re/ships 9.2: land layer 0 free for `x <= 9`, water base layer 4 free for
/// `x >= 10` (layer 6, 2 x 2 with window `x-1...x`, so free for `x >= 11`).
pub fn shore_state(rules: &Rules) -> SimState {
    grid_state(rules, |x, _| x < FIRST_WATER_X, |x, _| x >= FIRST_WATER_X)
}

/// Island without ship water in the surroundings: land only in the block `1 <= x, z <= 5`, water as at the
/// coast for `x >= 10`. The block lies more than two cells from the layer 6 water (`x >= 11`), the
/// inner search of the meeting point calculation (radius 2) finds nothing from there.
pub fn island_state(rules: &Rules) -> SimState {
    grid_state(
        rules,
        |x, z| (1..=5).contains(&x) && (1..=5).contains(&z),
        |x, _| x >= FIRST_WATER_X,
    )
}

/// World coordinate of the tile center.
fn world(t: i32) -> f32 {
    t as f32 * 128.0 + 64.0
}

/// Finished object at the center of the tile `tile`, direction 0. The rotation about the y axis is the one
/// `world::set_direction` writes for direction 0 (`rad(dir + 90 degrees)`); without `ROT_Y` -90 degrees
/// would result (`apply_placement` subtracts 90 degrees).
pub fn spawn(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
    let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
    desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
    desc.rotation[1] = (90 << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
    create_object(sim, &mut desc).expect("object created")
}
