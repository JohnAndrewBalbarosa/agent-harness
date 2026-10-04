//! Per-project scoping for `obs query --project`: TEMP views with the same names, filters,
//! and joins as the Postgres `project_<uuid>` schema views (server.mjs:69-104).
//!
//! TEMP objects shadow `main` objects of the same name, so `SELECT * FROM turn_usage` becomes
//! project-scoped. View bodies therefore reference `main.<table>` explicitly (an unqualified
//! name would resolve to the view itself). Differences from server.mjs:
//! - `one_day_log_rollups` (duplicate of `daily_rollups`) is dropped;
//! - `herdr_runtime_instances` is global, so it is not shadowed (`main.herdr_runtime_instances`).

use rusqlite::Connection;

use crate::failure::CliResult;

const PROJECT_PLACEHOLDER: &str = "{project}";

pub struct ProjectView {
    pub name: &'static str,
    select_template: &'static str,
}

const fn view(name: &'static str, select_template: &'static str) -> ProjectView {
    ProjectView {
        name,
        select_template,
    }
}

pub const MARKDOWN_PROJECT_VIEWS: [ProjectView; 4] = [
    view("markdown_documents", "SELECT * FROM main.markdown_documents WHERE project_id={project}"),
    view("markdown_document_versions", "SELECT v.* FROM main.markdown_document_versions v JOIN main.markdown_documents d USING(document_id) WHERE d.project_id={project}"),
    view("markdown_context_batches", "SELECT * FROM main.markdown_context_batches WHERE project_id={project}"),
    view("markdown_context_observations", "SELECT o.* FROM main.markdown_context_observations o JOIN main.markdown_context_batches b USING(batch_id) WHERE b.project_id={project}"),
];

pub const PROJECT_VIEWS: [ProjectView; 33] = [
    view("project", "SELECT * FROM main.projects WHERE project_id={project}"),
    view("project_aliases", "SELECT * FROM main.project_aliases WHERE project_id={project}"),
    view("project_context_versions", "SELECT * FROM main.project_context_versions WHERE project_id={project}"),
    view("sessions", "SELECT * FROM main.sessions WHERE project_id={project}"),
    view("prompt_nodes", "SELECT p.* FROM main.prompt_nodes p JOIN main.sessions s USING(session_id) WHERE s.project_id={project}"),
    view("code_versions", "SELECT * FROM main.code_versions WHERE project_id={project}"),
    view("executions", "SELECT * FROM main.executions WHERE project_id={project}"),
    view("change_attributions", "SELECT a.* FROM main.change_attributions a JOIN main.executions x USING(execution_id) WHERE x.project_id={project}"),
    view("prompt_change_assessments", "SELECT * FROM main.prompt_change_assessments WHERE project_id={project}"),
    view("change_packages", "SELECT * FROM main.change_packages WHERE project_id={project}"),
    view("benchmark_programs", "SELECT * FROM main.benchmark_programs WHERE project_id={project}"),
    view("benchmark_contract_versions", "SELECT v.* FROM main.benchmark_contract_versions v JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("benchmark_metrics", "SELECT m.* FROM main.benchmark_metrics m JOIN main.benchmark_contract_versions v USING(contract_version_id) JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("benchmark_references", "SELECT DISTINCT r.* FROM main.benchmark_references r JOIN main.benchmark_metrics m USING(reference_id) JOIN main.benchmark_contract_versions v USING(contract_version_id) JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("benchmark_observations", "SELECT o.* FROM main.benchmark_observations o JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("benchmark_runs", "SELECT r.* FROM main.benchmark_runs r JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("benchmark_results", "SELECT r.* FROM main.benchmark_results r JOIN main.benchmark_runs b USING(run_id) JOIN main.benchmark_programs p USING(program_id) WHERE p.project_id={project}"),
    view("components", "SELECT * FROM main.components WHERE project_id={project}"),
    view("event_definitions", "SELECT DISTINCT d.* FROM main.event_definitions d JOIN main.events e USING(event_definition_id) JOIN main.executions x USING(execution_id) WHERE x.project_id={project}"),
    view("events", "SELECT e.* FROM main.events e JOIN main.executions x USING(execution_id) WHERE x.project_id={project}"),
    view("error_groups", "SELECT * FROM main.error_groups WHERE project_id={project}"),
    view("error_occurrences", "SELECT o.* FROM main.error_occurrences o JOIN main.error_groups g USING(error_group_id) WHERE g.project_id={project}"),
    view("event_payloads", "SELECT p.* FROM main.event_payloads p JOIN main.events e ON e.event_id=p.event_id AND e.occurred_at=p.event_occurred_at JOIN main.executions x USING(execution_id) WHERE x.project_id={project}"),
    view("daily_rollups", "SELECT * FROM main.daily_rollups WHERE project_id={project}"),
    view("herdr_pane_bindings", "SELECT * FROM main.herdr_pane_bindings WHERE project_id={project}"),
    view("agent_state_events", "SELECT * FROM main.agent_state_events WHERE project_id={project}"),
    view("herdr_runtime_events", "SELECT * FROM main.herdr_runtime_events WHERE project_id={project}"),
    view("turn_usage", "SELECT * FROM main.turn_usage WHERE project_id={project}"),
    view("prompt_native_turns", "SELECT * FROM main.prompt_native_turns WHERE project_id={project}"),
    view("tool_uses", "SELECT * FROM main.tool_uses WHERE project_id={project}"),
    view("project_handoffs", "SELECT * FROM main.project_handoffs WHERE project_id={project}"),
    view("pane_evidence_snapshots", "SELECT * FROM main.pane_evidence_snapshots WHERE project_id={project}"),
    view("turn_activity", "SELECT * FROM main.turn_activity WHERE project_id={project}"),
];

/// A view whose body does not compile (e.g. a base table absent in an older database).
/// It stays installed, so queries naming it fail loudly instead of reading unscoped rows.
pub struct UnavailableView {
    pub name: &'static str,
    pub reason: String,
}

/// `CREATE TEMP VIEW` statements for one project (`project_id` must already be a validated UUID).
pub fn project_view_statements(project_id: &str) -> Vec<(&'static str, String)> {
    let literal = sql_text_literal(project_id);

    PROJECT_VIEWS
        .iter()
        .chain(MARKDOWN_PROJECT_VIEWS.iter())
        .map(|definition| {
            let select = definition
                .select_template
                .replace(PROJECT_PLACEHOLDER, &literal);
            (
                definition.name,
                format!("CREATE TEMP VIEW {} AS {select}", definition.name),
            )
        })
        .collect()
}

/// Mental model: create every view (SQLite resolves names lazily), then compile each one to
/// report views whose base tables are missing. Creation itself failing is a hard error.
pub fn install_project_views(
    conn: &Connection,
    project_id: &str,
) -> CliResult<Vec<UnavailableView>> {
    let mut unavailable = Vec::new();
    let markdown_available = has_markdown_tables(conn)?;

    for (name, statement) in project_view_statements(project_id) {
        // Older databases contain no Markdown tables to expose. A partial migration
        // installs every Markdown view so missing joins still fail closed.
        if name.starts_with("markdown_") && !markdown_available {
            continue;
        }
        conn.execute_batch(&statement)?;
        if let Err(error) = conn.prepare(&format!("SELECT * FROM temp.{name} LIMIT 0")) {
            unavailable.push(UnavailableView {
                name,
                reason: error.to_string(),
            });
        }
    }
    Ok(unavailable)
}

fn has_markdown_tables(conn: &Connection) -> CliResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name IN
         ('markdown_documents','markdown_document_versions','markdown_context_batches','markdown_context_observations'))",
        [],
        |row| row.get(0),
    )?)
}

