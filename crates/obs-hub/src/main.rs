//! `obs-hub` entry point: `serve` (default), `maintain`, `import-postgres`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use obs_hub::{build_router, import_postgres, maintenance, Database, HubConfig};

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_BLOCKING_THREADS: usize = 2;

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let config = HubConfig::from_env()?;

    initialize_logging(&config.log_level);

    match arguments.first().map(String::as_str) {
        None | Some("serve") => run_server(config),
        Some("maintain") => run_maintenance_once(&config),
        Some("import-postgres") => run_postgres_import(&arguments[1..], &config),
        Some("--version") => {
            println!("obs-hub {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            bail!("unknown command `{other}` (expected serve | maintain | import-postgres)")
        }
    }
}

// Mental model:
// open database → run maintenance → listen → repeat maintenance daily → stop on Ctrl+C
fn run_server(config: HubConfig) -> Result<()> {
    let runtime = build_small_runtime()?;

    runtime.block_on(async move {
        let database = Arc::new(Database::open(&config.database_path)?);
        maintenance::run_logged(&database).await;

        let listener = tokio::net::TcpListener::bind((config.host.as_str(), config.port))
            .await
            .with_context(|| format!("failed to bind {}:{}", config.host, config.port))?;
        tracing::info!(
            event = "hub.listen",
            host = %config.host,
            port = config.port,
            database = %config.database_path.display(),
            "observability hub listening"
        );

        schedule_daily_maintenance(Arc::clone(&database));

        axum::serve(listener, build_router(database))
            .with_graceful_shutdown(wait_for_shutdown_signal())
            .await
            .context("http server failed")
    })
}

fn run_maintenance_once(config: &HubConfig) -> Result<()> {
    let runtime = build_small_runtime()?;

    runtime.block_on(async {
        let database = Arc::new(Database::open(&config.database_path)?);
        let report = maintenance::run(&database).await?;
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    })
}

fn run_postgres_import(arguments: &[String], config: &HubConfig) -> Result<()> {
    let dump_directory = required_flag(arguments, "--dump")?;
    let database_path = optional_flag(arguments, "--db")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.database_path.clone());

    let report = import_postgres::run(dump_directory.as_ref(), &database_path)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// One OS thread for async work plus a tiny blocking pool for SQLite keeps idle RSS low.
fn build_small_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .build()
        .context("failed to build tokio runtime")
}

fn schedule_daily_maintenance(database: Arc<Database>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            maintenance::run_logged(&database).await;
        }
    });
}

async fn wait_for_shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(event = "hub.signal_failed", error = %error, "cannot listen for Ctrl+C");
        std::future::pending::<()>().await;
    }
    tracing::info!(event = "hub.shutdown", "observability hub stopping");
}

fn initialize_logging(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .init();
}

fn required_flag<'a>(arguments: &'a [String], name: &str) -> Result<&'a str> {
    optional_flag(arguments, name).with_context(|| format!("missing required flag {name} <value>"))
}

#[inline]
fn optional_flag<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}
