#!/usr/bin/env python3
"""Synthetic native Codex over an exclusively created OpenSSH fixture.
No paid provider, real model credential or user server. Never replay a lost launch.
"""
import argparse, hashlib, http.server, importlib.util, json, os
from pathlib import Path
import shutil, signal, subprocess, tempfile, threading, time, uuid

spec=importlib.util.spec_from_file_location('standalone',Path(__file__).with_name('codex-standalone.py'))
standalone=importlib.util.module_from_spec(spec);spec.loader.exec_module(standalone)
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--installer',required=True)
parser.add_argument('--controller',required=True)
parser.add_argument('--codex',required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
mirror_binary=os.environ["SEMON_SSH_BINARY"]
Provider=standalone.Provider
catalog=json.loads(subprocess.check_output([args.codex,'debug','models','--bundled'],env={}))
Provider.native_instructions=next(model['model_messages']['instructions_template'] for model in catalog['models'] if model['slug']=='gpt-6-luna')
provider=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
threading.Thread(target=provider.serve_forever,daemon=True).start()
work=Path(tempfile.mkdtemp(prefix='semon-native-ssh-'))
receiver_dir=work/"receiver"
receiver_dir.mkdir(mode=0o700)
mirror_token=subprocess.check_output([mirror_binary,"receive","token","add","native-307","--dir",str(receiver_dir)]).decode().strip()
port_socket=__import__("socket").socket();port_socket.bind(("127.0.0.1",0));receiver_port=port_socket.getsockname()[1];port_socket.close()
receiver=subprocess.Popen([mirror_binary,"receive","--dir",str(receiver_dir),"--listen","127.0.0.1:"+str(receiver_port)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
known=work/'known_hosts'
known.write_bytes(subprocess.check_output(['ssh-keyscan','-T','2','-t','ed25519','-p',os.environ['SEMON_SSH_PORT'],'127.0.0.1'],stderr=subprocess.DEVNULL))
ssh=['ssh','-T','-i',os.environ['SEMON_SSH_KEY_FILE'],'-p',os.environ['SEMON_SSH_PORT'],'-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(known),'fixture@127.0.0.1']
def remote(script,input=None):
    command="python3 -c '"+script.replace("'","'\\''")+"'"
    result=subprocess.run(ssh+[command],input=None if input is None else json.dumps(input),capture_output=True,text=True,timeout=40)
    if result.returncode: raise AssertionError('private fixture command refused: '+str(result.returncode))
    return json.loads(result.stdout) if result.stdout else None

def rpc(root,value):
    return remote("import json,socket,sys\np=json.load(sys.stdin)\ns=socket.socket(socket.AF_UNIX);s.settimeout(20);s.connect(p['root']+'/control.sock');s.sendall(json.dumps(p['value']).encode());s.shutdown(socket.SHUT_WR);out=b''\nwhile True:\n b=s.recv(65536)\n if not b: break\n out+=b\nprint(out.decode())",{'root':root,'value':value})

forward=subprocess.Popen(ssh[:-1]+['-o','ExitOnForwardFailure=yes','-N','-R','127.0.0.1:33221:127.0.0.1:'+str(provider.server_port),'-R','127.0.0.1:33222:127.0.0.1:'+str(receiver_port),ssh[-1]],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
report={'native_version':subprocess.check_output([args.codex,'--version'],env={}).decode().strip(),'package_sha256':'4fcc47ab57f52ff75363951a8761146cd10c8288bd86fed45487dbb204a16b71','controller_sha256':hashlib.sha256(Path(args.controller).read_bytes()).hexdigest(),'pairing':'Codex0.160.0 / OpenSSH Linux namespace executor'}
try:
    time.sleep(.2)
    assert forward.poll() is None,'synthetic provider forwarding failed'
    enrollment=str(uuid.uuid4());launch=uuid.uuid4().hex
    root='/home/fixture/.local/state/semon-ssh/'+enrollment
    remote("import os,pathlib,sys,json\nos.umask(0o077)\np=pathlib.Path(json.load(sys.stdin)['root']);p.mkdir(parents=True,mode=0o700)\nfor a in (p,p.parent,p.parent.parent,p.parent.parent.parent): a.chmod(0o700)",{'root':root})
    config='model="openai/gpt-6-luna"\nmodel_provider="synthetic"\n[model_providers.synthetic]\nname="Synthetic"\nbase_url="http://127.0.0.1:33221/v1"\nwire_api="responses"\nenv_key="OPENAI_API_KEY"\nrequires_openai_auth=false\nsupports_websockets=false\n'
    bootstrap={'controller':args.controller,'enrollment':enrollment,'launch':launch,'expires':int(time.time())+150,'config':config}
    result=subprocess.run([args.installer],input=json.dumps(bootstrap),text=True,capture_output=True,timeout=45)
    assert result.returncode==0,result.stderr
    accepted=json.loads(result.stdout)
    if accepted['state']!='running':
        diagnostic=remote("import pathlib,json,sys\np=pathlib.Path(json.load(sys.stdin)['root']);text=(p/'controller.log').read_text();assert 'synthetic-private-307-key' not in text;print(json.dumps({'controller':text[-4000:],'ended':(p/'ended.json').exists()}))",{'root':root})
        raise AssertionError({'launch':accepted,'diagnostic':diagnostic})
    duplicate=subprocess.run([args.installer],input=json.dumps(bootstrap),text=True,capture_output=True,timeout=45)
    assert duplicate.returncode==0 and json.loads(duplicate.stdout)==accepted,'duplicate launch changed thread'
    thread=accepted['thread'];snap=rpc(root,{'op':'snapshot'})['snapshot']
    scope={'enrollment':enrollment,'authority':str(uuid.uuid4()),'owner':'owner-307','workspace':'workspace-307','connection':'ssh-307','session':'session-307','operation':launch,'epoch':1,'thread':thread,'model_connection':'codex-307','expires':bootstrap['expires']}
    broker=Path(__file__).parents[2].joinpath('crates/semon-ssh/src/execution.py').read_text()
    def exchange(method,id,payload=None):
        return remote(broker,{'version':1,'scope':scope,'method':method,'id':id,'payload':payload})
    id=hashlib.sha256(('ssh-dispatch-v1/session-307/1/'+launch).encode()).hexdigest()[:32]
    assert exchange('authorize',id)['outcome']=='accepted'
    payload={'generation':snap['generation'],'text':'Run the bounded shell check and return its exact output.'}
    receipt=exchange('dispatch',id,payload); assert receipt['outcome']=='accepted',receipt
    deadline=time.monotonic()+35;answered=False
    while time.monotonic()<deadline:
        snap=rpc(root,{'op':'snapshot'})['snapshot']
        requests=[row for row in snap['requests'] if row['state']['state']=='open']
        if requests and not answered:
            row=requests[0]
            expected="/usr/bin/bash -lc '"+standalone.COMMAND.replace("'","'\"'\"'")+"'"
            assert row['payload']['method']=='item/commandExecution/requestApproval' and row['payload']['params']['command']==expected,'unexpected native permission request'
            answer={'id':uuid.uuid4().hex,'op':'answer','request':row['id'],'hash':row['hash'],'answer':{'decision':'allow'},'thread':thread,'generation':snap['generation'],'activeTurn':snap['activeTurn'],'expires':int(time.time()*1000)+10000}
            assert 'error' not in rpc(root,answer),'explicit synthetic native approval refused'
            answered=True
        if snap['lastTurn'] is not None and snap['activeTurn'] is None: break
        time.sleep(.025)
    assert snap['lastTurn']['status']=='completed',snap
    assert answered,'qualified boundary not exercised'
    assert not Provider.failures,Provider.failures
    count=len(Provider.turn_calls)
    assert exchange('dispatch',id,payload)['outcome']=='accepted'
    assert exchange('reconcile',id)['outcome']=='accepted'
    reconnect=uuid.uuid4().hex
    assert exchange('reconnect',reconnect)['outcome']=='reconnected'
    generation=exchange('observe',id)['generation']
    repair=uuid.uuid4().hex
    assert exchange('repair',repair,{'generation':2,'model_key':Provider.credential})['outcome']=='repaired'
    assert exchange('repair',repair,{'generation':2,'model_key':Provider.credential})['outcome']=='repaired'
    assert exchange('observe',id)['thread']==thread
    assert len(Provider.turn_calls)==count,'duplicate/reconnect/repair replayed a native turn'
    end=uuid.uuid4().hex
    ended=exchange('revoke',end);assert ended['outcome']=='revoked'
    deadline=time.monotonic()+3
    while not ended['writer_excluded'] and time.monotonic()<deadline:
        ended=exchange('reconcile',end);time.sleep(.025)
    assert ended['writer_excluded'],ended
    # Revocation is permanent; attempting new authorization cannot resurrect it.
    denied=subprocess.run(ssh+["python3 -c '"+broker.replace("'","'\\''")+"'"],input=json.dumps({'version':1,'scope':scope,'method':'authorize','id':id,'payload':None}),text=True,capture_output=True,timeout=35)
    assert denied.returncode==44,'revoked grant resurrected'
    evidence=remote("import pathlib,json,sys\np=pathlib.Path(json.load(sys.stdin)['root']);print((p/'ended.json').read_text())",{'root':root})
    assert evidence['supervisor_reaped'] and evidence['thread']==thread
    records=remote("import pathlib,json,sys\np=pathlib.Path(json.load(sys.stdin)['root']);rows=[]\nfor f in (p/'session/home/sessions').rglob('*.jsonl'):\n rows.extend(json.loads(line) for line in f.read_text().splitlines())\nprint(json.dumps(rows))",{'root':root})
    calls=[row['payload'] for row in records if row['type']=='response_item' and row['payload']['type'] in ('function_call','custom_tool_call')]
    assert len(calls)==1 and calls[0]['name']=='exec_command',calls
    outputs=[row['payload'] for row in records if row['type']=='response_item' and row['payload']['type'] in ('function_call_output','custom_tool_call_output')]
    assert any(standalone.MARKER in str(row.get('output')) and 'Process exited with code 0' in str(row.get('output')) for row in outputs),outputs
    # Original native records are transported and parsed by the real Semon CLI
    # on the same SSH server as the native controller, not generated history.
    capture=subprocess.run([args.installer],input=json.dumps({'action':'capture','enrollment':enrollment,'mirror':mirror_binary,'destination':'http://127.0.0.1:33222','token':mirror_token,'receiver':'native-307'}),text=True,capture_output=True,timeout=65)
    assert capture.returncode==0 and json.loads(capture.stdout)['running'],'native-home-only capture failed'
    deadline=time.monotonic()+10
    while True:
        readback=subprocess.check_output([mirror_binary,'sessions','--all','--machines',str(receiver_dir),'--no-local','--model-json'],text=True,timeout=20)
        if thread in readback and standalone.MARKER in readback: break
        assert time.monotonic()<deadline,'native capture/readback did not converge'
        time.sleep(.05)
    assert thread in readback and standalone.MARKER in readback,'native push/readback lost the selected thread or tool output'
    assert Provider.credential not in readback and mirror_token not in readback,'custody leaked into parsed history'
    watch_receipt=remote("import pathlib,json,sys\np=json.load(sys.stdin);root=pathlib.Path(p['root']);r=json.loads((root/'receipt').read_text());run=(root/'run').read_text();assert '--codex-home' in run and str(root/'session/home') in run;print(json.dumps({'scoped':r['capture_native']}))",{'root':root})
    assert watch_receipt['scoped']
    report['native_push_readback']=True
    report['native_home_scoped_watch']=True
    report['push_binary_sha256']=hashlib.sha256(Path(mirror_binary).read_bytes()).hexdigest()
    report['readback_bytes']=len(readback.encode())
    # Simulate a lost broker acknowledgement after the native result was durably
    # recorded, with the controller already stopped. Reconciliation reads the
    # exact protected receipt index; it never sends a new native command.
    remote("import json,pathlib,sys\np=json.load(sys.stdin);f=pathlib.Path(p['root'])/'execution'/('receipt-'+p['id']+'.json');r=json.loads(f.read_text());r['receipt']['outcome']='unknown';f.write_text(json.dumps(r))",{'root':root,'id':id})
    recovered=exchange('reconcile',id);assert recovered['outcome']=='accepted',recovered
    remote("import json,pathlib,sys\np=json.load(sys.stdin);r=pathlib.Path(p['root']);f=r/'execution'/('receipt-'+p['id']+'.json');v=json.loads(f.read_text());v['receipt']['outcome']='unknown';f.write_text(json.dumps(v));(r/'native-receipts'/(p['id']+'.json')).unlink()",{'root':root,'id':id})
    missing=exchange('reconcile',id);assert missing['outcome']=='unknown',missing
    assert len(Provider.turn_calls)==count,'lost/missing receipt caused a native replay'
    # Read-only launch observation needs no model credential and proves the final
    # state independently of native-driver connectivity.
    observer=Path(__file__).parents[2].joinpath('crates/semon-ssh/src/controller-observe.py').read_text()+'\n'+Path(__file__).parents[2].joinpath('crates/semon-ssh/src/controller-bootstrap.py').read_text()
    binding={'owner':'owner-307','workspace':'workspace-307','connection':'ssh-307','session':'session-307','model_connection':'codex-307','model_generation':1,'epoch':1}
    observed=remote(observer,{'version':1,'method':'observe','enrollment':enrollment,'launch':launch,'binding':binding})
    assert observed['state']=='ended' and observed['writer_excluded'],observed
    # A native controller, unlike a synthetic transport fixture, must stop with
    # no browser and even if a peer holds an unfinished status request open.
    for lifetime_case in ('expiry','controller_crash','explicit_stop'):
        next_enrollment=str(uuid.uuid4());next_launch=uuid.uuid4().hex
        next_root='/home/fixture/.local/state/semon-ssh/'+next_enrollment
        remote("import os,pathlib,sys,json\nos.umask(0o077);pathlib.Path(json.load(sys.stdin)['root']).mkdir(parents=True,mode=0o700)",{'root':next_root})
        next_bootstrap=dict(bootstrap,enrollment=next_enrollment,launch=next_launch,expires=int(time.time())+(10 if lifetime_case=='expiry' else 60))
        next_result=subprocess.run([args.installer],input=json.dumps(next_bootstrap),text=True,capture_output=True,timeout=40)
        assert next_result.returncode==0,next_result.stderr
        next_accepted=json.loads(next_result.stdout);assert next_accepted['state']=='running',next_accepted
        if lifetime_case=='explicit_stop':
            stopped=remote(observer,{'version':1,'method':'stop','authorize_stop':True,'enrollment':next_enrollment,'launch':next_launch,'binding':binding})
            assert stopped['launch']==next_launch
        elif lifetime_case=='controller_crash':
            # Obtain the exact protected IPC peer identity, then stop only that
            # exclusively created fixture controller. Its supervisor adopts and
            # reaps the native descendants; it never launches a replacement.
            remote("import os,socket,struct,json,sys\np=json.load(sys.stdin);s=socket.socket(socket.AF_UNIX);s.connect(p['root']+'/control.sock');pid,uid,gid=struct.unpack('3i',s.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12));assert uid==os.getuid();os.kill(pid,9)",{'root':next_root})
        elif lifetime_case=='expiry':
            stalled_script="import socket,json,sys,time\np=json.load(sys.stdin);s=socket.socket(socket.AF_UNIX);s.connect(p['root']+'/control.sock');time.sleep(15)"
            stalled=subprocess.Popen(ssh+["python3 -c '"+stalled_script.replace("'","'\\''")+"'"],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,text=True)
            stalled.stdin.write(json.dumps({'root':next_root}));stalled.stdin.close()
        bound=time.monotonic()+13
        while True:
            final=remote(observer,{'version':1,'method':'observe','enrollment':next_enrollment,'launch':next_launch,'binding':binding})
            if final['state']=='ended': break
            assert time.monotonic()<bound,(lifetime_case,final)
            time.sleep(.05)
        assert final['writer_excluded'] and final['thread']==next_accepted['thread'],final
        if lifetime_case=='expiry':
            stalled.terminate();stalled.wait(timeout=3)
        report[lifetime_case+'_reaped']=True
    assert len(Provider.turn_calls)==count,'expiry/crash fixture fabricated a native turn'
    report.update({'native_thread':thread,'enrollment':enrollment,'launch':launch,'native_tool_executed':True,'duplicate_prevented':True,'reconnect_same_thread':True,'idle_repair_same_thread':True,'revocation_permanent':True,'writer_excluded':True,'stopped_controller_receipt_recovery':True,'missing_result_remains_unknown':True,'model_free_final_observation':True,'turn_provider_calls':count,'rollout_records':len(records)})
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
finally:
    forward.terminate()
    try: forward.wait(timeout=3)
    except subprocess.TimeoutExpired: forward.kill();forward.wait()
    provider.shutdown();receiver.terminate()
    try: receiver.wait(timeout=3)
    except subprocess.TimeoutExpired: receiver.kill();receiver.wait()
    shutil.rmtree(work)
