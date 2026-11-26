"""Game definitions. Each game says where mods deploy and how to map an
extracted archive onto that deploy root (Vortex calls these 'mod types')."""
from __future__ import annotations

import re
from pathlib import Path
from typing import TYPE_CHECKING

from ..archive import ci_child

if TYPE_CHECKING:
    from ..instance import Instance

Layout = list[tuple[Path, str]]  # (source file/dir, destination relative to deploy root)


def safe_name(s: str) -> str:
    s = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", s).strip(" .")
    return s[:100] or "Mod"


def walk_dirs(root: Path, max_depth: int = 6):
    """Breadth-first directories under root (including root), shallowest first."""
    level = [root]
    for _ in range(max_depth + 1):
        nxt = []
        for d in level:
            yield d
            try:
                nxt += sorted((c for c in d.iterdir() if c.is_dir() and not c.name.startswith("__MACOSX")),
                              key=lambda p: p.name.lower())
            except OSError:
                pass
        level = nxt
        if not level:
            return


def dirs_with_file(root: Path, filename: str) -> list[Path]:
    """Shallowest directories that directly contain filename (case-insensitive);
    directories nested inside an earlier match are skipped."""
    found: list[Path] = []
    for d in walk_dirs(root):
        if any(f == d or f in d.parents for f in found):
            continue
        c = ci_child(d, filename)
        if c is not None and c.is_file():
            found.append(d)
    return found


def dir_with_children(root: Path, names: set[str], exts: set[str] = frozenset()) -> Path | None:
    """Shallowest dir having a child whose lowercase name is in names, or a file
    with one of exts."""
    for d in walk_dirs(root):
        try:
            for c in d.iterdir():
                n = c.name.lower()
                if n in names or (c.is_file() and Path(n).suffix in exts):
                    return d
        except OSError:
            pass
    return None


class GameDef:
    id = ""
    name = ""
    steam_appid = 0
    nexus_domain = ""
    nexus_game_id = 0
    exe = ""                 # relative path used to validate the install folder
    deploy_root = ""         # relative to the game folder
    has_plugins = False      # Bethesda-style plugin load order
    notes = ""
    trust_steam = False      # found through Steam by app id: no exe check needed
    alt_appids: tuple[int, ...] = ()   # other Steam editions of the same game

    def deploy_dir(self, inst: "Instance") -> Path:
        """Where mods go. Most games: a folder inside the game folder."""
        return inst.game_dir / self.deploy_root if self.deploy_root else inst.game_dir

    def not_ready(self, inst: "Instance") -> str:
        """Why mods can't be deployed yet ('' if they can)."""
        return ""

    def layout(self, root: Path, mod_name: str) -> Layout:
        """Map an extracted archive onto the deploy root. Return [] if unknown."""
        return [(root, "")]

    def fomod_base(self) -> str:
        """Where FOMOD destinations are relative to (inside the deploy root)."""
        return ""

    def post_deploy(self, inst: "Instance") -> list[str]:
        """Hook after deployment (write plugins.txt, ini tweaks...). Returns warnings."""
        return []

    def validate(self, game_dir: Path) -> bool:
        return (Path(game_dir) / self.exe).is_file()


# ---------------------------------------------------------------- simple games
class Cyberpunk2077(GameDef):
    id, name = "cyberpunk2077", "Cyberpunk 2077"
    steam_appid, nexus_domain, nexus_game_id = 1091500, "cyberpunk2077", 3333
    exe = "bin/x64/Cyberpunk2077.exe"
    ROOT_DIRS = {"archive", "bin", "r6", "red4ext", "engine", "mods"}
    notes = ("Mods deploy into the game folder. Frameworks such as Cyber Engine Tweaks, "
             "RED4ext and redscript are installed like any other mod.")

    def layout(self, root, mod_name):
        d = dir_with_children(root, self.ROOT_DIRS)
        if d is not None:
            return [(d, "")]
        out: Layout = []
        for cet in dirs_with_file(root, "init.lua"):
            out.append((cet, f"bin/x64/plugins/cyber_engine_tweaks/mods/{cet.name}"))
        for f in root.rglob("*"):
            if not f.is_file() or any(c == f or c in f.parents for c, _ in out):
                continue
            ext = f.suffix.lower()
            if ext in (".archive", ".xl"):
                out.append((f, f"archive/pc/mod/{f.name}"))
            elif ext == ".reds":
                out.append((f, f"r6/scripts/{safe_name(mod_name)}/{f.name}"))
            elif ext in (".yaml", ".yml", ".tweak"):
                out.append((f, f"r6/tweaks/{safe_name(mod_name)}/{f.name}"))
        return out


