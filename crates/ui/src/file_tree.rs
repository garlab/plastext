use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, PathBuilder, Pixels, Point, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, canvas, div, img, point, prelude::*, px, uniform_list,
};
use plastext_editor::Theme;
use plastext_platform::watch::Batch;
use plastext_project::{EntryKind, FileTree, TreeOptions, read_dir};

use crate::icon_themes::{IconThemes, language_id};
use plastext_settings::icons::Appearance;

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 14.;
pub(crate) const TOOLBAR_HEIGHT: f32 = 30.;
mod creation;
mod git;
use creation::Creation;

#[cfg(test)]
#[path = "file_tree_tests.rs"]
mod tests;

pub enum FileTreeEvent {
    New {
        folder: bool,
    },
    Create {
        path: PathBuf,
        folder: bool,
    },
    Action(FileAction),
    Open {
        path: PathBuf,
        preview: bool,
    },
    ContextMenu {
        path: Option<PathBuf>,
        position: Point<Pixels>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum FileAction {
    Duplicate,
    Copy,
    Cut,
    Paste,
    Rename,
    Move,
    Trash,
}

/// The sidebar's file tree (spec PRJ-2): directories are read in the
/// background when expanded, and only the visible rows are rendered.
pub struct FileTreeView {
    tree: FileTree,
    options: Arc<TreeOptions>,
    selected: Option<PathBuf>,
    focus: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    listings: HashMap<PathBuf, Task<()>>,
    relist: HashSet<PathBuf>,
    directory_versions: HashMap<PathBuf, DirectoryVersion>,
    creation: Option<Creation>,
    git_status: Option<Arc<plastext_git::RepositorySet>>,
    icons: Entity<IconThemes>,
    _icons_changed: Subscription,
}

#[derive(Clone, PartialEq, Eq)]
struct DirectoryVersion {
    target: PathBuf,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    len: u64,
}

impl DirectoryVersion {
    fn read(path: &std::path::Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            target: std::fs::canonicalize(path)?,
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            len: metadata.len(),
        })
    }
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

impl FileTreeView {
    pub fn new(
        root: PathBuf,
        options: TreeOptions,
        icons: Entity<IconThemes>,
        cx: &mut Context<Self>,
    ) -> Self {
        let icons_changed = cx.observe(&icons, |_, _, cx| cx.notify());
        let mut this = Self {
            tree: FileTree::new(root.clone()),
            options: Arc::new(options),
            selected: None,
            focus: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            listings: HashMap::new(),
            relist: HashSet::new(),
            directory_versions: HashMap::new(),
            creation: None,
            git_status: None,
            icons,
            _icons_changed: icons_changed,
        };
        this.list(root, cx);
        this
    }

    pub fn select(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.selected = path;
        cx.notify();
    }
    pub fn selected(&self) -> Option<PathBuf> {
        self.selected.clone()
    }
    pub(crate) fn set_git_status(
        &mut self,
        snapshot: Option<Arc<plastext_git::RepositorySet>>,
        cx: &mut Context<Self>,
    ) {
        self.git_status = snapshot;
        cx.notify();
    }
    pub(crate) fn git_status(&self, path: &std::path::Path) -> Option<plastext_git::FileStatus> {
        self.git_status.as_ref()?.status(path)
    }
    pub(crate) fn git_folder_status(
        &self,
        path: &std::path::Path,
    ) -> Option<plastext_git::FileStatus> {
        self.git_status.as_ref()?.folder_status(path)
    }
    pub(crate) fn git_revision(&self) -> u64 {
        self.tree.revision()
    }
    pub(crate) fn git_discovery_directories(&self) -> Vec<PathBuf> {
        // Probe listed child folders too, without recursively enumerating them.
        self.tree
            .visible_directories()
            .iter()
            .cloned()
            .chain(
                self.tree
                    .rows()
                    .iter()
                    .filter(|row| row.entry.kind == EntryKind::Dir)
                    .map(|row| row.entry.path.clone()),
            )
            .take(4000)
            .collect()
    }
    pub(crate) fn watched_directories(&self) -> &[PathBuf] {
        self.tree.visible_directories()
    }

