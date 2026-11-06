use super::*;
use crate::actions::{ClearRecentFolders, OpenFile, OpenFolder, OpenRecentFolder};
use gpui::{FontWeight, Global, img, svg};

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

    pub(super) fn render_welcome(
        &self,
        theme: Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let recent = cx
            .try_global::<RecentFolders>()
            .map(|history| history.0.as_slice())
            .unwrap_or_default();
        let header = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(img("app-icon.png").size(px(88.)).flex_none())
            .child(
                div()
                    .text_size(px(30.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("plastext"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .text_center()
                    .child("Open a folder or file to get started."),
            );
        let actions = div()
            .flex()
            .gap_3()
            .child(open_action(true, theme))
            .child(open_action(false, theme));
        let recent_folders = div()
            .flex()
            .flex_col()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .pt_5()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child("Recent folders"),
                    )
                    .child(
                        div()
                            .id("welcome-clear-recent")
                            .debug_selector(|| "welcome-clear-recent".into())
                            .text_xs()
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|button| button.text_color(theme.text))
                            .child("Clear Recent")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ClearRecentFolders), cx)
                            }),
                    ),
            )
            .child(
                div()
                    .id("welcome-recent-list")
                    .debug_selector(|| "welcome-recent-list".into())
                    .max_h(px(216.))
                    .overflow_y_scroll()
                    .children(
                        recent
                            .iter()
                            .enumerate()
                            .map(|(index, path)| recent_folder(index, path, theme)),
                    ),
            );
        let content = div()
            .debug_selector(|| "welcome-content".into())
            .w_full()
            .max_w(px(520.))
            .flex()
            .flex_col()
            .gap_6()
            .child(header)
            .child(actions)
            .when(!recent.is_empty(), |view| view.child(recent_folders));
        // Percentage min-height has no definite basis inside a scroll view.
        // Use the actual viewport so short content centers vertically, while
        // small windows can still scroll without clipping the header/actions.
        let height = (window.viewport_size().height
            - if self.notice.is_some() {
                px(STATUS_BAR_HEIGHT)
            } else {
                px(0.)
            })
        .max(px(0.));
        div()
            .id("welcome")
            .debug_selector(|| "welcome".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .min_h(height)
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(content),
            )
    }
}

fn open_action(folder: bool, theme: Theme) -> impl IntoElement {
    let id = if folder {
        "welcome-open-folder"
    } else {
        "welcome-open-file"
    };
    let shortcut = match (folder, cfg!(target_os = "macos")) {
        (true, true) => "⌥⌘O",
        (false, true) => "⌘O",
        (true, false) => "Ctrl+Alt+O",
        (false, false) => "Ctrl+O",
    };
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex_1()
        .min_w_0()
        .h(px(64.))
        .px_4()
        .flex()
        .items_center()
        .gap_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .cursor_pointer()
        .hover(|button| button.border_color(theme.syntax(plastext_syntax::Highlight::Link)))
        .child(
            svg()
                .path(if folder {
                    "file-icons/folder-base.svg"
                } else {
                    "file-icons/document.svg"
                })
                .size(px(22.))
                .flex_none()
                .text_color(theme.syntax(plastext_syntax::Highlight::Link)),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(if folder {
                            "Open Folder…"
                        } else {
                            "Open File…"
                        }),
                )
                .child(div().text_xs().text_color(theme.text_muted).child(shortcut)),
        )
        .on_click(move |_, window, cx| {
            if folder {
                window.dispatch_action(Box::new(OpenFolder), cx);
            } else {
                window.dispatch_action(Box::new(OpenFile), cx);
            }
        })
}

fn recent_folder(index: usize, path: &Path, theme: Theme) -> impl IntoElement {
    let path = path.to_owned();
    div()
        .id(("welcome-recent", index))
        .debug_selector(move || format!("welcome-recent-{index}"))
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .hover(|row| row.bg(theme.surface))
        .child(
            svg()
                .path("file-icons/folder-base.svg")
                .size(px(18.))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(
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
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(path.parent().unwrap_or(&path).display().to_string()),
                ),
        )
        .child(div().flex_none().text_color(theme.text_muted).child("›"))
        .on_click(move |_, window, cx| {
            window.dispatch_action(Box::new(OpenRecentFolder { path: path.clone() }), cx)
        })
}
