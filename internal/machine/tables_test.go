package machine

import "testing"

func TestConversions(t *testing.T) {
	tests := []struct {
		name string
		fn   func(float64) float64
		pct  float64
		want float64
	}{
		{"drum at a table row", DrumRPM, 80, 52},
		{"drum between rows", DrumRPM, 82.5, 54},
		{"drum below range", DrumRPM, -10, 0},
		{"drum above range", DrumRPM, 150, 73},
		{"air at a table row", AirflowM3h, 45, 124},
		{"air between rows", AirflowM3h, 47.5, 130},
		{"air off", AirflowM3h, 0, 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.fn(tt.pct); got != tt.want {
				t.Errorf("got %v; want %v", got, tt.want)
			}
		})
	}
}
