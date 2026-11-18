//! Debug UI with egui: object inspector, players and accounts, tick and checksum (key I), plus the
//! selection box (the selection itself is shown by the ground rings, `markers`). Game-related
//! indicators stay in the original font (overlay). The rows arise without egui (testable);
//! [`EguiLayer`] binds egui to window and GPU. The window does not intervene in the simulation; the
//! debug button "complete immediately" of the queue sits in the "Economy" window next to "x"
//! ([`crate::economy_ui`], command `DebugCompleteOrder`).

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind};
use opennml_sim::combat::projectile::ProjectileKind;
use opennml_sim::economy::resource;
use opennml_sim::fx::{Fx, FxVec3};
use opennml_sim::id::ObjectId;
use opennml_sim::module::gather::hunting::{self, Hunting};
use opennml_sim::module::gather::{Gather, load, status as gather_status};
use opennml_sim::module::{MODULE_NAMES, ModuleKind, construct, fight_near};
use opennml_sim::object::GameObject;
use opennml_sim::state::{Control, SimState};
use winit::window::Window;

use opennml_client::game::sim_host::SimHost;

/// ID readable: `0x... (index i, slot s, generation g)`.
pub fn describe_id(id: ObjectId) -> String {
    if id == ObjectId::NONE {
        return "-".into();
    }
    format!(
        "{:#010x} (index {}, slot {}, generation {})",
        id.0,
        id.index(),
        id.slot(),
        id.generation()
    )
}

/// Rows of the inspector for an object: (name, value); empty if the object does not exist.
pub fn object_rows(sim: &SimHost, id: ObjectId) -> Vec<(String, String)> {
    let Some(o) = sim.state.object(id, false) else {
        return Vec::new();
    };
    let type_name = sim
        .rules
        .types
        .get(&o.guid)
        .map_or("?", |t| t.name.as_str());
    let [x, y, z] = o.world_pos;
    let anim = &o.anim;
    let mut rows = vec![
        ("ID".to_string(), describe_id(o.id)),
        ("Type".into(), format!("{} {type_name}", o.guid)),
        ("Owner".into(), o.owner().to_string()),
        ("State".into(), format!("{:#010x}", o.status)),
        ("Position".into(), format!("({x:.1}, {y:.1}, {z:.1})")),
        ("Tile".into(), format!("{:?}", o.tile)),
        (
            "Direction".into(),
            format!("{:.2} degrees", o.direction.to_f32()),
        ),
        (
            "Animation".into(),
            format!(
                "{} (wanted {}, start {} ms, {} starts{})",
                anim.current,
                anim.wanted,
                anim.start_ms,
                anim.starts,
                if anim.blend { ", crossfaded" } else { "" }
            ),
        ),
        ("Target".into(), describe_id(o.target)),
        (
            "Messages".into(),
            format!(
                "{} in the FIFO, {} commands (pointer {})",
                o.fifo.len(),
                o.commands.len(),
                o.command_cursor
            ),
        ),
        ("Tick group".into(), o.tick_group.to_string()),
        (
            "Rendering".into(),
            format!(
                "Render bits {:#x}, progress {:.3}{}",
                o.render_bits,
                o.progress_scale,
                if o.selectable { "" } else { ", not selectable" }
            ),
        ),
    ];
    if let Some(c) = sim.combat_info(o)
        && c.hp[1] != 0
    {
        rows.push((
            "Combat".into(),
            format!(
                "HP {}/{}, rank {}, experience {} %",
                c.hp[0],
                c.hp[1],
                c.rank,
                c.experience / 1000
            ),
        ));
    }
    if o.load_amount != 0 {
        rows.push((
            "Load".into(),
            format!("{} {}", o.load_amount, load_name(o.load_kind)),
        ));
    }
    if let Some(ModuleKind::Walk(w)) = o.module(0).map(|m| &m.kind) {
        let s = &w.speed;
        rows.push((
            "Walk".into(),
            format!(
                "State {} (before {}), end {}, {} waypoints, {} trajectory points",
                w.state,
                w.restore_state,
                w.ende,
                w.high_path.len(),
                w.trajectory.len()
            ),
        ));
        rows.push((
            "Group".into(),
            if w.group >= 0 {
                w.group.to_string()
            } else {
                "none".into()
            },
        ));
        rows.push(("Rank".into(), w.formation_num.to_string()));
        rows.push((
            "Speed".into(),
            format!(
                "{} per tick (default {}, cruising {}), rotation {:.0} degrees{}",
                s.cur,
                s.default,
                s.cruising,
                s.turn as f32 / 4096.0,
                if s.running { ", running" } else { "" }
            ),
        ));
        rows.push((
            "Planner".into(),
            format!(
                "B* {}, phase {}, mode {}, retries {}{}{}{}",
                w.last_bstar_result,
                w.phase,
                w.request.mode,
                w.retries,
                if w.barging { ", barging" } else { "" },
                if w.in_obstacle { ", in obstacle" } else { "" },
                if w.wait_for.is_none() {
                    String::new()
                } else {
                    format!(", waiting for {}", describe_id(w.wait_for))
                }
            ),
        ));
    }
    for (index, module) in o.modules.iter().enumerate() {
        let Some(module) = module else {
            continue;
        };
        let base = &module.base;
        let kind = match &module.kind {
            ModuleKind::Inert => "inert",
            ModuleKind::Walk(_) => "Walk",
            ModuleKind::Stand(_) => "Stand",
            ModuleKind::Building(_) => "Building",
            ModuleKind::Tree(_) => "Tree",
            ModuleKind::Natur(_) => "Nature",
            ModuleKind::Resource(_) => "Resource",
            ModuleKind::Lumber(_) => "Lumbering",
            ModuleKind::Mining(_) => "Gold mining",
            ModuleKind::Harvest(_) => "Harvest",
            ModuleKind::Fishing(_) => "Fishing",
            ModuleKind::Quarter(_) => "Quarters",
            ModuleKind::SelfHealing(_) => "Self-healing",
            ModuleKind::Construct(_) => "Construct",
            ModuleKind::Attack(_) => "Attack",
            ModuleKind::FightNear(_) => "Melee",
            ModuleKind::Dying(_) => "Dying",
            ModuleKind::FightFar(_) => "Ranged combat",
            ModuleKind::Shoot(_) => "Shot",
            ModuleKind::Animal(_) => "Animal",
            ModuleKind::Edible(_) => "Carcass",
            ModuleKind::Hunting(_) => "Hunting",
            ModuleKind::Riding(_) => "Riding",
            ModuleKind::Swim(_) => "Swimming",
            ModuleKind::Diving(_) => "Diving",
            ModuleKind::Train(_) => "Train",
            ModuleKind::MultiHead(_) => "Multipart front",
            ModuleKind::MultiPart(_) => "Multipart rear",
            ModuleKind::GlobalUpgrade(_) => "Global upgrade",
            ModuleKind::Spell(_) => "Spell",
            ModuleKind::ExecTempUpgrade(_) => "Execute ability",
            ModuleKind::TransportTent(_) => "Tent transport",
            ModuleKind::Tent(_) => "Tent",
            ModuleKind::Trading(_) => "Trading",
            ModuleKind::Masking(_) => "Camouflage",
            ModuleKind::Assassin(_) => "Dig in",
            ModuleKind::Healing(_) => "Healing",
            ModuleKind::EvokeGhost(_) => "Evoke ghost",
            ModuleKind::Ghost(_) => "Ghost",
            ModuleKind::Banish(_) => "Banish",
            ModuleKind::EvokeAnimal(_) => "Evoke animal",
            ModuleKind::Bomberman(_) => "Demolition expert",
            ModuleKind::Killer(_) => "Killer",
            ModuleKind::SpecialMove(_) => "Smith",
            ModuleKind::HeadHunter(_) => "Headhunter",
            ModuleKind::Robbing(_) => "Robber",
        };
        rows.push((
            format!(
                "Module {index} {}",
                MODULE_NAMES.get(index).copied().unwrap_or("?")
            ),
            format!(
                "{kind}, {}{}, flags {:#x}, variants {:#x}, {} timers",
                if base.enabled { "on" } else { "off" },
                if base.idle { ", idle" } else { "" },
                base.flags,
                base.prop_bits,
                base.events.len() + base.timers_ws.len()
            ),
        ));
        rows.extend(module_rows(&sim.state, o, &module.kind));
    }
    rows
}

