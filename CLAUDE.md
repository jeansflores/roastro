# Roastro

Roasting software for a Carmomaq Caloratto 5 kg roaster that speaks Modbus TCP. Go backend and tools now; a TS/React frontend will live in `web/`. Background, machine facts and decisions: `docs/context.md`. Milestones and feature plan: `docs/roadmap.md`. Primary sources (manual, electrical schematic): `docs/reference/`.

## Rules

- **Everything in English**: code, comments, docs, commit messages, UI text. Talk to Jean in Brazilian Portuguese.
- **Writes to the machine are dangerous.** It is a gas roaster. Any code path that writes to a PLC must refuse non-loopback targets unless the user passes an explicit opt-in (`--allow-write` in CLIs, an explicit confirmation in a UI). Remote access is read-only. Never weaken this guard, never default it on.
- **`configs/regmap.json` is provisional.** Addresses are made up until the real Carmomaq `.aset` arrives. Point `name`s are the contract between the simulator and the app: never rename them. Code must look points up by name, never hard-code addresses.
- Do not copy code from Artisan (GPLv3). Reading its `.aset` files for the register map is fine.
- Commits follow Conventional Commits (`feat(plc): ...`, `test(roast): ...`). Commit only when Jean asks.
- Jean is learning Go with this repo: write idiomatic, commercial-grade Go. When you introduce a new idiom or package, update `docs/study-guide.md`.

## Layout

```
cmd/sim        PLC simulator: Modbus TCP :5502 + HTTP control panel :8080
cmd/probe      line-based Modbus client (read, CSV, one-off writes)
cmd/monitor    terminal dashboard (Bubble Tea) with live readings and keyboard commands
internal/regmap  register map loading, encode/decode
internal/sim     thermal model, PLC state, interlocks, autopilot
internal/plc     Modbus client: read/write by point name, reconnect with backoff
internal/roast   pure roast calculations (RoR, ...)
internal/tui     Bubble Tea model/view for cmd/monitor
configs/       runtime config (register map)
docs/          context, roadmap, study guide, reference PDFs
```

## Commands

```bash
make test       # go test -race ./...
make lint       # golangci-lint run (config in .golangci.yml)
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
