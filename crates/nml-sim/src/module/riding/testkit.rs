//! Test world for riding, swimming and diving: Dragoon (module 13), Warrior (14, 15),
//! diver (14, 15, 24), Colonist (13), PRI warrior (13, 14), horse and canoe, all with walk, stand
//! and dying module; 64 x 64 tiles of land, revealed for all players. The lists are given as text
//! like the stand module's `30001`; plus a headquarters (quarters); [`lake_rules`] adds the water map.

use nml_formats::targets::{parse_target_defines, parse_targets};

use crate::id::ObjectId;
use crate::object::status;
use crate::rules::Rules;
use crate::sim::Sim;

pub(crate) use crate::command::testkit::{spawn, state};

/// SPA_Kolonist (module 13, land). `140003` (running) is `27` instead of `2`, so that walking
/// (`140002 = 2`) and running stay observable.
pub const COLONIST: u32 = 1100;
/// Horse (land, RUN, SADDLE; no module).
pub const HORSE: u32 = 3410;
/// WID canoe (water 2 x 2, SADDLEBOAT, layer 6; no module).
pub const CANOE: u32 = 1408;
/// PRI canoe (like the WID canoe; in the shipped data with `dummy01`, re/riding 13.1).
pub const PRI_CANOE: u32 = 1712;

/// ENG_Dragoner (module 13, land).
pub const DRAGOON: u32 = 2007;
/// WID_Krieger (modules 14 and 15, land + water, layer 1).
pub const WARRIOR: u32 = 1400;
/// WID_Gewehrschuetze as diver (modules 14, 15 and 24).
pub const DIVER: u32 = 1403;
/// Diver without stamina (attribute 28 with value 0): can never dive (re/swim-dive 8.9, in the
/// shipped data 1705).
pub const NO_AIR_DIVER: u32 = 1705;
/// PRI_Krieger (modules 13 and 14, land + water): disembarks via Riding and looks for a horse after a
/// failed boarding (K-226).
pub const PRI_WARRIOR: u32 = 1700;
/// Rider placement `2008 = 2007 + 3410` (`translate.ini`, re/riding 1.3): Dragoon on horse.
pub const DRAGOON_ON_HORSE: u32 = 2008;
/// Headquarters with building (3) and quarters (23, capacity 5) like `command::testkit::HQ`.
pub const HQ: u32 = 1000;

/// Land (`2048`) or land + water (`0x42900`, layer 1).
const LAND: u32 = 2048;
const AMPHIBIOUS: u32 = 0x42900;
/// Horse: land, RUN (`0x4000`), SADDLE (`0x8000`), `0x200000` (shipped value `0x20C900`).
const HORSE_FLAGS: u32 = 0x20_C900;
/// Canoe: water 2 x 2, SADDLEBOAT (`0x10000`).
const CANOE_FLAGS: u32 = 0x3_2200;

/// Shoot module 10 (arrow) and melee module 18 of the Dragoon; when mounted, both use the
/// animation lists of the riding module.
const SHOOT_AND_FIGHT: &str = "PROPID 10\nVALUEID 110001 6 1 256\nPROPID 18\n\
VALUEID 190001 6 1 0\nVALUEID 190002 1 1 1\n";
const FIGHT_ONLY: &str = "PROPID 18\nVALUEID 190001 6 1 0\nVALUEID 190002 1 1 1\n";

/// Modules 13 and 14 of the PRI warrior (PRI_Krieger 1700, re/riding 1.3).
const PRI_MODULES: &str = "PROPID 13\nVALUEID 140002 1 1 5\nVALUEID 140003 1 1 6\n\
VALUEID 140004 1 1 8\nVALUEID 140005 1 1 7\nPROPID 14\nVALUEID 150002 1 1 1\n\
VALUEID 150003 1 1 2\n";

