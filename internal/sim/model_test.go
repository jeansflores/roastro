package sim

import "testing"

// Guards that the curve still looks like a roast when someone tunes the parameters.
func TestRoastLooksReal(t *testing.T) {
	s := New(1)
	ap := NewAutopilot()
	var chargeAt, tpT, dropAt float64
	tp := 999.0
	charged := false
	for i := 0; i < 2*3600*10; i++ {
		s.Step(0.1)
		if i%10 == 0 {
			ap.Tick(s)
		}
		sn := s.Snapshot()
		if sn.Loaded && !charged {
			charged, chargeAt = true, sn.SimTime
		}
		if charged && sn.Loaded && sn.BT < tp {
			tp, tpT = sn.BT, sn.SimTime-chargeAt
		}
		if charged && !sn.Loaded {
			dropAt = sn.SimTime - chargeAt
			break
		}
	}
	if !charged || dropAt == 0 {
		t.Fatal("autopilot did not complete a roast")
	}
	if tp < 70 || tp > 105 || tpT < 50 || tpT > 120 {
		t.Errorf("turning point out of range: %.1f °C at %.0fs", tp, tpT)
	}
	if dropAt < 480 || dropAt > 900 {
		t.Errorf("roast duration out of range: %.0fs", dropAt)
	}
	t.Logf("TP %.1f °C @ %.0fs, drop @ %.0fs", tp, tpT, dropAt)
}

func TestEmergencyCutsEverything(t *testing.T) {
	s := New(1)
	for _, n := range []string{"drum_on", "fan_on", "burner"} {
		s.Set(n, 1)
	}
	s.Set("gas_cmd", 80)
	s.Step(10)
	s.SetFault("emergency", true)
	s.Step(5)
	sn := s.Snapshot()
	if sn.Burner || sn.FanOn || sn.DrumOn || sn.GasFb > 1 {
		t.Fatalf("emergency stop did not cut power: %+v", sn)
	}
	if err := s.Set("burner", 1); err == nil {
		t.Fatal("should refuse to light the burner during an emergency stop")
	}
}

func TestFlameFaultClosesGas(t *testing.T) {
	s := New(1)
	for _, n := range []string{"drum_on", "fan_on", "burner"} {
		s.Set(n, 1)
	}
	s.Set("gas_cmd", 80)
	s.Step(10)
	s.SetFault("flame", true)
	s.Step(10)
	if s.Snapshot().GasFb > 1 {
		t.Fatal("flame failure did not close the gas valve")
	}
}
