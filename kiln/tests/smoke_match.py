"""Hermetic test of matching mods to a game version, and of the Downloads-folder
watcher. A fake Skyrim just switched to 1.6.1170 still has the 1.7.104 SKSE and a
plugin built for 1.7.104; Nexus (fake) keeps older files of both. Kiln must pick
SKSE 2.2.8 and the plugin's 1.6.1170 file, install them in place of the old ones,
prefer the working copy of a duplicate, and install archives dropped into
Downloads (recognised by Nexus's file naming).
Usage: python3 tests/smoke_match.py <empty work dir>"""
import os, struct, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"),
                  XDG_DOWNLOAD_DIR=str(work / "home/Downloads"))
(work / "home/Downloads").mkdir(parents=True)
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, nexus, downloader, launchguard, pe, versionmatch, doctor
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
launchguard.game_running = lambda inst: False
sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke_doctor_helpers import skse_plugin, version_blob   # noqa: E402

# a re-released main file ("added 1.7.99, previous versions are still compatible") beats an
# archived build that names 1.6.1170 (the real BDI Universal Support case: the archived one crashed)
from types import SimpleNamespace as _F
_bdi = [_F(file_id=1, category="ARCHIVED", name="BDI Universal Support", version="1.7.99.1", date=1,
           description="CommonLibSSE-NG update with support for Skyrim 1.6.1170 and 1.7.99."),
        _F(file_id=2, category="MAIN", name="BDI Universal Support", version="v0.13.0.4", date=5,
           description="Added support for Skyrim 1.7.99, previous versions are still compatible")]
assert versionmatch.best_file(_bdi, (1, 6, 1170))[0].file_id == 2
assert versionmatch.mentions("1.7.99, previous versions are still compatible") == [("upto", (0, 0, 0), (1, 7, 99))]

OK = pe.ADDRLIB | pe.STRUCTS_629
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(skse_plugin("exe", built=1760000000) + version_blob(1, 6, 1170, 0))
(sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "appid" "489830" "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition").mkdir(parents=True)

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()


def add(mid, files, **kw):
    m = Mod(id=mid, name=kw.pop("name", mid), **kw)
    for rel, data in files.items():
        f = inst.mod_path(m) / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_bytes(data)
    inst.mods.append(m)
    return m


P = "SKSE/Plugins/"
add("SKSE", {"__root__/skse64_loader.exe": version_blob(0, 2, 3, 1), "__root__/skse64_1_7_104.dll": version_blob(0, 2, 3, 1)},
    name="Skyrim Script Extender (SKSE64)", nexus_mod_id=30379, nexus_file_id=795992, version="2.3.1",
    file_name="Skyrim Script Extender (SKSE64) Steam")
add("Address Library", {P + "versionlib-1-6-1170-0.bin": b"B"}, nexus_mod_id=32444, nexus_file_id=50)
add("Old Fix", {P + "OldFix.dll": skse_plugin("OldFix", 0, compat=[(1, 7, 104)])},
    nexus_mod_id=200, nexus_file_id=9, version="2.0", file_name="Old Fix")
# a duplicate pair: the newer copy only works on 1.7.104, the older on 1.6.1170
add("Dup (new)", {P + "Dup.dll": skse_plugin("Dup", 0, compat=[(1, 7, 104)])}, name="Dup", nexus_mod_id=300, nexus_file_id=10)
add("Dup (old)", {P + "Dup.dll": skse_plugin("Dup", OK)}, name="Dup", nexus_mod_id=300, nexus_file_id=3)
inst.save()
from kiln import deploy
deploy.deploy(inst)

# ---- fake Nexus, older files included
NF = nexus.NexusFile
T = int(time.time())
FILES = {
    30379: [NF(792256, "Skyrim Script Extender (SKSE64) Steam", "2.2.8", "OLD_VERSION", T - 900, 1, "skse228.zip",
               "Compatible with Skyrim Special Edition 1.6.1170 from Steam"),
            NF(470991, "Skyrim Script Extender (SKSE64) GOG", "2.2.6", "MAIN", T - 800, 1, "gog.zip",
               "Compatible with Skyrim Special Edition 1.6.1179 from GOG.com"),
            NF(795992, "Skyrim Script Extender (SKSE64) Steam", "2.3.1", "MAIN", T - 100, 1, "skse231.zip",
               "Compatible with Skyrim Special Edition 1.7.104 from Steam")],
    200: [NF(5, "Old Fix", "1.0", "OLD_VERSION", T - 500, 1, "oldfix10.zip", "For AE 1.6.1170"),
          NF(9, "Old Fix", "2.0", "MAIN", T - 50, 1, "oldfix20.zip", "For 1.7.104")],
    400: [NF(41, "Cool Mod", "1.0", "MAIN", T, 1, "Cool Mod-400-1-0-1760000000.zip", "")],
}
nexus.mod_files = lambda gid, mid: list(FILES.get(mid, []))
nexus.mod_details = lambda dom, gid, mid: nexus.ModDetails(
    mid, {30379: "Skyrim Script Extender (SKSE64)", 200: "Old Fix", 400: "Cool Mod"}.get(mid, f"Mod{mid}"),
    files=list(FILES.get(mid, [])), category="Utilities")
