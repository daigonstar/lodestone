"""Stardew Valley mod dependencies, read from each mod's manifest.json the way
SMAPI does, and mapped to Nexus so missing ones can be installed.

Mapping a SMAPI UniqueID to a Nexus mod uses, in order:
  1. SMAPI's own web API (smapi.io), which knows the Nexus page of most mods;
  2. another missing ID by the same author that did resolve (content-pack parts
     like "Rafseazz.RSVCP" ship inside the main download, here Ridgeside Village);
  3. a Nexus search on the name hidden in the ID ("StardewValleyExpandedCP" →
     "Stardew Valley Expanded").
If one of your installed collections includes that Nexus mod, its file version is used.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .instance import Instance


def read_manifest(path: Path) -> dict:
    """manifest.json allows comments and trailing commas, like SMAPI."""
    text = path.read_text(encoding="utf-8-sig", errors="replace")
    text = re.sub(r'("(?:\\.|[^"\\])*")|/\*.*?\*/|//[^\n]*', lambda m: m.group(1) or "", text, flags=re.S)
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    try:
        d = json.loads(text)
    except ValueError:
        return {}
    return {k.lower(): v for k, v in d.items()} if isinstance(d, dict) else {}


@dataclass
class Missing:
    unique_id: str
    needed_by: list[str] = field(default_factory=list)   # manifest names
    min_version: str = ""
    # filled in by resolve():
    nexus_id: int = 0
    name: str = ""
    how: str = ""            # how it was matched, shown to the user
    via: str = ""            # UniqueID of the dependency that brings this one along


def scan(mods_dir: Path) -> tuple[dict[str, str], list[Missing]]:
    """Read every manifest under Mods/. Returns ({uid: name} installed, [missing required])."""
    provided: dict[str, str] = {}
    needs: dict[str, Missing] = {}
    manifests = []
    for dirpath, dirs, files in os.walk(mods_dir):
        dirs[:] = [d for d in dirs if not d.startswith(".")]      # SMAPI ignores dot-folders
        if "manifest.json" in files:
            m = read_manifest(Path(dirpath) / "manifest.json")
            if m.get("uniqueid"):
                manifests.append(m)
                provided[str(m["uniqueid"]).lower()] = m.get("name") or m["uniqueid"]
            dirs[:] = []                                             # a mod's subfolders aren't mods
    for m in manifests:
        name = m.get("name") or m["uniqueid"]
        deps = []
        for d in m.get("dependencies") or []:
            if isinstance(d, dict):
                d = {k.lower(): v for k, v in d.items()}
                if d.get("isrequired", True) is not False and d.get("uniqueid"):
                    deps.append((d["uniqueid"], d.get("minimumversion") or ""))
        cpf = m.get("contentpackfor")
        if isinstance(cpf, dict):
            cpf = {k.lower(): v for k, v in cpf.items()}
            if cpf.get("uniqueid"):
                deps.append((cpf["uniqueid"], cpf.get("minimumversion") or ""))
        for uid, minv in deps:
            if str(uid).lower() in provided:
                continue
            e = needs.setdefault(str(uid).lower(), Missing(str(uid)))
            if name not in e.needed_by:
                e.needed_by.append(name)
            e.min_version = e.min_version or str(minv)
    return provided, sorted(needs.values(), key=lambda e: (-len(e.needed_by), e.unique_id.lower()))


def _words(uid: str) -> str:
    """'FlashShifter.StardewValleyExpandedCP' → 'Stardew Valley Expanded'."""
    tail = uid.split(".", 1)[-1]
    tail = re.sub(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])|[_\-.]+", " ", tail)
    tail = re.sub(r"\b(CP|C#|CS|FTM|JA|AT|DGA|Core|Code|Content|Pack)\b", " ", tail)
    return " ".join(tail.split())


def smapi_lookup(uids: list[str]) -> dict[str, tuple[int, str]]:
    """{uid_lower: (nexus id, name)} from SMAPI's web API."""
    import requests
    if not uids:
        return {}
    try:
        r = requests.post("https://smapi.io/api/v3.0/mods", timeout=25,
                          headers={"User-Agent": "Kiln", "Content-Type": "application/json"},
                          json={"mods": [{"id": u} for u in uids], "apiVersion": "4.0.0",
                                "gameVersion": "1.6", "platform": "Linux",
                                "includeExtendedMetadata": True})
        r.raise_for_status()
        out = {}
        for m in r.json():
            md = m.get("metadata") or {}
            if md.get("nexusID"):
                out[m["id"].lower()] = (int(md["nexusID"]), md.get("name") or m["id"])
        return out
    except Exception:  # noqa: BLE001  (offline: fall back to the other ways)
        return {}


def resolve(missing: list[Missing], domain: str = "stardewvalley") -> list[Missing]:
    from . import nexus
    found = smapi_lookup([m.unique_id for m in missing])
    for m in missing:
        hit = found.get(m.unique_id.lower())
        if hit:
            m.nexus_id, m.name, m.how = hit[0], hit[1], "SMAPI's mod database"
    # parts that come with a resolved main mod by the same author
    by_author = {}
    for m in missing:
        if m.nexus_id:
            by_author.setdefault(m.unique_id.split(".")[0].lower(), m)
    for m in missing:
        if not m.nexus_id:
            main = by_author.get(m.unique_id.split(".")[0].lower())
            if main:
                m.via, m.name, m.how = main.unique_id, main.name, f"part of {main.name}"
    # name search
    for m in missing:
        if m.nexus_id or m.via:
            continue
        q = _words(m.unique_id)
        if not q:
            continue
        try:
            res, _total = nexus.search(domain, q, "endorsements", 1, 5)
        except Exception:  # noqa: BLE001
            continue
        qw = set(q.lower().split())
        best = next((r for r in res if qw <= set(re.sub(r"[^a-z0-9 ]", " ", r.name.lower()).split())), None)
        if best:
            m.nexus_id, m.name, m.how = best.mod_id, best.name, f"Nexus search for “{q}”"
    return missing


def collection_files(inst: "Instance") -> dict[int, int]:
    """{nexus mod id: main file id} from the collections installed for this game."""
    from . import collections as nc
    out: dict[int, int] = {}
    for c in inst.collections:
        try:
            r = nc.revision(inst.game.nexus_domain, c["slug"], c.get("revision"))
        except Exception:  # noqa: BLE001
            continue
        for m in r.mods:
            if m.category == "MAIN":
                out.setdefault(m.mod_id, m.file_id)
    return out


def check(inst: "Instance") -> list[Missing]:
    """Missing required dependencies of the deployed Stardew mods (no network)."""
    if inst.game.id != "stardewvalley":
        return []
    return scan(inst.deploy_dir)[1]
