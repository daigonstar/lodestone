"""The rest of Nexus's most popular games (by downloads). Each says where its mods
go and how to fit an archive into place. Detection trusts Steam: a game is found
by its (verified) Steam app id, so the exe names here only matter when you pick
a folder by hand."""
from __future__ import annotations

import json
import os
import re
import time
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import TYPE_CHECKING

from .. import steam
from ..archive import ci_child
from . import GameDef, Layout, dir_with_children, dirs_with_file, safe_name, walk_dirs
from .bethesda import BethesdaGame, _ensure_ini

if TYPE_CHECKING:
    from ..instance import Instance


class SteamGame(GameDef):
    """A game found through Steam by app id. exes: any of these marks a folder you pick by hand."""
    exes: tuple[str, ...] = ()
    trust_steam = True
    alt_appids: tuple[int, ...] = ()

    def validate(self, game_dir):
        g = Path(game_dir)
        if self.exes:
            return any((g / e).is_file() for e in self.exes)
        return g.is_dir() and any(g.iterdir())


def _files(root: Path):
    return [f for f in root.rglob("*") if f.is_file()]


def _covered(p: Path, out: Layout) -> bool:
    return any(s == p or s in p.parents for s, _ in out)


# ====================================================================== Bethesda (classic)
class ClassicBethesda(BethesdaGame, SteamGame):
    """Oblivion, Fallout 3, New Vegas and Skyrim (2011): plugins.txt lists only the
    active plugins (no '*'), and load order comes from file dates."""
    by_dates = True
    loadorder_txt = False

    def validate(self, game_dir):
        return SteamGame.validate(self, game_dir)

    def read_plugins_txt(self, inst):
        p = self.plugins_txt(inst)
        if not p or not p.is_file():
            return []
        return [(ln.strip(), True) for ln in p.read_text(encoding="cp1252", errors="replace").splitlines()
                if ln.strip() and not ln.startswith("#")]

    def write_plugins_txt(self, inst, plugins):
        p = self.plugins_txt(inst)
        if not p:
            return None
        p.parent.mkdir(parents=True, exist_ok=True)
        implicit = [n for n in self.implicit_plugins(inst.game_dir)]
        active = implicit + [n for n, en in plugins if en and n.lower() not in {i.lower() for i in implicit}]
        p.write_bytes(("\r\n".join(active) + "\r\n").encode("cp1252", "replace"))
        order = implicit + [n for n, _ in plugins if n.lower() not in {i.lower() for i in implicit}]
        if self.loadorder_txt:
            p.with_name("loadorder.txt").write_bytes(("\r\n".join(order) + "\r\n").encode("cp1252", "replace"))
        if self.by_dates:
            _date_order(inst.deploy_dir, order)
        return p


def _date_order(data: Path, order: list[str]) -> None:
    """Older engines load plugins oldest first: give them dates in the wanted order."""
    base = time.time() - 86400 * 365 * 5
    for i, name in enumerate(order):
        f = ci_child(data, name)
        if f is not None and f.is_file():
            try:
                os.utime(f, (base + i * 60, base + i * 60))
            except OSError:
                pass


class SkyrimLE(ClassicBethesda):
    id, name = "skyrim", "Skyrim (2011)"
    steam_appid, nexus_domain, nexus_game_id = 72850, "skyrim", 110
    exe, exes = "TESV.exe", ("TESV.exe",)
    appdata_name = mygames_name = "Skyrim"
    implicit = ["Skyrim.esm", "Update.esm"]
    by_dates, loadorder_txt = False, True


class FalloutNV(ClassicBethesda):
    id, name = "newvegas", "Fallout: New Vegas"
    steam_appid, nexus_domain, nexus_game_id = 22380, "newvegas", 130
    exe, exes = "FalloutNV.exe", ("FalloutNV.exe",)
    appdata_name = mygames_name = "FalloutNV"
    implicit = ["FalloutNV.esm"]


class Fallout3(ClassicBethesda):
    id, name = "fallout3", "Fallout 3"
    steam_appid, nexus_domain, nexus_game_id = 22370, "fallout3", 120
    alt_appids = (22300,)
    exe, exes = "Fallout3.exe", ("Fallout3.exe",)
    appdata_name = mygames_name = "Fallout3"
    implicit = ["Fallout3.esm"]


