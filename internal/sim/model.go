// Package sim is a simplified thermal model of the Caloratto 5 kg roaster.
// It is not exact physics: it is tuned to produce curves that look like a
// real roast (turning point, declining RoR, ET dip at charge, first crack
// around 196 °C).
package sim

import (
	"fmt"
	"math"
	"math/rand"
	"sync"
)

const (
	pMaxW       = 8886.0 // 7,641 kcal/h (LPG), from the manual
	burnerEff   = 0.80
	drumCap     = 25000.0 // J/K, drum + housing mass
	lossW       = 5.0     // W/K loss through the housing
	exhaustFrac = 0.12    // fraction of hot air that actually carries heat away
	safetyLimit = 280.0   // "Limite segurança" on the HMI options screen
	latentJkg   = 2.26e6
)

// Manual p. 16: fan % -> airflow (m³/h)
var fanTable = [][2]float64{
	{0, 0}, {5, 71}, {10, 76}, {15, 80}, {20, 83}, {25, 86}, {30, 90}, {35, 105},
	{40, 113}, {45, 124}, {50, 136}, {55, 150}, {60, 161}, {65, 178}, {70, 192},
	{75, 203}, {80, 218}, {85, 229}, {90, 240}, {95, 254}, {100, 260}, {105, 271},
	{110, 274}, {115, 278}, {120, 282},
}

// Manual p. 16: drum % -> rpm
var drumTable = [][2]float64{
	{0, 0}, {5, 5}, {10, 8}, {15, 12}, {20, 15}, {25, 17}, {30, 19}, {35, 23}, {40, 26},
	{45, 29}, {50, 33}, {55, 36}, {60, 39}, {65, 42}, {70, 46}, {75, 49}, {80, 52},
	{85, 56}, {90, 59}, {95, 62}, {100, 65}, {105, 68}, {110, 71}, {115, 72}, {120, 73},
}

func interp(t [][2]float64, x float64) float64 {
	if x <= t[0][0] {
		return t[0][1]
	}
	for i := 1; i < len(t); i++ {
		if x <= t[i][0] {
			a, b := t[i-1], t[i]
			return a[1] + (b[1]-a[1])*(x-a[0])/(b[0]-a[0])
		}
	}
	return t[len(t)-1][1]
}

// AirflowM3h converts a fan setting in percent into airflow in m³/h.
func AirflowM3h(pct float64) float64 { return interp(fanTable, pct) }

// DrumRPM converts a drum setting in percent into drum rpm.
func DrumRPM(pct float64) float64 { return interp(drumTable, pct) }

// Event is something notable that happened inside the simulator, at simulated time T.
type Event struct {
	T    float64 `json:"t"`
	Name string  `json:"name"`
	Info string  `json:"info,omitempty"`
}

// Sim is the simulated roaster and its PLC. All methods are safe for concurrent use.
type Sim struct {
	mu sync.Mutex

	// commands (what the PLC receives)
	GasCmd, Drum, Fan, RoastPoint          float64
	Burner, DrumOn, FanOn, Stirrer, Cooler bool

	// injectable faults
	Emergency, FlameFault bool

	// physical state
	Tamb, Tm, Tb, BTprobe, ETprobe, GasFb float64
	Loaded                                bool
	beanKg, waterKg, dryKg, chargeKg      float64
	coolerT                               float64

	// PLC state
	T, chargeAt, lastTimer float64
	running, bell, trip    bool
	hopperUntil            float64

	Events []Event
	rng    *rand.Rand
}

// New returns a cold machine. The seed drives the probe noise.
func New(seed int64) *Sim {
	s := &Sim{rng: rand.New(rand.NewSource(seed))}
	s.reset()
	return s
}

func (s *Sim) reset() {
	*s = Sim{rng: s.rng, Tamb: 25, Tm: 25, Tb: 25, BTprobe: 25, ETprobe: 25, coolerT: 25,
		Drum: 80, Fan: 45, GasCmd: 0, RoastPoint: 210}
}

// Reset returns the machine to a cold, empty state.
func (s *Sim) Reset() { s.mu.Lock(); defer s.mu.Unlock(); s.reset(); s.event("reset", "") }

