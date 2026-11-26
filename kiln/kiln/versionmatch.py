"""Matching mods to a game version.

Nexus keeps a mod's older files (OLD_VERSION, sometimes ARCHIVED), and authors of
version-specific mods say which game version each file is for: “Compatible with
Skyrim Special Edition 1.6.1170 from Steam”, “for 1.6.629-1.6.640”, “AE 1.6.1130+”.
best_file() reads that and picks the file for your version. plan() works out
what a setup needs after a game version change: the script extender build,
Address Library, and every mod whose SKSE plugin won't load.
"""
from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .instance import Instance, Mod

VER = r"(\d+\.\d+\.\d+)(?:\.\d+)?"


def _v(s: str) -> tuple[int, ...]:
    return tuple(int(x) for x in s.split(".")[:3])


def mentions(text: str) -> list[tuple[str, tuple, tuple]]:
    """Game versions a file's text talks about: [(kind, low, high)] where kind is
    'exact', 'range' or 'from' (an open range, “1.6.1130+” / “1.6.1130 and later”)."""
    t = re.sub(r"<[^>]+>|\[/?\w+[^\]]*\]", " ", text.lower())       # html and bbcode
    out = []
    for m in re.finditer(r"\b(\d+)\.(\d+)\.x+\b", t):                     # “AE 1.6.xxx”
        a, b = int(m.group(1)), int(m.group(2))
        out.append(("family", (a, b, 0), (a, b, 99999)))
    t = re.sub(r"\b\d+\.\d+\.x+\b", " ", t)
    for m in re.finditer(rf"{VER}\s*(?:and|or)\s*(?:earlier|below|older|lower)", t):
        out.append(("upto", (0, 0, 0), _v(m.group(1))))
    t = re.sub(rf"{VER}\s*(?:and|or)\s*(?:earlier|below|older|lower)", " ", t)
    # “added support for 1.7.99, previous versions are still compatible”
    older = (rf"{VER}[^.\d]{{0,40}}?(?:all\s+)?(?:previous|earlier|older)\s+(?:game\s+)?versions?\s+"
             r"(?:are\s+|is\s+)?(?:still\s+|also\s+)?(?:compatible|supported|work)")
    for m in re.finditer(older, t):
        out.append(("upto", (0, 0, 0), _v(m.group(1))))
    t = re.sub(older, " ", t)
    for m in re.finditer(rf"(?:up to|until|through)\s*(?:version\s*)?{VER}", t):
        out.append(("upto", (0, 0, 0), _v(m.group(1))))
    t = re.sub(rf"(?:up to|until|through)\s*(?:version\s*)?{VER}", " ", t)
    for m in re.finditer(rf"{VER}\s*(?:-|–|to|through)\s*{VER}", t):
        out.append(("range", _v(m.group(1)), _v(m.group(2))))
    t2 = re.sub(rf"{VER}\s*(?:-|–|to|through)\s*{VER}", " ", t)
    for m in re.finditer(rf"{VER}\s*(\+|and (?:up|later|newer|above)|or (?:later|newer))?", t2):
        v = _v(m.group(1))
        out.append(("from" if m.group(2) else "exact", v, (99, 0, 0) if m.group(2) else v))
    return out


def fit(text: str, rt: tuple[int, ...]) -> int:
    """How well a file's text fits game version rt: 10 exact, 8 in a range,
    6 an open range, 0 says nothing, -6 is for other versions."""
    ms = [m for m in mentions(text) if m[1][:2] == rt[:2] or m[0] != "exact"]  # ignore e.g. mod versions
    ms = [m for m in ms if m[2][0] == rt[0] or m[1][0] == rt[0]]               # same game generation
    if not ms:
        return 0
    best = -6
    for kind, lo, hi in ms:
        if kind == "exact" and lo == tuple(rt[:3]):
            best = max(best, 10)
        elif kind == "range" and lo <= tuple(rt[:3]) <= hi:
            best = max(best, 8)
        elif kind == "family" and lo <= tuple(rt[:3]) <= hi:
            best = max(best, 7)
        elif kind in ("from", "upto") and lo <= tuple(rt[:3]) <= hi:
            best = max(best, 6)
    return best


CAT_RANK = {"MAIN": 3, "UPDATE": 2, "OPTIONAL": 1, "OLD_VERSION": 0, "ARCHIVED": -1}


def best_file(files, rt: tuple[int, ...], mine=None, platform: str = "steam"):
    """(NexusFile, how sure: 'exact' | 'likely' | 'guess') or (None, why not)."""
    usable = [f for f in files if f.category in CAT_RANK]
    if not usable:
        return None, "The mod has no downloadable files."

    def score(f):
        text = f"{f.name} {f.description}"     # not f.version: that's the mod's own version
        s = fit(text, rt) * 10
        low = f.name.lower()                 # platform words count in the file's name only
        if platform != "gog" and re.search(r"\bgog\b", low):
            s -= 50
        if re.search(r"\bvr\b", low) and "vr" not in (mine.name.lower() if mine else ""):
            s -= 50
        if mine is not None and _family(f.name) == _family(mine.name):
            s += 5                           # the same file of the mod, another version
        # any file that supports the version beats one that doesn't say; among those,
        # the current main file beats old/archived ones, then the most precise, then newest
        level = 2 if s >= 55 else (1 if s >= 0 else 0)
        return (level, CAT_RANK[f.category], s, f.date)

    ranked = sorted(usable, key=score, reverse=True)
    top = ranked[0]
    level, _cat, s, _d = score(top)
    if level == 0:
        return None, "Every file on the mod's page is marked for other game versions."
    return top, ("exact" if s >= 80 else "likely" if s >= 55 else "guess")


