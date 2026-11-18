//! Tests of the engine per slot (passes, tick, pauses, synchronous run, start) and of the flow builtins.

use super::builtins::{self, arith, flow, has_case, lists, memory};
use super::catalog::{KiCatalog, KiPrototype};
use super::engine::{Engine, KiCtx, SLICE_DT, SYNC_DT};
use super::testkit::{self, TestWorld, int, word};
use super::vm::{Thread, op, status};
use super::world::{KiLimit, KiNote};
use super::{KiPause, KiState, MAX_SYNC_STEPS, MAX_THREADS};
use crate::rng::CrtRand;

/// Builtin of the test world (goes to `TestWorld`, logs the thread).
const MARK: u32 = 600;

fn callb(id: u32) -> u32 {
    word(op::CALLB, id)
}

/// Module with the code `code` and the integer variables `0..8` with the initial values `inits`.
fn module(id: u32, inits: &[(u32, i32)], code: &[u32]) -> KiPrototype {
    testkit::proto(id, &[(0, 8)], inits, code)
}

/// Test rig: catalog, randomness, test world and an engine on slot 0.
struct Bench {
    catalog: KiCatalog,
    rand: CrtRand,
    world: TestWorld,
    engine: Engine,
}

impl Bench {
    fn new(protos: Vec<KiPrototype>) -> Self {
        Self {
            catalog: testkit::catalog(protos),
            rand: CrtRand::new(1),
            world: TestWorld::default(),
            engine: Engine::new(0),
        }
    }

    fn spawn(&mut self, module: u32, args: &[i32]) -> Option<i32> {
        let mut cx = KiCtx::new(&self.catalog, &mut self.rand, &mut self.world);
        self.engine.spawn(&mut cx, module, args)
    }

    /// Enters thread `id` of module `module` by hand.
    fn insert(&mut self, id: i32, module: u32) {
        let t = self.thread(id, module);
        self.engine.threads.insert(id, t);
    }

    fn thread(&self, id: i32, module: u32) -> Thread {
        let p = self.catalog.get(module).expect("test module missing");
        Thread::from_prototype(p, id, 0, Vec::new())
    }

    fn slice(&mut self) -> bool {
        let mut cx = KiCtx::new(&self.catalog, &mut self.rand, &mut self.world);
        self.engine.run_slice(&mut cx, SLICE_DT)
    }

    fn tick(&mut self) -> bool {
        let mut cx = KiCtx::new(&self.catalog, &mut self.rand, &mut self.world);
        self.engine.tick(&mut cx)
    }

    fn run_sync(&mut self, module: u32, args: &[i32]) -> Option<i32> {
        let mut cx = KiCtx::new(&self.catalog, &mut self.rand, &mut self.world);
        self.engine.run_sync(&mut cx, module, args).map(|r| r.id)
    }

    fn run_sync_pause(&mut self, module: u32, args: &[i32]) -> Option<bool> {
        let mut cx = KiCtx::new(&self.catalog, &mut self.rand, &mut self.world);
        self.engine.run_sync(&mut cx, module, args).map(|r| r.pause)
    }

    /// Threads of the logged calls of `MARK`, cleared afterwards.
    fn take_marks(&mut self) -> Vec<i32> {
        let marks = self
            .world
            .calls
            .iter()
            .filter(|c| u32::from(c.2) == MARK)
            .map(|c| c.1)
            .collect();
        self.world.calls.clear();
        marks
    }

    fn get(&self, id: i32) -> &Thread {
        self.engine.threads.get(id).expect("thread missing")
    }

    fn keys(&self) -> Vec<i32> {
        self.engine.threads.iter().map(|(k, _)| k).collect()
    }
}

