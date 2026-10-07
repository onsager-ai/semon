# Read-only current controller state. Never resolve a model key or restart work.
def observe(root,request):
    claim=read(root/'controller-install.json')
    if claim['launch']!=request['launch'] or claim['binding']!=request['binding']: sys.exit(44)
    result={'launch':request['launch'],'state':'unknown','thread':None,'writer_excluded':False}
    if (root/'launch.json').exists():
        launch=read(root/'launch.json')
        if launch.get('launch')!=request['launch'] or launch.get('binding')!=request['binding']: sys.exit(44)
        result['thread']=launch.get('thread')
    if (root/'ended.json').exists():
        ended=read(root/'ended.json')
        if ended.get('launch')==request['launch'] and ended.get('binding')==request['binding'] and ended.get('supervisor_reaped') is True and ended.get('writer_excluded') is True:
            result['state']='ended';result['writer_excluded']=True
            return result
    try:
        ipc=root/'control.sock';private(ipc)
        if not stat.S_ISSOCK(ipc.lstat().st_mode): fail()
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(.5);peer.connect(str(ipc));peer.sendall(b'{"op":"snapshot"}');peer.shutdown(socket.SHUT_WR)
            response=bytearray()
            while True:
                data=peer.recv(65536)
                if not data: break
                response.extend(data)
                if len(response)>2097152: fail()
            snap=json.loads(response)['snapshot']
            if snap.get('thread')==result['thread'] and snap.get('connected') is True:
                result['state']='running'
    except Exception: pass
    return result
