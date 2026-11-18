//! Pair tracking of the collisions (`CollisionRecords`, `GameWorld+0x3A4`, re/collision
//! A3.2 and B2): once per 100 ms tick before the object ticks, check all pairs of neighboring buckets of the
//! movers, create or move hard/soft records and report them to both units
//! (message `0x800`); the walk modules then decide (`opennml_sim::module::walk::collide`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::message::Message;
use crate::motion::step::COST_COLLISION;
use crate::sim::Sim;

use super::{CollisionGrid, pair_overlap_cost};

/// Key of a record: smaller and larger ID (compared signed).
pub type PairKey = (i32, i32);

/// Record (`0x18` bytes, re/collision B2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// 0 new, 1 being resolved, 2 done/delete, 3 ignore, 4 watch.
    pub state: i32,
    pub hard: bool,
    /// 0 undetermined, 1 MoveMove, 2 MoveStand, 3 StandStand.
    pub kind: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionRecords {
    /// `+0x20`: hard records in list order.
    pub hard: Vec<PairKey>,
    /// `+0x04`: soft records.
    pub soft: Vec<PairKey>,
    /// `+0x3C`: lookup by key (never iterated).
    pub records: BTreeMap<PairKey, Record>,
}

impl CollisionRecords {
    /// Savegame check: no key appears twice in the lists (hard union soft), and each
    /// has a record. Otherwise `tick` would access a missing or already removed record
    /// (`records[&key]`).
    pub fn is_consistent(&self) -> bool {
        let listed = self.hard.len() + self.soft.len();
        let distinct: BTreeSet<PairKey> = self.hard.iter().chain(&self.soft).copied().collect();
        distinct.len() == listed
            && self
                .hard
                .iter()
                .chain(&self.soft)
                .all(|k| self.records.contains_key(k))
    }
}

/// Key of a pair (`JG`, signed).
pub fn pair_key(a: ObjectId, b: ObjectId) -> PairKey {
    let (a, b) = (a.as_i32(), b.as_i32());
    if a > b { (b, a) } else { (a, b) }
}

/// Order of the pair checks of `CollisionMgr_DetectPairCollisions` (`0x004B7F30`): per bucket
/// of the movers (row outer) the pairs in the bucket, then with the right, the lower and the
/// lower right neighbor, finally right against lower (counter-diagonal).
pub fn pair_order(grid: &CollisionGrid) -> Vec<(ObjectId, ObjectId)> {
    let (rows, cols) = (grid.rows, grid.cols);
    let list = |r: i32, c: i32| &grid.movers[(r * cols + c) as usize];
    let mut pairs = Vec::new();
    for r in 0..rows {
        let mut cur = Some(list(r, 0));
        let mut below = (r + 1 < rows).then(|| list(r + 1, 0));
        for c in 1..=cols {
            let Some(cur_list) = cur else { break };
            let below_cur = below;
            let right = (c < cols).then(|| list(r, c));
            below = (c < cols && r + 1 < rows).then(|| list(r + 1, c));
            if !cur_list.is_empty() || right.is_some_and(|l| !l.is_empty()) {
                for (i, &n1) in cur_list.iter().enumerate() {
                    pairs.extend(cur_list[i + 1..].iter().map(|&n2| (n1, n2)));
                    if let Some(right) = right {
                        pairs.extend(right.iter().map(|&n| (n1, n)));
                    }
                    if let Some(bc) = below_cur {
                        pairs.extend(bc.iter().map(|&n| (n1, n)));
                        if let Some(b) = below {
                            pairs.extend(b.iter().map(|&n| (n1, n)));
                        }
                    }
                }
                if let (Some(bc), Some(right)) = (below_cur, right) {
                    for &m in right {
                        pairs.extend(bc.iter().map(|&n| (m, n)));
                    }
                }
            }
            cur = right;
        }
    }
    pairs
}

