import io
import json
import pathlib
import subprocess
import tempfile
import unittest
import urllib.error
from unittest.mock import patch

import test_worker as fixtures
w=fixtures.w


class MetadataTests(unittest.TestCase):
    def test_public_quota_error_does_not_repeat_anonymous_request_via_cli(self):
        error=urllib.error.HTTPError('https://api.github.com/example',403,'Forbidden',
            {'X-RateLimit-Remaining':'0','X-RateLimit-Reset':'2000'},io.BytesIO(b'{}'))
        with patch.object(w,'owner_github_available',return_value=False),patch.object(w,'http_json',side_effect=error),patch.object(w,'github_cli_json') as cli,patch.object(w.time,'time',return_value=1000):
            with self.assertRaises(w.GitHubRateLimited) as raised:w.gh('repos/example/project/releases/latest')
        self.assertEqual(raised.exception.retry_at_ms,2000000)
        cli.assert_not_called()
        self.assertNotIn('command_failed',str(raised.exception))

    def test_existing_owner_account_is_used_without_exporting_credential(self):
        response=subprocess.CompletedProcess([],0,b'HTTP/2.0 200 OK\r\nContent-Type: application/json\r\n\r\n{"tag_name":"v1.0.0"}\n',b'')
        with patch.object(w,'owner_github_available',return_value=True),patch.object(w.subprocess,'run',return_value=response) as run,patch.object(w,'http_json') as public:
            self.assertEqual(w.gh('repos/example/project/releases/latest'),{'tag_name':'v1.0.0'})
        public.assert_not_called()
        self.assertEqual(run.call_args.args[0],['gh','api','--include','--hostname','github.com','repos/example/project/releases/latest'])
        self.assertNotIn('env',run.call_args.kwargs)

    def test_authenticated_rate_limit_preserves_its_own_retry_header(self):
        response=subprocess.CompletedProcess([],1,b'HTTP/2.0 429 Too Many Requests\nRetry-After: 700\nX-RateLimit-Remaining: 0\nX-RateLimit-Reset: 1400\n\n{}',b'sensitive diagnostic not for agent')
        with patch.object(w.subprocess,'run',return_value=response),patch.object(w.time,'time',return_value=1000):
            with self.assertRaises(w.GitHubRateLimited) as raised:w.github_cli_json('repos/example/project/releases/latest')
        self.assertEqual(raised.exception.retry_at_ms,1700000)
        self.assertNotIn('sensitive',str(raised.exception))

    def test_invalid_owner_login_is_actionable_and_does_not_publish_stderr(self):
        response=subprocess.CompletedProcess([],1,b'HTTP/2.0 401 Unauthorized\n\n{}',b'private detail')
        with patch.object(w.subprocess,'run',return_value=response):
            with self.assertRaisesRegex(RuntimeError,'login is invalid') as raised:w.github_cli_json('repos/example/project/releases/latest')
        self.assertNotIn('private',str(raised.exception))

    def test_optional_cli_missing_still_supports_public_metadata(self):
        with patch.object(w.subprocess,'run',side_effect=FileNotFoundError),patch.object(w,'http_json',return_value={'tag_name':'v1'}) as public:
            self.assertEqual(w.gh('repos/example/project/releases/latest'),{'tag_name':'v1'})
        public.assert_called_once()

    def test_missing_owner_token_is_not_an_authenticated_api_fallback(self):
        with patch.object(w.subprocess,'run',return_value=subprocess.CompletedProcess([],1,b'',b'no token')) as run:
            self.assertFalse(w.owner_github_available())
        self.assertEqual(run.call_args.args[0],['gh','auth','token','--hostname','github.com'])

    def test_retry_headers_and_secondary_limit(self):
        with patch.object(w.time,'time',return_value=1000):
            self.assertEqual(w.github_rate_limit(403,{},b'{"message":"secondary rate limit"}').retry_at_ms,1060000)
            self.assertEqual(w.github_rate_limit(429,{'Retry-After':'Thu, 01 Jan 1970 00:25:00 GMT'}).retry_at_ms,1500000)
            for value in ('bad','nan','inf','1e99','-1'):
                self.assertEqual(w.github_rate_limit(429,{'X-RateLimit-Remaining':'0','X-RateLimit-Reset':value}).retry_at_ms,1060000)
            self.assertIsNone(w.github_rate_limit(403,{},b'{"message":"permission denied"}'))

    def test_cache_is_one_plan_only_and_failed_results_are_not_cached(self):
        with patch.object(w,'owner_github_available',return_value=False) as owner,patch.object(w,'http_json',side_effect=[{'tag':'one'},{'tag':'two'}]) as public:
            with w.github_metadata_session():
                self.assertEqual(w.gh('same'),{'tag':'one'})
                self.assertEqual(w.gh('same'),{'tag':'one'})
            with w.github_metadata_session():self.assertEqual(w.gh('same'),{'tag':'two'})
        self.assertEqual(public.call_count,2);self.assertEqual(owner.call_count,2)


