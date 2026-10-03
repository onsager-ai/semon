#!/usr/bin/env python3
"""Pinned isolated Codex fork persistence against a synthetic localhost model."""
import argparse, pathlib, os, json, subprocess, http.server, threading, queue, time, hashlib

class Mock(http.server.BaseHTTPRequestHandler):
    requests = []
    def log_message(self, *_): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.requests.append({'path': self.path, 'model': body.get('model'), 'prior_prompt_present': 'SEMON_SYNTHETIC_FORK_PARENT' in json.dumps(body.get('input')), 'prior_reply_present': 'SYNTHETIC_ACK' in json.dumps(body.get('input'))})
        rid, mid = f'fixture-response-{len(self.requests)}', f'fixture-message-{len(self.requests)}'
        message = {'id':mid, 'type':'message', 'role':'assistant', 'status':'completed', 'content':[{'type':'output_text','text':'SYNTHETIC_ACK','annotations':[]}]}
        events = [
            {'type':'response.created','response':{'id':rid,'status':'in_progress','output':[]}},
            {'type':'response.output_item.added','output_index':0,'item':dict(message,status='in_progress',content=[])},
            {'type':'response.content_part.added','item_id':mid,'output_index':0,'content_index':0,'part':{'type':'output_text','text':'','annotations':[]}},
            {'type':'response.output_text.delta','item_id':mid,'output_index':0,'content_index':0,'delta':'SYNTHETIC_ACK'},
            {'type':'response.output_text.done','item_id':mid,'output_index':0,'content_index':0,'text':'SYNTHETIC_ACK'},
            {'type':'response.output_item.done','output_index':0,'item':message},
            {'type':'response.completed','response':{'id':rid,'status':'completed','output':[message],'usage':{'input_tokens':5,'output_tokens':3,'total_tokens':8}}},
        ]
        data = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)


parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--codex',type=pathlib.Path,required=True)
parser.add_argument('--output',type=pathlib.Path,required=True)
args=parser.parse_args()
probe_sha256=hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
root=args.output.resolve();root.mkdir(mode=0o700,exist_ok=False)
for name in ('home','work','tmp'): (root/name).mkdir(mode=0o700)
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Mock);threading.Thread(target=server.serve_forever,daemon=True).start()
env={key:os.environ[key] for key in ('PATH','LANG','LC_ALL','LC_CTYPE','TZ','TERM') if key in os.environ}
env.update(CODEX_HOME=str(root/'home'),TMPDIR=str(root/'tmp'))
(root/'home/config.toml').write_text(f'''model = "mock-model"
model_provider = "synthetic"
[analytics]
enabled = false
[feedback]
enabled = false
[model_providers.synthetic]
name = "Synthetic localhost"
base_url = "http://127.0.0.1:{server.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
''')
cli=str(args.codex.resolve(strict=True))
proc=None
try:
 version=subprocess.check_output([cli,'--version'],env=env,text=True).strip()
 if version != 'codex-cli 0.159.0-alpha.3': raise RuntimeError('Unexpected pinned version: '+version)
 initial=subprocess.run([cli,'exec','--skip-git-repo-check','--ignore-rules','--json','-C',str(root/'work'),'SEMON_SYNTHETIC_FORK_PARENT'],env=env,cwd=root/'work',input='',text=True,capture_output=True,timeout=60)
 (root/'initial.stdout').write_text(initial.stdout);(root/'initial.stderr').write_text(initial.stderr);initial.check_returncode()
 sources=list((root/'home/sessions').rglob('*.jsonl'))
 if len(sources)!=1 or len(Mock.requests)!=1: raise RuntimeError('Unexpected initial source/provider count')
 parent=sources[0];sid=json.loads(parent.read_text().splitlines()[0])['payload']['id'];initial_bytes=parent.read_bytes();(root/'initial-rollout.jsonl').write_bytes(initial_bytes)
 proc=subprocess.Popen([cli,'app-server','--listen','stdio://'],env=env,cwd=root/'work',stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=open(root/'app-server.stderr','w'),text=True,bufsize=1)
 incoming=queue.Queue();threading.Thread(target=lambda:[incoming.put(line) for line in proc.stdout],daemon=True).start();observed=[]
 def rpc(identifier,method,params):
  proc.stdin.write(json.dumps({'id':identifier,'method':method,'params':params})+'\n');proc.stdin.flush();deadline=time.monotonic()+45
  while time.monotonic()<deadline:
   try: line=incoming.get(timeout=1)
   except queue.Empty:
    if proc.poll() is not None:raise RuntimeError('App-server exited')
    continue
   value=json.loads(line);observed.append(value)
   if value.get('id')==identifier:
    if 'error' in value:raise RuntimeError(json.dumps(value['error']))
    return value['result']
  raise RuntimeError('RPC timeout '+method)
 rpc(1,'initialize',{'clientInfo':{'name':'semon-synthetic-probe','version':'1'},'capabilities':{'experimentalApi':True}})
 proc.stdin.write(json.dumps({'method':'initialized','params':{}})+'\n');proc.stdin.flush()
 fork=rpc(2,'thread/fork',{'threadId':sid,'excludeTurns':True})
 child_id=fork['thread']['id'];deadline=time.monotonic()+10
 while time.monotonic()<deadline:
  children=[p for p in (root/'home/sessions').rglob('*.jsonl') if p!=parent]
  if children:break
  time.sleep(.1)
 if len(children)!=1:raise RuntimeError('Expected one child rollout')
 child=children[0];child_bytes=child.read_bytes();(root/'forked-rollout.jsonl').write_bytes(child_bytes)
 if parent.read_bytes()!=initial_bytes:raise RuntimeError('Fork mutated parent')
 if len(Mock.requests)!=1:raise RuntimeError('Fork made a model request')
 meta=json.loads(child_bytes.splitlines()[0])['payload']
 if child_id==sid or meta.get('id')!=child_id or meta.get('forked_from_id')!=sid:
  raise RuntimeError('Unexpected persisted logical fork identities')
 base=meta.get('history_base',{})
 if base.get('thread_id')!=sid or base.get('end_byte_offset')!=len(initial_bytes) or base.get('end_ordinal_exclusive')!=len(initial_bytes.splitlines()):
  raise RuntimeError('Unexpected persisted physical history prefix')
 if meta.get('forked_from_ordinal_exclusive')!=len(initial_bytes.splitlines()):
  raise RuntimeError('Unexpected logical fork boundary')
 fork_request_count=len(Mock.requests)
 started=rpc(3,'turn/start',{'threadId':child_id,'input':[{'type':'text','text':'SEMON_SYNTHETIC_FORK_CHILD','text_elements':[]}]})
 deadline=time.monotonic()+45
 completed=False
 while time.monotonic()<deadline:
  try:line=incoming.get(timeout=1)
  except queue.Empty:
   if proc.poll() is not None:raise RuntimeError('App-server exited during child turn')
   continue
  value=json.loads(line);observed.append(value)
  if value.get('method')=='turn/completed' and value.get('params',{}).get('threadId')==child_id:
   completed=True;break
 if not completed:raise RuntimeError('Missing child turn completion')
 own_turn_id=started['turn']['id']
 deadline=time.monotonic()+10
 while time.monotonic()<deadline:
  resumed=child.read_bytes()
  if resumed.endswith(b'\n'):
   records=[json.loads(line) for line in resumed.splitlines()]
   if any(row.get('type')=='event_msg' and row.get('payload',{}).get('type')=='task_complete' and row['payload'].get('turn_id')==own_turn_id for row in records):break
  time.sleep(.1)
 else:raise RuntimeError('Missing durable own child turn completion')
 if not resumed.startswith(child_bytes):raise RuntimeError('Child turn mutated fork prefix')
 if parent.read_bytes()!=initial_bytes:raise RuntimeError('Child turn mutated parent')
 if len(Mock.requests)!=2 or not Mock.requests[-1]['prior_prompt_present'] or not Mock.requests[-1]['prior_reply_present']:
  raise RuntimeError('Expected one child mock request with inherited context')
 (root/'child-turn-rollout.jsonl').write_bytes(resumed)
 (root/'rpc-private.json').write_text(json.dumps(observed,indent=2)+'\n')
 report={'version':version,'probe_sha256':probe_sha256,'sources':{name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ('initial-rollout.jsonl','forked-rollout.jsonl','child-turn-rollout.jsonl')},'binary_sha256':hashlib.sha256(pathlib.Path(cli).read_bytes()).hexdigest(),'parent':sid,'child':child_id,'child_meta_fields':sorted(meta),'forked_from_id':meta.get('forked_from_id'),'history_base':meta.get('history_base'),'forked_from_ordinal_exclusive':meta.get('forked_from_ordinal_exclusive'),'fork_request_count':fork_request_count,'request_count':len(Mock.requests),'live_provider_observations':Mock.requests,'parent_unchanged':True,'child_record_types':[json.loads(line)['type'] for line in child_bytes.splitlines()]}
 (root/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
finally:
 if proc is not None:
  proc.terminate()
  try:proc.wait(timeout=5)
  except subprocess.TimeoutExpired:proc.kill();proc.wait()
 server.shutdown();server.server_close()
