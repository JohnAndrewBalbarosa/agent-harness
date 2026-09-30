//! Port of `tests/unit/test_services.py` `BootstrapTests` (receiver/console monitor now mean "daemon ensured").

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use harness_services::ensure::layout::{native_hub_binary, services_binary, EnsureLayout};
use harness_services::ensure::probes::ServiceProbes;
use harness_services::ensure::steps::ensure_orb;
use harness_services::ensure::{run_steps, single_instance};
use serde_json::Value;

type Launch = (Vec<String>, String);

struct FakeProbes {
    launched: RefCell<Vec<Launch>>,
    is_hub_healthy: bool,
    is_daemon_running: bool,
    daemon_checks: Cell<usize>,
    launch_error: Option<io::ErrorKind>,
}

impl FakeProbes {
    fn new() -> Self {
        Self {
            launched: RefCell::new(Vec::new()),
            is_hub_healthy: false,
            is_daemon_running: false,
            daemon_checks: Cell::new(0),
            launch_error: None,
        }
    }

    fn launches(&self) -> Vec<Launch> {
        self.launched.borrow().clone()
    }
}

impl ServiceProbes for FakeProbes {
    fn launch(&self, command: &[OsString], cwd: &Path) -> io::Result<()> {
        if let Some(kind) = self.launch_error {
            return Err(io::Error::new(kind, "fixture launch failure"));
        }
        let parts = command
            .iter()
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        self.launched
            .borrow_mut()
            .push((parts, cwd.display().to_string()));
        Ok(())
    }

    fn http_healthy(&self, _url: &str) -> bool {
        self.is_hub_healthy
    }

    fn daemon_running(&self) -> bool {
        self.daemon_checks.set(self.daemon_checks.get() + 1);
        self.is_daemon_running
    }
}

fn layout(root: &Path) -> EnsureLayout {
    EnsureLayout {
        root: root.to_path_buf(),
        orb_health: root.join("orb-health.json"),
        obs_endpoint: "http://127.0.0.1:1".to_owned(),
        orb_ui: true,
    }
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

fn orb_pythonw(root: &Path) -> PathBuf {
    root.join("tools")
        .join("codex-status-orb")
        .join(".venv")
        .join("Scripts")
        .join("pythonw.exe")
}

fn log_records(root: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(
        root.join("var")
            .join("logs")
            .join("services.lifecycle.jsonl"),
    )
    .unwrap();
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn result_of(records: &[Value], operation: &str) -> String {
    records
        .iter()
        .rev()
        .find(|record| record["operation"] == operation)
        .map(|record| {
            record["details"]["result"]
                .as_str()
                .or_else(|| record["details"]["error"].as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap()
}

#[test]
fn daemon_is_launched_once_for_receiver_and_console_monitor() {
    let dir = tempfile::tempdir().unwrap();
    let probes = FakeProbes::new();
    touch(&services_binary(dir.path()));

    run_steps(&layout(dir.path()), &probes);

    let launches = probes.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches[0].0,
        vec![
            services_binary(dir.path()).display().to_string(),
            "serve".to_owned()
        ]
    );
    assert_eq!(launches[0].1, dir.path().display().to_string());
    let records = log_records(dir.path());
    assert_eq!(result_of(&records, "receiver.ensure"), "requested");
    assert_eq!(result_of(&records, "console-monitor.ensure"), "requested");
}

#[test]
fn running_daemon_is_not_launched_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut probes = FakeProbes::new();
    probes.is_daemon_running = true;
    touch(&services_binary(dir.path()));

    run_steps(&layout(dir.path()), &probes);

    assert!(probes.launches().is_empty());
    assert_eq!(probes.daemon_checks.get(), 1);
    let records = log_records(dir.path());
    assert_eq!(result_of(&records, "receiver.ensure"), "already_running");
    assert_eq!(
        result_of(&records, "console-monitor.ensure"),
        "already_running"
    );
}

#[test]
fn orb_starts_only_when_installed_and_not_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let probes = FakeProbes::new();
    run_steps(&layout(dir.path()), &probes);
    assert!(probes.launches().is_empty());

    touch(&orb_pythonw(dir.path()));
    run_steps(&layout(dir.path()), &probes);
    assert_eq!(probes.launches().len(), 1);
    assert!(probes.launches()[0].0.last().unwrap().ends_with("orb.py"));

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    std::fs::write(
        dir.path().join("orb-health.json"),
        serde_json::json!({"status": "healthy", "updated_at": now}).to_string(),
    )
    .unwrap();
    run_steps(&layout(dir.path()), &probes);
    assert_eq!(probes.launches().len(), 1);
}

#[test]
fn stale_orb_health_is_not_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let probes = FakeProbes::new();
    touch(&orb_pythonw(dir.path()));
    std::fs::write(
        dir.path().join("orb-health.json"),
        r#"{"status": "healthy", "updated_at": 1}"#,
    )
    .unwrap();

    assert_eq!(
        ensure_orb(&layout(dir.path()), &probes).unwrap(),
        "requested"
    );
}

#[test]
fn orb_ui_can_be_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let probes = FakeProbes::new();
    touch(&orb_pythonw(dir.path()));
    let mut headless = layout(dir.path());
    headless.orb_ui = false;

    assert_eq!(
        ensure_orb(&headless, &probes).unwrap(),
        "requested_headless"
    );
    assert_eq!(probes.launches()[0].0.last().unwrap(), "--headless");

    let root = dir.path().display().to_string();
    let env_off = BTreeMap::from([
        ("ORB_UI".to_owned(), "0".to_owned()),
        ("LOCALAPPDATA".to_owned(), root.clone()),
    ]);
    let env_on = BTreeMap::from([("LOCALAPPDATA".to_owned(), root)]);
    assert!(!EnsureLayout::from_env(dir.path(), &env_off).orb_ui);
    assert!(EnsureLayout::from_env(dir.path(), &env_on).orb_ui);
    assert_eq!(
        EnsureLayout::from_env(dir.path(), &env_on).orb_health,
        dir.path().join("CodexStatusOrb").join("health.json")
    );
}

