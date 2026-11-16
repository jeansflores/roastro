//! The PLC simulator inside the app, for demos and development: one
//! executable shows a live roast with nothing else installed. Off unless
//! `--simulator` is passed; never used against the machine.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use regmap::Map;

/// Serves the simulator on `127.0.0.1:port` with its autopilot on and the
/// warm-up skipped. Must run inside a Tokio runtime.
pub async fn start(map: Arc<Map>, speed: f64, port: u16) -> anyhow::Result<()> {
    let model = Arc::new(Mutex::new(sim::Sim::new(1)));
    model
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .preheat(195.0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let service = sim::ModbusService::new(map, model.clone());
    tokio::spawn(async move {
        let on_connected = |stream, peer| {
            let svc = service.for_peer(peer);
            async move {
                tokio_modbus::server::tcp::accept_tcp_connection(stream, peer, move |_| {
                    Ok(Some(svc.clone()))
                })
            }
        };
        let _ = tokio_modbus::server::tcp::Server::new(listener)
            .serve(&on_connected, |_| {})
            .await;
    });
    tokio::spawn(async move {
        let mut pilot = sim::Autopilot::default();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut acc = 0.0;
        loop {
            tick.tick().await;
            let mut s = model.lock().unwrap_or_else(PoisonError::into_inner);
            s.step(0.1 * speed);
            acc += 0.1 * speed;
            if acc >= 1.0 {
                acc = 0.0;
                pilot.tick(&mut s);
            }
        }
    });
    Ok(())
}
