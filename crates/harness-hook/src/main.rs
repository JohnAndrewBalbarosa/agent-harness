//! `harness-hook <agent> [<native-event>]` with the hook payload on stdin.

use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

/// Hook payloads are small JSON documents; anything larger is truncated (and then fails to parse).
const MAX_STDIN_BYTES: u64 = 8 * 1024 * 1024;
/// An agent that never closes stdin must not hang the hook.
const STDIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Not valid UTF-8: the router reports `hook.payload.invalid` and prints the safe default.
const UNREADABLE_STDIN: &[u8] = &[0xff];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("markdown") {
        return harness_hook::consumers::markdown_context::cli::run(&arguments[1..]);
    }
    let Some(agent) = arguments.first() else {
        return ExitCode::SUCCESS;
    };

    let stdin = read_all_stdin();
    let output =
        harness_hook::router::run_hook(agent, arguments.get(1).map(String::as_str), &stdin);

    write_stdout(&output);
    ExitCode::SUCCESS
}

// Mental model: read on a helper thread (bounded size) → wait at most STDIN_TIMEOUT → otherwise invalid.
fn read_all_stdin() -> Vec<u8> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = std::io::stdin()
            .take(MAX_STDIN_BYTES)
            .read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
    receiver
        .recv_timeout(STDIN_TIMEOUT)
        .unwrap_or_else(|_| UNREADABLE_STDIN.to_vec())
}

fn write_stdout(output: &str) {
    if output.is_empty() {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(output.as_bytes());
    let _ = stdout.flush();
}
