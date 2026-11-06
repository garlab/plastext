use std::{
    cell::Cell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, DispatchPhase, Entity, FocusHandle,
    Focusable, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, Pixels, Point, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, actions, canvas, div, prelude::*, px, relative, uniform_list,
};
use plastext_buffer::{FileSource, LineIndex, OpenOptions};
use plastext_editor::{HexView, TextStyle, TextView, Theme};
use plastext_project::{ShowIgnored, TreeOptions};
use plastext_settings::{self as settings, Settings};

use crate::{
    csv_view::CsvView,
    file_tree::{FileTreeEvent, FileTreeView},
    icon_themes::{IconNotice, IconThemes},
    image_view::ImageView,
    layout::{Axis, Layout, Resize},
    markdown_view::{MarkdownEvent, MarkdownView},
    pdf_view::PdfView,
    picker::{Picker, PickerEvent, PickerItem},
    quick_open::{QuickOpen, QuickOpenEvent},
    search_panel::{SearchPanel, SearchPanelEvent},
    tab_view::TabView,
    tex_view::TexView,
    zoom::Zoom,
};

mod tab_menu;
use tab_menu::TabMenu;
mod file_actions;
mod file_menu;
use file_menu::FileMenu;
mod editing;
#[cfg(test)]
mod menu_tests;
mod opening;
pub(crate) mod welcome;
pub use editing::request_quit;
use opening::{Destination, OpenKind, Openings, Placement};

actions!(
    workspace,
    [
        CloseTab,
        CloseWindow,
        NewFile,
        SplitRight,
        SplitDown,
        OpenQuickOpen,
        GoToLine,
        SearchProject,
        ShowFiles,
        GoBack,
        GoForward,
        GoToSymbol,
        OpenCommandPalette,
        ToggleOutline,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        FileIconTheme,
        ImportIconTheme
    ]
);

/// How far back navigation history goes.
const HISTORY_LEN: usize = 100;

const KEY_CONTEXT: &str = "Workspace";
const SIDEBAR_WIDTH: f32 = 260.;
const OUTLINE_WIDTH: f32 = 240.;
const TAB_BAR_HEIGHT: f32 = 34.;
const STATUS_BAR_HEIGHT: f32 = 24.;

/// Registers the workspace's and its views' key bindings.
pub fn init(cx: &mut App) {
    plastext_editor::init(cx);
    crate::csv_view::init(cx);
    crate::markdown_view::init(cx);
    crate::pdf_view::init(cx);
    crate::image_view::init(cx);
    crate::record_view::init(cx);
    crate::tex_view::init(cx);
    crate::quick_open::init(cx);
    crate::picker::init(cx);
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewFile, context),
        KeyBinding::new("cmd-p", OpenQuickOpen, context),
        KeyBinding::new("ctrl-g", GoToLine, context),
        KeyBinding::new("cmd-shift-f", SearchProject, context),
        KeyBinding::new("cmd-shift-e", ShowFiles, context),
        KeyBinding::new("ctrl--", GoBack, context),
        KeyBinding::new("ctrl-shift--", GoForward, context),
        KeyBinding::new("cmd-[", GoBack, context),
        KeyBinding::new("cmd-]", GoForward, context),
        KeyBinding::new("cmd-shift-o", GoToSymbol, context),
        KeyBinding::new("cmd-shift-p", OpenCommandPalette, context),
        KeyBinding::new("cmd-alt-b", ToggleOutline, context),
        KeyBinding::new("alt-z", plastext_editor::actions::ToggleWordWrap, context),
        KeyBinding::new(
            "ctrl-cmd-space",
            plastext_editor::actions::ShowCharacterPalette,
            context,
        ),
        // Canonical modified bindings also supply native menu shortcut labels;
        // read-mode aliases such as `n` and `/` remain local to their views.
        KeyBinding::new("cmd-c", plastext_editor::actions::Copy, context),
        KeyBinding::new("cmd-x", plastext_editor::actions::Cut, context),
        KeyBinding::new("cmd-v", plastext_editor::actions::Paste, context),
        KeyBinding::new("cmd-a", plastext_editor::actions::SelectAll, context),
        KeyBinding::new("cmd-z", plastext_editor::actions::Undo, context),
        KeyBinding::new("cmd-shift-z", plastext_editor::actions::Redo, context),
        KeyBinding::new("cmd-f", plastext_editor::actions::Find, context),
        KeyBinding::new("cmd-g", plastext_editor::actions::FindNext, context),
        KeyBinding::new(
            "cmd-shift-g",
            plastext_editor::actions::FindPrevious,
            context,
        ),
        KeyBinding::new("cmd-shift-v", crate::actions::TogglePreview, context),
        // A read-mode key (spec §4.2), so only where the text view has focus.
        KeyBinding::new("o", ToggleOutline, Some("TextView")),
        KeyBinding::new("o", ToggleOutline, Some("MarkdownPreview")),
        KeyBinding::new("cmd-w", CloseTab, context),
        KeyBinding::new("cmd-shift-w", CloseWindow, context),
        KeyBinding::new("cmd-s", plastext_editor::actions::Save, context),
        KeyBinding::new("cmd-c", plastext_editor::actions::Copy, Some("FileTree")),
        KeyBinding::new("cmd-x", plastext_editor::actions::Cut, Some("FileTree")),
        KeyBinding::new("cmd-v", plastext_editor::actions::Paste, Some("FileTree")),
        KeyBinding::new("cmd-d", SplitRight, context),
        KeyBinding::new("cmd-shift-d", SplitDown, context),
        KeyBinding::new("cmd-+", ZoomIn, context),
        KeyBinding::new("cmd-=", ZoomIn, context),
        KeyBinding::new("cmd--", ZoomOut, context),
        KeyBinding::new("cmd-0", ResetZoom, context),
    ]);
}

