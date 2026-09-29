//! Tables migrated from Postgres, in an order where every referenced table precedes its
//! referrers. Self-references (prompt_nodes, executions) are satisfied by deferred FK checks.
//!
//! Skipped on purpose: `schema_migrations` (SQLite seeds its own), `project_schemas` and all
//! `project_*` schemas (per-project Postgres views), event partitions (the `events` parent
//! already returns every partition's rows), views.

pub const IMPORT_ORDER: [&str; 34] = [
    "projects",
    "project_aliases",
    "project_context_versions",
    "agent_instances",
    "sessions",
    "prompt_nodes",
    "code_versions",
    "executions",
    "change_attributions",
    "components",
    "event_definitions",
    "events",
    "error_groups",
    "error_occurrences",
    "event_payloads",
    "daily_rollups",
    "change_packages",
    "prompt_change_assessments",
    "benchmark_references",
    "benchmark_programs",
    "benchmark_contract_versions",
    "benchmark_metrics",
    "benchmark_observations",
    "benchmark_runs",
    "benchmark_results",
    "herdr_runtime_instances",
    "herdr_pane_bindings",
    "agent_state_events",
    "herdr_runtime_events",
    "turn_usage",
    "project_handoffs",
    "pane_evidence_snapshots",
    "prompt_native_turns",
    "tool_uses",
];

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::collections::HashSet;

    /// SQLite tables that exist in the target schema but are not data to migrate.
    const NOT_IMPORTED_TARGET_TABLES: [&str; 1] = ["schema_migrations"];

    fn schema_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../store/schema.sql"))
            .unwrap();
        connection
    }

    fn referenced_tables(connection: &Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1)")
            .unwrap();
        statement
            .query_map([table], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|name| name.unwrap())
            .collect()
    }

    #[test]
    fn every_foreign_key_target_is_imported_earlier_or_is_self() {
        let connection = schema_connection();

        for (position, table) in IMPORT_ORDER.iter().enumerate() {
            for parent in referenced_tables(&connection, table) {
                let parent_position = IMPORT_ORDER
                    .iter()
                    .position(|candidate| *candidate == parent)
                    .unwrap_or_else(|| panic!("{table} references non-imported {parent}"));
                assert!(
                    parent_position <= position,
                    "{table} is imported before its parent {parent}"
                );
            }
        }
    }

    #[test]
    fn order_covers_every_target_data_table_exactly_once() {
        let connection = schema_connection();
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let target_tables: HashSet<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|name| name.unwrap())
            .filter(|name| !NOT_IMPORTED_TARGET_TABLES.contains(&name.as_str()))
            .collect();
        let ordered: HashSet<String> = IMPORT_ORDER.iter().map(|name| name.to_string()).collect();

        assert_eq!(
            ordered.len(),
            IMPORT_ORDER.len(),
            "duplicate table in IMPORT_ORDER"
        );
        assert_eq!(ordered, target_tables);
    }
}
