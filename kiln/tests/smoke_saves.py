"""Hermetic test of save management: Skyrim SE saves (real format: header, screenshot,
LZ4-compressed plugin list) read for the Saves page; separate saves per profile
(sLocalSavePath in Skyrim.ini follows the profile, existing saves move into it);
copy / move / rename / back up; a warning when a save uses mods the profile doesn't
load; and Kiln's own LZ4 decoder for systems without the lz4 module.
Usage: python3 tests/smoke_saves.py <empty work dir>"""
import os, struct, sys, time, zipfile
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
import lz4.block

sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ")
(sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "appid" "489830" "installdir" "Skyrim Special Edition" }')
mg = lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition"
(mg / "Saves").mkdir(parents=True)
(mg / "Skyrim.INI").write_bytes(b"[General]\r\nsLanguage=ENGLISH\r\n\r\n[Display]\r\nbAllowScreenshot=1\r\n")


def ws(s):
    b = s.encode()
    return struct.pack("<H", len(b)) + b


def ess(name, player, level, where, played, plugins, light=(), char="402AA95A", color=(200, 40, 40)):
    w, h = 32, 20
    hdr = (struct.pack("<II", 12, 7) + ws(player) + struct.pack("<I", level) + ws(where) + ws(played)
           + ws("NordRace") + struct.pack("<HffQ", 0, 0.0, 1.0, 0) + struct.pack("<II", w, h) + struct.pack("<H", 2))
    shot = bytes(color + (255,)) * (w * h)
    pinfo = bytes([len(plugins)]) + b"".join(ws(p) for p in plugins)
    body = bytes([78]) + struct.pack("<I", len(pinfo)) + pinfo + struct.pack("<H", len(light)) \
        + b"".join(ws(p) for p in light) + b"\0" * 5000
    comp = lz4.block.compress(body, store_size=False)
    data = (b"TESV_SAVEGAME" + struct.pack("<I", len(hdr)) + hdr + shot
            + struct.pack("<II", len(body), len(comp)) + comp)
    p = mg / "Saves" / f"{name}_{char}_0_4E616D65_Tamriel_000003_20261113185006_1_1.ess"
    p.write_bytes(data)
    p.with_suffix(".skse").write_bytes(b"co")
    return p


old1 = ess("Save1", "Lydia", 12, "Whiterun", "001.59.36", ["Skyrim.esm", "Update.esm", "Alpha.esp"])
time.sleep(0.05)
old2 = ess("Autosave1", "Lydia", 13, "Riverwood", "002.10.00", ["Skyrim.esm", "Alpha.esp", "Gone.esp"],
           light=["GoneLight.esl"])
time.sleep(0.05)
ess("Quicksave0", "Borri", 3, "Helgen", "000.30.14", ["Skyrim.esm"], char="11111111", color=(40, 200, 40))

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()
m = Mod(id="Alpha", name="Alpha")
f = inst.mod_path(m) / "Alpha.esp"; f.parent.mkdir(parents=True); f.write_bytes(b"TES4" + b"\0" * 20)
inst.mods.append(m); inst.save()
from kiln import deploy, savegames as sg
deploy.deploy(inst); inst.sync_plugins(); inst.save()

# ---- faster start-up: an empty <plugin>.ini for every plugin without one
data = sk / "Data"
assert (data / "Alpha.ini").read_bytes() == b"" and (data / "Skyrim.ini").read_bytes() == b""
(data / "Beta.esp").write_bytes(b"TES4")              # a plugin outside Kiln; an empty INI left by a test
(data / "Beta.ini").write_bytes(b"")
deploy.deploy(inst)
man = deploy._load_manifest(inst)
assert sorted(man["generated"]) == ["Alpha.ini", "Beta.ini", "Skyrim.ini"], man["generated"]
real = Mod(id="AlphaIni", name="Alpha settings")
fi = inst.mod_path(real) / "Alpha.ini"; fi.parent.mkdir(parents=True); fi.write_bytes(b"[General]\nx=1\n")
inst.mods.append(real); inst.save(); deploy.deploy(inst)
assert (data / "Alpha.ini").read_bytes().startswith(b"[General]"), "a real settings file wins"
assert "Alpha.ini" not in deploy._load_manifest(inst)["generated"]
assert not (inst.staging / ".kiln-backup").exists() or not any((inst.staging / ".kiln-backup").rglob("Alpha.ini"))
inst.mods.remove(real); inst.save(); deploy.deploy(inst)
assert (data / "Alpha.ini").read_bytes() == b"", "back to an empty one when the mod goes"
(data / "Beta.esp").unlink(); deploy.deploy(inst)
assert not (data / "Beta.ini").exists(), "removed with its plugin"
deploy.purge(inst)
assert not (data / "Alpha.ini").exists() and not (data / "Skyrim.ini").exists()
deploy.deploy(inst); inst.sync_plugins(); inst.save()

# ---- reading
saves = sg.list_saves(inst)
for s in saves:
    print(f"{s.kind:9} {s.player} lvl {s.level} {s.location} | {s.played} | plugins {s.plugins} | missing {sg.missing(inst, s)}")
assert [s.kind for s in saves] == ["Quicksave", "Autosave", "Save"]
a = next(s for s in saves if s.kind == "Autosave")
assert (a.player, a.level, a.location, a.race, a.number) == ("Lydia", 13, "Riverwood", "NordRace", 7)
assert a.plugins == ["Skyrim.esm", "Alpha.esp", "Gone.esp", "GoneLight.esl"]
assert sg.missing(inst, a) == ["Gone.esp", "GoneLight.esl"]
assert next(s for s in saves if s.kind == "Save").played == "1h 59m played"
assert a.shot[:2] == (32, 20) and a.shot[2][:4] == bytes((200, 40, 40, 255))
assert len(a.files) == 2 and a.files[1].suffix == ".skse"
assert len({s.character for s in saves}) == 2

