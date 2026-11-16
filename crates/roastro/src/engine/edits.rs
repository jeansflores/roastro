//! What people change through the API, with the rules from "Contract between
//! the backend and the web UI" and "Recording through a PLC dropout or an app
//! restart": event times never in the future, on recorded samples (only Drop
//! may fall inside a Gap) and in roast order; weights positive; a reference
//! is another Done roast.

use recorder::{Input, Millis};
use roast::{EventKind, Source};
use store::{RoastId, RoastPatch, RoastState};
use uuid::Uuid;

use super::{Edit, Engine};
use crate::api::ApiError;
use crate::wire::{
    CoffeeIn, CoffeeOut, CoffeePatch, Live, NextRoastIn, NextRoastOut, NoteIn, NoteOut, NotePatch,
    RoastDoc, RoastPage, RoastPatchIn,
};

/// Roasts per page of `GET /api/roasts`.
const PAGE: usize = 50;

impl Engine {
    pub(super) fn edit(&mut self, edit: Edit) {
        match edit {
            Edit::SetEvent(id, kind, body, reply) => {
                let _ = reply.send(self.set_event(id, &kind, body.at));
            }
            Edit::DeleteEvent(id, kind, reply) => {
                let _ = reply.send(self.delete_event(id, &kind));
            }
            Edit::AddNote(id, body, reply) => {
                let _ = reply.send(self.add_note(id, &body));
            }
            Edit::PatchNote(id, note, body, reply) => {
                let _ = reply.send(self.patch_note(id, note, &body));
            }
            Edit::DeleteNote(id, note, reply) => {
                let _ = reply.send(self.delete_note(id, note));
            }
            Edit::PatchRoast(id, body, reply) => {
                let _ = reply.send(self.patch_roast(id, body));
            }
            Edit::Discard(id, reply) => {
                let _ = reply.send(self.discard(id, true));
            }
            Edit::Restore(id, reply) => {
                let _ = reply.send(self.discard(id, false));
            }
            Edit::Close(id, reply) => {
                let _ = reply.send(self.close(id));
            }
            Edit::GetNextRoast(reply) => {
                let _ = reply.send(self.next_roast());
            }
            Edit::PutNextRoast(body, reply) => {
                let _ = reply.send(self.put_next_roast(&body));
            }
            Edit::Coffees(archived, reply) => {
                let coffees = self
                    .recording
                    .store()
                    .coffees(archived)
                    .map_err(ApiError::internal);
                let _ = reply.send(coffees.map(|cs| cs.into_iter().map(CoffeeOut::from).collect()));
            }
            Edit::AddCoffee(body, reply) => {
                let _ = reply.send(self.add_coffee(&body));
            }
            Edit::PatchCoffee(id, body, reply) => {
                let _ = reply.send(self.patch_coffee(id, &body));
            }
            Edit::Roasts(before, discarded, reply) => {
                let _ = reply.send(self.roast_page(before, discarded));
            }
        }
    }

    /// A roast that exists, as stored.
    fn stored(&self, id: RoastId) -> Result<store::Roast, ApiError> {
        self.recording.store().roast(id).map_err(|e| match e {
            store::Error::NoRoast(_) => {
                ApiError::not_found("roast_not_found", format!("no roast {id}"))
            }
            other => ApiError::internal(other),
        })
    }

    /// Tells the live screens when the roast on screen changed.
    fn changed(&self, id: RoastId) -> Result<RoastDoc, ApiError> {
        let doc = self.doc(id)?;
        if self.shown == Some(id) {
            self.publish(Live::Roast(self.doc(id)?));
        }
        Ok(doc)
    }

    fn set_event(&mut self, id: RoastId, kind: &str, at: Millis) -> Result<RoastDoc, ApiError> {
        let kind = parse_kind(kind)?;
        let roast = self.stored(id)?;
        self.check_time(id, Some(kind), at)?;
        let out_of_order = roast
            .events
            .iter()
            .filter(|e| e.kind != kind)
            .find(|e| (e.kind < kind && e.at >= at) || (e.kind > kind && e.at <= at));
        if let Some(other) = out_of_order {
            return Err(ApiError::unprocessable(
                "event_out_of_order",
                format!(
                    "{kind} must come {} {}; move that one first",
                    if other.kind < kind { "after" } else { "before" },
                    other.kind
                ),
            ));
        }
        let receiving = self.recording.current() == Some(id);
        if kind == EventKind::Drop && receiving && self.recording.is_before_drop() {
            // The recorder must know, so the tail starts from this Drop.
            self.apply(Input::DropPressed(at))?;
            return self.doc(id);
        }
        self.recording
            .store_mut()
            .set_event(id, kind, at, Source::Manual)
            .map_err(ApiError::internal)?;
        self.changed(id)
    }

