//! Dialog stack `MenuSystem` and command queue of the application (re/menu-system 12.1, 5.2;
//! re/menu-flow, section "Commands", with the correction for `0x15`).
//!
//! [`MenuSystem`] runs the orders `{name UPPERCASE, flag, mode}` for opening and closing dialogs: the
//! queue `+0x58`, the running transitions `+0x74` and the list of open names `+0xC8`. A dialog is built on
//! first opening and deleted after closing (O-97); the name table `+0x20` and the tree are in
//! [`UiTree`]. [`AppQueue`] is the command list `GameApp+0xD6B0` with the wait state `+0xD72C`; it hands out at most
//! one command per frame (`GameApp_Tick` `0x004CB3B4`-`0x004CB463`).

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use nml_formats::dlg::desc::frame_flags;
use nml_formats::dlg::{AppCommand, WidgetKind, upper_word};

use crate::dialog::DialogLibrary;
use crate::msg::AppCmd;
use crate::tree::{NodeId, NodeKind, UiEnv, UiTree};
use crate::widgets::frame::{self, FrameMode};

/// Length of a music file in ms (path as read in the `.dlg`); `None` = unknown.
pub type MusicLength = Box<dyn Fn(&str) -> Option<u32>>;

/// Whether the dialog stack has orders (`FUN_004DCF20` returns 1 if queue `+0x64` **and** running
/// transitions `+0x80` are empty). The listener of the widgets reads it for `0x12F` (12.1.11) while the stack itself
/// is mutably borrowed (processing `0x136` in the drawing pass); hence a shared flag that
/// [`MenuSystem`] updates after every change of its lists.
#[derive(Debug, Clone, Default)]
pub struct TriggerGate(Rc<Cell<bool>>);

impl TriggerGate {
    /// `true` while an order is waiting or running: every trigger is refused.
    pub fn busy(&self) -> bool {
        self.0.get()
    }
}

/// Order `{CString name (UPPERCASE), flag, mode}` (`FUN_004DE570`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    /// Flag 1 (set by the commands numbered 3 and 5): wait until no transition is running anymore.
    wait: bool,
    mode: FrameMode,
}

/// The dialog stack `MenuSystem` (`FUN_004DEBB0`) without the name table (which is in the tree).
pub struct MenuSystem {
    library: DialogLibrary,
    /// `+0x58`: waiting orders.
    pending: VecDeque<Entry>,
    /// `+0x74`: running transitions.
    running: Vec<Entry>,
    /// `+0xC8`: open names as passed, also duplicates (O-96).
    open_names: Vec<String>,
    gate: TriggerGate,
    /// Supplier of the music length: asked before building a frame with `MUSIC`.
    music_length: Option<MusicLength>,
    /// `Engine.ini` `IsMenuLevel` at build time: the build reads the variant of the `.dlg` that matches it
    /// ([`DialogLibrary::expanded_for`]); `MenuApp::frame` updates it before every frame.
    menu_level: bool,
}

impl MenuSystem {
    pub fn new(library: DialogLibrary, music_length: Option<MusicLength>) -> MenuSystem {
        MenuSystem {
            library,
            pending: VecDeque::new(),
            running: Vec::new(),
            open_names: Vec::new(),
            gate: TriggerGate::default(),
            music_length,
            menu_level: false,
        }
    }

    /// `IsMenuLevel` for the next builds (open dialogs stay as they were built, O-438).
    pub fn set_menu_level(&mut self, on: bool) {
        self.menu_level = on;
    }

    pub fn library(&self) -> &DialogLibrary {
        &self.library
    }

    /// The shared flag "orders open" for the listener of the widgets.
    pub fn gate(&self) -> TriggerGate {
        self.gate.clone()
    }