# Kiln's own LZ4 decoder (no lz4 module) gives the same result
raw = bytes(range(256)) * 40 + b"abcabcabcabc" * 300
comp = lz4.block.compress(raw, store_size=False)
sys.modules["lz4.block"] = None                     # import now fails
assert sg.lz4_block(comp, len(raw)) == raw
sg._cache.clear()
assert sg.read_save(sg.list_saves(inst, read=False)[1]).plugins == a.plugins
del sys.modules["lz4.block"]

# ---- separate saves per profile
assert not sg.enabled(inst)
assert sg.point_game(inst) is False, "off: nothing to change"
moved = sg.turn_on(inst, move_existing=True)
assert moved == 3
ini = mg / "Skyrim.INI"
txt = ini.read_bytes()
print("ini:", txt)
assert b"sLanguage=ENGLISH\r\nsLocalSavePath=Saves\\Default\\\r\n\r\n[Display]" in txt, "blank line kept"
assert len(sg.list_saves(inst)) == 3 and sg.list_saves(inst, sg.SHARED) == []
assert (mg / "Saves/Default" / old1.name).is_file() and (mg / "Saves/Default" / old1.with_suffix(".skse").name).is_file()

inst.create_profile("Survival")
inst.switch_profile("Survival"); deploy.deploy(inst)
assert b"sLocalSavePath=Saves\\Survival\\" in ini.read_bytes(), "the ini follows the profile"
assert sg.list_saves(inst) == [] and (mg / "Saves/Survival").is_dir()
src = next(s for s in sg.list_saves(inst, "Default") if s.player == "Borri")
sg.copy_to(inst, src, "Survival")
assert [s.player for s in sg.list_saves(inst)] == ["Borri"] and len(sg.list_saves(inst, "Default")) == 3
sg.copy_to(inst, next(s for s in sg.list_saves(inst, "Default") if s.kind == "Save"), "Survival", move=True)
assert len(sg.list_saves(inst, "Default")) == 2 and len(sg.list_saves(inst)) == 2
z = sg.backup(inst, "Survival", work / "backups")
assert sorted(zipfile.ZipFile(z).namelist()) == sorted(f.name for s in sg.list_saves(inst) for f in s.files)

inst.rename_profile("Survival", "Hardcore"); sg.rename_profile(inst, "Survival", "Hardcore")
assert (mg / "Saves/Hardcore").is_dir() and not (mg / "Saves/Survival").exists()
assert b"sLocalSavePath=Saves\\Hardcore\\" in ini.read_bytes()

# ---- the page
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
page = w.saves_tab
w.go_to(page)


def settle(cond, t=10):
    end = time.time() + t
    while time.time() < end and not cond():
        app.processEvents(); time.sleep(0.03)
    app.processEvents()


page.folder_combo.setCurrentIndex(page.folder_combo.findData("Default"))
settle(lambda: page.list.count() == 2)
print("folders:", [page.folder_combo.itemText(i) for i in range(page.folder_combo.count())], "|", page.count.text())
assert page.banner.isHidden() and page.list.count() == 2
assert "1 use mods that are off" in page.count.text()
# Default's saves are checked against Default's own mods (Alpha is on there), not the active profile's
page.list.setCurrentRow([page.list.item(i).data(Qt.UserRole).kind for i in range(2)].index("Autosave"))
app.processEvents()
print("warning:", page.warn.text()[:90])
assert not page.warn.isHidden() and "Gone.esp" in page.warn.text()
assert "Alpha.esp" not in page.warn.text() and "profile “Default”" in page.warn.text()
assert page.shot.pixmap() is not None and not page.shot.pixmap().isNull()
moves = [a.text() for a in page.move_btn.menu().actions()]
assert "Hardcore" in moves and "Default" not in moves, moves
w.resize(1300, 850); app.processEvents()
page.grab().save(str(work / "saves_page.png"))
print("screenshot:", work / "saves_page.png")

# the banner's Folders menu opens the right folders; "Change game folder…" starts in the game's
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import QFileDialog
opened, started = [], []
QDesktopServices.openUrl = staticmethod(lambda u: (opened.append(u.toLocalFile()), True)[1])
QFileDialog.getExistingDirectory = staticmethod(lambda p, t, d, *a, **k: (started.append(d), "")[1])
(mg / "SKSE").mkdir(exist_ok=True)
w._fill_folder_menu()
acts = [a for a in w.folder_menu.actions() if a.text()]
print("folders menu:", [a.text() for a in acts])
for a in acts:
    if a.text().startswith("Open") and a.isEnabled():
        a.trigger()
want = {str(sk), str(sk / "Data"), str(inst.mods_dir), str(mg / "Saves/Hardcore"), str(mg / "SKSE"), str(mg)}
assert want <= set(opened), (want - set(opened), opened)
acts[-1].trigger()
assert acts[-1].text() == "Change game folder…" and started == [str(sk)], started

# deleting goes to the Trash
from PySide6.QtCore import QFile
trashed = []
QFile.moveToTrash = staticmethod(lambda p: (trashed.append(p), os.remove(p), True)[-1])
page.delete(); settle(lambda: page.list.count() == 1)
assert len(trashed) == 2 and any(t.endswith(".skse") for t in trashed), trashed

# turning it off: the game saves into Saves\ again
sg.turn_off(inst)
assert b"sLocalSavePath" not in ini.read_bytes() and b"sLanguage=ENGLISH" in ini.read_bytes()
page.refresh(); settle(lambda: not page.banner.isHidden())
assert not page.banner.isHidden()
print("saves smoke OK")
