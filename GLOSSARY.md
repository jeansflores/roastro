# Roastro

Software that records, shows and eventually conducts coffee roasts on a drum roaster that speaks Modbus TCP.

## Language

**Roast**:
One batch of coffee, from charge to drop. Readings taken before (preheat) and after (cooling) belong to it but are not part of it.
_Avoid_: Batch, batelada, session

**Charge**:
The moment green coffee enters the hot drum; the zero of the roast clock.
_Avoid_: Load, start

**Drop**:
The moment roasted coffee leaves the drum for the cooling tray; the end of the roast.
_Avoid_: Discharge, dump, end
