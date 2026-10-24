use std::path::{Path, PathBuf};

use gpui::{Context, SharedString, Subscription, Window, div, prelude::*, px};

use crate::theme::Theme;

/// A window's root view: one project (spec PRJ-1), laid out as in spec §4.1.
///
/// The sidebar, editor area and status bar are placeholders for now.
pub struct Workspace {
    root: Option<PathBuf>,
    _appearance: Subscription,
}

impl Workspace {
    pub fn new(root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        Self {
            root,
            _appearance: appearance,
        }
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
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let message = match &self.root {
            Some(root) => root.display().to_string(),
            None => "No folder open. Run `plastext <folder>` to open one.".to_owned(),
        };

        let sidebar = div()
            .w(px(240.))
            .p_2()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .text_color(theme.text_muted)
            .child("Files");
        let editor_area = div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_muted)
            .child(message);
        let status_bar = div()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.text_muted)
            .child("READ");

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .text_sm()
            .child(div().flex_1().flex().child(sidebar).child(editor_area))
            .child(status_bar)
    }
}
