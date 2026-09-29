//! `harness-hook`: the single entry point for every agent hook. It normalizes the agent's native payload
//! into a common event, runs every subscribed consumer in-process (threads, per-consumer timeouts), and
//! prints the agent's native response. It never fails its agent: every error path prints the adapter's
//! safe default and exits 0.
//!
//! Mental model:
//! stdin payload → adapter.normalize → Event → subscribed consumers (parallel threads)
//! → adapter.respond(results) → stdout

pub mod adapters;
pub mod consumer;
pub mod consumers;
pub mod events;
pub mod hook_context;
pub mod router;
pub mod runtime_env;
pub mod shared_state;
pub mod transcripts;
