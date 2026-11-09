# Store absolute time; derive the roast clock

Samples and roast events are stored with their wall-clock timestamps, and the roast clock (time since Charge) is computed on read from the Charge event. The obvious alternative, storing seconds since Charge as most roasting tools display them, would make correcting Charge a rewrite of every sample and event in the roast; with absolute time, moving Charge or Drop changes one value and everything else follows. The same rule keeps other derived values (rate of rise, phases, Development time ratio) out of storage.
