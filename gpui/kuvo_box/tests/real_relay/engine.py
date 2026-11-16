# A stand-in engine for a test box: says ses_test is there, takes prompts, logs what each carried.
import json, sys, http.server
log = open(sys.argv[2], "a")
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, value, status=200):
        body = json.dumps(value).encode()
        self.send_response(status); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        path = self.path.split("?")[0]
        if path.startswith("/api/session/ses_test") and path.count("/") == 3:
            return self.reply({"data": {"id": "ses_test", "directory": "/tmp", "location": {"directory": "/tmp"}, "time": {"created": 1, "updated": 1}}})
        if path == "/api/session/active": return self.reply({"data": {}})
        if path.endswith("/inbox"): return self.reply({"data": []})
        if path.endswith("/message"): return self.reply({"data": []})
        return self.reply({"data": []})
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0); body = self.rfile.read(n)
        if self.path.endswith("/prompt"):
            given = json.loads(body)
            files = [(f.get("name"), len(f.get("uri", "")), f.get("uri", "")[:40]) for f in given.get("files", [])]
            log.write(json.dumps({"id": given.get("id"), "text": given.get("text", "")[:80], "files": files}) + "\n"); log.flush()
        return self.reply({})
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
