//! The per-native-session chain kept in the spool `context` table (hook.py `process`, first half):
//! every hook re-announces the agent; the first hook of a native session opens an observability
//! session; a `SessionStart` on a known native session (resume) re-announces the same session.

use obs_cli::python_compat::{python_float_repr, python_utc_now, uuid7_text};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};

use super::agent_identity::agent_instance_id;
use super::hook_scope::HookScope;
use super::operations::operation;
use super::payload_fields::{is_present, optional_json};
use super::project_registration::ProjectIdentity;
use super::spool_store::Spool;

const SOURCE_KIND_AGENT: i64 = 1;
const SESSION_KIND_AGENT: i64 = 1;
const CONTEXT_COLUMNS: &str = "session_id,prompt_id,execution_id,trace_id,last_prompt_id,native_turn,prompt_ordinal,sequence_no,wait_count,background_count,behavior_audit_enabled";

/// One `context` row (`SELECT * ... WHERE native_session=?`), columns hook.py reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub execution_id: Option<String>,
    pub trace_id: Option<String>,
    pub last_prompt_id: Option<String>,
    pub native_turn: Option<String>,
    pub prompt_ordinal: i64,
    pub sequence_no: i64,
    pub wait_count: i64,
    pub background_count: i64,
    pub behavior_audit_enabled: i64,
}

impl ContextRow {
    /// Python truthiness of `row["execution_id"]`.
    #[inline]
    pub fn has_live_execution(&self) -> bool {
        is_present(self.execution_id.as_deref())
    }

    #[inline]
    pub fn execution_id_json(&self) -> Value {
        optional_json(self.execution_id.as_deref())
    }

    #[inline]
    pub fn prompt_id_json(&self) -> Value {
        optional_json(self.prompt_id.as_deref())
    }

    #[inline]
    pub fn trace_id_json(&self) -> Value {
        optional_json(self.trace_id.as_deref())
    }

    #[inline]
    pub fn native_turn_json(&self) -> Value {
        optional_json(self.native_turn.as_deref())
    }
}

/// Mental model: announce agent → open (first sight) or resume (SessionStart) → the current row.
pub fn attach(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    operations: &mut Vec<Value>,
) -> anyhow::Result<ContextRow> {
    let agent_id = agent_instance_id(&scope.instance_key);
    operations.push(agent_register_operation(&agent_id, &scope.instance_key));

    match load_context_row(spool.conn(), &scope.native_session)? {
        None => open_session(scope, spool, project, &agent_id, operations),
        Some(row) if scope.hook_event == "SessionStart" => {
            resume_session(scope, spool, project, &agent_id, &row, operations)?;
            Ok(row)
        }
        Some(row) => Ok(row),
    }
}

pub fn load_context_row(
    conn: &Connection,
    native_session: &str,
) -> rusqlite::Result<Option<ContextRow>> {
    conn.query_row(
        &format!("SELECT {CONTEXT_COLUMNS} FROM context WHERE native_session=?"),
        params![native_session],
        context_row_from,
    )
    .optional()
}

fn open_session(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    agent_id: &str,
    operations: &mut Vec<Value>,
) -> anyhow::Result<ContextRow> {
    let session_id = uuid7_text();
    operations.push(session_start_operation(
        scope,
        spool,
        project,
        agent_id,
        &session_id,
    )?);
    spool.conn().execute(
        "INSERT OR REPLACE INTO context(native_session,project_id,session_id,cwd,updated_at) VALUES(?,?,?,?,?)",
        params![scope.native_session, project.id_text, session_id, scope.cwd_text(), python_utc_now()],
    )?;
    load_context_row(spool.conn(), &scope.native_session)?
        .ok_or_else(|| anyhow::anyhow!("context row vanished after insert"))
}

fn resume_session(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    agent_id: &str,
    row: &ContextRow,
    operations: &mut Vec<Value>,
) -> anyhow::Result<()> {
    operations.push(session_start_operation(
        scope,
        spool,
        project,
        agent_id,
        &row.session_id,
    )?);
    spool.conn().execute(
        "UPDATE context SET cwd=?,updated_at=? WHERE native_session=?",
        params![scope.cwd_text(), python_utc_now(), scope.native_session],
    )?;
    Ok(())
}

fn agent_register_operation(agent_id: &str, instance_key: &str) -> Value {
    operation(
        "agent.register",
        json!({
            "agentInstanceId": agent_id,
            "instanceKey": instance_key,
            "sourceKind": SOURCE_KIND_AGENT,
            "displayName": format!("Codex {instance_key}"),
        }),
        None,
    )
}

/// `sessionFingerprint = digest(f"{instance_key}|{native_session}", keyed=True)`.
fn session_start_operation(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    agent_id: &str,
    session_id: &str,
) -> anyhow::Result<Value> {
    let fingerprint =
        spool.keyed_digest(&format!("{}|{}", scope.instance_key, scope.native_session))?;
    Ok(operation(
        "session.start",
        json!({
            "sessionId": session_id,
            "projectId": project.id,
            "agentInstanceId": agent_id,
            "sessionKind": SESSION_KIND_AGENT,
            "sessionFingerprint": fingerprint,
        }),
        None,
    ))
}

fn context_row_from(row: &Row) -> rusqlite::Result<ContextRow> {
    Ok(ContextRow {
        session_id: column_text(row, 0)?.unwrap_or_default(),
        prompt_id: column_text(row, 1)?,
        execution_id: column_text(row, 2)?,
        trace_id: column_text(row, 3)?,
        last_prompt_id: column_text(row, 4)?,
        native_turn: column_text(row, 5)?,
        prompt_ordinal: column_int(row, 6)?,
        sequence_no: column_int(row, 7)?,
        wait_count: column_int(row, 8)?,
        background_count: column_int(row, 9)?,
        behavior_audit_enabled: column_int(row, 10)?,
    })
}

/// SQLite is dynamically typed and Python writers bind whatever they hold; read any storage class as text.
fn column_text(row: &Row, index: usize) -> rusqlite::Result<Option<String>> {
    Ok(match row.get_ref(index)? {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string()),
        ValueRef::Real(number) => Some(python_float_repr(number)),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
    })
}

/// Python `int(row[column])` for the NOT NULL counters.
fn column_int(row: &Row, index: usize) -> rusqlite::Result<i64> {
    Ok(match row.get_ref(index)? {
        ValueRef::Integer(number) => number,
        ValueRef::Real(number) => number.trunc() as i64,
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).trim().parse().unwrap_or(0),
        ValueRef::Null | ValueRef::Blob(_) => 0,
    })
}
