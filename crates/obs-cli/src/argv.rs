//! Hand-rolled argv parsing that reproduces the argparse behaviour obs.py relies on
//! (obs.py:1271-1369): options in any order, `--flag=value`, unique-prefix abbreviations,
//! per-option type/choice validation in argv order, required checks, then
//! "unrecognized arguments" reported by the top-level `obs` parser. Exit code 2 on errors.
//!
//! Deviation: usage lines are printed on one line (argparse wraps to the terminal width),
//! and `-h` prints only the usage line.

use crate::failure::{CliFailure, CliResult};
use crate::python_compat::{parse_python_int, python_repr_str};

pub const TOP_LEVEL_PROG: &str = "obs";
pub const TOP_LEVEL_USAGE: &str = "obs [-h] {project,prompt,event,errors,analytics,trace,diagnose,context,runtime,benchmark,change,git,flush,query} ...";

const HELP_FLAGS: [&str; 2] = ["-h", "--help"];

/// A parser whose only job is choosing the next subcommand (argparse sub-parsers).
pub struct CommandGroup {
    pub prog: &'static str,
    pub usage: &'static str,
    pub dest: &'static str,
    pub choices: &'static [&'static str],
}

/// A leaf subcommand parser.
pub struct CommandSpec {
    pub prog: &'static str,
    pub usage: &'static str,
    pub options: &'static [OptionSpec],
    pub positionals: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub struct OptionSpec {
    pub flag: &'static str,
    pub arity: Arity,
    pub rule: ValueRule,
    pub required: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// `action="store_true"`.
    Switch,
    /// Plain store: the last occurrence wins.
    Single,
    /// `action="append"`.
    Repeated,
}

#[derive(Clone, Copy)]
pub enum ValueRule {
    Text,
    Integer,
    OneOf(&'static [&'static str]),
}

impl OptionSpec {
    pub const fn switch(flag: &'static str) -> Self {
        Self::build(flag, Arity::Switch, ValueRule::Text)
    }

    pub const fn text(flag: &'static str) -> Self {
        Self::build(flag, Arity::Single, ValueRule::Text)
    }

    pub const fn integer(flag: &'static str) -> Self {
        Self::build(flag, Arity::Single, ValueRule::Integer)
    }

    pub const fn one_of(flag: &'static str, choices: &'static [&'static str]) -> Self {
        Self::build(flag, Arity::Single, ValueRule::OneOf(choices))
    }

    pub const fn repeated(flag: &'static str) -> Self {
        Self::build(flag, Arity::Repeated, ValueRule::Text)
    }

    pub const fn required(self) -> Self {
        Self {
            required: true,
            ..self
        }
    }

    const fn build(flag: &'static str, arity: Arity, rule: ValueRule) -> Self {
        Self {
            flag,
            arity,
            rule,
            required: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct ParsedArgs {
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    positionals: Vec<String>,
}

impl ParsedArgs {
    /// Last stored value (argparse `store`).
    pub fn text(&self, flag: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| *name == flag)
            .map(|(_, value)| value.as_str())
    }

    /// Every stored value in argv order (argparse `append`).
    pub fn texts(&self, flag: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(name, _)| *name == flag)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// Integer options were validated during parsing, so a present value always converts.
    pub fn integer(&self, flag: &str) -> Option<i64> {
        self.text(flag).and_then(parse_python_int)
    }

    pub fn switch(&self, flag: &str) -> bool {
        self.switches.contains(&flag)
    }

    pub fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }
}

/// Picks the subcommand token for `group` and returns it with the remaining argv.
pub fn split_subcommand<'a>(
    arguments: &'a [String],
    group: &CommandGroup,
) -> CliResult<(&'a str, &'a [String])> {
    let Some(first) = arguments.first() else {
        return Err(CliFailure::usage(
            group.prog,
            group.usage,
            required_message(&[group.dest]),
        ));
    };

    if is_help_flag(first) {
        return Err(CliFailure::HelpRequested {
            usage: group.usage.to_owned(),
        });
    }

    if !group.choices.contains(&first.as_str()) {
        return Err(CliFailure::usage(
            group.prog,
            group.usage,
            invalid_choice_message(group.dest, first, group.choices),
        ));
    }

    Ok((first.as_str(), &arguments[1..]))
}

/// Mental model: walk argv left to right like argparse. Options consume their value and are
/// validated immediately; surplus tokens are collected as unrecognized; required options are
/// checked after the walk, and unrecognized tokens are reported last by the top-level parser.
pub fn parse(spec: &CommandSpec, arguments: &[String]) -> CliResult<ParsedArgs> {
    let mut parsed = ParsedArgs::default();
    let mut unrecognized: Vec<&str> = Vec::new();
    let mut after_terminator = false;
    let mut index = 0;

    while index < arguments.len() {
        let token = arguments[index].as_str();
        index += 1;

        if !after_terminator && token == "--" {
            after_terminator = true;
            continue;
        }

        let classified = if after_terminator {
            OptionToken::Positional
        } else {
            classify_token(spec, token)?
        };

        match classified {
            OptionToken::Help => {
                return Err(CliFailure::HelpRequested {
                    usage: spec.usage.to_owned(),
                })
            }
            OptionToken::Known {
                option,
                inline_value,
            } => {
                index = consume_option(spec, &mut parsed, option, inline_value, arguments, index)?;
            }
            OptionToken::Positional => {
                accept_positional(spec, &mut parsed, &mut unrecognized, token)
            }
            OptionToken::Unknown => unrecognized.push(token),
        }
    }

    ensure_required_present(spec, &parsed)?;
    ensure_nothing_unrecognized(&unrecognized)?;
    Ok(parsed)
}

enum OptionToken<'a> {
    Help,
    Known {
        option: &'static OptionSpec,
        inline_value: Option<&'a str>,
    },
    Positional,
    Unknown,
}

