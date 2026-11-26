"""What each game needs before modding: loaders and the frameworks almost every
mod list builds on. Required items are what mods won't work without;
recommended ones are what most guides install first."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from .archive import ci_resolve

if TYPE_CHECKING:
    from .instance import Instance


@dataclass
class Essential:
    key: str
    name: str
    why: str
    required: bool
    nexus_id: int = 0                     # 0 = not a Nexus download (see kind)
    kind: str = "nexus"                   # "nexus" or "smapi"
    detect: list[str] = field(default_factory=list)  # game-folder paths/globs: any present = installed
    prefer: tuple[str, ...] = ()          # words that pick the right main file
    # A file only the build for the installed game version has, e.g. "skse64_{u}.dll"
    # ({u} = 1_6_1170, {d} = 1-6-1170). Missing while the essential is installed means
    # it's made for another game version.
    runtime_file: str = ""


ESSENTIALS: dict[str, list[Essential]] = {
    "skyrimse": [
        Essential("skse", "Skyrim Script Extender (SKSE64)",
                  "Lets SKSE plugins run. Most large Skyrim mods need it.", True, 30379,
                  detect=["skse64_loader.exe"], prefer=("steam",), runtime_file="skse64_{u}.dll"),
        Essential("addresslib", "Address Library for SKSE Plugins",
                  "Keeps SKSE plugins working across game updates.", True, 32444,
                  detect=["Data/SKSE/Plugins/versionlib-*.bin", "Data/SKSE/Plugins/version-*.bin"],
                  prefer=("all in one",), runtime_file="Data/SKSE/Plugins/versionlib-{d}-0.bin"),
        Essential("ussep", "Unofficial Skyrim Special Edition Patch",
                  "Fixes thousands of bugs; many mods are built on top of it.", False, 266,
                  detect=["Data/unofficial skyrim special edition patch.esp"]),
        Essential("skyui", "SkyUI", "Better menus, and the Mod Configuration Menu many mods use.",
                  False, 12604, detect=["Data/SkyUI_SE.esp"]),
    ],
    "fallout4": [
        Essential("f4se", "Fallout 4 Script Extender (F4SE)",
                  "Lets F4SE plugins run. Most large Fallout 4 mods need it.", True, 42147,
                  detect=["f4se_loader.exe"], runtime_file="f4se_{u}.dll"),
        Essential("addresslib", "Address Library for F4SE Plugins",
                  "Keeps F4SE plugins working across game updates.", True, 47327,
                  detect=["Data/F4SE/Plugins/version-*.bin", "Data/F4SE/Plugins/version-*.csv"],
                  prefer=("all in one",), runtime_file="Data/F4SE/Plugins/version-{d}-0.bin"),
        Essential("ufo4p", "Unofficial Fallout 4 Patch",
                  "Fixes thousands of bugs; many mods are built on top of it.", False, 4598,
                  detect=["Data/Unofficial Fallout 4 Patch.esp"]),
        Essential("mcm", "Mod Configuration Menu", "In-game settings menu used by many mods.",
                  False, 21497, detect=["Data/MCM.esp", "Data/F4SE/Plugins/mcm.dll"]),
    ],
    "cyberpunk2077": [
        Essential("cet", "Cyber Engine Tweaks", "Runs Lua mods and adds the in-game mod console.",
                  True, 107, detect=["bin/x64/plugins/cyber_engine_tweaks.asi"]),
        Essential("red4ext", "RED4ext", "Loads native plugins; ArchiveXL, TweakXL and Codeware need it.",
                  True, 2380, detect=["red4ext/RED4ext.dll"]),
        Essential("redscript", "redscript", "Compiles script mods (.reds).", True, 1511,
                  detect=["engine/tools/scc.exe"]),
        Essential("archivexl", "ArchiveXL", "Lets mods add new items, clothes and models.", False, 4198,
                  detect=["red4ext/plugins/ArchiveXL/ArchiveXL.dll"]),
        Essential("tweakxl", "TweakXL", "Lets mods change game data (tweaks).", False, 4197,
                  detect=["red4ext/plugins/TweakXL/TweakXL.dll"]),
        Essential("codeware", "Codeware", "Script library many newer mods depend on.", False, 7780,
                  detect=["red4ext/plugins/Codeware/Codeware.dll"]),
    ],
    "stardewvalley": [
        Essential("smapi", "SMAPI", "The Stardew mod loader. Without it, no mods load at all.", True,
                  kind="smapi", detect=["StardewModdingAPI"]),
        Essential("contentpatcher", "Content Patcher",
                  "Needed by most content mods (anything marked [CP]).", False, 1915,
                  detect=["Mods/ContentPatcher/ContentPatcher.dll"]),
        Essential("gmcm", "Generic Mod Config Menu", "In-game settings for your mods.", False, 5098,
                  detect=["Mods/GenericModConfigMenu/GenericModConfigMenu.dll"]),
    ],
}

NOTES = {
    "witcher3": "No loader is needed: mods load from the game's mods folder.",
    "nomanssky": "No loader is needed: mods load from GAMEDATA/MODS.",
    "7daystodie": "No loader is needed, but most mods need the game started without "
                  "EasyAntiCheat. Pick “Play without EAC” when Steam asks.",
}


def _notes_from_games():
    """Each newer game's own notes (what it needs) double as its setup notes."""
    from .games import all_games
    for g in all_games():
        if g.notes and g.id not in NOTES and g.id not in ESSENTIALS:
            NOTES[g.id] = g.notes


