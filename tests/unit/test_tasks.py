import json
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from core.events import make_event, to_json
from core.tasks import state as tasks

ROOT = Path(__file__).resolve().parents[2]
NOW = datetime(2026, 9, 28, 12, 0, 0, tzinfo=timezone.utc)


def subagent(event_type, agent_id, agent="claude"):
    native = "SubagentStart" if event_type == "subagent.start" else "SubagentStop"
    return make_event(event_type, agent, "default", {"session_id": "s1"}, native,
                      {"subagent": {"id": agent_id, "type": "general-purpose"}})


class ApplyTests(unittest.TestCase):
    def test_start_then_stop_without_mutation(self):
        empty = {}
        started = tasks.apply_event(empty, subagent("subagent.start", "a1"), NOW)
        self.assertEqual(list(started), ["a1"])
        self.assertEqual(empty, {})
        self.assertEqual(tasks.apply_event(started, subagent("subagent.stop", "a1"), NOW), {})
        self.assertEqual(list(started), ["a1"])

    def test_other_events_and_missing_ids_leave_state_unchanged(self):
        state = {"a1": NOW.isoformat()}
        stop = make_event("turn.stop", "claude", "default", {"session_id": "s1"}, "Stop")
        self.assertEqual(tasks.apply_event(state, stop, NOW), state)
        self.assertEqual(tasks.apply_event(state, subagent("subagent.start", ""), NOW), state)


class OutstandingTests(unittest.TestCase):
    def test_fresh_agents_only(self):
        state = {"old": "2026-09-28T08:00:00+00:00", "new": "2026-09-28T11:59:00+00:00"}
        self.assertEqual(tasks.fresh_agents(state, NOW), ["new"])


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def test_record_and_read_back(self):
        tasks.record(self.store, subagent("subagent.start", "a1"), NOW)
        self.assertEqual(tasks.outstanding(self.store, "claude", "s1", None, NOW), ["a1"])
        tasks.record(self.store, subagent("subagent.stop", "a1"), NOW)
        self.assertEqual(tasks.outstanding(self.store, "claude", "s1", None, NOW), [])

    def test_sessions_and_agents_are_isolated(self):
        tasks.record(self.store, subagent("subagent.start", "a1", agent="claude"), NOW)
        self.assertEqual(tasks.outstanding(self.store, "codex", "s1", None, NOW), [])

    def test_hostile_session_ids_stay_inside_store(self):
        event = make_event("subagent.start", "claude", "default", {"session_id": "..\\..\\evil"}, "SubagentStart",
                           {"subagent": {"id": "a1", "type": "x"}})
        tasks.record(self.store, event, NOW)
        self.assertEqual([p.parent for p in self.store.rglob("*.json")], [self.store])

    def test_records_survive_concurrent_readers(self):
        import threading
        stop, errors = threading.Event(), []

        def reader():
            while not stop.is_set():
                tasks.outstanding(self.store, "claude", "s1", None, NOW)

        threads = [threading.Thread(target=reader) for _ in range(4)]
        for t in threads:
            t.start()
        try:
            for n in range(60):
                try:
                    tasks.record(self.store, subagent("subagent.start", f"a{n}"), NOW)
                    tasks.record(self.store, subagent("subagent.stop", f"a{n}"), NOW)
                except OSError as error:
                    errors.append(error)
        finally:
            stop.set()
            for t in threads:
                t.join()
        self.assertEqual(errors, [])
        self.assertEqual(tasks.outstanding(self.store, "claude", "s1", None, NOW), [])

    def test_concurrent_state_updates(self):
        writer = ("import sys; sys.path.insert(0, sys.argv[1]);"
                  "from pathlib import Path; from datetime import datetime, timezone;"
                  "from core.events import from_json; from core.tasks import state;"
                  "state.record(Path(sys.argv[2]), from_json(sys.argv[3]), datetime.now(timezone.utc))")
        procs = [subprocess.Popen([sys.executable, "-c", writer, str(ROOT), str(self.store),
                                   to_json(subagent("subagent.start", f"agent{n}"))]) for n in range(10)]
        for p in procs:
            self.assertEqual(p.wait(timeout=60), 0)
        now = datetime.now(timezone.utc)
        self.assertEqual(sorted(tasks.outstanding(self.store, "claude", "s1", None, now)), sorted(f"agent{n}" for n in range(10)))


class ConsumerTests(unittest.TestCase):
    def test_consumer_records_event_from_stdin(self):
        with tempfile.TemporaryDirectory() as store:
            done = subprocess.run([sys.executable, "-m", "core.tasks.consumer"], input=to_json(subagent("subagent.start", "zz")).encode(),
                                  cwd=str(ROOT), env={**__import__("os").environ, "AH_TASKS_DIR": store, "PYTHONPATH": str(ROOT)}, timeout=60)
            self.assertEqual(done.returncode, 0)
            state = json.loads(next(Path(store).glob("*.json")).read_text(encoding="utf-8"))
            self.assertIn("zz", state)

    def test_consumer_ignores_garbage(self):
        done = subprocess.run([sys.executable, "-m", "core.tasks.consumer"], input=b"not json", cwd=str(ROOT), timeout=60,
                              env={**__import__("os").environ, "PYTHONPATH": str(ROOT)})
        self.assertEqual(done.returncode, 0)


if __name__ == "__main__":
    unittest.main()
