//! Command bar of the game UI: data, button object, set building, bar window and cursors. The module does not know
//! `nml-sim`; the client hands in a snapshot of the selection each frame.
//!
//! Basis: re/game-ui 2.1 and N.1-N.5, N.14. This stage contains the data ([`data`]), the button object ([`button`]),
//! the snapshot of the selection ([`model`]), the set building ([`set`]), the bar window with events, click paths and
//! drawing ([`window`]) and the cursors for building, walls, target and defense radius ([`cursor`]). In addition the
//! corner buttons of the minimap ([`minimap_buttons`]): the same button object, children of the minimap.

pub mod button;
pub mod cursor;
pub mod data;
pub mod minimap_buttons;
pub mod model;
pub mod set;
pub mod window;

pub use button::{ToolbarOut, ToolbarSound};
pub use cursor::{
    BuildCursor, Cursor, CursorFrame, DefenseCursor, FortCursor, FortPhase, TargetCursor, WallPart,
    ring,
};
pub use model::{HuntTarget, Production, SelectedObject, ToolbarKeys, ToolbarModel};
pub use set::{ButtonSet, DynamicKind, MenuChoice, ObjectRow};
pub use window::{Toolbar, ToolbarEvent, ToolbarIntent};
