"""Hermetic test of game-process tracking (Stop, ending a game that hangs after its
window closes) and of the optional AI helper against a fake Ollama server.
Usage: python3 tests/smoke_ai_proc.py <empty work dir>"""
import http.server, json, os, shutil, subprocess, sys, threading, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, gameproc, ai, launchguard
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ"); (sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True)

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()
for mid, name, on, files in (("tweaks", "powerofthree's Tweaks", False, {"SKSE/Plugins/po3_Tweaks.dll": "d"}),
                             ("flm", "FormList Manipulator - FLM", True, {"SKSE/Plugins/FormListManipulator.dll": "d"}),
                             ("other", "Some Armour", True, {"meshes/a.nif": "n"})):
    m = Mod(id=mid, name=name, enabled=on, version="1.0")
    for rel, d in files.items():
        f = inst.mod_path(m) / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_text(d)
    inst.mods.append(m)
inst.profiles[inst.active_profile] = {"members": ["flm", "other"]}
inst.save()

# ---- a stand-in game process: a copy of `sleep` called SkyrimSE.exe
fake = work / "bin/SkyrimSE.exe"; fake.parent.mkdir()
shutil.copy(shutil.which("sleep"), fake)
proc = subprocess.Popen([str(fake), "600"])
time.sleep(0.3)
pids = gameproc.game_pids(inst)
print("game processes found:", pids, "| ours:", proc.pid)
assert proc.pid in pids

# ---- the AI: a fake Ollama
asked = []
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        body = json.dumps({"models": [{"name": "tiny:1b", "capabilities": ["completion", "thinking"]}]}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        asked.append(req)
        self.send_response(200); self.end_headers()
        for piece in ("Tick ", "powerofthree's Tweaks ", "under Show all downloaded."):
            self.wfile.write((json.dumps({"message": {"content": piece}, "done": False}) + "\n").encode())
            self.wfile.flush(); time.sleep(0.05)
        self.wfile.write((json.dumps({"message": {"content": ""}, "done": True}) + "\n").encode())
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
URL = f"http://127.0.0.1:{srv.server_port}"
print("models:", ai.list_models(URL))
found = ai.lookup(inst, "po3's Tweaks is required for FormListManipulator to work")
print("lookup:\n" + found)
assert "powerofthree's Tweaks 1.0: downloaded but NOT in this profile" in found and "FormList Manipulator" in found
try:
    ai.list_models("http://127.0.0.1:1")
    raise SystemExit("should fail")
except ai.AIError as e:
    print("no Ollama:", e)

from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
QMessageBox.information = staticmethod(lambda *a, **k: None)
from kiln import gui, doctor


def wait(cond, secs=20):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()


w = gui.MainWindow(); w.resize(1300, 850)
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)

# ---- hang: the window was there, then went away while the process stayed
windows = {"has": True}
gameproc.has_window = lambda pids: windows["has"]
launchguard.game_running = lambda i: proc.poll() is None
w._watch_launch(inst); w.play_timer.stop()          # as after pressing Play; polled by hand below
w.playing.update(t0=time.time() - 60, seen_game=True, reported=True)
w._poll_launch()
print("button while running:", w.header.play_btn.text())
assert "Stop" in w.header.play_btn.text()
windows["has"] = False
w._poll_launch()                       # window gone: start counting
w.playing["no_window_since"] = time.time() - 11
w._poll_launch()                       # still gone after 10s: end it
print("hung game ended:", wait(lambda: proc.poll() is not None, 10), "| message:", w.statusBar().currentMessage()[:70])
assert proc.poll() is not None
w._poll_launch()
assert "Play" in w.header.play_btn.text()

# Stop button on a running game
proc2 = subprocess.Popen([str(fake), "600"]); time.sleep(0.3)
launchguard.game_running = lambda i: proc2.poll() is None
w.stop_game(inst)
assert wait(lambda: proc2.poll() is not None, 10)
print("Stop button ended the game ✔")

# ---- AI in the window
w.settings.set("ai_enabled", True); w.settings.set("ai_url", URL)
w.ai_url.setText(URL); w.ai_on.setChecked(True); w.refresh_ai_models()
assert wait(lambda: w.ai_model.count() == 1)
print("Settings model list:", [w.ai_model.itemText(i) for i in range(w.ai_model.count())],
      "| saved:", w.settings.get("ai_model"), w.settings.get("ai_model_thinking"))
w.go_to(w.doctor_tab); w.doctor_tab.refresh()
assert wait(lambda: w.doctor_tab.checked_at)
assert not w.doctor_tab.ask_btn.isHidden()
w.ask_ai(None, w.doctor_tab.issues)
dlg = w._ai_dlg
dlg.q.setPlainText("po3's Tweaks is required for FormListManipulator. What do I do?")
dlg.ask()
assert wait(lambda: dlg.job is None and "Show all downloaded" in dlg.view.toPlainText(), 15)
print("AI answer shown:", dlg.view.toPlainText().splitlines()[-1])
req = asked[-1]
assert req["think"] is False and req["model"] == "tiny:1b"
assert "Kiln looked up the mods mentioned" in req["messages"][-1]["content"]
assert "SETUP DETAILS" in req["messages"][0]["content"]
print("sent: system prompt + setup, question + lookup ✔ (think off for thinking models)")
srv.shutdown()
print("ai/proc smoke OK")
