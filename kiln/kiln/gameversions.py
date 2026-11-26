"""Which version of a game is installed, and switching to another one.

Older versions come from Steam itself: Steam's console command
`download_depot <app> <depot> <manifest>` fetches the files of an old build
for a game you own. Kiln shows the commands (only ones checked against several
modding guides), watches the downloads, then swaps the files into the game
folder. The files it replaces are kept, so you can switch back at any time.
Kiln also stops Steam from updating the game behind your back.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import stat
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Callable

from . import pe, steam, steamcfg

if TYPE_CHECKING:
    from .instance import Instance


@dataclass
class GameVersion:
    version: str                       # as the game's exe reports it, e.g. "1.6.1170.0"
    label: str
    why: str
    depots: dict[int, str]             # depot id -> manifest id
    exe_depot: int = 0                 # depot holding the exe (checked after download)
    notes: list[str] = field(default_factory=list)
    sources: list[str] = field(default_factory=list)


CATALOGUE: dict[str, list[GameVersion]] = {
    "skyrimse": [
        GameVersion(
            "1.6.1170.0", "Skyrim 1.6.1170 (January 2024)",
            "The version most mods and collections are made for. Most SKSE plugins support it.",
            {489831: "8442952117333549665", 489832: "8042843504692938467",
             489833: "1914580699073641964"}, exe_depot=489833,
            notes=["Steam downloads about 13 GB.",
                   "Needs the SKSE build for 1.6.1170 (from skse.silverlock.org). Kiln's "
                   "Troubleshoot page checks it after switching and links to it.",
                   "Address Library “All in one (Anniversary Edition)” covers 1.6.1170."],
            sources=["https://tech.sportskeeda.com/gaming-news/how-downgrade-skyrim-patch-1-6-1170",
                     "https://www.nexusmods.com/skyrimspecialedition/articles/12471"]),
    ],
}

CONTENT_CATALOG = {"skyrimse": "AppData/Local/Skyrim Special Edition/ContentCatalog.txt"}


# ------------------------------------------------------------------ what's installed
def installed_version(inst: "Instance") -> str:
    v = pe.file_version(inst.game_dir / inst.game.exe) if inst.game.exe else None
    return ".".join(map(str, v)) if v else ""


def short(v: str) -> str:
    """'1.6.1170.0' -> '1.6.1170'"""
    parts = (v or "").split(".")
    while len(parts) > 3 and parts[-1] == "0":
        parts.pop()
    return ".".join(parts)


def same(a: str, b: str) -> bool:
    return bool(a and b) and short(a) == short(b)


def for_game(game_id: str) -> list[GameVersion]:
    return CATALOGUE.get(game_id, [])


# ------------------------------------------------------------------ stored versions
def store_dir(inst: "Instance") -> Path:
    return inst.staging / "game-versions"


def _state_file(inst) -> Path:
    return store_dir(inst) / "state.json"


def state(inst: "Instance") -> dict:
    try:
        return json.loads(_state_file(inst).read_text())
    except (OSError, ValueError):
        return {}


def _save_state(inst, st: dict) -> None:
    _state_file(inst).parent.mkdir(parents=True, exist_ok=True)
    _state_file(inst).write_text(json.dumps(st, indent=1))


def stored(inst: "Instance") -> list[str]:
    """Versions kept by Kiln that you can switch back to."""
    d = store_dir(inst)
    return sorted((p.name for p in d.iterdir() if p.is_dir() and (p / "files").is_dir()), reverse=True) \
        if d.is_dir() else []


# ------------------------------------------------------------------ Steam downloads
def depot_dirs(appid: int, depot: int) -> list[Path]:
    """Where Steam may have put a download_depot result. Newer Steam clients use
    <Steam>/steamrt64/steamapps/content; its log says exactly where."""
    logged = []
    for r in steam.steam_roots():
        for name in ("console_log.txt", "content_log.txt"):
            try:
                text = (r / "logs" / name).read_text(errors="replace")[-2000000:]
            except OSError:
                continue
            for m in re.finditer(rf'Depot download complete : "([^"]*?app_{appid}[\\/]+depot_{depot})"', text):
                logged.append(Path(m.group(1).replace("\\", "/")))
    out = logged[::-1]                 # the newest download first
    roots = list(dict.fromkeys(list(steam.steam_roots()) + list(steam.libraries())))
    for r in roots:
        for sub in ("steamapps", "steamrt64/steamapps", "ubuntu12_32/steamapps"):
            out.append(r / sub / "content" / f"app_{appid}" / f"depot_{depot}")
    return list(dict.fromkeys(out))


def depot_dir(appid: int, depot: int) -> Path | None:
    return next((d for d in depot_dirs(appid, depot) if d.is_dir()), None)


def commands(inst: "Instance", gv: GameVersion) -> list[str]:
    return [f"download_depot {inst.game.steam_appid} {d} {m}" for d, m in gv.depots.items()]


def _size(d: Path) -> int:
    n = 0
    for dp, _ds, fs in os.walk(d):
        for f in fs:
            try:
                n += (Path(dp) / f).stat().st_size
            except OSError:
                pass
    return n


def _log_says_done(appid: int, depot: int, since: float = 0) -> bool:
    for r in steam.steam_roots():
        for name in ("console_log.txt", "content_log.txt"):
            f = r / "logs" / name
            try:
                if f.stat().st_mtime < since:
                    continue
                text = f.read_text(errors="replace")[-400000:]
            except OSError:
                continue
            if re.search(rf"Depot download complete.*?app_{appid}[\\/]+depot_{depot}", text):
                return True
    return False


class DepotWatch:
    """Tracks the depot downloads for one version: 'waiting' (not started),
    'downloading' (growing) or 'ready'."""

    def __init__(self, inst: "Instance", gv: GameVersion):
        self.inst, self.gv = inst, gv
        self.sizes: dict[int, tuple[int, float]] = {}

    def poll(self) -> dict[int, tuple[str, int]]:
        out = {}
        appid = self.inst.game.steam_appid
        for depot in self.gv.depots:
            d = depot_dir(appid, depot)
            if not d:
                out[depot] = ("waiting", 0)
                continue
            size = _size(d)
            last, since = self.sizes.get(depot, (-1, time.time()))
            if size != last:
                self.sizes[depot] = (size, time.time())
                since = time.time()
            stable = size > 0 and time.time() - since > 20
            ok_exe = True
            if depot == self.gv.exe_depot:
                exe = d / self.inst.game.exe
                v = pe.file_version(exe) if exe.is_file() else None
                ok_exe = bool(v) and same(".".join(map(str, v)), self.gv.version)
            done = (_log_says_done(appid, depot) or stable) and ok_exe and size > 0
            out[depot] = ("ready" if done else "downloading", size)
        return out


# ------------------------------------------------------------------ switching
def _move(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists() or dst.is_symlink():
        dst.unlink()
    try:
        os.replace(src, dst)
    except OSError:                       # another drive
        shutil.copy2(src, dst)
        src.unlink()


def _files(root: Path) -> list[str]:
    return sorted(Path(dp, f).relative_to(root).as_posix() for dp, _ds, fs in os.walk(root) for f in fs)


def _swap(inst: "Instance", src: Path, current: str, progress: Callable[[int, int], None] | None):
    """Move src's files into the game folder; what they replace goes to the store
    under the current version, so it can be swapped back."""
    game = inst.game_dir
    keep = store_dir(inst) / current
    keep_files = keep / "files"
    meta_f = keep / "version.json"
    try:
        meta = json.loads(meta_f.read_text())
    except (OSError, ValueError):
        meta = {"version": current, "absent": []}
    files = _files(src)
    for i, rel in enumerate(files):
        target = game / rel
        if target.exists():
            _move(target, keep_files / rel)
        elif rel not in meta["absent"]:
            meta["absent"].append(rel)       # not part of the current version: remove on the way back
        _move(src / rel, target)
        if progress:
            progress(i + 1, len(files))
    keep.mkdir(parents=True, exist_ok=True)
    meta_f.write_text(json.dumps(meta, indent=1))


def switch_to_download(inst: "Instance", gv: GameVersion, progress=None) -> str:
    """Install a downloaded old version. Returns the version that was replaced."""
    current = installed_version(inst)
    appid = inst.game.steam_appid
    dirs = [depot_dir(appid, d) for d in gv.depots]
    if not all(dirs):
        raise RuntimeError("Not all of Steam's downloads are there yet.")
    staging = store_dir(inst) / ".incoming"
    if staging.exists():
        shutil.rmtree(staging)
    for d in dirs:                            # merge the depots (later ones win, like the guides)
        for rel in _files(d):
            _move(d / rel, staging / rel)
    _swap(inst, staging, current, progress)
    shutil.rmtree(staging, ignore_errors=True)
    for d in dirs:
        shutil.rmtree(d, ignore_errors=True)
    _after_switch(inst, current, gv.version)
    return current


def switch_to_stored(inst: "Instance", version: str, progress=None) -> str:
    """Swap back to a version Kiln kept."""
    current = installed_version(inst)
    keep = store_dir(inst) / version
    files = keep / "files"
    try:
        absent = json.loads((keep / "version.json").read_text()).get("absent", [])
    except (OSError, ValueError):
        absent = []
    if not files.is_dir():
        raise RuntimeError(f"Kiln has no copy of {version}.")
    # files the stored version didn't have go into the current version's store
    cur_keep = store_dir(inst) / current
    for rel in absent:
        p = inst.game_dir / rel
        if p.exists():
            _move(p, cur_keep / "files" / rel)
    _swap(inst, files, current, progress)
    shutil.rmtree(keep, ignore_errors=True)
    _after_switch(inst, current, version)
    return current


def _after_switch(inst, old: str, new: str) -> None:
    # a ContentCatalog from the newer version makes older ones crash at start-up
    rel = CONTENT_CATALOG.get(inst.game.id)
    u = steam.prefix_user_dir(inst.game.steam_appid, inst.game_dir)
    if rel and u and (u / rel).is_file():
        _move(u / rel, store_dir(inst) / old / "ContentCatalog.txt")
    st = state(inst)
    st.setdefault("original", old)
    st["active"] = new
    st["history"] = (st.get("history") or []) + [{"from": old, "to": new, "at": int(time.time())}]
    if same(new, st.get("original", "")):
        st.pop("original", None)
    _save_state(inst, st)


# ------------------------------------------------------------------ keeping Steam from updating
def appmanifest(inst: "Instance") -> Path | None:
    for lib in steam.libraries():
        f = lib / "steamapps" / f"appmanifest_{inst.game.steam_appid}.acf"
        if f.is_file():
            return f
    return None


def update_protection(inst: "Instance") -> bool:
    f = appmanifest(inst)
    return bool(f and not os.access(f, os.W_OK))


def set_update_protection(inst: "Instance", on: bool) -> None:
    """On: Steam only updates the game when you launch it from Steam, and the game's
    manifest is made read-only so Steam can't start an update at all. Steam must
    be closed. Off undoes both."""
    f = appmanifest(inst)
    if not f:
        raise RuntimeError("Couldn't find the game's Steam manifest.")
    mode = f.stat().st_mode
    f.chmod(mode | stat.S_IWUSR)
    if on:
        if steamcfg.steam_running():
            raise RuntimeError("Steam is running. Close it first so it doesn't undo the change.")
        d = steamcfg.loads(f.read_text(errors="replace"))
        node = next((v for k, v in d.items() if k.lower() == "appstate" and isinstance(v, dict)), None)
        if node is None:
            raise RuntimeError("The game's Steam manifest has an unexpected layout.")
        node["AutoUpdateBehavior"] = "1"
        shutil.copy2(f, f.with_name(f.name + f".kiln-{time.strftime('%Y%m%d-%H%M%S')}.bak"))
        f.write_text(steamcfg.dumps(d))
        f.chmod((mode | stat.S_IRUSR) & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))