/// A window's root view: one project with its file tree, panes of tabs and
/// status bar (spec §4.1, PRJ-1).
pub struct Workspace {
    root: Option<PathBuf>,
    tree: Option<Entity<FileTreeView>>,
    icon_themes: Entity<IconThemes>,
    panes: HashMap<usize, Pane>,
    layout: Layout,
    active_pane: usize,
    next_pane: usize,
    close_prompt: bool,
    /// The last problem worth telling the user about, shown in the status bar.
    notice: Option<SharedString>,
    tree_options: TreeOptions,
    sidebar: Sidebar,
    sidebar_width: Pixels,
    divider_drag: Option<DividerDrag>,
    search: Option<(Entity<SearchPanel>, Subscription)>,
    quick_open: Option<(Entity<QuickOpen>, Subscription)>,
    picker: Option<(Entity<Picker>, Subscription)>,
    tab_menu: Option<TabMenu>,
    file_menu: Option<FileMenu>,
    file_prompt: Option<file_actions::FilePrompt>,
    file_busy: bool,
    outline: Outline,
    open_options: OpenOptions,
    text_style: TextStyle,
    zoom: Zoom,
    markdown_settings: Arc<settings::Markdown>,
    tex_settings: settings::Tex,
    focus_handle: FocusHandle,
    /// Files in the order their tabs were last activated, most recent last.
    recent: Vec<PathBuf>,
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

/// The initial position or source mode of an opened file.
#[derive(Clone)]
enum Target {
    Line(u64),
    Offset(u64),
    Anchor(String),
    Source,
}

/// The outline panel on the right: the active file's symbols.
#[derive(Default)]
struct Outline {
    shown: bool,
    /// The symbol containing the top of the view, as last shown.
    current: Option<usize>,
    scroll_handle: UniformListScrollHandle,
}

/// What the sidebar shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Files,
    Search,
}

#[derive(Clone, Copy)]
struct SidebarDrag {
    start_x: Pixels,
    start_width: Pixels,
}

#[derive(Clone, Copy)]
enum DividerDrag {
    Sidebar(SidebarDrag),
    Pane { resize: Resize, start: Pixels },
}

impl DividerDrag {
    fn cursor(self) -> CursorStyle {
        match self {
            Self::Pane { resize, .. } if resize.axis == Axis::Vertical => CursorStyle::ResizeUpDown,
            _ => CursorStyle::ResizeLeftRight,
        }
    }
}

fn axis_position(axis: Axis, position: Point<Pixels>) -> Pixels {
    match axis {
        Axis::Horizontal => position.x,
        Axis::Vertical => position.y,
    }
}

impl SidebarDrag {
    fn width_at(self, x: Pixels, available: Pixels) -> Pixels {
        clamp_sidebar_width(self.start_width + x - self.start_x, available)
    }
}

/// Reserve room for the document, including in a narrow window.
fn clamp_sidebar_width(width: Pixels, available: Pixels) -> Pixels {
    let available = available.max(px(0.));
    let maximum = (available - px(320.).min(available / 2.)).min(px(600.));
    width.clamp(px(160.).min(maximum), maximum)
}

#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    active: Option<usize>,
    opening: Openings,
}

struct Tab {
    path: PathBuf,
    title: SharedString,
    view: TabView,
    preview: bool,
    _status: Subscription,
    _editing_status: Option<Subscription>,
    _links: Option<Subscription>,
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
        let mut subscriptions = vec![
            cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
            cx.observe_global::<welcome::RecentFolders>(|_, cx| cx.notify()),
        ];
        let icon_themes = IconThemes::shared(&settings.icons, cx);
        subscriptions.push(
            cx.subscribe(&icon_themes, |this, _, notice: &IconNotice, cx| {
                this.show_notice(notice.0.clone(), cx);
            }),
        );
        let files = &settings.files;
        let show_ignored = match files.show_ignored {
            settings::ShowIgnored::Hide => ShowIgnored::Hide,
            settings::ShowIgnored::Dim => ShowIgnored::Dim,
            settings::ShowIgnored::Show => ShowIgnored::Show,
        };
        let mut notice = icon_themes.read(cx).error.clone().map(Into::into);
        let tree_options = TreeOptions::new(files.show_hidden, show_ignored, &files.exclude)
            .unwrap_or_else(|err| {
                notice = Some(format!("Ignoring files.exclude in the settings: {err}").into());
                TreeOptions::default()
            });
        let tree = root.clone().map(|root| {
            let tree =
                cx.new(|cx| FileTreeView::new(root, tree_options.clone(), icon_themes.clone(), cx));
            subscriptions.push(
                cx.subscribe_in(&tree, window, |this, _, event, window, cx| match event {
                    FileTreeEvent::Open { path, preview } => {
                        this.record_jump(cx);
                        this.open_with_focus(
                            path.clone(),
                            None,
                            if *preview {
                                OpenKind::Preview
                            } else {
                                OpenKind::Keep
                            },
                            !preview,
                            window,
                            cx,
                        );
                    }
                    FileTreeEvent::Action(action) => this.file_action(*action, window, cx),
                    FileTreeEvent::ContextMenu { path, position } => {
                        this.open_file_menu(path.clone(), *position, window, cx);
                    }
                }),
            );
            tree
        });
        Self {
            root,
            tree,
            icon_themes,
            panes: HashMap::from([(0, Pane::default())]),
            layout: Layout::Pane(0),
            active_pane: 0,
            next_pane: 1,
            close_prompt: false,
            notice,
            tree_options,
            sidebar: Sidebar::Files,
            sidebar_width: px(SIDEBAR_WIDTH),
            divider_drag: None,
            search: None,
            quick_open: None,
            picker: None,
            tab_menu: None,
            file_menu: None,
            file_prompt: None,
            file_busy: false,
            outline: Outline::default(),
            open_options: OpenOptions {
                large_file_threshold: settings.limits.large_file_threshold.0,
                block_cache: settings.limits.block_cache.0,
            },
            text_style: TextStyle {
                font_family: settings.editor.font.clone().into(),
                font_size: px(settings.editor.font_size),
                line_height: settings.editor.line_height,
                tab_size: settings.editor.tab_size,
                sticky_scroll: settings.editor.sticky_scroll,
                max_parse_size: settings.limits.syntax_max_file_size.0,
            },
            markdown_settings: Arc::new(settings.markdown.clone()),
            tex_settings: settings.tex.clone(),
            zoom: Zoom::default(),
            focus_handle: cx.focus_handle(),
            recent: Vec::new(),
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
        self.open_with_kind(path, target, OpenKind::Keep, window, cx);
    }

