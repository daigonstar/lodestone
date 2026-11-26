"""Hermetic test of the Logs tab: a fake Skyrim whose SKSE log stops while handing the
first start-up signal to Behavior Data Injector (the game died inside it, so nothing
says "error"). Kiln must tell that story in plain English, name the mod, offer to turn
it off (safely, through Fix everything, since nothing uses it), suggest a crash
logger, and translate the other logs: plugin errors flagged, harmless "trampoline"
lines and info lines that happen to contain "error" not flagged.
Usage: python3 tests/smoke_logs.py <empty work dir>"""
import os, sys, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, launchguard
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
launchguard.game_running = lambda inst: False
sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke_doctor_helpers import skse_plugin, version_blob   # noqa: E402
from kiln import pe

OK = pe.ADDRLIB | pe.SIGNATURES
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(skse_plugin("exe", built=1760000000) + version_blob(1, 6, 1170, 0))
(sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "appid" "489830" "installdir" "Skyrim Special Edition" }')
mg = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition"
(mg / "SKSE").mkdir(parents=True)

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
add("SKSE", {"__root__/skse64_loader.exe": version_blob(0, 2, 2, 8), "__root__/skse64_1_6_1170.dll": version_blob(0, 2, 2, 8)},
    name="Skyrim Script Extender (SKSE64)")
add("Address Library", {P + "versionlib-1-6-1170-0.bin": b"B"})
add("Alpha", {P + "Alpha.dll": skse_plugin("Alpha", OK)}, name="Alpha Tweaks")
add("BDI", {P + "BehaviorDataInjector.dll": skse_plugin("BehaviorDataInjector", OK)},
    name="Behavior Data Injector Universal Support", nexus_mod_id=78159)
add("Physics", {P + "hdtSMP64.dll": skse_plugin("hdtSMP64", OK)}, name="FSMP - Faster HDT-SMP")
add("Broken", {P + "BrokenThing.dll": skse_plugin("BrokenThing", OK)}, name="Broken Thing")
inst.save()
from kiln import deploy
deploy.deploy(inst)
time.sleep(1.1)                      # the logs are written after the deploy (like a real launch)

(mg / "SKSE/skse64.log").write_text(
    "SKSE64 runtime: initialize (version = 2.2.8 01064920 01DD739CFFC1FD74, os = 6.2 (9200))\n"
    "checking plugin Alpha.dll\n"
    "plugin Alpha.dll (00000001 Alpha 00000001) loaded correctly (handle 1)\n"
    "plugin BehaviorDataInjector.dll (00000001 BehaviorDataInjector 00010030) loaded correctly (handle 2)\n"
    "plugin BrokenThing.dll (00000001 BrokenThing 00000001) loaded correctly (handle 3)\n"
    "plugin hdtSMP64.dll (00000001 hdtSMP64 00000001) loaded correctly (handle 4)\n"
    "trampoline alloc 00006FFFF9C5FC00x0000000000000400 failed (000001E7)\n"
    "dispatch message (0) to plugin listeners\n"
    "sending message type 0 to plugin 1\n"
    "sending message type 0 to plugin 2\n")
(mg / "SKSE/hdtSMP64.log").write_text("trampoline alloc 00006FFFF9C5FC00x0000000000000400 failed (000001E7)\n" * 40)
(mg / "SKSE/BrokenThing.log").write_text(
    "[18:30:02.100] [I] BrokenThing v1.0 loaded\n"
    "[18:30:02.200] [E] Failed to open Data/SKSE/Plugins/BrokenThing/config.json: file not found\n"
    "[18:30:02.300] [E] Failed to open Data/SKSE/Plugins/BrokenThing/config.json: file not found\n"
    "[18:30:02.400] [W] Form 0x00012345 not found in Missing.esp\n")
(mg / "SKSE/Alpha.log").write_text(
    "[2026-11-13 18:30:03.479][memory_access_errors.h:108 ][I]: installed misc memory access error fixes\n"
    "[2026-11-13 18:30:03.500][main.cpp:10 ][I]: Alpha loaded\n")
(mg / "SKSE/EngineFixes.log").write_text("[2026-11-13 20:00:00.000][main.cpp:87 ][I]: EngineFixes PreLoad\n"
                                         "[2026-11-13 20:01:51.000][main.cpp:39 ][I]: time to main menu 111079\n")
(sk / "d3dx9_42.log").write_text('attempting to load "S:\\\\Data\\\\SKSE\\\\Plugins\\\\Alpha.dll"\nloaded successfully\nloader finished\n')

from kiln import logreader, doctor
reps = logreader.reports(inst, running=False)
by = {r.title: r for r in reps}
for r in reps:
    print(f"[{r.level:7}] {r.title}: {r.summary}")
xse = by["SKSE (the script extender)"]
assert xse.level == "problem" and "BehaviorDataInjector.dll" in xse.summary
assert any("game version 1.6.1170" in ln.plain for ln in xse.lines)
stop_line = next(ln for ln in xse.lines if "The log stops" in ln.plain)
assert stop_line.mod == "BDI" and "none of your mods provide any" in stop_line.plain
assert by["FSMP - Faster HDT-SMP"].level == "note", "trampoline lines are harmless"
assert by["Alpha Tweaks"].level == "ok", "an [I] line containing 'error' isn't an error"
bt = by["Broken Thing"]
assert bt.level == "problem" and bt.mods == ["Broken"]
assert sum("config.json" in ln.raw for ln in bt.lines) == 1, "repeated lines merged"
assert any("isn't there" in ln.plain for ln in bt.lines)
assert any("Missing.esp" in ln.raw and "plugin (.esp/.esm)" in ln.plain for ln in bt.lines)
assert by["Engine Fixes preloader"].level == "ok"
st = logreader.story(inst, reps)
print("story:", st.headline, "|", st.details)
assert st.level == "problem" and "closed by itself while starting" in st.headline and st.mods == ["BDI"]
assert any("crash logger" in d for d in st.details)
assert "Start-up took 1 min 51 s to reach the main menu." in st.details, st.details

