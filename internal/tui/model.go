// Package tui is the terminal dashboard behind cmd/monitor. It follows the
// Bubble Tea architecture: Model holds all state, Update is the only place
// that changes it, and View renders it.
package tui

import (
	"fmt"
	"log/slog"
	"maps"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/jeansflores/roastro/internal/plc"
	"github.com/jeansflores/roastro/internal/roast"
)

// Writer sends a command to the PLC. *plc.Client implements it.
type Writer interface {
	Write(name string, v float64) error
}

// Config holds what the dashboard needs to know about its environment.
type Config struct {
	Target    string        // shown in the header
	CanWrite  bool          // false disables every command key
	TimeScale float64       // simulator --speed, so RoR is in simulated °C/min
	RoRWindow time.Duration // default 30s
}

// Messages flowing into Update besides key presses.
type (
	eventMsg     plc.Event
	eventsClosed struct{}
	tickMsg      time.Time
	writeDone    struct {
		name  string
		value float64
		err   error
	}
)

// Model is the dashboard state.
type Model struct {
	cfg    Config
	events <-chan plc.Event
	writer Writer
	log    *slog.Logger

	now   time.Time          // refreshed by ticks, drives "x s ago"
	start time.Time          // first event; zero point of the roast clock
	ror   *roast.RoR         // rate of rise of BT
	value map[string]float64 // latest good reading

	// pending holds targets of writes still in flight, so a second key
	// press nudges from the first one's target instead of a stale reading.
	pending map[string]float64

	connected bool
	attempts  int // poll attempts seen, good or bad
	reads     int
	errors    int
	latency   time.Duration
	lastOK    time.Time
	lastErr   error
	nextRetry time.Time

	status string // result of the last command
	width  int
}

// New returns a dashboard fed by events that sends commands through w.
func New(cfg Config, events <-chan plc.Event, w Writer, log *slog.Logger) Model {
	if cfg.TimeScale <= 0 {
		cfg.TimeScale = 1
	}
	if cfg.RoRWindow == 0 {
		cfg.RoRWindow = 30 * time.Second
	}
	return Model{
		cfg:     cfg,
		events:  events,
		writer:  w,
		log:     log,
		now:     time.Now(),
		ror:     roast.NewRoR(cfg.RoRWindow),
		pending: map[string]float64{},
	}
}

// Init starts listening for poll events and starts the clock.
func (m Model) Init() tea.Cmd {
	return tea.Batch(waitForEvent(m.events), tick())
}

// waitForEvent turns the next value on the channel into a message. Bubble
// Tea runs commands in their own goroutine, so blocking here is fine.
func waitForEvent(events <-chan plc.Event) tea.Cmd {
	return func() tea.Msg {
		ev, ok := <-events
		if !ok {
			return eventsClosed{}
		}
		return eventMsg(ev)
	}
}

func tick() tea.Cmd {
	return tea.Tick(250*time.Millisecond, func(t time.Time) tea.Msg { return tickMsg(t) })
}

// Update applies one message to the model.
func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		m.applyEvent(plc.Event(msg))
		return m, waitForEvent(m.events)
	case eventsClosed:
		return m, tea.Quit
	case tickMsg:
		m.now = time.Time(msg)
		return m, tick()
	case writeDone:
		delete(m.pending, msg.name)
		if msg.err != nil {
			m.status = fmt.Sprintf("✗ %s=%g: %v", msg.name, msg.value, msg.err)
			m.log.Warn("write failed", "point", msg.name, "value", msg.value, "err", msg.err)
		} else {
			m.status = fmt.Sprintf("✓ %s=%g", msg.name, msg.value)
			m.value[msg.name] = msg.value // the PLC accepted it; the next poll confirms
			m.log.Info("write", "point", msg.name, "value", msg.value)
		}
		return m, nil
	case tea.WindowSizeMsg:
		m.width = msg.Width
		return m, nil
	case tea.KeyPressMsg:
		return m.handleKey(msg.String())
	}
	return m, nil
}

func (m *Model) applyEvent(ev plc.Event) {
	m.attempts++
	m.now = ev.At
	if ev.Err != nil {
		m.connected = false
		m.errors++
		m.lastErr = ev.Err
		m.nextRetry = ev.At.Add(ev.RetryIn)
		return
	}
	m.connected = true
	m.reads++
	m.latency = ev.Reading.Latency
	m.lastOK = ev.At
	m.value = maps.Clone(ev.Reading.Values) // owned copy: Update writes into it

	if m.start.IsZero() {
		m.start = ev.At
	}
	roastClock := time.Duration(float64(ev.At.Sub(m.start)) * m.cfg.TimeScale)
	m.ror.Add(roastClock, m.value["bt"])
}

// command describes what a key does to the machine.
type command struct {
	point  string
	toggle bool    // flip a coil
	step   float64 // or nudge a register
	min    float64
	max    float64
}

var keymap = map[string]command{
	"b": {point: "burner", toggle: true},
	"d": {point: "drum_on", toggle: true},
	"f": {point: "fan_on", toggle: true},
	"s": {point: "stirrer", toggle: true},
	"c": {point: "cooler", toggle: true},
	"+": {point: "gas_cmd", step: +5, max: 100},
	"=": {point: "gas_cmd", step: +5, max: 100}, // "+" without shift
	"-": {point: "gas_cmd", step: -5, max: 100},
	"]": {point: "fan", step: +5, max: 120},
	"[": {point: "fan", step: -5, max: 120},
	"}": {point: "drum", step: +5, max: 120},
	"{": {point: "drum", step: -5, max: 120},
}

func (m Model) handleKey(key string) (tea.Model, tea.Cmd) {
	if key == "q" || key == "ctrl+c" {
		return m, tea.Quit
	}
	cmd, ok := keymap[key]
	if !ok {
		return m, nil
	}
	if !m.cfg.CanWrite {
		m.status = "✗ read-only: writes to this target are blocked (see --allow-write)"
		return m, nil
	}
	current, ok := m.pending[cmd.point]
	if !ok {
		current, ok = m.value[cmd.point]
	}
	if !ok {
		m.status = "✗ no reading yet; wait for the PLC before sending commands"
		return m, nil
	}

	target := 1.0
	if cmd.toggle {
		if current != 0 {
			target = 0
		}
	} else {
		target = min(max(current+cmd.step, cmd.min), cmd.max)
	}
	m.pending[cmd.point] = target
	m.status = fmt.Sprintf("… %s=%g", cmd.point, target)
	return m, m.write(cmd.point, target)
}

// write runs the Modbus write off the UI goroutine and reports back.
func (m Model) write(name string, v float64) tea.Cmd {
	return func() tea.Msg {
		return writeDone{name: name, value: v, err: m.writer.Write(name, v)}
	}
}
