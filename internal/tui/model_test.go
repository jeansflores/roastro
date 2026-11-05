package tui

import (
	"errors"
	"log/slog"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/jeansflores/roastro/internal/plc"
)

type write struct {
	name  string
	value float64
}

// fakeWriter records writes and fails them with err.
type fakeWriter struct {
	writes []write
	err    error
}

func (w *fakeWriter) Write(name string, v float64) error {
	w.writes = append(w.writes, write{name, v})
	return w.err
}

var t0 = time.Date(2026, 11, 4, 10, 0, 0, 0, time.UTC)

func newModel(canWrite bool, w Writer) Model {
	return New(Config{Target: "tcp://127.0.0.1:5502", CanWrite: canWrite}, nil, w, slog.New(slog.DiscardHandler))
}

func reading(values map[string]float64) eventMsg {
	return eventMsg(plc.Event{At: t0, Reading: plc.Reading{Values: values, Latency: time.Millisecond}, RetryIn: time.Second})
}

func key(s string) tea.KeyPressMsg {
	r := []rune(s)[0]
	return tea.KeyPressMsg{Code: r, Text: s}
}

// update runs one message through Update and returns the concrete Model.
func update(t *testing.T, m Model, msg tea.Msg) (Model, tea.Cmd) {
	t.Helper()
	next, cmd := m.Update(msg)
	return next.(Model), cmd
}

func TestUpdateTracksTheLink(t *testing.T) {
	m := newModel(true, &fakeWriter{})

	m, _ = update(t, m, reading(map[string]float64{"bt": 198.4}))
	if !m.connected || m.reads != 1 || m.value["bt"] != 198.4 || m.lastOK != t0 {
		t.Fatalf("after a good read: connected=%v reads=%d bt=%v lastOK=%v", m.connected, m.reads, m.value["bt"], m.lastOK)
	}

	down := errors.New("connection refused")
	m, _ = update(t, m, eventMsg(plc.Event{At: t0.Add(time.Second), Err: down, Failures: 1, RetryIn: 2 * time.Second}))
	if m.connected || m.errors != 1 || !errors.Is(m.lastErr, down) {
		t.Fatalf("after a failed read: connected=%v errors=%d lastErr=%v", m.connected, m.errors, m.lastErr)
	}
	if m.value["bt"] != 198.4 {
		t.Errorf("a failed read cleared the last good value: bt=%v", m.value["bt"])
	}
	if want := t0.Add(3 * time.Second); !m.nextRetry.Equal(want) {
		t.Errorf("nextRetry = %v; want %v", m.nextRetry, want)
	}
	if m.attempts != 2 {
		t.Errorf("attempts = %d; want 2", m.attempts)
	}
}

func TestKeysSendCommands(t *testing.T) {
	state := map[string]float64{"burner": 0, "drum_on": 1, "gas_cmd": 98, "fan": 3, "drum": 80}
	tests := []struct {
		key  string
		want write
	}{
		{"b", write{"burner", 1}},
		{"d", write{"drum_on", 0}},
		{"+", write{"gas_cmd", 100}}, // clamped
		{"-", write{"gas_cmd", 93}},
		{"[", write{"fan", 0}}, // clamped
		{"]", write{"fan", 8}},
		{"}", write{"drum", 85}},
	}
	for _, tt := range tests {
		t.Run(tt.key, func(t *testing.T) {
			w := &fakeWriter{}
			m, _ := update(t, newModel(true, w), reading(state))
			m, cmd := update(t, m, key(tt.key))
			if cmd == nil {
				t.Fatal("no command returned")
			}
			msg := cmd() // runs the write
			if len(w.writes) != 1 || w.writes[0] != tt.want {
				t.Fatalf("writes = %v; want [%v]", w.writes, tt.want)
			}
			m, _ = update(t, m, msg)
			if m.status == "" {
				t.Error("status line empty after the write finished")
			}
		})
	}
}

// Pressing "+" twice before the first write returns must ask for 10%, not
// 5% twice: the second press builds on the pending target.
func TestRepeatedKeysBuildOnPendingWrites(t *testing.T) {
	w := &fakeWriter{}
	m, _ := update(t, newModel(true, w), reading(map[string]float64{"gas_cmd": 0}))
	m, first := update(t, m, key("+"))
	m, second := update(t, m, key("+"))

	m, _ = update(t, m, first())
	_, _ = update(t, m, second())
	want := []write{{"gas_cmd", 5}, {"gas_cmd", 10}}
	if len(w.writes) != 2 || w.writes[0] != want[0] || w.writes[1] != want[1] {
		t.Errorf("writes = %v; want %v", w.writes, want)
	}
}

func TestKeysRefuseWhenUnsafeOrBlind(t *testing.T) {
	tests := []struct {
		name     string
		canWrite bool
		read     bool
	}{
		{"read-only target", false, true},
		{"no reading yet", true, false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			w := &fakeWriter{}
			m := newModel(tt.canWrite, w)
			if tt.read {
				m, _ = update(t, m, reading(map[string]float64{"burner": 0}))
			}
			m, cmd := update(t, m, key("b"))
			if cmd != nil {
				cmd()
			}
			if len(w.writes) != 0 {
				t.Errorf("writes = %v; want none", w.writes)
			}
			if m.status == "" {
				t.Error("no explanation on the status line")
			}
		})
	}
}

func TestWriteFailureIsShown(t *testing.T) {
	w := &fakeWriter{err: errors.New("server device failure")}
	m, _ := update(t, newModel(true, w), reading(map[string]float64{"burner": 0}))
	_, cmd := update(t, m, key("b"))
	m, _ = update(t, m, cmd())
	if want := "✗ burner=1: server device failure"; m.status != want {
		t.Errorf("status = %q; want %q", m.status, want)
	}
}

func TestQuit(t *testing.T) {
	for _, msg := range []tea.Msg{key("q"), tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl}, eventsClosed{}} {
		_, cmd := update(t, newModel(true, &fakeWriter{}), msg)
		if cmd == nil {
			t.Fatalf("%v: no command; want tea.Quit", msg)
		}
		if _, ok := cmd().(tea.QuitMsg); !ok {
			t.Errorf("%v: command does not quit", msg)
		}
	}
}

func TestViewRenders(t *testing.T) {
	m := newModel(true, &fakeWriter{})
	if m.View().Content == "" {
		t.Fatal("empty view before the first reading")
	}
	m, _ = update(t, m, reading(map[string]float64{"bt": 200, "emergency": 1}))
	if m.View().Content == "" {
		t.Fatal("empty view after a reading")
	}
}
