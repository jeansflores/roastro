//! What travels to the web UI, as settled in "Contract between the backend
//! and the web UI": `snake_case` JSON, instants in epoch milliseconds,
//! weights in grams, ratios as fractions.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use recorder::{Millis, Sample};
use roast::{EventKind, metrics, ror_series};
use serde::Serialize;
use store::{RoastId, RoastState, Store};
use uuid::Uuid;

/// A message on the live stream (`GET /api/live`): `{ "type", "data" }`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Live {
    /// Everything at once, on every (re)connect.
    Snapshot(Snapshot),
    /// One reading.
    Sample(SampleOut),
    /// The current roast changed.
    Roast(RoastDoc),
    /// The PLC link changed.
    Link(Link),
}

impl Live {
    /// The message as JSON text.
    pub fn to_json(&self) -> String {
        // Serializing these plain structs cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// The live screen's starting point.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    /// The PLC link.
    pub link: Link,
    /// What the operator prepared for the next roast.
    pub next_roast: NextRoastOut,
    /// The current roast (Recording, or the last one until the next Charge).
    pub roast: Option<RoastDoc>,
    /// The current roast's readings so far, or the last minute while waiting.
    pub samples: Vec<SampleOut>,
}

/// One reading.
#[derive(Debug, Clone, Serialize)]
pub struct SampleOut {
    /// When it was read.
    pub at: Millis,
    /// Every register-map point by name.
    pub values: HashMap<String, f64>,
    /// BT rate of rise in °C/min, during a roast.
    pub ror: Option<f64>,
}

/// State of the PLC link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Readings arrive.
    Connected,
    /// Reads fail.
    Down,
}

/// The PLC link, as the `link` message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    /// Connected or down.
    pub state: LinkState,
    /// Since when.
    pub since: Millis,
    /// The last error, while down.
    pub error: Option<String>,
    /// When the next attempt happens, while down.
    pub retry_at: Option<Millis>,
}

/// The Next roast.
#[derive(Debug, Serialize)]
pub struct NextRoastOut {
    /// Coffee to roast.
    pub coffee_id: Option<Uuid>,
    /// Green weight.
    pub green_weight_g: Option<i64>,
    /// Who roasts.
    pub roasted_by: String,
    /// Roast drawn behind.
    pub reference_id: Option<Uuid>,
}

impl NextRoastOut {
    /// Reads it from the database.
    pub fn load(store: &Store) -> store::Result<Self> {
        let next = store.next_roast()?;
        Ok(Self {
            coffee_id: next.coffee_id,
            green_weight_g: next.green_weight_g,
            roasted_by: next.roasted_by,
            reference_id: next.reference_id.map(|r| r.0),
        })
    }
}

/// A roast for the UI: `GET /api/roasts/{id}` and the `roast` message.
#[derive(Debug, Serialize)]
pub struct RoastDoc {
    pub id: Uuid,
    pub number: i64,
    /// `recording`, `done`, `incomplete` or `discarded`.
    pub state: &'static str,
    pub charge_at: Option<Millis>,
    pub roasted_by: String,
    pub green_weight_g: Option<i64>,
    pub roasted_weight_g: Option<i64>,
    pub coffee: Option<CoffeeRef>,
    pub reference: Option<ReferenceRef>,
    pub events: Vec<EventOut>,
    pub notes: Vec<NoteOut>,
    pub metrics: MetricsOut,
}

#[derive(Debug, Serialize)]
pub struct CoffeeRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ReferenceRef {
    pub id: Uuid,
    pub number: i64,
    pub coffee_name: Option<String>,
    pub charge_at: Option<Millis>,
}

#[derive(Debug, Serialize)]
pub struct EventOut {
    pub kind: &'static str,
    pub at: Millis,
    pub source: &'static str,
    pub original_at: Option<Millis>,
}

