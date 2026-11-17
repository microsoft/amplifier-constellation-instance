"""The accepted stop must finish before a subsequent start or binary swap."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('worker', Path(__file__).with_name('worker.py'))
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class LaunchdStopTests(unittest.TestCase):
    def control(self):
        obj = worker.Worker.__new__(worker.Worker)
        obj.c = {'services': [{'kind': 'launchd', 'domain': 'gui/501', 'name': 'fixture', 'file': '/fixture.plist'}]}
        obj.state = {'service_snapshot': {'fixture': True}}
        return obj

    def test_delayed_exit_is_observed_before_start(self):
        status_codes = iter([0, 0, 113])
        calls = []
        def invoke(argv, **kwargs):
            calls.append(argv[1])
            return subprocess.CompletedProcess(argv, next(status_codes) if argv[1] == 'print' else 0, b'')
        with patch.object(worker.subprocess, 'run', side_effect=invoke), patch.object(worker.time, 'sleep'):
            obj = self.control()
            scope = {'services': ['fixture']}
            obj.control('stop', scope)
            obj.control('start', scope)
        self.assertEqual(calls, ['print', 'bootout', 'print', 'print', 'bootstrap'])

    def test_absent_service_needs_no_stop(self):
        with patch.object(worker.subprocess, 'run', return_value=subprocess.CompletedProcess([], 113, b'')) as run:
            self.control().control('stop', {'services': ['fixture']})
        self.assertEqual([c.args[0][1] for c in run.call_args_list], ['print'])

    def test_initial_read_error_is_not_absence(self):
        with patch.object(worker.subprocess, 'run', return_value=subprocess.CompletedProcess([], 5, b'')) as run:
            with self.assertRaisesRegex(RuntimeError, 'state_unknown'):
                self.control().control('stop', {'services': ['fixture']})
        self.assertEqual(run.call_count, 1)

    def test_read_error_after_stop_refuses_progress(self):
        results = [subprocess.CompletedProcess([], n, b'') for n in [0, 0, 5]]
        with patch.object(worker.subprocess, 'run', side_effect=results) as run:
            with self.assertRaisesRegex(RuntimeError, 'state_unknown'):
                self.control().control('stop', {'services': ['fixture']})
        self.assertEqual([c.args[0][1] for c in run.call_args_list], ['print', 'bootout', 'print'])

    def test_deadline_never_reissues_stop(self):
        clock = [0.0]
        def sleep(seconds): clock[0] += seconds
        with patch.object(worker.time, 'monotonic', side_effect=lambda: clock[0]), patch.object(worker.time, 'sleep', side_effect=sleep), patch.object(worker.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, b'')) as run:
            with self.assertRaisesRegex(TimeoutError, 'did_not_settle'):
                worker.stop_launchd('gui/501/fixture', timeout=.1)
        self.assertEqual(sum(c.args[0][1] == 'bootout' for c in run.call_args_list), 1)
        self.assertFalse(any(c.args[0][1] == 'bootstrap' for c in run.call_args_list))

    def test_failed_stop_is_not_reissued(self):
        with patch.object(worker.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 0, b''), subprocess.CompletedProcess([], 5, b'')]) as run:
            with self.assertRaisesRegex(RuntimeError, 'command_failed'):
                worker.stop_launchd('gui/501/fixture')
        self.assertEqual([c.args[0][1] for c in run.call_args_list], ['print', 'bootout'])


if __name__ == '__main__':
    unittest.main()
