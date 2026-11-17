//! No Man's Land renderer on wgpu (Vulkan/DX12): terrain, water and objects like the Direct3D 8
//! renderer of the original (re/renderer), plus the 2D pass of the UI (`ui::UiRenderer`: draw
//! commands from `nml-ui`, also for the font and the debug overlay of the viewer).

pub mod capture;
pub mod effects;
pub mod gamma;
mod gpu;
pub mod markers;
pub mod marks;
pub mod objects;
pub mod pick;
pub mod pointer;
pub mod scene;
pub mod shadows;
pub mod shared;
pub mod terrain;
pub mod texture;
pub mod ui;
pub mod water;
pub mod weather;

pub use capture::{CAPTURE_FORMAT, render_to_image};
pub use gamma::{GammaPass, gamma_ramp};
pub use gpu::{BackendChoice, FrameOutcome, Gpu, backend_attempts, headless_device, surface_size};
pub use markers::{GroundMarker, MarkerStats, WorldDecal};
pub use marks::GroundMarkStats;
pub use pointer::{PointerPiece, PointerRenderer, PointerScene};
pub use scene::{
    EntryLook, FogSettings, MemoryPiece, ModelEntry, PreviewPiece, SceneRenderer, ViewParams,
};
pub use shadows::{EntryShadow, ShadowStats};
pub use ui::UiRenderer;
