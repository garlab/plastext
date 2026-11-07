//! One editing owner per document, with pane-local byte anchors. The UI can
//! own this model in one entity; background tasks receive immutable snapshots.
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    ops::Range,
};

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

    pub fn rebase(&mut self, edit: &EditDelta) {
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
    additional: Vec<Selection>,
}

impl PaneSnapshot {
    pub fn document(&self) -> &DocumentSnapshot {
        &self.document
    }
    pub fn selection(&self) -> Selection {
        self.selection
    }
    pub fn selections(&self) -> Vec<Selection> {
        std::iter::once(self.selection)
            .chain(self.additional.iter().copied())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    UnknownPane,
    Stale,
    Pending,
    InvalidSelections,
    Limit(&'static str),
    Text(TextError),
}

impl From<TextError> for SessionError {
    fn from(error: TextError) -> Self {
        Self::Text(error)
    }
}

struct Pane {
    viewport: u64,
    resolved: RefCell<Vec<Selection>>,
    selection: Selection,
    additional: Vec<Selection>,
    epoch: u64,
}

pub struct DocumentSession {
    buffer: EditBuffer,
    generation: u64,
    panes: BTreeMap<PaneId, Pane>,
    next_pane: u64,
    last_actor: Option<PaneId>,
    changes: VecDeque<(u64, Vec<EditDelta>)>,
}

impl DocumentSession {
    pub fn new(buffer: EditBuffer) -> Self {
        Self {
            buffer,
            generation: 0,
            panes: BTreeMap::new(),
            next_pane: 0,
            last_actor: None,
            changes: VecDeque::new(),
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

    /// Bounded coordinate changes for view caches. None means the caller must
    /// discard its cache (reload, or more than 256 unobserved changes).
    pub fn edits_since(&self, generation: u64) -> Option<Vec<EditDelta>> {
        if generation == self.generation {
            return Some(Vec::new());
        }
        if generation > self.generation
            || self
                .changes
                .front()
                .is_none_or(|(first, _)| generation + 1 < *first)
        {
            return None;
        }
        Some(
            self.changes
                .iter()
                .filter(|(g, _)| *g > generation)
                .flat_map(|(_, edits)| edits.iter().cloned())
                .collect(),
        )
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
                viewport: selection.range().start,
                resolved: RefCell::new(vec![selection]),
                selection,
                additional: Vec::new(),
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

    pub fn set_viewport(&mut self, id: PaneId, offset: u64) {
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.viewport = offset.min(crate::ByteSource::len(&self.buffer.snapshot()));
        }
    }
    pub fn viewport(&self, id: PaneId) -> Option<u64> {
        self.panes.get(&id).map(|pane| pane.viewport)
    }
    pub(crate) fn reload_positions(&self) -> Vec<u64> {
        self.panes
            .values()
            .flat_map(|pane| {
                std::iter::once(pane.viewport).chain(
                    std::iter::once(pane.selection)
                        .chain(pane.additional.iter().copied())
                        .flat_map(|s| [s.anchor.offset, s.head.offset]),
                )
            })
            .collect()
    }

    pub fn pane_snapshot(&self, id: PaneId) -> Result<PaneSnapshot, SessionError> {
        let pane = self.panes.get(&id).ok_or(SessionError::UnknownPane)?;
        let document = self.snapshot();
        let mut resolved = pane.resolved.borrow_mut();
        let selections: Vec<_> = std::iter::once(pane.selection)
            .chain(pane.additional.iter().copied())
            .collect();
        for selection in &selections[resolved.len()..] {
            resolved.push(selection.resolved(&document.text)?);
        }
        let mut resolved = normalize_selections(resolved.clone())?;
        Ok(PaneSnapshot {
            selection: resolved.remove(0),
            additional: resolved,
            document,
            id,
            epoch: pane.epoch,
        })
    }
    pub fn pane_snapshot_for(&self, base: &PaneSnapshot) -> Result<PaneSnapshot, SessionError> {
        self.pane_snapshot(base.id)
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

    pub fn set_selections(
        &mut self,
        base: &PaneSnapshot,
        selections: Vec<Selection>,
    ) -> Result<(), SessionError> {
        self.validate(base)?;
        let selections = normalize_selections(selections)?;
        for selection in &selections {
            self.validate_selection(*selection)?;
        }
        self.update_selections(base.id, selections);
        self.last_actor = None;
        self.buffer.break_undo_group();
        Ok(())
    }

    fn update_selections(&mut self, id: PaneId, mut selections: Vec<Selection>) {
        let primary = selections.remove(0);
        self.update_selection(id, primary);
        let pane = self.panes.get_mut(&id).unwrap();
        *pane.resolved.get_mut() = std::iter::once(primary)
            .chain(selections.iter().copied())
            .collect();
        pane.additional = selections;
    }

    /// Apply a complete transaction prepared against this pane's immutable
    /// version. All panes are rebased together, and one Undo restores it all.
    pub fn apply_prepared(
        &mut self,
        base: &PaneSnapshot,
        prepared: crate::PreparedEdits,
        group: UndoGroup,
    ) -> Result<(), SessionError> {
        if let Err(error) = self.validate(base) {
            self.buffer.retire_prepared(prepared);
            return Err(error);
        }
        let desired = prepared.selections.clone();
        let edits = prepared.edits.clone();
        let group = if self.last_actor == Some(base.id) {
            group
        } else {
            UndoGroup::New
        };
        if !self.buffer.apply_prepared(prepared, group) {
            return Err(SessionError::Stale);
        }
        if edits.is_empty() {
            if let Some(selections) = desired {
                self.update_selections(base.id, selections);
                self.last_actor = None;
                self.buffer.break_undo_group();
            }
            return Ok(());
        }
        for edit in &edits {
            rebase_panes(&mut self.panes, edit);
        }
        if let Some(selections) = desired {
            self.update_selections(base.id, selections);
        }
        self.last_actor = Some(base.id);
        self.changed(edits);
        Ok(())
    }

    pub fn replace_selections(
        &mut self,
        base: &PaneSnapshot,
        text: &str,
        group: UndoGroup,
    ) -> Result<(), SessionError> {
        let edits: Vec<_> = base
            .selections()
            .iter()
            .map(|s| crate::TextEdit {
                range: s.range(),
                text: text.into(),
            })
            .collect();
        let prepared = base.document.text.prepare_edits(&edits)?;
        self.apply_prepared(base, prepared, group)?;
        let pane = self.panes.get_mut(&base.id).unwrap();
        pane.selection = Selection::caret(pane.selection.range().end);
        for selection in &mut pane.additional {
            *selection = Selection::caret(selection.range().end);
        }
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
        self.changed(vec![edit]);
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
            self.changed(Vec::new());
            self.last_actor = None;
        }
        Ok(changed)
    }

    pub fn undo(&mut self, base: &PaneSnapshot) -> Result<bool, SessionError> {
        self.validate(base)?;
        let panes = &mut self.panes;
        let mut changes = Vec::new();
        let changed = self.buffer.undo_with(|edits| {
            for edit in edits.iter().rev() {
                let inverse = edit.inverse();
                rebase_panes(panes, &inverse);
                changes.push(inverse);
            }
        });
        if changed {
            self.changed(changes);
        }
        self.last_actor = None;
        Ok(changed)
    }

    pub fn redo(&mut self, base: &PaneSnapshot) -> Result<bool, SessionError> {
        self.validate(base)?;
        let panes = &mut self.panes;
        let mut changes = Vec::new();
        let changed = self.buffer.redo_with(|edits| {
            for edit in edits {
                rebase_panes(panes, edit);
                changes.push(edit.clone());
            }
        });
        if changed {
            self.changed(changes);
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
        mapping: &crate::reload::Mapping,
    ) -> Result<EditBuffer, Box<EditBuffer>> {
        if (self.is_dirty() && !discard_edits) || !self.is_current(base) {
            return Err(Box::new(buffer));
        }
        if self.reload_positions().iter().any(|&offset| {
            mapping
                .map(Anchor {
                    offset,
                    bias: Bias::Before,
                })
                .is_none()
        }) {
            return Err(Box::new(buffer));
        }
        let retired = std::mem::replace(&mut self.buffer, buffer);
        for pane in self.panes.values_mut() {
            pane.resolved.get_mut().clear();
            pane.viewport = mapping
                .map(Anchor {
                    offset: pane.viewport,
                    bias: Bias::After,
                })
                .unwrap();
            for selection in std::iter::once(&mut pane.selection).chain(&mut pane.additional) {
                selection.anchor.offset = mapping.map(selection.anchor).unwrap();
                selection.head.offset = mapping.map(selection.head).unwrap();
                if selection.anchor.offset == selection.head.offset {
                    *selection = Selection::caret(selection.head.offset);
                }
            }
        }
        self.last_actor = None;
        self.changed(Vec::new());
        self.changes.clear();
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
        pane.resolved.get_mut().clear();
        pane.selection = selection;
        pane.additional.clear();
        pane.epoch = pane
            .epoch
            .checked_add(1)
            .expect("pane generation exhausted");
    }

    fn changed(&mut self, edits: Vec<EditDelta>) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("document generation exhausted");
        self.changes.push_back((self.generation, edits));
        while self.changes.len() > 256
            || self
                .changes
                .iter()
                .map(|(_, edits)| edits.len())
                .sum::<usize>()
                > 4096
        {
            self.changes.pop_front();
        }
    }
}

fn rebase_panes(panes: &mut BTreeMap<PaneId, Pane>, edit: &EditDelta) {
    for pane in panes.values_mut() {
        let mut viewport = Selection::new(pane.viewport, pane.viewport);
        viewport.rebase(edit);
        pane.viewport = viewport.anchor.offset;
        pane.resolved.get_mut().clear();
        pane.selection.rebase(edit);
        for selection in &mut pane.additional {
            selection.rebase(edit);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CursorCommand {
    Move { direction: Direction, extend: bool },
    Delete { direction: Direction },
    MoveWord { direction: Direction, extend: bool },
    DeleteWord { direction: Direction },
}

impl CursorCommand {
    fn direction(self) -> Direction {
        match self {
            Self::Move { direction, .. }
            | Self::Delete { direction }
            | Self::MoveWord { direction, .. }
            | Self::DeleteWord { direction } => direction,
        }
    }
    fn extend(self) -> bool {
        matches!(
            self,
            Self::Move { extend: true, .. } | Self::MoveWord { extend: true, .. }
        )
    }
    fn deleting(self) -> bool {
        matches!(self, Self::Delete { .. } | Self::DeleteWord { .. })
    }
}

enum Navigation {
    Grapheme(GraphemeSearch),
    Word(crate::WordSearch),
}
impl Navigation {
    fn poll(&mut self, chunks: usize) -> Result<Option<u64>, TextError> {
        match self {
            Self::Grapheme(search) => search.poll(chunks),
            Self::Word(search) => search.poll(chunks * 128),
        }
    }
}

pub struct PendingCommand {
    base: PaneSnapshot,
    command: CursorCommand,
    steps: Vec<(Selection, Option<Navigation>, Option<u64>)>,
    next: usize,
}
impl PendingCommand {
    pub fn poll(&mut self, chunks: usize) -> Result<bool, SessionError> {
        for _ in 0..chunks {
            if self.next == self.steps.len() {
                return Ok(true);
            }
            let (_, search, target) = &mut self.steps[self.next];
            if target.is_none() {
                *target = search.as_mut().unwrap().poll(1)?;
            }
            if target.is_some() {
                self.next += 1;
            }
        }
        Ok(self.next == self.steps.len())
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
        let mut steps = Vec::new();
        for selection in base.selections() {
            let range = selection.range();
            let collapse = !range.is_empty() && !command.extend();
            let target = collapse.then_some(if command.direction() == Direction::Backward {
                range.start
            } else {
                range.end
            });
            let search = if collapse {
                None
            } else if matches!(
                command,
                CursorCommand::MoveWord { .. } | CursorCommand::DeleteWord { .. }
            ) {
                Some(Navigation::Word(
                    base.document
                        .text
                        .word_search(selection.head.offset, command.direction())?,
                ))
            } else {
                Some(Navigation::Grapheme(base.document.text.grapheme_search(
                    selection.head.offset,
                    command.direction(),
                )?))
            };
            steps.push((selection, search, target));
        }
        Ok(PendingCommand {
            base: base.clone(),
            command,
            steps,
            next: 0,
        })
    }
    pub fn finish_command(&mut self, command: &PendingCommand) -> Result<(), SessionError> {
        self.validate(&command.base)?;
        let targets: Vec<_> = command
            .steps
            .iter()
            .map(|(selection, _, target)| {
                target
                    .map(|target| (*selection, target))
                    .ok_or(SessionError::Pending)
            })
            .collect::<Result<_, _>>()?;
        if !command.command.deleting() {
            let selections = targets
                .into_iter()
                .map(|(s, target)| {
                    Selection::new(
                        if command.command.extend() {
                            s.anchor.offset
                        } else {
                            target
                        },
                        target,
                    )
                })
                .collect();
            self.set_selections(&command.base, selections)
        } else {
            let ranges = normalize_selections(
                targets
                    .into_iter()
                    .map(|(s, target)| {
                        if s.range().is_empty() {
                            Selection::new(s.head.offset, target)
                        } else {
                            s
                        }
                    })
                    .collect(),
            )?;
            let edits: Vec<_> = ranges
                .iter()
                .map(|s| crate::TextEdit {
                    range: s.range(),
                    text: String::new(),
                })
                .collect();
            let prepared = command.base.document.text.prepare_edits(&edits)?;
            self.apply_prepared(&command.base, prepared, UndoGroup::New)?;
            let pane = self.panes.get_mut(&command.base.id).unwrap();
            pane.selection = Selection::caret(pane.selection.range().start);
            for selection in &mut pane.additional {
                *selection = Selection::caret(selection.range().start);
            }
            let selections = self.pane_snapshot(command.base.id)?.selections();
            self.update_selections(command.base.id, normalize_selections(selections)?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;

/// Keep the first (primary) selection while merging overlaps and duplicate
/// carets. Adjacent nonempty selections remain separate edit targets.
pub(crate) fn normalize_selections(
    selections: Vec<Selection>,
) -> Result<Vec<Selection>, SessionError> {
    if selections.is_empty() || selections.len() > 4096 {
        return Err(SessionError::InvalidSelections);
    }
    let primary = selections[0];
    let mut sorted = selections;
    sorted.sort_by_key(|s| (s.range().start, s.range().end));
    let mut result: Vec<Selection> = Vec::new();
    for selection in sorted {
        if let Some(last) = result.last_mut() {
            let a = last.range();
            let b = selection.range();
            if b.start < a.end || (b.start == a.end && (a.is_empty() || b.is_empty())) {
                *last = Selection::new(a.start, a.end.max(b.end));
                continue;
            }
        }
        result.push(selection);
    }
    let at = result
        .iter()
        .position(|s| {
            s.range().start <= primary.range().start && s.range().end >= primary.range().end
        })
        .unwrap();
    let mut selected = result.remove(at);
    if primary.anchor.offset > primary.head.offset {
        selected = Selection::new(selected.range().end, selected.range().start);
    }
    result.insert(0, selected);
    Ok(result)
}
