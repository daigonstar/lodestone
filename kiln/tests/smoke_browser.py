"""Hermetic test of Kiln's in-app Nexus browser: free-account collection run with
background downloads, pause/resume, and plain file downloads. Pages come from a
local fake server; the test clicks "Slow download" itself, as a user would.
Usage: python3 tests/smoke_browser.py <empty work dir>"""
import json, os, struct, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve()
work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", QT_QPA_PLATFORM="offscreen", QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "cfg"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from kiln import steam, nexus, collections as nc, downloader, desktop
lib = work / "lib"
steam.libraries = lambda: [lib]
steam.steam_roots = lambda: []
desktop.is_nxm_handler = lambda: True        # never touch the real xdg-mime setup

def plugin(masters=()):
    sub = b"".join(b"MAST" + struct.pack("<H", len(m) + 1) + m.encode() + b"\0" for m in masters)
    body = b"HEDR" + struct.pack("<H", 12) + b"\0" * 12 + sub
    return b"TES4" + struct.pack("<II", len(body), 0) + b"\0" * 12 + body

game = lib / "steamapps/common/Skyrim Special Edition"
(game / "Data").mkdir(parents=True)
(game / "SkyrimSE.exe").write_bytes(b"MZ")
(game / "Data/Skyrim.esm").write_bytes(plugin())
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True)

# --- fake Nexus files: mod 1 (plain), mod 2 (FOMOD), mod 3 (optional) ---------
files = work / "nexusfiles"
files.mkdir()
with zipfile.ZipFile(files / "1-11.zip", "w") as z:
    z.writestr("Alpha/Alpha.esp", plugin(["Skyrim.esm"]))
    z.writestr("Alpha/textures/shared.dds", "ALPHA")
cfg = '''<config><moduleName>Beta</moduleName><installSteps order="Explicit"><installStep name="Main">
<optionalFileGroups><group name="Style" type="SelectExactlyOne"><plugins order="Explicit">
<plugin name="Light"><files><file source="light.dds" destination="textures/style.dds"/></files><typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
<plugin name="Dark"><files><file source="dark.dds" destination="textures/style.dds"/></files><typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
</plugins></group></optionalFileGroups></installStep></installSteps>
<requiredInstallFiles><file source="Beta.esp" destination="Beta.esp"/><file source="shared.dds" destination="textures/shared.dds"/></requiredInstallFiles></config>'''
with zipfile.ZipFile(files / "2-22.zip", "w") as z:
    z.writestr("fomod/ModuleConfig.xml", cfg)
    z.writestr("Beta.esp", plugin(["Skyrim.esm"]))
    z.writestr("light.dds", "LIGHT"); z.writestr("dark.dds", "DARK"); z.writestr("shared.dds", "BETA")
with zipfile.ZipFile(files / "3-33.zip", "w") as z:
    z.writestr("Gamma.esp", plugin())

manifest = {
    "info": {"name": "Test Collection", "installInstructions": "Have fun."},
    "mods": [
        {"name": "Alpha", "source": {"type": "nexus", "modId": 1, "fileId": 11, "md5": "a", "logicalFilename": "Alpha"}},
        {"name": "Beta", "source": {"type": "nexus", "modId": 2, "fileId": 22, "md5": "b", "logicalFilename": "Beta"},
         "choices": {"type": "fomod", "options": [{"name": "Main", "groups": [{"name": "Style", "choices": [{"name": "Dark", "idx": 1}]}]}]}},
        {"name": "Gamma", "optional": True, "source": {"type": "nexus", "modId": 3, "fileId": 33}},
        {"name": "Patch", "source": {"type": "bundle", "fileExpression": "Patch"}},
        {"name": "Some Tool", "source": {"type": "browse", "url": "https://example.org/tool", "instructions": "Run it once."}},
    ],
    # Beta must load BEFORE Alpha (so Alpha's shared.dds wins), even though Alpha is listed first
    "modRules": [{"type": "before", "source": {"fileMD5": "b"}, "reference": {"fileMD5": "a"}}],
    "plugins": [{"name": "Patch.esp", "enabled": True}, {"name": "Beta.esp", "enabled": True},
                {"name": "Alpha.esp", "enabled": False}],
}
coll = work / "coll.zip"
with zipfile.ZipFile(coll, "w") as z:
    z.writestr("collection.json", json.dumps(manifest))
    z.writestr("bundled/Patch/Patch.esp", plugin(["Alpha.esp", "Beta.esp"]))

REV = nc.CollectionRevision(slug="test01", domain="skyrimspecialedition", name="Test Collection",
    author="me", revision=3, description_md="**hello**", summary="s", download_path="/v2/x",
    mods=[nc.CollectionMod(1, 11, "Alpha", "Alpha main", "1.0", False, 1, "MAIN"),
          nc.CollectionMod(2, 22, "Beta", "Beta main", "2.0", False, 1, "MAIN"),
          nc.CollectionMod(3, 33, "Gamma", "Gamma", "1", True, 1, "MAIN")], bundled_count=1)
nc.revision = lambda d, s, r=None: REV
nc.manifest_download_url = lambda key, path: "fake://collection"
nexus.mod_details = lambda d, g, mid: nexus.ModDetails(mid, f"Mod{mid}", files=[
    nexus.NexusFile(mid * 11, f"file{mid}", "1.0", "MAIN", 0, 1, f"{mid}-{mid * 11}.zip")])
