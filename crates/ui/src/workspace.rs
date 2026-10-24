use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    NavigationDirection, SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use plastext_buffer::{ByteSource as _, FileSource, LineIndex, OpenOptions};
use plastext_editor::{HexView, TextStyle, TextView, Theme};
use plastext_project::{ShowIgnored, TreeOptions};
use plastext_settings::{self as settings, Settings};

use crate::{
    file_tree::{FileTreeEvent, FileTreeView},
    layout::{Axis, Layout},
    quick_open::{QuickOpen, QuickOpenEvent},
    search_panel::{SearchPanel, SearchPanelEvent},
    tab_view::TabView,
};

actions!(
    workspace,
    [
        CloseTab,
        SplitRight,
        SplitDown,
        OpenQuickOpen,
        GoToLine,
        SearchProject,
        ShowFiles,
        GoBack,
        GoForward
    ]
);

/// How far back navigation history goes.
const HISTORY_LEN: usize = 100;

const KEY_CONTEXT: &str = "Workspace";
const SIDEBAR_WIDTH: f32 = 260.;
const TAB_BAR_HEIGHT: f32 = 34.;
const STATUS_BAR_HEIGHT: f32 = 24.;

/// Registers the workspace's and its views' key bindings.
pub fn init(cx: &mut App) {
    plastext_editor::init(cx);
    crate::quick_open::init(cx);
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-p", OpenQuickOpen, context),
        KeyBinding::new("ctrl-g", GoToLine, context),
        KeyBinding::new("cmd-shift-f", SearchProject, context),
        KeyBinding::new("cmd-shift-e", ShowFiles, context),
        KeyBinding::new("ctrl--", GoBack, context),
        KeyBinding::new("ctrl-shift--", GoForward, context),
        KeyBinding::new("cmd-[", GoBack, context),
        KeyBinding::new("cmd-]", GoForward, context),
        KeyBinding::new("cmd-w", CloseTab, context),
        KeyBinding::new("cmd-\\", SplitRight, context),
        KeyBinding::new("cmd-alt-\\", SplitDown, context),
    ]);
}

/// A window's root view: one project with its file tree, panes of tabs and
/// status bar (spec §4.1, PRJ-1).
pub struct Workspace {
    root: Option<PathBuf>,
    tree: Option<Entity<FileTreeView>>,
    panes: HashMap<usize, Pane>,
    layout: Layout,
    active_pane: usize,
    next_pane: usize,
    /// The last problem worth telling the user about, shown in the status bar.
    notice: Option<SharedString>,
    tree_options: TreeOptions,
    sidebar: Sidebar,
    search: Option<(Entity<SearchPanel>, Subscription)>,
    quick_open: Option<(Entity<QuickOpen>, Subscription)>,
    open_options: OpenOptions,
    syntax_max_file_size: u64,
    text_style: TextStyle,
    focus_handle: FocusHandle,
    /// Where jumps came from (spec NAV-5), most recent last.
    back: Vec<Place>,
    /// Where going back came from.
    forward: Vec<Place>,
    _subscriptions: Vec<Subscription>,
}

/// A position in a file, for navigation history.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    path: PathBuf,
    offset: u64,
}

/// Where to scroll a file once it's open.
#[derive(Clone, Copy)]
enum Target {
    Line(u64),
    Offset(u64),
}

/// What the sidebar shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Files,
    Search,
}

#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    active: Option<usize>,
}

struct Tab {
    path: PathBuf,
    title: SharedString,
    view: TabView,
    _status: Subscription,
}

impl Pane {
    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }
}

