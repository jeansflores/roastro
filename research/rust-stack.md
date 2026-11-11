# Rust stack for Roastro

Research for issue #12. Checked on 2026-11-11 against crates.io metadata, the published `.crate` sources (Cargo.toml, README, CHANGELOG, source), docs.rs, the rustc book and the Rust std docs. Versions are the latest stable on crates.io on that date.

## Question

Which crates and build setup replace the Go choices (ADR 0002)? Constraints:

- One binary for Windows x86_64, built from Linux.
- One PLC read per second over Modbus TCP; roasts recorded in SQLite.
- A REST/WebSocket API and the built React UI, served on localhost from the same binary.
- Also: a Modbus TCP server simulator, a CLI probe and a terminal dashboard.
- Safety: writes to a PLC must be refused for non-loopback targets unless the user opts in.

## Recommendation at a glance

| Concern | Pick | Version | Why, in one line |
|---|---|---|---|
| SQLite | `rusqlite` with `bundled` | 0.42.0 (libsqlite3-sys 0.40.0, SQLite 3.54.1) | Synchronous, thin, one `Connection` fits "one connection" exactly; `bundled` compiles SQLite from C so no system lib is needed on Windows. |
| Migrations | Homegrown runner on `PRAGMA user_version` | n/a | Same design as the Go decision; ~40 lines; avoids a version lock with `rusqlite_migration`. |
| Async runtime | `tokio` | 1.54.1 | Required by `tokio-modbus` and `axum`; one runtime for everything. |
| Modbus client + server | `tokio-modbus` (`tcp`, `tcp-server`) | 0.17.0 | Only permissively licensed crate with both a TCP client and a TCP server; the server's per-request `Service` maps directly onto the Go `ModbusHandler`. |
| HTTP + WebSocket | `axum` with `ws` | 0.8.9 | Tokio-native, built-in WebSocket upgrade, the de facto Rust web framework. |
| Embedded UI | `rust-embed` | 8.14.0 | Embeds `web/dist` in release builds, reads from disk in debug builds, has an official axum SPA example. |
| Terminal dashboard | `ratatui` (crossterm backend, default) | 0.30.2 | Standard Rust TUI; use its `ratatui::crossterm` re-export so the versions never drift. |
| CLI args | `clap` with `derive` | 4.6.7 | Declarative flags and subcommands; replaces Go `flag`. |
| Logging | `tracing` + `tracing-subscriber` (`env-filter`) | 0.1.44 / 0.3.23 | Structured, span-aware; replaces `log/slog`. |
| Errors | `thiserror` in library modules, `anyhow` in binaries | 2.0.21 / 1.0.104 | Typed errors where callers match on them (e.g. write blocked), context-rich errors at the top. |
| JSON / config | `serde` + `serde_json` | 1.0.229 / 1.0.154 | Loads `configs/regmap.json` and serializes API payloads. |
| App-data dir | `dirs` | 7.0.0 | `data_local_dir()` is `%LOCALAPPDATA%` on Windows. |
| Windows build | `cargo-xwin` to `x86_64-pc-windows-msvc` | 0.23.1 | Builds the MSVC target (Windows' native ABI, Tauri's default) from Linux with clang-cl, including SQLite's C code. Fallback: mingw-w64 to `x86_64-pc-windows-gnu`. |

Maintenance signals are in each section. Every pick released in the last 13 months; most in the last 3.

## 1. SQLite

### Candidates

| Crate | Latest | Released | Notes |
|---|---|---|---|
| `rusqlite` | 0.42.0 | 2026-11-11 | Sync wrapper over `libsqlite3-sys`. MSRV 1.89. ~60M recent downloads. |
| `sqlx` (`sqlx-sqlite`) | 0.9.0 | 2026-05-21 | Async; runs SQLite on a worker thread (`sqlx-sqlite/src/connection/worker.rs`). Pins `libsqlite3-sys >=0.30.1, <0.38.0`. |
| `sqlite` (stainless-steel) | 0.37.4 | 2026-10-07 | Smaller, less used (~0.3M recent downloads). |
| `turso` (Limbo rewrite) | 0.8.3 | 2026-11-10 | Pure-Rust rewrite of SQLite, still 0.x and not file-format-proven for long-lived data. |

### Pick: `rusqlite = { version = "0.42", features = ["bundled"] }`

