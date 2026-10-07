"""Independent offline lifetime proofs, including orphan adoption after crash."""
import json, os
from pathlib import Path
import socket, subprocess, tempfile, time, unittest
SUPERVISOR=Path(__file__).parents[2]/'crates/semon-ssh/src/controller-supervisor.py'

class Lifetime(unittest.TestCase):
    def run_supervisor(self,crash=False,stalled=False):
        with tempfile.TemporaryDirectory(prefix='semon-controller-life-') as directory:
            root=Path(directory);root.chmod(0o700)
            controller=root/'controller'
            controller.write_text('''#!/usr/bin/python3
import json,os,pathlib,subprocess,sys,time,socket
p=json.load(sys.stdin);root=pathlib.Path(p['root'])
child=subprocess.Popen(['/bin/sleep','90'],start_new_session=True)
(root/'native-pid').write_text(str(child.pid))
(root/'launch.json').write_text(json.dumps({'thread':'native-synthetic-thread'}))
''' + ('os._exit(13)\n' if crash else '''
s=socket.socket(socket.AF_UNIX);s.bind(str(root/'control.sock'));s.listen(1)
if p['stalled']:
 peer,_=s.accept();peer.recv(10000)
else: time.sleep(90)
'''))
            controller.chmod(0o700)
            request={'root':str(root),'launch':'a'*32,'expires':int(time.time())+3,'binding':{'owner':'synthetic-owner'},'model_key':'synthetic-custody-only','stalled':stalled}
            process=subprocess.Popen(['/usr/bin/python3',str(SUPERVISOR)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            process.stdin.write(json.dumps(request));process.stdin.close()
            deadline=time.monotonic()+5
            while not (root/'native-pid').exists():
                self.assertIsNone(process.poll());self.assertLess(time.monotonic(),deadline);time.sleep(.01)
            pid=int((root/'native-pid').read_text())
            peer=None
            if stalled:
                while not (root/'control.sock').exists(): time.sleep(.01)
                peer=socket.socket(socket.AF_UNIX);peer.connect(str(root/'control.sock'))
                # Deliberately send no body/EOF: observation client is stalled.
            try:
                self.assertEqual(process.wait(timeout=7),0,process.stderr.read())
                elapsed=5-(deadline-time.monotonic())
                self.assertLess(elapsed,5,'lifetime depended on socket activity')
                ended=json.loads((root/'ended.json').read_text())
                self.assertTrue(ended['writer_excluded']);self.assertTrue(ended['supervisor_reaped'])
                self.assertEqual(ended['thread'],'native-synthetic-thread')
                self.assertFalse(Path('/proc',str(pid)).exists(),'native descendant survived controller shutdown')
                self.assertEqual(ended['cause'],'controller_exit' if crash else 'expired')
                for path in root.glob('*.json'):
                    self.assertNotIn(request['model_key'],path.read_text(),'custody leaked into durable receipt')
            finally:
                if peer: peer.close()
                process.stdout.close();process.stderr.close()
                if process.poll() is None: process.kill();process.wait()
    def test_expiry_without_browser_reaps_native(self): self.run_supervisor()
    def test_stalled_observer_does_not_extend_lifetime(self): self.run_supervisor(stalled=True)
    def test_crashed_controller_adopts_and_reaps_native(self): self.run_supervisor(crash=True)
if __name__=='__main__': unittest.main()
