// probe: cliente Modbus mínimo. Serve contra o simulador e contra a máquina real.
//
//	go run ./cmd/probe                                   # lê e imprime a cada 1s
//	go run ./cmd/probe --csv torra.csv                   # grava CSV também
//	go run ./cmd/probe set burner=1 gas_cmd=60           # escreve (só localhost)
//	go run ./cmd/probe --url tcp://192.168.5.11:502      # máquina real, só leitura
package main

import (
	"encoding/csv"
	"flag"
	"fmt"
	"log"
	"net"
	"net/url"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/project-c/internal/regmap"
)

func main() {
	addr := flag.String("url", "tcp://127.0.0.1:5502", "CLP (máquina real: tcp://192.168.5.11:502)")
	mapPath := flag.String("map", "configs/regmap.json", "mapa de registradores")
	every := flag.Duration("interval", time.Second, "intervalo de leitura")
	csvPath := flag.String("csv", "", "grava as leituras em CSV")
	scale := flag.Float64("time-scale", 1, "use o mesmo --speed do simulador para o RoR sair em °C/min simulado")
	allowWrite := flag.Bool("allow-write", false, "libera escrita em host que não é localhost (máquina real!)")
	flag.Parse()

	m, err := regmap.Load(*mapPath)
	if err != nil {
		log.Fatal(err)
	}
	if flag.Arg(0) == "set" && !isLocal(*addr) && !*allowWrite {
		log.Fatal("escrita bloqueada: alvo não é localhost. Use --allow-write SÓ com alguém ao lado da máquina.")
	}
	c, err := modbus.NewClient(&modbus.ClientConfiguration{URL: *addr, Timeout: 2 * time.Second})
	if err != nil {
		log.Fatal(err)
	}
	if err := c.Open(); err != nil {
		log.Fatalf("não conectou em %s: %v", *addr, err)
	}
	defer c.Close()
	c.SetUnitId(m.UnitID)

	if flag.Arg(0) == "set" {
		for _, kv := range flag.Args()[1:] {
			if err := write(c, m, kv); err != nil {
				log.Fatalf("%s: %v", kv, err)
			}
			fmt.Println("ok", kv)
		}
		return
	}
	poll(c, m, *every, *csvPath, *scale)
}

func isLocal(u string) bool {
	pu, err := url.Parse(u)
	if err != nil {
		return false
	}
	host, _, _ := net.SplitHostPort(pu.Host)
	ip := net.ParseIP(host)
	return host == "localhost" || (ip != nil && ip.IsLoopback())
}

func write(c *modbus.ModbusClient, m *regmap.Map, kv string) error {
	name, val, ok := strings.Cut(kv, "=")
	if !ok {
		return fmt.Errorf("use nome=valor")
	}
	p, ok := m.ByName(name)
	if !ok || !p.Writable {
		return fmt.Errorf("ponto inexistente ou não gravável")
	}
	v, err := strconv.ParseFloat(val, 64)
	if err != nil {
		return err
	}
	if p.Kind == regmap.Coil {
		return c.WriteCoil(p.Addr, v != 0)
	}
	return c.WriteRegister(p.Addr, p.Encode(v))
}

// readAll lê cada tipo em um bloco contíguo (menos round-trips).
func readAll(c *modbus.ModbusClient, m *regmap.Map) (map[string]float64, error) {
	out := map[string]float64{}
	byKind := map[regmap.Kind][]regmap.Point{}
	for _, p := range m.Points {
		byKind[p.Kind] = append(byKind[p.Kind], p)
	}
	for k, ps := range byKind {
		lo, hi := ps[0].Addr, ps[0].Addr
		for _, p := range ps {
			lo, hi = min(lo, p.Addr), max(hi, p.Addr)
		}
		n := hi - lo + 1
		switch k {
		case regmap.Holding, regmap.Input:
			rt := modbus.HOLDING_REGISTER
			if k == regmap.Input {
				rt = modbus.INPUT_REGISTER
			}
			regs, err := c.ReadRegisters(lo, n, rt)
			if err != nil {
				return nil, fmt.Errorf("%s %d+%d: %w", k, lo, n, err)
			}
			for _, p := range ps {
				out[p.Name] = p.Decode(regs[p.Addr-lo])
			}
		case regmap.Coil, regmap.Discrete:
			var bits []bool
			var err error
			if k == regmap.Coil {
				bits, err = c.ReadCoils(lo, n)
			} else {
				bits, err = c.ReadDiscreteInputs(lo, n)
			}
			if err != nil {
				return nil, fmt.Errorf("%s %d+%d: %w", k, lo, n, err)
			}
			for _, p := range ps {
				if bits[p.Addr-lo] {
					out[p.Name] = 1
				} else {
					out[p.Name] = 0
				}
			}
		}
	}
	return out, nil
}

type sample struct {
	t  time.Time
	bt float64
}

func poll(c *modbus.ModbusClient, m *regmap.Map, every time.Duration, csvPath string, scale float64) {
	names := make([]string, 0, len(m.Points))
	for _, p := range m.Points {
		names = append(names, p.Name)
	}
	sort.Strings(names)

	var cw *csv.Writer
	if csvPath != "" {
		f, err := os.Create(csvPath)
		if err != nil {
			log.Fatal(err)
		}
		defer f.Close()
		cw = csv.NewWriter(f)
		cw.Write(append([]string{"time", "ror_bt"}, names...))
	}

	var hist []sample
	tk := time.NewTicker(every)
	for ; ; <-tk.C {
		v, err := readAll(c, m)
		now := time.Now()
		if err != nil {
			log.Printf("leitura falhou: %v", err)
			continue
		}
		// RoR em °C/min sobre janela de 30s (de tempo simulado)
		hist = append(hist, sample{now, v["bt"]})
		win := time.Duration(float64(30*time.Second) / scale)
		for len(hist) > 2 && now.Sub(hist[1].t) >= win {
			hist = hist[1:]
		}
		ror := 0.0
		if dt := now.Sub(hist[0].t).Minutes() * scale; dt > 0 {
			ror = (v["bt"] - hist[0].bt) / dt
		}
		flags := ""
		for _, f := range []string{"burner", "stirrer", "cooler", "bell", "flame_fault", "emergency", "safety_trip"} {
			if v[f] != 0 {
				flags += " " + strings.ToUpper(f)
			}
		}
		fmt.Printf("%s  t=%4.0fs  BT=%6.1f  ET=%6.1f  RoR=%6.1f  gas=%3.0f/%3.0f%%  ar=%3.0f%%  tambor=%3.0f%%%s\n",
			now.Format("15:04:05"), v["timer_s"], v["bt"], v["et"], ror, v["gas_cmd"], v["gas_fb"], v["fan"], v["drum"], flags)

		if cw != nil {
			row := []string{now.Format(time.RFC3339), strconv.FormatFloat(ror, 'f', 2, 64)}
			for _, n := range names {
				row = append(row, strconv.FormatFloat(v[n], 'f', -1, 64))
			}
			cw.Write(row)
			cw.Flush()
		}
	}
}
