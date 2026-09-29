//! Library side of `obs-rs`: the obs.py-compatible building blocks (Python-compatible text,
//! spool access, hub client, project resolution) shared with `harness-hook`.

pub mod argv;
pub mod change_assessment;
pub mod commands;
pub mod failure;
pub mod git;
pub mod hub_client;
pub mod operation;
pub mod project_marker;
pub mod project_resolution;
pub mod python_compat;
pub mod query;
pub mod settings;
pub mod spool;
