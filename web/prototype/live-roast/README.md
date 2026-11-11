# PROTOTYPE: live roast screen (throwaway)

Answers the wayfinder ticket "Live roast screen layout" (jeansflores/roastro#9):
what should the live roast screen look like and how should it behave?
Three structurally different layouts on one page, switchable with `?variant=`:

| Key | Name | Numbers | Marking events and notes | Gas / air |
|---|---|---|---|---|
| A | Cockpit | Rail of big numbers on the right | Click the chart: a menu opens where you clicked and stamps **that** instant | Strip under the chart |
| B | Big screen | Strip of huge numbers across the top | One big "next event" button; clicking the chart opens a note dialog stamped **now** | Inside the chart's bottom band |
| C | Logbook | Values tagged on the head of each line | Click a chart to pin an instant; the timeline's composer marks or notes it | Third stacked chart |
| **D** | **Cockpit + timeline (chosen)** | A's rail, or B's top strip with the "Big numbers" toggle | A's click menu; C's timeline in the rail, its "now" row carries the next mark | A's strip |

**Verdict (2026-11-09):** variant D, with the Big numbers toggle in v1. A, B and C
stay here only as the record of what was compared.

All three replay the same recorded simulator roasts (`src/data/roasts.json`,
regenerate with `go run ./web/prototype/live-roast/gen > web/prototype/live-roast/src/data/roasts.json`
from the repo root). Nothing is persisted and nothing talks to a PLC.

## Run

```bash
make prototype-live     # from the repo root, then open http://localhost:5199
```

To show the chosen screen to someone, open `http://localhost:5199/?demo=1`:
only variant D.

The white bar at the bottom is scaffolding, not design: ← / → switch variants,
space pauses, speed buttons, and "Jump to…" skips to preheat, first crack, after
drop and so on (jumping forward pretends Dry end and First crack were marked).
Shortcuts on every variant: C Charge, D Dry end, F First crack, S Second crack,
X Drop, N note.
