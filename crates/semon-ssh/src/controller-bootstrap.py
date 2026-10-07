# Explicit controller installation into an existing mirror enrollment. It never
# runs from ordinary connection checks or mirror bootstrap. Private stdin only.
import base64, fcntl, hashlib, json, os, pathlib, re, socket, stat, subprocess, sys, time
os.umask(0o077)

def fail(): sys.exit(46)
def private(path,directory=False):
    meta=path.lstat()
    if meta.st_uid!=os.getuid() or meta.st_mode&0o077 or stat.S_ISLNK(meta.st_mode) or (directory and not stat.S_ISDIR(meta.st_mode)): fail()
def atomic(path,value):
    tmp=path.with_suffix('.new')
    fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'w') as f:
        json.dump(value,f,separators=(',',':'));f.flush();os.fsync(f.fileno())
    os.replace(tmp,path)
    fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)
def read(path):
    private(path)
    with os.fdopen(os.open(path,os.O_RDONLY|os.O_NOFOLLOW),'r') as f: return json.load(f)
def write(path,data,mode):
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,mode)
    with os.fdopen(fd,'wb') as f: f.write(data);f.flush();os.fsync(f.fileno())
try:
    raw=sys.stdin.buffer.read(24*1024*1024+1)
    if len(raw)>24*1024*1024: fail()
    request=json.loads(raw)
    if request['version']!=1 or request['method'] not in ('openai_api_key','observe'): fail()
    if request['method']=='openai_api_key' and request.get('authorize_execution') is not True: fail()
    uuid=r'[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}'
    if not re.fullmatch(uuid,request['enrollment']) or not re.fullmatch(r'[0-9a-f]{32}',request['launch']): fail()
    binding=request['binding']
    if set(binding)!={'owner','workspace','connection','session','model_connection','model_generation','epoch'}: fail()
    for key in ('owner','workspace','connection','session','model_connection'):
        if not isinstance(binding[key],str) or not re.fullmatch(r'[A-Za-z0-9_.-]{1,128}',binding[key]) or binding[key] in ('.','..'): fail()
    if any(type(binding[key]) is not int or binding[key]<1 for key in ('model_generation','epoch')): fail()
    if request['method']!='observe' and (type(request['expires']) is not int or not time.time()<request['expires']<=time.time()+301): fail()
    root=pathlib.Path.home()/'.local'/'state'/'semon-ssh'/request['enrollment']
    for path in reversed((root,*root.parents)):
        if path.is_symlink(): fail()
        if path.is_relative_to(pathlib.Path.home()):
            meta=path.lstat()
            if meta.st_uid!=os.getuid() or meta.st_mode&0o022: fail()
    private(root,True)
    if request['method']=='observe':
        print(json.dumps(observe(root,request)));sys.exit(0)
    lock=os.open(root/'installation.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    private(root/'installation.lock')
    try: fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    except BlockingIOError: sys.exit(45)
    artifact=base64.b64decode(request['controller'],validate=True)
    if not 0<len(artifact)<=16*1024*1024 or hashlib.sha256(artifact).hexdigest()!=request['controller_sha256']: fail()
    config=request['model_config']
    if not isinstance(config,str) or len(config.encode())>32768: fail()
    identity={'launch':request['launch'],'binding':binding,'native_directory':request['native_directory'],'controller_sha256':request['controller_sha256'],'model_config_sha256':hashlib.sha256(config.encode()).hexdigest(),'expires':request['expires']}
    claim=root/'controller-install.json'
    if claim.exists() or claim.is_symlink():
        if read(claim)!=identity: sys.exit(44)
        # Duplicate/uncertain installation is observation only, never a launch.
        receipt=observe(root,request)
        print(json.dumps(receipt));sys.exit(0)
    native=pathlib.Path(request['native_directory'])
    if not native.is_absolute() or native.resolve()!=native or native.is_relative_to(root/'workspace'): fail()
    # Integrity-pinned complete upstream package. No partial executable install or
    # unqualified alternate model resources/executors are accepted.
    manifest=NATIVE_MANIFEST
    paths={str(path.relative_to(native)) for path in native.rglob('*') if not path.is_dir()}
    if paths!=set(manifest['files']): fail()
    for name,digest in manifest['files'].items():
        path=native/name;meta=path.lstat()
        if not stat.S_ISREG(meta.st_mode) or meta.st_mode&0o022 or meta.st_uid not in (0,os.getuid()): fail()
        for ancestor in path.parents:
            info=ancestor.lstat()
            if stat.S_ISLNK(info.st_mode) or info.st_mode&0o022 or info.st_uid not in (0,os.getuid()): fail()
            if ancestor==native: break
        if hashlib.file_digest(path.open('rb'),'sha256').hexdigest()!=digest: fail()
    # Commit this identity before creating native state. Failed/partial setup
    # remains uncertain and cannot be silently retried into a new native thread.
    atomic(claim,identity)
    (root/'workspace').mkdir(mode=0o700)
    write(root/'controller',artifact,0o700)
    write(root/'model.toml',config.encode(),0o600)
    write(root/'controller-supervisor.py',SUPERVISOR.encode(),0o600)
    request.pop('controller');request.pop('model_config');request.pop('controller_sha256')
    request['root']=str(root);request['binary']=str(native/'bin'/'codex')
    process=subprocess.Popen(['/usr/bin/python3',str(root/'controller-supervisor.py')],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env={'PATH':'/usr/bin:/bin','HOME':str(pathlib.Path.home())},start_new_session=True,close_fds=True)
    process.stdin.write(json.dumps(request,separators=(',',':')).encode());process.stdin.close()
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        if (root/'launch.json').exists():
            receipt=read(root/'launch.json')
            if receipt.get('state')=='running' and (root/'control.sock').exists():
                print(json.dumps(observe(root,request)));sys.exit(0)
        if process.poll() is not None: break
        time.sleep(.025)
    # Controller/supervisor may still finish. Preserve identity and inspect it;
    # setup timeout is never a reason to launch again.
    print(json.dumps({'launch':request['launch'],'state':'unknown'}))
except Exception:
    sys.exit(46)
