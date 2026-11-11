use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use regmap::{Kind, Map, Point};
use tokio::sync::Mutex;
use tokio_modbus::client::{Context, Reader as _, Writer as _};
use tokio_modbus::{ExceptionCode, Slave};

/// Why a PLC call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The URL is not `tcp://host:port`.
    #[error("plc: invalid url {0:?} (want tcp://host:port)")]
    InvalidUrl(String),
    /// A write to a non-loopback target without the explicit opt-in.
    #[error(
        "plc: write blocked: target is not localhost; opt in only with someone next to the machine"
    )]
    WriteBlocked,
    /// The register map has no point with this name.
    #[error("plc: unknown point {0:?}")]
    UnknownPoint(String),
    /// The point exists but is read only.
    #[error("plc: point {0:?} is not writable")]
    NotWritable(String),
    /// The TCP connection could not be made.
    #[error("plc: connect {target}: {source}")]
    Connect {
        /// `host:port` that was dialled.
        target: String,
        /// Underlying error.
        source: io::Error,
    },
    /// The PLC did not answer in time.
    #[error("plc: {op}: timed out")]
    Timeout {
        /// What was being done.
        op: String,
    },
    /// The connection broke or the PLC answered garbage.
    #[error("plc: {op}: {source}")]
    Transport {
        /// What was being done.
        op: String,
        /// Underlying error.
        source: tokio_modbus::Error,
    },
    /// The PLC answered with fewer values than requested.
    #[error("plc: {op}: short response")]
    ShortResponse {
        /// What was being done.
        op: String,
    },
    /// The PLC answered and refused (a Modbus exception). The connection is fine.
    #[error("plc: {op}: PLC refused: {code}")]
    Exception {
        /// What was being done.
        op: String,
        /// Exception code sent by the PLC.
        code: ExceptionCode,
    },
}

impl Error {
    /// Reports whether the connection should be dropped and redialled.
    fn breaks_connection(&self) -> bool {
        !matches!(self, Error::Exception { .. })
    }
}

/// How to reach the PLC.
#[derive(Debug, Clone)]
pub struct Config {
    /// For example `tcp://127.0.0.1:5502` or `tcp://192.168.5.11:502`.
    pub url: String,
    /// Register map that translates names into Modbus tables.
    pub map: Arc<Map>,
    /// Per request and per connection attempt.
    pub timeout: Duration,
    /// Permits writes to a non-loopback target. Leave it false unless someone
    /// is standing next to the machine.
    pub allow_write: bool,
}

impl Config {
    /// A config with a 1 s timeout and writes limited to localhost.
    #[must_use]
    pub fn new(url: impl Into<String>, map: Arc<Map>) -> Self {
        Self {
            url: url.into(),
            map,
            timeout: Duration::from_secs(1),
            allow_write: false,
        }
    }
}

/// One snapshot of every point in the register map.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// Physical value per point name (1/0 for bits).
    pub values: HashMap<String, f64>,
    /// Time spent on the Modbus round trips.
    pub latency: Duration,
}

