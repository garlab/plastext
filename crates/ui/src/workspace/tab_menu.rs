use super::*;
use gpui::{KeyDownEvent, anchored};

pub(super) struct TabMenu {
    pane: usize,
    path: PathBuf,
    position: Point<Pixels>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    selected: Option<usize>,
}

#[derive(Clone, Copy)]
enum CloseTabs {
    Others,
    Right,
    Left,
    Saved,
}
const ITEMS: [(CloseTabs, &str); 4] = [
    (CloseTabs::Others, "Close Others"),
    (CloseTabs::Right, "Close Right"),
    (CloseTabs::Left, "Close Left"),
    (CloseTabs::Saved, "Close Saved Files"),
];

struct ClosePlan {
    close: Vec<usize>,
    active: Option<usize>,
}
impl CloseTabs {
    fn plan(
        self,
        count: usize,
        target: usize,
        active: Option<usize>,
        saved: impl Fn(usize) -> bool,
    ) -> Option<ClosePlan> {
        if target >= count {
            return None;
        }
        let (close, keep): (Vec<_>, Vec<_>) = (0..count).partition(|&index| match self {
            Self::Others => index != target,
            Self::Right => index > target,
            Self::Left => index < target,
            Self::Saved => saved(index),
        });
        let active = active.filter(|index| keep.contains(index)).or_else(|| {
            keep.into_iter()
                .min_by_key(|index| index.abs_diff(active.unwrap_or(target)))
        });
        Some(ClosePlan { close, active })
    }
}

fn saved(tab: &Tab, cx: &App) -> bool {
    tab.view
        .editing_document(cx)
        .is_none_or(|doc| doc.read(cx).can_close_as_saved())
}

impl Workspace {
    pub(super) fn open_tab_menu(
        &mut self,
        pane: usize,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_file_menu(window, cx);
        self.dismiss_tab_menu(window, cx);
        let Some(tab) = self.panes.get(&pane).and_then(|pane| pane.tabs.get(index)) else {
            return;
        };
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.tab_menu = Some(TabMenu {
            pane,
            path: tab.path.clone(),
            position,
            focus,
            previous_focus,
            selected: None,
        });
        cx.notify();
    }

