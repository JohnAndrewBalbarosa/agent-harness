# Codex CLI Notify

Codex CLI Notify is a hook-only local notification package for Codex CLI.
It shows desktop notifications when Codex asks for permission, finishes a turn,
or completes a subagent task.

It does not install a Codex skill and it does not send anything to mobile push,
Slack, webhooks, Pushover, or any other external service. Everything stays on
the local computer.

## What It Does

Codex CLI Notify registers Codex hooks for these events:

| Codex hook event | What you get | Enabled by default |
|---|---|---|
| `PermissionRequest` | A local notification when Codex needs approval. On Windows, this can be an actionable approval overlay. | Yes |
| `Stop` | A local notification when the main Codex turn finishes. | Yes |
| `SubagentStop` | A local notification when a Codex subagent finishes. | Yes |

On Windows, `PermissionRequest` can show an actionable toast-style overlay:

| Button | Hook decision returned to Codex | Result |
|---|---|---|
| `Allow once` | `allow` | Approves the current permission request and skips the CLI approval prompt. |
| `Deny` | `deny` | Denies the current permission request. |
| `Use CLI prompt` | No decision | Falls back to the normal Codex CLI approval prompt. |

This does not type into your terminal. The hook returns JSON on stdout, so it
does not depend on terminal focus, window titles, or keyboard simulation.

macOS and Linux currently use native OS notifications for permission requests;
approval still happens in the Codex CLI prompt.

## Platform Support

| OS | Notification method | Color customization | Actionable permission approval |
|---|---|---|---|
| Windows | Local overlay by default, optional native toast fallback | Yes, per event | Yes |
| macOS | `osascript display notification` | No | No |
| Linux | `notify-send` | No | No |

Windows overlay mode is the default because it supports colors and permission
buttons. If you set Windows `mode` to `native`, normal notifications use the
Windows toast API, but actionable permission buttons are only available in
overlay mode.

## Requirements

- Codex CLI with hook support.
- Python 3.8 or newer.
- Windows: Python launcher `py` or `python` available in PowerShell.
- macOS: `python3`; `osascript` is included with macOS.
- Linux: `python3`; `notify-send` installed.

Linux install hint:

```bash
sudo apt install libnotify-bin
```

Use the equivalent package manager command for your distribution if you are not
on Debian or Ubuntu.

## Easy Install

These commands install the notifier globally for the current user. After
installing, restart Codex and run `/hooks`, then trust the `Codex CLI Notify`
hook definitions.

### Windows

PowerShell:

```powershell
cd C:\path\to\codex-cli-notify
py -3 .\scripts\install.py --scope global
```

If `py -3` is not available:

```powershell
python .\scripts\install.py --scope global
```

Installed files:

```text
%USERPROFILE%\.codex\hooks\codex-cli-notify
%USERPROFILE%\.codex\hooks.json
%USERPROFILE%\.codex\codex-cli-notify.json
```

### macOS

Terminal:

```bash
cd /path/to/codex-cli-notify
python3 scripts/install.py --scope global
```

Installed files:

```text
~/.codex/hooks/codex-cli-notify
~/.codex/hooks.json
~/.codex/codex-cli-notify.json
```

### Linux

Terminal:

```bash
cd /path/to/codex-cli-notify
python3 scripts/install.py --scope global
```

If desktop notifications do not appear, install `notify-send` first:

```bash
sudo apt install libnotify-bin
```

Installed files:

```text
~/.codex/hooks/codex-cli-notify
~/.codex/hooks.json
~/.codex/codex-cli-notify.json
```

## Installing From a ZIP

Windows PowerShell:

```powershell
Expand-Archive .\codex-cli-notify-actionable.zip -DestinationPath .
cd .\codex-cli-notify
py -3 .\scripts\install.py --scope global
```

macOS/Linux:

```bash
unzip codex-cli-notify-actionable.zip
cd codex-cli-notify
python3 scripts/install.py --scope global
```

## Repo-Local Install

Use repo-local install when you only want this notifier for one repository.

Windows PowerShell:

