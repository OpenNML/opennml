//! Widget tree of the menu runtime (re/menu-system 2.3, 3.3, 5.1, 12.1, 12.5): arena of [`Node`]s with the desktop as
//! root and the invisible dialog root as first child, name table (`MenuSystem+0x20`), construction from a
//! [`WidgetDesc`] like `FUN_004E23E0`, broadcast (`FUN_00459FB0`) and deferred deletions
//! (`FUN_0045B9E0`/`FUN_0045B1E0`).
//!
//! Rectangles lie in the 1280 x 1024 grid relative to the client origin of the parent (`+0x10`); `abs_rect` and
//! `client_rect` compute absolutely (`FUN_004599A0`, `FUN_004598D0`). Node IDs are never reused; a deleted node is no
//! longer in the tree afterwards ([`UiTree::contains`]).
//!
//! The behavior of the element kinds (slots, `OnAttach`, `OnMessage`, queries) and their states live in
//! [`crate::widgets`], the dispatcher in [`crate::input`]; the tree only calls them.

use std::collections::BTreeMap;

use opennml_formats::dlg::desc::{common_flags, frame_flags};
use opennml_formats::dlg::{WidgetDesc, WidgetKind, upper_word};

use crate::bkg_paint::{LoadedBkg, load_bkg};
use crate::coords::{VIRTUAL_H, VIRTUAL_W, normalize_rect};
use crate::desktop::DesktopImages;
use crate::dialog::{
    BUTTON_BORDER_BKG, BUTTON_DISABLED_BKG, BUTTON_MOUSEOVER_BKG, BUTTON_NORMAL_BKG,
    BUTTON_PRESSED_BKG, EDITFIELD_BKG, UiContext, constructed_frame_flags, frame_bkg,
};
use crate::geom::{client_rel, offset};
use crate::msg::{ElementMsg, UiEffect, UiListener};
use crate::widgets;
pub use crate::widgets::button::ButtonState;
pub use crate::widgets::combo::PopupState;
pub use crate::widgets::edit::EditState;
pub use crate::widgets::frame::{FrameMode, FrameState};
pub use crate::widgets::label::{CreditsRoll, LabelState};
pub use crate::widgets::listbox::ListState;
pub use crate::widgets::radio::RadioState;
pub use crate::widgets::slider::SliderState;

/// The whole grid: rectangle of desktop, dialog root and `ModalBlocker` (`FUN_0045A270(0, 0, 0x500, 0x400)`).
pub(crate) const FULL: [i32; 4] = [0, 0, VIRTUAL_W, VIRTUAL_H];

/// Run-time context for construction, broadcast, dispatcher and ticks: data for loading and
/// measuring, the listener of the widgets, the collected effects and the program time in ms (never system time).
pub struct UiEnv<'a> {
    pub ui: UiContext<'a>,
    pub listener: &'a mut dyn UiListener,
    pub fx: &'a mut Vec<UiEffect>,
    pub now: u64,
}

/// ID of a node; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

/// A widget (base layout re/menu-system 2.3).
#[derive(Debug, Clone)]
pub struct Node {
    /// Parent `+0x0C`; only the desktop has none.
    pub parent: Option<NodeId>,
    /// Children `+0x08` in insertion order (head = drawn at the bottom/first, end = on top).
    pub children: Vec<NodeId>,
    /// Children marked for deletion `+0x04` (`FUN_0045B9E0`), processed by [`UiTree::run_deletes`].
    pub pending_deletes: Vec<NodeId>,
    /// Frame name in uppercase (`+0x98`); for the frame, its own name.
    pub dialog: String,
    /// Element name in uppercase with loop counters, without prefix (`+0x9C`, [`WidgetDesc::element`]).
    pub name: String,
    /// Rectangle `+0x10` in the grid relative to the client origin of the parent, corners ordered.
    pub rect: [i32; 4],
    /// Element flags (`+0xA0` or frame `+0xAC`), for the frame after the constructor `FUN_00461AD0`.
    pub flags: u32,
    /// `+0x30`: all constructors set 1 (`HIDE` never acts on this, 12.5); the dialog root starts invisible.
    pub visible: bool,
    /// `+0x4C` active; the base constructor sets 1 (12.5).
    pub enabled: bool,
    /// `+0x40` mouse over.
    pub hover: bool,
    /// `+0x41` left pressed.
    pub pressed_left: bool,
    /// `+0x43` right pressed.
    pub pressed_right: bool,
    /// `+0x34` mouse captured.
    pub captured: bool,
    /// `+0x35` keyboard focus (chain up to the root).
    pub focused: bool,
    /// Background sets `+0x6C` in constructor order; `None` if the `.bkg` is missing (set without a background, the
    /// numbers of the other sets stay).
    pub bkg_sets: Vec<Option<LoadedBkg>>,
    /// Active set (`+0x70...+0x8F`).
    pub bkg_set: usize,
    pub kind: NodeKind,
    /// Visit in the last drawing pass ([`crate::paint::tick_tree`]): number of the pass and the visible rectangle `V`
    /// in the grid that `Widget_RenderTree` gave to `DrawBackground`/`Draw`; [`crate::paint::paint_ui`] draws exactly
    /// the nodes visited this way.
    pub(crate) visit: Option<(u64, [i32; 4])>,
}

/// Kind of a node with its state (re/menu-system 2.1).
#[derive(Debug, Clone)]
pub enum NodeKind {
    /// `MenuDesktop` (`0x0061A2F0`), the topmost root.
    Desktop,
    /// Base widget without its own kind: the dialog root `MenuSystem+0x08`.
    Plain,
    Frame(FrameState),
    /// `MenuButton`, also the ComboBox (bit `0x10`).
    Button(ButtonState),
    ComboPopup(PopupState),
    ListBox(ListState),
    EditField(EditState),
    Slider(SliderState),
    Radio(RadioState),
    Label(LabelState),
    /// `ModalBlocker` (`0x0061AA18`): covers the grid and passes every broadcast on to `owner` (12.6).
    ModalBlocker {
        owner: NodeId,
    },
}

