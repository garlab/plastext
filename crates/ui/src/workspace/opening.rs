use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenKind {
    Preview,
    Keep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Placement {
    Existing(usize),
    Replace(usize),
    Append,
}

impl OpenKind {
    pub fn placement<'a>(
        self,
        path: &Path,
        tabs: impl Iterator<Item = (&'a Path, bool)>,
    ) -> Placement {
        let mut preview = None;
        for (index, (existing, is_preview)) in tabs.enumerate() {
            if path == existing {
                return Placement::Existing(index);
            }
            if is_preview {
                preview = Some(index);
            }
        }
        match (self, preview) {
            (Self::Preview, Some(index)) => Placement::Replace(index),
            _ => Placement::Append,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Destination {
    pub pane: usize,
    pub kind: OpenKind,
    pub activate: bool,
    pub focus_editor: bool,
}

#[derive(Clone, Copy)]
struct Pending {
    id: u64,
    kind: OpenKind,
    focus_editor: bool,
}

/// Per-pane requests: obsolete previews cannot win a race with newer clicks.
/// A double-click promotes an in-flight open instead of starting a duplicate.
#[derive(Default)]
pub(super) struct Openings {
    next: u64,
    latest: Option<u64>,
    pending: HashMap<PathBuf, Pending>,
}

impl Openings {
    pub fn begin(&mut self, path: &Path, kind: OpenKind) -> Option<u64> {
        self.pending
            .retain(|existing, request| existing == path || request.kind == OpenKind::Keep);
        if let Some(request) = self.pending.get_mut(path) {
            if kind == OpenKind::Keep {
                request.kind = kind;
            }
            self.latest = Some(request.id);
            return None;
        }
        self.next += 1;
        let id = self.next;
        self.pending.insert(
            path.to_owned(),
            Pending {
                id,
                kind,
                focus_editor: true,
            },
        );
        self.latest = Some(id);
        Some(id)
    }

    pub fn finish(&mut self, path: &Path, id: u64) -> Option<(OpenKind, bool)> {
        let request = self.pending.get(path)?;
        if request.id != id {
            return None;
        }
        let kind = request.kind;
        self.pending.remove(path);
        Some((kind, self.latest == Some(id)))
    }

    pub fn cancel(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    pub fn set_focus(&mut self, path: &Path, focus_editor: bool) {
        if let Some(pending) = self.pending.get_mut(path) {
            pending.focus_editor = focus_editor;
        }
    }
    pub fn focus_editor(&self, path: &Path) -> bool {
        self.pending
            .get(path)
            .is_some_and(|pending| pending.focus_editor)
    }
    pub fn cancel_under(&mut self, path: &Path) {
        self.pending.retain(|pending, _| !pending.starts_with(path));
    }

    /// Explicit tab selection/closure supersedes outstanding preview navigation.
    pub fn select(&mut self) {
        self.latest = None;
        self.pending
            .retain(|_, request| request.kind == OpenKind::Keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_reuses_its_slot_but_kept_tabs_and_existing_files_survive() {
        let tabs = [
            (Path::new("kept.md"), false),
            (Path::new("preview.md"), true),
        ];
        assert_eq!(
            OpenKind::Preview.placement(Path::new("next.md"), tabs.into_iter()),
            Placement::Replace(1)
        );
        assert_eq!(
            OpenKind::Keep.placement(Path::new("next.md"), tabs.into_iter()),
            Placement::Append
        );
        assert_eq!(
            OpenKind::Preview.placement(Path::new("kept.md"), tabs.into_iter()),
            Placement::Existing(0)
        );
        assert_eq!(
            OpenKind::Keep.placement(Path::new("preview.md"), tabs.into_iter()),
            Placement::Existing(1)
        );
        assert_eq!(
            OpenKind::Preview.placement(
                Path::new("next.md"),
                tabs.into_iter().map(|(path, _)| (path, false))
            ),
            Placement::Append
        );
    }

    #[test]
    fn slow_preview_cannot_replace_the_latest_file_even_when_reopened() {
        let mut opens = Openings::default();
        let a = Path::new("a.md");
        let b = Path::new("b.md");
        let first = opens.begin(a, OpenKind::Preview).unwrap();
        let second = opens.begin(b, OpenKind::Preview).unwrap();
        let third = opens.begin(a, OpenKind::Preview).unwrap();
        assert_eq!(opens.finish(a, first), None);
        assert_eq!(opens.finish(b, second), None);
        assert_eq!(opens.finish(a, third), Some((OpenKind::Preview, true)));
    }

    #[test]
    fn double_click_promotes_the_pending_open_and_keeps_it_on_later_navigation() {
        let mut opens = Openings::default();
        let a = Path::new("a.md");
        let id = opens.begin(a, OpenKind::Preview).unwrap();
        assert_eq!(opens.begin(a, OpenKind::Keep), None);
        assert_eq!(opens.begin(a, OpenKind::Preview), None);
        opens.begin(Path::new("b.md"), OpenKind::Preview);
        assert_eq!(opens.finish(a, id), Some((OpenKind::Keep, false)));
    }

    #[test]
    fn a_new_navigation_target_supersedes_the_pending_callback() {
        let mut opens = Openings::default();
        let path = Path::new("a.md");
        let first = opens.begin(path, OpenKind::Keep).unwrap();
        opens.cancel(path);
        let second = opens.begin(path, OpenKind::Keep).unwrap();
        assert_eq!(opens.finish(path, first), None);
        assert_eq!(opens.finish(path, second), Some((OpenKind::Keep, true)));
    }

    #[test]
    fn selection_cancels_previews_without_losing_explicit_opens_or_affecting_other_panes() {
        let mut first_pane = Openings::default();
        let mut second_pane = Openings::default();
        let a = Path::new("a.md");
        let kept = first_pane.begin(a, OpenKind::Keep).unwrap();
        let preview = first_pane
            .begin(Path::new("b.md"), OpenKind::Preview)
            .unwrap();
        let other = second_pane.begin(a, OpenKind::Preview).unwrap();
        first_pane.select();
        assert_eq!(first_pane.finish(Path::new("b.md"), preview), None);
        assert_eq!(first_pane.finish(a, kept), Some((OpenKind::Keep, false)));
        assert_eq!(
            second_pane.finish(a, other),
            Some((OpenKind::Preview, true))
        );
    }
}
