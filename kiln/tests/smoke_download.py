"""Downloader: resumes after dropped connections, refreshes expired links, gives a clear error."""
import http.server, os, socket, sys, tempfile, threading
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from kiln import downloader

DATA = os.urandom(3_000_000)
hits = {"n": 0, "ranges": []}

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        hits["n"] += 1
        if self.path.startswith("/expired"):
            self.send_response(403); self.end_headers(); return
        if self.path.startswith("/busy"):
            self.send_response(503); self.end_headers(); return
        start = 0
        rng = self.headers.get("Range")
        if rng:
            start = int(rng.split("=")[1].rstrip("-")); hits["ranges"].append(start)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{len(DATA)-1}/{len(DATA)}")
        else:
            self.send_response(200)
        self.send_header("Content-Length", str(len(DATA) - start))
        self.send_header("Content-Disposition", 'attachment; filename="Mod 1.0.zip"')
        self.end_headers()
        body = DATA[start:]
        if self.path.startswith("/flaky") and hits["n"] <= 2:
            self.wfile.write(body[:700_000]); self.wfile.flush()
            self.connection.shutdown(socket.SHUT_RDWR); return   # drop mid-file
        self.wfile.write(body)

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{srv.server_port}"
nosleep = lambda s: None

with tempfile.TemporaryDirectory() as d:
    d = Path(d)
    p = downloader.download(base + "/flaky", d / "a", sleep=nosleep)
    assert p.read_bytes() == DATA and p.name == "Mod 1.0.zip", p
    assert hits["ranges"] and hits["ranges"][0] > 0, hits
    print("dropped twice, resumed from", hits["ranges"], "→ ok")

    calls = []
    def links():
        calls.append(1)
        return [base + ("/expired" if len(calls) == 1 else "/ok")]
    p = downloader.download(links, d / "b", sleep=nosleep)
    assert p.read_bytes() == DATA and len(calls) == 2
    print("expired link refreshed → ok")

    try:
        downloader.download(base + "/busy", d / "c", sleep=nosleep, attempts=3)
        raise SystemExit("should fail")
    except downloader.DownloadError as e:
        print("busy server:", e)
    try:
        downloader.download("http://127.0.0.1:1/x", d / "c", sleep=nosleep, attempts=2)
        raise SystemExit("should fail")
    except downloader.DownloadError as e:
        print("refused:", e)
    assert not list((d / "c").glob("*.part"))
    print(downloader.describe(Exception("HTTPSConnectionPool(host='supporter-files.nexus-cdn.com', port=443): Read timed out. (read timeout=30)")))
print("download smoke OK")
