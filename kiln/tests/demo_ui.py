"""Render Kiln's new UI against a fake Skyrim install and save screenshots.
Usage: python3 tests/demo_ui.py <empty work dir> <screenshot dir>"""
import json, os, struct, sys, time
from pathlib import Path
work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
os.environ.update(KILN_NO_WELCOME="1", QT_QPA_PLATFORM="offscreen", QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu",
                  XDG_CONFIG_HOME=str(work / "cfg"), XDG_CACHE_HOME=str(work / "cache"),
                  XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, os.path.expanduser("~/projects/kiln"))
from kiln import steam, desktop
lib = work / "lib"
steam.libraries = lambda: [lib]
desktop.is_nxm_handler = lambda: True
def plugin(masters=(), flags=0):
    sub = b"".join(b"MAST" + struct.pack("<H", len(m) + 1) + m.encode() + b"\0" for m in masters)
    body = b"HEDR" + struct.pack("<H", 12) + b"\0" * 12 + sub
    return b"TES4" + struct.pack("<II", len(body), flags) + b"\0" * 12 + body
game = lib / "steamapps/common/Skyrim Special Edition"
(game / "Data").mkdir(parents=True, exist_ok=True)
(game / "SkyrimSE.exe").write_bytes(b"MZ"); (game / "Data/Skyrim.esm").write_bytes(plugin())
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True, exist_ok=True)
st = lib / "kiln/skyrimse"; mods = st / "mods"
spec = [  # id, name, ver, author, nexus id, category, files
 ("SkyUI", "SkyUI", "5.2SE", "schlangster", 12604, "User Interface", {"SkyUI_SE.esp": plugin(["Skyrim.esm"]), "interface/skyui/config.txt": b"a"*4000, "SkyUI_SE.bsa": b"x"*900000}),
 ("USSEP", "Unofficial Skyrim Special Edition Patch", "4.3.2", "Arthmoor", 266, "Patches", {"unofficial skyrim special edition patch.esp": plugin(["Skyrim.esm"], 1), "textures/landscape/dirt01.dds": b"u"*300000}),
 ("SMIM", "Static Mesh Improvement Mod", "2.08", "Brumbek", 659, "Models and Textures", {"textures/landscape/dirt01.dds": b"s"*500000, "meshes/clutter/bucket.nif": b"m"*20000, "textures/clutter/bucket.dds": b"t"*80000}),
 ("Noble", "Noble Skyrim Mod HD-2K", "1.1", "Gamwich", 21423, "Models and Textures", {"textures/landscape/dirt01.dds": b"n"*700000, "textures/architecture/whiterun/wrwall.dds": b"w"*400000}),
 ("Sounds", "Immersive Sounds - Compendium", "1.3", "lilith", 523, "Audio", {"Immersive Sounds - Compendium.esp": plugin(["Skyrim.esm"])}),
 ("MyTweaks", "My ini tweaks", "", "", None, "Utilities", {"MyTweaks.esp": plugin(["Skyrim.esm", "SkyUI_SE.esp"])}),
]
state = {"game": "skyrimse", "mods": [], "plugins": [], "collections": [{"slug": "abc123", "name": "Essentials", "revision": 2, "installed_at": 0}]}
for i, (mid, name, ver, author, nid, cat, files) in enumerate(spec):
    for rel, data in files.items():
        f = mods / mid / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_bytes(data)
    state["mods"].append(dict(id=mid, name=name, version=ver, author=author, nexus_mod_id=nid,
        nexus_file_id=1000 + i if nid else None, enabled=mid != "Sounds", installed_at=int(time.time()) - i * 86400 * 3,
        category=cat, collection="abc123" if mid in ("SkyUI", "USSEP") else "",
        notes="Keep this below SMIM." if mid == "Noble" else "", archive=f"{mid}.7z" if nid else ""))
st.mkdir(parents=True, exist_ok=True); (st / "kiln.json").write_text(json.dumps(state))
from PySide6.QtCore import QCoreApplication, Qt, QEventLoop, QTimer
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview
from PySide6.QtWidgets import QApplication
app = QApplication([])
from kiln import gui, nexus
def wait(s):
    l = QEventLoop(); QTimer.singleShot(int(s * 1000), l.quit); l.exec()
