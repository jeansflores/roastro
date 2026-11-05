// Package plc talks to the roaster PLC over Modbus TCP by point name. Callers
// never see addresses: the register map translates names into Modbus tables.
package plc

import (
	"errors"
	"fmt"
	"net"
	"net/url"
	"sync"
	"time"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/roastro/internal/regmap"
)

// ErrWriteBlocked is returned by Client.Write when the target is not a
// loopback address and writes were not explicitly allowed.
var ErrWriteBlocked = errors.New("plc: write blocked: target is not localhost; opt in only with someone next to the machine")

// Config configures a Client.
type Config struct {
	URL     string // e.g. tcp://127.0.0.1:5502 or tcp://192.168.5.11:502
	Map     *regmap.Map
	Timeout time.Duration // per request; defaults to 1s

	// AllowWrite permits writes to a non-loopback target. Leave it false
	// unless someone is standing next to the machine.
	AllowWrite bool
}

// Reading is one snapshot of every point in the register map.
type Reading struct {
	Values  map[string]float64
	Latency time.Duration // time spent on the Modbus round trips
}

// Client reads and writes PLC points by name. It connects lazily and drops
// the connection after a transport error, so the next call reconnects. It is
// safe for concurrent use.
type Client struct {
	cfg Config

	mu sync.Mutex
	mb *modbus.ModbusClient // nil while disconnected
}

// NewClient returns a Client for cfg. It does not connect yet.
func NewClient(cfg Config) *Client {
	if cfg.Timeout == 0 {
		cfg.Timeout = time.Second
	}
	return &Client{cfg: cfg}
}

// CanWrite reports whether Write would be allowed to reach the target.
func (c *Client) CanWrite() bool { return c.cfg.AllowWrite || IsLoopback(c.cfg.URL) }

// Read fetches every point in the register map.
func (c *Client) Read() (Reading, error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	start := time.Now()
	if err := c.connectLocked(); err != nil {
		return Reading{}, err
	}
	values, err := readAll(c.mb, c.cfg.Map)
	if err != nil {
		c.dropLocked()
		return Reading{}, err
	}
	return Reading{Values: values, Latency: time.Since(start)}, nil
}

// Write sets point name to v, in physical units (1/0 for coils).
func (c *Client) Write(name string, v float64) error {
	if !c.CanWrite() {
		return ErrWriteBlocked
	}
	p, ok := c.cfg.Map.ByName(name)
	if !ok {
		return fmt.Errorf("plc: unknown point %q", name)
	}
	if !p.Writable {
		return fmt.Errorf("plc: point %q is not writable", name)
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	if err := c.connectLocked(); err != nil {
		return err
	}
	var err error
	if p.Kind == regmap.Coil {
		err = c.mb.WriteCoil(p.Addr, v != 0)
	} else {
		err = c.mb.WriteRegister(p.Addr, p.Encode(v))
	}
	if err != nil {
		// A Modbus exception means the PLC answered and refused; the
		// connection is fine. Anything else is a transport problem.
		if !isException(err) {
			c.dropLocked()
		}
		return fmt.Errorf("plc: write %s=%g: %w", name, v, err)
	}
	return nil
}

// Close drops the connection, if any.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.mb == nil {
		return nil
	}
	err := c.mb.Close()
	c.mb = nil
	return err
}

func (c *Client) connectLocked() error {
	if c.mb != nil {
		return nil
	}
	mb, err := modbus.NewClient(&modbus.ClientConfiguration{URL: c.cfg.URL, Timeout: c.cfg.Timeout})
	if err != nil {
		return fmt.Errorf("plc: %w", err)
	}
	if err := mb.Open(); err != nil {
		return fmt.Errorf("plc: connect %s: %w", c.cfg.URL, err)
	}
	if err := mb.SetUnitId(c.cfg.Map.UnitID); err != nil {
		_ = mb.Close()
		return fmt.Errorf("plc: %w", err)
	}
	c.mb = mb
	return nil
}

func (c *Client) dropLocked() {
	if c.mb != nil {
		_ = c.mb.Close()
		c.mb = nil
	}
}

// readAll reads each data table as one contiguous block, which keeps the
// number of round trips at one per table.
func readAll(mb *modbus.ModbusClient, m *regmap.Map) (map[string]float64, error) {
	byKind := map[regmap.Kind][]regmap.Point{}
	for _, p := range m.Points {
		byKind[p.Kind] = append(byKind[p.Kind], p)
	}

	out := make(map[string]float64, len(m.Points))
	for kind, points := range byKind {
		lo, hi := points[0].Addr, points[0].Addr
		for _, p := range points {
			lo, hi = min(lo, p.Addr), max(hi, p.Addr)
		}
		n := hi - lo + 1

		switch kind {
		case regmap.Holding, regmap.Input:
			rt := modbus.HOLDING_REGISTER
			if kind == regmap.Input {
				rt = modbus.INPUT_REGISTER
			}
			regs, err := mb.ReadRegisters(lo, n, rt)
			if err != nil {
				return nil, fmt.Errorf("plc: read %s %d+%d: %w", kind, lo, n, err)
			}
			for _, p := range points {
				out[p.Name] = p.Decode(regs[p.Addr-lo])
			}
		case regmap.Coil, regmap.Discrete:
			read := mb.ReadCoils
			if kind == regmap.Discrete {
				read = mb.ReadDiscreteInputs
			}
			bits, err := read(lo, n)
			if err != nil {
				return nil, fmt.Errorf("plc: read %s %d+%d: %w", kind, lo, n, err)
			}
			for _, p := range points {
				out[p.Name] = 0
				if bits[p.Addr-lo] {
					out[p.Name] = 1
				}
			}
		}
	}
	return out, nil
}

// modbusExceptions are errors the library returns when the server answered
// with a Modbus exception code.
var modbusExceptions = []error{
	modbus.ErrIllegalFunction,
	modbus.ErrIllegalDataAddress,
	modbus.ErrIllegalDataValue,
	modbus.ErrServerDeviceFailure,
	modbus.ErrAcknowledge,
	modbus.ErrServerDeviceBusy,
	modbus.ErrMemoryParityError,
	modbus.ErrGWPathUnavailable,
	modbus.ErrGWTargetFailedToRespond,
}

func isException(err error) bool {
	for _, e := range modbusExceptions {
		if errors.Is(err, e) {
			return true
		}
	}
	return false
}

// IsLoopback reports whether a Modbus URL such as tcp://127.0.0.1:5502 points
// at this machine.
func IsLoopback(rawURL string) bool {
	u, err := url.Parse(rawURL)
	if err != nil {
		return false
	}
	host := u.Hostname()
	if host == "localhost" {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}
