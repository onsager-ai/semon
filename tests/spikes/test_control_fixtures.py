"""Offline assertions against native control evidence, not a mock product driver."""
import hashlib
import json
from pathlib import Path
import unittest

REPO = Path(__file__).resolve().parents[2]
ROOT = REPO / 'tests/fixtures/compatibility/codex-0.159.0-alpha.3/control'


class NativeControlFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.report = json.loads((ROOT/'qualification.json').read_text())
        cls.cases = cls.report['cases']
        cls.frames = cls.report['frames']

    def test_provenance_and_disposable_paths(self):
        manifest = json.loads((ROOT/'manifest.json').read_text())
        probe = (REPO/manifest['source_reference']['probe_path']).read_bytes()
        self.assertEqual(hashlib.sha256(probe).hexdigest(), manifest['source_reference']['probe_sha256'])
        for entry in manifest['fixtures']:
            data = (ROOT/entry['path']).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), entry['fixture_sha256'])
            self.assertNotIn(b'/workspace/', data)
        self.assertTrue(self.report['probe_completed'])
        self.assertFalse(self.report['local_control_go'])
        self.assertEqual(self.report['version'], 'codex-cli 0.159.0-alpha.3')

    def test_default_tui_and_daemon_are_blocked_not_live_attach_evidence(self):
        for case in ('default_daemon', 'default_tui'):
            record = self.cases[case]
            self.assertEqual(record['exit'], 1)
            self.assertIn('package link escapes its root', record.get('stderr', record.get('terminal')))
        self.assertFalse(self.cases['default_daemon']['control_socket_exists'])
        self.assertIn('foreground', self.report['scope'])

    def test_broadcast_reconnect_and_exact_item_command_binding(self):
        request = next(row['frame'] for row in self.frames if row['client']=='owner'
            and row['frame'].get('method')=='item/commandExecution/requestApproval')
        self.assertIs(type(request['id']), int)
        p = request['params']
        item = next(row['frame']['params'] for row in self.frames if row['client']=='owner'
            and row['frame'].get('method')=='item/started'
            and row['frame']['params']['item']['type']=='commandExecution'
            and row['frame']['params']['turnId']==p['turnId'])
        self.assertEqual((item['threadId'], item['turnId'], item['item']['id']),
                         (p['threadId'], p['turnId'], p['itemId']))
        self.assertEqual((item['item']['command'], item['item']['cwd']), (p['command'], p['cwd']))
        self.assertEqual(p['command'], "/bin/bash -lc 'printf SEMON_APPROVAL_PROBE'")
        self.assertEqual(p['cwd'], '<FIXTURE>/work')
        self.assertTrue(self.cases['approval_broadcast']['same_frame'])
        self.assertEqual(self.cases['pending_reconnect'],
                         {'replayed': True, 'same_request': True, 'resume_succeeded': True})
        for client in ('owner', 'observer', 'reconnected'):
            self.assertTrue(any(row['client']==client and row['direction']=='receive'
                and row['frame']==request for row in self.frames))

    def test_resolution_cannot_attribute_a_winner_and_late_answer_cannot_override(self):
        for frame in self.cases['resolution']:
            self.assertEqual(set(frame['params']), {'threadId', 'requestId'})
            self.assertEqual(frame['params']['requestId'], self.cases['approval_broadcast']['request_id'])
        self.assertEqual(self.cases['competing_answer']['status'], 'declined')
        responses = [row['frame']['result']['decision'] for row in self.frames
            if row['direction']=='send' and 'result' in row['frame']]
        self.assertIn('accept', responses)
        self.assertIn('decline', responses)
        self.assertEqual(set(responses), {'accept', 'decline'})

    def test_allow_once_has_separate_execution_evidence(self):
        item = self.cases['allow_once_outcome']
        self.assertEqual(item['status'], 'completed')
        self.assertEqual(item['exitCode'], 0)
        self.assertEqual(item['aggregatedOutput'], 'SEMON_APPROVAL_PROBE')
        self.assertEqual(item['id'], 'allow_fixture')

    def test_stale_turn_interrupt_and_persisted_outcomes(self):
        self.assertIn('expected active turn id', self.cases['stale_steer']['error']['message'])
        self.assertEqual(self.cases['interrupt_acceptance']['result'], {})
        self.assertEqual(self.cases['interrupt_outcome'], 'interrupted')
        history = self.cases['persisted_history']['result']['thread']
        self.assertIn('interrupted', [turn['status'] for turn in history['turns']])
        self.assertNotIn('MUST_NOT_APPEAR', json.dumps(history))

    def test_malformed_client_isolation_and_pending_session_termination(self):
        self.assertIn('result', self.cases['after_malformed'])
        self.assertEqual(self.cases['request_at_termination']['method'], 'item/commandExecution/requestApproval')
        self.assertEqual(self.cases['server_termination'], {'probe_error': 'disconnected'})
        request_id = self.cases['request_at_termination']['id']
        self.assertFalse(any(row['direction']=='send' and row['frame'].get('id')==request_id
                             and 'result' in row['frame'] for row in self.frames))

    def test_unqualified_questions_and_diffs_are_not_support_claims(self):
        self.assertIsNone(self.cases['question_request'])
        self.assertIsNone(self.cases['file_request'])
        self.assertTrue(self.cases['patch_not_written'])
        feedback = json.dumps(self.cases['synthetic_tool_feedback'])
        self.assertIn('request_user_input is unavailable in Default mode', feedback)
        self.assertIn('unsupported call: apply_patch', feedback)


if __name__ == '__main__':
    unittest.main()