def _family(name: str) -> str:
    """A file's name without version numbers and platform words."""
    n = re.sub(rf"{VER}|\bv?\d+(?:[._]\d+)*\b|\b(steam|gog|ae|se|for|skyrim|special|edition|anniversary)\b",
               " ", name.lower())
    return " ".join(n.split())


# ------------------------------------------------------------------ what a setup needs
@dataclass
class Need:
    title: str
    why: str
    nexus_id: int
    mod: "Mod | None" = None          # the Kiln mod it replaces (None: a new install)
    file: object = None               # NexusFile chosen
    sure: str = ""
    problem: str = ""                 # set when nothing suitable was found
    files: list = field(default_factory=list)
    reuse: "Mod | None" = None        # a copy already downloaded that works: switch to it


def _staged_dlls(inst: "Instance", mod) -> list[Path]:
    root = inst.mod_path(mod)
    return [root / r for r in inst.mod_files(mod)
            if r.lower().endswith(".dll") and "skse/plugins/" in r.lower()]


def broken_plugins(inst: "Instance", rt: tuple[int, ...]) -> list[tuple["Mod", list[str]]]:
    """Enabled mods with SKSE plugins that SKSE would refuse on game version rt."""
    from . import pe
    from .doctor import XSE
    x = XSE.get(inst.game.id)
    if not x:
        return []
    out = []
    for m in inst.mods:
        if not m.enabled:
            continue
        bad = []
        for f in _staged_dlls(inst, m):
            info = pe.xse_plugin(f)
            code, _why = info.verdict(rt, None, True)
            if code not in ("ok", "addrlib"):
                bad.append(f.name)
        if bad:
            out.append((m, bad))
    return out


def plan(inst: "Instance", rt: tuple[int, ...] | None = None) -> list[Need]:
    """Everything to (re)install so the setup fits game version rt (default: installed)."""
    from . import nexus, pe
    from .archive import ci_resolve
    from .doctor import XSE, runtime
    rt = tuple(rt or runtime(inst) or ())
    x = XSE.get(inst.game.id)
    if not rt or not x:
        return []
    rts = ".".join(map(str, rt))
    gid = inst.game.nexus_game_id
    needs: list[Need] = []
    # 1. the script extender build for this game version
    dll = x["dll"].format("_".join(map(str, rt)))
    if not ci_resolve(inst.game_dir, dll):
        needs.append(Need(f"{x['name']} for {x['game']} {rts}",
                          f"The {x['name']} you have is for another game version.",
                          x["nexus"], inst.find_nexus(x["nexus"])))
    # 2. Address Library covering it
    plugins = inst.game_dir / inst.game.deploy_root / x["plugins"]
    names = [f"versionlib-{rt[0]}-{rt[1]}-{rt[2]}-0.bin", f"version-{rt[0]}-{rt[1]}-{rt[2]}-0.bin"]
    if plugins.is_dir() and not any(ci_resolve(plugins, n) for n in names):
        needs.append(Need(f"Address Library for {x['game']} {rts}",
                          "Most SKSE plugins need it to find their way around this game version.",
                          x["addrlib"], inst.find_nexus(x["addrlib"])))
    # 3. mods whose SKSE plugins won't load on this version (skipping ones with another,
    #    working copy installed: Troubleshoot's duplicate fix handles those)
    broken = broken_plugins(inst, rt)
    broken_ids = {m.id for m, _ in broken}
    working = {m.nexus_mod_id for m in inst.mods if m.enabled and m.nexus_mod_id and m.id not in broken_ids
               and _staged_dlls(inst, m)}
    for m, bad in broken:
        if m.nexus_mod_id and m.nexus_mod_id in working:
            continue
        if not m.nexus_mod_id:
            needs.append(Need(m.name, f"{', '.join(bad)} won't load on {rts}.", 0, m,
                              problem="Not from Nexus: Kiln can't look for another version."))
            continue
        if m.nexus_mod_id in (x["nexus"], x["addrlib"]):
            continue
        needs.append(Need(m.name, f"{', '.join(bad)} won't load on {rts}.", m.nexus_mod_id, m))
    # a working copy already downloaded beats a download
    for n in needs:
        if n.mod is None or not n.nexus_id:
            continue
        for o in inst.mods:
            if o is n.mod or o.nexus_mod_id != n.nexus_id or o.enabled:
                continue
            dlls = _staged_dlls(inst, o)
            if dlls and all(pe.xse_plugin(f).verdict(rt, None, True)[0] in ("ok", "addrlib") for f in dlls):
                if n.reuse is None or (o.nexus_file_id or 0) > (n.reuse.nexus_file_id or 0):
                    n.reuse = o
    # look up files
    for n in needs:
        if not n.nexus_id or n.reuse is not None:
            continue
        try:
            n.files = nexus.mod_files(gid, n.nexus_id)
        except Exception as e:  # noqa: BLE001
            n.problem = f"Couldn't read the mod's files on Nexus: {e}"
            continue
        f, sure = best_file(n.files, rt, n.mod)
        if f is None:
            n.problem = sure
        elif n.mod and n.mod.nexus_file_id == f.file_id:
            n.problem = (f"The file you have is already the best match on Nexus, but it doesn't "
                         f"support {rts}. The author hasn't made a version for it.")
        else:
            n.file, n.sure = f, sure
    return needs
