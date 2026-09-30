//! argparse-compatible CLI (interval_supervisor.py:27-74).
//!
//! argparse rules reproduced: options in any order, `--flag=value`, unique-prefix abbreviations,
//! `int` validation in argv order, last occurrence wins, `-h/--help`, the REMAINDER positional that
//! swallows everything from the first non-option token (so `--` is kept as `command[0]`), then
//! required → unrecognized → the script's own `parser.error` checks. Errors exit 2 with
//! `usage: ...` + `<prog>: error: <message>` on stderr.
//!
//! Deviation: the usage line is never wrapped to the terminal width.

use std::path::Path;
use std::process::ExitCode;

use obs_cli::python_compat::{parse_python_int, python_repr_str, with_platform_newlines};

const ARGPARSE_ERROR_EXIT_CODE: u8 = 2;
const DEFAULT_PROG: &str = "interval-supervisor";
const COMMAND_TERMINATOR: &str = "--";
const HELP_FLAGS: [&str; 2] = ["-h", "--help"];
const DESCRIPTION: &str = "Run a command and return one bounded JSON completion summary";

const DEFAULT_INTERVAL_SECONDS: i64 = 30;
const MINIMUM_INTERVAL_SECONDS: i64 = 5;
const DEFAULT_LOG_DIR: &str = "var/logs";
const DEFAULT_MAX_LOG_LINES: i64 = 200;
const DEFAULT_MAX_LINE_CHARS: i64 = 2000;
const DEFAULT_SUCCESS_TAIL_LINES: i64 = 2;
const DEFAULT_FAILURE_TAIL_LINES: i64 = 20;

/// One `parser.add_argument` for an option that takes a value.
struct OptionSpec {
    flag: &'static str,
    metavar: &'static str,
    is_integer: bool,
    help: &'static str,
}

static OPTIONS: [OptionSpec; 9] = [
    OptionSpec::text("--label", "LABEL", ""),
    OptionSpec::integer("--interval", "INTERVAL", "seconds between bounded checks"),
    OptionSpec::text("--log-dir", "LOG_DIR", ""),
    OptionSpec::text("--cwd", "CWD", ""),
    OptionSpec::text("--project-id", "PROJECT_ID", ""),
    OptionSpec::integer(
        "--max-log-lines",
        "MAX_LOG_LINES",
        "maximum retained lines for each output stream",
    ),
    OptionSpec::integer(
        "--max-line-chars",
        "MAX_LINE_CHARS",
        "maximum characters per line",
    ),
    OptionSpec::integer(
        "--success-tail-lines",
        "SUCCESS_TAIL_LINES",
        "meaningful lines returned for a successful command",
    ),
    OptionSpec::integer(
        "--failure-tail-lines",
        "FAILURE_TAIL_LINES",
        "meaningful lines returned for a failed command",
    ),
];

/// The `--max-*` / `--*-tail-lines` options checked for `>= 1`, in the script's order.
const POSITIVE_OPTIONS: [&str; 4] = [
    "--max-log-lines",
    "--max-line-chars",
    "--success-tail-lines",
    "--failure-tail-lines",
];

impl OptionSpec {
    const fn text(flag: &'static str, metavar: &'static str, help: &'static str) -> Self {
        Self {
            flag,
            metavar,
            is_integer: false,
            help,
        }
    }

    const fn integer(flag: &'static str, metavar: &'static str, help: &'static str) -> Self {
        Self {
            flag,
            metavar,
            is_integer: true,
            help,
        }
    }
}

/// Validated arguments (the Python `argparse.Namespace`).
#[derive(Debug, Clone)]
pub struct SupervisorArguments {
    pub label: String,
    pub interval_seconds: u64,
    pub log_dir: String,
    /// `None` means Python's default `os.getcwd()`.
    pub cwd: Option<String>,
    pub project_id: String,
    pub max_log_lines: usize,
    pub max_line_chars: usize,
    pub success_tail_lines: usize,
    pub failure_tail_lines: usize,
    pub command: Vec<String>,
}

pub enum ArgumentOutcome {
    Run(SupervisorArguments),
    /// Help was printed or a usage error was reported; exit with this code.
    Exit(ExitCode),
}

/// Why parsing stopped before a run.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseStop {
    Help,
    Usage(String),
}

