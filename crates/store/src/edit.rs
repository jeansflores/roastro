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
    /// Free text.
    pub notes: String,
    /// Hidden from pickers; its roasts stay visible.
    pub archived: bool,
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
            "SELECT id, name, notes, archived FROM coffee WHERE ?1 OR archived = 0 ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([include_archived], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, notes, archived) = row?;
            out.push(Coffee {
                id: parse_uuid(&id)?,
                name,
                notes,
                archived,
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
                "SELECT name, notes, archived FROM coffee WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok(Coffee {
                        id,
                        name: r.get(0)?,
                        notes: r.get(1)?,
                        archived: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Renames and/or archives a coffee; `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn update_coffee(
        &mut self,
        id: Uuid,
        name: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Option<Coffee>> {
        if let Some(name) = name {
            self.conn.execute(
                "UPDATE coffee SET name = ?1 WHERE id = ?2",
                params![name, id.to_string()],
            )?;
        }
        if let Some(archived) = archived {
            self.conn.execute(
                "UPDATE coffee SET archived = ?1 WHERE id = ?2",
                params![archived, id.to_string()],
            )?;
        }
        self.coffee(id)
    }

    /// Roasts newest first, `limit` at a time, older than `before` if given;
    /// discarded ones only if asked.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub fn roasts(
        &self,
        before: Option<RoastId>,
        include_discarded: bool,
        limit: usize,
    ) -> Result<Vec<Roast>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT id FROM roast
             WHERE machine_id = ?1
               AND (?2 IS NULL OR number < (SELECT number FROM roast WHERE id = ?2))
               AND (?3 OR discarded_at IS NULL)
             ORDER BY number DESC LIMIT ?4",
        )?;
        let ids = stmt.query_map(
            params![
                self.machine_id.to_string(),
                before.map(|b| b.to_string()),
                include_discarded,
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
