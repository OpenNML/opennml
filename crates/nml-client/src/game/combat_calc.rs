//! Health bar (`GameObject_DrawHealthBar` `0x0046C0F0`, re/combat-host 4.3) and rank icon
//! (`erfahrung0r.tga`, re/combat-damage 4.1) over selected objects and the object under
//! the mouse, plus the counter-upgrade icon (`counterupgrade.tga`, `FUN_00469F60`)
//! over own objects with a running counter-upgrade, and the group digit (`zahl_<i>.tga`,
//! O-313) next to the bar of own objects in a control group. Presentation only; the simulation stays
//! untouched.
//!
//! This holds the calculation (which objects, where in the frame, combat line) and the drawing path of `nml-game` as
//! draw commands of the UI ([`overlay_cmds`]); the viewer draws the same calculation with
//! egui (`apps/nml-viewer/src/combat_overlay.rs`).

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use nml_rda::Vfs;
use nml_sim::id::ObjectId;
use nml_sim::object::{GameObject, status};
use nml_ui::assets::UiAssets;
use nml_ui::draw::{Blend, DrawCmd, Filter};

use crate::game::control_groups::{ControlGroups, GROUPS};
use crate::game::grid::project;
use crate::game::sim_host::SimHost;

pub const RANK_ICONS: [&str; 5] = [
    "data/icons/erfahrung01.tga",
    "data/icons/erfahrung02.tga",
    "data/icons/erfahrung03.tga",
    "data/icons/erfahrung04.tga",
    "data/icons/erfahrung05.tga",
];

/// Counter-upgrade icon (`GameObject_Construct` `0x0047091E`, texture `+0x2F8`).
pub const COUNTER_UPGRADE_ICON: &str = "data/icons/counterupgrade.tga";

/// Group digits (`GameObject_Construct` `0x00470500`: loop `i = 0..9` over `data/icons/zahl_%d.tga`
/// `0x0061AD34` to `+0x2FC + 4i`): index = group number, group 0 (key 0) shows `zahl_0.tga`.
pub const GROUP_ICONS: [&str; GROUPS] = [
    "data/icons/zahl_0.tga",
    "data/icons/zahl_1.tga",
    "data/icons/zahl_2.tga",
    "data/icons/zahl_3.tga",
    "data/icons/zahl_4.tga",
    "data/icons/zahl_5.tga",
    "data/icons/zahl_6.tga",
    "data/icons/zahl_7.tga",
    "data/icons/zahl_8.tga",
    "data/icons/zahl_9.tga",
];

/// Group digit (`FUN_004697E0` from `FUN_0046C0F0` `0x0046C2DD..0x0046C335`): billboard in world units around the
/// anchor `A` (bar center): x 15..45 along the camera's right axis, y -15..15 (30 x 30), uv (0, 0) at (15, 15).
const DIGIT_X: [f32; 2] = [15.0, 45.0];
const DIGIT_HALF: f32 = 15.0;
/// Anchor above the top of the hull (`[0x0061AC90]` = 15, like the bar center) and lift when a bar was drawn
/// (`[0x0061AC8C]` = 12, `0x0046C2D3`).
const DIGIT_ANCHOR_LIFT: f32 = 15.0;
const DIGIT_BAR_LIFT: f32 = 12.0;

/// Texture of the health bar (`GameObject_FinishPlacement` `0x00470350` -> `GameObject+0x2E0`, re/combat-host 4.3):
/// 32 x 32 gray gradient, modulated with red or green.
pub const BAR_TEXTURE: &str = "data/textures/selektion/balken.bmp";

/// Attribute hit points (`AttributeSet_IsPresent(..., 0xE)`).
const HIT_POINTS: u32 = 14;
/// Fraction 100 % in the attribute set.
const FULL: f32 = 100_000.0;
/// Rank icon (`FUN_0046BBE0`): billboard 40 x 40 world units (corner points +-20), center 30 above the
/// top of the hull, 50 higher for riders (`ISMOUNTED` `0x2000`), shifted by 50 against the yaw for
/// occupants (`ISMOUNTEDONBOAT` `0x8000`) ([`rank_icon_offset`]).
const ICON_HALF: f32 = 20.0;
const ICON_LIFT: f32 = 30.0;
const ICON_MOUNTED_LIFT: f32 = 50.0;
/// Lateral shift of the occupants: locally `(0, 0, -50)`.
const ICON_PASSENGER_SHIFT: f32 = 50.0;
/// Counter-upgrade icon (`FUN_00469F60`): center 50 above the top of the hull (the rank icon 30), same
/// size and the same rider and occupant shift.
const COUNTER_ICON_LIFT: f32 = 50.0;

/// Shift of the rank icon by the riding status (`FUN_0046BBE0` `0x0046BCB6..0x0046BD6C`,
/// re/riding 7.3, C-206): `0x2000` raises by 50 in y; `0x8000` shifts locally by `(0, 0, -50)` and
/// rotates the vector with `RotationY(yaw)` (D3DX, row vector: `x' = -50 sin(theta)`, `z' = -50 cos(theta)`).
/// `yaw` is the rendering rotation of the object (for occupants that of the boat) in radians.
pub fn rank_icon_offset(status: u32, yaw: f32) -> Vec3 {
    if status & status::ISMOUNTEDONBOAT != 0 {
        Vec3::new(
            -ICON_PASSENGER_SHIFT * yaw.sin(),
            0.0,
            -ICON_PASSENGER_SHIFT * yaw.cos(),
        )
    } else if status & status::ISMOUNTED != 0 {
        Vec3::Y * ICON_MOUNTED_LIFT
    } else {
        Vec3::ZERO
    }
}

/// Name of a stance (`m_AttackMode`, command `0x12`) like the UI (Shift + 1 .. 5);
/// other values as a number.
pub fn stance_name(mode: i32) -> String {
    match mode {
        0 => "Cease fire".into(),
        1 => "Hold position".into(),
        2 => "Defensive".into(),
        3 => "Offensive".into(),
        4 => "Defend area".into(),
        other => other.to_string(),
    }
}

