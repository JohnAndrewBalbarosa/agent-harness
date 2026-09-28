"""Record visible console windows without capturing titles or command lines."""

from __future__ import annotations

import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import time


SHARED = Path.home() / ".codex-shared"
LOG_PATH = SHARED / "var" / "logs" / "console-window.lifecycle.jsonl"
MAX_LOG_BYTES = 1_048_576
BACKUP_COUNT = 2
CONSOLE_PROCESSES = frozenset(
    {"cmd.exe", "conhost.exe", "openconsole.exe", "powershell.exe", "pwsh.exe"}
)
EVENT_SYSTEM_FOREGROUND = 0x0003
EVENT_OBJECT_SHOW = 0x8002
WINEVENT_OUTOFCONTEXT = 0x0000
WINEVENT_SKIPOWNPROCESS = 0x0002
OBJID_WINDOW = 0
TH32CS_SNAPPROCESS = 0x00000002
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
ERROR_ALREADY_EXISTS = 183


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.c_void_p),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", wintypes.LONG),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", wintypes.WCHAR * 260),
    ]


def profile_from_path(path: str) -> str:
    normalized = path.lower().replace("/", "\\")
    for profile in ("cy", "feu"):
        if f"\\.codex-{profile}\\" in normalized:
            return profile
    if "\\.codex\\" in normalized:
        return "personal"
    return "unknown"


def is_console_window(process_name: str, class_name: str) -> bool:
    return process_name.lower() in CONSOLE_PROCESSES or "console" in class_name.lower()


def _rotate_log() -> None:
    if not LOG_PATH.exists() or LOG_PATH.stat().st_size < MAX_LOG_BYTES:
        return
    for index in range(BACKUP_COUNT, 1, -1):
        older = LOG_PATH.with_name(f"{LOG_PATH.name}.{index - 1}")
        newer = LOG_PATH.with_name(f"{LOG_PATH.name}.{index}")
        if older.exists():
            os.replace(older, newer)
    os.replace(LOG_PATH, LOG_PATH.with_name(f"{LOG_PATH.name}.1"))


def emit(operation: str, outcome: str, *, severity: str = "info", **details: object) -> None:
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
    _rotate_log()
    record = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "severity": severity,
        "component": "console-window-monitor",
        "operation": operation,
        "outcome": outcome,
        "details": details,
    }
    with LOG_PATH.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, separators=(",", ":"), ensure_ascii=False) + "\n")


def configure_win32():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    user = ctypes.WinDLL("user32", use_last_error=True)
    kernel.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    kernel.CreateMutexW.restype = wintypes.HANDLE
    kernel.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    kernel.Process32FirstW.restype = wintypes.BOOL
    kernel.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESSENTRY32W)]
    kernel.Process32NextW.restype = wintypes.BOOL
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetWindowThreadProcessId.restype = wintypes.DWORD
    user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user.GetClassNameW.restype = ctypes.c_int
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    user.IsWindowVisible.restype = wintypes.BOOL
    user.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
    user.GetAncestor.restype = wintypes.HWND
    user.SetWinEventHook.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE, ctypes.c_void_p,
                                     wintypes.DWORD, wintypes.DWORD, wintypes.DWORD]
    user.SetWinEventHook.restype = wintypes.HANDLE
    user.UnhookWinEvent.argtypes = [wintypes.HANDLE]
    user.UnhookWinEvent.restype = wintypes.BOOL
    user.GetMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT]
    user.GetMessageW.restype = ctypes.c_int
    return kernel, user


def processes(kernel) -> dict[int, tuple[str, int]]:
    snapshot = kernel.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snapshot == ctypes.c_void_p(-1).value:
        return {}
    entries: dict[int, tuple[str, int]] = {}
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(entry)
        if kernel.Process32FirstW(snapshot, ctypes.byref(entry)):
            while True:
                entries[int(entry.th32ProcessID)] = (str(entry.szExeFile), int(entry.th32ParentProcessID))
                if not kernel.Process32NextW(snapshot, ctypes.byref(entry)):
                    break
    finally:
        kernel.CloseHandle(snapshot)
    return entries


