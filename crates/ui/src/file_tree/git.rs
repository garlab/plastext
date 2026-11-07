use super::*;
use plastext_git::FolderSummary;
use std::path::Path;

pub(super) struct Tooltip(Vec<SharedString>);

impl Tooltip {
    pub fn new(folder: &Path, summary: &FolderSummary) -> Self {
        let noun = if summary.changed == 1 {
            "file"
        } else {
            "files"
        };
        let mut heading = format!("Git: {} changed {noun}", summary.changed);
        if summary.deleted != 0 {
            heading.push_str(&format!(" ({} deleted)", summary.deleted));
        }
        let example = summary
            .example
            .strip_prefix(folder)
            .unwrap_or(&summary.example);
        let mut lines = vec![
            heading.into(),
            example.to_string_lossy().into_owned().into(),
        ];
        if summary.changed > 1 {
            lines.push(format!("and {} more", summary.changed - 1).into());
        }
        lines.push(
            "Includes hidden and deleted files; deleted files aren't listed in the tree.".into(),
        );
        Self(lines)
    }
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        div()
            .px_2()
            .py_1()
            .text_xs()
            .max_w(px(440.))
            .bg(theme.surface)
            .text_color(theme.text)
            .border_1()
            .border_color(theme.border)
            .rounded_sm()
            .children(self.0.iter().cloned().map(|line| div().child(line)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleted_hidden_file_explains_a_colored_folder_without_visible_children() {
        let folder = PathBuf::from("project");
        let summary = FolderSummary {
            status: Default::default(),
            changed: 1,
            deleted: 1,
            example: Arc::new(folder.join(".yarn/releases/yarn.cjs")),
        };
        let tooltip = Tooltip::new(&folder, &summary);
        assert_eq!(tooltip.0[0], "Git: 1 changed file (1 deleted)");
        assert_eq!(
            tooltip.0[1],
            Path::new(".yarn/releases/yarn.cjs")
                .to_string_lossy()
                .as_ref()
        );
    }
}
