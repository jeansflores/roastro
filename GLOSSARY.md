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
The moment the beans finish drying and turn yellow; judged by eye.
_Avoid_: Yellowing, end of drying

**First crack**:
The start of the first audible cracking of the beans.
_Avoid_: FC (in prose), crack

**Second crack**:
The start of the second, quieter cracking, in darker roasts.
_Avoid_: SC (in prose)

**Drop**:
The moment roasted coffee leaves the drum for the cooling tray; the end of the roast.
_Avoid_: Discharge, dump, end
