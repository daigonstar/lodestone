"""Nexus Mods API client.

Browsing and file lists use the public GraphQL API (v2). Downloads use the v1
REST API with the user's personal API key: Premium users can request a link
directly; free users click "Mod Manager Download" on the site, which sends an
nxm:// link carrying a one-time key that v1 accepts.
"""
from __future__ import annotations

import html
import re
import threading
import time
from dataclasses import dataclass, field
from urllib.parse import parse_qs, urlparse

import requests

from . import APP_NAME, __version__

V1 = "https://api.nexusmods.com/v1"
V2 = "https://api.nexusmods.com/v2/graphql"
SITE = "https://www.nexusmods.com"

session = requests.Session()
session.headers.update({
    "User-Agent": f"{APP_NAME}/{__version__} (Linux; Python requests)",
    "Application-Name": APP_NAME,
    "Application-Version": __version__,
})

SORTS = [
    ("endorsements", "Most endorsed"),
    ("downloads", "Most downloaded"),
    ("updatedAt", "Recently updated"),
    ("createdAt", "Newest"),
]


class NexusError(RuntimeError):
    pass


# ------------------------------------------------------------------ GraphQL
SPACING = 0.25              # seconds between requests (all threads together)
WAITS = (2, 4, 8, 16, 30)   # back-off when Nexus says "too many requests" and doesn't say how long
MAX_WAIT = 60               # never wait longer than this for one request
SLOW_DOWN = ("Nexus is getting too many requests from Kiln right now and asked it to slow down. "
             "Try again in a minute or two.")
_lock = threading.Lock()
_next_at = 0.0
sleep = time.sleep          # tests replace this


def _pace() -> None:
    """Space requests out, across every thread."""
    global _next_at
    with _lock:
        now = time.monotonic()
        wait = _next_at - now
        _next_at = max(now, _next_at) + SPACING
    if wait > 0 and threading.current_thread() is not threading.main_thread():
        sleep(wait)                     # (never pause the window)


def _retry_after(r) -> float | None:
    try:
        return float(r.headers.get("Retry-After", ""))
    except (TypeError, ValueError):
        return None


def _gql(query: str, variables: dict | None = None) -> dict:
    """One GraphQL request. Paced, and retried with back-off when Nexus rate-limits
    (never on the window's own thread: it would freeze)."""
    background = threading.current_thread() is not threading.main_thread()
    waited = 0.0
    for attempt in range(len(WAITS) + 1):
        _pace()
        try:
            r = session.post(V2, json={"query": query, "variables": variables or {}}, timeout=25)
        except requests.RequestException as e:
            raise NexusError(f"Network error: {e}") from e
        if r.status_code == 429 or r.status_code in (502, 503, 504):
            wait = _retry_after(r) or WAITS[min(attempt, len(WAITS) - 1)]
            if not background or attempt == len(WAITS) or waited + wait > MAX_WAIT:
                raise NexusError(SLOW_DOWN if r.status_code == 429 else
                                 f"Nexus is having trouble right now (HTTP {r.status_code}). Try again shortly.")
            waited += wait
            sleep(wait)
            continue
        try:
            data = r.json()
        except ValueError:
            raise NexusError(f"Nexus returned HTTP {r.status_code}")
        if data.get("errors"):
            raise NexusError("; ".join(e.get("message", "?") for e in data["errors"]))
        return data.get("data") or {}
    raise NexusError(SLOW_DOWN)


# answers that rarely change, remembered for a while (several checks ask the same thing)
CACHE_SECONDS = 600
_cache: dict[tuple, tuple[float, object]] = {}


def _cached(key: tuple, fetch):
    hit = _cache.get(key)
    if hit and time.monotonic() - hit[0] < CACHE_SECONDS:
        return hit[1]
    val = fetch()
    _cache[key] = (time.monotonic(), val)
    return val


def forget_cache() -> None:
    _cache.clear()


@dataclass
class ModSummary:
    mod_id: int
    name: str
    summary: str = ""
    author: str = ""
    version: str = ""
    downloads: int = 0
    endorsements: int = 0
    updated: str = ""
    thumb: str = ""
    picture: str = ""
    category: str = ""
    adult: bool = False


@dataclass
class NexusFile:
    file_id: int
    name: str
    version: str
    category: str      # MAIN, OPTIONAL, UPDATE, MISCELLANEOUS, OLD_VERSION, ARCHIVED, ...
    date: int
    size: int
    file_name: str
    description: str = ""
    primary: bool = False

    @property
    def current(self) -> bool:
        return self.category not in ("OLD_VERSION", "ARCHIVED", "REMOVED", "DELETED")


