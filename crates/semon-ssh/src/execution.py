# Private driver broker v1. No arbitrary commands, installation or compute control.
import fcntl, hashlib, json, os, pathlib, re, socket, stat, sys, time

class Refusal(Exception):
    def __init__(self, code): self.code = code

def refuse(code): raise Refusal(code)
def private(path, directory=False):
    info = path.lstat()
    if info.st_uid != os.getuid() or info.st_mode & 0o077 or stat.S_ISLNK(info.st_mode):
        refuse(46)
    if directory and not stat.S_ISDIR(info.st_mode): refuse(46)

def read(path):
    private(path)
    if not stat.S_ISREG(path.lstat().st_mode): refuse(46)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as f:
        data = f.read(131073)
    if len(data) > 131072: refuse(46)
    return json.loads(data)

def atomic(path, value):
    tmp = path.with_suffix('.new')
    if tmp.exists() or tmp.is_symlink():
        private(tmp)
        tmp.unlink()
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f, separators=(',', ':'))
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)

def rpc(payload):
    ipc = enrollment / 'control.sock'
    private(ipc)
    if not stat.S_ISSOCK(ipc.lstat().st_mode): refuse(46)
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(max(0.001,min(25,deadline-time.monotonic())))
        s.connect(str(ipc))
        s.sendall(json.dumps(payload, separators=(',', ':')).encode())
        s.shutdown(socket.SHUT_WR)
        data = bytearray()
        while True:
            remaining = min(25,deadline-time.monotonic())
            if remaining <= 0: refuse(46)
            s.settimeout(remaining)
            block = s.recv(65536)
            if not block: break
            data.extend(block)
            if len(data) > 2097152: refuse(46)
        return json.loads(data)

def evidence(record):
    # Missing controller/action/index evidence stays unknown. Never resend work.
    try:
        if record['method']=='revoke':
            ended=read(enrollment/'ended.json')
            installed=enrollment/'controller-install.json'
            if ended.get('ended') is True and ended.get('thread')==scope['thread']:
                if not installed.exists() or (ended.get('binding')==read(installed)['binding'] and ended.get('writer_excluded') is True and ended.get('supervisor_reaped') is True):
                    record['receipt']['writer_excluded']=True
            return record
        snap=rpc({'op':'snapshot'})['snapshot']
        if snap.get('thread')!=scope['thread']: return record
        if record['method']=='dispatch':
            delivery=snap.get('actions',{}).get(identifier,{}).get('delivery')
            if delivery=='accepted': record['receipt']['outcome']='accepted'
        elif record['method']=='reconnect' and snap.get('connected') is True:
            record['receipt']['outcome']='reconnected'
        elif record['method']=='repair' and snap.get('renewalId')==identifier:
            record['receipt']['outcome']='repaired'
    except Exception: pass
    if record['method']=='dispatch' and record['receipt']['outcome']=='unknown' and 'native_identity' in record:
        try:
            proof=read(enrollment/'native-receipts'/(identifier+'.json'))
            stable=read(enrollment/'controller-install.json')['binding']
            if proof.get('thread')==scope['thread'] and proof.get('binding')==stable and proof.get('native_identity')==record['native_identity'] and proof.get('delivery') in ('accepted','rejected'):
                record['receipt']['outcome']=proof['delivery']
        except Exception: pass
    if record['method']=='repair' and record['receipt']['outcome']=='unknown':
        try:
            proof=read(enrollment/'native-renewals'/(identifier+'.json'))
            stable=read(enrollment/'controller-install.json')['binding']
            if proof.get('thread')==scope['thread'] and proof.get('binding')==stable and proof.get('generation')==record['identity'] and proof.get('state')=='accepted':
                record['receipt']['outcome']='repaired'
        except Exception: pass
    return record

