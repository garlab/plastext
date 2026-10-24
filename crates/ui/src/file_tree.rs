use std::{collections::HashMap, path::PathBuf, sync::Arc};

use gpui::{
    Context, EventEmitter, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};
use plastext_editor::Theme;
use plastext_project::{EntryKind, FileTree, TreeOptions, read_dir};

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 14.;

pub enum FileTreeEvent {
    Open(PathBuf),
}

/// The sidebar's file tree (spec PRJ-2): directories are read in the
/// background when expanded, and only the visible rows are rendered.
pub struct FileTreeView {
    tree: FileTree,
    options: Arc<TreeOptions>,
    selected: Option<PathBuf>,
    scroll_handle: UniformListScrollHandle,
    listings: HashMap<PathBuf, Task<()>>,
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

impl FileTreeView {
    pub fn new(root: PathBuf, options: TreeOptions, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tree: FileTree::new(root.clone()),
            options: Arc::new(options),
            selected: None,
            scroll_handle: UniformListScrollHandle::new(),
            listings: HashMap::new(),
        };
        this.list(root, cx);
        this
    }

    pub fn select(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.selected = path;
        cx.notify();
    }

    fn list(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let root = self.tree.root().to_owned();
        let options = self.options.clone();
        let listing = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { read_dir(&root, &dir, &options) }
        });
        let task = cx.spawn({
            let dir = dir.clone();
            async move |this, cx| {
                let entries = listing.await;
                this.update(cx, |this, cx| {
                    this.listings.remove(&dir);
                    // An unreadable directory shows as empty.
                    this.tree.set_children(&dir, entries.unwrap_or_default());
                    cx.notify();
                })
                .ok();
            }
        });
        self.listings.insert(dir, task);
    }

    fn click(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.tree.rows().get(index) else {
            return;
        };
        let entry = row.entry.clone();
        match entry.kind {
            EntryKind::Dir => {
                if self.tree.toggle(&entry.path) {
                    self.list(entry.path.clone(), cx);
                }
            }
            EntryKind::File => cx.emit(FileTreeEvent::Open(entry.path.clone())),
        }
        cx.notify();
    }
}

impl Render for FileTreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "file-tree",
            self.tree.rows().len(),
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let theme = Theme::get(window, cx);
                range
                    .filter_map(|index| {
                        let row = this.tree.rows().get(index)?;
                        let entry = &row.entry;
                        let selected = this.selected.as_ref() == Some(&entry.path);
                        let chevron = match (entry.kind, row.expanded) {
                            (EntryKind::Dir, true) => "▾",
                            (EntryKind::Dir, false) => "▸",
                            (EntryKind::File, _) => "",
                        };
                        let name: SharedString = entry.name.clone().into();
                        Some(
                            div()
                                .id(index)
                                .h(px(ROW_HEIGHT))
                                .pl(px(8. + INDENT * row.depth as f32))
                                .pr_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .when(selected, |row| row.bg(theme.border))
                                .when(!selected, |row| row.hover(|row| row.bg(theme.background)))
                                .text_color(if entry.ignored {
                                    theme.text_muted
                                } else {
                                    theme.text
                                })
                                .child(
                                    div()
                                        .w(px(12.))
                                        .flex_none()
                                        .text_color(theme.text_muted)
                                        .child(chevron),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(name),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.click(index, cx))),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll_handle.clone())
        .size_full()
        .py_1()
    }
}
