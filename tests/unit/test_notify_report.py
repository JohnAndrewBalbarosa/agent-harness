import unittest
from datetime import datetime, timedelta, timezone

from core.notify import report as nr

T0 = datetime(2026, 9, 28, 0, 0, 0, tzinfo=timezone.utc)


def at(seconds):
    return T0 + timedelta(seconds=seconds)


def iso(seconds):
    return at(seconds).isoformat().replace("+00:00", "Z")


def user(content, seconds, meta=False):
    entry = {"type": "user", "timestamp": iso(seconds), "message": {"content": content}}
    if meta:
        entry["isMeta"] = True
    return entry


def assistant(seconds, stop_reason=None):
    return {"type": "assistant", "timestamp": iso(seconds), "message": {"stop_reason": stop_reason}}


class ClassifyTests(unittest.TestCase):
    def test_classifies_user_entries(self):
        self.assertEqual(nr.classify(user("fix the bug", 0)), "prompt")
        self.assertEqual(nr.classify(user("<task-notification>\n<task-id>x", 0)), "task-notification")
        self.assertEqual(nr.classify(user("<command-name>/model</command-name>", 0)), "command")
        self.assertEqual(nr.classify(user("<local-command-stdout>ok", 0)), "command")
        self.assertIsNone(nr.classify(user("reminder", 0, meta=True)))
        self.assertIsNone(nr.classify(user([{"type": "tool_result"}], 0)))
        self.assertEqual(nr.classify(assistant(0)), "assistant")


class PrematureTests(unittest.TestCase):
    def timeline(self, *entries):
        return nr.timeline(list(entries))

    def test_toast_after_turn_really_ended_is_not_premature(self):
        tl = self.timeline(user("go", 0), assistant(10, "end_turn"), user("next", 100))
        self.assertIsNone(nr.premature_reason(at(12), tl))

    def test_toast_while_assistant_keeps_working_is_premature(self):
        tl = self.timeline(user("go", 0), assistant(10), assistant(40, "end_turn"))
        self.assertIn("kept working", nr.premature_reason(at(12), tl))

    def test_toast_before_background_task_finished_is_premature(self):
        tl = self.timeline(user("go", 0), assistant(10, "end_turn"),
                           user("<task-notification>\n<task-id>a1", 90), assistant(95, "end_turn"))
        self.assertIn("background task", nr.premature_reason(at(12), tl))

    def test_activity_after_next_real_prompt_is_ignored(self):
        tl = self.timeline(user("go", 0), assistant(10, "end_turn"), user("new prompt", 50), assistant(60))
        self.assertIsNone(nr.premature_reason(at(12), tl))

    def test_small_grace_period_is_not_premature(self):
        tl = self.timeline(user("go", 0), assistant(10), assistant(13, "end_turn"))
        self.assertIsNone(nr.premature_reason(at(12), tl))


class SilentReasonTests(unittest.TestCase):
    def record(self, **overrides):
        base = {"event": "turn.stop", "decision": "toast", "toastResult": "windows:handed",
                "windowsNotificationState": "accepts-notifications", "herdr": {"inHerdr": True, "focused": False}}
        base.update(overrides)
        return base

    def test_clean_toast_has_no_reasons(self):
        self.assertEqual(nr.silent_reasons(self.record()), [])

    def test_skipped_toast_reports_decision(self):
        self.assertIn("no toast: skip:rate-limited", nr.silent_reasons(self.record(decision="skip", reason="rate-limited")))

    def test_windows_suppression_is_reported(self):
        reasons = nr.silent_reasons(self.record(windowsNotificationState="busy (fullscreen app)"))
        self.assertTrue(any("Windows suppressing" in reason for reason in reasons))

    def test_focused_herdr_pane_means_no_herdr_sound(self):
        reasons = nr.silent_reasons(self.record(herdr={"inHerdr": True, "focused": True}))
        self.assertTrue(any("Herdr plays no sound" in reason for reason in reasons))

    def test_toast_error_is_reported(self):
        self.assertIn("toast failed: windows:error(OSError)", nr.silent_reasons(self.record(toastResult="windows:error(OSError)")))

    def test_user_prompt_submit_is_never_flagged(self):
        self.assertEqual(nr.silent_reasons(self.record(event="prompt.submit", decision="mark")), [])


class ToastSourceTests(unittest.TestCase):
    def test_labels_each_toast_source(self):
        herdr_app = "NotifyIconGeneratedAumid_9185652406082797978"
        self.assertEqual(nr.toast_source(herdr_app, "Claude Code / Tapos na ang prompt!"), "agent-harness via Herdr")
        self.assertEqual(nr.toast_source(herdr_app, "Codex (cy) / Tapos na ang prompt!"), "agent-harness via Herdr")
        self.assertEqual(nr.toast_source(herdr_app, "claude needs attention / Projects · 2"), "Herdr screen detection")
        self.assertEqual(nr.toast_source("{1AC14E77}\\WindowsPowerShell\\v1.0\\powershell.exe", "Claude Code / x"), "agent-harness (Windows)")


class ToastTextTests(unittest.TestCase):
    def test_extracts_text_nodes_from_toast_xml(self):
        xml = '<toast><visual><binding><text id="1">claude needs attention</text><text id="2">Projects · 2</text></binding></visual></toast>'
        self.assertEqual(nr.toast_text(xml), "claude needs attention / Projects · 2")


if __name__ == "__main__":
    unittest.main()