// QUIRK E-9: kept
#[test]
fn twenty_threads_run_in_cmap_order() {
    let code = [callb(MARK), callb(MARK), callb(MARK), word(op::JMP, 0)];
    let mut b = Bench::new(vec![module(1, &[], &code)]);
    for id in 0..20 {
        assert_eq!(b.spawn(1, &[]), Some(id));
    }
    assert_eq!(b.engine.threads.bucket_count(), 17);
    b.slice();
    // Bucket 0: IDs 15...0 (newest first), bucket 1: IDs 19...16; three steps per thread.
    let order: Vec<i32> = (0..16).rev().chain((16..20).rev()).collect();
    let expected: Vec<i32> = order.iter().flat_map(|&id| [id; 3]).collect();
    assert_eq!(b.take_marks(), expected);
}

// QUIRK E-9: kept
#[test]
fn a_spawn_runs_in_the_same_slice_only_behind_the_position() {
    let marks = [callb(MARK); 6];
    let mut starter = vec![word(op::SPAWN, 3)];
    starter.extend_from_slice(&marks);
    let protos = || {
        vec![
            module(1, &[], &starter),
            module(2, &[], &marks),
            module(3, &[], &marks),
        ]
    };

    // New thread 16 (bucket 1) lies between thread 0 (bucket 0) and the already determined
    // position 32 (bucket 2): it does not run until the next pass.
    let mut b = Bench::new(protos());
    b.insert(0, 1);
    b.insert(32, 2);
    b.engine.next_thread_id = 16;
    b.slice();
    assert_eq!(b.take_marks(), [0, 0, 32, 32, 32]);
    assert_eq!(b.keys(), [0, 16, 32]);
    b.slice();
    assert_eq!(b.take_marks(), [0, 0, 0, 16, 16, 16, 32, 32, 32]);

    // New thread 48 (bucket 3) lies behind the position: it runs in the same pass after 32.
    let mut b = Bench::new(protos());
    b.insert(0, 1);
    b.insert(32, 2);
    b.engine.next_thread_id = 48;
    b.slice();
    assert_eq!(b.take_marks(), [0, 0, 32, 32, 32, 48, 48, 48]);
    assert_eq!(b.engine.next_thread_id, 49);
}

// QUIRK E-9: kept
#[test]
fn a_spawn_into_the_bucket_of_the_position_waits() {
    // Thread 33 goes to the front of the bucket of position 32 - before it - and waits.
    let marks = [callb(MARK); 6];
    let mut starter = vec![word(op::SPAWN, 3)];
    starter.extend_from_slice(&marks);
    let mut b = Bench::new(vec![
        module(1, &[], &starter),
        module(2, &[], &marks),
        module(3, &[], &marks),
    ]);
    b.insert(0, 1);
    b.insert(32, 2);
    b.engine.next_thread_id = 33;
    b.slice();
    assert_eq!(b.take_marks(), [0, 0, 32, 32, 32]);
    assert_eq!(b.keys(), [0, 33, 32]);
}

// QUIRK E-8: kept
#[test]
fn one_tick_runs_three_slices_of_three_steps() {
    let code = [word(op::PUSHA, 1); 12];
    let mut b = Bench::new(vec![module(1, &[], &code)]);
    b.spawn(1, &[]);
    b.slice();
    assert_eq!(b.get(0).pc, 3);
    assert!(!b.tick());
    assert_eq!(b.get(0).pc, 12);

    let mut b = Bench::new(vec![module(1, &[], &code)]);
    b.spawn(1, &[]);
    b.tick();
    assert_eq!(b.get(0).pc, 9);
}

// QUIRK E-8: kept
#[test]
fn pause_of_1000_ms_ends_after_167_slots() {
    // c1000 = int(0). After the call of MARK, `PUSHA 1` runs in the 9th step slot, then `JMP 3`, so
    // that the thread stays for reading.
    let code = [
        word(op::LOAD, int(0)),
        callb(0x8E),
        callb(MARK),
        word(op::PUSHA, 1),
        word(op::JMP, 3),
    ];
    let mut b = Bench::new(vec![module(1, &[(int(0), 1000)], &code)]);
    b.spawn(1, &[]);

    // Tick 1: step slot 1 LOAD, slot 2 sets the pause, slots 3-9 count down.
    b.tick();
    assert_eq!(b.get(0).pause_ms, 1000 - 7 * 6);
    assert_eq!(b.get(0).pause_ms, 958);
    for _ in 2..=18 {
        b.tick();
    }
    assert_eq!(b.get(0).pause_ms, 40);
    assert!(b.take_marks().is_empty());

    // Tick 19: step slots 1-7 count from 40 to -2 (167 in total), slot 8 calls MARK, slot 9
    // executes `PUSHA 1`.
    b.tick();
    let t = b.get(0);
    assert_eq!(t.pause_ms, -2);
    assert_eq!((t.pc, t.stack.as_slice()), (4, &[0, 1][..]));
    assert_eq!(b.take_marks(), [0]);
}

