use super::*;
use plastext_editor::EditingDocument;
use std::time::Duration;

#[derive(Clone)]
pub(super) enum CloseTarget {
    Tabs {
        pane: usize,
        paths: Vec<PathBuf>,
        active: Option<PathBuf>,
    },
    Window,
    Quit,
}

pub fn request_quit(cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<Workspace>())
    {
        window
            .update(cx, |workspace, window, cx| {
                workspace.confirm_close(CloseTarget::Quit, window, cx)
            })
            .ok();
    } else {
        quit(cx);
    }
}
fn quit(cx: &mut App) {
    #[cfg(target_os = "macos")]
    plastext_platform::quit::allow();
    cx.quit();
}

impl Workspace {
    pub fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.confirm_close(CloseTarget::Window, window, cx);
        false
    }
    pub(super) fn close_tab(
        &mut self,
        pane: usize,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.panes.get(&pane).and_then(|pane| pane.tabs.get(index)) else {
            return;
        };
        self.confirm_close(
            CloseTarget::Tabs {
                pane,
                paths: vec![tab.path.clone()],
                active: None,
            },
            window,
            cx,
        );
    }
    fn close_documents(&self, target: &CloseTarget, cx: &App) -> Vec<Entity<EditingDocument>> {
        let mut documents = match target {
            CloseTarget::Quit => EditingDocument::all(cx),
            CloseTarget::Window => self
                .panes
                .values()
                .flat_map(|pane| &pane.tabs)
                .filter_map(|tab| tab.view.editing_document(cx))
                .collect(),
            CloseTarget::Tabs { pane, paths, .. } => self
                .panes
                .get(pane)
                .into_iter()
                .flat_map(|pane| &pane.tabs)
                .filter(|tab| paths.contains(&tab.path))
                .filter_map(|tab| tab.view.editing_document(cx))
                .filter(|doc| doc.read(cx).view_count() <= 1 || doc.read(cx).has_pending_input())
                .collect(),
        };
        documents.sort_by_key(Entity::entity_id);
        documents.dedup_by_key(|entity| entity.entity_id());
        documents
    }
    pub(super) fn confirm_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_prompt {
            return;
        }
        let documents = self.close_documents(&target, cx);
        if documents
            .iter()
            .any(|doc| doc.read(cx).has_pending_input() || doc.read(cx).is_saving())
        {
            self.show_notice("Finish the pending edit or save before closing.", cx);
            return;
        }
        let dirty: Vec<_> = documents
            .into_iter()
            .filter(|doc| doc.read(cx).is_dirty())
            .filter_map(|doc| Some((doc.clone(), doc.read(cx).version()?)))
            .collect();
        if dirty.is_empty() {
            self.finish_close(target, window, cx);
            return;
        }
        self.close_prompt = true;
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Save changes before closing?",
            Some("Discarding will lose the unsaved changes."),
            &["Save", "Cancel", "Discard"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(1);
            if answer == 0 {
                for (document, _) in &dirty {
                    document.update(cx, |doc, cx| doc.save(cx)).ok();
                }
                loop {
                    let saving = cx
                        .update(|_, cx| dirty.iter().any(|(doc, _)| doc.read(cx).is_saving()))
                        .unwrap_or(false);
                    if !saving {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.close_prompt = false;
                let safe = if answer == 0 {
                    this.close_documents(&target, cx)
                        .iter()
                        .all(|doc| !doc.read(cx).is_dirty() && !doc.read(cx).is_saving())
                } else if answer == 2 {
                    dirty.iter().all(|(doc, version)| {
                        doc.read(cx).is_current(version)
                            && !doc.read(cx).has_pending_input()
                            && !doc.read(cx).is_saving()
                    }) && this
                        .close_documents(&target, cx)
                        .iter()
                        .filter(|doc| doc.read(cx).is_dirty())
                        .all(|doc| dirty.iter().any(|(old, _)| old == doc))
                } else {
                    false
                };
                if safe {
                    this.finish_close(target, window, cx);
                } else if answer != 1 {
                    this.show_notice(
                        "The document changed or could not be saved. It remains open.",
                        cx,
                    );
                }
            })
            .ok();
            cx.background_executor()
                .spawn(async move {
                    drop(dirty);
                })
                .detach();
        })
        .detach();
    }
    fn finish_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            CloseTarget::Quit => quit(cx),
            CloseTarget::Window => window.remove_window(),
            CloseTarget::Tabs {
                pane,
                paths,
                active,
            } => {
                for path in paths.into_iter().rev() {
                    if let Some(index) = self
                        .panes
                        .get(&pane)
                        .and_then(|pane| pane.tabs.iter().position(|tab| tab.path == path))
                    {
                        self.close_tab_now(pane, index, window, cx);
                    }
                }
                if let Some(path) = active
                    && let Some(index) = self
                        .panes
                        .get(&pane)
                        .and_then(|pane| pane.tabs.iter().position(|tab| tab.path == path))
                {
                    self.activate(pane, index, window, cx);
                }
            }
        }
    }
}