    /// Whether an order is waiting or running (the opposite of `FUN_004DCF20`); as long as it is, `0x12F` refuses
    /// every trigger.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty() || !self.running.is_empty()
    }

    /// Open names (`+0xC8`) in opening order, as passed.
    pub fn open_names(&self) -> &[String] {
        &self.open_names
    }

    fn sync(&self) {
        self.gate.0.set(self.busy());
    }

    /// Open `FUN_004E3010(name, flag)` (12.1.1): start or enqueue the order; if `name` is exactly `"start"`
    /// or `"start2"` (`_mbscmp`, case matters), additionally `open("logo", 0)`; on success the
    /// name goes to the list of open names, even if the order is only enqueued.
    pub fn open(&mut self, tree: &mut UiTree, name: &str, wait: bool, env: &mut UiEnv) -> bool {
        let entry = Entry {
            name: upper_word(name),
            wait,
            mode: FrameMode::Open,
        };
        let ok = self.start_or_queue(tree, entry, env);
        if name == "start" || name == "start2" {
            self.open(tree, "logo", false, env);
        }
        if ok {
            self.open_names.push(name.to_string());
        }
        ok
    }

    /// Close `FUN_004E2E80(name, flag)` (12.1.3): not loaded -> nothing (`true`); otherwise an order, with
    /// `start`/`start2` also `close("logo", 0)`, and the name leaves the list once.
    pub fn close(&mut self, tree: &mut UiTree, name: &str, wait: bool, env: &mut UiEnv) -> bool {
        if self.lookup(tree, name, false, env).is_none() {
            return true;
        }
        let entry = Entry {
            name: upper_word(name),
            wait,
            mode: FrameMode::Close,
        };
        let ok = self.start_or_queue(tree, entry, env);
        if name == "start" || name == "start2" {
            self.close(tree, "logo", false, env);
        }
        if let Some(i) = self.open_names.iter().position(|n| n == name) {
            self.open_names.remove(i);
        }
        ok
    }

    /// Close all `FUN_004E2C10` (command `0x34`, 12.1.9): for every open name that is loaded, a
    /// closing order with flag 0 (without the `logo` rule), then clear the list; duplicate names close twice.
    pub fn close_all(&mut self, tree: &mut UiTree, env: &mut UiEnv) {
        for name in self.open_names.clone() {
            if self.lookup(tree, &name, false, env).is_some() {
                let entry = Entry {
                    name: upper_word(&name),
                    wait: false,
                    mode: FrameMode::Close,
                };
                self.start_or_queue(tree, entry, env);
            }
        }
        self.open_names.clear();
    }

    /// `0x136` "transition done" (`FUN_004E2820` via the listener `0x004E2E50`, 12.1.4): remove every running order
    /// of this frame, destroy the dialog for a closing order ([`Self::destroy`]); then, if
    /// nothing is waiting and the name table is empty, make the dialog root invisible, otherwise start waiting orders
    /// until a flag-1 order meets a running transition. Without `MOVE`, `0x136` comes synchronously
    /// from [`frame::transition`] (return value `true`), so recursively.
    pub fn transition_done(&mut self, tree: &mut UiTree, frame: NodeId, env: &mut UiEnv) {
        let mut i = 0;
        while i < self.running.len() {
            if tree.find(&self.running[i].name) == Some(frame) {
                let entry = self.running.remove(i);
                self.sync();
                if entry.mode == FrameMode::Close {
                    destroy(tree, &entry.name);
                }
            } else {
                i += 1;
            }
        }
        if self.pending.is_empty() {
            if tree.name_count() == 0 {
                set_root_visible(tree, false);
            }
            return;
        }
        while let Some(head) = self.pending.front() {
            if head.wait && !self.running.is_empty() {
                return;
            }
            set_root_visible(tree, true);
            let Some(entry) = self.pending.pop_front() else {
                break;
            };
            self.sync();
            self.run(tree, entry, env);
        }
    }

    /// Start or enqueue `FUN_004E2740` (12.1.2): if transitions are running and the order has flag 1 or one is
    /// already waiting -> append at the back. Otherwise dialog root visible, fetch the frame or build it only now,
    /// order to "running", start the transition.
    fn start_or_queue(&mut self, tree: &mut UiTree, entry: Entry, env: &mut UiEnv) -> bool {
        if !self.running.is_empty() && (entry.wait || !self.pending.is_empty()) {
            self.pending.push_back(entry);
            self.sync();
            return true;
        }
        set_root_visible(tree, true);
        self.run(tree, entry, env)
    }

    /// Fetch the frame (`create = 1`), order to "running" (`AddTail` before `Transition`), transition; without `MOVE`
    /// immediately [`Self::transition_done`]. `false` if the dialog does not exist.
    fn run(&mut self, tree: &mut UiTree, entry: Entry, env: &mut UiEnv) -> bool {
        let Some(f) = self.lookup(tree, &entry.name, true, env) else {
            log::warn!("Dialog \"{}\" cannot be opened", entry.name);
            return false;
        };
        let mode = entry.mode;
        self.running.push(entry);
        self.sync();
        if frame::transition(tree, f, mode, env) {
            self.transition_done(tree, f, env);
        }
        true
    }

    /// Fetch `FUN_004E24F0(name, create)`: entry of the name table (uppercase); if it is missing and `create`,
    /// build the file from the map (all top-level widgets with the dialog root as parent, `OnAttach` on attaching)
    /// and search again. The commands of the file (`0x20`/`0x21`, the reader sends them when reading `BACKGROUND`/
    /// `LEVELBACKGROUND` via `FUN_004BCB40`, `0x004DDEF7`) go before the build - so before the queries in `OnAttach` -
    /// directly into the application queue ([`crate::msg::UiListener::push_command`]). Before building a frame
    /// with `MUSIC` the supplier sets the music length that the desktop reports to the credits on `0x162` (in the
    /// original the new music is already playing by then). An entry that is not a frame counts as missing (the
    /// reimplementation calls `Transition` only on frames).
    fn lookup(
        &mut self,
        tree: &mut UiTree,
        name: &str,
        create: bool,
        env: &mut UiEnv,
    ) -> Option<NodeId> {
        let frame_of = |tree: &UiTree, id: NodeId| {
            matches!(tree.node(id).kind, NodeKind::Frame(_)).then_some(id)
        };
        if let Some(id) = tree.find(name) {
            return frame_of(tree, id);
        }
        if !create {
            return None;
        }
        let expanded = self.library.expanded_for(name, self.menu_level)?;
        for cmd in &expanded.commands {
            env.listener.push_command(match cmd {
                AppCommand::HideLevelBackground => AppCmd::HideLevelBackground,
                AppCommand::LoadLevelBackground(path) => AppCmd::LoadMenuLevel(path.clone()),
            });
        }
        for widget in &expanded.widgets {
            if let WidgetKind::Frame(f) = &widget.kind
                && f.flags & frame_flags::MUSIC != 0
                && let Some(length) = &self.music_length
            {
                tree.set_music_length(length(f.music.as_deref().unwrap_or("")).unwrap_or(0));
            }
            tree.build_dialog(widget, env);
        }
        tree.find(name).and_then(|id| frame_of(tree, id))
    }
}

