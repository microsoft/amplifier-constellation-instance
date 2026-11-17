"""Real launchd settlement on a disposable macOS runner-owned job only.

No installation paths, credentials, network, models, or agent processes are used.
The child delays SIGTERM so an acknowledged bootout is not mistaken for exit.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

WORKER_PATH = Path(__file__).with_name("worker.py")
spec = importlib.util.spec_from_file_location("launchd_live_worker", WORKER_PATH)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)

CHILD = '''import json, os, signal, sys, time
from pathlib import Path
root = Path(sys.argv[1])
generation = int((root / "generation").read_text())
def record(event):
    with (root / "child.jsonl").open("a") as output:
        output.write(json.dumps({"event": event, "generation": generation, "pid": os.getpid(), "monotonic": time.monotonic()}) + "\\n")
        output.flush()
        os.fsync(output.fileno())
def stop(signum, frame):
    record("term_received")
    time.sleep(1.25)
    record("exiting")
    raise SystemExit(0)
signal.signal(signal.SIGTERM, stop)
record("ready")
time.sleep(25)
record("watchdog_expired")
'''


@unittest.skipUnless(sys.platform == "darwin", "requires real macOS launchd")
class LaunchdStopLiveTests(unittest.TestCase):
    def test_control_waits_for_delayed_exit_before_one_restart(self):
        root = Path(tempfile.mkdtemp(prefix="constellation-launchd-stop-"))
        label = "io.constellation.fixture.launchd-stop." + str(uuid.uuid4())
        domain = "gui/" + str(os.getuid())
        service = domain + "/" + label
        plist = root / "job.plist"
        allowed = (["launchctl", "print", service], ["launchctl", "bootout", service],
                   ["launchctl", "bootstrap", domain, str(plist)])
        real_run = subprocess.run
        started = time.monotonic()
        receipt = {"worker_sha256": hashlib.sha256(WORKER_PATH.read_bytes()).hexdigest(),
                   "service": service, "commands": [], "generations": [], "cleanup_errors": []}
        admitted = False
        pids = []

        def command(argv, **kwargs):
            self.assertIn(argv, allowed, "Fixture may control only its unique job")
            entry = {"argv": argv, "start_seconds": time.monotonic() - started}
            receipt["commands"].append(entry)
            try:
                result = real_run(argv, **kwargs)
                entry.update(returncode=result.returncode,
                             stdout=(result.stdout or b"").decode(errors="replace"),
                             stderr=(result.stderr or b"").decode(errors="replace"))
                return result
            except BaseException as error:
                entry["error_type"] = type(error).__name__
                raise
            finally:
                entry["end_seconds"] = time.monotonic() - started

        def present():
            result = command(["launchctl", "print", service], capture_output=True, timeout=5)
            self.assertIn(result.returncode, (0, 113), "Unknown launchd state is not absence")
            return result.returncode == 0

        def events():
            path = root / "child.jsonl"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        def alive(pid):
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False

        def ready(generation):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                rows = [row for row in events() if row["event"] == "ready" and row["generation"] == generation]
                if rows:
                    self.assertEqual(len(rows), 1)
                    pid = rows[0]["pid"]
                    self.assertIsInstance(pid, int)
                    self.assertGreater(pid, 1)
                    self.assertTrue(alive(pid))
                    pids.append(pid)
                    return pid
                time.sleep(.05)
            self.fail("Owned fixture did not start within its readiness bound")

        # Only the service-control seam is initialized; no Worker constructor,
        # registration, journal, network connection, or update cycle is invoked.
        control = worker.Worker.__new__(worker.Worker)
        control.c = {"services": [{"kind": "launchd", "domain": domain, "name": label, "file": str(plist)}]}
        control.state = {"service_snapshot": {label: True}}
        scope = {"services": [label]}
        with patch.object(worker.subprocess, "run", side_effect=command):
            try:
                self.assertFalse(present(), "Random fixture label must initially be absent")
                (root / "child.py").write_text(CHILD)
                plist.write_bytes(plistlib.dumps({
                    "Label": label, "ProgramArguments": [sys.executable, "-I", "-B", str(root / "child.py"), str(root)],
                    "RunAtLoad": True, "KeepAlive": False, "ExitTimeOut": 5,
                    "StandardOutPath": str(root / "stdout.log"), "StandardErrorPath": str(root / "stderr.log")}))
                for generation in (1, 2):
                    (root / "generation").write_text(str(generation))
                    # Cleanup authority follows confirmed absence and this
                    # invocation's own bootstrap intent, including lost replies.
                    admitted = True
                    control.control("start", scope)
                    pid = ready(generation)
                    self.assertTrue(present())
                    before = time.monotonic()
                    control.control("stop", scope)
                    returned = time.monotonic()
                    self.assertFalse(present(), "Stop returned while the job was still registered")
                    self.assertFalse(alive(pid), "Stop returned before the owned process exited")
                    rows = [row for row in events() if row["generation"] == generation]
                    self.assertEqual([row["event"] for row in rows], ["ready", "term_received", "exiting"])
                    self.assertGreaterEqual(rows[2]["monotonic"] - rows[1]["monotonic"], 1.2)
                    self.assertLessEqual(rows[2]["monotonic"], returned)
                    receipt["generations"].append({"generation": generation, "pid": pid,
                        "stop_elapsed_seconds": returned - before, "absent": True, "process_exited": True})
                self.assertNotEqual(pids[0], pids[1])
                self.assertEqual(sum(c["argv"][1] == "bootstrap" for c in receipt["commands"]), 2)
                self.assertEqual(sum(c["argv"][1] == "bootout" for c in receipt["commands"]), 2)
                receipt["exercise_passed"] = True
            finally:
                # Never stop a label observed before this invocation owned it.
                # Failure cleanup is separate from the exercise, never a retry.
                absent = False
                try:
                    if admitted:
                        if present():
                            receipt["cleanup_stop_required"] = True
                            worker.stop_launchd(service, timeout=15)
                        absent = not present()
                    else:
                        absent = True  # No bootstrap was attempted by us.
                except BaseException as error:
                    receipt["cleanup_errors"].append(type(error).__name__ + ": " + str(error))
                try:
                    receipt["child_events"] = events()
                    owned_pids = set(pids + [row["pid"] for row in receipt["child_events"]])
                    exited = all(not alive(pid) for pid in owned_pids)
                    receipt["child_stdout"] = (root / "stdout.log").read_text() if (root / "stdout.log").exists() else ""
                    receipt["child_stderr"] = (root / "stderr.log").read_text() if (root / "stderr.log").exists() else ""
                    receipt.update(cleanup_job_absent=absent, cleanup_processes_exited=exited)
                    if absent and exited:
                        shutil.rmtree(root)
                    receipt["cleanup_directory_removed"] = not root.exists()
                except BaseException as error:
                    receipt["cleanup_errors"].append(type(error).__name__ + ": " + str(error))
                receipt["elapsed_seconds"] = time.monotonic() - started
                receipt["passed"] = bool(receipt.get("exercise_passed") and receipt.get("cleanup_job_absent")
                    and receipt.get("cleanup_processes_exited") and receipt.get("cleanup_directory_removed")
                    and not receipt["cleanup_errors"])
                if root.exists():
                    receipt["retained_failure_directory"] = str(root)
                print(json.dumps(receipt), flush=True)
        self.assertTrue(receipt["passed"], "Disposable job or directory cleanup did not settle")


if __name__ == "__main__":
    unittest.main()