```powershell
py -3 .\scripts\install.py --scope repo --repo C:\path\to\repo
```

macOS/Linux:

```bash
python3 scripts/install.py --scope repo --repo /path/to/repo
```

Repo-local files:

```text
/path/to/repo/.codex/hooks/codex-cli-notify
/path/to/repo/.codex/hooks.json
/path/to/repo/.codex/codex-cli-notify.json
```

## Update or Reinstall

Run the installer again:

```bash
python3 scripts/install.py --scope global
```

Windows:

```powershell
py -3 .\scripts\install.py --scope global
```

The installer updates the hook package and hook definitions. It does not
overwrite your existing config file unless you pass `--overwrite-config`.

To overwrite the config with the current default example:

```bash
python3 scripts/install.py --scope global --overwrite-config
```

Windows:

```powershell
py -3 .\scripts\install.py --scope global --overwrite-config
```

## Configuration

Global config file:

```text
~/.codex/codex-cli-notify.json
```

Windows path:

```text
%USERPROFILE%\.codex\codex-cli-notify.json
```

Example config:

```json
{
  "events": ["PermissionRequest", "Stop", "SubagentStop"],
  "log_path": "~/.codex/codex-cli-notify.log",
  "encoding": "utf-8",
  "language": "auto",
  "message_max_chars": 360,
  "windows": {
    "enabled": true,
    "mode": "overlay",
    "duration_ms": 6500,
    "width": 440,
    "margin": 24,
    "position": "bottom-right",
    "color_mode": "accent",
    "opacity": 0.96,
    "colors": {
      "PermissionRequest": "#F59E0B",
      "Stop": "#22C55E",
      "SubagentStop": "#3B82F6",
      "default": "#64748B"
    },
    "permission_actions": {
      "enabled": true,
      "timeout_seconds": 300,
      "timeout_choice": "defer",
      "close_choice": "defer",
      "deny_message": "Denied from the Codex local permission toast.",
      "button_labels": {
        "allow": "Allow once",
        "deny": "Deny",
        "defer": "Use CLI prompt"
      }
    }
  },
  "macos": {"enabled": true},
  "linux": {"enabled": true, "notify_send_path": "notify-send"}
}
```

### Language and Encoding

The default language mode is:

```json
{
  "language": "auto",
  "encoding": "utf-8"
}
```

`language: "auto"` uses the language visible in the hook payload. For example,
if the request contains Korean text, default notification titles and permission
button labels are shown in Korean. If the payload is English, defaults stay in
English.

`encoding` controls how hook stdin/stdout/stderr are decoded and encoded. The
default is `utf-8`, which is the safest default for multilingual hook JSON.

You can temporarily override encoding from the shell that starts Codex:

Windows PowerShell:

```powershell
$env:CODEX_CLI_NOTIFY_ENCODING = "utf-8"
codex
```

macOS/Linux:

```bash
export CODEX_CLI_NOTIFY_ENCODING=utf-8
codex
```

Use `locale` or `auto` only when you intentionally want to use the OS preferred
encoding instead of UTF-8.

### Windows Permission Buttons

Relevant config:

```json
{
  "windows": {
    "mode": "overlay",
    "permission_actions": {
      "enabled": true,
      "timeout_seconds": 300,
      "timeout_choice": "defer",
      "close_choice": "defer",
      "deny_message": "Denied from the Codex local permission toast.",
      "button_labels": {
        "allow": "Allow once",
        "deny": "Deny",
        "defer": "Use CLI prompt"
      }
    }
  }
}
```

`timeout_choice` is used when the permission overlay times out.
`close_choice` is used when the overlay is closed.

Allowed values:

| Value | Meaning |
|---|---|
| `allow` | Automatically approve. |
| `deny` | Automatically deny. |
| `defer` | Use the normal Codex CLI approval prompt. |

The conservative default is `defer`.

### Windows Colors

Use `color_mode: "accent"` to show a colored side accent, or
`color_mode: "background"` to use the event color as the whole notification
background.

