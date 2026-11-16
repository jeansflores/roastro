# PROTOTYPE: roast history and curve comparison (throwaway)

Answers jeansflores/roastro#10: how does the roaster find a past roast, compare
curves and choose a Reference roast? One self-contained page, open
`index.html` in a browser. Three structurally different variants, switched from
the white bar at the bottom, the ← / → keys, or `#a`, `#b`, `#c`:

| Key | Name | Finding a roast | Comparing curves | Reference |
|---|---|---|---|---|
| A | Library | List grouped by day, filters (coffee/lot, last 7/30 days, search incl. notes) | Tick roasts in the list to overlay them on the selected one; "Show its reference" | "Use as reference for next roast" on the detail |
| B | Compare table | Sortable metrics table (FC, Dev, DTR, Drop BT, loss, RoR swing) with sparklines | Ticked rows overlay in the big chart, align on Charge or First crack, differences vs. the first ticked | In the roast drawer (click #) |
| C | By coffee | Pick the coffee, its roasts as small multiples | Detail against the coffee's best, the previous roast, or nothing; step older/newer | "Mark as best": the coffee's best is the next reference |

Around the history, the screens the app would have, linked from a sidebar:
**Live roast** (a roast in progress, ticking in real time, also shown in the
sidebar card and pinned at the top of the Library list), **New roast** (the Next
roast with a reference preview), **Coffees** (green coffees, lots, stats, add /
finish a lot, "roast it next") and **Machine** (connection, register map, live
readings; read only). Routes: `#live`, `#a`/`#b`/`#c` (Roasts), `#new`,
`#coffees`, `#machine`.

The roasts are synthetic example data generated in the page; nothing is
persisted and nothing talks to the engine or a PLC.
