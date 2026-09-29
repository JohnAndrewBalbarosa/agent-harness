//! What commands print, in obs.py's formats:
//! - command results: `json.dumps(value, indent=2)` (ASCII-escaped)
//! - hub responses:   `json.dumps(value, ensure_ascii=False, indent=2)` (obs.py:869 `query`)

use std::io::Write;

use anyhow::Context;
use serde_json::Value;

use crate::failure::CliResult;
use crate::hub_client::fetch_hub_object;
use crate::python_compat::{to_python_json, with_platform_newlines, JsonLayout};
use crate::settings::Settings;

pub fn print_command_result(value: &Value) -> CliResult<()> {
    print_line(&to_python_json(value, JsonLayout::Indented, true))
}

/// obs.py:869 `query(path)`.
pub fn print_hub_response(path: &str) -> CliResult<()> {
    let settings = Settings::from_env();
    let response = fetch_hub_object(&settings, path)?;
    print_line(&to_python_json(&response, JsonLayout::Indented, false))
}

/// Python `print(text)`, including Windows `\r\n` translation.
pub fn print_line(text: &str) -> CliResult<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(with_platform_newlines(&format!("{text}\n")).as_bytes())
        .and_then(|()| stdout.flush())
        .context("unable to write to stdout")?;
    Ok(())
}
