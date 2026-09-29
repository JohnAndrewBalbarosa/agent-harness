//! `obs-rs`: Rust port of a subset of `obs.py` (same stdout/stderr text, exit codes,
//! and spool writes) plus the read-only `query` subcommand over the hub SQLite file.
//!
//! This file only routes argv to a subcommand; every subcommand lives in `commands/`.

use obs_cli::{argv, commands, failure};

use std::process::ExitCode;

use argv::{split_subcommand, CommandGroup};
use failure::{CliFailure, CliResult};

/// obs.py:1272-1368 subcommand names; unported ones are reported instead of rejected as invalid.
const TOP_LEVEL: CommandGroup = CommandGroup {
    prog: argv::TOP_LEVEL_PROG,
    usage: argv::TOP_LEVEL_USAGE,
    dest: "command",
    choices: &[
        "project",
        "prompt",
        "event",
        "errors",
        "analytics",
        "trace",
        "diagnose",
        "context",
        "runtime",
        "benchmark",
        "change",
        "git",
        "flush",
        "query",
    ],
};

const PROJECT_GROUP: CommandGroup = CommandGroup {
    prog: "obs project",
    usage: "obs project [-h] {register} ...",
    dest: "project_command",
    choices: &["register"],
};

const PROMPT_GROUP: CommandGroup = CommandGroup {
    prog: "obs prompt",
    usage: "obs prompt [-h] {enrich,show} ...",
    dest: "prompt_command",
    choices: &["enrich", "show"],
};

const CHANGE_GROUP: CommandGroup = CommandGroup {
    prog: "obs change",
    usage: "obs change [-h] {plan,finish,package} ...",
    dest: "change_command",
    choices: &["plan", "finish", "package"],
};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();

    match dispatch(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => failure.report(),
    }
}

fn dispatch(arguments: &[String]) -> CliResult<()> {
    let (command, rest) = split_subcommand(arguments, &TOP_LEVEL)?;

    match command {
        "errors" => commands::errors::run(rest),
        "analytics" => commands::analytics::run(rest),
        "trace" => commands::trace::run(rest),
        "diagnose" => commands::diagnose::run(rest),
        "prompt" => dispatch_prompt(rest),
        "change" => dispatch_change(rest),
        "project" => dispatch_project(rest),
        "flush" => commands::flush::run(rest),
        "query" => commands::query::run(rest),
        other => Err(not_ported(other)),
    }
}

fn dispatch_prompt(arguments: &[String]) -> CliResult<()> {
    match split_subcommand(arguments, &PROMPT_GROUP)? {
        ("show", rest) => commands::prompt_show::run(rest),
        (other, _) => Err(not_ported(&format!("prompt {other}"))),
    }
}

fn dispatch_change(arguments: &[String]) -> CliResult<()> {
    match split_subcommand(arguments, &CHANGE_GROUP)? {
        ("plan", rest) => commands::change_plan::run(rest),
        ("finish", rest) => commands::change_finish::run(rest),
        ("package", rest) => commands::change_package::run(rest),
        (other, _) => Err(not_ported(&format!("change {other}"))),
    }
}

fn dispatch_project(arguments: &[String]) -> CliResult<()> {
    match split_subcommand(arguments, &PROJECT_GROUP)? {
        ("register", rest) => commands::project_register::run(rest),
        (other, _) => Err(not_ported(&format!("project {other}"))),
    }
}

#[inline]
fn not_ported(command: &str) -> CliFailure {
    CliFailure::refusal(format!(
        "obs-rs does not implement `{command}`; run it through obs.py."
    ))
}