    pub(crate) fn refresh_external(&mut self, batch: &Batch, polled: bool, cx: &mut Context<Self>) {
        let ignore_changed = batch.ignore_rules_changed();
        for dir in self.tree.visible_directories().to_vec() {
            let forced = ignore_changed
                || batch.affects_directory(&dir)
                || self
                    .directory_versions
                    .get(&dir)
                    .is_some_and(|version| batch.affects_directory(&version.target));
            if forced || polled && !self.listings.contains_key(&dir) {
                self.list_if_changed(dir, forced, cx);
            }
        }
    }
    pub fn is_directory(&self, path: &std::path::Path) -> bool {
        self.tree
            .rows()
            .iter()
            .any(|row| row.entry.path == path && row.entry.kind == EntryKind::Dir)
    }
    pub fn selected_directory(&self) -> PathBuf {
        self.selected
            .as_ref()
            .and_then(|path| self.tree.rows().iter().find(|row| row.entry.path == *path))
            .map(|row| {
                if row.entry.kind == EntryKind::Dir {
                    row.entry.path.clone()
                } else {
                    row.entry
                        .path
                        .parent()
                        .unwrap_or(self.tree.root())
                        .to_owned()
                }
            })
            .unwrap_or_else(|| self.tree.root().to_owned())
    }
    pub fn refresh(
        &mut self,
        source: &std::path::Path,
        destination: Option<&std::path::Path>,
        changed_source: bool,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .tree
            .rows()
            .iter()
            .position(|row| self.selected.as_ref() == Some(&row.entry.path))
            .and_then(|index| {
                self.tree.rows()[index..]
                    .iter()
                    .find(|row| !row.entry.path.starts_with(source))
                    .or_else(|| {
                        self.tree.rows()[..index]
                            .iter()
                            .rev()
                            .find(|row| !row.entry.path.starts_with(source))
                    })
            })
            .map(|row| row.entry.path.clone());
        if changed_source {
            self.tree.forget(source);
        }
        if source.starts_with(self.tree.root())
            && let Some(parent) = source.parent()
        {
            self.list(parent.to_owned(), cx);
        }
        if let Some(destination) = destination.filter(|path| path.starts_with(self.tree.root())) {
            self.selected = Some(destination.to_owned());
            let mut parents: Vec<_> = destination
                .ancestors()
                .skip(1)
                .take_while(|path| path.starts_with(self.tree.root()))
                .map(std::path::Path::to_owned)
                .collect();
            parents.reverse();
            for parent in parents {
                self.tree.expand(&parent);
                self.list(parent, cx);
            }
        } else if changed_source
            && self
                .selected
                .as_ref()
                .is_some_and(|path| path.starts_with(source))
        {
            self.selected = next;
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .creation
            .as_ref()
            .is_some_and(|creation| creation.input.focus_handle(cx).is_focused(window))
        {
            match event.keystroke.key.as_str() {
                "enter" => self.submit_create(window, cx),
                "escape" => self.cancel_create(window, cx),
                _ => return,
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.platform {
            let action = match key {
                "backspace" => FileAction::Trash,
                _ => return,
            };
            cx.emit(FileTreeEvent::Action(action));
        } else {
            let current = self
                .tree
                .rows()
                .iter()
                .position(|row| Some(&row.entry.path) == self.selected.as_ref());
            match key {
                "f2" => cx.emit(FileTreeEvent::Action(FileAction::Rename)),
                "enter" => {
                    if let Some(index) = current {
                        self.click(index, 2, cx);
                    }
                }
                "up" | "down" if !self.tree.rows().is_empty() => {
                    let last = self.tree.rows().len() - 1;
                    let index = current.map_or(0, |index| {
                        if key == "up" {
                            index.saturating_sub(1)
                        } else {
                            (index + 1).min(last)
                        }
                    });
                    self.selected = Some(self.tree.rows()[index].entry.path.clone());
                    self.scroll_handle
                        .scroll_to_item(index, gpui::ScrollStrategy::Center);
                    cx.notify();
                }
                _ => return,
            }
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn list(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.list_if_changed(dir, true, cx);
    }

    fn list_if_changed(&mut self, dir: PathBuf, force: bool, cx: &mut Context<Self>) {
        // Coalesce changes arriving during enumeration into one follow-up.
        if self.listings.contains_key(&dir) {
            if force {
                self.relist.insert(dir);
            }
            return;
        }
        let root = self.tree.root().to_owned();
        let options = self.options.clone();
        let previous = self.directory_versions.get(&dir).cloned();
        let listing = cx.background_executor().spawn({
            let dir = dir.clone();
            async move {
                // Read metadata before listing so changes during enumeration
                // are caught on the next check. Unchanged polls only stat.
                let version = DirectoryVersion::read(&dir).ok();
                if !force && version.is_some() && version == previous {
                    return None;
                }
                Some((version, read_dir(&root, &dir, &options)))
            }
        });
        let task = cx.spawn({
            let dir = dir.clone();
            async move |this, cx| {
                let result = listing.await;
                this.update(cx, |this, cx| {
                    this.listings.remove(&dir);
                    if this.relist.remove(&dir) {
                        this.list(dir, cx);
                        return;
                    }
                    let Some((version, entries)) = result else {
                        return;
                    };
                    this.directory_versions.remove(&dir);
                    if entries.is_ok()
                        && let Some(version) = version
                    {
                        this.directory_versions.insert(dir.clone(), version);
                    }
                    // An unreadable directory shows as empty.
                    let removed = this.tree.set_children(&dir, entries.unwrap_or_default());
                    if !removed.is_empty() {
                        let survives =
                            |path: &PathBuf| !removed.iter().any(|old| path.starts_with(old));
                        this.listings.retain(|path, _| survives(path));
                        this.relist.retain(survives);
                        this.directory_versions.retain(|path, _| survives(path));
                        if this.selected.as_ref().is_some_and(|path| !survives(path)) {
                            this.selected = None;
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        });
        self.listings.insert(dir, task);
    }

    fn click(&mut self, index: usize, click_count: usize, cx: &mut Context<Self>) {
        let Some(row) = self.tree.rows().get(index) else {
            return;
        };
        let entry = row.entry.clone();
        self.selected = Some(entry.path.clone());
        match entry.kind {
            EntryKind::Dir => {
                self.tree.toggle(&entry.path);
                // Collapsed directories are not watched. Refresh all newly
                // visible listings, including previously expanded descendants.
                for dir in self.tree.visible_directories().to_vec() {
                    if dir.starts_with(&entry.path) {
                        self.list(dir, cx);
                    }
                }
            }
            EntryKind::File => cx.emit(FileTreeEvent::Open {
                path: entry.path.clone(),
                preview: click_count < 2,
            }),
        }
        cx.notify();
    }
}

impl Render for FileTreeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let extra = self.creation_rows();
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .key_context("FileTree")
            .when(self.selected.is_some(), |view| {
                view.on_action(cx.listener(|_, _: &plastext_editor::actions::Copy, _, cx| {
                    cx.emit(FileTreeEvent::Action(FileAction::Copy))
                }))
                .on_action(cx.listener(
                    |_, _: &plastext_editor::actions::Cut, _, cx| {
                        cx.emit(FileTreeEvent::Action(FileAction::Cut))
                    },
                ))
            })
            .on_action(
                cx.listener(|_, _: &plastext_editor::actions::Paste, _, cx| {
                    cx.emit(FileTreeEvent::Action(FileAction::Paste))
                }),
            )
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus.focus(window)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    this.focus.focus(window);
                    this.select(None, cx);
                    cx.emit(FileTreeEvent::ContextMenu {
                        path: None,
                        position: event.position,
                    });
                }),
            )
            .child(self.render_toolbar(theme, cx))
            .child(
                uniform_list(
                    "file-tree",
                    self.tree.rows().len() + extra,
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let theme = Theme::get(window, cx);
                        let light = matches!(
                            window.appearance(),
                            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
                        );
                        let icons = this.icons.read(cx);
                        let icon_theme = icons.current();
                        let appearance = if icons.preferences.high_contrast {
                            Appearance::HighContrast
                        } else if light {
                            Appearance::Light
                        } else {
                            Appearance::Dark
                        };
                        let hide_arrows = icon_theme
                            .as_ref()
                            .is_some_and(|theme| theme.rules.hides_explorer_arrows);
                        let scale = window.scale_factor();
                        let (insert, depth) = this.creation_position();
                        let extra = this.creation_rows();
                        range
                            .filter_map(|index| {
                                if extra != 0 && (insert..insert + extra).contains(&index) {
                                    return Some(this.render_creation(
                                        index != insert,
                                        depth,
                                        theme,
                                        cx,
                                    ));
                                }
                                let index = if extra != 0 && index >= insert + extra {
                                    index - extra
                                } else {
                                    index
                                };
                                let row = this.tree.rows().get(index)?;
                                let entry = &row.entry;
                                let selected = this.selected.as_ref() == Some(&entry.path);
                                let name: SharedString = entry.name.clone().into();
                                let menu_path = entry.path.clone();
                                let icon = icon_theme
                                    .as_ref()
                                    .and_then(|theme| {
                                        let language = language_id(&entry.path);
                                        theme.rules.resolve(
                                            &entry.path,
                                            entry.kind == EntryKind::Dir,
                                            row.expanded,
                                            false,
                                            language.as_deref(),
                                            appearance,
                                        )
                                    })
                                    .map(|asset| {
                                        let image = this.icons.update(cx, |icons, cx| {
                                            icons.image(asset, scale, theme.text.into(), cx)
                                        });
                                        div()
                                            .size(px(18.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(entry.ignored, |icon| icon.opacity(0.5))
                                            .children(image.map(|image| img(image).size(px(16.))))
                                    });
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
                                        .when(!selected, |row| {
                                            row.hover(|row| row.bg(theme.background))
                                        })
                                        .when_some(
                                            (entry.kind == EntryKind::Dir)
                                                .then(|| {
                                                    this.git_status
                                                        .as_ref()?
                                                        .folder_summary(&entry.path)
                                                        .cloned()
                                                })
                                                .flatten(),
                                            |row, summary| {
                                                let path = entry.path.clone();
                                                row.tooltip(move |_, cx| {
                                                    cx.new(|_| git::Tooltip::new(&path, &summary))
                                                        .into()
                                                })
                                            },
                                        )
                                        .text_color(
                                            crate::workspace::git::color(
                                                if entry.kind == EntryKind::Dir {
                                                    this.git_folder_status(&entry.path)
                                                } else {
                                                    this.git_status(&entry.path)
                                                },
                                                theme,
                                            )
                                            .unwrap_or(if entry.ignored {
                                                theme.text_muted
                                            } else {
                                                theme.text
                                            }),
                                        )
                                        // A file icon occupies the disclosure column.
                                        // An extra empty chevron slot makes files look
                                        // like children of their sibling directories.
                                        .when(
                                            !hide_arrows
                                                && (entry.kind == EntryKind::Dir || icon.is_none()),
                                            |row_element| {
                                                row_element.child(
                                                    div().size(px(18.)).flex_none().when(
                                                        entry.kind == EntryKind::Dir,
                                                        |slot| {
                                                            slot.child(disclosure(
                                                                row.expanded,
                                                                theme.text_muted,
                                                            ))
                                                        },
                                                    ),
                                                )
                                            },
                                        )
                                        .children(icon)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(name),
                                        )
                                        .on_click(cx.listener(
                                            move |this, event: &ClickEvent, window, cx| {
                                                this.focus.focus(window);
                                                this.click(index, event.click_count(), cx)
                                            },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    window.prevent_default();
                                                    this.focus.focus(window);
                                                    this.select(Some(menu_path.clone()), cx);
                                                    cx.emit(FileTreeEvent::ContextMenu {
                                                        path: Some(menu_path.clone()),
                                                        position: event.position,
                                                    });
                                                },
                                            ),
                                        )
                                        .into_any_element(),
                                )
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll_handle.clone())
                .flex_1()
                .min_h_0()
                .w_full()
                .py_1(),
            )
    }
}

impl Focusable for FileTreeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A consistent disclosure chevron independent of the system font's glyphs.
pub(super) fn disclosure(expanded: bool, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut path = PathBuilder::stroke(px(1.75));
            let points = if expanded {
                [(4., 6.5), (9., 11.5), (14., 6.5)]
            } else {
                [(6.5, 4.), (11.5, 9.), (6.5, 14.)]
            };
            path.move_to(bounds.origin + point(px(points[0].0), px(points[0].1)));
            for (x, y) in &points[1..] {
                path.line_to(bounds.origin + point(px(*x), px(*y)));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(18.))
}
