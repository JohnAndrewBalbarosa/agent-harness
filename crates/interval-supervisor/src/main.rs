//! `interval-supervisor`: Rust port of `tools/observability-client/interval_supervisor.py`.
//!
//! The child process never streams into the agent session. Its output is drained into bounded
//! in-memory tails, persisted as bounded diagnostic files on every interval, and reduced to ONE
//! compact JSON completion record on stdout. Successful runs expose only their final meaningful
//! lines; failed runs expose a bounded diagnostic tail from both streams.
//!
//! Deliberate fix: `--project-id` events go through the obs spool (`obs_cli` operations) instead
//! of the Python `sdk.python` import that no longer exists.

mod bounded_log;
mod child_process;
mod cli_arguments;
mod completion_record;
mod failure_summary;
mod interrupt_signal;
mod line_normalizer;
mod observability_events;
mod path_resolution;
mod python_text;
mod state_file;
mod stream_drain;
mod supervision;

use std::process::ExitCode;

use cli_arguments::{parse_arguments, ArgumentOutcome};

/// Python `SystemExit(1)` for anything that goes wrong outside the child (unwritable log dir...).
const INTERNAL_FAILURE_EXIT_CODE: u8 = 1;

fn main() -> ExitCode {
    // Mental model: argv → (usage error | help | supervised run) → the process exit code the
    // Python tool would have produced. A supervised run ends via `std::process::exit` so the
    // child's full exit code (including negative / >255 values) is forwarded unchanged.
    let argv: Vec<String> = std::env::args().collect();
    let arguments = match parse_arguments(&argv) {
        ArgumentOutcome::Run(arguments) => arguments,
        ArgumentOutcome::Exit(code) => return code,
    };

    match supervision::run(&arguments) {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(error) => {
            eprintln!("interval-supervisor: {error:#}");
            ExitCode::from(INTERNAL_FAILURE_EXIT_CODE)
        }
    }
}
