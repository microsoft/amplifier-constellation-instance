"""Exercise scheduled and requested work against the same enrolled installation."""
import copy
import json
import sys
import unittest
import uuid
from unittest.mock import patch

import test_worker as fixtures

w = fixtures.w


class ComponentScheduleTests(unittest.TestCase):
    def setUp(self):
        fixtures.UpdateTests.setUp(self)
        self.game = self.home / 'game-version'
        self.game.write_text('first')
        self.session = self.home / 'session'
        self.session.write_bytes(b'original session and paused work')
        self.scope = {'instance': False, 'services': [], 'slack_homes': []}
        self.registration['components'].append({
            'id': 'game', 'provider': 'command', 'protocol': 2,
            'commands': {action: [sys.executable, 'game', action]
                         for action in ('status', 'plan', 'apply', 'verify', 'rollback')},
            'restart_scope': self.scope,
        })
        self.registration['participants'] = [{
            'id': 'game', 'protocol': 1, 'components': ['game'],
            'commands': {action: [sys.executable, 'participant', action]
                         for action in w.PARTICIPANT_ACTIONS},
        }]
        self.registration['when_idle_components'] = ['game']
        w.write(self.path, self.registration)
        self.worker = fixtures.UpdateTests.worker(self)
        self.request = None
        self.worker.report = lambda **kwargs: {
            'policy': self.worker.policy, 'request': self.request, 'active_prompts': 0,
        }
        self.calls = []
        self.worker.provider_call = self.provider
        self.worker.participant_call = self.participant
        self.instance_stat = self.binary.stat()
        self.worker.control = lambda action, scope: self.calls.append((action, scope))
        self.candidate_lookup = self.enter_patch('candidate', self.offer_instance)
        self.enter_patch('run', self.version)
        self.sleep = patch.object(w.time, 'sleep')
        self.sleep.start()
        self.addCleanup(self.sleep.stop)

    def enter_patch(self, name, handler):
        patcher = patch.object(w, name, side_effect=handler)
        mock = patcher.start()
        self.addCleanup(patcher.stop)
        return mock

    def version(self, argv, **kwargs):
        if argv == [str(self.binary), '--version']:
            return b'agent-instance 0.1.0'
        self.assertEqual(argv, [sys.executable, 'game', 'status'])
        return json.dumps({'version': self.game.read_text()}).encode()

    def provider(self, component, action, folder):
        if component['id'] == 'other':
            self.assertEqual(action, 'plan')
            return b'{"available": true, "version": "manual-only-upgrade"}'
        self.assertEqual(component['id'], 'game')
        self.calls.append(action)
        if action == 'plan':
            return json.dumps({'available': self.game.read_text() != 'next', 'version': 'next'}).encode()
        receipt = w.read(folder/'game.provider.json')
        self.assertEqual((receipt['id'], receipt['component']), (folder.name, 'game'))
        if action == 'apply':
            self.game.write_text('next')
        elif action == 'rollback':
            self.game.write_text('first')
        return b'{}'

    def participant(self, participant, action, transaction, outcome):
        envelope = w.read(w.Path(transaction['directory'])/'game.json')
        self.assertEqual(envelope['plan']['components'], ['game'])
        if action == 'prepare' and action not in self.calls:
            self.assertEqual(self.game.read_text(), 'first')
            self.calls.append(action)
            return False
        self.calls.append(action)
        return True

    def offer_instance(self, component, channel, folder):
        self.assertEqual(component['id'], 'instance')
        binary = folder/'instance.candidate'
        binary.write_bytes(b'new-binary')
        return {'id': 'instance', 'available': True, 'installed_sha256': w.digest(self.binary),
                'sha256': w.digest(binary), 'version': 'old', 'target_version': 'new'}

    def test_scheduled_install_waits_for_idle_and_rolls_back_only_selected_component(self):
        self.worker.tick()
        self.assertEqual(self.worker.state['state'], 'succeeded')
        plan = w.read(self.state/'plans'/self.worker.state['plan_id']/'plan.json')
        self.assertEqual(plan['when_idle_components'], ['game'])
        self.assertEqual(plan['components'][0]['reason'], 'outside_idle_selection')
        self.assertTrue(plan['components'][0]['available_at_check'])
        self.assertEqual(plan['restart_scope'], self.scope)
        self.assertEqual(self.calls[:4], ['plan', 'plan', 'prepare', 'prepare'])
        discovery = w.read(self.state/'plans'/plan['discovery_plan_id']/'plan.json')
        self.assertNotEqual(discovery['id'], plan['id'])
        self.assertEqual([part['id'] for part in discovery['components'] if part['available']],
                         ['instance', 'game'])
        calls = list(self.calls)
        with self.assertRaises(ValueError):
            self.worker.apply(discovery, self.worker.policy)
        with self.assertRaisesRegex(ValueError, 'no_installed_version'):
            self.worker.apply(discovery, self.worker.policy, rollback=True)
        self.assertEqual(self.calls, calls)
        self.assertEqual(self.game.read_text(), 'next')
        self.worker.apply(plan, self.worker.policy, rollback=True)
        self.assertEqual(self.worker.state['state'], 'rolled_back')
        self.assertEqual(self.game.read_text(), 'first')
        self.assertEqual(self.binary.read_bytes(), b'old-binary')
        self.assertEqual(self.binary.stat().st_mtime_ns, self.instance_stat.st_mtime_ns)
        self.assertEqual(self.session.read_bytes(), b'original session and paused work')
        self.assertEqual(self.candidate_lookup.call_count, 1)

    def test_manual_check_stays_full_and_cannot_be_auto_applied(self):
        self.request = {'pending': True, 'id': str(uuid.uuid4()), 'actor': 'owner',
                        'action': 'check', 'policy_revision': self.worker.policy['revision']}
        self.worker.tick()
        self.request = None
        plan_id = self.worker.state['plan_id']
        plan = w.read(self.state/'plans'/plan_id/'plan.json')
        self.assertNotIn('when_idle_components', plan)
        self.assertEqual([p['id'] for p in plan['components'] if p['available']], ['instance', 'game'])
        self.worker.tick()
        self.assertEqual(self.worker.state['plan_id'], plan_id)
        self.assertEqual(self.calls, ['plan'])
        self.assertEqual(self.game.read_text(), 'first')
        self.assertEqual(self.binary.read_bytes(), b'old-binary')
        self.assertEqual(self.candidate_lookup.call_count, 1)

        self.worker.participant_call = lambda participant, action, tx, outcome: True
        self.request = {'pending': True, 'id': str(uuid.uuid4()), 'actor': 'owner',
                        'action': 'apply', 'plan_id': plan_id,
                        'policy_revision': self.worker.policy['revision']}
        self.worker.tick()
        self.assertEqual(self.worker.state['state'], 'succeeded')
        self.assertEqual(self.game.read_text(), 'next')
        self.assertEqual(self.binary.read_bytes(), b'new-binary')

    def test_deferred_command_provider_is_discovered_but_never_installed(self):
        other = copy.deepcopy(self.registration['components'][1])
        other['id'] = 'other'
        self.worker.c['components'].append(other)
        self.worker.tick()
        self.assertEqual(self.worker.state['state'], 'succeeded')
        folder = self.state/'plans'/self.worker.state['plan_id']
        plan = w.read(folder/'plan.json')
        deferred = next(p for p in plan['components'] if p['id'] == 'other')
        self.assertFalse(deferred['available'])
        self.assertEqual(deferred['reason'], 'outside_idle_selection')
        self.assertTrue(deferred['available_at_check'])
        self.assertEqual(w.digest(folder/'other.provider.json'), deferred['provider_plan_sha256'])
        self.assertEqual(plan['provider_attempts'], ['game'])

    def test_hourly_check_keeps_unrelated_offer_available_for_explicit_apply(self):
        self.game.write_text('next')
        self.worker.tick()
        self.assertEqual(self.worker.state['state'], 'available')
        plan = w.read(self.state/'plans'/self.worker.state['plan_id']/'plan.json')
        self.assertTrue(plan['components'][0]['available'])
        self.assertFalse(plan['components'][1]['available'])
        self.assertEqual(self.calls, ['plan'])
        self.worker.tick()
        self.assertEqual(self.binary.read_bytes(), b'old-binary')
        self.assertEqual(self.worker.state['plan_id'], plan['id'])
        self.assertEqual(self.calls, ['plan'])
        self.worker.apply(plan, self.worker.policy)
        self.assertEqual(self.binary.read_bytes(), b'new-binary')
        self.assertEqual(self.game.read_text(), 'next')

    def test_changed_policy_or_expanded_plan_refuses_before_draining(self):
        plan = self.worker.plan(self.worker.policy, when_idle_components=['game'])
        for edit in ('full', 'revision', 'unselected_available', 'discovery_digest', 'wrong_registration'):
            with self.subTest(edit=edit):
                altered = copy.deepcopy(plan)
                if edit == 'full':
                    altered.pop('when_idle_components')
                elif edit == 'revision':
                    altered['policy_revision'] -= 1
                elif edit == 'unselected_available':
                    altered['components'][0]['available'] = True
                elif edit == 'discovery_digest':
                    altered['discovery_plan_sha256'] = 'a' * 64
                else:
                    altered['registration_sha256'] = 'a' * 64
                with self.assertRaises(ValueError):
                    self.worker.apply(altered, self.worker.policy, scheduled=True)
        self.assertEqual(self.calls, ['plan', 'plan'])

    def test_disabling_periodic_checks_defers_a_prepared_scoped_install(self):
        self.worker.plan(self.worker.policy, when_idle_components=['game'])
        self.worker.policy['check_minutes'] = 0
        self.worker.tick()
        self.assertEqual(self.game.read_text(), 'first')
        self.assertEqual(self.calls, ['plan', 'plan'])

    def test_only_distinct_registered_component_ids_can_be_enrolled(self):
        for ids in ([], True, 'game', ['game', 'game'], ['missing'], [None]):
            with self.subTest(ids=ids):
                w.write(self.path, dict(self.registration, when_idle_components=ids))
                with self.assertRaisesRegex(ValueError, 'when_idle_components'):
                    w.load_registration(self.path)


if __name__ == '__main__':
    unittest.main()
