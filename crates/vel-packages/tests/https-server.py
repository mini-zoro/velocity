"""Loopback-only TLS fixture; this interpreter is a test dependency, never a package runtime."""
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import ssl
import sys
import time

root, certificate, key, ready = map(Path, sys.argv[1:])


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        name = self.path.removeprefix("/")
        forbidden = ("Authorization", "Proxy-Authorization", "Cookie", "Referer")
        with (root / "requests.log").open("a", encoding="utf-8") as log:
            print(name, *[key for key in forbidden if key in self.headers], file=log)
        if name == "stall":
            time.sleep(60)
            return
        if "/" in name or ".." in name or not (root / name).is_file():
            self.send_error(404)
            return
        self.connection.sendall((root / name).read_bytes())
        self.close_connection = True

    def log_message(self, *_args):
        pass


server = HTTPServer(("127.0.0.1", 0), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.load_cert_chain(certificate, key)
server.socket = context.wrap_socket(server.socket, server_side=True)
ready.write_text(str(server.server_port))
server.serve_forever()
