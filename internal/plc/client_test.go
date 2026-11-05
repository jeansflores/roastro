package plc

import (
	"errors"
	"fmt"
	"log/slog"
	"net"
	"testing"
	"time"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/project-c/internal/regmap"
	"github.com/jeansflores/project-c/internal/sim"
)

func loadMap(t *testing.T) *regmap.Map {
	t.Helper()
	m, err := regmap.Load("../../configs/regmap.json")
	if err != nil {
		t.Fatal(err)
	}
	return m
}

// freeURL returns a Modbus URL on a loopback port that was free a moment ago.
func freeURL(t *testing.T) string {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = l.Close() }()
	return fmt.Sprintf("tcp://%s", l.Addr())
}

// serve starts a Modbus server for s on url. Stop it with the returned func.
func serve(t *testing.T, url string, m *regmap.Map, s *sim.Sim) (stop func()) {
	t.Helper()
	srv, err := modbus.NewServer(&modbus.ServerConfiguration{URL: url, Timeout: time.Minute, MaxClients: 4},
		sim.NewModbusHandler(m, s, slog.New(slog.DiscardHandler)))
	if err != nil {
		t.Fatal(err)
	}
	if err := srv.Start(); err != nil {
		t.Fatal(err)
	}
	stopped := false
	stop = func() {
		if !stopped {
			stopped = true
			_ = srv.Stop()
		}
	}
	t.Cleanup(stop)
	return stop
}

func TestClientReadWrite(t *testing.T) {
	m, s, url := loadMap(t), sim.New(1), freeURL(t)
	serve(t, url, m, s)
	c := NewClient(Config{URL: url, Map: m})
	t.Cleanup(func() { _ = c.Close() })

	r, err := c.Read()
	if err != nil {
		t.Fatal(err)
	}
	if bt := r.Values["bt"]; bt < 23 || bt > 27 {
		t.Errorf("bt = %v on a cold machine; want about 25", bt)
	}

	for name, v := range map[string]float64{"drum_on": 1, "gas_cmd": 60} {
		if err := c.Write(name, v); err != nil {
			t.Fatalf("Write(%s): %v", name, err)
		}
	}
	snap := s.Snapshot()
	if !snap.DrumOn || snap.GasCmd != 60 {
		t.Errorf("sim state after writes: drum_on=%v gas_cmd=%v; want true, 60", snap.DrumOn, snap.GasCmd)
	}
}

func TestClientWriteErrors(t *testing.T) {
	m, s, url := loadMap(t), sim.New(1), freeURL(t)
	serve(t, url, m, s)
	c := NewClient(Config{URL: url, Map: m})
	t.Cleanup(func() { _ = c.Close() })

	if err := c.Write("nope", 1); err == nil {
		t.Error("Write(unknown point) = nil; want error")
	}
	if err := c.Write("bt", 100); err == nil {
		t.Error("Write(read-only point) = nil; want error")
	}

	// The simulator refuses to light the burner during an emergency stop.
	if err := s.SetFault("emergency", true); err != nil {
		t.Fatal(err)
	}
	if err := c.Write("burner", 1); !errors.Is(err, modbus.ErrServerDeviceFailure) {
		t.Errorf("Write(burner) during emergency = %v; want ErrServerDeviceFailure", err)
	}
	if _, err := c.Read(); err != nil {
		t.Errorf("Read after a refused write = %v; the connection should survive an exception", err)
	}
}

func TestClientBlocksRemoteWrites(t *testing.T) {
	m := loadMap(t)
	tests := []struct {
		url        string
		allowWrite bool
		want       bool
	}{
		{"tcp://127.0.0.1:5502", false, true},
		{"tcp://localhost:5502", false, true},
		{"tcp://[::1]:5502", false, true},
		{"tcp://192.168.5.11:502", false, false},
		{"tcp://192.168.5.11:502", true, true},
		{"tcp://roaster.tailnet.ts.net:502", false, false},
	}
	for _, tt := range tests {
		c := NewClient(Config{URL: tt.url, Map: m, AllowWrite: tt.allowWrite})
		if got := c.CanWrite(); got != tt.want {
			t.Errorf("CanWrite(%s, allow=%v) = %v; want %v", tt.url, tt.allowWrite, got, tt.want)
		}
		if !tt.want {
			// Must refuse before ever dialing the machine.
			if err := c.Write("burner", 1); !errors.Is(err, ErrWriteBlocked) {
				t.Errorf("Write(%s) = %v; want ErrWriteBlocked", tt.url, err)
			}
		}
	}
}

func TestClientReconnects(t *testing.T) {
	m, s, url := loadMap(t), sim.New(1), freeURL(t)
	stop := serve(t, url, m, s)
	c := NewClient(Config{URL: url, Map: m, Timeout: 500 * time.Millisecond})
	t.Cleanup(func() { _ = c.Close() })

	if _, err := c.Read(); err != nil {
		t.Fatalf("first read: %v", err)
	}

	stop() // the PLC goes away (power, cable, Tailscale...)
	if _, err := c.Read(); err == nil {
		t.Fatal("read with the server down = nil; want error")
	}

	serve(t, url, m, s) // and comes back on the same address
	if _, err := c.Read(); err != nil {
		t.Fatalf("read after the server came back: %v", err)
	}
}