func (s *Sim) event(name, info string) {
	s.Events = append(s.Events, Event{T: s.T, Name: name, Info: info})
	if len(s.Events) > 200 {
		s.Events = s.Events[len(s.Events)-200:]
	}
}

// Step advances the simulation by dt seconds of simulated time.
func (s *Sim) Step(dt float64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	const h = 0.1
	for dt > 1e-9 {
		d := math.Min(h, dt)
		s.step(d)
		dt -= d
	}
}

func (s *Sim) step(dt float64) {
	s.T += dt

	// Interlocks a roaster PLC typically has.
	if s.Emergency {
		if s.Burner || s.DrumOn || s.FanOn || s.Stirrer || s.Cooler {
			s.event("emergency_stop", "everything off")
		}
		s.Burner, s.DrumOn, s.FanOn, s.Stirrer, s.Cooler = false, false, false, false, false
	}
	if !s.trip && (s.BTprobe > safetyLimit || s.ETprobe > safetyLimit) {
		s.trip = true
		s.event("safety_trip", fmt.Sprintf("BT=%.0f ET=%.0f", s.BTprobe, s.ETprobe))
	} else if s.trip && s.BTprobe < safetyLimit-20 && s.ETprobe < safetyLimit-20 {
		s.trip = false
		s.event("safety_clear", "")
	}
	firing := s.Burner && s.FanOn && s.DrumOn && !s.FlameFault && !s.trip

	// Valve servo: follows the command with lag and a slew-rate limit.
	target := 0.0
	if firing {
		target = clamp(s.GasCmd, 0, 100)
	}
	tau := 1.5
	if !firing {
		tau = 0.4 // safety valves close almost instantly
	}
	step := (target - s.GasFb) * dt / tau
	s.GasFb += clamp(step, -60*dt, 20*dt)
	P := 0.0
	if firing {
		P = s.GasFb / 100 * pMaxW
	}

	// Air
	q := 10.0 // natural draft with the fan off
	if s.FanOn {
		q = AirflowM3h(s.Fan)
	}
	mcp := 1.2 * q / 3600 * 1005 // W/K
	drumF := 0.3
	if s.DrumOn {
		drumF = 0.6 + 0.4*clamp(s.Drum/100, 0, 1.2)
	}

	// Part of the burner heat reaches the beans directly through hot air
	// (convection); the rest heats the drum, which passes it on by contact.
	Pin := burnerEff * P
	Qb, Qconv, Qevap, Qexo := 0.0, 0.0, 0.0, 0.0
	if s.Loaded {
		airF := clamp(q/150, 0.4, 1.3)
		const tGas = 420.0 // nominal temperature of the hot air entering the drum
		Qconv = 0.6 * airF * Pin * clamp(s.beanKg/5, 0.2, 1.2) * clamp((tGas-s.Tb)/(tGas-25), 0, 1)
		ub := (10 + 0.2*mcp) * drumF * (s.beanKg / 5)
		Qb = ub*(s.Tm-s.Tb) + Qconv
		if s.waterKg > 0 && s.Tb > 100 {
			// Simplification: water loss follows the "physical" rate, but its
			// energy cost is capped so the curve does not stall (the model has
			// no temperature gradient inside the bean).
			rate := 5e-5 * (s.Tb - 100) * s.waterKg // kg/s
			Qevap = math.Min(rate*latentJkg, 0.45*math.Max(Qb, 0))
			s.waterKg = math.Max(0, s.waterKg-rate*dt)
		}
		Qexo = 50 * s.dryKg * math.Exp(-((s.Tb-200)/9)*((s.Tb-200)/9))
		cb := s.dryKg*1300 + s.waterKg*4186
		s.Tb += (Qb - Qevap + Qexo) / cb * dt
		s.beanKg = s.dryKg + s.waterKg
	}
	s.Tm += (Pin - Qconv - (exhaustFrac*mcp+lossW)*(s.Tm-s.Tamb) - (Qb - Qconv)) / drumCap * dt

	// Probes (thermocouples) with thermal lag
	etTarget := s.Tamb + (s.Tm-s.Tamb)*0.96
	btTarget := s.Tamb + (s.Tm-s.Tamb)*0.93
	btTau := 20.0
	if s.Loaded {
		etTarget = s.Tb + 0.5*(s.Tm-s.Tb)
		btTarget = s.Tb
		btTau = 38
	}
	s.ETprobe += (etTarget - s.ETprobe) * dt / 10
	s.BTprobe += (btTarget - s.BTprobe) * dt / btTau

	// Cooling tray
	ct := 400.0
	if s.Cooler && s.Stirrer {
		ct = 45
	}
	s.coolerT += (s.Tamb - s.coolerT) * dt / ct

	if s.running && s.RoastPoint > 0 && s.BTprobe >= s.RoastPoint && !s.bell {
		s.bell = true
		s.event("bell", fmt.Sprintf("roast point %.1f °C", s.RoastPoint))
	}
}