#[test]
fn hub_starts_only_when_installed_and_down() {
    let dir = tempfile::tempdir().unwrap();
    let probes = FakeProbes::new();
    run_steps(&layout(dir.path()), &probes);
    assert!(probes.launches().is_empty());

    let hub = native_hub_binary(dir.path());
    touch(&hub);
    let mut healthy = FakeProbes::new();
    healthy.is_hub_healthy = true;
    run_steps(&layout(dir.path()), &healthy);
    assert!(healthy.launches().is_empty());

    run_steps(&layout(dir.path()), &probes);
    let launches = probes.launches();
    assert_eq!(
        launches[0].0,
        vec![hub.display().to_string(), "serve".to_owned()]
    );
    assert_eq!(launches[0].1, hub.parent().unwrap().display().to_string());
}

#[test]
fn second_bootstrap_is_a_no_op_while_one_runs() {
    let dir = tempfile::tempdir().unwrap();
    let first = single_instance(dir.path()).unwrap();
    assert!(first.is_some());
    let second = single_instance(dir.path()).unwrap();
    assert!(second.is_none());
    drop(first);
    assert!(single_instance(dir.path()).unwrap().is_some());
}

#[test]
fn every_step_is_logged() {
    let dir = tempfile::tempdir().unwrap();
    run_steps(&layout(dir.path()), &FakeProbes::new());

    let operations: Vec<String> = log_records(dir.path())
        .iter()
        .map(|record| record["operation"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        operations,
        vec![
            "receiver.ensure",
            "orb.ensure",
            "hub.ensure",
            "console-monitor.ensure"
        ]
    );
}

#[test]
fn a_failing_step_is_logged_and_the_rest_still_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut probes = FakeProbes::new();
    probes.launch_error = Some(io::ErrorKind::NotFound);
    touch(&orb_pythonw(dir.path()));
    touch(&native_hub_binary(dir.path()));

    run_steps(&layout(dir.path()), &probes);

    let records = log_records(dir.path());
    let orb = records
        .iter()
        .find(|record| record["operation"] == "orb.ensure")
        .unwrap();
    assert_eq!(orb["outcome"], "failed");
    assert_eq!(orb["details"]["error"], "FileNotFoundError");
    let hub = records
        .iter()
        .find(|record| record["operation"] == "hub.ensure")
        .unwrap();
    assert_eq!(hub["outcome"], "failed");
    assert_eq!(
        result_of(&records, "console-monitor.ensure"),
        "not_installed"
    );
}
