//! `MenuFrame` (vtable `0x0061A700`, re/menu-system 5.3, 8.4.3, 12.2, 12.5): query "frame opened", transition
//! (`FUN_004617F0`) with fly-in or fly-out and the movement step in `Tick`.

use nml_formats::dlg::FrameDesc;
use nml_formats::dlg::desc::frame_flags;

use super::{due, idiv, lock, query_head};
use crate::bkg_paint::load_bkg;
use crate::tree::{Broadcast, NodeId, NodeKind, UiEnv, UiTree};

/// Duration of the crossfade without `MOVE` (`FUN_004617F0`, `local_8 = 500`).
const FADE_MS: u32 = 500;
/// Bar offset after closing (`0x00460C89`: `+0x134 = -500`).
const CLOSED_BAR_Y: i32 = -500;

/// Transition kind (`+0x12C`): `0x10000` open, `0x20000` close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMode {
    Open,
    Close,
}

/// State of a `MenuFrame` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct FrameState {
    pub desc: FrameDesc,
    /// `+0x10C/+0x110`: start point of the movement (top-left corner in the grid).
    pub start: [i32; 2],
    /// `+0x11C/+0x120`: target point.
    pub target: [i32; 2],
    /// `+0x100`: remaining time in ms.
    pub rem: i32,
    /// `+0x104`: duration in ms (`DURATION`).
    pub duration: i32,
    /// `+0x12C`: most recently begun transition (`None` = none yet).
    pub mode: Option<FrameMode>,
    /// `+0xFC` bit 0: movement running.
    pub moving: bool,
    /// `+0x130/+0x134`: offset of bars, title and ornaments (8.4.3), horizontal only; after closing
    /// `(0, -500)`.
    pub bar: [i32; 2],
    /// `+0x94`: time of the last `Tick` (ms of the program clock).
    pub last: u64,
    /// `+0x128`: next query time; `None` = never again. The constructor sets 1 (due immediately).
    pub query_at: Option<u64>,
}

/// Result of a movement step: new top-left corner and whether the movement ended (`0x136`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStep {
    pub pos: [i32; 2],
    pub done: bool,
}

impl FrameState {
    /// Constructor `FUN_00461AD0`: no movement, offset `(0, 0)`, query due.
    pub fn new(desc: FrameDesc) -> FrameState {
        FrameState {
            desc,
            start: [0; 2],
            target: [0; 2],
            rem: 0,
            duration: 0,
            mode: None,
            moving: false,
            bar: [0; 2],
            last: 0,
            query_at: Some(0),
        }
    }

    /// `FUN_004604F0(sx, sy, ex, ey, duration)`: start, target, time, remaining time = duration, movement on.
    pub fn begin(&mut self, start: [i32; 2], target: [i32; 2], duration: i32, now: u64) {
        self.start = start;
        self.target = target;
        self.last = now;
        self.duration = duration;
        self.rem = duration;
        self.moving = true;
    }

    /// `Tick` `FUN_004615A0`: `dt = now - last` (32 bit like `GetTickCount`), if the movement runs, one step
    /// [`Self::step`]; afterwards `last = now`.
    pub fn tick(&mut self, now: u64) -> Option<FrameStep> {
        let dt = now.wrapping_sub(self.last) as u32 as i32;
        let step = self.moving.then(|| self.step(dt));
        self.last = now;
        step
    }

    /// Movement step `FUN_00460B40(dt)` (disassembly `0x00460B40`...`0x00460D12`, 12.2):
    ///
    /// ```text
    /// rem -= dt
    /// rem > 0:  x = ex + rem*(sx-ex)/dur,  y = ey + rem*(sy-ey)/dur          (IDIV)
    ///           half = dur/2
    ///           bar = rem > half and opening  ->  ((rem-half)*(sx-ex)/half, 0)
    ///                 rem < half and closing  ->  (-((half-rem)*(sx-ex)/half), 0)
    ///                 otherwise (0, 0)
    /// rem <= 0: x = ex, y = ey, bar = (0, 0), movement off; closing: bar = (0, -500); message 0x136
    /// ```
    ///
    /// Deviation: `half = 0` (duration 1 with `dt = 0`) divides by 0 in the original; here the division yields 0.
    pub fn step(&mut self, dt: i32) -> FrameStep {
        self.rem = self.rem.wrapping_sub(dt);
        let [sx, sy] = self.start;
        let [ex, ey] = self.target;
        if self.rem < 1 {
            self.moving = false;
            self.bar = if self.mode == Some(FrameMode::Close) {
                [0, CLOSED_BAR_Y]
            } else {
                [0, 0]
            };
            return FrameStep {
                pos: [ex, ey],
                done: true,
            };
        }
        let (rem, dur) = (self.rem, self.duration);
        let x = ex.wrapping_add(idiv(rem.wrapping_mul(sx.wrapping_sub(ex)), dur));
        let y = ey.wrapping_add(idiv(rem.wrapping_mul(sy.wrapping_sub(ey)), dur));
        let half = dur / 2;
        let dx = sx.wrapping_sub(ex);
        self.bar = if rem > half && self.mode == Some(FrameMode::Open) {
            [idiv(rem.wrapping_sub(half).wrapping_mul(dx), half), 0]
        } else if rem < half && self.mode == Some(FrameMode::Close) {
            [
                idiv(half.wrapping_sub(rem).wrapping_mul(dx), half).wrapping_neg(),
                0,
            ]
        } else {
            [0, 0]
        };
        FrameStep {
            pos: [x, y],
            done: false,
        }
    }
}