w = gui.MainWindow(); w.resize(1500, 920); w.show()
w.updates["SkyUI"] = nexus.NexusFile(9999, "SkyUI", "5.3SE", "MAIN", int(time.time()), 1, "x.7z")
w.endorsements[("skyrimspecialedition", 266)] = "Endorsed"
w.refresh_mods()
wait(1)
for i in range(w.mod_tree.topLevelItemCount()):
    if w.mod_tree.topLevelItem(i).data(0, Qt.UserRole) == "SMIM":
        w.mod_tree.setCurrentItem(w.mod_tree.topLevelItem(i))
wait(6)
out = Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
w.grab().save(str(out / "1-overview.png"))
w.mod_panel.tabs.setCurrentIndex(2); w.mod_panel.conf_tree.expandAll(); wait(0.5)
w.grab().save(str(out / "2-conflicts.png"))
w.mod_panel.tabs.setCurrentIndex(1); wait(0.3); w.grab().save(str(out / "3-files.png"))
w.mod_panel.tabs.setCurrentIndex(4); wait(0.3); w.grab().save(str(out / "4-info.png"))
# conflict resolution: make Noble lose to SMIM
for i in range(w.mod_tree.topLevelItemCount()):
    if w.mod_tree.topLevelItem(i).data(0, Qt.UserRole) == "SMIM":
        w.mod_tree.setCurrentItem(w.mod_tree.topLevelItem(i))
w.mod_panel.tabs.setCurrentIndex(2)
for i in range(w.mod_panel.conf_tree.topLevelItemCount()):
    it = w.mod_panel.conf_tree.topLevelItem(i)
    if it.data(0, Qt.UserRole) == "Noble": w.mod_panel.conf_tree.setCurrentItem(it)
w.mod_panel._resolve(True); wait(0.5)
print("order after 'make SMIM win vs Noble':", [m.id for m in w.inst.mods])
w.mod_filter.setCurrentIndex(w.mod_filter.findData("cat:Models and Textures")); wait(0.3)
print("filter Models and Textures:", [w.mod_tree.topLevelItem(i).text(0) for i in range(w.mod_tree.topLevelItemCount())])
w.mod_filter.setCurrentIndex(0); w.mod_search.setText("ui"); wait(0.3)
print("search 'ui':", [w.mod_tree.topLevelItem(i).text(0) for i in range(w.mod_tree.topLevelItemCount())])
w.mod_search.setText(""); wait(0.2)
print("header stats:", w.header.stats.text())
w.nav.setCurrentRow(4); wait(0.3); print("nav -> tab", w.tabs.currentIndex())

# ---------------- profiles ----------------
from kiln import deploy as _dep
from PySide6.QtWidgets import QInputDialog, QMessageBox
inst = w.inst
inst.create_profile("Survival", inst.active_profile)
w.switch_profile("Survival"); wait(1.5)
w.set_mods_enabled([inst.get("SkyUI"), inst.get("Noble")], False); wait(1.5)
data = game / "Data"
print("profile Survival enabled:", [m.id for m in inst.mods if m.enabled],
      "| SkyUI deployed:", (data / "SkyUI_SE.esp").exists())
w.switch_profile("Default"); wait(1.5)
print("profile Default enabled:", [m.id for m in inst.mods if m.enabled],
      "| SkyUI deployed:", (data / "SkyUI_SE.esp").exists(), "| profiles:", inst.profile_names(),
      "| combo:", w.header.profile_combo.currentText())
inst.mods.reverse(); inst.save(); w.refresh_mods()

# ---------------- auto-sort ----------------
from kiln.sortdialog import SortDialog
dlg = SortDialog(inst, w); dlg.resize(900, 520); dlg.show(); wait(0.5)
dlg.grab().save(str(out / "6-autosort.png"))
dlg.apply(); dlg.close(); w.refresh_mods()
print("after auto-sort:", [m.id for m in inst.mods])

# ---------------- dashboard ----------------
w.go_to(w.dash_tab); wait(5)
w.grab().save(str(out / "5-dashboard.png"))
print("attention items:", w.dash_tab.att_body.count())
