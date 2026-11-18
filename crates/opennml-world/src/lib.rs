//! World model of No Man's Land: assembles the game files and computes the derived data of the
//! original (terrain, chunk textures, object matrices). Independent of graphics and window;
//! coordinates in the original system (left-handed, y up).

pub mod anim;
pub mod assets;
pub mod camera;
pub mod chunks;
pub mod classes;
pub mod environment;
pub mod level;
pub mod markers;
pub mod math;
pub mod rand;
pub mod shadows;
pub mod terrain;
pub mod visibility;
pub mod weather;
