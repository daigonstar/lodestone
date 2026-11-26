"""Watch your Downloads folder for mod archives you download by hand (the
Nexus website's “Manual download”, a script extender from its own site...) and
work out which game and mod each one is, so Kiln can install it.

Recognised:
  * Nexus files: the website names them “<Name>-<mod id>-<version>-<time>.ext”. Kiln
    checks the mod id against your games on Nexus and only accepts it when that
    mod really has a file of exactly this name.
  * Script extenders: skse64_2_02_08.7z, f4se_0_06_23.7z, sfse_*, nvse_*, obse_*, skse_*.
"""
from __future__ import annotations

import re
import time
from dataclasses import dataclass
from pathlib import Path

from PySide6.QtCore import QFileSystemWatcher, QObject, QStandardPaths, QTimer, Signal

ARCHIVES = (".7z", ".zip", ".rar")
XSE_NAMES = {"skse64": ("skyrimse", 30379, "Skyrim Script Extender (SKSE64)"),
             "f4se": ("fallout4", 42147, "Fallout 4 Script Extender (F4SE)"),
             "sfse": ("starfield", 0, "Starfield Script Extender (SFSE)"),
             "nvse": ("newvegas", 0, "xNVSE"), "obse": ("oblivion", 0, "xOBSE"),
             "skse": ("skyrim", 0, "Skyrim Script Extender (SKSE)")}


def default_folder() -> Path:
    p = QStandardPaths.writableLocation(QStandardPaths.DownloadLocation)
    return Path(p) if p else Path.home() / "Downloads"


@dataclass
class Found:
    path: Path
    game_id: str
    meta: dict
    how: str


def identify(path: Path, games) -> Found | None:
    """games: [(game_def, ...)] in the order to try (current game first)."""
    name = path.name
    low = name.lower()
    m = re.match(r"^(skse64|f4se|sfse|nvse|obse|skse)_(\d+(?:_\d+)+)(_gog)?\.(7z|zip)$", low)
    if m:
        gid, nexus_id, title = XSE_NAMES[m.group(1)]
        if any(g.id == gid for g in games):
            ver = ".".join(str(int(x)) for x in m.group(2).split("_"))
            return Found(path, gid, {"name": title, "version": ver, "nexus_mod_id": nexus_id or None},
                         f"{title} {ver}")
    stem = name[: -len(Path(name).suffix)]
    parts = stem.split("-")
    cands = [p for p in parts[1:] if p.isdigit() and len(p) < 9][:3]   # mod ids (not the timestamp)
    if not cands:
        return None
    from . import nexus
    for g in games:
        if not g.nexus_game_id:
            continue
        for mid in cands:
            try:
                files = nexus.mod_files(g.nexus_game_id, int(mid))
            except Exception:  # noqa: BLE001
                continue
            f = next((f for f in files if f.file_name and f.file_name.lower() == low), None)
            if f is None:
                continue
            try:
                det = nexus.mod_details(g.nexus_domain, g.nexus_game_id, int(mid))
                title = det.name
                extra = {"author": det.author, "category": det.category, "summary": det.summary,
                         "picture": det.picture}
            except Exception:  # noqa: BLE001
                title, extra = f.name, {}
            if f.category not in ("MAIN", "") and f.name and f.name.lower() not in title.lower():
                title = f"{title}: {f.name}"
            meta = {"name": title, "version": f.version, "nexus_mod_id": int(mid),
                    "nexus_file_id": f.file_id, "file_name": f.name, "file_date": f.date, **extra}
            return Found(path, g.id, meta, f"{title} {f.version} (Nexus mod {mid})")
    return None


class DownloadsWatcher(QObject):
    """Emits arrived(path) once a new archive in the folder has finished downloading."""
    arrived = Signal(object)

    def __init__(self, folder: Path, parent=None):
        super().__init__(parent)
        self.folder = Path(folder)
        self.started = time.time()
        self.seen: set[str] = set()
        self.sizes: dict[str, int] = {}
        self.fs = QFileSystemWatcher(self)
        if self.folder.is_dir():
            self.fs.addPath(str(self.folder))
        self.fs.directoryChanged.connect(lambda _p: self.timer.start())
        self.timer = QTimer(self, interval=2000)       # also catches downloads still growing
        self.timer.timeout.connect(self.scan)
        self.timer.start()

    def ignore(self, path: Path) -> None:
        self.seen.add(str(path))

    def scan(self) -> None:
        busy = False
        try:
            entries = list(self.folder.iterdir())
        except OSError:
            return
        names = {e.name for e in entries}
        for p in entries:
            if p.suffix.lower() not in ARCHIVES or str(p) in self.seen or not p.is_file():
                continue
            try:
                st = p.stat()
            except OSError:
                continue
            if st.st_mtime < self.started - 5:
                self.seen.add(str(p))          # was there before Kiln started
                continue
            # browsers write to a temporary name first
            if any(n.startswith(p.name) and n.endswith((".part", ".crdownload", ".download"))
                   for n in names):
                busy = True
                continue
            if self.sizes.get(str(p)) != st.st_size:
                self.sizes[str(p)] = st.st_size
                busy = True
                continue
            self.seen.add(str(p))
            self.arrived.emit(p)
        if not busy:
            self.timer.stop()
