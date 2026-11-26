"""Nexus requirements: the “Requirements” list on each mod's page (e.g. FormList
Manipulator → powerofthree's Tweaks), checked against your profile.

Those lists are noisy, so only real gaps are reported:
  * entries marked optional, VR-only, “only if you use …” are skipped;
  * a modern replacement counts (Pandora for Nemesis/FNIS, Open Animation
    Replacer for Dynamic Animation Replacer);
  * only frameworks and libraries count: a mod with SKSE/F4SE plugins or scripts,
    or one Nexus files under Modders Resources / Utilities / Frameworks. Patches
    listing the mods they patch are left to the missing-masters check.
Requirements are cached for a week (cache_dir/requirements-<game>.json).
"""
from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from .paths import cache_dir

if TYPE_CHECKING:
    from .instance import Instance, Mod

WEEK = 7 * 86400
BATCH = 25
OPTIONAL = re.compile(r"\boptional\b|\bonly required\b|\brequired (?:only )?if\b|\bneeded (?:only )?if\b|\bonly (?:if|when|for|needed)\b|\bif you (?:use|have|want|are)\b|\bfor (?:the )?vr\b|"
                      r"\bvr only\b|\bnot required\b|\brecommended\b|\bsoft\b|\bfor (?:the )?(?:optional )?patch\b|"
                      r"\bfor compatib|\bone of\b|\beither\b|\balternative", re.I)
FRAMEWORK_CATS = {"modders resources", "utilities", "frameworks", "modders resources and tutorials",
                  "modding tools", "skse plugins"}
# required mod id -> words in the name of a mod that does its job
EQUIVALENT = {
    60033: ("pandora", "nemesis"),                   # Nemesis
    3038: ("pandora", "nemesis", "fnis"),            # FNIS
    33746: ("open animation replacer", "dynamic animation replacer"),   # DAR
}


@dataclass
class Missing:
    nexus_id: int
    name: str
    needed_by: list = field(default_factory=list)    # [Mod]
    notes: list = field(default_factory=list)
    have: "Mod | None" = None                        # downloaded but not active in this profile


def _cache_file(domain: str):
    return cache_dir() / f"requirements-{domain}.json"


def _load(domain):
    try:
        return json.loads(_cache_file(domain).read_text())
    except (OSError, ValueError):
        return {}


def fetch(domain: str, game_id: int, mod_ids: list[int], network: bool = True) -> dict[str, dict]:
    """{mod id: {"at": time, "reqs": [...], "cat": category}} for these mods, from the
    cache, fetching what's missing or stale (25 mods per request)."""
    from . import nexus
    cache = _load(domain)
    now = time.time()
    todo = [i for i in mod_ids if now - cache.get(str(i), {}).get("at", 0) > WEEK]
    if todo and network and not __import__("os").environ.get("KILN_OFFLINE"):
        q = ("query($ids:[CompositeDomainWithIdInput!]!, $n:Int){ legacyModsByDomain(ids:$ids, count:$n){ nodes{"
             " modId modCategory { name } modRequirements{ nexusRequirements(count:15){ nodes{"
             " modId modName externalRequirement notes gameId } } } } } }")
        for i in range(0, len(todo), BATCH):
            part = todo[i:i + BATCH]
            try:
                d = nexus._gql(q, {"ids": [{"gameDomain": domain, "modId": x} for x in part], "n": len(part)})
            except Exception:  # noqa: BLE001  (offline: use what's cached)
                break
            for n in (d.get("legacyModsByDomain") or {}).get("nodes") or []:
                reqs = [{"id": int(r["modId"]), "name": r.get("modName") or "", "notes": r.get("notes") or "",
                         "external": bool(r.get("externalRequirement")), "game": str(r.get("gameId") or "")}
                        for r in ((n.get("modRequirements") or {}).get("nexusRequirements") or {}).get("nodes") or []
                        if r.get("modId")]
                cache[str(n["modId"])] = {"at": now, "reqs": reqs,
                                          "cat": ((n.get("modCategory") or {}).get("name") or "").lower()}
            for x in part:                               # remember mods Nexus didn't return (hidden/deleted)
                cache.setdefault(str(x), {"at": now, "reqs": [], "cat": ""})
        try:
            _cache_file(domain).parent.mkdir(parents=True, exist_ok=True)
            _cache_file(domain).write_text(json.dumps(cache))
        except OSError:
            pass
    return cache


