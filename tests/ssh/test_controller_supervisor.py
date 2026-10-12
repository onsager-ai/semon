"""Independent offline lifetime proofs, including orphan adoption after crash."""
import ast, json, os
from pathlib import Path
import socket, subprocess, tempfile, time, types, unittest
SUPERVISOR=Path(__file__).parents[2]/'crates/semon-ssh/src/controller-supervisor.py'

class Lifetime(unittest.TestCase):
    def run_supervisor(self,crash=False,stalled=False,threaded=False):
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
            if threaded:
                controller.write_text(controller.read_text().replace(
                    "child=subprocess.Popen(['/bin/sleep','90'],start_new_session=True)\n(root/'native-pid').write_text(str(child.pid))",
                    "import threading\ndef native():\n child=subprocess.Popen(['/bin/sleep','90'],start_new_session=True)\n (root/'native-pid').write_text(str(child.pid))\n time.sleep(90)\nthreading.Thread(target=native).start()\nwhile not (root/'native-pid').exists(): time.sleep(.01)"))
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
    def test_native_spawned_by_another_task_is_reaped(self): self.run_supervisor(threaded=True)
class DescendantScope(unittest.TestCase):
    def test_walk_reads_only_owned_tasks_and_checks_parent_identity(self):
        # Execute the real walker against proc-shaped metadata, without starting
        # the lease supervisor. Any whole-machine enumeration fails the proof.
        visited=[]
        children={'/proc/10/task/10/children':'20',
                  '/proc/20/task/20/children':'',
                  '/proc/20/task/21/children':'30 99',
                  '/proc/30/task/30/children':''}
        identities={10:(1,'root'),20:(10,'controller'),30:(20,'native'),99:(1,'foreign')}
        # A descendant deeper than the former global scan's round count must
        # not escape writer exclusion.
        children['/proc/30/task/30/children']='31'
        for pid in range(31,50):
            children[f'/proc/{pid}/task/{pid}/children']=str(pid+1) if pid<49 else ''
            identities[pid]=(pid-1,f'native-{pid}')
        class ProcPath:
            def __init__(self,*parts): self.path='/'.join(str(p).rstrip('/') for p in parts)
            def __truediv__(self,part): return ProcPath(self.path,part)
            def iterdir(self):
                visited.append(self.path)
                tasks={'/proc/10/task':['10'],'/proc/20/task':['20','21'],'/proc/30/task':['30']}
                tasks.update({f'/proc/{pid}/task':[str(pid)] for pid in range(31,50)})
                return [self/name for name in tasks[self.path]]
            def read_text(self):
                visited.append(self.path)
                return children[self.path]
            def glob(self,pattern): raise AssertionError('unrelated process enumeration')
        module=ast.parse(SUPERVISOR.read_text())
        function=next(node for node in module.body if isinstance(node,ast.FunctionDef) and node.name=='children')
        namespace={'os':types.SimpleNamespace(getpid=lambda:10),
                   'pathlib':types.SimpleNamespace(Path=ProcPath),'identity':identities.get}
        exec(compile(ast.Module(body=[function],type_ignores=[]),str(SUPERVISOR),'exec'),namespace)
        self.assertEqual(namespace['children'](),{pid:item for pid,item in identities.items() if pid not in (10,99)})
        self.assertTrue(visited)
        self.assertTrue(all(int(path.split('/')[2]) in identities and int(path.split('/')[2])!=99 for path in visited))

if __name__=='__main__': unittest.main()