_notes_from_games()


def _present(game_dir: Path, pattern: str) -> bool:
    if any(ch in pattern for ch in "*?["):
        parent, _, glob = pattern.rpartition("/")
        d = ci_resolve(game_dir, parent) if parent else game_dir
        if d is None or not Path(d).is_dir():
            return False
        import fnmatch
        return any(fnmatch.fnmatch(c.name.lower(), glob.lower()) for c in Path(d).iterdir())
    return ci_resolve(game_dir, pattern) is not None


def downloaded(inst: "Instance", e: Essential):
    """The Kiln mod for this essential, preferring one in the current profile."""
    if not e.nexus_id:
        return None
    ms = [m for m in inst.mods if m.nexus_mod_id == e.nexus_id]
    return next((m for m in ms if m.enabled), None) or next((m for m in ms if inst.in_profile(m)), None) \
        or (max(ms, key=lambda m: m.nexus_file_id or 0) if ms else None)


def game_runtime(inst: "Instance") -> tuple[int, int, int] | None:
    """The installed game's version from its exe, e.g. (1, 6, 1170)."""
    from . import pe
    v = pe.file_version(inst.game_dir / inst.game.exe) if inst.game.exe else None
    return tuple(v[:3]) if v else None


def runtime_path(e: Essential, rt) -> str:
    return e.runtime_file.format(u="_".join(map(str, rt)), d="-".join(map(str, rt)))


def status(inst: "Instance", e: Essential) -> str:
    """'installed'; 'mismatch' (installed, but made for another game version);
    'disabled' (in this profile but switched off); 'elsewhere' (downloaded for
    another profile, so it can be reused); or 'missing'."""
    st = _status(inst, e)
    if st == "installed" and e.runtime_file:
        rt = game_runtime(inst)
        if rt and not _present(inst.game_dir, runtime_path(e, rt)):
            return "mismatch"
    return st


def _status(inst: "Instance", e: Essential) -> str:
    if e.nexus_id:
        mine = downloaded(inst, e)
        if mine is not None:
            if mine.enabled:
                return "installed"
            return "disabled" if inst.in_profile(mine) else "elsewhere"
    if any(_present(inst.game_dir, p) for p in e.detect):
        return "installed"
    return "missing"


def mismatched(inst: "Instance") -> list[Essential]:
    return [e for e in for_game(inst.game.id) if status(inst, e) == "mismatch"]


def for_game(game_id: str) -> list[Essential]:
    return ESSENTIALS.get(game_id, [])


def missing(inst: "Instance", required_only: bool = False) -> list[Essential]:
    return [e for e in for_game(inst.game.id)
            if status(inst, e) != "installed" and (e.required or not required_only)]


def pick_file(files, e: Essential, rt=None, mine=None):
    """Choose the file to install. With the game version known (rt) and an
    essential tied to it, the file made for that version (older files included);
    otherwise current MAIN files, preferring the primary one and any file whose
    name has the essential's preferred words."""
    if rt and e.runtime_file:
        from . import versionmatch
        f, _sure = versionmatch.best_file(files, rt, mine)
        if f is not None:
            return f
    mains = [f for f in files if f.category == "MAIN"] or [f for f in files if f.category != "ARCHIVED"]
    if not mains:
        return None
    def score(f):
        name = (f.name + " " + f.description).lower()
        return (sum(w in name for w in e.prefer), f.primary, f.date)
    return max(mains, key=score)
