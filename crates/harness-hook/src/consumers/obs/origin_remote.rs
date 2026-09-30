//! The `origin` remote URL for the project alias, read straight from the repository's git config
//! instead of spawning `git remote get-url origin` on every hook (one process per tool call).
//! Configs that git would rewrite or extend (`insteadOf`, `include`) fall back to the git CLI so the
//! alias hash stays identical to hook.py's.

use std::path::{Path, PathBuf};

use obs_cli::git::git_value;

const ORIGIN_SECTION: &str = "[remote \"origin\"]";
const GITDIR_PREFIX: &str = "gitdir:";

// Mental model: find the repository config → if git would transform it, ask git → else read `url`
// from the `[remote "origin"]` section.
pub fn origin_remote_url(root: &Path) -> String {
    let Some(config_text) = repository_config(root) else {
        return String::new();
    };
    if needs_git_resolution(&config_text) {
        return git_value(root, &["remote", "get-url", "origin"]);
    }
    origin_url_in(&config_text).unwrap_or_default()
}

/// `<git common dir>/config` of the repository containing `start`, walking up like git does.
fn repository_config(start: &Path) -> Option<String> {
    let dot_git = start
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|path| path.exists())?;
    let git_dir = resolve_git_dir(&dot_git)?;
    let common_dir = common_dir_of(&git_dir);
    std::fs::read_to_string(common_dir.join("config")).ok()
}

/// `.git` is a directory, or a file `gitdir: <path>` (worktrees, submodules).
fn resolve_git_dir(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let text = std::fs::read_to_string(dot_git).ok()?;
    let target = text.trim().strip_prefix(GITDIR_PREFIX)?.trim();
    let base = dot_git.parent()?;
    Some(base.join(target))
}

/// Worktrees keep shared config in the directory named by their `commondir` file.
fn common_dir_of(git_dir: &Path) -> PathBuf {
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(relative) => git_dir.join(relative.trim()),
        Err(_) => git_dir.to_path_buf(),
    }
}

#[inline]
fn needs_git_resolution(config_text: &str) -> bool {
    let lowered = config_text.to_ascii_lowercase();
    lowered.contains("insteadof") || lowered.contains("[include")
}

/// The first `url = ...` inside `[remote "origin"]`.
fn origin_url_in(config_text: &str) -> Option<String> {
    let mut is_in_origin = false;
    for line in config_text.lines().map(str::trim) {
        if line.starts_with('[') {
            is_in_origin = line.eq_ignore_ascii_case(ORIGIN_SECTION);
            continue;
        }
        if is_in_origin {
            if let Some(url) = url_value(line) {
                return Some(url);
            }
        }
    }
    None
}

fn url_value(line: &str) -> Option<String> {
    let (key, value) = line.split_once('=')?;
    if !key.trim().eq_ignore_ascii_case("url") {
        return None;
    }
    Some(unquoted(value.trim()).to_owned())
}

#[inline]
fn unquoted(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://x/up.git\n[remote \"origin\"]\n\turl = https://github.com/me/repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";

    #[test]
    fn reads_the_origin_url_only() {
        assert_eq!(
            origin_url_in(CONFIG).as_deref(),
            Some("https://github.com/me/repo.git")
        );
        assert_eq!(origin_url_in("[remote \"upstream\"]\n\turl = u\n"), None);
    }

    #[test]
    fn rewrites_and_includes_defer_to_git() {
        assert!(needs_git_resolution(
            "[url \"git@x:\"]\n\tinsteadOf = https://x/\n"
        ));
        assert!(needs_git_resolution("[include]\n\tpath = other\n"));
        assert!(!needs_git_resolution(CONFIG));
    }

    #[test]
    fn finds_config_from_a_subdirectory_and_worktree_file() {
        let repo = tempfile::tempdir().unwrap();
        let git_dir = repo.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("config"), CONFIG).unwrap();
        let nested = repo.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(origin_remote_url(&nested), "https://github.com/me/repo.git");

        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(
            worktree.path().join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        assert_eq!(
            origin_remote_url(worktree.path()),
            "https://github.com/me/repo.git"
        );
    }

    #[test]
    fn no_repository_means_no_remote() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(origin_remote_url(dir.path()), "");
    }
}
