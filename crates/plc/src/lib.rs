//! Talks to the roaster PLC over Modbus TCP by point name. Callers never see
//! addresses: the register map translates names into Modbus tables.
//!
//! Writes reach only this machine unless [`Config::allow_write`] is set, and
//! the check happens before anything is dialled.

mod client;
mod poller;

pub use client::{Client, Config, Error, Reading, is_loopback};
pub use poller::{Event, Poller, Reader, backoff};
pub use tokio_modbus::ExceptionCode;
