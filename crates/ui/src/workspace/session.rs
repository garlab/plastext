//! Session snapshots are collected only on close. Restored inactive tabs are metadata.
use super::*;
use crate::tab_view::session::DeferredView;
use settings::session::{Session, SessionChild, SessionLayout, SessionPane, SessionTab};

/// Configured by the app. Tests and benchmark windows default to no storage.
pub struct SessionStorage(pub Option<PathBuf>);
impl gpui::Global for SessionStorage {}

impl Workspace {
    pub(crate) fn session_snapshot(&self, cx: &App) -> Option<(PathBuf, Session)> {
        let root = self.root.clone()?;
        let ids = self.layout.panes();
        fn layout(value: &Layout, ids: &[usize]) -> SessionLayout {
            match value {
                Layout::Pane(id) => SessionLayout::Pane(ids.iter().position(|p| p == id).unwrap()),
                Layout::Split(axis, children) => SessionLayout::Split {
                    vertical: *axis == Axis::Vertical,
                    children: children
                        .iter()
                        .map(|c| SessionChild {
                            layout: layout(&c.layout, ids),
                            weight: c.weight,
                        })
                        .collect(),
                },
            }
        }
        let panes = ids
            .iter()
            .map(|id| {
                let pane = &self.panes[id];
                let mut active = None;
                let tabs: Vec<_> = pane
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, tab)| {
                        let document = tab.view.editing_document(cx);
                        if document.as_ref().is_some_and(|d| d.read(cx).is_untitled()) {
                            return None;
                        }
                        if pane.active == Some(index) {
                            active = Some(index);
                        }
                        Some((
                            index,
                            SessionTab {
                                path: document.as_ref().map_or_else(
                                    || tab.path.clone(),
                                    |d| d.read(cx).path().to_owned(),
                                ),
                                preview: tab.preview && !tab.view.is_dirty(cx),
                                view: tab.view.session_state(cx),
                            },
                        ))
                    })
                    .collect();
                let active = active
                    .and_then(|old| tabs.iter().position(|(i, _)| *i == old))
                    .or_else(|| (!tabs.is_empty()).then_some(0));
                SessionPane {
                    tabs: tabs.into_iter().map(|(_, tab)| tab).collect(),
                    active,
                }
            })
            .collect();
        Some((
            root,
            Session {
                layout: layout(&self.layout, &ids),
                panes,
                active_pane: ids
                    .iter()
                    .position(|id| *id == self.active_pane)
                    .unwrap_or(0),
                sidebar_width: self.sidebar_width.into(),
                zoom: self.zoom.percent(),
                sync_scroll: self.scroll_sync.enabled,
            },
        ))
    }

    pub fn restore_session(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if session.validate().is_err()
            || self
                .panes
                .values()
                .any(|p| !p.tabs.is_empty() || p.opening.is_pending())
        {
            return;
        }
        fn layout(value: SessionLayout) -> Layout {
            match value {
                SessionLayout::Pane(id) => Layout::Pane(id),
                SessionLayout::Split { vertical, children } => Layout::Split(
                    if vertical {
                        Axis::Vertical
                    } else {
                        Axis::Horizontal
                    },
                    children
                        .into_iter()
                        .map(|c| crate::layout::Child {
                            layout: layout(c.layout),
                            weight: c.weight,
                        })
                        .collect(),
                ),
            }
        }
        self.layout = layout(session.layout);
        self.active_pane = session.active_pane;
        self.next_pane = session.panes.len();
        self.panes = (0..self.next_pane)
            .map(|id| (id, Pane::default()))
            .collect();
        self.sidebar_width = px(session.sidebar_width);
        self.zoom = Zoom::from_percent(session.zoom);
        self.scroll_sync.enabled = session.sync_scroll;
        for (pane, saved) in session.panes.into_iter().enumerate() {
            for tab in saved.tabs {
                let view = TabView::Deferred(cx.new(|cx| DeferredView::new(tab.view, cx)));
                self.add_tab(
                    tab.path,
                    view,
                    Destination {
                        pane,
                        kind: if tab.preview {
                            OpenKind::Preview
                        } else {
                            OpenKind::Keep
                        },
                        activate: false,
                        focus_editor: false,
                    },
                    window,
                    cx,
                );
            }
            self.panes.get_mut(&pane).unwrap().active = saved.active;
        }
        // All headers and splits exist before any file is opened.
        for pane in self.layout.panes() {
            if let Some(index) = self.panes[&pane].active {
                self.load_session_tab(pane, index, pane == self.active_pane, window, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn load_session_tab(
        &mut self,
        pane: usize,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.panes.get(&pane).and_then(|p| p.tabs.get(index)) else {
            return;
        };
        let TabView::Deferred(view) = &tab.view else {
            return;
        };
        if self.panes[&pane].opening.contains(&tab.path) {
            return;
        }
        let state = view.read(cx).state.clone();
        let path = tab.path.clone();
        let kind = if tab.preview {
            OpenKind::Preview
        } else {
            OpenKind::Keep
        };
        view.update(cx, |v, cx| {
            v.error = None;
            cx.notify();
        });
        self.open_in(
            pane,
            path,
            Some(Target::Session(state)),
            kind,
            focus,
            window,
            cx,
        );
    }
}

/// Serializes close-time writes across windows and rejects repeated close requests.
struct Closing;
impl gpui::Global for Closing {}
pub(super) fn closing(cx: &App) -> bool {
    cx.try_global::<Closing>().is_some()
}

impl Workspace {
    pub(super) fn save_session_and_close(
        &mut self,
        all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if closing(cx) {
            return;
        }
        let path = cx.try_global::<SessionStorage>().and_then(|s| s.0.clone());
        let Some(path) = path else {
            if all {
                super::editing::quit(cx);
            } else {
                window.remove_window();
            }
            return;
        };
        let windows: Vec<_> = cx
            .windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Workspace>())
            .collect();
        let current = window.window_handle().window_id();
        let mut sessions = Vec::new();
        if let Some(saved) = self.session_snapshot(cx) {
            sessions.push(saved);
        }
        if all {
            for handle in &windows {
                if handle.window_id() != current
                    && let Ok(other) = handle.read(cx)
                    && let Some(saved) = other.session_snapshot(cx)
                {
                    sessions.push(saved);
                }
            }
        }
        if sessions.is_empty() {
            if all {
                super::editing::quit(cx);
            } else {
                window.remove_window();
            }
            return;
        }
        // Discard was already approved. Preserve that decision only while the
        // documents remain unchanged during the background metadata write.
        let target = if all {
            super::editing::CloseTarget::Quit
        } else {
            super::editing::CloseTarget::Window
        };
        let documents: Vec<_> = self
            .close_documents(&target, cx)
            .into_iter()
            .map(|d| {
                let version = d.read(cx).version();
                (d, version)
            })
            .collect();
        cx.set_global(Closing);
        let save = cx
            .background_executor()
            .spawn(async move { settings::session::Sessions::save(&path, sessions) });
        cx.spawn_in(window, async move |this, cx| {
            let result = save.await;
            let mut accepted = true;
            if let Err(error) = result {
                let answer = cx.update(|window, cx| {
                    window.prompt(
                        gpui::PromptLevel::Warning,
                        "Couldn't save the folder session",
                        Some(&format!("{error}\nThe previous saved layout is unchanged.")),
                        &["Close Without Saving Layout", "Cancel"],
                        cx,
                    )
                });
                accepted = match answer {
                    Ok(answer) => answer.await == Ok(0),
                    Err(_) => false,
                };
            }
            // Release the guard even if the originating window disappeared.
            gpui::AsyncApp::update(cx, |cx| cx.remove_global::<Closing>()).ok();
            this.update_in(cx, |this, window, cx| {
                if !accepted {
                    return;
                }
                let same_windows = !all
                    || cx
                        .windows()
                        .into_iter()
                        .filter_map(|w| w.downcast::<Workspace>())
                        .collect::<Vec<_>>()
                        == windows;
                let unchanged = documents.iter().all(|(doc, version)| {
                    let doc = doc.read(cx);
                    !doc.has_pending_input()
                        && !doc.is_saving()
                        && !doc.file_operation_pending()
                        && (!doc.is_dirty() || version.as_ref().is_some_and(|v| doc.is_current(v)))
                });
                let known = this.close_documents(&target, cx).iter().all(|doc| {
                    !doc.read(cx).is_dirty() || documents.iter().any(|(old, _)| old == doc)
                });
                if !same_windows || !unchanged || !known {
                    this.show_notice(
                        "The workspace changed while closing. Close again when ready.",
                        cx,
                    );
                    return;
                }
                if all {
                    super::editing::quit(cx);
                } else {
                    window.remove_window();
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
