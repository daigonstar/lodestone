"""Bethesda games (Skyrim SE/AE, Fallout 4): Data folder mods, plugin load
order written to plugins.txt inside the Proton prefix."""
from __future__ import annotations

import struct
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from .. import steam
from ..archive import ci_child
from . import walk_dirs, GameDef, dir_with_children

if TYPE_CHECKING:
    from ..instance import Instance

PLUGIN_EXTS = {".esp", ".esm", ".esl"}
DATA_MARKERS = {
    "meshes", "textures", "scripts", "interface", "sound", "music", "strings", "seq",
    "skse", "f4se", "materials", "lodsettings", "shadersfx", "video", "grass", "nemesis_engine",
    "meshes", "facegen", "dialogueviews", "calientetools", "netscriptframework", "sky", "vis",
    "programs", "distantlod", "lod", "terrain", "mcm", "platform", "source", "tools",
}
DATA_EXTS = PLUGIN_EXTS | {".bsa", ".ba2"}

FLAG_MASTER = 0x1
FLAG_LIGHT = 0x200


@dataclass
class PluginInfo:
    name: str
    is_master: bool = False   # ESM flag or .esm/.esl extension
    is_light: bool = False    # ESL flag or .esl
    masters: list[str] = field(default_factory=list)
    error: str = ""


def read_plugin(path: Path) -> PluginInfo:
    info = PluginInfo(path.name)
    ext = path.suffix.lower()
    info.is_master = ext in (".esm", ".esl")
    info.is_light = ext == ".esl"
    try:
        with open(path, "rb") as fh:
            head = fh.read(24)
            if len(head) < 24 or head[:4] != b"TES4":
                info.error = "not a TES4 plugin"
                return info
            size, flags = struct.unpack_from("<II", head, 4)
            info.is_master |= bool(flags & FLAG_MASTER)
            info.is_light |= bool(flags & FLAG_LIGHT)
            data = fh.read(min(size, 1 << 20))
        off = 0
        while off + 6 <= len(data):
            typ = data[off:off + 4]
            ln = struct.unpack_from("<H", data, off + 4)[0]
            if typ == b"MAST":
                info.masters.append(data[off + 6:off + 6 + ln].rstrip(b"\0").decode("cp1252", "replace"))
            off += 6 + ln
    except (OSError, struct.error) as e:
        info.error = str(e)
    return info


