"""Hermetic test of the Mod Doctor: a fake Skyrim with every kind of problem planted
(duplicate mod whose old copy wins, SKSE plugins for the wrong game version, a mod
one folder too deep, an empty mod, missing / disabled / misordered masters, FNIS,
a crash report), plus a Stardew SMAPI log. Checks the diagnosis, then runs
“Fix everything” through the window and checks what's left.
Usage: python3 tests/smoke_doctor.py <empty work dir> [screenshot dir]"""
import os, struct, sys, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
shots = Path(sys.argv[2]) if len(sys.argv) > 2 else None
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, pe, doctor, crashlogs, deploy
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
from kiln import launchguard
launchguard.game_running = lambda inst: False      # a real game may be running on this machine
cfg = sroot / "userdata/1/config/localconfig.vdf"; cfg.parent.mkdir(parents=True)
cfg.write_text('"UserLocalConfigStore"\n{\n"Software"\n{\n"Valve"\n{\n"Steam"\n{\n"apps"\n{\n"489830"\n{\n'
               '"LaunchOptions"\t\t"bash -c \'exec \\"${@/SkyrimSELauncher.exe/skse64_loader.exe}\\"\' -- %command%"\n'
               '}\n}\n}\n}\n}\n}\n')


# ------------------------------------------------------------------ fake binaries
def version_blob(*v):
    ms, ls = (v[0] << 16) | v[1], (v[2] << 16) | v[3]
    return b"MZ" + b"\0" * 62 + struct.pack("<IIII", 0xFEEF04BD, 0x10000, ms, ls)


def skse_plugin(name, indep=0, indep_ex=0, compat=(), query_only=False, built=1760000000):
    """A minimal 64-bit DLL exporting SKSEPlugin_Version (or only SKSEPlugin_Query)."""
    base_rva, raw = 0x1000, 0x200
    sym = b"SKSEPlugin_Query" if query_only else b"SKSEPlugin_Version"
    data = bytearray(0x600)
    # export directory at 0, arrays after it, then the name, then the version struct
    funcs, names, ords, name_off, ver_off = 0x40, 0x48, 0x50, 0x60, 0x100
    struct.pack_into("<IIHHIIIIIII", data, 0, 0, 0, 0, 0, base_rva + name_off, 1, 1, 1,
                     base_rva + funcs, base_rva + names, base_rva + ords)
    struct.pack_into("<I", data, funcs, base_rva + ver_off)
    struct.pack_into("<I", data, names, base_rva + name_off)
    struct.pack_into("<H", data, ords, 0)
    data[name_off:name_off + len(sym)] = sym
    if not query_only:
        struct.pack_into("<II", data, ver_off, 1, 1)
        data[ver_off + 8:ver_off + 8 + len(name)] = name.encode()
        b = ver_off + 8 + 256 + 256 + 252
        struct.pack_into("<II", data, b, indep_ex, indep)
        for i, c in enumerate(compat):
            struct.pack_into("<I", data, b + 8 + 4 * i, pe.pack_version(c))
    hdr = bytearray(raw)
    hdr[:2] = b"MZ"; struct.pack_into("<I", hdr, 0x3C, 0x40)
    hdr[0x40:0x44] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", hdr, 0x44, 0x8664, 1, built, 0, 0, 240, 0x2022)
    opt = 0x58
    struct.pack_into("<H", hdr, opt, 0x20B)
    struct.pack_into("<II", hdr, opt + 112, base_rva, 40)          # export directory
    sec = opt + 240
    struct.pack_into("<8sIIIIIIHHI", hdr, sec, b".rdata", len(data), base_rva, len(data), raw, 0, 0, 0, 0, 0x40000040)
    return bytes(hdr) + bytes(data)


def plugin(masters=(), esm=False):
    body = b"HEDR" + struct.pack("<H", 12) + b"\0" * 12
    for m in masters:
        n = m.encode() + b"\0"
        body += b"MAST" + struct.pack("<H", len(n)) + n + b"DATA" + struct.pack("<H", 8) + b"\0" * 8
    return b"TES4" + struct.pack("<II", len(body), 1 if esm else 0) + b"\0" * 12 + body


