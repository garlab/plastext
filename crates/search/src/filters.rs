//! Search-only filters, layered over the project's ignore rules.
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

#[derive(Clone, Default)]
pub struct FileFilters {
    include: GlobSet,
    exclude: GlobSet,
}

impl FileFilters {
    pub fn new(include: &str, exclude: &str) -> Result<Self, String> {
        Ok(Self {
            include: compile(include).map_err(|err| format!("Files to include: {err}"))?,
            exclude: compile(exclude).map_err(|err| format!("Files to exclude: {err}"))?,
        })
    }

    pub fn matches(&self, path: &Path) -> bool {
        (self.include.is_empty() || self.include.is_match(path)) && !self.exclude.is_match(path)
    }
}

fn compile(text: &str) -> Result<GlobSet, String> {
    let mut set = GlobSetBuilder::new();
    // Commas inside brace alternatives/classes (or escaped commas) are not separators.
    let mut depth = 0usize;
    let mut class = false;
    let mut escaped = false;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' => class = true,
            ']' => class = false,
            '{' if !class => depth += 1,
            '}' if !class => depth = depth.saturating_sub(1),
            ',' if !class && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let rooted = part.starts_with("./") || part.starts_with('/');
        let part = part
            .strip_prefix("./")
            .unwrap_or(part)
            .trim_start_matches('/')
            .trim_end_matches('/');
        if part.is_empty() {
            return Err("Enter a file or directory glob.".into());
        }
        let pattern = if !rooted && !part.contains('/') {
            format!("**/{part}")
        } else {
            part.to_owned()
        };
        // A matched directory includes its descendants, as in editor search filters.
        for pattern in [pattern.clone(), format!("{pattern}/**")] {
            set.add(
                GlobBuilder::new(&pattern)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .build()
                    .map_err(|err| err.to_string())?,
            );
        }
    }
    set.build().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_style_patterns_and_exclusion_precedence() {
        let filters = FileFilters::new("src/**, *.{md,txt}", "**/generated/**, README.md").unwrap();
        for path in ["src/a.rs", "notes.md", "docs/a.txt"] {
            assert!(filters.matches(Path::new(path)), "{path}");
        }
        for path in [
            "other/a.rs",
            "src/generated/a.rs",
            "README.md",
            "docs/README.md",
        ] {
            assert!(!filters.matches(Path::new(path)), "{path}");
        }
        let filters = FileFilters::new("./src", "target").unwrap();
        assert!(filters.matches(Path::new("src/deep/a.rs")));
        assert!(!filters.matches(Path::new("nested/src/a.rs")));
        assert!(!filters.matches(Path::new("src/target/a.rs")));
        assert!(FileFilters::new("[", "").is_err());
        let filters = FileFilters::new(r"a\,b.txt, [x,y].rs", "").unwrap();
        assert!(filters.matches(Path::new("a,b.txt")));
        assert!(filters.matches(Path::new("x.rs")));
    }
}
