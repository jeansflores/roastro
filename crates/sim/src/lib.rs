//! A simplified thermal model of the Caloratto 5 kg roaster and its PLC,
//! served over Modbus TCP so the app can be developed without the machine.

mod autopilot;
mod modbus;
mod model;
mod rng;

pub use autopilot::{Autopilot, Phase};
pub use modbus::ModbusService;
pub use model::{Error, Event, Fault, Sim, Snapshot};
