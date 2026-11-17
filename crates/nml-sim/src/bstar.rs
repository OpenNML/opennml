//! B* local planner (`BStarPlanner`, per player slot, re/local-planner): greedy
//! best-first search in four axis directions with step size = speed, sorted by
//! collision cost + distance, with counting budgets (500 expansions per call, one call per player and
//! 100 ms), then smoothing via the movement model into a trajectory with one point per
//! object tick.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::collision::{UnitView, query_cost_at};
use crate::fx::{fixed_div, fixed_hypot};
use crate::id::ObjectId;
use crate::motion::step::{COST_BLOCKED, COST_COLLISION, COST_OUTSIDE, spread_turn_in_place};
use crate::motion::{SpeedInfo, TrajPoint, step};
use crate::sim::Sim;
use crate::world::wrap_angle;

/// Planners per player slot 0-7 and one for all other owners.
pub const PLANNER_COUNT: usize = 9;
/// Node storage (`BStarPlanner_Init(0x1900)`).
pub const NODE_CAPACITY: usize = 0x1900;
/// Expansions per call except in mode 2.
const EXPANSIONS_PER_CALL: i32 = 500;
/// Counter "owns the active search".
const OWNS_SEARCH: i32 = 0x3FFF_FFFF;
/// Directions of the four neighbors -x, +z, +x, -z.
const DIRECTIONS: [i32; 4] = [-180 << 12, 90 << 12, 0, -90 << 12];
/// Maximum number of steps of a segment simulation (exception A7: the original has no limit).
const MAX_SEGMENT_STEPS: usize = 10_000;

/// Planner of an owner (`GameWorld_GetBStarPlanner` `0x00475380`): slots 0-7, otherwise 8.
pub fn planner_index(owner: u8) -> usize {
    if owner < 8 { usize::from(owner) } else { 8 }
}

/// Request (`0x38` bytes, `PropertyWalk+0x240` or `BStarPlanner+0xA8`, re/local-planner 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub start: (i32, i32),
    pub start_dir: i32,
    pub goal: (i32, i32),
    /// 0 drive to point, 1 goal blocked, 2 out of a hard obstacle, 3 out of a soft blockage.
    pub mode: i32,
    /// Goal radius (-1 = default).
    pub radius: i32,
    /// Cost threshold "free" for mode 2/3.
    pub cost_limit: i32,
    pub barging: bool,
    /// Movement layer (index) of the cost query.
    pub layer: usize,
    pub unit: ObjectId,
    /// Speed multiplier.
    pub mult: i32,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            start: (0, 0),
            start_dir: 0,
            goal: (0, 0),
            mode: 0,
            radius: -1,
            cost_limit: 0,
            barging: false,
            layer: 0,
            unit: ObjectId::NONE,
            mult: 1,
        }
    }
}

impl Request {
    fn from_unit(sim: &Sim, unit: ObjectId) -> ((i32, i32), i32) {
        sim.object(unit)
            .map_or(((0, 0), 0), |o| ((o.grid.x.0, o.grid.z.0), o.direction.0))
    }

    /// `BStarRequest_InitGoTo` (`0x0054B510`): mode 0 (free) or 1 (goal blocked).
    pub fn go_to(
        sim: &Sim,
        unit: ObjectId,
        layer: usize,
        target: (i32, i32),
        blocked: bool,
        radius: i32,
        barging: bool,
    ) -> Self {
        let (start, start_dir) = Self::from_unit(sim, unit);
        Self {
            start,
            start_dir,
            goal: target,
            mode: i32::from(blocked),
            radius,
            cost_limit: COST_BLOCKED,
            barging,
            layer,
            unit,
            mult: 1,
        }
    }

    /// `BStarRequest_InitEscape` (`0x0054B470`): mode 2 (hard) or 3 (soft, factor 5).
    pub fn escape(sim: &Sim, unit: ObjectId, layer: usize, soft: bool) -> Self {
        let (start, start_dir) = Self::from_unit(sim, unit);
        Self {
            start,
            start_dir,
            goal: start,
            mode: 2 + i32::from(soft),
            radius: -1,
            cost_limit: 0,
            barging: false,
            layer,
            unit,
            mult: if soft { 5 } else { 1 },
        }
    }
}

/// Node (`0x24` bytes, re/local-planner 2.2); the index is the position in the node array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub x: i32,
    pub z: i32,
    pub dir: i32,
    /// Predecessor (-1 = root).
    pub parent: i32,
    /// Distance to the goal in steps (fixed point).
    pub h: i32,
    /// Collision cost at creation.
    pub cost: i32,
}