/// Combat line of the overlay: stance (only with an attack module), hit points
/// current/maximum, rank and experience in whole percent (rounded down).
pub fn combat_line(c: &nml_control::SimCombatInfo) -> String {
    let mut parts = Vec::new();
    if let Some(a) = &c.attack {
        parts.push(format!("Stance {}", stance_name(a.mode)));
    }
    parts.push(format!("HP {}/{}", c.hp[0], c.hp[1]));
    parts.push(format!("Rank {}", c.rank));
    parts.push(format!("Experience {} %", c.experience / 1000));
    format!("Combat: {}", parts.join(" · "))
}

/// Two rectangles in world coordinates (each the lower left and upper right corner in the image plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthBar {
    pub background: [Vec3; 2],
    pub fill: [Vec3; 2],
    pub red: u32,
    pub green: u32,
}

/// Bar 60 x 8 world units, center 15 + 4 above `top` (top of the hull), fill up to `(2f - 1)*30`.
pub fn health_bar(top: Vec3, fraction: f32, right: Vec3) -> HealthBar {
    let f = fraction.clamp(0.0, 1.0);
    let c = top + Vec3::Y * 15.0;
    let at = |x: f32, y: f32| c + right * x + Vec3::Y * y;
    HealthBar {
        background: [at(-30.0, -4.0), at(30.0, 4.0)],
        fill: [at(-30.0, -4.0), at((2.0 * f - 1.0) * 30.0, 4.0)],
        red: 0xFFFF_0000,
        green: 0xFF00_FF00,
    }
}

/// `GameObject_MatchesClass(0x10000, 0, 0)` (`0x0046B9D0`): bit `0x10000` of the flags of module 0
/// (without the low byte); disabled modules count.
fn has_walk_class_10000(o: &GameObject) -> bool {
    o.has_module(0, true)
        && o.module(0)
            .is_some_and(|m| m.base.flags & !0xFF & 0x1_0000 != 0)
}

/// Does the object reach `FUN_0046C0F0` (`FUN_0047A490` with `0x4000000`): not dead (`0x40000`), not
/// quartered (`0x100`), no module 5 (nature), status `0x4000` only with class `0x10000`, and visible to the active
/// player (fog check `0x00467D70`). The bar is additionally limited there by attribute 14
/// ([`bar_drawn`]), the group digit is not.
fn overlay_wanted(sim: &SimHost, o: &GameObject) -> bool {
    o.status & (status::DIED | status::QUARTERED) == 0
        && !o.has_module(5, false)
        && (o.status & status::CARRIING == 0 || has_walk_class_10000(o))
        && sim.fog_selectable(o)
}

/// Whether `FUN_0046C0F0` draws a bar (`0x0046C26D..0x0046C2D9`): only with hit points (attribute 14).
fn bar_drawn(o: &GameObject) -> bool {
    o.attributes.is_present(HIT_POINTS)
}

/// Objects for which `FUN_0046C0F0` runs (bar and group digit): the selection in its order, then the
/// object under the mouse (each object once), by [`overlay_wanted`].
pub fn overlay_targets(
    sim: &SimHost,
    selection: &[ObjectId],
    hovered: Option<ObjectId>,
) -> Vec<ObjectId> {
    let mut out: Vec<ObjectId> = Vec::new();
    for &id in selection.iter().chain(hovered.as_ref()) {
        if !out.contains(&id)
            && sim
                .state
                .object(id, false)
                .is_some_and(|o| overlay_wanted(sim, o))
        {
            out.push(id);
        }
    }
    out
}

/// Objects with a health bar: [`overlay_targets`] with hit points (attribute 14).
pub fn bar_targets(
    sim: &SimHost,
    selection: &[ObjectId],
    hovered: Option<ObjectId>,
) -> Vec<ObjectId> {
    let mut out = overlay_targets(sim, selection, hovered);
    out.retain(|&id| sim.state.object(id, false).is_some_and(bar_drawn));
    out
}

/// Rank icon over the object (`FUN_0046BBE0`, from `ObjectManager_RenderObjects` for all objects,
/// re/combat-damage 4.1): returns when dead (`0x40000`); foreign objects additionally return when quartered
/// or `MASKED` (`0x100100`) and without a seen tile (`FUN_0047B320`, bit 1 of the fog state
/// at `+0x328/+0x32C`), own ones always; drawn for rank 1..5 and not quartered.
pub fn rank_icon_visible(sim: &SimHost, id: ObjectId) -> bool {
    let Some(o) = sim.state.object(id, false) else {
        return false;
    };
    (1..=RANK_ICONS.len() as u8).contains(&o.rank)
        && o.status & (status::DIED | status::QUARTERED) == 0
        && (o.owner() == sim.active_player
            || (o.status & status::MASKED == 0 && sim.fog_selectable(o)))
}

/// Counter-upgrade icon over the object (`FUN_00469F60`, from `GameObject_RenderRankIcon`
/// `0x0046BBE0`): not dead (`0x40000`, return at the start),
/// only in the branch "owner = local player" (foreign objects never show it) and only for
/// `m_ShowCounterUpgrade` (`+0x2C`) > 0; `FUN_00469F60` draws only with an enabled walk
/// (`HasModule(0, 0)`) or building module (`HasModule(3, 0)`). The icon thus hits the
/// affected player, not the trigger (the counter rises only for the trigger's enemies, re/upgrades 3.2).
pub fn counter_icon_visible(sim: &SimHost, o: &GameObject) -> bool {
    o.show_counter_upgrade > 0
        && o.status & status::DIED == 0
        && o.owner() == sim.active_player
        && (o.has_module(0, false) || o.has_module(3, false))
}

/// A bar in pixels (physical pixels): rectangles as `[top left, bottom right]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenBar {
    pub background: [[f32; 2]; 2],
    pub fill: [[f32; 2]; 2],
    /// Share of the fill 0..1 (`fraction(14) * 1e-5`, clamped): right texture edge of the fill.
    pub fraction: f32,
    pub red: u32,
    pub green: u32,
}

/// A rank icon in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenIcon {
    /// Rank 1..5.
    pub rank: u8,
    pub rect: [[f32; 2]; 2],
}

