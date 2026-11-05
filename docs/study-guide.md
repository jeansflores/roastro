# Study guide

This repo doubles as an end-to-end Go learning project. This page maps the idioms it uses to where they live in the code, so you can read the code with a question in mind. Keep it current: when a change introduces a new idiom, add it here.

Suggested reading order: `internal/roast` → `internal/regmap` → `internal/plc` → `internal/tui` → `cmd/monitor`. Each step depends only on the ones before it.

## 1. Project layout

```
cmd/<name>/main.go   one directory per binary; package main; only wiring
internal/<pkg>/      the real code; importable only from inside this module
configs/             runtime data, not code
```

- **`internal/` is enforced by the compiler.** Another module cannot import `github.com/jeansflores/project-c/internal/plc`. This lets us change APIs freely while the project is young.
- **`main` stays thin.** `cmd/monitor/main.go` parses flags, builds objects and connects them. No business logic, so everything interesting is testable without running a binary.
- **Package names are short, lowercase nouns** that say what the package *provides*: `plc`, `roast`, `tui`. The caller reads `plc.NewClient`, `roast.NewRoR`, so names inside packages do not repeat the package (`plc.Client`, not `plc.PLCClient`).
- **Dependencies point one way:** `tui → plc → regmap`, `tui → roast`, `tui → machine`, `sim → machine`. Nothing imports `tui`. When the monitor needed the rpm/m³/h tables, they moved from `sim` into `machine` rather than letting `tui` import the simulator (see commit `refactor: move the manual's conversion tables`).

## 2. Errors

- **Errors are values, returned last.** `func (c *Client) Read() (Reading, error)`.
- **Wrap with `%w` to add context and keep the cause:** `fmt.Errorf("plc: connect %s: %w", url, err)` in `internal/plc/client.go`. Messages are lowercase, without trailing punctuation, prefixed by the package, so a chain reads like `plc: read holding 0+8: EOF`.
- **Sentinel errors** for conditions callers branch on: `plc.ErrWriteBlocked`. Callers test with `errors.Is(err, plc.ErrWriteBlocked)`, which also sees through wrapping. `isException` uses the same idea to tell "the PLC answered no" from "the cable is gone".
- **`main` → `run() error`.** `cmd/probe` and `cmd/monitor` do the work in `run` and only `main` exits. `log.Fatal` inside `run` would skip deferred `Close` calls; the linter (`gocritic: exitAfterDefer`) caught exactly that in the imported code.
- **Explicitly ignoring an error is a decision, so write it down:** `_ = c.Close()` and the `set` helper in `internal/sim/autopilot.go`, with a comment saying why it is safe.

## 3. Interfaces

- **Small, defined by the consumer.** `tui.Writer` (one method) lives in `tui`, not in `plc`. `plc.Reader` lives next to `Poller`, which consumes it. `*plc.Client` satisfies both without declaring anything: Go interfaces are implicit.
- **Accept interfaces, return structs.** `plc.NewClient` returns `*Client` (concrete, full API); `tui.New` accepts a `Writer` (just what it needs). This is what lets `internal/tui/model_test.go` use a `fakeWriter` and `internal/plc/poller_test.go` use a `scriptedReader`.

## 4. Concurrency

- **A goroutine that owns a loop, a channel out, a context to stop it.** `Poller.Run` in `internal/plc/poller.go`:
  - `ctx context.Context` is the first parameter and the only way to stop it;
  - it returns a receive-only channel (`<-chan Event`) so callers cannot send or close it;
  - the goroutine closes the channel when it exits (`defer close(out)`), so readers see the end;
  - every blocking point is a `select` that also watches `ctx.Done()`.
- **Mutex for shared state.** `plc.Client` guards its connection with `sync.Mutex`; the TUI's write goroutine and the poller can call it at the same time. Methods named `...Locked` expect the caller to hold the lock.
- **The race detector:** `make test` runs `go test -race`. Keep it that way.

## 5. Data and values

