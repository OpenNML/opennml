//! Shared parts of the programs with a window (viewer, `nml-game`): virtual clock, control channel server, input
//! and held keys, monitor and mode selection, frame counter, screenshot helpers, window scaffolding (window, GPU,
//! window mode, frame pacing) and the UI layer (images, fonts, texts of the installation), the menu host
//! (`menu_host`: input, sound, display mode, pointer), the settings file (`settings`), the key codes of the
//! original (`vk`), the UI commands of the control channel (`ui_control`) and the videos (`video`: Bink files of
//! the installation with image and sound track). Also here: the game host of a match (`game`: simulation, map
//! view, effects, camera, selection, messages, voices, control channel queries; test kits with the feature
//! `testkit`), the egui-free frame loop of a match (`game_session`: loading in phases, camera, simulation,
//! selection, right-click, drawing the world) and the savegame container (`game_save`), the conversion of the UI's
//! start request into the setup of the simulation (`match_request`), the GPU-free flow of a match from the menu
//! (`game_flow`: loading screen, game menu, saving, loading) and the savegames in the folder `savegame/`
//! (`save_files`), the model and clock of the 3D mouse pointer (`pointer_model`). Tests share a small installation
//! (`testutil`).
//!
//! Split off from `apps/nml-viewer` without a change in behavior; without `egui`. The UI layer
//! ([`ui_layer::UiLayer`]) is GPU-free, its renderer (`nml_render::UiRenderer`) belongs to the host and is passed
//! in when drawing.

pub mod app_icon;
pub mod clock;
pub mod control;
pub mod display;
pub mod frames;
pub mod game;
pub mod game_flow;
pub mod game_save;
pub mod game_session;
pub mod keys;
pub mod match_request;
pub mod menu_host;
pub mod net_session;
pub mod pointer_model;
pub mod random_map_state;
pub mod replay_file;
pub mod save_files;
pub mod settings;
pub mod snapshot;
#[cfg(test)]
mod testutil;
pub mod ui_control;
pub mod ui_layer;
pub mod video;
pub mod vk;
pub mod window;