    fn delete_event(&mut self, id: RoastId, kind: &str) -> Result<RoastDoc, ApiError> {
        let kind = parse_kind(kind)?;
        self.stored(id)?;
        if !kind.is_manual_only() {
            return Err(ApiError::unprocessable(
                "event_not_deletable",
                format!("{kind} can only be moved, not removed"),
            ));
        }
        self.recording
            .store_mut()
            .delete_event(id, kind)
            .map_err(ApiError::internal)?;
        self.changed(id)
    }

    /// The contract's time rules for events (`kind` given) and notes.
    fn check_time(&self, id: RoastId, kind: Option<EventKind>, at: Millis) -> Result<(), ApiError> {
        if at > (self.now)() {
            return Err(ApiError::unprocessable(
                "event_in_future",
                "that instant has not happened yet",
            ));
        }
        let (before, after) = self
            .recording
            .store()
            .sample_neighbors(id, at)
            .map_err(ApiError::internal)?;
        let gap = Millis::try_from(self.ror.gap.as_millis()).unwrap_or(Millis::MAX);
        let ok = match (before, after) {
            (Some(b), Some(a)) => a - b <= gap || kind == Some(EventKind::Drop), // only Drop may sit inside a Gap
            // Just after the last reading of a roast still recording: "now".
            (Some(b), None) => at - b <= gap && self.recording.current() == Some(id),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(ApiError::unprocessable(
                "event_outside_samples",
                "that instant is outside the roast's recorded readings",
            ))
        }
    }

    fn add_note(&mut self, id: RoastId, body: &NoteIn) -> Result<NoteOut, ApiError> {
        self.stored(id)?;
        let text = non_empty(&body.text, "empty_note", "a note needs some text")?;
        self.check_time(id, None, body.at)?;
        let note = self
            .recording
            .store_mut()
            .add_note(id, body.at, text)
            .map_err(ApiError::internal)?;
        self.changed(id)?;
        Ok(NoteOut {
            id: note.id,
            at: note.at,
            text: note.text,
        })
    }

    fn patch_note(
        &mut self,
        id: RoastId,
        note: Uuid,
        body: &NotePatch,
    ) -> Result<NoteOut, ApiError> {
        self.stored(id)?;
        let text = body
            .text
            .as_deref()
            .map(|t| non_empty(t, "empty_note", "a note needs some text"))
            .transpose()?;
        if let Some(at) = body.at {
            self.check_time(id, None, at)?;
        }
        let note = self
            .recording
            .store_mut()
            .update_note(id, note, body.at, text)
            .map_err(ApiError::internal)?
            .ok_or_else(|| {
                ApiError::not_found("note_not_found", format!("no note {note} on roast {id}"))
            })?;
        self.changed(id)?;
        Ok(NoteOut {
            id: note.id,
            at: note.at,
            text: note.text,
        })
    }

    fn delete_note(&mut self, id: RoastId, note: Uuid) -> Result<(), ApiError> {
        self.stored(id)?;
        if !self
            .recording
            .store_mut()
            .delete_note(id, note)
            .map_err(ApiError::internal)?
        {
            return Err(ApiError::not_found(
                "note_not_found",
                format!("no note {note} on roast {id}"),
            ));
        }
        self.changed(id).map(|_| ())
    }

    fn patch_roast(&mut self, id: RoastId, body: RoastPatchIn) -> Result<RoastDoc, ApiError> {
        self.stored(id)?;
        for weight in [body.green_weight_g, body.roasted_weight_g]
            .into_iter()
            .flatten()
            .flatten()
        {
            positive(weight)?;
        }
        if let Some(Some(coffee)) = body.coffee_id {
            self.known_coffee(coffee)?;
        }
        if let Some(Some(reference)) = body.reference_id {
            self.valid_reference(RoastId(reference), Some(id))?;
        }
        let patch = RoastPatch {
            coffee_id: body.coffee_id,
            green_weight_g: body.green_weight_g,
            roasted_weight_g: body.roasted_weight_g,
            roasted_by: body.roasted_by,
            reference_id: body.reference_id.map(|r| r.map(RoastId)),
        };
        self.recording
            .store_mut()
            .update_roast(id, &patch)
            .map_err(ApiError::internal)?;
        self.changed(id)
    }

    fn discard(&mut self, id: RoastId, discard: bool) -> Result<RoastDoc, ApiError> {
        self.stored(id)?;
        if discard && self.recording.current() == Some(id) {
            if self.recording.is_before_drop() {
                return Err(ApiError::conflict(
                    "roast_recording",
                    "close or drop the roast before discarding it",
                ));
            }
            self.apply(Input::ClosePressed)?; // cut the tail short
        }
        let at = discard.then(|| (self.now)());
        self.recording
            .store_mut()
            .set_discarded(id, at)
            .map_err(ApiError::internal)?;
        let doc = self.changed(id)?;
        if discard && self.shown == Some(id) {
            self.shown = None; // the live screen goes back to waiting for Charge
        }
        Ok(doc)
    }

