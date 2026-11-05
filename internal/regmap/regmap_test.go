package regmap

import (
	"os"
	"path/filepath"
	"testing"
)

func TestPointEncodeDecode(t *testing.T) {
	tests := []struct {
		name  string
		point Point
		value float64
		raw   uint16
	}{
		{"unscaled", Point{Scale: 1}, 60, 60},
		{"zero scale means 1", Point{}, 42, 42},
		{"tenths", Point{Scale: 10}, 198.7, 1987},
		{"signed negative", Point{Scale: 10, Signed: true}, -12.5, 0xFF83}, // -125
		{"unsigned clamps at zero", Point{Scale: 1}, -5, 0},
		{"unsigned clamps at max", Point{Scale: 10}, 7000, 65535},
		{"signed clamps at max", Point{Scale: 10, Signed: true}, 4000, 32767},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.point.Encode(tt.value); got != tt.raw {
				t.Errorf("Encode(%v) = %#x; want %#x", tt.value, got, tt.raw)
			}
		})
	}
}

func TestPointRoundTrip(t *testing.T) {
	p := Point{Scale: 10, Signed: true}
	for _, v := range []float64{-40, 0, 25.3, 198.7, 280} {
		if got := p.Decode(p.Encode(v)); got != v {
			t.Errorf("Decode(Encode(%v)) = %v", v, got)
		}
	}
}

func TestLoad(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantErr bool
	}{
		{"valid", `{"unit_id":1,"points":[{"name":"bt","kind":"holding","addr":0},{"name":"burner","kind":"coil","addr":0}]}`, false},
		{"duplicate name", `{"points":[{"name":"bt","kind":"holding","addr":0},{"name":"bt","kind":"holding","addr":1}]}`, true},
		{"duplicate address", `{"points":[{"name":"bt","kind":"holding","addr":0},{"name":"et","kind":"holding","addr":0}]}`, true},
		{"bad json", `{`, true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "regmap.json")
			if err := os.WriteFile(path, []byte(tt.json), 0o600); err != nil {
				t.Fatal(err)
			}
			_, err := Load(path)
			if (err != nil) != tt.wantErr {
				t.Errorf("Load() error = %v; wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// TestRepoMap keeps the committed register map loadable.
func TestRepoMap(t *testing.T) {
	m, err := Load("../../configs/regmap.json")
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := m.ByName("bt"); !ok {
		t.Error(`committed map has no "bt" point`)
	}
	if p, ok := m.At(Coil, 0); !ok || p.Name != "burner" {
		t.Errorf("At(Coil, 0) = %q, %v; want burner", p.Name, ok)
	}
}