/// Name of a resource kind (`load::*`, kind of the credit).
pub fn load_name(kind: i32) -> &'static str {
    match kind {
        load::GOLD => "Gold",
        load::WOOD => "Wood",
        load::FOOD => "Food",
        _ => "-",
    }
}

/// Grid position (fixed point) in tiles.
fn tiles(p: FxVec3) -> String {
    format!("({:.2}, {:.2})", p.x.to_f32(), p.z.to_f32())
}

/// State of the gatherer base, readable.
fn gather_state(status: i32) -> String {
    match status {
        gather_status::IDLE => "free".into(),
        gather_status::TO_SOURCE => "to source".into(),
        gather_status::TO_DELIVERY => "to storage".into(),
        gather_status::DELIVERING => "delivering".into(),
        gather_status::SEARCH => "searching".into(),
        200 => "chopping".into(),
        300 => "mining gold".into(),
        100 => "harvesting".into(),
        400 => "fishing".into(),
        other => other.to_string(),
    }
}

/// State of the hunter (module 22), readable: 100-102 are states of hunting, not of harvesting; the
/// others as for any gatherer.
fn hunting_state(status: i32) -> String {
    match status {
        hunting::state::GOTO_CARCASS => "to carcass".into(),
        hunting::state::SEARCH => "searching game".into(),
        hunting::state::HUNTING => "hunting".into(),
        other => gather_state(other),
    }
}

/// State of a construction worker, readable.
fn construct_state(status: i32) -> &'static str {
    match status {
        construct::state::IDLE => "free",
        construct::state::STARTED => "starting",
        construct::state::BUILDING => "building",
        construct::state::ARRIVED => "arrived",
        construct::state::WALKING => "walking to the construction site",
        construct::state::REPAIRING => "repairing",
        construct::state::TARGET_DONE => "target done",
        _ => "?",
    }
}

/// State of the attack module (`m_Status`), readable.
fn attack_state(status: i32) -> String {
    match status {
        0 => "calm".into(),
        1 => "approach".into(),
        2 => "strike running".into(),
        other => other.to_string(),
    }
}

/// `m_Aggressiveness`, readable.
fn aggressiveness_name(value: i32) -> &'static str {
    match value {
        1 => "no combat",
        2 => "retaliation",
        3 => "escalatable",
        4 => "target search",
        _ => "?",
    }
}