class BethesdaGame(GameDef):
    deploy_root = "Data"
    has_plugins = True
    appdata_name = ""            # folder under AppData/Local holding plugins.txt
    mygames_name = ""            # folder under Documents/My Games
    implicit: list[str] = []     # always-loaded base plugins (never listed in plugins.txt)
    ccc_file = ""                # Creation Club list in the game folder
    notes = ("Plugin order is written to plugins.txt in the game's Proton prefix. Run the game "
             "once through Steam so the prefix exists. Script extenders (SKSE/F4SE) go in the "
             "game folder by hand.")

    def layout(self, root, mod_name):
        # Script extenders (SKSE/F4SE): loader exe + dlls go next to the game exe,
        # the Data folder (scripts) goes into Data.
        from .. import deploy
        for d in walk_dirs(root, 2):
            kids = list(d.iterdir())
            if any(c.is_file() and c.name.lower().endswith("_loader.exe") for c in kids):
                out = []
                for c in kids:
                    if c.is_dir() and c.name.lower() == "data":
                        out.append((c, ""))
                    elif c.is_file() and c.suffix.lower() in (".exe", ".dll"):
                        out.append((c, deploy.ROOT_PREFIX + c.name))
                return out
        d = dir_with_children(root, DATA_MARKERS | {"data"}, DATA_EXTS)
        if d is None:
            # Only DLLs (e.g. Engine Fixes' preloader: d3dx9_42.dll, tbb.dll): they belong
            # next to the game exe.
            for dd in walk_dirs(root, 2):
                dlls = [c for c in dd.iterdir() if c.is_file() and c.suffix.lower() == ".dll"]
                if dlls:
                    return [(c, deploy.ROOT_PREFIX + c.name) for c in dd.iterdir() if c.is_file()]
            return []
        data = ci_child(d, "data")
        # Archive wrapped everything in Data/ (sometimes with game-root files next to it).
        if data is not None and data.is_dir() and not any(
                c.name.lower() in DATA_MARKERS or c.suffix.lower() in DATA_EXTS
                for c in d.iterdir() if c != data):
            root_files = [c for c in d.iterdir() if c.is_file() and c.suffix.lower() in (".dll", ".exe")]
            return [(data, "")] + [(c, deploy.ROOT_PREFIX + c.name) for c in root_files]
        # DLLs/EXEs loose next to the Data folders belong in the game folder, not Data/
        loose = [c for c in d.iterdir() if c.is_file() and c.suffix.lower() in (".dll", ".exe")]
        if loose:
            return [(c, deploy.ROOT_PREFIX + c.name) for c in loose] + \
                [(c, c.name) for c in d.iterdir() if c not in loose]
        return [(d, "")]

    # -- plugins -----------------------------------------------------------
    def implicit_plugins(self, game_dir: Path) -> list[str]:
        out = list(self.implicit)
        ccc = ci_child(Path(game_dir), self.ccc_file) if self.ccc_file else None
        if ccc is not None and ccc.is_file():
            out += [l.strip() for l in ccc.read_text(errors="replace").splitlines() if l.strip()]
        return out

    def plugins_txt(self, inst: "Instance") -> Path | None:
        user = steam.prefix_user_dir(self.steam_appid, inst.game_dir)
        if not user:
            return None
        return user / "AppData/Local" / self.appdata_name / "plugins.txt"

    def read_plugins_txt(self, inst: "Instance") -> list[tuple[str, bool]]:
        p = self.plugins_txt(inst)
        if not p or not p.is_file():
            return []
        out = []
        for line in p.read_text(encoding="cp1252", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            out.append((line[1:], True) if line.startswith("*") else (line, False))
        return out

    def write_plugins_txt(self, inst: "Instance", plugins: list[tuple[str, bool]]) -> Path | None:
        p = self.plugins_txt(inst)
        if not p:
            return None
        p.parent.mkdir(parents=True, exist_ok=True)
        implicit = {n.lower() for n in self.implicit_plugins(inst.game_dir)}
        lines = ["# This file is used by the game to keep track of your downloaded content.",
                 "# Please do not modify this file. (Managed by Kiln)"]
        lines += [("*" if en else "") + name for name, en in plugins if name.lower() not in implicit]
        tmp = p.with_suffix(".tmp")
        tmp.write_bytes(("\r\n".join(lines) + "\r\n").encode("cp1252", "replace"))
        tmp.replace(p)
        return p

    def post_deploy(self, inst):
        warnings = []
        inst.sync_plugins()
        if self.plugins_txt(inst) is None:
            warnings.append(f"No Proton prefix found for {self.name}; run the game once through "
                            "Steam, then deploy again so plugins.txt can be written.")
        else:
            self.write_plugins_txt(inst, [(p["name"], p["enabled"]) for p in inst.plugins])
        warnings += inst.missing_master_warnings()
        return warnings


class SkyrimSE(BethesdaGame):
    id, name = "skyrimse", "Skyrim Special Edition"
    steam_appid, nexus_domain, nexus_game_id = 489830, "skyrimspecialedition", 1704
    exe = "SkyrimSE.exe"
    appdata_name = "Skyrim Special Edition"
    mygames_name = "Skyrim Special Edition"
    implicit = ["Skyrim.esm", "Update.esm", "Dawnguard.esm", "HearthFires.esm", "Dragonborn.esm"]
    ccc_file = "Skyrim.ccc"


class Fallout4(BethesdaGame):
    id, name = "fallout4", "Fallout 4"
    steam_appid, nexus_domain, nexus_game_id = 377160, "fallout4", 1151
    exe = "Fallout4.exe"
    appdata_name = "Fallout4"
    mygames_name = "Fallout4"
    implicit = ["Fallout4.esm", "DLCRobot.esm", "DLCworkshop01.esm", "DLCCoast.esm",
                "DLCworkshop02.esm", "DLCworkshop03.esm", "DLCNukaWorld.esm",
                "DLCUltraHighResolution.esm"]
    ccc_file = "Fallout4.ccc"

    def post_deploy(self, inst):
        warnings = super().post_deploy(inst)
        # Fallout 4 ignores loose files unless archive invalidation is on.
        user = steam.prefix_user_dir(self.steam_appid, inst.game_dir)
        if user:
            ini = user / "Documents/My Games/Fallout4/Fallout4Custom.ini"
            try:
                _ensure_ini(ini, "Archive", {"bInvalidateOlderFiles": "1",
                                             "sResourceDataDirsFinal": ""})
            except OSError as e:
                warnings.append(f"Couldn't update Fallout4Custom.ini: {e}")
        return warnings


def _ensure_ini(path: Path, section: str, values: dict[str, str]) -> None:
    """Set keys in one section of a Windows ini, keeping everything else."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines() if path.is_file() else []
    out, in_sec, seen_sec, done = [], False, False, set()
    for line in lines:
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            if in_sec:
                out += [f"{k}={v}" for k, v in values.items() if k not in done]
                done |= set(values)
            in_sec = s[1:-1].lower() == section.lower()
            seen_sec |= in_sec
        elif in_sec and "=" in s:
            k = s.split("=", 1)[0].strip()
            match = next((key for key in values if key.lower() == k.lower()), None)
            if match:
                out.append(f"{match}={values[match]}")
                done.add(match)
                continue
        out.append(line)
    if in_sec:
        out += [f"{k}={v}" for k, v in values.items() if k not in done]
    elif not seen_sec:
        out += ["", f"[{section}]"] + [f"{k}={v}" for k, v in values.items()]
    new = "\r\n".join(out).lstrip("\r\n") + "\r\n"
    if not path.is_file() or path.read_text(encoding="utf-8", errors="replace") != new:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(new, encoding="utf-8", newline="")
