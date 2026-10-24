use gpui::{AnyView, App, Context, Entity, FocusHandle, Focusable, SharedString, Subscription};
use plastext_editor::{HexView, HexViewStatus, TextView, TextViewStatus};

/// What a tab shows: text, or a hex dump for binary files (spec TXT-9).
#[derive(Clone)]
pub(crate) enum TabView {
    Text(Entity<TextView>),
    Hex(Entity<HexView>),
}

pub(crate) enum TabStatus {
    Text(TextViewStatus),
    Hex(HexViewStatus),
}

impl TabView {
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Text(view) => view.focus_handle(cx),
            Self::Hex(view) => view.focus_handle(cx),
        }
    }

    /// Another view of the same file at the same position.
    pub fn duplicate(&self, cx: &mut App) -> Self {
        match self {
            Self::Text(view) => Self::Text(view.update(cx, |view, cx| view.duplicate(cx))),
            Self::Hex(view) => Self::Hex(view.update(cx, |view, cx| view.duplicate(cx))),
        }
    }

    /// Scrolls to 1-based `line`; hex views have no lines.
    pub fn scroll_to_line(&self, line: u64, cx: &mut App) {
        if let Self::Text(view) = self {
            view.update(cx, |view, cx| view.scroll_to_line(line, cx));
        }
    }

    /// The byte offset at the top of the view.
    pub fn position(&self, cx: &App) -> u64 {
        match self {
            Self::Text(view) => view.read(cx).position(),
            Self::Hex(view) => view.read(cx).position(),
        }
    }

    pub fn scroll_to_offset(&self, offset: u64, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |view, cx| view.scroll_to_offset(offset, cx)),
            Self::Hex(view) => view.update(cx, |view, cx| view.scroll_to_offset(offset, cx)),
        }
    }

    pub fn status(&self, cx: &App) -> TabStatus {
        match self {
            Self::Text(view) => TabStatus::Text(view.read(cx).status()),
            Self::Hex(view) => TabStatus::Hex(view.read(cx).status()),
        }
    }

    /// Re-renders the observer whenever the view changes.
    pub fn observe<T: 'static>(&self, cx: &mut Context<T>) -> Subscription {
        match self {
            Self::Text(view) => cx.observe(view, |_, _, cx| cx.notify()),
            Self::Hex(view) => cx.observe(view, |_, _, cx| cx.notify()),
        }
    }

    pub fn to_any(&self) -> AnyView {
        match self {
            Self::Text(view) => view.clone().into(),
            Self::Hex(view) => view.clone().into(),
        }
    }
}

impl TabStatus {
    /// A problem or passing message worth showing on its own.
    pub fn message(&self) -> Option<SharedString> {
        match self {
            Self::Text(status) => status.error.clone().or_else(|| status.notice.clone()),
            Self::Hex(status) => status.error.clone(),
        }
    }

    /// The status bar's right-hand side.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self {
            Self::Text(status) => {
                if let Some(selected) = status.selected {
                    parts.push(format!("{} selected", format_size(selected)));
                }
                if let Some(indexing) = status.indexing {
                    parts.push(format!("Indexing lines {:.0}%", indexing * 100.));
                }
                let lines = if status.indexing.is_some() {
                    format!("~{} lines", status.lines)
                } else {
                    format!("{} lines", status.lines)
                };
                match status.line {
                    Some(line) => parts.push(format!("Ln {line} of {lines}")),
                    None => parts.push(lines),
                }
                parts.push(format_size(status.len));
                parts.push(status.language.unwrap_or("Plain text").to_owned());
            }
            Self::Hex(status) => {
                parts.push(format!("Offset {:#x}", status.offset));
                parts.push(format_size(status.len));
                parts.push("Binary".to_owned());
            }
        }
        parts.join("    ")
    }
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000. && unit < UNITS.len() - 1 {
        size /= 1000.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn formats_sizes_in_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500), "1.5 KB");
        assert_eq!(format_size(10_000_000_000), "10.0 GB");
    }
}
