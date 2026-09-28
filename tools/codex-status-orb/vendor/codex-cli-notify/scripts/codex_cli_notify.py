#!/usr/bin/env python3
"""Local desktop notifications for Codex hook events.

This script is designed to be called by Codex Hooks. It does not send any
network/mobile/webhook notifications. On Windows it uses a small colorable local notification overlay by default.
For PermissionRequest on Windows, the overlay can return allow/deny decisions
back to Codex directly. On macOS/Linux it uses the OS default notifier.
"""
from __future__ import annotations

import argparse
import copy
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import textwrap
import time
import locale
from pathlib import Path
from typing import Any, Dict, Iterable, Mapping, MutableMapping, Optional, Tuple

APP_NAME = "Codex"
PACKAGE_NAME = "codex-cli-notify"
DEFAULT_EVENTS = {"PermissionRequest", "Stop", "SubagentStop"}

DEFAULT_CONFIG: Dict[str, Any] = {
    "events": ["PermissionRequest", "Stop", "SubagentStop"],
    "log_path": "~/.codex/codex-cli-notify.log",
    "encoding": "utf-8",
    "language": "auto",
    "message_max_chars": 360,
    "macos": {
        "enabled": True,
    },
    "linux": {
        "enabled": True,
        "notify_send_path": "notify-send",
    },
    "windows": {
        "enabled": True,
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
            "default": "#64748B",
        },
        "permission_actions": {
            "enabled": True,
            "timeout_seconds": 300,
            "timeout_choice": "defer",
            "close_choice": "defer",
            "deny_message": "Denied from the Codex local permission toast.",
            "button_labels": {
                "allow": "Allow once",
                "deny": "Deny",
                "defer": "Use CLI prompt",
            },
        },
    },
}

_HEX_RE = re.compile(r"^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")
_HANGUL_RE = re.compile(r"[\u1100-\u11FF\u3130-\u318F\uAC00-\uD7A3]")
_CJK_RE = re.compile(r"[\u3040-\u30FF\u3400-\u4DBF\u4E00-\u9FFF\uAC00-\uD7A3]")

_TEXT = {
    "en": {
        "permission_title": "Codex approval requested",
        "stop_title": "Codex task complete",
        "subagent_title": "Codex subtask complete",
        "stop_fallback": "Turn completed.",
        "subagent_fallback": "Subagent completed.",
        "subagent_default": "subagent",
        "unknown_fallback": "Codex event occurred.",
        "permission_allow": "Allow once",
        "permission_deny": "Deny",
        "permission_defer": "Use CLI prompt",
        "deny_message": "Denied from the Codex local permission toast.",
    },
    "ko": {
        "permission_title": "Codex 승인 요청",
        "stop_title": "Codex 작업 완료",
        "subagent_title": "Codex 하위 작업 완료",
        "stop_fallback": "작업이 완료되었습니다.",
        "subagent_fallback": "하위 작업이 완료되었습니다.",
        "subagent_default": "하위 에이전트",
        "unknown_fallback": "Codex 이벤트가 발생했습니다.",
        "permission_allow": "이번만 허용",
        "permission_deny": "거부",
        "permission_defer": "CLI 프롬프트 사용",
        "deny_message": "Codex 로컬 권한 알림에서 거부되었습니다.",
    },
}
_DEFAULT_EN_BUTTON_LABELS = {
    "allow": _TEXT["en"]["permission_allow"],
    "deny": _TEXT["en"]["permission_deny"],
    "defer": _TEXT["en"]["permission_defer"],
}
_DEFAULT_EN_DENY_MESSAGES = {
    _TEXT["en"]["deny_message"],
    "Denied from the Codex CLI Notify permission toast.",
}


def _preferred_encoding(config: Optional[Mapping[str, Any]] = None) -> str:
    raw = os.environ.get("CODEX_CLI_NOTIFY_ENCODING")
    if raw is None:
        raw = os.environ.get("CODEX_LOCAL_NOTIFY_ENCODING")
    if raw is None and config:
        raw = config.get("encoding")
    value = str(raw or "utf-8").strip().lower()
    if value in {"", "auto", "locale"}:
        return locale.getpreferredencoding(False) or "utf-8"
    return value


def _configure_text_streams(encoding: Optional[str] = None) -> None:
    selected = encoding or _preferred_encoding()
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding=selected, errors="replace")
        except Exception:
            pass


_configure_text_streams()


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name)
    if raw is None:
        return default
    return raw.strip().lower() not in {"0", "false", "no", "off", ""}


def _env_first(*names: str) -> Optional[str]:
    for name in names:
        value = os.environ.get(name)
        if value is not None:
            return value
    return None


def _env_bool_first(primary: str, legacy: str, default: bool) -> bool:
    raw = _env_first(primary, legacy)
    if raw is None:
        return default
    return raw.strip().lower() not in {"0", "false", "no", "off", ""}


def _deep_merge(base: Dict[str, Any], overlay: Mapping[str, Any]) -> Dict[str, Any]:
    result = copy.deepcopy(base)
    for key, value in overlay.items():
        if isinstance(value, Mapping) and isinstance(result.get(key), dict):
            result[key] = _deep_merge(result[key], value)  # type: ignore[arg-type]
        else:
            result[key] = copy.deepcopy(value)
    return result


def _default_config_path() -> Path:
    return Path.home() / ".codex" / "codex-cli-notify.json"


