use super::*;
use copy_text::Format;
use gpui::{KeyDownEvent, Point, anchored, deferred};

pub(super) struct Menu {
    position: Point<Pixels>,
    focus: FocusHandle,
    selected: Option<usize>,
}

#[cfg(test)]
#[path = "context_menu_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
enum Command {
    Cut,
    Copy,
    Paste,
    Location,
    Lines,
    WithLocation,
}
const ITEMS: [(Command, &str, &str); 6] = [
    (Command::Cut, "Cut", "⌘X"),
    (Command::Copy, "Copy", "⌘C"),
    (Command::Paste, "Paste", "⌘V"),
    (Command::Location, "Copy Location", ""),
    (Command::Lines, "Copy Selected Line Range", ""),
    (Command::WithLocation, "Copy Text with Location", ""),
];

impl TextView {
    pub(super) fn open_context_menu(
        &mut self,
        mut hit: Hit,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside_selection = self.selection.is_some_and(|selection| {
            let range = selection.range();
            !range.is_empty() && range.start <= hit.offset && hit.offset <= range.end
        });
        if !inside_selection {
            // Opening the menu in the gutter selects the row, without folding it.
            hit.on_fold_toggle = false;
            self.mouse_down(hit, 1, false, cx);
        }
        self.selecting = false;
        self.drag = None;
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(Menu {
            position,
            focus,
            selected: None,
        });
        cx.notify();
    }

    fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu.take() {
            if menu.focus.is_focused(window) {
                self.focus_handle.focus(window);
            }
            cx.notify();
        }
    }

    fn context_menu_options(&self) -> [bool; 6] {
        let ready = !self.has_pending_input();
        let selection = self.selection.map(|selection| selection.range());
        let selected = selection.as_ref().is_some_and(|range| !range.is_empty());
        let bounded = selection
            .as_ref()
            .is_some_and(|range| range.end - range.start <= MAX_COPY);
        let named = self.file_name.is_some() && selection.is_some();
        [
            ready && self.is_editing() && selected && bounded,
            ready && selected && bounded,
            ready && self.is_editing(),
            ready && named,
            ready && named && selected,
            ready && named && bounded,
        ]
    }

    fn context_menu_command(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.is_none() || !self.context_menu_options()[index] {
            return;
        }
        self.dismiss_context_menu(window, cx);
        match ITEMS[index].0 {
            Command::Cut => self.edit_cut(&editing::Cut, window, cx),
            Command::Copy => self.copy(&Copy, window, cx),
            Command::Paste => self.edit_paste(&editing::Paste, window, cx),
            Command::Location => self.copy_text(Format::Location, cx),
            Command::Lines => self.copy_text(Format::Lines, cx),
            Command::WithLocation => self.copy_text(Format::WithLocation, cx),
        }
    }

    fn context_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.prevent_default();
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.platform {
            match key {
                "x" => self.context_menu_command(0, window, cx),
                "c" => self.context_menu_command(1, window, cx),
                "v" => self.context_menu_command(2, window, cx),
                _ => {}
            }
            return;
        }
        match key {
            "escape" => self.dismiss_context_menu(window, cx),
            "enter" | "space" => {
                if let Some(index) = self.context_menu.as_ref().and_then(|menu| menu.selected) {
                    self.context_menu_command(index, window, cx);
                }
            }
            "up" | "down" => {
                let enabled = self.context_menu_options();
                let Some(menu) = &mut self.context_menu else {
                    return;
                };
                let count = ITEMS.len();
                let down = key == "down";
                let current = menu.selected.unwrap_or(if down { count - 1 } else { 0 });
                menu.selected = (1..=count)
                    .map(|step| {
                        if down {
                            (current + step) % count
                        } else {
                            (current + count - step) % count
                        }
                    })
                    .find(|&index| enabled[index]);
                cx.notify();
            }
            _ => {}
        }
    }

    pub(super) fn render_context_menu(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        let enabled = self.context_menu_options();
        let selected = menu.selected;
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(4.))
                    .child(
                        div()
                            .id("editor-context-menu")
                            .track_focus(&menu.focus)
                            .occlude()
                            .w(px(272.))
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                this.dismiss_context_menu(window, cx)
                            }))
                            .on_key_down(cx.listener(Self::context_menu_key))
                            .children(ITEMS.iter().enumerate().map(
                                |(index, &(_, label, shortcut))| {
                                    div()
                                        .when(index == 3, |row| {
                                            row.mt_1()
                                                .pt_1()
                                                .border_t_1()
                                                .border_color(theme.border)
                                        })
                                        .child(
                                            div()
                                                .id(("editor-menu-item", index))
                                                .px_3()
                                                .py_1()
                                                .rounded_sm()
                                                .flex()
                                                .justify_between()
                                                .child(label)
                                                .child(
                                                    div()
                                                        .text_color(theme.text_muted)
                                                        .child(shortcut),
                                                )
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
                                                                this.context_menu_command(
                                                                    index, window, cx,
                                                                );
                                                            },
                                                        ))
                                                }),
                                        )
                                },
                            )),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