class StardewValley(GameDef):
    id, name = "stardewvalley", "Stardew Valley"
    steam_appid, nexus_domain, nexus_game_id = 413150, "stardewvalley", 1303
    exe = "Stardew Valley.dll"
    deploy_root = "Mods"
    notes = ("Mods need SMAPI. Install SMAPI with its own installer, then set the Steam launch "
             "option to run StardewModdingAPI.")

    def validate(self, game_dir):
        g = Path(game_dir)
        return any((g / n).is_file() for n in ("Stardew Valley.dll", "Stardew Valley.exe",
                                                "StardewValley", "Stardew Valley"))

    def layout(self, root, mod_name):
        return [(d, d.name) for d in dirs_with_file(root, "manifest.json")]


class SevenDaysToDie(GameDef):
    id, name = "7daystodie", "7 Days to Die"
    steam_appid, nexus_domain, nexus_game_id = 251570, "7daystodie", 1059
    exe = "7DaysToDie.exe"
    deploy_root = "Mods"

    def validate(self, game_dir):
        g = Path(game_dir)
        return (g / "7DaysToDie.exe").is_file() or (g / "7DaysToDie.x86_64").is_file()

    def layout(self, root, mod_name):
        return [(d, d.name) for d in dirs_with_file(root, "ModInfo.xml")]


class Witcher3(GameDef):
    id, name = "witcher3", "The Witcher 3"
    steam_appid, nexus_domain, nexus_game_id = 292030, "witcher3", 952
    exe = "bin/x64/witcher3.exe"
    notes = "Merge conflicting script mods with Script Merger (run it through Proton)."

    def validate(self, game_dir):
        g = Path(game_dir)
        return (g / "bin/x64/witcher3.exe").is_file() or (g / "bin/x64_dx12/witcher3.exe").is_file()

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"mods", "dlc", "bin"})
        if d is not None:
            return [(d, "")]
        out: Layout = []
        for d in walk_dirs(root):
            if any(s == d or s in d.parents for s, _ in out):
                continue
            if ci_child(d, "content") is not None and d != root:
                kind = "dlc" if d.name.lower().startswith("dlc") else "mods"
                name = d.name if kind == "dlc" or d.name.lower().startswith("mod") else f"mod{d.name}"
                out.append((d, f"{kind}/{name}"))
        if not out and ci_child(root, "content") is not None:
            out.append((root, f"mods/mod{safe_name(mod_name).replace(' ', '')}"))
        return out


class NoMansSky(GameDef):
    id, name = "nomanssky", "No Man's Sky"
    steam_appid, nexus_domain, nexus_game_id = 275850, "nomanssky", 1634
    exe = "Binaries/NMS.exe"
    deploy_root = "GAMEDATA/MODS"

    def layout(self, root, mod_name):
        for d in walk_dirs(root, 3):
            gd = ci_child(d, "GAMEDATA")
            if gd is not None:
                mods = ci_child(gd, "MODS")
                return [(mods, "")] if mods is not None else []
        from ..archive import unwrap
        top = unwrap(root)
        files = [c for c in top.iterdir() if c.is_file()]
        if files or top != root:
            return [(top, safe_name(mod_name if top == root else top.name))]
        return [(c, c.name) for c in top.iterdir() if c.is_dir()]


# ---------------------------------------------------------------- registry
_GAMES: list[GameDef] | None = None


def all_games() -> list[GameDef]:
    global _GAMES
    if _GAMES is None:
        from .bethesda import Fallout4, SkyrimSE
        from .more import more_games
        _GAMES = [SkyrimSE(), Fallout4(), Cyberpunk2077(), StardewValley(), Witcher3(),
                  NoMansSky(), SevenDaysToDie()] + more_games()
    return list(_GAMES)


def by_id(game_id: str) -> GameDef | None:
    return next((g for g in all_games() if g.id == game_id), None)


def by_domain(domain: str) -> GameDef | None:
    return next((g for g in all_games() if g.nexus_domain == domain.lower()), None)
