use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::tree::WorkspaceMarkdownTree;

const STOP_WORDS: &[&str] = &[
    "about", "after", "also", "change", "file", "folder", "from", "into", "markdown", "system",
    "that", "this", "with",
];

pub fn relevant_directories(tree: &WorkspaceMarkdownTree, query: &str) -> Vec<PathBuf> {
    let normalized_query = normalize(query);
    let query_tokens = tokens(&normalized_query);
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let mut ranked = tree
        .directories()
        .into_iter()
        .filter_map(|directory| {
            let score = directory_score(tree, &directory, &normalized_query, &query_tokens);
            (score > 0).then_some((score, directory))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for (_, directory) in ranked {
        for ancestor in ancestors_nearest_first(&directory) {
            if seen.insert(ancestor.clone()) {
                ordered.push(ancestor);
            }
        }
    }
    ordered
}

fn directory_score(
    tree: &WorkspaceMarkdownTree,
    directory: &Path,
    query: &str,
    query_tokens: &HashSet<String>,
) -> usize {
    let directory_text = normalize(&directory.to_string_lossy());
    let files = tree.markdown_files_in(directory);
    let explicit_directory = !directory_text.is_empty() && query.contains(&directory_text);
    let explicit_file = files.iter().any(|file| {
        let relative = normalize(&file.to_string_lossy());
        let name = file
            .file_name()
            .map(|name| normalize(&name.to_string_lossy()))
            .unwrap_or_default();
        query.contains(&relative) || (!name.is_empty() && query.contains(&name))
    });
    if explicit_file {
        return 1_000 + directory.components().count();
    }
    if explicit_directory {
        return 500 + directory.components().count();
    }

    let mut candidate_tokens = tokens(&directory_text);
    for file in files {
        if let Some(stem) = file.file_stem() {
            candidate_tokens.extend(tokens(&normalize(&stem.to_string_lossy())));
        }
    }
    let overlap = query_tokens.intersection(&candidate_tokens).count();
    overlap * 10 + usize::from(overlap > 0) * directory.components().count()
}

fn ancestors_nearest_first(directory: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut current = Some(directory);
    while let Some(path) = current {
        ancestors.push(path.to_path_buf());
        current = path.parent();
    }
    ancestors
}

fn normalize(text: &str) -> String {
    text.replace('\\', "/").to_lowercase()
}

fn tokens(text: &str) -> HashSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.len() >= 3 && !STOP_WORDS.contains(token))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_two_branches_and_shared_ancestors_once() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("systems/auth")).unwrap();
        std::fs::create_dir_all(directory.path().join("systems/login")).unwrap();
        std::fs::write(directory.path().join("ROOT.md"), "root").unwrap();
        std::fs::write(directory.path().join("systems/auth/spec.md"), "auth").unwrap();
        std::fs::write(directory.path().join("systems/login/design.md"), "login").unwrap();
        let tree = WorkspaceMarkdownTree::build(directory.path()).unwrap();

        let selected = relevant_directories(&tree, "change the auth and login architecture");
        assert!(selected.contains(&PathBuf::from("systems/auth")));
        assert!(selected.contains(&PathBuf::from("systems/login")));
        assert_eq!(
            selected
                .iter()
                .filter(|path| path.as_os_str().is_empty())
                .count(),
            1
        );
    }
}
