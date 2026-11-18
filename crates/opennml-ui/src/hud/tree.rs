//! Window tree of the game UI with layers, dispatch and drawing order (re/game-ui 1.1, N.1, N.13; quirk O-248). The
//! tree holds only data: what a window does on an event is decided by the caller through the handler of
//! [`GameWindows::dispatch`].

use opennml_formats::engine_ini::EngineConfig;

use super::HudEvent;
use crate::coords::{self, Screen};

/// The three switches of `Engine.ini` that include or omit windows (`0x004491F0`): `IsStatusBar`, `IsMap`,
/// `IsToolBar` (each value != 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudSwitches {
    pub status_bar: bool,
    pub map: bool,
    pub tool_bar: bool,
}

impl HudSwitches {
    /// Switches from the loaded `Engine.ini` (value != 0).
    pub fn from_config(config: &EngineConfig) -> Self {
        Self {
            status_bar: config.is_status_bar != 0,
            map: config.is_map != 0,
            tool_bar: config.is_tool_bar != 0,
        }
    }
}

impl Default for HudSwitches {
    /// Default of `Engine.ini` (`FUN_00494FD0`): all three on.
    fn default() -> Self {
        Self {
            status_bar: true,
            map: true,
            tool_bar: true,
        }
    }
}

/// Kind of a game window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Base window `GameApp+0xD5E8`, layer 4; receives the input (`0x004B90BD`).
    Base,
    /// Root `0x004491F0`, layer 3.
    Root,
    /// Status bar `0x00452720`, layer 2.
    StatusBar,
    /// Minimap window `0x0044A710`, layer 2.
    Minimap,
    /// Command bar `0x00456630`, layer 2.
    Toolbar,
    /// Cinemascope/dialog area `0x004483D0`, layer 2, always present.
    Cinemascope,
    /// Tooltip window `0x004592E0`, layer 1, only with the command bar.
    Tooltip,
}

impl WindowKind {
    /// Name in the report of the control channel (`ui_state.game.hud.windows[].kind`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Root => "root",
            Self::StatusBar => "status_bar",
            Self::Minimap => "minimap",
            Self::Toolbar => "toolbar",
            Self::Cinemascope => "cinemascope",
            Self::Tooltip => "tooltip",
        }
    }
}

/// ID of a window in the tree (index in creation order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub usize);

/// VTable slot that the dispatch calls: 3 event, 2 mouse (re/game-ui 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Slot 3.
    Event,
    /// Slot 2.
    Mouse,
}

/// Position of a raw mouse kind (1-5, `0x12`, `0x13`, `0x2D`, `0x2E`) in UI pixels: the first two words
/// (`HudEvent::args = [x, y]`); `None` for other kinds or without two words.
pub fn mouse_point(ev: &HudEvent) -> Option<[i32; 2]> {
    use super::HudEventKind as K;
    let raw = matches!(
        ev.kind,
        K::MouseMove
            | K::LeftUp
            | K::LeftDown
            | K::RightUp
            | K::RightDown
            | K::DoubleRight
            | K::DoubleLeft
            | K::WheelUp
            | K::WheelDown
    );
    match ev.args.as_slice() {
        [x, y, ..] if raw => Some([*x, *y]),
        _ => None,
    }
}

/// Slot on which the minimap evaluates an event of kind `kind`: right press, release and double (5, 4, `0x12`) as
/// well as `0x22` on slot 3 (`0x0044A630`), everything else (move, left, left double, `0x19`, `0x1D`) on slot 2
/// (`0x0044A4C0`); re/game-ui D5.1, section "Input".
pub fn minimap_slot(kind: super::HudEventKind) -> Slot {
    use super::HudEventKind as K;
    match kind {
        K::RightUp | K::RightDown | K::DoubleRight | K::MinimapButtons => Slot::Event,
        _ => Slot::Mouse,
    }
}

