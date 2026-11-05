// Package roast holds pure roast calculations. It knows nothing about Modbus,
// terminals or wall-clock time: callers pass in the roast clock.
package roast

import "time"

// RoR computes the rate of rise of a temperature in °C/min over a sliding
// time window. It fits a least-squares line through the samples in the
// window, which is far less sensitive to probe noise than comparing the two
// endpoints.
//
// The zero value is not usable; create one with NewRoR.
type RoR struct {
	window  time.Duration
	samples []sample
}

type sample struct {
	t time.Duration // roast clock
	v float64       // °C
}

// NewRoR returns a RoR that looks back over the given window.
func NewRoR(window time.Duration) *RoR {
	return &RoR{window: window}
}

// Add records temperature v at roast-clock time t. Samples must arrive in
// increasing t; a sample that goes back in time resets the history, which is
// what happens when the roast clock restarts.
func (r *RoR) Add(t time.Duration, v float64) {
	if n := len(r.samples); n > 0 && t <= r.samples[n-1].t {
		r.samples = r.samples[:0]
	}
	r.samples = append(r.samples, sample{t, v})

	// Drop samples that fell out of the window, keeping at least two.
	cut := 0
	for cut < len(r.samples)-2 && t-r.samples[cut].t > r.window {
		cut++
	}
	r.samples = r.samples[cut:]
}

// Value returns the current rate of rise in °C/min. ok is false until there
// are at least two samples to fit a line through.
func (r *RoR) Value() (rate float64, ok bool) {
	n := float64(len(r.samples))
	if n < 2 {
		return 0, false
	}
	// Least squares slope: Σ(dx·dy) / Σ(dx²), with x in minutes.
	var mx, my float64
	for _, s := range r.samples {
		mx += s.t.Minutes()
		my += s.v
	}
	mx, my = mx/n, my/n
	var num, den float64
	for _, s := range r.samples {
		dx := s.t.Minutes() - mx
		num += dx * (s.v - my)
		den += dx * dx
	}
	if den == 0 {
		return 0, false
	}
	return num / den, true
}

// Reset forgets all samples.
func (r *RoR) Reset() { r.samples = r.samples[:0] }
