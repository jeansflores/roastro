# project-c

Roasting software for a **Carmomaq Caloratto 5 kg** coffee roaster, which talks **Modbus TCP**. The app is in its early days: this repository holds a PLC simulator, a terminal monitor that proves the Modbus link end to end, and a minimal line-based client. Background and decisions: [`docs/context.md`](docs/context.md). Roadmap: [`docs/roadmap.md`](docs/roadmap.md). Learning notes: [`docs/study-guide.md`](docs/study-guide.md).

| Command | What it is |
|---|---|
| `cmd/sim` | Pretends to be the PLC: thermal model (drum, beans, moisture, lagging probes, gas servo) and interlocks, served over Modbus TCP, plus an HTTP panel for the "physical world" |
| `cmd/monitor` | Live terminal dashboard: roast values, link health, keyboard commands, automatic reconnection |
| `cmd/probe` | One line per reading, optional CSV, one-off writes |

All three behave the same against the simulator and the real machine.

## Run

Requires Go 1.27. Run from the repository root.

```bash
make run-sim     # simulator with the autopilot, 10x faster (SPEED=10)
make monitor     # dashboard against it, in another terminal
make help        # everything else
```

`--auto` (used by `make run-sim`) turns on an "operator" that preheats, charges 5 kg at 200 °C, adjusts gas and air by temperature band, drops at 210 °C, cools and starts over. Without `--auto`, you (or your app) are in control. To skip the cold warm-up: `curl 'localhost:8080/preheat?temp=200'`.

### Monitor

```
project-c monitor  tcp://127.0.0.1:5502  ● CONNECTED  writes enabled
╭──────────────────────────────────────────╮ ╭──────────────────────────────────────────╮
│ Roast                                    │ │ Link                                     │
│                                          │ │                                          │
│ BT        124.8 °C   RoR +15.6 °C/min    │ │ Status   connected                       │
│ ET        171.9 °C                       │ │ Latency  540µs                           │
│ Timer    03:53                           │ │ Last OK  0.6 s ago                       │
│                                          │ │ Polls    245  ok 241  failed 4           │
│ Gas       80 %  (valve  80 %)            │ │ Last err                                 │
│ Drum      80 %   52 rpm                  │ │ plc: read holding 0+8: EOF               │
│ Air       45 %  124 m³/h                 │ ╰──────────────────────────────────────────╯
│                                          │
│ BURNER DRUM FAN stirrer cooler           │
╰──────────────────────────────────────────╯
b burner · d drum · f fan · s stirrer · c cooler · +/- gas · ]/[ air · }/{ drum · q quit
```

- Polls every point once per `--interval` (1 s). When the PLC goes away the header turns red and the poller backs off 1 s, 2 s, 4 s, 8 s, 10 s; it reconnects by itself.
- `--time-scale` must match the simulator's `--speed` for RoR to come out in roast °C/min.
- Logs go to `monitor.log` (`--log`, `--log-level`), because the terminal belongs to the dashboard.
- Keys only work against localhost unless you pass `--allow-write`.

### Simulator panel (HTTP :8080)

| Route | What it does |
|---|---|
| `/state` | Full state as JSON, including the **internal truth** (actual bean temperature, moisture), useful to validate the app's event detection |
| `/charge?kg=5&green=25` | Charges coffee (on the real machine this is the hopper lever) |
| `/drop` | Drops the batch |
| `/preheat?temp=200` | Skips the warm-up |
| `/fault?name=flame&on=1` | Simulates a flame failure (the gas valve closes) |
| `/fault?name=emergency&on=1` | Simulates the emergency stop (everything turns off) |
| `/speed?x=30` | Changes the time acceleration |
| `/reset` | Returns to a cold machine |

### Sending commands

```bash
go run ./cmd/probe set drum_on=1 fan_on=1 burner=1 gas_cmd=60
```

Writes are **blocked** when the target is not localhost. For the real machine you must pass `--allow-write` explicitly, and only do so **with someone standing next to the roaster**.

## Against the real machine

```bash
go run ./cmd/probe --url tcp://192.168.5.11:502 --csv roast.csv
```

The PC must be on the `192.168.5.x` network, as shown on page 23 of the manual, or connected through Tailscale.

## ⚠️ The register map is provisional

The addresses in `configs/regmap.json` are **made up**. When the Carmomaq `.aset` file arrives, change `kind`, `addr`, `scale` and `signed` on each point. The `name` values are fixed, because that is how the simulator and the app find each other. No code changes are needed.

## What the model covers (and what it does not)

Points tuned to look like a real roast:
- **Turning point** around 80 °C at about 1:15.
- **RoR** peaking around 25 °C/min, declining to about 8 at the end.
- **First crack** around 196 °C at 8–9 min, with a drop at 210 °C around 10:45.
- **ET dip** at charge.
- Conversion from % to **rpm** and **m³/h** using the table on page 16 of the manual.

Simulated interlocks. These are reasonable assumptions and must be confirmed on the machine:
- The burner only fires with both the drum **and** the fan on.
- A flame failure closes the gas valve.
- The emergency stop turns everything off and refuses to relight the burner.
- The 280 °C safety limit cuts the burner, as on the manual's options screen.

What is not exact:
- The physics is simplified (lumped-parameter model).
- Weight loss stays around 9%; real roasts lose 12–15%, because the model has no organic loss.
- The numbers are good for developing the UI and algorithms, not for predicting a roast.

`go test ./...` checks that the curve stays plausible (turning point range and duration) and that the interlocks work.
