"""Offline provenance and exact-identity checks; does not launch any harness."""
import hashlib
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1] / 'fixtures/compatibility'


class CompatibilityFixtures(unittest.TestCase):
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
