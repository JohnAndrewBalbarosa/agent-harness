//! Local OTLP/HTTP-JSON usage receiver (`core/usage/receiver.py`): every connected agent's built-in
//! OpenTelemetry log records → whitelisted `turn-usage.record` operations in the observability spool.
//!
//! Mental model:
//! `serve` → exclusive bind on `127.0.0.1:{OTLP_PORT|4320}` (taken → quiet `Ok`) → axum `router`
//! → `POST /v1/logs` body → `usage_records` (decode → adapter by `service.name` prefix → Claude/Codex
//! whitelist → project from the spool `context` table, else the global project) → `usage_enqueue`
//! → `FlushRequester::request_flush` (the daemon's drain loop; never a spawned process).
//!
//! Deliberate deviations from Python: writes honour `AH_OBS_SPOOL` (`ServiceContext::spool_path`),
//! requests are handled concurrently, and no flush process is spawned per POST.

pub mod adapter_dispatch;
pub mod claude_usage;
pub mod codex_usage;
pub mod exclusive_bind;
pub mod http_routes;
pub mod logs_ingestion;
pub mod otlp_decoding;
pub mod project_lookup;
pub mod python_semantics;
pub mod usage_enqueue;
pub mod usage_records;

use std::sync::Arc;

use anyhow::Context;
use obs_cli::python_compat::parse_python_int;

use crate::service_context::ServiceContext;
use crate::spool_drain::DrainHandle;

/// receiver.py:23 `HOST`.
pub const HOST: &str = "127.0.0.1";
/// receiver.py:31 `int(... .get("OTLP_PORT") or 4320)`.
pub const DEFAULT_PORT: u16 = 4320;
/// receiver.py:24 `MAX_BODY_BYTES`.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// What the receiver needs from the spool drain: one debounced "flush soon" signal per enqueue batch.
/// `DrainHandle` implements it; tests substitute a counting fake.
pub trait FlushRequester: Send + Sync + 'static {
    fn request_flush(&self);
}

impl FlushRequester for DrainHandle {
    #[inline]
    fn request_flush(&self) {
        DrainHandle::request_flush(self);
    }
}

/// Shared, read-only request state.
pub struct ReceiverState {
    pub context: ServiceContext,
    pub flush: Arc<dyn FlushRequester>,
}

/// Daemon entry: runs until the listener fails. A port that cannot be bound (another receiver,
/// Python or Rust, already owns it) is logged and returns `Ok(())`, like receiver.py:143 `serve()`.
pub async fn serve(context: ServiceContext, drain: DrainHandle) -> anyhow::Result<()> {
    // Mental model: port from the merged env → exclusive std listener → tokio listener → axum.
    let port = listen_port(&context)?;
    let listener = match exclusive_bind::bind_exclusive(port) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("usage receiver: {HOST}:{port} unavailable ({error}); not serving");
            return Ok(());
        }
    };
    let listener = tokio::net::TcpListener::from_std(listener)
        .context("usage receiver: unable to register the listener with tokio")?;
    axum::serve(listener, router(context, drain))
        .await
        .context("usage receiver: server stopped")
}

/// The receiver's routes wired to the daemon's drain.
pub fn router(context: ServiceContext, drain: DrainHandle) -> axum::Router {
    router_with_flush_requester(context, Arc::new(drain))
}

/// Same routes with any flush signal (tests count requests with a fake).
pub fn router_with_flush_requester(
    context: ServiceContext,
    flush: Arc<dyn FlushRequester>,
) -> axum::Router {
    http_routes::routes(Arc::new(ReceiverState { context, flush }))
}

/// receiver.py:30 `port()`: `OTLP_PORT` from the merged runtime env (process wins over `.env`), else
/// 4320. Python crashes on an invalid value; this returns an error instead.
pub fn listen_port(context: &ServiceContext) -> anyhow::Result<u16> {
    let Some(text) = context.env_value("OTLP_PORT") else {
        return Ok(DEFAULT_PORT);
    };
    parse_python_int(text)
        .and_then(|port| u16::try_from(port).ok())
        .with_context(|| format!("usage receiver: OTLP_PORT={text:?} is not a valid TCP port"))
}
