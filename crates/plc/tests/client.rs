//! The client against a real Modbus TCP server: a small in-memory PLC.

use std::collections::HashMap;
use std::future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use plc::{Client, Config, Error, ExceptionCode};
use regmap::Map;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_modbus::server::Service;
use tokio_modbus::server::tcp::{Server, accept_tcp_connection};
use tokio_modbus::{Request, Response};

#[derive(Default)]
struct State {
    holding: HashMap<u16, u16>,
    coils: HashMap<u16, bool>,
    /// Refuse every coil write, like a PLC during an emergency stop.
    refuse_coils: bool,
}

/// An in-memory PLC: unset registers read as zero.
#[derive(Clone, Default)]
struct FakePlc(Arc<Mutex<State>>);

impl Service for FakePlc {
    type Request = Request<'static>;
    type Response = Response;
    type Exception = ExceptionCode;
    type Future = future::Ready<Result<Response, ExceptionCode>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        let mut s = self.0.lock().unwrap();
        let words = |s: &State, lo: u16, n: u16| {
            (lo..lo + n)
                .map(|a| s.holding.get(&a).copied().unwrap_or(0))
                .collect()
        };
        let bits = |s: &State, lo: u16, n: u16| {
            (lo..lo + n)
                .map(|a| s.coils.get(&a).copied().unwrap_or(false))
                .collect()
        };
        future::ready(match req {
            Request::ReadHoldingRegisters(lo, n) => {
                Ok(Response::ReadHoldingRegisters(words(&s, lo, n)))
            }
            Request::ReadInputRegisters(_, n) => {
                Ok(Response::ReadInputRegisters(vec![0; n.into()]))
            }
            Request::ReadCoils(lo, n) => Ok(Response::ReadCoils(bits(&s, lo, n))),
            Request::ReadDiscreteInputs(_, n) => {
                Ok(Response::ReadDiscreteInputs(vec![false; n.into()]))
            }
            Request::WriteSingleCoil(_, _) if s.refuse_coils => {
                Err(ExceptionCode::ServerDeviceFailure)
            }
            Request::WriteSingleCoil(a, v) => {
                s.coils.insert(a, v);
                Ok(Response::WriteSingleCoil(a, v))
            }
            Request::WriteSingleRegister(a, w) => {
                s.holding.insert(a, w);
                Ok(Response::WriteSingleRegister(a, w))
            }
            _ => Err(ExceptionCode::IllegalFunction),
        })
    }
}

/// A running fake server; dropping `stop` (or sending on it) shuts it down
/// along with its open connections.
struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Running {
    async fn shut_down(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
    }
}

fn serve(listener: TcpListener, plc: FakePlc) -> Running {
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let on_connected = |stream, addr| {
            let plc = plc.clone();
            async move { accept_tcp_connection(stream, addr, move |_| Ok(Some(plc.clone()))) }
        };
        let abort = async {
            let _ = stopped.await;
        };
        Server::new(listener)
            .serve_until(&on_connected, |_| {}, abort)
            .await
            .unwrap();
    });
    Running { stop, task }
}

async fn start(plc: FakePlc) -> (SocketAddr, Running) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, serve(listener, plc))
}

fn map() -> Arc<Map> {
    Arc::new(
        Map::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../configs/regmap.json"
        ))
        .unwrap(),
    )
}

fn client(addr: SocketAddr) -> Client {
    let cfg = Config {
        timeout: Duration::from_millis(500),
        ..Config::new(format!("tcp://{addr}"), map())
    };
    Client::new(cfg).unwrap()
}

#[tokio::test]
async fn reads_and_writes_by_name() {
    let plc = FakePlc::default();
    plc.0.lock().unwrap().holding.insert(0, 253); // bt, tenths of °C
    let (addr, _server) = start(plc.clone()).await;
    let c = client(addr);

    let r = c.read().await.unwrap();
    assert!(
        (r.values["bt"] - 25.3).abs() < 1e-9,
        "bt = {}",
        r.values["bt"]
    );
    assert_eq!(r.values.len(), map().points().len(), "every point read");

    c.write("drum_on", 1.0).await.unwrap();
    c.write("gas_cmd", 60.0).await.unwrap();
    let s = plc.0.lock().unwrap();
    assert_eq!(
        (s.coils.get(&1), s.holding.get(&2)),
        (Some(&true), Some(&60))
    );
}

#[tokio::test]
async fn rejects_bad_writes_and_survives_a_refusal() {
    let plc = FakePlc::default();
    let (addr, _server) = start(plc.clone()).await;
    let c = client(addr);

    assert!(matches!(
        c.write("nope", 1.0).await,
        Err(Error::UnknownPoint(_))
    ));
    assert!(matches!(
        c.write("bt", 100.0).await,
        Err(Error::NotWritable(_))
    ));

    plc.0.lock().unwrap().refuse_coils = true;
    let refused = c.write("burner", 1.0).await;
    assert!(
        matches!(
            refused,
            Err(Error::Exception {
                code: ExceptionCode::ServerDeviceFailure,
                ..
            })
        ),
        "{refused:?}"
    );
    c.read()
        .await
        .expect("the connection should survive an exception");
}

#[tokio::test]
async fn reconnects_when_the_plc_comes_back() {
    let (addr, server) = start(FakePlc::default()).await;
    let c = client(addr);
    c.read().await.expect("first read");

    server.shut_down().await; // the PLC goes away (power, cable, VPN...)
    assert!(c.read().await.is_err(), "read with the server down");

    let listener = TcpListener::bind(addr).await.unwrap(); // and comes back on the same address
    let _server = serve(listener, FakePlc::default());
    c.read().await.expect("read after the server came back");
}