#[derive(Debug, Serialize)]
pub struct NoteOut {
    pub id: Uuid,
    pub at: Millis,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct MetricsOut {
    pub duration_ms: Option<Millis>,
    pub phases: Vec<PhaseOut>,
    pub development_time_ratio: Option<f64>,
    pub weight_loss_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PhaseOut {
    pub kind: &'static str,
    pub from: Millis,
    pub to: Millis,
    pub ratio: f64,
}

impl RoastDoc {
    /// Builds the document for roast `id`; `now` closes a running phase.
    pub fn load(store: &Store, id: RoastId, now: Millis) -> store::Result<Self> {
        let r = store.roast(id)?;
        let charge_at = r
            .events
            .iter()
            .find(|e| e.kind == EventKind::Charge)
            .map(|e| e.at);
        let coffee = match r.coffee_id {
            Some(c) => store.coffee_name(c)?.map(|name| CoffeeRef { id: c, name }),
            None => None,
        };
        let reference = match r.reference_id {
            Some(ref_id) => {
                let referenced = store.roast(ref_id)?;
                Some(ReferenceRef {
                    id: ref_id.0,
                    number: referenced.number,
                    coffee_name: match referenced.coffee_id {
                        Some(c) => store.coffee_name(c)?,
                        None => None,
                    },
                    charge_at: referenced
                        .events
                        .iter()
                        .find(|e| e.kind == EventKind::Charge)
                        .map(|e| e.at),
                })
            }
            None => None,
        };
        let pairs: Vec<(EventKind, Millis)> = r.events.iter().map(|e| (e.kind, e.at)).collect();
        let m = metrics(&pairs, now, r.green_weight_g, r.roasted_weight_g);
        Ok(Self {
            id: r.id.0,
            number: r.number,
            state: if r.discarded_at.is_some() {
                "discarded"
            } else {
                state_name(r.state)
            },
            charge_at,
            roasted_by: r.roasted_by,
            green_weight_g: r.green_weight_g,
            roasted_weight_g: r.roasted_weight_g,
            coffee,
            reference,
            events: r
                .events
                .iter()
                .map(|e| EventOut {
                    kind: e.kind.as_str(),
                    at: e.at,
                    source: e.source.as_str(),
                    original_at: e.original_at,
                })
                .collect(),
            notes: store
                .notes(id)?
                .into_iter()
                .map(|n| NoteOut {
                    id: n.id,
                    at: n.at,
                    text: n.text,
                })
                .collect(),
            metrics: MetricsOut {
                duration_ms: m.duration_ms,
                phases: m
                    .phases
                    .iter()
                    .map(|p| PhaseOut {
                        kind: p.kind.as_str(),
                        from: p.from,
                        to: p.to,
                        ratio: p.ratio,
                    })
                    .collect(),
                development_time_ratio: m.development_time_ratio,
                weight_loss_ratio: m.weight_loss_ratio,
            },
        })
    }
}

fn state_name(state: RoastState) -> &'static str {
    state.as_str()
}

/// How rate of rise is computed everywhere (same window as the live `RoR`).
#[derive(Debug, Clone, Copy)]
pub struct RorSettings {
    /// Look-back window.
    pub window: Duration,
    /// Readings further apart than this restart it (a Gap).
    pub gap: Duration,
    /// Simulator speed (1 on the machine).
    pub time_scale: f64,
}

/// A roast's readings with their rate of rise, which starts at Charge.
pub fn samples_with_ror(
    samples: Vec<Sample>,
    charge_at: Option<Millis>,
    ror: RorSettings,
) -> Vec<SampleOut> {
    let start = charge_at.unwrap_or(Millis::MAX);
    let from_charge: Vec<(Millis, Option<f64>)> = samples
        .iter()
        .filter(|s| s.at >= start)
        .map(|s| (s.at, s.values.get("bt").copied()))
        .collect();
    let mut rates = ror_series(&from_charge, ror.window, ror.gap, ror.time_scale).into_iter();
    samples
        .into_iter()
        .map(|s| {
            let rate = if s.at >= start {
                rates.next().flatten()
            } else {
                None
            };
            SampleOut {
                at: s.at,
                values: s.values,
                ror: rate,
            }
        })
        .collect()
}

/// Readings in columns, as uPlot draws them (`GET /api/roasts/{id}/samples`).
#[derive(Debug, Serialize)]
pub struct Columns {
    pub at: Vec<Millis>,
    pub ror: Vec<Option<f64>>,
    /// One column per point; `null` where a reading lacks it.
    pub values: BTreeMap<String, Vec<Option<f64>>>,
}

impl Columns {
    /// Turns rows into columns.
    pub fn from_rows(rows: &[SampleOut]) -> Self {
        let mut names: Vec<&String> = rows.iter().flat_map(|r| r.values.keys()).collect();
        names.sort_unstable();
        names.dedup();
        let values = names
            .into_iter()
            .map(|n| {
                (
                    n.clone(),
                    rows.iter().map(|r| r.values.get(n).copied()).collect(),
                )
            })
            .collect();
        Self {
            at: rows.iter().map(|r| r.at).collect(),
            ror: rows.iter().map(|r| r.ror).collect(),
            values,
        }
    }
}
