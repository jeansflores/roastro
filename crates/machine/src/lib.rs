//! Facts about the Caloratto roaster taken from its manual, shared by the
//! simulator and the app.

/// Manual p. 16: fan % → airflow (m³/h).
const FAN_TABLE: [(f64, f64); 25] = [
    (0.0, 0.0),
    (5.0, 71.0),
    (10.0, 76.0),
    (15.0, 80.0),
    (20.0, 83.0),
    (25.0, 86.0),
    (30.0, 90.0),
    (35.0, 105.0),
    (40.0, 113.0),
    (45.0, 124.0),
    (50.0, 136.0),
    (55.0, 150.0),
    (60.0, 161.0),
    (65.0, 178.0),
    (70.0, 192.0),
    (75.0, 203.0),
    (80.0, 218.0),
    (85.0, 229.0),
    (90.0, 240.0),
    (95.0, 254.0),
    (100.0, 260.0),
    (105.0, 271.0),
    (110.0, 274.0),
    (115.0, 278.0),
    (120.0, 282.0),
];

/// Manual p. 16: drum % → rpm.
const DRUM_TABLE: [(f64, f64); 25] = [
    (0.0, 0.0),
    (5.0, 5.0),
    (10.0, 8.0),
    (15.0, 12.0),
    (20.0, 15.0),
    (25.0, 17.0),
    (30.0, 19.0),
    (35.0, 23.0),
    (40.0, 26.0),
    (45.0, 29.0),
    (50.0, 33.0),
    (55.0, 36.0),
    (60.0, 39.0),
    (65.0, 42.0),
    (70.0, 46.0),
    (75.0, 49.0),
    (80.0, 52.0),
    (85.0, 56.0),
    (90.0, 59.0),
    (95.0, 62.0),
    (100.0, 65.0),
    (105.0, 68.0),
    (110.0, 71.0),
    (115.0, 72.0),
    (120.0, 73.0),
];

/// Linear interpolation over a table sorted by x, clamped at both ends.
fn interp(table: &[(f64, f64)], x: f64) -> f64 {
    let Some(&(x0, y0)) = table.first() else {
        return 0.0;
    };
    if x <= x0 {
        return y0;
    }
    for pair in table.windows(2) {
        let ((xa, ya), (xb, yb)) = (pair[0], pair[1]);
        if x <= xb {
            return ya + (yb - ya) * (x - xa) / (xb - xa);
        }
    }
    table.last().map_or(0.0, |&(_, y)| y)
}

/// Converts a fan setting in percent into airflow in m³/h.
#[must_use]
pub fn airflow_m3h(pct: f64) -> f64 {
    interp(&FAN_TABLE, pct)
}

/// Converts a drum setting in percent into drum rpm.
#[must_use]
pub fn drum_rpm(pct: f64) -> f64 {
    interp(&DRUM_TABLE, pct)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Case = (&'static str, fn(f64) -> f64, f64, f64);

    #[test]
    fn converts_settings() {
        let cases: [Case; 7] = [
            ("drum at a table row", drum_rpm, 80.0, 52.0),
            ("drum between rows", drum_rpm, 82.5, 54.0),
            ("drum below range", drum_rpm, -10.0, 0.0),
            ("drum above range", drum_rpm, 150.0, 73.0),
            ("air at a table row", airflow_m3h, 45.0, 124.0),
            ("air between rows", airflow_m3h, 47.5, 130.0),
            ("air off", airflow_m3h, 0.0, 0.0),
        ];
        for (name, f, pct, want) in cases {
            let got = f(pct);
            assert!((got - want).abs() < 1e-9, "{name}: got {got}, want {want}");
        }
    }
}
