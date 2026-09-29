//! `obs change package` (obs.py:526 `package_changes`): after an authorized Git delivery
//! succeeded, close the batch of counted prompts. Never authorizes a merge.

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::argv::{parse, CommandSpec, OptionSpec, ParsedArgs};
use crate::change_assessment::identity::{resolve_change_identity, ChangeIdentity};
use crate::commands::output::print_command_result;
use crate::failure::{CliFailure, CliResult};
use crate::operation::build_operation;
use crate::python_compat::{python_utc_now, uuid7_text};
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::change_ledger::{record_package, unpackaged_assessment_ids, ChangePackage};
use crate::spool::flush::HubSink;
use crate::spool::open::open_spool;
use crate::spool::queue::enqueue_and_flush;

const DELIVERY_MODES: &[&str] = &["local", "push", "pull-request"];

const SPEC: CommandSpec = CommandSpec {
    prog: "obs change package",
    usage: "obs change package [-h] --delivery-mode {local,push,pull-request} [--commit-sha COMMIT_SHA] [--pr-url PR_URL] [--confirm-policy-reviewed] [--prompt-id PROMPT_ID] [--delivery-stream DELIVERY_STREAM]",
    options: &[
        OptionSpec::one_of("--delivery-mode", DELIVERY_MODES).required(),
        OptionSpec::text("--commit-sha"),
        OptionSpec::text("--pr-url"),
        OptionSpec::switch("--confirm-policy-reviewed"),
        OptionSpec::text("--prompt-id"),
        OptionSpec::text("--delivery-stream"),
    ],
    positionals: &[],
};

struct PackageRequest {
    delivery_mode: String,
    commit_sha: Option<String>,
    pr_url: Option<String>,
    is_policy_reviewed: bool,
    prompt_id: Option<String>,
    delivery_stream: Option<String>,
}

impl PackageRequest {
    fn from_args(args: &ParsedArgs) -> Self {
        Self {
            delivery_mode: args.text("--delivery-mode").unwrap_or_default().to_owned(),
            commit_sha: non_empty_owned(args.text("--commit-sha")),
            pr_url: non_empty_owned(args.text("--pr-url")),
            is_policy_reviewed: args.switch("--confirm-policy-reviewed"),
            prompt_id: args.text("--prompt-id").map(str::to_owned),
            delivery_stream: args.text("--delivery-stream").map(str::to_owned),
        }
    }
}

/// obs.py `args.commit_sha or None`.
#[inline]
fn non_empty_owned(value: Option<&str>) -> Option<String> {
    value.filter(|text| !text.is_empty()).map(str::to_owned)
}

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let request = PackageRequest::from_args(&args);

    if !request.is_policy_reviewed {
        return Err(CliFailure::refusal(
            "Refusing to close the batch until the active AGENTS.md cascade has been reviewed.",
        ));
    }

    let settings = Settings::from_env();
    let conn = open_spool()?;
    let result = package_changes(&conn, &settings, &request)?;
    print_command_result(&result)
}

/// Mental model: identity → collect counted, unpackaged assessments → one package row linking
/// them (single transaction) → spool the `change-assessment.package` operation.
fn package_changes(
    conn: &Connection,
    settings: &Settings,
    request: &PackageRequest,
) -> CliResult<Value> {
    let identity = resolve_change_identity(
        conn,
        request.prompt_id.as_deref(),
        request.delivery_stream.as_deref(),
    )?;
    let assessment_ids =
        unpackaged_assessment_ids(conn, &identity.project_id, &identity.delivery_stream)?;

    if assessment_ids.is_empty() {
        return Err(CliFailure::refusal(
            "No un-packaged code-changing prompts exist for this delivery stream.",
        ));
    }

    let package = new_package(&identity, request, &assessment_ids);
    close_batch(conn, &package, &assessment_ids)?;

    let operation = build_operation(
        "change-assessment.package",
        &package_operation_data(&identity, &package),
    );
    enqueue_and_flush(conn, settings, &HubSink { settings }, &[operation])?;

    Ok(package_result(&package))
}

fn new_package(
    identity: &ChangeIdentity,
    request: &PackageRequest,
    assessment_ids: &[String],
) -> ChangePackage {
    ChangePackage {
        package_id: uuid7_text(),
        project_id: identity.project_id.clone(),
        delivery_stream: identity.delivery_stream.clone(),
        delivery_mode: request.delivery_mode.clone(),
        commit_sha: request.commit_sha.clone(),
        pr_url: request.pr_url.clone(),
        prompt_count: i64::try_from(assessment_ids.len()).unwrap_or(i64::MAX),
        created_at: python_utc_now(),
    }
}

fn close_batch(
    conn: &Connection,
    package: &ChangePackage,
    assessment_ids: &[String],
) -> CliResult<()> {
    let transaction = begin_write(conn)?;
    record_package(&transaction, package, assessment_ids)?;
    transaction.commit()?;
    Ok(())
}

fn package_operation_data(identity: &ChangeIdentity, package: &ChangePackage) -> Value {
    json!({
        "packageId": package.package_id,
        "projectId": package.project_id,
        "promptId": identity.prompt_id,
        "deliveryStream": package.delivery_stream,
        "deliveryMode": package.delivery_mode,
        "commitSha": package.commit_sha,
        "prUrl": package.pr_url,
        "promptCount": package.prompt_count,
        "createdAt": package.created_at,
    })
}

fn package_result(package: &ChangePackage) -> Value {
    json!({
        "packageId": package.package_id,
        "projectId": package.project_id,
        "deliveryStream": package.delivery_stream,
        "deliveryMode": package.delivery_mode,
        "promptCount": package.prompt_count,
        "remainingCount": 0,
        "mergeAuthorized": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_batch_resets_the_counter() {
        let conn = Connection::open_in_memory().unwrap();
        crate::spool::open::prepare_spool_connection(&conn).unwrap();
        for index in 0..3 {
            conn.execute(
                "INSERT INTO prompt_change_assessments(assessment_id,project_id,prompt_id,delivery_stream,change_kind,declared_class,effective_class,status,increments_counter,created_at) VALUES(?,'p',?,'main','code','routine','routine','success',1,?)",
                rusqlite::params![format!("a{index}"), format!("q{index}"), format!("t{index}")],
            )
            .unwrap();
        }
        let identity = ChangeIdentity {
            project_id: "p".to_owned(),
            prompt_id: "q2".to_owned(),
            delivery_stream: "main".to_owned(),
        };
        let request = PackageRequest {
            delivery_mode: "push".to_owned(),
            commit_sha: None,
            pr_url: None,
            is_policy_reviewed: true,
            prompt_id: None,
            delivery_stream: None,
        };

        let ids = unpackaged_assessment_ids(&conn, "p", "main").unwrap();
        assert_eq!(ids, vec!["a0", "a1", "a2"]);
        let package = new_package(&identity, &request, &ids);
        close_batch(&conn, &package, &ids).unwrap();

        assert!(unpackaged_assessment_ids(&conn, "p", "main")
            .unwrap()
            .is_empty());
        assert_eq!(package_result(&package)["promptCount"], 3);
    }
}