/// Destroy dialog `FUN_004DD170` (12.1.4, 12.1.5): remove every key that equals the name or starts with
/// it (O-90), and schedule the frame - for a modal one its `ModalBlocker` at the topmost root - for
/// deletion (12.1.8). No widget state remains; the next opening builds anew (O-97).
fn destroy(tree: &mut UiTree, name: &str) {
    let Some(frame) = tree.remove_names_with_prefix(name) else {
        return;
    };
    let Some(parent) = tree.node(frame).parent else {
        return;
    };
    if matches!(tree.node(parent).kind, NodeKind::ModalBlocker { owner } if owner == frame) {
        let desktop = tree.desktop();
        tree.schedule_delete(desktop, parent);
    } else {
        tree.schedule_delete(parent, frame);
    }
}

/// `FUN_004DCE40(v)`: visibility of the dialog root (the input bit `0x0066FA60+0x110` has no counterpart).
fn set_root_visible(tree: &mut UiTree, visible: bool) {
    let root = tree.dialog_root();
    tree.node_mut(root).visible = visible;
}

/// Wait state `GameApp+0xD72C`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// 1 (`0x15`): until the dialog root has no children (`FUN_004DCE80`).
    Dialogs,
    /// 2 (`0x1A`): until no video is running anymore.
    Video,
    /// 3 (`0x30`): until the clock passes the point in time.
    Until(u64),
}

/// Command queue of the application (`GameApp+0xD6B0`, `FUN_004BCB40` appends at the back) with a wait state.
#[derive(Debug, Clone, Default)]
pub struct AppQueue {
    cmds: VecDeque<AppCmd>,
    wait: Option<Wait>,
}

impl AppQueue {
    /// Append a command at the back (`FUN_004BCB40`).
    pub fn push(&mut self, c: AppCmd) {
        self.cmds.push_back(c);
    }

    /// Discard all waiting commands (`FUN_004BC6C0`: under the lock `GameApp+0xD740` free every command of the list
    /// `+0xD6A4`, `RemoveAll`; re/singleplayer-menus part 1, 6.1). The function does not know the wait state;
    /// its only caller `0x28` runs only when no waiting applies anymore anyway.
    pub fn clear(&mut self) {
        self.cmds.clear();
    }

    /// Discard waiting commands for which `keep` is false (no counterpart in the original:
    /// [`crate::app::MenuApp::end_with_message`] takes an enqueued `{0x0E}` out). The wait state
    /// stays.
    pub fn retain(&mut self, keep: impl FnMut(&AppCmd) -> bool) {
        self.cmds.retain(keep);
    }

