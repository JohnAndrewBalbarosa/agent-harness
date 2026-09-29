//! Port of tests/unit/test_notify_deliver.py (fake runner, no real processes).

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use harness_hook::consumers::notify::decide::Sound;
use harness_hook::consumers::notify::deliver::{
    deliver, CommandOutput, CommandRunner, LaunchFailure, ToastRequest,
};

const DEFAULT_HERDR_STDOUT: &[u8] = br#"{"result":{"shown":true,"reason":"shown"}}"#;

struct FakeRun {
    calls: Mutex<Vec<Vec<String>>>,
    herdr_stdout: Vec<u8>,
    fail_herdr: bool,
    fail_windows: bool,
}

impl FakeRun {
    fn new() -> Self {
        Self::with_stdout(DEFAULT_HERDR_STDOUT)
    }

    fn with_stdout(stdout: &[u8]) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            herdr_stdout: stdout.to_vec(),
            fail_herdr: false,
            fail_windows: false,
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeRun {
    fn run(&self, args: &[String], _timeout: Duration) -> Result<CommandOutput, LaunchFailure> {
        self.calls.lock().unwrap().push(args.to_vec());
        let is_herdr = args.iter().any(|arg| arg == "notification");
        if (is_herdr && self.fail_herdr) || (!is_herdr && self.fail_windows) {
            return Err(LaunchFailure::OS_ERROR);
        }
        Ok(CommandOutput {
            exit_code: 0,
            stdout: if is_herdr {
                self.herdr_stdout.clone()
            } else {
                Vec::new()
            },
        })
    }
}

fn done(title: &str) -> ToastRequest<'_> {
    ToastRequest {
        title,
        message: "Tapos na ang prompt!",
        sound: Sound::Done,
    }
}

fn script() -> &'static Path {
    Path::new("toast.ps1")
}

#[test]
fn outside_herdr_windows_toast_with_sound() {
    let run = FakeRun::new();
    let result = deliver(&done("Claude Code"), None, &run, script());
    assert_eq!(result, "windows:handed");
    let calls = run.calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].contains(&"-Silent".to_owned()));
    assert!(calls[0].contains(&"Tapos na ang prompt!".to_owned()));
    assert_eq!(
        &calls[0][..5],
        [
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File"
        ]
    );
}

#[test]
fn inside_herdr_bell_plus_silent_windows_popup() {
    let run = FakeRun::new();
    let result = deliver(
        &done("Codex (cy)"),
        Some(Path::new("C:/h/herdr.exe")),
        &run,
        script(),
    );
    assert_eq!(result, "herdr:shown + windows:handed(silent)");
    let calls = run.calls();
    let (herdr_call, windows_call) = (&calls[0], &calls[1]);
    assert_eq!(
        &herdr_call[..4],
        ["C:/h/herdr.exe", "notification", "show", "Codex (cy)"]
    );
    let sound_at = herdr_call.iter().position(|arg| arg == "--sound").unwrap();
    assert_eq!(herdr_call[sound_at + 1], "done");
    assert!(windows_call.contains(&"-Silent".to_owned()));
}

#[test]
fn herdr_not_shown_falls_back_to_sounded_windows_toast() {
    let run = FakeRun::with_stdout(br#"{"result": {"shown": false, "reason": "disabled"}}"#);
    let result = deliver(
        &done("Claude Code"),
        Some(Path::new("herdr.exe")),
        &run,
        script(),
    );
    assert_eq!(result, "herdr:not-shown(disabled) + windows:handed");
    assert!(!run.calls().last().unwrap().contains(&"-Silent".to_owned()));
}

#[test]
fn herdr_failure_falls_back() {
    let run = FakeRun {
        fail_herdr: true,
        ..FakeRun::new()
    };
    let result = deliver(
        &done("Claude Code"),
        Some(Path::new("herdr.exe")),
        &run,
        script(),
    );
    assert_eq!(result, "herdr:error(OSError) + windows:handed");
}

#[test]
fn herdr_garbage_stdout_is_a_decode_error() {
    let run = FakeRun::with_stdout(b"nope");
    let result = deliver(
        &done("Claude Code"),
        Some(Path::new("herdr.exe")),
        &run,
        script(),
    );
    assert_eq!(result, "herdr:error(JSONDecodeError) + windows:handed");
}

#[test]
fn windows_failure_is_reported_not_raised() {
    let run = FakeRun {
        fail_windows: true,
        ..FakeRun::new()
    };
    assert_eq!(
        deliver(&done("Claude Code"), None, &run, script()),
        "windows:error(OSError)"
    );
}

#[test]
fn request_sound_is_forwarded() {
    let run = FakeRun::new();
    let request = ToastRequest {
        title: "Claude Code",
        message: "Kailangan ng input mo!",
        sound: Sound::Request,
    };
    deliver(&request, Some(Path::new("herdr.exe")), &run, script());
    let calls = run.calls();
    let sound_at = calls[0].iter().position(|arg| arg == "--sound").unwrap();
    assert_eq!(calls[0][sound_at + 1], "request");
}