- `bundled` turns on `libsqlite3-sys/bundled`, which compiles the vendored amalgamation with the `cc` crate. libsqlite3-sys 0.40.0 vendors SQLite **3.54.1** (`sqlite3/sqlite3.h`), the same version modernc bundled in the Go decision, so the WAL-reset fix (3.51.3+) is in. Source: `rusqlite-0.42.0/Cargo.toml` `[features]`, `libsqlite3-sys-0.40.0/Cargo.toml`.
- Default features are only `cache` plus a wasm-only FFI that is gated to `target_family = "wasm"`, so a native build pulls in nothing extra. Source: `rusqlite-0.42.0/Cargo.toml`.
- `Connection` is `Send` but not `Sync` (`unsafe impl Send for Connection` in `src/lib.rs`). That matches the "one connection" rule: own it in one place.
- Setup on open: `conn.pragma_update(None, "journal_mode", "WAL")`, `synchronous = NORMAL`, `foreign_keys = ON`, and `conn.busy_timeout(Duration::from_secs(5))` (`src/pragma.rs`, `src/busy.rs`). Unlike Go's pool, there is only one connection, so setting them once after open is enough.
- Async integration: run DB work on a dedicated thread that owns the `Connection` and receives commands over a channel, or hold it in a `std::sync::Mutex` and call it through `tokio::task::spawn_blocking`. At one insert per second either is fine; the owning thread is the cleaner seam.

Rejected:

- **sqlx**: async API and compile-time-checked queries are not needed at 1 write/s; its migrator keeps state in a migrations table (`sqlx-sqlite/src/migrate.rs`, `ensure_migrations_table`), not `user_version`; and it pins `libsqlite3-sys < 0.38`, so it cannot coexist with current rusqlite.
- **sqlite** (stainless-steel): workable but a much smaller user base than rusqlite.
- **turso**: a rewrite, not SQLite; too young for the only copy of someone's roast history.

### Migrations on `PRAGMA user_version`

Pick: a homegrown runner, same as the Go design.

- Keep `migrations/0001_init.sql`, `0002_....sql`, and embed them with a static slice: `const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql"), ...];`
- On open: read `conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))`. For each migration with index >= that value, open `conn.transaction()`, `execute_batch` the SQL, set `user_version` to the new number, commit. Forward-only.
- Refuse to start if `user_version` is greater than `MIGRATIONS.len()` (a newer binary wrote the DB).

Rejected:

- **`rusqlite_migration` 2.7.0** (released 2026-11-10). It does use `user_version`, which is exactly the right design (README: "This library uses the `user_version` value instead"). But 2.7.0 depends on `rusqlite = "0.41.0"` while rusqlite 0.42.0 came out on 2026-11-11, and `libsqlite3-sys` declares `links = "sqlite3"`, so Cargo allows only one version in the graph. Using it now means pinning rusqlite to 0.41. It is a fine swap-in later if the runner grows; the SQL files stay the same.
- **`refinery`** 0.10.0: keeps state in its own history table, not `user_version`.

### Carry-overs from "SQLite in Go"

Still valid: WAL; one connection; `VACUUM INTO` for backups; never put the DB on a network share or synced folder; close before deleting files on Windows; only one SQLite library per process (in Rust that is enforced by `links = "sqlite3"`).

One change worth making: use `dirs::data_local_dir()` (`{FOLDERID_LocalAppData}`, `C:\Users\Alice\AppData\Local`, per docs.rs) rather than Roaming `%AppData%`. Roaming profiles can be synced on domain-joined PCs, which is exactly what the Go note warned against for WAL. Source: https://docs.rs/dirs/7.0.0/dirs/fn.data_local_dir.html

## 2. Async runtime and Modbus TCP

### Runtime: `tokio` 1.54.1 (2026-10-21)

Both `tokio-modbus` and `axum` are built on it, so there is no real choice. Use `tokio-util`'s `CancellationToken` (0.7.20) for shutdown, which `tokio-modbus`'s own server already uses internally.

### Modbus: `tokio-modbus = { version = "0.17", default-features = false, features = ["tcp", "tcp-server"] }`

Released 2025-10-22; repo last pushed 2026-07-22 (commits in July 2026), MIT OR Apache-2.0, ~0.46M recent downloads. Source: `tokio-modbus-0.17.0/Cargo.toml`, `CHANGELOG.md`, GitHub API.