def _load_config(path: Optional[str] = None) -> Dict[str, Any]:
    selected = path or _env_first(
        "CODEX_CLI_NOTIFY_CONFIG",
        "CODEX_LOCAL_NOTIFY_CONFIG",
        "_CODEX_CLI_NOTIFY_CONFIG",
        "_CODEX_LOCAL_CONFIG",
    )
    config = copy.deepcopy(DEFAULT_CONFIG)
    if selected:
        config_path = Path(selected).expanduser()
    else:
        config_path = _default_config_path()
    try:
        if config_path.exists():
            with config_path.open("r", encoding="utf-8") as f:
                loaded = json.load(f)
            if isinstance(loaded, Mapping):
                config = _deep_merge(config, loaded)
    except Exception as exc:
        _log(f"config load failed: {config_path}: {exc!r}", config)
    config["_config_path"] = str(config_path)
    return config


def _log_path(config: Optional[Mapping[str, Any]] = None) -> Path:
    explicit = _env_first("CODEX_CLI_NOTIFY_LOG", "CODEX_LOCAL_NOTIFY_LOG")
    if explicit:
        return Path(explicit).expanduser()
    if config:
        raw = config.get("log_path")
        if isinstance(raw, str) and raw.strip():
            return Path(raw).expanduser()
    return Path.home() / ".codex" / "codex-cli-notify.log"


def _log(message: str, config: Optional[Mapping[str, Any]] = None) -> None:
    try:
        path = _log_path(config)
        path.parent.mkdir(parents=True, exist_ok=True)
        timestamp = time.strftime("%Y-%m-%dT%H:%M:%S%z")
        with path.open("a", encoding="utf-8") as f:
            f.write(f"[{timestamp}] {message}\n")
    except Exception:
        # Hooks should never fail just because logging failed.
        pass


def _truncate(text: Any, limit: int = 220) -> str:
    if text is None:
        return ""
    if not isinstance(text, str):
        try:
            text = json.dumps(text, ensure_ascii=False, sort_keys=True)
        except Exception:
            text = str(text)
    text = " ".join(text.split())
    if len(text) <= limit:
        return text
    return text[: max(0, limit - 3)].rstrip() + "..."


def _first_nonempty(*values: Any) -> str:
    for value in values:
        if value is None:
            continue
        if isinstance(value, str):
            if value.strip():
                return value.strip()
        else:
            return str(value)
    return ""


def _contains_hangul(value: Any) -> bool:
    if value is None:
        return False
    if isinstance(value, str):
        return bool(_HANGUL_RE.search(value))
    if isinstance(value, Mapping):
        return any(_contains_hangul(k) or _contains_hangul(v) for k, v in value.items())
    if isinstance(value, Iterable) and not isinstance(value, (bytes, bytearray)):
        return any(_contains_hangul(item) for item in value)
    return False


def _contains_cjk(value: Any) -> bool:
    if value is None:
        return False
    if isinstance(value, str):
        return bool(_CJK_RE.search(value))
    if isinstance(value, Mapping):
        return any(_contains_cjk(k) or _contains_cjk(v) for k, v in value.items())
    if isinstance(value, Iterable) and not isinstance(value, (bytes, bytearray)):
        return any(_contains_cjk(item) for item in value)
    return False


def _ui_font_family(text: Any, *, monospace: bool = False) -> str:
    if _contains_cjk(text):
        if platform.system().lower() == "windows":
            return "Malgun Gothic"
        if platform.system().lower() == "darwin":
            return "Apple SD Gothic Neo"
        return "Noto Sans CJK"
    return "Consolas" if monospace else "Segoe UI"


def _notification_language(payload: Mapping[str, Any], config: Mapping[str, Any]) -> str:
    explicit = _env_first("CODEX_CLI_NOTIFY_LANGUAGE", "CODEX_LOCAL_NOTIFY_LANGUAGE")
    configured = explicit or config.get("language") or "auto"
    language = str(configured).strip().lower()
    if language in {"ko", "kr", "korean", "한국어"}:
        return "ko"
    if language in {"en", "eng", "english"}:
        return "en"
    return "ko" if _contains_hangul(payload) else "en"


def _localized_text(language: str) -> Mapping[str, str]:
    return _TEXT.get(language, _TEXT["en"])


def _payload_event_name(payload: Mapping[str, Any]) -> str:
    return str(
        payload.get("hook_event_name")
        or payload.get("hookEventName")
        or payload.get("event")
        or payload.get("type")
        or "unknown"
    )


def _command_from_tool_input(tool_input: Any) -> str:
    if isinstance(tool_input, Mapping):
        return _first_nonempty(tool_input.get("command"), tool_input.get("description"), tool_input)
    return _first_nonempty(tool_input)