// QUIRK E-12: kept
#[test]
fn sync_run_burns_pauses_without_game_time() {
    // Pause 1000 -> 50 steps of 20 ms each, then the rest; the instance never enters the table.
    let code = [word(op::LOAD, int(0)), callb(0x8E), callb(MARK)];
    let mut b = Bench::new(vec![module(1, &[(int(0), 1000)], &code)]);
    b.engine.next_thread_id = 5;
    assert_eq!(b.run_sync(1, &[]), Some(5));
    assert_eq!(b.engine.next_thread_id, 6, "the ID is consumed");
    assert!(b.engine.threads.is_empty());
    assert_eq!(b.take_marks(), [5]);
    assert!(b.world.notes.is_empty());

    // Step count at the limit: 2 commands + k pause steps + MARK + end.
    let k = MAX_SYNC_STEPS as i32 - 4;
    for (pause, limited) in [(SYNC_DT * k, false), (SYNC_DT * k + 1, true)] {
        let mut b = Bench::new(vec![module(1, &[(int(0), pause)], &code)]);
        assert_eq!(b.run_sync(1, &[]), Some(0));
        assert_eq!(b.take_marks(), [0], "pause {pause}");
        let expected = if limited {
            vec![KiNote::Limit {
                slot: 0,
                thread: 0,
                module: 1,
                limit: KiLimit::SyncSteps,
            }]
        } else {
            Vec::new()
        };
        assert_eq!(b.world.notes, expected, "pause {pause}");
    }

    // A thread started by the synchronous module is in the table normally; the arguments
    // are on the stack in order.
    let mut b = Bench::new(vec![
        module(1, &[], &[word(op::SPAWN, 2)]),
        module(2, &[], &[callb(MARK)]),
    ]);
    assert_eq!(b.run_sync(1, &[7, 8]), Some(0));
    assert_eq!(b.keys(), [1]);
    let t = b.get(1);
    assert_eq!(
        (t.player, t.module, t.stack.as_slice()),
        (0, 2, &[7, 8][..])
    );
    assert!(
        b.take_marks().is_empty(),
        "the new thread does not run until the tick"
    );
}

// QUIRK E-12: kept
#[test]
fn sync_run_reports_the_ki_pause() {
    // `0x18C` in the synchronous run stops nothing in the run; the run reports it to the caller.
    let mut b = Bench::new(vec![
        module(1, &[], &[callb(0x18C), callb(MARK)]),
        module(2, &[], &[callb(MARK)]),
    ]);
    assert_eq!(b.run_sync_pause(1, &[]), Some(true));
    assert_eq!(b.take_marks(), [0], "the run continues after 0x18C");
    assert_eq!(b.run_sync_pause(2, &[]), Some(false));
    assert_eq!(b.run_sync_pause(99, &[]), None);
}

// QUIRK E-12: kept
#[test]
fn sync_run_stops_at_the_step_limit() {
    let mut b = Bench::new(vec![module(1, &[], &[word(op::JMP, 0)])]);
    b.engine.next_thread_id = 3;
    assert_eq!(b.run_sync(1, &[]), Some(3));
    assert_eq!(
        b.world.notes,
        [KiNote::Limit {
            slot: 0,
            thread: 3,
            module: 1,
            limit: KiLimit::SyncSteps,
        }]
    );
    assert!(b.engine.threads.is_empty());
    assert_eq!(b.engine.next_thread_id, 4);
}