    fn open_with_kind(
        &mut self,
        path: PathBuf,
        target: Option<Target>,
        kind: OpenKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_with_focus(path, target, kind, true, window, cx);
    }

    fn open_with_focus(
        &mut self,
        path: PathBuf,
        target: Option<Target>,
        kind: OpenKind,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.active_pane;
        if let Some(index) = self.panes[&pane]
            .tabs
            .iter()
            .position(|tab| tab.path == path)
        {
            let state = self.panes.get_mut(&pane).unwrap();
            state.opening.select();
            if kind == OpenKind::Keep {
                state.tabs[index].preview = false;
            }
            self.activate_with_focus(pane, index, focus_editor, window, cx);
            let view = self.panes[&pane].tabs[index].view.clone();
            scroll_to(&view, target, window, cx);
            return;
        }
        let pending = &mut self.panes.get_mut(&pane).unwrap().opening;
        // A newer line/anchor destination must not reuse the older callback's target.
        if target.is_some() {
            pending.cancel(&path);
        }
        let request = pending.begin(&path, kind);
        pending.set_focus(&path, focus_editor);
        let Some(request) = request else {
            return;
        };
        let options = self.open_options;
        let opening_started = Instant::now();
        let style = self.text_style.clone();
        let markdown_settings = self.markdown_settings.clone();
        let opening = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                if plastext_assets::is_image(&path) {
                    let source = Arc::new(FileSource::open(
                        &path,
                        OpenOptions {
                            large_file_threshold: 0,
                            block_cache: 4 * 1024 * 1024,
                        },
                    )?);
                    let lines = if path
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("svg"))
                    {
                        Some(LineIndex::for_source(source.clone())?)
                    } else {
                        None
                    };
                    return Ok(Opened::Image(source, lines));
                }
                if plastext_platform::pdf::is_pdf(&path, b"") {
                    // Keep even small PDFs file-backed; preview owns no full-file buffer.
                    let source = Arc::new(FileSource::open(
                        &path,
                        OpenOptions {
                            large_file_threshold: 0,
                            block_cache: 8 * 1024 * 1024,
                        },
                    )?);
                    return Ok(Opened::Pdf(source));
                }
                let record = crate::record_view::document::Kind::for_path(&path).is_some();
                let options = if record {
                    OpenOptions {
                        large_file_threshold: 0,
                        block_cache: 8 * 1024 * 1024,
                    }
                } else {
                    options
                };
                let source = Arc::new(FileSource::open(&path, options)?);
                let head = source.head(plastext_buffer::SNIFF_LEN)?;
                if plastext_platform::pdf::is_pdf(&path, &head) {
                    return Ok(Opened::Pdf(source));
                }
                if !record && plastext_buffer::looks_binary(&head) {
                    return Ok(Opened::Binary(source));
                }
                let index = LineIndex::for_source(source.clone())?;
                let language = plastext_syntax::detect(&path, &head[..head.len().min(256)]);
                std::io::Result::Ok(Opened::Text(source, index, language))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            this.update_in(cx, |this, window, cx| {
                let focus_editor = this
                    .panes
                    .get(&pane)
                    .is_some_and(|pane| pane.opening.focus_editor(&path));
                let Some((kind, activate)) = this
                    .panes
                    .get_mut(&pane)
                    .and_then(|pane| pane.opening.finish(&path, request))
                else {
                    return;
                };
                match opened {
                    Ok(opened) => {
                        let view = match opened {
                            Opened::Pdf(source) => TabView::Pdf(
                                cx.new(|cx| PdfView::new(path.clone(), source, style, cx)),
                            ),
                            Opened::Text(source, index, _)
                                if crate::record_view::document::Kind::for_path(&path)
                                    .is_some() =>
                            {
                                let kind =
                                    crate::record_view::document::Kind::for_path(&path).unwrap();
                                TabView::Record(cx.new(|cx| {
                                    crate::record_view::RecordView::new(
                                        path.clone(),
                                        kind,
                                        source,
                                        index,
                                        style,
                                        cx,
                                    )
                                }))
                            }
                            Opened::Text(source, index, _)
                                if path.extension().is_some_and(|s| {
                                    s.eq_ignore_ascii_case("csv") || s.eq_ignore_ascii_case("tsv")
                                }) =>
                            {
                                TabView::Csv(cx.new(|cx| {
                                    let mut view =
                                        CsvView::new(path.clone(), source, index, style, cx);
                                    if let Some(frames) = std::env::var("PLASTEXT_PROFILE_READING")
                                        .ok()
                                        .and_then(|v| v.parse::<usize>().ok())
                                    {
                                        view.profile_reading(opening_started, frames);
                                    }
                                    view
                                }))
                            }
                            Opened::Image(source, lines) => {
                                TabView::Image(cx.new(|cx| {
                                    ImageView::new(path.clone(), source, lines, style, cx)
                                }))
                            }
                            Opened::Text(source, index, _)
                                if plastext_markdown::is_markdown(&path) =>
                            {
                                TabView::Markdown(cx.new(|cx| {
                                    let mut view = MarkdownView::new(
                                        path.clone(),
                                        source,
                                        index,
                                        style,
                                        markdown_settings,
                                        cx,
                                    );
                                    if let Some(frames) = std::env::var("PLASTEXT_PROFILE_READING")
                                        .ok()
                                        .and_then(|v| v.parse::<usize>().ok())
                                    {
                                        view.profile_reading(opening_started, frames);
                                    }
                                    view
                                }))
                            }
                            Opened::Text(source, index, language)
                                if path
                                    .extension()
                                    .is_some_and(|s| s.eq_ignore_ascii_case("tex")) =>
                            {
                                TabView::Tex(cx.new(|cx| {
                                    TexView::new(
                                        path.clone(),
                                        source,
                                        index,
                                        style,
                                        language,
                                        (this.root.clone(), this.tex_settings.clone()),
                                        cx,
                                    )
                                }))
                            }
                            Opened::Text(source, index, language) => TabView::Text(cx.new(|cx| {
                                let mut view = TextView::new(source, index, style, language, cx);
                                if let Some(frames) = std::env::var("PLASTEXT_PROFILE_READING")
                                    .ok()
                                    .and_then(|value| value.parse::<usize>().ok())
                                {
                                    view.profile_reading(opening_started, frames.max(1));
                                }
                                view
                            })),
                            Opened::Binary(source) => {
                                TabView::Hex(cx.new(|cx| HexView::new(source, style, cx)))
                            }
                        };
                        scroll_to(&view, target, window, cx);
                        this.add_tab(
                            path,
                            view,
                            Destination {
                                pane,
                                kind,
                                activate,
                                focus_editor,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(err) => {
                        this.notice =
                            Some(format!("Couldn't open {}: {err}", path.display()).into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Adds a tab to its original pane. Replaced previews release their resources.
    fn add_tab(
        &mut self,
        path: PathBuf,
        view: TabView,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.panes.get(&destination.pane) else {
            return;
        };
        let placement = destination.kind.placement(
            &path,
            pane.tabs
                .iter()
                .map(|tab| (tab.path.as_path(), tab.preview && !tab.view.is_dirty(cx))),
        );
        if let Placement::Existing(index) = placement {
            if destination.kind == OpenKind::Keep {
                self.panes.get_mut(&destination.pane).unwrap().tabs[index].preview = false;
            }
            if destination.activate && self.active_pane == destination.pane {
                self.activate_with_focus(
                    destination.pane,
                    index,
                    destination.focus_editor,
                    window,
                    cx,
                );
            }
            cx.notify();
            return;
        }
        match &view {
            TabView::Text(text) => {
                let document =
                    plastext_editor::EditingDocument::shared(path.clone(), self.open_options, cx);
                text.update(cx, |view, cx| {
                    view.set_editing_document(document, cx);
                    view.edit_by_default(cx);
                });
            }
            TabView::Markdown(markdown) => {
                let document =
                    plastext_editor::EditingDocument::shared(path.clone(), self.open_options, cx);
                markdown.update(cx, |view, cx| view.set_editing_document(document, cx));
            }
            _ => {}
        }
        view.set_zoom(self.zoom.factor(), self.text_style.font_size, cx);
        let title = path
            .file_name()
            .map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
            .into();
        let status = view.observe(cx);
        let editing_status = view.editing_document(cx).map(|document| {
            cx.observe(&document, |this, _, cx| {
                for pane in this.panes.values_mut() {
                    for tab in &mut pane.tabs {
                        if tab.view.is_dirty(cx) {
                            tab.preview = false;
                        }
                    }
                }
                cx.notify();
            })
        });
        let links = if let TabView::Markdown(markdown) = &view {
            Some(
                cx.subscribe_in(markdown, window, |this, _, event, window, cx| {
                    let MarkdownEvent::Open { path, fragment } = event;
                    this.record_jump(cx);
                    this.open(
                        path.clone(),
                        fragment.clone().map(Target::Anchor),
                        window,
                        cx,
                    );
                }),
            )
        } else {
            None
        };
        let pane_id = destination.pane;
        let pane = self.panes.get_mut(&pane_id).unwrap();
        let tab = Tab {
            path,
            title,
            view,
            preview: destination.kind == OpenKind::Preview,
            _status: status,
            _editing_status: editing_status,
            _links: links,
        };
        let index = match placement {
            Placement::Replace(index) => {
                pane.tabs[index].view.set_visible(false, cx);
                pane.tabs[index] = tab;
                index
            }
            Placement::Append => {
                pane.tabs.push(tab);
                pane.tabs.len() - 1
            }
            Placement::Existing(_) => unreachable!(),
        };
        if destination.activate || pane.active.is_none() {
            pane.active = Some(index);
        }
        if destination.activate && self.active_pane == pane_id {
            self.activate_with_focus(pane_id, index, destination.focus_editor, window, cx);
        } else {
            cx.notify();
        }
    }

    fn activate(&mut self, pane: usize, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_with_focus(pane, index, true, window, cx);
    }
    fn activate_with_focus(
        &mut self,
        pane: usize,
        index: usize,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if focus_editor {
            self.tab_menu = None;
            self.file_menu = None;
        }
        let Some(tab) = self.panes.get_mut(&pane).and_then(|pane_state| {
            pane_state.active = Some(index);
            pane_state.tabs.get(index)
        }) else {
            return;
        };
        if focus_editor {
            tab.view.focus_handle(cx).focus(window);
        }
        let path = tab.path.clone();
        self.recent.retain(|recent| *recent != path);
        self.recent.push(path);
        if self.recent.len() > HISTORY_LEN {
            self.recent.remove(0);
        }
        self.active_pane = pane;
        if focus_editor {
            self.sync_tree_selection(cx);
        }
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

    fn close_tab_now(
        &mut self,
        pane_id: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_tab_menu(window, cx);
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if index >= pane.tabs.len() {
            return;
        }
        pane.opening.select();
        pane.tabs.remove(index).view.set_visible(false, cx);
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
        self.divider_drag = None;
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
        self.picker = None;
        // Most recent first, without the file already in front.
        let current = self.panes[&self.active_pane]
            .active_tab()
            .map(|tab| tab.path.clone());
        let recent: Vec<PathBuf> = self
            .recent
            .iter()
            .rev()
            .filter(|path| Some(*path) != current.as_ref())
            .cloned()
            .collect();
        let quick_open = cx.new(|cx| QuickOpen::new(root, options, &recent, window, cx));
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

    fn toggle_outline(&mut self, _: &ToggleOutline, _: &mut Window, cx: &mut Context<Self>) {
        self.outline.shown = !self.outline.shown;
        self.outline.current = None;
        cx.notify();
    }

    /// The outline panel (spec §4.1): the active file's symbols, following
    /// the scroll position; clicking one goes there.
    fn render_outline(&mut self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.outline.shown {
            return None;
        }
        let tab = self.panes[&self.active_pane].active_tab();
        let view = tab.map(|tab| tab.view.clone());
        let symbols = view.as_ref().and_then(|view| view.symbols(cx));
        let current = symbols
            .as_ref()
            .zip(view.as_ref())
            .and_then(|(symbols, view)| {
                let position = view.position(cx) as usize;
                symbols
                    .iter()
                    .rposition(|symbol| symbol.range.start <= position)
            });
        if current != self.outline.current {
            self.outline.current = current;
            if let Some(current) = current {
                self.outline
                    .scroll_handle
                    .scroll_to_item(current, ScrollStrategy::Center);
            }
        }
        let body = match symbols.zip(view) {
            Some((symbols, view)) => uniform_list(
                "outline",
                symbols.len(),
                cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|index| {
                            let symbol = &symbols[index];
                            let view = view.clone();
                            let offset = symbol.range.start as u64;
                            div()
                                .id(index)
                                .h(px(24.))
                                .pl(px(10. + 12. * symbol.depth as f32))
                                .pr_2()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .when(Some(index) == current, |row| row.bg(theme.selection))
                                .when(Some(index) != current, |row| {
                                    row.hover(|row| row.bg(theme.background))
                                })
                                .child(symbol.name.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.record_jump(cx);
                                    view.scroll_to_offset(offset, cx);
                                }))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.outline.scroll_handle.clone())
            .flex_1()
            .into_any_element(),
            None => div()
                .p_3()
                .text_color(theme.text_muted)
                .child("No outline for this file.")
                .into_any_element(),
        };
        Some(
            div()
                .w(px(OUTLINE_WIDTH))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .bg(theme.surface)
                .border_l_1()
                .border_color(theme.border)
                .text_sm()
                .child(
                    div()
                        .h(px(TAB_BAR_HEIGHT))
                        .flex_none()
                        .px_3()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child("Outline"),
                )
                .child(body)
                .into_any_element(),
        )
    }

    fn show_files(&mut self, _: &ShowFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = Sidebar::Files;
        if let Some(tree) = &self.tree {
            tree.focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    fn sidebar_available_width(&self, window: &Window) -> Pixels {
        window.viewport_size().width
            - if self.outline.shown {
                px(OUTLINE_WIDTH)
            } else {
                px(0.)
            }
    }

    fn divider_drag_overlay(&self, drag: DividerDrag, cx: &Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .cursor(drag.cursor())
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        // Window listeners keep the drag active beyond the divider's hitbox.
                        let moved = weak.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            moved
                                .update(cx, |this, cx| {
                                    if let Some(drag) = this.divider_drag {
                                        if event.dragging() {
                                            match drag {
                                                DividerDrag::Sidebar(drag) => {
                                                    this.sidebar_width = drag.width_at(
                                                        event.position.x,
                                                        this.sidebar_available_width(window),
                                                    );
                                                }
                                                DividerDrag::Pane { resize, start } => {
                                                    let delta =
                                                        axis_position(resize.axis, event.position)
                                                            - start;
                                                    this.layout.resize(resize, delta.into());
                                                }
                                            }
                                        } else {
                                            // Also recover if the mouse was released outside the window.
                                            this.divider_drag = None;
                                        }
                                        cx.notify();
                                    }
                                })
                                .ok();
                        });
                        let released = weak.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                                released
                                    .update(cx, |this, cx| {
                                        this.divider_drag = None;
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        });
                    },
                )
                .size_full(),
            )
    }

    fn render_sidebar(&self, width: Pixels, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let sidebar = div()
            .w(width)
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
                .gap_3()
                .text_color(theme.text_muted)
                .child("No folder open")
                .child(welcome::open_folder_button("sidebar-open-folder", theme))
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

    fn go_to_symbol(&mut self, _: &GoToSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.panes[&self.active_pane].active_tab() else {
            return;
        };
        let Some(symbols) = tab.view.symbols(cx) else {
            self.notice = Some("There are no symbols in this file (yet).".into());
            cx.notify();
            return;
        };
        let position = tab.view.position(cx) as usize;
        let current = symbols
            .iter()
            .rposition(|symbol| symbol.range.start <= position)
            .unwrap_or(0);
        let items = symbols
            .iter()
            .map(|symbol| PickerItem {
                label: symbol.name.clone().into(),
                detail: symbol.kind.clone().into(),
                depth: symbol.depth,
            })
            .collect();
        self.open_picker(
            "Go to symbol",
            items,
            current,
            window,
            cx,
            move |this, picked, _, cx| {
                this.record_jump(cx);
                if let Some(tab) = this.panes[&this.active_pane].active_tab() {
                    let offset = symbols[picked].range.start as u64;
                    tab.view.clone().scroll_to_offset(offset, cx);
                }
            },
        );
    }

    /// Choose a built-in or imported file-icon theme.
    fn file_icon_theme(&mut self, _: &FileIconTheme, window: &mut Window, cx: &mut Context<Self>) {
        let themes = self.icon_themes.read(cx);
        let mut choices = vec![
            ("material".to_owned(), "Material".to_owned()),
            ("none".to_owned(), "None".to_owned()),
        ];
        choices.extend(
            themes
                .preferences
                .imports
                .iter()
                .map(|entry| (entry.id(), entry.name.clone())),
        );
        let selected = choices
            .iter()
            .position(|(id, _)| id == &themes.active)
            .unwrap_or(0);
        let mut items: Vec<_> = choices
            .iter()
            .enumerate()
            .map(|(i, (_, name))| PickerItem {
                label: name.clone().into(),
                detail: if i == selected {
                    "Selected".into()
                } else if i > 1 {
                    "Imported".into()
                } else {
                    "".into()
                },
                depth: 0,
            })
            .collect();
        items.push(PickerItem {
            label: "Import Icon Theme…".into(),
            detail: "JSON file or extension folder".into(),
            depth: 0,
        });
        self.open_picker(
            "File Icon Theme",
            items,
            selected,
            window,
            cx,
            move |this, index, window, cx| {
                if let Some((id, _)) = choices.get(index) {
                    this.icon_themes
                        .update(cx, |themes, cx| themes.select(id.clone(), true, cx));
                } else {
                    this.import_icon_theme(&ImportIconTheme, window, cx);
                }
            },
        );
    }

    fn import_icon_theme(
        &mut self,
        _: &ImportIconTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Import Icon Theme".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                error => {
                    this.update(cx, |this, cx| {
                        this.show_notice(format!("Couldn't open icon theme picker: {error:?}"), cx)
                    })
                    .ok();
                    None
                }
            };
            let Some(path) = path else { return };
            let work = cx
                .background_executor()
                .spawn(async move { settings::icons::discover(&path) });
            let result = work.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(entries) => {
                    let only = (entries.len() == 1).then(|| entries[0].id());
                    this.icon_themes.update(cx, |themes, cx| {
                        themes.register(entries, cx);
                        if let Some(id) = only.as_ref() {
                            themes.select(id.clone(), true, cx);
                        }
                    });
                    if only.is_none() {
                        this.file_icon_theme(&FileIconTheme, window, cx);
                    }
                }
                Err(error) => this.show_notice(format!("Couldn't import icon theme: {error}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Lists the commands available where the focus is, with their shortcuts.
    fn open_command_palette(
        &mut self,
        _: &OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut actions = window.available_actions(cx);
        actions.sort_by_key(|action| command_name(action.name()));
        actions.dedup_by_key(|action| action.name());
        let items = actions
            .iter()
            .map(|action| {
                let shortcut = window
                    .bindings_for_action(action.as_ref())
                    .last()
                    .map(|binding| {
                        let keys: Vec<String> = binding
                            .keystrokes()
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        keys.join(" ")
                    })
                    .unwrap_or_default();
                PickerItem {
                    label: command_name(action.name()).into(),
                    detail: shortcut.into(),
                    depth: 0,
                }
            })
            .collect();
        self.open_picker(
            "Run a command",
            items,
            0,
            window,
            cx,
            move |_, picked, window, cx| {
                window.dispatch_action(actions[picked].boxed_clone(), cx);
            },
        );
    }

    /// Shows a picker over `items`; `picked` runs with the chosen index once
    /// the picker has closed and focus is back where it was.
    fn open_picker(
        &mut self,
        placeholder: &'static str,
        items: Vec<PickerItem>,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        picked: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.quick_open = None;
        let picker = cx.new(|cx| Picker::new(placeholder, items, selected, window, cx));
        let events = cx.subscribe_in(&picker, window, move |this, _, event, window, cx| {
            this.close_quick_open(window, cx);
            if let PickerEvent::Picked(index) = event {
                picked(this, *index, window, cx);
            }
        });
        self.picker = Some((picker, events));
        cx.notify();
    }

    /// Closes Quick Open or the picker, whichever is open.
    fn close_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closed_picker = self.picker.take().is_some();
        if self.quick_open.take().is_none() && !closed_picker {
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
        self.divider_drag = None;
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
            Some((path, view)) => self.add_tab(
                path,
                view,
                Destination {
                    pane: new_pane,
                    kind: OpenKind::Keep,
                    activate: true,
                    focus_editor: true,
                },
                window,
                cx,
            ),
            None => cx.notify(),
        }
    }

    fn render_layout(&self, layout: &Layout, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Pane(id) => self.render_pane(*id, theme, cx).into_any_element(),
            Layout::Split(axis, children) => {
                let axis = *axis;
                let total: f32 = children.iter().map(|child| child.weight).sum();
                let extent = Rc::new(Cell::new(px(0.)));
                let measured = extent.clone();
                let mut split = div().relative().size_full().child(
                    canvas(
                        move |bounds, _, _| {
                            measured.set(match axis {
                                Axis::Horizontal => bounds.size.width,
                                Axis::Vertical => bounds.size.height,
                            })
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
                let mut offset = 0.;
                let mut dividers = Vec::new();
                for (i, child) in children.iter().enumerate() {
                    let fraction = child.weight / total;
                    if i > 0 {
                        dividers.push((child.layout.first_pane(), offset));
                    }
                    split = split.child(
                        div()
                            .absolute()
                            .min_w_0()
                            .min_h_0()
                            .when(axis == Axis::Horizontal, |pane| {
                                pane.left(relative(offset))
                                    .top_0()
                                    .w(relative(fraction))
                                    .h_full()
                            })
                            .when(axis == Axis::Vertical, |pane| {
                                pane.top(relative(offset))
                                    .left_0()
                                    .h(relative(fraction))
                                    .w_full()
                            })
                            .border_color(theme.border)
                            .when(i > 0 && axis == Axis::Horizontal, |pane| pane.border_l_1())
                            .when(i > 0 && axis == Axis::Vertical, |pane| pane.border_t_1())
                            .child(self.render_layout(&child.layout, theme, cx)),
                    );
                    offset += fraction;
                }
                // Paint the hit areas last so both sides of each divider are draggable.
                split
                    .children(dividers.into_iter().map(|(pane_after, offset)| {
                        let extent = extent.clone();
                        div()
                            .id(("pane-divider", pane_after))
                            .absolute()
                            .occlude()
                            .when(axis == Axis::Horizontal, |divider| {
                                divider
                                    .left(relative(offset))
                                    .ml(px(-4.))
                                    .top_0()
                                    .bottom_0()
                                    .w(px(8.))
                                    .cursor(CursorStyle::ResizeLeftRight)
                            })
                            .when(axis == Axis::Vertical, |divider| {
                                divider
                                    .top(relative(offset))
                                    .mt(px(-4.))
                                    .left_0()
                                    .right_0()
                                    .h(px(8.))
                                    .cursor(CursorStyle::ResizeUpDown)
                            })
                            .hover(|divider| divider.bg(theme.border))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if let Some(resize) =
                                        this.layout.begin_resize(pane_after, extent.get().into())
                                    {
                                        this.divider_drag = Some(DividerDrag::Pane {
                                            resize,
                                            start: axis_position(axis, event.position),
                                        });
                                        cx.stop_propagation();
                                        cx.notify();
                                    }
                                }),
                            )
                    }))
                    .into_any_element()
            }
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
                    .child(
                        div()
                            .when(tab.preview && !tab.view.is_dirty(cx), |title| {
                                title.italic()
                            })
                            .child(if tab.view.is_dirty(cx) {
                                format!("{} ●", tab.title).into()
                            } else {
                                tab.title.clone()
                            }),
                    )
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
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if let Some(pane) = this.panes.get_mut(&pane_id) {
                            pane.opening.select();
                            if event.click_count() >= 2 {
                                pane.tabs[index].preview = false;
                            }
                        }
                        this.activate(pane_id, index, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();
                            this.open_tab_menu(pane_id, index, event.position, window, cx);
                        }),
                    )
            }))
    }

    fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if self.zoom == zoom {
            return;
        }
        self.zoom = zoom;
        for pane in self.panes.values() {
            for tab in &pane.tabs {
                tab.view
                    .set_zoom(zoom.factor(), self.text_style.font_size, cx);
            }
        }
        cx.notify();
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
            .child(if status.as_ref().is_some_and(|status| matches!(status, crate::tab_view::TabStatus::Text(status) if status.editing)) { "EDIT" } else { "READ" })
            .children(notice)
            .children(view_message)
            .child(div().flex_1())
            .when(self.zoom != Zoom::default(), |d| {
                d.child(format!("{}%", self.zoom.percent()))
            })
            .children(status.map(|status| status.summary()))
    }
}

/// A readable name for an action: `workspace::OpenQuickOpen` becomes
/// "Workspace: Open Quick Open".
fn command_name(action: &str) -> String {
    let (namespace, name) = action.rsplit_once("::").unwrap_or(("", action));
    let mut words = String::new();
    for (i, ch) in name.char_indices() {
        if i > 0 && ch.is_uppercase() {
            words.push(' ');
        }
        words.push(ch);
    }
    let namespace: Vec<String> = namespace
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .collect();
    if namespace.is_empty() {
        words
    } else {
        format!("{}: {words}", namespace.join(" "))
    }
}

fn scroll_to(view: &TabView, target: Option<Target>, window: &mut Window, cx: &mut App) {
    match target {
        Some(Target::Line(line)) => view.scroll_to_line(line, cx),
        Some(Target::Offset(offset)) => view.scroll_to_offset(offset, cx),
        Some(Target::Anchor(anchor)) => view.scroll_to_anchor(anchor, cx),
        Some(Target::Source) => {
            if let TabView::Markdown(view) = view {
                view.update(cx, |view, cx| view.show_source(window, cx));
            }
        }
        None => {}
    }
}

/// A file opened in the background, ready for a view.
enum Opened {
    Pdf(Arc<FileSource>),
    Image(Arc<FileSource>, Option<Arc<LineIndex>>),
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
        for pane in self.panes.values() {
            for (index, tab) in pane.tabs.iter().enumerate() {
                tab.view.set_visible(pane.active == Some(index), cx);
            }
        }
        let theme = Theme::get(window, cx);
        let sidebar_width =
            clamp_sidebar_width(self.sidebar_width, self.sidebar_available_width(window));
        let sidebar = self.render_sidebar(sidebar_width, theme, cx);
        let editor_area =
            div()
                .flex_1()
                .min_w_0()
                .child(self.render_layout(&self.layout, theme, cx));
        let outline = self.render_outline(theme, cx);
        let welcome = self.is_welcome();

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .when(
                self.panes[&self.active_pane].active_tab().is_some(),
                |view| view.on_action(cx.listener(Self::close_active_tab)),
            )
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.confirm_close(editing::CloseTarget::Window, window, cx);
            }))
            .when(
                self.panes[&self.active_pane]
                    .active_tab()
                    .and_then(|tab| tab.view.editing_document(cx))
                    .is_some_and(|doc| doc.read(cx).is_editing() || doc.read(cx).is_dirty()),
                |view| {
                    view.on_action(cx.listener(
                        |this, _: &plastext_editor::actions::Save, _, cx| {
                            if let Some(document) = this.panes[&this.active_pane]
                                .active_tab()
                                .and_then(|tab| tab.view.editing_document(cx))
                            {
                                if document.read(cx).has_pending_input() {
                                    this.show_notice("Finish the pending edit before saving.", cx);
                                } else {
                                    document.update(cx, |doc, cx| doc.save(cx));
                                }
                            }
                        },
                    ))
                },
            )
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::open_quick_open))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::search_project))
            .on_action(cx.listener(Self::show_files))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::open_command_palette))
            .on_action(cx.listener(Self::file_icon_theme))
            .on_action(cx.listener(Self::import_icon_theme))
            .on_action(cx.listener(Self::toggle_outline))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.set_zoom(this.zoom.step(1), cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.set_zoom(this.zoom.step(-1), cx);
            }))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| {
                this.set_zoom(Zoom::default(), cx);
            }))
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
            .when(welcome, |view| view.child(self.render_welcome(theme, cx)))
            .when(!welcome, |view| {
                view.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(sidebar)
                        .child(editor_area)
                        .children(outline)
                        .child(
                            div()
                                .id("sidebar-divider")
                                .absolute()
                                .left(sidebar_width - px(4.))
                                .top_0()
                                .bottom_0()
                                .w(px(8.))
                                .occlude()
                                .cursor(CursorStyle::ResizeLeftRight)
                                .hover(|divider| divider.bg(theme.border))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.divider_drag =
                                            Some(DividerDrag::Sidebar(SidebarDrag {
                                                start_x: event.position.x,
                                                start_width: sidebar_width,
                                            }));
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
            })
            .when(!welcome || self.notice.is_some(), |view| {
                view.child(self.render_status_bar(theme, cx))
            })
            .when_some(self.divider_drag, |d, drag| {
                d.child(self.divider_drag_overlay(drag, cx))
            })
            .children(self.picker.as_ref().map(|(picker, _)| {
                div()
                    .absolute()
                    .top(px(56.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(picker.clone())
            }))
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
            .children(self.render_tab_menu(theme, cx))
            .children(self.render_file_menu(theme, cx))
            .children(self.render_file_prompt(theme, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{SidebarDrag, clamp_sidebar_width, command_name};
    use gpui::px;

    #[test]
    fn sidebar_drag_preserves_grab_offset_and_document_space() {
        let drag = SidebarDrag {
            start_x: px(263.),
            start_width: px(260.),
        };
        assert_eq!(drag.width_at(px(303.), px(1000.)), px(300.));
        assert_eq!(drag.width_at(px(-100.), px(1000.)), px(160.));
        assert_eq!(drag.width_at(px(1200.), px(1000.)), px(600.));
        assert_eq!(drag.width_at(px(900.), px(800.)), px(480.));
        assert_eq!(clamp_sidebar_width(px(260.), px(400.)), px(200.));
        assert_eq!(clamp_sidebar_width(px(260.), px(200.)), px(100.));
        assert_eq!(clamp_sidebar_width(px(260.), px(-10.)), px(0.));
    }

    #[test]
    fn names_commands_readably() {
        assert_eq!(
            command_name("workspace::OpenQuickOpen"),
            "Workspace: Open Quick Open"
        );
        assert_eq!(command_name("text_view::Top"), "Text View: Top");
        assert_eq!(command_name("Quit"), "Quit");
    }
}
