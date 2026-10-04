//! Allowlisted canonical repository roots; a regex never replaces path containment.
use std::io;
use std::path::{Component, Path, PathBuf};

use regex::RegexBuilder;

use crate::hook_context::HookContext;

pub fn allowed_repository(context: &HookContext, cwd: &Path) -> io::Result<Option<PathBuf>> {
    let cwd = cwd.canonicalize()?;
    let base = projects_directory(context)?;
    if !cwd.starts_with(&base) || cwd == base {
        return Ok(None);
    }
    let root = cwd
        .ancestors()
        .take_while(|path| *path != base)
        .find(|path| {
            path.join(".git").exists() || path.join("observability.project.toml").is_file()
        });
    let Some(root) = root else {
        return Ok(None);
    };
    let root = root.canonicalize()?;
    if !root.starts_with(&base) || root == base {
        return Ok(None);
    }
    let default_pattern = format!("^{}/.+$", regex::escape(&normalized_path(&base)));
    let pattern = context
        .env_value("AH_MARKDOWN_ALLOWED_ROOT_REGEX")
        .unwrap_or(&default_pattern);
    let expression = RegexBuilder::new(pattern)
        .case_insensitive(cfg!(windows))
        .size_limit(65_536)
        .build()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    Ok(expression.is_match(&normalized_path(&root)).then_some(root))
}

fn projects_directory(context: &HookContext) -> io::Result<PathBuf> {
    let configured = context
        .env_value("AH_MARKDOWN_PROJECTS_DIR")
        .map(PathBuf::from);
    let default = context
        .env_value("USERPROFILE")
        .or_else(|| context.env_value("HOME"))
        .map(|home| PathBuf::from(home).join("Desktop").join("Projects"));
    configured
        .or(default)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Configure AH_MARKDOWN_PROJECTS_DIR",
            )
        })?
        .canonicalize()
}

pub fn resolve_document(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected a project-relative Markdown path",
        ));
    }
    let absolute = root.join(relative).canonicalize()?;
    if !absolute.starts_with(root)
        || !absolute.is_file()
        || !absolute
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Markdown path is outside the repository",
        ));
    }
    Ok(absolute)
}

pub fn normalized_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    text.strip_prefix("//?/").unwrap_or(&text).to_owned()
}
