# Project context

Written on 2026-11-04 from an earlier claude.ai conversation, to continue the work in Claude Code. Primary sources live in [`reference/`](reference/):

- `operation-manual-rev01.pdf`: "Manual técnico de operação e manutenção – Caloratto 5 e 10 kg", rev. 01, April 2024.
- `electrical-schematic-weg.pdf`: "CALORATTO - COEL", Actual Soluções em Tecnologia (WEG), 2020-07-14.

## 1. Goal

Jean owns a Carmomaq Caloratto (5 kg) coffee roaster. He used Cropster (paid; the subscription lapsed) and knows Artisan (open source, Python, GPLv3). The goal is our own software to monitor and control the machine, with the charts and settings Artisan lacks.

- Stack: Rust backend (Modbus polling, persistence) and a TypeScript/React frontend, shipped as one binary that serves the UI on localhost (no internet needed); the first tools were written in Go and rewritten in Rust (ADR 0002). A desktop wrapper (Tauri) may come later; Electron is ruled out. Main target: Windows.
- Visual reference: perssua.com (desktop app with its own window, dark and clean). Roast screen: the chart dominates, big numbers on the side.
- Jean is travelling. Development happens against the simulator. At times a colleague will be at the roastery and can connect the real machine for tests.

## 2. The machine

Specifications:

- Caloratto 5 kg: 5 kg per batch, 20 kg/h, 7,641 kcal/h (LPG) ≈ 8.9 kW.
- Caloratto 10 kg: 10 kg per batch, 40 kg/h, 11,462 kcal/h (LPG).
- 220 V single phase. Gas at 30–100 mbar.

### Communication: Ethernet, Modbus TCP (not USB)

- The PLC is at `192.168.5.11`.
- The PC needs a static IP on the same network. The manual (p. 23) uses `192.168.5.2 / 255.255.255.0`, gateway `192.168.5.11`.
- In Cropster the connector is "Carmomaq PLC" with host `192.168.5.11`. Channels shown: bean temp (ch0), exhaust (ch1), drum speed (ch2), airflow (ch3) and gas %.
- According to Cropster's public docs, Caloratto/Masteratto machines built before 2020 used "Generic Modbus over Network (TCP)" with default IP `192.168.0.10`. Jean's machine follows the manual: `192.168.5.11`.
- Carmomaq's Artisan setup has buttons to turn the stirrer, burner ("power"), fan and drum on and off, and sliders for air, power and drum. The `.aset` settings file is linked on p. 25 of the manual: <https://drive.google.com/file/d/1VOpkJgTtUojalDRGbpgMkGVFJRgIp4W9/view>.

### Internal architecture (from the electrical schematic)

- Central COEL PLC.
  - AI1: bean thermocouple (BT), terminals 18 and 19.
  - AI2: air/exhaust thermocouple (ET), terminals 20 and 21.
  - AI3: gas valve servo position feedback.
  - AO1: 0–10 V to the gas valve servo (burner power).
- Inverters: INV1 drum, INV2 fan, INV3 stirrer.
  - Drum and fan sit on an internal RS-485 bus (PLC COM2, shared with the HMI). This does not affect the software: the PC only talks to the PLC over TCP.
- Digital inputs (X6): D01 emergency stop, D02 flame failure, D03 hopper sensor, D06 stirrer, D07 cooler.
- Outputs (X7): D13 bell, D15 burner.
- INOVA module: hardware flame controller (flame sensor, ignition, gas valve). It cuts the gas on its own, independently of any software.
- HMI options screen:
  - "Limite segurança 280 °C" (safety limit);
  - automatic cooling;
  - stirrer and cooler timers;
  - recipes stored in the PLC: up to 50, switching burner, fan and drum % by temperature band, plus an "AUTO MODE".

### Manual p. 16: setting % → drum rpm and airflow (m³/h)

| % | rpm | m³/h | | % | rpm | m³/h |
|---|---|---|---|---|---|---|
| 5 | 5 | 71 | | 65 | 42 | 178 |
| 10 | 8 | 76 | | 70 | 46 | 192 |
| 15 | 12 | 80 | | 75 | 49 | 203 |
| 20 | 15 | 83 | | 80 | 52 | 218 |
| 25 | 17 | 86 | | 85 | 56 | 229 |
| 30 | 19 | 90 | | 90 | 59 | 240 |
| 35 | 23 | 105 | | 95 | 62 | 254 |
| 40 | 26 | 113 | | 100 | 65 | 260 |
| 45 | 29 | 124 | | 105 | 68 | 271 |
| 50 | 33 | 136 | | 110 | 71 | 274 |
| 55 | 36 | 150 | | 115 | 72 | 278 |
| 60 | 39 | 161 | | 120 | 73 | 282 |