Client (app, probe, monitor):

- `tokio_modbus::client::tcp::connect_slave(SocketAddr, Slave(unit_id))` returns a `client::Context` (`src/client/tcp.rs`).
- Results are `Result<Result<T, ExceptionCode>, Error>` (`src/lib.rs`): the outer error is transport, the inner is a Modbus exception. That is exactly the distinction the Go client makes in `isException`: drop and reconnect on the outer error only.
- **No built-in timeout or reconnect** on the async client (only the `sync` client has `connect_with_timeout`). Wrap every call in `tokio::time::timeout(Duration::from_secs(1), ...)`, and on timeout or transport error drop the `Context` and reconnect on the next poll. The Go `Poller` backoff logic ports directly; it is plain code, no crate needed.
- Disable `rtu` (default) to avoid pulling serial deps.

Server (simulator):

- Implement `tokio_modbus::server::Service` (`call(&self, Request) -> Future<Output = Result<Response, ExceptionCode>>`) on a handler that holds the register map and a handle to the sim state. Same shape as Go's `ModbusHandler`.
- `Server::new(listener).serve(&on_connected, on_process_error)`. `on_connected` gets `(TcpStream, SocketAddr)` per connection and returns the service, so each connection's service can carry the peer address for the "coil write from X" log. Returning `Ok(None)` rejects a connection. Source: `src/server/tcp.rs`.
- Use `SlaveRequest` as the request type to see the unit id and answer only `unit_id`, 0 or 255, as Go's `checkUnit` does.
- tokio-modbus logs through the `log` crate; `tracing-subscriber`'s default `tracing-log` feature bridges it.

Rejected:

- **`rodbus`** 1.6.0 (Step Function I/O): has built-in reconnect strategies, but the README says it is "publicly available under a non-commercial / non-production license" and "You must purchase a commercial license to use this software for profit." Not usable for a production roaster.
- **Writing Modbus TCP by hand**: the framing is small, but tokio-modbus already handles framing, exceptions and server dispatch, and is permissively licensed.

## 3. HTTP, WebSocket and the embedded UI

### `axum = { version = "0.8", features = ["ws"] }` (0.8.9, 2026-04-14)

- The `ws` feature brings `axum::extract::ws::WebSocketUpgrade` over `tokio-tungstenite`. Source: `axum-0.8.9/Cargo.toml`.
- Tokio-native, maintained under tokio-rs, by far the most used Rust web framework (~160M recent downloads).
- `tower-http` 0.7.1 for compression and tracing middleware if needed.

### `rust-embed = { version = "8", features = ["mime-guess"] }` (8.14.0, 2026-11-11)

- README: "loads files into the rust binary at compile time during release and loads the file from the fs during dev". With `debug-embed` it embeds in debug too. During development the Vite dev server proxies to the API anyway.
- The crate ships an `examples/axum-spa/` that uses `Router::fallback` to serve assets and fall back to `index.html` for client routes. Use it as the reference (MIT/Apache, safe to read).
- **Build-order constraint:** in release the derive fails with "folder '…' does not exist" if `web/dist` is missing (`rust-embed-impl/src/lib.rs`). The release build must run `npm ci && npm run build` in `web/` first. Make a `build.rs`-free Makefile target do both, rather than calling npm from `build.rs`.
- Maintenance note: the repository URL moved to `pyrossh.dev/repos/rust-embed` (off GitHub), but releases continue (8.14.0 on 2026-11-11).

Rejected:

- **`include_dir`** 0.7.4: last release 2024-06-17; always embeds, no debug passthrough, no MIME helpers.
- **`memory-serve`** 2.4.0: axum-specific, adds brotli and cache headers, but far smaller user base (~32K recent downloads). Precompression is pointless on localhost.
- **`tower-http::services::ServeDir`** alone: serves from disk, so it does not give a single binary.

## 4. Terminal dashboard

### `ratatui` 0.30.2 (2026-06-19), default `crossterm` backend