/// `BStarPlanner` (`0x200` bytes, re/local-planner 2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Planner {
    /// `+0x4C`: `FindPath`/`ContinuePath` calls in the current 100 ms window.
    pub calls: i32,
    /// `+0x50`: working speed (`cur` = step size).
    pub speed: SpeedInfo,
    /// `+0x7C`: speed of the unit (mode 2).
    pub saved: SpeedInfo,
    /// `+0xA8`.
    pub req: Request,
    /// `+0xE4`: expansions per call.
    pub max_expansions: i32,
    /// `+0xEC`: neighbor offsets.
    pub offsets: [(i32, i32); 4],
    /// `+0x134`: node array, node 0 = root.
    pub nodes: Vec<Node>,
    /// `+0x138`: best node (mode 0/1).
    pub best: usize,
    /// `+0x13C`: node expanded last.
    pub current: usize,
    /// `+0x140`: visited positions (never iterated).
    pub visited: BTreeMap<(i32, i32), u32>,
    /// `+0x15C`: OPEN, sorted (head first).
    pub open: VecDeque<u32>,
    /// `+0x178`: node chain, index 0 = end.
    pub chain: Vec<u32>,
    /// `+0x1C8`: last chain index.
    pub last: i32,
    /// `+0x1CC`: node budget.
    pub budget: i32,
    /// `+0x1D0`: smoothing is pending.
    pub smoothing: bool,
    /// `+0x1D4`: status (1, 3-7, 0).
    pub status: i32,
    /// `+0x1D8`: result trajectory.
    pub traj: VecDeque<TrajPoint>,
    /// `+0x1F4`: use the best node.
    pub use_best: bool,
    /// `+0x1F8`, `+0x1FC`: smoothing from/to.
    pub to: i32,
    pub from: i32,
    /// `+0x18C`: attempt counter per unit.
    pub attempts: BTreeMap<u32, i32>,
    /// `+0x1A8`: has made a request in the window.
    pub requested: BTreeSet<u32>,
    /// `+0x1C4`: maximum of the counters of the previous window.
    pub prev_max: i32,
}

impl Planner {
    /// Indices and divisors within range (savegame check): a corrupted planner must neither
    /// crash nor hang.
    ///
    /// - Nodes: predecessors always lie **before** the node (so the chain never becomes a cycle), all
    ///   references point to nodes.
    /// - A unit with `OWNS_SEARCH` continues the search: it needs nodes (root) and a
    ///   step size > 0 (divisor of `add_node`) while a search is running.
    /// - Request: multiplier >= 1 (divisor of the smoothing).
    /// - Rotation of the working speed `1 ≤ turn ≤ 180°` (divisor of the turning radius).
    /// - While the smoothing runs, `0 ≤ to < from < chain length` holds (or `from < 1`: finished).
    pub fn is_consistent(&self) -> bool {
        let n = self.nodes.len();
        let ok = |i: u32| (i as usize) < n;
        let turn_ok = |s: &SpeedInfo| (1..=crate::motion::speed::MAX_TURN).contains(&s.turn);
        let owns = self.attempts.values().any(|&c| c >= OWNS_SEARCH);
        self.open.iter().all(|&i| ok(i))
            && self.chain.iter().all(|&i| ok(i))
            && self
                .nodes
                .iter()
                .enumerate()
                .all(|(i, o)| o.parent >= -1 && o.parent < i as i32)
            && (n == 0 || (self.best < n && self.current < n))
            && (!owns || (n > 0 && (self.smoothing || self.speed.cur > 0)))
            && self.req.mult >= 1
            && turn_ok(&self.speed)
            && (!self.smoothing
                || (self.to >= 0
                    && self.from < self.chain.len() as i32
                    && (self.from < 1 || self.to < self.from)))
    }

    /// `BStarPlanner_ResetTickBudget` (`0x00408600`): counters of the units without a request in the
    /// window are dropped, remember the maximum of the others.
    pub fn reset_tick_budget(&mut self) {
        self.calls = 0;
        self.prev_max = 0;
        let requested = &self.requested;
        let mut max = 0;
        self.attempts.retain(|unit, c| {
            let keep = requested.contains(unit);
            if keep {
                max = max.max(*c);
            }
            keep
        });
        self.prev_max = max;
        self.requested.clear();
    }

