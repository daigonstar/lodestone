"""Hermetic test of "play without mods" (deploy_vanilla): on a fake Skyrim SE with SKSE64 and a
plugin mod, a vanilla deploy keeps only the loader, switches every plugin off in plugins.txt,
leaves the mod list / enabled states / load order untouched, and a normal deploy restores all.
Usage: python3 tests/smoke_vanilla.py <empty work dir>"""
import os, sys, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"),
                  XDG_CONFIG_HOME=str(work / "home/.config"), XDG_CACHE_HOME=str(work / "cache"),
                  XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, deploy, launchguard
lib = work / "lib"
steam.libraries = lambda: [lib]
launchguard.game_running = lambda inst: False
from kiln.games import by_id
from kiln.instance import Instance

TES4 = b"TES4" + b"\0" * 20
g = by_id("skyrimse")
gd = lib / "steamapps/common/Skyrim Special Edition"
(lib / "steamapps").mkdir(parents=True)
(lib / f"steamapps/appmanifest_{g.steam_appid}.acf").write_text(f'"AppState" {{ "appid" "{g.steam_appid}" "installdir" "Skyrim Special Edition" }}')
(gd / g.exe).parent.mkdir(parents=True, exist_ok=True)
(gd / g.exe).write_bytes(b"MZ")
(gd / "Data").mkdir(exist_ok=True)
(gd / "Data/Skyrim.esm").write_bytes(TES4)
(lib / f"steamapps/compatdata/{g.steam_appid}/pfx/drive_c/users/steamuser").mkdir(parents=True)


def zip_of(name, files):
    z = work / f"{name}.zip"
    with zipfile.ZipFile(z, "w") as zz:
        for n, data in files.items():
            zz.writestr(n, data)
    return z


inst = Instance(g, gd, work / "staging"); inst.load()
for name, files in [("SKSE", {"skse64_loader.exe": "MZ", "skse64_1_6_1170.dll": "d", "Data/Scripts/skse.pex": "s"}),
                    ("Cool", {"Data/Cool.esp": TES4, "Data/meshes/a.nif": "n"})]:
    inst.finish_install(inst.prepare(zip_of(name, files), name, {"name": name}))
deploy.deploy(inst); g.post_deploy(inst)
for p in inst.plugins:
    p["enabled"] = True
g.write_plugins_txt(inst, [(p["name"], p["enabled"]) for p in inst.plugins])

before_plugins = [dict(p) for p in inst.plugins]
before_enabled = {m.id: m.enabled for m in inst.mods}
assert (gd / "Data/Cool.esp").exists() and (gd / "skse64_loader.exe").exists()
assert deploy.is_deployed(inst) and not deploy.is_vanilla(inst)

keep = deploy.loader_mod_ids(inst)
print("loader mods kept:", [m.name for m in inst.mods if m.id in keep])
deploy.deploy_vanilla(inst)
ptxt = g.plugins_txt(inst).read_text(encoding="cp1252")
problems = []
if not (gd / "skse64_loader.exe").exists(): problems.append("SKSE loader was removed")
if (gd / "Data/Cool.esp").exists(): problems.append("mod plugin still deployed")
if (gd / "Data/meshes/a.nif").exists(): problems.append("mod files still deployed")
if "*Cool.esp" in ptxt: problems.append("Cool.esp still enabled in plugins.txt")
if inst.plugins != before_plugins: problems.append("plugin load order changed")
if {m.id: m.enabled for m in inst.mods} != before_enabled: problems.append("mod enabled states changed")
if deploy.is_deployed(inst) or not deploy.is_vanilla(inst): problems.append("vanilla state not recorded")
if not (gd / "Data/Skyrim.esm").exists(): problems.append("game file removed")
print("after vanilla:", "OK" if not problems else problems)

deploy.deploy(inst); g.post_deploy(inst)
if not (gd / "Data/Cool.esp").exists(): problems.append("full deploy didn't restore the mod")
if not deploy.is_deployed(inst) or deploy.is_vanilla(inst): problems.append("full deploy didn't clear vanilla")
if "*Cool.esp" not in g.plugins_txt(inst).read_text(encoding="cp1252"): problems.append("plugin not re-enabled")
print("problems:", problems)
assert not problems
print("vanilla smoke OK")