# ------------------------------------------------------------------ fake Skyrim
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(skse_plugin("exe", built=1760000000) + version_blob(1, 6, 1170, 0))
(sk / "skse64_loader.exe").write_bytes(version_blob(0, 2, 2, 6))
(sk / "skse64_1_6_1170.dll").write_bytes(version_blob(0, 2, 2, 6))
(sk / "Data/Skyrim.esm").write_bytes(plugin(esm=True))
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
user = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser"
(user / "AppData/Local/Skyrim Special Edition").mkdir(parents=True)

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging")
inst.load()
P = "SKSE/Plugins/"
OK = pe.ADDRLIB | pe.STRUCTS_629


def add(mid, files, enabled=True, **kw):
    m = Mod(id=mid, name=kw.pop("name", mid), enabled=enabled, **kw)
    root = inst.mod_path(m); root.mkdir(parents=True, exist_ok=True)
    for rel, data in files.items():
        f = root / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_bytes(data)
    inst.mods.append(m)
    return m


add("Address Library", {P + "versionlib-1-6-1170-0.bin": b"B"}, nexus_mod_id=32444, nexus_file_id=50)
add("Good Plugin", {P + "Good.dll": skse_plugin("Good", OK)}, nexus_mod_id=100, nexus_file_id=1)
add("Old Fix (new)", {P + "OldFix.dll": skse_plugin("OldFix", OK), P + "OldFix.ini": b"x"},
    name="Old Fix", nexus_mod_id=200, nexus_file_id=9, version="2.0")
add("Old Fix (old)", {P + "OldFix.dll": skse_plugin("OldFix", 0, compat=[(1, 6, 640)]), P + "OldFix.ini": b"y"},
    name="Old Fix", nexus_mod_id=200, nexus_file_id=5, version="1.0")
add("Ancient", {P + "Ancient.dll": skse_plugin("Ancient", query_only=True)}, nexus_mod_id=300, nexus_file_id=3)
add("Deep Mod", {"Deep Mod v1/meshes/thing.nif": b"nif", "Deep Mod v1/textures/thing.dds": b"dds"})
add("Empty Mod", {})
add("Main", {"Main.esp": plugin()})
add("Patch", {"Patch.esp": plugin(["Skyrim.esm", "Main.esp"])})
add("Needy", {"Needy.esp": plugin(["Missing.esm"])})
add("OffMaster", {"OffMaster.esm": plugin(esm=True)}, enabled=False)
add("UsesOff", {"UsesOff.esp": plugin(["OffMaster.esm"])})
add("Old Camera", {P + "OldCamera.dll": skse_plugin("OldCamera", pe.SIGNATURES, pe.NO_STRUCT_USE,
                                                    built=1660000000)}, nexus_mod_id=400, nexus_file_id=4)
add("FNIS", {"tools/GenerateFNIS_for_Users/GenerateFNISforUsers.exe": b"MZ"}, nexus_mod_id=3038, nexus_file_id=1)
inst.save()
deploy.deploy(inst)
inst.sync_plugins()
order = ["Patch.esp", "Main.esp", "Needy.esp", "UsesOff.esp"]
inst.plugins.sort(key=lambda p: order.index(p["name"]) if p["name"] in order else 99)
inst.save()

mg = user / "Documents/My Games/Skyrim Special Edition/SKSE"; mg.mkdir(parents=True)
(mg / "crash-2026-11-13-10-00-00.log").write_text("""Skyrim SSE v1.6.1170
CrashLoggerSSE v1-15-0-0

Unhandled exception "EXCEPTION_ACCESS_VIOLATION" at 0x7FF71C2A1B2C SkyrimSE.exe+0D6DDDA	mov rax, [rcx]

PROBABLE CALL STACK:
	[ 0] 0x7FF71C2A1B2C       SkyrimSE.exe+0D6DDDA
	[ 1] 0x7FFC00001234           Good.dll+0001234
	[ 2] 0x7FFC00005678       kernel32.dll+0005678

REGISTERS:
	RAX 0x1B2C3D4E5F60     (TESObjectREFR*)
		Name: "Lydia"
		File: "Main.esp"
		Modified by: Skyrim.esm -> Main.esp -> Patch.esp

STACK:
	[RSP+0  ] 0x1B2C3D4E5F70     (BSFadeNode*)
		Name: "meshes\\thing.nif"
""")