def _is_framework(inst: "Instance", mod, category: str) -> bool:
    if category and category.lower() in FRAMEWORK_CATS:
        return True
    if mod is not None:
        return any(r.lower().endswith((".dll", ".pex")) for r in inst.mod_files(mod))
    return False


CONTENT_EXTS = (".dll", ".pex", ".esp", ".esm", ".esl", ".bsa", ".ba2", ".exe", ".nif", ".dds", ".hkx", ".swf",
                ".bin", ".csv", ".pak", ".lua", ".asi", ".json")


def _provides(inst: "Instance", mod) -> bool:
    """A mod that's more than settings (an “INI file” add-on doesn't make the mod installed)."""
    return any(r.lower().endswith(CONTENT_EXTS) for r in inst.mod_files(mod))


def _fits(inst, mod, rt) -> bool:
    if not rt or inst.game.id != "skyrimse":
        return True
    from . import pe, versionmatch
    return all(pe.xse_plugin(f).verdict(rt, None, True)[0] in ("ok", "addrlib")
               for f in versionmatch._staged_dlls(inst, mod))


def _edition_note(notes: str, rt) -> bool | None:
    """Notes that are just “SE”/“AE”: True if they fit the installed game version."""
    n = notes.strip().upper().strip(".")
    if n not in ("SE", "AE") or not rt:
        return None
    return (n == "AE") == (tuple(rt) >= (1, 6, 0))


def check(inst: "Instance", network: bool = True) -> list[Missing]:
    g = inst.game
    if not g.nexus_domain or not g.nexus_game_id:
        return []
    enabled = [m for m in inst.mods if m.enabled and m.nexus_mod_id]
    if not enabled:
        return []
    active_ids = {m.nexus_mod_id for m in enabled if _provides(inst, m)}
    from .essentials import game_runtime
    rt = game_runtime(inst)
    active_names = " ".join(m.name.lower() for m in inst.mods if m.enabled)
    by_nexus: dict[int, list] = {}
    for m in inst.mods:
        if m.nexus_mod_id:
            by_nexus.setdefault(m.nexus_mod_id, []).append(m)
    vr = "vr" in g.id
    data = fetch(g.nexus_domain, g.nexus_game_id, sorted({m.nexus_mod_id for m in enabled}), network)
    found: dict[int, Missing] = {}
    for m in enabled:
        for r in (data.get(str(m.nexus_mod_id)) or {}).get("reqs", []):
            rid = r["id"]
            if r["external"] or rid in active_ids or (r["game"] and r["game"] != str(g.nexus_game_id)):
                continue
            if not vr and re.search(r"\bvr\b|skyrimvr|sksevr", r["name"].lower()):
                continue
            if OPTIONAL.search(r["notes"]) or _edition_note(r["notes"], rt) is False:
                continue
            base = re.sub(r"\s*[-(].*$", "", r["name"]).strip().lower()
            if len(base) >= 5 and any(base in m2.name.lower() for m2 in enabled if m2.nexus_mod_id != rid
                                      and _provides(inst, m2)):
                continue                     # e.g. “Mfg Fix NG” does the job of “Mfg Fix”
            if rid in EQUIVALENT and any(w in active_names for w in EQUIVALENT[rid]):
                continue
            e = found.setdefault(rid, Missing(rid, r["name"]))
            if m not in e.needed_by:
                e.needed_by.append(m)
            if r["notes"] and r["notes"] not in e.notes:
                e.notes.append(r["notes"])
    if not found:
        return []
    # categories of the missing ones, and whether you already have them
    cats = fetch(g.nexus_domain, g.nexus_game_id, sorted(found), network)
    out = []
    for rid, e in found.items():
        have = [h for h in by_nexus.get(rid, []) if _provides(inst, h)] or by_nexus.get(rid, [])
        # the copy that works on this game version first, then the newest
        e.have = max(have, key=lambda x: (_fits(inst, x, rt), x.nexus_file_id or 0)) if have else None
        if _is_framework(inst, e.have, (cats.get(str(rid)) or {}).get("cat", "")):
            out.append(e)
    return sorted(out, key=lambda e: (-len(e.needed_by), e.name.lower()))