// Charge loads kg of green coffee at greenT °C into the drum.
func (s *Sim) Charge(kg, greenT float64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.Loaded {
		return fmt.Errorf("drum already loaded")
	}
	if kg <= 0 || kg > 6 {
		return fmt.Errorf("batch size out of range (0-6 kg)")
	}
	s.Loaded, s.chargeKg, s.beanKg = true, kg, kg
	s.waterKg = kg * 0.11
	s.dryKg = kg - s.waterKg
	s.Tb = greenT
	s.running, s.bell, s.chargeAt = true, false, s.T
	s.hopperUntil = s.T + 3
	s.event("charge", fmt.Sprintf("%.2f kg @ BT %.1f", kg, s.BTprobe))
	return nil
}

// Drop empties the drum into the cooling tray.
func (s *Sim) Drop() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.Loaded {
		return fmt.Errorf("drum is empty")
	}
	loss := (1 - s.beanKg/s.chargeKg) * 100
	s.event("drop", fmt.Sprintf("BT %.1f, %.0fs, weight loss %.1f%%", s.BTprobe, s.T-s.chargeAt, loss))
	s.coolerT = s.Tb
	s.Loaded, s.running = false, false
	s.lastTimer = s.T - s.chargeAt
	return nil
}

// Preheat skips the warm-up (test shortcut only).
func (s *Sim) Preheat(t float64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.Tm, s.BTprobe, s.ETprobe = t+15, t, t+7
	s.event("preheat_shortcut", fmt.Sprintf("%.0f °C", t))
}

// SetFault turns an injectable fault ("emergency" or "flame") on or off.
func (s *Sim) SetFault(name string, on bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	switch name {
	case "emergency":
		s.Emergency = on
	case "flame":
		s.FlameFault = on
	default:
		return fmt.Errorf("unknown fault %q (use emergency|flame)", name)
	}
	s.event("fault_"+name, fmt.Sprint(on))
	return nil
}

func (s *Sim) noise() float64 { return s.rng.NormFloat64() * 0.25 }

// Get returns the value of a regmap point as the PLC would report it.
func (s *Sim) Get(name string) (float64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.get(name)
}

func (s *Sim) getLocked(name string) float64 { v, _ := s.get(name); return v }

func (s *Sim) get(name string) (float64, error) {
	b := func(v bool) float64 {
		if v {
			return 1
		}
		return 0
	}
	switch name {
	case "bt":
		return s.BTprobe + s.noise(), nil
	case "et":
		return s.ETprobe + s.noise(), nil
	case "gas_cmd":
		return s.GasCmd, nil
	case "gas_fb":
		return math.Round(s.GasFb), nil
	case "drum":
		return s.Drum, nil
	case "fan":
		return s.Fan, nil
	case "roast_point":
		return s.RoastPoint, nil
	case "timer_s":
		if s.running {
			return math.Floor(s.T - s.chargeAt), nil
		}
		return math.Floor(s.lastTimer), nil
	case "burner":
		return b(s.Burner), nil
	case "drum_on":
		return b(s.DrumOn), nil
	case "fan_on":
		return b(s.FanOn), nil
	case "stirrer":
		return b(s.Stirrer), nil
	case "cooler":
		return b(s.Cooler), nil
	case "emergency":
		return b(s.Emergency), nil
	case "flame_fault":
		return b(s.FlameFault), nil
	case "hopper_sensor":
		return b(s.T < s.hopperUntil), nil
	case "bell":
		return b(s.bell), nil
	case "safety_trip":
		return b(s.trip), nil
	}
	return 0, fmt.Errorf("unknown point %q", name)
}