/// Construction type of a dialog: at run time or in the file state for static drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildMode {
    Runtime,
    Static,
}

/// Broadcasts (`Widget_Broadcast`, message IDs re/menu-system 3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broadcast {
    /// `0x17E`: ESC from the keyboard (or end of the credits).
    Escape,
    /// `0x17F`: ENTER.
    Enter,
    /// `0x12C`: take the selection from the list (raw index), close the list.
    ListPick(i32),
    /// `0x12D`: abort the list.
    ListCancel,
    /// `0x12E` code 5 as a broadcast: query again.
    Requery,
    /// `0x12E` code 2 as a broadcast: query the entries again (ListBox) or rebuild (Label: `OnAttach` again).
    RequeryItems,
    /// `0x140`: set the first line (nameless scroll bar -> parent).
    ScrollFirst(i32),
    /// `0x141`: set the steps and value of the scroll bar.
    ScrollRange { value: i32, steps: i32 },
    /// `0x15E`: fade in the background image.
    ShowImage { path: String, ms: u32 },
    /// `0x15F`: fade out the background image.
    HideImage { path: String, ms: u32 },
    /// `0x160`: play the menu music.
    Music { path: String, looping: bool },
    /// `0x161`: restart the menu music.
    RestartMusic,
    /// `0x162`: query the music length; the desktop answers (`FUN_0045CB70`) via [`UiTree::take_reply`].
    MusicLength,
}

/// The widget tree.
#[derive(Debug, Clone)]
pub struct UiTree {
    nodes: BTreeMap<NodeId, Node>,
    next_id: u32,
    desktop: NodeId,
    dialog_root: NodeId,
    /// Name table `MenuSystem+0x20`: key frame + element in uppercase ([`WidgetDesc::name`]).
    names: BTreeMap<String, NodeId>,
    /// The world behind the menu is drawn (`FUN_004BA680() == 0`): only then does the desktop draw its `.bkg` set
    /// `levelbackground.bkg` (`FUN_0045CAF0`). `MenuApp::frame` sets it.
    pub world_shown: bool,
    /// Length of the running menu music in ms that the desktop reports on `0x162` (`FUN_0045CB70`: without music 0);
    /// the host sets it with [`UiTree::set_music_length`].
    music_length_ms: u32,
    /// Reply field `msg[2]` of the last answered broadcast (`0x162`).
    reply: Option<i32>,
    /// Image table of the desktop (`MenuDesktop` `+0x94...`, 12.3); `0x15E`/`0x15F` to the desktop land here.
    pub images: DesktopImages,
    /// Number of the last drawing pass ([`crate::paint::tick_tree`]).
    pub(crate) pass: u64,
    /// Program time of the last drawing pass; [`crate::paint::paint_ui`] must get the same time, otherwise it draws
    /// crossfades at a different time than the pass advanced them.
    pub(crate) pass_now: u64,
    /// Last pointer point in the grid (absolute) that the dispatcher converted; the artificial mouse movement of
    /// `FUN_0045B850` (ComboBox list) runs there (`FUN_004CCB00` returns the current mouse position).
    pub(crate) pointer: Option<[i32; 2]>,
    /// Recorded slot calls (tests only, [`widgets::Trace`]).
    #[cfg(test)]
    pub(crate) trace: Vec<widgets::Trace>,
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

impl Node {
    /// Base constructor `FUN_0045A270`: visible, active, without a name and background sets.
    pub(crate) fn new(kind: NodeKind, rect: [i32; 4], flags: u32) -> Node {
        Node {
            parent: None,
            children: Vec::new(),
            pending_deletes: Vec::new(),
            dialog: String::new(),
            name: String::new(),
            rect,
            flags,
            visible: true,
            enabled: true,
            hover: false,
            pressed_left: false,
            pressed_right: false,
            captured: false,
            focused: false,
            bkg_sets: Vec::new(),
            bkg_set: 0,
            kind,
            visit: None,
        }
    }
}

impl UiTree {
    /// Desktop as root (0,0-1280,1024), below it the dialog root, initially invisible (`FUN_004DEBB0`,
    /// `0x004DED11`...`0x004DED89`).
    pub fn new() -> Self {
        let mut tree = UiTree {
            nodes: BTreeMap::new(),
            next_id: 0,
            desktop: NodeId(0),
            dialog_root: NodeId(0),
            names: BTreeMap::new(),
            world_shown: false,
            music_length_ms: 0,
            reply: None,
            images: DesktopImages::new(),
            pass: 0,
            pass_now: 0,
            pointer: None,
            #[cfg(test)]
            trace: Vec::new(),
        };
        tree.desktop = tree.alloc(Node::new(NodeKind::Desktop, FULL, 0));
        let mut root = Node::new(NodeKind::Plain, FULL, 0);
        root.visible = false;
        tree.dialog_root = tree.alloc(root);
        let (desktop, root) = (tree.desktop, tree.dialog_root);
        tree.link(desktop, root);
        tree
    }

    fn alloc(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, node);
        id
    }

