"""Hermetic test of the 30 added games: each is found through a fake Steam
library, a typical mod archive for it is installed and deployed, and the files
must land where that game loads them. Also checks plugins.txt / Morrowind.ini /
Plugins.txt / modsettings.lsx handling, and that the window lists them.
Usage: python3 tests/smoke_games.py <empty work dir>"""
import os, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu", XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, deploy, launchguard
lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
launchguard.game_running = lambda inst: False
from kiln.games import all_games, by_id
from kiln.games.more import more_games
from kiln.instance import Instance

TES4 = b"TES4" + b"\0" * 20
PREFIX_GAMES = {"thesims4", "dragonage", "dragonage2", "baldursgate3"}
NO_PREFIX = "dragonage2"          # this one hasn't been started yet: deploying must say so


def install_game(g, appid=None):
    appid = appid or g.steam_appid
    d = lib / "steamapps/common" / g.name.replace(":", "")
    (lib / "steamapps").mkdir(parents=True, exist_ok=True)
    (lib / f"steamapps/appmanifest_{appid}.acf").write_text(
        f'"AppState" {{ "appid" "{appid}" "installdir" "{d.name}" }}')
    exe = d / g.exe
    exe.parent.mkdir(parents=True, exist_ok=True)
    exe.write_bytes(b"MZ")
    if g.id != NO_PREFIX:
        (lib / f"steamapps/compatdata/{appid}/pfx/drive_c/users/steamuser").mkdir(parents=True, exist_ok=True)
    return d


def user(g):
    return lib / f"steamapps/compatdata/{g.steam_appid}/pfx/drive_c/users/steamuser"


def zip_of(name, files):
    z = work / "archives" / f"{name}.zip"
    z.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(z, "w") as zz:
        for n, data in files.items():
            zz.writestr(n, data)
    return z


# game id -> (archive files, [paths that must exist after deploy, relative to the game folder;
#             "~/" means inside the Proton prefix user folder])
CASES = {
    "skyrim": ({"Data/meshes/a.nif": "n", "Data/Cool.esp": TES4}, ["Data/meshes/a.nif", "Data/Cool.esp"]),
    "newvegas": ({"Cool.esp": TES4, "textures/x.dds": "t"}, ["Data/Cool.esp", "Data/textures/x.dds"]),
    "baldursgate3": ({"Cool/Cool.pak": "p", "Cool/info.json":
                      '{"Mods":[{"Author":"a","Name":"Cool","Folder":"Cool_abc","Version":"36028797018963968",'
                      '"UUID":"11111111-2222-3333-4444-555555555555","MD5":"x"}]}'},
                     ["~/AppData/Local/Larian Studios/Baldur's Gate 3/Mods/Cool.pak"]),
    "oblivion": ({"Data/Cool.esp": TES4}, ["Data/Cool.esp"]),
    "starfield": ({"Cool.esm": TES4}, ["Data/Cool.esm"]),
    "fallout3": ({"Cool.esp": TES4}, ["Data/Cool.esp"]),
    "mountandblade2bannerlord": ({"CoolMod/SubModule.xml": "<x/>", "CoolMod/bin/a.dll": "d"},
                                 ["Modules/CoolMod/SubModule.xml", "Modules/CoolMod/bin/a.dll"]),
    "morrowind": ({"Data Files/Cool.esp": "TES3"}, ["Data Files/Cool.esp"]),
    "monsterhunterworld": ({"nativePC/wp/x.mod3": "m"}, ["nativePC/wp/x.mod3"]),
    "bladeandsorcery": ({"Cool/manifest.json": "{}", "Cool/a.bundle": "b"},
                        ["BladeAndSorcery_Data/StreamingAssets/Mods/Cool/manifest.json"]),
    "dragonage": ({"override/cool.erf": "e"}, ["~/Documents/BioWare/Dragon Age/packages/core/override/Cool/cool.erf"]),
    "reddeadredemption2": ({"Cool/install.xml": "<x/>", "Cool/a.ytd": "y"}, ["lml/Cool/install.xml"]),
    "stellarblade": ({"Cool_P.pak": "p", "Cool_P.utoc": "u"}, ["SB/Content/Paks/~mods/Cool_P.pak", "SB/Content/Paks/~mods/Cool_P.utoc"]),
    "eldenring": ({"cool.dll": "d", "cool.ini": "i"}, ["Game/mods/cool.dll", "Game/mods/cool.ini"]),
    "readyornot": ({"pakchunk99-Cool.pak": "p"}, ["ReadyOrNot/Content/Paks/~mods/pakchunk99-Cool.pak"]),
    "residentevil42023": ({"Cool/natives/STM/a.tex": "t", "Cool/modinfo.ini": "i"}, ["natives/STM/a.tex"]),
    "palworld": ({"Cool.pak": "p", "LuaMod/Scripts/main.lua": "l"},
                 ["Pal/Content/Paks/~mods/Cool.pak", "Pal/Binaries/Win64/ue4ss/Mods/LuaMod/Scripts/main.lua"]),
    "monsterhunterwilds": ({"reframework/plugins/a.dll": "d"}, ["reframework/plugins/a.dll"]),
    "subnautica": ({"BepInEx/plugins/Cool/Cool.dll": "d"}, ["BepInEx/plugins/Cool/Cool.dll"]),
    "oblivionremastered": ({"Cool.esp": TES4, "Cool_P.pak": "p"},
                           ["OblivionRemastered/Content/Dev/ObvData/Data/Cool.esp",
                            "OblivionRemastered/Content/Paks/~mods/Cool_P.pak"]),
    "monsterhunterrise": ({"natives/STM/a.tex": "t"}, ["natives/STM/a.tex"]),
    "mysummercar": ({"Cool.dll": "d", "Assets/Cool/a.unity3d": "a"}, ["Mods/Cool.dll", "Mods/Assets/Cool/a.unity3d"]),
    "thesims4": ({"Stuff/a.package": "p", "Stuff/deep/b.ts4script": "s"},
                 ["~/Documents/Electronic Arts/The Sims 4/Mods/Cool/a.package",
                  "~/Documents/Electronic Arts/The Sims 4/Mods/Cool/b.ts4script"]),
    "dragonage2": ({"override/cool.erf": "e"}, []),
    "valheim": ({"Cool.dll": "d"}, ["BepInEx/plugins/Cool/Cool.dll"]),
    "tcgcardshopsimulator": ({"plugins/Cool.dll": "d"}, ["BepInEx/plugins/Cool.dll"]),
    "stalker2heartofchornobyl": ({"Cool_P.pak": "p"}, ["Stalker2/Content/Paks/~mods/Cool_P.pak"]),
    "kingdomcomedeliverance": ({"Cool/mod.manifest": "<x/>", "Cool/Data/cool.pak": "p"},
                               ["Mods/Cool/mod.manifest", "Mods/Cool/Data/cool.pak"]),
    "residentevil22019": ({"Cool/natives/x64/a.tex": "t"}, ["natives/x64/a.tex"]),
    "kingdomcomedeliverance2": ({"cool.pak": "p"}, ["Mods/Cool/Data/cool.pak"]),
}
games = more_games()
assert len(games) == 30 and set(CASES) == {g.id for g in games}, set(CASES) ^ {g.id for g in games}