@dataclass
class ModDetails(ModSummary):
    description_html: str = ""
    files: list[NexusFile] = field(default_factory=list)


_MOD_FIELDS = ("modId name summary author version downloads endorsements updatedAt "
               "thumbnailUrl pictureUrl adultContent modCategory { name }")


def _summary(n: dict) -> dict:
    return dict(
        mod_id=int(n["modId"]), name=n.get("name") or "?", summary=html.unescape(n.get("summary") or ""),
        author=n.get("author") or "", version=n.get("version") or "",
        downloads=int(n.get("downloads") or 0), endorsements=int(n.get("endorsements") or 0),
        updated=(n.get("updatedAt") or "")[:10], thumb=n.get("thumbnailUrl") or "",
        picture=n.get("pictureUrl") or "", category=(n.get("modCategory") or {}).get("name", ""),
        adult=bool(n.get("adultContent")),
    )


_CATEGORY_CACHE: dict[str, list[tuple[str, int]]] = {}


def mod_categories(domain: str) -> list[tuple[str, int]]:
    """[(category name, number of mods)] for a game, alphabetical."""
    if domain not in _CATEGORY_CACHE:
        d = _gql("query($f: ModsFilter) { mods(filter: $f, count: 1, facets: {categoryName: []})"
                 " { facetsData } }", {"f": {"gameDomainName": [{"value": domain, "op": "EQUALS"}]}})
        cats = ((d.get("mods") or {}).get("facetsData") or {}).get("categoryName") or {}
        _CATEGORY_CACHE[domain] = sorted(((k.strip(), int(v)) for k, v in cats.items() if k.strip()),
                                         key=lambda kv: kv[0].lower())
    return _CATEGORY_CACHE[domain]


def search(domain: str, text: str = "", sort: str = "endorsements", page: int = 1,
           per_page: int = 20, adult: bool = False, category: str = "") -> tuple[list[ModSummary], int]:
    flt: dict = {"gameDomainName": [{"value": domain, "op": "EQUALS"}]}
    if category:
        flt["categoryName"] = [{"value": category, "op": "EQUALS"}]
    if text.strip():
        flt["name"] = [{"value": text.strip(), "op": "WILDCARD"}]
    if not adult:
        flt["adultContent"] = [{"value": False, "op": "EQUALS"}]
    q = ("query($f: ModsFilter, $s: [ModsSort!], $n: Int, $o: Int) {"
         " mods(filter: $f, sort: $s, count: $n, offset: $o) { totalCount nodes { %s } } }" % _MOD_FIELDS)
    d = _gql(q, {"f": flt, "s": [{sort: {"direction": "DESC"}}], "n": per_page,
                 "o": (page - 1) * per_page}).get("mods") or {}
    return [ModSummary(**_summary(n)) for n in d.get("nodes") or []], int(d.get("totalCount") or 0)


def mod_details(domain: str, game_id: int, mod_id: int) -> ModDetails:
    return _cached(("details", domain, mod_id), lambda: _mod_details(domain, game_id, mod_id))


def _mod_details(domain: str, game_id: int, mod_id: int) -> ModDetails:
    q = ("query($ids: [CompositeDomainWithIdInput!]!, $mid: ID!, $gid: ID!) {"
         " legacyModsByDomain(ids: $ids) { nodes { %s description } }"
         " modFiles(modId: $mid, gameId: $gid) { fileId name version category date sizeInBytes"
         " uri description primary } }" % _MOD_FIELDS)
    d = _gql(q, {"ids": [{"gameDomain": domain, "modId": mod_id}], "mid": mod_id, "gid": game_id})
    nodes = (d.get("legacyModsByDomain") or {}).get("nodes") or []
    if not nodes:
        raise NexusError(f"Mod {mod_id} not found (it may be hidden or deleted).")
    n = nodes[0]
    det = ModDetails(**_summary(n), description_html=bbcode_to_html(n.get("description") or ""))
    det.files = [_file(f) for f in d.get("modFiles") or []]
    det.files.sort(key=lambda f: (not f.current, f.category != "MAIN", -f.date))
    return det


def mod_files(game_id: int, mod_id: int) -> list[NexusFile]:
    def fetch():
        q = ("query($mid: ID!, $gid: ID!) { modFiles(modId: $mid, gameId: $gid) {"
             " fileId name version category date sizeInBytes uri description primary } }")
        return [_file(f) for f in _gql(q, {"mid": mod_id, "gid": game_id}).get("modFiles") or []]
    return list(_cached(("files", game_id, mod_id), fetch))