### Normal operation (manual)

1. Heat to 200 °C before the first batch.
2. Press START on the timer and open the hopper (mechanical lever).
3. Adjust gas and air during the roast.
4. Drop: stirrer on, then open the drum door (mechanical).
5. Cool in the cooling tray.
6. Never switch the machine off with the beans above 60 °C.

**Charge and drop are mechanical.** The PLC does not move the coffee. Artisan's Caloratto 2.0 preset does tell the PLC about them (holding 50: write 1 at charge, 2 at drop), but whether the PLC exposes them for reading is unknown, so the app cannot count on knowing them on its own. Either the user presses a Charge/Drop button in the app (as in Cropster and Artisan), or the app detects them: charge from the sharp BT drop, drop from the behavior afterwards. Both can be done.

### Register map (from Artisan's Caloratto 2.0 preset, 2026-11-12)

Modbus TCP, port 502, unit 1. Everything is a holding register. Not yet checked on the machine.

| Holding | What | Confidence |
|---|---|---|
| 43, 44 | the two temperatures, tenths of °C, unsigned | high; which is BT is unconfirmed (Artisan's first input is usually ET, so 43 = ET, 44 = BT). At charge BT drops far more than ET. |
| 45 | burner power, 0–100 %, read and written | high |
| 46 | drum speed, 0–120 %, read and written | high (matches the manual's 0–120 table) |
| 47 | fan speed, 0–120 %, read and written | high |
| 50 | write 1 at charge, 2 at drop | medium; readability unknown |
| 55, 56, 57 | burner, fan, drum on/off: write 1 = on, 2 = off | medium (from button labels) |
| 52, 58 | two more on/off commands, likely stirrer and cooler | low |
| 60 | pressure fan on/off (hidden button) | low |
| 49, 59 | a disabled fourth channel ("damper") | low |

Not in the preset: gas valve feedback, roast point, the PLC timer and the alarm inputs (emergency, flame failure, hopper sensor, safety limit).

What this means for `configs/regmap.json`:

- On/off commands are holding registers with 1 = on, 2 = off, not coils with 1/0. The map format cannot express that yet.
- Command registers may not be readable. Polling them in the same block read as 43–47 could fail every read, so they need a "write-only" marking before they enter the map. Artisan itself only reads 43–47.
- Until those are settled, `configs/regmap.caloratto-2.0.json` holds the read-only part (43–47) for the first session on the machine and for running the simulator with the real register layout: `make run-sim MAP=configs/regmap.caloratto-2.0.json`.

## 3. Useful external resources

- Artisan repo: <https://github.com/artisan-roaster-scope/artisan>.
  - PR #1233 (2023), sent by Carmomaq's automation lead, brought updated `.aset` setups for post-2020 Carmomaq machines. They live in `src/includes/Machines/Carmomaq/`.
  - `Caloratto_2.0.aset` matches this machine (host `192.168.5.11`); see "Register map" below. `Caloratto-Materatto_Legacy.aset` is for pre-2020 machines (host `192.168.0.10`, BT/ET only at holding 8000/8001) and does not.
  - The `.aset` files are not copied into this repository (Artisan is GPLv3); read them there.
- <https://github.com/turicas/carmomaq>: utilities to collect data from and control Carmomaq roasters.
- License: Artisan is GPLv3. Reading the `.aset` files to learn the map is fine. Copying Artisan code would make this project GPL. The plan is to write our own code.

## 4. Remote access (decision)

- **Never expose port 502 to the internet.** Modbus has no authentication.
- Plan: a Raspberry Pi (or mini PC) next to the machine, wired into `192.168.5.x`, online via Wi-Fi or 4G, running Tailscale as a subnet router (`sudo tailscale up --advertise-routes=192.168.5.0/24`, then approve the route in the admin panel).
- Quick alternative: install Tailscale or RDP on the roasting PC that already lives there.
- The PLC only answers with the machine's main switch on.
- **Safety rule: remote access is read-only.** Writes (gas, burner, etc.) only with someone physically next to the machine. It is a 5 kg gas roaster, and the fire procedure requires a person present.

## 5. What exists so far

A PLC simulator and a probe client, described in the [README](../README.md). Modbus TCP server and client: `tokio-modbus`.

Two channels in the simulator:

- **Modbus (:5502)**: the operator or the app commanding the machine. Same channel as the real machine.
- **HTTP (:8080)**: the "physical world" and shortcuts that do not exist on the machine (`/state`, `/charge`, `/drop`, `/preheat`, `/fault`, `/speed`, `/reset`). `/state` also returns the internal truth (`truth_bean_temp`, `truth_moisture_pct`) to validate the app's event detection.

Register map points (the **names** are fixed; `addr`/`kind`/`scale` are made up):

- holding: `bt`(0, ×10, signed), `et`(1, ×10, signed), `gas_cmd`(2, W), `gas_fb`(3), `drum`(4, W), `fan`(5, W), `roast_point`(6, ×10, W), `timer_s`(7)
- coil: `burner`(0), `drum_on`(1), `fan_on`(2), `stirrer`(3), `cooler`(4), all writable
- discrete: `emergency`(0), `flame_fault`(1), `hopper_sensor`(2), `bell`(3), `safety_trip`(4)

The real register layout is known from Artisan's Caloratto 2.0 preset (see "Register map" above). Moving these names to it needs two format additions first (1/2 on/off encoding, write-only points); the read-only part is in `configs/regmap.caloratto-2.0.json`.

### Thermal model

Lumped parameters, tuned to look real:

- Nodes: drum temperature (Tm) and bean temperature (Tb). 11% moisture evaporates above 100 °C. Exothermic bump near 200 °C.
- Heat: part of the burner output reaches the beans by convection (depends on airflow); the rest heats the drum, which passes it on by contact (depends on rpm).
- Probes with lag: BT τ ≈ 38 s with coffee, ET τ ≈ 10 s, ±0.25 °C noise.
- Gas servo with lag; valves close fast without flame.
- Autopilot result: TP ≈ 81 °C at ~1:15, RoR peaking ~25 °C/min and falling to ~8, ~196 °C at 8–9 min, drop at 210 °C at ~10:45, final moisture ~2%, weight loss ~9% (real: 12–15%; no organic loss in the model).

**Assumed interlocks** (to confirm on the real machine):

- burner only with drum and fan on;
- emergency stop turns everything off and refuses to relight the burner;
- flame failure closes the gas;
- the 280 °C limit cuts the burner; it clears below 260 °C;
- bell when BT ≥ roast point.

## 6. Next steps

Agreed:

- Start the app on top of the simulator. First milestone: a terminal monitor (`crates/monitor`) proving that the Modbus link works end to end, including reconnection and writes.

Proposed, not yet decided:

- A `scenario.json` for the simulator with three blocks:
  - machine: power, inertia, probe noise, a Caloratto 10 kg preset;
  - coffee: moisture, density, green temperature;
  - faults: loose or stuck thermocouple, spikes, latency, timeouts, connection dropping mid-roast (it will happen over Tailscale), PLC refusing writes.

Ideas for the app:

1. Rust backend: 1 Hz polling, time series in SQLite, automatic reconnection, events (charge, TP, DE, first crack, drop), both manual and auto-detected.
2. Calculations: RoR (configurable window, smoothing), phases (drying, Maillard, development), DTR, background reference curve, comparing roasts.
3. Show rpm and m³/h next to % (table above).
4. TS frontend: live roast screen, history, profiles.
5. Control (writes) later, with confirmation in the UI and blocked over remote access.

First test on the real machine (when the colleague is there):

- Read-only, from a PC on `192.168.5.x`: `cargo run -q -p probe -- --url tcp://192.168.5.11:502 --map configs/regmap.caloratto-2.0.json --csv first-roast.csv`. Writes to a non-local target are blocked, and that map has no writable points.
- Compare the values with the machine's own panel, and charge once to tell BT from ET.
- The CSV also helps calibrate the simulator.
- Do not let AI agents or other tools run commands on the machine's network: Modbus has no authentication and this is a gas roaster.

## Appendix A: open questions

- Register map details still open: which of holding 43/44 is BT, whether command registers (50, 52, 55–60) can be read, and where (if anywhere) the PLC exposes gas valve feedback, the timer and the alarms.
- Confirm the PLC's real interlocks.
- Exact year of the machine (the schematic is from 2020-07; the manual is rev. 2024).