# an old Microsoft C++ runtime in the prefix, and SKSE's log from the last launch
win = lib / "steamapps/compatdata/489830/pfx/drive_c/windows/system32"; win.mkdir(parents=True)
(win / "msvcp140.dll").write_bytes(version_blob(14, 0, 24215, 1))
(mg / "skse64.log").write_text(
    "checking plugin Good.dll\n"
    "plugin Good.dll (00000001 Good 01000000) disabled, fatal error occurred while loading plugin 0 (handle 9)\n"
    "plugin OldFix.dll (00000001 OldFix 01000000) couldn't load plugin 998 (handle 20)\n")


def keys(issues):
    return sorted(i.key.split(":")[0] for i in issues)


issues = doctor.diagnose(inst)
print("verdict:", doctor.verdict(issues))
for i in issues:
    print(f"  [{i.severity}] {i.title}  -> {[a.label for a in i.actions]}")
k = keys(issues)
vc = next(i for i in issues if i.key == "vcruntime")
print("runtime issue:", vc.severity, "|", vc.title, "| mods:", vc.mods)
assert vc.severity == "crash" and "Good.dll" in vc.explain and vc.actions[0].fix == "vcredist"
for want in ("dupe", "xse_incompatible", "xse_pattern", "misplaced", "empty", "plugin_order", "master_missing",
             "master_mod_off", "known", "crash"):
    assert want in k, (want, k)
assert not any(i.key == "xse_incompatible:Old Fix (old)" for i in issues), "old duplicate reported twice"
crash = next(i for i in issues if i.key.startswith("crash:"))
assert "Good.dll" in crash.explain and "Good Plugin" in crash.explain and "Main.esp" in crash.explain, crash.explain
anc = next(i for i in issues if i.key.startswith("xse_incompatible"))
assert "Ancient" in anc.title and "1.5.97" in anc.explain, anc.explain

# ---- SMAPI log (Stardew)
from kiln import launchguard
sd = lib / "steamapps/common/Stardew Valley"; (sd / "Mods").mkdir(parents=True)
(sd / "Stardew Valley.dll").write_text("x")
(lib / "steamapps/appmanifest_413150.acf").write_text('"AppState" { "installdir" "Stardew Valley" }')
sinst = Instance(by_id("stardewvalley"), sd, work / "sd-staging"); sinst.load()
m = Mod(id="Old Thing", name="Old Thing"); r = sinst.mod_path(m); (r / "OldThing").mkdir(parents=True)
(r / "OldThing/manifest.json").write_text('{"Name": "Old Thing", "UniqueID": "a.old", "Version": "1.0"}')
sinst.mods.append(m); sinst.save()
log = work / "home/.config/StardewValley/ErrorLogs/SMAPI-latest.txt"; log.parent.mkdir(parents=True)
log.write_text("[12:00:00 INFO  SMAPI] SMAPI 4.1.0 with Stardew Valley 1.6.15\n"
               "[12:00:01 ERROR SMAPI]    Skipped mods\n"
               "[12:00:01 ERROR SMAPI]    --------------------------------------------------\n"
               "[12:00:01 ERROR SMAPI]       - Old Thing 1.0 because it's no longer compatible. Please check for a new version.\n"
               "[12:00:01 ERROR SMAPI] \n"
               "[12:05:00 ERROR Buggy Mod] This mod failed in the GameLoop.UpdateTicked event.\n")
si = doctor.diagnose(sinst)
print("stardew:", [(i.severity, i.title) for i in si if i.source == "logs"])
assert any(i.key == "smapi_skip:Old Thing" and i.mods == ["Old Thing"] for i in si), [i.key for i in si]
assert any(i.key == "smapi_err:Buggy Mod" for i in si)

