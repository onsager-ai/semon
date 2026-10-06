"""Offline SDK boundary and real Unix bridge tests; no provider qualification."""
import importlib.util
import json
from pathlib import Path
import shlex
import socket
import subprocess
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('worker',Path(__file__).with_name('e2b-managed.py'))
worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
LABELS={k:'fixture' for k in worker.LABELS}
def request(**extra):return dict(version=1,api_key='synthetic-provider',labels=LABELS,
    profile={'template':'fixture-profile','timeout':300,'lifecycle':{'on_timeout':{'action':'kill'}}},**extra)
class Tests(unittest.TestCase):
    def test_single_create_defaults_and_no_model_credential(self):
        calls=[]
        class SDK:
            @staticmethod
            def create(**kw):calls.append(kw);return SimpleNamespace(sandbox_id='vm-one')
        self.assertEqual(worker.managed(request(method='create'),SDK),{'id':'vm-one'})
        self.assertEqual(len(calls),1)
        self.assertEqual(calls[0]['template'],'fixture-profile');self.assertEqual(calls[0]['timeout'],300);self.assertEqual(calls[0]['retries'],0)
        self.assertEqual(calls[0]['lifecycle'],{'on_timeout':{'action':'kill'}})
        self.assertNotIn('model_key',calls[0])
    def test_missing_or_invalid_profile_never_creates(self):
        class SDK:
            @staticmethod
            def create(**kw):self.fail('invalid profile must not provision')
        for profile in [None, {}, {'template':'base','timeout':True,'lifecycle':{'on_timeout':{'action':'kill'}}},
                        {'template':'base','timeout':300,'lifecycle':{'on_timeout':{'action':'pause'}}}]:
            value=request(method='create');value['profile']=profile
            self.assertEqual(worker.managed(value,SDK),{'error':'invalid_profile'})
    def test_foreign_and_paused_resources_never_connect(self):
        for metadata,state in [({},'running'),(LABELS,'paused')]:
            class SDK:
                @staticmethod
                def get_info(*args,**kw):return SimpleNamespace(metadata=metadata,state=state)
                @staticmethod
                def connect(*args,**kw):self.fail('must not connect/resume')
            self.assertIn('error',worker.managed(request(method='rpc',id='vm-one',payload={'op':'snapshot'}),SDK))
    def test_real_private_bridge_carries_payload_without_command_interpolation(self):
        with tempfile.TemporaryDirectory() as root:
            sockpath=root+'/control.sock';server=socket.socket(socket.AF_UNIX);server.bind(sockpath);server.listen()
            payload={'op':'send','text':'private task; $(do-not-run) `literal`\nnext'}
            seen=[]
            def receive():
                conn,_=server.accept();buf=bytearray()
                while True:
                    block=conn.recv(65536)
                    if not block:break
                    buf.extend(block)
                seen.append(json.loads(buf));conn.sendall(b'{"snapshot":{"thread":"native"}}');conn.close()
            thread=threading.Thread(target=receive);thread.start()
            commands=[]
            class Files:
                def write(self,path,data,**kw):Path(path.replace('/var/lib/semon',root)).write_text(data)
            class Commands:
                def run(self,cmd,**kw):
                    selfcmd=cmd.replace('/var/lib/semon',root);commands.append(cmd)
                    result=subprocess.run(selfcmd,shell=True,capture_output=True,text=True,timeout=5)
                    return SimpleNamespace(exit_code=result.returncode,stdout=result.stdout)
            vm=SimpleNamespace(files=Files(),commands=Commands())
            class SDK:
                @staticmethod
                def get_info(*args,**kw):return SimpleNamespace(metadata=LABELS,state='running')
                @staticmethod
                def connect(*args,**kw):return vm
            with patch.object(worker, 'attach_running', return_value=vm):
                result=worker.managed(request(method='rpc',id='vm-one',payload=payload),SDK)
            thread.join(5);server.close()
            self.assertEqual(result,{'snapshot':{'thread':'native'}});self.assertEqual(seen,[payload])
            self.assertTrue(all(payload['text'] not in cmd and 'synthetic-provider' not in cmd for cmd in commands))
            self.assertFalse(list(Path(root).glob('request-*')))
    def test_device_bootstrap_transports_opaque_auth_only_in_private_file(self):
        artifact = json.dumps({'auth_mode':'chatgpt','OPENAI_API_KEY':None,
            'tokens':{'access_token':'synthetic-access','refresh_token':'synthetic-refresh'}})
        commands, files = [], {}
        class Files:
            def write(self,path,data,**kw): files[path]=data
        class Commands:
            def run(self,cmd,**kw):
                commands.append(cmd)
                return SimpleNamespace(exit_code=0,stdout='')
        vm=SimpleNamespace(files=Files(),commands=Commands())
        class SDK:
            @staticmethod
            def get_info(*args,**kw):return SimpleNamespace(metadata=LABELS,state='running')
        value=request(method='bootstrap',id='vm-one',model_method='chatgpt_device_code',
            auth_artifact=artifact,artifact_url='https://artifact.test/guest.tar.gz',
            artifact_sha256='a'*64,push_token='synthetic-push',push_url='https://mirror.test/api/push')
        with patch.object(worker,'attach_running',return_value=vm):
            self.assertEqual(worker.managed(value,SDK),{'started':True})
        launch=json.loads(files['/var/lib/semon/launch.json'])
        self.assertEqual(launch['auth_artifact'],artifact)
        self.assertIsNone(launch['api_key'])
        self.assertTrue(all('synthetic-access' not in cmd and 'synthetic-refresh' not in cmd
            and 'synthetic-push' not in cmd and 'synthetic-provider' not in cmd for cmd in commands))
        self.assertIn('chmod 600 /var/lib/semon/launch.json',commands)
        commands.clear();value['model_key']='mixed-key'
        with patch.object(worker,'attach_running',return_value=vm):
            self.assertEqual(worker.managed(value,SDK),{'error':'model_auth_required'})
        self.assertEqual(commands,[])

if __name__=='__main__':unittest.main()
