//! The embedded `OTEL_SERVICE_PREFIXES` must equal what receiver.py:57 `_service_prefixes` reads from
//! `adapters/*/manifest.toml` (sorted by path; `agent`, `otel_service_names`).

use std::path::PathBuf;

use harness_services::otlp_receiver::adapter_dispatch::OTEL_SERVICE_PREFIXES;

fn adapters_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("adapters")
}

/// `key = "value"` or `key = ["a", "b"]  # comment` → the quoted strings on that line.
fn quoted_values(text: &str, key: &str) -> Vec<String> {
    let Some(line) = text
        .lines()
        .find(|line| line.split('=').next().map(str::trim) == Some(key))
    else {
        return Vec::new();
    };
    let value = line.split_once('=').unwrap().1;
    let value = value.split('#').next().unwrap();
    value
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn embedded_prefixes_match_adapter_manifests() {
    let mut manifests: Vec<PathBuf> = std::fs::read_dir(adapters_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path().join("manifest.toml"))
        .filter(|path| path.is_file())
        .collect();
    manifests.sort();

    let mut from_manifests = Vec::new();
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest).unwrap();
        let agent = quoted_values(&text, "agent").remove(0);
        for prefix in quoted_values(&text, "otel_service_names") {
            from_manifests.push((prefix, agent.clone()));
        }
    }
    let embedded: Vec<(String, String)> = OTEL_SERVICE_PREFIXES
        .iter()
        .map(|(prefix, adapter)| ((*prefix).to_owned(), adapter.agent_name().to_owned()))
        .collect();

    assert_eq!(embedded, from_manifests);
}