    pub(super) fn dismiss_tab_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.tab_menu.take() {
            if menu.focus.is_focused(window)
                && let Some(previous) = menu.previous_focus
            {
                previous.focus(window);
            }
            cx.notify();
        }
    }

    fn tab_menu_options(&self, cx: &App) -> Option<[bool; 4]> {
        let menu = self.tab_menu.as_ref()?;
        let pane = self.panes.get(&menu.pane)?;
        let index = pane.tabs.iter().position(|tab| tab.path == menu.path)?;
        Some(ITEMS.map(|(action, _)| {
            action
                .plan(pane.tabs.len(), index, pane.active, |index| {
                    saved(&pane.tabs[index], cx)
                })
                .is_some_and(|plan| !plan.close.is_empty())
        }))
    }

    fn close_tab_group(&mut self, action: CloseTabs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_menu.as_ref() else {
            return;
        };
        let pane_id = menu.pane;
        let path = menu.path.clone();
        self.dismiss_tab_menu(window, cx);
        // Resolve the target by path, so an asynchronous tab open cannot make
        // an old positional index act on a different tab.
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let Some(index) = pane.tabs.iter().position(|tab| tab.path == path) else {
            return;
        };
        let Some(plan) = action.plan(pane.tabs.len(), index, pane.active, |index| {
            saved(&pane.tabs[index], cx)
        }) else {
            return;
        };
        let paths = plan
            .close
            .iter()
            .map(|&index| pane.tabs[index].path.clone())
            .collect();
        let active = plan.active.map(|index| pane.tabs[index].path.clone());
        if plan.close.is_empty() {
            return;
        }
        self.confirm_close(
            editing::CloseTarget::Tabs {
                pane: pane_id,
                paths,
                active,
            },
            window,
            cx,
        );
    }

    fn tab_menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.prevent_default();
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_tab_menu(window, cx),
            "enter" | "space" => {
                if let Some(index) = self.tab_menu.as_ref().and_then(|menu| menu.selected)
                    && self
                        .tab_menu_options(cx)
                        .is_some_and(|enabled| enabled[index])
                {
                    self.close_tab_group(ITEMS[index].0, window, cx);
                }
            }
            "up" | "down" => {
                let Some(enabled) = self.tab_menu_options(cx) else {
                    return;
                };
                let Some(menu) = &mut self.tab_menu else {
                    return;
                };
                let down = event.keystroke.key == "down";
                let current = menu
                    .selected
                    .unwrap_or(if down { ITEMS.len() - 1 } else { 0 });
                menu.selected = (1..=ITEMS.len())
                    .map(|step| {
                        if down {
                            (current + step) % ITEMS.len()
                        } else {
                            (current + ITEMS.len() - step) % ITEMS.len()
                        }
                    })
                    .find(|index| enabled[*index]);
                cx.notify();
            }
            _ => {}
        }
    }

    pub(super) fn render_tab_menu(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.tab_menu.as_ref()?;
        let enabled = self.tab_menu_options(cx)?;
        let selected = menu.selected;
        Some(
            anchored()
                .position(menu.position)
                .snap_to_window_with_margin(px(4.))
                .child(
                    div()
                        .id("tab-context-menu")
                        .track_focus(&menu.focus)
                        .occlude()
                        .w(px(200.))
                        .p_1()
                        .flex()
                        .flex_col()
                        .bg(theme.surface)
                        .text_color(theme.text)
                        .text_sm()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_md()
                        .shadow_lg()
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| this.dismiss_tab_menu(window, cx)),
                        )
                        .on_key_down(cx.listener(Self::tab_menu_key))
                        .children(
                            ITEMS
                                .into_iter()
                                .enumerate()
                                .map(|(index, (action, label))| {
                                    div()
                                        .id(("tab-menu-item", index))
                                        .debug_selector(move || format!("tab-menu-item-{index}"))
                                        .px_3()
                                        .py_1()
                                        .rounded_sm()
                                        .child(label)
                                        .when(!enabled[index], |item| {
                                            item.text_color(theme.text_muted).opacity(0.5)
                                        })
                                        .when(enabled[index], |item| {
                                            item.cursor_pointer()
                                                .when(selected == Some(index), |item| {
                                                    item.bg(theme.border)
                                                })
                                                .hover(|item| item.bg(theme.border))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.close_tab_group(action, window, cx);
                                                    },
                                                ))
                                        })
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ClipboardItem, TestAppContext, VisualTestContext};

    #[gpui::test]
    fn close_saved_preserves_drafts_pending_files_and_other_panes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let paths = ["a.txt", "b.txt", "dirty.txt"].map(|name| dir.path().join(name));
        for path in &paths {
            std::fs::write(path, "original").unwrap();
        }
        cx.update(crate::init);
        let window = cx.add_window(|window, cx| {
            Workspace::new(Some(dir.path().to_owned()), Arc::default(), window, cx)
        });
        for path in &paths {
            window
                .update(cx, |workspace, window, cx| {
                    workspace.open_file(path.clone(), None, window, cx)
                })
                .unwrap();
            cx.run_until_parked();
        }
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("unsaved edits".into())));
        cx.simulate_keystrokes(window.into(), "cmd-v");
        window
            .update(cx, |workspace, window, cx| {
                workspace.activate(0, 0, window, cx)
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-d");
        window
            .update(cx, |workspace, window, cx| {
                workspace.activate(0, 2, window, cx)
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-n");
        window
            .update(cx, |workspace, window, cx| {
                workspace.open_tab_menu(0, 0, gpui::point(px(300.), px(50.)), window, cx);
                assert!(workspace.tab_menu_options(cx).unwrap()[3]);
                let document = workspace.panes[&0].tabs[1]
                    .view
                    .editing_document(cx)
                    .unwrap();
                // The menu was opened before the operation started. Evaluate again on click.
                document.update(cx, |doc, cx| doc.set_file_operation_pending(true, cx));
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        let bounds = visual.debug_bounds("tab-menu-item-3").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        assert!(!cx.has_pending_prompt());
        window
            .update(cx, |workspace, window, cx| {
                let pane = &workspace.panes[&0];
                assert_eq!(pane.tabs.len(), 3);
                assert_eq!(pane.tabs[0].path, paths[1]);
                assert_eq!(pane.tabs[1].path, paths[2]);
                assert!(pane.tabs[1].view.is_dirty(cx));
                assert!(
                    pane.tabs[2]
                        .view
                        .editing_document(cx)
                        .unwrap()
                        .read(cx)
                        .is_untitled()
                );
                assert_eq!(pane.active, Some(2));
                let other = workspace.panes.iter().find(|(id, _)| **id != 0).unwrap().1;
                assert_eq!(other.tabs.len(), 1);
                assert_eq!(other.tabs[0].path, paths[0]);
                workspace.open_tab_menu(0, 1, gpui::point(px(300.), px(50.)), window, cx);
                assert!(!workspace.tab_menu_options(cx).unwrap()[3]);
                let document = workspace.panes[&0].tabs[0]
                    .view
                    .editing_document(cx)
                    .unwrap();
                document.update(cx, |doc, cx| doc.set_file_operation_pending(false, cx));
                assert!(workspace.tab_menu_options(cx).unwrap()[3]);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "up enter");
        window
            .update(cx, |workspace, window, cx| {
                assert_eq!(workspace.panes[&0].tabs.len(), 2);
                workspace.open_tab_menu(0, 0, gpui::point(px(300.), px(50.)), window, cx);
                assert!(!workspace.tab_menu_options(cx).unwrap()[3]);
                window.remove_window();
            })
            .unwrap();
    }

    #[gpui::test]
    fn close_saved_can_remove_every_tab_in_a_split(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.txt");
        std::fs::write(&path, "saved").unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|window, cx| {
            let mut workspace = Workspace::new(None, Arc::default(), window, cx);
            workspace.open_file(path.clone(), None, window, cx);
            workspace
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "cmd-d");
        window
            .update(cx, |workspace, window, cx| {
                assert_eq!(workspace.panes.len(), 2);
                workspace.open_tab_menu(
                    workspace.active_pane,
                    0,
                    gpui::point(px(300.), px(50.)),
                    window,
                    cx,
                );
                assert_eq!(
                    workspace.tab_menu_options(cx).unwrap(),
                    [false, false, false, true]
                );
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "up enter");
        assert!(!cx.has_pending_prompt());
        window
            .update(cx, |workspace, window, cx| {
                assert_eq!(workspace.panes.len(), 1);
                let tab = workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap();
                assert_eq!(tab.path, path);
                assert!(tab.view.focus_handle(cx).is_focused(window));
                window.remove_window();
            })
            .unwrap();
    }

    #[test]
    fn closing_groups_preserve_surviving_selection() {
        let right = CloseTabs::Right.plan(5, 2, Some(1), |_| false).unwrap();
        assert_eq!(right.close, [3, 4]);
        assert_eq!(right.active, Some(1));
        let left = CloseTabs::Left.plan(5, 2, Some(4), |_| false).unwrap();
        assert_eq!(left.close, [0, 1]);
        assert_eq!(left.active, Some(4));
        let others = CloseTabs::Others.plan(5, 2, Some(4), |_| false).unwrap();
        assert_eq!(others.close, [0, 1, 3, 4]);
        assert_eq!(others.active, Some(2));
        assert_eq!(
            CloseTabs::Right
                .plan(5, 2, Some(4), |_| false)
                .unwrap()
                .active,
            Some(2)
        );
        assert_eq!(
            CloseTabs::Left
                .plan(5, 2, Some(0), |_| false)
                .unwrap()
                .active,
            Some(2)
        );
    }

    #[test]
    fn edge_tabs_disable_empty_operations_and_stale_indices_are_ignored() {
        assert!(
            CloseTabs::Left
                .plan(3, 0, Some(0), |_| false)
                .unwrap()
                .close
                .is_empty()
        );
        assert!(
            CloseTabs::Right
                .plan(3, 2, Some(2), |_| false)
                .unwrap()
                .close
                .is_empty()
        );
        for (action, _) in ITEMS {
            assert!(action.plan(1, 0, None, |_| false).unwrap().close.is_empty());
            assert!(action.plan(0, 0, None, |_| false).is_none());
            assert!(action.plan(2, 2, Some(1), |_| false).is_none());
        }
    }

    #[test]
    fn saved_files_can_be_disjoint_or_include_every_tab() {
        let plan = CloseTabs::Saved
            .plan(5, 2, Some(4), |index| [0, 2, 3].contains(&index))
            .unwrap();
        assert_eq!(plan.close, [0, 2, 3]);
        assert_eq!(plan.active, Some(4));
        let plan = CloseTabs::Saved
            .plan(4, 1, Some(1), |index| index < 2)
            .unwrap();
        assert_eq!(plan.active, Some(2));
        let plan = CloseTabs::Saved.plan(3, 1, Some(2), |_| true).unwrap();
        assert_eq!(plan.close, [0, 1, 2]);
        assert_eq!(plan.active, None);
    }
}
