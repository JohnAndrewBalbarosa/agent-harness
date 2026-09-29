//! `obs query [--project <uuid> | --global] [--db <path>] [--format json|table] [--limit N] "<SQL>"`
//! (new; replaces `docker exec … psql`). Read-only; project-scoped by default via TEMP views.

use std::path::PathBuf;

use crate::argv::{parse, CommandSpec, OptionSpec, ParsedArgs};
use crate::commands::output::print_line;
use crate::failure::{CliFailure, CliResult};
use crate::project_resolution::explicit_or_marker_project_id;
use crate::query::project_views::{install_project_views, UnavailableView};
use crate::query::read_only_connection::{forbid_writes, open_read_only};
use crate::query::render::{render_json, render_table};
use crate::query::statement::run_read_only_statement;

const FORMATS: &[&str] = &["json", "table"];
const DEFAULT_ROW_LIMIT: i64 = 200;
const MAX_ROW_LIMIT: i64 = 100_000;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs query",
    usage: "obs query [-h] [--project PROJECT] [--global] [--db DB] [--format {json,table}] [--limit LIMIT] sql",
    options: &[
        OptionSpec::text("--project"),
        OptionSpec::switch("--global"),
        OptionSpec::text("--db"),
        OptionSpec::one_of("--format", FORMATS),
        OptionSpec::integer("--limit"),
    ],
    positionals: &["sql"],
};

enum QueryScope {
    Project(String),
    Global,
}

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let scope = resolve_query_scope(&args)?;
    let limit = row_limit(&args)?;
    let database = args
        .text("--db")
        .map(PathBuf::from)
        .unwrap_or_else(obs_common::default_database_path);
    let sql = args.positional(0).unwrap_or_default();

    // Mental model: open read-only → scope with TEMP views → lock the connection → run → render.
    let conn = open_read_only(&database)?;
    if let QueryScope::Project(project_id) = &scope {
        warn_unavailable_views(&install_project_views(&conn, project_id)?);
    }
    forbid_writes(&conn)?;

    let result = run_read_only_statement(&conn, sql, limit)?;
    let rendered = match args.text("--format") {
        Some("table") => render_table(&result),
        _ => render_json(&result),
    };
    print_line(&rendered)?;

    if result.is_truncated {
        eprintln!(
            "obs query: output truncated to the first {limit} rows; raise --limit to see more."
        );
    }
    Ok(())
}

/// `--global` → no views; otherwise `--project` or the cwd marker, as a canonical UUID.
fn resolve_query_scope(args: &ParsedArgs) -> CliResult<QueryScope> {
    let explicit_project = args.text("--project");

    if args.switch("--global") {
        if explicit_project.is_some() {
            return Err(CliFailure::usage(
                SPEC.prog,
                SPEC.usage,
                "argument --global: not allowed with argument --project",
            ));
        }
        return Ok(QueryScope::Global);
    }

    let project_id = explicit_or_marker_project_id(explicit_project)?;
    obs_common::normalize_uuid(&project_id)
        .map(QueryScope::Project)
        .map_err(|error| CliFailure::refusal(format!("Project ID must be a UUID: {error}")))
}

fn row_limit(args: &ParsedArgs) -> CliResult<usize> {
    let limit = args.integer("--limit").unwrap_or(DEFAULT_ROW_LIMIT);

    if !(1..=MAX_ROW_LIMIT).contains(&limit) {
        return Err(CliFailure::refusal(format!(
            "Limit must be between 1 and {MAX_ROW_LIMIT}."
        )));
    }
    Ok(usize::try_from(limit).unwrap_or(usize::MAX))
}

/// One bounded stderr line: which views fail to compile, and the first reason.
fn warn_unavailable_views(views: &[UnavailableView]) {
    let Some(first) = views.first() else {
        return;
    };
    let names: Vec<&str> = views.iter().map(|view| view.name).collect();
    eprintln!(
        "obs query: {} project view(s) unavailable ({}); first error: {}",
        views.len(),
        names.join(", "),
        first.reason
    );
}
