"""Bring mods over from other Linux mod managers (Amethyst for now)."""
from __future__ import annotations

import shutil
import time
from pathlib import Path
from typing import TYPE_CHECKING

from .games import safe_name

if TYPE_CHECKING:
    from .instance import Instance

AMETHYST_NAMES = {"cyberpunk2077": "Cyberpunk 2077", "stardewvalley": "Stardew Valley",
                  "skyrimse": "Skyrim Special Edition", "fallout4": "Fallout 4",
                  "witcher3": "The Witcher 3", "nomanssky": "No Man's Sky", "7daystodie": "7 Days to Die"}


def amethyst_mods_dir(game_id: str) -> Path | None:
    name = AMETHYST_NAMES.get(game_id)
    d = Path.home() / "Games/Amethyst" / name / "mods" if name else None
    return d if d and d.is_dir() else None


def amethyst_candidates(inst: "Instance") -> list[Path]:
    d = amethyst_mods_dir(inst.game.id)
    if not d:
        return []
    have = {m.name.lower() for m in inst.mods}
    return [p for p in sorted(d.iterdir(), key=lambda p: p.name.lower())
            if p.is_dir() and p.name.lower() not in have and any(p.rglob("*"))]


def _meta(folder: Path) -> dict:
    """Amethyst/MO2-style meta.ini: version, modid, fileid, url."""
    out = {}
    f = folder / "meta.ini"
    if f.is_file():
        for line in f.read_text(errors="replace").splitlines():
            k, _, v = line.partition("=")
            out[k.strip().lower()] = v.strip()
    return out


def import_amethyst(inst: "Instance", folders: list[Path]) -> list[str]:
    from .instance import Mod
    added = []
    for src in folders:
        meta = _meta(src)
        base = safe_name(src.name)
        mid, n = base, 2
        while inst.get(mid) or (inst.mods_dir / mid).exists():
            mid, n = f"{base} ({n})", n + 1
        dest = inst.mods_dir / mid
        shutil.copytree(src, dest, ignore=shutil.ignore_patterns("meta.ini"))
        nid = meta.get("modid", "")
        fid = meta.get("fileid", "")
        inst.mods.append(Mod(
            id=mid, name=src.name, version=meta.get("version", ""), enabled=True,
            nexus_mod_id=int(nid) if nid.isdigit() and int(nid) > 0 else None,
            nexus_file_id=int(fid) if fid.isdigit() and int(fid) > 0 else None,
            author=meta.get("author", ""), category=meta.get("categoryname", ""),
            summary=meta.get("description", ""), source_url=meta.get("nexusurl", ""),
            installed_at=int(time.time()), notes="Imported from Amethyst Mod Manager."))
        added.append(mid)
    inst.save()
    return added