class Oblivion(ClassicBethesda):
    id, name = "oblivion", "Oblivion"
    steam_appid, nexus_domain, nexus_game_id = 22330, "oblivion", 101
    exe, exes = "Oblivion.exe", ("Oblivion.exe",)
    appdata_name = mygames_name = "Oblivion"
    implicit = ["Oblivion.esm"]


class Morrowind(ClassicBethesda):
    id, name = "morrowind", "Morrowind"
    steam_appid, nexus_domain, nexus_game_id = 22320, "morrowind", 100
    exe, exes = "Morrowind.exe", ("Morrowind.exe",)
    deploy_root = "Data Files"
    implicit = []
    notes = "Active plugins are written to Morrowind.ini; load order follows file dates, as the game expects."

    def layout(self, root, mod_name):
        for d in walk_dirs(root, 3):
            df = ci_child(d, "Data Files")
            if df is not None and df.is_dir():
                return [(df, "")]
        return super().layout(root, mod_name)

    def plugins_txt(self, inst):
        return inst.game_dir / "Morrowind.ini"

    def read_plugins_txt(self, inst):
        ini = self.plugins_txt(inst)
        if not ini.is_file():
            return []
        return [(m.group(1).strip(), True) for m in
                re.finditer(r"(?im)^GameFile\d+=(.+)$", ini.read_text(encoding="cp1252", errors="replace"))]

    def write_plugins_txt(self, inst, plugins):
        ini = self.plugins_txt(inst)
        if not ini.is_file():
            return None
        text = ini.read_text(encoding="cp1252", errors="replace")
        active = [n for n, en in plugins if en]
        masters = [n for n in active if n.lower().endswith(".esm")]
        active = masters + [n for n in active if n not in masters]
        block = "[Game Files]\r\n" + "".join(f"GameFile{i}={n}\r\n" for i, n in enumerate(active))
        if re.search(r"(?im)^\[Game Files\]", text):
            text = re.sub(r"(?ims)^\[Game Files\]\s*\n(?:GameFile\d+=.*?\n)*", block, text, count=1)
        else:
            text = text.rstrip("\r\n") + "\r\n\r\n" + block
        ini.write_bytes(text.encode("cp1252", "replace"))
        _date_order(inst.deploy_dir, [n for n, _ in plugins])
        return ini


class Starfield(BethesdaGame, SteamGame):
    id, name = "starfield", "Starfield"
    steam_appid, nexus_domain, nexus_game_id = 1716740, "starfield", 4187
    exe, exes = "Starfield.exe", ("Starfield.exe",)
    appdata_name = mygames_name = "Starfield"
    implicit = ["Starfield.esm", "Constellation.esm", "OldMars.esm", "BlueprintShips-Starfield.esm",
                "SFBGS003.esm", "SFBGS004.esm", "SFBGS006.esm", "SFBGS007.esm", "SFBGS008.esm"]
    ccc_file = "Starfield.ccc"

    def validate(self, game_dir):
        return SteamGame.validate(self, game_dir)

    def post_deploy(self, inst):
        warnings = super().post_deploy(inst)
        user = steam.prefix_user_dir(self.steam_appid, inst.game_dir)
        if user:
            try:   # loose files in the game's Data folder only load with these set
                _ensure_ini(user / "Documents/My Games/Starfield/StarfieldCustom.ini", "Archive",
                            {"bInvalidateOlderFiles": "1", "sResourceDataDirsFinal": ""})
            except OSError as e:
                warnings.append(f"Couldn't update StarfieldCustom.ini: {e}")
        return warnings


# ====================================================================== Unreal Engine
PAK_EXTS = {".pak", ".utoc", ".ucas", ".sig"}


