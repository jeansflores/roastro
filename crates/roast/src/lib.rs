//! Pure roast calculations. This crate knows nothing about Modbus,
//! terminals or wall-clock time: callers pass in the roast clock.

mod ror;

pub use ror::RoR;
