#!/usr/bin/env python3
"""Explicit E2B provider/guest actions through one private JSON stdio request.

The host owns authorization, credentials, call claims and reconciliation. This
worker never retries create or task dispatch. Inventory remains read-only in the
separate inventory driver. A lost reply is unknown, never proof of absence.
"""
import importlib.metadata
import json
import logging
from pathlib import Path
import re
import sys

ROOT = '/home/user/.semon-runtime'
BIN = '/home/user/.local/bin'
UNKNOWN = {'status': 'unknown', 'runtime': None, 'thread': None}
IDENTIFIER = re.compile(r'[A-Za-z0-9_.-]{1,128}\Z')


def execute(request, sandbox_class):
    if (set(request) != {'method', 'api_key', 'runtime', 'labels', 'payload'}
            or request['method'] not in ('create', 'bootstrap', 'dispatch', 'inspect', 'inspect_bootstrap', 'qualify_profile')
            or not isinstance(request['api_key'], str) or not 1 <= len(request['api_key']) <= 4096
            or not isinstance(request['labels'], dict)
            or len(request['labels']) != 7
            or any(not IDENTIFIER.fullmatch(value) or value in ('.', '..')
                   for value in request['labels'].values())):
        return UNKNOWN
    key = request['api_key']
    method = request['method']
    payload = request['payload']
    if method == 'qualify_profile':
        if request['runtime'] is not None or payload != {}:
            return UNKNOWN
        box = sandbox_class.create(template='base', metadata=request['labels'], secure=True,
                                   timeout=300, lifecycle={'on_timeout': {'action': 'pause', 'keep_memory': True}},
                                   api_key=key, request_timeout=30)
        runtime = box.sandbox_id
        try:
            info = sandbox_class.get_info(runtime, api_key=key, request_timeout=10)
            if any(info.metadata.get(name) != value for name, value in request['labels'].items()):
                return UNKNOWN
            box.commands.run(f'mkdir -p {BIN} {ROOT}', timeout=10)
            for name in ('codex', 'semon'):
                with open('/usr/local/bin/' + name, 'rb') as binary:
                    box.files.write(BIN + '/' + name, binary)
            box.commands.run(f'chmod 700 {BIN}/codex {BIN}/semon', timeout=10)
            if box.commands.run(BIN + '/codex --version', timeout=10).stdout.strip() != 'codex-cli 0.159.0-alpha.3':
                return UNKNOWN
            box.commands.run(BIN + '/semon --help', timeout=10)
            box.files.write(ROOT + '/retained', 'provider-profile-v1')
            box.pause(keep_memory=False)
            # Explicit qualification action only; runtime inspection never resumes.
            box = sandbox_class.connect(runtime, api_key=key, timeout=300, on_resume='reboot', request_timeout=30)
            if box.files.read(ROOT + '/retained') != 'provider-profile-v1':
                return UNKNOWN
            return {'status': 'profile_qualified', 'runtime': runtime, 'thread': None}
        finally:
            # Only this throwaway qualification resource, with no accepted user
            # task/recovery data, is eligible for cleanup. Never clean user launches.
            info = sandbox_class.get_info(runtime, api_key=key, request_timeout=10)
            if all(info.metadata.get(name) == value for name, value in request['labels'].items()):
                box.kill()
    if method == 'create':
        if request['runtime'] is not None or payload != {}:
            return UNKNOWN
        # Qualified host policy must explicitly enable this fixed bounded profile.
        box = sandbox_class.create(template='base', metadata=request['labels'], secure=True,
                                   timeout=3600, lifecycle={'on_timeout': {'action': 'pause',
                                                                          'keep_memory': True}},
                                   api_key=key, request_timeout=30)
        return {'status': 'created', 'runtime': box.sandbox_id, 'thread': None}
    runtime = request['runtime']
    if not isinstance(runtime, str) or not IDENTIFIER.fullmatch(runtime) or runtime in ('.', '..'):
        return UNKNOWN
    # Constructor attaches without connect(), which could silently resume compute.
    info = sandbox_class.get_info(runtime, api_key=key, request_timeout=10)
    if (any(info.metadata.get(name) != value for name, value in request['labels'].items())
            or getattr(info.state, 'value', info.state) != 'running'):
        return UNKNOWN
    box = sandbox_class(runtime, api_key=key, request_timeout=10)
    if method == 'inspect_bootstrap':
        result = box.commands.run(f'test -f {ROOT}/bootstrapped && echo bootstrapped', timeout=10)
        return {'status': 'bootstrapped' if result.stdout.strip() == 'bootstrapped' else 'unknown', 'runtime': runtime, 'thread': None}
    if method == 'bootstrap':
        if set(payload) != {'push_url', 'push_token', 'auth'}:
            return UNKNOWN
        box.commands.run(f'mkdir -p {ROOT} {BIN} /home/user/workspace /home/user/.codex; chmod 700 {ROOT} /home/user/.codex', timeout=10)
        for name in ('codex', 'semon'):
            with open('/usr/local/bin/' + name, 'rb') as binary:
                box.files.write(BIN + '/' + name, binary)
        box.files.write(ROOT + '/guest.py', Path(__file__).with_name('runtime-guest.py').read_text())
        box.files.write(ROOT + '/push-token', payload['push_token'])
        # Model auth never joins the mirrored session paths or Push records.
        box.files.write('/home/user/.codex/auth.json', json.dumps(payload['auth']))
        box.commands.run(f'chmod 700 {BIN}/codex {BIN}/semon; chmod 600 {ROOT}/push-token /home/user/.codex/auth.json', timeout=10)
        version = box.commands.run(BIN + '/codex --version', timeout=10)
        if version.stdout.strip() != 'codex-cli 0.159.0-alpha.3':
            return UNKNOWN
        # URL is data supplied by the embedding, never interpolated into a shell.
        box.files.write(ROOT + '/push-config.json', json.dumps({'url': payload['push_url']}))
        box.files.write(ROOT + '/push.py', "import fcntl,json,subprocess\nwith open('push.lock','a') as lock:\n try: fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)\n except BlockingIOError: raise SystemExit(0)\n subprocess.run(['/home/user/.local/bin/semon','push','--to',json.load(open('push-config.json'))['url'],'--token-file','push-token','--codex-home','/home/user/.codex','--watch'])\n")
        box.commands.run(f'python3 {ROOT}/push.py >{ROOT}/push-error 2>&1', cwd=ROOT, background=True, timeout=0)
        box.commands.run(f'touch {ROOT}/bootstrapped; sync', timeout=10)
        return {'status': 'bootstrapped', 'runtime': runtime, 'thread': None}
    if method == 'dispatch':
        if set(payload) != {'task'} or not isinstance(payload['task'], str) or not 1 <= len(payload['task'].encode()) <= 65536:
            return UNKNOWN
        box.files.write(ROOT + '/task', payload['task'])
        box.commands.run(f'python3 {ROOT}/guest.py launch >{ROOT}/supervisor-error 2>&1', background=True, timeout=0)
    result = box.commands.run(f'python3 {ROOT}/guest.py inspect', timeout=10)
    receipt = json.loads(result.stdout)
    if receipt['status'] not in ('absent', 'starting', 'running', 'completed', 'delivery_unknown'):
        return UNKNOWN
    return {'status': receipt['status'], 'runtime': runtime, 'thread': receipt['thread']}


def main():
    logging.disable(logging.CRITICAL)
    result = UNKNOWN
    try:
        data = sys.stdin.buffer.read(262145)
        if len(data) > 262144 or importlib.metadata.version('e2b') != '2.52.0':
            raise ValueError()
        from e2b import Sandbox
        result = execute(json.loads(data), Sandbox)
    except Exception:
        pass
    print(json.dumps(result, separators=(',', ':')))


if __name__ == '__main__':
    main()