class UnrealGame(SteamGame):
    """Pak mods go to <Project>/Content/Paks/~mods; UE4SS mods to
    <Project>/Binaries/Win64/ue4ss/Mods."""
    project = ""

    def validate(self, game_dir):
        return (Path(game_dir) / self.project).is_dir() or SteamGame.validate(self, game_dir)

    def layout(self, root, mod_name):
        proj = self.project.lower()
        d = dir_with_children(root, {proj})
        if d is not None:
            return [(d, "")]
        d = dir_with_children(root, {"content", "binaries"})
        if d is not None and ci_child(d, "Content") is not None:
            return [(d, self.project)]
        out: Layout = []
        for d in walk_dirs(root, 4):          # UE4SS mods: <Name>/Scripts/main.lua or <Name>/dlls/main.dll
            if _covered(d, out) or d == root:
                continue
            scripts, dlls = ci_child(d, "Scripts"), ci_child(d, "dlls")
            if (scripts is not None and ci_child(scripts, "main.lua") is not None) or \
                    (dlls is not None and ci_child(dlls, "main.dll") is not None):
                out.append((d, f"{self.project}/Binaries/Win64/ue4ss/Mods/{d.name}"))
        for f in _files(root):
            if _covered(f, out):
                continue
            if f.suffix.lower() in PAK_EXTS:
                out.append((f, f"{self.project}/Content/Paks/~mods/{f.name}"))
            elif f.name.lower() in ("ue4ss.dll", "dwmapi.dll", "ue4ss-settings.ini", "xinput1_3.dll"):
                out.append((f.parent, f"{self.project}/Binaries/Win64"))
        return out


class Palworld(UnrealGame):
    id, name = "palworld", "Palworld"
    steam_appid, nexus_domain, nexus_game_id = 1623730, "palworld", 6063
    exe, exes, project = "Palworld.exe", ("Palworld.exe",), "Pal"
    notes = "Lua and DLL mods need the Palworld build of UE4SS; pak mods work on their own."


class StellarBlade(UnrealGame):
    id, name = "stellarblade", "Stellar Blade"
    steam_appid, nexus_domain, nexus_game_id = 3489700, "stellarblade", 7804
    exe, project = "SB.exe", "SB"


class ReadyOrNot(UnrealGame):
    id, name = "readyornot", "Ready or Not"
    steam_appid, nexus_domain, nexus_game_id = 1144200, "readyornot", 4205
    exe, exes, project = "ReadyOrNot.exe", ("ReadyOrNot.exe",), "ReadyOrNot"


class Stalker2(UnrealGame):
    id, name = "stalker2heartofchornobyl", "S.T.A.L.K.E.R. 2"
    steam_appid, nexus_domain, nexus_game_id = 1643320, "stalker2heartofchornobyl", 6944
    exe, exes, project = "Stalker2.exe", ("Stalker2.exe",), "Stalker2"


class OblivionRemastered(UnrealGame):
    """Unreal on the outside, Oblivion inside: plugins (.esp/.esm/.bsa) go to
    ObvData/Data and are listed in the Plugins.txt there; paks go to ~mods."""
    id, name = "oblivionremastered", "Oblivion Remastered"
    steam_appid, nexus_domain, nexus_game_id = 2623190, "oblivionremastered", 7587
    exe, exes, project = "OblivionRemastered.exe", ("OblivionRemastered.exe",), "OblivionRemastered"
    DATA = "OblivionRemastered/Content/Dev/ObvData/Data"
    notes = "Plugins are added to Plugins.txt in ObvData/Data; Kiln keeps the game's own entries."

    def layout(self, root, mod_name):
        d = dir_with_children(root, {self.project.lower()})
        if d is not None:
            return [(d, "")]
        out: Layout = []
        for f in _files(root):
            ext = f.suffix.lower()
            if ext in (".esp", ".esm", ".bsa"):
                out.append((f, f"{self.DATA}/{f.name}"))
            elif ext in PAK_EXTS:
                out.append((f, f"{self.project}/Content/Paks/~mods/{f.name}"))
            elif f.name.lower().startswith("obse64") and ext in (".exe", ".dll"):
                out.append((f, f"{self.project}/Binaries/Win64/{f.name}"))
        if any(dst.startswith(self.DATA) for _, dst in out):   # loose files next to the plugins
            for sub in ("meshes", "textures", "sound", "obse"):
                for dd in walk_dirs(root, 3):
                    c = ci_child(dd, sub)
                    if c is not None and c.is_dir() and not _covered(c, out):
                        out.append((c, f"{self.DATA}/{c.name}"))
        return out or super().layout(root, mod_name)

    def post_deploy(self, inst):
        data = inst.game_dir / self.DATA
        ptxt = data / "Plugins.txt"
        if not ptxt.is_file():
            return []
        mine_f = inst.staging / "plugins-added.json"
        try:
            added = set(json.loads(mine_f.read_text()))
        except (OSError, ValueError):
            added = set()
        lines = [l.strip() for l in ptxt.read_text(encoding="utf-8", errors="replace").splitlines() if l.strip()]
        present = {p.name.lower(): p.name for p in data.iterdir() if p.suffix.lower() in (".esp", ".esm")}
        keep = [l for l in lines if not (l.lower() in added and l.lower() not in present)]
        listed = {l.lower() for l in keep}
        new = [present[k] for k in sorted(present) if k not in listed]
        ptxt.write_text("\n".join(keep + new) + "\n", encoding="utf-8")
        mine_f.write_text(json.dumps(sorted((added | {n.lower() for n in new}) & set(present))))
        return []


