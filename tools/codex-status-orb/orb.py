from __future__ import annotations

import ctypes
import math
import os
import subprocess
import sys
import threading
import time
import tkinter as tk
import winsound
from pathlib import Path
from typing import Any

import psutil
import pystray
from PIL import Image, ImageDraw
from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from mode import headless_requested
from status_common import (
    COLORS,
    CONFIG_PATH,
    DATA_ROOT,
    LOG_PATH,
    PRIORITY,
    aggregate_status,
    checkpoint,
    focus_terminal,
    find_turn_abort_after,
    find_task_complete_after,
    find_user_input_state_after,
    load_config,
    log,
    process_matches,
    read_sessions,
    remove_session,
    save_config,
    state_path,
    write_session,
    write_health,
)

ORB_SIZE = 64
CIRCLE_SIZE = 52
POLL_MS = 100
SOUND_BY_STATUS = {
    "blocked": "SystemHand",
}
ATTENTION_TONES = ((880, 140), (1175, 180))
DONE_BELL_PATH = Path(__file__).resolve().parent / "assets" / "service-bell.mp3"


class StatusOrb:
    def __init__(self, headless: bool = False) -> None:
        self.headless = headless
        try:
            ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        except Exception:
            try:
                ctypes.windll.shcore.SetProcessDpiAwareness(2)
            except Exception:
                pass
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateMutexW.restype = ctypes.c_void_p
        self.mutex = kernel32.CreateMutexW(None, False, "Local\\CodexStatusOrb.SingleInstance")
        if not self.mutex:
            raise ctypes.WinError(ctypes.get_last_error())
        if ctypes.get_last_error() == 183:  # ERROR_ALREADY_EXISTS
            raise SystemExit(0)
        self.config = load_config()
        self.sessions: list[dict[str, Any]] = []
        self.known_events: dict[str, int] = {}
        self.last_process_check = 0.0
        self.drag_origin: tuple[int, int, int, int] | None = None
        self.dragged = False
        self.phase = 0.0
        self.dirty = threading.Event()
        self.dirty.set()
        self.error_count = 0
        self.last_error = ""
        self.last_heartbeat = 0.0
        self.last_color_key: tuple[str, int] | None = None
        self.last_session_color_events: dict[str, int] = {}
        self.observer: Observer | None = None
        self.panel: tk.Toplevel | None = None
        self.root: tk.Tk | None = None
        self.tray: pystray.Icon | None = None
        if headless:  # same scan/sound/recovery/health loop, no window or tray
            self.initial_scan()
            self.start_watcher()
            checkpoint("orb_started", pid=os.getpid(), headless=True)
            return
        self.root = tk.Tk()
        self.root.title("Codex Status Orb")
        self.root.overrideredirect(True)
        self.root.attributes("-topmost", True)
        self.transparent = "#010203"
        self.root.configure(bg=self.transparent)
        try:
            self.root.wm_attributes("-transparentcolor", self.transparent)
        except tk.TclError:
            pass
        self.canvas = tk.Canvas(self.root, width=ORB_SIZE, height=ORB_SIZE, bg=self.transparent, highlightthickness=0)
        self.canvas.pack()
        self.canvas.bind("<ButtonPress-1>", self.on_press)
        self.canvas.bind("<B1-Motion>", self.on_drag)
        self.canvas.bind("<ButtonRelease-1>", self.on_release)
        self.canvas.bind("<Button-3>", self.popup_menu)
        self.root_menu = self.make_root_menu()
        self.place_window()
        if self.config.get("hidden"):
            self.root.withdraw()
        self.tray = pystray.Icon("codex-status-orb", self.make_icon("idle"), "Codex Status Orb", self.make_tray_menu())
        self.tray.run_detached()
        self.initial_scan()
        self.start_watcher()
        checkpoint("orb_started", pid=os.getpid())
        self.tick()

    def start_watcher(self) -> None:
        orb = self

        class SessionHandler(FileSystemEventHandler):
            def on_any_event(self, event) -> None:
                paths = (str(getattr(event, "src_path", "")), str(getattr(event, "dest_path", "")))
                if not event.is_directory and any(path.lower().endswith((".json", ".jsonl")) for path in paths):
                    orb.dirty.set()

        try:
            self.observer = Observer()
            handler = SessionHandler()
            self.observer.schedule(handler, str(DATA_ROOT / "sessions"), recursive=False)
            for codex_home in (Path.home() / ".codex", Path.home() / ".codex-cy", Path.home() / ".codex-feu"):
                transcript_root = codex_home / "sessions"
                if transcript_root.is_dir():
                    self.observer.schedule(handler, str(transcript_root), recursive=True)
            self.observer.start()
            checkpoint("watcher_started", backend=type(self.observer).__name__)
        except Exception as exc:
            self.observer = None
            self.record_error("watcher_start", exc)

    def place_window(self) -> None:
        position = self.config.get("position")
        if isinstance(position, list) and len(position) == 2:
            x, y = int(position[0]), int(position[1])
        else:
            self.root.update_idletasks()
            x = max(24, self.root.winfo_screenwidth() - ORB_SIZE - 24)
            y = 24
        screen_width = self.root.winfo_screenwidth()
        screen_height = self.root.winfo_screenheight()
        x = min(max(0, x), max(0, screen_width - ORB_SIZE))
        y = min(max(0, y), max(0, screen_height - ORB_SIZE))
        if self.config.get("position") != [x, y]:
            self.config["position"] = [x, y]
            save_config(self.config)
        self.root.geometry(f"{ORB_SIZE}x{ORB_SIZE}+{x}+{y}")

    def initial_scan(self) -> None:
        self.sessions = read_sessions()
        self.known_events = {str(s["session_id"]): int(s.get("event_ns", 0)) for s in self.sessions}

    def make_icon(self, status: str) -> Image.Image:
        image = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
        draw = ImageDraw.Draw(image)
        draw.ellipse((7, 7, 57, 57), fill=COLORS.get(status, COLORS["idle"]), outline=(255, 255, 255, 220), width=3)
        return image

    def make_tray_menu(self) -> pystray.Menu:
        return pystray.Menu(
            pystray.MenuItem("Show sessions", lambda _icon, _item: self.ui(self.show_panel), default=True),
            pystray.MenuItem("Hide / Show orb", lambda _icon, _item: self.ui(self.toggle_hidden)),
            pystray.MenuItem("Mute sounds", lambda _icon, _item: self.ui(self.toggle_mute), checked=lambda _: bool(self.config.get("muted"))),
            pystray.MenuItem("Test done bell", lambda _icon, _item: self.ui(lambda: self.play_status("ready"))),
            pystray.MenuItem("Reset position", lambda _icon, _item: self.ui(self.reset_position)),
            pystray.MenuItem("Open logs", lambda _icon, _item: self.ui(self.open_logs)),
            pystray.MenuItem("Run diagnostics", lambda _icon, _item: self.run_action("diagnostics")),
            pystray.MenuItem("Repair hooks", lambda _icon, _item: self.run_action("repair")),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Exit", lambda _icon, _item: self.ui(self.exit_app)),
        )

    def make_root_menu(self) -> tk.Menu:
        menu = tk.Menu(self.root, tearoff=False)
        menu.add_command(label="Show sessions", command=self.show_panel)
        menu.add_command(label="Hide orb", command=self.toggle_hidden)
        menu.add_command(label="Mute / Unmute", command=self.toggle_mute)
        menu.add_command(label="Test done bell", command=lambda: self.play_status("ready"))
        menu.add_command(label="Reset position", command=self.reset_position)
        menu.add_command(label="Open logs", command=self.open_logs)
        menu.add_command(label="Run diagnostics", command=lambda: self.run_action("diagnostics"))
        menu.add_command(label="Repair hooks", command=lambda: self.run_action("repair"))
        menu.add_separator()
        menu.add_command(label="Exit", command=self.exit_app)
        return menu

    def ui(self, callback) -> None:
        try:
            self.root.after(0, callback)
        except Exception:
            pass

    def popup_menu(self, event: tk.Event) -> None:
        self.root_menu.tk_popup(event.x_root, event.y_root)

    def on_press(self, event: tk.Event) -> None:
        self.drag_origin = (event.x_root, event.y_root, self.root.winfo_x(), self.root.winfo_y())
        self.dragged = False

    def on_drag(self, event: tk.Event) -> None:
        if not self.drag_origin:
            return
        sx, sy, wx, wy = self.drag_origin
        dx, dy = event.x_root - sx, event.y_root - sy
        if abs(dx) + abs(dy) > 3:
            self.dragged = True
        self.root.geometry(f"+{wx + dx}+{wy + dy}")

    def on_release(self, _event: tk.Event) -> None:
        if self.dragged:
            self.config["position"] = [self.root.winfo_x(), self.root.winfo_y()]
            save_config(self.config)
        else:
            self.show_panel()
        self.drag_origin = None

    def toggle_hidden(self) -> None:
        hidden = self.root.state() == "withdrawn"
        if hidden:
            self.root.deiconify()
            self.root.lift()
            self.config["hidden"] = False
        else:
            self.root.withdraw()
            self.config["hidden"] = True
        save_config(self.config)

    def toggle_mute(self) -> None:
        self.config["muted"] = not bool(self.config.get("muted"))
        save_config(self.config)
        self.tray.update_menu()

    def reset_position(self) -> None:
        self.config["position"] = None
        save_config(self.config)
        self.place_window()
        self.root.deiconify()

    def exit_app(self) -> None:
        try:
            checkpoint("orb_stopping")
            if self.observer:
                self.observer.stop()
                self.observer.join(timeout=2)
            self.tray.stop()
        finally:
            self.root.destroy()

    def open_logs(self) -> None:
        LOG_PATH.touch(exist_ok=True)
        os.startfile(LOG_PATH)

    def run_action(self, action: str) -> None:
        def worker() -> None:
            try:
                checkpoint("action_started", action=action)
                script = "manage_hooks.py" if action == "repair" else "doctor.py"
                command = [sys.executable, str(Path(__file__).with_name(script))]
                if action == "repair":
                    command.append("install")
                completed = subprocess.run(
                    command, capture_output=True, text=True, timeout=30, check=False,
                    creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)),
                )
                if completed.stdout.strip():
                    log(completed.stdout.strip())
                if completed.stderr.strip():
                    log(completed.stderr.strip())
                checkpoint("action_finished", action=action, exit_code=completed.returncode)
                if completed.returncode:
                    raise RuntimeError(f"{action} exited {completed.returncode}")
                winsound.MessageBeep(winsound.MB_OK)
            except Exception as exc:
                self.record_error(f"action_{action}", exc)
                winsound.MessageBeep(winsound.MB_ICONHAND)

        threading.Thread(target=worker, name=f"orb-{action}", daemon=True).start()

    def record_error(self, checkpoint_name: str, exc: Exception) -> None:
        self.error_count += 1
        self.last_error = f"{type(exc).__name__}: {exc}"
        log(f"checkpoint={checkpoint_name} error={self.last_error!r}")
        try:
            write_health("degraded", checkpoint_name, self.error_count, self.last_error)
        except Exception as health_exc:
            log(f"checkpoint=health_write_failed source={checkpoint_name!r} error={health_exc!r}")

    def sound_focus_context(self, session: dict[str, Any]) -> dict[str, Any]:
        foreground_pid = 0
        try:
            hwnd = ctypes.windll.user32.GetForegroundWindow()
            pid = ctypes.c_ulong()
            ctypes.windll.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            foreground_pid = int(pid.value)
        except Exception as exc:
            checkpoint("sound_focus_check_failed", error=repr(exc))
        target_pids = {int(session.get("terminal_pid", 0)), int(session.get("shell_pid", 0))} - {0}
        return {
            "focused": foreground_pid in target_pids,
            "foreground_pid": foreground_pid,
            "terminal_pid": int(session.get("terminal_pid", 0)),
        }

    def play_status(self, status: str, session: dict[str, Any]) -> None:
        focus = self.sound_focus_context(session)
        if session.get("herdr_managed"):
            checkpoint(
                "sound_decision", status=status, decision="suppressed",
                reason="herdr-native-authority", authority="herdr", **focus,
            )
            return
        if self.config.get("muted"):
            checkpoint("sound_decision", status=status, decision="suppressed", reason="muted", **focus)
            return
        try:
            if status == "ready" and self.play_done_bell():
                checkpoint("sound_decision", status=status, decision="played", sound="done-bell", **focus)
                return
            if status == "needs_input" and self.play_attention_ping():
                checkpoint("sound_decision", status=status, decision="played", sound="attention-ping", **focus)
                return
            if status not in SOUND_BY_STATUS and status != "ready":
                checkpoint("sound_decision", status=status, decision="suppressed", reason="no-sound-for-status", **focus)
                return
            fallback = SOUND_BY_STATUS.get(status, "SystemAsterisk")
            winsound.PlaySound(fallback, winsound.SND_ALIAS | winsound.SND_ASYNC | winsound.SND_NODEFAULT)
            checkpoint("sound_decision", status=status, decision="requested", sound=fallback, **focus)
        except Exception as exc:
            checkpoint("sound_decision", status=status, decision="failed", error=repr(exc), **focus)

    def play_attention_ping(self) -> bool:
        try:
            for frequency, duration_ms in ATTENTION_TONES:
                winsound.Beep(frequency, duration_ms)
            checkpoint("attention_ping_played", success=True, tone_count=len(ATTENTION_TONES))
            return True
        except Exception as exc:
            checkpoint("attention_ping_played", success=False, error=repr(exc))
            return False

    def play_done_bell(self) -> bool:
        if not DONE_BELL_PATH.is_file():
            checkpoint("done_bell_fallback", reason="asset-missing")
            return False
        try:
            winmm = ctypes.WinDLL("winmm")
            send = winmm.mciSendStringW
            send.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint, ctypes.c_void_p]
            send.restype = ctypes.c_uint
            send("close codex_done_bell", None, 0, None)
            opened = send(f'open "{DONE_BELL_PATH}" type mpegvideo alias codex_done_bell', None, 0, None)
            played = send("play codex_done_bell from 0", None, 0, None) if opened == 0 else opened
            checkpoint("done_bell_played", success=played == 0, mci_error=int(played))
            return played == 0
        except Exception as exc:
            log(f"checkpoint=done_bell_error error={exc!r}")
            return False

    def validate_processes(self, sessions: list[dict[str, Any]]) -> None:
        now = time.time()
        if now - self.last_process_check < 1.0:
            return
        self.last_process_check = now
        for session in sessions:
            pid = int(session.get("codex_pid", 0))
            if pid and process_matches(pid, session.get("codex_create_time")):
                continue
            status = str(session.get("status", "idle"))
            if status in {"running", "needs_input"}:
                blocked = dict(session)
                blocked.pop("_path", None)
                blocked.update(status="blocked", event="UnexpectedProcessExit", event_ns=time.time_ns(), updated_at=now)
                write_session(blocked)
            elif status == "blocked" and now - float(session.get("updated_at", now)) > 600:
                remove_session(str(session["session_id"]))
            elif status in {"idle", "ready"} and now - float(session.get("updated_at", now)) > 10:
                remove_session(str(session["session_id"]))

    def recover_aborted_turns(self, sessions: list[dict[str, Any]]) -> None:
        for session in sessions:
            if str(session.get("status")) not in {"running", "needs_input"}:
                continue
            aborted = find_turn_abort_after(session)
            if not aborted:
                continue
            detected_ns = time.time_ns()
            completed_at = float(aborted.get("completed_at", time.time()))
            recovered = dict(session)
            recovered.pop("_path", None)
            recovered.update(
                status="ready",
                event="TurnAborted",
                event_ns=detected_ns,
                updated_at=completed_at,
                hook_received_ns=int(completed_at * 1_000_000_000),
                state_written_ns=detected_ns,
                abort_reason=str(aborted.get("reason", "interrupted")),
            )
            write_session(recovered)
            checkpoint(
                "turn_abort_recovered",
                session=session.get("session_id", ""),
                reason=aborted.get("reason", "interrupted"),
                abort_to_detect_ms=round((detected_ns / 1_000_000_000 - completed_at) * 1000, 3),
            )

    def recover_missing_stop_hooks(self, sessions: list[dict[str, Any]]) -> None:
        for session in sessions:
            if str(session.get("status")) not in {"running", "needs_input"}:
                continue
            completed = find_task_complete_after(session)
            if not completed:
                continue
            detected_ns = time.time_ns()
            completed_at = float(completed.get("completed_at", time.time()))
            recovered = dict(session)
            recovered.pop("_path", None)
            recovered.update(
                status="ready",
                event="StopFallback",
                event_ns=detected_ns,
                updated_at=completed_at,
                hook_received_ns=int(completed_at * 1_000_000_000),
                state_written_ns=detected_ns,
                recovery_reason="Stop hook missing after task_complete; hook launch may have failed (for example, Windows access denied)",
            )
            write_session(recovered)
            checkpoint(
                "stop_hook_missing_recovered",
                session=session.get("session_id", ""),
                turn=completed.get("turn_id", ""),
                suspected_error="hook-launch-failed-or-missing",
                action="marked-ready-and-play-sound",
                task_complete_to_detect_ms=round((detected_ns / 1_000_000_000 - completed_at) * 1000, 3),
            )

    def recover_user_input_states(self, sessions: list[dict[str, Any]]) -> None:
        for session in sessions:
            status = str(session.get("status", "idle"))
            event = str(session.get("event", ""))
            input_state = find_user_input_state_after(session)
            if not input_state:
                continue
            requested_at = float(input_state.get("requested_at", 0))
            if input_state.get("pending") and status == "running" and requested_at > float(session.get("updated_at", 0)):
                detected_ns = time.time_ns()
                updated = dict(session)
                updated.pop("_path", None)
                updated.update(status="needs_input", event="UserInputRequested", event_ns=detected_ns,
                               updated_at=requested_at, hook_received_ns=int(requested_at * 1_000_000_000),
                               state_written_ns=detected_ns, input_call_id=str(input_state.get("call_id", "")),
                               input_kind=str(input_state.get("input_kind") or "general"))
                write_session(updated)
                checkpoint("user_input_detected", session=session.get("session_id", ""),
                           call_id=input_state.get("call_id", ""), action="marked-needs-input-and-play-sound",
                           request_to_detect_ms=round((detected_ns / 1_000_000_000 - requested_at) * 1000, 3))
            elif not input_state.get("pending") and status == "needs_input" and event == "UserInputRequested":
                detected_ns = time.time_ns()
                resolved_at = float(input_state.get("resolved_at", time.time()))
                updated = dict(session)
                updated.pop("_path", None)
                updated.update(status="running", event="UserInputResolved", event_ns=detected_ns,
                               updated_at=resolved_at, hook_received_ns=int(resolved_at * 1_000_000_000),
                               state_written_ns=detected_ns, input_kind="")
                write_session(updated)
                checkpoint("user_input_resolved", session=session.get("session_id", ""),
                           call_id=input_state.get("call_id", ""), action="marked-running")

    def discover_codex_processes(self, sessions: list[dict[str, Any]]) -> None:
        represented = {int(item.get("codex_pid", 0)) for item in sessions if int(item.get("codex_pid", 0))}
        real_pids = {
            int(item.get("codex_pid", 0))
            for item in sessions
            if int(item.get("codex_pid", 0)) and not str(item.get("session_id", "")).startswith("process-")
        }
        for session in sessions:
            sid = str(session.get("session_id", ""))
            if sid.startswith("process-") and int(session.get("codex_pid", 0)) in real_pids:
                remove_session(sid)
        for proc in psutil.process_iter(["pid", "name", "create_time", "cwd"]):
            try:
                if str(proc.info.get("name", "")).lower() != "codex.exe" or proc.pid in represented:
                    continue
                shell_pid = 0
                terminal_pid = 0
                terminal_kind = "unknown"
                for parent in proc.parents():
                    name = parent.name().lower()
                    if not shell_pid and name in {"cmd.exe", "powershell.exe", "pwsh.exe"}:
                        shell_pid = parent.pid
                    if name in {"windowsterminal.exe", "openconsole.exe", "conhost.exe"}:
                        terminal_pid = parent.pid
                        terminal_kind = name.removesuffix(".exe")
                        break
                sid = f"process-{proc.pid}"
                write_session({
                    "session_id": sid,
                    "event": "ProcessDiscovered",
                    "status": "idle",
                    "cwd": str(proc.info.get("cwd") or "Codex CLI"),
                    "event_ns": time.time_ns(),
                    "updated_at": time.time(),
                    "codex_pid": proc.pid,
                    "codex_create_time": float(proc.info.get("create_time") or proc.create_time()),
                    "shell_pid": shell_pid,
                    "terminal_pid": terminal_pid or shell_pid,
                    "terminal_kind": terminal_kind,
                    "wt_session": "",
                    "discovered": True,
                })
                checkpoint("process_discovered", pid=proc.pid, session=sid)
            except (psutil.AccessDenied, psutil.NoSuchProcess, psutil.ZombieProcess):
                continue

    def scan(self) -> None:
        sessions = read_sessions()
        self.recover_aborted_turns(sessions)
        sessions = read_sessions()
        self.recover_user_input_states(sessions)
        sessions = read_sessions()
        self.recover_missing_stop_hooks(sessions)
        sessions = read_sessions()
        self.discover_codex_processes(sessions)
        sessions = read_sessions()
        self.validate_processes(sessions)
        sessions = read_sessions()
        for session in sessions:
            sid = str(session["session_id"])
            event_ns = int(session.get("event_ns", 0))
            old = self.known_events.get(sid)
            if old is None or event_ns > old:
                detected_ns = time.time_ns()
                state_written_ns = int(session.get("state_written_ns", event_ns))
                checkpoint(
                    "state_detected",
                    event=session.get("event", ""),
                    session=sid,
                    state_to_detect_ms=round((detected_ns - state_written_ns) / 1_000_000, 3),
                )
                self.play_status(str(session.get("status", "idle")), session)
            self.known_events[sid] = event_ns
        active_ids = {str(s["session_id"]) for s in sessions}
        self.known_events = {key: value for key, value in self.known_events.items() if key in active_ids}
        self.sessions = sessions
        self.dirty.clear()

    def draw(self) -> None:
        status = aggregate_status(self.sessions)
        applied_ns = time.time_ns()
        for session in self.sessions:
            sid = str(session.get("session_id", ""))
            event_ns = int(session.get("event_ns", 0))
            if self.last_session_color_events.get(sid) == event_ns:
                continue
            if session.get("state_written_ns") and session.get("hook_received_ns"):
                checkpoint(
                    "session_color_applied",
                    color=session.get("status", "idle"),
                    event=session.get("event", ""),
                    session=sid,
                    state_to_color_ms=round((applied_ns - int(session["state_written_ns"])) / 1_000_000, 3),
                    hook_to_color_ms=round((applied_ns - int(session["hook_received_ns"])) / 1_000_000, 3),
                )
            self.last_session_color_events[sid] = event_ns
        active_session_ids = {str(item.get("session_id", "")) for item in self.sessions}
        self.last_session_color_events = {
            sid: event for sid, event in self.last_session_color_events.items() if sid in active_session_ids
        }
        self.phase += 0.22
        pulse = 2.5 * (0.5 + 0.5 * math.sin(self.phase)) if status == "running" else 0
        margin = (ORB_SIZE - CIRCLE_SIZE + pulse) / 2
        end = ORB_SIZE - margin
        self.canvas.delete("all")
        self.canvas.create_oval(margin, margin, end, end, fill=COLORS[status], outline="#FFFFFF", width=3)
        count = len(self.sessions)
        if count > 1:
            self.canvas.create_oval(42, 2, 62, 22, fill="#111827", outline="#FFFFFF", width=1)
            self.canvas.create_text(52, 12, text=str(min(count, 99)), fill="#FFFFFF", font=("Segoe UI", 8, "bold"))
        self.tray.icon = self.make_icon(status)
        self.tray.title = f"Codex: {status.replace('_', ' ')} ({count} session{'s' if count != 1 else ''})"
        newest = max(
            self.sessions,
            key=lambda item: (PRIORITY.get(str(item.get("status", "idle")), 0), int(item.get("event_ns", 0))),
            default=None,
        )
        color_key = (status, int(newest.get("event_ns", 0)) if newest else 0)
        if color_key != self.last_color_key:
            if newest and newest.get("state_written_ns") and newest.get("hook_received_ns"):
                state_written_ns = int(newest["state_written_ns"])
                hook_received_ns = int(newest["hook_received_ns"])
                checkpoint(
                    "color_applied",
                    color=status,
                    event=newest.get("event", ""),
                    session=newest.get("session_id", ""),
                    state_to_color_ms=round((applied_ns - state_written_ns) / 1_000_000, 3),
                    hook_to_color_ms=round((applied_ns - hook_received_ns) / 1_000_000, 3),
                )
            self.last_color_key = color_key

    def show_panel(self) -> None:
        if self.panel and self.panel.winfo_exists():
            self.panel.destroy()
            self.panel = None
            return
        panel = tk.Toplevel(self.root)
        self.panel = panel
        panel.title("Codex sessions")
        panel.attributes("-topmost", True)
        panel.configure(bg="#111827")
        panel.resizable(False, False)
        tk.Label(panel, text="Codex CLI sessions", bg="#111827", fg="#FFFFFF", font=("Segoe UI", 12, "bold"), padx=14, pady=10).pack(anchor="w")
        if not self.sessions:
            tk.Label(panel, text="No active sessions", bg="#111827", fg="#94A3B8", padx=14, pady=16).pack(anchor="w")
        for session in sorted(self.sessions, key=lambda s: PRIORITY.get(str(s.get("status")), 0), reverse=True):
            self.add_session_row(panel, session)
        panel.update_idletasks()
        width = 380
        height = min(440, max(90, panel.winfo_reqheight()))
        x = max(0, self.root.winfo_x() - width + ORB_SIZE)
        y = self.root.winfo_y() + ORB_SIZE + 4
        panel.geometry(f"{width}x{height}+{x}+{y}")
        panel.protocol("WM_DELETE_WINDOW", lambda: (panel.destroy(), setattr(self, "panel", None)))

    def add_session_row(self, panel: tk.Toplevel, session: dict[str, Any]) -> None:
        status = str(session.get("status", "idle"))
        cwd = Path(str(session.get("cwd") or "Unknown"))
        name = cwd.name or str(cwd)
        input_label = {
            "technology_selection": " • Technology choice needed",
            "technology_status": " • Test status needed",
        }.get(str(session.get("input_kind") or ""), "")
        detail = f"{status.replace('_', ' ').title()}{input_label}  •  {str(session.get('session_id'))[:10]}"
        frame = tk.Frame(panel, bg="#1F2937", cursor="hand2", padx=10, pady=8)
        frame.pack(fill="x", padx=10, pady=3)
        dot = tk.Label(frame, text="●", fg=COLORS[status], bg="#1F2937", font=("Segoe UI", 13))
        dot.pack(side="left", padx=(0, 8))
        text_frame = tk.Frame(frame, bg="#1F2937")
        text_frame.pack(side="left", fill="x", expand=True)
        title = tk.Label(text_frame, text=name, fg="#FFFFFF", bg="#1F2937", font=("Segoe UI", 10, "bold"), anchor="w")
        title.pack(fill="x")
        subtitle = tk.Label(text_frame, text=detail, fg="#CBD5E1", bg="#1F2937", font=("Segoe UI", 8), anchor="w")
        subtitle.pack(fill="x")
        for widget in (frame, dot, text_frame, title, subtitle):
            widget.bind("<Button-1>", lambda _event, value=session: self.focus(value))

    def focus(self, session: dict[str, Any]) -> None:
        focused = focus_terminal(session)
        checkpoint("session_clicked", session=session.get("session_id", ""), focused=focused)
        if not focused:
            self.root.bell()
        if self.panel:
            self.panel.destroy()
            self.panel = None

    def tick(self) -> None:
        try:
            now = time.time()
            if self.dirty.is_set() or now - self.last_process_check >= 1.0:
                self.scan()
            if not self.headless:
                self.draw()
            if now - self.last_heartbeat >= 1.0:
                write_health("healthy", "ui_tick", self.error_count, self.last_error)
                self.last_heartbeat = now
        except Exception as exc:
            self.record_error("ui_tick", exc)
        finally:
            if self.root is not None:
                self.root.after(POLL_MS, self.tick)

    def run(self) -> None:
        if self.root is not None:
            self.root.mainloop()
            return
        while True:
            self.tick()
            self.dirty.wait(POLL_MS / 1000)


if __name__ == "__main__":
    try:
        StatusOrb(headless=headless_requested(sys.argv, os.environ)).run()
    except SystemExit:
        pass
    except Exception as exc:
        log(f"fatal-error {exc!r}")
