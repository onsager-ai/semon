#!/usr/bin/env python3
"""Resume mechanics against an isolated localhost provider; no credentials or real model."""
import argparse, hashlib, http.server, json, os, pathlib, shutil, sqlite3, subprocess, tempfile, threading, uuid

class Mock(http.server.BaseHTTPRequestHandler):
    requests = []
    def log_message(self, *_): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.requests.append(body)
        rid, mid = 'resp_' + uuid.uuid4().hex, 'msg_' + uuid.uuid4().hex
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

def main():
    ap = argparse.ArgumentParser(); ap.add_argument('--output', default='codex-resume-spike.json'); args = ap.parse_args()
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Mock); threading.Thread(target=server.serve_forever,daemon=True).start()
    report = {'codex_version':subprocess.check_output(['codex','--version'],text=True).strip(),'provider':'isolated localhost synthetic SSE','cases':[]}
    with tempfile.TemporaryDirectory(prefix='semon-codex-resume-') as tmp:
        base = pathlib.Path(tmp); cwd=base/'work'; cwd.mkdir()
        config = f'''model = "mock-model"\nmodel_provider = "synthetic"\n[model_providers.synthetic]\nname = "Synthetic localhost"\nbase_url = "http://127.0.0.1:{server.server_port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\n'''
        def run(home, resume=None):
            home.mkdir(exist_ok=True); (home/'config.toml').write_text(config)
            env = {k:v for k,v in os.environ.items() if not any(x in k for x in ['API_KEY','TOKEN','AUTH','CODEX_HOME','OPENAI','AZURE'])}; env['CODEX_HOME']=str(home)
            cmd=['codex','exec','--skip-git-repo-check','--ignore-rules','--json','-C',str(cwd)]
            if resume: cmd += ['resume',resume,'SEMON_RESUME_PROBE']
            else: cmd += ['SEMON_SYNTHETIC_PRIOR_TURN_7d902b']
            before=len(Mock.requests)
            p=subprocess.run(cmd,env=env,capture_output=True,text=True,input="",timeout=45)
            requests=Mock.requests[before:]
            database_rows={}
            for db in home.glob('*.sqlite'):
                with sqlite3.connect(db) as conn:
                    database_rows[db.name]={table:conn.execute('SELECT count(*) FROM '+ '"'+table.replace('"','""')+'"').fetchone()[0] for (table,) in conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")}
            return {'database_rows':database_rows,'exit':p.returncode,'request_count':len(requests),'prior_turn_replayed':any('SEMON_SYNTHETIC_PRIOR_TURN_7d902b' in json.dumps(r.get('input')) for r in requests),'prior_reply_replayed':any('SYNTHETIC_ACK' in json.dumps(r.get('input')) for r in requests),'events':[json.loads(x).get('type') for x in p.stdout.splitlines() if x.startswith('{')],'stderr':p.stderr[-2500:],'databases':sorted(x.name for x in home.glob('*.sqlite'))}
        source=base/'source'; initial=run(source); report['initial']=initial
        rollouts=list(source.glob('sessions/**/*.jsonl'))
        if initial['exit'] or len(rollouts)!=1: raise RuntimeError(json.dumps(report))
        rollout=rollouts[0]; meta=json.loads(rollout.read_text().splitlines()[0])['payload']; sid=meta['id']; report['session_meta_keys']=sorted(meta); report['history_mode']=meta.get('history_mode')
        for case in ['rollout_only','rollout_plus_databases','rollout_plus_writer_lock']:
            home=base/case; dest=home/rollout.relative_to(source); dest.parent.mkdir(parents=True); shutil.copyfile(rollout,dest)
            if case=='rollout_plus_databases':
                for db in source.glob('*.sqlite'):
                    with sqlite3.connect(db) as src, sqlite3.connect(home/db.name) as dst: src.backup(dst)
            if case=='rollout_plus_writer_lock':
                import fcntl
                locks=home/'thread-writer-locks'; locks.mkdir(); lock=open(locks/(sid+'.lock'),'w'); fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
            before=hashlib.sha256(dest.read_bytes()).hexdigest()
            # A copied state index can retain an absolute source path. Hide it during resume
            # so success cannot accidentally use the donor's still-accessible rollout.
            hidden=base/'donor-rollout-hidden.jsonl'; rollout.rename(hidden)
            try: result=run(home,sid)
            finally: hidden.rename(rollout)
            result.update(case=case,rollout_changed=before!=hashlib.sha256(dest.read_bytes()).hexdigest(),local_rollouts=len(list(home.glob('sessions/**/*.jsonl'))))
            report['cases'].append(result)
            if case=='rollout_plus_writer_lock': lock.close()
    server.shutdown();
    report['checks_passed'] = (report['initial']['exit']==0 and report['cases'][0]['exit']==0 and report['cases'][0]['prior_turn_replayed'] and report['cases'][0]['prior_reply_replayed'] and report['cases'][1]['exit']!=0 and report['cases'][1]['request_count']==0 and report['cases'][2]['exit']!=0 and report['cases'][2]['request_count']==0)
    pathlib.Path(args.output).write_text(json.dumps(report,indent=2)+'\n'); print(json.dumps(report,indent=2))
    if not report['checks_passed']: raise SystemExit(1)

if __name__=='__main__': main()
