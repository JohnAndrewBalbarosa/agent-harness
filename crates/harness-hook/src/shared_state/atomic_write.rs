//! Write-then-rename so readers never observe a partial file.

use std::io::{self, Write};
use std::path::Path;

/// `<base>.<pid>.<nanos>.tmp`: unique per writer, so concurrent hooks never share (or delete) a temp file.
pub fn unique_temp_name(base: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{base}.{}.{nanos}.tmp", std::process::id())
}

/// Writes `bytes` to `<target>`'s directory as `temp_name`, optionally fsyncs, then replaces `target`.
pub fn replace_file(target: &Path, temp_name: &str, bytes: &[u8], sync: bool) -> io::Result<()> {
    let directory = target.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)?;
    let temp_path = directory.join(temp_name);

    let written =
        write_temp(&temp_path, bytes, sync).and_then(|()| std::fs::rename(&temp_path, target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    written
}

fn write_temp(temp_path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    let mut file = std::fs::File::create(temp_path)?;
    file.write_all(bytes)?;
    if sync {
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_content_and_removes_the_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        std::fs::write(&target, b"old").unwrap();
        replace_file(&target, "state.json.1.tmp", b"new", true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!dir.path().join("state.json.1.tmp").exists());
    }
}
