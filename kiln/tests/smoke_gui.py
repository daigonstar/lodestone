"""Hermetic offscreen GUI smoke test. Only a fake Steam library is visible, so
real games are never touched. Usage: python3 tests/smoke_gui.py <empty work dir>"""
import json, os, struct, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve()
assert not any(work.iterdir()) if work.exists() else True, "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", QT_QPA_PLATFORM="offscreen", XDG_CONFIG_HOME=str(work / "cfg"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from kiln import steam
lib = work / "lib"
steam.libraries = lambda: [lib]          # hide every real Steam library
steam.steam_roots = lambda: []

def plugin(masters=(), esm=False):
    sub = b"".join(b"MAST" + struct.pack("<H", len(m) + 1) + m.encode() + b"\0" for m in masters)
    body = b"HEDR" + struct.pack("<H", 12) + b"\0" * 12 + sub
    return b"TES4" + struct.pack("<II", len(body), 1 if esm else 0) + b"\0" * 12 + body

game = lib / "steamapps/common/Skyrim Special Edition"
(game / "Data").mkdir(parents=True)
(game / "SkyrimSE.exe").write_bytes(b"MZ")
(game / "Data/Skyrim.esm").write_bytes(plugin(esm=True))
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True)
arc = work / "fancy.zip"
cfg = '''<config><moduleName>Fancy</moduleName>
<requiredInstallFiles><folder source="core" destination=""/></requiredInstallFiles>
<installSteps order="Explicit"><installStep name="Pick"><optionalFileGroups><group name="Colour" type="SelectExactlyOne"><plugins order="Explicit">
<plugin name="Red"><description>red</description><files><file source="opt\\red.dds" destination="textures\\colour.dds"/></files><typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
<plugin name="Blue"><description>blue</description><files><file source="opt\\Blue.dds" destination="textures\\colour.dds"/></files><typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
</plugins></group></optionalFileGroups></installStep></installSteps></config>'''
with zipfile.ZipFile(arc, "w") as z:
    z.writestr("Fancy/fomod/ModuleConfig.xml", cfg.encode("utf-16"))
    z.writestr("Fancy/core/Fancy.esp", plugin(["Skyrim.esm"]))
    z.writestr("Fancy/opt/red.dds", "R")
    z.writestr("Fancy/opt/blue.dds", "B")

from PySide6.QtWidgets import QApplication, QMessageBox, QRadioButton
from PySide6.QtTest import QTest
app = QApplication([])
errors = []
QMessageBox.critical = staticmethod(lambda p, t, m, *a: errors.append((t, m)))
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
from kiln import gui, fomod_dialog, deploy

def fake_exec(self):  # pick "Red" in the wizard, then Install
    for b in self.scroll.widget().findChildren(QRadioButton):
        if b.text().startswith("Red"):
            b.setChecked(True)
    self.next()
    return self.result()
fomod_dialog.FomodDialog.exec = fake_exec

def wait(cond, secs=30):
    """Run the real event loop (like the app does) until cond() or timeout."""
    from PySide6.QtCore import QEventLoop, QTimer
    loop, end = QEventLoop(), time.time() + secs
    def check():
        if cond() or time.time() > end:
            loop.quit()
    t = QTimer(interval=50, timeout=check)
    t.start()
    loop.exec()
    t.stop()
    return cond()

w = gui.MainWindow()
w.show()
assert w.game.id == "skyrimse" and w.inst, "fake Skyrim not detected"
assert all(i.game_dir.is_relative_to(work) for i in w.instances.values()), "real game visible!"
print("detected:", w.inst.game_dir.relative_to(work))
t0 = time.time()
w.install_archive(arc, {"name": "Fancy"})
data = game / "Data"
ok = wait(lambda: (data / "Fancy.esp").exists() and "Deployed" in w.statusBar().currentMessage())
print(f"install+deploy {'OK' if ok else 'TIMEOUT'} in {time.time() - t0:.1f}s:", w.statusBar().currentMessage())
print("Data:", sorted(str(p.relative_to(data)) for p in data.rglob("*") if p.is_file()))
cd = next((p for p in data.rglob("*") if p.name.lower() == "colour.dds"), None)
print("colour.dds:", cd.read_bytes() if cd else None)
pt = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition/plugins.txt"
print("plugins.txt:", pt.read_text().splitlines()[2:] if pt.exists() else None)
print("plugins tab:", [w.plugin_list.item(i).text() for i in range(w.plugin_list.count())])
print("errors:", errors)
print("purge:", deploy.purge(w.inst).summary(), "| Data now:", sorted(p.name for p in data.iterdir()))