/// Contents of the economy modules for the inspector: building (construction, production, docking slots,
/// rally point), gatherer (state, load, source, storage), construction worker, quarters. Attribute values
/// with the upgrade percentage of the type from `state` (like `sim_query`).
pub fn module_rows(state: &SimState, o: &GameObject, kind: &ModuleKind) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let gatherer = |g: &Gather| {
        (
            "  Gatherer".to_string(),
            format!(
                "{}, load {} {}, source {}, storage {}",
                gather_state(g.status),
                o.load_amount,
                load_name(o.load_kind),
                describe_id(g.resource_hid),
                describe_id(g.hq_hid)
            ),
        )
    };
    // Hunting has a target instead of the source (`m_TargetHID`: animal or carcass).
    let hunter = |h: &Hunting| {
        (
            "  Gatherer".to_string(),
            format!(
                "{}, load {} {}, target {}, storage {}",
                hunting_state(h.gather.status),
                o.load_amount,
                load_name(o.load_kind),
                describe_id(h.target_hid),
                describe_id(h.gather.hq_hid)
            ),
        )
    };
    match kind {
        ModuleKind::Building(b) => {
            let done = b.build_time.max(1).saturating_sub(b.build_counter);
            let pct = i64::from(done) * 100 / i64::from(b.build_time.max(1));
            rows.push((
                "  Construction".into(),
                format!(
                    "{}, remaining {} of {} ({pct} %){}{}",
                    if b.is_build_building {
                        "construction site"
                    } else {
                        "done"
                    },
                    b.build_counter,
                    b.build_time,
                    if b.is_dead { ", dead" } else { "" },
                    if b.is_sleeping { ", sleeping" } else { "" }
                ),
            ));
            let queue: Vec<String> = b.make_queue.iter().map(u32::to_string).collect();
            rows.push((
                "  Production".into(),
                if b.make_guid == 0 && queue.is_empty() {
                    "-".into()
                } else {
                    format!(
                        "{} (remaining {} ms of {}), queue [{}]",
                        b.make_guid,
                        b.make_time,
                        b.make_start,
                        queue.join(", ")
                    )
                },
            ));
            let used = b
                .parking_lots
                .iter()
                .filter(|p| p.hid != ObjectId::NONE)
                .count();
            rows.push((
                "  Docking slots".into(),
                format!(
                    "{used} occupied, {} free",
                    b.parking_lots.len().saturating_sub(used)
                ),
            ));
            rows.push((
                "  Rally point".into(),
                format!(
                    "{} tiles, target {}",
                    tiles(b.meeting_point),
                    describe_id(b.meeting_target)
                ),
            ));
            if b.target != ObjectId(0) && b.target != ObjectId::NONE {
                rows.push(("  Deposit".into(), describe_id(b.target)));
            }
        }
        ModuleKind::Lumber(l) => rows.push(gatherer(&l.gather)),
        ModuleKind::Mining(m) => rows.push(gatherer(&m.gather)),
        ModuleKind::Harvest(h) => rows.push(gatherer(&h.gather)),
        ModuleKind::Fishing(f) => rows.push(gatherer(&f.gather)),
        ModuleKind::Hunting(h) => rows.push(hunter(h)),
        ModuleKind::Construct(c) => rows.push((
            "  Construction worker".into(),
            format!(
                "{}, building {}, speed {}",
                construct_state(c.status),
                describe_id(c.building),
                c.speed
            ),
        )),
        ModuleKind::Quarter(q) => {
            let occupants: Vec<String> = q.occupants.iter().map(|id| describe_id(*id)).collect();
            rows.push((
                "  Quarters".into(),
                format!("{}/{} occupied: {}", q.cur, q.max, occupants.join("; ")),
            ));
        }
        ModuleKind::Attack(a) => rows.push((
            "  Attack".into(),
            format!(
                "{}, stance {} ({}), aggressiveness {} ({}), target {}, stubbornness {}/{}/{}, {}",
                attack_state(a.status),
                opennml_client::game::combat_calc::stance_name(a.mode),
                a.mode,
                aggressiveness_name(a.aggressiveness),
                a.aggressiveness,
                describe_id(a.target),
                a.stubbornness[0],
                a.stubbornness[1],
                a.stubbornness[2],
                if a.automatic {
                    "automatic"
                } else {
                    "commanded"
                }
            ),
        )),
        // The swing runs as long as its timers (group 0x80) are pending.
        ModuleKind::FightNear(f) => {
            let swinging = o
                .module(fight_near::INDEX)
                .is_some_and(|m| !m.base.events.is_empty());
            rows.push((
                "  Melee".into(),
                format!(
                    "{}, target {}",
                    if swinging { "swing open" } else { "no swing" },
                    describe_id(f.target)
                ),
            ));
        }
        ModuleKind::Dying(d) => rows.push((
            "  Dying".into(),
            format!(
                "{}, sinking {} ms",
                match d.status {
                    0 => "calm",
                    1 => "died",
                    2 => "sinking",
                    _ => "?",
                },
                d.die_counter
            ),
        )),
        ModuleKind::Resource(r) if r.resource_id != 0 => rows.push((
            "  Stock".into(),
            o.attributes
                .current_value(
                    r.resource_id,
                    opennml_sim::upgrades::type_upgrade_percent(
                        state,
                        o.owner(),
                        o.guid,
                        r.resource_id,
                    ),
                )
                .to_string(),
        )),
        _ => {}
    }
    rows
}

/// Rows of the "Projectiles" section: the flying projectiles of the simulation (kind, remaining time of
/// the flight time, shooter, target) and below them the number of flights of the host. Read only.
pub fn projectile_rows(state: &SimState, host_flights: usize) -> Vec<String> {
    let list = &state.projectiles.list;
    let mut rows = vec![format!("Simulation: {} projectiles", list.len())];
    for p in list {
        let kind = match p.kind {
            ProjectileKind::Arrow if p.flags & 0x40 != 0 => "Fire arrow",
            ProjectileKind::Arrow => "Arrow",
            ProjectileKind::CannonBall => "Cannonball",
            ProjectileKind::Dynamite => "Dynamite",
            ProjectileKind::Explosion => "Explosion",
            ProjectileKind::Storm => "Storm",
            ProjectileKind::Throw => "Dynamite throw",
        };
        // Storm: `zone` is the pulse counter (0 or 1, re/spells 3.8).
        let pulse = if p.kind == ProjectileKind::Storm {
            format!(", pulse {}", p.zone)
        } else {
            String::new()
        };
        rows.push(format!(
            "{} {kind}: remaining {}/{} ms, shooter {}, target {}{pulse}",
            p.id,
            p.remaining_ms,
            p.flight_ms,
            describe_id(p.shooter),
            describe_id(p.target)
        ));
    }
    rows.push(format!("Host: {host_flights} flights"));
    rows
}