# ====================================================================== RE Engine
class REEngineGame(SteamGame):
    """Resident Evil / Monster Hunter Rise & Wilds: REFramework plus loose files under natives/."""
    notes = ("Loose-file mods (natives/) need REFramework. Mods made for Fluffy Mod Manager "
             "that patch .pak files aren't supported yet.")

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"natives", "reframework", "dinput8.dll"})
        if d is not None:
            return [(d, "")]
        return []


class RE4(REEngineGame):
    id, name = "residentevil42023", "Resident Evil 4 (2023)"
    steam_appid, nexus_domain, nexus_game_id = 2050650, "residentevil42023", 5195
    exe, exes = "re4.exe", ("re4.exe",)


class RE2(REEngineGame):
    id, name = "residentevil22019", "Resident Evil 2 (2019)"
    steam_appid, nexus_domain, nexus_game_id = 883710, "residentevil22019", 2702
    exe, exes = "re2.exe", ("re2.exe",)


class MHRise(REEngineGame):
    id, name = "monsterhunterrise", "Monster Hunter Rise"
    steam_appid, nexus_domain, nexus_game_id = 1446780, "monsterhunterrise", 4095
    exe, exes = "MonsterHunterRise.exe", ("MonsterHunterRise.exe",)


class MHWilds(REEngineGame):
    id, name = "monsterhunterwilds", "Monster Hunter Wilds"
    steam_appid, nexus_domain, nexus_game_id = 2246340, "monsterhunterwilds", 6993
    exe, exes = "MonsterHunterWilds.exe", ("MonsterHunterWilds.exe",)


class MHWorld(SteamGame):
    id, name = "monsterhunterworld", "Monster Hunter: World"
    steam_appid, nexus_domain, nexus_game_id = 582010, "monsterhunterworld", 2531
    exe, exes = "MonsterHunterWorld.exe", ("MonsterHunterWorld.exe",)
    notes = "Mods go into nativePC and need Stracker's Loader."

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"nativepc", "dinput8.dll", "loader.dll"})
        return [(d, "")] if d is not None else []


# ====================================================================== root/loader games
class EldenRing(SteamGame):
    id, name = "eldenring", "Elden Ring"
    steam_appid, nexus_domain, nexus_game_id = 1245620, "eldenring", 4333
    exe, exes = "Game/eldenring.exe", ("Game/eldenring.exe",)
    deploy_root = "Game"
    notes = ("DLL mods load through Elden Mod Loader (Game/mods). Modded play must be offline: "
             "start the game with Easy Anti-Cheat off, or you risk a ban online. Mods that "
             "replace game files need Mod Engine 2, which Kiln doesn't run yet.")

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"mods", "dinput8.dll"})
        if d is not None:
            return [(d, "")]
        dlls = [f for f in _files(root) if f.suffix.lower() == ".dll"]
        if dlls:
            return [(dlls[0].parent, "mods")]
        return []


class RDR2(SteamGame):
    id, name = "reddeadredemption2", "Red Dead Redemption 2"
    steam_appid, nexus_domain, nexus_game_id = 1174180, "reddeadredemption2", 3024
    exe, exes = "RDR2.exe", ("RDR2.exe",)
    notes = ("Script mods need Script Hook RDR2; replacement mods need Lenny's Mod Loader. "
             "Never take a modded game into Red Dead Online: Rockstar bans for it.")

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"lml", "scripts", "dinput8.dll", "scripthookrdr2.dll"}, {".asi"})
        if d is not None:
            return [(d, "")]
        return [(d, f"lml/{d.name}") for d in dirs_with_file(root, "install.xml")]


