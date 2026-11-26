"""Game settings normally set in a game's own launcher (resolution, fullscreen,
quality). Kiln starts Bethesda games through their script extender, which skips
the launcher, and Skyrim SE has no display options in game, so Kiln edits the
same files the launcher writes: <Game>Prefs.ini in the Proton prefix, using the
launcher's own Low/Medium/High/Ultra presets from the game folder.

Mods that take over display settings (SSE Display Tweaks) are updated too, since
theirs win.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from . import steam
from .archive import ci_child

if TYPE_CHECKING:
    from .instance import Instance


@dataclass
class Spec:
    prefs: str              # prefs ini, relative to Documents/My Games
    launcher: str           # the game's own launcher exe
    presets: tuple[str, ...] = ("Low", "Medium", "High", "Ultra")


SPECS = {
    "skyrimse": Spec("Skyrim Special Edition/SkyrimPrefs.ini", "SkyrimSELauncher.exe"),
    "fallout4": Spec("Fallout4/Fallout4Prefs.ini", "Fallout4Launcher.exe"),
    "skyrim": Spec("Skyrim/SkyrimPrefs.ini", "SkyrimLauncher.exe"),
    "newvegas": Spec("FalloutNV/FalloutPrefs.ini", "FalloutNVLauncher.exe"),
    "fallout3": Spec("Fallout3/FalloutPrefs.ini", "Fallout3Launcher.exe"),
    "oblivion": Spec("Oblivion/Oblivion.ini", "OblivionLauncher.exe"),
}
# keys the display settings use in the prefs ini ([Display] section)
FULL, BORDERLESS, W, H, VSYNC = "bFull Screen", "bBorderless", "iSize W", "iSize H", "iVSyncPresentInterval"
TAA, FXAA = "bUseTAA", "bFXAAEnabled"
KEEP_ON_PRESET = {k.lower() for k in (FULL, BORDERLESS, W, H, VSYNC)}   # a preset doesn't change your screen


def supported(inst: "Instance") -> bool:
    return inst.game.id in SPECS


def prefs_path(inst: "Instance") -> Path | None:
    s = SPECS.get(inst.game.id)
    u = steam.prefix_user_dir(inst.game.steam_appid, inst.game_dir)
    return u / "Documents/My Games" / s.prefs if (s and u) else None


# ------------------------------------------------------------------ a careful ini editor
def read_ini(path: Path) -> dict[str, dict[str, str]]:
    out: dict[str, dict[str, str]] = {}
    sec = ""
    try:
        text = path.read_text(encoding="utf-8-sig", errors="replace")
    except OSError:
        return out
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            sec = s[1:-1].lower()
            out.setdefault(sec, {})
        elif "=" in s and not s.startswith((";", "#")):
            k, v = s.split("=", 1)
            out.setdefault(sec, {})[k.strip().lower()] = v.strip()
    return out


def write_ini(path: Path, changes: dict[str, dict[str, str]]) -> None:
    """Set keys (case-insensitive, sections too), keeping everything else and the
    file's own line endings. Writes in place so hardlinked (deployed) files stay linked."""
    try:
        raw = path.read_bytes()
    except OSError:
        raw = b""
    text = raw.decode("utf-8-sig", errors="replace")
    nl = "\r\n" if "\r\n" in text or not text else "\n"
    lines = text.splitlines()
    todo = {sec.lower(): {k.lower(): (k, v) for k, v in kv.items()} for sec, kv in changes.items()}
    out, sec = [], None

    def flush(s):
        for _low, (k, v) in list(todo.get(s, {}).items()):
            out.append(f"{k}={v}")
        todo.pop(s, None)

    for line in lines:
        st = line.strip()
        if st.startswith("[") and st.endswith("]"):
            if sec is not None:
                # new keys go after the section's last setting, before its blank lines
                blanks = 0
                while out and not out[-1].strip() and todo.get(sec):
                    out.pop()
                    blanks += 1
                flush(sec)
                out += [""] * blanks
            sec = st[1:-1].lower()
            out.append(line)
            continue
        if sec is not None and "=" in st and not st.startswith((";", "#")):
            k = st.split("=", 1)[0].strip().lower()
            if k in todo.get(sec, {}):
                key, v = todo[sec].pop(k)
                out.append(f"{line.split('=', 1)[0].rstrip()}={v}")
                continue
        out.append(line)
    if sec is not None:
        flush(sec)
    for s, kv in todo.items():
        if kv:
            out += ["", f"[{s[:1].upper() + s[1:]}]"] + [f"{k}={v}" for k, v in kv.values()]
    data = (nl.join(out).strip(nl) + nl).encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "r+b" if path.exists() else "wb") as fh:
        fh.seek(0)
        fh.write(data)
        fh.truncate()


# ------------------------------------------------------------------ the settings
@dataclass
class Settings:
    width: int = 1920
    height: int = 1080
    mode: str = "fullscreen"         # fullscreen | borderless | window
    vsync: bool = True
    aa: str = "taa"                  # off | fxaa | taa
    preset: str = ""                 # "" = leave quality as it is