/// Rows of the "Groups" section: per group ID, type, members, anchor, march, frames, `ready` and
/// `reported`/`arrived`. Read only.
pub fn group_rows(state: &SimState) -> Vec<String> {
    let groups = &state.groups.map;
    let mut rows = vec![format!("{} groups", groups.len())];
    for (_, g) in groups.iter() {
        let members: Vec<String> = g.members.iter().map(|m| m.0.to_string()).collect();
        let mut row = format!(
            "{} type {}: {} members [{}], anchor ({}, {}, {}){}",
            g.id,
            g.kind,
            g.members.len(),
            members.join(", "),
            g.anchor.0,
            g.anchor.1,
            g.anchor.2,
            if g.marching { ", marching" } else { "" }
        );
        if let Some(f) = &g.formation {
            row.push_str(&format!(
                ", {} frames, ready {}, reported {}, arrived {}",
                f.frames.len(),
                f.ready.len(),
                f.reported,
                f.arrived
            ));
        }
        rows.push(row);
    }
    rows
}

/// Rows "Economy": living space limit, market prices (gold per good, fixed point) and resource factor
/// of slots 0-7.
pub fn economy_rows(state: &SimState) -> Vec<String> {
    let e = &state.economy;
    let price = |p: i32| format!("{:.3} Gold ({p})", Fx(p).to_f32());
    vec![
        format!("Living space limit {}", e.max_living_space),
        format!(
            "Market: Wood {}, Food {}",
            price(e.wood_price),
            price(e.food_price)
        ),
        format!(
            "Resource factor %: {}",
            e.resource_factor
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ]
}

/// Columns of the player table.
pub const PLAYER_COLUMNS: [&str; 10] = [
    "Slot", "Control", "Name", "Nation", "Team", "Gold", "Wood", "Food", "Enemies", "Sight",
];

/// Slots of a bit mask as a list, e.g. `1,3`.
fn slot_list(mask: u8) -> String {
    (0..8u8)
        .filter(|q| mask & (1 << q) != 0)
        .map(|q| q.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Rows of the player table (occupied slots 0-7): resources in whole units, plus population
/// occupied/available, enemies and shared vision (this slot sees what the named ones see) as slot
/// lists.
pub fn player_rows(state: &SimState) -> Vec<Vec<String>> {
    state
        .players
        .iter()
        .enumerate()
        .take(8)
        .filter(|(_, p)| p.control != Control::Empty)
        .map(|(slot, p)| {
            let r: [i32; 5] =
                std::array::from_fn(|i| p.resource(i, state.economy.max_living_space));
            vec![
                slot.to_string(),
                format!("{:?}", p.control),
                p.name.clone(),
                opennml_formats::sound::NATIONS
                    .get(p.nation as usize)
                    .copied()
                    .unwrap_or("?")
                    .to_string(),
                p.team.to_string(),
                r[resource::GOLD].to_string(),
                r[resource::WOOD].to_string(),
                format!(
                    "{} (pop. {}/{})",
                    r[resource::FOOD],
                    r[resource::POPULATION],
                    r[resource::LIVING_SPACE]
                ),
                slot_list(state.diplomacy.enemy[slot]),
                slot_list(state.diplomacy.view[slot]),
            ]
        })
        .collect()
}

/// Rows of the "Fog" section: cycle, sight sources, tile counts of the active player (with shared
/// vision), "fog off" and overlay level. Read only.
pub fn fog_rows(sim: &SimHost) -> Vec<String> {
    let r = opennml_client::game::fog_view::fog_result(&sim.state, sim.active_player, None);
    let overlay = match sim.fog.overlay {
        None => "off",
        Some(1) => "explored",
        Some(3) => "seen",
        Some(_) => "cleared",
    };
    vec![
        format!(
            "Cycle {}/{}/{} (processed/player/per tick)",
            r.processed, r.cycle_player, r.per_tick
        ),
        format!("Sight sources {}", r.sources),
        format!(
            "Player {}: explored {}, seen {}, cleared {} of {} tiles",
            r.player,
            r.explored,
            r.seen,
            r.cleared,
            r.width * r.height
        ),
        format!(
            "Fog off (O): {}",
            if sim.fog.show_mode == 0 { "no" } else { "yes" }
        ),
        format!("Overlay (F): {overlay}"),
    ]
}

/// What the UI shows per frame (screen points in physical pixels).
pub struct DebugView<'a> {
    pub sim: &'a SimHost,
    /// Object of the inspector (first selected or the one under the mouse).
    pub inspected: Option<ObjectId>,
    pub selected: usize,
    /// Running selection box.
    pub drag: Option<([f32; 2], [f32; 2])>,
    /// Overlay of the pathfinding: tile quads (screen points) with fill color.
    pub grid_cells: Vec<([[f32; 2]; 4], [u8; 4])>,
    /// Tiles of the tile path (outline) and the line unit -> waypoints -> target.
    pub path_tiles: Vec<[[f32; 2]; 4]>,
    pub path_line: Vec<[f32; 2]>,
    /// Rows of the "Pathfinding" section in the debug window.
    pub path_rows: Vec<String>,
    /// Number of projectile flights of the host ("Projectiles" section).
    pub flights: usize,
}

/// Builds the UI of a frame; `open` = window visible (key I), `match_open` = "Match" window
/// (button in the window, [`crate::ki_ui::build_match`]), `script_open` = "Script" window (button in
/// the window, key J, [`crate::script_ui::build_script`]).
pub fn build(
    ctx: &egui::Context,
    open: &mut bool,
    match_open: &mut bool,
    script_open: &mut bool,
    view: &DebugView,
) {
    let ppp = ctx.pixels_per_point();
    let point = |p: [f32; 2]| Pos2::new(p[0] / ppp, p[1] / ppp);
    let painter = ctx.layer_painter(egui::LayerId::background());
    for (quad, [r, g, b, a]) in &view.grid_cells {
        painter.add(egui::Shape::convex_polygon(
            quad.iter().map(|&p| point(p)).collect(),
            Color32::from_rgba_unmultiplied(*r, *g, *b, *a),
            Stroke::NONE,
        ));
    }
    for quad in &view.path_tiles {
        painter.add(egui::Shape::closed_line(
            quad.iter().map(|&p| point(p)).collect(),
            Stroke::new(1.0, Color32::YELLOW),
        ));
    }
    if view.path_line.len() >= 2 {
        painter.add(egui::Shape::line(
            view.path_line.iter().map(|&p| point(p)).collect(),
            Stroke::new(2.5, Color32::from_rgb(0, 230, 255)),
        ));
    }
    if let Some((a, b)) = view.drag {
        painter.rect_stroke(
            Rect::from_two_pos(point(a), point(b)),
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
    }
    let sim = view.sim;
    egui::Window::new("Simulation")
        .open(open)
        .default_pos([16.0, 220.0])
        .default_width(460.0)
        .default_height(480.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.label(format!(
                "Tick {} ({} steps) | game type {} | GameSpeed {}",
                sim.state.tick, sim.state.steps, sim.state.mode, sim.game_speed
            ));
            ui.label(format!(
                "Checksum {:016x} | active player {} | {} selected",
                sim.checksum(),
                sim.active_player,
                view.selected
            ));
            ui.horizontal(|ui| {
                ui.label(format!(
                    "AI {} | difficulty {} | script {}",
                    if sim.setup.ki.enabled { "on" } else { "off" },
                    sim.setup.ki.campaign_difficulty,
                    if sim.setup.script.enabled {
                        "on"
                    } else {
                        "off"
                    }
                ));
                if ui.button("Match ...").clicked() {
                    *match_open = true;
                }
                if ui.button("Script ...").clicked() {
                    *script_open = true;
                }
            });
            egui::CollapsingHeader::new("Players")
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("players").striped(true).show(ui, |ui| {
                        for c in PLAYER_COLUMNS {
                            ui.strong(c);
                        }
                        ui.end_row();
                        for row in player_rows(&sim.state) {
                            for cell in row {
                                ui.label(cell);
                            }
                            ui.end_row();
                        }
                    });
                });
            egui::CollapsingHeader::new("Economy")
                .default_open(false)
                .show(ui, |ui| {
                    for row in economy_rows(&sim.state) {
                        ui.label(row);
                    }
                });
            egui::CollapsingHeader::new("Projectiles")
                .default_open(false)
                .show(ui, |ui| {
                    for row in projectile_rows(&sim.state, view.flights) {
                        ui.label(row);
                    }
                });
            egui::CollapsingHeader::new("Groups")
                .default_open(false)
                .show(ui, |ui| {
                    for row in group_rows(&sim.state) {
                        ui.label(row);
                    }
                });
            egui::CollapsingHeader::new("Fog")
                .default_open(false)
                .show(ui, |ui| {
                    for row in fog_rows(sim) {
                        ui.label(row);
                    }
                });
            if !view.path_rows.is_empty() {
                egui::CollapsingHeader::new("Pathfinding")
                    .default_open(true)
                    .show(ui, |ui| {
                        for row in &view.path_rows {
                            ui.label(row);
                        }
                    });
            }
            egui::CollapsingHeader::new("Object")
                .default_open(true)
                .show(ui, |ui| match view.inspected {
                    None => {
                        ui.label("nothing selected (left click, selection box)");
                    }
                    Some(id) => {
                        egui::Grid::new("object").striped(true).show(ui, |ui| {
                            for (name, value) in object_rows(sim, id) {
                                ui.label(name);
                                ui.label(value);
                                ui.end_row();
                            }
                        });
                    }
                });
        });
}

