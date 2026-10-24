use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Anchor(String),
    Local {
        path: PathBuf,
        fragment: Option<String>,
    },
    External(String),
    Blocked,
}

fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// Only explicit browser schemes can leave the app. Script/custom schemes
/// are never dispatched, whether they came from Markdown or safe HTML.
pub fn resolve_link(document: &Path, target: &str) -> LinkTarget {
    let target = target.trim();
    if target.chars().any(char::is_control) {
        return LinkTarget::Blocked;
    }
    if let Some(anchor) = target.strip_prefix('#') {
        return LinkTarget::Anchor(decode(anchor));
    }
    if target.starts_with("//") {
        return resolve_link(document, &format!("https:{target}"));
    }
    if let Ok(url) = Url::parse(target) {
        return match url.scheme() {
            "https" | "http" | "mailto" => LinkTarget::External(url.into()),
            "file" => match url.to_file_path() {
                Ok(path) => LinkTarget::Local {
                    path,
                    fragment: url.fragment().map(decode),
                },
                Err(_) => LinkTarget::Blocked,
            },
            _ => LinkTarget::Blocked,
        };
    }
    // Invalid scheme-looking URLs must not become local paths accidentally.
    if target
        .split('/')
        .next()
        .is_some_and(|part| part.contains(':'))
    {
        return LinkTarget::Blocked;
    }
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(p, f)| (p, Some(decode(f))));
    if path.is_empty() {
        return LinkTarget::Blocked;
    }
    LinkTarget::Local {
        path: document
            .parent()
            .unwrap_or(Path::new("."))
            .join(decode(path)),
        fragment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_paths_anchors_and_only_safe_schemes() {
        let doc = Path::new("/work/README.md");
        assert_eq!(
            resolve_link(doc, "./other%20doc.md#hello%20world"),
            LinkTarget::Local {
                path: "/work/./other doc.md".into(),
                fragment: Some("hello world".into())
            }
        );
        assert_eq!(
            resolve_link(doc, "#my-heading"),
            LinkTarget::Anchor("my-heading".into())
        );
        assert_eq!(
            resolve_link(doc, "src/lib.rs#L42"),
            LinkTarget::Local {
                path: "/work/src/lib.rs".into(),
                fragment: Some("L42".into())
            }
        );
        for target in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "vscode:run",
            "java\nscript:x",
            "https:bad\0url",
        ] {
            assert_eq!(resolve_link(doc, target), LinkTarget::Blocked);
        }
        assert!(matches!(
            resolve_link(doc, "https://example.com/a"),
            LinkTarget::External(_)
        ));
    }
}
