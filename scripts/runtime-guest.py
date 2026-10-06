#!/usr/bin/env python3
"""One retained Codex launch. Receipts precede spawn; unknown never means resend.

Private control files are outside logs. Inspect uses the supervisor's receipt and
process handle, not provider PID facts. No recovery/replacement is implied.
"""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path('/home/user/.semon-runtime')
CODEX = '/home/user/.local/bin/codex'
SEMON = '/home/user/.local/bin/semon'


def write(name, value):
    target = ROOT / name
    temporary = target.with_suffix('.tmp')
    with open(temporary, 'w') as file:
        os.chmod(temporary, 0o600)
        json.dump(value, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, target)
    fd = os.open(ROOT, os.O_DIRECTORY)
    os.fsync(fd)
    os.close(fd)


def inspect():
    if not (ROOT / 'receipt.json').exists():
        return {'status': 'absent', 'thread': None}
    receipt = json.loads((ROOT / 'receipt.json').read_text())
    # The retained exclusive lock establishes a currently executing supervisor.
    with open(ROOT / 'launch.lock', 'a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            status = ('completed' if receipt.get('exit_code') == 0 else 'agent_failed') if receipt['status'] == 'completed' else 'delivery_unknown'
        except BlockingIOError:
            status = 'running' if receipt['status'] == 'running' else 'starting'
    thread = None
    if (ROOT / 'events.jsonl').exists():
        with open(ROOT / 'events.jsonl') as events:
            for line in events:
                try:
                    event = json.loads(line)
                    if event.get('type') == 'thread.started':
                        thread = event.get('thread_id')
                        break
                except ValueError:
                    pass
    return {'status': status, 'thread': thread}


def launch():
    os.umask(0o077)
    ROOT.mkdir(mode=0o700, exist_ok=True)
    with open(ROOT / 'launch.lock', 'a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        if (ROOT / 'receipt.json').exists():
            return  # Even a dead supervisor must not replay an accepted task.
        task = (ROOT / 'task').read_text()
        write('receipt.json', {'status': 'accepted'})
        environment = {key: value for key, value in os.environ.items()
                       if key not in ('OPENAI_API_KEY', 'CODEX_API_KEY', 'CODEX_ACCESS_TOKEN',
                                      'E2B_API_KEY', 'GH_TOKEN', 'GITHUB_TOKEN')}
        environment['CODEX_HOME'] = '/home/user/.codex'
        with open(ROOT / 'events.jsonl', 'wb') as output, open(ROOT / 'agent-error', 'wb') as errors:
            try:
                child = subprocess.Popen([CODEX, '-c', 'cli_auth_credentials_store="file"',
                                          '-c', 'model_provider="openai"', 'exec', '--json',
                                          '--skip-git-repo-check', '-'],
                                         cwd='/home/user/workspace', env=environment,
                                         stdin=subprocess.PIPE, stdout=output, stderr=errors)
                child.stdin.write(task.encode())
                child.stdin.close()
                write('receipt.json', {'status': 'running'})
                code = child.wait()
                write('receipt.json', {'status': 'completed', 'exit_code': code})
            except Exception:
                write('receipt.json', {'status': 'delivery_unknown'})


if __name__ == '__main__':
    if sys.argv[1:] == ['inspect']:
        print(json.dumps(inspect()))
    elif sys.argv[1:] == ['launch']:
        launch()
