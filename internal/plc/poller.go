package plc

import (
	"context"
	"log/slog"
	"time"
)

// Reader is anything that can take a Reading. *Client implements it; tests
// use fakes.
type Reader interface {
	Read() (Reading, error)
}

// Event is the outcome of one poll attempt.
type Event struct {
	At      time.Time
	Reading Reading // zero when Err is set
	Err     error

	// Failures counts consecutive failed attempts, including this one.
	// It is 0 after a successful read.
	Failures int
	// RetryIn is how long the poller waits before the next attempt.
	RetryIn time.Duration
}

// Poller reads from a Reader at a fixed interval and backs off while the
// reads fail. The zero value of each optional field picks a default.
type Poller struct {
	Reader   Reader
	Interval time.Duration // between successful reads; default 1s

	BaseBackoff time.Duration // first wait after a failure; default 1s
	MaxBackoff  time.Duration // cap on the wait; default 10s

	Log *slog.Logger // default: discard
}

// Run polls until ctx is canceled and delivers every attempt on the returned
// channel, which is closed when Run stops.
func (p Poller) Run(ctx context.Context) <-chan Event {
	p.setDefaults()
	out := make(chan Event, 1)
	go func() {
		defer close(out)
		failures := 0
		for {
			start := time.Now()
			r, err := p.Reader.Read()
			ev := Event{At: start, Reading: r, Err: err, RetryIn: p.Interval}

			switch {
			case err != nil:
				failures++
				ev.Failures = failures
				ev.RetryIn = Backoff(failures, p.BaseBackoff, p.MaxBackoff)
				if failures == 1 {
					p.Log.Warn("plc disconnected", "err", err)
				}
				p.Log.Debug("poll failed", "failures", failures, "retry_in", ev.RetryIn, "err", err)
			case failures > 0:
				p.Log.Info("plc reconnected", "after_failures", failures)
				failures = 0
			}

			select {
			case out <- ev:
			case <-ctx.Done():
				return
			}

			timer := time.NewTimer(time.Until(start.Add(ev.RetryIn)))
			select {
			case <-timer.C:
			case <-ctx.Done():
				timer.Stop()
				return
			}
		}
	}()
	return out
}

func (p *Poller) setDefaults() {
	if p.Interval == 0 {
		p.Interval = time.Second
	}
	if p.BaseBackoff == 0 {
		p.BaseBackoff = time.Second
	}
	if p.MaxBackoff == 0 {
		p.MaxBackoff = 10 * time.Second
	}
	if p.Log == nil {
		p.Log = slog.New(slog.DiscardHandler)
	}
}

// Backoff returns the wait after the given number of consecutive failures:
// base, 2·base, 4·base, ... capped at limit.
func Backoff(failures int, base, limit time.Duration) time.Duration {
	if failures < 1 {
		return 0
	}
	d := base
	for i := 1; i < failures && d < limit; i++ {
		d *= 2
	}
	return min(d, limit)
}
