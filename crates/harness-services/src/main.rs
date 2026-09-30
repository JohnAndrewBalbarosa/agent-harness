//! `harness-services ensure | serve | herdr-subscribe`. Always exits 0 on expected conditions
//! (already running, nothing to do); failures are logged, never surfaced to the agent.

use std::process::ExitCode;

use harness_services::service_context::ServiceContext;
use harness_services::{daemon, ensure, herdr_subscriber};

fn main() -> ExitCode {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let context = ServiceContext::from_process();

    let outcome = match mode.as_str() {
        "ensure" => ensure::run(&context),
        "serve" => daemon::run(&context),
        "herdr-subscribe" => herdr_subscriber::run(&context),
        "--version" => {
            println!("harness-services {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(anyhow::anyhow!(
            "unknown mode `{other}` (expected ensure | serve | herdr-subscribe)"
        )),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("harness-services {mode}: {error:#}");
            ExitCode::from(2)
        }
    }
}
