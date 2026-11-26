"""Hermetic test of a full collection install (Premium and free-account flows).
Only a fake Steam library is visible and every Nexus download is faked locally.
Usage: python3 tests/smoke_collections.py <empty work dir>"""
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
nexus.manual_download_page = lambda d, mid, fid: "about:blank"   # no real site in tests
nexus.download_links = lambda key, d, mid, fid, k=None, e=None: [f"fake://{mid}-{fid}"]
real_download = downloader.download
def fake_download(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    url = url()[0] if callable(url) else url
    src = coll if url == "fake://collection" else files / (url[7:] + ".zip")
    dest.mkdir(parents=True, exist_ok=True)
    out = dest / (fallback_name or src.name)
    out.write_bytes(src.read_bytes())
    return out
downloader.download = fake_download

from PySide6.QtCore import QEventLoop, QTimer
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

# ---------------- Premium run ----------------
w.user = nexus.User("tester", True, 1)
w.tabs.setCurrentWidget(w.coll_tab)
w.show_collection("test01")
wait(lambda: w.c_rev is not None)
print("listed:", [(w.c_mods.topLevelItem(i).text(0), w.c_mods.topLevelItem(i).checkState(0).name)
                  for i in range(w.c_mods.topLevelItemCount())])
w.install_collection()
ok = wait(lambda: infos)
print("premium run finished:", ok)
print("summary:", infos[-1].replace("\n", " | ") if infos else None)
print("statuses:", [(w.c_mods.topLevelItem(i).text(0), w.c_mods.topLevelItem(i).text(4))
                    for i in range(w.c_mods.topLevelItemCount())])
print("mod order:", [(m.name, m.collection) for m in w.inst.mods])
print("shared.dds:", (data / "textures/shared.dds").read_bytes(), "(Alpha should win)")
print("style.dds:", (data / "textures/style.dds").read_bytes(), "(saved choice: DARK)")
print("plugins.txt:", pt.read_text().splitlines()[2:])
print("recorded collections:", w.inst.collections)
print("errors:", errors)

# ---------------- free-account run (reinstall Gamma via nxm) ----------------
w.user = nexus.User("tester", False, 1)
infos.clear(); opened.clear()
w.show_collection("test01")
wait(lambda: w.c_rev is not None and w.c_mods.topLevelItemCount() == 3)
for i in range(w.c_mods.topLevelItemCount()):   # tick only the optional Gamma
    it = w.c_mods.topLevelItem(i)
    it.setCheckState(0, gui.Qt.Checked if it.text(0).startswith("Gamma") else gui.Qt.Unchecked)
w.install_collection()
wait(lambda: w.crun and w.crun.get("waiting"))
print("free: waiting for", w.crun["waiting"], "| opened:", opened)
w.handle_nxm("nxm://skyrimspecialedition/mods/3/files/33?key=k&expires=1&user_id=1")
print("free run finished:", wait(lambda: infos), "| Gamma installed:",
      any(m.name == "Mod3" or m.nexus_mod_id == 3 for m in w.inst.mods))
print("errors:", errors)
print("mods after both runs:", sorted(m.id for m in w.inst.mods))