- The default feature set includes the `crossterm` backend (via `ratatui-crossterm` 0.1.2, which supports crossterm 0.28 and 0.29). Import crossterm through `ratatui::crossterm` instead of adding `crossterm` directly, so the two never resolve to different versions. Source: `ratatui-0.30.2/Cargo.toml`.
- crossterm 0.29.0 supports the Windows console natively, so the monitor runs on the roasting PC too.
- The Bubble Tea model/update/view split in `internal/tui` maps onto ratatui's immediate-mode `draw` plus an event loop; the logic tests port as plain unit tests on the state struct.

Rejected:

- **cursive**: retained-mode widget tree, heavier, less suited to a live chart.
- **Raw crossterm**: no layout or widgets.

## 5. CLI, logging, errors, JSON

- **`clap` 4.6.7** with `derive`: `#[derive(Parser)]` structs replace `flag`. Keep `--allow-write` a plain `bool` flag that defaults to false; never read it from an env var or config file. Rejected: `argh`, `pico-args` (fewer features, less familiar).
- **`tracing` 0.1.44 + `tracing-subscriber` 0.3.23** with `env-filter` and `fmt`: `RUST_LOG`-style filtering, structured fields like `slog`. Rejected: `log` + `env_logger` (no spans; tracing bridges `log` anyway).
- **`thiserror` 2.0.21** for module error enums (`plc::Error::WriteBlocked`, `UnknownPoint`, `NotWritable`, `Transport`, `Exception`); **`anyhow` 1.0.104** in `main`. Rejected: `color-eyre` 0.6.5 (last release 2025-05-30, prettier but not needed), `snafu` (more ceremony).
- **`serde` 1.0.229 + `serde_json` 1.0.154**: `#[derive(Deserialize)]` on the regmap types. Use `#[serde(rename_all = "snake_case")]` on the `kind` enum and keep the `name` strings as the lookup key, per CLAUDE.md. Use `#[serde(deny_unknown_fields)]` sparingly: the regmap has a `_note` field.

## 6. Building for Windows from Linux

### Targets (rustc book)

- `x86_64-pc-windows-msvc`: Tier 1 with host tools.
- `x86_64-pc-windows-gnu`: Tier 1 with host tools.
- `x86_64-pc-windows-gnullvm`: Tier 2.

Source: https://doc.rust-lang.org/nightly/rustc/platform-support.html

### Pick: `cargo-xwin` 0.23.1 (2026-08-13) to `x86_64-pc-windows-msvc`

Why MSVC: it is the ABI rustup uses on Windows hosts, the one a native Windows CI job builds, and Tauri's default (ADR 0002 mentions Tauri later). `libsqlite3-sys`'s bundled build goes through the `cc` crate, which knows clang-cl.

From the README:

- "Cross compile Cargo project to Windows msvc target with ease using xwin".
- Prerequisites: clang, and `rustup component add llvm-tools` for assembly. A full LLVM install is recommended.
- **License:** "By using this software you are consented to accept the license at https://go.microsoft.com/fwlink/?LinkId=2086102". It downloads the MSVC CRT and Windows SDK. Jean should read and accept that once, knowingly.
- `cargo xwin cache xwin` pre-downloads the CRT/SDK for offline/CI caching; `XWIN_CACHE_DIR` sets where.

Recipe (local, CachyOS/Arch: clang and lld already present):

```bash
rustup target add x86_64-pc-windows-msvc
rustup component add llvm-tools
cargo install --locked cargo-xwin
(cd web && npm ci && npm run build)          # rust-embed needs web/dist
cargo xwin build --release --target x86_64-pc-windows-msvc
# -> target/x86_64-pc-windows-msvc/release/roastro.exe
```

Optional static CRT so the exe has no `VCRUNTIME140.dll` dependency (standard rustc target feature, not cargo-xwin specific):

```toml
# .cargo/config.toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Not verified on this machine (it would require accepting the Microsoft license); verify with `llvm-readobj --coff-imports roastro.exe` that only system DLLs are imported.

### Fallback: mingw-w64 to `x86_64-pc-windows-gnu`

No Microsoft license download, a single distro package:

```bash
sudo pacman -S mingw-w64-gcc        # Ubuntu: apt install gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

The `cc` crate finds `x86_64-w64-mingw32-gcc` for SQLite. Trade-offs: different ABI from a native Windows build, debug info is DWARF not PDB, and a binary that mixes in MSVC-only libraries later (some Tauri/WebView2 paths) would not link.

Rejected: **`cross`** 0.2.5. Last release 2023-02-04 (repo still active on main), needs Docker or Podman, and its Windows image just wraps mingw anyway.