// QUIRK E-6: deviation
#[test]
fn sync_run_and_spawn_of_a_missing_module_start_nothing() {
    let mut b = Bench::new(vec![module(1, &[], &[])]);
    assert_eq!(b.run_sync(99, &[]), None);
    assert_eq!(b.spawn(99, &[]), None);
    assert_eq!(b.engine.next_thread_id, 0);
    assert!(b.engine.threads.is_empty());
    assert_eq!(
        b.world.notes,
        [
            KiNote::MissingModule {
                slot: 0,
                thread: -1,
                module: 99,
                target: 99,
                spawn: false,
            },
            KiNote::MissingModule {
                slot: 0,
                thread: -1,
                module: 99,
                target: 99,
                spawn: true,
            },
        ]
    );
}

#[test]
fn spawn_pushes_the_arguments_and_respects_the_thread_limit() {
    let mut b = Bench::new(vec![module(1, &[], &[callb(MARK)])]);
    b.engine.next_thread_id = 40;
    assert_eq!(b.spawn(1, &[1, 2, 3]), Some(40));
    let t = b.get(40);
    assert_eq!(
        (t.player, t.status, t.stack.as_slice()),
        (0, status::RUNNING, &[1, 2, 3][..])
    );

    for _ in 1..MAX_THREADS {
        assert!(b.spawn(1, &[]).is_some());
    }
    assert_eq!(b.engine.threads.len(), MAX_THREADS);
    let next = b.engine.next_thread_id;
    assert_eq!(b.spawn(1, &[]), None);
    assert_eq!(b.engine.next_thread_id, next, "no ID consumed");
    assert_eq!(
        b.world.notes,
        [KiNote::Limit {
            slot: 0,
            thread: -1,
            module: 1,
            limit: KiLimit::Threads,
        }]
    );
}

/// Enters thread `id` with the child `child` (and its child `grandchild`) into the table.
fn insert_with_children(b: &mut Bench, id: i32, module: u32, child: u32, grandchild: Option<u32>) {
    let mut t = b.thread(id, module);
    let mut c = b.thread(id, child);
    if let Some(g) = grandchild {
        c.child = Some(Box::new(b.thread(id, g)));
        c.status |= status::CHILD;
    }
    t.child = Some(Box::new(c));
    t.status |= status::CHILD;
    b.engine.threads.insert(id, t);
}

// QUIRK E-11, E-44: kept
#[test]
fn stop_builtins_follow_e11_and_p9() {
    let idle = [word(op::PUSHA, 1); 9];
    let stop = [callb(0x19), word(op::PUSHA, 1), word(op::PUSHA, 2)];
    let protos = || {
        vec![
            module(1, &[], &idle),
            module(2, &[], &stop),
            module(
                3,
                &[],
                &[callb(0x1A), word(op::PUSHA, 1), word(op::PUSHA, 2)],
            ),
        ]
    };
    let stops = |b: &Bench| -> Vec<(i32, bool)> {
        b.engine.threads.iter().map(|(k, t)| (k, t.stop)).collect()
    };

    // Depth 0: only other threads with a child; those without a child and the own thread stay free.
    let mut b = Bench::new(protos());
    b.insert(0, 2);
    insert_with_children(&mut b, 1, 1, 1, None);
    b.insert(2, 1);
    b.slice();
    assert_eq!(stops(&b), [(2, false), (1, true), (0, false)]);
    assert_eq!(b.get(0).pc, 3, "the own thread keeps running");

    // Depth 1: the child does not stop the own thread.
    let mut b = Bench::new(protos());
    insert_with_children(&mut b, 0, 1, 2, None);
    insert_with_children(&mut b, 1, 1, 1, None);
    b.slice();
    assert_eq!(stops(&b), [(1, true), (0, false)]);

    // Depth 2 (grandchild, E-44): the own thread stops too; its remaining steps in the step slot
    // are dropped.
    let mut b = Bench::new(protos());
    insert_with_children(&mut b, 0, 1, 1, Some(2));
    insert_with_children(&mut b, 1, 1, 1, None);
    b.slice();
    assert_eq!(stops(&b), [(1, true), (0, true)]);
    assert_eq!(grandchild_pc(&b, 0), Some(1));

    // A stopped thread does not count down its pause and executes nothing.
    b.engine.threads.get_mut(1).expect("Thread 1").pause_ms = 100;
    b.slice();
    assert_eq!(b.get(1).pause_ms, 100);
    assert_eq!(grandchild_pc(&b, 0), Some(1));

    // `0x1A` from the free thread 6 releases all, including the stopped thread 5 without a child.
    b.insert(5, 3);
    b.engine.threads.get_mut(5).expect("Thread 5").stop = true;
    b.insert(6, 3);
    b.slice();
    assert_eq!(stops(&b), [(6, false), (5, false), (1, false), (0, false)]);
}