class Bannerlord(SteamGame):
    id, name = "mountandblade2bannerlord", "Mount & Blade II: Bannerlord"
    steam_appid, nexus_domain, nexus_game_id = 261550, "mountandblade2bannerlord", 3174
    exe = "bin/Win64_Shipping_Client/Bannerlord.exe"
    exes = (exe,)
    deploy_root = "Modules"
    notes = "Turn mods on and set their order in the game's launcher (Mods tab)."

    def layout(self, root, mod_name):
        return [(d, d.name) for d in dirs_with_file(root, "SubModule.xml")]


class BladeAndSorcery(SteamGame):
    id, name = "bladeandsorcery", "Blade & Sorcery"
    steam_appid, nexus_domain, nexus_game_id = 629730, "bladeandsorcery", 2673
    exe, exes = "BladeAndSorcery.exe", ("BladeAndSorcery.exe",)
    deploy_root = "BladeAndSorcery_Data/StreamingAssets/Mods"

    def layout(self, root, mod_name):
        return [(d, d.name) for d in dirs_with_file(root, "manifest.json")]


class BepInExGame(SteamGame):
    """Unity games modded with BepInEx: plugins in BepInEx/plugins/<mod>."""
    notes = "Mods need BepInEx (install it like a mod first)."

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"bepinex", "winhttp.dll", "doorstop_config.ini"})
        if d is not None:
            return [(d, "")]
        d = dir_with_children(root, {"plugins", "patchers", "config"})
        if d is not None and ci_child(d, "plugins") is not None:
            return [(d, "BepInEx")]
        dlls = [f for f in _files(root) if f.suffix.lower() == ".dll"]
        if dlls:
            base = min((f.parent for f in dlls), key=lambda p: len(p.parts))
            return [(base, f"BepInEx/plugins/{safe_name(mod_name if base == root else base.name)}")]
        return []


class Subnautica(BepInExGame):
    id, name = "subnautica", "Subnautica"
    steam_appid, nexus_domain, nexus_game_id = 264710, "subnautica", 1155
    exe, exes = "Subnautica.exe", ("Subnautica.exe",)

    def layout(self, root, mod_name):
        qm = dirs_with_file(root, "mod.json")
        if qm and not dir_with_children(root, {"bepinex"}):
            return [(d, f"QMods/{d.name}") for d in qm]
        return super().layout(root, mod_name)


class Valheim(BepInExGame):
    id, name = "valheim", "Valheim"
    steam_appid, nexus_domain, nexus_game_id = 892970, "valheim", 3667
    exes = ("valheim.exe", "valheim.x86_64")
    exe = "valheim.exe"
    notes = ("Mods need BepInEx. Valheim has a native Linux version; Windows BepInEx packs "
             "need the game run through Proton (Steam → Properties → Compatibility).")


class TCGCardShop(BepInExGame):
    id, name = "tcgcardshopsimulator", "TCG Card Shop Simulator"
    steam_appid, nexus_domain, nexus_game_id = 3070070, "tcgcardshopsimulator", 6826
    exe = "Card Shop Simulator.exe"


class MySummerCar(SteamGame):
    id, name = "mysummercar", "My Summer Car"
    steam_appid, nexus_domain, nexus_game_id = 516750, "mysummercar", 2600
    exe, exes = "mysummercar.exe", ("mysummercar.exe",)
    deploy_root = "Mods"
    notes = "Mods need MSCLoader."

    def layout(self, root, mod_name):
        d = dir_with_children(root, {"assets", "references"}, {".dll"})
        return [(d, "")] if d is not None else []


class KCDBase(SteamGame):
    deploy_root = "Mods"

    def layout(self, root, mod_name):
        found = dirs_with_file(root, "mod.manifest")
        if found:
            return [(d, d.name) for d in found]
        paks = [f for f in _files(root) if f.suffix.lower() == ".pak"]
        if paks:
            return [(f, f"{safe_name(mod_name)}/Data/{f.name}") for f in paks]
        return []


class KCD(KCDBase):
    id, name = "kingdomcomedeliverance", "Kingdom Come: Deliverance"
    steam_appid, nexus_domain, nexus_game_id = 379430, "kingdomcomedeliverance", 2298
    exe, exes = "Bin/Win64/KingdomCome.exe", ("Bin/Win64/KingdomCome.exe",)


