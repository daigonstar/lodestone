"""Save games, kept per profile.

Bethesda games read `sLocalSavePath` from their main ini ([General] section, path
relative to Documents/My Games/<game>). With separate saves on, Kiln points it at
Saves\\<profile>\\ whenever the profile changes (and before every launch), so each
profile has its own saves and a save made with one mod list is never loaded with
another. Off, the game uses Saves\\ as usual.

The save files themselves are read for the Saves page: character, level,
location, in-game date, the screenshot, and the plugins the save was made with
(so Kiln can warn when a save uses mods the profile doesn't have).
"""
from __future__ import annotations

import re
import shutil
import struct
import time
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from . import steam
from .gamesettings import read_ini, write_ini

if TYPE_CHECKING:
    from .instance import Instance


@dataclass
class Spec:
    ini: str                    # main ini (in My Games/<game>) that holds sLocalSavePath
    ext: str                    # save file extension
    cosaves: tuple[str, ...]    # script extender co-save extensions kept with each save


SPECS = {
    "skyrimse": Spec("Skyrim.ini", ".ess", (".skse",)),
    "skyrim": Spec("Skyrim.ini", ".ess", (".skse",)),
    "fallout4": Spec("Fallout4.ini", ".fos", (".f4se",)),
    "newvegas": Spec("Fallout.ini", ".fos", (".nvse",)),
    "fallout3": Spec("Fallout.ini", ".fos", (".fose",)),
    "oblivion": Spec("Oblivion.ini", ".ess", (".obse",)),
}
ROOT = "Saves"
SHARED = ""                     # the folder name for Saves\ itself (no profile)


def supported(inst: "Instance") -> bool:
    return inst.game.id in SPECS and bool(getattr(inst.game, "mygames_name", ""))


def mygames(inst: "Instance") -> Path | None:
    u = steam.prefix_user_dir(inst.game.steam_appid, inst.game_dir)
    name = getattr(inst.game, "mygames_name", "")
    return u / "Documents/My Games" / name if (u and name) else None


def _ci(folder: Path, name: str) -> Path:
    """A file in folder, matching its name case-insensitively (Skyrim.INI vs Skyrim.ini)."""
    if folder.is_dir():
        for c in folder.iterdir():
            if c.name.lower() == name.lower():
                return c
    return folder / name


def ini_path(inst: "Instance") -> Path | None:
    mg = mygames(inst)
    return _ci(mg, SPECS[inst.game.id].ini) if (mg and supported(inst)) else None


# ------------------------------------------------------------------ per-profile folders
def enabled(inst: "Instance") -> bool:
    return bool(inst.saves.get("per_profile"))


def folder_name(profile: str) -> str:
    """A safe folder name for a profile."""
    n = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", profile).strip(" .")
    return n or "Profile"


def saves_dir(inst: "Instance", profile: str | None = None) -> Path | None:
    """Where a profile's saves live (Saves\\ itself when separate saves are off)."""
    mg = mygames(inst)
    if not mg:
        return None
    root = _ci(mg, ROOT)
    if profile == SHARED or not enabled(inst):
        return root
    return root / folder_name(profile or inst.active_profile)


def point_game(inst: "Instance") -> bool:
    """Make the game save to (and load from) the active profile's folder. Returns
    True if the ini changed. With separate saves off, removes Kiln's setting."""
    ini = ini_path(inst)
    if ini is None:
        return False
    cur = read_ini(ini).get("general", {}).get("slocalsavepath", "")
    if enabled(inst):
        want = f"{ROOT}\\{folder_name(inst.active_profile)}\\"
        d = saves_dir(inst)
        if d is not None:
            d.mkdir(parents=True, exist_ok=True)
    else:
        want = "" if cur.lower().startswith(f"{ROOT.lower()}\\") and cur.count("\\") == 2 else cur
    if cur == want:
        return False
    if want:
        write_ini(ini, {"General": {"sLocalSavePath": want}})
    else:
        _drop_key(ini, "General", "sLocalSavePath")
    return True