    fn close(&mut self, id: RoastId) -> Result<RoastDoc, ApiError> {
        let roast = self.stored(id)?;
        if roast.state != RoastState::Recording
            || self.recording.current() != Some(id)
            || !self.recording.is_before_drop()
        {
            return Err(ApiError::conflict(
                "roast_not_recording",
                "only a roast between Charge and Drop can be closed",
            ));
        }
        self.apply(Input::ClosePressed)?;
        self.doc(id)
    }

    fn next_roast(&self) -> Result<NextRoastOut, ApiError> {
        NextRoastOut::load(self.recording.store()).map_err(ApiError::internal)
    }

    fn put_next_roast(&mut self, body: &NextRoastIn) -> Result<NextRoastOut, ApiError> {
        if let Some(weight) = body.green_weight_g {
            positive(weight)?;
        }
        if let Some(coffee) = body.coffee_id {
            self.known_coffee(coffee)?;
        }
        if let Some(reference) = body.reference_id {
            self.valid_reference(RoastId(reference), None)?;
        }
        let next = store::NextRoast {
            coffee_id: body.coffee_id,
            green_weight_g: body.green_weight_g,
            roasted_by: body.roasted_by.trim().to_owned(),
            reference_id: body.reference_id.map(RoastId),
        };
        self.recording
            .store_mut()
            .set_next_roast(&next)
            .map_err(ApiError::internal)?;
        let out = self.next_roast()?;
        self.publish(Live::NextRoast(self.next_roast()?));
        Ok(out)
    }

    fn add_coffee(&mut self, body: &CoffeeIn) -> Result<CoffeeOut, ApiError> {
        let name = non_empty(&body.name, "empty_name", "a coffee needs a name")?;
        let id = self
            .recording
            .store_mut()
            .add_coffee(name)
            .map_err(ApiError::internal)?;
        self.coffee_out(id)
    }

    fn patch_coffee(&mut self, id: Uuid, body: &CoffeePatch) -> Result<CoffeeOut, ApiError> {
        let name = body
            .name
            .as_deref()
            .map(|n| non_empty(n, "empty_name", "a coffee needs a name"))
            .transpose()?;
        self.recording
            .store_mut()
            .update_coffee(id, name, body.archived)
            .map_err(ApiError::internal)?
            .map(CoffeeOut::from)
            .ok_or_else(|| ApiError::not_found("coffee_not_found", format!("no coffee {id}")))
    }

    fn coffee_out(&self, id: Uuid) -> Result<CoffeeOut, ApiError> {
        self.recording
            .store()
            .coffee(id)
            .map_err(ApiError::internal)?
            .map(CoffeeOut::from)
            .ok_or_else(|| ApiError::not_found("coffee_not_found", format!("no coffee {id}")))
    }

    fn known_coffee(&self, id: Uuid) -> Result<(), ApiError> {
        match self
            .recording
            .store()
            .coffee(id)
            .map_err(ApiError::internal)?
        {
            Some(_) => Ok(()),
            None => Err(ApiError::unprocessable(
                "coffee_not_found",
                format!("no coffee {id}"),
            )),
        }
    }

    /// A reference must be another roast, Done and not discarded.
    fn valid_reference(&self, reference: RoastId, of: Option<RoastId>) -> Result<(), ApiError> {
        let invalid = |why: &str| Err(ApiError::unprocessable("invalid_reference", why.to_owned()));
        if of == Some(reference) {
            return invalid("a roast cannot be its own reference");
        }
        match self.recording.store().roast(reference) {
            Ok(r) if r.state == RoastState::Done && r.discarded_at.is_none() => Ok(()),
            Ok(_) => invalid("the reference must be a Done roast that is not discarded"),
            Err(store::Error::NoRoast(_)) => invalid("no such roast"),
            Err(other) => Err(ApiError::internal(other)),
        }
    }

    fn roast_page(&self, before: Option<Uuid>, discarded: bool) -> Result<RoastPage, ApiError> {
        let roasts = self
            .recording
            .store()
            .roasts(before.map(RoastId), discarded, PAGE)
            .map_err(ApiError::internal)?;
        let next_before = (roasts.len() == PAGE)
            .then(|| roasts.last().map(|r| r.id.0))
            .flatten();
        let roasts = roasts
            .into_iter()
            .map(|r| self.doc(r.id))
            .collect::<Result<_, _>>()?;
        Ok(RoastPage {
            roasts,
            next_before,
        })
    }
}

fn parse_kind(kind: &str) -> Result<EventKind, ApiError> {
    kind.parse()
        .map_err(|_| ApiError::invalid("unknown_event_kind", format!("no event kind {kind:?}")))
}

fn non_empty<'a>(text: &'a str, code: &'static str, message: &str) -> Result<&'a str, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        Err(ApiError::invalid(code, message.to_owned()))
    } else {
        Ok(text)
    }
}

fn positive(weight_g: i64) -> Result<(), ApiError> {
    if weight_g > 0 {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "invalid_weight",
            "weights are positive grams",
        ))
    }
}
