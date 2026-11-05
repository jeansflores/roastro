// Command sim pretends to be the Caloratto PLC speaking Modbus TCP.
//
//	go run ./cmd/sim --auto --speed 10
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/project-c/internal/regmap"
	"github.com/jeansflores/project-c/internal/sim"
)

// handler translates Modbus requests into reads and writes on the model.
type handler struct {
	m *regmap.Map
	s *sim.Sim
}

func (h *handler) checkUnit(id uint8) error {
	if id != h.m.UnitID && id != 0 && id != 255 {
		return modbus.ErrGWTargetFailedToRespond
	}
	return nil
}

func (h *handler) readBools(k regmap.Kind, addr, qty uint16) []bool {
	out := make([]bool, qty)
	for i := range out {
		if p, ok := h.m.At(k, addr+uint16(i)); ok {
			v, _ := h.s.Get(p.Name)
			out[i] = v != 0
		}
	}
	return out
}

func (h *handler) readRegs(k regmap.Kind, addr, qty uint16) []uint16 {
	out := make([]uint16, qty)
	for i := range out {
		if p, ok := h.m.At(k, addr+uint16(i)); ok {
			v, _ := h.s.Get(p.Name)
			out[i] = p.Encode(v)
		}
	}
	return out
}

func (h *handler) HandleCoils(r *modbus.CoilsRequest) ([]bool, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	if !r.IsWrite {
		return h.readBools(regmap.Coil, r.Addr, r.Quantity), nil
	}
	for i, v := range r.Args {
		p, ok := h.m.At(regmap.Coil, r.Addr+uint16(i))
		if !ok || !p.Writable {
			return nil, modbus.ErrIllegalDataAddress
		}
		val := 0.0
		if v {
			val = 1
		}
		if err := h.s.Set(p.Name, val); err != nil {
			log.Printf("write refused %s: %v", p.Name, err)
			return nil, modbus.ErrServerDeviceFailure
		}
		log.Printf("[%s] coil %s=%v", r.ClientAddr, p.Name, v)
	}
	return nil, nil
}

func (h *handler) HandleDiscreteInputs(r *modbus.DiscreteInputsRequest) ([]bool, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	return h.readBools(regmap.Discrete, r.Addr, r.Quantity), nil
}

func (h *handler) HandleHoldingRegisters(r *modbus.HoldingRegistersRequest) ([]uint16, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	if !r.IsWrite {
		return h.readRegs(regmap.Holding, r.Addr, r.Quantity), nil
	}
	for i, raw := range r.Args {
		p, ok := h.m.At(regmap.Holding, r.Addr+uint16(i))
		if !ok || !p.Writable {
			return nil, modbus.ErrIllegalDataAddress
		}
		if err := h.s.Set(p.Name, p.Decode(raw)); err != nil {
			return nil, modbus.ErrServerDeviceFailure
		}
		log.Printf("[%s] reg %s=%g", r.ClientAddr, p.Name, p.Decode(raw))
	}
	return nil, nil
}

func (h *handler) HandleInputRegisters(r *modbus.InputRegistersRequest) ([]uint16, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	return h.readRegs(regmap.Input, r.Addr, r.Quantity), nil
}

func main() {
	listen := flag.String("modbus", "tcp://0.0.0.0:5502", "Modbus TCP listen address (the real machine uses :502)")
	httpAddr := flag.String("http", ":8080", "simulator HTTP control panel")
	mapPath := flag.String("map", "configs/regmap.json", "register map")
	speed := flag.Float64("speed", 1, "time acceleration (10 = a 10 min roast in 1 min)")
	auto := flag.Bool("auto", false, "autopilot: roast on its own in a loop")
	flag.Parse()

	m, err := regmap.Load(*mapPath)
	if err != nil {
		log.Fatal(err)
	}
	s := sim.New(time.Now().UnixNano())
	var ap *sim.Autopilot
	if *auto {
		ap = sim.NewAutopilot()
	}
	spd := *speed
	var ctl sync.Mutex // guards spd and ap between the physics loop and HTTP

	srv, err := modbus.NewServer(&modbus.ServerConfiguration{
		URL: *listen, Timeout: 60 * time.Second, MaxClients: 8,
	}, &handler{m: m, s: s})
	if err != nil {
		log.Fatal(err)
	}
	if err := srv.Start(); err != nil {
		log.Fatal(err)
	}
	log.Printf("Modbus TCP on %s (unit %d), speed %gx, autopilot=%v", *listen, m.UnitID, spd, *auto)

	// Physics loop: 10 Hz of wall-clock time.
	go func() {
		tk := time.NewTicker(100 * time.Millisecond)
		acc := 0.0
		for range tk.C {
			ctl.Lock()
			s.Step(0.1 * spd)
			acc += 0.1 * spd
			if ap != nil && acc >= 1 { // the operator decides once per simulated second
				ap.Tick(s)
				acc = 0
			}
			ctl.Unlock()
		}
	}()

	mux := http.NewServeMux()
	reply := func(w http.ResponseWriter, err error) {
		w.Header().Set("Content-Type", "application/json")
		if err != nil {
			w.WriteHeader(http.StatusBadRequest)
			json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
			return
		}
		json.NewEncoder(w).Encode(s.Snapshot())
	}
	num := func(r *http.Request, k string, def float64) float64 {
		if v, err := strconv.ParseFloat(r.URL.Query().Get(k), 64); err == nil {
			return v
		}
		return def
	}
	mux.HandleFunc("/state", func(w http.ResponseWriter, r *http.Request) { reply(w, nil) })
	mux.HandleFunc("/charge", func(w http.ResponseWriter, r *http.Request) {
		reply(w, s.Charge(num(r, "kg", 5), num(r, "green", 25)))
	})
	mux.HandleFunc("/drop", func(w http.ResponseWriter, r *http.Request) { reply(w, s.Drop()) })
	mux.HandleFunc("/preheat", func(w http.ResponseWriter, r *http.Request) {
		s.Preheat(num(r, "temp", 200))
		reply(w, nil)
	})
	mux.HandleFunc("/reset", func(w http.ResponseWriter, r *http.Request) {
		ctl.Lock()
		s.Reset()
		if ap != nil {
			ap = sim.NewAutopilot()
		}
		ctl.Unlock()
		reply(w, nil)
	})
	mux.HandleFunc("/fault", func(w http.ResponseWriter, r *http.Request) {
		reply(w, s.SetFault(r.URL.Query().Get("name"), r.URL.Query().Get("on") != "0"))
	})
	mux.HandleFunc("/speed", func(w http.ResponseWriter, r *http.Request) {
		ctl.Lock()
		spd = num(r, "x", spd)
		ctl.Unlock()
		reply(w, nil)
	})
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintln(w, "caloratto-sim: GET /state | /charge?kg=5 | /drop | /preheat?temp=200 | /reset | /fault?name=flame|emergency&on=1|0 | /speed?x=10")
	})
	log.Printf("HTTP panel on http://localhost%s", *httpAddr)
	log.Fatal(http.ListenAndServe(*httpAddr, mux))
}
