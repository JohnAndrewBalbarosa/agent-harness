//! `harness-hook <agent> [<native-event>]` with the hook payload on stdin.

use std::io::{Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(agent) = arguments.first() else {
        return ExitCode::SUCCESS;
    };

    let stdin = read_all_stdin();
    let output =
        harness_hook::router::run_hook(agent, arguments.get(1).map(String::as_str), &stdin);

    write_stdout(&output);
    ExitCode::SUCCESS
}

fn read_all_stdin() -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buffer);
    buffer
}

fn write_stdout(output: &str) {
    if output.is_empty() {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(output.as_bytes());
    let _ = stdout.flush();
}
