import unittest

from core.events import make_event
from core.router.consumers import consumers_for


def names(event_type, native="Stop", env=None):
    event = make_event(event_type, "claude", "default", {"session_id": "s"}, native)
    return {c.name for c in consumers_for(event, env or {})}


class RegistryTests(unittest.TestCase):
    def test_turn_stop_routes_to_obs_notify_and_usage(self):
        self.assertTrue({"obs", "notify", "usage-fallback"} <= names("turn.stop"))
        self.assertFalse({"tasks", "context"} & names("turn.stop"))

    def test_subagent_events_route_to_tasks_only_among_harness_consumers(self):
        self.assertIn("tasks", names("subagent.start", "SubagentStart"))
        self.assertFalse({"obs", "notify", "context"} & names("subagent.start", "SubagentStart"))

    def test_session_start_routes_to_context(self):
        self.assertIn("context", names("session.start", "SessionStart"))

    def test_herdr_bridge_only_inside_herdr(self):
        self.assertNotIn("herdr-bridge", names("turn.stop"))
        self.assertNotIn("herdr-bridge", names("turn.stop", env={"HERDR_ENV": "1"}))
        self.assertIn("herdr-bridge", names("turn.stop", env={"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1"}))


if __name__ == "__main__":
    unittest.main()
