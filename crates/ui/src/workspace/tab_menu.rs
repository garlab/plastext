use super::*;
use gpui::{KeyDownEvent, anchored};
use std::ops::Range;

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
}
const ITEMS: [(CloseTabs, &str); 3] = [
    (CloseTabs::Others, "Close Others"),
    (CloseTabs::Right, "Close Right"),
    (CloseTabs::Left, "Close Left"),
];

struct ClosePlan {
    keep: Range<usize>,
    active: usize,
}
impl CloseTabs {
    fn plan(self, count: usize, target: usize, active: Option<usize>) -> Option<ClosePlan> {
        if target >= count {
            return None;
        }
        let keep = match self {
            Self::Others => target..target + 1,
            Self::Right => 0..target + 1,
            Self::Left => target..count,
        };
        let active = active
            .filter(|index| keep.contains(index))
            .unwrap_or(target)
            - keep.start;
        Some(ClosePlan { keep, active })
    }
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

    fn tab_menu_options(&self) -> Option<[bool; 3]> {
        let menu = self.tab_menu.as_ref()?;
        let pane = self.panes.get(&menu.pane)?;
        let index = pane.tabs.iter().position(|tab| tab.path == menu.path)?;
        Some(ITEMS.map(|(action, _)| {
            action
                .plan(pane.tabs.len(), index, pane.active)
                .is_some_and(|plan| plan.keep.len() < pane.tabs.len())
        }))
    }

    fn close_tab_group(&mut self, action: CloseTabs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_menu.take() else {
            return;
        };
        // Resolve the target by path, so an asynchronous tab open cannot make
        // an old positional index act on a different tab.
        let Some(pane) = self.panes.get_mut(&menu.pane) else {
            return;
        };
        let Some(index) = pane.tabs.iter().position(|tab| tab.path == menu.path) else {
            return;
        };
        let Some(plan) = action.plan(pane.tabs.len(), index, pane.active) else {
            return;
        };
        let paths = pane
            .tabs
            .iter()
            .enumerate()
            .filter(|(index, _)| !plan.keep.contains(index))
            .map(|(_, tab)| tab.path.clone())
            .collect();
        let active = Some(pane.tabs[plan.keep.start + plan.active].path.clone());
        self.confirm_close(
            editing::CloseTarget::Tabs {
                pane: menu.pane,
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
                        .tab_menu_options()
                        .is_some_and(|enabled| enabled[index])
                {
                    self.close_tab_group(ITEMS[index].0, window, cx);
                }
            }
            "up" | "down" => {
                let Some(enabled) = self.tab_menu_options() else {
                    return;
                };
                let Some(menu) = &mut self.tab_menu else {
                    return;
                };
                let down = event.keystroke.key == "down";
                let current = menu.selected.unwrap_or(if down { 2 } else { 0 });
                menu.selected = (1..=3)
                    .map(|step| {
                        if down {
                            (current + step) % 3
                        } else {
                            (current + 3 - step) % 3
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
        let enabled = self.tab_menu_options()?;
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
                        .w(px(180.))
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

    #[test]
    fn closing_groups_keeps_the_clicked_tab_and_preserves_surviving_selection() {
        let right = CloseTabs::Right.plan(5, 2, Some(1)).unwrap();
        assert_eq!(right.keep, 0..3);
        assert_eq!(right.active, 1);
        let left = CloseTabs::Left.plan(5, 2, Some(4)).unwrap();
        assert_eq!(left.keep, 2..5);
        assert_eq!(left.active, 2);
        let others = CloseTabs::Others.plan(5, 2, Some(4)).unwrap();
        assert_eq!(others.keep, 2..3);
        assert_eq!(others.active, 0);
        assert_eq!(CloseTabs::Right.plan(5, 2, Some(4)).unwrap().active, 2);
        assert_eq!(CloseTabs::Left.plan(5, 2, Some(0)).unwrap().active, 0);
    }

    #[test]
    fn edge_tabs_disable_empty_operations_and_stale_indices_are_ignored() {
        assert_eq!(CloseTabs::Left.plan(3, 0, Some(0)).unwrap().keep.len(), 3);
        assert_eq!(CloseTabs::Right.plan(3, 2, Some(2)).unwrap().keep.len(), 3);
        for (action, _) in ITEMS {
            assert_eq!(action.plan(1, 0, None).unwrap().keep, 0..1);
            assert!(action.plan(0, 0, None).is_none());
            assert!(action.plan(2, 2, Some(1)).is_none());
        }
    }
}
