//! `_fan_out` / `_invoke`: every subscribed consumer runs on its own thread; the router waits for each one
//! until its own deadline (start + timeout). A consumer that misses its deadline is abandoned (it keeps
//! running until the process exits) and reported as exit 124; a panicking or `Err` consumer is exit 1.
//! Results keep registry order, like `[future.result() for future in futures]`.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::lifecycle_records;
use crate::adapters::bounded_stdout;
use crate::consumer::{Consumer, ConsumerResult};
use crate::hook_context::HookContext;

/// `subprocess` exit code semantics kept from the Python router.
pub const TIMEOUT_EXIT_CODE: i32 = 124;
pub const LAUNCH_FAILED_EXIT_CODE: i32 = 127;
pub const FAILURE_EXIT_CODE: i32 = 1;

/// A consumer thread that has been started and not yet collected.
struct RunningConsumer {
    name: &'static str,
    timeout: Duration,
    started: Instant,
    finished: Option<Receiver<FinishedConsumer>>,
}

/// What a consumer thread reports back.
struct FinishedConsumer {
    exit_code: i32,
    stdout: String,
    duration: Duration,
}

// Mental model: start all threads first (parallel), then collect in registry order, each wait bounded by
// that consumer's own deadline.
pub fn run_consumers(
    consumers: Vec<Box<dyn Consumer>>,
    context: &Arc<HookContext>,
    log: &Path,
) -> Vec<ConsumerResult> {
    let running: Vec<RunningConsumer> = consumers
        .into_iter()
        .map(|consumer| start_consumer(consumer, context, log))
        .collect();

    running
        .into_iter()
        .map(|consumer| collect_consumer(consumer, log))
        .collect()
}

fn start_consumer(
    consumer: Box<dyn Consumer>,
    context: &Arc<HookContext>,
    log: &Path,
) -> RunningConsumer {
    let name = consumer.name();
    let timeout = consumer.timeout();
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    let shared_context = Arc::clone(context);

    let spawned = thread::Builder::new()
        .name(format!("consumer-{name}"))
        .spawn(move || {
            let finished = run_guarded(consumer.as_ref(), &shared_context, started);
            let _ = sender.send(finished);
        });

    let finished = match spawned {
        Ok(_) => Some(receiver),
        Err(_) => {
            lifecycle_records::consumer_launch_failed(log, name, "OSError");
            None
        }
    };
    RunningConsumer {
        name,
        timeout,
        started,
        finished,
    }
}

/// Runs one consumer, turning `Err` and panics into exit 1 and bounding stdout like the Python router.
fn run_guarded(
    consumer: &dyn Consumer,
    context: &HookContext,
    started: Instant,
) -> FinishedConsumer {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| consumer.run(context)));
    let (exit_code, stdout) = match outcome {
        Ok(Ok(result)) => (result.exit_code, bounded_stdout(&result.stdout)),
        Ok(Err(_)) | Err(_) => (FAILURE_EXIT_CODE, String::new()),
    };
    FinishedConsumer {
        exit_code,
        stdout,
        duration: started.elapsed(),
    }
}

fn collect_consumer(consumer: RunningConsumer, log: &Path) -> ConsumerResult {
    let Some(receiver) = consumer.finished else {
        return ConsumerResult::failed(consumer.name, LAUNCH_FAILED_EXIT_CODE);
    };
    let remaining = consumer.timeout.saturating_sub(consumer.started.elapsed());

    match receiver.recv_timeout(remaining) {
        Ok(finished) => {
            lifecycle_records::consumer_exited(
                log,
                consumer.name,
                finished.exit_code,
                finished.duration,
            );
            ConsumerResult {
                name: consumer.name,
                exit_code: finished.exit_code,
                stdout: finished.stdout,
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            lifecycle_records::consumer_timeout(log, consumer.name, consumer.timeout);
            ConsumerResult::failed(consumer.name, TIMEOUT_EXIT_CODE)
        }
        Err(RecvTimeoutError::Disconnected) => {
            lifecycle_records::consumer_exited(
                log,
                consumer.name,
                FAILURE_EXIT_CODE,
                Duration::ZERO,
            );
            ConsumerResult::failed(consumer.name, FAILURE_EXIT_CODE)
        }
    }
}
