#!/usr/bin/env python3
"""Non-private upload codec experiment, separate from product defaults.

Uses the exact JSON/base64 Append envelope, real HTTP/1.1 loopback requests and
fsync-before-ACK storage. The experimental receiver decodes bounded request
bodies; it is not Hub or its authorization/quota/Index implementation. No source
homes, credentials or external services are read. Each codec runs in a fresh
process so peak RSS is comparable. Requires Python 3 and the system libzstd.
"""

import argparse
import base64
import ctypes
import ctypes.util
import gzip
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import resource
import statistics
import subprocess
import sys
import tempfile
import threading
import time


class Zstd:
    def __init__(self):
        self.lib = ctypes.CDLL(ctypes.util.find_library('zstd'))
        for name, arguments in {
            'ZSTD_compressBound': [ctypes.c_size_t],
            'ZSTD_compress': [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int],
            'ZSTD_decompress': [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t],
            'ZSTD_isError': [ctypes.c_size_t],
        }.items():
            function = getattr(self.lib, name)
            function.argtypes = arguments
            function.restype = ctypes.c_size_t

    def compress(self, data):
        buffer = ctypes.create_string_buffer(self.lib.ZSTD_compressBound(len(data)))
        size = self.lib.ZSTD_compress(buffer, len(buffer), data, len(data), 3)
        assert not self.lib.ZSTD_isError(size)
        return buffer.raw[:size]

    def decompress(self, data):
        buffer = ctypes.create_string_buffer(6 << 20)
        size = self.lib.ZSTD_decompress(buffer, len(buffer), data, len(data))
        assert not self.lib.ZSTD_isError(size)
        return buffer.raw[:size]


class Connection(http.client.HTTPConnection):
    wire_bytes = 0

    def send(self, data):
        self.wire_bytes += len(data)
        return super().send(data)


def percentile(values, fraction):
    return sorted(values)[min(len(values)-1, int(len(values)*fraction))]


