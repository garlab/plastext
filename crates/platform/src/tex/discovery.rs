//! Bounded, data-only discovery. This is deliberately not a TeX interpreter.
use super::{Config, Engine};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Project {
    root: Option<PathBuf>,
    input_folders: Vec<PathBuf>,
}

impl Config {
    pub fn automatic_root(
        input: &Path,
        workspace: Option<&Path>,
        engine: Engine,
    ) -> Result<Self, String> {
        let mut config = Self::new(input, engine)?;
        config.automatic = Some(boundary(&config.input, workspace));
        config.refresh_inputs()
    }
    pub fn with_additional_inputs(mut self, inputs: &[PathBuf]) -> Result<Self, String> {
        if inputs.is_empty() {
            return Ok(self);
        }
        self.extra_inputs.extend_from_slice(inputs);
        if self.automatic.is_none() {
            return Self::with_inputs(&self.input, self.engine, &self.extra_inputs);
        }
        self.refresh_inputs()
    }
    /// Find a root and local inputs without executing project configuration.
    pub fn automatic(
        input: &Path,
        workspace: Option<&Path>,
        engine: Engine,
    ) -> Result<Self, String> {
        let input = input.canonicalize().map_err(|e| e.to_string())?;
        let boundary = boundary(&input, workspace);
        let (project, base) = project(&input, &boundary)?;
        let root = if let Some(root) = project.root {
            inside(&base.join(root), &boundary)
                .ok_or("Configured TeX root must be inside the project")?
        } else {
            find_root(&input, &boundary)?
        };
        let inputs = project
            .input_folders
            .iter()
            .map(|path| {
                inside(&base.join(path), &boundary)
                    .ok_or_else(|| "Configured TeX inputs must be inside the project".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut config = Self::with_inputs(&root, engine, &inputs)?;
        config.automatic = Some(boundary);
        config.refresh_inputs()
    }

    pub(super) fn refresh_inputs(&self) -> Result<Self, String> {
        let Some(boundary) = &self.automatic else {
            return Ok(self.clone());
        };
        let mut inputs = self.extra_inputs.clone();
        let root = self.input.parent().unwrap();
        let mut pending = vec![self.input.clone()];
        let mut seen = HashSet::new();
        let mut bytes = 0;
        let mut lookups = 0;
        'scan: while let Some(path) = pending.pop() {
            if seen.len() >= 128 || bytes >= 16 * 1024 * 1024 {
                break;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            let text = read(&path, 2 * 1024 * 1024)?;
            bytes += text.len();
            let text = uncomment(&text);
            // Literal relative paths also cover simple path macros such as
            // \newcommand{\figures}{../figures}. No macro is executed/expanded.
            for value in groups(&text) {
                if !value.starts_with("../") && !value.starts_with("./") {
                    continue;
                }
                if value.contains(['\\', '{', '}', '#', '\n']) {
                    continue;
                }
                for base in [root, path.parent().unwrap()] {
                    if lookups == 2048 {
                        break 'scan;
                    }
                    lookups += 1;
                    if let Some(dependency) = resolve(base, value, boundary) {
                        let folder = if dependency.is_dir() {
                            dependency.as_path()
                        } else {
                            dependency.parent().unwrap()
                        };
                        if !folder.starts_with(root)
                            && !inputs.iter().any(|p| folder.starts_with(p))
                        {
                            inputs.retain(|p| !p.starts_with(folder));
                            inputs.push(folder.to_owned());
                        }
                    }
                }
            }
            for dependency in includes(&text) {
                for base in [root, path.parent().unwrap()] {
                    if lookups == 2048 {
                        break 'scan;
                    }
                    lookups += 1;
                    if let Some(path) = resolve(base, dependency, boundary)
                        && path.extension().is_some_and(|ext| ext == "tex")
                        && pending.len() < 128
                    {
                        pending.push(path);
                    }
                }
            }
        }
        let mut config = Self::with_inputs(&self.input, self.engine.clone(), &inputs)?;
        config.automatic = self.automatic.clone();
        config.extra_inputs = self.extra_inputs.clone();
        Ok(config)
    }
}

fn boundary(input: &Path, workspace: Option<&Path>) -> PathBuf {
    let parent = input.parent().unwrap();
    for ancestor in parent.ancestors().take(32) {
        if ancestor.join(".git").exists() {
            return ancestor.to_owned();
        }
    }
    workspace
        .and_then(|p| p.canonicalize().ok())
        .filter(|p| input.starts_with(p))
        .unwrap_or_else(|| parent.to_owned())
}
fn inside(path: &Path, boundary: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    path.starts_with(boundary).then_some(path)
}
fn resolve(base: &Path, value: &str, boundary: &Path) -> Option<PathBuf> {
    if Path::new(value).is_absolute() || value.contains(['\\', '#', '{', '}']) {
        return None;
    }
    let path = base.join(value);
    inside(&path, boundary).or_else(|| {
        if path.extension().is_some() {
            return None;
        }
        ["tex", "pdf", "png", "jpg", "jpeg"]
            .into_iter()
            .find_map(|extension| inside(&path.with_extension(extension), boundary))
    })
}
fn read(path: &Path, limit: u64) -> Result<String, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("TeX input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn project(input: &Path, boundary: &Path) -> Result<(Project, PathBuf), String> {
    for ancestor in input
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|p| p.starts_with(boundary))
    {
        let path = ancestor.join(".plastext/tex.toml");
        if path.exists() {
            let path =
                inside(&path, boundary).ok_or("TeX configuration must be inside the project")?;
            if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 64 * 1024 {
                return Err("TeX configuration exceeds 64 KiB".into());
            }
            let text = read(&path, 64 * 1024)?;
            let settings =
                toml::from_str(&text).map_err(|e| format!("Invalid .plastext/tex.toml: {e}"))?;
            return Ok((settings, ancestor.to_owned()));
        }
    }
    Ok((Project::default(), boundary.to_owned()))
}
fn root_hint(text: &str) -> Option<&str> {
    text.lines().take(64).find_map(|line| {
        let line = line
            .trim()
            .strip_prefix('%')?
            .trim()
            .strip_prefix('!')?
            .trim();
        let (key, value) = line.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("tex root")
            .then_some(value.trim())
    })
}
fn find_root(input: &Path, boundary: &Path) -> Result<PathBuf, String> {
    let mut current = input.to_owned();
    let mut seen = HashSet::new();
    for depth in 0..8 {
        if !seen.insert(current.clone()) {
            return Err("Circular TeX root directive".into());
        }
        let text = read(&current, 64 * 1024)?;
        if let Some(hint) = root_hint(&text) {
            if depth == 7 {
                return Err("TeX root directives are nested too deeply".into());
            }
            current = resolve(current.parent().unwrap(), hint, boundary)
                .ok_or("TeX root directive must name a file inside the project")?;
            continue;
        }
        if uncomment(&text).contains("\\documentclass") {
            return Ok(current);
        }
        break;
    }
    let mut candidates = Vec::new();
    let mut entries = 0;
    let mut remaining: usize = 16 * 1024 * 1024;
    let mut lookups = 2048;
    for folder in input
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|p| p.starts_with(boundary))
    {
        for entry in fs::read_dir(folder).map_err(|e| e.to_string())? {
            entries += 1;
            if entries > 512 {
                break;
            }
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "tex") {
                continue;
            }
            if remaining == 0 {
                return Err(
                    "TeX root discovery limit reached; choose a root in Preview settings".into(),
                );
            }
            let text = read(&path, (64 * 1024).min(remaining) as u64)?;
            remaining = remaining.saturating_sub(text.len());
            let text = uncomment(&text);
            if text.contains("\\documentclass")
                && references(&path, input, boundary, &mut remaining, &mut lookups)?
            {
                candidates.push(path);
            }
        }
        if entries > 512 {
            break;
        }
    }
    if entries > 512 || remaining == 0 || lookups == 0 {
        return Err("TeX root discovery limit reached; choose a root in Preview settings".into());
    }
    match candidates.as_slice() {
        [root] => Ok(root.clone()),
        [] => Err("No TeX root found. Add % !TeX root = main.tex or choose a root in Preview settings.".into()),
        _ => Err("Several TeX roots include this file. Add % !TeX root = main.tex or choose a root in Preview settings.".into()),
    }
}
fn references(
    root: &Path,
    target: &Path,
    boundary: &Path,
    remaining: &mut usize,
    lookups: &mut usize,
) -> Result<bool, String> {
    let mut pending = vec![root.to_owned()];
    let mut seen = HashSet::new();
    let mut bytes = 0;
    while let Some(path) = pending.pop() {
        if path == target {
            return Ok(true);
        }
        if seen.len() >= 64 || bytes >= 4 * 1024 * 1024 || *remaining == 0 {
            break;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let text = read(&path, (256 * 1024).min(*remaining) as u64)?;
        *remaining = remaining.saturating_sub(text.len());
        let text = uncomment(&text);
        bytes += text.len();
        for value in includes(&text) {
            for base in [root.parent().unwrap(), path.parent().unwrap()] {
                if *lookups == 0 {
                    return Ok(false);
                }
                *lookups -= 1;
                if let Some(path) = resolve(base, value, boundary)
                    && path.is_file()
                    && pending.len() < 64
                {
                    pending.push(path);
                }
            }
        }
    }
    Ok(false)
}
fn uncomment(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut slashes = 0;
            for (i, c) in line.char_indices() {
                if c == '%' && slashes % 2 == 0 {
                    return &line[..i];
                }
                slashes = if c == '\\' { slashes + 1 } else { 0 };
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn groups(text: &str) -> impl Iterator<Item = &str> {
    text.split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(value, _)| value.trim()))
}
fn includes(text: &str) -> impl Iterator<Item = &str> {
    text.split('\\').skip(1).filter_map(|command| {
        let rest = command
            .strip_prefix("input")
            .or_else(|| command.strip_prefix("include"))?;
        let rest = rest.trim_start().strip_prefix('{')?;
        rest.split_once('}').map(|(path, _)| path.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir(root.join("paper")).unwrap();
        fs::write(
            root.join("paper/main.tex"),
            "\\documentclass{article}\\input{chapter}",
        )
        .unwrap();
        fs::write(root.join("paper/chapter.tex"), "A chapter").unwrap();
        (dir, root)
    }
    #[test]
    fn finds_root_and_respects_directives_comments_and_ambiguity() {
        let (_dir, root) = fixture();
        let chapter = root.join("paper/chapter.tex");
        let main = root.join("paper/main.tex");
        assert_eq!(find_root(&chapter, &root).unwrap(), main);
        fs::write(
            root.join("paper/other.tex"),
            "\\documentclass{article}% \\input{chapter}",
        )
        .unwrap();
        assert_eq!(find_root(&chapter, &root).unwrap(), main);
        fs::write(
            root.join("paper/other.tex"),
            "\\documentclass{article}\\input{chapter}",
        )
        .unwrap();
        assert!(find_root(&chapter, &root).unwrap_err().contains("Several"));
        fs::write(&chapter, "% !TeX root = main.tex\nA chapter").unwrap();
        assert_eq!(find_root(&chapter, &root).unwrap(), main);
        fs::write(&main, "% !TeX root = chapter.tex").unwrap();
        assert!(find_root(&chapter, &root).unwrap_err().contains("Circular"));
    }
    #[test]
    fn config_and_references_cannot_escape_project() {
        let (_dir, root) = fixture();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.tex");
        fs::write(&secret, "private").unwrap();
        assert!(inside(&secret, &root).is_none());
        fs::create_dir(root.join("figures")).unwrap();
        fs::write(root.join("figures/chart.pdf"), "placeholder").unwrap();
        assert_eq!(
            resolve(&root.join("paper"), "../figures/chart", &root),
            Some(root.join("figures/chart.pdf"))
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, root.join("paper/link.tex")).unwrap();
            assert!(resolve(&root, "paper/link.tex", &root).is_none());
        }
        fs::create_dir(root.join(".plastext")).unwrap();
        fs::write(
            root.join(".plastext/tex.toml"),
            "root = 'paper/main.tex'\ninput_folders = ['paper']",
        )
        .unwrap();
        let (config, base) = project(&root.join("paper/chapter.tex"), &root).unwrap();
        assert_eq!(base, root);
        assert_eq!(config.root.unwrap(), PathBuf::from("paper/main.tex"));
        assert!(toml::from_str::<Project>("command = 'make'").is_err());
    }
    #[test]
    fn bounded_search_never_presents_a_partial_scan_as_an_unambiguous_root() {
        let (_dir, root) = fixture();
        for number in 0..513 {
            fs::write(root.join(format!("paper/item-{number}")), "").unwrap();
        }
        let chapter = root.join("paper/chapter.tex");
        assert!(
            find_root(&chapter, &root)
                .unwrap_err()
                .contains("limit reached")
        );
        // An explicit directive avoids directory searching even in a huge folder.
        fs::write(&chapter, "% !TeX root = main.tex").unwrap();
        assert_eq!(
            find_root(&chapter, &root).unwrap(),
            root.join("paper/main.tex")
        );
    }
}
