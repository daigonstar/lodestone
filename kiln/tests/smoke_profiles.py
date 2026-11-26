"""Hermetic test of profile-scoped mod lists and the Nexus/Collections category filters.
Usage: python3 tests/smoke_profiles.py <empty work dir> [screenshot dir]"""
import json, os, sys, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
shots = Path(sys.argv[2]) if len(sys.argv) > 2 else None
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, nexus, collections as nc
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
from kiln import launchguard
launchguard.game_running = lambda inst: False      # a real game may be running on this machine
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ"); (sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(lib / "steamapps/appmanifest_489830.acf").write_text('"AppState" { "installdir" "Skyrim Special Edition" }')
(lib / "steamapps/compatdata/489830/pfx/drive_c/users/steamuser").mkdir(parents=True)

# ---- an old-format state file (no "members"): Default made first, "Survival" later
st = work / "staging"
mods = []
for i, (name, t) in enumerate([("A", 100), ("B", 110), ("C", 120), ("D", 300), ("E", 310)]):
    (st / "mods" / name).mkdir(parents=True)
    (st / "mods" / name / f"{name}.txt").write_text(name)
    mods.append({"id": name, "name": f"Mod {name}", "enabled": name in ("A", "B"), "installed_at": t,
                 "nexus_mod_id": 1000 + i, "nexus_file_id": 1})
(st / "kiln.json").write_text(json.dumps({
    "game": "skyrimse", "mods": mods, "plugins": [], "collections": [], "active_profile": "Default",
    "profiles": {"Default": {"order": [m["id"] for m in mods], "enabled": ["A", "B"], "plugins": [], "created": 50},
                 "Survival": {"order": [m["id"] for m in mods], "enabled": ["D"], "plugins": [], "created": 250}}}))

from kiln.games import by_id
from kiln.instance import Instance
inst = Instance(by_id("skyrimse"), sk, st); inst.load()
print("migrated:", {n: sorted(inst.members(n)) for n in inst.profile_names()})
assert inst.members("Default") == {"A", "B", "C"}, inst.members("Default")     # C: installed while Default was newest
assert inst.members("Survival") == {"D", "E"}

# ---- categories (fake Nexus)
calls = []
nexus.mod_categories = lambda d: [("Armour", 120), ("Bug Fixes", 30)]
nc.categories = lambda gid: ["Essentials", "Themed"]
nc.game_versions = lambda d: []
nexus.search = lambda *a, **k: (calls.append(("mods", a)), ([], 0))[1]
nc.search = lambda *a, **k: (calls.append(("colls", a)), ([], 0))[1]

from PySide6.QtCore import QCoreApplication, QEventLoop, QTimer, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
asked = []
QMessageBox.question = staticmethod(lambda p, t, m, *a, **k: (asked.append(m), QMessageBox.Yes)[1])
errors = []
QMessageBox.information = staticmethod(lambda p, t, m, *a, **k: None)
QMessageBox.warning = staticmethod(lambda p, t, m, *a, **k: None)
QMessageBox.critical = staticmethod(lambda p, t, m, *a, **k: errors.append((t, m)))
from kiln import gui


def wait(cond, secs=20):
    loop, end = QEventLoop(), time.time() + secs
    t = QTimer(interval=50, timeout=lambda: (cond() or time.time() > end) and loop.quit())
    t.start(); loop.exec(); t.stop(); return cond()


def shown():
    return [w.mod_tree.topLevelItem(i).data(0, Qt.UserRole) for i in range(w.mod_tree.topLevelItemCount())]


w = gui.MainWindow(); w.resize(1400, 900); w.show()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
w.go_to(w.mods_tab)
print("Default shows:", shown(), "|", w.mods_hint.text().split(".")[0])
assert shown() == ["A", "B", "C"]
assert "mods in this profile" in w.header.stats.text()

w.show_all_mods.setChecked(True)
print("show all:", shown())
assert shown() == ["A", "B", "C", "D", "E"]
it = next(w.mod_tree.topLevelItem(i) for i in range(5) if w.mod_tree.topLevelItem(i).data(0, Qt.UserRole) == "E")
assert it.font(0).italic() and it.text(5) == "other profile", (it.font(0).italic(), it.text(5))
if shots:
    shots.mkdir(parents=True, exist_ok=True); w.grab().save(str(shots / "mods-show-all.png"))
it.setCheckState(0, Qt.Checked)                # tick a mod from another profile: it joins this one
w.show_all_mods.setChecked(False)
print("after ticking E:", shown())
assert shown() == ["A", "B", "C", "E"] and inst.get("E").enabled
w.remove_mods_from_profile([inst.get("B")])
print("after removing B from profile:", shown(), "| B enabled:", inst.get("B").enabled)
assert shown() == ["A", "C", "E"] and not inst.get("B").enabled
assert "B" not in inst.members("Default") and inst.profiles_using(inst.get("B")) == []

