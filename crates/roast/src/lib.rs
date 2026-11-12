//! Pure roast calculations and the roast's own vocabulary. This crate knows
//! nothing about Modbus, terminals or wall-clock time: callers pass in the
//! roast clock.

mod event;
mod ror;

pub use event::{EventKind, Source, UnknownName};
pub use ror::RoR;
