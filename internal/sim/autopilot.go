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

func NewAutopilot() *Autopilot {
	return &Autopilot{ChargeTemp: 200, DropTemp: 210, BatchKg: 5, phase: "start"}
}

func (a *Autopilot) Phase() string { return a.phase }

func (a *Autopilot) Tick(s *Sim) {
	snap := s.Snapshot()
	if snap.Emergency || snap.FlameFault {
		return // injected fault: the "operator" waits for you to clear it
	}
	switch a.phase {
	case "start":
		s.Set("drum_on", 1)
		s.Set("fan_on", 1)
		s.Set("drum", 80)
		s.Set("fan", 35)
		s.Set("gas_cmd", 70)
		s.Set("roast_point", a.DropTemp)
		s.Set("burner", 1)
		a.phase = "preheat"
	case "preheat":
		// hold BT near the charge temperature
		switch {
		case snap.BT < a.ChargeTemp-10:
			s.Set("gas_cmd", 75)
		case snap.BT < a.ChargeTemp:
			s.Set("gas_cmd", 50)
		default:
			s.Set("gas_cmd", 80)
			s.Set("fan", 45)
			s.Charge(a.BatchKg, 25)
			a.phase = "roast"
		}
	case "roast":
		switch {
		case snap.RoastElapsed < 90 || snap.BT < 150: // the probe is misleading until the turning point
			s.Set("gas_cmd", 80)
		case snap.BT < 185:
			s.Set("gas_cmd", 70)
			s.Set("fan", 55)
		default:
			s.Set("gas_cmd", 50)
			s.Set("fan", 65)
		}
		if snap.BT >= a.DropTemp {
			s.Drop()
			s.Set("stirrer", 1)
			s.Set("cooler", 1)
			s.Set("gas_cmd", 40)
			s.Set("fan", 35)
			a.waitUntil = snap.SimTime + 240
			a.phase = "between"
		}
	case "between":
		if snap.SimTime >= a.waitUntil {
			s.Set("stirrer", 0)
			s.Set("cooler", 0)
			a.phase = "preheat"
		}
	}
}