w.switch_profile("Survival") if hasattr(w, "switch_profile") else inst.switch_profile("Survival")
wait(lambda: inst.active_profile == "Survival", 10); w.refresh_mods()
print("Survival shows:", shown())
assert shown() == ["D", "E"], shown()
w.remove_mods([inst.get("E")])                 # E is in both: the question warns about Default
assert any("Default" in m for m in asked), asked[-1:]
print("delete warning mentions the other profile ✔")

# a fresh empty profile shows nothing until mods are added
inst.create_profile("Fresh"); inst.switch_profile("Fresh"); w.refresh_mods()
print("Fresh shows:", shown(), "|", w.mods_hint.text().split(".")[0])
assert shown() == []
if shots:
    w.grab().save(str(shots / "mods-fresh-profile.png"))

# ---- essentials: the banner button, and reusing a download from another profile
from kiln import essentials
al = next(e for e in essentials.for_game("skyrimse") if e.key == "addresslib")
(st / "mods/AddrLib/SKSE/Plugins").mkdir(parents=True)
(st / "mods/AddrLib/SKSE/Plugins/versionlib-1-6-1170-0.bin").write_text("b")
from kiln.instance import Mod
inst.switch_profile("Survival")
inst.mods.append(Mod(id="AddrLib", name="Address Library", nexus_mod_id=32444, nexus_file_id=7))
inst.add_to_profile([inst.get("AddrLib")])
inst.switch_profile("Fresh"); w.refresh_all()
b = w.header.ess_btn
print("banner button:", b.isVisible(), "|", b.text(), "|", b.property("alert"))
assert b.isVisible() and b.text().startswith("Install essentials") and b.property("alert") == "required"
assert essentials.status(inst, al) == "elsewhere"
w.open_setup(); dlg = w._setup_dlg
print("dialog says:", dlg.items["addresslib"].text(4))
assert dlg.items["addresslib"].text(4) == "downloaded: will reuse"
for k, it in dlg.items.items():
    if k != "addresslib" and it.flags() & Qt.ItemIsUserCheckable:
        it.setCheckState(0, Qt.Unchecked)
n_before = len(inst.mods)
dlg.install()
wait(lambda: dlg.items["addresslib"].text(4).startswith("✔"), 10)
print("after install:", dlg.items["addresslib"].text(4), "| enabled:", inst.get("AddrLib").enabled,
      "| mods:", n_before, "->", len(inst.mods))
assert inst.get("AddrLib").enabled and inst.in_profile(inst.get("AddrLib")) and len(inst.mods) == n_before
assert "AddrLib" in inst.members("Survival"), "still in the other profile too"
w.refresh_top()
print("button now:", b.text())
assert "(" in b.text() and b.text() != "Install essentials (4)"
if shots:
    dlg.close(); w.go_to(w.mods_tab); w.grab().save(str(shots / "banner-essentials.png"))
print("errors:", errors)

# ---- category filters
w.go_to(w.nexus_tab); w.search_nexus(1)
assert wait(lambda: w.n_cat.count() == 3)
print("Nexus categories:", [w.n_cat.itemText(i) for i in range(w.n_cat.count())])
calls.clear(); w.n_cat.setCurrentIndex(w.n_cat.findData("Bug Fixes"))
assert wait(lambda: calls), "choosing a category should search"
print("search args:", calls[-1][1][-1])
assert calls[-1][1][-1] == "Bug Fixes"
w.go_to(w.coll_tab); w.search_collections(1)
assert wait(lambda: w.c_cat.count() == 3)
calls.clear(); w.c_cat.setCurrentIndex(w.c_cat.findData("Themed"))
assert wait(lambda: calls) and calls[-1][1][-2] == "Themed", calls
print("Collections categories:", [w.c_cat.itemText(i) for i in range(w.c_cat.count())], "→ filtered ✔")
if shots:
    w.grab().save(str(shots / "collections-category.png"))
# installing a mod yourself switches it on in this profile, even if Kiln already had it
# switched off here (the SSE Display Tweaks case); a reinstall keeps its state
import zipfile
z = work / "dt.zip"
with zipfile.ZipFile(z, "w") as zz:
    zz.writestr("SKSE/Plugins/Tweaks.ini", "[Render]\nFramerateLimit=240\n")
off = next(m for m in inst.mods if not m.enabled and m.nexus_mod_id)
meta = {"name": off.name, "nexus_mod_id": off.nexus_mod_id, "nexus_file_id": off.nexus_file_id,
        "file_name": off.file_name}
new = inst.finish_install(inst.prepare(z, "dt", dict(meta)))
assert new.id == off.id and new.enabled, "installed it yourself: on"
new.enabled = False; inst.save()
again = inst.finish_install(inst.prepare(z, "dt", dict(meta, replace_id=new.id)))
assert not again.enabled, "a reinstall keeps it off"
print("profiles smoke OK")
