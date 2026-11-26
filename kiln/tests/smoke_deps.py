"""Hermetic test: Stardew missing-dependency detection and install, and that a main
file and an optional file of the same Nexus mod are installed side by side.
Usage: python3 tests/smoke_deps.py <empty work dir>"""
import json, os, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, nexus, downloader, desktop, smapideps, launchguard
lib = work / "lib"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: []
desktop.is_nxm_handler = lambda: True; steamcfg.steam_running = lambda: False
sd = lib / "steamapps/common/Stardew Valley"; (sd / "Mods").mkdir(parents=True)
(sd / "Stardew Valley.dll").write_text("x")
(sd / "StardewModdingAPI").write_text("#!/bin/sh"); (sd / "StardewModdingAPI.dll").write_text("x")
(lib / "steamapps/appmanifest_413150.acf").write_text('"AppState" { "installdir" "Stardew Valley" }')

files = work / "nexusfiles"; files.mkdir()
def man(uid, name, deps=(), cpf=None):
    d = {"Name": name, "UniqueID": uid, "Version": "1.0",
         "Dependencies": [{"UniqueID": u, "IsRequired": req} for u, req in deps]}
    if cpf: d["ContentPackFor"] = {"UniqueID": cpf}
    return "// comment SMAPI allows\n" + json.dumps(d, indent=1)[:-1] + ",\n}"   # trailing comma too
def z(name, entries):
    with zipfile.ZipFile(files / name, "w") as zz:
        for n, d in entries.items(): zz.writestr(n, d)
# the user's mods: one needs Big Mod (C#) and its CP part; one needs an unknown mod optionally
z("100-1.zip", {"Fish/manifest.json": man("Me.Fish", "Fancy Fish", [("Author.BigMod", True)], cpf="Author.BigModCP")})
z("101-1.zip", {"Extra/manifest.json": man("Me.Extra", "Extra", [("Nobody.Unknown", False)])})
# Big Mod's main file brings both parts; its optional file is something else
z("900-1.zip", {"Big Mod/[CP] Big Mod/manifest.json": man("Author.BigModCP", "Big Mod CP", cpf="Pathoschild.ContentPatcher"),
                "Big Mod/Big Mod/manifest.json": man("Author.BigMod", "Big Mod")})
z("900-2.zip", {"Big Farm/manifest.json": man("Author.BigFarm", "Big Farm", cpf="Pathoschild.ContentPatcher")})
z("1915-1.zip", {"ContentPatcher/manifest.json": man("Pathoschild.ContentPatcher", "Content Patcher")})
def details(dom, gid, mid):
    fl = [nexus.NexusFile(1, f"Main {mid}", "1.0", "MAIN", int(time.time()), 1000, f"{mid}-1.zip", primary=True)]
    if mid == 900:
        fl = [nexus.NexusFile(1, "Big Mod", "1.0", "MAIN", int(time.time()), 1000, "900-1.zip", primary=True),
              nexus.NexusFile(2, "Big Farm", "1.0", "OPTIONAL", int(time.time()), 1000, "900-2.zip")]
    return nexus.ModDetails(mid, {900: "Big Mod", 1915: "Content Patcher"}.get(mid, f"Mod{mid}"), files=fl)
nexus.mod_details = details
nexus.download_links = lambda key, d, mid, fid, k=None, e=None: [f"fake://{mid}-{fid}"]
smapideps.smapi_lookup = lambda uids: {u.lower(): v for u, v in {
    "Author.BigMod": (900, "Big Mod"), "Pathoschild.ContentPatcher": (1915, "Content Patcher")}.items()
    if u.lower() in {x.lower() for x in uids}}
def fake_download(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    url = url()[0] if callable(url) else url
    src = files / (url[7:] + ".zip"); dest.mkdir(parents=True, exist_ok=True)
    out = dest / (fallback_name or src.name); out.write_bytes(src.read_bytes()); return out
downloader.download = fake_download

from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
infos, errors = [], []
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
QMessageBox.information = staticmethod(lambda p, t, m, *a: infos.append(m))
QMessageBox.critical = staticmethod(lambda p, t, m, *a: errors.append((t, m)))
from kiln import gui
def wait(cond, secs=30):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()

w = gui.MainWindow()
w.settings.set("nexus_api_key", "fake"); w.user = nexus.User("tester", True, 1)
w.game_combo.setCurrentIndex(next(i for i, g in enumerate(w.games) if g.id == "stardewvalley"))
inst = w.inst
for mid in (100, 101):
    w.start_download(inst, mid, 1, switch_tab=False)
wait(lambda: len(inst.mods) == 2); w._deploy_sync(inst)
miss = smapideps.check(inst)
print("missing:", [(m.unique_id, m.needed_by) for m in miss])
c = [c for c in launchguard.preflight(inst) if c.fix == "deps"]
print("play check:", c[0].title if c else None)

w.open_deps(); dlg = w._deps_dlg
wait(lambda: dlg.go.isEnabled(), 20)
print("dialog rows:", [(dlg.tree.topLevelItem(i).text(0), dlg.tree.topLevelItem(i).text(2), dlg.tree.topLevelItem(i).text(3))
                       for i in range(dlg.tree.topLevelItemCount())])
dlg.install()
wait(lambda: w.crun is None and infos, 30)
wait(lambda: "Every mod" in dlg.head.text() or dlg.tree.topLevelItemCount() and dlg.go.isEnabled(), 20)
print("after install:", dlg.head.text().split("</h3>")[0].replace("<h3>", ""))
wait(lambda: not smapideps.check(inst) or False, 1)
print("still missing:", [m.unique_id for m in smapideps.check(inst)])
if smapideps.check(inst):      # Content Patcher (needed by Big Mod CP) is the next round
    dlg.recheck(); wait(lambda: dlg.go.isEnabled(), 20); infos.clear(); dlg.install()
    wait(lambda: w.crun is None and infos, 30)
    print("second round, still missing:", [m.unique_id for m in smapideps.check(inst)])

# main + optional file of the same Nexus mod side by side
n_before = len(inst.mods)
w.start_download(inst, 900, 2, switch_tab=False)
wait(lambda: len(inst.mods) == n_before + 1, 20)
same = [(m.name, m.nexus_file_id) for m in inst.mods if m.nexus_mod_id == 900]
print("files of mod 900:", same)
print("errors:", errors)