/// Texture changes of a pass: `upload` for every new or changed texture, the freed ones go to
/// `free_after_paint` (free them only after painting). Empties the delta: egui checks in the debug build on
/// dropping that nothing was left unprocessed.
fn apply_textures(
    mut delta: egui::TexturesDelta,
    mut upload: impl FnMut(egui::TextureId, &egui::epaint::ImageDelta),
    free_after_paint: &mut Vec<egui::TextureId>,
) {
    for (id, deltas) in delta.set.drain() {
        for image in &deltas {
            upload(id, image);
        }
    }
    free_after_paint.extend(delta.free.drain());
}

/// egui on window and GPU: input, one pass per frame, painting over the finished frame.
pub struct EguiLayer {
    pub ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    paint: Vec<egui::ClippedPrimitive>,
    textures_free: Vec<egui::TextureId>,
}

impl EguiLayer {
    /// Without animations (expanding and collapsing, fading in): same steps yield same frames.
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Self {
            ctx,
            state,
            renderer,
            paint: Vec::new(),
            textures_free: Vec::new(),
        }
    }

    /// Window event to egui; `true` if egui claims it for itself (e.g. a click into the
    /// window).
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Screen point (physical pixels) in egui points.
    fn points(&self, x: f64, y: f64) -> Pos2 {
        let ppp = self.ctx.pixels_per_point();
        Pos2::new(x as f32 / ppp, y as f32 / ppp)
    }

    /// Mouse movement of the control channel to egui (takes effect in the next pass).
    pub fn pointer_moved(&mut self, x: f64, y: f64) {
        let pos = self.points(x, y);
        self.state
            .egui_input_mut()
            .events
            .push(egui::Event::PointerMoved(pos));
    }

    /// Pointer of the control channel leaves the window.
    pub fn pointer_gone(&mut self) {
        self.state
            .egui_input_mut()
            .events
            .push(egui::Event::PointerGone);
    }

    /// Mouse button of the control channel to egui; `true` if the point lies over an egui window of the
    /// last pass (then a press belongs to egui, not to the map).
    pub fn pointer_button(
        &mut self,
        x: f64,
        y: f64,
        primary: bool,
        pressed: bool,
        shift: bool,
        ctrl: bool,
    ) -> bool {
        let pos = self.points(x, y);
        self.state
            .egui_input_mut()
            .events
            .push(egui::Event::PointerButton {
                pos,
                button: if primary {
                    egui::PointerButton::Primary
                } else {
                    egui::PointerButton::Secondary
                },
                pressed,
                modifiers: egui::Modifiers {
                    shift,
                    ctrl,
                    command: ctrl,
                    ..Default::default()
                },
            });
        self.ctx
            .layer_id_at(pos)
            .is_some_and(|l| l.order != egui::Order::Background)
    }

    /// One pass at time `now_ms` of the virtual clock (instead of the real time of egui-winit): take
    /// over input, run `build`, queue textures and triangles.
    pub fn run(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        now_ms: f64,
        mut build: impl FnMut(&egui::Context),
    ) {
        let mut input = self.state.take_egui_input(window);
        input.time = Some(now_ms / 1000.0);
        let output = self.ctx.run_ui(input, |ui| build(ui.ctx()));
        self.state
            .handle_platform_output(window, output.platform_output);
        let renderer = &mut self.renderer;
        apply_textures(
            output.textures_delta,
            |id, delta| renderer.update_texture(device, queue, id, delta),
            &mut self.textures_free,
        );
        self.paint = self.ctx.tessellate(output.shapes, output.pixels_per_point);
    }

    /// Paints the last pass over `target` (size in physical pixels).
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.0, size.1],
            pixels_per_point: self.ctx.pixels_per_point(),
        };
        let extra = self
            .renderer
            .update_buffers(device, queue, encoder, &self.paint, &screen);
        if !extra.is_empty() {
            queue.submit(extra);
        }
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        self.renderer.render(&mut pass, &self.paint, &screen);
        drop(pass);
        for id in std::mem::take(&mut self.textures_free) {
            self.renderer.free_texture(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_GAME_SPEED;
    use opennml_sim::rules::Rules;
    use opennml_sim::setup::MatchSetup;
    use opennml_sim::world::{ObjectDesc, create_object};

    const TYPES: &str =
        "GUID 1100\nNAME Colonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n";

    fn host() -> (SimHost, ObjectId) {
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1100, 2, [64.0, 0.0, 64.0])).unwrap();
        host.state.players[2].control = Control::Human;
        host.state.players[2].nation = 2;
        (host, id)
    }

    #[test]
    fn the_inspector_lists_id_type_animation_and_modules() {
        let (host, id) = host();
        let rows = object_rows(&host, id);
        let get = |name: &str| rows.iter().find(|r| r.0 == name).unwrap().1.clone();
        assert!(get("ID").contains("slot 2"), "{}", get("ID"));
        assert_eq!(get("Type"), "1100 Colonist");
        assert_eq!(get("Owner"), "2");
        assert!(get("Animation").starts_with("0 "), "{}", get("Animation"));
        let stand = rows.iter().find(|r| r.0 == "Module 2 STAND").unwrap();
        assert!(stand.1.starts_with("Stand, on"), "{}", stand.1);
        assert!(rows.iter().any(|r| r.0 == "Module 0 PTTOPT"));
        assert!(object_rows(&host, ObjectId::NONE).is_empty());
        assert_eq!(describe_id(ObjectId::NONE), "-");
    }

    #[test]
    fn the_stock_row_counts_the_upgrade_percent() {
        // "Stock" with the upgrade percentage of the type like `sim_query`.
        // Gold (attribute 26) base value 200, synthetic upgrade +50 % for 1100 on slot 2.
        let (mut host, id) = host();
        host.state
            .object_mut(id, false)
            .unwrap()
            .attributes
            .set_modifier(26, 200, false, 0);
        let kind =
            ModuleKind::Resource(opennml_sim::module::resource::Resource { resource_id: 26 });
        let stock = |host: &SimHost| {
            let o = host.state.object(id, false).unwrap();
            module_rows(&host.state, o, &kind)
        };
        assert_eq!(stock(&host), [("  Stock".to_string(), "200".to_string())]);
        host.state.type_upgrades[2]
            .entry(1100)
            .or_default()
            .set_percent(9999, 26, 50);
        assert_eq!(stock(&host), [("  Stock".to_string(), "300".to_string())]);
    }

    #[test]
    fn the_hunter_row_names_the_hunting_states_and_shows_the_target() {
        // 100 is "to carcass" when hunting (not "harvesting"), 101/102 had no name, and the target
        // (`m_TargetHID`) was missing.
        let (host, id) = host();
        let o = host.state.object(id, false).unwrap();
        let target = ObjectId::new(3, 4, 5);
        let row = |status: i32| {
            let mut h = Hunting::default();
            h.gather.status = status;
            h.target_hid = target;
            module_rows(&host.state, o, &ModuleKind::Hunting(Box::new(h)))
        };
        for (status, name) in [
            (100, "to carcass"),
            (101, "searching game"),
            (102, "hunting"),
            (0, "free"),
            (1, "to source"),
        ] {
            let rows = row(status);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, "  Gatherer");
            assert!(
                rows[0].1.starts_with(&format!("{name}, load ")),
                "{status}: {}",
                rows[0].1
            );
            assert!(
                rows[0]
                    .1
                    .contains(&format!("target {}", describe_id(target))),
                "{}",
                rows[0].1
            );
        }
        // Harvesting keeps its names.
        assert_eq!(gather_state(100), "harvesting");
    }

    #[test]
    fn the_projectile_section_lists_flights_and_the_host_tracks() {
        use opennml_sim::combat::projectile::Projectile;
        use opennml_sim::message::Message;
        let (mut host, id) = host();
        assert_eq!(
            projectile_rows(&host.state, 0),
            ["Simulation: 0 projectiles", "Host: 0 flights"]
        );
        let p = |id_: u32, kind, flags| Projectile {
            id: id_,
            kind,
            flags,
            remaining_ms: 300,
            flight_ms: 900,
            shooter: id,
            target: ObjectId::NONE,
            damage: Message::new(0x17F, &[0; 12]),
            aim: FxVec3::default(),
            target_pos: [0.0; 3],
            start: [0.0; 3],
            zone: 0,
        };
        let list = vec![
            p(7, ProjectileKind::Arrow, 0x21),
            p(8, ProjectileKind::Arrow, 0x61),
            p(9, ProjectileKind::CannonBall, 0x1C),
            p(10, ProjectileKind::Dynamite, 4),
            p(11, ProjectileKind::Explosion, 0),
            p(12, ProjectileKind::Storm, 0),
            p(13, ProjectileKind::Throw, 0x201),
        ];
        host.state.projectiles.list = list;
        let before = opennml_sim::save::checksum(&host.state);
        let rows = projectile_rows(&host.state, 2);
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[0], "Simulation: 7 projectiles");
        assert!(
            rows[1].starts_with("7 Arrow: remaining 300/900 ms, shooter 0x"),
            "{}",
            rows[1]
        );
        assert!(rows[1].ends_with(", target -"), "{}", rows[1]);
        assert!(rows[2].starts_with("8 Fire arrow: "), "{}", rows[2]);
        assert!(rows[3].starts_with("9 Cannonball: "), "{}", rows[3]);
        // World effects.
        assert!(rows[4].starts_with("10 Dynamite: "), "{}", rows[4]);
        assert!(rows[5].starts_with("11 Explosion: "), "{}", rows[5]);
        assert!(rows[6].starts_with("12 Storm: "), "{}", rows[6]);
        assert!(rows[6].ends_with(", target -, pulse 0"), "{}", rows[6]);
        assert!(!rows[5].contains("pulse"), "{}", rows[5]);
        // Throw of the headhunter.
        assert!(rows[7].starts_with("13 Dynamite throw: "), "{}", rows[7]);
        assert_eq!(rows[8], "Host: 2 flights");
        assert_eq!(opennml_sim::save::checksum(&host.state), before);
    }

    #[test]
    fn the_inspector_shows_hit_points_stance_melee_and_dying_rows() {
        let (mut host, sabre, warrior) = opennml_client::game::sim_control::testkit::combat_host();
        host.submit_own(opennml_sim::command::CommandKind::Attack {
            units: vec![sabre],
            target: warrior,
            pos: None,
            mods: 0,
        });
        for _ in 0..40 {
            host.step_once();
        }
        let before = opennml_sim::save::checksum(&host.state);
        let rows = object_rows(&host, sabre);
        let get = |name: &str| {
            rows.iter()
                .find(|r| r.0 == name)
                .map(|r| r.1.clone())
                .unwrap_or_else(|| panic!("{name} is missing: {rows:?}"))
        };
        let c = host
            .combat_info(host.state.object(sabre, false).unwrap())
            .unwrap();
        assert_eq!(c.hp[1], 190);
        assert_eq!(
            get("Combat"),
            format!(
                "HP {}/190, rank {}, experience {} %",
                c.hp[0],
                c.rank,
                c.experience / 1000
            )
        );
        let attack = get("  Attack");
        assert!(
            attack.starts_with(
                "strike running, stance Defensive (2), aggressiveness retaliation (2), target 0x"
            ),
            "{attack}"
        );
        assert!(attack.ends_with(", commanded"), "{attack}");
        assert!(
            get("  Melee").starts_with("swing open, target 0x"),
            "{}",
            get("  Melee")
        );
        assert_eq!(get("  Dying"), "calm, sinking 0 ms");
        assert_eq!(opennml_sim::save::checksum(&host.state), before);
    }

    #[test]
    fn the_inspector_shows_construction_production_and_market_rows() {
        let types = "GUID 1001\nNAME House\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n";
        let mut rules = Rules::synthetic(types, 16);
        rules
            .defaults
            .insert(1001, [(13, 20), (14, 900)].into_iter().collect());
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let site =
            create_object(&mut sim, &mut ObjectDesc::new(1001, 0, [576.0, 0.0, 576.0])).unwrap();
        let rows = object_rows(&host, site);
        let get = |name: &str| rows.iter().find(|r| r.0 == name).unwrap().1.clone();
        assert!(
            get("  Construction").starts_with("construction site, remaining 2000 of 2000 (0 %)"),
            "{}",
            get("  Construction")
        );
        assert_eq!(get("  Production"), "-");
        assert!(
            get("  Docking slots").ends_with("free"),
            "{}",
            get("  Docking slots")
        );
        assert!(get("Rendering").starts_with("Render bits 0x4, progress 0.100"));
        let economy = economy_rows(&host.state);
        assert_eq!(economy[0], "Living space limit 125");
        assert_eq!(
            economy[1],
            "Market: Wood 0.250 Gold (1024), Food 0.330 Gold (1351)"
        );
        assert!(economy[2].starts_with("Resource factor %: 100 100"));
        assert_eq!(load_name(1), "Wood");
    }

    #[test]
    fn the_player_table_shows_occupied_slots_with_whole_resources() {
        let (host, _) = host();
        let rows = player_rows(&host.state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "2");
        assert_eq!(rows[0][3], "SPA");
        assert_eq!(rows[0][5], "30000");
        assert_eq!(rows[0].len(), PLAYER_COLUMNS.len());
        // Enemies and shared vision as a slot list (protocol 10).
        let n = rows[0].len();
        assert_eq!(
            (rows[0][n - 2].as_str(), rows[0][n - 1].as_str()),
            ("0,1,3,4,5,6,7", "2")
        );
    }

    #[test]
    fn the_fog_section_names_cycle_counts_and_view_switches() {
        let (mut host, _) = host();
        host.active_player = 2;
        host.state.fog.explored.or(1, 1, 1 << 2);
        host.fog.overlay = Some(3);
        let rows = fog_rows(&host);
        assert!(
            rows[2].starts_with("Player 2: explored 1, seen 0"),
            "{rows:?}"
        );
        assert_eq!(rows[3], "Fog off (O): no");
        assert_eq!(rows[4], "Overlay (F): seen");
        host.set_fog_shown(false);
        assert_eq!(fog_rows(&host)[3], "Fog off (O): yes");
    }

    #[test]
    fn the_panel_builds_without_a_window() {
        let (host, id) = host();
        let ctx = egui::Context::default();
        let view = DebugView {
            sim: &host,
            inspected: Some(id),
            selected: 1,
            drag: Some(([10.0, 10.0], [50.0, 40.0])),
            grid_cells: vec![(
                [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
                [220, 30, 30, 110],
            )],
            path_tiles: vec![[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]],
            path_line: vec![[0.0, 0.0], [30.0, 40.0]],
            path_rows: vec!["Layer 0 (Land 1x1): 0 blocked".into()],
            flights: 0,
        };
        let (mut open, mut match_open, mut script_open) = (true, false, false);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build(
                ui.ctx(),
                &mut open,
                &mut match_open,
                &mut script_open,
                &view,
            )
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
    }

    /// The texture delta is consumed (egui checks in the debug build on dropping that it is empty): first
    /// upload the font atlas and a loaded texture, after dropping the handle queue the release.
    #[test]
    fn apply_textures_consumes_delta() {
        let ctx = egui::Context::default();
        let mut handle = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Rank");
            handle = Some(ui.ctx().load_texture(
                "rank",
                egui::ColorImage::filled([4, 4], Color32::WHITE),
                egui::TextureOptions::NEAREST,
            ));
        });
        let id = handle.as_ref().unwrap().id();
        let mut uploaded = Vec::new();
        let mut free = Vec::new();
        apply_textures(output.textures_delta, |id, _| uploaded.push(id), &mut free);
        assert!(uploaded.contains(&egui::TextureId::default()));
        assert!(uploaded.contains(&id));
        assert!(free.is_empty());

        drop(handle);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Rank");
        });
        uploaded.clear();
        apply_textures(output.textures_delta, |id, _| uploaded.push(id), &mut free);
        assert!(uploaded.is_empty());
        assert_eq!(free, vec![id]);
    }

    #[test]
    fn group_info_leaves_the_checksum_unchanged() {
        use opennml_client::game::economy::testkit::{COLONIST, host, spawn};
        use opennml_sim::command::CommandKind;
        // Two colonists like `two_colonists` of the command bar tests (`opennml_client::game::unit_bar`).
        let mut host = host();
        let units = vec![
            spawn(&mut host, COLONIST, 0, (10, 10)),
            spawn(&mut host, COLONIST, 0, (12, 10)),
        ];
        host.submit_own(CommandKind::SetFormation {
            units: units.clone(),
            formation: 1,
        });
        for _ in 0..4 {
            host.step_once();
        }
        let before = host.checksum();
        let section = host.section(&units);
        let _ = host.query(Some(0), None, None, 100);
        assert_eq!(host.checksum(), before);
        assert!(section.groups.iter().any(|g| g.kind == 1));
        // "Groups" section of the debug window and the object rows "Group"/"Rank".
        let rows = group_rows(&host.state);
        assert_eq!(rows[0], format!("{} groups", section.groups.len()));
        // Colonists have rank 0: they walk in the side group (type 0), the formation stays empty.
        assert!(
            rows.iter().any(|r| r.contains("type 1: 0 members")),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("type 0: 2 members")),
            "{rows:?}"
        );
        let object = object_rows(&host, units[0]);
        assert!(object.iter().any(|(k, v)| k == "Group" && v != "none"));
        assert!(object.iter().any(|(k, _)| k == "Rank"));
        assert_eq!(host.checksum(), before);
        let walk = host.query(Some(0), None, Some(&[units[0].0]), 1);
        let walk = walk.objects[0].walk.as_ref().expect("walk module");
        assert!(walk.group.is_some());
    }
}
