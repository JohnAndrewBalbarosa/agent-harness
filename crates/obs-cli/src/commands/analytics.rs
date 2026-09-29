//! `obs analytics [--project] [--days]` (obs.py:1389): hub analytics over 1–30 days.

use crate::argv::{parse, CommandSpec, OptionSpec};
use crate::commands::output::print_hub_response;
use crate::failure::{CliFailure, CliResult};
use crate::project_resolution::explicit_or_marker_project_id;

const DEFAULT_DAYS: i64 = 7;
const MIN_DAYS: i64 = 1;
const MAX_DAYS: i64 = 30;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs analytics",
    usage: "obs analytics [-h] [--project PROJECT] [--days DAYS]",
    options: &[OptionSpec::text("--project"), OptionSpec::integer("--days")],
    positionals: &[],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let project_id = explicit_or_marker_project_id(args.text("--project"))?;
    let days = args.integer("--days").unwrap_or(DEFAULT_DAYS);

    if !is_supported_window(days) {
        return Err(CliFailure::refusal("Days must be between 1 and 30."));
    }
    print_hub_response(&format!("/v1/projects/{project_id}/analytics?days={days}"))
}

#[inline]
fn is_supported_window(days: i64) -> bool {
    (MIN_DAYS..=MAX_DAYS).contains(&days)
}
