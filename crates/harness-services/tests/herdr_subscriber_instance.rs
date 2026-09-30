//! Single-instance lock/pid files and the event recorder's log + spool output.

use std::collections::BTreeMap;

use harness_hook::consumers::herdr_bridge::background_launch::subscriber_lock_path;
use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use harness_services::herdr_subscriber::event_recorder::EventRecorder;
use harness_services::herdr_subscriber::instance_lock::{
    pid_path_for, socket_identity, InstanceAcquisition, SubscriberInstance,
};
use harness_services::herdr_subscriber::stop_signal::is_lock_file_missing;
use harness_services::herdr_subscriber::transport::windows_pipe_address;
use harness_services::service_context::ServiceContext;
use serde_json::{json, Value};

#[test]
fn only_one_instance_holds_a_socket_lock() {
    let root = tempfile::tempdir().unwrap();
    let first = match SubscriberInstance::acquire(root.path(), "herdr-sock").unwrap() {
        InstanceAcquisition::Acquired(instance) => instance,
        InstanceAcquisition::AlreadyRunning => panic!("first acquire must win"),
    };
    let lock_path = subscriber_lock_path(root.path(), "herdr-sock");
    assert_eq!(first.lock_path(), lock_path);
    assert_eq!(first.identity(), socket_identity("herdr-sock"));
    assert_eq!(first.identity().len(), 16);
    let pid_path = pid_path_for(&lock_path);
    assert_eq!(first.pid_path(), pid_path);
    assert_eq!(
        std::fs::read_to_string(&pid_path).unwrap(),
        std::process::id().to_string()
    );

    assert!(matches!(
        SubscriberInstance::acquire(root.path(), "herdr-sock").unwrap(),
        InstanceAcquisition::AlreadyRunning
    ));
    // The hook's probe (`File::try_lock` on a second handle) sees it as running.
    let probe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    assert!(probe.try_lock().is_err());
    assert!(matches!(
        SubscriberInstance::acquire(root.path(), "other-sock").unwrap(),
        InstanceAcquisition::Acquired(_)
    ));

    drop(first);
    assert!(!pid_path.exists());
    assert!(lock_path.exists());
    assert!(matches!(
        SubscriberInstance::acquire(root.path(), "herdr-sock").unwrap(),
        InstanceAcquisition::Acquired(_)
    ));
}

#[test]
fn lock_loss_means_the_lock_file_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("subscriber-x.lock");
    assert!(is_lock_file_missing(&path));
    std::fs::write(&path, b"").unwrap();
    assert!(!is_lock_file_missing(&path));
}

#[test]
fn pipe_address_is_built_like_socket_stream_mjs() {
    assert_eq!(windows_pipe_address("herdr-123"), r"\\.\pipe\herdr-123");
}

fn context_for(root: &std::path::Path) -> ServiceContext {
    let spool = root.join("spool").join("spool.sqlite3");
    ServiceContext {
        harness_root: root.to_path_buf(),
        env: BTreeMap::from([("AH_OBS_SPOOL".to_owned(), spool.display().to_string())]),
    }
}

fn log_records(log: &BridgeLog) -> Vec<Value> {
    std::fs::read_to_string(log.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn bound_events_are_logged_and_enqueued_into_the_spool() {
    let root = tempfile::tempdir().unwrap();
    let context = context_for(root.path());
    let sessions = root
        .path()
        .join("var")
        .join("herdr-bridge")
        .join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("s1.json"),
        json!({"pane_id": "p1", "project_id": "proj-1", "session_id": "s1", "updated_at": 1.0})
            .to_string(),
    )
    .unwrap();
    let log = BridgeLog::under_root(root.path());
    let recorder = EventRecorder::new(&context, &log);

    let bound = json!({"event": {"event": "pane.exited", "pane_id": "p1"}});
    recorder.record(bound.as_object().unwrap(), 1);
    let unbound = json!({"type": "tab.created", "tab": {"paneId": "p9"}});
    recorder.record(unbound.as_object().unwrap(), 2);

    let records = log_records(&log);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["event"], json!("herdr.event"));
    assert_eq!(records[0]["event_name"], json!("pane.exited"));
    assert_eq!(records[0]["pane"], json!("p1"));
    assert_eq!(records[0]["sequence"], json!(1));
    assert_eq!(records[1]["event_name"], json!("tab.created"));
    assert_eq!(records[1]["pane"], json!("p9"));

    let spool = rusqlite::Connection::open(context.spool_path()).unwrap();
    let payloads: Vec<String> = spool
        .prepare("SELECT payload FROM queue")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(payloads.len(), 1);
    let operation: Value = serde_json::from_str(&payloads[0]).unwrap();
    assert_eq!(operation["kind"], json!("herdr-event.record"));
    assert_eq!(operation["data"]["projectId"], json!("proj-1"));
    assert_eq!(operation["data"]["nativeSession"], json!("s1"));
    assert_eq!(operation["data"]["paneId"], json!("p1"));
    assert_eq!(operation["data"]["sequenceNo"], json!(1));
}
