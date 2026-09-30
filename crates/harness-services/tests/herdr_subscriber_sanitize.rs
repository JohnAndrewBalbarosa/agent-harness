//! subscriber.py `sanitize` parity.

use harness_services::herdr_subscriber::sanitize::{
    is_large_key, is_sensitive_key, sanitize_event, sanitize_event_body,
};
use serde_json::{json, Value};

#[test]
fn values_nested_deeper_than_five_levels_are_depth_limited() {
    let event = json!({"a": {"a": {"a": {"a": {"a": {"a": 1, "b": [1]}}}}}});
    let bounded = sanitize_event(&event);
    assert_eq!(
        bounded["a"]["a"]["a"]["a"]["a"]["a"],
        json!("<depth-limited>")
    );
    assert_eq!(
        bounded["a"]["a"]["a"]["a"]["a"]["b"],
        json!("<depth-limited>")
    );
}

#[test]
fn sensitive_keys_are_redacted_whatever_their_value() {
    let event = json!({
        "AuthToken": "abc",
        "client_secret": {"nested": 1},
        "Authorization": ["x"],
        "private_key": "k",
        "privatekey": "k",
        "private--key": "kept",
        "cookies": 5,
        "name": "ok",
    });
    let bounded = sanitize_event(&event);
    for key in [
        "AuthToken",
        "client_secret",
        "Authorization",
        "private_key",
        "privatekey",
        "cookies",
    ] {
        assert_eq!(bounded[key], json!("<redacted>"), "{key}");
    }
    assert_eq!(bounded["private--key"], json!("kept"));
    assert_eq!(bounded["name"], json!("ok"));
}

#[test]
fn private_key_pattern_allows_one_non_newline_character() {
    assert!(is_sensitive_key("PRIVATE KEY"));
    assert!(!is_sensitive_key("private\nkey"));
    assert!(is_sensitive_key("my_password_hash"));
    assert!(!is_sensitive_key("pane_id"));
}

#[test]
fn strings_are_truncated_by_key_class_with_a_marker() {
    let long = "é".repeat(2500);
    let bounded =
        sanitize_event(&json!({"screen_text": long, "title": long, "exact": "x".repeat(500)}));
    let screen = bounded["screen_text"].as_str().unwrap();
    assert_eq!(screen.chars().count(), 500 + "<truncated>".len());
    assert!(screen.ends_with("<truncated>"));
    let title = bounded["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 2000 + "<truncated>".len());
    assert_eq!(bounded["exact"].as_str().unwrap(), "x".repeat(500));
    assert!(is_large_key("RawOutput"));
    assert!(!is_large_key("title"));
}

#[test]
fn list_items_inherit_the_parent_key() {
    let bounded = sanitize_event(&json!({"output": ["y".repeat(600)], "tokens": ["a"]}));
    assert_eq!(bounded["output"][0].as_str().unwrap().chars().count(), 511);
    assert_eq!(bounded["tokens"], json!("<redacted>"));
}

#[test]
fn objects_and_lists_keep_their_first_eighty_entries_and_keys_are_cut() {
    let mut map = serde_json::Map::new();
    for index in 0..100 {
        map.insert(format!("k{index:03}"), json!(index));
    }
    let long_key = "q".repeat(130);
    map.insert(long_key, json!(1));
    let bounded = sanitize_event_body(&map);
    assert_eq!(bounded.len(), 80);
    assert_eq!(bounded.keys().next().map(String::as_str), Some("k000"));

    let mut long_keyed = serde_json::Map::new();
    long_keyed.insert("q".repeat(130), json!(1));
    let with_long_key = sanitize_event_body(&long_keyed);
    let key = with_long_key.keys().next().unwrap().clone();
    assert_eq!(key.len(), 120);

    let list: Vec<Value> = (0..100).map(Value::from).collect();
    let bounded_list = sanitize_event(&json!({ "items": list }));
    assert_eq!(bounded_list["items"].as_array().unwrap().len(), 80);
}

#[test]
fn scalars_pass_through() {
    let event = json!({"n": 1.5, "b": true, "z": null, "i": -3});
    assert_eq!(sanitize_event(&event), event);
}
