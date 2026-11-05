package sim

import "testing"

// Garante que a curva continua com "cara de torra" se alguém mexer nos parâmetros.
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
		t.Fatal("autopilot não completou uma torra")
	}
	if tp < 70 || tp > 105 || tpT < 50 || tpT > 120 {
		t.Errorf("turning point fora do esperado: %.1f °C em %.0fs", tp, tpT)
	}
	if dropAt < 480 || dropAt > 900 {
		t.Errorf("duração da torra fora do esperado: %.0fs", dropAt)
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
		t.Fatalf("emergência não cortou: %+v", sn)
	}
	if err := s.Set("burner", 1); err == nil {
		t.Fatal("deveria recusar ligar queimador em emergência")
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
		t.Fatal("falta de chama não fechou o gás")
	}
}
