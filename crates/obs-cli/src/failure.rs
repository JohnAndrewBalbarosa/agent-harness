//! How a command ends when it does not succeed. The variants reproduce what obs.py produces:
//! `raise SystemExit("...")` (message on stderr, exit 1) and argparse errors (usage + exit 2).

use std::process::ExitCode;

use crate::python_compat::with_platform_newlines;

pub type CliResult<T> = Result<T, CliFailure>;

const SYSTEM_EXIT_CODE: u8 = 1;
const ARGPARSE_ERROR_CODE: u8 = 2;

#[derive(Debug)]
pub enum CliFailure {
    /// obs.py `raise SystemExit("message")`.
    Refusal(String),
    /// argparse `parser.error(message)` for the parser named `prog`.
    Usage {
        prog: String,
        usage: String,
        message: String,
    },
    /// argparse `-h/--help`: usage on stdout, exit 0.
    HelpRequested { usage: String },
    /// Unexpected failure (Python would print a traceback and exit 1).
    Internal(anyhow::Error),
}

impl CliFailure {
    pub fn refusal(message: impl Into<String>) -> Self {
        Self::Refusal(message.into())
    }

    pub fn usage(prog: &str, usage: &str, message: impl Into<String>) -> Self {
        Self::Usage {
            prog: prog.to_owned(),
            usage: usage.to_owned(),
            message: message.into(),
        }
    }

    /// Prints where Python would print and returns the exit code Python would use.
    pub fn report(self) -> ExitCode {
        match self {
            Self::Refusal(message) => {
                print_stderr(&format!("{message}\n"));
                ExitCode::from(SYSTEM_EXIT_CODE)
            }
            Self::Usage {
                prog,
                usage,
                message,
            } => {
                print_stderr(&format!("usage: {usage}\n{prog}: error: {message}\n"));
                ExitCode::from(ARGPARSE_ERROR_CODE)
            }
            Self::HelpRequested { usage } => {
                print!("{}", with_platform_newlines(&format!("usage: {usage}\n")));
                ExitCode::SUCCESS
            }
            Self::Internal(error) => {
                print_stderr(&format!("obs-rs: {error:#}\n"));
                ExitCode::from(SYSTEM_EXIT_CODE)
            }
        }
    }
}

#[inline]
fn print_stderr(text: &str) {
    eprint!("{}", with_platform_newlines(text));
}

impl From<anyhow::Error> for CliFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<rusqlite::Error> for CliFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(anyhow::Error::new(error).context("spool database operation failed"))
    }
}