/// Layers of the window tree (O-248).
const LAYER_BASE: u8 = 4;
const LAYER_ROOT: u8 = 3;
const LAYER_BAR: u8 = 2;
const LAYER_TOOLTIP: u8 = 1;

#[derive(Debug, Clone)]
struct Node {
    kind: WindowKind,
    rect: [i32; 4],
    layer: u8,
    visible: bool,
    children: Vec<WindowId>,
}

/// The window tree of the game view: base (4) -> root (3) -> status bar, minimap, bar, cinemascope (2) and tooltip
/// (1).
#[derive(Debug, Clone)]
pub struct GameWindows {
    nodes: Vec<Node>,
}

impl GameWindows {
    /// Creates the windows according to the switches (`0x004491F0`): base -> root -> (status bar, minimap, bar,
    /// cinemascope) -> tooltip. Cinemascope is always created, the tooltip only with `tool_bar`. Rectangles: base,
    /// root, cinemascope and tooltip the whole screen, the others as in [`coords`] (fixed pixels, O-34).
    pub fn new(screen: Screen, switches: HudSwitches, status_size: i32) -> Self {
        let full = [0, 0, screen.width, screen.height];
        let mut windows = Self { nodes: Vec::new() };
        let base = windows.add(None, WindowKind::Base, full, LAYER_BASE);
        let root = windows.add(Some(base), WindowKind::Root, full, LAYER_ROOT);
        if switches.status_bar {
            let r = coords::status_rect(screen, status_size);
            windows.add(Some(root), WindowKind::StatusBar, r, LAYER_BAR);
        }
        if switches.map {
            let r = coords::minimap_rect(screen);
            windows.add(Some(root), WindowKind::Minimap, r, LAYER_BAR);
        }
        if switches.tool_bar {
            let r = coords::toolbar_rect(screen);
            windows.add(Some(root), WindowKind::Toolbar, r, LAYER_BAR);
        }
        windows.add(Some(root), WindowKind::Cinemascope, full, LAYER_BAR);
        if switches.tool_bar {
            windows.add(Some(root), WindowKind::Tooltip, full, LAYER_TOOLTIP);
        }
        windows
    }

