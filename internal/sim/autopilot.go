package sim

// Autopilot plays the operator: it preheats, charges, adjusts gas and air by
// temperature band, drops and starts over. It keeps data flowing into the app
// with nobody at the controls.
type Autopilot struct {
	ChargeTemp float64 // BT at which to charge
	DropTemp   float64 // BT at which to drop
	BatchKg    float64
	phase      string
	waitUntil  float64
}

// NewAutopilot returns an autopilot that charges 5 kg at 200 °C and drops at 210 °C.
func NewAutopilot() *Autopilot {
	return &Autopilot{ChargeTemp: 200, DropTemp: 210, BatchKg: 5, phase: "start"}
}

// Phase reports the current step of the roast cycle.
func (a *Autopilot) Phase() string { return a.phase }

// Tick makes one operator decision. Call it once per simulated second.
func (a *Autopilot) Tick(s *Sim) {
	snap := s.Snapshot()
	if snap.Emergency || snap.FlameFault {
		return // injected fault: the "operator" waits for you to clear it
	}
	switch a.phase {
	case "start":
		set(s, "drum_on", 1)
		set(s, "fan_on", 1)
		set(s, "drum", 80)
		set(s, "fan", 35)
		set(s, "gas_cmd", 70)
		set(s, "roast_point", a.DropTemp)
		set(s, "burner", 1)
		a.phase = "preheat"
	case "preheat":
		// hold BT near the charge temperature
		switch {
		case snap.BT < a.ChargeTemp-10:
			set(s, "gas_cmd", 75)
		case snap.BT < a.ChargeTemp:
			set(s, "gas_cmd", 50)
		default:
			set(s, "gas_cmd", 80)
			set(s, "fan", 45)
			if s.Charge(a.BatchKg, 25) == nil {
				a.phase = "roast"
			}
		}
	case "roast":
		switch {
		case snap.RoastElapsed < 90 || snap.BT < 150: // the probe is misleading until the turning point
			set(s, "gas_cmd", 80)
		case snap.BT < 185:
			set(s, "gas_cmd", 70)
			set(s, "fan", 55)
		default:
			set(s, "gas_cmd", 50)
			set(s, "fan", 65)
		}
		if snap.BT >= a.DropTemp && s.Drop() == nil {
			set(s, "stirrer", 1)
			set(s, "cooler", 1)
			set(s, "gas_cmd", 40)
			set(s, "fan", 35)
			a.waitUntil = snap.SimTime + 240
			a.phase = "between"
		}
	case "between":
		if snap.SimTime >= a.waitUntil {
			set(s, "stirrer", 0)
			set(s, "cooler", 0)
			a.phase = "preheat"
		}
	}
}

// set writes a point and ignores the result. The only refusal Sim.Set can
// produce for these points is the emergency stop, which Tick checks first.
func set(s *Sim, name string, v float64) { _ = s.Set(name, v) }
