//! The PDF report of a roast ("Exporting roasts, old data and backups", #54):
//! a first page with the header, the curve with the reference and the
//! phases, the figures, events and notes; then the readings every 30 s. To
//! print, or to attach to the batch. Drawn with PDF's own fonts, nothing
//! embedded.
// Points are f32 in PDF and f64 in the roast; converting is the job here.
// x, y, w, h, s, t read like the drawing's math.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]

use i18n::{t, tf};
use pdf_writer::{Content, Finish as _, Name, Pdf, Rect, Ref, Str};
use recorder::Millis;

use crate::wire::{Columns, RoastDoc};

/// A4, in points.
const PAGE: (f32, f32) = (595.0, 842.0);
const MARGIN: f32 = 40.0;
/// Readings in the table, every this many roast seconds.
const EVERY_S: f64 = 30.0;
const ROWS_PER_PAGE: usize = 58;

const SANS: Name = Name(b"F1");
const BOLD: Name = Name(b"F2");
const MONO: Name = Name(b"F3");

type Rgb = (f32, f32, f32);
const BT: Rgb = (0.86, 0.55, 0.05);
const ET: Rgb = (0.26, 0.42, 0.85);
const ROR: Rgb = (0.08, 0.6, 0.55);
const GRID: Rgb = (0.88, 0.88, 0.88);
const MUTED: Rgb = (0.45, 0.45, 0.45);
const INK: Rgb = (0.1, 0.1, 0.1);

/// The reference drawn behind the curve: its number and BT by roast second.
pub struct ReferenceCurve<'a> {
    /// "Roast #n".
    pub number: i64,
    /// Roast seconds and BT.
    pub points: &'a [(f64, Option<f64>)],
}

/// Text in the PDF's `WinAnsiEncoding`: Latin letters and the Portuguese
/// accents as they are, a few typographic marks mapped, anything else `?`.
fn win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            ' '..='~' => c as u8,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '€' => 0x80,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '−' => b'-',
            _ => b'?',
        })
        .collect()
}

/// `m:ss` of roast seconds, `-m:ss` before Charge.
fn clock(s: f64) -> String {
    let whole = s.abs().round() as i64;
    let sign = if s < 0.0 { "-" } else { "" };
    format!("{sign}{}:{:02}", whole / 60, whole % 60)
}

fn one(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{v:.1}"))
}

/// A page being written: its drawing and where the text cursor is.
struct Sheet {
    content: Content,
}

impl Sheet {
    fn new() -> Self {
        Self {
            content: Content::new(),
        }
    }

    fn text(&mut self, font: Name, size: f32, x: f32, y: f32, color: Rgb, text: &str) {
        let c = &mut self.content;
        c.set_fill_rgb(color.0, color.1, color.2);
        c.begin_text();
        c.set_font(font, size);
        c.next_line(x, y);
        c.show(Str(&win_ansi(text)));
        c.end_text();
    }

    fn line(&mut self, points: &[(f32, f32)], color: Rgb, width: f32, dash: Option<[f32; 2]>) {
        if points.len() < 2 {
            return;
        }
        let c = &mut self.content;
        c.save_state();
        c.set_stroke_rgb(color.0, color.1, color.2);
        c.set_line_width(width);
        if let Some(d) = dash {
            c.set_dash_pattern(d, 0.0);
        }
        c.move_to(points[0].0, points[0].1);
        for &(x, y) in &points[1..] {
            c.line_to(x, y);
        }
        c.stroke();
        c.restore_state();
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        let c = &mut self.content;
        c.save_state();
        c.set_fill_rgb(color.0, color.1, color.2);
        c.rect(x, y, w, h);
        c.fill_nonzero();
        c.restore_state();
    }
}

/// Splits text into lines of at most `width` characters, at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let last = lines.last_mut().expect("never empty");
        if !last.is_empty() && last.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_owned());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    lines
}