```json
{
  "windows": {
    "color_mode": "accent",
    "colors": {
      "PermissionRequest": "#F59E0B",
      "Stop": "#22C55E",
      "SubagentStop": "#3B82F6",
      "default": "#64748B"
    }
  }
}
```

Temporary color overrides:

Windows PowerShell:

```powershell
$env:CODEX_CLI_NOTIFY_WINDOWS_COLOR_PERMISSIONREQUEST = "#EF4444"
$env:CODEX_CLI_NOTIFY_WINDOWS_COLOR_STOP = "#10B981"
$env:CODEX_CLI_NOTIFY_WINDOWS_COLOR_SUBAGENTSTOP = "#6366F1"
codex
```

macOS/Linux:

```bash
export CODEX_CLI_NOTIFY_WINDOWS_COLOR_PERMISSIONREQUEST="#EF4444"
export CODEX_CLI_NOTIFY_WINDOWS_COLOR_STOP="#10B981"
export CODEX_CLI_NOTIFY_WINDOWS_COLOR_SUBAGENTSTOP="#6366F1"
codex
```

Environment overrides only affect hooks started by a Codex process launched
from the same shell.

## Test Notifications

### Show Real Local Notifications

Windows PowerShell:

```powershell
py -3 .\scripts\codex_cli_notify.py --config $env:USERPROFILE\.codex\codex-cli-notify.json --test all
```

macOS/Linux:

```bash
python3 scripts/codex_cli_notify.py --config ~/.codex/codex-cli-notify.json --test all
```

### Test Without Showing Notifications

Dry-run prints the notification payload instead of opening a desktop alert.

```bash
python3 scripts/codex_cli_notify.py --dry-run --test permission
python3 scripts/codex_cli_notify.py --dry-run --test stop
python3 scripts/codex_cli_notify.py --dry-run --test subagent
```

Windows PowerShell:

```powershell
py -3 .\scripts\codex_cli_notify.py --dry-run --test permission
py -3 .\scripts\codex_cli_notify.py --dry-run --test stop
py -3 .\scripts\codex_cli_notify.py --dry-run --test subagent
```

### Test Unicode / Korean Text

These tests preserve multilingual text cases, including Korean permission
request text.

```bash
python3 scripts/codex_cli_notify.py --dry-run --test permission-unicode
python3 scripts/codex_cli_notify.py --dry-run --test stop-unicode
python3 scripts/codex_cli_notify.py --dry-run --test subagent-unicode
```

Windows PowerShell:

```powershell
py -3 .\scripts\codex_cli_notify.py --dry-run --test permission-unicode
py -3 .\scripts\codex_cli_notify.py --dry-run --test stop-unicode
py -3 .\scripts\codex_cli_notify.py --dry-run --test subagent-unicode
```

### Test Permission Decision Stdout

```bash
python3 scripts/codex_cli_notify.py --test permission --simulate-choice allow
python3 scripts/codex_cli_notify.py --test permission --simulate-choice deny
python3 scripts/codex_cli_notify.py --test permission --simulate-choice defer
```

Unicode permission decision test:

```bash
python3 scripts/codex_cli_notify.py --test permission-unicode --simulate-choice deny
```

Behavior:

| Choice | Output |
|---|---|
| `allow` | Prints Codex hook JSON with an allow decision. |
| `deny` | Prints Codex hook JSON with a deny decision and message. |
| `defer` | Prints no decision, so Codex uses the normal CLI approval prompt. |

`Stop` and `SubagentStop` tests print `{"continue":true}` on stdout for hook
compatibility. Dry-run notification summaries are printed to stderr.

## Troubleshooting

### Notifications Do Not Show

1. Restart Codex after installing.
2. Run `/hooks` in Codex and trust `Codex CLI Notify`.
3. Confirm that the config file exists.
4. Run a dry-run test.
5. On Linux, confirm that `notify-send` exists:

```bash
which notify-send
```

### Korean or Unicode Text Looks Broken

The default config uses UTF-8:

```json
{
  "encoding": "utf-8",
  "language": "auto"
}
```

If the hook is launched from a shell with unusual encoding settings, set:

Windows PowerShell:

