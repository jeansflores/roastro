//! Corrections and edits made by people after (or during) a roast, the
//! coffees, and the roast list.

use recorder::Millis;
use roast::EventKind;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{Error, Note, Result, Roast, RoastId, Store, parse_uuid};

/// A partial update of a roast's fields: `None` leaves a field alone,
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoastPatch {
    /// The coffee roasted.
    pub coffee_id: Option<Option<Uuid>>,
    /// Green weight in grams.
    pub green_weight_g: Option<Option<i64>>,
    /// Roasted weight in grams.
    pub roasted_weight_g: Option<Option<i64>>,
    /// Who roasted it.
    pub roasted_by: Option<String>,
    /// The reference roast.
    pub reference_id: Option<Option<RoastId>>,
}

/// A green coffee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coffee {
    /// Id.
    pub id: Uuid,
    /// Name as the roaster calls it.
    pub name: String,
    /// Free text: how it roasts, what to watch.
    pub notes: String,
    /// The supplier's lot.
    pub lot: String,
    /// Country or region.
    pub origin: String,
    /// Washed, natural, honey…
    pub process: String,
    /// Who sold it.
    pub supplier: String,
    /// The lot is finished: hidden from pickers; its roasts stay visible.
    pub archived: bool,
}

/// What changes on a coffee: only the fields given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoffeeChange {
    /// New name.
    pub name: Option<String>,
    /// New notes.
    pub notes: Option<String>,
    /// New lot.
    pub lot: Option<String>,
    /// New origin.
    pub origin: Option<String>,
    /// New process.
    pub process: Option<String>,
    /// New supplier.
    pub supplier: Option<String>,
    /// Finished (or back in use).
    pub archived: Option<bool>,
}

/// How a coffee has roasted: its finished roasts, not discarded.
#[derive(Debug, Clone, PartialEq)]
pub struct CoffeeStats {
    /// Which coffee.
    pub coffee_id: Uuid,
    /// How many roasts.
    pub roasts: i64,
    /// The newest one's number.
    pub last_number: i64,
    /// When the newest one was charged.
    pub last_charge_at: Option<Millis>,
    /// Green coffee roasted, grams.
    pub green_weight_g: i64,
    /// Mean Development time ratio of the roasts with First crack and Drop.
    pub average_development_time_ratio: Option<f64>,
    /// Mean weight loss of the roasts with both weights.
    pub average_weight_loss_ratio: Option<f64>,
}

/// Which roasts a listing shows. The default lists every roast not discarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoastQuery {
    /// Only roasts older than this one (the next page).
    pub before: Option<RoastId>,
    /// Discarded roasts too.
    pub include_discarded: bool,
    /// Only this coffee's roasts.
    pub coffee_id: Option<Uuid>,
    /// Charged at or after this instant.
    pub since: Option<Millis>,
    /// Charged before this instant.
    pub until: Option<Millis>,
    /// The roast's number, or text in its notes or its coffee's name.
    pub text: Option<String>,
}

