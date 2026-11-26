"""Hermetic test of game settings (what Skyrim's launcher sets): reading and writing
SkyrimPrefs.ini without disturbing the rest, quality presets that keep your screen
settings, SSE Display Tweaks kept in step, and opening the launcher through Proton.
Usage: python3 tests/smoke_settings.py <empty work dir>"""
import os, sys, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, launchguard, gamesettings as gs
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
launchguard.game_running = lambda inst: False
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data/SKSE/Plugins").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ"); (sk / "SkyrimSELauncher.exe").write_bytes(b"MZ")
(sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(sk / "Ultra.ini").write_text("[Display]\r\niShadowMapResolution=4096\r\nbFull Screen=1\r\niSize W=1280\r\n[Grass]\r\nfGrassStartFadeDistance=7000\r\n")
(sk / "Low.ini").write_text("[Display]\r\niShadowMapResolution=1024\r\n")
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
mg = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition"
mg.mkdir(parents=True)
prefs = mg / "SkyrimPrefs.ini"
prefs.write_bytes(b"[Display]\r\nfShadowDistance=8000.0000\r\niShadowMapResolution=2048\r\nbUseTAA=1\r\n"
                  b"bFXAAEnabled=0\r\nbBorderless=0\r\nbFull Screen=0\r\niSize H=1080\r\niSize W=1920\r\n"
                  b"[Controls]\r\nbUseKinect=0\r\n[Grass]\r\n")
from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()
m = Mod(id="Display Tweaks", name="SSE Display Tweaks")
f = inst.mod_path(m) / "SKSE/Plugins/SSEDisplayTweaks.ini"; f.parent.mkdir(parents=True)
f.write_text("[Render]\nFullscreen=false\nBorderless=true\nResolution=1280x720\n; comment kept\n")
inst.mods.append(m); inst.save()
from kiln import deploy
deploy.deploy(inst)

cur = gs.current(inst)
print("read:", cur)
assert (cur.width, cur.height, cur.mode, cur.aa) == (1920, 1080, "window", "taa")
print("presets:", gs.presets(inst))
assert gs.presets(inst) == ["Low", "Ultra"]

done = gs.apply(inst, gs.Settings(2560, 1440, "borderless", True, "fxaa", "Ultra"))
print("applied:", done)
d = gs.read_ini(prefs)
assert d["display"]["isize w"] == "2560" and d["display"]["bborderless"] == "1" and d["display"]["bfull screen"] == "0"
assert d["display"]["ishadowmapresolution"] == "4096", "the Ultra preset's quality values"
assert d["grass"]["fgrassstartfadedistance"] == "7000"
assert d["controls"]["busekinect"] == "0" and d["display"]["bfxaaenabled"] == "1" and d["display"]["busetaa"] == "0"
assert b"\r\n" in prefs.read_bytes() and b"\r\r" not in prefs.read_bytes()
dt = (sk / "Data/SKSE/Plugins/SSEDisplayTweaks.ini")
txt = dt.read_text()
print("SSE Display Tweaks:", [l for l in txt.splitlines() if "=" in l])
assert "Resolution=2560x1440" in txt and "Borderless=true" in txt and "; comment kept" in txt
assert dt.stat().st_ino == f.stat().st_ino, "edited in place: still Kiln's hardlink"

# ---- the window
from PySide6.QtCore import QCoreApplication, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
errors = []
QMessageBox.critical = staticmethod(lambda p, t, m, *a, **k: errors.append((t, m)))
from kiln import gui
w = gui.MainWindow()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
assert not w.header.set_btn.isHidden()
w.open_game_settings(); dlg = w._gs_dlg
print("dialog shows:", dlg.res.currentText(), "|", dlg.mode.currentText(), "|", dlg.aa.currentText())
assert dlg.res.currentData() == (2560, 1440) and dlg.mode.currentData() == "borderless"
dlg.mode.setCurrentIndex(dlg.mode.findData("fullscreen"))
dlg.res.setEditText("3440 x 1440")
dlg.save()
d = gs.read_ini(prefs)["display"]
print("saved:", d["isize w"], d["isize h"], "full", d["bfull screen"], "| status:", w.statusBar().currentMessage()[:60])
assert (d["isize w"], d["isize h"], d["bfull screen"]) == ("3440", "1440", "1")

# the launcher opens through the game's Proton, in the game's prefix
ran = []
from kiln import vcruntime
proton = work / "proton"; proton.write_text("#!/bin/sh\n"); proton.chmod(0o755)
vcruntime.proton_for = lambda inst: proton
import subprocess
subprocess.Popen = lambda cmd, **kw: ran.append((cmd, kw.get("env", {}).get("STEAM_COMPAT_DATA_PATH")))
w.open_game_settings(); w._gs_dlg.open_launcher()
print("launcher:", ran[-1][0][1:], "| prefix:", ran[-1][1].split("/steamapps/")[-1])
assert ran[-1][0][1] == "run" and ran[-1][0][2].endswith("SkyrimSELauncher.exe") and ran[-1][1].endswith("compatdata/489830")
assert not errors, errors
print("settings smoke OK")
