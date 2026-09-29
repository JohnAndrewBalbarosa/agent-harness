//! `obs errors [--project]` (obs.py:1385): the project's error groups from the hub.

use crate::argv::{parse, CommandSpec, OptionSpec};
use crate::commands::output::print_hub_response;
use crate::failure::CliResult;
use crate::project_resolution::explicit_or_marker_project_id;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs errors",
    usage: "obs errors [-h] [--project PROJECT]",
    options: &[OptionSpec::text("--project")],
    positionals: &[],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let project_id = explicit_or_marker_project_id(args.text("--project"))?;

    print_hub_response(&format!("/v1/projects/{project_id}/errors"))
}
