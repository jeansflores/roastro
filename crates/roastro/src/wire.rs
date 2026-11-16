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
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // every message travels in an Arc; boxing buys nothing
pub enum Live {
    /// Everything at once, on every (re)connect.
    Snapshot(Snapshot),
    /// One reading.
    Sample(SampleOut),
    /// The current roast changed.
    Roast(RoastDoc),
    /// The PLC link changed.
    Link(Link),
    /// The Next roast changed.
    NextRoast(NextRoastOut),
    /// The machine in use changed, or its settings.
    Machine(MachineOut),
}

impl Live {
    /// The message as JSON text.
    #[must_use]
    pub fn to_json(&self) -> String {
        // Serializing these plain structs cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// The live screen's starting point.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// The machine in use.
    pub machine: MachineOut,
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
    /// No machine is set up yet.
    NotSetUp,
    /// Dialling, before the first answer or after Retry now.
    Connecting,
    /// Readings arrive.
    Connected,
    /// Reads fail; retrying on its own.
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
    /// What to check, in plain words, while down.
    pub advice: Option<String>,
}

/// The machine Roastro reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachineOut {
    pub name: String,
    /// `tcp://host:port`; empty until set up (and on the Demo machine).
    pub address: String,
    pub model: String,
    pub model_title: String,
    /// The built-in simulator: its roasts stay out of the real history.
    pub demo: bool,
}

impl From<store::Machine> for MachineOut {
    fn from(m: store::Machine) -> Self {
        Self {
            model_title: crate::machine::model_title(&m.model).to_owned(),
            name: m.name,
            address: m.address,
            model: m.model,
            demo: m.demo,
        }
    }
}

/// Setting up the machine, or testing settings before saving them.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MachineIn {
    #[serde(default)]
    pub name: String,
    /// As typed: `192.168.5.11`, `192.168.5.11:502` or `tcp://…`.
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub model: String,
    /// Use the Demo machine instead; the rest is ignored.
    #[serde(default)]
    pub demo: bool,
}

/// What one test read brought back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestOut {
    /// Every register-map point by name.
    pub values: BTreeMap<String, f64>,
    /// How long the read took, ms.
    pub latency_ms: u64,
}

/// A stretch when the machine stopped answering after having answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkDropOut {
    pub started_at: Millis,
    /// `null` while still down.
    pub ended_at: Option<Millis>,
    pub error: String,
    /// The roast being recorded then, if any.
    pub roast_number: Option<i64>,
}

/// The Next roast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NextRoastOut {
    /// Coffee to roast.
    pub coffee_id: Option<Uuid>,
    /// Green weight.
    pub green_weight_g: Option<i64>,
    /// Who roasts.
    pub roasted_by: String,
    /// Roast drawn behind.
    pub reference_id: Option<Uuid>,
    /// The chosen coffee's latest Done roast, offered as the reference.
    pub suggested_reference_id: Option<Uuid>,
    /// The coffee's name, for a summary.
    pub coffee_name: Option<String>,
    /// The reference's number, for a summary.
    pub reference_number: Option<i64>,
}

impl NextRoastOut {
    /// Reads it from the database, with the suggested reference.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn load(store: &Store) -> store::Result<Self> {
        let next = store.next_roast()?;
        let suggested = match next.coffee_id {
            Some(c) => store.latest_done_roast(c)?.map(|r| r.0),
            None => None,
        };
        let coffee_name = match next.coffee_id {
            Some(c) => store.coffee_name(c)?,
            None => None,
        };
        let reference_number = match next.reference_id {
            Some(r) => Some(store.roast(r)?.number),
            None => None,
        };
        Ok(Self {
            coffee_id: next.coffee_id,
            green_weight_g: next.green_weight_g,
            roasted_by: next.roasted_by,
            reference_id: next.reference_id.map(|r| r.0),
            suggested_reference_id: suggested,
            coffee_name,
            reference_number,
        })
    }
}

/// A roast for the UI: `GET /api/roasts/{id}` and the `roast` message.
#[derive(Debug, Clone, Serialize)]
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
    /// Figures read from the samples, for listings and the detail; only on a
    /// finished roast, and only where the engine was asked for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryOut>,
}