#[inline]
fn sql_text_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "01a0d736-8ed8-76dc-9c20-c342229f9a82";

    #[test]
    fn covers_the_server_views_minus_the_documented_exclusions() {
        let names: Vec<&str> = PROJECT_VIEWS
            .iter()
            .map(|definition| definition.name)
            .collect();

        assert_eq!(names.len(), 33);
        assert!(!names.contains(&"one_day_log_rollups"));
        assert!(!names.contains(&"herdr_runtime_instances"));
        assert!(names.contains(&"turn_usage"));
    }

    #[test]
    fn every_statement_is_scoped_and_main_qualified() {
        for (name, statement) in project_view_statements(PROJECT) {
            assert!(statement.starts_with(&format!("CREATE TEMP VIEW {name} AS SELECT")));
            assert!(
                statement.contains(&format!("project_id='{PROJECT}'")),
                "{name}"
            );
            assert!(!statement.contains(PROJECT_PLACEHOLDER));
            assert!(statement.contains("FROM main."), "{name}");
            assert!(
                !statement.contains(" JOIN sessions") && !statement.contains(" JOIN executions"),
                "{name}"
            );
        }
    }

    #[test]
    fn temp_view_shadows_main_table_for_unqualified_queries() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE turn_usage(project_id TEXT, tokens INTEGER);
             INSERT INTO turn_usage VALUES('01a0d736-8ed8-76dc-9c20-c342229f9a82', 5), ('other', 7);",
        )
        .unwrap();

        let unavailable = install_project_views(&conn, PROJECT).unwrap();

        assert_eq!(unavailable.len(), 32);
        assert!(unavailable.iter().all(|view| view.name != "turn_usage"));
        let total: i64 = conn
            .query_row("SELECT sum(tokens) FROM turn_usage", [], |row| row.get(0))
            .unwrap();
        let global: i64 = conn
            .query_row("SELECT sum(tokens) FROM main.turn_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!((total, global), (5, 12));
    }

    #[test]
    fn literal_escapes_quotes() {
        assert_eq!(sql_text_literal("a'b"), "'a''b'");
    }

    #[test]
    fn partial_markdown_schema_installs_scoped_views_and_keeps_missing_joins_closed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE markdown_documents(project_id TEXT, relative_path TEXT);
             INSERT INTO markdown_documents VALUES('{PROJECT}', 'docs/a.md'), ('other', 'docs/b.md');"
        )).unwrap();
        let unavailable = install_project_views(&conn, PROJECT).unwrap();
        assert!(!unavailable
            .iter()
            .any(|view| view.name == "markdown_documents"));
        assert!(unavailable
            .iter()
            .any(|view| view.name == "markdown_document_versions"));
        let count: i64 = conn
            .query_row("SELECT count(*) FROM markdown_documents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
}
