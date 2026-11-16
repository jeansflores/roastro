//! Pure roast calculations and the roast's own vocabulary. This crate knows
//! nothing about Modbus, terminals or wall-clock time: callers pass in the
//! roast clock.

mod event;
mod metrics;
mod ror;

pub use event::{EventKind, Source, UnknownName};
pub use metrics::{Metrics, Phase, PhaseKind, largest_fall, metrics, ror_series};
pub use ror::RoR;