def _compose_notification(payload: Mapping[str, Any], config: Mapping[str, Any]) -> Tuple[str, str, str]:
    """Return (event_name, title, body)."""
    event_name = _payload_event_name(payload)
    language = _notification_language(payload, config)
    text = _localized_text(language)
    max_chars = int(config.get("message_max_chars") or 360)
    cwd = _truncate(payload.get("cwd"), 90)
    model = _truncate(payload.get("model"), 60)
    suffix_bits = [bit for bit in (cwd, model) if bit]
    suffix = " | ".join(suffix_bits)

    if event_name == "PermissionRequest":
        tool_name = _truncate(payload.get("tool_name") or "permission", 60)
        tool_input = payload.get("tool_input")
        description = ""
        if isinstance(tool_input, Mapping):
            description = _first_nonempty(tool_input.get("description"), tool_input.get("reason"))
        command = _truncate(_command_from_tool_input(tool_input), max_chars)
        detail = _first_nonempty(description, command, suffix)
        body = f"{tool_name}: {detail}" if detail else tool_name
        if suffix and suffix not in body:
            body = f"{body}\n{suffix}"
        return event_name, text["permission_title"], _truncate(body, max_chars)

    if event_name == "Stop":
        last = _truncate(payload.get("last_assistant_message"), max_chars)
        body = _first_nonempty(last, suffix, text["stop_fallback"])
        if suffix and suffix not in body:
            body = f"{body}\n{suffix}"
        return event_name, text["stop_title"], _truncate(body, max_chars)

    if event_name == "SubagentStop":
        agent_type = _truncate(payload.get("agent_type") or text["subagent_default"], 70)
        last = _truncate(payload.get("last_assistant_message"), max_chars)
        body = _first_nonempty(last, suffix, text["subagent_fallback"])
        body = f"{agent_type}: {body}"
        if suffix and suffix not in body:
            body = f"{body}\n{suffix}"
        return event_name, text["subagent_title"], _truncate(body, max_chars)

    body = _first_nonempty(_truncate(payload, max_chars), text["unknown_fallback"])
    return event_name, f"Codex: {event_name}", body


def _allowed_event(event_name: str, config: Mapping[str, Any]) -> bool:
    configured = _env_first("CODEX_CLI_NOTIFY_EVENTS", "CODEX_LOCAL_NOTIFY_EVENTS")
    if configured:
        allowed = {x.strip() for x in configured.split(",") if x.strip()}
        return "*" in allowed or event_name in allowed
    raw = config.get("events", list(DEFAULT_EVENTS))
    if isinstance(raw, str):
        allowed = {x.strip() for x in raw.split(",") if x.strip()}
    elif isinstance(raw, Iterable):
        allowed = {str(x).strip() for x in raw if str(x).strip()}
    else:
        allowed = DEFAULT_EVENTS
    return "*" in allowed or event_name in allowed


def _run(cmd: Iterable[str], *, env: Optional[Dict[str, str]] = None, timeout: float = 5.0) -> bool:
    try:
        subprocess.run(
            list(cmd),
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            timeout=timeout,
            check=False,
            creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)),
        )
        return True
    except Exception as exc:
        _log(f"command notification failed: {exc!r}")
        return False


def _notify_macos(title: str, message: str, config: Mapping[str, Any]) -> bool:
    if not bool(config.get("macos", {}).get("enabled", True)):
        return False
    osascript = shutil.which("osascript")
    if not osascript:
        return False
    script = f"display notification {json.dumps(message)} with title {json.dumps(title)}"
    return _run([osascript, "-e", script])


def _notify_linux(title: str, message: str, config: Mapping[str, Any]) -> bool:
    linux_cfg = config.get("linux", {}) if isinstance(config.get("linux"), Mapping) else {}
    if not bool(linux_cfg.get("enabled", True)):
        return False
    notify_send_name = str(linux_cfg.get("notify_send_path") or "notify-send")
    notify_send = shutil.which(notify_send_name) or (notify_send_name if Path(notify_send_name).exists() else None)
    if not notify_send:
        return False
    return _run([notify_send, "--app-name", APP_NAME, title, message])


def _safe_hex_color(value: Any, fallback: str = "#64748B") -> str:
    if not isinstance(value, str):
        value = fallback
    match = _HEX_RE.match(value.strip())
    if not match:
        match = _HEX_RE.match(fallback)
    if not match:
        return "#64748B"
    digits = match.group(1)
    if len(digits) == 3:
        digits = "".join(ch * 2 for ch in digits)
    return "#" + digits.upper()


def _event_env_suffix(event_name: str) -> str:
    return re.sub(r"[^A-Z0-9]+", "_", event_name.upper()).strip("_") or "DEFAULT"


def _windows_color_for(event_name: str, config: Mapping[str, Any]) -> str:
    suffix = _event_env_suffix(event_name)
    env_keys = [
        f"CODEX_CLI_NOTIFY_WINDOWS_COLOR_{suffix}",
        f"CODEX_CLI_NOTIFY_COLOR_{suffix}",
        f"CODEX_LOCAL_NOTIFY_WINDOWS_COLOR_{suffix}",
        f"CODEX_LOCAL_NOTIFY_COLOR_{suffix}",
    ]
    for key in env_keys:
        value = os.environ.get(key)
        if value:
            return _safe_hex_color(value)
    windows_cfg = config.get("windows", {}) if isinstance(config.get("windows"), Mapping) else {}
    colors = windows_cfg.get("colors", {}) if isinstance(windows_cfg.get("colors"), Mapping) else {}
    return _safe_hex_color(colors.get(event_name) or colors.get("default") or "#64748B")




def _windows_cfg(config: Mapping[str, Any]) -> Mapping[str, Any]:
    return config.get("windows", {}) if isinstance(config.get("windows"), Mapping) else {}


def _windows_permission_actions_cfg(config: Mapping[str, Any]) -> Mapping[str, Any]:
    windows_cfg = _windows_cfg(config)
    actions = windows_cfg.get("permission_actions", {})
    return actions if isinstance(actions, Mapping) else {}