issues = doctor.diagnose(inst)
stop = next(i for i in issues if i.key.startswith("xse_stop:"))
print("doctor:", stop.title, [(a.label, a.auto) for a in stop.actions])
assert stop.severity == "crash" and stop.mods == ["BDI"] and stop.auto_action.fix == "disable:BDI"
assert any(i.key == "no_crash_logger" for i in issues)

# the tab
from PySide6.QtCore import QCoreApplication, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
from kiln import gui
w = gui.MainWindow()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
from kiln.doctorpage import LogCard
page = w.doctor_tab
page.refresh()
end = time.time() + 20
while time.time() < end and not page.logs.reports:
    app.processEvents(); time.sleep(0.05)
app.processEvents()
lv = page.logs
cards = lv.body.findChildren(LogCard)
print("tab:", page.tabs.tabText(1), "| cards:", [c.r.title for c in cards], "|", lv.count.text())
assert page.tabs.tabText(1) == "Logs (2)"
assert {c.r.title for c in cards} == {"SKSE (the script extender)", "Broken Thing"}
assert "closed by itself" in lv.story_box.text()
skse_card = next(c for c in cards if c.r.title.startswith("SKSE"))
assert not skse_card.detail.isHidden() and skse_card.toggle.text() == "▾", "problems from the last launch open by themselves"
lv.only_bad.setChecked(False); app.processEvents()
assert len(lv.body.findChildren(LogCard)) >= 6
lv.only_bad.setChecked(True)
page.tabs.setCurrentIndex(1)
w.go_to(page)
w.resize(1300, 900); app.processEvents()
shot = work / "logs_tab.png"
page.grab().save(str(shot))
print("screenshot:", shot)

# Fix everything turns BDI off (the only safe automatic fix here)
plan = doctor.plan_fix_all(issues)
assert [a.fix for _i, a in plan if a.fix.endswith(":BDI")] == ["disable:BDI"]
# BDI listed as a requirement isn't needed while no mod ships a config for it
assert logreader.unused_requirement(inst, [78146]) and logreader.unused_requirement(inst, [78159])
assert logreader.unused_requirement(inst, [12345]) == ""
add("Moveset", {P + "BehaviorDataInjector/Moveset_BDI.json": b"[]"}, name="A Moveset")
inst.save(); deploy.deploy(inst)
assert logreader.unused_requirement(inst, [78159]) == "", "a mod ships a config now: it's needed"
# a main file and its add-on from the same Nexus page are not "installed twice"
# (the real Shadow of Skyrim case: the add-on replaces 1 of the main file's scripts)
main = {f"Scripts/sos_{i}.pex": b"x" for i in range(20)} | {"Shadow.esp": b"TES4" + b"\0" * 20}
add("SoS", main, name="Shadow of Skyrim", nexus_mod_id=65136, nexus_file_id=320534, file_name="Shadow of Skyrim")
add("SoS addon", {"Scripts/sos_0.pex": b"y"}, name="Shadow of Skyrim: No Startup Message",
    nexus_mod_id=65136, nexus_file_id=340857, file_name="Shadow of Skyrim - No Startup Message")
inst.save(); deploy.deploy(inst)
assert not [i for i in doctor.diagnose(inst) if i.key == "dupe:65136"], "a patch isn't a duplicate"
add("SoS again", dict(main), name="Shadow of Skyrim", nexus_mod_id=65136, nexus_file_id=350000,
    file_name="Shadow of Skyrim")
inst.save(); deploy.deploy(inst)
d = next(i for i in doctor.diagnose(inst) if i.key == "dupe:65136")
assert d.auto_action.fix == "disable:SoS", d.auto_action.fix     # the real second copy is
# the main file switched off while its add-on is on (what the old duplicate check did):
# Kiln turns the main file back on, just before the add-on, so the add-on still wins
for mid in ("SoS again",):
    inst.mods.remove(inst.get(mid))
sos, addon = inst.get("SoS"), inst.get("SoS addon")
sos.enabled = False
inst.mods.remove(sos); inst.mods.append(sos)          # and after it in the order, like the real setup
inst.save(); deploy.deploy(inst)
iss = doctor.diagnose(inst)
mo = next(i for i in iss if i.key == "main_off:SoS")
print("main file:", mo.title)
assert mo.auto_action.fix == "enable_main:SoS|SoS addon"
assert not [i for i in iss if i.key.startswith("main_off:") and i is not mo], "only the real main file"
assert doctor.apply(inst, mo.auto_action.fix)
ids = [m.id for m in inst.mods]
assert inst.get("SoS").enabled and ids.index("SoS") == ids.index("SoS addon") - 1
deploy.deploy(inst)
assert (sk / "Data/Scripts/sos_0.pex").read_bytes() == b"y", "the add-on's file still wins"
assert (sk / "Data/Shadow.esp").is_file()
assert not [i for i in doctor.diagnose(inst) if i.key.startswith(("main_off:", "dupe:"))]
print("logs smoke OK")
