//! The machine lifecycle against a real Modbus server: not set up, test
//! settings, set up, readings ("Machine setup and connection lifecycle", #34).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use roastro::engine::{Edit, Request};
use roastro::wire::{LinkState, Live, MachineIn};
use tokio::sync::{mpsc, oneshot};

/// Serves the simulator on a free loopback port; returns the port.
async fn simulator() -> u16 {
    simulator_with(|_| {}).await
}

/// As [`simulator`], set up by `setup` first.
async fn simulator_with(setup: impl FnOnce(&mut sim::Sim)) -> u16 {
    let map = Arc::new(roastro::machine::model_map(roastro::machine::SIMULATOR).unwrap());
    let model = Arc::new(Mutex::new(sim::Sim::new(1)));
    model.lock().unwrap().preheat(195.0);
    setup(&mut model.lock().unwrap());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let service = sim::ModbusService::new(map, model);
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
    port
}

async fn ask<T>(
    requests: &mpsc::Sender<Request>,
    make: impl FnOnce(oneshot::Sender<Result<T, roastro::api::ApiError>>) -> Edit,
) -> Result<T, roastro::api::ApiError> {
    let (tx, rx) = oneshot::channel();
    requests.send(Request::Edit(make(tx))).await.unwrap();
    rx.await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn sets_up_a_machine_after_testing_it() {
    let port = simulator().await;
    let dir = std::env::temp_dir().join(format!("roastro-it-{}", uuid::Uuid::now_v7()));
    let running = roastro::start(roastro::Config {
        url: None,
        map: roastro::machine::model_map(roastro::machine::SIMULATOR).unwrap(),
        db: dir.join("roastro.db"),
        interval: Duration::from_millis(100),
        time_scale: 1.0,
        demo: None,
    })
    .unwrap();
    let requests = running.requests.clone();
    let mut live = running.live.subscribe();

    let (tx, rx) = oneshot::channel();
    requests.send(Request::Subscribe(tx)).await.unwrap();
    let (snapshot, _sub) = rx.await.unwrap().unwrap();
    let Live::Snapshot(snapshot) = &*snapshot else {
        panic!("a snapshot first")
    };
    assert_eq!(snapshot.link.state, LinkState::NotSetUp);

    let settings = |port: u16| MachineIn {
        name: "Bench".into(),
        address: format!("127.0.0.1:{port}"),
        model: roastro::machine::SIMULATOR.into(),
        demo: false,
    };
    // A port where nothing listens: the advice says so.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let refused = ask(&requests, |r| Edit::TestMachine(settings(closed), r))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "machine_unreachable");
    assert!(refused.to_string().contains("does not answer"), "{refused}");

    let test = ask(&requests, |r| Edit::TestMachine(settings(port), r))
        .await
        .unwrap();
    assert!(test.values.contains_key("bt"), "{test:?}");
    assert_eq!(
        ask(&requests, Edit::Machine).await.unwrap().address,
        "",
        "a test changes nothing"
    );

    let machine = ask(&requests, |r| Edit::SetMachine(settings(port), r))
        .await
        .unwrap();
    assert_eq!(machine.address, format!("tcp://127.0.0.1:{port}"));
    let connected = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(msg) = live.recv().await
                && matches!(&*msg, Live::Sample(_))
            {
                return;
            }
        }
    })
    .await;
    assert!(connected.is_ok(), "readings arrive from the machine set up");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_plc_still_records() {
    // The `slow` scenario: every answer 400 ms late, three answers a reading.
    let port = simulator_with(|s| s.set_link_delay(Duration::from_millis(400))).await;
    let dir = std::env::temp_dir().join(format!("roastro-slow-{}", uuid::Uuid::now_v7()));
    let running = roastro::start(roastro::Config {
        url: Some(format!("tcp://127.0.0.1:{port}")),
        map: roastro::machine::model_map(roastro::machine::SIMULATOR).unwrap(),
        db: dir.join("roastro.db"),
        interval: Duration::from_secs(1),
        time_scale: 1.0,
        demo: None,
    })
    .unwrap();
    let mut live = running.live.subscribe();
    let mut samples = 0;
    let mut downs = 0;
    let _ = tokio::time::timeout(Duration::from_secs(6), async {
        while let Ok(msg) = live.recv().await {
            match &*msg {
                Live::Sample(_) => samples += 1,
                Live::Link(l) if l.state == LinkState::Down => downs += 1,
                _ => {}
            }
        }
    })
    .await;
    assert!(samples >= 3, "readings keep arriving: {samples}");
    assert_eq!(downs, 0, "late is not down");
    let _ = std::fs::remove_dir_all(dir);
}