# ------------------------------------------------------------------ the window
from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
asked, errors = [], []
QMessageBox.question = staticmethod(lambda p, t, m, *a, **k: (asked.append(m), QMessageBox.Yes)[1])
QMessageBox.information = staticmethod(lambda p, t, m, *a: None)
QMessageBox.warning = staticmethod(lambda p, t, m, *a: errors.append(m))
QMessageBox.critical = staticmethod(lambda p, t, m, *a: errors.append((t, m)))
from kiln import gui


def wait(cond, secs=30):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()


w = gui.MainWindow(); w.resize(1400, 950); w.show()
inst.load()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
assert w.inst.staging == work / "staging", w.inst.staging
w.go_to(w.doctor_tab)
assert wait(lambda: w.doctor_tab.issues, 20)
print("page headline:", w.doctor_tab.headline.text())
print("fix-all plan:", [a.describe for _i, a in doctor.plan_fix_all(w.doctor_tab.issues)])
if shots:
    shots.mkdir(parents=True, exist_ok=True); wait(lambda: False, 0.5)
    w.grab().save(str(shots / "troubleshoot-before.png"))
before = len(w.doctor_tab.issues)
w.doctor_tab.issues_seen = None
w.doctor_tab.checked_at = 0                      # wait for the check that follows the fixes
w.doctor_tab.fix_everything()
assert wait(lambda: w.doctor_tab.recheck.isEnabled() and w.doctor_tab.checked_at, 20)
wait(lambda: False, 1.5)
after = w.doctor_tab.issues
print(f"issues: {before} -> {len(after)}")
for i in after:
    print(f"  left: [{i.severity}] {i.title}")
left = keys(after)
for gone in ("dupe", "misplaced", "plugin_order", "master_missing", "master_mod_off"):
    assert gone not in left, (gone, left)
assert not (inst.get("Old Fix (old)") and w.inst.get("Old Fix (old)").enabled)
assert (sk / "Data/meshes/thing.nif").is_file(), "relaid-out mod not deployed"
assert pe.xse_plugin(sk / "Data/SKSE/Plugins/OldFix.dll").independence == OK, "new OldFix.dll should win"
pl = {p["name"]: p["enabled"] for p in w.inst.plugins}
assert pl["Needy.esp"] is False and pl["OffMaster.esm"] is True, pl
names = [p["name"] for p in w.inst.plugins]
assert names.index("Main.esp") < names.index("Patch.esp"), names
assert "empty" in left and "xse_incompatible" in left, left     # these need the user
if shots:
    w.grab().save(str(shots / "troubleshoot-after.png"))

# Play warns about crash-level problems: put one back and press Play
w.inst.get("Needy").enabled = True; w.inst.save()
for p in w.inst.plugins:
    if p["name"] == "Needy.esp":
        p["enabled"] = True
gate = []
QMessageBox.exec = lambda self: (gate.append(self.text()), self.done(0))[0]
launched = []
steam.launch = lambda appid: launched.append(appid)
w._deploy_sync(w.inst)
print("play gate allowed:", w._doctor_gate(w.inst), "| message:", gate[0].splitlines()[0] if gate else None)
assert gate and "Needy.esp" in gate[0]
# “Update it”: download Microsoft's installer and run it in the prefix (both faked here)
from kiln import vcruntime, downloader
got = {}
def fake_dl(url, dest, progress=None, cancelled=None, fallback_name="", **_):
    got["url"] = url; dest.mkdir(parents=True, exist_ok=True)
    out = dest / fallback_name; out.write_bytes(b"MZ"); return out
downloader.download = fake_dl
def fake_install(inst, exe):
    got["exe"] = exe.name
    (win / "msvcp140.dll").write_bytes(version_blob(14, 44, 35211, 0)); return (14, 44, 35211)
vcruntime.install = fake_install
w.run_doctor_action(doctor.Action("vcredist", "Update it"))
assert wait(lambda: "exe" in got and not vcruntime.too_old(w.inst), 20), got
print("runtime update:", got, "->", vcruntime.prefix_version(w.inst))
assert got["url"] == vcruntime.REDIST_URL
assert not any(i.key == "vcruntime" for i in doctor.diagnose(w.inst))

print("errors:", errors)
assert not errors
print("doctor smoke OK")
