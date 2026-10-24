use std::path::PathBuf;

use plastext_cli::Location;

/// A window to open: a project folder, and files to open in it.
#[derive(Debug, PartialEq)]
pub struct WindowRequest {
    pub root: Option<PathBuf>,
    pub files: Vec<Location>,
}

/// Groups command-line locations into windows (spec CLI-1). Each folder gets
/// a window. Each file opens in the window of the deepest folder containing
/// it, or else in a window for its parent folder. Without locations, there's
/// one empty window. Blocks: it checks which paths are folders.
pub fn window_requests(locations: Vec<Location>) -> Vec<WindowRequest> {
    let (folders, files): (Vec<_>, Vec<_>) = locations
        .into_iter()
        .partition(|location| location.path.is_dir());
    let mut requests: Vec<WindowRequest> = Vec::new();
    for folder in folders {
        if !requests
            .iter()
            .any(|request| request.root.as_ref() == Some(&folder.path))
        {
            requests.push(WindowRequest {
                root: Some(folder.path),
                files: Vec::new(),
            });
        }
    }
    for file in files {
        let containing = requests
            .iter_mut()
            .filter(|request| {
                request
                    .root
                    .as_ref()
                    .is_some_and(|root| file.path.starts_with(root))
            })
            .max_by_key(|request| {
                request
                    .root
                    .as_ref()
                    .map_or(0, |root| root.components().count())
            });
        match containing {
            Some(request) => request.files.push(file),
            None => requests.push(WindowRequest {
                root: file.path.parent().map(ToOwned::to_owned),
                files: vec![file],
            }),
        }
    }
    if requests.is_empty() {
        requests.push(WindowRequest {
            root: None,
            files: Vec::new(),
        });
    }
    requests
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn location(path: PathBuf, line: Option<u32>) -> Location {
        Location {
            path,
            line,
            column: None,
        }
    }

    #[test]
    fn files_join_the_window_of_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(root.join("src")).unwrap();
        for path in [
            root.join("src/lib.rs"),
            root.join("README.md"),
            other.path().join("notes.md"),
        ] {
            fs::write(path, "").unwrap();
        }

        let requests = window_requests(vec![
            location(root.join("src/lib.rs"), Some(3)),
            location(root.clone(), None),
            location(other.path().join("notes.md"), None),
            location(root.join("README.md"), None),
            location(root.clone(), None),
        ]);
        assert_eq!(
            requests,
            [
                WindowRequest {
                    root: Some(root.clone()),
                    files: vec![
                        location(root.join("src/lib.rs"), Some(3)),
                        location(root.join("README.md"), None)
                    ],
                },
                WindowRequest {
                    root: Some(other.path().to_owned()),
                    files: vec![location(other.path().join("notes.md"), None)],
                },
            ]
        );
    }

    #[test]
    fn no_locations_open_one_empty_window() {
        assert_eq!(
            window_requests(Vec::new()),
            [WindowRequest {
                root: None,
                files: Vec::new()
            }]
        );
    }
}