def current(inst: "Instance") -> Settings:
    d = read_ini(prefs_path(inst)).get("display", {}) if prefs_path(inst) else {}
    s = Settings()
    s.width = int(d.get(W.lower(), s.width) or s.width)
    s.height = int(d.get(H.lower(), s.height) or s.height)
    full, border = d.get(FULL.lower(), "1") == "1", d.get(BORDERLESS.lower(), "0") == "1"
    s.mode = "fullscreen" if full else ("borderless" if border else "window")
    s.vsync = d.get(VSYNC.lower(), "1") != "0"
    s.aa = "taa" if d.get(TAA.lower()) == "1" else ("fxaa" if d.get(FXAA.lower()) == "1" else "off")
    return s


def presets(inst: "Instance") -> list[str]:
    s = SPECS.get(inst.game.id)
    return [p for p in (s.presets if s else ()) if ci_child(inst.game_dir, f"{p}.ini") is not None]


def apply(inst: "Instance", s: Settings) -> list[str]:
    """Write the settings. Returns what was changed, in words."""
    path = prefs_path(inst)
    if path is None:
        raise RuntimeError("The game's Proton folder doesn't exist yet. Start the game once through Steam.")
    done = []
    changes: dict[str, dict[str, str]] = {}
    if s.preset:
        src = ci_child(inst.game_dir, f"{s.preset}.ini")
        if src is not None:
            for sec, kv in read_ini(Path(src)).items():
                for k, v in kv.items():
                    if k not in KEEP_ON_PRESET:
                        changes.setdefault(sec, {})[k] = v
            done.append(f"quality preset {s.preset}")
    disp = changes.setdefault("display", {})
    disp.update({FULL: "1" if s.mode == "fullscreen" else "0",
                 BORDERLESS: "1" if s.mode == "borderless" else "0",
                 W: str(s.width), H: str(s.height), VSYNC: "1" if s.vsync else "0",
                 TAA: "1" if s.aa == "taa" else "0", FXAA: "1" if s.aa == "fxaa" else "0"})
    # write keys in their original case where the file already has them
    write_ini(path, changes)
    done.append(f"{s.width}×{s.height}, {s.mode}, V-Sync {'on' if s.vsync else 'off'}, "
                f"antialiasing {s.aa.upper() if s.aa != 'off' else 'off'}")
    done += _display_mods(inst, s)
    return done


def _display_mods(inst: "Instance", s: Settings) -> list[str]:
    """SSE Display Tweaks overrides the game's own display settings: set it too."""
    out = []
    ini = inst.deploy_dir / "SKSE/Plugins/SSEDisplayTweaks.ini"
    if ini.is_file():
        write_ini(ini, {"Render": {"Fullscreen": "true" if s.mode == "fullscreen" else "false",
                                   "Borderless": "true" if s.mode == "borderless" else "false",
                                   "Resolution": f"{s.width}x{s.height}",
                                   "EnableVSync": "true" if s.vsync else "false"}})
        out.append("SSE Display Tweaks (it overrides the game's display settings)")
    return out


def screen_sizes() -> list[tuple[int, int]]:
    """Your monitors' native resolutions first, then common ones."""
    sizes = []
    try:
        from PySide6.QtGui import QGuiApplication
        for scr in QGuiApplication.screens():
            g, r = scr.geometry(), scr.devicePixelRatio()
            sizes.append((round(g.width() * r), round(g.height() * r)))
    except Exception:  # noqa: BLE001
        pass
    for wh in ((3840, 2160), (3440, 1440), (2560, 1440), (2560, 1080), (1920, 1200), (1920, 1080),
               (1680, 1050), (1600, 900), (1366, 768), (1280, 800), (1280, 720)):
        if wh not in sizes:
            sizes.append(wh)
    return sizes


def open_launcher(inst: "Instance") -> None:
    """Run the game's own launcher (its Options window) through the game's Proton."""
    import os
    import subprocess
    from . import vcruntime
    spec = SPECS.get(inst.game.id)
    exe = ci_child(inst.game_dir, spec.launcher) if spec else None
    if exe is None:
        raise RuntimeError("This game's launcher isn't in its folder.")
    proton = vcruntime.proton_for(inst)
    compat = steam.proton_prefix(inst.game.steam_appid, inst.game_dir)
    if proton is None or compat is None:
        raise RuntimeError("Couldn't find the game's Proton setup. Start the game once through Steam.")
    env = dict(os.environ, STEAM_COMPAT_DATA_PATH=str(compat.parent.parent),
               STEAM_COMPAT_CLIENT_INSTALL_PATH=str(next(iter(steam.steam_roots()), Path.home())),
               SteamAppId=str(inst.game.steam_appid), SteamGameId=str(inst.game.steam_appid))
    subprocess.Popen([str(proton), "run", str(exe)], cwd=str(inst.game_dir), env=env,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
