"""Faster Skyrim start-up under Proton.

Skyrim reads an optional settings file for every plugin, Data\\<plugin>.ini, once
per setting (about 2,000 times each). Most plugins don't have one. On Windows a
missing file is found missing instantly; under Wine every look-up of a missing
file reads through the whole Data folder comparing names without regard to case.
With a few hundred plugins that's hundreds of thousands of folder scans, all
before the game opens its window: minutes of start-up (3 min 23 s measured with
414 plugins, 1 min 51 s with this).

So Kiln deploys an empty <plugin>.ini for every plugin that has none. An empty
settings file changes nothing in game. A real one from a mod always wins, and
Kiln removes its empty ones when the plugin goes, and on purge.
"""
from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .instance import Instance

GAMES = {"skyrimse", "skyrim"}
PLUGIN_EXTS = (".esp", ".esm", ".esl")


def applies(inst: "Instance") -> bool:
    return inst.game.id in GAMES


def empty_plugin_inis(inst: "Instance", manifest: dict) -> tuple[int, int]:
    """Bring the empty INIs in line with the plugins in Data. Updates
    manifest['generated']; returns (created, removed)."""
    root = inst.deploy_dir
    had = set(manifest.get("generated", []))
    if not root.is_dir():
        manifest["generated"] = []
        return 0, 0
    entries = {c.name.lower(): c for c in root.iterdir()}
    want = {}
    for low, c in entries.items():
        if low.endswith(PLUGIN_EXTS) and c.is_file():
            ini = c.name[:-4] + ".ini"
            want[ini.lower()] = ini
    created = removed = 0
    keep = []
    # drop ours that aren't needed any more (plugin gone), if still empty
    for name in had:
        p = root / name
        if name.lower() in want and p.is_file():
            continue
        if p.is_file() and p.stat().st_size == 0:
            p.unlink()
            removed += 1
        entries.pop(name.lower(), None)
    for low, ini in want.items():
        ours = next((n for n in had if n.lower() == low), None)
        cur = entries.get(low)
        if cur is not None:
            # a real settings file (from a mod or the user) wins; an empty one we didn't
            # make (an earlier test, or a lost manifest) is adopted
            if ours or (cur.is_file() and cur.stat().st_size == 0 and not cur.stat().st_nlink > 1):
                keep.append(cur.name)
            continue
        (root / ini).write_bytes(b"")
        keep.append(ini)
        created += 1
    manifest["generated"] = sorted(keep)
    return created, removed


def remove_generated(inst: "Instance", manifest: dict) -> int:
    """Purge: delete the empty INIs Kiln made."""
    n = 0
    for name in manifest.get("generated", []):
        p = inst.deploy_dir / name
        if p.is_file() and p.stat().st_size == 0:
            p.unlink()
            n += 1
    manifest["generated"] = []
    return n