class KCD2(KCDBase):
    id, name = "kingdomcomedeliverance2", "Kingdom Come: Deliverance II"
    steam_appid, nexus_domain, nexus_game_id = 1771300, "kingdomcomedeliverance2", 7286
    exe = "Bin/Win64MasterMasterSteamPGO/KingdomCome.exe"


# ====================================================================== mods in the Proton prefix
class PrefixGame(SteamGame):
    """Games whose mods live in the user's folders inside the Proton prefix
    (Documents, AppData). The game must have been started once so the prefix exists."""
    prefix_deploy = ""          # relative to the prefix's steamuser folder

    def deploy_dir(self, inst: "Instance") -> Path:
        u = steam.prefix_user_dir(self.steam_appid, inst.game_dir)
        if u is None:   # where it will be once the game has run (deploying waits for that)
            lib = steam.library_of(inst.game_dir) or inst.game_dir.parent
            u = lib / "steamapps/compatdata" / str(self.steam_appid) / "pfx/drive_c/users/steamuser"
        return u / self.prefix_deploy

    def not_ready(self, inst: "Instance") -> str:
        if steam.prefix_user_dir(self.steam_appid, inst.game_dir) is None:
            return (f"{self.name}'s mods go in its Proton folder, which doesn't exist yet. Start "
                    "the game once through Steam, quit, then deploy again.")
        return ""


class Sims4(PrefixGame):
    id, name = "thesims4", "The Sims 4"
    steam_appid, nexus_domain, nexus_game_id = 1222670, "thesims4", 641
    exe, exes = "Game/Bin/TS4_x64.exe", ("Game/Bin/TS4_x64.exe",)
    prefix_deploy = "Documents/Electronic Arts/The Sims 4/Mods"
    notes = "Turn on custom content and script mods in the game's options (Game Options → Other)."

    def layout(self, root, mod_name):
        out = [(f, f"{safe_name(mod_name)}/{f.name}") for f in _files(root)
               if f.suffix.lower() in (".package", ".ts4script", ".cfg")]
        return out


class DragonAgeBase(PrefixGame):
    notes = ("Override mods go to packages/core/override. .dazip mods need the game's own "
             "DAUpdater and aren't handled by Kiln yet.")

    def layout(self, root, mod_name):
        for d in walk_dirs(root, 3):
            ov = ci_child(d, "override")
            if ov is not None and ov.is_dir():
                return [(ov, safe_name(mod_name))]
        from ..archive import unwrap
        top = unwrap(root)
        return [(top, safe_name(mod_name))] if any(_files(top)) else []


class DragonAgeOrigins(DragonAgeBase):
    id, name = "dragonage", "Dragon Age: Origins"
    steam_appid, nexus_domain, nexus_game_id = 47810, "dragonage", 140
    alt_appids = (17450,)
    exe, exes = "bin_ship/daorigins.exe", ("bin_ship/daorigins.exe",)
    prefix_deploy = "Documents/BioWare/Dragon Age/packages/core/override"


class DragonAge2(DragonAgeBase):
    id, name = "dragonage2", "Dragon Age II"
    steam_appid, nexus_domain, nexus_game_id = 1238040, "dragonage2", 141
    exe, exes = "bin_ship/DragonAge2.exe", ("bin_ship/DragonAge2.exe",)
    prefix_deploy = "Documents/BioWare/Dragon Age 2/packages/core/override"


BG3_SETTINGS = "AppData/Local/Larian Studios/Baldur's Gate 3/PlayerProfiles/Public/modsettings.lsx"


