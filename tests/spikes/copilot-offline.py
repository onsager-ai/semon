#!/usr/bin/env python3
"""Probe pinned Copilot persistence with a local mock; no account or API calls.

Run with --copilot pointing at an already installed pinned executable and
--output pointing at a new disposable directory. This does not install a CLI,
read a native home, or claim native model/approval/interactive coverage.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--copilot', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--expected-version', choices=['1.0.90', '1.0.91'], default='1.0.91')
    args = parser.parse_args()
    executable = args.copilot.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    environment = dict(os.environ)
    # All model requests go to this process's localhost server; never forward
    # GitHub or provider credentials to the disposable CLI or tool processes.
    for name in list(environment):
        if name.startswith('COPILOT_') or name in ('GH_TOKEN', 'GITHUB_TOKEN'):
            environment.pop(name)
    environment.update(COPILOT_HOME=str(output / 'home'), COPILOT_OFFLINE='true')
    version = subprocess.run([str(executable), '--no-auto-update', '--version'], env=environment,
                             text=True, capture_output=True, check=True)
    if version.stdout.splitlines()[0] != f'GitHub Copilot CLI {args.expected_version}.':
        raise RuntimeError(f'Expected pinned Copilot {args.expected_version}')
    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'object': 'list', 'data': [
                {'id': 'gpt-4', 'object': 'model'}]}).encode())

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['content-length'])))
            requests.append(request)
            has_results = any(m.get('role') == 'tool' for m in request.get('messages', []))
            if has_results:
                delta = {'role': 'assistant', 'content': 'Disposable mock response.'}
                finish = 'stop'
            else:
                calls = [{'index': i, 'id': f'mock-call-{i}', 'type': 'function',
                          'function': {'name': 'bash', 'arguments': json.dumps({
                              'command': command, 'description': 'Disposable mock tool probe'})}}
                         for i, command in enumerate(['printf mock-one', 'printf mock-two; exit 1'])]
                delta = {'role': 'assistant', 'tool_calls': calls}
                finish = 'tool_calls'
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for choice in [{'delta': delta, 'index': 0, 'finish_reason': None},
                           {'delta': {}, 'index': 0, 'finish_reason': finish}]:
                message = {'id': f'mock-{len(requests)}', 'object': 'chat.completion.chunk',
                           'created': 1, 'model': 'gpt-4', 'choices': [choice]}
                self.wfile.write(('data: ' + json.dumps(message) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    environment.update(COPILOT_PROVIDER_BASE_URL=f'http://127.0.0.1:{server.server_port}/v1',
                       COPILOT_MODEL='gpt-4')
    try:
        process = subprocess.run([
            str(executable), '--no-auto-update', '--disable-builtin-mcps',
            '--no-custom-instructions', '--no-remote', '--no-remote-export', '-p', 'Respond with a brief greeting.',
            '--allow-all-tools', '--output-format', 'json'], cwd=output, env=environment,
            text=True, capture_output=True, timeout=60)
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
    (output / 'stdout.jsonl').write_text(process.stdout)
    (output / 'stderr.txt').write_text(process.stderr)
    process.check_returncode()
    files = list((output / 'home/session-state').glob('*/events.jsonl'))
    if len(files) != 1:
        raise RuntimeError(f'Expected one disposable session, got {len(files)}')
    native = files[0].read_bytes()
    records = [json.loads(line) for line in native.splitlines()]
    start_rows = [r['data'] for r in records if r['type'] == 'tool.execution_start']
    complete_rows = [r['data'] for r in records if r['type'] == 'tool.execution_complete']
    request_rows = [call for r in records if r['type'] == 'assistant.message'
                    for call in r['data'].get('toolRequests', [])]
    starts = {r['toolCallId']: r for r in start_rows}
    completes = {r['toolCallId']: r for r in complete_rows}
    requested = {r['toolCallId']: r for r in request_rows}
    if len(start_rows) != 2 or len(complete_rows) != 2 or len(request_rows) != 2:
        raise RuntimeError('Unexpected missing or duplicated tool evidence')
    if set(starts) != {'mock-call-0', 'mock-call-1'} or starts.keys() != completes.keys() or starts.keys() != requested.keys():
        raise RuntimeError('Native tool identities did not join exactly')
    for identity, expected in [('mock-call-0', 0), ('mock-call-1', 1)]:
        if starts[identity]['arguments'] != requested[identity]['arguments']:
            raise RuntimeError('Requested and executed tool arguments differ')
        if starts[identity]['toolName'] != 'bash':
            raise RuntimeError('Expected overlapping same-name calls')
        if completes[identity]['shellExecution']['exitCode'] != expected:
            raise RuntimeError('Persisted shell exit code differs from mock command')
        expected_text = 'mock-one' if expected == 0 else 'mock-two'
        if expected_text not in completes[identity]['result']['content']:
            raise RuntimeError('Persisted output joined to the wrong call')
    if any(r.get('ephemeral') for r in records):
        raise RuntimeError('An ephemeral event unexpectedly persisted')
    report = {'version': version.stdout.strip(), 'native_sha256': hashlib.sha256(native).hexdigest(),
              'persisted_event_types': sorted({r['type'] for r in records}),
              'exact_tool_ids': sorted(starts), 'request_count': len(requests),
              'confidence': 'native headless persistence, localhost mock model; no native token claim'}
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