    /// Waiting commands in sequence.
    pub fn iter(&self) -> impl Iterator<Item = &AppCmd> {
        self.cmds.iter()
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// Whether a wait state applies.
    pub fn waiting(&self) -> bool {
        self.wait.is_some()
    }

    /// The command of this frame after drawing (`GameApp_Tick` `0x004CB3B4`-`0x004CB463`). Without a wait state the
    /// foremost command; a wait command sets the state (`0x004CA3A3`, `0x004CA3B2`, `0x004C9AD9`: `0x30`
    /// waits until `now + ms`). With a wait state there is no command; once the condition is met, the
    /// state ends, but the next command comes only in the following frame:
    /// - `WaitDialogs`: the dialog root has no children anymore (blockers of modal dialogs do not count, 12.1.10);
    /// - `WaitVideo`: the menu root has no video anymore (`video_running` false; `FUN_0045CAE0` checks
    ///   `Root+0xB8 != 0`, re/video, section "After the video") - so immediately without an open video;
    /// - `WaitMs`: `now > point in time` (`JNC` at `0x004CB45B`).
    pub fn pop_ready(&mut self, tree: &UiTree, now: u64, video_running: bool) -> Option<AppCmd> {
        if let Some(wait) = self.wait {
            let done = match wait {
                Wait::Dialogs => tree.node(tree.dialog_root()).children.is_empty(),
                Wait::Video => !video_running,
                Wait::Until(t) => now > t,
            };
            if done {
                self.wait = None;
            }
            return None;
        }
        let cmd = self.cmds.pop_front()?;
        self.wait = match cmd {
            AppCmd::WaitDialogs => Some(Wait::Dialogs),
            AppCmd::WaitVideo => Some(Wait::Video),
            AppCmd::WaitMs(ms) => Some(Wait::Until(now.saturating_add(u64::from(ms)))),
            _ => None,
        };
        Some(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str) -> AppCmd {
        AppCmd::Open {
            name: name.into(),
            wait: true,
        }
    }

    #[test]
    fn queue_hands_out_one_command_and_waits() {
        let tree = UiTree::new();
        let mut q = AppQueue::default();
        q.push(AppCmd::WaitMs(100));
        q.push(open("a"));
        q.push(AppCmd::WaitVideo);
        q.push(open("b"));
        assert_eq!(q.pop_ready(&tree, 0, false), Some(AppCmd::WaitMs(100)));
        assert!(q.waiting());
        assert_eq!(q.pop_ready(&tree, 50, false), None);
        assert_eq!(
            q.pop_ready(&tree, 100, false),
            None,
            "not until after the point in time"
        );
        assert_eq!(
            q.pop_ready(&tree, 101, false),
            None,
            "the end costs one frame"
        );
        assert!(!q.waiting());
        assert_eq!(q.pop_ready(&tree, 102, false), Some(open("a")));
        assert_eq!(q.pop_ready(&tree, 103, false), Some(AppCmd::WaitVideo));
        assert_eq!(q.pop_ready(&tree, 104, false), None);
        assert_eq!(q.pop_ready(&tree, 105, false), Some(open("b")));
        assert_eq!(q.pop_ready(&tree, 106, false), None);
        assert!(q.is_empty());
    }

    #[test]
    fn queue_waits_for_the_video() {
        // `0x1A` (`GameApp+0xD72C = 2`) waits until the menu root has no video anymore (`FUN_0045CAE0`); the end
        // costs one frame like every wait state.
        let tree = UiTree::new();
        let mut q = AppQueue::default();
        q.push(AppCmd::PlayVideo("data/videos/t.bik".into()));
        q.push(AppCmd::WaitVideo);
        q.push(AppCmd::AfterVideo);
        assert_eq!(
            q.pop_ready(&tree, 0, false),
            Some(AppCmd::PlayVideo("data/videos/t.bik".into()))
        );
        assert_eq!(q.pop_ready(&tree, 16, true), Some(AppCmd::WaitVideo));
        for t in [32, 48, 5_000] {
            assert_eq!(q.pop_ready(&tree, t, true), None, "{t} ms");
            assert!(q.waiting());
        }
        assert_eq!(
            q.pop_ready(&tree, 5_016, false),
            None,
            "the end costs one frame"
        );
        assert!(!q.waiting());
        assert_eq!(q.pop_ready(&tree, 5_032, true), Some(AppCmd::AfterVideo));
        assert!(q.is_empty());
    }

    #[test]
    fn clear_drops_the_commands_but_not_the_wait() {
        // `FUN_004BC6C0` frees only the commands of the list; a running wait state is not part of it.
        let tree = UiTree::new();
        let mut q = AppQueue::default();
        q.push(AppCmd::WaitMs(100));
        q.push(open("a"));
        q.push(open("b"));
        assert_eq!(q.pop_ready(&tree, 0, false), Some(AppCmd::WaitMs(100)));
        q.clear();
        assert!(q.is_empty());
        assert!(q.waiting());
        q.push(open("c"));
        assert_eq!(q.pop_ready(&tree, 101, false), None);
        assert_eq!(q.pop_ready(&tree, 102, false), Some(open("c")));
    }
}
