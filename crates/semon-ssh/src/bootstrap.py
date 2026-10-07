# Versioned, idempotent mirror-only installation. No agent execution.
import base64, fcntl, hashlib, json, os, pathlib, signal, subprocess, sys, time
try:
    os.umask(0o077)
    p = json.load(sys.stdin)
    assert p['version'] == 1
    root = pathlib.Path.home() / '.local' / 'state' / 'semon-ssh'
    # Refuse symlinks at every installation ancestor, including the home.
    for parent in reversed((root, *root.parents)):
        if parent.is_symlink():
            raise ValueError('unsafe directory')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if root.stat().st_uid != os.getuid() or root.stat().st_mode & 0o077:
        raise ValueError('unsafe permissions')
    d = root / p['operation']
    if d.is_symlink():
        raise ValueError('unsafe directory')
    d.mkdir(mode=0o700, exist_ok=True)
    if d.stat().st_uid != os.getuid() or d.stat().st_mode & 0o077:
        raise ValueError('unsafe permissions')
    def write(name, data, mode):
        tmp = d / (name + '.new')
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
        with os.fdopen(fd, 'wb') as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, d / name)
    lock = os.open(d / 'setup.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX)
    # Recover only our private interrupted temporary files.
    for name in ('semon', 'token', 'run', 'receipt'):
        tmp = d / (name + '.new')
        if tmp.exists() or tmp.is_symlink():
            tmp.unlink()
    raw = base64.b64decode(p['binary'], validate=True)
    assert hashlib.sha256(raw).hexdigest() == p['sha256']
    # A busy watch lock means this operation is already running. Keep its
    # executable inode intact, but refresh the token for explicit repair.
    watch = os.open(d / 'watch.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    busy = False
    try:
        fcntl.flock(watch, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        busy = True
    receiver = p.get('receiver_identity')
    if receiver is not None:
        assert isinstance(receiver, str) and 0 < len(receiver.encode()) <= 256
    prior = json.loads((d / 'receipt').read_text()) if busy else {}
    changed_receiver = prior.get('receiver') != receiver or prior.get('destination') != p['destination']
    if busy and ((d / 'token').read_bytes() != p['token'].encode() or changed_receiver):
        # Credential rotation keeps one logical receiver checkpoint. A new
        # receiver or destination must not leave the old watcher publishing.
        # Validate the process identity before signalling our own watch group.
        receipt = prior
        pid = receipt['pid']
        stat = pathlib.Path('/proc/' + str(pid) + '/stat').read_text().rsplit(')', 1)[1].split()
        assert stat[19] == receipt['start'] and int(stat[3]) == pid
        assert pathlib.Path('/proc/sys/kernel/random/boot_id').read_text() == receipt['boot']
        os.killpg(pid, signal.SIGTERM)
        deadline = time.monotonic() + 3
        while True:
            try:
                fcntl.flock(watch, fcntl.LOCK_EX | fcntl.LOCK_NB)
                busy = False
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError('old mirror still running')
                time.sleep(0.05)
    if not busy:
        write('semon', raw, 0o700)
        result = subprocess.run([str(d / 'semon'), '--version'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        assert result.returncode == 0
    write('token', p['token'].encode(), 0o600)
    import shlex
    state_export = ''
    if receiver is not None:
        # Keep prior checkpoints intact. URL alone cannot identify a receiver;
        # a newly enrolled receiver needs the unchanged original history too.
        state_home = d / 'receiver-state' / hashlib.sha256(receiver.encode()).hexdigest()
        for directory in (state_home.parent, state_home):
            if directory.is_symlink():
                raise ValueError('unsafe state directory')
            directory.mkdir(mode=0o700, exist_ok=True)
            if directory.stat().st_uid != os.getuid() or directory.stat().st_mode & 0o077:
                raise ValueError('unsafe state permissions')
        state_export = 'export XDG_STATE_HOME=' + shlex.quote(str(state_home)) + '\n'
    cmd = [str(d / 'semon'), 'push', '--to', p['destination'], '--token-file', str(d / 'token'), '--watch']
    write('run', ('#!/bin/sh\n' + state_export + 'exec flock -n ' + shlex.quote(str(d / 'watch.lock')) + ' ' + shlex.join(cmd) + '\n').encode(), 0o700)
    if not busy:
        fcntl.flock(watch, fcntl.LOCK_UN)
        process = subprocess.Popen(['nohup', str(d / 'run')], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)
        stat = pathlib.Path('/proc/' + str(process.pid) + '/stat').read_text().rsplit(')', 1)[1].split()
        write('receipt', json.dumps({'pid': process.pid, 'start': stat[19], 'boot': pathlib.Path('/proc/sys/kernel/random/boot_id').read_text(), 'receiver': receiver, 'destination': p['destination']}).encode(), 0o600)
        time.sleep(0.5)
        try:
            fcntl.flock(watch, fcntl.LOCK_EX | fcntl.LOCK_NB)
            raise ValueError('push exited')
        except BlockingIOError:
            pass
    print(json.dumps({'running': True}))
except Exception:
    # Never echo a payload, path, token, or host-provided exception.
    sys.exit(43)
