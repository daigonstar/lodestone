"""Steam library discovery and Proton prefix paths."""
from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path


def steam_roots() -> list[Path]:
    home = Path.home()
    cands = [
        home / ".local/share/Steam",
        home / ".steam/steam",
        home / ".steam/root",
        home / ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        home / "snap/steam/common/.local/share/Steam",
    ]
    seen, out = set(), []
    for c in cands:
        try:
            r = c.resolve()
        except OSError:
            continue
        if r.is_dir() and r not in seen:
            seen.add(r)
            out.append(r)
    return out


def libraries() -> list[Path]:
    libs: list[Path] = []
    for root in steam_roots():
        libs.append(root)
        vdf = root / "steamapps/libraryfolders.vdf"
        if vdf.is_file():
            for m in re.finditer(r'"path"\s+"([^"]+)"', vdf.read_text(errors="replace")):
                libs.append(Path(m.group(1).replace("\\\\", "\\")))
    seen, out = set(), []
    for lib in libs:
        try:
            r = lib.resolve()
        except OSError:
            continue
        if r not in seen and r.is_dir():
            seen.add(r)
            out.append(r)
    return out


def find_app(app_id: int) -> tuple[Path, Path] | None:
    """Return (install_dir, library) for a Steam app, or None."""
    for lib in libraries():
        acf = lib / f"steamapps/appmanifest_{app_id}.acf"
        if acf.is_file():
            m = re.search(r'"installdir"\s+"([^"]+)"', acf.read_text(errors="replace"))
            if m:
                d = lib / "steamapps/common" / m.group(1)
                if d.is_dir():
                    return d, lib
    return None


def library_of(path: Path) -> Path | None:
    """The Steam library containing path (…/steamapps/common/<game>)."""
    p = Path(path).resolve()
    for parent in p.parents:
        if parent.name == "steamapps":
            return parent.parent
    return None


def proton_prefix(app_id: int, game_dir: Path | None = None) -> Path | None:
    """drive_c of the game's Proton prefix, if it exists."""
    libs = []
    if game_dir:
        lib = library_of(game_dir)
        if lib:
            libs.append(lib)
    libs += libraries()
    for lib in libs:
        p = lib / f"steamapps/compatdata/{app_id}/pfx/drive_c"
        if p.is_dir():
            return p
    return None


def prefix_user_dir(app_id: int, game_dir: Path | None = None) -> Path | None:
    c = proton_prefix(app_id, game_dir)
    return c / "users/steamuser" if c else None


def launch(app_id: int) -> None:
    url = f"steam://rungameid/{app_id}"
    for cmd in (["steam", url], ["xdg-open", url]):
        if shutil.which(cmd[0]):
            subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             start_new_session=True)
            return
    raise RuntimeError("Neither 'steam' nor 'xdg-open' was found on PATH.")


ART_KINDS = ("icon", "header", "hero", "logo", "capsule")
_ART_NAMES = {"header": ("header.jpg", "library_header.jpg"), "hero": ("library_hero.jpg",),
              "logo": ("logo.png",), "capsule": ("library_600x900.jpg", "library_capsule.jpg")}


def art(appid: int) -> dict[str, Path]:
    """Game artwork from Steam's local library cache: icon, header, hero, logo, capsule.
    Handles both the flat layout and the newer hashed sub-folder layout."""
    import re as _re
    out: dict[str, Path] = {}
    for root in steam_roots():
        d = root / "appcache" / "librarycache" / str(appid)
        if not d.is_dir():
            continue
        for f in d.rglob("*"):
            if not f.is_file():
                continue
            if f.parent == d and _re.fullmatch(r"[0-9a-f]{40}\.(jpg|png|ico)", f.name):
                out.setdefault("icon", f)
            for kind, names in _ART_NAMES.items():
                if f.name in names:
                    out.setdefault(kind, f)
        flat = Path(root) / "appcache" / "librarycache"
        for kind, pat in (("icon", f"{appid}_icon.jpg"), ("header", f"{appid}_header.jpg"),
                          ("hero", f"{appid}_library_hero.jpg"), ("logo", f"{appid}_logo.png"),
                          ("capsule", f"{appid}_library_600x900.jpg")):
            if (flat / pat).is_file():
                out.setdefault(kind, flat / pat)
    return out


def art_cdn_urls(appid: int) -> dict[str, str]:
    base = f"https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{appid}"
    return {"header": f"{base}/header.jpg", "hero": f"{base}/library_hero.jpg",
            "logo": f"{base}/logo.png", "capsule": f"{base}/library_600x900.jpg"}
