//! One module per ported subcommand. Each exposes `run(arguments)` taking the argv that
//! follows its subcommand words.

pub mod analytics;
pub mod change_finish;
pub mod change_package;
pub mod change_plan;
pub mod diagnose;
pub mod errors;
pub mod flush;
mod output;
pub mod project_register;
pub mod prompt_show;
pub mod query;
pub mod trace;
