use super::*;
use crate::actions::{ClearRecentFolders, OpenFile, OpenFolder, OpenRecentFolder};
use gpui::{FontWeight, Global};

/// Presentation state; persistence and folder opening belong to the app.
#[derive(Default)]
pub struct RecentFolders(pub Vec<PathBuf>);
impl Global for RecentFolders {}

pub(super) fn open_folder_button(id: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_4()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .bg(theme.syntax(plastext_syntax::Highlight::Link))
        .text_color(theme.background)
        .font_weight(FontWeight::SEMIBOLD)
        .text_sm()
        .hover(|button| button.opacity(0.85))
        .child("Open Folder…")
        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenFolder), cx))
}

impl Workspace {
    pub fn is_welcome(&self) -> bool {
        self.root.is_none() && self.panes.values().all(|pane| pane.tabs.is_empty())
    }

    /// A welcome window can be reused only when no file or dialog is in flight.
    pub fn can_open_folder_in_place(&self) -> bool {
        self.is_welcome()
            && self.panes.values().all(|pane| !pane.opening.is_pending())
            && self.file_prompt.is_none()
            && !self.file_busy
    }

    pub(super) fn render_welcome(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let recent = cx
            .try_global::<RecentFolders>()
            .map(|history| history.0.as_slice())
            .unwrap_or_default();
        div()
            .id("welcome")
            .debug_selector(|| "welcome".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p_8()
                    .child(
                        div()
                            .debug_selector(|| "welcome-content".into())
                            .w_full()
                            .max_w(px(560.))
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(34.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("plastext"),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.text_muted)
                                            .text_sm()
                                            .child("Open a folder or file to get started."),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(open_folder_button("welcome-open-folder", theme))
                                    .child(
                                        div()
                                            .id("welcome-open-file")
                                            .debug_selector(|| "welcome-open-file".into())
                                            .px_4()
                                            .py_2()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .border_1()
                                            .border_color(theme.border)
                                            .text_sm()
                                            .hover(|button| button.bg(theme.surface))
                                            .child("Open File…")
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(Box::new(OpenFile), cx)
                                            }),
                                    ),
                            )
                            .when(!recent.is_empty(), |view| {
                                view.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .justify_between()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child("Recent folders"),
                                                )
                                                .child(
                                                    div()
                                                        .id("welcome-clear-recent")
                                                        .debug_selector(|| {
                                                            "welcome-clear-recent".into()
                                                        })
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .cursor_pointer()
                                                        .hover(|button| {
                                                            button.text_color(theme.text)
                                                        })
                                                        .child("Clear Recent")
                                                        .on_click(|_, window, cx| {
                                                            window.dispatch_action(
                                                                Box::new(ClearRecentFolders),
                                                                cx,
                                                            )
                                                        }),
                                                ),
                                        )
                                        .children(recent.iter().enumerate().map(
                                            |(index, path)| {
                                                let path = path.clone();
                                                div()
                                                    .id(("welcome-recent", index))
                                                    .debug_selector(move || {
                                                        format!("welcome-recent-{index}")
                                                    })
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .px_3()
                                                    .py_2()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|row| row.bg(theme.surface))
                                                    .child(
                                                        div().text_sm().text_ellipsis().child(
                                                            path.file_name()
                                                                .unwrap_or(path.as_os_str())
                                                                .to_string_lossy()
                                                                .into_owned(),
                                                        ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(theme.text_muted)
                                                            .text_ellipsis()
                                                            .child(path.display().to_string()),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        window.dispatch_action(
                                                            Box::new(OpenRecentFolder {
                                                                path: path.clone(),
                                                            }),
                                                            cx,
                                                        )
                                                    })
                                            },
                                        )),
                                )
                            }),
                    ),
            )
    }
}
