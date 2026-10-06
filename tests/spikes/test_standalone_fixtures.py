"""Sanitized native request/execution evidence, distinct from live provider qualification."""
import hashlib
import json
from pathlib import Path
import unittest

REPO = Path(__file__).resolve().parents[2]
ROOT = REPO / 'tests/fixtures/compatibility/codex-0.160.0/standalone'


class StandaloneNativeEvidence(unittest.TestCase):
    def test_source_and_evidence_hashes(self):
        manifest = json.loads((ROOT/'manifest.json').read_text())
        for row in manifest['sources'] + manifest['fixtures']:
            self.assertEqual(hashlib.sha256((REPO/row['path']).read_bytes()).hexdigest(), row['sha256'])
        self.assertEqual(manifest['native_source'], 'a956835d020762cb2b570053af06f643a11c0ecc')
        self.assertFalse(manifest['live_openrouter_qualified'])
        self.assertFalse(manifest['native_ssh_qualified'])

    def test_native_baseline_advertises_disabled_host(self):
        before = json.loads((ROOT/'baseline.json').read_text())
        self.assertEqual(before['model'], 'openai/gpt-6-luna')
        self.assertEqual(before['executor_schema']['type'], 'custom')
        self.assertEqual(before['executor_schema']['name'], 'exec')
        self.assertEqual(before['runtime_error'], 'code-mode host is disabled')
        self.assertIsNone(before['shell_exit'])
        self.assertFalse(before['standalone_instructions'])
        self.assertTrue(before['shutdown_reaped'])

    def test_native_direct_schema_and_actual_execution(self):
        after = json.loads((ROOT/'corrected.json').read_text())
        self.assertTrue(after['native_model_instructions_preserved'])
        self.assertTrue(after['all_native_model_messages_preserved'])
        profile = after['session_profile']
        self.assertEqual(profile['instruction_source'], 'native debug models --bundled')
        self.assertEqual(profile['profile'], 'standalone-direct-v1')
        self.assertEqual(profile['native_version'], '0.160.0')
        for key in ('native_catalog_sha256', 'effective_catalog_sha256'):
            self.assertRegex(profile[key], r'^[a-f0-9]{64}$')
        schema = after['executor_schema']
        self.assertEqual(schema['type'], 'function')
        self.assertEqual(schema['name'], 'exec_command')
        self.assertEqual(schema['parameters']['properties']['cmd']['type'], 'string')
        self.assertNotIn('exec', after['tool_names'])
        self.assertNotIn('spawn_agent', after['tool_names'])
        self.assertEqual(after['shell_exit'], 0)
        self.assertEqual(after['shell_output'], 'SEMON_EXECUTED_307:42')
        self.assertEqual(after['final_output'], after['shell_output'])
        for key in ('standalone_instructions', 'credential_header_only', 'credential_excluded_from_executor_and_files', 'duplicate_prevented', 'shutdown_reaped'):
            self.assertTrue(after[key])

    def test_reconciliation_never_replays_unknown_work(self):
        rows = json.loads((ROOT/'reconciliation.json').read_text())
        self.assertEqual(len({row['operation'] for row in rows}), len(rows))
        for row in rows:
            self.assertTrue(row['dispatch_receipt_persisted'])
            self.assertTrue(row['native_process_absent'])
            self.assertFalse(row['replayed'])
            self.assertLessEqual(row['tool_call_count'], 1)


if __name__ == '__main__':
    unittest.main()
