//! Roasts out of Roastro ("Exporting roasts, old data and backups", #54).

use i18n::t;
use std::fmt::Write as _;

use recorder::Millis;

use crate::wire::{Columns, RoastDoc};

/// Points first in a row, in this order; the others follow by name.
const FIRST: [&str; 2] = ["bt", "et"];

/// A number as Excel in Brazilian Portuguese reads it: decimal comma.
fn decimal(v: f64, places: usize) -> String {
    format!("{v:.places$}").replace('.', ",")
}

/// A field, quoted when it holds the separator, a quote or a line break.
fn field(text: &str) -> String {
    if text.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// `m:ss`, with a minus sign before Charge.
fn clock(s: f64) -> String {
    #[allow(clippy::cast_possible_truncation)] // a roast's seconds
    let whole = s.abs().round() as i64;
    let sign = if s < 0.0 { "-" } else { "" };
    format!("{sign}{}:{:02}", whole / 60, whole % 60)
}

/// "2026-11-16 08:05:12" in the computer's time zone.
fn instant(at: Millis) -> String {
    jiff::Timestamp::from_millisecond(at)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// One roast as CSV for a spreadsheet: a row per reading (roast clock,
/// instant, BT, ET, `RoR`, every other point), with each event and note on
/// the first reading at or after it. Written for Excel in Brazilian
/// Portuguese: UTF-8 with a BOM, `;` between fields, decimal comma.
#[must_use]
pub fn roast_csv(roast: &RoastDoc, columns: &Columns, time_scale: f64) -> String {
    let charge = roast
        .charge_at
        .unwrap_or_else(|| columns.at.first().copied().unwrap_or(0));
    #[allow(clippy::cast_precision_loss)] // milliseconds of a roast
    let roast_s = |at: Millis| (at - charge) as f64 * time_scale / 1000.0;
    let mut points: Vec<&str> = FIRST
        .iter()
        .copied()
        .filter(|p| columns.values.contains_key(*p))
        .collect();
    points.extend(
        columns
            .values
            .keys()
            .map(String::as_str)
            .filter(|p| !FIRST.contains(p)),
    );
    // What happened, placed on the first reading at or after it.
    let mut marks: Vec<(Vec<String>, Vec<String>)> =
        vec![(Vec::new(), Vec::new()); columns.at.len()];
    let row_of = |at: Millis| {
        columns
            .at
            .iter()
            .position(|&t| t >= at)
            .or_else(|| columns.at.len().checked_sub(1))
    };
    for e in &roast.events {
        if let Some(i) = row_of(e.at) {
            marks[i].0.push(e.kind.parse().map_or_else(
                |_| e.kind.replace('_', " "),
                |k| crate::engine::event_name(k).to_owned(),
            ));
        }
    }
    for n in &roast.notes {
        if let Some(i) = row_of(n.at) {
            marks[i].1.push(n.text.clone());
        }
    }

    let mut out = String::from("\u{feff}");
    let mut header = vec![
        t("Roast time (s)").to_owned(),
        t("Roast time").to_owned(),
        t("Instant").to_owned(),
    ];
    header.extend(points.iter().map(|p| match *p {
        "bt" => "BT (°C)".to_owned(),
        "et" => "ET (°C)".to_owned(),
        other => other.to_owned(),
    }));
    header.insert(5.min(header.len()), "RoR (°C/min)".to_owned());
    header.extend([t("Event").to_owned(), t("Note").to_owned()]);
    let _ = writeln!(out, "{}", header.join(";"));
    for (i, &at) in columns.at.iter().enumerate() {
        let s = roast_s(at);
        let mut row = vec![decimal(s, 1), clock(s), instant(at)];
        let value = |p: &str| {
            columns
                .values
                .get(p)
                .and_then(|c| c[i])
                .map_or_else(String::new, |v| decimal(v, 1))
        };
        row.extend(points.iter().map(|p| value(p)));
        let ror = columns.ror[i].map_or_else(String::new, |v| decimal(v, 1));
        row.insert(5.min(row.len()), ror);
        row.push(field(&marks[i].0.join(" · ")));
        row.push(field(&marks[i].1.join(" · ")));
        let _ = writeln!(out, "{}", row.join(";"));
    }
    out
}

/// A file name for a roast's export: `roast-41-2026-11-16.csv`.
#[must_use]
pub fn file_name(roast: &RoastDoc, extension: &str) -> String {
    let day = roast
        .charge_at
        .map(|at| instant(at).chars().take(10).collect::<String>())
        .unwrap_or_default();
    format!("roast-{}-{day}.{extension}", roast.number)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::*;
    use crate::wire::{EventOut, MetricsOut, NoteOut};

    #[test]
    fn writes_a_roast_for_excel_in_portuguese() {
        let roast = RoastDoc {
            id: Uuid::now_v7(),
            number: 41,
            state: "done",
            charge_at: Some(10_000),
            roasted_by: String::new(),
            green_weight_g: None,
            roasted_weight_g: None,
            coffee: None,
            reference: None,
            events: vec![EventOut {
                kind: "first_crack",
                at: 10_500,
                source: "manual",
                original_at: None,
            }],
            notes: vec![NoteOut {
                id: Uuid::now_v7(),
                at: 12_000,
                text: "gas 70; air \"up\"".into(),
            }],
            metrics: MetricsOut {
                duration_ms: None,
                phases: Vec::new(),
                development_time_ratio: None,
                weight_loss_ratio: None,
            },
            summary: None,
        };
        let columns = Columns {
            at: vec![9_000, 11_000, 12_000],
            ror: vec![None, Some(12.25), Some(11.0)],
            values: BTreeMap::from([
                ("fan".to_owned(), vec![Some(40.0), Some(40.0), Some(60.0)]),
                (
                    "bt".to_owned(),
                    vec![Some(200.0), Some(180.5), Some(170.04)],
                ),
                ("et".to_owned(), vec![Some(230.0), None, Some(220.0)]),
            ]),
        };
        let csv = roast_csv(&roast, &columns, 1.0);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with('\u{feff}'), "a BOM for Excel");
        assert_eq!(
            lines[0].trim_start_matches('\u{feff}'),
            "Roast time (s);Roast time;Instant;BT (°C);ET (°C);RoR (°C/min);fan;Event;Note"
        );
        let cells = |line: &str| line.split(';').map(str::to_owned).collect::<Vec<_>>();
        let lead_in = cells(lines[1]);
        assert_eq!(
            (lead_in[0].as_str(), lead_in[1].as_str()),
            ("-1,0", "-0:01")
        );
        let fc = cells(lines[2]);
        assert_eq!(
            (
                fc[3].as_str(),
                fc[4].as_str(),
                fc[5].as_str(),
                fc[7].as_str()
            ),
            ("180,5", "", "12,2", "First crack"),
            "decimal comma, a missing value left empty, the event on its row"
        );
        assert!(
            lines[3].ends_with(";\"gas 70; air \"\"up\"\"\""),
            "a note with ; and quotes stays one field: {}",
            lines[3]
        );
        assert_eq!(
            file_name(&roast, "csv").len(),
            "roast-41-2026-11-16.csv".len()
        );
    }
}