// Set applies a write coming from Modbus.
func (s *Sim) Set(name string, v float64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	on := v != 0
	before := fmt.Sprint(s.getLocked(name))
	switch name {
	case "gas_cmd":
		s.GasCmd = clamp(v, 0, 100)
	case "drum":
		s.Drum = clamp(v, 0, 120)
	case "fan":
		s.Fan = clamp(v, 0, 120)
	case "roast_point":
		s.RoastPoint = clamp(v, 0, 260)
	case "burner":
		if on && s.Emergency {
			return fmt.Errorf("emergency stop engaged")
		}
		s.Burner = on
	case "drum_on":
		s.DrumOn = on && !s.Emergency
	case "fan_on":
		s.FanOn = on && !s.Emergency
	case "stirrer":
		s.Stirrer = on && !s.Emergency
	case "cooler":
		s.Cooler = on && !s.Emergency
	default:
		return fmt.Errorf("point %q is not writable", name)
	}
	if fmt.Sprint(s.getLocked(name)) != before {
		s.event("write", fmt.Sprintf("%s=%g", name, v))
	}
	return nil
}

// Snapshot includes the internal "truth" (actual bean temperature, moisture)
// to help validate the app's event detection. The real PLC does not expose it.
type Snapshot struct {
	SimTime      float64 `json:"sim_time_s"`
	BT           float64 `json:"bt"`
	ET           float64 `json:"et"`
	GasCmd       float64 `json:"gas_cmd"`
	GasFb        float64 `json:"gas_fb"`
	Drum         float64 `json:"drum_pct"`
	DrumRPM      float64 `json:"drum_rpm"`
	Fan          float64 `json:"fan_pct"`
	AirflowM3h   float64 `json:"airflow_m3h"`
	Burner       bool    `json:"burner"`
	DrumOn       bool    `json:"drum_on"`
	FanOn        bool    `json:"fan_on"`
	Stirrer      bool    `json:"stirrer"`
	Cooler       bool    `json:"cooler"`
	Loaded       bool    `json:"loaded"`
	Bell         bool    `json:"bell"`
	SafetyTrip   bool    `json:"safety_trip"`
	Emergency    bool    `json:"emergency"`
	FlameFault   bool    `json:"flame_fault"`
	TrueBeanT    float64 `json:"truth_bean_temp"`
	TrueDrumT    float64 `json:"truth_drum_temp"`
	MoisturePct  float64 `json:"truth_moisture_pct"`
	CoolerBeanT  float64 `json:"truth_cooler_bean_temp"`
	RoastElapsed float64 `json:"roast_elapsed_s"`
	Events       []Event `json:"events"`
}

// Snapshot returns a consistent copy of the whole state.
func (s *Sim) Snapshot() Snapshot {
	s.mu.Lock()
	defer s.mu.Unlock()
	m := 0.0
	if s.beanKg > 0 && s.Loaded {
		m = s.waterKg / s.beanKg * 100
	}
	el := 0.0
	if s.running {
		el = s.T - s.chargeAt
	}
	return Snapshot{
		SimTime: s.T, BT: r1(s.BTprobe), ET: r1(s.ETprobe), GasCmd: s.GasCmd, GasFb: r1(s.GasFb),
		Drum: s.Drum, DrumRPM: r1(DrumRPM(s.Drum)), Fan: s.Fan, AirflowM3h: r1(AirflowM3h(s.Fan)),
		Burner: s.Burner, DrumOn: s.DrumOn, FanOn: s.FanOn, Stirrer: s.Stirrer, Cooler: s.Cooler,
		Loaded: s.Loaded, Bell: s.bell, SafetyTrip: s.trip, Emergency: s.Emergency, FlameFault: s.FlameFault,
		TrueBeanT: r1(s.Tb), TrueDrumT: r1(s.Tm), MoisturePct: r1(m), CoolerBeanT: r1(s.coolerT),
		RoastElapsed: r1(el), Events: append([]Event(nil), s.Events...),
	}
}

func clamp(v, lo, hi float64) float64 { return math.Max(lo, math.Min(hi, v)) }
func r1(v float64) float64            { return math.Round(v*10) / 10 }
