//! Rally-point flag (re/world-display W.1, D-257 ... D-262): the model `flag.rdo` at the rally point
//! of a **single** selected own building whose type menu has button 447. Presentation only:
//! the building module's stored rally point is read, the simulation stays untouched.
//!
//! In the original `PropertyBuilding_LoadMeetingFlagModel` (`0x0050AAF0`) loads one instance per building,
//! `ObjectManager_RenderObjects` (`0x0047C5F0`) sends `0x2603` for a single selection, and
//! `PropertyBuilding_RenderMeetingFlag` (`0x00507BB0`) draws it. The reimplementation queries
//! [`rally_flag`] per frame and draws the flag once as a world model (deviating from D-258).

use glam::{Affine3A, Vec3};
use opennml_effects::output::ModelDraw;
use opennml_sim::id::ObjectId;
use opennml_sim::module::{ModuleKind, building};
use opennml_sim::rules::Rules;
use opennml_sim::state::SimState;
use opennml_sim::world::grid_to_world;

/// The flag model (string `0x00622C7C`, the program's only flag file).
pub const FLAG_MODEL: &str = "data/r3d/misc/tools/sammelpunkt/flag.rdo";

/// `BUTTONMSG` of the "Set rally point" button: only types with this button in their `toolbar.ini` menu get
/// a flag (`FUN_0040A4E0(0x1BF)`).
pub const RALLY_POINT_BUTTON: i32 = 447;

/// An entry of the color table from the bit patterns of the `f32`.
const fn rgb(r: u32, g: u32, b: u32) -> [f32; 3] {
    [f32::from_bits(r), f32::from_bits(g), f32::from_bits(b)]
}

/// White (`1.0`).
const WHITE: u32 = 0x3F80_0000;

/// Flag colors per color index (`f32[4]` table `0x00664158`, the fourth component is 0), bit-exact
/// as in the program. Equal to `g_PlayerColors` `0x0061ABB0` except slot 0 (blue 0.081
/// instead of 0.051) and slots 8 ... 11 (white instead of gray; D-259).
pub const FLAG_COLORS: [[f32; 3]; 12] = [
    rgb(WHITE, 0x3F4E_147B, 0x3DA5_E354),
    rgb(0, 0x3EE6_E979, 0x3F66_E979),
    rgb(0x3DB8_51EC, 0x3F27_AE14, 0x3D81_0625),
    rgb(0x3F44_DD2F, 0x3DC0_8312, 0),
    rgb(0x3F02_8F5C, 0x3B83_126F, 0x3F58_D4FE),
    rgb(0x3F39_999A, 0, 0x3ECC_CCCD),
    rgb(WHITE, 0x3F1F_7CEE, 0x3D9F_BE77),
    rgb(0, 0x3F28_B439, 0x3F28_B439),
    rgb(WHITE, WHITE, WHITE),
    rgb(WHITE, WHITE, WHITE),
    rgb(WHITE, WHITE, WHITE),
    rgb(WHITE, WHITE, WHITE),
];

/// Flag color for the color index `GameObject+0x2D0` (`0x00507C8E`: `SHL 4` into the table, without
/// a range check). Outside 0 ... 11 the original read foreign memory; the reimplementation then takes white
/// like the neutral slot 8 (like `marker_color`).
pub fn flag_color(index: i32) -> [f32; 3] {
    usize::try_from(index)
        .ok()
        .and_then(|i| FLAG_COLORS.get(i))
        .copied()
        .unwrap_or(FLAG_COLORS[8])
}

/// The type `guid` has a flag: its menu in `toolbar.ini` (`GameWorld+0x1E0`, `CMap::Lookup`
/// `0x0047DE40` with the type GUID) has a button `BUTTONMSG 447` at the top. A
/// construction site carries the GUID of its building and gets the flag likewise (re/world-display W.1.1).
pub fn has_flag(rules: &Rules, guid: u32) -> bool {
    rules
        .toolbar
        .menus
        .get(&(guid as i32))
        .is_some_and(|m| m.iter().any(|b| b.msg == RALLY_POINT_BUTTON))
}

/// The flag of a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RallyFlag {
    /// The selected building.
    pub building: ObjectId,
    /// Stored rally point in world coordinates (`GridPos_ToWorldFloat` `0x0042FF70`) with the
    /// stored height, without a terrain query (D-260).
    pub position: Vec3,
    /// Flag color ([`flag_color`] of the building's color index).
    pub color: [f32; 3],
    /// `m_MeetingTarget` (`+0x35C`) as long as the object exists: the flag then stands on top of the
    /// target (W-100; practically unreachable in the game). Only the renderer knows the height of the
    /// hull (`MapView::set_rally_flag`).
    pub target: Option<ObjectId>,
}

