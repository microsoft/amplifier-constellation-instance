"""Real process/lock enrollment races; OS managers and network stay synthetic."""
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from unittest.mock import patch

ROOT = Path(os.environ.get("CONSTELLATION_ENROLLMENT_SOURCE", Path(__file__).parent)).resolve()


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


w = load(ROOT / "worker.py", "race_worker")
m = load(ROOT / "install.py", "race_installer")


def wait_file(path, seconds=10):
    deadline = time.monotonic() + seconds
    while not path.exists():
        if time.monotonic() >= deadline:
            raise AssertionError("Disposable fixture did not reach its barrier: " + path.name)
        time.sleep(.01)


def process_fixture(mode, folder, worker_path):
    root = Path(folder)
    module = load(Path(worker_path), "child_worker")
    if mode == "hold":
        with module.single_worker(root / "owner"):
            (root / "ready").touch()
            wait_file(root / "release")
        return
    real_sleep = time.sleep
    module.time.sleep = lambda seconds: real_sleep(min(seconds, .02))
    module.ssl.create_default_context = lambda **kwargs: None
    calls = []
    def tick(worker):
        calls.append(time.monotonic_ns())
        (root / (mode + "-ticks.json")).write_text(json.dumps(calls))
        (root / (mode + "-ready")).touch()
        if mode == "running" and len(calls) == 1:
            wait_file(root / "cycle-release")
        if mode == "successor":
            assert not worker.state.get("enrollment")
            assert worker.c == module.load_registration(worker.registration)
    module.Worker.tick = tick
    module.Worker.refresh_helper = lambda self: None
    try:
        module.run_worker(root / "owner/registration.json", once=mode == "successor")
    except module.EnrollmentYield:
        (root / (mode + "-yielded")).touch()


class EnrollmentRaceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home"
        self.state = self.root / "owner"
        (self.home / "bin").mkdir(parents=True)
        self.state.mkdir()
        self.binary = self.home / "bin/agent-instance"
        self.binary.write_bytes(b"not executed")
        (self.home / "bin/agent-instance.exe").write_bytes(b"not executed")
        self.identity = str(uuid.uuid4())
        self.config = {"config": {"instance_id": self.identity, "backend": None, "owner": "fixture-owner"}}
        (self.home / "config.json").write_text(json.dumps(self.config))
        (self.home / "install.json").write_text(json.dumps({"service": {"kind": "launchd", "name": "fixture-instance"}}))
        self.registration = {"schema": 1, "instance_id": self.identity, "home": str(self.home),
            "state": str(self.state), "components": [{"id": "instance", "kind": "instance", "binary": str(self.binary)}],
            "services": [], "slack_homes": [], "participants": [],
            "service_connections": {"schema": 1, "services": [], "connections": []}}
        self.original = {"state": "idle", "components": [], "last_check": 123,
            "transaction": None, "active_request": None, "completed_requests": ["retained-complete"],
            "unknown_receipts": [{"id": "historical-unknown", "settled": False}]}
        w.write(self.state / "registration.json", self.registration)
        w.write(self.state / "state.json", self.original)
        (self.state / "worker.py").write_bytes(b"original helper fixture")
        self.processes = []
        self.addCleanup(self.cleanup_processes)
        self.manager_calls = []

    def cleanup_processes(self):
        (self.root / "release").touch()
        (self.root / "cycle-release").touch()
        for child in self.processes:
            if child.poll() is None:
                child.terminate()
            try:
                child.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.communicate(timeout=5)
                self.fail("Owned fixture did not terminate")

    def child(self, mode, worker=None):
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--fixture", mode,
                   str(self.root), str(worker or ROOT / "worker.py")]
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.processes.append(child)
        return child

    def snapshot(self):
        return {path.relative_to(self.root): path.read_bytes() for path in self.root.rglob("*")
                if path.is_file() and path.name not in ("worker.lock", "enrollment.lock")}

    @contextlib.contextmanager
    def fake_manager(self, system="Linux", action=None):
        def command(*argv):
            self.manager_calls.append(tuple(argv))
            if action:
                action(argv)
            return ""
        with contextlib.ExitStack() as mocks:
            mocks.enter_context(patch.object(m.platform, "system", return_value=system))
            if system == "Darwin":
                mocks.enter_context(patch.object(m.os, "getuid", return_value=12345, create=True))
            mocks.enter_context(patch.object(m.Path, "home", return_value=self.root))
            mocks.enter_context(patch.object(m, "run", side_effect=command))
            mocks.enter_context(patch.object(m.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)))
            mocks.enter_context(patch.object(m, "ENROLLMENT_TIMEOUT", 2, create=True))
            if hasattr(m, "load_worker"):
                mocks.enter_context(patch.object(m, "load_worker", return_value=w))
            yield

    def enroll(self):
        return m.enroll(self.home, self.state, [], ROOT / "worker.py")

    def test_noncooperating_live_worker_refuses_before_any_mutation_or_restart(self):
        child = self.child("hold")
        wait_file(self.root / "ready")
        before = self.snapshot()
        reads = []
        original_read = w.read
        def observed(path):
            reads.append(Path(path))
            return original_read(path)
        with self.fake_manager(), patch.object(m, "ENROLLMENT_TIMEOUT", .15, create=True), \
             patch.object(w, "read", side_effect=observed):
            with self.assertRaisesRegex(ValueError, "busy_before_enrollment"):
                self.enroll()
        self.assertIsNone(child.poll())
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.manager_calls, [])
        self.assertNotIn(self.state / "state.json", reads)

    def test_duplicate_startup_never_touches_admission_or_yields_live_helper(self):
        child = self.child("hold")
        wait_file(self.root / "ready")
        with patch.object(w, "enrollment_guard", wraps=w.enrollment_guard) as admission:
            with self.assertRaises(OSError):
                w.run_worker(self.state / "registration.json", once=True)
            admission.assert_not_called()
        self.assertIsNone(child.poll())

    def test_inflight_report_keeps_idle_helper_ownership(self):
        with patch.object(w.ssl, "create_default_context"):
            worker = w.Worker(self.state / "registration.json")
        entered, release = threading.Event(), threading.Event()
        def report():
            entered.set()
            if not release.wait(5):
                raise AssertionError("Owned report fixture was not released")
        thread = threading.Thread(target=report)
        worker._reporting_threads = [thread]
        thread.start()
        try:
            self.assertTrue(entered.wait(2))
            with w.enrollment_guard(self.state), w.single_worker(self.state), \
                 patch.object(w.time, "sleep", side_effect=KeyboardInterrupt("bounded observation")), \
                 patch.object(worker, "tick") as tick, patch.object(worker, "save") as save:
                with self.assertRaises(KeyboardInterrupt):
                    worker.serve()
                tick.assert_not_called()
                save.assert_not_called()
                self.assertTrue(thread.is_alive())
                with self.assertRaises(w.LockBusy):
                    with w.single_worker(self.state):
                        self.fail("Original lifetime ownership must remain held")
        finally:
            release.set()
            thread.join(5)
        self.assertFalse(thread.is_alive())
        self.assertTrue(worker.enrollment_ready())

    def test_all_unsettled_states_refuse_without_mutation_or_manager_action(self):
        cases = [
            {"state": state} for state in ["stopping", "swapping", "verifying", "rolling_back", "waiting_for_idle", "recovery_required"]
        ] + [{"transaction": {"id": "unknown", "phase": "restoring"}}, {"active_request": "pending"}]
        for values in cases:
            with self.subTest(values=values):
                w.write(self.state / "state.json", {**self.original, **values})
                before = self.snapshot()
                with self.fake_manager(), self.assertRaisesRegex(ValueError, "current update or connection"):
                    self.enroll()
                self.assertEqual(self.snapshot(), before)
                self.assertEqual(self.manager_calls, [])

    def test_unsettled_connection_blocks_but_retained_terminal_report_does_not(self):
        for filename, value in [
            ("connection-operation.json", {"command": {"request_id": "original"}}),
            ("connection-report.json", {"connections": [{"settled": False, "state": "unknown"}]}),
            ("connection-report.json", {"connections": [], "add_result": {"settled": False}}),
        ]:
            with self.subTest(filename=filename, value=value):
                path = self.state / filename
                w.write(path, value)
                before = self.snapshot()
                with self.fake_manager(), self.assertRaisesRegex(ValueError, "current update or connection"):
                    self.enroll()
                self.assertEqual(self.snapshot(), before)
                path.unlink()
        terminal = {"connections": [{"settled": True, "state": "connected", "request_id": "retained"}]}
        w.write(self.state / "connection-report.json", terminal)
        with self.fake_manager():
            self.enroll()
        self.assertEqual(w.read(self.state / "connection-report.json"), terminal)
        saved = w.read(self.state / "state.json")
        self.assertEqual(saved["completed_requests"], self.original["completed_requests"])
        self.assertEqual(saved["unknown_receipts"], self.original["unknown_receipts"])

    def test_cycle_handoff_fences_restart_and_new_process_admission(self):
        old = self.child("running")
        wait_file(self.root / "running-ready")
        calls = []
        errors = []
        successors = []
        def manager(argv):
            # The old helper has released its lifetime singleton. Its fixture
            # reports that exit on a separate pipe/file before interpreter
            # teardown, which may finish just after the installer acquired it.
            self.assertEqual(old.wait(timeout=2), 0)
            if not successors:
                successors.append(self.child("successor", self.state / "worker.py"))
                time.sleep(.15)
            self.assertFalse((self.root / "successor-ready").exists(), "Restarted process must wait outside singleton")
            calls.append(argv)
        def installer():
            try:
                with self.fake_manager(action=manager):
                    self.enroll()
            except BaseException as error:
                errors.append(error)
        attempt = threading.Thread(target=installer)
        attempt.start()
        time.sleep(.1)
        self.assertFalse(self.manager_calls)
        self.assertIsNone(old.poll())
        (self.root / "cycle-release").touch()
        attempt.join(5)
        self.assertFalse(attempt.is_alive())
        self.assertEqual(errors, [])
        self.assertTrue(calls)
        wait_file(self.root / "successor-ready")
        for process in [old, *successors]:
            stdout, stderr = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 0, stdout + stderr)
        self.assertTrue((self.root / "running-yielded").exists())
        self.assertFalse(w.read(self.state / "state.json").get("enrollment"))

    def test_worker_keeps_unsettled_singleton_when_admission_is_closed(self):
        for values in [{"transaction": {"id": "original"}}, {"active_request": "pending"}, {"state": "recovery_required"}]:
            with self.subTest(values=values):
                w.write(self.state / "state.json", {**self.original, **values})
                with patch.object(w.ssl, "create_default_context"):
                    worker = w.Worker(self.state / "registration.json")
                sleeps = []
                def stop_after_observation(seconds):
                    sleeps.append(seconds)
                    raise KeyboardInterrupt("end bounded observation")
                with w.enrollment_guard(self.state), w.single_worker(self.state), \
                     patch.object(w.time, "sleep", side_effect=stop_after_observation), \
                     patch.object(worker, "tick") as tick, patch.object(worker, "save") as save:
                    with self.assertRaises(KeyboardInterrupt):
                        worker.serve()
                    tick.assert_not_called()
                    save.assert_not_called()
                self.assertEqual(sleeps, [5])

    def test_pending_publication_blocks_work_refresh_and_changed_intent(self):
        original_publish = m.publish_helper
        calls = []
        def fail_after_marker(path, content, mode=0o600):
            if path.name != "worker.py":
                calls.append(path.name)
                raise OSError("publication interrupted")
            return original_publish(path, content, mode)
        with self.fake_manager(), patch.object(m, "publish_helper", side_effect=fail_after_marker):
            with self.assertRaisesRegex(OSError, "publication interrupted"):
                self.enroll()
        saved = w.read(self.state / "state.json")
        self.assertTrue(saved["enrollment"])
        self.assertEqual(saved["unknown_receipts"], self.original["unknown_receipts"])
        self.assertEqual(self.manager_calls, [])
        with patch.object(w.ssl, "create_default_context"):
            worker = w.Worker(self.state / "registration.json")
        with patch.object(worker, "report", return_value={}) as report, \
             patch.object(worker, "connection_tick") as connection, \
             patch.object(worker, "recover_transaction") as recover:
            worker.tick()
            report.assert_called_once()
            connection.assert_not_called()
            recover.assert_not_called()
        before = self.snapshot()
        self.config["config"]["owner"] = "changed-owner"
        (self.home / "config.json").write_text(json.dumps(self.config))
        unchanged = self.snapshot()
        with self.fake_manager(), self.assertRaisesRegex(ValueError, "unfinished_enrollment_inputs_changed"):
            self.enroll()
        self.assertEqual(self.snapshot(), unchanged)
        self.config["config"]["owner"] = "fixture-owner"
        (self.home / "config.json").write_text(json.dumps(self.config))
        with self.fake_manager():
            self.enroll()
        current = w.read(self.state / "state.json")
        self.assertNotIn("enrollment", current)
        self.assertEqual(current["unknown_receipts"], self.original["unknown_receipts"])

    def test_manager_failure_or_unknown_retains_marker_until_explicit_same_input_recovery(self):
        cases = [("Linux", phase) for phase in ("definition", "daemon-reload", "enable", "restart")]
        cases += [("Darwin", phase) for phase in ("bootout", "definition", "bootstrap")]
        cases += [("Windows", "powershell.exe")]
        for system, phase in cases:
            with self.subTest(system=system, phase=phase):
                w.write(self.state / "registration.json", self.registration)
                w.write(self.state / "state.json", self.original)
                self.manager_calls.clear()
                original_text, original_bytes = Path.write_text, Path.write_bytes
                fired = []
                def fail():
                    fired.append(phase)
                    raise subprocess.TimeoutExpired("fixture manager outcome unknown", 1)
                def manager(argv):
                    if phase in argv:
                        fail()
                def write_text(path, *args, **kwargs):
                    result = original_text(path, *args, **kwargs)
                    if phase == "definition" and path.suffix == ".service":
                        fail()
                    return result
                def write_bytes(path, *args, **kwargs):
                    result = original_bytes(path, *args, **kwargs)
                    if phase == "definition" and path.suffix == ".plist":
                        fail()
                    return result
                def query(argv, **kwargs):
                    if phase == "bootout" and "bootout" in argv:
                        fail()
                    return subprocess.CompletedProcess(argv, 0)
                with self.fake_manager(system, manager), \
                     patch.object(m.subprocess, "run", side_effect=query), \
                     patch.object(Path, "write_text", new=write_text), \
                     patch.object(Path, "write_bytes", new=write_bytes):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        self.enroll()
                self.assertEqual(fired, [phase], "Unknown manager operation must not retry")
                state = w.read(self.state / "state.json")
                self.assertTrue(state.get("enrollment"))
                self.assertEqual(state["unknown_receipts"], self.original["unknown_receipts"])
                with patch.object(w.ssl, "create_default_context"):
                    worker = w.Worker(self.state / "registration.json")
                with patch.object(worker, "report", return_value={}), \
                     patch.object(worker, "connection_tick") as connection:
                    worker.tick()
                    connection.assert_not_called()
                with self.fake_manager(system):
                    self.enroll()
                self.assertNotIn("enrollment", w.read(self.state / "state.json"))

    def test_failure_after_each_publication_boundary_preserves_exact_recovery(self):
        # Capture the real publication list, then independently recreate a fresh
        # fixture and fail immediately after each durable write/replace.
        for system in ["Linux", "Darwin", "Windows"]:
            with self.subTest(system=system):
                original_write, original_publish = w.write, m.publish_helper
                events = []
                def write(path, value):
                    original_write(path, value)
                    events.append(("json", Path(path).name))
                def publish(path, content, mode=0o600):
                    original_publish(path, content, mode)
                    events.append(("helper", path.name))
                with self.fake_manager(system), patch.object(w, "write", side_effect=write), patch.object(m, "publish_helper", side_effect=publish):
                    self.enroll()
                self.assertIn(("json", "registration.json"), events)
                self.assertEqual(events[0], ("helper", "worker.py"))
                # Every mutation is replayed from the same owner inputs below;
                # the final manager call remains outside partial publication.
                for boundary in range(len(events)):
                    w.write(self.state / "registration.json", self.registration)
                    w.write(self.state / "state.json", self.original)
                    self.manager_calls.clear()
                    count = [0]
                    def stop():
                        count[0] += 1
                        if count[0] == boundary + 1:
                            raise KeyboardInterrupt("installer stopped")
                    def stop_write(path, value):
                        original_write(path, value)
                        stop()
                    def stop_publish(path, content, mode=0o600):
                        original_publish(path, content, mode)
                        stop()
                    with self.fake_manager(system), patch.object(w, "write", side_effect=stop_write), patch.object(m, "publish_helper", side_effect=stop_publish):
                        with self.assertRaises(KeyboardInterrupt):
                            self.enroll()
                    if boundary < len(events) - 1:
                        self.assertEqual(self.manager_calls, [])
                    else:
                        self.assertTrue(self.manager_calls)
                    state = w.read(self.state / "state.json")
                    self.assertEqual(state["unknown_receipts"], self.original["unknown_receipts"])
                    if 1 <= boundary < len(events) - 1:
                        self.assertIn("enrollment", state)
                    with self.fake_manager(system):
                        self.enroll()
                    state = w.read(self.state / "state.json")
                    self.assertNotIn("enrollment", state)
                    self.assertEqual(state["unknown_receipts"], self.original["unknown_receipts"])


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--fixture":
        process_fixture(*sys.argv[2:])
    else:
        unittest.main()
