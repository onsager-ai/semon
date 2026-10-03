"""Experiment-only bridge from E2B HTTPS ingress to the reference receiver.

Run inside the independent, operation-owned E2B receiver sandbox. This is not
production ingress; receiver token authentication remains mandatory downstream.
"""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client

class Proxy(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path not in ('/v1/mirror/append', '/v1/mirror/facts'):
            self.send_error(404)
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
        except ValueError:
            self.send_error(400)
            return
        if length <= 0:
            self.send_error(400)
            return
        if length > 6 * 1024 * 1024:
            self.send_error(413)
            return
        c = http.client.HTTPConnection('127.0.0.1', 8735, timeout=120)
        c.request('POST', self.path, body=self.rfile.read(length), headers={
            'Authorization': self.headers.get('Authorization', ''),
            'Content-Type': 'application/json',
        })
        r = c.getresponse()
        body = r.read()
        self.send_response(r.status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        c.close()
    def log_message(self, *args):
        pass

ThreadingHTTPServer(('0.0.0.0', 8787), Proxy).serve_forever()
