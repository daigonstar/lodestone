#!/usr/bin/env python3
"""Plays Lodestone's part to check hello_plugin.py follows the contract in docs/PLUGINS.md.

Run it with:  python3 docs/examples/hello-plugin/test_hello_plugin.py
You can point it at your own plug-in instead:  ... test_hello_plugin.py ./my-plugin
"""
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

here = os.path.dirname(os.path.abspath(__file__))
command = [sys.executable, os.path.join(here, "hello_plugin.py")] if len(sys.argv) < 2 else [sys.argv[1]]
p = subprocess.Popen(command + ["--lodestone-bridge"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)


def line():
    return json.loads(p.stdout.readline())


def get(url, token=None):
    req = urllib.request.Request(url.split("#")[0], headers={"x-plugin-token": token} if token else {})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.headers, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.headers, e.read().decode()


def send(message):
    p.stdin.write(json.dumps(message) + "\n")
    p.stdin.flush()


hello = line()
assert hello["type"] == "hello" and hello["ui"].startswith("http://127.0.0.1:") and "#t=" in hello["ui"], hello
assert all(isinstance(x, str) for x in hello["permissions"])
base, token = hello["ui"].split("#t=")
print("hello ok:", hello["name"], hello["version"], hello["permissions"])

status, headers, body = get(base)
assert status == 200 and "frame-ancestors" in (headers.get("content-security-policy") or ""), (status, headers)
print("page ok, framing is restricted to Lodestone")
assert get(base.rstrip("/") + "/api/playing")[0] == 401, "the API must refuse a request without the token"
print("API refuses requests without the token")

# A request to Lodestone: answer it as Lodestone would after you approved game:read.
import threading
result = {}
t = threading.Thread(target=lambda: result.update(zip(("status", "headers", "body"), get(base.rstrip("/") + "/api/playing", token))))
t.start()
request = line()
assert request["type"] == "request" and request["method"] == "playing", request
send({"type": "response", "id": request["id"], "ok": {"title": "Hades II"}})
t.join(15)
assert result["status"] == 200 and json.loads(result["body"]) == {"ok": {"title": "Hades II"}}, result
print("request and response ok")

# An error answer (permission not approved) reaches the page as an error.
t = threading.Thread(target=lambda: result.update(zip(("status", "headers", "body"), get(base.rstrip("/") + "/api/playing", token))))
t.start()
request = line()
send({"type": "response", "id": request["id"], "error": "not allowed (approve it in Lodestone)"})
t.join(15)
assert json.loads(result["body"]) == {"error": "not allowed (approve it in Lodestone)"}, result
print("refusal handled")

send({"type": "event", "name": "theme", "data": {"--bg": "#123456", "--text": "#ffffff"}})
import time
time.sleep(0.3)
assert "#123456" in get(base.rstrip("/") + "/style.css")[2]
print("theme event ok")

# Closing the plug-in's input is how Lodestone says "quit".
p.stdin.close()
assert p.wait(timeout=5) == 0
print("quits when its input closes")
print("ALL OK")
