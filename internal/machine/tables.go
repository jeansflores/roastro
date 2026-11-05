// Package machine holds facts about the Caloratto roaster taken from its
// manual, shared by the simulator and the app.
package machine

// Manual p. 16: fan % -> airflow (m³/h)
var fanTable = [][2]float64{
	{0, 0}, {5, 71}, {10, 76}, {15, 80}, {20, 83}, {25, 86}, {30, 90}, {35, 105},
	{40, 113}, {45, 124}, {50, 136}, {55, 150}, {60, 161}, {65, 178}, {70, 192},
	{75, 203}, {80, 218}, {85, 229}, {90, 240}, {95, 254}, {100, 260}, {105, 271},
	{110, 274}, {115, 278}, {120, 282},
}

// Manual p. 16: drum % -> rpm
var drumTable = [][2]float64{
	{0, 0}, {5, 5}, {10, 8}, {15, 12}, {20, 15}, {25, 17}, {30, 19}, {35, 23}, {40, 26},
	{45, 29}, {50, 33}, {55, 36}, {60, 39}, {65, 42}, {70, 46}, {75, 49}, {80, 52},
	{85, 56}, {90, 59}, {95, 62}, {100, 65}, {105, 68}, {110, 71}, {115, 72}, {120, 73},
}

func interp(t [][2]float64, x float64) float64 {
	if x <= t[0][0] {
		return t[0][1]
	}
	for i := 1; i < len(t); i++ {
		if x <= t[i][0] {
			a, b := t[i-1], t[i]
			return a[1] + (b[1]-a[1])*(x-a[0])/(b[0]-a[0])
		}
	}
	return t[len(t)-1][1]
}

// AirflowM3h converts a fan setting in percent into airflow in m³/h.
func AirflowM3h(pct float64) float64 { return interp(fanTable, pct) }

// DrumRPM converts a drum setting in percent into drum rpm.
func DrumRPM(pct float64) float64 { return interp(drumTable, pct) }