nexus.download_links = lambda key, d, mid, fid, k=None, e=None: [f"fake://{mid}-{fid}"]
real_download = downloader.download
def fake_download(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    url = url()[0] if callable(url) else url
    src = coll if url == "fake://collection" else files / (url[7:] + ".zip")
    dest.mkdir(parents=True, exist_ok=True)
    out = dest / (fallback_name or src.name)
    out.write_bytes(src.read_bytes())
    if url != "fake://collection":
        time.sleep(1.5)   # slow enough to see the next page open meanwhile
    return out
downloader.download = fake_download

from PySide6.QtCore import QEventLoop, QTimer, Qt, QCoreApplication
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
infos, errors, opened = [], [], []
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
QMessageBox.information = staticmethod(lambda p, t, m, *a: infos.append(m))
QMessageBox.critical = staticmethod(lambda p, t, m, *a: errors.append((t, m)))
QDesktopServices.openUrl = staticmethod(lambda u: opened.append(u.toString()) or True)
from kiln import gui

def wait(cond, secs=30):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop()
    return cond()

w = gui.MainWindow()
w.settings.set("nexus_api_key", "fake")
assert w.inst and w.inst.game_dir.is_relative_to(work), "real game visible!"
data = game / "Data"
pt = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition/plugins.txt"


# ---- local fake "Nexus" pages -------------------------------------------------
import http.server, threading
www = work / "www"; www.mkdir()
(www / "page.html").write_text("""<html><body><div style="height:3000px">files…</div>
<button id=s onclick="var q=new URLSearchParams(location.search);
location.href='nxm://skyrimspecialedition/mods/'+q.get('mod')+'/files/'+q.get('file_id')+'?key=k&expires=1&user_id=1'">
Slow download</button></body></html>""")
(www / "plain.html").write_text('<a id=d href="3-33.zip" download>Manual download</a>')
(www / "3-33.zip").write_bytes((files / "3-33.zip").read_bytes())
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(s, *a, **k): super().__init__(*a, directory=str(www), **k)
    def log_message(s, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{srv.server_address[1]}"
nexus.manual_download_page = lambda d, mid, fid: f"{BASE}/page.html?mod={mid}&file_id={fid}"

w.user = nexus.User("tester", False, 1)
b = w.browser()
print("browser available:", b is not None)

def js(code):
    out = []
    b.page_.runJavaScript(code, 0, lambda r: out.append(r))
    wait(lambda: out, 5)
    return out[0] if out else None

def page_for(mid):
    return wait(lambda: f"mod={mid}&" in b.view.url().toString() and
                js("document.getElementById('s') && document.getElementById('s').style.outline") != "", 15)

def click():
    js("document.getElementById('s').click(); 1")

# ---------------- free run: background downloads ----------------
w.tabs.setCurrentWidget(w.coll_tab)
w.show_collection("test01")
wait(lambda: w.c_rev is not None and w.c_mods.topLevelItemCount() == 3)
for i in range(3):
    w.c_mods.topLevelItem(i).setCheckState(0, gui.Qt.Checked)
w.install_collection()
print("page 1 (Alpha) shown + highlighted:", page_for(1),
      "| outline:", js("document.getElementById('s').style.outline"))
print("run banner:", b.run_label.text())
click()
print("page 2 (Beta) opened while Alpha downloads:", page_for(2),
      "| Alpha status:", next(e["status"] for e in w.crun["entries"] if e.get("mod") and e["mod"].mod_id == 1))
# pause here, with Alpha in the background and Beta waiting
w._crun_stop()
wait(lambda: w.inst.find_nexus(1) is not None, 20)
print("after pause: Alpha installed:", w.inst.find_nexus(1) is not None, "| run file:",
      json.loads((w.inst.staging / "collections/test01.run.json").read_text()))
print("progress text:", w.c_progress.text())

# ---------------- resume (as after a restart) ----------------
w.show_collection("test01")
wait(lambda: w.c_rev is not None and w.c_mods.topLevelItemCount() == 3)
print("resume button:", w.c_install.text(), "| ticked:",
      [w.c_mods.topLevelItem(i).text(0) for i in range(3) if w.c_mods.topLevelItem(i).checkState(0) == gui.Qt.Checked])
infos.clear()
w.install_collection()
print("page Beta:", page_for(2)); click()
print("page Gamma:", page_for(3))
# keyboard instead of the mouse: the button has focus, and the download key presses it
print("Slow download focused:", wait(lambda: js("document.activeElement && document.activeElement.id") == "s", 5),
      "| key:", b.key_shortcut.key().toString(), "| hint mentions it:", "F2" in b.hint.text())
b.activateWindow(); b.press_download()
b.press_download()                      # a second press right away is ignored (one press per file)
print("finished:", wait(lambda: infos, 40))
print("summary:", infos[-1].replace("\n", " | ") if infos else None)
print("installed:", sorted((m.name, m.collection) for m in w.inst.mods))
print("style.dds:", (data / "textures/style.dds").read_bytes(), "| run file gone:",
      not (w.inst.staging / "collections/test01.run.json").exists())
print("errors:", errors)

# ---------------- plain (non-nxm) download lands in the game's downloads ----------------
g3 = w.inst.find_nexus(3); w.inst.remove(g3)
w.open_browser(f"{BASE}/plain.html")
b.context.update(mod_id=3, file_id=33)
wait(lambda: "plain.html" in b.view.url().toString() and js("!!document.getElementById('d')"), 10)
js("document.getElementById('d').click(); 1")
print("plain download installed:", wait(lambda: w.inst.find_nexus(3) is not None, 20),
      "| in downloads:", sorted(p.name for p in w.inst.downloads_dir.iterdir() if p.suffix == ".zip"))
print("errors:", errors)
srv.shutdown()