# a few files the games themselves would have
fixtures = {}
for g in games:
    appid = 22300 if g.id == "fallout3" else None      # the non-GOTY edition: found via its other app id
    gd = install_game(g, appid)
    fixtures[g.id] = gd
(fixtures["morrowind"] / "Data Files").mkdir(exist_ok=True)
(fixtures["morrowind"] / "Data Files/Morrowind.esm").write_text("TES3")
(fixtures["morrowind"] / "Morrowind.ini").write_text("[General]\r\nx=1\r\n\r\n[Game Files]\r\nGameFile0=Morrowind.esm\r\n")
for gid, esm in (("skyrim", "Skyrim.esm"), ("newvegas", "FalloutNV.esm"), ("oblivion", "Oblivion.esm"),
                 ("starfield", "Starfield.esm")):
    (fixtures[gid] / "Data").mkdir(exist_ok=True)
    (fixtures[gid] / "Data" / esm).write_bytes(TES4)
obv = fixtures["oblivionremastered"] / "OblivionRemastered/Content/Dev/ObvData/Data"
obv.mkdir(parents=True)
(obv / "Oblivion.esm").write_bytes(TES4)
(obv / "Plugins.txt").write_text("Oblivion.esm\nDLCBattlehornCastle.esp\n")
bg3 = by_id("baldursgate3")
lsx = user(bg3) / "AppData/Local/Larian Studios/Baldur's Gate 3/PlayerProfiles/Public/modsettings.lsx"
lsx.parent.mkdir(parents=True)
lsx.write_text('<?xml version="1.0" encoding="UTF-8"?>\n<save><version major="4" minor="7" revision="1" build="3"/>'
               '<region id="ModuleSettings"><node id="root"><children><node id="Mods"><children>'
               '<node id="ModuleShortDesc"><attribute id="Folder" type="LSString" value="GustavX"/>'
               '<attribute id="UUID" type="guid" value="cb555efe-2d9e-131f-8195-a89329d218ea"/></node>'
               '</children></node></children></node></region></save>')