def run(mode, mib):
    zstd = Zstd()
    encode = {'identity': lambda data: data, 'gzip': lambda data: gzip.compress(data, compresslevel=1, mtime=0), 'zstd': zstd.compress}[mode]
    decode = {'identity': lambda data: data, 'gzip': gzip.decompress, 'zstd': zstd.decompress}[mode]
    fixture_root = Path(__file__).resolve().parent.parent / 'tests/fixtures/compatibility'
    files = sorted(fixture_root.rglob('*.jsonl'))
    corpus = b''.join(path.read_bytes() for path in files)
    corpus_hash = hashlib.sha256(corpus).hexdigest()
    corpus = (corpus * ((mib * (1 << 20) + len(corpus)-1) // len(corpus)))[:mib * (1 << 20)]
    chunks = [corpus[at:at+(256 << 10)] for at in range(0, len(corpus), 256 << 10)]
    cpu = {'encode_s': 0.0, 'decode_s': 0.0}
    with tempfile.TemporaryDirectory(prefix='semon-upload-bench-') as directory:
        stored = Path(directory) / 'logical.jsonl'
        stored.write_bytes(b'')

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_POST(self):
                size = int(self.headers['Content-Length'])
                assert size <= 6 << 20
                data = self.rfile.read(size)
                started = time.thread_time()
                data = decode(data)
                cpu['decode_s'] += time.thread_time() - started
                assert len(data) <= 6 << 20
                body = json.loads(data)
                logical = base64.b64decode(body['bytes'], validate=True)
                with stored.open('ab') as target:
                    assert target.tell() == body['offset']
                    target.write(logical)
                    target.flush()
                    os.fsync(target.fileno())
                response = json.dumps({'length': body['offset']+len(logical), 'generation': body['generation']}).encode()
                self.send_response(200)
                self.send_header('Content-Length', str(len(response)))
                self.end_headers()
                self.wfile.write(response)
                self.wfile.flush()

            def do_GET(self):
                offset = int(self.path.split('=')[-1])
                with stored.open('rb') as source:
                    source.seek(offset)
                    response = source.read(4096)
                self.send_response(200)
                self.send_header('Content-Length', str(len(response)))
                self.end_headers()
                self.wfile.write(response)
                self.wfile.flush()

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        connection = Connection('127.0.0.1', server.server_port)
        # Disable Nagle in both directions; otherwise delayed ACK dominates
        # small requests and obscures codec/storage costs on loopback.
        Handler.disable_nagle_algorithm = True
        offset = 0
        json_bytes = 0
        payload_bytes = 0
        head = hashlib.sha256(corpus[:4096]).hexdigest()

        def post(logical):
            nonlocal offset, json_bytes, payload_bytes
            started = time.perf_counter()
            body = json.dumps({'root':'codex','path':'archived_sessions/synthetic.jsonl','offset':offset,'head_sha256':head,'generation':'a'*32,'bytes':base64.b64encode(logical).decode(),'replace':False}, separators=(',', ':')).encode()
            encoded_at = time.thread_time()
            data = encode(body)
            cpu['encode_s'] += time.thread_time()-encoded_at
            json_bytes += len(body)
            payload_bytes += len(data)
            connection.request('POST', '/v1/mirror/append', data, {'Content-Type':'application/json', 'Content-Encoding':mode})
            response = connection.getresponse()
            assert response.status == 200
            ack = json.loads(response.read())
            offset += len(logical)
            assert ack['length'] == offset
            return (time.perf_counter()-started)*1000

        process_cpu = time.process_time()
        started = time.perf_counter()
        for chunk in chunks:
            post(chunk)
        history_seconds = time.perf_counter()-started
        history_wire = connection.wire_bytes
        assert stored.read_bytes() == corpus
        live = b'{"type":"user","message":{"role":"user","content":"synthetic live append"}}\n'
        live_ms = [post(live) for _ in range(100)]
        read_ms = []
        for index in range(100):
            at = (index*7919) % max(1, len(corpus)-4096)
            before = time.perf_counter()
            connection.request('GET', f'/read?offset={at}')
            response = connection.getresponse()
            assert response.read() == corpus[at:at+4096]
            read_ms.append((time.perf_counter()-before)*1000)
        total_cpu = time.process_time()-process_cpu
        connection.close()
        server.shutdown()
        server.server_close()
    return {'mode':mode,'gzip_level':1 if mode=='gzip' else None,'zstd_level':3 if mode=='zstd' else None,'fixture_count':len(files),'corpus_sha256':corpus_hash,'logical_history_bytes':len(corpus),'history_requests':len(chunks),'history_wire_bytes':history_wire,'all_json_bytes':json_bytes,'all_encoded_body_bytes':payload_bytes,'history_seconds':history_seconds,'history_mib_per_s':len(corpus)/(1 << 20)/history_seconds,'process_cpu_s':total_cpu,**cpu,'peak_rss_kib':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'live_p50_ms':statistics.median(live_ms),'live_p95_ms':percentile(live_ms,.95),'read_p50_ms':statistics.median(read_ms),'read_p95_ms':percentile(read_ms,.95)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mib', type=int, default=16)
    parser.add_argument('--mode', choices=['identity','gzip','zstd'])
    args = parser.parse_args()
    if not 1 <= args.mib <= 256:
        parser.error('--mib must be between 1 and 256')
    if args.mode:
        print(json.dumps(run(args.mode, args.mib), sort_keys=True))
    else:
        results = [json.loads(subprocess.check_output([sys.executable, __file__, '--mib', str(args.mib), '--mode', mode])) for mode in ['identity','gzip','zstd']]
        print(json.dumps({'scope':'experimental Python loopback receiver; no Hub speedup claim','python':sys.version.split()[0],'results':results}, indent=2))


if __name__ == '__main__':
    main()
