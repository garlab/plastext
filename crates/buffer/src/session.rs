//! One editing owner per document, with pane-local byte anchors. The UI can
//! own this model in one entity; background tasks receive immutable snapshots.
use std::{collections::BTreeMap, ops::Range};

use crate::{
    Bias, Direction, EditBuffer, GraphemeSearch, TextError, TextFormat, TextSnapshot, UndoGroup,
    edit::EditDelta,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PaneId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub offset: u64,
    pub bias: Bias,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Anchor,
    pub head: Anchor,
}

impl Selection {
    pub fn caret(offset: u64) -> Self {
        Self {
            anchor: Anchor {
                offset,
                bias: Bias::After,
            },
            head: Anchor {
                offset,
                bias: Bias::After,
            },
        }
    }

    pub fn new(anchor: u64, head: u64) -> Self {
        if anchor == head {
            return Self::caret(head);
        }
        Self {
            anchor: Anchor {
                offset: anchor,
                bias: if anchor < head {
                    Bias::Before
                } else {
                    Bias::After
                },
            },
            head: Anchor {
                offset: head,
                bias: if anchor < head {
                    Bias::After
                } else {
                    Bias::Before
                },
            },
        }
    }

    pub fn range(self) -> Range<u64> {
        self.anchor.offset.min(self.head.offset)..self.anchor.offset.max(self.head.offset)
    }

    fn rebase(&mut self, edit: &EditDelta) {
        for anchor in [&mut self.anchor, &mut self.head] {
            anchor.offset = if anchor.offset < edit.range.start {
                anchor.offset
            } else if anchor.offset >= edit.range.end && !edit.range.is_empty() {
                edit.range.start + edit.inserted + (anchor.offset - edit.range.end)
            } else if anchor.offset > edit.range.end {
                anchor.offset + edit.inserted
            } else {
                edit.range.start
                    + if anchor.bias == Bias::After {
                        edit.inserted
                    } else {
                        0
                    }
            };
        }
    }

