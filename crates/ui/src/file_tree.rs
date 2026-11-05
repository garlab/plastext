use std::{collections::HashMap, path::PathBuf, sync::Arc};

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, PathBuilder, Pixels, Point, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, canvas, div, img, point, prelude::*, px, uniform_list,
};
use plastext_editor::Theme;
use plastext_project::{EntryKind, FileTree, TreeOptions, read_dir};

use crate::icon_themes::{IconThemes, language_id};
use plastext_settings::icons::Appearance;

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 14.;

pub enum FileTreeEvent {
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
    icons: Entity<IconThemes>,
    _icons_changed: Subscription,
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
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.platform {
            let action = match key {
                "backspace" => FileAction::Trash,
                "c" => FileAction::Copy,
                "x" => FileAction::Cut,
                "v" => FileAction::Paste,
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

    fn click(&mut self, index: usize, click_count: usize, cx: &mut Context<Self>) {
        let Some(row) = self.tree.rows().get(index) else {
            return;
        };
        let entry = row.entry.clone();
        self.selected = Some(entry.path.clone());
        match entry.kind {
            EntryKind::Dir => {
                if self.tree.toggle(&entry.path) {
                    self.list(entry.path.clone(), cx);
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("FileTree")
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
            .child(
                uniform_list(
                    "file-tree",
                    self.tree.rows().len(),
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
                        range
                            .filter_map(|index| {
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
                                        .text_color(if entry.ignored {
                                            theme.text_muted
                                        } else {
                                            theme.text
                                        })
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
                                        ),
                                )
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll_handle.clone())
                .size_full()
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