/// A group digit in pixels: the world billboard (30 x 30 world units, [`group_icons`]) projected like the
/// rank icons to a rectangle `[top left, bottom right]`; texture [`GROUP_ICONS`]`[group]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupIcon {
    /// First group 0..9 containing the object (`FUN_004FAF20`).
    pub group: u8,
    pub rect: [[f32; 2]; 2],
}

/// What a frame draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CombatOverlay {
    pub bars: Vec<ScreenBar>,
    pub icons: Vec<ScreenIcon>,
    /// Counter-upgrade icons (`[top left, bottom right]`).
    pub counter_icons: Vec<[[f32; 2]; 2]>,
    /// Group digits ([`group_icons`]); [`frame_overlay`] leaves them empty, the session of `nml-game` fills them
    /// (`GameSession::combat_cmds`). The viewer draws none.
    pub group_icons: Vec<GroupIcon>,
}

/// Rectangle from two corner points, ordered (top left, bottom right).
fn screen_rect(a: [f32; 2], b: [f32; 2]) -> [[f32; 2]; 2] {
    [
        [a[0].min(b[0]), a[1].min(b[1])],
        [a[0].max(b[0]), a[1].max(b[1])],
    ]
}

/// Bars and rank icons of the frame: `tops` = top edge of the hull per drawn object
/// (world space, center), `right` the right axis of the camera (billboard). The original draws a
/// billboard in the scene, but without depth (states `Font` and `3dicons-cubic`); here the
/// overlay is likewise on top (re/combat-host, re/game-ui).
/// Objects without a drawn model (e.g. construction sites while only the scaffolding stands) or behind the
/// camera are omitted.
pub fn frame_overlay(
    sim: &SimHost,
    selection: &[ObjectId],
    hovered: Option<ObjectId>,
    tops: &HashMap<ObjectId, Vec3>,
    view_proj: Mat4,
    right: Vec3,
    size: (u32, u32),
) -> CombatOverlay {
    let mut out = CombatOverlay::default();
    for id in bar_targets(sim, selection, hovered) {
        let (Some(&top), Some(o)) = (tops.get(&id), sim.state.object(id, false)) else {
            continue;
        };
        let fraction = o.attributes.current_fraction(HIT_POINTS) as f32 / FULL;
        let bar = health_bar(top, fraction, right);
        let corner = |p: Vec3| project(view_proj, p, size);
        let (Some(b0), Some(b1), Some(f0), Some(f1)) = (
            corner(bar.background[0]),
            corner(bar.background[1]),
            corner(bar.fill[0]),
            corner(bar.fill[1]),
        ) else {
            continue;
        };
        out.bars.push(ScreenBar {
            background: screen_rect(b0, b1),
            fill: screen_rect(f0, f1),
            fraction: fraction.clamp(0.0, 1.0),
            red: bar.red,
            green: bar.green,
        });
    }
    for o in sim.state.objects() {
        if o.rank == 0 || !rank_icon_visible(sim, o.id) {
            continue;
        }
        let Some(&top) = tops.get(&o.id) else {
            continue;
        };
        // Billboard 40 x 40 world units, center 30 above the top of the hull; mounted 50 higher, in the boat
        // shifted by 50 against the boat's yaw (re/riding 7.3, C-206).
        let c = top + rank_icon_offset(o.status, sim.display(o).1.y) + Vec3::Y * ICON_LIFT;
        let corner = |x: f32, y: f32| project(view_proj, c + right * x + Vec3::Y * y, size);
        let (Some(a), Some(b)) = (corner(-ICON_HALF, -ICON_HALF), corner(ICON_HALF, ICON_HALF))
        else {
            continue;
        };
        out.icons.push(ScreenIcon {
            rank: o.rank,
            rect: screen_rect(a, b),
        });
    }
    for o in sim.state.objects() {
        if !counter_icon_visible(sim, o) {
            continue;
        }
        let Some(&top) = tops.get(&o.id) else {
            continue;
        };
        // Walk: rider and occupant shift as with the rank icon; buildings without.
        let shift = if o.has_module(0, false) {
            rank_icon_offset(o.status, sim.display(o).1.y)
        } else {
            Vec3::ZERO
        };
        let c = top + shift + Vec3::Y * COUNTER_ICON_LIFT;
        let corner = |x: f32, y: f32| project(view_proj, c + right * x + Vec3::Y * y, size);
        let (Some(a), Some(b)) = (corner(-ICON_HALF, -ICON_HALF), corner(ICON_HALF, ICON_HALF))
        else {
            continue;
        };
        out.counter_icons.push(screen_rect(a, b));
    }
    out
}

/// Group digits of the frame (`FUN_0046C0F0` `0x0046C2DD..0x0046C335`, O-313): for each object of `targets`
/// ([`overlay_targets`]: selection and object under the mouse) of the active slot (`GameObject_GetOwnerSlot`
/// `0x004669C0` against the local slot) with a group in `groups` (the local player; first group,
/// [`ControlGroups::first_group_of`]). Anchor like the bar center: `top` + 15 in world y, 12 higher when a bar
/// is drawn ([`bar_drawn`]); the billboard 30 x 30 world units lies at x 15..45 along `right`, y -15..15
/// (world y like bar and rank icon). Like the bar without the rider and occupant shift of the original
/// (`0x0046C1B7..0x0046C21A`; the reimplementation's bar takes `top` unshifted). Objects without a drawn model or
/// behind the camera are omitted.
pub fn group_icons(
    sim: &SimHost,
    targets: &[ObjectId],
    groups: &ControlGroups,
    tops: &HashMap<ObjectId, Vec3>,
    view_proj: Mat4,
    right: Vec3,
    size: (u32, u32),
) -> Vec<GroupIcon> {
    let mut out = Vec::new();
    for &id in targets {
        let Some(o) = sim.state.object(id, false) else {
            continue;
        };
        if o.owner() != sim.active_player {
            continue;
        }
        let (Some(group), Some(&top)) = (groups.first_group_of(id), tops.get(&id)) else {
            continue;
        };
        let lift = if bar_drawn(o) {
            DIGIT_ANCHOR_LIFT + DIGIT_BAR_LIFT
        } else {
            DIGIT_ANCHOR_LIFT
        };
        let anchor = top + Vec3::Y * lift;
        let corner = |x: f32, y: f32| project(view_proj, anchor + right * x + Vec3::Y * y, size);
        let (Some(a), Some(b)) = (
            corner(DIGIT_X[0], DIGIT_HALF),
            corner(DIGIT_X[1], -DIGIT_HALF),
        ) else {
            continue;
        };
        out.push(GroupIcon {
            group,
            rect: screen_rect(a, b),
        });
    }
    out
}

