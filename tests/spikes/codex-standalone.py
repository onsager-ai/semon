#!/usr/bin/env python3
"""Bounded native LocalSession regression; synthetic Responses provider, no paid model.

Private bootstrap is stdin. A fresh owner home/workspace and one stable operation
are used. Never rerun an interrupted operation: retain its state for inspection.
"""
import argparse
import http.server
import hashlib
import importlib.util
import json
import os
import signal
from pathlib import Path
import subprocess
import tempfile
import threading
import time

spec = importlib.util.spec_from_file_location('native_probe', Path(__file__).with_name('codex-control.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
MARKER = 'SEMON_EXECUTED_307:42'
COMMAND = 'test -z "${OPENAI_API_KEY+x}" && test -z "${SEMON_PRIVATE_CANARY+x}" && test ! -e /home/agent/.codex/config.toml && printf "SEMON_EXECUTED_307:%s" "$((19+23))"'


def outbound_tools(body):
    return body.get('tools') or next(row['tools'] for row in body['input'] if row.get('type') == 'additional_tools')


def tool_names(tools):
    names = []
    for tool in tools:
        if tool.get('type') == 'namespace':
            names.extend(tool_names(tool.get('tools', [])))
        elif 'name' in tool:
            names.append(tool['name'])
    return names


def tool_specs(tools):
    for tool in tools:
        if tool.get('type') == 'namespace':
            yield from tool_specs(tool.get('tools', []))
        else:
            yield tool


def native_processes(binary):
    """Only the exclusively installed fixture binary; never read process env/argv."""
    identity = Path(binary).stat()
    found = []
    for proc in Path('/proc').glob('[0-9]*'):
        try:
            executable = (proc/'exe').stat()
            if (executable.st_dev, executable.st_ino) == (identity.st_dev, identity.st_ino):
                found.append(int(proc.name))
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            pass
    return found


class Provider(m.Provider):
    calls = []
    turn_calls = []
    failures = []
    credential = 'synthetic-private-307-key'
    baseline = False

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            self.calls.append(body)
            assert self.headers.get('Authorization') == 'Bearer ' + self.credential
            assert self.credential not in json.dumps(body)
            title = any('User prompt:' in str(row.get('content', '')) and 'title' in str(row.get('content', '')).lower() for row in body.get('input', []))
            if not title:
                self.turn_calls.append(body)
                text = json.dumps(body)
                names = tool_names(outbound_tools(body))
                outputs = [row for row in body['input'] if row.get('type') in ('function_call_output', 'custom_tool_call_output')]
                if not self.baseline:
                    assert 'functions.exec' not in text
                    assert '<multi_agent_role>' not in text
                    assert 'exec_command' in names, names
                    assert 'exec' not in names, names
                    assert not any('spawn_agent' in name for name in names), names
                if self.baseline:
                    assert 'functions.exec' in text
                    assert '<multi_agent_role>' in text
                    assert 'exec' in names, names
                    if not outputs:
                        item = {'type':'custom_tool_call','id':'ct_fixture','call_id':'call_fixture','name':'exec','input':'text(await tools.exec_command({cmd:"printf SHOULD_NOT_EXECUTE"}));'}
                    else:
                        assert 'code-mode host is disabled' in str(outputs)
                        item = {'type':'message','id':'msg_fixture','role':'assistant','status':'completed','content':[{'type':'output_text','text':'CODE_MODE_HOST_DISABLED','annotations':[]}]}
                elif not outputs:
                    item = m.tool('exec_command', {'cmd': COMMAND, 'yield_time_ms': 1000, 'max_output_tokens': 1000, 'sandbox_permissions': 'require_escalated', 'justification': 'Bounded authorized synthetic qualification'})
                else:
                    assert any(MARKER in str(row['output']) for row in outputs)
                    item = {'type': 'message', 'id': 'msg_fixture', 'role': 'assistant', 'status': 'completed', 'content': [{'type': 'output_text', 'text': MARKER, 'annotations': []}]}
            else:
                item = {'type':'message','id':'msg_title','role':'assistant','status':'completed','content':[{'type':'output_text','text':'Synthetic qualification','annotations':[]}]}
            events = [
                {'type':'response.created','response':{'id':'resp_fixture','status':'in_progress','output':[]}},
                {'type':'response.output_item.added','output_index':0,'item':item},
                {'type':'response.output_item.done','output_index':0,'item':item},
                {'type':'response.completed','response':{'id':'resp_fixture','status':'completed','output':[item],'usage':{'input_tokens':5,'output_tokens':3,'total_tokens':8}}},
            ]
            data = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
            self.send_response(200)
            self.send_header('Content-Type','text/event-stream')
            self.send_header('Content-Length',str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except Exception as error:
            self.failures.append(type(error).__name__ + ': ' + str(error))
            self.send_error(400)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', required=True)
    parser.add_argument('--driver', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--operation', required=True, help='Persisted fresh 32-hex operation identity; never replay an interrupted operation')
    parser.add_argument('--baseline', action='store_true', help='Use only with a driver built from unmodified 52d177c; expect the disabled-host failure')
    args = parser.parse_args()
    operation = args.operation
    assert len(operation) == 32 and all(c in '0123456789abcdef' for c in operation)
    approval_operation = hashlib.sha256(('approval:'+operation).encode()).hexdigest()[:32]
    Provider.baseline = args.baseline
    assert not native_processes(args.codex), 'fixture package already has live processes; reconcile first'
    provider = http.server.ThreadingHTTPServer(('127.0.0.1',0), Provider)
    threading.Thread(target=provider.serve_forever,daemon=True).start()
    # /tmp is an executor tmpfs: its host paths cannot be the qualified workspace.
    root = Path(tempfile.mkdtemp(prefix='standalone-307-', dir=Path.home()))
    root.chmod(0o700)
    work = root/'work'
    work.mkdir()
    state = root/'owner'
    config = root/'model.toml'
    config.write_text(f'model="openai/gpt-6-luna"\nmodel_provider="synthetic"\n[model_providers.synthetic]\nname="Synthetic OpenRouter route"\nbase_url="http://127.0.0.1:{provider.server_port}/v1"\nwire_api="responses"\nenv_key="OPENAI_API_KEY"\nrequires_openai_auth=false\nsupports_websockets=false\n')
    private = {'binary':args.codex,'workspace':str(work),'state':str(state),'config':str(config),'api_key':Provider.credential,'operation':operation,'approval_operation':approval_operation,'expected_command':"/usr/bin/bash -lc '"+COMMAND.replace("'","'\"'\"'")+"'",'text':'Run the bounded shell check and return its exact output.'}
    env = dict(os.environ, SEMON_PRIVATE_CANARY='private-controller-only')
    try:
        process = subprocess.Popen([args.driver],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=env,start_new_session=True)
        try:
            stdout, stderr = process.communicate(json.dumps(private), timeout=70)
        except subprocess.TimeoutExpired:
            # Kill only this fixture's process group; leave state for reconciliation.
            # A forced timeout is never successful shutdown or permission to replay.
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
        assert not Provider.failures, Provider.failures[:3]
        assert process.returncode == 0, stderr
        report = json.loads(stdout)
        assert report['shutdown_reaped'] and report['duplicate_prevented']
        deadline = time.monotonic() + 2
        while native_processes(args.codex) and time.monotonic() < deadline:
            time.sleep(0.025)
        assert not native_processes(args.codex), 'native controller/executor shutdown unknown'
        assert report['snapshot']['lastTurn']['status'] == 'completed', report
        rollouts = list((state/'home'/'sessions').rglob('*.jsonl'))
        assert len(rollouts) == 1
        records = [json.loads(line) for line in rollouts[0].read_text().splitlines()]
        items = [row['payload'] for row in records if row['type'] == 'response_item']
        calls = [row for row in items if row['type'] in ('function_call', 'custom_tool_call')]
        assert len(calls) == 1 and calls[0]['name'] == ('exec' if args.baseline else 'exec_command'), calls
        outputs = [row for row in items if row['type'] in ('function_call_output', 'custom_tool_call_output')]
        if args.baseline:
            assert 'code-mode host is disabled' in str(outputs), outputs
        else:
            assert any(MARKER in str(row.get('output','')) and 'Process exited with code 0' in str(row.get('output','')) for row in outputs), outputs
        finals = [row for row in items if row['type'] == 'message' and row.get('role') == 'assistant']
        assert finals[-1]['content'][0]['text'] == ('CODE_MODE_HOST_DISABLED' if args.baseline else MARKER), finals
        for path in root.rglob('*'):
            if path.is_file() and not path.is_symlink():
                assert Provider.credential.encode() not in path.read_bytes(), 'credential in file: '+path.name
        assert Provider.credential not in stdout + stderr
        assert len(Provider.turn_calls) == 2, len(Provider.turn_calls)
        report = {'version':m.PIN,'model':'openai/gpt-6-luna','provider':'synthetic localhost Responses; no live model qualification','operation':operation,'tool_names':tool_names(outbound_tools(Provider.turn_calls[0])),'standalone_instructions':True,'command':COMMAND,'shell_output':MARKER,'shell_exit':0,'final_output':MARKER,'credential_header_only':True,'credential_excluded_from_executor_and_files':True,'duplicate_prevented':True,'shutdown_reaped':True}
        executor = next(tool for tool in tool_specs(outbound_tools(Provider.turn_calls[0])) if tool.get('name') == ('exec' if args.baseline else 'exec_command'))
        report['executor_schema'] = {key: executor[key] for key in ('type', 'name', 'parameters', 'format') if key in executor}
        report['controller_and_executor_processes_absent'] = True
        if args.baseline:
            report.update(standalone_instructions=False, command='printf SHOULD_NOT_EXECUTE', shell_output=None, shell_exit=None, final_output='CODE_MODE_HOST_DISABLED', credential_excluded_from_executor_and_files=False, runtime_error='code-mode host is disabled')
        args.output.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report))
    finally:
        provider.shutdown()
        # Retain disposable state for independent reconciliation on failure.
        print('Private qualification state retained at '+str(root),file=__import__('sys').stderr)

if __name__ == '__main__':
    main()