    /// `AddChild(child, 0)` without `OnAttach`: append at the end (`AddTail`, `FUN_0045BA50`).
    fn link(&mut self, parent: NodeId, child: NodeId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// `Widget_AddChild` `FUN_0045BA50` with `after = 0`: append at the end, then `OnAttach` (slot 2) of the child.
    fn attach(&mut self, parent: NodeId, child: NodeId, env: &mut UiEnv) {
        self.link(parent, child);
        self.on_attach(child, env);
    }

    /// `Widget_AddChild(node, 0)` at run time (ComboBox list, scroll bar of ListBox and Label): append at the end, then
    /// `OnAttach` of the new child; returns its ID.
    pub(crate) fn add_child(&mut self, parent: NodeId, node: Node, env: &mut UiEnv) -> NodeId {
        let id = self.alloc(node);
        self.attach(parent, id, env);
        id
    }

    /// Delete all children immediately (`FUN_00459860`, `MenuLabel.OnAttach`), without marking.
    pub(crate) fn remove_children_now(&mut self, id: NodeId, env: &mut UiEnv) {
        for child in std::mem::take(&mut self.node_mut(id).children) {
            self.free(child, env);
        }
    }

    /// `OnAttach` (slot 2) after attaching, in construction order (parent before the children); behavior per kind in
    /// [`widgets::on_attach`].
    fn on_attach(&mut self, id: NodeId, env: &mut UiEnv) {
        widgets::on_attach(self, id, env);
    }

    /// Builds a widget with its children (element parser of the original, `FUN_004DD9C0` and siblings).
    fn build_widget(
        &mut self,
        desc: &WidgetDesc,
        parent: NodeId,
        mode: BuildMode,
        env: &mut UiEnv,
    ) -> NodeId {
        let mut node = construct(desc, env);
        if mode == BuildMode::Static
            && let WidgetKind::Frame(f) = &desc.kind
            && f.flags & frame_flags::MOVE != 0
        {
            // End position of the fly-in: top left corner = ZIELPOSITION, size from EXTENT (12.2).
            let [l, t, r, b] = node.rect;
            let [x, y] = f.target;
            node.rect = [
                x,
                y,
                x.wrapping_add(r.wrapping_sub(l)),
                y.wrapping_add(b.wrapping_sub(t)),
            ];
        }
        let modal = matches!(&desc.kind, WidgetKind::Frame(f) if f.flags & frame_flags::MODAL != 0);
        let id = self.alloc(node);
        if modal && !desc.children.is_empty() {
            // 0x004DE125: ModalBlocker(frame) at the topmost root (FUN_00459420), the frame into the blocker.
            // Only the construction at the first child checks 0x200; a childless frame arises at the end word (O-87).
            let blocker = self.alloc(Node::new(NodeKind::ModalBlocker { owner: id }, FULL, 0));
            let desktop = self.desktop;
            self.hang(desktop, blocker, mode, env);
            self.hang(blocker, id, mode, env);
        } else {
            self.hang(parent, id, mode, env);
        }
        // Name table MenuSystem+0x20 (`CMap::operator[]`: a duplicate key afterwards points to the new one).
        self.names.insert(desc.name.clone(), id);
        for child in &desc.children {
            self.build_widget(child, id, mode, env);
        }
        id
    }

    /// Attaching by construction type: at run time with `OnAttach`, statically with [`widgets::attach_static`].
    fn hang(&mut self, parent: NodeId, child: NodeId, mode: BuildMode, env: &mut UiEnv) {
        match mode {
            BuildMode::Runtime => self.attach(parent, child, env),
            BuildMode::Static => {
                self.link(parent, child);
                widgets::attach_static(self, child, env);
            }
        }
    }

    pub fn desktop(&self) -> NodeId {
        self.desktop
    }

    pub fn dialog_root(&self) -> NodeId {
        self.dialog_root
    }

    /// Whether the node is (still) in the tree.
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// The node; a deleted one is a programming error.
    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes
            .get(&id)
            .unwrap_or_else(|| panic!("widget {id:?} is not (any more) in the tree"))
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes
            .get_mut(&id)
            .unwrap_or_else(|| panic!("widget {id:?} is not (any more) in the tree"))
    }

    /// Builds a dialog like `FUN_004E23E0` with the dialog root as parent (12.1.2) and returns its topmost node
    /// (`desc`, in the shipped data the `FRAME`). Without `MODAL` a frame is appended at the end of its parent; with
    /// `MODAL` and at least one child (O-87) a `ModalBlocker` is appended at the end of the desktop, so behind the
    /// dialog root, and the frame below it (5.1, 12.1.6). Elements in file order; every node goes into the name table
    /// under [`WidgetDesc::name`], and `OnAttach` runs per widget when attaching.
    pub fn build_dialog(&mut self, desc: &WidgetDesc, env: &mut UiEnv) -> NodeId {
        let root = self.dialog_root;
        self.build_widget(desc, root, BuildMode::Runtime, env)
    }

    /// Builds a dialog in the **file state** for static drawing ([`crate::dialog::paint_static`]): frames with `MOVE`
    /// at the end position of the fly-in, instead of `OnAttach` only [`widgets::attach_static`] (first queries, slider
    /// set, credits; no focus, no scroll bars, no music). Position in the tree, names and blocker as in
    /// [`Self::build_dialog`].
    pub(crate) fn build_dialog_static(&mut self, desc: &WidgetDesc, env: &mut UiEnv) -> NodeId {
        let root = self.dialog_root;
        self.build_widget(desc, root, BuildMode::Static, env)
    }

    /// Entry of the name table; the key is uppercased as in `FUN_004E24F0` (`FUN_00574EA0`). An entry pointing to an
    /// already deleted node (in the original a freed pointer) counts as missing.
    pub fn find(&self, key: &str) -> Option<NodeId> {
        self.names
            .get(&upper_word(key))
            .copied()
            .filter(|&id| self.contains(id))
    }

