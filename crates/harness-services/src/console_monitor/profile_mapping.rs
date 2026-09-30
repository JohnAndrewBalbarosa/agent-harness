//! Codex profile from a `codex.exe` image path (Python `profile_from_path`).

/// Maps an executable path to `cy`, `feu`, `personal`, or `unknown`.
///
/// Case-insensitive; `/` is treated as `\`. `cy`/`feu` win over the plain `.codex` directory.
pub fn profile_from_path(path: &str) -> &'static str {
    let normalized = path.to_lowercase().replace('/', "\\");
    if normalized.contains("\\.codex-cy\\") {
        return "cy";
    }
    if normalized.contains("\\.codex-feu\\") {
        return "feu";
    }
    if normalized.contains("\\.codex\\") {
        return "personal";
    }
    "unknown"
}
