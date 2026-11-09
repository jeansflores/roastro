// PROTOTYPE, throwaway. Runs the simulator with its autopilot and writes two
// recorded roasts (a reference and a "live" one) as JSON for the live roast
// screen prototype to replay. Run from the repo root:
//
//	go run ./web/prototype/live-roast/gen > web/prototype/live-roast/src/data/roasts.json
package main

import (
	"encoding/json"
	"log"
	"math"
	"os"
	"time"

	"github.com/jeansflores/roastro/internal/roast"
	"github.com/jeansflores/roastro/internal/sim"
)

type recorded struct {
	// Columns: roast clock (s), bt, et, ror (°C/min), gas_fb (%), fan (%), drum (%).
	Samples [][]float64        `json:"samples"`
	DropT   float64            `json:"drop_t"`
	Truth   map[string]float64 `json:"truth"` // roast clock (s) when each event "really" happened
}

func record(seed int64, ap *sim.Autopilot) recorded {
	s := sim.New(seed)
	ror := roast.NewRoR(30 * time.Second)
	var rows [][]float64
	chargeAt, dropAt := -1.0, -1.0
	wasLoaded := false
	for range 4 * 3600 {
		ap.Tick(s)
		s.Step(1)
		snap := s.Snapshot()
		if snap.Loaded && !wasLoaded {
			chargeAt = snap.SimTime
		}
		if !snap.Loaded && wasLoaded {
			dropAt = snap.SimTime
		}
		wasLoaded = snap.Loaded
		bt, _ := s.Get("bt")
		et, _ := s.Get("et")
		ror.Add(time.Duration(snap.SimTime*float64(time.Second)), bt)
		r, _ := ror.Value()
		rows = append(rows, []float64{snap.SimTime, r1(bt), r1(et), r1(r), snap.GasFb, snap.Fan, snap.Drum})
		if dropAt > 0 && snap.SimTime > dropAt+60 {
			break
		}
	}
	if chargeAt < 0 || dropAt < 0 {
		log.Fatal("no complete roast recorded")
	}

	var out recorded
	minBT, tpT := math.Inf(1), 0.0
	out.Truth = map[string]float64{}
	for _, r := range rows {
		t := math.Round(r[0] - chargeAt)
		if t < -120 {
			continue
		}
		row := append([]float64{t}, r[1:]...)
		out.Samples = append(out.Samples, row)
		if t > 0 && t < 240 && r[1] < minBT {
			minBT, tpT = r[1], t
		}
		if t > 120 {
			if _, ok := out.Truth["dry_end"]; !ok && r[1] >= 155 {
				out.Truth["dry_end"] = t
			}
			if _, ok := out.Truth["first_crack"]; !ok && r[1] >= 195 {
				out.Truth["first_crack"] = t
			}
		}
	}
	out.Truth["turning_point"] = tpT
	out.DropT = math.Round(dropAt - chargeAt)
	return out
}

func r1(v float64) float64 { return math.Round(v*10) / 10 }

func main() {
	ref := sim.NewAutopilot()
	live := sim.NewAutopilot()
	live.ChargeTemp, live.DropTemp, live.BatchKg = 206, 213, 4.6

	enc := json.NewEncoder(os.Stdout)
	if err := enc.Encode(map[string]recorded{
		"reference": record(1, ref),
		"live":      record(2, live),
	}); err != nil {
		log.Fatal(err)
	}
}
