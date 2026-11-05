// Command sim pretends to be the Caloratto PLC speaking Modbus TCP.
//
//	go run ./cmd/sim --auto --speed 10
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"log/slog"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/roastro/internal/regmap"
	"github.com/jeansflores/roastro/internal/sim"
)

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
	}, sim.NewModbusHandler(m, s, slog.Default()))
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
		var body any = s.Snapshot()
		if err != nil {
			w.WriteHeader(http.StatusBadRequest)
			body = map[string]string{"error": err.Error()}
		}
		if err := json.NewEncoder(w).Encode(body); err != nil {
			log.Printf("http: encode response: %v", err)
		}
	}
	num := func(r *http.Request, k string, def float64) float64 {
		if v, err := strconv.ParseFloat(r.URL.Query().Get(k), 64); err == nil {
			return v
		}
		return def
	}
	mux.HandleFunc("/state", func(w http.ResponseWriter, _ *http.Request) { reply(w, nil) })
	mux.HandleFunc("/charge", func(w http.ResponseWriter, r *http.Request) {
		reply(w, s.Charge(num(r, "kg", 5), num(r, "green", 25)))
	})
	mux.HandleFunc("/drop", func(w http.ResponseWriter, _ *http.Request) { reply(w, s.Drop()) })
	mux.HandleFunc("/preheat", func(w http.ResponseWriter, r *http.Request) {
		s.Preheat(num(r, "temp", 200))
		reply(w, nil)
	})
	mux.HandleFunc("/reset", func(w http.ResponseWriter, _ *http.Request) {
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
	mux.HandleFunc("/", func(w http.ResponseWriter, _ *http.Request) {
		_, _ = fmt.Fprintln(w, "caloratto-sim: GET /state | /charge?kg=5 | /drop | /preheat?temp=200 | /reset | /fault?name=flame|emergency&on=1|0 | /speed?x=10")
	})
	log.Printf("HTTP panel on http://localhost%s", *httpAddr)
	log.Fatal(http.ListenAndServe(*httpAddr, mux))
}