impl Store {
    fn ensure_roast(&self, id: RoastId) -> Result<()> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM roast WHERE id = ?1)",
            [id.to_string()],
            |r| r.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(Error::NoRoast(id))
        }
    }

    /// Applies a partial update to a roast's fields.
    ///
    /// # Errors
    ///
    /// Fails for an unknown roast, coffee or reference, a weight that is not
    /// positive, or a database error; nothing is written then.
    pub fn update_roast(&mut self, id: RoastId, patch: &RoastPatch) -> Result<()> {
        self.ensure_roast(id)?;
        let tx = self.conn.transaction()?;
        let key = id.to_string();
        if let Some(coffee) = patch.coffee_id {
            tx.execute(
                "UPDATE roast SET coffee_id = ?1 WHERE id = ?2",
                params![coffee.map(|c| c.to_string()), key],
            )?;
        }
        if let Some(green) = patch.green_weight_g {
            tx.execute(
                "UPDATE roast SET green_weight_g = ?1 WHERE id = ?2",
                params![green, key],
            )?;
        }
        if let Some(roasted) = patch.roasted_weight_g {
            tx.execute(
                "UPDATE roast SET roasted_weight_g = ?1 WHERE id = ?2",
                params![roasted, key],
            )?;
        }
        if let Some(by) = &patch.roasted_by {
            tx.execute(
                "UPDATE roast SET roasted_by = ?1 WHERE id = ?2",
                params![by, key],
            )?;
        }
        if let Some(reference) = patch.reference_id {
            tx.execute(
                "UPDATE roast SET reference_id = ?1 WHERE id = ?2",
                params![reference.map(|r| r.to_string()), key],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes an event; `false` if there was none of that kind.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn delete_event(&mut self, roast: RoastId, kind: EventKind) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM roast_event WHERE roast_id = ?1 AND kind = ?2",
            params![roast.to_string(), kind.as_str()],
        )?;
        Ok(n > 0)
    }

    /// The last reading at or before `at` and the first at or after it.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn sample_neighbors(
        &self,
        roast: RoastId,
        at: Millis,
    ) -> Result<(Option<Millis>, Option<Millis>)> {
        let key = roast.to_string();
        let before = self.conn.query_row(
            "SELECT MAX(at) FROM sample WHERE roast_id = ?1 AND at <= ?2",
            params![key, at],
            |r| r.get(0),
        )?;
        let after = self.conn.query_row(
            "SELECT MIN(at) FROM sample WHERE roast_id = ?1 AND at >= ?2",
            params![key, at],
            |r| r.get(0),
        )?;
        Ok((before, after))
    }

    /// Adds a note to a roast.
    ///
    /// # Errors
    ///
    /// Fails for an unknown roast or on a database error.
    pub fn add_note(&mut self, roast: RoastId, at: Millis, text: &str) -> Result<Note> {
        self.ensure_roast(roast)?;
        let id = Uuid::now_v7();
        self.conn.execute(
            "INSERT INTO roast_note (id, roast_id, at, text) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), roast.to_string(), at, text],
        )?;
        Ok(Note {
            id,
            at,
            text: text.to_owned(),
        })
    }

    /// Changes a note's instant or text; `None` if the roast has no such note.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn update_note(
        &mut self,
        roast: RoastId,
        note: Uuid,
        at: Option<Millis>,
        text: Option<&str>,
    ) -> Result<Option<Note>> {
        let (key, note_key) = (roast.to_string(), note.to_string());
        if let Some(at) = at {
            self.conn.execute(
                "UPDATE roast_note SET at = ?1 WHERE id = ?2 AND roast_id = ?3",
                params![at, note_key, key],
            )?;
        }
        if let Some(text) = text {
            self.conn.execute(
                "UPDATE roast_note SET text = ?1 WHERE id = ?2 AND roast_id = ?3",
                params![text, note_key, key],
            )?;
        }
        Ok(self
            .conn
            .query_row(
                "SELECT at, text FROM roast_note WHERE id = ?1 AND roast_id = ?2",
                params![note_key, key],
                |r| {
                    Ok(Note {
                        id: note,
                        at: r.get(0)?,
                        text: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Removes a note; `false` if the roast has no such note.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn delete_note(&mut self, roast: RoastId, note: Uuid) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM roast_note WHERE id = ?1 AND roast_id = ?2",
            params![note.to_string(), roast.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Discards a roast (`Some(when)`) or restores it (`None`). A discarded
    /// roast is hidden, never erased.
    ///
    /// # Errors
    ///
    /// Fails for an unknown roast or on a database error.
    pub fn set_discarded(&mut self, roast: RoastId, at: Option<Millis>) -> Result<()> {
        self.ensure_roast(roast)?;
        self.conn.execute(
            "UPDATE roast SET discarded_at = ?1 WHERE id = ?2",
            params![at, roast.to_string()],
        )?;
        Ok(())
    }

    /// The latest Done, not discarded roast of a coffee: the suggested
    /// reference when that coffee is picked.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn latest_done_roast(&self, coffee: Uuid) -> Result<Option<RoastId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM roast WHERE coffee_id = ?1 AND state = 'done' AND discarded_at IS NULL
                 ORDER BY number DESC LIMIT 1",
                [coffee.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|id| parse_uuid(&id).map(RoastId)).transpose()
    }

    /// Coffees by name; archived ones only if asked.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn coffees(&self, include_archived: bool) -> Result<Vec<Coffee>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, notes, lot, origin, process, supplier, archived FROM coffee
             WHERE ?1 OR archived = 0 ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([include_archived], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Coffee {
                    id: Uuid::nil(),
                    name: r.get(1)?,
                    notes: r.get(2)?,
                    lot: r.get(3)?,
                    origin: r.get(4)?,
                    process: r.get(5)?,
                    supplier: r.get(6)?,
                    archived: r.get(7)?,
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, coffee) = row?;
            out.push(Coffee {
                id: parse_uuid(&id)?,
                ..coffee
            });
        }
        Ok(out)
    }

    /// One coffee.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn coffee(&self, id: Uuid) -> Result<Option<Coffee>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name, notes, lot, origin, process, supplier, archived FROM coffee WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok(Coffee {
                        id,
                        name: r.get(0)?,
                        notes: r.get(1)?,
                        lot: r.get(2)?,
                        origin: r.get(3)?,
                        process: r.get(4)?,
                        supplier: r.get(5)?,
                        archived: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// Changes the fields given; `None` if the coffee does not exist.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn update_coffee(&mut self, id: Uuid, change: &CoffeeChange) -> Result<Option<Coffee>> {
        let texts = [
            ("name", &change.name),
            ("notes", &change.notes),
            ("lot", &change.lot),
            ("origin", &change.origin),
            ("process", &change.process),
            ("supplier", &change.supplier),
        ];
        let tx = self.conn.transaction()?;
        for (column, value) in texts {
            if let Some(value) = value {
                // The column names are this list's own, never input.
                tx.execute(
                    &format!("UPDATE coffee SET {column} = ?1 WHERE id = ?2"),
                    params![value, id.to_string()],
                )?;
            }
        }
        if let Some(archived) = change.archived {
            tx.execute(
                "UPDATE coffee SET archived = ?1 WHERE id = ?2",
                params![archived, id.to_string()],
            )?;
        }
        tx.commit()?;
        self.coffee(id)
    }

    /// Every coffee's figures over its finished roasts on this machine.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn coffee_stats(&self) -> Result<Vec<CoffeeStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.coffee_id, COUNT(*), MAX(r.number), MAX(c.at),
                    COALESCE(SUM(r.green_weight_g), 0),
                    AVG(CASE WHEN d.at > c.at AND f.at IS NOT NULL
                             THEN CAST(d.at - f.at AS REAL) / (d.at - c.at) END),
                    AVG(CASE WHEN r.roasted_weight_g IS NOT NULL AND r.green_weight_g IS NOT NULL
                             THEN 1.0 - CAST(r.roasted_weight_g AS REAL) / r.green_weight_g END)
             FROM roast r
             LEFT JOIN roast_event c ON c.roast_id = r.id AND c.kind = 'charge'
             LEFT JOIN roast_event f ON f.roast_id = r.id AND f.kind = 'first_crack'
             LEFT JOIN roast_event d ON d.roast_id = r.id AND d.kind = 'drop'
             WHERE r.machine_id = ?1 AND r.coffee_id IS NOT NULL
               AND r.discarded_at IS NULL AND r.state <> 'recording'
             GROUP BY r.coffee_id",
        )?;
        let rows = stmt.query_map([self.machine_id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                CoffeeStats {
                    coffee_id: Uuid::nil(),
                    roasts: r.get(1)?,
                    last_number: r.get(2)?,
                    last_charge_at: r.get(3)?,
                    green_weight_g: r.get(4)?,
                    average_development_time_ratio: r.get(5)?,
                    average_weight_loss_ratio: r.get(6)?,
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, stats) = row?;
            out.push(CoffeeStats {
                coffee_id: parse_uuid(&id)?,
                ..stats
            });
        }
        Ok(out)
    }

    /// Roasts newest first, `limit` at a time, as the query narrows them.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn roasts(&self, query: &RoastQuery, limit: usize) -> Result<Vec<Roast>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let text = query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.trim_start_matches('#').to_owned());
        let like = text.as_deref().map(|t| {
            let escaped = t
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let mut stmt = self.conn.prepare(
            "SELECT r.id FROM roast r
             LEFT JOIN roast_event c ON c.roast_id = r.id AND c.kind = 'charge'
             WHERE r.machine_id = ?1
               AND (?2 IS NULL OR r.number < (SELECT number FROM roast WHERE id = ?2))
               AND (?3 OR r.discarded_at IS NULL)
               AND (?4 IS NULL OR r.coffee_id = ?4)
               AND (?5 IS NULL OR c.at >= ?5)
               AND (?6 IS NULL OR c.at < ?6)
               AND (?7 IS NULL
                    OR CAST(r.number AS TEXT) = ?8
                    OR EXISTS (SELECT 1 FROM roast_note n
                               WHERE n.roast_id = r.id AND n.text LIKE ?7 ESCAPE '\\')
                    OR EXISTS (SELECT 1 FROM coffee k
                               WHERE k.id = r.coffee_id AND k.name LIKE ?7 ESCAPE '\\'))
             ORDER BY r.number DESC LIMIT ?9",
        )?;
        let ids = stmt.query_map(
            params![
                self.machine_id.to_string(),
                query.before.map(|b| b.to_string()),
                query.include_discarded,
                query.coffee_id.map(|c| c.to_string()),
                query.since,
                query.until,
                like,
                text,
                limit
            ],
            |r| r.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for id in ids {
            out.push(self.roast(RoastId(parse_uuid(&id?)?))?);
        }
        Ok(out)
    }
}
