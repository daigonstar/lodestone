"""Automatic load order, following common modding practice (and the ideas LOOT uses).

Mod order decides who wins file conflicts (lower in the list wins):
  frameworks → fixes → UI → broad texture/mesh overhauls → specific visuals →
  environment → gameplay → characters → quests/locations → animation/audio →
  presets (ENB etc.) → compatibility patches → your own tweaks.
Within texture/mesh tiers, bigger overhauls go first so targeted retextures win.

Hard rules always win over the tiers:
  - a mod whose plugins need another mod's plugins (masters) loads after it;
  - rules you made ("load after X") are kept.

Plugin order: masters (ESM) first, then normal plugins; every plugin after its
masters; unofficial patches early and compatibility patches late; otherwise the
current order is kept.
"""
from __future__ import annotations

from pathlib import Path

import heapq
import re
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .instance import Instance, Mod

TIERS = [
    # (tier, label, category keywords, name keywords)
    (0, "Framework / library", ("framework", "librar", "modders resources", "resources", "skse",
                                "f4se", "script extender", "utilit", "dependenc"),
     ("framework", "library", "address library", "skse", "f4se", "engine fixes", "powerofthree",
      "papyrus", "jcontainers", "ui extensions", "mcm helper", "red4ext", "cyber engine tweaks",
      "archivexl", "tweakxl", "codeware", "redscript", "smapi", "content patcher")),
    (1, "Bug fixes", ("bug fix", "patches - official", "unofficial"),
     ("unofficial", "bug fix", "bugfix", "ussep", "ufo4p", "scrambled bugs")),
    (2, "User interface", ("user interface", "interface", "ui", "hud", "menu"),
     ("skyui", "hud", " ui", "ui ", "menu", "interface", "map")),
    (3, "Texture & mesh overhaul", (), ("overhaul", "hd", "2k", "4k", "8k", "retexture",
                                         "smim", "texture pack", "noble", "skyland")),
    (4, "Models & textures", ("models and textures", "texture", "mesh", "visual", "graphics",
                               "models"), ("texture", "mesh", "retex", "hd ")),
    (5, "Environment", ("environment", "weather", "lighting", "landscape", "flora", "water",
                        "cities", "towns", "buildings", "visuals and graphics"),
     ("weather", "lighting", "flora", "grass", "tree", "water", "city", "cities", "landscape")),
    (6, "Gameplay", ("gameplay", "combat", "skills", "magic", "items", "weapons", "armour", "armor",
                     "clothing", "crafting", "economy", "overhauls", "immersion", "stealth",
                     "cheats", "vehicles", "perks"), ()),
    (7, "Characters & NPCs", ("character", "npc", "follower", "companion", "body", "hair", "face",
                              "races", "creatures", "presets - character", "player"), ()),
    (8, "Quests & locations", ("quest", "location", "new lands", "dungeons", "houses", "player homes",
                               "adventure"), ()),
    (9, "Animation & audio", ("animation", "audio", "sound", "music", "voice"), ()),
    (10, "Presets (ENB / ReShade)", ("enb", "reshade", "presets - enb", "post-processing"),
     ("enb", "reshade", "preset")),
    (11, "Compatibility patches", ("patches", "compatibility", "merged"),
     ("patch", "compatibility", "compat", "merged", "conflict resolution")),
    (12, "Your tweaks", (), ("tweak", "override", "output", "my ", "ini ")),
]
LABELS = {t: label for t, label, _, _ in TIERS}


def _words(s: str) -> str:
    return " " + re.sub(r"[^a-z0-9]+", " ", (s or "").lower()) + " "


def classify(mod: "Mod") -> int:
    cat, name = _words(mod.category), _words(mod.name)
    # patches and fixes are judged by name first; "Unofficial ... Patch" is a fix, not a compat patch
    if name.startswith(" unofficial ") or name.strip() in ("ussep", "ufo4p"):
        return 1
    if re.search(r" (patch|patches|compatibility|compat) ", name) and "official" not in cat:
        return 11          # e.g. "Immersive Armors - USSEP Patch"
    if re.search(r" unofficial | ussep | ufo4p ", name):
        return 1
    if not mod.nexus_mod_id and re.search(r" (tweak|tweaks|override|output|my) ", name):
        return 12
    for tier, _l, cat_kw, _n in TIERS:
        if tier in (3, 11, 12):
            continue
        if any(_words(k).strip() in cat for k in cat_kw if k):
            if tier == 4 and re.search(r" (overhaul|hd|2k|4k|8k) ", name):
                return 3
            return tier
    for tier, _l, _c, name_kw in TIERS:
        if any(_words(k) in name for k in name_kw):
            return tier
    return 6 if mod.nexus_mod_id else 12   # unknown Nexus mods sit with gameplay; local ones last


