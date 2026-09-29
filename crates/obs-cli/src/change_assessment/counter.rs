//! The deterministic code-changing-prompt counter (obs.py:488 and :515 `_finish_result`).
//! A prompt counts once: successful, `--kind code`, at least one changed file. Reaching the
//! threshold yields `PACKAGE_NOW`, which authorizes packaging review only — never a merge.

use serde_json::{json, Value};

use crate::change_assessment::classification::ChangeClass;

pub const DEFAULT_PACKAGE_THRESHOLD: i64 = 5;
const SUCCESS_OUTCOME: &str = "success";
const CODE_KIND: &str = "code";
const PACKAGE_NOW: &str = "PACKAGE_NOW";
const CONTINUE: &str = "CONTINUE";
const POLICY_RESOLUTION: &str = "current-agent-must-apply-active-AGENTS.md-cascade";

/// obs.py:489 `outcome == "success" and change_kind == "code" and files > 0`.
#[inline]
pub fn counts_toward_package(outcome: &str, change_kind: &str, files_changed: i64) -> bool {
    outcome == SUCCESS_OUTCOME && change_kind == CODE_KIND && files_changed > 0
}

#[inline]
pub fn is_package_threshold_reached(count: i64, threshold: i64) -> bool {
    count >= threshold
}

pub struct FinishSummary<'a> {
    pub assessment_id: &'a str,
    pub project_id: &'a str,
    pub prompt_id: &'a str,
    pub delivery_stream: &'a str,
    pub effective_class: ChangeClass,
    pub counted_prompts: i64,
    pub threshold: i64,
    pub newly_counted: bool,
}

/// obs.py:515 `_finish_result`, keys in obs.py order.
pub fn finish_result(summary: &FinishSummary<'_>) -> Value {
    let reached = is_package_threshold_reached(summary.counted_prompts, summary.threshold);

    json!({
        "assessmentId": summary.assessment_id,
        "projectId": summary.project_id,
        "promptId": summary.prompt_id,
        "deliveryStream": summary.delivery_stream,
        "effectiveClass": summary.effective_class.as_str(),
        "counterIncremented": summary.newly_counted,
        "codeChangePromptCount": summary.counted_prompts,
        "threshold": summary.threshold,
        "thresholdReached": reached,
        "directive": if reached { PACKAGE_NOW } else { CONTINUE },
        "policyResolution": POLICY_RESOLUTION,
        "mergeAuthorized": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(count: i64) -> FinishSummary<'static> {
        FinishSummary {
            assessment_id: "a",
            project_id: "p",
            prompt_id: "q",
            delivery_stream: "main",
            effective_class: ChangeClass::Routine,
            counted_prompts: count,
            threshold: DEFAULT_PACKAGE_THRESHOLD,
            newly_counted: true,
        }
    }

    #[test]
    fn only_successful_code_prompts_with_files_count() {
        assert!(counts_toward_package("success", "code", 1));
        assert!(!counts_toward_package("success", "code", 0));
        assert!(!counts_toward_package("failed", "code", 3));
        assert!(!counts_toward_package("success", "docs", 3));
    }

    #[test]
    fn fifth_prompt_returns_package_now() {
        assert_eq!(finish_result(&summary(4))["directive"], "CONTINUE");
        let reached = finish_result(&summary(5));
        assert_eq!(reached["directive"], "PACKAGE_NOW");
        assert_eq!(reached["thresholdReached"], true);
        assert_eq!(reached["mergeAuthorized"], false);
    }

    #[test]
    fn result_keys_follow_obs_py_order() {
        let result = finish_result(&summary(1));
        let keys: Vec<&str> = result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "assessmentId",
                "projectId",
                "promptId",
                "deliveryStream",
                "effectiveClass",
                "counterIncremented",
                "codeChangePromptCount",
                "threshold",
                "thresholdReached",
                "directive",
                "policyResolution",
                "mergeAuthorized",
            ]
        );
    }
}
