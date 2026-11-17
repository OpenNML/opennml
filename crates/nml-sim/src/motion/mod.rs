//! Movement in the fixed point of the original (re/motion): speeds (`SpeedInfo`) and
//! a movement step (`Motion_StepTowardsTarget`) with the trajectory point.

pub mod speed;
pub mod step;

pub use speed::SpeedInfo;
pub use step::{StepWorld, TrajPoint, step, turn_towards};

/// "No direction" (`Fixed_FromInt(−200)`, `0x00671FD8`).
pub const NO_DIRECTION: i32 = -200 << 12;