    /// `BStarPlanner_TryAcquirePlanSlot` (`0x00409020`): counter +1 (also on rejection); allowed
    /// if it exceeds the previous-window maximum and the planner has not yet run in the window.
    pub fn try_acquire(&mut self, unit: ObjectId) -> bool {
        let c = self
            .attempts
            .get(&unit.0)
            .copied()
            .unwrap_or(0)
            .wrapping_add(1);
        self.requested.insert(unit.0);
        self.attempts.insert(unit.0, c);
        c > self.prev_max && self.calls < 1
    }

    /// `BStarPlanner_ForgetUnitAttempts` (`0x00408390`).
    pub fn forget_unit(&mut self, unit: ObjectId) {
        self.attempts.remove(&unit.0);
    }

    fn step_size(&self) -> i32 {
        self.speed.cur
    }

    /// `Fixed_Div(d, step size)`: remaining cost of a node and budget of the search.
    ///
    /// Deviation (K-327): with a step size of 0 the result is 0. The original divides by zero there and crashes
    /// (`BStarPlanner_SetupSearch` `0x00409600`, `BStarPlanner_AddNode` `0x00409AA0`); all other inputs compute
    /// bit-identically. The search then ends by itself: all neighbors lie on the start point, which was already
    /// visited.
    fn per_step(&self, d: i32) -> i32 {
        match self.step_size() {
            0 => 0,
            s => fixed_div(d, s),
        }
    }

    fn dist(a: (i32, i32), b: (i32, i32)) -> i32 {
        fixed_hypot(a.0.wrapping_sub(b.0), a.1.wrapping_sub(b.1))
    }

    /// `BStarPlanner_InsertOpenSorted` (`0x00409090`): before the first with key >= new (LIFO).
    fn insert_open(&mut self, idx: u32) {
        let mode = self.req.mode;
        let key = |n: &Node| {
            if mode == 3 {
                n.cost
            } else {
                n.cost.wrapping_add(n.h)
            }
        };
        let k = key(&self.nodes[idx as usize]);
        match self
            .open
            .iter()
            .position(|&e| key(&self.nodes[e as usize]) >= k)
        {
            Some(p) => self.open.insert(p, idx),
            None => self.open.push_back(idx),
        }
    }

    /// `BStarPlanner_CheckGoalReached` (`0x00408020`) for the expanded node.
    fn check_goal(&mut self) -> i32 {
        let n = self.nodes[self.current];
        if self.req.mode == 2 || self.req.mode == 3 {
            return if n.cost <= self.req.cost_limit { 4 } else { 1 };
        }
        self.budget = self.budget.wrapping_sub(1);
        if self.budget < 1 {
            return 5;
        }
        let d = Self::dist((n.x, n.z), self.req.goal);
        if d > self.req.radius {
            return 1;
        }
        if self.req.mult > 1 && d > self.req.radius / self.req.mult {
            return 6;
        }
        3
    }
}

fn unit_of(sim: &Sim, p: &Planner) -> Option<ObjectId> {
    sim.object(p.req.unit).map(|o| o.id)
}

fn node_cost(sim: &mut Sim, p: &Planner, x: i32, z: i32) -> i32 {
    let unit = unit_of(sim, p);
    query_cost_at(sim, x, z, unit, p.req.layer, false, p.req.barging)
}

/// Take the planner out of the state, work on it, put it back (the cost queries need the world).
fn with_planner<R>(sim: &mut Sim, slot: usize, f: impl FnOnce(&mut Sim, &mut Planner) -> R) -> R {
    let mut p = std::mem::take(&mut sim.state.planners[slot]);
    let r = f(sim, &mut p);
    sim.state.planners[slot] = p;
    r
}

