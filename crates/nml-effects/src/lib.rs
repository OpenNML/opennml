//! Effect run time of No Man's Land (re/effects): templates from `.pef`, instances with
//! particle emitters and billboard layers, advancing on the virtual clock. Pure CPU -
//! without `wgpu`/`winit`; output is vertex lists (FVF `0x1C2`) and draw commands that
//! `nml-render` draws.

pub mod ambient;
pub mod attached;
pub mod billboard;
pub mod browser;
pub mod effect;
pub mod fire;
pub mod handle;
pub mod library;
pub mod marks;
pub mod object;
pub mod output;
pub mod particles;
pub mod triggers;
pub mod world;

/// Seed of the random number generator of the effects.
pub const EFFECTS_SEED: u32 = 2;
