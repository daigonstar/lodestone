"""The Microsoft Visual C++ runtime inside a game's Proton prefix.

Many games' Steam installers put the 2015 runtime (msvcp140.dll 14.0.x) into
the prefix and mark it “native”. Mods compiled with Visual Studio 2022 17.10 or
newer (most script-extender plugins from mid-2024 on) need 14.40+: with the old
one they crash while starting (SKSE: “fatal error occurred while loading plugin”,
or Windows error 998). The fix is Microsoft's current Visual C++ Redistributable,
installed into the prefix through the game's own Proton.
"""
from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

from . import pe, steam, steamcfg

if TYPE_CHECKING:
    from .instance import Instance

MINIMUM = (14, 40, 0)
REDIST_URL = "https://aka.ms/vs/17/release/vc_redist.x64.exe"   # Microsoft's permanent link


def prefix_version(inst: "Instance") -> tuple[int, ...] | None:
    """Version of msvcp140.dll in the game's prefix (None: no prefix / no file)."""
    c = steam.proton_prefix(inst.game.steam_appid, inst.game_dir)
    f = c / "windows/system32/msvcp140.dll" if c else None
    if not f or not f.is_file():
        return None
    v = pe.file_version(f)
    return tuple(v[:3]) if v else None


def too_old(inst: "Instance") -> bool:
    v = prefix_version(inst)
    return bool(v) and v < MINIMUM


def dll_mods(inst: "Instance") -> int:
    """How many enabled mods bring Windows DLLs (the kind that needs the runtime)."""
    return sum(1 for m in inst.mods if m.enabled and any(r.lower().endswith(".dll") for r in inst.mod_files(m)))


# ------------------------------------------------------------------ which Proton runs the game
def _tool_name(appid: int) -> str:
    for root in steam.steam_roots():
        f = root / "config/config.vdf"
        try:
            d = steamcfg.loads(f.read_text(errors="replace"))
        except OSError:
            continue
        node = d
        for k in ("InstallConfigStore", "Software", "Valve", "Steam", "CompatToolMapping"):
            node = next((v for kk, v in node.items() if kk.lower() == k.lower() and isinstance(v, dict)), None)
            if node is None:
                break
        if node:
            for key in (str(appid), "0"):
                name = (node.get(key) or {}).get("name") if isinstance(node.get(key), dict) else None
                if name:
                    return name
    return ""


def proton_for(inst: "Instance") -> Path | None:
    """The `proton` script Steam uses for this game (its own choice, else the default)."""
    name = _tool_name(inst.game.steam_appid)
    cands: list[Path] = []
    for root in steam.steam_roots():
        ctd = root / "compatibilitytools.d"
        if ctd.is_dir():
            for d in ctd.iterdir():
                vdf = d / "compatibilitytool.vdf"
                if vdf.is_file() and name and re.search(rf'"{re.escape(name)}"', vdf.read_text(errors="replace")):
                    cands.append(d / "proton")
    official = {"proton_experimental": ["Proton - Experimental"], "proton_hotfix": ["Proton Hotfix"]}
    m = re.match(r"proton_(\d+)", name)
    if m:
        official[name] = [f"Proton {m.group(1)}.0", f"Proton {m.group(1)}.0 (Beta)"]
    for lib in steam.libraries():
        for n in official.get(name, []):
            cands.append(lib / "steamapps/common" / n / "proton")
    if not cands:      # nothing configured: the newest official Proton installed
        found = [p / "proton" for lib in steam.libraries() for p in (lib / "steamapps/common").glob("Proton *")
                 if (p / "proton").is_file()]
        cands += sorted(found, reverse=True)
    return next((c for c in cands if c.is_file()), None)


def install(inst: "Instance", installer: Path) -> tuple[int, ...] | None:
    """Run Microsoft's redistributable inside the game's prefix. Returns the new version."""
    proton = proton_for(inst)
    if proton is None:
        raise RuntimeError("Couldn't find the Proton version this game uses.")
    compat = steam.proton_prefix(inst.game.steam_appid, inst.game_dir)
    if compat is None:
        raise RuntimeError("The game's Proton folder doesn't exist yet. Start the game once through Steam.")
    env = dict(os.environ,
               STEAM_COMPAT_DATA_PATH=str(compat.parent.parent),           # .../compatdata/<appid>
               STEAM_COMPAT_CLIENT_INSTALL_PATH=str(next(iter(steam.steam_roots()), Path.home())),
               SteamAppId=str(inst.game.steam_appid), SteamGameId=str(inst.game.steam_appid))
    r = subprocess.run([str(proton), "run", str(installer), "/install", "/quiet", "/norestart"],
                       env=env, capture_output=True, timeout=900)
    v = prefix_version(inst)
    if not v or v < MINIMUM:
        raise RuntimeError(f"The installer finished (exit code {r.returncode}) but the runtime is still "
                           f"{'.'.join(map(str, v or ())) or 'missing'}.")
    return v
