//! interval_supervisor.py:175 `main`: one supervised run from prepared paths to exit code.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use obs_cli::python_compat::{to_python_json, with_platform_newlines, JsonLayout};
use serde_json::{Map, Value};

use crate::bounded_log::{persist_tail, write_text, SharedLog};
use crate::child_process::{process_exit_code, python_returncode, spawn_child};
use crate::cli_arguments::SupervisorArguments;
use crate::completion_record::{
    completion_record, start_failure_record, ChildOutcome, RecordLayout, StartFailure,
};
use crate::interrupt_signal::{install_interrupt_handler, interrupt_requested};
use crate::observability_events::ObservabilityEvents;
use crate::path_resolution::python_resolve;
use crate::state_file::{merged_final_state, running_state, sanitize_label, write_state, LogFiles};
use crate::stream_drain::spawn_drain;

/// How often the loop checks for exit / interrupt between interval checkpoints.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// interval_supervisor.py:286 `join(timeout=2)` per drain thread.
const DRAIN_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
/// After Ctrl+C the child (which shares the console / process group) gets this long to exit on
/// its own before it is killed.
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
/// Python `return 1` when `Popen` raised.
const START_FAILURE_EXIT_CODE: i32 = 1;

/// Where this run writes and how it is identified.
struct RunLayout {
    root: PathBuf,
    safe_label: String,
    files: LogFiles,
}

/// `time.time()` at start (for `startedAt`) plus a monotonic clock for the duration.
struct RunClock {
    started: Instant,
    started_at_epoch_seconds: f64,
}

/// The running child and everything the loop updates while it runs.
struct RunningChild {
    child: Child,
    stdout_lines: SharedLog,
    stderr_lines: SharedLog,
    drains_finished: Receiver<()>,
    drain_count: usize,
    running_state: Map<String, Value>,
}

/// Mental model: prepare paths → announce → start the child (or report why it could not start)
/// → supervise with interval checkpoints → finalize logs, state, completion record, events.
/// Returns the process exit code (the child's `returncode`, or 1 when it never started).
pub fn run(arguments: &SupervisorArguments) -> anyhow::Result<i32> {
    let layout = prepare_run_layout(arguments)?;
    let clock = start_clock();
    install_interrupt_handler();

    let mut events = ObservabilityEvents::start(&arguments.project_id);
    events.process_started(&layout.safe_label, arguments.interval_seconds);

    let child = match spawn_child(&arguments.command, &layout.root) {
        Ok(child) => child,
        Err(failure) => {
            finalize_start_failure(arguments, &layout, &clock, &failure, &mut events);
            events.close();
            return Ok(START_FAILURE_EXIT_CODE);
        }
    };

    let mut running = start_supervision(child, arguments, &layout, &clock)?;
    let status = supervise_until_exit(&mut running, arguments, &layout, &mut events);
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            // Best effort: never leave an unsupervised child behind.
            let _ = running.child.kill();
            return Err(error).context("unable to wait for the supervised command");
        }
    };

    let exit_code = finalize_completion(&running, status, arguments, &layout, &clock, &mut events);
    events.close();
    Ok(process_exit_code(exit_code))
}

/// `root = Path(cwd).resolve()`, `log_dir = (root / log_dir).resolve()` + `mkdir(parents=True)`.
fn prepare_run_layout(arguments: &SupervisorArguments) -> anyhow::Result<RunLayout> {
    let cwd = match &arguments.cwd {
        Some(cwd) => PathBuf::from(cwd),
        None => std::env::current_dir().context("unable to read the current directory")?,
    };
    let root = python_resolve(&cwd);
    let log_dir = python_resolve(&root.join(&arguments.log_dir));
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("unable to create log directory {}", log_dir.display()))?;

    let safe_label = sanitize_label(&arguments.label);
    let files = LogFiles::for_label(&log_dir, &safe_label);
    Ok(RunLayout {
        root,
        safe_label,
        files,
    })
}

#[inline]
fn start_clock() -> RunClock {
    let started_at_epoch_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or_default();
    RunClock {
        started: Instant::now(),
        started_at_epoch_seconds,
    }
}

/// interval_supervisor.py:214-239: record with `exit_code: null`, bounded files, one JSON line.
fn finalize_start_failure(
    arguments: &SupervisorArguments,
    layout: &RunLayout,
    clock: &RunClock,
    failure: &StartFailure,
    events: &mut ObservabilityEvents,
) {
    let record = start_failure_record(
        &layout.safe_label,
        clock.started.elapsed().as_secs_f64(),
        failure,
        arguments.max_line_chars,
        &record_layout(arguments, &layout.files),
    );
    let detail = record
        .get("tail")
        .and_then(|tail| tail.get(0))
        .and_then(Value::as_str)
        .unwrap_or_default();

    report_if_failed(write_text(&layout.files.stderr, &format!("{detail}\n")));
    report_if_failed(write_text(&layout.files.stdout, ""));
    report_if_failed(write_state(&layout.files.state, &record));
    print_completion_record(&record);
    events.start_failed(&layout.safe_label, &failure.exception_type);
}