# ---- the window: finds all 30 (plus the originals aren't installed here)
from PySide6.QtCore import QCoreApplication, Qt
QCoreApplication.setAttribute(Qt.AA_ShareOpenGLContexts)
from kiln import webview  # noqa
from PySide6.QtWidgets import QApplication, QMessageBox
app = QApplication([])
QMessageBox.question = staticmethod(lambda *a, **k: QMessageBox.Yes)
QMessageBox.information = staticmethod(lambda *a, **k: None)
from kiln import gui
w = gui.MainWindow()
found = [g.id for g in w.games if w._detect(g)]
print(f"window lists {len(w.games)} games; found {len(found)}; first: {[g.name for g in w.games[:3]]}")
assert len(w.games) == 37 and set(found) == set(CASES)
assert w.games[0].id in CASES, "installed games come first"
assert by_id("fallout3").steam_appid == 22300, "found through the other Fallout 3 edition"
w.close()

# ---- install + deploy one mod per game
bad = []
for g in games:
    gd = w._detect(g)
    inst = Instance(g, gd, work / "staging" / g.id); inst.load()
    files, expect = CASES[g.id]
    try:
        prep = inst.prepare(zip_of(g.id, files), "Cool", {"name": "Cool"})
        inst.finish_install(prep)
        res = deploy.deploy(inst)
        g.post_deploy(inst)
    except deploy.DeployError as e:
        if g.id == NO_PREFIX and "Start the game once" in str(e):
            print(f"  ✔ {g.name}: refuses to deploy before the game's first run")
            continue
        bad.append((g.name, f"deploy error: {e}")); continue
    except Exception as e:  # noqa: BLE001
        bad.append((g.name, f"{type(e).__name__}: {e}")); continue
    missing = [p for p in expect if not ((user(g) / p[2:]) if p.startswith("~/") else (gd / p)).exists()]
    print(f"  {'✔' if not missing else '✖'} {g.name}: {res.linked + res.copied} file(s) deployed"
          + (f"; missing {missing}" if missing else ""))
    if missing:
        bad.append((g.name, missing))

# ---- load order files
def text(p):
    return Path(p).read_text(encoding="cp1252", errors="replace")
sky = text(user(by_id("skyrim")) / "AppData/Local/Skyrim/plugins.txt")
print("Skyrim (2011) plugins.txt:", sky.split())
assert "Cool.esp" in sky and "*" not in sky
assert (user(by_id("skyrim")) / "AppData/Local/Skyrim/loadorder.txt").is_file()
nv = text(user(by_id("newvegas")) / "AppData/Local/FalloutNV/plugins.txt").split()
print("New Vegas plugins.txt:", nv)
assert nv[0] == "FalloutNV.esm" and "Cool.esp" in nv
mw = text(fixtures["morrowind"] / "Morrowind.ini")
print("Morrowind.ini:", [l for l in mw.splitlines() if l.startswith("GameFile")], "| [General] kept:", "x=1" in mw)
assert "GameFile0=Morrowind.esm" in mw and "GameFile1=Cool.esp" in mw and "x=1" in mw
md = fixtures["morrowind"] / "Data Files"
assert (md / "Morrowind.esm").stat().st_mtime < (md / "Cool.esp").stat().st_mtime, "dates set in load order"
sf = text(user(by_id("starfield")) / "Documents/My Games/Starfield/StarfieldCustom.ini")
assert "bInvalidateOlderFiles=1" in sf
ptxt = (obv / "Plugins.txt").read_text().split()
print("Oblivion Remastered Plugins.txt:", ptxt)
assert ptxt[:2] == ["Oblivion.esm", "DLCBattlehornCastle.esp"] and "Cool.esp" in ptxt
ls = lsx.read_text()
print("BG3 modsettings.lsx has the mod:", "11111111-2222-3333-4444-555555555555" in ls,
      "| kept GustavX:", "GustavX" in ls, "| backup:", lsx.with_name("modsettings.lsx.kiln-backup").is_file())
assert "11111111-2222-3333-4444-555555555555" in ls and "GustavX" in ls

# ---- loaders: Play's checks know them
st = Instance(by_id("palworld"), fixtures["palworld"], work / "staging/palworld"); st.load()
need = [(ld.name, n) for ld, n in launchguard.needed_loaders(st)]
print("Palworld loaders needed:", need, "| launch options:", launchguard.wanted_launch_options(st, ""))
assert need == [("UE4SS", 1)] and "dwmapi=n,b" in launchguard.wanted_launch_options(st, "")
sfi = Instance(by_id("starfield"), fixtures["starfield"], work / "staging/starfield"); sfi.load()
(fixtures["starfield"] / "sfse_loader.exe").write_bytes(b"MZ")
print("Starfield launch options:", launchguard.wanted_launch_options(sfi, ""))
assert "sfse_loader.exe" in launchguard.wanted_launch_options(sfi, "")

print("problems:", bad)
assert not bad
print("games smoke OK")
