package sim

import (
	"log/slog"

	"github.com/simonvetter/modbus"

	"github.com/jeansflores/project-c/internal/regmap"
)

// ModbusHandler serves a Sim over Modbus, translating register reads and
// writes into Sim.Get and Sim.Set calls through a register map. It implements
// modbus.RequestHandler.
type ModbusHandler struct {
	m   *regmap.Map
	s   *Sim
	log *slog.Logger
}

// NewModbusHandler returns a handler that exposes s at the addresses in m.
func NewModbusHandler(m *regmap.Map, s *Sim, log *slog.Logger) *ModbusHandler {
	return &ModbusHandler{m: m, s: s, log: log}
}

func (h *ModbusHandler) checkUnit(id uint8) error {
	if id != h.m.UnitID && id != 0 && id != 255 {
		return modbus.ErrGWTargetFailedToRespond
	}
	return nil
}

func (h *ModbusHandler) readBools(k regmap.Kind, addr, qty uint16) []bool {
	out := make([]bool, qty)
	for i := range out {
		if p, ok := h.m.At(k, addr+uint16(i)); ok {
			v, _ := h.s.Get(p.Name)
			out[i] = v != 0
		}
	}
	return out
}

func (h *ModbusHandler) readRegs(k regmap.Kind, addr, qty uint16) []uint16 {
	out := make([]uint16, qty)
	for i := range out {
		if p, ok := h.m.At(k, addr+uint16(i)); ok {
			v, _ := h.s.Get(p.Name)
			out[i] = p.Encode(v)
		}
	}
	return out
}

// HandleCoils serves coil reads and writes.
func (h *ModbusHandler) HandleCoils(r *modbus.CoilsRequest) ([]bool, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	if !r.IsWrite {
		return h.readBools(regmap.Coil, r.Addr, r.Quantity), nil
	}
	for i, v := range r.Args {
		p, ok := h.m.At(regmap.Coil, r.Addr+uint16(i))
		if !ok || !p.Writable {
			return nil, modbus.ErrIllegalDataAddress
		}
		val := 0.0
		if v {
			val = 1
		}
		if err := h.s.Set(p.Name, val); err != nil {
			h.log.Warn("write refused", "point", p.Name, "err", err)
			return nil, modbus.ErrServerDeviceFailure
		}
		h.log.Info("coil write", "client", r.ClientAddr, "point", p.Name, "value", v)
	}
	return nil, nil
}

// HandleDiscreteInputs serves discrete input reads.
func (h *ModbusHandler) HandleDiscreteInputs(r *modbus.DiscreteInputsRequest) ([]bool, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	return h.readBools(regmap.Discrete, r.Addr, r.Quantity), nil
}

// HandleHoldingRegisters serves holding register reads and writes.
func (h *ModbusHandler) HandleHoldingRegisters(r *modbus.HoldingRegistersRequest) ([]uint16, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	if !r.IsWrite {
		return h.readRegs(regmap.Holding, r.Addr, r.Quantity), nil
	}
	for i, raw := range r.Args {
		p, ok := h.m.At(regmap.Holding, r.Addr+uint16(i))
		if !ok || !p.Writable {
			return nil, modbus.ErrIllegalDataAddress
		}
		if err := h.s.Set(p.Name, p.Decode(raw)); err != nil {
			h.log.Warn("write refused", "point", p.Name, "err", err)
			return nil, modbus.ErrServerDeviceFailure
		}
		h.log.Info("register write", "client", r.ClientAddr, "point", p.Name, "value", p.Decode(raw))
	}
	return nil, nil
}

// HandleInputRegisters serves input register reads.
func (h *ModbusHandler) HandleInputRegisters(r *modbus.InputRegistersRequest) ([]uint16, error) {
	if err := h.checkUnit(r.UnitId); err != nil {
		return nil, err
	}
	return h.readRegs(regmap.Input, r.Addr, r.Quantity), nil
}
