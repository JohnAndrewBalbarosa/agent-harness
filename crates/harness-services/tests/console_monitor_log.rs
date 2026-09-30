//! Console monitor record shape, home resolution, and 1 MiB x 2 rotation.

use std::fs;

use harness_services::console_monitor::monitor_log::{
    backup_path, render_line, rotate_if_needed, MonitorLog, LOG_FILE_NAME,
};
use harness_services::console_monitor::monitor_records::{
    hook_failed, inspect_failed, start_failed, started, stopped, window_visible, VisibleWindow,
};
use harness_services::console_monitor::process_ancestry::{Ancestry, AncestryLink};

fn codex_ancestry() -> Ancestry {
    Ancestry {
        chain: vec![
            AncestryLink {
                pid: 40,
                process: "cmd.exe".into(),
                parent_pid: 20,
            },
            AncestryLink {
                pid: 20,
                process: "codex.exe".into(),
                parent_pid: 10,
            },
        ],
        profile: "personal",
        codex_pid: Some(20),
    }
}

#[test]
fn visible_record_matches_python_bytes() {
    let window = VisibleWindow {
        event_id: 0x8002,
        pid: 40,
        parent_pid: 20,
        process: "cmd.exe",
        window_class: "ConsoleWindowClass",
    };
    let line = render_line(
        "2026-09-30T01:02:03Z",
        &window_visible(&window, &codex_ancestry()),
    )
    .expect("render");
    let expected = concat!(
        r#"{"timestamp":"2026-09-30T01:02:03Z","severity":"info","component":"console-window-monitor","#,
        r#""operation":"console.window.visible","outcome":"observed","details":{"eventId":32770,"pid":40,"#,
        r#""parentPid":20,"process":"cmd.exe","windowClass":"ConsoleWindowClass","profile":"personal","#,
        r#""codexAncestorPid":20,"ancestry":[{"pid":40,"process":"cmd.exe","parentPid":20},"#,
        r#"{"pid":20,"process":"codex.exe","parentPid":10}]}}"#,
        "\n"
    );
    assert_eq!(line, expected);
}

#[test]
fn lifecycle_records_match_python_details() {
    let render = |record| render_line("T", &record).expect("render");
    assert!(render(start_failed(5)).contains(
        r#""severity":"error","component":"console-window-monitor","operation":"monitor.start","outcome":"failed","details":{"reason":"mutex_unavailable","errorCode":5}"#
    ));
    assert!(render(hook_failed(3, 87)).contains(
        r#""operation":"monitor.hook","outcome":"failed","details":{"eventId":3,"errorCode":87}"#
    ));
    assert!(render(started(9)).contains(
        r#""operation":"monitor.start","outcome":"succeeded","details":{"processId":9,"windowMode":"hidden"}"#
    ));
    assert!(render(stopped(9))
        .contains(r#""operation":"monitor.stop","outcome":"completed","details":{"processId":9}"#));
    assert!(render(inspect_failed("panic")).contains(
        r#""severity":"error","component":"console-window-monitor","operation":"console.window.inspect","outcome":"failed","details":{"errorType":"panic"}"#
    ));
}

#[test]
fn non_ascii_is_kept_verbatim() {
    let window = VisibleWindow {
        event_id: 3,
        pid: 1,
        parent_pid: 0,
        process: "cmd.exe",
        window_class: "Kónsole",
    };
    let line = render_line("T", &window_visible(&window, &codex_ancestry())).expect("render");
    assert!(line.contains("\"windowClass\":\"Kónsole\""));
}

#[test]
fn home_resolution_prefers_userprofile_then_home() {
    let log = MonitorLog::from_env(|key| match key {
        "USERPROFILE" => Some(String::new()),
        "HOME" => Some("/home/u".into()),
        _ => None,
    })
    .expect("home");
    let expected = std::path::Path::new("/home/u")
        .join(".codex-shared")
        .join("var")
        .join("logs")
        .join(LOG_FILE_NAME);
    assert_eq!(log.path(), expected.as_path());
    assert!(MonitorLog::from_env(|_| None).is_none());
}

#[test]
fn emit_creates_directories_and_appends_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = MonitorLog::under_home(dir.path());
    log.emit(&started(1)).expect("first");
    log.emit(&stopped(1)).expect("second");
    let content = fs::read_to_string(log.path()).expect("read");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
    let timestamp = first["timestamp"].as_str().expect("timestamp");
    assert_eq!(timestamp.len(), 20);
    assert!(timestamp.ends_with('Z'));
}

#[test]
fn rotation_shifts_backups_and_keeps_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(LOG_FILE_NAME);
    fs::write(&path, "current-full").expect("write current");
    fs::write(backup_path(&path, 1), "one").expect("write .1");
    fs::write(backup_path(&path, 2), "two").expect("write .2");

    rotate_if_needed(&path, 4, 2).expect("rotate");

    assert!(!path.exists());
    assert_eq!(
        fs::read_to_string(backup_path(&path, 1)).expect(".1"),
        "current-full"
    );
    assert_eq!(
        fs::read_to_string(backup_path(&path, 2)).expect(".2"),
        "one"
    );
    assert!(!backup_path(&path, 3).exists());
}

#[test]
fn rotation_skips_small_or_missing_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(LOG_FILE_NAME);
    rotate_if_needed(&path, 4, 2).expect("missing is fine");
    fs::write(&path, "abc").expect("write");
    rotate_if_needed(&path, 4, 2).expect("small");
    assert!(path.exists());
    assert!(!backup_path(&path, 1).exists());
}
