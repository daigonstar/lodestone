"""Nexus Collections: search, revision details (public GraphQL), and the
collection archive (collection.json + bundled files; needs the API key).

collection.json is Vortex's format. The fields used here:
  mods[]: name, version, optional, source{type: nexus|bundle|direct|browse,
          modId, fileId, md5, logicalFilename, fileExpression, url, instructions},
          choices{type: fomod, options[{name, groups[{name, choices[{name, idx}]}]}]},
          instructions
  modRules[]: {type: before|after|requires|conflicts, source{...}, reference{...}}
  plugins[] / loadOrder[]: {name, enabled}
"""
from __future__ import annotations

import json
import re
import shutil
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

from . import archive, nexus
from .nexus import NexusError, _gql, session

SORTS = [
    ("endorsements", "Most endorsed"),
    ("downloads", "Most downloaded"),
    ("rating", "Highest rated"),
    ("updatedAt", "Recently updated"),
    ("createdAt", "Newest"),
]


@dataclass
class CollectionSummary:
    slug: str
    name: str
    summary: str
    author: str
    downloads: int
    endorsements: int
    thumb: str
    revision: int
    mod_count: int
    size: int
    adult: bool
    game_versions: list[str] = field(default_factory=list)   # what the curator built it for


@dataclass
class CollectionMod:
    mod_id: int
    file_id: int
    mod_name: str
    file_name: str
    version: str
    optional: bool
    size_kb: int
    category: str
    adult: bool = False


@dataclass
class CollectionRevision:
    slug: str
    domain: str
    name: str
    author: str
    revision: int
    description_md: str
    summary: str
    download_path: str
    mods: list[CollectionMod] = field(default_factory=list)
    bundled_count: int = 0
    total_size: int = 0
    game_versions: list[str] = field(default_factory=list)

    @property
    def page(self) -> str:
        return f"{nexus.SITE}/games/{self.domain}/collections/{self.slug}"


def categories(game_id: int) -> list[str]:
    """Collection categories (Nexus uses the same few for every game)."""
    d = _gql("query($g: Int) { categories(gameId: $g) { name approved } }", {"g": game_id})
    return sorted({c["name"] for c in d.get("categories") or [] if c.get("approved", True)},
                  key=str.lower)


def game_versions(domain: str) -> list[tuple[str, int]]:
    """[(game version, number of collections built for it)], newest version first."""
    d = _gql("query($f: CollectionsSearchFilter) { collectionsV2(filter: $f, count: 1,"
             " facets: {gameVersion: []}) { facetsData } }",
             {"f": {"gameDomain": [{"value": domain, "op": "EQUALS"}]}})
    raw = ((d.get("collectionsV2") or {}).get("facetsData") or {}).get("gameVersion") or {}

    def key(v):
        nums = re.findall(r"\d+", v)
        return [int(n) for n in nums[:4]] if re.match(r"^\d+(\.\d+)+", v) else [-1]
    return sorted(((v, int(n)) for v, n in raw.items() if re.match(r"^\d+(\.\d+)+", v) and v != "0.0.0"),
                  key=lambda vn: key(vn[0]), reverse=True)


def search(domain: str, text: str = "", sort: str = "endorsements", page: int = 1,
           per_page: int = 20, adult: bool = False, category: str = "",
           game_version: str = "") -> tuple[list[CollectionSummary], int]:
    flt: dict = {"gameDomain": [{"value": domain, "op": "EQUALS"}]}
    if category:
        flt["categoryName"] = [{"value": category, "op": "EQUALS"}]
    if game_version:
        flt["gameVersion"] = [{"value": game_version, "op": "EQUALS"}]
    if text.strip():
        flt["generalSearch"] = [{"value": text.strip(), "op": "WILDCARD"}]
    if not adult:
        flt["adultContent"] = [{"value": False, "op": "EQUALS"}]
    q = ("query($f: CollectionsSearchFilter, $s: [CollectionsSearchSort!], $n: Int, $o: Int) {"
         " collectionsV2(filter: $f, sort: $s, count: $n, offset: $o) { totalCount nodes {"
         " slug name summary user { name } totalDownloads endorsements"
         " tileImage { thumbnailUrl(size: small) }"
         " latestPublishedRevision { revisionNumber modCount totalSize adultContent"
         " gameVersions { reference } } } } }")
    d = _gql(q, {"f": flt, "s": [{sort: {"direction": "DESC"}}], "n": per_page,
                 "o": (page - 1) * per_page})["collectionsV2"]
    out = []
    for n in d["nodes"]:
        rev = n.get("latestPublishedRevision") or {}
        if not rev:
            continue
        out.append(CollectionSummary(
            slug=n["slug"], name=n.get("name") or "?", summary=n.get("summary") or "",
            author=(n.get("user") or {}).get("name", ""), downloads=int(n.get("totalDownloads") or 0),
            endorsements=int(n.get("endorsements") or 0),
            thumb=(n.get("tileImage") or {}).get("thumbnailUrl") or "",
            revision=int(rev.get("revisionNumber") or 0), mod_count=int(rev.get("modCount") or 0),
            size=int(rev.get("totalSize") or 0), adult=bool(rev.get("adultContent")),
            game_versions=[g.get("reference") for g in rev.get("gameVersions") or [] if g.get("reference")]))
    return out, int(d["totalCount"])


