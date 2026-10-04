use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

const PRUNED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "__pycache__",
    ".backups",
    ".cache",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMarkdownTree {
    pub schema_version: u32,
    pub canonical_root: PathBuf,
    pub root_node: MarkdownFolderNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownFolderNode {
    pub current_folder_name: String,
    pub markdown_file_names: Vec<String>,
    pub child_folders: Vec<MarkdownFolderNode>,
}

impl WorkspaceMarkdownTree {
    pub fn build(root: &Path) -> anyhow::Result<Self> {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let root_name = canonical_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| canonical_root.display().to_string());
        let mut root_node = MarkdownFolderNode::new(root_name);
        let mut builder = WalkBuilder::new(&canonical_root);
        builder
            .hidden(false)
            .follow_links(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .require_git(false)
            .ignore(true)
            .parents(true)
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !entry.file_type().is_some_and(|kind| kind.is_dir())
                    || (!is_pruned_directory(&entry.file_name().to_string_lossy())
                        && !entry.path().join(".git").exists()
                        && !entry.path().join("observability.project.toml").is_file())
            });

        for result in builder.build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Ok(relative) = entry.path().strip_prefix(&canonical_root) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                root_node.ensure_folder(relative);
            } else if is_markdown(entry.path()) {
                let parent = relative.parent().unwrap_or_else(|| Path::new(""));
                let name = entry.file_name().to_string_lossy().into_owned();
                root_node
                    .ensure_folder(parent)
                    .markdown_file_names
                    .push(name);
            }
        }
        root_node.sort_recursively();
        Ok(Self {
            schema_version: 1,
            canonical_root,
            root_node,
        })
    }

    pub fn markdown_files_in(&self, relative_directory: &Path) -> Vec<PathBuf> {
        self.root_node
            .folder(relative_directory)
            .map(|node| {
                node.markdown_file_names
                    .iter()
                    .map(|name| relative_directory.join(name))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn directories(&self) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        self.root_node
            .collect_directories(Path::new(""), &mut directories);
        directories
    }
}

impl MarkdownFolderNode {
    fn new(current_folder_name: String) -> Self {
        Self {
            current_folder_name,
            markdown_file_names: Vec::new(),
            child_folders: Vec::new(),
        }
    }

    fn ensure_folder(&mut self, relative: &Path) -> &mut Self {
        let mut current = self;
        for component in normal_components(relative) {
            let name = component.to_string_lossy().into_owned();
            let index = current
                .child_folders
                .iter()
                .position(|child| child.current_folder_name.eq_ignore_ascii_case(&name))
                .unwrap_or_else(|| {
                    current.child_folders.push(Self::new(name));
                    current.child_folders.len() - 1
                });
            current = &mut current.child_folders[index];
        }
        current
    }

    fn folder(&self, relative: &Path) -> Option<&Self> {
        let mut current = self;
        for component in normal_components(relative) {
            let name = component.to_string_lossy();
            current = current
                .child_folders
                .iter()
                .find(|child| child.current_folder_name.eq_ignore_ascii_case(&name))?;
        }
        Some(current)
    }

    fn sort_recursively(&mut self) {
        self.markdown_file_names
            .sort_by_key(|name| name.to_lowercase());
        self.markdown_file_names.dedup();
        self.child_folders
            .sort_by_key(|child| child.current_folder_name.to_lowercase());
        for child in &mut self.child_folders {
            child.sort_recursively();
        }
    }

    fn collect_directories(&self, relative: &Path, output: &mut Vec<PathBuf>) {
        output.push(relative.to_path_buf());
        for child in &self.child_folders {
            let path = relative.join(&child.current_folder_name);
            child.collect_directories(&path, output);
        }
    }
}

pub fn resolve_workspace_root(cwd: &Path) -> PathBuf {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    find_ancestor(&start, |directory| directory.join(".git").exists())
        .or_else(|| {
            find_ancestor(&start, |directory| {
                directory.join("observability.project.toml").is_file()
            })
        })
        .unwrap_or(start)
}

fn find_ancestor(start: &Path, predicate: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|directory| predicate(directory))
        .map(Path::to_path_buf)
}

fn normal_components(path: &Path) -> impl Iterator<Item = &std::ffi::OsStr> {
    path.components().filter_map(|component| match component {
        Component::Normal(name) => Some(name),
        _ => None,
    })
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn is_pruned_directory(name: &str) -> bool {
    PRUNED_DIRECTORIES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_deterministic_tree_and_respects_ignores() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("auth/login")).unwrap();
        std::fs::create_dir_all(directory.path().join("ignored")).unwrap();
        std::fs::write(directory.path().join("README.md"), "root").unwrap();
        std::fs::write(directory.path().join("auth/A.MD"), "auth").unwrap();
        std::fs::write(directory.path().join("auth/login/z.md"), "login").unwrap();
        std::fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(directory.path().join("ignored/no.md"), "ignored").unwrap();

        let tree = WorkspaceMarkdownTree::build(directory.path()).unwrap();
        assert_eq!(tree.root_node.markdown_file_names, ["README.md"]);
        assert_eq!(
            tree.markdown_files_in(Path::new("auth")),
            [PathBuf::from("auth/A.MD")]
        );
        assert!(!tree.directories().contains(&PathBuf::from("ignored")));
    }

    #[test]
    fn git_root_precedes_project_marker() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".git")).unwrap();
        std::fs::create_dir_all(directory.path().join("a/b")).unwrap();
        std::fs::write(directory.path().join("a/observability.project.toml"), "x").unwrap();
        assert_eq!(
            resolve_workspace_root(&directory.path().join("a/b")),
            directory.path().canonicalize().unwrap()
        );
    }
}
