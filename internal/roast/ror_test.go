package roast

import (
	"math"
	"testing"
	"time"
)

func TestRoRValue(t *testing.T) {
	// ramp returns a temperature that rises perMin °C/min from base.
	ramp := func(base, perMin float64) func(time.Duration) float64 {
		return func(t time.Duration) float64 { return base + perMin*t.Minutes() }
	}
	tests := []struct {
		name    string
		window  time.Duration
		samples int // one per second, starting at t=1s
		temp    func(time.Duration) float64
		want    float64
		wantOK  bool
	}{
		{"no samples", 30 * time.Second, 0, ramp(100, 10), 0, false},
		{"one sample", 30 * time.Second, 1, ramp(100, 10), 0, false},
		{"two samples", 30 * time.Second, 2, ramp(100, 10), 10, true},
		{"flat", 30 * time.Second, 60, ramp(200, 0), 0, true},
		{"rising", 30 * time.Second, 60, ramp(90, 12.5), 12.5, true},
		{"falling (turning point side)", 30 * time.Second, 60, ramp(200, -40), -40, true},
		{
			name:    "only the window counts",
			window:  30 * time.Second,
			samples: 120,
			temp: func(t time.Duration) float64 {
				// 20 °C/min for the first minute, then 5 °C/min.
				if t <= time.Minute {
					return 100 + 20*t.Minutes()
				}
				return 120 + 5*(t-time.Minute).Minutes()
			},
			want:   5,
			wantOK: true,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r := NewRoR(tt.window)
			for i := 1; i <= tt.samples; i++ {
				at := time.Duration(i) * time.Second
				r.Add(at, tt.temp(at))
			}
			got, ok := r.Value()
			if ok != tt.wantOK || math.Abs(got-tt.want) > 1e-9 {
				t.Errorf("Value() = %v, %v; want %v, %v", got, ok, tt.want, tt.wantOK)
			}
		})
	}
}

func TestRoRResetsWhenClockGoesBack(t *testing.T) {
	r := NewRoR(30 * time.Second)
	r.Add(10*time.Second, 100)
	r.Add(20*time.Second, 110)
	r.Add(5*time.Second, 50) // roast clock restarted

	if _, ok := r.Value(); ok {
		t.Fatal("Value() ok after clock went back; want history reset")
	}
}

func TestRoRSmoothsNoise(t *testing.T) {
	r := NewRoR(30 * time.Second)
	for i := 1; i <= 30; i++ {
		noise := 0.25
		if i%2 == 0 {
			noise = -0.25
		}
		r.Add(time.Duration(i)*time.Second, 150+10*float64(i)/60+noise)
	}
	got, _ := r.Value()
	if math.Abs(got-10) > 1 {
		t.Errorf("Value() = %.2f with ±0.25 °C noise; want within 1 of 10", got)
	}
}
