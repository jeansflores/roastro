use std::future::{Ready, ready};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use regmap::{Kind, Map};
use tokio_modbus::server::Service;
use tokio_modbus::{ExceptionCode, Request, Response, SlaveRequest};

use crate::Sim;

/// Serves a [`Sim`] over Modbus TCP, translating register reads and writes
/// into [`Sim::get`] and [`Sim::set`] through a register map.
///
/// Make one per connection with [`ModbusService::for_peer`] so writes are
/// logged with the client's address.
#[derive(Debug, Clone)]
pub struct ModbusService {
    map: Arc<Map>,
    sim: Arc<Mutex<Sim>>,
    peer: Option<SocketAddr>,
}

impl ModbusService {
    /// Exposes `sim` at the addresses in `map`.
    #[must_use]
    pub fn new(map: Arc<Map>, sim: Arc<Mutex<Sim>>) -> Self {
        Self {
            map,
            sim,
            peer: None,
        }
    }

    /// The same service, tagged with the client it serves.
    #[must_use]
    pub fn for_peer(&self, peer: SocketAddr) -> Self {
        Self {
            peer: Some(peer),
            ..self.clone()
        }
    }

    fn handle(&self, req: Request<'_>) -> Result<Response, ExceptionCode> {
        // A poisoned lock only means another request panicked; the model is still usable.
        let mut sim = self.sim.lock().unwrap_or_else(PoisonError::into_inner);
        match req {
            Request::ReadCoils(addr, n) => Ok(Response::ReadCoils(self.read(
                &mut sim,
                Kind::Coil,
                addr,
                n,
                |v| v != 0.0,
            ))),
            Request::ReadDiscreteInputs(addr, n) => Ok(Response::ReadDiscreteInputs(self.read(
                &mut sim,
                Kind::Discrete,
                addr,
                n,
                |v| v != 0.0,
            ))),
            Request::ReadHoldingRegisters(addr, n) => Ok(Response::ReadHoldingRegisters(
                self.read_words(&mut sim, Kind::Holding, addr, n),
            )),
            Request::ReadInputRegisters(addr, n) => Ok(Response::ReadInputRegisters(
                self.read_words(&mut sim, Kind::Input, addr, n),
            )),
            Request::WriteSingleCoil(addr, on) => {
                self.write(&mut sim, Kind::Coil, addr, |_| if on { 1.0 } else { 0.0 })?;
                Ok(Response::WriteSingleCoil(addr, on))
            }
            Request::WriteMultipleCoils(addr, values) => {
                for (addr, &on) in addresses(addr, values.len())?.zip(values.iter()) {
                    self.write(&mut sim, Kind::Coil, addr, |_| if on { 1.0 } else { 0.0 })?;
                }
                Ok(Response::WriteMultipleCoils(addr, quantity(values.len())?))
            }
            Request::WriteSingleRegister(addr, word) => {
                self.write(&mut sim, Kind::Holding, addr, |p| p.decode(word))?;
                Ok(Response::WriteSingleRegister(addr, word))
            }
            Request::WriteMultipleRegisters(addr, words) => {
                for (addr, &word) in addresses(addr, words.len())?.zip(words.iter()) {
                    self.write(&mut sim, Kind::Holding, addr, |p| p.decode(word))?;
                }
                Ok(Response::WriteMultipleRegisters(
                    addr,
                    quantity(words.len())?,
                ))
            }
            _ => Err(ExceptionCode::IllegalFunction),
        }
    }

    /// Reads `n` points of `kind` from `addr`; gaps in the map read as zero.
    fn read<T: Default>(
        &self,
        sim: &mut Sim,
        kind: Kind,
        addr: u16,
        n: u16,
        convert: impl Fn(f64) -> T,
    ) -> Vec<T> {
        (0..n)
            .map(|i| {
                addr.checked_add(i)
                    .and_then(|a| self.map.at(kind, a))
                    .and_then(|p| sim.get(&p.name).ok())
                    .map_or_else(T::default, &convert)
            })
            .collect()
    }

    fn read_words(&self, sim: &mut Sim, kind: Kind, addr: u16, n: u16) -> Vec<u16> {
        (0..n)
            .map(|i| {
                addr.checked_add(i)
                    .and_then(|a| self.map.at(kind, a))
                    .and_then(|p| sim.get(&p.name).ok().map(|v| p.encode(v)))
                    .unwrap_or(0)
            })
            .collect()
    }

