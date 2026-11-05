// Package regmap describes where each quantity lives in the PLC (kind, address
// and scale). It is the only thing that changes once the real Carmomaq .aset
// register map arrives.
package regmap

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
)

type Kind string

const (
	Holding  Kind = "holding"
	Input    Kind = "input"
	Coil     Kind = "coil"
	Discrete Kind = "discrete"
)

type Point struct {
	Name     string  `json:"name"`
	Kind     Kind    `json:"kind"`
	Addr     uint16  `json:"addr"`
	Scale    float64 `json:"scale"`
	Signed   bool    `json:"signed"`
	Writable bool    `json:"writable"`
	Unit     string  `json:"unit"`
	Desc     string  `json:"desc"`
}

func (p Point) IsBool() bool { return p.Kind == Coil || p.Kind == Discrete }

// Encode converts a physical value into a raw register value.
func (p Point) Encode(v float64) uint16 {
	s := p.Scale
	if s == 0 {
		s = 1
	}
	raw := math.Round(v * s)
	if p.Signed {
		return uint16(int16(math.Max(-32768, math.Min(32767, raw))))
	}
	return uint16(math.Max(0, math.Min(65535, raw)))
}

// Decode converts a raw register value into a physical value.
func (p Point) Decode(raw uint16) float64 {
	s := p.Scale
	if s == 0 {
		s = 1
	}
	if p.Signed {
		return float64(int16(raw)) / s
	}
	return float64(raw) / s
}

type Map struct {
	UnitID uint8   `json:"unit_id"`
	Points []Point `json:"points"`

	byName map[string]Point
	byAddr map[Kind]map[uint16]Point
}

func Load(path string) (*Map, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var m Map
	if err := json.Unmarshal(b, &m); err != nil {
		return nil, fmt.Errorf("regmap %s: %w", path, err)
	}
	m.byName = map[string]Point{}
	m.byAddr = map[Kind]map[uint16]Point{}
	for _, p := range m.Points {
		if _, dup := m.byName[p.Name]; dup {
			return nil, fmt.Errorf("regmap: duplicate name %q", p.Name)
		}
		if m.byAddr[p.Kind] == nil {
			m.byAddr[p.Kind] = map[uint16]Point{}
		}
		if other, dup := m.byAddr[p.Kind][p.Addr]; dup {
			return nil, fmt.Errorf("regmap: %s and %s share address %s/%d", p.Name, other.Name, p.Kind, p.Addr)
		}
		m.byName[p.Name] = p
		m.byAddr[p.Kind][p.Addr] = p
	}
	return &m, nil
}

func (m *Map) ByName(name string) (Point, bool) { p, ok := m.byName[name]; return p, ok }

func (m *Map) At(k Kind, addr uint16) (Point, bool) { p, ok := m.byAddr[k][addr]; return p, ok }