nexus.download_links = lambda key, d, mid, fid, k=None, e=None: [f"fake://{mid}-{fid}"]
zips = work / "zips"; zips.mkdir()
with zipfile.ZipFile(zips / "30379-792256.zip", "w") as z:
    z.writestr("skse64_2_02_08/skse64_loader.exe", version_blob(0, 2, 2, 8))
    z.writestr("skse64_2_02_08/skse64_1_6_1170.dll", version_blob(0, 2, 2, 8))
    z.writestr("skse64_2_02_08/Data/Scripts/SKSE.pex", "s")
with zipfile.ZipFile(zips / "200-5.zip", "w") as z:
    z.writestr("SKSE/Plugins/OldFix.dll", skse_plugin("OldFix", 0, compat=[(1, 6, 1170)]))


def fake_download(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    url = url()[0] if callable(url) else url
    src = zips / (url[7:] + ".zip")
    dest.mkdir(parents=True, exist_ok=True)
    out = dest / (fallback_name or src.name); out.write_bytes(src.read_bytes()); return out
downloader.download = fake_download

# ---- the plan
needs = versionmatch.plan(inst)          # before the duplicate is sorted out
for n in needs:
    print(f"  need: {n.title} | {n.why} | -> {(n.file.name, n.file.version) if n.file else n.problem} ({n.sure})")
assert {n.nexus_id for n in needs} == {30379, 200}, [n.nexus_id for n in needs]   # Dup has a working copy
pick = {n.nexus_id: n for n in needs}
assert pick[30379].file.file_id == 792256 and pick[30379].sure == "exact"
assert pick[200].file.file_id == 5

# ---- duplicates keep the copy that works on 1.6.1170
dup = next(i for i in doctor.diagnose(inst) if i.key == "dupe:300")
print("duplicate fix:", dup.actions[0].fix, "|", dup.title)
assert dup.actions[0].fix == "disable:Dup (new)"
doctor.apply(inst, dup.actions[0].fix)
deploy.deploy(inst)

# ---- through the window
from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
infos, errors = [], []
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
QMessageBox.information = staticmethod(lambda p, t, m, *a, **k: infos.append(m))
QMessageBox.warning = staticmethod(lambda p, t, m, *a, **k: errors.append(m))
QMessageBox.critical = staticmethod(lambda p, t, m, *a, **k: errors.append((t, m)))
from kiln import gui


def wait(cond, secs=30):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()


w = gui.MainWindow(); w.resize(1300, 850)
w.settings.set("nexus_api_key", "fake"); w.user = nexus.User("tester", True, 1)
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
# essentials: SKSE is the 1.7.104 build on a 1.6.1170 game
from kiln import essentials
skse = next(e for e in essentials.for_game("skyrimse") if e.key == "skse")
print("SKSE essential:", essentials.status(inst, skse), "| banner:", w.header.ess_btn.text())
assert essentials.status(inst, skse) == "mismatch" and w.header.ess_btn.text().startswith("Fix essentials")
w.settings.set("essentials_match", "auto")      # the “Always do this automatically” choice
w.check_essentials_version()
assert wait(lambda: w.crun is None and inst.find_nexus(30379).nexus_file_id == 792256, 60), errors
wait(lambda: (sk / "skse64_1_6_1170.dll").exists(), 20)
w.refresh_top()
print("after the automatic swap:", essentials.status(inst, skse), inst.find_nexus(30379).version,
      "| banner:", w.header.ess_btn.text())
assert essentials.status(inst, skse) == "installed" and not (sk / "skse64_1_7_104.dll").exists()
assert len([m for m in inst.mods if m.nexus_mod_id == 30379]) == 1

w.open_match(); dlg = w._match_dlg               # now only the mod is left
assert wait(lambda: dlg.tree.topLevelItemCount() == 1), dlg.head.text()
print("match window:", [(dlg.tree.topLevelItem(i).text(0), dlg.tree.topLevelItem(i).text(3))
                        for i in range(dlg.tree.topLevelItemCount())])
dlg.install()
assert wait(lambda: w.crun is None and all(m.nexus_file_id in (792256, 5) for m in inst.mods
                                           if m.nexus_mod_id in (30379, 200)), 60), errors
wait(lambda: (sk / "skse64_1_6_1170.dll").exists(), 20)
print("SKSE now:", inst.find_nexus(30379).version, "| root:", sorted(p.name for p in sk.glob("skse64*")))
assert (sk / "skse64_1_6_1170.dll").exists() and not (sk / "skse64_1_7_104.dll").exists()
assert len([m for m in inst.mods if m.nexus_mod_id == 30379]) == 1, "replaced, not installed beside"
assert pe.xse_plugin(sk / "Data/SKSE/Plugins/OldFix.dll").compatible == [pe.pack_version((1, 6, 1170))]
left = [i.key for i in doctor.diagnose(inst) if i.source == "xse"]
print("Troubleshoot xse issues left:", left)
assert not left

# ---- Downloads folder watcher
dl = w.watch_folder()
print("watching:", dl)
assert dl == work / "home/Downloads"
with zipfile.ZipFile(dl / "Cool Mod-400-1-0-1760000000.zip", "w") as z:
    z.writestr("Cool.esp", b"TES4" + b"\0" * 20)
with zipfile.ZipFile(dl / "holiday photos.zip", "w") as z:
    z.writestr("a.jpg", "x")
assert wait(lambda: inst.find_nexus(400) is not None, 30), w.statusBar().currentMessage()
cool = inst.find_nexus(400)
print("installed from Downloads:", cool.name, cool.nexus_file_id, "| original kept:",
      (dl / "Cool Mod-400-1-0-1760000000.zip").exists(),
      "| copy in Kiln:", (inst.downloads_dir / "Cool Mod-400-1-0-1760000000.zip").exists())
assert cool.nexus_file_id == 41 and (sk / "Data/Cool.esp").exists() or wait(lambda: (sk / "Data/Cool.esp").exists(), 10)
assert not any("holiday" in m.name.lower() for m in inst.mods)
# ---- Engine Fixes on a pre-1.7.99 game needs its preloader (Part 2) next to the exe
from kiln import launchguard as lg
add("Engine Fixes", {P + "EngineFixes.dll": skse_plugin("EngineFixes", OK)}, nexus_mod_id=17230, nexus_file_id=725753)
inst.save(); w.refresh_mods()
need = [c for c in lg.preflight(inst) if c.fix == "loader:efpre"]
print("Play check:", need[0].title if need else None)
assert need and need[0].level == "error"
FILES[17230] = [NF(725261, "Engine Fixes - SKSE64 Preloader", "7", "MAIN", T, 1, "pre.zip", "extract to your main Skyrim folder"),
                NF(725753, "Engine Fixes - Main File", "7.0.20", "MAIN", T, 1, "ef.zip", "")]
with zipfile.ZipFile(zips / "17230-725261.zip", "w") as z:
    for n in ("d3dx9_42.dll", "tbb.dll", "tbbmalloc.dll"):
        z.writestr(n, "dll")
w.run_doctor_action(doctor.Action("install_named:17230:preloader", "Install it"))
assert wait(lambda: (sk / "d3dx9_42.dll").exists(), 30), errors
print("preloader in the game folder:", sorted(p.name for p in sk.iterdir() if p.suffix == ".dll"))
assert (sk / "tbbmalloc.dll").exists() and not (sk / "Data/d3dx9_42.dll").exists()
assert not [c for c in lg.preflight(inst) if c.fix == "loader:efpre" and c.level == "error"]
assert "d3dx9_42=n,b" in lg.wanted_launch_options(inst, "")
print("launch options:", lg.wanted_launch_options(inst, ""))

# ---- a working copy already downloaded (switched off) beats a download
add("Helper (new)", {P + "Helper.dll": skse_plugin("Helper", OK)}, name="Helper", nexus_mod_id=500,
    nexus_file_id=20, version="1.5.0", enabled=False)
add("Helper (old)", {P + "Helper.dll": skse_plugin("Helper", 0, compat=[(1, 6, 353)])}, name="Helper",
    nexus_mod_id=500, nexus_file_id=10, version="1.3.2")
inst.save()
w.deploy_timer.stop(); w._deploy_sync(inst)
n = next(n for n in versionmatch.plan(inst) if n.nexus_id == 500)
print("match picks:", n.reuse.id if n.reuse else None)
assert n.reuse is not None and n.reuse.id == "Helper (new)" and n.file is None
iss = next(i for i in doctor.diagnose(inst) if i.key == "xse_incompatible:Helper (old)")
print("Troubleshoot:", iss.actions[0].label, iss.actions[0].auto)
assert iss.actions[0].fix == "swap:Helper (new):Helper (old)" and iss.actions[0].auto
doctor.apply(inst, iss.actions[0].fix)
assert inst.get("Helper (new)").enabled and not inst.get("Helper (old)").enabled
w.deploy_timer.stop(); w._deploy_sync(inst)

print("errors:", errors)
assert not errors
print("match smoke OK")