    fn write(
        &self,
        sim: &mut Sim,
        kind: Kind,
        addr: u16,
        value: impl FnOnce(&regmap::Point) -> f64,
    ) -> Result<(), ExceptionCode> {
        let p = self
            .map
            .at(kind, addr)
            .filter(|p| p.writable)
            .ok_or(ExceptionCode::IllegalDataAddress)?;
        let v = value(p);
        let peer = self.peer.map_or_else(|| "?".to_owned(), |a| a.to_string());
        sim.set(&p.name, v).map_err(|err| {
            tracing::warn!(point = %p.name, %err, "write refused");
            ExceptionCode::ServerDeviceFailure
        })?;
        tracing::info!(client = %peer, point = %p.name, value = v, "write");
        Ok(())
    }
}

/// The addresses of a multi-write, refusing one that runs past the address space.
fn addresses(start: u16, len: usize) -> Result<impl Iterator<Item = u16>, ExceptionCode> {
    let n = quantity(len)?;
    start
        .checked_add(n.saturating_sub(1))
        .ok_or(ExceptionCode::IllegalDataAddress)?;
    Ok((0..n).map(move |i| start + i))
}

fn quantity(len: usize) -> Result<u16, ExceptionCode> {
    u16::try_from(len).map_err(|_| ExceptionCode::IllegalDataValue)
}

impl Service for ModbusService {
    type Request = SlaveRequest<'static>;
    type Response = Response;
    type Exception = ExceptionCode;
    type Future = Ready<Result<Response, ExceptionCode>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        let unit = self.map.unit_id();
        // 0 is broadcast and 255 is what most TCP clients send; anything else must match.
        if req.slave != unit && req.slave != 0 && req.slave != 255 {
            return ready(Err(ExceptionCode::GatewayTargetDevice));
        }
        ready(self.handle(req.request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fault;

    fn service() -> (ModbusService, Arc<Mutex<Sim>>) {
        let map = Arc::new(
            Map::load(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../configs/regmap.json"
            ))
            .unwrap(),
        );
        let sim = Arc::new(Mutex::new(Sim::new(1)));
        (ModbusService::new(map, sim.clone()), sim)
    }

    fn call(
        svc: &ModbusService,
        slave: u8,
        request: Request<'static>,
    ) -> Result<Response, ExceptionCode> {
        svc.call(SlaveRequest { slave, request }).into_inner()
    }

    #[test]
    fn reads_the_model_through_the_map() {
        let (svc, _) = service();
        let Ok(Response::ReadHoldingRegisters(words)) =
            call(&svc, 1, Request::ReadHoldingRegisters(0, 8))
        else {
            panic!("want holding registers");
        };
        // bt: tenths of °C on a cold machine.
        assert!((240..=260).contains(&words[0]), "bt raw = {}", words[0]);
        assert_eq!(words[4], 80, "drum % at rest");
    }

    #[test]
    fn writes_reach_the_model() {
        let (svc, sim) = service();
        call(&svc, 1, Request::WriteSingleCoil(1, true)).unwrap(); // drum_on
        call(&svc, 1, Request::WriteSingleRegister(2, 60)).unwrap(); // gas_cmd
        let sn = sim.lock().unwrap().snapshot();
        assert!(sn.drum_on, "drum_on");
        assert!(
            (sn.gas_cmd - 60.0).abs() < f64::EPSILON,
            "gas_cmd = {}",
            sn.gas_cmd
        );
    }

    #[test]
    fn refuses_what_the_plc_would_refuse() {
        let (svc, sim) = service();
        assert_eq!(
            call(&svc, 7, Request::ReadCoils(0, 1)),
            Err(ExceptionCode::GatewayTargetDevice),
            "wrong unit"
        );
        assert_eq!(
            call(&svc, 1, Request::WriteSingleRegister(0, 1000)),
            Err(ExceptionCode::IllegalDataAddress),
            "bt is read only"
        );
        sim.lock().unwrap().set_fault(Fault::Emergency, true);
        assert_eq!(
            call(&svc, 1, Request::WriteSingleCoil(0, true)),
            Err(ExceptionCode::ServerDeviceFailure),
            "burner during emergency"
        );
        assert_eq!(
            call(&svc, 1, Request::ReportServerId),
            Err(ExceptionCode::IllegalFunction)
        );
    }
}