_REV_FIELDS = ("revisionNumber downloadLink modCount totalSize gameVersions { reference }"
               " externalResources { resourceType }"
               " modFiles { fileId optional file { modId name version size category"
               " mod { name modId adultContent } } }")


def revision(domain: str, slug: str, rev: int | None = None) -> CollectionRevision:
    if rev is None:
        q = ("query($s: String, $d: String) { collection(slug: $s, domainName: $d, viewAdultContent: true) {"
             " name summary description user { name } latestPublishedRevision { %s } } }" % _REV_FIELDS)
        c = _gql(q, {"s": slug, "d": domain}).get("collection")
        if not c or not c.get("latestPublishedRevision"):
            raise NexusError(f"Collection {slug} not found or has no published revision.")
        r = c["latestPublishedRevision"]
    else:
        q = ("query($s: String, $r: Int, $d: String) { collectionRevision(slug: $s, revision: $r,"
             " domainName: $d, viewAdultContent: true) { %s collection { name summary description"
             " user { name } } } }" % _REV_FIELDS)
        r = _gql(q, {"s": slug, "r": rev, "d": domain}).get("collectionRevision")
        if not r:
            raise NexusError(f"Revision {rev} of collection {slug} not found.")
        c = r["collection"]
    mods = []
    for mf in r.get("modFiles") or []:
        f = mf.get("file") or {}
        m = f.get("mod") or {}
        if not f:
            continue
        mods.append(CollectionMod(
            mod_id=int(f.get("modId") or m.get("modId")), file_id=int(mf["fileId"]),
            mod_name=m.get("name") or f.get("name") or "?", file_name=f.get("name") or "",
            version=f.get("version") or "", optional=bool(mf.get("optional")),
            size_kb=int(f.get("size") or 0), category=f.get("category") or "",
            adult=bool(m.get("adultContent"))))
    return CollectionRevision(
        slug=slug, domain=domain, name=c.get("name") or slug, author=(c.get("user") or {}).get("name", ""),
        revision=int(r["revisionNumber"]), description_md=c.get("description") or "",
        summary=c.get("summary") or "", download_path=r.get("downloadLink") or "",
        mods=mods, total_size=int(r.get("totalSize") or 0),
        bundled_count=sum(1 for e in r.get("externalResources") or [] if e.get("resourceType") == "bundle"),
        game_versions=[g.get("reference") for g in r.get("gameVersions") or [] if g.get("reference")])


# ------------------------------------------------------------------ manifest
@dataclass
class ManifestMod:
    name: str
    source_type: str
    mod_id: int | None
    file_id: int | None
    optional: bool
    md5: str = ""
    logical_name: str = ""
    file_expression: str = ""
    url: str = ""
    instructions: str = ""
    choices: list | None = None       # FOMOD options
    raw: dict = field(default_factory=dict)


@dataclass
class Manifest:
    root: Path                          # extracted collection archive
    info: dict
    mods: list[ManifestMod]
    rules: list[dict]
    plugins: list[tuple[str, bool]]

    def for_file(self, mod_id: int, file_id: int) -> ManifestMod | None:
        return next((m for m in self.mods if m.mod_id == mod_id and m.file_id == file_id), None) or \
            next((m for m in self.mods if m.mod_id == mod_id), None)

    def bundled(self) -> list[ManifestMod]:
        return [m for m in self.mods if m.source_type == "bundle"]

    def external(self) -> list[ManifestMod]:
        return [m for m in self.mods if m.source_type in ("direct", "browse", "manual")]

    def bundled_path(self, m: ManifestMod) -> Path | None:
        b = archive.ci_child(self.root, "bundled")
        if b is None:
            return None
        name = m.file_expression or m.logical_name or m.name
        p = archive.ci_child(b, name)
        if p is None:  # fall back to any entry starting with the name
            p = next((c for c in b.iterdir() if c.name.lower().startswith(name.lower())), None)
        return p

    def cleanup(self) -> None:
        shutil.rmtree(self.root.parent, ignore_errors=True)