### CI (GitHub Actions)

Two jobs, so the Windows binary is both built and tested on Windows:

1. `ubuntu-latest`: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`; then the cargo-xwin release build above, uploading `roastro.exe` as an artifact. Needs: `actions/setup-node` for `web/`, the msvc target, `cargo install --locked cargo-xwin` (or `pip install cargo-xwin`), clang/lld (present on the hosted Ubuntu image; `apt-get install clang lld` otherwise), and a cache keyed on cargo-xwin's version for `XWIN_CACHE_DIR` plus the usual cargo cache.
2. `windows-latest`: `cargo test` natively (MSVC is preinstalled). This is the only place tests actually run on Windows: path handling, file locking on delete, and WAL behavior. `cargo xwin test` would need wine.

## Safety: the write guard in Rust

None of the chosen crates enforce anything; the guard stays in Roastro's code. Points the port must keep or tighten:

- **Decide on the resolved address.** `tokio-modbus` connects to a `SocketAddr`, so the host is resolved before connecting. Check `addr.ip().to_canonical().is_loopback()`. `to_canonical()` (stable since 1.75) turns IPv4-mapped IPv6 such as `::ffff:127.0.0.1` into IPv4 first; without it that address is not counted as loopback. Source: https://doc.rust-lang.org/std/net/enum.IpAddr.html
- **Hide the raw writer.** Keep the `tokio_modbus::client::Context` private inside the `plc` module so the only path to `write_single_register` / `write_single_coil` goes through `Client::write`, which checks `allow_write || is_loopback` and returns `Error::WriteBlocked`. Consider making `allow_write` a constructor-only field so it cannot change after connect.
- **Bind the HTTP server to `127.0.0.1`** by default, never `0.0.0.0`. Remote access stays read-only.
- **Check the `Origin` header on the WebSocket upgrade** (and on any REST route that writes). Browsers do not apply the same-origin policy to WebSocket handshakes, so any web page open on the roasting PC could otherwise connect to `ws://127.0.0.1:...` and send commands. RFC 6455 section 10.2 tells servers to validate `Origin`. axum does not do this for you.
- **`--allow-write` is a CLI flag only**, default false, never from env or config, matching CLAUDE.md.

## Open risks

1. **rusqlite_migration lags rusqlite by one release** today. Not a problem with the homegrown runner, but any future SQLite-adjacent crate must agree on one `libsqlite3-sys` version (`links = "sqlite3"`).
2. **tokio-modbus has no client timeout/reconnect**; Roastro owns that code and its tests (as it did in Go). Its last release is 13 months old, though the repo had commits in July 2026.
3. **cargo-xwin downloads Microsoft's CRT/SDK under Microsoft's license**; Jean must accept that. The mingw fallback avoids it.
4. **Release builds depend on `web/dist`** existing; CI and the Makefile must build the UI first.
5. **rust-embed moved off GitHub**; watch that releases keep coming. `memory-serve` is the drop-in alternative for axum.
6. **The Windows cross build was not run here**; the recipe comes from the crates' own docs and must be verified on the first `cargo xwin build`.

## Sources

- crates.io API: `https://crates.io/api/v1/crates/<name>` for every crate above (versions, dates, downloads, repository)
- Published crate sources from `https://static.crates.io/crates/<name>/<name>-<version>.crate`: rusqlite 0.42.0, libsqlite3-sys 0.40.0, rusqlite_migration 2.7.0, sqlx-sqlite 0.9.0, tokio-modbus 0.17.0, rodbus 1.6.0 (README only), axum 0.8.9, rust-embed 8.14.0, rust-embed-impl 8.14.0, memory-serve 2.4.0, ratatui 0.30.2, cargo-xwin 0.23.1
- https://github.com/slowtec/tokio-modbus (GitHub API: last push, recent commits)
- https://github.com/cross-rs/cross/releases
- https://doc.rust-lang.org/nightly/rustc/platform-support.html
- https://doc.rust-lang.org/std/net/enum.IpAddr.html
- https://docs.rs/dirs/7.0.0/dirs/fn.data_local_dir.html
- https://www.rfc-editor.org/rfc/rfc6455#section-10.2
- Prior decision: `research/sqlite-in-go.md` on branch `research/sqlite-in-go` (issue #3)
