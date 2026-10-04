//! Sandbox conformance for filename discovery, explicit coverage and repository boundaries.
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use harness_hook::consumer::Consumer;
use harness_hook::consumers::markdown_context::{
    self as markdown,
    index::MarkdownIndex,
    observations::observe_tool_read,
    policy::allowed_repository,
    reader::{read_selected, ReadRequest},
};
use harness_hook::hook_context::HookContext;
use serde_json::{json, Value};

struct Sandbox {
    temporary: tempfile::TempDir,
    root: PathBuf,
    context: HookContext,
}

impl Sandbox {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let projects = temporary.path().join("Projects");
        let root = projects.join("repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/design.md"), "secret-document-body-é🙂").unwrap();
        let root = root.canonicalize().unwrap();
        let payload = json!({"session_id": "session-a", "turn_id": "turn-a", "cwd": root});
        let event = harness_hook::events::make_event(
            "prompt.submit",
            "codex",
            "personal",
            payload.as_object().unwrap(),
            "UserPromptSubmit",
            Default::default(),
        );
        let context = HookContext {
            event,
            native_payload: payload,
            native_bytes: Vec::new(),
            env: [
                (
                    "AH_MARKDOWN_PROJECTS_DIR".into(),
                    projects.to_string_lossy().into_owned(),
                ),
                ("AH_MARKDOWN_TELEMETRY_ENABLED".into(), "0".into()),
            ]
            .into(),
            harness_root: temporary.path().join("harness"),
        };
        Self {
            temporary,
            root,
            context,
        }
    }

    fn index(&self) -> (MarkdownIndex, bool) {
        MarkdownIndex::load(
            &markdown::cache_for(&self.context, &self.root),
            &self.root,
            false,
        )
        .unwrap()
    }

    fn read(&self, offset: usize, max: usize, force: bool) -> Vec<markdown::reader::ReadResult> {
        read_selected(
            &self.context,
            &markdown::cache_for(&self.context, &self.root),
            &self.index().0,
            &ReadRequest {
                files: vec![PathBuf::from("docs/design.md")],
                offset,
                max_characters: max,
                force,
            },
        )
        .unwrap()
    }
}

#[test]
fn prompt_supplies_relative_paths_once_and_never_bodies_or_pretool_batches() {
    let sandbox = Sandbox::new();
    let consumer = markdown::MarkdownContextConsumer;
    let first = consumer.run(&sandbox.context).unwrap().stdout;
    assert!(first.contains("docs/design.md"));
    assert!(!first.contains("secret-document-body"));
    assert!(!consumer.subscribes_to("tool.requested"));
    assert!(consumer.run(&sandbox.context).unwrap().stdout.is_empty());
}

#[test]
fn allowlist_rejects_umbrella_outside_root_invalid_regex_and_sibling_repo() {
    let mut sandbox = Sandbox::new();
    assert_eq!(
        allowed_repository(&sandbox.context, &sandbox.root.join("docs")).unwrap(),
        Some(sandbox.root.clone())
    );
    assert!(
        allowed_repository(&sandbox.context, sandbox.root.parent().unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        allowed_repository(&sandbox.context, sandbox.temporary.path())
            .unwrap()
            .is_none()
    );
    sandbox
        .context
        .env
        .insert("AH_MARKDOWN_ALLOWED_ROOT_REGEX".into(), "[".into());
    assert!(allowed_repository(&sandbox.context, &sandbox.root).is_err());
    sandbox.context.env.insert(
        "AH_MARKDOWN_ALLOWED_ROOT_REGEX".into(),
        "/different-repo$".into(),
    );
    assert!(allowed_repository(&sandbox.context, &sandbox.root)
        .unwrap()
        .is_none());
}

#[test]
fn unchanged_metadata_and_completed_readonly_tool_do_not_rebuild_the_tree() {
    let mut sandbox = Sandbox::new();
    assert!(sandbox.index().1);
    assert!(!sandbox.index().1);
    let cache = markdown::cache_for(&sandbox.context, &sandbox.root);
    let before = std::fs::read(&cache.tree).unwrap();
    sandbox.context.event.event = "tool.completed".into();
    sandbox.context.native_payload =
        json!({"tool_name": "Bash", "tool_input": {"command": "git status --short"}});
    markdown::MarkdownContextConsumer
        .run(&sandbox.context)
        .unwrap();
    assert_eq!(std::fs::read(&cache.tree).unwrap(), before);
    std::fs::write(sandbox.root.join("docs/design.md"), "changed-and-longer").unwrap();
    assert!(sandbox.index().1);
    std::fs::create_dir_all(sandbox.root.join("new/nested")).unwrap();
    std::fs::write(sandbox.root.join("new/nested/new.md"), "new").unwrap();
    assert!(sandbox
        .index()
        .0
        .paths()
        .contains(&PathBuf::from("new/nested/new.md")));
    std::fs::rename(
        sandbox.root.join("docs/design.md"),
        sandbox.root.join("docs/renamed.md"),
    )
    .unwrap();
    assert!(!sandbox
        .index()
        .0
        .paths()
        .contains(&PathBuf::from("docs/design.md")));
}

#[test]
fn partial_reads_merge_ranges_deduplicate_full_reads_and_reload_changed_versions() {
    let sandbox = Sandbox::new();
    let first = sandbox.read(0, 7, false);
    assert_eq!(first[0].coverage, "partial");
    assert_eq!(sandbox.read(0, 7, false)[0].outcome, "already_observed");
    assert_eq!(sandbox.read(7, 100, false)[0].outcome, "emitted");
    assert_eq!(sandbox.read(0, 100, false)[0].outcome, "already_observed");
    assert_eq!(sandbox.read(0, 100, true)[0].outcome, "emitted");
    std::fs::write(sandbox.root.join("docs/design.md"), "new-version").unwrap();
    assert_eq!(sandbox.read(0, 100, false)[0].content, "new-version");
}

#[test]
fn traversal_ignored_files_and_nested_repositories_cannot_be_selected() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.root.join(".gitignore"), "ignored.md\n").unwrap();
    std::fs::write(sandbox.root.join("ignored.md"), "private").unwrap();
    std::fs::create_dir_all(sandbox.root.join("nested/.git")).unwrap();
    std::fs::write(sandbox.root.join("nested/other.md"), "other").unwrap();
    let (index, _) = sandbox.index();
    assert!(!index.paths().contains(&PathBuf::from("nested/other.md")));
    let cache = markdown::cache_for(&sandbox.context, &sandbox.root);
    for path in ["../outside.md", "ignored.md", "nested/other.md"] {
        assert!(read_selected(
            &sandbox.context,
            &cache,
            &index,
            &ReadRequest {
                files: vec![PathBuf::from(path)],
                offset: 0,
                max_characters: 100,
                force: false
            }
        )
        .is_err());
    }
}

