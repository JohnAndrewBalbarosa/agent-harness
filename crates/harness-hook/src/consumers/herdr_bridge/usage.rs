//! `exact_usage(payload)` (`hook.py`): token counts the agent reported in the hook payload itself, if any.
//!
//! Source: the first object among `usage`, `token_usage`, `response.usage`, `response.token_usage`. Each target
//! takes the first alias that is present and not null; only non-negative numbers count (Python `bool` is an
//! `int`, floats are truncated by `int()`).

use serde_json::{Map, Value};

/// `(target, aliases)` in Python dict order.
const USAGE_ALIASES: [(&str, &[&str]); 4] = [
    (
        "inputTokens",
        &["input_tokens", "inputTokens", "prompt_tokens"],
    ),
    (
        "outputTokens",
        &["output_tokens", "outputTokens", "completion_tokens"],
    ),
    (
        "cachedInputTokens",
        &["cached_input_tokens", "cachedInputTokens", "cached_tokens"],
    ),
    ("reasoningTokens", &["reasoning_tokens", "reasoningTokens"]),
];

/// `(values, availability)`: values keep the target order; availability is `exact` or `unavailable`.
pub fn exact_usage(payload: &Map<String, Value>) -> (Map<String, Value>, &'static str) {
    let source = usage_source(payload).filter(|source| !source.is_empty());
    let mut values = Map::new();
    for (target, aliases) in USAGE_ALIASES {
        let count = source.and_then(|source| token_count(source, aliases));
        values.insert(target.to_owned(), count.map_or(Value::Null, Value::from));
    }
    let availability = if values.values().any(|value| !value.is_null()) {
        "exact"
    } else {
        "unavailable"
    };
    (values, availability)
}

fn usage_source(payload: &Map<String, Value>) -> Option<&Map<String, Value>> {
    let response = payload.get("response").and_then(Value::as_object);
    let candidates = [
        payload.get("usage"),
        payload.get("token_usage"),
        response.and_then(|response| response.get("usage")),
        response.and_then(|response| response.get("token_usage")),
    ];
    candidates.into_iter().flatten().find_map(Value::as_object)
}

/// `int(raw) if isinstance(raw, (int, float)) and raw >= 0 else None`.
fn token_count(source: &Map<String, Value>, aliases: &[&str]) -> Option<i64> {
    let raw = aliases
        .iter()
        .filter_map(|name| source.get(*name))
        .find(|value| !value.is_null())?;
    match raw {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => (integer >= 0).then_some(integer),
            None => number
                .as_f64()
                .filter(|float| *float >= 0.0)
                .map(|float| float.trunc() as i64),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn reads_the_first_usage_object_with_aliases() {
        let (values, availability) = exact_usage(&object(
            json!({"response": {"usage": {"prompt_tokens": 3, "output_tokens": 2.7, "cached_tokens": -1}}}),
        ));
        assert_eq!(availability, "exact");
        assert_eq!(
            Value::Object(values),
            json!({"inputTokens": 3, "outputTokens": 2, "cachedInputTokens": null, "reasoningTokens": null})
        );
    }

    #[test]
    fn empty_or_missing_usage_is_unavailable() {
        let (values, availability) = exact_usage(&object(
            json!({"usage": {}, "token_usage": {"input_tokens": 1}}),
        ));
        assert_eq!(availability, "unavailable");
        assert!(values.values().all(Value::is_null));
    }
}
