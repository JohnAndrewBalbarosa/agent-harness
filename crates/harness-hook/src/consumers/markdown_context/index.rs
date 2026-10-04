//! Cached filename inventory; metadata probes do not read Markdown bodies.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::state::CachePaths;
use super::tree::WorkspaceMarkdownTree;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownIndex {
    pub tree: WorkspaceMarkdownTree,
    pub metadata: BTreeMap<PathBuf, FileStamp>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStamp {
    modified_nanos: u128,
    length: u64,
}

impl MarkdownIndex {
    // Mental model: locked cached snapshot → probe recorded metadata → rebuild only on change.
    pub fn load(cache: &CachePaths, root: &Path, refresh: bool) -> io::Result<(Self, bool)> {
        crate::shared_state::file_lock::with_sidecar_lock(&cache.tree, || {
            if !refresh {
                if let Ok(bytes) = std::fs::read(&cache.tree) {
                    if let Ok(index) = serde_json::from_slice::<Self>(&bytes) {
                        if index.tree.canonical_root == root && index.is_current(root) {
                            return Ok((index, false));
                        }
                    }
                }
            }
            let tree = WorkspaceMarkdownTree::build(root).map_err(io::Error::other)?;
            let mut paths = tree.directories();
            paths.extend(
                tree.directories()
                    .iter()
                    .flat_map(|path| tree.markdown_files_in(path)),
            );
            // Ignore/config changes affect visibility even when Markdown itself is unchanged.
            paths.extend([
                PathBuf::from(".gitignore"),
                PathBuf::from(".ignore"),
                PathBuf::from(".git/info/exclude"),
            ]);
            for directory in tree.directories() {
                paths.push(directory.join(".gitignore"));
                paths.push(directory.join(".ignore"));
            }
            let metadata = paths
                .into_iter()
                .map(|path| {
                    let stamp = file_stamp(&root.join(&path));
                    (path, stamp)
                })
                .collect();
            let index = Self { tree, metadata };
            let bytes = serde_json::to_vec(&index).map_err(io::Error::other)?;
            let name = crate::shared_state::atomic_write::unique_temp_name("markdown-tree");
            crate::shared_state::atomic_write::replace_file(&cache.tree, &name, &bytes, false)?;
            Ok((index, true))
        })
    }

    fn is_current(&self, root: &Path) -> bool {
        self.metadata
            .iter()
            .all(|(path, stamp)| file_stamp(&root.join(path)) == *stamp)
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths = self
            .tree
            .directories()
            .iter()
            .flat_map(|path| self.tree.markdown_files_in(path))
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    let metadata = std::fs::metadata(path).ok();
    FileStamp {
        modified_nanos: metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos()),
        length: metadata.map_or(0, |value| value.len()),
    }
}