/// `CollisionMgr_UpdatePairRecord` (`0x004B7D50`): new records at the back, switch hard<->soft
/// at the front (state 0); a soft one in state 2 goes to 3 (quirk: then oscillates 2/3).
fn update(sim: &mut Sim, a: ObjectId, b: ObjectId) {
    let cost = pair_overlap_cost(sim, a, b, None, None, false, false);
    let key = pair_key(a, b);
    let recs = &mut sim.state.collision_records;
    if cost >= COST_COLLISION {
        match recs.records.get_mut(&key) {
            None => {
                recs.records.insert(
                    key,
                    Record {
                        state: 0,
                        hard: true,
                        kind: 0,
                    },
                );
                recs.hard.push(key);
            }
            Some(r) if !r.hard => {
                r.hard = true;
                if let Some(p) = recs.soft.iter().position(|&k| k == key) {
                    recs.soft.remove(p);
                    recs.hard.insert(0, key);
                    r.state = 0;
                }
            }
            Some(_) => {}
        }
    } else if cost > 0 {
        match recs.records.get_mut(&key) {
            None => {
                recs.records.insert(
                    key,
                    Record {
                        state: 0,
                        hard: false,
                        kind: 0,
                    },
                );
                recs.soft.push(key);
            }
            Some(r) if r.hard => {
                r.hard = false;
                if let Some(p) = recs.hard.iter().position(|&k| k == key) {
                    recs.hard.remove(p);
                    recs.soft.insert(0, key);
                    r.state = 0;
                }
            }
            Some(r) => {
                if r.state == 2 {
                    r.state = 3;
                }
            }
        }
    }
}

/// `GameObject_PostCollisionRecord` (`0x00466740`): synchronously to the walk module (`0x800`).
fn post(sim: &mut Sim, to: ObjectId, key: PairKey, hard: bool) {
    if sim.module(to, 0).is_some() {
        crate::module::walk::query(
            sim,
            to,
            &mut Message::new(0x800, &[key.0, key.1, i32::from(hard)]),
        );
    }
}

/// `CollisionRecords_Tick` (`0x0040EC50`, phase 0 before the object ticks): detect, then delete finished
/// records and report the rest to both units (the larger ID first).
pub fn tick(sim: &mut Sim) {
    for (a, b) in pair_order(&sim.state.collision) {
        update(sim, a, b);
    }
    for hard in [true, false] {
        let mut i = 0;
        loop {
            let recs = &sim.state.collision_records;
            let list = if hard { &recs.hard } else { &recs.soft };
            let Some(&key) = list.get(i) else { break };
            let r = recs.records[&key];
            if r.state == 2 {
                let recs = &mut sim.state.collision_records;
                if hard {
                    recs.hard.remove(i);
                } else {
                    recs.soft.remove(i);
                }
                recs.records.remove(&key);
                continue;
            }
            i += 1;
            let (hi, lo) = (ObjectId::from_i32(key.1), ObjectId::from_i32(key.0));
            let (Some(oh), Some(ol)) = (sim.object(hi), sim.object(lo)) else {
                set_state(sim, key, 2);
                continue;
            };
            if !hard {
                let mut state = r.state;
                if (oh.status | ol.status) & 0x800 != 0 {
                    state = 0;
                }
                let d = fixed_hypot(
                    oh.grid.x.0.wrapping_sub(ol.grid.x.0),
                    oh.grid.z.0.wrapping_sub(ol.grid.z.0),
                );
                let apart = d > oh.soft_radius.wrapping_add(ol.soft_radius);
                if (state == 4 && apart) || state == 3 {
                    set_state(sim, key, 2);
                    continue;
                }
                set_state(sim, key, state);
            }
            post(sim, hi, key, hard);
            post(sim, lo, key, hard);
        }
    }
}

