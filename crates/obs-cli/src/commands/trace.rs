//! `obs trace <trace_id>` (obs.py:1394): every event recorded under one trace.

use crate::argv::{parse, CommandSpec};
use crate::commands::output::print_hub_response;
use crate::failure::CliResult;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs trace",
    usage: "obs trace [-h] trace_id",
    options: &[],
    positionals: &["trace_id"],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let trace_id = args.positional(0).unwrap_or_default();

    print_hub_response(&format!("/v1/traces/{trace_id}"))
}