/// argparse `_parse_optional` order: exact flag, `flag=value`, unique prefix, negative number,
/// contains a space, otherwise an unknown option.
fn classify_token<'a>(spec: &CommandSpec, token: &'a str) -> CliResult<OptionToken<'a>> {
    if !starts_like_option(token) {
        return Ok(OptionToken::Positional);
    }

    if is_help_flag(token) {
        return Ok(OptionToken::Help);
    }

    let (name, inline_value) = split_inline_value(token);

    if let Some(option) = find_exact_option(spec, name) {
        return Ok(OptionToken::Known {
            option,
            inline_value,
        });
    }

    if let Some(known) = find_abbreviated_option(spec, token, name, inline_value)? {
        return Ok(known);
    }

    if is_negative_number(token) || token.contains(' ') {
        return Ok(OptionToken::Positional);
    }

    Ok(OptionToken::Unknown)
}

#[inline]
fn starts_like_option(token: &str) -> bool {
    token.starts_with('-') && token.len() > 1
}

#[inline]
fn is_help_flag(token: &str) -> bool {
    HELP_FLAGS.contains(&token)
}

fn split_inline_value(token: &str) -> (&str, Option<&str>) {
    match token.split_once('=') {
        Some((name, value)) if token.starts_with("--") => (name, Some(value)),
        _ => (token, None),
    }
}

fn find_exact_option(spec: &CommandSpec, name: &str) -> Option<&'static OptionSpec> {
    let options: &'static [OptionSpec] = spec.options;
    options.iter().find(|option| option.flag == name)
}