    /// Table part of `FUN_004DD170` (12.1.4): removes every key that equals `name` (uppercase) or starts with it
    /// (`CString_Left(len)`, O-90), and returns the node of the exactly equal key - the frame of the dialog that the
    /// caller (or its `ModalBlocker`) marks for deletion.
    pub fn remove_names_with_prefix(&mut self, name: &str) -> Option<NodeId> {
        let key = upper_word(name);
        let frame = self.names.get(&key).copied();
        self.names.retain(|k, _| !k.starts_with(key.as_str()));
        frame.filter(|&id| self.contains(id))
    }

    /// Marks a child for deletion (`FUN_0045B9E0`: at the end of the list `+0x04` of the parent). The node stays in the
    /// tree until [`Self::run_deletes`], so it keeps being ticked, drawn and hit (12.1.8).
    pub fn schedule_delete(&mut self, parent: NodeId, child: NodeId) {
        self.node_mut(parent).pending_deletes.push(child);
    }

    /// Processes the marked deletions (`FUN_0045B1E0` at the desktop): first recursively the children in list order,
    /// then its own list in marking order. An entry that is not (any more) a child does nothing
    /// (`FUN_0045A3C0`); otherwise the child falls out of the tree together with its subtree.
    pub fn run_deletes(&mut self, env: &mut UiEnv) {
        let desktop = self.desktop;
        self.run_deletes_at(desktop, env);
    }

    ///
    /// The own list is processed until it is empty: a destructor can append new entries to the **same** list while it
    /// is being processed (the `ComboPopup` its blocker to that of the desktop, [`Self::free`]). Deviation (O-109):
    /// `FUN_0045B1E0` reads the successor before deleting (`0x0045B215`) and frees the list afterwards (`0x0045B22E`);
    /// if the deleted entry was the last one, the new one is lost with the list, and the blocker of an open ComboBox
    /// list in a modal dialog stays on the desktop forever (list visible, every click swallowed, owner freed).
    fn run_deletes_at(&mut self, id: NodeId, env: &mut UiEnv) {
        for child in self.node(id).children.clone() {
            if self.contains(child) {
                self.run_deletes_at(child, env);
            }
        }
        loop {
            let pending = std::mem::take(&mut self.node_mut(id).pending_deletes);
            if pending.is_empty() {
                break;
            }
            for child in pending {
                if let Some(pos) = self.node(id).children.iter().position(|&c| c == child) {
                    self.node_mut(id).children.remove(pos);
                    self.free(child, env);
                }
            }
        }
    }

    /// Removes a node together with its subtree from the arena. The destructor of the `ComboPopup` (`FUN_0045F230`)
    /// marks its `ModalBlocker` for deletion at the topmost root (desktop). If the popup lies in a non-modal frame, the
    /// frame falls while descending into the dialog root, and [`Self::run_deletes`] processes the list of the desktop
    /// only afterwards - blocker and list fall in the same pass. If it lies in a modal frame, it falls **out of** the
    /// list of the desktop together with its blocker; the new entry lands in the list that is just being processed
    /// (lost in the original, O-109; deleted here in the same pass, [`Self::run_deletes_at`]). The destructor of the
    /// credits label (`FUN_004622A0`) fades out its shown image ([`widgets::label::destroyed`]).
    fn free(&mut self, id: NodeId, env: &mut UiEnv) {
        if let Some(node) = self.nodes.remove(&id) {
            match &node.kind {
                NodeKind::ComboPopup(PopupState {
                    blocker: Some(blocker),
                    ..
                }) => {
                    let desktop = self.desktop;
                    if self.contains(desktop) {
                        self.schedule_delete(desktop, *blocker);
                    }
                }
                NodeKind::Label(state) => widgets::label::destroyed(self, node.flags, state, env),
                _ => {}
            }
            for child in node.children {
                self.free(child, env);
            }
        }
    }

    /// `Widget_Broadcast` `FUN_00459FB0` from `from`: depth-first search, children in insertion order, then `OnMessage`
    /// (slot 19) of the node itself; the first receiver ends the broadcast (`true`). Visibility and lock do not count
    /// (3.3). The child list is traversed live, the successor read before the visit: a child appended during the visit
    /// is still reached, unless the visited one was the last.
    pub fn broadcast(&mut self, from: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
        if !self.contains(from) {
            return false;
        }
        let mut i = 0;
        while let Some(&child) = self.node(from).children.get(i) {
            let was_last = i + 1 >= self.node(from).children.len();
            if self.broadcast(child, msg, env) {
                return true;
            }
            if was_last || !self.contains(from) {
                break;
            }
            i += 1;
        }
        self.on_message(from, msg, env)
    }

    /// Targeted requery (`FUN_004DD600`): the element `dialog`/`element` via the name table (key frame name + element
    /// name, uppercase) and its `OnMessage` (slot 19) with `{0x12E, code}`: code 5 as [`Broadcast::Requery`], code 2 as
    /// [`Broadcast::RequeryItems`]. If the node is missing, nothing happens (the original checks the lookup,
    /// `0x004DD600`); other codes are known by no widget of the reimplementation (warning). Returns whether the widget
    /// accepted the message.
    pub fn requery(&mut self, dialog: &str, element: &str, code: u32, env: &mut UiEnv) -> bool {
        let Some(id) = self.find(&format!("{dialog}{element}")) else {
            return false;
        };
        let msg = match code {
            5 => Broadcast::Requery,
            2 => Broadcast::RequeryItems,
            other => {
                log::warn!(
                    "Requery {dialog}/{element} with code {other}: no receiver in the reimplementation"
                );
                return false;
            }
        };
        self.on_message(id, &msg, env)
    }

    /// `OnMessage` (slot 19) per kind ([`widgets::on_message`]): `ModalBlocker` passes on, button takes ESC/ENTER and
    /// the requery, the desktop the image broadcasts ([`Self::images`]) and the music length.
    fn on_message(&mut self, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
        widgets::on_message(self, id, msg, env)
    }