/// The roast as a PDF: page one with the curve, figures, events and notes;
/// then the readings every 30 s.
#[must_use]
#[allow(clippy::too_many_lines)] // one report, top to bottom
pub fn roast_pdf(
    roast: &RoastDoc,
    columns: &Columns,
    reference: Option<&ReferenceCurve<'_>>,
    time_scale: f64,
) -> Vec<u8> {
    let charge = roast
        .charge_at
        .unwrap_or_else(|| columns.at.first().copied().unwrap_or(0));
    let roast_s = |at: Millis| (at - charge) as f64 * time_scale / 1000.0;
    let column = |name: &str| columns.values.get(name);
    let bt = column("bt");
    let et = column("et");
    let event = |kind: &str| {
        roast
            .events
            .iter()
            .find(|e| e.kind == kind)
            .map(|e| roast_s(e.at))
    };
    let drop = event("drop");
    let first_crack = event("first_crack");
    let end = drop
        .or_else(|| columns.at.last().map(|&at| roast_s(at)))
        .unwrap_or(600.0);

    let mut first = Sheet::new();
    let (w, h) = PAGE;
    let mut y = h - MARGIN - 14.0;

    // Header.
    first.text(
        BOLD,
        20.0,
        MARGIN,
        y,
        INK,
        &tf("Roast #{n}", &[("n", &roast.number)]),
    );
    if let Some(at) = roast.charge_at {
        let when = jiff::Timestamp::from_millisecond(at)
            .map(|t| {
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%d/%m/%Y %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        first.text(SANS, 11.0, w - MARGIN - 90.0, y + 4.0, MUTED, &when);
    }
    y -= 20.0;
    let weights = match (roast.green_weight_g, roast.roasted_weight_g) {
        (Some(g), Some(r)) => {
            format!("{:.3} → {:.3} kg", g as f64 / 1000.0, r as f64 / 1000.0).replace('→', "->")
        }
        (Some(g), None) => tf(
            "{kg} kg green",
            &[("kg", &format!("{:.3}", g as f64 / 1000.0))],
        ),
        _ => String::new(),
    };
    let line = [
        roast
            .coffee
            .as_ref()
            .map_or_else(|| t("No coffee set").to_owned(), |c| c.name.clone()),
        roast.roasted_by.clone(),
        weights,
        roast
            .reference
            .as_ref()
            .map(|r| tf("reference Roast #{n}", &[("n", &r.number)]))
            .unwrap_or_default(),
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    first.text(SANS, 11.0, MARGIN, y, INK, &line);

    // The chart.
    let (cx0, cx1) = (MARGIN + 30.0, w - MARGIN - 24.0);
    let (cy0, cy1) = (y - 300.0, y - 14.0);
    let x_max = (end + 30.0).max(120.0);
    let x = |s: f64| cx0 + (cx1 - cx0) * (s.clamp(0.0, x_max) / x_max) as f32;
    let y_t = |t: f64| cy0 + (cy1 - cy0) * ((t.clamp(40.0, 250.0) - 50.0) / 200.0) as f32;
    let y_r = |r: f64| cy0 + (cy1 - cy0) * (r.clamp(0.0, 30.0) / 30.0) as f32;
    // Phase bands.
    let tint = |c: Rgb| {
        (
            1.0 - (1.0 - c.0) * 0.12,
            1.0 - (1.0 - c.1) * 0.12,
            1.0 - (1.0 - c.2) * 0.12,
        )
    };
    for p in &roast.metrics.phases {
        let color = match p.kind {
            "drying" => ET,
            "maillard" => BT,
            _ => ROR,
        };
        let (a, b) = (x(roast_s(p.from)), x(roast_s(p.to)));
        first.fill(a, cy0, (b - a).max(0.0), cy1 - cy0, tint(color));
    }
    let mut temp = 50.0;
    while temp <= 250.0 {
        first.line(&[(cx0, y_t(temp)), (cx1, y_t(temp))], GRID, 0.5, None);
        first.text(
            SANS,
            7.0,
            MARGIN,
            y_t(temp) - 2.0,
            MUTED,
            &format!("{temp:.0}°"),
        );
        temp += 50.0;
    }
    for r in [0.0, 10.0, 20.0, 30.0] {
        first.text(SANS, 7.0, cx1 + 4.0, y_r(r) - 2.0, ROR, &format!("{r:.0}"));
    }
    let step = if x_max > 900.0 { 120.0 } else { 60.0 };
    let mut s = 0.0;
    while s <= x_max {
        first.line(&[(x(s), cy0), (x(s), cy1)], GRID, 0.5, None);
        first.text(SANS, 7.0, x(s) - 8.0, cy0 - 10.0, MUTED, &clock(s));
        s += step;
    }
    // A curve broken where values are missing or out of the roast.
    let curve = |sheet: &mut Sheet,
                 values: &[(f64, Option<f64>)],
                 y: &dyn Fn(f64) -> f32,
                 color: Rgb,
                 width: f32,
                 dash: Option<[f32; 2]>| {
        let mut run: Vec<(f32, f32)> = Vec::new();
        for &(s, v) in values {
            if let Some(v) = v.filter(|_| (0.0..=x_max).contains(&s)) {
                run.push((x(s), y(v)));
            } else {
                sheet.line(&run, color, width, dash);
                run.clear();
            }
        }
        sheet.line(&run, color, width, dash);
    };
    let series = |c: Option<&Vec<Option<f64>>>| -> Vec<(f64, Option<f64>)> {
        columns
            .at
            .iter()
            .enumerate()
            .map(|(i, &at)| (roast_s(at), c.and_then(|c| c[i])))
            .collect()
    };
    if let Some(reference) = reference {
        curve(
            &mut first,
            reference.points,
            &y_t,
            MUTED,
            1.0,
            Some([4.0, 3.0]),
        );
    }
    curve(&mut first, &series(et), &y_t, ET, 1.0, None);
    let tp = event("turning_point").unwrap_or(f64::MAX);
    let ror: Vec<(f64, Option<f64>)> = columns
        .at
        .iter()
        .zip(&columns.ror)
        .map(|(&at, &r)| (roast_s(at), r.filter(|_| roast_s(at) >= tp)))
        .collect();
    curve(&mut first, &ror, &y_r, ROR, 0.8, None);
    curve(&mut first, &series(bt), &y_t, BT, 1.6, None);
    for e in &roast.events {
        if e.kind == "charge" {
            continue;
        }
        let s = roast_s(e.at);
        first.line(&[(x(s), cy0), (x(s), cy1)], MUTED, 0.4, Some([1.0, 2.0]));
        let name = match e.kind {
            "turning_point" => "TP",
            "dry_end" => "DE",
            "first_crack" => "FC",
            "second_crack" => "SC",
            other => other.parse().map_or(other, crate::engine::event_name),
        };
        let mark = format!("{name} {}", clock(s));
        // Near the right edge the label goes to the left of its line.
        let width = mark.chars().count() as f32 * 3.6;
        let lx = if x(s) + 2.0 + width > cx1 {
            x(s) - 2.0 - width
        } else {
            x(s) + 2.0
        };
        first.text(SANS, 7.0, lx, cy1 - 8.0, INK, &mark);
    }
    // Legend.
    let legend_y = cy0 - 24.0;
    let mut lx = cx0;
    let mut legend = vec![("BT", BT), ("ET", ET), (t("RoR (right axis)"), ROR)];
    let reference_label = reference.map(|r| tf("Roast #{n} (reference)", &[("n", &r.number)]));
    if let Some(label) = &reference_label {
        legend.push((label.as_str(), MUTED));
    }
    for (label, color) in legend {
        first.line(
            &[(lx, legend_y + 3.0), (lx + 14.0, legend_y + 3.0)],
            color,
            1.6,
            None,
        );
        first.text(SANS, 8.0, lx + 18.0, legend_y, INK, label);
        lx += 22.0 + label.len() as f32 * 4.6;
    }

    // Figures.
    let mut y = legend_y - 30.0;
    let summary = roast.summary.as_ref();
    let figures = [
        (t("Total time"), drop.map_or_else(|| "—".to_owned(), clock)),
        (
            t("First crack"),
            first_crack.map_or_else(|| "—".to_owned(), clock),
        ),
        (
            t("Development"),
            drop.zip(first_crack)
                .map_or_else(|| "—".to_owned(), |(d, f)| clock(d - f)),
        ),
        (
            "DTR",
            roast
                .metrics
                .development_time_ratio
                .map_or_else(|| "—".to_owned(), |d| format!("{:.1}%", d * 100.0)),
        ),
        (
            t("Drop BT"),
            format!("{}°", one(summary.and_then(|s| s.drop_bt))),
        ),
        (
            t("Weight loss"),
            roast
                .metrics
                .weight_loss_ratio
                .map_or_else(|| "—".to_owned(), |l| format!("{:.1}%", l * 100.0)),
        ),
        (t("RoR fall"), one(summary.and_then(|s| s.ror_fall))),
    ];
    let col = (w - 2.0 * MARGIN) / figures.len() as f32;
    for (i, (label, value)) in figures.iter().enumerate() {
        let fx = MARGIN + col * i as f32;
        first.text(SANS, 6.5, fx, y + 14.0, MUTED, &label.to_uppercase());
        first.text(BOLD, 13.0, fx, y, INK, value);
    }

    // Phases.
    y -= 30.0;
    let total = roast
        .metrics
        .duration_ms
        .map_or(0.0, |ms| ms as f64 * time_scale / 1000.0);
    let phases: Vec<String> = roast
        .metrics
        .phases
        .iter()
        .map(|p| {
            let name = match p.kind {
                "drying" => t("Drying"),
                "maillard" => t("Maillard"),
                _ => t("Development"),
            };
            format!(
                "{name} {} ({:.0}%)",
                clock(roast_s(p.to) - roast_s(p.from)),
                p.ratio * 100.0
            )
        })
        .collect();
    first.text(BOLD, 9.0, MARGIN, y, INK, &t("Phases").to_uppercase());
    first.text(SANS, 10.0, MARGIN + 60.0, y, INK, &phases.join("   ·   "));
    if total > 0.0 {
        y -= 4.0;
    }

    // Events and notes, side by side.
    y -= 24.0;
    let top = y;
    first.text(BOLD, 9.0, MARGIN, y, INK, &t("Events").to_uppercase());
    for e in &roast.events {
        y -= 13.0;
        let at = roast_s(e.at);
        let bt_then = columns
            .at
            .iter()
            .position(|&t| t >= e.at)
            .and_then(|i| bt.and_then(|c| c[i]));
        let name = e.kind.parse().map_or_else(
            |_| e.kind.replace('_', " "),
            |k| crate::engine::event_name(k).to_owned(),
        );
        first.text(MONO, 9.0, MARGIN, y, INK, &format!("{:>6}", clock(at)));
        first.text(SANS, 9.5, MARGIN + 44.0, y, INK, &name);
        first.text(
            SANS,
            9.5,
            MARGIN + 140.0,
            y,
            BT,
            &format!("{}°", one(bt_then)),
        );
        let source = match e.source {
            "auto" => t("auto"),
            "manual" => t("manual"),
            "computed" => t("computed"),
            other => other,
        };
        first.text(SANS, 8.0, MARGIN + 190.0, y, MUTED, source);
    }
    let nx = w / 2.0;
    let mut ny = top;
    first.text(BOLD, 9.0, nx, ny, INK, &t("Notes").to_uppercase());
    if roast.notes.is_empty() {
        ny -= 13.0;
        first.text(SANS, 9.5, nx, ny, MUTED, t("No notes."));
    }
    for n in &roast.notes {
        for (i, line) in wrap(&n.text, 52).into_iter().enumerate() {
            ny -= 13.0;
            if ny < MARGIN {
                break;
            }
            if i == 0 {
                first.text(
                    MONO,
                    9.0,
                    nx,
                    ny,
                    INK,
                    &format!("{:>6}", clock(roast_s(n.at))),
                );
            }
            first.text(SANS, 9.5, nx + 44.0, ny, INK, &line);
        }
    }

    // The readings every 30 s.
    let mut rows: Vec<String> = Vec::new();
    let mut next =
        (roast_s(columns.at.first().copied().unwrap_or(charge)) / EVERY_S).ceil() * EVERY_S;
    let pick = |name: &str, i: usize| column(name).and_then(|c| c[i]);
    for (i, &at) in columns.at.iter().enumerate() {
        let s = roast_s(at);
        if s + 0.5 < next {
            continue;
        }
        next += EVERY_S;
        rows.push(format!(
            "{:>7}  {:>7}  {:>7}  {:>7}  {:>6}  {:>6}  {:>6}",
            clock(s),
            one(pick("bt", i)),
            one(pick("et", i)),
            one(columns.ror[i].filter(|_| s >= tp)),
            pick("gas_fb", i)
                .or_else(|| pick("gas_cmd", i))
                .map_or_else(|| "—".to_owned(), |v| format!("{v:.0}")),
            pick("fan", i).map_or_else(|| "—".to_owned(), |v| format!("{v:.0}")),
            pick("drum", i).map_or_else(|| "—".to_owned(), |v| format!("{v:.0}")),
        ));
    }
    let mut sheets = vec![first];
    for chunk in rows.chunks(ROWS_PER_PAGE) {
        let mut sheet = Sheet::new();
        let mut y = h - MARGIN - 12.0;
        sheet.text(
            BOLD,
            12.0,
            MARGIN,
            y,
            INK,
            &tf("Roast #{n}: readings every 30 s", &[("n", &roast.number)]),
        );
        y -= 22.0;
        sheet.text(
            MONO,
            9.0,
            MARGIN,
            y,
            MUTED,
            &format!(
                "{:>7}  {:>7}  {:>7}  {:>7}  {:>6}  {:>6}  {:>6}",
                t("Time"),
                "BT",
                "ET",
                "RoR",
                t("Gas"),
                t("Air"),
                t("Drum")
            ),
        );
        for row in chunk {
            y -= 12.5;
            sheet.text(MONO, 9.0, MARGIN, y, INK, row);
        }
        sheets.push(sheet);
    }

    // The document.
    let mut doc = Pdf::new();
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let fonts = [
        (SANS, Ref::new(3), "Helvetica"),
        (BOLD, Ref::new(4), "Helvetica-Bold"),
        (MONO, Ref::new(5), "Courier"),
    ];
    let pages: Vec<(Ref, Ref)> = (0..sheets.len())
        .map(|i| {
            let n = i32::try_from(i).unwrap_or(i32::MAX);
            (Ref::new(10 + 2 * n), Ref::new(11 + 2 * n))
        })
        .collect();
    doc.catalog(catalog).pages(tree);
    doc.pages(tree)
        .kids(pages.iter().map(|p| p.0))
        .count(i32::try_from(pages.len()).unwrap_or(i32::MAX));
    for (name, id, base) in fonts {
        let _ = name;
        doc.type1_font(id)
            .base_font(Name(base.as_bytes()))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    for (sheet, (page_id, content_id)) in sheets.into_iter().zip(&pages) {
        let mut page = doc.page(*page_id);
        page.media_box(Rect::new(0.0, 0.0, w, h));
        page.parent(tree);
        page.contents(*content_id);
        let mut resources = page.resources();
        let mut font_dict = resources.fonts();
        for (name, id, _) in fonts {
            font_dict.pair(name, id);
        }
        font_dict.finish();
        resources.finish();
        page.finish();
        doc.stream(*content_id, &sheet.content.finish());
    }
    doc.finish()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::*;
    use crate::wire::{EventOut, MetricsOut, NoteOut};

    #[test]
    fn writes_the_report_and_the_readings_pages() {
        let event = |kind, at| EventOut {
            kind,
            at,
            source: "auto",
            original_at: None,
        };
        let roast = RoastDoc {
            id: Uuid::now_v7(),
            number: 41,
            state: "done",
            charge_at: Some(0),
            roasted_by: "Ana".into(),
            green_weight_g: Some(5000),
            roasted_weight_g: Some(4200),
            coffee: None,
            reference: None,
            events: vec![
                event("charge", 0),
                event("first_crack", 480_000),
                event("drop", 600_000),
            ],
            notes: vec![NoteOut {
                id: Uuid::now_v7(),
                at: 300_000,
                text: "Café com açúcar e canela — 🔥".into(),
            }],
            metrics: MetricsOut {
                duration_ms: Some(600_000),
                phases: Vec::new(),
                development_time_ratio: Some(0.2),
                weight_loss_ratio: Some(0.16),
            },
            summary: None,
        };
        // A reading a second for 50 minutes: enough readings for three pages.
        let at: Vec<Millis> = (0..3000).map(|s| s * 1000).collect();
        let columns = Columns {
            ror: vec![Some(10.0); at.len()],
            values: BTreeMap::from([(
                "bt".to_owned(),
                at.iter()
                    .map(|&t| Some(100.0 + t as f64 / 10_000.0))
                    .collect(),
            )]),
            at,
        };
        let pdf = roast_pdf(&roast, &columns, None, 1.0);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-"));
        assert!(
            text.contains("/Count 3"),
            "page one and two pages of readings"
        );
        assert!(text.contains("/WinAnsiEncoding"));
        assert_eq!(
            win_ansi("ação − é")[..4],
            [b'a', 0xe7, 0xe3, b'o'],
            "Portuguese stays"
        );
        assert_eq!(win_ansi("🔥"), b"?");
    }

    #[test]
    fn wraps_long_notes() {
        assert_eq!(wrap("a bb ccc dddd", 6), ["a bb", "ccc", "dddd"]);
    }
}