class SchedulerTests(unittest.TestCase):
    setUp=fixtures.UpdateTests.setUp
    worker=fixtures.UpdateTests.worker
    def test_aliases_share_one_release_lookup_per_plan(self):
        obj=self.worker()
        obj.c['components']=[{'id':name,'kind':'runtime','binary':str(self.binary)}
                             for name in ('runtime','task-helpers','task-helpers-constellation')]
        with patch.object(w,'owner_github_available',return_value=False,create=True),patch.object(w,'http_json',return_value={'tag_name':'v0.14.0'}) as public,patch.object(w,'run',return_value=b'agent-runtime 0.14.0'):
            plan=obj.plan(obj.policy)
        self.assertEqual(public.call_count,1)
        self.assertEqual(len(plan['components']),3)
        self.assertTrue(all(not c['available'] for c in plan['components']))

    def test_server_backoff_survives_restart_and_never_applies_old_plan(self):
        obj=self.worker();obj.policy['install']='when_idle'
        obj.save(state='check_failed',last_check=0,github_retry_at_ms=2000000,github_rate_limit_count=1,plan_id='old')
        obj=self.worker();obj.policy['install']='when_idle'
        obj.apply=lambda *a,**k:self.fail('old plan applied during quota wait')
        obj.plan=lambda *a,**k:self.fail('network called before reset')
        with patch.object(w.time,'time',return_value=1000):obj.tick()
        self.assertEqual(obj.state['last_check'],0)

    def test_limit_error_is_recorded_and_retry_obeys_policy(self):
        obj=self.worker();obj.policy['install']='when_idle'
        obj.plan=lambda *a:(_ for _ in ()).throw(w.GitHubRateLimited(4000000))
        with patch.object(w.time,'time',return_value=3601):obj.tick()
        self.assertEqual(obj.state['github_retry_at_ms'],4000000)
        self.assertEqual(obj.state['last_check'],3601000)
        obj.policy['check_minutes']=0
        obj.plan=lambda *a:self.fail('disabled automatic checks retried')
        with patch.object(w.time,'time',return_value=5000):obj.tick()

    def test_successful_retry_clears_backoff_and_keeps_fixed_same_version_binary(self):
        obj=self.worker();obj.save(state='check_failed',last_check=950000,github_retry_at_ms=1000000,github_rate_limit_count=2)
        obj.c['components'][0]['kind']='runtime'
        with patch.object(w.time,'time',return_value=1001),patch.object(w,'owner_github_available',return_value=False),patch.object(w,'http_json',return_value={'tag_name':'v0.14.0'}),patch.object(w,'run',return_value=b'agent-runtime 0.14.0'):
            obj.tick()
        self.assertEqual(obj.state['state'],'up_to_date')
        self.assertEqual(obj.state['github_retry_at_ms'],0)
        self.assertEqual(obj.state['github_rate_limit_count'],0)
        self.assertEqual(self.binary.read_bytes(),b'old-binary')

    def test_request_during_wait_does_not_extend_backoff_or_replay(self):
        obj=self.worker();obj.save(state='check_failed',last_check=900000,github_retry_at_ms=2000000,github_rate_limit_count=1)
        request={'pending':True,'id':'wait-request','actor':'agent','action':'update','policy_revision':0}
        obj.report=lambda **kwargs:{'policy':obj.policy,'request':request,'active_prompts':0}
        with patch.object(w.time,'time',return_value=1000),patch.object(w,'gh') as network:obj.tick()
        network.assert_not_called()
        self.assertEqual(obj.state['github_retry_at_ms'],2000000)
        self.assertEqual(obj.state['last_check'],900000)
        self.assertEqual(obj.state['github_rate_limit_count'],1)
        self.assertIn('wait-request',obj.state['completed_requests'])

    def test_repeated_secondary_limits_back_off_exponentially(self):
        obj=self.worker();obj.defer_github_check(w.GitHubRateLimited(1060000),1000000)
        self.assertEqual(obj.state['github_retry_at_ms'],1060000)
        obj.defer_github_check(w.GitHubRateLimited(1120000),1060000)
        self.assertEqual(obj.state['github_retry_at_ms'],1180000)

    def test_non_quota_failure_after_reset_does_not_create_five_second_retry_loop(self):
        obj=self.worker();obj.save(state='check_failed',last_check=950000,github_retry_at_ms=1000000)
        with patch.object(w.time,'time',return_value=1001),patch.object(obj,'build_plan',side_effect=RuntimeError('offline')) as check:
            obj.tick()
            obj.tick()
        self.assertEqual(check.call_count,1)
        self.assertEqual(obj.state['github_retry_at_ms'],0)


if __name__=='__main__':unittest.main()
