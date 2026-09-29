//! `obs prompt show <prompt_id>` (obs.py:1383): the stored prompt record from the hub.

use crate::argv::{parse, CommandSpec};
use crate::commands::output::print_hub_response;
use crate::failure::CliResult;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs prompt show",
    usage: "obs prompt show [-h] prompt_id",
    options: &[],
    positionals: &["prompt_id"],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let prompt_id = args.positional(0).unwrap_or_default();

    print_hub_response(&format!("/v1/prompts/{prompt_id}"))
}