    fn add(
        &mut self,
        parent: Option<WindowId>,
        kind: WindowKind,
        rect: [i32; 4],
        layer: u8,
    ) -> WindowId {
        let id = WindowId(self.nodes.len());
        self.nodes.push(Node {
            kind,
            rect,
            layer,
            visible: true,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            self.nodes[p.0].children.push(id);
        }
        id
    }

    /// All windows in creation order.
    pub fn ids(&self) -> impl Iterator<Item = WindowId> + use<> {
        (0..self.nodes.len()).map(WindowId)
    }

    /// The first window of the kind (each kind occurs at most once).
    pub fn find(&self, kind: WindowKind) -> Option<WindowId> {
        self.nodes.iter().position(|n| n.kind == kind).map(WindowId)
    }

    /// Kind of the window.
    pub fn kind(&self, id: WindowId) -> WindowKind {
        self.nodes[id.0].kind
    }

    /// Rectangle `[left, top, right, bottom]` in screen pixels.
    pub fn rect(&self, id: WindowId) -> [i32; 4] {
        self.nodes[id.0].rect
    }

    /// Layer (4 base ... 1 tooltip).
    pub fn layer(&self, id: WindowId) -> u8 {
        self.nodes[id.0].layer
    }

    /// Visibility (`+0x1C`).
    pub fn visible(&self, id: WindowId) -> bool {
        self.nodes[id.0].visible
    }

    /// Sets the visibility (`+0x1C`). Children of an invisible window neither receive events nor are they drawn.
    pub fn set_visible(&mut self, id: WindowId, on: bool) {
        self.nodes[id.0].visible = on;
    }

    /// Children in list order.
    pub fn children(&self, id: WindowId) -> &[WindowId] {
        &self.nodes[id.0].children
    }

    /// Dispatches an event starting at the base window (`0x004528F0`, re/game-ui N.1; the input enters with layer 4,
    /// `0x004B90BD`). Per node:
    /// 1. If its layer is not `layer`, the result is false.
    /// 2. First slot 3 (event).
    /// 3. Only if visible: the children per layer 0...4, within the layer in list order.
    /// 4. Last slot 2 (mouse).
    ///
    /// Slots 3 and 2 also run for an invisible node. The first true of the handler ends the dispatch.
    pub fn dispatch(
        &self,
        layer: u8,
        ev: &HudEvent,
        handler: &mut dyn FnMut(WindowId, Slot, &HudEvent) -> bool,
    ) -> bool {
        self.dispatch_node(WindowId(0), layer, ev, handler)
    }

    fn dispatch_node(
        &self,
        id: WindowId,
        layer: u8,
        ev: &HudEvent,
        handler: &mut dyn FnMut(WindowId, Slot, &HudEvent) -> bool,
    ) -> bool {
        let node = &self.nodes[id.0];
        if node.layer != layer {
            return false;
        }
        if handler(id, Slot::Event, ev) {
            return true;
        }
        if node.visible {
            for l in 0..=LAYER_BASE {
                for &child in &node.children {
                    if self.dispatch_node(child, l, ev, handler) {
                        return true;
                    }
                }
            }
        }
        handler(id, Slot::Mouse, ev)
    }

    /// Like [`GameWindows::dispatch`] with the content of the nodes and the minimap:
    /// - **Minimap:** `minimap(event)` on the slot from [`minimap_slot`] (right press/release/double on slot 3
    ///   `0x0044A630`, move and left on slot 2 `0x0044A4C0`), the other slot returns false. What it does not take
    ///   goes on to the bar in tree order: the minimap is the earlier child of the root, so in the 16-px overlap at
    ///   800 x 600 whatever it takes applies (diamond, rectangle per re/game-ui D5.1).
    /// - **Corner buttons of the minimap:** children of the minimap node (layer 1), so after slot 3 and before slot 2
    ///   of the minimap and only for a visible node: `minimap_buttons(event)`, usually
    ///   [`crate::hud::toolbar::minimap_buttons::MinimapButtons::mouse`]. The tree does not keep them as nodes of
    ///   their own.
    /// - **Command bar:** `toolbar(slot, event)`, usually [`crate::hud::toolbar::Toolbar::handle`].
    /// - all other nodes: `others`.
    ///
    /// False means as with `dispatch`: nobody took it, the input belongs to the world.
    pub fn dispatch_with_toolbar(
        &self,
        layer: u8,
        ev: &HudEvent,
        minimap: &mut dyn FnMut(&HudEvent) -> bool,
        minimap_buttons: &mut dyn FnMut(&HudEvent) -> bool,
        toolbar: &mut dyn FnMut(Slot, &HudEvent) -> bool,
        others: &mut dyn FnMut(WindowId, Slot, &HudEvent) -> bool,
    ) -> bool {
        self.dispatch(layer, ev, &mut |id, slot, e| match self.kind(id) {
            // Slot 3, then the children (they follow slot 3 in `dispatch_node`), then slot 2.
            WindowKind::Minimap => {
                (slot == minimap_slot(e.kind) && minimap(e))
                    || (slot == Slot::Event && self.visible(id) && minimap_buttons(e))
            }
            WindowKind::Toolbar => toolbar(slot, e),
            _ => others(id, slot, e),
        })
    }

    /// Order in which drawing happens, starting at the base window with layer 4 (`0x004529A0`): a node is only
    /// visited if it is visible and its layer fits; then itself, afterwards its children per layer 4 -> 0. Children
    /// of invisible nodes are missing. The tooltip (layer 1) thus comes last and lies on top.
    pub fn draw_order(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.draw_node(WindowId(0), LAYER_BASE, &mut out);
        out
    }

    fn draw_node(&self, id: WindowId, layer: u8, out: &mut Vec<WindowId>) {
        let node = &self.nodes[id.0];
        if !node.visible || node.layer != layer {
            return;
        }
        out.push(id);
        for l in (0..=LAYER_BASE).rev() {
            for &child in &node.children {
                self.draw_node(child, l, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::HudEventKind;

    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    fn all_windows() -> GameWindows {
        GameWindows::new(SCREEN, HudSwitches::default(), 26)
    }

    fn kinds(w: &GameWindows, ids: &[WindowId]) -> Vec<WindowKind> {
        ids.iter().map(|&i| w.kind(i)).collect()
    }

    /// Records who gets a turn in which slot; `stop_at` returns true there.
    fn record(
        w: &GameWindows,
        stop_at: Option<(WindowKind, Slot)>,
    ) -> (bool, Vec<(WindowKind, Slot)>) {
        let mut log = Vec::new();
        let ev = HudEvent::new(HudEventKind::Frame);
        let hit = w.dispatch(4, &ev, &mut |id, slot, _| {
            log.push((w.kind(id), slot));
            stop_at == Some((w.kind(id), slot))
        });
        (hit, log)
    }

    #[test]
    fn mouse_point_reads_x_y_of_raw_mouse_only() {
        // Raw mouse `args = [x, y]` in UI pixels.
        let at = |k, a: &[i32]| mouse_point(&HudEvent::with_args(k, a));
        assert_eq!(at(HudEventKind::LeftDown, &[3, 4]), Some([3, 4]));
        assert_eq!(at(HudEventKind::DoubleRight, &[3, 4, 9]), Some([3, 4]));
        assert_eq!(at(HudEventKind::MouseMove, &[3]), None);
        assert_eq!(at(HudEventKind::Rebuild, &[3, 4]), None);
        assert_eq!(at(HudEventKind::Button, &[300, 4]), None);
    }

    // QUIRK O-34, O-248: kept
    #[test]
    fn windows_have_layers_4_3_2_1() {
        let w = all_windows();
        let layer = |k| w.layer(w.find(k).unwrap());
        assert_eq!(layer(WindowKind::Base), 4);
        assert_eq!(layer(WindowKind::Root), 3);
        assert_eq!(layer(WindowKind::StatusBar), 2);
        assert_eq!(layer(WindowKind::Minimap), 2);
        assert_eq!(layer(WindowKind::Toolbar), 2);
        assert_eq!(layer(WindowKind::Cinemascope), 2);
        assert_eq!(layer(WindowKind::Tooltip), 1);
        // Rectangles: base, root, cinemascope, tooltip the screen; the others as in `coords`.
        let rect = |k| w.rect(w.find(k).unwrap());
        for k in [
            WindowKind::Base,
            WindowKind::Root,
            WindowKind::Cinemascope,
            WindowKind::Tooltip,
        ] {
            assert_eq!(rect(k), [0, 0, 1024, 768], "{k:?}");
        }
        assert_eq!(rect(WindowKind::Toolbar), [0, 592, 640, 768]);
        assert_eq!(rect(WindowKind::Minimap), [848, 592, 1024, 768]);
        assert_eq!(rect(WindowKind::StatusBar), [0, 0, 1024, 26]);
        // Creation order under the root: status bar, minimap, bar, cinemascope, tooltip.
        let root = w.find(WindowKind::Root).unwrap();
        assert_eq!(
            kinds(&w, w.children(root)),
            [
                WindowKind::StatusBar,
                WindowKind::Minimap,
                WindowKind::Toolbar,
                WindowKind::Cinemascope,
                WindowKind::Tooltip
            ]
        );
        assert!(w.ids().all(|i| w.visible(i)));
    }

    #[test]
    fn switches_build_only_enabled_windows() {
        let off = HudSwitches {
            status_bar: false,
            map: false,
            tool_bar: false,
        };
        let w = GameWindows::new(SCREEN, off, 26);
        assert_eq!(
            kinds(&w, &w.ids().collect::<Vec<_>>()),
            [WindowKind::Base, WindowKind::Root, WindowKind::Cinemascope]
        );
        // Only the bar: with tooltip, without status bar and minimap.
        let only_bar = HudSwitches {
            tool_bar: true,
            ..off
        };
        let w = GameWindows::new(SCREEN, only_bar, 26);
        assert_eq!(
            kinds(&w, &w.ids().collect::<Vec<_>>()),
            [
                WindowKind::Base,
                WindowKind::Root,
                WindowKind::Toolbar,
                WindowKind::Cinemascope,
                WindowKind::Tooltip
            ]
        );
        assert!(w.find(WindowKind::StatusBar).is_none());
        assert!(w.find(WindowKind::Minimap).is_none());
        // The switches come from Engine.ini (value != 0).
        let mut cfg = EngineConfig::default();
        assert_eq!(HudSwitches::from_config(&cfg), HudSwitches::default());
        cfg.is_status_bar = 0;
        cfg.is_map = 7;
        cfg.is_tool_bar = 0;
        assert_eq!(
            HudSwitches::from_config(&cfg),
            HudSwitches {
                status_bar: false,
                map: true,
                tool_bar: false
            }
        );
    }

    // QUIRK O-248: kept
    #[test]
    fn dispatch_runs_event_before_children_and_mouse_after() {
        let off = HudSwitches {
            status_bar: false,
            map: false,
            tool_bar: false,
        };
        let w = GameWindows::new(SCREEN, off, 26);
        let (hit, log) = record(&w, None);
        assert!(!hit);
        assert_eq!(
            log,
            [
                (WindowKind::Base, Slot::Event),
                (WindowKind::Root, Slot::Event),
                (WindowKind::Cinemascope, Slot::Event),
                (WindowKind::Cinemascope, Slot::Mouse),
                (WindowKind::Root, Slot::Mouse),
                (WindowKind::Base, Slot::Mouse),
            ]
        );
        // The first true ends the dispatch: slot 3 of the root takes it, children and mouse stay out.
        let (hit, log) = record(&w, Some((WindowKind::Root, Slot::Event)));
        assert!(hit);
        assert_eq!(
            log,
            [
                (WindowKind::Base, Slot::Event),
                (WindowKind::Root, Slot::Event)
            ]
        );
        // Slot 2 of a child takes it: slot 2 of the parent does not come up any more.
        let (hit, log) = record(&w, Some((WindowKind::Cinemascope, Slot::Mouse)));
        assert!(hit);
        assert_eq!(log.last(), Some(&(WindowKind::Cinemascope, Slot::Mouse)));
        assert_eq!(log.len(), 4);
        // A layer other than that of the base window is rejected without anything running.
        let mut called = false;
        let ev = HudEvent::new(HudEventKind::Frame);
        assert!(!w.dispatch(3, &ev, &mut |_, _, _| {
            called = true;
            true
        }));
        assert!(!called);
    }

    // QUIRK O-248: kept
    #[test]
    fn invisible_node_still_gets_event_and_mouse_but_not_its_children() {
        let mut w = all_windows();
        let root = w.find(WindowKind::Root).unwrap();
        w.set_visible(root, false);
        let (hit, log) = record(&w, None);
        assert!(!hit);
        assert_eq!(
            log,
            [
                (WindowKind::Base, Slot::Event),
                (WindowKind::Root, Slot::Event),
                (WindowKind::Root, Slot::Mouse),
                (WindowKind::Base, Slot::Mouse),
            ]
        );
        // An invisible leaf still gets both slots.
        w.set_visible(root, true);
        let bar = w.find(WindowKind::Toolbar).unwrap();
        w.set_visible(bar, false);
        let (_, log) = record(&w, None);
        assert!(log.contains(&(WindowKind::Toolbar, Slot::Event)));
        assert!(log.contains(&(WindowKind::Toolbar, Slot::Mouse)));
    }

    // QUIRK O-248: kept
    #[test]
    fn tooltip_gets_events_before_the_bars() {
        let w = all_windows();
        let (_, log) = record(&w, None);
        let pos = |k, s| log.iter().position(|e| *e == (k, s)).unwrap();
        let tip = pos(WindowKind::Tooltip, Slot::Event);
        for k in [
            WindowKind::StatusBar,
            WindowKind::Minimap,
            WindowKind::Toolbar,
            WindowKind::Cinemascope,
        ] {
            assert!(tip < pos(k, Slot::Event), "{k:?}");
        }
        // And the bars in list order.
        assert!(pos(WindowKind::StatusBar, Slot::Event) < pos(WindowKind::Minimap, Slot::Event));
        assert!(pos(WindowKind::Minimap, Slot::Event) < pos(WindowKind::Toolbar, Slot::Event));
        // If the tooltip takes the event, the bars do not see it.
        let (hit, log) = record(&w, Some((WindowKind::Tooltip, Slot::Event)));
        assert!(hit);
        assert!(!log.iter().any(|(k, _)| *k == WindowKind::Toolbar));
    }

    // QUIRK O-248: kept
    #[test]
    fn draw_order_skips_children_of_invisible_nodes_and_draws_tooltip_last() {
        let mut w = all_windows();
        assert_eq!(
            kinds(&w, &w.draw_order()),
            [
                WindowKind::Base,
                WindowKind::Root,
                WindowKind::StatusBar,
                WindowKind::Minimap,
                WindowKind::Toolbar,
                WindowKind::Cinemascope,
                WindowKind::Tooltip,
            ]
        );
        // An invisible leaf is missing, the others stay.
        let map = w.find(WindowKind::Minimap).unwrap();
        w.set_visible(map, false);
        assert_eq!(
            kinds(&w, &w.draw_order()),
            [
                WindowKind::Base,
                WindowKind::Root,
                WindowKind::StatusBar,
                WindowKind::Toolbar,
                WindowKind::Cinemascope,
                WindowKind::Tooltip,
            ]
        );
        // An invisible root takes all children with it.
        let root = w.find(WindowKind::Root).unwrap();
        w.set_visible(root, false);
        assert_eq!(kinds(&w, &w.draw_order()), [WindowKind::Base]);
        // Invisible base: nothing.
        w.set_visible(WindowId(0), false);
        assert!(w.draw_order().is_empty());
    }

    /// The corner buttons are children of the minimap - after its slot 3, before its slot 2 and before the bar; only
    /// for a visible node.
    // QUIRK O-388: kept
    #[test]
    fn minimap_buttons_come_between_slot_3_and_slot_2() {
        let mut w = all_windows();
        let log = std::cell::RefCell::new(Vec::new());
        let run = |w: &GameWindows, kind: HudEventKind, buttons_take: bool| {
            log.borrow_mut().clear();
            let ev = HudEvent::with_args(kind, &[900, 700]);
            let taken = w.dispatch_with_toolbar(
                4,
                &ev,
                &mut |e| {
                    log.borrow_mut()
                        .push(format!("map {:?}", minimap_slot(e.kind)));
                    false
                },
                &mut |_| {
                    log.borrow_mut().push("buttons".to_string());
                    buttons_take
                },
                &mut |slot, _| {
                    log.borrow_mut().push(format!("bar {slot:?}"));
                    false
                },
                &mut |_, _, _| false,
            );
            (taken, log.borrow().clone())
        };
        let (taken, order) = run(&w, HudEventKind::RightDown, false);
        assert!(!taken);
        assert_eq!(order[..2], ["map Event", "buttons"]);
        let (_, order) = run(&w, HudEventKind::LeftDown, false);
        assert_eq!(order[..2], ["buttons", "map Mouse"]);
        let (taken, order) = run(&w, HudEventKind::LeftDown, true);
        assert!(taken);
        assert_eq!(order, ["buttons"], "taken: neither slot 2 nor bar");
        let map = w.find(WindowKind::Minimap).unwrap();
        w.set_visible(map, false);
        let (_, order) = run(&w, HudEventKind::LeftDown, true);
        assert!(!order.contains(&"buttons".to_string()), "{order:?}");
    }
}