/// The flag for the selection `selection` for the viewer `viewer` (re/world-display W.1.3, D-257): exactly one selected
/// object (`Selection_GetFirst` `0x0047A6B0`), owned by the viewer, with an **active**
/// building module (`GameObject_SendMessage` `0x0046E800` reaches only active modules; shipyards, harbors,
/// pitched tents and construction sites yes, units and ships never) and a type menu with button 447
/// ([`has_flag`]). No check of fog, pause or construction state.
pub fn rally_flag(
    state: &SimState,
    rules: &Rules,
    selection: &[ObjectId],
    viewer: u8,
) -> Option<RallyFlag> {
    let [id] = selection else {
        return None;
    };
    let o = state.object(*id, false)?;
    if o.owner() != viewer || !o.has_module(building::INDEX, false) || !has_flag(rules, o.guid) {
        return None;
    }
    let b = building_of(state, *id)?;
    // `GameWorld_GetObjectById(target, 0)`: a vanished target counts as none (re/world-display W.1.2).
    let target =
        Some(b.meeting_target).filter(|t| !t.is_none() && state.object(*t, false).is_some());
    Some(RallyFlag {
        building: *id,
        position: Vec3::from(grid_to_world(b.meeting_point, rules.map.cell)),
        color: flag_color(o.color),
        target,
    })
}

/// Draw order for the flag at `position` (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`):
/// pure translation (no rotation, scale 1), state block `Objects` (CULL CCW, fog like the
/// objects), animation 0 as a loop at speed 128 (`0x0058F910`) at time `now_ms`, player color
/// in the first `EF-MATERIAL` (`0x005884C0`), brightness 1 (`ModelInstance_Render(1.0, ...)`, without
/// fog brightness) and without shadow (`CASTSHADOW 0`, no path through the `ObjectRenderer`).
///
/// `now_ms` is the clock of the effect world (`EffectWorld::now_ms`): it stands still during the pause and in the game
/// menu
/// like the original's game time (`0x005778A0`, D-261), but runs like all effects at one-to-one speed with the
/// frame time, not with `(GameSpeed + 10) / 20` (at `GameSpeed` != 10 the flag waves correspondingly
/// faster or slower in the original).
pub fn flag_draw(position: Vec3, color: [f32; 3], now_ms: u32) -> ModelDraw {
    ModelDraw {
        model: FLAG_MODEL.into(),
        world: Affine3A::from_translation(position),
        animation: Some((0, now_ms as f32)),
        tint: None,
        alpha: None,
        blend: None,
        cull: true,
        fog: true,
        mesh: None,
        player_color: Some(color),
    }
}