def _normalize_permission_choice(value: Any, fallback: str = "defer") -> str:
    choice = str(value or "").strip().lower().replace("-", "_")
    aliases = {
        "approve": "allow",
        "approved": "allow",
        "yes": "allow",
        "y": "allow",
        "allow_once": "allow",
        "block": "deny",
        "denied": "deny",
        "no": "deny",
        "n": "deny",
        "cli": "defer",
        "ask": "defer",
        "continue": "defer",
        "default": "defer",
        "none": "defer",
    }
    choice = aliases.get(choice, choice)
    if choice in {"allow", "deny", "defer"}:
        return choice
    return fallback if fallback in {"allow", "deny", "defer"} else "defer"


def _permission_button_label(labels: Mapping[str, Any], key: str, language: str) -> str:
    raw = labels.get(key)
    if raw and str(raw) != _DEFAULT_EN_BUTTON_LABELS[key]:
        return str(raw)
    return _localized_text(language)[f"permission_{key}"]


def _permission_decision_stdout(choice: str, config: Mapping[str, Any], language: str = "en") -> str:
    normalized = _normalize_permission_choice(choice)
    if normalized == "allow":
        payload = {
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow"},
            }
        }
        return json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    if normalized == "deny":
        actions_cfg = _windows_permission_actions_cfg(config)
        raw_message = actions_cfg.get("deny_message")
        if raw_message and str(raw_message) not in _DEFAULT_EN_DENY_MESSAGES:
            message = str(raw_message)
        else:
            message = _localized_text(language)["deny_message"]
        payload = {
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "deny", "message": message},
            }
        }
        return json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    return ""


def _permission_action_title_and_body(payload: Mapping[str, Any], config: Mapping[str, Any]) -> Tuple[str, str, str, str]:
    """Return (event_name, title, body, command_text) for PermissionRequest UI."""
    event_name, title, body = _compose_notification(payload, config)
    command_text = _command_from_tool_input(payload.get("tool_input"))
    command_text = _first_nonempty(command_text, body)
    return event_name, title, body, command_text

def _contrast_text(hex_color: str) -> str:
    color = _safe_hex_color(hex_color).lstrip("#")
    r = int(color[0:2], 16) / 255.0
    g = int(color[2:4], 16) / 255.0
    b = int(color[4:6], 16) / 255.0
    def channel(c: float) -> float:
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    luminance = 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    return "#000000" if luminance > 0.56 else "#FFFFFF"


def _pythonw_command() -> list[str]:
    exe = Path(sys.executable)
    if exe.name.lower() == "python.exe":
        candidate = exe.with_name("pythonw.exe")
        if candidate.exists():
            return [str(candidate)]
    pythonw = shutil.which("pythonw.exe")
    if pythonw:
        return [pythonw]
    pyw = shutil.which("pyw.exe")
    if pyw:
        return [pyw, "-3"]
    return [sys.executable]


def _launch_windows_overlay(event_name: str, title: str, message: str, config: Mapping[str, Any]) -> bool:
    env = os.environ.copy()
    env["_CODEX_CLI_NOTIFY_EVENT"] = event_name
    env["_CODEX_CLI_NOTIFY_TITLE"] = title
    env["_CODEX_CLI_NOTIFY_BODY"] = message
    env["_CODEX_CLI_NOTIFY_CONFIG"] = str(config.get("_config_path") or _default_config_path())
    env["_CODEX_CLI_NOTIFY_CONFIG_JSON"] = json.dumps(config, ensure_ascii=False, separators=(",", ":"))
    env["PYTHONIOENCODING"] = _preferred_encoding(config)
    env["CODEX_CLI_NOTIFY_ENCODING"] = _preferred_encoding(config)
    env["_CODEX_LOCAL_EVENT"] = event_name
    env["_CODEX_LOCAL_TITLE"] = title
    env["_CODEX_LOCAL_BODY"] = message
    env["_CODEX_LOCAL_CONFIG"] = str(config.get("_config_path") or _default_config_path())
    env["_CODEX_LOCAL_CONFIG_JSON"] = json.dumps(config, ensure_ascii=False, separators=(",", ":"))
    cmd = _pythonw_command() + [str(Path(__file__).resolve()), "--show-windows-overlay"]
    creationflags = 0
    for name in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"):
        creationflags |= int(getattr(subprocess, name, 0))
    try:
        subprocess.Popen(
            cmd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=creationflags,
        )
        return True
    except Exception as exc:
        _log(f"windows overlay launch failed: {exc!r}", config)
        return False