fn grandchild_pc(b: &Bench, id: i32) -> Option<u32> {
    b.get(id)
        .child
        .as_deref()
        .and_then(|c| c.child.as_deref())
        .map(|g| g.pc)
}

// QUIRK E-44: kept
#[test]
fn stop_in_a_sync_run_hits_every_thread_with_a_child() {
    let idle = [word(op::PUSHA, 1); 9];
    let mut b = Bench::new(vec![module(1, &[], &idle), module(2, &[], &[callb(0x19)])]);
    insert_with_children(&mut b, 0, 1, 1, None);
    b.insert(1, 1);
    b.engine.next_thread_id = 2;
    assert_eq!(b.run_sync(2, &[]), Some(2));
    assert!(b.get(0).stop);
    assert!(!b.get(1).stop);
}

// QUIRK E-42: kept
#[test]
fn semaphore_wait_takes_and_continues_in_the_same_slot() {
    let code = [word(op::PUSHA, 1); 6];
    let mut b = Bench::new(vec![module(1, &[], &code)]);
    b.spawn(1, &[]);
    b.engine
        .threads
        .get_mut(0)
        .expect("Thread 0")
        .wait_semaphore = 5;
    b.engine.mem.semaphores.insert(5, 1);
    b.slice();
    assert_eq!((b.get(0).pc, b.get(0).wait_semaphore), (0, 5));

    // re/ki-vm 12.3 P3: takeover and command in the same step slot -> three commands in the pass.
    b.engine.mem.semaphores.insert(5, 0);
    b.slice();
    assert_eq!((b.get(0).pc, b.get(0).wait_semaphore), (3, -1));
    assert_eq!(b.engine.mem.semaphores.get(&5), Some(&1));
}

// QUIRK E-42: kept
#[test]
fn semaphore_builtins_set_clear_and_read() {
    // k = int(0) = 4, j = int(1) = 9.
    let code = [
        word(op::LOAD, int(0)),
        callb(0x31), // missing -> 0, nothing created
        word(op::LOAD, int(1)),
        callb(0x30), // missing -> nothing
        word(op::LOAD, int(0)),
        callb(0x2F), // take
        word(op::LOAD, int(0)),
        callb(0x31), // 1
        word(op::LOAD, int(0)),
        callb(0x30), // release
        word(op::LOAD, int(0)),
        callb(0x31), // 0
    ];
    let mut b = Bench::new(vec![module(1, &[(int(0), 4), (int(1), 9)], &code)]);
    b.spawn(1, &[]);
    b.slice();
    assert_eq!(b.get(0).stack, [0, 9]);
    assert!(b.engine.mem.semaphores.is_empty());
    b.slice();
    assert_eq!(b.get(0).stack, [0]);
    assert_eq!(
        b.engine.mem.semaphores.iter().collect::<Vec<_>>(),
        [(&4, &1)]
    );
    b.slice();
    assert_eq!(b.get(0).stack, [0, 1, 4]);
    b.slice();
    assert_eq!(b.get(0).stack, [0, 1, 0]);
    assert_eq!(
        b.engine.mem.semaphores.iter().collect::<Vec<_>>(),
        [(&4, &0)]
    );
}

