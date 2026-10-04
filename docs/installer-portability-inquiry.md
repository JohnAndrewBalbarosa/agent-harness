# Installer portability inquiry

Status: owner approved the three fixture-only substitutions; assertions unchanged.

- Failing check: `tests/test_installer.ps1:106–107` rejects literal Windows user paths except its existing synthetic `x` and `Juan dela Cruz` fixtures.
- Evidence: unchanged HEAD `crates/harness-services/tests/console_monitor_logic.rs:35,39,135` uses a different synthetic username; the installer reports those three existing fixtures as non-portable.
- Observed: the installer sandbox runs, then its source-portability assertion fails. This failure predates the Markdown change.
- Expected: synthetic profile-path fixtures remain portable under the existing scanner, while profile selection, case-insensitivity and process ancestry assertions retain their behavior.
- Proposed resolution: replace only the synthetic fixture username with the scanner's existing `x` placeholder. Do not relax the scanner, change assertions, skip checks or touch real runtime configuration.
- Checks: rerun complete Rust/Python/installer/secret/E2E passes; verify the fixture diff changes only synthetic paths.
- Recovery: restore the fixture-only diff; no live configuration or database writes.
