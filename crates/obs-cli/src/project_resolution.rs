//! Which project a command acts on, resolved exactly as obs.py does:
//! the nearest `observability.project.toml` walking up from the resolved cwd (obs.py:200),
//! with an explicit `--project` taking precedence where obs.py allows it (obs.py:1386).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::failure::{CliFailure, CliResult};
use crate::project_marker::{parse_project_table, ProjectTable, MARKER_FILE_NAME};

pub struct ProjectMarker {
    pub path: PathBuf,
    pub project: ProjectTable,
}

impl ProjectMarker {
    /// obs.py `config_path.parent`.
    pub fn root(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// obs.py `str(config["project"]["id"])`.
    pub fn project_id_text(&self) -> CliResult<String> {
        self.required_field("id").map(|value| value.to_python_str())
    }

    pub fn required_field(&self, key: &str) -> CliResult<&crate::project_marker::MarkerValue> {
        self.project.get(key).ok_or_else(|| {
            CliFailure::Internal(anyhow::anyhow!(
                "{} is missing project.{key}",
                self.path.display()
            ))
        })
    }
}

/// `args.project or marker id`, else obs.py's "Project ID is required." refusal.
pub fn explicit_or_marker_project_id(explicit: Option<&str>) -> CliResult<String> {
    if let Some(project_id) = explicit.filter(|value| !value.is_empty()) {
        return Ok(project_id.to_owned());
    }

    match find_marker_from_cwd()? {
        Some(marker) => marker.project_id_text(),
        None => Err(CliFailure::refusal("Project ID is required.")),
    }
}

pub fn find_marker_from_cwd() -> CliResult<Option<ProjectMarker>> {
    find_project_marker(&current_directory()?)
}

/// obs.py:200 `project_config(start)`: first ancestor (inclusive) holding the marker file.
pub fn find_project_marker(start: &Path) -> CliResult<Option<ProjectMarker>> {
    let resolved = resolve_path(start);

    for directory in resolved.ancestors() {
        let candidate = directory.join(MARKER_FILE_NAME);
        if candidate.exists() {
            return load_marker(candidate).map(Some);
        }
    }
    Ok(None)
}

fn load_marker(path: PathBuf) -> CliResult<ProjectMarker> {
    let text =
        fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))?;
    let project = parse_project_table(&text).map_err(|problem| {
        CliFailure::Internal(anyhow::anyhow!("invalid {}: {problem}", path.display()))
    })?;
    Ok(ProjectMarker { path, project })
}

/// `Path.cwd()` (not resolved).
pub fn current_directory() -> CliResult<PathBuf> {
    Ok(env::current_dir().context("unable to read the current directory")?)
}

/// Python `Path.resolve()`: absolute, symlinks resolved, no Windows `\\?\` verbatim prefix.
/// Paths that cannot be canonicalized stay absolute but unresolved (strict=False).
pub fn resolve_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };

    fs::canonicalize(&absolute)
        .map(without_verbatim_prefix)
        .unwrap_or(absolute)
}

fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();

    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(local) => PathBuf::from(local),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_windows_verbatim_prefixes() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from("/tmp/x")),
            PathBuf::from("/tmp/x")
        );
    }

    #[test]
    fn finds_marker_in_an_ancestor() {
        let root = env::temp_dir().join(format!("obs-rs-marker-{}", std::process::id()));
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join(MARKER_FILE_NAME), "[project]\nid = \"p-1\"\n").unwrap();

        let marker = find_project_marker(&nested).unwrap().unwrap();
        assert_eq!(marker.project_id_text().unwrap(), "p-1");
        assert_eq!(marker.root(), resolve_path(&root).as_path());

        fs::remove_dir_all(&root).unwrap();
    }
}