def manifest_download_url(api_key: str, download_path: str) -> str:
    if not api_key:
        raise NexusError("A Nexus API key is needed to download collections. Add it in Settings.")
    url = download_path if download_path.startswith("http") else f"https://api.nexusmods.com{download_path}"
    r = session.get(url, headers={"apikey": api_key}, timeout=25)
    if r.status_code == 401:
        raise NexusError("Nexus rejected the API key (401).")
    if r.status_code >= 400:
        raise NexusError(f"Nexus returned HTTP {r.status_code} for the collection download link.")
    d = r.json()
    links = d.get("download_links") if isinstance(d, dict) else d
    uris = [l.get("URI") for l in links or [] if isinstance(l, dict) and l.get("URI")]
    if not uris:
        raise NexusError("Nexus returned no download link for this collection.")
    return uris[0]


def load_manifest(archive_path: Path, work_dir: Path) -> Manifest:
    tmp = Path(tempfile.mkdtemp(prefix=".collection-", dir=work_dir))
    root = tmp / "x"
    archive.extract(archive_path, root)
    cj = archive.ci_child(root, "collection.json")
    if cj is None:
        shutil.rmtree(tmp, ignore_errors=True)
        raise NexusError("The collection archive has no collection.json.")
    return parse_manifest(json.loads(cj.read_text(encoding="utf-8-sig")), root)


def parse_manifest(data: dict, root: Path) -> Manifest:
    mods = []
    for m in data.get("mods") or []:
        src = m.get("source") or {}
        ch = m.get("choices") or {}
        mods.append(ManifestMod(
            name=m.get("name") or src.get("logicalFilename") or "?",
            source_type=src.get("type", "nexus"),
            mod_id=int(src["modId"]) if str(src.get("modId", "")).isdigit() else None,
            file_id=int(src["fileId"]) if str(src.get("fileId", "")).isdigit() else None,
            optional=bool(m.get("optional")), md5=src.get("md5") or "",
            logical_name=src.get("logicalFilename") or "", file_expression=src.get("fileExpression") or "",
            url=src.get("url") or "", instructions=m.get("instructions") or src.get("instructions") or "",
            choices=ch.get("options") if ch.get("type") == "fomod" else None, raw=m))
    plugins = []
    for p in data.get("plugins") or data.get("loadOrder") or []:
        if isinstance(p, dict) and (p.get("name") or p.get("id")):
            plugins.append((p.get("name") or p.get("id"), p.get("enabled", True) is not False))
        elif isinstance(p, str):
            plugins.append((p, True))
    return Manifest(root=root, info=data.get("info") or {}, mods=mods,
                    rules=data.get("modRules") or [], plugins=plugins)


def order_by_rules(manifest: Manifest, ids: dict[int, str], current: list[str]) -> list[str]:
    """Reorder the collection's mods (given as {nexus_mod_id: kiln mod id}) inside
    `current` to satisfy before/after rules; other mods keep their places."""
    def ref_to_id(ref: dict) -> str | None:
        for m in manifest.mods:
            if m.mod_id is None or m.mod_id not in ids:
                continue
            if (ref.get("fileMD5") and ref["fileMD5"] == m.md5) or \
               (ref.get("logicalFileName") and ref["logicalFileName"] == m.logical_name) or \
               (ref.get("fileExpression") and ref["fileExpression"] == m.file_expression) or \
               (ref.get("id") and str(ref["id"]) == str(m.mod_id)):
                return ids[m.mod_id]
        return None

    after: dict[str, set[str]] = {}   # x -> set of mods that must come before x
    for r in manifest.rules:
        a, b = ref_to_id(r.get("source") or {}), ref_to_id(r.get("reference") or {})
        if not a or not b or a == b:
            continue
        if r.get("type") == "before":
            after.setdefault(b, set()).add(a)
        elif r.get("type") == "after":
            after.setdefault(a, set()).add(b)
    members = [m for m in current if m in set(ids.values())]
    out: list[str] = []

    def visit(x, stack=()):
        if x in out or x in stack:
            return
        for y in sorted(after.get(x, ()), key=members.index):
            visit(y, stack + (x,))
        out.append(x)

    for x in members:
        visit(x)
    slots = iter(out)
    return [next(slots) if m in set(members) else m for m in current]