/// Mental model: parse → report exactly like argparse on help / error.
pub fn parse_arguments(argv: &[String]) -> ArgumentOutcome {
    let prog = prog_name(argv.first().map(String::as_str));
    let tokens = argv.get(1..).unwrap_or_default();

    match parse_tokens(tokens) {
        Ok(arguments) => ArgumentOutcome::Run(arguments),
        Err(ParseStop::Help) => {
            print!("{}", with_platform_newlines(&help_text(&prog)));
            ArgumentOutcome::Exit(ExitCode::SUCCESS)
        }
        Err(ParseStop::Usage(message)) => {
            eprint!(
                "{}",
                with_platform_newlines(&format!(
                    "usage: {}\n{prog}: error: {message}\n",
                    usage_line(&prog)
                ))
            );
            ArgumentOutcome::Exit(ExitCode::from(ARGPARSE_ERROR_EXIT_CODE))
        }
    }
}

/// Mental model: walk options (argparse `consume_optional`) until the first non-option token,
/// which starts the REMAINDER; then required → unrecognized → script-level checks.
pub fn parse_tokens(tokens: &[String]) -> Result<SupervisorArguments, ParseStop> {
    let walk = walk_options(tokens)?;
    ensure_label_present(&walk)?;
    ensure_nothing_unrecognized(&walk)?;
    build_validated_arguments(walk)
}

/// What the option walk collected, in argv order.
#[derive(Default)]
struct OptionWalk {
    values: Vec<(&'static str, String)>,
    unrecognized: Vec<String>,
    remainder: Vec<String>,
}

impl OptionWalk {
    /// argparse `store`: the last occurrence wins.
    fn last(&self, flag: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| *name == flag)
            .map(|(_, value)| value.as_str())
    }
}

enum TokenKind<'a> {
    Help,
    Known {
        option: &'static OptionSpec,
        inline_value: Option<&'a str>,
    },
    Unknown,
    /// `--` or any argparse "A" token: the REMAINDER starts here.
    RemainderStart,
}

fn walk_options(tokens: &[String]) -> Result<OptionWalk, ParseStop> {
    let mut walk = OptionWalk::default();
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index].as_str();
        match classify_token(token)? {
            TokenKind::RemainderStart => {
                walk.remainder = tokens[index..].to_vec();
                break;
            }
            TokenKind::Help => return Err(ParseStop::Help),
            TokenKind::Unknown => {
                walk.unrecognized.push(token.to_owned());
                index += 1;
            }
            TokenKind::Known {
                option,
                inline_value,
            } => {
                index = consume_option_value(&mut walk, option, inline_value, tokens, index + 1)?;
            }
        }
    }
    Ok(walk)
}

/// argparse `_parse_optional`: exact flag, `flag=value`, unique prefix, negative number,
/// contains a space, otherwise an unknown option.
fn classify_token(token: &str) -> Result<TokenKind<'_>, ParseStop> {
    if token == COMMAND_TERMINATOR || !starts_like_option(token) {
        return Ok(TokenKind::RemainderStart);
    }
    if HELP_FLAGS.contains(&token) {
        return Ok(TokenKind::Help);
    }

    let (name, inline_value) = split_inline_value(token);
    if let Some(option) = OPTIONS.iter().find(|option| option.flag == name) {
        return Ok(TokenKind::Known {
            option,
            inline_value,
        });
    }
    if let Some(kind) = classify_abbreviation(token, name, inline_value)? {
        return Ok(kind);
    }
    if is_negative_number(token) || token.contains(' ') {
        return Ok(TokenKind::RemainderStart);
    }
    Ok(TokenKind::Unknown)
}

fn classify_abbreviation<'a>(
    token: &str,
    name: &str,
    inline_value: Option<&'a str>,
) -> Result<Option<TokenKind<'a>>, ParseStop> {
    if !name.starts_with("--") {
        return Ok(None);
    }

    let help_matches = HELP_FLAGS[1].starts_with(name);
    let matches: Vec<&'static OptionSpec> = OPTIONS
        .iter()
        .filter(|option| option.flag.starts_with(name))
        .collect();

    match (matches.as_slice(), help_matches) {
        ([], false) => Ok(None),
        ([], true) => Ok(Some(TokenKind::Help)),
        ([option], false) => Ok(Some(TokenKind::Known {
            option,
            inline_value,
        })),
        (options, help) => {
            let mut candidates: Vec<&str> = options.iter().map(|option| option.flag).collect();
            if help {
                candidates.insert(0, HELP_FLAGS[1]);
            }
            Err(ParseStop::Usage(format!(
                "ambiguous option: {token} could match {}",
                candidates.join(", ")
            )))
        }
    }
}