/// Reads and writes PLC points by name.
///
/// It connects lazily and drops the connection after a transport error, so the
/// next call redials. Calls are serialised; share it behind an `Arc`.
pub struct Client {
    cfg: Config,
    host: String,
    port: u16,
    conn: Mutex<Option<Context>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.cfg.url)
            .field("allow_write", &self.cfg.allow_write)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Returns a client for `cfg`. It does not connect yet.
    ///
    /// # Errors
    ///
    /// Fails if the URL is not `tcp://host:port`.
    pub fn new(cfg: Config) -> Result<Self, Error> {
        let (host, port) = parse_url(&cfg.url).ok_or_else(|| Error::InvalidUrl(cfg.url.clone()))?;
        Ok(Self {
            cfg,
            host,
            port,
            conn: Mutex::new(None),
        })
    }

    /// Reports whether [`Client::write`] would be allowed to reach the target.
    #[must_use]
    pub fn can_write(&self) -> bool {
        self.cfg.allow_write || is_loopback_host(&self.host)
    }

    /// Fetches every point in the register map, one round trip per data table.
    ///
    /// # Errors
    ///
    /// Fails if the PLC cannot be reached, times out or refuses.
    pub async fn read(&self) -> Result<Reading, Error> {
        let start = Instant::now();
        let mut conn = self.conn.lock().await;
        let ctx = match conn.take() {
            Some(ctx) => conn.insert(ctx),
            None => conn.insert(self.connect().await?),
        };
        let result = read_all(ctx, &self.cfg.map, self.cfg.timeout).await;
        if result.as_ref().is_err_and(Error::breaks_connection) {
            *conn = None;
        }
        Ok(Reading {
            values: result?,
            latency: start.elapsed(),
        })
    }

    /// Sets point `name` to `value`, in physical units (1/0 for coils).
    ///
    /// # Errors
    ///
    /// Fails with [`Error::WriteBlocked`] before dialling anything when the
    /// target is not this machine and writes were not allowed; otherwise on an
    /// unknown or read-only point, or if the PLC cannot be reached or refuses.
    pub async fn write(&self, name: &str, value: f64) -> Result<(), Error> {
        if !self.can_write() {
            return Err(Error::WriteBlocked);
        }
        let p = self
            .cfg
            .map
            .by_name(name)
            .ok_or_else(|| Error::UnknownPoint(name.to_owned()))?;
        if !p.writable {
            return Err(Error::NotWritable(name.to_owned()));
        }
        let mut conn = self.conn.lock().await;
        let ctx = match conn.take() {
            Some(ctx) => conn.insert(ctx),
            None => conn.insert(self.connect().await?),
        };
        let op = format!("write {name}={value}");
        let result = if p.kind == Kind::Coil {
            call(
                &op,
                self.cfg.timeout,
                ctx.write_single_coil(p.addr, value != 0.0),
            )
            .await
        } else {
            call(
                &op,
                self.cfg.timeout,
                ctx.write_single_register(p.addr, p.encode(value)),
            )
            .await
        };
        if result.as_ref().is_err_and(Error::breaks_connection) {
            *conn = None;
        }
        result
    }

    /// Drops the connection, if any.
    pub async fn close(&self) {
        *self.conn.lock().await = None;
    }

    async fn connect(&self) -> Result<Context, Error> {
        let target = format!("{}:{}", self.host, self.port);
        let fail = |source| Error::Connect {
            target: target.clone(),
            source,
        };
        let timed_out = || fail(io::ErrorKind::TimedOut.into());

        let addrs = tokio::time::timeout(
            self.cfg.timeout,
            tokio::net::lookup_host((self.host.as_str(), self.port)),
        )
        .await
        .map_err(|_| timed_out())?
        .map_err(fail)?;
        // A loopback name must only ever reach this machine, whatever it resolves to.
        let loopback_only = is_loopback_host(&self.host);
        let addrs: Vec<SocketAddr> = addrs
            .filter(|a| !loopback_only || a.ip().to_canonical().is_loopback())
            .collect();

        let mut last = io::Error::new(io::ErrorKind::NotFound, "no usable address");
        for addr in addrs {
            let slave = Slave(self.cfg.map.unit_id());
            match tokio::time::timeout(
                self.cfg.timeout,
                tokio_modbus::client::tcp::connect_slave(addr, slave),
            )
            .await
            {
                Ok(Ok(ctx)) => return Ok(ctx),
                Ok(Err(e)) => last = e,
                Err(_) => last = io::ErrorKind::TimedOut.into(),
            }
        }
        Err(fail(last))
    }
}

/// Runs one Modbus request with a timeout and sorts out its three outcomes.
async fn call<T>(
    op: &str,
    timeout: Duration,
    request: impl Future<Output = tokio_modbus::Result<T>>,
) -> Result<T, Error> {
    match tokio::time::timeout(timeout, request).await {
        Err(_) => Err(Error::Timeout { op: op.to_owned() }),
        Ok(Err(source)) => Err(Error::Transport {
            op: op.to_owned(),
            source: closed_as_eof(source),
        }),
        Ok(Ok(Err(code))) => Err(Error::Exception {
            op: op.to_owned(),
            code,
        }),
        Ok(Ok(Ok(value))) => Ok(value),
    }
}

/// The peer closing the socket can surface as an OS error with code 0, which
/// prints as "Success (os error 0)". Say what happened instead.
fn closed_as_eof(err: tokio_modbus::Error) -> tokio_modbus::Error {
    match err {
        tokio_modbus::Error::Transport(io) if io.raw_os_error() == Some(0) => {
            io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed by the PLC").into()
        }
        other => other,
    }
}