try:
    os.umask(0o077)
    deadline = time.monotonic()+30
    raw = sys.stdin.buffer.read(131073)
    if len(raw) > 131072: refuse(46)
    request = json.loads(raw)
    if request.get('version') != 1: refuse(46)
    scope = request['scope']
    identifier = request['id']
    ident = re.compile(r'[A-Za-z0-9_.-]{1,128}\Z')
    uuid = re.compile(r'[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\Z')
    if set(scope) != {'enrollment','authority','owner','workspace','connection','session','operation','epoch','thread','model_connection','expires'}: refuse(46)
    if not all(isinstance(scope[k], str) and ident.fullmatch(scope[k]) and scope[k] not in ('.','..') for k in ('owner','workspace','connection','session','operation','thread','model_connection')): refuse(46)
    if not all(uuid.fullmatch(scope[k]) for k in ('enrollment','authority')): refuse(46)
    if not re.fullmatch(r'[0-9a-f]{32}', identifier): refuse(46)
    if type(scope['epoch']) is not int or scope['epoch'] < 1: refuse(46)
    if type(scope['expires']) is not int or not time.time() < scope['expires'] <= time.time()+301: refuse(44)
    method = request['method']
    if method not in ('authorize','dispatch','reconcile','repair','revoke','observe','reconnect'): refuse(46)
    enrollment = pathlib.Path.home() / '.local' / 'state' / 'semon-ssh' / scope['enrollment']
    # The enrollment is established by separately authorized mirror bootstrap.
    for ancestor in reversed((enrollment, *enrollment.parents)):
        if ancestor.is_symlink(): refuse(46)
        if ancestor.is_relative_to(pathlib.Path.home()):
            info = ancestor.lstat()
            if info.st_uid != os.getuid() or info.st_mode & 0o022: refuse(46)
    private(enrollment, True)
    installed = enrollment / 'controller-install.json'
    if installed.exists() or installed.is_symlink():
        launch = read(enrollment / 'launch.json')
        stable = read(installed)['binding']
        if any(stable.get(key) != scope[key] for key in ('owner','workspace','connection','session','model_connection','epoch')) or launch.get('thread') != scope['thread']:
            refuse(44)
    root = enrollment / 'execution'
    root.mkdir(mode=0o700, exist_ok=True)
    private(root, True)
    lockpath = root / 'writer.lock'
    lock = os.open(lockpath, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    private(lockpath)
    try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError: refuse(45)
    grantpath = root / ('authority-' + scope['authority'] + '.json')
    binding = {k:v for k,v in scope.items() if k != 'expires'}
    grant = read(grantpath) if grantpath.exists() else None
    if grant is not None and grant['binding'] != binding: refuse(44)
    if method == 'authorize':
        if grant is not None and grant['revoked']: refuse(44)
        # A grant is usable only with a protected, already qualified driver.
        snap = rpc({'op':'snapshot'})['snapshot']
        if snap.get('thread') != scope['thread']: refuse(46)
        atomic(grantpath, {'binding':binding,'revoked':False})
        print(json.dumps({'id':identifier,'outcome':'accepted','writer_excluded':False}))
        sys.exit(0)
    if grant is None: refuse(44)
    if method == 'observe':
        if grant['revoked']: refuse(44)
        snap = rpc({'op':'snapshot'})['snapshot']
        if snap.get('thread') != scope['thread']: refuse(46)
        generation = snap.get('generation')
        if not isinstance(generation,str) or not 1 <= len(generation) <= 256 or any(ord(c)<32 for c in generation): refuse(46)
        print(json.dumps({'thread':scope['thread'],'generation':generation,'connected':snap.get('connected') is True,'writer_active':snap.get('activeTurn') is not None}))
        sys.exit(0)
    receiptpath = root / ('receipt-' + identifier + '.json')
    record = read(receiptpath) if receiptpath.exists() else None
    if record is not None and record['binding'] != binding: refuse(44)
    if method == 'reconcile':
        if record is None:
            print(json.dumps({'id':identifier,'outcome':'unknown','writer_excluded':False}))
        else:
            record = evidence(record)
            atomic(receiptpath, record)
            print(json.dumps(record['receipt']))
        sys.exit(0)
    if grant['revoked'] and method != 'revoke': refuse(44)
    payload = request['payload']
    identity = None
    if method == 'dispatch':
        identity = hashlib.sha256(json.dumps(payload,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    elif method == 'repair':
        if type(payload.get('generation')) is not int or payload['generation'] < 1: refuse(46)
        identity = payload['generation']
    if record is not None:
        if record['method'] != method or record['identity'] != identity: refuse(46)
        # Duplicate delivery is inspection only, even if the first client vanished.
        record = evidence(record)
        atomic(receiptpath, record)
        print(json.dumps(record['receipt']))
        sys.exit(0)
    if grant['revoked']: refuse(44)
    payload = request['payload']
    record = {'binding':binding,'method':method,'identity':identity,'receipt':{'id':identifier,'outcome':'unknown','writer_excluded':False}}
    if method=='dispatch':
        record['native_identity']={'generation':payload['generation'],'input_sha256':hashlib.sha256(payload['text'].encode()).hexdigest()}
    if method == 'revoke':
        # Revocation fences future delivery even if shutdown cannot be confirmed.
        grant['revoked'] = True
        atomic(grantpath, grant)
        record['receipt']['outcome'] = 'revoked'
        command = {'op':'end'}
    elif method == 'reconnect':
        command = {'op':'reconnect'}
    else:
        snap = rpc({'op':'snapshot'})['snapshot']
        if snap.get('thread') != scope['thread']: refuse(46)
        if snap.get('activeTurn') is not None: refuse(45)
        if method == 'dispatch':
            if not isinstance(payload.get('generation'),str) or not 1 <= len(payload['generation']) <= 256 or snap.get('generation') != payload['generation']: refuse(46)
            text = payload.get('text')
            if not isinstance(text,str) or not 1 <= len(text.encode()) <= 65536: refuse(46)
            command = {'op':'send','id':identifier,'thread':scope['thread'],'generation':payload['generation'],'activeTurn':None,'expires':min(scope['expires']*1000,int(time.time()*1000)+25000),'text':text}
        else:
            model = payload.get('model_key')
            if not isinstance(model,str) or not 1 <= len(model) <= 4096 or any(ord(c)<32 for c in model): refuse(46)
            command = {'op':'renew','id':identifier,'method':'openai_api_key','model_key':model,'model_generation':payload['generation']}
    # This durable claim is the no-replay boundary. Cancellation, expiry or crash
    # after this fsync leaves Unknown until qualified driver evidence resolves it.
    atomic(receiptpath, record)
    try:
        if method == 'revoke':
            snap = rpc({'op':'snapshot'})['snapshot']
            if snap.get('thread') != scope['thread']: refuse(46)
        elif time.time() >= scope['expires']:
            refuse(44)
        response = rpc(command)
        if method == 'revoke' and response.get('ended') is True:
            record['receipt']['writer_excluded'] = True
        else:
            record = evidence(record)
    except Exception:
        pass
    atomic(receiptpath, record)
    print(json.dumps(record['receipt']))
except Refusal as error:
    sys.exit(error.code)
except Exception:
    # Never echo remote paths, prompts, credential material or driver exceptions.
    sys.exit(46)
