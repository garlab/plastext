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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ClipboardItem, TestAppContext};

    #[gpui::test]
    fn dirty_markdown_preview_is_pinned_and_uses_close_confirmation(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.md");
        std::fs::write(&path, "# Original\n").unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|window, cx| {
            let mut workspace = Workspace::new(None, Arc::default(), window, cx);
            workspace.open_with_kind(path.clone(), None, OpenKind::Preview, window, cx);
            workspace
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "cmd-shift-v");
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("# Unsaved\n".into())));
        cx.simulate_keystrokes(window.into(), "cmd-a cmd-v");
        window.update(cx, |workspace, _, cx| {
            let tab = workspace.panes[&workspace.active_pane].active_tab().unwrap();
            assert!(tab.view.is_dirty(cx));
            assert!(!tab.preview);
            assert!(matches!(tab.view.status(cx), crate::tab_view::TabStatus::Text(status) if status.editing));
        }).unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-shift-v cmd-w");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        window
            .update(cx, |workspace, _, cx| {
                let tab = workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap();
                assert!(tab.view.is_dirty(cx));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.simulate_prompt_answer("Discard");
        cx.run_until_parked();
        window
            .update(cx, |workspace, window, _| {
                assert!(workspace.panes[&workspace.active_pane].tabs.is_empty());
                window.remove_window();
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# Original\n");
    }

    #[gpui::test]
    fn text_opens_editable_and_binary_remains_read_only(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.rs");
        let binary = dir.path().join("data.bin");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        std::fs::write(&binary, [0, 1, 2, 0, 255]).unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|window, cx| {
            let mut workspace = Workspace::new(None, Arc::default(), window, cx);
            workspace.open_with_kind(path.clone(), None, OpenKind::Preview, window, cx);
            workspace
        });
        cx.run_until_parked();
        window
            .update(cx, |workspace, _, cx| {
                let tab = workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap();
                assert!(
                    tab.preview,
                    "opening in edit mode must not pin a preview tab"
                );
                assert!(!tab.view.is_dirty(cx));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape e");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(5));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
        window.update(cx, |workspace, _, cx| {
            let tab = workspace.panes[&workspace.active_pane].active_tab().unwrap();
            assert!(tab.view.is_dirty(cx));
            assert!(!tab.preview);
            assert!(matches!(tab.view.status(cx), crate::tab_view::TabStatus::Text(status) if status.editing));
        }).unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-s");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "efn main() {}\n");
        // Opening an existing file at a line places the editing caret there too.
        window
            .update(cx, |workspace, window, cx| {
                workspace.open_with_kind(
                    path.clone(),
                    Some(Target::Line(2)),
                    OpenKind::Keep,
                    window,
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "x cmd-s");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "efn main() {}\nx");
        window
            .update(cx, |workspace, window, cx| {
                workspace.open_with_kind(binary.clone(), None, OpenKind::Keep, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "e enter cmd-s");
        window
            .update(cx, |workspace, window, cx| {
                let tab = workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap();
                assert!(matches!(tab.view, TabView::Hex(_)));
                assert!(tab.view.editing_document(cx).is_none());
                window.remove_window();
            })
            .unwrap();
        assert_eq!(std::fs::read(&binary).unwrap(), [0, 1, 2, 0, 255]);
    }
}