fn find_abbreviated_option<'a>(
    spec: &CommandSpec,
    token: &str,
    name: &str,
    inline_value: Option<&'a str>,
) -> CliResult<Option<OptionToken<'a>>> {
    if !name.starts_with("--") {
        return Ok(None);
    }

    let help_matches = HELP_FLAGS[1].starts_with(name);
    let options: &'static [OptionSpec] = spec.options;
    let option_matches: Vec<&'static OptionSpec> = options
        .iter()
        .filter(|option| option.flag.starts_with(name))
        .collect();

    match (option_matches.as_slice(), help_matches) {
        ([], false) => Ok(None),
        ([], true) => Ok(Some(OptionToken::Help)),
        ([option], false) => Ok(Some(OptionToken::Known {
            option,
            inline_value,
        })),
        (options, help) => {
            let mut candidates: Vec<&str> = options.iter().map(|option| option.flag).collect();
            if help {
                candidates.insert(0, HELP_FLAGS[1]);
            }
            Err(CliFailure::usage(
                spec.prog,
                spec.usage,
                format!(
                    "ambiguous option: {token} could match {}",
                    candidates.join(", ")
                ),
            ))
        }
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

fn consume_option(
    spec: &CommandSpec,
    parsed: &mut ParsedArgs,
    option: &'static OptionSpec,
    inline_value: Option<&str>,
    arguments: &[String],
    next_index: usize,
) -> CliResult<usize> {
    if option.arity == Arity::Switch {
        if let Some(value) = inline_value {
            return Err(option_error(
                spec,
                option,
                format!("ignored explicit argument {}", python_repr_str(value)),
            ));
        }
        parsed.switches.push(option.flag);
        return Ok(next_index);
    }

    let (value, following_index) = match inline_value {
        Some(value) => (value, next_index),
        None => match arguments.get(next_index) {
            Some(candidate) if can_be_option_value(candidate) => {
                (candidate.as_str(), next_index + 1)
            }
            _ => {
                return Err(option_error(
                    spec,
                    option,
                    "expected one argument".to_owned(),
                ))
            }
        },
    };

    validate_value(spec, option, value)?;
    parsed.values.push((option.flag, value.to_owned()));
    Ok(following_index)
}

/// argparse only lets a token be an option's value when it is classified as an argument ("A").
#[inline]
fn can_be_option_value(token: &str) -> bool {
    !starts_like_option(token) || is_negative_number(token) || token.contains(' ')
}

fn validate_value(spec: &CommandSpec, option: &OptionSpec, value: &str) -> CliResult<()> {
    match option.rule {
        ValueRule::Text => Ok(()),
        ValueRule::Integer if parse_python_int(value).is_some() => Ok(()),
        ValueRule::Integer => Err(option_error(
            spec,
            option,
            format!("invalid int value: {}", python_repr_str(value)),
        )),
        ValueRule::OneOf(choices) if choices.contains(&value) => Ok(()),
        ValueRule::OneOf(choices) => Err(CliFailure::usage(
            spec.prog,
            spec.usage,
            invalid_choice_message(option.flag, value, choices),
        )),
    }
}

fn accept_positional<'a>(
    spec: &CommandSpec,
    parsed: &mut ParsedArgs,
    unrecognized: &mut Vec<&'a str>,
    token: &'a str,
) {
    if parsed.positionals.len() < spec.positionals.len() {
        parsed.positionals.push(token.to_owned());
    } else {
        unrecognized.push(token);
    }
}

fn ensure_required_present(spec: &CommandSpec, parsed: &ParsedArgs) -> CliResult<()> {
    let missing_options = spec
        .options
        .iter()
        .filter(|option| option.required && parsed.text(option.flag).is_none())
        .map(|option| option.flag);
    let missing_positionals = spec
        .positionals
        .iter()
        .skip(parsed.positionals.len())
        .copied();
    let missing: Vec<&str> = missing_options.chain(missing_positionals).collect();

    if missing.is_empty() {
        return Ok(());
    }
    Err(CliFailure::usage(
        spec.prog,
        spec.usage,
        required_message(&missing),
    ))
}

fn ensure_nothing_unrecognized(unrecognized: &[&str]) -> CliResult<()> {
    if unrecognized.is_empty() {
        return Ok(());
    }
    Err(CliFailure::usage(
        TOP_LEVEL_PROG,
        TOP_LEVEL_USAGE,
        format!("unrecognized arguments: {}", unrecognized.join(" ")),
    ))
}

#[inline]
fn option_error(spec: &CommandSpec, option: &OptionSpec, detail: String) -> CliFailure {
    CliFailure::usage(
        spec.prog,
        spec.usage,
        format!("argument {}: {detail}", option.flag),
    )
}

#[inline]
fn required_message(names: &[&str]) -> String {
    format!("the following arguments are required: {}", names.join(", "))
}

