# Roadmap

Grow in small, testable steps, the way M0 was built: each milestone works on its own and is tried against the simulator (or the machine) before the next one starts. The first goal is software that works for **our** roaster. Selling it is a direction the architecture must not block, not work for now.

## The loop

The features are not a list; they feed each other. This came from the roaster who runs our machine:

```
 Inventory ──▶ Profile ──▶ Roast ──▶ History ──▶ Cupping
 (green coffee) (recipe A/B/C) (live curve) (saved curve) (tasting)
    ▲                                                      │
    └──────────── adjust the profile from the tasting ◀────┘
```

- **Profiles** turn an experienced roaster's feel ("roast A, B or C for this bean") into something the system can show, compare against and eventually follow.
- **History** matters because a tasting only teaches something when it is linked to the curve of that exact batch: taste it, dislike it, open the curve and see what was done.
- **Automation** is the end goal ("a meta é automatizar isso"). The machine already has some, but it is very limited.

## Milestones

| | Milestone | Delivers | Needs the machine? |
|---|---|---|---|
| **M0** | Link proven | Simulator, `probe`, terminal `monitor`; reads, writes and reconnection validated | ✅ done |
| **M1** | Real machine, read-only | Real register map from the Carmomaq `.aset`; values match the machine's own panel | yes: blocked on the `.aset` |
| **M2** | Record roasts | Every roast saved (SQLite): curve, events (charge, TP, dry end, first crack, drop), green/roasted weight, notes | no |
| **M3** | UI: live + history | Web UI served by the Go backend: live roast screen with a reference roast drawn behind it, roast library, curves compared side by side | no |
| **M4** | Profiles | Named recipes per coffee and roast level; live deviation alerts; auto-marking events by temperature | no |
| **M5** | Inventory | Green coffee lots in; stock deducted automatically per batch from the charge weight | no |
| **M6** | Cupping | Tasting scores and notes attached to a roast, opening its curve | no |
| **M7** | Automation | The app follows a profile and adjusts gas and air by itself | yes, and it is the riskiest step |
| **M8** | Commercial | Several machines and models, remote updates, optional cloud | — |

### Notes

- **Only M1 waits on the machine.** M2 to M6 can be built and tested entirely against the simulator, so waiting for the `.aset` blocks nothing.
- **M7 comes last on purpose.** It is closed-loop writing to a gas roaster. It needs trustworthy profiles (M4), plenty of recorded roasts (M2) and an "armed" control mode tested to exhaustion, with someone next to the machine. The write rules in `CLAUDE.md` apply in full.
- **Before M7, ask the roaster** what the machine's built-in automation does today (roast point / bell?) and where it falls short. That becomes the M7 requirements.
- **M8 shapes decisions, not work.** Keep the register map per machine model as data, keep `plc` separate from the UI, keep writes local-only. Details of distribution and updates live in the architecture notes when we get there.

## Open questions

- **Call with the roaster about interventions per phase.** He adjusts gas and air within the windows of three chemical reactions in three stages and prefers to explain it live. Feeds M4 (profiles) and M7 (automation).
- **Secure the name.** The product is called **Roastro** (roast + maestro: "conduct every roast"). Before investing in a brand, register a domain (`roastro.app`, `roastro.coffee` and `roastro.com.br` looked free on 2026-11-05; `roastro.com` is taken) and search INPI classes 9 and 42. Jean is aligning this with his partner first.
- What does the built-in automation do, and what is missing? (for M7)
- Which cupping protocol does the roaster use (SCA form, a simpler in-house sheet)? (for M6)
- How are green coffee lots identified today (supplier, farm, lot code, bag weight)? (for M5)
- More feature ideas from Jean are still to come; add them to the table or to this list.