    /// Number of entries of the name table (`MenuSystem+0x2C`); without entries the dialog root becomes invisible after
    /// the last transition (12.1.4).
    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    /// Music length in ms that the desktop reports on the broadcast `0x162` (host: length of the running menu music,
    /// 0 without music).
    pub fn set_music_length(&mut self, ms: u32) {
        self.music_length_ms = ms;
    }

    /// Put the answer of the desktop to `0x162` (`FUN_0045CB70`) into the reply field.
    pub(crate) fn answer_music_length(&mut self) {
        self.reply = Some(i32::try_from(self.music_length_ms).unwrap_or(i32::MAX));
    }

    /// Fetch and clear the reply field `msg[2]` of the last broadcast; `None` without a reply.
    pub(crate) fn take_reply(&mut self) -> Option<i32> {
        self.reply.take()
    }

    /// Keyboard focus (`FUN_0045B240`): clear in the whole tree, then set for `id` and all ancestors.
    pub fn set_focus(&mut self, id: NodeId) {
        for node in self.nodes.values_mut() {
            node.focused = false;
        }
        let mut cur = Some(id);
        while let Some(c) = cur {
            let node = self.node_mut(c);
            node.focused = true;
            cur = node.parent;
        }
    }

    /// Client area relative to the own corner (`FUN_004598D0`) with the `CLIENTAREA` of the active set.
    fn client_rel(&self, id: NodeId) -> [i32; 4] {
        let node = self.node(id);
        let area = node
            .bkg_sets
            .get(node.bkg_set)
            .and_then(Option::as_ref)
            .map(|b| b.desc.client_area);
        client_rel(node.rect, area)
    }

    /// Absolute grid rectangle (`FUN_00459A10`): the own rectangle, shifted by corner + client origin of every ancestor
    /// (`FUN_004599A0`).
    pub fn abs_rect(&self, id: NodeId) -> [i32; 4] {
        let mut rect = self.node(id).rect;
        let mut cur = self.node(id).parent;
        while let Some(p) = cur {
            let node = self.node(p);
            let c = self.client_rel(p);
            rect = offset(
                rect,
                [
                    node.rect[0].wrapping_add(c[0]),
                    node.rect[1].wrapping_add(c[1]),
                ],
            );
            cur = node.parent;
        }
        rect
    }

    /// Absolute client area in the grid (`FUN_00459AC0`).
    pub fn client_rect(&self, id: NodeId) -> [i32; 4] {
        let abs = self.abs_rect(id);
        offset(self.client_rel(id), [abs[0], abs[1]])
    }

    /// Element message with dialog and element names of the node (`FUN_00464970`).
    pub fn element_msg(&self, id: NodeId, code: u32) -> ElementMsg {
        let node = self.node(id);
        ElementMsg::new(&node.dialog, &node.name, code)
    }
}

/// Constructor of a widget from the descriptor (base `FUN_0045A270` and the constructors per kind): rectangle with
/// ordered corners - for the frame `EXTENT`, also with `MOVE` (the fly-in starts there, 12.2) -, flags (frame after
/// [`constructed_frame_flags`]), names and the background sets in constructor order: frame
/// `menuframe.bkg`/`BKGFILE` (without `NOFRAME`), button and ComboBox `button_normal`/`_mouseover`/`_pressed`/
/// `_disabled`/`_border.bkg` (3.5), ListBox `comboframe.bkg`, EditField `editfield.bkg`. RadioButton and Label
/// have no set; the slider loads its own in `OnAttach` (`0x00465660`). The EditField is locked from the constructor
/// on if the file carries `DISABLE` (`FUN_0045FD20`, 12.6).
fn construct(desc: &WidgetDesc, env: &mut UiEnv) -> Node {
    let (kind, extent, flags, bkgs): (NodeKind, [i32; 4], u32, Vec<String>) = match &desc.kind {
        WidgetKind::Frame(f) => (
            NodeKind::Frame(FrameState::new(f.clone())),
            f.extent,
            constructed_frame_flags(f.flags),
            frame_bkg(f).into_iter().collect(),
        ),
        WidgetKind::Button(b) | WidgetKind::ComboBox(b, _) => {
            let list = match &desc.kind {
                WidgetKind::ComboBox(_, l) => Some(l.clone()),
                _ => None,
            };
            let sets = [
                BUTTON_NORMAL_BKG,
                BUTTON_MOUSEOVER_BKG,
                BUTTON_PRESSED_BKG,
                BUTTON_DISABLED_BKG,
                BUTTON_BORDER_BKG,
            ];
            (
                NodeKind::Button(ButtonState::new(b.clone(), list)),
                b.extent,
                b.flags,
                sets.iter().map(|s| s.to_string()).collect(),
            )
        }
        WidgetKind::ListBox(l) => {
            let mut node = widgets::listbox::new_node(l.clone(), env);
            name_node(&mut node, desc);
            return node;
        }
        WidgetKind::EditField(e) => (
            NodeKind::EditField(EditState::new(e.clone())),
            e.extent,
            e.flags,
            vec![EDITFIELD_BKG.to_string()],
        ),
        WidgetKind::Slider(s) => {
            let mut node = widgets::slider::new_node(s.clone());
            name_node(&mut node, desc);
            return node;
        }
        WidgetKind::RadioButton(r) => (
            NodeKind::Radio(RadioState::new(r.clone())),
            r.extent,
            r.flags,
            Vec::new(),
        ),
        WidgetKind::Label(l) => (
            NodeKind::Label(LabelState::new(l.clone())),
            l.extent,
            l.flags,
            Vec::new(),
        ),
    };
    let mut node = Node::new(kind, normalize_rect(extent), flags);
    if matches!(desc.kind, WidgetKind::EditField(_)) {
        node.enabled = flags & common_flags::DISABLE == 0;
    }
    name_node(&mut node, desc);
    let ui = &mut env.ui;
    node.bkg_sets = bkgs
        .iter()
        .map(|path| load_bkg(ui.vfs, path, ui.assets))
        .collect();
    node
}

