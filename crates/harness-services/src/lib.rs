//! `harness-services`: the agent harness's background work in one Rust binary.
//!
//! Mental model:
//! session.start hook → `ensure` (one-shot) → starts `serve` (single-instance daemon: OTLP usage receiver,
//! console window monitor, spool drain) + the hub + the orb when they are down.
//! Inside Herdr the hook starts one `herdr-subscribe` process per Herdr socket.

pub mod console_monitor;
pub mod daemon;
pub mod ensure;
pub mod herdr_subscriber;
pub mod otlp_receiver;
pub mod service_context;
pub mod spool_drain;