@dataclass
class SortResult:
    order: list[str]                     # mod ids
    tiers: dict[str, int]
    reasons: dict[str, str]
    cycles: list[str]

    def moved(self, before: list[str]) -> int:
        return sum(1 for a, b in zip(before, self.order) if a != b)


def _topo(ids: list[str], edges: dict[str, set[str]], key) -> tuple[list[str], list[str]]:
    """Kahn's algorithm; edges[a] = nodes that must come after a. Picks the
    lowest key among ready nodes. Cycles are broken at the lowest key."""
    indeg = {i: 0 for i in ids}
    for a, outs in edges.items():
        for b in outs:
            if b in indeg and a in indeg and a != b:
                indeg[b] += 1
    heap = [(key(i), i) for i in ids if indeg[i] == 0]
    heapq.heapify(heap)
    out, done, broken = [], set(), []
    while len(out) < len(ids):
        if not heap:   # cycle: release the best remaining node
            rest = [i for i in ids if i not in done]
            pick = min(rest, key=key)
            broken.append(pick)
            indeg[pick] = 0
            heap.append((key(pick), pick))
        _, i = heapq.heappop(heap)
        if i in done:
            continue
        done.add(i)
        out.append(i)
        for b in edges.get(i, ()):
            if b in indeg and b not in done and b != i:
                indeg[b] -= 1
                if indeg[b] == 0:
                    heapq.heappush(heap, (key(b), b))
    return out, broken


def plugins_by_mod(inst: "Instance") -> dict[str, list[str]]:
    from .games.bethesda import PLUGIN_EXTS
    out: dict[str, list[str]] = {}
    if not inst.game.has_plugins:
        return out
    for m in inst.mods:
        out[m.id] = [r for r in inst.mod_files(m) if "/" not in r and Path(r).suffix.lower() in PLUGIN_EXTS]
    return out


def sort_mods(inst: "Instance") -> SortResult:
    from .games.bethesda import read_plugin
    mods = inst.mods
    cur = {m.id: i for i, m in enumerate(mods)}
    tiers = {m.id: classify(m) for m in mods}
    reasons = {m.id: LABELS[tiers[m.id]] for m in mods}
    size: dict[str, int] = {}
    for m in mods:
        if tiers[m.id] in (3, 4):
            size[m.id] = len(inst.mod_files(m))
    edges: dict[str, set[str]] = {}

    # plugin masters → the providing mod loads first
    owner: dict[str, str] = {}
    pmap = plugins_by_mod(inst)
    for mid, names in pmap.items():
        for n in names:
            owner.setdefault(n.lower(), mid)
    for mid, names in pmap.items():
        for n in names:
            for master in read_plugin(inst.mod_path(inst.get(mid)) / n).masters:
                src = owner.get(master.lower())
                if src and src != mid:
                    edges.setdefault(src, set()).add(mid)
                    reasons[mid] = f"{LABELS[tiers[mid]]}; needs {master}"
    # your own rules
    for m in mods:
        for r in m.rules or []:
            other = r.get("mod")
            if other not in cur:
                continue
            if r.get("type") == "after":
                edges.setdefault(other, set()).add(m.id)
            else:
                edges.setdefault(m.id, set()).add(other)
            reasons[m.id] = f"{LABELS[tiers[m.id]]}; your rule: {r['type']} {inst.get(other).name}"

    key = lambda i: (tiers[i], -size.get(i, 0), cur[i])
    order, broken = _topo([m.id for m in mods], edges, key)
    return SortResult(order, tiers, reasons, broken)


def sort_plugins(inst: "Instance") -> list[dict]:
    """New plugin list (same entries, same enabled state) in a sensible order."""
    from .games.bethesda import read_plugin
    plugins = inst.plugins
    names = [p["name"] for p in plugins]
    low = {n.lower(): n for n in names}
    cur = {n: i for i, n in enumerate(names)}
    info = {n: read_plugin(inst.deploy_dir / n) for n in names}
    mod_of: dict[str, str] = {}
    for mid, ps in plugins_by_mod(inst).items():
        for n in ps:
            mod_of.setdefault(n.lower(), mid)
    tier = {}
    for n in names:
        m = inst.get(mod_of.get(n.lower(), ""))
        t = classify(m) if m else 6
        w = _words(n)
        if w.startswith(" unofficial "):
            t = 1
        elif re.search(r" (patch|compat|compatibility) ", w):
            t = 11
        tier[n] = t
    edges: dict[str, set[str]] = {}
    for n in names:
        for master in info[n].masters:
            src = low.get(master.lower())
            if src and src != n:
                edges.setdefault(src, set()).add(n)
    key = lambda n: (0 if info[n].is_master else 1, tier[n], cur[n])
    order, _ = _topo(names, edges, key)
    by = {p["name"]: p for p in plugins}
    return [by[n] for n in order]