#[test]
fn native_read_tracking_requires_successful_content_evidence() {
    let mut sandbox = Sandbox::new();
    let cache = markdown::cache_for(&sandbox.context, &sandbox.root);
    sandbox.context.native_payload = json!({"tool_name": "Read", "tool_input": {"file_path": sandbox.root.join("docs/design.md")},
        "tool_response": {"is_error": true}});
    observe_tool_read(&sandbox.context, &cache, &sandbox.root).unwrap();
    assert_eq!(sandbox.read(0, 100, false)[0].outcome, "emitted");
    sandbox.context.native_payload["tool_response"] =
        Value::String("secret-document-body-é🙂".into());
    observe_tool_read(&sandbox.context, &cache, &sandbox.root).unwrap();
    assert_eq!(sandbox.read(0, 100, false)[0].outcome, "already_observed");
    std::fs::write(sandbox.root.join("docs/design.md"), "different-version").unwrap();
    sandbox.context.native_payload["tool_response"] = json!({"content": "unknown tool format"});
    observe_tool_read(&sandbox.context, &cache, &sandbox.root).unwrap();
    assert_eq!(sandbox.read(0, 100, false)[0].outcome, "emitted");
}

#[test]
fn session_instance_and_epoch_isolate_coverage_and_session_budget_is_enforced() {
    let mut sandbox = Sandbox::new();
    sandbox
        .context
        .env
        .insert("AH_MARKDOWN_SESSION_CHARS".into(), "256".into());
    std::fs::write(sandbox.root.join("docs/design.md"), "é".repeat(600)).unwrap();
    assert_eq!(sandbox.read(0, 500, false)[0].content.chars().count(), 256);
    assert_eq!(sandbox.read(256, 500, false)[0].outcome, "budget_exhausted");
    sandbox.context.env.insert(
        "AH_MARKDOWN_CONTEXT_EPOCH".into(),
        "after-compaction".into(),
    );
    assert_eq!(sandbox.read(0, 10, false)[0].outcome, "emitted");
    sandbox.context.event.instance = "cy".into();
    assert_eq!(sandbox.read(0, 10, false)[0].outcome, "emitted");
}

#[test]
fn invalid_session_state_is_observable_instead_of_silently_resetting_dedupe() {
    let sandbox = Sandbox::new();
    sandbox.read(0, 100, false);
    let cache = markdown::cache_for(&sandbox.context, &sandbox.root);
    std::fs::write(&cache.session, "invalid-json").unwrap();
    assert!(cache.update_session(|_| Ok(())).is_err());
}

#[cfg(unix)]
#[test]
fn file_symlink_cannot_escape_canonical_root() {
    let sandbox = Sandbox::new();
    let outside = sandbox.temporary.path().join("outside.md");
    std::fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, sandbox.root.join("escape.md")).unwrap();
    assert!(markdown::policy::resolve_document(&sandbox.root, Path::new("escape.md")).is_err());
}