/// `BStarPlanner_SetupSearch` (`0x00409600`).
fn setup_search(sim: &mut Sim, p: &mut Planner, unit_speed: &SpeedInfo, req: &Request) {
    p.attempts.insert(req.unit.0, OWNS_SEARCH);
    let sp = &mut p.speed;
    if req.mode == 2 {
        sp.set_default_speed(0x1000);
        sp.set_cruising_speed(0x1000);
        sp.set_cur_speed(0x1000);
        sp.set_turn_speed(180 << 12);
        sp.min = 0;
        sp.instant_turn = unit_speed.instant_turn;
        p.saved = *unit_speed;
        p.max_expansions = i32::MAX;
    } else {
        let m = req.mult;
        sp.set_default_speed(m.wrapping_mul(unit_speed.default));
        sp.set_cruising_speed(m.wrapping_mul(unit_speed.cruising));
        sp.set_cur_speed(m.wrapping_mul(unit_speed.cur));
        sp.set_turn_speed(m.wrapping_mul(unit_speed.turn));
        // Set only after the setters (quirk: their limit check saw the `min` of the previous search).
        sp.min = if unit_speed.min != 0 { 10 } else { 0 };
        sp.instant_turn = unit_speed.instant_turn;
        p.max_expansions = EXPANSIONS_PER_CALL;
    }
    p.req = *req;
    p.req.start_dir = wrap_angle(req.start_dir);
    let s = p.step_size();
    p.offsets = [(-s, 0), (0, s), (s, 0), (0, -s)];
    p.open.clear();
    p.visited.clear();
    if p.req.radius <= s / 2 {
        p.req.radius = s.wrapping_mul(2) / 3;
    }
    let d = Planner::dist(p.req.start, p.req.goal);
    p.budget = if p.req.mode == 1 {
        (p.per_step(d) >> 12).wrapping_mul(3).wrapping_add(0x17)
    } else {
        i32::MAX
    };
    let cost = node_cost(sim, p, p.req.start.0, p.req.start.1);
    p.nodes.clear();
    p.nodes.push(Node {
        x: p.req.start.0,
        z: p.req.start.1,
        dir: p.req.start_dir,
        parent: -1,
        h: p.per_step(d),
        cost,
    });
    p.open.push_back(0);
    p.visited.insert(p.req.start, 0);
    p.smoothing = false;
    p.best = 0;
    p.current = 0;
}

/// `BStarPlanner_AddNode` (`0x00409AA0`): visited positions stay untouched; even rejected
/// neighbors occupy a node and count as visited.
fn add_node(sim: &mut Sim, p: &mut Planner, pos: (i32, i32), dir: i32, parent: usize) {
    if p.visited.contains_key(&pos) {
        return;
    }
    let h = p.per_step(Planner::dist(pos, p.req.goal));
    let cost = node_cost(sim, p, pos.0, pos.1);
    let idx = p.nodes.len() as u32;
    p.nodes.push(Node {
        x: pos.0,
        z: pos.1,
        dir,
        parent: parent as i32,
        h,
        cost,
    });
    match p.req.mode {
        2 => {
            if cost < COST_OUTSIDE {
                p.insert_open(idx);
            }
        }
        3 => {
            if cost < COST_BLOCKED {
                p.insert_open(idx);
            }
        }
        _ => {
            if cost < COST_COLLISION || (p.req.barging && cost < COST_BLOCKED) {
                p.insert_open(idx);
                if h.wrapping_add(10) < p.nodes[p.best].h {
                    p.best = idx as usize;
                }
            }
        }
    }
    p.visited.insert(pos, idx);
}

/// `BStarPlanner_RunExpansions` (`0x0040A180`): 1 = continue, 2 = search finished (smoothing pending),
/// 3-7 only for a chain of one node.
fn run_expansions(sim: &mut Sim, p: &mut Planner) -> i32 {
    p.status = 1;
    let mut i = 0;
    while i < p.max_expansions {
        if p.status != 1 {
            break;
        }
        match p.open.pop_front() {
            None => {
                p.status = 7;
                p.use_best = true;
            }
            Some(n) => {
                p.current = n as usize;
                p.status = p.check_goal();
                if p.status == 1 {
                    if p.nodes.len() < NODE_CAPACITY - 4 {
                        let node = p.nodes[p.current];
                        for (offset, dir) in p.offsets.into_iter().zip(DIRECTIONS) {
                            let pos =
                                (node.x.wrapping_add(offset.0), node.z.wrapping_add(offset.1));
                            add_node(sim, p, pos, dir, p.current);
                        }
                    } else {
                        p.status = 6;
                        p.use_best = true;
                    }
                } else {
                    p.use_best = p.req.mode == 1;
                }
            }
        }
        i += 1;
    }
    if matches!(p.status, 3..=7) && !build_node_chain(sim, p, p.use_best) {
        p.smoothing = true;
        return 2;
    }
    p.status
}