def updated_at(domain: str, mod_ids: list[int], batch: int = 25) -> dict[int, float]:
    """{mod id: last-updated time (epoch)} for many mods, 25 per request."""
    from datetime import datetime
    out: dict[int, float] = {}
    q = ("query($ids:[CompositeDomainWithIdInput!]!, $n:Int){ legacyModsByDomain(ids:$ids, count:$n){"
         " nodes{ modId updatedAt } } }")
    ids = list(dict.fromkeys(mod_ids))
    for i in range(0, len(ids), batch):
        part = ids[i:i + batch]
        d = _gql(q, {"ids": [{"gameDomain": domain, "modId": x} for x in part], "n": len(part)})
        for n in (d.get("legacyModsByDomain") or {}).get("nodes") or []:
            try:
                out[int(n["modId"])] = datetime.fromisoformat(
                    str(n.get("updatedAt")).replace("Z", "+00:00")).timestamp()
            except (TypeError, ValueError, KeyError):
                pass
    return out


def _file(f: dict) -> NexusFile:
    return NexusFile(
        file_id=int(f["fileId"]), name=f.get("name") or "", version=f.get("version") or "",
        category=f.get("category") or "", date=int(f.get("date") or 0),
        size=int(f.get("sizeInBytes") or 0), file_name=f.get("uri") or "",
        description=html.unescape(f.get("description") or ""), primary=bool(f.get("primary")),
    )


def fetch_bytes(url: str) -> bytes:
    r = session.get(url, timeout=20)
    r.raise_for_status()
    return r.content


def mod_page(domain: str, mod_id: int) -> str:
    return f"{SITE}/{domain}/mods/{mod_id}"


def manual_download_page(domain: str, mod_id: int, file_id: int) -> str:
    """The site page with the 'Mod Manager Download' button for one file."""
    return f"{SITE}/{domain}/mods/{mod_id}?tab=files&file_id={file_id}&nmm=1"


# ------------------------------------------------------------------ v1 (needs API key)
@dataclass
class User:
    name: str
    is_premium: bool
    user_id: int


def _v1(path: str, api_key: str, params: dict | None = None, post: dict | None = None):
    if not api_key:
        raise NexusError("No Nexus API key set. Add it in Settings.")
    try:
        if post is not None:
            r = session.post(f"{V1}/{path}", data=post, headers={"apikey": api_key}, timeout=25)
        else:
            r = session.get(f"{V1}/{path}", params=params, headers={"apikey": api_key}, timeout=25)
    except requests.RequestException as e:
        raise NexusError(f"Network error: {e}") from e
    if r.status_code == 401:
        raise NexusError("Nexus rejected the API key (401). Check it in Settings.")
    if r.status_code == 403:
        try:
            msg = r.json().get("message", "")
        except ValueError:
            msg = ""
        raise NexusError(msg or "Nexus refused the request (403). Free accounts must use the "
                                "site's “Mod Manager Download” button.")
    if r.status_code == 429:
        raise NexusError("Nexus API rate limit reached; try again later.")
    if r.status_code >= 400:
        raise NexusError(f"Nexus returned HTTP {r.status_code}")
    return r.json()


def validate_key(api_key: str) -> User:
    d = _v1("users/validate.json", api_key)
    return User(name=d.get("name", "?"), is_premium=bool(d.get("is_premium")),
                user_id=int(d.get("user_id") or 0))


def download_links(api_key: str, domain: str, mod_id: int, file_id: int,
                   key: str | None = None, expires: str | None = None) -> list[str]:
    params = {"key": key, "expires": expires} if key else None
    d = _v1(f"games/{domain}/mods/{mod_id}/files/{file_id}/download_link.json", api_key, params)
    urls = [x.get("URI") for x in d if x.get("URI")]
    if not urls:
        raise NexusError("Nexus returned no download servers.")
    return urls


def endorsements(api_key: str) -> dict[tuple[str, int], str]:
    """{(domain, mod_id): "Endorsed" | "Abstained"} for the key's account."""
    return {(e.get("domain_name", ""), int(e.get("mod_id") or 0)): e.get("status", "")
            for e in _v1("user/endorsements.json", api_key) or []}


def endorse(api_key: str, domain: str, mod_id: int, version: str, undo: bool = False) -> str:
    """Endorse (or withdraw the endorsement of) a mod. Returns the new status."""
    action = "abstain" if undo else "endorse"
    _v1(f"games/{domain}/mods/{mod_id}/{action}.json", api_key, post={"version": version or ""})
    return "Abstained" if undo else "Endorsed"


