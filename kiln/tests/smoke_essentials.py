"""Hermetic test of the essentials setup: Skyrim (SKSE into the game root + Address
Library, Premium downloads, launch options) and Stardew (SMAPI from a fake release).
Also saves screenshots of the setup and welcome dialogs.
Usage: python3 tests/smoke_essentials.py <empty work dir> [screenshot dir]"""
import io, os, pwd, sys, time, zipfile
from pathlib import Path
REAL_HOME = Path(pwd.getpwuid(os.getuid()).pw_dir)

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
shots = Path(sys.argv[2]) if len(sys.argv) > 2 else None
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, nexus, downloader, desktop, launchguard
lib = work / "lib"; sroot = work / "steamroot"
real_roots = [REAL_HOME / ".local/share/Steam", REAL_HOME / ".steam/steam"]
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
desktop.is_nxm_handler = lambda: True
steamcfg.steam_running = lambda: False
art_src = next((r / "appcache/librarycache" for r in real_roots if (r / "appcache/librarycache").is_dir()), None)
if art_src:   # real box art for the screenshots
    (sroot / "appcache").mkdir(parents=True); (sroot / "appcache/librarycache").symlink_to(art_src)
cfg = sroot / "userdata/1/config/localconfig.vdf"; cfg.parent.mkdir(parents=True)
cfg.write_text('"UserLocalConfigStore"\n{\n"Software"\n{\n"Valve"\n{\n"Steam"\n{\n"apps"\n{\n}\n}\n}\n}\n}\n')

sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ"); (sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True)
sd = lib / "steamapps/common/Stardew Valley"; (sd / "Mods").mkdir(parents=True)
(sd / "Stardew Valley.dll").write_text("x"); (sd / "Stardew Valley.deps.json").write_text("{}")
(lib / "steamapps/appmanifest_413150.acf").write_text('"AppState" { "installdir" "Stardew Valley" }')

# fake Nexus: SKSE (two MAIN files, Steam is primary), Address Library, USSEP, SkyUI
files = work / "nexusfiles"; files.mkdir()
def z(name, entries):
    with zipfile.ZipFile(files / name, "w") as zz:
        for n, d in entries.items(): zz.writestr(n, d)
z("30379-1.zip", {"skse64_2_03_01/skse64_loader.exe": "L", "skse64_2_03_01/skse64_1_6_1170.dll": "D",
                  "skse64_2_03_01/Data/Scripts/SKSE.pex": "S", "skse64_2_03_01/readme.txt": "r"})
z("32444-1.zip", {"SKSE/Plugins/versionlib-1-6-1170-0.bin": "B"})
z("266-1.zip", {"unofficial skyrim special edition patch.esp": b"TES4" + b"\0" * 20})
z("12604-1.zip", {"SkyUI_SE.esp": b"TES4" + b"\0" * 20})
def details(dom, gid, mid):
    fl = [nexus.NexusFile(1, f"Main {mid} Steam", "1.0", "MAIN", int(time.time()), 1000, f"{mid}-1.zip",
                          primary=True)]
    if mid == 30379:
        fl.append(nexus.NexusFile(2, "SKSE GOG", "0.9", "MAIN", int(time.time()), 1000, "x.zip"))
    return nexus.ModDetails(mid, {30379: "SKSE64", 32444: "Address Library", 266: "USSEP", 12604: "SkyUI"}.get(mid, f"Mod{mid}"),
                            files=fl, category="Modders Resources")
nexus.mod_details = details
nexus.download_links = lambda key, d, mid, fid, k=None, e=None: [f"fake://{mid}-{fid}"]
smapi_zip = work / "SMAPI-9.9.9-installer.zip"
inner = io.BytesIO()
with zipfile.ZipFile(inner, "w") as zz:
    zz.writestr("StardewModdingAPI", "#!/bin/sh\n"); zz.writestr("StardewModdingAPI.dll", "x")
with zipfile.ZipFile(smapi_zip, "w") as zz:
    zz.writestr("SMAPI 9.9.9 installer/internal/linux/install.dat", inner.getvalue())
launchguard.smapi_release = lambda: ("9.9.9", "fake://smapi", smapi_zip.stat().st_size)
def fake_download(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    url = url()[0] if callable(url) else url
    src = smapi_zip if url == "fake://smapi" else files / (url[7:] + ".zip")
    dest.mkdir(parents=True, exist_ok=True)
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
from kiln import gui, essentials
def wait(cond, secs=30):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()

w = gui.MainWindow(); w.resize(1400, 900)
w.settings.set("nexus_api_key", "fake"); w.user = nexus.User("tester", True, 1)
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse"); w.game_combo.setCurrentIndex(idx)
assert w.inst and w.inst.game_dir.is_relative_to(work)
print("Skyrim missing before:", [e.key for e in essentials.missing(w.inst)])
w.open_setup(); dlg = w._setup_dlg
wait(lambda: all(k in dlg.files for k in ("skse", "addresslib", "ussep", "skyui")))
print("files picked:", {k: f.name for k, (d, f) in dlg.files.items()})
if shots:
    shots.mkdir(parents=True, exist_ok=True); dlg.grab().save(str(shots / "setup-skyrim.png"))
dlg.install()
print("Skyrim run finished:", wait(lambda: w.crun is None and infos, 60))
wait(lambda: steamcfg.get_launch_options(489830) != "", 10)
print("Skyrim missing after:", [e.key for e in essentials.missing(w.inst)])
print("game root:", sorted(p.name for p in sk.iterdir()))
print("Data/SKSE/Plugins:", sorted(p.name for p in (sk / "Data/SKSE/Plugins").iterdir()))
print("Skyrim launch options:", steamcfg.get_launch_options(489830))
print("collections recorded:", w.inst.collections, "| mod collection fields:", {m.collection for m in w.inst.mods})
print("preflight:", [(c.level, c.title) for c in launchguard.preflight(w.inst)])

# ---- Stardew: SMAPI from "GitHub"
infos.clear()
idx = next(i for i, g in enumerate(w.games) if g.id == "stardewvalley"); w.game_combo.setCurrentIndex(idx)
w.open_setup(); dlg = w._setup_dlg
for k in ("contentpatcher", "gmcm"): dlg.items[k].setCheckState(0, Qt.Unchecked)
wait(lambda: dlg.items["smapi"].text(2).startswith("SMAPI"), 10)
dlg.install()
print("SMAPI:", wait(lambda: os.access(sd / "StardewModdingAPI", os.X_OK), 20), "| status:", dlg.items["smapi"].text(4))
wait(lambda: steamcfg.get_launch_options(413150) != "", 10)
print("Stardew launch options:", steamcfg.get_launch_options(413150))
print("errors:", errors)
if shots:
    dlg.fill(); w._setup_dlg.grab().save(str(shots / "setup-stardew.png"))
    from kiln.setupdialog import WelcomeDialog
    wd = WelcomeDialog(w); wd.show(); wait(lambda: False, 1); wd.grab().save(str(shots / "welcome.png"))
