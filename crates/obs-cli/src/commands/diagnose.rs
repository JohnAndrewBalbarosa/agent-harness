//! `obs diagnose <error_group_id>` (obs.py:1395): one error group with its occurrences.

use crate::argv::{parse, CommandSpec};
use crate::commands::output::print_hub_response;
use crate::failure::CliResult;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs diagnose",
    usage: "obs diagnose [-h] error_group_id",
    options: &[],
    positionals: &["error_group_id"],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let error_group_id = args.positional(0).unwrap_or_default();

    print_hub_response(&format!("/v1/errors/{error_group_id}"))
}
