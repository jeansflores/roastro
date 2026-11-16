//! Roastro's `SQLite` database: the schema, its migrations and the roast
//! record (decided in "What a Roast record holds" and "`SQLite` in Go for
//! Roastro", carried over to Rust in "Rust stack for Roastro").
//!
//! - Migrations are numbered `.sql` files embedded in the binary. Each runs
//!   in a transaction that also bumps `PRAGMA user_version`.
//! - One connection, WAL journal, foreign keys on.
//! - Instants are epoch milliseconds (ADR 0001), weights whole grams, ids
//!   `UUIDv7`.
//! - Every sample is written as it arrives, so a crash loses at most the one
//!   in flight.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use recorder::{Closure, Millis, Sample};
use roast::{EventKind, Source};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

mod edit;

pub use edit::{
    Coffee, CoffeeChange, CoffeeStats, DEMO_MACHINE, LinkDrop, Machine, RoastPatch, RoastQuery,
};

/// The migrations, in order. Never edit one that has shipped; add a new one.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_coffee_details.sql"),
    include_str!("../migrations/0003_link_drop.sql"),
    include_str!("../migrations/0004_machine_settings.sql"),
];

/// Name of the machine row created with a new database.
const DEFAULT_MACHINE: &str = "Caloratto 5 kg";

/// Why a database call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `SQLite` refused or failed.
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored sample is not valid JSON.
    #[error("database: sample values: {0}")]
    Json(#[from] serde_json::Error),
    /// A stored value is not what the schema allows.
    #[error("database: corrupt value {0:?}")]
    Corrupt(String),
    /// No roast with this id.
    #[error("no roast {0}")]
    NoRoast(RoastId),
    /// The file is not a Roastro database.
    #[error("{0} is not a Roastro backup")]
    NotABackup(String),
    /// The backup comes from a newer Roastro.
    #[error("{0} comes from a newer Roastro: update this one first")]
    NewerBackup(String),
}

/// A database result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A roast's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoastId(pub Uuid);

impl fmt::Display for RoastId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where a roast is in its lifecycle. Discarding is separate (see
/// [`Roast::discarded_at`]), so restoring brings the roast back as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoastState {
    /// Between Charge and Drop.
    Recording,
    /// Drop happened.
    Done,
    /// Closed without Drop.
    Incomplete,
}

impl RoastState {
    /// The stable name used in storage and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RoastState::Recording => "recording",
            RoastState::Done => "done",
            RoastState::Incomplete => "incomplete",
        }
    }
}

impl FromStr for RoastState {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "recording" => Ok(RoastState::Recording),
            "done" => Ok(RoastState::Done),
            "incomplete" => Ok(RoastState::Incomplete),
            _ => Err(Error::Corrupt(s.to_owned())),
        }
    }
}

/// One stored roast event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Which event.
    pub kind: EventKind,
    /// When it happened.
    pub at: Millis,
    /// How its time was decided.
    pub source: Source,
    /// The automatic time, kept when a person corrected it.
    pub original_at: Option<Millis>,
}

/// A roast as stored, without its samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roast {
    /// Id.
    pub id: RoastId,
    /// Per-machine sequence number, for display ("Roast #41").
    pub number: i64,
    /// Lifecycle state.
    pub state: RoastState,
    /// When it was discarded, if it was.
    pub discarded_at: Option<Millis>,
    /// The coffee roasted, once someone says.
    pub coffee_id: Option<Uuid>,
    /// Who roasted it (free text).
    pub roasted_by: String,
    /// Green weight in grams.
    pub green_weight_g: Option<i64>,
    /// Roasted weight in grams.
    pub roasted_weight_g: Option<i64>,
    /// The roast it was roasted against.
    pub reference_id: Option<RoastId>,
    /// Its events, in roast order.
    pub events: Vec<Event>,
}

/// A free-text comment on an instant of a roast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Id.
    pub id: Uuid,
    /// The instant it is about.
    pub at: Millis,
    /// What the operator wrote.
    pub text: String,
}

/// What the operator prepares before Charge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NextRoast {
    /// Coffee to roast.
    pub coffee_id: Option<Uuid>,
    /// Green weight in grams.
    pub green_weight_g: Option<i64>,
    /// Who roasts.
    pub roasted_by: String,
    /// Roast to draw behind it.
    pub reference_id: Option<RoastId>,
}