/// Modules 14 and 15 of the warrior (WID_Krieger 1400, re/riding 1.3, re/swim-dive 1.4).
const BOAT_AND_SWIM: &str = "PROPID 14\nVALUEID 150002 1 1 19\nVALUEID 150003 1 1 18\n\
VALUEID 150004 1 1 20\nPROPID 15\nVALUEID 160002 1 1 3\nVALUEID 160003 1 1 4\n\
VALUEID 160004 1 1 5\n";

/// Unit with walk (radius hard 512, soft 1024), stand and dying module; `modules` appends the
/// modules of the table.
fn unit(guid: u32, name: &str, walk_flags: u32, modules: &str) -> String {
    format!(
        "GUID {guid}\nNAME {name}\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\n\
         VALUEID 10003 6 1 {walk_flags}\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\n\
         VALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n\
         PROPID 2\nVALUEID 30001 1 1 0\nPROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 3\n\
         {modules}"
    )
}

/// Names of the right-click entries 403 and 404 (`targets.ini`, re/riding 2.5).
pub const DEFINES: &str = "PROP_RIDING 13
PROP_RIDINGBOAT 14
MID_PROP_PTTOPT_SADDLE 32768
MID_PROP_PTTOPT_SADDLEBOAT 65536
";

/// Right-click actions saddling up on a horse (403: own riders with module 13; target SADDLE, not
/// `CARRIING`, fog, **without player list**) and on a boat (404: module 14; target SADDLEBOAT, not
/// `CARRIING`, fog, own only). Structure like the entries in `command::testkit::TARGETS`; the
/// table there stays unchanged (its tests depend on the order).
pub const TARGETS: &str = concat!(
    "BEGIN
BUTTONMSG 403
MY_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKPROPERTY
MM_MODE MM_HL_MYLIST
MM_PROPID PROP_RIDING
MM_END
",
    "TARGET_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKFOG
MM_MODE MM_CHECKHANDLETYPE
MM_MATCHBITS MID_PROP_PTTOPT_SADDLE
MM_MODE MM_CHECKNOTSTATUS
MM_STATUS CARRIING
MM_END
END
",
    "BEGIN
BUTTONMSG 404
MY_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKPROPERTY
MM_MODE MM_HL_MYLIST
MM_PROPID PROP_RIDINGBOAT
MM_END
",
    "TARGET_MM
MM_MODE MM_ANDALL
MM_MODE MM_CHECKFOG
MM_MODE MM_HL_MYLIST
MM_MODE MM_CHECKHANDLETYPE
MM_MATCHBITS MID_PROP_PTTOPT_SADDLEBOAT
MM_MODE MM_CHECKNOTSTATUS
MM_STATUS CARRIING
MM_END
END
",
);