#[test]
fn builtin_0x18c_pauses_the_ki_after_the_tick() {
    let catalog = testkit::catalog(vec![
        module(1, &[], &[callb(0x18C), word(op::JMP, 1)]),
        module(2, &[], &[callb(MARK), word(op::JMP, 0)]),
    ]);
    let mut world = TestWorld::default();
    let mut ki = KiState::new(1);
    assert_eq!(ki.paused, KiPause::Running);
    assert!(ki.engines.iter().all(Option::is_none));
    for (slot, module) in [(0, 1), (1, 2)] {
        let p = catalog.get(module).expect("test module");
        let engine = ki.engine_mut(slot);
        assert_eq!(engine.slot, slot);
        engine
            .threads
            .insert(0, Thread::from_prototype(p, 0, slot, Vec::new()));
    }
    ki.tick_engines(&catalog, &mut world);
    // Slot 1 still runs fully in the same tick: 9 steps, of which 5 times MARK.
    let marks: Vec<(u8, i32)> = world
        .calls
        .iter()
        .filter(|c| u32::from(c.2) == MARK)
        .map(|c| (c.0, c.1))
        .collect();
    assert_eq!(marks, [(1, 0); 5]);
    assert_eq!(ki.paused, KiPause::Paused);
    assert_eq!(
        ki.engines[0]
            .as_ref()
            .map(|e| e.threads.get(0).map(|t| t.pc)),
        Some(Some(1))
    );

    let before = ki.clone();
    world.calls.clear();
    ki.tick_engines(&catalog, &mut world);
    assert!(world.calls.is_empty());
    assert_eq!(ki, before);
}

#[test]
fn finished_threads_leave_the_table_in_the_same_slot() {
    let mut b = Bench::new(vec![
        module(1, &[], &[word(op::PUSHA, 1), word(op::PUSHA, 2)]),
        module(2, &[], &[word(op::PUSHA, 1); 3]),
    ]);
    b.spawn(1, &[]);
    b.spawn(2, &[]);
    b.slice();
    // Thread 0: two commands, in the third step slot end of code -> removed immediately (re/ki-vm 12.3 P4).
    assert_eq!(b.keys(), [1]);
    assert_eq!((b.get(1).pc, b.get(1).status), (3, status::RUNNING));
    b.slice();
    assert!(b.engine.threads.is_empty());
}

#[test]
fn local_builtins_come_from_one_source() {
    for id in 0..0x400 {
        let a = arith::handles(id);
        let f = flow::handles(id);
        let l = lists::handles(id);
        let m = memory::handles(id);
        assert!(
            u8::from(a) + u8::from(f) + u8::from(l) + u8::from(m) <= 1,
            "Builtin {id:#x} duplicated"
        );
        assert_eq!(builtins::is_local(id), a || f || l || m, "Builtin {id:#x}");
        if a || f || l || m {
            assert!(has_case(id), "Builtin {id:#x} without case");
        }
    }
    for id in [0x19, 0x1A, 0x2F, 0x30, 0x31, 0x8E, 0x18C] {
        assert!(flow::handles(id), "Builtin {id:#x}");
    }
    // 78 memory IDs (catalog A 2, B 3, C 3), each exactly once; 0x147 is a world query
    // (request 0x511D) in the middle of the range of the position lists.
    assert_eq!((0..0x400).filter(|&id| memory::handles(id)).count(), 78);
    assert!(!builtins::is_local(0x147));
    // List objects 24, randomness 1 (flow 7 + 1), VM builtins 7 (arithmetic 23 + 7).
    assert_eq!((0..0x400).filter(|&id| lists::handles(id)).count(), 24);
    assert_eq!((0..0x400).filter(|&id| flow::handles(id)).count(), 8);
    assert_eq!((0..0x400).filter(|&id| arith::handles(id)).count(), 30);
    assert_eq!((0..0x400).filter(|&id| builtins::is_local(id)).count(), 140);
}