/// `BStarPlanner_BuildNodeChain` (`0x00409FA0`); `true` = finished (chain of one node).
fn build_node_chain(sim: &mut Sim, p: &mut Planner, use_best: bool) -> bool {
    p.last = -1;
    p.traj.clear();
    p.chain.clear();
    if p.req.mode == 2 {
        if p.nodes[p.current].parent != -1 {
            p.chain.push(p.current as u32);
        }
        p.chain.push(0);
        let u = p.saved;
        let m = p.req.mult;
        let sp = &mut p.speed;
        sp.set_default_speed(u.default.wrapping_mul(m));
        sp.set_cruising_speed(u.cruising.wrapping_mul(m));
        sp.set_cur_speed(u.cur.wrapping_mul(m));
        sp.set_turn_speed(u.turn.wrapping_mul(m));
        sp.min = if u.min != 0 { 10 } else { 0 };
    } else {
        if use_best {
            p.current = p.best;
            if p.status == 7 && p.best != 0 {
                p.status = 6;
            }
        }
        let mut n = p.current;
        while p.nodes[n].parent != -1 {
            p.chain.push(n as u32);
            n = p.nodes[n].parent as usize;
        }
        p.chain.push(n as u32);
    }
    p.last = p.chain.len() as i32 - 1;
    smooth_path(sim, p, true)
}

/// `BStarPlanner_SmoothPath` (`0x00409CA0`); `init` = call from `BuildNodeChain`.
fn smooth_path(sim: &mut Sim, p: &mut Planner, init: bool) -> bool {
    if init {
        p.from = p.last;
        p.to = 0;
        if p.last < 1 {
            return true;
        }
        if p.req.mode == 3 {
            p.req.barging = true;
        }
        let m = p.req.mult;
        if m != 1 {
            let sp = &mut p.speed;
            let cruising = sp.cruising / m;
            sp.set_default_speed(sp.default / m);
            sp.set_cruising_speed(cruising);
            sp.set_turn_speed(sp.turn / m);
            sp.set_cur_speed(sp.cur / m);
        }
        return false;
    }
    let done = smooth_step(p, &mut |p, from, to| simulate_segment(sim, p, from, to));
    if done && !p.traj.is_empty() && p.speed.instant_turn {
        let turn = p.speed.turn;
        for i in 1..p.traj.len() {
            spread_turn_in_place(&mut p.traj, Some(i), turn);
        }
    }
    done
}

/// Index logic of the smoothing (re/local-planner 5.2): from the start member first the chain end, then
/// every tenth member, then the single members. The limit of 100 attempts per call is checked only in the
/// ten-step loop; the attempts of the single loop count too.
fn smooth_step(
    p: &mut Planner,
    simulate: &mut dyn FnMut(&mut Planner, usize, usize) -> bool,
) -> bool {
    let mut tries = -1;
    loop {
        if p.from < 1 {
            return true;
        }
        if p.to < p.from {
            loop {
                tries += 1;
                if tries >= 100 {
                    return false;
                }
                let (f, t) = (p.chain[p.from as usize], p.chain[p.to as usize]);
                if simulate(p, f as usize, t as usize) {
                    break;
                }
                p.to += 10;
                if p.to >= p.from {
                    break;
                }
            }
        }
        if p.to < p.from {
            cut(p);
        } else {
            p.to -= 9;
            let mut ok = false;
            while p.to < p.from {
                let (f, t) = (p.chain[p.from as usize], p.chain[p.to as usize]);
                ok = simulate(p, f as usize, t as usize);
                if ok {
                    break;
                }
                p.to += 1;
                // counted, but not checked here (INC EBP at 0x00409ECC): counts for later coarse loops
                tries += 1;
            }
            if !ok {
                if p.status == 3 {
                    p.status = 6;
                }
                return true;
            }
            cut(p);
        }
        p.to = 0;
    }
}

/// Remove the intermediate members `to + 1 … from − 1`; the next section begins at the member reached.
fn cut(p: &mut Planner) {
    p.from -= 1;
    while p.to < p.from {
        p.chain.remove(p.from as usize);
        p.from -= 1;
        p.last = p.last.wrapping_sub(1);
    }
}

/// `BStarPlanner_SimulateSegment` (`0x00409100`): movement steps until arrival; only an
/// arrived segment is appended (the duplicate start point only for the first).
fn simulate_segment(sim: &mut Sim, p: &mut Planner, from: usize, to: usize) -> bool {
    let f = p.nodes[from];
    let target = (p.nodes[to].x, p.nodes[to].z);
    let mut st = TrajPoint {
        x: f.x,
        z: f.z,
        dir: f.dir,
        mode: 0,
        sense: 0,
        inside: false,
        speed: p.speed.cur,
    };
    let check = if p.req.mode == 3 {
        1
    } else if p.req.barging {
        2
    } else {
        0
    };
    let mut seg = Vec::new();
    let mut view = UnitView {
        unit: unit_of(sim, p),
        layer: p.req.layer,
        sim,
    };
    let mut r = 0;
    for _ in 0..MAX_SEGMENT_STEPS {
        if p.req.mode == 2 && !st.inside {
            st.inside = true;
        }
        seg.push(st);
        r = step(&mut st, target, &mut p.speed, check, &mut view);
        if r != 0 {
            break;
        }
    }
    if r == 0 {
        log::warn!("B*: segment did not arrive after {MAX_SEGMENT_STEPS} steps, discarded");
    }
    if r != 2 {
        return false;
    }
    seg.push(st);
    p.nodes[to].dir = st.dir;
    if !p.traj.is_empty() {
        seg.remove(0);
    }
    p.traj.extend(seg);
    true
}