    fn resolved(mut self, snapshot: &TextSnapshot) -> Result<Self, TextError> {
        for anchor in [&mut self.anchor, &mut self.head] {
            anchor.offset = snapshot.clip_to_scalar(anchor.offset, anchor.bias)?;
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct DocumentSnapshot {
    text: TextSnapshot,
    generation: u64,
}

impl DocumentSnapshot {
    pub fn text(&self) -> &TextSnapshot {
        &self.text
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone)]
pub struct PaneSnapshot {
    document: DocumentSnapshot,
    id: PaneId,
    epoch: u64,
    selection: Selection,
}

impl PaneSnapshot {
    pub fn document(&self) -> &DocumentSnapshot {
        &self.document
    }
    pub fn selection(&self) -> Selection {
        self.selection
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    UnknownPane,
    Stale,
    Pending,
    Text(TextError),
}

impl From<TextError> for SessionError {
    fn from(error: TextError) -> Self {
        Self::Text(error)
    }
}

struct Pane {
    selection: Selection,
    epoch: u64,
}

pub struct DocumentSession {
    buffer: EditBuffer,
    generation: u64,
    panes: BTreeMap<PaneId, Pane>,
    next_pane: u64,
    last_actor: Option<PaneId>,
}

impl DocumentSession {
    pub fn new(buffer: EditBuffer) -> Self {
        Self {
            buffer,
            generation: 0,
            panes: BTreeMap::new(),
            next_pane: 0,
            last_actor: None,
        }
    }

    /// The UI must drain retired versions after each mutation and drop them on a worker.
    pub fn defer_history_drop(&mut self) {
        self.buffer.defer_history_drop();
    }
    pub fn take_retired_history(&mut self) -> Option<crate::RetiredHistory> {
        self.buffer.take_retired_history()
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            text: self.buffer.snapshot(),
            generation: self.generation,
        }
    }

    /// Includes a monotonically increasing session generation: editing and
    /// undoing back to the same tree must not revive an obsolete job.
    pub fn is_current(&self, snapshot: &DocumentSnapshot) -> bool {
        snapshot.generation == self.generation
            && snapshot.text.same_version(&self.buffer.snapshot())
    }

    pub fn attach_pane(&mut self, selection: Selection) -> Result<PaneId, SessionError> {
        self.validate_selection(selection)?;
        let id = PaneId(self.next_pane);
        self.next_pane = self
            .next_pane
            .checked_add(1)
            .expect("pane identifiers exhausted");
        self.panes.insert(
            id,
            Pane {
                selection,
                epoch: 0,
            },
        );
        Ok(id)
    }

    pub fn detach_pane(&mut self, id: PaneId) -> bool {
        if self.last_actor == Some(id) {
            self.last_actor = None;
        }
        self.panes.remove(&id).is_some()
    }

    pub fn pane_snapshot(&self, id: PaneId) -> Result<PaneSnapshot, SessionError> {
        let pane = self.panes.get(&id).ok_or(SessionError::UnknownPane)?;
        let document = self.snapshot();
        Ok(PaneSnapshot {
            selection: pane.selection.resolved(&document.text)?,
            document,
            id,
            epoch: pane.epoch,
        })
    }

    pub fn set_selection(
        &mut self,
        base: &PaneSnapshot,
        selection: Selection,
    ) -> Result<(), SessionError> {
        self.validate(base)?;
        self.validate_selection(selection)?;
        self.update_selection(base.id, selection);
        self.last_actor = None;
        self.buffer.break_undo_group();
        Ok(())
    }

    /// Place the IME selection inside an edit without splitting its undo group.
    pub fn set_composition_selection(
        &mut self,
        base: &PaneSnapshot,
        selection: Selection,
    ) -> Result<(), SessionError> {
        self.validate(base)?;
        self.validate_selection(selection)?;
        self.update_selection(base.id, selection);
        Ok(())
    }

    pub fn replace(
        &mut self,
        base: &PaneSnapshot,
        range: Range<u64>,
        text: &str,
        group: UndoGroup,
    ) -> Result<(), SessionError> {
        self.validate(base)?;
        let group = if self.last_actor == Some(base.id) {
            group
        } else {
            UndoGroup::New
        };
        self.buffer.replace_text(range.clone(), text, group)?;
        if range.is_empty() && text.is_empty() {
            return Ok(());
        }
        let edit = EditDelta {
            range,
            inserted: text.len() as u64,
        };
        rebase_panes(&mut self.panes, &edit);
        self.update_selection(base.id, Selection::caret(edit.range.start + edit.inserted));
        self.last_actor = Some(base.id);
        self.changed();
        Ok(())
    }

    pub fn insert_newline(&mut self, base: &PaneSnapshot) -> Result<(), SessionError> {
        self.validate(base)?;
        let ending = base.document.text.preferred_line_ending().as_str();
        self.replace(base, base.selection.range(), ending, UndoGroup::New)
    }

    /// Output settings are shared by all panes. Stale menus/jobs cannot change
    /// a newer document; undo/redo restores settings without moving selections.
    pub fn set_format(
        &mut self,
        base: &DocumentSnapshot,
        format: TextFormat,
    ) -> Result<bool, SessionError> {
        if !self.is_current(base) {
            return Err(SessionError::Stale);
        }
        let changed = self.buffer.set_format(format);
        if changed {
            self.changed();
            self.last_actor = None;
        }
        Ok(changed)
    }

    pub fn undo(&mut self, base: &PaneSnapshot) -> Result<bool, SessionError> {
        self.validate(base)?;
        let panes = &mut self.panes;
        let changed = self.buffer.undo_with(|edits| {
            for edit in edits.iter().rev() {
                rebase_panes(panes, &edit.inverse());
            }
        });
        if changed {
            self.changed();
        }
        self.last_actor = None;
        Ok(changed)
    }

    pub fn redo(&mut self, base: &PaneSnapshot) -> Result<bool, SessionError> {
        self.validate(base)?;
        let panes = &mut self.panes;
        let changed = self.buffer.redo_with(|edits| {
            for edit in edits {
                rebase_panes(panes, edit);
            }
        });
        if changed {
            self.changed();
        }
        self.last_actor = None;
        Ok(changed)
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }
    pub fn can_undo(&self) -> bool {
        self.buffer.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.buffer.can_redo()
    }
    pub fn set_history_budget(&mut self, bytes: u64) {
        self.buffer.set_history_budget(bytes);
    }

    pub fn mark_saved(&mut self, saved: &DocumentSnapshot) -> bool {
        let accepted = self.buffer.mark_saved(&saved.text);
        if accepted {
            self.last_actor = None;
        }
        accepted
    }

    /// Install an external version only while the originating version is still
    /// current. Dirty content needs an explicit discard. Retire the old buffer
    /// off the UI thread.
    pub(super) fn reload(
        &mut self,
        base: &DocumentSnapshot,
        buffer: EditBuffer,
        discard_edits: bool,
    ) -> Result<EditBuffer, Box<EditBuffer>> {
        if (self.is_dirty() && !discard_edits) || !self.is_current(base) {
            return Err(Box::new(buffer));
        }
        let len = crate::ByteSource::len(&buffer.snapshot());
        let retired = std::mem::replace(&mut self.buffer, buffer);
        for pane in self.panes.values_mut() {
            pane.selection.anchor.offset = pane.selection.anchor.offset.min(len);
            pane.selection.head.offset = pane.selection.head.offset.min(len);
            if pane.selection.anchor.offset == pane.selection.head.offset {
                pane.selection = Selection::caret(pane.selection.head.offset);
            }
        }
        self.last_actor = None;
        self.changed();
        Ok(retired)
    }

    fn validate(&self, base: &PaneSnapshot) -> Result<(), SessionError> {
        let pane = self.panes.get(&base.id).ok_or(SessionError::UnknownPane)?;
        if !self.is_current(&base.document) || pane.epoch != base.epoch {
            return Err(SessionError::Stale);
        }
        Ok(())
    }

    fn validate_selection(&self, selection: Selection) -> Result<(), SessionError> {
        let snapshot = self.buffer.snapshot();
        snapshot.require_scalar_boundary(selection.anchor.offset)?;
        snapshot.require_scalar_boundary(selection.head.offset)?;
        Ok(())
    }

    fn update_selection(&mut self, id: PaneId, selection: Selection) {
        let pane = self.panes.get_mut(&id).unwrap();
        pane.selection = selection;
        pane.epoch = pane
            .epoch
            .checked_add(1)
            .expect("pane generation exhausted");
    }

    fn changed(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("document generation exhausted");
    }
}

fn rebase_panes(panes: &mut BTreeMap<PaneId, Pane>, edit: &EditDelta) {
    for pane in panes.values_mut() {
        pane.selection.rebase(edit);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CursorCommand {
    Move { direction: Direction, extend: bool },
    Delete { direction: Direction },
}

/// Holds at most one bounded grapheme search and the originating pane stamp.
/// Dropping it cancels the operation; polling never changes document content.
pub struct PendingCommand {
    base: PaneSnapshot,
    command: CursorCommand,
    search: Option<GraphemeSearch>,
    target: Option<u64>,
}

impl PendingCommand {
    pub fn poll(&mut self, chunks: usize) -> Result<bool, SessionError> {
        if self.target.is_none() {
            self.target = self.search.as_mut().unwrap().poll(chunks)?;
        }
        Ok(self.target.is_some())
    }
    pub fn snapshot(&self) -> &TextSnapshot {
        &self.base.document.text
    }
}

impl DocumentSession {
    pub fn cursor_command(
        &self,
        base: &PaneSnapshot,
        command: CursorCommand,
    ) -> Result<PendingCommand, SessionError> {
        self.validate(base)?;
        let direction = match command {
            CursorCommand::Move { direction, .. } | CursorCommand::Delete { direction } => {
                direction
            }
        };
        let range = base.selection.range();
        let collapse =
            !range.is_empty() && !matches!(command, CursorCommand::Move { extend: true, .. });
        let target = collapse.then_some(if direction == Direction::Backward {
            range.start
        } else {
            range.end
        });
        let search = if collapse {
            None
        } else {
            Some(
                base.document
                    .text
                    .grapheme_search(base.selection.head.offset, direction)?,
            )
        };
        Ok(PendingCommand {
            base: base.clone(),
            command,
            target,
            search,
        })
    }

    /// May be retried after Missing. Selection/content changes, pane closure or
    /// an edit/undo cycle invalidate the command rather than applying old offsets.
    pub fn finish_command(&mut self, command: &PendingCommand) -> Result<(), SessionError> {
        self.validate(&command.base)?;
        let target = command.target.ok_or(SessionError::Pending)?;
        match command.command {
            CursorCommand::Move { extend, .. } => {
                let anchor = if extend {
                    command.base.selection.anchor.offset
                } else {
                    target
                };
                self.update_selection(command.base.id, Selection::new(anchor, target));
                self.last_actor = None;
                self.buffer.break_undo_group();
                Ok(())
            }
            CursorCommand::Delete { .. } => {
                let range = command.base.selection.range();
                let range = if range.is_empty() {
                    range.start.min(target)..range.start.max(target)
                } else {
                    range
                };
                self.replace(&command.base, range, "", UndoGroup::New)
            }
        }
    }
}

#[cfg(test)]
mod tests;
