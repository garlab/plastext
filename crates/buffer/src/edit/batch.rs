//! Atomic multi-range transactions. Preparation can run on a worker; installing
//! a prepared tree never rereads or copies the document.
use super::*;
use crate::TextError;

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub range: Range<u64>,
    pub text: String,
}

pub struct PreparedEdits {
    before: TextSnapshot,
    after: TextSnapshot,
    charge: u64,
    pub(crate) edits: Vec<EditDelta>,
    pub(crate) selections: Option<Vec<crate::Selection>>,
}

impl TextSnapshot {
    /// Edits use this snapshot's coordinates, must not overlap, and must end at
    /// scalar boundaries. Missing/error leaves the live document untouched.
    pub fn prepare_edits(&self, edits: &[TextEdit]) -> Result<PreparedEdits, TextError> {
        self.prepare_impl(edits, None)
    }

    /// Worker-only variant: loads each missing boundary before continuing, without
    /// restarting the batch or requiring every edit to fit in the viewport cache.
    pub fn prepare_edits_loading(
        &self,
        edits: &[TextEdit],
        cancel: &AtomicBool,
    ) -> Result<PreparedEdits, TextError> {
        self.prepare_impl(edits, Some(cancel))
    }

    fn prepare_impl(
        &self,
        edits: &[TextEdit],
        cancel: Option<&AtomicBool>,
    ) -> Result<PreparedEdits, TextError> {
        let mut ordered: Vec<_> = edits.iter().collect();
        ordered.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut previous = None;
        for edit in &ordered {
            if edit.range.start > edit.range.end
                || previous.is_some_and(|end| edit.range.start < end)
            {
                return Err(TextError::Edit(EditError::InvalidRange));
            }
            loaded(self, cancel, || {
                self.require_scalar_boundary(edit.range.start)
            })?;
            loaded(self, cancel, || {
                self.require_scalar_boundary(edit.range.end)
            })?;
            previous = Some(edit.range.end);
        }
        let mut prepared = PreparedEdits {
            before: self.clone(),
            after: self.clone(),
            charge: 0,
            edits: Vec::new(),
            selections: None,
        };
        for edit in ordered.into_iter().rev() {
            if edit.range.is_empty() && edit.text.is_empty() {
                continue;
            }
            let (after, charge, delta) = loaded(&prepared.after, cancel, || {
                replace_snapshot(&prepared.after, edit.range.clone(), edit.text.as_bytes()).map_err(
                    |error| match error {
                        EditError::Read(read) => TextError::Read(read),
                        other => TextError::Edit(other),
                    },
                )
            })?;
            prepared.after = after;
            prepared.charge = prepared.charge.saturating_add(charge);
            prepared.edits.push(delta);
        }
        Ok(prepared)
    }
}

impl EditBuffer {
    pub(crate) fn retire_prepared(&mut self, prepared: PreparedEdits) {
        if let Some(retired) = &mut self.retired {
            retired.push(Change {
                before: prepared.before,
                after: prepared.after,
                charge: prepared.charge,
                edits: prepared.edits,
            });
        }
    }
    pub(crate) fn apply_prepared(&mut self, mut prepared: PreparedEdits, group: UndoGroup) -> bool {
        if !self.current.same_version(&prepared.before) {
            self.retire_prepared(prepared);
            return false;
        }
        if prepared.edits.is_empty() {
            return true;
        }
        prepared.after.revision = self.next_revision;
        self.record(
            prepared.before,
            prepared.after,
            prepared.charge,
            prepared.edits,
            group,
        );
        true
    }
}

fn loaded<T>(
    text: &TextSnapshot,
    cancel: Option<&AtomicBool>,
    mut f: impl FnMut() -> Result<T, TextError>,
) -> Result<T, TextError> {
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(TextError::Read(ReadError::Changed));
        }
        match f() {
            Err(TextError::Read(ReadError::Missing(range))) if cancel.is_some() => text
                .load(range)
                .map_err(|_| TextError::Read(ReadError::Changed))?,
            result => return result,
        }
    }
}

impl PreparedEdits {
    /// Validate post-edit selections on the prepared version before the atomic install.
    pub fn with_selections(
        self,
        selections: Vec<crate::Selection>,
        cancel: &AtomicBool,
    ) -> Result<Self, crate::SessionError> {
        self.select(selections, Some(cancel))
    }
    pub fn with_cached_selections(
        self,
        selections: Vec<crate::Selection>,
    ) -> Result<Self, crate::SessionError> {
        self.select(selections, None)
    }
    fn select(
        mut self,
        selections: Vec<crate::Selection>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, crate::SessionError> {
        let selections = crate::session::normalize_selections(selections)?;
        for selection in &selections {
            loaded(&self.after, cancel, || {
                self.after.require_scalar_boundary(selection.anchor.offset)
            })?;
            loaded(&self.after, cancel, || {
                self.after.require_scalar_boundary(selection.head.offset)
            })?;
        }
        self.selections = Some(selections);
        Ok(self)
    }
}
