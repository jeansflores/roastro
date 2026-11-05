// Command monitor is a live terminal dashboard for the roaster PLC. It polls
// every point in the register map, shows the roast and the health of the
// Modbus link, and sends commands from the keyboard.
//
//	go run ./cmd/sim --auto --speed 10              # in one terminal
//	go run ./cmd/monitor --time-scale 10            # in another
//	go run ./cmd/monitor --url tcp://192.168.5.11:502   # real machine, read-only
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/jeansflores/project-c/internal/plc"
	"github.com/jeansflores/project-c/internal/regmap"
	"github.com/jeansflores/project-c/internal/tui"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "monitor:", err)
		os.Exit(1)
	}
}

func run() error {
	addr := flag.String("url", "tcp://127.0.0.1:5502", "PLC address (real machine: tcp://192.168.5.11:502)")
	mapPath := flag.String("map", "configs/regmap.json", "register map")
	interval := flag.Duration("interval", time.Second, "polling interval")
	timeout := flag.Duration("timeout", time.Second, "Modbus request timeout")
	scale := flag.Float64("time-scale", 1, "use the simulator --speed so RoR comes out in simulated °C/min")
	allowWrite := flag.Bool("allow-write", false, "allow writes to a non-localhost target (real machine!)")
	logPath := flag.String("log", "monitor.log", "log file (the terminal belongs to the dashboard)")
	logLevel := flag.String("log-level", "info", "debug, info, warn or error")
	flag.Parse()

	logger, closeLog, err := openLog(*logPath, *logLevel)
	if err != nil {
		return err
	}
	defer closeLog()

	m, err := regmap.Load(*mapPath)
	if err != nil {
		return err
	}
	client := plc.NewClient(plc.Config{URL: *addr, Map: m, Timeout: *timeout, AllowWrite: *allowWrite})
	defer func() { _ = client.Close() }()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	events := plc.Poller{Reader: client, Interval: *interval, Log: logger}.Run(ctx)

	logger.Info("monitor started", "url", *addr, "can_write", client.CanWrite(), "interval", *interval)
	model := tui.New(tui.Config{
		Target:    *addr,
		CanWrite:  client.CanWrite(),
		TimeScale: *scale,
	}, events, client, logger)

	if _, err := tea.NewProgram(model).Run(); err != nil {
		return fmt.Errorf("dashboard: %w", err)
	}
	logger.Info("monitor stopped")
	return nil
}

// openLog returns a logger that writes to path. The returned func closes it.
func openLog(path, level string) (*slog.Logger, func(), error) {
	var lvl slog.Level
	if err := lvl.UnmarshalText([]byte(level)); err != nil {
		return nil, nil, fmt.Errorf("--log-level: %w", err)
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return nil, nil, fmt.Errorf("log file: %w", err)
	}
	logger := slog.New(slog.NewTextHandler(f, &slog.HandlerOptions{Level: lvl}))
	return logger, func() { _ = f.Close() }, nil
}
