import unittest

from core.events import make_event
from core.notify.decide import NotifyState, decide

RATE = (5, 60)
NOW = 1_000_000


def ev(event_type, agent="claude", **data):
    native = {"prompt.submit": "UserPromptSubmit", "turn.stop": "Stop", "attention.needed": "Notification"}[event_type]
    return make_event(event_type, agent, "default", {"session_id": "s"}, native, data)


def attention(kind, message="Claude needs your permission"):
    return ev("attention.needed", attention={"kind": kind, "message": message})


class DecideTests(unittest.TestCase):
    def test_prompt_marks_pending(self):
        decision, state = decide(ev("prompt.submit", prompt="x"), NotifyState(False, ()), [], NOW, RATE)
        self.assertEqual((decision.action, state.pending), ("mark", True))

    def test_stop_with_pending_and_no_background_toasts_done(self):
        decision, state = decide(ev("turn.stop"), NotifyState(True, ()), [], NOW, RATE)
        self.assertEqual((decision.action, decision.sound, state.pending), ("toast", "done", False))
        self.assertEqual(decision.message, "Tapos na ang prompt!")
        self.assertEqual(state.recent_toasts, (NOW,))

    def test_stop_without_pending_is_skipped(self):
        decision, _ = decide(ev("turn.stop"), NotifyState(False, ()), [], NOW, RATE)
        self.assertEqual((decision.action, decision.reason), ("skip", "no-pending-prompt"))

    def test_stop_hook_active_is_skipped_and_keeps_pending(self):
        decision, state = decide(ev("turn.stop", stop_hook_active=True), NotifyState(True, ()), [], NOW, RATE)
        self.assertEqual((decision.action, decision.reason, state.pending), ("skip", "stop-hook-active", True))

    def test_stop_with_background_tasks_defers_and_keeps_pending(self):
        decision, state = decide(ev("turn.stop"), NotifyState(True, ()), ["agent-1"], NOW, RATE)
        self.assertEqual((decision.action, decision.reason, state.pending), ("defer", "background-tasks", True))

    def test_permission_always_toasts_request(self):
        decision, _ = decide(attention("permission"), NotifyState(False, ()), ["agent-1"], NOW, RATE)
        self.assertEqual((decision.action, decision.sound, decision.message), ("toast", "request", "Claude needs your permission"))

    def test_idle_defers_while_background_tasks_run(self):
        decision, _ = decide(attention("idle", "waiting"), NotifyState(False, ()), ["shell-1"], NOW, RATE)
        self.assertEqual((decision.action, decision.reason), ("defer", "background-tasks"))

    def test_idle_without_background_toasts(self):
        decision, _ = decide(attention("idle", "waiting"), NotifyState(False, ()), [], NOW, RATE)
        self.assertEqual((decision.action, decision.sound), ("toast", "request"))

    def test_rate_limit_skips_and_old_toasts_expire(self):
        busy = NotifyState(True, tuple(NOW - i for i in range(5)))
        decision, state = decide(ev("turn.stop"), busy, [], NOW, RATE)
        self.assertEqual((decision.action, decision.reason, state.pending), ("skip", "rate-limited", False))
        old = NotifyState(True, tuple(NOW - 100 - i for i in range(5)))
        self.assertEqual(decide(ev("turn.stop"), old, [], NOW, RATE)[0].action, "toast")

    def test_future_timestamps_do_not_block(self):
        skewed = NotifyState(True, tuple(NOW + 28_800 + i for i in range(5)))  # old local-time clock bug
        self.assertEqual(decide(ev("turn.stop"), skewed, [], NOW, RATE)[0].action, "toast")

    def test_attention_message_default(self):
        decision, _ = decide(attention("permission", message=None), NotifyState(False, ()), [], NOW, RATE)
        self.assertEqual(decision.message, "Kailangan ng input mo!")

    def test_state_is_not_mutated(self):
        state = NotifyState(True, (NOW - 1,))
        decide(ev("turn.stop"), state, [], NOW, RATE)
        self.assertEqual(state, NotifyState(True, (NOW - 1,)))


if __name__ == "__main__":
    unittest.main()
