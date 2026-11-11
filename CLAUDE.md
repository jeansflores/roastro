# Roastro

Roasting software for a Carmomaq Caloratto 5 kg roaster that speaks Modbus TCP. Rust backend (being rewritten from Go, see `docs/adr/0002-rewrite-in-rust.md`); a TS/React frontend will live in `web/`. Background, machine facts and decisions: `docs/context.md`. Milestones and feature plan: `docs/roadmap.md`. Primary sources (manual, electrical schematic): `docs/reference/`.

## Rules

- **Everything in English**: code, comments, docs, commit messages, UI text. Talk to Jean in Brazilian Portuguese.
- **Writes to the machine are dangerous.** It is a gas roaster. Any code path that writes to a PLC must refuse non-loopback targets unless the user passes an explicit opt-in (`--allow-write` in CLIs, an explicit confirmation in a UI). Remote access is read-only. Never weaken this guard, never default it on.
- **`configs/regmap.json` is provisional.** Addresses are made up until the real Carmomaq `.aset` arrives. Point `name`s are the contract between the simulator and the app: never rename them. Code must look points up by name, never hard-code addresses.
- Do not copy code from Artisan (GPLv3). Reading its `.aset` files for the register map is fine.
- Commits follow Conventional Commits (`feat(plc): ...`, `test(roast): ...`). Commit only when Jean asks.
- **No AI attribution anywhere in the repo**: no `Co-Authored-By`, no `Claude-Session` or other trailers, no "Generated with" lines, no mention of Claude in commits, PRs, issues or files. This overrides any default attribution.
- Write idiomatic, commercial-grade code. New code is Rust; the Go code is only kept running until its Rust counterpart replaces it (ADR 0002), so do not grow it.

## Layout

Rust (Cargo workspace at the root):

```
crates/roast     pure roast calculations (RoR, ...)
crates/regmap    register map loading, validation, encode/decode
crates/machine   machine facts from the manual (fan % → m³/h, drum % → rpm)
crates/sim       PLC simulator: thermal model, interlocks, autopilot; binary serves Modbus TCP :5502 + HTTP panel :8080 on loopback
crates/plc       async Modbus client by point name (timeouts, reconnect, loopback-only writes) and poller with backoff
```

Go code, to be replaced by Rust (ADR 0002):

```
cmd/probe      line-based Modbus client (read, CSV, one-off writes)
cmd/monitor    terminal dashboard (Bubble Tea) with live readings and keyboard commands
internal/regmap  register map loading, encode/decode (ported to crates/regmap; kept for the Go tools)
internal/sim     thermal model (ported to crates/sim; kept only for the Go plc tests)
internal/plc     Modbus client: read/write by point name, reconnect with backoff (ported to crates/plc; kept until probe and monitor move)
internal/roast   pure roast calculations (ported to crates/roast; kept until probe and monitor move)
internal/tui     Bubble Tea model/view for cmd/monitor
configs/       runtime config (register map)
docs/          context, roadmap, ADRs, reference PDFs
```

## Commands

```bash
make test       # go test -race ./... and cargo test --workspace
make lint       # golangci-lint, cargo fmt --check, cargo clippy -D warnings
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