pub(crate) fn state(tree: &UiTree, id: NodeId) -> &FrameState {
    match &tree.node(id).kind {
        NodeKind::Frame(s) => s,
        _ => panic!("widget {id:?} is not a Frame"),
    }
}

pub(crate) fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut FrameState {
    match &mut tree.node_mut(id).kind {
        NodeKind::Frame(s) => s,
        _ => panic!("widget {id:?} is not a Frame"),
    }
}

/// Query "frame opened" `FUN_00461140` (from `OnAttach` `0x00461C9C` and at the start of `Draw`
/// `FUN_00461520`, 12.5): only with a due timer (`+0x128`, constructor 1 -> exactly one query per construction, unless
/// the logic supplies a `timeout`); codes 9 and 5 with the frame name as dialog **and** element name; afterwards
/// `+0x4C = (flags & 0x30000000) == 0`.
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    lock(tree.node_mut(id));
}

/// `OnAttach` `0x00461C80` (12.1.12): the first query (`0x00461C9C`), then with `MENUFRAME_MUSIC` (`0x40`,
/// `0x00461CA1`) the broadcast `0x160 {2, MUSIC, !DONOTLOOPMUSIC}` from the top root (`0x00461CB9`...`0x00461D12`,
/// bit `0x800` via `SHR 0xB; NOT; AND 1`); the desktop turns it into [`crate::msg::UiEffect::MenuMusic`]. The path
/// goes out as read (uppercased by the tokenizer). Afterwards (`0x00461D17`...) level background or clearing of the
/// desktop set (5.4): load `levelbackground.bkg` or release the sets.
/// `PRELOAD` only preloads textures in the original; skipped, as it has no visible effect.
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    query(tree, id, env);
    let flags = tree.node(id).flags;
    if flags & frame_flags::MUSIC != 0 {
        let path = state(tree, id).desc.music.clone().unwrap_or_default();
        let looping = flags & frame_flags::DONOTLOOPMUSIC == 0;
        let root = if tree.node(id).parent.is_some() {
            tree.desktop()
        } else {
            id
        };
        tree.broadcast(root, &Broadcast::Music { path, looping }, env);
    }
    // `0x00461D17`...`0x00461DA1`: with `MENUFRAME_LEVELBACKGROUND` (`0x80`) the desktop loads
    // `data/menü/levelbackground.bkg` (`FUN_0045BB00`) as its set; otherwise, without `DONOTCLEARBACKGROUND`, it
    // releases its sets (`FUN_0045A0A0`: `+0x6C` empty, `+0x90 = 0`). It is drawn only over a visible world.
    let desktop = tree.desktop();
    if flags & frame_flags::LEVELBACKGROUND != 0 {
        let ui = &mut env.ui;
        let bkg = load_bkg(ui.vfs, LEVEL_BACKGROUND_BKG, ui.assets);
        let node = tree.node_mut(desktop);
        node.bkg_sets = vec![bkg];
        node.bkg_set = 0;
    } else if flags & frame_flags::DONOTCLEARBACKGROUND == 0 {
        let node = tree.node_mut(desktop);
        node.bkg_sets.clear();
        node.bkg_set = 0;
    }
}

/// Background of the desktop above the menu level (`0x0061A7B4`).
const LEVEL_BACKGROUND_BKG: &str = "data/menü/levelbackground.bkg";

/// `Tick` (slot 3, `FUN_004615A0`) in the drawing pass: movement step; when the movement ends, the frame reports
/// `0x136` (`FUN_00459F70` to the listener, here `on_done`) **before** setting the rectangle (`0x00460CB5`, then
/// `FUN_0045A210`); the rectangle keeps its size.
pub(crate) fn tick(
    tree: &mut UiTree,
    id: NodeId,
    env: &mut UiEnv,
    on_done: &mut dyn FnMut(&mut UiTree, NodeId, &mut UiEnv),
) {
    let Some(step) = state_mut(tree, id).tick(env.now) else {
        return;
    };
    if step.done {
        on_done(tree, id, env);
    }
    if tree.contains(id) {
        let node = tree.node_mut(id);
        let [l, t, r, b] = node.rect;
        let [x, y] = step.pos;
        node.rect = [
            x,
            y,
            x.wrapping_add(r.wrapping_sub(l)),
            y.wrapping_add(b.wrapping_sub(t)),
        ];
    }
}

