package plc

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestBackoff(t *testing.T) {
	const base, limit = time.Second, 10 * time.Second
	tests := []struct {
		failures int
		want     time.Duration
	}{
		{0, 0},
		{1, 1 * time.Second},
		{2, 2 * time.Second},
		{3, 4 * time.Second},
		{4, 8 * time.Second},
		{5, 10 * time.Second},
		{50, 10 * time.Second},
	}
	for _, tt := range tests {
		if got := Backoff(tt.failures, base, limit); got != tt.want {
			t.Errorf("Backoff(%d) = %v; want %v", tt.failures, got, tt.want)
		}
	}
}

// scriptedReader returns the errors in script, in order, then succeeds.
type scriptedReader struct {
	script []error
	calls  int
}

func (r *scriptedReader) Read() (Reading, error) {
	r.calls++
	if r.calls <= len(r.script) {
		return Reading{}, r.script[r.calls-1]
	}
	return Reading{Values: map[string]float64{"bt": 200}}, nil
}

func TestPollerBacksOffAndRecovers(t *testing.T) {
	down := errors.New("connection refused")
	p := Poller{
		Reader:      &scriptedReader{script: []error{down, down, down}},
		Interval:    time.Millisecond,
		BaseBackoff: time.Millisecond,
		MaxBackoff:  3 * time.Millisecond,
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	events := p.Run(ctx)

	want := []struct {
		failures int
		retryIn  time.Duration
		ok       bool
	}{
		{1, 1 * time.Millisecond, false},
		{2, 2 * time.Millisecond, false},
		{3, 3 * time.Millisecond, false}, // capped
		{0, 1 * time.Millisecond, true},  // back to the normal interval
	}
	for i, w := range want {
		ev, open := <-events
		if !open {
			t.Fatalf("event %d: channel closed early", i)
		}
		if ev.Failures != w.failures || ev.RetryIn != w.retryIn || (ev.Err == nil) != w.ok {
			t.Errorf("event %d = {Failures:%d RetryIn:%v Err:%v}; want {Failures:%d RetryIn:%v ok:%v}",
				i, ev.Failures, ev.RetryIn, ev.Err, w.failures, w.retryIn, w.ok)
		}
	}
	if got := (<-events).Reading.Values["bt"]; got != 200 {
		t.Errorf("bt = %v; want 200", got)
	}
}

func TestPollerStopsOnCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	events := Poller{Reader: &scriptedReader{}, Interval: time.Hour}.Run(ctx)
	<-events
	cancel()

	select {
	case _, open := <-events:
		if open {
			t.Fatal("got an event after cancel; want channel closed")
		}
	case <-time.After(time.Second):
		t.Fatal("channel not closed after cancel")
	}
}
