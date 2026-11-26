"""Hermetic test of being polite to Nexus: GraphQL requests are paced, a "429 Too Many
Requests" is retried after the wait Nexus asks for (in the background only; the window
never freezes), answers are remembered for a while, and "Check for updates" asks for
last-updated dates in bulk instead of one request per mod.
Usage: python3 tests/smoke_ratelimit.py <empty work dir>"""
import os, sys, threading, time
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import nexus


class Resp:
    def __init__(self, code, data=None, headers=None):
        self.status_code, self._data, self.headers = code, data, headers or {}

    def json(self):
        if self._data is None:
            raise ValueError("not json")
        return self._data


posts, slept = [], []
script = []                       # responses to hand out, in order


def fake_post(url, json=None, timeout=None):
    posts.append(json)
    if script:
        return script.pop(0)
    q = json["query"]
    if "modFiles" in q:
        return Resp(200, {"data": {"modFiles": [{"fileId": 9, "name": "Main", "version": "2", "category": "MAIN",
                                                  "date": 2_000_000_000, "sizeInBytes": 1, "uri": "x.zip"}]}})
    if "legacyModsByDomain" in q and "ids" in json["variables"] and "nodes{ modId updatedAt }" in q:
        ids = [i["modId"] for i in json["variables"]["ids"]]
        # mod 7 was updated recently; the rest long ago
        return Resp(200, {"data": {"legacyModsByDomain": {"nodes": [
            {"modId": i, "updatedAt": "2030-01-01T00:00:00Z" if i == 7 else "2001-01-01T00:00:00Z"}
            for i in ids]}}})
    return Resp(200, {"data": {}})


nexus.session.post = fake_post
nexus.sleep = lambda s: slept.append(s)


def in_background(fn, *a):
    box = {}
    t = threading.Thread(target=lambda: box.update(r=_try(fn, *a)))
    t.start(); t.join()
    return box["r"]


def _try(fn, *a):
    try:
        return fn(*a)
    except Exception as e:  # noqa: BLE001
        return e


# 429 twice (once with Retry-After), then fine: retried in the background
script[:] = [Resp(429, None, {"Retry-After": "3"}), Resp(429), Resp(200, {"data": {"ok": 1}})]
slept.clear(); posts.clear()
r = in_background(nexus._gql, "query { ok }")
print("background:", r, "| waited", [s for s in slept if s >= 1])
assert r == {"ok": 1} and len(posts) == 3
assert [s for s in slept if s >= 1] == [3.0, 4], slept       # Nexus's wait, then the back-off

# on the window's thread: no waiting, a plain-English error at once
script[:] = [Resp(429)]
slept.clear()
try:
    nexus._gql("query { ok }")
    raise AssertionError("should have failed")
except nexus.NexusError as e:
    print("window thread:", e)
    assert "slow down" in str(e) and not [s for s in slept if s >= 1]

# it gives up after about a minute, with the same plain message
script[:] = [Resp(429, None, {"Retry-After": "40"})] * 3
r = in_background(nexus._gql, "query { ok }")
assert isinstance(r, nexus.NexusError) and "slow down" in str(r), r
script.clear()

# pacing: back-to-back requests from a worker are spaced out
slept.clear(); nexus._next_at = 0
in_background(lambda: [nexus._gql("query { ok }") for _ in range(4)])
assert len([s for s in slept if 0 < s < 1]) >= 3, slept     # (fake sleep: the waits add up)

# file lists are remembered: asking twice sends one request
posts.clear()
a = in_background(nexus.mod_files, 1704, 55)
b = in_background(nexus.mod_files, 1704, 55)
assert a == b and len(posts) == 1 and a[0].file_id == 9

# ---- "Check for updates": bulk dates, then file lists only for changed mods
from kiln import steam, steamcfg, launchguard
steam.libraries = lambda: [work / "lib"]; steam.steam_roots = lambda: [work / "steamroot"]
steamcfg.steam_running = lambda: False
launchguard.game_running = lambda inst: False
sk = work / "lib/steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
(sk / "SkyrimSE.exe").write_bytes(b"MZ"); (sk / "Data/Skyrim.esm").write_bytes(b"TES4" + b"\0" * 20)
(work / "lib/steamapps/appmanifest_489830.acf").write_text(
    '"AppState" { "appid" "489830" "installdir" "Skyrim Special Edition" }')
from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()
now = int(time.time())
for i in range(1, 61):                      # 60 mods from Nexus
    m = Mod(id=f"m{i}", name=f"Mod {i}", nexus_mod_id=i, nexus_file_id=1, installed_at=now)
    (inst.mod_path(m) / "x.esp").parent.mkdir(parents=True); (inst.mod_path(m) / "x.esp").write_bytes(b"TES4")
    inst.mods.append(m)
inst.save()
from PySide6.QtCore import QCoreApplication, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication
app = QApplication([])
from PySide6.QtWidgets import QMessageBox
errors = []
QMessageBox.critical = staticmethod(lambda p, t, m, *a, **k: errors.append((t, m)))
QMessageBox.information = staticmethod(lambda *a, **k: errors.append(a[1:3]))
from kiln import gui
w = gui.MainWindow()
w.instances["skyrimse"] = inst
idx = next(i for i, g in enumerate(w.games) if g.id == "skyrimse")
w.game_combo.setCurrentIndex(idx); w.select_game(idx)
end = time.time() + 3
while time.time() < end:                    # let the window's own start-up requests finish
    app.processEvents(); time.sleep(0.02)
nexus.forget_cache(); posts.clear()
w.updates = None
w.check_updates()
end = time.time() + 20
while time.time() < end and w.updates is None and not errors:
    app.processEvents(); time.sleep(0.02)
assert not errors, errors
kinds = ["dates" if "nodes{ modId updatedAt }" in p["query"] else "files" if "modFiles" in p["query"] else "?"
         for p in posts]
print("update check sent:", {k: kinds.count(k) for k in set(kinds)}, "| updates:", list(w.updates or {}))
assert kinds.count("dates") == 3, kinds              # 60 mods, 25 per request
assert kinds.count("files") == 1, kinds              # only mod 7 changed since it was installed
assert list(w.updates) == ["m7"]
print("rate limit smoke OK")