/// Rules on a 64 x 64 map (land). Speed 183, sight 32, hit points 100, stamina 500;
/// rider placement 2008 and mounty bonus as in the shipped data.
pub fn rules() -> Rules {
    let types = [
        unit(
            DRAGOON,
            "ENG_Dragoner",
            LAND,
            &format!(
                "PROPID 13\nVALUEID 140002 1 1 6\nVALUEID 140003 1 1 6\nVALUEID 140004 1 1 7\n\
                 VALUEID 140005 1 1 3\nVALUEID 140006 1 1 5\nVALUEID 140007 1 1 5\n\
                 {SHOOT_AND_FIGHT}"
            ),
        ),
        unit(
            COLONIST,
            "SPA_Kolonist",
            LAND,
            "PROPID 13\nVALUEID 140001 6 1 0\nVALUEID 140002 1 1 2\nVALUEID 140003 1 1 27\n\
             VALUEID 140004 1 1 15\nVALUEID 140005 1 1 14\nVALUEID 140006 1 1 32\n",
        ),
        unit(HORSE, "ENG_Pferd", HORSE_FLAGS, ""),
        unit(CANOE, "WID_Kanu", CANOE_FLAGS, ""),
        unit(PRI_CANOE, "PRI_Kanu", CANOE_FLAGS, ""),
        unit(
            WARRIOR,
            "WID_Krieger",
            AMPHIBIOUS,
            &format!("{BOAT_AND_SWIM}{FIGHT_ONLY}"),
        ),
        unit(PRI_WARRIOR, "PRI_Krieger", AMPHIBIOUS, PRI_MODULES),
        unit(
            NO_AIR_DIVER,
            "PRI_Schamane",
            AMPHIBIOUS,
            &format!("{BOAT_AND_SWIM}PROPID 24\nVALUEID 250001 1 1 7\nVALUEID 250002 1 1 8\n"),
        ),
        unit(
            DIVER,
            "WID_Gewehrschuetze",
            AMPHIBIOUS,
            &format!("{BOAT_AND_SWIM}PROPID 24\nVALUEID 250001 1 1 7\nVALUEID 250002 1 1 8\n"),
        ),
        format!(
            "GUID {HQ}\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\n\
             VALUEID 40003 6 1 14336\nPROPID 23\n"
        ),
    ]
    .concat();
    let mut r = Rules::synthetic(&types, 64);
    r.targets = parse_targets(TARGETS, &parse_target_defines(DEFINES)).entries;
    r.translations
        .insert(DRAGOON_ON_HORSE, vec![DRAGOON, HORSE]);
    // `MOUNTY_BONUS 18 20` (shipped data, re/riding 4.6): sight +20 % when mounting.
    r.bonus.mounty.insert(18, 20);
    // Without stamina: attribute 28 with value 0 (like `default.dat` for 1705).
    r.defaults.insert(
        NO_AIR_DIVER,
        [(14, 100), (16, 183), (18, 32), (28, 0)]
            .into_iter()
            .collect(),
    );
    r.defaults
        .insert(HQ, [(13, 90), (14, 4000), (23, 5)].into_iter().collect());
    r.building_shapes.insert(
        HQ,
        crate::rules::BuildingShape {
            center: [0.0; 3],
            half: [100.0, 50.0, 100.0],
            free_nodes: Vec::new(),
            helper_nodes: Default::default(),
        },
    );
    for guid in [
        DRAGOON,
        COLONIST,
        HORSE,
        CANOE,
        PRI_CANOE,
        WARRIOR,
        DIVER,
        PRI_WARRIOR,
    ] {
        // Speed: horse 227, otherwise 183.
        let speed = if guid == HORSE { 227 } else { 183 };
        r.defaults.insert(
            guid,
            [(14, 100), (16, speed), (18, 32), (28, 500)]
                .into_iter()
                .collect(),
        );
    }
    r
}

/// Rules with a lake (swimming and diving): water level 0.0 in both places
/// (`map.water_level` and `terrain.params.water_level`; `classify_tiles` reads the second), all
/// grid points with `20 <= i, j <= 40` at `floor`. The remaining land lies 16.0 above the water:
/// land whose highest corner = water level would get neither `ABOVE_WATER` nor `UNDER_WATER`
/// (`path/terrain.rs`), and the water layers 4/6/8 would be free there. Tile heights and
/// tile classes recomputed as in `Rules::synthetic`.
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

/// Set the rider-mount link directly (without approach, bonus and animations): `target` on both sides,
/// on the rider `0x2000` or `0x8000` (mount with walk flag `SADDLEBOAT 0x10000`), on the mount
/// `CARRIING`.
pub fn link(sim: &mut Sim, rider: ObjectId, mount: ObjectId) {
    let boat = sim
        .module(mount, crate::module::walk::INDEX)
        .is_some_and(|m| m.base.flags & 0x1_0000 != 0);
    let bit = if boat {
        status::ISMOUNTEDONBOAT
    } else {
        status::ISMOUNTED
    };
    let r = sim.object_mut(rider).expect("rider present");
    r.target = mount;
    r.set_status_flags(bit, true);
    let m = sim.object_mut(mount).expect("mount present");
    m.target = rider;
    m.set_status_flags(status::CARRIING, true);
}