def _drop_key(path: Path, section: str, key: str) -> None:
    try:
        raw = path.read_bytes().decode("utf-8-sig", "replace")
    except OSError:
        return
    nl = "\r\n" if "\r\n" in raw else "\n"
    out, sec = [], ""
    for line in raw.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            sec = s[1:-1].lower()
        elif sec == section.lower() and s.split("=", 1)[0].strip().lower() == key.lower():
            continue
        out.append(line)
    with open(path, "r+b") as fh:
        fh.write((nl.join(out) + nl).encode("utf-8"))
        fh.truncate()


def turn_on(inst: "Instance", move_existing: bool) -> int:
    """Give each profile its own saves. Existing saves in Saves\\ can move into the
    active profile's folder. Returns how many saves moved."""
    root = saves_dir(inst, SHARED)
    inst.saves["per_profile"] = True
    inst.save()
    moved = 0
    if move_existing and root is not None and root.is_dir():
        dest = saves_dir(inst)
        dest.mkdir(parents=True, exist_ok=True)
        for s in list_saves(inst, SHARED, read=False):
            for f in s.files:
                f.rename(dest / f.name)
            moved += 1
    point_game(inst)
    return moved


def turn_off(inst: "Instance") -> None:
    """Back to one shared folder. The profiles' saves stay in their folders (the
    Saves page can still copy them out)."""
    inst.saves["per_profile"] = False
    inst.save()
    point_game(inst)


def rename_profile(inst: "Instance", old: str, new: str) -> None:
    """Called after a profile is renamed: its saves folder follows."""
    if not enabled(inst):
        return
    a, b = saves_dir(inst, old), saves_dir(inst, new)
    if a and b and a.is_dir() and not b.exists():
        a.rename(b)
    point_game(inst)


def profile_folders(inst: "Instance") -> list[tuple[str, Path]]:
    """(label, folder) for every place saves can be: each profile, then Saves\\ itself."""
    out = []
    if enabled(inst):
        for n in inst.profile_names():
            d = saves_dir(inst, n)
            if d is not None:
                out.append((n, d))
    root = saves_dir(inst, SHARED)
    if root is not None:
        out.append((SHARED, root))
    return out


# ------------------------------------------------------------------ reading saves
@dataclass
class Save:
    path: Path
    files: list[Path]                   # the save and its co-saves
    mtime: float
    size: int
    player: str = ""
    level: int = 0
    location: str = ""
    game_date: str = ""                 # time played (Skyrim: "001.59.36" hours.minutes.seconds)
    race: str = ""
    number: int = 0
    shot: tuple[int, int, bytes] | None = None     # width, height, RGBA
    plugins: list[str] = field(default_factory=list)
    error: str = ""

    @property
    def kind(self) -> str:
        n = self.path.name.lower()
        return "Autosave" if n.startswith("autosave") else "Quicksave" if n.startswith("quicksave") else \
            "Exit save" if n.startswith("exitsave") else "Save"

    @property
    def character(self) -> str:
        """Saves of the same character share an id in their file name."""
        m = re.match(r"(?:\w+?)_([0-9A-F]{8})_", self.path.name, re.I)
        return f"{self.player} ({m.group(1)})" if m else self.player

    @property
    def played(self) -> str:
        m = re.match(r"(\d+)\.(\d+)\.(\d+)$", self.game_date)
        if not m:
            return self.game_date
        h, mi, _sec = (int(x) for x in m.groups())       # hours.minutes.seconds of real play
        return f"{h}h {mi:02d}m played" if h else f"{mi}m played"


def list_saves(inst: "Instance", profile: str | None = None, read: bool = True) -> list[Save]:
    """Saves in a profile's folder (or Saves\\ itself for SHARED), newest first."""
    spec = SPECS.get(inst.game.id)
    d = saves_dir(inst, profile)
    if spec is None or d is None or not d.is_dir():
        return []
    out = []
    for p in d.iterdir():
        if p.is_file() and p.suffix.lower() == spec.ext:
            files = [p] + [c for c in (p.with_suffix(x) for x in spec.cosaves) if c.is_file()]
            st = p.stat()
            s = Save(p, files, st.st_mtime, sum(f.stat().st_size for f in files))
            if read:
                try:
                    read_save(s)
                except Exception as e:  # noqa: BLE001
                    s.error = f"Couldn't read it: {e}"
            out.append(s)
    out.sort(key=lambda s: s.mtime, reverse=True)
    return out