/// Names from the descriptor: `+0x98` dialog name = name of the enclosing FRAME (`MenuSystem+0x1C`), so the key
/// without the element name at the end (for the frame, its name); `+0x9C` element name.
fn name_node(node: &mut Node, desc: &WidgetDesc) {
    node.dialog = match desc.kind {
        WidgetKind::Frame(_) => desc.name.clone(),
        _ => desc
            .name
            .strip_suffix(desc.element.as_str())
            .unwrap_or_default()
            .to_string(),
    };
    node.name = desc.element.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::Screen;
    use crate::dialog::tests::{Fixture, dialog, fixture};
    use crate::msg::{NoListener, code};

    const S1280: Screen = Screen {
        width: 1280,
        height: 1024,
    };

    /// Listener that allows triggering and records every message.
    #[derive(Default)]
    struct Recorder {
        allow: bool,
        msgs: Vec<(String, String, u32)>,
    }

    impl UiListener for Recorder {
        fn element(&mut self, msg: &mut ElementMsg, _fx: &mut Vec<UiEffect>) -> i32 {
            self.msgs
                .push((msg.dialog.clone(), msg.element.clone(), msg.code));
            -1
        }

        fn may_trigger(&mut self) -> i32 {
            if self.allow { 0 } else { -1 }
        }
    }

    fn build(
        f: &mut Fixture,
        tree: &mut UiTree,
        src: &str,
        listener: &mut dyn UiListener,
    ) -> NodeId {
        let desc = dialog(src);
        let mut fx = Vec::new();
        let mut env = UiEnv {
            ui: f.ctx(S1280),
            listener,
            fx: &mut fx,
            now: 0,
        };
        tree.build_dialog(&desc, &mut env)
    }

    fn run_deletes(f: &mut Fixture, tree: &mut UiTree) {
        let mut fx = Vec::new();
        let mut env = UiEnv {
            ui: f.ctx(S1280),
            listener: &mut NoListener,
            fx: &mut fx,
            now: 0,
        };
        tree.run_deletes(&mut env);
    }

    fn broadcast(
        f: &mut Fixture,
        tree: &mut UiTree,
        msg: &Broadcast,
        listener: &mut dyn UiListener,
    ) -> bool {
        let mut fx = Vec::new();
        let mut env = UiEnv {
            ui: f.ctx(S1280),
            listener,
            fx: &mut fx,
            now: 0,
        };
        let desktop = tree.desktop();
        tree.broadcast(desktop, msg, &mut env)
    }

    #[test]
    fn new_tree_has_desktop_and_hidden_dialog_root() {
        let tree = UiTree::new();
        let desktop = tree.node(tree.desktop());
        assert!(matches!(desktop.kind, NodeKind::Desktop));
        assert_eq!(desktop.parent, None);
        assert_eq!(desktop.children, [tree.dialog_root()]);
        assert_eq!(desktop.rect, [0, 0, 1280, 1024]);
        let root = tree.node(tree.dialog_root());
        assert!(matches!(root.kind, NodeKind::Plain));
        assert_eq!(root.parent, Some(tree.desktop()));
        assert!(!root.visible);
        assert_eq!(root.rect, [0, 0, 1280, 1024]);
    }

    // QUIRK O-87: kept
    #[test]
    fn modal_frames_hang_under_a_blocker_at_the_desktop() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        let frame = build(
            &mut f,
            &mut tree,
            "FRAME MSGBOX\n FLAGS MENUFRAME_MODAL /FLAGS\n EXTENT 100 100 500 400\n\
             BUTTON OK EXTENT 0 0 10 10 /BUTTON\n/FRAME\n",
            &mut NoListener,
        );
        let desktop = tree.desktop();
        let children = tree.node(desktop).children.clone();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0], tree.dialog_root());
        let blocker = children[1];
        assert!(
            matches!(tree.node(blocker).kind, NodeKind::ModalBlocker { owner } if owner == frame)
        );
        assert_eq!(tree.node(blocker).rect, [0, 0, 1280, 1024]);
        assert_eq!(tree.node(blocker).children, [frame]);
        assert_eq!(tree.node(frame).parent, Some(blocker));
        assert!(tree.node(tree.dialog_root()).children.is_empty());
        // Position: frame absolute like EXTENT, client around CLIENTAREA 15 30 15 30 (menuframe.bkg of the test data).
        assert_eq!(tree.abs_rect(frame), [100, 100, 500, 400]);
        assert_eq!(tree.client_rect(frame), [115, 130, 485, 370]);
        let ok = tree.find("MSGBOXOK").expect("OK");
        assert_eq!(tree.node(ok).parent, Some(frame));
        assert_eq!(tree.abs_rect(ok), [115, 130, 125, 140]);
        // A later built non-modal dialog attaches to the dialog root, the blocker stays on top behind it.
        let other = build(
            &mut f,
            &mut tree,
            "FRAME START EXTENT 0 0 10 10 /FRAME\n",
            &mut NoListener,
        );
        assert_eq!(tree.node(tree.dialog_root()).children, [other]);
        assert_eq!(tree.node(desktop).children, [tree.dialog_root(), blocker]);
    }

    // QUIRK O-87: kept
    #[test]
    fn childless_modal_frame_is_not_modal() {
        // O-87: only the construction at the first child creates the ModalBlocker.
        let mut f = fixture();
        let mut tree = UiTree::new();
        let frame = build(
            &mut f,
            &mut tree,
            "FRAME EMPTY FLAGS MENUFRAME_MODAL /FLAGS EXTENT 0 0 10 10 /FRAME\n",
            &mut NoListener,
        );
        assert_eq!(tree.node(frame).parent, Some(tree.dialog_root()));
        assert_eq!(tree.node(tree.desktop()).children, [tree.dialog_root()]);
        assert_ne!(tree.node(frame).flags & frame_flags::MODAL, 0);
    }

    #[test]
    fn names_are_frame_prefixed_and_upper() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        let frame = build(
            &mut f,
            &mut tree,
            "FRAME options\n FLAGS MENUFRAME_MOVE /FLAGS\n EXTENT -440 212 0 912\n ZIELPOSITION 40 212\n\
             BUTTON ok EXTENT 0 0 10 10 /BUTTON\n LOOP 2\n LABEL t /LABEL\n /LOOP\n/FRAME\n",
            &mut NoListener,
        );
        assert_eq!(tree.find("OPTIONS"), Some(frame));
        let ok = tree.find("OPTIONSOK").expect("OPTIONSOK");
        assert_eq!(tree.find("optionsok"), Some(ok));
        assert_eq!(tree.find("OK"), None);
        assert_eq!(tree.node(ok).dialog, "OPTIONS");
        assert_eq!(tree.node(ok).name, "OK");
        assert_eq!(tree.node(frame).dialog, "OPTIONS");
        assert_eq!(tree.node(frame).name, "OPTIONS");
        let t1 = tree.find("OPTIONST1").expect("OPTIONST1");
        assert_eq!(tree.node(t1).name, "T1");
        // Messages carry the names in lowercase (frame, element).
        let msg = tree.element_msg(ok, code::TRIGGERED);
        assert_eq!(
            (msg.dialog.as_str(), msg.element.as_str()),
            ("options", "ok")
        );
        let msg = tree.element_msg(frame, code::QUERY_STATUS);
        assert_eq!(
            (msg.dialog.as_str(), msg.element.as_str()),
            ("options", "options")
        );
        // A frame with MOVE starts at EXTENT (12.2), children in file order.
        assert_eq!(tree.node(frame).rect, [-440, 212, 0, 912]);
        assert_eq!(tree.node(frame).children.len(), 3);
        assert_eq!(tree.node(frame).children[0], ok);
        // Background sets of the button in constructor order, missing files as a gap.
        let sets = &tree.node(ok).bkg_sets;
        assert_eq!(sets.len(), 5);
        assert!(sets[0].is_some() && sets[1].is_none() && sets[2].is_none());
        assert!(sets[3].is_some() && sets[4].is_some());
        assert_eq!(tree.node(ok).bkg_set, 0);
    }

    // QUIRK O-90: kept
    #[test]
    fn prefix_removal_takes_neighbours() {
        // O-90: removing SOUNDOPTIONS also hits SOUNDOPTIONS2 and its elements.
        let mut f = fixture();
        let mut tree = UiTree::new();
        let sound = build(
            &mut f,
            &mut tree,
            "FRAME SOUNDOPTIONS EXTENT 0 0 10 10 BUTTON BACK /BUTTON /FRAME\n",
            &mut NoListener,
        );
        let sound2 = build(
            &mut f,
            &mut tree,
            "FRAME SOUNDOPTIONS2 EXTENT 0 0 10 10 BUTTON OK /BUTTON /FRAME\n",
            &mut NoListener,
        );
        let other = build(
            &mut f,
            &mut tree,
            "FRAME SOUND EXTENT 0 0 10 10 BUTTON OK /BUTTON /FRAME\n",
            &mut NoListener,
        );
        assert_eq!(tree.remove_names_with_prefix("soundoptions"), Some(sound));
        for key in [
            "SOUNDOPTIONS",
            "SOUNDOPTIONSBACK",
            "SOUNDOPTIONS2",
            "SOUNDOPTIONS2OK",
        ] {
            assert_eq!(tree.find(key), None, "{key}");
        }
        // Shorter names stay; the nodes themselves stay in the tree (deleting is a separate step).
        assert_eq!(tree.find("SOUND"), Some(other));
        assert!(tree.find("SOUNDOK").is_some());
        assert!(tree.contains(sound) && tree.contains(sound2));
        // Without an exact match no frame.
        assert_eq!(tree.remove_names_with_prefix("SOUNDOPTIONS"), None);
    }

    // QUIRK O-15: kept
    #[test]
    fn broadcast_ignores_visibility_and_stops_at_the_first_taker() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        build(
            &mut f,
            &mut tree,
            "FRAME DLG EXTENT 0 0 100 100\n\
             BUTTON A FLAGS MENUBUTTON_CHECKESCAPE /FLAGS /BUTTON\n\
             BUTTON B FLAGS MENUBUTTON_CHECKESCAPE MENUBUTTON_CHECKENTER /FLAGS /BUTTON\n/FRAME\n",
            &mut NoListener,
        );
        let a = tree.find("DLGA").unwrap();
        tree.node_mut(a).visible = false;
        tree.node_mut(a).enabled = false;
        // The dialog root is invisible, A hidden and locked: the broadcast reaches A anyway, and only A.
        let mut rec = Recorder {
            allow: true,
            ..Recorder::default()
        };
        assert!(broadcast(&mut f, &mut tree, &Broadcast::Escape, &mut rec));
        assert_eq!(
            rec.msgs,
            [("dlg".to_string(), "a".to_string(), code::TRIGGERED)]
        );
        let mut rec = Recorder {
            allow: true,
            ..Recorder::default()
        };
        assert!(broadcast(&mut f, &mut tree, &Broadcast::Enter, &mut rec));
        assert_eq!(
            rec.msgs,
            [("dlg".to_string(), "b".to_string(), code::TRIGGERED)]
        );
        // Without a release (0x12F returns -1) nobody accepts, there is no message.
        let mut rec = Recorder::default();
        assert!(!broadcast(&mut f, &mut tree, &Broadcast::Escape, &mut rec));
        assert!(rec.msgs.is_empty());
        // A button does not accept other broadcasts.
        let mut rec = Recorder {
            allow: true,
            ..Recorder::default()
        };
        assert!(!broadcast(
            &mut f,
            &mut tree,
            &Broadcast::ScrollFirst(0),
            &mut rec
        ));
        assert!(rec.msgs.is_empty());
    }

    #[test]
    fn broadcast_reaches_modal_frames_through_their_blocker() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        build(
            &mut f,
            &mut tree,
            "FRAME MSGBOX FLAGS MENUFRAME_MODAL /FLAGS EXTENT 0 0 100 100\n\
             BUTTON OK FLAGS MENUBUTTON_CHECKENTER /FLAGS /BUTTON\n/FRAME\n",
            &mut NoListener,
        );
        let mut rec = Recorder {
            allow: true,
            ..Recorder::default()
        };
        assert!(broadcast(&mut f, &mut tree, &Broadcast::Enter, &mut rec));
        assert_eq!(
            rec.msgs,
            [("msgbox".to_string(), "ok".to_string(), code::TRIGGERED)]
        );
    }

    #[test]
    fn deletes_wait_for_run_deletes() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        let frame = build(
            &mut f,
            &mut tree,
            "FRAME DLG EXTENT 0 0 100 100 BUTTON A /BUTTON BUTTON B /BUTTON /FRAME\n",
            &mut NoListener,
        );
        let a = tree.find("DLGA").unwrap();
        let b = tree.find("DLGB").unwrap();
        let root = tree.dialog_root();
        tree.schedule_delete(frame, a);
        tree.schedule_delete(root, frame);
        // Marked is not yet deleted.
        assert!(tree.contains(frame) && tree.contains(a) && tree.contains(b));
        assert_eq!(tree.node(root).children, [frame]);
        assert_eq!(tree.node(frame).children, [a, b]);
        run_deletes(&mut f, &mut tree);
        assert!(!tree.contains(frame) && !tree.contains(a) && !tree.contains(b));
        assert!(tree.node(root).children.is_empty());
        assert!(tree.node(root).pending_deletes.is_empty());
        // Orphaned table entries count as missing; processing again does nothing.
        assert_eq!(tree.find("DLG"), None);
        run_deletes(&mut f, &mut tree);
        // A child that no longer hangs under the parent is not deleted (FUN_0045A3C0).
        let other = build(
            &mut f,
            &mut tree,
            "FRAME X EXTENT 0 0 1 1 /FRAME\n",
            &mut NoListener,
        );
        let desktop = tree.desktop();
        tree.schedule_delete(desktop, other);
        run_deletes(&mut f, &mut tree);
        assert!(tree.contains(other));
    }

    #[test]
    fn focus_marks_one_chain() {
        let mut f = fixture();
        let mut tree = UiTree::new();
        let frame = build(
            &mut f,
            &mut tree,
            "FRAME DLG EXTENT 0 0 100 100 BUTTON A /BUTTON BUTTON B /BUTTON /FRAME\n",
            &mut NoListener,
        );
        let a = tree.find("DLGA").unwrap();
        let b = tree.find("DLGB").unwrap();
        tree.set_focus(a);
        tree.set_focus(b);
        let focused: Vec<NodeId> = [tree.desktop(), tree.dialog_root(), frame, a, b]
            .into_iter()
            .filter(|&id| tree.node(id).focused)
            .collect();
        assert_eq!(focused, [tree.desktop(), tree.dialog_root(), frame, b]);
    }

    #[test]
    fn requery_reaches_the_named_element_only() {
        use crate::widgets::testkit::Harness;
        let mut h = Harness::new();
        h.build(
            "FRAME DLG EXTENT 0 0 600 500
 BUTTON OK EXTENT 0 0 10 10 /BUTTON
             LISTBOX L EXTENT 0 20 200 120 /LISTBOX
/FRAME
",
        );
        h.rec.msgs.clear();
        // Code 5 to the button: full query (9, 5, 7), accepted; the names regardless of case.
        assert!(h.with(|t, _, e| t.requery("dlg", "ok", 5, e)));
        assert_eq!(
            h.rec.msgs.iter().map(|m| m.2).collect::<Vec<_>>(),
            [9, 5, 7]
        );
        assert!(h.rec.msgs.iter().all(|m| m.1 == "ok"));
        // Code 2 to the list: entries anew (the list asks 9, 5, 2, 0).
        h.rec.msgs.clear();
        assert!(h.with(|t, _, e| t.requery("DLG", "L", 2, e)));
        assert!(
            h.rec
                .msgs
                .iter()
                .any(|m| m.1 == "l" && m.2 == code::QUERY_ITEMS)
        );
        // The button does not accept code 2; missing elements and other codes: nothing.
        h.rec.msgs.clear();
        assert!(!h.with(|t, _, e| t.requery("dlg", "ok", 2, e)));
        assert!(!h.with(|t, _, e| t.requery("dlg", "missing", 5, e)));
        assert!(!h.with(|t, _, e| t.requery("other", "ok", 5, e)));
        assert!(!h.with(|t, _, e| t.requery("dlg", "ok", 4, e)));
        assert!(h.rec.msgs.is_empty());
    }
}
