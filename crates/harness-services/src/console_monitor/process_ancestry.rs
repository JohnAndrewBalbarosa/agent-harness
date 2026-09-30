//! Parent-process walk looking for `codex.exe` (Python `ancestry`).

use std::collections::{HashMap, HashSet};

use super::profile_mapping::profile_from_path;

/// Maximum number of links recorded in one walk.
pub const MAX_ANCESTRY_DEPTH: usize = 12;
/// The ancestor that makes a console window reportable (compared lowercase).
pub const CODEX_EXECUTABLE: &str = "codex.exe";

/// One row of the ToolHelp32 process snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub name: String,
    pub parent_pid: u32,
}

/// pid → (exe name, parent pid), as captured by one snapshot.
pub type ProcessTable = HashMap<u32, ProcessEntry>;

/// One recorded link of the chain: `{"pid", "process", "parentPid"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestryLink {
    pub pid: u32,
    pub process: String,
    pub parent_pid: u32,
}

/// Result of the walk: the chain, the Codex profile, and the `codex.exe` pid when one was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestry {
    pub chain: Vec<AncestryLink>,
    pub profile: &'static str,
    pub codex_pid: Option<u32>,
}

/// Exe name for `pid`, `"unknown"` when absent from the snapshot (Python default).
#[inline]
pub fn process_name(table: &ProcessTable, pid: u32) -> &str {
    table
        .get(&pid)
        .map_or("unknown", |entry| entry.name.as_str())
}

/// Parent pid for `pid`, `0` when absent from the snapshot.
#[inline]
pub fn parent_pid(table: &ProcessTable, pid: u32) -> u32 {
    table.get(&pid).map_or(0, |entry| entry.parent_pid)
}

/// Walks from `pid` up the parent chain (max [`MAX_ANCESTRY_DEPTH`] links, cycle-safe, stops at pid 0).
///
/// `path_for_pid` is only called for the `codex.exe` link, to map its image path to a profile.
pub fn walk_ancestry(
    start_pid: u32,
    table: &ProcessTable,
    mut path_for_pid: impl FnMut(u32) -> String,
) -> Ancestry {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut pid = start_pid;
    while pid != 0 && seen.insert(pid) && chain.len() < MAX_ANCESTRY_DEPTH {
        let name = process_name(table, pid).to_owned();
        let parent = parent_pid(table, pid);
        let is_codex = name.to_lowercase() == CODEX_EXECUTABLE;
        chain.push(AncestryLink {
            pid,
            process: name,
            parent_pid: parent,
        });
        if is_codex {
            let profile = profile_from_path(&path_for_pid(pid));
            return Ancestry {
                chain,
                profile,
                codex_pid: Some(pid),
            };
        }
        pid = parent;
    }
    Ancestry {
        chain,
        profile: "unknown",
        codex_pid: None,
    }
}