def _notify_windows_native(title: str, message: str, config: Mapping[str, Any]) -> bool:
    ps = shutil.which("pwsh") or shutil.which("powershell") or shutil.which("powershell.exe")
    if not ps:
        return False
    script = r'''
$title = if ($env:_CODEX_CLI_NOTIFY_TITLE) { $env:_CODEX_CLI_NOTIFY_TITLE } else { $env:_CODEX_LOCAL_TITLE }
$message = if ($env:_CODEX_CLI_NOTIFY_BODY) { $env:_CODEX_CLI_NOTIFY_BODY } else { $env:_CODEX_LOCAL_BODY }
try {
  [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
  [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
  $escTitle = [Security.SecurityElement]::Escape($title)
  $escMessage = [Security.SecurityElement]::Escape($message)
  $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
  $xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$escTitle</text><text>$escMessage</text></binding></visual></toast>")
  $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
  [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Codex').Show($toast)
} catch {
  try { [console]::beep(880, 220) } catch {}
}
'''
    env = os.environ.copy()
    env["_CODEX_CLI_NOTIFY_TITLE"] = title
    env["_CODEX_CLI_NOTIFY_BODY"] = message
    env["PYTHONIOENCODING"] = _preferred_encoding(config)
    env["CODEX_CLI_NOTIFY_ENCODING"] = _preferred_encoding(config)
    env["_CODEX_LOCAL_TITLE"] = title
    env["_CODEX_LOCAL_BODY"] = message
    return _run([ps, "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script], env=env, timeout=5)


def _notify_windows(event_name: str, title: str, message: str, config: Mapping[str, Any]) -> bool:
    windows_cfg = config.get("windows", {}) if isinstance(config.get("windows"), Mapping) else {}
    if not bool(windows_cfg.get("enabled", True)):
        return False
    mode = _env_first("CODEX_CLI_NOTIFY_WINDOWS_MODE", "CODEX_LOCAL_NOTIFY_WINDOWS_MODE") or str(windows_cfg.get("mode") or "overlay")
    mode = mode.strip().lower()
    if mode == "native":
        return _notify_windows_native(title, message, config)
    return _launch_windows_overlay(event_name, title, message, config) or _notify_windows_native(title, message, config)


def _notify_desktop(event_name: str, title: str, message: str, config: Mapping[str, Any]) -> bool:
    if not _env_bool_first("CODEX_CLI_NOTIFY_ENABLED", "CODEX_LOCAL_NOTIFY_ENABLED", True):
        return False
    if _env_bool_first("CODEX_CLI_NOTIFY_DRY_RUN", "CODEX_LOCAL_NOTIFY_DRY_RUN", False):
        print(json.dumps({"event": event_name, "title": title, "message": message}, ensure_ascii=False), file=sys.stderr)
        return True
    system = platform.system().lower()
    if system == "windows":
        return _notify_windows(event_name, title, message, config)
    if system == "darwin":
        return _notify_macos(title, message, config)
    if system == "linux":
        return _notify_linux(title, message, config)
    return False


def send_notification(payload: Mapping[str, Any], config: Mapping[str, Any]) -> None:
    event_name, title, message = _compose_notification(payload, config)
    if not _allowed_event(event_name, config):
        return
    delivered = _notify_desktop(event_name, title, message, config)
    if not delivered and not _env_bool_first("CODEX_CLI_NOTIFY_ALLOW_SILENT", "CODEX_LOCAL_NOTIFY_ALLOW_SILENT", True):
        _log(f"no local notification channel delivered event={event_name}", config)


def _success_stdout_for_hook(payload: Mapping[str, Any]) -> str:
    event_name = _payload_event_name(payload)
    if event_name in {"Stop", "SubagentStop"}:
        return json.dumps({"continue": True}, separators=(",", ":"))
    return ""


def _load_payload_from_stdin(config: Optional[Mapping[str, Any]] = None) -> Dict[str, Any]:
    try:
        raw_bytes = sys.stdin.buffer.read()
        raw = raw_bytes.decode(_preferred_encoding(config), errors="replace")
    except Exception:
        raw = sys.stdin.read()
    if not raw.strip():
        return {}
    loaded = json.loads(raw)
    if isinstance(loaded, dict):
        return loaded
    return {"hook_event_name": "unknown", "payload": loaded}


def _test_payload(kind: str) -> Dict[str, Any]:
    unicode_test = kind.endswith("-unicode") or kind.endswith("-ko")
    base_kind = kind
    for suffix in ("-unicode", "-ko"):
        if base_kind.endswith(suffix):
            base_kind = base_kind[: -len(suffix)]
            break
    if base_kind == "permission":
        tool_input = (
            {
                "command": "python -m pip install 예제패키지",
                "description": "네트워크 접근 또는 셸 권한 상승이 필요합니다",
            }
            if unicode_test
            else {"command": "npm install", "description": "Network access or shell escalation required"}
        )
        return {
            "hook_event_name": "PermissionRequest",
            "session_id": "test-session",
            "turn_id": "test-turn",
            "cwd": str(Path.cwd()),
            "model": "test-model",
            "permission_mode": "default",
            "tool_name": "Bash",
            "last_user_message": "hook 알림 테스트에서 한글이 깨지는지 확인해줘" if unicode_test else "Test hook notifications.",
            "tool_input": tool_input,
        }
    if base_kind == "subagent":
        return {
            "hook_event_name": "SubagentStop",
            "session_id": "test-session",
            "turn_id": "test-turn",
            "cwd": str(Path.cwd()),
            "model": "test-model",
            "permission_mode": "default",
            "agent_id": "agent-test",
            "agent_type": "검토자" if unicode_test else "reviewer",
            "last_user_message": "하위 작업 완료 알림을 한국어로 확인해줘" if unicode_test else "Test subagent notifications.",
            "last_assistant_message": "하위 작업 검토가 완료되었습니다." if unicode_test else "Subagent review finished.",
        }
    return {
        "hook_event_name": "Stop",
        "session_id": "test-session",
        "turn_id": "test-turn",
        "cwd": str(Path.cwd()),
        "model": "test-model",
        "permission_mode": "default",
        "stop_hook_active": False,
        "last_user_message": "hook 알림 테스트에서 한글이 깨지는지 확인해줘" if unicode_test else "Test hook notifications.",
        "last_assistant_message": "작업이 성공적으로 완료되었습니다." if unicode_test else "Task completed successfully.",
    }


def _load_config_for_overlay() -> Dict[str, Any]:
    raw = _env_first("_CODEX_CLI_NOTIFY_CONFIG_JSON", "_CODEX_LOCAL_CONFIG_JSON")
    if raw:
        try:
            loaded = json.loads(raw)
            if isinstance(loaded, dict):
                return _deep_merge(DEFAULT_CONFIG, loaded)
        except Exception:
            pass
    return _load_config(_env_first("_CODEX_CLI_NOTIFY_CONFIG", "_CODEX_LOCAL_CONFIG"))




def _show_windows_permission_prompt(payload: Mapping[str, Any], config: Mapping[str, Any]) -> str:
    """Show a blocking Windows toast-style prompt and return allow/deny/defer.

    This is intentionally implemented as an in-process overlay instead of a
    detached notification so Codex can read the hook's stdout decision.
    """
    try:
        import tkinter as tk
        from tkinter import scrolledtext
    except Exception as exc:
        _log(f"tkinter unavailable for permission prompt: {exc!r}", config)
        return "defer"

    event_name, title, body, command_text = _permission_action_title_and_body(payload, config)
    language = _notification_language(payload, config)
    windows_cfg = _windows_cfg(config)
    actions_cfg = _windows_permission_actions_cfg(config)

    color = _windows_color_for(event_name, config)
    width = max(360, min(860, int(windows_cfg.get("width") or 440)))
    margin = max(0, min(96, int(windows_cfg.get("margin") or 24)))
    opacity = float(windows_cfg.get("opacity") or 0.96)
    opacity = min(1.0, max(0.3, opacity))
    position = str(windows_cfg.get("position") or "bottom-right").lower()
    color_mode = str(windows_cfg.get("color_mode") or "accent").lower()
    timeout_seconds = max(0, min(3600, int(actions_cfg.get("timeout_seconds") or 300)))
    timeout_choice = _normalize_permission_choice(actions_cfg.get("timeout_choice"), "defer")
    close_choice = _normalize_permission_choice(actions_cfg.get("close_choice"), "defer")
    labels = actions_cfg.get("button_labels", {}) if isinstance(actions_cfg.get("button_labels"), Mapping) else {}
    allow_label = _permission_button_label(labels, "allow", language)
    deny_label = _permission_button_label(labels, "deny", language)
    defer_label = _permission_button_label(labels, "defer", language)
    max_chars = int(config.get("message_max_chars") or 360)
    body = _truncate(body, max_chars)
    command_text = _truncate(command_text, max(700, max_chars * 2))
    label_font = _ui_font_family(f"{title}\n{body}")
    command_font = _ui_font_family(command_text, monospace=True)
    result = {"choice": close_choice}

    if color_mode == "background":
        bg = color
        fg = _contrast_text(color)
        title_fg = fg
        body_fg = fg
        panel_bg = color
        accent_width = 0
        button_bg = "#111827" if fg == "#FFFFFF" else "#F8FAFC"
        button_fg = _contrast_text(button_bg)
    else:
        bg = "#111827"
        panel_bg = "#111827"
        fg = "#FFFFFF"
        title_fg = "#FFFFFF"
        body_fg = "#E5E7EB"
        accent_width = 9
        button_bg = "#1F2937"
        button_fg = "#FFFFFF"

    try:
        root = tk.Tk()
        root.title("Codex approval")
        root.overrideredirect(True)
        root.configure(bg=color)
        try:
            root.attributes("-topmost", True)
        except Exception:
            pass
        try:
            root.attributes("-alpha", opacity)
        except Exception:
            pass
        try:
            root.attributes("-toolwindow", True)
        except Exception:
            pass

        outer = tk.Frame(root, bg=color, borderwidth=0)
        outer.pack(fill="both", expand=True)
        if accent_width > 0:
            accent = tk.Frame(outer, bg=color, width=accent_width, borderwidth=0)
            accent.pack(side="left", fill="y")
        content = tk.Frame(outer, bg=panel_bg, padx=14, pady=12, borderwidth=0)
        content.pack(side="left", fill="both", expand=True)

        title_label = tk.Label(
            content,
            text=title,
            bg=panel_bg,
            fg=title_fg,
            anchor="w",
            justify="left",
            font=(label_font, 10, "bold"),
        )
        title_label.pack(fill="x", anchor="w")

        body_label = tk.Label(
            content,
            text=body,
            bg=panel_bg,
            fg=body_fg,
            anchor="w",
            justify="left",
            wraplength=max(280, width - 60),
            font=(label_font, 9),
        )
        body_label.pack(fill="x", anchor="w", pady=(6, 8))

        command_box = scrolledtext.ScrolledText(
            content,
            height=4,
            wrap="word",
            bg="#0B1220" if color_mode != "background" else panel_bg,
            fg=body_fg,
            insertbackground=body_fg,
            relief="flat",
            borderwidth=0,
            font=(command_font, 8),
        )
        command_box.insert("1.0", command_text)
        command_box.configure(state="disabled")
        command_box.pack(fill="x", expand=False, pady=(0, 10))

        button_row = tk.Frame(content, bg=panel_bg, borderwidth=0)
        button_row.pack(fill="x", anchor="e")

        def choose(choice: str) -> None:
            result["choice"] = _normalize_permission_choice(choice)
            try:
                root.destroy()
            except Exception:
                pass

        common_button = {
            "font": (label_font, 9),
            "relief": "flat",
            "borderwidth": 0,
            "padx": 12,
            "pady": 7,
            "cursor": "hand2",
        }

        # Safe/default path is first and gets focus. Allow is deliberately last.
        defer_btn = tk.Button(
            button_row,
            text=defer_label,
            command=lambda: choose("defer"),
            bg=button_bg,
            fg=button_fg,
            activebackground=button_bg,
            activeforeground=button_fg,
            **common_button,
        )
        defer_btn.pack(side="right", padx=(8, 0))
        deny_btn = tk.Button(
            button_row,
            text=deny_label,
            command=lambda: choose("deny"),
            bg="#7F1D1D",
            fg="#FFFFFF",
            activebackground="#991B1B",
            activeforeground="#FFFFFF",
            **common_button,
        )
        deny_btn.pack(side="right", padx=(8, 0))
        allow_btn = tk.Button(
            button_row,
            text=allow_label,
            command=lambda: choose("allow"),
            bg=color,
            fg=_contrast_text(color),
            activebackground=color,
            activeforeground=_contrast_text(color),
            **common_button,
        )
        allow_btn.pack(side="right", padx=(8, 0))

        hint_text = "A = allow, D = deny, Esc = CLI prompt"
        hint = tk.Label(content, text=hint_text, bg=panel_bg, fg="#9CA3AF", anchor="w", font=("Segoe UI", 8))
        hint.pack(fill="x", pady=(8, 0))

        def on_close(_: Any = None) -> None:
            choose(close_choice)

        root.bind("<Escape>", lambda _: choose("defer"))
        root.bind("a", lambda _: choose("allow"))
        root.bind("A", lambda _: choose("allow"))
        root.bind("d", lambda _: choose("deny"))
        root.bind("D", lambda _: choose("deny"))
        root.protocol("WM_DELETE_WINDOW", on_close)

        root.update_idletasks()
        req_h = root.winfo_reqheight()
        height = max(210, min(420, req_h))
        screen_w = root.winfo_screenwidth()
        screen_h = root.winfo_screenheight()
        if "left" in position:
            x = margin
        else:
            x = max(margin, screen_w - width - margin)
        if "top" in position:
            y = margin
        else:
            y = max(margin, screen_h - height - margin - 16)
        root.geometry(f"{width}x{height}+{x}+{y}")
        defer_btn.focus_set()
        root.after(100, lambda: root.lift())
        if timeout_seconds > 0:
            root.after(timeout_seconds * 1000, lambda: choose(timeout_choice))
        root.mainloop()
    except Exception as exc:
        _log(f"windows permission prompt crashed: {exc!r}", config)
        return "defer"

    return _normalize_permission_choice(result.get("choice"), "defer")


def _permission_action_stdout_or_notify(
    payload: Mapping[str, Any],
    config: Mapping[str, Any],
    *,
    simulate_choice: Optional[str] = None,
) -> str:
    """Handle PermissionRequest and return hook stdout, if any.

    Empty string means no hook decision; Codex will continue its normal CLI approval flow.
    """
    event_name = _payload_event_name(payload)
    if not _allowed_event(event_name, config):
        return ""

    language = _notification_language(payload, config)
    if simulate_choice is not None:
        return _permission_decision_stdout(simulate_choice, config, language)

    system = platform.system().lower()
    actions_cfg = _windows_permission_actions_cfg(config)
    actions_enabled = bool(actions_cfg.get("enabled", True))

    if system == "windows" and actions_enabled and not _env_bool_first("CODEX_CLI_NOTIFY_DRY_RUN", "CODEX_LOCAL_NOTIFY_DRY_RUN", False):
        choice = _show_windows_permission_prompt(payload, config)
        return _permission_decision_stdout(choice, config, language)

    # macOS/Linux, Windows native mode, disabled action mode, and dry-run keep the
    # existing behavior: notify only and let the CLI approval prompt decide.
    send_notification(payload, config)
    return ""

def _show_windows_overlay() -> int:
    # Import lazily so non-Windows/macOS/Linux default notification paths have no tkinter dependency at import time.
    try:
        import tkinter as tk
    except Exception as exc:
        _log(f"tkinter unavailable for windows overlay: {exc!r}")
        return 0

    config = _load_config_for_overlay()
    event_name = _env_first("_CODEX_CLI_NOTIFY_EVENT", "_CODEX_LOCAL_EVENT") or "unknown"
    title = _env_first("_CODEX_CLI_NOTIFY_TITLE", "_CODEX_LOCAL_TITLE") or "Codex"
    message = _env_first("_CODEX_CLI_NOTIFY_BODY", "_CODEX_LOCAL_BODY") or ""

    windows_cfg = config.get("windows", {}) if isinstance(config.get("windows"), Mapping) else {}
    color = _windows_color_for(event_name, config)
    duration_ms = int(windows_cfg.get("duration_ms") or 6500)
    width = max(300, min(760, int(windows_cfg.get("width") or 440)))
    margin = max(0, min(96, int(windows_cfg.get("margin") or 24)))
    opacity = float(windows_cfg.get("opacity") or 0.96)
    opacity = min(1.0, max(0.3, opacity))
    position = str(windows_cfg.get("position") or "bottom-right").lower()
    color_mode = str(windows_cfg.get("color_mode") or "accent").lower()
    max_chars = int(config.get("message_max_chars") or 360)
    message = _truncate(message, max_chars)
    label_font = _ui_font_family(f"{title}\n{message}")

    if color_mode == "background":
        bg = color
        fg = _contrast_text(color)
        title_fg = fg
        body_fg = fg
        accent_width = 0
    else:
        bg = "#111827"
        fg = "#FFFFFF"
        title_fg = "#FFFFFF"
        body_fg = "#E5E7EB"
        accent_width = 9

    try:
        root = tk.Tk()
        root.title("Codex")
        root.overrideredirect(True)
        root.configure(bg=color)
        try:
            root.attributes("-topmost", True)
        except Exception:
            pass
        try:
            root.attributes("-alpha", opacity)
        except Exception:
            pass
        try:
            root.attributes("-toolwindow", True)
        except Exception:
            pass

        outer = tk.Frame(root, bg=color, borderwidth=0)
        outer.pack(fill="both", expand=True)
        if accent_width > 0:
            accent = tk.Frame(outer, bg=color, width=accent_width, borderwidth=0)
            accent.pack(side="left", fill="y")
        content = tk.Frame(outer, bg=bg, padx=14, pady=12, borderwidth=0)
        content.pack(side="left", fill="both", expand=True)

        title_label = tk.Label(
            content,
            text=title,
            bg=bg,
            fg=title_fg,
            anchor="w",
            justify="left",
            font=(label_font, 10, "bold"),
        )
        title_label.pack(fill="x", anchor="w")
        body_label = tk.Label(
            content,
            text=message,
            bg=bg,
            fg=body_fg,
            anchor="w",
            justify="left",
            wraplength=max(240, width - 60),
            font=(label_font, 9),
        )
        body_label.pack(fill="x", anchor="w", pady=(6, 0))

        root.update_idletasks()
        height = root.winfo_reqheight()
        height = max(84, min(260, height))
        screen_w = root.winfo_screenwidth()
        screen_h = root.winfo_screenheight()
        if "left" in position:
            x = margin
        else:
            x = max(margin, screen_w - width - margin)
        if "top" in position:
            y = margin
        else:
            y = max(margin, screen_h - height - margin - 16)
        root.geometry(f"{width}x{height}+{x}+{y}")

        def close(_: Any = None) -> None:
            try:
                root.destroy()
            except Exception:
                pass

        root.bind("<Button-1>", close)
        root.bind("<Escape>", close)
        root.after(duration_ms, close)
        root.after(100, lambda: root.lift())
        root.mainloop()
    except Exception as exc:
        _log(f"windows overlay crashed: {exc!r}", config)
    return 0


def _print_config(config: Mapping[str, Any]) -> None:
    safe = copy.deepcopy(dict(config))
    print(json.dumps(safe, indent=2, ensure_ascii=False))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", help="Path to codex-cli-notify.json.")
    parser.add_argument(
        "--test",
        choices=[
            "permission",
            "permission-unicode",
            "permission-ko",
            "stop",
            "stop-unicode",
            "stop-ko",
            "subagent",
            "subagent-unicode",
            "subagent-ko",
            "all",
        ],
        help="Send a local test notification.",
    )
    parser.add_argument("--dry-run", action="store_true", help="Print notification payload to stderr instead of showing a desktop alert.")
    parser.add_argument("--simulate-choice", choices=["allow", "deny", "defer"], help="Testing only: emit the PermissionRequest decision for this choice without showing UI.")
    parser.add_argument("--print-config", action="store_true", help="Print resolved notifier configuration.")
    parser.add_argument("--show-windows-overlay", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.show_windows_overlay:
        return _show_windows_overlay()

    if args.dry_run:
        os.environ["CODEX_CLI_NOTIFY_DRY_RUN"] = "1"
        os.environ["CODEX_LOCAL_NOTIFY_DRY_RUN"] = "1"

    config = _load_config(args.config)
    _configure_text_streams(_preferred_encoding(config))

    if args.print_config:
        _print_config(config)
        return 0

    payload: Dict[str, Any]
    try:
        if args.test == "all":
            for kind in ("permission", "stop", "subagent", "permission-unicode", "stop-unicode", "subagent-unicode"):
                payload = _test_payload(kind)
                if _payload_event_name(payload) == "PermissionRequest":
                    out = _permission_action_stdout_or_notify(payload, config, simulate_choice=args.simulate_choice)
                    if out:
                        print(out)
                else:
                    send_notification(payload, config)
                time.sleep(0.35)
            return 0
        payload = _test_payload(args.test) if args.test else _load_payload_from_stdin(config)
        if payload:
            if _env_bool_first("CODEX_CLI_NOTIFY_LOG_PAYLOAD", "CODEX_LOCAL_NOTIFY_LOG_PAYLOAD", False):
                _log("payload=" + json.dumps(payload, ensure_ascii=False, sort_keys=True), config)
            if _payload_event_name(payload) == "PermissionRequest":
                out = _permission_action_stdout_or_notify(payload, config, simulate_choice=args.simulate_choice)
                if out:
                    sys.stdout.write(out)
                return 0
            send_notification(payload, config)
        out = _success_stdout_for_hook(payload)
        if out:
            sys.stdout.write(out)
        return 0
    except Exception as exc:
        _log(f"notifier crashed: {exc!r}", config)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