/// `BStarPlanner_FindPath` (`0x0040A3B0`): if the unit is missing, the counter of the **previous**
/// request is deleted (quirk).
pub fn find_path(sim: &mut Sim, slot: usize, unit_speed: &SpeedInfo, req: &Request) -> i32 {
    with_planner(sim, slot, |sim, p| {
        if sim.object(req.unit).is_none() {
            p.attempts.remove(&p.req.unit.0);
            return 0;
        }
        setup_search(sim, p, unit_speed, req);
        p.calls = p.calls.wrapping_add(1);
        run_expansions(sim, p)
    })
}

/// `BStarPlanner_ContinuePath` (`0x0040A430`): only for the unit that owns the search.
pub fn continue_path(sim: &mut Sim, slot: usize, unit: ObjectId) -> i32 {
    with_planner(sim, slot, |sim, p| {
        if sim.object(p.req.unit).is_none() {
            p.attempts.remove(&p.req.unit.0);
            return 0;
        }
        let owner =
            p.req.unit == unit && p.attempts.get(&unit.0).is_some_and(|&c| c >= OWNS_SEARCH);
        if !owner {
            return 0;
        }
        p.requested.insert(unit.0);
        p.calls = p.calls.wrapping_add(1);
        if !p.smoothing {
            run_expansions(sim, p)
        } else if smooth_path(sim, p, false) {
            p.status
        } else {
            2
        }
    })
}

/// `BStarPlanner_EndIncompletePath` (`0x0040A360`): path to the best node (without counting).
pub fn end_incomplete_path(sim: &mut Sim, slot: usize) -> i32 {
    with_planner(sim, slot, |sim, p| {
        if p.nodes.is_empty() {
            return 0;
        }
        p.status = 6;
        if build_node_chain(sim, p, true) {
            if p.last == 0 {
                p.status = 0;
            }
            return p.status;
        }
        p.smoothing = true;
        2
    })
}