def executable_path(kernel, pid: int) -> str:
    handle = kernel.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return ""
    try:
        buffer = ctypes.create_unicode_buffer(32768)
        length = wintypes.DWORD(len(buffer))
        if kernel.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(length)):
            return buffer.value
        return ""
    finally:
        kernel.CloseHandle(handle)


def ancestry(pid: int, entries: dict[int, tuple[str, int]], path_for_pid) -> tuple[list[dict[str, object]], str, int | None]:
    chain: list[dict[str, object]] = []
    profile = "unknown"
    codex_pid = None
    seen: set[int] = set()
    while pid and pid not in seen and len(chain) < 12:
        seen.add(pid)
        name, parent = entries.get(pid, ("unknown", 0))
        chain.append({"pid": pid, "process": name, "parentPid": parent})
        if name.lower() == "codex.exe":
            codex_pid = pid
            profile = profile_from_path(path_for_pid(pid))
            break
        pid = parent
    return chain, profile, codex_pid


def main() -> int:
    kernel, user = configure_win32()
    mutex = kernel.CreateMutexW(None, False, "Local\\CodexSharedConsoleWindowMonitor")
    if not mutex:
        emit("monitor.start", "failed", severity="error", reason="mutex_unavailable", errorCode=ctypes.get_last_error())
        return 2
    if ctypes.get_last_error() == ERROR_ALREADY_EXISTS:
        kernel.CloseHandle(mutex)
        return 0

    callback_type = ctypes.WINFUNCTYPE(None, wintypes.HANDLE, wintypes.DWORD, wintypes.HWND,
                                      wintypes.LONG, wintypes.LONG, wintypes.DWORD, wintypes.DWORD)
    recently_seen: dict[tuple[int, int], float] = {}

    def on_window(_hook, event, hwnd, object_id, child_id, _thread, _millis):
        try:
            if not hwnd or object_id != OBJID_WINDOW or child_id or not user.IsWindowVisible(hwnd):
                return
            if user.GetAncestor(hwnd, 2) != hwnd:
                return
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            entries = processes(kernel)
            name = entries.get(pid.value, ("unknown", 0))[0]
            window_class = ctypes.create_unicode_buffer(128)
            user.GetClassNameW(hwnd, window_class, len(window_class))
            if not is_console_window(name, window_class.value):
                return
            key = (int(hwnd), pid.value)
            now = time.monotonic()
            if event == EVENT_SYSTEM_FOREGROUND and key in recently_seen:
                return
            if now - recently_seen.get(key, -10.0) < 2.0:
                return
            recently_seen[key] = now
            if len(recently_seen) > 128:
                recently_seen.clear()
            chain, profile, codex_pid = ancestry(pid.value, entries, lambda process_id: executable_path(kernel, process_id))
            if codex_pid is None:
                return
            emit("console.window.visible", "observed", eventId=event, pid=pid.value,
                 parentPid=entries.get(pid.value, ("", 0))[1], process=name, windowClass=window_class.value[:80],
                 profile=profile, codexAncestorPid=codex_pid, ancestry=chain)
        except Exception as error:
            emit("console.window.inspect", "failed", severity="error", errorType=type(error).__name__)

    callback = callback_type(on_window)
    hooks = []
    try:
        for event in (EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND):
            hook = user.SetWinEventHook(event, event, None, callback, 0, 0,
                                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
            if not hook:
                emit("monitor.hook", "failed", severity="error", eventId=event, errorCode=ctypes.get_last_error())
                return 2
            hooks.append(hook)
        emit("monitor.start", "succeeded", processId=os.getpid(), windowMode="hidden")
        message = wintypes.MSG()
        while user.GetMessageW(ctypes.byref(message), None, 0, 0) > 0:
            pass
        return 0
    finally:
        for hook in hooks:
            user.UnhookWinEvent(hook)
        kernel.CloseHandle(mutex)
        emit("monitor.stop", "completed", processId=os.getpid())


if __name__ == "__main__":
    raise SystemExit(main())