/// The database. One per process; not shared between threads.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    machine_id: Uuid,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened or a migration fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    ///
    /// # Errors
    ///
    /// Fails if a migration fails.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        let machine_id = current_machine(&conn)?;
        Ok(Self { conn, machine_id })
    }

    /// The schema version, i.e. how many migrations have run.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn schema_version(&self) -> Result<usize> {
        let v: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        usize::try_from(v).map_err(|_| Error::Corrupt(v.to_string()))
    }

    /// Records a Charge: a new Recording roast numbered after the last one,
    /// its Charge event, its lead-in readings, and the Next roast's fields.
    ///
    /// # Errors
    ///
    /// Fails on a database error; nothing is written then.
    pub fn start_roast(
        &mut self,
        charge_at: Millis,
        source: Source,
        lead_in: &[Sample],
    ) -> Result<RoastId> {
        let tx = self.conn.transaction()?;
        let id = RoastId(Uuid::now_v7());
        let number: i64 = tx.query_row(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM roast WHERE machine_id = ?1",
            [self.machine_id.to_string()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO roast (id, machine_id, number, state, coffee_id, roasted_by, green_weight_g, reference_id)
             SELECT ?1, ?2, ?3, 'recording', coffee_id, roasted_by, green_weight_g, reference_id FROM next_roast WHERE id = 1",
            params![id.to_string(), self.machine_id.to_string(), number],
        )?;
        insert_event(&tx, id, EventKind::Charge, charge_at, source, None)?;
        for s in lead_in {
            insert_sample(&tx, id, s)?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Appends one reading to a roast. A second reading at the same instant
    /// replaces the first.
    ///
    /// # Errors
    ///
    /// Fails on a database error or an unknown roast.
    pub fn add_sample(&mut self, roast: RoastId, s: &Sample) -> Result<()> {
        let tx = self.conn.transaction()?;
        insert_sample(&tx, roast, s)?;
        tx.commit()?;
        Ok(())
    }

    /// Records an event, replacing any earlier one of the same kind. When a
    /// person moves an automatic or computed event, its first time is kept in
    /// `original_at`. A Drop makes a Recording or Incomplete roast Done.
    ///
    /// # Errors
    ///
    /// Fails on a database error or an unknown roast.
    pub fn set_event(
        &mut self,
        roast: RoastId,
        kind: EventKind,
        at: Millis,
        source: Source,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let previous: Option<(String, Option<Millis>, Millis)> = tx
            .query_row(
                "SELECT source, original_at, at FROM roast_event WHERE roast_id = ?1 AND kind = ?2",
                params![roast.to_string(), kind.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let original_at = match previous {
            Some((_, Some(original), _)) => Some(original),
            Some((prev_source, None, prev_at))
                if source == Source::Manual && prev_source != Source::Manual.as_str() =>
            {
                Some(prev_at)
            }
            _ => None,
        };
        tx.execute(
            "DELETE FROM roast_event WHERE roast_id = ?1 AND kind = ?2",
            params![roast.to_string(), kind.as_str()],
        )?;
        insert_event(&tx, roast, kind, at, source, original_at)?;
        if kind == EventKind::Drop {
            tx.execute(
                "UPDATE roast SET state = 'done' WHERE id = ?1 AND state IN ('recording', 'incomplete')",
                [roast.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records that a roast stopped receiving samples. A roast closed without
    /// Drop becomes Incomplete; one already Done stays Done.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn close_roast(&mut self, roast: RoastId, closure: Closure) -> Result<()> {
        if closure.is_incomplete() {
            self.conn.execute(
                "UPDATE roast SET state = 'incomplete' WHERE id = ?1 AND state = 'recording'",
                [roast.to_string()],
            )?;
        }
        Ok(())
    }

    /// The roast still Recording, if any: after a restart it is resumed across
    /// the Gap ("Recording through a PLC dropout or an app restart").
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn recording_roast(&self) -> Result<Option<RoastId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM roast WHERE machine_id = ?1 AND state = 'recording' ORDER BY number DESC LIMIT 1",
                [self.machine_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|id| parse_uuid(&id).map(RoastId)).transpose()
    }

    /// The instant of a roast's last reading, if it has any.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn last_sample_at(&self, roast: RoastId) -> Result<Option<Millis>> {
        Ok(self.conn.query_row(
            "SELECT MAX(at) FROM sample WHERE roast_id = ?1",
            [roast.to_string()],
            |r| r.get(0),
        )?)
    }

    /// One roast with its events, or [`Error::NoRoast`].
    ///
    /// # Errors
    ///
    /// Fails on a database error or an unknown roast.
    pub fn roast(&self, id: RoastId) -> Result<Roast> {
        let row = self
            .conn
            .query_row(
                "SELECT number, state, discarded_at, coffee_id, roasted_by, green_weight_g, roasted_weight_g, reference_id
                 FROM roast WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<Millis>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or(Error::NoRoast(id))?;
        let (
            number,
            state,
            discarded_at,
            coffee_id,
            roasted_by,
            green_weight_g,
            roasted_weight_g,
            reference_id,
        ) = row;
        Ok(Roast {
            id,
            number,
            state: state.parse()?,
            discarded_at,
            coffee_id: coffee_id.as_deref().map(parse_uuid).transpose()?,
            roasted_by,
            green_weight_g,
            roasted_weight_g,
            reference_id: reference_id
                .as_deref()
                .map(|r| parse_uuid(r).map(RoastId))
                .transpose()?,
            events: self.events(id)?,
        })
    }

    fn events(&self, id: RoastId) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, at, source, original_at FROM roast_event WHERE roast_id = ?1")?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Millis>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<Millis>>(3)?,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (kind, at, source, original_at) = row?;
            events.push(Event {
                kind: kind.parse().map_err(|_| Error::Corrupt(kind.clone()))?,
                at,
                source: source.parse().map_err(|_| Error::Corrupt(source.clone()))?,
                original_at,
            });
        }
        events.sort_by_key(|e| e.kind);
        Ok(events)
    }

    /// A roast's readings, oldest first.
    ///
    /// # Errors
    ///
    /// Fails on a database error or a corrupt sample.
    pub fn samples(&self, id: RoastId) -> Result<Vec<Sample>> {
        let mut stmt = self
            .conn
            .prepare("SELECT at, vals FROM sample WHERE roast_id = ?1 ORDER BY at")?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((r.get::<_, Millis>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut samples = Vec::new();
        for row in rows {
            let (at, vals) = row?;
            let values: HashMap<String, f64> = serde_json::from_str(&vals)?;
            samples.push(Sample { at, values });
        }
        Ok(samples)
    }

    /// What the operator prepared for the next roast.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn next_roast(&self) -> Result<NextRoast> {
        let (coffee, green, roasted_by, reference): (Option<String>, Option<i64>, String, Option<String>) =
            self.conn.query_row(
                "SELECT coffee_id, green_weight_g, roasted_by, reference_id FROM next_roast WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        Ok(NextRoast {
            coffee_id: coffee.as_deref().map(parse_uuid).transpose()?,
            green_weight_g: green,
            roasted_by,
            reference_id: reference
                .as_deref()
                .map(|r| parse_uuid(r).map(RoastId))
                .transpose()?,
        })
    }

    /// Replaces the Next roast.
    ///
    /// # Errors
    ///
    /// Fails on a database error, e.g. an unknown coffee or roast.
    pub fn set_next_roast(&mut self, next: &NextRoast) -> Result<()> {
        self.conn.execute(
            "UPDATE next_roast SET coffee_id = ?1, green_weight_g = ?2, roasted_by = ?3, reference_id = ?4 WHERE id = 1",
            params![
                next.coffee_id.map(|c| c.to_string()),
                next.green_weight_g,
                next.roasted_by,
                next.reference_id.map(|r| r.to_string()),
            ],
        )?;
        Ok(())
    }

    /// A coffee's name, if it exists.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn coffee_name(&self, id: Uuid) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name FROM coffee WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// A roast's notes, in roast-clock order.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn notes(&self, roast: RoastId) -> Result<Vec<Note>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, at, text FROM roast_note WHERE roast_id = ?1 ORDER BY at, id")?;
        let rows = stmt.query_map([roast.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Millis>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut notes = Vec::new();
        for row in rows {
            let (id, at, text) = row?;
            notes.push(Note {
                id: parse_uuid(&id)?,
                at,
                text,
            });
        }
        Ok(notes)
    }

    /// Adds a coffee and returns its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn add_coffee(&mut self, name: &str) -> Result<Uuid> {
        let id = Uuid::now_v7();
        self.conn.execute(
            "INSERT INTO coffee (id, name) VALUES (?1, ?2)",
            params![id.to_string(), name],
        )?;
        Ok(id)
    }
}

/// The machine in use, else the first real one; made on first open.
fn current_machine(conn: &Connection) -> Result<Uuid> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT COALESCE(
                (SELECT m.id FROM machine m JOIN app_setting s
                   ON s.key = 'current_machine' AND s.value = m.id),
                (SELECT id FROM machine WHERE demo = 0 ORDER BY rowid LIMIT 1))",
            [],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(id) = existing {
        return parse_uuid(&id);
    }
    let id = Uuid::now_v7();
    conn.execute(
        "INSERT INTO machine (id, name) VALUES (?1, ?2)",
        params![id.to_string(), DEFAULT_MACHINE],
    )?;
    Ok(id)
}

/// What a backup file holds, read before restoring it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// Roasts in it, discarded ones included.
    pub roasts: i64,
    /// When its newest roast was charged.
    pub last_charge_at: Option<Millis>,
    /// Coffees in it.
    pub coffees: i64,
}

impl Store {
    /// Writes a consistent copy of the whole database to `path`, even while
    /// roasts are being recorded.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn back_up(&self, path: &Path) -> Result<()> {
        self.conn.backup(
            rusqlite::MAIN_DB,
            path,
            None::<fn(rusqlite::backup::Progress)>,
        )?;
        Ok(())
    }

    /// Reads what a backup holds, refusing a file that is not a Roastro
    /// database or comes from a newer Roastro.
    ///
    /// # Errors
    ///
    /// [`Error::NotABackup`] for such a file; a database error otherwise.
    pub fn inspect_backup(path: &Path) -> Result<BackupInfo> {
        let not_a_backup = || Error::NotABackup(path.display().to_string());
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| not_a_backup())?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|_| not_a_backup())?;
        let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
        if version < 1 {
            return Err(not_a_backup());
        }
        if version > known {
            return Err(Error::NewerBackup(path.display().to_string()));
        }
        conn.query_row(
            "SELECT (SELECT COUNT(*) FROM roast),
                    (SELECT MAX(at) FROM roast_event WHERE kind = 'charge'),
                    (SELECT COUNT(*) FROM coffee)",
            [],
            |r| {
                Ok(BackupInfo {
                    roasts: r.get(0)?,
                    last_charge_at: r.get(1)?,
                    coffees: r.get(2)?,
                })
            },
        )
        .map_err(|_| not_a_backup())
    }

    /// Replaces everything with a backup's contents, brings its schema up to
    /// date and uses its machine. Check it with [`Store::inspect_backup`]
    /// first.
    ///
    /// # Errors
    ///
    /// Fails on a file that is not a Roastro backup, or a database error.
    pub fn restore(&mut self, path: &Path) -> Result<()> {
        Self::inspect_backup(path)?;
        self.conn.restore(
            rusqlite::MAIN_DB,
            path,
            None::<fn(rusqlite::backup::Progress)>,
        )?;
        migrate(&mut self.conn)?;
        self.machine_id = current_machine(&self.conn)?;
        Ok(())
    }
}

/// Runs every migration newer than the database's `user_version`.
fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let current = usize::try_from(current).map_err(|_| Error::Corrupt(current.to_string()))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        let version =
            i64::try_from(i + 1).map_err(|_| Error::Corrupt(format!("migration {}", i + 1)))?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

fn insert_sample(tx: &Transaction<'_>, roast: RoastId, s: &Sample) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO sample (roast_id, at, vals) VALUES (?1, ?2, ?3)",
        params![roast.to_string(), s.at, serde_json::to_string(&s.values)?],
    )?;
    Ok(())
}

fn insert_event(
    tx: &Transaction<'_>,
    roast: RoastId,
    kind: EventKind,
    at: Millis,
    source: Source,
    original_at: Option<Millis>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO roast_event (roast_id, kind, at, source, original_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![roast.to_string(), kind.as_str(), at, source.as_str(), original_at],
    )?;
    Ok(())
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| Error::Corrupt(s.to_owned()))
}

#[cfg(test)]
mod tests;