/// `BStarPlanner_FetchResultPath` (`0x00408D90`): trajectory without the start point, last point with
/// end marker 1; `false` (only the start point as the end) if there is no path.
pub fn fetch_result_path(sim: &mut Sim, slot: usize, out: &mut VecDeque<TrajPoint>) -> bool {
    let p = &mut sim.state.planners[slot];
    p.requested.remove(&p.req.unit.0);
    p.smoothing = false;
    out.clear();
    if p.traj.len() > 1 {
        out.extend(p.traj.iter().skip(1).copied());
        if let Some(last) = out.back_mut() {
            last.mode = 1;
        }
        true
    } else {
        out.push_front(TrajPoint {
            x: p.req.start.0,
            z: p.req.start.1,
            dir: p.req.start_dir,
            mode: 1,
            sense: 0,
            inside: false,
            speed: p.speed.cur,
        });
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    const X0: i32 = 0x8800;

    fn setup(blocked: &[(i32, i32)]) -> (Rules, SimState, ObjectId) {
        let rules = Rules::synthetic("GUID 1100\nNAME K\nVALUEID 100 6 1 0\nPROPID 2\n", 128);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        for &(x, z) in blocked {
            state.path.mark_cell(1, x, z, true);
        }
        let unit = {
            let mut sim = Sim::new(&mut state, &rules);
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [100.0, 0.0, 100.0])).unwrap()
        };
        (rules, state, unit)
    }

    fn unit_speed(step: i32) -> SpeedInfo {
        SpeedInfo {
            default: step,
            cruising: step,
            cur: step,
            turn: 45 << 12,
            spin: 90 << 12,
            ..SpeedInfo::default()
        }
    }

    fn request(unit: ObjectId, goal: (i32, i32), mode: i32) -> Request {
        Request {
            start: (X0, X0),
            goal,
            mode,
            cost_limit: if mode >= 2 { 0 } else { COST_BLOCKED },
            unit,
            ..Request::default()
        }
    }

    fn chain_points(p: &Planner) -> Vec<(i32, i32)> {
        p.chain
            .iter()
            .rev()
            .map(|&i| (p.nodes[i as usize].x, p.nodes[i as usize].z))
            .collect()
    }

    /// Search without smoothing (result 2 = smoothing pending), cost = cell of the midpoint.
    fn search(
        blocked: &[(i32, i32)],
        req: impl Fn(ObjectId) -> Request,
        step: i32,
    ) -> (i32, Planner) {
        let (rules, mut state, unit) = setup(blocked);
        let mut sim = Sim::new(&mut state, &rules);
        let r = find_path(&mut sim, 0, &unit_speed(step), &req(unit));
        (r, sim.state.planners[0].clone())
    }

    #[test]
    fn searches_match_the_vectors_of_the_disassembly_model() {
        // V1 free straight: 11 expansions, 32 nodes, chain along +x.
        let (r, p) = search(&[], |u| request(u, (X0 + 10 * 0x400, X0), 0), 0x400);
        assert_eq!(
            (r, p.status, p.nodes.len(), p.open.len(), p.best),
            (2, 3, 32, 21, 30)
        );
        assert_eq!(
            chain_points(&p),
            (0..=10).map(|i| (X0 + i * 0x400, X0)).collect::<Vec<_>>()
        );
        assert_eq!(p.req.radius, 0x2AA);
        // V2 diagonal: alternately first +x, then +z (LIFO).
        let (r, p) = search(&[], |u| request(u, (X0 + 0x1400, X0 + 0x1400), 0), 0x400);
        assert_eq!((r, p.status, p.nodes.len(), p.open.len()), (2, 3, 24, 13));
        let c = chain_points(&p);
        assert_eq!(
            c[1..4],
            [(0x8C00, 0x8800), (0x8C00, 0x8C00), (0x9000, 0x8C00)]
        );
        // V3 obstacle block x = 10, z = 7-9.
        let wall: Vec<_> = (7..10).map(|z| (10, z)).collect();
        let (r, p) = search(&wall, |u| request(u, (0xC800, X0), 0), 0x400);
        assert_eq!(
            (r, p.status, p.nodes.len(), p.open.len(), p.chain.len()),
            (2, 3, 91, 37, 29)
        );
        // V4 mode 1: budget 71 exhausted, ends at the best node.
        let block: Vec<_> = (11..14)
            .flat_map(|x| (5..13).map(move |z| (x, z)))
            .collect();
        let (r, p) = search(&block, |u| request(u, (0xC800, X0), 1), 0x400);
        assert_eq!(
            (r, p.status, p.nodes.len(), p.open.len(), p.best),
            (2, 5, 117, 28, 27)
        );
        assert_eq!(*chain_points(&p).last().unwrap(), (0xAC00, X0));
        // V6 mode 2: step of one tile, chain [start, found point], speed back afterwards.
        let block: Vec<_> = (7..10).flat_map(|x| (7..10).map(move |z| (x, z))).collect();
        let (r, p) = search(&block, |u| request(u, (X0, X0), 2), 0x400);
        assert_eq!((r, p.status, p.nodes.len(), p.open.len()), (2, 4, 8, 5));
        assert_eq!(chain_points(&p), [(X0, X0), (X0, 0x6800)]);
        assert_eq!((p.offsets[2].0, p.speed.cur), (0x1000, 0x400));
        // V7 enclosed: OPEN empty, 7 -> 6 via the best node.
        let ring: Vec<_> = (6..11)
            .flat_map(|x| (6..11).map(move |z| (x, z)))
            .filter(|&(x, z)| !(7..10).contains(&x) || !(7..10).contains(&z))
            .collect();
        let (r, p) = search(&ring, |u| request(u, (0x10800, X0), 0), 0x400);
        assert_eq!(
            (r, p.status, p.nodes.len(), p.open.len(), p.best),
            (2, 6, 192, 0, 15)
        );
        // V9 start = goal: immediately 3, chain of one node.
        let (r, p) = search(&[], |u| request(u, (X0, X0), 0), 0x400);
        assert_eq!((r, p.chain.len()), (3, 1));
    }

    #[test]
    fn long_searches_continue_and_end_incomplete() {
        // V5: 500 expansions per call, continuation only for the owner.
        let (rules, mut state, unit) = setup(&[]);
        let mut sim = Sim::new(&mut state, &rules);
        let r = find_path(
            &mut sim,
            0,
            &unit_speed(0x100),
            &request(unit, (X0 + 600 * 0x100, X0), 0),
        );
        let p = &sim.state.planners[0];
        assert_eq!((r, p.nodes.len(), p.open.len()), (1, 1502, 1002));
        assert_eq!(continue_path(&mut sim, 0, ObjectId(77)), 0);
        let r = continue_path(&mut sim, 0, unit);
        let p = &sim.state.planners[0];
        assert_eq!(
            (r, p.status, p.nodes.len(), p.open.len(), p.best),
            (2, 3, 1802, 1201, 1800)
        );
        // V10: after two calls ends at the best node.
        let (rules, mut state, unit) = setup(&[]);
        let mut sim = Sim::new(&mut state, &rules);
        let req = request(unit, (X0 + 1200 * 0x100, X0), 0);
        assert_eq!(find_path(&mut sim, 0, &unit_speed(0x100), &req), 1);
        assert_eq!(continue_path(&mut sim, 0, unit), 1);
        assert_eq!(end_incomplete_path(&mut sim, 0), 2);
        let p = &sim.state.planners[0];
        assert_eq!((p.status, p.nodes.len()), (6, 3002));
        assert_eq!(*chain_points(p).last().unwrap(), (0x47000, X0));
    }

    #[test]
    fn smoothing_tries_far_links_first_in_steps_of_ten() {
        // V8: chain of 35 members, a segment succeeds at a distance of at most 7 members.
        let mut p = Planner {
            chain: (0..35).collect(),
            status: 3,
            from: 34,
            to: 0,
            last: 34,
            ..Planner::default()
        };
        let mut tried = Vec::new();
        let done = smooth_step(&mut p, &mut |_, a, b| {
            tried.push((a, b));
            a - b <= 7
        });
        assert!(done);
        assert_eq!(tried.len(), 25);
        assert_eq!(
            &tried[..5],
            [(34, 0), (34, 10), (34, 20), (34, 30), (30, 0)]
        );
        assert_eq!(p.chain, [0, 3, 10, 13, 20, 23, 30, 34]);
    }

    #[test]
    fn the_plan_slot_goes_to_the_longest_waiting_unit_once_per_window() {
        let mut p = Planner::default();
        let (a, b) = (ObjectId(0x10000), ObjectId(0x20000));
        assert!(p.try_acquire(a));
        p.calls = 1;
        assert!(!p.try_acquire(b));
        p.reset_tick_budget();
        // a counts 1, b 1: maximum 1 -> in the next window it needs 2.
        assert_eq!(p.prev_max, 1);
        assert!(p.try_acquire(b));
        // a no longer makes requests and loses its counter at the next sync.
        p.reset_tick_budget();
        assert!(!p.attempts.contains_key(&a.0) && p.prev_max == 2);
        assert_eq!(planner_index(3), 3);
        assert_eq!(planner_index(8), 8);
        assert_eq!(planner_index(10), 8);
    }

    #[test]
    fn a_full_search_with_smoothing_yields_one_point_per_tick() {
        // Free path: result 2, then the smoothing in the next call; trajectory without start.
        let (rules, mut state, unit) = setup(&[]);
        let mut sim = Sim::new(&mut state, &rules);
        let req = request(unit, (X0 + 0x2000, X0), 0);
        assert_eq!(find_path(&mut sim, 0, &unit_speed(0x400), &req), 2);
        assert_eq!(continue_path(&mut sim, 0, unit), 3);
        let mut out = VecDeque::new();
        assert!(fetch_result_path(&mut sim, 0, &mut out));
        assert_eq!(out.len(), 8);
        assert_eq!(
            (out[0].x, out[7].x, out[7].mode),
            (X0 + 0x400, X0 + 0x2000, 1)
        );
        assert!(!sim.state.planners[0].requested.contains(&unit.0));
    }

    #[test]
    fn fine_attempts_count_toward_the_limit_of_later_coarse_loops() {
        // Only the direct neighbor succeeds. Per round 20 coarse attempts and 9/8/7 single calls; the
        // counted failures of the single loop (8+7+6) bring the fourth coarse loop to the
        // 100th attempt, so that it aborts after 19 calls.
        let mut p = Planner {
            chain: (0..=200).collect(),
            status: 0,
            from: 200,
            to: 0,
            last: 200,
            ..Planner::default()
        };
        let mut calls = 0;
        let done = smooth_step(&mut p, &mut |_, f, t| {
            calls += 1;
            t + 1 == f
        });
        assert!(!done);
        assert_eq!(p.from, 197);
        assert_eq!(calls, 103);
    }
}
