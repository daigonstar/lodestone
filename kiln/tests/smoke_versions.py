"""Hermetic test of game versions: a fake Skyrim 1.7.104, fake Steam depot downloads
of 1.6.1170, switching to it through the window and back again, Steam update
protection, and the Collections page version filter / warning.
Usage: python3 tests/smoke_versions.py <empty work dir> [screenshot dir]"""
import json, os, struct, sys, time
from pathlib import Path
from types import SimpleNamespace

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
shots = Path(sys.argv[2]) if len(sys.argv) > 2 else None
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, nexus, collections as nc, gameversions as gv
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
from kiln import launchguard
launchguard.game_running = lambda inst: False      # a real game may be running on this machine


def exe(*v):
    ms, ls = (v[0] << 16) | v[1], (v[2] << 16) | v[3]
    return b"MZ" + b"\0" * 62 + struct.pack("<IIII", 0xFEEF04BD, 0x10000, ms, ls)


sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(exe(1, 7, 104, 0))
(sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20 + b"NEW")
(lib / "steamapps/appmanifest_489830.acf").write_text(
    '"AppState"\n{\n\t"appid"\t\t"489830"\n\t"installdir"\t\t"Skyrim Special Edition"\n'
    '\t"AutoUpdateBehavior"\t\t"0"\n}\n')
user = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser"
(user / "AppData/Local/Skyrim Special Edition").mkdir(parents=True)
(user / "AppData/Local/Skyrim Special Edition/ContentCatalog.txt").write_text("from 1.7")

