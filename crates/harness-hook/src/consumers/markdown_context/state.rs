use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::tree::WorkspaceMarkdownTree;
use crate::shared_state::atomic_write::{replace_file, unique_temp_name};
use crate::shared_state::file_lock::with_sidecar_lock;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMarkdownState {
    #[serde(default)]
    pub loaded_content_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub index_hash: Option<String>,
    #[serde(default)]
    pub observations: BTreeMap<String, ContentObservation>,
    #[serde(default)]
    pub supplied_characters: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentObservation {
    pub hash: String,
    pub source: String,
    pub coverage: String,
    pub start_character: Option<usize>,
    pub end_character: Option<usize>,
    #[serde(default)]
    pub ranges: Vec<(usize, usize)>,
}

impl ContentObservation {
    pub fn covers(&self, hash: &str, start: usize, end: usize) -> bool {
        self.hash == hash
            && (self.coverage == "full"
                || self
                    .ranges
                    .iter()
                    .any(|(left, right)| *left <= start && *right >= end))
    }

    pub fn include_range(&mut self, start: usize, end: usize, total: usize) {
        self.ranges.push((start, end));
        self.ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (left, right) in &self.ranges {
            if let Some(last) = merged.last_mut().filter(|last| last.1 >= *left) {
                last.1 = last.1.max(*right);
            } else {
                merged.push((*left, *right));
            }
        }
        self.coverage = if merged.first() == Some(&(0, total)) {
            "full"
        } else {
            "partial"
        }
        .into();
        // Dropping excess disjoint ranges is conservative: a later explicit request can reread them.
        merged.truncate(64);
        self.ranges = merged;
    }
}

pub struct CachePaths {
    pub tree: PathBuf,
    pub session: PathBuf,
}

impl CachePaths {
    pub fn new(harness_root: &Path, workspace_root: &Path, session_id: &str) -> Self {
        let base = harness_root
            .join("var")
            .join("markdown-context")
            .join("workspaces")
            .join(sha256_text(&workspace_root.to_string_lossy()));
        Self {
            tree: base.join("tree.json"),
            session: base
                .join("sessions")
                .join(format!("{}.json", sha256_text(session_id))),
        }
    }

    pub fn save_tree(&self, tree: &WorkspaceMarkdownTree) -> io::Result<()> {
        write_json(&self.tree, tree)
    }

    pub fn load_tree(&self) -> io::Result<WorkspaceMarkdownTree> {
        read_json(&self.tree)
    }

    pub fn update_session<T>(
        &self,
        work: impl FnOnce(&mut SessionMarkdownState) -> io::Result<T>,
    ) -> io::Result<T> {
        with_sidecar_lock(&self.session, || {
            let mut state = match read_json(&self.session) {
                Ok(state) => state,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    SessionMarkdownState::default()
                }
                Err(error) => return Err(error),
            };
            let result = work(&mut state)?;
            write_json_unlocked(&self.session, &state)?;
            Ok(result)
        })
    }

    pub fn remove_session(&self) {
        let _ = std::fs::remove_file(&self.session);
    }
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_text(text: &str) -> String {
    sha256_bytes(text.as_bytes())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    with_sidecar_lock(path, || write_json_unlocked(path, value))
}

fn write_json_unlocked<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    replace_file(path, &unique_temp_name(&file_name), &bytes, false)
}
