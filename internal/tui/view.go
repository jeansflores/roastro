package tui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/jeansflores/project-c/internal/machine"
)

// ANSI colors follow the terminal's own palette, so the dashboard looks
// right on both dark and light themes.
var (
	green  = lipgloss.Color("2")
	red    = lipgloss.Color("1")
	yellow = lipgloss.Color("3")
	dim    = lipgloss.Color("8")

	panel = lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(dim).
		Padding(0, 1)
	title  = lipgloss.NewStyle().Bold(true)
	label  = lipgloss.NewStyle().Foreground(dim).Width(9)
	big    = lipgloss.NewStyle().Bold(true)
	on     = lipgloss.NewStyle().Bold(true).Foreground(green)
	off    = lipgloss.NewStyle().Foreground(dim)
	alarm  = lipgloss.NewStyle().Bold(true).Foreground(red)
	notice = lipgloss.NewStyle().Bold(true).Foreground(yellow)
	help   = lipgloss.NewStyle().Foreground(dim)
)

const panelWidth = 44

// View renders the dashboard.
func (m Model) View() tea.View {
	roastPanel := panel.Width(panelWidth).Render(m.roastView())
	linkPanel := panel.Width(panelWidth).Render(m.linkView())

	body := lipgloss.JoinHorizontal(lipgloss.Top, roastPanel, " ", linkPanel)
	if m.width > 0 && m.width < 2*panelWidth+1 {
		body = lipgloss.JoinVertical(lipgloss.Left, roastPanel, linkPanel)
	}

	v := tea.NewView(strings.Join([]string{m.headerView(), body, m.status, m.helpView()}, "\n"))
	v.AltScreen = true
	v.WindowTitle = "project-c monitor"
	return v
}

func (m Model) headerView() string {
	conn := notice.Render("◌ CONNECTING")
	switch {
	case m.connected:
		conn = on.Render("● CONNECTED")
	case m.errors > 0:
		conn = alarm.Render("● DISCONNECTED")
	}
	writes := off.Render("read-only")
	if m.cfg.CanWrite {
		writes = notice.Render("writes enabled")
	}
	return fmt.Sprintf("%s  %s  %s  %s", title.Render("project-c monitor"), m.cfg.Target, conn, writes)
}

func (m Model) roastView() string {
	if m.value == nil {
		return title.Render("Roast") + "\n\n" + off.Render("waiting for the first reading…")
	}
	v := m.value
	rate := "  —"
	if r, ok := m.ror.Value(); ok {
		rate = fmt.Sprintf("%+5.1f", r)
	}

	rows := []string{
		title.Render("Roast"),
		"",
		row("BT", big.Render(fmt.Sprintf("%6.1f °C", v["bt"]))+"   RoR "+big.Render(rate)+" °C/min"),
		row("ET", fmt.Sprintf("%6.1f °C", v["et"])),
		row("Timer", clock(time.Duration(v["timer_s"])*time.Second)),
		"",
		row("Gas", fmt.Sprintf("%3.0f %%  (valve %3.0f %%)", v["gas_cmd"], v["gas_fb"])),
		row("Drum", fmt.Sprintf("%3.0f %%  %3.0f rpm", v["drum"], machine.DrumRPM(v["drum"]))),
		row("Air", fmt.Sprintf("%3.0f %%  %3.0f m³/h", v["fan"], machine.AirflowM3h(v["fan"]))),
		"",
		strings.Join([]string{
			flag(v, "burner", "BURNER"), flag(v, "drum_on", "DRUM"), flag(v, "fan_on", "FAN"),
			flag(v, "stirrer", "STIRRER"), flag(v, "cooler", "COOLER"),
		}, " "),
	}
	var alarms []string
	for _, a := range []struct{ point, text string }{
		{"emergency", "EMERGENCY STOP"},
		{"flame_fault", "FLAME FAILURE"},
		{"safety_trip", "SAFETY LIMIT"},
	} {
		if v[a.point] != 0 {
			alarms = append(alarms, alarm.Render(a.text))
		}
	}
	if v["bell"] != 0 {
		alarms = append(alarms, notice.Render("ROAST POINT"))
	}
	if v["hopper_sensor"] != 0 {
		alarms = append(alarms, notice.Render("HOPPER"))
	}
	if len(alarms) > 0 {
		rows = append(rows, strings.Join(alarms, " "))
	}
	return strings.Join(rows, "\n")
}

func (m Model) linkView() string {
	status := notice.Render("connecting")
	switch {
	case m.connected:
		status = on.Render("connected")
	case m.errors > 0:
		wait := max(m.nextRetry.Sub(m.now), 0).Round(time.Second)
		status = alarm.Render("down") + fmt.Sprintf(", retry in %v", wait)
	}
	lastOK := "never"
	if !m.lastOK.IsZero() {
		lastOK = fmt.Sprintf("%.1f s ago", m.now.Sub(m.lastOK).Seconds())
	}
	lastErr := "none"
	if m.lastErr != nil {
		lastErr = m.lastErr.Error()
	}

	rows := []string{
		title.Render("Link"),
		"",
		row("Status", status),
		row("Latency", m.latency.Round(10*time.Microsecond).String()),
		row("Last OK", lastOK),
		row("Polls", fmt.Sprintf("%d  ok %d  failed %d", m.attempts, m.reads, m.errors)),
		row("Last err", ""),
		off.Width(panelWidth - 4).Render(lastErr),
	}
	return strings.Join(rows, "\n")
}

func (m Model) helpView() string {
	if !m.cfg.CanWrite {
		return help.Render("q quit · read-only target")
	}
	return help.Render("b burner · d drum · f fan · s stirrer · c cooler · +/- gas · ]/[ air · }/{ drum · q quit")
}

func row(name, value string) string { return label.Render(name) + value }

func flag(v map[string]float64, point, text string) string {
	if v[point] != 0 {
		return on.Render(text)
	}
	return off.Render(strings.ToLower(text))
}

func clock(d time.Duration) string {
	return fmt.Sprintf("%02d:%02d", int(d.Minutes()), int(d.Seconds())%60)
}