fn consume_option_value(
    walk: &mut OptionWalk,
    option: &'static OptionSpec,
    inline_value: Option<&str>,
    tokens: &[String],
    next_index: usize,
) -> Result<usize, ParseStop> {
    let (value, following_index) = match inline_value {
        Some(value) => (value, next_index),
        None => match tokens.get(next_index) {
            Some(candidate) if can_be_option_value(candidate) => {
                (candidate.as_str(), next_index + 1)
            }
            _ => return Err(option_error(option, "expected one argument")),
        },
    };

    if option.is_integer && parse_python_int(value).is_none() {
        return Err(option_error(
            option,
            &format!("invalid int value: {}", python_repr_str(value)),
        ));
    }
    walk.values.push((option.flag, value.to_owned()));
    Ok(following_index)
}

fn ensure_label_present(walk: &OptionWalk) -> Result<(), ParseStop> {
    if walk.last("--label").is_some() {
        return Ok(());
    }
    Err(ParseStop::Usage(
        "the following arguments are required: --label".to_owned(),
    ))
}

fn ensure_nothing_unrecognized(walk: &OptionWalk) -> Result<(), ParseStop> {
    if walk.unrecognized.is_empty() {
        return Ok(());
    }
    Err(ParseStop::Usage(format!(
        "unrecognized arguments: {}",
        walk.unrecognized.join(" ")
    )))
}

/// interval_supervisor.py:61-74, in the script's order.
fn build_validated_arguments(walk: OptionWalk) -> Result<SupervisorArguments, ParseStop> {
    let command = command_after_terminator(&walk.remainder)?;
    let interval = integer_or(&walk, "--interval", DEFAULT_INTERVAL_SECONDS);
    if interval < MINIMUM_INTERVAL_SECONDS {
        return Err(ParseStop::Usage(
            "interval must be at least 5 seconds".to_owned(),
        ));
    }
    let positive = positive_limits(&walk)?;

    Ok(SupervisorArguments {
        label: walk.last("--label").unwrap_or_default().to_owned(),
        interval_seconds: u64::try_from(interval).unwrap_or(u64::MAX),
        log_dir: walk.last("--log-dir").unwrap_or(DEFAULT_LOG_DIR).to_owned(),
        cwd: walk.last("--cwd").map(str::to_owned),
        project_id: walk.last("--project-id").unwrap_or_default().to_owned(),
        max_log_lines: positive[0],
        max_line_chars: positive[1],
        success_tail_lines: positive[2],
        failure_tail_lines: positive[3],
        command,
    })
}

fn command_after_terminator(remainder: &[String]) -> Result<Vec<String>, ParseStop> {
    match remainder.split_first() {
        Some((first, rest)) if first == COMMAND_TERMINATOR => Ok(rest.to_vec()),
        _ => Err(ParseStop::Usage("command must follow --".to_owned())),
    }
}

fn positive_limits(walk: &OptionWalk) -> Result<[usize; 4], ParseStop> {
    let defaults = [
        DEFAULT_MAX_LOG_LINES,
        DEFAULT_MAX_LINE_CHARS,
        DEFAULT_SUCCESS_TAIL_LINES,
        DEFAULT_FAILURE_TAIL_LINES,
    ];
    let mut limits = [0usize; 4];

    for (slot, (flag, default)) in POSITIVE_OPTIONS.iter().zip(defaults).enumerate() {
        let value = integer_or(walk, flag, default);
        if value < 1 {
            return Err(ParseStop::Usage(format!("{flag} must be at least 1")));
        }
        limits[slot] = usize::try_from(value).unwrap_or(usize::MAX);
    }
    Ok(limits)
}

/// Integer options were validated during the walk, so a present value always converts.
#[inline]
fn integer_or(walk: &OptionWalk, flag: &str, default: i64) -> i64 {
    walk.last(flag)
        .and_then(parse_python_int)
        .unwrap_or(default)
}

#[inline]
fn starts_like_option(token: &str) -> bool {
    token.starts_with('-') && token.len() > 1
}

#[inline]
fn split_inline_value(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) if token.starts_with("--") => (name, Some(value)),
        _ => (token, None),
    }
}

/// argparse `^-\d+$|^-\d*\.\d+$`.
fn is_negative_number(token: &str) -> bool {
    let Some(body) = token.strip_prefix('-') else {
        return false;
    };
    let all_digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());

    match body.split_once('.') {
        None => all_digits(body),
        Some((whole, fraction)) => (whole.is_empty() || all_digits(whole)) && all_digits(fraction),
    }
}

/// argparse only lets a token be an option's value when it is classified as an argument ("A").
#[inline]
fn can_be_option_value(token: &str) -> bool {
    !starts_like_option(token) || is_negative_number(token) || token.contains(' ')
}

#[inline]
fn option_error(option: &OptionSpec, detail: &str) -> ParseStop {
    ParseStop::Usage(format!("argument {}: {detail}", option.flag))
}

