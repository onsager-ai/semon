# Synthetic qualification driver only. Never installed by production APIs.
import json, os, pathlib, socket, sys, time
root = pathlib.Path(sys.argv[1])
os.umask(0o077)
listener = socket.socket(socket.AF_UNIX)
listener.bind(str(root / 'control.sock'))
os.chmod(root / 'control.sock', 0o600)
listener.listen(8)
snap = {'thread':'synthetic-thread','generation':'synthetic-generation','connected':True,'activeTurn':None,'actions':{}}
sends = 0
repairs = 0
for _ in range(200):
    s, _ = listener.accept()
    try:
        data = bytearray()
        while True:
            b = s.recv(65536)
            if not b: break
            data.extend(b)
        p = json.loads(data)
        if p['op'] == 'reconnect': snap['generation'] = 'reconnected-generation'
        if p['op'] == 'send':
            sends += 1
            snap['actions'][p['id']] = {'delivery':'accepted'}
            (root / 'counts.json').write_text(json.dumps({'sends':sends,'repairs':repairs}))
            if p['text'] == 'missing-result':
                del snap['actions'][p['id']]
                s.close()
                continue
            if p['text'] == 'lost-reply':
                # Accepted native work with an interrupted transport reply.
                s.close()
                continue
            if p['text'] == 'slow-reply': time.sleep(3)
        if p['op'] == 'renew':
            assert p['model_key'] == 'synthetic-model-secret'
            repairs += 1
            snap['renewalId'] = p['id']
            (root / 'counts.json').write_text(json.dumps({'sends':sends,'repairs':repairs}))
            s.close()
            continue
        if p['op'] == 'end':
            (root / 'ended.json').write_text(json.dumps({'ended':True,'thread':snap['thread']}))
        s.sendall(json.dumps({'snapshot':snap,'ended':p['op']=='end'}).encode())
    except (BrokenPipeError, ConnectionResetError): pass
    finally: s.close()
    if p['op'] == 'end': break
listener.close()
(root / 'control.sock').unlink()
