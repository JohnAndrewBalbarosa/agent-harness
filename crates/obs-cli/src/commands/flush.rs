//! `obs flush` (obs.py:1461): deliver the oldest spooled batch, print `{"flushed": n}`.

use serde_json::json;

use crate::argv::{parse, CommandSpec};
use crate::commands::output::print_line;
use crate::failure::CliResult;
use crate::python_compat::{to_python_json, JsonLayout};
use crate::settings::Settings;
use crate::spool::flush::{flush_oldest_batch, HubSink};
use crate::spool::open::open_spool;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs flush",
    usage: "obs flush [-h]",
    options: &[],
    positionals: &[],
};

pub fn run(arguments: &[String]) -> CliResult<()> {
    parse(&SPEC, arguments)?;
    let settings = Settings::from_env();
    let conn = open_spool()?;

    let flushed = flush_oldest_batch(
        &conn,
        &settings,
        &HubSink {
            settings: &settings,
        },
    )?;
    print_line(&to_python_json(
        &json!({ "flushed": flushed }),
        JsonLayout::Spaced,
        true,
    ))
}
