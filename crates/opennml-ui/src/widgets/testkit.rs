//! Test helpers for the dispatcher and the element kinds: a recording listener and a scaffold of fixture, tree,
//! pointer and effects.

use crate::coords::Screen;
use crate::dialog::tests::{Fixture, dialog, fixture};
use crate::input::{MouseButton, Pointer, UiInput, dispatch_key, dispatch_mouse};
use crate::msg::{ElementMsg, Param, UiEffect, UiListener};
use crate::tree::{NodeId, UiEnv, UiTree};

/// Listener that writes down every message as (dialog, element, code) and in full (as sent, before the response);
/// `answer` answers it (default -1), `allow` permits triggering (`0x12F` = 0, otherwise -1).
pub(crate) struct Recorder {
    pub allow: bool,
    pub msgs: Vec<(String, String, u32)>,
    pub sent: Vec<ElementMsg>,
    pub answer: Box<dyn FnMut(&mut ElementMsg) -> i32>,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder {
            allow: false,
            msgs: Vec::new(),
            sent: Vec::new(),
            answer: Box::new(|_| -1),
        }
    }
}

impl UiListener for Recorder {
    fn element(&mut self, msg: &mut ElementMsg, _fx: &mut Vec<UiEffect>) -> i32 {
        self.msgs
            .push((msg.dialog.clone(), msg.element.clone(), msg.code));
        self.sent.push(msg.clone());
        (self.answer)(msg)
    }

    fn may_trigger(&mut self) -> i32 {
        if self.allow { 0 } else { -1 }
    }
}

/// Fixture, tree, listener, effects, pointer, program time and screen (default 1280 x 1024: pixels = grid).
pub(crate) struct Harness {
    pub f: Fixture,
    pub tree: UiTree,
    pub rec: Recorder,
    pub fx: Vec<UiEffect>,
    pub pointer: Pointer,
    pub now: u64,
    pub screen: Screen,
}

impl Harness {
    pub fn new() -> Harness {
        Harness {
            f: fixture(),
            tree: UiTree::new(),
            rec: Recorder::default(),
            fx: Vec::new(),
            pointer: Pointer::default(),
            now: 0,
            screen: Screen {
                width: 1280,
                height: 1024,
            },
        }
    }

    /// Calls `f` with the tree, the pointer and the run-time context.
    pub fn with<R>(&mut self, f: impl FnOnce(&mut UiTree, &mut Pointer, &mut UiEnv) -> R) -> R {
        let mut env = UiEnv {
            ui: self.f.ctx(self.screen),
            listener: &mut self.rec,
            fx: &mut self.fx,
            now: self.now,
        };
        f(&mut self.tree, &mut self.pointer, &mut env)
    }

    /// Builds a dialog from `.dlg` text and makes the dialog root visible (like `FUN_004DCE40(1)` when opening,
    /// 12.1.2; the scaffold bypasses the dialog stack `crate::menu`).
    pub fn build(&mut self, src: &str) -> NodeId {
        let desc = dialog(src);
        let id = self.with(|t, _, e| t.build_dialog(&desc, e));
        let root = self.tree.dialog_root();
        self.tree.node_mut(root).visible = true;
        id
    }

    pub fn input(&mut self, input: UiInput) -> bool {
        self.with(|t, p, e| dispatch_mouse(t, p, &input, e))
    }

    pub fn key(&mut self, held: &[u8], ch: Option<u16>) -> bool {
        self.with(|t, _, e| dispatch_key(t, held, ch, e))
    }

    /// Query "at the start of `Draw`" of the widget at time `self.now`.
    pub fn query(&mut self, id: NodeId) {
        self.with(|t, _, e| super::query(t, id, e));
    }

    /// Drawing pass [`crate::paint::tick_tree`] at time `self.now`; returns the frames whose transition ended in
    /// this pass (`0x136`), in the order of the reports.
    pub fn tick(&mut self) -> Vec<NodeId> {
        let mut done = Vec::new();
        self.with(|t, _, e| crate::paint::tick_tree(t, e, &mut |_, id, _| done.push(id)));
        done
    }

    /// [`crate::paint::paint_ui`] at time `self.now` (after [`Self::tick`]).
    pub fn paint(&mut self) -> Vec<crate::draw::DrawCmd> {
        let now = self.now;
        let mut ctx = self.f.ctx(self.screen);
        crate::paint::paint_ui(&self.tree, &mut ctx, now)
    }

    /// Left click at a point: move, press, release.
    pub fn click(&mut self, (x, y): (i32, i32)) {
        self.input(UiInput::Move { x, y });
        self.input(UiInput::Down(MouseButton::Left));
        self.input(UiInput::Up(MouseButton::Left));
    }

    /// Codes of the messages of an element (name lowercase) in the order sent.
    pub fn codes(&self, element: &str) -> Vec<u32> {
        self.rec
            .msgs
            .iter()
            .filter(|m| m.1 == element)
            .map(|m| m.2)
            .collect()
    }

    /// Integer parameter `name` of the sent messages of an element with code `code`.
    pub fn sent_ints(&self, element: &str, code: u32, name: &str) -> Vec<i32> {
        self.rec
            .sent
            .iter()
            .filter(|m| m.element == element && m.code == code)
            .filter_map(|m| match m.params.get(name) {
                Some(Param::Int(v)) => Some(*v),
                _ => None,
            })
            .collect()
    }

    /// Forget messages.
    pub fn forget(&mut self) {
        self.rec.msgs.clear();
        self.rec.sent.clear();
    }
}

/// Center of the absolute rectangle (at 1280 x 1024 also in screen pixels).
pub(crate) fn center(tree: &UiTree, id: NodeId) -> (i32, i32) {
    let r = tree.abs_rect(id);
    ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2)
}