/// What a finished roast's samples say, beyond its events.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryOut {
    /// Bean temperature at Drop, °C.
    pub drop_bt: Option<f64>,
    /// The largest fall of the `RoR` within 30 s between Turning point and
    /// Drop, °C/min: a crash or a swing.
    pub ror_fall: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoffeeRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceRef {
    pub id: Uuid,
    pub number: i64,
    pub coffee_name: Option<String>,
    pub charge_at: Option<Millis>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventOut {
    pub kind: &'static str,
    pub at: Millis,
    pub source: &'static str,
    pub original_at: Option<Millis>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteOut {
    pub id: Uuid,
    pub at: Millis,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsOut {
    pub duration_ms: Option<Millis>,
    pub phases: Vec<PhaseOut>,
    pub development_time_ratio: Option<f64>,
    pub weight_loss_ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhaseOut {
    pub kind: &'static str,
    pub from: Millis,
    pub to: Millis,
    pub ratio: f64,
}

impl RoastDoc {
    /// Builds the document for roast `id`; `now` closes a running phase.
    ///
    /// # Errors
    ///
    /// Fails for an unknown roast or on a database error.
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
            summary: None,
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
#[must_use]
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
#[derive(Debug, Clone, Serialize)]
pub struct Columns {
    pub at: Vec<Millis>,
    pub ror: Vec<Option<f64>>,
    /// One column per point; `null` where a reading lacks it.
    pub values: BTreeMap<String, Vec<Option<f64>>>,
}

impl Columns {
    /// Turns rows into columns.
    #[must_use]
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

/// Reads a field that may be absent (leave alone) or `null` (clear):
/// absent is `None`, `null` is `Some(None)`.
#[allow(clippy::option_option)] // the two layers are the point: absent vs null
fn present<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(d).map(Some)
}

/// `PUT /api/roasts/{id}/events/{kind}`.
#[derive(Debug, serde::Deserialize)]
pub struct EventIn {
    pub at: Millis,
}

/// `POST /api/roasts/{id}/notes`.
#[derive(Debug, serde::Deserialize)]
pub struct NoteIn {
    pub at: Millis,
    pub text: String,
}

/// `PATCH /api/roasts/{id}/notes/{note_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct NotePatch {
    pub at: Option<Millis>,
    pub text: Option<String>,
}

/// `PATCH /api/roasts/{id}`: only the fields present change; `null` clears.
#[allow(clippy::option_option)] // absent (leave alone) vs null (clear)
#[derive(Debug, Default, serde::Deserialize)]
pub struct RoastPatchIn {
    #[serde(default, deserialize_with = "present")]
    pub coffee_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    pub green_weight_g: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    pub roasted_weight_g: Option<Option<i64>>,
    pub roasted_by: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub reference_id: Option<Option<Uuid>>,
}

/// `PUT /api/next-roast`: the whole Next roast.
#[derive(Debug, serde::Deserialize)]
pub struct NextRoastIn {
    pub coffee_id: Option<Uuid>,
    pub green_weight_g: Option<i64>,
    #[serde(default)]
    pub roasted_by: String,
    pub reference_id: Option<Uuid>,
}

/// A coffee.
#[derive(Debug, Clone, Serialize)]
pub struct CoffeeOut {
    pub id: Uuid,
    pub name: String,
    pub notes: String,
    pub lot: String,
    pub origin: String,
    pub process: String,
    pub supplier: String,
    /// The lot is finished.
    pub archived: bool,
    /// How it has roasted, in listings; absent when it has no finished roast.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<CoffeeStatsOut>,
}

/// A coffee's figures over its finished roasts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoffeeStatsOut {
    pub roasts: i64,
    pub last_number: i64,
    pub last_charge_at: Option<Millis>,
    pub green_weight_g: i64,
    pub average_development_time_ratio: Option<f64>,
    pub average_weight_loss_ratio: Option<f64>,
}

impl From<store::CoffeeStats> for CoffeeStatsOut {
    fn from(s: store::CoffeeStats) -> Self {
        Self {
            roasts: s.roasts,
            last_number: s.last_number,
            last_charge_at: s.last_charge_at,
            green_weight_g: s.green_weight_g,
            average_development_time_ratio: s.average_development_time_ratio,
            average_weight_loss_ratio: s.average_weight_loss_ratio,
        }
    }
}

impl From<store::Coffee> for CoffeeOut {
    fn from(c: store::Coffee) -> Self {
        Self {
            id: c.id,
            name: c.name,
            notes: c.notes,
            lot: c.lot,
            origin: c.origin,
            process: c.process,
            supplier: c.supplier,
            archived: c.archived,
            stats: None,
        }
    }
}

/// `POST /api/coffees`: a name, and the rest when known.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CoffeeIn {
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub lot: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub process: String,
    #[serde(default)]
    pub supplier: String,
}

/// `PATCH /api/coffees/{id}`: only the fields present change.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CoffeePatch {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub lot: Option<String>,
    pub origin: Option<String>,
    pub process: Option<String>,
    pub supplier: Option<String>,
    pub archived: Option<bool>,
}

/// `GET /api/roasts` query: which roasts a page lists. The default lists
/// every roast not discarded, newest first.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct RoastQueryIn {
    /// The next page: roasts older than this one.
    pub before: Option<Uuid>,
    /// Discarded roasts too.
    #[serde(default)]
    pub discarded: bool,
    /// Only this coffee's roasts.
    pub coffee: Option<Uuid>,
    /// Charged at or after (epoch ms).
    pub since: Option<Millis>,
    /// Charged before (epoch ms).
    pub until: Option<Millis>,
    /// A roast number, or text in its notes or coffee name.
    pub q: Option<String>,
}

/// `GET /api/roasts`: a page of roasts, newest first.
#[derive(Debug, Clone, Serialize)]
pub struct RoastPage {
    pub roasts: Vec<RoastDoc>,
    /// Pass as `?before=` for the next page; `null` on the last one.
    pub next_before: Option<Uuid>,
}
