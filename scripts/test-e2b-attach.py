"""Real pinned SDK construction over a mock GET; no provider or compute access."""
import importlib.util
import importlib.metadata
from pathlib import Path
import unittest
from unittest.mock import patch
import httpx
from e2b import Sandbox
from e2b.api.client.client import Client

spec = importlib.util.spec_from_file_location('attachment', Path(__file__).with_name('e2b-attach.py'))
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)
LABELS = {name: 'fixture' for name in ('semon_version', 'semon_deployment', 'semon_owner',
                                     'semon_workspace', 'semon_session', 'semon_operation', 'semon_epoch')}


class AttachmentTests(unittest.TestCase):
    def detail(self):
        return dict(templateID='base', sandboxID='owned-runtime', clientID='fixture',
                    startedAt='2026-10-05T00:00:00Z', endAt='2026-10-05T01:00:00Z',
                    envdVersion='0.2.0', cpuCount=2, memoryMB=512, diskSizeMB=1024,
                    state='running', metadata=LABELS, envdAccessToken='synthetic-guest-token')

    def attach(self, detail, status=200):
        calls = []
        def receive(request):
            calls.append((request.method, request.url.path))
            return httpx.Response(status, json=detail)
        http = httpx.Client(base_url='https://api.e2b.dev', transport=httpx.MockTransport(receive))
        client = Client(base_url='https://api.e2b.dev').set_httpx_client(http)
        with http, patch('e2b.api.client_sync.get_api_client', return_value=client) as factory, \
                patch.object(Sandbox, 'connect', side_effect=AssertionError('must not resume')), \
                patch.object(Sandbox, 'create', side_effect=AssertionError('must not provision')):
            result = worker.attach_running('owned-runtime', 'synthetic-provider', LABELS, Sandbox)
        self.assertEqual(calls, [('GET', '/sandboxes/owned-runtime')])
        self.assertEqual(factory.call_args.args[0].retries, 0)
        return result

    def test_actual_sdk_attaches_with_private_guest_token_and_no_connect(self):
        self.assertEqual(importlib.metadata.version('e2b'), '2.52.0')
        box = self.attach(self.detail())
        self.assertEqual(box.sandbox_id, 'owned-runtime')
        self.assertEqual(box._envd_access_token, 'synthetic-guest-token')
        self.assertEqual(box.connection_config.retries, 0)
        self.assertEqual(box.connection_config.sandbox_headers['X-Access-Token'], 'synthetic-guest-token')

    def test_wrong_owner_or_paused_or_missing_token_never_attach(self):
        for change in [dict(metadata={}), dict(state='paused'), dict(sandboxID='foreign-runtime'),
                       dict(envdAccessToken=''), dict(envdAccessToken=None)]:
            self.assertIsNone(self.attach(self.detail() | change))

    def test_provider_refusal_stays_unknown_after_one_get(self):
        self.assertIsNone(self.attach({'code': 401, 'message': 'refused'}, status=401))


if __name__ == '__main__':
    unittest.main()
