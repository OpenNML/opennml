//! UI core of "No Man's Land": screen coordinates, draw commands, images, texts, fonts, text layout, image
//! quads, `.bkg` frames, desktop image, frame ornaments, the static drawing of whole dialogs and the menu run time
//! (widget tree, dispatcher, elements, drawing from the tree with fly-in and image table, dialog stack, command
//! queue and `MenuApp`) with video playback as data.
//!
//! The crate knows neither `wgpu` nor `winit` nor `egui`: it only produces data (`DrawCmd` lists) that a renderer
//! executes. The same inputs yield the same command lists (determinism).
//!
//! Basis: re/ui-rendering 2.1 (coordinates), 3 (fonts, 3.8 text layout), re/menu-system 8.4 (drawing).

pub mod app;
pub mod assets;
pub mod atlas_gen;
pub mod bkg_paint;
pub mod campaign;
pub mod coords;
pub mod desktop;
pub mod dialog;
pub mod draft;
pub mod draw;
pub mod fonts;
pub mod geom;
pub mod hud;
pub mod input;
pub mod keys;
pub mod loading;
pub mod menu;
pub mod msg;
pub mod own_texts;
pub mod paint;
pub mod profiles;
pub mod saves;
pub mod screens;
pub mod system_language;
pub mod text;
pub mod tree;
pub mod version_line;
pub mod video;
pub mod widgets;

#[cfg(test)]
pub(crate) mod testutil;
