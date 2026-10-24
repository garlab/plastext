use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString, Subscription, Window,
    actions, div, prelude::*, px,
};
use plastext_buffer::OpenOptions;
use plastext_editor::{TextView, TextViewStatus, Theme};
use plastext_project::TreeOptions;

use crate::file_tree::{FileTreeEvent, FileTreeView};

actions!(workspace, [CloseTab]);

const KEY_CONTEXT: &str = "Workspace";
const SIDEBAR_WIDTH: f32 = 260.;
const TAB_BAR_HEIGHT: f32 = 34.;
const STATUS_BAR_HEIGHT: f32 = 24.;

/// Registers the workspace's and its views' key bindings.
pub fn init(cx: &mut App) {
    plastext_editor::init(cx);
    cx.bind_keys([KeyBinding::new("cmd-w", CloseTab, Some(KEY_CONTEXT))]);
}

/// A window's root view: one project with its file tree, tabs and status bar
/// (spec §4.1, PRJ-1).
pub struct Workspace {
    root: Option<PathBuf>,
    tree: Option<Entity<FileTreeView>>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// The last problem worth telling the user about, shown in the status bar.
    notice: Option<SharedString>,
    open_options: OpenOptions,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

struct Tab {
    path: PathBuf,
    title: SharedString,
    view: Entity<TextView>,
    _status: Subscription,
}

impl Workspace {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe_window_appearance(window, |_, _, cx| cx.notify())];
        let tree = root.clone().map(|root| {
            let tree = cx.new(|cx| FileTreeView::new(root, TreeOptions::default(), cx));
            subscriptions.push(
                cx.subscribe_in(&tree, window, |this, _, event, window, cx| match event {
                    FileTreeEvent::Open(path) => this.open_file(path.clone(), None, window, cx),
                }),
            );
            tree
        });
        Self {
            root,
            tree,
            tabs: Vec::new(),
            active: None,
            notice: None,
            open_options: OpenOptions::default(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The window title for a project: its folder name, or the app name without one.
    pub fn title(root: Option<&Path>) -> SharedString {
        match root {
            Some(root) => root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy()
                .into_owned()
                .into(),
            None => "plastext".into(),
        }
    }

    /// Opens `path` in a tab, or switches to its tab, and scrolls to 1-based `line`.
    pub fn open_file(
        &mut self,
        path: PathBuf,
        line: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate(index, window, cx);
            if let Some(line) = line {
                self.tabs[index]
                    .view
                    .update(cx, |view, cx| view.scroll_to_line(line, cx));
            }
            return;
        }
        let options = self.open_options;
        let opening = cx.background_executor().spawn({
            let path = path.clone();
            async move { plastext_buffer::open(&path, options) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            this.update_in(cx, |this, window, cx| match opened {
                Ok((source, index)) => {
                    let view = cx.new(|cx| TextView::new(source, index, cx));
                    if let Some(line) = line {
                        view.update(cx, |view, cx| view.scroll_to_line(line, cx));
                    }
                    this.add_tab(path, view, window, cx);
                }
                Err(err) => {
                    this.notice = Some(format!("Couldn't open {}: {err}", path.display()).into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn add_tab(
        &mut self,
        path: PathBuf,
        view: Entity<TextView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = path
            .file_name()
            .map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
            .into();
        let status = cx.observe(&view, |_, _, cx| cx.notify());
        self.tabs.push(Tab {
            path,
            title,
            view,
            _status: status,
        });
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(index);
        self.notice = None;
        let tab = &self.tabs[index];
        tab.view.focus_handle(cx).focus(window);
        let path = tab.path.clone();
        if let Some(tree) = &self.tree {
            tree.update(cx, |tree, cx| tree.select(Some(path), cx));
        }
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        match self.active {
            _ if self.tabs.is_empty() => {
                self.active = None;
                self.focus_handle.focus(window);
                if let Some(tree) = &self.tree {
                    tree.update(cx, |tree, cx| tree.select(None, cx));
                }
            }
            Some(active) if active >= index => self.activate(active.saturating_sub(1), window, cx),
            Some(active) => self.activate(active, window, cx),
            None => {}
        }
        cx.notify();
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active) = self.active {
            self.close_tab(active, window, cx);
        }
    }

    fn render_tab_bar(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tabs")
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let active = self.active == Some(index);
                div()
                    .id(("tab", index))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .pr_2()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |tab| {
                        tab.bg(theme.background).text_color(theme.text)
                    })
                    .when(!active, |tab| tab.text_color(theme.text_muted))
                    .child(tab.title.clone())
                    .child(
                        div()
                            .id(("close-tab", index))
                            .px_1()
                            .rounded_sm()
                            .text_color(theme.text_muted)
                            .hover(|close| close.bg(theme.border))
                            .child("×")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            })),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.activate(index, window, cx)),
                    )
            }))
    }

    fn render_status_bar(&self, theme: Theme, cx: &App) -> impl IntoElement {
        let status = self
            .active
            .and_then(|active| self.tabs.get(active))
            .map(|tab| tab.view.read(cx).status());
        let notice = self
            .notice
            .clone()
            .or_else(|| status.as_ref().and_then(|status| status.error.clone()));
        div()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_4()
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.text_muted)
            .child("READ")
            .children(notice)
            .child(div().flex_1())
            .children(status.map(status_text))
    }
}

fn status_text(status: TextViewStatus) -> String {
    let mut parts = Vec::new();
    if let Some(indexing) = status.indexing {
        parts.push(format!("Indexing lines {:.0}%", indexing * 100.));
    }
    let lines = if status.indexing.is_some() {
        format!("~{} lines", status.lines)
    } else {
        format!("{} lines", status.lines)
    };
    match status.line {
        Some(line) => parts.push(format!("Ln {line} of {lines}")),
        None => parts.push(lines),
    }
    parts.push(format_size(status.len));
    parts.join("    ")
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000. && unit < UNITS.len() - 1 {
        size /= 1000.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let sidebar = div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .text_sm()
            .map(|sidebar| match &self.tree {
                Some(tree) => sidebar.child(tree.clone()),
                None => sidebar
                    .p_3()
                    .text_color(theme.text_muted)
                    .child("No folder open. Run `plastext <folder>` to open one."),
            });
        let content = match self.active.and_then(|active| self.tabs.get(active)) {
            Some(tab) => div().flex_1().min_h_0().child(tab.view.clone()),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.text_muted)
                .child(match &self.root {
                    Some(root) => format!("{}: pick a file in the sidebar", root.display()),
                    None => "Nothing open".to_owned(),
                }),
        };
        let editor_area = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .when(!self.tabs.is_empty(), |area| {
                area.child(self.render_tab_bar(theme, cx))
            })
            .child(content);

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_active_tab))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sidebar)
                    .child(editor_area),
            )
            .child(self.render_status_bar(theme, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn formats_sizes_in_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500), "1.5 KB");
        assert_eq!(format_size(10_000_000_000), "10.0 GB");
    }
}
