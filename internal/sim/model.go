// Package sim é um modelo térmico simplificado da Caloratto 5 kg.
// Não é física exata: é calibrado para gerar curvas com cara de torra real
// (turning point, RoR caindo, dip do ET na carga, 1º crack perto de 196 °C).
package sim

import (
	"fmt"
	"math"
	"math/rand"
	"sync"
)

const (
	pMaxW       = 8886.0 // 7.641 kcal/h (GLP), do manual
	burnerEff   = 0.80
	drumCap     = 25000.0 // J/K, massa do cilindro + carcaça
	lossW       = 5.0     // W/K perda pela carcaça
	exhaustFrac = 0.12    // fração do ar quente que realmente leva calor embora
	safetyLimit = 280.0   // "Limite segurança" da tela de opções
	latentJkg   = 2.26e6
)

// Tabela da pág. 16 do manual: % -> vazão de ar (m³/h)
var fanTable = [][2]float64{
	{0, 0}, {5, 71}, {10, 76}, {15, 80}, {20, 83}, {25, 86}, {30, 90}, {35, 105},
	{40, 113}, {45, 124}, {50, 136}, {55, 150}, {60, 161}, {65, 178}, {70, 192},
	{75, 203}, {80, 218}, {85, 229}, {90, 240}, {95, 254}, {100, 260}, {105, 271},
	{110, 274}, {115, 278}, {120, 282},
}

// Tabela da pág. 16: % -> rpm do cilindro
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

func AirflowM3h(pct float64) float64 { return interp(fanTable, pct) }
func DrumRPM(pct float64) float64    { return interp(drumTable, pct) }

type Event struct {
	T    float64 `json:"t"`
	Name string  `json:"name"`
	Info string  `json:"info,omitempty"`
}

type Sim struct {
	mu sync.Mutex

	// comandos (o que o CLP recebe)
	GasCmd, Drum, Fan, RoastPoint          float64
	Burner, DrumOn, FanOn, Stirrer, Cooler bool

	// falhas injetáveis
	Emergency, FlameFault bool

	// estado físico
	Tamb, Tm, Tb, BTprobe, ETprobe, GasFb float64
	Loaded                                bool
	beanKg, waterKg, dryKg, chargeKg      float64
	coolerT                               float64

	// estado do CLP
	T, chargeAt, lastTimer float64
	running, bell, trip    bool
	hopperUntil            float64

	Events []Event
	rng    *rand.Rand
}

func New(seed int64) *Sim {
	s := &Sim{rng: rand.New(rand.NewSource(seed))}
	s.reset()
	return s
}

func (s *Sim) reset() {
	*s = Sim{rng: s.rng, Tamb: 25, Tm: 25, Tb: 25, BTprobe: 25, ETprobe: 25, coolerT: 25,
		Drum: 80, Fan: 45, GasCmd: 0, RoastPoint: 210}
}

func (s *Sim) Reset() { s.mu.Lock(); defer s.mu.Unlock(); s.reset(); s.event("reset", "") }

func (s *Sim) event(name, info string) {
	s.Events = append(s.Events, Event{T: s.T, Name: name, Info: info})
	if len(s.Events) > 200 {
		s.Events = s.Events[len(s.Events)-200:]
	}
}