/// Reads each data table as one contiguous block.
async fn read_all(
    ctx: &mut Context,
    map: &Map,
    timeout: Duration,
) -> Result<HashMap<String, f64>, Error> {
    let mut out = HashMap::with_capacity(map.points().len());
    for kind in [Kind::Holding, Kind::Input, Kind::Coil, Kind::Discrete] {
        let points: Vec<&Point> = map.points().iter().filter(|p| p.kind == kind).collect();
        let (Some(lo), Some(hi)) = (
            points.iter().map(|p| p.addr).min(),
            points.iter().map(|p| p.addr).max(),
        ) else {
            continue;
        };
        let n = hi - lo + 1;
        let op = format!("read {kind} {lo}+{n}");
        let short = || Error::ShortResponse { op: op.clone() };
        match kind {
            Kind::Holding | Kind::Input => {
                let words = if kind == Kind::Holding {
                    call(&op, timeout, ctx.read_holding_registers(lo, n)).await?
                } else {
                    call(&op, timeout, ctx.read_input_registers(lo, n)).await?
                };
                for p in points {
                    let raw = *words.get(usize::from(p.addr - lo)).ok_or_else(short)?;
                    out.insert(p.name.clone(), p.decode(raw));
                }
            }
            Kind::Coil | Kind::Discrete => {
                let bits = if kind == Kind::Coil {
                    call(&op, timeout, ctx.read_coils(lo, n)).await?
                } else {
                    call(&op, timeout, ctx.read_discrete_inputs(lo, n)).await?
                };
                for p in points {
                    let on = *bits.get(usize::from(p.addr - lo)).ok_or_else(short)?;
                    out.insert(p.name.clone(), if on { 1.0 } else { 0.0 });
                }
            }
        }
    }
    Ok(out)
}

/// Splits `tcp://host:port` (IPv6 hosts in brackets) into host and port.
fn parse_url(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("tcp://")?;
    let (host, port) = match rest.strip_prefix('[') {
        Some(v6) => {
            let (host, port) = v6.split_once(']')?;
            (host, port.strip_prefix(':')?)
        }
        None => rest.rsplit_once(':')?,
    };
    let port = port.parse().ok()?;
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

/// Reports whether a host names this machine, without asking DNS: `localhost`
/// or a loopback IP literal, IPv4-mapped IPv6 included.
fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// Reports whether a Modbus URL such as `tcp://127.0.0.1:5502` points at this
/// machine.
#[must_use]
pub fn is_loopback(url: &str) -> bool {
    parse_url(url).is_some_and(|(host, _)| is_loopback_host(&host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        assert_eq!(
            parse_url("tcp://127.0.0.1:5502"),
            Some(("127.0.0.1".into(), 5502))
        );
        assert_eq!(parse_url("tcp://[::1]:502"), Some(("::1".into(), 502)));
        assert_eq!(
            parse_url("tcp://roaster.local:502"),
            Some(("roaster.local".into(), 502))
        );
        for bad in [
            "127.0.0.1:502",
            "tcp://:502",
            "tcp://host",
            "tcp://host:x",
            "tcp://[::1]502",
        ] {
            assert_eq!(parse_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn recognises_loopback() {
        for url in [
            "tcp://127.0.0.1:5502",
            "tcp://localhost:5502",
            "tcp://[::1]:5502",
            "tcp://127.0.0.2:502",
            "tcp://[::ffff:127.0.0.1]:502",
        ] {
            assert!(is_loopback(url), "{url}");
        }
        for url in [
            "tcp://192.168.5.11:502",
            "tcp://roaster.tailnet.ts.net:502",
            "tcp://localhost.example.com:502",
            "tcp://[::ffff:192.168.5.11]:502",
        ] {
            assert!(!is_loopback(url), "{url}");
        }
    }

    #[tokio::test]
    async fn blocks_remote_writes_before_dialling() {
        let map = Arc::new(
            Map::from_json(
                r#"{"points":[{"name":"burner","kind":"coil","addr":0,"writable":true}]}"#,
            )
            .unwrap(),
        );
        let cases = [
            ("tcp://127.0.0.1:5502", false, true),
            ("tcp://192.168.5.11:502", false, false),
            ("tcp://192.168.5.11:502", true, true),
            ("tcp://roaster.tailnet.ts.net:502", false, false),
        ];
        for (url, allow_write, want) in cases {
            let client = Client::new(Config {
                allow_write,
                ..Config::new(url, map.clone())
            })
            .unwrap();
            assert_eq!(client.can_write(), want, "{url} allow={allow_write}");
            if !want {
                // Must refuse before ever dialling the machine.
                assert!(
                    matches!(client.write("burner", 1.0).await, Err(Error::WriteBlocked)),
                    "{url}"
                );
            }
        }
    }
}
