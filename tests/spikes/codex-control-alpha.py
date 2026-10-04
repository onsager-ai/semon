#!/usr/bin/env python3
"""Qualification only: isolated native Codex + synthetic localhost Responses provider.

No user config/auth is copied. No product capability is enabled by a passing probe. Requires websockets==16.0.
The foreground socket is an explicit alternative to the default TUI daemon.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import pty
import select
import re
import signal
import shutil
import subprocess
import tempfile
import threading
import time

from websockets.sync.client import unix_connect
from websockets.exceptions import ConnectionClosed

PIN = 'codex-cli 0.159.0-alpha.3'
COMMAND = 'printf SEMON_APPROVAL_PROBE'


class Provider(http.server.BaseHTTPRequestHandler):
    calls = []
    pending = None

    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.calls.append(body)
        item = Provider.pending
        Provider.pending = None
        if item is None:
            item = {'type': 'message', 'id': 'msg_fixture', 'role': 'assistant',
                    'status': 'completed', 'content': [{'type': 'output_text',
                    'text': 'SYNTHETIC_ACK', 'annotations': []}]}
        events = [
            {'type': 'response.created', 'response': {'id': 'resp_fixture', 'status': 'in_progress', 'output': []}},
            {'type': 'response.output_item.added', 'output_index': 0, 'item': item},
            {'type': 'response.output_item.done', 'output_index': 0, 'item': item},
            {'type': 'response.completed', 'response': {'id': 'resp_fixture', 'status': 'completed', 'output': [item],
                'usage': {'input_tokens': 5, 'output_tokens': 3, 'total_tokens': 8}}},
        ]
        data = ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class Client:
    """WebSocket over private Unix socket, with bounded waits and a frame record."""
    def __init__(self, binary, env, sock, label, frames):
        self.label, self.frames, self.next_id = label, frames, 100
        self.inbox, self.saved = queue.Queue(), []
        self.socket = unix_connect(str(sock), open_timeout=5, max_size=2**20)
        threading.Thread(target=self.read, daemon=True).start()
        self.rpc('initialize', {'clientInfo': {'name': 'semon_qualification', 'version': '1'},
            'capabilities': {'experimentalApi': True}})
        self.send({'method': 'initialized'})

    def read(self):
        try:
            for line in self.socket:
                frame = json.loads(line)
                self.frames.append({'client': self.label, 'direction': 'receive', 'frame': frame})
                self.inbox.put(frame)
        except (ConnectionClosed, json.JSONDecodeError):
            pass
        finally:
            self.inbox.put({'probe_error': 'disconnected'})

    def send(self, frame):
        self.frames.append({'client': self.label, 'direction': 'send', 'frame': frame})
        self.socket.send(json.dumps(frame))

    def wait(self, predicate, timeout=10):
        for i, frame in enumerate(self.saved):
            if predicate(frame):
                return self.saved.pop(i)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                frame = self.inbox.get(timeout=max(.001, deadline-time.monotonic()))
            except queue.Empty:
                break
            if predicate(frame):
                return frame
            self.saved.append(frame)
        raise TimeoutError(self.label + ': native frame not observed')

    def rpc(self, method, params):
        self.next_id += 1
        self.send({'id': self.next_id, 'method': method, 'params': params})
        return self.wait(lambda x: x.get('id') == self.next_id and 'method' not in x)

    def event(self, method, timeout=10):
        return self.wait(lambda x: x.get('method') == method, timeout)

    def close(self):
        self.socket.close()


def tool(name, arguments):
    return {'type': 'function_call', 'id': 'fc_fixture', 'call_id': 'call_fixture',
            'name': name, 'arguments': json.dumps(arguments)}


def tui_probe(binary, env, work):
    master, slave = pty.openpty()
    process = subprocess.Popen([binary, '--no-alt-screen', '-C', str(work)],
        env=env, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    output = bytearray()
    try:
        deadline = time.monotonic() + 45
        while process.poll() is None and time.monotonic() < deadline:
            if select.select([master], [], [], .1)[0]:
                try:
                    part = os.read(master, 65536)
                except OSError:
                    break
                output.extend(part)
                if b'\x1b[6n' in part:
                    os.write(master, b'\x1b[1;1R')
        if process.poll() is None:
            process.terminate()
        stop(process)
        try:
            while select.select([master], [], [], .1)[0]:
                output.extend(os.read(master, 65536))
        except OSError:
            pass
    finally:
        os.close(master)
        if process.poll() is None:
            process.kill()
            process.wait()
    # Preserve printable terminal evidence without screen-control sequences.
    text = output.decode(errors='replace')
    text = re.sub(r'\x1b\][^\x1b]*(?:\x1b\\|\x07)', '', text)
    text = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', text)
    return {'exit': process.returncode, 'terminal': text[-5000:]}


def stop(process):
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default='codex')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    binary = shutil.which(args.codex)
    if not binary:
        parser.error('Codex binary unavailable')
    version = subprocess.check_output([binary, '--version'], text=True).strip()
    if version != PIN:
        parser.error('untested binary: ' + version + '; expected ' + PIN)
    report = {'version': version, 'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
        'base_sha': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        'provider': 'synthetic localhost SSE; real native app-server; no model credentials',
        'scope': 'qualification only; foreground sessions are not default TUI attachment',
        'local_control_go': False, 'probe_completed': False,
        'cases': {}, 'frames': []}
    clients = []
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='semon-control-', dir=args.output.parent.resolve()) as temp:
            root = Path(temp)
            home, work, sock = root/'home', root/'work', root/'app.sock'
            home.mkdir(mode=0o700)
            work.mkdir()
            # Preserve proxy/CA settings; strip model and harness authentication selectors.
            env = {k: v for k, v in os.environ.items()
                if not any(word in k for word in ('TOKEN', 'AUTH', 'API_KEY', 'CODEX', 'OPENAI', 'AZURE'))}
            env.update(CODEX_HOME=str(home), TERM='xterm-256color', RUST_LOG='error')
            config = f'''model = "mock-model"
model_provider = "synthetic"
[model_providers.synthetic]
name = "Synthetic localhost"
base_url = "http://127.0.0.1:{server.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
'''
            (home/'config.toml').write_text(config)
            daemon = subprocess.run([binary, 'app-server', 'daemon', 'start'], env=env,
                capture_output=True, text=True, timeout=45)
            report['cases']['default_daemon'] = {'exit': daemon.returncode,
                'stdout': daemon.stdout, 'stderr': daemon.stderr,
                'control_socket_exists': (home/'app-server-control/app-server-control.sock').exists()}
            report['cases']['default_tui'] = tui_probe(binary, env, work)
            if daemon.returncode == 0:
                subprocess.run([binary, 'app-server', 'daemon', 'stop'], env=env,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15, check=True)
            # The foreground alternative has a separate, private socket and lifecycle.
            with (root/'server.log').open('w') as log:
                native = subprocess.Popen([binary, 'app-server', '--listen', 'unix://'+str(sock)],
                    env=env, stdout=log, stderr=log, start_new_session=True)
                try:
                    deadline = time.monotonic()+10
                    while not sock.exists() and time.monotonic()<deadline:
                        if native.poll() is not None:
                            raise RuntimeError('foreground server exited')
                        time.sleep(.05)
                    def connect(label):
                        c = Client(binary, env, sock, label, report['frames'])
                        clients.append(c)
                        return c
                    a, b = connect('owner'), connect('observer')
                    thread = a.rpc('thread/start', {'cwd': str(work), 'approvalPolicy': 'on-request',
                        'sandbox': 'read-only'})['result']['thread']['id']
                    a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'WARMUP_PROBE'}]})
                    a.event('turn/completed')
                    report['cases']['observer_resume'] = b.rpc('thread/resume', {'threadId': thread})
                    Provider.pending = tool('exec_command', {'cmd': COMMAND,
                        'sandbox_permissions': 'require_escalated', 'justification': 'Synthetic disposable approval probe'})
                    turn = a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'SEMON_PROBE'}]})['result']['turn']['id']
                    method = 'item/commandExecution/requestApproval'
                    ra = a.event(method)
                    try:
                        rb = b.event(method, 2)
                    except TimeoutError:
                        rb = None
                    report['cases']['approval_broadcast'] = {'observer_received': rb is not None, 'same_frame': ra == rb,
                        'request_id': ra['id'], 'thread_id': thread, 'turn_id': turn,
                        'item_id': ra['params']['itemId'], 'command': ra['params']['command']}
                    c = connect('reconnected')
                    resumed = c.rpc('thread/resume', {'threadId': thread})
                    try:
                        replay = c.event(method, 2)
                    except TimeoutError:
                        replay = None
                    report['cases']['pending_reconnect'] = {'replayed': replay is not None,
                        'same_request': replay == ra, 'resume_succeeded': 'result' in resumed}
                    stale = b.rpc('turn/steer', {'threadId': thread, 'expectedTurnId': 'stale-turn',
                        'input': [{'type': 'text', 'text': 'MUST_NOT_APPEAR'}]})
                    report['cases']['stale_steer'] = stale
                    # Proxy clients stand in for racing viewers; this is NOT a TUI race.
                    a.send({'id': ra['id'], 'result': {'decision': 'decline'}})
                    resolved = [a.event('serverRequest/resolved')]
                    try:
                        resolved.append(b.event('serverRequest/resolved', 2))
                    except TimeoutError:
                        pass
                    report['cases']['resolution'] = resolved
                    b.send({'id': ra['id'], 'result': {'decision': 'accept'}})
                    completed = a.event('item/completed')
                    while completed['params']['item']['type'] != 'commandExecution':
                        completed = a.event('item/completed')
                    report['cases']['competing_answer'] = completed['params']['item']
                    report['cases']['turn_completed'] = a.event('turn/completed')['params']['turn']['status']
                    # No silent retry after transport loss. Observe a new exact-target turn.
                    Provider.pending = tool('exec_command', {'cmd': COMMAND,
                        'sandbox_permissions': 'require_escalated', 'justification': 'Interrupt fixture'})
                    active = a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'INTERRUPT_PROBE'}]})['result']['turn']['id']
                    a.event(method)
                    report['cases']['interrupt_acceptance'] = b.rpc('turn/interrupt', {'threadId': thread, 'turnId': active})
                    report['cases']['interrupt_outcome'] = a.event('turn/completed')['params']['turn']['status']
                    report['cases']['persisted_history'] = a.rpc('thread/read', {'threadId': thread, 'includeTurns': True})
                    # File changes carry their diff on item/started, not the approval.
                    patch = '*** Begin Patch\n*** Add File: '+str(root/'outside.txt')+'\n+SEMON_PATCH_PROBE\n*** End Patch'
                    Provider.pending = tool('apply_patch', {'patch': patch})
                    Provider.pending['call_id'] = 'patch_fixture'
                    a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'PATCH_PROBE'}]})
                    try:
                        file_request = a.event('item/fileChange/requestApproval', 3)
                    except TimeoutError:
                        file_request = None
                    report['cases']['file_request'] = file_request
                    if file_request:
                        a.send({'id': file_request['id'], 'result': {'decision': 'decline'}})
                        a.event('serverRequest/resolved')
                    a.event('turn/completed')
                    report['cases']['patch_not_written'] = not (root/'outside.txt').exists()
                    Provider.pending = tool('request_user_input', {'questions': [{
                        'id': 'choice', 'header': 'Choice', 'question': 'Which fixture?',
                        'options': [{'label': 'One', 'description': 'First'}, {'label': 'Two', 'description': 'Second'}]}]})
                    a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'QUESTION_PROBE'}]})
                    try:
                        question = a.event('item/tool/requestUserInput', 3)
                    except TimeoutError:
                        question = None
                    report['cases']['question_request'] = question
                    if question:
                        a.send({'id': question['id'], 'result': {'answers': {'choice': {'answers': ['One']}}}})
                        a.event('serverRequest/resolved')
                    report['cases']['question_turn'] = a.event('turn/completed')['params']['turn']['status']
                    Provider.pending = tool('exec_command', {'cmd': COMMAND,
                        'sandbox_permissions': 'require_escalated', 'justification': 'Allow once fixture'})
                    Provider.pending['call_id'] = 'allow_fixture'
                    a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'ALLOW_ONCE_PROBE'}]})
                    allow = a.event(method)
                    a.send({'id': allow['id'], 'result': {'decision': 'accept'}})
                    a.event('serverRequest/resolved')
                    done = a.event('item/completed')
                    while done['params']['item']['id'] != 'allow_fixture':
                        done = a.event('item/completed')
                    report['cases']['allow_once_outcome'] = done['params']['item']
                    a.event('turn/completed')
                    # Invalid JSON should not destroy other clients/the daemon.
                    malformed = connect('malformed')
                    malformed.socket.send('{broken')
                    report['cases']['after_malformed'] = b.rpc('thread/read', {'threadId': thread, 'includeTurns': False})
                    Provider.pending = tool('exec_command', {'cmd': COMMAND,
                        'sandbox_permissions': 'require_escalated', 'justification': 'Termination fixture'})
                    Provider.pending['call_id'] = 'termination_fixture'
                    a.rpc('turn/start', {'threadId': thread, 'input': [{'type': 'text', 'text': 'TERMINATION_PROBE'}]})
                    report['cases']['request_at_termination'] = a.event(method)
                    native.terminate()
                    stop(native)
                    report['cases']['server_termination'] = a.wait(lambda x: x.get('probe_error') == 'disconnected')
                finally:
                    for client in clients:
                        client.close()
                    if native.poll() is None:
                        native.terminate()
                        stop(native)
                    try:
                        os.killpg(native.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            report['cases']['synthetic_tool_feedback'] = [item for body in Provider.calls for item in body.get('input', []) if item.get('type') in ('function_call_output', 'custom_tool_call_output')]
            report['probe_completed'] = True
            # Retain exact native identities; normalize disposable paths only.
            report = json.loads(json.dumps(report).replace(str(root), '<FIXTURE>'))
    finally:
        server.shutdown()
        server.server_close()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'version': version, 'cases': list(report['cases']), 'output': str(args.output)}))


if __name__ == '__main__':
    main()