impl Workspace {
    pub fn new(
        root: Option<PathBuf>,
        settings: Arc<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = vec![cx.observe_window_appearance(window, |_, _, cx| cx.notify())];
        let files = &settings.files;
        let show_ignored = match files.show_ignored {
            settings::ShowIgnored::Hide => ShowIgnored::Hide,
            settings::ShowIgnored::Dim => ShowIgnored::Dim,
            settings::ShowIgnored::Show => ShowIgnored::Show,
        };
        let mut notice = None;
        let tree_options = TreeOptions::new(files.show_hidden, show_ignored, &files.exclude)
            .unwrap_or_else(|err| {
                notice = Some(format!("Ignoring files.exclude in the settings: {err}").into());
                TreeOptions::default()
            });
        let tree = root.clone().map(|root| {
            let tree = cx.new(|cx| FileTreeView::new(root, tree_options.clone(), cx));
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
            panes: HashMap::from([(0, Pane::default())]),
            layout: Layout::Pane(0),
            active_pane: 0,
            next_pane: 1,
            notice,
            tree_options,
            sidebar: Sidebar::Files,
            search: None,
            quick_open: None,
            open_options: OpenOptions {
                large_file_threshold: settings.limits.large_file_threshold.0,
                block_cache: settings.limits.block_cache.0,
            },
            syntax_max_file_size: settings.limits.syntax_max_file_size.0,
            text_style: TextStyle {
                font_family: settings.editor.font.clone().into(),
                font_size: px(settings.editor.font_size),
                line_height: settings.editor.line_height,
                tab_size: settings.editor.tab_size,
            },
            focus_handle: cx.focus_handle(),
            back: Vec::new(),
            forward: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Shows `notice` in the status bar until it's clicked away.
    pub fn show_notice(&mut self, notice: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        cx.notify();
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

    /// Opens `path` in a tab of the active pane, or switches to its tab there,
    /// and scrolls to 1-based `line`.
    pub fn open_file(
        &mut self,
        path: PathBuf,
        line: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_jump(cx);
        self.open(path, line.map(Target::Line), window, cx);
    }

    fn open(
        &mut self,
        path: PathBuf,
        target: Option<Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.active_pane;
        if let Some(index) = self.panes[&pane]
            .tabs
            .iter()
            .position(|tab| tab.path == path)
        {
            self.activate(pane, index, window, cx);
            let view = self.panes[&pane].tabs[index].view.clone();
            scroll_to(&view, target, cx);
            return;
        }
        let options = self.open_options;
        let syntax_max_file_size = self.syntax_max_file_size;
        let style = self.text_style.clone();
        let opening = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                let source = Arc::new(FileSource::open(&path, options)?);
                let head = source.head(plastext_buffer::SNIFF_LEN)?;
                if plastext_buffer::looks_binary(&head) {
                    return Ok(Opened::Binary(source));
                }
                let index = LineIndex::for_source(source.clone())?;
                let language = (source.in_memory().is_some()
                    && source.len() <= syntax_max_file_size)
                    .then(|| plastext_syntax::detect(&path, &head[..head.len().min(256)]))
                    .flatten();
                std::io::Result::Ok(Opened::Text(source, index, language))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            this.update_in(cx, |this, window, cx| match opened {
                Ok(opened) => {
                    let view = match opened {
                        Opened::Text(source, index, language) => TabView::Text(
                            cx.new(|cx| TextView::new(source, index, style, language, cx)),
                        ),
                        Opened::Binary(source) => {
                            TabView::Hex(cx.new(|cx| HexView::new(source, style, cx)))
                        }
                    };
                    scroll_to(&view, target, cx);
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

    /// Adds a tab to the active pane, or the first pane if that one is gone.
    fn add_tab(
        &mut self,
        path: PathBuf,
        view: TabView,
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
        let status = view.observe(cx);
        let pane_id = self.active_pane;
        let pane = self.panes.entry(pane_id).or_default();
        pane.tabs.push(Tab {
            path,
            title,
            view,
            _status: status,
        });
        let index = pane.tabs.len() - 1;
        self.activate(pane_id, index, window, cx);
    }

    fn activate(&mut self, pane: usize, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.panes.get_mut(&pane).and_then(|pane_state| {
            pane_state.active = Some(index);
            pane_state.tabs.get(index)
        }) else {
            return;
        };
        tab.view.focus_handle(cx).focus(window);
        self.active_pane = pane;
        self.sync_tree_selection(cx);
        cx.notify();
    }

    /// Makes `pane` the one new files open in, without moving focus.
    fn focus_pane(&mut self, pane: usize, cx: &mut Context<Self>) {
        if self.active_pane != pane {
            self.active_pane = pane;
            self.sync_tree_selection(cx);
            cx.notify();
        }
    }

    fn sync_tree_selection(&self, cx: &mut Context<Self>) {
        let path = self.panes[&self.active_pane]
            .active_tab()
            .map(|tab| tab.path.clone());
        if let Some(tree) = &self.tree {
            tree.update(cx, |tree, cx| tree.select(path, cx));
        }
    }

    fn close_tab(
        &mut self,
        pane_id: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if index >= pane.tabs.len() {
            return;
        }
        pane.tabs.remove(index);
        if !pane.tabs.is_empty() {
            let active = pane.active.unwrap_or(0);
            let next = if active >= index {
                active.saturating_sub(1)
            } else {
                active
            };
            let next = next.min(pane.tabs.len() - 1);
            self.activate(pane_id, next, window, cx);
            return;
        }
        pane.active = None;
        if self.layout.remove(pane_id) {
            self.panes.remove(&pane_id);
        }
        let next_pane = self.layout.panes()[0];
        match self.panes[&next_pane].active {
            Some(index) => self.activate(next_pane, index, window, cx),
            None => {
                self.active_pane = next_pane;
                self.focus_handle.focus(window);
                self.sync_tree_selection(cx);
            }
        }
        cx.notify();
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.panes[&self.active_pane].active {
            self.close_tab(self.active_pane, index, window, cx);
        }
    }

    fn open_quick_open(&mut self, _: &OpenQuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let options = self.tree_options.clone();
        let quick_open = cx.new(|cx| QuickOpen::new(root, options, window, cx));
        let events = cx.subscribe_in(&quick_open, window, |this, _, event, window, cx| {
            this.close_quick_open(window, cx);
            match event {
                QuickOpenEvent::Open { path, line } => {
                    this.open_file(path.clone(), *line, window, cx)
                }
                QuickOpenEvent::GoToLine(line) => {
                    this.record_jump(cx);
                    if let Some(tab) = this.panes[&this.active_pane].active_tab() {
                        tab.view.clone().scroll_to_line(*line, cx);
                    }
                }
                QuickOpenEvent::Dismissed => {}
            }
        });
        self.quick_open = Some((quick_open, events));
        cx.notify();
    }

    fn search_project(&mut self, _: &SearchProject, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let search = match &self.search {
            Some((search, _)) => search.clone(),
            None => {
                let options = self.tree_options.clone();
                let search = cx.new(|cx| SearchPanel::new(root, options, cx));
                let events =
                    cx.subscribe_in(&search, window, |this, _, event, window, cx| match event {
                        SearchPanelEvent::Open { path, line } => {
                            this.open_file(path.clone(), Some(*line), window, cx)
                        }
                    });
                self.search = Some((search.clone(), events));
                search
            }
        };
        self.sidebar = Sidebar::Search;
        search.update(cx, |search, cx| search.focus(window, cx));
        cx.notify();
    }

    /// Where the active tab is, if there is one.
    fn place(&self, cx: &App) -> Option<Place> {
        let tab = self.panes[&self.active_pane].active_tab()?;
        Some(Place {
            path: tab.path.clone(),
            offset: tab.view.position(cx),
        })
    }

    /// Remembers the current place before a jump, for going back to it.
    fn record_jump(&mut self, cx: &App) {
        let Some(place) = self.place(cx) else {
            return;
        };
        if self.back.last() != Some(&place) {
            self.back.push(place);
            if self.back.len() > HISTORY_LEN {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(place) = self.back.pop() {
            self.forward.extend(self.place(cx));
            self.open(place.path, Some(Target::Offset(place.offset)), window, cx);
        }
    }

    fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(place) = self.forward.pop() {
            self.back.extend(self.place(cx));
            self.open(place.path, Some(Target::Offset(place.offset)), window, cx);
        }
    }

    fn show_files(&mut self, _: &ShowFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = Sidebar::Files;
        cx.notify();
    }

    fn render_sidebar(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let sidebar = div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .text_sm();
        let Some(tree) = &self.tree else {
            return sidebar
                .p_3()
                .text_color(theme.text_muted)
                .child("No folder open. Run `plastext <folder>` to open one.")
                .into_any_element();
        };
        let tab = |id: &'static str, label: &'static str, shown: bool| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .when(shown, |tab| tab.bg(theme.border).text_color(theme.text))
                .when(!shown, |tab| tab.text_color(theme.text_muted))
                .child(label)
        };
        let header = div()
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                tab("sidebar-files", "Files", self.sidebar == Sidebar::Files).on_click(
                    cx.listener(|this, _, window, cx| this.show_files(&ShowFiles, window, cx)),
                ),
            )
            .child(
                tab("sidebar-search", "Search", self.sidebar == Sidebar::Search).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.search_project(&SearchProject, window, cx)
                    }),
                ),
            );
        let content = match (&self.search, self.sidebar) {
            (Some((search, _)), Sidebar::Search) => search.clone().into_any_element(),
            _ => tree.clone().into_any_element(),
        };
        sidebar
            .child(header)
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes[&self.active_pane].active_tab().is_none() {
            return;
        }
        self.open_quick_open(&OpenQuickOpen, window, cx);
        if let Some((quick_open, _)) = &self.quick_open {
            quick_open.update(cx, |quick_open, cx| quick_open.set_query(":", cx));
        }
    }

    fn close_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_open.take().is_none() {
            return;
        }
        match self.panes[&self.active_pane].active_tab() {
            Some(tab) => tab.view.focus_handle(cx).focus(window),
            None => self.focus_handle.focus(window),
        }
        cx.notify();
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Horizontal, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Vertical, window, cx);
    }

    /// Adds a pane next to the active one, showing the active tab's file at
    /// the same position.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let new_pane = self.next_pane;
        self.next_pane += 1;
        if !self.layout.split(self.active_pane, axis, new_pane) {
            return;
        }
        let duplicate = self.panes[&self.active_pane]
            .active_tab()
            .map(|tab| (tab.path.clone(), tab.view.clone()))
            .map(|(path, view)| (path, view.duplicate(cx)));
        self.panes.insert(new_pane, Pane::default());
        self.active_pane = new_pane;
        match duplicate {
            Some((path, view)) => self.add_tab(path, view, window, cx),
            None => cx.notify(),
        }
    }

    fn render_layout(&self, layout: &Layout, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Pane(id) => self.render_pane(*id, theme, cx).into_any_element(),
            Layout::Split(axis, children) => div()
                .size_full()
                .flex()
                .when(*axis == Axis::Vertical, |split| split.flex_col())
                .children(children.iter().enumerate().map(|(i, child)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .border_color(theme.border)
                        .when(i > 0 && *axis == Axis::Horizontal, |child| {
                            child.border_l_1()
                        })
                        .when(i > 0 && *axis == Axis::Vertical, |child| child.border_t_1())
                        .child(self.render_layout(child, theme, cx))
                }))
                .into_any_element(),
        }
    }

    fn render_pane(&self, id: usize, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = &self.panes[&id];
        let content = match pane.active_tab() {
            Some(tab) => div().flex_1().min_h_0().child(tab.view.to_any()),
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
        div()
            .id(("pane", id))
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.focus_pane(id, cx)),
            )
            .when(!pane.tabs.is_empty(), |pane_element| {
                pane_element.child(self.render_tab_bar(id, theme, cx))
            })
            .child(content)
    }

    fn render_tab_bar(
        &self,
        pane_id: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane = &self.panes[&pane_id];
        let focused_pane = pane_id == self.active_pane;
        div()
            .id("tabs")
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .children(pane.tabs.iter().enumerate().map(|(index, tab)| {
                let active = pane.active == Some(index);
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
                    .when(active, |tab| tab.bg(theme.background))
                    .text_color(if active && focused_pane {
                        theme.text
                    } else {
                        theme.text_muted
                    })
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
                                this.close_tab(pane_id, index, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate(pane_id, index, window, cx)
                    }))
            }))
    }

    fn render_status_bar(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.panes[&self.active_pane]
            .active_tab()
            .map(|tab| tab.view.status(cx));
        let notice = self.notice.clone().map(|notice| {
            div()
                .id("notice")
                .cursor_pointer()
                .text_color(theme.text)
                .child(format!("{notice}  ×"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.notice = None;
                    cx.notify();
                }))
        });
        let view_message = status.as_ref().and_then(|status| status.message());
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
            .children(view_message)
            .child(div().flex_1())
            .children(status.map(|status| status.summary()))
    }
}

fn scroll_to(view: &TabView, target: Option<Target>, cx: &mut App) {
    match target {
        Some(Target::Line(line)) => view.scroll_to_line(line, cx),
        Some(Target::Offset(offset)) => view.scroll_to_offset(offset, cx),
        None => {}
    }
}

/// A file opened in the background, ready for a view.
enum Opened {
    Text(
        Arc<FileSource>,
        Arc<LineIndex>,
        Option<&'static plastext_syntax::Language>,
    ),
    Binary(Arc<FileSource>),
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let sidebar = self.render_sidebar(theme, cx);
        let editor_area =
            div()
                .flex_1()
                .min_w_0()
                .child(self.render_layout(&self.layout, theme, cx));

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::open_quick_open))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::search_project))
            .on_action(cx.listener(Self::show_files))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go_forward(&GoForward, window, cx)),
            )
            .relative()
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
            .children(self.quick_open.as_ref().map(|(quick_open, _)| {
                div()
                    .absolute()
                    .top(px(56.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(quick_open.clone())
            }))
    }
}
