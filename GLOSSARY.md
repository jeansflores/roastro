# Roastro

Software that records, shows and eventually conducts coffee roasts on a drum roaster that speaks Modbus TCP.

## Language

### Roasts

**Roast**:
One batch of coffee, from charge to drop, together with a short window of readings before charge and after drop. The cooling tray has no sensor, so nothing after that window belongs to the roast.
_Avoid_: Batch, batelada, session

**Roast clock**:
Time elapsed since charge; zero at charge, negative in the window before it.
_Avoid_: Timer (the PLC's timer is a separate signal)

**Incomplete roast**:
A roast closed without a drop (a new charge started, or the operator closed it); it becomes a normal roast once its drop is marked.
_Avoid_: Unfinished, broken roast

**Discarded roast**:
A roast the operator rejects (a false charge, a test); hidden from the history but never erased.
_Avoid_: Deleted roast

**Gap**:
A stretch of a roast with no readings, because the PLC or Roastro was out; the roast goes on across it.
_Avoid_: Outage, dropout, hole

**Reference roast**:
A past roast chosen to be drawn behind another roast, live or in the history, as the curve to match; usually the best roast of that coffee.
_Avoid_: Background curve, template, profile (a profile is more than a reference)

**Next roast**:
What the operator prepares for the coming roast before charge (coffee, green weight, who roasts, reference roast); it is attached to the roast at charge.
_Avoid_: Pre-roast, setup, draft roast

**Coffee**:
A green coffee as the roaster names it (e.g. "Bourbon Amarelo – Fazenda X"); a roast names the coffee it roasted (filled in before charge or afterwards), and the history is filtered by it.
_Avoid_: Bean, product, lot (a lot is one purchase of a coffee)

**Roast note**:
A free-text comment the operator attaches to an instant of the roast clock (a smell, a color, an observation).
_Avoid_: Annotation, comment, mark

### Roast events

**Roast event**:
A named moment of a roast on the roast clock. Each one is either detected automatically or marked by the operator, and can be corrected later.
_Avoid_: Mark, milestone

**Charge**:
The moment green coffee enters the hot drum; the zero of the roast clock.
_Avoid_: Load, start

**Turning point**:
The lowest bean temperature after charge, where the probe stops falling and starts rising.
_Avoid_: Bottom, TP (in prose)

**Dry end**:
The moment the beans change color (turn yellow), ending the drying phase and starting the Maillard phase; judged by eye.
_Avoid_: Yellowing, end of drying, "Maillard" (that is the phase it starts)

**First crack**:
The first audible pops of the beans; marked at the very first pops, not once cracking is under way.
_Avoid_: FC (in prose), crack

**Second crack**:
The start of the second, quieter cracking, in darker roasts.
_Avoid_: SC (in prose)

**Drop**:
The moment roasted coffee leaves the drum for the cooling tray; the end of the roast.
_Avoid_: Discharge, dump, end

### Roast phases

**Drying phase**:
The part of a roast from charge to dry end.
_Avoid_: Drying stage

**Maillard phase**:
The part of a roast from dry end to first crack, while the beans brown.
_Avoid_: Browning phase, Maillard reaction (in this sense)

**Development phase**:
The part of a roast from first crack to drop.
_Avoid_: Development stage, post-crack

**Development time ratio**:
The development phase's share of the whole roast time, as a percentage.
_Avoid_: DTR (in prose), development percentage
