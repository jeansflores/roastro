// Command probe is a minimal Modbus client. It works against both the
// simulator and the real machine.
//
//	go run ./cmd/probe                                   # read and print every 1s
//	go run ./cmd/probe --csv roast.csv                   # also write a CSV
//	go run ./cmd/probe set burner=1 gas_cmd=60           # write (localhost only)
//	go run ./cmd/probe --url tcp://192.168.5.11:502      # real machine, read-only
package main

import (
	"encoding/csv"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/jeansflores/project-c/internal/plc"
	"github.com/jeansflores/project-c/internal/regmap"
	"github.com/jeansflores/project-c/internal/roast"
)

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}

func run() error {
	addr := flag.String("url", "tcp://127.0.0.1:5502", "PLC address (real machine: tcp://192.168.5.11:502)")
	mapPath := flag.String("map", "configs/regmap.json", "register map")
	every := flag.Duration("interval", time.Second, "polling interval")
	csvPath := flag.String("csv", "", "write readings to a CSV file")
	scale := flag.Float64("time-scale", 1, "use the simulator --speed so RoR comes out in simulated °C/min")
	allowWrite := flag.Bool("allow-write", false, "allow writes to a non-localhost target (real machine!)")
	flag.Parse()

	m, err := regmap.Load(*mapPath)
	if err != nil {
		return err
	}
	c := plc.NewClient(plc.Config{URL: *addr, Map: m, Timeout: 2 * time.Second, AllowWrite: *allowWrite})
	defer func() { _ = c.Close() }()

	if flag.Arg(0) == "set" {
		for _, kv := range flag.Args()[1:] {
			if err := write(c, kv); err != nil {
				return fmt.Errorf("%s: %w", kv, err)
			}
			fmt.Println("ok", kv)
		}
		return nil
	}
	return poll(c, m, *every, *csvPath, *scale)
}

func write(c *plc.Client, kv string) error {
	name, val, ok := strings.Cut(kv, "=")
	if !ok {
		return errors.New("use name=value")
	}
	v, err := strconv.ParseFloat(val, 64)
	if err != nil {
		return err
	}
	return c.Write(name, v)
}

func poll(c *plc.Client, m *regmap.Map, every time.Duration, csvPath string, scale float64) error {
	names := make([]string, 0, len(m.Points))
	for _, p := range m.Points {
		names = append(names, p.Name)
	}
	sort.Strings(names)

	var cw *csv.Writer
	if csvPath != "" {
		f, err := os.Create(csvPath)
		if err != nil {
			return err
		}
		defer func() { _ = f.Close() }()
		cw = csv.NewWriter(f)
		if err := cw.Write(append([]string{"time", "ror_bt"}, names...)); err != nil {
			return fmt.Errorf("csv: %w", err)
		}
	}

	start := time.Now()
	ror := roast.NewRoR(30 * time.Second)
	tk := time.NewTicker(every)
	for ; ; <-tk.C {
		r, err := c.Read()
		now := time.Now()
		if err != nil {
			log.Printf("read failed: %v", err)
			continue
		}
		v := r.Values
		ror.Add(time.Duration(float64(now.Sub(start))*scale), v["bt"])
		rate, _ := ror.Value()

		flags := ""
		for _, f := range []string{"burner", "stirrer", "cooler", "bell", "flame_fault", "emergency", "safety_trip"} {
			if v[f] != 0 {
				flags += " " + strings.ToUpper(f)
			}
		}
		fmt.Printf("%s  t=%4.0fs  BT=%6.1f  ET=%6.1f  RoR=%6.1f  gas=%3.0f/%3.0f%%  air=%3.0f%%  drum=%3.0f%%%s\n",
			now.Format("15:04:05"), v["timer_s"], v["bt"], v["et"], rate, v["gas_cmd"], v["gas_fb"], v["fan"], v["drum"], flags)

		if cw != nil {
			row := []string{now.Format(time.RFC3339), strconv.FormatFloat(rate, 'f', 2, 64)}
			for _, n := range names {
				row = append(row, strconv.FormatFloat(v[n], 'f', -1, 64))
			}
			if err := cw.Write(row); err != nil {
				return fmt.Errorf("csv: %w", err)
			}
			cw.Flush()
			if err := cw.Error(); err != nil {
				return fmt.Errorf("csv: %w", err)
			}
		}
	}
}