# ------------------------------------------------------------------ nxm://
@dataclass
class NxmLink:
    domain: str
    mod_id: int
    file_id: int
    key: str | None = None
    expires: str | None = None
    user_id: str | None = None


@dataclass
class NxmCollection:
    domain: str
    slug: str
    revision: int | None


def parse_nxm(uri: str) -> NxmLink | NxmCollection:
    """nxm://<domain>/mods/<mod_id>/files/<file_id>?key=..&expires=..&user_id=..
    nxm://<domain>/collections/<slug>/revisions/<n>"""
    u = urlparse(uri.strip())
    if u.scheme.lower() != "nxm":
        raise ValueError("Not an nxm:// link")
    parts = [p for p in u.path.split("/") if p]
    if parts[:1] == ["collections"] and len(parts) >= 2:
        rev = parts[3] if len(parts) >= 4 and parts[2] == "revisions" else None
        return NxmCollection(domain=u.netloc.lower(), slug=parts[1],
                             revision=int(rev) if rev and rev.isdigit() else None)
    if len(parts) < 4 or parts[0] != "mods" or parts[2] != "files":
        raise ValueError(f"Unrecognised nxm link: {uri}")
    q = parse_qs(u.query)
    return NxmLink(domain=u.netloc.lower(), mod_id=int(parts[1]), file_id=int(parts[3]),
                   key=(q.get("key") or [None])[0], expires=(q.get("expires") or [None])[0],
                   user_id=(q.get("user_id") or [None])[0])


# ------------------------------------------------------------------ BBCode
_SIMPLE = {
    "b": ("<b>", "</b>"), "i": ("<i>", "</i>"), "u": ("<u>", "</u>"), "s": ("<s>", "</s>"),
    "center": ("<div align='center'>", "</div>"), "right": ("<div align='right'>", "</div>"),
    "quote": ("<blockquote>", "</blockquote>"), "code": ("<pre>", "</pre>"),
    "spoiler": ("<div style='background:#8884'>", "</div>"), "heading": ("<h3>", "</h3>"),
    "line": ("<hr>", ""),
}
_SIZES = {"1": "x-small", "2": "small", "3": "medium", "4": "large", "5": "x-large", "6": "xx-large"}


def bbcode_to_html(text: str) -> str:
    """Good-enough BBCode → Qt rich text for Nexus descriptions. Images become links
    (QTextBrowser can't fetch remote images)."""
    t = text.replace("<br />", "\n").replace("<br>", "\n")
    t = html.escape(html.unescape(t), quote=False)
    t = re.sub(r"\[img\](.*?)\[/img\]", r"<a href='\1'>[image]</a>", t, flags=re.I | re.S)
    t = re.sub(r"\[url=([^\]]+)\](.*?)\[/url\]", r"<a href='\1'>\2</a>", t, flags=re.I | re.S)
    t = re.sub(r"\[url\](.*?)\[/url\]", r"<a href='\1'>\1</a>", t, flags=re.I | re.S)
    t = re.sub(r"\[youtube\](.*?)\[/youtube\]",
               r"<a href='https://youtu.be/\1'>[YouTube video]</a>", t, flags=re.I | re.S)
    t = re.sub(r"\[color=([#\w]+)\]", r"<span style='color:\1'>", t, flags=re.I)
    t = re.sub(r"\[/color\]", "</span>", t, flags=re.I)
    t = re.sub(r"\[size=(\d)\]", lambda m: f"<span style='font-size:{_SIZES.get(m.group(1), 'medium')}'>",
               t, flags=re.I)
    t = re.sub(r"\[/size\]", "</span>", t, flags=re.I)
    t = re.sub(r"\[font=[^\]]*\]|\[/font\]", "", t, flags=re.I)
    t = re.sub(r"\[list(=1)?\]", lambda m: "<ol>" if m.group(1) else "<ul>", t, flags=re.I)
    t = re.sub(r"\[/list\]", "</ul>", t, flags=re.I)
    t = re.sub(r"\[\*\]", "<li>", t)
    t = re.sub(r"\[/\*\]", "</li>", t)
    for tag, (a, b) in _SIMPLE.items():
        t = re.sub(rf"\[{tag}(=[^\]]*)?\]", a, t, flags=re.I)
        t = re.sub(rf"\[/{tag}\]", b, t, flags=re.I)
    t = re.sub(r"\[/?(?:left|justify|indent|ol|ul|li|table|tr|td|th|hr)\]", "", t, flags=re.I)
    return t.replace("\n", "<br>")