// Step avança a simulação dt segundos (tempo simulado).
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

	// Intertravamentos que um CLP de torrador tipicamente tem.
	if s.Emergency {
		if s.Burner || s.DrumOn || s.FanOn || s.Stirrer || s.Cooler {
			s.event("emergency_stop", "tudo desligado")
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

	// Servo da válvula: segue o comando com atraso e limite de velocidade.
	target := 0.0
	if firing {
		target = clamp(s.GasCmd, 0, 100)
	}
	tau := 1.5
	if !firing {
		tau = 0.4 // válvulas de segurança fecham na hora
	}
	step := (target - s.GasFb) * dt / tau
	s.GasFb += clamp(step, -60*dt, 20*dt)
	P := 0.0
	if firing {
		P = s.GasFb / 100 * pMaxW
	}

	// Ar
	q := 10.0 // tiragem natural com ventilador desligado
	if s.FanOn {
		q = AirflowM3h(s.Fan)
	}
	mcp := 1.2 * q / 3600 * 1005 // W/K
	drumF := 0.3
	if s.DrumOn {
		drumF = 0.6 + 0.4*clamp(s.Drum/100, 0, 1.2)
	}

	// Parte do calor do queimador chega no grão direto pelo ar quente
	// (convecção); o resto aquece o cilindro, que passa calor por contato.
	Pin := burnerEff * P
	Qb, Qconv, Qevap, Qexo := 0.0, 0.0, 0.0, 0.0
	if s.Loaded {
		airF := clamp(q/150, 0.4, 1.3)
		const tGas = 420.0 // temperatura nominal do ar quente que entra no tambor
		Qconv = 0.6 * airF * Pin * clamp(s.beanKg/5, 0.2, 1.2) * clamp((tGas-s.Tb)/(tGas-25), 0, 1)
		ub := (10 + 0.2*mcp) * drumF * (s.beanKg / 5)
		Qb = ub*(s.Tm-s.Tb) + Qconv
		if s.waterKg > 0 && s.Tb > 100 {
			// Simplificação: a perda de água segue a taxa "física", mas o custo
			// energético é limitado para não travar a curva (o modelo não tem
			// gradiente interno no grão).
			rate := 5e-5 * (s.Tb - 100) * s.waterKg // kg/s
			Qevap = math.Min(rate*latentJkg, 0.45*math.Max(Qb, 0))
			s.waterKg = math.Max(0, s.waterKg-rate*dt)
		}
		Qexo = 50 * s.dryKg * math.Exp(-math.Pow((s.Tb-200)/9, 2))
		cb := s.dryKg*1300 + s.waterKg*4186
		s.Tb += (Qb - Qevap + Qexo) / cb * dt
		s.beanKg = s.dryKg + s.waterKg
	}
	s.Tm += (Pin - Qconv - (exhaustFrac*mcp+lossW)*(s.Tm-s.Tamb) - (Qb - Qconv)) / drumCap * dt

	// Sondas (termopares) com inércia
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

	// Resfriador
	ct := 400.0
	if s.Cooler && s.Stirrer {
		ct = 45
	}
	s.coolerT += (s.Tamb - s.coolerT) * dt / ct

	if s.running && s.RoastPoint > 0 && s.BTprobe >= s.RoastPoint && !s.bell {
		s.bell = true
		s.event("bell", fmt.Sprintf("ponto de torra %.1f °C", s.RoastPoint))
	}
}

func (s *Sim) Charge(kg, greenT float64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.Loaded {
		return fmt.Errorf("já tem café no cilindro")
	}
	if kg <= 0 || kg > 6 {
		return fmt.Errorf("carga fora da faixa (0-6 kg)")
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

func (s *Sim) Drop() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.Loaded {
		return fmt.Errorf("cilindro vazio")
	}
	loss := (1 - s.beanKg/s.chargeKg) * 100
	s.event("drop", fmt.Sprintf("BT %.1f, %.0fs, perda %.1f%%", s.BTprobe, s.T-s.chargeAt, loss))
	s.coolerT = s.Tb
	s.Loaded, s.running = false, false
	s.lastTimer = s.T - s.chargeAt
	return nil
}

// Preheat pula o aquecimento (só para testes).
func (s *Sim) Preheat(t float64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.Tm, s.BTprobe, s.ETprobe = t+15, t, t+7
	s.event("preheat_shortcut", fmt.Sprintf("%.0f °C", t))
}

func (s *Sim) SetFault(name string, on bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	switch name {
	case "emergency":
		s.Emergency = on
	case "flame":
		s.FlameFault = on
	default:
		return fmt.Errorf("falha desconhecida %q (use emergency|flame)", name)
	}
	s.event("fault_"+name, fmt.Sprint(on))
	return nil
}

func (s *Sim) noise() float64 { return s.rng.NormFloat64() * 0.25 }

// Get devolve o valor "como o CLP reportaria" para um nome do regmap.
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
	return 0, fmt.Errorf("ponto desconhecido %q", name)
}

// Set aplica uma escrita vinda do Modbus.
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
			return fmt.Errorf("emergência acionada")
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
		return fmt.Errorf("ponto %q não é gravável", name)
	}
	if fmt.Sprint(s.getLocked(name)) != before {
		s.event("write", fmt.Sprintf("%s=%g", name, v))
	}
	return nil
}

// Snapshot inclui a "verdade" interna (temp. real do grão, umidade) para
// depurar a detecção de eventos do seu app. O CLP real não expõe isso.
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