/// argparse `prog = os.path.basename(sys.argv[0])`.
fn prog_name(argv0: Option<&str>) -> String {
    argv0
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_PROG)
        .to_owned()
}

fn usage_line(prog: &str) -> String {
    let options: Vec<String> = OPTIONS
        .iter()
        .map(|option| {
            let rendered = format!("{} {}", option.flag, option.metavar);
            if option.flag == "--label" {
                rendered
            } else {
                format!("[{rendered}]")
            }
        })
        .collect();
    format!("{prog} [-h] {} ...", options.join(" "))
}

fn help_text(prog: &str) -> String {
    let mut text = format!(
        "usage: {}\n\n{DESCRIPTION}\n\npositional arguments:\n  command\n\noptions:\n  -h, --help            show this help message and exit\n",
        usage_line(prog)
    );
    for option in &OPTIONS {
        text.push_str(&help_row(option));
    }
    text
}

/// argparse help rows: invocation padded to column 24, or help on the next line when too long.
fn help_row(option: &OptionSpec) -> String {
    const HELP_COLUMN: usize = 24;
    let invocation = format!("  {} {}", option.flag, option.metavar);

    if option.help.is_empty() {
        return format!("{invocation}\n");
    }
    if invocation.len() + 2 <= HELP_COLUMN {
        return format!("{invocation:<width$}{}\n", option.help, width = HELP_COLUMN);
    }
    format!(
        "{invocation}\n{:width$}{}\n",
        "",
        option.help,
        width = HELP_COLUMN
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn usage_error(items: &[&str]) -> String {
        match parse_tokens(&tokens(items)) {
            Err(ParseStop::Usage(message)) => message,
            other => panic!("expected usage error, got {other:?}"),
        }
    }

    #[test]
    fn defaults_match_python() {
        let parsed = parse_tokens(&tokens(&["--label", "x", "--", "echo", "hi"])).unwrap();

        assert_eq!(parsed.interval_seconds, 30);
        assert_eq!(parsed.log_dir, "var/logs");
        assert_eq!(parsed.cwd, None);
        assert_eq!(parsed.project_id, "");
        assert_eq!(parsed.max_log_lines, 200);
        assert_eq!(parsed.max_line_chars, 2000);
        assert_eq!(parsed.success_tail_lines, 2);
        assert_eq!(parsed.failure_tail_lines, 20);
        assert_eq!(parsed.command, ["echo", "hi"]);
    }

    #[test]
    fn remainder_keeps_options_after_the_terminator() {
        let parsed = parse_tokens(&tokens(&[
            "--lab=x",
            "--interval",
            "5",
            "--",
            "tool",
            "--interval",
            "1",
        ]))
        .unwrap();

        assert_eq!(parsed.label, "x");
        assert_eq!(parsed.interval_seconds, 5);
        assert_eq!(parsed.command, ["tool", "--interval", "1"]);
    }

    #[test]
    fn reports_argparse_errors_in_python_order() {
        assert_eq!(
            usage_error(&["--", "echo"]),
            "the following arguments are required: --label"
        );
        assert_eq!(
            usage_error(&["--label", "x", "--bogus", "--", "echo"]),
            "unrecognized arguments: --bogus"
        );
        assert_eq!(
            usage_error(&["--label", "x", "echo"]),
            "command must follow --"
        );
        assert_eq!(usage_error(&["--label", "x"]), "command must follow --");
        assert_eq!(
            usage_error(&["--label", "x", "--interval", "4", "--", "echo"]),
            "interval must be at least 5 seconds"
        );
        assert_eq!(
            usage_error(&["--label", "x", "--max-line-chars", "0", "--", "echo"]),
            "--max-line-chars must be at least 1"
        );
        assert_eq!(
            usage_error(&["--label", "x", "--interval", "soon", "--", "echo"]),
            "argument --interval: invalid int value: 'soon'"
        );
        assert_eq!(
            usage_error(&["--label", "--", "echo"]),
            "argument --label: expected one argument"
        );
        assert_eq!(
            usage_error(&["--max", "3", "--label", "x", "--", "echo"]),
            "ambiguous option: --max could match --max-log-lines, --max-line-chars"
        );
    }

    #[test]
    fn help_flag_stops_parsing() {
        assert_eq!(
            parse_tokens(&tokens(&["--label", "x", "-h", "--", "echo"])).unwrap_err(),
            ParseStop::Help
        );
    }

    #[test]
    fn prog_is_the_argv0_basename() {
        assert_eq!(
            prog_name(Some("/usr/bin/interval-supervisor")),
            "interval-supervisor"
        );
        assert_eq!(prog_name(None), "interval-supervisor");
    }
}
