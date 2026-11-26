"""Hermetic test of the launch guard: Cyberpunk (Amethyst import, CET/RED4ext checks,
launch options, in-game notice, broken deployments, foreign links) and Stardew (SMAPI).
Usage: python3 tests/smoke_launch.py <empty work dir>"""
import io, json, os, sys, time, zipfile
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", KILN_OFFLINE="1", HOME=str(work / "home"), QT_QPA_PLATFORM="offscreen",
                  XDG_CONFIG_HOME=str(work / "home/.config"), XDG_CACHE_HOME=str(work / "cache"),
                  XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, steamcfg, launchguard, importers, deploy
from kiln.games import by_id
from kiln.instance import Instance

lib = work / "lib"; sroot = work / "steamroot"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [sroot]
steamcfg.steam_running = lambda: False
cfg = sroot / "userdata/123/config/localconfig.vdf"; cfg.parent.mkdir(parents=True)
cfg.write_text('"UserLocalConfigStore"\n{\n\t"Software"\n\t{\n\t\t"Valve"\n\t\t{\n\t\t\t"Steam"\n\t\t\t{\n'
               '\t\t\t\t"apps"\n\t\t\t\t{\n\t\t\t\t\t"1091500"\n\t\t\t\t\t{\n\t\t\t\t\t\t"LaunchOptions"\t\t"-modded"\n'
               '\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n')

# ---------------- Cyberpunk ----------------
cp = lib / "steamapps/common/Cyberpunk 2077"; (cp / "bin/x64").mkdir(parents=True)
(cp / "bin/x64/Cyberpunk2077.exe").write_bytes(b"MZ")
inst = Instance(by_id("cyberpunk2077"), cp, lib / "kiln/cyberpunk2077")
# a CET Lua mod and a RED4ext plugin, but no loaders
for rel in ("bin/x64/plugins/cyber_engine_tweaks/mods/SomeLuaMod/init.lua", "red4ext/plugins/TweakXL/TweakXL.dll"):
    f = inst.mods_dir / "UserMods" / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_text("x")
from kiln.instance import Mod
inst.mods.append(Mod(id="UserMods", name="UserMods")); inst.save()
deploy.deploy(inst)
# a leftover link from another manager
other = work / "home/OtherMgr/overwrite/red4ext/config.ini"; other.parent.mkdir(parents=True); other.write_text("cfg")
(cp / "red4ext").mkdir(exist_ok=True); (cp / "red4ext/config.ini").symlink_to(other)

def show(title, checks):
    print(title); [print(f"   [{c.level}] {c.title} | fix={c.fix}") for c in checks]

show("CP before:", launchguard.preflight(inst))
# fake Amethyst with the loaders
am = work / "home/Games/Amethyst/Cyberpunk 2077/mods"
for mod, files in {"CET 1.38": ["bin/x64/version.dll", "bin/x64/plugins/cyber_engine_tweaks.asi"],
                   "RED4ext": ["bin/x64/winmm.dll", "red4ext/RED4ext.dll"]}.items():
    for rel in files:
        f = am / mod / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_text(mod)
    (am / mod / "meta.ini").write_text(f"[General]\nmodid=107\nversion=1.0\nauthor=someone\ncategoryname=Modders Resources\n")
print("amethyst candidates:", [p.name for p in importers.amethyst_candidates(inst)])
importers.import_amethyst(inst, importers.amethyst_candidates(inst))
launchguard.materialize_links(launchguard.foreign_links(inst))
print("created notice mod:", launchguard.ensure_notice_mod(inst))
deploy.deploy(inst)
c = next(c for c in launchguard.preflight(inst) if c.fix == "launch_options")
print("launch options wanted:", c.data["wanted"])
steamcfg.set_launch_options(1091500, c.data["wanted"])
print("written + read back:", steamcfg.get_launch_options(1091500), "| backups:",
      len(list(cfg.parent.glob("*.bak"))))
show("CP after fixes:", launchguard.preflight(inst))
print("config.ini is a real file now:", not (cp / "red4ext/config.ini").is_symlink(),
      (cp / "red4ext/config.ini").read_text())
lua = (cp / "bin/x64/plugins/cyber_engine_tweaks/mods/kiln_notice/init.lua").read_text()
print("notice deployed:", "SetWarningMessage" in lua, "|", lua.splitlines()[1][:110])
# simulate Steam "verify integrity" removing a loader
(cp / "bin/x64/version.dll").unlink()
show("CP after Steam removed version.dll:", [c for c in launchguard.preflight(inst) if c.level != "ok"])
# post-launch log check
t0 = time.time()
log = cp / "bin/x64/plugins/cyber_engine_tweaks/cyber_engine_tweaks.log"; log.parent.mkdir(parents=True, exist_ok=True)
log.write_text("[info] CET version 1.38\n[info] Mod SomeLuaMod loaded! ('...')\n[info] Mod kiln_notice loaded!\n")
(cp / "red4ext/logs").mkdir(parents=True, exist_ok=True)
(cp / "red4ext/logs/red4ext-1.log").write_text("[info] RED4ext started\n[info] TweakXL successfully loaded\n")
for r in launchguard.check_logs(inst, t0):
    print(f"   log {r.loader.name}: started={r.started} ok={r.ok} {r.summary}")

# ---------------- Stardew ----------------
sd = lib / "steamapps/common/Stardew Valley"; sd.mkdir(parents=True)
(sd / "Stardew Valley.dll").write_text("x")
sinst = Instance(by_id("stardewvalley"), sd, lib / "kiln/stardewvalley")
for name in ("ContentPatcher", "[CP] Thing"):
    f = sinst.mods_dir / name / name / "manifest.json"; f.parent.mkdir(parents=True); f.write_text("{}")
    sinst.mods.append(Mod(id=name, name=name))
sinst.save(); deploy.deploy(sinst)
show("Stardew before:", [c for c in launchguard.preflight(sinst) if c.level != "ok"])
# a fake SMAPI installer zip with the real layout
inner = io.BytesIO()
with zipfile.ZipFile(inner, "w") as z:
    z.writestr("StardewModdingAPI", "#!/bin/sh\n"); z.writestr("StardewModdingAPI.dll", "x")
    z.writestr("smapi-internal/config.json", "{}"); z.writestr("Mods/ConsoleCommands/manifest.json", "{}")
zp = work / "SMAPI-4.3.2-installer.zip"
with zipfile.ZipFile(zp, "w") as z:
    z.writestr("SMAPI 4.3.2 installer/internal/linux/install.dat", inner.getvalue())
    z.writestr("SMAPI 4.3.2 installer/install on Linux.sh", "#!/bin/sh")
files = launchguard.install_smapi(sinst, zp)
print("SMAPI files:", files, "| executable:", os.access(sd / "StardewModdingAPI", os.X_OK))
c = next(c for c in launchguard.preflight(sinst) if c.fix == "launch_options")
print("Stardew launch options wanted:", c.data["wanted"])
steamcfg.set_launch_options(413150, c.data["wanted"])
show("Stardew after:", launchguard.preflight(sinst))
slog = work / "home/.config/StardewValley/ErrorLogs/SMAPI-latest.txt"; slog.parent.mkdir(parents=True)
slog.write_text("[SMAPI] Loaded 99 mods:\n...\n")
print("SMAPI log:", [(r.loader.name, r.ok, r.summary) for r in launchguard.check_logs(sinst, time.time() - 5)])
print("vdf still parses, both games set:", steamcfg.loads(cfg.read_text())["UserLocalConfigStore"]["Software"]["Valve"]["Steam"]["apps"].keys())
