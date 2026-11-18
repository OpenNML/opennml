//! Game host of a match: simulation and control channel queries (`sim_host`, `sim_control`), map view
//! (`map`) with effects (`projectiles`, `building_fires`, `heal_effects`, `train_effects`,
//! `ability_effects`, `world_effects`; state for the control channel: `world_display`), camera input
//! (`input`), selection (`selection`), economy and command bar without drawing (`economy`,
//! `unit_bar`; snapshot of the match for the bar: `toolbar_host`), messages (`notices`), voices (`voices`),
//! markers, fog and paths (`markers`, `fog_view`, `fog_memory`, `path_view`), font of the map view
//! (`overlay`), speech output of the dialog lines (`speech_audio`), the calculation of the combat display
//! (`combat_calc`), the presentation of the scripts (`presentation`), the view settings (`options`), the hotkeys
//! (`hotkeys`), the control groups (`control_groups`), the pause (`pause`), the mouse pointer (`pointer`), the
//! shared query of the input under lock and cinemascope (`input_gate`), the minimap (`minimap`), the masks of
//! diplomacy (`diplomacy`) and the turn driver of the network match (`net_driver`). Without `egui`: the windows
//! and drawers using egui stay in the viewer.
//!
//! Test kits (`testworld`, `sim_host::testkit`, `economy::testkit` ...) are visible in the crate's own tests
//! and with the feature `testkit` (for the viewer's tests).

pub mod ability_effects;
pub mod blood;
pub mod building_fires;
pub mod chat;
pub mod combat_calc;
pub mod control_groups;
pub mod destruction_marks;
pub mod diplomacy;
pub mod economy;
pub mod fog_memory;
pub mod fog_view;
pub mod formation_order;
pub mod grid;
pub mod heal_effects;
pub mod hotkeys;
pub mod hud;
pub mod input;
pub mod input_gate;
pub mod map;
pub mod markers;
pub mod minimap;
pub mod movement_effects;
pub mod net_driver;
pub mod notices;
pub mod options;
pub mod overlay;
pub mod path_view;
pub mod pause;
pub mod pointer;
pub mod presentation;
pub mod projectiles;
pub mod rally_flag;
pub mod script_camera;
pub mod selection;
pub mod sim_control;
pub mod sim_host;
pub mod speech_audio;
#[cfg(any(test, feature = "testkit"))]
pub mod testworld;
pub mod texts;
pub mod toolbar_host;
pub mod train_effects;
pub mod unit_bar;
pub mod voices;
pub mod world_display;
pub mod world_effects;