/// Python 3.14 argparse wording (choices printed with `str`, joined by ", ").
#[inline]
fn invalid_choice_message(name: &str, value: &str, choices: &[&str]) -> String {
    format!(
        "argument {name}: invalid choice: {} (choose from {})",
        python_repr_str(value),
        choices.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: &[&str] = &["non-code", "docs", "media", "code"];
    const OPTIONS: &[OptionSpec] = &[
        OptionSpec::one_of("--kind", KINDS).required(),
        OptionSpec::integer("--estimated-lines"),
        OptionSpec::integer("--estimated-files"),
        OptionSpec::repeated("--risk"),
        OptionSpec::switch("--confirm"),
    ];
    const SPEC: CommandSpec = CommandSpec {
        prog: "obs change plan",
        usage: "obs change plan [-h] --kind {non-code,docs,media,code}",
        options: OPTIONS,
        positionals: &[],
    };
    const POSITIONAL_SPEC: CommandSpec = CommandSpec {
        prog: "obs trace",
        usage: "obs trace [-h] trace_id",
        options: &[],
        positionals: &["trace_id"],
    };

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    fn usage_message(failure: CliFailure) -> (String, String) {
        match failure {
            CliFailure::Usage { prog, message, .. } => (prog, message),
            other => panic!("expected usage failure, got {other:?}"),
        }
    }

    #[test]
    fn parses_options_in_any_order_with_inline_values_and_repeats() {
        let parsed = parse(
            &SPEC,
            &argv(&[
                "--risk",
                "auth",
                "--estimated-lines=12",
                "--kind",
                "code",
                "--risk",
                "db",
            ]),
        )
        .unwrap();

        assert_eq!(parsed.text("--kind"), Some("code"));
        assert_eq!(parsed.integer("--estimated-lines"), Some(12));
        assert_eq!(parsed.texts("--risk"), vec!["auth", "db"]);
        assert!(!parsed.switch("--confirm"));
    }

    #[test]
    fn accepts_unique_prefix_and_negative_numbers() {
        let parsed = parse(
            &SPEC,
            &argv(&["--kind", "docs", "--estimated-l", "-5", "--conf"]),
        )
        .unwrap();

        assert_eq!(parsed.integer("--estimated-lines"), Some(-5));
        assert!(parsed.switch("--confirm"));
    }

    #[test]
    fn rejects_ambiguous_prefix() {
        let failure = parse(&SPEC, &argv(&["--kind", "code", "--estimated", "1"])).unwrap_err();
        let (_, message) = usage_message(failure);

        assert_eq!(
            message,
            "ambiguous option: --estimated could match --estimated-lines, --estimated-files"
        );
    }

    #[test]
    fn reports_missing_required_on_the_subcommand_parser() {
        let (prog, message) = usage_message(parse(&SPEC, &argv(&[])).unwrap_err());

        assert_eq!(prog, "obs change plan");
        assert_eq!(message, "the following arguments are required: --kind");
    }

    #[test]
    fn validates_types_and_choices_in_argv_order() {
        let (_, int_message) = usage_message(
            parse(&SPEC, &argv(&["--estimated-files", "x", "--kind", "bad"])).unwrap_err(),
        );
        let (_, choice_message) =
            usage_message(parse(&SPEC, &argv(&["--kind", "bad"])).unwrap_err());

        assert_eq!(
            int_message,
            "argument --estimated-files: invalid int value: 'x'"
        );
        assert_eq!(
            choice_message,
            "argument --kind: invalid choice: 'bad' (choose from non-code, docs, media, code)"
        );
    }

    #[test]
    fn value_cannot_be_another_option() {
        let (_, message) = usage_message(parse(&SPEC, &argv(&["--kind", "--risk"])).unwrap_err());

        assert_eq!(message, "argument --kind: expected one argument");
    }

    #[test]
    fn unrecognized_tokens_are_reported_by_the_top_level_parser() {
        let (prog, message) =
            usage_message(parse(&POSITIONAL_SPEC, &argv(&["abc", "extra", "--nope"])).unwrap_err());

        assert_eq!(prog, "obs");
        assert_eq!(message, "unrecognized arguments: extra --nope");
    }

    #[test]
    fn positional_after_terminator_may_start_with_dash() {
        let parsed = parse(&POSITIONAL_SPEC, &argv(&["--", "-abc"])).unwrap();

        assert_eq!(parsed.positional(0), Some("-abc"));
    }

    #[test]
    fn subcommand_choice_is_validated() {
        let group = CommandGroup {
            prog: "obs change",
            usage: "obs change [-h] {plan,finish,package} ...",
            dest: "change_command",
            choices: &["plan", "finish", "package"],
        };
        let arguments = argv(&["plan", "--kind", "code"]);
        let (command, rest) = split_subcommand(&arguments, &group).unwrap();

        assert_eq!(command, "plan");
        assert_eq!(rest.len(), 2);
        let (_, message) = usage_message(split_subcommand(&argv(&["nope"]), &group).unwrap_err());
        assert_eq!(
            message,
            "argument change_command: invalid choice: 'nope' (choose from plan, finish, package)"
        );
    }
}