_cache: dict[tuple[str, float], Save] = {}


def read_save(s: Save, plugins: bool = True) -> Save:
    key = (str(s.path), s.mtime)
    hit = _cache.get(key)
    if hit is not None:
        for k in ("player", "level", "location", "game_date", "race", "number", "shot", "plugins"):
            setattr(s, k, getattr(hit, k))
        return s
    with open(s.path, "rb") as fh:
        head = fh.read(16)
        fh.seek(0)
        if head.startswith(b"TESV_SAVEGAME"):
            _read_tesv(fh, s, plugins)
        elif head.startswith(b"FO4_SAVEGAME"):
            _read_fo4(fh, s, plugins)
        else:
            s.error = "Kiln can't read this game's save details."
    _cache[key] = s
    return s


class _R:
    def __init__(self, b: bytes):
        self.b, self.i = b, 0

    def u8(self):
        self.i += 1
        return self.b[self.i - 1]

    def u16(self):
        self.i += 2
        return struct.unpack_from("<H", self.b, self.i - 2)[0]

    def u32(self):
        self.i += 4
        return struct.unpack_from("<I", self.b, self.i - 4)[0]

    def f32(self):
        self.i += 4
        return struct.unpack_from("<f", self.b, self.i - 4)[0]

    def u64(self):
        self.i += 8
        return struct.unpack_from("<Q", self.b, self.i - 8)[0]

    def ws(self) -> str:
        n = self.u16()
        self.i += n
        return self.b[self.i - n:self.i].decode("utf-8", "replace")


def _read_tesv(fh, s: Save, plugins: bool) -> None:
    fh.read(13)
    hsize = struct.unpack("<I", fh.read(4))[0]
    r = _R(fh.read(hsize))
    ver = r.u32()
    s.number = r.u32()
    s.player, s.level, s.location, s.game_date, s.race = r.ws(), r.u32(), r.ws(), r.ws(), r.ws()
    r.u16(), r.f32(), r.f32(), r.u64()                 # sex, xp, xp to level, file time
    w, h = r.u32(), r.u32()
    comp = r.u16() if ver >= 12 else 0
    bpp = 4 if ver >= 11 else 3
    pix = fh.read(w * h * bpp)
    if bpp == 3:
        pix = b"".join(pix[i:i + 3] + b"\xff" for i in range(0, len(pix), 3))
    s.shot = (w, h, pix)
    if not plugins:
        return
    if comp:
        ulen, clen = struct.unpack("<II", fh.read(8))
        data = fh.read(clen)
        body = zlib.decompress(data) if comp == 1 else lz4_block(data, ulen, limit=600_000)
    else:
        body = fh.read(600_000)
    b = _R(body)
    form = b.u8()
    b.u32()                                            # plugin info size
    names = [b.ws() for _ in range(b.u8())]
    if form >= 78 and ver >= 12:
        names += [b.ws() for _ in range(b.u16())]
    s.plugins = names


def _read_fo4(fh, s: Save, plugins: bool) -> None:
    fh.read(12)
    hsize = struct.unpack("<I", fh.read(4))[0]
    r = _R(fh.read(hsize))
    r.u32()
    s.number = r.u32()
    s.player, s.level, s.location, s.game_date, s.race = r.ws(), r.u32(), r.ws(), r.ws(), r.ws()
    r.u16(), r.f32(), r.f32(), r.u64()
    w, h = r.u32(), r.u32()
    s.shot = (w, h, fh.read(w * h * 4))
    if not plugins:
        return
    b = _R(fh.read(600_000))
    form = b.u8()
    b.ws()                                             # game version
    b.u32()
    names = [b.ws() for _ in range(b.u8())]
    if form >= 68:
        names += [b.ws() for _ in range(b.u16())]
    s.plugins = names


