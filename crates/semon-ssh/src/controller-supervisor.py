# Protected independent lifetime supervisor. No network, credential files or replay.
import ctypes, json, os, pathlib, signal, subprocess, sys, time
os.umask(0o077)
raw=sys.stdin.buffer.read(131073)
if len(raw)>131072: sys.exit(46)
request=json.loads(raw)
root=pathlib.Path(request['root'])
if type(request['expires']) is not int or not time.time()<request['expires']<=time.time()+301: sys.exit(44)
# Adopt only our descendants if the controller dies; unrelated processes cannot
# enter this exclusively started process tree. Linux failure is fatal.
if ctypes.CDLL(None,use_errno=True).prctl(36,1,0,0,0)!=0: sys.exit(46)

def atomic(path,value):
    tmp=path.with_suffix('.new')
    fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'w') as f:
        json.dump(value,f,separators=(',',':'));f.flush();os.fsync(f.fileno())
    os.replace(tmp,path)
    fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)

def identity(pid):
    try:
        fields=pathlib.Path('/proc',str(pid),'stat').read_text().rsplit(')',1)[1].split()
        return int(fields[1]),fields[19]
    except (OSError,ValueError,IndexError): return None

def children():
    found={os.getpid()}; identities={}
    # Repeated bounded ancestry expansion handles newly adopted descendants.
    for _ in range(16):
        added=False
        for proc in pathlib.Path('/proc').glob('[0-9]*'):
            pid=int(proc.name)
            item=identity(pid)
            if item and item[0] in found and pid not in found:
                found.add(pid);identities[pid]=item;added=True
        if not added: break
    return identities

def reap():
    while True:
        try:
            if os.waitpid(-1,os.WNOHANG)[0]==0: break
        except ChildProcessError: break

boot=pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip()
atomic(root/'supervisor.json',{'pid':os.getpid(),'start':identity(os.getpid())[1],'boot':boot,'launch':request['launch'],'expires':request['expires']})
log=os.open(root/'controller.log',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
controller=subprocess.Popen([str(root/'controller')],stdin=subprocess.PIPE,stdout=log,stderr=log,env={'PATH':'/usr/bin:/bin','HOME':str(pathlib.Path.home())},start_new_session=True)
os.close(log)
controller.stdin.write(json.dumps(request,separators=(',',':')).encode());controller.stdin.close()
# Monotonic bound also survives backward wall-clock adjustments. No browser is
# needed to trigger expiry. An incoming stalled socket cannot extend this bound.
remaining=max(0,min(300,request['expires']-time.time()))
deadline=time.monotonic()+remaining
while controller.poll() is None and time.monotonic()<deadline and time.time()<request['expires']:
    time.sleep(.05)
cause='expired' if controller.poll() is None else 'controller_exit'
for sig in (signal.SIGTERM,signal.SIGKILL):
    for pid,item in children().items():
        if identity(pid)==item:
            try: os.kill(pid,sig)
            except ProcessLookupError: pass
    until=time.monotonic()+1
    while time.monotonic()<until:
        controller.poll();reap()
        if not children(): break
        time.sleep(.025)
    if not children(): break
controller.poll();reap()
excluded=not children()
try: launch=json.loads((root/'launch.json').read_text())
except (OSError,ValueError): launch={}
if excluded:
    # Native reaped evidence and the immutable launch binding are retained even
    # after controller failure. No fresh session may reuse this launch/home.
    atomic(root/'ended.json',{'ended':True,'thread':launch.get('thread'),'launch':request['launch'],'binding':request['binding'],'writer_excluded':True,'cause':cause,'supervisor_reaped':True})
else:
    atomic(root/'shutdown-unknown.json',{'launch':request['launch'],'binding':request['binding'],'outcome':'unknown','cause':cause})
sys.exit(0 if excluded else 46)