/// Transition `FUN_004617F0(mode)` (5.3, 12.2): with `MOVE` broadcast `0x15E` (open) or `0x15F` (close) with
/// `BACKGROUND` (`+0xA0`) and `2*DURATION` from the top root, then the movement from `EXTENT.x1/y1` to
/// `ZIELPOSITION` or back; `+0x12C = mode`. Without `MOVE` broadcast with 500 ms (opening only with
/// `MENUFRAME_BACKGROUND`, closing always), then **immediately** `0x136`: the return value `true` tells the caller
/// that the transition is already finished (dialog stack).
pub fn transition(tree: &mut UiTree, id: NodeId, mode: FrameMode, env: &mut UiEnv) -> bool {
    let node = tree.node(id);
    let flags = node.flags;
    // `FUN_00459420`: the top root, provided the widget is attached.
    let root = if node.parent.is_some() {
        tree.desktop()
    } else {
        id
    };
    let s = state(tree, id);
    let path = s.desc.background.clone().unwrap_or_default();
    let image = |ms: u32| match mode {
        FrameMode::Open => Broadcast::ShowImage {
            path: path.clone(),
            ms,
        },
        FrameMode::Close => Broadcast::HideImage {
            path: path.clone(),
            ms,
        },
    };
    if flags & frame_flags::MOVE != 0 {
        let duration = s.desc.duration_ms;
        let extent = [s.desc.extent[0], s.desc.extent[1]];
        let goal = s.desc.target;
        tree.broadcast(root, &image(duration.wrapping_shl(1) as u32), env);
        let now = env.now;
        let s = state_mut(tree, id);
        match mode {
            FrameMode::Open => s.begin(extent, goal, duration, now),
            FrameMode::Close => s.begin(goal, extent, duration, now),
        }
        s.mode = Some(mode);
        return false;
    }
    if mode == FrameMode::Close || flags & frame_flags::BACKGROUND != 0 {
        tree.broadcast(root, &image(FADE_MS), env);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::testkit::Harness;

    /// `start.dlg`: `EXTENT -440 212 0 912`, `ZIELPOSITION 40 212`, `DURATION 300`.
    const START: &str = "FRAME START FLAGS MENUFRAME_NOFRAME MENUFRAME_MOVE /FLAGS EXTENT -440 212 0 912 \
                         ZIELPOSITION 40 212 DURATION 300 BAREXTENT 0 140 550 225 /FRAME\n";

    #[test]
    fn frame_queries_once_per_build_with_its_own_name() {
        // FUN_00461140: codes 9 and 5 under (frame, frame), once at construction; the response to code 5 locks.
        let mut h = Harness::new();
        h.rec.answer = Box::new(|m| {
            if m.code == crate::msg::code::QUERY_STATUS {
                m.params.insert(
                    "status".into(),
                    crate::msg::Param::Int(nml_formats::dlg::desc::common_flags::DISABLE as i32),
                );
                return 0;
            }
            -1
        });
        let frame = h.build(START);
        assert_eq!(
            h.rec.msgs,
            [
                ("start".to_string(), "start".to_string(), 9),
                ("start".to_string(), "start".to_string(), 5)
            ]
        );
        assert!(!h.tree.node(frame).enabled);
        h.forget();
        h.now = 10_000;
        h.with(|t, _, e| query(t, frame, e));
        assert!(h.rec.msgs.is_empty(), "without timeout never again");
    }

    #[test]
    fn step_follows_the_integer_formula() {
        let mut s = FrameState::new(FrameDesc::default());
        s.begin([-440, 212], [40, 212], 300, 1000);
        s.mode = Some(FrameMode::Open);
        let x = |s: &mut FrameState, now| s.tick(now).unwrap().pos[0];
        assert_eq!(x(&mut s, 1000), -440);
        assert_eq!(x(&mut s, 1075), -320);
        assert_eq!(s.bar, [-240, 0]);
        assert_eq!(x(&mut s, 1150), -200);
        assert_eq!(s.bar, [0, 0]);
        assert_eq!(x(&mut s, 1299), 39);
        let end = s.tick(1300).unwrap();
        assert_eq!(
            end,
            FrameStep {
                pos: [40, 212],
                done: true
            }
        );
        assert!(!s.moving);
        assert_eq!(s.tick(1400), None, "no movement anymore");
    }
}
