//! Python `Path(...).resolve()` (non-strict): absolute, symlinks resolved for the part that
//! exists, `..` applied after resolution like `os.path.realpath`, no Windows `\\?\` prefix.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
const VERBATIM_PREFIX: &str = r"\\?\";

/// Mental model: make absolute → walk components, canonicalizing each prefix that exists.
pub fn python_resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut resolved = PathBuf::new();

    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                if let Ok(real) = fs::canonicalize(&resolved) {
                    resolved = without_verbatim_prefix(real);
                }
            }
        }
    }
    resolved
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\server\share` → `\\server\share` (what Python prints).
#[inline]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(VERBATIM_UNC_PREFIX) {
        return PathBuf::from(OsString::from(format!(r"\\{rest}")));
    }
    if let Some(rest) = text.strip_prefix(VERBATIM_PREFIX) {
        return PathBuf::from(rest);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_missing_tails_lexically_under_a_real_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let real_root = fs::canonicalize(directory.path()).unwrap();
        let resolved = python_resolve(&directory.path().join("a").join("..").join("logs"));

        assert_eq!(resolved, without_verbatim_prefix(real_root).join("logs"));
    }

    #[test]
    fn strips_windows_verbatim_prefixes() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\work")),
            PathBuf::from(r"C:\work")
        );
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
    }
}
