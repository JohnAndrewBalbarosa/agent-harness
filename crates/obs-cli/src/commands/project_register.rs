//! `obs project register [--init --name --slug]` (obs.py:1371-1381, :719 `register_project`):
//! optionally create `observability.project.toml` in the cwd, then spool `project.register`
//! with path/remote alias hashes and print the marker's `[project]` table.

use std::fs;
use std::path::Path;

use anyhow::Context;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::argv::{parse, CommandSpec, OptionSpec, ParsedArgs};
use crate::commands::output::print_command_result;
use crate::failure::{CliFailure, CliResult};
use crate::git::git_value;
use crate::operation::build_operation;
use crate::project_marker::{render_new_marker, MARKER_FILE_NAME};
use crate::project_resolution::{current_directory, find_marker_from_cwd, ProjectMarker};
use crate::python_compat::uuid7_text;
use crate::settings::Settings;
use crate::spool::flush::HubSink;
use crate::spool::open::open_spool;
use crate::spool::queue::enqueue_and_flush;

/// obs.py alias kinds: 1 = git remote URL, 2 = local root path.
const REMOTE_ALIAS_KIND: i64 = 1;
const PATH_ALIAS_KIND: i64 = 2;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs project register",
    usage: "obs project register [-h] [--init] [--name NAME] [--slug SLUG]",
    options: &[
        OptionSpec::switch("--init"),
        OptionSpec::text("--name"),
        OptionSpec::text("--slug"),
    ],
    positionals: &[],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let marker = marker_for_registration(&args)?;
    let settings = Settings::from_env();
    let conn = open_spool()?;

    let project = register_project(&conn, &settings, &marker)?;
    print_command_result(&project)
}

/// The nearest marker; with `--init` and none found, a new one is written in the cwd first.
fn marker_for_registration(args: &ParsedArgs) -> CliResult<ProjectMarker> {
    let mut found = find_marker_from_cwd()?;

    if args.switch("--init") && found.is_none() {
        write_new_marker(args)?;
        found = find_marker_from_cwd()?;
    }

    found.ok_or_else(|| {
        CliFailure::refusal("No observability.project.toml found. Use obs project register --init.")
    })
}

fn write_new_marker(args: &ParsedArgs) -> CliResult<()> {
    let cwd = current_directory()?;
    let name = non_empty(args.text("--name"))
        .map(str::to_owned)
        .unwrap_or_else(|| directory_name(&cwd));
    let slug = non_empty(args.text("--slug"))
        .map(str::to_owned)
        .unwrap_or_else(|| slug_from_name(&name));

    let path = cwd.join(MARKER_FILE_NAME);
    fs::write(&path, render_new_marker(&uuid7_text(), &name, &slug))
        .with_context(|| format!("unable to write {}", path.display()))?;
    Ok(())
}

/// Python `Path.cwd().name`.
#[inline]
fn directory_name(directory: &Path) -> String {
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// obs.py `re.sub(r"[^a-z0-9._-]+", "-", name.lower()).strip("-")`.
fn slug_from_name(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut is_in_replaced_run = false;

    for character in name.to_lowercase().chars() {
        if is_slug_character(character) {
            slug.push(character);
            is_in_replaced_run = false;
        } else if !is_in_replaced_run {
            slug.push('-');
            is_in_replaced_run = true;
        }
    }
    slug.trim_matches('-').to_owned()
}

#[inline]
fn is_slug_character(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

#[inline]
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// obs.py:719 `register_project`: spool the registration, return the `[project]` table.
fn register_project(
    conn: &Connection,
    settings: &Settings,
    marker: &ProjectMarker,
) -> CliResult<Value> {
    let data = json!({
        "projectId": marker.required_field("id")?.to_json(),
        "displayName": marker.required_field("name")?.to_json(),
        "slug": marker.required_field("slug")?.to_json(),
        "aliases": project_aliases(marker.root()),
    });

    let operation = build_operation("project.register", &data);
    enqueue_and_flush(conn, settings, &HubSink { settings }, &[operation])?;
    Ok(marker.project.to_json())
}

/// Hashed, never raw: the lowercased root path and, when present, the lowercased origin URL.
fn project_aliases(root: &Path) -> Vec<Value> {
    let mut aliases = vec![alias(PATH_ALIAS_KIND, &root.to_string_lossy())];

    let remote = git_value(root, &["remote", "get-url", "origin"]);
    if !remote.is_empty() {
        aliases.push(alias(REMOTE_ALIAS_KIND, &remote));
    }
    aliases
}

#[inline]
fn alias(kind: i64, identity: &str) -> Value {
    json!({ "kind": kind, "hash": sha256_hex(&identity.to_lowercase()) })
}

/// obs.py `digest(value)` (unkeyed SHA-256 hex).
#[inline]
fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_obs_py_regex() {
        assert_eq!(slug_from_name("My Project!"), "my-project");
        assert_eq!(slug_from_name("a -b"), "a--b");
        assert_eq!(
            slug_from_name("--Codex System.Harness__"),
            "codex-system.harness__"
        );
        assert_eq!(slug_from_name("Café"), "caf");
    }

    #[test]
    fn digest_is_plain_sha256_hex() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