/// Building module (module 3) of the object `id`.
fn building_of(state: &SimState, id: ObjectId) -> Option<&building::Building> {
    match &state.object(id, false)?.module(building::INDEX)?.kind {
        ModuleKind::Building(b) => Some(b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{COLONIST, HQ, STORE, host, site, spawn, world};
    use crate::game::sim_host::SimHost;
    use crate::game::toolbar_host::TARGET_DECAL_BUTTONS;
    use opennml_formats::gifcon::ToolbarButton;
    use opennml_sim::fx::{Fx, FxVec3};
    use opennml_sim::world::world_to_grid;

    /// The test world has no button 447 in the HQ menu; added only here so that
    /// stance and menus of other tests stay the same.
    fn host_with_flag() -> SimHost {
        let mut host = host();
        host.rules
            .toolbar
            .menus
            .entry(HQ as i32)
            .or_default()
            .push(ToolbarButton {
                id: 9,
                msg: RALLY_POINT_BUTTON,
                ..ToolbarButton::default()
            });
        host
    }

    fn meeting_point(host: &SimHost, id: ObjectId) -> FxVec3 {
        building_of(&host.state, id).unwrap().meeting_point
    }

    fn flag(host: &SimHost, selection: &[ObjectId]) -> Option<RallyFlag> {
        rally_flag(&host.state, &host.rules, selection, host.active_player)
    }

    // QUIRK D-257, D-260: kept
    #[test]
    fn flag_for_the_single_own_hq_at_its_stored_meeting_point() {
        let mut host = host_with_flag();
        // With node `ausgang`: the node including its height.
        let mut shape = host.rules.building_shapes[&HQ].clone();
        shape
            .helper_nodes
            .insert("ausgang".into(), [200.5, 10.0, -300.0]);
        host.rules.building_shapes.insert(HQ, shape);
        let at_node = spawn(&mut host, HQ, 0, (30, 30));
        let mp = meeting_point(&host, at_node);
        let g = host.state.object(at_node, false).unwrap().grid;
        assert_eq!(mp.y, Fx(g.y.0 + (10 << 12) / 128), "height of the node");
        let f = flag(&host, &[at_node]).expect("flag at the HQ");
        assert_eq!(f.building, at_node);
        assert_eq!(f.target, None);
        assert_eq!(
            f.position,
            Vec3::from(grid_to_world(mp, host.rules.map.cell))
        );
        assert_eq!(f.position.y, 10.0);
        // Without a node: three tiles in front of the building (-z), height of the building.
        let mut shape = host.rules.building_shapes[&HQ].clone();
        shape.helper_nodes.clear();
        host.rules.building_shapes.insert(HQ, shape);
        let plain = spawn(&mut host, HQ, 0, (40, 40));
        let f = flag(&host, &[plain]).expect("flag at the HQ");
        assert_eq!(
            f.position,
            Vec3::new(world(40), 0.0, world(40) - 3.0 * 128.0)
        );
        // The stored height applies, not the terrain (D-260).
        if let Some(o) = host.state.object_mut(plain, false)
            && let Some(m) = o.module_mut(building::INDEX)
            && let ModuleKind::Building(b) = &mut m.kind
        {
            b.meeting_point.y = Fx(7 << 12);
        }
        assert_eq!(flag(&host, &[plain]).unwrap().position.y, 7.0 * 128.0);
    }

    // QUIRK D-259: kept
    #[test]
    fn flag_colour_for_slot_zero_and_neutral() {
        let three = |c: [f32; 3]| c.map(|v| (v * 1000.0).round() / 1000.0);
        assert_eq!(three(flag_color(0)), [1.0, 0.805, 0.081]);
        assert_eq!(flag_color(0)[2].to_bits(), 0x3DA5_E354);
        assert_eq!(flag_color(8), [1.0; 3]);
        assert_eq!(flag_color(11), [1.0; 3]);
        assert_eq!(flag_color(-1), [1.0; 3]);
        assert_eq!(flag_color(12), [1.0; 3]);
        assert_eq!(
            flag_color(7),
            [
                0.0,
                f32::from_bits(0x3F28_B439),
                f32::from_bits(0x3F28_B439)
            ]
        );
        // The color comes from the color index `GO+0x2D0` of the building, not from the owner.
        let mut host = host_with_flag();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        assert_eq!(
            flag(&host, &[hq]).unwrap().color,
            flag_color(host.state.object(hq, false).unwrap().color)
        );
        host.state.object_mut(hq, false).unwrap().color = 3;
        assert_eq!(flag(&host, &[hq]).unwrap().color, FLAG_COLORS[3]);
    }

    // QUIRK D-257: kept
    #[test]
    fn no_flag_for_mixed_foreign_allied_or_storage_selection() {
        let mut host = host_with_flag();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let other_hq = spawn(&mut host, HQ, 0, (45, 30));
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let store = spawn(&mut host, STORE, 0, (20, 40));
        let foreign = spawn(&mut host, HQ, 1, (50, 50));
        let allied = spawn(&mut host, HQ, 2, (10, 50));
        host.state.diplomacy.friend[0] |= 1 << 2;
        host.state.diplomacy.friend[2] |= 1;
        assert!(flag(&host, &[hq]).is_some());
        assert!(flag(&host, &[hq, worker]).is_none(), "multi-selection");
        assert!(flag(&host, &[hq, other_hq]).is_none(), "two HQs");
        assert!(flag(&host, &[]).is_none());
        assert!(flag(&host, &[worker]).is_none(), "unit");
        assert!(
            flag(&host, &[store]).is_none(),
            "storage without button 447"
        );
        assert!(flag(&host, &[foreign]).is_none(), "foreign");
        assert!(flag(&host, &[allied]).is_none(), "allied");
        // For the owner of the foreign HQ, its flag stands there.
        assert!(rally_flag(&host.state, &host.rules, &[foreign], 1).is_some());
    }

    // QUIRK D-257: kept
    #[test]
    fn flag_at_an_own_construction_site() {
        let mut host = host_with_flag();
        let hq_site = site(&mut host, HQ, 0, (30, 30));
        let o = host.state.object(hq_site, false).unwrap();
        let ModuleKind::Building(b) = &o.module(building::INDEX).unwrap().kind else {
            panic!("building module expected");
        };
        assert!(b.is_build_building, "construction site");
        let f = flag(&host, &[hq_site]).expect("flag at the construction site");
        assert_eq!(
            f.position,
            Vec3::from(grid_to_world(b.meeting_point, host.rules.map.cell))
        );
    }

    // QUIRK D-262: kept
    #[test]
    fn flag_at_the_unrasterized_click_point_and_at_water_level() {
        let mut host = host_with_flag();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let cell = host.rules.map.cell;
        // Expected: the click point through the fixed-point grid and back (`world_to_grid` truncates,
        // deviation < 1/32 unit), not the tile center.
        let expect = |p: [f32; 3]| Vec3::from(grid_to_world(world_to_grid(p, cell), cell));
        let click = Vec3::new(7.3 * 128.0, 0.0, 9.6 * 128.0);
        let r = host.issue_target_action(&[hq], None, click, Some(false));
        assert_eq!(r.map(|r| r.0), Some(RALLY_POINT_BUTTON));
        host.step_once();
        let f = flag(&host, &[hq]).unwrap();
        assert_eq!(f.position, expect(click.into()));
        assert!((f.position - click).abs().max_element() <= cell[0] as f32 / 4096.0);
        // Over water: the rally point takes the water height (`max(terrain, water)`), the flag
        // stands on the water surface.
        host.rules.map.water_level = 5.0;
        let click = Vec3::new(20.25 * 128.0, 0.0, 11.5 * 128.0);
        host.issue_target_action(&[hq], None, click, Some(false));
        host.step_once();
        let f = flag(&host, &[hq]).unwrap();
        assert_eq!(f.position, expect([click.x, 5.0, click.z]));
        assert!((f.position.y - 5.0).abs() <= cell[1] as f32 / 4096.0);
        // No feedback: no target-point decal for 447 (D-262, `0x0044D350` case `0x1BF`).
        assert!(!TARGET_DECAL_BUTTONS.contains(&RALLY_POINT_BUTTON));
    }

    #[test]
    fn one_flag_draw_per_frame() {
        let d = flag_draw(Vec3::new(1.0, 2.0, 3.0), FLAG_COLORS[2], 1234);
        assert_eq!(d.model, FLAG_MODEL);
        assert_eq!(
            d.world,
            Affine3A::from_translation(Vec3::new(1.0, 2.0, 3.0))
        );
        assert_eq!(d.animation, Some((0, 1234.0)));
        assert_eq!(d.player_color, Some(FLAG_COLORS[2]));
        assert!(d.cull && d.fog);
        assert_eq!(
            (d.tint, d.alpha, d.blend, d.mesh.as_deref()),
            (None, None, None, None)
        );
    }

    /// Shipped data: 38 types with button 447 (German edition with patch; HQ yes, storage no) and the
    /// flag model with animation.
    // QUIRK D-257: kept
    #[test]
    fn corpus_flag_types_have_button_447() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let level = "data/levels/multiplayer/[2players]-tintebariver.lvl";
        let scene = opennml_world::level::load_level(&vfs, level).unwrap();
        let sim = crate::game::sim_host::start_sim(
            &vfs,
            &scene,
            10,
            crate::game::sim_host::testkit::default_setup,
        )
        .unwrap()
        .expect("match");
        let rules = &sim.rules;
        let flagged: Vec<i32> = rules
            .toolbar
            .menus
            .keys()
            .copied()
            .filter(|&g| u32::try_from(g).is_ok_and(|g| has_flag(rules, g)))
            .collect();
        if opennml_rda::testutil::game_version(&vfs)
            == Ok(opennml_rda::testutil::GameVersion::German)
        {
            assert_eq!(flagged.len(), 38, "{flagged:?}");
        }
        assert!(has_flag(rules, 1000), "headquarters");
        assert!(!has_flag(rules, 1003), "warehouse");
        let mut models = opennml_world::assets::ModelCache::new();
        let asset = models.get(&vfs, FLAG_MODEL).expect("flag.rdo");
        assert!(
            asset.animations.first().is_some_and(Option::is_some),
            "animation flag.rdm"
        );
    }
}