/// Integer destination rectangle `[x0, y0, x1, y1]` (rounded) of a rectangle in pixels.
fn quad_dst(r: [[f32; 2]; 2]) -> [i32; 4] {
    [r[0][0], r[0][1], r[1][0], r[1][1]].map(|v| v.round() as i32)
}

/// The combat display as draw commands of the UI (drawing path of `nml-game`) in the
/// order of the world pass (`GameEngine_RenderWorld` `0x00438C60`): first rank and counter-upgrade icons (from
/// `ObjectManager_RenderObjects`, `0x0046BBE0`/`0x00469F60`, state `3dicons-cubic`: without depth, linearly filtered,
/// color white), then the health bars (`0x0047A490` with `0x4000000` -> `0x004693C0`, state `Font`: without depth,
/// point filter): red over the whole texture, green on top with `u` 0..fraction, both with [`BAR_TEXTURE`]; then the
/// group digits (`FUN_004697E0` after the bar in `FUN_0046C0F0`, state `3dicons-cubic`: linear, white,
/// [`GROUP_ICONS`]). Textures via `assets` (loaded once); if an icon is missing it is omitted, if the
/// bar texture is missing, flat color areas remain. The original draws the icons of one object directly in a row, here
/// all rank icons come before all counter-upgrade icons and all bars before all digits (for different
/// objects they almost never overlap).
pub fn overlay_cmds(
    overlay: &CombatOverlay,
    assets: &mut UiAssets,
    vfs: &Vfs,
    out: &mut Vec<DrawCmd>,
) {
    const WHITE: u32 = 0xFFFF_FFFF;
    let icon = |texture, rect| DrawCmd::Quad {
        texture: Some(texture),
        dst: quad_dst(rect),
        uv: [0.0, 0.0, 1.0, 1.0],
        color: WHITE,
        filter: Filter::Linear,
        blend: Blend::Alpha,
    };
    for i in &overlay.icons {
        let path = RANK_ICONS.get(usize::from(i.rank).wrapping_sub(1));
        if let Some(texture) = path.and_then(|p| assets.texture(vfs, p)) {
            out.push(icon(texture, i.rect));
        }
    }
    if !overlay.counter_icons.is_empty()
        && let Some(texture) = assets.texture(vfs, COUNTER_UPGRADE_ICON)
    {
        out.extend(overlay.counter_icons.iter().map(|&r| icon(texture, r)));
    }
    if !overlay.bars.is_empty() {
        let texture = assets.texture(vfs, BAR_TEXTURE);
        for bar in &overlay.bars {
            out.push(DrawCmd::Quad {
                texture,
                dst: quad_dst(bar.background),
                uv: [0.0, 0.0, 1.0, 1.0],
                color: bar.red,
                filter: Filter::Point,
                blend: Blend::Alpha,
            });
            out.push(DrawCmd::Quad {
                texture,
                dst: quad_dst(bar.fill),
                uv: [0.0, 0.0, bar.fraction, 1.0],
                color: bar.green,
                filter: Filter::Point,
                blend: Blend::Alpha,
            });
        }
    }
    for d in &overlay.group_icons {
        let path = GROUP_ICONS.get(usize::from(d.group));
        if let Some(texture) = path.and_then(|p| assets.texture(vfs, p)) {
            out.push(icon(texture, d.rect));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{COLONIST, HQ, host, rules, spawn};
    use crate::game::options::DEFAULT_GAME_SPEED;
    use crate::game::sim_host::SimHost;
    use glam::{Mat4, Vec3};
    use nml_sim::attributes::AttributeSet;
    use nml_sim::id::ObjectId;
    use nml_sim::object::status;
    use nml_sim::save::checksum;
    use nml_sim::setup::MatchSetup;
    use nml_sim::state::Control;
    use std::collections::HashMap;

    #[test]
    fn the_bar_is_60_by_8_world_units_red_behind_green_by_fraction() {
        let b = health_bar(Vec3::new(100.0, 50.0, 0.0), 0.25, Vec3::X);
        // Background x -30..30, fill x -30..(2f - 1)*30 = -15; height 15 above the hull, +-4.
        assert_eq!(
            b.background,
            [Vec3::new(70.0, 61.0, 0.0), Vec3::new(130.0, 69.0, 0.0)]
        );
        assert_eq!(
            b.fill,
            [Vec3::new(70.0, 61.0, 0.0), Vec3::new(85.0, 69.0, 0.0)]
        );
        assert_eq!((b.red, b.green), (0xFFFF_0000, 0xFF00_FF00));
        let empty = health_bar(Vec3::ZERO, -0.5, Vec3::X);
        assert_eq!(
            empty.fill[0],
            empty.fill[1] - Vec3::new(0.0, 8.0, 0.0),
            "fraction < 0 -> empty fill"
        );
    }

    #[test]
    fn the_rank_icon_of_a_passenger_is_offset_by_the_yaw() {
        let near = |a: Vec3, b: Vec3| (a - b).abs().max_element() < 1e-4;
        // `0x8000` (re/riding 7.3): locally (0, 0, -50) with RotationY(yaw) as a row vector.
        // Yaw pi/2 (direction 0) -> (-50, 0, 0); yaw pi (direction 90 degrees) -> (0, 0, +50).
        let half_pi = std::f32::consts::FRAC_PI_2;
        let a = rank_icon_offset(status::ISMOUNTEDONBOAT, half_pi);
        assert!(near(a, Vec3::new(-50.0, 0.0, 0.0)), "{a}");
        let b = rank_icon_offset(status::ISMOUNTEDONBOAT, std::f32::consts::PI);
        assert!(near(b, Vec3::new(0.0, 0.0, 50.0)), "{b}");
        // Yaw 0: locally (0, 0, -50) unrotated.
        assert!(near(
            rank_icon_offset(status::ISMOUNTEDONBOAT, 0.0),
            Vec3::new(0.0, 0.0, -50.0)
        ));
        // Mounted (`0x2000`): only +50 in y as before, independent of the yaw.
        assert_eq!(
            rank_icon_offset(status::ISMOUNTED, 1.0),
            Vec3::new(0.0, 50.0, 0.0)
        );
        // Otherwise no shift; further status bits change nothing.
        assert_eq!(rank_icon_offset(0, 1.0), Vec3::ZERO);
        assert_eq!(rank_icon_offset(status::MOVE, 1.0), Vec3::ZERO);
    }

    #[test]
    fn the_combat_line_names_stance_hit_points_rank_and_experience() {
        let attack = nml_control::SimAttackInfo {
            status: 0,
            target: None,
            automatic: false,
            aggressiveness: 4,
            mode: 3,
            stubbornness: [0; 3],
            search_counter: 0,
        };
        let mut info = nml_control::SimCombatInfo {
            hp: [199, 199],
            hp_fraction: 100_000,
            rank: 1,
            experience: 40_000,
            last_attacker: None,
            ticks_since_hit: 0,
            render_bits: 0,
            attack: Some(attack),
            dying: None,
        };
        assert_eq!(
            combat_line(&info),
            "Combat: Stance Offensive · HP 199/199 · Rank 1 · Experience 40 %"
        );
        let names: Vec<String> = (0..=5).map(stance_name).collect();
        assert_eq!(
            names,
            [
                "Cease fire",
                "Hold position",
                "Defensive",
                "Offensive",
                "Defend area",
                "5"
            ]
        );
        // Without an attack module no stance (buildings, animals without module 17).
        info.attack = None;
        info.hp = [0, 160];
        info.experience = 99_999;
        assert_eq!(
            combat_line(&info),
            "Combat: HP 0/160 · Rank 1 · Experience 99 %"
        );
    }

    #[test]
    fn rank_icons_name_the_original_files() {
        assert_eq!(RANK_ICONS[0], "data/icons/erfahrung01.tga");
        assert_eq!(RANK_ICONS[4], "data/icons/erfahrung05.tga");
    }

    /// Host like `host()`, but without a revealed map: nothing is seen.
    fn fogged_host() -> SimHost {
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
        host
    }

    /// Hit points (attribute 14) and rank, as a fighter has them; the colonist of the test world has
    /// none.
    fn arm(host: &mut SimHost, id: ObjectId, rank: u8) {
        let o = host.state.object_mut(id, false).unwrap();
        o.attributes = AttributeSet::initial(|a| if a == 14 { 100 } else { 0 }, false);
        o.rank = rank;
    }

    fn set_status(host: &mut SimHost, id: ObjectId, bits: u32) {
        host.state.object_mut(id, false).unwrap().status |= bits;
    }

    #[test]
    fn bars_go_to_the_selection_and_the_object_under_the_mouse_only() {
        let mut h = host();
        let own = spawn(&mut h, COLONIST, 0, (10, 10));
        let foe = spawn(&mut h, COLONIST, 1, (20, 10));
        let plain = spawn(&mut h, COLONIST, 0, (12, 10));
        let hq = spawn(&mut h, HQ, 0, (30, 30));
        let other = spawn(&mut h, COLONIST, 0, (14, 10));
        for id in [own, foe, other] {
            arm(&mut h, id, 0);
        }
        // Buildings are selectable only with a fog memory (`0x00467D70`): let one tick run.
        h.step_once();
        // Selection, then mouse object (also a foreign one); objects without selection and mouse: no bar.
        assert_eq!(bar_targets(&h, &[own], Some(foe)), [own, foe]);
        assert_eq!(bar_targets(&h, &[own], None), [own]);
        assert_eq!(bar_targets(&h, &[], Some(foe)), [foe]);
        assert_eq!(bar_targets(&h, &[], None), []);
        // The same object in the selection and under the mouse: one bar.
        assert_eq!(bar_targets(&h, &[own], Some(own)), [own]);
        // Without attribute 14 (colonist of the test world) no bar; buildings yes.
        assert_eq!(bar_targets(&h, &[plain, hq], None), [hq]);
    }

    #[test]
    fn dead_quartered_and_carried_objects_get_no_bar() {
        let mut h = host();
        let ids: Vec<_> = (0..4)
            .map(|i| spawn(&mut h, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        for &id in &ids {
            arm(&mut h, id, 0);
        }
        assert_eq!(bar_targets(&h, &ids, None), ids);
        set_status(&mut h, ids[0], status::DIED);
        set_status(&mut h, ids[1], status::QUARTERED);
        // Status 0x4000 (CARRIING) only with class 0x10000 of the walk module: the colonist of the
        // test world does not have it.
        set_status(&mut h, ids[2], status::CARRIING);
        assert_eq!(bar_targets(&h, &ids, Some(ids[0])), [ids[3]]);
    }

    #[test]
    fn the_bar_needs_the_object_to_be_seen_by_the_active_player() {
        let mut h = fogged_host();
        let foe = spawn(&mut h, COLONIST, 1, (40, 40));
        arm(&mut h, foe, 0);
        assert_eq!(
            bar_targets(&h, &[], Some(foe)),
            [],
            "foreign object in the fog"
        );
        nml_sim::fog::reveal_map(&mut h.state, -1);
        assert_eq!(bar_targets(&h, &[], Some(foe)), [foe]);
    }

    #[test]
    fn selected_foreigner_keeps_ring_and_bar_after_masking() {
        // re/world-display W.2.1, D-266: ring and health bar of the selection list (`0x0047A490`) check
        // only the fog (`0x00467D70`); an already selected foreign unit that masks itself keeps
        // both, masking does not deselect. The renderer supplies the bar's position even without a
        // drawn position (`SceneRenderer::object_tops` for `MASKED`).
        let mut h = host();
        let foe = spawn(&mut h, COLONIST, 1, (20, 10));
        arm(&mut h, foe, 0);
        let marked = |h: &SimHost| {
            crate::game::markers::ground_marker_objects(&h.state, &[foe], None, None, |o| {
                h.fog_selectable(o)
            })
        };
        let tops: HashMap<ObjectId, Vec3> = [(foe, Vec3::new(2624.0, 100.0, 1344.0))].into();
        let bars = |h: &SimHost| {
            let view_proj = Mat4::from_scale(Vec3::new(1.0 / 4096.0, 1.0 / 4096.0, 1.0 / 10000.0));
            frame_overlay(h, &[foe], None, &tops, view_proj, Vec3::X, (640, 480))
                .bars
                .len()
        };
        assert_eq!(bar_targets(&h, &[foe], None), [foe]);
        assert_eq!(marked(&h), [foe]);
        assert_eq!(bars(&h), 1);
        set_status(&mut h, foe, status::MASKED);
        assert!(!h.state.diplomacy.shares_vision(0, 1));
        assert_eq!(bar_targets(&h, &[foe], None), [foe], "bar stays");
        assert_eq!(marked(&h), [foe], "ring stays");
        assert_eq!(bars(&h), 1);
        // The rank icon, in contrast, is missing (D-265).
        h.state.object_mut(foe, false).unwrap().rank = 1;
        assert!(!rank_icon_visible(&h, foe));
    }

    #[test]
    fn the_rank_icon_follows_rank_quarter_ownership_fog_and_mask() {
        let mut h = fogged_host();
        let own = spawn(&mut h, COLONIST, 0, (10, 10));
        let foe = spawn(&mut h, COLONIST, 1, (40, 40));
        arm(&mut h, own, 0);
        arm(&mut h, foe, 1);
        // Rank 0: no icon.
        assert!(!rank_icon_visible(&h, own));
        // Own unit with rank 1: visible, even without a seen tile.
        h.state.object_mut(own, false).unwrap().rank = 1;
        assert!(rank_icon_visible(&h, own));
        // Foreign unit in the fog: no; revealed: yes; masked: no.
        assert!(!rank_icon_visible(&h, foe));
        nml_sim::fog::reveal_map(&mut h.state, -1);
        assert!(rank_icon_visible(&h, foe));
        set_status(&mut h, foe, status::MASKED);
        assert!(!rank_icon_visible(&h, foe));
        // Foreign and quartered (`0x100100`) or dead: no.
        h.state.object_mut(foe, false).unwrap().status = status::QUARTERED;
        assert!(!rank_icon_visible(&h, foe));
        h.state.object_mut(foe, false).unwrap().status = status::DIED;
        assert!(!rank_icon_visible(&h, foe));
        h.state.object_mut(foe, false).unwrap().status = 0;
        assert!(rank_icon_visible(&h, foe));
        // Quartered or dead unit: no (also the own one).
        set_status(&mut h, own, status::QUARTERED);
        assert!(!rank_icon_visible(&h, own));
        h.state.object_mut(own, false).unwrap().status = status::DIED;
        assert!(!rank_icon_visible(&h, own));
        // A rank outside 1..5 has no icon.
        h.state.object_mut(own, false).unwrap().status = 0;
        h.state.object_mut(own, false).unwrap().rank = 6;
        assert!(!rank_icon_visible(&h, own));
    }

    #[test]
    fn the_counter_upgrade_icon_marks_affected_objects() {
        let mut h = host();
        let own = spawn(&mut h, COLONIST, 0, (10, 10));
        let calm = spawn(&mut h, COLONIST, 0, (12, 10));
        let hq = spawn(&mut h, HQ, 0, (30, 30));
        let foe = spawn(&mut h, COLONIST, 1, (20, 10));
        arm(&mut h, own, 2);
        let view_proj = Mat4::from_scale(Vec3::splat(0.001));
        let size = (200, 200);
        let tops: HashMap<ObjectId, Vec3> = [
            (own, Vec3::ZERO),
            (calm, Vec3::new(500.0, 0.0, 0.0)),
            (hq, Vec3::new(-500.0, 0.0, 0.0)),
            (foe, Vec3::new(0.0, 0.0, 500.0)),
        ]
        .into();
        let frame = |h: &SimHost| frame_overlay(h, &[], None, &tops, view_proj, Vec3::X, size);
        // Without a running counter-upgrade (`m_ShowCounterUpgrade` = 0) no icon.
        assert!(frame(&h).counter_icons.is_empty());
        for id in [own, hq, foe] {
            h.state.object_mut(id, false).unwrap().show_counter_upgrade = 1;
        }
        let before = checksum(&h.state);
        let icons = frame(&h).counter_icons;
        assert_eq!(checksum(&h.state), before, "presentation changes nothing");
        // Own unit and own building; the foreign object does not show it (`0x0046BBE0`:
        // `FUN_00469F60` only in the branch "owner = local player").
        assert_eq!(icons.len(), 2, "{icons:?}");
        let centre = |r: [[f32; 2]; 2]| [(r[0][0] + r[1][0]) / 2.0, (r[0][1] + r[1][1]) / 2.0];
        // 40 x 40 world units (4 x 4 pixels), center 50 above the top of the hull (5 pixels),
        // so 20 above the center of the rank icon.
        let unit = icons[0];
        assert!((unit[1][0] - unit[0][0] - 4.0).abs() < 1e-3, "{unit:?}");
        assert!((unit[1][1] - unit[0][1] - 4.0).abs() < 1e-3, "{unit:?}");
        let c = centre(unit);
        assert!(
            (c[0] - 100.0).abs() < 1e-3 && (c[1] - 95.0).abs() < 1e-3,
            "{c:?}"
        );
        let c = centre(icons[1]);
        assert!(
            (c[0] - 50.0).abs() < 1e-3 && (c[1] - 95.0).abs() < 1e-3,
            "{c:?}"
        );
        // Dead: no icon (return at the start of `0x0046BBE0`).
        set_status(&mut h, own, status::DIED);
        assert_eq!(frame(&h).counter_icons.len(), 1);
        assert_eq!(COUNTER_UPGRADE_ICON, "data/icons/counterupgrade.tga");
    }

    #[test]
    fn the_draw_commands_put_icons_under_textured_bars() {
        use nml_ui::assets::UiAssets;
        use nml_ui::draw::{DrawCmd, Filter};
        let vfs = nml_rda::Vfs::from_archives(Vec::new());
        let mut assets = UiAssets::new();
        let bar_tex = assets.insert_image(BAR_TEXTURE, image::RgbaImage::new(32, 32));
        let rank2 = assets.insert_image(RANK_ICONS[1], image::RgbaImage::new(16, 16));
        let counter = assets.insert_image(COUNTER_UPGRADE_ICON, image::RgbaImage::new(16, 16));
        let overlay = CombatOverlay {
            bars: vec![ScreenBar {
                background: [[10.2, 20.0], [70.4, 28.0]],
                fill: [[10.2, 20.0], [25.3, 28.0]],
                fraction: 0.25,
                red: 0xFFFF_0000,
                green: 0xFF00_FF00,
            }],
            icons: vec![ScreenIcon {
                rank: 2,
                rect: [[30.0, 0.0], [50.0, 20.0]],
            }],
            counter_icons: vec![[[30.0, -10.0], [50.0, 10.0]]],
            group_icons: Vec::new(),
        };
        let mut out = Vec::new();
        overlay_cmds(&overlay, &mut assets, &vfs, &mut out);
        let quad = |texture, dst, uv, color, filter| DrawCmd::Quad {
            texture: Some(texture),
            dst,
            uv,
            color,
            filter,
            blend: Blend::Alpha,
        };
        // Icons from `ObjectManager_RenderObjects` (state `3dicons-cubic`, linear), then the bars from step 13
        // (`0x004693C0`, state `Font`, point filter): red whole, green with u 0..fraction, both with `balken.bmp`.
        assert_eq!(
            out,
            [
                quad(
                    rank2,
                    [30, 0, 50, 20],
                    [0.0, 0.0, 1.0, 1.0],
                    0xFFFF_FFFF,
                    Filter::Linear
                ),
                quad(
                    counter,
                    [30, -10, 50, 10],
                    [0.0, 0.0, 1.0, 1.0],
                    0xFFFF_FFFF,
                    Filter::Linear
                ),
                quad(
                    bar_tex,
                    [10, 20, 70, 28],
                    [0.0, 0.0, 1.0, 1.0],
                    0xFFFF_0000,
                    Filter::Point
                ),
                quad(
                    bar_tex,
                    [10, 20, 25, 28],
                    [0.0, 0.0, 0.25, 1.0],
                    0xFF00_FF00,
                    Filter::Point
                ),
            ]
        );
        // Without a rank texture (file missing) the icon is omitted; bars without a texture remain flat color areas.
        let mut bare = UiAssets::new();
        let mut out = Vec::new();
        overlay_cmds(&overlay, &mut bare, &vfs, &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(matches!(
            out[0],
            DrawCmd::Quad {
                texture: None,
                color: 0xFFFF_0000,
                ..
            }
        ));
    }

    /// O-313 (`FUN_0046C0F0` `0x0046C2DD..0x0046C335`): the group digit appears only on own
    /// objects that are selected or under the mouse, for the first group; billboard 30 x 30 world units,
    /// x 15..45, y -15..15 around the bar center (top of hull + 15), 12 higher when a bar is drawn. Without
    /// hit points (attribute 14) there is no bar, but the digit still appears (without +12). Drawn after the
    /// bars, `zahl_<i>.tga`, linear, white.
    #[test]
    fn group_digit_only_on_own_objects_with_a_bar() {
        use crate::game::control_groups::ControlGroups;
        let mut h = host();
        let own = spawn(&mut h, COLONIST, 0, (10, 10));
        let foe = spawn(&mut h, COLONIST, 1, (20, 10));
        let plain = spawn(&mut h, COLONIST, 0, (12, 10));
        let idle = spawn(&mut h, COLONIST, 0, (14, 10));
        let loose = spawn(&mut h, COLONIST, 0, (16, 10));
        for id in [own, foe, idle, loose] {
            arm(&mut h, id, 0);
        }
        let mut groups = ControlGroups::default();
        // Own object in G3 and G5 (list by hand): the first group counts (`FUN_004FAF20`).
        groups.lists[3] = vec![own];
        groups.lists[5] = vec![own];
        groups.lists[1] = vec![foe];
        groups.lists[2] = vec![plain];
        groups.lists[0] = vec![idle];
        let view_proj = Mat4::from_scale(Vec3::splat(0.001));
        let size = (200, 200);
        let tops: HashMap<ObjectId, Vec3> = [
            (own, Vec3::ZERO),
            (foe, Vec3::new(-500.0, 0.0, 0.0)),
            (plain, Vec3::new(500.0, 0.0, 0.0)),
            (idle, Vec3::new(0.0, 500.0, 0.0)),
            (loose, Vec3::new(0.0, -500.0, 0.0)),
        ]
        .into();
        let digits = |h: &SimHost, selection: &[ObjectId], hovered: Option<ObjectId>| {
            let targets = overlay_targets(h, selection, hovered);
            group_icons(h, &targets, &groups, &tops, view_proj, Vec3::X, size)
        };
        let before = checksum(&h.state);
        // Enemy selected (in G1 of the local player): no digit; own one in G3 and G5: 3; without hit points: 2.
        let icons = digits(&h, &[foe, own, plain, loose], None);
        assert_eq!(checksum(&h.state), before, "presentation changes nothing");
        assert_eq!(
            icons.iter().map(|i| i.group).collect::<Vec<_>>(),
            [3, 2],
            "{icons:?}"
        );
        // No bar for `plain` (attribute 14 missing), the digit applies anyway.
        assert_eq!(bar_targets(&h, &[own, plain], None), [own]);
        let near = |a: [[f32; 2]; 2], b: [[f32; 2]; 2]| {
            a.iter()
                .flatten()
                .zip(b.iter().flatten())
                .all(|(x, y)| (x - y).abs() < 1e-3)
        };
        // 1 world unit = 0.1 pixel; center of the frame (100, 100). With a bar: y center 15 + 12 = 27 above the hull.
        assert!(
            near(icons[0].rect, [[101.5, 95.8], [104.5, 98.8]]),
            "{:?}",
            icons[0].rect
        );
        // Without a bar: y center 15 above the hull, horizontally 15..45 right of the center.
        assert!(
            near(icons[1].rect, [[151.5, 97.0], [154.5, 100.0]]),
            "{:?}",
            icons[1].rect
        );
        // Under the mouse: yes; neither selected nor under the mouse: no; no group: no.
        let hovered = digits(&h, &[], Some(idle));
        assert_eq!(hovered.iter().map(|i| i.group).collect::<Vec<_>>(), [0]);
        assert!(digits(&h, &[], None).is_empty());
        assert!(digits(&h, &[loose], Some(loose)).is_empty());
        // Dead or quartered: no bar and no digit (filter of the caller `0x0047A490`).
        set_status(&mut h, own, status::DIED);
        assert!(digits(&h, &[own], None).is_empty());

        // Draw commands: bar, then the digits (`zahl_<i>.tga`, `3dicons-cubic`: linear, white).
        use nml_ui::assets::UiAssets;
        use nml_ui::draw::{DrawCmd, Filter};
        assert_eq!(GROUP_ICONS.len(), 10);
        for (i, path) in GROUP_ICONS.iter().enumerate() {
            assert_eq!(*path, format!("data/icons/zahl_{i}.tga"));
        }
        let vfs = nml_rda::Vfs::from_archives(Vec::new());
        let mut assets = UiAssets::new();
        let bar_tex = assets.insert_image(BAR_TEXTURE, image::RgbaImage::new(32, 32));
        let three = assets.insert_image(GROUP_ICONS[3], image::RgbaImage::new(16, 16));
        let mut overlay = CombatOverlay {
            bars: vec![ScreenBar {
                background: [[10.0, 20.0], [70.0, 28.0]],
                fill: [[10.0, 20.0], [70.0, 28.0]],
                fraction: 1.0,
                red: 0xFFFF_0000,
                green: 0xFF00_FF00,
            }],
            group_icons: vec![GroupIcon {
                group: 3,
                rect: [[41.6, 10.0], [70.4, 19.0]],
            }],
            ..CombatOverlay::default()
        };
        let digit = DrawCmd::Quad {
            texture: Some(three),
            dst: [42, 10, 70, 19],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: 0xFFFF_FFFF,
            filter: Filter::Linear,
            blend: Blend::Alpha,
        };
        let mut out = Vec::new();
        overlay_cmds(&overlay, &mut assets, &vfs, &mut out);
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(matches!(
            out[0],
            DrawCmd::Quad { texture: Some(t), color: 0xFFFF_0000, .. } if t == bar_tex
        ));
        assert_eq!(out[2], digit);
        // Without a bar the digit stays; if the file is missing, it is omitted.
        overlay.bars.clear();
        let mut out = Vec::new();
        overlay_cmds(&overlay, &mut assets, &vfs, &mut out);
        assert_eq!(out, [digit]);
        overlay.group_icons[0].group = 7;
        let mut out = Vec::new();
        overlay_cmds(&overlay, &mut assets, &vfs, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn the_frame_projects_bars_and_rank_icons_without_touching_the_simulation() {
        let mut h = host();
        let own = spawn(&mut h, COLONIST, 0, (10, 10));
        let calm = spawn(&mut h, COLONIST, 0, (12, 10));
        arm(&mut h, own, 2);
        arm(&mut h, calm, 0);
        // Side view: x to the right, y up, 200 x 200 pixels for 2000 world units
        // (1 world unit = 0.1 pixel).
        let view_proj = Mat4::from_scale(Vec3::splat(0.001));
        let size = (200, 200);
        let tops: HashMap<ObjectId, Vec3> =
            [(own, Vec3::ZERO), (calm, Vec3::new(500.0, 0.0, 0.0))].into();
        let before = checksum(&h.state);
        let frame = frame_overlay(&h, &[own], None, &tops, view_proj, Vec3::X, size);
        assert_eq!(checksum(&h.state), before, "presentation changes nothing");
        // One bar (only the selection), one rank icon (only `own` has a rank).
        assert_eq!(frame.bars.len(), 1);
        let bar = &frame.bars[0];
        // Background 60 world units = 6 pixels wide, center 15 above the hull.
        let width = bar.background[1][0] - bar.background[0][0];
        assert!((width - 6.0).abs() < 1e-3, "{width}");
        let centre_y = (bar.background[0][1] + bar.background[1][1]) / 2.0;
        assert!((centre_y - (100.0 - 1.5)).abs() < 1e-3, "{centre_y}");
        // Full fill (attribute 14 full): as wide as the background.
        assert_eq!(bar.fill, bar.background);
        assert_eq!(frame.icons.len(), 1);
        let icon = &frame.icons[0];
        assert_eq!(icon.rank, 2);
        // Billboard 40 x 40 world units (4 x 4 pixels), center 30 world units (3 pixels)
        // above the top of the hull, horizontally around its center.
        assert!((icon.rect[1][0] - icon.rect[0][0] - 4.0).abs() < 1e-3);
        assert!((icon.rect[1][1] - icon.rect[0][1] - 4.0).abs() < 1e-3);
        let centre = [
            (icon.rect[0][0] + icon.rect[1][0]) / 2.0,
            (icon.rect[0][1] + icon.rect[1][1]) / 2.0,
        ];
        assert!((centre[0] - 100.0).abs() < 1e-3, "{:?}", icon.rect);
        assert!((centre[1] - (100.0 - 3.0)).abs() < 1e-3, "{:?}", icon.rect);
        // Rider (`ISMOUNTED`): 50 world units higher.
        set_status(&mut h, own, status::ISMOUNTED);
        let frame = frame_overlay(&h, &[], None, &tops, view_proj, Vec3::X, size);
        let rect = frame.icons[0].rect;
        let centre_y = (rect[0][1] + rect[1][1]) / 2.0;
        assert!((centre_y - (100.0 - 8.0)).abs() < 1e-3, "{rect:?}");
    }
}
