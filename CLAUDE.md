# Roastro

Roasting software for a Carmomaq Caloratto 5 kg roaster that speaks Modbus TCP. Rust backend (see `docs/adr/0002-rewrite-in-rust.md`); a TS/React frontend will live in `web/`. Background, machine facts and decisions: `docs/context.md`. Milestones and feature plan: `docs/roadmap.md`. Primary sources (manual, electrical schematic): `docs/reference/`.

## Rules

- **Everything in English**: code, comments, docs, commit messages, UI text. Talk to Jean in Brazilian Portuguese.
- **Writes to the machine are dangerous.** It is a gas roaster. Any code path that writes to a PLC must refuse non-loopback targets unless the user passes an explicit opt-in (`--allow-write` in CLIs, an explicit confirmation in a UI). Remote access is read-only. Never weaken this guard, never default it on.
- **`configs/regmap.json` is provisional.** Its addresses are made up for the simulator. The real layout is known from Artisan's Caloratto 2.0 preset (`docs/context.md`, "Register map"); moving to it waits on two map-format additions. `configs/regmap.caloratto-2.0.json` is a read-only test map for the real machine and must stay without writable points. Point `name`s are the contract between the simulator and the app: never rename them. Code must look points up by name, never hard-code addresses.
- Do not copy code from Artisan (GPLv3). Reading its `.aset` files for the register map is fine.
- Commits follow Conventional Commits (`feat(plc): ...`, `test(roast): ...`). Commit only when Jean asks.
- **No AI attribution anywhere in the repo**: no `Co-Authored-By`, no `Claude-Session` or other trailers, no "Generated with" lines, no mention of Claude in commits, PRs, issues or files. This overrides any default attribution.
- Write idiomatic, commercial-grade Rust.

## Layout

Cargo workspace at the root:

```
crates/roast     roast vocabulary (event kinds, sources) and pure calculations (RoR, ...)
crates/recorder  pure lifecycle state machine: readings in, roasts out (Charge/Drop detection, turning point, lead-in, tail, Gaps)
crates/store     SQLite: schema, embedded migrations on PRAGMA user_version, the roast record
crates/regmap    register map loading, validation, encode/decode
crates/machine   machine facts from the manual (fan % → m³/h, drum % → rpm)
crates/plc       async Modbus client by point name (timeouts, reconnect, loopback-only writes) and poller with backoff
crates/sim       PLC simulator: thermal model, interlocks, autopilot; binary serves Modbus TCP :5502 + HTTP panel :8080 on loopback
crates/probe     line-based Modbus client (read, CSV, one-off writes)
crates/monitor   terminal dashboard (ratatui) with live readings and keyboard commands
configs/         runtime config (register map)
docs/            context, roadmap, ADRs, reference PDFs
```

## Commands

```bash
make test       # cargo test --workspace
make lint       # cargo fmt --check, cargo clippy -D warnings
make run-sim    # simulator with autopilot, SPEED=10
make monitor    # terminal monitor against the local simulator, SPEED=10
make probe      # line-based probe, SPEED=10
```

Run binaries from the repo root: the default `--map` path is `configs/regmap.json`. `make lint` and `make test` must pass before committing.

## Agent skills

### Issue tracker

Issues live in GitHub Issues on jeansflores/roastro (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default labels: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
