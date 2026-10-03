"""Offline provenance and exact-identity checks; does not launch any harness."""
import hashlib
import json
from pathlib import Path
import unittest
import importlib.util
import tempfile
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1] / 'fixtures/compatibility'


class CompatibilityFixtures(unittest.TestCase):
    def test_runtime_environment_does_not_forward_ambient_secrets(self):
        script = Path(__file__).with_name('copilot-offline.py')
        spec = importlib.util.spec_from_file_location('copilot_probe', script)
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        ambient = {'PATH': '/usr/bin', 'LANG': 'C.UTF-8', 'HOME': '/fixture/private',
                   'OPENAI_API_KEY': 'fixture-only', 'AWS_SECRET_ACCESS_KEY': 'fixture-only',
                   'DATABASE_URL': 'fixture-only', 'ARBITRARY_CI_SECRET': 'fixture-only',
                   'GH_TOKEN': 'fixture-only', 'COPILOT_PROVIDER_API_KEY': 'fixture-only'}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            original = dict(ambient)
            environment = probe.probe_environment(output, ambient)
            self.assertEqual(set(environment), {'PATH', 'LANG', 'COPILOT_HOME', 'COPILOT_OFFLINE',
                                               'TMPDIR', 'TMP', 'TEMP'})
            self.assertEqual(environment['COPILOT_HOME'], str(output / 'home'))
            self.assertEqual(environment['TMPDIR'], str(output / 'tmp'))
            self.assertEqual(ambient, original, 'parent settings must remain intact')

    def test_copilot_exact_subagent_edges_and_inclusive_mock_usage(self):
        for version in ('1.0.90', '1.0.91'):
            with self.subTest(version=version):
                directory = ROOT / f'copilot-{version}' / 'subagent'
                manifest = json.loads((directory / 'manifest.json').read_text())
                data = (directory / manifest['fixture_path']).read_bytes()
                self.assertEqual(hashlib.sha256(data).hexdigest(), manifest['fixture_sha256'])
                self.assertNotIn(b'/workspace/', data)
                self.assertRegex(manifest['binary_sha256'], r'^[a-f0-9]{64}$')
                rows = list(map(json.loads, data.splitlines()))
                starts = [r['data'] for r in rows if r['type'] == 'subagent.started']
                completed = [r['data'] for r in rows if r['type'] == 'subagent.completed']
                self.assertEqual(len(starts), 1)
                self.assertEqual(len(completed), 1)
                call = starts[0]['toolCallId']
                self.assertEqual(call, completed[0]['toolCallId'])
                requests = [c for r in rows if r['type'] == 'assistant.message'
                            for c in r['data'].get('toolRequests', [])]
                self.assertEqual([c['toolCallId'] for c in requests], [call])
                results = [r['data'] for r in rows if r['type'] == 'tool.execution_complete']
                self.assertEqual([r['toolCallId'] for r in results], [call])
                child = [r['data'] for r in rows if r['type'] == 'assistant.message'
                         and r['data'].get('parentToolCallId') == call]
                self.assertEqual(len(child), 1)
                users = {r['data']['messageId']: r['data'] for r in rows if r['type'] == 'user.message'}
                origin = users[child[0]['originatingMessageId']]
                root_origins = {r['data']['originatingMessageId'] for r in rows
                                if r['type'] == 'assistant.message'
                                and not r['data'].get('parentToolCallId')}
                self.assertNotIn(origin['messageId'], root_origins)
                self.assertTrue(origin['source'].startswith('agent-'))
                self.assertEqual(origin['interactionId'], child[0]['interactionId'])
                self.assertEqual(origin['turnId'], child[0]['turnId'])
                # These native records have no agentId: do not manufacture one.
                for record in (starts[0], completed[0], child[0], origin):
                    self.assertNotIn('agentId', record)
                self.assertEqual(starts[0]['agentName'], completed[0]['agentName'])
                self.assertEqual(starts[0]['agentDisplayName'], completed[0]['agentDisplayName'])
                self.assertEqual(manifest['source_reference']['semon_commit'],
                                 'c6287dbbd09afc05cc7bdff44a7e456c17148214')
                self.assertRegex(manifest['source_reference']['probe_sha256'], r'^[a-f0-9]{64}$')
                metrics = [r['data']['modelMetrics']['gpt-4'] for r in rows
                           if r['type'] == 'session.shutdown']
                self.assertEqual(len(metrics), 1)
                self.assertEqual(metrics[0]['requests']['count'], 3)
                self.assertEqual((metrics[0]['usage']['inputTokens'],
                                  metrics[0]['usage']['outputTokens'],
                                  metrics[0]['usage']['cacheReadTokens']), (33, 9, 6))
                self.assertEqual(completed[0]['totalTokens'], 14)
                self.assertEqual(completed[0]['totalToolCalls'], 0)
                for r in rows:
                    if r['type'] == 'session.start':
                        self.assertEqual(r['data']['copilotVersion'], version)
                    if r['type'] == 'system.message':
                        self.assertEqual(r['data'], {'content': '[fixture: native system prompt removed]'})

    def test_lifecycle_validation_survives_python_optimization(self):
        # Only a disposable fake executable is launched, never a native harness.
        script = Path(__file__).with_name('copilot-lifecycle.py')
        for failure in ('changed-prefix', 'executed-denial'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fake = root / 'fake-copilot'
                fake.write_text('#!' + sys.executable + '\n' + """
import os, pathlib, sys
if '--version' in sys.argv:
    print('GitHub Copilot CLI 1.0.91.')
    sys.exit(0)
root = pathlib.Path.cwd()
state = pathlib.Path(os.environ['COPILOT_HOME']) / 'session-state'
initial = state / 'synthetic-session' / 'events.jsonl'
initial.parent.mkdir(parents=True, exist_ok=True)
if any(arg.startswith('--resume=') for arg in sys.argv):
    initial.write_bytes(b'changed\\n' if FAILURE == 'changed-prefix' else b'{}\\n{}\\n')
elif '--deny-tool=shell' in sys.argv:
    denied = state / 'synthetic-denied' / 'events.jsonl'
    denied.parent.mkdir(parents=True)
    denied.write_bytes(b'{}\\n')
    (root / 'SHOULD-NOT-EXIST').touch()
else:
    initial.write_bytes(b'{}\\n')
""".replace('FAILURE', repr(failure)))
                fake.chmod(0o700)
                output = root / 'probe'
                result = subprocess.run([sys.executable, '-O', str(script), '--copilot',
                                         str(fake), '--output', str(output)],
                                        capture_output=True, text=True, timeout=15)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('RuntimeError: Probe validation failed:', result.stderr)
                self.assertFalse((output / 'report.json').exists())

    def test_claude_native_resume_and_copied_fork_context(self):
        directory = ROOT / 'claude-2.1.288/lifecycle'
        manifest = json.loads((directory / 'manifest.json').read_text())
        fixtures = {}
        for entry in manifest['fixtures']:
            data = (directory / entry['path']).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry['fixture_sha256'])
            self.assertNotIn(b'/workspace/', data)
            fixtures[entry['path']] = data
        self.assertTrue(fixtures['resumed-transcript.jsonl'].startswith(fixtures['initial-transcript.jsonl']))
        parent = list(map(json.loads, fixtures['resumed-transcript.jsonl'].splitlines()))
        child = list(map(json.loads, fixtures['forked-transcript.jsonl'].splitlines()))
        messages = lambda rows: [r for r in rows if r['type'] in ('user', 'assistant')]
        parent_messages = messages(parent)
        child_messages = messages(child)
        self.assertEqual(len(parent_messages), 4)
        self.assertEqual(len(child_messages), 6)
        for original, inherited in zip(parent_messages, child_messages):
            self.assertEqual(original['uuid'], inherited['uuid'])
            self.assertEqual(original['parentUuid'], inherited['parentUuid'])
            self.assertEqual(original['message'], inherited['message'])
            self.assertNotEqual(original['sessionId'], inherited['sessionId'])
        for rows, expected in ((parent, (10, 6)), (child, (15, 9))):
            costs = [r for r in rows if r['type'] == 'cost-state']
            usage = costs[-1]['modelUsage']['claude-sonnet-4-6']
            self.assertEqual((usage['inputTokens'], usage['outputTokens']), expected)
            for row in rows:
                if row['type'] == 'attachment':
                    self.assertEqual(row['attachment']['fixture_removed'], 'native harness attachment payload')
                self.assertNotIn('parentSessionId', row)
        source = manifest['source_reference']
        self.assertEqual(hashlib.sha256(Path(__file__).with_name('claude-lifecycle.py').read_bytes()).hexdigest(), source['probe_sha256'])

    def test_native_baseline_provenance_and_hashes(self):
        for fixture in ('claude-2.1.288', 'codex-0.159.0-alpha.3'):
            with self.subTest(fixture=fixture):
                directory = ROOT / fixture
                manifest = json.loads((directory / 'manifest.json').read_text())
                native = (directory / manifest['fixture_path']).read_bytes()
                self.assertEqual(hashlib.sha256(native).hexdigest(), manifest['fixture_sha256'])
                self.assertEqual(manifest['evidence_origin'],
                                 'native CLI persistence with deterministic mock model')
                self.assertRegex(manifest['binary_sha256'], r'^[a-f0-9]{64}$')
                self.assertNotIn(b'/tmp/semon-codex-resume-', native)
                self.assertTrue(manifest['transformations'])
                self.assertTrue(manifest['limitations'])
                self.assertNotEqual(manifest['original_source_sha256'], manifest['fixture_sha256'])
                records = [json.loads(line) for line in native.splitlines()]
                if fixture.startswith('claude'):
                    self.assertEqual({row['version'] for row in records if 'version' in row}, {'2.1.288'})
                    self.assertTrue(any(row['type'] == 'user' for row in records))
                    self.assertTrue(any(row['type'] == 'assistant' for row in records))
                else:
                    self.assertEqual(records[0]['type'], 'session_meta')
                    self.assertEqual(records[0]['payload']['cli_version'], '0.159.0-alpha.3')
                    self.assertEqual(records[0]['payload']['history_mode'], 'paginated')
                    for row in records:
                        skills = row.get('payload', {}).get('state', {}).get('host_skills', {})
                        if isinstance(skills, dict) and 'body' in skills:
                            self.assertEqual(skills['body'], '[fixture: native host skill instructions removed]')

    def test_copilot_resume_denial_and_cumulative_mock_usage(self):
        for version in ('1.0.90', '1.0.91'):
            directory = ROOT / f'copilot-{version}' / 'lifecycle'
            manifest = json.loads((directory / 'manifest.json').read_text())
            self.assertEqual(manifest['harness_version'], version)
            self.assertEqual(manifest['evidence_origin'],
                             'native CLI persistence with deterministic mock model')
            fixtures = {}
            for entry in manifest['fixtures']:
                data = (directory / entry['path']).read_bytes()
                self.assertEqual(hashlib.sha256(data).hexdigest(), entry['fixture_sha256'])
                self.assertNotEqual(entry['original_source_sha256'], entry['fixture_sha256'])
                self.assertNotIn(b'/workspace/scratch/', data)
                fixtures[entry['path']] = data
                for row in map(json.loads, data.splitlines()):
                    if row['type'] == 'session.start':
                        self.assertEqual(row['data']['copilotVersion'], version)
                        self.assertEqual(row['data']['version'], 1)
                    if row['type'] == 'system.message':
                        self.assertEqual(row['data'], {'content': '[fixture: native system prompt removed]'})
            initial = fixtures['initial.events.jsonl']
            resumed = fixtures['resumed.events.jsonl']
            self.assertTrue(resumed.startswith(initial))
            records = list(map(json.loads, resumed.splitlines()))
            self.assertEqual(sum(row['type'] == 'session.resume' for row in records), 1)
            snapshots = [row['data']['modelMetrics']['gpt-4'] for row in records
                         if row['type'] == 'session.shutdown']
            self.assertEqual([(row['usage']['inputTokens'], row['usage']['outputTokens'],
                               row['usage']['cacheReadTokens']) for row in snapshots],
                             [(11, 3, 2), (22, 6, 4)])
            self.assertEqual([row['requests']['count'] for row in snapshots], [1, 2])
            denial = list(map(json.loads, fixtures['denied.events.jsonl'].splitlines()))
            starts = [row['data'] for row in denial if row['type'] == 'tool.execution_start']
            results = [row['data'] for row in denial if row['type'] == 'tool.execution_complete']
            requested = [call for row in denial if row['type'] == 'assistant.message'
                         for call in row['data'].get('toolRequests', [])]
            self.assertEqual(len(starts), 1)
            self.assertEqual(len(results), 1)
            self.assertEqual(len(requested), 1)
            self.assertEqual(starts[0]['toolCallId'], results[0]['toolCallId'])
            self.assertEqual(starts[0]['toolCallId'], requested[0]['toolCallId'])
            self.assertEqual(starts[0]['arguments'], requested[0]['arguments'])
            self.assertFalse(results[0]['success'])
            self.assertEqual(results[0]['error']['code'], 'denied')
            self.assertNotIn('shellExecution', results[0])
            self.assertFalse(any('approval' in row['type'] for row in denial),
                             'no explicit approval record is established by this probe')

    def test_source_shaped_hashes_and_explicit_unknown_version(self):
        manifest = json.loads((ROOT / 'v1/manifest.json').read_text())
        self.assertEqual(manifest['version'], 1)
        for entry in manifest['fixtures']:
            with self.subTest(path=entry['path']):
                self.assertEqual(entry['origin'], 'source-shaped synthetic')
                self.assertIsNone(entry['harness_version'])
                self.assertIsNone(entry['build'])
                self.assertTrue(entry['source_reference'])
                self.assertTrue(entry['limitations'])
                self.assertEqual(hashlib.sha256((ROOT / 'v1' / entry['path']).read_bytes()).hexdigest(),
                                 entry['sha256'])

    def test_pinned_mock_recording_hash_format_and_exact_joins(self):
        for version in ('1.0.90', '1.0.91'):
            with self.subTest(version=version):
                self.check_mock_recording(version)

    def check_mock_recording(self, version):
        directory = ROOT / f'copilot-{version}'
        manifest = json.loads((directory / 'manifest.json').read_text())
        native = (directory / manifest['fixture_path']).read_bytes()
        self.assertEqual(hashlib.sha256(native).hexdigest(), manifest['fixture_sha256'])
        self.assertEqual(manifest['harness_version'], version)
        self.assertTrue(manifest['transformations'])
        self.assertTrue(manifest['unverified'])
        rows = [json.loads(line) for line in native.splitlines()]
        self.assertEqual(rows[0]['type'], 'session.start')
        self.assertEqual(rows[0]['data']['version'], 1)
        self.assertEqual(rows[0]['data']['copilotVersion'], version)
        self.assertEqual(len({row['id'] for row in rows}), len(rows))
        starts = [row['data'] for row in rows if row['type'] == 'tool.execution_start']
        ends = [row['data'] for row in rows if row['type'] == 'tool.execution_complete']
        requested = [call for row in rows if row['type'] == 'assistant.message'
                     for call in row['data'].get('toolRequests', [])]
        self.assertEqual(len(starts), 2)
        self.assertEqual(len(ends), 2)
        by_id = {row['toolCallId']: row for row in starts}
        requests = {row['toolCallId']: row for row in requested}
        self.assertEqual(len(by_id), 2)
        self.assertEqual(set(by_id), set(requests))
        self.assertEqual({row['toolCallId'] for row in ends}, set(by_id))
        self.assertEqual({row['toolName'] for row in starts}, {'bash'})
        self.assertEqual([row['toolCallId'] for row in ends], manifest['completion_order'])
        self.assertEqual([row['toolCallId'] for row in ends],
                         list(reversed([row['toolCallId'] for row in starts])))
        for end in ends:
            identity = end['toolCallId']
            self.assertEqual(by_id[identity]['arguments'], requests[identity]['arguments'])
            expected = 'mock-two' if identity == 'mock-call-1' else 'mock-one'
            self.assertIn(expected, end['result']['content'])
        failure = next(row for row in ends if row['toolCallId'] == 'mock-call-1')
        self.assertTrue(failure['success'])
        self.assertEqual(failure['shellExecution']['exitCode'], 1)
        self.assertFalse(any(row.get('ephemeral') for row in rows))


if __name__ == '__main__':
    unittest.main()