fn set_state(sim: &mut Sim, key: PairKey, state: i32) {
    if let Some(r) = sim.state.collision_records.records.get_mut(&key) {
        r.state = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::pair_overlap_cost;

    #[test]
    fn pairs_are_checked_bucket_by_bucket_in_the_original_order() {
        // re/collision A6 T11: 2 x 2 buckets, (row, column) (0,0) = [40, 42], (0,1) = [41, 45],
        // (1,0) = [44], (1,1) = [43].
        let mut g = CollisionGrid::new(16, 16);
        let ids = |v: &[u32]| v.iter().map(|&i| ObjectId(i)).collect::<Vec<_>>();
        g.movers[0] = ids(&[40, 42]);
        g.movers[1] = ids(&[41, 45]);
        g.movers[2] = ids(&[44]);
        g.movers[3] = ids(&[43]);
        let got: Vec<(u32, u32)> = pair_order(&g)
            .into_iter()
            .map(|(a, b)| (a.0, b.0))
            .collect();
        assert_eq!(
            got,
            [
                (40, 42),
                (40, 41),
                (40, 45),
                (40, 44),
                (40, 43),
                (42, 41),
                (42, 45),
                (42, 44),
                (42, 43),
                (41, 44),
                (45, 44),
                (41, 45),
                (41, 43),
                (45, 43),
                (44, 43)
            ]
        );
        assert_eq!(pair_key(ObjectId(0x8000_0000), ObjectId(5)), (i32::MIN, 5));
    }

    /// Two colonists (hard radius 512, soft 1024): pair costs `d = 0x700` soft,
    /// `0x300` hard (re/collision B10 T0).
    const TYPES: &str = "GUID 1100
NAME SPA_Kolonist
VALUEID 100 6 1 0
PROPID 0
VALUEID 10003 6 1 264448
VALUEID 10004 6 1 512
VALUEID 10005 6 1 1024
VALUEID 10006 6 1 200
VALUEID 10007 6 1 1
PROPID 2
VALUEID 30001 1 1 0
";

    #[test]
    fn records_move_between_the_lists_when_the_overlap_changes_kind() {
        use crate::rules::Rules;
        use crate::state::SimState;
        use crate::world::{ObjectDesc, create_object};
        let rules = Rules::synthetic(TYPES, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let spawn = |sim: &mut Sim, x: i32| {
            let pos = [x as f32 / 32.0, 0.0, 0x2000 as f32 / 32.0];
            create_object(sim, &mut ObjectDesc::new(1100, 0, pos)).unwrap()
        };
        let (a, b, c) = (
            spawn(&mut sim, 0x2000),
            spawn(&mut sim, 0x2700),
            spawn(&mut sim, 0x6000),
        );
        let at = |sim: &mut Sim, id: ObjectId, x: i32| {
            let o = sim.object_mut(id).unwrap();
            o.grid.x = crate::fx::Fx(x);
        };
        let (ka, kc) = (pair_key(a, b), pair_key(a, c));

        // Soft overlap (d = 0x700, cost 0x200): new soft record at the back.
        assert!(pair_overlap_cost(&mut sim, a, b, None, None, false, false) > 0);
        update(&mut sim, a, b);
        let recs = &sim.state.collision_records;
        assert_eq!((recs.hard.len(), recs.soft.clone()), (0, vec![ka]));
        assert_eq!(
            recs.records[&ka],
            Record {
                state: 0,
                hard: false,
                kind: 0
            }
        );
        assert!(recs.is_consistent());

        // A second soft overlap arrives at the back; no distance: no record.
        at(&mut sim, c, 0x2000 + 0x600);
        update(&mut sim, a, c);
        assert_eq!(sim.state.collision_records.soft, [ka, kc]);
        at(&mut sim, c, 0x6000);
        update(&mut sim, a, c);
        assert_eq!(
            sim.state.collision_records.soft,
            [ka, kc],
            "cost 0 changes nothing"
        );

        // Soft -> hard (d = 0x300): at the front of the hard list, state reset to 0.
        sim.state
            .collision_records
            .records
            .get_mut(&ka)
            .unwrap()
            .state = 4;
        at(&mut sim, b, 0x2300);
        update(&mut sim, a, b);
        let recs = &sim.state.collision_records;
        assert_eq!((recs.hard.clone(), recs.soft.clone()), (vec![ka], vec![kc]));
        assert_eq!(
            recs.records[&ka],
            Record {
                state: 0,
                hard: true,
                kind: 0
            }
        );

        // Hard stays hard: the state of the record stays.
        sim.state
            .collision_records
            .records
            .get_mut(&ka)
            .unwrap()
            .state = 1;
        update(&mut sim, a, b);
        assert_eq!(sim.state.collision_records.records[&ka].state, 1);

        // Hard -> soft (d = 0x700): at the front of the soft list (before the older one), state 0.
        at(&mut sim, b, 0x2700);
        update(&mut sim, a, b);
        let recs = &sim.state.collision_records;
        assert_eq!((recs.hard.len(), recs.soft.clone()), (0, vec![ka, kc]));
        assert_eq!(
            recs.records[&ka],
            Record {
                state: 0,
                hard: false,
                kind: 0
            }
        );

        // A soft record in state 2 (done) goes to 3 on a renewed overlap (oscillates),
        // other states stay.
        let set = |sim: &mut Sim, state| {
            sim.state
                .collision_records
                .records
                .get_mut(&ka)
                .unwrap()
                .state = state;
        };
        for (before, after) in [(2, 3), (3, 3), (0, 0), (1, 1), (4, 4)] {
            set(&mut sim, before);
            update(&mut sim, a, b);
            assert_eq!(
                sim.state.collision_records.records[&ka].state, after,
                "{before}"
            );
        }
        assert!(sim.state.collision_records.is_consistent());
    }
}