def lz4_block(src: bytes, size: int, limit: int | None = None) -> bytes:
    """Decompress an LZ4 block (Skyrim SE saves). Stops once `limit` bytes are out."""
    try:
        import lz4.block
        return lz4.block.decompress(src, uncompressed_size=size)
    except ImportError:
        pass
    out = bytearray()
    i, n = 0, len(src)
    stop = min(size, limit) if limit else size
    while i < n and len(out) < stop:
        tok = src[i]
        i += 1
        lit = tok >> 4
        if lit == 15:
            while True:
                x = src[i]
                i += 1
                lit += x
                if x != 255:
                    break
        out += src[i:i + lit]
        i += lit
        if i >= n:
            break
        off = src[i] | (src[i + 1] << 8)
        i += 2
        ml = tok & 15
        if ml == 15:
            while True:
                x = src[i]
                i += 1
                ml += x
                if x != 255:
                    break
        ml += 4
        start = len(out) - off
        for k in range(ml):              # may overlap itself
            out.append(out[start + k])
    return bytes(out)


# ------------------------------------------------------------------ checks
BASE = {"skyrim.esm", "update.esm", "dawnguard.esm", "hearthfires.esm", "dragonborn.esm",
        "fallout4.esm", "dlcrobot.esm", "dlcworkshop01.esm", "dlccoast.esm", "dlcworkshop02.esm",
        "dlcworkshop03.esm", "dlcnukaworld.esm", "dlcultrahighresolution.esm",
        "falloutnv.esm", "fallout3.esm", "oblivion.esm"}


def loaded_plugins(inst: "Instance", profile: str | None = None) -> set[str]:
    """Plugins a profile loads (the active one by default): its enabled plugins, the
    base game's, and Creation Club content (loaded automatically)."""
    plugins = inst.plugins
    if profile and profile != inst.active_profile and profile in inst.profiles:
        plugins = inst.profiles[profile].get("plugins", [])
    out = {p["name"].lower() for p in plugins if p.get("enabled")} | BASE
    data = inst.deploy_dir
    if data.is_dir():
        out |= {c.name.lower() for c in data.iterdir() if c.name.lower().startswith("cc")
                and c.suffix.lower() in (".esm", ".esl")}
    return out


def missing(inst: "Instance", s: Save, have: set[str] | None = None) -> list[str]:
    """Plugins the save was made with that this profile doesn't load."""
    have = have if have is not None else loaded_plugins(inst)
    return [p for p in s.plugins if p.lower() not in have]


# ------------------------------------------------------------------ actions
def copy_to(inst: "Instance", s: Save, profile: str, move: bool = False) -> Path:
    dest = saves_dir(inst, profile)
    if dest is None:
        raise RuntimeError("The game's save folder isn't there yet.")
    dest.mkdir(parents=True, exist_ok=True)
    if (dest / s.path.name).exists():
        raise RuntimeError(f"There's already a save called {s.path.name} there.")
    for f in s.files:
        (f.rename if move else lambda d, f=f: shutil.copy2(f, d))(dest / f.name)
    return dest / s.path.name


def delete(s: Save) -> bool:
    """Move a save (and its co-save) to the Trash. False if there's no Trash."""
    from PySide6.QtCore import QFile
    ok = True
    for f in s.files:
        ok = QFile.moveToTrash(str(f)) and ok
    return ok


def backup(inst: "Instance", profile: str | None, dest_dir: Path) -> Path:
    """Zip every save in a profile's folder. Returns the zip."""
    saves = list_saves(inst, profile, read=False)
    dest_dir.mkdir(parents=True, exist_ok=True)
    label = folder_name(profile or inst.active_profile) if profile != SHARED else "shared"
    z = dest_dir / f"{inst.game.id}-{label}-saves-{time.strftime('%Y%m%d-%H%M%S')}.zip"
    with zipfile.ZipFile(z, "w", zipfile.ZIP_STORED) as zf:      # saves are already compressed
        for s in saves:
            for f in s.files:
                zf.write(f, f.name)
    return z


def thumbnail_png(s: Save) -> bytes:
    """The screenshot as PNG bytes (for tests and exports)."""
    from PySide6.QtCore import QBuffer
    from PySide6.QtGui import QImage
    if not s.shot:
        return b""
    w, h, pix = s.shot
    img = QImage(pix, w, h, w * 4, QImage.Format_RGBA8888).copy()
    buf = QBuffer()
    buf.open(QBuffer.WriteOnly)
    img.save(buf, "PNG")
    return bytes(buf.data())

