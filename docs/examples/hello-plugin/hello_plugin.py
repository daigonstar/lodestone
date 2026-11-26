#!/usr/bin/env python3
"""A tiny Lodestone plug-in, to show the whole contract in one file (see docs/PLUGINS.md).

It serves one page on 127.0.0.1, says hello to Lodestone, asks which game you're playing (the `playing`
request, permission `game:read`), shows Lodestone's colours, and quits when Lodestone closes its input.
It only uses Python's standard library. Lodestone starts it with `--lodestone-bridge`; without that
argument there's no bridge and it just prints the address of its page.
"""
import json
import secrets
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN = secrets.token_urlsafe(32)  # per launch; Lodestone gets it in the hello, never over the network
BRIDGE = "--lodestone-bridge" in sys.argv
theme = {}
pending = {}  # request id -> (event, [reply])
out_lock = threading.Lock()
next_id = 0
PAGE = """<!doctype html><meta charset="utf-8"><title>Hello plug-in</title>
<link rel="stylesheet" href="/style.css"><script src="/app.js" defer></script>
<h1>Hello from a plug-in</h1><p id="game">Asking Lodestone…</p>"""
SCRIPT = """const token = location.hash.slice(3); // the page was opened as ...#t=<token>
fetch("/api/playing", { headers: { "x-plugin-token": token } }).then(r => r.json()).then(j => {
  document.getElementById("game").textContent = j.error ? "Lodestone said: " + j.error : j.ok ? "You're playing " + j.ok.title : "You're not playing anything from Lodestone.";
});"""


def send(message):
    """One JSON message per line on standard output (this is the only thing printed there)."""
    with out_lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def ask_lodestone(method, params=None):
    """A request to Lodestone; blocks until it answers (it refuses anything you haven't approved)."""
    global next_id
    next_id += 1
    ev, reply = threading.Event(), []
    pending[next_id] = (ev, reply)
    send({"type": "request", "id": next_id, "method": method, "params": params or {}})
    if not ev.wait(30):
        return {"error": "Lodestone didn't answer"}
    return reply[0]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # nothing about requests goes to logs
        pass

    def reply(self, code, body, kind):
        data = body.encode()
        self.send_response(code)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        # Only Lodestone's window may frame this page; the page loads nothing from anywhere else.
        self.send_header("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors tauri://localhost http://tauri.localhost")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        # Only requests addressed to exactly this address are answered (another web page can't reach us
        # through a rebound name).
        if self.headers.get("host") != "127.0.0.1:%d" % self.server.server_port:
            return self.reply(421, "wrong host", "text/plain")
        if self.path == "/":
            return self.reply(200, PAGE, "text/html; charset=utf-8")
        if self.path == "/app.js":
            return self.reply(200, SCRIPT, "text/javascript")
        if self.path == "/style.css":
            css = "body { background: %s; color: %s; font-family: sans-serif; padding: 1em; }" % (theme.get("--bg", "#0f1117"), theme.get("--text", "#e8ebf2"))
            return self.reply(200, css, "text/css")
        # Everything else needs the per-launch token.
        if not secrets.compare_digest(self.headers.get("x-plugin-token", ""), TOKEN):
            return self.reply(401, json.dumps({"error": "not allowed"}), "application/json")
        if self.path == "/api/playing":
            return self.reply(200, json.dumps(ask_lodestone("playing")), "application/json")
        self.reply(404, "not found", "text/plain")


def read_lodestone():
    """Lodestone's lines: answers to our requests, and events. When its input closes, we quit."""
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("type") == "response" and msg.get("id") in pending:
            ev, reply = pending.pop(msg["id"])
            reply.append({"error": msg["error"]} if "error" in msg else {"ok": msg.get("ok")})
            ev.set()
        elif msg.get("type") == "event" and msg.get("name") == "theme":
            theme.update(msg.get("data") or {})
    import os
    os._exit(0)


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)  # port 0: any free port
    ui = "http://127.0.0.1:%d/#t=%s" % (server.server_port, TOKEN)
    if not BRIDGE:
        print(ui)
        return server.serve_forever()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    send({"type": "hello", "name": "Hello plug-in", "version": "0.1.0", "ui": ui, "permissions": ["game:read"]})
    read_lodestone()


if __name__ == "__main__":
    main()