```powershell
$env:CODEX_CLI_NOTIFY_ENCODING = "utf-8"
codex
```

Then run:

```powershell
py -3 .\scripts\codex_cli_notify.py --dry-run --test permission-unicode
```

On Windows, the overlay also chooses a CJK-capable font when the notification
contains CJK text.

### Windows Permission Buttons Do Not Appear

Check that Windows mode is `overlay`:

```json
{
  "windows": {
    "mode": "overlay",
    "permission_actions": {
      "enabled": true
    }
  }
}
```

If mode is `native`, permission requests are shown as normal notifications and
approval remains in the Codex CLI prompt.

## Uninstall

Global uninstall:

```bash
python3 scripts/uninstall.py --scope global
```

Windows:

```powershell
py -3 .\scripts\uninstall.py --scope global
```

Remove the config file too:

```bash
python3 scripts/uninstall.py --scope global --remove-config
```

Remove legacy skill folders from older notifier versions:

```bash
python3 scripts/uninstall.py --scope global --remove-legacy-skill
```

## Migration From Older Skill Versions

This package replaces older `Codex Push Notifier` and `Codex Local Notifier`
hook entries with `Codex CLI Notify`.

It does not install or require a Codex skill. If you previously installed an
older skill-based version, you can remove legacy skill folders during install:

```bash
python3 scripts/install.py --scope global --remove-legacy-skill
```

Windows:

```powershell
py -3 .\scripts\install.py --scope global --remove-legacy-skill
```

## Korean Guide / 한국어 안내

Codex CLI Notify는 Codex CLI hook 전용 로컬 알림 패키지입니다. Codex가
권한 승인을 요청하거나, 작업 turn이 끝나거나, subagent 작업이 끝났을 때
PC 안에서만 알림을 띄웁니다.

외부 서버, 모바일 push, Slack, webhook, Pushover로 전송하지 않습니다.
`SKILL.md`를 설치하는 Skill 패키지도 아닙니다.

### 빠른 설치

Windows PowerShell:

```powershell
cd C:\path\to\codex-cli-notify
py -3 .\scripts\install.py --scope global
```

macOS:

```bash
cd /path/to/codex-cli-notify
python3 scripts/install.py --scope global
```

Linux:

```bash
cd /path/to/codex-cli-notify
python3 scripts/install.py --scope global
```

Linux에서 알림이 뜨지 않으면 `notify-send`를 설치하세요.

```bash
sudo apt install libnotify-bin
```

설치 후 Codex를 재시작하고 `/hooks`에서 `Codex CLI Notify` hook을 trust
처리해야 합니다.

### 주요 동작

- `PermissionRequest`: 권한 요청 알림.
- `Stop`: 메인 작업 완료 알림.
- `SubagentStop`: subagent 작업 완료 알림.
- Windows에서는 `PermissionRequest`를 버튼이 있는 overlay로 표시할 수
  있습니다.
- `Allow once`는 현재 요청만 승인합니다.
- `Deny`는 현재 요청을 거부합니다.
- `Use CLI prompt`는 Codex CLI의 기존 승인 프롬프트로 넘깁니다.

### 한글 및 인코딩

기본 설정은 다음과 같습니다.

```json
{
  "encoding": "utf-8",
  "language": "auto"
}
```

`language: "auto"`는 hook payload에 들어온 사용자 언어를 보고 기본 알림
문구를 선택합니다. 한글 권한 요청이면 기본 제목과 버튼 문구가 한국어로
표시됩니다.

한글 권한 요청 테스트:

```powershell
py -3 .\scripts\codex_cli_notify.py --dry-run --test permission-unicode
```

macOS/Linux:

```bash
python3 scripts/codex_cli_notify.py --dry-run --test permission-unicode
```

여전히 한글이 깨지면 Codex를 시작하는 셸에서 UTF-8을 명시하세요.

Windows PowerShell:

```powershell
$env:CODEX_CLI_NOTIFY_ENCODING = "utf-8"
codex
```

macOS/Linux:

```bash
export CODEX_CLI_NOTIFY_ENCODING=utf-8
codex
```