- **Value vs pointer receivers.** `tui.Model` uses value receivers because Bubble Tea treats the model as a value that `Update` returns. `plc.Client` uses pointer receivers because it holds a mutex and a connection that must not be copied.
- **Maps are references.** Copying a struct that holds a map copies the pointer, not the data. The TUI originally stored the poller's map and then wrote into it, which leaked changes back to the caller; a test caught it and `maps.Clone` fixed it (`applyEvent` in `internal/tui/model.go`).
- **Zero values and defaults.** `Poller{Reader: c}` is usable: `setDefaults` fills in zero fields. `plc.Config.Timeout` defaults to 1 s. Make the zero value useful, or say in the doc comment that it is not (`roast.RoR`).
- **`time.Duration` for every duration**, never a bare `int` of seconds. `roast.RoR` takes the roast clock as a `Duration` and stays free of wall-clock time, which is why it is trivially testable.
- **Builtins `min` and `max`** (Go 1.21+) for clamping: `min(max(v, lo), hi)`.

## 6. Testing

- **Table-driven tests with subtests:** `TestRoRValue`, `TestPointEncodeDecode`, `TestKeysSendCommands`. Add a case by adding a row; `t.Run(tt.name, ...)` gives each row its own name in the output (`go test -run 'TestRoRValue/rising' -v ./internal/roast`).
- **`t.Helper()`** in helpers so failures point at the test line, not the helper.
- **`t.Cleanup` and `t.TempDir`** instead of manual teardown.
- **Real dependencies when they are cheap.** `internal/plc/client_test.go` starts a real Modbus server with the real simulator on a free port, stops it and starts it again to test reconnection. No mocks of the Modbus library.
- **Testing Bubble Tea without a terminal:** call `Update` with a message, inspect the returned model, and run the returned `tea.Cmd` yourself (`cmd()`) to trigger the side effect. `internal/tui/model_test.go` does exactly this.
- **Regression tests name the bug:** `TestRepeatedKeysBuildOnPendingWrites` exists because pressing `+` twice quickly set gas to 5 % instead of 10 %.

## 7. Bubble Tea (the Elm architecture)

- `Model` holds all state. `Update(msg) (Model, Cmd)` is the only place it changes. `View()` is a pure function of the model.
- Side effects (Modbus writes, waiting on a channel, timers) are **commands**: functions returning a message, which Bubble Tea runs on another goroutine. `waitForEvent` turns the poller's channel into a stream of messages by re-issuing itself after each event.
- Logic (`model.go`) is separate from rendering (`view.go`); only the former is unit-tested.

## 8. Logging

- **`log/slog`** (standard library, structured): `logger.Warn("plc disconnected", "err", err)` produces `level=WARN msg="plc disconnected" err=...`. Keys and values, not formatted strings, so logs can be filtered and parsed.
- Loggers are **passed in** (`Poller.Log`, `tui.New(..., log)`), never global, and default to `slog.DiscardHandler` in libraries.

## 9. Tooling

- `make lint` runs golangci-lint with `.golangci.yml`: the standard linters plus `revive` (doc comments on exported names, naming), `errorlint` (use `errors.Is`/`%w`), `gocritic`, `misspell` and others. Its first run over the imported simulator found 41 issues; commit `fix: address golangci-lint findings` is a good diff to read.
- `golangci-lint fmt` formats and orders imports: standard library, third party, then this module.
- `go doc <pkg> <Symbol>` reads any package's docs offline, e.g. `go doc charm.land/bubbletea/v2 Model`.

## Exercises

1. Add a `--ror-window` flag to `cmd/monitor` (hint: `tui.Config.RoRWindow` already exists).
2. Show the time since charge using the hopper sensor instead of the PLC timer. Write the test first.
3. Make `roast.RoR` ignore a single spike larger than 20 °C (a loose thermocouple). Add a table row for it.
4. Add a `Status` type (`Connecting`, `Connected`, `Down`) to `tui` with a `String()` method, replacing the two booleans the view switches on.