# what Steam's download_depot leaves behind
content = sroot / "steamrt64/steamapps/content/app_489830"     # where current Steam clients put them
(content / "depot_489833").mkdir(parents=True)
(content / "depot_489833/SkyrimSE.exe").write_bytes(exe(1, 6, 1170, 0))
(content / "depot_489831/Data").mkdir(parents=True)
(content / "depot_489831/Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20 + b"OLD")
(content / "depot_489832/Data").mkdir(parents=True)
(content / "depot_489832/Data/Skyrim - Old Only.bsa").write_bytes(b"BSA")
(sroot / "logs").mkdir(parents=True)
(sroot / "logs/console_log.txt").write_text("".join(
    f'[2026-11-13 16:00:0{i}] Depot download complete : "{sroot}/steamrt64\\steamapps\\content\\app_489830\\depot_{d}" (manifest)\n'
    for i, d in enumerate((489831, 489832, 489833))))

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()
m = Mod(id="Tex", name="Some Textures"); (inst.mod_path(m) / "textures").mkdir(parents=True)
(inst.mod_path(m) / "textures/a.dds").write_text("dds"); inst.mods.append(m); inst.save()
from kiln import deploy
deploy.deploy(inst)
assert (sk / "Data/textures/a.dds").is_file()
print("installed:", gv.installed_version(inst))

# fake Nexus collections
nc.categories = lambda gid: ["Essentials"]
nexus.mod_categories = lambda d: [("Armour", 3)]
nc.game_versions = lambda d: [("1.7.104.0", 383), ("1.6.1170.0", 2989), ("1.6.640.0", 682)]
found = [nc.CollectionSummary("aaa", "Old Style", "", "x", 1, 1, "", 1, 10, 0, False, ["1.6.1170.0"]),
         nc.CollectionSummary("bbb", "New Style", "", "y", 1, 1, "", 1, 10, 0, False, ["1.7.104.0"])]
searches = []
nc.search = lambda *a, **k: (searches.append(a), (found, 2))[1]
nexus.search = lambda *a, **k: ([], 0)

from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
asked, errors, infos = [], [], []
QMessageBox.question = staticmethod(lambda p, t, m, *a, **k: (asked.append(m), QMessageBox.Yes)[1])
QMessageBox.information = staticmethod(lambda p, t, m, *a, **k: infos.append(m))
QMessageBox.warning = staticmethod(lambda p, t, m, *a, **k: errors.append(m))
QMessageBox.critical = staticmethod(lambda p, t, m, *a, **k: errors.append((t, m)))
from kiln import gui


def wait(cond, secs=20):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()


w = gui.MainWindow(); w.resize(1400, 900); w.show()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
b = w.header.ver_btn
print("banner:", b.isVisible(), b.text())
assert b.isVisible() and b.text() == "Game 1.7.104 ▾"

# ---- Collections: version filter, labels, warning
w.go_to(w.coll_tab); w.search_collections(1)
assert wait(lambda: w.c_ver.count() == 4 and w.c_list.count() == 2)
print("version filter:", [w.c_ver.itemText(i) for i in range(w.c_ver.count())])
assert w.c_ver.itemText(1).startswith("✔ 1.7.104")
print("list:", [w.c_list.item(i).text().splitlines()[1] for i in range(2)])
assert "⚠" in w.c_list.item(0).text() and "✔" in w.c_list.item(1).text()
searches.clear(); w.c_ver.setCurrentIndex(w.c_ver.findData("1.6.1170.0"))
assert wait(lambda: searches) and searches[-1][-1] == "1.6.1170.0", searches
note = w._collection_version_note(SimpleNamespace(game_versions=["1.6.1170.0"]))
print("collection page note:", note.replace("<br>", " ")[:110], "…")
assert "⚠" in note and "1.7.104" in note

# ---- the version window
w.open_versions(); dlg = w._ver_dlg
row = dlg.rows["1.6.1170.0"]
print("commands:", [gv.commands(inst, row["ver"])])
dlg._poll()
print("depots:", {d: l.text() for d, l in row["status"].items()})
assert row["switch"].isEnabled(), "all depots downloaded: switch should be enabled"
if shots:
    shots.mkdir(parents=True, exist_ok=True); dlg.grab().save(str(shots / "version-dialog.png"))
dlg.switch_download(row["ver"])
assert wait(lambda: infos, 30), errors
print("message:", infos[-1][:90])
print("now:", gv.installed_version(inst), "| Skyrim.esm:", (sk / "Data/Skyrim.esm").read_bytes()[-3:],
      "| mod redeployed:", (sk / "Data/textures/a.dds").is_file())
assert gv.same(gv.installed_version(inst), "1.6.1170")
assert (sk / "Data/Skyrim.esm").read_bytes().endswith(b"OLD") and (sk / "Data/Skyrim - Old Only.bsa").is_file()
assert (sk / "Data/textures/a.dds").is_file()
assert not (user / "AppData/Local/Skyrim Special Edition/ContentCatalog.txt").exists()
assert gv.stored(inst) == ["1.7.104.0"], gv.stored(inst)
man = lib / "steamapps/appmanifest_489830.acf"
print("Steam protection:", gv.update_protection(inst), "| AutoUpdateBehavior:",
      steamcfg.loads(man.read_text())["AppState"]["AutoUpdateBehavior"])
assert gv.update_protection(inst) and steamcfg.loads(man.read_text())["AppState"]["AutoUpdateBehavior"] == "1"
assert not any((content / f"depot_{d}").exists() for d in (489831, 489832, 489833)), "downloads used up"
print("banner now:", b.text())
assert b.text() == "Game 1.6.1170 ▾"
assert w.tabs.currentWidget() is w.doctor_tab

# ---- and back
infos.clear(); asked.clear()
dlg = w._ver_dlg; dlg.fill()
dlg.switch_stored("1.7.104.0")
assert wait(lambda: infos, 30), errors
print("back to:", gv.installed_version(inst), "| esm:", (sk / "Data/Skyrim.esm").read_bytes()[-3:],
      "| old-only bsa gone:", not (sk / "Data/Skyrim - Old Only.bsa").exists(),
      "| kept:", gv.stored(inst), "| protection:", gv.update_protection(inst))
assert gv.same(gv.installed_version(inst), "1.7.104")
assert (sk / "Data/Skyrim.esm").read_bytes().endswith(b"NEW")
assert not (sk / "Data/Skyrim - Old Only.bsa").exists() and (sk / "Data/textures/a.dds").is_file()
assert gv.stored(inst) == ["1.6.1170.0"] and not gv.update_protection(inst)
assert any("update the game again" in q for q in asked)
print("errors:", errors)
assert not errors
print("versions smoke OK")