/// Running state on disk, then one drain thread per pipe.
fn start_supervision(
    mut child: Child,
    arguments: &SupervisorArguments,
    layout: &RunLayout,
    clock: &RunClock,
) -> anyhow::Result<RunningChild> {
    let running_state = running_state(
        &layout.safe_label,
        child.id(),
        arguments.interval_seconds,
        &layout.files,
        clock.started_at_epoch_seconds,
    );
    report_if_failed(write_state(&layout.files.state, &running_state));

    let stdout_lines = SharedLog::new(arguments.max_log_lines);
    let stderr_lines = SharedLog::new(arguments.max_log_lines);
    let (finished_sender, drains_finished) = mpsc::channel();
    let mut drain_count = 0;

    if let Some(stdout) = child.stdout.take() {
        spawn_drain(
            stdout,
            stdout_lines.clone(),
            arguments.max_line_chars,
            finished_sender.clone(),
        )
        .context("unable to start the stdout drain thread")?;
        drain_count += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_drain(
            stderr,
            stderr_lines.clone(),
            arguments.max_line_chars,
            finished_sender,
        )
        .context("unable to start the stderr drain thread")?;
        drain_count += 1;
    }

    Ok(RunningChild {
        child,
        stdout_lines,
        stderr_lines,
        drains_finished,
        drain_count,
        running_state,
    })
}

/// Mental model: poll for exit → on interrupt terminate the child → every interval rewrite the
/// bounded logs and running state and emit a checkpoint (Python `wait(timeout=interval)`).
fn supervise_until_exit(
    running: &mut RunningChild,
    arguments: &SupervisorArguments,
    layout: &RunLayout,
    events: &mut ObservabilityEvents,
) -> io::Result<ExitStatus> {
    let interval = Duration::from_secs(arguments.interval_seconds);
    let mut next_checkpoint = Instant::now() + interval;

    loop {
        if let Some(status) = running.child.try_wait()? {
            return Ok(status);
        }
        if interrupt_requested() {
            return terminate_child(&mut running.child);
        }
        if Instant::now() >= next_checkpoint {
            write_checkpoint(running, arguments, layout, events);
            next_checkpoint = Instant::now() + interval;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Keeps bounded diagnostics inspectable during long runs without streaming them anywhere.
fn write_checkpoint(
    running: &RunningChild,
    arguments: &SupervisorArguments,
    layout: &RunLayout,
    events: &mut ObservabilityEvents,
) {
    persist_both_tails(running, &layout.files);
    report_if_failed(write_state(&layout.files.state, &running.running_state));
    events.checkpoint(
        &layout.safe_label,
        running.child.id(),
        arguments.interval_seconds,
    );
}

/// Grace period for a child that received the same Ctrl+C, then a hard kill.
fn terminate_child(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + TERMINATION_GRACE;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        thread::sleep(POLL_INTERVAL);
    }
    // The child may exit between the last poll and the kill; `wait` still reports its status.
    let _ = child.kill();
    child.wait()
}

/// interval_supervisor.py:286-338. Returns the child's Python `returncode`.
fn finalize_completion(
    running: &RunningChild,
    status: ExitStatus,
    arguments: &SupervisorArguments,
    layout: &RunLayout,
    clock: &RunClock,
    events: &mut ObservabilityEvents,
) -> i64 {
    wait_for_drains(&running.drains_finished, running.drain_count);
    let stdout_lines = running.stdout_lines.snapshot();
    let stderr_lines = running.stderr_lines.snapshot();
    report_if_failed(persist_tail(&layout.files.stdout, &stdout_lines));
    report_if_failed(persist_tail(&layout.files.stderr, &stderr_lines));

    let exit_code = python_returncode(status);
    let record = completion_record(
        &ChildOutcome {
            label: &layout.safe_label,
            exit_code,
            duration_seconds: clock.started.elapsed().as_secs_f64(),
            stdout_lines: &stdout_lines,
            stderr_lines: &stderr_lines,
        },
        &record_layout(arguments, &layout.files),
    );
    let final_state = merged_final_state(&running.running_state, &record);
    report_if_failed(write_state(&layout.files.state, &final_state));
    print_completion_record(&record);

    if exit_code == 0 {
        events.completed(&layout.safe_label, exit_code, &record);
    } else {
        events.failed(&layout.safe_label, exit_code, &record);
    }
    exit_code
}

/// A grandchild that inherited a pipe can keep it open; stop waiting after the Python timeout.
fn wait_for_drains(finished: &Receiver<()>, expected: usize) {
    for _ in 0..expected {
        if finished.recv_timeout(DRAIN_JOIN_TIMEOUT).is_err() {
            return;
        }
    }
}

#[inline]
fn persist_both_tails(running: &RunningChild, files: &LogFiles) {
    report_if_failed(persist_tail(
        &files.stdout,
        &running.stdout_lines.snapshot(),
    ));
    report_if_failed(persist_tail(
        &files.stderr,
        &running.stderr_lines.snapshot(),
    ));
}

#[inline]
fn record_layout<'a>(arguments: &SupervisorArguments, files: &'a LogFiles) -> RecordLayout<'a> {
    RecordLayout {
        success_tail_lines: arguments.success_tail_lines,
        failure_tail_lines: arguments.failure_tail_lines,
        stdout_path: &files.stdout,
        stderr_path: &files.stderr,
    }
}

/// `print(json.dumps(record, separators=(",", ":")))`: the ONLY stdout output.
fn print_completion_record(record: &Map<String, Value>) {
    let line = to_python_json(&Value::Object(record.clone()), JsonLayout::Compact, true);
    if let Err(error) = print_line(&line) {
        eprintln!("interval-supervisor: unable to print the completion record: {error}");
    }
}

/// Python `print(text)`, including Windows `\r\n` translation.
#[inline]
fn print_line(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(with_platform_newlines(&format!("{text}\n")).as_bytes())?;
    stdout.flush()
}

/// Diagnostic-file writes never abort supervision; each failure is reported on stderr.
#[inline]
fn report_if_failed(result: anyhow::Result<()>) {
    if let Err(error) = result {
        eprintln!("interval-supervisor: {error:#}");
    }
}
