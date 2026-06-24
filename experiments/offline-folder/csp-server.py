import http.server, socketserver, os, functools
CSP = os.environ.get("CSP", "")
class H(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        if CSP:
            self.send_header("Content-Security-Policy", CSP)
        super().end_headers()
H.extensions_map = {**H.extensions_map, ".wasm": "application/wasm", ".mjs": "text/javascript"}
Handler = functools.partial(H, directory="dist")
socketserver.ThreadingTCPServer.allow_reuse_address = True
with socketserver.ThreadingTCPServer(("", 8124), Handler) as httpd:
    httpd.serve_forever()