class BaldursGate3(PrefixGame):
    id, name = "baldursgate3", "Baldur's Gate 3"
    steam_appid, nexus_domain, nexus_game_id = 1086940, "baldursgate3", 3474
    exe, exes = "bin/bg3.exe", ("bin/bg3.exe", "bin/bg3_dx11.exe")
    prefix_deploy = "AppData/Local/Larian Studios/Baldur's Gate 3/Mods"
    notes = ("Pak mods go to the Mods folder. Kiln adds those that include an info.json to "
             "modsettings.lsx (backed up first). Script Extender and loose-file mods are installed "
             "in the game folder by hand for now.")

    def layout(self, root, mod_name):
        out: Layout = [(f, f.name) for f in _files(root) if f.suffix.lower() == ".pak"]
        info = next((f for f in _files(root) if f.name.lower() == "info.json"), None)
        if out and info is not None:          # BG3 Mod Manager metadata, used for modsettings.lsx
            out.append((info, f"{Path(out[0][1]).stem}.kiln-info.json"))
        return out

    def post_deploy(self, inst):
        u = steam.prefix_user_dir(self.steam_appid, inst.game_dir)
        if not u:
            return []
        lsx = u / BG3_SETTINGS
        mods = self.deploy_dir(inst)
        if not lsx.is_file() or not mods or not mods.is_dir():
            return ["Start Baldur's Gate 3 once so it creates modsettings.lsx, then deploy again."]
        infos = []
        for f in mods.glob("*.kiln-info.json"):
            try:
                d = json.loads(f.read_text(encoding="utf-8-sig"))
            except (OSError, ValueError):
                continue
            for m in d.get("Mods") or []:
                if m.get("UUID") and m.get("Folder"):
                    infos.append(m)
        return _bg3_settings(inst, lsx, infos)


def _bg3_settings(inst, lsx: Path, infos: list[dict]) -> list[str]:
    """Add the deployed mods to modsettings.lsx (and take out ones Kiln added that are
    gone). The game's own entries and anything another tool added are left alone."""
    state_f = inst.staging / "bg3-added.json"
    try:
        added = set(json.loads(state_f.read_text()))
    except (OSError, ValueError):
        added = set()
    try:
        tree = ET.parse(lsx)
    except ET.ParseError as e:
        return [f"modsettings.lsx couldn't be read ({e}); Kiln left it alone."]
    root = tree.getroot()
    mods_node = root.find(".//node[@id='Mods']/children")
    if mods_node is None:
        return ["modsettings.lsx has an unexpected layout; Kiln left it alone."]
    order_node = root.find(".//node[@id='ModOrder']/children")
    want = {m["UUID"]: m for m in infos}

    def uuid_of(n):
        a = n.find("attribute[@id='UUID']")
        return a.get("value") if a is not None else ""
    changed = False
    for parent in [p for p in (mods_node, order_node) if p is not None]:
        for n in list(parent):
            u = uuid_of(n)
            if u in added and u not in want:
                parent.remove(n)
                changed = True
    have = {uuid_of(n) for n in mods_node}
    for u, m in want.items():
        if u in have:
            continue
        n = ET.SubElement(mods_node, "node", id="ModuleShortDesc")
        for k, t, v in (("Folder", "LSString", m.get("Folder", "")), ("MD5", "LSString", m.get("MD5", "")),
                        ("Name", "LSString", m.get("Name", "")), ("PublishHandle", "uint64", "0"),
                        ("UUID", "guid", u), ("Version64", "int64", str(m.get("Version") or m.get("Version64") or 0))):
            ET.SubElement(n, "attribute", id=k, type=t, value=str(v))
        if order_node is not None:
            o = ET.SubElement(order_node, "node", id="Module")
            ET.SubElement(o, "attribute", id="UUID", type="FixedString", value=u)
        changed = True
    if changed:
        bak = lsx.with_name("modsettings.lsx.kiln-backup")
        if not bak.exists():
            bak.write_bytes(lsx.read_bytes())
        tree.write(lsx, encoding="utf-8", xml_declaration=True)
    state_f.write_text(json.dumps(sorted(set(want) | (added & set(want)))))
    return []


def more_games() -> list[GameDef]:
    """In Nexus popularity order (downloads), after the games Kiln started with."""
    return [SkyrimLE(), FalloutNV(), BaldursGate3(), Oblivion(), Starfield(), Fallout3(), Bannerlord(),
            Morrowind(), MHWorld(), BladeAndSorcery(), DragonAgeOrigins(), RDR2(), StellarBlade(),
            EldenRing(), ReadyOrNot(), RE4(), Palworld(), MHWilds(), Subnautica(), OblivionRemastered(),
            MHRise(), MySummerCar(), Sims4(), DragonAge2(), Valheim(), TCGCardShop(), Stalker2(),
            KCD(), RE2(), KCD2()]
