//! The code version a turn ended on (hook.py Stop: `git_value` HEAD/branch + obs.py:687
//! `worktree_fingerprint`), using the same git CLI commands so fingerprints match obs.py byte for byte.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use obs_cli::git::git_value;
use obs_cli::project_resolution::resolve_path;
use sha2::{Digest, Sha256};

const MAX_HASHED_FILE_BYTES: u64 = 50 * 1024 * 1024;
const READ_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeVersion {
    /// `git rev-parse HEAD` ("" outside a repository).
    pub commit: String,
    /// `git branch --show-current` ("" when detached or outside a repository).
    pub branch: String,
    /// `worktree_fingerprint(root)`: `None` for a clean tree.
    pub dirty_hash: Option<String>,
}

impl CodeVersion {
    pub fn capture(root: &Path) -> Self {
        Self {
            commit: git_value(root, &["rev-parse", "HEAD"]),
            branch: git_value(root, &["branch", "--show-current"]),
            dirty_hash: worktree_fingerprint(root),
        }
    }

    /// `f"{commit or 'unversioned'}[+dirty:{dirty_hash[:12]}]"`.
    pub fn label(&self) -> String {
        let base = if self.commit.is_empty() {
            "unversioned"
        } else {
            self.commit.as_str()
        };
        match &self.dirty_hash {
            Some(hash) => format!("{base}+dirty:{}", &hash[..hash.len().min(12)]),
            None => base.to_owned(),
        }
    }
}

/// Mental model: porcelain status → sha256(binary diff vs HEAD) → each untracked file's
/// (relative path, size, content ≤ 50 MiB) in sorted order. Stops (keeping the partial hash) when the
/// diff command fails, exactly like obs.py's outer `except`.
pub fn worktree_fingerprint(root: &Path) -> Option<String> {
    let status = git_value(root, &["status", "--porcelain=v1", "--untracked-files=all"]);
    if status.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(status.as_bytes());

    if let Some(patch) = binary_diff_against_head(root) {
        hasher.update(Sha256::digest(patch));
        hash_untracked_files(root, &mut hasher);
    }
    Some(hex::encode(hasher.finalize()))
}

/// `subprocess.check_output(["git", "-C", root, "diff", "--binary", "--no-ext-diff", "HEAD", "--"])`.
fn binary_diff_against_head(root: &Path) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["diff", "--binary", "--no-ext-diff", "HEAD", "--"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn hash_untracked_files(root: &Path, hasher: &mut Sha256) {
    let listing = git_value(root, &["ls-files", "--others", "--exclude-standard"]);
    let mut untracked: Vec<&str> = listing.lines().collect();
    untracked.sort_unstable();
    let resolved_root = resolve_path(root);

    for relative in untracked {
        if hash_untracked_file(root, &resolved_root, relative, hasher).is_err() {
            hasher.update(format!("unreadable:{relative}").as_bytes());
        }
    }
}

/// One file: `relative`, `str(st_size)`, then content when it is a regular file ≤ 50 MiB.
fn hash_untracked_file(
    root: &Path,
    resolved_root: &Path,
    relative: &str,
    hasher: &mut Sha256,
) -> io::Result<()> {
    let candidate = resolve_path(&root.join(relative));
    if !candidate.starts_with(resolved_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "outside the worktree",
        ));
    }
    let metadata = std::fs::metadata(&candidate)?;
    hasher.update(relative.as_bytes());
    hasher.update(metadata.len().to_string().as_bytes());

    if metadata.is_file() && metadata.len() <= MAX_HASHED_FILE_BYTES {
        let mut file = File::open(&candidate)?;
        let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
        loop {
            let read = file.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            hasher.update(&chunk[..read]);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_marks_dirty_trees_and_unversioned_roots() {
        let clean = CodeVersion {
            commit: "abc123".into(),
            branch: "master".into(),
            dirty_hash: None,
        };
        assert_eq!(clean.label(), "abc123");
        let dirty = CodeVersion {
            commit: String::new(),
            branch: String::new(),
            dirty_hash: Some("ab".repeat(32)),
        };
        assert_eq!(dirty.label(), "unversioned+dirty:abababababab");
    }

    #[test]
    fn non_repository_has_no_version() {
        let dir = tempfile::tempdir().unwrap();
        let version = CodeVersion::capture(dir.path());
        assert_eq!(version.commit, "");
        assert_eq!(version.dirty_hash, None);
    }
}
