//! Whole transactional ingestion contract: normalized rows, replay, rollback and tenant isolation.
use obs_hub::operations::{apply_batch, Operation};
use rusqlite::Connection;
use serde_json::{json, Value};

const PROJECT: &str = "01a0d736-8ed8-76dc-9c20-c342229f9a82";
const OTHER: &str = "01a0d736-8ed8-76dc-9c20-c342229f9a83";
const BATCH: &str = "01a0d736-8ed8-76dc-9c20-c342229f9a84";

fn operation(kind: &str, data: Value) -> Operation {
    Operation {
        operation_id: "01a0d736-8ed8-76dc-9c20-c342229f9a85".into(),
        kind: kind.into(),
        occurred_at: "2026-10-05T00:00:00.000Z".into(),
        data: data.as_object().unwrap().clone(),
    }
}

fn database() -> Connection {
    let mut connection = obs_hub::store::open_database(std::path::Path::new(":memory:")).unwrap();
    for id in [PROJECT, OTHER] {
        apply_batch(
            &mut connection,
            &[operation(
                "project.register",
                json!({"projectId":id,
            "displayName":id,"slug": format!("project-{id}"),"aliases":[]}),
            )],
        )
        .unwrap();
    }
    connection
}

fn batch() -> Operation {
    operation(
        "markdown-context.record",
        json!({"projectId":PROJECT,"batchId":BATCH,
        "nativeSession":"native-a","agent":"codex","instance":"personal","reason":"explicit_selection",
        "documents":[{"path":"docs/design.md","hash":"ab".repeat(32),"byteSize":30,
            "source":"explicit_read","coverage":"partial","outcome":"emitted",
            "startCharacter":0,"endCharacter":10,"suppliedCharacters":10}]}),
    )
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn replay_and_second_batch_share_one_document_and_version() {
    let mut connection = database();
    let mut first = batch();
    apply_batch(&mut connection, &[first.clone(), first.clone()]).unwrap();
    first.data.insert(
        "batchId".into(),
        json!("01a0d736-8ed8-76dc-9c20-c342229f9a86"),
    );
    apply_batch(&mut connection, &[first]).unwrap();
    assert_eq!(count(&connection, "markdown_documents"), 1);
    assert_eq!(count(&connection, "markdown_document_versions"), 1);
    assert_eq!(count(&connection, "markdown_context_observations"), 2);
    assert_eq!(count(&connection, "markdown_context_batches"), 2);
}

#[test]
fn invalid_metadata_rolls_back_every_row() {
    for key in ["path", "hash", "source", "coverage"] {
        let mut connection = database();
        let mut invalid = batch();
        invalid.data.get_mut("documents").unwrap()[0][key] = json!("../invalid");
        assert!(apply_batch(&mut connection, &[invalid]).is_err());
        assert_eq!(count(&connection, "markdown_context_batches"), 0);
        assert_eq!(count(&connection, "markdown_documents"), 0);
    }
}

#[test]
fn batch_identity_cannot_be_replayed_across_projects() {
    let mut connection = database();
    apply_batch(&mut connection, &[batch()]).unwrap();
    let mut wrong = batch();
    wrong.data.insert("projectId".into(), json!(OTHER));
    assert!(apply_batch(&mut connection, &[wrong]).is_err());
    assert_eq!(count(&connection, "markdown_documents"), 1);
}
